//! `PermissionEngine` — the rule-table stage (stage 1 of 3) of the
//! permission pipeline (design doc `02-tools-permissions.md` §5).
//!
//! Pipeline recap: **(1)** rule table (this module, no interaction) →
//! **(2)** plugin hook gate ([`ToolCallGate`], reserved for M2) →
//! **(3)** ask-the-user prompt (UI callback, M1 headless reads stdin).
//! Any Deny/Block surfaces to the model as
//! [`ToolError::PermissionDenied`](crate::tool::ToolError::PermissionDenied),
//! not as a process error.
//!
//! M1 scope (per `07-m1-plan.md` T3): rule-level evaluation only — `Ask`
//! is returned as-is and the caller decides how to prompt.

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The decision produced by evaluating the permission rules.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum PermissionAction {
    /// Proceed without prompting.
    Allow,
    /// Refuse; the model receives a permission error.
    Deny,
    /// Ask the user (TUI prompt; headless: stdin / settings fallback).
    Ask,
    /// No decision was made — the pipeline should continue with the next
    /// stage (hook gate, then default policy). Only meaningful as an
    /// engine default, never as a rule action.
    #[default]
    NoMatch,
}

/// The action a single rule prescribes. A matched rule always decides.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RuleAction {
    Allow,
    Deny,
    Ask,
}

impl From<RuleAction> for PermissionAction {
    fn from(action: RuleAction) -> Self {
        match action {
            RuleAction::Allow => PermissionAction::Allow,
            RuleAction::Deny => PermissionAction::Deny,
            RuleAction::Ask => PermissionAction::Ask,
        }
    }
}

/// Where a rule comes from. Metadata in M1 — it does not affect matching;
/// the session layer (T5) and settings use it for rule persistence and
/// expiry (`Once` rules are consumed after first use, later).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Scope {
    /// Project-local rules (`.mcode/` config).
    #[default]
    Project,
    /// User-global rules (`~/.mcode/`).
    User,
    /// Rules granted for the current session.
    Session,
    /// One-shot grant for a single call.
    Once,
}

/// One permission rule: `tool(arg_pattern) → action`.
///
/// `arg_pattern` is a [globset] glob matched against the tool's *salient
/// argument* (see [`arg_of`]): for `bash` the command string, for
/// file tools the path. Glob `*` crosses path separators by default
/// (globset semantics), so `*.env` matches `config/prod.env` and
/// `**/*.env` also matches a bare `.env` at the root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionRule {
    /// Exact tool name the rule applies to (e.g. `"bash"`).
    pub tool: String,
    /// Glob matched against the salient argument.
    pub arg_pattern: String,
    /// What to do when the rule matches.
    pub action: RuleAction,
    /// Provenance of the rule (metadata in M1).
    pub scope: Scope,
}

impl PermissionRule {
    /// Convenience constructor with project scope.
    pub fn new(
        tool: impl Into<String>,
        arg_pattern: impl Into<String>,
        action: RuleAction,
    ) -> Self {
        Self {
            tool: tool.into(),
            arg_pattern: arg_pattern.into(),
            action,
            scope: Scope::Project,
        }
    }
}

/// Extract the salient argument that permission rules match against, per
/// builtin tool. Returns `None` for tools without a known extractor —
/// the engine then matches against the empty string, so only `*`
/// patterns apply.
///
/// Builtins: `bash` → `command`; `read`/`write`/`edit` → `path`;
/// `grep` → `pattern`. M2 plugins register their own extractors via the
/// plugin API.
pub fn arg_of(tool_name: &str, args: &Value) -> Option<String> {
    let key = match tool_name {
        "bash" => "command",
        "read" | "write" | "edit" => "path",
        "grep" => "pattern",
        _ => return None,
    };
    args.get(key).and_then(Value::as_str).map(str::to_owned)
}

/// Compile-and-match a glob. Malformed patterns never match (documented
/// footgun; rule sources are validated when they start coming from user
/// settings in M2).
fn glob_matches(pattern: &str, value: &str) -> bool {
    globset::Glob::new(pattern)
        .map(|glob| glob.compile_matcher().is_match(value))
        .unwrap_or(false)
}

/// **Reserved M2 extension point** — the plugin hook gate between rule
/// evaluation (stage 1) and the permission prompt (stage 3) of the
/// permission pipeline (`02-tools-permissions.md` §5; `03-plugins.md`
/// §4.2 event `tool_call`, Gate semantics: rewrite arguments / block).
///
/// M1 stores a [`ToolCallGate`] in [`PermissionEngine::hook_runner`] but
/// never invokes it; the dispatch pipeline (`mcode-agent`, M2) will call
/// `gate` after the rule table yields `Ask`/`NoMatch` and before the user
/// is prompted. The full `HookRunner` (plugin-host, `03-plugins.md`)
/// implements this trait.
#[async_trait]
pub trait ToolCallGate: Send + Sync {
    /// Inspect — and possibly rewrite in place — the arguments of a tool
    /// call about to be dispatched.
    async fn gate(&self, tool_name: &str, args: &mut Value) -> GateResult;
}

/// Outcome of a [`ToolCallGate::gate`] call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum GateResult {
    /// No objection; continue the pipeline (arguments possibly rewritten).
    Pass,
    /// Block the call. The reason is surfaced to the model as
    /// [`ToolError::PermissionDenied`](crate::tool::ToolError::PermissionDenied).
    Block(String),
}

/// Rule-table permission evaluator.
///
/// Matching semantics: rules are consulted **in listed order and the
/// first matching rule wins** — later rules do *not* override earlier
/// ones; list order is priority order. A rule matches when its tool name
/// equals the called tool exactly and its `arg_pattern` glob matches the
/// tool's salient argument. When no rule matches, [`evaluate`] returns
/// the engine's `default_action` (M1 default: [`PermissionAction::Allow`]).
///
/// [`evaluate`]: PermissionEngine::evaluate
pub struct PermissionEngine {
    rules: Vec<PermissionRule>,
    default_action: PermissionAction,
    /// **Reserved M2 call point** (see [`ToolCallGate`]): the gate that
    /// runs between rule evaluation and the user prompt. M1 leaves this
    /// `None` everywhere; `07-m1-plan.md` §M2 衔接 fills it.
    pub hook_runner: Option<Arc<dyn ToolCallGate>>,
}

impl PermissionEngine {
    /// M1 default rules: `bash(*)` → Ask; everything else → Allow
    /// (`07-m1-plan.md` risk table, "bash 安全").
    pub fn new() -> Self {
        Self::with_rules(vec![PermissionRule::new("bash", "*", RuleAction::Ask)])
    }

    /// An engine with custom rules and the M1 default fallback (Allow).
    pub fn with_rules(rules: Vec<PermissionRule>) -> Self {
        Self {
            rules,
            default_action: PermissionAction::Allow,
            hook_runner: None,
        }
    }

    /// Rules in evaluation order (first match wins).
    pub fn rules(&self) -> &[PermissionRule] {
        &self.rules
    }

    /// Change the action returned when no rule matches. Set to
    /// [`PermissionAction::NoMatch`] to delegate everything unmatched to
    /// the hook/prompt stages.
    pub fn set_default_action(&mut self, action: PermissionAction) {
        self.default_action = action;
    }

    /// The action used when no rule matches (M1 default: Allow).
    pub fn default_action(&self) -> PermissionAction {
        self.default_action
    }

    /// Evaluate the rules for a tool call: first-match-wins in listed
    /// order; falls back to [`Self::default_action`] when nothing
    /// matches. Pure function of (rules, tool, args) — no I/O, no
    /// prompting (that is stages 2–3).
    pub fn evaluate(&self, tool_name: &str, args: &Value) -> PermissionAction {
        for rule in &self.rules {
            if rule.tool != tool_name {
                continue;
            }
            let arg = arg_of(tool_name, args).unwrap_or_default();
            if glob_matches(&rule.arg_pattern, &arg) {
                return rule.action.into();
            }
        }
        self.default_action
    }
}

impl Default for PermissionEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn rule(tool: &str, pattern: &str, action: RuleAction) -> PermissionRule {
        PermissionRule::new(tool, pattern, action)
    }

    fn engine(rules: Vec<PermissionRule>) -> PermissionEngine {
        PermissionEngine::with_rules(rules)
    }

    #[test]
    fn m1_default_rules() {
        let engine = PermissionEngine::new();
        // bash asks, regardless of the command.
        assert_eq!(
            engine.evaluate("bash", &json!({"command": "cargo test"})),
            PermissionAction::Ask
        );
        assert_eq!(
            engine.evaluate("bash", &json!({"command": "rm -rf /"})),
            PermissionAction::Ask
        );
        // Everything else allows.
        assert_eq!(
            engine.evaluate("read", &json!({"path": "Cargo.toml"})),
            PermissionAction::Allow
        );
        assert_eq!(
            engine.evaluate("write", &json!({"path": "src/main.rs", "content": "x"})),
            PermissionAction::Allow
        );
        // Unknown tools also fall through to the default.
        assert_eq!(
            engine.evaluate("mcp:server:tool", &json!({})),
            PermissionAction::Allow
        );
    }

    /// Table-driven: (rules, tool, args) → expected action.
    #[test]
    fn rule_matching_table() {
        struct Case {
            name: &'static str,
            rules: Vec<PermissionRule>,
            tool: &'static str,
            args: Value,
            expect: PermissionAction,
        }
        let allow_bash_cargo = rule("bash", "cargo *", RuleAction::Allow);
        let ask_bash_rm = rule("bash", "rm *", RuleAction::Ask);
        let deny_env = rule("write", "**/*.env", RuleAction::Deny);

        let cases = vec![
            Case {
                name: "allow bash cargo",
                rules: vec![allow_bash_cargo.clone()],
                tool: "bash",
                args: json!({"command": "cargo build --release"}),
                expect: PermissionAction::Allow,
            },
            Case {
                name: "glob requires a suffix: 'cargo *' does not match bare 'cargo'",
                rules: vec![allow_bash_cargo.clone()],
                tool: "bash",
                args: json!({"command": "cargo"}),
                expect: PermissionAction::Allow, // falls through to default
            },
            Case {
                name: "deny nested and bare .env",
                rules: vec![deny_env.clone()],
                tool: "write",
                args: json!({"path": "config/prod.env", "content": ""}),
                expect: PermissionAction::Deny,
            },
            Case {
                name: "deny bare .env at root (**/ matches zero dirs)",
                rules: vec![deny_env.clone()],
                tool: "write",
                args: json!({"path": ".env", "content": ""}),
                expect: PermissionAction::Deny,
            },
            Case {
                name: "non-env write unaffected",
                rules: vec![deny_env.clone()],
                tool: "write",
                args: json!({"path": "src/main.rs", "content": ""}),
                expect: PermissionAction::Allow,
            },
            Case {
                name: "tool name must match exactly",
                rules: vec![deny_env.clone()],
                tool: "edit",
                args: json!({"path": "prod.env", "old_string": "a", "new_string": "b"}),
                expect: PermissionAction::Allow,
            },
            Case {
                name: "first match wins over later rules",
                rules: vec![
                    rule("bash", "*", RuleAction::Deny),
                    allow_bash_cargo.clone(),
                ],
                tool: "bash",
                args: json!({"command": "cargo test"}),
                expect: PermissionAction::Deny,
            },
            Case {
                name: "later rule only reached when earlier does not match",
                rules: vec![allow_bash_cargo.clone(), ask_bash_rm.clone()],
                tool: "bash",
                args: json!({"command": "rm -rf /tmp/x"}),
                expect: PermissionAction::Ask,
            },
            Case {
                name: "no rules → default action",
                rules: vec![],
                tool: "bash",
                args: json!({"command": "anything"}),
                expect: PermissionAction::Allow,
            },
            Case {
                name: "missing salient arg matches only '*' patterns",
                rules: vec![ask_bash_rm.clone()],
                tool: "bash",
                args: json!({"path": "not a command"}),
                expect: PermissionAction::Allow,
            },
            Case {
                name: "wrong-typed salient arg treated as absent",
                rules: vec![ask_bash_rm.clone()],
                tool: "bash",
                args: json!({"command": 42}),
                expect: PermissionAction::Allow,
            },
            Case {
                name: "malformed glob never matches",
                rules: vec![rule("bash", "[unclosed", RuleAction::Deny)],
                tool: "bash",
                args: json!({"command": "cargo test"}),
                expect: PermissionAction::Allow,
            },
            Case {
                name: "grep matches on pattern",
                rules: vec![rule("grep", "secret*", RuleAction::Deny)],
                tool: "grep",
                args: json!({"pattern": "secrets/keys"}),
                expect: PermissionAction::Deny,
            },
            Case {
                name: "'*' crosses separators (globset default)",
                rules: vec![rule("read", "*.env", RuleAction::Deny)],
                tool: "read",
                args: json!({"path": "deep/nested/.env"}),
                expect: PermissionAction::Deny,
            },
        ];

        for case in cases {
            let engine = engine(case.rules);
            assert_eq!(
                engine.evaluate(case.tool, &case.args),
                case.expect,
                "case {:?} failed",
                case.name
            );
        }
    }

    #[test]
    fn default_action_can_be_changed() {
        let mut engine = engine(vec![]);
        engine.set_default_action(PermissionAction::Ask);
        assert_eq!(engine.default_action(), PermissionAction::Ask);
        assert_eq!(
            engine.evaluate("read", &json!({"path": "x"})),
            PermissionAction::Ask
        );

        engine.set_default_action(PermissionAction::NoMatch);
        assert_eq!(
            engine.evaluate("read", &json!({"path": "x"})),
            PermissionAction::NoMatch
        );
    }

    #[test]
    fn rules_kept_in_order_and_serializable() {
        let rules = vec![allow(), deny()];
        let engine = engine(rules.clone());
        assert_eq!(engine.rules(), rules.as_slice());

        let json = serde_json::to_string(&rules[0]).unwrap();
        let back: PermissionRule = serde_json::from_str(&json).unwrap();
        assert_eq!(back, rules[0]);

        fn allow() -> PermissionRule {
            PermissionRule::new("bash", "cargo *", RuleAction::Allow)
        }
        fn deny() -> PermissionRule {
            PermissionRule {
                tool: "write".into(),
                arg_pattern: "**/*.env".into(),
                action: RuleAction::Deny,
                scope: Scope::User,
            }
        }
    }

    #[tokio::test]
    async fn hook_runner_is_reserved_but_callable() {
        // M2 extension point: stored, unused by evaluate(), directly
        // invocable — proving the call point shape works.
        struct NoSecrets;

        #[async_trait]
        impl ToolCallGate for NoSecrets {
            async fn gate(&self, _tool_name: &str, args: &mut Value) -> GateResult {
                if let Some(cmd) = args.get("command").and_then(Value::as_str) {
                    if cmd.contains("secret") {
                        return GateResult::Block("secrets are off limits".into());
                    }
                    *args = json!({"command": format!("{cmd} # gated")});
                }
                GateResult::Pass
            }
        }

        let mut engine = engine(vec![rule("bash", "cargo *", RuleAction::Allow)]);
        assert!(engine.hook_runner.is_none());
        engine.hook_runner = Some(Arc::new(NoSecrets));

        // evaluate() ignores the gate in M1.
        assert_eq!(
            engine.evaluate("bash", &json!({"command": "cargo test"})),
            PermissionAction::Allow
        );

        // The gate itself rewrites / blocks as designed.
        let gate = engine.hook_runner.as_ref().unwrap();
        let mut ok_args = json!({"command": "cargo test"});
        assert_eq!(gate.gate("bash", &mut ok_args).await, GateResult::Pass);
        assert_eq!(ok_args["command"], "cargo test # gated");

        let mut bad_args = json!({"command": "cat secret.txt"});
        assert_eq!(
            gate.gate("bash", &mut bad_args).await,
            GateResult::Block("secrets are off limits".into())
        );
    }
}

//! `HookRunner` — the plugin hook dispatch point called at every node the
//! design docs mark (design doc `03-plugins.md` §4; `01-agent-core.md` §3).
//!
//! **M1 placeholder**: all three methods pass through untouched. The agent
//! loop already calls them at the documented nodes, so M2 only has to fill
//! in the real plugin-host implementation behind the same signatures —
//! no loop changes needed (`07-m1-plan.md` §M2 衔接).
//!
//! The three dispatch semantics (pi's model):
//!
//! * [`notify`](HookRunner::notify) — fire-and-forget broadcast.
//! * [`transform`](HookRunner::transform) — middleware chain: value in,
//!   possibly rewritten value out.
//! * [`gate`](HookRunner::gate) — may rewrite the payload in place and/or
//!   block the action ([`GateResult::Block`]).

use serde_json::Value;
use std::sync::Arc;

/// Outcome of a [`HookRunner::gate`] call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateResult {
    /// No objection; continue (payload possibly rewritten).
    Pass,
    /// Block the action. The reason is surfaced to the model as an
    /// `is_error` tool result (or ignored for `StopGate` in M1).
    Block(String),
}

/// The loop node a hook is being invoked at (the agent-loop rows of the
/// v0.1 event table, `03-plugins.md` §4.2). Payload-free in M1; M2
/// enriches events with their JSON payloads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookEvent {
    /// A turn started (Notify).
    TurnStart,
    /// A turn ended (Notify).
    TurnEnd,
    /// User input is about to enter the context — prompt, steer, or
    /// follow-up (Transform).
    UserPrompt,
    /// Before every LLM request (Transform: may rewrite the request).
    BeforeProviderRequest,
    /// An assistant message started streaming (Notify).
    MessageStart,
    /// An assistant message finished streaming (Transform: may rewrite
    /// the whole message before it enters history).
    MessageEnd,
    /// A tool call is about to be dispatched (Gate: may rewrite arguments
    /// or block).
    ToolCall,
    /// A tool result is about to be written back into the context
    /// (Transform: redaction, summarization, truncation).
    ToolResult,
    /// The agent is about to stop (Gate: plugins may block the stop and
    /// inject follow-ups).
    StopGate,
}

/// Empty placeholder hook runner (M1). Every method is a pass-through:
/// [`notify`](HookRunner::notify) does nothing, [`transform`](HookRunner::transform)
/// returns its value unchanged, and [`gate`](HookRunner::gate) always
/// passes except when tests install [`HookRunner::with_test_gate`], which
/// may rewrite arguments or block. M2 replaces the internals with the
/// plugin host; the loop-side call points stay fixed.
type TestGate = Arc<dyn Fn(&mut Value) -> GateResult + Send + Sync>;

pub struct HookRunner {
    test_gate: Option<TestGate>,
}

impl HookRunner {
    /// An empty runner.
    pub fn new() -> Self {
        Self { test_gate: None }
    }

    /// Install a tool-call gate used by tests to rewrite or block arguments.
    pub fn with_test_gate(
        mut self,
        gate: impl Fn(&mut Value) -> GateResult + Send + Sync + 'static,
    ) -> Self {
        self.test_gate = Some(Arc::new(gate));
        self
    }
}

impl Default for HookRunner {
    fn default() -> Self {
        Self::new()
    }
}

impl HookRunner {
    /// Broadcast an event; the return value is ignored (Notify).
    pub async fn notify(&self, _event: HookEvent) {}

    /// Middleware: potentially rewrite `value` on its way through the
    /// loop node (Transform). M1 passes the value through untouched.
    pub async fn transform<T>(&self, _event: HookEvent, value: T) -> T {
        value
    }

    /// Inspect — and possibly rewrite in place — a `payload`, and either
    /// pass or block the action (Gate). Production M1 always passes; tests
    /// may install a gate that rewrites or blocks.
    ///
    /// Call sites: `ToolCall` payloads are the call's arguments (may be
    /// rewritten before execution); `StopGate` payloads are reserved for
    /// M2 follow-up injection (`Value::Null` today).
    pub async fn gate(&self, event: HookEvent, payload: &mut Value) -> GateResult {
        if event == HookEvent::ToolCall
            && let Some(gate) = &self.test_gate
        {
            return gate(payload);
        }
        GateResult::Pass
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn placeholder_methods_pass_through() {
        let hooks = HookRunner::new();
        hooks.notify(HookEvent::TurnStart).await;
        assert_eq!(
            hooks
                .transform(HookEvent::UserPrompt, "unchanged".to_string())
                .await,
            "unchanged"
        );
        let mut payload = serde_json::json!({"command": "ls"});
        assert_eq!(
            hooks.gate(HookEvent::ToolCall, &mut payload).await,
            GateResult::Pass
        );
        assert_eq!(payload, serde_json::json!({"command": "ls"}));
        // Default constructible (the loop stores it in TurnEnv).
        let _hooks: HookRunner = Default::default();
    }
}

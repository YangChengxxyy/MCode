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

use mcode_tools::permission::GateResult;
use serde_json::Value;

/// The loop node a hook is being invoked at (the agent-loop rows of the
/// v0.1 event table, `03-plugins.md` §4.2). Payload-free in M1; M2
/// enriches events with their JSON payloads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookEvent {
    /// A turn started / ended (Notify).
    TurnStart,
    /// A turn started / ended (Notify).
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
    /// A tool call is about to be dispatched — permission stage 2 (Gate:
    /// may rewrite arguments or block).
    ToolCall,
    /// A tool result is about to be written back into the context
    /// (Transform: redaction, summarization, truncation).
    ToolResult,
    /// The agent is about to stop (Gate: plugins may block the stop and
    /// inject follow-ups).
    StopGate,
    /// A permission decision was requested from / resolved by the user
    /// (Notify; telemetry).
    PermissionRequested,
    /// A permission decision was requested from / resolved by the user
    /// (Notify; telemetry).
    PermissionResolved,
}

/// Empty placeholder hook runner (M1). Every method is a pass-through:
/// [`notify`](HookRunner::notify) does nothing, [`transform`](HookRunner::transform)
/// returns its value unchanged, and [`gate`](HookRunner::gate) always
/// passes. M2 replaces the internals with the plugin host; the loop-side
/// call points stay fixed.
pub struct HookRunner;

impl HookRunner {
    /// An empty runner.
    pub fn new() -> Self {
        Self
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
    /// pass or block the action (Gate). M1 always passes.
    ///
    /// Call sites: `ToolCall` payloads are the call's arguments (may be
    /// rewritten before execution); `StopGate` payloads are reserved for
    /// M2 follow-up injection (`Value::Null` today).
    pub async fn gate(&self, _event: HookEvent, _payload: &mut Value) -> GateResult {
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

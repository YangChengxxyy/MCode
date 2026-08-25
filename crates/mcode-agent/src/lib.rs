//! `mcode-agent` — the UI-free, session-free agent double loop
//! (M1 T4; design doc `01-agent-core.md` §3, adopting pi's
//! `runAgentLoop` structure).
//!
//! ```text
//! caller ──Message──► Agent::prompt(msg, &TurnEnv)
//!                        │
//!                        ▼ double loop
//!        outer: drain follow-up queue whenever the agent would stop
//!          inner: build request → provider.stream → mirror deltas as
//!                 SessionEvents → dispatch tool calls (3-stage
//!                 permission pipeline) → write results back
//!                        │
//!                        ▼
//!        steer queue drained after every response cycle (jumps the
//!        queue ahead of follow-ups); abort() / env.cancel ends the
//!        turn with TurnOutcome::Aborted
//! ```
//!
//! * [`Agent`] owns the conversation state and the steer/follow-up
//!   queues; [`AgentHandle`] lets other tasks steer, follow up, or
//!   abort while a turn streams.
//! * [`TurnEnv`] injects everything ambient — provider, tool registry,
//!   permission engine, hooks, cancellation, the event bus, and the
//!   stage-3 [`PermissionPrompt`] callback ([`AllowAll`] / [`DenyAll`]
//!   ship as the two trivial wirings).
//! * [`HookRunner`] is the M1 placeholder for the plugin hook host: the
//!   loop already calls `notify` / `transform` / `gate` at every node
//!   the design docs mark; all three pass through until M2.

pub mod agent;
pub mod env;
pub mod hooks;
mod turn;

pub use agent::{Agent, AgentConfig, AgentHandle, AgentState, QueueMode};
pub use env::{AllowAll, DenyAll, PermissionPrompt, PermissionRequest, TurnEnv};
pub use hooks::{HookEvent, HookRunner};

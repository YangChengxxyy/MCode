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
//!                 SessionEvents → dispatch registered tools → write
//!                 results back
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
//!   hooks, cancellation, and the event bus. Registered schema-valid
//!   tools execute directly; no permission callback is required.
//! * [`HookRunner`] is the M1 placeholder for the plugin hook host: the
//!   loop already calls `notify` / `transform` / `gate` at every node
//!   the design docs mark. Production M1 passes through; tests may install
//!   a tool-call gate that rewrites arguments or blocks.

pub mod agent;
pub mod env;
pub mod hooks;
mod turn;

pub use agent::{Agent, AgentConfig, AgentHandle, AgentState, QueueMode};
pub use env::TurnEnv;
pub use hooks::{GateResult, HookEvent, HookRunner};

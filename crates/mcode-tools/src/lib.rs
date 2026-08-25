//! `mcode-tools` — the MCode tool system: `Tool` trait, registry,
//! permission engine, and builtin tools (design doc
//! `02-tools-permissions.md`; M1 T3).
//!
//! ```text
//! model tool_call ──► ToolRegistry::get ──► PermissionEngine::evaluate
//!                        (ToolDyn)             (Allow / Deny / Ask)
//!                                                  │ Ask → returned to the caller
//!                                                  ▼
//!                              ToolDyn::execute_dyn(args, ToolCtx, ToolStream)
//!                              │ validate against schemars schema
//!                              │ execute typed Tool
//!                              ▼
//!                              ToolResult { content → LLM, details → UI }
//! ```
//!
//! * Every tool derives **one** JSON Schema via `schemars`, used both for
//!   the LLM tool spec and runtime argument validation.
//! * [`ToolRegistry`] is last-wins per name, so plugins can override
//!   builtins.
//! * [`PermissionEngine`] is rule-level only in M1: `Ask` is returned to
//!   the caller; the hook gate (stage 2) is reserved via
//!   [`PermissionEngine::hook_runner`] for M2.
//! * Builtin tools (read/write/edit/bash/grep) double as the reference
//!   implementation for the plugin tool API.

pub mod builtin;
pub mod ctx;
pub mod permission;
pub mod registry;
pub mod stream;
pub mod tool;

pub use builtin::{
    BashTool, EditTool, GrepTool, ReadTool, WriteTool, builtin_tools, register_builtins,
};
pub use ctx::ToolCtx;
pub use permission::{
    GateResult, PermissionAction, PermissionEngine, PermissionRule, RuleAction, Scope,
    ToolCallGate, arg_of,
};
pub use registry::ToolRegistry;
pub use stream::{ToolProgress, ToolStream, ToolStreamItem, ToolStreamReceiver};
pub use tool::{Concurrency, Tool, ToolDyn, ToolError, ToolResult};

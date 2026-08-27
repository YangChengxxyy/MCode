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
//! * Trusted builtin tools (read/write/edit/bash/grep/find) provide the
//!   minimal recovery surface and cannot depend on external search binaries.

pub mod builtin;
pub mod ctx;
pub mod permission;
pub mod registry;
pub mod stream;
pub mod tool;

pub use builtin::fs_io::{
    FileAccess, FileRead, FileRevision, FileSnapshot, FileWrite, PreparedFile, prepare_file,
    prepare_file_async, read_file, read_file_async, read_file_snapshot, read_file_snapshot_async,
    write_file, write_file_async,
};
pub use builtin::fs_search::{
    PreparedSearch, SearchAccess, live_search_thread_handles, live_search_workers, prepare_search,
    prepare_search_async, prepare_search_async_with_access, prepare_search_with_access,
    run_search_worker_until_cancel,
};
pub use builtin::{
    BashTool, EditTool, FindTool, GrepTool, ReadTool, WriteTool, builtin_tools, register_builtins,
};
pub use ctx::ToolCtx;
pub use permission::{
    GateResult, PermissionAction, PermissionEngine, PermissionRule, RuleAction, Scope,
    ToolCallGate, arg_of,
};
pub use registry::ToolRegistry;
pub use stream::{ToolProgress, ToolStream, ToolStreamItem, ToolStreamReceiver};
pub use tool::{Concurrency, Tool, ToolDyn, ToolError, ToolResult};

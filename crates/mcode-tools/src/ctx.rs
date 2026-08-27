//! `ToolCtx` — the per-invocation execution context handed to every tool
//! (design doc `02-tools-permissions.md` §4).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use mcode_core::events::SessionEvent;
use mcode_core::ids::{CallId, SessionId};
use tokio_util::sync::CancellationToken;

use crate::builtin::fs_io::PreparedFile;
use crate::builtin::fs_search::PreparedSearch;

/// Everything a tool needs to know about the call it is executing.
///
/// Kept deliberately small for M1: cwd, ids, cancellation, optional event
/// emission, and an optional prepared search capability. Tools must treat the
/// context as read-only ambient state.
#[derive(Clone)]
pub struct ToolCtx {
    /// Working directory of the session; relative tool paths resolve
    /// against it (see [`ToolCtx::resolve`]).
    pub cwd: PathBuf,
    /// Session the call belongs to.
    pub session_id: SessionId,
    /// The tool call being executed.
    pub call_id: CallId,
    /// Cooperative cancellation; long-running tools should poll it and
    /// abort early.
    pub cancel: CancellationToken,
    /// Optional sink for extra UI events a tool wants to emit
    /// (design doc §4 `emit_event`). `None` in M1's builtin tools; kept
    /// so later integration does not change the struct shape.
    pub emit_event: Option<Arc<dyn Fn(SessionEvent) + Send + Sync>>,
    /// Ready grep/find root bound at dispatch preflight, if any.
    ///
    /// Present only after a successful prepare. Execution takes that root
    /// once and never re-resolves it.
    pub prepared_search: Option<Arc<PreparedSearch>>,
    /// Ready file capability bound at dispatch preflight, if any.
    ///
    /// Host-owned. Execution takes the inner handles once and never
    /// re-resolves. Not exposed to WASM.
    pub prepared_file: Option<Arc<PreparedFile>>,
}

impl ToolCtx {
    /// A context with a fresh token, no event emitter, and no prepared search.
    pub fn new(cwd: impl Into<PathBuf>, session_id: SessionId, call_id: CallId) -> Self {
        Self {
            cwd: cwd.into(),
            session_id,
            call_id,
            cancel: CancellationToken::new(),
            emit_event: None,
            prepared_search: None,
            prepared_file: None,
        }
    }

    /// Replace the cancellation token (builder style).
    pub fn with_cancel(mut self, cancel: CancellationToken) -> Self {
        self.cancel = cancel;
        self
    }

    /// Bind a ready preflight grep/find root (builder style).
    pub fn with_prepared_search(mut self, prepared: Arc<PreparedSearch>) -> Self {
        self.prepared_search = Some(prepared);
        self
    }

    /// Bind a ready preflight file capability (builder style).
    pub fn with_prepared_file(mut self, prepared: Arc<PreparedFile>) -> Self {
        self.prepared_file = Some(prepared);
        self
    }

    /// Set the event emitter (builder style).
    pub fn with_emitter(mut self, emit: Arc<dyn Fn(SessionEvent) + Send + Sync>) -> Self {
        self.emit_event = Some(emit);
        self
    }

    /// Resolve a tool-supplied path against the session cwd: absolute
    /// paths pass through, relative paths are joined onto `cwd`.
    pub fn resolve(&self, path: &str) -> PathBuf {
        let p = Path::new(path);
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            self.cwd.join(p)
        }
    }

    /// Fire the optional event emitter; a no-op when unset.
    pub fn emit(&self, event: SessionEvent) {
        if let Some(emit) = &self.emit_event {
            emit(event);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mcode_core::events::MessageDelta;
    use std::sync::Mutex;

    fn ctx() -> ToolCtx {
        ToolCtx::new("/tmp/mcode", SessionId::from("s1"), CallId::from("c1"))
    }

    #[test]
    fn resolves_relative_paths_against_cwd() {
        assert_eq!(
            ctx().resolve("a/b.txt"),
            PathBuf::from("/tmp/mcode/a/b.txt")
        );
        assert_eq!(ctx().resolve("/abs/b.txt"), PathBuf::from("/abs/b.txt"));
    }

    #[test]
    fn defaults_are_uncancelled_and_silent() {
        let ctx = ctx();
        assert!(!ctx.cancel.is_cancelled());
        assert!(ctx.emit_event.is_none());
        // emit is a no-op when unset — must not panic.
        ctx.emit(SessionEvent::TurnStarted);
    }

    #[test]
    fn emitter_receives_events() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = {
            let seen = Arc::clone(&seen);
            Arc::new(move |ev: SessionEvent| seen.lock().unwrap().push(ev))
        };
        let ctx = ctx().with_emitter(sink);
        let event = SessionEvent::MessageDelta(MessageDelta::TextDelta("x".into()));
        ctx.emit(event.clone());
        assert_eq!(*seen.lock().unwrap(), vec![event]);
    }

    #[test]
    fn with_cancel_replaces_token() {
        let token = CancellationToken::new();
        token.cancel();
        let ctx = ctx().with_cancel(token);
        assert!(ctx.cancel.is_cancelled());
    }
}

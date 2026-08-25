//! The per-turn environment: everything one turn of the agent loop needs
//! from its surroundings (design doc `01-agent-core.md` §3), plus the
//! stage-3 permission callback.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use mcode_core::events::SessionEvent;
use mcode_core::ids::SessionId;
use mcode_llm::Provider;
use mcode_tools::ToolRegistry;
use mcode_tools::permission::PermissionEngine;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use crate::hooks::HookRunner;

/// A permission decision request handed to a [`PermissionPrompt`].
///
/// Mirrors the telemetry payload of
/// [`SessionEvent::PermissionRequested`](mcode_core::SessionEvent::PermissionRequested);
/// UIs answer asynchronously and return the decision as a plain `bool`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PermissionRequest {
    /// Correlation id shared by the `PermissionRequested` /
    /// `PermissionResolved` session events.
    pub request_id: String,
    /// Tool the model wants to call.
    pub tool_name: String,
    /// (Possibly hook-rewritten) call arguments.
    pub arguments: Value,
}

/// Permission pipeline stage 3 (`02-tools-permissions.md` §5): the
/// "ask the user" callback. Injected via [`TurnEnv`] so each front end
/// wires its own implementation — a TUI shows a confirmation dialog, the
/// headless CLI reads stdin, tests inject [`AllowAll`] / [`DenyAll`].
#[async_trait]
pub trait PermissionPrompt: Send + Sync {
    /// Present the request; `true` allows the call exactly once, `false`
    /// denies it (the model receives a permission-error tool result).
    async fn prompt(&self, req: PermissionRequest) -> bool;
}

/// [`PermissionPrompt`] that approves every request (yolo wiring).
pub struct AllowAll;

#[async_trait]
impl PermissionPrompt for AllowAll {
    async fn prompt(&self, _req: PermissionRequest) -> bool {
        true
    }
}

/// [`PermissionPrompt`] that denies every request — the safe default
/// wiring for [`TurnEnv::new`].
pub struct DenyAll;

#[async_trait]
impl PermissionPrompt for DenyAll {
    async fn prompt(&self, _req: PermissionRequest) -> bool {
        false
    }
}

/// Everything one turn needs: the model provider, tools, permissions,
/// hooks, cancellation, the event bus, and the ask-the-user callback.
///
/// The agent itself stays UI-free and session-free: all ambient
/// dependencies flow in through this struct, freshly borrowable per
/// `prompt()` call. `cancel` is the *caller's* token; the agent derives
/// a child token from it per turn so its own `abort()` can fire the same
/// cancellation without owning the parent.
pub struct TurnEnv<'a> {
    /// LLM provider to stream from.
    pub provider: &'a dyn Provider,
    /// Tool registry the model's calls dispatch through.
    pub tools: &'a ToolRegistry,
    /// Permission rule engine (pipeline stage 1).
    pub permissions: &'a PermissionEngine,
    /// Plugin hook runner (pipeline stage 2 + loop-node hooks).
    pub hooks: &'a HookRunner,
    /// Cooperative turn cancellation. Firing it aborts the in-flight
    /// turn: the current stream terminates with `Cancelled`, the turn
    /// ends with [`TurnOutcome::Aborted`], and state stays consistent.
    ///
    /// [`TurnOutcome::Aborted`]: mcode_core::events::TurnOutcome::Aborted
    pub cancel: CancellationToken,
    /// Fan-out bus for session events (UI, telemetry, tests subscribe).
    pub events: broadcast::Sender<SessionEvent>,
    /// Permission stage 3: how `Ask` decisions resolve.
    pub permission_prompt: Arc<dyn PermissionPrompt>,
    /// Working directory tools resolve relative paths against.
    pub cwd: PathBuf,
    /// Session the turn belongs to (flows into `ToolCtx`).
    pub session_id: SessionId,
}

impl<'a> TurnEnv<'a> {
    /// Wire up an environment with safe defaults: a fresh cancellation
    /// token, a private 256-slot event channel, [`DenyAll`] for `Ask`
    /// decisions, the process cwd, and a fresh session id. Override with
    /// the `with_*` builders.
    pub fn new(
        provider: &'a dyn Provider,
        tools: &'a ToolRegistry,
        permissions: &'a PermissionEngine,
        hooks: &'a HookRunner,
    ) -> Self {
        Self {
            provider,
            tools,
            permissions,
            hooks,
            cancel: CancellationToken::new(),
            events: broadcast::channel(256).0,
            permission_prompt: Arc::new(DenyAll),
            cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            session_id: SessionId::new(),
        }
    }

    /// Use this cancellation token (builder style).
    pub fn with_cancel(mut self, cancel: CancellationToken) -> Self {
        self.cancel = cancel;
        self
    }

    /// Publish session events on this broadcast sender (builder style).
    pub fn with_events(mut self, events: broadcast::Sender<SessionEvent>) -> Self {
        self.events = events;
        self
    }

    /// Resolve `Ask` permission decisions with this callback (builder
    /// style).
    pub fn with_permission_prompt(mut self, prompt: Arc<dyn PermissionPrompt>) -> Self {
        self.permission_prompt = prompt;
        self
    }

    /// Set the tool working directory (builder style).
    pub fn with_cwd(mut self, cwd: impl Into<PathBuf>) -> Self {
        self.cwd = cwd.into();
        self
    }

    /// Set the session id (builder style).
    pub fn with_session_id(mut self, session_id: SessionId) -> Self {
        self.session_id = session_id;
        self
    }
}

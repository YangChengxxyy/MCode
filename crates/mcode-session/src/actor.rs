//! The session actor (design doc `01-agent-core.md` §4): a tokio task
//! owning one [`Agent`], its JSONL store, and its conversation tree —
//! the boundary between the UI/CLI world (commands in, events out) and
//! the agent loop.
//!
//! ```text
//! UI / CLI ──SessionCommand──► SessionHandle ──mpsc──► SessionActor (task)
//!    ▲                                                      │
//!    └──────────── broadcast ◄───────────────────────────────┤ each turn runs
//!                 SessionEvent                               ▼ mcode-agent's
//!                                                 Agent::prompt(msg, &TurnEnv)
//! ```
//!
//! # Actor ↔ Agent boundary
//!
//! The agent stays UI-free and session-free (T4); the actor adds
//! everything session-shaped:
//!
//! * **Persistence** — after every turn the actor diffs the agent's
//!   message history against the persisted prefix of the *current
//!   branch* and appends the new tail as tree entries (`MessageAdded`
//!   equivalence without broadcast-capacity loss: what the loop
//!   produced is exactly what lands in the file).
//! * **Concurrency** — while a turn runs, the actor still services
//!   `Steer` / `FollowUp` / `Abort` by forwarding them to the agent's
//!   [`AgentHandle`](mcode_agent::AgentHandle) (they need to reach the
//!   loop mid-stream). `Prompt` / `Fork` / `Resume` arriving mid-turn
//!   are deferred and processed after it ends — prompts serialize,
//!   steers jump the queue.
//! * **Fork/Resume** — `Fork` re-points the append tip and rewinds the
//!   in-memory history to the fork point (the file keeps both
//!   branches); `Resume` reloads state from disk, discarding whatever
//!   was only in memory.
//!
//! # Assembly indirection
//!
//! Everything ambient a turn needs (provider, tools, permissions,
//! hooks, ask-the-user callback, cwd, cancellation) is passed in
//! explicitly via [`SessionEnv`] behind `Arc`s, so the actor can
//! borrow a fresh [`TurnEnv`] from them on every turn. The
//! [`AgentFactory`] closure adds indirection for *building* the agent
//! itself (tests and front ends inject custom agents); the crate
//! depends only on the `dyn Provider` abstraction, never on a concrete
//! provider.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use mcode_agent::{Agent, AgentConfig, DenyAll, HookRunner, PermissionPrompt, TurnEnv};
use mcode_core::McodeError;
use mcode_core::events::{SessionCommand, SessionEvent};
use mcode_core::ids::{MessageId, SessionId};
use mcode_core::message::Message;
use mcode_llm::Provider;
use mcode_tools::ToolRegistry;
use mcode_tools::permission::PermissionEngine;
use tokio::sync::{broadcast, mpsc, watch};
use tokio_util::sync::CancellationToken;

use crate::paths;
use crate::store::{SessionEntry, SessionHeader, SessionStore, load_session};
use crate::tree::SessionTree;

/// Command-channel capacity per session.
const COMMAND_CHANNEL_CAPACITY: usize = 64;

/// Builds a session's [`Agent`] from its config — the injection point
/// for tests and front ends (`Arc<dyn Fn>` so it crosses into the
/// actor task). The default is plain `Agent::new`
/// ([`default_agent_factory`]).
pub type AgentFactory = Arc<dyn Fn(AgentConfig) -> Agent + Send + Sync>;

/// The default factory: a plain [`Agent::new`] agent.
pub fn default_agent_factory() -> AgentFactory {
    Arc::new(Agent::new)
}

/// Everything one session needs from its surroundings, owned behind
/// `Arc`s so the actor can borrow a fresh [`TurnEnv`] per turn. This is
/// the explicit assembly the caller (CLI, TUI, test) wires — the
/// session crate never constructs a concrete provider.
pub struct SessionEnv {
    /// LLM provider to stream from (`dyn` — any implementation).
    pub provider: Arc<dyn Provider>,
    /// Tool registry the model's calls dispatch through.
    pub tools: Arc<ToolRegistry>,
    /// Permission rule engine (pipeline stage 1).
    pub permissions: Arc<PermissionEngine>,
    /// Plugin hook runner (M1 placeholder).
    pub hooks: Arc<HookRunner>,
    /// Permission stage 3: how `Ask` decisions resolve (default
    /// [`DenyAll`] — the safe wiring).
    pub permission_prompt: Arc<dyn PermissionPrompt>,
    /// Working directory tools resolve relative paths against; also
    /// selects the session directory (`~/.mcode/sessions/<cwd-slug>`).
    pub cwd: PathBuf,
    /// Session-level cancellation: firing it aborts the in-flight turn.
    pub cancel: CancellationToken,
}

impl SessionEnv {
    /// Wire an environment with safe defaults (fresh permission
    /// engine, placeholder hooks, `DenyAll`, process cwd, fresh
    /// token); override with the `with_*` builders.
    pub fn new(provider: Arc<dyn Provider>, tools: Arc<ToolRegistry>) -> Self {
        Self {
            provider,
            tools,
            permissions: Arc::new(PermissionEngine::new()),
            hooks: Arc::new(HookRunner::new()),
            permission_prompt: Arc::new(DenyAll),
            cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            cancel: CancellationToken::new(),
        }
    }

    /// Use this permission engine (builder style).
    pub fn with_permissions(mut self, permissions: Arc<PermissionEngine>) -> Self {
        self.permissions = permissions;
        self
    }

    /// Use this hook runner (builder style).
    pub fn with_hooks(mut self, hooks: Arc<HookRunner>) -> Self {
        self.hooks = hooks;
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

    /// Use this cancellation token (builder style).
    pub fn with_cancel(mut self, cancel: CancellationToken) -> Self {
        self.cancel = cancel;
        self
    }
}

/// Identity facts about the live session (shared between the actor and
/// its handle; `Resume` swaps them).
#[derive(Debug, Clone)]
pub struct SessionMeta {
    /// The session currently loaded by the actor.
    pub session_id: SessionId,
    /// The JSONL file the actor appends to.
    pub path: PathBuf,
}

/// The public handle to a session actor (design doc §4 shape):
/// commands in via mpsc, events out via broadcast.
pub struct SessionHandle {
    cmd_tx: mpsc::Sender<SessionCommand>,
    events: broadcast::Receiver<SessionEvent>,
    events_tx: broadcast::Sender<SessionEvent>,
    processed: watch::Receiver<u64>,
    meta: Arc<RwLock<SessionMeta>>,
    join: tokio::task::JoinHandle<()>,
}

impl SessionHandle {
    /// Start a brand-new session: fresh id, file at
    /// `~/.mcode/sessions/<cwd-slug>/<timestamp>_<uuid>.jsonl`
    /// (`$MCODE_HOME` override applies). Must be called inside a
    /// tokio runtime (the actor is spawned here).
    pub fn new(
        env: SessionEnv,
        agent_config: AgentConfig,
        factory: AgentFactory,
    ) -> Result<Self, McodeError> {
        let path = paths::new_session_file(&env.cwd);
        Self::new_at(path, env, agent_config, factory)
    }

    /// Start a new session writing to an explicit path (tests, exotic
    /// layouts).
    pub fn new_at(
        path: impl Into<PathBuf>,
        env: SessionEnv,
        agent_config: AgentConfig,
        factory: AgentFactory,
    ) -> Result<Self, McodeError> {
        let path = path.into();
        let session_id = SessionId::new();
        let header = SessionHeader::new(session_id.clone(), env.cwd.to_string_lossy().into_owned());
        let store = SessionStore::create(&path, &header)?;
        let agent = factory(agent_config.clone());
        let actor = SessionActor {
            session_id,
            path,
            agent,
            agent_config,
            factory,
            env,
            store,
            tree: SessionTree::new(),
            tip: None,
            persisted_count: 0,
            events: broadcast::channel(256).0,
            processed: watch::channel(0).0,
            seq: 0,
            deferred: VecDeque::new(),
            meta: None,
        };
        Ok(actor.spawn())
    }

    /// Resume a persisted session by path or session id: load the file,
    /// rebuild the agent's message history from the latest branch, and
    /// continue appending to the same file.
    pub fn resume(
        spec: &str,
        env: SessionEnv,
        agent_config: AgentConfig,
        factory: AgentFactory,
    ) -> Result<Self, McodeError> {
        let Some(path) = paths::resolve_session(spec) else {
            return Err(McodeError::Session(format!(
                "no session found for '{spec}' (neither an existing file nor a known session id)"
            )));
        };
        Self::resume_path(path, env, agent_config, factory)
    }

    /// Resume a persisted session by explicit file path.
    pub fn resume_path(
        path: impl Into<PathBuf>,
        env: SessionEnv,
        agent_config: AgentConfig,
        factory: AgentFactory,
    ) -> Result<Self, McodeError> {
        let path = path.into();
        let loaded = load_session(&path)?;
        let tree = SessionTree::from_entries(loaded.entries);
        let tip = tree.latest_leaf();
        let messages = tip
            .as_ref()
            .and_then(|t| tree.messages_to(t))
            .unwrap_or_default();
        let mut agent = factory(agent_config.clone());
        agent.state_mut().messages = messages.clone();
        let store = SessionStore::open(&path)?;
        let actor = SessionActor {
            session_id: loaded.header.session_id,
            path,
            agent,
            agent_config,
            factory,
            env,
            store,
            tree,
            tip,
            persisted_count: messages.len(),
            events: broadcast::channel(256).0,
            processed: watch::channel(0).0,
            seq: 0,
            deferred: VecDeque::new(),
            meta: None,
        };
        Ok(actor.spawn())
    }

    /// Send a command to the actor.
    pub async fn send(&self, cmd: SessionCommand) -> Result<(), McodeError> {
        self.cmd_tx
            .send(cmd)
            .await
            .map_err(|_| McodeError::Session("session actor is not running".into()))
    }

    /// Start a new turn from a user prompt.
    pub async fn prompt(&self, msg: Message) -> Result<(), McodeError> {
        self.send(SessionCommand::Prompt(msg)).await
    }

    /// Queue a steering message (jumps the queue at the next boundary).
    pub async fn steer(&self, msg: Message) -> Result<(), McodeError> {
        self.send(SessionCommand::Steer(msg)).await
    }

    /// Queue a follow-up (delivered when the agent would stop).
    pub async fn follow_up(&self, msg: Message) -> Result<(), McodeError> {
        self.send(SessionCommand::FollowUp(msg)).await
    }

    /// Abort the in-flight turn.
    pub async fn abort(&self) -> Result<(), McodeError> {
        self.send(SessionCommand::Abort).await
    }

    /// Fork the conversation tree at an entry id.
    pub async fn fork(&self, at: MessageId) -> Result<(), McodeError> {
        self.send(SessionCommand::Fork { at }).await
    }

    /// Wait until the actor has fully processed `n` commands in total
    /// (turn finished *and* history persisted). The deterministic
    /// "everything hit the disk" barrier.
    pub async fn wait_processed(&self, n: u64) -> Result<(), McodeError> {
        let mut rx = self.processed.clone();
        rx.wait_for(|&processed| processed >= n)
            .await
            .map(|_| ())
            .map_err(|_| McodeError::Session("session actor stopped before processing".into()))
    }

    /// A fresh subscription to the session's event stream.
    pub fn subscribe(&self) -> broadcast::Receiver<SessionEvent> {
        self.events_tx.subscribe()
    }

    /// The handle's own event receiver (design doc §4 field).
    pub fn events_mut(&mut self) -> &mut broadcast::Receiver<SessionEvent> {
        &mut self.events
    }

    /// The session the actor currently has loaded (`Resume` swaps it).
    pub fn session_id(&self) -> SessionId {
        self.read_meta().session_id
    }

    /// The file the actor currently appends to.
    pub fn path(&self) -> PathBuf {
        self.read_meta().path
    }

    fn read_meta(&self) -> SessionMeta {
        self.meta
            .read()
            .map(|guard| guard.clone())
            .unwrap_or_else(|_| SessionMeta {
                session_id: SessionId::from("unknown"),
                path: PathBuf::from("unknown"),
            })
    }

    /// Consume the handle and join the actor task (dropping the handle
    /// also ends the actor once the command channel drains).
    pub fn shutdown(self) -> tokio::task::JoinHandle<()> {
        self.join
    }
}

/// The session actor: one task owning the agent, the store, and the
/// tree. Spawned via [`SessionActor::spawn`]; drive it through
/// [`SessionHandle`].
pub(crate) struct SessionActor {
    session_id: SessionId,
    path: PathBuf,
    agent: Agent,
    agent_config: AgentConfig,
    factory: AgentFactory,
    env: SessionEnv,
    store: SessionStore,
    tree: SessionTree,
    /// The entry id the next append names as parent.
    tip: Option<MessageId>,
    /// How many leading messages of the agent's history are already
    /// persisted on the current branch.
    persisted_count: usize,
    events: broadcast::Sender<SessionEvent>,
    /// Command counter published after each command is fully handled
    /// (persistence included).
    processed: watch::Sender<u64>,
    seq: u64,
    /// Commands deferred while a turn was streaming.
    deferred: VecDeque<SessionCommand>,
    meta: Option<Arc<RwLock<SessionMeta>>>,
}

impl SessionActor {
    fn spawn(mut self) -> SessionHandle {
        let (cmd_tx, cmd_rx) = mpsc::channel(COMMAND_CHANNEL_CAPACITY);
        let (processed_tx, processed_rx) = watch::channel(0u64);
        self.processed = processed_tx;
        let meta = Arc::new(RwLock::new(SessionMeta {
            session_id: self.session_id.clone(),
            path: self.path.clone(),
        }));
        self.meta = Some(Arc::clone(&meta));
        let events_tx = self.events.clone();
        let events_rx = events_tx.subscribe();
        let join = tokio::spawn(self.run(cmd_rx));
        SessionHandle {
            cmd_tx,
            events: events_rx,
            events_tx,
            processed: processed_rx,
            meta,
            join,
        }
    }

    /// The command loop: deferred commands first (received mid-turn
    /// and queued), then the channel.
    async fn run(mut self, mut cmd_rx: mpsc::Receiver<SessionCommand>) {
        loop {
            let cmd = match self.deferred.pop_front() {
                Some(cmd) => cmd,
                None => match cmd_rx.recv().await {
                    Some(cmd) => cmd,
                    None => break,
                },
            };
            self.handle_command(cmd, &mut cmd_rx).await;
            self.seq += 1;
            self.publish_progress();
        }
        // Commands deferred during the final turn still run.
        while let Some(cmd) = self.deferred.pop_front() {
            self.handle_command(cmd, &mut cmd_rx).await;
            self.seq += 1;
            self.publish_progress();
        }
    }

    fn publish_progress(&self) {
        let _ = self.processed.send(self.seq);
    }

    async fn handle_command(
        &mut self,
        cmd: SessionCommand,
        cmd_rx: &mut mpsc::Receiver<SessionCommand>,
    ) {
        match cmd {
            SessionCommand::Prompt(msg) => self.run_prompt_turn(msg, cmd_rx).await,
            // Queued while idle: delivered before the first response of
            // the next turn (agent semantics).
            SessionCommand::Steer(msg) => self.agent.steer(msg),
            SessionCommand::FollowUp(msg) => self.agent.follow_up(msg),
            SessionCommand::Abort => self.agent.abort(),
            SessionCommand::Fork { at } => self.fork(at),
            SessionCommand::Resume { session } => self.resume_session(session),
        }
    }

    /// Run one turn, intercepting commands while it streams: steer /
    /// follow-up / abort forward straight to the agent handle; prompt /
    /// fork / resume wait for the turn to end. Afterwards the history
    /// tail is persisted.
    async fn run_prompt_turn(&mut self, msg: Message, cmd_rx: &mut mpsc::Receiver<SessionCommand>) {
        let provider = Arc::clone(&self.env.provider);
        let tools = Arc::clone(&self.env.tools);
        let permissions = Arc::clone(&self.env.permissions);
        let hooks = Arc::clone(&self.env.hooks);
        let permission_prompt = Arc::clone(&self.env.permission_prompt);
        let env = TurnEnv::new(&*provider, &tools, &permissions, &hooks)
            .with_events(self.events.clone())
            .with_cancel(self.env.cancel.clone())
            .with_permission_prompt(permission_prompt)
            .with_cwd(self.env.cwd.clone())
            .with_session_id(self.session_id.clone());

        let handle = self.agent.handle();
        let outcome = {
            let mut turn = std::pin::pin!(self.agent.prompt(msg, &env));
            let mut channel_open = true;
            loop {
                tokio::select! {
                    result = &mut turn => break result,
                    cmd = cmd_rx.recv(), if channel_open => match cmd {
                        Some(SessionCommand::Steer(msg)) => handle.steer(msg),
                        Some(SessionCommand::FollowUp(msg)) => handle.follow_up(msg),
                        Some(SessionCommand::Abort) => handle.abort(),
                        Some(deferred) => self.deferred.push_back(deferred),
                        None => channel_open = false,
                    },
                }
            }
        };
        if let Err(err) = &outcome {
            // The agent already broadcast Error + TurnEnded(Aborted);
            // log for the operator's trace.
            tracing::warn!(session = %self.session_id, %err, "session turn failed");
        }
        self.persist_new_messages();
    }

    /// Append every message the loop produced since the last persist:
    /// the suffix of the agent history beyond [`Self::persisted_count`]
    /// (identical to reacting to each `MessageAdded` event, but
    /// immune to broadcast capacity loss).
    fn persist_new_messages(&mut self) {
        let messages: Vec<Message> = self.agent.state().messages.clone();
        let mut index = self.persisted_count;
        while index < messages.len() {
            let id = MessageId::new();
            let entry =
                SessionEntry::from_message(id.clone(), self.tip.clone(), messages[index].clone());
            if let Err(err) = self.store.append(&entry) {
                let _ = self
                    .events
                    .send(SessionEvent::Error(McodeError::Session(format!(
                        "failed to persist session entry: {err}"
                    ))));
                // persisted_count stays put: the next turn retries this
                // suffix (a partially written line is skipped on load).
                break;
            }
            self.tree.insert(entry);
            self.tip = Some(id);
            index += 1;
        }
        self.persisted_count = index;
    }

    /// Fork the conversation at `at`: the append tip moves there and
    /// the in-memory history rewinds to the fork point's replay. The
    /// file keeps both branches; queued steer/follow-ups survive (they
    /// have not entered history yet).
    fn fork(&mut self, at: MessageId) {
        match self.tree.fork_at(&at) {
            Ok(fork) => {
                if let Some(messages) = self.tree.messages_to(fork.id()) {
                    self.tip = Some(fork.id().clone());
                    self.persisted_count = messages.len();
                    self.agent.state_mut().messages = messages;
                }
            }
            Err(err) => self.emit_error(err),
        }
    }

    /// Reload session state from disk: the current file when `session`
    /// is this session, otherwise the file found for that id. Anything
    /// that was only in memory (an in-flight fork not yet re-appended)
    /// is replaced by the persisted truth.
    fn resume_session(&mut self, session: SessionId) {
        let path = if session == self.session_id {
            self.path.clone()
        } else {
            match paths::find_session_by_id(&paths::sessions_root(), &session) {
                Some(path) => path,
                None => {
                    self.emit_error(McodeError::Session(format!(
                        "cannot resume: no session file found for id {session}"
                    )));
                    return;
                }
            }
        };
        self.reload_from(&path);
    }

    fn reload_from(&mut self, path: &Path) {
        let loaded = match load_session(path) {
            Ok(loaded) => loaded,
            Err(err) => {
                self.emit_error(err);
                return;
            }
        };
        let tree = SessionTree::from_entries(loaded.entries);
        let tip = tree.latest_leaf();
        let messages = tip
            .as_ref()
            .and_then(|t| tree.messages_to(t))
            .unwrap_or_default();
        match SessionStore::open(path) {
            Ok(store) => self.store = store,
            Err(err) => {
                self.emit_error(err);
                return;
            }
        }
        let mut agent = (self.factory)(self.agent_config.clone());
        agent.state_mut().messages = messages.clone();
        self.agent = agent;
        self.tree = tree;
        self.tip = tip;
        self.persisted_count = messages.len();
        self.session_id = loaded.header.session_id;
        self.path = path.to_path_buf();
        if let Some(meta) = &self.meta {
            if let Ok(mut guard) = meta.write() {
                guard.session_id = self.session_id.clone();
                guard.path = self.path.clone();
            }
        }
    }

    fn emit_error(&self, err: McodeError) {
        let _ = self.events.send(SessionEvent::Error(err));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_agent_factory_builds_plain_agents() {
        let agent = default_agent_factory()(AgentConfig::new("m"));
        assert!(agent.state().messages.is_empty());
        assert!(!agent.state().is_streaming);
    }

    #[test]
    fn session_env_builders_override_defaults() {
        struct UnusedProvider;
        #[async_trait::async_trait]
        impl Provider for UnusedProvider {
            fn id(&self) -> &str {
                "unused"
            }
            async fn stream(
                &self,
                _request: &mcode_llm::Request,
                _cancel: CancellationToken,
            ) -> Result<mcode_llm::EventStream, mcode_llm::LlmError> {
                Err(mcode_llm::LlmError::Config("unused".into()))
            }
        }
        let provider: Arc<dyn Provider> = Arc::new(UnusedProvider);
        let env = SessionEnv::new(provider, Arc::new(ToolRegistry::new()))
            .with_cwd("/tmp/project")
            .with_cancel(CancellationToken::new());
        assert_eq!(env.cwd, PathBuf::from("/tmp/project"));
        assert!(!env.cancel.is_cancelled());
    }
}

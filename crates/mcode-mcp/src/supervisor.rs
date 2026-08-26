//! Per-server actors, bounded reconnect, and transactional catalog publication.

// Rust guideline compliant 2026-08-20.

use std::{
    collections::{BTreeMap, BTreeSet},
    io::Write,
    sync::Arc,
    time::Duration,
};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::{
    sync::{Mutex, RwLock, broadcast, mpsc, watch},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

use crate::{
    catalog::{CatalogParts, CatalogSection, CatalogSnapshot, Generation},
    config::{McpPluginConfig, ServerConfig},
    error::{Error, ErrorKind, Recovery, Result},
    host::{AuthHostHandle, LogLevel, McpHostHandle},
    identity::{NamespacedId, ServerName},
    protocol::{
        Capability, ConnectContext, McpConnectorHandle, McpSessionHandle, NegotiatedServer, Page,
        PromptGetOutcome, RemotePrompt, RemoteResource, RemoteResourceTemplate, RemoteTool,
        RequestControl, ResourceReadOutcome, SessionEvent, SessionEventSink, ToolCallOutcome,
    },
    validation::{
        sanitize_json, validate_completion_result, validate_prompt_result,
        validate_resource_result, validate_tool_arguments, validate_tool_result,
    },
};

/// Maximum event slots when configured payloads are small.
const SERVER_EVENT_MAX_CAPACITY: usize = 128;
/// Payload budget for each actor event queue.
///
/// This retains several default-size task updates without multiplying a 64 MiB
/// configured event by the former 128-slot capacity. One event larger than
/// this budget still receives one slot so delivery remains possible.
const SERVER_EVENT_BUFFER_BYTES: usize = 16 * 1024 * 1024;
/// Maximum extra bytes added when text truncation appends `…`.
const UTF8_ELLIPSIS_BYTES: usize = 3;

fn server_event_capacity(config: &ServerConfig) -> usize {
    let limits = &config.output_limits;
    let max_payload_bytes = limits
        .max_output_bytes
        .max(limits.max_log_bytes)
        .max(limits.max_string_bytes.saturating_add(UTF8_ELLIPSIS_BYTES));
    let max_event_bytes = max_payload_bytes.saturating_add(std::mem::size_of::<SessionEvent>());
    let slots = SERVER_EVENT_BUFFER_BYTES
        .checked_div(max_event_bytes.max(1))
        .unwrap_or(0)
        .clamp(1, SERVER_EVENT_MAX_CAPACITY);
    // Tokio broadcast rounds capacity up to a power of two, so pass the lower power.
    let rounded = slots.checked_next_power_of_two().unwrap_or(slots);
    if rounded > slots {
        rounded / 2
    } else {
        rounded
    }
}

/// Fine-grained startup progress for one server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub enum InitProgress {
    /// Resolving secret references through the host.
    ResolvingConfiguration,
    /// Opening a direct child or HTTP transport.
    ConnectingTransport,
    /// Negotiating protocol version and capabilities.
    Negotiating,
    /// Fully paginating and validating catalogs.
    LoadingCatalog,
    /// Establishing current-protocol notification subscriptions.
    Subscribing,
}

/// Independent lifecycle phase for one server actor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub enum ServerPhase {
    /// Configuration disabled this server.
    Disabled,
    /// Connection or initialization is in progress.
    Connecting,
    /// Session and immutable catalog are ready.
    Ready,
    /// Session remains usable but a transactional refresh failed.
    Degraded,
    /// Waiting for a bounded reconnect delay.
    BackingOff,
    /// Fatal failure or reconnect budget exhaustion.
    Failed,
    /// Graceful shutdown is in progress.
    ShuttingDown,
    /// Actor and transport have stopped.
    Stopped,
}

/// Sanitized error payload suitable for a server status view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerErrorPayload {
    /// Stable error classification.
    pub kind: ErrorKind,
    /// Whether a new connection may recover.
    pub recovery: Recovery,
    /// Sanitized bounded message.
    pub message: String,
}

impl From<&Error> for ServerErrorPayload {
    fn from(error: &Error) -> Self {
        Self {
            kind: error.kind(),
            recovery: error.recovery(),
            message: error.message().to_owned(),
        }
    }
}

/// Immutable status payload published by one actor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerState {
    /// Server identity.
    pub server: ServerName,
    /// Current lifecycle phase.
    pub phase: ServerPhase,
    /// Current initialization step.
    pub init_progress: Option<InitProgress>,
    /// Current zero-based reconnect attempt.
    pub reconnect_attempt: u32,
    /// Last transactionally committed catalog generation.
    pub generation: Generation,
    /// Negotiated peer information when connected.
    pub negotiated: Option<NegotiatedServer>,
    /// Last sanitized failure without credentials or remote bodies.
    pub last_error: Option<ServerErrorPayload>,
}

impl ServerState {
    fn new(server: ServerName, phase: ServerPhase) -> Self {
        Self {
            server,
            phase,
            init_progress: None,
            reconnect_attempt: 0,
            generation: Generation::default(),
            negotiated: None,
            last_error: None,
        }
    }
}

/// Sanitized asynchronous notifications published by one server actor.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum ServerEvent {
    /// A subscribed resource changed.
    ResourceUpdated {
        /// Sanitized resource URI from the server notification.
        uri: String,
    },
    /// One negotiated task changed status.
    TaskStatus(Value),
}

struct ServerRuntime {
    name: ServerName,
    config: ServerConfig,
    state: watch::Sender<ServerState>,
    session: RwLock<Option<McpSessionHandle>>,
    catalog: RwLock<Arc<CatalogSnapshot>>,
    events: broadcast::Sender<ServerEvent>,
    shutdown: CancellationToken,
}

/// Cloneable request and status handle for one isolated server actor.
#[derive(Clone)]
pub struct ServerHandle {
    runtime: Arc<ServerRuntime>,
}

impl std::fmt::Debug for ServerHandle {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ServerHandle")
            .field("server", &self.runtime.name)
            .finish_non_exhaustive()
    }
}

impl ServerHandle {
    /// Returns the server identity.
    #[must_use]
    pub fn name(&self) -> &ServerName {
        &self.runtime.name
    }

    /// Returns the latest independent state payload.
    #[must_use]
    pub fn state(&self) -> ServerState {
        self.runtime.state.borrow().clone()
    }

    /// Subscribes to state changes.
    #[must_use]
    pub fn subscribe_state(&self) -> watch::Receiver<ServerState> {
        self.runtime.state.subscribe()
    }

    /// Subscribes to sanitized resource and task notifications.
    ///
    /// Receivers report lag through [`broadcast::error::RecvError::Lagged`] when
    /// they cannot keep up with the byte-budget-derived actor channel.
    #[must_use]
    pub fn subscribe_events(&self) -> broadcast::Receiver<ServerEvent> {
        self.runtime.events.subscribe()
    }

    /// Waits until this server is ready or terminally failed.
    ///
    /// # Errors
    ///
    /// Returns on failure, shutdown, or timeout without affecting other servers.
    pub async fn wait_ready(&self, timeout: Duration) -> Result<ServerState> {
        let server = self.runtime.name.clone();
        let mut receiver = self.runtime.state.subscribe();
        tokio::time::timeout(timeout, async move {
            loop {
                let state = receiver.borrow().clone();
                match state.phase {
                    ServerPhase::Ready | ServerPhase::Degraded => return Ok(state),
                    ServerPhase::Failed | ServerPhase::Disabled | ServerPhase::Stopped => {
                        return Err(Error::new(
                            ErrorKind::Unavailable,
                            Recovery::Fatal,
                            state.last_error.map_or_else(
                                || "MCP server is unavailable".to_owned(),
                                |e| e.message,
                            ),
                        )
                        .with_server(server.clone()));
                    }
                    _ => {}
                }
                receiver.changed().await.map_err(|_| {
                    Error::new(
                        ErrorKind::Unavailable,
                        Recovery::Fatal,
                        "MCP server actor stopped before becoming ready",
                    )
                    .with_server(server.clone())
                })?;
            }
        })
        .await
        .map_err(|_| {
            Error::new(
                ErrorKind::Timeout,
                Recovery::Recoverable,
                "timed out waiting for MCP server readiness",
            )
            .with_server(self.runtime.name.clone())
        })?
    }

    /// Returns the current immutable catalog snapshot.
    pub async fn catalog(&self) -> Arc<CatalogSnapshot> {
        self.runtime.catalog.read().await.clone()
    }

    /// Calls a namespaced tool exactly once.
    ///
    /// A timeout, disconnect, or interrupted process never retries this call,
    /// regardless of server annotations or apparent idempotence.
    ///
    /// # Errors
    ///
    /// Returns unavailable, provenance, schema, capability, transport, or
    /// protocol errors from this server only.
    pub async fn call_tool(
        &self,
        id: &NamespacedId,
        arguments: Value,
        control: RequestControl,
    ) -> Result<ToolCallOutcome> {
        self.check_identity(id)?;
        let catalog = self.catalog().await;
        let tool = catalog.tool(id).ok_or_else(|| {
            Error::new(
                ErrorKind::Unavailable,
                Recovery::Fatal,
                "tool identity is not in the current catalog generation",
            )
            .with_server(self.runtime.name.clone())
        })?;
        let arguments = validate_tool_arguments(
            &self.runtime.name,
            &tool.input_schema,
            &arguments,
            &self.runtime.config.output_limits,
        )?;
        let outcome = self
            .session()
            .await?
            .inner()
            .call_tool(&tool.remote_name, arguments, control)
            .await?;
        match outcome {
            ToolCallOutcome::Complete { result, is_error } => Ok(ToolCallOutcome::Complete {
                result: validate_tool_result(
                    &self.runtime.name,
                    &result,
                    &self.runtime.config.output_limits,
                )?,
                is_error,
            }),
            ToolCallOutcome::Task { task } => Ok(ToolCallOutcome::Task {
                task: sanitize_json(
                    &self.runtime.name,
                    &task,
                    &self.runtime.config.output_limits,
                )?,
            }),
            ToolCallOutcome::InputRequired { request } => Ok(ToolCallOutcome::InputRequired {
                request: sanitize_json(
                    &self.runtime.name,
                    &request,
                    &self.runtime.config.output_limits,
                )?,
            }),
        }
    }

    /// Reads one resource URI.
    pub async fn read_resource(
        &self,
        uri: &str,
        control: RequestControl,
    ) -> Result<ResourceReadOutcome> {
        let outcome = self
            .session()
            .await?
            .inner()
            .read_resource(uri, control)
            .await?;
        match outcome {
            ResourceReadOutcome::Complete { result } => Ok(ResourceReadOutcome::Complete {
                result: validate_resource_result(
                    &self.runtime.name,
                    &result,
                    &self.runtime.config.output_limits,
                )?,
            }),
            ResourceReadOutcome::InputRequired { request } => {
                Ok(ResourceReadOutcome::InputRequired {
                    request: sanitize_json(
                        &self.runtime.name,
                        &request,
                        &self.runtime.config.output_limits,
                    )?,
                })
            }
        }
    }

    /// Subscribes to one resource URI.
    pub async fn subscribe_resource(&self, uri: &str) -> Result<()> {
        self.session().await?.inner().subscribe_resource(uri).await
    }

    /// Unsubscribes from one resource URI.
    pub async fn unsubscribe_resource(&self, uri: &str) -> Result<()> {
        self.session()
            .await?
            .inner()
            .unsubscribe_resource(uri)
            .await
    }

    /// Gets a namespaced prompt with string arguments.
    pub async fn get_prompt(
        &self,
        id: &NamespacedId,
        arguments: BTreeMap<String, String>,
        control: RequestControl,
    ) -> Result<PromptGetOutcome> {
        self.check_identity(id)?;
        let catalog = self.catalog().await;
        let prompt = catalog.prompt(id).ok_or_else(|| {
            Error::new(
                ErrorKind::Unavailable,
                Recovery::Fatal,
                "prompt identity is not in the current catalog generation",
            )
            .with_server(self.runtime.name.clone())
        })?;
        let outcome = self
            .session()
            .await?
            .inner()
            .get_prompt(&prompt.remote_name, arguments, control)
            .await?;
        match outcome {
            PromptGetOutcome::Complete { result } => Ok(PromptGetOutcome::Complete {
                result: validate_prompt_result(
                    &self.runtime.name,
                    &result,
                    &self.runtime.config.output_limits,
                )?,
            }),
            PromptGetOutcome::InputRequired { request } => Ok(PromptGetOutcome::InputRequired {
                request: sanitize_json(
                    &self.runtime.name,
                    &request,
                    &self.runtime.config.output_limits,
                )?,
            }),
        }
    }

    /// Requests prompt or resource argument completion.
    pub async fn complete(&self, request: Value, control: RequestControl) -> Result<Value> {
        let result = self
            .session()
            .await?
            .inner()
            .complete(request, control)
            .await?;
        validate_completion_result(
            &self.runtime.name,
            &result,
            &self.runtime.config.output_limits,
        )
    }

    /// Sets the negotiated legacy logging level.
    pub async fn set_log_level(&self, level: LogLevel) -> Result<()> {
        self.session().await?.inner().set_log_level(level).await
    }

    /// Announces roots changes only when the negotiated protocol supports it.
    pub async fn notify_roots_changed(&self) -> Result<()> {
        self.session().await?.inner().notify_roots_changed().await
    }

    /// Polls one negotiated task.
    pub async fn get_task(&self, task_id: &str, control: RequestControl) -> Result<Value> {
        self.session()
            .await?
            .inner()
            .get_task(task_id, control)
            .await
    }

    /// Supplies responses to one negotiated task.
    pub async fn update_task(
        &self,
        task_id: &str,
        responses: Value,
        control: RequestControl,
    ) -> Result<()> {
        self.session()
            .await?
            .inner()
            .update_task(task_id, responses, control)
            .await
    }

    /// Cancels one negotiated task.
    pub async fn cancel_task(&self, task_id: &str, control: RequestControl) -> Result<()> {
        self.session()
            .await?
            .inner()
            .cancel_task(task_id, control)
            .await
    }

    /// Requests actor shutdown.
    pub fn request_shutdown(&self) {
        self.runtime.shutdown.cancel();
    }

    async fn session(&self) -> Result<McpSessionHandle> {
        self.runtime.session.read().await.clone().ok_or_else(|| {
            Error::new(
                ErrorKind::Unavailable,
                Recovery::Recoverable,
                "MCP server is not ready",
            )
            .with_server(self.runtime.name.clone())
        })
    }

    fn check_identity(&self, id: &NamespacedId) -> Result<()> {
        if id.server() == &self.runtime.name {
            Ok(())
        } else {
            Err(Error::new(
                ErrorKind::Conflict,
                Recovery::Fatal,
                "namespaced item belongs to a different MCP server",
            )
            .with_server(self.runtime.name.clone()))
        }
    }
}

/// Transaction-ready catalogs from all supervised servers.
#[derive(Debug, Clone)]
pub struct SupervisorCatalog {
    servers: BTreeMap<ServerName, Arc<CatalogSnapshot>>,
    identities: BTreeSet<NamespacedId>,
}

impl SupervisorCatalog {
    /// Returns snapshots keyed by server.
    #[must_use]
    pub fn servers(&self) -> &BTreeMap<ServerName, Arc<CatalogSnapshot>> {
        &self.servers
    }

    /// Returns all stable identities after collision checking.
    #[must_use]
    pub fn identities(&self) -> &BTreeSet<NamespacedId> {
        &self.identities
    }
}

/// Supervisor for one first-party `mcode.mcp` plugin and many servers.
pub struct McpSupervisor {
    servers: BTreeMap<ServerName, ServerHandle>,
    joins: Mutex<BTreeMap<ServerName, JoinHandle<()>>>,
}

impl std::fmt::Debug for McpSupervisor {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("McpSupervisor")
            .field("servers", &self.servers.keys().collect::<Vec<_>>())
            .finish_non_exhaustive()
    }
}

impl McpSupervisor {
    /// Validates configuration and starts enabled server actors concurrently.
    ///
    /// A server connection failure is reported only in that server's state and
    /// does not fail or stop its siblings.
    ///
    /// # Errors
    ///
    /// Returns only for invalid static JSON configuration.
    ///
    /// # Panics
    ///
    /// Panics when called outside a Tokio runtime because enabled actors spawn immediately.
    pub fn start(
        config: McpPluginConfig,
        connector: McpConnectorHandle,
        host: McpHostHandle,
        auth: AuthHostHandle,
    ) -> Result<Self> {
        config.validate()?;
        let mut servers = BTreeMap::new();
        let mut joins = BTreeMap::new();
        for (name, server_config) in config.servers {
            let phase = if server_config.enabled {
                ServerPhase::Connecting
            } else {
                ServerPhase::Disabled
            };
            let (state, _) = watch::channel(ServerState::new(name.clone(), phase));
            let event_capacity = server_event_capacity(&server_config);
            let (events, _) = broadcast::channel(event_capacity);
            let runtime = Arc::new(ServerRuntime {
                name: name.clone(),
                config: server_config.clone(),
                state,
                session: RwLock::new(None),
                catalog: RwLock::new(Arc::new(CatalogSnapshot::empty(name.clone()))),
                events,
                shutdown: CancellationToken::new(),
            });
            let handle = ServerHandle {
                runtime: Arc::clone(&runtime),
            };
            if server_config.enabled {
                let (event_tx, event_rx) = mpsc::channel(event_capacity);
                let join = tokio::spawn(run_server_actor(
                    runtime,
                    connector.clone(),
                    host.clone(),
                    auth.clone(),
                    SessionEventSink::new(event_tx),
                    event_rx,
                ));
                joins.insert(name.clone(), join);
            }
            servers.insert(name, handle);
        }
        Ok(Self {
            servers,
            joins: Mutex::new(joins),
        })
    }

    /// Returns one server handle.
    #[must_use]
    pub fn server(&self, name: &ServerName) -> Option<&ServerHandle> {
        self.servers.get(name)
    }

    /// Returns all configured servers, including disabled ones.
    #[must_use]
    pub fn servers(&self) -> &BTreeMap<ServerName, ServerHandle> {
        &self.servers
    }

    /// Builds an explicit collision-checked upper registration transaction.
    ///
    /// No tool-name or semantic filtering is applied, including search tools.
    ///
    /// # Errors
    ///
    /// Returns an explicit conflict before a plugin registry can commit.
    pub async fn catalog(&self) -> Result<SupervisorCatalog> {
        let mut snapshots = BTreeMap::new();
        let mut identities = BTreeSet::new();
        for (server, handle) in &self.servers {
            let snapshot = handle.catalog().await;
            for identity in snapshot.identities() {
                if !identities.insert(identity.clone()) {
                    return Err(Error::new(
                        ErrorKind::Conflict,
                        Recovery::Fatal,
                        format!("catalog identity collision: {identity}"),
                    ));
                }
            }
            snapshots.insert(server.clone(), snapshot);
        }
        Ok(SupervisorCatalog {
            servers: snapshots,
            identities,
        })
    }

    /// Gracefully stops every server independently and joins all actors.
    pub async fn shutdown(&self) {
        for handle in self.servers.values() {
            handle.request_shutdown();
        }
        let mut joins = self.joins.lock().await;
        let pending = std::mem::take(&mut *joins);
        drop(joins);
        for (_, join) in pending {
            let _ = join.await;
        }
    }
}

enum ConnectedExit {
    Shutdown,
    Reconnect(Error),
}

async fn run_server_actor(
    runtime: Arc<ServerRuntime>,
    connector: McpConnectorHandle,
    host: McpHostHandle,
    auth: AuthHostHandle,
    event_sink: SessionEventSink,
    mut events: mpsc::Receiver<SessionEvent>,
) {
    let mut attempt = 0_u32;
    loop {
        if runtime.shutdown.is_cancelled() {
            break;
        }
        publish_state(
            &runtime,
            ServerPhase::Connecting,
            Some(InitProgress::ResolvingConfiguration),
            attempt,
            None,
            None,
        );
        publish_state(
            &runtime,
            ServerPhase::Connecting,
            Some(InitProgress::ConnectingTransport),
            attempt,
            None,
            None,
        );
        let connect = connector.connect(ConnectContext {
            server: runtime.name.clone(),
            config: runtime.config.clone(),
            host: host.clone(),
            auth: auth.clone(),
            events: event_sink.clone(),
        });
        publish_state(
            &runtime,
            ServerPhase::Connecting,
            Some(InitProgress::Negotiating),
            attempt,
            None,
            None,
        );
        let session = match tokio::time::timeout(runtime.config.timeouts.connect(), connect).await {
            Ok(Ok(session)) => session,
            Ok(Err(error)) => {
                if !schedule_reconnect(&runtime, &error, &mut attempt).await {
                    wait_for_shutdown(&runtime).await;
                    break;
                }
                continue;
            }
            Err(_) => {
                let error = Error::new(
                    ErrorKind::Timeout,
                    Recovery::Recoverable,
                    "MCP connect and negotiation timed out",
                )
                .with_server(runtime.name.clone());
                if !schedule_reconnect(&runtime, &error, &mut attempt).await {
                    wait_for_shutdown(&runtime).await;
                    break;
                }
                continue;
            }
        };
        let negotiated = session.negotiated().clone();
        publish_state(
            &runtime,
            ServerPhase::Connecting,
            Some(InitProgress::LoadingCatalog),
            attempt,
            Some(negotiated.clone()),
            None,
        );
        let generation = runtime.catalog.read().await.generation().next();
        let catalog =
            match fetch_full_catalog(&runtime.name, &runtime.config, &session, generation).await {
                Ok(catalog) => catalog,
                Err(error) => {
                    let _ = session.inner().close().await;
                    if !schedule_reconnect(&runtime, &error, &mut attempt).await {
                        wait_for_shutdown(&runtime).await;
                        break;
                    }
                    continue;
                }
            };
        let committed_generation = catalog.generation();
        *runtime.catalog.write().await = Arc::new(catalog);
        *runtime.session.write().await = Some(session.clone());
        attempt = 0;
        publish_state(
            &runtime,
            ServerPhase::Ready,
            None,
            attempt,
            Some(negotiated),
            None,
        );
        let mut ready_state = runtime.state.borrow().clone();
        ready_state.generation = committed_generation;
        let _ = runtime.state.send_replace(ready_state);

        match connected_loop(&runtime, &session, &mut events).await {
            ConnectedExit::Shutdown => {
                shutdown_session(&runtime, session).await;
                break;
            }
            ConnectedExit::Reconnect(error) => {
                *runtime.session.write().await = None;
                let _ = session.inner().close().await;
                if !schedule_reconnect(&runtime, &error, &mut attempt).await {
                    wait_for_shutdown(&runtime).await;
                    break;
                }
            }
        }
    }
    *runtime.session.write().await = None;
    publish_state(&runtime, ServerPhase::Stopped, None, 0, None, None);
}

async fn connected_loop(
    runtime: &Arc<ServerRuntime>,
    session: &McpSessionHandle,
    events: &mut mpsc::Receiver<SessionEvent>,
) -> ConnectedExit {
    let mut ping = tokio::time::interval(runtime.config.timeouts.ping_interval());
    ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let _ = ping.tick().await;
    loop {
        tokio::select! {
            () = runtime.shutdown.cancelled() => return ConnectedExit::Shutdown,
            _ = ping.tick() => {
                match tokio::time::timeout(runtime.config.timeouts.ping(), session.inner().ping()).await {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => return ConnectedExit::Reconnect(error),
                    Err(_) => return ConnectedExit::Reconnect(
                        Error::new(ErrorKind::Timeout, Recovery::Recoverable, "MCP ping timed out")
                            .with_server(runtime.name.clone())
                    ),
                }
            }
            event = events.recv() => match event {
                Some(SessionEvent::Disconnected(error)) => return ConnectedExit::Reconnect(error),
                Some(SessionEvent::ToolListChanged) => {
                    if session.negotiated().capabilities.supports(Capability::ToolListChanged) {
                        refresh_catalog(runtime, session, CatalogSection::Tools).await;
                    }
                }
                Some(SessionEvent::ResourceListChanged) => {
                    if session.negotiated().capabilities.supports(Capability::ResourceListChanged) {
                        refresh_catalog(runtime, session, CatalogSection::Resources).await;
                    }
                }
                Some(SessionEvent::PromptListChanged) => {
                    if session.negotiated().capabilities.supports(Capability::PromptListChanged) {
                        refresh_catalog(runtime, session, CatalogSection::Prompts).await;
                    }
                }
                Some(SessionEvent::ResourceUpdated { uri }) => {
                    let _ = runtime.events.send(ServerEvent::ResourceUpdated { uri });
                }
                Some(SessionEvent::TaskStatus(status)) => {
                    let _ = runtime.events.send(ServerEvent::TaskStatus(status));
                }
                Some(SessionEvent::Log(_)) => {}
                None => return ConnectedExit::Reconnect(
                    Error::transport(runtime.name.clone(), "MCP event stream closed")
                ),
            }
        }
    }
}

async fn refresh_catalog(
    runtime: &Arc<ServerRuntime>,
    session: &McpSessionHandle,
    section: CatalogSection,
) {
    let current = runtime.catalog.read().await.clone();
    let mut parts = current.to_parts();
    let result = match section {
        CatalogSection::Tools => fetch_tools(&runtime.config, session)
            .await
            .map(|tools| parts.tools = tools),
        CatalogSection::Resources => {
            match tokio::try_join!(
                fetch_resources(&runtime.config, session),
                fetch_resource_templates(&runtime.config, session)
            ) {
                Ok((resources, templates)) => {
                    parts.resources = resources;
                    parts.resource_templates = templates;
                    Ok(())
                }
                Err(error) => Err(error),
            }
        }
        CatalogSection::Prompts => fetch_prompts(&runtime.config, session)
            .await
            .map(|prompts| parts.prompts = prompts),
    };
    let snapshot = result.and_then(|()| {
        CatalogSnapshot::build(
            runtime.name.clone(),
            current.generation().next(),
            parts,
            &runtime.config.output_limits,
        )
    });
    match snapshot {
        Ok(snapshot) => {
            let generation = snapshot.generation();
            *runtime.catalog.write().await = Arc::new(snapshot);
            let mut state = runtime.state.borrow().clone();
            state.phase = ServerPhase::Ready;
            state.generation = generation;
            state.last_error = None;
            let _ = runtime.state.send_replace(state);
        }
        Err(error) => {
            let mut state = runtime.state.borrow().clone();
            state.phase = ServerPhase::Degraded;
            state.last_error = Some((&error).into());
            let _ = runtime.state.send_replace(state);
        }
    }
}

async fn fetch_full_catalog(
    server: &ServerName,
    config: &ServerConfig,
    session: &McpSessionHandle,
    generation: Generation,
) -> Result<CatalogSnapshot> {
    let (tools, resources, templates, prompts) = tokio::try_join!(
        fetch_tools(config, session),
        fetch_resources(config, session),
        fetch_resource_templates(config, session),
        fetch_prompts(config, session),
    )?;
    CatalogSnapshot::build(
        server.clone(),
        generation,
        CatalogParts {
            tools,
            resources,
            resource_templates: templates,
            prompts,
        },
        &config.output_limits,
    )
}

async fn fetch_tools(config: &ServerConfig, session: &McpSessionHandle) -> Result<Vec<RemoteTool>> {
    if !session.negotiated().capabilities.tools {
        return Ok(Vec::new());
    }
    paginate(config, |cursor| session.inner().list_tools(cursor)).await
}

async fn fetch_resources(
    config: &ServerConfig,
    session: &McpSessionHandle,
) -> Result<Vec<RemoteResource>> {
    if !session.negotiated().capabilities.resources {
        return Ok(Vec::new());
    }
    paginate(config, |cursor| session.inner().list_resources(cursor)).await
}

async fn fetch_resource_templates(
    config: &ServerConfig,
    session: &McpSessionHandle,
) -> Result<Vec<RemoteResourceTemplate>> {
    if !session.negotiated().capabilities.resource_templates {
        return Ok(Vec::new());
    }
    paginate(config, |cursor| {
        session.inner().list_resource_templates(cursor)
    })
    .await
}

async fn fetch_prompts(
    config: &ServerConfig,
    session: &McpSessionHandle,
) -> Result<Vec<RemotePrompt>> {
    if !session.negotiated().capabilities.prompts {
        return Ok(Vec::new());
    }
    paginate(config, |cursor| session.inner().list_prompts(cursor)).await
}

async fn paginate<T, F, Fut>(config: &ServerConfig, mut fetch: F) -> Result<Vec<T>>
where
    T: Serialize,
    F: FnMut(Option<String>) -> Fut,
    Fut: std::future::Future<Output = Result<Page<T>>>,
{
    let mut output = Vec::new();
    let mut cursor = None;
    let mut seen = BTreeSet::new();
    let mut retained_bytes = 0usize;
    for _ in 0..config.output_limits.max_pages {
        let page = fetch(cursor.clone()).await?;
        if page.items.len()
            > config
                .output_limits
                .max_catalog_items
                .saturating_sub(output.len())
        {
            return Err(Error::new(
                ErrorKind::Validation,
                Recovery::Fatal,
                "MCP pagination exceeded the catalog item cap",
            ));
        }
        let remaining = config
            .output_limits
            .max_output_bytes
            .saturating_sub(retained_bytes);
        let page_bytes = serialized_size_with_limit(&page.items, remaining)?;
        retained_bytes = retained_bytes.saturating_add(page_bytes);
        if let Some(next) = page.next_cursor.as_ref() {
            if next.len() > config.output_limits.max_string_bytes {
                return Err(Error::new(
                    ErrorKind::Validation,
                    Recovery::Fatal,
                    "MCP pagination cursor exceeds the configured string cap",
                ));
            }
            if next.len()
                > config
                    .output_limits
                    .max_output_bytes
                    .saturating_sub(retained_bytes)
            {
                return Err(Error::new(
                    ErrorKind::Validation,
                    Recovery::Fatal,
                    "MCP pagination exceeded the cumulative byte cap",
                ));
            }
            retained_bytes = retained_bytes.saturating_add(next.len());
        }

        output.extend(page.items);
        let Some(next) = page.next_cursor else {
            return Ok(output);
        };
        if next.is_empty() || !seen.insert(next.clone()) {
            return Err(Error::new(
                ErrorKind::Protocol,
                Recovery::Fatal,
                "MCP pagination cursor is empty or repeated",
            ));
        }
        cursor = Some(next);
    }
    Err(Error::new(
        ErrorKind::Validation,
        Recovery::Fatal,
        "MCP pagination exceeded the page cap",
    ))
}

#[derive(Debug)]
struct ByteCounter {
    bytes: usize,
    limit: usize,
}

impl Write for ByteCounter {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        if buffer.len() > self.limit.saturating_sub(self.bytes) {
            return Err(std::io::Error::other(
                "serialized value exceeded its byte cap",
            ));
        }
        self.bytes = self.bytes.saturating_add(buffer.len());
        Ok(buffer.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn serialized_size_with_limit(value: &impl Serialize, limit: usize) -> Result<usize> {
    let mut counter = ByteCounter { bytes: 0, limit };
    serde_json::to_writer(&mut counter, value).map_err(|_| {
        Error::new(
            ErrorKind::Validation,
            Recovery::Fatal,
            "MCP pagination exceeded the cumulative byte cap",
        )
    })?;
    Ok(counter.bytes)
}

async fn schedule_reconnect(
    runtime: &Arc<ServerRuntime>,
    error: &Error,
    attempt: &mut u32,
) -> bool {
    let can_retry = error.recovery() == Recovery::Recoverable
        && runtime.config.reconnect.enabled
        && *attempt < runtime.config.reconnect.max_attempts;
    if !can_retry {
        publish_state(
            runtime,
            ServerPhase::Failed,
            None,
            *attempt,
            None,
            Some(error),
        );
        return false;
    }
    let delay = runtime.config.reconnect.delay(*attempt);
    *attempt = attempt.saturating_add(1);
    publish_state(
        runtime,
        ServerPhase::BackingOff,
        None,
        *attempt,
        None,
        Some(error),
    );
    tokio::select! {
        () = runtime.shutdown.cancelled() => false,
        () = tokio::time::sleep(delay) => true,
    }
}

async fn shutdown_session(runtime: &Arc<ServerRuntime>, session: McpSessionHandle) {
    publish_state(
        runtime,
        ServerPhase::ShuttingDown,
        None,
        0,
        Some(session.negotiated().clone()),
        None,
    );
    *runtime.session.write().await = None;
    let _ = tokio::time::timeout(runtime.config.timeouts.shutdown(), session.inner().close()).await;
}

async fn wait_for_shutdown(runtime: &Arc<ServerRuntime>) {
    runtime.shutdown.cancelled().await;
}

fn publish_state(
    runtime: &ServerRuntime,
    phase: ServerPhase,
    progress: Option<InitProgress>,
    attempt: u32,
    negotiated: Option<NegotiatedServer>,
    error: Option<&Error>,
) {
    let previous = runtime.state.borrow();
    let generation = previous.generation;
    let state = ServerState {
        server: runtime.name.clone(),
        phase,
        init_progress: progress,
        reconnect_attempt: attempt,
        generation,
        negotiated: negotiated.or_else(|| previous.negotiated.clone()),
        last_error: error.map(Into::into),
    };
    drop(previous);
    let _ = runtime.state.send_replace(state);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pagination_config() -> ServerConfig {
        serde_json::from_value(serde_json::json!({
            "transport": {"type":"stdio", "command":"fake"}
        }))
        .unwrap()
    }

    #[test]
    fn generation_is_monotonic() {
        assert_eq!(Generation::new(4).next().get(), 5);
    }

    #[test]
    fn event_queue_capacity_is_derived_from_payload_bytes() {
        let config = pagination_config();
        let capacity = server_event_capacity(&config);
        let max_event_bytes = config
            .output_limits
            .max_output_bytes
            .saturating_add(std::mem::size_of::<SessionEvent>());
        assert!(capacity < SERVER_EVENT_MAX_CAPACITY);
        assert!(capacity.saturating_mul(max_event_bytes) <= SERVER_EVENT_BUFFER_BYTES);

        let mut largest = pagination_config();
        largest.output_limits.max_output_bytes = 64 * 1024 * 1024;
        assert_eq!(server_event_capacity(&largest), 1);
    }

    #[tokio::test]
    async fn pagination_rejects_oversized_cursors_before_retaining_them() {
        let mut config = pagination_config();
        config.output_limits.max_string_bytes = 64;
        let result = paginate(&config, |_| {
            std::future::ready(Ok(Page::<String>::new(Vec::new(), Some("x".repeat(65)))))
        })
        .await;
        assert_eq!(result.unwrap_err().kind(), ErrorKind::Validation);
    }

    #[tokio::test]
    async fn pagination_bounds_cumulative_serialized_bytes() {
        let mut config = pagination_config();
        config.output_limits.max_output_bytes = 1_024;
        let mut page = 0usize;
        let result = paginate(&config, |_| {
            page = page.saturating_add(1);
            std::future::ready(Ok(Page::new(
                vec!["x".repeat(600)],
                (page == 1).then(|| "next".to_owned()),
            )))
        })
        .await;
        assert_eq!(result.unwrap_err().kind(), ErrorKind::Validation);
    }
}

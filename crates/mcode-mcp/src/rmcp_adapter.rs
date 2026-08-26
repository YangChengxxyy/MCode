//! `rmcp` 3.1.4 adapter isolated behind the crate's protocol traits.

// Rust guideline compliant 2026-08-26.

#![expect(
    deprecated,
    reason = "roots, sampling, and logging remain required compatibility capabilities"
)]

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
    time::Duration,
};

use async_trait::async_trait;
use futures::StreamExt;
use rmcp::{
    ClientHandler,
    handler::client::progress::ProgressDispatcher,
    model::{
        CallToolRequest, CallToolRequestParams, CancelTaskParams, CancelTaskRequest,
        ClientCapabilities, ClientInfo, ClientRequest, CompleteRequest, CompleteRequestParams,
        CreateMessageRequestParams, CreateMessageResult, ElicitRequestParams, ElicitResult,
        ElicitationAction, ElicitationCapability, ExtensionCapabilities, FormElicitationCapability,
        GetPromptRequest, GetPromptRequestParams, GetTaskParams, GetTaskRequest, Implementation,
        ListRootsResult, LoggingLevel, PaginatedRequestParams, PingRequest, Prompt,
        ReadResourceRequest, ReadResourceRequestParams, Root as RmcpRoot, RootsCapabilities,
        SamplingCapability, ServerNotification, ServerPeerInfo, ServerResult, SetLevelRequest,
        SetLevelRequestParams, SubscribeRequestParams, SubscriptionFilter, TASKS_EXTENSION_ID,
        Tool, UnsubscribeRequestParams, UpdateTaskParams, UpdateTaskRequest,
        UrlElicitationCapability,
    },
    service::{
        ClientLifecycleMode, ClientServiceExt, NotificationContext, Peer, PeerRequestOptions,
        RequestContext, RequestHandle, RoleClient, RunningService, ServiceError,
    },
    transport::{
        IntoTransport,
        common::client_side_sse::{SseRetryPolicy, SseStreamContext},
        streamable_http_client::{
            StreamableHttpClientTransport, StreamableHttpClientTransportConfig,
        },
    },
};
use serde_json::{Map, Value, json};
use tokio::{
    io::AsyncReadExt,
    sync::{Mutex, RwLock},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::{
    auth::build_http_client,
    config::{AuthConfig, ServerConfig, TransportConfig, TrustLevel},
    error::{Error, ErrorKind, Recovery, Result},
    host::{
        ElicitationRequest, ElicitationResponse, HostContext, HostOperation, LogEvent, LogLevel,
        McpHostHandle, PermissionDecision, PermissionRequest, SamplingRequest,
    },
    http::{DnsResolverHandle, HttpSecurityPolicy, SseReconnectGate, SystemDnsResolver},
    identity::ServerName,
    process::{BoundedStdioTransport, NoProcessHost, ProcessHostHandle, ProcessSpec},
    protocol::{
        Capability, ConnectContext, MCP_LEGACY_PROTOCOL_VERSION, MCP_PROTOCOL_VERSION,
        McpConnector, McpSession, McpSessionHandle, NegotiatedCapabilities, NegotiatedServer, Page,
        ProgressUpdate, PromptGetOutcome, RemotePrompt, RemotePromptArgument, RemoteResource,
        RemoteResourceTemplate as ProtocolResourceTemplate, RemoteTool, RequestControl,
        ResourceReadOutcome, SessionEvent, SessionEventSink, ToolCallOutcome,
    },
    validation::{
        sanitize_json, sanitize_text, validate_completion_result, validate_elicitation_content,
        validate_elicitation_schema, validate_prompt_result, validate_resource_result,
        validate_tool_result,
    },
};

/// Official SDK connector for direct stdio and Streamable HTTP.
#[derive(Debug, Clone)]
pub struct RmcpConnector {
    process_host: ProcessHostHandle,
    resolver: DnsResolverHandle,
}

impl RmcpConnector {
    /// Creates a connector with explicit process containment and DNS adapters.
    #[must_use]
    pub fn new(process_host: ProcessHostHandle, resolver: DnsResolverHandle) -> Self {
        Self {
            process_host,
            resolver,
        }
    }

    /// Creates an HTTP-capable connector that rejects stdio until adapted.
    #[must_use]
    pub fn http_only() -> Self {
        Self::new(
            ProcessHostHandle::new(NoProcessHost),
            DnsResolverHandle::new(SystemDnsResolver),
        )
    }
}

#[async_trait]
impl McpConnector for RmcpConnector {
    async fn connect(&self, context: ConnectContext) -> Result<McpSessionHandle> {
        let bridge = HostBridge::new(&context);
        let running = match &context.config.transport {
            TransportConfig::Stdio(config) => {
                let mut secret_env = BTreeMap::new();
                for (name, binding) in &config.env.secrets {
                    let value = context
                        .auth
                        .inner()
                        .resolve_secret(&context.server, &binding.secret_ref)
                        .await?;
                    secret_env.insert(name.clone(), value);
                }
                let process = self
                    .process_host
                    .spawn_direct(ProcessSpec {
                        server: context.server.clone(),
                        executable: config.command.clone(),
                        args: config.args.clone(),
                        cwd: config.cwd.clone(),
                        inherit_env: config.env.inherit.clone(),
                        secret_env,
                    })
                    .await?;
                let (transport, stderr) = BoundedStdioTransport::new(
                    process,
                    context.config.output_limits.max_message_bytes,
                    context.config.timeouts.shutdown(),
                );
                if let Some(stderr) = stderr {
                    spawn_stderr_drain(
                        stderr,
                        context.events.clone(),
                        context.config.output_limits.max_log_bytes,
                    );
                }
                serve_rmcp(&context.server, bridge, transport).await?
            }
            TransportConfig::StreamableHttp(config) => {
                if matches!(config.auth, AuthConfig::OAuth2(_)) {
                    bridge
                        .authorize(
                            HostOperation::OAuth,
                            json!({
                                "operation": "oauth2.1",
                                "server": context.server.to_string(),
                            }),
                        )
                        .await?;
                }
                let policy = HttpSecurityPolicy::new(
                    context.server.clone(),
                    context.config.trust.clone(),
                    context.config.output_limits.clone(),
                    context.config.timeouts.clone(),
                );
                let reconnect = SseReconnectGate::new(context.config.reconnect.clone());
                let client = build_http_client(
                    &context.server,
                    config,
                    &context.auth,
                    self.resolver.clone(),
                    policy,
                )
                .await?
                .with_sse_reconnect(reconnect.clone());
                let mut transport_config = StreamableHttpClientTransportConfig::with_uri(
                    Arc::<str>::from(config.url.as_str()),
                );
                transport_config.allow_stateless = true;
                transport_config.max_sse_event_size =
                    context.config.output_limits.max_sse_event_bytes;
                // The SDK recovery path replays the in-flight request after reinitialization.
                transport_config.reinit_on_expired_session = false;
                transport_config.retry_config = Arc::new(ConfiguredSseRetry(reconnect));
                let transport =
                    StreamableHttpClientTransport::with_client(client, transport_config);
                serve_rmcp(&context.server, bridge, transport).await?
            }
        };

        running
            .peer()
            .set_response_cache_config(
                rmcp::service::ClientCacheConfig::default().with_serve_stale_on_error(false),
            )
            .await;
        let peer_info = running.peer_info().ok_or_else(|| {
            protocol_error(
                &context.server,
                "rmcp completed startup without peer information",
            )
        })?;
        let negotiated = negotiated_server(&peer_info);
        let session = RmcpSession::new(
            context.server,
            context.config,
            negotiated,
            running,
            context.events,
        );
        session.start_modern_subscription().await?;
        Ok(McpSessionHandle::new(session))
    }
}

async fn serve_rmcp<T, E, A>(
    server: &ServerName,
    bridge: HostBridge,
    transport: T,
) -> Result<RunningService<RoleClient, HostBridge>>
where
    T: IntoTransport<RoleClient, E, A>,
    E: std::error::Error + Send + Sync + 'static,
{
    bridge
        .serve_with_lifecycle(
            transport,
            ClientLifecycleMode::Auto {
                preferred_versions: vec![rmcp::model::ProtocolVersion::V_2026_07_28],
                legacy_version: Some(rmcp::model::ProtocolVersion::V_2025_11_25),
            },
        )
        .await
        .map_err(|error| {
            let recovery = if error.is_authorization_required() {
                Recovery::Fatal
            } else {
                Recovery::Recoverable
            };
            Error::new(
                if error.is_authorization_required() {
                    ErrorKind::Authentication
                } else {
                    ErrorKind::Transport
                },
                recovery,
                "MCP lifecycle negotiation failed",
            )
            .with_server(server.clone())
        })
}

#[derive(Debug, Clone)]
struct ConfiguredSseRetry(SseReconnectGate);

impl SseRetryPolicy for ConfiguredSseRetry {
    fn retry(&self, current_times: usize) -> Option<Duration> {
        // Delay only. Pending/live pairing lives on the stream-local token from
        // [`Self::stream_context`], so a shared policy object cannot leak waits.
        self.0.retry_delay(current_times)
    }

    fn stream_context(&self) -> SseStreamContext {
        SseStreamContext::new(self.0.stream_token())
    }
}

fn spawn_stderr_drain(
    mut stderr: crate::process::BoxAsyncRead,
    events: SessionEventSink,
    max_log_bytes: usize,
) {
    tokio::spawn(async move {
        let mut buffer = vec![0_u8; 4_096];
        loop {
            let count = match stderr.read(&mut buffer).await {
                Ok(0) | Err(_) => break,
                Ok(count) => count,
            };
            let message = sanitize_text(&String::from_utf8_lossy(&buffer[..count]), max_log_bytes);
            let _ = events.try_send(SessionEvent::Log(LogEvent {
                level: LogLevel::Debug,
                logger: Some("stdio.stderr".to_owned()),
                data: Value::String(message),
            }));
        }
    });
}

#[derive(Clone)]
struct HostBridge {
    context: HostContext,
    host: McpHostHandle,
    events: SessionEventSink,
    limits: crate::config::OutputLimits,
    progress: ProgressDispatcher,
}

impl HostBridge {
    fn new(context: &ConnectContext) -> Self {
        Self {
            context: HostContext {
                server: context.server.clone(),
                trust: context.config.trust.clone(),
            },
            host: context.host.clone(),
            events: context.events.clone(),
            limits: context.config.output_limits.clone(),
            progress: ProgressDispatcher::new(),
        }
    }

    async fn authorize(&self, operation: HostOperation, preview: Value) -> Result<()> {
        let trusted = self.context.trust.level == TrustLevel::Trusted
            && match operation {
                HostOperation::Sampling => self.context.trust.allow_sampling,
                HostOperation::FormElicitation | HostOperation::UrlElicitation => {
                    self.context.trust.allow_elicitation
                }
                HostOperation::Roots => self.context.trust.allow_roots,
                HostOperation::OAuth => self.context.trust.allow_oauth,
            };
        if !trusted {
            return Err(Error::new(
                ErrorKind::Trust,
                Recovery::Fatal,
                "server-initiated operation lacks an explicit trust grant",
            )
            .with_server(self.context.server.clone()));
        }
        match self
            .host
            .inner()
            .authorize(&self.context, PermissionRequest { operation, preview })
            .await?
        {
            PermissionDecision::AllowOnce => Ok(()),
            PermissionDecision::Deny { reason } => {
                Err(Error::new(ErrorKind::Permission, Recovery::Fatal, reason)
                    .with_server(self.context.server.clone()))
            }
        }
    }

    fn host_capabilities(&self) -> crate::host::HostCapabilities {
        self.host.inner().capabilities(&self.context)
    }
}

impl ClientHandler for HostBridge {
    async fn create_message(
        &self,
        params: CreateMessageRequestParams,
        context: RequestContext<RoleClient>,
    ) -> std::result::Result<CreateMessageResult, rmcp::ErrorData> {
        params
            .validate()
            .map_err(|_| rmcp_error("sampling request is invalid", "validation"))?;
        let value = serde_json::to_value(params)
            .map_err(|_| rmcp_error("sampling request could not be encoded", "validation"))?;
        let value =
            sanitize_json(&self.context.server, &value, &self.limits).map_err(to_rmcp_error)?;
        protected_call(
            &context.ct,
            self.authorize(HostOperation::Sampling, value.clone()),
        )
        .await?;
        let response = protected_call(
            &context.ct,
            self.host
                .inner()
                .sample(&self.context, SamplingRequest { params: value }),
        )
        .await?;
        let value = sanitize_json(&self.context.server, &response.result, &self.limits)
            .map_err(to_rmcp_error)?;
        let result: CreateMessageResult = serde_json::from_value(value)
            .map_err(|_| rmcp_error("sampling response is invalid", "validation"))?;
        result
            .validate()
            .map_err(|_| rmcp_error("sampling response is invalid", "validation"))?;
        Ok(result)
    }

    async fn list_roots(
        &self,
        context: RequestContext<RoleClient>,
    ) -> std::result::Result<ListRootsResult, rmcp::ErrorData> {
        protected_call(
            &context.ct,
            self.authorize(HostOperation::Roots, json!({"operation":"roots/list"})),
        )
        .await?;
        let roots = protected_call(&context.ct, self.host.inner().roots(&self.context)).await?;
        if roots.len() > self.limits.max_catalog_items {
            return Err(rmcp_error("host returned too many roots", "validation"));
        }
        let roots = roots
            .into_iter()
            .map(|root| {
                let uri = sanitize_text(&root.uri, self.limits.max_string_bytes);
                let mut output = RmcpRoot::new(uri);
                if let Some(name) = root.name {
                    output = output.with_name(sanitize_text(&name, self.limits.max_string_bytes));
                }
                output
            })
            .collect();
        Ok(ListRootsResult::new(roots))
    }

    async fn create_elicitation(
        &self,
        request: ElicitRequestParams,
        context: RequestContext<RoleClient>,
    ) -> std::result::Result<ElicitResult, rmcp::ErrorData> {
        let (operation, host_request, form_schema) = match request {
            ElicitRequestParams::FormElicitationParams {
                message,
                requested_schema,
                ..
            } => {
                let schema = serde_json::to_value(requested_schema).map_err(|_| {
                    rmcp_error("elicitation schema could not be encoded", "validation")
                })?;
                let schema =
                    validate_elicitation_schema(&self.context.server, &schema, &self.limits)
                        .map_err(to_rmcp_error)?;
                (
                    HostOperation::FormElicitation,
                    ElicitationRequest::Form {
                        message: sanitize_text(&message, self.limits.max_log_bytes),
                        requested_schema: schema.clone(),
                    },
                    Some(schema),
                )
            }
            ElicitRequestParams::UrlElicitationParams {
                message,
                url,
                elicitation_id,
                ..
            } => {
                let parsed = Url::parse(&url)
                    .map_err(|_| rmcp_error("elicitation URL is invalid", "validation"))?;
                if parsed.scheme() != "https" || parsed.host().is_none() {
                    return Err(rmcp_error("elicitation URL must be HTTPS", "trust"));
                }
                (
                    HostOperation::UrlElicitation,
                    ElicitationRequest::Url {
                        message: sanitize_text(&message, self.limits.max_log_bytes),
                        url,
                        elicitation_id: sanitize_text(
                            &elicitation_id,
                            self.limits.max_string_bytes,
                        ),
                    },
                    None,
                )
            }
            _ => return Err(rmcp_error("unsupported elicitation mode", "unsupported")),
        };
        let preview = serde_json::to_value(&host_request)
            .map_err(|_| rmcp_error("elicitation preview failed", "validation"))?;
        protected_call(&context.ct, self.authorize(operation, preview)).await?;
        let response = protected_call(
            &context.ct,
            self.host.inner().elicit(&self.context, host_request),
        )
        .await?;
        match response {
            ElicitationResponse::Accept { content } => {
                let content = match (content, form_schema) {
                    (Some(content), Some(schema)) => Some(
                        validate_elicitation_content(
                            &self.context.server,
                            &schema,
                            &content,
                            &self.limits,
                        )
                        .map_err(to_rmcp_error)?,
                    ),
                    (None, Some(_)) => {
                        return Err(rmcp_error(
                            "accepted form elicitation has no content",
                            "validation",
                        ));
                    }
                    (content, None) => content,
                };
                let mut result = ElicitResult::new(ElicitationAction::Accept);
                if let Some(content) = content {
                    result = result.with_content(content);
                }
                Ok(result)
            }
            ElicitationResponse::Decline => Ok(ElicitResult::new(ElicitationAction::Decline)),
            ElicitationResponse::Cancel => Ok(ElicitResult::new(ElicitationAction::Cancel)),
        }
    }

    async fn on_progress(
        &self,
        params: rmcp::model::ProgressNotificationParam,
        _context: NotificationContext<RoleClient>,
    ) {
        self.progress.handle_notification(params).await;
    }

    async fn on_logging_message(
        &self,
        params: rmcp::model::LoggingMessageNotificationParam,
        _context: NotificationContext<RoleClient>,
    ) {
        let event = LogEvent {
            level: from_rmcp_log_level(params.level),
            logger: params
                .logger
                .map(|value| sanitize_text(&value, self.limits.max_log_bytes)),
            data: sanitize_json(&self.context.server, &params.data, &self.limits)
                .unwrap_or_else(|_| Value::String("[invalid remote log]".to_owned())),
        };
        self.host.inner().log(&self.context, event.clone()).await;
        let _ = self.events.send(SessionEvent::Log(event)).await;
    }

    async fn on_resource_updated(
        &self,
        params: rmcp::model::ResourceUpdatedNotificationParam,
        _context: NotificationContext<RoleClient>,
    ) {
        let _ = self
            .events
            .send(SessionEvent::ResourceUpdated {
                uri: sanitize_text(&params.uri, self.limits.max_string_bytes),
            })
            .await;
    }

    async fn on_resource_list_changed(&self, _context: NotificationContext<RoleClient>) {
        let _ = self.events.send(SessionEvent::ResourceListChanged).await;
    }

    async fn on_tool_list_changed(&self, _context: NotificationContext<RoleClient>) {
        let _ = self.events.send(SessionEvent::ToolListChanged).await;
    }

    async fn on_prompt_list_changed(&self, _context: NotificationContext<RoleClient>) {
        let _ = self.events.send(SessionEvent::PromptListChanged).await;
    }

    async fn on_task_status(
        &self,
        params: rmcp::model::TaskStatusNotificationParams,
        _context: NotificationContext<RoleClient>,
    ) {
        if let Ok(value) = serde_json::to_value(params)
            && let Ok(value) = sanitize_json(&self.context.server, &value, &self.limits)
        {
            let _ = self.events.send(SessionEvent::TaskStatus(value)).await;
        }
    }

    fn get_info(&self) -> ClientInfo {
        let host = self.host_capabilities();
        let mut capabilities = ClientCapabilities::default();
        if host.roots && self.context.trust.allow_roots {
            let mut roots = RootsCapabilities::default();
            roots.list_changed = host.roots_list_changed.then_some(true);
            capabilities.roots = Some(roots);
        }
        if host.sampling && self.context.trust.allow_sampling {
            let mut sampling = SamplingCapability::default();
            sampling.tools = host.sampling_tools.then_some(Map::new());
            capabilities.sampling = Some(sampling);
        }
        if self.context.trust.allow_elicitation && (host.form_elicitation || host.url_elicitation) {
            let mut elicitation = ElicitationCapability::default();
            elicitation.form = host
                .form_elicitation
                .then_some(FormElicitationCapability::default().with_schema_validation(true));
            elicitation.url = host
                .url_elicitation
                .then_some(UrlElicitationCapability::default());
            capabilities.elicitation = Some(elicitation);
        }
        if host.tasks {
            let mut extensions = ExtensionCapabilities::new();
            extensions.insert(TASKS_EXTENSION_ID.to_owned(), Map::new());
            capabilities.extensions = Some(extensions);
        }
        ClientInfo::new(
            capabilities,
            Implementation::new("mcode.mcp", env!("CARGO_PKG_VERSION")),
        )
        .with_protocol_version(rmcp::model::ProtocolVersion::V_2026_07_28)
    }
}

struct SubscriptionRuntime {
    resources: BTreeSet<String>,
    cancel: Option<CancellationToken>,
    join: Option<JoinHandle<()>>,
}

impl SubscriptionRuntime {
    fn new() -> Self {
        Self {
            resources: BTreeSet::new(),
            cancel: None,
            join: None,
        }
    }
}

struct RmcpSession {
    server: ServerName,
    config: ServerConfig,
    negotiated: NegotiatedServer,
    running: RwLock<Option<RunningService<RoleClient, HostBridge>>>,
    progress: ProgressDispatcher,
    events: SessionEventSink,
    subscription: Mutex<SubscriptionRuntime>,
}

impl RmcpSession {
    fn new(
        server: ServerName,
        config: ServerConfig,
        negotiated: NegotiatedServer,
        running: RunningService<RoleClient, HostBridge>,
        events: SessionEventSink,
    ) -> Self {
        let progress = running.service().progress.clone();
        Self {
            server,
            config,
            negotiated,
            running: RwLock::new(Some(running)),
            progress,
            events,
            subscription: Mutex::new(SubscriptionRuntime::new()),
        }
    }

    async fn peer(&self) -> Result<Peer<RoleClient>> {
        self.running
            .read()
            .await
            .as_ref()
            .map(|running| running.peer().clone())
            .ok_or_else(|| unavailable_error(&self.server))
    }

    async fn request(
        &self,
        request: ClientRequest,
        control: RequestControl,
    ) -> Result<ServerResult> {
        if control.cancellation.is_cancelled() {
            return Err(cancelled_error(&self.server));
        }

        let total = tokio::time::sleep(self.config.timeouts.request_total());
        tokio::pin!(total);
        let peer = tokio::select! {
            biased;
            () = control.cancellation.cancelled() => return Err(cancelled_error(&self.server)),
            () = &mut total => return Err(timeout_error(&self.server)),
            result = self.peer() => result?,
        };
        let send = peer.send_cancellable_request(request, PeerRequestOptions::no_options());
        tokio::pin!(send);
        let mut handle = tokio::select! {
            biased;
            () = control.cancellation.cancelled() => return Err(cancelled_error(&self.server)),
            () = &mut total => return Err(timeout_error(&self.server)),
            result = &mut send => {
                result.map_err(|error| map_service_error(&self.server, error))?
            }
        };
        let subscribe = self.progress.subscribe(handle.progress_token.clone());
        tokio::pin!(subscribe);
        let mut progress = tokio::select! {
            biased;
            () = control.cancellation.cancelled() => {
                cancel_request(
                    handle,
                    "caller cancelled request",
                    self.config.timeouts.shutdown(),
                );
                return Err(cancelled_error(&self.server));
            }
            () = &mut total => {
                cancel_request(
                    handle,
                    "request total timeout",
                    self.config.timeouts.shutdown(),
                );
                return Err(timeout_error(&self.server));
            }
            subscriber = &mut subscribe => subscriber,
        };
        let idle = tokio::time::sleep(self.config.timeouts.request());
        let mut progress_open = true;
        tokio::pin!(idle);

        loop {
            tokio::select! {
                biased;
                () = control.cancellation.cancelled() => {
                    cancel_request(
                        handle,
                        "caller cancelled request",
                        self.config.timeouts.shutdown(),
                    );
                    return Err(cancelled_error(&self.server));
                }
                () = &mut total => {
                    cancel_request(
                        handle,
                        "request total timeout",
                        self.config.timeouts.shutdown(),
                    );
                    return Err(timeout_error(&self.server));
                }
                response = &mut handle.rx => {
                    return response
                        .map_err(|_| unavailable_error(&self.server))?
                        .map_err(|error| map_service_error(&self.server, error));
                }
                update = progress.next(), if progress_open => {
                    if let Some(update) = update {
                        control.observer.notify(ProgressUpdate {
                            progress: update.progress,
                            total: update.total,
                            message: update.message.map(|message| {
                                sanitize_text(&message, self.config.output_limits.max_log_bytes)
                            }),
                        });
                        idle.as_mut().reset(tokio::time::Instant::now() + self.config.timeouts.request());
                    } else {
                        progress_open = false;
                    }
                }
                () = &mut idle => {
                    cancel_request(
                        handle,
                        "request idle timeout",
                        self.config.timeouts.shutdown(),
                    );
                    return Err(timeout_error(&self.server));
                }
            }
        }
    }

    async fn timed<T>(
        &self,
        future: impl std::future::Future<Output = std::result::Result<T, ServiceError>>,
    ) -> Result<T> {
        tokio::time::timeout(self.config.timeouts.request_total(), future)
            .await
            .map_err(|_| timeout_error(&self.server))?
            .map_err(|error| map_service_error(&self.server, error))
    }

    async fn start_modern_subscription(&self) -> Result<()> {
        if self.negotiated.is_modern() {
            self.restart_subscription().await?;
        }
        Ok(())
    }

    async fn restart_subscription(&self) -> Result<()> {
        let mut state = self.subscription.lock().await;
        if let Some(cancel) = state.cancel.take() {
            cancel.cancel();
        }
        if let Some(join) = state.join.take() {
            let _ = join.await;
        }
        let mut filter = SubscriptionFilter::new();
        filter.tools_list_changed = self
            .negotiated
            .capabilities
            .tool_list_changed
            .then_some(true);
        filter.prompts_list_changed = self
            .negotiated
            .capabilities
            .prompt_list_changed
            .then_some(true);
        filter.resources_list_changed = self
            .negotiated
            .capabilities
            .resource_list_changed
            .then_some(true);
        if !state.resources.is_empty() {
            filter.resource_subscriptions = Some(state.resources.iter().cloned().collect());
        }
        let has_filter = filter.tools_list_changed == Some(true)
            || filter.prompts_list_changed == Some(true)
            || filter.resources_list_changed == Some(true)
            || filter.resource_subscriptions.is_some();
        if !has_filter {
            return Ok(());
        }
        let peer = self.peer().await?;
        let mut subscription = self.timed(peer.listen(filter)).await?;
        let cancel = CancellationToken::new();
        let task_cancel = cancel.clone();
        let events = self.events.clone();
        let server = self.server.clone();
        let max_string = self.config.output_limits.max_string_bytes;
        let max_output = self.config.output_limits.clone();
        let join = tokio::spawn(async move {
            loop {
                tokio::select! {
                    () = task_cancel.cancelled() => {
                        let _ = subscription.cancel().await;
                        break;
                    }
                    notification = subscription.next() => match notification {
                        Ok(Some(notification)) => {
                            if let Some(event) = subscription_event(notification, &server, &max_output, max_string) {
                                let _ = events.send(event).await;
                            }
                        }
                        Ok(None) => {
                            if !task_cancel.is_cancelled() {
                                let _ = events.send(SessionEvent::Disconnected(
                                    Error::transport(server.clone(), "MCP subscription stream ended")
                                )).await;
                            }
                            break;
                        }
                        Err(error) => {
                            let _ = events.send(SessionEvent::Disconnected(
                                map_service_error(&server, error)
                            )).await;
                            break;
                        }
                    }
                }
            }
        });
        state.cancel = Some(cancel);
        state.join = Some(join);
        Ok(())
    }

    async fn stop_subscription(&self) {
        let mut state = self.subscription.lock().await;
        if let Some(cancel) = state.cancel.take() {
            cancel.cancel();
        }
        if let Some(join) = state.join.take() {
            let _ = join.await;
        }
    }
}

#[async_trait]
impl McpSession for RmcpSession {
    fn negotiated(&self) -> &NegotiatedServer {
        &self.negotiated
    }

    async fn ping(&self) -> Result<()> {
        let request = ClientRequest::PingRequest(PingRequest::default());
        match self.request(request, RequestControl::new()).await? {
            ServerResult::EmptyResult(_) => Ok(()),
            _ => Err(protocol_error(&self.server, "unexpected ping response")),
        }
    }

    async fn list_tools(&self, cursor: Option<String>) -> Result<Page<RemoteTool>> {
        self.negotiated.require(&self.server, Capability::Tools)?;
        let peer = self.peer().await?;
        let result = self
            .timed(peer.list_tools(Some(PaginatedRequestParams::default().with_cursor(cursor))))
            .await?;
        let items = result
            .tools
            .into_iter()
            .map(remote_tool)
            .collect::<Result<_>>()?;
        Ok(Page::new(items, result.next_cursor))
    }

    async fn call_tool(
        &self,
        name: &str,
        arguments: Value,
        control: RequestControl,
    ) -> Result<ToolCallOutcome> {
        self.negotiated.require(&self.server, Capability::Tools)?;
        let arguments = arguments
            .as_object()
            .cloned()
            .ok_or_else(|| validation_error(&self.server, "tool arguments must be an object"))?;
        let params = CallToolRequestParams::new(name.to_owned()).with_arguments(arguments);
        let request = ClientRequest::CallToolRequest(CallToolRequest::new(params));
        match self.request(request, control).await? {
            ServerResult::CallToolResult(result) => {
                let is_error = result.is_error.unwrap_or(false);
                let value = serde_json::to_value(result)
                    .map_err(|_| protocol_error(&self.server, "tool result encoding failed"))?;
                let result =
                    validate_tool_result(&self.server, &value, &self.config.output_limits)?;
                Ok(ToolCallOutcome::Complete { result, is_error })
            }
            ServerResult::CreateTaskResult(task) => {
                self.negotiated.require(&self.server, Capability::Tasks)?;
                let value = serde_json::to_value(task)
                    .map_err(|_| protocol_error(&self.server, "task result encoding failed"))?;
                Ok(ToolCallOutcome::Task {
                    task: sanitize_json(&self.server, &value, &self.config.output_limits)?,
                })
            }
            ServerResult::InputRequiredResult(request) => {
                let value = serde_json::to_value(request).map_err(|_| {
                    protocol_error(&self.server, "input-required result encoding failed")
                })?;
                Ok(ToolCallOutcome::InputRequired {
                    request: sanitize_json(&self.server, &value, &self.config.output_limits)?,
                })
            }
            _ => Err(protocol_error(
                &self.server,
                "unexpected tools/call response",
            )),
        }
    }

    async fn list_resources(&self, cursor: Option<String>) -> Result<Page<RemoteResource>> {
        self.negotiated
            .require(&self.server, Capability::Resources)?;
        let peer = self.peer().await?;
        let result = self
            .timed(peer.list_resources(Some(PaginatedRequestParams::default().with_cursor(cursor))))
            .await?;
        Ok(Page::new(
            result.resources.into_iter().map(remote_resource).collect(),
            result.next_cursor,
        ))
    }

    async fn list_resource_templates(
        &self,
        cursor: Option<String>,
    ) -> Result<Page<ProtocolResourceTemplate>> {
        self.negotiated
            .require(&self.server, Capability::ResourceTemplates)?;
        let peer = self.peer().await?;
        let result = self
            .timed(peer.list_resource_templates(Some(
                PaginatedRequestParams::default().with_cursor(cursor),
            )))
            .await?;
        Ok(Page::new(
            result
                .resource_templates
                .into_iter()
                .map(remote_resource_template)
                .collect(),
            result.next_cursor,
        ))
    }

    async fn read_resource(
        &self,
        uri: &str,
        control: RequestControl,
    ) -> Result<ResourceReadOutcome> {
        self.negotiated
            .require(&self.server, Capability::Resources)?;
        let params = ReadResourceRequestParams::new(uri.to_owned());
        let request = ClientRequest::ReadResourceRequest(ReadResourceRequest::new(params));
        match self.request(request, control).await? {
            ServerResult::ReadResourceResult(result) => {
                let value = serde_json::to_value(result)
                    .map_err(|_| protocol_error(&self.server, "resource result encoding failed"))?;
                Ok(ResourceReadOutcome::Complete {
                    result: validate_resource_result(
                        &self.server,
                        &value,
                        &self.config.output_limits,
                    )?,
                })
            }
            ServerResult::InputRequiredResult(request) => {
                let value = serde_json::to_value(request).map_err(|_| {
                    protocol_error(&self.server, "input-required result encoding failed")
                })?;
                Ok(ResourceReadOutcome::InputRequired {
                    request: sanitize_json(&self.server, &value, &self.config.output_limits)?,
                })
            }
            _ => Err(protocol_error(
                &self.server,
                "unexpected resources/read response",
            )),
        }
    }

    async fn subscribe_resource(&self, uri: &str) -> Result<()> {
        self.negotiated
            .require(&self.server, Capability::ResourceSubscribe)?;
        if self.negotiated.is_modern() {
            self.subscription
                .lock()
                .await
                .resources
                .insert(uri.to_owned());
            self.restart_subscription().await
        } else {
            let peer = self.peer().await?;
            self.timed(peer.subscribe(SubscribeRequestParams::new(uri.to_owned())))
                .await
        }
    }

    async fn unsubscribe_resource(&self, uri: &str) -> Result<()> {
        self.negotiated
            .require(&self.server, Capability::ResourceSubscribe)?;
        if self.negotiated.is_modern() {
            self.subscription.lock().await.resources.remove(uri);
            self.restart_subscription().await
        } else {
            let peer = self.peer().await?;
            self.timed(peer.unsubscribe(UnsubscribeRequestParams::new(uri.to_owned())))
                .await
        }
    }

    async fn list_prompts(&self, cursor: Option<String>) -> Result<Page<RemotePrompt>> {
        self.negotiated.require(&self.server, Capability::Prompts)?;
        let peer = self.peer().await?;
        let result = self
            .timed(peer.list_prompts(Some(PaginatedRequestParams::default().with_cursor(cursor))))
            .await?;
        Ok(Page::new(
            result.prompts.into_iter().map(remote_prompt).collect(),
            result.next_cursor,
        ))
    }

    async fn get_prompt(
        &self,
        name: &str,
        arguments: BTreeMap<String, String>,
        control: RequestControl,
    ) -> Result<PromptGetOutcome> {
        self.negotiated.require(&self.server, Capability::Prompts)?;
        let arguments = arguments
            .into_iter()
            .map(|(name, value)| (name, Value::String(value)))
            .collect();
        let params = GetPromptRequestParams::new(name.to_owned()).with_arguments(arguments);
        let request = ClientRequest::GetPromptRequest(GetPromptRequest::new(params));
        match self.request(request, control).await? {
            ServerResult::GetPromptResult(result) => {
                let value = serde_json::to_value(result)
                    .map_err(|_| protocol_error(&self.server, "prompt result encoding failed"))?;
                Ok(PromptGetOutcome::Complete {
                    result: validate_prompt_result(
                        &self.server,
                        &value,
                        &self.config.output_limits,
                    )?,
                })
            }
            ServerResult::InputRequiredResult(request) => {
                let value = serde_json::to_value(request).map_err(|_| {
                    protocol_error(&self.server, "input-required result encoding failed")
                })?;
                Ok(PromptGetOutcome::InputRequired {
                    request: sanitize_json(&self.server, &value, &self.config.output_limits)?,
                })
            }
            _ => Err(protocol_error(
                &self.server,
                "unexpected prompts/get response",
            )),
        }
    }

    async fn complete(&self, request: Value, control: RequestControl) -> Result<Value> {
        self.negotiated
            .require(&self.server, Capability::Completion)?;
        let params: CompleteRequestParams = serde_json::from_value(request)
            .map_err(|_| validation_error(&self.server, "completion request is invalid"))?;
        let request = ClientRequest::CompleteRequest(CompleteRequest::new(params));
        match self.request(request, control).await? {
            ServerResult::CompleteResult(result) => {
                let value = serde_json::to_value(result).map_err(|_| {
                    protocol_error(&self.server, "completion result encoding failed")
                })?;
                validate_completion_result(&self.server, &value, &self.config.output_limits)
            }
            _ => Err(protocol_error(
                &self.server,
                "unexpected completion response",
            )),
        }
    }

    async fn set_log_level(&self, level: LogLevel) -> Result<()> {
        self.negotiated.require(&self.server, Capability::Logging)?;
        let level = to_rmcp_log_level(level);
        let request =
            ClientRequest::SetLevelRequest(SetLevelRequest::new(SetLevelRequestParams::new(level)));
        match self.request(request, RequestControl::new()).await? {
            ServerResult::EmptyResult(_) => Ok(()),
            _ => Err(protocol_error(&self.server, "unexpected logging response")),
        }
    }

    async fn notify_roots_changed(&self) -> Result<()> {
        if self.negotiated.is_modern() {
            return Err(Error::unsupported(
                self.server.clone(),
                "roots_changed on modern protocol",
            ));
        }
        let host_capabilities = self
            .running
            .read()
            .await
            .as_ref()
            .map(|running| running.service().host_capabilities())
            .ok_or_else(|| unavailable_error(&self.server))?;
        if !host_capabilities.roots_list_changed {
            return Err(Error::unsupported(self.server.clone(), "roots_changed"));
        }
        self.peer()
            .await?
            .notify_roots_list_changed()
            .await
            .map_err(|error| map_service_error(&self.server, error))
    }

    async fn get_task(&self, task_id: &str, control: RequestControl) -> Result<Value> {
        self.negotiated.require(&self.server, Capability::Tasks)?;
        let request = ClientRequest::GetTaskRequest(GetTaskRequest::new(GetTaskParams::new(
            task_id.to_owned(),
        )));
        match self.request(request, control).await? {
            ServerResult::GetTaskResult(result) => {
                let value = serde_json::to_value(result)
                    .map_err(|_| protocol_error(&self.server, "task result encoding failed"))?;
                sanitize_json(&self.server, &value, &self.config.output_limits)
            }
            _ => Err(protocol_error(
                &self.server,
                "unexpected tasks/get response",
            )),
        }
    }

    async fn update_task(
        &self,
        task_id: &str,
        responses: Value,
        control: RequestControl,
    ) -> Result<()> {
        self.negotiated.require(&self.server, Capability::Tasks)?;
        let params: UpdateTaskParams = serde_json::from_value(json!({
            "taskId": task_id,
            "inputResponses": responses,
        }))
        .map_err(|_| validation_error(&self.server, "tasks/update input is invalid"))?;
        let request = ClientRequest::UpdateTaskRequest(UpdateTaskRequest::new(params));
        match self.request(request, control).await? {
            ServerResult::TaskAckResult(_) | ServerResult::EmptyResult(_) => Ok(()),
            _ => Err(protocol_error(
                &self.server,
                "unexpected tasks/update response",
            )),
        }
    }

    async fn cancel_task(&self, task_id: &str, control: RequestControl) -> Result<()> {
        self.negotiated.require(&self.server, Capability::Tasks)?;
        let request = ClientRequest::CancelTaskRequest(CancelTaskRequest::new(
            CancelTaskParams::new(task_id.to_owned()),
        ));
        match self.request(request, control).await? {
            ServerResult::TaskAckResult(_) | ServerResult::EmptyResult(_) => Ok(()),
            _ => Err(protocol_error(
                &self.server,
                "unexpected tasks/cancel response",
            )),
        }
    }

    async fn close(&self) -> Result<()> {
        self.stop_subscription().await;
        let mut running = self.running.write().await;
        let Some(mut running) = running.take() else {
            return Ok(());
        };
        match running
            .close_with_timeout(self.config.timeouts.shutdown())
            .await
        {
            Ok(Some(_)) => Ok(()),
            Ok(None) => Err(Error::new(
                ErrorKind::Shutdown,
                Recovery::Fatal,
                "MCP session shutdown timed out",
            )
            .with_server(self.server.clone())),
            Err(_) => Err(Error::new(
                ErrorKind::Shutdown,
                Recovery::Fatal,
                "MCP session shutdown task failed",
            )
            .with_server(self.server.clone())),
        }
    }
}

fn negotiated_server(info: &ServerPeerInfo) -> NegotiatedServer {
    let capabilities = &info.capabilities;
    let resources = capabilities.resources.as_ref();
    let tools = capabilities.tools.as_ref();
    let prompts = capabilities.prompts.as_ref();
    NegotiatedServer {
        protocol_version: info.protocol_version.to_string(),
        implementation_name: info.server_info.as_ref().map(|value| value.name.clone()),
        implementation_version: info.server_info.as_ref().map(|value| value.version.clone()),
        capabilities: NegotiatedCapabilities {
            tools: tools.is_some(),
            tool_list_changed: tools.is_some_and(|value| value.list_changed == Some(true)),
            resources: resources.is_some(),
            resource_templates: resources.is_some(),
            resource_subscribe: resources.is_some_and(|value| value.subscribe == Some(true)),
            resource_list_changed: resources.is_some_and(|value| value.list_changed == Some(true)),
            prompts: prompts.is_some(),
            prompt_list_changed: prompts.is_some_and(|value| value.list_changed == Some(true)),
            completion: capabilities.completions.is_some(),
            logging: capabilities.logging.is_some(),
            tasks: capabilities.supports_tasks(),
        },
    }
}

fn remote_tool(tool: Tool) -> Result<RemoteTool> {
    Ok(RemoteTool {
        name: tool.name.into_owned(),
        title: tool.title,
        description: tool.description.map(|value| value.into_owned()),
        input_schema: Value::Object(tool.input_schema.as_ref().clone()),
        output_schema: tool
            .output_schema
            .map(|schema| Value::Object(schema.as_ref().clone())),
        annotations: tool.annotations.map(|annotations| {
            serde_json::to_value(annotations).unwrap_or_else(|_| Value::Object(Map::new()))
        }),
    })
}

fn remote_resource(resource: rmcp::model::Resource) -> RemoteResource {
    RemoteResource {
        uri: resource.uri,
        name: resource.name,
        title: resource.title,
        description: resource.description,
        mime_type: resource.mime_type,
        size: resource.size,
    }
}

fn remote_resource_template(template: rmcp::model::ResourceTemplate) -> ProtocolResourceTemplate {
    ProtocolResourceTemplate {
        uri_template: template.uri_template,
        name: template.name,
        title: template.title,
        description: template.description,
        mime_type: template.mime_type,
    }
}

fn remote_prompt(prompt: Prompt) -> RemotePrompt {
    RemotePrompt {
        name: prompt.name,
        title: prompt.title,
        description: prompt.description,
        arguments: prompt
            .arguments
            .unwrap_or_default()
            .into_iter()
            .map(|argument| RemotePromptArgument {
                name: argument.name,
                title: argument.title,
                description: argument.description,
                required: argument.required.unwrap_or(false),
            })
            .collect(),
    }
}

fn subscription_event(
    notification: ServerNotification,
    server: &ServerName,
    limits: &crate::config::OutputLimits,
    max_string: usize,
) -> Option<SessionEvent> {
    match notification {
        ServerNotification::ToolListChangedNotification(_) => Some(SessionEvent::ToolListChanged),
        ServerNotification::PromptListChangedNotification(_) => {
            Some(SessionEvent::PromptListChanged)
        }
        ServerNotification::ResourceListChangedNotification(_) => {
            Some(SessionEvent::ResourceListChanged)
        }
        ServerNotification::ResourceUpdatedNotification(notification) => {
            Some(SessionEvent::ResourceUpdated {
                uri: sanitize_text(&notification.params.uri, max_string),
            })
        }
        ServerNotification::TaskStatusNotification(notification) => {
            serde_json::to_value(notification.params)
                .ok()
                .and_then(|value| sanitize_json(server, &value, limits).ok())
                .map(SessionEvent::TaskStatus)
        }
        _ => None,
    }
}

fn from_rmcp_log_level(level: LoggingLevel) -> LogLevel {
    match level {
        LoggingLevel::Debug => LogLevel::Debug,
        LoggingLevel::Info => LogLevel::Info,
        LoggingLevel::Notice => LogLevel::Notice,
        LoggingLevel::Warning => LogLevel::Warning,
        LoggingLevel::Error => LogLevel::Error,
        LoggingLevel::Critical => LogLevel::Critical,
        LoggingLevel::Alert => LogLevel::Alert,
        LoggingLevel::Emergency => LogLevel::Emergency,
    }
}

fn to_rmcp_log_level(level: LogLevel) -> LoggingLevel {
    match level {
        LogLevel::Debug => LoggingLevel::Debug,
        LogLevel::Info => LoggingLevel::Info,
        LogLevel::Notice => LoggingLevel::Notice,
        LogLevel::Warning => LoggingLevel::Warning,
        LogLevel::Error => LoggingLevel::Error,
        LogLevel::Critical => LoggingLevel::Critical,
        LogLevel::Alert => LoggingLevel::Alert,
        LogLevel::Emergency => LoggingLevel::Emergency,
    }
}

fn map_service_error(server: &ServerName, error: ServiceError) -> Error {
    match error {
        ServiceError::Timeout { .. } => timeout_error(server),
        ServiceError::Cancelled { .. } => Error::new(
            ErrorKind::Cancelled,
            Recovery::Fatal,
            "MCP peer cancelled the request",
        )
        .with_server(server.clone()),
        ServiceError::TransportClosed | ServiceError::TransportSend(_) => {
            Error::transport(server.clone(), "MCP transport closed")
        }
        ServiceError::McpError(_) | ServiceError::UnexpectedResponse => {
            protocol_error(server, "MCP peer returned a protocol error")
        }
        _ => protocol_error(server, "MCP request failed"),
    }
}

async fn protected_call<T>(
    cancellation: &CancellationToken,
    future: impl std::future::Future<Output = Result<T>>,
) -> std::result::Result<T, rmcp::ErrorData> {
    tokio::select! {
        result = future => result.map_err(to_rmcp_error),
        () = cancellation.cancelled() => Err(rmcp_error(
            "protected server-to-client operation was cancelled",
            "cancelled",
        )),
    }
}

fn to_rmcp_error(error: Error) -> rmcp::ErrorData {
    rmcp::ErrorData::new(
        rmcp::model::ErrorCode::INVALID_REQUEST,
        error.to_string(),
        Some(json!({
            "kind": format!("{:?}", error.kind()),
            "server": error.server().map(ToString::to_string),
        })),
    )
}

fn rmcp_error(message: &'static str, kind: &'static str) -> rmcp::ErrorData {
    rmcp::ErrorData::new(
        rmcp::model::ErrorCode::INVALID_REQUEST,
        message,
        Some(json!({"kind":kind})),
    )
}

fn cancel_request(handle: RequestHandle<RoleClient>, reason: &'static str, timeout: Duration) {
    tokio::spawn(async move {
        let _ = tokio::time::timeout(timeout, handle.cancel(Some(reason.to_owned()))).await;
    });
}

fn cancelled_error(server: &ServerName) -> Error {
    Error::new(
        ErrorKind::Cancelled,
        Recovery::Fatal,
        "MCP request was cancelled",
    )
    .with_server(server.clone())
}

fn timeout_error(server: &ServerName) -> Error {
    Error::new(
        ErrorKind::Timeout,
        Recovery::Recoverable,
        "MCP request exceeded its deadline",
    )
    .with_server(server.clone())
}

fn protocol_error(server: &ServerName, message: impl AsRef<str>) -> Error {
    Error::new(ErrorKind::Protocol, Recovery::Fatal, message).with_server(server.clone())
}

fn validation_error(server: &ServerName, message: impl AsRef<str>) -> Error {
    Error::new(ErrorKind::Validation, Recovery::Fatal, message).with_server(server.clone())
}

fn unavailable_error(server: &ServerName) -> Error {
    Error::new(
        ErrorKind::Unavailable,
        Recovery::Recoverable,
        "MCP server session is not connected",
    )
    .with_server(server.clone())
}

const _: [&str; 2] = [MCP_PROTOCOL_VERSION, MCP_LEGACY_PROTOCOL_VERSION];

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use async_trait::async_trait;
    use serde_json::json;
    use tokio::sync::mpsc;

    use super::*;
    use crate::{
        TrustConfig,
        host::{
            AuthHostHandle, ElicitationResponse, HostCapabilities, LogEvent, McpHost, NoAuthHost,
            PermissionDecision, PermissionRequest, Root, SamplingResponse,
        },
    };

    #[derive(Debug, Clone)]
    struct RecordingHost {
        authorizations: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl McpHost for RecordingHost {
        fn capabilities(&self, _context: &HostContext) -> HostCapabilities {
            HostCapabilities {
                sampling: true,
                form_elicitation: true,
                roots: true,
                ..HostCapabilities::default()
            }
        }

        async fn authorize(
            &self,
            _context: &HostContext,
            _request: PermissionRequest,
        ) -> Result<PermissionDecision> {
            self.authorizations.fetch_add(1, Ordering::SeqCst);
            Ok(PermissionDecision::AllowOnce)
        }

        async fn sample(
            &self,
            _context: &HostContext,
            request: SamplingRequest,
        ) -> Result<SamplingResponse> {
            Ok(SamplingResponse {
                result: request.params,
            })
        }

        async fn elicit(
            &self,
            _context: &HostContext,
            _request: ElicitationRequest,
        ) -> Result<ElicitationResponse> {
            Ok(ElicitationResponse::Decline)
        }

        async fn roots(&self, _context: &HostContext) -> Result<Vec<Root>> {
            Ok(Vec::new())
        }

        async fn log(&self, _context: &HostContext, _event: LogEvent) {}
    }

    fn context(trust: TrustConfig, host: McpHostHandle) -> ConnectContext {
        let config: ServerConfig = serde_json::from_value(json!({
            "transport": {"type":"stdio", "command":"fake"},
            "trust": trust,
        }))
        .unwrap();
        let (sender, _receiver) = mpsc::channel(8);
        ConnectContext {
            server: ServerName::new("protected").unwrap(),
            config,
            host,
            auth: AuthHostHandle::new(NoAuthHost),
            events: SessionEventSink::new(sender),
        }
    }

    #[test]
    fn sse_retry_honors_per_stream_current_times_and_disabled_policy() {
        let disabled = ConfiguredSseRetry(SseReconnectGate::new(crate::ReconnectConfig {
            enabled: false,
            max_attempts: 5,
            initial_delay_ms: 10,
            max_delay_ms: 20,
        }));
        assert_eq!(disabled.retry(0), None);

        let bounded = ConfiguredSseRetry(SseReconnectGate::new(crate::ReconnectConfig {
            enabled: true,
            max_attempts: 4,
            initial_delay_ms: 10,
            max_delay_ms: 25,
        }));
        assert_eq!(bounded.retry(0), Some(Duration::from_millis(10)));
        assert_eq!(bounded.retry(1), Some(Duration::from_millis(20)));
        assert_eq!(bounded.retry(2), Some(Duration::from_millis(25)));
        assert_eq!(bounded.retry(3), Some(Duration::from_millis(25)));
        assert_eq!(bounded.retry(4), None);
        assert_eq!(bounded.retry(usize::MAX), None);

        let other_stream = ConfiguredSseRetry(bounded.0.clone());
        assert_eq!(other_stream.retry(0), Some(Duration::from_millis(10)));
        assert_eq!(bounded.retry(4), None);
    }

    #[test]
    fn sse_auto_reconnect_composed_delay_does_not_exceed_policy() {
        let config = crate::ReconnectConfig {
            enabled: true,
            max_attempts: 4,
            initial_delay_ms: 10,
            max_delay_ms: 25,
        };
        let gate = SseReconnectGate::new(config.clone());
        let policy = ConfiguredSseRetry(gate.clone());
        let token = gate.stream_token();

        assert_eq!(gate.reconnect_request_delay(true, false).unwrap(), None);
        token.note_live();

        let policy_wait = token.begin_policy_retry(0).unwrap();
        let extra = token.extra_get_delay();
        assert_eq!(policy_wait + extra.unwrap_or_default(), config.delay(0));
        assert_eq!(
            extra, None,
            "EOF already waited retry(0); GET must not add initial_delay"
        );

        token.note_live();
        assert_eq!(
            token.extra_get_delay(),
            Some(config.delay(0)),
            "mid-stream error first GET is the SDK-skipped wait"
        );

        for attempt in 1..4 {
            let policy_wait = token.begin_policy_retry(attempt).unwrap();
            let extra = token.extra_get_delay();
            let total = policy_wait + extra.unwrap_or_default();
            assert_eq!(extra, None);
            assert_eq!(total, config.delay(attempt as u32));
            assert!(
                total <= Duration::from_millis(config.max_delay_ms),
                "composed reconnect wait {total:?} exceeded max_delay_ms"
            );
        }
        assert_eq!(policy.retry(4), None);
        assert_eq!(token.begin_policy_retry(4), None);
    }

    #[test]
    fn sse_retry_stream_context_is_independent_per_stream() {
        let policy = ConfiguredSseRetry(SseReconnectGate::new(crate::ReconnectConfig {
            enabled: true,
            max_attempts: 4,
            initial_delay_ms: 10,
            max_delay_ms: 25,
        }));
        let stream_a = policy.stream_context();
        let stream_b = policy.stream_context();
        stream_a.note_live();
        assert_eq!(
            stream_b.extra_get_delay(),
            None,
            "stream B must not observe A's live credit"
        );
        assert_eq!(stream_a.extra_get_delay(), Some(Duration::from_millis(10)));
        assert_eq!(policy.retry(0), Some(Duration::from_millis(10)));
    }

    #[tokio::test]
    async fn protected_operations_require_trust_before_product_permission() {
        let count = Arc::new(AtomicUsize::new(0));
        let host = McpHostHandle::new(RecordingHost {
            authorizations: Arc::clone(&count),
        });
        let bridge = HostBridge::new(&context(TrustConfig::default(), host));
        assert_eq!(
            bridge
                .authorize(HostOperation::Sampling, json!({"messages":[]}))
                .await
                .unwrap_err()
                .kind(),
            ErrorKind::Trust
        );
        assert_eq!(count.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn trusted_sampling_elicitation_and_oauth_cross_permission_port() {
        let count = Arc::new(AtomicUsize::new(0));
        let host = McpHostHandle::new(RecordingHost {
            authorizations: Arc::clone(&count),
        });
        let trust = TrustConfig {
            level: TrustLevel::Trusted,
            allow_sampling: true,
            allow_elicitation: true,
            allow_oauth: true,
            ..TrustConfig::default()
        };
        let bridge = HostBridge::new(&context(trust, host));
        bridge
            .authorize(HostOperation::Sampling, json!({"messages":[]}))
            .await
            .unwrap();
        bridge
            .authorize(HostOperation::FormElicitation, json!({"type":"object"}))
            .await
            .unwrap();
        bridge
            .authorize(HostOperation::OAuth, json!({"operation":"oauth2.1"}))
            .await
            .unwrap();
        assert_eq!(count.load(Ordering::SeqCst), 3);
    }
}

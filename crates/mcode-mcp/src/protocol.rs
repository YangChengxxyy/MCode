//! Adapter-friendly MCP client protocol surface.

// Rust guideline compliant 2026-08-20.

use std::{collections::BTreeMap, fmt, sync::Arc};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::{
    config::ServerConfig,
    error::{Error, Result},
    host::{AuthHostHandle, LogEvent, McpHostHandle},
    identity::ServerName,
};

/// Official `rmcp` SDK release pinned by Cargo and audited by this crate.
pub const RMCP_SDK_VERSION: &str = "3.1.4";
/// Protocol version explicitly advertised for current discover-mode peers.
pub const MCP_PROTOCOL_VERSION: &str = "2026-07-28";
/// Explicit legacy fallback used for initialize-mode interoperability.
pub const MCP_LEGACY_PROTOCOL_VERSION: &str = "2025-11-25";

/// One negotiated operation family.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub enum Capability {
    /// Tool listing and invocation.
    Tools,
    /// Tool list-change notifications.
    ToolListChanged,
    /// Resource listing and reading.
    Resources,
    /// Resource templates.
    ResourceTemplates,
    /// Resource subscriptions.
    ResourceSubscribe,
    /// Resource list-change notifications.
    ResourceListChanged,
    /// Prompt listing and retrieval.
    Prompts,
    /// Prompt list-change notifications.
    PromptListChanged,
    /// Argument completion.
    Completion,
    /// Legacy server logging controls and notifications.
    Logging,
    /// Negotiated tasks extension.
    Tasks,
}

/// Capabilities advertised by one negotiated server.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NegotiatedCapabilities {
    /// Tool operations.
    pub tools: bool,
    /// Tool list-change notifications.
    pub tool_list_changed: bool,
    /// Resource operations.
    pub resources: bool,
    /// Resource template listing.
    pub resource_templates: bool,
    /// Resource subscriptions.
    pub resource_subscribe: bool,
    /// Resource list-change notifications.
    pub resource_list_changed: bool,
    /// Prompt operations.
    pub prompts: bool,
    /// Prompt list-change notifications.
    pub prompt_list_changed: bool,
    /// Completion operations.
    pub completion: bool,
    /// Legacy logging operations.
    pub logging: bool,
    /// Tasks extension operations.
    pub tasks: bool,
}

impl NegotiatedCapabilities {
    /// Tests whether a capability was actually negotiated.
    #[must_use]
    pub const fn supports(&self, capability: Capability) -> bool {
        match capability {
            Capability::Tools => self.tools,
            Capability::ToolListChanged => self.tools && self.tool_list_changed,
            Capability::Resources => self.resources,
            Capability::ResourceTemplates => self.resources && self.resource_templates,
            Capability::ResourceSubscribe => self.resources && self.resource_subscribe,
            Capability::ResourceListChanged => self.resources && self.resource_list_changed,
            Capability::Prompts => self.prompts,
            Capability::PromptListChanged => self.prompts && self.prompt_list_changed,
            Capability::Completion => self.completion,
            Capability::Logging => self.logging,
            Capability::Tasks => self.tasks,
        }
    }
}

/// Server identity and capabilities fixed by protocol negotiation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NegotiatedServer {
    /// Selected protocol version; never inferred from a later SDK default.
    pub protocol_version: String,
    /// Optional implementation name.
    pub implementation_name: Option<String>,
    /// Optional implementation version.
    pub implementation_version: Option<String>,
    /// Server-advertised capabilities.
    pub capabilities: NegotiatedCapabilities,
}

impl NegotiatedServer {
    /// Fails closed when a server did not advertise a required operation.
    ///
    /// # Errors
    ///
    /// Returns [`crate::error::ErrorKind::UnsupportedCapability`] when absent.
    pub fn require(&self, server: &ServerName, capability: Capability) -> Result<()> {
        if self.capabilities.supports(capability) {
            Ok(())
        } else {
            Err(Error::unsupported(
                server.clone(),
                format!("{capability:?}"),
            ))
        }
    }

    /// Returns whether this connection uses modern discover lifecycle semantics.
    #[must_use]
    pub fn is_modern(&self) -> bool {
        self.protocol_version.as_str() >= MCP_PROTOCOL_VERSION
    }
}

/// One bounded page returned by a list operation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Page<T> {
    /// Items on this page.
    pub items: Vec<T>,
    /// Opaque cursor for the next page.
    pub next_cursor: Option<String>,
}

impl<T> Page<T> {
    /// Creates one page.
    #[must_use]
    pub fn new(items: Vec<T>, next_cursor: Option<String>) -> Self {
        Self { items, next_cursor }
    }
}

/// Server-provided tool metadata before provenance is attached.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteTool {
    /// Remote name.
    pub name: String,
    /// Optional title.
    pub title: Option<String>,
    /// Optional description.
    pub description: Option<String>,
    /// Input JSON Schema.
    pub input_schema: Value,
    /// Optional output JSON Schema.
    pub output_schema: Option<Value>,
    /// Untrusted hint metadata retained for display only.
    pub annotations: Option<Value>,
}

/// Server-provided concrete resource metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteResource {
    /// Resource URI.
    pub uri: String,
    /// Remote name.
    pub name: String,
    /// Optional title.
    pub title: Option<String>,
    /// Optional description.
    pub description: Option<String>,
    /// Optional MIME type.
    pub mime_type: Option<String>,
    /// Optional raw byte size.
    pub size: Option<u64>,
}

/// Server-provided resource template metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteResourceTemplate {
    /// RFC 6570 URI template.
    pub uri_template: String,
    /// Remote name.
    pub name: String,
    /// Optional title.
    pub title: Option<String>,
    /// Optional description.
    pub description: Option<String>,
    /// Optional MIME type.
    pub mime_type: Option<String>,
}

/// One server-provided prompt argument.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemotePromptArgument {
    /// Argument name.
    pub name: String,
    /// Optional title.
    pub title: Option<String>,
    /// Optional description.
    pub description: Option<String>,
    /// Whether the argument is required.
    pub required: bool,
}

/// Server-provided prompt metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemotePrompt {
    /// Remote name.
    pub name: String,
    /// Optional title.
    pub title: Option<String>,
    /// Optional description.
    pub description: Option<String>,
    /// Declared prompt arguments.
    pub arguments: Vec<RemotePromptArgument>,
}

/// Cooperative cancellation shared with one request.
#[derive(Debug, Clone, Default)]
pub struct RequestCancellation(CancellationToken);

impl RequestCancellation {
    /// Creates an uncancelled request token.
    #[must_use]
    pub fn new() -> Self {
        Self(CancellationToken::new())
    }

    /// Requests cancellation.
    pub fn cancel(&self) {
        self.0.cancel();
    }

    /// Returns whether cancellation was requested.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.0.is_cancelled()
    }

    /// Waits until cancellation is requested.
    pub async fn cancelled(&self) {
        self.0.cancelled().await;
    }
}

/// One bounded MCP progress update.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgressUpdate {
    /// Current progress value.
    pub progress: f64,
    /// Optional total value.
    pub total: Option<f64>,
    /// Optional sanitized message.
    pub message: Option<String>,
}

/// Observer for request progress.
pub trait RequestObserver: Send + Sync + 'static {
    /// Receives one best-effort progress update.
    fn on_progress(&self, update: ProgressUpdate);
}

/// Cloneable, type-erased request observer.
#[derive(Clone)]
pub struct RequestObserverHandle(Arc<dyn RequestObserver>);

impl RequestObserverHandle {
    /// Erases a concrete progress observer.
    #[must_use]
    pub fn new(observer: impl RequestObserver) -> Self {
        Self(Arc::new(observer))
    }

    /// Delivers one progress update to the wrapped observer.
    pub fn notify(&self, update: ProgressUpdate) {
        self.0.on_progress(update);
    }
}

impl fmt::Debug for RequestObserverHandle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("RequestObserverHandle(..)")
    }
}

/// No-op progress observer.
#[derive(Debug, Clone, Copy, Default)]
pub struct IgnoreProgress;

impl RequestObserver for IgnoreProgress {
    fn on_progress(&self, _update: ProgressUpdate) {}
}

/// Per-request cancellation, progress, and timeout behavior.
#[derive(Debug, Clone)]
pub struct RequestControl {
    /// Cooperative cancellation.
    pub cancellation: RequestCancellation,
    /// Best-effort progress observer.
    pub observer: RequestObserverHandle,
}

impl RequestControl {
    /// Creates request control with no cancellation and ignored progress.
    #[must_use]
    pub fn new() -> Self {
        Self {
            cancellation: RequestCancellation::new(),
            observer: RequestObserverHandle::new(IgnoreProgress),
        }
    }

    /// Uses the supplied cancellation token.
    #[must_use]
    pub fn with_cancellation(mut self, cancellation: RequestCancellation) -> Self {
        self.cancellation = cancellation;
        self
    }

    /// Uses the supplied progress observer.
    #[must_use]
    pub fn with_observer(mut self, observer: RequestObserverHandle) -> Self {
        self.observer = observer;
        self
    }
}

impl Default for RequestControl {
    fn default() -> Self {
        Self::new()
    }
}

/// Result of one `tools/call` request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "resultType", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ToolCallOutcome {
    /// Tool execution completed.
    Complete {
        /// Validated MCP result object.
        result: Value,
        /// Whether the server marked the tool result as an execution error.
        is_error: bool,
    },
    /// The latest tasks extension materialized a task.
    Task {
        /// Validated task creation result.
        task: Value,
    },
    /// The peer requested a bounded multi-round input exchange.
    InputRequired {
        /// Raw validated input request envelope for a future host adapter.
        request: Value,
    },
}

/// Result of one resource read.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "resultType", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ResourceReadOutcome {
    /// Read completed.
    Complete {
        /// Validated MCP result object.
        result: Value,
    },
    /// The peer requested a bounded multi-round input exchange.
    InputRequired {
        /// Raw validated input request envelope.
        request: Value,
    },
}

/// Result of one prompt retrieval.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "resultType", rename_all = "snake_case")]
#[non_exhaustive]
pub enum PromptGetOutcome {
    /// Prompt retrieval completed.
    Complete {
        /// Validated MCP result object.
        result: Value,
    },
    /// The peer requested a bounded multi-round input exchange.
    InputRequired {
        /// Raw validated input request envelope.
        request: Value,
    },
}

/// Events emitted by a connected session to its owning server actor.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum SessionEvent {
    /// Tool catalog invalidation.
    ToolListChanged,
    /// Resource and template catalog invalidation.
    ResourceListChanged,
    /// Prompt catalog invalidation.
    PromptListChanged,
    /// A subscribed resource changed.
    ResourceUpdated { uri: String },
    /// Sanitized server log event.
    Log(LogEvent),
    /// The transport ended unexpectedly.
    Disconnected(Error),
    /// One task changed status.
    TaskStatus(Value),
}

/// Bounded event sink connecting a protocol adapter to its server actor.
#[derive(Debug, Clone)]
pub struct SessionEventSink(mpsc::Sender<SessionEvent>);

impl SessionEventSink {
    /// Wraps an actor event sender.
    #[must_use]
    pub fn new(sender: mpsc::Sender<SessionEvent>) -> Self {
        Self(sender)
    }

    /// Delivers an event with backpressure.
    ///
    /// # Errors
    ///
    /// Returns an unavailable error when the owning actor has stopped.
    pub async fn send(&self, event: SessionEvent) -> Result<()> {
        self.0.send(event).await.map_err(|_| {
            Error::new(
                crate::error::ErrorKind::Unavailable,
                crate::error::Recovery::Fatal,
                "MCP server actor event channel is closed",
            )
        })
    }

    /// Attempts best-effort delivery without blocking a child stderr drain.
    #[must_use]
    pub fn try_send(&self, event: SessionEvent) -> bool {
        self.0.try_send(event).is_ok()
    }
}

/// Complete dependencies for connecting one server.
#[derive(Debug, Clone)]
pub struct ConnectContext {
    /// Validated server name.
    pub server: ServerName,
    /// Validated per-server configuration.
    pub config: ServerConfig,
    /// Server-to-client host adapter.
    pub host: McpHostHandle,
    /// Credential and browser adapter.
    pub auth: AuthHostHandle,
    /// Actor event sink.
    pub events: SessionEventSink,
}

/// One established MCP client session.
#[async_trait]
pub trait McpSession: Send + Sync + 'static {
    /// Returns immutable negotiated server information.
    fn negotiated(&self) -> &NegotiatedServer;

    /// Sends a protocol ping.
    async fn ping(&self) -> Result<()>;

    /// Lists one tools page.
    async fn list_tools(&self, cursor: Option<String>) -> Result<Page<RemoteTool>>;

    /// Calls one tool exactly once; side-effecting requests are never retried.
    async fn call_tool(
        &self,
        name: &str,
        arguments: Value,
        control: RequestControl,
    ) -> Result<ToolCallOutcome>;

    /// Lists one resources page.
    async fn list_resources(&self, cursor: Option<String>) -> Result<Page<RemoteResource>>;

    /// Lists one resource-templates page.
    async fn list_resource_templates(
        &self,
        cursor: Option<String>,
    ) -> Result<Page<RemoteResourceTemplate>>;

    /// Reads one resource URI.
    async fn read_resource(
        &self,
        uri: &str,
        control: RequestControl,
    ) -> Result<ResourceReadOutcome>;

    /// Subscribes to resource updates.
    async fn subscribe_resource(&self, uri: &str) -> Result<()>;

    /// Unsubscribes from resource updates.
    async fn unsubscribe_resource(&self, uri: &str) -> Result<()>;

    /// Lists one prompts page.
    async fn list_prompts(&self, cursor: Option<String>) -> Result<Page<RemotePrompt>>;

    /// Gets one prompt.
    async fn get_prompt(
        &self,
        name: &str,
        arguments: BTreeMap<String, String>,
        control: RequestControl,
    ) -> Result<PromptGetOutcome>;

    /// Requests argument completion.
    async fn complete(&self, request: Value, control: RequestControl) -> Result<Value>;

    /// Sets the legacy remote log level.
    async fn set_log_level(&self, level: crate::host::LogLevel) -> Result<()>;

    /// Announces changed roots to a legacy server.
    async fn notify_roots_changed(&self) -> Result<()>;

    /// Polls one negotiated task.
    async fn get_task(&self, task_id: &str, control: RequestControl) -> Result<Value>;

    /// Supplies input responses to one negotiated task.
    async fn update_task(
        &self,
        task_id: &str,
        responses: Value,
        control: RequestControl,
    ) -> Result<()>;

    /// Requests cooperative cancellation of one negotiated task.
    async fn cancel_task(&self, task_id: &str, control: RequestControl) -> Result<()>;

    /// Gracefully closes this session and its contained transport.
    async fn close(&self) -> Result<()>;
}

/// Cloneable, type-erased established session.
#[derive(Clone)]
pub struct McpSessionHandle(Arc<dyn McpSession>);

impl McpSessionHandle {
    /// Erases a concrete protocol session.
    #[must_use]
    pub fn new(session: impl McpSession) -> Self {
        Self(Arc::new(session))
    }

    /// Returns immutable negotiated server information.
    #[must_use]
    pub fn negotiated(&self) -> &NegotiatedServer {
        self.0.negotiated()
    }

    /// Borrows the adapter-neutral session interface.
    #[must_use]
    pub fn as_session(&self) -> &dyn McpSession {
        self.0.as_ref()
    }

    pub(crate) fn inner(&self) -> &dyn McpSession {
        self.as_session()
    }
}

impl fmt::Debug for McpSessionHandle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("McpSessionHandle")
            .field("negotiated", self.negotiated())
            .finish()
    }
}

/// Factory for transport-specific MCP client sessions.
#[async_trait]
pub trait McpConnector: Send + Sync + 'static {
    /// Connects, negotiates, and returns one isolated client session.
    async fn connect(&self, context: ConnectContext) -> Result<McpSessionHandle>;
}

/// Cloneable, type-erased session connector.
#[derive(Clone)]
pub struct McpConnectorHandle(Arc<dyn McpConnector>);

impl McpConnectorHandle {
    /// Erases a concrete connector.
    #[must_use]
    pub fn new(connector: impl McpConnector) -> Self {
        Self(Arc::new(connector))
    }

    /// Connects one server through the wrapped adapter.
    ///
    /// # Errors
    ///
    /// Returns sanitized configuration, authentication, transport, or protocol errors.
    pub async fn connect(&self, context: ConnectContext) -> Result<McpSessionHandle> {
        self.0.connect(context).await
    }
}

impl fmt::Debug for McpConnectorHandle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("McpConnectorHandle(..)")
    }
}

//! UI-neutral trust, permission, sampling, elicitation, roots, and auth ports.

// Rust guideline compliant 2026-08-20.

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    config::TrustConfig,
    error::{Error, ErrorKind, Recovery, Result},
    identity::ServerName,
    secret::{SecretBytes, SecretRef, SecretStoreKey, SecretValue},
};

/// Immutable provenance supplied to every host callback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostContext {
    /// Server making the request.
    pub server: ServerName,
    /// Explicit trust grants for that server.
    pub trust: TrustConfig,
}

/// Host capabilities that may be advertised to an MCP server.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostCapabilities {
    /// The host can service permission-gated sampling requests.
    pub sampling: bool,
    /// Sampling responses may include server-supplied tool definitions.
    pub sampling_tools: bool,
    /// The host can service form-mode elicitation.
    pub form_elicitation: bool,
    /// The host can service URL-mode elicitation.
    pub url_elicitation: bool,
    /// The host can provide permission-filtered roots.
    pub roots: bool,
    /// The host can announce roots changes on legacy protocol sessions.
    pub roots_list_changed: bool,
    /// The client implements the negotiated tasks extension.
    pub tasks: bool,
}

/// A sensitive operation requiring a host permission decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub enum HostOperation {
    /// Server-initiated LLM sampling.
    Sampling,
    /// Server-initiated structured form elicitation.
    FormElicitation,
    /// Server-initiated URL presentation.
    UrlElicitation,
    /// Sharing filesystem or URI roots with a server.
    Roots,
    /// Interactive OAuth authorization for a remote server.
    OAuth,
}

/// A sanitized request for one host permission decision.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionRequest {
    /// Operation the remote server requested.
    pub operation: HostOperation,
    /// Bounded, secret-free preview suitable for a consent UI.
    pub preview: Value,
}

/// A structured host permission decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "camelCase")]
#[non_exhaustive]
pub enum PermissionDecision {
    /// Permit this single request.
    AllowOnce,
    /// Reject without attempting the operation.
    Deny {
        /// Sanitized reason suitable for a JSON-RPC error payload.
        reason: String,
    },
}

/// A sampling request containing only server-supplied MCP context.
///
/// The engine never appends a host conversation. A provider adapter must make
/// any additional context sharing an independent, explicit permission action.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SamplingRequest {
    /// Validated MCP `sampling/createMessage` parameters.
    pub params: Value,
}

/// A validated host sampling response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SamplingResponse {
    /// MCP-compatible result object.
    pub result: Value,
}

/// A sanitized server elicitation request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "camelCase")]
#[non_exhaustive]
pub enum ElicitationRequest {
    /// Collect non-secret structured values according to a bounded schema.
    Form {
        /// Human-readable prompt stripped of terminal controls.
        message: String,
        /// Validated requested JSON Schema.
        requested_schema: Value,
    },
    /// Present a bounded HTTPS URL for an out-of-band flow.
    Url {
        /// Human-readable prompt stripped of terminal controls.
        message: String,
        /// URL to present; the MCP server receives no browser credentials.
        url: String,
        /// Server correlation identifier.
        elicitation_id: String,
    },
}

/// Result of a host elicitation interaction.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "camelCase")]
#[non_exhaustive]
pub enum ElicitationResponse {
    /// The user accepted and supplied non-secret structured content.
    Accept {
        /// Content validated against the requested form schema.
        #[serde(default)]
        content: Option<Value>,
    },
    /// The user explicitly declined.
    Decline,
    /// The user cancelled the interaction.
    Cancel,
}

/// One root the host may share with an authorized MCP server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Root {
    /// Root URI.
    pub uri: String,
    /// Optional human-readable label.
    #[serde(default)]
    pub name: Option<String>,
}

/// Sanitized remote logging level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub enum LogLevel {
    /// Debug-level message.
    Debug,
    /// Informational message.
    Info,
    /// Notice-level message.
    Notice,
    /// Warning message.
    Warning,
    /// Error message.
    Error,
    /// Critical message.
    Critical,
    /// Alert message.
    Alert,
    /// Emergency message.
    Emergency,
}

/// A bounded, sanitized log notification.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogEvent {
    /// Remote log level.
    pub level: LogLevel,
    /// Optional logger name.
    pub logger: Option<String>,
    /// Sanitized JSON data.
    pub data: Value,
}

/// Host integration used for server-to-client operations.
///
/// Implementations normally live in the first-party plugin host. They must not
/// depend on this crate's caller being a TUI; headless adapters can deny safely.
#[async_trait]
pub trait McpHost: Send + Sync + 'static {
    /// Returns capabilities the concrete host can actually service.
    fn capabilities(&self, context: &HostContext) -> HostCapabilities;

    /// Resolves trust and interactive permission for one operation.
    async fn authorize(
        &self,
        context: &HostContext,
        request: PermissionRequest,
    ) -> Result<PermissionDecision>;

    /// Performs one sampling request without adding the host conversation.
    async fn sample(
        &self,
        context: &HostContext,
        request: SamplingRequest,
    ) -> Result<SamplingResponse>;

    /// Presents one sanitized elicitation request.
    async fn elicit(
        &self,
        context: &HostContext,
        request: ElicitationRequest,
    ) -> Result<ElicitationResponse>;

    /// Returns roots filtered for this server and permission decision.
    async fn roots(&self, context: &HostContext) -> Result<Vec<Root>>;

    /// Receives one sanitized remote log notification.
    async fn log(&self, context: &HostContext, event: LogEvent);
}

/// Cloneable, type-erased handle for an [`McpHost`].
#[derive(Clone)]
pub struct McpHostHandle(Arc<dyn McpHost>);

impl McpHostHandle {
    /// Erases a concrete host adapter.
    #[must_use]
    pub fn new(host: impl McpHost) -> Self {
        Self(Arc::new(host))
    }

    pub(crate) fn inner(&self) -> &dyn McpHost {
        self.0.as_ref()
    }
}

impl std::fmt::Debug for McpHostHandle {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("McpHostHandle(..)")
    }
}

/// Safe headless host that advertises no interactive capabilities.
#[derive(Debug, Clone, Copy, Default)]
pub struct HeadlessHost;

#[async_trait]
impl McpHost for HeadlessHost {
    fn capabilities(&self, _context: &HostContext) -> HostCapabilities {
        HostCapabilities::default()
    }

    async fn authorize(
        &self,
        context: &HostContext,
        _request: PermissionRequest,
    ) -> Result<PermissionDecision> {
        Ok(PermissionDecision::Deny {
            reason: format!("headless host denied request from {}", context.server),
        })
    }

    async fn sample(
        &self,
        context: &HostContext,
        _request: SamplingRequest,
    ) -> Result<SamplingResponse> {
        Err(headless_error(context, "sampling"))
    }

    async fn elicit(
        &self,
        context: &HostContext,
        _request: ElicitationRequest,
    ) -> Result<ElicitationResponse> {
        Err(headless_error(context, "elicitation"))
    }

    async fn roots(&self, context: &HostContext) -> Result<Vec<Root>> {
        Err(headless_error(context, "roots"))
    }

    async fn log(&self, _context: &HostContext, _event: LogEvent) {}
}

/// Browser authorization request emitted by the OAuth coordinator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorizationPresentation {
    /// Server being authorized.
    pub server: ServerName,
    /// Authorization URL generated with PKCE and CSRF state.
    pub authorization_url: String,
    /// Redirect URI the host must capture.
    pub redirect_uri: String,
}

/// Browser callback captured by an [`AuthHost`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorizationCallback {
    /// Complete callback URL, including code, state, and optional issuer.
    pub redirect_url: String,
}

/// Host port for credentials, OAuth state, and browser interaction.
#[async_trait]
pub trait AuthHost: Send + Sync + 'static {
    /// Resolves a configured static secret reference.
    async fn resolve_secret(
        &self,
        server: &ServerName,
        secret_ref: &SecretRef,
    ) -> Result<SecretValue>;

    /// Loads an opaque token or PKCE record from secure storage.
    async fn load_record(&self, key: &SecretStoreKey) -> Result<Option<SecretBytes>>;

    /// Saves an opaque token or PKCE record to secure storage.
    async fn save_record(&self, key: &SecretStoreKey, value: SecretBytes) -> Result<()>;

    /// Removes an obsolete opaque record.
    async fn delete_record(&self, key: &SecretStoreKey) -> Result<()>;

    /// Opens or presents authorization and waits for its callback.
    async fn authorize_browser(
        &self,
        request: AuthorizationPresentation,
    ) -> Result<AuthorizationCallback>;
}

/// Cloneable, type-erased handle for an [`AuthHost`].
#[derive(Clone)]
pub struct AuthHostHandle(Arc<dyn AuthHost>);

impl AuthHostHandle {
    /// Erases a concrete authentication host adapter.
    #[must_use]
    pub fn new(host: impl AuthHost) -> Self {
        Self(Arc::new(host))
    }

    pub(crate) fn inner(&self) -> &dyn AuthHost {
        self.0.as_ref()
    }
}

impl std::fmt::Debug for AuthHostHandle {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("AuthHostHandle(..)")
    }
}

/// Authentication host that never reads ambient credentials.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoAuthHost;

#[async_trait]
impl AuthHost for NoAuthHost {
    async fn resolve_secret(
        &self,
        server: &ServerName,
        _secret_ref: &SecretRef,
    ) -> Result<SecretValue> {
        Err(auth_unavailable(server))
    }

    async fn load_record(&self, _key: &SecretStoreKey) -> Result<Option<SecretBytes>> {
        Ok(None)
    }

    async fn save_record(&self, key: &SecretStoreKey, _value: SecretBytes) -> Result<()> {
        Err(Error::new(
            ErrorKind::Authentication,
            Recovery::Fatal,
            format!("no secure credential store is available for {key}"),
        ))
    }

    async fn delete_record(&self, _key: &SecretStoreKey) -> Result<()> {
        Ok(())
    }

    async fn authorize_browser(
        &self,
        request: AuthorizationPresentation,
    ) -> Result<AuthorizationCallback> {
        Err(auth_unavailable(&request.server))
    }
}

fn headless_error(context: &HostContext, operation: &str) -> Error {
    Error::new(
        ErrorKind::Permission,
        Recovery::Fatal,
        format!("headless host cannot authorize {operation}"),
    )
    .with_server(context.server.clone())
}

fn auth_unavailable(server: &ServerName) -> Error {
    Error::new(
        ErrorKind::Authentication,
        Recovery::Fatal,
        "authentication requires an AuthHost credential/browser adapter",
    )
    .with_server(server.clone())
}

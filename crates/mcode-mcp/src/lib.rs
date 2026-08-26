//! Client-only MCP engine foundation for the first-party `mcode.mcp` plugin.
//!
//! This crate owns protocol negotiation, transports, OAuth, per-server
//! supervision, bounded catalogs, and protected server-to-client callbacks. It
//! does not implement an MCP server, plugin registry, TUI, provider, search
//! semantics, shell execution, or subagent runtime.
//!
//! Runtime settings are versioned JSON under
//! `plugins.mcode.mcp.servers.<name>`. One plugin may supervise many isolated
//! servers; no item is filtered by tool name or inferred semantics.

// Rust guideline compliant 2026-08-20.

mod auth;
pub mod catalog;
pub mod config;
pub mod error;
pub mod host;
pub mod http;
pub mod identity;
pub mod persistence;
pub mod process;
pub mod protocol;
mod rmcp_adapter;
pub mod secret;
pub mod supervisor;
pub mod validation;

#[doc(inline)]
pub use catalog::{
    CatalogParts, CatalogPrompt, CatalogResource, CatalogResourceTemplate, CatalogSection,
    CatalogSnapshot, CatalogTool, Generation,
};
#[doc(inline)]
pub use config::{
    AuthConfig, CONFIG_VERSION, EnvironmentConfig, McpPluginConfig, OAuth2Config,
    OAuthRegistration, OutputLimits, ReconnectConfig, SecretBinding, ServerConfig,
    StdioTransportConfig, StreamableHttpTransportConfig, TimeoutConfig, TransportConfig,
    TrustConfig, TrustLevel,
};
#[doc(inline)]
pub use error::{Error, ErrorKind, Recovery, Result};
#[doc(inline)]
pub use host::{
    AuthHost, AuthHostHandle, AuthorizationCallback, AuthorizationPresentation, ElicitationRequest,
    ElicitationResponse, HeadlessHost, HostCapabilities, HostContext, HostOperation, LogEvent,
    LogLevel, McpHost, McpHostHandle, NoAuthHost, PermissionDecision, PermissionRequest, Root,
    SamplingRequest, SamplingResponse,
};
#[doc(inline)]
pub use http::{
    DnsResolver, DnsResolverHandle, HttpSecurityPolicy, SecureHttpClient, SystemDnsResolver,
};
#[doc(inline)]
pub use identity::{ItemKind, NamespacedId, ServerName};
#[doc(inline)]
pub use persistence::{
    CallLedgerEntry, CallTerminalState, PERSISTED_MCP_STATE_VERSION, PersistedMcpState,
    PersistedServerState,
};
#[doc(inline)]
pub use process::{
    BoxAsyncRead, BoxAsyncWrite, ContainedProcess, NoProcessHost, ProcessExit, ProcessHost,
    ProcessHostHandle, ProcessSpec, SpawnedProcess,
};
#[doc(inline)]
pub use protocol::{
    Capability, ConnectContext, IgnoreProgress, MCP_LEGACY_PROTOCOL_VERSION, MCP_PROTOCOL_VERSION,
    McpConnector, McpConnectorHandle, McpSession, McpSessionHandle, NegotiatedCapabilities,
    NegotiatedServer, Page, ProgressUpdate, PromptGetOutcome, RMCP_SDK_VERSION, RemotePrompt,
    RemotePromptArgument, RemoteResource, RemoteResourceTemplate, RemoteTool, RequestCancellation,
    RequestControl, RequestObserver, RequestObserverHandle, ResourceReadOutcome, SessionEvent,
    SessionEventSink, ToolCallOutcome,
};
#[doc(inline)]
pub use rmcp_adapter::RmcpConnector;
#[doc(inline)]
pub use secret::{SecretBytes, SecretRef, SecretStoreKey, SecretValue};
#[doc(inline)]
pub use supervisor::{
    InitProgress, McpSupervisor, ServerErrorPayload, ServerEvent, ServerHandle, ServerPhase,
    ServerState, SupervisorCatalog,
};

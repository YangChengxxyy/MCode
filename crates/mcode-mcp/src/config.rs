//! Versioned JSON configuration for the first-party `mcode.mcp` plugin.

// Rust guideline compliant 2026-08-20.

use std::{collections::BTreeMap, path::PathBuf, time::Duration};

use http::HeaderName;
use serde::{Deserialize, Serialize};
use url::{Host, Url};

use crate::{
    error::{Error, ErrorKind, Recovery, Result},
    identity::ServerName,
    secret::SecretRef,
};

/// The only configuration version accepted by this crate.
pub const CONFIG_VERSION: u32 = 1;

/// Configuration rooted at `plugins.mcode.mcp`.
///
/// The surrounding settings loader should deserialize exactly this JSON object.
/// Cargo manifests are not runtime configuration sources.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpPluginConfig {
    /// Version of this persisted JSON shape.
    pub version: u32,
    /// Independently supervised MCP servers keyed by stable server name.
    pub servers: BTreeMap<ServerName, ServerConfig>,
}

impl McpPluginConfig {
    /// Validates the complete configuration before any server starts.
    ///
    /// # Errors
    ///
    /// Returns a configuration error without reading files, environment values,
    /// DNS, or credentials.
    pub fn validate(&self) -> Result<()> {
        if self.version != CONFIG_VERSION {
            return Err(config_error(format!(
                "unsupported mcode.mcp config version {}; expected {CONFIG_VERSION}",
                self.version
            )));
        }
        if self.servers.len() > 128 {
            return Err(config_error("at most 128 MCP servers may be configured"));
        }
        for (name, server) in &self.servers {
            server.validate(name)?;
        }
        Ok(())
    }
}

/// Configuration for one independently supervised MCP server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ServerConfig {
    /// Whether this server actor should start.
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// Direct stdio or Streamable HTTP transport settings.
    pub transport: TransportConfig,
    /// Deadlines for initialization, calls, pings, and shutdown.
    #[serde(default)]
    pub timeouts: TimeoutConfig,
    /// Hard bounds for remote schemas, content, logs, and wire frames.
    #[serde(default)]
    pub output_limits: OutputLimits,
    /// Bounded reconnect policy applied only to recoverable transport failures.
    #[serde(default)]
    pub reconnect: ReconnectConfig,
    /// Explicit trust grants; every dangerous grant defaults to false.
    #[serde(default)]
    pub trust: TrustConfig,
}

impl ServerConfig {
    fn validate(&self, name: &ServerName) -> Result<()> {
        self.timeouts.validate(name)?;
        self.output_limits.validate(name)?;
        self.reconnect.validate(name)?;
        self.trust.validate(name)?;
        match &self.transport {
            TransportConfig::Stdio(config) => config.validate(name),
            TransportConfig::StreamableHttp(config) => config.validate(name, &self.trust),
        }
    }
}

/// The supported client transports.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
#[non_exhaustive]
pub enum TransportConfig {
    /// Spawn one executable directly and exchange newline-delimited JSON-RPC.
    Stdio(StdioTransportConfig),
    /// Connect to a current Streamable HTTP endpoint.
    StreamableHttp(StreamableHttpTransportConfig),
}

/// Direct-spawn stdio configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StdioTransportConfig {
    /// Executable path or name passed directly to the process host.
    pub command: String,
    /// Argument vector; no shell parsing or interpolation occurs.
    #[serde(default)]
    pub args: Vec<String>,
    /// Optional child working directory.
    #[serde(default)]
    pub cwd: Option<PathBuf>,
    /// Explicit environment allowlist and secret bindings.
    #[serde(default)]
    pub env: EnvironmentConfig,
}

impl StdioTransportConfig {
    fn validate(&self, name: &ServerName) -> Result<()> {
        if self.command.is_empty()
            || self.command.len() > 4_096
            || self.command.chars().any(char::is_control)
        {
            return Err(server_config_error(
                name,
                "stdio command is empty or unsafe",
            ));
        }
        if self.args.len() > 256
            || self.args.iter().any(|arg| {
                arg.len() > 16_384
                    || arg.chars().any(|value| value == '\0')
                    || looks_like_secret_argument(arg)
            })
        {
            return Err(server_config_error(
                name,
                "stdio arguments exceed safe bounds or appear to contain credentials",
            ));
        }
        self.env.validate(name)
    }
}

/// Child environment policy.
///
/// The process host must clear the ambient environment, copy only `inherit`
/// names, and materialize `secrets` through its authorized secret store.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EnvironmentConfig {
    /// Names explicitly allowed to inherit from the parent process.
    #[serde(default)]
    pub inherit: Vec<String>,
    /// Environment names whose values come from opaque secret references.
    #[serde(default)]
    pub secrets: BTreeMap<String, SecretBinding>,
}

impl EnvironmentConfig {
    fn validate(&self, server: &ServerName) -> Result<()> {
        if self.inherit.len() > 128 || self.secrets.len() > 128 {
            return Err(server_config_error(
                server,
                "stdio environment allowlist exceeds 128 entries",
            ));
        }
        for name in self.inherit.iter().chain(self.secrets.keys()) {
            if !valid_environment_name(name) {
                return Err(server_config_error(
                    server,
                    "stdio environment variable name is invalid",
                ));
            }
        }
        let mut inherited = self.inherit.clone();
        inherited.sort_unstable();
        if inherited.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(server_config_error(
                server,
                "stdio environment allowlist contains a duplicate",
            ));
        }
        if self
            .inherit
            .iter()
            .any(|name| is_secret_environment_name(name))
        {
            return Err(server_config_error(
                server,
                "credential-like environment names must use a secretRef binding",
            ));
        }
        if self
            .secrets
            .keys()
            .any(|name| self.inherit.iter().any(|inherited| inherited == name))
        {
            return Err(server_config_error(
                server,
                "an environment name cannot be inherited and secret-bound",
            ));
        }
        Ok(())
    }
}

/// A JSON-safe pointer to secret material.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SecretBinding {
    /// Host secret-store key; never the secret value itself.
    pub secret_ref: SecretRef,
}

/// Streamable HTTP client configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StreamableHttpTransportConfig {
    /// MCP endpoint URL. HTTPS is required unless trusted local HTTP is explicit.
    pub url: String,
    /// Custom header values resolved through the host secret store.
    #[serde(default)]
    pub headers: BTreeMap<String, SecretBinding>,
    /// Static bearer or OAuth 2.1 authorization settings.
    #[serde(default)]
    pub auth: AuthConfig,
}

impl StreamableHttpTransportConfig {
    fn validate(&self, name: &ServerName, trust: &TrustConfig) -> Result<()> {
        let url = Url::parse(&self.url)
            .map_err(|_| server_config_error(name, "Streamable HTTP URL is invalid"))?;
        if !url.username().is_empty() || url.password().is_some() {
            return Err(server_config_error(
                name,
                "Streamable HTTP URL must not contain userinfo",
            ));
        }
        match url.scheme() {
            "https" => {}
            "http" if trust.level == TrustLevel::Trusted && trust.allow_http => {}
            _ => {
                return Err(server_config_error(
                    name,
                    "Streamable HTTP requires HTTPS unless trusted allowHttp is explicit",
                ));
            }
        }
        if url.host().is_none() {
            return Err(server_config_error(
                name,
                "Streamable HTTP URL needs a host",
            ));
        }
        if is_literal_loopback_or_localhost(&url)
            && !(trust.level == TrustLevel::Trusted && trust.allow_localhost)
        {
            return Err(server_config_error(
                name,
                "localhost requires trusted allowLocalhost",
            ));
        }
        if self.headers.len() > 64 {
            return Err(server_config_error(
                name,
                "at most 64 custom headers are allowed",
            ));
        }
        for header in self.headers.keys() {
            validate_secret_header(name, header)?;
        }
        if matches!(self.auth, AuthConfig::OAuth2(_))
            && !(trust.level == TrustLevel::Trusted && trust.allow_oauth)
        {
            return Err(server_config_error(
                name,
                "OAuth requires trusted allowOAuth and a protected host permission",
            ));
        }
        self.auth.validate(name)
    }
}

/// HTTP authentication mode.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
#[non_exhaustive]
pub enum AuthConfig {
    /// No HTTP authorization.
    #[default]
    None,
    /// Resolve one bearer token from the host secret store.
    StaticBearer {
        /// Host secret-store key for the token without a `Bearer` prefix.
        secret_ref: SecretRef,
    },
    /// OAuth 2.1 authorization-code flow with mandatory S256 PKCE.
    #[serde(rename = "oauth2")]
    OAuth2(OAuth2Config),
}

impl AuthConfig {
    fn validate(&self, server: &ServerName) -> Result<()> {
        if let Self::OAuth2(config) = self {
            config.validate(server)?;
        }
        Ok(())
    }
}

/// OAuth 2.1 and client-registration configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OAuth2Config {
    /// Redirect URI handled by the host browser/UI adapter.
    pub redirect_uri: String,
    /// Requested scopes; an empty list delegates selection to server metadata.
    #[serde(default)]
    pub scopes: Vec<String>,
    /// Registration material and fallback policy.
    pub registration: OAuthRegistration,
}

impl OAuth2Config {
    fn validate(&self, server: &ServerName) -> Result<()> {
        let redirect = Url::parse(&self.redirect_uri)
            .map_err(|_| server_config_error(server, "OAuth redirectUri is invalid"))?;
        let safe_redirect = redirect.scheme() == "https"
            || (redirect.scheme() == "http" && is_literal_loopback_or_localhost(&redirect));
        if !safe_redirect || redirect.host().is_none() {
            return Err(server_config_error(
                server,
                "OAuth redirectUri must be HTTPS or an HTTP loopback URI",
            ));
        }
        if self.scopes.len() > 64
            || self.scopes.iter().any(|scope| {
                scope.is_empty() || scope.len() > 256 || scope.chars().any(char::is_control)
            })
        {
            return Err(server_config_error(
                server,
                "OAuth scopes exceed safe bounds",
            ));
        }
        self.registration.validate(server)
    }
}

/// OAuth client registration material.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
#[non_exhaustive]
pub enum OAuthRegistration {
    /// Prefer CIMD when advertised, then fall back to dynamic registration.
    Auto {
        /// Human-readable name used for dynamic registration.
        client_name: String,
        /// Optional hosted Client ID Metadata Document URL.
        #[serde(default)]
        client_metadata_url: Option<String>,
    },
    /// Use a client ID issued out of band.
    PreRegistered {
        /// Public OAuth client identifier.
        client_id: String,
        /// Optional confidential-client secret reference.
        #[serde(default)]
        client_secret: Option<SecretBinding>,
    },
    /// Require a hosted Client ID Metadata Document (CIMD).
    ClientMetadata {
        /// HTTPS URL with a non-root path.
        url: String,
    },
    /// Require metadata-advertised Dynamic Client Registration.
    Dynamic {
        /// Human-readable name sent to the registration endpoint.
        client_name: String,
    },
}

impl OAuthRegistration {
    fn validate(&self, server: &ServerName) -> Result<()> {
        match self {
            Self::Auto {
                client_name,
                client_metadata_url,
            } => {
                validate_client_name(server, client_name)?;
                if let Some(url) = client_metadata_url {
                    validate_metadata_url(server, url)?;
                }
            }
            Self::PreRegistered { client_id, .. } => {
                if client_id.is_empty()
                    || client_id.len() > 2_048
                    || client_id.chars().any(char::is_control)
                {
                    return Err(server_config_error(server, "OAuth clientId is invalid"));
                }
            }
            Self::ClientMetadata { url } => validate_metadata_url(server, url)?,
            Self::Dynamic { client_name } => validate_client_name(server, client_name)?,
        }
        Ok(())
    }
}

/// Per-server deadlines in milliseconds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TimeoutConfig {
    /// Connect and protocol-negotiation deadline.
    pub connect_ms: u64,
    /// Idle request deadline, reset by matching progress notifications.
    pub request_ms: u64,
    /// Absolute request deadline even while progress arrives.
    pub request_total_ms: u64,
    /// Ping interval while connected.
    pub ping_interval_ms: u64,
    /// Ping response deadline.
    pub ping_ms: u64,
    /// Graceful shutdown deadline.
    pub shutdown_ms: u64,
}

impl TimeoutConfig {
    fn validate(&self, server: &ServerName) -> Result<()> {
        const MAX_TIMEOUT_MS: u64 = 24 * 60 * 60 * 1_000;
        let values = [
            self.connect_ms,
            self.request_ms,
            self.request_total_ms,
            self.ping_interval_ms,
            self.ping_ms,
            self.shutdown_ms,
        ];
        if values
            .iter()
            .any(|value| *value == 0 || *value > MAX_TIMEOUT_MS)
            || self.request_total_ms < self.request_ms
        {
            return Err(server_config_error(server, "timeout values are invalid"));
        }
        Ok(())
    }

    /// Returns the connect deadline.
    #[must_use]
    pub const fn connect(&self) -> Duration {
        Duration::from_millis(self.connect_ms)
    }

    /// Returns the idle request deadline.
    #[must_use]
    pub const fn request(&self) -> Duration {
        Duration::from_millis(self.request_ms)
    }

    /// Returns the absolute request deadline.
    #[must_use]
    pub const fn request_total(&self) -> Duration {
        Duration::from_millis(self.request_total_ms)
    }

    /// Returns the ping interval.
    #[must_use]
    pub const fn ping_interval(&self) -> Duration {
        Duration::from_millis(self.ping_interval_ms)
    }

    /// Returns the ping deadline.
    #[must_use]
    pub const fn ping(&self) -> Duration {
        Duration::from_millis(self.ping_ms)
    }

    /// Returns the shutdown deadline.
    #[must_use]
    pub const fn shutdown(&self) -> Duration {
        Duration::from_millis(self.shutdown_ms)
    }
}

impl Default for TimeoutConfig {
    fn default() -> Self {
        Self {
            connect_ms: 30_000,
            request_ms: 120_000,
            request_total_ms: 300_000,
            ping_interval_ms: 30_000,
            ping_ms: 10_000,
            shutdown_ms: 5_000,
        }
    }
}

/// Hard resource bounds for one server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OutputLimits {
    /// Maximum bytes in one stdio JSON-RPC line or HTTP JSON body.
    pub max_message_bytes: usize,
    /// Maximum raw bytes retained for one SSE event.
    pub max_sse_event_bytes: usize,
    /// Maximum combined HTTP response-header bytes.
    pub max_header_bytes: usize,
    /// Maximum redirects followed for one HTTP operation.
    pub max_redirects: usize,
    /// Maximum catalog pages per list operation.
    pub max_pages: usize,
    /// Maximum items in each catalog section.
    pub max_catalog_items: usize,
    /// Maximum recursively visited JSON nodes.
    pub max_json_nodes: usize,
    /// Maximum JSON nesting depth.
    pub max_json_depth: usize,
    /// Maximum UTF-8 bytes in one untrusted string.
    pub max_string_bytes: usize,
    /// Maximum content blocks in one result.
    pub max_content_blocks: usize,
    /// Maximum serialized bytes returned by one operation.
    pub max_output_bytes: usize,
    /// Maximum bytes retained from one log or progress message.
    pub max_log_bytes: usize,
}

impl OutputLimits {
    fn validate(&self, server: &ServerName) -> Result<()> {
        let valid = (1_024..=64 * 1024 * 1024).contains(&self.max_message_bytes)
            && (1_024..=16 * 1024 * 1024).contains(&self.max_sse_event_bytes)
            && (1_024..=1024 * 1024).contains(&self.max_header_bytes)
            && self.max_redirects <= 20
            && (1..=1_000).contains(&self.max_pages)
            && (1..=100_000).contains(&self.max_catalog_items)
            && (16..=1_000_000).contains(&self.max_json_nodes)
            && (4..=128).contains(&self.max_json_depth)
            && (64..=8 * 1024 * 1024).contains(&self.max_string_bytes)
            && (1..=4_096).contains(&self.max_content_blocks)
            && (1_024..=64 * 1024 * 1024).contains(&self.max_output_bytes)
            && (64..=1024 * 1024).contains(&self.max_log_bytes);
        if !valid {
            return Err(server_config_error(
                server,
                "output limits are outside hard bounds",
            ));
        }
        Ok(())
    }
}

impl Default for OutputLimits {
    fn default() -> Self {
        Self {
            max_message_bytes: 4 * 1024 * 1024,
            max_sse_event_bytes: 1024 * 1024,
            max_header_bytes: 64 * 1024,
            max_redirects: 5,
            max_pages: 100,
            max_catalog_items: 10_000,
            max_json_nodes: 20_000,
            max_json_depth: 64,
            max_string_bytes: 1024 * 1024,
            max_content_blocks: 256,
            max_output_bytes: 4 * 1024 * 1024,
            max_log_bytes: 16 * 1024,
        }
    }
}

/// Bounded exponential reconnect policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReconnectConfig {
    /// Whether recoverable transport failures may reconnect.
    pub enabled: bool,
    /// Maximum reconnect attempts per outage.
    pub max_attempts: u32,
    /// First backoff delay.
    pub initial_delay_ms: u64,
    /// Maximum backoff delay.
    pub max_delay_ms: u64,
}

impl ReconnectConfig {
    fn validate(&self, server: &ServerName) -> Result<()> {
        if self.max_attempts > 32
            || self.initial_delay_ms == 0
            || self.max_delay_ms < self.initial_delay_ms
            || self.max_delay_ms > 60 * 60 * 1_000
        {
            return Err(server_config_error(server, "reconnect policy is invalid"));
        }
        Ok(())
    }

    /// Returns the bounded delay for a zero-based retry attempt.
    #[must_use]
    pub fn delay(&self, attempt: u32) -> Duration {
        let multiplier = 1_u64.checked_shl(attempt.min(31)).unwrap_or(u64::MAX);
        Duration::from_millis(
            self.initial_delay_ms
                .saturating_mul(multiplier)
                .min(self.max_delay_ms),
        )
    }
}

impl Default for ReconnectConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_attempts: 5,
            initial_delay_ms: 250,
            max_delay_ms: 10_000,
        }
    }
}

/// Explicit trust grants for one server.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TrustConfig {
    /// Base trust level; untrusted is the default.
    #[serde(default)]
    pub level: TrustLevel,
    /// Permit plain HTTP for this trusted server.
    #[serde(default)]
    pub allow_http: bool,
    /// Permit loopback or `localhost` network targets.
    #[serde(default)]
    pub allow_localhost: bool,
    /// Permit RFC-private network targets; metadata addresses remain blocked.
    #[serde(default)]
    pub allow_private_network: bool,
    /// Permit server-initiated sampling after host permission.
    #[serde(default)]
    pub allow_sampling: bool,
    /// Permit server-initiated elicitation after host permission.
    #[serde(default)]
    pub allow_elicitation: bool,
    /// Permit sharing host roots after host permission.
    #[serde(default)]
    pub allow_roots: bool,
    /// Permit an interactive OAuth browser flow after host permission.
    #[serde(default, rename = "allowOAuth")]
    pub allow_oauth: bool,
}

impl TrustConfig {
    fn validate(&self, server: &ServerName) -> Result<()> {
        let grants = self.allow_http
            || self.allow_localhost
            || self.allow_private_network
            || self.allow_sampling
            || self.allow_elicitation
            || self.allow_roots
            || self.allow_oauth;
        if grants && self.level != TrustLevel::Trusted {
            return Err(server_config_error(
                server,
                "dangerous trust grants require level 'trusted'",
            ));
        }
        Ok(())
    }
}

/// Whether a server has received an explicit trust grant.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub enum TrustLevel {
    /// No dangerous server-initiated or local-network behavior is permitted.
    #[default]
    Untrusted,
    /// Explicit user trust; individual grants are still required.
    Trusted,
}

fn default_enabled() -> bool {
    true
}

fn valid_environment_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 256
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

fn is_secret_environment_name(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    [
        "TOKEN",
        "SECRET",
        "PASSWORD",
        "PASSWD",
        "API_KEY",
        "PRIVATE_KEY",
    ]
    .iter()
    .any(|marker| upper.contains(marker))
}

fn looks_like_secret_argument(argument: &str) -> bool {
    let lower = argument.to_ascii_lowercase();
    [
        "--token",
        "--password",
        "--secret",
        "--api-key",
        "authorization:",
        "bearer ",
        "access_token=",
        "refresh_token=",
        "api_key=",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
}

fn validate_secret_header(server: &ServerName, value: &str) -> Result<()> {
    let header = HeaderName::from_bytes(value.as_bytes())
        .map_err(|_| server_config_error(server, "custom header name is invalid"))?;
    let reserved = [
        "authorization",
        "content-length",
        "content-type",
        "host",
        "last-event-id",
        "mcp-session-id",
        "mcp-protocol-version",
        "transfer-encoding",
    ];
    if reserved
        .iter()
        .any(|name| header.as_str().eq_ignore_ascii_case(name))
        || header.as_str().to_ascii_lowercase().starts_with("mcp-")
    {
        return Err(server_config_error(
            server,
            "custom header conflicts with an MCP or HTTP transport header",
        ));
    }
    Ok(())
}

fn is_literal_loopback_or_localhost(url: &Url) -> bool {
    match url.host() {
        Some(Host::Domain(host)) => {
            let host = host.trim_end_matches('.');
            host.eq_ignore_ascii_case("localhost")
                || host.to_ascii_lowercase().ends_with(".localhost")
        }
        Some(Host::Ipv4(address)) => address.is_loopback(),
        Some(Host::Ipv6(address)) => address.is_loopback(),
        None => false,
    }
}

fn validate_client_name(server: &ServerName, value: &str) -> Result<()> {
    if value.is_empty() || value.len() > 256 || value.chars().any(char::is_control) {
        return Err(server_config_error(server, "OAuth clientName is invalid"));
    }
    Ok(())
}

fn validate_metadata_url(server: &ServerName, value: &str) -> Result<()> {
    let url = Url::parse(value)
        .map_err(|_| server_config_error(server, "OAuth client metadata URL is invalid"))?;
    if url.scheme() != "https" || url.host().is_none() || url.path() == "/" {
        return Err(server_config_error(
            server,
            "OAuth client metadata URL must be HTTPS with a non-root path",
        ));
    }
    Ok(())
}

fn config_error(message: impl AsRef<str>) -> Error {
    Error::new(ErrorKind::Configuration, Recovery::Fatal, message)
}

fn server_config_error(server: &ServerName, message: impl AsRef<str>) -> Error {
    config_error(message).with_server(server.clone())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn valid_json() -> serde_json::Value {
        json!({
            "version": 1,
            "servers": {
                "context7": {
                    "enabled": true,
                    "transport": {
                        "type": "streamableHttp",
                        "url": "https://mcp.example.test/mcp",
                        "headers": {
                            "x-api-key": {"secretRef": "keychain://context7"}
                        },
                        "auth": {"type": "none"}
                    }
                }
            }
        })
    }

    #[test]
    fn json_roundtrip_and_validation() {
        let config: McpPluginConfig = serde_json::from_value(valid_json()).unwrap();
        config.validate().unwrap();
        let roundtrip: McpPluginConfig =
            serde_json::from_value(serde_json::to_value(&config).unwrap()).unwrap();
        assert_eq!(roundtrip, config);
    }

    #[test]
    fn plaintext_header_shape_is_rejected() {
        let mut value = valid_json();
        value["servers"]["context7"]["transport"]["headers"]["x-api-key"] =
            json!("plaintext-token");
        assert!(serde_json::from_value::<McpPluginConfig>(value).is_err());
    }

    #[test]
    fn localhost_requires_explicit_trust() {
        let mut value = valid_json();
        value["servers"]["context7"]["transport"]["url"] = json!("http://127.0.0.1:8080/mcp");
        let config: McpPluginConfig = serde_json::from_value(value.clone()).unwrap();
        assert!(config.validate().is_err());

        value["servers"]["context7"]["trust"] = json!({
            "level": "trusted",
            "allowHttp": true,
            "allowLocalhost": true
        });
        let config: McpPluginConfig = serde_json::from_value(value).unwrap();
        config.validate().unwrap();
    }

    #[test]
    fn oauth_requires_explicit_trust_and_protected_grant() {
        let mut value = valid_json();
        value["servers"]["context7"]["transport"]["auth"] = json!({
            "type": "oauth2",
            "redirectUri": "http://localhost:8765/callback",
            "scopes": ["mcp:tools"],
            "registration": {
                "type": "auto",
                "clientName": "MCode",
                "clientMetadataUrl": "https://client.example.test/mcode.json"
            }
        });
        let config: McpPluginConfig = serde_json::from_value(value.clone()).unwrap();
        assert!(config.validate().is_err());
        assert!(
            !config.servers[&ServerName::new("context7").unwrap()]
                .trust
                .allow_oauth
        );

        value["servers"]["context7"]["trust"] = json!({
            "level": "trusted",
            "allowOAuth": true
        });
        let config: McpPluginConfig = serde_json::from_value(value).unwrap();
        config.validate().unwrap();
        assert!(
            config.servers[&ServerName::new("context7").unwrap()]
                .trust
                .allow_oauth
        );
    }

    #[test]
    fn stdio_env_is_allowlist_only() {
        let value = json!({
            "version": 1,
            "servers": {
                "github": {
                    "transport": {
                        "type": "stdio",
                        "command": "github-mcp",
                        "args": ["stdio"],
                        "env": {
                            "inherit": ["PATH"],
                            "secrets": {
                                "GITHUB_TOKEN": {"secretRef": "keychain://github"}
                            }
                        }
                    }
                }
            }
        });
        let config: McpPluginConfig = serde_json::from_value(value).unwrap();
        config.validate().unwrap();
    }

    #[test]
    fn stdio_credentials_must_use_secret_refs() {
        let value = json!({
            "version": 1,
            "servers": {
                "unsafe": {
                    "transport": {
                        "type": "stdio",
                        "command": "unsafe-mcp",
                        "args": ["--token=plaintext"],
                        "env": {"inherit": ["GITHUB_TOKEN"]}
                    }
                }
            }
        });
        let config: McpPluginConfig = serde_json::from_value(value).unwrap();
        assert!(config.validate().is_err());
    }
}

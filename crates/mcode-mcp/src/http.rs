//! SSRF-hardened HTTP execution and Streamable HTTP transport backend.

// Rust guideline compliant 2026-08-26.

use std::{
    collections::HashMap,
    fmt,
    net::{IpAddr, Ipv6Addr, SocketAddr},
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    task::{Context, Poll, ready},
    time::Duration,
};

use async_trait::async_trait;
use bytes::Bytes;
use futures::{Stream, StreamExt, stream::BoxStream};
use http::{
    HeaderMap, HeaderName, HeaderValue, Method, StatusCode,
    header::{ACCEPT, CONTENT_ENCODING, CONTENT_TYPE, LOCATION, WWW_AUTHENTICATE},
};
use reqwest_mcp as reqwest;
use rmcp::{
    model::{ClientJsonRpcMessage, JsonRpcMessage, ServerJsonRpcMessage},
    transport::{
        common::{client_side_sse::SseStreamRetryHooks, http_header::HEADER_SESSION_ID},
        streamable_http_client::{
            AuthRequiredError, InsufficientScopeError, SseError, StreamableHttpClient,
            StreamableHttpError, StreamableHttpPostResponse,
        },
    },
};
use sse_stream::{Sse, SseStream};
use url::{Host, Url};

use crate::{
    config::{OutputLimits, ReconnectConfig, TimeoutConfig, TrustConfig, TrustLevel},
    error::{Error, ErrorKind, Recovery, Result},
    identity::ServerName,
    secret::SecretValue,
};

const JSON_MIME_TYPE: &str = "application/json";
const SSE_MIME_TYPE: &str = "text/event-stream";
const METADATA_HOSTS: &[&str] = &[
    "metadata",
    "metadata.google.internal",
    "metadata.azure.internal",
    "instance-data",
];

/// DNS resolver injected into the secure HTTP client.
#[async_trait]
pub trait DnsResolver: Send + Sync + 'static {
    /// Resolves every address for `host` and `port`.
    async fn resolve(&self, host: &str, port: u16) -> Result<Vec<SocketAddr>>;
}

/// Cloneable, type-erased DNS resolver.
#[derive(Clone)]
pub struct DnsResolverHandle(Arc<dyn DnsResolver>);

impl DnsResolverHandle {
    /// Erases a concrete resolver.
    #[must_use]
    pub fn new(resolver: impl DnsResolver) -> Self {
        Self(Arc::new(resolver))
    }

    async fn resolve(&self, host: &str, port: u16) -> Result<Vec<SocketAddr>> {
        self.0.resolve(host, port).await
    }
}

impl fmt::Debug for DnsResolverHandle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("DnsResolverHandle(..)")
    }
}

/// Tokio resolver used by production HTTP adapters.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemDnsResolver;

#[async_trait]
impl DnsResolver for SystemDnsResolver {
    async fn resolve(&self, host: &str, port: u16) -> Result<Vec<SocketAddr>> {
        tokio::net::lookup_host((host, port))
            .await
            .map(|addresses| addresses.collect())
            .map_err(|_| {
                Error::new(
                    ErrorKind::Transport,
                    Recovery::Recoverable,
                    "DNS resolution failed",
                )
            })
    }
}

/// Per-server HTTP security policy.
#[derive(Debug, Clone)]
pub struct HttpSecurityPolicy {
    server: ServerName,
    trust: TrustConfig,
    limits: OutputLimits,
    timeouts: TimeoutConfig,
}

impl HttpSecurityPolicy {
    /// Creates a policy from validated server configuration.
    #[must_use]
    pub fn new(
        server: ServerName,
        trust: TrustConfig,
        limits: OutputLimits,
        timeouts: TimeoutConfig,
    ) -> Self {
        Self {
            server,
            trust,
            limits,
            timeouts,
        }
    }

    /// Returns the protected server identity.
    #[must_use]
    pub fn server(&self) -> &ServerName {
        &self.server
    }

    /// Validates one hop against scheme, hostname, and resolved-address policy.
    ///
    /// # Errors
    ///
    /// Returns a trust error for HTTP downgrade, metadata, local, private,
    /// link-local, multicast, or otherwise non-routable targets.
    pub fn validate_hop(&self, url: &Url, addresses: &[SocketAddr]) -> Result<()> {
        if !url.username().is_empty() || url.password().is_some() || url.host().is_none() {
            return Err(self.trust_error("HTTP URL contains forbidden userinfo or no host"));
        }
        match url.scheme() {
            "https" => {}
            "http" if self.trust.level == TrustLevel::Trusted && self.trust.allow_http => {}
            _ => return Err(self.trust_error("HTTP target requires HTTPS")),
        }
        let host = url
            .host_str()
            .unwrap_or_default()
            .trim_end_matches('.')
            .to_ascii_lowercase();
        if METADATA_HOSTS.contains(&host.as_str()) {
            return Err(self.trust_error("cloud metadata hostname is forbidden"));
        }
        if addresses.is_empty() {
            return Err(self.transport_error("DNS returned no addresses"));
        }
        for address in addresses {
            self.validate_ip(address.ip())?;
        }
        Ok(())
    }

    fn validate_ip(&self, address: IpAddr) -> Result<()> {
        if is_metadata_address(address) {
            return Err(self.trust_error("cloud metadata address is forbidden"));
        }
        if is_loopback(address) {
            if self.trust.level == TrustLevel::Trusted && self.trust.allow_localhost {
                return Ok(());
            }
            return Err(self.trust_error("loopback address requires allowLocalhost"));
        }
        if is_private(address) {
            if self.trust.level == TrustLevel::Trusted && self.trust.allow_private_network {
                return Ok(());
            }
            return Err(self.trust_error("private address requires allowPrivateNetwork"));
        }
        if is_link_local(address) {
            return Err(self.trust_error("link-local addresses are forbidden"));
        }
        if is_non_routable(address) {
            return Err(self.trust_error("non-routable address is forbidden"));
        }
        Ok(())
    }

    fn trust_error(&self, message: impl AsRef<str>) -> Error {
        Error::new(ErrorKind::Trust, Recovery::Fatal, message).with_server(self.server.clone())
    }

    fn transport_error(&self, message: impl AsRef<str>) -> Error {
        Error::new(ErrorKind::Transport, Recovery::Recoverable, message)
            .with_server(self.server.clone())
    }
}

/// Errors emitted by [`SecureHttpClient`].
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum SecureHttpError {
    /// Policy rejected a target or redirect.
    #[error("HTTP security policy rejected the request")]
    Policy(#[source] Error),
    /// DNS resolution failed or timed out.
    #[error("HTTP DNS resolution failed")]
    Dns(#[source] Error),
    /// TLS, connection, or HTTP execution failed.
    #[error("HTTP request failed")]
    Request(#[source] reqwest::Error),
    /// Response headers exceeded their cap.
    #[error("HTTP response headers exceeded their cap")]
    HeadersTooLarge,
    /// Response body exceeded its cap.
    #[error("HTTP response body exceeded its cap")]
    BodyTooLarge,
    /// Redirect response was malformed or exceeded its cap.
    #[error("HTTP redirect was rejected")]
    Redirect,
    /// Compressed responses are disabled to prevent decompression bombs.
    #[error("compressed HTTP response was rejected")]
    CompressedResponse,
    /// Request construction failed.
    #[error("HTTP request could not be constructed")]
    InvalidRequest,
    /// Dynamic authorization failed without exposing token details.
    #[error("HTTP authorization failed")]
    Authentication(#[source] Error),
    /// SSE reconnects are disabled or the configured attempt budget is zero.
    #[error("SSE reconnect budget is unavailable")]
    SseReconnectUnavailable,
}

impl From<reqwest::Error> for SecureHttpError {
    fn from(value: reqwest::Error) -> Self {
        Self::Request(value)
    }
}

/// Redirect behavior for a secure HTTP operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RedirectMode {
    Follow,
    Stop,
}

/// Raw bounded HTTP response used by MCP and OAuth adapters.
pub(crate) struct SecureResponse {
    pub status: StatusCode,
    pub headers: HeaderMap,
    response: reqwest::Response,
}

#[async_trait]
pub(crate) trait BearerTokenProvider: Send + Sync + 'static {
    async fn token(&self) -> std::result::Result<SecretValue, SecureHttpError>;

    async fn upgrade_scope(
        &self,
        required_scope: &str,
    ) -> std::result::Result<bool, SecureHttpError>;
}

#[derive(Clone)]
enum BearerSource {
    Static(SecretValue),
    Dynamic(Arc<dyn BearerTokenProvider>),
}

fn reconnect_delay(config: &ReconnectConfig, current_times: usize) -> Option<Duration> {
    if !config.enabled {
        return None;
    }
    let attempt = u32::try_from(current_times).ok()?;
    (attempt < config.max_attempts).then(|| config.delay(attempt))
}

/// Per-stream/outage retry state passed along the rmcp reconnect call chain.
///
/// Each `SseAutoReconnectStream` owns a distinct token. Policy waits, GET extra
/// delays, and Drop/cancel/abort only mutate this token.
#[derive(Debug, Clone)]
pub(crate) struct StreamRetryToken {
    config: ReconnectConfig,
    pending_policy_wait: Arc<AtomicBool>,
    live: Arc<AtomicBool>,
}

struct PendingPolicyWait {
    token: StreamRetryToken,
    armed: bool,
}

impl PendingPolicyWait {
    fn new(token: StreamRetryToken) -> Self {
        Self { token, armed: true }
    }

    fn disarm(mut self) {
        self.armed = false;
    }
}

impl Drop for PendingPolicyWait {
    fn drop(&mut self) {
        if self.armed {
            self.token.clear_pending();
        }
    }
}

impl StreamRetryToken {
    fn new(config: ReconnectConfig) -> Self {
        Self {
            config,
            pending_policy_wait: Arc::new(AtomicBool::new(false)),
            live: Arc::new(AtomicBool::new(false)),
        }
    }

    fn clear_pending(&self) {
        self.pending_policy_wait.store(false, Ordering::SeqCst);
    }

    /// Records that this reconnect stream is currently connected.
    pub(crate) fn note_live(&self) {
        self.live.store(true, Ordering::SeqCst);
    }

    /// Starts one policy wait for this stream/outage only.
    ///
    /// Sets `pending_policy_wait` immediately. Callers that return a wait
    /// future must create [`PendingPolicyWait`] before boxing so an unpolled
    /// drop still clears the flag.
    #[must_use]
    pub(crate) fn begin_policy_retry(&self, current_times: usize) -> Option<Duration> {
        let delay = reconnect_delay(&self.config, current_times)?;
        self.pending_policy_wait.store(true, Ordering::SeqCst);
        self.live.store(false, Ordering::SeqCst);
        Some(delay)
    }

    /// Extra delay before this stream's next reconnect GET.
    #[must_use]
    pub(crate) fn extra_get_delay(&self) -> Option<Duration> {
        let Some(initial) = reconnect_delay(&self.config, 0) else {
            self.clear_pending();
            self.live.store(false, Ordering::SeqCst);
            return None;
        };
        if self.pending_policy_wait.swap(false, Ordering::SeqCst) {
            self.live.store(false, Ordering::SeqCst);
            None
        } else if self.live.swap(false, Ordering::SeqCst) {
            Some(initial)
        } else {
            None
        }
    }

    /// RAII policy wait. Cancel clears pending on this token only.
    ///
    /// The guard is owned by the returned future from construction, not from
    /// first poll, so `drop(wait)` without polling still clears pending.
    #[must_use]
    pub(crate) fn policy_retry_wait(
        &self,
        current_times: usize,
    ) -> Option<futures::future::BoxFuture<'static, ()>> {
        let delay = self.begin_policy_retry(current_times)?;
        let guard = PendingPolicyWait::new(self.clone());
        Some(Box::pin(async move {
            tokio::time::sleep(delay).await;
            guard.disarm();
        }))
    }
}

impl SseStreamRetryHooks for StreamRetryToken {
    fn note_live(&self) {
        StreamRetryToken::note_live(self);
    }

    fn policy_retry(&self, current_times: usize) -> Option<Duration> {
        self.begin_policy_retry(current_times)
    }

    fn extra_get_delay(&self) -> Option<Duration> {
        StreamRetryToken::extra_get_delay(self)
    }

    fn policy_retry_wait(
        &self,
        current_times: usize,
    ) -> Option<futures::future::BoxFuture<'static, ()>> {
        StreamRetryToken::policy_retry_wait(self, current_times)
    }
}

#[derive(Debug)]
struct SseReconnectState {
    config: ReconnectConfig,
    common_stream_started: AtomicBool,
}

/// Policy gate for Streamable HTTP SSE reconnects.
///
/// Attempt budgets live in rmcp's per-stream `current_times`. Each actual SSE
/// stream/outage owns a [`StreamRetryToken`] created by [`Self::stream_token`]
/// and bound through vendored rmcp, so concurrent common and request streams
/// cannot reset, steal, or stack each other's waits. A stream therefore waits
/// `delay(n)` and cannot exceed `max_delay_ms` when another stream reconnects.
///
/// Policy-wait futures, failed reconnect GETs, stream drop, success, and abort
/// all drop or disarm that token via RAII. They cannot leave pending flags for a
/// later stream. This gate itself only rejects reconnect GETs when disabled or
/// when `max_attempts` is zero; extra delay for the SDK-skipped first GET after a
/// mid-stream error lives on the token. Server `retry` fields are stripped so
/// they cannot bypass the configured backoff clamp.
#[derive(Debug, Clone)]
pub(crate) struct SseReconnectGate(Arc<SseReconnectState>);

impl SseReconnectGate {
    /// Creates a session-level SSE reconnect policy gate.
    #[must_use]
    pub(crate) fn new(config: ReconnectConfig) -> Self {
        Self(Arc::new(SseReconnectState {
            config,
            common_stream_started: AtomicBool::new(false),
        }))
    }

    /// Allocates retry state for one SSE stream/outage.
    #[must_use]
    pub(crate) fn stream_token(&self) -> StreamRetryToken {
        StreamRetryToken::new(self.0.config.clone())
    }

    /// Returns the configured delay for one rmcp stream/outage attempt.
    ///
    /// `current_times` is the SDK's per-stream counter. Concurrent streams pass
    /// independent values, so one outage cannot exhaust or reset another.
    #[must_use]
    pub(crate) fn retry_delay(&self, current_times: usize) -> Option<Duration> {
        reconnect_delay(&self.0.config, current_times)
    }

    /// Returns whether this Streamable HTTP GET may proceed, without extra delay.
    ///
    /// Extra delay is applied by the stream-local token in rmcp's reconnect GET,
    /// not by guessing a session or task owner here.
    ///
    /// # Errors
    ///
    /// Returns [`SecureHttpError::SseReconnectUnavailable`] when reconnects are
    /// disabled or `max_attempts` is zero.
    pub(crate) fn reconnect_request_delay(
        &self,
        has_session_id: bool,
        has_last_event_id: bool,
    ) -> std::result::Result<Option<Duration>, SecureHttpError> {
        let is_reconnect = has_last_event_id
            || !has_session_id
            || self.0.common_stream_started.swap(true, Ordering::SeqCst);
        if !is_reconnect {
            return Ok(None);
        }
        if reconnect_delay(&self.0.config, 0).is_none() {
            return Err(SecureHttpError::SseReconnectUnavailable);
        }
        Ok(None)
    }

    async fn before_stream_request(
        &self,
        has_session_id: bool,
        has_last_event_id: bool,
    ) -> std::result::Result<(), SecureHttpError> {
        if let Some(delay) = self.reconnect_request_delay(has_session_id, has_last_event_id)? {
            tokio::time::sleep(delay).await;
        }
        Ok(())
    }

    fn constrain_event(&self, mut event: Sse) -> Sse {
        // rmcp 3.1.4 otherwise lets the server-provided delay bypass the client policy.
        event.retry = None;
        event
    }

    fn constrain_stream(
        &self,
        stream: BoxStream<'static, std::result::Result<Sse, SseError>>,
    ) -> BoxStream<'static, std::result::Result<Sse, SseError>> {
        let gate = self.clone();
        stream
            .map(move |item| item.map(|event| gate.constrain_event(event)))
            .boxed()
    }
}

/// Streamable HTTP backend with per-hop SSRF and DNS-rebinding protection.
#[derive(Clone)]
pub struct SecureHttpClient {
    resolver: DnsResolverHandle,
    policy: HttpSecurityPolicy,
    bearer: Option<BearerSource>,
    secret_headers: Arc<Vec<(HeaderName, SecretValue)>>,
    sse_reconnect: Option<SseReconnectGate>,
}

impl fmt::Debug for SecureHttpClient {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SecureHttpClient")
            .field("server", self.policy.server())
            .field("bearer", &self.bearer.as_ref().map(|_| "[REDACTED]"))
            .field(
                "secret_header_names",
                &self
                    .secret_headers
                    .iter()
                    .map(|(name, _)| name)
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}

impl SecureHttpClient {
    /// Creates an unauthenticated secure backend.
    #[must_use]
    pub fn new(policy: HttpSecurityPolicy, resolver: DnsResolverHandle) -> Self {
        Self {
            resolver,
            policy,
            bearer: None,
            secret_headers: Arc::new(Vec::new()),
            sse_reconnect: None,
        }
    }

    pub(crate) fn with_sse_reconnect(mut self, gate: SseReconnectGate) -> Self {
        self.sse_reconnect = Some(gate);
        self
    }

    pub(crate) fn with_bearer(mut self, bearer: SecretValue) -> Self {
        self.bearer = Some(BearerSource::Static(bearer));
        self
    }

    pub(crate) fn with_bearer_provider(mut self, provider: impl BearerTokenProvider) -> Self {
        self.bearer = Some(BearerSource::Dynamic(Arc::new(provider)));
        self
    }

    pub(crate) fn with_secret_headers(mut self, headers: Vec<(HeaderName, SecretValue)>) -> Self {
        self.secret_headers = Arc::new(headers);
        self
    }

    fn constrain_sse_stream(
        &self,
        stream: BoxStream<'static, std::result::Result<Sse, SseError>>,
    ) -> BoxStream<'static, std::result::Result<Sse, SseError>> {
        match &self.sse_reconnect {
            Some(gate) => gate.constrain_stream(stream),
            None => stream,
        }
    }

    pub(crate) async fn execute(
        &self,
        method: Method,
        url: Url,
        headers: HeaderMap,
        body: Vec<u8>,
        redirect_mode: RedirectMode,
        timeout: Option<Duration>,
    ) -> std::result::Result<SecureResponse, SecureHttpError> {
        if body.len() > self.policy.limits.max_message_bytes {
            return Err(SecureHttpError::BodyTooLarge);
        }
        let original_origin = origin(&url);
        let mut current = url;
        let mut method = method;
        let mut body = body;
        let mut retain_sensitive_headers = true;
        let mut scope_upgraded = false;
        let mut redirect_count = 0usize;

        loop {
            let addresses = self.resolve_and_validate(&current).await?;
            let client = self.build_pinned_client(&current, &addresses, timeout)?;
            let mut request = client.request(method.clone(), current.as_str());
            for (name, value) in &headers {
                if retain_sensitive_headers || is_redirect_safe_header(name) {
                    request = request.header(name, value);
                }
            }
            if retain_sensitive_headers {
                if let Some(bearer) = self.bearer_token().await? {
                    request = request.bearer_auth(bearer.expose());
                }
                for (name, value) in self.secret_headers.iter() {
                    let value = HeaderValue::from_str(value.expose())
                        .map_err(|_| SecureHttpError::InvalidRequest)?;
                    request = request.header(name, value);
                }
            }
            if !body.is_empty() {
                request = request.body(body.clone());
            }
            let response = request.send().await?;
            validate_response_headers(&response, self.policy.limits.max_header_bytes)?;
            if response
                .headers()
                .get(CONTENT_ENCODING)
                .is_some_and(|value| !value.as_bytes().eq_ignore_ascii_case(b"identity"))
            {
                return Err(SecureHttpError::CompressedResponse);
            }
            let status = response.status();
            if status == StatusCode::FORBIDDEN
                && retain_sensitive_headers
                && !scope_upgraded
                && let Some(challenge) = response
                    .headers()
                    .get(WWW_AUTHENTICATE)
                    .and_then(|value| value.to_str().ok())
                && let Some(scope) = extract_scope(challenge)
                && self.upgrade_scope(&scope).await?
            {
                scope_upgraded = true;
                if is_replay_safe_method(&method) {
                    continue;
                }
            }
            if !status.is_redirection() || redirect_mode == RedirectMode::Stop {
                return Ok(SecureResponse {
                    status,
                    headers: response.headers().clone(),
                    response,
                });
            }
            if redirect_count >= self.policy.limits.max_redirects {
                return Err(SecureHttpError::Redirect);
            }
            redirect_count = redirect_count.saturating_add(1);
            let location = response
                .headers()
                .get(LOCATION)
                .and_then(|value| value.to_str().ok())
                .ok_or(SecureHttpError::Redirect)?;
            let next = current
                .join(location)
                .map_err(|_| SecureHttpError::Redirect)?;
            let same_origin = origin(&next) == original_origin;
            let redirects_as_get = matches!(status, StatusCode::SEE_OTHER)
                || (matches!(status, StatusCode::MOVED_PERMANENTLY | StatusCode::FOUND)
                    && method != Method::GET
                    && method != Method::HEAD);
            if redirects_as_get {
                method = Method::GET;
                body.clear();
            } else if !same_origin && (!body.is_empty() || !is_replay_safe_method(&method)) {
                return Err(SecureHttpError::Redirect);
            }
            retain_sensitive_headers = same_origin;
            current = next;
        }
    }

    async fn bearer_token(&self) -> std::result::Result<Option<SecretValue>, SecureHttpError> {
        match &self.bearer {
            Some(BearerSource::Static(value)) => Ok(Some(value.clone())),
            Some(BearerSource::Dynamic(provider)) => provider.token().await.map(Some),
            None => Ok(None),
        }
    }

    async fn upgrade_scope(
        &self,
        required_scope: &str,
    ) -> std::result::Result<bool, SecureHttpError> {
        match &self.bearer {
            Some(BearerSource::Dynamic(provider)) => provider.upgrade_scope(required_scope).await,
            Some(BearerSource::Static(_)) | None => Ok(false),
        }
    }

    pub(crate) async fn read_body(
        &self,
        response: SecureResponse,
        cap: usize,
    ) -> std::result::Result<(StatusCode, HeaderMap, Vec<u8>), SecureHttpError> {
        if response
            .response
            .content_length()
            .is_some_and(|length| length > cap as u64)
        {
            return Err(SecureHttpError::BodyTooLarge);
        }
        let status = response.status;
        let headers = response.headers;
        let mut output = Vec::new();
        let mut stream = response.response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            if chunk.len() > cap.saturating_sub(output.len()) {
                return Err(SecureHttpError::BodyTooLarge);
            }
            output.extend_from_slice(&chunk);
        }
        Ok((status, headers, output))
    }

    async fn resolve_and_validate(
        &self,
        url: &Url,
    ) -> std::result::Result<Vec<SocketAddr>, SecureHttpError> {
        let port = url
            .port_or_known_default()
            .ok_or(SecureHttpError::InvalidRequest)?;
        let addresses = match url.host() {
            Some(Host::Ipv4(address)) => vec![SocketAddr::new(IpAddr::V4(address), port)],
            Some(Host::Ipv6(address)) => vec![SocketAddr::new(IpAddr::V6(address), port)],
            Some(Host::Domain(host)) => tokio::time::timeout(
                self.policy.timeouts.connect(),
                self.resolver.resolve(host, port),
            )
            .await
            .map_err(|_| {
                SecureHttpError::Dns(
                    Error::new(
                        ErrorKind::Timeout,
                        Recovery::Recoverable,
                        "DNS resolution timed out",
                    )
                    .with_server(self.policy.server.clone()),
                )
            })?
            .map_err(SecureHttpError::Dns)?,
            None => return Err(SecureHttpError::InvalidRequest),
        };
        self.policy
            .validate_hop(url, &addresses)
            .map_err(SecureHttpError::Policy)?;
        Ok(addresses)
    }

    fn build_pinned_client(
        &self,
        url: &Url,
        addresses: &[SocketAddr],
        timeout: Option<Duration>,
    ) -> std::result::Result<reqwest::Client, SecureHttpError> {
        let mut builder = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(self.policy.timeouts.connect())
            .timeout(timeout.unwrap_or_else(|| self.policy.timeouts.request_total()))
            .min_tls_version(reqwest::tls::Version::TLS_1_2)
            .pool_max_idle_per_host(0)
            .no_proxy();
        if let Some(Host::Domain(host)) = url.host() {
            builder = builder.resolve_to_addrs(host, addresses);
        }
        builder.build().map_err(SecureHttpError::Request)
    }

    fn request_headers(
        &self,
        custom: HashMap<HeaderName, HeaderValue>,
        accept: &'static str,
    ) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(ACCEPT, HeaderValue::from_static(accept));
        headers.insert(
            http::header::ACCEPT_ENCODING,
            HeaderValue::from_static("identity"),
        );
        for (name, value) in custom {
            headers.insert(name, value);
        }
        headers
    }

    async fn classify_auth_response<E>(response: &SecureResponse) -> Option<StreamableHttpError<E>>
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        let challenge = response
            .headers
            .get(WWW_AUTHENTICATE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned)?;
        match response.status {
            StatusCode::UNAUTHORIZED => Some(StreamableHttpError::AuthRequired(
                AuthRequiredError::new(challenge),
            )),
            StatusCode::FORBIDDEN => {
                let required_scope = extract_scope(&challenge);
                Some(StreamableHttpError::InsufficientScope(
                    InsufficientScopeError::new(challenge, required_scope),
                ))
            }
            _ => None,
        }
    }
}

impl StreamableHttpClient for SecureHttpClient {
    type Error = SecureHttpError;

    async fn post_message(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session_id: Option<Arc<str>>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> std::result::Result<StreamableHttpPostResponse, StreamableHttpError<Self::Error>> {
        self.post_message_with_max_sse_event_size(
            uri,
            message,
            session_id,
            auth_header,
            custom_headers,
            self.policy.limits.max_sse_event_bytes,
        )
        .await
    }

    async fn post_message_with_max_sse_event_size(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session_id: Option<Arc<str>>,
        _auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
        max_sse_event_size: usize,
    ) -> std::result::Result<StreamableHttpPostResponse, StreamableHttpError<Self::Error>> {
        let url = Url::parse(uri.as_ref())
            .map_err(|_| StreamableHttpError::Client(SecureHttpError::InvalidRequest))?;
        let mut headers =
            self.request_headers(custom_headers, "application/json, text/event-stream");
        headers.insert(CONTENT_TYPE, HeaderValue::from_static(JSON_MIME_TYPE));
        let session_attached = session_id.is_some();
        if let Some(session_id) = session_id {
            let value = HeaderValue::from_str(session_id.as_ref())
                .map_err(|_| StreamableHttpError::Client(SecureHttpError::InvalidRequest))?;
            headers.insert(HeaderName::from_static("mcp-session-id"), value);
        }
        let body = serde_json::to_vec(&message)?;
        if body.len() > self.policy.limits.max_message_bytes {
            return Err(StreamableHttpError::Client(SecureHttpError::BodyTooLarge));
        }
        let response = self
            .execute(Method::POST, url, headers, body, RedirectMode::Follow, None)
            .await
            .map_err(StreamableHttpError::Client)?;
        if let Some(error) = Self::classify_auth_response(&response).await {
            return Err(error);
        }
        if matches!(
            response.status,
            StatusCode::ACCEPTED | StatusCode::NO_CONTENT
        ) {
            return Ok(StreamableHttpPostResponse::Accepted);
        }
        if response.status == StatusCode::NOT_FOUND && session_attached {
            return Err(StreamableHttpError::SessionExpired);
        }
        let content_type = content_type(&response.headers);
        let response_session = response
            .headers
            .get(HEADER_SESSION_ID)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        if response.status.is_success()
            && response.response.content_length() == Some(0)
            && !matches!(message, ClientJsonRpcMessage::Request(_))
        {
            return Ok(StreamableHttpPostResponse::Accepted);
        }
        if content_type.as_deref().is_some_and(is_sse) {
            if !response.status.is_success() {
                return Err(StreamableHttpError::UnexpectedServerResponse(
                    "non-success SSE response".into(),
                ));
            }
            let stream = bounded_sse_stream(response.response.bytes_stream(), max_sse_event_size);
            return Ok(StreamableHttpPostResponse::Sse(
                self.constrain_sse_stream(stream),
                response_session,
            ));
        }
        let (status, _, body) = self
            .read_body(response, self.policy.limits.max_message_bytes)
            .await
            .map_err(StreamableHttpError::Client)?;
        if content_type.as_deref().is_none_or(|value| !is_json(value)) {
            return Err(StreamableHttpError::UnexpectedContentType(content_type));
        }
        let parsed = serde_json::from_slice::<ServerJsonRpcMessage>(&body);
        if !status.is_success() {
            return match parsed {
                Ok(message @ JsonRpcMessage::Error(_)) => {
                    Ok(StreamableHttpPostResponse::Json(message, response_session))
                }
                _ => Err(StreamableHttpError::UnexpectedServerResponse(
                    "non-success HTTP response".into(),
                )),
            };
        }
        match parsed {
            Ok(message) => Ok(StreamableHttpPostResponse::Json(message, response_session)),
            Err(error)
                if !matches!(message, ClientJsonRpcMessage::Request(_)) && body.is_empty() =>
            {
                let _ = error;
                Ok(StreamableHttpPostResponse::Accepted)
            }
            Err(error) => Err(StreamableHttpError::Deserialize(error)),
        }
    }

    async fn delete_session(
        &self,
        uri: Arc<str>,
        session_id: Arc<str>,
        _auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> std::result::Result<(), StreamableHttpError<Self::Error>> {
        let url = Url::parse(uri.as_ref())
            .map_err(|_| StreamableHttpError::Client(SecureHttpError::InvalidRequest))?;
        let mut headers = self.request_headers(custom_headers, "application/json");
        headers.insert(
            HeaderName::from_static("mcp-session-id"),
            HeaderValue::from_str(session_id.as_ref())
                .map_err(|_| StreamableHttpError::Client(SecureHttpError::InvalidRequest))?,
        );
        let response = self
            .execute(
                Method::DELETE,
                url,
                headers,
                Vec::new(),
                RedirectMode::Follow,
                None,
            )
            .await
            .map_err(StreamableHttpError::Client)?;
        if let Some(error) = Self::classify_auth_response(&response).await {
            return Err(error);
        }
        if response.status == StatusCode::METHOD_NOT_ALLOWED {
            return Ok(());
        }
        if response.status.is_success() {
            Ok(())
        } else {
            Err(StreamableHttpError::UnexpectedServerResponse(
                "session deletion failed".into(),
            ))
        }
    }

    async fn get_stream(
        &self,
        uri: Arc<str>,
        session_id: Option<Arc<str>>,
        last_event_id: Option<String>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> std::result::Result<
        BoxStream<'static, std::result::Result<Sse, SseError>>,
        StreamableHttpError<Self::Error>,
    > {
        self.get_stream_with_max_sse_event_size(
            uri,
            session_id,
            last_event_id,
            auth_header,
            custom_headers,
            self.policy.limits.max_sse_event_bytes,
        )
        .await
    }

    async fn get_stream_with_max_sse_event_size(
        &self,
        uri: Arc<str>,
        session_id: Option<Arc<str>>,
        last_event_id: Option<String>,
        _auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
        max_sse_event_size: usize,
    ) -> std::result::Result<
        BoxStream<'static, std::result::Result<Sse, SseError>>,
        StreamableHttpError<Self::Error>,
    > {
        if let Some(gate) = &self.sse_reconnect {
            gate.before_stream_request(session_id.is_some(), last_event_id.is_some())
                .await
                .map_err(StreamableHttpError::Client)?;
        }
        let url = Url::parse(uri.as_ref())
            .map_err(|_| StreamableHttpError::Client(SecureHttpError::InvalidRequest))?;
        let mut headers =
            self.request_headers(custom_headers, "text/event-stream, application/json");
        if let Some(session_id) = session_id {
            headers.insert(
                HeaderName::from_static("mcp-session-id"),
                HeaderValue::from_str(session_id.as_ref())
                    .map_err(|_| StreamableHttpError::Client(SecureHttpError::InvalidRequest))?,
            );
        }
        if let Some(last_event_id) = last_event_id {
            headers.insert(
                HeaderName::from_static("last-event-id"),
                HeaderValue::from_str(&last_event_id)
                    .map_err(|_| StreamableHttpError::Client(SecureHttpError::InvalidRequest))?,
            );
        }
        let response = self
            .execute(
                Method::GET,
                url,
                headers,
                Vec::new(),
                RedirectMode::Follow,
                None,
            )
            .await
            .map_err(StreamableHttpError::Client)?;
        if let Some(error) = Self::classify_auth_response(&response).await {
            return Err(error);
        }
        if response.status == StatusCode::METHOD_NOT_ALLOWED {
            return Err(StreamableHttpError::ServerDoesNotSupportSse);
        }
        if !response.status.is_success() {
            return Err(StreamableHttpError::UnexpectedServerResponse(
                "SSE request failed".into(),
            ));
        }
        let kind = content_type(&response.headers);
        if kind.as_deref().is_none_or(|value| !is_sse(value)) {
            return Err(StreamableHttpError::UnexpectedContentType(kind));
        }
        Ok(self.constrain_sse_stream(bounded_sse_stream(
            response.response.bytes_stream(),
            max_sse_event_size,
        )))
    }
}

fn validate_response_headers(
    response: &reqwest::Response,
    cap: usize,
) -> std::result::Result<(), SecureHttpError> {
    let total = response
        .headers()
        .iter()
        .fold(0usize, |total, (name, value)| {
            total
                .saturating_add(name.as_str().len())
                .saturating_add(value.as_bytes().len())
        });
    if total > cap {
        Err(SecureHttpError::HeadersTooLarge)
    } else {
        Ok(())
    }
}

fn is_redirect_safe_header(name: &HeaderName) -> bool {
    matches!(name.as_str(), "accept" | "accept-encoding" | "content-type")
}

fn origin(url: &Url) -> (String, Option<String>, Option<u16>) {
    (
        url.scheme().to_owned(),
        url.host_str().map(|host| host.to_ascii_lowercase()),
        url.port_or_known_default(),
    )
}

fn content_type(headers: &HeaderMap) -> Option<String> {
    headers
        .get(CONTENT_TYPE)
        .map(|value| String::from_utf8_lossy(value.as_bytes()).into_owned())
}

fn is_json(value: &str) -> bool {
    value
        .split(';')
        .next()
        .is_some_and(|mime| mime.trim().eq_ignore_ascii_case(JSON_MIME_TYPE))
}

fn is_sse(value: &str) -> bool {
    value
        .split(';')
        .next()
        .is_some_and(|mime| mime.trim().eq_ignore_ascii_case(SSE_MIME_TYPE))
}

fn extract_scope(challenge: &str) -> Option<String> {
    let lower = challenge.to_ascii_lowercase();
    let index = lower.find("scope=")? + "scope=".len();
    let tail = &challenge[index..];
    if let Some(quoted) = tail.strip_prefix('"') {
        return quoted.split('"').next().map(str::to_owned);
    }
    tail.split([',', ' ']).next().map(str::to_owned)
}

fn is_replay_safe_method(method: &Method) -> bool {
    method == Method::GET || method == Method::HEAD
}

fn normalize_ip(address: IpAddr) -> IpAddr {
    match address {
        IpAddr::V6(address) => address
            .to_ipv4_mapped()
            .map_or(IpAddr::V6(address), IpAddr::V4),
        address => address,
    }
}

fn is_loopback(address: IpAddr) -> bool {
    normalize_ip(address).is_loopback()
}

fn is_private(address: IpAddr) -> bool {
    match normalize_ip(address) {
        IpAddr::V4(address) => {
            address.is_private()
                || matches!(address.octets(), [100, second, ..] if (64..=127).contains(&second))
        }
        IpAddr::V6(address) => (address.segments()[0] & 0xfe00) == 0xfc00,
    }
}

fn is_link_local(address: IpAddr) -> bool {
    match normalize_ip(address) {
        IpAddr::V4(address) => address.is_link_local(),
        IpAddr::V6(address) => (address.segments()[0] & 0xffc0) == 0xfe80,
    }
}

fn is_metadata_address(address: IpAddr) -> bool {
    match normalize_ip(address) {
        IpAddr::V4(address) => matches!(address.octets(), [169, 254, 169, 254]),
        IpAddr::V6(_) => false,
    }
}

fn is_non_routable(address: IpAddr) -> bool {
    match normalize_ip(address) {
        IpAddr::V4(address) => {
            let [first, second, ..] = address.octets();
            address.is_unspecified()
                || address.is_broadcast()
                || address.is_multicast()
                || first == 0
                || first >= 240
                || (first == 192 && second == 0)
                || (first == 198 && matches!(second, 18 | 19 | 51))
                || (first == 203 && second == 0)
        }
        IpAddr::V6(address) => {
            address.is_unspecified() || address.is_multicast() || is_documentation_v6(address)
        }
    }
}

fn is_documentation_v6(address: Ipv6Addr) -> bool {
    let segments = address.segments();
    segments[0] == 0x2001 && segments[1] == 0x0db8
}

#[derive(Debug, thiserror::Error)]
enum BoundedSseError {
    #[error("SSE source failed")]
    Source(#[source] Box<dyn std::error::Error + Send + Sync>),
    #[error("SSE event exceeded {0} bytes")]
    EventTooLarge(usize),
}

#[derive(Debug)]
struct SseLimiter {
    max: usize,
    event_bytes: usize,
    line_bytes: usize,
    previous_cr: bool,
}

impl SseLimiter {
    fn observe(&mut self, chunk: &[u8]) -> std::result::Result<(), BoundedSseError> {
        for byte in chunk {
            if self.previous_cr {
                self.previous_cr = false;
                if *byte == b'\n' {
                    continue;
                }
            }
            match *byte {
                b'\r' => {
                    self.finish_line()?;
                    self.previous_cr = true;
                }
                b'\n' => self.finish_line()?,
                _ => {
                    self.line_bytes = self.line_bytes.saturating_add(1);
                    self.check()?;
                }
            }
        }
        Ok(())
    }

    fn finish_line(&mut self) -> std::result::Result<(), BoundedSseError> {
        if self.line_bytes == 0 {
            self.event_bytes = 0;
        } else {
            self.event_bytes = self
                .event_bytes
                .saturating_add(self.line_bytes)
                .saturating_add(1);
        }
        self.line_bytes = 0;
        self.check()
    }

    fn check(&self) -> std::result::Result<(), BoundedSseError> {
        if self.event_bytes.saturating_add(self.line_bytes) > self.max {
            Err(BoundedSseError::EventTooLarge(self.max))
        } else {
            Ok(())
        }
    }
}

struct BoundedByteStream<S> {
    inner: Pin<Box<S>>,
    limiter: SseLimiter,
    failed: bool,
}

impl<S, E> Stream for BoundedByteStream<S>
where
    S: Stream<Item = std::result::Result<Bytes, E>>,
    E: std::error::Error + Send + Sync + 'static,
{
    type Item = std::result::Result<Bytes, BoundedSseError>;

    fn poll_next(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if self.failed {
            return Poll::Ready(None);
        }
        match ready!(self.inner.as_mut().poll_next(context)) {
            Some(Ok(chunk)) => match self.limiter.observe(&chunk) {
                Ok(()) => Poll::Ready(Some(Ok(chunk))),
                Err(error) => {
                    self.failed = true;
                    Poll::Ready(Some(Err(error)))
                }
            },
            Some(Err(error)) => {
                self.failed = true;
                Poll::Ready(Some(Err(BoundedSseError::Source(Box::new(error)))))
            }
            None => Poll::Ready(None),
        }
    }
}

fn bounded_sse_stream<S, E>(
    stream: S,
    max_event_bytes: usize,
) -> BoxStream<'static, std::result::Result<Sse, SseError>>
where
    S: Stream<Item = std::result::Result<Bytes, E>> + Send + 'static,
    E: std::error::Error + Send + Sync + 'static,
{
    SseStream::from_bytes_stream(BoundedByteStream {
        inner: Box::pin(stream),
        limiter: SseLimiter {
            max: max_event_bytes,
            event_bytes: 0,
            line_bytes: 0,
            previous_cr: false,
        },
        failed: false,
    })
    .boxed()
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;

    use super::*;

    fn policy(trust: TrustConfig) -> HttpSecurityPolicy {
        HttpSecurityPolicy::new(
            ServerName::new("http-test").unwrap(),
            trust,
            OutputLimits::default(),
            TimeoutConfig::default(),
        )
    }

    #[test]
    fn default_policy_blocks_ssrf_ranges() {
        let policy = policy(TrustConfig::default());
        let url = Url::parse("https://example.test/mcp").unwrap();
        for address in [
            IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
            IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)),
            IpAddr::V4(Ipv4Addr::new(169, 254, 169, 254)),
            "::ffff:127.0.0.1".parse().unwrap(),
            "::ffff:169.254.1.1".parse().unwrap(),
        ] {
            assert!(
                policy
                    .validate_hop(&url, &[SocketAddr::new(address, 443)])
                    .is_err()
            );
        }
    }

    #[test]
    fn localhost_requires_all_explicit_grants() {
        let trust = TrustConfig {
            level: TrustLevel::Trusted,
            allow_http: true,
            allow_localhost: true,
            ..TrustConfig::default()
        };
        let policy = policy(trust);
        let url = Url::parse("http://localhost:8080/mcp").unwrap();
        policy
            .validate_hop(
                &url,
                &[SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 8080)],
            )
            .unwrap();
    }

    #[test]
    fn cross_origin_headers_are_classified_fail_closed() {
        assert!(is_redirect_safe_header(&ACCEPT));
        assert!(!is_redirect_safe_header(&http::header::AUTHORIZATION));
        assert!(!is_redirect_safe_header(&HeaderName::from_static(
            "x-api-key"
        )));
    }

    fn reconnect_config(enabled: bool, max_attempts: u32) -> ReconnectConfig {
        ReconnectConfig {
            enabled,
            max_attempts,
            initial_delay_ms: 1,
            max_delay_ms: 1,
        }
    }

    #[tokio::test]
    async fn reconnect_gate_blocks_disabled_reconnects_without_shared_budget() {
        let disabled = SseReconnectGate::new(reconnect_config(false, 2));
        disabled.before_stream_request(true, false).await.unwrap();
        assert!(disabled.before_stream_request(true, false).await.is_err());
        assert!(disabled.before_stream_request(true, true).await.is_err());
        assert_eq!(disabled.retry_delay(0), None);

        let zero_attempts = SseReconnectGate::new(reconnect_config(true, 0));
        zero_attempts
            .before_stream_request(true, false)
            .await
            .unwrap();
        assert!(matches!(
            zero_attempts.before_stream_request(true, true).await,
            Err(SecureHttpError::SseReconnectUnavailable)
        ));
        assert!(matches!(
            zero_attempts.before_stream_request(true, false).await,
            Err(SecureHttpError::SseReconnectUnavailable)
        ));
        assert_eq!(zero_attempts.retry_delay(0), None);

        let bounded = SseReconnectGate::new(reconnect_config(true, 2));
        bounded.before_stream_request(true, false).await.unwrap();
        for _ in 0..8 {
            bounded.before_stream_request(true, true).await.unwrap();
        }
        assert_eq!(
            bounded.retry_delay(0),
            Some(std::time::Duration::from_millis(1))
        );
        assert_eq!(
            bounded.retry_delay(1),
            Some(std::time::Duration::from_millis(1))
        );
        assert_eq!(bounded.retry_delay(2), None);
        assert_eq!(bounded.retry_delay(usize::MAX), None);
    }

    #[test]
    fn reconnect_token_applies_initial_delay_only_to_first_stream_error_reconnect() {
        let gate = SseReconnectGate::new(ReconnectConfig {
            enabled: true,
            max_attempts: 2,
            initial_delay_ms: 40,
            max_delay_ms: 40,
        });
        let token = gate.stream_token();
        assert_eq!(gate.reconnect_request_delay(true, false).unwrap(), None);
        token.note_live();
        assert_eq!(token.extra_get_delay(), Some(Duration::from_millis(40)));
        assert_eq!(token.extra_get_delay(), None);
    }

    #[test]
    fn reconnect_token_and_policy_do_not_stack_delays_on_rmcp_reconnect_path() {
        let config = ReconnectConfig {
            enabled: true,
            max_attempts: 4,
            initial_delay_ms: 10,
            max_delay_ms: 25,
        };
        let gate = SseReconnectGate::new(config.clone());
        let token = gate.stream_token();
        assert_eq!(gate.reconnect_request_delay(true, false).unwrap(), None);
        token.note_live();

        let policy_wait = token.begin_policy_retry(0).unwrap();
        let extra = token.extra_get_delay();
        assert_eq!(policy_wait, Duration::from_millis(10));
        assert_eq!(extra, None);
        assert_eq!(
            policy_wait + extra.unwrap_or_default(),
            config.delay(0),
            "graceful EOF must wait delay(0), not delay(0) + initial"
        );

        token.note_live();
        assert_eq!(
            token.extra_get_delay(),
            Some(config.delay(0)),
            "mid-stream error first GET is the only SDK-skipped wait"
        );

        for attempt in 1..4 {
            let policy_wait = token.begin_policy_retry(attempt).unwrap();
            let extra = token.extra_get_delay();
            let total = policy_wait + extra.unwrap_or_default();
            assert_eq!(extra, None);
            assert_eq!(total, config.delay(attempt as u32));
            assert!(total <= Duration::from_millis(config.max_delay_ms));
        }
        assert_eq!(token.begin_policy_retry(4), None);
    }

    #[tokio::test]
    async fn concurrent_stream_tokens_do_not_consume_each_others_policy_waits() {
        let config = ReconnectConfig {
            enabled: true,
            max_attempts: 4,
            initial_delay_ms: 10,
            max_delay_ms: 10,
        };
        let gate = SseReconnectGate::new(config.clone());
        assert_eq!(gate.reconnect_request_delay(true, false).unwrap(), None);

        let stream_a = gate.stream_token();
        let stream_b = gate.stream_token();
        stream_a.note_live();
        let policy_wait = stream_a.begin_policy_retry(0).unwrap();
        stream_b.note_live();
        let b_extra = stream_b.extra_get_delay();
        let a_extra = stream_a.extra_get_delay();

        let a_total = policy_wait + a_extra.unwrap_or_default();
        assert_eq!(policy_wait, Duration::from_millis(10));
        assert_eq!(a_extra, None, "stream A must consume its own policy wait");
        assert_eq!(a_total, config.delay(0));
        assert!(
            a_total <= Duration::from_millis(config.max_delay_ms),
            "interleaved stream B must not force A to stack delay(0) + initial"
        );
        assert_eq!(
            b_extra,
            Some(config.delay(0)),
            "stream B must keep its mid-stream delay instead of stealing A's wait"
        );

        let both_a = gate.stream_token();
        let both_b = gate.stream_token();
        for token in [&both_a, &both_b] {
            let policy_wait = token.begin_policy_retry(1).unwrap();
            let extra = token.extra_get_delay();
            let total = policy_wait + extra.unwrap_or_default();
            assert_eq!(extra, None);
            assert_eq!(total, config.delay(1));
            assert!(total <= Duration::from_millis(config.max_delay_ms));
        }
    }

    #[test]
    fn reconnect_gate_clamps_server_retry_without_resetting_other_streams() {
        let gate = SseReconnectGate::new(reconnect_config(true, 1));
        let event = gate.constrain_event(
            Sse::default()
                .retry(u64::MAX)
                .data("{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}"),
        );
        assert_eq!(event.retry, None);
        assert_eq!(
            gate.retry_delay(0),
            Some(std::time::Duration::from_millis(1))
        );
        assert_eq!(gate.retry_delay(1), None);
    }

    #[tokio::test]
    async fn concurrent_sse_streams_do_not_share_attempt_budgets() {
        let gate = SseReconnectGate::new(reconnect_config(true, 1));
        let health = gate.clone();
        let failing = gate.clone();
        let cancelled = gate.clone();
        let health_task = tokio::spawn(async move {
            health.before_stream_request(true, false).await.unwrap();
            let _ = health.constrain_event(
                Sse::default().data("{\"jsonrpc\":\"2.0\",\"method\":\"notifications/ping\"}"),
            );
            health.retry_delay(0)
        });
        let failing_task = tokio::spawn(async move {
            failing.before_stream_request(true, true).await.unwrap();
            (failing.retry_delay(0), failing.retry_delay(1))
        });
        let cancel_task =
            tokio::spawn(async move { cancelled.before_stream_request(true, true).await });
        cancel_task.abort();
        let _ = cancel_task.await;
        let health_delay = health_task.await.unwrap();
        let (first, exhausted) = failing_task.await.unwrap();
        assert_eq!(health_delay, first);
        assert!(first.is_some());
        assert!(exhausted.is_none());
        gate.before_stream_request(true, true).await.unwrap();
        assert_eq!(gate.retry_delay(1), None);
    }

    #[test]
    fn dropping_a_live_token_does_not_skip_a_sibling_stream_delay() {
        let config = ReconnectConfig {
            enabled: true,
            max_attempts: 2,
            initial_delay_ms: 10,
            max_delay_ms: 10,
        };
        let gate = SseReconnectGate::new(config.clone());
        let live = gate.stream_token();
        live.note_live();
        drop(live);
        let later = gate.stream_token();
        later.note_live();
        assert_eq!(later.extra_get_delay(), Some(config.delay(0)));
    }

    #[test]
    fn stream_error_keeps_skipped_delay_on_the_same_token_after_inner_drop() {
        let config = ReconnectConfig {
            enabled: true,
            max_attempts: 2,
            initial_delay_ms: 10,
            max_delay_ms: 10,
        };
        let token = SseReconnectGate::new(config.clone()).stream_token();
        token.note_live();
        drop(token.clone());
        assert_eq!(token.extra_get_delay(), Some(config.delay(0)));
        assert_eq!(token.extra_get_delay(), None);
    }

    #[tokio::test]
    async fn cancelled_policy_wait_does_not_skip_a_later_stream_delay() {
        let config = ReconnectConfig {
            enabled: true,
            max_attempts: 2,
            initial_delay_ms: 10,
            max_delay_ms: 10,
        };
        let gate = SseReconnectGate::new(config.clone());
        let waiting = gate.stream_token();
        let wait = waiting.policy_retry_wait(0).expect("retry(0) should arm");
        drop(wait);

        waiting.note_live();
        assert_eq!(
            waiting.extra_get_delay(),
            Some(config.delay(0)),
            "unpolled cancel must clear pending so the same token still applies initial backoff"
        );

        let later = gate.stream_token();
        later.note_live();
        assert_eq!(
            later.extra_get_delay(),
            Some(config.delay(0)),
            "cancelled policy wait must not be consumed by a later stream"
        );
    }

    #[tokio::test]
    async fn cancelled_failed_get_policy_wait_does_not_skip_or_stack_delays() {
        let config = ReconnectConfig {
            enabled: true,
            max_attempts: 4,
            initial_delay_ms: 10,
            max_delay_ms: 25,
        };
        let gate = SseReconnectGate::new(config.clone());
        let outage = gate.stream_token();
        outage.note_live();
        assert_eq!(
            outage.extra_get_delay(),
            Some(config.delay(0)),
            "mid-stream error GET must not be a zero delay"
        );

        let (armed, armed_rx) = tokio::sync::oneshot::channel();
        let waiting = outage.clone();
        let handle = tokio::spawn(async move {
            let wait = waiting
                .policy_retry_wait(1)
                .expect("failed GET must arm retry(1)");
            let _ = armed.send(());
            wait.await;
        });
        armed_rx.await.expect("failed-GET policy wait should arm");
        handle.abort();
        let _ = handle.await;

        assert_eq!(
            outage.extra_get_delay(),
            None,
            "cancelled failed-GET wait must not stack initial_delay on the same outage"
        );

        let later = gate.stream_token();
        later.note_live();
        assert_eq!(
            later.extra_get_delay(),
            Some(config.delay(0)),
            "cancelled failed-GET wait must not skip a later stream's delay"
        );
        assert_eq!(later.begin_policy_retry(0), Some(config.delay(0)));
        assert_eq!(later.begin_policy_retry(4), None);
        assert_eq!(
            gate.stream_token().begin_policy_retry(0),
            Some(config.delay(0)),
            "attempts stay independent after a cancelled failed GET"
        );
    }

    #[tokio::test]
    async fn concurrent_failed_reconnect_gets_keep_independent_attempts() {
        let config = ReconnectConfig {
            enabled: true,
            max_attempts: 4,
            initial_delay_ms: 10,
            max_delay_ms: 25,
        };
        let gate = SseReconnectGate::new(config.clone());
        let stream_a = gate.stream_token();
        let stream_b = gate.stream_token();
        stream_a.note_live();
        stream_b.note_live();
        assert_eq!(stream_a.extra_get_delay(), Some(config.delay(0)));
        let wait_a = stream_a
            .policy_retry_wait(1)
            .expect("stream A failed GET should wait retry(1)");
        assert_eq!(
            stream_b.extra_get_delay(),
            Some(config.delay(0)),
            "stream B must not steal A's pending failed-GET wait"
        );
        drop(wait_a);
        let wait_b = stream_b
            .policy_retry_wait(1)
            .expect("stream B failed GET should wait retry(1)");
        wait_b.await;
        let extra_b = stream_b.extra_get_delay();
        let extra_a = stream_a.extra_get_delay();
        assert_eq!(extra_b, None, "completed wait must not stack delay");
        assert_eq!(extra_a, None, "cancelled wait must not skip or stack delay");
        let total_b = config.delay(1) + extra_b.unwrap_or_default();
        assert_eq!(total_b, config.delay(1));
        assert!(total_b <= Duration::from_millis(config.max_delay_ms));
        assert_eq!(stream_a.begin_policy_retry(4), None);
        assert_eq!(
            stream_b.begin_policy_retry(0),
            Some(config.delay(0)),
            "stream B attempts stay independent of A's exhausted wait"
        );
    }

    #[tokio::test]
    async fn graceful_eof_policy_wait_does_not_stack_after_inner_stream_drop() {
        let config = ReconnectConfig {
            enabled: true,
            max_attempts: 2,
            initial_delay_ms: 10,
            max_delay_ms: 10,
        };
        let token = SseReconnectGate::new(config.clone()).stream_token();
        token.note_live();
        let policy_wait = token.begin_policy_retry(0).unwrap();
        drop(token.clone());
        let extra = token.extra_get_delay();
        assert_eq!(policy_wait, config.delay(0));
        assert_eq!(
            extra, None,
            "GET after EOF policy wait must not add initial_delay after the inner stream drops"
        );
    }

    #[derive(Debug, Clone)]
    struct LocalResolver(SocketAddr);

    #[async_trait]
    impl DnsResolver for LocalResolver {
        async fn resolve(&self, _host: &str, _port: u16) -> Result<Vec<SocketAddr>> {
            Ok(vec![self.0])
        }
    }

    #[tokio::test]
    async fn get_stream_reconnect_after_sse_error_honors_zero_attempt_budget() {
        use axum::{Router, extract::State, response::IntoResponse, routing::get};

        #[derive(Clone)]
        struct GetCount(Arc<std::sync::atomic::AtomicUsize>);

        async fn sse(State(count): State<GetCount>) -> impl IntoResponse {
            count.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            (
                StatusCode::OK,
                [("content-type", "text/event-stream")],
                "id: event-1\ndata: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/ping\"}\n\n",
            )
        }

        let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let gets = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let server = tokio::spawn({
            let gets = Arc::clone(&gets);
            async move {
                axum::serve(
                    listener,
                    Router::new()
                        .route("/mcp", get(sse))
                        .with_state(GetCount(gets)),
                )
                .await
                .unwrap();
            }
        });
        let trust = TrustConfig {
            level: TrustLevel::Trusted,
            allow_http: true,
            allow_localhost: true,
            ..TrustConfig::default()
        };
        let blocked = SecureHttpClient::new(
            policy(trust.clone()),
            DnsResolverHandle::new(LocalResolver(address)),
        )
        .with_sse_reconnect(SseReconnectGate::new(reconnect_config(true, 0)));
        let uri: Arc<str> = format!("http://sse.test:{}/mcp", address.port()).into();
        let mut stream = blocked
            .get_stream(
                uri.clone(),
                Some(Arc::from("session-1")),
                None,
                None,
                HashMap::new(),
            )
            .await
            .unwrap();
        assert_eq!(
            stream.next().await.unwrap().unwrap().id.as_deref(),
            Some("event-1")
        );
        assert_eq!(gets.load(std::sync::atomic::Ordering::SeqCst), 1);
        let error = match blocked
            .get_stream(
                uri.clone(),
                Some(Arc::from("session-1")),
                Some("event-1".into()),
                None,
                HashMap::new(),
            )
            .await
        {
            Ok(_) => panic!("zero-attempt reconnect must not send GET"),
            Err(error) => error,
        };
        assert!(matches!(
            error,
            StreamableHttpError::Client(SecureHttpError::SseReconnectUnavailable)
        ));
        assert_eq!(gets.load(std::sync::atomic::Ordering::SeqCst), 1);

        let allowed = SecureHttpClient::new(
            policy(trust),
            DnsResolverHandle::new(LocalResolver(address)),
        )
        .with_sse_reconnect(SseReconnectGate::new(reconnect_config(true, 1)));
        let mut resumed = allowed
            .get_stream(
                uri,
                Some(Arc::from("session-1")),
                Some("event-1".into()),
                None,
                HashMap::new(),
            )
            .await
            .unwrap();
        assert_eq!(
            resumed.next().await.unwrap().unwrap().id.as_deref(),
            Some("event-1")
        );
        assert_eq!(gets.load(std::sync::atomic::Ordering::SeqCst), 2);
        server.abort();
    }

    #[derive(Clone)]
    struct UpgradableBearer {
        upgraded: Arc<std::sync::atomic::AtomicBool>,
        upgrades: Arc<std::sync::atomic::AtomicUsize>,
    }

    #[async_trait]
    impl BearerTokenProvider for UpgradableBearer {
        async fn token(&self) -> std::result::Result<SecretValue, SecureHttpError> {
            let value = if self.upgraded.load(std::sync::atomic::Ordering::SeqCst) {
                "expanded-token"
            } else {
                "initial-token"
            };
            Ok(SecretValue::new(value))
        }

        async fn upgrade_scope(
            &self,
            required_scope: &str,
        ) -> std::result::Result<bool, SecureHttpError> {
            assert_eq!(required_scope, "write");
            self.upgrades
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.upgraded
                .store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(true)
        }
    }

    #[tokio::test]
    async fn scope_upgrade_does_not_replay_a_post() {
        use axum::{
            Router, extract::State, http::HeaderMap as AxumHeaderMap, response::IntoResponse,
            routing::post,
        };

        async fn handler(
            State(calls): State<Arc<std::sync::atomic::AtomicUsize>>,
            _headers: AxumHeaderMap,
        ) -> impl IntoResponse {
            calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            (
                StatusCode::FORBIDDEN,
                [(WWW_AUTHENTICATE, "Bearer scope=\"write\"")],
                "",
            )
                .into_response()
        }

        let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let server_calls = Arc::clone(&calls);
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                Router::new()
                    .route("/mcp", post(handler))
                    .with_state(server_calls),
            )
            .await
            .unwrap();
        });
        let upgraded = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let upgrades = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let trust = TrustConfig {
            level: TrustLevel::Trusted,
            allow_http: true,
            allow_localhost: true,
            ..TrustConfig::default()
        };
        let client = SecureHttpClient::new(
            policy(trust),
            DnsResolverHandle::new(LocalResolver(address)),
        )
        .with_bearer_provider(UpgradableBearer {
            upgraded,
            upgrades: Arc::clone(&upgrades),
        });
        let response = client
            .execute(
                Method::POST,
                Url::parse(&format!("http://oauth.test:{}/mcp", address.port())).unwrap(),
                HeaderMap::new(),
                Vec::new(),
                RedirectMode::Follow,
                None,
            )
            .await
            .unwrap();
        assert_eq!(response.status, StatusCode::FORBIDDEN);
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(upgrades.load(std::sync::atomic::Ordering::SeqCst), 1);
        server.abort();
    }
}

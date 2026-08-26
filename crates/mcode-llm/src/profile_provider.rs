//! HTTP provider assembled from a data-only [`ProviderProfile`].
//!
//! One transport loop drives all reusable wire adapters. Header precedence is:
//! Pi identity/profile headers, ordinary model metadata, authentication, then
//! explicit provider and per-call overlays.

use std::collections::BTreeMap;
use std::fmt;
use std::time::Duration;

use async_trait::async_trait;
use mcode_core::message::ContentBlock;
use reqwest::header::{ACCEPT, AUTHORIZATION, HeaderMap, HeaderName, HeaderValue, USER_AGENT};
use tokio_stream::StreamExt;
use tokio_util::sync::CancellationToken;

use crate::anthropic::{self, AnthropicAggregator};
use crate::chat_completions::ChatCompletionAggregator;
use crate::error::LlmError;
use crate::identity::ClientIdentity;
use crate::profile::{
    ApiKey, AuthScheme, HeaderOverlay, HeaderProfile, ModelLayers, ModelSettings, ProviderProfile,
    WireKind, is_auth_header, resolve_model_settings,
};
use crate::provider::{Provider, Request, StreamEvent};
use crate::responses::{self, ResponsesAggregator};
use crate::sse::SseFramer;
use crate::stream::{EventStream, EventStreamSender};

/// Maximum bytes read from a non-success HTTP response.
const MAX_ERROR_BODY_BYTES: usize = 8 * 1_024;

/// Maximum same-origin redirects followed for one request.
///
/// This matches reqwest's default redirect limit while replacing its
/// cross-origin behavior with a stricter replay and credential boundary.
const MAX_REDIRECTS: usize = 10;

/// Explicit options applied to one provider call.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProviderCallOptions {
    /// Highest-priority ordinary model settings.
    pub model: ModelSettings,
    /// Explicit custom headers applied last, including intentional auth
    /// replacement.
    pub headers: HeaderOverlay,
}

/// A provider configured by endpoint/profile data and a reusable wire adapter.
#[derive(Clone)]
pub struct ProfileProvider {
    client: reqwest::Client,
    profile: ProviderProfile,
    api_key: Option<ApiKey>,
    identity: ClientIdentity,
    timeout: Option<Duration>,
    catalog_settings: ModelSettings,
    provider_settings: ModelSettings,
    selection_settings: ModelSettings,
    custom_headers: HeaderOverlay,
}

impl fmt::Debug for ProfileProvider {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProfileProvider")
            .field("profile", &self.profile)
            .field("api_key", &self.api_key)
            .field("identity", &self.identity)
            .field("timeout", &self.timeout)
            .field("catalog_settings", &self.catalog_settings)
            .field("provider_settings", &self.provider_settings)
            .field("selection_settings", &self.selection_settings)
            .field("custom_headers", &self.custom_headers)
            .finish_non_exhaustive()
    }
}

impl ProfileProvider {
    /// Creates an authenticated provider from a validated profile and key.
    ///
    /// # Errors
    ///
    /// Returns [`LlmError::Config`] when the profile or key is invalid, or when
    /// the HTTP client cannot be built.
    pub fn new(profile: ProviderProfile, api_key: impl Into<String>) -> Result<Self, LlmError> {
        let api_key = ApiKey::new(api_key)?;
        Self::with_api_key(profile, api_key)
    }

    /// Creates an authenticated provider from a redacting [`ApiKey`].
    ///
    /// # Errors
    ///
    /// Returns [`LlmError::Config`] when the profile is invalid, uses no auth,
    /// or the HTTP client cannot be built.
    pub fn with_api_key(profile: ProviderProfile, api_key: ApiKey) -> Result<Self, LlmError> {
        if profile.auth().scheme == AuthScheme::None {
            return Err(LlmError::Config(format!(
                "provider '{}' is configured without authentication",
                profile.id()
            )));
        }
        Self::from_parts(profile, Some(api_key))
    }

    /// Creates a provider for a profile that uses no authentication.
    ///
    /// # Errors
    ///
    /// Returns [`LlmError::Config`] when the profile requires authentication or
    /// fails validation.
    pub fn without_auth(profile: ProviderProfile) -> Result<Self, LlmError> {
        if profile.auth().scheme != AuthScheme::None {
            return Err(LlmError::Config(format!(
                "provider '{}' requires an API key",
                profile.id()
            )));
        }
        Self::from_parts(profile, None)
    }

    /// Resolves a profile's base URL and API key from environment references.
    ///
    /// Provider profiles serialize only the environment-variable name. Secret
    /// values stay at the explicit/environment credential boundary and never
    /// enter ordinary JSON configuration.
    ///
    /// # Errors
    ///
    /// Returns [`LlmError::Config`] for missing credentials, invalid overrides,
    /// or an invalid profile.
    pub fn from_profile(mut profile: ProviderProfile) -> Result<Self, LlmError> {
        // Validate names before passing them to `std::env::var`, which panics
        // for empty names and names containing `=` or NUL.
        profile.validate()?;
        if let Some(base_url) = profile
            .base_url_env()
            .and_then(|name| std::env::var(name).ok())
            .filter(|value| !value.trim().is_empty())
        {
            profile = profile.with_base_url(base_url)?;
        }
        match profile.auth().scheme {
            AuthScheme::None => Self::without_auth(profile),
            AuthScheme::Bearer | AuthScheme::XApiKey => {
                let env = profile.auth().env.as_deref().ok_or_else(|| {
                    LlmError::Config("authenticated profile has no auth env".into())
                })?;
                let key = std::env::var(env)
                    .ok()
                    .filter(|value| !value.trim().is_empty())
                    .ok_or_else(|| {
                        LlmError::Config(format!(
                            "no API key for provider '{}': set {env} or inject one explicitly",
                            profile.id()
                        ))
                    })?;
                Self::new(profile, key)
            }
        }
    }

    fn from_parts(profile: ProviderProfile, api_key: Option<ApiKey>) -> Result<Self, LlmError> {
        profile.validate()?;
        let client = reqwest::Client::builder()
            .redirect(same_origin_redirect_policy())
            .build()
            .map_err(map_reqwest_error)?;
        Ok(Self {
            client,
            profile,
            api_key,
            identity: ClientIdentity::system_pi(),
            timeout: None,
            catalog_settings: ModelSettings::default(),
            provider_settings: ModelSettings::default(),
            selection_settings: ModelSettings::default(),
            custom_headers: HeaderOverlay::new(),
        })
    }

    /// Returns the immutable provider profile.
    pub fn profile(&self) -> &ProviderProfile {
        &self.profile
    }

    /// Returns the complete protocol endpoint URL.
    pub fn endpoint(&self) -> String {
        self.profile.endpoint()
    }

    /// Sets a total timeout spanning connect through response body.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// Replaces the default Pi-compatible identity.
    pub fn with_identity(mut self, identity: ClientIdentity) -> Self {
        self.identity = identity;
        self
    }

    /// Sets the lowest-priority catalog settings for the selected model.
    pub fn with_catalog_settings(mut self, settings: ModelSettings) -> Self {
        self.catalog_settings = settings;
        self
    }

    /// Sets provider-configuration model overrides.
    pub fn with_provider_settings(mut self, settings: ModelSettings) -> Self {
        self.provider_settings = settings;
        self
    }

    /// Sets selected-model overrides.
    pub fn with_selection_settings(mut self, settings: ModelSettings) -> Self {
        self.selection_settings = settings;
        self
    }

    /// Sets an explicit custom header overlay applied after authentication.
    pub fn with_headers(mut self, headers: HeaderOverlay) -> Self {
        self.custom_headers = headers;
        self
    }

    /// Injects an explicit OpenCode session header.
    ///
    /// # Errors
    ///
    /// Returns [`LlmError::Config`] when `session` is not a valid header value.
    pub fn with_opencode_session(mut self, session: impl Into<String>) -> Result<Self, LlmError> {
        let _ = self
            .custom_headers
            .insert("x-opencode-session", session.into())?;
        Ok(self)
    }

    /// Returns the fully resolved model settings for a request.
    pub fn resolved_settings(
        &self,
        request: &Request,
        per_call: Option<&ModelSettings>,
    ) -> ModelSettings {
        resolve_model_settings(ModelLayers {
            catalog: Some(&self.catalog_settings),
            provider_correction: self.profile.model_correction(request.model.as_str()),
            provider_config: Some(&self.provider_settings),
            selection: Some(&self.selection_settings),
            per_call,
        })
    }

    /// Returns a redacted snapshot of the exact request header precedence.
    ///
    /// Authentication values are replaced with `[REDACTED]`.
    ///
    /// # Errors
    ///
    /// Returns [`LlmError::Config`] if a deserialized or custom header is
    /// invalid.
    pub fn header_snapshot(
        &self,
        request: &Request,
        options: Option<&ProviderCallOptions>,
    ) -> Result<BTreeMap<String, String>, LlmError> {
        let resolved = self.resolved_settings(request, options.map(|value| &value.model));
        let headers = assemble_headers(
            &self.profile,
            &self.identity,
            &resolved.headers,
            self.api_key.as_ref(),
            &self.custom_headers,
            options.map(|value| &value.headers),
        )?;
        Ok(headers
            .iter()
            .map(|(name, value)| {
                let value = if is_auth_header(name.as_str()) {
                    "[REDACTED]".to_owned()
                } else {
                    value.to_str().unwrap_or("[INVALID]").to_owned()
                };
                (name.as_str().to_owned(), value)
            })
            .collect())
    }

    /// Starts a stream with explicit highest-priority per-call options.
    ///
    /// # Errors
    ///
    /// Returns configuration errors before spawning I/O. Network/protocol
    /// failures are delivered as terminal [`StreamEvent::Error`] values.
    pub async fn stream_with_options(
        &self,
        request: &Request,
        options: &ProviderCallOptions,
        cancel: CancellationToken,
    ) -> Result<EventStream, LlmError> {
        self.stream_inner(request, Some(options), cancel).await
    }

    async fn stream_inner(
        &self,
        request: &Request,
        options: Option<&ProviderCallOptions>,
        cancel: CancellationToken,
    ) -> Result<EventStream, LlmError> {
        self.profile.validate()?;
        let settings = self.resolved_settings(request, options.map(|value| &value.model));
        let headers = assemble_headers(
            &self.profile,
            &self.identity,
            &settings.headers,
            self.api_key.as_ref(),
            &self.custom_headers,
            options.map(|value| &value.headers),
        )?;
        let body = build_body(&self.profile, request, &settings);
        let decoder = WireDecoder::new(self.profile.wire());
        let client = self.client.clone();
        let url = self.endpoint();
        let timeout = self.timeout;
        let provider_id = self.profile.id().to_owned();
        let (sender, stream) = EventStream::channel_with_cancel(cancel.clone());

        tokio::spawn(async move {
            let mut request_builder = client.post(&url).headers(headers).json(&body);
            if let Some(timeout) = timeout {
                request_builder = request_builder.timeout(timeout);
            }
            let response = tokio::select! {
                biased;
                response = request_builder.send() => match response {
                    Ok(response) => response,
                    Err(error) => {
                        let _ = sender.push(StreamEvent::Error(map_reqwest_error(error)));
                        return;
                    }
                },
                _ = cancel.cancelled() => {
                    let _ = sender.push(StreamEvent::Error(LlmError::Cancelled));
                    return;
                },
                _ = sender.closed() => {
                    // The consumer dropped the stream while the request was
                    // still in flight; stop instead of holding the task and
                    // connection open with nobody listening.
                    return;
                }
            };

            let status = response.status();
            if !status.is_success() {
                let body = tokio::select! {
                    biased;
                    body = bounded_error_body(response) => body,
                    _ = cancel.cancelled() => Err(LlmError::Cancelled),
                    _ = sender.closed() => {
                        // The consumer dropped the stream while the error
                        // body was still pending; nobody will read the
                        // diagnostic, so abandon the stalled body instead
                        // of parking this task (and its connection) on
                        // network data indefinitely.
                        return;
                    }
                };
                match body {
                    Ok(body) => {
                        let _ = sender.push(StreamEvent::Error(LlmError::Http {
                            status: status.as_u16(),
                            body,
                        }));
                    }
                    Err(LlmError::Cancelled) => {
                        let _ = sender.push(StreamEvent::Error(LlmError::Cancelled));
                    }
                    Err(error) => {
                        // The response status is already known. A body read
                        // failure adds diagnostics but must not erase that HTTP
                        // classification (including timeout/transport causes).
                        let _ = sender.push(StreamEvent::Error(LlmError::Http {
                            status: status.as_u16(),
                            body: LlmError::excerpt(format!(
                                "failed to read error response body: {error}"
                            )),
                        }));
                    }
                }
                return;
            }
            let provenance = ReplayProvenance {
                provider: provider_id,
                // Use the URL that produced the successful response rather
                // than the configured URL. The redirect policy guarantees
                // that its origin is still inside the request trust domain.
                endpoint: response.url().origin().ascii_serialization(),
            };
            if !sender.push(StreamEvent::Start) {
                return;
            }
            drive_response(response, decoder, sender, cancel, &provenance).await;
        });
        Ok(stream)
    }
}

#[async_trait]
impl Provider for ProfileProvider {
    fn id(&self) -> &str {
        self.profile.id()
    }

    async fn stream(
        &self,
        request: &Request,
        cancel: CancellationToken,
    ) -> Result<EventStream, LlmError> {
        self.stream_inner(request, None, cancel).await
    }
}

#[derive(Debug)]
enum WireDecoder {
    Chat(ChatCompletionAggregator),
    Responses(ResponsesAggregator),
    Anthropic(AnthropicAggregator),
}

impl WireDecoder {
    fn new(wire: WireKind) -> Self {
        match wire {
            WireKind::OpenAiChatCompletions => Self::Chat(ChatCompletionAggregator::new()),
            WireKind::OpenAiResponses => Self::Responses(ResponsesAggregator::new()),
            WireKind::AnthropicMessages => Self::Anthropic(AnthropicAggregator::new()),
        }
    }

    fn on_data(&mut self, payload: &str) -> Result<Vec<StreamEvent>, LlmError> {
        match self {
            Self::Chat(aggregator) => aggregator.on_data(payload),
            Self::Responses(aggregator) => aggregator.on_data(payload),
            Self::Anthropic(aggregator) => aggregator.on_data(payload),
        }
    }

    fn is_terminal(&self) -> bool {
        match self {
            Self::Chat(_) => false,
            Self::Responses(aggregator) => aggregator.is_terminal(),
            Self::Anthropic(aggregator) => aggregator.is_terminal(),
        }
    }

    fn finish(&mut self) -> Result<Vec<StreamEvent>, LlmError> {
        match self {
            Self::Chat(aggregator) => aggregator.finish(),
            Self::Responses(aggregator) => aggregator.finish(),
            Self::Anthropic(aggregator) => aggregator.finish(),
        }
    }
}

fn build_body(
    profile: &ProviderProfile,
    request: &Request,
    settings: &ModelSettings,
) -> serde_json::Value {
    // Opaque replay state only crosses the wire inside the profile's
    // replay trust domain (itself on its current endpoint, plus
    // explicitly trusted gateway producers).
    let replay = profile.replay_domain();
    match profile.wire() {
        WireKind::OpenAiChatCompletions => {
            crate::chat_completions::build_request_body_with_settings(request, settings)
        }
        WireKind::OpenAiResponses => {
            responses::build_request_body_with_settings(request, settings, &replay)
        }
        WireKind::AnthropicMessages => {
            anthropic::build_request_body_with_settings(request, settings, &replay)
        }
    }
}

async fn drive_response(
    response: reqwest::Response,
    mut decoder: WireDecoder,
    sender: EventStreamSender,
    cancel: CancellationToken,
    provenance: &ReplayProvenance,
) {
    let mut bytes = response.bytes_stream();
    let mut framer = SseFramer::new();
    // `Ok(false)` marks a vanished receiver (EventStream dropped); the
    // loops below stop immediately so no HTTP task or connection is left
    // parked on network data nobody will read.
    let outcome: Result<bool, LlmError> = 'read: {
        loop {
            let chunk = tokio::select! {
                biased;
                _ = cancel.cancelled() => break 'read Err(LlmError::Cancelled),
                _ = sender.closed() => return,
                chunk = bytes.next() => match chunk {
                    Some(Ok(bytes)) => bytes,
                    Some(Err(error)) => break 'read Err(map_reqwest_error(error)),
                    None => break 'read Ok(true),
                },
            };
            match push_payloads(&mut decoder, &sender, framer.feed(&chunk)) {
                Ok(true) => {}
                Ok(false) => return,
                Err(error) => break 'read Err(error),
            }
            if framer.is_done() || decoder.is_terminal() {
                break 'read Ok(true);
            }
        }
    };

    let outcome = match outcome {
        Ok(true) if !framer.is_done() && !decoder.is_terminal() => {
            push_payloads(&mut decoder, &sender, framer.finish())
        }
        other => other,
    };
    match outcome {
        // Receiver gone: nothing left to deliver, so skip the final flush.
        Ok(false) => {}
        Ok(true) => match decoder.finish() {
            Ok(mut events) => {
                stamp_replay_provenance(&mut events, provenance);
                for event in events {
                    if !sender.push(event) {
                        return;
                    }
                }
            }
            Err(error) => {
                let _ = sender.push(StreamEvent::Error(error));
            }
        },
        Err(error) => {
            let _ = sender.push(StreamEvent::Error(error));
        }
    }
}

/// Producing-side provenance stamped onto replay state.
///
/// The provider id comes from the effective profile and the endpoint comes
/// from the final successful response URL. Cross-origin redirects are stopped
/// before reqwest can replay credentials or the POST body.
struct ReplayProvenance {
    provider: String,
    endpoint: String,
}

/// Records the producing profile and endpoint origin on every replay
/// state handed to the caller, so persisted sessions carry explicit
/// provenance for the wire, profile, and endpoint trust boundary.
fn stamp_replay_provenance(events: &mut [StreamEvent], provenance: &ReplayProvenance) {
    for event in events.iter_mut() {
        let StreamEvent::Done { message } = event else {
            continue;
        };
        for block in &mut message.blocks {
            let ContentBlock::Thinking(thinking) = block else {
                continue;
            };
            if let Some(state) = &mut thinking.replay {
                state
                    .provider
                    .get_or_insert_with(|| provenance.provider.clone());
                state
                    .endpoint
                    .get_or_insert_with(|| provenance.endpoint.clone());
            }
        }
    }
}

/// Feeds decoded events for one SSE frame into the stream.
///
/// Returns `Ok(false)` when the receiving [`EventStream`] is already gone
/// so callers stop reading instead of collapsing the lost receiver into a
/// successful continue.
fn push_payloads(
    decoder: &mut WireDecoder,
    sender: &EventStreamSender,
    payloads: Vec<String>,
) -> Result<bool, LlmError> {
    for payload in payloads {
        for event in decoder.on_data(&payload)? {
            if !sender.push(event) {
                return Ok(false);
            }
        }
        if decoder.is_terminal() {
            break;
        }
    }
    Ok(true)
}

fn assemble_headers(
    profile: &ProviderProfile,
    identity: &ClientIdentity,
    model_headers: &HeaderOverlay,
    api_key: Option<&ApiKey>,
    custom_headers: &HeaderOverlay,
    per_call_headers: Option<&HeaderOverlay>,
) -> Result<HeaderMap, LlmError> {
    let mut headers = HeaderMap::new();
    headers.insert(ACCEPT, HeaderValue::from_static("text/event-stream"));
    insert_header(&mut headers, USER_AGENT.as_str(), identity.user_agent())?;
    if profile.wire() == WireKind::AnthropicMessages {
        insert_header(&mut headers, "anthropic-version", "2023-06-01")?;
    }
    match profile.header_profile() {
        HeaderProfile::Pi => {}
        HeaderProfile::OpenRouter => {
            insert_header(&mut headers, "http-referer", "https://pi.dev")?;
            insert_header(&mut headers, "x-openrouter-title", "pi")?;
            insert_header(&mut headers, "x-openrouter-categories", "cli-agent")?;
        }
        HeaderProfile::OpenCode => {
            insert_header(&mut headers, "x-opencode-client", "pi")?;
        }
    }
    apply_overlay(&mut headers, profile.headers(), false)?;
    apply_overlay(&mut headers, model_headers, true)?;

    match profile.auth().scheme {
        AuthScheme::Bearer => {
            let key = api_key.ok_or_else(|| {
                LlmError::Config(format!("provider '{}' has no API key", profile.id()))
            })?;
            let value = format!("Bearer {}", key.expose());
            insert_header(&mut headers, AUTHORIZATION.as_str(), &value)?;
        }
        AuthScheme::XApiKey => {
            let key = api_key.ok_or_else(|| {
                LlmError::Config(format!("provider '{}' has no API key", profile.id()))
            })?;
            insert_header(&mut headers, "x-api-key", key.expose())?;
        }
        AuthScheme::None => {}
    }
    apply_overlay(&mut headers, custom_headers, false)?;
    if let Some(per_call_headers) = per_call_headers {
        apply_overlay(&mut headers, per_call_headers, false)?;
    }
    Ok(headers)
}

fn apply_overlay(
    headers: &mut HeaderMap,
    overlay: &HeaderOverlay,
    protect_auth: bool,
) -> Result<(), LlmError> {
    for (name, value) in overlay.iter() {
        if protect_auth && is_auth_header(name) {
            continue;
        }
        insert_header(headers, name, value)?;
    }
    Ok(())
}

fn insert_header(headers: &mut HeaderMap, name: &str, value: &str) -> Result<(), LlmError> {
    let name = HeaderName::from_bytes(name.as_bytes())
        .map_err(|_| LlmError::Config("invalid HTTP header name".into()))?;
    let value = HeaderValue::from_str(value)
        .map_err(|_| LlmError::Config(format!("invalid value for header '{name}'")))?;
    headers.insert(name, value);
    Ok(())
}

/// Follows redirects only while the complete chain remains on one origin.
///
/// Stopping returns the redirect response to the caller without issuing a
/// request to the new origin. This protects every credential-like custom
/// header and the replay-bearing POST body, not just reqwest's built-in list
/// of sensitive headers.
fn same_origin_redirect_policy() -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(|attempt| {
        if attempt.previous().len() > MAX_REDIRECTS {
            return attempt.error("too many redirects");
        }
        let stays_on_origin = attempt
            .previous()
            .first()
            .is_some_and(|original| original.origin() == attempt.url().origin());
        if stays_on_origin {
            attempt.follow()
        } else {
            attempt.stop()
        }
    })
}

async fn bounded_error_body(response: reqwest::Response) -> Result<String, LlmError> {
    let mut stream = response.bytes_stream();
    let mut body = Vec::new();
    let mut truncated = false;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(map_reqwest_error)?;
        let remaining = MAX_ERROR_BODY_BYTES.saturating_sub(body.len());
        if chunk.len() > remaining {
            body.extend_from_slice(&chunk[..remaining]);
            truncated = true;
            break;
        }
        body.extend_from_slice(&chunk);
        if body.len() == MAX_ERROR_BODY_BYTES {
            truncated = true;
            break;
        }
    }
    let mut body = String::from_utf8_lossy(&body).into_owned();
    if truncated {
        body.push_str("… [truncated]");
    }
    Ok(LlmError::excerpt(body))
}

fn map_reqwest_error(error: reqwest::Error) -> LlmError {
    if error.is_timeout() {
        LlmError::Timeout
    } else {
        LlmError::Transport(error.without_url().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::ClientIdentity;
    use crate::profile::{openai_profile, opencode_profile, openrouter_profile};

    fn request() -> Request {
        Request::new("model")
    }

    #[test]
    fn header_snapshot_has_pi_identity_and_openrouter_attribution() {
        let provider = ProfileProvider::new(openrouter_profile(), "secret")
            .unwrap()
            .with_identity(ClientIdentity::pi("linux", "6.8", "x64").unwrap());
        let snapshot = provider.header_snapshot(&request(), None).unwrap();
        assert_eq!(snapshot["user-agent"], "pi (linux 6.8; x64)");
        assert_eq!(snapshot["http-referer"], "https://pi.dev");
        assert_eq!(snapshot["x-openrouter-title"], "pi");
        assert_eq!(snapshot["x-openrouter-categories"], "cli-agent");
        assert_eq!(snapshot["authorization"], "[REDACTED]");
        assert!(snapshot.values().all(|value| !value.contains("mcode/")));
    }

    #[test]
    fn model_auth_is_blocked_but_explicit_overlay_is_last() {
        let mut model_headers = HeaderOverlay::new();
        model_headers
            .insert("authorization", "Bearer model-metadata")
            .unwrap();
        model_headers.insert("x-order", "model").unwrap();
        let provider_settings = ModelSettings {
            headers: model_headers,
            ..ModelSettings::default()
        };
        let mut explicit = HeaderOverlay::new();
        explicit.insert("authorization", "Bearer explicit").unwrap();
        explicit.insert("x-order", "explicit").unwrap();
        let provider = ProfileProvider::new(openrouter_profile(), "real-key")
            .unwrap()
            .with_provider_settings(provider_settings)
            .with_headers(explicit);
        let snapshot = provider.header_snapshot(&request(), None).unwrap();
        assert_eq!(snapshot["authorization"], "[REDACTED]");
        assert_eq!(snapshot["x-order"], "explicit");
    }

    #[test]
    fn opencode_session_is_explicit_and_validated() {
        let provider = ProfileProvider::new(opencode_profile(), "secret")
            .unwrap()
            .with_opencode_session("session-1")
            .unwrap();
        let snapshot = provider.header_snapshot(&request(), None).unwrap();
        assert_eq!(snapshot["x-opencode-client"], "pi");
        assert_eq!(snapshot["x-opencode-session"], "session-1");
        assert!(
            ProfileProvider::new(opencode_profile(), "secret")
                .unwrap()
                .with_opencode_session("bad\r\nvalue")
                .is_err()
        );
    }

    #[test]
    fn debug_does_not_expose_api_key() {
        let provider = ProfileProvider::new(openrouter_profile(), "super-secret-key").unwrap();
        let debug = format!("{provider:?}");
        assert!(!debug.contains("super-secret-key"));
        assert!(debug.contains("REDACTED"));
    }

    #[test]
    fn profile_rejects_an_unqueryable_environment_name() {
        for invalid in ["", "BAD=NAME", "BAD\0NAME"] {
            let profile = openai_profile().with_base_url_env(invalid);
            assert!(matches!(
                ProfileProvider::from_profile(profile),
                Err(LlmError::Config(_))
            ));
        }
    }
}

// Rust guideline compliant 2026-08-26

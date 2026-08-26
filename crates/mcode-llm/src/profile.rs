//! Provider profiles, authentication schemes, and layered model settings.
//!
//! Profiles describe endpoint and policy data only. They serialize as ordinary
//! JSON with credential environment references, never credential values. Wire
//! serializers and SSE decoders live in protocol modules, so one adapter can
//! serve many providers without provider-name checks.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use mcode_core::message::{ReplayDomain, ReplayWire};
use reqwest::Url;
use reqwest::header::{HeaderName, HeaderValue};
use serde::{Deserialize, Deserializer, Serialize};

use crate::error::LlmError;

/// Wire protocol spoken by a provider endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WireKind {
    /// OpenAI-compatible `POST /chat/completions`.
    OpenAiChatCompletions,
    /// OpenAI `POST /responses`.
    OpenAiResponses,
    /// Anthropic `POST /v1/messages`.
    AnthropicMessages,
}

impl WireKind {
    /// Returns the path appended to a profile base URL.
    pub fn endpoint_path(self) -> &'static str {
        match self {
            Self::OpenAiChatCompletions => "chat/completions",
            Self::OpenAiResponses => "responses",
            Self::AnthropicMessages => "v1/messages",
        }
    }
}

/// HTTP authentication header scheme.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthScheme {
    /// `Authorization: Bearer <key>`.
    Bearer,
    /// `x-api-key: <key>`.
    XApiKey,
    /// No authentication header.
    None,
}

/// Authentication metadata containing only a credential reference.
///
/// This type is safe for ordinary JSON because `env` names a source and never
/// stores the credential value itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthProfile {
    /// Header scheme used for credentials.
    pub scheme: AuthScheme,
    /// Environment variable consulted for a key, when authentication is used.
    ///
    /// Profile validation rejects names that `std::env` cannot query safely.
    pub env: Option<String>,
}

impl AuthProfile {
    /// Creates bearer authentication using `env`.
    pub fn bearer(env: impl Into<String>) -> Self {
        Self {
            scheme: AuthScheme::Bearer,
            env: Some(env.into()),
        }
    }

    /// Creates `x-api-key` authentication using `env`.
    pub fn x_api_key(env: impl Into<String>) -> Self {
        Self {
            scheme: AuthScheme::XApiKey,
            env: Some(env.into()),
        }
    }

    /// Creates a profile that sends no authentication header.
    pub fn none() -> Self {
        Self {
            scheme: AuthScheme::None,
            env: None,
        }
    }
}

/// Built-in non-secret header and client-identity policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HeaderProfile {
    /// Pi-compatible user-agent only.
    Pi,
    /// Pi identity plus OpenRouter attribution headers.
    OpenRouter,
    /// Pi identity plus `x-opencode-client: pi`.
    OpenCode,
}

/// Coarse capabilities shared by all models unless corrected per model.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderCapabilities {
    /// Whether the provider supports tool calls.
    pub tools: bool,
    /// Whether the provider can stream reasoning content.
    pub thinking: bool,
    /// Whether the provider accepts image input.
    pub images: bool,
}

/// A validated case-insensitive HTTP header overlay.
///
/// Names are normalized to lowercase. Its `Debug` implementation redacts
/// authentication headers, and JSON serialization rejects them. Credentials
/// must instead use [`ApiKey`] through an explicit or environment boundary.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct HeaderOverlay(BTreeMap<String, String>);

impl HeaderOverlay {
    /// Creates an empty overlay.
    pub fn new() -> Self {
        Self::default()
    }

    /// Validates and inserts one header, replacing any case-insensitive match.
    ///
    /// # Errors
    ///
    /// Returns [`LlmError::Config`] for an invalid name or value.
    pub fn insert(
        &mut self,
        name: impl AsRef<str>,
        value: impl Into<String>,
    ) -> Result<Option<String>, LlmError> {
        let name = HeaderName::from_bytes(name.as_ref().as_bytes())
            .map_err(|_| LlmError::Config("invalid HTTP header name".into()))?;
        let value = value.into();
        HeaderValue::from_str(&value)
            .map_err(|_| LlmError::Config(format!("invalid value for header '{}'", name)))?;
        Ok(self.0.insert(name.as_str().to_owned(), value))
    }

    /// Builds an overlay from validated name/value pairs.
    ///
    /// # Errors
    ///
    /// Returns [`LlmError::Config`] when any pair is invalid.
    pub fn from_pairs<I, N, V>(pairs: I) -> Result<Self, LlmError>
    where
        I: IntoIterator<Item = (N, V)>,
        N: AsRef<str>,
        V: Into<String>,
    {
        let mut overlay = Self::new();
        for (name, value) in pairs {
            let _ = overlay.insert(name, value)?;
        }
        Ok(overlay)
    }

    /// Gets a header value by a case-insensitive name.
    pub fn get(&self, name: &str) -> Option<&str> {
        HeaderName::from_bytes(name.as_bytes())
            .ok()
            .and_then(|name| self.0.get(name.as_str()))
            .map(String::as_str)
    }

    /// Iterates over normalized names and raw values.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.0
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
    }

    /// Returns whether the overlay contains no headers.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Applies `higher` so its values take precedence.
    pub fn overlay(&mut self, higher: &Self) {
        self.0.extend(higher.0.clone());
    }

    /// Returns a diagnostic snapshot with credential headers redacted.
    pub fn redacted_snapshot(&self) -> BTreeMap<String, String> {
        self.0
            .iter()
            .map(|(name, value)| {
                let value = if is_auth_header(name) {
                    "[REDACTED]".to_owned()
                } else {
                    value.clone()
                };
                (name.clone(), value)
            })
            .collect()
    }
}

impl Serialize for HeaderOverlay {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        if self.0.keys().any(|name| is_auth_header(name)) {
            return Err(serde::ser::Error::custom(
                "credential headers cannot be stored in ordinary JSON configuration",
            ));
        }
        self.0.serialize(serializer)
    }
}

impl fmt::Debug for HeaderOverlay {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("HeaderOverlay")
            .field(&self.redacted_snapshot())
            .finish()
    }
}

impl<'de> Deserialize<'de> for HeaderOverlay {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = BTreeMap::<String, String>::deserialize(deserializer)?;
        if raw.keys().any(|name| is_auth_header(name)) {
            return Err(serde::de::Error::custom(
                "credential headers are not allowed in ordinary JSON configuration",
            ));
        }
        Self::from_pairs(raw).map_err(serde::de::Error::custom)
    }
}

/// A credential whose diagnostic formatting is always redacted.
#[derive(Clone, PartialEq, Eq)]
pub struct ApiKey(String);

impl ApiKey {
    /// Creates a non-empty credential safe for an HTTP header value.
    ///
    /// # Errors
    ///
    /// Returns [`LlmError::Config`] without echoing the value when validation
    /// fails.
    pub fn new(value: impl Into<String>) -> Result<Self, LlmError> {
        let value = value.into();
        if value.trim().is_empty() || HeaderValue::from_str(&value).is_err() {
            return Err(LlmError::Config("invalid or empty API key".into()));
        }
        Ok(Self(value))
    }

    pub(crate) fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ApiKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ApiKey([REDACTED])")
    }
}

/// Layerable model metadata and request settings.
///
/// Every `Some` value overrides the lower layer; headers merge by normalized
/// name. This same type represents catalog facts and higher-priority patches.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelSettings {
    /// Maximum total context tokens.
    pub context_window: Option<u64>,
    /// Maximum input tokens, when distinct from the context window.
    pub max_input_tokens: Option<u64>,
    /// Maximum generated tokens.
    pub max_output_tokens: Option<u64>,
    /// Whether tool calls are supported.
    pub tools: Option<bool>,
    /// Whether reasoning is supported.
    pub thinking: Option<bool>,
    /// Whether image input is supported.
    pub images: Option<bool>,
    /// Ordinary model-specific headers.
    ///
    /// Authentication headers in this layer are ignored when requests are
    /// assembled; use an explicit custom overlay when intentional.
    #[serde(default)]
    pub headers: HeaderOverlay,
}

impl ModelSettings {
    /// Applies a higher-priority settings layer.
    pub fn overlay(&mut self, higher: &Self) {
        if higher.context_window.is_some() {
            self.context_window = higher.context_window;
        }
        if higher.max_input_tokens.is_some() {
            self.max_input_tokens = higher.max_input_tokens;
        }
        if higher.max_output_tokens.is_some() {
            self.max_output_tokens = higher.max_output_tokens;
        }
        if higher.tools.is_some() {
            self.tools = higher.tools;
        }
        if higher.thinking.is_some() {
            self.thinking = higher.thinking;
        }
        if higher.images.is_some() {
            self.images = higher.images;
        }
        self.headers.overlay(&higher.headers);
    }
}

/// Named settings layers in ascending precedence order.
#[derive(Debug, Clone, Copy, Default)]
pub struct ModelLayers<'a> {
    /// Built-in or Pi per-provider catalog metadata.
    pub catalog: Option<&'a ModelSettings>,
    /// Provider-owned model corrections.
    pub provider_correction: Option<&'a ModelSettings>,
    /// User provider configuration.
    pub provider_config: Option<&'a ModelSettings>,
    /// Selected-model override.
    pub selection: Option<&'a ModelSettings>,
    /// One-call override.
    pub per_call: Option<&'a ModelSettings>,
}

/// Resolves model settings according to documented layer precedence.
pub fn resolve_model_settings(layers: ModelLayers<'_>) -> ModelSettings {
    let mut resolved = ModelSettings::default();
    for layer in [
        layers.catalog,
        layers.provider_correction,
        layers.provider_config,
        layers.selection,
        layers.per_call,
    ]
    .into_iter()
    .flatten()
    {
        resolved.overlay(layer);
    }
    resolved
}

/// Data-only provider profile consumed by reusable wire adapters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "UncheckedProviderProfile")]
pub struct ProviderProfile {
    id: String,
    wire: WireKind,
    base_url: String,
    base_url_env: Option<String>,
    auth: AuthProfile,
    header_profile: HeaderProfile,
    headers: HeaderOverlay,
    capabilities: ProviderCapabilities,
    model_corrections: BTreeMap<String, ModelSettings>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    trusted_replay: BTreeSet<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UncheckedProviderProfile {
    id: String,
    wire: WireKind,
    base_url: String,
    #[serde(default)]
    base_url_env: Option<String>,
    auth: AuthProfile,
    #[serde(default = "default_header_profile")]
    header_profile: HeaderProfile,
    #[serde(default)]
    headers: HeaderOverlay,
    #[serde(default)]
    capabilities: ProviderCapabilities,
    #[serde(default)]
    model_corrections: BTreeMap<String, ModelSettings>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    trusted_replay: BTreeSet<String>,
}

impl TryFrom<UncheckedProviderProfile> for ProviderProfile {
    type Error = LlmError;

    fn try_from(unchecked: UncheckedProviderProfile) -> Result<Self, Self::Error> {
        validate_profile_id(&unchecked.id)?;
        validate_optional_environment_name(unchecked.base_url_env.as_deref(), "base URL")?;
        validate_auth(&unchecked.auth)?;
        validate_trusted_replay(&unchecked.trusted_replay)?;
        Ok(Self {
            id: unchecked.id,
            wire: unchecked.wire,
            base_url: normalize_base_url(unchecked.base_url)?,
            base_url_env: unchecked.base_url_env,
            auth: unchecked.auth,
            header_profile: unchecked.header_profile,
            headers: unchecked.headers,
            capabilities: unchecked.capabilities,
            model_corrections: unchecked.model_corrections,
            trusted_replay: unchecked.trusted_replay,
        })
    }
}

impl ProviderProfile {
    /// Creates a validated profile with Pi identity and no extra headers.
    ///
    /// # Errors
    ///
    /// Returns [`LlmError::Config`] for an invalid id, URL, environment
    /// reference, or auth profile.
    pub fn new(
        id: impl Into<String>,
        wire: WireKind,
        base_url: impl Into<String>,
        auth: AuthProfile,
    ) -> Result<Self, LlmError> {
        let id = id.into();
        validate_profile_id(&id)?;
        validate_auth(&auth)?;
        let base_url = normalize_base_url(base_url.into())?;
        Ok(Self {
            id,
            wire,
            base_url,
            base_url_env: None,
            auth,
            header_profile: HeaderProfile::Pi,
            headers: HeaderOverlay::new(),
            capabilities: ProviderCapabilities::default(),
            model_corrections: BTreeMap::new(),
            trusted_replay: BTreeSet::new(),
        })
    }

    /// Validates all security-sensitive profile fields.
    ///
    /// # Errors
    ///
    /// Returns [`LlmError::Config`] for an invalid id, URL, environment
    /// reference, or auth profile.
    pub fn validate(&self) -> Result<(), LlmError> {
        validate_profile_id(&self.id)?;
        validate_optional_environment_name(self.base_url_env.as_deref(), "base URL")?;
        validate_auth(&self.auth)?;
        let _ = normalize_base_url(self.base_url.clone())?;
        validate_trusted_replay(&self.trusted_replay)?;
        if self.headers.iter().any(|(name, _)| is_auth_header(name)) {
            return Err(LlmError::Config(
                "provider profile JSON cannot contain credential headers".into(),
            ));
        }
        Ok(())
    }

    /// Returns the stable provider id.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Returns the selected wire protocol.
    pub fn wire(&self) -> WireKind {
        self.wire
    }

    /// Returns the normalized base URL without a trailing slash.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Returns the optional environment variable for a base-URL override.
    pub fn base_url_env(&self) -> Option<&str> {
        self.base_url_env.as_deref()
    }

    /// Returns authentication metadata.
    pub fn auth(&self) -> &AuthProfile {
        &self.auth
    }

    /// Returns the built-in header policy.
    pub fn header_profile(&self) -> HeaderProfile {
        self.header_profile
    }

    /// Returns profile-level static headers.
    pub fn headers(&self) -> &HeaderOverlay {
        &self.headers
    }

    /// Returns coarse provider capabilities.
    pub fn capabilities(&self) -> ProviderCapabilities {
        self.capabilities
    }

    /// Returns the correction for `model`, when one is registered.
    pub fn model_correction(&self, model: &str) -> Option<&ModelSettings> {
        self.model_corrections.get(model)
    }

    /// Returns all model corrections keyed by exact model id.
    pub fn model_corrections(&self) -> &BTreeMap<String, ModelSettings> {
        &self.model_corrections
    }

    /// Returns the producing profile ids whose replay state this profile
    /// may replay verbatim in addition to its own.
    pub fn trusted_replay_providers(&self) -> &BTreeSet<String> {
        &self.trusted_replay
    }

    /// Returns the replay trust domain enforced for this profile: verbatim
    /// replay only for state this profile produced on its current
    /// endpoint, plus explicitly trusted gateway producers. Wire adapters
    /// consult it before reusing opaque replay payloads, so switching
    /// profiles — or repointing a profile id at a different host through a
    /// base-URL override — never ships them to an unrelated endpoint.
    pub fn replay_domain(&self) -> ReplayDomain {
        ReplayDomain {
            wire: replay_wire(self.wire),
            provider: self.id.clone(),
            endpoint: self.replay_endpoint(),
            trusted: self.trusted_replay.iter().cloned().collect(),
        }
    }

    /// Returns the origin (`scheme://host[:port]`) of the effective base
    /// URL that replay trust binds to. The base URL already includes any
    /// environment override applied when the provider was constructed, so
    /// a redirected profile id reports the redirected origin and stops
    /// verbatim-replaying state recorded on the previous endpoint.
    pub(crate) fn replay_endpoint(&self) -> String {
        endpoint_origin(&self.base_url)
    }

    /// Replaces the base URL after validation.
    ///
    /// # Errors
    ///
    /// Returns [`LlmError::Config`] for an unsafe or malformed URL.
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Result<Self, LlmError> {
        self.base_url = normalize_base_url(base_url.into())?;
        Ok(self)
    }

    /// Sets the environment variable used for a base-URL override.
    ///
    /// [`Self::validate`] rejects names that `std::env` cannot query safely.
    pub fn with_base_url_env(mut self, env: impl Into<String>) -> Self {
        self.base_url_env = Some(env.into());
        self
    }

    /// Replaces authentication metadata.
    ///
    /// # Errors
    ///
    /// Returns [`LlmError::Config`] when an environment-variable name is
    /// missing or cannot be queried safely.
    pub fn with_auth(mut self, auth: AuthProfile) -> Result<Self, LlmError> {
        validate_auth(&auth)?;
        self.auth = auth;
        Ok(self)
    }

    /// Selects a built-in header policy.
    pub fn with_header_profile(mut self, profile: HeaderProfile) -> Self {
        self.header_profile = profile;
        self
    }

    /// Replaces profile-level static headers.
    pub fn with_headers(mut self, headers: HeaderOverlay) -> Self {
        self.headers = headers;
        self
    }

    /// Replaces coarse provider capabilities.
    pub fn with_capabilities(mut self, capabilities: ProviderCapabilities) -> Self {
        self.capabilities = capabilities;
        self
    }

    /// Adds or replaces one exact-model correction.
    pub fn with_model_correction(
        mut self,
        model: impl Into<String>,
        correction: ModelSettings,
    ) -> Self {
        self.model_corrections.insert(model.into(), correction);
        self
    }

    /// Explicitly trusts one producing profile id for verbatim replay of
    /// its wire-only state (a gateway known to share this profile's
    /// backend). This is the sole way replay state crosses a profile
    /// boundary.
    ///
    /// # Errors
    ///
    /// Returns [`LlmError::Config`] when `provider` is not a valid
    /// profile id.
    pub fn with_trusted_replay_provider(
        mut self,
        provider: impl AsRef<str>,
    ) -> Result<Self, LlmError> {
        let provider = provider.as_ref();
        validate_profile_id(provider)?;
        self.trusted_replay.insert(provider.to_owned());
        Ok(self)
    }

    /// Returns the complete endpoint URL for this profile's wire protocol.
    pub fn endpoint(&self) -> String {
        format!("{}/{}", self.base_url, self.wire.endpoint_path())
    }
}

/// Returns built-in profiles for generic OpenAI, OpenAI, Anthropic, DeepSeek,
/// OpenRouter, and OpenCode.
pub fn builtin_profiles() -> Vec<ProviderProfile> {
    vec![
        generic_openai_profile(),
        openai_profile(),
        anthropic_profile(),
        deepseek_profile(),
        openrouter_profile(),
        opencode_profile(),
    ]
}

/// Returns the generic OpenAI-compatible chat-completions profile.
pub fn generic_openai_profile() -> ProviderProfile {
    builtin_profile(
        "generic-openai",
        WireKind::OpenAiChatCompletions,
        "https://api.openai.com/v1",
        AuthProfile::bearer("OPENAI_API_KEY"),
    )
    .with_base_url_env("OPENAI_BASE_URL")
    .with_capabilities(all_capabilities())
}

/// Returns the first-party OpenAI Responses profile.
pub fn openai_profile() -> ProviderProfile {
    builtin_profile(
        "openai",
        WireKind::OpenAiResponses,
        "https://api.openai.com/v1",
        AuthProfile::bearer("OPENAI_API_KEY"),
    )
    .with_base_url_env("OPENAI_BASE_URL")
    .with_capabilities(all_capabilities())
}

/// Returns the Anthropic Messages profile.
pub fn anthropic_profile() -> ProviderProfile {
    let headers = HeaderOverlay::from_pairs([("anthropic-version", "2023-06-01")])
        .expect("built-in Anthropic header must be valid");
    builtin_profile(
        "anthropic",
        WireKind::AnthropicMessages,
        "https://api.anthropic.com",
        AuthProfile::x_api_key("ANTHROPIC_API_KEY"),
    )
    .with_base_url_env("ANTHROPIC_BASE_URL")
    .with_headers(headers)
    .with_capabilities(all_capabilities())
}

/// Returns the DeepSeek OpenAI-compatible profile.
pub fn deepseek_profile() -> ProviderProfile {
    builtin_profile(
        "deepseek",
        WireKind::OpenAiChatCompletions,
        "https://api.deepseek.com",
        AuthProfile::bearer("DEEPSEEK_API_KEY"),
    )
    .with_base_url_env("DEEPSEEK_BASE_URL")
    .with_capabilities(ProviderCapabilities {
        tools: true,
        thinking: true,
        images: false,
    })
    .with_model_correction(
        "deepseek-reasoner",
        ModelSettings {
            thinking: Some(true),
            images: Some(false),
            ..ModelSettings::default()
        },
    )
}

/// Returns the OpenRouter OpenAI-compatible profile and attribution headers.
pub fn openrouter_profile() -> ProviderProfile {
    builtin_profile(
        "openrouter",
        WireKind::OpenAiChatCompletions,
        "https://openrouter.ai/api/v1",
        AuthProfile::bearer("OPENROUTER_API_KEY"),
    )
    .with_base_url_env("OPENROUTER_BASE_URL")
    .with_header_profile(HeaderProfile::OpenRouter)
    .with_capabilities(all_capabilities())
}

/// Returns the OpenCode OpenAI-compatible profile.
pub fn opencode_profile() -> ProviderProfile {
    builtin_profile(
        "opencode",
        WireKind::OpenAiChatCompletions,
        "https://opencode.ai/zen/v1",
        AuthProfile::bearer("OPENCODE_API_KEY"),
    )
    .with_base_url_env("OPENCODE_BASE_URL")
    .with_header_profile(HeaderProfile::OpenCode)
    .with_capabilities(all_capabilities())
}

fn builtin_profile(id: &str, wire: WireKind, base_url: &str, auth: AuthProfile) -> ProviderProfile {
    ProviderProfile::new(id, wire, base_url, auth).expect("built-in provider profile must be valid")
}

fn default_header_profile() -> HeaderProfile {
    HeaderProfile::Pi
}

fn all_capabilities() -> ProviderCapabilities {
    ProviderCapabilities {
        tools: true,
        thinking: true,
        images: true,
    }
}

pub(crate) fn validate_profile_id(id: &str) -> Result<(), LlmError> {
    let mut characters = id.chars();
    let starts_valid = characters
        .next()
        .is_some_and(|character| character.is_ascii_alphanumeric());
    let rest_valid = characters
        .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.'));
    if !starts_valid || !rest_valid {
        return Err(LlmError::Config(format!(
            "invalid provider profile id '{id}'"
        )));
    }
    Ok(())
}

fn validate_auth(auth: &AuthProfile) -> Result<(), LlmError> {
    validate_optional_environment_name(auth.env.as_deref(), "authentication")?;
    match auth.scheme {
        AuthScheme::None => Ok(()),
        AuthScheme::Bearer | AuthScheme::XApiKey if auth.env.is_none() => Err(LlmError::Config(
            "authenticated provider profile requires an auth env name".into(),
        )),
        AuthScheme::Bearer | AuthScheme::XApiKey => Ok(()),
    }
}

/// Validates an optional environment-variable name before any OS lookup.
fn validate_optional_environment_name(name: Option<&str>, purpose: &str) -> Result<(), LlmError> {
    let Some(name) = name else {
        return Ok(());
    };
    if name.is_empty() || name.as_bytes().contains(&b'=') || name.as_bytes().contains(&b'\0') {
        return Err(LlmError::Config(format!(
            "{purpose} environment variable name must not be empty or contain '=' or NUL"
        )));
    }
    Ok(())
}

fn normalize_base_url(raw: String) -> Result<String, LlmError> {
    let raw = raw.trim().trim_end_matches('/');
    let parsed =
        Url::parse(raw).map_err(|_| LlmError::Config("invalid provider base URL".into()))?;
    if !matches!(parsed.scheme(), "http" | "https")
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err(LlmError::Config(
            "provider base URL must be an http(s) origin/path without credentials, query, or fragment"
                .into(),
        ));
    }
    Ok(raw.to_owned())
}

fn validate_trusted_replay(trusted: &BTreeSet<String>) -> Result<(), LlmError> {
    for provider in trusted {
        validate_profile_id(provider)?;
    }
    Ok(())
}

fn replay_wire(kind: WireKind) -> ReplayWire {
    match kind {
        WireKind::OpenAiChatCompletions => ReplayWire::OpenAiChatCompletions,
        WireKind::OpenAiResponses => ReplayWire::OpenAiResponses,
        WireKind::AnthropicMessages => ReplayWire::AnthropicMessages,
    }
}

/// Returns the origin (`scheme://host[:port]`) of a normalized base URL.
///
/// Both the recorded producer provenance and the consuming domain derive
/// origins through this one function, so equality comparisons never
/// misfire over default ports or equivalent spellings.
fn endpoint_origin(base_url: &str) -> String {
    Url::parse(base_url)
        .map(|url| url.origin().ascii_serialization())
        .unwrap_or_else(|_| base_url.to_owned())
}

pub(crate) fn is_auth_header(name: &str) -> bool {
    let normalized = name.to_ascii_lowercase().replace(['-', '_'], "");
    normalized.ends_with("authorization")
        || normalized.ends_with("apikey")
        || normalized.ends_with("authkey")
        || normalized.ends_with("accesskey")
        || normalized.ends_with("subscriptionkey")
        || normalized == "token"
        || normalized.ends_with("token")
        || normalized.contains("secret")
        || normalized.contains("credential")
        || normalized.contains("password")
        || matches!(normalized.as_str(), "cookie" | "setcookie")
}

#[cfg(test)]
mod tests {
    use super::*;
    use mcode_core::message::ReplayState;

    #[test]
    fn api_key_debug_is_redacted() {
        let key = ApiKey::new("sk-secret-value").unwrap();
        let debug = format!("{key:?}");
        assert!(debug.contains("REDACTED"));
        assert!(!debug.contains("sk-secret-value"));
    }

    #[test]
    fn header_overlay_normalizes_and_redacts_auth() {
        let overlay = HeaderOverlay::from_pairs([
            ("X-Test", "first"),
            ("x-test", "second"),
            ("Authorization", "Bearer secret"),
        ])
        .unwrap();
        assert_eq!(overlay.get("X-TEST"), Some("second"));
        assert_eq!(overlay.redacted_snapshot()["authorization"], "[REDACTED]");
        assert!(!format!("{overlay:?}").contains("Bearer secret"));
        assert!(serde_json::to_string(&overlay).is_err());
        for name in [
            "authorization",
            "api-key",
            "x-api-key",
            "x-auth-token",
            "ocp-apim-subscription-key",
            "x-client-secret",
            "x-oauth-credential",
            "x-session-password",
            "cookie",
        ] {
            let encoded = serde_json::json!({(name): "ordinary-json-secret"});
            assert!(
                serde_json::from_value::<HeaderOverlay>(encoded).is_err(),
                "credential header {name} must be rejected"
            );
        }

        let credential_overlay = HeaderOverlay::from_pairs([
            ("api-key", "api-secret"),
            ("x-auth-token", "token-secret"),
        ])
        .unwrap();
        let debug = format!("{credential_overlay:?}");
        assert!(!debug.contains("api-secret"));
        assert!(!debug.contains("token-secret"));
        assert!(serde_json::to_string(&credential_overlay).is_err());
    }

    #[test]
    fn profile_json_contains_only_credential_references() {
        let profile = openai_profile();
        let json = serde_json::to_string(&profile).unwrap();
        assert!(json.contains("OPENAI_API_KEY"));
        assert!(!json.contains("Bearer"));
        assert!(!json.contains("sk-"));

        let mut invalid = serde_json::to_value(&profile).unwrap();
        invalid["api_key"] = serde_json::Value::String("must-not-be-configured".into());
        assert!(serde_json::from_value::<ProviderProfile>(invalid).is_err());
    }

    #[test]
    fn rejects_header_and_url_injection() {
        assert!(HeaderOverlay::from_pairs([("x-test\r\nboom", "x")]).is_err());
        assert!(HeaderOverlay::from_pairs([("x-test", "ok\r\nboom")]).is_err());
        assert!(
            ProviderProfile::new(
                "bad/id",
                WireKind::OpenAiChatCompletions,
                "https://example.com",
                AuthProfile::none(),
            )
            .is_err()
        );
        assert!(
            ProviderProfile::new(
                "safe",
                WireKind::OpenAiChatCompletions,
                "https://user:secret@example.com/v1",
                AuthProfile::none(),
            )
            .is_err()
        );
        let serialized = serde_json::json!({
            "id": "safe",
            "wire": "open_ai_chat_completions",
            "base_url": "https://user:secret@example.com/v1",
            "auth": {"scheme": "none", "env": null}
        });
        assert!(serde_json::from_value::<ProviderProfile>(serialized).is_err());
    }

    #[test]
    fn profile_serde_roundtrip_revalidates_fields() {
        let profile = openrouter_profile();
        let serialized = serde_json::to_value(&profile).unwrap();
        let roundtrip: ProviderProfile = serde_json::from_value(serialized).unwrap();
        assert_eq!(roundtrip, profile);
    }

    #[test]
    fn rejects_environment_names_that_std_env_cannot_query() {
        for invalid in ["", "BAD=NAME", "BAD\0NAME"] {
            let mut base_url_profile = serde_json::to_value(openai_profile()).unwrap();
            base_url_profile["base_url_env"] = serde_json::json!(invalid);
            assert!(
                serde_json::from_value::<ProviderProfile>(base_url_profile).is_err(),
                "accepted invalid base URL environment name {invalid:?}"
            );

            let mut auth_profile = serde_json::to_value(openai_profile()).unwrap();
            auth_profile["auth"]["env"] = serde_json::json!(invalid);
            assert!(
                serde_json::from_value::<ProviderProfile>(auth_profile).is_err(),
                "accepted invalid authentication environment name {invalid:?}"
            );
        }

        let invalid = openai_profile().with_base_url_env("BAD=NAME");
        assert!(matches!(invalid.validate(), Err(LlmError::Config(_))));
    }

    #[test]
    fn layer_precedence_and_header_overlay_are_stable() {
        let mut low_headers = HeaderOverlay::new();
        low_headers.insert("x-layer", "catalog").unwrap();
        let catalog = ModelSettings {
            max_output_tokens: Some(100),
            thinking: Some(false),
            headers: low_headers,
            ..ModelSettings::default()
        };
        let correction = ModelSettings {
            max_output_tokens: Some(200),
            ..ModelSettings::default()
        };
        let provider_config = ModelSettings {
            thinking: Some(true),
            ..ModelSettings::default()
        };
        let selection = ModelSettings {
            max_output_tokens: Some(300),
            ..ModelSettings::default()
        };
        let mut call_headers = HeaderOverlay::new();
        call_headers.insert("X-Layer", "call").unwrap();
        let per_call = ModelSettings {
            max_output_tokens: Some(400),
            headers: call_headers,
            ..ModelSettings::default()
        };
        let resolved = resolve_model_settings(ModelLayers {
            catalog: Some(&catalog),
            provider_correction: Some(&correction),
            provider_config: Some(&provider_config),
            selection: Some(&selection),
            per_call: Some(&per_call),
        });
        assert_eq!(resolved.max_output_tokens, Some(400));
        assert_eq!(resolved.thinking, Some(true));
        assert_eq!(resolved.headers.get("x-layer"), Some("call"));
    }

    #[test]
    fn trusted_replay_gate_is_explicit_and_serializes_as_plain_json() {
        let base = anthropic_profile();
        // Default domain: only the profile itself on its own endpoint.
        assert!(base.trusted_replay_providers().is_empty());
        let domain = base.replay_domain();
        assert_eq!(domain.provider, "anthropic");
        assert_eq!(domain.wire, ReplayWire::AnthropicMessages);
        assert_eq!(domain.endpoint, "https://api.anthropic.com");
        assert!(domain.trusted.is_empty());

        // A base-URL override (as environment resolution applies one) keeps the id but
        // moves the trust domain to the redirected origin, so state
        // recorded on the previous endpoint no longer replays verbatim.
        let redirected = base
            .clone()
            .with_base_url("https://mirror.example")
            .unwrap();
        let redirected_domain = redirected.replay_domain();
        assert_eq!(redirected_domain.provider, "anthropic");
        assert_eq!(redirected_domain.endpoint, "https://mirror.example");
        assert!(
            ReplayState::new(ReplayWire::AnthropicMessages, "sig")
                .with_provider("anthropic")
                .with_endpoint("https://api.anthropic.com")
                .is_replayable_on(&domain)
        );
        assert!(
            !ReplayState::new(ReplayWire::AnthropicMessages, "sig")
                .with_provider("anthropic")
                .with_endpoint("https://api.anthropic.com")
                .is_replayable_on(&redirected_domain)
        );

        // Explicit trust extends the domain and round-trips as data.
        let gateway = base
            .clone()
            .with_trusted_replay_provider("anthropic-direct")
            .unwrap();
        assert!(
            gateway
                .replay_domain()
                .trusted
                .contains(&"anthropic-direct".to_owned())
        );
        let serialized = serde_json::to_value(&gateway).unwrap();
        assert_eq!(
            serde_json::from_value::<ProviderProfile>(serialized).unwrap(),
            gateway
        );

        // Invalid producer ids are rejected in builders and JSON.
        assert!(base.clone().with_trusted_replay_provider("bad/id").is_err());
        let mut invalid = serde_json::to_value(&base).unwrap();
        invalid["trusted_replay"] = serde_json::json!(["also bad"]);
        assert!(serde_json::from_value::<ProviderProfile>(invalid).is_err());
    }

    #[test]
    fn builtins_select_protocols_and_identity_profiles() {
        assert_eq!(
            generic_openai_profile().wire(),
            WireKind::OpenAiChatCompletions
        );
        assert_eq!(openai_profile().wire(), WireKind::OpenAiResponses);
        assert_eq!(anthropic_profile().wire(), WireKind::AnthropicMessages);
        assert_eq!(deepseek_profile().wire(), WireKind::OpenAiChatCompletions);
        assert_eq!(
            openrouter_profile().header_profile(),
            HeaderProfile::OpenRouter
        );
        assert_eq!(opencode_profile().header_profile(), HeaderProfile::OpenCode);
        assert_eq!(
            anthropic_profile().headers().get("anthropic-version"),
            Some("2023-06-01")
        );
    }
}

// Rust guideline compliant 2026-08-26

//! Tolerant models.dev catalog parsing, refresh, and offline cache support.
//!
//! The approximately four-megabyte upstream catalog is never embedded. MCode
//! ships only a tiny fallback and overlays a validated network/cache snapshot
//! when available. Cache paths and endpoints are injectable for local tests.

use std::collections::BTreeMap;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use reqwest::StatusCode;
use reqwest::header::{ACCEPT, ETAG, IF_NONE_MATCH, USER_AGENT};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio_stream::StreamExt;

use crate::error::LlmError;
use crate::identity::ClientIdentity;
use crate::profile::{HeaderOverlay, ModelSettings, WireKind, is_auth_header};

/// Public models.dev API endpoint.
pub const MODELS_DEV_URL: &str = "https://models.dev/api.json";

/// Default model id when a caller omits an explicit selection.
///
/// Built-in Anthropic and DeepSeek profiles keep their catalog ids so a
/// `--provider` switch does not inherit OpenAI's `gpt-4o-mini`, which those
/// official endpoints reject. OpenRouter catalog ids are `provider/model`
/// (`openai/gpt-4o-mini`); a bare `gpt-4o-mini` is not a valid OpenRouter id.
/// Unknown profile ids fall back by wire kind.
pub fn default_model_id(provider_id: &str, wire: WireKind) -> &'static str {
    match provider_id {
        "anthropic" => "claude-sonnet-4-5",
        "deepseek" => "deepseek-chat",
        "openrouter" => "openai/gpt-4o-mini",
        "openai" | "generic-openai" | "opencode" => "gpt-4o-mini",
        _ => match wire {
            WireKind::AnthropicMessages => "claude-sonnet-4-5",
            WireKind::OpenAiChatCompletions | WireKind::OpenAiResponses => "gpt-4o-mini",
        },
    }
}

/// Short total timeout so catalog refresh cannot delay basic startup for long.
const DEFAULT_REFRESH_TIMEOUT: Duration = Duration::from_secs(3);
/// Short connection timeout; stale cache remains usable after failure.
const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(1);
/// The upstream file is roughly four MiB; this allows growth without unbounded
/// allocation from an untrusted server or cache file.
const MAX_CATALOG_BYTES: usize = 8 * 1_024 * 1_024;
/// Version of MCode's cache envelope, independent from models.dev's schema.
const CACHE_VERSION: u32 = 1;

/// One model served by one catalog provider.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogModel {
    /// Provider-local model id used on requests.
    pub id: String,
    /// Human-readable display name.
    pub name: String,
    /// Provider-neutral limits and capabilities.
    pub settings: ModelSettings,
}

/// One models.dev provider entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogProvider {
    /// Stable provider id.
    pub id: String,
    /// Human-readable display name.
    pub name: String,
    /// Models keyed by provider-local model id.
    pub models: BTreeMap<String, CatalogModel>,
}

/// Parsed model catalog keyed by provider id.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelCatalog {
    providers: BTreeMap<String, CatalogProvider>,
}

impl ModelCatalog {
    /// Creates an empty catalog.
    pub fn new() -> Self {
        Self::default()
    }

    /// Parses a models.dev API JSON document tolerantly.
    ///
    /// Unknown fields are ignored and common numeric strings are accepted.
    ///
    /// # Errors
    ///
    /// Returns [`LlmError::Config`] for invalid JSON, a non-object root, or a
    /// document containing no usable model entries.
    pub fn from_models_dev(bytes: impl AsRef<[u8]>) -> Result<Self, LlmError> {
        let value: Value = serde_json::from_slice(bytes.as_ref())
            .map_err(|error| LlmError::Config(format!("invalid models.dev JSON: {error}")))?;
        parse_catalog_value(&value)
    }

    /// Returns the tiny built-in fallback catalog.
    pub fn fallback() -> Self {
        let mut catalog = Self::new();
        for (provider_id, provider_name, model_id, model_name, settings) in [
            (
                "openai",
                "OpenAI",
                "gpt-4o-mini",
                "GPT-4o mini",
                ModelSettings {
                    context_window: Some(128_000),
                    max_output_tokens: Some(16_384),
                    tools: Some(true),
                    thinking: Some(false),
                    images: Some(true),
                    ..ModelSettings::default()
                },
            ),
            (
                "anthropic",
                "Anthropic",
                "claude-sonnet-4-5",
                "Claude Sonnet 4.5",
                ModelSettings {
                    context_window: Some(200_000),
                    max_output_tokens: Some(8_192),
                    tools: Some(true),
                    thinking: Some(true),
                    images: Some(true),
                    ..ModelSettings::default()
                },
            ),
            (
                "deepseek",
                "DeepSeek",
                "deepseek-chat",
                "DeepSeek Chat",
                ModelSettings {
                    context_window: Some(64_000),
                    max_output_tokens: Some(8_192),
                    tools: Some(true),
                    thinking: Some(false),
                    images: Some(false),
                    ..ModelSettings::default()
                },
            ),
            (
                "deepseek",
                "DeepSeek",
                "deepseek-reasoner",
                "DeepSeek Reasoner",
                ModelSettings {
                    context_window: Some(64_000),
                    max_output_tokens: Some(8_192),
                    tools: Some(true),
                    thinking: Some(true),
                    images: Some(false),
                    ..ModelSettings::default()
                },
            ),
            (
                "openrouter",
                "OpenRouter",
                "openai/gpt-4o-mini",
                "GPT-4o mini",
                ModelSettings {
                    context_window: Some(128_000),
                    max_output_tokens: Some(16_384),
                    tools: Some(true),
                    thinking: Some(false),
                    images: Some(true),
                    ..ModelSettings::default()
                },
            ),
        ] {
            catalog.insert_model(
                provider_id,
                provider_name,
                CatalogModel {
                    id: model_id.into(),
                    name: model_name.into(),
                    settings,
                },
            );
        }
        catalog
    }

    /// Looks up a provider.
    pub fn provider(&self, id: &str) -> Option<&CatalogProvider> {
        self.providers.get(id)
    }

    /// Looks up a model by provider and provider-local id.
    pub fn model(&self, provider_id: &str, model_id: &str) -> Option<&CatalogModel> {
        self.provider(provider_id)?.models.get(model_id)
    }

    /// Iterates over providers in stable lexical order.
    pub fn providers(&self) -> impl Iterator<Item = &CatalogProvider> {
        self.providers.values()
    }

    /// Returns the total number of model entries.
    pub fn model_count(&self) -> usize {
        self.providers
            .values()
            .map(|provider| provider.models.len())
            .sum()
    }

    /// Overlays `higher` onto this catalog.
    ///
    /// Existing model settings merge field-by-field so sparse tolerant input
    /// does not erase lower-level fallback facts.
    pub fn overlay(&mut self, higher: Self) {
        for (provider_id, higher_provider) in higher.providers {
            let provider = self
                .providers
                .entry(provider_id.clone())
                .or_insert_with(|| CatalogProvider {
                    id: provider_id,
                    name: higher_provider.name.clone(),
                    models: BTreeMap::new(),
                });
            provider.name = higher_provider.name;
            for (model_id, higher_model) in higher_provider.models {
                match provider.models.get_mut(&model_id) {
                    Some(model) => {
                        model.name = higher_model.name;
                        model.settings.overlay(&higher_model.settings);
                    }
                    None => {
                        provider.models.insert(model_id, higher_model);
                    }
                }
            }
        }
    }

    fn insert_model(&mut self, provider_id: &str, provider_name: &str, model: CatalogModel) {
        self.providers
            .entry(provider_id.to_owned())
            .or_insert_with(|| CatalogProvider {
                id: provider_id.to_owned(),
                name: provider_name.to_owned(),
                models: BTreeMap::new(),
            })
            .models
            .insert(model.id.clone(), model);
    }
}

/// Origin of a loaded catalog snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogOrigin {
    /// Tiny built-in fallback only.
    BuiltInFallback,
    /// Valid offline/stale cache.
    Cache,
    /// Fresh network response.
    Network,
    /// Server returned HTTP 304 and the cache was reused.
    NotModified,
}

/// Catalog plus refresh metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogSnapshot {
    /// Merged fallback and loaded catalog.
    pub catalog: ModelCatalog,
    /// Source selected after fallback handling.
    pub origin: CatalogOrigin,
    /// Cached/server ETag, when supplied.
    pub etag: Option<String>,
}

/// Refresh client with injectable endpoint, cache path, timeout, and identity.
#[derive(Clone)]
pub struct CatalogClient {
    client: reqwest::Client,
    endpoint: String,
    cache_path: PathBuf,
    timeout: Duration,
    offline: bool,
    identity: ClientIdentity,
}

impl std::fmt::Debug for CatalogClient {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CatalogClient")
            .field("endpoint", &self.endpoint)
            .field("cache_path", &self.cache_path)
            .field("timeout", &self.timeout)
            .field("offline", &self.offline)
            .field("identity", &self.identity)
            .finish_non_exhaustive()
    }
}

impl CatalogClient {
    /// Creates a client using models.dev and the injected cache path.
    pub fn new(cache_path: impl Into<PathBuf>) -> Self {
        let client = reqwest::Client::builder()
            .connect_timeout(DEFAULT_CONNECT_TIMEOUT)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self {
            client,
            endpoint: MODELS_DEV_URL.into(),
            cache_path: cache_path.into(),
            timeout: DEFAULT_REFRESH_TIMEOUT,
            offline: false,
            identity: ClientIdentity::system_pi(),
        }
    }

    /// Replaces the catalog endpoint after URL validation.
    ///
    /// # Errors
    ///
    /// Returns [`LlmError::Config`] for non-http(s), credential-bearing, or
    /// otherwise malformed URLs.
    pub fn with_endpoint(mut self, endpoint: impl Into<String>) -> Result<Self, LlmError> {
        let endpoint = endpoint.into();
        let parsed = reqwest::Url::parse(&endpoint)
            .map_err(|_| LlmError::Config("invalid model catalog URL".into()))?;
        if !matches!(parsed.scheme(), "http" | "https")
            || parsed.host_str().is_none()
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.query().is_some()
            || parsed.fragment().is_some()
        {
            return Err(LlmError::Config(
                "model catalog URL must be http(s) without credentials, query, or fragment".into(),
            ));
        }
        self.endpoint = endpoint;
        Ok(self)
    }

    /// Sets a total network refresh timeout.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Enables or disables network access.
    pub fn with_offline(mut self, offline: bool) -> Self {
        self.offline = offline;
        self
    }

    /// Replaces the default Pi-compatible request identity.
    pub fn with_identity(mut self, identity: ClientIdentity) -> Self {
        self.identity = identity;
        self
    }

    /// Returns the injected cache path.
    pub fn cache_path(&self) -> &Path {
        &self.cache_path
    }

    /// Loads cache/fallback and attempts a conditional network refresh.
    ///
    /// Network, HTTP, parse, and cache-write failures never fail this method;
    /// the last valid cache or built-in fallback is returned instead.
    pub async fn load(&self) -> CatalogSnapshot {
        let cached = read_cache(&self.cache_path);
        if self.offline {
            return cached_snapshot(cached);
        }

        let mut request = self
            .client
            .get(&self.endpoint)
            .header(ACCEPT, "application/json")
            .header(USER_AGENT, self.identity.user_agent())
            .timeout(self.timeout);
        if let Some(etag) = cached.as_ref().and_then(|cache| cache.etag.as_deref()) {
            request = request.header(IF_NONE_MATCH, etag);
        }

        let response = match request.send().await {
            Ok(response) => response,
            Err(_) => return cached_snapshot(cached),
        };
        if response.status() == StatusCode::NOT_MODIFIED {
            return match cached {
                Some(cache) => snapshot_from_cache(cache, CatalogOrigin::NotModified),
                None => fallback_snapshot(),
            };
        }
        if !response.status().is_success() {
            return cached_snapshot(cached);
        }

        let etag = response
            .headers()
            .get(ETAG)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let body = match bounded_body(response).await {
            Ok(body) => body,
            Err(_) => return cached_snapshot(cached),
        };
        let value: Value = match serde_json::from_slice(&body) {
            Ok(value) => value,
            Err(_) => return cached_snapshot(cached),
        };
        let remote = match parse_catalog_value(&value) {
            Ok(catalog) => catalog,
            Err(_) => return cached_snapshot(cached),
        };

        let envelope = CacheEnvelope {
            version: CACHE_VERSION,
            etag: etag.clone(),
            body: value,
        };
        // A cache failure must not discard a valid network response.
        let _ = write_cache_atomic(&self.cache_path, &envelope);
        let mut catalog = ModelCatalog::fallback();
        catalog.overlay(remote);
        CatalogSnapshot {
            catalog,
            origin: CatalogOrigin::Network,
            etag,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CacheEnvelope {
    version: u32,
    #[serde(default)]
    etag: Option<String>,
    body: Value,
}

struct ValidCache {
    catalog: ModelCatalog,
    etag: Option<String>,
}

fn parse_catalog_value(value: &Value) -> Result<ModelCatalog, LlmError> {
    let root = value
        .as_object()
        .ok_or_else(|| LlmError::Config("models.dev catalog root must be an object".into()))?;
    let provider_root = root
        .get("providers")
        .and_then(Value::as_object)
        .unwrap_or(root);
    let mut catalog = ModelCatalog::new();
    for (provider_key, provider_value) in provider_root {
        let Some(provider) = provider_value.as_object() else {
            continue;
        };
        let provider_id = provider
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or(provider_key);
        let provider_name = provider
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or(provider_id);
        let Some(models) = provider.get("models") else {
            continue;
        };
        if let Some(models) = models.as_object() {
            for (model_key, model_value) in models {
                if let Some(model) = parse_model(model_key, model_value) {
                    catalog.insert_model(provider_id, provider_name, model);
                }
            }
        } else if let Some(models) = models.as_array() {
            for model_value in models {
                let model_key = model_value
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if let Some(model) = parse_model(model_key, model_value) {
                    catalog.insert_model(provider_id, provider_name, model);
                }
            }
        }
    }
    if catalog.model_count() == 0 {
        return Err(LlmError::Config(
            "models.dev catalog contains no usable models".into(),
        ));
    }
    Ok(catalog)
}

fn parse_model(fallback_id: &str, value: &Value) -> Option<CatalogModel> {
    let object = value.as_object()?;
    let id = object
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or(fallback_id);
    if id.is_empty() {
        return None;
    }
    let name = object.get("name").and_then(Value::as_str).unwrap_or(id);
    let limit = object.get("limit").and_then(Value::as_object);
    let modalities = object.get("modalities").and_then(Value::as_object);
    let input_modalities = modalities
        .and_then(|modalities| modalities.get("input"))
        .and_then(Value::as_array);
    let mut headers = HeaderOverlay::new();
    if let Some(raw_headers) = object.get("headers").and_then(Value::as_object) {
        for (header, value) in raw_headers {
            if is_auth_header(header) {
                continue;
            }
            if let Some(value) = value.as_str() {
                // Tolerant parsing drops an invalid optional header instead of
                // rejecting a four-megabyte catalog snapshot.
                let _ = headers.insert(header, value.to_owned());
            }
        }
    }
    let attachment = optional_bool(object.get("attachment"));
    let images = input_modalities.map_or(attachment, |modalities| {
        Some(
            modalities
                .iter()
                .any(|value| value.as_str() == Some("image")),
        )
    });
    Some(CatalogModel {
        id: id.to_owned(),
        name: name.to_owned(),
        settings: ModelSettings {
            context_window: limit
                .and_then(|limit| limit.get("context"))
                .and_then(tolerant_u64)
                .or_else(|| object.get("context_window").and_then(tolerant_u64)),
            max_input_tokens: limit
                .and_then(|limit| limit.get("input"))
                .and_then(tolerant_u64),
            max_output_tokens: limit
                .and_then(|limit| limit.get("output"))
                .and_then(tolerant_u64)
                .or_else(|| object.get("max_output_tokens").and_then(tolerant_u64)),
            tools: optional_bool(object.get("tool_call")),
            thinking: optional_bool(object.get("reasoning")),
            images,
            headers,
        },
    })
}

fn optional_bool(value: Option<&Value>) -> Option<bool> {
    match value {
        Some(Value::Bool(value)) => Some(*value),
        Some(Value::String(value)) if value.eq_ignore_ascii_case("true") => Some(true),
        Some(Value::String(value)) if value.eq_ignore_ascii_case("false") => Some(false),
        _ => None,
    }
}

fn tolerant_u64(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
}

fn read_cache(path: &Path) -> Option<ValidCache> {
    let metadata = std::fs::metadata(path).ok()?;
    if metadata.len() > (MAX_CATALOG_BYTES as u64) + 1_024 * 1_024 {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    let value: Value = serde_json::from_slice(&bytes).ok()?;
    if let Ok(envelope) = serde_json::from_value::<CacheEnvelope>(value.clone()) {
        if envelope.version != CACHE_VERSION {
            return None;
        }
        let catalog = parse_catalog_value(&envelope.body).ok()?;
        return Some(ValidCache {
            catalog,
            etag: envelope.etag,
        });
    }
    // Accept a raw models.dev snapshot as a migration/import convenience.
    let catalog = parse_catalog_value(&value).ok()?;
    Some(ValidCache {
        catalog,
        etag: None,
    })
}

fn cached_snapshot(cache: Option<ValidCache>) -> CatalogSnapshot {
    match cache {
        Some(cache) => snapshot_from_cache(cache, CatalogOrigin::Cache),
        None => fallback_snapshot(),
    }
}

fn snapshot_from_cache(cache: ValidCache, origin: CatalogOrigin) -> CatalogSnapshot {
    let mut catalog = ModelCatalog::fallback();
    catalog.overlay(cache.catalog);
    CatalogSnapshot {
        catalog,
        origin,
        etag: cache.etag,
    }
}

fn fallback_snapshot() -> CatalogSnapshot {
    CatalogSnapshot {
        catalog: ModelCatalog::fallback(),
        origin: CatalogOrigin::BuiltInFallback,
        etag: None,
    }
}

async fn bounded_body(response: reqwest::Response) -> Result<Vec<u8>, LlmError> {
    let mut stream = response.bytes_stream();
    let mut body = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| LlmError::Transport(error.without_url().to_string()))?;
        if body.len().saturating_add(chunk.len()) > MAX_CATALOG_BYTES {
            return Err(LlmError::Config(format!(
                "models.dev response exceeds {MAX_CATALOG_BYTES} bytes"
            )));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn write_cache_atomic(path: &Path, envelope: &CacheEnvelope) -> io::Result<()> {
    let mut envelope = envelope.clone();
    strip_credentials(&mut envelope.body);
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty());
    if let Some(parent) = parent {
        std::fs::create_dir_all(parent)?;
    }
    let directory = parent.unwrap_or_else(|| Path::new("."));
    let mut temporary = tempfile::NamedTempFile::new_in(directory)?;
    serde_json::to_writer(temporary.as_file_mut(), &envelope).map_err(io::Error::other)?;
    temporary.as_file_mut().flush()?;
    temporary.as_file().sync_all()?;
    temporary
        .into_temp_path()
        .persist(path)
        .map_err(|error| error.error)
}

fn strip_credentials(value: &mut Value) {
    match value {
        Value::Object(object) => {
            object.retain(|name, _| !is_secret_json_field(name));
            if let Some(Value::Object(headers)) = object.get_mut("headers") {
                headers.retain(|name, _| !is_auth_header(name));
            }
            for value in object.values_mut() {
                strip_credentials(value);
            }
        }
        Value::Array(values) => {
            for value in values {
                strip_credentials(value);
            }
        }
        _ => {}
    }
}

fn is_secret_json_field(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().replace(['-', '_'], "").as_str(),
        "apikey" | "authorization" | "accesstoken" | "refreshtoken" | "oauthtoken" | "clientsecret"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::WireKind;
    use serde_json::json;

    fn sample_catalog(context: Value) -> Value {
        json!({
            "sample": {
                "id": "sample",
                "name": "Sample Provider",
                "unknown_provider_field": true,
                "api_key": "must-not-enter-config",
                "models": {
                    "model-a": {
                        "id": "model-a",
                        "name": "Model A",
                        "reasoning": true,
                        "tool_call": "true",
                        "attachment": false,
                        "limit": {
                            "context": context,
                            "input": 120,
                            "output": "42"
                        },
                        "modalities": {"input": ["text", "image"], "output": ["text"]},
                        "headers": {
                            "x-model-feature": "enabled",
                            "authorization": "Bearer must-not-enter-config"
                        },
                        "unknown_model_field": {"anything": true}
                    }
                }
            }
        })
    }

    #[test]
    fn parses_models_dev_tolerantly() {
        let catalog =
            ModelCatalog::from_models_dev(sample_catalog(json!("1000")).to_string()).unwrap();
        let model = catalog.model("sample", "model-a").unwrap();
        assert_eq!(model.settings.context_window, Some(1_000));
        assert_eq!(model.settings.max_input_tokens, Some(120));
        assert_eq!(model.settings.max_output_tokens, Some(42));
        assert_eq!(model.settings.tools, Some(true));
        assert_eq!(model.settings.thinking, Some(true));
        assert_eq!(model.settings.images, Some(true));
        assert_eq!(
            model.settings.headers.get("x-model-feature"),
            Some("enabled")
        );
        assert!(model.settings.headers.get("authorization").is_none());
        assert!(serde_json::to_string(&catalog).is_ok());
    }

    #[test]
    fn invalid_or_empty_catalog_is_rejected() {
        assert!(ModelCatalog::from_models_dev(b"not json").is_err());
        assert!(ModelCatalog::from_models_dev(b"{}").is_err());
        assert!(ModelCatalog::from_models_dev(b"[]").is_err());
    }

    #[test]
    fn fallback_is_tiny_and_has_core_models() {
        let fallback = ModelCatalog::fallback();
        assert!(fallback.model_count() <= 8);
        assert!(fallback.model("openai", "gpt-4o-mini").is_some());
        assert!(fallback.model("anthropic", "claude-sonnet-4-5").is_some());
        assert!(fallback.model("deepseek", "deepseek-chat").is_some());
        assert!(fallback.model("openrouter", "openai/gpt-4o-mini").is_some());
    }

    #[test]
    fn default_model_id_matches_fallback_catalog_and_wire() {
        let fallback = ModelCatalog::fallback();
        assert_eq!(
            default_model_id("openai", WireKind::OpenAiResponses),
            "gpt-4o-mini"
        );
        assert_eq!(
            default_model_id("anthropic", WireKind::AnthropicMessages),
            "claude-sonnet-4-5"
        );
        assert_eq!(
            default_model_id("deepseek", WireKind::OpenAiChatCompletions),
            "deepseek-chat"
        );
        assert_eq!(
            default_model_id("generic-openai", WireKind::OpenAiChatCompletions),
            "gpt-4o-mini"
        );
        assert_eq!(
            default_model_id("openrouter", WireKind::OpenAiChatCompletions),
            "openai/gpt-4o-mini"
        );
        for (provider, wire) in [
            ("openai", WireKind::OpenAiResponses),
            ("anthropic", WireKind::AnthropicMessages),
            ("deepseek", WireKind::OpenAiChatCompletions),
            ("openrouter", WireKind::OpenAiChatCompletions),
        ] {
            let id = default_model_id(provider, wire);
            assert!(
                fallback.model(provider, id).is_some(),
                "{provider} {id} missing from fallback catalog"
            );
        }
        assert_eq!(
            default_model_id("my-claude", WireKind::AnthropicMessages),
            "claude-sonnet-4-5"
        );
    }

    #[test]
    fn atomic_cache_replaces_old_file_and_remains_parseable() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("catalog.json");
        std::fs::write(&path, "old").unwrap();
        let envelope = CacheEnvelope {
            version: CACHE_VERSION,
            etag: Some("\"etag-1\"".into()),
            body: sample_catalog(json!(2048)),
        };
        write_cache_atomic(&path, &envelope).unwrap();
        let persisted = std::fs::read_to_string(&path).unwrap();
        assert!(!persisted.contains("must-not-enter-config"));
        assert!(!persisted.contains("authorization"));
        assert!(!persisted.contains("api_key"));
        let cache = read_cache(&path).expect("valid replacement cache");
        assert_eq!(cache.etag.as_deref(), Some("\"etag-1\""));
        assert_eq!(
            cache
                .catalog
                .model("sample", "model-a")
                .unwrap()
                .settings
                .context_window,
            Some(2_048)
        );
        let entries: Vec<_> = std::fs::read_dir(directory.path())
            .unwrap()
            .flatten()
            .collect();
        assert_eq!(entries.len(), 1, "temporary file must be atomically moved");
    }

    #[tokio::test]
    async fn offline_uses_valid_cache_and_corrupt_cache_uses_fallback() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("catalog.json");
        let envelope = CacheEnvelope {
            version: CACHE_VERSION,
            etag: Some("\"cached\"".into()),
            body: sample_catalog(json!(4096)),
        };
        write_cache_atomic(&path, &envelope).unwrap();
        let snapshot = CatalogClient::new(&path).with_offline(true).load().await;
        assert_eq!(snapshot.origin, CatalogOrigin::Cache);
        assert_eq!(snapshot.etag.as_deref(), Some("\"cached\""));
        assert!(snapshot.catalog.model("sample", "model-a").is_some());

        std::fs::write(&path, "corrupt").unwrap();
        let snapshot = CatalogClient::new(&path).with_offline(true).load().await;
        assert_eq!(snapshot.origin, CatalogOrigin::BuiltInFallback);
        assert!(snapshot.catalog.model("openai", "gpt-4o-mini").is_some());
    }
}

// Rust guideline compliant 2026-08-26

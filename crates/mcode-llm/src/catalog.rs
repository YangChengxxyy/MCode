//! Per-provider Pi remote catalog overlay with a tiny compiled fallback.
//!
//! Production traffic uses `https://pi.dev/api/models/providers/{id}` rather
//! than a full-catalog dump. Remote JSON is untrusted: it may update model
//! metadata only and cannot change a [`crate::ProviderProfile`] endpoint,
//! wire, trust domain, or credential destination. Cache files are
//! strict versioned envelopes written atomically in the destination
//! directory and bound to both provider id and catalog origin.

use std::collections::{BTreeMap, HashMap};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use reqwest::StatusCode;
use reqwest::header::{ACCEPT, ETAG, IF_NONE_MATCH, LAST_MODIFIED, USER_AGENT};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Number, Value};
use tokio::sync::Notify;
use tokio_stream::StreamExt;
use tokio_util::sync::CancellationToken;

use crate::error::LlmError;
use crate::identity::ClientIdentity;
use crate::profile::{ModelSettings, WireKind, validate_profile_id};
use crate::profile_provider::same_origin_redirect_policy;

/// Pi catalog origin. Per-provider paths are appended by [`CatalogClient`].
pub const PI_CATALOG_BASE_URL: &str = "https://pi.dev";

/// Stale-while-revalidate window matching Pi 0.84.3.
///
/// Changing this alters how often a successful cache is revalidated. Pi
/// uses four hours so catalog traffic stays off the startup path.
pub const REMOTE_CATALOG_REFRESH_INTERVAL: Duration = Duration::from_secs(4 * 60 * 60);

/// Hard cap applied to remote advertised context before it becomes
/// catalog settings.
///
/// This matches `mcode_compaction::trigger::HARD_MAX_WORKING_TOKENS`.
/// Remote catalogs must not raise the host working context above 400k;
/// user JSON overrides still win at a higher layer, and compaction
/// clamps independently.
pub const CATALOG_CONTEXT_CLAMP_TOKENS: u64 = 400_000;

/// Per-attempt HTTP timeout matching Pi 0.84.3 `attemptTimeoutMs`.
const DEFAULT_ATTEMPT_TIMEOUT: Duration = Duration::from_millis(4_000);
/// Covers three attempts of [`DEFAULT_ATTEMPT_TIMEOUT`].
const DEFAULT_TOTAL_DEADLINE: Duration = Duration::from_secs(12);
/// Pi `maxRetries` is 2, so three attempts including the first.
const DEFAULT_MAX_ATTEMPTS: u32 = 3;
/// Short connect timeout; stale cache remains usable after failure.
const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(1);
/// Per-provider JSON is small; this bounds an untrusted server or cache.
const MAX_CATALOG_BYTES: usize = 1_024 * 1_024;
/// Hard cap for a persisted cache file, including envelope overhead.
///
/// Catalog bodies are capped at [`MAX_CATALOG_BYTES`]. The extra 64 KiB
/// covers the versioned envelope (provider id, origin, ETag, digest).
/// Reads stop at `MAX_CACHE_BYTES + 1` so a file that grows after `fstat`
/// cannot force an unbounded allocation.
const MAX_CACHE_BYTES: usize = MAX_CATALOG_BYTES + 64 * 1_024;
/// Reject a provider document that lists more models than this.
const MAX_MODELS: usize = 512;
/// Maximum accepted model id length in UTF-8 bytes.
const MAX_ID_BYTES: usize = 128;
/// Maximum accepted display-name length in UTF-8 bytes.
const MAX_NAME_BYTES: usize = 256;
/// Maximum accepted informational wire-hint length in UTF-8 bytes.
const MAX_WIRE_HINT_BYTES: usize = 64;
/// Version of MCode's per-provider cache envelope.
///
/// Version 2 requires a normalized catalog origin so a shared cache
/// directory cannot reuse another origin's body or validator.
const CACHE_VERSION: u32 = 2;

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

/// Optional remote cost metadata. Values are untrusted display facts.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelCost {
    /// USD per million input tokens, when advertised.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<Number>,
    /// USD per million output tokens, when advertised.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<Number>,
    /// USD per million cache-read tokens, when advertised.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read: Option<Number>,
    /// USD per million cache-write tokens, when advertised.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write: Option<Number>,
}

/// One model served by one catalog provider.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogModel {
    /// Provider-local model id used on requests.
    pub id: String,
    /// Human-readable display name.
    pub name: String,
    /// Provider-neutral limits and capabilities.
    ///
    /// `context_window` is the clamped effective value, never a raw
    /// advertised figure above [`CATALOG_CONTEXT_CLAMP_TOKENS`].
    pub settings: ModelSettings,
    /// Advertised context before the 400k clamp, when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub advertised_context_window: Option<u64>,
    /// Optional untrusted cost metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<ModelCost>,
    /// Informational wire/API hint. Never applied to a profile.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wire_hint: Option<String>,
}

/// One catalog provider entry.
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

    /// Parses one provider's untrusted catalog JSON document.
    ///
    /// Accepts a model array, `{ "models": ... }`, or an object map.
    /// Unknown and trust-sensitive fields are ignored. The returned
    /// catalog contains only `provider_id`.
    ///
    /// # Errors
    ///
    /// Returns [`LlmError::Config`] for an invalid provider id, invalid
    /// JSON, or a document containing no usable model entries.
    pub fn from_provider_json(
        provider_id: impl AsRef<str>,
        bytes: impl AsRef<[u8]>,
    ) -> Result<Self, LlmError> {
        let provider_id = provider_id.as_ref();
        validate_profile_id(provider_id)?;
        let value: Value = serde_json::from_slice(bytes.as_ref())
            .map_err(|error| LlmError::Config(format!("invalid provider catalog JSON: {error}")))?;
        parse_provider_document(provider_id, &value)
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
                    advertised_context_window: None,
                    cost: None,
                    wire_hint: None,
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
                        if higher_model.advertised_context_window.is_some() {
                            model.advertised_context_window =
                                higher_model.advertised_context_window;
                        }
                        if higher_model.cost.is_some() {
                            model.cost = higher_model.cost;
                        }
                        if higher_model.wire_hint.is_some() {
                            model.wire_hint = higher_model.wire_hint;
                        }
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
    /// Valid cache selected without a successful network response.
    Cache,
    /// Fresh network response.
    Network,
    /// Server returned HTTP 304 and the cache was reused.
    NotModified,
    /// Remote reported no catalog for this provider (HTTP 404/501).
    Unsupported,
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
    /// Unix milliseconds of the last completed remote check, when known.
    pub checked_at: Option<u64>,
}

/// Options for an explicit catalog refresh.
#[derive(Debug, Clone)]
pub struct CatalogRefresh {
    /// Bypass the stale window when network access is allowed.
    pub force: bool,
    /// When false, return cache or fallback without network I/O.
    pub allow_network: bool,
    /// Stops this caller waiting for a shared refresh.
    ///
    /// The token does not cancel the shared network task. Retries and
    /// cache writes continue, and other waiters are not poisoned.
    pub cancel: CancellationToken,
}

impl Default for CatalogRefresh {
    fn default() -> Self {
        Self::new()
    }
}

impl CatalogRefresh {
    /// Returns options that allow network and honor the stale window.
    pub fn new() -> Self {
        Self {
            force: false,
            allow_network: true,
            cancel: CancellationToken::new(),
        }
    }

    /// Forces a network revalidation when network access is allowed.
    pub fn force() -> Self {
        Self {
            force: true,
            allow_network: true,
            cancel: CancellationToken::new(),
        }
    }
}

/// Refresh client with injectable origin, cache directory, and identity.
#[derive(Clone)]
pub struct CatalogClient {
    client: reqwest::Client,
    base_url: String,
    cache_dir: PathBuf,
    total_deadline: Duration,
    attempt_timeout: Duration,
    max_attempts: u32,
    refresh_interval: Duration,
    offline: bool,
    identity: ClientIdentity,
    inflight: Arc<Mutex<HashMap<String, Arc<SharedFetch>>>>,
}

struct SharedFetch {
    notify: Notify,
    result: Mutex<Option<CatalogSnapshot>>,
}

impl std::fmt::Debug for CatalogClient {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CatalogClient")
            .field("base_url", &self.base_url)
            .field("cache_dir", &self.cache_dir)
            .field("total_deadline", &self.total_deadline)
            .field("attempt_timeout", &self.attempt_timeout)
            .field("max_attempts", &self.max_attempts)
            .field("refresh_interval", &self.refresh_interval)
            .field("offline", &self.offline)
            .field("identity", &self.identity)
            .finish_non_exhaustive()
    }
}

impl CatalogClient {
    /// Creates a client using Pi's per-provider catalog and `cache_dir`.
    pub fn new(cache_dir: impl Into<PathBuf>) -> Self {
        let client = reqwest::Client::builder()
            .connect_timeout(DEFAULT_CONNECT_TIMEOUT)
            .redirect(same_origin_redirect_policy())
            .build()
            .unwrap_or_else(|_| {
                reqwest::Client::builder()
                    .redirect(reqwest::redirect::Policy::none())
                    .build()
                    .expect("HTTP client without redirects")
            });
        Self {
            client,
            base_url: PI_CATALOG_BASE_URL.into(),
            cache_dir: cache_dir.into(),
            total_deadline: DEFAULT_TOTAL_DEADLINE,
            attempt_timeout: DEFAULT_ATTEMPT_TIMEOUT,
            max_attempts: DEFAULT_MAX_ATTEMPTS,
            refresh_interval: REMOTE_CATALOG_REFRESH_INTERVAL,
            offline: false,
            identity: ClientIdentity::system_pi(),
            inflight: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Replaces the catalog origin after URL validation.
    ///
    /// The origin must be an http(s) URL without credentials, query,
    /// fragment, or extra path. Per-provider paths are appended.
    ///
    /// # Errors
    ///
    /// Returns [`LlmError::Config`] for a malformed or unsafe URL.
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Result<Self, LlmError> {
        self.base_url = validate_catalog_base_url(base_url.into())?;
        Ok(self)
    }

    /// Sets the total network refresh deadline covering every attempt.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.total_deadline = timeout;
        self
    }

    /// Sets the per-attempt HTTP timeout.
    pub fn with_attempt_timeout(mut self, timeout: Duration) -> Self {
        self.attempt_timeout = timeout;
        self
    }

    /// Sets the attempt budget, clamped to `1..=8`.
    pub fn with_max_attempts(mut self, attempts: u32) -> Self {
        self.max_attempts = attempts.clamp(1, 8);
        self
    }

    /// Sets the stale-while-revalidate window.
    pub fn with_refresh_interval(mut self, interval: Duration) -> Self {
        self.refresh_interval = interval;
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

    /// Returns the injected cache directory.
    pub fn cache_dir(&self) -> &Path {
        &self.cache_dir
    }

    /// Returns the cache file for `provider_id` at this client's origin.
    ///
    /// The default Pi origin keeps `{provider}.json`. Other origins add a
    /// short digest so two catalogs cannot share a file.
    pub fn cache_path(&self, provider_id: &str) -> Option<PathBuf> {
        validate_profile_id(provider_id).ok().map(|_| {
            self.cache_dir
                .join(cache_file_name(provider_id, &self.base_url))
        })
    }

    /// Returns fallback overlaid with last valid cache, without network.
    pub fn snapshot(&self, provider_id: &str) -> CatalogSnapshot {
        if validate_profile_id(provider_id).is_err() {
            return fallback_snapshot();
        }
        cached_snapshot(self.read_cache(provider_id))
    }

    /// When network access is enabled, fetches only when no valid cache exists.
    ///
    /// Network, HTTP, parse, and cache-write failures never fail this
    /// method; the last valid cache or built-in fallback is returned.
    pub async fn ensure_fetched(&self, provider_id: &str) -> CatalogSnapshot {
        if validate_profile_id(provider_id).is_err() {
            return fallback_snapshot();
        }
        if let Some(cache) = self.read_cache(provider_id) {
            return snapshot_from_cache(cache);
        }
        self.refresh(provider_id, CatalogRefresh::new()).await
    }

    /// Returns cache immediately. With network access enabled, fetches only when no valid cache exists.
    ///
    /// A stale cache is returned at once and revalidated in the background.
    pub async fn load_lazy(&self, provider_id: &str) -> CatalogSnapshot {
        let snapshot = self.snapshot(provider_id);
        if self.offline {
            return snapshot;
        }
        if snapshot.origin == CatalogOrigin::BuiltInFallback {
            return self.ensure_fetched(provider_id).await;
        }
        if self.cache_is_stale(snapshot.checked_at) {
            let client = self.clone();
            let provider_id = provider_id.to_owned();
            tokio::spawn(async move {
                let _ = client.refresh(&provider_id, CatalogRefresh::new()).await;
            });
        }
        snapshot
    }

    /// Loads cache/fallback and refreshes according to [`CatalogRefresh`].
    ///
    /// Failures never fail this method. [`CatalogRefresh::cancel`] only
    /// stops this caller waiting; a shared fetch keeps retrying and
    /// writing cache.
    pub async fn refresh(&self, provider_id: &str, options: CatalogRefresh) -> CatalogSnapshot {
        if validate_profile_id(provider_id).is_err() {
            return fallback_snapshot();
        }
        let cached = self.read_cache(provider_id);
        if self.offline || !options.allow_network {
            return cached_snapshot(cached);
        }
        if options.cancel.is_cancelled() {
            return cached_snapshot(cached);
        }
        if !options.force
            && let Some(cache) = cached.as_ref()
            && !self.cache_is_stale(Some(cache.checked_at))
        {
            return snapshot_from_cache(cache.clone());
        }
        self.singleflight_network(provider_id, cached, options)
            .await
    }

    /// With network access enabled, refreshes cache when missing, invalid, or stale; otherwise returns cache/fallback.
    pub async fn load(&self, provider_id: &str) -> CatalogSnapshot {
        self.refresh(provider_id, CatalogRefresh::new()).await
    }

    fn cache_is_stale(&self, checked_at: Option<u64>) -> bool {
        let Some(checked_at) = checked_at else {
            return true;
        };
        now_millis().saturating_sub(checked_at) >= millis(self.refresh_interval)
    }

    fn read_cache(&self, provider_id: &str) -> Option<ValidCache> {
        let path = self.cache_path(provider_id)?;
        read_cache(&path, provider_id, &self.base_url)
    }

    async fn singleflight_network(
        &self,
        provider_id: &str,
        cached: Option<ValidCache>,
        options: CatalogRefresh,
    ) -> CatalogSnapshot {
        let fallback = cached_snapshot(cached.clone());
        let key = inflight_key(&self.base_url, provider_id);
        let (shared, is_leader) = {
            let mut map = self.inflight.lock().expect("catalog inflight lock");
            if let Some(existing) = map.get(&key) {
                (Arc::clone(existing), false)
            } else {
                let shared = Arc::new(SharedFetch {
                    notify: Notify::new(),
                    result: Mutex::new(None),
                });
                map.insert(key.clone(), Arc::clone(&shared));
                (shared, true)
            }
        };
        if is_leader {
            // The shared task uses a fresh token. A caller's token only
            // stops that caller waiting; network retries and cache writes
            // continue. Leader cancellation therefore cannot publish
            // fallback to other waiters.
            let client = self.clone();
            let provider_id = provider_id.to_owned();
            let guard = InflightGuard {
                shared: Arc::clone(&shared),
                inflight: Arc::clone(&self.inflight),
                key,
                fallback: fallback.clone(),
                published: false,
            };
            tokio::spawn(async move {
                let snapshot = client
                    .network_refresh(&provider_id, cached, &CancellationToken::new())
                    .await;
                guard.publish(snapshot);
            });
        }
        wait_for_shared(shared, &options.cancel, fallback).await
    }

    async fn network_refresh(
        &self,
        provider_id: &str,
        cached: Option<ValidCache>,
        cancel: &CancellationToken,
    ) -> CatalogSnapshot {
        let url = match provider_catalog_url(&self.base_url, provider_id) {
            Ok(url) => url,
            Err(_) => return cached_snapshot(cached),
        };
        let validator = cached.as_ref().and_then(|cache| {
            if cache.empty {
                None
            } else {
                cache.etag.as_deref()
            }
        });
        let fetched = match self.send_with_retry(&url, validator, cancel).await {
            Ok(fetched) => fetched,
            Err(LlmError::Cancelled) => return cached_snapshot(cached),
            Err(_) => return cached_snapshot(cached),
        };
        if cancel.is_cancelled() {
            return cached_snapshot(cached);
        }
        let checked_at = now_millis();
        let status = fetched.status;
        if status == StatusCode::NOT_MODIFIED {
            return match cached {
                Some(cache) if !cache.empty => {
                    let mut updated = cache.clone();
                    updated.checked_at = checked_at;
                    let _ = self.persist(provider_id, &updated);
                    let mut snapshot = snapshot_from_cache(updated);
                    snapshot.origin = CatalogOrigin::NotModified;
                    snapshot
                }
                Some(cache) => snapshot_from_cache(cache),
                None => fallback_snapshot(),
            };
        }
        if status == StatusCode::NOT_FOUND || status == StatusCode::NOT_IMPLEMENTED {
            let empty = ValidCache {
                catalog: ModelCatalog::new(),
                etag: None,
                last_modified: None,
                checked_at,
                empty: true,
            };
            let _ = self.persist(provider_id, &empty);
            return snapshot_from_cache(empty);
        }
        if !status.is_success() {
            return match cached {
                Some(cache) => {
                    let mut updated = cache;
                    updated.checked_at = checked_at;
                    let _ = self.persist(provider_id, &updated);
                    snapshot_from_cache(updated)
                }
                None => fallback_snapshot(),
            };
        }

        let etag = fetched.etag;
        let last_modified = fetched.last_modified;
        let value: Value = match serde_json::from_slice(&fetched.body) {
            Ok(value) => value,
            Err(_) => return cached_snapshot(cached),
        };
        let remote = match parse_provider_document(provider_id, &value) {
            Ok(catalog) => catalog,
            Err(_) => return cached_snapshot(cached),
        };
        let cache = ValidCache {
            catalog: remote,
            etag,
            last_modified,
            checked_at,
            empty: false,
        };
        let _ = self.persist(provider_id, &cache);
        let mut snapshot = snapshot_from_cache(cache);
        snapshot.origin = CatalogOrigin::Network;
        snapshot
    }

    async fn send_with_retry(
        &self,
        url: &reqwest::Url,
        validator: Option<&str>,
        cancel: &CancellationToken,
    ) -> Result<FetchedDocument, LlmError> {
        let deadline = tokio::time::Instant::now() + self.total_deadline;
        let attempts = self.max_attempts.max(1);
        let mut last_error = LlmError::Timeout;
        for attempt in 0..attempts {
            if cancel.is_cancelled() {
                return Err(LlmError::Cancelled);
            }
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return Err(LlmError::Timeout);
            }
            let attempt_timeout = self.attempt_timeout.min(remaining);
            let attempt_deadline = tokio::time::Instant::now() + attempt_timeout;
            let mut request = self
                .client
                .get(url.clone())
                .header(ACCEPT, "application/json")
                .header(USER_AGENT, self.identity.user_agent())
                .timeout(attempt_timeout);
            if let Some(etag) = validator {
                request = request.header(IF_NONE_MATCH, etag);
            }
            let result = tokio::select! {
                biased;
                () = cancel.cancelled() => Err(LlmError::Cancelled),
                () = tokio::time::sleep(remaining) => Err(LlmError::Timeout),
                response = request.send() => match response {
                    Ok(response) => Ok(response),
                    Err(error) if error.is_timeout() => Err(LlmError::Timeout),
                    Err(error) => Err(LlmError::Transport(error.without_url().to_string())),
                },
            };
            let response = match result {
                Ok(response)
                    if should_retry_status(response.status()) && attempt + 1 < attempts =>
                {
                    last_error = LlmError::Http {
                        status: response.status().as_u16(),
                        body: String::new(),
                    };
                    continue;
                }
                Ok(response) => response,
                Err(error) => {
                    last_error = error;
                    if next_retry(attempt, attempts, deadline, &last_error) {
                        continue;
                    }
                    return Err(give_up_error(attempt, attempts, deadline, last_error));
                }
            };
            let status = response.status();
            let etag = header_string(response.headers().get(ETAG));
            let last_modified = header_string(response.headers().get(LAST_MODIFIED));
            if !status.is_success() {
                return Ok(FetchedDocument {
                    status,
                    etag,
                    last_modified,
                    body: Vec::new(),
                });
            }
            let body_budget =
                attempt_deadline.saturating_duration_since(tokio::time::Instant::now());
            if body_budget.is_zero() {
                last_error = LlmError::Timeout;
                if next_retry(attempt, attempts, deadline, &last_error) {
                    continue;
                }
                return Err(give_up_error(attempt, attempts, deadline, last_error));
            }
            let body = tokio::select! {
                biased;
                () = cancel.cancelled() => Err(LlmError::Cancelled),
                () = tokio::time::sleep(body_budget) => Err(LlmError::Timeout),
                body = bounded_body(response, cancel) => body,
            };
            match body {
                Ok(body) => {
                    return Ok(FetchedDocument {
                        status,
                        etag,
                        last_modified,
                        body,
                    });
                }
                Err(error) => {
                    last_error = error;
                    if next_retry(attempt, attempts, deadline, &last_error) {
                        continue;
                    }
                    return Err(give_up_error(attempt, attempts, deadline, last_error));
                }
            }
        }
        Err(last_error)
    }

    fn persist(&self, provider_id: &str, cache: &ValidCache) -> io::Result<()> {
        let Some(path) = self.cache_path(provider_id) else {
            return Ok(());
        };
        write_cache_atomic(&path, &cache_envelope(provider_id, &self.base_url, cache))
    }
}

struct FetchedDocument {
    status: StatusCode,
    etag: Option<String>,
    last_modified: Option<String>,
    body: Vec<u8>,
}

struct InflightGuard {
    shared: Arc<SharedFetch>,
    inflight: Arc<Mutex<HashMap<String, Arc<SharedFetch>>>>,
    key: String,
    fallback: CatalogSnapshot,
    published: bool,
}

impl InflightGuard {
    /// Detaches this flight, then publishes so waiters cannot return
    /// while a later force refresh could still join this snapshot.
    fn publish(mut self, snapshot: CatalogSnapshot) {
        self.store(Some(snapshot));
    }

    fn store(&mut self, snapshot: Option<CatalogSnapshot>) {
        {
            let mut map = self.inflight.lock().expect("catalog inflight lock");
            if map
                .get(&self.key)
                .is_some_and(|current| Arc::ptr_eq(current, &self.shared))
            {
                map.remove(&self.key);
            }
            let mut result = self
                .shared
                .result
                .lock()
                .expect("catalog fetch result lock");
            if result.is_none() {
                *result = Some(snapshot.unwrap_or_else(|| self.fallback.clone()));
            }
        }
        self.published = true;
        self.shared.notify.notify_waiters();
    }
}

impl Drop for InflightGuard {
    fn drop(&mut self) {
        if self.published {
            return;
        }
        self.store(None);
    }
}

/// Returns the shared snapshot, or `fallback` if this caller cancels.
///
/// Cancellation does not stop the shared fetch or other waiters.
async fn wait_for_shared(
    shared: Arc<SharedFetch>,
    cancel: &CancellationToken,
    fallback: CatalogSnapshot,
) -> CatalogSnapshot {
    loop {
        let notified = shared.notify.notified();
        if let Some(snapshot) = shared
            .result
            .lock()
            .expect("catalog fetch result lock")
            .clone()
        {
            return snapshot;
        }
        tokio::select! {
            biased;
            () = cancel.cancelled() => return fallback,
            () = notified => {}
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CacheEnvelope {
    version: u32,
    provider_id: String,
    origin: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    etag: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_modified: Option<String>,
    checked_at: u64,
    digest: String,
    models: Vec<CachedModel>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CachedModel {
    id: String,
    name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cost: Option<ModelCost>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    advertised_context_window: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    context_window: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max_input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max_output_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tools: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    thinking: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    images: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    wire_hint: Option<String>,
}

#[derive(Debug, Clone)]
struct ValidCache {
    catalog: ModelCatalog,
    etag: Option<String>,
    last_modified: Option<String>,
    checked_at: u64,
    empty: bool,
}

fn parse_provider_document(provider_id: &str, value: &Value) -> Result<ModelCatalog, LlmError> {
    let entries = collect_model_entries(value);
    if entries.len() > MAX_MODELS {
        return Err(LlmError::Config(format!(
            "provider catalog lists more than {MAX_MODELS} models"
        )));
    }
    let mut catalog = ModelCatalog::new();
    for (fallback_id, model_value) in entries {
        if let Some(model) = parse_model(provider_id, &fallback_id, model_value) {
            catalog.insert_model(provider_id, provider_id, model);
        }
    }
    if catalog.model_count() == 0 {
        return Err(LlmError::Config(
            "provider catalog contains no usable models".into(),
        ));
    }
    Ok(catalog)
}

fn collect_model_entries(value: &Value) -> Vec<(String, &Value)> {
    match value {
        Value::Array(items) => items
            .iter()
            .filter(|item| item.is_object())
            .map(|item| {
                let id = item
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned();
                (id, item)
            })
            .collect(),
        Value::Object(map) => {
            if let Some(models) = map.get("models") {
                return collect_model_entries(models);
            }
            let mut out = Vec::new();
            collect_object_models(map, &mut out, 0);
            out
        }
        _ => Vec::new(),
    }
}

fn collect_object_models<'a>(
    map: &'a Map<String, Value>,
    out: &mut Vec<(String, &'a Value)>,
    depth: u8,
) {
    if out.len() > MAX_MODELS {
        return;
    }
    for (key, value) in map {
        let Some(object) = value.as_object() else {
            continue;
        };
        if looks_like_model(object) {
            let id = object
                .get("id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
                .unwrap_or(key);
            out.push((id.to_owned(), value));
        } else if depth < 2 {
            collect_object_models(object, out, depth + 1);
        }
        if out.len() > MAX_MODELS {
            return;
        }
    }
}

fn looks_like_model(object: &Map<String, Value>) -> bool {
    if object.contains_key("models") {
        return false;
    }
    object
        .get("id")
        .and_then(Value::as_str)
        .is_some_and(|id| !id.is_empty())
        || object.contains_key("contextWindow")
        || object.contains_key("context_window")
        || object.contains_key("limit")
        || object.contains_key("maxTokens")
        || object.contains_key("max_tokens")
        || object.contains_key("reasoning")
        || object.contains_key("cost")
}

fn parse_model(provider_id: &str, fallback_id: &str, value: &Value) -> Option<CatalogModel> {
    let object = value.as_object()?;
    if let Some(remote_provider) = object.get("provider").and_then(Value::as_str)
        && remote_provider != provider_id
    {
        return None;
    }
    let id = object
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .unwrap_or(fallback_id);
    if id.is_empty() || id.len() > MAX_ID_BYTES {
        return None;
    }
    let name = object.get("name").and_then(Value::as_str).unwrap_or(id);
    if name.len() > MAX_NAME_BYTES {
        return None;
    }
    let limit = object.get("limit").and_then(Value::as_object);
    let advertised = limit
        .and_then(|limit| limit.get("context"))
        .and_then(tolerant_u64)
        .or_else(|| object.get("contextWindow").and_then(tolerant_u64))
        .or_else(|| object.get("context_window").and_then(tolerant_u64));
    let max_input_tokens = limit
        .and_then(|limit| limit.get("input"))
        .and_then(tolerant_u64)
        .or_else(|| object.get("max_input_tokens").and_then(tolerant_u64));
    let max_output_tokens = limit
        .and_then(|limit| limit.get("output"))
        .and_then(tolerant_u64)
        .or_else(|| object.get("maxTokens").and_then(tolerant_u64))
        .or_else(|| object.get("max_tokens").and_then(tolerant_u64))
        .or_else(|| object.get("max_output_tokens").and_then(tolerant_u64))
        .or_else(|| object.get("max").and_then(tolerant_u64));
    let tools =
        optional_bool(object.get("tools")).or_else(|| optional_bool(object.get("tool_call")));
    let thinking =
        optional_bool(object.get("thinking")).or_else(|| optional_bool(object.get("reasoning")));
    let images = optional_bool(object.get("images")).or_else(|| {
        object
            .get("input")
            .and_then(Value::as_array)
            .map(|modalities| {
                modalities
                    .iter()
                    .any(|value| value.as_str() == Some("image"))
            })
            .or_else(|| {
                object
                    .get("modalities")
                    .and_then(Value::as_object)
                    .and_then(|modalities| modalities.get("input"))
                    .and_then(Value::as_array)
                    .map(|modalities| {
                        modalities
                            .iter()
                            .any(|value| value.as_str() == Some("image"))
                    })
            })
            .or_else(|| optional_bool(object.get("attachment")))
    });
    let capability = object
        .get("capability")
        .or_else(|| object.get("capabilities"))
        .and_then(Value::as_object);
    let tools = tools.or_else(|| capability.and_then(|value| optional_bool(value.get("tools"))));
    let thinking =
        thinking.or_else(|| capability.and_then(|value| optional_bool(value.get("thinking"))));
    let images = images.or_else(|| capability.and_then(|value| optional_bool(value.get("images"))));
    Some(CatalogModel {
        id: id.to_owned(),
        name: name.to_owned(),
        settings: ModelSettings {
            context_window: advertised.map(|value| value.min(CATALOG_CONTEXT_CLAMP_TOKENS)),
            max_input_tokens,
            max_output_tokens,
            tools,
            thinking,
            images,
            headers: crate::profile::HeaderOverlay::new(),
        },
        advertised_context_window: advertised,
        cost: parse_cost(object.get("cost")),
        wire_hint: parse_wire_hint(
            object
                .get("wire_hint")
                .or_else(|| object.get("wire"))
                .or_else(|| object.get("api")),
        ),
    })
}

fn parse_cost(value: Option<&Value>) -> Option<ModelCost> {
    let object = value?.as_object()?;
    let cost = ModelCost {
        input: number_field(object, "input"),
        output: number_field(object, "output"),
        cache_read: number_field(object, "cacheRead")
            .or_else(|| number_field(object, "cache_read")),
        cache_write: number_field(object, "cacheWrite")
            .or_else(|| number_field(object, "cache_write")),
    };
    if cost == ModelCost::default() {
        None
    } else {
        Some(cost)
    }
}

fn number_field(object: &Map<String, Value>, name: &str) -> Option<Number> {
    match object.get(name)? {
        Value::Number(number) => Some(number.clone()),
        _ => None,
    }
}

fn parse_wire_hint(value: Option<&Value>) -> Option<String> {
    let hint = value?.as_str()?.trim();
    if hint.is_empty() || hint.len() > MAX_WIRE_HINT_BYTES {
        return None;
    }
    if hint.contains("://") || hint.contains('/') || hint.contains('\\') {
        return None;
    }
    Some(hint.to_owned())
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

fn cache_envelope(provider_id: &str, origin: &str, cache: &ValidCache) -> CacheEnvelope {
    let models = cached_models(&cache.catalog);
    let digest = digest_models(&models);
    CacheEnvelope {
        version: CACHE_VERSION,
        provider_id: provider_id.to_owned(),
        origin: origin.to_owned(),
        etag: cache.etag.clone(),
        last_modified: cache.last_modified.clone(),
        checked_at: cache.checked_at,
        digest,
        models,
    }
}

fn cached_models(catalog: &ModelCatalog) -> Vec<CachedModel> {
    catalog
        .providers()
        .flat_map(|provider| provider.models.values())
        .map(|model| CachedModel {
            id: model.id.clone(),
            name: model.name.clone(),
            cost: model.cost.clone(),
            advertised_context_window: model.advertised_context_window,
            context_window: model.settings.context_window,
            max_input_tokens: model.settings.max_input_tokens,
            max_output_tokens: model.settings.max_output_tokens,
            tools: model.settings.tools,
            thinking: model.settings.thinking,
            images: model.settings.images,
            wire_hint: model.wire_hint.clone(),
        })
        .collect()
}

fn catalog_from_cached(provider_id: &str, models: Vec<CachedModel>) -> ModelCatalog {
    let mut catalog = ModelCatalog::new();
    for model in models {
        catalog.insert_model(
            provider_id,
            provider_id,
            CatalogModel {
                id: model.id,
                name: model.name,
                settings: ModelSettings {
                    context_window: model.context_window,
                    max_input_tokens: model.max_input_tokens,
                    max_output_tokens: model.max_output_tokens,
                    tools: model.tools,
                    thinking: model.thinking,
                    images: model.images,
                    headers: crate::profile::HeaderOverlay::new(),
                },
                advertised_context_window: model.advertised_context_window,
                cost: model.cost,
                wire_hint: model.wire_hint,
            },
        );
    }
    catalog
}

fn read_cache(path: &Path, provider_id: &str, origin: &str) -> Option<ValidCache> {
    let file = std::fs::File::open(path).ok()?;
    let metadata = file.metadata().ok()?;
    if !metadata.is_file() {
        return None;
    }
    if metadata.len() > MAX_CACHE_BYTES as u64 {
        return None;
    }
    let bytes = read_cache_bytes(file)?;
    let envelope: CacheEnvelope = serde_json::from_slice(&bytes).ok()?;
    if envelope.version != CACHE_VERSION
        || envelope.provider_id != provider_id
        || envelope.origin != origin
    {
        return None;
    }
    if envelope.digest != digest_models(&envelope.models) {
        return None;
    }
    if envelope.models.len() > MAX_MODELS {
        return None;
    }
    let empty = envelope.models.is_empty();
    Some(ValidCache {
        catalog: catalog_from_cached(provider_id, envelope.models),
        etag: envelope.etag,
        last_modified: envelope.last_modified,
        checked_at: envelope.checked_at,
        empty,
    })
}

/// Accepts cache input no larger than [`MAX_CACHE_BYTES`].
///
/// Reads at most `MAX_CACHE_BYTES + 1` bytes so an extra byte proves
/// overflow. Larger inputs are rejected without buffering the remainder.
fn read_cache_bytes(reader: impl Read) -> Option<Vec<u8>> {
    let mut bytes = Vec::new();
    let limit = (MAX_CACHE_BYTES as u64).saturating_add(1);
    reader.take(limit).read_to_end(&mut bytes).ok()?;
    if bytes.len() > MAX_CACHE_BYTES {
        None
    } else {
        Some(bytes)
    }
}

fn cached_snapshot(cache: Option<ValidCache>) -> CatalogSnapshot {
    match cache {
        Some(cache) => snapshot_from_cache(cache),
        None => fallback_snapshot(),
    }
}

fn snapshot_from_cache(cache: ValidCache) -> CatalogSnapshot {
    let origin = if cache.empty {
        CatalogOrigin::Unsupported
    } else {
        CatalogOrigin::Cache
    };
    let etag = cache.etag.clone();
    let checked_at = Some(cache.checked_at);
    let mut catalog = ModelCatalog::fallback();
    catalog.overlay(cache.catalog);
    CatalogSnapshot {
        catalog,
        origin,
        etag,
        checked_at,
    }
}

fn fallback_snapshot() -> CatalogSnapshot {
    CatalogSnapshot {
        catalog: ModelCatalog::fallback(),
        origin: CatalogOrigin::BuiltInFallback,
        etag: None,
        checked_at: None,
    }
}

async fn bounded_body(
    response: reqwest::Response,
    cancel: &CancellationToken,
) -> Result<Vec<u8>, LlmError> {
    let mut stream = response.bytes_stream();
    let mut body = Vec::new();
    loop {
        let chunk = tokio::select! {
            biased;
            () = cancel.cancelled() => return Err(LlmError::Cancelled),
            chunk = stream.next() => chunk,
        };
        let Some(chunk) = chunk else {
            break;
        };
        let chunk = chunk.map_err(|error| LlmError::Transport(error.without_url().to_string()))?;
        if body.len().saturating_add(chunk.len()) > MAX_CATALOG_BYTES {
            return Err(LlmError::Config(format!(
                "provider catalog response exceeds {MAX_CATALOG_BYTES} bytes"
            )));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn write_cache_atomic(path: &Path, envelope: &CacheEnvelope) -> io::Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty());
    if let Some(parent) = parent {
        std::fs::create_dir_all(parent)?;
    }
    let directory = parent.unwrap_or_else(|| Path::new("."));
    let mut temporary = tempfile::NamedTempFile::new_in(directory)?;
    serde_json::to_writer(temporary.as_file_mut(), envelope).map_err(io::Error::other)?;
    temporary.as_file_mut().flush()?;
    temporary.as_file().sync_all()?;
    temporary
        .into_temp_path()
        .persist(path)
        .map_err(|error| error.error)
}

fn digest_models(models: &[CachedModel]) -> String {
    let bytes = serde_json::to_vec(models).unwrap_or_else(|_| b"[]".to_vec());
    format!("b3:{}", encode_hex(blake3::hash(&bytes).as_bytes()))
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[usize::from(byte >> 4)] as char);
        out.push(HEX[usize::from(byte & 0x0f)] as char);
    }
    out
}

fn validate_catalog_base_url(raw: String) -> Result<String, LlmError> {
    let parsed = reqwest::Url::parse(&raw)
        .map_err(|_| LlmError::Config("invalid model catalog URL".into()))?;
    let path_ok = parsed.path().is_empty() || parsed.path() == "/";
    if !matches!(parsed.scheme(), "http" | "https")
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || !path_ok
    {
        return Err(LlmError::Config(
            "model catalog URL must be an http(s) origin without credentials, path, query, or fragment"
                .into(),
        ));
    }
    let origin = parsed.origin();
    if !origin.is_tuple() {
        return Err(LlmError::Config("invalid model catalog URL".into()));
    }
    Ok(origin.ascii_serialization())
}

fn cache_file_name(provider_id: &str, origin: &str) -> String {
    if origin == PI_CATALOG_BASE_URL {
        format!("{provider_id}.json")
    } else {
        format!("{provider_id}.{}.json", origin_file_token(origin))
    }
}

fn origin_file_token(origin: &str) -> String {
    // 64-bit prefix only disambiguates local files; the envelope still
    // stores the full origin and rejects a mismatch.
    encode_hex(&blake3::hash(origin.as_bytes()).as_bytes()[..8])
}

fn inflight_key(origin: &str, provider_id: &str) -> String {
    format!("{origin}\n{provider_id}")
}

fn next_retry(
    attempt: u32,
    attempts: u32,
    deadline: tokio::time::Instant,
    error: &LlmError,
) -> bool {
    !matches!(error, LlmError::Cancelled | LlmError::Config(_))
        && attempt + 1 < attempts
        && tokio::time::Instant::now() < deadline
}

fn give_up_error(
    attempt: u32,
    attempts: u32,
    deadline: tokio::time::Instant,
    error: LlmError,
) -> LlmError {
    if matches!(error, LlmError::Cancelled | LlmError::Config(_)) || attempt + 1 >= attempts {
        error
    } else if tokio::time::Instant::now() >= deadline {
        LlmError::Timeout
    } else {
        error
    }
}

fn provider_catalog_url(base: &str, provider_id: &str) -> Result<reqwest::Url, LlmError> {
    let mut url = reqwest::Url::parse(base)
        .map_err(|_| LlmError::Config("invalid model catalog URL".into()))?;
    {
        let mut segments = url
            .path_segments_mut()
            .map_err(|()| LlmError::Config("invalid model catalog URL".into()))?;
        segments.pop_if_empty();
        segments.extend(["api", "models", "providers", provider_id]);
    }
    Ok(url)
}

fn header_string(value: Option<&reqwest::header::HeaderValue>) -> Option<String> {
    value
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
}

fn should_retry_status(status: StatusCode) -> bool {
    matches!(status.as_u16(), 408 | 425 | 429 | 500 | 502 | 503 | 504)
}

fn now_millis() -> u64 {
    u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or(u64::MAX)
}

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::{ModelLayers, WireKind, resolve_model_settings};
    use serde_json::json;

    fn sample_object_map(context: Value) -> Value {
        json!({
            "model-a": {
                "id": "model-a",
                "name": "Model A",
                "provider": "sample",
                "reasoning": true,
                "tool_call": "true",
                "attachment": false,
                "api": "openai-completions",
                "cost": {"input": 1, "output": 2, "cacheRead": 0.1, "cacheWrite": 1.25},
                "limit": {
                    "context": context,
                    "input": 120,
                    "output": "42"
                },
                "modalities": {"input": ["text", "image"], "output": ["text"]},
                "baseUrl": "https://evil.example/v1",
                "headers": {
                    "x-model-feature": "enabled",
                    "authorization": "Bearer must-not-enter-config"
                },
                "auth": {"scheme": "bearer", "env": "EVIL_KEY"},
                "api_key": "must-not-enter-config",
                "unknown_model_field": {"anything": true}
            }
        })
    }

    #[test]
    fn parses_object_map_and_array_documents() {
        let catalog = ModelCatalog::from_provider_json(
            "sample",
            sample_object_map(json!("1000")).to_string(),
        )
        .unwrap();
        let model = catalog.model("sample", "model-a").unwrap();
        assert_eq!(model.settings.context_window, Some(1_000));
        assert_eq!(model.settings.max_input_tokens, Some(120));
        assert_eq!(model.settings.max_output_tokens, Some(42));
        assert_eq!(model.settings.tools, Some(true));
        assert_eq!(model.settings.thinking, Some(true));
        assert_eq!(model.settings.images, Some(true));
        assert!(model.settings.headers.is_empty());
        assert_eq!(model.wire_hint.as_deref(), Some("openai-completions"));
        assert_eq!(
            model.cost.as_ref().and_then(|cost| cost.input.clone()),
            Some(Number::from(1u64))
        );

        let array = json!([{
            "id": "model-b",
            "name": "Model B",
            "provider": "sample",
            "contextWindow": 2048,
            "maxTokens": 128,
            "reasoning": false,
            "input": ["text"]
        }]);
        let catalog = ModelCatalog::from_provider_json("sample", array.to_string()).unwrap();
        assert!(catalog.model("sample", "model-b").is_some());

        let wrapped = json!({"models": {"model-c": {
            "id": "model-c",
            "name": "Model C",
            "context_window": 512
        }}});
        let catalog = ModelCatalog::from_provider_json("sample", wrapped.to_string()).unwrap();
        assert!(catalog.model("sample", "model-c").is_some());

        let nested = json!({"openai-completions": {"model-d": {
            "id": "model-d",
            "name": "Model D",
            "contextWindow": 256,
            "capability": {"tools": true, "thinking": false, "images": false}
        }}});
        let catalog = ModelCatalog::from_provider_json("sample", nested.to_string()).unwrap();
        let model = catalog.model("sample", "model-d").unwrap();
        assert_eq!(model.settings.tools, Some(true));
        assert_eq!(model.settings.thinking, Some(false));
    }

    #[test]
    fn rejects_overlong_ids_and_too_many_models() {
        let long_id = "a".repeat(MAX_ID_BYTES + 1);
        let oversized = json!([{"id": long_id, "name": "x", "contextWindow": 1}]);
        assert!(ModelCatalog::from_provider_json("sample", oversized.to_string()).is_err());

        let models: Vec<Value> = (0..=MAX_MODELS)
            .map(|index| json!({"id": format!("m{index}"), "contextWindow": 1}))
            .collect();
        assert!(
            ModelCatalog::from_provider_json("sample", Value::Array(models).to_string()).is_err()
        );
    }

    #[test]
    fn clamps_advertised_context_and_ignores_trust_fields() {
        let catalog = ModelCatalog::from_provider_json(
            "sample",
            sample_object_map(json!(1_000_000)).to_string(),
        )
        .unwrap();
        let model = catalog.model("sample", "model-a").unwrap();
        assert_eq!(model.advertised_context_window, Some(1_000_000));
        assert_eq!(
            model.settings.context_window,
            Some(CATALOG_CONTEXT_CLAMP_TOKENS)
        );
        let encoded = serde_json::to_string(&catalog).unwrap();
        assert!(!encoded.contains("evil.example"));
        assert!(!encoded.contains("must-not-enter-config"));
        assert!(!encoded.contains("authorization"));
        assert!(!encoded.contains("EVIL_KEY"));
    }

    #[test]
    fn rejects_mismatched_provider_and_empty_documents() {
        let foreign = json!([{"id": "stolen", "provider": "other", "contextWindow": 100}]);
        assert!(ModelCatalog::from_provider_json("sample", foreign.to_string()).is_err());
        assert!(ModelCatalog::from_provider_json("sample", b"not json").is_err());
        assert!(ModelCatalog::from_provider_json("sample", b"{}").is_err());
        assert!(ModelCatalog::from_provider_json("sample", b"[]").is_err());
        assert!(ModelCatalog::from_provider_json("../evil", b"[]").is_err());
    }

    #[test]
    fn user_settings_win_over_catalog_facts() {
        let catalog =
            ModelCatalog::from_provider_json("sample", sample_object_map(json!(800)).to_string())
                .unwrap();
        let catalog_settings = &catalog.model("sample", "model-a").unwrap().settings;
        let user = ModelSettings {
            context_window: Some(12_000),
            thinking: Some(false),
            ..ModelSettings::default()
        };
        let resolved = resolve_model_settings(ModelLayers {
            catalog: Some(catalog_settings),
            provider_config: Some(&user),
            ..ModelLayers::default()
        });
        assert_eq!(resolved.context_window, Some(12_000));
        assert_eq!(resolved.thinking, Some(false));
        assert_eq!(resolved.tools, Some(true));
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
    fn atomic_cache_replaces_old_file_and_strips_untrusted_fields() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("sample.json");
        std::fs::write(&path, "old").unwrap();
        let catalog =
            ModelCatalog::from_provider_json("sample", sample_object_map(json!(2048)).to_string())
                .unwrap();
        let cache = ValidCache {
            catalog,
            etag: Some("\"etag-1\"".into()),
            last_modified: Some("Wed, 21 Oct 2015 07:28:00 GMT".into()),
            checked_at: 1,
            empty: false,
        };
        write_cache_atomic(
            &path,
            &cache_envelope("sample", PI_CATALOG_BASE_URL, &cache),
        )
        .unwrap();
        let persisted = std::fs::read_to_string(&path).unwrap();
        assert!(!persisted.contains("must-not-enter-config"));
        assert!(!persisted.contains("authorization"));
        assert!(!persisted.contains("evil.example"));
        let loaded =
            read_cache(&path, "sample", PI_CATALOG_BASE_URL).expect("valid replacement cache");
        assert_eq!(loaded.etag.as_deref(), Some("\"etag-1\""));
        assert_eq!(
            loaded
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

    #[test]
    fn corrupt_digest_or_version_is_ignored() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("sample.json");
        let catalog =
            ModelCatalog::from_provider_json("sample", sample_object_map(json!(64)).to_string())
                .unwrap();
        let mut envelope = cache_envelope(
            "sample",
            PI_CATALOG_BASE_URL,
            &ValidCache {
                catalog,
                etag: Some("\"x\"".into()),
                last_modified: None,
                checked_at: 9,
                empty: false,
            },
        );
        envelope.digest = "b3:deadbeef".into();
        write_cache_atomic(&path, &envelope).unwrap();
        assert!(read_cache(&path, "sample", PI_CATALOG_BASE_URL).is_none());
    }

    #[tokio::test]
    async fn offline_uses_valid_cache_and_corrupt_cache_uses_fallback() {
        let directory = tempfile::tempdir().unwrap();
        let catalog =
            ModelCatalog::from_provider_json("sample", sample_object_map(json!(4096)).to_string())
                .unwrap();
        let cache = ValidCache {
            catalog,
            etag: Some("\"cached\"".into()),
            last_modified: None,
            checked_at: 4,
            empty: false,
        };
        write_cache_atomic(
            &directory.path().join("sample.json"),
            &cache_envelope("sample", PI_CATALOG_BASE_URL, &cache),
        )
        .unwrap();
        let snapshot = CatalogClient::new(directory.path())
            .with_offline(true)
            .load("sample")
            .await;
        assert_eq!(snapshot.origin, CatalogOrigin::Cache);
        assert_eq!(snapshot.etag.as_deref(), Some("\"cached\""));
        assert!(snapshot.catalog.model("sample", "model-a").is_some());

        std::fs::write(directory.path().join("sample.json"), "corrupt").unwrap();
        let snapshot = CatalogClient::new(directory.path())
            .with_offline(true)
            .load("sample")
            .await;
        assert_eq!(snapshot.origin, CatalogOrigin::BuiltInFallback);
        assert!(snapshot.catalog.model("openai", "gpt-4o-mini").is_some());
    }

    #[test]
    fn rejects_models_dev_style_full_catalog_url() {
        let error = CatalogClient::new(".")
            .with_base_url("https://models.dev/api.json")
            .unwrap_err();
        assert!(matches!(error, LlmError::Config(_)));
    }

    #[test]
    fn catalog_origin_is_normalized_and_bound_to_cache() {
        let pi = CatalogClient::new(".")
            .with_base_url("https://PI.DEV/")
            .unwrap();
        assert_eq!(
            pi.cache_path("openai").unwrap().file_name().unwrap(),
            "openai.json"
        );
        let custom = CatalogClient::new(".")
            .with_base_url("http://127.0.0.1:9")
            .unwrap();
        let custom_name = custom
            .cache_path("openai")
            .unwrap()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        assert_ne!(custom_name, "openai.json");
        assert!(custom_name.starts_with("openai."));
        assert!(custom_name.ends_with(".json"));

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("sample.json");
        let catalog =
            ModelCatalog::from_provider_json("sample", sample_object_map(json!(64)).to_string())
                .unwrap();
        let cache = ValidCache {
            catalog,
            etag: Some("\"x\"".into()),
            last_modified: None,
            checked_at: 9,
            empty: false,
        };
        write_cache_atomic(
            &path,
            &cache_envelope("sample", PI_CATALOG_BASE_URL, &cache),
        )
        .unwrap();
        assert!(read_cache(&path, "sample", PI_CATALOG_BASE_URL).is_some());
        assert!(read_cache(&path, "sample", "http://127.0.0.1:9").is_none());
    }

    #[test]
    fn cache_reader_accepts_max_bytes_and_rejects_oversize() {
        let exact = vec![0_u8; MAX_CACHE_BYTES];
        assert_eq!(
            read_cache_bytes(exact.as_slice()).map(|bytes| bytes.len()),
            Some(MAX_CACHE_BYTES)
        );

        let oversize = vec![0_u8; MAX_CACHE_BYTES + 1];
        assert!(read_cache_bytes(oversize.as_slice()).is_none());

        struct OversizeReader {
            remaining: usize,
        }
        impl Read for OversizeReader {
            fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
                let n = buf.len().min(self.remaining);
                buf[..n].fill(b'x');
                self.remaining -= n;
                Ok(n)
            }
        }
        assert!(
            read_cache_bytes(OversizeReader {
                remaining: MAX_CACHE_BYTES.saturating_add(2),
            })
            .is_none()
        );
    }
}

// Rust guideline compliant 2026-08-26

//! `mcode-llm` — provider abstraction for MCode (design doc
//! `01-agent-core.md` §2).
//!
//! Everything model-related flows through the [`Provider`] trait:
//!
//! ```text
//! caller ──Request──► Provider::stream(req, cancel) ──► EventStream
//!                                                          │ Start
//!                                                          │ TextDelta / ThinkingDelta
//!                                                          │ ToolCallDelta … ToolCallEnd
//!                                                          ▼ Done { message } | Error
//! ```
//!
//! * [`Request`] carries the model id, system prompt parts, the shared
//!   [`mcode_core::message::Message`] history, [`mcode_core::ToolSpec`]s serialized
//!   from the tool registry, and an optional thinking configuration.
//! * [`EventStream`] is a push-channel plus async iterator: providers
//!   push [`StreamEvent`]s as they arrive; the stream terminates after
//!   `Done`/`Error` (or when the caller cancels).
//! * [`ProfileProvider`] combines a data-only [`ProviderProfile`] with reusable
//!   OpenAI Chat Completions, OpenAI Responses, or Anthropic Messages wire
//!   adapters. Profiles are resolved through [`ProviderRegistry`] or loaded as
//!   ordinary JSON. Credential values stay behind explicit or environment
//!   references and never enter profile JSON.
//! * [`catalog`] loads Pi per-provider catalogs with ETag caching, offline
//!   reuse, atomic replacement, and a tiny built-in fallback.
//!
//! Providers are plain data plumbing: no UI, no session state — the
//! same separation pi's agent loop relies on.

pub mod anthropic;
pub mod catalog;
pub mod chat_completions;
pub mod error;
pub mod identity;
pub mod profile;
pub mod profile_provider;
pub mod provider;
pub mod registry;
pub mod responses;
pub mod sse;
pub mod stream;

pub use anthropic::AnthropicAggregator;
pub use catalog::{
    CATALOG_CONTEXT_CLAMP_TOKENS, CatalogClient, CatalogModel, CatalogOrigin, CatalogProvider,
    CatalogRefresh, CatalogSnapshot, ModelCatalog, ModelCost, PI_CATALOG_BASE_URL,
    REMOTE_CATALOG_REFRESH_INTERVAL, default_model_id,
};
pub use chat_completions::{ChatCompletionAggregator, DEFAULT_OPENAI_BASE_URL};
pub use error::LlmError;
pub use identity::{ClientIdentity, pi_compat_user_agent};
pub use profile::{
    ApiKey, AuthProfile, AuthScheme, HeaderOverlay, HeaderProfile, ModelLayers, ModelSettings,
    ProviderCapabilities, ProviderProfile, WireKind, anthropic_profile, builtin_profiles,
    deepseek_profile, generic_openai_profile, openai_profile, opencode_profile, openrouter_profile,
    resolve_model_settings,
};
pub use profile_provider::{ProfileProvider, ProviderCallOptions};
pub use provider::{ModelId, Provider, Request, StreamEvent, ThinkingConfig, ThinkingLevel};
pub use registry::ProviderRegistry;
pub use responses::ResponsesAggregator;
pub use sse::SseFramer;
pub use stream::{EventStream, EventStreamSender};

// Re-exported for downstream crates so iterating an `EventStream` does
// not require depending on `tokio-stream` directly.
pub use tokio_stream::StreamExt;
pub use tokio_util::sync::CancellationToken;

// Rust guideline compliant 2026-08-26

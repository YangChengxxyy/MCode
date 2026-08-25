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
//!   [`mcode_core::message::Message`] history, [`ToolSpec`]s serialized
//!   from the tool registry, and an optional thinking configuration.
//! * [`EventStream`] is a push-channel plus async iterator: providers
//!   push [`StreamEvent`]s as they arrive; the stream terminates after
//!   `Done`/`Error` (or when the caller cancels).
//! * Two implementations ship in M1:
//!   [`OpenAiProvider`](openai::OpenAiProvider) for any
//!   OpenAI-compatible `/chat/completions` endpoint (SSE streaming with
//!   incremental `tool_calls` argument aggregation), and
//!   [`FakeProvider`](fake::FakeProvider), a scripted provider that
//!   records requests and powers all downstream no-network tests.
//! * API keys resolve via [`auth`]: explicit argument → provider env
//!   var → `~/.mcode/auth.toml` (`$MCODE_HOME` overrides the home).
//!
//! Providers are plain data plumbing: no UI, no session state — the
//! same separation pi's agent loop relies on.

pub mod auth;
pub mod error;
pub mod fake;
pub mod openai;
pub mod provider;
pub mod stream;

pub use auth::{auth_file_path, mcode_home, resolve_api_key};
pub use error::LlmError;
pub use fake::{FakeProvider, ScriptTurn};
pub use openai::{ChatCompletionAggregator, DEFAULT_OPENAI_BASE_URL, OpenAiProvider, SseFramer};
pub use provider::{ModelId, Provider, Request, StreamEvent, ThinkingConfig, ThinkingLevel};
pub use stream::{EventStream, EventStreamSender};

// Re-exported for downstream crates so iterating an `EventStream` does
// not require depending on `tokio-stream` directly.
pub use tokio_stream::StreamExt;
pub use tokio_util::sync::CancellationToken;

//! Typed, content-redacted events delivered to plugins.
//!
//! Event payloads omit prompts, model output text, tool arguments, tool
//! results, headers, request bodies, transcripts, and secret material.
//! Compaction is a closed host core and has no plugin hook or event.

// Rust guideline compliant 2026-08-26.

use serde::{Deserialize, Serialize};

use crate::ids::Identifier;

/// Event categories available for manifest subscriptions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EventKind {
    /// Active model metadata changed.
    Model,
    /// A model stream changed phase.
    Stream,
    /// Token and cost counters changed.
    Usage,
    /// A tool invocation changed phase.
    Tool,
    /// A network operation changed phase.
    Network,
}

/// Generic activity phase without payload content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ActivityPhase {
    /// Work was accepted.
    Queued,
    /// Work started.
    Started,
    /// Progress metadata changed.
    Updated,
    /// Work completed successfully.
    Completed,
    /// Work failed.
    Failed,
    /// Work was cancelled.
    Cancelled,
}

/// Provider-neutral model identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelIdentity {
    /// Provider namespace.
    pub provider: String,
    /// Provider-owned model id.
    pub model: String,
}

/// Redacted model-selection event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelEvent {
    /// Newly active model, when one is selected.
    pub active: Option<ModelIdentity>,
    /// Previously active model, when known.
    pub previous: Option<ModelIdentity>,
}

/// Provider-neutral usage counters.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UsageMetrics {
    /// Input tokens billed or reported.
    pub input_tokens: u64,
    /// Output tokens billed or reported.
    pub output_tokens: u64,
    /// Cache-read tokens billed or reported.
    pub cache_read_tokens: u64,
    /// Cache-write tokens billed or reported.
    pub cache_write_tokens: u64,
    /// Cost in millionths of the host currency unit.
    pub cost_micros: u64,
}

/// Redacted stream event containing sizes rather than content.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StreamEvent {
    /// Host-issued stream id.
    pub stream_id: Identifier,
    /// Current stream phase.
    pub phase: ActivityPhase,
    /// Cumulative output bytes observed by the host.
    pub output_bytes: u64,
    /// Latest usage, when available.
    pub usage: Option<UsageMetrics>,
    /// Sanitized failure code without provider response text.
    pub failure_code: Option<Identifier>,
}

/// Standalone usage update.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UsageEvent {
    /// Model associated with the counters, when known.
    pub model: Option<ModelIdentity>,
    /// Updated counters.
    pub usage: UsageMetrics,
}

/// Redacted tool activity without arguments or result content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ToolEvent {
    /// Host-issued call id.
    pub call_id: Identifier,
    /// Public tool name.
    pub tool_name: Identifier,
    /// Current invocation phase.
    pub phase: ActivityPhase,
    /// Input size in bytes, without the input itself.
    pub input_bytes: u64,
    /// Output size in bytes, without the output itself.
    pub output_bytes: u64,
    /// Sanitized failure code.
    pub failure_code: Option<Identifier>,
}

/// Network endpoint stripped of path, query, credentials, and headers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NetworkEndpoint {
    /// Lowercase transport scheme such as `https`.
    pub scheme: String,
    /// DNS host or literal address only.
    pub host: String,
    /// Explicit port, when present.
    pub port: Option<u16>,
}

/// Redacted network activity without body or header content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NetworkEvent {
    /// Host-issued operation id.
    pub operation_id: Identifier,
    /// Current operation phase.
    pub phase: ActivityPhase,
    /// Endpoint stripped to scheme, host, and port.
    pub endpoint: NetworkEndpoint,
    /// Response status, when a response exists.
    pub status: Option<u16>,
    /// Bytes sent without request content.
    pub sent_bytes: u64,
    /// Bytes received without response content.
    pub received_bytes: u64,
    /// Sanitized failure code.
    pub failure_code: Option<Identifier>,
}

/// Event envelope delivered through a plugin's bounded mailbox.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "event",
    content = "data",
    rename_all = "camelCase",
    deny_unknown_fields
)]
pub enum PluginEvent {
    /// Active-model metadata changed.
    Model(ModelEvent),
    /// A model stream changed phase.
    Stream(StreamEvent),
    /// Usage counters changed.
    Usage(UsageEvent),
    /// A tool invocation changed phase.
    Tool(ToolEvent),
    /// A network operation changed phase.
    Network(NetworkEvent),
}

impl PluginEvent {
    /// Returns the event's subscription category.
    #[must_use]
    pub fn kind(&self) -> EventKind {
        match self {
            Self::Model(_) => EventKind::Model,
            Self::Stream(_) => EventKind::Stream,
            Self::Usage(_) => EventKind::Usage,
            Self::Tool(_) => EventKind::Tool,
            Self::Network(_) => EventKind::Network,
        }
    }

    /// Validates bounded public metadata in this event.
    ///
    /// # Errors
    ///
    /// Returns [`EventValidationError`] for malformed model or endpoint
    /// metadata.
    pub fn validate(&self) -> Result<(), EventValidationError> {
        match self {
            Self::Model(event) => {
                if let Some(model) = &event.active {
                    validate_model(model)?;
                }
                if let Some(model) = &event.previous {
                    validate_model(model)?;
                }
            }
            Self::Usage(event) => {
                if let Some(model) = &event.model {
                    validate_model(model)?;
                }
            }
            Self::Network(event) => validate_endpoint(&event.endpoint)?,
            Self::Stream(_) | Self::Tool(_) => {}
        }
        Ok(())
    }
}

fn validate_model(model: &ModelIdentity) -> Result<(), EventValidationError> {
    validate_public_text(&model.provider, 128)?;
    validate_public_text(&model.model, 256)
}

fn validate_endpoint(endpoint: &NetworkEndpoint) -> Result<(), EventValidationError> {
    validate_public_text(&endpoint.scheme, 16)?;
    validate_public_text(&endpoint.host, 253)?;
    if endpoint
        .scheme
        .bytes()
        .any(|byte| !byte.is_ascii_lowercase())
        || endpoint.host.contains(['/', '@', '?', '#'])
    {
        return Err(EventValidationError);
    }
    Ok(())
}

fn validate_public_text(value: &str, max_bytes: usize) -> Result<(), EventValidationError> {
    if value.is_empty() || value.len() > max_bytes || value.chars().any(char::is_control) {
        return Err(EventValidationError);
    }
    Ok(())
}

/// A redacted event contained malformed public metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("plugin event metadata is invalid")]
pub struct EventValidationError;

#[cfg(test)]
mod tests {
    use super::{ActivityPhase, EventKind, NetworkEndpoint, NetworkEvent, PluginEvent};
    use crate::ids::Identifier;

    #[test]
    fn event_shape_exposes_no_prompt_secret_transcript_or_compaction() {
        let event = PluginEvent::Network(NetworkEvent {
            operation_id: Identifier::parse("request_1").expect("id"),
            phase: ActivityPhase::Completed,
            endpoint: NetworkEndpoint {
                scheme: "https".into(),
                host: "example.com".into(),
                port: None,
            },
            status: Some(200),
            sent_bytes: 12,
            received_bytes: 34,
            failure_code: None,
        });
        assert_eq!(event.kind(), EventKind::Network);
        let json = serde_json::to_string(&event).expect("serialize");
        assert!(!json.contains("prompt"));
        assert!(!json.contains("secret"));
        assert!(!json.contains("transcript"));
        assert!(!json.contains("compaction"));
        event.validate().expect("valid event");
    }
}

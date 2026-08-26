//! The `Provider` trait and the request/stream-event vocabulary every
//! provider implementation shares (design doc `01-agent-core.md` §2).

use mcode_core::message::{AssistantMessage, Message, ToolCall};
use mcode_core::tool::ToolSpec;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::error::LlmError;
use crate::stream::EventStream;

/// Identifier of a model, e.g. `"gpt-4o-mini"`. Opaque string newtype —
/// providers interpret it; callers choose it via their model config.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ModelId(String);

impl ModelId {
    /// Borrow the model id string.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consume and return the inner string.
    pub fn into_inner(self) -> String {
        self.0
    }
}

impl fmt::Display for ModelId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for ModelId {
    fn from(s: &str) -> Self {
        Self(s.to_owned())
    }
}

impl From<String> for ModelId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

use std::fmt;

/// A single streaming request to a model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Request {
    /// Model to stream from (provider-specific id).
    pub model: ModelId,
    /// System prompt parts; providers emit them as leading system messages
    /// in the given order.
    pub system_prompt: Vec<String>,
    /// Conversation history (user / assistant / tool results; `Custom`
    /// messages are skipped when translating to provider wire formats).
    pub messages: Vec<Message>,
    /// Tools the model may call, serialized from the tool registry.
    pub tools: Vec<ToolSpec>,
    /// Thinking / reasoning configuration, when the model supports it.
    pub thinking: Option<ThinkingConfig>,
}

impl Request {
    /// Build a request for `model` with empty history; set the public
    /// fields directly or use the `with_*` builders.
    pub fn new(model: impl Into<ModelId>) -> Self {
        Self {
            model: model.into(),
            system_prompt: Vec::new(),
            messages: Vec::new(),
            tools: Vec::new(),
            thinking: None,
        }
    }

    /// Append a system prompt part.
    pub fn with_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt.push(prompt.into());
        self
    }

    /// Append a message to the conversation history.
    pub fn with_message(mut self, message: Message) -> Self {
        self.messages.push(message);
        self
    }

    /// Append a tool.
    pub fn with_tool(mut self, tool: ToolSpec) -> Self {
        self.tools.push(tool);
        self
    }

    /// Set the thinking configuration.
    pub fn with_thinking(mut self, thinking: ThinkingConfig) -> Self {
        self.thinking = Some(thinking);
        self
    }
}

/// Thinking / reasoning budget configuration.
///
/// M1 models reasoning as a discrete level (matching OpenAI's
/// `reasoning_effort`); token-budget fields can be added when an
/// Anthropic-style provider lands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThinkingConfig {
    /// Requested reasoning depth.
    pub level: ThinkingLevel,
}

/// Discrete reasoning depth levels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThinkingLevel {
    /// Reason as little as possible.
    Minimal,
    /// Low reasoning effort.
    Low,
    /// Medium reasoning effort.
    Medium,
    /// High reasoning effort.
    High,
}

impl ThinkingLevel {
    /// The wire string OpenAI-compatible endpoints expect for
    /// `reasoning_effort`.
    pub fn as_effort_str(&self) -> &'static str {
        match self {
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }
}

/// Events yielded while an assistant message streams in. A stream is
/// terminated exactly once by [`StreamEvent::Done`] (success) or
/// [`StreamEvent::Error`] (failure); consumers may stop early by
/// cancelling the request's token.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum StreamEvent {
    /// The request was accepted and streaming has begun.
    Start,
    /// Incremental text content.
    TextDelta(String),
    /// Incremental thinking / reasoning content.
    ThinkingDelta(String),
    /// Incremental tool-call arguments; `partial_json` is a raw JSON
    /// fragment (not necessarily valid JSON on its own).
    ToolCallDelta { id: String, partial_json: String },
    /// A tool call finished aggregating; carries the complete call with
    /// parsed `arguments`.
    ToolCallEnd(ToolCall),
    /// The message completed; carries the fully assembled
    /// [`AssistantMessage`].
    Done { message: AssistantMessage },
    /// Streaming failed; the stream terminates after this event.
    Error(LlmError),
}

/// A source of model completions (OpenAI-compatible endpoint, Anthropic,
/// scripted fake for tests, …).
///
/// `stream` returns immediately with an [`EventStream`]; the provider
/// pushes events onto it as they arrive. Cancellation is cooperative:
/// providers must stop producing and release resources when `cancel`
/// fires (the returned stream also terminates itself when cancelled).
#[async_trait::async_trait]
pub trait Provider: Send + Sync {
    /// Stable provider identifier, e.g. `"openai"` (used in auth config
    /// and telemetry).
    fn id(&self) -> &str;

    /// Start streaming a completion for `req`.
    async fn stream(
        &self,
        req: &Request,
        cancel: CancellationToken,
    ) -> Result<EventStream, LlmError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_id_display_and_conversions() {
        let id = ModelId::from("gpt-4o-mini");
        assert_eq!(id.as_str(), "gpt-4o-mini");
        assert_eq!(id.to_string(), "gpt-4o-mini");
        assert_eq!(ModelId::from(String::from("x")).into_inner(), "x");
    }

    #[test]
    fn request_builders() {
        let req = Request::new("m1")
            .with_system_prompt("be terse")
            .with_system_prompt("use tools")
            .with_message(Message::User(mcode_core::UserMessage::text("hi")))
            .with_tool(ToolSpec {
                name: "read".into(),
                description: "read a file".into(),
                params_schema: serde_json::json!({"type": "object"}),
            })
            .with_thinking(ThinkingConfig {
                level: ThinkingLevel::Medium,
            });
        assert_eq!(req.model.as_str(), "m1");
        assert_eq!(req.system_prompt.len(), 2);
        assert_eq!(req.messages.len(), 1);
        assert_eq!(req.tools.len(), 1);
        assert_eq!(req.thinking.map(|t| t.level), Some(ThinkingLevel::Medium));
    }

    #[test]
    fn request_and_events_roundtrip() {
        // Requests and events both cross serde boundaries (test fixtures
        // and logs); pin the roundtrip.
        let req = Request::new("gpt-4o-mini")
            .with_message(Message::User(mcode_core::UserMessage::text("hi")))
            .with_tool(ToolSpec {
                name: "bash".into(),
                description: "run".into(),
                params_schema: serde_json::json!({"type": "object"}),
            })
            .with_thinking(ThinkingConfig {
                level: ThinkingLevel::High,
            });
        let json = serde_json::to_string(&req).unwrap();
        let back: Request = serde_json::from_str(&json).unwrap();
        assert_eq!(back, req);

        let events = vec![
            StreamEvent::Start,
            StreamEvent::TextDelta("a".into()),
            StreamEvent::ThinkingDelta("b".into()),
            StreamEvent::ToolCallDelta {
                id: "c1".into(),
                partial_json: "{\"x\":".into(),
            },
            StreamEvent::ToolCallEnd(ToolCall::new("c1", "read", serde_json::json!({"x": 1}))),
            StreamEvent::Done {
                message: AssistantMessage {
                    blocks: vec![mcode_core::ContentBlock::Text("done".into())],
                    usage: None,
                    stop_reason: mcode_core::StopReason::Stop,
                },
            },
            StreamEvent::Error(LlmError::Timeout),
        ];
        for event in events {
            let json = serde_json::to_string(&event).unwrap();
            let back: StreamEvent = serde_json::from_str(&json).unwrap();
            assert_eq!(back, event);
        }
    }

    #[test]
    fn thinking_level_wire_strings() {
        assert_eq!(ThinkingLevel::Minimal.as_effort_str(), "minimal");
        assert_eq!(ThinkingLevel::Low.as_effort_str(), "low");
        assert_eq!(ThinkingLevel::Medium.as_effort_str(), "medium");
        assert_eq!(ThinkingLevel::High.as_effort_str(), "high");
    }
}

//! Message model — the core vocabulary exchanged between user, model, tools,
//! and plugins (design doc `01-agent-core.md` §1).
//!
//! Serde uses the default externally-tagged representation here; the exact
//! wire format for LLM providers is owned by `mcode-llm` (T2) and the
//! session-log format by `mcode-session` (T5).

use serde::{Deserialize, Serialize};

/// A message in the conversation tree.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Message {
    /// A message authored by the user (prompts, steers, follow-ups).
    User(UserMessage),
    /// A message produced by the model.
    Assistant(AssistantMessage),
    /// The result of executing a tool call.
    ToolResult(ToolResultMessage),
    /// Plugin-defined message. The `data` payload passes through
    /// serialization untouched so plugins can persist arbitrary state —
    /// the Rust replacement for pi's declaration merging.
    Custom(CustomMessage),
}

/// A user-authored message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UserMessage {
    pub content: Vec<ContentBlock>,
}

impl UserMessage {
    /// Build a plain-text user message.
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            content: vec![ContentBlock::Text(text.into())],
        }
    }
}

/// A model-produced message: ordered content blocks plus turn metadata.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssistantMessage {
    pub blocks: Vec<ContentBlock>,
    /// Token usage as reported by the provider, when available.
    pub usage: Option<Usage>,
    pub stop_reason: StopReason,
}

/// A single unit of message content.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ContentBlock {
    /// Plain text.
    Text(String),
    /// Model reasoning ("thinking") content.
    Thinking(String),
    /// A request to invoke a tool.
    ToolCall(ToolCall),
    /// Binary payload (currently only used for images).
    Image(BinaryData),
}

/// A tool invocation requested by the model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    /// Provider-assigned call id (opaque string; matched by
    /// [`ToolResultMessage::tool_call_id`]).
    pub id: String,
    /// Name of the tool to invoke.
    pub name: String,
    /// Arguments as raw JSON; validated against the tool's schema at
    /// dispatch time (`mcode-tools`).
    pub arguments: serde_json::Value,
}

/// The outcome of executing a tool call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolResultMessage {
    /// Id of the [`ToolCall`] this answers.
    pub tool_call_id: String,
    /// Content visible to the model.
    pub content: Vec<ContentBlock>,
    pub is_error: bool,
    /// Structured details for the UI layer only — never enters LLM context
    /// (structured diffs, cwd, …). Splitting `details` from `content`
    /// keeps tokens out of the model loop (pi's ToolResult pattern).
    pub details: Option<serde_json::Value>,
}

/// A plugin-defined message; serialized transparently (`data` passes
/// through verbatim). Session logs store it as-is for persistence of
/// plugin state such as plan trackers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CustomMessage {
    /// Plugin-scoped kind discriminator, e.g. `"plugin:plan"`.
    pub kind: String,
    /// Arbitrary plugin payload, preserved verbatim.
    pub data: serde_json::Value,
}

/// Why the model stopped generating.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StopReason {
    /// Natural end of turn.
    Stop,
    /// The model wants to call tools.
    ToolUse,
    /// Output was cut off by a length/token limit.
    Length,
    /// Generation ended because of an error.
    Error,
}

/// Token usage reported by a provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

/// Binary content (base64) with its MIME type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BinaryData {
    /// Base64-encoded bytes, as expected by provider image APIs.
    pub data: String,
    /// MIME type, e.g. `"image/png"`.
    pub mime_type: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn assert_roundtrip<T>(value: &T)
    where
        T: Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug,
    {
        let json = serde_json::to_string(value).expect("serialize");
        let back: T = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(&back, value);
    }

    fn sample_tool_call() -> ToolCall {
        ToolCall {
            id: "call_abc123".into(),
            name: "read".into(),
            arguments: json!({"path": "Cargo.toml", "offset": 1}),
        }
    }

    #[test]
    fn user_message_roundtrip() {
        assert_roundtrip(&Message::User(UserMessage::text("hello")));
        assert_roundtrip(&Message::User(UserMessage {
            content: vec![
                ContentBlock::Text("describe this:".into()),
                ContentBlock::Image(BinaryData {
                    data: "aGVsbG8=".into(),
                    mime_type: "image/png".into(),
                }),
            ],
        }));
    }

    #[test]
    fn assistant_message_roundtrip_all_block_kinds() {
        for stop_reason in [
            StopReason::Stop,
            StopReason::ToolUse,
            StopReason::Length,
            StopReason::Error,
        ] {
            let msg = Message::Assistant(AssistantMessage {
                blocks: vec![
                    ContentBlock::Thinking("let me think".into()),
                    ContentBlock::Text("reading the file".into()),
                    ContentBlock::ToolCall(sample_tool_call()),
                    ContentBlock::Image(BinaryData {
                        data: "AAEC".into(),
                        mime_type: "image/jpeg".into(),
                    }),
                ],
                usage: Some(Usage {
                    input_tokens: 1200,
                    output_tokens: 42,
                }),
                stop_reason,
            });
            assert_roundtrip(&msg);
        }
    }

    #[test]
    fn assistant_message_without_usage_roundtrip() {
        assert_roundtrip(&Message::Assistant(AssistantMessage {
            blocks: vec![ContentBlock::Text("hi".into())],
            usage: None,
            stop_reason: StopReason::Stop,
        }));
    }

    #[test]
    fn tool_result_roundtrip_with_and_without_details() {
        let base = ToolResultMessage {
            tool_call_id: "call_abc123".into(),
            content: vec![ContentBlock::Text("file contents".into())],
            is_error: false,
            details: None,
        };
        assert_roundtrip(&Message::ToolResult(base.clone()));

        let with_details = ToolResultMessage {
            is_error: true,
            details: Some(json!({"cwd": "/tmp", "diff": {"added": 3, "removed": 1}})),
            ..base
        };
        assert_roundtrip(&Message::ToolResult(with_details));
    }

    #[test]
    fn tool_call_arguments_preserve_arbitrary_json() {
        let arguments = json!({"nested": {"list": [1, 2.5, null, true, "x"], "obj": {}}});
        let call = ToolCall {
            id: "c1".into(),
            name: "bash".into(),
            arguments: arguments.clone(),
        };
        let back: ToolCall = serde_json::from_str(&serde_json::to_string(&call).unwrap()).unwrap();
        assert_eq!(back.arguments, arguments);
    }

    #[test]
    fn custom_message_preserves_arbitrary_json() {
        let data = json!({
            "plan": [
                {"step": 1, "title": "探索实现方案", "done": true},
                {"step": 2, "title": "write code", "done": false, "notes": null}
            ],
            "meta": {"progress": 0.5, "tags": [], "owner": serde_json::Value::Null}
        });
        let msg = Message::Custom(CustomMessage {
            kind: "plugin:plan".into(),
            data: data.clone(),
        });
        let json = serde_json::to_string(&msg).unwrap();
        let back: Message = serde_json::from_str(&json).unwrap();
        match back {
            Message::Custom(custom) => {
                assert_eq!(custom.kind, "plugin:plan");
                assert_eq!(custom.data, data);
            }
            other => panic!("expected Custom, got {other:?}"),
        }
    }

    #[test]
    fn stop_reason_and_usage_roundtrip() {
        assert_roundtrip(&StopReason::ToolUse);
        assert_roundtrip(&Usage {
            input_tokens: 7,
            output_tokens: 9,
        });
        assert_roundtrip(&BinaryData {
            data: "Zm9v".into(),
            mime_type: "application/octet-stream".into(),
        });
    }
}

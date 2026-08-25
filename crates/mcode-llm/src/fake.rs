//! Scripted provider for tests — the foundation of every downstream
//! no-network test (agent loop, session, CLI e2e).
//!
//! A `FakeProvider` holds a script of [`ScriptTurn`]s. Every call to
//! [`Provider::stream`] records the request and consumes the next turn:
//! a [`ScriptTurn::Message`] is streamed as `TextDelta`/`ThinkingDelta`
//! shards and `ToolCallDelta` fragments followed by `ToolCallEnd` and
//! `Done`; a [`ScriptTurn::Error`] fails the turn deterministically.
//! When the script is exhausted, `stream` returns
//! [`LlmError::Config`] — deterministic, so tests fail loudly on
//! script/turn mismatches.
//!
//! Scripts load from an inline `Vec`, a JSON string, or a JSON file
//! (the `--fake <script.json>` plumbing of the CLI). The JSON shape is
//! a flat, human-friendly form:
//!
//! ```json
//! [
//!   { "text": "I'll read the file.",
//!     "tool_calls": [{"id": "call_1", "name": "read",
//!                     "arguments": {"path": "Cargo.toml"}}],
//!     "stop_reason": "ToolUse" },
//!   { "text": "It contains the workspace config.", "stop_reason": "Stop" },
//!   { "error": { "http": { "status": 429, "body": "rate limited" } } }
//! ]
//! ```
//!
//! `stop_reason` defaults to `ToolUse` when the turn has tool calls and
//! `Stop` otherwise; `thinking` adds a thinking block; `usage` attaches
//! token counts. An exact [`mcode_core::AssistantMessage`] can also be
//! given via `{"message": …}` (used when serializing scripts back out).

use std::collections::VecDeque;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use mcode_core::message::{AssistantMessage, ContentBlock, StopReason, Usage};
use serde::ser::SerializeMap;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::error::LlmError;
use crate::provider::{Provider, Request, StreamEvent};
use crate::stream::{EventStream, EventStreamSender};

/// Characters per streamed delta shard.
const SHARD_CHARS: usize = 16;

/// One scripted turn: either stream a message or fail with an error.
#[derive(Debug, Clone, PartialEq)]
pub enum ScriptTurn {
    /// Stream this assistant message as one full turn.
    Message(AssistantMessage),
    /// Terminate the turn with this error.
    Error(LlmError),
}

/// Ergonomic on-disk turn shape; converts into [`ScriptTurn`].
#[derive(Deserialize)]
struct RawTurn {
    /// Exact assistant message (alternative to the flat fields).
    #[serde(default)]
    message: Option<AssistantMessage>,
    /// Turn fails with this error (takes precedence over everything).
    #[serde(default)]
    error: Option<LlmError>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    thinking: Option<String>,
    #[serde(default)]
    tool_calls: Vec<mcode_core::message::ToolCall>,
    #[serde(default)]
    usage: Option<Usage>,
    #[serde(default)]
    stop_reason: Option<StopReason>,
}

impl Serialize for ScriptTurn {
    /// Serialized as the exact `{"message": …}` / `{"error": …}` form so
    /// round-trips are lossless (the flat form cannot represent every
    /// block combination).
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            Self::Message(message) => {
                let mut map = serializer.serialize_map(Some(1))?;
                map.serialize_entry("message", message)?;
                map.end()
            }
            Self::Error(error) => {
                let mut map = serializer.serialize_map(Some(1))?;
                map.serialize_entry("error", error)?;
                map.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for ScriptTurn {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = RawTurn::deserialize(deserializer)?;
        if let Some(error) = raw.error {
            return Ok(Self::Error(error));
        }
        if let Some(message) = raw.message {
            return Ok(Self::Message(message));
        }
        if raw.text.is_none()
            && raw.thinking.is_none()
            && raw.tool_calls.is_empty()
            && raw.usage.is_none()
        {
            return Err(serde::de::Error::custom(
                "script turn must set `error`, `message`, or content fields \
                    (`text`, `thinking`, `tool_calls`)",
            ));
        }
        let mut blocks = Vec::new();
        if let Some(thinking) = raw.thinking {
            blocks.push(ContentBlock::Thinking(thinking));
        }
        if let Some(text) = raw.text {
            blocks.push(ContentBlock::Text(text));
        }
        for call in raw.tool_calls {
            blocks.push(ContentBlock::ToolCall(call));
        }
        let stop_reason = raw.stop_reason.unwrap_or({
            if blocks
                .iter()
                .any(|block| matches!(block, ContentBlock::ToolCall(_)))
            {
                StopReason::ToolUse
            } else {
                StopReason::Stop
            }
        });
        Ok(Self::Message(AssistantMessage {
            blocks,
            usage: raw.usage,
            stop_reason,
        }))
    }
}

/// Scripted [`Provider`]: replays a fixed sequence of turns and records
/// every request it receives for test assertions.
#[derive(Debug)]
pub struct FakeProvider {
    id: String,
    turns: Arc<Mutex<VecDeque<ScriptTurn>>>,
    requests: Arc<Mutex<Vec<Request>>>,
    /// Delay between emitted events (lets steer/abort tests win races
    /// deterministically). Zero by default.
    delay: Duration,
}

impl FakeProvider {
    /// Build from an inline script.
    pub fn new(turns: Vec<ScriptTurn>) -> Self {
        Self {
            id: "fake".into(),
            turns: Arc::new(Mutex::new(turns.into_iter().collect())),
            requests: Arc::new(Mutex::new(Vec::new())),
            delay: Duration::ZERO,
        }
    }

    /// Build from plain assistant messages.
    pub fn from_messages(messages: Vec<AssistantMessage>) -> Self {
        Self::new(messages.into_iter().map(ScriptTurn::Message).collect())
    }

    /// Build from a JSON script (see the module docs for the shape).
    pub fn from_json_str(script: &str) -> Result<Self, LlmError> {
        let turns: Vec<ScriptTurn> = serde_json::from_str(script)
            .map_err(|err| LlmError::Config(format!("invalid fake script JSON: {err}")))?;
        Ok(Self::new(turns))
    }

    /// Build from a JSON script file.
    pub fn from_json_file(path: impl AsRef<Path>) -> Result<Self, LlmError> {
        let path = path.as_ref();
        let script = std::fs::read_to_string(path).map_err(|err| {
            LlmError::Config(format!("cannot read fake script {}: {err}", path.display()))
        })?;
        Self::from_json_str(&script)
    }

    /// Delay between emitted events (steer/abort test support).
    pub fn with_delay(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }

    /// Override the provider id.
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = id.into();
        self
    }

    /// All requests received so far, in order.
    pub fn recorded_requests(&self) -> Vec<Request> {
        self.requests.lock().expect("fake requests lock").clone()
    }

    /// Number of unplayed turns left in the script.
    pub fn remaining_turns(&self) -> usize {
        self.turns.lock().expect("fake turns lock").len()
    }

    /// Push a turn to the back of the script (mid-test extension).
    pub fn push_turn(&self, turn: ScriptTurn) {
        self.turns.lock().expect("fake turns lock").push_back(turn);
    }
}

/// Split text into deterministic shards of [`SHARD_CHARS`] characters.
fn shard(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    chars
        .chunks(SHARD_CHARS)
        .map(|chunk| chunk.iter().collect())
        .collect()
}

/// Emit one event, respecting the inter-event delay and cancellation.
/// Returns false when the consumer is gone or the request was cancelled.
async fn emit(
    tx: &EventStreamSender,
    event: StreamEvent,
    delay: Duration,
    cancel: &CancellationToken,
) -> bool {
    if delay > Duration::ZERO {
        tokio::select! {
            _ = tokio::time::sleep(delay) => {}
            _ = cancel.cancelled() => return false,
        }
    }
    tx.push(event)
}

#[async_trait]
impl Provider for FakeProvider {
    fn id(&self) -> &str {
        &self.id
    }

    async fn stream(
        &self,
        req: &Request,
        cancel: CancellationToken,
    ) -> Result<EventStream, LlmError> {
        {
            let mut requests = self.requests.lock().expect("fake requests lock");
            requests.push(req.clone());
        }
        let turn = self
            .turns
            .lock()
            .expect("fake turns lock")
            .pop_front()
            .ok_or_else(|| LlmError::Config("fake provider script exhausted".into()))?;

        let (tx, stream) = EventStream::channel_with_cancel(cancel.clone());
        let delay = self.delay;
        tokio::spawn(async move {
            if !emit(&tx, StreamEvent::Start, delay, &cancel).await {
                return;
            }
            match turn {
                ScriptTurn::Message(message) => {
                    for block in &message.blocks {
                        match block {
                            ContentBlock::Text(text) => {
                                for shard in shard(text) {
                                    if !emit(&tx, StreamEvent::TextDelta(shard), delay, &cancel)
                                        .await
                                    {
                                        return;
                                    }
                                }
                            }
                            ContentBlock::Thinking(text) => {
                                for shard in shard(text) {
                                    if !emit(&tx, StreamEvent::ThinkingDelta(shard), delay, &cancel)
                                        .await
                                    {
                                        return;
                                    }
                                }
                            }
                            ContentBlock::ToolCall(call) => {
                                let arguments = call.arguments.to_string();
                                for shard in shard(&arguments) {
                                    if !emit(
                                        &tx,
                                        StreamEvent::ToolCallDelta {
                                            id: call.id.clone(),
                                            partial_json: shard,
                                        },
                                        delay,
                                        &cancel,
                                    )
                                    .await
                                    {
                                        return;
                                    }
                                }
                                if !emit(
                                    &tx,
                                    StreamEvent::ToolCallEnd(call.clone()),
                                    delay,
                                    &cancel,
                                )
                                .await
                                {
                                    return;
                                }
                            }
                            // Images are not streamable as deltas; they
                            // still arrive inside the final Done message.
                            ContentBlock::Image(_) => {}
                        }
                    }
                    tx.push(StreamEvent::Done { message });
                }
                ScriptTurn::Error(error) => {
                    tx.push(StreamEvent::Error(error));
                }
            }
        });
        Ok(stream)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mcode_core::message::ToolCall;
    use serde_json::json;

    fn text_turn(text: &str) -> ScriptTurn {
        ScriptTurn::Message(AssistantMessage {
            blocks: vec![ContentBlock::Text(text.into())],
            usage: None,
            stop_reason: StopReason::Stop,
        })
    }

    #[test]
    fn shard_splits_on_char_boundaries() {
        assert_eq!(shard(""), Vec::<String>::new());
        assert_eq!(shard("short"), vec!["short".to_string()]);
        let shards = shard("0123456789abcdefghij");
        assert_eq!(
            shards,
            vec![String::from("0123456789abcdef"), String::from("ghij")]
        );
        // Multi-byte characters count as single chars.
        let unicode = shard(&"你".repeat(20));
        assert_eq!(unicode.len(), 2);
        assert_eq!(unicode[0].chars().count(), 16);
    }

    #[test]
    fn flat_json_turn_infers_stop_reason() {
        let script: Vec<ScriptTurn> = serde_json::from_str(
            r#"[
                {"text": "hello"},
                {"text": "calling", "tool_calls": [
                    {"id": "c1", "name": "read", "arguments": {"path": "x"}}
                ]}
            ]"#,
        )
        .unwrap();
        let ScriptTurn::Message(plain) = &script[0] else {
            panic!("expected message turn");
        };
        assert_eq!(plain.stop_reason, StopReason::Stop);
        let ScriptTurn::Message(with_call) = &script[1] else {
            panic!("expected message turn");
        };
        assert_eq!(with_call.stop_reason, StopReason::ToolUse);
        assert_eq!(with_call.blocks.len(), 2);
    }

    #[test]
    fn json_turn_roundtrip_exact_form() {
        let turns = vec![
            ScriptTurn::Message(AssistantMessage {
                blocks: vec![
                    ContentBlock::Thinking("hmm".into()),
                    ContentBlock::Text("answer".into()),
                    ContentBlock::ToolCall(ToolCall {
                        id: "c1".into(),
                        name: "bash".into(),
                        arguments: json!({"command": "ls"}),
                    }),
                ],
                usage: Some(Usage {
                    input_tokens: 5,
                    output_tokens: 6,
                }),
                stop_reason: StopReason::ToolUse,
            }),
            ScriptTurn::Error(LlmError::Http {
                status: 429,
                body: "rate limited".into(),
            }),
        ];
        let json = serde_json::to_string(&turns).unwrap();
        let back: Vec<ScriptTurn> = serde_json::from_str(&json).unwrap();
        assert_eq!(back, turns);
    }

    #[test]
    fn error_turn_json_shape() {
        let turns: Vec<ScriptTurn> =
            serde_json::from_str(r#"[{"error": {"http": {"status": 500, "body": "boom"}}}]"#)
                .unwrap();
        assert_eq!(
            turns,
            vec![ScriptTurn::Error(LlmError::Http {
                status: 500,
                body: "boom".into()
            })]
        );
    }

    #[test]
    fn empty_turn_is_rejected() {
        let err = serde_json::from_str::<Vec<ScriptTurn>>(r#"[{}]"#).unwrap_err();
        assert!(err.to_string().contains("script turn"));
    }

    #[test]
    fn message_field_form_is_accepted() {
        let turns: Vec<ScriptTurn> = serde_json::from_str(
            r#"[{"message": {"blocks": [{"Text": "exact"}], "stop_reason": "Stop"}}]"#,
        )
        .unwrap();
        assert_eq!(turns, vec![text_turn("exact")]);
    }
}

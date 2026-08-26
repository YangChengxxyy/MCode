//! Anthropic Messages wire adapter.
//!
//! The adapter owns only `/v1/messages` JSON and SSE translation. Endpoint,
//! authentication, and identity policy come from a [`crate::ProviderProfile`].

use std::collections::HashMap;

use mcode_core::message::{
    AssistantMessage, ContentBlock, Message, ReplayDomain, ReplayState, ReplayWire, StopReason,
    ThinkingBlock, ToolCall, Usage,
};
use serde_json::{Value, json};

use crate::error::LlmError;
use crate::profile::{ModelSettings, WireKind};
use crate::provider::{Request, StreamEvent, ThinkingLevel};

/// Returns the protocol implemented by this adapter.
pub const WIRE_KIND: WireKind = WireKind::AnthropicMessages;

/// Conservative fallback required by the Anthropic Messages API.
const DEFAULT_MAX_TOKENS: u64 = 8_192;

/// Builds an Anthropic Messages request with default model settings.
///
/// `replay` is the consuming profile's trust domain; only its own or
/// explicitly trusted replay state is sent verbatim.
pub fn build_request_body(request: &Request, replay: &ReplayDomain) -> Value {
    build_request_body_with_settings(request, &ModelSettings::default(), replay)
}

pub(crate) fn build_request_body_with_settings(
    request: &Request,
    settings: &ModelSettings,
    replay: &ReplayDomain,
) -> Value {
    let thinking_budget = request
        .thinking
        .map(|thinking| thinking_budget(thinking.level));
    let mut max_tokens = settings.max_output_tokens.unwrap_or(DEFAULT_MAX_TOKENS);
    if let Some(budget) = thinking_budget {
        // Anthropic requires max_tokens to leave room beyond the thinking
        // budget. Keep the caller's larger explicit limit when present.
        max_tokens = max_tokens.max(budget.saturating_add(1_024));
    }

    let mut body = serde_json::Map::new();
    body.insert("model".into(), Value::String(request.model.to_string()));
    body.insert("max_tokens".into(), max_tokens.into());
    body.insert(
        "messages".into(),
        Value::Array(anthropic_messages(&request.messages, replay)),
    );
    body.insert("stream".into(), Value::Bool(true));
    if !request.system_prompt.is_empty() {
        body.insert(
            "system".into(),
            Value::String(request.system_prompt.join("\n\n")),
        );
    }
    if !request.tools.is_empty() {
        body.insert(
            "tools".into(),
            Value::Array(
                request
                    .tools
                    .iter()
                    .map(|tool| {
                        json!({
                            "name": tool.name,
                            "description": tool.description,
                            "input_schema": tool.params_schema,
                        })
                    })
                    .collect(),
            ),
        );
    }
    if let Some(budget_tokens) = thinking_budget {
        body.insert(
            "thinking".into(),
            json!({"type": "enabled", "budget_tokens": budget_tokens}),
        );
    }
    Value::Object(body)
}

fn thinking_budget(level: ThinkingLevel) -> u64 {
    match level {
        ThinkingLevel::Minimal => 1_024,
        ThinkingLevel::Low => 2_048,
        ThinkingLevel::Medium => 4_096,
        ThinkingLevel::High => 8_192,
    }
}

fn anthropic_messages(messages: &[Message], replay: &ReplayDomain) -> Vec<Value> {
    let mut output = Vec::new();
    for message in messages {
        match message {
            Message::User(user) => {
                let content: Vec<Value> =
                    user.content.iter().filter_map(user_content_block).collect();
                push_message(&mut output, "user", content);
            }
            Message::Assistant(assistant) => {
                let content: Vec<Value> = assistant
                    .blocks
                    .iter()
                    .filter_map(|block| match block {
                        // Assistant phase metadata has no Anthropic
                        // representation; the visible text is preserved.
                        ContentBlock::Text(text) => {
                            Some(json!({"type": "text", "text": text.text}))
                        }
                        ContentBlock::ToolCall(call) => Some(json!({
                            "type": "tool_use",
                            "id": call.id,
                            "name": call.name,
                            "input": call.arguments,
                        })),
                        ContentBlock::Thinking(thinking) => {
                            anthropic_thinking_block(thinking, replay)
                        }
                        ContentBlock::Image(_) => None,
                    })
                    .collect();
                push_message(&mut output, "assistant", content);
            }
            Message::ToolResult(result) => {
                let mut result_content: Vec<Value> = result
                    .content
                    .iter()
                    .filter_map(user_content_block)
                    .collect();
                if result_content.is_empty() {
                    result_content.push(json!({"type": "text", "text": "(no tool output)"}));
                }
                push_message(
                    &mut output,
                    "user",
                    vec![json!({
                        "type": "tool_result",
                        "tool_use_id": result.tool_call_id,
                        "content": result_content,
                        "is_error": result.is_error,
                    })],
                );
            }
            Message::Custom(_) => {}
        }
    }
    output
}

fn anthropic_thinking_block(thinking: &ThinkingBlock, replay: &ReplayDomain) -> Option<Value> {
    let own_state = thinking
        .replay
        .as_ref()
        .filter(|state| state.is_replayable_on(replay));
    match own_state {
        Some(state) if state.redacted && !state.data.is_empty() => {
            Some(json!({"type": "redacted_thinking", "data": state.data}))
        }
        Some(state) if !state.data.is_empty() => Some(json!({
            "type": "thinking",
            "thinking": thinking.text,
            "signature": state.data,
        })),
        _ if !thinking.text.is_empty() => {
            // Foreign wire-only state, state from an untrusted profile, or
            // none at all is not valid Anthropic extended-thinking input.
            // Deterministically strip it and preserve the portable visible
            // text instead of shipping an opaque payload across a trust
            // boundary or sending an invalid signature.
            Some(json!({"type": "text", "text": thinking.text}))
        }
        _ => None,
    }
}

fn user_content_block(block: &ContentBlock) -> Option<Value> {
    match block {
        // Anthropic accepts a string here; provider-neutral phase metadata
        // must not turn the wire field into a serialized `TextBlock` object.
        ContentBlock::Text(text) => Some(json!({"type": "text", "text": text.text})),
        ContentBlock::Image(image) => Some(json!({
            "type": "image",
            "source": {
                "type": "base64",
                "media_type": image.mime_type,
                "data": image.data,
            },
        })),
        ContentBlock::Thinking(_) | ContentBlock::ToolCall(_) => None,
    }
}

fn push_message(output: &mut Vec<Value>, role: &str, mut content: Vec<Value>) {
    if content.is_empty() {
        return;
    }
    if let Some(previous) = output.last_mut().filter(|message| message["role"] == role)
        && let Some(previous_content) = previous.get_mut("content").and_then(Value::as_array_mut)
    {
        previous_content.append(&mut content);
        return;
    }
    output.push(json!({"role": role, "content": content}));
}

#[derive(Debug)]
enum AnthropicBlock {
    Text(String),
    Thinking(ThinkingBlock),
    Tool(AnthropicTool),
    Ignored,
}

#[derive(Debug)]
struct AnthropicTool {
    id: String,
    name: String,
    input: String,
    ended: bool,
}

/// Incrementally aggregates Anthropic Messages SSE payloads.
#[derive(Debug, Default)]
pub struct AnthropicAggregator {
    blocks: Vec<AnthropicBlock>,
    block_by_index: HashMap<u64, usize>,
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    stop_reason: Option<StopReason>,
    terminal: bool,
    finished: bool,
}

impl AnthropicAggregator {
    /// Creates an empty aggregator.
    pub fn new() -> Self {
        Self::default()
    }

    /// Decodes one Anthropic SSE data payload.
    ///
    /// # Errors
    ///
    /// Returns [`LlmError::Sse`] for malformed event shapes and
    /// [`LlmError::Http`] for Anthropic `error` events.
    pub fn on_data(&mut self, payload: &str) -> Result<Vec<StreamEvent>, LlmError> {
        let value: Value = serde_json::from_str(payload).map_err(|error| {
            LlmError::Sse(format!(
                "invalid JSON in Anthropic SSE ({error}): {}",
                LlmError::excerpt(payload)
            ))
        })?;
        let event_type = value.get("type").and_then(Value::as_str).ok_or_else(|| {
            LlmError::Sse(format!(
                "Anthropic SSE event has no string type: {}",
                LlmError::excerpt(payload)
            ))
        })?;
        let mut events = Vec::new();
        match event_type {
            "message_start" => {
                if let Some(usage) = value
                    .get("message")
                    .and_then(|message| message.get("usage"))
                {
                    self.capture_usage(usage);
                }
            }
            "content_block_start" => {
                let index = required_index(&value, event_type)?;
                let content_block = value.get("content_block").ok_or_else(|| {
                    LlmError::Sse("Anthropic content_block_start has no content_block".into())
                })?;
                self.start_block(index, content_block, &mut events)?;
            }
            "content_block_delta" => {
                let index = required_index(&value, event_type)?;
                let delta = value.get("delta").ok_or_else(|| {
                    LlmError::Sse("Anthropic content_block_delta has no delta".into())
                })?;
                self.apply_delta(index, delta, &mut events)?;
            }
            "content_block_stop" => {
                let index = required_index(&value, event_type)?;
                self.stop_block(index, &mut events)?;
            }
            "message_delta" => {
                if let Some(reason) = value
                    .get("delta")
                    .and_then(|delta| delta.get("stop_reason"))
                    .and_then(Value::as_str)
                {
                    self.stop_reason = Some(map_stop_reason(reason));
                }
                if let Some(usage) = value.get("usage") {
                    self.capture_usage(usage);
                }
            }
            "message_stop" => self.terminal = true,
            "error" => {
                let error = value.get("error").unwrap_or(&value);
                return Err(LlmError::Http {
                    status: 0,
                    body: LlmError::excerpt(error.to_string()),
                });
            }
            "ping" => {}
            _ => {}
        }
        Ok(events)
    }

    /// Returns whether `message_stop` was decoded.
    pub fn is_terminal(&self) -> bool {
        self.terminal
    }

    /// Finalizes outstanding tools and the normalized assistant message.
    ///
    /// # Errors
    ///
    /// Returns [`LlmError::Sse`] when the stream ended before `message_stop`.
    pub fn finish(&mut self) -> Result<Vec<StreamEvent>, LlmError> {
        if self.finished {
            return Ok(Vec::new());
        }
        if !self.terminal {
            return Err(LlmError::Sse(
                "Anthropic stream ended before message_stop".into(),
            ));
        }
        self.finished = true;
        let mut events = Vec::new();
        for block_index in 0..self.blocks.len() {
            self.end_tool(block_index, &mut events);
        }
        let inferred_tool_use = self
            .blocks
            .iter()
            .any(|block| matches!(block, AnthropicBlock::Tool(_)));
        let mut blocks = Vec::new();
        for block in std::mem::take(&mut self.blocks) {
            match block {
                AnthropicBlock::Text(text) => blocks.push(ContentBlock::Text(text.into())),
                AnthropicBlock::Thinking(thinking) => {
                    blocks.push(ContentBlock::Thinking(thinking));
                }
                AnthropicBlock::Tool(tool) => blocks.push(ContentBlock::ToolCall(ToolCall::new(
                    tool.id,
                    tool.name,
                    parse_arguments(&tool.input),
                ))),
                AnthropicBlock::Ignored => {}
            }
        }
        let usage = if self.input_tokens.is_some() || self.output_tokens.is_some() {
            Some(Usage {
                input_tokens: self.input_tokens.unwrap_or(0),
                output_tokens: self.output_tokens.unwrap_or(0),
            })
        } else {
            None
        };
        events.push(StreamEvent::Done {
            message: AssistantMessage {
                blocks,
                usage,
                stop_reason: self.stop_reason.unwrap_or(if inferred_tool_use {
                    StopReason::ToolUse
                } else {
                    StopReason::Stop
                }),
            },
        });
        Ok(events)
    }

    fn start_block(
        &mut self,
        index: u64,
        content: &Value,
        events: &mut Vec<StreamEvent>,
    ) -> Result<(), LlmError> {
        if self.block_by_index.contains_key(&index) {
            return Err(LlmError::Sse(format!(
                "duplicate Anthropic content block index {index}"
            )));
        }
        let block_type = content
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(|| LlmError::Sse("Anthropic content block has no string type".into()))?;
        let block = match block_type {
            "text" => {
                let initial = content
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if !initial.is_empty() {
                    events.push(StreamEvent::TextDelta(initial.to_owned()));
                }
                AnthropicBlock::Text(initial.to_owned())
            }
            "thinking" => {
                let initial = content
                    .get("thinking")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if !initial.is_empty() {
                    events.push(StreamEvent::ThinkingDelta(initial.to_owned()));
                }
                let mut thinking = ThinkingBlock::new(initial);
                if let Some(signature) = content.get("signature").and_then(Value::as_str) {
                    thinking.replay =
                        Some(ReplayState::new(ReplayWire::AnthropicMessages, signature));
                }
                AnthropicBlock::Thinking(thinking)
            }
            "redacted_thinking" => {
                let data = content
                    .get("data")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let display = "[Reasoning redacted]";
                events.push(StreamEvent::ThinkingDelta(display.to_owned()));
                AnthropicBlock::Thinking(ThinkingBlock::new(display).with_replay(
                    ReplayState::new(ReplayWire::AnthropicMessages, data).with_redacted(true),
                ))
            }
            "tool_use" => {
                let id = content
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or("tool_call")
                    .to_owned();
                let name = content
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                let input = content
                    .get("input")
                    .filter(|input| !input.is_null() && **input != json!({}))
                    .map(Value::to_string)
                    .unwrap_or_default();
                if !input.is_empty() {
                    events.push(StreamEvent::ToolCallDelta {
                        id: id.clone(),
                        partial_json: input.clone(),
                    });
                }
                AnthropicBlock::Tool(AnthropicTool {
                    id,
                    name,
                    input,
                    ended: false,
                })
            }
            _ => AnthropicBlock::Ignored,
        };
        let block_index = self.blocks.len();
        self.blocks.push(block);
        self.block_by_index.insert(index, block_index);
        Ok(())
    }

    fn apply_delta(
        &mut self,
        index: u64,
        delta: &Value,
        events: &mut Vec<StreamEvent>,
    ) -> Result<(), LlmError> {
        let block_index = self.block_index(index)?;
        let delta_type = delta
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(|| LlmError::Sse("Anthropic content delta has no string type".into()))?;
        match delta_type {
            "text_delta" => {
                let text = required_delta_string(delta, "text", delta_type)?;
                let AnthropicBlock::Text(buffer) = &mut self.blocks[block_index] else {
                    return Err(LlmError::Sse(format!(
                        "text delta targets non-text block {index}"
                    )));
                };
                buffer.push_str(text);
                events.push(StreamEvent::TextDelta(text.to_owned()));
            }
            "thinking_delta" => {
                let delta_text = required_delta_string(delta, "thinking", delta_type)?;
                let AnthropicBlock::Thinking(thinking) = &mut self.blocks[block_index] else {
                    return Err(LlmError::Sse(format!(
                        "thinking delta targets non-thinking block {index}"
                    )));
                };
                thinking.text.push_str(delta_text);
                events.push(StreamEvent::ThinkingDelta(delta_text.to_owned()));
            }
            "input_json_delta" => {
                let partial = required_delta_string(delta, "partial_json", delta_type)?;
                let AnthropicBlock::Tool(tool) = &mut self.blocks[block_index] else {
                    return Err(LlmError::Sse(format!(
                        "input JSON delta targets non-tool block {index}"
                    )));
                };
                tool.input.push_str(partial);
                events.push(StreamEvent::ToolCallDelta {
                    id: tool.id.clone(),
                    partial_json: partial.to_owned(),
                });
            }
            "signature_delta" => {
                let signature = required_delta_string(delta, "signature", delta_type)?;
                let AnthropicBlock::Thinking(thinking) = &mut self.blocks[block_index] else {
                    return Err(LlmError::Sse(format!(
                        "signature delta targets non-thinking block {index}"
                    )));
                };
                thinking
                    .replay
                    .get_or_insert_with(|| {
                        ReplayState::new(ReplayWire::AnthropicMessages, String::new())
                    })
                    .data
                    .push_str(signature);
            }
            _ => {}
        }
        Ok(())
    }

    fn stop_block(&mut self, index: u64, events: &mut Vec<StreamEvent>) -> Result<(), LlmError> {
        let block_index = self.block_index(index)?;
        self.end_tool(block_index, events);
        Ok(())
    }

    fn end_tool(&mut self, block_index: usize, events: &mut Vec<StreamEvent>) {
        let AnthropicBlock::Tool(tool) = &mut self.blocks[block_index] else {
            return;
        };
        if tool.ended {
            return;
        }
        tool.ended = true;
        events.push(StreamEvent::ToolCallEnd(ToolCall::new(
            tool.id.clone(),
            tool.name.clone(),
            parse_arguments(&tool.input),
        )));
    }

    fn block_index(&self, index: u64) -> Result<usize, LlmError> {
        self.block_by_index.get(&index).copied().ok_or_else(|| {
            LlmError::Sse(format!("Anthropic delta references unknown block {index}"))
        })
    }

    fn capture_usage(&mut self, usage: &Value) {
        if let Some(input) = usage.get("input_tokens").and_then(Value::as_u64) {
            self.input_tokens = Some(input);
        }
        if let Some(output) = usage.get("output_tokens").and_then(Value::as_u64) {
            self.output_tokens = Some(output);
        }
    }
}

fn required_index(value: &Value, event_type: &str) -> Result<u64, LlmError> {
    value.get("index").and_then(Value::as_u64).ok_or_else(|| {
        LlmError::Sse(format!(
            "Anthropic event '{event_type}' has no numeric index"
        ))
    })
}

fn required_delta_string<'a>(
    delta: &'a Value,
    field: &str,
    delta_type: &str,
) -> Result<&'a str, LlmError> {
    delta.get(field).and_then(Value::as_str).ok_or_else(|| {
        LlmError::Sse(format!(
            "Anthropic delta '{delta_type}' has no string '{field}'"
        ))
    })
}

fn parse_arguments(raw: &str) -> Value {
    if raw.trim().is_empty() {
        json!({})
    } else {
        serde_json::from_str(raw).unwrap_or(Value::Null)
    }
}

fn map_stop_reason(reason: &str) -> StopReason {
    match reason {
        "tool_use" => StopReason::ToolUse,
        "max_tokens" => StopReason::Length,
        "refusal" => StopReason::Error,
        "end_turn" | "stop_sequence" | "pause_turn" => StopReason::Stop,
        _ => StopReason::Stop,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mcode_core::message::{BinaryData, TextBlock, ToolResultMessage, UserMessage};
    use mcode_core::tool::ToolSpec;

    /// Domain of the built-in `anthropic` profile, as the transport
    /// would derive it (id plus effective endpoint origin).
    const OWN_ENDPOINT: &str = "https://api.anthropic.com";

    fn own_domain() -> ReplayDomain {
        ReplayDomain::new(ReplayWire::AnthropicMessages, "anthropic", OWN_ENDPOINT)
    }

    #[test]
    fn request_uses_top_level_system_tools_and_tool_results() {
        let request = Request::new("claude-test")
            .with_system_prompt("one")
            .with_system_prompt("two")
            .with_message(Message::User(UserMessage {
                content: vec![
                    ContentBlock::Text("look".into()),
                    ContentBlock::Image(BinaryData {
                        data: "AAEC".into(),
                        mime_type: "image/png".into(),
                    }),
                ],
            }))
            .with_message(Message::Assistant(AssistantMessage {
                blocks: vec![
                    ContentBlock::Thinking(
                        ThinkingBlock::new("checked").with_replay(
                            ReplayState::new(ReplayWire::AnthropicMessages, "thinking-signature")
                                .with_provider("anthropic")
                                .with_endpoint(OWN_ENDPOINT),
                        ),
                    ),
                    ContentBlock::ToolCall(ToolCall::new("tool_1", "read", json!({"path": "x"}))),
                ],
                usage: None,
                stop_reason: StopReason::ToolUse,
            }))
            .with_message(Message::ToolResult(ToolResultMessage {
                tool_call_id: "tool_1".into(),
                content: vec![ContentBlock::Text("contents".into())],
                is_error: false,
                details: None,
            }))
            .with_tool(ToolSpec {
                name: "read".into(),
                description: "read".into(),
                params_schema: json!({"type":"object"}),
            });
        let body = build_request_body(&request, &own_domain());
        assert_eq!(body["system"], "one\n\ntwo");
        assert_eq!(body["max_tokens"], DEFAULT_MAX_TOKENS);
        assert_eq!(body["tools"][0]["input_schema"], json!({"type":"object"}));
        assert_eq!(body["messages"][1]["content"][0]["type"], "thinking");
        assert_eq!(
            body["messages"][1]["content"][0]["signature"],
            "thinking-signature"
        );
        assert_eq!(body["messages"][1]["content"][1]["type"], "tool_use");
        assert_eq!(body["messages"][2]["content"][0]["type"], "tool_result");
        assert_eq!(
            body["messages"][0]["content"][1]["source"]["media_type"],
            "image/png"
        );
    }

    #[test]
    fn partial_json_text_thinking_usage_and_stop_reason_roundtrip() {
        let payloads = [
            json!({"type":"message_start","message":{"usage":{"input_tokens":11,"output_tokens":0}}}),
            json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"hmm"}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"sig-part-1"}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"-part-2"}}),
            json!({"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}),
            json!({"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"answer"}}),
            json!({"type":"content_block_start","index":2,"content_block":{"type":"tool_use","id":"tool_1","name":"read","input":{}}}),
            json!({"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"{\"pa"}}),
            json!({"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"th\":\"x\"}"}}),
            json!({"type":"content_block_stop","index":2}),
            json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":7}}),
            json!({"type":"message_stop"}),
        ];
        let mut aggregator = AnthropicAggregator::new();
        let mut events = Vec::new();
        for payload in payloads {
            events.extend(aggregator.on_data(&payload.to_string()).unwrap());
        }
        assert!(aggregator.is_terminal());
        events.extend(aggregator.finish().unwrap());
        assert!(events.contains(&StreamEvent::ThinkingDelta("hmm".into())));
        assert!(events.contains(&StreamEvent::TextDelta("answer".into())));
        let StreamEvent::Done { message } = events.last().unwrap() else {
            panic!("expected Done");
        };
        assert_eq!(message.stop_reason, StopReason::ToolUse);
        let ContentBlock::Thinking(thinking) = &message.blocks[0] else {
            panic!("expected thinking block");
        };
        assert_eq!(thinking.text, "hmm");
        let replay = thinking.replay.as_ref().expect("replay state");
        assert_eq!(replay.wire, ReplayWire::AnthropicMessages);
        assert!(
            replay.provider.is_none(),
            "aggregators do not know the profile"
        );
        assert_eq!(replay.data, "sig-part-1-part-2");
        assert!(!replay.redacted);
        assert_eq!(
            message.usage,
            Some(Usage {
                input_tokens: 11,
                output_tokens: 7
            })
        );
        assert_eq!(
            message.blocks[2],
            ContentBlock::ToolCall(ToolCall::new("tool_1", "read", json!({"path":"x"}),))
        );
    }

    #[test]
    fn redacted_thinking_roundtrips_encrypted_payload() {
        let mut aggregator = AnthropicAggregator::new();
        for payload in [
            json!({"type":"content_block_start","index":0,"content_block":{"type":"redacted_thinking","data":"encrypted-state"}}),
            json!({"type":"content_block_stop","index":0}),
            json!({"type":"message_delta","delta":{"stop_reason":"tool_use"}}),
            json!({"type":"message_stop"}),
        ] {
            aggregator.on_data(&payload.to_string()).unwrap();
        }
        let events = aggregator.finish().unwrap();
        let StreamEvent::Done { message } = events.last().unwrap() else {
            panic!("expected Done");
        };
        let ContentBlock::Thinking(thinking) = &message.blocks[0] else {
            panic!("expected thinking block");
        };
        let replay = thinking.replay.as_ref().expect("replay state");
        assert!(replay.redacted);
        assert_eq!(replay.wire, ReplayWire::AnthropicMessages);
        assert_eq!(replay.data, "encrypted-state");

        // The transport stamps the producing profile and endpoint onto
        // delivered state; simulate that before replaying the persisted
        // message.
        let mut message = message.clone();
        let ContentBlock::Thinking(thinking) = &mut message.blocks[0] else {
            panic!("expected thinking block");
        };
        let replay = thinking.replay.as_mut().expect("replay state");
        replay.provider = Some("anthropic".into());
        replay.endpoint = Some(OWN_ENDPOINT.into());
        let request = Request::new("claude-test").with_message(Message::Assistant(message));
        let body = build_request_body(&request, &own_domain());
        assert_eq!(
            body["messages"][0]["content"][0],
            json!({"type":"redacted_thinking","data":"encrypted-state"})
        );
    }

    #[test]
    fn untrusted_replay_state_is_stripped_but_own_and_trusted_replay() {
        // Responses reasoning state crossing to the Anthropic wire must be
        // downgraded to plain text, never sent as a thinking signature.
        let foreign = ThinkingBlock::new("checked upstream").with_replay(
            ReplayState::new(
                ReplayWire::OpenAiResponses,
                r#"{"type":"reasoning","id":"rs_1","encrypted_content":"opaque"}"#,
            )
            .with_provider("openai"),
        );
        let request =
            Request::new("claude-test").with_message(Message::Assistant(AssistantMessage {
                blocks: vec![ContentBlock::Thinking(foreign)],
                usage: None,
                stop_reason: StopReason::ToolUse,
            }));
        let body = build_request_body(&request, &own_domain());
        let content = &body["messages"][0]["content"];
        assert_eq!(
            content[0],
            json!({"type": "text", "text": "checked upstream"})
        );
        assert!(!content.to_string().contains("signature"));
        assert!(!content.to_string().contains("encrypted"));

        // Same wire but a different, untrusted profile: the signature must
        // not cross the trust boundary, so it is also downgraded.
        let other_profile = ThinkingBlock::new("gateway thought").with_replay(
            ReplayState::new(ReplayWire::AnthropicMessages, "gateway-signature")
                .with_provider("anthropic-gateway")
                .with_endpoint("https://gateway.example"),
        );
        let request =
            Request::new("claude-test").with_message(Message::Assistant(AssistantMessage {
                blocks: vec![ContentBlock::Thinking(other_profile)],
                usage: None,
                stop_reason: StopReason::ToolUse,
            }));
        let body = build_request_body(&request, &own_domain());
        assert_eq!(
            body["messages"][0]["content"][0],
            json!({"type": "text", "text": "gateway thought"})
        );

        // The producing profile replays its own signature verbatim, and a
        // profile explicitly trusting that producer replays it too
        // regardless of which host that producer pointed at.
        let own = ThinkingBlock::new("thought").with_replay(
            ReplayState::new(ReplayWire::AnthropicMessages, "own-signature")
                .with_provider("anthropic")
                .with_endpoint(OWN_ENDPOINT),
        );
        let request =
            Request::new("claude-test").with_message(Message::Assistant(AssistantMessage {
                blocks: vec![ContentBlock::Thinking(own)],
                usage: None,
                stop_reason: StopReason::ToolUse,
            }));
        let body = build_request_body(&request, &own_domain());
        assert_eq!(
            body["messages"][0]["content"][0]["signature"],
            json!("own-signature")
        );
        let trusted = ThinkingBlock::new("thought").with_replay(
            ReplayState::new(ReplayWire::AnthropicMessages, "gateway-signature")
                .with_provider("anthropic-gateway")
                .with_endpoint("https://gateway.example"),
        );
        let request =
            Request::new("claude-test").with_message(Message::Assistant(AssistantMessage {
                blocks: vec![ContentBlock::Thinking(trusted)],
                usage: None,
                stop_reason: StopReason::ToolUse,
            }));
        let body = build_request_body(&request, &own_domain().with_trusted("anthropic-gateway"));
        assert_eq!(
            body["messages"][0]["content"][0]["signature"],
            json!("gateway-signature")
        );
    }

    #[test]
    fn phases_have_no_anthropic_representation_but_all_text_survives() {
        let phase = mcode_core::message::AssistantPhase::Commentary;
        let request = Request::new("claude-test")
            .with_message(Message::User(UserMessage {
                content: vec![ContentBlock::Text(
                    TextBlock::new("question").with_phase(phase),
                )],
            }))
            .with_message(Message::Assistant(AssistantMessage {
                blocks: vec![
                    ContentBlock::Text(TextBlock::new("preamble").with_phase(phase)),
                    ContentBlock::ToolCall(ToolCall::new("tool_1", "read", json!({}))),
                ],
                usage: None,
                stop_reason: StopReason::ToolUse,
            }))
            .with_message(Message::ToolResult(ToolResultMessage {
                tool_call_id: "tool_1".into(),
                content: vec![ContentBlock::Text(
                    TextBlock::new("tool output").with_phase(phase),
                )],
                is_error: false,
                details: None,
            }));
        let body = build_request_body(&request, &own_domain());
        assert_eq!(
            body["messages"][0]["content"][0],
            json!({"type": "text", "text": "question"})
        );
        assert_eq!(
            body["messages"][1]["content"][0],
            json!({"type": "text", "text": "preamble"})
        );
        assert_eq!(
            body["messages"][2]["content"][0]["content"][0],
            json!({"type": "text", "text": "tool output"})
        );
        assert!(!body.to_string().contains("phase"));
    }

    #[test]
    fn malformed_error_and_unterminated_stream_fail() {
        let mut aggregator = AnthropicAggregator::new();
        assert!(matches!(aggregator.on_data("{"), Err(LlmError::Sse(_))));
        let error = aggregator
            .on_data(
                &json!({"type":"error","error":{"type":"overloaded_error","message":"busy"}})
                    .to_string(),
            )
            .unwrap_err();
        assert!(matches!(error, LlmError::Http { status: 0, .. }));
        assert!(matches!(aggregator.finish(), Err(LlmError::Sse(_))));
    }
}

// Rust guideline compliant 2026-08-26

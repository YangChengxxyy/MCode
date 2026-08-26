//! OpenAI Responses wire adapter.
//!
//! This module serializes provider-neutral requests and incrementally decodes
//! Responses SSE events. It contains no provider ids or endpoint configuration.

use std::collections::HashMap;

use mcode_core::message::{
    AssistantMessage, AssistantPhase, ContentBlock, Message, ReplayDomain, ReplayState, ReplayWire,
    StopReason, TextBlock, ThinkingBlock, ToolCall, Usage,
};
use serde_json::{Value, json};

use crate::error::LlmError;
use crate::profile::{ModelSettings, WireKind};
use crate::provider::{Request, StreamEvent};

/// Returns the protocol implemented by this adapter.
pub const WIRE_KIND: WireKind = WireKind::OpenAiResponses;

/// Builds a streaming Responses request with default model settings.
///
/// `replay` is the consuming profile's trust domain; only its own or
/// explicitly trusted reasoning state is replayed verbatim.
pub fn build_request_body(request: &Request, replay: &ReplayDomain) -> Value {
    build_request_body_with_settings(request, &ModelSettings::default(), replay)
}

pub(crate) fn build_request_body_with_settings(
    request: &Request,
    settings: &ModelSettings,
    replay: &ReplayDomain,
) -> Value {
    let mut body = serde_json::Map::new();
    body.insert("model".into(), Value::String(request.model.to_string()));
    body.insert(
        "input".into(),
        Value::Array(response_input(&request.messages, replay)),
    );
    body.insert("stream".into(), Value::Bool(true));
    // Conversation history is managed locally. Request encrypted reasoning on
    // every Responses call so reasoning models remain statelessly replayable,
    // including when they reason by default without an explicit effort setting.
    body.insert("store".into(), Value::Bool(false));
    body.insert("include".into(), json!(["reasoning.encrypted_content"]));

    if !request.system_prompt.is_empty() {
        body.insert(
            "instructions".into(),
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
                        if tool.name == "apply_patch" {
                            json!({
                                "type": "custom",
                                "name": tool.name,
                                "description": tool.description,
                            })
                        } else {
                            json!({
                                "type": "function",
                                "name": tool.name,
                                "description": tool.description,
                                "parameters": tool.params_schema,
                            })
                        }
                    })
                    .collect(),
            ),
        );
    }
    if let Some(thinking) = request.thinking {
        body.insert(
            "reasoning".into(),
            json!({"effort": thinking.level.as_effort_str()}),
        );
    }
    if let Some(max_output_tokens) = settings.max_output_tokens {
        body.insert("max_output_tokens".into(), max_output_tokens.into());
    }
    Value::Object(body)
}

fn response_input(messages: &[Message], replay: &ReplayDomain) -> Vec<Value> {
    let custom_calls = custom_call_ids(messages);
    let mut input = Vec::new();
    for message in messages {
        match message {
            Message::User(user) => {
                let content: Vec<Value> = user
                    .content
                    .iter()
                    .filter_map(|block| match block {
                        ContentBlock::Text(text) => {
                            Some(json!({"type": "input_text", "text": text.text}))
                        }
                        ContentBlock::Image(image) => Some(json!({
                            "type": "input_image",
                            "image_url": format!(
                                "data:{};base64,{}",
                                image.mime_type, image.data
                            ),
                        })),
                        ContentBlock::Thinking(_) | ContentBlock::ToolCall(_) => None,
                    })
                    .collect();
                if !content.is_empty() {
                    input.push(json!({"role": "user", "content": content}));
                }
            }
            Message::Assistant(assistant) => {
                for block in &assistant.blocks {
                    match block {
                        ContentBlock::Text(text) => push_assistant_input_text(&mut input, text),
                        ContentBlock::Thinking(thinking) => {
                            if let Some(item) = openai_reasoning_item(thinking, replay) {
                                input.push(item);
                            } else if !thinking.text.is_empty() {
                                // Unsigned reasoning cannot be reconstructed as a reasoning item.
                                // Preserve it as valid assistant input rather than silently dropping it.
                                push_assistant_input_text(
                                    &mut input,
                                    &TextBlock::new(thinking.text.clone()),
                                );
                            }
                        }
                        ContentBlock::ToolCall(call) => {
                            if call.name == "apply_patch" {
                                let mut item = json!({
                                    "type": "custom_tool_call",
                                    "call_id": call.id,
                                    "name": call.name,
                                    "input": custom_input(&call.arguments),
                                });
                                attach_item_id(&mut item, call.item_id.as_deref());
                                input.push(item);
                            } else {
                                let mut item = json!({
                                    "type": "function_call",
                                    "call_id": call.id,
                                    "name": call.name,
                                    "arguments": call.arguments.to_string(),
                                });
                                attach_item_id(&mut item, call.item_id.as_deref());
                                input.push(item);
                            }
                        }
                        ContentBlock::Image(_) => {}
                    }
                }
            }
            Message::ToolResult(result) => {
                let output = tool_output(&result.content);
                let item_type = if custom_calls.contains_key(&result.tool_call_id) {
                    "custom_tool_call_output"
                } else {
                    "function_call_output"
                };
                input.push(json!({
                    "type": item_type,
                    "call_id": result.tool_call_id,
                    "output": output,
                }));
                let images: Vec<Value> = result
                    .content
                    .iter()
                    .filter_map(|block| match block {
                        ContentBlock::Image(image) => Some(json!({
                            "type": "input_image",
                            "image_url": format!(
                                "data:{};base64,{}",
                                image.mime_type, image.data
                            ),
                        })),
                        _ => None,
                    })
                    .collect();
                if !images.is_empty() {
                    input.push(json!({"role": "user", "content": images}));
                }
            }
            Message::Custom(_) => {}
        }
    }
    input
}

/// Appends one assistant message item, preserving a captured phase.
///
/// OpenAI's Responses contract requires manually replayed assistant
/// history to carry the original `phase` so commentary segments are
/// not mistaken for final answers in tool-dense flows. Phase-less
/// text keeps the plain `input_text` shape.
fn push_assistant_input_text(input: &mut Vec<Value>, text: &TextBlock) {
    if text.text.is_empty() {
        return;
    }
    let mut item = serde_json::Map::new();
    item.insert("role".into(), Value::String("assistant".into()));
    if let Some(phase) = text.phase {
        item.insert("phase".into(), Value::String(phase.as_str().to_owned()));
    }
    item.insert(
        "content".into(),
        json!([{"type": "input_text", "text": text.text}]),
    );
    input.push(Value::Object(item));
}

fn openai_reasoning_item(thinking: &ThinkingBlock, replay: &ReplayDomain) -> Option<Value> {
    let state = thinking.replay.as_ref()?;
    // Foreign wire state (e.g. an Anthropic signature) or state from an
    // untrusted profile is never a valid reasoning item; fall back to the
    // text downgrade instead.
    if !state.is_replayable_on(replay) {
        return None;
    }
    let item = serde_json::from_str::<Value>(&state.data).ok()?;
    (item.get("type").and_then(Value::as_str) == Some("reasoning")).then_some(item)
}

fn attach_item_id(item: &mut Value, item_id: Option<&str>) {
    if let Some(item_id) = item_id.filter(|id| !id.is_empty()) {
        item["id"] = Value::String(item_id.to_owned());
    }
}

fn custom_call_ids(messages: &[Message]) -> HashMap<String, ()> {
    messages
        .iter()
        .filter_map(|message| match message {
            Message::Assistant(assistant) => Some(&assistant.blocks),
            _ => None,
        })
        .flatten()
        .filter_map(|block| match block {
            ContentBlock::ToolCall(call) if call.name == "apply_patch" => {
                Some((call.id.clone(), ()))
            }
            _ => None,
        })
        .collect()
}

fn custom_input(arguments: &Value) -> String {
    arguments
        .as_str()
        .map_or_else(|| arguments.to_string(), str::to_owned)
}

fn tool_output(blocks: &[ContentBlock]) -> String {
    let text = blocks
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    if text.is_empty() {
        if blocks
            .iter()
            .any(|block| matches!(block, ContentBlock::Image(_)))
        {
            "(see attached image)".into()
        } else {
            "(no tool output)".into()
        }
    } else {
        text
    }
}

#[derive(Debug)]
enum BlockAcc {
    Text(TextBlock),
    Thinking(ThinkingBlock),
    Tool(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ContentKind {
    Text,
    Thinking,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToolKind {
    Function,
    Custom,
}

#[derive(Debug)]
struct ToolSlot {
    item_id: String,
    call_id: String,
    name: String,
    arguments: String,
    kind: ToolKind,
    ended: bool,
}

/// Incrementally aggregates OpenAI Responses SSE payloads.
#[derive(Debug, Default)]
pub struct ResponsesAggregator {
    blocks: Vec<BlockAcc>,
    content_by_item: HashMap<String, usize>,
    content_by_output: HashMap<u64, usize>,
    tools: Vec<ToolSlot>,
    tool_by_item: HashMap<String, usize>,
    tool_by_output: HashMap<u64, usize>,
    usage: Option<Usage>,
    stop_reason: Option<StopReason>,
    refused: bool,
    terminal: bool,
    finished: bool,
}

impl ResponsesAggregator {
    /// Creates an empty aggregator.
    pub fn new() -> Self {
        Self::default()
    }

    /// Decodes one Responses SSE data payload.
    ///
    /// # Errors
    ///
    /// Returns [`LlmError::Sse`] for malformed JSON/event shapes and
    /// [`LlmError::Http`] for streamed API failures.
    pub fn on_data(&mut self, payload: &str) -> Result<Vec<StreamEvent>, LlmError> {
        let value: Value = serde_json::from_str(payload).map_err(|error| {
            LlmError::Sse(format!(
                "invalid JSON in Responses SSE ({error}): {}",
                LlmError::excerpt(payload)
            ))
        })?;
        let event_type = value.get("type").and_then(Value::as_str).ok_or_else(|| {
            LlmError::Sse(format!(
                "Responses SSE event has no string type: {}",
                LlmError::excerpt(payload)
            ))
        })?;
        let mut events = Vec::new();
        match event_type {
            "response.output_text.delta" => {
                let delta = required_string(&value, "delta", event_type)?;
                let slot = self.resolve_event_content(&value, ContentKind::Text);
                self.append_text(slot, delta, &mut events);
            }
            "response.output_text.done" => {
                let complete = value
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let slot = self.resolve_event_content(&value, ContentKind::Text);
                self.set_complete_text(slot, complete, &mut events);
            }
            "response.refusal.delta" => {
                let delta = required_string(&value, "delta", event_type)?;
                let slot = self.resolve_event_content(&value, ContentKind::Text);
                self.refused = true;
                self.stop_reason = Some(StopReason::Error);
                self.append_text(slot, delta, &mut events);
            }
            "response.refusal.done" => {
                let complete = value
                    .get("refusal")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let slot = self.resolve_event_content(&value, ContentKind::Text);
                self.refused = true;
                self.stop_reason = Some(StopReason::Error);
                self.set_complete_text(slot, complete, &mut events);
            }
            "response.reasoning_text.delta" | "response.reasoning_summary_text.delta" => {
                let delta = required_string(&value, "delta", event_type)?;
                let slot = self.resolve_event_content(&value, ContentKind::Thinking);
                self.append_thinking(slot, delta, &mut events);
            }
            "response.output_item.added" => {
                if let Some(item) = value.get("item") {
                    self.add_output_item(
                        item,
                        value.get("output_index").and_then(Value::as_u64),
                        &mut events,
                    )?;
                }
            }
            "response.output_item.done" => {
                if let Some(item) = value.get("item") {
                    self.finish_output_item(
                        item,
                        value.get("output_index").and_then(Value::as_u64),
                        &mut events,
                    )?;
                }
            }
            "response.function_call_arguments.delta" => {
                let fragment = required_string(&value, "delta", event_type)?;
                let slot = self.resolve_event_tool(&value, ToolKind::Function);
                self.append_tool_delta(slot, fragment, &mut events);
            }
            "response.function_call_arguments.done" => {
                let complete = value
                    .get("arguments")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let slot = self.resolve_event_tool(&value, ToolKind::Function);
                self.set_complete_arguments(slot, complete, &mut events);
                self.end_tool(slot, &mut events);
            }
            "response.custom_tool_call_input.delta" => {
                let fragment = required_string(&value, "delta", event_type)?;
                let slot = self.resolve_event_tool(&value, ToolKind::Custom);
                self.append_tool_delta(slot, fragment, &mut events);
            }
            "response.custom_tool_call_input.done" => {
                let complete = value
                    .get("input")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let slot = self.resolve_event_tool(&value, ToolKind::Custom);
                self.set_complete_arguments(slot, complete, &mut events);
                self.end_tool(slot, &mut events);
            }
            "response.completed" => {
                self.capture_response(value.get("response"), &mut events)?;
                self.terminal = true;
            }
            "response.incomplete" => {
                self.capture_response(value.get("response"), &mut events)?;
                if self.stop_reason.is_none() {
                    self.stop_reason = Some(StopReason::Length);
                }
                self.terminal = true;
            }
            "response.failed" | "error" => {
                let error = value
                    .get("response")
                    .and_then(|response| response.get("error"))
                    .or_else(|| value.get("error"))
                    .unwrap_or(&value);
                return Err(LlmError::Http {
                    status: 0,
                    body: LlmError::excerpt(error.to_string()),
                });
            }
            // Lifecycle and content-part marker events carry no deltas.
            "response.created"
            | "response.in_progress"
            | "response.content_part.added"
            | "response.content_part.done"
            | "response.reasoning_summary_part.added"
            | "response.reasoning_summary_part.done" => {}
            _ => {}
        }
        Ok(events)
    }

    /// Returns whether a completed/incomplete terminal event was decoded.
    pub fn is_terminal(&self) -> bool {
        self.terminal
    }

    /// Finalizes outstanding calls and the normalized assistant message.
    ///
    /// # Errors
    ///
    /// Returns [`LlmError::Sse`] when the byte stream ended without a
    /// Responses terminal event.
    pub fn finish(&mut self) -> Result<Vec<StreamEvent>, LlmError> {
        if self.finished {
            return Ok(Vec::new());
        }
        if !self.terminal {
            return Err(LlmError::Sse(
                "Responses stream ended before completed/incomplete event".into(),
            ));
        }
        self.finished = true;
        let mut events = Vec::new();
        for index in 0..self.tools.len() {
            self.end_tool(index, &mut events);
        }
        let mut blocks = Vec::new();
        for block in std::mem::take(&mut self.blocks) {
            match block {
                BlockAcc::Text(text) => blocks.push(ContentBlock::Text(text)),
                BlockAcc::Thinking(thinking) => blocks.push(ContentBlock::Thinking(thinking)),
                BlockAcc::Tool(index) => {
                    blocks.push(ContentBlock::ToolCall(self.tool_call(index)));
                }
            }
        }
        let stop_reason = self.stop_reason.unwrap_or({
            if self.refused {
                StopReason::Error
            } else if self.tools.is_empty() {
                StopReason::Stop
            } else {
                StopReason::ToolUse
            }
        });
        events.push(StreamEvent::Done {
            message: AssistantMessage {
                blocks,
                usage: self.usage,
                stop_reason,
            },
        });
        Ok(events)
    }

    fn add_output_item(
        &mut self,
        item: &Value,
        output_index: Option<u64>,
        events: &mut Vec<StreamEvent>,
    ) -> Result<(), LlmError> {
        if let Some(kind) = content_kind(item) {
            let slot = self.resolve_item_content(item, output_index, kind);
            if kind == ContentKind::Text {
                self.apply_message_phase(slot, item);
            }
            return Ok(());
        }
        let Some(kind) = tool_kind(item) else {
            return Ok(());
        };
        let slot = self.resolve_item_tool(item, output_index, kind);
        self.update_tool_identity(slot, item);
        if let Some(initial) = item_arguments(item, kind).filter(|value| !value.is_empty()) {
            self.append_tool_delta(slot, initial, events);
        }
        Ok(())
    }

    fn finish_output_item(
        &mut self,
        item: &Value,
        output_index: Option<u64>,
        events: &mut Vec<StreamEvent>,
    ) -> Result<(), LlmError> {
        if let Some(kind) = content_kind(item) {
            let slot = self.resolve_item_content(item, output_index, kind);
            match kind {
                ContentKind::Text => {
                    let (complete, refused) = message_content(item);
                    self.apply_message_phase(slot, item);
                    if refused {
                        self.refused = true;
                        self.stop_reason = Some(StopReason::Error);
                    }
                    self.set_complete_text(slot, &complete, events);
                }
                ContentKind::Thinking => {
                    let complete = reasoning_text(item);
                    let replay = Some(ReplayState::new(
                        ReplayWire::OpenAiResponses,
                        item.to_string(),
                    ));
                    self.set_complete_thinking(slot, &complete, replay, events);
                }
            }
            return Ok(());
        }

        let Some(kind) = tool_kind(item) else {
            return Ok(());
        };
        let slot = self.resolve_item_tool(item, output_index, kind);
        self.update_tool_identity(slot, item);
        if let Some(complete) = item_arguments(item, kind) {
            self.set_complete_arguments(slot, complete, events);
        }
        self.end_tool(slot, events);
        Ok(())
    }

    fn resolve_event_content(&mut self, event: &Value, kind: ContentKind) -> usize {
        let item_id = event
            .get("item_id")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let output_index = event.get("output_index").and_then(Value::as_u64);
        self.resolve_content(item_id, output_index, kind)
    }

    fn resolve_item_content(
        &mut self,
        item: &Value,
        output_index: Option<u64>,
        kind: ContentKind,
    ) -> usize {
        let item_id = item.get("id").and_then(Value::as_str).unwrap_or_default();
        self.resolve_content(item_id, output_index, kind)
    }

    fn resolve_content(
        &mut self,
        item_id: &str,
        output_index: Option<u64>,
        kind: ContentKind,
    ) -> usize {
        if let Some(&slot) = self
            .content_by_item
            .get(item_id)
            .filter(|_| !item_id.is_empty())
        {
            return slot;
        }
        if let Some(slot) =
            output_index.and_then(|index| self.content_by_output.get(&index).copied())
        {
            if !item_id.is_empty() {
                self.content_by_item.insert(item_id.to_owned(), slot);
            }
            return slot;
        }
        if item_id.is_empty()
            && output_index.is_none()
            && let Some((slot, _)) = self
                .blocks
                .iter()
                .enumerate()
                .next_back()
                .filter(|(_, block)| block_content_kind(block) == Some(kind))
        {
            return slot;
        }
        self.push_content(item_id, output_index, kind)
    }

    fn push_content(
        &mut self,
        item_id: &str,
        output_index: Option<u64>,
        kind: ContentKind,
    ) -> usize {
        let slot = self.blocks.len();
        let block = match kind {
            ContentKind::Text => BlockAcc::Text(TextBlock::new(String::new())),
            ContentKind::Thinking => BlockAcc::Thinking(ThinkingBlock::new(String::new())),
        };
        self.blocks.push(block);
        if !item_id.is_empty() {
            self.content_by_item.insert(item_id.to_owned(), slot);
        }
        if let Some(output_index) = output_index {
            self.content_by_output.insert(output_index, slot);
        }
        slot
    }

    fn resolve_event_tool(&mut self, event: &Value, kind: ToolKind) -> usize {
        let item_id = event
            .get("item_id")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let output_index = event.get("output_index").and_then(Value::as_u64);
        if let Some(&slot) = self
            .tool_by_item
            .get(item_id)
            .filter(|_| !item_id.is_empty())
        {
            return slot;
        }
        if let Some(slot) = output_index.and_then(|index| self.tool_by_output.get(&index).copied())
        {
            return slot;
        }
        self.push_tool(item_id, output_index, kind)
    }

    fn resolve_item_tool(
        &mut self,
        item: &Value,
        output_index: Option<u64>,
        kind: ToolKind,
    ) -> usize {
        let item_id = item.get("id").and_then(Value::as_str).unwrap_or_default();
        if let Some(&slot) = self
            .tool_by_item
            .get(item_id)
            .filter(|_| !item_id.is_empty())
        {
            return slot;
        }
        if let Some(slot) = output_index.and_then(|index| self.tool_by_output.get(&index).copied())
        {
            if !item_id.is_empty() {
                self.tool_by_item.insert(item_id.to_owned(), slot);
                self.tools[slot].item_id = item_id.to_owned();
            }
            return slot;
        }
        self.push_tool(item_id, output_index, kind)
    }

    fn push_tool(&mut self, item_id: &str, output_index: Option<u64>, kind: ToolKind) -> usize {
        let index = self.tools.len();
        let fallback_id = output_index.map_or_else(
            || format!("call_{index}"),
            |output| format!("call_{output}"),
        );
        self.tools.push(ToolSlot {
            item_id: item_id.to_owned(),
            call_id: fallback_id,
            name: String::new(),
            arguments: String::new(),
            kind,
            ended: false,
        });
        if !item_id.is_empty() {
            self.tool_by_item.insert(item_id.to_owned(), index);
        }
        if let Some(output_index) = output_index {
            self.tool_by_output.insert(output_index, index);
        }
        self.blocks.push(BlockAcc::Tool(index));
        index
    }

    fn update_tool_identity(&mut self, slot: usize, item: &Value) {
        let tool = &mut self.tools[slot];
        if let Some(call_id) = item
            .get("call_id")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        {
            tool.call_id = call_id.to_owned();
        }
        if let Some(name) = item
            .get("name")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        {
            tool.name = name.to_owned();
        }
    }

    fn append_tool_delta(&mut self, slot: usize, fragment: &str, events: &mut Vec<StreamEvent>) {
        if fragment.is_empty() {
            return;
        }
        let tool = &mut self.tools[slot];
        tool.arguments.push_str(fragment);
        events.push(StreamEvent::ToolCallDelta {
            id: tool.call_id.clone(),
            partial_json: fragment.to_owned(),
        });
    }

    fn set_complete_arguments(
        &mut self,
        slot: usize,
        complete: &str,
        events: &mut Vec<StreamEvent>,
    ) {
        if complete.is_empty() {
            return;
        }
        if self.tools[slot].arguments.is_empty() {
            self.append_tool_delta(slot, complete, events);
        } else if self.tools[slot].arguments != complete {
            self.tools[slot].arguments = complete.to_owned();
        }
    }

    fn end_tool(&mut self, slot: usize, events: &mut Vec<StreamEvent>) {
        if self.tools[slot].ended {
            return;
        }
        self.tools[slot].ended = true;
        events.push(StreamEvent::ToolCallEnd(self.tool_call(slot)));
    }

    fn tool_call(&self, slot: usize) -> ToolCall {
        let tool = &self.tools[slot];
        let arguments = match tool.kind {
            ToolKind::Function => parse_json_arguments(&tool.arguments),
            ToolKind::Custom => Value::String(tool.arguments.clone()),
        };
        ToolCall::new(tool.call_id.clone(), tool.name.clone(), arguments)
            .with_item_id(tool.item_id.clone())
    }

    /// Captures a message item's `phase` onto its text block.
    ///
    /// The phase is taken from the first event that carries one so a
    /// `done` item without the field cannot erase a captured phase.
    fn apply_message_phase(&mut self, slot: usize, item: &Value) {
        let Some(phase) = item
            .get("phase")
            .and_then(Value::as_str)
            .and_then(AssistantPhase::parse)
        else {
            return;
        };
        if let BlockAcc::Text(text) = &mut self.blocks[slot] {
            text.phase = Some(phase);
        }
    }

    fn append_text(&mut self, slot: usize, delta: &str, events: &mut Vec<StreamEvent>) {
        if delta.is_empty() {
            return;
        }
        let BlockAcc::Text(text) = &mut self.blocks[slot] else {
            return;
        };
        text.text.push_str(delta);
        events.push(StreamEvent::TextDelta(delta.to_owned()));
    }

    fn set_complete_text(&mut self, slot: usize, complete: &str, events: &mut Vec<StreamEvent>) {
        let BlockAcc::Text(text) = &mut self.blocks[slot] else {
            return;
        };
        if text.text == complete {
            return;
        }
        if let Some(suffix) = complete.strip_prefix(text.text.as_str()) {
            if !suffix.is_empty() {
                text.text.push_str(suffix);
                events.push(StreamEvent::TextDelta(suffix.to_owned()));
            }
        } else {
            text.text = complete.to_owned();
        }
    }

    fn append_thinking(&mut self, slot: usize, delta: &str, events: &mut Vec<StreamEvent>) {
        if delta.is_empty() {
            return;
        }
        let BlockAcc::Thinking(thinking) = &mut self.blocks[slot] else {
            return;
        };
        thinking.text.push_str(delta);
        events.push(StreamEvent::ThinkingDelta(delta.to_owned()));
    }

    fn set_complete_thinking(
        &mut self,
        slot: usize,
        complete: &str,
        replay: Option<ReplayState>,
        events: &mut Vec<StreamEvent>,
    ) {
        let BlockAcc::Thinking(thinking) = &mut self.blocks[slot] else {
            return;
        };
        if !complete.is_empty() && thinking.text != complete {
            if let Some(suffix) = complete.strip_prefix(&thinking.text) {
                if !suffix.is_empty() {
                    thinking.text.push_str(suffix);
                    events.push(StreamEvent::ThinkingDelta(suffix.to_owned()));
                }
            } else {
                thinking.text = complete.to_owned();
            }
        }
        if replay.is_some() {
            thinking.replay = replay;
        }
    }

    fn capture_response(
        &mut self,
        response: Option<&Value>,
        events: &mut Vec<StreamEvent>,
    ) -> Result<(), LlmError> {
        let Some(response) = response else {
            return Ok(());
        };
        if let Some(output) = response.get("output").and_then(Value::as_array) {
            for (index, item) in output.iter().enumerate() {
                self.finish_output_item(item, u64::try_from(index).ok(), events)?;
            }
        }
        if let Some(usage) = response.get("usage") {
            self.usage = Some(Usage {
                input_tokens: usage
                    .get("input_tokens")
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
                output_tokens: usage
                    .get("output_tokens")
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
            });
        }
        match response.get("status").and_then(Value::as_str) {
            Some("incomplete") => {
                let reason = response
                    .get("incomplete_details")
                    .and_then(|details| details.get("reason"))
                    .and_then(Value::as_str);
                self.stop_reason =
                    Some(if reason.is_none_or(|value| value == "max_output_tokens") {
                        StopReason::Length
                    } else {
                        StopReason::Error
                    });
            }
            Some("failed" | "cancelled") => self.stop_reason = Some(StopReason::Error),
            _ if self.refused => self.stop_reason = Some(StopReason::Error),
            _ => {}
        }
        Ok(())
    }
}

fn required_string<'a>(
    value: &'a Value,
    field: &str,
    event_type: &str,
) -> Result<&'a str, LlmError> {
    value.get(field).and_then(Value::as_str).ok_or_else(|| {
        LlmError::Sse(format!(
            "Responses event '{event_type}' has no string '{field}'"
        ))
    })
}

fn content_kind(item: &Value) -> Option<ContentKind> {
    match item.get("type").and_then(Value::as_str) {
        Some("message") => Some(ContentKind::Text),
        Some("reasoning") => Some(ContentKind::Thinking),
        _ => None,
    }
}

fn block_content_kind(block: &BlockAcc) -> Option<ContentKind> {
    match block {
        BlockAcc::Text(_) => Some(ContentKind::Text),
        BlockAcc::Thinking(_) => Some(ContentKind::Thinking),
        BlockAcc::Tool(_) => None,
    }
}

fn message_content(item: &Value) -> (String, bool) {
    let mut text = String::new();
    let mut refused = false;
    for content in item
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        match content.get("type").and_then(Value::as_str) {
            Some("output_text") => {
                if let Some(value) = content.get("text").and_then(Value::as_str) {
                    text.push_str(value);
                }
            }
            Some("refusal") => {
                refused = true;
                if let Some(value) = content.get("refusal").and_then(Value::as_str) {
                    text.push_str(value);
                }
            }
            _ => {}
        }
    }
    (text, refused)
}

fn reasoning_text(item: &Value) -> String {
    let collect = |field: &str| {
        item.get(field)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|part| part.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n\n")
    };
    let summary = collect("summary");
    if summary.is_empty() {
        collect("content")
    } else {
        summary
    }
}

fn tool_kind(item: &Value) -> Option<ToolKind> {
    match item.get("type").and_then(Value::as_str) {
        Some("function_call") => Some(ToolKind::Function),
        Some("custom_tool_call") => Some(ToolKind::Custom),
        _ => None,
    }
}

fn item_arguments(item: &Value, kind: ToolKind) -> Option<&str> {
    let field = match kind {
        ToolKind::Function => "arguments",
        ToolKind::Custom => "input",
    };
    item.get(field).and_then(Value::as_str)
}

fn parse_json_arguments(raw: &str) -> Value {
    if raw.trim().is_empty() {
        json!({})
    } else {
        serde_json::from_str(raw).unwrap_or(Value::Null)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mcode_core::message::{ToolResultMessage, UserMessage};
    use mcode_core::tool::ToolSpec;

    /// Domain of the built-in `openai` profile, as the transport would
    /// derive it (id plus effective endpoint origin).
    const OWN_ENDPOINT: &str = "https://api.openai.com";

    fn own_domain() -> ReplayDomain {
        ReplayDomain::new(ReplayWire::OpenAiResponses, "openai", OWN_ENDPOINT)
    }

    use crate::provider::{ThinkingConfig, ThinkingLevel};

    #[test]
    fn request_roundtrips_function_custom_calls_and_results() {
        let request = Request::new("gpt-5")
            .with_system_prompt("system one")
            .with_system_prompt("system two")
            .with_message(Message::User(UserMessage::text("hello")))
            .with_message(Message::Assistant(AssistantMessage {
                blocks: vec![
                    ContentBlock::ToolCall(ToolCall::new("call_fn", "read", json!({"path": "x"}))),
                    ContentBlock::ToolCall(ToolCall::new(
                        "call_patch",
                        "apply_patch",
                        Value::String("*** Begin Patch".into()),
                    )),
                ],
                usage: None,
                stop_reason: StopReason::ToolUse,
            }))
            .with_message(Message::ToolResult(ToolResultMessage {
                tool_call_id: "call_fn".into(),
                content: vec![ContentBlock::Text("ok".into())],
                is_error: false,
                details: None,
            }))
            .with_message(Message::ToolResult(ToolResultMessage {
                tool_call_id: "call_patch".into(),
                content: vec![ContentBlock::Text("Done!".into())],
                is_error: false,
                details: None,
            }))
            .with_tool(ToolSpec {
                name: "read".into(),
                description: "read".into(),
                params_schema: json!({"type": "object"}),
            })
            .with_tool(ToolSpec {
                name: "apply_patch".into(),
                description: "patch".into(),
                params_schema: json!({"type": "string"}),
            });
        let body = build_request_body(&request, &own_domain());
        assert_eq!(body["instructions"], "system one\n\nsystem two");
        assert_eq!(body["tools"][0]["type"], "function");
        assert_eq!(body["tools"][1]["type"], "custom");
        let input = body["input"].as_array().unwrap();
        assert!(input.iter().any(|item| item["type"] == "function_call"));
        assert!(input.iter().any(|item| item["type"] == "custom_tool_call"));
        assert!(
            input
                .iter()
                .any(|item| item["type"] == "function_call_output")
        );
        assert!(
            input
                .iter()
                .any(|item| item["type"] == "custom_tool_call_output")
        );
    }

    #[test]
    fn assistant_history_uses_valid_input_text_parts() {
        let request = Request::new("gpt-5").with_message(Message::Assistant(AssistantMessage {
            blocks: vec![
                ContentBlock::Text("answer".into()),
                ContentBlock::Thinking("unsigned summary".into()),
            ],
            usage: None,
            stop_reason: StopReason::Stop,
        }));
        let body = build_request_body(&request, &own_domain());
        assert_eq!(body["store"], false);
        assert_eq!(body["include"], json!(["reasoning.encrypted_content"]));
        let input = body["input"].as_array().unwrap();
        assert_eq!(input.len(), 2);
        assert!(input.iter().all(|item| item["role"] == "assistant"));
        assert!(
            input
                .iter()
                .all(|item| item["content"][0]["type"] == "input_text")
        );
        assert!(!body.to_string().contains("output_text"));
    }

    #[test]
    fn foreign_replay_state_downgrades_to_portable_text() {
        // An Anthropic thinking signature crossing to the Responses wire must
        // not be sent as a reasoning item; the visible text survives as
        // ordinary assistant input.
        let foreign = ThinkingBlock::new("prior thought").with_replay(
            ReplayState::new(ReplayWire::AnthropicMessages, "anthropic-signature")
                .with_provider("anthropic"),
        );
        let request = Request::new("gpt-5").with_message(Message::Assistant(AssistantMessage {
            blocks: vec![
                ContentBlock::Thinking(foreign),
                ContentBlock::Text(TextBlock::new("done")),
            ],
            usage: None,
            stop_reason: StopReason::ToolUse,
        }));
        let body = build_request_body(&request, &own_domain());
        let input = body["input"].as_array().unwrap();
        assert!(
            !input
                .iter()
                .any(|item| item["type"] == "reasoning" || item["phase"].is_string())
        );
        assert_eq!(input.len(), 2);
        assert_eq!(input[0]["content"][0]["text"], "prior thought");
        assert_eq!(input[1]["content"][0]["text"], "done");
        assert!(input.iter().all(|item| item["role"] == "assistant"));
        // A Responses reasoning item whose payload is not reasoning JSON
        // also downgrades instead of producing an invalid item.
        let malformed = ThinkingBlock::new("odd")
            .with_replay(ReplayState::new(ReplayWire::OpenAiResponses, "not-json"));
        let request = Request::new("gpt-5").with_message(Message::Assistant(AssistantMessage {
            blocks: vec![ContentBlock::Thinking(malformed)],
            usage: None,
            stop_reason: StopReason::Stop,
        }));
        let body = build_request_body(&request, &own_domain());
        assert!(!body["input"].to_string().contains("reasoning"));

        // Same wire but a different, untrusted profile: its encrypted
        // reasoning must not cross the trust boundary; only the visible
        // text survives. A profile that explicitly trusts the producer
        // replays the item verbatim.
        let untrusted = ThinkingBlock::new("gateway thought").with_replay(
            ReplayState::new(
                ReplayWire::OpenAiResponses,
                json!({"type":"reasoning","id":"rs_g","encrypted_content":"gateway-opaque"})
                    .to_string(),
            )
            .with_provider("openai-gateway")
            .with_endpoint("https://gateway.example"),
        );
        let request = Request::new("gpt-5").with_message(Message::Assistant(AssistantMessage {
            blocks: vec![ContentBlock::Thinking(untrusted)],
            usage: None,
            stop_reason: StopReason::Stop,
        }));
        let body = build_request_body(&request, &own_domain());
        let rendered = body["input"].to_string();
        assert!(!rendered.contains("gateway-opaque"), "{rendered}");
        assert!(rendered.contains("gateway thought"), "{rendered}");
        let body = build_request_body(&request, &own_domain().with_trusted("openai-gateway"));
        assert!(body["input"].to_string().contains("gateway-opaque"));
    }

    #[test]
    fn phases_are_captured_in_stream_and_replayed_in_original_order() {
        // Tool-dense flow: commentary preamble, tool call, then the final
        // answer as a second message item, each with its own phase.
        let commentary_item = json!({
            "type": "message",
            "id": "msg_1",
            "role": "assistant",
            "phase": "commentary",
            "status": "completed",
            "content": [{"type": "output_text", "text": "let me check the manifest"}]
        });
        let final_item = json!({
            "type": "message",
            "id": "msg_2",
            "role": "assistant",
            "phase": "final_answer",
            "status": "completed",
            "content": [{"type": "output_text", "text": "the manifest is valid"}]
        });
        let function_call = json!({
            "type": "function_call",
            "id": "fc_1",
            "call_id": "call_1",
            "name": "read",
            "arguments": "{\"path\":\"x\"}"
        });
        let mut aggregator = ResponsesAggregator::new();
        for payload in [
            json!({"type":"response.output_item.added","output_index":0,"item":{"type":"message","id":"msg_1","role":"assistant","phase":"commentary","status":"in_progress","content":[]}}),
            json!({"type":"response.output_text.delta","item_id":"msg_1","output_index":0,"delta":"let me check "}),
            json!({"type":"response.output_text.delta","item_id":"msg_1","output_index":0,"delta":"the manifest"}),
            json!({"type":"response.output_item.done","output_index":0,"item":commentary_item.clone()}),
            json!({"type":"response.output_item.done","output_index":1,"item":function_call.clone()}),
            json!({"type":"response.output_item.added","output_index":2,"item":{"type":"message","id":"msg_2","role":"assistant","phase":"final_answer","status":"in_progress","content":[]}}),
            json!({"type":"response.output_item.done","output_index":2,"item":final_item}),
            json!({"type":"response.completed","response":{"status":"completed"}}),
        ] {
            aggregator.on_data(&payload.to_string()).unwrap();
        }
        let events = aggregator.finish().unwrap();
        let StreamEvent::Done { message } = events.last().unwrap() else {
            panic!("expected Done");
        };
        let texts: Vec<(&str, Option<AssistantPhase>)> = message
            .blocks
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Text(text) => Some((text.text.as_str(), text.phase)),
                _ => None,
            })
            .collect();
        assert_eq!(
            texts,
            vec![
                (
                    "let me check the manifest",
                    Some(AssistantPhase::Commentary)
                ),
                ("the manifest is valid", Some(AssistantPhase::FinalAnswer)),
            ]
        );
        // Tool call stays interleaved between the two text blocks.
        assert!(matches!(message.blocks[1], ContentBlock::ToolCall(_)));

        // Next-turn replay: both message items return with their original
        // phase, in the original order around the function call.
        let request = Request::new("gpt-5")
            .with_message(Message::Assistant(message.clone()))
            .with_message(Message::ToolResult(ToolResultMessage {
                tool_call_id: "call_1".into(),
                content: vec![ContentBlock::Text(TextBlock::new("ok"))],
                is_error: false,
                details: None,
            }));
        let body = build_request_body(&request, &own_domain());
        let input = body["input"].as_array().unwrap();
        let shapes: Vec<String> = input
            .iter()
            .map(|item| {
                format!(
                    "{}:{}",
                    item.get("phase")
                        .and_then(Value::as_str)
                        .unwrap_or(item.get("type").and_then(Value::as_str).unwrap_or("?")),
                    item.get("role").and_then(Value::as_str).unwrap_or("")
                )
            })
            .collect();
        assert_eq!(
            shapes,
            vec![
                "commentary:assistant",
                "function_call:",
                "final_answer:assistant",
                "function_call_output:"
            ]
        );
        assert_eq!(input[0]["content"][0]["text"], "let me check the manifest");
        assert_eq!(input[2]["content"][0]["text"], "the manifest is valid");
    }

    #[test]
    fn reasoning_state_and_tool_item_ids_replay_on_the_next_request() {
        let reasoning_done = json!({
            "type": "reasoning",
            "id": "rs_1",
            "status": "completed",
            "summary": [{"type": "summary_text", "text": "checked"}]
        });
        let reasoning_terminal = json!({
            "type": "reasoning",
            "id": "rs_1",
            "status": "completed",
            "summary": [{"type": "summary_text", "text": "checked"}],
            "encrypted_content": "opaque-state"
        });
        let function_call = json!({
            "type": "function_call",
            "id": "fc_1",
            "call_id": "call_1",
            "name": "read",
            "arguments": "{\"path\":\"x\"}"
        });
        let mut aggregator = ResponsesAggregator::new();
        for payload in [
            json!({"type":"response.output_item.added","output_index":0,"item":{"type":"reasoning","id":"rs_1","summary":[]}}),
            json!({"type":"response.reasoning_summary_text.delta","item_id":"rs_1","output_index":0,"delta":"checked"}),
            json!({"type":"response.output_item.done","output_index":0,"item":reasoning_done}),
            json!({"type":"response.output_item.done","output_index":1,"item":function_call.clone()}),
            json!({"type":"response.completed","response":{"status":"completed","output":[reasoning_terminal, function_call]}}),
        ] {
            aggregator.on_data(&payload.to_string()).unwrap();
        }
        let events = aggregator.finish().unwrap();
        let StreamEvent::Done { message } = events.last().unwrap() else {
            panic!("expected Done");
        };
        let thinking = message
            .blocks
            .iter()
            .find_map(|block| match block {
                ContentBlock::Thinking(thinking) => Some(thinking),
                _ => None,
            })
            .expect("reasoning block");
        assert_eq!(thinking.text, "checked");
        let replay = thinking.replay.as_ref().expect("replay state");
        assert_eq!(replay.wire, ReplayWire::OpenAiResponses);
        assert!(replay.data.contains("opaque-state"));
        assert!(
            replay.provider.is_none(),
            "aggregators do not know the profile"
        );
        let call = message
            .blocks
            .iter()
            .find_map(|block| match block {
                ContentBlock::ToolCall(call) => Some(call),
                _ => None,
            })
            .expect("tool call");
        assert_eq!(call.id, "call_1");
        assert_eq!(call.item_id.as_deref(), Some("fc_1"));

        // The transport stamps the producing profile and endpoint onto
        // delivered state; simulate that before replaying the persisted
        // message.
        let mut message = message.clone();
        let thinking_block = message
            .blocks
            .iter_mut()
            .find_map(|block| match block {
                ContentBlock::Thinking(thinking) => Some(thinking),
                _ => None,
            })
            .expect("reasoning block");
        let replay_state = thinking_block.replay.as_mut().expect("replay state");
        replay_state.provider = Some("openai".into());
        replay_state.endpoint = Some(OWN_ENDPOINT.into());

        let request = Request::new("gpt-5")
            .with_thinking(ThinkingConfig {
                level: ThinkingLevel::High,
            })
            .with_message(Message::Assistant(message))
            .with_message(Message::ToolResult(ToolResultMessage {
                tool_call_id: call.id.clone(),
                content: vec![ContentBlock::Text("contents".into())],
                is_error: false,
                details: None,
            }));
        let body = build_request_body(&request, &own_domain());
        assert_eq!(body["include"], json!(["reasoning.encrypted_content"]));
        let input = body["input"].as_array().unwrap();
        assert!(input.iter().any(|item| {
            item["type"] == "reasoning"
                && item["id"] == "rs_1"
                && item["encrypted_content"] == "opaque-state"
        }));
        assert!(input.iter().any(|item| {
            item["type"] == "function_call" && item["id"] == "fc_1" && item["call_id"] == "call_1"
        }));
        assert!(
            input.iter().any(|item| {
                item["type"] == "function_call_output" && item["call_id"] == "call_1"
            })
        );
    }

    #[test]
    fn opaque_tool_ids_are_not_parsed_as_packed_encodings() {
        // A public Request / restored session may carry any opaque id, including
        // strings that look like a length-prefixed encoding or contain `|`.
        let request = Request::new("gpt-5")
            .with_message(Message::Assistant(AssistantMessage {
                blocks: vec![ContentBlock::ToolCall(ToolCall::new(
                    "v1:1:1:ab",
                    "read",
                    json!({"path": "x"}),
                ))],
                usage: None,
                stop_reason: StopReason::ToolUse,
            }))
            .with_message(Message::ToolResult(ToolResultMessage {
                tool_call_id: "v1:1:1:ab".into(),
                content: vec![ContentBlock::Text("ok".into())],
                is_error: false,
                details: None,
            }));
        let input = build_request_body(&request, &own_domain())["input"]
            .as_array()
            .unwrap()
            .clone();
        assert!(input.iter().any(|item| {
            item["type"] == "function_call"
                && item["call_id"] == "v1:1:1:ab"
                && item.get("id").is_none()
        }));
        assert!(input.iter().any(|item| {
            item["type"] == "function_call_output" && item["call_id"] == "v1:1:1:ab"
        }));
    }

    #[test]
    fn aggregator_keeps_call_and_item_ids_with_delimiters() {
        let mut aggregator = ResponsesAggregator::new();
        let mut events = aggregator
            .on_data(
                &json!({
                    "type":"response.output_item.done",
                    "output_index":0,
                    "item":{
                        "type":"function_call",
                        "id":"fc|1",
                        "call_id":"a|b",
                        "name":"read",
                        "arguments":"{\"path\":\"x\"}"
                    }
                })
                .to_string(),
            )
            .unwrap();
        aggregator
            .on_data(&json!({"type":"response.completed","response":{}}).to_string())
            .unwrap();
        events.extend(aggregator.finish().unwrap());
        let call = events
            .iter()
            .find_map(|event| match event {
                StreamEvent::ToolCallEnd(call) => Some(call),
                _ => None,
            })
            .expect("call");
        assert_eq!(call.id, "a|b");
        assert_eq!(call.item_id.as_deref(), Some("fc|1"));
    }

    #[test]
    fn function_call_replay_keeps_pipe_in_call_id() {
        let request = Request::new("gpt-5")
            .with_message(Message::Assistant(AssistantMessage {
                blocks: vec![ContentBlock::ToolCall(
                    ToolCall::new("a|b", "read", json!({"path": "x"})).with_item_id("fc|1"),
                )],
                usage: None,
                stop_reason: StopReason::ToolUse,
            }))
            .with_message(Message::ToolResult(ToolResultMessage {
                tool_call_id: "a|b".into(),
                content: vec![ContentBlock::Text("ok".into())],
                is_error: false,
                details: None,
            }));
        let input = build_request_body(&request, &own_domain())["input"]
            .as_array()
            .unwrap()
            .clone();
        assert!(input.iter().any(|item| {
            item["type"] == "function_call" && item["call_id"] == "a|b" && item["id"] == "fc|1"
        }));
        assert!(
            input
                .iter()
                .any(|item| { item["type"] == "function_call_output" && item["call_id"] == "a|b" })
        );
    }

    #[test]
    fn streamed_and_terminal_refusals_are_visible_errors() {
        let mut streamed = ResponsesAggregator::new();
        let mut events = Vec::new();
        for payload in [
            json!({"type":"response.output_item.added","output_index":0,"item":{"type":"message","id":"msg_1","role":"assistant","status":"in_progress","content":[]}}),
            json!({"type":"response.refusal.delta","item_id":"msg_1","output_index":0,"delta":"I cannot help."}),
            json!({"type":"response.completed","response":{"status":"completed"}}),
        ] {
            events.extend(streamed.on_data(&payload.to_string()).unwrap());
        }
        events.extend(streamed.finish().unwrap());
        assert!(events.contains(&StreamEvent::TextDelta("I cannot help.".into())));
        let StreamEvent::Done { message } = events.last().unwrap() else {
            panic!("expected Done");
        };
        assert_eq!(message.stop_reason, StopReason::Error);
        assert_eq!(
            message.blocks,
            vec![ContentBlock::Text("I cannot help.".into())]
        );

        let mut terminal = ResponsesAggregator::new();
        let events = terminal
            .on_data(
                &json!({
                    "type":"response.completed",
                    "response":{
                        "status":"completed",
                        "output":[{
                            "type":"message",
                            "id":"msg_2",
                            "role":"assistant",
                            "status":"completed",
                            "content":[{"type":"refusal","refusal":"Request refused."}]
                        }]
                    }
                })
                .to_string(),
            )
            .unwrap();
        assert_eq!(
            events,
            vec![StreamEvent::TextDelta("Request refused.".into())]
        );
        let events = terminal.finish().unwrap();
        let StreamEvent::Done { message } = events.last().unwrap() else {
            panic!("expected Done");
        };
        assert_eq!(message.stop_reason, StopReason::Error);
        assert_eq!(
            message.blocks,
            vec![ContentBlock::Text("Request refused.".into())]
        );
    }

    #[test]
    fn interleaved_calls_text_thinking_usage_and_no_done_sentinel() {
        let mut aggregator = ResponsesAggregator::new();
        let payloads = [
            json!({"type":"response.output_item.added","output_index":0,"item":{"type":"function_call","id":"fc1","call_id":"c1","name":"read","arguments":""}}),
            json!({"type":"response.output_item.added","output_index":1,"item":{"type":"function_call","id":"fc2","call_id":"c2","name":"bash","arguments":""}}),
            json!({"type":"response.function_call_arguments.delta","item_id":"fc1","output_index":0,"delta":"{\"path\":"}),
            json!({"type":"response.reasoning_text.delta","delta":"think"}),
            json!({"type":"response.function_call_arguments.delta","item_id":"fc2","output_index":1,"delta":"{\"command\":"}),
            json!({"type":"response.function_call_arguments.done","item_id":"fc1","output_index":0,"arguments":"{\"path\":\"x\"}"}),
            json!({"type":"response.function_call_arguments.done","item_id":"fc2","output_index":1,"arguments":"{\"command\":\"ls\"}"}),
            json!({"type":"response.output_text.delta","delta":"done"}),
            json!({"type":"response.completed","response":{"status":"completed","usage":{"input_tokens":7,"output_tokens":9}}}),
        ];
        let mut events = Vec::new();
        for payload in payloads {
            events.extend(aggregator.on_data(&payload.to_string()).unwrap());
        }
        assert!(aggregator.is_terminal());
        events.extend(aggregator.finish().unwrap());
        assert!(events.contains(&StreamEvent::ThinkingDelta("think".into())));
        assert!(events.contains(&StreamEvent::TextDelta("done".into())));
        let ends: Vec<&ToolCall> = events
            .iter()
            .filter_map(|event| match event {
                StreamEvent::ToolCallEnd(call) => Some(call),
                _ => None,
            })
            .collect();
        assert_eq!(ends.len(), 2);
        assert_eq!(ends[0].arguments, json!({"path": "x"}));
        assert_eq!(ends[1].arguments, json!({"command": "ls"}));
        let StreamEvent::Done { message } = events.last().unwrap() else {
            panic!("expected Done");
        };
        assert_eq!(
            message.usage,
            Some(Usage {
                input_tokens: 7,
                output_tokens: 9
            })
        );
        assert_eq!(message.stop_reason, StopReason::ToolUse);
    }

    #[test]
    fn custom_apply_patch_item_normalizes_to_string_arguments() {
        let mut aggregator = ResponsesAggregator::new();
        let mut events = aggregator
            .on_data(
                &json!({"type":"response.output_item.done","output_index":0,"item":{"type":"custom_tool_call","id":"ct1","call_id":"patch1","name":"apply_patch","input":"*** Begin Patch"}}).to_string(),
            )
            .unwrap();
        aggregator
            .on_data(&json!({"type":"response.completed","response":{}}).to_string())
            .unwrap();
        events.extend(aggregator.finish().unwrap());
        let call = events
            .iter()
            .find_map(|event| match event {
                StreamEvent::ToolCallEnd(call) => Some(call),
                _ => None,
            })
            .expect("expected call end");
        assert_eq!(call.name, "apply_patch");
        assert_eq!(call.arguments, Value::String("*** Begin Patch".into()));
    }

    #[test]
    fn incomplete_and_failed_are_distinct() {
        let mut incomplete = ResponsesAggregator::new();
        incomplete
            .on_data(&json!({"type":"response.incomplete","response":{"usage":{"input_tokens":1,"output_tokens":2}}}).to_string())
            .unwrap();
        let events = incomplete.finish().unwrap();
        let StreamEvent::Done { message } = events.last().unwrap() else {
            panic!("expected Done");
        };
        assert_eq!(message.stop_reason, StopReason::Length);

        let mut failed = ResponsesAggregator::new();
        let error = failed
            .on_data(
                &json!({"type":"response.failed","response":{"error":{"message":"boom"}}})
                    .to_string(),
            )
            .unwrap_err();
        assert!(matches!(error, LlmError::Http { status: 0, .. }));
    }

    #[test]
    fn malformed_or_unterminated_stream_fails() {
        let mut aggregator = ResponsesAggregator::new();
        assert!(matches!(
            aggregator.on_data("not json"),
            Err(LlmError::Sse(_))
        ));
        assert!(matches!(aggregator.finish(), Err(LlmError::Sse(_))));
    }
}

// Rust guideline compliant 2026-08-26

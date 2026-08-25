//! OpenAI-compatible chat-completions provider.
//!
//! Speaks the `/chat/completions` wire format shared by OpenAI and most
//! compatible endpoints (DeepSeek, Groq, Together, vLLM, …). The module
//! contains three layers:
//!
//! * request serialization ([`build_request_body`]) —
//!   converts our [`Message`] model into OpenAI chat messages;
//! * an incremental SSE pipeline — [`SseFramer`] (bytes → event data
//!   payloads) feeding [`ChatCompletionAggregator`] (payloads →
//!   [`StreamEvent`]s, aggregating `tool_calls` argument shards);
//! * [`OpenAiProvider`] — the [`Provider`] implementation driving both
//!   over `reqwest`.
//!
//! Known deviations from the strict OpenAI contract (all deliberate,
//! matching what pi does for the same fleet of compatible servers):
//!
//! * reasoning deltas are read from `reasoning_content` / `reasoning` /
//!   `reasoning_text` (first non-empty wins) so llama.cpp-style servers
//!   stream thinking;
//! * usage is also accepted at the choice level (`choice.usage`,
//!   Moonshot-style) in addition to `chunk.usage`;
//! * a missing `finish_reason` is not fatal — the stop reason is
//!   inferred from the accumulated content;
//! * an `{"error": …}` object delivered mid-stream fails the stream with
//!   [`LlmError::Http`] (`status: 0`).

use std::collections::HashMap;
use std::time::Duration;

use async_trait::async_trait;
use mcode_core::message::{AssistantMessage, ContentBlock, Message, StopReason, ToolCall, Usage};
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use tokio_stream::StreamExt;
use tokio_util::sync::CancellationToken;

use crate::auth;
use crate::error::LlmError;
use crate::provider::{Provider, Request, StreamEvent};
use crate::stream::{EventStream, EventStreamSender};

/// Default base URL of the public OpenAI API.
pub const DEFAULT_OPENAI_BASE_URL: &str = "https://api.openai.com/v1";

// ---------------------------------------------------------------------------
// Request serialization (our model → OpenAI chat format)
// ---------------------------------------------------------------------------

/// Wire body of a streaming chat-completions request.
#[derive(Serialize)]
struct ChatRequestBody<'a> {
    model: &'a str,
    messages: Vec<ChatMessage>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<ChatTool<'a>>,
    stream: bool,
    stream_options: StreamOptions,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_effort: Option<&'a str>,
}

#[derive(Serialize)]
struct StreamOptions {
    include_usage: bool,
}

/// One message in the OpenAI chat format. Internally tagged by `role`.
#[derive(Serialize)]
#[serde(tag = "role", rename_all = "snake_case")]
enum ChatMessage {
    System {
        content: String,
    },
    User {
        content: UserContent,
    },
    Assistant {
        #[serde(skip_serializing_if = "Option::is_none")]
        content: Option<String>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        tool_calls: Vec<ChatToolCall>,
    },
    Tool {
        tool_call_id: String,
        content: String,
    },
}

/// User content: a bare string when text-only, content parts otherwise.
#[derive(Serialize)]
#[serde(untagged)]
enum UserContent {
    Text(String),
    Parts(Vec<ContentPart>),
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ContentPart {
    Text { text: String },
    ImageUrl { image_url: ImageUrl },
}

#[derive(Serialize)]
struct ImageUrl {
    url: String,
}

#[derive(Serialize)]
struct ChatToolCall {
    id: String,
    r#type: &'static str,
    function: FunctionCall,
}

#[derive(Serialize)]
struct FunctionCall {
    name: String,
    /// Arguments re-serialized as a JSON string (OpenAI requirement).
    arguments: String,
}

#[derive(Serialize)]
struct ChatTool<'a> {
    r#type: &'static str,
    function: FunctionSpec<'a>,
}

#[derive(Serialize)]
struct FunctionSpec<'a> {
    name: &'a str,
    description: &'a str,
    parameters: &'a Value,
}

/// Build the JSON body for a streaming request. Infallible: our request
/// types always serialize.
pub fn build_request_body(req: &Request) -> Value {
    let body = ChatRequestBody {
        model: req.model.as_str(),
        messages: chat_messages(&req.system_prompt, &req.messages),
        tools: req
            .tools
            .iter()
            .map(|spec| ChatTool {
                r#type: "function",
                function: FunctionSpec {
                    name: spec.name.as_str(),
                    description: spec.description.as_str(),
                    parameters: &spec.params_schema,
                },
            })
            .collect(),
        stream: true,
        stream_options: StreamOptions {
            include_usage: true,
        },
        reasoning_effort: req
            .thinking
            .as_ref()
            .map(|config| config.level.as_effort_str()),
    };
    serde_json::to_value(body).expect("request body serialization is infallible")
}

/// Convert system prompt parts and a conversation into OpenAI chat
/// messages.
///
/// Conversion rules:
/// * each system-prompt part becomes one `system` message, in order;
/// * user text/image blocks become `user` content (bare string when
///   text-only, content parts when images are present);
/// * assistant text blocks join into `content`; `ToolCall` blocks become
///   `tool_calls`; `Thinking` blocks are dropped (the chat-completions
///   format has no standard field for replaying reasoning — providers
///   that want it use proprietary ones);
/// * [`Message::ToolResult`] becomes a `tool` message (text blocks joined
///   by newlines, `"(no tool output)"` placeholder when empty). Images
///   from tool results are forwarded as a follow-up `user` message with
///   image parts (pi's pattern for vision-capable models);
/// * [`Message::Custom`] never enters LLM context and is skipped.
fn chat_messages(system_prompt: &[String], messages: &[Message]) -> Vec<ChatMessage> {
    let mut out: Vec<ChatMessage> = system_prompt
        .iter()
        .map(|prompt| ChatMessage::System {
            content: prompt.clone(),
        })
        .collect();

    let mut pending_images: Vec<ContentPart> = Vec::new();
    for message in messages {
        // Flush image parts collected from a run of tool results before
        // any non-tool-result message.
        let flush_images = |pending: &mut Vec<ContentPart>, out: &mut Vec<ChatMessage>| {
            if pending.is_empty() {
                return;
            }
            let mut parts = vec![ContentPart::Text {
                text: "Attached image(s) from tool result:".into(),
            }];
            parts.append(pending);
            out.push(ChatMessage::User {
                content: UserContent::Parts(parts),
            });
        };

        match message {
            Message::User(user) => {
                flush_images(&mut pending_images, &mut out);
                let text: Vec<&str> = user
                    .content
                    .iter()
                    .filter_map(|block| match block {
                        ContentBlock::Text(text) => Some(text.as_str()),
                        _ => None,
                    })
                    .collect();
                let images: Vec<&mcode_core::message::BinaryData> = user
                    .content
                    .iter()
                    .filter_map(|block| match block {
                        ContentBlock::Image(data) => Some(data),
                        _ => None,
                    })
                    .collect();
                let content = if images.is_empty() {
                    if text.is_empty() {
                        continue;
                    }
                    UserContent::Text(text.join("\n"))
                } else {
                    let mut parts: Vec<ContentPart> = text
                        .into_iter()
                        .map(|t| ContentPart::Text { text: t.to_owned() })
                        .collect();
                    parts.extend(images.iter().map(|image| ContentPart::ImageUrl {
                        image_url: ImageUrl {
                            url: format!("data:{};base64,{}", image.mime_type, image.data),
                        },
                    }));
                    UserContent::Parts(parts)
                };
                out.push(ChatMessage::User { content });
            }
            Message::Assistant(assistant) => {
                flush_images(&mut pending_images, &mut out);
                let text: String = assistant
                    .blocks
                    .iter()
                    .filter_map(|block| match block {
                        ContentBlock::Text(text) => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("");
                let tool_calls: Vec<ChatToolCall> = assistant
                    .blocks
                    .iter()
                    .filter_map(|block| match block {
                        ContentBlock::ToolCall(call) => Some(ChatToolCall {
                            id: call.id.clone(),
                            r#type: "function",
                            function: FunctionCall {
                                name: call.name.clone(),
                                arguments: call.arguments.to_string(),
                            },
                        }),
                        _ => None,
                    })
                    .collect();
                // Skip empty assistant messages: strict servers reject
                // messages with neither content nor tool_calls (e.g.
                // aborted responses).
                if text.is_empty() && tool_calls.is_empty() {
                    continue;
                }
                out.push(ChatMessage::Assistant {
                    content: (!text.is_empty()).then_some(text),
                    tool_calls,
                });
            }
            Message::ToolResult(result) => {
                let text: String = result
                    .content
                    .iter()
                    .filter_map(|block| match block {
                        ContentBlock::Text(text) => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                let has_images = result
                    .content
                    .iter()
                    .any(|block| matches!(block, ContentBlock::Image(_)));
                let content = if !text.is_empty() {
                    text
                } else if has_images {
                    "(see attached image)".into()
                } else {
                    "(no tool output)".into()
                };
                out.push(ChatMessage::Tool {
                    tool_call_id: result.tool_call_id.clone(),
                    content,
                });
                for block in &result.content {
                    if let ContentBlock::Image(image) = block {
                        pending_images.push(ContentPart::ImageUrl {
                            image_url: ImageUrl {
                                url: format!("data:{};base64,{}", image.mime_type, image.data),
                            },
                        });
                    }
                }
            }
            // Plugin-persisted state: never enters LLM context.
            Message::Custom(_) => {
                flush_images(&mut pending_images, &mut out);
            }
        }
    }

    // Trailing images from a final tool-result run.
    if !pending_images.is_empty() {
        let mut parts = vec![ContentPart::Text {
            text: "Attached image(s) from tool result:".into(),
        }];
        parts.append(&mut pending_images);
        out.push(ChatMessage::User {
            content: UserContent::Parts(parts),
        });
    }
    out
}

// ---------------------------------------------------------------------------
// Incremental SSE framing
// ---------------------------------------------------------------------------

/// Frames a byte stream into server-sent-event data payloads.
///
/// Follows the SSE spec closely enough for real OpenAI-compatible
/// servers: events are separated by blank lines; `data:` lines are
/// joined with `\n` (multi-line data); comment lines (`:`) and other
/// fields (`event:`, `id:`, `retry:`) are ignored; `\r\n` endings are
/// tolerated. The `data: [DONE]` sentinel stops the framer — anything
/// after it is dropped.
///
/// Bytes may be fed in chunks of arbitrary size (including mid-UTF-8
/// and mid-line); the framer buffers until a complete line arrives.
#[derive(Debug, Default)]
pub struct SseFramer {
    buf: Vec<u8>,
    data_lines: Vec<String>,
    done: bool,
}

impl SseFramer {
    /// New empty framer.
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed raw bytes; returns the data payloads of all completed events.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<String> {
        if self.done {
            return Vec::new();
        }
        self.buf.extend_from_slice(bytes);
        let mut payloads = Vec::new();
        while let Some(pos) = self.buf.iter().position(|&b| b == b'\n') {
            let mut line: Vec<u8> = self.buf.drain(..=pos).collect();
            line.pop(); // '\n'
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            self.process_line(&line, &mut payloads);
            if self.done {
                self.buf.clear();
                break;
            }
        }
        payloads
    }

    /// Flush a final event that was not terminated by a blank line (EOF).
    pub fn finish(&mut self) -> Vec<String> {
        let mut payloads = Vec::new();
        if self.done {
            return payloads;
        }
        if !self.buf.is_empty() {
            let mut line = std::mem::take(&mut self.buf);
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            self.process_line(&line, &mut payloads);
        }
        self.dispatch(&mut payloads);
        payloads
    }

    /// Whether the `[DONE]` sentinel has been seen.
    pub fn is_done(&self) -> bool {
        self.done
    }

    fn process_line(&mut self, line: &[u8], payloads: &mut Vec<String>) {
        if line.is_empty() {
            self.dispatch(payloads);
        } else if line.starts_with(b":") {
            // Comment / keep-alive: ignored, does not end the event.
        } else if let Some(data) = line.strip_prefix(b"data:") {
            // Strip a single optional space after the colon.
            let data = data.strip_prefix(b" ").unwrap_or(data);
            self.data_lines
                .push(String::from_utf8_lossy(data).into_owned());
        }
        // Other field names (event:, id:, retry:) and bare bytes are ignored.
    }

    fn dispatch(&mut self, payloads: &mut Vec<String>) {
        if self.data_lines.is_empty() {
            return;
        }
        let payload = self.data_lines.join("\n");
        self.data_lines.clear();
        if payload == "[DONE]" {
            self.done = true;
        } else {
            payloads.push(payload);
        }
    }
}

// ---------------------------------------------------------------------------
// Chunk aggregation
// ---------------------------------------------------------------------------

/// Streaming shape of a `chat.completion.chunk` payload. Unknown fields
/// are ignored — compatible servers add plenty.
#[derive(Deserialize)]
struct ChatCompletionChunk {
    #[serde(default)]
    choices: Vec<ChunkChoice>,
    #[serde(default)]
    usage: Option<ChunkUsage>,
}

#[derive(Deserialize)]
struct ChunkChoice {
    #[serde(default)]
    delta: ChunkDelta,
    #[serde(default)]
    finish_reason: Option<String>,
    /// Non-standard fallback (Moonshot & friends).
    #[serde(default)]
    usage: Option<ChunkUsage>,
}

#[derive(Deserialize, Default)]
struct ChunkDelta {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    reasoning_content: Option<String>,
    #[serde(default)]
    reasoning: Option<String>,
    #[serde(default)]
    reasoning_text: Option<String>,
    #[serde(default)]
    tool_calls: Vec<ChunkToolCall>,
}

#[derive(Deserialize)]
struct ChunkToolCall {
    #[serde(default)]
    index: Option<u32>,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    function: Option<ChunkFunction>,
}

#[derive(Deserialize)]
struct ChunkFunction {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
}

#[derive(Deserialize, Clone, Copy)]
struct ChunkUsage {
    #[serde(default)]
    prompt_tokens: Option<u64>,
    #[serde(default)]
    completion_tokens: Option<u64>,
}

/// Accumulated state of one streaming tool call.
struct ToolSlot {
    id: String,
    name: String,
    /// Raw concatenated `arguments` fragments (parsed at the end).
    arguments: String,
}

/// Content blocks under construction, in first-arrival order.
enum BlockAcc {
    Text(String),
    Thinking(String),
    /// Index into [`ChatCompletionAggregator::tool_slots`].
    ToolCall(usize),
}

/// Aggregates `chat.completion.chunk` payloads into [`StreamEvent`]s.
///
/// Emits `TextDelta` / `ThinkingDelta` / `ToolCallDelta` as fragments
/// arrive, and on [`ChatCompletionAggregator::finish`] emits one
/// `ToolCallEnd` per aggregated call (block order) followed by the
/// terminal `Done` event with the fully assembled [`AssistantMessage`].
#[derive(Default)]
pub struct ChatCompletionAggregator {
    blocks: Vec<BlockAcc>,
    tool_slots: Vec<ToolSlot>,
    slot_by_index: HashMap<u32, usize>,
    slot_by_id: HashMap<String, usize>,
    stop_reason: Option<StopReason>,
    usage: Option<Usage>,
}

impl ChatCompletionAggregator {
    /// New empty aggregator.
    pub fn new() -> Self {
        Self::default()
    }

    /// Process one SSE data payload; returns the events it produced.
    pub fn on_data(&mut self, payload: &str) -> Result<Vec<StreamEvent>, LlmError> {
        let value: Value = serde_json::from_str(payload).map_err(|err| {
            LlmError::Sse(format!(
                "invalid JSON in SSE data ({err}): {}",
                LlmError::excerpt(payload)
            ))
        })?;
        // Some servers (OpenRouter, vLLM, …) deliver API errors
        // mid-stream as 200 + {"error": …} payloads.
        if let Some(error) = value.get("error").filter(|e| !e.is_null()) {
            return Err(LlmError::Http {
                status: 0,
                body: LlmError::excerpt(error.to_string()),
            });
        }
        let chunk: ChatCompletionChunk = serde_json::from_value(value).map_err(|err| {
            LlmError::Sse(format!(
                "unexpected chat.completion.chunk shape ({err}): {}",
                LlmError::excerpt(payload)
            ))
        })?;

        let mut events = Vec::new();
        if let Some(usage) = chunk.usage {
            self.usage = Some(map_usage(usage));
        }
        let Some(choice) = chunk.choices.into_iter().next() else {
            // Usage-only chunks (stream_options.include_usage) have no
            // choices; nothing else to do.
            return Ok(events);
        };
        if chunk.usage.is_none() {
            if let Some(usage) = choice.usage {
                self.usage = Some(map_usage(usage));
            }
        }
        if let Some(reason) = choice.finish_reason.as_deref() {
            self.stop_reason = Some(map_stop_reason(reason));
        }

        let delta = choice.delta;
        if let Some(text) = delta.content.filter(|c| !c.is_empty()) {
            self.append_block_text(&text);
            events.push(StreamEvent::TextDelta(text));
        }
        // Reasoning fields: take the first non-empty one (some servers
        // duplicate content across several fields).
        let reasoning = [
            delta.reasoning_content.as_deref(),
            delta.reasoning.as_deref(),
            delta.reasoning_text.as_deref(),
        ]
        .into_iter()
        .flatten()
        .find(|r| !r.is_empty());
        if let Some(thinking) = reasoning {
            self.append_block_thinking(thinking);
            events.push(StreamEvent::ThinkingDelta(thinking.to_owned()));
        }

        for tool_call in delta.tool_calls {
            let slot_idx = self.resolve_slot(&tool_call);
            let slot = &mut self.tool_slots[slot_idx];
            if let Some(name) = tool_call
                .function
                .as_ref()
                .and_then(|f| f.name.as_deref())
                .filter(|n| !n.is_empty())
            {
                if slot.name.is_empty() {
                    slot.name = name.to_owned();
                }
            }
            if let Some(fragment) = tool_call
                .function
                .as_ref()
                .and_then(|f| f.arguments.as_deref())
                .filter(|a| !a.is_empty())
            {
                slot.arguments.push_str(fragment);
                events.push(StreamEvent::ToolCallDelta {
                    id: slot.id.clone(),
                    partial_json: fragment.to_owned(),
                });
            }
        }

        Ok(events)
    }

    /// Finalize the message: emits `ToolCallEnd` for every aggregated
    /// tool call plus the terminal `Done` event. Call once, after the
    /// byte stream (or the `[DONE]` sentinel) is exhausted.
    pub fn finish(&mut self) -> Vec<StreamEvent> {
        let mut events = Vec::new();
        let mut blocks = Vec::new();
        for block in std::mem::take(&mut self.blocks) {
            match block {
                BlockAcc::Text(text) => blocks.push(ContentBlock::Text(text)),
                BlockAcc::Thinking(text) => blocks.push(ContentBlock::Thinking(text)),
                BlockAcc::ToolCall(slot_idx) => {
                    let slot = &self.tool_slots[slot_idx];
                    let call = ToolCall {
                        id: if slot.id.is_empty() {
                            format!("call_{slot_idx}")
                        } else {
                            slot.id.clone()
                        },
                        name: slot.name.clone(),
                        arguments: parse_arguments(&slot.arguments),
                    };
                    blocks.push(ContentBlock::ToolCall(call.clone()));
                    events.push(StreamEvent::ToolCallEnd(call));
                }
            }
        }
        // Lenient stop-reason inference for servers that never send
        // finish_reason (see module docs).
        let stop_reason = self.stop_reason.unwrap_or({
            if self.tool_slots.is_empty() {
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
        events
    }

    fn append_block_text(&mut self, text: &str) {
        match self.blocks.last_mut() {
            Some(BlockAcc::Text(buffer)) => buffer.push_str(text),
            _ => self.blocks.push(BlockAcc::Text(text.to_owned())),
        }
    }

    fn append_block_thinking(&mut self, text: &str) {
        match self.blocks.last_mut() {
            Some(BlockAcc::Thinking(buffer)) => buffer.push_str(text),
            _ => self.blocks.push(BlockAcc::Thinking(text.to_owned())),
        }
    }

    /// Map a streamed tool-call delta onto an accumulation slot, creating
    /// one when needed. Primary key is `index` (OpenAI always sends it);
    /// `id` is the fallback; index-less/id-less fragments attach to a
    /// lone existing slot or start a new one.
    fn resolve_slot(&mut self, delta: &ChunkToolCall) -> usize {
        if let Some(index) = delta.index {
            if let Some(&slot) = self.slot_by_index.get(&index) {
                return slot;
            }
            return self.push_slot(delta.index, delta.id.as_deref());
        }
        if let Some(id) = delta.id.as_deref().filter(|id| !id.is_empty()) {
            if let Some(&slot) = self.slot_by_id.get(id) {
                return slot;
            }
            return self.push_slot(None, Some(id));
        }
        if self.tool_slots.len() == 1 {
            return 0;
        }
        self.push_slot(None, None)
    }

    fn push_slot(&mut self, index: Option<u32>, id: Option<&str>) -> usize {
        let slot_idx = self.tool_slots.len();
        self.tool_slots.push(ToolSlot {
            id: id.unwrap_or_default().to_owned(),
            name: String::new(),
            arguments: String::new(),
        });
        if let Some(index) = index {
            self.slot_by_index.insert(index, slot_idx);
        }
        if let Some(id) = id.filter(|id| !id.is_empty()) {
            self.slot_by_id.insert(id.to_owned(), slot_idx);
        }
        self.blocks.push(BlockAcc::ToolCall(slot_idx));
        slot_idx
    }
}

fn parse_arguments(raw: &str) -> Value {
    if raw.trim().is_empty() {
        return Value::Object(Default::default());
    }
    // Invalid aggregated JSON (e.g. truncated by a length stop) degrades
    // to Null: tool dispatch fails schema validation, the tool result
    // reports the error, and the model can retry.
    serde_json::from_str(raw).unwrap_or(Value::Null)
}

fn map_usage(usage: ChunkUsage) -> Usage {
    Usage {
        input_tokens: usage.prompt_tokens.unwrap_or(0),
        output_tokens: usage.completion_tokens.unwrap_or(0),
    }
}

fn map_stop_reason(reason: &str) -> StopReason {
    match reason {
        "tool_calls" => StopReason::ToolUse,
        "length" => StopReason::Length,
        "content_filter" => StopReason::Error,
        // "stop", legacy "function_call", and unknown reasons.
        _ => StopReason::Stop,
    }
}

// ---------------------------------------------------------------------------
// Provider
// ---------------------------------------------------------------------------

/// Provider for OpenAI-compatible `/chat/completions` endpoints.
#[derive(Clone)]
pub struct OpenAiProvider {
    client: reqwest::Client,
    id: String,
    base_url: String,
    api_key: String,
    /// Per-request total timeout (connect through end of body).
    timeout: Option<Duration>,
}

impl OpenAiProvider {
    /// Build a provider for an explicit base URL and API key. `base_url`
    /// is everything up to (but excluding) `/chat/completions`, e.g.
    /// `https://api.openai.com/v1` (trailing slashes trimmed).
    pub fn new(base_url: impl Into<String>, api_key: impl Into<String>) -> Self {
        Self::from_parts(base_url.into(), api_key.into())
    }

    /// Build from the environment:
    ///
    /// * `OPENAI_BASE_URL` — base URL (default:
    ///   [`DEFAULT_OPENAI_BASE_URL`]);
    /// * `OPENAI_API_KEY` — API key, falling back to `~/.mcode/auth.toml`
    ///   via [`crate::auth`].
    pub fn from_env() -> Result<Self, LlmError> {
        let api_key = auth::resolve_api_key(None, "OPENAI_API_KEY", "openai")?;
        let base_url = std::env::var("OPENAI_BASE_URL").ok();
        Ok(Self::from_parts(base_url.unwrap_or_default(), api_key))
    }

    fn from_parts(base_url: String, api_key: String) -> Self {
        Self {
            client: build_client(None),
            id: "openai".into(),
            base_url: base_url.trim_end_matches('/').to_owned(),
            api_key,
            timeout: None,
        }
    }

    /// Override the provider id (for compatible endpoints registered
    /// under their own name, e.g. `"deepseek"`).
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = id.into();
        self
    }

    /// Set a total per-request timeout (connection + full body).
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// Full chat-completions endpoint URL.
    pub fn endpoint(&self) -> String {
        format!("{}/chat/completions", self.base_url)
    }
}

fn build_client(timeout: Option<Duration>) -> reqwest::Client {
    let mut builder =
        reqwest::Client::builder().user_agent(concat!("mcode/", env!("CARGO_PKG_VERSION")));
    if let Some(timeout) = timeout {
        builder = builder.timeout(timeout);
    }
    builder.build().unwrap_or_default()
}

fn map_reqwest_error(err: reqwest::Error) -> LlmError {
    if err.is_timeout() {
        LlmError::Timeout
    } else {
        LlmError::Transport(err.to_string())
    }
}

/// Feed SSE payloads through the aggregator, pushing produced events.
/// Returns the first error, if any.
fn push_payloads(
    agg: &mut ChatCompletionAggregator,
    tx: &EventStreamSender,
    payloads: Vec<String>,
) -> Result<(), LlmError> {
    for payload in payloads {
        for event in agg.on_data(&payload)? {
            if !tx.push(event) {
                // Consumer is gone; nothing left to do.
                return Ok(());
            }
        }
    }
    Ok(())
}

#[async_trait]
impl Provider for OpenAiProvider {
    fn id(&self) -> &str {
        &self.id
    }

    async fn stream(
        &self,
        req: &Request,
        cancel: CancellationToken,
    ) -> Result<EventStream, LlmError> {
        let (tx, stream) = EventStream::channel_with_cancel(cancel.clone());
        let client = self.client.clone();
        let url = self.endpoint();
        let api_key = self.api_key.clone();
        let timeout = self.timeout;
        let body = build_request_body(req);

        tokio::spawn(async move {
            let mut request = client
                .post(&url)
                .bearer_auth(&api_key)
                .header("accept", "text/event-stream")
                .json(&body);
            if let Some(timeout) = timeout {
                request = request.timeout(timeout);
            }

            let response = tokio::select! {
                biased;
                response = request.send() => match response {
                    Ok(response) => response,
                    Err(err) => {
                        tx.push(StreamEvent::Error(map_reqwest_error(err)));
                        return;
                    }
                },
                _ = cancel.cancelled() => {
                    tx.push(StreamEvent::Error(LlmError::Cancelled));
                    return;
                }
            };

            let status = response.status();
            if !status.is_success() {
                let body = response.text().await.unwrap_or_default();
                tx.push(StreamEvent::Error(LlmError::Http {
                    status: status.as_u16(),
                    body: LlmError::excerpt(body),
                }));
                return;
            }
            if !tx.push(StreamEvent::Start) {
                return;
            }

            let mut framer = SseFramer::new();
            let mut aggregator = ChatCompletionAggregator::new();
            let mut bytes = response.bytes_stream();

            // Outcome of the read loop: Ok(()) when the stream is
            // exhausted or [DONE] seen; Err on transport/SSE errors or
            // cancellation.
            let outcome: Result<(), LlmError> = 'read: {
                loop {
                    let chunk = tokio::select! {
                        biased;
                        _ = cancel.cancelled() => break 'read Err(LlmError::Cancelled),
                        chunk = bytes.next() => match chunk {
                            Some(Ok(bytes)) => bytes,
                            Some(Err(err)) => {
                                break 'read Err(map_reqwest_error(err));
                            }
                            None => break 'read Ok(()),
                        },
                    };
                    if let Err(err) = push_payloads(&mut aggregator, &tx, framer.feed(&chunk)) {
                        break 'read Err(err);
                    }
                    if framer.is_done() {
                        break 'read Ok(());
                    }
                }
            };

            match outcome {
                Ok(()) => {
                    let _ = push_payloads(&mut aggregator, &tx, framer.finish());
                    for event in aggregator.finish() {
                        if !tx.push(event) {
                            return;
                        }
                    }
                }
                Err(err) => {
                    tx.push(StreamEvent::Error(err));
                }
            }
        });

        Ok(stream)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mcode_core::message::ToolResultMessage;
    use serde_json::json;

    fn openai_chunk(payload: Value) -> String {
        payload.to_string()
    }

    #[test]
    fn usage_only_chunk_produces_no_events() {
        let mut agg = ChatCompletionAggregator::new();
        let payload = openai_chunk(json!({
            "choices": [],
            "usage": {"prompt_tokens": 10, "completion_tokens": 5}
        }));
        assert!(agg.on_data(&payload).unwrap().is_empty());
        let events = agg.finish();
        let done = events.last().unwrap();
        let StreamEvent::Done { message } = done else {
            panic!("expected Done, got {done:?}");
        };
        assert_eq!(
            message.usage,
            Some(Usage {
                input_tokens: 10,
                output_tokens: 5
            })
        );
        assert_eq!(message.stop_reason, StopReason::Stop);
    }

    #[test]
    fn choice_level_usage_fallback() {
        let mut agg = ChatCompletionAggregator::new();
        let payload = openai_chunk(json!({
            "choices": [{"index": 0, "delta": {}, "finish_reason": "stop",
                          "usage": {"prompt_tokens": 3, "completion_tokens": 4}}]
        }));
        agg.on_data(&payload).unwrap();
        let events = agg.finish();
        let StreamEvent::Done { message } = events.last().unwrap() else {
            panic!("expected Done");
        };
        assert_eq!(message.usage.unwrap().input_tokens, 3);
    }

    #[test]
    fn midstream_error_object_maps_to_http_zero() {
        let mut agg = ChatCompletionAggregator::new();
        let err = agg
            .on_data(&json!({"error": {"message": "rate limited"}}).to_string())
            .unwrap_err();
        match err {
            LlmError::Http { status, body } => {
                assert_eq!(status, 0);
                assert!(body.contains("rate limited"));
            }
            other => panic!("expected Http, got {other:?}"),
        }
    }

    #[test]
    fn stop_reason_mapping() {
        assert_eq!(map_stop_reason("stop"), StopReason::Stop);
        assert_eq!(map_stop_reason("tool_calls"), StopReason::ToolUse);
        assert_eq!(map_stop_reason("length"), StopReason::Length);
        assert_eq!(map_stop_reason("content_filter"), StopReason::Error);
        assert_eq!(map_stop_reason("function_call"), StopReason::Stop);
        assert_eq!(map_stop_reason("anything-else"), StopReason::Stop);
    }

    #[test]
    fn parse_arguments_handles_empty_and_invalid() {
        assert_eq!(parse_arguments(""), json!({}));
        assert_eq!(parse_arguments("   "), json!({}));
        assert_eq!(parse_arguments("{\"a\":1}"), json!({"a": 1}));
        assert_eq!(parse_arguments("{\"trunc"), Value::Null);
    }

    #[test]
    fn tool_result_text_joining_and_placeholder() {
        let messages = vec![
            Message::ToolResult(ToolResultMessage {
                tool_call_id: "c1".into(),
                content: vec![
                    ContentBlock::Text("line one".into()),
                    ContentBlock::Text("line two".into()),
                ],
                is_error: false,
                details: None,
            }),
            Message::ToolResult(ToolResultMessage {
                tool_call_id: "c2".into(),
                content: vec![],
                is_error: false,
                details: None,
            }),
        ];
        let converted = chat_messages(&[], &messages);
        assert_eq!(converted.len(), 2);
        let Value::Object(map) = serde_json::to_value(&converted[0]).unwrap() else {
            panic!("expected object");
        };
        assert_eq!(map["role"], "tool");
        assert_eq!(map["tool_call_id"], "c1");
        assert_eq!(map["content"], "line one\nline two");
        let Value::Object(map2) = serde_json::to_value(&converted[1]).unwrap() else {
            panic!("expected object");
        };
        assert_eq!(map2["content"], "(no tool output)");
    }

    #[test]
    fn empty_assistant_message_is_skipped() {
        let messages = vec![Message::Assistant(AssistantMessage {
            blocks: vec![ContentBlock::Thinking("reasoning only".into())],
            usage: None,
            stop_reason: StopReason::Stop,
        })];
        // Thinking-only assistant history is dropped: nothing to send.
        assert!(chat_messages(&[], &messages).is_empty());
    }
}

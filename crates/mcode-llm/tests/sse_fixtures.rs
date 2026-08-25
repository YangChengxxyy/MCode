//! Table-driven SSE replay tests: each fixture under `tests/fixtures/`
//! is fed through `SseFramer` + `ChatCompletionAggregator` (in odd-sized
//! byte chunks, to prove incremental buffering) and must produce the
//! exact expected event sequence.

use mcode_core::message::{AssistantMessage, ContentBlock, StopReason, ToolCall, Usage};
use mcode_llm::error::LlmError;
use mcode_llm::openai::{ChatCompletionAggregator, SseFramer};
use mcode_llm::provider::StreamEvent;

fn text_message(blocks_text: &str, usage: Option<Usage>) -> AssistantMessage {
    AssistantMessage {
        blocks: vec![ContentBlock::Text(blocks_text.into())],
        usage,
        stop_reason: StopReason::Stop,
    }
}

fn tool_call(id: &str, name: &str, arguments: serde_json::Value) -> ToolCall {
    ToolCall {
        id: id.into(),
        name: name.into(),
        arguments,
    }
}

/// Drain SSE payloads through the aggregator into `events`; returns
/// false when an error event was pushed (stream terminated).
fn drain_payloads(
    agg: &mut ChatCompletionAggregator,
    events: &mut Vec<StreamEvent>,
    payloads: Vec<String>,
) -> bool {
    for payload in payloads {
        match agg.on_data(&payload) {
            Ok(mut produced) => events.append(&mut produced),
            Err(err) => {
                events.push(StreamEvent::Error(err));
                return false;
            }
        }
    }
    true
}

/// Replay raw SSE bytes exactly the way `OpenAiProvider` does after a
/// 2xx response: feed chunks, stop at `[DONE]`, flush at EOF, finalize.
/// Feeds 7 bytes at a time so line/event boundaries land mid-chunk.
fn events_from_sse(bytes: &[u8]) -> Vec<StreamEvent> {
    let mut events = vec![StreamEvent::Start];
    let mut framer = SseFramer::new();
    let mut aggregator = ChatCompletionAggregator::new();
    for chunk in bytes.chunks(7) {
        if !drain_payloads(&mut aggregator, &mut events, framer.feed(chunk)) {
            return events;
        }
        if framer.is_done() {
            break;
        }
    }
    // `framer.finish()` is a no-op after [DONE], same as the provider.
    if !drain_payloads(&mut aggregator, &mut events, framer.finish()) {
        return events;
    }
    events.extend(aggregator.finish());
    events
}

fn done_event(message: AssistantMessage) -> StreamEvent {
    StreamEvent::Done { message }
}

#[test]
fn text_only_fixture() {
    let bytes = include_str!("fixtures/text_only.sse");
    let events = events_from_sse(bytes.as_bytes());
    assert_eq!(
        events,
        vec![
            StreamEvent::Start,
            StreamEvent::TextDelta("Hello".into()),
            StreamEvent::TextDelta(" world".into()),
            done_event(text_message(
                "Hello world",
                Some(Usage {
                    input_tokens: 9,
                    output_tokens: 2,
                }),
            )),
        ]
    );
}

#[test]
fn single_tool_call_sharded_across_many_chunks() {
    let bytes = include_str!("fixtures/tool_call_sharded.sse");
    let events = events_from_sse(bytes.as_bytes());
    let expected_call = tool_call(
        "call_abc",
        "read",
        serde_json::json!({"path": "Cargo.toml"}),
    );
    assert_eq!(
        events,
        vec![
            StreamEvent::Start,
            StreamEvent::TextDelta("Reading the file.".into()),
            StreamEvent::ToolCallDelta {
                id: "call_abc".into(),
                partial_json: "{\"pa".into(),
            },
            StreamEvent::ToolCallDelta {
                id: "call_abc".into(),
                partial_json: "th\": ".into(),
            },
            StreamEvent::ToolCallDelta {
                id: "call_abc".into(),
                partial_json: "\"Cargo.toml\"}".into(),
            },
            StreamEvent::ToolCallEnd(expected_call.clone()),
            done_event(AssistantMessage {
                blocks: vec![
                    ContentBlock::Text("Reading the file.".into()),
                    ContentBlock::ToolCall(expected_call),
                ],
                usage: None,
                stop_reason: StopReason::ToolUse,
            }),
        ]
    );
}

#[test]
fn parallel_tool_calls_interleaved_by_index() {
    let bytes = include_str!("fixtures/parallel_tool_calls.sse");
    let events = events_from_sse(bytes.as_bytes());
    let call_1 = tool_call("call_1", "read", serde_json::json!({"path": "Cargo.toml"}));
    let call_2 = tool_call("call_2", "bash", serde_json::json!({"command": "ls"}));
    assert_eq!(
        events,
        vec![
            StreamEvent::Start,
            StreamEvent::ToolCallDelta {
                id: "call_1".into(),
                partial_json: "{\"path\":".into(),
            },
            StreamEvent::ToolCallDelta {
                id: "call_2".into(),
                partial_json: "{\"command\":".into(),
            },
            StreamEvent::ToolCallDelta {
                id: "call_1".into(),
                partial_json: "\"Cargo.toml\"}".into(),
            },
            StreamEvent::ToolCallDelta {
                id: "call_2".into(),
                partial_json: "\"ls\"}".into(),
            },
            StreamEvent::ToolCallEnd(call_1.clone()),
            StreamEvent::ToolCallEnd(call_2.clone()),
            done_event(AssistantMessage {
                blocks: vec![
                    ContentBlock::ToolCall(call_1),
                    ContentBlock::ToolCall(call_2),
                ],
                usage: None,
                stop_reason: StopReason::ToolUse,
            }),
        ]
    );
}

#[test]
fn done_sentinel_stops_the_stream_and_infers_stop_reason() {
    let bytes = include_str!("fixtures/done_sentinel.sse");
    let events = events_from_sse(bytes.as_bytes());
    assert_eq!(
        events,
        vec![
            StreamEvent::Start,
            StreamEvent::TextDelta("no finish".into()),
            done_event(text_message("no finish", None)),
        ]
    );
}

#[test]
fn malformed_data_line_fails_the_stream_with_sse_error() {
    let bytes = include_str!("fixtures/malformed.sse");
    let events = events_from_sse(bytes.as_bytes());
    assert_eq!(events.len(), 3);
    assert_eq!(events[0], StreamEvent::Start);
    assert_eq!(events[1], StreamEvent::TextDelta("partial ok".into()));
    match &events[2] {
        StreamEvent::Error(LlmError::Sse(message)) => {
            assert!(message.contains("invalid JSON"), "got: {message}");
            assert!(message.contains("not valid json"), "got: {message}");
        }
        other => panic!("expected Sse error, got {other:?}"),
    }
}

#[test]
fn crlf_comments_and_choice_level_usage() {
    let bytes = include_str!("fixtures/comments_crlf_usage.sse");
    let events = events_from_sse(bytes.as_bytes());
    assert_eq!(
        events,
        vec![
            StreamEvent::Start,
            StreamEvent::TextDelta("crlf ok".into()),
            done_event(text_message(
                "crlf ok",
                Some(Usage {
                    input_tokens: 4,
                    output_tokens: 3,
                }),
            )),
        ]
    );
}

#[test]
fn reasoning_deltas_stream_as_thinking() {
    let bytes = include_str!("fixtures/reasoning.sse");
    let events = events_from_sse(bytes.as_bytes());
    assert_eq!(
        events,
        vec![
            StreamEvent::Start,
            StreamEvent::ThinkingDelta("thinking...".into()),
            StreamEvent::ThinkingDelta(" more".into()),
            StreamEvent::TextDelta("the answer".into()),
            done_event(AssistantMessage {
                blocks: vec![
                    ContentBlock::Thinking("thinking... more".into()),
                    ContentBlock::Text("the answer".into()),
                ],
                usage: None,
                stop_reason: StopReason::Stop,
            }),
        ]
    );
}

#[test]
fn midstream_error_object_fails_with_http_zero() {
    let bytes = include_str!("fixtures/midstream_error.sse");
    let events = events_from_sse(bytes.as_bytes());
    assert_eq!(events.len(), 3);
    assert_eq!(events[1], StreamEvent::TextDelta("before".into()));
    match &events[2] {
        StreamEvent::Error(LlmError::Http { status, body }) => {
            assert_eq!(*status, 0);
            assert!(body.contains("quota exceeded"), "got: {body}");
        }
        other => panic!("expected Http error, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Framer-level behavior not covered by the replay fixtures
// ---------------------------------------------------------------------------

#[test]
fn framer_joins_multi_line_data_with_newlines() {
    let raw = b"data: {\"a\":\ndata: 1}\n\n";
    let mut framer = SseFramer::new();
    assert_eq!(framer.feed(raw), vec!["{\"a\":\n1}".to_string()]);
}

#[test]
fn framer_accepts_no_space_after_data_colon() {
    let raw = b"data:{\"x\":1}\n\n";
    let mut framer = SseFramer::new();
    assert_eq!(framer.feed(raw), vec!["{\"x\":1}".to_string()]);
}

#[test]
fn framer_flushes_event_without_trailing_blank_line() {
    let raw = b"data: {\"x\":1}";
    let mut framer = SseFramer::new();
    assert!(framer.feed(raw).is_empty());
    assert_eq!(framer.finish(), vec!["{\"x\":1}".to_string()]);
}

#[test]
fn framer_ignores_everything_after_done() {
    let raw = b"data: [DONE]\n\ndata: more\n\n";
    let mut framer = SseFramer::new();
    assert!(framer.feed(raw).is_empty());
    assert!(framer.is_done());
    assert!(framer.feed(b"data: even more\n\n").is_empty());
    assert!(framer.finish().is_empty());
}

#[test]
fn framer_handles_byte_at_a_time_feeding() {
    let raw = b": c\n\ndata: {\"k\":1}\n\ndata: [DONE]\n\n";
    let mut framer = SseFramer::new();
    let mut payloads = Vec::new();
    for byte in raw {
        payloads.extend(framer.feed(&[*byte]));
    }
    payloads.extend(framer.finish());
    assert_eq!(payloads, vec!["{\"k\":1}".to_string()]);
    assert!(framer.is_done());
}

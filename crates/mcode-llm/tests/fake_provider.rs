//! FakeProvider integration tests: turn-by-turn replay, request
//! recording, deterministic error turns, script exhaustion, JSON
//! loading, and cancellation during delayed streaming.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use mcode_core::message::{AssistantMessage, ContentBlock, StopReason, ToolCall, Usage};
use mcode_core::tool::ToolSpec;
use mcode_llm::CancellationToken;
use mcode_llm::error::LlmError;
use mcode_llm::provider::{Provider, Request, StreamEvent};
use mcode_llm::{FakeProvider, ScriptTurn, StreamExt};
use serde_json::json;

fn text_turn(text: &str) -> ScriptTurn {
    ScriptTurn::Message(AssistantMessage {
        blocks: vec![ContentBlock::Text(text.into())],
        usage: None,
        stop_reason: StopReason::Stop,
    })
}

fn tool_call_turn(text: &str, id: &str, name: &str, arguments: serde_json::Value) -> ScriptTurn {
    ScriptTurn::Message(AssistantMessage {
        blocks: vec![
            ContentBlock::Text(text.into()),
            ContentBlock::ToolCall(ToolCall {
                id: id.into(),
                name: name.into(),
                arguments,
            }),
        ],
        usage: Some(Usage {
            input_tokens: 11,
            output_tokens: 7,
        }),
        stop_reason: StopReason::ToolUse,
    })
}

async fn collect_all(provider: &FakeProvider, request: &Request) -> Vec<StreamEvent> {
    let cancel = CancellationToken::new();
    let stream = provider
        .stream(request, cancel)
        .await
        .expect("stream starts");
    let mut events = Vec::new();
    let mut stream = stream;
    while let Some(event) = stream.next().await {
        events.push(event);
    }
    events
}

fn sample_request(label: &str) -> Request {
    Request::new("fake-model")
        .with_system_prompt("sys")
        .with_message(mcode_core::Message::User(mcode_core::UserMessage::text(
            label,
        )))
        .with_tool(ToolSpec {
            name: "read".into(),
            description: "read".into(),
            params_schema: json!({"type": "object"}),
        })
}

#[tokio::test]
async fn replays_script_turn_by_turn() {
    let provider = FakeProvider::new(vec![
        text_turn("first answer"),
        tool_call_turn("reading", "call_1", "read", json!({"path": "Cargo.toml"})),
    ]);

    // Turn 1: plain text, sharded into 16-char deltas.
    let events = collect_all(&provider, &sample_request("q1")).await;
    assert_eq!(
        events,
        vec![
            StreamEvent::Start,
            StreamEvent::TextDelta("first answer".into()),
            StreamEvent::Done {
                message: AssistantMessage {
                    blocks: vec![ContentBlock::Text("first answer".into())],
                    usage: None,
                    stop_reason: StopReason::Stop,
                }
            },
        ]
    );
    assert_eq!(provider.remaining_turns(), 1);

    // Turn 2: text + tool call; argument JSON sharded into deltas and
    // aggregated back by ToolCallEnd.
    let events = collect_all(&provider, &sample_request("q2")).await;
    let expected_call = ToolCall {
        id: "call_1".into(),
        name: "read".into(),
        arguments: json!({"path": "Cargo.toml"}),
    };
    assert_eq!(
        events,
        vec![
            StreamEvent::Start,
            StreamEvent::TextDelta("reading".into()),
            StreamEvent::ToolCallDelta {
                id: "call_1".into(),
                partial_json: "{\"path\":\"Cargo.t".into(),
            },
            StreamEvent::ToolCallDelta {
                id: "call_1".into(),
                partial_json: "oml\"}".into(),
            },
            StreamEvent::ToolCallEnd(expected_call.clone()),
            StreamEvent::Done {
                message: AssistantMessage {
                    blocks: vec![
                        ContentBlock::Text("reading".into()),
                        ContentBlock::ToolCall(expected_call),
                    ],
                    usage: Some(Usage {
                        input_tokens: 11,
                        output_tokens: 7,
                    }),
                    stop_reason: StopReason::ToolUse,
                }
            },
        ]
    );
    assert_eq!(provider.remaining_turns(), 0);
}

#[tokio::test]
async fn records_every_request_for_assertions() {
    let provider = FakeProvider::new(vec![text_turn("a"), text_turn("b")]);
    let first = sample_request("first");
    let second = sample_request("second");

    let _ = collect_all(&provider, &first).await;
    let _ = collect_all(&provider, &second).await;

    let recorded = provider.recorded_requests();
    assert_eq!(recorded.len(), 2);
    assert_eq!(recorded[0], first);
    assert_eq!(recorded[1], second);
    assert_eq!(recorded[0].model.as_str(), "fake-model");
    assert_eq!(recorded[0].tools.len(), 1);
}

#[tokio::test]
async fn error_turn_fails_deterministically() {
    let provider = FakeProvider::new(vec![
        text_turn("ok"),
        ScriptTurn::Error(LlmError::Http {
            status: 429,
            body: "rate limited".into(),
        }),
    ]);

    let events = collect_all(&provider, &sample_request("q")).await;
    assert!(matches!(events.last(), Some(StreamEvent::Done { .. })));

    let events = collect_all(&provider, &sample_request("q")).await;
    assert_eq!(
        events,
        vec![
            StreamEvent::Start,
            StreamEvent::Error(LlmError::Http {
                status: 429,
                body: "rate limited".into(),
            }),
        ]
    );
}

#[tokio::test]
async fn exhausted_script_errors_deterministically() {
    let provider = FakeProvider::new(vec![text_turn("only")]);
    let _ = collect_all(&provider, &sample_request("q")).await;

    let err = provider
        .stream(&sample_request("q2"), CancellationToken::new())
        .await
        .unwrap_err();
    assert!(matches!(err, LlmError::Config(ref msg) if msg.contains("exhausted")));
    // The failed call is still recorded.
    assert_eq!(provider.recorded_requests().len(), 2);
}

#[tokio::test]
async fn loads_script_from_json_file() {
    let dir = TempDir::new("fake-script");
    let script_path = dir.write(
        "script.json",
        r#"[
            {"text": "I'll read the file.", "tool_calls": [
                {"id": "call_1", "name": "read", "arguments": {"path": "Cargo.toml"}}
            ], "usage": {"input_tokens": 3, "output_tokens": 4}},
            {"text": "done", "stop_reason": "Stop"}
        ]"#,
    );

    let provider = FakeProvider::from_json_file(&script_path).expect("script loads");
    let events = collect_all(&provider, &sample_request("q")).await;

    // Start, 2 text shards, 2 argument shards, ToolCallEnd, Done.
    assert_eq!(events.len(), 7);
    let StreamEvent::Done { message } = events.last().unwrap() else {
        panic!("expected Done");
    };
    assert_eq!(message.stop_reason, StopReason::ToolUse);
    assert_eq!(
        message.usage,
        Some(Usage {
            input_tokens: 3,
            output_tokens: 4,
        })
    );
    assert!(matches!(message.blocks[0], ContentBlock::Text(_)));
    assert!(matches!(message.blocks[1], ContentBlock::ToolCall(_)));

    // Turn 2 from the same file.
    let events = collect_all(&provider, &sample_request("q")).await;
    assert!(matches!(events.last(), Some(StreamEvent::Done { .. })));
}

#[tokio::test]
async fn invalid_script_file_is_a_config_error() {
    let dir = TempDir::new("bad-script");
    let bad = dir.write("bad.json", "[{\"text\": ");
    let err = FakeProvider::from_json_file(&bad).unwrap_err();
    assert!(matches!(err, LlmError::Config(_)));

    let missing = dir.path().join("nope.json");
    let err = FakeProvider::from_json_file(&missing).unwrap_err();
    assert!(matches!(err, LlmError::Config(_)));
}

#[tokio::test]
async fn cancellation_stops_delayed_streaming() {
    // Long text (13 shards) with a per-event delay; cancel mid-stream.
    let long_text: String = "abcdefghij0123".repeat(13);
    let provider =
        FakeProvider::new(vec![text_turn(&long_text)]).with_delay(Duration::from_millis(5));

    let cancel = CancellationToken::new();
    let mut stream = provider
        .stream(&sample_request("q"), cancel.clone())
        .await
        .unwrap();

    let mut events = Vec::new();
    while let Some(event) = stream.next().await {
        events.push(event.clone());
        if matches!(event, StreamEvent::TextDelta(_)) {
            cancel.cancel();
        }
    }

    // Iteration terminated; the producer stopped without a Done and the
    // stream surfaced cancellation.
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, StreamEvent::Done { .. })),
        "cancelled stream must not complete: {events:?}"
    );
    assert_eq!(
        events.last(),
        Some(&StreamEvent::Error(LlmError::Cancelled))
    );
}

#[tokio::test]
async fn push_turn_extends_script_mid_test() {
    let provider = FakeProvider::new(vec![]);
    provider.push_turn(text_turn("late"));
    let events = collect_all(&provider, &sample_request("q")).await;
    assert!(matches!(events.last(), Some(StreamEvent::Done { .. })));
}

/// Minimal temp-dir guard (shared shape with the auth unit tests).
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "mcode-llm-fake-{tag}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        Self(dir)
    }

    fn write(&self, name: &str, content: &str) -> PathBuf {
        let path = self.0.join(name);
        std::fs::write(&path, content).expect("write temp file");
        path
    }

    fn path(&self) -> &PathBuf {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

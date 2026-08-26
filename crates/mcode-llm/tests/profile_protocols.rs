//! End-to-end HTTP tests for Responses and Anthropic profile adapters.

mod common;

use std::time::Duration;

use common::{MockResponse, MockServer};
use mcode_core::message::{
    AssistantMessage, ContentBlock, Message, ReplayState, ReplayWire, StopReason, TextBlock,
    ThinkingBlock, ToolCall, ToolResultMessage, Usage, UserMessage,
};
use mcode_core::tool::ToolSpec;
use mcode_llm::{
    AnthropicAggregator, AuthProfile, CancellationToken, ClientIdentity, HeaderOverlay, LlmError,
    ModelSettings, ProfileProvider, Provider, Request, StreamEvent, StreamExt, WireKind,
    anthropic_profile, deepseek_profile, openai_profile,
};
use serde_json::{Value, json};

fn chunked_sse(events: &[Value]) -> MockResponse {
    let body: String = events
        .iter()
        .map(|event| format!("data: {event}\n\n"))
        .collect();
    MockResponse::sse(body.as_bytes().chunks(29).map(<[u8]>::to_vec).collect())
}

fn sample_request(model: &str) -> Request {
    Request::new(model)
        .with_system_prompt("be concise")
        .with_message(Message::User(UserMessage::text("read x")))
        .with_tool(ToolSpec {
            name: "read".into(),
            description: "Read a file".into(),
            params_schema: json!({
                "type": "object",
                "properties": {"path": {"type": "string"}}
            }),
        })
}

async fn collect(provider: &ProfileProvider, request: &Request) -> Vec<StreamEvent> {
    let mut stream = provider
        .stream(request, CancellationToken::new())
        .await
        .expect("stream starts");
    let mut events = Vec::new();
    while let Some(event) = stream.next().await {
        events.push(event);
    }
    events
}

#[tokio::test]
async fn responses_posts_expected_headers_and_finishes_without_done_sentinel() {
    let response = chunked_sse(&[
        json!({"type":"response.output_item.added","output_index":0,"item":{"type":"function_call","id":"fc_1","call_id":"call_1","name":"read","arguments":""}}),
        json!({"type":"response.function_call_arguments.delta","item_id":"fc_1","output_index":0,"delta":"{\"path\":\"x\"}"}),
        json!({"type":"response.output_item.done","output_index":0,"item":{"type":"function_call","id":"fc_1","call_id":"call_1","name":"read","arguments":"{\"path\":\"x\"}"}}),
        json!({"type":"response.reasoning_text.delta","delta":"checking"}),
        json!({"type":"response.output_text.delta","delta":"done"}),
        json!({"type":"response.completed","response":{"status":"completed","usage":{"input_tokens":5,"output_tokens":6}}}),
    ]);
    let mut server = MockServer::spawn(vec![response]);
    let profile = openai_profile()
        .with_base_url(server.base_url())
        .expect("local base URL");
    let provider = ProfileProvider::new(profile, "sk-test")
        .unwrap()
        .with_identity(ClientIdentity::pi("linux", "6.8", "x64").unwrap());
    let request = sample_request("gpt-test")
        .with_message(Message::Assistant(AssistantMessage {
            blocks: vec![
                ContentBlock::Thinking(
                    ThinkingBlock::new("checked").with_replay(
                        ReplayState::new(
                            ReplayWire::OpenAiResponses,
                            json!({
                                "type":"reasoning",
                                "id":"rs_previous",
                                "status":"completed",
                                "summary":[{"type":"summary_text","text":"checked"}],
                                "encrypted_content":"opaque-previous"
                            })
                            .to_string(),
                        )
                        .with_provider("openai")
                        .with_endpoint(server.base_url()),
                    ),
                ),
                ContentBlock::Text(TextBlock::new("previous answer")),
                ContentBlock::ToolCall(
                    ToolCall::new("previous", "read", json!({"path":"old"}))
                        .with_item_id("fc_previous"),
                ),
            ],
            usage: None,
            stop_reason: StopReason::ToolUse,
        }))
        .with_message(Message::ToolResult(ToolResultMessage {
            tool_call_id: "previous".into(),
            content: vec![ContentBlock::Text("old data".into())],
            is_error: false,
            details: None,
        }));

    let events = collect(&provider, &request).await;
    assert_eq!(events[0], StreamEvent::Start);
    assert!(events.contains(&StreamEvent::ThinkingDelta("checking".into())));
    assert!(events.contains(&StreamEvent::TextDelta("done".into())));
    let StreamEvent::Done { message } = events.last().unwrap() else {
        panic!("expected Done: {events:?}");
    };
    assert_eq!(message.stop_reason, StopReason::ToolUse);
    assert_eq!(
        message.usage,
        Some(Usage {
            input_tokens: 5,
            output_tokens: 6
        })
    );
    assert!(message.blocks.iter().any(|block| {
        matches!(block, ContentBlock::ToolCall(call) if call.arguments == json!({"path":"x"}))
    }));

    let captured = server.request().await;
    assert_eq!(captured.method, "POST");
    assert_eq!(captured.path, "/responses");
    assert_eq!(captured.header("authorization"), Some("Bearer sk-test"));
    assert_eq!(captured.header("user-agent"), Some("pi (linux 6.8; x64)"));
    assert!(
        captured
            .header("user-agent")
            .is_none_or(|value| !value.contains("mcode/"))
    );
    let body = captured.json();
    assert_eq!(body["model"], "gpt-test");
    assert_eq!(body["instructions"], "be concise");
    assert_eq!(body["stream"], true);
    assert_eq!(body["tools"][0]["type"], "function");
    let input = body["input"].as_array().unwrap();
    assert!(input.iter().any(|item| {
        item["type"] == "reasoning"
            && item["id"] == "rs_previous"
            && item["encrypted_content"] == "opaque-previous"
    }));
    assert!(
        input.iter().any(|item| {
            item["role"] == "assistant" && item["content"][0]["type"] == "input_text"
        })
    );
    assert!(input.iter().any(|item| {
        item["type"] == "function_call"
            && item["id"] == "fc_previous"
            && item["call_id"] == "previous"
    }));
    assert!(
        input.iter().any(|item| {
            item["type"] == "function_call_output" && item["call_id"] == "previous"
        })
    );
    assert!(!body.to_string().contains("output_text"));
}

#[tokio::test]
async fn deepseek_reuses_chat_adapter_and_custom_overlay_is_last() {
    let response = MockResponse::sse(vec![
        b"data: {\"choices\":[{\"delta\":{\"content\":\"ok\"},\"finish_reason\":\"stop\"}]}\n\n"
            .to_vec(),
        b"data: [DONE]\n\n".to_vec(),
    ]);
    let mut server = MockServer::spawn(vec![response]);
    let profile = deepseek_profile()
        .with_base_url(server.base_url())
        .expect("local base URL");
    let mut model_headers = HeaderOverlay::new();
    model_headers
        .insert("authorization", "Bearer model-must-not-win")
        .unwrap();
    model_headers.insert("x-order", "model").unwrap();
    let mut explicit_headers = HeaderOverlay::new();
    explicit_headers
        .insert("authorization", "Bearer explicit")
        .unwrap();
    explicit_headers.insert("x-order", "explicit").unwrap();
    let provider = ProfileProvider::new(profile, "profile-key")
        .unwrap()
        .with_provider_settings(ModelSettings {
            headers: model_headers,
            ..ModelSettings::default()
        })
        .with_headers(explicit_headers);

    let events = collect(&provider, &sample_request("deepseek-chat")).await;
    assert!(matches!(events.last(), Some(StreamEvent::Done { .. })));
    let captured = server.request().await;
    assert_eq!(captured.path, "/chat/completions");
    assert_eq!(captured.header("authorization"), Some("Bearer explicit"));
    assert_eq!(captured.header("x-order"), Some("explicit"));
    assert_eq!(captured.json()["model"], "deepseek-chat");
}

#[tokio::test]
async fn anthropic_posts_messages_headers_and_decodes_partial_json() {
    let response = chunked_sse(&[
        json!({"type":"message_start","message":{"usage":{"input_tokens":9,"output_tokens":0}}}),
        json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"hmm"}}),
        json!({"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}),
        json!({"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"reading"}}),
        json!({"type":"content_block_start","index":2,"content_block":{"type":"tool_use","id":"tool_1","name":"read","input":{}}}),
        json!({"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"{\"path\":"}}),
        json!({"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"\"x\"}"}}),
        json!({"type":"content_block_stop","index":2}),
        json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":4}}),
        json!({"type":"message_stop"}),
    ]);
    let mut server = MockServer::spawn(vec![response]);
    let profile = anthropic_profile()
        .with_base_url(server.base_url())
        .expect("local base URL");
    let provider = ProfileProvider::new(profile, "anthropic-test")
        .unwrap()
        .with_identity(ClientIdentity::pi("darwin", "24.0", "arm64").unwrap());
    let request = sample_request("claude-test")
        .with_message(Message::Assistant(AssistantMessage {
            blocks: vec![
                ContentBlock::Thinking(
                    ThinkingBlock::new("prior thought").with_replay(
                        ReplayState::new(ReplayWire::AnthropicMessages, "prior-signature")
                            .with_provider("anthropic")
                            .with_endpoint(server.base_url()),
                    ),
                ),
                ContentBlock::ToolCall(ToolCall::new("previous", "read", json!({"path":"old"}))),
            ],
            usage: None,
            stop_reason: StopReason::ToolUse,
        }))
        .with_message(Message::ToolResult(ToolResultMessage {
            tool_call_id: "previous".into(),
            content: vec![ContentBlock::Text("old data".into())],
            is_error: false,
            details: None,
        }));

    let events = collect(&provider, &request).await;
    assert!(events.contains(&StreamEvent::ThinkingDelta("hmm".into())));
    let call = events
        .iter()
        .find_map(|event| match event {
            StreamEvent::ToolCallEnd(call) => Some(call),
            _ => None,
        })
        .expect("tool call end");
    assert_eq!(call.arguments, json!({"path":"x"}));
    let StreamEvent::Done { message } = events.last().unwrap() else {
        panic!("expected Done");
    };
    assert_eq!(
        message.usage,
        Some(Usage {
            input_tokens: 9,
            output_tokens: 4
        })
    );

    let captured = server.request().await;
    assert_eq!(captured.path, "/v1/messages");
    assert_eq!(captured.header("x-api-key"), Some("anthropic-test"));
    assert_eq!(captured.header("authorization"), None);
    assert_eq!(captured.header("anthropic-version"), Some("2023-06-01"));
    assert_eq!(
        captured.header("user-agent"),
        Some("pi (darwin 24.0; arm64)")
    );
    let body = captured.json();
    assert_eq!(body["system"], "be concise");
    assert_eq!(body["max_tokens"], 8_192);
    assert!(body["messages"].as_array().unwrap().iter().any(|message| {
        message["content"]
            .as_array()
            .is_some_and(|content| content.iter().any(|block| block["type"] == "tool_result"))
    }));
    let assistant = &body["messages"][1]["content"];
    assert_eq!(assistant[0]["type"], "thinking");
    assert_eq!(assistant[0]["thinking"], "prior thought");
    assert_eq!(assistant[0]["signature"], "prior-signature");
    assert_eq!(assistant[1]["type"], "tool_use");
}

#[tokio::test]
async fn anthropic_wire_can_use_bearer_auth() {
    let response = chunked_sse(&[json!({"type":"message_stop"})]);
    let mut server = MockServer::spawn(vec![response]);
    let profile = mcode_llm::ProviderProfile::new(
        "anthropic-bearer",
        WireKind::AnthropicMessages,
        server.base_url(),
        AuthProfile::bearer("ANTHROPIC_TOKEN"),
    )
    .unwrap();
    let provider = ProfileProvider::new(profile, "bearer-test").unwrap();
    let events = collect(&provider, &sample_request("claude-test")).await;
    assert!(matches!(events.last(), Some(StreamEvent::Done { .. })));
    let captured = server.request().await;
    assert_eq!(captured.header("authorization"), Some("Bearer bearer-test"));
    assert_eq!(captured.header("x-api-key"), None);
    assert_eq!(captured.header("anthropic-version"), Some("2023-06-01"));
}

#[tokio::test]
async fn cross_origin_redirect_does_not_replay_post_or_credentials() {
    let sink_response = MockResponse::json("200 OK", &[], json!({"unexpected": true}));
    let sink = MockServer::spawn(vec![sink_response]);
    let location = format!("{}/capture", sink.base_url());
    let redirect = MockResponse::json(
        "307 Temporary Redirect",
        &[("Location", location.as_str())],
        json!({"error": "redirect blocked"}),
    );
    let mut source = MockServer::spawn(vec![redirect]);
    let source_origin = source.base_url();
    let profile = anthropic_profile()
        .with_base_url(&source_origin)
        .expect("local base URL");
    let mut custom_headers = HeaderOverlay::new();
    custom_headers
        .insert("x-provider-credential", "custom-secret")
        .unwrap();
    let provider = ProfileProvider::new(profile, "anthropic-secret")
        .unwrap()
        .with_headers(custom_headers);
    let request =
        sample_request("claude-test").with_message(Message::Assistant(AssistantMessage {
            blocks: vec![ContentBlock::Thinking(
                ThinkingBlock::new("private thought").with_replay(
                    ReplayState::new(ReplayWire::AnthropicMessages, "opaque-replay")
                        .with_provider("anthropic")
                        .with_endpoint(&source_origin),
                ),
            )],
            usage: None,
            stop_reason: StopReason::Stop,
        }));

    let events = collect(&provider, &request).await;
    assert!(matches!(
        events.as_slice(),
        [StreamEvent::Error(LlmError::Http { status: 307, .. })]
    ));
    let captured = source.request().await;
    assert_eq!(captured.header("x-api-key"), Some("anthropic-secret"));
    assert_eq!(
        captured.header("x-provider-credential"),
        Some("custom-secret")
    );
    assert!(captured.json().to_string().contains("opaque-replay"));

    // A broken policy would replay the POST immediately; once collection has
    // returned, a short deadline is enough to prove the sink saw no request.
    assert!(
        tokio::time::timeout(
            Duration::from_millis(100),
            sink.wait_for_completed_connections(1)
        )
        .await
        .is_err(),
        "cross-origin redirect was followed"
    );
}

#[tokio::test]
async fn errors_are_bounded_redacted_and_malformed_sse_fails() {
    let secret = "sk-server-echoed-secret";
    let response = MockResponse::json(
        "401 Unauthorized",
        &[],
        json!({"error": "x".repeat(2_000), "api_key": secret}),
    );
    let malformed = MockResponse::sse(vec![b"data: not-json\n\n".to_vec()]);
    let mut server = MockServer::spawn(vec![response, malformed]);
    let server_url = server.base_url();
    let profile = openai_profile().with_base_url(&server_url).unwrap();
    let provider = ProfileProvider::new(profile, "request-key").unwrap();

    let events = collect(&provider, &sample_request("gpt-test")).await;
    let StreamEvent::Error(LlmError::Http { status, body }) = &events[0] else {
        panic!("expected HTTP error: {events:?}");
    };
    assert_eq!(*status, 401);
    assert!(body.contains("REDACTED"));
    assert!(!body.contains(secret));
    assert!(body.chars().count() < 600);

    let events = collect(&provider, &sample_request("gpt-test")).await;
    assert_eq!(events[0], StreamEvent::Start);
    assert!(matches!(events[1], StreamEvent::Error(LlmError::Sse(_))));
    let _ = server.request().await;
    let _ = server.request().await;
}

#[tokio::test]
async fn plaintext_error_body_is_redacted_in_event_display_and_debug() {
    let response = MockResponse::chunks(
        "401 Unauthorized",
        "text/plain",
        &[],
        vec![
            concat!(
                "authorization=Bearer transport-auth ",
                "auth-key=transport-auth-key&access-key:transport-access-key\n",
                "cookie=session_id=transport-cookie; theme=dark\n",
                "password: 'transport password'",
            )
            .as_bytes()
            .to_vec(),
        ],
        Duration::ZERO,
    );
    let mut server = MockServer::spawn(vec![response]);
    let profile = openai_profile().with_base_url(server.base_url()).unwrap();
    let provider = ProfileProvider::new(profile, "key").unwrap();

    let events = collect(&provider, &sample_request("gpt-test")).await;
    let [StreamEvent::Error(error @ LlmError::Http { status, body })] = events.as_slice() else {
        panic!("expected one HTTP error: {events:?}");
    };
    assert_eq!(*status, 401);
    for rendered in [body.clone(), error.to_string(), format!("{error:?}")] {
        assert!(rendered.contains("REDACTED"), "{rendered}");
        for secret in [
            "transport-auth",
            "transport-auth-key",
            "transport-access-key",
            "session_id=transport-cookie",
            "transport password",
        ] {
            assert!(!rendered.contains(secret), "leaked {secret}: {rendered}");
        }
    }
    let _ = server.request().await;
}

#[tokio::test]
async fn non_success_status_survives_error_body_read_failure() {
    let broken = MockResponse::chunks(
        "502 Bad Gateway",
        "application/json",
        &[("Content-Length", "128")],
        vec![b"{\"error\":\"short\"}".to_vec()],
        Duration::ZERO,
    );
    let mut server = MockServer::spawn(vec![broken]);
    let profile = openai_profile().with_base_url(server.base_url()).unwrap();
    let provider = ProfileProvider::new(profile, "key").unwrap();

    let events = collect(&provider, &sample_request("gpt-test")).await;
    let [StreamEvent::Error(LlmError::Http { status, body })] = events.as_slice() else {
        panic!("expected one HTTP error: {events:?}");
    };
    assert_eq!(*status, 502);
    assert!(
        body.contains("failed to read error response body"),
        "{body}"
    );
    let _ = server.request().await;
}

#[tokio::test]
async fn shared_transport_maps_timeout_and_cancellation() {
    let mut timeout_server = MockServer::spawn(vec![MockResponse::stall()]);
    let profile = openai_profile()
        .with_base_url(timeout_server.base_url())
        .unwrap();
    let provider = ProfileProvider::new(profile, "key")
        .unwrap()
        .with_timeout(Duration::from_millis(75));
    let events = collect(&provider, &sample_request("gpt-test")).await;
    assert_eq!(events, vec![StreamEvent::Error(LlmError::Timeout)]);
    let _ = timeout_server.request().await;

    let mut cancel_server = MockServer::spawn(vec![MockResponse::stall()]);
    let profile = anthropic_profile()
        .with_base_url(cancel_server.base_url())
        .unwrap();
    let provider = ProfileProvider::new(profile, "key").unwrap();
    let cancel = CancellationToken::new();
    let mut stream = provider
        .stream(&sample_request("claude-test"), cancel.clone())
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(25)).await;
    cancel.cancel();
    assert_eq!(
        stream.next().await,
        Some(StreamEvent::Error(LlmError::Cancelled))
    );
    assert!(stream.next().await.is_none());
    let _ = cancel_server.request().await;
}

#[tokio::test]
async fn responses_stream_stamps_provenance_and_preserves_phases_on_replay() {
    let reasoning_item = json!({
        "type": "reasoning",
        "id": "rs_1",
        "status": "completed",
        "summary": [{"type": "summary_text", "text": "checked"}],
        "encrypted_content": "opaque-state"
    });
    let commentary_item = json!({
        "type": "message",
        "id": "msg_1",
        "role": "assistant",
        "phase": "commentary",
        "status": "completed",
        "content": [{"type": "output_text", "text": "let me look"}]
    });
    let final_item = json!({
        "type": "message",
        "id": "msg_2",
        "role": "assistant",
        "phase": "final_answer",
        "status": "completed",
        "content": [{"type": "output_text", "text": "all done"}]
    });
    let first = chunked_sse(&[
        json!({"type":"response.output_item.added","output_index":0,"item":{"type":"reasoning","id":"rs_1","summary":[]}}),
        json!({"type":"response.reasoning_summary_text.delta","item_id":"rs_1","delta":"checked"}),
        json!({"type":"response.output_item.added","output_index":1,"item":{"type":"message","id":"msg_1","role":"assistant","phase":"commentary","status":"in_progress","content":[]}}),
        json!({"type":"response.output_text.delta","item_id":"msg_1","delta":"let me look"}),
        json!({"type":"response.output_item.added","output_index":2,"item":{"type":"message","id":"msg_2","role":"assistant","phase":"final_answer","status":"in_progress","content":[]}}),
        json!({"type":"response.output_text.delta","item_id":"msg_2","delta":"all done"}),
        json!({"type":"response.completed","response":{"status":"completed","output":[reasoning_item, commentary_item, final_item]}}),
    ]);
    // Second response: a minimal terminal so the replayed request can be
    // captured without another full turn.
    let second =
        chunked_sse(&[json!({"type":"response.completed","response":{"status":"completed"}})]);
    let mut server = MockServer::spawn(vec![first, second]);
    let profile = openai_profile()
        .with_base_url(server.base_url())
        .expect("local base URL");
    let provider = ProfileProvider::new(profile, "sk-test").unwrap();

    let events = collect(&provider, &sample_request("gpt-test")).await;
    let StreamEvent::Done { message } = events.last().unwrap() else {
        panic!("expected Done: {events:?}");
    };
    let first_captured = server.request().await;
    assert_eq!(first_captured.json()["input"][0]["role"], "user");
    let thinking = message
        .blocks
        .iter()
        .find_map(|block| match block {
            ContentBlock::Thinking(thinking) => Some(thinking),
            _ => None,
        })
        .expect("reasoning block");
    let replay = thinking.replay.as_ref().expect("replay state");
    assert_eq!(replay.wire, ReplayWire::OpenAiResponses);
    // The transport stamps the producing profile id and endpoint origin
    // onto the state.
    assert_eq!(replay.provider.as_deref(), Some("openai"));
    assert_eq!(replay.endpoint.as_deref(), Some(server.base_url().as_str()));
    let texts: Vec<(&str, Option<mcode_core::message::AssistantPhase>)> = message
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
                "let me look",
                Some(mcode_core::message::AssistantPhase::Commentary)
            ),
            (
                "all done",
                Some(mcode_core::message::AssistantPhase::FinalAnswer)
            ),
        ]
    );

    // Same provider resumes: replay the persisted message verbatim.
    let request = Request::new("gpt-test").with_message(Message::Assistant(message.clone()));
    let events = collect(&provider, &request).await;
    assert!(matches!(events.last(), Some(StreamEvent::Done { .. })));
    let captured = server.request().await;
    let input = captured.json()["input"].as_array().unwrap().clone();
    assert_eq!(
        input_shapes(&input),
        vec![
            "reasoning:",
            "commentary:assistant",
            "final_answer:assistant"
        ]
    );
    assert_eq!(input[0]["id"], "rs_1");
    assert_eq!(input[0]["encrypted_content"], "opaque-state");
}

fn input_shapes(input: &[Value]) -> Vec<String> {
    input
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
        .collect()
}

#[tokio::test]
async fn same_wire_profile_switch_needs_complete_explicit_replay_trust() {
    // A different Anthropic-Messages profile (bearer gateway) points at
    // an unrelated host. Wire equality alone is not a trust boundary,
    // and explicit provider trust never substitutes for endpoint
    // provenance on either side.
    let response = chunked_sse(&[json!({"type":"message_stop"})]);
    let mut server = MockServer::spawn(vec![response.clone(), response.clone(), response]);
    let gateway = mcode_llm::ProviderProfile::new(
        "anthropic-gateway",
        WireKind::AnthropicMessages,
        server.base_url(),
        AuthProfile::bearer("ANTHROPIC_TOKEN"),
    )
    .unwrap();
    let history = |endpoint: Option<&str>| {
        let replay = ReplayState::new(ReplayWire::AnthropicMessages, "prior-signature")
            .with_provider("anthropic");
        let replay = match endpoint {
            Some(endpoint) => replay.with_endpoint(endpoint),
            None => replay,
        };
        sample_request("claude-test").with_message(Message::Assistant(AssistantMessage {
            blocks: vec![ContentBlock::Thinking(
                ThinkingBlock::new("prior thought").with_replay(replay),
            )],
            usage: None,
            stop_reason: StopReason::Stop,
        }))
    };

    // Complete but untrusted cross-profile provenance is downgraded.
    let provider = ProfileProvider::new(gateway.clone(), "gateway-key").unwrap();
    let events = collect(&provider, &history(Some("https://api.anthropic.com"))).await;
    assert!(matches!(events.last(), Some(StreamEvent::Done { .. })));
    let captured = server.request().await;
    let content = captured.json()["messages"][1]["content"].to_string();
    assert!(!content.contains("signature"), "{content}");
    assert!(!content.contains("prior-signature"), "{content}");
    assert!(content.contains("prior thought"), "{content}");

    let trusting = gateway.with_trusted_replay_provider("anthropic").unwrap();

    // Trust cannot make producer state with no endpoint replayable.
    let provider = ProfileProvider::new(trusting.clone(), "gateway-key").unwrap();
    let events = collect(&provider, &history(None)).await;
    assert!(matches!(events.last(), Some(StreamEvent::Done { .. })));
    let captured = server.request().await;
    let content = captured.json()["messages"][1]["content"].to_string();
    assert!(!content.contains("signature"), "{content}");
    assert!(!content.contains("prior-signature"), "{content}");
    assert!(content.contains("prior thought"), "{content}");

    // Explicit trust plus valid producer and consumer origins replays.
    let provider = ProfileProvider::new(trusting, "gateway-key").unwrap();
    let events = collect(&provider, &history(Some("https://api.anthropic.com"))).await;
    assert!(matches!(events.last(), Some(StreamEvent::Done { .. })));
    let captured = server.request().await;
    let body = captured.json();
    let content = &body["messages"][1]["content"];
    assert_eq!(content[0]["type"], "thinking");
    assert_eq!(content[0]["thinking"], "prior thought");
    assert_eq!(content[0]["signature"], "prior-signature");
}

#[tokio::test]
async fn same_profile_id_on_redirected_endpoint_downgrades_replay() {
    // The built-in `openai` id survives a base-URL override (that is how
    // `OPENAI_BASE_URL` is applied). A session recorded on the official
    // endpoint must therefore NOT replay its encrypted reasoning
    // verbatim to the redirected profile: the id alone is not a trust
    // boundary — the endpoint origin is.
    let response =
        chunked_sse(&[json!({"type":"response.completed","response":{"status":"completed"}})]);
    let mut server = MockServer::spawn(vec![response]);
    let profile = openai_profile()
        .with_base_url(server.base_url())
        .expect("local base URL");
    let provider = ProfileProvider::new(profile, "sk-test").unwrap();
    let request = sample_request("gpt-test").with_message(Message::Assistant(AssistantMessage {
        blocks: vec![ContentBlock::Thinking(
            ThinkingBlock::new("checked").with_replay(
                ReplayState::new(
                    ReplayWire::OpenAiResponses,
                    json!({
                        "type":"reasoning",
                        "id":"rs_official",
                        "status":"completed",
                        "summary":[{"type":"summary_text","text":"checked"}],
                        "encrypted_content":"opaque-official"
                    })
                    .to_string(),
                )
                .with_provider("openai")
                .with_endpoint("https://api.openai.com"),
            ),
        )],
        usage: None,
        stop_reason: StopReason::Stop,
    }));

    let events = collect(&provider, &request).await;
    assert!(matches!(events.last(), Some(StreamEvent::Done { .. })));
    let captured = server.request().await;
    let input = captured.json()["input"].to_string();
    assert!(!input.contains("opaque-official"), "{input}");
    assert!(!input.contains("rs_official"), "{input}");
    assert!(input.contains("checked"), "{input}");
}

#[tokio::test]
async fn cross_provider_resume_strips_wire_only_state() {
    // Responses history replayed to the Anthropic wire: the reasoning JSON
    // must not become a thinking signature; the visible text survives.
    let anthropic_side = chunked_sse(&[json!({"type":"message_stop"})]);
    let mut server = MockServer::spawn(vec![anthropic_side]);
    let profile = anthropic_profile()
        .with_base_url(server.base_url())
        .expect("local base URL");
    let provider = ProfileProvider::new(profile, "key").unwrap();
    let request =
        sample_request("claude-test").with_message(Message::Assistant(AssistantMessage {
            blocks: vec![ContentBlock::Thinking(
                ThinkingBlock::new("checked upstream").with_replay(
                    ReplayState::new(
                        ReplayWire::OpenAiResponses,
                        json!({
                            "type":"reasoning",
                            "id":"rs_1",
                            "encrypted_content":"opaque"
                        })
                        .to_string(),
                    )
                    .with_provider("openai"),
                ),
            )],
            usage: None,
            stop_reason: StopReason::Stop,
        }));
    let events = collect(&provider, &request).await;
    assert!(matches!(events.last(), Some(StreamEvent::Done { .. })));
    let captured = server.request().await;
    let content = captured.json()["messages"][1]["content"].to_string();
    assert!(!content.contains("signature"), "{content}");
    assert!(!content.contains("encrypted"), "{content}");
    assert!(content.contains("checked upstream"), "{content}");

    // Anthropic history replayed to the Responses wire: no reasoning item
    // is fabricated from the foreign signature.
    let responses_side =
        chunked_sse(&[json!({"type":"response.completed","response":{"status":"completed"}})]);
    let mut server = MockServer::spawn(vec![responses_side]);
    let profile = openai_profile()
        .with_base_url(server.base_url())
        .expect("local base URL");
    let provider = ProfileProvider::new(profile, "key").unwrap();
    let request = sample_request("gpt-test").with_message(Message::Assistant(AssistantMessage {
        blocks: vec![ContentBlock::Thinking(
            ThinkingBlock::new("prior thought").with_replay(
                ReplayState::new(ReplayWire::AnthropicMessages, "prior-signature")
                    .with_provider("anthropic"),
            ),
        )],
        usage: None,
        stop_reason: StopReason::Stop,
    }));
    let events = collect(&provider, &request).await;
    assert!(matches!(events.last(), Some(StreamEvent::Done { .. })));
    let captured = server.request().await;
    let input = captured.json()["input"].to_string();
    assert!(!input.contains("reasoning"), "{input}");
    assert!(!input.contains("prior-signature"), "{input}");
    assert!(input.contains("prior thought"), "{input}");
}

/// Asserts that the provider transport releases its connection by a deadline.
async fn assert_connection_completed(server: &MockServer, expected: usize) {
    tokio::time::timeout(
        Duration::from_secs(3),
        server.wait_for_completed_connections(expected),
    )
    .await
    .expect("transport task did not release its connection after the EventStream was dropped");
}

#[tokio::test]
async fn dropped_event_stream_stops_the_http_task_mid_stream() {
    // Headers go out immediately, but the first body chunk is held back
    // for a long time. Once the consumer drops the EventStream, the
    // transport task must stop reading instead of parking itself (and
    // its connection) on network data nobody will consume.
    let response = MockResponse::chunks(
        "200 OK",
        "text/event-stream",
        &[],
        vec![b"data: {\"type\":\"message_stop\"}\n\n".to_vec()],
        Duration::from_secs(30),
    );
    let server = MockServer::spawn(vec![response]);
    let profile = anthropic_profile()
        .with_base_url(server.base_url())
        .expect("local base URL");
    let provider = ProfileProvider::new(profile, "key").unwrap();
    let mut stream = provider
        .stream(&sample_request("claude-test"), CancellationToken::new())
        .await
        .expect("stream starts");
    assert_eq!(stream.next().await, Some(StreamEvent::Start));
    drop(stream);
    assert_connection_completed(&server, 1).await;
}

#[tokio::test]
async fn dropped_event_stream_stops_the_http_task_on_stalled_error_body() {
    // A non-2xx status whose body never arrives: the transport parks on
    // the error-body read. Once the consumer drops the EventStream, that
    // read must be abandoned instead of holding the task and connection
    // open forever (the error-body branch needs the same liveness as the
    // send phase and the success-body loop).
    let response = MockResponse::chunks(
        "500 Internal Server Error",
        "application/json",
        &[],
        vec![b"{\"error\":\"slow\"}".to_vec()],
        Duration::from_secs(30),
    );
    let mut server = MockServer::spawn(vec![response]);
    let profile = openai_profile()
        .with_base_url(server.base_url())
        .expect("local base URL");
    let provider = ProfileProvider::new(profile, "key").unwrap();
    let stream = provider
        .stream(&sample_request("gpt-test"), CancellationToken::new())
        .await
        .expect("stream starts");
    // Do not drop until the non-success response head has actually been
    // written and flushed to the client. This deterministically crosses
    // the before-headers phase without relying on scheduler timing.
    tokio::time::timeout(Duration::from_secs(3), server.wait_for_response_head())
        .await
        .expect("mock server did not flush the response head");
    drop(stream);
    assert_connection_completed(&server, 1).await;
}

#[tokio::test]
async fn dropped_event_stream_stops_the_http_task_before_headers() {
    // The server accepts the request and never answers. Once headers are
    // pending, dropping the EventStream must abandon the in-flight request
    // rather than wait the stalled endpoint out. `request()` resolves only
    // after the connector established the socket and the request reached
    // the server, so dropping afterwards never races reqwest's connector
    // (a never-used socket would be managed by the client pool, not by our
    // transport task).
    let mut server = MockServer::spawn(vec![MockResponse::stall()]);
    let profile = openai_profile()
        .with_base_url(server.base_url())
        .expect("local base URL");
    let provider = ProfileProvider::new(profile, "key").unwrap();
    let stream = provider
        .stream(&sample_request("gpt-test"), CancellationToken::new())
        .await
        .expect("stream starts");
    let _ = server.request().await;
    drop(stream);
    assert_connection_completed(&server, 1).await;
}

#[test]
fn aggregator_types_remain_public_for_sans_io_testing() {
    let _ = AnthropicAggregator::new();
}

// Rust guideline compliant 2026-08-26

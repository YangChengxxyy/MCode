//! Message → OpenAI request serialization shape tests.

use mcode_core::message::{
    AssistantMessage, BinaryData, ContentBlock, Message, StopReason, ToolCall, ToolResultMessage,
    UserMessage,
};
use mcode_core::tool::ToolSpec;
use mcode_llm::chat_completions::build_request_body;
use mcode_llm::provider::{Request, ThinkingConfig, ThinkingLevel};
use mcode_llm::{AuthProfile, ProviderProfile, WireKind};
use serde_json::json;

fn read_spec() -> ToolSpec {
    ToolSpec {
        name: "read".into(),
        description: "Read a file".into(),
        params_schema: json!({
            "type": "object",
            "properties": {"path": {"type": "string"}},
            "required": ["path"]
        }),
    }
}

fn full_conversation_request() -> Request {
    Request::new("gpt-4o-mini")
        .with_system_prompt("prompt A")
        .with_system_prompt("prompt B")
        .with_message(Message::User(UserMessage::text("read Cargo.toml")))
        .with_message(Message::Assistant(AssistantMessage {
            blocks: vec![
                ContentBlock::Thinking("internal reasoning".into()),
                ContentBlock::Text("sure".into()),
                ContentBlock::ToolCall(ToolCall::new(
                    "call_1",
                    "read",
                    json!({"path": "Cargo.toml"}),
                )),
            ],
            usage: None,
            stop_reason: StopReason::ToolUse,
        }))
        .with_message(Message::ToolResult(ToolResultMessage {
            tool_call_id: "call_1".into(),
            content: vec![ContentBlock::Text("file contents".into())],
            is_error: false,
            details: Some(json!({"diff": "elided"})),
        }))
        .with_tool(read_spec())
}

#[test]
fn full_conversation_serializes_to_openai_chat_shape() {
    let body = build_request_body(&full_conversation_request());
    assert_eq!(
        body,
        json!({
            "model": "gpt-4o-mini",
            "messages": [
                {"role": "system", "content": "prompt A"},
                {"role": "system", "content": "prompt B"},
                {"role": "user", "content": "read Cargo.toml"},
                {
                    "role": "assistant",
                    "content": "sure",
                    "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": {
                            "name": "read",
                            "arguments": "{\"path\":\"Cargo.toml\"}"
                        }
                    }]
                },
                {"role": "tool", "tool_call_id": "call_1", "content": "file contents"}
            ],
            "tools": [{
                "type": "function",
                "function": {
                    "name": "read",
                    "description": "Read a file",
                    "parameters": {
                        "type": "object",
                        "properties": {"path": {"type": "string"}},
                        "required": ["path"]
                    }
                }
            }],
            "stream": true,
            "stream_options": {"include_usage": true}
        }),
        "thinking blocks must be dropped, details must stay out of the tool message"
    );
}

#[test]
fn minimal_request_omits_optional_fields() {
    let req = Request::new("m").with_message(Message::User(UserMessage::text("hi")));
    let body = build_request_body(&req);
    let map = body.as_object().unwrap();
    assert!(!map.contains_key("tools"));
    assert!(!map.contains_key("reasoning_effort"));
    assert_eq!(map.len(), 4); // model, messages, stream, stream_options
}

#[test]
fn thinking_maps_to_reasoning_effort() {
    let req = Request::new("o4-mini").with_thinking(ThinkingConfig {
        level: ThinkingLevel::High,
    });
    let body = build_request_body(&req);
    assert_eq!(body["reasoning_effort"], json!("high"));
}

#[test]
fn user_image_becomes_data_url_parts() {
    let req = Request::new("gpt-4o-mini").with_message(Message::User(UserMessage {
        content: vec![
            ContentBlock::Text("describe:".into()),
            ContentBlock::Image(BinaryData {
                data: "aGVsbG8=".into(),
                mime_type: "image/png".into(),
            }),
        ],
    }));
    let body = build_request_body(&req);
    assert_eq!(
        body["messages"][0],
        json!({
            "role": "user",
            "content": [
                {"type": "text", "text": "describe:"},
                {"type": "image_url", "image_url": {"url": "data:image/png;base64,aGVsbG8="}}
            ]
        })
    );
}

#[test]
fn tool_result_image_becomes_placeholder_plus_follow_up_user_message() {
    let req = Request::new("gpt-4o-mini").with_message(Message::ToolResult(ToolResultMessage {
        tool_call_id: "c1".into(),
        content: vec![ContentBlock::Image(BinaryData {
            data: "AAEC".into(),
            mime_type: "image/jpeg".into(),
        })],
        is_error: false,
        details: None,
    }));
    let body = build_request_body(&req);
    let messages = body["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 2);
    assert_eq!(
        messages[0],
        json!({"role": "tool", "tool_call_id": "c1", "content": "(see attached image)"})
    );
    assert_eq!(messages[1]["role"], "user");
    let parts = messages[1]["content"].as_array().unwrap();
    assert_eq!(parts[0]["text"], "Attached image(s) from tool result:");
    assert_eq!(parts[1]["image_url"]["url"], "data:image/jpeg;base64,AAEC");
}

#[test]
fn tool_call_only_assistant_message_omits_content() {
    let req = Request::new("m").with_message(Message::Assistant(AssistantMessage {
        blocks: vec![ContentBlock::ToolCall(ToolCall::new(
            "c9",
            "bash",
            json!({"command": "ls"}),
        ))],
        usage: None,
        stop_reason: StopReason::ToolUse,
    }));
    let body = build_request_body(&req);
    let assistant = &body["messages"][0];
    assert_eq!(assistant["role"], "assistant");
    assert!(assistant.get("content").is_none());
    assert_eq!(assistant["tool_calls"][0]["id"], "c9");
}

#[test]
fn custom_messages_are_skipped() {
    let req = Request::new("m")
        .with_message(Message::User(UserMessage::text("a")))
        .with_message(Message::Custom(mcode_core::CustomMessage {
            kind: "plugin:plan".into(),
            data: json!({"steps": []}),
        }))
        .with_message(Message::User(UserMessage::text("b")));
    let body = build_request_body(&req);
    let messages = body["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 2);
}

#[test]
fn base_url_normalization() {
    let profile = ProviderProfile::new(
        "local",
        WireKind::OpenAiChatCompletions,
        "https://api.example.com/v1/",
        AuthProfile::none(),
    )
    .unwrap();
    assert_eq!(
        profile.endpoint(),
        "https://api.example.com/v1/chat/completions"
    );
    assert_eq!(profile.id(), "local");

    let custom = ProviderProfile::new(
        "deepseek",
        WireKind::OpenAiChatCompletions,
        "https://deepseek.example",
        AuthProfile::none(),
    )
    .unwrap();
    assert_eq!(custom.id(), "deepseek");
    assert_eq!(
        custom.endpoint(),
        "https://deepseek.example/chat/completions"
    );
}

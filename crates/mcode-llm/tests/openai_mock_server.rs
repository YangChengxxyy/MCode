//! HTTP-level tests for chat-completions [`ProfileProvider`] against a
//! localhost mock server (no external network). Covers the full reqwest
//! path: request shape on the wire, SSE happy path, non-2xx error mapping,
//! timeout, and cancellation.

use std::net::SocketAddr;
use std::time::Duration;

use mcode_core::message::{ContentBlock, StopReason, Usage};
use mcode_llm::error::LlmError;
use mcode_llm::provider::{Provider, Request, StreamEvent};
use mcode_llm::{AuthProfile, ProfileProvider, ProviderProfile, WireKind};
use mcode_llm::{CancellationToken, StreamExt};
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// A request captured by the mock server.
#[derive(Debug)]
struct CapturedRequest {
    request_line: String,
    authorization: Option<String>,
    accept: Option<String>,
    body: serde_json::Value,
}

impl CapturedRequest {
    fn header(&self, name: &str) -> Option<&str> {
        match name {
            "authorization" => self.authorization.as_deref(),
            "accept" => self.accept.as_deref(),
            _ => None,
        }
    }
}

async fn read_request(stream: &mut TcpStream) -> Option<CapturedRequest> {
    let mut buf = Vec::new();
    // Read until end of headers.
    loop {
        let mut chunk = [0u8; 4096];
        let n = stream.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
    }
    let head = String::from_utf8_lossy(&buf).into_owned();
    let mut lines = head.split("\r\n");
    let request_line = lines.next().unwrap_or_default().to_string();
    let mut content_length = 0usize;
    let mut authorization = None;
    let mut accept = None;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        match name.to_ascii_lowercase().as_str() {
            "content-length" => content_length = value.parse().unwrap_or(0),
            "authorization" => authorization = Some(value.to_string()),
            "accept" => accept = Some(value.to_string()),
            _ => {}
        }
    }
    let header_end = buf
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|i| i + 4)
        .unwrap_or(buf.len());
    let mut body_bytes = buf[header_end..].to_vec();
    while body_bytes.len() < content_length {
        let mut chunk = [0u8; 4096];
        let n = stream.read(&mut chunk).await.ok()?;
        if n == 0 {
            break;
        }
        body_bytes.extend_from_slice(&chunk[..n]);
    }
    let body: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap_or(json!(null));

    Some(CapturedRequest {
        request_line,
        authorization,
        accept,
        body,
    })
}

/// Full mock stack: capture requests, respond per `response` (None =
/// stall after accepting).
fn spawn_responder(
    response: Option<&'static str>,
) -> (SocketAddr, tokio::sync::mpsc::Receiver<CapturedRequest>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    // from_std requires non-blocking mode on the underlying socket.
    listener.set_nonblocking(true).expect("nonblocking");
    let listener = TcpListener::from_std(listener).expect("convert");
    let (tx, rx) = tokio::sync::mpsc::channel(4);
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            let tx = tx.clone();
            let response = response;
            tokio::spawn(async move {
                let Some(captured) = read_request(&mut stream).await else {
                    return;
                };
                let _ = tx.send(captured).await;
                if let Some(response) = response {
                    let _ = stream.write_all(response.as_bytes()).await;
                    let _ = stream.flush().await;
                    let _ = stream.shutdown().await;
                } else {
                    // Stall: keep the connection open without data.
                    tokio::time::sleep(Duration::from_secs(60)).await;
                }
            });
        }
    });
    (addr, rx)
}

fn provider_for(addr: SocketAddr) -> ProfileProvider {
    let profile = ProviderProfile::new(
        "local",
        WireKind::OpenAiChatCompletions,
        format!("http://{addr}/v1"),
        AuthProfile::bearer("OPENAI_API_KEY"),
    )
    .expect("local profile");
    ProfileProvider::new(profile, "sk-test-key").expect("provider")
}

fn sample_request() -> Request {
    Request::new("gpt-4o-mini")
        .with_system_prompt("be terse")
        .with_message(mcode_core::Message::User(mcode_core::UserMessage::text(
            "hi",
        )))
}

const EMPTY_200_RESPONSE: &str = concat!(
    "HTTP/1.1 200 OK\r\n",
    "Content-Type: text/event-stream\r\n",
    "Connection: close\r\n",
    "\r\n",
);

const NON_SSE_JSON_200_RESPONSE: &str = concat!(
    "HTTP/1.1 200 OK\r\n",
    "Content-Type: application/json\r\n",
    "Connection: close\r\n",
    "\r\n",
    r#"{"id":"chatcmpl-1","choices":[{"message":{"role":"assistant","content":"hi"},"finish_reason":"stop"}]}"#,
);

const HAPPY_SSE_RESPONSE: &str = concat!(
    "HTTP/1.1 200 OK\r\n",
    "Content-Type: text/event-stream\r\n",
    "Connection: close\r\n",
    "\r\n",
    "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Hi\"},\"finish_reason\":null}]}\n\n",
    "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}],",
    "\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":1}}\n\n",
    "data: [DONE]\n\n"
);

async fn collect_events(
    provider: &ProfileProvider,
    req: &Request,
    cancel: CancellationToken,
) -> Vec<StreamEvent> {
    let mut stream = provider.stream(req, cancel).await.expect("stream starts");
    let mut events = Vec::new();
    while let Some(event) = stream.next().await {
        events.push(event);
    }
    events
}

#[tokio::test]
async fn streams_sse_over_http_and_sends_expected_request() {
    let (addr, mut captured_rx) = spawn_responder(Some(HAPPY_SSE_RESPONSE));
    let provider = provider_for(addr);

    let events = collect_events(&provider, &sample_request(), CancellationToken::new()).await;

    // Event sequence: Start, one text delta, Done with usage.
    assert_eq!(events.len(), 3);
    assert_eq!(events[0], StreamEvent::Start);
    assert_eq!(events[1], StreamEvent::TextDelta("Hi".into()));
    let StreamEvent::Done { message } = &events[2] else {
        panic!("expected Done, got {:?}", events[2]);
    };
    assert_eq!(message.blocks, vec![ContentBlock::Text("Hi".into())]);
    assert_eq!(
        message.usage,
        Some(Usage {
            input_tokens: 3,
            output_tokens: 1,
        })
    );
    assert_eq!(message.stop_reason, StopReason::Stop);

    // The captured HTTP request must carry the auth header and the
    // serialized body.
    let captured = captured_rx.recv().await.expect("request captured");
    assert!(
        captured
            .request_line
            .starts_with("POST /v1/chat/completions HTTP/1.1")
    );
    assert_eq!(captured.header("authorization"), Some("Bearer sk-test-key"));
    assert_eq!(captured.header("accept"), Some("text/event-stream"));
    assert_eq!(captured.body["model"], "gpt-4o-mini");
    assert_eq!(captured.body["stream"], true);
    assert_eq!(
        captured.body["messages"],
        json!([
            {"role": "system", "content": "be terse"},
            {"role": "user", "content": "hi"}
        ])
    );
}

#[tokio::test]
async fn empty_2xx_body_is_a_protocol_error() {
    let (addr, _rx) = spawn_responder(Some(EMPTY_200_RESPONSE));
    let provider = provider_for(addr);
    let events = collect_events(&provider, &sample_request(), CancellationToken::new()).await;
    match events.as_slice() {
        [
            StreamEvent::Start,
            StreamEvent::Error(LlmError::Sse(message)),
        ] => {
            assert!(
                message.contains("without an assistant choice"),
                "got: {message}"
            );
        }
        other => panic!("expected Start + Sse error, got {other:?}"),
    }
}

#[tokio::test]
async fn non_sse_2xx_body_is_a_protocol_error() {
    let (addr, _rx) = spawn_responder(Some(NON_SSE_JSON_200_RESPONSE));
    let provider = provider_for(addr);
    let events = collect_events(&provider, &sample_request(), CancellationToken::new()).await;
    match events.as_slice() {
        [
            StreamEvent::Start,
            StreamEvent::Error(LlmError::Sse(message)),
        ] => {
            assert!(
                message.contains("without an assistant choice"),
                "got: {message}"
            );
        }
        other => panic!("expected Start + Sse error, got {other:?}"),
    }
}

const USAGE_ONLY_SSE_RESPONSE: &str = concat!(
    "HTTP/1.1 200 OK\r\n",
    "Content-Type: text/event-stream\r\n",
    "Connection: close\r\n",
    "\r\n",
    "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":0}}\n\n",
    "data: [DONE]\n\n",
);

#[tokio::test]
async fn usage_only_sse_is_a_protocol_error() {
    let (addr, _rx) = spawn_responder(Some(USAGE_ONLY_SSE_RESPONSE));
    let provider = provider_for(addr);
    let events = collect_events(&provider, &sample_request(), CancellationToken::new()).await;
    match events.as_slice() {
        [
            StreamEvent::Start,
            StreamEvent::Error(LlmError::Sse(message)),
        ] => {
            assert!(
                message.contains("without an assistant choice"),
                "got: {message}"
            );
        }
        other => panic!("expected Start + Sse error, got {other:?}"),
    }
}

#[tokio::test]
async fn non_2xx_maps_to_http_error_with_body_excerpt() {
    let response = concat!(
        "HTTP/1.1 429 Too Many Requests\r\n",
        "Content-Type: application/json\r\n",
        "Connection: close\r\n",
        "\r\n",
        "{\"error\":{\"message\":\"rate limited, retry later\"}}"
    );
    // Leak a static string for the spawned responder.
    let response: &'static str = Box::leak(response.to_string().into_boxed_str());
    let (addr, _rx) = spawn_responder(Some(response));
    let provider = provider_for(addr);

    let events = collect_events(&provider, &sample_request(), CancellationToken::new()).await;
    assert_eq!(events.len(), 1);
    match &events[0] {
        StreamEvent::Error(LlmError::Http { status, body }) => {
            assert_eq!(*status, 429);
            assert!(body.contains("rate limited"), "got: {body}");
        }
        other => panic!("expected Http error, got {other:?}"),
    }
}

#[tokio::test]
async fn timeout_maps_to_timeout_error() {
    let (addr, _rx) = spawn_responder(None); // accepts, then stalls
    let provider = provider_for(addr).with_timeout(Duration::from_millis(150));

    let events = collect_events(&provider, &sample_request(), CancellationToken::new()).await;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0], StreamEvent::Error(LlmError::Timeout));
}

#[tokio::test]
async fn cancellation_while_awaiting_response_yields_cancelled() {
    let (addr, _rx) = spawn_responder(None); // accepts, then stalls

    let provider = provider_for(addr);
    let cancel = CancellationToken::new();
    let mut stream = provider
        .stream(&sample_request(), cancel.clone())
        .await
        .expect("stream starts");

    // Give the request time to be in flight, then cancel.
    tokio::time::sleep(Duration::from_millis(50)).await;
    cancel.cancel();

    let mut events = Vec::new();
    while let Some(event) = stream.next().await {
        events.push(event);
    }
    assert_eq!(events.len(), 1);
    assert_eq!(events[0], StreamEvent::Error(LlmError::Cancelled));
}

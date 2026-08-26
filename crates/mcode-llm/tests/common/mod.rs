//! Local raw-TCP HTTP test support shared across protocol and catalog tests.

use std::collections::{BTreeMap, VecDeque};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Notify;

/// One request captured by the local server.
#[derive(Debug)]
pub struct CapturedRequest {
    /// HTTP method.
    pub method: String,
    /// Request target path and query.
    pub path: String,
    /// Lowercase headers.
    pub headers: BTreeMap<String, String>,
    /// Raw request body.
    pub body: Vec<u8>,
}

impl CapturedRequest {
    /// Returns one case-insensitive header value.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .get(&name.to_ascii_lowercase())
            .map(String::as_str)
    }

    /// Parses the body as JSON.
    pub fn json(&self) -> Value {
        serde_json::from_slice(&self.body).expect("captured request body must be JSON")
    }
}

/// Owned response plan for one accepted connection.
#[derive(Debug, Clone)]
pub struct MockResponse {
    head: Vec<u8>,
    chunks: Vec<Vec<u8>>,
    chunk_delay: Duration,
    stall: bool,
}

impl MockResponse {
    /// Creates a response whose body is written in caller-provided chunks.
    pub fn chunks(
        status: &str,
        content_type: &str,
        headers: &[(&str, &str)],
        chunks: Vec<Vec<u8>>,
        chunk_delay: Duration,
    ) -> Self {
        let mut head =
            format!("HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nConnection: close\r\n");
        for (name, value) in headers {
            head.push_str(name);
            head.push_str(": ");
            head.push_str(value);
            head.push_str("\r\n");
        }
        head.push_str("\r\n");
        Self {
            head: head.into_bytes(),
            chunks,
            chunk_delay,
            stall: false,
        }
    }

    /// Creates one JSON response.
    pub fn json(status: &str, headers: &[(&str, &str)], body: Value) -> Self {
        Self::chunks(
            status,
            "application/json",
            headers,
            vec![body.to_string().into_bytes()],
            Duration::ZERO,
        )
    }

    /// Creates one SSE response split at exact byte chunks.
    pub fn sse(chunks: Vec<Vec<u8>>) -> Self {
        Self::chunks(
            "200 OK",
            "text/event-stream",
            &[],
            chunks,
            Duration::from_millis(1),
        )
    }

    /// Creates a response that accepts the request and then stalls.
    pub fn stall() -> Self {
        Self {
            head: Vec::new(),
            chunks: Vec::new(),
            chunk_delay: Duration::ZERO,
            stall: true,
        }
    }
}

/// Running local server and captured-request channel.
pub struct MockServer {
    addr: SocketAddr,
    requests: tokio::sync::mpsc::Receiver<CapturedRequest>,
    response_heads: tokio::sync::mpsc::Receiver<()>,
    completed: Arc<AtomicUsize>,
    completion_changed: Arc<Notify>,
}

impl MockServer {
    /// Spawns a server consuming one response plan per connection.
    pub fn spawn(responses: Vec<MockResponse>) -> Self {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind mock server");
        let addr = listener.local_addr().expect("mock server address");
        listener
            .set_nonblocking(true)
            .expect("set mock server nonblocking");
        let listener = TcpListener::from_std(listener).expect("convert mock listener");
        let responses = Arc::new(Mutex::new(VecDeque::from(responses)));
        let completed = Arc::new(AtomicUsize::new(0));
        let completion_changed = Arc::new(Notify::new());
        let (sender, requests) = tokio::sync::mpsc::channel(16);
        let (response_head_sender, response_heads) = tokio::sync::mpsc::channel(16);
        let completed_for_task = Arc::clone(&completed);
        let completion_changed_for_task = Arc::clone(&completion_changed);
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                let response = responses.lock().expect("response plans lock").pop_front();
                let sender = sender.clone();
                let response_head_sender = response_head_sender.clone();
                let completed = Arc::clone(&completed_for_task);
                let completion_changed = Arc::clone(&completion_changed_for_task);
                tokio::spawn(async move {
                    serve_connection(stream, response, sender, response_head_sender).await;
                    completed.fetch_add(1, Ordering::SeqCst);
                    completion_changed.notify_one();
                });
            }
        });
        Self {
            addr,
            requests,
            response_heads,
            completed,
            completion_changed,
        }
    }

    /// Returns `http://127.0.0.1:<port>`.
    pub fn base_url(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// Waits until `expected` connection handlers have finished.
    pub async fn wait_for_completed_connections(&self, expected: usize) {
        loop {
            let changed = self.completion_changed.notified();
            if self.completed.load(Ordering::SeqCst) >= expected {
                return;
            }
            changed.await;
        }
    }

    /// Waits until the next response head was written and flushed.
    pub async fn wait_for_response_head(&mut self) {
        self.response_heads
            .recv()
            .await
            .expect("mock response-head channel closed");
    }

    /// Waits for the next captured request.
    pub async fn request(&mut self) -> CapturedRequest {
        tokio::time::timeout(Duration::from_secs(2), self.requests.recv())
            .await
            .expect("request capture timed out")
            .expect("mock request channel closed")
    }
}

async fn serve_connection(
    mut stream: TcpStream,
    response: Option<MockResponse>,
    sender: tokio::sync::mpsc::Sender<CapturedRequest>,
    response_head_sender: tokio::sync::mpsc::Sender<()>,
) {
    let Some(request) = read_request(&mut stream).await else {
        return;
    };
    let _ = sender.send(request).await;
    let Some(response) = response else {
        return;
    };
    if response.stall {
        // No response is ever written; wait, but wake up as soon as the
        // client disconnects.
        client_gone_or_sleep(&mut stream, Duration::from_secs(60)).await;
        return;
    }
    if stream.write_all(&response.head).await.is_err() || stream.flush().await.is_err() {
        return;
    }
    let _ = response_head_sender.send(()).await;
    for chunk in response.chunks {
        if response.chunk_delay > Duration::ZERO
            && client_gone_or_sleep(&mut stream, response.chunk_delay).await
        {
            return;
        }
        if stream.write_all(&chunk).await.is_err() {
            return;
        }
        let _ = stream.flush().await;
    }
    let _ = stream.shutdown().await;
}

/// Sleeps for `delay`, returning early with `true` when the client half
/// closes the connection (read EOF) so tests can observe abandoned
/// transports without waiting out the full delay.
async fn client_gone_or_sleep(stream: &mut TcpStream, delay: Duration) -> bool {
    let mut byte = [0_u8; 1];
    tokio::select! {
        _ = tokio::time::sleep(delay) => false,
        read = stream.read(&mut byte) => match read {
            Ok(0) | Err(_) => true,
            Ok(_) => false,
        },
    }
}

async fn read_request(stream: &mut TcpStream) -> Option<CapturedRequest> {
    let mut bytes = Vec::new();
    loop {
        let mut chunk = [0_u8; 4_096];
        let count = stream.read(&mut chunk).await.ok()?;
        if count == 0 {
            return None;
        }
        bytes.extend_from_slice(&chunk[..count]);
        if bytes.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
    }
    let header_end = bytes.windows(4).position(|window| window == b"\r\n\r\n")? + 4;
    let head = String::from_utf8_lossy(&bytes[..header_end]);
    let mut lines = head.split("\r\n");
    let mut request_line = lines.next()?.split_whitespace();
    let method = request_line.next()?.to_owned();
    let path = request_line.next()?.to_owned();
    let mut headers = BTreeMap::new();
    let mut content_length = 0;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let name = name.to_ascii_lowercase();
        let value = value.trim().to_owned();
        if name == "content-length" {
            content_length = value.parse().unwrap_or(0);
        }
        headers.insert(name, value);
    }
    let mut body = bytes[header_end..].to_vec();
    while body.len() < content_length {
        let mut chunk = [0_u8; 4_096];
        let count = stream.read(&mut chunk).await.ok()?;
        if count == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..count]);
    }
    body.truncate(content_length);
    Some(CapturedRequest {
        method,
        path,
        headers,
        body,
    })
}

// Rust guideline compliant 2026-08-26

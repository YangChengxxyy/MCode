//! Offline Streamable HTTP, redirect, and DNS-rebinding coverage.

// Rust guideline compliant 2026-08-20.

use std::{
    collections::HashMap,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use async_trait::async_trait;
use axum::{
    Router,
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode, header::LOCATION},
    response::IntoResponse,
    routing::post,
};
use futures::StreamExt;
use http::{HeaderName, HeaderValue};
use mcode_mcp::{
    DnsResolver, DnsResolverHandle, Error, ErrorKind, HttpSecurityPolicy, OutputLimits, Recovery,
    SecureHttpClient, ServerName, TimeoutConfig, TrustConfig, TrustLevel,
};
use rmcp::{
    model::{ClientJsonRpcMessage, ClientRequest, PingRequest, RequestId},
    transport::streamable_http_client::{
        StreamableHttpClient, StreamableHttpError, StreamableHttpPostResponse,
    },
};
use tokio::sync::Mutex;

fn trusted_policy(server: &str) -> HttpSecurityPolicy {
    HttpSecurityPolicy::new(
        ServerName::new(server).unwrap(),
        TrustConfig {
            level: TrustLevel::Trusted,
            allow_http: true,
            allow_localhost: true,
            ..TrustConfig::default()
        },
        OutputLimits::default(),
        TimeoutConfig::default(),
    )
}

fn ping() -> ClientJsonRpcMessage {
    ClientJsonRpcMessage::request(
        ClientRequest::PingRequest(PingRequest::default()),
        RequestId::Number(1),
    )
}

#[derive(Debug, Clone)]
struct FixedResolver(SocketAddr);

#[async_trait]
impl DnsResolver for FixedResolver {
    async fn resolve(&self, _host: &str, _port: u16) -> mcode_mcp::Result<Vec<SocketAddr>> {
        Ok(vec![self.0])
    }
}

#[derive(Debug, Clone)]
struct RebindingResolver {
    calls: Arc<AtomicUsize>,
    first: SocketAddr,
}

#[async_trait]
impl DnsResolver for RebindingResolver {
    async fn resolve(&self, _host: &str, port: u16) -> mcode_mcp::Result<Vec<SocketAddr>> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        if call == 0 {
            Ok(vec![self.first])
        } else {
            Ok(vec![SocketAddr::new(
                IpAddr::V4(Ipv4Addr::new(10, 0, 0, 8)),
                port,
            )])
        }
    }
}

#[derive(Debug, Clone, Default)]
struct ResumeCapture {
    session: Arc<Mutex<Option<String>>>,
    last_event: Arc<Mutex<Option<String>>>,
}

async fn initial_sse() -> impl IntoResponse {
    (
        StatusCode::OK,
        [
            ("content-type", "text/event-stream"),
            ("mcp-session-id", "session-1"),
        ],
        "id: event-1\ndata: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}\n\n",
    )
}

async fn resumed_sse(
    State(capture): State<ResumeCapture>,
    headers: HeaderMap,
) -> impl IntoResponse {
    *capture.session.lock().await = headers
        .get("mcp-session-id")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    *capture.last_event.lock().await = headers
        .get("last-event-id")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    (
        StatusCode::OK,
        [("content-type", "text/event-stream")],
        "id: event-2\ndata: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/tools/list_changed\"}\n\n",
    )
}

#[tokio::test]
async fn local_streamable_http_carries_session_and_last_event_id() {
    let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let address = listener.local_addr().unwrap();
    let capture = ResumeCapture::default();
    let app = Router::new()
        .route("/mcp", post(initial_sse).get(resumed_sse))
        .with_state(capture.clone());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let client = SecureHttpClient::new(
        trusted_policy("local-http"),
        DnsResolverHandle::new(FixedResolver(address)),
    );
    let uri: Arc<str> = format!("http://local.test:{}/mcp", address.port()).into();
    let response = client
        .post_message(uri.clone(), ping(), None, None, HashMap::new())
        .await
        .unwrap();
    let StreamableHttpPostResponse::Sse(mut stream, session) = response else {
        panic!("expected SSE response");
    };
    let first = stream.next().await.unwrap().unwrap();
    assert_eq!(first.id.as_deref(), Some("event-1"));
    assert_eq!(session.as_deref(), Some("session-1"));

    let mut resumed = client
        .get_stream(
            uri,
            Some(Arc::from("session-1")),
            Some("event-1".into()),
            None,
            HashMap::new(),
        )
        .await
        .unwrap();
    assert_eq!(
        resumed.next().await.unwrap().unwrap().id.as_deref(),
        Some("event-2")
    );
    assert_eq!(capture.session.lock().await.as_deref(), Some("session-1"));
    assert_eq!(capture.last_event.lock().await.as_deref(), Some("event-1"));

    server.abort();
}

#[derive(Debug, Clone)]
struct RedirectState {
    target: String,
    original_header: Arc<Mutex<Option<String>>>,
}

async fn redirect(State(state): State<RedirectState>, headers: HeaderMap) -> impl IntoResponse {
    *state.original_header.lock().await = headers
        .get("x-api-key")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    (
        StatusCode::TEMPORARY_REDIRECT,
        [(LOCATION, state.target)],
        "",
    )
}

#[derive(Debug, Clone, Default)]
struct RedirectTargetCapture {
    calls: Arc<AtomicUsize>,
    header: Arc<Mutex<Option<String>>>,
    body: Arc<Mutex<Option<Vec<u8>>>>,
}

async fn redirected(
    State(capture): State<RedirectTargetCapture>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    capture.calls.fetch_add(1, Ordering::SeqCst);
    *capture.header.lock().await = headers
        .get("x-api-key")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    *capture.body.lock().await = Some(body.to_vec());
    (
        StatusCode::OK,
        [("content-type", "application/json")],
        "{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}",
    )
}

#[tokio::test]
async fn cross_origin_temporary_redirect_does_not_forward_a_post() {
    let target_listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let target_address = target_listener.local_addr().unwrap();
    let target_capture = RedirectTargetCapture::default();
    let target_app = Router::new()
        .route("/target", post(redirected))
        .with_state(target_capture.clone());
    let target_server =
        tokio::spawn(async move { axum::serve(target_listener, target_app).await.unwrap() });

    let source_listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let source_address = source_listener.local_addr().unwrap();
    let original_header = Arc::new(Mutex::new(None));
    let source_app = Router::new()
        .route("/mcp", post(redirect))
        .with_state(RedirectState {
            target: format!("http://redirect.test:{}/target", target_address.port()),
            original_header: original_header.clone(),
        });
    let source_server =
        tokio::spawn(async move { axum::serve(source_listener, source_app).await.unwrap() });

    #[derive(Debug, Clone)]
    struct PortResolver {
        source: SocketAddr,
        target: SocketAddr,
    }
    #[async_trait]
    impl DnsResolver for PortResolver {
        async fn resolve(&self, _host: &str, port: u16) -> mcode_mcp::Result<Vec<SocketAddr>> {
            Ok(vec![if port == self.source.port() {
                self.source
            } else {
                self.target
            }])
        }
    }

    let client = SecureHttpClient::new(
        trusted_policy("redirect"),
        DnsResolverHandle::new(PortResolver {
            source: source_address,
            target: target_address,
        }),
    );
    let mut headers = HashMap::new();
    headers.insert(
        HeaderName::from_static("x-api-key"),
        HeaderValue::from_static("opaque-marker"),
    );
    let error = client
        .post_message(
            format!("http://source.test:{}/mcp", source_address.port()).into(),
            ping(),
            None,
            None,
            headers,
        )
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        StreamableHttpError::Client(mcode_mcp::http::SecureHttpError::Redirect)
    ));
    assert_eq!(
        original_header.lock().await.as_deref(),
        Some("opaque-marker")
    );
    assert_eq!(target_capture.calls.load(Ordering::SeqCst), 0);
    assert!(target_capture.header.lock().await.is_none());
    assert!(target_capture.body.lock().await.is_none());

    source_server.abort();
    target_server.abort();
}

async fn json_ping() -> impl IntoResponse {
    (
        StatusCode::OK,
        [("content-type", "application/json")],
        "{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}",
    )
}

#[tokio::test]
async fn dns_is_revalidated_and_pinned_on_every_request() {
    let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let address = listener.local_addr().unwrap();
    let app = Router::new().route("/mcp", post(json_ping));
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let calls = Arc::new(AtomicUsize::new(0));
    let client = SecureHttpClient::new(
        trusted_policy("rebind"),
        DnsResolverHandle::new(RebindingResolver {
            calls: calls.clone(),
            first: address,
        }),
    );
    let uri: Arc<str> = format!("http://rebind.test:{}/mcp", address.port()).into();
    client
        .post_message(uri.clone(), ping(), None, None, HashMap::new())
        .await
        .unwrap();
    let error = client
        .post_message(uri, ping(), None, None, HashMap::new())
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        StreamableHttpError::Client(mcode_mcp::http::SecureHttpError::Policy(_))
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 2);

    server.abort();
}

#[test]
fn metadata_address_remains_blocked_even_when_localhost_is_trusted() {
    let policy = trusted_policy("metadata");
    let error = policy
        .validate_hop(
            &"http://metadata.google.internal/mcp".parse().unwrap(),
            &[SocketAddr::new(
                IpAddr::V4(Ipv4Addr::new(169, 254, 169, 254)),
                80,
            )],
        )
        .unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Trust);
    let _ = Error::new(ErrorKind::Trust, Recovery::Fatal, "checked");
}

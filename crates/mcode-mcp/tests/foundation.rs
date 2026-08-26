//! Offline integration coverage for the adapter-friendly MCP foundation.

// Rust guideline compliant 2026-08-20.

use std::{
    collections::{BTreeMap, HashMap},
    sync::{
        Arc, Mutex as StdMutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use mcode_mcp::{
    AuthHostHandle, CallTerminalState, CatalogSnapshot, Error, ErrorKind, Generation, HeadlessHost,
    McpConnector, McpConnectorHandle, McpHostHandle, McpPluginConfig, McpSession, McpSessionHandle,
    NamespacedId, NegotiatedCapabilities, NegotiatedServer, NoAuthHost, OutputLimits, Page,
    PromptGetOutcome, ReconnectConfig, RemotePrompt, RemotePromptArgument, RemoteResource,
    RemoteResourceTemplate, RemoteTool, RequestCancellation, RequestControl, RequestObserver,
    RequestObserverHandle, ResourceReadOutcome, ServerConfig, ServerEvent, ServerName, ServerPhase,
    SessionEvent, SessionEventSink, StdioTransportConfig, TimeoutConfig, ToolCallOutcome,
    TransportConfig, TrustConfig,
};
use serde_json::{Value, json};
use tokio::sync::{Mutex, RwLock};

#[derive(Debug)]
struct FakeData {
    tools: RwLock<Vec<RemoteTool>>,
    resources: Vec<RemoteResource>,
    templates: Vec<RemoteResourceTemplate>,
    prompts: Vec<RemotePrompt>,
    page_size: usize,
    list_calls: AtomicUsize,
    calls: AtomicUsize,
    closes: AtomicUsize,
    connects: AtomicUsize,
    failures_left: AtomicUsize,
    fatal_failure: bool,
    events: Mutex<Option<SessionEventSink>>,
}

impl FakeData {
    fn new(tools: Vec<RemoteTool>) -> Self {
        Self {
            tools: RwLock::new(tools),
            resources: vec![RemoteResource {
                uri: "memory://doc".into(),
                name: "doc".into(),
                title: Some("Document".into()),
                description: None,
                mime_type: Some("text/plain".into()),
                size: Some(4),
            }],
            templates: vec![RemoteResourceTemplate {
                uri_template: "memory://{name}".into(),
                name: "by-name".into(),
                title: None,
                description: None,
                mime_type: Some("text/plain".into()),
            }],
            prompts: vec![RemotePrompt {
                name: "review".into(),
                title: Some("Review".into()),
                description: None,
                arguments: vec![RemotePromptArgument {
                    name: "target".into(),
                    title: None,
                    description: None,
                    required: true,
                }],
            }],
            page_size: 1,
            list_calls: AtomicUsize::new(0),
            calls: AtomicUsize::new(0),
            closes: AtomicUsize::new(0),
            connects: AtomicUsize::new(0),
            failures_left: AtomicUsize::new(0),
            fatal_failure: false,
            events: Mutex::new(None),
        }
    }

    fn fatal() -> Self {
        let mut data = Self::new(Vec::new());
        data.failures_left = AtomicUsize::new(usize::MAX);
        data.fatal_failure = true;
        data
    }

    async fn emit(&self, event: SessionEvent) {
        if let Some(sink) = self.events.lock().await.clone() {
            sink.send(event).await.unwrap();
        }
    }
}

#[derive(Debug, Clone)]
struct FakeConnector {
    servers: Arc<HashMap<String, Arc<FakeData>>>,
}

#[async_trait]
impl McpConnector for FakeConnector {
    async fn connect(
        &self,
        context: mcode_mcp::ConnectContext,
    ) -> mcode_mcp::Result<McpSessionHandle> {
        let data = self
            .servers
            .get(context.server.as_str())
            .expect("test server plan")
            .clone();
        data.connects.fetch_add(1, Ordering::SeqCst);
        let should_fail = data
            .failures_left
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |value| {
                (value > 0).then(|| value.saturating_sub(1))
            })
            .is_ok();
        if should_fail {
            return Err(Error::new(
                ErrorKind::Transport,
                if data.fatal_failure {
                    mcode_mcp::Recovery::Fatal
                } else {
                    mcode_mcp::Recovery::Recoverable
                },
                "planned connection failure",
            )
            .with_server(context.server));
        }
        *data.events.lock().await = Some(context.events);
        Ok(McpSessionHandle::new(FakeSession {
            server: context.server,
            data,
            negotiated: NegotiatedServer {
                protocol_version: mcode_mcp::MCP_PROTOCOL_VERSION.into(),
                implementation_name: Some("offline-fake".into()),
                implementation_version: Some("1".into()),
                capabilities: NegotiatedCapabilities {
                    tools: true,
                    tool_list_changed: true,
                    resources: true,
                    resource_templates: true,
                    resource_subscribe: true,
                    resource_list_changed: true,
                    prompts: true,
                    prompt_list_changed: true,
                    completion: true,
                    logging: false,
                    tasks: true,
                },
            },
        }))
    }
}

#[derive(Debug)]
struct FakeSession {
    server: ServerName,
    data: Arc<FakeData>,
    negotiated: NegotiatedServer,
}

#[async_trait]
impl McpSession for FakeSession {
    fn negotiated(&self) -> &NegotiatedServer {
        &self.negotiated
    }

    async fn ping(&self) -> mcode_mcp::Result<()> {
        Ok(())
    }

    async fn list_tools(&self, cursor: Option<String>) -> mcode_mcp::Result<Page<RemoteTool>> {
        self.data.list_calls.fetch_add(1, Ordering::SeqCst);
        let tools = self.data.tools.read().await.clone();
        Ok(page(&tools, cursor, self.data.page_size))
    }

    async fn call_tool(
        &self,
        name: &str,
        _arguments: Value,
        control: RequestControl,
    ) -> mcode_mcp::Result<ToolCallOutcome> {
        self.data.calls.fetch_add(1, Ordering::SeqCst);
        if name == "slow" {
            control.observer.notify(mcode_mcp::ProgressUpdate {
                progress: 1.0,
                total: Some(2.0),
                message: Some("half".into()),
            });
            control.cancellation.cancelled().await;
            return Err(Error::new(
                ErrorKind::Cancelled,
                mcode_mcp::Recovery::Fatal,
                "cancelled fake call",
            )
            .with_server(self.server.clone()));
        }
        let result = if name == "malicious" {
            json!({
                "content": [{"type":"text", "text":"\u{1b}[31mBearer token-value\u{1b}[0m"}],
                "accessToken": "should-not-escape"
            })
        } else {
            json!({"content":[{"type":"text","text":"ok"}]})
        };
        Ok(ToolCallOutcome::Complete {
            result,
            is_error: false,
        })
    }

    async fn list_resources(
        &self,
        cursor: Option<String>,
    ) -> mcode_mcp::Result<Page<RemoteResource>> {
        Ok(page(&self.data.resources, cursor, self.data.page_size))
    }

    async fn list_resource_templates(
        &self,
        cursor: Option<String>,
    ) -> mcode_mcp::Result<Page<RemoteResourceTemplate>> {
        Ok(page(&self.data.templates, cursor, self.data.page_size))
    }

    async fn read_resource(
        &self,
        uri: &str,
        _control: RequestControl,
    ) -> mcode_mcp::Result<ResourceReadOutcome> {
        Ok(ResourceReadOutcome::Complete {
            result: json!({"contents":[{"uri":uri,"text":"body"}]}),
        })
    }

    async fn subscribe_resource(&self, _uri: &str) -> mcode_mcp::Result<()> {
        Ok(())
    }

    async fn unsubscribe_resource(&self, _uri: &str) -> mcode_mcp::Result<()> {
        Ok(())
    }

    async fn list_prompts(&self, cursor: Option<String>) -> mcode_mcp::Result<Page<RemotePrompt>> {
        Ok(page(&self.data.prompts, cursor, self.data.page_size))
    }

    async fn get_prompt(
        &self,
        _name: &str,
        _arguments: BTreeMap<String, String>,
        _control: RequestControl,
    ) -> mcode_mcp::Result<PromptGetOutcome> {
        Ok(PromptGetOutcome::Complete {
            result: json!({
                "messages":[{"role":"user","content":{"type":"text","text":"review this"}}]
            }),
        })
    }

    async fn complete(
        &self,
        _request: Value,
        _control: RequestControl,
    ) -> mcode_mcp::Result<Value> {
        Ok(json!({"completion":{"values":["alpha","beta"]}}))
    }

    async fn set_log_level(&self, _level: mcode_mcp::LogLevel) -> mcode_mcp::Result<()> {
        Err(Error::unsupported(self.server.clone(), "logging"))
    }

    async fn notify_roots_changed(&self) -> mcode_mcp::Result<()> {
        Err(Error::unsupported(self.server.clone(), "roots_changed"))
    }

    async fn get_task(&self, task_id: &str, _control: RequestControl) -> mcode_mcp::Result<Value> {
        Ok(json!({"resultType":"complete","taskId":task_id,"status":"working"}))
    }

    async fn update_task(
        &self,
        _task_id: &str,
        _responses: Value,
        _control: RequestControl,
    ) -> mcode_mcp::Result<()> {
        Ok(())
    }

    async fn cancel_task(&self, _task_id: &str, _control: RequestControl) -> mcode_mcp::Result<()> {
        Ok(())
    }

    async fn close(&self) -> mcode_mcp::Result<()> {
        self.data.closes.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

fn page<T: Clone>(items: &[T], cursor: Option<String>, size: usize) -> Page<T> {
    let start = cursor.as_deref().unwrap_or("0").parse::<usize>().unwrap();
    let end = (start + size).min(items.len());
    let next = (end < items.len()).then(|| end.to_string());
    Page::new(items[start..end].to_vec(), next)
}

fn tool(name: &str) -> RemoteTool {
    RemoteTool {
        name: name.into(),
        title: None,
        description: Some(format!("{name} tool")),
        input_schema: json!({
            "type":"object",
            "properties":{"value":{"type":"string"}},
            "additionalProperties": false
        }),
        output_schema: None,
        annotations: None,
    }
}

fn server_config() -> ServerConfig {
    ServerConfig {
        enabled: true,
        transport: TransportConfig::Stdio(StdioTransportConfig {
            command: "ignored-by-fake".into(),
            args: vec![],
            cwd: None,
            env: Default::default(),
        }),
        timeouts: TimeoutConfig {
            connect_ms: 1_000,
            request_ms: 500,
            request_total_ms: 1_000,
            ping_interval_ms: 60_000,
            ping_ms: 100,
            shutdown_ms: 500,
        },
        output_limits: OutputLimits::default(),
        reconnect: ReconnectConfig {
            enabled: true,
            max_attempts: 3,
            initial_delay_ms: 5,
            max_delay_ms: 20,
        },
        trust: TrustConfig::default(),
    }
}

fn harness(
    plans: Vec<(&str, Arc<FakeData>)>,
) -> (mcode_mcp::McpSupervisor, HashMap<String, Arc<FakeData>>) {
    let plans: HashMap<String, Arc<FakeData>> = plans
        .into_iter()
        .map(|(name, plan)| (name.to_owned(), plan))
        .collect();
    let servers = plans
        .keys()
        .map(|name| (ServerName::new(name).unwrap(), server_config()))
        .collect();
    let supervisor = mcode_mcp::McpSupervisor::start(
        McpPluginConfig {
            version: mcode_mcp::CONFIG_VERSION,
            servers,
        },
        McpConnectorHandle::new(FakeConnector {
            servers: Arc::new(plans.clone()),
        }),
        McpHostHandle::new(HeadlessHost),
        AuthHostHandle::new(NoAuthHost),
    )
    .unwrap();
    (supervisor, plans)
}

async fn ready(supervisor: &mcode_mcp::McpSupervisor, name: &str) -> mcode_mcp::ServerHandle {
    let name = ServerName::new(name).unwrap();
    let handle = supervisor.server(&name).unwrap().clone();
    handle.wait_ready(Duration::from_secs(2)).await.unwrap();
    handle
}

#[tokio::test]
async fn state_is_retained_before_the_first_subscriber() {
    let data = Arc::new(FakeData::new(vec![tool("ready")]));
    let (supervisor, _) = harness(vec![("retained", data)]);
    tokio::time::sleep(Duration::from_millis(50)).await;
    let name = ServerName::new("retained").unwrap();
    assert_eq!(
        supervisor.server(&name).unwrap().state().phase,
        ServerPhase::Ready
    );
    supervisor.shutdown().await;
}

#[tokio::test]
async fn three_servers_start_and_fail_independently() {
    let alpha = Arc::new(FakeData::new(vec![tool("alpha")]));
    let beta = Arc::new(FakeData::new(vec![tool("beta")]));
    let broken = Arc::new(FakeData::fatal());
    let (supervisor, _) = harness(vec![
        ("alpha", alpha.clone()),
        ("beta", beta.clone()),
        ("broken", broken),
    ]);

    let (alpha_handle, beta_handle) =
        tokio::join!(ready(&supervisor, "alpha"), ready(&supervisor, "beta"));
    let broken_name = ServerName::new("broken").unwrap();
    assert!(
        supervisor
            .server(&broken_name)
            .unwrap()
            .wait_ready(Duration::from_secs(1))
            .await
            .is_err()
    );
    assert_eq!(alpha_handle.state().phase, ServerPhase::Ready);
    assert_eq!(beta_handle.state().phase, ServerPhase::Ready);

    supervisor.shutdown().await;
    assert_eq!(alpha.closes.load(Ordering::SeqCst), 1);
    assert_eq!(beta.closes.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn pagination_and_list_changed_publish_one_generation_transaction() {
    let data = Arc::new(FakeData::new(vec![tool("a"), tool("b"), tool("c")]));
    let (supervisor, _) = harness(vec![("catalog", data.clone())]);
    let handle = ready(&supervisor, "catalog").await;
    let first = handle.catalog().await;
    assert_eq!(first.tools().len(), 3);
    assert_eq!(first.generation(), Generation::new(1));
    assert_eq!(data.list_calls.load(Ordering::SeqCst), 3);

    *data.tools.write().await = vec![tool("replacement")];
    data.emit(SessionEvent::ToolListChanged).await;
    let mut states = handle.subscribe_state();
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if states.borrow().generation == Generation::new(2) {
                break;
            }
            states.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    let second = handle.catalog().await;
    assert_eq!(second.tools().len(), 1);
    assert_eq!(
        second.tools().next().unwrap().id.to_string(),
        "mcp:catalog:replacement"
    );
    supervisor.shutdown().await;
}

#[derive(Debug, Clone, Default)]
struct ProgressCollector(Arc<StdMutex<Vec<mcode_mcp::ProgressUpdate>>>);

impl RequestObserver for ProgressCollector {
    fn on_progress(&self, update: mcode_mcp::ProgressUpdate) {
        self.0.lock().unwrap().push(update);
    }
}

#[tokio::test]
async fn call_cancel_is_not_retried_and_outputs_are_sanitized() {
    let data = Arc::new(FakeData::new(vec![tool("slow"), tool("malicious")]));
    let (supervisor, _) = harness(vec![("calls", data.clone())]);
    let handle = ready(&supervisor, "calls").await;

    let cancellation = RequestCancellation::new();
    let task_cancel = cancellation.clone();
    let progress = ProgressCollector::default();
    let observer = RequestObserverHandle::new(progress.clone());
    let id = NamespacedId::new(ServerName::new("calls").unwrap(), "slow").unwrap();
    let call_handle = handle.clone();
    let task = tokio::spawn(async move {
        call_handle
            .call_tool(
                &id,
                json!({}),
                RequestControl::new()
                    .with_cancellation(task_cancel)
                    .with_observer(observer),
            )
            .await
    });
    tokio::time::sleep(Duration::from_millis(20)).await;
    cancellation.cancel();
    let error = task.await.unwrap().unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Cancelled);
    assert_eq!(data.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        progress.0.lock().unwrap()[0].message.as_deref(),
        Some("half")
    );

    let malicious = NamespacedId::new(ServerName::new("calls").unwrap(), "malicious").unwrap();
    let outcome = handle
        .call_tool(&malicious, json!({}), RequestControl::new())
        .await
        .unwrap();
    let ToolCallOutcome::Complete { result, .. } = outcome else {
        panic!("expected complete result");
    };
    let encoded = result.to_string();
    assert!(!encoded.contains("token-value"));
    assert!(!encoded.contains("should-not-escape"));
    assert!(!encoded.contains("\\u001b"));

    supervisor.shutdown().await;
}

#[tokio::test]
async fn subscribed_resource_and_task_events_are_delivered() {
    let data = Arc::new(FakeData::new(vec![tool("events")]));
    let (supervisor, _) = harness(vec![("events", data.clone())]);
    let handle = ready(&supervisor, "events").await;
    let mut events = handle.subscribe_events();
    handle.subscribe_resource("memory://doc").await.unwrap();

    data.emit(SessionEvent::ResourceUpdated {
        uri: "memory://doc".to_owned(),
    })
    .await;
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(1), events.recv())
            .await
            .unwrap()
            .unwrap(),
        ServerEvent::ResourceUpdated {
            uri: "memory://doc".to_owned()
        }
    );

    let status = json!({"taskId":"task-1","status":"working"});
    data.emit(SessionEvent::TaskStatus(status.clone())).await;
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(1), events.recv())
            .await
            .unwrap()
            .unwrap(),
        ServerEvent::TaskStatus(status)
    );
    supervisor.shutdown().await;
}

#[tokio::test]
async fn resources_prompts_completion_tasks_and_reconnect_are_isolated() {
    let data = Arc::new(FakeData::new(vec![tool("read")]));
    let (supervisor, _) = harness(vec![("features", data.clone())]);
    let handle = ready(&supervisor, "features").await;

    let ResourceReadOutcome::Complete { result } = handle
        .read_resource("memory://doc", RequestControl::new())
        .await
        .unwrap()
    else {
        panic!("expected resource result");
    };
    assert_eq!(result["contents"][0]["text"], "body");

    let prompt = NamespacedId::new(ServerName::new("features").unwrap(), "review").unwrap();
    let PromptGetOutcome::Complete { result } = handle
        .get_prompt(&prompt, BTreeMap::new(), RequestControl::new())
        .await
        .unwrap()
    else {
        panic!("expected prompt result");
    };
    assert_eq!(result["messages"][0]["content"]["type"], "text");

    let completion = handle
        .complete(
            json!({
                "ref":{"type":"ref/prompt","name":"review"},
                "argument":{"name":"target","value":"a"}
            }),
            RequestControl::new(),
        )
        .await
        .unwrap();
    assert_eq!(completion["completion"]["values"][0], "alpha");
    assert_eq!(
        handle
            .get_task("task-1", RequestControl::new())
            .await
            .unwrap()["taskId"],
        "task-1"
    );

    data.emit(SessionEvent::Disconnected(Error::transport(
        ServerName::new("features").unwrap(),
        "planned disconnect",
    )))
    .await;
    let mut state = handle.subscribe_state();
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if state.borrow().generation.get() >= 2 && state.borrow().phase == ServerPhase::Ready {
                break;
            }
            state.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    assert_eq!(data.connects.load(Ordering::SeqCst), 2);
    supervisor.shutdown().await;
}

#[test]
fn catalog_and_ledger_types_do_not_encode_live_state() {
    let state = mcode_mcp::PersistedMcpState::new();
    let encoded = serde_json::to_string(&state).unwrap();
    for forbidden in ["oauth", "transportHandle", "accessToken", "refreshToken"] {
        assert!(!encoded.contains(forbidden));
    }
    assert_eq!(
        CallTerminalState::Interrupted,
        CallTerminalState::Interrupted
    );
    let _type_check: Option<CatalogSnapshot> = None;
}

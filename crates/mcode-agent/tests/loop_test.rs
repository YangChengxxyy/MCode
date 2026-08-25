//! Agent double-loop integration tests — the M1 T4 matrix from
//! `07-m1-plan.md`, driven by the scripted `FakeProvider` (zero
//! network):
//!
//! 1. Single-turn text reply stops.
//! 2. Multi-turn tool loop: tool call → execute → write back → second
//!    response stops.
//! 3. Steer: queued mid-stream, jumps the queue to become the next user
//!    input after the current response.
//! 4. Follow-up: delivered when the agent is about to stop.
//! 5. Abort: `CancellationToken` / `agent.abort()` mid-turn; state
//!    stays consistent (no half `TurnEnded::Completed`).
//! 6. Tool errors (permission rule deny, declined prompt, unknown tool,
//!    failing tool, truncated length) become `is_error` tool results;
//!    the loop continues.
//!
//! Plus event-sequence assertions (subscribing to the broadcast bus)
//! and queue-mode drain semantics.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use mcode_agent::{
    Agent, AgentConfig, AllowAll, DenyAll, HookRunner, PermissionPrompt, QueueMode, TurnEnv,
};
use mcode_core::events::{MessageDelta, SessionEvent, TurnOutcome};
use mcode_core::message::{
    AssistantMessage, ContentBlock, Message, StopReason, ToolCall, UserMessage,
};
use mcode_llm::{FakeProvider, ScriptTurn};
use mcode_tools::permission::{PermissionEngine, PermissionRule, RuleAction};
use mcode_tools::{Tool, ToolCtx, ToolError, ToolRegistry, ToolResult, ToolStream};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::broadcast;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

/// A constant inter-event delay so concurrent test tasks (steerer,
/// canceller) win the race deterministically on the single-threaded
/// test runtime.
const DELAY: Duration = Duration::from_millis(2);

// ---------------------------------------------------------------------
// Test tools
// ---------------------------------------------------------------------

/// Echoes its `text` argument back.
struct EchoTool;

#[derive(Deserialize, JsonSchema)]
struct EchoArgs {
    /// Text to echo back.
    text: String,
}

#[async_trait]
impl Tool for EchoTool {
    type Args = EchoArgs;
    type Output = ();

    fn name(&self) -> &str {
        "echo"
    }
    fn description(&self) -> &str {
        "Echo text back (test fixture)."
    }
    async fn execute(
        &self,
        args: Self::Args,
        _ctx: &ToolCtx,
        _out: &mut ToolStream,
    ) -> Result<ToolResult, ToolError> {
        Ok(ToolResult::text(format!("echo: {}", args.text)))
    }
}

/// Always fails with an execution error.
struct FailingTool;

#[derive(Deserialize, JsonSchema)]
struct NoArgs {}

#[async_trait]
impl Tool for FailingTool {
    type Args = NoArgs;
    type Output = ();
    fn name(&self) -> &str {
        "failing"
    }
    fn description(&self) -> &str {
        "Always fails (test fixture)."
    }
    async fn execute(
        &self,
        _args: Self::Args,
        _ctx: &ToolCtx,
        _out: &mut ToolStream,
    ) -> Result<ToolResult, ToolError> {
        Err(ToolError::Execution(
            "boom: intentional test failure".into(),
        ))
    }
}

/// Pushes two progress items, then returns.
struct ProgressTool;

#[async_trait]
impl Tool for ProgressTool {
    type Args = NoArgs;
    type Output = ();
    fn name(&self) -> &str {
        "progress"
    }
    fn description(&self) -> &str {
        "Emits progress, then completes (test fixture)."
    }
    async fn execute(
        &self,
        _args: Self::Args,
        _ctx: &ToolCtx,
        out: &mut ToolStream,
    ) -> Result<ToolResult, ToolError> {
        out.progress("step 1");
        out.progress("step 2");
        Ok(ToolResult::text("progress done"))
    }
}

// ---------------------------------------------------------------------
// Test rig: owns the ambient objects, builds a TurnEnv per call
// ---------------------------------------------------------------------

struct Rig {
    provider: FakeProvider,
    registry: ToolRegistry,
    engine: PermissionEngine,
    hooks: HookRunner,
    events: broadcast::Sender<SessionEvent>,
    cancel: CancellationToken,
    prompt: Arc<dyn PermissionPrompt>,
}

impl Rig {
    fn new(provider: FakeProvider) -> Self {
        let registry = ToolRegistry::new();
        registry.register(Arc::new(EchoTool));
        registry.register(Arc::new(FailingTool));
        registry.register(Arc::new(ProgressTool));
        Self {
            provider,
            registry,
            engine: PermissionEngine::new(),
            hooks: HookRunner::new(),
            events: broadcast::channel(256).0,
            cancel: CancellationToken::new(),
            prompt: Arc::new(DenyAll),
        }
    }

    fn with_engine(mut self, engine: PermissionEngine) -> Self {
        self.engine = engine;
        self
    }

    fn with_prompt(mut self, prompt: Arc<dyn PermissionPrompt>) -> Self {
        self.prompt = prompt;
        self
    }

    fn env(&self) -> TurnEnv<'_> {
        TurnEnv::new(&self.provider, &self.registry, &self.engine, &self.hooks)
            .with_events(self.events.clone())
            .with_cancel(self.cancel.clone())
            .with_permission_prompt(self.prompt.clone())
    }
}

// ---------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------

fn user(text: &str) -> Message {
    Message::User(UserMessage::text(text))
}

fn text_turn(text: &str) -> ScriptTurn {
    ScriptTurn::Message(AssistantMessage {
        blocks: vec![ContentBlock::Text(text.into())],
        usage: None,
        stop_reason: StopReason::Stop,
    })
}

fn tool_turn(text: &str, calls: Vec<(&str, &str, Value)>) -> ScriptTurn {
    let mut blocks = vec![ContentBlock::Text(text.into())];
    for (id, name, args) in calls {
        blocks.push(ContentBlock::ToolCall(ToolCall {
            id: id.into(),
            name: name.into(),
            arguments: args,
        }));
    }
    ScriptTurn::Message(AssistantMessage {
        blocks,
        usage: None,
        stop_reason: StopReason::ToolUse,
    })
}

/// Collect events until the turn ends.
fn spawn_collector(rig: &Rig) -> JoinHandle<Vec<SessionEvent>> {
    let mut rx = rig.events.subscribe();
    tokio::spawn(async move {
        let mut out = Vec::new();
        while let Ok(event) = rx.recv().await {
            let done = matches!(event, SessionEvent::TurnEnded(_));
            out.push(event);
            if done {
                break;
            }
        }
        out
    })
}

/// Spawn a task that waits for the first streamed text delta and then
/// runs `f` (steer / follow-up / abort / cancel) mid-stream.
fn spawn_on_first_delta<F: FnOnce() + Send + 'static>(rig: &Rig, f: F) -> JoinHandle<()> {
    let mut rx = rig.events.subscribe();
    tokio::spawn(async move {
        while let Ok(event) = rx.recv().await {
            if matches!(
                event,
                SessionEvent::MessageDelta(MessageDelta::TextDelta(_))
            ) {
                break;
            }
        }
        f();
    })
}

fn position(events: &[SessionEvent], pred: impl Fn(&SessionEvent) -> bool, what: &str) -> usize {
    events
        .iter()
        .position(pred)
        .unwrap_or_else(|| panic!("missing event: {what}; events: {events:#?}"))
}

fn tool_result(events: &[SessionEvent]) -> mcode_core::message::ToolResultMessage {
    events
        .iter()
        .find_map(|event| match event {
            SessionEvent::ToolCompleted { result, .. } => Some(result.clone()),
            _ => None,
        })
        .expect("a ToolCompleted event must exist")
}

// ---------------------------------------------------------------------
// 1. Single-turn text reply stops
// ---------------------------------------------------------------------

#[tokio::test]
async fn single_text_reply_stops_and_streams_events() {
    let rig = Rig::new(FakeProvider::new(vec![text_turn("Hello there!")]));
    let mut agent = Agent::new(AgentConfig::new("fake-model").with_system_prompt("be terse"));
    let collector = spawn_collector(&rig);

    let outcome = agent
        .prompt(user("hi"), &rig.env())
        .await
        .expect("prompt must succeed");

    assert_eq!(outcome, TurnOutcome::Completed);
    assert_eq!(
        agent.state().messages,
        vec![
            user("hi"),
            Message::Assistant(AssistantMessage {
                blocks: vec![ContentBlock::Text("Hello there!".into())],
                usage: None,
                stop_reason: StopReason::Stop,
            })
        ]
    );
    assert!(!agent.state().is_streaming);

    // Request shape: system prompt, history, registry specs flow in.
    let requests = rig.provider.recorded_requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].model.as_str(), "fake-model");
    assert_eq!(requests[0].system_prompt, vec!["be terse".to_string()]);
    assert_eq!(requests[0].messages, vec![user("hi")]);
    assert!(requests[0].tools.iter().any(|spec| spec.name == "echo"));

    // Event order: TurnStarted → MessageAdded(user) → deltas →
    // MessageAdded(assistant) → TurnEnded(Completed).
    let events = collector.await.expect("collector must finish");
    assert_eq!(events.first(), Some(&SessionEvent::TurnStarted));
    assert_eq!(
        events.last(),
        Some(&SessionEvent::TurnEnded(TurnOutcome::Completed))
    );
    let user_pos = position(
        &events,
        |e| matches!(e, SessionEvent::MessageAdded(Message::User(_))),
        "MessageAdded(user)",
    );
    let delta_pos = position(
        &events,
        |e| matches!(e, SessionEvent::MessageDelta(MessageDelta::TextDelta(_))),
        "TextDelta",
    );
    let assistant_pos = position(
        &events,
        |e| matches!(e, SessionEvent::MessageAdded(Message::Assistant(_))),
        "MessageAdded(assistant)",
    );
    assert_eq!(user_pos, 1);
    assert!(user_pos < delta_pos);
    assert!(delta_pos < assistant_pos);
    // No tool or permission events in a pure text turn.
    assert!(!events.iter().any(|e| matches!(
        e,
        SessionEvent::ToolStarted { .. } | SessionEvent::PermissionRequested { .. }
    )));
}

// ---------------------------------------------------------------------
// 2. Multi-turn tool loop
// ---------------------------------------------------------------------

#[tokio::test]
async fn tool_call_loop_executes_writes_back_and_stops() {
    let rig = Rig::new(FakeProvider::new(vec![
        tool_turn("let me echo", vec![("c1", "echo", json!({"text": "hi"}))]),
        text_turn("I echoed the text."),
    ]));
    let mut agent = Agent::new(AgentConfig::new("fake-model"));
    let collector = spawn_collector(&rig);

    let outcome = agent
        .prompt(user("run the echo"), &rig.env())
        .await
        .expect("prompt must succeed");

    assert_eq!(outcome, TurnOutcome::Completed);
    let messages = &agent.state().messages;
    assert_eq!(messages.len(), 4); // user, assistant(call), result, assistant(final)
    let Message::ToolResult(result) = &messages[2] else {
        panic!("message 3 must be the tool result: {messages:#?}");
    };
    assert_eq!(result.tool_call_id, "c1");
    assert!(!result.is_error);
    assert_eq!(result.content, vec![ContentBlock::Text("echo: hi".into())]);

    // The second request carries the full history including the result.
    let requests = rig.provider.recorded_requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[1].messages.len(), 3);
    assert!(matches!(&requests[1].messages[2], Message::ToolResult(_)));
    assert_eq!(requests[1].messages, messages[..3]);

    let events = collector.await.expect("collector must finish");
    let started = position(
        &events,
        |e| matches!(e, SessionEvent::ToolStarted { call_id, name } if call_id.as_str() == "c1" && name == "echo"),
        "ToolStarted(c1)",
    );
    let completed = position(
        &events,
        |e| matches!(e, SessionEvent::ToolCompleted { result, .. } if !result.is_error),
        "ToolCompleted(c1)",
    );
    let result_added = position(
        &events,
        |e| matches!(e, SessionEvent::MessageAdded(Message::ToolResult(_))),
        "MessageAdded(ToolResult)",
    );
    assert!(started < completed);
    assert_eq!(
        events.last(),
        Some(&SessionEvent::TurnEnded(TurnOutcome::Completed))
    );
    let _ = result_added;
}

// ---------------------------------------------------------------------
// 3. Steer: queued mid-stream, becomes the next user input
// ---------------------------------------------------------------------

#[tokio::test]
async fn steer_jumps_the_queue_after_the_current_response() {
    let rig = Rig::new(
        FakeProvider::new(vec![
            tool_turn(
                "fetching the value",
                vec![("c1", "echo", json!({"text": "data"}))],
            ),
            text_turn("Understood, pivoting now."),
        ])
        .with_delay(DELAY),
    );
    let mut agent = Agent::new(AgentConfig::new("fake-model"));
    let handle = agent.handle();
    let collector = spawn_collector(&rig);
    let steerer = spawn_on_first_delta(&rig, move || {
        handle.steer(user("stop, do this instead"));
    });

    let outcome = agent
        .prompt(user("compute something"), &rig.env())
        .await
        .expect("prompt must succeed");
    steerer.await.expect("steerer must finish");

    assert_eq!(outcome, TurnOutcome::Steered);

    // History: user, assistant(call), result, steer-user, final assistant.
    let messages = &agent.state().messages;
    assert_eq!(messages.len(), 5);
    assert_eq!(messages[3], user("stop, do this instead"));
    assert!(matches!(messages[4], Message::Assistant(_)));

    // The steer message was injected as the next user input before the
    // second response — the queue-jump assertion.
    let requests = rig.provider.recorded_requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[1].messages, messages[..4]);
    assert_eq!(requests[1].messages[3], user("stop, do this instead"));
    assert!(agent.state().messages().iter().all(|_| true)); // sanity access

    let events = collector.await.expect("collector must finish");
    assert_eq!(
        events.last(),
        Some(&SessionEvent::TurnEnded(TurnOutcome::Steered))
    );
    // The tool from response 1 still executed before the steer landed.
    assert!(
        events
            .iter()
            .any(|e| matches!(e, SessionEvent::ToolCompleted { .. }))
    );
}

// ---------------------------------------------------------------------
// 4. Follow-up: continues the agent that is about to stop
// ---------------------------------------------------------------------

#[tokio::test]
async fn follow_up_continues_when_agent_would_stop() {
    let rig = Rig::new(
        FakeProvider::new(vec![
            text_turn("First answer, complete."),
            text_turn("Follow-up answer, also done."),
        ])
        .with_delay(DELAY),
    );
    let mut agent = Agent::new(AgentConfig::new("fake-model"));
    let handle = agent.handle();
    let followupper = spawn_on_first_delta(&rig, move || {
        handle.follow_up(user("and also check the tests"));
    });

    let outcome = agent
        .prompt(user("answer the question"), &rig.env())
        .await
        .expect("prompt must succeed");
    followupper.await.expect("follow-up task must finish");

    // Follow-ups do not mark the turn as steered.
    assert_eq!(outcome, TurnOutcome::Completed);

    let messages = &agent.state().messages;
    assert_eq!(messages.len(), 4); // user, assistant, follow-up user, assistant
    assert_eq!(messages[2], user("and also check the tests"));

    let requests = rig.provider.recorded_requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[1].messages[2], user("and also check the tests"));
}

// ---------------------------------------------------------------------
// 5. Abort: cancellation ends the turn with consistent state
// ---------------------------------------------------------------------

#[tokio::test]
async fn abort_via_env_cancel_mid_stream_keeps_state_consistent() {
    let long = "x".repeat(600); // many shards → ample cancel window
    let rig = Rig::new(FakeProvider::new(vec![text_turn(&long)]).with_delay(DELAY));
    let cancel = rig.cancel.clone();
    let canceller = spawn_on_first_delta(&rig, move || cancel.cancel());
    let mut agent = Agent::new(AgentConfig::new("fake-model"));
    let collector = spawn_collector(&rig);

    let outcome = agent
        .prompt(user("an essay please"), &rig.env())
        .await
        .expect("abort is a normal outcome, not an error");
    canceller.await.expect("canceller must finish");

    assert_eq!(outcome, TurnOutcome::Aborted);
    assert!(!agent.state().is_streaming);
    // No partial assistant message enters the history.
    assert_eq!(agent.state().messages, vec![user("an essay please")]);

    let events = collector.await.expect("collector must finish");
    assert_eq!(
        events.last(),
        Some(&SessionEvent::TurnEnded(TurnOutcome::Aborted))
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, SessionEvent::TurnEnded(TurnOutcome::Completed)))
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, SessionEvent::MessageAdded(Message::Assistant(_))))
    );

    // The agent recovers: a fresh caller token starts a fresh turn
    // (the original env token stays cancelled, by design).
    rig.provider.push_turn(text_turn("recovered"));
    let outcome = agent
        .prompt(
            user("try again"),
            &rig.env().with_cancel(CancellationToken::new()),
        )
        .await
        .expect("second prompt must succeed");
    assert_eq!(outcome, TurnOutcome::Completed);
    // user1, user2, recovered assistant (no partial aborted response).
    assert_eq!(agent.state().messages.len(), 3);
}

#[tokio::test]
async fn abort_via_agent_handle_mid_stream() {
    let long = "x".repeat(600);
    let rig = Rig::new(FakeProvider::new(vec![text_turn(&long)]).with_delay(DELAY));
    let mut agent = Agent::new(AgentConfig::new("fake-model"));
    let handle = agent.handle();
    let canceller = spawn_on_first_delta(&rig, move || handle.abort());

    let outcome = agent
        .prompt(user("an essay please"), &rig.env())
        .await
        .expect("abort is a normal outcome");
    canceller.await.expect("canceller must finish");

    assert_eq!(outcome, TurnOutcome::Aborted);
    assert_eq!(agent.state().messages.len(), 1);

    // abort() while idle is a no-op.
    agent.abort();
    rig.provider.push_turn(text_turn("fine"));
    let outcome = agent
        .prompt(user("again"), &rig.env())
        .await
        .expect("prompt after idle abort must succeed");
    assert_eq!(outcome, TurnOutcome::Completed);
}

// ---------------------------------------------------------------------
// 6. Tool errors as is_error results; the loop continues
// ---------------------------------------------------------------------

#[tokio::test]
async fn permission_rule_deny_becomes_error_result_and_loop_continues() {
    let rig = Rig::new(FakeProvider::new(vec![
        tool_turn(
            "calling echo",
            vec![("c1", "echo", json!({"text": "nope"}))],
        ),
        text_turn("I was denied; noted."),
    ]))
    .with_engine(PermissionEngine::with_rules(vec![PermissionRule::new(
        "echo",
        "*",
        RuleAction::Deny,
    )]));
    let mut agent = Agent::new(AgentConfig::new("fake-model"));
    let collector = spawn_collector(&rig);

    let outcome = agent
        .prompt(user("echo something"), &rig.env())
        .await
        .expect("prompt must succeed");

    assert_eq!(outcome, TurnOutcome::Completed);
    let events = collector.await.expect("collector must finish");
    let result = tool_result(&events);
    assert!(result.is_error);
    let ContentBlock::Text(text) = &result.content[0] else {
        panic!("error content must be text: {result:#?}");
    };
    assert!(text.contains("permission denied"));
    // The loop continued: the model saw the error and answered.
    assert_eq!(rig.provider.recorded_requests().len(), 2);
    let Message::ToolResult(history_result) = &agent.state().messages[2] else {
        panic!("history must contain the error tool result");
    };
    assert!(history_result.is_error);
    // No permission prompt was involved.
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, SessionEvent::PermissionRequested { .. }))
    );
}

#[tokio::test]
async fn declined_permission_prompt_becomes_error_result() {
    let rig = Rig::new(FakeProvider::new(vec![
        tool_turn(
            "calling echo",
            vec![("c1", "echo", json!({"text": "ask first"}))],
        ),
        text_turn("Declined; moving on."),
    ]))
    .with_engine(PermissionEngine::with_rules(vec![PermissionRule::new(
        "echo",
        "*",
        RuleAction::Ask,
    )]))
    .with_prompt(Arc::new(DenyAll));
    let mut agent = Agent::new(AgentConfig::new("fake-model"));
    let collector = spawn_collector(&rig);

    let outcome = agent
        .prompt(user("echo something"), &rig.env())
        .await
        .expect("prompt must succeed");

    assert_eq!(outcome, TurnOutcome::Completed);
    let events = collector.await.expect("collector must finish");

    // Permission telemetry brackets the prompt.
    let requested = position(
        &events,
        |e| matches!(e, SessionEvent::PermissionRequested { tool_name, .. } if tool_name == "echo"),
        "PermissionRequested",
    );
    let resolved = position(
        &events,
        |e| matches!(e, SessionEvent::PermissionResolved { allowed, .. } if !allowed),
        "PermissionResolved(denied)",
    );
    assert!(requested < resolved);
    // The telemetry payload shares the request id.
    let (
        Some(SessionEvent::PermissionRequested {
            request_id: requested_id,
            ..
        }),
        Some(SessionEvent::PermissionResolved {
            request_id: resolved_id,
            allowed,
        }),
    ) = (
        events
            .iter()
            .find(|e| matches!(e, SessionEvent::PermissionRequested { .. })),
        events
            .iter()
            .find(|e| matches!(e, SessionEvent::PermissionResolved { .. })),
    )
    else {
        panic!("permission events must exist");
    };
    assert_eq!(requested_id, resolved_id);
    assert!(!allowed);

    let result = tool_result(&events);
    assert!(result.is_error);
    assert_eq!(rig.provider.recorded_requests().len(), 2);
}

#[tokio::test]
async fn allowed_permission_prompt_executes_and_streams_progress() {
    let rig = Rig::new(FakeProvider::new(vec![
        tool_turn(
            "running the progress tool",
            vec![("c1", "progress", json!({}))],
        ),
        text_turn("All steps finished."),
    ]))
    .with_engine(PermissionEngine::with_rules(vec![PermissionRule::new(
        "progress",
        "*",
        RuleAction::Ask,
    )]))
    .with_prompt(Arc::new(AllowAll));
    let mut agent = Agent::new(AgentConfig::new("fake-model"));
    let collector = spawn_collector(&rig);

    let outcome = agent
        .prompt(user("run steps"), &rig.env())
        .await
        .expect("prompt must succeed");

    assert_eq!(outcome, TurnOutcome::Completed);
    let events = collector.await.expect("collector must finish");
    assert!(
        events
            .iter()
            .any(|e| matches!(e, SessionEvent::PermissionResolved { allowed, .. } if *allowed))
    );
    // Live progress was forwarded between start and completion.
    let started = position(
        &events,
        |e| matches!(e, SessionEvent::ToolStarted { call_id, .. } if call_id.as_str() == "c1"),
        "ToolStarted",
    );
    let step1 = position(
        &events,
        |e| matches!(e, SessionEvent::ToolProgress { message, .. } if message == "step 1"),
        "ToolProgress(step 1)",
    );
    let completed = position(
        &events,
        |e| matches!(e, SessionEvent::ToolCompleted { result, .. } if !result.is_error),
        "ToolCompleted",
    );
    assert!(started < step1);
    assert!(step1 < completed);
    let result = tool_result(&events);
    assert_eq!(
        result.content,
        vec![ContentBlock::Text("progress done".into())]
    );
}

#[tokio::test]
async fn unknown_tool_and_failing_tool_become_error_results() {
    let rig = Rig::new(FakeProvider::new(vec![
        tool_turn(
            "one unknown, one doomed",
            vec![("c1", "missing", json!({})), ("c2", "failing", json!({}))],
        ),
        text_turn("Both calls failed; understood."),
    ]));
    let mut agent = Agent::new(AgentConfig::new("fake-model"));
    let collector = spawn_collector(&rig);

    let outcome = agent
        .prompt(user("try both tools"), &rig.env())
        .await
        .expect("prompt must succeed");

    assert_eq!(outcome, TurnOutcome::Completed);
    let messages = &agent.state().messages;
    assert_eq!(messages.len(), 5); // user, assistant(2 calls), 2 results, assistant
    let Message::ToolResult(unknown_result) = &messages[2] else {
        panic!("first result must be a tool result");
    };
    let Message::ToolResult(failing_result) = &messages[3] else {
        panic!("second result must be a tool result");
    };
    assert!(unknown_result.is_error);
    let ContentBlock::Text(unknown_text) = &unknown_result.content[0] else {
        panic!("must be text")
    };
    assert!(unknown_text.contains("unknown tool"));
    assert!(failing_result.is_error);
    let ContentBlock::Text(failing_text) = &failing_result.content[0] else {
        panic!("must be text")
    };
    assert!(failing_text.contains("intentional test failure"));

    let events = collector.await.expect("collector must finish");
    let started = events
        .iter()
        .filter(|e| matches!(e, SessionEvent::ToolStarted { .. }))
        .count();
    assert_eq!(started, 2);
    assert_eq!(rig.provider.recorded_requests().len(), 2);
}

#[tokio::test]
async fn length_truncated_tool_calls_are_failed_not_executed() {
    let truncated = ScriptTurn::Message(AssistantMessage {
        blocks: vec![
            ContentBlock::Text("truncated".into()),
            ContentBlock::ToolCall(ToolCall {
                id: "c1".into(),
                name: "echo".into(),
                arguments: json!({"text": "cut off mid-way"}),
            }),
        ],
        usage: None,
        stop_reason: StopReason::Length,
    });
    let rig = Rig::new(FakeProvider::new(vec![
        truncated,
        text_turn("Re-issuing with complete arguments next time."),
    ]));
    let mut agent = Agent::new(AgentConfig::new("fake-model"));
    let collector = spawn_collector(&rig);

    let outcome = agent
        .prompt(user("do it"), &rig.env())
        .await
        .expect("prompt must succeed");

    assert_eq!(outcome, TurnOutcome::Completed);
    let events = collector.await.expect("collector must finish");
    let result = tool_result(&events);
    assert!(result.is_error);
    let ContentBlock::Text(text) = &result.content[0] else {
        panic!("must be text")
    };
    assert!(text.contains("token limit"));
    assert_eq!(rig.provider.recorded_requests().len(), 2);
}

// ---------------------------------------------------------------------
// Provider failure: error event + aborted turn + Err return
// ---------------------------------------------------------------------

#[tokio::test]
async fn provider_error_returns_err_without_half_completed_turn() {
    let rig = Rig::new(FakeProvider::new(vec![ScriptTurn::Error(
        mcode_llm::LlmError::Http {
            status: 500,
            body: "boom".into(),
        },
    )]));
    let mut agent = Agent::new(AgentConfig::new("fake-model"));
    let collector = spawn_collector(&rig);

    let err = agent
        .prompt(user("hi"), &rig.env())
        .await
        .expect_err("provider failure must surface as Err");
    assert!(matches!(err, mcode_core::McodeError::Provider(_)));
    assert!(!agent.state().is_streaming);
    assert_eq!(agent.state().messages, vec![user("hi")]);

    let events = collector.await.expect("collector must finish");
    assert!(
        events
            .iter()
            .any(|e| matches!(e, SessionEvent::Error(mcode_core::McodeError::Provider(_))))
    );
    assert_eq!(
        events.last(),
        Some(&SessionEvent::TurnEnded(TurnOutcome::Aborted))
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, SessionEvent::TurnEnded(TurnOutcome::Completed)))
    );
}

// ---------------------------------------------------------------------
// Queue modes
// ---------------------------------------------------------------------

#[tokio::test]
async fn queue_mode_one_at_a_time_delivers_each_follow_up_in_own_request() {
    let rig = Rig::new(FakeProvider::new(vec![
        text_turn("answer one"),
        text_turn("answer two"),
        text_turn("answer three"),
    ]));
    let mut agent = Agent::new(AgentConfig::new("fake-model"));
    assert_eq!(agent.queue_mode(), QueueMode::OneAtATime);
    // Queued while idle (a subagent callback before the next prompt).
    agent.follow_up(user("task one"));
    agent.follow_up(user("task two"));

    let outcome = agent
        .prompt(user("start"), &rig.env())
        .await
        .expect("prompt must succeed");

    assert_eq!(outcome, TurnOutcome::Completed);
    assert_eq!(rig.provider.recorded_requests().len(), 3);
    let messages = &agent.state().messages;
    // user, asst, task1, asst, task2, asst
    assert_eq!(messages.len(), 6);
    assert_eq!(messages[2], user("task one"));
    assert_eq!(messages[4], user("task two"));
}

#[tokio::test]
async fn queue_mode_all_batches_follow_ups_into_one_request() {
    let rig = Rig::new(FakeProvider::new(vec![
        text_turn("answer one"),
        text_turn("answer both"),
    ]));
    let mut agent = Agent::new(AgentConfig::new("fake-model"));
    agent.set_queue_mode(QueueMode::All);
    agent.follow_up(user("task one"));
    agent.follow_up(user("task two"));

    let outcome = agent
        .prompt(user("start"), &rig.env())
        .await
        .expect("prompt must succeed");

    assert_eq!(outcome, TurnOutcome::Completed);
    assert_eq!(rig.provider.recorded_requests().len(), 2);
    let requests = rig.provider.recorded_requests();
    // Both follow-ups were injected before the second response.
    assert_eq!(requests[1].messages[2], user("task one"));
    assert_eq!(requests[1].messages[3], user("task two"));
}

// ---------------------------------------------------------------------
// Steer queued while idle is delivered before the first response
// ---------------------------------------------------------------------

#[tokio::test]
async fn steer_queued_while_idle_lands_before_the_first_response() {
    let rig = Rig::new(FakeProvider::new(vec![text_turn("combined answer")]));
    let mut agent = Agent::new(AgentConfig::new("fake-model"));
    agent.steer(user("context update before you start"));

    let outcome = agent
        .prompt(user("initial prompt"), &rig.env())
        .await
        .expect("prompt must succeed");

    assert_eq!(outcome, TurnOutcome::Steered);
    let requests = rig.provider.recorded_requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].messages.len(), 2);
    assert_eq!(requests[0].messages[0], user("initial prompt"));
    assert_eq!(
        requests[0].messages[1],
        user("context update before you start")
    );
}

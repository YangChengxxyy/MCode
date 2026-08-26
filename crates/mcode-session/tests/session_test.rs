//! Session integration tests — the M1 T5 matrix from `07-m1-plan.md`
//! plus the actor behaviors, driven by a test-local scripted provider
//! (zero network):
//!
//! 1. Store write → reload: entry sequence fully equivalent
//!    (ids / parent_ids / messages), and the re-serialized lines are
//!    byte-equal to the original file.
//! 2. Fork: two branches appended past the fork point stay
//!    independent; `messages_to` replays each branch correctly.
//! 3. Resume: a tool-calling session is persisted, reloaded from disk,
//!    and the rebuilt history matches the in-memory one; the session
//!    keeps taking prompts and appending afterwards.
//! 4. Corrupt line: a broken JSON line mid-file is skipped and counted;
//!    a lost *node* line truncates (not crashes) the replay.
//! 5. Header validation: missing header / unsupported `format_version`
//!    are explicit errors.
//!
//! Actor behaviors on top: fork through the handle (branch visible in
//! the file, next turn's provider request truncated accordingly), the
//! Resume command reloading from disk, steer forwarded mid-turn and
//! persisted, and the `$MCODE_HOME` layout + resume-by-id wiring.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use mcode_agent::AgentConfig;
use mcode_core::events::{SessionCommand, SessionEvent, TurnOutcome};
use mcode_core::ids::{MessageId, SessionId};
use mcode_core::message::{
    AssistantMessage, ContentBlock, Message, StopReason, ToolCall, ToolResultMessage, UserMessage,
};
mod common;
use common::local_provider::{LocalProvider, LocalTurn};
use mcode_session::{
    AgentFactory, FORMAT_VERSION, SessionEntry, SessionHandle, SessionHeader, SessionStore,
    SessionTree, default_agent_factory, load_session,
};
use mcode_tools::{Tool, ToolCtx, ToolError, ToolRegistry, ToolResult, ToolStream};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::sync::broadcast;

// ---------------------------------------------------------------------
// Test fixtures
// ---------------------------------------------------------------------

/// Echoes its `text` argument back (the same fixture the agent-loop
/// tests use).
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

fn registry() -> Arc<ToolRegistry> {
    let registry = ToolRegistry::new();
    registry.register(Arc::new(EchoTool));
    Arc::new(registry)
}

fn factory() -> AgentFactory {
    default_agent_factory()
}

fn user(text: &str) -> Message {
    Message::User(UserMessage::text(text))
}

fn text_turn(text: &str) -> LocalTurn {
    LocalTurn::Message(AssistantMessage {
        blocks: vec![ContentBlock::Text(text.into())],
        usage: None,
        stop_reason: StopReason::Stop,
    })
}

fn tool_turn(text: &str, id: &str, name: &str, args: Value) -> LocalTurn {
    LocalTurn::Message(AssistantMessage {
        blocks: vec![
            ContentBlock::Text(text.into()),
            ContentBlock::ToolCall(ToolCall::new(id, name, args)),
        ],
        usage: None,
        stop_reason: StopReason::ToolUse,
    })
}

fn env_for(provider: &Arc<LocalProvider>, cwd: &Path) -> mcode_session::SessionEnv {
    mcode_session::SessionEnv::new(provider.clone(), registry()).with_cwd(cwd.to_path_buf())
}

/// Receive the next event with a generous timeout (a hung actor must
/// fail the test, not hang it).
async fn next_event(rx: &mut broadcast::Receiver<SessionEvent>) -> SessionEvent {
    tokio::time::timeout(Duration::from_secs(10), rx.recv())
        .await
        .expect("event within 10s")
        .expect("event channel open")
}

/// Consume events until the first streamed text delta (turn is live).
async fn wait_for_text_delta(rx: &mut broadcast::Receiver<SessionEvent>) {
    loop {
        if matches!(
            next_event(rx).await,
            SessionEvent::MessageDelta(mcode_core::events::MessageDelta::TextDelta(_))
        ) {
            return;
        }
    }
}

/// Collect events until (and including) the turn's `TurnEnded`.
async fn collect_turn(rx: &mut broadcast::Receiver<SessionEvent>) -> Vec<SessionEvent> {
    let mut out = Vec::new();
    loop {
        let event = next_event(rx).await;
        let done = matches!(event, SessionEvent::TurnEnded(_));
        out.push(event);
        if done {
            return out;
        }
    }
}

/// The `MessageAdded` payloads of one turn, in order — the in-memory
/// history as observed by subscribers.
fn added_messages(events: &[SessionEvent]) -> Vec<Message> {
    events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::MessageAdded(msg) => Some(msg.clone()),
            _ => None,
        })
        .collect()
}

/// Drive one prompt through a handle and return the turn's events.
async fn run_prompt(handle: &SessionHandle, text: &str) -> Vec<SessionEvent> {
    let mut rx = handle.subscribe();
    handle.prompt(user(text)).await.expect("prompt sends");
    collect_turn(&mut rx).await
}

/// Load a file and replay its latest branch into messages.
fn reload(path: &Path) -> mcode_session::LoadedSession {
    load_session(path).expect("session loads")
}

fn branch_messages(path: &Path) -> Vec<Message> {
    SessionTree::from_entries(reload(path).entries).branch_messages()
}

// ---------------------------------------------------------------------
// 1. Store write → reload: full entry-sequence equivalence
// ---------------------------------------------------------------------

#[test]
fn store_write_reload_is_equivalent_and_byte_stable() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("s.jsonl");

    let session_id = SessionId::new();
    let header = SessionHeader::new(session_id.clone(), "/Users/cc/app");
    let mut store = SessionStore::create(&path, &header).unwrap();

    let entries = vec![
        SessionEntry::Message {
            id: MessageId::from("a1"),
            parent_id: None,
            message: user("read Cargo.toml"),
        },
        SessionEntry::Message {
            id: MessageId::from("a2"),
            parent_id: Some(MessageId::from("a1")),
            message: Message::Assistant(AssistantMessage {
                blocks: vec![
                    ContentBlock::Thinking("checking".into()),
                    ContentBlock::ToolCall(ToolCall::new(
                        "call_1",
                        "read",
                        json!({"path": "Cargo.toml"}),
                    )),
                ],
                usage: Some(mcode_core::Usage {
                    input_tokens: 1200,
                    output_tokens: 42,
                }),
                stop_reason: StopReason::ToolUse,
            }),
        },
        SessionEntry::Label {
            id: MessageId::from("a2"),
            label: "探索实现方案".into(),
        },
        SessionEntry::Message {
            id: MessageId::from("a3"),
            parent_id: Some(MessageId::from("a2")),
            message: Message::ToolResult(ToolResultMessage {
                tool_call_id: "call_1".into(),
                content: vec![ContentBlock::Text("[workspace]".into())],
                is_error: false,
                details: Some(json!({"lines": 32})),
            }),
        },
        SessionEntry::Custom {
            id: MessageId::from("a4"),
            parent_id: Some(MessageId::from("a3")),
            kind: "plugin:plan".into(),
            data: json!({"steps": [1, 2], "done": false}),
        },
    ];
    for entry in &entries {
        store.append(entry).unwrap();
    }
    drop(store);

    let loaded = reload(&path);
    assert_eq!(loaded.skipped_lines, 0);
    assert_eq!(loaded.header.format_version, FORMAT_VERSION);
    assert_eq!(loaded.header.session_id, session_id);
    assert_eq!(loaded.header.cwd, "/Users/cc/app");
    // Entry sequence fully equivalent: ids, parent ids, payloads.
    assert_eq!(loaded.entries, entries);

    // Byte equivalence: re-serializing the loaded header + entries
    // reproduces the file exactly.
    let original = std::fs::read_to_string(&path).unwrap();
    let mut rebuilt = loaded.header.to_line().unwrap();
    for entry in &loaded.entries {
        rebuilt.push('\n');
        rebuilt.push_str(&serde_json::to_string(entry).unwrap());
    }
    rebuilt.push('\n');
    assert_eq!(original, rebuilt);

    // And the tree replays the full chain including the custom entry.
    let tree = SessionTree::from_entries(loaded.entries);
    assert_eq!(
        tree.branch_messages(),
        vec![
            user("read Cargo.toml"),
            entries[1].as_message().unwrap(),
            entries[3].as_message().unwrap(),
            entries[4].as_message().unwrap(),
        ]
    );
}

// ---------------------------------------------------------------------
// 2. Fork: independent branches past the fork point
// ---------------------------------------------------------------------

#[test]
fn fork_branches_append_independently() {
    let mut tree = SessionTree::new();
    for entry in [
        SessionEntry::Message {
            id: MessageId::from("a1"),
            parent_id: None,
            message: user("start"),
        },
        SessionEntry::Message {
            id: MessageId::from("a2"),
            parent_id: Some(MessageId::from("a1")),
            message: text_message("original branch"),
        },
        SessionEntry::Message {
            id: MessageId::from("a3"),
            parent_id: Some(MessageId::from("a2")),
            message: text_message("old tip"),
        },
    ] {
        tree.insert(entry);
    }

    let fork = tree.fork_at(&MessageId::from("a2")).unwrap();
    // Branch B grows off the fork point…
    tree.insert(SessionEntry::Message {
        id: MessageId::from("b1"),
        parent_id: Some(fork.id().clone()),
        message: text_message("fork branch"),
    });
    // …while branch A keeps appending off its own tip.
    tree.insert(SessionEntry::Message {
        id: MessageId::from("a4"),
        parent_id: Some(MessageId::from("a3")),
        message: text_message("more of the old branch"),
    });

    // Each branch replays its own history; neither sees the other.
    assert_eq!(
        texts(&tree.messages_to(&MessageId::from("b1")).unwrap()),
        ["start", "original branch", "fork branch"]
    );
    assert_eq!(
        texts(&tree.messages_to(&MessageId::from("a4")).unwrap()),
        [
            "start",
            "original branch",
            "old tip",
            "more of the old branch"
        ]
    );
    // Shared prefix, divergent tips: the fork point carries both branches.
    assert_eq!(
        tree.children_of(&MessageId::from("a2")),
        &[MessageId::from("a3"), MessageId::from("b1")]
    );
    assert_eq!(
        tree.children_of(&MessageId::from("a1")),
        &[MessageId::from("a2")]
    );
    // The most recently appended node defines the latest branch.
    assert_eq!(tree.latest_leaf(), Some(MessageId::from("a4")));
}

/// First text block of a user/assistant message (test assertions).
fn block_text(message: &Message) -> String {
    match message {
        Message::User(user) => match &user.content[0] {
            ContentBlock::Text(text) => text.text.clone(),
            block => panic!("unexpected user block: {block:?}"),
        },
        Message::Assistant(assistant) => match &assistant.blocks[0] {
            ContentBlock::Text(text) => text.text.clone(),
            block => panic!("unexpected assistant block: {block:?}"),
        },
        other => panic!("unexpected message: {other:?}"),
    }
}

fn texts(messages: &[Message]) -> Vec<String> {
    messages.iter().map(block_text).collect()
}

fn text_message(text: &str) -> Message {
    Message::Assistant(AssistantMessage {
        blocks: vec![ContentBlock::Text(text.into())],
        usage: None,
        stop_reason: StopReason::Stop,
    })
}

// ---------------------------------------------------------------------
// 3. Resume: tool-calling session persisted, reloaded, continued
// ---------------------------------------------------------------------

#[tokio::test]
async fn resume_rebuilds_history_and_continues_appending() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("s.jsonl");

    let provider = Arc::new(LocalProvider::new(vec![
        tool_turn("let me echo", "c1", "echo", json!({"text": "hi"})),
        text_turn("echoed."),
        text_turn("resumed answer."),
    ]));

    // Turn 1: a full tool-calling turn against the actor.
    let env = env_for(&provider, dir.path());
    let handle = SessionHandle::new_at(&path, env, AgentConfig::new("fake"), factory()).unwrap();
    let events = run_prompt(&handle, "use the tool").await;
    assert_eq!(
        events.last(),
        Some(&SessionEvent::TurnEnded(TurnOutcome::Completed))
    );
    handle.wait_processed(1).await.expect("turn persisted");
    let in_memory = added_messages(&events);
    assert_eq!(in_memory.len(), 4); // user, assistant(call), result, assistant(final)
    let session_id = handle.session_id();
    handle.shutdown().await.expect("actor exits cleanly");

    // The file holds the same four messages on one chain.
    assert_eq!(branch_messages(&path), in_memory);

    // Resume from disk: the rebuilt agent must carry exactly the
    // persisted history (proven by the next provider request).
    let env2 = env_for(&provider, dir.path());
    let handle = SessionHandle::resume_path(&path, env2, AgentConfig::new("fake"), factory())
        .expect("resume");
    assert_eq!(handle.session_id(), session_id);

    let events = run_prompt(&handle, "continue").await;
    assert_eq!(
        events.last(),
        Some(&SessionEvent::TurnEnded(TurnOutcome::Completed))
    );
    handle
        .wait_processed(1)
        .await
        .expect("second turn persisted");
    handle.shutdown().await.expect("actor exits cleanly");

    let requests = provider.recorded_requests();
    assert_eq!(requests.len(), 3);
    // Request 2 (first after resume) = rebuilt history + new prompt.
    assert_eq!(requests[2].messages.len(), 5);
    assert_eq!(requests[2].messages[..4], in_memory[..]);
    assert_eq!(requests[2].messages[4], user("continue"));

    // The tool call really executed in turn 1.
    assert!(matches!(&in_memory[2], Message::ToolResult(result) if !result.is_error));

    // Appending continued on the resumed file: 4 + 2 entries.
    let tree = SessionTree::from_entries(reload(&path).entries);
    assert_eq!(tree.node_count(), 6);
    assert_eq!(tree.branch_messages().len(), 6);
    assert_eq!(tree.branch_messages()[..4], in_memory[..]);
}

// ---------------------------------------------------------------------
// 4. Corrupt lines: skip + count; lost node truncates the replay
// ---------------------------------------------------------------------

#[test]
fn corrupt_line_mid_file_is_skipped_and_counted() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("s.jsonl");

    let mut store =
        SessionStore::create(&path, &SessionHeader::new(SessionId::new(), "/w")).unwrap();
    let entries = vec![
        SessionEntry::Message {
            id: MessageId::from("a1"),
            parent_id: None,
            message: user("one"),
        },
        SessionEntry::Message {
            id: MessageId::from("a2"),
            parent_id: Some(MessageId::from("a1")),
            message: user("two"),
        },
        SessionEntry::Message {
            id: MessageId::from("a3"),
            parent_id: Some(MessageId::from("a2")),
            message: user("three"),
        },
    ];
    for entry in &entries {
        store.append(entry).unwrap();
    }
    drop(store);

    // Splice a broken JSON line into the middle of the file.
    let lines: Vec<String> = std::fs::read_to_string(&path)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect();
    let mut corrupted = lines.clone();
    corrupted.insert(2, "{\"type\":\"message\",\"id\":\"broken\",".into());
    std::fs::write(&path, corrupted.join("\n") + "\n").unwrap();

    let loaded = reload(&path);
    assert_eq!(loaded.skipped_lines, 1);
    assert_eq!(loaded.entries, entries);

    // A *lost node* line (not just an extra bad line): its child's
    // parent link dangles, and the replay truncates at the break
    // instead of failing.
    let mut holed = lines.clone();
    holed.remove(2); // drop the a2 line
    std::fs::write(&path, holed.join("\n") + "\n").unwrap();
    let loaded = reload(&path);
    assert_eq!(loaded.skipped_lines, 0);
    let tree = SessionTree::from_entries(loaded.entries);
    assert_eq!(tree.node_count(), 2);
    assert_eq!(tree.branch_messages(), vec![user("three")]);
}

// ---------------------------------------------------------------------
// 5. Header validation: missing / unsupported version
// ---------------------------------------------------------------------

#[test]
fn missing_or_unsupported_header_is_an_explicit_error() {
    let dir = TempDir::new().unwrap();

    // Missing header: the file starts with an entry line.
    let no_header = dir.path().join("no_header.jsonl");
    std::fs::write(
        &no_header,
        serde_json::to_string(&SessionEntry::Message {
            id: MessageId::from("a1"),
            parent_id: None,
            message: user("hi"),
        })
        .unwrap()
            + "\n",
    )
    .unwrap();
    let err = load_session(&no_header).unwrap_err();
    assert!(err.to_string().contains("header"), "{err}");

    // Empty file: no header at all.
    let empty = dir.path().join("empty.jsonl");
    std::fs::write(&empty, "\n\n").unwrap();
    let err = load_session(&empty).unwrap_err();
    assert!(err.to_string().contains("no header"), "{err}");

    // Unsupported future version.
    let future = dir.path().join("future.jsonl");
    let mut header = SessionHeader::new(SessionId::new(), "/w");
    header.format_version = FORMAT_VERSION + 1;
    std::fs::write(&future, header.to_line().unwrap() + "\n").unwrap();
    let err = load_session(&future).unwrap_err();
    assert!(
        err.to_string().contains("unsupported") && err.to_string().contains("format_version"),
        "{err}"
    );
}

// ---------------------------------------------------------------------
// Actor: fork through the handle
// ---------------------------------------------------------------------

#[tokio::test]
async fn actor_fork_grows_a_branch_and_truncates_next_turn() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("s.jsonl");

    let provider = Arc::new(LocalProvider::new(vec![text_turn("one"), text_turn("two")]));
    let env = env_for(&provider, dir.path());
    let handle = SessionHandle::new_at(&path, env, AgentConfig::new("fake"), factory()).unwrap();

    run_prompt(&handle, "first").await;
    handle.wait_processed(1).await.expect("turn persisted");

    // Fork at the first entry (the user message).
    let tree = SessionTree::from_entries(reload(&path).entries);
    let fork_id = tree.entries()[0].entry_id().clone();
    handle.fork(fork_id.clone()).await.unwrap();
    handle.wait_processed(2).await.expect("fork applied");

    // The next turn runs on the truncated history: user1 + user2 only.
    run_prompt(&handle, "second").await;
    handle.wait_processed(3).await.expect("turn persisted");
    handle.shutdown().await.expect("actor exits cleanly");

    let requests = provider.recorded_requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[1].messages, vec![user("first"), user("second"),]);

    // The file now holds both branches: user1 → asst("one") and
    // user1 → user2 → asst("two").
    let tree = SessionTree::from_entries(reload(&path).entries);
    assert_eq!(tree.node_count(), 4);
    assert_eq!(tree.children_of(&fork_id).len(), 2);
    assert_eq!(
        tree.branch_messages(),
        vec![user("first"), user("second"), text_message("two")]
    );
    // The old branch is still replayable.
    let old_tip = tree.entries()[1].entry_id().clone();
    assert_eq!(
        tree.messages_to(&old_tip).unwrap(),
        vec![user("first"), text_message("one")]
    );
}

// ---------------------------------------------------------------------
// Actor: the Resume command reloads from disk
// ---------------------------------------------------------------------

#[tokio::test]
async fn actor_resume_command_reloads_state_from_disk() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("s.jsonl");

    let provider = Arc::new(LocalProvider::new(vec![
        text_turn("one"),
        text_turn("final"),
    ]));
    let env = env_for(&provider, dir.path());
    let handle = SessionHandle::new_at(&path, env, AgentConfig::new("fake"), factory()).unwrap();

    run_prompt(&handle, "first").await;
    handle.wait_processed(1).await.expect("turn persisted");

    // Fork in memory (to the first entry), then Resume this session:
    // the disk truth — user1 + assistant("one") — must win.
    let tree = SessionTree::from_entries(reload(&path).entries);
    let fork_id = tree.entries()[0].entry_id().clone();
    handle.fork(fork_id).await.unwrap();
    handle.wait_processed(2).await.expect("fork applied");

    handle
        .send(SessionCommand::Resume {
            session: handle.session_id(),
        })
        .await
        .unwrap();
    handle.wait_processed(3).await.expect("resume applied");

    let events = run_prompt(&handle, "more").await;
    assert_eq!(
        events.last(),
        Some(&SessionEvent::TurnEnded(TurnOutcome::Completed))
    );
    handle.wait_processed(4).await.expect("turn persisted");
    handle.shutdown().await.expect("actor exits cleanly");

    let requests = provider.recorded_requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[1].messages,
        vec![user("first"), text_message("one"), user("more")]
    );
    // The fork never wrote anything; the file has one linear chain.
    let tree = SessionTree::from_entries(reload(&path).entries);
    assert_eq!(tree.node_count(), 4);
    assert_eq!(
        tree.branch_messages(),
        vec![
            user("first"),
            text_message("one"),
            user("more"),
            text_message("final")
        ]
    );
}

// ---------------------------------------------------------------------
// Actor: steer forwarded mid-turn and persisted
// ---------------------------------------------------------------------

#[tokio::test]
async fn actor_forwards_steer_mid_turn_and_persists_it() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("s.jsonl");

    let provider = Arc::new(
        LocalProvider::new(vec![
            tool_turn("echoing", "c1", "echo", json!({"text": "x"})),
            text_turn("steered answer"),
        ])
        .with_delay(Duration::from_millis(2)),
    );
    let env = env_for(&provider, dir.path());
    let handle = SessionHandle::new_at(&path, env, AgentConfig::new("fake"), factory()).unwrap();

    // Prompt, then steer once the first delta is streaming: the actor
    // intercepts the steer mid-turn and forwards it to the agent handle
    // (past the loop-entry drain, so it lands after the tool boundary).
    let mut rx = handle.subscribe();
    handle.prompt(user("run the tool")).await.unwrap();
    wait_for_text_delta(&mut rx).await;
    handle.steer(user("actually stop")).await.unwrap();
    let events = collect_turn(&mut rx).await;
    assert_eq!(
        events.last(),
        Some(&SessionEvent::TurnEnded(TurnOutcome::Steered))
    );
    handle.wait_processed(1).await.expect("turn persisted");
    handle.shutdown().await.expect("actor exits cleanly");

    // The persisted chain: user, assistant(call), result, steer, final.
    let messages = branch_messages(&path);
    assert_eq!(messages.len(), 5);
    assert_eq!(messages[0], user("run the tool"));
    assert!(matches!(&messages[1], Message::Assistant(a) if a.blocks.len() == 2));
    assert!(
        matches!(&messages[2], Message::ToolResult(r) if r.tool_call_id == "c1" && !r.is_error)
    );
    assert_eq!(messages[3], user("actually stop"));
    assert_eq!(messages[4], text_message("steered answer"));
}

// ---------------------------------------------------------------------
// Paths: $MCODE_HOME layout + resume by id
// ---------------------------------------------------------------------

// The only test that touches process environment variables; every
// other test passes explicit paths, so there is nothing to race.
#[tokio::test]
async fn mcode_home_override_lays_out_sessions_and_resume_by_id_finds_them() {
    let dir = TempDir::new().unwrap();
    let project = dir.path().join("project");
    std::fs::create_dir_all(&project).unwrap();

    unsafe { std::env::set_var("MCODE_HOME", dir.path().as_os_str()) };

    let provider = Arc::new(LocalProvider::new(vec![text_turn("answer")]));
    let env = env_for(&provider, &project);
    let handle =
        SessionHandle::new(env, AgentConfig::new("fake"), factory()).expect("session starts");

    // The file landed under <MCODE_HOME>/sessions/<cwd-slug>/ with the
    // timestamp_uuid.jsonl name.
    let path = handle.path();
    let relative = path.strip_prefix(dir.path()).unwrap();
    let expected_dir = Path::new("sessions").join(mcode_session::cwd_slug(&project));
    assert_eq!(relative.parent(), Some(expected_dir.as_path()));
    let name = relative.file_name().unwrap().to_str().unwrap();
    assert!(name.ends_with(".jsonl"));
    assert_eq!(name.matches('_').count(), 1, "{name}");

    // Run one turn so the file has content, then resume by id alone.
    let events = run_prompt(&handle, "hello").await;
    assert_eq!(
        events.last(),
        Some(&SessionEvent::TurnEnded(TurnOutcome::Completed))
    );
    handle.wait_processed(1).await.expect("turn persisted");
    let session_id = handle.session_id();
    handle.shutdown().await.expect("actor exits cleanly");
    assert!(path.is_file());

    // Resume by id alone — still under the override, so the search
    // scans this tempdir's sessions tree.
    let provider2 = Arc::new(LocalProvider::new(vec![]));
    let env2 = env_for(&provider2, &project);
    let resumed = SessionHandle::resume(
        &session_id.to_string(),
        env2,
        AgentConfig::new("fake"),
        factory(),
    )
    .expect("resume by id");
    assert_eq!(resumed.session_id(), session_id);
    assert_eq!(resumed.path(), path);
    resumed.shutdown().await.expect("actor exits cleanly");

    unsafe { std::env::remove_var("MCODE_HOME") };
}

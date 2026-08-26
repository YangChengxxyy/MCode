// Rust guideline compliant 2026-08-26.

use std::time::Duration;

use mcode_compaction::{
    CompactionCut, CompactionError, CompactionInput, CompactionMessage, CompactionOutput,
    CompactionPolicy, ContextTokenBudget, DeterministicDetails, DeterministicOperation,
    TokenEstimator, TriggerReason, ValidationCode, compact_context, plan_compaction,
    rebuild_context, validate_rebuilt_context,
};
use mcode_core::{
    AssistantMessage, ContentBlock, CustomMessage, Message, MessageId, StopReason, TextBlock,
    ToolCall, ToolResultMessage, UserMessage,
};
use mcode_llm::{CancellationToken, LlmError};

mod common;
use common::local_provider::{LocalProvider, LocalTurn};
use serde_json::json;

const CONTEXT_TOKENS: u64 = 100_000;

fn user(text: impl Into<String>) -> Message {
    Message::User(UserMessage::text(text))
}

fn assistant(text: impl Into<String>) -> Message {
    Message::Assistant(AssistantMessage {
        blocks: vec![ContentBlock::Text(TextBlock::new(text))],
        usage: None,
        stop_reason: StopReason::Stop,
    })
}

fn assistant_calls(calls: &[(&str, &str)]) -> Message {
    Message::Assistant(AssistantMessage {
        blocks: calls
            .iter()
            .map(|(id, name)| {
                ContentBlock::ToolCall(ToolCall::new(
                    (*id).to_owned(),
                    (*name).to_owned(),
                    json!({"value": id}),
                ))
            })
            .collect(),
        usage: None,
        stop_reason: StopReason::ToolUse,
    })
}

fn tool_result(id: &str, text: impl Into<String>) -> Message {
    Message::ToolResult(ToolResultMessage {
        tool_call_id: id.to_owned(),
        content: vec![ContentBlock::Text(TextBlock::new(text))],
        is_error: false,
        details: Some(json!({"not_for_model": true})),
    })
}

fn source(index: usize, message: Message, tokens: u64) -> CompactionMessage {
    CompactionMessage::new(message)
        .with_id(MessageId::from(format!("m{index}")))
        .with_token_count(tokens)
}

fn automatic_input(messages: Vec<CompactionMessage>, total_tokens: u64) -> CompactionInput {
    CompactionInput::new(
        "current-model",
        ContextTokenBudget::new(CONTEXT_TOKENS, total_tokens),
        messages,
    )
}

fn base_policy() -> CompactionPolicy {
    CompactionPolicy::new()
        .with_reserve_tokens(10_000)
        .with_max_summary_tokens(2_000)
}

fn valid_summary() -> String {
    r#"## Goal
Complete the private context-compaction foundation while preserving exact host state and safe conversation continuity.

## Constraints & Preferences
Keep provider selection with the host, expose no plugin strategy surface, avoid network tests, and preserve tool pairs.

## Progress
### Done
The planner, bounded transcript, deterministic sidecar, retry loop, and transactional validators are implemented and tested locally.

### In Progress
Final host actor and append-only session wiring remains intentionally outside this isolated foundation change.

### Blocked
No implementation blocker is known; native provider output limits await a future request-field addition.

## Key Decisions
Use fixed versioned schemas, conservative fallback estimation, complete user-turn cuts, and explicit split-prefix plans only when required.

## Files & Commands
- Model-proposed path that must not become authoritative: fake.txt

## Next Steps
Integrate from the session actor with optimistic branch checks, append a versioned entry, and swap in-memory state atomically.

## Critical Context
The latest real user request and deterministic file, command, todo, and background records remain typed side fields rather than model claims."#
        .to_owned()
}

fn compact_valid_summary() -> String {
    r#"## Goal
Preserve useful context safely.
## Constraints & Preferences
Keep the core private.
## Progress
### Done
Planning and validation work.
### In Progress
Actor wiring remains.
### Blocked
None.
## Key Decisions
Keep tool pairs intact.
## Files & Commands
- No deterministic file or command records were supplied by the host.

## Next Steps
Apply the validated result atomically.
## Critical Context
Retain exact host side fields."#
        .to_owned()
}

fn summary_turn(summary: impl Into<String>) -> LocalTurn {
    summary_turn_with_stop(summary, StopReason::Stop)
}

fn summary_turn_with_stop(summary: impl Into<String>, stop_reason: StopReason) -> LocalTurn {
    LocalTurn::Message(AssistantMessage {
        blocks: vec![ContentBlock::Text(TextBlock::new(summary))],
        usage: None,
        stop_reason,
    })
}

fn standard_messages() -> Vec<CompactionMessage> {
    vec![
        source(0, user("old request"), 25_000),
        source(1, assistant("old response"), 25_000),
        source(2, user("recent request"), 10_000),
        source(3, assistant("recent response"), 10_000),
    ]
}

#[test]
fn automatic_threshold_is_minimum_of_eighty_five_percent_and_reserve() {
    let policy = base_policy();
    let below = automatic_input(standard_messages(), 84_999);
    assert!(plan_compaction(&below, &policy).unwrap().is_none());

    let at_threshold = automatic_input(standard_messages(), 85_000);
    let plan = plan_compaction(&at_threshold, &policy)
        .unwrap()
        .expect("threshold should trigger");
    assert_eq!(plan.trigger_threshold_tokens(), 85_000);

    let reserve_limited = CompactionPolicy::new()
        .with_reserve_tokens(20_000)
        .with_max_summary_tokens(2_000);
    let input = automatic_input(standard_messages(), 80_000);
    let plan = plan_compaction(&input, &reserve_limited)
        .unwrap()
        .expect("context minus reserve should trigger");
    assert_eq!(plan.trigger_threshold_tokens(), 80_000);
}

#[test]
fn planner_cuts_at_complete_user_turn_boundary() {
    let messages = vec![
        source(0, user("turn one"), 20_000),
        source(1, assistant("answer one"), 20_000),
        source(2, user("turn two"), 15_000),
        source(3, assistant("answer two"), 15_000),
        source(4, user("turn three"), 10_000),
        source(5, assistant("answer three"), 10_000),
    ];
    let plan = plan_compaction(&automatic_input(messages, 90_000), &base_policy())
        .unwrap()
        .unwrap();
    assert!(matches!(
        plan.cut(),
        CompactionCut::MessageBoundary {
            next_message_index: 4,
            split_turn: false,
            ..
        }
    ));
    assert_eq!(plan.keep_recent_tokens(), 20_000);
}

#[test]
fn parallel_and_interleaved_tool_pairs_remain_on_one_side() {
    let messages = vec![
        source(0, user("run tools"), 8_000),
        source(
            1,
            assistant_calls(&[("c1", "read"), ("c2", "grep")]),
            12_000,
        ),
        source(2, assistant_calls(&[("c3", "bash")]), 10_000),
        source(3, tool_result("c2", "grep result"), 10_000),
        source(4, tool_result("c3", "bash result"), 10_000),
        source(5, tool_result("c1", "read result"), 20_000),
        source(6, user("latest request"), 10_000),
        source(7, assistant("latest answer"), 10_000),
    ];
    let plan = plan_compaction(&automatic_input(messages, 90_000), &base_policy())
        .unwrap()
        .unwrap();
    assert!(matches!(
        plan.cut(),
        CompactionCut::MessageBoundary {
            next_message_index: 6,
            split_turn: false,
            ..
        }
    ));
}

#[test]
fn orphan_tool_result_is_rejected() {
    let input = automatic_input(
        vec![
            source(0, user("bad history"), 40_000),
            source(1, tool_result("missing", "orphan"), 30_000),
            source(2, user("latest"), 10_000),
            source(3, assistant("answer"), 10_000),
        ],
        90_000,
    );
    let error = plan_compaction(&input, &base_policy()).unwrap_err();
    assert_eq!(error.code(), ValidationCode::OrphanToolResult);
}

#[test]
fn unresolved_tool_call_is_rejected() {
    let input = automatic_input(
        vec![
            source(0, user("bad history"), 40_000),
            source(1, assistant_calls(&[("never-answered", "read")]), 30_000),
            source(2, user("latest"), 10_000),
            source(3, assistant("answer"), 10_000),
        ],
        90_000,
    );
    let error = plan_compaction(&input, &base_policy()).unwrap_err();
    assert_eq!(error.code(), ValidationCode::UnresolvedToolCall);
}

#[test]
fn oversized_single_turn_gets_a_text_split_prefix_plan() {
    let huge = "large request segment ".repeat(15_000);
    let input = automatic_input(vec![source(0, user(huge), 90_000)], 90_000);
    let plan = plan_compaction(&input, &base_policy())
        .unwrap()
        .expect("oversized turn should be splittable");
    assert!(matches!(
        plan.cut(),
        CompactionCut::UserTextPrefix {
            message_index: 0,
            char_offset,
            ..
        } if *char_offset > 0
    ));
    assert!(plan.cut().is_split_prefix());
    assert!(plan.estimated_retained_tokens() <= plan.keep_recent_tokens());
}

#[tokio::test]
async fn previous_summary_occurs_once_and_new_span_is_separate() {
    let sentinel = "PREVIOUS_SUMMARY_UNIQUE_SENTINEL";
    let input = automatic_input(standard_messages(), 90_000)
        .with_previous_summary(format!("{sentinel}: prior compacted facts only"));
    let provider = LocalProvider::new(vec![summary_turn(valid_summary())]);
    let output = compact_context(&provider, &input, &base_policy(), &CancellationToken::new())
        .await
        .unwrap()
        .unwrap();
    assert!(!output.summary().contains(sentinel));

    let requests = provider.recorded_requests();
    assert!(requests[0].tools.is_empty());
    assert!(requests[0].thinking.is_none());
    assert_eq!(requests[0].model.as_str(), "current-model");
    let Message::User(prompt) = &requests[0].messages[0] else {
        panic!("expected summary prompt user message");
    };
    let prompt_text = prompt
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.text.as_str()),
            _ => None,
        })
        .collect::<String>();
    assert_eq!(prompt_text.matches(sentinel).count(), 1);
    assert!(prompt_text.contains("<previous_summary_data>"));
    assert!(prompt_text.contains("<new_conversation_span_data>"));
}

#[tokio::test]
async fn custom_state_never_enters_the_llm_request_and_survives_rebuild() {
    let custom = Message::Custom(CustomMessage {
        kind: "plugin:secret-state".to_owned(),
        data: json!({"private_sentinel": "CUSTOM_PAYLOAD_MUST_STAY_PRIVATE"}),
    });
    assert_eq!(TokenEstimator::conservative().estimate_message(&custom), 0);
    let messages = vec![
        source(0, user("old request"), 25_000),
        source(1, custom.clone(), 50_000),
        source(2, assistant("old response"), 25_000),
        source(3, user("recent request"), 10_000),
        source(4, assistant("recent response"), 10_000),
    ];
    let input = automatic_input(messages, 90_000);
    let provider = LocalProvider::new(vec![summary_turn(valid_summary())]);
    let output = compact_context(&provider, &input, &base_policy(), &CancellationToken::new())
        .await
        .unwrap()
        .unwrap();

    let request_json = serde_json::to_string(&provider.recorded_requests()[0]).unwrap();
    assert!(!request_json.contains("plugin:secret-state"));
    assert!(!request_json.contains("CUSTOM_PAYLOAD_MUST_STAY_PRIVATE"));
    let rebuilt = rebuild_context(&input, &output).unwrap();
    assert_eq!(
        rebuilt.iter().filter(|message| *message == &custom).count(),
        1
    );
}

#[tokio::test]
async fn transcript_keeps_messages_nearest_the_cut_and_records_older_omissions() {
    let messages = vec![
        source(0, user("OLDEST_MESSAGE_MUST_BE_OMITTED"), 20_000),
        source(1, assistant("x".repeat(350_000)), 20_000),
        source(2, user("near-cut user progress"), 20_000),
        source(
            3,
            assistant("NEAREST_COMPACTED_MESSAGE_MUST_BE_PRESENT"),
            20_000,
        ),
        source(4, user("retained request"), 5_000),
        source(5, assistant("retained response"), 5_000),
    ];
    let input = automatic_input(messages, 90_000);
    let provider = LocalProvider::new(vec![summary_turn(valid_summary())]);
    let output = compact_context(&provider, &input, &base_policy(), &CancellationToken::new())
        .await
        .unwrap()
        .unwrap();

    assert_eq!(output.details().transcript_omitted_messages(), 1);
    let request_json = serde_json::to_string(&provider.recorded_requests()[0]).unwrap();
    assert!(request_json.contains("NEAREST_COMPACTED_MESSAGE_MUST_BE_PRESENT"));
    assert!(!request_json.contains("OLDEST_MESSAGE_MUST_BE_OMITTED"));
    assert!(request_json.contains("omitted_older_messages=1"));
}

#[tokio::test]
async fn previous_summary_consumes_the_transcript_request_budget() {
    let previous_summary = "p".repeat(2_000);
    assert_eq!(
        TokenEstimator::conservative().estimate_text(&previous_summary),
        2_000
    );
    let messages = vec![
        source(0, user("older source message"), 3_000),
        source(1, assistant("m".repeat(6_500)), 3_000),
        source(2, user("retained request"), 1_000),
        source(3, assistant("retained response"), 1_000),
    ];
    let input = CompactionInput::new(
        "current-model",
        ContextTokenBudget::new(12_000, 10_500),
        messages,
    )
    .with_previous_summary(previous_summary);
    let policy = CompactionPolicy::new()
        .with_reserve_tokens(1_000)
        .with_keep_recent_tokens(2_000)
        .with_max_summary_tokens(2_000);
    let provider = LocalProvider::new(vec![summary_turn(valid_summary())]);
    let output = compact_context(&provider, &input, &policy, &CancellationToken::new())
        .await
        .unwrap()
        .unwrap();

    assert_eq!(output.details().transcript_omitted_messages(), 1);
    let request = &provider.recorded_requests()[0];
    let estimator = TokenEstimator::conservative();
    let request_tokens = request
        .system_prompt
        .iter()
        .fold(0_u64, |total, prompt| {
            total.saturating_add(estimator.estimate_text(prompt))
        })
        .saturating_add(estimator.estimate_messages(request.messages.iter()));
    assert!(
        request_tokens
            <= output
                .plan()
                .result_budget_tokens()
                .saturating_sub(output.plan().max_summary_tokens())
    );
}

#[tokio::test]
async fn oversized_previous_summaries_are_rejected_before_provider_use() {
    let cases = [
        (
            "x".repeat(7_000),
            base_policy(),
            "token ceiling should reject the input",
        ),
        (
            "x".repeat(65_537),
            CompactionPolicy::new()
                .with_reserve_tokens(10_000)
                .with_max_summary_tokens(70_000),
            "character ceiling should reject the input",
        ),
    ];
    for (previous_summary, policy, case) in cases {
        let input =
            automatic_input(standard_messages(), 90_000).with_previous_summary(previous_summary);
        let provider = LocalProvider::new(vec![summary_turn(valid_summary())]);
        let error = compact_context(&provider, &input, &policy, &CancellationToken::new())
            .await
            .expect_err(case);
        assert_eq!(
            error.validation().expect("validation error").code(),
            ValidationCode::SummaryBudgetExceeded
        );
        assert!(provider.recorded_requests().is_empty());
    }
}

#[tokio::test]
async fn latest_real_user_request_is_preserved_verbatim() {
    let latest = UserMessage {
        content: vec![
            ContentBlock::Text("exact first line\n  exact indentation".into()),
            ContentBlock::Text("second block <verbatim>".into()),
        ],
    };
    let mut messages = standard_messages();
    messages[2] = source(2, Message::User(latest.clone()), 10_000);
    let input = automatic_input(messages, 90_000);
    let provider = LocalProvider::new(vec![summary_turn(valid_summary())]);
    let output = compact_context(&provider, &input, &base_policy(), &CancellationToken::new())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        output
            .details()
            .latest_user_request()
            .expect("latest request")
            .message(),
        &latest
    );
}

#[tokio::test]
async fn tool_output_is_bounded_and_original_message_is_unchanged() {
    let messages = vec![
        source(0, user("inspect output"), 5_000),
        source(1, assistant_calls(&[("c1", "read")]), 10_000),
        source(2, tool_result("c1", "x".repeat(120_000)), 55_000),
        source(3, assistant("old completion"), 10_000),
        source(4, user("latest request"), 5_000),
        source(5, assistant("latest response"), 5_000),
    ];
    let input = automatic_input(messages, 90_000);
    let original = input.clone();
    let provider = LocalProvider::new(vec![summary_turn(valid_summary())]);
    let output = compact_context(&provider, &input, &base_policy(), &CancellationToken::new())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(input, original);
    assert_eq!(output.details().tool_result_truncations().len(), 1);

    let request_json = serde_json::to_string(&provider.recorded_requests()[0]).unwrap();
    assert!(request_json.contains("[TRUNCATED tool result:"));
    assert!(!request_json.contains("not_for_model"));
}

#[tokio::test]
async fn deterministic_details_merge_deduplicates_and_current_status_wins() {
    let previous = DeterministicDetails::new()
        .with_file_read("src/lib.rs")
        .with_command("cargo test")
        .with_todo_operation(DeterministicOperation::new(
            "todo-1",
            "write validator",
            "pending",
        ));
    let current = DeterministicDetails::new()
        .with_file_read("src/lib.rs")
        .with_file_read("src/planner.rs")
        .with_file_modified("src/lib.rs")
        .with_command("cargo test")
        .with_todo_operation(DeterministicOperation::new(
            "todo-1",
            "write validator",
            "completed",
        ))
        .with_background_operation(DeterministicOperation::new(
            "bg-1",
            "local checks",
            "running",
        ));
    let input = automatic_input(standard_messages(), 90_000)
        .with_previous_details(previous)
        .with_details(current);
    let provider = LocalProvider::new(vec![summary_turn(valid_summary())]);
    let output = compact_context(&provider, &input, &base_policy(), &CancellationToken::new())
        .await
        .unwrap()
        .unwrap();
    let details = output.details().deterministic();
    assert_eq!(details.files_read().len(), 2);
    assert_eq!(details.files_modified().len(), 1);
    assert_eq!(details.commands(), &["cargo test".to_owned()]);
    assert_eq!(details.todo_operations().len(), 1);
    assert_eq!(details.todo_operations()[0].status(), "completed");
    assert_eq!(details.background_operations().len(), 1);
    assert!(!output.summary().contains("fake.txt"));
    assert_eq!(output.summary().matches("src/lib.rs").count(), 2);
}

#[tokio::test]
async fn host_commands_cannot_trigger_provider_output_validation() {
    let details = DeterministicDetails::new()
        .with_command("rg '<previous_summary_data>'")
        .with_command("printf '## Next Steps'");
    let input = automatic_input(standard_messages(), 90_000).with_details(details);
    let provider = LocalProvider::new(vec![summary_turn(valid_summary())]);
    let output = compact_context(&provider, &input, &base_policy(), &CancellationToken::new())
        .await
        .unwrap()
        .unwrap();

    assert!(output.summary().contains("rg '<previous_summary_data>'"));
    assert!(output.summary().contains("printf '## Next Steps'"));
    validate_rebuilt_context(&input, &output).unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn deterministic_paths_use_exact_unix_identity() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;
    use std::path::PathBuf;

    let lossy_a = PathBuf::from(OsString::from_vec(vec![b'n', 0x80]));
    let lossy_b = PathBuf::from(OsString::from_vec(vec![b'n', 0x81]));
    assert_eq!(lossy_a.to_string_lossy(), lossy_b.to_string_lossy());
    let details = DeterministicDetails::new()
        .with_file_read(r"a\b")
        .with_file_read("a/b")
        .with_file_read(lossy_a.clone())
        .with_file_read(lossy_b.clone());
    let input = automatic_input(standard_messages(), 90_000).with_details(details);
    let provider = LocalProvider::new(vec![summary_turn(valid_summary())]);
    let output = compact_context(&provider, &input, &base_policy(), &CancellationToken::new())
        .await
        .unwrap()
        .unwrap();

    let expected = [
        PathBuf::from(r"a\b"),
        PathBuf::from("a/b"),
        lossy_a.clone(),
        lossy_b.clone(),
    ];
    assert_eq!(output.details().deterministic().files_read(), &expected);

    // The exact identity must survive the planned JSONL session entry: the
    // derived serde PathBuf encoding would have rejected these paths.
    let json = serde_json::to_string(&output).expect("non-UTF-8 paths must persist");
    let value: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert!(json.contains("unix_bytes"));
    let roundtrip: CompactionOutput = serde_json::from_value(value).unwrap();
    assert_eq!(roundtrip, output);
    assert_eq!(roundtrip.details().deterministic().files_read(), &expected);
}

#[cfg(windows)]
#[tokio::test]
async fn deterministic_paths_survive_non_utf16_persistence_exactly() {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;
    use std::path::PathBuf;

    // Unpaired surrogates are not valid UTF-8 but are legal Windows paths.
    let surrogate_a = PathBuf::from(OsString::from_wide(&[b'a' as u16, 0xD800, b'b' as u16]));
    let surrogate_b = PathBuf::from(OsString::from_wide(&[b'a' as u16, 0xDFFF, b'b' as u16]));
    assert_eq!(surrogate_a.to_string_lossy(), surrogate_b.to_string_lossy());
    let details = DeterministicDetails::new()
        .with_file_read("src/lib.rs")
        .with_file_read(surrogate_a.clone())
        .with_file_read(surrogate_b.clone());
    let input = automatic_input(standard_messages(), 90_000).with_details(details);
    let provider = LocalProvider::new(vec![summary_turn(valid_summary())]);
    let output = compact_context(&provider, &input, &base_policy(), &CancellationToken::new())
        .await
        .unwrap()
        .unwrap();

    let expected = [PathBuf::from("src/lib.rs"), surrogate_a, surrogate_b];
    assert_eq!(output.details().deterministic().files_read(), &expected);

    // Tagged UTF-16 code-unit persistence round-trips exactly as valid JSON.
    let json = serde_json::to_string(&output).expect("surrogate paths must persist");
    assert!(json.contains("windows_utf16"));
    assert!(json.contains("src/lib.rs"));
    let roundtrip: CompactionOutput = serde_json::from_str(&json).unwrap();
    assert_eq!(roundtrip, output);
    assert_eq!(roundtrip.details().deterministic().files_read(), &expected);
}

#[tokio::test]
async fn cancellation_stops_immediately_without_changing_input() {
    let input = automatic_input(standard_messages(), 90_000);
    let original = input.clone();
    let provider = LocalProvider::new(vec![summary_turn(valid_summary())])
        .with_delay(Duration::from_millis(100));
    let cancel = CancellationToken::new();
    let policy = base_policy();
    let operation = compact_context(&provider, &input, &policy, &cancel);
    tokio::pin!(operation);
    tokio::select! {
        result = &mut operation => panic!("operation completed before cancellation: {result:?}"),
        _ = tokio::time::sleep(Duration::from_millis(10)) => cancel.cancel(),
    }
    let error = operation.await.unwrap_err();
    assert!(error.is_cancelled());
    assert_eq!(input, original);
}

#[tokio::test]
async fn policies_above_three_provider_attempts_are_rejected() {
    let policy = base_policy().with_max_attempts(4);
    let provider = LocalProvider::new(vec![summary_turn(valid_summary())]);
    let error = compact_context(
        &provider,
        &automatic_input(standard_messages(), 90_000),
        &policy,
        &CancellationToken::new(),
    )
    .await
    .unwrap_err();
    assert_eq!(
        error.validation().expect("validation error").code(),
        ValidationCode::InvalidPolicy
    );
    assert!(provider.recorded_requests().is_empty());
}

#[tokio::test]
async fn transient_failures_retry_at_most_three_attempts() {
    let provider = LocalProvider::new(vec![
        LocalTurn::Fail(LlmError::Transport("connection reset".into())),
        LocalTurn::Fail(LlmError::Http {
            status: 503,
            body: "unavailable".into(),
        }),
        summary_turn(valid_summary()),
    ]);
    let output = compact_context(
        &provider,
        &automatic_input(standard_messages(), 90_000),
        &base_policy(),
        &CancellationToken::new(),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(output.details().attempts(), 3);
    assert_eq!(provider.recorded_requests().len(), 3);
}

#[tokio::test]
async fn transient_retry_count_is_bounded() {
    let provider = LocalProvider::new(vec![
        LocalTurn::Fail(LlmError::Timeout),
        LocalTurn::Fail(LlmError::Timeout),
        LocalTurn::Fail(LlmError::Timeout),
        summary_turn(valid_summary()),
    ]);
    let result = compact_context(
        &provider,
        &automatic_input(standard_messages(), 90_000),
        &base_policy(),
        &CancellationToken::new(),
    )
    .await;
    assert!(matches!(
        result,
        Err(CompactionError::Provider {
            error: LlmError::Timeout,
            attempts: 3,
        })
    ));
    assert_eq!(provider.recorded_requests().len(), 3);
    assert_eq!(provider.remaining_turns(), 1);
}

#[tokio::test]
async fn deterministic_provider_failures_are_not_retried() {
    for error in [
        LlmError::Config("bad credentials configuration".into()),
        LlmError::Sse("deterministic malformed payload".into()),
        LlmError::Http {
            status: 401,
            body: "unauthorized".into(),
        },
    ] {
        let provider = LocalProvider::new(vec![
            LocalTurn::Fail(error.clone()),
            summary_turn(valid_summary()),
        ]);
        let result = compact_context(
            &provider,
            &automatic_input(standard_messages(), 90_000),
            &base_policy(),
            &CancellationToken::new(),
        )
        .await;
        assert!(matches!(
            result,
            Err(CompactionError::Provider { attempts: 1, .. })
        ));
        assert_eq!(provider.recorded_requests().len(), 1);
        assert_eq!(provider.remaining_turns(), 1);
    }
}

#[tokio::test]
async fn length_truncated_summary_is_rejected_even_when_structurally_valid() {
    let provider = LocalProvider::new(vec![summary_turn_with_stop(
        valid_summary(),
        StopReason::Length,
    )]);
    let error = compact_context(
        &provider,
        &automatic_input(standard_messages(), 90_000),
        &base_policy(),
        &CancellationToken::new(),
    )
    .await
    .unwrap_err();
    assert_eq!(
        error.validation().expect("validation error").code(),
        ValidationCode::IncompleteSummary
    );
    assert_eq!(provider.recorded_requests().len(), 1);
}

#[tokio::test]
async fn empty_short_missing_heading_and_prompt_echo_summaries_are_rejected() {
    let short = "## Goal\nx\n## Constraints & Preferences\nx\n## Progress\n### Done\nx\n### In Progress\nx\n### Blocked\nx\n## Key Decisions\nx\n## Files & Commands\nx\n## Next Steps\nx\n## Critical Context\nx";
    let missing = valid_summary().replace("## Critical Context", "## Missing Context");
    let echoed = format!(
        "{}\n\nYou are MCode's private host context compactor.",
        valid_summary()
    );
    for (summary, expected) in [
        (String::new(), ValidationCode::EmptySummary),
        (short.to_owned(), ValidationCode::SummaryTooShort),
        (missing, ValidationCode::MissingHeading),
        (echoed, ValidationCode::PromptEcho),
    ] {
        let provider = LocalProvider::new(vec![summary_turn(summary)]);
        let error = compact_context(
            &provider,
            &automatic_input(standard_messages(), 90_000),
            &base_policy(),
            &CancellationToken::new(),
        )
        .await
        .unwrap_err();
        assert_eq!(
            error.validation().expect("validation error").code(),
            expected
        );
        assert_eq!(provider.recorded_requests().len(), 1);
    }
}

#[tokio::test]
async fn degenerate_summary_is_rejected_without_retry() {
    let repeated =
        "The same generic sentence repeats without preserving any distinct factual state.";
    let summary = format!(
        "## Goal\n{repeated}\n## Constraints & Preferences\n{repeated}\n## Progress\n### Done\n{repeated}\n### In Progress\n{repeated}\n### Blocked\n{repeated}\n## Key Decisions\n{repeated}\n## Files & Commands\n{repeated}\n## Next Steps\n{repeated}\n## Critical Context\n{repeated}"
    );
    let provider = LocalProvider::new(vec![summary_turn(summary), summary_turn(valid_summary())]);
    let error = compact_context(
        &provider,
        &automatic_input(standard_messages(), 90_000),
        &base_policy(),
        &CancellationToken::new(),
    )
    .await
    .unwrap_err();
    assert_eq!(
        error.validation().expect("validation error").code(),
        ValidationCode::DegenerateSummary
    );
    assert_eq!(provider.recorded_requests().len(), 1);
}

#[tokio::test]
async fn insufficient_estimated_savings_rejects_the_candidate() {
    let summary = compact_valid_summary();
    let summary_tokens = TokenEstimator::conservative().estimate_text(&summary);
    let maximum_after = 800_u64;
    let retained_tokens = maximum_after.saturating_sub(summary_tokens);
    let old_tokens = 1_000_u64.saturating_sub(retained_tokens);
    let messages = vec![
        source(0, user("old request"), old_tokens / 2),
        source(1, assistant("old response"), old_tokens - old_tokens / 2),
        source(2, user("recent request"), retained_tokens / 2),
        source(
            3,
            assistant("recent response"),
            retained_tokens - retained_tokens / 2,
        ),
    ];
    let input = CompactionInput::new(
        "current-model",
        ContextTokenBudget::new(2_000, 1_000),
        messages,
    )
    .with_trigger_reason(TriggerReason::Manual);
    let policy = CompactionPolicy::new()
        .with_reserve_tokens(100)
        .with_keep_recent_tokens(retained_tokens)
        .with_max_summary_tokens(summary_tokens);
    let provider = LocalProvider::new(vec![summary_turn(summary)]);
    let error = compact_context(&provider, &input, &policy, &CancellationToken::new())
        .await
        .unwrap_err();
    assert_eq!(
        error.validation().expect("validation error").code(),
        ValidationCode::InsufficientSavings
    );
}

#[tokio::test]
async fn rebuilt_result_budget_is_enforced_after_summary_framing() {
    let summary = compact_valid_summary();
    let summary_tokens = TokenEstimator::conservative().estimate_text(&summary);
    let result_budget = 4_000_u64;
    let retained_tokens = result_budget.saturating_sub(summary_tokens);
    let total_tokens = 10_000_u64;
    let old_tokens = total_tokens.saturating_sub(retained_tokens);
    let messages = vec![
        source(0, user("old request"), old_tokens / 2),
        source(1, assistant("old response"), old_tokens - old_tokens / 2),
        source(2, user("recent request"), retained_tokens / 2),
        source(
            3,
            assistant("recent response"),
            retained_tokens - retained_tokens / 2,
        ),
    ];
    let input = CompactionInput::new(
        "current-model",
        ContextTokenBudget::new(5_000, total_tokens),
        messages,
    );
    let policy = CompactionPolicy::new()
        .with_reserve_tokens(1_000)
        .with_keep_recent_tokens(retained_tokens)
        .with_max_summary_tokens(summary_tokens);
    let provider = LocalProvider::new(vec![summary_turn(summary)]);
    let error = compact_context(&provider, &input, &policy, &CancellationToken::new())
        .await
        .unwrap_err();
    assert_eq!(
        error.validation().expect("validation error").code(),
        ValidationCode::ResultBudgetExceeded
    );
}

#[tokio::test]
async fn output_and_details_roundtrip_and_rebuild_validate() {
    let input = automatic_input(standard_messages(), 90_000).with_details(
        DeterministicDetails::new()
            .with_file_read("src/lib.rs")
            .with_command("cargo test -p mcode-compaction"),
    );
    let policy = base_policy();
    let provider = LocalProvider::new(vec![summary_turn(valid_summary())]);
    let output = compact_context(&provider, &input, &policy, &CancellationToken::new())
        .await
        .unwrap()
        .unwrap();

    let input_roundtrip: CompactionInput =
        serde_json::from_str(&serde_json::to_string(&input).unwrap()).unwrap();
    let policy_roundtrip: CompactionPolicy =
        serde_json::from_str(&serde_json::to_string(&policy).unwrap()).unwrap();
    let output_roundtrip: CompactionOutput =
        serde_json::from_str(&serde_json::to_string(&output).unwrap()).unwrap();
    assert_eq!(input_roundtrip, input);
    assert_eq!(policy_roundtrip, policy);
    assert_eq!(output_roundtrip, output);

    assert_eq!(
        output.plan().source_first_id(),
        Some(&MessageId::from("m0"))
    );
    assert_eq!(output.plan().source_last_id(), Some(&MessageId::from("m3")));
    validate_rebuilt_context(&input, &output).unwrap();
    let rebuilt = rebuild_context(&input, &output).unwrap();
    assert!(matches!(rebuilt.first(), Some(Message::User(_))));
    assert_eq!(rebuilt.len(), 3);
}

#[tokio::test]
async fn stale_cut_index_is_rejected_after_serde_loading() {
    let input = automatic_input(standard_messages(), 90_000);
    let provider = LocalProvider::new(vec![summary_turn(valid_summary())]);
    let output = compact_context(&provider, &input, &base_policy(), &CancellationToken::new())
        .await
        .unwrap()
        .unwrap();
    let mut value = serde_json::to_value(&output).unwrap();
    value["plan"]["cut"]["next_message_index"] = json!(999);
    let stale = serde_json::from_value(value).unwrap();
    let error = validate_rebuilt_context(&input, &stale).unwrap_err();
    assert_eq!(error.code(), ValidationCode::CutOutOfRange);
}

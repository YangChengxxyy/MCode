//! Pure threshold, turn-boundary, and split-prefix planning.

use std::collections::BTreeSet;

use mcode_core::{ContentBlock, Message, UserMessage};

use crate::estimate::{
    TokenEstimator, estimate_partial_message, estimate_previous_summary, estimate_source_message,
};
use crate::topology::analyze_tool_pairs;
use crate::types::{
    COMPACTION_SCHEMA_VERSION, CompactionCut, CompactionInput, CompactionPlan, CompactionPolicy,
    MAX_PROVIDER_ATTEMPTS, MAX_SUMMARY_CHARS, TriggerReason, ValidationCode, ValidationError,
};

/// Automatic pressure threshold is fixed to 85 percent of model context.
const TRIGGER_PERCENT: u64 = 85;
/// A successful rebuild must save at least 20 percent of current tokens.
pub(crate) const MINIMUM_SAVINGS_PERCENT: u64 = 20;
/// Default recent verbatim context is capped at twenty thousand tokens.
const DEFAULT_KEEP_RECENT_CAP: u64 = 20_000;

/// Plans a safe prefix compaction without invoking a provider.
///
/// Automatic planning returns `Ok(None)` below
/// `min(85% of context, context - reserve)`. Manual planning bypasses only
/// that pressure check; all topology, budget, and version checks remain.
///
/// # Errors
///
/// Returns [`ValidationError`] for invalid versions, policy values, tool
/// topology, token metadata, or when no safe useful cut exists.
pub fn plan_compaction(
    input: &CompactionInput,
    policy: &CompactionPolicy,
) -> Result<Option<CompactionPlan>, ValidationError> {
    validate_input_and_policy(input, policy)?;
    let metrics = PlanMetrics::new(input, policy);
    if input.trigger_reason == TriggerReason::Automatic
        && input.budget.total_tokens < metrics.trigger_threshold_tokens
    {
        return Ok(None);
    }

    let topology = analyze_tool_pairs(input.messages.iter().map(|source| &source.message))?;
    let cut = select_cut(input, &metrics, &topology.safe_boundaries)?;
    let (estimated_compacted_tokens, estimated_retained_tokens) = estimate_cut_tokens(input, &cut)?;
    let estimated_compacted_tokens = estimated_compacted_tokens
        .saturating_add(estimate_previous_summary(input.previous_summary.as_deref()));

    let plan = CompactionPlan {
        schema_version: COMPACTION_SCHEMA_VERSION,
        trigger_reason: input.trigger_reason,
        source_message_count: input.messages.len(),
        source_first_id: input.messages.first().and_then(|source| source.id.clone()),
        source_last_id: input.messages.last().and_then(|source| source.id.clone()),
        cut,
        context_window_tokens: input.budget.context_window_tokens,
        total_tokens_before: input.budget.total_tokens,
        trigger_threshold_tokens: metrics.trigger_threshold_tokens,
        keep_recent_tokens: metrics.keep_recent_tokens,
        result_budget_tokens: metrics.result_budget_tokens,
        max_summary_tokens: policy.max_summary_tokens,
        estimated_compacted_tokens,
        estimated_retained_tokens,
        estimated_fixed_overhead_tokens: metrics.fixed_overhead_tokens,
    };
    validate_plan_against_input(input, &plan)?;
    Ok(Some(plan))
}

#[derive(Debug, Clone, Copy)]
struct PlanMetrics {
    trigger_threshold_tokens: u64,
    keep_recent_tokens: u64,
    result_budget_tokens: u64,
    fixed_overhead_tokens: u64,
    hard_retained_limit: u64,
    desired_retained_limit: u64,
}

impl PlanMetrics {
    fn new(input: &CompactionInput, policy: &CompactionPolicy) -> Self {
        let context = input.budget.context_window_tokens;
        let ratio_threshold = percent_of(context, TRIGGER_PERCENT);
        let result_budget_tokens = context.saturating_sub(policy.reserve_tokens);
        let trigger_threshold_tokens = ratio_threshold.min(result_budget_tokens);
        let keep_recent_tokens = policy
            .keep_recent_tokens
            .unwrap_or_else(|| DEFAULT_KEEP_RECENT_CAP.min(context / 4));
        let known_source_tokens = input
            .messages
            .iter()
            .fold(0_u64, |total, source| {
                total.saturating_add(estimate_source_message(source))
            })
            .saturating_add(estimate_previous_summary(input.previous_summary.as_deref()));
        let fixed_overhead_tokens = input
            .budget
            .total_tokens
            .saturating_sub(known_source_tokens);

        let result_allowance = result_budget_tokens
            .saturating_sub(fixed_overhead_tokens)
            .saturating_sub(policy.max_summary_tokens);
        let maximum_after_for_savings = percent_of(
            input.budget.total_tokens,
            100_u64.saturating_sub(MINIMUM_SAVINGS_PERCENT),
        );
        let savings_allowance = maximum_after_for_savings
            .saturating_sub(fixed_overhead_tokens)
            .saturating_sub(policy.max_summary_tokens);
        let hard_retained_limit = result_allowance.min(savings_allowance);
        let desired_retained_limit = keep_recent_tokens.min(hard_retained_limit);

        Self {
            trigger_threshold_tokens,
            keep_recent_tokens,
            result_budget_tokens,
            fixed_overhead_tokens,
            hard_retained_limit,
            desired_retained_limit,
        }
    }
}

fn select_cut(
    input: &CompactionInput,
    metrics: &PlanMetrics,
    safe_boundaries: &[bool],
) -> Result<CompactionCut, ValidationError> {
    let estimates: Vec<u64> = input.messages.iter().map(estimate_source_message).collect();
    let suffix_tokens = suffix_sums(&estimates);
    let complete_boundaries: Vec<usize> = input
        .messages
        .iter()
        .enumerate()
        .filter_map(|(index, source)| {
            (index > 0
                && safe_boundaries.get(index) == Some(&true)
                && matches!(source.message, Message::User(_)))
            .then_some(index)
        })
        .collect();

    if let Some(index) = first_boundary_within(
        &complete_boundaries,
        &suffix_tokens,
        metrics.desired_retained_limit,
    )
    .or_else(|| {
        first_boundary_within(
            &complete_boundaries,
            &suffix_tokens,
            metrics.hard_retained_limit,
        )
    }) {
        return Ok(boundary_cut(input, index, false));
    }

    let latest_user_index = input
        .messages
        .iter()
        .rposition(|source| matches!(source.message, Message::User(_)))
        .unwrap_or(0);
    let split_boundaries: Vec<usize> = (1..input.messages.len())
        .filter(|index| *index > latest_user_index && safe_boundaries.get(*index) == Some(&true))
        .collect();
    if let Some(index) = first_boundary_within(
        &split_boundaries,
        &suffix_tokens,
        metrics.desired_retained_limit,
    )
    .or_else(|| {
        first_boundary_within(
            &split_boundaries,
            &suffix_tokens,
            metrics.hard_retained_limit,
        )
    }) {
        return Ok(boundary_cut(input, index, true));
    }

    if metrics.hard_retained_limit > 0 {
        if let Some(cut) = split_latest_user_text(
            input,
            latest_user_index,
            metrics.desired_retained_limit,
            metrics.hard_retained_limit,
            safe_boundaries,
        )? {
            return Ok(cut);
        }
    }

    Err(ValidationError::new(
        ValidationCode::NoSafeCut,
        "no prefix can satisfy turn, tool-pair, savings, and result-budget constraints",
    ))
}

fn first_boundary_within(boundaries: &[usize], suffix_tokens: &[u64], limit: u64) -> Option<usize> {
    boundaries
        .iter()
        .copied()
        .find(|index| suffix_tokens.get(*index).copied().unwrap_or(u64::MAX) <= limit)
}

fn boundary_cut(input: &CompactionInput, index: usize, split_turn: bool) -> CompactionCut {
    CompactionCut::MessageBoundary {
        next_message_index: index,
        next_message_id: input
            .messages
            .get(index)
            .and_then(|source| source.id.clone()),
        split_turn,
    }
}

fn split_latest_user_text(
    input: &CompactionInput,
    message_index: usize,
    desired_limit: u64,
    hard_limit: u64,
    safe_boundaries: &[bool],
) -> Result<Option<CompactionCut>, ValidationError> {
    if safe_boundaries.get(message_index) != Some(&true) {
        return Ok(None);
    }
    let Some(source) = input.messages.get(message_index) else {
        return Ok(None);
    };
    let Message::User(user) = &source.message else {
        return Ok(None);
    };

    for limit in [desired_limit, hard_limit] {
        if limit == 0 {
            continue;
        }
        for (block_index, block) in user.content.iter().enumerate() {
            let ContentBlock::Text(text) = block else {
                continue;
            };
            let char_count = text.chars().count();
            if char_count < 2 {
                continue;
            }
            let Some(char_offset) =
                find_text_offset(input, message_index, block_index, char_count, limit)?
            else {
                continue;
            };
            return Ok(Some(CompactionCut::UserTextPrefix {
                message_index,
                message_id: source.id.clone(),
                block_index,
                char_offset,
            }));
        }
    }
    Ok(None)
}

fn find_text_offset(
    input: &CompactionInput,
    message_index: usize,
    block_index: usize,
    char_count: usize,
    retained_limit: u64,
) -> Result<Option<usize>, ValidationError> {
    let mut low = 1_usize;
    let mut high = char_count.saturating_sub(1);
    let mut found = None;
    while low <= high {
        let middle = low + (high - low) / 2;
        let cut = CompactionCut::UserTextPrefix {
            message_index,
            message_id: input.messages[message_index].id.clone(),
            block_index,
            char_offset: middle,
        };
        let (_, retained) = estimate_cut_tokens(input, &cut)?;
        if retained <= retained_limit {
            found = Some(middle);
            if middle == 0 {
                break;
            }
            high = middle.saturating_sub(1);
        } else {
            low = middle.saturating_add(1);
        }
    }
    Ok(found)
}

pub(crate) fn validate_plan_against_input(
    input: &CompactionInput,
    plan: &CompactionPlan,
) -> Result<(), ValidationError> {
    if plan.schema_version != COMPACTION_SCHEMA_VERSION {
        return Err(unsupported_version("compaction plan", plan.schema_version));
    }
    if plan.source_message_count != input.messages.len()
        || plan.source_first_id != input.messages.first().and_then(|source| source.id.clone())
        || plan.source_last_id != input.messages.last().and_then(|source| source.id.clone())
    {
        return Err(ValidationError::new(
            ValidationCode::CutOutOfRange,
            "compaction plan does not describe the current source snapshot",
        ));
    }

    let synthetic_policy = CompactionPolicy {
        schema_version: COMPACTION_SCHEMA_VERSION,
        reserve_tokens: input
            .budget
            .context_window_tokens
            .saturating_sub(plan.result_budget_tokens),
        keep_recent_tokens: Some(plan.keep_recent_tokens),
        max_summary_tokens: plan.max_summary_tokens,
        max_attempts: 1,
    };
    let metrics = PlanMetrics::new(input, &synthetic_policy);
    if plan.trigger_reason != input.trigger_reason
        || plan.context_window_tokens != input.budget.context_window_tokens
        || plan.total_tokens_before != input.budget.total_tokens
        || plan.trigger_threshold_tokens != metrics.trigger_threshold_tokens
        || plan.result_budget_tokens != metrics.result_budget_tokens
        || plan.estimated_fixed_overhead_tokens != metrics.fixed_overhead_tokens
    {
        return Err(ValidationError::new(
            ValidationCode::InvalidInput,
            "compaction plan token metrics do not match the source snapshot",
        ));
    }

    let topology = analyze_tool_pairs(input.messages.iter().map(|source| &source.message))?;
    validate_cut(input, &plan.cut, &topology.safe_boundaries)?;
    let (compacted, retained) = estimate_cut_tokens(input, &plan.cut)?;
    let compacted =
        compacted.saturating_add(estimate_previous_summary(input.previous_summary.as_deref()));
    if compacted != plan.estimated_compacted_tokens || retained != plan.estimated_retained_tokens {
        return Err(ValidationError::new(
            ValidationCode::InvalidInput,
            "compaction plan estimates do not match its cut",
        ));
    }

    let (prefix, suffix) = split_messages(input, &plan.cut)?;
    analyze_tool_pairs(prefix.iter())?;
    analyze_tool_pairs(suffix.iter())?;
    Ok(())
}

fn validate_cut(
    input: &CompactionInput,
    cut: &CompactionCut,
    safe_boundaries: &[bool],
) -> Result<(), ValidationError> {
    match cut {
        CompactionCut::MessageBoundary {
            next_message_index,
            next_message_id,
            split_turn,
        } => {
            if *next_message_index == 0 || *next_message_index >= input.messages.len() {
                return Err(ValidationError::new(
                    ValidationCode::CutOutOfRange,
                    "message-boundary cut is outside the source span",
                ));
            }
            if safe_boundaries.get(*next_message_index) != Some(&true) {
                return Err(ValidationError::new(
                    ValidationCode::NoSafeCut,
                    "message-boundary cut separates a tool call from its result",
                ));
            }
            let source = &input.messages[*next_message_index];
            if next_message_id != &source.id {
                return Err(ValidationError::new(
                    ValidationCode::CutIdMismatch,
                    "message-boundary cut id does not match its source index",
                ));
            }
            let expected_split = !matches!(source.message, Message::User(_));
            if *split_turn != expected_split {
                return Err(ValidationError::new(
                    ValidationCode::InvalidInput,
                    "message-boundary cut has an inconsistent turn classification",
                ));
            }
        }
        CompactionCut::UserTextPrefix {
            message_index,
            message_id,
            block_index,
            char_offset,
        } => {
            let Some(source) = input.messages.get(*message_index) else {
                return Err(ValidationError::new(
                    ValidationCode::CutOutOfRange,
                    "split-prefix message index is outside the source span",
                ));
            };
            if message_id != &source.id {
                return Err(ValidationError::new(
                    ValidationCode::CutIdMismatch,
                    "split-prefix message id does not match its source index",
                ));
            }
            if safe_boundaries.get(*message_index) != Some(&true) {
                return Err(ValidationError::new(
                    ValidationCode::NoSafeCut,
                    "split-prefix cut starts inside an unresolved tool pair",
                ));
            }
            let Message::User(user) = &source.message else {
                return Err(ValidationError::new(
                    ValidationCode::CutOutOfRange,
                    "split-prefix cut does not reference a user message",
                ));
            };
            let Some(ContentBlock::Text(text)) = user.content.get(*block_index) else {
                return Err(ValidationError::new(
                    ValidationCode::CutOutOfRange,
                    "split-prefix cut does not reference a text block",
                ));
            };
            let char_count = text.chars().count();
            if *char_offset == 0 || *char_offset >= char_count {
                return Err(ValidationError::new(
                    ValidationCode::CutOutOfRange,
                    "split-prefix character offset must leave non-empty text on both sides",
                ));
            }
        }
    }
    Ok(())
}

pub(crate) fn split_messages(
    input: &CompactionInput,
    cut: &CompactionCut,
) -> Result<(Vec<Message>, Vec<Message>), ValidationError> {
    match cut {
        CompactionCut::MessageBoundary {
            next_message_index, ..
        } => {
            if *next_message_index == 0 || *next_message_index >= input.messages.len() {
                return Err(ValidationError::new(
                    ValidationCode::CutOutOfRange,
                    "message-boundary cut is outside the source span",
                ));
            }
            Ok((
                input.messages[..*next_message_index]
                    .iter()
                    .map(|source| source.message.clone())
                    .collect(),
                input.messages[*next_message_index..]
                    .iter()
                    .map(|source| source.message.clone())
                    .collect(),
            ))
        }
        CompactionCut::UserTextPrefix {
            message_index,
            block_index,
            char_offset,
            ..
        } => {
            let Some(source) = input.messages.get(*message_index) else {
                return Err(ValidationError::new(
                    ValidationCode::CutOutOfRange,
                    "split-prefix message index is outside the source span",
                ));
            };
            let Message::User(user) = &source.message else {
                return Err(ValidationError::new(
                    ValidationCode::CutOutOfRange,
                    "split-prefix cut does not reference a user message",
                ));
            };
            let (prefix_user, suffix_user) = split_user_message(user, *block_index, *char_offset)?;
            let mut prefix: Vec<Message> = input.messages[..*message_index]
                .iter()
                .map(|source| source.message.clone())
                .collect();
            prefix.push(Message::User(prefix_user));
            let mut suffix = vec![Message::User(suffix_user)];
            suffix.extend(
                input.messages[message_index.saturating_add(1)..]
                    .iter()
                    .map(|source| source.message.clone()),
            );
            Ok((prefix, suffix))
        }
    }
}

fn split_user_message(
    user: &UserMessage,
    block_index: usize,
    char_offset: usize,
) -> Result<(UserMessage, UserMessage), ValidationError> {
    let Some(ContentBlock::Text(text)) = user.content.get(block_index) else {
        return Err(ValidationError::new(
            ValidationCode::CutOutOfRange,
            "split-prefix cut does not reference a text block",
        ));
    };
    let Some(byte_offset) = byte_offset_at_char(text, char_offset) else {
        return Err(ValidationError::new(
            ValidationCode::CutOutOfRange,
            "split-prefix character offset is outside its text block",
        ));
    };
    let (prefix_text, suffix_text) = text.split_at(byte_offset);
    if prefix_text.is_empty() || suffix_text.is_empty() {
        return Err(ValidationError::new(
            ValidationCode::CutOutOfRange,
            "split-prefix character offset must leave non-empty text on both sides",
        ));
    }

    let mut prefix_content = user.content[..block_index].to_vec();
    prefix_content.push(ContentBlock::Text(prefix_text.to_owned()));
    let mut suffix_content = vec![ContentBlock::Text(suffix_text.to_owned())];
    suffix_content.extend_from_slice(&user.content[block_index.saturating_add(1)..]);
    Ok((
        UserMessage {
            content: prefix_content,
        },
        UserMessage {
            content: suffix_content,
        },
    ))
}

fn estimate_cut_tokens(
    input: &CompactionInput,
    cut: &CompactionCut,
) -> Result<(u64, u64), ValidationError> {
    match cut {
        CompactionCut::MessageBoundary {
            next_message_index, ..
        } => {
            if *next_message_index == 0 || *next_message_index >= input.messages.len() {
                return Err(ValidationError::new(
                    ValidationCode::CutOutOfRange,
                    "message-boundary cut is outside the source span",
                ));
            }
            let compacted = input.messages[..*next_message_index]
                .iter()
                .fold(0_u64, |total, source| {
                    total.saturating_add(estimate_source_message(source))
                });
            let retained = input.messages[*next_message_index..]
                .iter()
                .fold(0_u64, |total, source| {
                    total.saturating_add(estimate_source_message(source))
                });
            Ok((compacted, retained))
        }
        CompactionCut::UserTextPrefix { message_index, .. } => {
            let (prefix, suffix) = split_messages(input, cut)?;
            let source = &input.messages[*message_index];
            let prefix_partial = prefix.last().ok_or_else(|| {
                ValidationError::new(
                    ValidationCode::CutOutOfRange,
                    "split-prefix produced no compacted message",
                )
            })?;
            let suffix_partial = suffix.first().ok_or_else(|| {
                ValidationError::new(
                    ValidationCode::CutOutOfRange,
                    "split-prefix produced no retained message",
                )
            })?;
            let before = input.messages[..*message_index]
                .iter()
                .fold(0_u64, |total, message| {
                    total.saturating_add(estimate_source_message(message))
                });
            let after = input.messages[message_index.saturating_add(1)..]
                .iter()
                .fold(0_u64, |total, message| {
                    total.saturating_add(estimate_source_message(message))
                });
            let mut prefix_tokens = estimate_partial_message(source, prefix_partial);
            let mut suffix_tokens = estimate_partial_message(source, suffix_partial);
            if let Some(total) = source.token_count {
                prefix_tokens = prefix_tokens.min(total);
                suffix_tokens = total.saturating_sub(prefix_tokens).max(1).min(total);
            }
            Ok((
                before.saturating_add(prefix_tokens),
                suffix_tokens.saturating_add(after),
            ))
        }
    }
}

fn validate_input_and_policy(
    input: &CompactionInput,
    policy: &CompactionPolicy,
) -> Result<(), ValidationError> {
    validate_version("compaction input", input.schema_version)?;
    validate_version("context token budget", input.budget.schema_version)?;
    validate_version("compaction policy", policy.schema_version)?;
    validate_details_version("current deterministic details", &input.details)?;
    if let Some(details) = &input.previous_details {
        validate_details_version("previous deterministic details", details)?;
    }
    for (index, source) in input.messages.iter().enumerate() {
        validate_version("compaction message", source.schema_version).map_err(|_| {
            ValidationError::new(
                ValidationCode::UnsupportedVersion,
                format!("compaction message {index} uses an unsupported schema version"),
            )
        })?;
    }

    if input.model.as_str().trim().is_empty()
        || input.budget.context_window_tokens == 0
        || input.budget.total_tokens == 0
        || input.messages.is_empty()
    {
        return Err(ValidationError::new(
            ValidationCode::InvalidInput,
            "model, context window, total tokens, and source messages must be non-empty",
        ));
    }
    if input
        .previous_summary
        .as_ref()
        .is_some_and(|summary| summary.trim().is_empty())
    {
        return Err(ValidationError::new(
            ValidationCode::InvalidInput,
            "previous summary must be absent rather than empty",
        ));
    }
    let mut ids = BTreeSet::new();
    for source in &input.messages {
        // Nested conditions instead of a let chain: let chains require the
        // Rust 1.88 parser while this workspace declares an 1.85 MSRV.
        if let Some(id) = &source.id {
            if !ids.insert(id.clone()) {
                return Err(ValidationError::new(
                    ValidationCode::InvalidInput,
                    "source message ids must be unique",
                ));
            }
        }
    }

    if policy.reserve_tokens >= input.budget.context_window_tokens
        || policy.max_summary_tokens == 0
        || policy.max_summary_tokens
            >= input
                .budget
                .context_window_tokens
                .saturating_sub(policy.reserve_tokens)
        || policy.keep_recent_tokens == Some(0)
        || policy.max_attempts == 0
        || policy.max_attempts > MAX_PROVIDER_ATTEMPTS
    {
        return Err(ValidationError::new(
            ValidationCode::InvalidPolicy,
            "reserve, keep, summary, or retry values are outside bounded policy limits",
        ));
    }
    if let Some(previous_summary) = input.previous_summary.as_deref() {
        let previous_chars = previous_summary.chars().count();
        let previous_tokens = TokenEstimator::conservative().estimate_text(previous_summary);
        if previous_chars > MAX_SUMMARY_CHARS || previous_tokens > policy.max_summary_tokens {
            return Err(ValidationError::new(
                ValidationCode::SummaryBudgetExceeded,
                format!(
                    "previous summary exceeds the {}-character or {}-token input ceiling",
                    MAX_SUMMARY_CHARS, policy.max_summary_tokens
                ),
            ));
        }
    }
    Ok(())
}

fn validate_details_version(
    label: &str,
    details: &crate::types::DeterministicDetails,
) -> Result<(), ValidationError> {
    validate_version(label, details.schema_version)?;
    for operation in details
        .todo_operations
        .iter()
        .chain(&details.background_operations)
    {
        validate_version("deterministic operation", operation.schema_version)?;
    }
    Ok(())
}

fn validate_version(label: &str, version: u32) -> Result<(), ValidationError> {
    if version == COMPACTION_SCHEMA_VERSION {
        Ok(())
    } else {
        Err(unsupported_version(label, version))
    }
}

fn unsupported_version(label: &str, version: u32) -> ValidationError {
    ValidationError::new(
        ValidationCode::UnsupportedVersion,
        format!(
            "{label} schema version {version} is unsupported; expected {COMPACTION_SCHEMA_VERSION}"
        ),
    )
}

fn suffix_sums(estimates: &[u64]) -> Vec<u64> {
    let mut suffix = vec![0_u64; estimates.len().saturating_add(1)];
    for index in (0..estimates.len()).rev() {
        suffix[index] = suffix[index + 1].saturating_add(estimates[index]);
    }
    suffix
}

fn percent_of(value: u64, percent: u64) -> u64 {
    let result = u128::from(value).saturating_mul(u128::from(percent)) / 100;
    u64::try_from(result).unwrap_or(u64::MAX)
}

fn byte_offset_at_char(text: &str, char_offset: usize) -> Option<usize> {
    if char_offset == text.chars().count() {
        return Some(text.len());
    }
    text.char_indices()
        .nth(char_offset)
        .map(|(byte_offset, _)| byte_offset)
}

// Rust guideline compliant 2026-08-26.

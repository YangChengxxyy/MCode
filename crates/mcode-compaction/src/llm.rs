//! Built-in provider invocation with bounded transient retry.

use std::time::Duration;

use mcode_core::{AssistantMessage, ContentBlock, StopReason};
use mcode_llm::{CancellationToken, LlmError, Provider, Request};

use crate::details::{latest_user_request, merge_deterministic_details};
use crate::plan_compaction;
use crate::transcript::{build_summary_request, serialize_compacted_span};
use crate::types::{
    COMPACTION_SCHEMA_VERSION, CompactionDetails, CompactionError, CompactionInput,
    CompactionOutput, CompactionPolicy, MAX_PROVIDER_ATTEMPTS, ValidationCode, ValidationError,
};
use crate::validation::{
    canonicalize_files_and_commands, estimated_after_tokens, validate_canonical_summary,
    validate_rebuilt_context, validate_summary,
};

/// Initial retry delay avoids a tight loop against an unhealthy provider.
const INITIAL_RETRY_DELAY: Duration = Duration::from_millis(50);
/// Retry delay remains short because the caller owns broader scheduling.
const MAX_RETRY_DELAY: Duration = Duration::from_millis(200);

/// Compacts one immutable snapshot through the current host provider.
///
/// The function never chooses or falls back to another provider. Each attempt
/// receives a fresh private cancellation token, requests no tools, and uses the
/// model id in [`CompactionInput`]. The current `mcode-llm::Request` has no
/// output-token field, so the prompt states the ceiling and post-validation
/// enforces it before any [`CompactionOutput`] is returned.
///
/// Automatic calls return `Ok(None)` below the fixed pressure threshold.
/// Caller state is borrowed immutably and is therefore unchanged on every
/// failure or cancellation path.
///
/// # Errors
///
/// Returns [`CompactionError`] for invalid source topology or policy, provider
/// failure, cancellation, malformed summaries, insufficient savings, or an
/// over-budget rebuilt context.
pub async fn compact_context(
    provider: &dyn Provider,
    input: &CompactionInput,
    policy: &CompactionPolicy,
    caller_cancel: &CancellationToken,
) -> Result<Option<CompactionOutput>, CompactionError> {
    if caller_cancel.is_cancelled() {
        return Err(CompactionError::Cancelled { attempts: 0 });
    }
    let Some(plan) = plan_compaction(input, policy)? else {
        return Ok(None);
    };
    let serialized = serialize_compacted_span(input, &plan)?;
    let request = build_summary_request(input, &plan, &serialized.text);
    debug_assert!(request.tools.is_empty());
    debug_assert!(request.thinking.is_none());

    let attempt_limit = policy.max_attempts.min(MAX_PROVIDER_ATTEMPTS);
    let mut attempts = 0_u32;
    let assistant = loop {
        attempts = attempts.saturating_add(1);
        match run_attempt(provider, &request, caller_cancel).await {
            Ok(message) => break message,
            Err(LlmError::Cancelled) => {
                return Err(CompactionError::Cancelled { attempts });
            }
            Err(error) if is_transient(&error) && attempts < attempt_limit => {
                wait_for_retry(attempts, caller_cancel).await?;
            }
            Err(error) => return Err(CompactionError::Provider { error, attempts }),
        }
    };

    let raw_summary = extract_summary(&assistant)?;
    validate_summary(&raw_summary, plan.max_summary_tokens)?;
    let deterministic = merge_deterministic_details(input);
    let summary = canonicalize_files_and_commands(&raw_summary, &deterministic)?;
    validate_canonical_summary(&summary, &deterministic, plan.max_summary_tokens)?;
    let estimated_tokens_after = estimated_after_tokens(&plan, &summary);
    let details = CompactionDetails {
        schema_version: COMPACTION_SCHEMA_VERSION,
        provider_id: provider.id().to_owned(),
        model: input.model.clone(),
        attempts,
        total_tokens_before: input.budget.total_tokens,
        estimated_tokens_after,
        latest_user_request: latest_user_request(input),
        deterministic,
        tool_result_truncations: serialized.truncations,
        transcript_omitted_messages: serialized.omitted_messages,
    };
    let output = CompactionOutput {
        schema_version: COMPACTION_SCHEMA_VERSION,
        summary,
        plan,
        details,
    };
    validate_rebuilt_context(input, &output)?;
    Ok(Some(output))
}

async fn run_attempt(
    provider: &dyn Provider,
    request: &Request,
    caller_cancel: &CancellationToken,
) -> Result<AssistantMessage, LlmError> {
    let attempt_cancel = CancellationToken::new();
    let stream = tokio::select! {
        biased;
        _ = caller_cancel.cancelled() => {
            attempt_cancel.cancel();
            return Err(LlmError::Cancelled);
        }
        result = provider.stream(request, attempt_cancel.clone()) => result?,
    };
    tokio::select! {
        biased;
        _ = caller_cancel.cancelled() => {
            attempt_cancel.cancel();
            Err(LlmError::Cancelled)
        }
        result = stream.into_final_message() => result,
    }
}

async fn wait_for_retry(
    completed_attempts: u32,
    caller_cancel: &CancellationToken,
) -> Result<(), CompactionError> {
    let exponent = completed_attempts.saturating_sub(1).min(2);
    let multiplier = 1_u32.checked_shl(exponent).unwrap_or(u32::MAX);
    let delay = INITIAL_RETRY_DELAY
        .checked_mul(multiplier)
        .unwrap_or(MAX_RETRY_DELAY)
        .min(MAX_RETRY_DELAY);
    tokio::select! {
        biased;
        _ = caller_cancel.cancelled() => Err(CompactionError::Cancelled {
            attempts: completed_attempts,
        }),
        _ = tokio::time::sleep(delay) => Ok(()),
    }
}

fn extract_summary(message: &AssistantMessage) -> Result<String, ValidationError> {
    if message
        .blocks
        .iter()
        .any(|block| matches!(block, ContentBlock::ToolCall(_)))
        || message.stop_reason == StopReason::ToolUse
    {
        return Err(ValidationError::new(
            ValidationCode::UnexpectedToolCall,
            "summary response attempted to call a tool despite tools being disabled",
        ));
    }
    match message.stop_reason {
        StopReason::Stop => {}
        StopReason::Length => {
            return Err(ValidationError::new(
                ValidationCode::IncompleteSummary,
                "summary response reached its output length limit",
            ));
        }
        StopReason::Error => {
            return Err(ValidationError::new(
                ValidationCode::IncompleteSummary,
                "summary response ended with an error stop reason",
            ));
        }
        StopReason::ToolUse => {
            return Err(ValidationError::new(
                ValidationCode::UnexpectedToolCall,
                "summary response ended with a tool-use stop reason",
            ));
        }
    }
    let text = message
        .blocks
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.as_str()),
            ContentBlock::Thinking(_) | ContentBlock::Image(_) | ContentBlock::ToolCall(_) => None,
        })
        .collect::<Vec<_>>()
        .join("");
    if text.trim().is_empty() {
        return Err(ValidationError::new(
            ValidationCode::EmptySummary,
            "summary response contains no text blocks",
        ));
    }
    Ok(text.trim().to_owned())
}

fn is_transient(error: &LlmError) -> bool {
    match error {
        LlmError::Transport(_) | LlmError::Timeout => true,
        LlmError::Http { status, .. } => {
            *status == 408 || *status == 429 || (500..=599).contains(status)
        }
        LlmError::Sse(_) | LlmError::Cancelled | LlmError::Config(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transient_classification_is_conservative() {
        assert!(is_transient(&LlmError::Transport("reset".into())));
        assert!(is_transient(&LlmError::Timeout));
        assert!(is_transient(&LlmError::Http {
            status: 429,
            body: String::new(),
        }));
        assert!(is_transient(&LlmError::Http {
            status: 503,
            body: String::new(),
        }));
        assert!(!is_transient(&LlmError::Http {
            status: 401,
            body: String::new(),
        }));
        assert!(!is_transient(&LlmError::Http {
            status: 0,
            body: String::new(),
        }));
        assert!(!is_transient(&LlmError::Sse("bad json".into())));
        assert!(!is_transient(&LlmError::Config("missing key".into())));
    }
}

// Rust guideline compliant 2026-08-26.

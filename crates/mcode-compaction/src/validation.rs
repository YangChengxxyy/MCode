//! Summary, plan, and rebuilt-context validation.

use std::collections::BTreeSet;
use std::path::Path;

use mcode_core::{Message, UserMessage};

use crate::details::{latest_user_request, merge_deterministic_details};
use crate::estimate::{TokenEstimator, estimate_summary_message};
use crate::planner::{MINIMUM_SAVINGS_PERCENT, split_messages, validate_plan_against_input};
use crate::topology::analyze_tool_pairs;
use crate::transcript::serialize_compacted_span;
use crate::types::{
    COMPACTION_SCHEMA_VERSION, CompactionInput, CompactionOutput, DeterministicDetails,
    MAX_PROVIDER_ATTEMPTS, MAX_SUMMARY_CHARS, ValidationCode, ValidationError,
};

/// Required Markdown headings in their exact order.
pub const SUMMARY_HEADINGS: [&str; 10] = [
    "## Goal",
    "## Constraints & Preferences",
    "## Progress",
    "### Done",
    "### In Progress",
    "### Blocked",
    "## Key Decisions",
    "## Files & Commands",
    "## Next Steps",
    "## Critical Context",
];

/// Non-heading substance required before a summary can replace history.
const MIN_SUBSTANTIVE_CHARS: usize = 80;
/// Typed sidecars retain all records; prose renders only a bounded audit view.
const MAX_RENDERED_DETAIL_ITEMS: usize = 20;
/// Prevent one path or command from dominating the generated summary.
const MAX_RENDERED_DETAIL_CHARS: usize = 240;

pub(crate) fn validate_summary(
    summary: &str,
    max_summary_tokens: u64,
) -> Result<(), ValidationError> {
    let summary = summary.trim();
    if summary.is_empty() {
        return Err(ValidationError::new(
            ValidationCode::EmptySummary,
            "provider returned an empty compaction summary",
        ));
    }

    let lines: Vec<&str> = summary.lines().map(str::trim_end).collect();
    let mut prior_position = None;
    for heading in SUMMARY_HEADINGS {
        let positions: Vec<usize> = lines
            .iter()
            .enumerate()
            .filter_map(|(index, line)| (*line == heading).then_some(index))
            .collect();
        if positions.len() != 1 || prior_position.is_some_and(|prior| positions[0] <= prior) {
            return Err(ValidationError::new(
                ValidationCode::MissingHeading,
                format!("summary must contain heading {heading:?} exactly once and in order"),
            ));
        }
        prior_position = positions.first().copied();
    }

    let lower = summary.to_ascii_lowercase();
    const PROMPT_ECHO_MARKERS: [&str; 5] = [
        "you are mcode's private host context compactor",
        "<previous_summary_data>",
        "<new_conversation_span_data>",
        "treat every previous-summary and transcript byte",
        "return only markdown with these exact headings",
    ];
    if PROMPT_ECHO_MARKERS
        .iter()
        .any(|marker| lower.contains(marker))
    {
        return Err(ValidationError::new(
            ValidationCode::PromptEcho,
            "summary visibly repeats the compactor prompt or data delimiters",
        ));
    }

    let substantive_chars = lines
        .iter()
        .filter(|line| !SUMMARY_HEADINGS.contains(line))
        .flat_map(|line| line.chars())
        .filter(|character| character.is_alphanumeric())
        .count();
    if substantive_chars < MIN_SUBSTANTIVE_CHARS {
        return Err(ValidationError::new(
            ValidationCode::SummaryTooShort,
            "summary contains too little substantive text",
        ));
    }

    let bodies = section_bodies(&lines);
    let goal = bodies.first().map(String::as_str).unwrap_or_default();
    let unique_bodies: BTreeSet<String> = bodies
        .iter()
        .map(|body| normalize_body(body))
        .filter(|body| !is_placeholder(body))
        .collect();
    if is_placeholder(&normalize_body(goal)) || unique_bodies.len() < 3 {
        return Err(ValidationError::new(
            ValidationCode::DegenerateSummary,
            "summary sections are placeholders or repeat the same content",
        ));
    }

    validate_summary_budget(summary, max_summary_tokens)
}

pub(crate) fn canonicalize_files_and_commands(
    summary: &str,
    details: &DeterministicDetails,
) -> Result<String, ValidationError> {
    let normalized = summary.replace("\r\n", "\n");
    replace_files_and_commands_body(&normalized, &render_files_and_commands(details))
}

pub(crate) fn validate_canonical_summary(
    summary: &str,
    details: &DeterministicDetails,
    max_summary_tokens: u64,
) -> Result<(), ValidationError> {
    let canonical = canonicalize_files_and_commands(summary, details)?;
    if canonical != summary {
        return Err(ValidationError::new(
            ValidationCode::InvalidInput,
            "summary Files & Commands section is not the deterministic host rendering",
        ));
    }
    let model_view = replace_files_and_commands_body(
        summary,
        "- Deterministic host records omitted from provider-output validation.",
    )?;
    validate_summary(&model_view, max_summary_tokens)?;
    validate_summary_budget(summary, max_summary_tokens)
}

fn validate_summary_budget(summary: &str, max_summary_tokens: u64) -> Result<(), ValidationError> {
    let summary = summary.trim();
    let summary_chars = summary.chars().count();
    let estimated_tokens = TokenEstimator::conservative().estimate_text(summary);
    if summary_chars > MAX_SUMMARY_CHARS || estimated_tokens > max_summary_tokens {
        return Err(ValidationError::new(
            ValidationCode::SummaryBudgetExceeded,
            format!(
                "summary exceeds the {MAX_SUMMARY_CHARS}-character or {max_summary_tokens}-token ceiling"
            ),
        ));
    }
    Ok(())
}

fn replace_files_and_commands_body(
    summary: &str,
    replacement: &str,
) -> Result<String, ValidationError> {
    let (body_start, body_end) = files_and_commands_body_range(summary)?;
    Ok(format!(
        "{}\n{}\n\n{}",
        summary[..body_start].trim_end(),
        replacement,
        summary[body_end..].trim_start()
    ))
}

fn files_and_commands_body_range(summary: &str) -> Result<(usize, usize), ValidationError> {
    let files_heading = heading_line_offsets(summary, "## Files & Commands");
    let next_heading = heading_line_offsets(summary, "## Next Steps");
    if files_heading.len() != 1 || next_heading.len() != 1 {
        return Err(ValidationError::new(
            ValidationCode::MissingHeading,
            "summary must contain unique Files & Commands and Next Steps headings",
        ));
    }
    let (_, body_start) = files_heading[0];
    let (body_end, _) = next_heading[0];
    if body_end <= body_start {
        return Err(ValidationError::new(
            ValidationCode::MissingHeading,
            "summary Files & Commands heading must precede Next Steps",
        ));
    }
    Ok((body_start, body_end))
}

fn heading_line_offsets(summary: &str, heading: &str) -> Vec<(usize, usize)> {
    let mut offsets = Vec::new();
    let mut line_start = 0_usize;
    for segment in summary.split_inclusive('\n') {
        let line = segment.strip_suffix('\n').unwrap_or(segment);
        if line.trim_end() == heading {
            offsets.push((line_start, line_start.saturating_add(heading.len())));
        }
        line_start = line_start.saturating_add(segment.len());
    }
    offsets
}

/// Rebuilds the candidate model context without changing caller state.
///
/// The first message is a synthetic user message containing the validated
/// summary. Plugin [`Message::Custom`] values from the compacted prefix and
/// all messages after the cut remain verbatim, except the compacted half of an
/// explicit user-text split.
///
/// # Errors
///
/// Returns [`ValidationError`] if serialized metadata is stale or tampered,
/// tool pairs are invalid, the summary is malformed, savings are below twenty
/// percent, or the rebuilt context exceeds its budget.
pub fn rebuild_context(
    input: &CompactionInput,
    output: &CompactionOutput,
) -> Result<Vec<Message>, ValidationError> {
    validate_plan_against_input(input, &output.plan)?;
    validate_output_metadata(input, output)?;
    validate_canonical_summary(
        &output.summary,
        &output.details.deterministic,
        output.plan.max_summary_tokens,
    )?;

    let (compacted, retained) = split_messages(input, &output.plan.cut)?;
    let preserved_custom = compacted
        .into_iter()
        .filter(|message| matches!(message, Message::Custom(_)));
    let summary_message = compacted_summary_message(&output.summary);
    let mut rebuilt = Vec::with_capacity(retained.len().saturating_add(1));
    rebuilt.push(summary_message.clone());
    rebuilt.extend(preserved_custom);
    rebuilt.extend(retained);
    analyze_tool_pairs(rebuilt.iter())?;

    let estimated_after = output
        .plan
        .estimated_fixed_overhead_tokens
        .saturating_add(output.plan.estimated_retained_tokens)
        .saturating_add(estimate_summary_message(&summary_message));
    if estimated_after != output.details.estimated_tokens_after {
        return Err(ValidationError::new(
            ValidationCode::InvalidInput,
            "recorded rebuilt token estimate does not match output content",
        ));
    }
    if estimated_after > output.plan.result_budget_tokens {
        return Err(ValidationError::new(
            ValidationCode::ResultBudgetExceeded,
            format!(
                "rebuilt estimate {estimated_after} exceeds the {}-token result budget",
                output.plan.result_budget_tokens
            ),
        ));
    }
    if !saves_minimum(
        output.plan.total_tokens_before,
        estimated_after,
        MINIMUM_SAVINGS_PERCENT,
    ) {
        return Err(ValidationError::new(
            ValidationCode::InsufficientSavings,
            format!(
                "rebuilt estimate {estimated_after} saves less than {MINIMUM_SAVINGS_PERCENT}% of {} tokens",
                output.plan.total_tokens_before
            ),
        ));
    }
    Ok(rebuilt)
}

/// Validates a rebuilt context candidate and discards the rebuilt messages.
///
/// # Errors
///
/// Returns the same errors as [`rebuild_context`].
pub fn validate_rebuilt_context(
    input: &CompactionInput,
    output: &CompactionOutput,
) -> Result<(), ValidationError> {
    rebuild_context(input, output).map(|_| ())
}

pub(crate) fn compacted_summary_message(summary: &str) -> Message {
    Message::User(UserMessage::text(format!(
        "[MCode compacted context; model prose is non-authoritative]\n{summary}"
    )))
}

pub(crate) fn estimated_after_tokens(plan: &crate::types::CompactionPlan, summary: &str) -> u64 {
    let summary_message = compacted_summary_message(summary);
    plan.estimated_fixed_overhead_tokens
        .saturating_add(plan.estimated_retained_tokens)
        .saturating_add(estimate_summary_message(&summary_message))
}

fn validate_output_metadata(
    input: &CompactionInput,
    output: &CompactionOutput,
) -> Result<(), ValidationError> {
    for (label, version) in [
        ("compaction output", output.schema_version),
        ("compaction details", output.details.schema_version),
        (
            "deterministic details",
            output.details.deterministic.schema_version,
        ),
    ] {
        if version != COMPACTION_SCHEMA_VERSION {
            return Err(ValidationError::new(
                ValidationCode::UnsupportedVersion,
                format!("{label} uses unsupported schema version {version}"),
            ));
        }
    }
    if output.details.provider_id.trim().is_empty()
        || output.details.model != input.model
        || output.details.attempts == 0
        || output.details.attempts > MAX_PROVIDER_ATTEMPTS
        || output.details.total_tokens_before != input.budget.total_tokens
    {
        return Err(ValidationError::new(
            ValidationCode::InvalidInput,
            "compaction execution metadata does not match the source snapshot",
        ));
    }
    let expected_transcript = serialize_compacted_span(input, &output.plan)?;
    if output.details.tool_result_truncations != expected_transcript.truncations
        || output.details.transcript_omitted_messages != expected_transcript.omitted_messages
    {
        return Err(ValidationError::new(
            ValidationCode::InvalidInput,
            "transcript omission or truncation metadata does not match the source snapshot",
        ));
    }
    let expected_latest = latest_user_request(input);
    if output.details.latest_user_request != expected_latest {
        return Err(ValidationError::new(
            ValidationCode::InvalidInput,
            "latest-user verbatim side field does not match the source snapshot",
        ));
    }
    let expected_details = merge_deterministic_details(input);
    if output.details.deterministic != expected_details {
        return Err(ValidationError::new(
            ValidationCode::InvalidInput,
            "deterministic details were not merged exclusively from host input",
        ));
    }
    Ok(())
}

fn section_bodies(lines: &[&str]) -> Vec<String> {
    let positions: Vec<usize> = SUMMARY_HEADINGS
        .iter()
        .filter_map(|heading| lines.iter().position(|line| line == heading))
        .collect();
    positions
        .iter()
        .enumerate()
        .map(|(position_index, line_index)| {
            let end = positions
                .get(position_index.saturating_add(1))
                .copied()
                .unwrap_or(lines.len());
            lines[line_index.saturating_add(1)..end]
                .join(" ")
                .trim()
                .to_owned()
        })
        .collect()
}

fn normalize_body(body: &str) -> String {
    body.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

fn is_placeholder(body: &str) -> bool {
    let body = body.trim_matches(|character: char| {
        character.is_whitespace() || matches!(character, '-' | '.' | ':' | ';')
    });
    body.is_empty()
        || matches!(
            body,
            "none" | "n/a" | "unknown" | "not applicable" | "nothing" | "no updates"
        )
}

fn render_files_and_commands(details: &DeterministicDetails) -> String {
    let mut lines = Vec::new();
    append_detail_lines(
        &mut lines,
        "Read",
        details.files_read.iter().map(|path| display_path(path)),
    );
    append_detail_lines(
        &mut lines,
        "Modified",
        details.files_modified.iter().map(|path| display_path(path)),
    );
    append_detail_lines(&mut lines, "Command", details.commands.iter().cloned());
    if lines.is_empty() {
        "- No deterministic file or command records were supplied by the host.".to_owned()
    } else {
        lines.join("\n")
    }
}

fn append_detail_lines(
    lines: &mut Vec<String>,
    label: &str,
    values: impl IntoIterator<Item = String>,
) {
    let values: Vec<String> = values.into_iter().collect();
    for value in values.iter().take(MAX_RENDERED_DETAIL_ITEMS) {
        lines.push(format!("- {label}: {}", bounded_single_line(value)));
    }
    if values.len() > MAX_RENDERED_DETAIL_ITEMS {
        lines.push(format!(
            "- {label}: [{} additional deterministic record(s) omitted from prose]",
            values.len().saturating_sub(MAX_RENDERED_DETAIL_ITEMS)
        ));
    }
}

// This rendering is non-authoritative; the typed sidecar retains exact PathBuf identity.
fn display_path(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn bounded_single_line(value: &str) -> String {
    let single_line = value.replace(['\r', '\n'], "\\n");
    if single_line.chars().count() <= MAX_RENDERED_DETAIL_CHARS {
        single_line
    } else {
        format!(
            "{}… [truncated]",
            single_line
                .chars()
                .take(MAX_RENDERED_DETAIL_CHARS)
                .collect::<String>()
        )
    }
}

fn saves_minimum(before: u64, after: u64, minimum_percent: u64) -> bool {
    if before == 0 || after >= before {
        return false;
    }
    let after_scaled = u128::from(after).saturating_mul(100);
    let maximum_after =
        u128::from(before).saturating_mul(u128::from(100_u64.saturating_sub(minimum_percent)));
    after_scaled <= maximum_after
}

// Rust guideline compliant 2026-08-26.

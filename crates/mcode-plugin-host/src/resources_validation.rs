//! Host-side Resources task semantic boundaries.

// Rust guideline compliant 2026-08-31.

use std::collections::BTreeSet;

use mcode_plugin_api::{
    ResourcesCatalogEntry, ResourcesCatalogResult, ResourcesPromptParam, ResourcesTaskProgress,
    ResourcesTaskRequest, ResourcesTaskResult, validate_resources_progress,
    validate_resources_result,
};

const MAX_AGGREGATE_CHARGE: u64 = 1_048_576;
const MAX_CONTRIBUTIONS_CHARGE: u64 = 262_144;
const MAX_PROMPT_TEXT_BYTES: u64 = 262_144;
const MAX_CATALOG_LIMIT: u16 = 128;
const MAX_READ_BYTES: u32 = 65_536;
const MAX_ARGUMENTS: usize = 64;
const MAX_ARGUMENT_VALUE_BYTES: usize = 65_536;
const MAX_PROMPT_PARAMS: usize = 64;
const MAX_PROMPT_MESSAGES: usize = 16;
const MAX_PROMPT_MESSAGE_BYTES: usize = 65_536;
const MAX_CONTRIBUTIONS: usize = 64;
const WIT_BOOLEAN_CHARGE: u64 = 1;
const WIT_U16_CHARGE: u64 = 2;
const WIT_U32_CHARGE: u64 = 4;
const WIT_U64_CHARGE: u64 = 8;
const WIT_LIST_CHARGE: u64 = 4;
const WIT_DISCRIMINANT_CHARGE: u64 = 4;

/// Reports a Resources semantic boundary rejection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub(crate) enum ResourcesValidationError {
    /// A value violated its grammar, relation, ordering, or minimum.
    #[error("Resources value is invalid")]
    InvalidArgument,
    /// A value, count, aggregate, or checked calculation exceeded its limit.
    #[error("Resources value exceeds its limit")]
    Limit,
}

type ValidationResult<T = u64> = Result<T, ResourcesValidationError>;

/// Validates one Resources request before guest operation allocation.
///
/// The returned value is the exact checked WIT logical charge.
///
/// # Errors
///
/// Returns [`ResourcesValidationError`] for invalid grammar, order, bounds,
/// aggregate charge, or arithmetic overflow.
pub(crate) fn validate_resources_request(request: &ResourcesTaskRequest) -> ValidationResult {
    let mut charge = LogicalCharge::new(MAX_AGGREGATE_CHARGE);
    charge.add(WIT_DISCRIMINANT_CHARGE)?;
    match request {
        ResourcesTaskRequest::Catalog(request) => {
            charge.add(WIT_U32_CHARGE)?;
            charge.add(WIT_U16_CHARGE)?;
            if request.limit == 0 {
                return Err(ResourcesValidationError::InvalidArgument);
            }
            if request.limit > MAX_CATALOG_LIMIT {
                return Err(ResourcesValidationError::Limit);
            }
        }
        ResourcesTaskRequest::Read(request) => {
            validate_local_id(&request.id, 128)?;
            charge.string(&request.id)?;
            charge.add(WIT_U64_CHARGE)?;
            charge.add(WIT_U32_CHARGE)?;
            if request.max_bytes < 4 {
                return Err(ResourcesValidationError::InvalidArgument);
            }
            if request.max_bytes > MAX_READ_BYTES {
                return Err(ResourcesValidationError::Limit);
            }
        }
        ResourcesTaskRequest::RenderPrompt(request) => {
            validate_local_id(&request.id, 128)?;
            charge.string(&request.id)?;
            validate_count(request.args.len(), MAX_ARGUMENTS)?;
            charge.add(WIT_LIST_CHARGE)?;
            validate_strict_ids(
                request.args.iter().map(|argument| argument.name.as_str()),
                64,
            )?;
            for argument in &request.args {
                validate_safe(&argument.value, MAX_ARGUMENT_VALUE_BYTES, false)?;
                charge.string(&argument.name)?;
                charge.string(&argument.value)?;
            }
        }
        ResourcesTaskRequest::Contributions => {}
    }
    Ok(charge.value())
}

/// Validates one Resources progress body against its request case.
///
/// The returned value is the exact checked WIT logical charge.
///
/// # Errors
///
/// Returns [`ResourcesValidationError::InvalidArgument`] when progress is
/// crossed with another request case.
pub(crate) fn validate_resources_progress_body(
    request: &ResourcesTaskRequest,
    progress: ResourcesTaskProgress,
) -> ValidationResult {
    validate_resources_request(request)?;
    validate_resources_progress(request, progress)
        .map_err(|_| ResourcesValidationError::InvalidArgument)?;
    Ok(4)
}

/// Validates one Resources result before retaining guest-derived data.
///
/// The returned value is the exact checked WIT logical charge. Relations that
/// require the immutable embedded catalog remain the operation owner's gate.
///
/// # Errors
///
/// Returns [`ResourcesValidationError`] for a crossed case, invalid grammar,
/// order, pagination, text, bounds, aggregate charge, or arithmetic overflow.
pub(crate) fn validate_resources_result_body(
    request: &ResourcesTaskRequest,
    result: &ResourcesTaskResult,
) -> ValidationResult {
    validate_resources_request(request)?;
    validate_resources_result(request, result)
        .map_err(|_| ResourcesValidationError::InvalidArgument)?;

    let mut charge = LogicalCharge::new(MAX_AGGREGATE_CHARGE);
    charge.add(WIT_DISCRIMINANT_CHARGE)?;
    match (request, result) {
        (ResourcesTaskRequest::Catalog(request), ResourcesTaskResult::Catalog(result)) => {
            validate_catalog_result(request.offset, request.limit, result, &mut charge)?
        }
        (ResourcesTaskRequest::Read(request), ResourcesTaskResult::Read(result)) => {
            validate_safe(&result.text, request.max_bytes as usize, false)?;
            charge.string(&result.text)?;
            charge.add(WIT_DISCRIMINANT_CHARGE)?;
            if let Some(next_offset) = result.next_offset {
                charge.add(WIT_U64_CHARGE)?;
                if result.text.is_empty() {
                    return Err(ResourcesValidationError::InvalidArgument);
                }
                let returned = checked_len(result.text.len())?;
                if request.offset.checked_add(returned) != Some(next_offset) {
                    return Err(ResourcesValidationError::InvalidArgument);
                }
            }
        }
        (ResourcesTaskRequest::RenderPrompt(request), ResourcesTaskResult::Prompt(result)) => {
            validate_prompt_result(&request.id, result, &mut charge)?
        }
        (ResourcesTaskRequest::Contributions, ResourcesTaskResult::Contributions(result)) => {
            validate_contributions_result(result, &mut charge)?
        }
        _ => return Err(ResourcesValidationError::InvalidArgument),
    }
    Ok(charge.value())
}

fn validate_catalog_result(
    offset: u32,
    limit: u16,
    result: &ResourcesCatalogResult,
    charge: &mut LogicalCharge,
) -> ValidationResult<()> {
    if result.items.len() > usize::from(limit) {
        return Err(ResourcesValidationError::Limit);
    }
    charge.add(WIT_LIST_CHARGE)?;

    let mut previous_key: Option<(u8, &str)> = None;
    let mut ids = BTreeSet::new();
    for entry in &result.items {
        charge.add(WIT_DISCRIMINANT_CHARGE)?;
        let (tag, id) = match entry {
            ResourcesCatalogEntry::Resource(resource) => {
                validate_local_id(&resource.id, 128)?;
                validate_label(&resource.title, 256)?;
                if resource
                    .size_hint
                    .is_some_and(|size| size > i64::MAX as u64)
                {
                    return Err(ResourcesValidationError::Limit);
                }
                charge.string(&resource.id)?;
                charge.string(&resource.title)?;
                charge.add(WIT_DISCRIMINANT_CHARGE)?;
                charge.add(WIT_DISCRIMINANT_CHARGE)?;
                if resource.size_hint.is_some() {
                    charge.add(WIT_U64_CHARGE)?;
                }
                (0, resource.id.as_str())
            }
            ResourcesCatalogEntry::Prompt(prompt) => {
                validate_local_id(&prompt.id, 128)?;
                validate_label(&prompt.title, 256)?;
                validate_prompt_params(&prompt.params, charge)?;
                charge.string(&prompt.id)?;
                charge.string(&prompt.title)?;
                (1, prompt.id.as_str())
            }
        };
        if previous_key.is_some_and(|previous| previous >= (tag, id)) || !ids.insert(id) {
            return Err(ResourcesValidationError::InvalidArgument);
        }
        previous_key = Some((tag, id));
    }

    charge.add(WIT_DISCRIMINANT_CHARGE)?;
    if let Some(next_offset) = result.next_offset {
        charge.add(WIT_U32_CHARGE)?;
        let item_count =
            u32::try_from(result.items.len()).map_err(|_| ResourcesValidationError::Limit)?;
        if item_count == 0 || offset.checked_add(item_count) != Some(next_offset) {
            return Err(ResourcesValidationError::InvalidArgument);
        }
    }
    Ok(())
}

fn validate_prompt_params(
    params: &[ResourcesPromptParam],
    charge: &mut LogicalCharge,
) -> ValidationResult<()> {
    validate_count(params.len(), MAX_PROMPT_PARAMS)?;
    validate_strict_ids(params.iter().map(|parameter| parameter.name.as_str()), 64)?;
    charge.add(WIT_LIST_CHARGE)?;
    for parameter in params {
        validate_label(&parameter.label, 128)?;
        charge.string(&parameter.name)?;
        charge.string(&parameter.label)?;
        charge.add(WIT_BOOLEAN_CHARGE)?;
    }
    Ok(())
}

fn validate_prompt_result(
    request_id: &str,
    result: &mcode_plugin_api::ResourcesPromptResult,
    charge: &mut LogicalCharge,
) -> ValidationResult<()> {
    if result.id != request_id {
        return Err(ResourcesValidationError::InvalidArgument);
    }
    validate_local_id(&result.id, 128)?;
    validate_count(result.messages.len(), MAX_PROMPT_MESSAGES)?;
    charge.string(&result.id)?;
    charge.add(WIT_LIST_CHARGE)?;

    let mut text_bytes = 0_u64;
    for message in &result.messages {
        validate_safe(&message.text, MAX_PROMPT_MESSAGE_BYTES, false)?;
        text_bytes = text_bytes
            .checked_add(checked_len(message.text.len())?)
            .ok_or(ResourcesValidationError::Limit)?;
        if text_bytes > MAX_PROMPT_TEXT_BYTES {
            return Err(ResourcesValidationError::Limit);
        }
        charge.add(WIT_DISCRIMINANT_CHARGE)?;
        charge.string(&message.text)?;
    }
    Ok(())
}

fn validate_contributions_result(
    result: &mcode_plugin_api::ResourcesContributionsResult,
    charge: &mut LogicalCharge,
) -> ValidationResult<()> {
    validate_count(result.items.len(), MAX_CONTRIBUTIONS)?;
    validate_strict_ids(
        result
            .items
            .iter()
            .map(|contribution| contribution.id.as_str()),
        128,
    )?;
    charge.add(WIT_LIST_CHARGE)?;
    for contribution in &result.items {
        charge.string(&contribution.id)?;
        charge.add(WIT_DISCRIMINANT_CHARGE)?;
    }
    if charge.value() > MAX_CONTRIBUTIONS_CHARGE {
        return Err(ResourcesValidationError::Limit);
    }
    Ok(())
}

fn validate_count(count: usize, maximum: usize) -> ValidationResult<()> {
    if count > maximum {
        return Err(ResourcesValidationError::Limit);
    }
    Ok(())
}

fn validate_strict_ids<'a>(
    values: impl Iterator<Item = &'a str>,
    maximum_bytes: usize,
) -> ValidationResult<()> {
    let mut previous = None;
    for value in values {
        validate_local_id(value, maximum_bytes)?;
        if previous.is_some_and(|prior| prior >= value) {
            return Err(ResourcesValidationError::InvalidArgument);
        }
        previous = Some(value);
    }
    Ok(())
}

fn validate_local_id(value: &str, maximum_bytes: usize) -> ValidationResult<()> {
    if value.len() > maximum_bytes {
        return Err(ResourcesValidationError::Limit);
    }
    let mut bytes = value.bytes();
    if !bytes.next().is_some_and(|byte| byte.is_ascii_lowercase())
        || !bytes.all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Err(ResourcesValidationError::InvalidArgument);
    }
    Ok(())
}

fn validate_label(value: &str, maximum_bytes: usize) -> ValidationResult<()> {
    if value.is_empty() || value.contains(['\t', '\n']) {
        return Err(ResourcesValidationError::InvalidArgument);
    }
    validate_safe(value, maximum_bytes, true)
}

fn validate_safe(
    value: &str,
    maximum_bytes: usize,
    require_nonempty: bool,
) -> ValidationResult<()> {
    if value.len() > maximum_bytes {
        return Err(ResourcesValidationError::Limit);
    }
    if require_nonempty && value.is_empty() {
        return Err(ResourcesValidationError::InvalidArgument);
    }
    if value.chars().any(is_unsafe_character) {
        return Err(ResourcesValidationError::InvalidArgument);
    }
    Ok(())
}

const fn is_unsafe_character(character: char) -> bool {
    matches!(
        character,
        '\u{0000}'..='\u{0008}'
            | '\u{000b}'..='\u{001f}'
            | '\u{007f}'..='\u{009f}'
            | '\u{061c}'
            | '\u{200e}'
            | '\u{200f}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2066}'..='\u{2069}'
    )
}

fn checked_len(value: usize) -> ValidationResult<u64> {
    u64::try_from(value).map_err(|_| ResourcesValidationError::Limit)
}

#[derive(Debug, Clone, Copy)]
struct LogicalCharge {
    value: u64,
    limit: u64,
}

impl LogicalCharge {
    const fn new(limit: u64) -> Self {
        Self { value: 0, limit }
    }

    fn add(&mut self, value: u64) -> ValidationResult<()> {
        self.value = self
            .value
            .checked_add(value)
            .ok_or(ResourcesValidationError::Limit)?;
        if self.value > self.limit {
            return Err(ResourcesValidationError::Limit);
        }
        Ok(())
    }

    fn string(&mut self, value: &str) -> ValidationResult<()> {
        self.add(4)?;
        self.add(checked_len(value.len())?)
    }

    const fn value(self) -> u64 {
        self.value
    }
}

#[cfg(test)]
#[path = "resources_validation_tests.rs"]
mod tests;

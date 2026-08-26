//! Deterministic sidecar merging and latest-user extraction.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use mcode_core::Message;

use crate::types::{
    COMPACTION_SCHEMA_VERSION, CompactionInput, DeterministicDetails, DeterministicOperation,
    LatestUserRequest,
};

pub(crate) fn merge_deterministic_details(input: &CompactionInput) -> DeterministicDetails {
    let previous = input.previous_details.as_ref();
    DeterministicDetails {
        schema_version: COMPACTION_SCHEMA_VERSION,
        files_read: merge_paths(
            previous.into_iter().flat_map(|details| &details.files_read),
            &input.details.files_read,
        ),
        files_modified: merge_paths(
            previous
                .into_iter()
                .flat_map(|details| &details.files_modified),
            &input.details.files_modified,
        ),
        commands: merge_strings(
            previous.into_iter().flat_map(|details| &details.commands),
            &input.details.commands,
        ),
        todo_operations: merge_operations(
            previous
                .into_iter()
                .flat_map(|details| &details.todo_operations),
            &input.details.todo_operations,
        ),
        background_operations: merge_operations(
            previous
                .into_iter()
                .flat_map(|details| &details.background_operations),
            &input.details.background_operations,
        ),
    }
}

pub(crate) fn latest_user_request(input: &CompactionInput) -> Option<LatestUserRequest> {
    input
        .messages
        .iter()
        .enumerate()
        .rev()
        .find_map(|(message_index, source)| match &source.message {
            Message::User(message) => Some(LatestUserRequest {
                schema_version: COMPACTION_SCHEMA_VERSION,
                message_index,
                message_id: source.id.clone(),
                message: message.clone(),
            }),
            Message::Assistant(_) | Message::ToolResult(_) | Message::Custom(_) => None,
        })
}

fn merge_paths<'a>(
    previous: impl IntoIterator<Item = &'a PathBuf>,
    current: &'a [PathBuf],
) -> Vec<PathBuf> {
    let mut seen = BTreeSet::new();
    let mut merged = Vec::new();
    for path in previous.into_iter().chain(current) {
        if seen.insert(path.clone()) {
            merged.push(path.clone());
        }
    }
    merged
}

fn merge_strings<'a>(
    previous: impl IntoIterator<Item = &'a String>,
    current: &'a [String],
) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut merged = Vec::new();
    for value in previous.into_iter().chain(current) {
        let value = value.trim();
        if !value.is_empty() && seen.insert(value.to_owned()) {
            merged.push(value.to_owned());
        }
    }
    merged
}

fn merge_operations<'a>(
    previous: impl IntoIterator<Item = &'a DeterministicOperation>,
    current: &'a [DeterministicOperation],
) -> Vec<DeterministicOperation> {
    let mut by_key = BTreeMap::<String, DeterministicOperation>::new();
    let mut order = Vec::<String>::new();
    for operation in previous.into_iter().chain(current) {
        let key = operation.key.trim();
        let key = if key.is_empty() {
            operation.label.trim()
        } else {
            key
        };
        if key.is_empty() {
            continue;
        }
        let key = key.to_owned();
        if !by_key.contains_key(&key) {
            order.push(key.clone());
        }
        let mut operation = operation.clone();
        operation.schema_version = COMPACTION_SCHEMA_VERSION;
        operation.key = key.clone();
        operation.label = operation.label.trim().to_owned();
        operation.status = operation.status.trim().to_owned();
        by_key.insert(key, operation);
    }
    order
        .into_iter()
        .filter_map(|key| by_key.remove(&key))
        .collect()
}

// Rust guideline compliant 2026-08-26.

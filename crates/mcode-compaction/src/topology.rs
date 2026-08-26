//! Tool-call topology checks shared by planning and rebuilt validation.

use std::collections::BTreeSet;

use mcode_core::{ContentBlock, Message};

use crate::types::{ValidationCode, ValidationError};

#[derive(Debug)]
pub(crate) struct ToolTopology {
    pub(crate) safe_boundaries: Vec<bool>,
}

pub(crate) fn analyze_tool_pairs<'a>(
    messages: impl IntoIterator<Item = &'a Message>,
) -> Result<ToolTopology, ValidationError> {
    let messages: Vec<&Message> = messages.into_iter().collect();
    let mut seen = BTreeSet::new();
    let mut open = BTreeSet::new();
    let mut safe_boundaries = Vec::with_capacity(messages.len().saturating_add(1));
    safe_boundaries.push(true);

    for (index, message) in messages.iter().enumerate() {
        match message {
            Message::Assistant(assistant) => {
                for call in assistant.blocks.iter().filter_map(|block| match block {
                    ContentBlock::ToolCall(call) => Some(call),
                    ContentBlock::Text(_) | ContentBlock::Thinking(_) | ContentBlock::Image(_) => {
                        None
                    }
                }) {
                    if call.id.is_empty() {
                        return Err(ValidationError::new(
                            ValidationCode::InvalidInput,
                            format!("tool call at message {index} has an empty id"),
                        ));
                    }
                    if !seen.insert(call.id.clone()) {
                        return Err(ValidationError::new(
                            ValidationCode::DuplicateToolCall,
                            format!("tool call id is duplicated at message {index}"),
                        ));
                    }
                    open.insert(call.id.clone());
                }
            }
            Message::ToolResult(result) => {
                if !open.remove(&result.tool_call_id) {
                    return Err(ValidationError::new(
                        ValidationCode::OrphanToolResult,
                        format!("tool result at message {index} has no unresolved call"),
                    ));
                }
            }
            Message::User(_) | Message::Custom(_) => {}
        }
        safe_boundaries.push(open.is_empty());
    }

    if !open.is_empty() {
        return Err(ValidationError::new(
            ValidationCode::UnresolvedToolCall,
            format!("{} tool call(s) have no result", open.len()),
        ));
    }

    Ok(ToolTopology { safe_boundaries })
}

// Rust guideline compliant 2026-08-26.

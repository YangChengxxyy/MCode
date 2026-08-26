//! Declarative UI action DTOs emitted by a plugin guest.

// Rust guideline compliant 2026-08-26.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ids::Identifier;
use crate::limits::MAX_UI_ACTION_BYTES;
use crate::validation::{is_terminal_control, validate_json_value};

/// Kind of host-interpreted UI action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum UiActionKind {
    /// Activate a declared command or control.
    Activate,
    /// Submit a bounded form payload.
    Submit,
    /// Dismiss a modal or overlay.
    Dismiss,
    /// Navigate within host-owned UI chrome.
    Navigate,
}

/// One declarative UI action submitted through the host import.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UiAction {
    /// Stable action id.
    pub id: Identifier,
    /// View that originated the action.
    pub view_id: Identifier,
    /// Host-interpreted action kind.
    pub kind: UiActionKind,
    /// Bounded JSON payload without terminal controls.
    pub payload: Value,
}

/// Validates a UI action DTO.
///
/// # Errors
///
/// Returns [`UiActionValidationError`] for oversized JSON, control characters,
/// or serialization failures.
pub fn validate_ui_action(action: &UiAction) -> Result<(), UiActionValidationError> {
    let serialized =
        serde_json::to_vec(action).map_err(|_| UiActionValidationError::Serialization)?;
    if serialized.len() > MAX_UI_ACTION_BYTES {
        return Err(UiActionValidationError::TooLarge);
    }
    if json_has_terminal_control(&action.payload) {
        return Err(UiActionValidationError::TerminalControl);
    }
    validate_json_value(&action.payload, MAX_UI_ACTION_BYTES)
        .map_err(|_| UiActionValidationError::InvalidPayload)?;
    Ok(())
}

fn json_has_terminal_control(value: &Value) -> bool {
    match value {
        Value::String(text) => text.chars().any(is_terminal_control),
        Value::Array(values) => values.iter().any(json_has_terminal_control),
        Value::Object(values) => values.values().any(json_has_terminal_control),
        Value::Null | Value::Bool(_) | Value::Number(_) => false,
    }
}

/// Invalid UI action DTO.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum UiActionValidationError {
    /// Serialized action exceeded its byte limit.
    #[error("plugin UI action exceeds its size limit")]
    TooLarge,
    /// Payload JSON was malformed or too deep.
    #[error("plugin UI action payload is invalid")]
    InvalidPayload,
    /// Action JSON contained a forbidden terminal control.
    #[error("plugin UI action contains a forbidden terminal control")]
    TerminalControl,
    /// The action could not be serialized.
    #[error("plugin UI action could not be serialized")]
    Serialization,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{UiAction, UiActionKind, UiActionValidationError, validate_ui_action};
    use crate::ids::Identifier;

    #[test]
    fn action_rejects_ansi_payload_text() {
        let action = UiAction {
            id: Identifier::parse("act.submit").expect("id"),
            view_id: Identifier::parse("view.main").expect("id"),
            kind: UiActionKind::Submit,
            payload: json!({"label": "ok"}),
        };
        validate_ui_action(&action).expect("valid");

        let ansi = UiAction {
            payload: json!("\u{1b}[31m"),
            ..action
        };
        assert_eq!(
            validate_ui_action(&ansi),
            Err(UiActionValidationError::TerminalControl)
        );
    }
}

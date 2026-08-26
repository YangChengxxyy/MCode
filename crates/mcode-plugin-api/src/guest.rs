//! Guest WIT constants and typed JSON DTOs carried over WIT strings.

// Rust guideline compliant 2026-08-26.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ids::Identifier;
use crate::limits::{MAX_DESCRIPTOR_JSON_BYTES, MAX_GUEST_OUTPUT_BYTES};
use crate::validation::{parse_strict_json, validate_json_value};

/// WIT package name.
pub const WIT_PACKAGE: &str = "mcode:plugin";

/// WIT world name.
pub const WIT_WORLD: &str = "plugin";

/// WIT package/world version.
pub const WIT_WORLD_VERSION: &str = "0.1.0";

/// Fully qualified world id recorded in `plugin.json`.
pub const WIT_WORLD_ID: &str = "mcode:plugin/plugin@0.1.0";

/// Fully qualified host interface id a component may import.
pub const HOST_INTERFACE_ID: &str = "mcode:plugin/host@0.1.0";

/// Canonical WIT source for the plugin world.
pub const PLUGIN_WIT: &str = include_str!("../wit/plugin.wit");

/// Target of one guest invocation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum GuestInvokeTarget {
    /// Declared tool contribution.
    Tool {
        /// Contribution id.
        id: Identifier,
    },
    /// Declared command contribution.
    Command {
        /// Contribution id.
        id: Identifier,
    },
}

/// JSON request body for the `invoke` export.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GuestInvokeRequest {
    /// Host-issued call id.
    pub call_id: Identifier,
    /// Declared contribution target.
    pub target: GuestInvokeTarget,
    /// Active plugin generation.
    pub generation: u64,
    /// Bounded input JSON.
    pub input: Value,
}

/// JSON response body for a successful `invoke` export.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GuestInvokeResponse {
    /// Optional bounded output JSON.
    #[serde(default)]
    pub output: Option<Value>,
}

/// JSON request body for the `render` export.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GuestRenderRequest {
    /// Declared view id.
    pub view_id: Identifier,
    /// Active plugin generation.
    pub generation: u64,
}

/// JSON response body for a successful `render` export.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GuestRenderResponse {
    /// Declarative view JSON document.
    pub view: Value,
}

/// Guest-encoded failure body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GuestErrorBody {
    /// Nested error object.
    pub error: GuestWireError,
}

/// Stable non-sensitive guest error code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GuestWireError {
    /// Identifier error code.
    pub code: Identifier,
}

/// Classifies a guest export string as success JSON or a typed error.
///
/// Empty strings are success with no payload.
///
/// # Errors
///
/// Returns [`GuestParseError`] when the payload is oversized, not strict JSON,
/// or not an object matching the success/error envelopes.
pub fn parse_guest_success(bytes: &str) -> Result<Option<Value>, GuestParseError> {
    if bytes.len() > MAX_GUEST_OUTPUT_BYTES {
        return Err(GuestParseError::TooLarge);
    }
    if bytes.is_empty() {
        return Ok(None);
    }
    let value = parse_strict_json(bytes.as_bytes()).map_err(|_| GuestParseError::InvalidJson)?;
    if let Ok(error) = serde_json::from_value::<GuestErrorBody>(value.clone()) {
        return Err(GuestParseError::Guest {
            code: error.error.code,
        });
    }
    validate_json_value(&value, MAX_DESCRIPTOR_JSON_BYTES)
        .map_err(|_| GuestParseError::TooLarge)?;
    Ok(Some(value))
}

/// Parses a guest export whose only success value is an empty string.
///
/// WIT `construct` and `on-event` treat an empty string as success. A typed
/// `{"error":{"code":"..."}}` envelope is a guest failure. Any other payload,
/// including non-error JSON, is invalid.
///
/// # Errors
///
/// Returns [`GuestParseError`] for oversized, malformed, non-empty success, or
/// typed guest-error payloads.
pub fn parse_guest_error(bytes: &str) -> Result<(), GuestParseError> {
    if bytes.len() > MAX_GUEST_OUTPUT_BYTES {
        return Err(GuestParseError::TooLarge);
    }
    if bytes.is_empty() {
        return Ok(());
    }
    let value = parse_strict_json(bytes.as_bytes()).map_err(|_| GuestParseError::InvalidJson)?;
    if let Ok(error) = serde_json::from_value::<GuestErrorBody>(value) {
        return Err(GuestParseError::Guest {
            code: error.error.code,
        });
    }
    Err(GuestParseError::InvalidJson)
}

/// Guest JSON envelope parse failure.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GuestParseError {
    /// Guest output exceeded its byte limit.
    #[error("plugin guest output exceeds its size limit")]
    TooLarge,
    /// Guest output was not strict JSON.
    #[error("plugin guest output is not strict JSON")]
    InvalidJson,
    /// Guest returned a typed error code.
    #[error("plugin guest returned error {code}")]
    Guest {
        /// Stable error code.
        code: Identifier,
    },
}

#[cfg(test)]
mod tests {
    use super::{GuestParseError, parse_guest_error, parse_guest_success};

    #[test]
    fn empty_string_is_success_and_error_envelope_is_typed() {
        assert_eq!(parse_guest_success("").expect("empty"), None);
        assert!(matches!(
            parse_guest_success(r#"{"error":{"code":"boom"}}"#),
            Err(GuestParseError::Guest { .. })
        ));
    }

    #[test]
    fn construct_style_export_accepts_only_empty_success() {
        parse_guest_error("").expect("empty success");
        assert!(matches!(
            parse_guest_error(r#"{"error":{"code":"boom"}}"#),
            Err(GuestParseError::Guest { .. })
        ));
        assert_eq!(parse_guest_error("{}"), Err(GuestParseError::InvalidJson));
        assert_eq!(
            parse_guest_error("not-json"),
            Err(GuestParseError::InvalidJson)
        );
    }
}

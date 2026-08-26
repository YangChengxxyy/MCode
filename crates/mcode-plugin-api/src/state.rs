//! Versioned extension state and custom-event DTOs.
//!
//! This module owns no session log, checkpoint, persistence, or compare-and-swap
//! implementation.

// Rust guideline compliant 2026-08-26.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ids::Identifier;
use crate::limits::{MAX_CUSTOM_EVENT_BYTES, MAX_STATE_VALUE_BYTES};
use crate::validation::{valid_public_text, validate_json_value};

const MAX_STATE_DECLARATIONS: usize = 64;

/// One portable JSON state key declared by a plugin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PortableStateDeclaration {
    /// Plugin-scoped state key.
    pub key: Identifier,
    /// Positive schema version.
    pub version: u32,
    /// Per-value byte limit, capped by [`MAX_STATE_VALUE_BYTES`].
    pub max_bytes: usize,
}

/// One secret name declared by a plugin.
///
/// This declares lookup intent only. No secret value can be serialized here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SecretStateDeclaration {
    /// Host-configured secret name.
    pub name: Identifier,
    /// Non-sensitive explanation shown during grant review.
    pub purpose: String,
}

/// Portable and secret state declarations from `plugin.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StateDeclarations {
    /// Portable JSON keys owned by a future session adapter.
    #[serde(default)]
    pub portable: Vec<PortableStateDeclaration>,
    /// Opaque secret-handle names.
    #[serde(default)]
    pub secret: Vec<SecretStateDeclaration>,
}

impl StateDeclarations {
    /// Validates versions, byte limits, uniqueness, and public descriptions.
    ///
    /// # Errors
    ///
    /// Returns [`StateDeclarationError`] for duplicate identifiers, zero
    /// versions, invalid size limits, excessive declarations, or control
    /// characters in a secret purpose.
    pub fn validate(&self) -> Result<(), StateDeclarationError> {
        if self.portable.len() + self.secret.len() > MAX_STATE_DECLARATIONS {
            return Err(StateDeclarationError::TooMany);
        }
        let mut portable = BTreeSet::new();
        for declaration in &self.portable {
            if declaration.version == 0 {
                return Err(StateDeclarationError::InvalidVersion);
            }
            if declaration.max_bytes == 0 || declaration.max_bytes > MAX_STATE_VALUE_BYTES {
                return Err(StateDeclarationError::InvalidLimit);
            }
            if !portable.insert(declaration.key.clone()) {
                return Err(StateDeclarationError::Duplicate);
            }
        }
        let mut secret = BTreeSet::new();
        for declaration in &self.secret {
            if !valid_public_text(&declaration.purpose, 512) {
                return Err(StateDeclarationError::InvalidPurpose);
            }
            if !secret.insert(declaration.name.clone()) {
                return Err(StateDeclarationError::Duplicate);
            }
        }
        Ok(())
    }

    /// Returns the declaration for `key`, when present.
    #[must_use]
    pub fn portable_key(&self, key: &Identifier) -> Option<&PortableStateDeclaration> {
        self.portable
            .iter()
            .find(|declaration| &declaration.key == key)
    }
}

/// Current versioned extension state returned by a session-owned adapter.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExtensionState {
    /// Plugin-scoped state key.
    pub key: Identifier,
    /// State schema version.
    pub version: u32,
    /// Plugin registry/runtime generation that produced this value.
    pub generation: u64,
    /// Portable JSON value.
    pub value: Value,
}

impl ExtensionState {
    /// Validates this DTO against a manifest declaration and generation.
    ///
    /// # Errors
    ///
    /// Returns [`StateDtoError`] for undeclared keys, version/generation
    /// mismatch, or oversized/deep JSON.
    pub fn validate(
        &self,
        declarations: &StateDeclarations,
        expected_generation: u64,
    ) -> Result<(), StateDtoError> {
        validate_state_fields(
            &self.key,
            self.version,
            self.generation,
            &self.value,
            declarations,
            expected_generation,
        )
    }
}

/// Versioned state replacement passed to a session-owned host adapter.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExtensionStateUpdate {
    /// Plugin-scoped state key.
    pub key: Identifier,
    /// State schema version.
    pub version: u32,
    /// Plugin registry/runtime generation producing this update.
    pub generation: u64,
    /// Replacement portable JSON value.
    pub value: Value,
}

impl ExtensionStateUpdate {
    /// Validates this DTO against a manifest declaration and generation.
    ///
    /// # Errors
    ///
    /// Returns [`StateDtoError`] for undeclared keys, version/generation
    /// mismatch, or oversized/deep JSON.
    pub fn validate(
        &self,
        declarations: &StateDeclarations,
        expected_generation: u64,
    ) -> Result<(), StateDtoError> {
        validate_state_fields(
            &self.key,
            self.version,
            self.generation,
            &self.value,
            declarations,
            expected_generation,
        )
    }
}

/// Versioned custom event passed to a session-owned host adapter.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExtensionEvent {
    /// Plugin-scoped event kind.
    pub kind: Identifier,
    /// Positive event schema version.
    pub version: u32,
    /// Plugin registry/runtime generation producing this event.
    pub generation: u64,
    /// Bounded event JSON payload.
    pub data: Value,
}

impl ExtensionEvent {
    /// Validates version, generation, and JSON bounds.
    ///
    /// # Errors
    ///
    /// Returns [`StateDtoError`] for zero versions, stale generations, or
    /// oversized/deep JSON.
    pub fn validate(&self, expected_generation: u64) -> Result<(), StateDtoError> {
        if self.version == 0 {
            return Err(StateDtoError::VersionMismatch);
        }
        if self.generation != expected_generation {
            return Err(StateDtoError::GenerationMismatch);
        }
        validate_json_value(&self.data, MAX_CUSTOM_EVENT_BYTES)
            .map_err(|_| StateDtoError::ValueInvalid)?;
        Ok(())
    }
}

fn validate_state_fields(
    key: &Identifier,
    version: u32,
    generation: u64,
    value: &Value,
    declarations: &StateDeclarations,
    expected_generation: u64,
) -> Result<(), StateDtoError> {
    let declaration = declarations
        .portable_key(key)
        .ok_or(StateDtoError::UndeclaredKey)?;
    if declaration.version != version {
        return Err(StateDtoError::VersionMismatch);
    }
    if generation != expected_generation {
        return Err(StateDtoError::GenerationMismatch);
    }
    validate_json_value(value, declaration.max_bytes).map_err(|_| StateDtoError::ValueInvalid)?;
    Ok(())
}

/// Invalid state declaration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum StateDeclarationError {
    /// Too many keys were declared.
    #[error("plugin declares too many state keys")]
    TooMany,
    /// A portable state version was zero.
    #[error("portable state version must be positive")]
    InvalidVersion,
    /// A portable state size limit was invalid.
    #[error("portable state size limit is invalid")]
    InvalidLimit,
    /// A state or secret identifier was duplicated.
    #[error("plugin state declaration contains a duplicate identifier")]
    Duplicate,
    /// A secret purpose was empty, oversized, or contained controls.
    #[error("plugin secret purpose is invalid")]
    InvalidPurpose,
}

/// Versioned extension state/event DTO failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum StateDtoError {
    /// Portable state key was not declared.
    #[error("extension state key was not declared")]
    UndeclaredKey,
    /// DTO schema version did not match its declaration.
    #[error("extension state or event version is invalid")]
    VersionMismatch,
    /// DTO came from another plugin generation.
    #[error("extension state or event generation is stale")]
    GenerationMismatch,
    /// JSON exceeded byte/depth/node limits.
    #[error("extension state or event JSON exceeds its limits")]
    ValueInvalid,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        ExtensionEvent, ExtensionStateUpdate, PortableStateDeclaration, StateDeclarations,
        StateDtoError,
    };
    use crate::ids::Identifier;

    fn declarations(max_bytes: usize) -> StateDeclarations {
        StateDeclarations {
            portable: vec![PortableStateDeclaration {
                key: Identifier::parse("items").expect("key"),
                version: 2,
                max_bytes,
            }],
            secret: vec![],
        }
    }

    #[test]
    fn state_dto_enforces_version_generation_and_size_without_owning_storage() {
        let update = ExtensionStateUpdate {
            key: Identifier::parse("items").expect("key"),
            version: 2,
            generation: 7,
            value: json!(["one"]),
        };
        update.validate(&declarations(64), 7).expect("valid");
        assert_eq!(
            update.validate(&declarations(64), 8),
            Err(StateDtoError::GenerationMismatch)
        );
        assert_eq!(
            update.validate(&declarations(4), 7),
            Err(StateDtoError::ValueInvalid)
        );

        let event = ExtensionEvent {
            kind: Identifier::parse("state.changed").expect("kind"),
            version: 0,
            generation: 7,
            data: json!({}),
        };
        assert_eq!(event.validate(7), Err(StateDtoError::VersionMismatch));
    }
}

//! Validated identifiers used across the public plugin boundary.

// Rust guideline compliant 2026-08-26.

use std::borrow::Borrow;
use std::fmt::{Display, Formatter};
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize};

const MAX_PLUGIN_ID_BYTES: usize = 128;
const MAX_IDENTIFIER_BYTES: usize = 96;

/// Error returned when a plugin identifier is malformed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid {kind}: {reason}")]
pub struct IdError {
    kind: &'static str,
    reason: &'static str,
}

impl IdError {
    fn new(kind: &'static str, reason: &'static str) -> Self {
        Self { kind, reason }
    }
}

/// Stable reverse-domain-like plugin identifier.
///
/// Values begin with a lowercase ASCII letter and may contain lowercase
/// letters, digits, `.`, `-`, and `_`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct PluginId(String);

impl PluginId {
    /// Parses and validates a plugin identifier.
    ///
    /// # Errors
    ///
    /// Returns [`IdError`] when `value` is empty, too long, non-ASCII, or
    /// contains unsupported characters.
    pub fn parse(value: impl Into<String>) -> Result<Self, IdError> {
        let value = value.into();
        validate_id(&value, MAX_PLUGIN_ID_BYTES, "plugin id")?;
        Ok(Self(value))
    }

    /// Returns the identifier as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Borrow<str> for PluginId {
    fn borrow(&self) -> &str {
        self.as_str()
    }
}

impl Display for PluginId {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl FromStr for PluginId {
    type Err = IdError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

impl<'de> Deserialize<'de> for PluginId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(value).map_err(serde::de::Error::custom)
    }
}

/// Stable identifier for a contribution, event, state key, or view.
///
/// The syntax matches [`PluginId`] but uses a shorter size limit.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct Identifier(String);

impl Identifier {
    /// Parses and validates a stable identifier.
    ///
    /// # Errors
    ///
    /// Returns [`IdError`] when `value` is empty, too long, non-ASCII, or
    /// contains unsupported characters.
    pub fn parse(value: impl Into<String>) -> Result<Self, IdError> {
        let value = value.into();
        validate_id(&value, MAX_IDENTIFIER_BYTES, "identifier")?;
        Ok(Self(value))
    }

    /// Returns the identifier as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Borrow<str> for Identifier {
    fn borrow(&self) -> &str {
        self.as_str()
    }
}

impl Display for Identifier {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl FromStr for Identifier {
    type Err = IdError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

impl<'de> Deserialize<'de> for Identifier {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(value).map_err(serde::de::Error::custom)
    }
}

fn validate_id(value: &str, max_bytes: usize, kind: &'static str) -> Result<(), IdError> {
    if value.is_empty() {
        return Err(IdError::new(kind, "must not be empty"));
    }
    if value.len() > max_bytes {
        return Err(IdError::new(kind, "exceeds the byte limit"));
    }
    let mut bytes = value.bytes();
    if !bytes.next().is_some_and(|byte| byte.is_ascii_lowercase()) {
        return Err(IdError::new(
            kind,
            "must start with a lowercase ASCII letter",
        ));
    }
    if !value.bytes().all(|byte| {
        byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'-' | b'_')
    }) {
        return Err(IdError::new(kind, "contains unsupported characters"));
    }
    if value.ends_with(['.', '-', '_']) {
        return Err(IdError::new(kind, "must end with a letter or digit"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{Identifier, PluginId};

    #[test]
    fn identifiers_validate_during_json_decode() {
        let id: PluginId = serde_json::from_str(r#""com.mcode.ask""#).expect("valid id");
        assert_eq!(id.as_str(), "com.mcode.ask");
        assert!(serde_json::from_str::<Identifier>(r#""Bad""#).is_err());
        assert!(PluginId::parse("a/escape").is_err());
    }
}

//! Opaque secret references and redacted in-memory values.

// Rust guideline compliant 2026-08-20.

use std::{fmt, sync::Arc};

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use zeroize::Zeroizing;

use crate::error::{Error, ErrorKind, Recovery, Result};

const MAX_SECRET_REFERENCE_BYTES: usize = 256;

/// An opaque key understood only by the host secret store.
///
/// This value is safe to persist in ordinary JSON; it is never interpreted as
/// credential material by the MCP engine.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SecretRef(String);

impl SecretRef {
    /// Validates a non-secret reference identifier.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::Configuration`] for empty or control-bearing values.
    pub fn new(value: impl AsRef<str>) -> Result<Self> {
        let value = value.as_ref();
        if value.is_empty()
            || value.len() > MAX_SECRET_REFERENCE_BYTES
            || value.chars().any(char::is_control)
        {
            return Err(Error::new(
                ErrorKind::Configuration,
                Recovery::Fatal,
                "secretRef must be non-empty, at most 256 bytes, and contain no controls",
            ));
        }
        Ok(Self(value.to_owned()))
    }

    /// Returns the host-defined reference identifier.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SecretRef {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl Serialize for SecretRef {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for SecretRef {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(de::Error::custom)
    }
}

/// Secret text that zeroizes its final in-memory allocation.
///
/// `Debug` and `Display` never reveal the contained value. Clones share one
/// zeroizing allocation so transport retries do not multiply secret copies.
#[derive(Clone)]
pub struct SecretValue(Arc<Zeroizing<String>>);

impl SecretValue {
    /// Wraps secret text returned by an authorized host store.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(Arc::new(Zeroizing::new(value.into())))
    }

    pub(crate) fn expose(&self) -> &str {
        self.0.as_str()
    }
}

impl fmt::Debug for SecretValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretValue([REDACTED])")
    }
}

impl fmt::Display for SecretValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[REDACTED]")
    }
}

/// Opaque serialized authorization state for a host-provided secret store.
///
/// The bytes may contain OAuth tokens or PKCE state. A host implementation must
/// persist them only in a credential store, never in ordinary settings JSON.
#[derive(Clone)]
pub struct SecretBytes(Arc<Zeroizing<Vec<u8>>>);

impl SecretBytes {
    /// Wraps an opaque secret record.
    #[must_use]
    pub fn new(value: impl Into<Vec<u8>>) -> Self {
        Self(Arc::new(Zeroizing::new(value.into())))
    }

    /// Borrows the opaque bytes for transfer to a secure host store.
    ///
    /// Callers must not log, serialize into configuration, or retain this slice.
    #[must_use]
    pub fn expose_secret(&self) -> &[u8] {
        self.0.as_slice()
    }
}

impl fmt::Debug for SecretBytes {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretBytes([REDACTED])")
    }
}

/// A namespaced key for opaque OAuth records in the host secret store.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SecretStoreKey(String);

impl SecretStoreKey {
    /// Creates a secret-store key from an engine-controlled value.
    ///
    /// # Errors
    ///
    /// Returns a configuration error when the key is empty or contains controls.
    pub fn new(value: impl AsRef<str>) -> Result<Self> {
        let value = value.as_ref();
        if value.is_empty() || value.len() > 512 || value.chars().any(char::is_control) {
            return Err(Error::new(
                ErrorKind::Configuration,
                Recovery::Fatal,
                "secret-store key is invalid",
            ));
        }
        Ok(Self(value.to_owned()))
    }

    /// Returns the engine-generated key.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SecretStoreKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_and_display_redact_secret_material() {
        let marker = "credential-marker-123";
        let value = SecretValue::new(marker);
        let bytes = SecretBytes::new(marker.as_bytes());
        assert!(!format!("{value:?} {value}").contains(marker));
        assert!(!format!("{bytes:?}").contains(marker));
    }
}

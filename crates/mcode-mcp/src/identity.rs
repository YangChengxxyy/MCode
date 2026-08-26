//! Stable MCP server and catalog identities.

// Rust guideline compliant 2026-08-20.

use std::{fmt, str::FromStr};

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

use crate::error::{Error, ErrorKind, Recovery, Result};

const MAX_SERVER_NAME_BYTES: usize = 64;
const MAX_ITEM_NAME_BYTES: usize = 256;

/// A validated key from `plugins.mcode.mcp.servers`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ServerName(String);

impl ServerName {
    /// Parses a server name safe for namespaced identities and diagnostics.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::Configuration`] for empty, oversized, or unsafe names.
    pub fn new(value: impl AsRef<str>) -> Result<Self> {
        let value = value.as_ref();
        let valid_length = !value.is_empty() && value.len() <= MAX_SERVER_NAME_BYTES;
        let valid_characters = value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'));
        if !valid_length || !valid_characters {
            return Err(Error::new(
                ErrorKind::Configuration,
                Recovery::Fatal,
                "server names must be 1-64 ASCII letters, digits, '.', '_' or '-'",
            ));
        }
        Ok(Self(value.to_owned()))
    }

    /// Returns the validated server name.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for ServerName {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for ServerName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for ServerName {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self> {
        Self::new(value)
    }
}

impl Serialize for ServerName {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ServerName {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(de::Error::custom)
    }
}

/// The catalog section that supplied an MCP item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub enum ItemKind {
    /// A callable MCP tool.
    Tool,
    /// A concrete MCP resource.
    Resource,
    /// An MCP resource URI template.
    ResourceTemplate,
    /// An MCP prompt template.
    Prompt,
}

/// A stable item identity with MCP provenance.
///
/// Its display form is always `mcp:<server>:<item>`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct NamespacedId {
    server: ServerName,
    item: String,
}

impl NamespacedId {
    /// Creates an identity after rejecting delimiter and terminal injection.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::Validation`] when the remote item name is unsafe.
    pub fn new(server: ServerName, item: impl AsRef<str>) -> Result<Self> {
        let item = item.as_ref();
        let valid = !item.is_empty()
            && item.len() <= MAX_ITEM_NAME_BYTES
            && !item.contains(':')
            && item
                .chars()
                .all(|character| !character.is_control() && character != '\u{1b}');
        if !valid {
            return Err(Error::new(
                ErrorKind::Validation,
                Recovery::Fatal,
                "remote item names must be non-empty, at most 256 bytes, and contain no ':' or controls",
            )
            .with_server(server));
        }
        Ok(Self {
            server,
            item: item.to_owned(),
        })
    }

    /// Returns the originating server.
    #[must_use]
    pub fn server(&self) -> &ServerName {
        &self.server
    }

    /// Returns the server-provided item name.
    #[must_use]
    pub fn item(&self) -> &str {
        &self.item
    }
}

impl fmt::Display for NamespacedId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "mcp:{}:{}", self.server, self.item)
    }
}

impl<'de> Deserialize<'de> for NamespacedId {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct PersistedNamespacedId {
            server: ServerName,
            item: String,
        }

        let persisted = PersistedNamespacedId::deserialize(deserializer)?;
        Self::new(persisted.server, persisted.item).map_err(de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_has_required_provenance_shape() {
        let id = NamespacedId::new(ServerName::new("github").unwrap(), "issues.list").unwrap();
        assert_eq!(id.to_string(), "mcp:github:issues.list");
    }

    #[test]
    fn delimiters_and_controls_are_rejected() {
        let server = ServerName::new("safe").unwrap();
        assert!(NamespacedId::new(server.clone(), "bad:name").is_err());
        assert!(NamespacedId::new(server, "bad\u{1b}[31m").is_err());
    }

    #[test]
    fn deserialization_preserves_item_invariants() {
        for item in [
            "bad:name".to_owned(),
            "bad\u{1b}[31m".to_owned(),
            "x".repeat(MAX_ITEM_NAME_BYTES + 1),
        ] {
            let encoded = serde_json::json!({"server":"safe", "item":item});
            assert!(serde_json::from_value::<NamespacedId>(encoded).is_err());
        }

        let expected = NamespacedId::new(ServerName::new("safe").unwrap(), "valid.item").unwrap();
        let encoded = serde_json::to_value(&expected).unwrap();
        assert_eq!(
            serde_json::from_value::<NamespacedId>(encoded).unwrap(),
            expected
        );
    }
}

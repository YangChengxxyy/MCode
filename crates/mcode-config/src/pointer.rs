//! RFC 6901 JSON Pointer values used by provenance and diagnostics.

// Rust guideline compliant 2026-08-26

use std::fmt::{self, Display, Formatter};

/// Identifies one value in the merged configuration using RFC 6901 syntax.
///
/// The empty string identifies the document root. Object member names escape
/// `~` as `~0` and `/` as `~1`; array indexes use their decimal spelling.
#[derive(Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct JsonPointer(String);

impl JsonPointer {
    /// Returns the pointer for the document root.
    #[must_use]
    pub fn root() -> Self {
        Self(String::new())
    }

    /// Returns this pointer as its RFC 6901 string representation.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Reports whether this pointer identifies the document root.
    #[must_use]
    pub fn is_root(&self) -> bool {
        self.0.is_empty()
    }

    pub(crate) fn child(&self, token: &str) -> Self {
        let mut pointer = self.0.clone();
        pointer.push('/');
        for character in token.chars() {
            match character {
                '~' => pointer.push_str("~0"),
                '/' => pointer.push_str("~1"),
                character => pointer.push(character),
            }
        }
        Self(pointer)
    }

    pub(crate) fn index(&self, index: usize) -> Self {
        self.child(&index.to_string())
    }

    pub(crate) fn is_self_or_descendant_of(&self, ancestor: &Self) -> bool {
        if ancestor.is_root() {
            return true;
        }
        self == ancestor
            || self
                .0
                .strip_prefix(&ancestor.0)
                .is_some_and(|suffix| suffix.starts_with('/'))
    }
}

impl AsRef<str> for JsonPointer {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl Display for JsonPointer {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl fmt::Debug for JsonPointer {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.debug_tuple("JsonPointer").field(&self.0).finish()
    }
}

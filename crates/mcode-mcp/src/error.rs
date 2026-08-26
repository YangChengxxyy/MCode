//! Sanitized error types for MCP boundaries.

// Rust guideline compliant 2026-08-20.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::identity::ServerName;

/// Indicates whether reconnecting can recover from an error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub enum Recovery {
    /// The operation may succeed after reconnecting the affected server.
    Recoverable,
    /// Retrying the same configuration cannot safely recover the operation.
    Fatal,
}

/// Classifies an MCP engine failure without exposing upstream details.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub enum ErrorKind {
    /// A JSON configuration value is invalid or unsafe.
    Configuration,
    /// The host or user denied an operation.
    Permission,
    /// The configured trust policy does not permit an operation.
    Trust,
    /// The negotiated peer does not support the requested capability.
    UnsupportedCapability,
    /// The peer violated the MCP or JSON-RPC protocol.
    Protocol,
    /// Untrusted schema, catalog, arguments, or content failed validation.
    Validation,
    /// A transport or contained process failed.
    Transport,
    /// A bounded operation exceeded its deadline.
    Timeout,
    /// A caller cancelled an in-flight request.
    Cancelled,
    /// Stable identities collide in a transactional catalog.
    Conflict,
    /// Authentication requires host action or failed safely.
    Authentication,
    /// Graceful shutdown did not complete.
    Shutdown,
    /// The named server is not ready for requests.
    Unavailable,
}

/// A sanitized MCP engine error.
///
/// Error messages are stripped of terminal controls and capped at construction.
/// Upstream errors are intentionally not retained because they can contain tokens,
/// request bodies, or hostile server output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    kind: ErrorKind,
    recovery: Recovery,
    server: Option<ServerName>,
    message: String,
}

impl Error {
    /// Creates a sanitized error with no server provenance.
    pub fn new(kind: ErrorKind, recovery: Recovery, message: impl AsRef<str>) -> Self {
        Self {
            kind,
            recovery,
            server: None,
            message: sanitize_diagnostic(message.as_ref()),
        }
    }

    /// Adds the affected server to this error.
    #[must_use]
    pub fn with_server(mut self, server: ServerName) -> Self {
        self.server = Some(server);
        self
    }

    /// Returns the stable error classification.
    #[must_use]
    pub const fn kind(&self) -> ErrorKind {
        self.kind
    }

    /// Returns whether reconnecting can recover from this error.
    #[must_use]
    pub const fn recovery(&self) -> Recovery {
        self.recovery
    }

    /// Returns the affected server when one is known.
    #[must_use]
    pub fn server(&self) -> Option<&ServerName> {
        self.server.as_ref()
    }

    /// Returns the already-sanitized diagnostic text.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    /// Creates a fail-closed unsupported-capability result.
    #[must_use]
    pub fn unsupported(server: ServerName, capability: impl AsRef<str>) -> Self {
        Self::new(
            ErrorKind::UnsupportedCapability,
            Recovery::Fatal,
            format!(
                "server does not advertise the required capability: {}",
                capability.as_ref()
            ),
        )
        .with_server(server)
    }

    /// Creates a recoverable transport error.
    #[must_use]
    pub fn transport(server: ServerName, message: impl AsRef<str>) -> Self {
        Self::new(ErrorKind::Transport, Recovery::Recoverable, message).with_server(server)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(server) = &self.server {
            write!(formatter, "MCP server {server}: {}", self.message)
        } else {
            formatter.write_str(&self.message)
        }
    }
}

impl std::error::Error for Error {}

/// The crate-wide result type.
pub type Result<T> = std::result::Result<T, Error>;

fn sanitize_diagnostic(input: &str) -> String {
    const MAX_DIAGNOSTIC_BYTES: usize = 1_024;

    let mut output = String::with_capacity(input.len().min(MAX_DIAGNOSTIC_BYTES));
    let mut chars = input.chars().peekable();
    while let Some(character) = chars.next() {
        if character == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                let _ = chars.next();
                for next in chars.by_ref() {
                    if ('@'..='~').contains(&next) {
                        break;
                    }
                }
            }
            continue;
        }
        if character.is_control() && !matches!(character, '\n' | '\r' | '\t') {
            continue;
        }
        if output.len() + character.len_utf8() > MAX_DIAGNOSTIC_BYTES {
            output.push('…');
            break;
        }
        output.push(character);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostics_drop_ansi_and_are_bounded() {
        let error = Error::new(
            ErrorKind::Protocol,
            Recovery::Fatal,
            format!("\u{1b}[31mbad\u{1b}[0m{}", "x".repeat(2_000)),
        );
        assert!(!error.message().contains('\u{1b}'));
        assert!(error.message().len() <= 1_027);
    }
}

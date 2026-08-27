//! Core error type shared across MCode crates.

use serde::{Deserialize, Serialize};

/// Unified error type for MCode.
///
/// All payloads are strings so the type stays `Clone` + `Serialize`: errors
/// travel through `SessionEvent` broadcasts and session logs. Subsystems
/// keep their detailed error types locally and convert at the boundary via
/// the `From` impls or by formatting into the matching variant.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, Serialize, Deserialize)]
pub enum McodeError {
    /// Filesystem or general I/O failure.
    #[error("io error: {0}")]
    Io(String),
    /// JSON (de)serialization failure.
    #[error("serde error: {0}")]
    Serde(String),
    /// LLM provider failure (network, auth, rate limit, API error).
    #[error("provider error: {0}")]
    Provider(String),
    /// Tool execution failure.
    #[error("tool error: {0}")]
    Tool(String),
    /// Plugin loading or hook failure.
    #[error("plugin error: {0}")]
    Plugin(String),
    /// Session store / actor failure.
    #[error("session error: {0}")]
    Session(String),
}

impl From<std::io::Error> for McodeError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err.to_string())
    }
}

impl From<serde_json::Error> for McodeError {
    fn from(err: serde_json::Error) -> Self {
        Self::Serde(err.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all_variants() -> Vec<McodeError> {
        vec![
            McodeError::Io("disk full".into()),
            McodeError::Serde("bad json".into()),
            McodeError::Provider("rate limited".into()),
            McodeError::Tool("exit code 1".into()),
            McodeError::Plugin("wit bind failed".into()),
            McodeError::Session("corrupt log".into()),
        ]
    }

    #[test]
    fn error_roundtrip() {
        for err in all_variants() {
            let json = serde_json::to_string(&err).unwrap();
            let back: McodeError = serde_json::from_str(&json).unwrap();
            assert_eq!(back, err);
        }
    }

    #[test]
    fn converts_from_io_error() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "missing file");
        let err = McodeError::from(io_err);
        assert!(matches!(err, McodeError::Io(_)));
        assert!(err.to_string().contains("missing file"));
    }

    #[test]
    fn converts_from_serde_json_error() {
        let json_err = serde_json::from_str::<serde_json::Value>("{").unwrap_err();
        let err = McodeError::from(json_err);
        assert!(matches!(err, McodeError::Serde(_)));
    }
}

//! Error type for LLM provider failures.

use mcode_core::McodeError;
use serde::{Deserialize, Serialize};

/// Errors raised by LLM providers: transport-level failures, non-success
/// HTTP responses, malformed SSE payloads, timeouts, cancellation, and
/// configuration problems (missing API key, bad base URL, …).
///
/// The type stays `Clone` + `Serialize` so it can travel through
/// [`crate::StreamEvent::Error`], session-event broadcasts, and logs —
/// the same constraint `McodeError` follows in `mcode-core`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LlmError {
    /// Non-success HTTP status, or an API error object delivered mid-stream
    /// (`status` is `0` when the stream carried no status information).
    #[error("http {status}: {body}")]
    Http { status: u16, body: String },
    /// Network / transport failure before or while reading the response.
    #[error("transport error: {0}")]
    Transport(String),
    /// Malformed or unexpected SSE payload (bad JSON, bad framing, …).
    #[error("sse error: {0}")]
    Sse(String),
    /// The request exceeded its configured timeout.
    #[error("request timed out")]
    Timeout,
    /// The request was cancelled through its `CancellationToken`.
    #[error("request cancelled")]
    Cancelled,
    /// Missing or invalid configuration.
    #[error("config error: {0}")]
    Config(String),
}

impl LlmError {
    /// Truncate a response body to a bounded excerpt for error payloads.
    pub(crate) fn excerpt(body: impl Into<String>) -> String {
        const MAX_BODY: usize = 512;
        let body = body.into();
        if body.chars().count() <= MAX_BODY {
            body
        } else {
            let truncated: String = body.chars().take(MAX_BODY).collect();
            format!("{truncated}… [truncated]")
        }
    }
}

impl From<LlmError> for McodeError {
    fn from(err: LlmError) -> Self {
        McodeError::Provider(err.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_roundtrip() {
        for err in [
            LlmError::Http {
                status: 429,
                body: "rate limited".into(),
            },
            LlmError::Transport("connection reset".into()),
            LlmError::Sse("bad chunk".into()),
            LlmError::Timeout,
            LlmError::Cancelled,
            LlmError::Config("no API key".into()),
        ] {
            let json = serde_json::to_string(&err).unwrap();
            let back: LlmError = serde_json::from_str(&json).unwrap();
            assert_eq!(back, err);
        }
    }

    #[test]
    fn serde_uses_snake_case_tags() {
        assert_eq!(
            serde_json::to_string(&LlmError::Timeout).unwrap(),
            "\"timeout\""
        );
        let err: LlmError = serde_json::from_str("{\"config\":\"boom\"}").unwrap();
        assert_eq!(err, LlmError::Config("boom".into()));
    }

    #[test]
    fn converts_to_mcode_error() {
        let err = McodeError::from(LlmError::Timeout);
        assert!(matches!(err, McodeError::Provider(_)));
        assert!(err.to_string().contains("timed out"));
    }

    #[test]
    fn excerpt_truncates_long_bodies() {
        let short = "hello".to_string();
        assert_eq!(LlmError::excerpt(short.clone()), short);
        let long = "x".repeat(1000);
        let excerpt = LlmError::excerpt(long);
        assert!(excerpt.starts_with('x'));
        assert!(excerpt.contains("[truncated]"));
        assert!(excerpt.chars().count() < 600);
    }
}

//! Error type for LLM provider failures.

use std::fmt;

use mcode_core::McodeError;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::profile::is_auth_header;

/// Errors raised by LLM providers: transport-level failures, non-success
/// HTTP responses, malformed SSE payloads, timeouts, cancellation, and
/// configuration problems (missing API key, bad base URL, …).
///
/// The type stays `Clone` + `Serialize` so it can travel through
/// [`crate::StreamEvent::Error`], session-event broadcasts, and logs —
/// the same constraint `McodeError` follows in `mcode-core`.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LlmError {
    /// Non-success HTTP status, or an API error object delivered mid-stream
    /// (`status` is `0` when the stream carried no status information).
    Http { status: u16, body: String },
    /// Network / transport failure before or while reading the response.
    Transport(String),
    /// Malformed or unexpected SSE payload (bad JSON, bad framing, …).
    Sse(String),
    /// The request exceeded its configured timeout.
    Timeout,
    /// The request was cancelled through its `CancellationToken`.
    Cancelled,
    /// Missing or invalid configuration.
    Config(String),
}

impl LlmError {
    /// Redact a response body, then truncate it to a bounded excerpt.
    ///
    /// Redaction runs on the complete length-bounded body *before* any
    /// display truncation: cutting first would turn long valid JSON into
    /// invalid JSON and drop the structural pass in favor of the weaker
    /// text scan. The input bound only caps work (8× the transport's
    /// error-body cap); the display bound applies afterwards.
    pub(crate) fn excerpt(body: impl AsRef<str>) -> String {
        const MAX_BODY: usize = 512;
        const MAX_REDACT_INPUT_BYTES: usize = 64 * 1_024;
        let body = body.as_ref();
        let mut input_end = body.len().min(MAX_REDACT_INPUT_BYTES);
        while !body.is_char_boundary(input_end) {
            input_end -= 1;
        }
        let body = redact_sensitive(&body[..input_end]);
        if body.chars().count() <= MAX_BODY {
            body
        } else {
            let truncated: String = body.chars().take(MAX_BODY).collect();
            format!("{truncated}… [truncated]")
        }
    }
}

impl fmt::Debug for LlmError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Http { status, body } => formatter
                .debug_struct("Http")
                .field("status", status)
                .field("body", &Self::excerpt(body))
                .finish(),
            Self::Transport(message) => formatter
                .debug_tuple("Transport")
                .field(&Self::excerpt(message))
                .finish(),
            Self::Sse(message) => formatter
                .debug_tuple("Sse")
                .field(&Self::excerpt(message))
                .finish(),
            Self::Timeout => formatter.write_str("Timeout"),
            Self::Cancelled => formatter.write_str("Cancelled"),
            Self::Config(message) => formatter
                .debug_tuple("Config")
                .field(&Self::excerpt(message))
                .finish(),
        }
    }
}

impl fmt::Display for LlmError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Http { status, body } => {
                write!(formatter, "http {status}: {}", Self::excerpt(body))
            }
            Self::Transport(message) => {
                write!(formatter, "transport error: {}", Self::excerpt(message))
            }
            Self::Sse(message) => {
                write!(formatter, "sse error: {}", Self::excerpt(message))
            }
            Self::Timeout => formatter.write_str("request timed out"),
            Self::Cancelled => formatter.write_str("request cancelled"),
            Self::Config(message) => {
                write!(formatter, "config error: {}", Self::excerpt(message))
            }
        }
    }
}

impl std::error::Error for LlmError {}

const REDACTED: &str = "[REDACTED]";

fn redact_sensitive(body: &str) -> String {
    if let Ok(mut value) = serde_json::from_str::<Value>(body) {
        redact_json(&mut value);
        return value.to_string();
    }
    redact_plain_text(body)
}

fn redact_json(value: &mut Value) {
    match value {
        Value::Object(object) => {
            for (key, value) in object {
                if is_sensitive_key(key) {
                    *value = Value::String(REDACTED.into());
                } else {
                    redact_json(value);
                }
            }
        }
        Value::Array(values) => {
            for value in values {
                redact_json(value);
            }
        }
        Value::String(text) => *text = redact_plain_text(text),
        _ => {}
    }
}

fn is_sensitive_key(key: &str) -> bool {
    // Shares the credential-name classification with header policy
    // (`profile::is_auth_header`) so body redaction cannot drift behind
    // names this repository already deems credential-like, such as
    // `auth-key`, `access-key`, `subscription-key`, or `cookie`.
    is_auth_header(key)
}

/// Redacts credential assignments and common standalone token forms.
///
/// The scan is case-insensitive for ASCII credential syntax and accepts
/// quoted or bare keys, `=` or `:`, optional whitespace, quoted values,
/// URL/query delimiters, and truncated values. Quoted keys are recognized
/// when their closing delimiter is reached; forward quote searches occur
/// only after a credential value is confirmed and that whole range is then
/// skipped. Consequently every input range is examined a constant number of
/// times, including malformed input with many unterminated escaped quotes.
fn redact_plain_text(body: &str) -> String {
    let mut output = String::with_capacity(body.len());
    let mut copied_until = 0;
    let mut cursor = 0;
    while cursor < body.len() {
        let redaction = bare_sensitive_assignment_value(body, cursor)
            .or_else(|| quoted_sensitive_assignment_value(body, cursor))
            .or_else(|| standalone_bearer_value(body, cursor))
            .or_else(|| prefixed_api_key(body, cursor));
        if let Some(redaction) = redaction {
            output.push_str(&body[copied_until..redaction.start]);
            output.push_str(REDACTED);
            cursor = redaction.end;
            copied_until = redaction.end;
            continue;
        }
        cursor = next_char_boundary(body, cursor);
    }
    output.push_str(&body[copied_until..]);
    output
}

/// Returns a sensitive bare-key assignment value beginning at `start`.
fn bare_sensitive_assignment_value(body: &str, start: usize) -> Option<std::ops::Range<usize>> {
    let bytes = body.as_bytes();
    if !bytes.get(start).is_some_and(|byte| is_key_byte(*byte))
        || (start > 0 && is_key_byte(bytes[start - 1]))
    {
        return None;
    }
    let mut key_end = start + 1;
    while bytes.get(key_end).is_some_and(|byte| is_key_byte(*byte)) {
        key_end += 1;
    }
    let key = &body[start..key_end];
    if !is_sensitive_key(key) {
        return None;
    }
    assignment_value(body, key_end, key)
}

/// Returns a sensitive quoted-key assignment value ending at `closing`.
///
/// Looking backward through only the adjacent ASCII key token avoids a
/// forward search from every quote. Escaped or non-token keys still receive
/// structural redaction whenever the complete body is valid JSON.
fn quoted_sensitive_assignment_value(body: &str, closing: usize) -> Option<std::ops::Range<usize>> {
    let bytes = body.as_bytes();
    let quote = match bytes.get(closing).copied()? {
        quote @ (b'"' | b'\'') => quote,
        _ => return None,
    };
    let mut key_start = closing;
    while key_start > 0 && is_key_byte(bytes[key_start - 1]) {
        key_start -= 1;
    }
    let opening = key_start.checked_sub(1)?;
    if bytes[opening] != quote {
        return None;
    }
    let key = &body[key_start..closing];
    if !is_sensitive_key(key) {
        return None;
    }
    assignment_value(body, closing + 1, key)
}

/// Returns the value following a confirmed sensitive assignment key.
fn assignment_value(body: &str, after_key: usize, key: &str) -> Option<std::ops::Range<usize>> {
    let mut cursor = skip_ascii_whitespace(body, after_key);
    if !matches!(body.as_bytes().get(cursor), Some(b'=' | b':')) {
        return None;
    }
    cursor = skip_ascii_whitespace(body, cursor + 1);
    match body.as_bytes().get(cursor).copied() {
        Some(quote @ (b'"' | b'\'')) => {
            let value_start = cursor + 1;
            let value_end = find_closing_quote(body, cursor, quote).unwrap_or(body.len());
            Some(value_start..value_end)
        }
        _ => {
            let value_end = if is_cookie_key(key) || is_authorization_key(key) {
                line_value_end(body, cursor)
            } else {
                field_value_end(body, cursor)
            };
            Some(cursor..value_end)
        }
    }
}

/// Locates a matching quote while honoring backslash escapes.
///
/// Callers invoke this only for a confirmed credential value and advance past
/// the scanned range, so all such searches have linear aggregate cost.
fn find_closing_quote(body: &str, open: usize, quote: u8) -> Option<usize> {
    let mut cursor = open + 1;
    let mut escaped = false;
    while cursor < body.len() {
        let byte = body.as_bytes()[cursor];
        if escaped {
            escaped = false;
        } else if byte == b'\\' {
            escaped = true;
        } else if byte == quote {
            return Some(cursor);
        }
        cursor += 1;
    }
    None
}

/// Returns the end of a `Bearer <credential>` value, if present.
fn bearer_value_end(body: &str, start: usize) -> Option<usize> {
    const BEARER: &str = "Bearer";
    let scheme_end = start.checked_add(BEARER.len())?;
    if !body.get(start..scheme_end)?.eq_ignore_ascii_case(BEARER)
        || !body
            .as_bytes()
            .get(scheme_end)
            .is_some_and(u8::is_ascii_whitespace)
    {
        return None;
    }

    let credential_start = skip_ascii_whitespace(body, scheme_end);
    match body.as_bytes().get(credential_start).copied() {
        Some(quote @ (b'"' | b'\'')) => Some(
            find_closing_quote(body, credential_start, quote).map_or(body.len(), |end| end + 1),
        ),
        Some(_) => Some(credential_token_end(body, credential_start)),
        None => Some(credential_start),
    }
}

/// Redacts a standalone `Bearer <credential>` fragment as one value.
fn standalone_bearer_value(body: &str, start: usize) -> Option<std::ops::Range<usize>> {
    if start > 0 && is_word_byte(body.as_bytes()[start - 1]) {
        return None;
    }
    bearer_value_end(body, start).map(|end| start..end)
}

/// Redacts standalone OpenAI-style `sk-…` and `sk_…` credentials.
fn prefixed_api_key(body: &str, start: usize) -> Option<std::ops::Range<usize>> {
    let end = start.checked_add(3)?;
    let prefix = body.get(start..end)?;
    if !prefix.eq_ignore_ascii_case("sk-") && !prefix.eq_ignore_ascii_case("sk_") {
        return None;
    }
    Some(start..credential_token_end(body, start))
}

fn credential_token_end(body: &str, start: usize) -> usize {
    let mut cursor = start;
    while cursor < body.len() {
        let byte = body.as_bytes()[cursor];
        if is_value_delimiter(byte) {
            break;
        }
        cursor = next_char_boundary(body, cursor);
    }
    cursor
}

/// Finds a bare field value, allowing spaces until another assignment.
fn field_value_end(body: &str, start: usize) -> usize {
    let mut cursor = start;
    while cursor < body.len() {
        let byte = body.as_bytes()[cursor];
        if matches!(byte, b'\r' | b'\n')
            || (byte.is_ascii_control() && !byte.is_ascii_whitespace())
            || matches!(
                byte,
                b'&' | b',' | b';' | b'"' | b'\'' | b'}' | b']' | b'|' | b'#'
            )
        {
            break;
        }
        if byte.is_ascii_whitespace() {
            let next = skip_ascii_whitespace(body, cursor);
            if starts_assignment(body, next) {
                break;
            }
            cursor = next;
            continue;
        }
        cursor = next_char_boundary(body, cursor);
    }
    cursor
}

/// Detects an adjacent ASCII-token assignment without searching for quotes.
fn starts_assignment(body: &str, start: usize) -> bool {
    let bytes = body.as_bytes();
    let Some(first) = bytes.get(start).copied() else {
        return false;
    };
    let mut cursor = match first {
        quote @ (b'"' | b'\'') => {
            let mut closing = start + 1;
            while bytes.get(closing).is_some_and(|byte| is_key_byte(*byte)) {
                closing += 1;
            }
            if bytes.get(closing) != Some(&quote) {
                return false;
            }
            closing + 1
        }
        byte if is_key_byte(byte) => {
            let mut end = start + 1;
            while bytes.get(end).is_some_and(|byte| is_key_byte(*byte)) {
                end += 1;
            }
            end
        }
        _ => return false,
    };
    cursor = skip_ascii_whitespace(body, cursor);
    matches!(bytes.get(cursor), Some(b'=' | b':'))
}

/// Cookie and authorization fields may contain internal assignments or
/// schemes plus credentials, so redact the complete line/query value.
fn line_value_end(body: &str, start: usize) -> usize {
    let mut cursor = start;
    while cursor < body.len() {
        let byte = body.as_bytes()[cursor];
        if byte.is_ascii_control() || matches!(byte, b'&' | b'}' | b']' | b'|' | b'#') {
            break;
        }
        cursor = next_char_boundary(body, cursor);
    }
    cursor
}

fn is_cookie_key(key: &str) -> bool {
    let normalized = key.to_ascii_lowercase().replace(['-', '_'], "");
    matches!(normalized.as_str(), "cookie" | "setcookie")
}

fn is_authorization_key(key: &str) -> bool {
    key.to_ascii_lowercase()
        .replace(['-', '_'], "")
        .ends_with("authorization")
}

fn is_key_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.')
}

fn is_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn is_value_delimiter(byte: u8) -> bool {
    byte.is_ascii_whitespace()
        || byte.is_ascii_control()
        || matches!(
            byte,
            b'&' | b';' | b',' | b'"' | b'\'' | b'}' | b']' | b'|' | b'#'
        )
}

fn skip_ascii_whitespace(body: &str, mut cursor: usize) -> usize {
    while body
        .as_bytes()
        .get(cursor)
        .is_some_and(u8::is_ascii_whitespace)
    {
        cursor += 1;
    }
    cursor
}

fn next_char_boundary(body: &str, cursor: usize) -> usize {
    cursor
        + body[cursor..]
            .chars()
            .next()
            .expect("cursor is before the string end")
            .len_utf8()
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

    #[test]
    fn display_and_debug_redact_credentials() {
        let error = LlmError::Http {
            status: 401,
            body: serde_json::json!({
                "error": "denied",
                "api_key": "sk-top-secret",
                "nested": {"authorization": "Bearer other-secret"}
            })
            .to_string(),
        };
        for rendered in [error.to_string(), format!("{error:?}")] {
            assert!(rendered.contains("REDACTED"));
            assert!(!rendered.contains("top-secret"));
            assert!(!rendered.contains("other-secret"));
        }

        let json_message = LlmError::excerpt(
            r#"{"message":"bad Bearer bearer-secret and sk-inline-secret","x-api-key":"header-secret"}"#,
        );
        assert!(json_message.contains("REDACTED"));
        assert!(!json_message.contains("bearer-secret"));
        assert!(!json_message.contains("inline-secret"));
        assert!(!json_message.contains("header-secret"));

        let truncated_json = format!(
            "{{\"api_key\":\"plain-secret\",\"padding\":\"{}",
            "x".repeat(4_000)
        );
        let rendered = LlmError::excerpt(truncated_json);
        assert!(rendered.contains("REDACTED"));
        assert!(!rendered.contains("plain-secret"));
    }

    #[test]
    fn long_valid_json_is_redacted_before_truncation() {
        // A body longer than any legacy pre-parse cut must still reach the
        // structural JSON redaction: spacing after the colon and names
        // like `client_secret` are invisible to the old marker scan.
        let body = serde_json::json!({
            "client_secret": "long-client-secret",
            "api_key": "spaced-key-secret",
            "detail": "x".repeat(4_000),
        })
        .to_string();
        let error = LlmError::Http {
            status: 400,
            body: body.clone(),
        };
        assert!(body.chars().count() > 2_048);
        for rendered in [error.to_string(), format!("{error:?}")] {
            assert!(rendered.contains("REDACTED"), "{rendered}");
            assert!(!rendered.contains("long-client-secret"), "{rendered}");
            assert!(!rendered.contains("spaced-key-secret"), "{rendered}");
        }
    }

    #[test]
    fn credential_key_coverage_matches_header_policy() {
        // Names already classified as credentials by header policy must
        // also redact in error bodies, both via the structural JSON pass
        // and via the non-JSON quoted-pair fallback.
        let body = serde_json::json!({
            "auth-key": "auth-secret",
            "access-key": "access-secret",
            "ocp-apim-subscription-key": "subscription-secret",
            "cookie": "session_id=abc",
            "set-cookie": "session_id=def",
        })
        .to_string();
        let rendered = LlmError::excerpt(body);
        assert!(rendered.contains("REDACTED"), "{rendered}");
        for secret in [
            "auth-secret",
            "access-secret",
            "subscription-secret",
            "session_id=abc",
            "session_id=def",
        ] {
            assert!(!rendered.contains(secret), "{rendered}");
        }

        let plain = "echo {\"cookie\":\"session_id=abc\", \"auth-key\": \"plain-auth-secret\"}";
        let rendered = LlmError::excerpt(plain);
        assert!(!rendered.contains("session_id=abc"), "{rendered}");
        assert!(!rendered.contains("plain-auth-secret"), "{rendered}");
        assert!(rendered.contains("REDACTED"), "{rendered}");
    }

    #[test]
    fn non_json_fallback_redacts_spaced_and_unterminated_values() {
        // Not valid JSON as a whole: the fallback text scan must still
        // cover quoted pairs with spaces and an unterminated secret.
        let truncated = "prefix {\"client_secret\": \"unterminated-secret";
        let rendered = LlmError::excerpt(truncated);
        assert!(rendered.contains("REDACTED"), "{rendered}");
        assert!(!rendered.contains("unterminated-secret"), "{rendered}");

        let spaced = "echo {\"api_key\": \"spaced-secret\", \"ok\": \"visible\"}";
        let rendered = LlmError::excerpt(spaced);
        assert!(rendered.contains("[REDACTED]"), "{rendered}");
        assert!(!rendered.contains("spaced-secret"), "{rendered}");
        assert!(rendered.contains("visible"), "{rendered}");
    }

    #[test]
    fn plaintext_assignments_use_the_shared_credential_key_policy() {
        let body = concat!(
            "status=401 AUTH-KEY=auth-value access-key : 'access value' ",
            "Ocp-Apim-Subscription-Key = \"subscription-value\" ",
            "Authorization=Bearer bearer-value API_KEY:api-value ",
            "token = token-value client_secret: secret-value ",
            "PASSWORD=\"password value\"\n",
            "credential=multi word value status=403\n",
            "COOKIE=session_id=abc; theme=dark\n",
            "Set-Cookie: session_id=def; Path=/; HttpOnly",
        );
        let rendered = LlmError::excerpt(body);
        assert!(rendered.contains("status=401"), "{rendered}");
        assert!(rendered.contains(REDACTED), "{rendered}");
        for secret in [
            "auth-value",
            "access value",
            "subscription-value",
            "Bearer bearer-value",
            "bearer-value",
            "api-value",
            "token-value",
            "secret-value",
            "password value",
            "multi word value",
            "session_id=abc",
            "session_id=def",
            "theme=dark",
        ] {
            assert!(!rendered.contains(secret), "leaked {secret}: {rendered}");
        }
    }

    #[test]
    fn plaintext_url_query_quotes_and_multiple_fields_are_redacted() {
        let body = concat!(
            "GET /fail?Api-Key=url-key&ToKeN=url-token#fragment ",
            "\"auth-key\" : \"quoted auth\" access-key='spaced access' ",
            "authorization = 'Bearer quoted-secret' message=visible",
        );
        let rendered = LlmError::excerpt(body);
        assert!(rendered.contains("message=visible"), "{rendered}");
        for secret in [
            "url-key",
            "url-token",
            "quoted auth",
            "spaced access",
            "Bearer quoted-secret",
            "quoted-secret",
        ] {
            assert!(!rendered.contains(secret), "leaked {secret}: {rendered}");
        }
    }

    #[test]
    fn plaintext_redaction_precedes_display_and_debug_truncation() {
        let body = format!(
            "{} authorization=Bearer boundary-secret token=tail-secret status=401",
            "x".repeat(480)
        );
        let error = LlmError::Http {
            status: 401,
            body: body.clone(),
        };
        for rendered in [
            LlmError::excerpt(&body),
            error.to_string(),
            format!("{error:?}"),
        ] {
            assert!(rendered.contains(REDACTED), "{rendered}");
            assert!(!rendered.contains("boundary-secret"), "{rendered}");
            assert!(!rendered.contains("tail-secret"), "{rendered}");
            assert!(rendered.chars().count() < 600, "{rendered}");
        }
    }

    #[test]
    fn malformed_escaped_quote_noise_keeps_redaction_work_linear() {
        // Every quote is escaped relative to a preceding candidate opening.
        // A forward closing-quote search at each position would rescan almost
        // the entire bounded input for every quote.
        let body = format!("token=tail-secret {}", "\\\"".repeat(30_000));
        let rendered = LlmError::excerpt(body);
        assert!(rendered.contains(REDACTED), "{rendered}");
        assert!(!rendered.contains("tail-secret"), "{rendered}");
    }
}

// Rust guideline compliant 2026-08-26

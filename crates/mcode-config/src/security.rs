//! Credential-reference enforcement and merged-value resource validation.

// Rust guideline compliant 2026-08-26

use serde_json::Value;

use crate::{
    ConfigError, ConfigErrorKind, ConfigLimits, ConfigSource, JsonPointer, ReloadCancellation,
};

pub(crate) fn validate_value_limits(
    value: &Value,
    limits: ConfigLimits,
    cancellation: &ReloadCancellation,
) -> Result<(), ConfigError> {
    validate_value_limits_with_initial_nodes(value, limits, cancellation, 0)
}

pub(crate) fn validate_envelope_value_limits(
    value: &Value,
    limits: ConfigLimits,
    cancellation: &ReloadCancellation,
) -> Result<(), ConfigError> {
    // The fixed envelope adds its root value, two member names, and the
    // format-version scalar. The payload root is counted by the traversal.
    const ENVELOPE_NODE_OVERHEAD: usize = 4;
    validate_value_limits_with_initial_nodes(value, limits, cancellation, ENVELOPE_NODE_OVERHEAD)
}

fn validate_value_limits_with_initial_nodes(
    value: &Value,
    limits: ConfigLimits,
    cancellation: &ReloadCancellation,
    initial_nodes: usize,
) -> Result<(), ConfigError> {
    let mut stack = vec![(value, 1_usize, JsonPointer::root())];
    let mut nodes = initial_nodes;

    while let Some((current, depth, pointer)) = stack.pop() {
        ensure_active(cancellation)?;
        if depth > limits.max_depth {
            return Err(ConfigError::new(ConfigErrorKind::TooDeep).at_pointer(pointer));
        }
        nodes = increment_nodes(nodes, 1, limits.max_nodes, &pointer)?;

        match current {
            Value::Object(object) => {
                for (key, child) in object {
                    let child_pointer = pointer.child(key);
                    nodes = increment_nodes(nodes, 1, limits.max_nodes, &child_pointer)?;
                    stack.push((child, depth.saturating_add(1), child_pointer));
                }
            }
            Value::Array(array) => {
                for (index, child) in array.iter().enumerate() {
                    stack.push((child, depth.saturating_add(1), pointer.index(index)));
                }
            }
            Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
        }
    }
    Ok(())
}

pub(crate) fn validate_patch_credentials(
    value: &Value,
    source: Option<&ConfigSource>,
    cancellation: &ReloadCancellation,
) -> Result<(), ConfigError> {
    validate_credentials(value, source, cancellation, CredentialContext::MergePatch)
}

pub(crate) fn validate_material_credentials(
    value: &Value,
    cancellation: &ReloadCancellation,
) -> Result<(), ConfigError> {
    validate_credentials(value, None, cancellation, CredentialContext::Material)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CredentialContext {
    MergePatch,
    Material,
}

fn validate_credentials(
    value: &Value,
    source: Option<&ConfigSource>,
    cancellation: &ReloadCancellation,
    root_context: CredentialContext,
) -> Result<(), ConfigError> {
    let mut stack = vec![(value, JsonPointer::root(), root_context)];
    while let Some((current, pointer, context)) = stack.pop() {
        ensure_active_for_source(cancellation, source)?;
        match current {
            Value::Object(object) => {
                for (key, child) in object {
                    let child_pointer = pointer.child(key);
                    if is_credential_like(key) {
                        // Null deletes a member only while traversing an RFC
                        // 7396 patch object. Array contents and merged snapshots
                        // are material values, so null cannot survive there.
                        if context == CredentialContext::MergePatch && child.is_null() {
                            continue;
                        }
                        if !is_strict_secret_reference(child) {
                            return Err(error_for_source(ConfigErrorKind::CredentialValue, source)
                                .at_pointer(child_pointer));
                        }
                        continue;
                    }
                    stack.push((child, child_pointer, context));
                }
            }
            Value::Array(array) => {
                for (index, child) in array.iter().enumerate() {
                    stack.push((child, pointer.index(index), CredentialContext::Material));
                }
            }
            Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
        }
    }
    Ok(())
}

fn increment_nodes(
    current: usize,
    increment: usize,
    maximum: usize,
    pointer: &JsonPointer,
) -> Result<usize, ConfigError> {
    let next = current
        .checked_add(increment)
        .ok_or_else(|| ConfigError::new(ConfigErrorKind::TooManyNodes))?;
    if next > maximum {
        return Err(ConfigError::new(ConfigErrorKind::TooManyNodes).at_pointer(pointer.clone()));
    }
    Ok(next)
}

fn ensure_active(cancellation: &ReloadCancellation) -> Result<(), ConfigError> {
    if cancellation.is_cancelled() {
        Err(ConfigError::new(ConfigErrorKind::Cancelled))
    } else {
        Ok(())
    }
}

fn ensure_active_for_source(
    cancellation: &ReloadCancellation,
    source: Option<&ConfigSource>,
) -> Result<(), ConfigError> {
    if !cancellation.is_cancelled() {
        return Ok(());
    }
    Err(error_for_source(ConfigErrorKind::Cancelled, source))
}

fn error_for_source(kind: ConfigErrorKind, source: Option<&ConfigSource>) -> ConfigError {
    source.map_or_else(
        || ConfigError::new(kind),
        |source| ConfigError::for_source(kind, source),
    )
}

fn is_strict_secret_reference(value: &Value) -> bool {
    let Value::Object(reference) = value else {
        return false;
    };
    reference.len() == 1
        && reference
            .get("secretRef")
            .and_then(Value::as_str)
            .is_some_and(|name| !name.trim().is_empty())
}

// Suffix matching is limited to explicit credential compounds. Bare `key`
// is intentionally absent so ordinary words such as `monkey` cannot match.
const CREDENTIAL_COMPOUND_SUFFIXES: &[&str] = &[
    "accesskey",
    "accesskeys",
    "accesstoken",
    "accesstokens",
    "apikey",
    "apikeys",
    "apitoken",
    "apitokens",
    "authkey",
    "authkeys",
    "authtoken",
    "authtokens",
    "bearertoken",
    "bearertokens",
    "clientkey",
    "clientkeys",
    "clientsecret",
    "clientsecrets",
    "clienttoken",
    "clienttokens",
    "encryptionkey",
    "encryptionkeys",
    "idtoken",
    "idtokens",
    "oauthtoken",
    "oauthtokens",
    "privatekey",
    "privatekeys",
    "refreshtoken",
    "refreshtokens",
    "sessioncookie",
    "sessioncookies",
    "sessiontoken",
    "sessiontokens",
    "signingkey",
    "signingkeys",
];

// These markers are unambiguous as suffixes even when an all-uppercase run
// hides its internal boundary. `key` remains restricted to known compounds,
// and token markers are handled separately to preserve quantity fields.
const CREDENTIAL_MARKER_SUFFIXES: &[&str] = &[
    "authorization",
    "authorizations",
    "bearer",
    "bearers",
    "cookie",
    "cookies",
    "credential",
    "credentials",
    "passphrase",
    "passphrases",
    "passwd",
    "passwds",
    "password",
    "passwords",
    "secret",
    "secrets",
];

const TOKEN_MARKERS: &[&str] = &["token", "tokens"];

// Every word in a token-quantity key must come from this closed vocabulary.
const TOKEN_QUANTITY_TERMS: &[&str] = &[
    "budget",
    "budgets",
    "cached",
    "completion",
    "completions",
    "context",
    "contexts",
    "count",
    "counts",
    "input",
    "inputs",
    "limit",
    "limits",
    "max",
    "maximum",
    "min",
    "minimum",
    "output",
    "outputs",
    "prompt",
    "prompts",
    "reasoning",
    "total",
    "totals",
    "usage",
    "window",
    "windows",
];

fn is_credential_like(key: &str) -> bool {
    let terms = split_key_terms(key);
    if terms.iter().any(|term| {
        matches!(
            term.as_str(),
            "authorization"
                | "authorizations"
                | "bearer"
                | "bearers"
                | "cookie"
                | "cookies"
                | "credential"
                | "credentials"
                | "key"
                | "keys"
                | "passphrase"
                | "passphrases"
                | "passwd"
                | "passwds"
                | "password"
                | "passwords"
                | "secret"
                | "secrets"
        )
    }) {
        return true;
    }

    let compact: String = terms.iter().map(String::as_str).collect();
    let suffix_candidate = strip_numeric_version_suffix(&compact);
    if has_credential_compound_suffix(suffix_candidate)
        || terms
            .iter()
            .any(|term| has_credential_compound_suffix(term))
        || has_credential_marker_suffix(suffix_candidate)
        || terms.iter().any(|term| has_credential_marker_suffix(term))
    {
        return true;
    }

    let has_token_marker = terms.iter().any(|term| has_token_marker_suffix(term))
        || has_token_marker_suffix(suffix_candidate);
    has_token_marker && !is_token_quantity(&compact)
}

fn strip_numeric_version_suffix(compact_key: &str) -> &str {
    // A trailing number, optionally introduced by `v`, is metadata rather than
    // part of the field's semantic name and must not conceal its marker.
    let without_digits = compact_key.trim_end_matches(|character: char| character.is_ascii_digit());
    if without_digits.len() == compact_key.len() {
        return compact_key;
    }
    without_digits.strip_suffix('v').unwrap_or(without_digits)
}

fn has_credential_compound_suffix(compact_key: &str) -> bool {
    // Concatenated and all-uppercase identifiers do not always expose an
    // internal case boundary. A known compound remains credential-like when
    // prefixed by a qualifier such as `cached`.
    CREDENTIAL_COMPOUND_SUFFIXES
        .iter()
        .any(|&suffix| compact_key.ends_with(suffix))
}

fn has_credential_marker_suffix(compact_key: &str) -> bool {
    CREDENTIAL_MARKER_SUFFIXES
        .iter()
        .any(|&marker| compact_key.ends_with(marker))
}

fn has_token_marker_suffix(compact_key: &str) -> bool {
    TOKEN_MARKERS
        .iter()
        .any(|&marker| compact_key.ends_with(marker))
}

fn is_token_quantity(compact_key: &str) -> bool {
    // Segmentation keeps all-uppercase quantity forms such as `CACHEDTOKENS`
    // ordinary while refusing unknown qualifiers such as `cachedJWTTOKEN`.
    let mut pending = vec![(0_usize, false, false)];
    let mut visited = vec![Vec::new(); compact_key.len() + 1];

    while let Some((index, has_token, has_quantity)) = pending.pop() {
        let state = (has_token, has_quantity);
        if visited[index].contains(&state) {
            continue;
        }
        visited[index].push(state);

        if index == compact_key.len() {
            if has_token && has_quantity {
                return true;
            }
            continue;
        }

        let remaining = &compact_key[index..];
        for &marker in TOKEN_MARKERS {
            if remaining.starts_with(marker) {
                pending.push((index + marker.len(), true, has_quantity));
            }
        }
        for &quantity in TOKEN_QUANTITY_TERMS {
            if remaining.starts_with(quantity) {
                pending.push((index + quantity.len(), has_token, true));
            }
        }
    }

    false
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AsciiIdentifierClass {
    Lowercase,
    Uppercase,
    Digit,
    Separator,
}

fn ascii_identifier_class(byte: u8) -> AsciiIdentifierClass {
    match byte {
        b'a'..=b'z' => AsciiIdentifierClass::Lowercase,
        b'A'..=b'Z' => AsciiIdentifierClass::Uppercase,
        b'0'..=b'9' => AsciiIdentifierClass::Digit,
        _ => AsciiIdentifierClass::Separator,
    }
}

fn starts_new_identifier_term(
    previous: Option<AsciiIdentifierClass>,
    current: AsciiIdentifierClass,
    next: Option<AsciiIdentifierClass>,
) -> bool {
    use AsciiIdentifierClass::{Digit, Lowercase, Uppercase};

    matches!(
        (previous, current, next),
        (Some(Lowercase), Uppercase, _)
            | (Some(Uppercase), Uppercase, Some(Lowercase))
            | (Some(Lowercase), Digit, _)
            | (Some(Uppercase), Digit, _)
            | (Some(Digit), Lowercase, _)
            | (Some(Digit), Uppercase, _)
    )
}

fn push_key_term(terms: &mut Vec<String>, term: &mut String) {
    if !term.is_empty() {
        terms.push(std::mem::take(term));
    }
}

fn split_key_terms(key: &str) -> Vec<String> {
    let mut terms = Vec::new();
    let mut term = String::new();
    let mut previous = None;
    let bytes = key.as_bytes();

    // ASCII letters use conventional lower-to-upper and acronym-to-word
    // boundaries. Digits are separate terms so they cannot mask a credential
    // marker. Separators, including non-ASCII UTF-8 bytes, end the current term.
    for (index, &byte) in bytes.iter().enumerate() {
        let current = ascii_identifier_class(byte);
        if current == AsciiIdentifierClass::Separator {
            push_key_term(&mut terms, &mut term);
            previous = None;
            continue;
        }

        let next = bytes
            .get(index.saturating_add(1))
            .copied()
            .map(ascii_identifier_class);
        if starts_new_identifier_term(previous, current, next) {
            push_key_term(&mut terms, &mut term);
        }
        term.push(char::from(byte.to_ascii_lowercase()));
        previous = Some(current);
    }
    push_key_term(&mut terms, &mut term);
    terms
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        is_credential_like, split_key_terms, validate_material_credentials,
        validate_patch_credentials,
    };
    use crate::{ConfigErrorKind, ReloadCancellation};

    #[test]
    fn key_terms_follow_ascii_identifier_boundaries() {
        let cases: &[(&str, &[&str])] = &[
            ("cachedAPIToken", &["cached", "api", "token"]),
            ("cachedOAuthToken", &["cached", "o", "auth", "token"]),
            (
                "HTTPServer2APIKeys",
                &["http", "server", "2", "api", "keys"],
            ),
            ("snake_case-KEYS", &["snake", "case", "keys"]),
            ("api2Key", &["api", "2", "key"]),
            ("apiKey2", &["api", "key", "2"]),
            ("CACHEDAPITOKEN", &["cachedapitoken"]),
        ];

        for &(key, expected) in cases {
            let terms = split_key_terms(key);
            let actual: Vec<&str> = terms.iter().map(String::as_str).collect();
            assert_eq!(actual, expected, "{key}");
        }
    }

    #[test]
    fn credential_key_classifier_handles_adversarial_boundaries() {
        let credential_keys = [
            "apiKey",
            "apiKeys",
            "API_KEYS",
            "accessKeys",
            "ACCESS_TOKEN",
            "cachedAccessToken",
            "CACHED_ACCESS_TOKEN",
            "cachedAPIToken",
            "cachedAPIKey",
            "cachedOAuthToken",
            "cachedAPITokens",
            "cached_api_token",
            "cached-api-key",
            "CACHED_API_TOKEN",
            "CACHED-API-KEY",
            "cachedAPITOKEN",
            "CACHEDAPITOKEN",
            "CACHEDAPIKEY",
            "CACHEDOAUTHTOKEN",
            "cachedAPITOKEN2",
            "CACHEDAPITOKEN2",
            "CACHEDAPIKEY_V2",
            "CACHEDAPIKEYV2",
            "cachedAPITOKEN2Value",
            "CACHEDAPIKEY_V2_BACKUP",
            "cachedAPISECRET",
            "CACHEDAPISECRET",
            "cachedAPISECRET2Value",
            "cachedJWTTOKEN",
            "CACHEDJWTTOKEN",
            "cachedJWTTOKEN2",
            "client-secret",
            "authorization",
            "secretRef",
            "api2Key",
            "apiKey2",
            "max2Tokens",
        ];
        for key in credential_keys {
            assert!(is_credential_like(key), "credential key bypassed: {key}");
        }

        let ordinary_keys = [
            "maxTokens",
            "max_tokens",
            "MAX_TOKENS",
            "tokenBudget",
            "token-budget",
            "TOKEN_BUDGET",
            "cachedTokens",
            "cached_tokens",
            "CACHED_TOKENS",
            "CACHEDTOKENS",
            "PROMPTTOKENS",
            "CONTEXTWINDOWTOKENS",
            "tokenCount",
            "token_count",
            "promptTokens",
            "completion_tokens",
            "totalTokens",
            "monkey",
            "MONKEY",
            "keyboardLayout",
            "KEYBOARD_LAYOUT",
            "cachedAPIKeyboard",
            "tokenizer",
            "version2",
        ];
        for key in ordinary_keys {
            assert!(!is_credential_like(key), "ordinary key rejected: {key}");
        }
    }

    #[test]
    fn credential_null_is_valid_only_in_merge_patch_objects() {
        let cancellation = ReloadCancellation::new();
        let deletion = json!({"apiKey": null});
        validate_patch_credentials(&deletion, None, &cancellation).expect("patch deletion marker");
        let error = validate_material_credentials(&deletion, &cancellation)
            .expect_err("material credential null");
        assert_eq!(error.kind(), ConfigErrorKind::CredentialValue);

        let array_replacement = json!({"providers": [{"apiKey": null}]});
        let error = validate_patch_credentials(&array_replacement, None, &cancellation)
            .expect_err("array contents are material");
        assert_eq!(error.kind(), ConfigErrorKind::CredentialValue);
    }
}

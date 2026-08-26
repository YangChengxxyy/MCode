//! Bounded validation, sanitization, and redaction for untrusted MCP data.

// Rust guideline compliant 2026-08-20.

use jsonschema::{Retrieve, Uri};
use serde_json::{Map, Value};

use crate::{
    config::OutputLimits,
    error::{Error, ErrorKind, Recovery, Result},
    identity::ServerName,
};

const REDACTED: &str = "[REDACTED]";
const REFERENCE_KEYWORDS: [&str; 3] = ["$ref", "$dynamicRef", "$recursiveRef"];

#[derive(Debug, Clone, Copy)]
struct RejectExternalRetriever;

impl Retrieve for RejectExternalRetriever {
    fn retrieve(
        &self,
        _uri: &Uri<String>,
    ) -> std::result::Result<Value, Box<dyn std::error::Error + Send + Sync>> {
        Err(Box::new(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "external JSON Schema retrieval is disabled",
        )))
    }
}

/// Bounds a remote JSON value and returns a redacted copy for logs or diagnostics.
///
/// Secret-like keys and bearer tokens are replaced in this copy only. Real
/// `tools/call` arguments must be validated with [`validate_tool_arguments`],
/// which never rewrites caller-supplied values.
///
/// # Errors
///
/// Returns a validation error when size, depth, node, or string limits are exceeded.
pub fn sanitize_json(server: &ServerName, value: &Value, limits: &OutputLimits) -> Result<Value> {
    sanitize_json_with_key_redaction(server, value, limits, true)
}

fn sanitize_schema_json(
    server: &ServerName,
    value: &Value,
    limits: &OutputLimits,
) -> Result<Value> {
    sanitize_json_with_key_redaction(server, value, limits, false)
}

fn sanitize_json_with_key_redaction(
    server: &ServerName,
    value: &Value,
    limits: &OutputLimits,
    redact_secret_keys: bool,
) -> Result<Value> {
    let encoded_size = serde_json::to_vec(value)
        .map_err(|_| validation_error(server, "remote JSON could not be encoded"))?
        .len();
    if encoded_size > limits.max_output_bytes {
        return Err(validation_error(
            server,
            "remote JSON exceeds the configured output cap",
        ));
    }
    let mut budget = JsonBudget {
        nodes: limits.max_json_nodes,
        max_depth: limits.max_json_depth,
        max_string_bytes: limits.max_string_bytes,
    };
    sanitize_value(server, value, &mut budget, 0, None, redact_secret_keys)
}

fn enforce_json_bounds(server: &ServerName, value: &Value, limits: &OutputLimits) -> Result<()> {
    let encoded_size = serde_json::to_vec(value)
        .map_err(|_| validation_error(server, "remote JSON could not be encoded"))?
        .len();
    if encoded_size > limits.max_output_bytes {
        return Err(validation_error(
            server,
            "remote JSON exceeds the configured output cap",
        ));
    }
    let mut budget = JsonBudget {
        nodes: limits.max_json_nodes,
        max_depth: limits.max_json_depth,
        max_string_bytes: limits.max_string_bytes,
    };
    consume_value_budget(server, value, &mut budget, 0)
}

/// Validates an arbitrary JSON Schema without resolving external references.
///
/// This accepts non-object schemas because current MCP output schemas may use
/// boolean schemas and other valid JSON Schema forms.
///
/// # Errors
///
/// Returns a validation error for malformed, oversized, or external-reference schemas.
pub fn validate_json_schema(
    server: &ServerName,
    schema: &Value,
    limits: &OutputLimits,
) -> Result<Value> {
    let sanitized = sanitize_schema_json(server, schema, limits)?;
    reject_external_references(server, &sanitized)?;
    compile_schema(&sanitized)
        .map_err(|_| validation_error(server, "value is not a valid JSON Schema"))?;
    Ok(sanitized)
}

/// Validates a tool input schema before it can reach an upper registry.
///
/// # Errors
///
/// Returns a validation error for malformed, oversized, external-reference, or
/// non-object input schemas.
pub fn validate_tool_schema(
    server: &ServerName,
    schema: &Value,
    limits: &OutputLimits,
) -> Result<Value> {
    let sanitized = validate_json_schema(server, schema, limits)?;
    if !sanitized.is_object() {
        return Err(validation_error(
            server,
            "tool inputSchema must be a JSON object",
        ));
    }
    Ok(sanitized)
}

/// Validates arguments against a previously bounded tool schema.
///
/// Size, depth, and node limits are enforced, but argument values are not
/// redacted or otherwise rewritten. Diagnostic copies must go through
/// [`sanitize_json`].
///
/// # Errors
///
/// Returns a generic validation error without embedding arguments or validator diagnostics.
pub fn validate_tool_arguments(
    server: &ServerName,
    schema: &Value,
    arguments: &Value,
    limits: &OutputLimits,
) -> Result<Value> {
    enforce_json_bounds(server, arguments, limits)?;
    let validator = compile_schema(schema)
        .map_err(|_| validation_error(server, "tool schema could not be compiled"))?;
    if !validator.is_valid(arguments) {
        return Err(validation_error(
            server,
            "tool arguments do not match the negotiated input schema",
        ));
    }
    Ok(arguments.clone())
}

/// Validates and sanitizes one tool-call result.
///
/// # Errors
///
/// Returns a validation error for excessive or malformed content blocks.
pub fn validate_tool_result(
    server: &ServerName,
    result: &Value,
    limits: &OutputLimits,
) -> Result<Value> {
    let sanitized = sanitize_json(server, result, limits)?;
    if let Some(content) = sanitized.get("content") {
        validate_content_array(server, content, limits)?;
    }
    Ok(sanitized)
}

/// Validates and sanitizes one resource-read result.
///
/// # Errors
///
/// Returns a validation error for malformed or excessive resource contents.
pub fn validate_resource_result(
    server: &ServerName,
    result: &Value,
    limits: &OutputLimits,
) -> Result<Value> {
    let sanitized = sanitize_json(server, result, limits)?;
    let Some(contents) = sanitized.get("contents").and_then(Value::as_array) else {
        return Err(validation_error(
            server,
            "resources/read result must contain a contents array",
        ));
    };
    if contents.len() > limits.max_content_blocks {
        return Err(validation_error(
            server,
            "resource result contains too many content blocks",
        ));
    }
    for content in contents {
        let Some(object) = content.as_object() else {
            return Err(validation_error(
                server,
                "resource content must be an object",
            ));
        };
        if object.get("uri").and_then(Value::as_str).is_none()
            || (object.get("text").and_then(Value::as_str).is_none()
                && object.get("blob").and_then(Value::as_str).is_none())
        {
            return Err(validation_error(
                server,
                "resource content is missing uri and text/blob",
            ));
        }
    }
    Ok(sanitized)
}

/// Validates and sanitizes one prompt result.
///
/// # Errors
///
/// Returns a validation error for malformed messages or content blocks.
pub fn validate_prompt_result(
    server: &ServerName,
    result: &Value,
    limits: &OutputLimits,
) -> Result<Value> {
    let sanitized = sanitize_json(server, result, limits)?;
    let Some(messages) = sanitized.get("messages").and_then(Value::as_array) else {
        return Err(validation_error(
            server,
            "prompts/get result must contain a messages array",
        ));
    };
    if messages.len() > limits.max_content_blocks {
        return Err(validation_error(
            server,
            "prompt result has too many messages",
        ));
    }
    for message in messages {
        let Some(content) = message.get("content") else {
            return Err(validation_error(
                server,
                "prompt message is missing content",
            ));
        };
        validate_content_block(server, content)?;
    }
    Ok(sanitized)
}

/// Validates a completion response and its value count.
///
/// # Errors
///
/// Returns a validation error for malformed or excessive completions.
pub fn validate_completion_result(
    server: &ServerName,
    result: &Value,
    limits: &OutputLimits,
) -> Result<Value> {
    let sanitized = sanitize_json(server, result, limits)?;
    let values = sanitized
        .get("completion")
        .and_then(|completion| completion.get("values"))
        .and_then(Value::as_array)
        .ok_or_else(|| validation_error(server, "completion result is malformed"))?;
    if values.len() > limits.max_catalog_items
        || values.iter().any(|value| value.as_str().is_none())
    {
        return Err(validation_error(
            server,
            "completion values exceed safe bounds",
        ));
    }
    Ok(sanitized)
}

/// Rejects form elicitation schemas that request likely credentials.
///
/// # Errors
///
/// Returns a trust error when any schema property is secret-like.
pub fn validate_elicitation_schema(
    server: &ServerName,
    schema: &Value,
    limits: &OutputLimits,
) -> Result<Value> {
    let schema = sanitize_schema_json(server, schema, limits)?;
    reject_external_references(server, &schema)?;
    if contains_secret_property(&schema) {
        return Err(Error::new(
            ErrorKind::Trust,
            Recovery::Fatal,
            "elicitation forms may not request credentials or secret values",
        )
        .with_server(server.clone()));
    }
    compile_schema(&schema)
        .map_err(|_| validation_error(server, "elicitation requestedSchema is invalid"))?;
    Ok(schema)
}

/// Validates accepted elicitation content against its schema and secret policy.
///
/// # Errors
///
/// Returns a validation or trust error for schema mismatches or secret-like keys.
pub fn validate_elicitation_content(
    server: &ServerName,
    schema: &Value,
    content: &Value,
    limits: &OutputLimits,
) -> Result<Value> {
    let content = sanitize_json(server, content, limits)?;
    if contains_secret_property(&content) {
        return Err(Error::new(
            ErrorKind::Trust,
            Recovery::Fatal,
            "elicitation response contains a forbidden secret-like field",
        )
        .with_server(server.clone()));
    }
    let validator = compile_schema(schema)
        .map_err(|_| validation_error(server, "elicitation schema could not be compiled"))?;
    if !validator.is_valid(&content) {
        return Err(validation_error(
            server,
            "elicitation response does not match requestedSchema",
        ));
    }
    Ok(content)
}

/// Strips ANSI/control sequences and truncates at a UTF-8 boundary.
#[must_use]
pub fn sanitize_text(input: &str, max_bytes: usize) -> String {
    let mut output = String::with_capacity(input.len().min(max_bytes));
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
        if output.len() + character.len_utf8() > max_bytes {
            output.push('…');
            break;
        }
        output.push(character);
    }
    redact_bearer_tokens(&output)
}

/// Validates one remote catalog label and returns sanitized text.
///
/// # Errors
///
/// Returns a validation error when the original exceeds the string cap.
pub fn validate_catalog_text(
    server: &ServerName,
    value: &str,
    limits: &OutputLimits,
) -> Result<String> {
    if value.len() > limits.max_string_bytes {
        return Err(validation_error(
            server,
            "catalog string exceeds configured cap",
        ));
    }
    Ok(sanitize_text(value, limits.max_string_bytes))
}

struct JsonBudget {
    nodes: usize,
    max_depth: usize,
    max_string_bytes: usize,
}

fn sanitize_value(
    server: &ServerName,
    value: &Value,
    budget: &mut JsonBudget,
    depth: usize,
    key: Option<&str>,
    redact_secret_keys: bool,
) -> Result<Value> {
    if depth > budget.max_depth {
        return Err(validation_error(server, "remote JSON nesting is too deep"));
    }
    if budget.nodes == 0 {
        return Err(validation_error(
            server,
            "remote JSON contains too many nodes",
        ));
    }
    budget.nodes -= 1;

    if redact_secret_keys && key.is_some_and(is_secret_key) {
        consume_value_contents(server, value, budget, depth)?;
        return Ok(Value::String(REDACTED.to_owned()));
    }

    match value {
        Value::Null | Value::Bool(_) | Value::Number(_) => Ok(value.clone()),
        Value::String(string) => {
            if string.len() > budget.max_string_bytes {
                return Err(validation_error(
                    server,
                    "remote JSON string exceeds configured cap",
                ));
            }
            Ok(Value::String(sanitize_text(
                string,
                budget.max_string_bytes,
            )))
        }
        Value::Array(values) => values
            .iter()
            .map(|value| sanitize_value(server, value, budget, depth + 1, None, redact_secret_keys))
            .collect::<Result<Vec<_>>>()
            .map(Value::Array),
        Value::Object(values) => {
            let mut output = Map::new();
            for (name, value) in values {
                if name.len() > budget.max_string_bytes || name.chars().any(char::is_control) {
                    return Err(validation_error(server, "remote JSON key is unsafe"));
                }
                output.insert(
                    sanitize_text(name, budget.max_string_bytes),
                    sanitize_value(
                        server,
                        value,
                        budget,
                        depth + 1,
                        Some(name),
                        redact_secret_keys,
                    )?,
                );
            }
            Ok(Value::Object(output))
        }
    }
}

fn consume_value_budget(
    server: &ServerName,
    value: &Value,
    budget: &mut JsonBudget,
    depth: usize,
) -> Result<()> {
    if depth > budget.max_depth {
        return Err(validation_error(server, "remote JSON nesting is too deep"));
    }
    if budget.nodes == 0 {
        return Err(validation_error(
            server,
            "remote JSON contains too many nodes",
        ));
    }
    budget.nodes -= 1;
    consume_value_contents(server, value, budget, depth)
}

fn consume_value_contents(
    server: &ServerName,
    value: &Value,
    budget: &mut JsonBudget,
    depth: usize,
) -> Result<()> {
    match value {
        Value::String(string) if string.len() > budget.max_string_bytes => Err(validation_error(
            server,
            "remote JSON string exceeds configured cap",
        )),
        Value::Array(values) => {
            for value in values {
                consume_value_budget(server, value, budget, depth + 1)?;
            }
            Ok(())
        }
        Value::Object(values) => {
            for (name, value) in values {
                if name.len() > budget.max_string_bytes || name.chars().any(char::is_control) {
                    return Err(validation_error(server, "remote JSON key is unsafe"));
                }
                consume_value_budget(server, value, budget, depth + 1)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn validate_content_array(
    server: &ServerName,
    content: &Value,
    limits: &OutputLimits,
) -> Result<()> {
    let Some(blocks) = content.as_array() else {
        return Err(validation_error(server, "tool content must be an array"));
    };
    if blocks.len() > limits.max_content_blocks {
        return Err(validation_error(
            server,
            "tool result contains too many blocks",
        ));
    }
    for block in blocks {
        validate_content_block(server, block)?;
    }
    Ok(())
}

fn validate_content_block(server: &ServerName, block: &Value) -> Result<()> {
    let Some(object) = block.as_object() else {
        return Err(validation_error(server, "content block must be an object"));
    };
    let Some(kind) = object.get("type").and_then(Value::as_str) else {
        return Err(validation_error(server, "content block has no type"));
    };
    let valid = match kind {
        "text" => object.get("text").and_then(Value::as_str).is_some(),
        "image" | "audio" => {
            object.get("data").and_then(Value::as_str).is_some()
                && object.get("mimeType").and_then(Value::as_str).is_some()
        }
        "resource" => object.get("resource").and_then(Value::as_object).is_some(),
        "resource_link" => {
            object.get("uri").and_then(Value::as_str).is_some()
                && object.get("name").and_then(Value::as_str).is_some()
        }
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err(validation_error(
            server,
            "content block type or required fields are invalid",
        ))
    }
}

fn compile_schema(
    schema: &Value,
) -> std::result::Result<jsonschema::Validator, jsonschema::ValidationError<'static>> {
    jsonschema::options()
        .with_retriever(RejectExternalRetriever)
        .build(schema)
}

fn reject_external_references(server: &ServerName, value: &Value) -> Result<()> {
    match value {
        Value::Array(values) => {
            for value in values {
                reject_external_references(server, value)?;
            }
        }
        Value::Object(values) => {
            for keyword in REFERENCE_KEYWORDS {
                if let Some(reference) = values.get(keyword).and_then(Value::as_str)
                    && !reference.starts_with('#')
                {
                    return Err(validation_error(
                        server,
                        "external JSON Schema references are not permitted",
                    ));
                }
            }
            for value in values.values() {
                reject_external_references(server, value)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn contains_secret_property(value: &Value) -> bool {
    match value {
        Value::Array(values) => values.iter().any(contains_secret_property),
        Value::Object(values) => values.iter().any(|(key, value)| {
            is_secret_key(key)
                || (key == "properties"
                    && value.as_object().is_some_and(|properties| {
                        properties.keys().any(|name| is_secret_key(name))
                    }))
                || contains_secret_property(value)
        }),
        _ => false,
    }
}

fn is_secret_key(key: &str) -> bool {
    let normalized: String = key
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect();
    [
        "authorization",
        "apikey",
        "accesstoken",
        "refreshtoken",
        "token",
        "password",
        "passwd",
        "secret",
        "clientsecret",
        "credential",
        "privatekey",
    ]
    .iter()
    .any(|needle| normalized == *needle || normalized.ends_with(needle))
}

fn redact_bearer_tokens(input: &str) -> String {
    let lower = input.to_ascii_lowercase();
    let mut output = String::with_capacity(input.len());
    let mut copied_until = 0;
    let mut search_from = 0;

    while let Some(relative_index) = lower[search_from..].find("bearer") {
        let marker_end = search_from + relative_index + "bearer".len();
        let Some((whitespace_offset, _)) = input[marker_end..]
            .char_indices()
            .next()
            .filter(|(_, character)| character.is_whitespace())
        else {
            search_from = marker_end;
            continue;
        };
        let marker_end = marker_end + whitespace_offset;
        let Some(token_offset) = input[marker_end..]
            .char_indices()
            .find_map(|(offset, character)| (!character.is_whitespace()).then_some(offset))
        else {
            break;
        };
        let token_start = marker_end + token_offset;
        let token_end = input[token_start..]
            .char_indices()
            .find_map(|(offset, character)| {
                (!is_bearer_token_character(character)).then_some(token_start + offset)
            })
            .unwrap_or(input.len());
        let token_end = if token_end == token_start {
            input[token_start..]
                .find(char::is_whitespace)
                .map_or(input.len(), |offset| token_start + offset)
        } else {
            token_end
        };
        output.push_str(&input[copied_until..token_start]);
        output.push_str(REDACTED);
        copied_until = token_end;
        search_from = token_end;
    }

    if copied_until == 0 {
        input.to_owned()
    } else {
        output.push_str(&input[copied_until..]);
        output
    }
}

fn is_bearer_token_character(character: char) -> bool {
    character.is_ascii_alphanumeric()
        || matches!(character, '-' | '.' | '_' | '~' | '+' | '/' | '=')
}

fn validation_error(server: &ServerName, message: impl AsRef<str>) -> Error {
    Error::new(ErrorKind::Validation, Recovery::Fatal, message).with_server(server.clone())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn server() -> ServerName {
        ServerName::new("malicious").unwrap()
    }

    #[test]
    fn deep_schema_and_external_references_are_rejected() {
        let limits = OutputLimits {
            max_json_depth: 4,
            ..OutputLimits::default()
        };
        let deep = json!({"a": {"b": {"c": {"d": {"e": {}}}}}});
        assert!(validate_tool_schema(&server(), &deep, &limits).is_err());
        for keyword in REFERENCE_KEYWORDS {
            let schema = json!({keyword: "https://attacker.invalid/schema"});
            assert!(
                validate_tool_schema(&server(), &schema, &OutputLimits::default()).is_err(),
                "{keyword} must not resolve an external resource"
            );
        }
    }

    #[test]
    fn output_is_redacted_and_ansi_is_removed() {
        let value = json!({
            "content": [{
                "type":"text",
                "text":"\u{1b}[31mBearer abc123\u{1b}[0m then BEARER second-secret,Bearer third-secret;Bearer\tfourth-secret"
            }],
            "token": {"value": "literal-secret"}
        });
        let value = validate_tool_result(&server(), &value, &OutputLimits::default()).unwrap();
        let encoded = value.to_string();
        assert!(!encoded.contains("abc123"));
        assert!(!encoded.contains("second-secret"));
        assert!(!encoded.contains("third-secret"));
        assert!(!encoded.contains("fourth-secret"));
        assert!(!encoded.contains("literal-secret"));
        assert_eq!(value["token"], REDACTED);
        assert!(!encoded.contains("\\u001b"));
    }

    #[test]
    fn secret_named_tool_properties_remain_schemas() {
        let schema = json!({
            "type":"object",
            "properties":{"token":{"type":"string"}}
        });
        let validated = validate_tool_schema(&server(), &schema, &OutputLimits::default()).unwrap();
        assert_eq!(validated["properties"]["token"]["type"], "string");
    }

    #[test]
    fn call_tool_arguments_keep_secret_named_values() {
        let schema = json!({
            "type":"object",
            "properties":{
                "token":{"type":"string"},
                "prompt":{"type":"string"}
            },
            "required":["token"]
        });
        let arguments = json!({
            "token":"real-tool-token-value",
            "prompt":"Bearer abc123"
        });
        let validated =
            validate_tool_arguments(&server(), &schema, &arguments, &OutputLimits::default())
                .unwrap();
        assert_eq!(validated, arguments);
        assert_eq!(validated["token"], "real-tool-token-value");
        assert_eq!(validated["prompt"], "Bearer abc123");

        let diagnostic = sanitize_json(&server(), &arguments, &OutputLimits::default()).unwrap();
        assert_eq!(diagnostic["token"], REDACTED);
        assert!(!diagnostic.to_string().contains("real-tool-token-value"));
        assert!(!diagnostic.to_string().contains("abc123"));

        let error = validate_tool_arguments(
            &server(),
            &schema,
            &json!({"token":1}),
            &OutputLimits::default(),
        )
        .unwrap_err();
        assert!(!error.message().contains("real-tool-token-value"));
        assert!(!error.to_string().contains("abc123"));
        assert!(!error.message().contains("token"));
    }

    #[test]
    fn elicitation_cannot_request_secrets() {
        let schema = json!({
            "type":"object",
            "properties":{"apiKey":{"type":"string"}}
        });
        assert!(validate_elicitation_schema(&server(), &schema, &OutputLimits::default()).is_err());
    }
}

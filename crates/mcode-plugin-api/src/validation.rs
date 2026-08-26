//! Internal bounded JSON and text validation.

// Rust guideline compliant 2026-08-26.

use std::fmt::Formatter;

use serde::de::{Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Number, Value};

use crate::limits::{MAX_JSON_DEPTH, MAX_JSON_NODES};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ValueValidationError {
    TooLarge,
    TooDeep,
    TooManyNodes,
    InvalidKey,
    Serialization,
}

pub(crate) fn parse_strict_json(bytes: &[u8]) -> Result<Value, serde_json::Error> {
    serde_json::from_slice::<StrictValue>(bytes).map(|value| value.0)
}

struct StrictValue(Value);

impl<'de> Deserialize<'de> for StrictValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(StrictValueVisitor)
    }
}

struct StrictValueVisitor;

impl<'de> Visitor<'de> for StrictValueVisitor {
    type Value = StrictValue;

    fn expecting(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("a JSON value without duplicate object keys")
    }

    fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E> {
        Ok(StrictValue(Value::Bool(value)))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E> {
        Ok(StrictValue(Value::Number(Number::from(value))))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
        Ok(StrictValue(Value::Number(Number::from(value))))
    }

    fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Number::from_f64(value)
            .map(Value::Number)
            .map(StrictValue)
            .ok_or_else(|| E::custom("non-finite JSON number"))
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(StrictValue(Value::String(value.to_owned())))
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E> {
        Ok(StrictValue(Value::String(value)))
    }

    fn visit_none<E>(self) -> Result<Self::Value, E> {
        Ok(StrictValue(Value::Null))
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(StrictValue(Value::Null))
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut values = Vec::new();
        while let Some(value) = sequence.next_element::<StrictValue>()? {
            values.push(value.0);
        }
        Ok(StrictValue(Value::Array(values)))
    }

    fn visit_map<A>(self, mut object: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut values = Map::new();
        while let Some((key, value)) = object.next_entry::<String, StrictValue>()? {
            if values.insert(key, value.0).is_some() {
                return Err(serde::de::Error::custom("duplicate JSON object key"));
            }
        }
        Ok(StrictValue(Value::Object(values)))
    }
}

pub(crate) fn validate_json_value(
    value: &Value,
    max_bytes: usize,
) -> Result<usize, ValueValidationError> {
    let bytes = serde_json::to_vec(value)
        .map_err(|_| ValueValidationError::Serialization)?
        .len();
    if bytes > max_bytes {
        return Err(ValueValidationError::TooLarge);
    }
    let mut stack = vec![(value, 1usize)];
    let mut nodes = 0usize;
    while let Some((current, depth)) = stack.pop() {
        nodes = nodes.saturating_add(1);
        if nodes > MAX_JSON_NODES {
            return Err(ValueValidationError::TooManyNodes);
        }
        if depth > MAX_JSON_DEPTH {
            return Err(ValueValidationError::TooDeep);
        }
        match current {
            Value::Array(values) => {
                stack.extend(values.iter().map(|child| (child, depth + 1)));
            }
            Value::Object(values) => {
                for (key, child) in values {
                    if key.is_empty() || key.len() > 256 || key.chars().any(char::is_control) {
                        return Err(ValueValidationError::InvalidKey);
                    }
                    stack.push((child, depth + 1));
                }
            }
            Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
        }
    }
    Ok(bytes)
}

pub(crate) fn valid_public_text(value: &str, max_bytes: usize) -> bool {
    !value.is_empty()
        && value.len() <= max_bytes
        && !value
            .chars()
            .any(|character| character.is_control() && !matches!(character, '\n' | '\t'))
}

pub(crate) fn is_terminal_control(character: char) -> bool {
    if matches!(character, '\n' | '\t') {
        return false;
    }
    character.is_control() || matches!(character, '\u{009b}' | '\u{009d}')
}

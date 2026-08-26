//! Strict, bounded JSON envelope parsing with duplicate-key detection.

// Rust guideline compliant 2026-08-26

use std::collections::BTreeSet;
use std::fmt::{self, Formatter};

use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Number, Value};

use crate::{
    ConfigError, ConfigErrorKind, ConfigLimits, ConfigSource, FORMAT_VERSION, JsonPointer,
    ReloadCancellation,
};

pub(crate) const CONFIG_FIELD: &str = "config";
pub(crate) const FORMAT_VERSION_FIELD: &str = "formatVersion";

#[derive(Debug, Clone, Copy)]
enum ParseViolationKind {
    Cancelled,
    TooDeep,
    TooManyNodes,
    DuplicateKey,
}

#[derive(Debug)]
struct ParseViolation {
    kind: ParseViolationKind,
    pointer: Option<JsonPointer>,
}

struct ParseState<'a> {
    limits: ConfigLimits,
    cancellation: &'a ReloadCancellation,
    nodes: usize,
    violation: Option<ParseViolation>,
}

impl ParseState<'_> {
    fn enter_value<E: de::Error>(&mut self, depth: usize, pointer: &JsonPointer) -> Result<(), E> {
        if self.cancellation.is_cancelled() {
            return Err(self.reject(ParseViolationKind::Cancelled, Some(pointer.clone())));
        }
        if depth > self.limits.max_depth {
            return Err(self.reject(ParseViolationKind::TooDeep, Some(pointer.clone())));
        }
        self.count_node(pointer)
    }

    fn count_member<E: de::Error>(&mut self, pointer: &JsonPointer) -> Result<(), E> {
        self.count_node(pointer)
    }

    fn count_node<E: de::Error>(&mut self, pointer: &JsonPointer) -> Result<(), E> {
        self.nodes = self.nodes.checked_add(1).ok_or_else(|| {
            self.reject::<E>(ParseViolationKind::TooManyNodes, Some(pointer.clone()))
        })?;
        if self.nodes > self.limits.max_nodes {
            return Err(self.reject(ParseViolationKind::TooManyNodes, Some(pointer.clone())));
        }
        Ok(())
    }

    fn reject<E: de::Error>(
        &mut self,
        kind: ParseViolationKind,
        pointer: Option<JsonPointer>,
    ) -> E {
        if self.violation.is_none() {
            self.violation = Some(ParseViolation { kind, pointer });
        }
        E::custom("strict configuration JSON rejected")
    }
}

struct ValueSeed<'state, 'cancel> {
    state: &'state mut ParseState<'cancel>,
    depth: usize,
    pointer: JsonPointer,
}

impl<'de> DeserializeSeed<'de> for ValueSeed<'_, '_> {
    type Value = Value;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        self.state.enter_value(self.depth, &self.pointer)?;
        deserializer.deserialize_any(ValueVisitor {
            state: self.state,
            depth: self.depth,
            pointer: self.pointer,
        })
    }
}

struct ValueVisitor<'state, 'cancel> {
    state: &'state mut ParseState<'cancel>,
    depth: usize,
    pointer: JsonPointer,
}

impl<'de> Visitor<'de> for ValueVisitor<'_, '_> {
    type Value = Value;

    fn expecting(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str("a strict JSON value")
    }

    fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(Value::Bool(value))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(Value::Number(Number::from(value)))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(Value::Number(Number::from(value)))
    }

    fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Number::from_f64(value)
            .map(Value::Number)
            .ok_or_else(|| E::custom("non-finite JSON number"))
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(Value::String(value.to_owned()))
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(Value::String(value))
    }

    fn visit_none<E>(self) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(Value::Null)
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(Value::Null)
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let child_depth = self.depth.checked_add(1).ok_or_else(|| {
            self.state
                .reject::<A::Error>(ParseViolationKind::TooDeep, Some(self.pointer.clone()))
        })?;
        let capacity = sequence
            .size_hint()
            .unwrap_or(0)
            .min(self.state.limits.max_nodes);
        let mut values = Vec::with_capacity(capacity);
        let mut index = 0_usize;
        loop {
            let pointer = self.pointer.index(index);
            let seed = ValueSeed {
                state: self.state,
                depth: child_depth,
                pointer,
            };
            match sequence.next_element_seed(seed)? {
                Some(value) => values.push(value),
                None => break,
            }
            index = index.checked_add(1).ok_or_else(|| {
                self.state.reject::<A::Error>(
                    ParseViolationKind::TooManyNodes,
                    Some(self.pointer.clone()),
                )
            })?;
        }
        Ok(Value::Array(values))
    }

    fn visit_map<A>(self, mut access: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let child_depth = self.depth.checked_add(1).ok_or_else(|| {
            self.state
                .reject::<A::Error>(ParseViolationKind::TooDeep, Some(self.pointer.clone()))
        })?;
        let mut keys = BTreeSet::new();
        let mut values = Map::new();
        while let Some(key) = access.next_key::<String>()? {
            let pointer = self.pointer.child(&key);
            self.state.count_member::<A::Error>(&pointer)?;
            if !keys.insert(key.clone()) {
                return Err(self
                    .state
                    .reject(ParseViolationKind::DuplicateKey, Some(pointer)));
            }
            let value = access.next_value_seed(ValueSeed {
                state: self.state,
                depth: child_depth,
                pointer,
            })?;
            values.insert(key, value);
        }
        Ok(Value::Object(values))
    }
}

pub(crate) fn parse_envelope(
    bytes: &[u8],
    source: &ConfigSource,
    limits: ConfigLimits,
    cancellation: &ReloadCancellation,
) -> Result<Value, ConfigError> {
    if std::str::from_utf8(bytes).is_err() {
        return Err(ConfigError::for_source(ConfigErrorKind::NonUtf8, source));
    }

    let mut state = ParseState {
        limits,
        cancellation,
        nodes: 0,
        violation: None,
    };
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    // The envelope is structural and does not consume the caller's configured
    // value-depth budget. Its children begin at depth one.
    let parsed = ValueSeed {
        state: &mut state,
        depth: 0,
        pointer: JsonPointer::root(),
    }
    .deserialize(&mut deserializer);

    let value = match parsed {
        Ok(value) => value,
        Err(_) => return Err(classify_parse_failure(source, state.violation)),
    };
    if deserializer.end().is_err() {
        return Err(ConfigError::for_source(
            ConfigErrorKind::InvalidJson,
            source,
        ));
    }

    extract_envelope(value, source)
}

fn classify_parse_failure(source: &ConfigSource, violation: Option<ParseViolation>) -> ConfigError {
    let Some(violation) = violation else {
        return ConfigError::for_source(ConfigErrorKind::InvalidJson, source);
    };
    let kind = match violation.kind {
        ParseViolationKind::Cancelled => ConfigErrorKind::Cancelled,
        ParseViolationKind::TooDeep => ConfigErrorKind::TooDeep,
        ParseViolationKind::TooManyNodes => ConfigErrorKind::TooManyNodes,
        ParseViolationKind::DuplicateKey => ConfigErrorKind::DuplicateKey,
    };
    let error = ConfigError::for_source(kind, source);
    match violation.pointer {
        Some(pointer) => error.at_pointer(pointer),
        None => error,
    }
}

fn extract_envelope(value: Value, source: &ConfigSource) -> Result<Value, ConfigError> {
    let Value::Object(mut envelope) = value else {
        return Err(ConfigError::for_source(
            ConfigErrorKind::InvalidEnvelope,
            source,
        ));
    };
    if envelope.len() != 2
        || !envelope.contains_key(FORMAT_VERSION_FIELD)
        || !envelope.contains_key(CONFIG_FIELD)
    {
        return Err(ConfigError::for_source(
            ConfigErrorKind::InvalidEnvelope,
            source,
        ));
    }

    let version = envelope
        .remove(FORMAT_VERSION_FIELD)
        .and_then(|value| value.as_u64());
    let Some(version) = version else {
        return Err(ConfigError::for_source(
            ConfigErrorKind::InvalidEnvelope,
            source,
        ));
    };
    if version != u64::from(FORMAT_VERSION) {
        return Err(ConfigError::for_source(
            ConfigErrorKind::UnsupportedFormatVersion,
            source,
        ));
    }

    envelope
        .remove(CONFIG_FIELD)
        .ok_or_else(|| ConfigError::for_source(ConfigErrorKind::InvalidEnvelope, source))
}

//! RFC 7396 JSON Merge Patch with per-pointer source provenance.

// Rust guideline compliant 2026-08-26

use std::collections::BTreeMap;

use serde_json::{Map, Value};

use crate::{ConfigError, ConfigErrorKind, ConfigSource, JsonPointer, ReloadCancellation};

pub(crate) type ProvenanceMap = BTreeMap<JsonPointer, ConfigSource>;

pub(crate) fn merge_patch(
    target: &mut Value,
    patch: &Value,
    source: &ConfigSource,
    provenance: &mut ProvenanceMap,
    cancellation: &ReloadCancellation,
) -> Result<(), ConfigError> {
    merge_at(
        target,
        patch,
        source,
        &JsonPointer::root(),
        provenance,
        cancellation,
    )
}

fn merge_at(
    target: &mut Value,
    patch: &Value,
    source: &ConfigSource,
    pointer: &JsonPointer,
    provenance: &mut ProvenanceMap,
    cancellation: &ReloadCancellation,
) -> Result<(), ConfigError> {
    ensure_active(cancellation, source)?;
    match patch {
        Value::Object(patch_object) => {
            if !target.is_object() {
                *target = Value::Object(Map::new());
                remove_provenance(provenance, pointer);
                provenance.insert(pointer.clone(), source.clone());
            } else if !provenance.contains_key(pointer) {
                provenance.insert(pointer.clone(), source.clone());
            }

            let Value::Object(target_object) = target else {
                unreachable!("target was converted to an object above");
            };
            for (key, patch_child) in patch_object {
                ensure_active(cancellation, source)?;
                let child_pointer = pointer.child(key);
                if patch_child.is_null() {
                    target_object.remove(key);
                    remove_provenance(provenance, &child_pointer);
                    continue;
                }
                let target_child = target_object.entry(key.clone()).or_insert(Value::Null);
                merge_at(
                    target_child,
                    patch_child,
                    source,
                    &child_pointer,
                    provenance,
                    cancellation,
                )?;
            }
            Ok(())
        }
        replacement => {
            *target = replacement.clone();
            remove_provenance(provenance, pointer);
            assign_provenance(replacement, pointer, source, provenance, cancellation)
        }
    }
}

fn remove_provenance(provenance: &mut ProvenanceMap, pointer: &JsonPointer) {
    if pointer.is_root() {
        provenance.clear();
        return;
    }
    let keys: Vec<JsonPointer> = provenance
        .range(pointer.clone()..)
        .map(|(candidate, _)| candidate)
        .take_while(|candidate| candidate.is_self_or_descendant_of(pointer))
        .cloned()
        .collect();
    for key in keys {
        provenance.remove(&key);
    }
}

fn assign_provenance(
    value: &Value,
    pointer: &JsonPointer,
    source: &ConfigSource,
    provenance: &mut ProvenanceMap,
    cancellation: &ReloadCancellation,
) -> Result<(), ConfigError> {
    let mut stack = vec![(value, pointer.clone())];
    while let Some((current, current_pointer)) = stack.pop() {
        ensure_active(cancellation, source)?;
        provenance.insert(current_pointer.clone(), source.clone());
        match current {
            Value::Object(object) => {
                for (key, child) in object {
                    stack.push((child, current_pointer.child(key)));
                }
            }
            Value::Array(array) => {
                for (index, child) in array.iter().enumerate() {
                    stack.push((child, current_pointer.index(index)));
                }
            }
            Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
        }
    }
    Ok(())
}

fn ensure_active(
    cancellation: &ReloadCancellation,
    source: &ConfigSource,
) -> Result<(), ConfigError> {
    if cancellation.is_cancelled() {
        Err(ConfigError::for_source(ConfigErrorKind::Cancelled, source))
    } else {
        Ok(())
    }
}

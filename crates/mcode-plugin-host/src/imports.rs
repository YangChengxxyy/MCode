//! Component import inspection against the WIT world and capability grants.

// Rust guideline compliant 2026-08-26.

use mcode_plugin_api::{HOST_INTERFACE_ID, PluginManifest};
use wasmtime::component::Component;

use crate::error::HostError;

/// Names that imply ambient WASI or other denied host surfaces.
const DENIED_PREFIXES: &[&str] = &[
    "wasi:",
    "wasi-unstable",
    "wasi_snapshot",
    "env",
    "wasi_unstable",
];

/// Inspects component imports and fails closed on ambient WASI or extras.
///
/// Manifest `imports` must equal the component import set. Every name must be
/// the WIT host interface. Capability grants never authorize WASI or extra
/// imports; UI methods on the host interface are gated at call time.
///
/// # Errors
///
/// Returns [`HostError::ForbiddenImport`] or [`HostError::ImportMismatch`].
pub fn validate_component_imports(
    component: &Component,
    manifest: &PluginManifest,
) -> Result<Vec<String>, HostError> {
    let names = component_import_names(component)?;
    for name in &names {
        if is_denied_import(name) || name.as_str() != HOST_INTERFACE_ID {
            return Err(HostError::ForbiddenImport);
        }
    }
    let mut declared: Vec<String> = manifest.imports().to_vec();
    declared.sort();
    let mut actual = names.clone();
    actual.sort();
    if declared != actual {
        return Err(HostError::ImportMismatch);
    }
    Ok(names)
}

fn component_import_names(component: &Component) -> Result<Vec<String>, HostError> {
    let ty = component.component_type();
    Ok(ty
        .imports(component.engine())
        .map(|(name, _item)| name.to_string())
        .collect())
}

fn is_denied_import(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    DENIED_PREFIXES
        .iter()
        .any(|prefix| lower.starts_with(prefix))
        || lower.contains("filesystem")
        || lower.contains("environment")
        || lower.contains("sockets")
        || lower.contains("cli/")
        || lower.contains("http")
        || lower.contains("random")
        || lower.contains("clocks")
        || lower.contains("io/")
        || lower.contains("secret")
        || lower.contains("process")
}

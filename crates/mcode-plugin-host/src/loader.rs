//! WASM component loading, import checks, and generation spawn.

// Rust guideline compliant 2026-08-26.

use std::fs;

use mcode_plugin_api::{CapabilityGrants, CapabilityUse, PluginManifest, declaration_allows};
use wasmtime::component::Component;

#[cfg(feature = "test-util")]
use crate::actor::GuestKind;
use crate::actor::{PluginHandle, RuntimeLimits, instantiate_wasm};
use crate::error::HostError;
use crate::imports::validate_component_imports;
use crate::sandbox::new_engine;

/// Loads a WASM component from `plugin.json` and starts one generation actor.
///
/// Import inspection runs before instantiation. Ambient WASI is never linked.
///
/// # Errors
///
/// Returns [`HostError`] for I/O, forbidden imports, instantiation, or guest
/// construct failures.
pub fn load_wasm_generation(
    manifest: &PluginManifest,
    grants: &CapabilityGrants,
    generation: u64,
    limits: RuntimeLimits,
) -> Result<PluginHandle, HostError> {
    load_wasm_bytes(
        manifest,
        &fs::read(manifest.resolved_component()).map_err(|_| HostError::InvalidComponent)?,
        grants,
        generation,
        limits,
    )
}

/// Loads a WASM component from bytes.
///
/// # Errors
///
/// Returns [`HostError`] for compile, import, or instantiate failures.
pub fn load_wasm_bytes(
    manifest: &PluginManifest,
    wasm: &[u8],
    grants: &CapabilityGrants,
    generation: u64,
    limits: RuntimeLimits,
) -> Result<PluginHandle, HostError> {
    let engine = new_engine()?;
    let component = Component::new(&engine, wasm).map_err(|_| HostError::InvalidComponent)?;
    validate_component_imports(&component, manifest)?;
    let ui_declared = declaration_allows(
        manifest.capabilities(),
        manifest.plugin_root(),
        &CapabilityUse::Ui,
    );
    let guest = instantiate_wasm(&engine, &component, limits.sandbox(), grants, ui_declared)?;
    PluginHandle::spawn(manifest.id().clone(), generation, engine, guest, limits)
}

/// Compiles a WAT or binary component for tests and sandboxes.
///
/// # Errors
///
/// Returns [`HostError::InvalidComponent`] when Wasmtime cannot parse `source`.
pub fn compile_component(
    engine: &wasmtime::Engine,
    source: impl AsRef<[u8]>,
) -> Result<Component, HostError> {
    Component::new(engine, source).map_err(|_| HostError::InvalidComponent)
}

#[cfg(feature = "test-util")]
pub(crate) fn spawn_fake(
    plugin_id: mcode_plugin_api::PluginId,
    generation: u64,
    guest: Box<dyn crate::test_util::FakeGuest>,
    limits: RuntimeLimits,
) -> Result<PluginHandle, HostError> {
    let engine = new_engine()?;
    PluginHandle::spawn(
        plugin_id,
        generation,
        engine,
        GuestKind::Fake(guest),
        limits,
    )
}

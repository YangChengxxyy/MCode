//! Pack compilation boundary tests.

// Rust guideline compliant 2026-08-31.

use std::convert::Infallible;

use wasm_encoder::reencode::{Error, Reencode, utils};
use wit_component::{ComponentEncoder, StringEncoding, embed_component_metadata};
use wit_parser::{ManglingAndAbi, Resolve};

use crate::{ComponentLimits, ComponentWorld, PreflightError};

use super::{PluginRuntime, RuntimeError};

fn bounded_component(name: &str, source: &str) -> Vec<u8> {
    let mut resolve = Resolve::default();
    let package = resolve
        .push_str(name, source)
        .expect("canonical Pack fixture WIT must parse");
    let world = resolve
        .select_world(&[package], Some(name))
        .expect("canonical Pack fixture world must exist");
    let module = wit_component::dummy_module(&resolve, world, ManglingAndAbi::Standard32);
    let mut module = bounded_dummy_module(&module);
    embed_component_metadata(&mut module, &resolve, world, StringEncoding::UTF8)
        .expect("canonical Pack fixture metadata must embed");
    ComponentEncoder::default()
        .module(&module)
        .expect("canonical Pack fixture module must decode")
        .validate(true)
        .encode()
        .expect("canonical Pack fixture component must encode")
}

fn bounded_dummy_module(module: &[u8]) -> Vec<u8> {
    let mut output = wasm_encoder::Module::new();
    BoundedMemory
        .parse_core_module(&mut output, wasmparser::Parser::new(0), module)
        .expect("dummy Pack core module must reencode");
    output.finish()
}

struct BoundedMemory;

impl Reencode for BoundedMemory {
    type Error = Infallible;

    fn memory_type(
        &mut self,
        memory: wasmparser::MemoryType,
    ) -> Result<wasm_encoder::MemoryType, Error<Self::Error>> {
        let mut memory = utils::memory_type(self, memory);
        memory.maximum = Some(1_024);
        Ok(memory)
    }
}

fn session_component() -> Vec<u8> {
    bounded_component(
        "session",
        include_str!("../../../mcode-plugin-api/wit/feature-pack/session.wit"),
    )
}

fn provider_component() -> Vec<u8> {
    bounded_component(
        "provider",
        include_str!("../../../mcode-plugin-api/wit/provider/provider.wit"),
    )
}

#[test]
fn exact_pack_world_compilation_initializes_runtime_readiness() {
    let runtime = PluginRuntime::new();
    let bytes = session_component();

    let _compiled = runtime
        .compile_pack(bytes, ComponentWorld::Session, ComponentLimits::default())
        .expect("exact Session Pack compile");

    assert!(runtime.new_owner().is_ok());
}

#[test]
fn crossed_pack_world_does_not_initialize_runtime_readiness() {
    let runtime = PluginRuntime::new();
    let bytes = session_component();

    let result = runtime.compile_pack(bytes, ComponentWorld::Todo, ComponentLimits::default());

    assert!(matches!(result, Err(RuntimeError::Preflight(_))));
    assert!(matches!(
        runtime.new_owner(),
        Err(RuntimeError::RuntimeUninitialized)
    ));
}

#[test]
fn manager_world_is_rejected_by_pack_compilation() {
    let runtime = PluginRuntime::new();

    let result = runtime.compile_pack(
        ComponentWorld::Manager.reference_bytes(),
        ComponentWorld::Manager,
        ComponentLimits::default(),
    );

    assert!(matches!(result, Err(RuntimeError::InvalidPackWorld)));
    assert!(matches!(
        runtime.new_owner(),
        Err(RuntimeError::RuntimeUninitialized)
    ));
}

#[test]
fn oversized_pack_does_not_initialize_runtime_readiness() {
    let runtime = PluginRuntime::new();
    let bytes = provider_component();
    let limit = ComponentLimits::new(bytes.len() - 1).expect("smaller nonzero bound");

    let result = runtime.compile_pack(&bytes, ComponentWorld::Provider, limit);

    assert!(matches!(
        result,
        Err(RuntimeError::Preflight(PreflightError::ComponentTooLarge))
    ));
    assert!(matches!(
        runtime.new_owner(),
        Err(RuntimeError::RuntimeUninitialized)
    ));
}

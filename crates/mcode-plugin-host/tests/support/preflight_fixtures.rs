//! Canonical generated component fixtures shared by preflight tests.

use std::convert::Infallible;

use mcode_plugin_host::ComponentWorld;
use wasm_encoder::reencode::{Error, Reencode, utils};
use wit_component::{ComponentEncoder, StringEncoding, embed_component_metadata};
use wit_parser::{ManglingAndAbi, Resolve};

#[allow(dead_code, reason = "matrix tests use this shared helper")]
pub(crate) fn canonical_components() -> Vec<(ComponentWorld, Vec<u8>)> {
    ComponentWorld::ALL
        .into_iter()
        .map(|world| (world, canonical_component(world)))
        .collect()
}

pub(crate) fn canonical_component(world: ComponentWorld) -> Vec<u8> {
    let (name, source) = world_source(world);
    component_from_wit(name, source)
}

#[allow(dead_code, reason = "shape-mutation tests use this shared helper")]
pub(crate) fn component_from_wit(name: &str, source: &str) -> Vec<u8> {
    let mut resolve = Resolve::default();
    let package = resolve
        .push_str(name, source)
        .expect("canonical fixture WIT must parse");
    let world_id = resolve
        .select_world(&[package], Some(name))
        .expect("canonical fixture world must exist");
    let module = wit_component::dummy_module(&resolve, world_id, ManglingAndAbi::Standard32);
    let mut module = bounded_dummy_module(&module);
    embed_component_metadata(&mut module, &resolve, world_id, StringEncoding::UTF8)
        .expect("canonical fixture metadata must embed");
    ComponentEncoder::default()
        .module(&module)
        .expect("canonical fixture module must decode")
        .validate(true)
        .encode()
        .expect("canonical fixture component must encode")
}

#[allow(dead_code, reason = "scanner tests use this shared helper")]
pub(crate) fn component_binary(wat: &str) -> Vec<u8> {
    let bytes = wat::parse_str(wat).expect("valid fixture WAT");
    assert!(
        wasmparser::Parser::is_component(&bytes),
        "fixture must be a component-model artifact"
    );
    bytes
}

fn bounded_dummy_module(module: &[u8]) -> Vec<u8> {
    let mut output = wasm_encoder::Module::new();
    BoundedMemory
        .parse_core_module(&mut output, wasmparser::Parser::new(0), module)
        .expect("canonical fixture module must reencode");
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

#[allow(dead_code, reason = "shape-mutation tests use this shared helper")]
pub(crate) fn world_source(world: ComponentWorld) -> (&'static str, &'static str) {
    match world {
        ComponentWorld::Web => (
            "web",
            include_str!("../../../mcode-plugin-api/wit/feature-pack/web.wit"),
        ),
        ComponentWorld::Mcp => (
            "mcp",
            include_str!("../../../mcode-plugin-api/wit/feature-pack/mcp.wit"),
        ),
        ComponentWorld::Usage => (
            "usage",
            include_str!("../../../mcode-plugin-api/wit/feature-pack/usage.wit"),
        ),
        ComponentWorld::Provider => (
            "provider",
            include_str!("../../../mcode-plugin-api/wit/provider/provider.wit"),
        ),
    }
}

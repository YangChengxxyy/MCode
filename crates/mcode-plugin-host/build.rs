use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use wasm_encoder::reencode::{Error, Reencode, utils};
use wit_component::{ComponentEncoder, StringEncoding, embed_component_metadata};
use wit_parser::{ManglingAndAbi, Resolve};

const WORLDS: [(&str, &str, &str); 4] = [
    ("web", "feature-pack/web.wit", "web"),
    ("mcp", "feature-pack/mcp.wit", "mcp"),
    ("usage", "feature-pack/usage.wit", "usage"),
    ("provider", "provider/provider.wit", "provider"),
];

fn main() {
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest directory"));
    let wit_root = manifest.join("../mcode-plugin-api/wit");
    let output = PathBuf::from(env::var_os("OUT_DIR").expect("build output directory"));

    for (label, relative_path, world_name) in WORLDS {
        let path = wit_root.join(relative_path);
        println!("cargo:rerun-if-changed={}", path.display());
        let bytes = dummy_component(&path, world_name);
        fs::write(output.join(format!("{label}.wasm")), bytes)
            .expect("write canonical reference component");
    }
}

struct BoundedMemory;

impl Reencode for BoundedMemory {
    type Error = std::convert::Infallible;

    fn memory_type(
        &mut self,
        memory: wasmparser::MemoryType,
    ) -> Result<wasm_encoder::MemoryType, Error<Self::Error>> {
        let mut memory = utils::memory_type(self, memory);
        memory.maximum = Some(1_024);
        Ok(memory)
    }
}

fn dummy_component(path: &Path, world_name: &str) -> Vec<u8> {
    let mut resolve = Resolve::default();
    let (package, _) = resolve.push_path(path).expect("parse canonical WIT");
    let world = resolve
        .select_world(&[package], Some(world_name))
        .expect("select canonical world");
    let module = wit_component::dummy_module(&resolve, world, ManglingAndAbi::Standard32);
    let mut bounded = wasm_encoder::Module::new();
    BoundedMemory
        .parse_core_module(&mut bounded, wasmparser::Parser::new(0), &module)
        .expect("canonical dummy module must reencode");
    let mut module = bounded.finish();
    embed_component_metadata(&mut module, &resolve, world, StringEncoding::UTF8)
        .expect("embed canonical component metadata");
    ComponentEncoder::default()
        .module(&module)
        .expect("load canonical dummy module")
        .validate(true)
        .encode()
        .expect("encode canonical reference component")
}

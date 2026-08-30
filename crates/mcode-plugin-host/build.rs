use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use wit_component::{ComponentEncoder, StringEncoding, embed_component_metadata};
use wit_parser::{ManglingAndAbi, Resolve};

const WORLDS: [(&str, &str, &str); 13] = [
    ("manager", "manager.wit", "manager"),
    ("session", "feature-pack/session.wit", "session"),
    ("compaction", "feature-pack/compaction.wit", "compaction"),
    ("resources", "feature-pack/resources.wit", "resources"),
    ("ask", "feature-pack/ask.wit", "ask"),
    ("todo", "feature-pack/todo.wit", "todo"),
    ("web", "feature-pack/web.wit", "web"),
    ("mcp", "feature-pack/mcp.wit", "mcp"),
    ("usage", "feature-pack/usage.wit", "usage"),
    ("subagents", "feature-pack/subagents.wit", "subagents"),
    ("workspace", "feature-pack/workspace.wit", "workspace"),
    ("ui", "feature-pack/ui.wit", "ui"),
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

fn dummy_component(path: &Path, world_name: &str) -> Vec<u8> {
    let mut resolve = Resolve::default();
    let (package, _) = resolve.push_path(path).expect("parse canonical WIT");
    let world = resolve
        .select_world(&[package], Some(world_name))
        .expect("select canonical world");
    let mut module = wit_component::dummy_module(&resolve, world, ManglingAndAbi::Standard32);
    embed_component_metadata(&mut module, &resolve, world, StringEncoding::UTF8)
        .expect("embed canonical component metadata");
    ComponentEncoder::default()
        .module(&module)
        .expect("load canonical dummy module")
        .validate(true)
        .encode()
        .expect("encode canonical reference component")
}

//! Shared test helpers for WASM fixtures and manifests.

#![allow(dead_code, reason = "helpers are selected per integration crate")]

use mcode_plugin_api::{CapabilityGrants, PluginManifest, PluginSource, Provenance, TrustLevel};
use mcode_plugin_host::{
    RuntimeLimits, SandboxLimits, compile_component, load_wasm_bytes, new_engine,
};
use serde_json::json;
use wasmtime::component::Component;

pub fn base_manifest_json(component: &str) -> serde_json::Value {
    json!({
        "manifestVersion": 1,
        "id": "com.mcode.fixture",
        "name": "Fixture",
        "version": "1.0.0",
        "sdkVersion": "1.0.0",
        "witWorld": "mcode:plugin/plugin@0.1.0",
        "component": component,
        "imports": [],
        "capabilities": [{"kind": "ui"}],
        "contributions": {
            "tools": [{
                "id": "tool.main",
                "name": "fixture_tool",
                "displayName": "Fixture",
                "description": "Fixture tool",
                "inputSchema": {"type": "object"}
            }],
            "eventSubscriptions": [{
                "id": "events.main",
                "events": ["model"],
                "mailboxCapacity": 8
            }],
            "widgets": [{
                "metadata": {
                    "id": "status.main",
                    "region": "global",
                    "priority": 0,
                    "width": {"min": 1, "max": 80},
                    "invalidation": {"mode": "manual"}
                },
                "description": "status"
            }]
        }
    })
}

pub fn parse_manifest(
    root: &std::path::Path,
    component: &str,
    imports: &[String],
) -> PluginManifest {
    let mut value = base_manifest_json(component);
    value["imports"] = json!(imports);
    PluginManifest::parse_json(&serde_json::to_vec(&value).expect("json"), root).expect("manifest")
}

pub fn provenance(manifest: &PluginManifest) -> Provenance {
    Provenance::new(
        manifest.id().clone(),
        manifest.version(),
        PluginSource::Bundled {
            bundle: "fixture".into(),
        },
        TrustLevel::BuiltIn,
    )
    .expect("provenance")
}

pub fn export_only_wat() -> &'static str {
    r#"
(component
  (core module $m
    (memory (export "mem") 1)
    (global $bump (mut i32) (i32.const 64))
    (data (i32.const 16) "{}")
    (func $realloc (export "realloc") (param i32 i32 i32 i32) (result i32)
      (local $ptr i32)
      (local.set $ptr (global.get $bump))
      (global.set $bump
        (i32.add (global.get $bump) (i32.and (i32.add (local.get 3) (i32.const 7)) (i32.const -8))))
      (local.get $ptr)
    )
    (func $empty (export "empty") (result i32)
      (i32.store (i32.const 0) (i32.const 0))
      (i32.store (i32.const 4) (i32.const 0))
      (i32.const 0)
    )
    (func $obj (export "obj") (param i32 i32) (result i32)
      (i32.store (i32.const 8) (i32.const 16))
      (i32.store (i32.const 12) (i32.const 2))
      (i32.const 8)
    )
  )
  (core instance $i (instantiate $m))
  (alias core export $i "mem" (core memory $mem))
  (alias core export $i "realloc" (core func $realloc))
  (func (export "construct") (result string)
    (canon lift (core func $i "empty") (memory $mem) (realloc $realloc)))
  (func (export "invoke") (param "request-json" string) (result string)
    (canon lift (core func $i "obj") (memory $mem) (realloc $realloc)))
  (func (export "on-event") (param "event-json" string) (result string)
    (canon lift (core func $i "obj") (memory $mem) (realloc $realloc)))
  (func (export "render") (param "request-json" string) (result string)
    (canon lift (core func $i "obj") (memory $mem) (realloc $realloc)))
)
"#
}

pub fn infinite_invoke_render_wat() -> &'static str {
    r#"
(component
  (core module $m
    (memory (export "mem") 1)
    (func $realloc (export "realloc") (param i32 i32 i32 i32) (result i32)
      (i32.const 32)
    )
    (func $empty (export "empty") (result i32)
      (i32.const 0)
    )
    (func $obj (export "obj") (param i32 i32) (result i32)
      (i32.const 0)
    )
    (func $loop (export "loop") (param i32 i32) (result i32)
      (loop $forever (br $forever))
      (i32.const 0)
    )
  )
  (core instance $i (instantiate $m))
  (alias core export $i "mem" (core memory $mem))
  (alias core export $i "realloc" (core func $realloc))
  (func (export "construct") (result string)
    (canon lift (core func $i "empty") (memory $mem) (realloc $realloc)))
  (func (export "invoke") (param "request-json" string) (result string)
    (canon lift (core func $i "loop") (memory $mem) (realloc $realloc)))
  (func (export "on-event") (param "event-json" string) (result string)
    (canon lift (core func $i "obj") (memory $mem) (realloc $realloc)))
  (func (export "render") (param "request-json" string) (result string)
    (canon lift (core func $i "loop") (memory $mem) (realloc $realloc)))
)
"#
}

pub fn infinite_event_wat() -> &'static str {
    r#"
(component
  (core module $m
    (memory (export "mem") 1)
    (func $realloc (export "realloc") (param i32 i32 i32 i32) (result i32)
      (i32.const 32)
    )
    (func $empty (export "empty") (result i32)
      (i32.const 0)
    )
    (func $obj (export "obj") (param i32 i32) (result i32)
      (i32.const 0)
    )
    (func $loop (export "loop") (param i32 i32) (result i32)
      (loop $forever (br $forever))
      (i32.const 0)
    )
  )
  (core instance $i (instantiate $m))
  (alias core export $i "mem" (core memory $mem))
  (alias core export $i "realloc" (core func $realloc))
  (func (export "construct") (result string)
    (canon lift (core func $i "empty") (memory $mem) (realloc $realloc)))
  (func (export "invoke") (param "request-json" string) (result string)
    (canon lift (core func $i "obj") (memory $mem) (realloc $realloc)))
  (func (export "on-event") (param "event-json" string) (result string)
    (canon lift (core func $i "loop") (memory $mem) (realloc $realloc)))
  (func (export "render") (param "request-json" string) (result string)
    (canon lift (core func $i "obj") (memory $mem) (realloc $realloc)))
)
"#
}

pub fn wasi_import_wat() -> &'static str {
    r#"
(component
  (import "wasi:cli/environment@0.2.0" (instance
    (export "get-environment" (func (result u32)))
  ))
)
"#
}

pub fn construct_error_wat() -> &'static str {
    r#"
(component
  (core module $m
    (memory (export "mem") 1)
    (global $bump (mut i32) (i32.const 64))
    (data (i32.const 16) "{\"error\":{\"code\":\"boom\"}}")
    (func $realloc (export "realloc") (param i32 i32 i32 i32) (result i32)
      (local $ptr i32)
      (local.set $ptr (global.get $bump))
      (global.set $bump
        (i32.add (global.get $bump) (i32.and (i32.add (local.get 3) (i32.const 7)) (i32.const -8))))
      (local.get $ptr)
    )
    (func $err (export "err") (result i32)
      (i32.store (i32.const 0) (i32.const 16))
      (i32.store (i32.const 4) (i32.const 25))
      (i32.const 0)
    )
    (func $obj (export "obj") (param i32 i32) (result i32)
      (i32.store (i32.const 8) (i32.const 16))
      (i32.store (i32.const 12) (i32.const 2))
      (i32.const 8)
    )
  )
  (core instance $i (instantiate $m))
  (alias core export $i "mem" (core memory $mem))
  (alias core export $i "realloc" (core func $realloc))
  (func (export "construct") (result string)
    (canon lift (core func $i "err") (memory $mem) (realloc $realloc)))
  (func (export "invoke") (param "request-json" string) (result string)
    (canon lift (core func $i "obj") (memory $mem) (realloc $realloc)))
  (func (export "on-event") (param "event-json" string) (result string)
    (canon lift (core func $i "obj") (memory $mem) (realloc $realloc)))
  (func (export "render") (param "request-json" string) (result string)
    (canon lift (core func $i "obj") (memory $mem) (realloc $realloc)))
)
"#
}

pub fn construct_nonempty_wat() -> &'static str {
    r#"
(component
  (core module $m
    (memory (export "mem") 1)
    (global $bump (mut i32) (i32.const 64))
    (data (i32.const 16) "{}")
    (func $realloc (export "realloc") (param i32 i32 i32 i32) (result i32)
      (local $ptr i32)
      (local.set $ptr (global.get $bump))
      (global.set $bump
        (i32.add (global.get $bump) (i32.and (i32.add (local.get 3) (i32.const 7)) (i32.const -8))))
      (local.get $ptr)
    )
    (func $obj0 (export "obj0") (result i32)
      (i32.store (i32.const 8) (i32.const 16))
      (i32.store (i32.const 12) (i32.const 2))
      (i32.const 8)
    )
    (func $obj (export "obj") (param i32 i32) (result i32)
      (i32.store (i32.const 8) (i32.const 16))
      (i32.store (i32.const 12) (i32.const 2))
      (i32.const 8)
    )
  )
  (core instance $i (instantiate $m))
  (alias core export $i "mem" (core memory $mem))
  (alias core export $i "realloc" (core func $realloc))
  (func (export "construct") (result string)
    (canon lift (core func $i "obj0") (memory $mem) (realloc $realloc)))
  (func (export "invoke") (param "request-json" string) (result string)
    (canon lift (core func $i "obj") (memory $mem) (realloc $realloc)))
  (func (export "on-event") (param "event-json" string) (result string)
    (canon lift (core func $i "obj") (memory $mem) (realloc $realloc)))
  (func (export "render") (param "request-json" string) (result string)
    (canon lift (core func $i "obj") (memory $mem) (realloc $realloc)))
)
"#
}

pub fn huge_memory_wat() -> &'static str {
    r#"
(component
  (core module $m
    (func $empty (export "empty") (result i32) (i32.const 0))
    (func $obj (export "obj") (param i32 i32) (result i32) (i32.const 0))
    (func $realloc (export "realloc") (param i32 i32 i32 i32) (result i32) (i32.const 0))
    (memory (export "mem") 3000)
  )
  (core instance $i (instantiate $m))
  (alias core export $i "mem" (core memory $mem))
  (alias core export $i "realloc" (core func $realloc))
  (func (export "construct") (result string)
    (canon lift (core func $i "empty") (memory $mem) (realloc $realloc)))
  (func (export "invoke") (param "request-json" string) (result string)
    (canon lift (core func $i "obj") (memory $mem) (realloc $realloc)))
  (func (export "on-event") (param "event-json" string) (result string)
    (canon lift (core func $i "obj") (memory $mem) (realloc $realloc)))
  (func (export "render") (param "request-json" string) (result string)
    (canon lift (core func $i "obj") (memory $mem) (realloc $realloc)))
)
"#
}

pub fn compile(wat: &str) -> Component {
    let engine = new_engine().expect("engine");
    compile_component(&engine, wat).expect("component")
}

pub fn tight_limits() -> RuntimeLimits {
    RuntimeLimits::new(
        8,
        32 * 1024,
        std::time::Duration::from_millis(200),
        std::time::Duration::from_millis(500),
        SandboxLimits::new(200_000, 2 * 1024 * 1024, 1024, 4, 4, 4).expect("sandbox"),
    )
    .expect("limits")
}

pub fn load_wat(
    root: &std::path::Path,
    wat: &str,
    imports: &[String],
) -> mcode_plugin_host::PluginHandle {
    let wasm_path = root.join("plugin.wasm");
    std::fs::write(&wasm_path, wat.as_bytes()).expect("write wat as wasm source");
    let manifest = parse_manifest(root, "plugin.wasm", imports);
    load_wasm_bytes(
        &manifest,
        wat.as_bytes(),
        &CapabilityGrants::none(),
        1,
        tight_limits(),
    )
    .expect("load")
}

pub fn model_event() -> mcode_plugin_api::PluginEvent {
    mcode_plugin_api::PluginEvent::Model(mcode_plugin_api::ModelEvent {
        active: None,
        previous: None,
    })
}

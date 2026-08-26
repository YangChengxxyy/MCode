//! Wasmtime bindgen for the `mcode:plugin/plugin` world.

// Rust guideline compliant 2026-08-26.

#![allow(missing_docs, reason = "generated bindgen types")]

wasmtime::component::bindgen!({
    path: "../mcode-plugin-api/wit",
    world: "plugin",
});

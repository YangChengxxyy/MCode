//! Wasmtime bindings for the sole current Provider world.

// Rust guideline compliant 2026-08-29.

#![allow(missing_docs, reason = "generated Wasmtime bindings")]

wasmtime::component::bindgen!({
    path: "../mcode-plugin-api/wit/provider",
    world: "provider",
});

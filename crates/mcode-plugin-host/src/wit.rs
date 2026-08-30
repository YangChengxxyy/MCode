//! Wasmtime bindings for the sole current Manager world.

// Rust guideline compliant 2026-08-31.

#![allow(missing_docs, reason = "generated Wasmtime bindings")]

wasmtime::component::bindgen!({
    path: "../mcode-plugin-api/wit",
    world: "manager",
    imports: { default: async | store },
    exports: { default: async },
});

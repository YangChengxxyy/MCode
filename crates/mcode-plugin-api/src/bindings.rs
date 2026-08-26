//! Optional `wit-bindgen` guest bindings for `wasm32` plugin authors.
//!
//! Enable the `guest` feature from a WebAssembly component crate. The host
//! crate never enables this feature.

// Rust guideline compliant 2026-08-26.

wit_bindgen::generate!({
    path: "wit",
    world: "plugin",
    generate_all,
});

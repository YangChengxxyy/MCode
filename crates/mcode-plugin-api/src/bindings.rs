//! Optional `wit-bindgen` bindings for Manager component guests.
//!
//! Enable the `guest` feature from a component guest crate. The generated
//! surface contains only the current Manager world; the final component must
//! still pass the Host's exact no-WASI import preflight.

// Rust guideline compliant 2026-08-29.

#![allow(missing_docs, reason = "generated guest bindings")]

wit_bindgen::generate!({
    path: "wit",
    world: "manager",
    pub_export_macro: true,
});

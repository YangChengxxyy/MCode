//! Wasmtime Component Model host runtime for MCode WASM plugins.
//!
//! Each plugin generation owns a single [`Engine`] and [`Store`] on a dedicated
//! actor thread. Invoke, event, and render work is admitted through a bounded
//! mailbox. Ambient WASI filesystem, environment, network, process, and secret
//! APIs are never linked. There is no in-process, external-process, native
//! library, or MCP-transport plugin backend. Compaction, transcripts, and raw
//! terminal access are not part of this runtime.

// Rust guideline compliant 2026-08-26.

#![warn(missing_docs)]
#![deny(unsafe_op_in_unsafe_fn)]
#![forbid(unsafe_code)]

mod actor;
mod discovery;
mod error;
mod host_api;
mod imports;
mod loader;
mod mailbox;
mod registry;
mod sandbox;
mod wit;

#[cfg(feature = "test-util")]
pub mod test_util;

#[doc(inline)]
pub use actor::{LifecycleState, PluginHandle, RuntimeLimits};
#[doc(inline)]
pub use discovery::{
    DiscoveredPlugin, DiscoveryIssue, DiscoveryIssueKind, DiscoveryReport, discover_directory,
    discover_plugin_root,
};
#[doc(inline)]
pub use error::HostError;
#[doc(inline)]
pub use loader::{compile_component, load_wasm_bytes, load_wasm_generation};
#[doc(inline)]
pub use mailbox::EventDelivery;
#[doc(inline)]
pub use registry::{
    ContributionKind, HOST_BINDINGS_VERSION, HostBindings, HostBindingsError, PluginRegistration,
    PluginRegistry, RegisteredContribution, RegistryChange, RegistryError, RegistrySnapshot,
    RegistryTransaction, ToolBindingTarget,
};
#[doc(inline)]
pub use sandbox::{SandboxLimits, engine_config, new_engine};

//! Wasmtime Component Model host runtime for MCode WASM plugins.
//!
//! Each plugin generation owns a single [`Engine`] and [`Store`] on a dedicated
//! actor thread. Invoke, event, and render work is admitted through a bounded
//! mailbox. Ambient WASI filesystem, environment, network, process, and secret
//! APIs are never linked. There is no in-process, external-process, native
//! library, or MCP-transport plugin backend. Transcripts and raw terminal
//! access are not part of this runtime. Core has no compaction implementation
//! or fallback; the future signed `com.mcode.compaction` Pack belongs to the
//! separate Host CompactionPack Service and is unavailable when that Pack is
//! absent.

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
mod provider_routes;
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
pub use provider_routes::{
    AuthFingerprint, AuthSlotId, EndpointFingerprint, MAX_PROVIDER_ROUTE_CLAIMS,
    MAX_PROVIDER_ROUTE_CLAIMS_PER_BATCH, MAX_USAGE_CONTEXTS_PER_LEDGER, ModelAlias, ModelId,
    ModelRouteLease, ProviderGeneration, ProviderId, ProviderRouteClaim, ProviderRouteError,
    ProviderRouteId, ProviderRouteLedger, ProviderRouteOwner, ProviderRouteOwnership,
    ProviderRouteSnapshot, RequestId, TokenCount, TurnId, UsageContextSnapshot, UsageCounters,
    UsageSample,
};
#[doc(inline)]
pub use registry::{
    ContributionKind, HOST_BINDINGS_VERSION, HostBindings, HostBindingsError, PluginRegistration,
    PluginRegistry, RegisteredContribution, RegistryChange, RegistryError, RegistrySnapshot,
    RegistryTransaction, ToolBindingTarget,
};
#[doc(inline)]
pub use sandbox::{SandboxLimits, engine_config, new_engine};

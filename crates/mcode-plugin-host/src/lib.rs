//! Bounded Wasmtime preflight and Host-only plugin substrates.
//!
//! Scanner-first preflight validates all 13 sole-current component worlds.
//! Authoritative fixed-12 Manager loading verifies exact artifact bytes before
//! the opaque runtime foundation compiles them. Bounded Stores execute only
//! typed, asynchronous Manager initialize, poll, and shutdown calls. One
//! owner-bound lease preserves fuel across those lifecycle segments. A fixed-12
//! authority director prepares complete generations, publishes them atomically,
//! and retires stale Store ownership after cancellation and quiescence. Trust,
//! installation, FeaturePack, and ProviderPack effects remain outside this
//! slice. Private Provider bindings and pure DTO validators remain Store-free.

// Rust guideline compliant 2026-08-31.

#![warn(missing_docs)]
#![deny(unsafe_op_in_unsafe_fn)]
#![forbid(unsafe_code)]

mod component;
mod component_scanner;
mod component_shape;
mod component_world;
mod error;
mod feature_gateway;
mod manager_director;
mod manager_loading;
mod provider_routes;
mod provider_validation;
mod provider_wit;
/// Scanner-gated, fail-closed Wasmtime ownership and admission.
pub mod runtime;
mod wit;

#[doc(inline)]
pub use component::{
    ComponentLimits, MAX_COMPONENT_BYTES, preflight_component, preflight_manager_component,
};
#[doc(inline)]
pub use component_world::ComponentWorld;
#[doc(inline)]
pub use error::{CallerBindingError, ImportCategory, PreflightError};
#[doc(inline)]
pub use feature_gateway::{FeatureCaller, bind_feature_caller, decode_bound_feature_task};
#[doc(inline)]
pub use manager_director::{
    CurrentManagerGeneration, ManagerGenerationDirector, ManagerGenerationSnapshot,
    PreparationProgress, ReconciliationError, ReconciliationOutcome,
};
#[doc(inline)]
pub use manager_loading::{
    CompiledManagerCandidate, ManagerCandidates, ManagerLoadError, load_manager_candidates,
};
#[doc(inline)]
pub use provider_routes::{
    AuthFingerprint, AuthSlotId, EndpointFingerprint, MAX_PROVIDER_ROUTE_CLAIMS,
    MAX_PROVIDER_ROUTE_CLAIMS_PER_BATCH, MAX_USAGE_CONTEXTS_PER_LEDGER, ModelAlias, ModelId,
    ModelRouteLease, ProviderGeneration, ProviderId, ProviderRouteClaim, ProviderRouteError,
    ProviderRouteId, ProviderRouteLedger, ProviderRouteOwner, ProviderRouteOwnership,
    ProviderRouteSnapshot, RequestId, TokenCount, TurnId, UsageContextSnapshot, UsageCounters,
    UsageSample,
};

//! Bounded Wasmtime preflight and Host-only plugin substrates.
//!
//! Scanner-first preflight validates all 13 sole-current component worlds.
//! Authoritative fixed-12 Manager loading verifies exact artifact bytes before
//! the opaque runtime foundation compiles them. Bounded Stores execute only
//! typed, asynchronous Manager initialize, poll, and shutdown calls. Each
//! lifecycle call receives one fresh owner-bound fuel lease. A fixed-12 authority
//! director prepares complete generations, publishes them atomically, and polls
//! an exact expected current generation without exposing Store ownership. Exact
//! Pack IDs can be loaded only through a matching current Manager
//! generation, with canonical inventory digest verification and no directory
//! discovery. Exact configured Pack sets are prepared in private Stores and
//! atomically replace one generation's active set only after final authority
//! revalidation; typed Pack task execution remains outside this slice.
//! Private Provider bindings and pure DTO validators remain Store-free.

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
mod pack_activation;
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "T8.3 consumes the generation-bound Pack candidate boundary"
    )
)]
mod pack_loading;
mod pack_selection;
mod pack_wit;
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
    CurrentManagerGeneration, CurrentManagerPoll, ManagerGenerationCallError,
    ManagerGenerationDirector, ManagerGenerationSnapshot, PreparationProgress, ReconciliationError,
    ReconciliationOutcome,
};
#[doc(inline)]
pub use manager_loading::{
    CompiledManagerCandidate, ManagerCandidates, ManagerLoadError, load_manager_candidates,
};
#[doc(inline)]
pub use pack_selection::PackConfigurationError;
#[doc(inline)]
pub use provider_routes::{
    AuthFingerprint, AuthSlotId, EndpointFingerprint, MAX_PROVIDER_ROUTE_CLAIMS,
    MAX_PROVIDER_ROUTE_CLAIMS_PER_BATCH, MAX_USAGE_CONTEXTS_PER_LEDGER, ModelAlias, ModelId,
    ModelRouteLease, ProviderGeneration, ProviderId, ProviderRouteClaim, ProviderRouteError,
    ProviderRouteId, ProviderRouteLedger, ProviderRouteOwner, ProviderRouteOwnership,
    ProviderRouteSnapshot, RequestId, TokenCount, TurnId, UsageContextSnapshot, UsageCounters,
    UsageSample,
};

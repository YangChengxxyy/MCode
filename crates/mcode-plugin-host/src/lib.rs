//! Bounded Wasmtime preflight and Host-only plugin substrates.
//!
//! T7 scans and validates all 13 sole-current component worlds without
//! creating a Wasmtime store, instantiating a component, or calling a guest.
//! Only each selected world's exact imports, exports, and typed members are
//! accepted. The opaque runtime foundation lazily initializes only after a
//! scanner-issued token, owns each bounded Store, and admits only asynchronous
//! Manager instantiation. Lifecycle, loading, generation, waiting, and trust
//! remain outside this foundation. Private Provider bindings and pure DTO
//! validators remain Store-free.

// Rust guideline compliant 2026-08-29.

#![warn(missing_docs)]
#![deny(unsafe_op_in_unsafe_fn)]
#![forbid(unsafe_code)]

mod component;
mod component_scanner;
mod component_shape;
mod component_world;
mod error;
mod feature_gateway;
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
pub use provider_routes::{
    AuthFingerprint, AuthSlotId, EndpointFingerprint, MAX_PROVIDER_ROUTE_CLAIMS,
    MAX_PROVIDER_ROUTE_CLAIMS_PER_BATCH, MAX_USAGE_CONTEXTS_PER_LEDGER, ModelAlias, ModelId,
    ModelRouteLease, ProviderGeneration, ProviderId, ProviderRouteClaim, ProviderRouteError,
    ProviderRouteId, ProviderRouteLedger, ProviderRouteOwner, ProviderRouteOwnership,
    ProviderRouteSnapshot, RequestId, TokenCount, TurnId, UsageContextSnapshot, UsageCounters,
    UsageSample,
};

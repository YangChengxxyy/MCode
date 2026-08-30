//! Bounded Wasmtime preflight and Host-only plugin substrates.
//!
//! T7 compiles and validates the sole current Manager component shape without
//! creating a Wasmtime store, instantiating a component, or calling a guest.
//! Only the canonical FeatureService import and typed lifecycle export are
//! accepted. Private Provider bindings and pure DTO validators enforce the
//! current catalog, wire-JSON, prepare, and decoder-local bounds without a
//! Store or stateful runtime. Provider route ownership remains Host-only and
//! contains no guest codec, transport, credential, URL, socket, or Wasmtime
//! handle.

// Rust guideline compliant 2026-08-29.

#![warn(missing_docs)]
#![deny(unsafe_op_in_unsafe_fn)]
#![forbid(unsafe_code)]

mod component;
mod error;
mod feature_gateway;
mod provider_routes;
mod provider_validation;
mod provider_wit;
mod wit;

#[doc(inline)]
pub use component::{ComponentLimits, MAX_MANAGER_COMPONENT_BYTES, preflight_manager_component};
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

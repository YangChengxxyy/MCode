//! Sole-current typed contracts for MCode Manager, FeaturePack, and ProviderPack components.
//!
//! Managers use [`MANAGER_WORLD_ID`], import only [`FEATURE_SERVICE_INTERFACE_ID`],
//! and export only [`MANAGER_LIFECYCLE_INTERFACE_ID`]. Feature packs share
//! [`FEATURE_PACK_WIT_PACKAGE`] while keeping eleven physically independent worlds
//! and family-local DTO namespaces. Provider packs use the zero-import
//! [`PROVIDER_WORLD_ID`] and export [`PROVIDER_INTERFACE_ID`]. Manager task transport
//! uses strict bounded JSON because WIT carries its gateway as strings. Pack DTOs
//! stay typed in WIT. This crate contains ABI artifacts only and no component runtime.
//!
//! # Examples
//!
//! ```
//! use mcode_plugin_api::{
//!     FEATURE_PACK_WIT_PACKAGE, MANAGER_JSON_ABI_VERSION, MANAGER_WORLD_ID,
//!     PROVIDER_WORLD_ID,
//! };
//!
//! assert_eq!(MANAGER_JSON_ABI_VERSION, "0.0.1");
//! assert_eq!(MANAGER_WORLD_ID, "mcode:plugin/manager@0.0.1");
//! assert_eq!(FEATURE_PACK_WIT_PACKAGE, "mcode:feature-pack@0.0.1");
//! assert_eq!(PROVIDER_WORLD_ID, "mcode:provider-pack/provider@0.0.1");
//! ```

// Rust guideline compliant 2026-08-30.

#![warn(missing_docs)]
#![deny(unsafe_op_in_unsafe_fn)]
#![forbid(unsafe_code)]

mod identity;
mod strict_json;
mod task_wire;

#[cfg(feature = "guest")]
pub mod bindings;

#[doc(inline)]
pub use identity::{
    MAX_OPERATION_ID_BYTES, MAX_TASK_GENERATION, MIN_OPERATION_ID_BYTES, OperationId,
    TASK_ID_BYTES, TaskErrorCode, TaskFailure, TaskGeneration, TaskId, TaskIdentityError,
    is_valid_operation_id,
};
#[doc(inline)]
pub use task_wire::{
    FeatureTaskBody, FeatureTaskClosed, FeatureTaskCompleted, FeatureTaskControl, FeatureTaskError,
    FeatureTaskHandle, FeatureTaskProgress, FeatureTaskRejection, FeatureTaskRequest,
    FeatureTaskStart, FeatureTaskTerminal, FeatureTaskUpdate, MAX_DECLARED_OPERATIONS,
    MAX_MANAGER_TASK_WIRE_BYTES, TaskRequestMetadata, TaskState, TaskWireError,
    decode_feature_task_request, validate_declared_operation,
};

/// JSON task-wire ABI version.
pub const MANAGER_JSON_ABI_VERSION: &str = "0.0.1";

/// Fully qualified current Manager WIT package identifier.
pub const MANAGER_WIT_PACKAGE: &str = "mcode:plugin@0.0.1";

/// Current Manager world name.
pub const MANAGER_WORLD: &str = "manager";

/// Current Manager package and world version.
pub const MANAGER_WORLD_VERSION: &str = "0.0.1";

/// Fully qualified current Manager world identifier.
pub const MANAGER_WORLD_ID: &str = "mcode:plugin/manager@0.0.1";

/// Sole Host import interface identifier.
pub const FEATURE_SERVICE_INTERFACE_ID: &str = "mcode:plugin/feature-service@0.0.1";

/// Sole Manager guest export interface identifier.
pub const MANAGER_LIFECYCLE_INTERFACE_ID: &str = "mcode:plugin/manager-lifecycle@0.0.1";

/// Canonical current Manager WIT source.
pub const MANAGER_WIT: &str = include_str!("../wit/manager.wit");

/// Fully qualified current FeaturePack WIT package identifier.
pub const FEATURE_PACK_WIT_PACKAGE: &str = "mcode:feature-pack@0.0.1";

/// Fully qualified current ProviderPack WIT package identifier.
pub const PROVIDER_WIT_PACKAGE: &str = "mcode:provider-pack@0.0.1";

/// Current ProviderPack world name.
pub const PROVIDER_WORLD: &str = "provider";

/// Current ProviderPack package and world version.
pub const PROVIDER_WORLD_VERSION: &str = "0.0.1";

/// Fully qualified current ProviderPack world identifier.
pub const PROVIDER_WORLD_ID: &str = "mcode:provider-pack/provider@0.0.1";

/// Sole ProviderPack guest export interface name.
pub const PROVIDER_INTERFACE: &str = "provider-api";

/// Fully qualified sole ProviderPack guest export interface identifier.
pub const PROVIDER_INTERFACE_ID: &str = "mcode:provider-pack/provider-api@0.0.1";

/// Canonical current ProviderPack WIT source.
pub const PROVIDER_WIT: &str = include_str!("../wit/provider/provider.wit");

//! Sole-current typed contract for MCode Manager components.
//!
//! The only component world is [`MANAGER_WORLD_ID`]. Managers can import only
//! [`FEATURE_SERVICE_INTERFACE_ID`] and export only
//! [`MANAGER_LIFECYCLE_INTERFACE_ID`]. Task transport uses strict bounded JSON
//! because WIT carries the gateway as strings. `operationId` is a declarative
//! canonical key shared with Host-vault authority; `taskId` is the Host-issued
//! task instance. Lifecycle state and errors stay typed in WIT. This crate
//! exposes no generic JSON value, runtime handle,
//! manifest, capability, contribution, event, state, UI, or provenance API.
//!
//! # Examples
//!
//! ```
//! use mcode_plugin_api::{MANAGER_JSON_ABI_VERSION, MANAGER_WORLD_ID};
//!
//! assert_eq!(MANAGER_JSON_ABI_VERSION, "0.0.1");
//! assert_eq!(MANAGER_WORLD_ID, "mcode:plugin/manager@0.0.1");
//! ```

// Rust guideline compliant 2026-08-29.

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

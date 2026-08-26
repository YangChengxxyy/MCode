//! WIT contract, strict `plugin.json`, and guest JSON DTOs for MCode plugins.
//!
//! This crate is independent of Wasmtime, TUI, providers, MCP, process, and
//! session persistence. The only plugin ABI is the WebAssembly Component Model
//! world in [`PLUGIN_WIT`]. There is no in-process, external-process, native
//! library, script, or MCP-transport plugin backend. Host runtime code lives in
//! `mcode-plugin-host`.
//!
//! # Examples
//!
//! ```
//! use mcode_plugin_api::{MANIFEST_VERSION, PLUGIN_WIT, WIT_WORLD_ID};
//!
//! assert_eq!(MANIFEST_VERSION, 1);
//! assert!(PLUGIN_WIT.contains(WIT_WORLD_ID.split('/').next().unwrap_or_default()));
//! ```

// Rust guideline compliant 2026-08-26.

#![warn(missing_docs)]
#![deny(unsafe_op_in_unsafe_fn)]
#![forbid(unsafe_code)]

mod action;
mod capability;
mod contribution;
mod events;
mod guest;
mod ids;
mod limits;
mod manifest;
mod path;
mod provenance;
mod state;
mod ui;
mod validation;

#[cfg(feature = "guest")]
pub mod bindings;

#[doc(inline)]
pub use action::{UiAction, UiActionKind, UiActionValidationError, validate_ui_action};
#[doc(inline)]
pub use capability::{
    CapabilityDeclaration, CapabilityGrants, CapabilityKind, CapabilityUse,
    CapabilityValidationError, FilesystemAccess, declaration_allows, validate_capabilities,
};
#[doc(inline)]
pub use contribution::{
    CommandDescriptor, ContributionValidationError, Contributions, EventSubscriptionDescriptor,
    ModalDescriptor, PromptDescriptor, ResourceDescriptor, ResourceKind, TimelineDescriptor,
    ToolDescriptor, ViewDescriptor, WidgetDescriptor,
};
#[doc(inline)]
pub use events::{
    ActivityPhase, EventKind, EventValidationError, ModelEvent, ModelIdentity, NetworkEndpoint,
    NetworkEvent, PluginEvent, StreamEvent, ToolEvent, UsageEvent, UsageMetrics,
};
#[doc(inline)]
pub use guest::{
    GuestErrorBody, GuestInvokeRequest, GuestInvokeResponse, GuestInvokeTarget, GuestParseError,
    GuestRenderRequest, GuestRenderResponse, GuestWireError, HOST_INTERFACE_ID, PLUGIN_WIT,
    WIT_PACKAGE, WIT_WORLD, WIT_WORLD_ID, WIT_WORLD_VERSION, parse_guest_error,
    parse_guest_success,
};
#[doc(inline)]
pub use ids::{IdError, Identifier, PluginId};
#[doc(inline)]
pub use limits::{
    MAX_CAPABILITIES, MAX_CONTRIBUTIONS, MAX_CUSTOM_EVENT_BYTES, MAX_DESCRIPTOR_JSON_BYTES,
    MAX_DESCRIPTORS_PER_KIND, MAX_GUEST_OUTPUT_BYTES, MAX_HOST_ACTION_RECORDS,
    MAX_HOST_BINDINGS_BYTES, MAX_HOST_LOG_BYTES, MAX_HOST_LOG_RECORDS, MAX_HOST_VIEW_RECORDS,
    MAX_JSON_DEPTH, MAX_JSON_NODES, MAX_MANIFEST_BYTES, MAX_PLUGIN_PATH_BYTES,
    MAX_PROMPT_CONTRIBUTION_BYTES, MAX_STATE_VALUE_BYTES, MAX_UI_ACTION_BYTES, MAX_UI_VIEW_BYTES,
};
#[doc(inline)]
pub use manifest::{
    MANIFEST_VERSION, ManifestError, PLUGIN_MANIFEST_SCHEMA_ID, PLUGIN_MANIFEST_SCHEMA_JSON,
    PluginManifest, SDK_VERSION, UnknownFieldPolicy,
};
#[doc(inline)]
pub use path::{PathValidationError, resolve_contained_path};
#[doc(inline)]
pub use provenance::{PluginSource, Provenance, ProvenanceError, SourceScope, TrustLevel};
#[doc(inline)]
pub use state::{
    ExtensionEvent, ExtensionState, ExtensionStateUpdate, PortableStateDeclaration,
    SecretStateDeclaration, StateDeclarationError, StateDeclarations, StateDtoError,
};
#[doc(inline)]
pub use ui::{
    Invalidation, TextTone, UiRegion, UiValidationError, UiView, ViewContent, ViewKind,
    ViewMetadata, WidthConstraints,
};

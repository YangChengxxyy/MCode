//! Strict layered JSON configuration for MCode composition roots.
//!
//! `mcode-config` is independent of providers, plugins, MCP, resources, the
//! CLI, and project-root discovery. Callers supply [`ConfigLayer`] values with
//! typed [`ConfigSource`] metadata. The runtime applies this fixed precedence:
//!
//! 1. compiled defaults;
//! 2. global `$MCODE_HOME/settings.json`;
//! 3. trusted project `.mcode/settings.json`;
//! 4. explicit ephemeral overrides.
//!
//! Every source is strict UTF-8 JSON with this exact envelope:
//!
//! ```json
//! {"formatVersion":1,"config":{"model":"example"}}
//! ```
//!
//! Only [`FORMAT_VERSION`] is accepted. Duplicate keys at any depth, comments,
//! trailing commas/content, partial input, invalid UTF-8, and configured
//! resource-limit violations fail the whole reload.
//!
//! # Merge and provenance
//!
//! Source payloads use [RFC 7396 JSON Merge Patch](https://www.rfc-editor.org/rfc/rfc7396):
//! objects merge recursively, arrays and scalars replace as a whole, and an
//! object member whose patch is `null` is deleted. There is no array-by-index
//! merge. [`ConfigSnapshot::provenance`] records the source of every final RFC
//! 6901 JSON Pointer. For a composed object, the object's own pointer retains
//! the source that created or replaced that object; changed descendants record
//! their own winning sources.
//!
//! # Security boundary
//!
//! Credential-like fields never accept inline scalar or array values. A
//! material credential field must be exactly `{"secretRef":"..."}`. `null` is
//! a deletion marker only in an RFC 7396 patch object; credential members inside
//! array replacements cannot use it. Credential markers match snake-case,
//! kebab-case, or camel-case term boundaries, and fail closed for unambiguous
//! concatenated/all-uppercase suffixes and trailing numeric version labels.
//! Token quantity settings such as `maxTokens` remain ordinary domain data.
//! The crate resolves no secrets and performs no environment-variable or
//! `${ENV}` interpolation.
//! Untrusted project sources are not read and produce bounded diagnostics.
//! Snapshot, error, layer, and runtime `Debug` output never renders JSON values.
//!
//! # Publication and persistence
//!
//! [`ConfigRuntime::reload`] reads and validates every participating source,
//! calls the caller-owned [`ConfigValidator`], computes a canonical digest, and
//! only then swaps the complete immutable snapshot. Failed reloads preserve the
//! previous snapshot; equal digests do not advance the generation. The reload
//! API is watcher-independent and cooperatively cancellable.
//!
//! [`ensure_home_layout`] bootstraps only the owned home root and `plugins/`.
//! Pre-existing prefixes outside the owned boundary may resolve through links;
//! the owned root and child use handle-relative no-follow operations.
//! [`ConfigErrorKind::LinkEscape`] applies when either owned directory is a
//! symlink or reparse point. Bootstrap requires current ownership (or `SYSTEM`
//! before Windows repair), applies Unix `0700` or an exact protected
//! current-user-plus-`SYSTEM` Windows DACL, and durably publishes each created
//! directory. Top-level Plugin containers, Managers, Packs, reserved Host-only
//! credentials, staging, authority files, and project `.mcode` paths remain
//! lazy and are not created. Crate-private owned-file transactions create only
//! requested ancestors, reject links and case aliases, use bounded reads and a
//! persistent lock, and publish private files through anchored durable rename.
//!
//! The standalone root `plugins.json` authority is available through
//! [`read_manager_registry`] and [`replace_manager_registry`]. It is a bounded,
//! strict, exact-12 [`ManagerRegistry`] with revision compare-and-swap; it
//! never uses layered configuration, merge patch, project overrides, or
//! migration.
//!
//! [`write_config_file`] remains a separate arbitrary-path API. It writes only
//! the JSON envelope, using a same-directory
//! random `create_new` temporary file, flush plus `sync_data`, an advisory lock,
//! and platform replacement semantics. It does not implement a secret store,
//! session persistence, plugin manifests, or CLI behavior.

// Rust guideline compliant 2026-08-28

#![warn(missing_docs)]
#![deny(unsafe_op_in_unsafe_fn)]

mod cancel;
mod error;
mod home;
mod limits;
mod manager_registry;
mod merge;
mod parse;
mod pointer;
mod runtime;
mod secure_fs;
mod security;
mod source;
mod write;

/// Exact configuration-envelope format version accepted by this crate.
pub const FORMAT_VERSION: u32 = 1;

#[doc(inline)]
pub use cancel::ReloadCancellation;
#[doc(inline)]
pub use error::{ConfigError, ConfigErrorKind};
#[doc(inline)]
pub use home::{HomeEnv, HomeLayout, MCODE_DIR_NAME, MCODE_HOME_ENV, PluginFamily};
#[doc(inline)]
pub use limits::{ConfigLimits, MAX_SUPPORTED_DEPTH};
#[doc(inline)]
pub use manager_registry::{
    ArtifactRef, AuthorityRevision, CanonicalVersion, MANAGER_REGISTRY_FORMAT_VERSION,
    MANAGER_REGISTRY_KIND, MAX_MANAGER_REGISTRY_BYTES, ManagerRecord, ManagerRegistry,
    ManagerRegistryDocument, Sha256Digest, SourceBindingId, TrustHighWater, read_manager_registry,
    replace_manager_registry,
};
#[doc(inline)]
pub use pointer::JsonPointer;
#[doc(inline)]
pub use runtime::{
    AcceptAllConfig, ConfigDiagnostic, ConfigDiagnosticCode, ConfigDigest, ConfigRuntime,
    ConfigSnapshot, ConfigValidator, ReloadOutcome, ValidationFailure,
};
#[doc(inline)]
pub use secure_fs::{
    AccessControlEvidence, NativeUnavailableReason, OwnedKind, ensure_home_layout,
    probe_access_control,
};
#[doc(inline)]
pub use source::{ConfigLayer, ConfigScope, ConfigSource, SourceTrust};
#[doc(inline)]
pub use write::{write_config_file, write_config_file_with_limits};

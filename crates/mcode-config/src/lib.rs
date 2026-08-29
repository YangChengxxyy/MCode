//! Strict owned configuration authorities for MCode.
//!
//! [`ensure_home_layout`] bootstraps only the owned home root and `plugins/`.
//! Authority files are lazy, bounded, strict JSON documents published through
//! anchored no-follow transactions with revision compare-and-swap.
//!
//! The root authorities are [`ManagerRegistry`] at `plugins.json` and
//! [`RootComposition`] at `config.json`. Manager receipts and nested Pack
//! installation documents occupy their canonical family paths. The Host vault
//! is exclusively `plugins/.host/auth.json`.
//!
//! Obsolete product artifacts are not configuration inputs. This crate has no
//! migration, compatibility read, layered merge, alias, or fallback for old
//! settings, model, credential, Plugin-lock, session, or sibling-Pack layouts.

// Rust guideline compliant 2026-08-29

#![warn(missing_docs)]
#![deny(unsafe_op_in_unsafe_fn)]

mod error;
mod home;
mod host_vault;
mod manager_receipt;
mod manager_registry;
mod pack_installation;
mod parse;
mod root_composition;
mod secure_fs;
mod staging;
mod transaction_id;

#[doc(inline)]
pub use error::{ConfigError, ConfigErrorKind};
#[doc(inline)]
pub use home::{HomeEnv, HomeLayout, MCODE_DIR_NAME, MCODE_HOME_ENV, PluginFamily};
#[doc(inline)]
pub use host_vault::{
    HOST_VAULT_FORMAT_VERSION, HOST_VAULT_KIND, HostVaultState, MAX_HOST_VAULT_BYTES,
    VaultRevision, initialize_empty_host_vault, read_host_vault_state,
};
#[doc(inline)]
pub use manager_receipt::{
    MANAGER_RECEIPT_FORMAT_VERSION, MANAGER_RECEIPT_KIND, MAX_MANAGER_RECEIPT_BYTES,
    ManagerReceiptDocument, read_manager_receipt, replace_manager_receipt,
};
#[doc(inline)]
pub use manager_registry::{
    ArtifactRef, AuthorityRevision, CanonicalVersion, MANAGER_REGISTRY_FORMAT_VERSION,
    MANAGER_REGISTRY_KIND, MAX_MANAGER_REGISTRY_BYTES, ManagerRecord, ManagerRegistry,
    ManagerRegistryDocument, Sha256Digest, SourceBindingId, TrustHighWater, read_manager_registry,
    replace_manager_registry,
};
#[doc(inline)]
pub use pack_installation::{
    BundlePath, InventoryEntry, MAX_PACK_INSTALLATION_BYTES, MAX_PACK_INVENTORY_ENTRIES,
    PACK_INSTALLATION_FORMAT_VERSION, PACK_INSTALLATION_KIND, PackInstallation,
    PackInstallationDocument, read_pack_installation, replace_pack_installation,
};
#[doc(inline)]
pub use root_composition::{
    DefaultRoute, MAX_ROOT_COMPOSITION_BYTES, PackId, ROOT_COMPOSITION_FORMAT_VERSION,
    ROOT_COMPOSITION_KIND, RootComposition, RootCompositionDocument, UiSelection,
    read_root_composition, replace_root_composition,
};
#[doc(inline)]
pub use secure_fs::{
    AccessControlEvidence, NativeUnavailableReason, OwnedKind, ensure_home_layout,
    probe_access_control,
};
#[doc(inline)]
pub use staging::{
    MAX_STAGING_DIRECTORIES, MAX_STAGING_ENTRIES, MAX_STAGING_FILE_BYTES, MAX_STAGING_FILES,
    MAX_STAGING_JOURNAL_BYTES, MAX_STAGING_ROOT_ENTRIES, MAX_STAGING_TOTAL_BYTES,
    StagedTransaction, StagingTransaction, begin_staging,
};
#[doc(inline)]
pub use transaction_id::TransactionId;

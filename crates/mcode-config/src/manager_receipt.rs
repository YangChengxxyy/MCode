//! Strict Host-generated receipts for installed Manager artifacts.
//!
//! A receipt is non-authoritative bookkeeping. It never controls enablement,
//! source binding, trust high-water, artifact selection, loading, routing, or
//! activation. Missing or corrupt receipts may be rebuilt only by a future
//! verified Host install transaction; this module does not rebuild them or
//! read or update `plugins.json`.

// Rust guideline compliant 2026-08-29

use std::path::PathBuf;

use serde::Serialize;
use serde_json::Value;

use crate::manager_registry::{exact_object, parse_active, take_positive_revision, take_string};
use crate::parse::parse_strict_value;
use crate::secure_fs::owned_file::{locked_update_owned_file, read_owned_file};
use crate::{
    ArtifactRef, AuthorityRevision, ConfigError, ConfigErrorKind, ConfigLimits, HomeLayout,
    PluginFamily, ReloadCancellation,
};

/// Maximum encoded size of one Manager installation receipt.
pub const MAX_MANAGER_RECEIPT_BYTES: usize = 16 * 1024;
/// Exact Manager installation receipt kind.
pub const MANAGER_RECEIPT_KIND: &str = "mcode-manager-installation-receipt";
/// Exact Manager installation receipt format version.
pub const MANAGER_RECEIPT_FORMAT_VERSION: u32 = 1;

const RECEIPT_MAX_DEPTH: usize = 4;
const RECEIPT_MAX_NODES: usize = 32;

/// Contains one validated, non-authoritative Manager installation receipt.
///
/// The canonical Manager identity is derived from [`PluginFamily`]. This value
/// cannot grant enablement, source binding, trust, selection, loading, routing,
/// or activation authority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagerReceiptDocument {
    revision: AuthorityRevision,
    family: PluginFamily,
    active: ArtifactRef,
}

impl ManagerReceiptDocument {
    /// Returns the positive persisted revision.
    #[must_use]
    pub fn revision(&self) -> AuthorityRevision {
        self.revision
    }

    /// Returns the family from which canonical Manager identity is derived.
    #[must_use]
    pub fn family(&self) -> PluginFamily {
        self.family
    }

    /// Returns the artifact recorded by this non-authoritative receipt.
    #[must_use]
    pub fn active(&self) -> &ArtifactRef {
        &self.active
    }
}

/// Reads one Manager receipt without creating filesystem objects.
///
/// Missing or corrupt receipts are not rebuilt by this operation.
///
/// # Errors
///
/// Returns [`ConfigError`] for owned-path security, bounded strict JSON,
/// family/path mismatch, or receipt validation failures.
pub fn read_manager_receipt(
    home: &HomeLayout,
    family: PluginFamily,
) -> Result<Option<ManagerReceiptDocument>, ConfigError> {
    let relative = receipt_relative_path(family);
    let target = home.manager_installation_json(family);
    let bytes = read_owned_file(home, relative, MAX_MANAGER_RECEIPT_BYTES)
        .map_err(|error| error.with_path(&target))?;
    bytes
        .as_deref()
        .map(|bytes| parse_document(home, family, bytes))
        .transpose()
}

/// Replaces one Manager receipt using a lock-held revision compare-and-swap.
///
/// A missing receipt has logical revision zero. The current receipt is read
/// once, strictly validated, and checked against its path family before the
/// revision comparison. This operation never reads or updates `plugins.json`.
///
/// # Errors
///
/// Returns [`ConfigErrorKind::RevisionConflict`] for a stale expectation,
/// [`ConfigErrorKind::RevisionExhausted`] at `i64::MAX`, and [`ConfigError`] for
/// strict receipt, serialization, or owned-file transaction failures.
pub fn replace_manager_receipt(
    home: &HomeLayout,
    family: PluginFamily,
    expected_revision: AuthorityRevision,
    active: &ArtifactRef,
) -> Result<ManagerReceiptDocument, ConfigError> {
    let relative = receipt_relative_path(family);
    let target = home.manager_installation_json(family);
    let mut written = None;
    locked_update_owned_file(home, relative, MAX_MANAGER_RECEIPT_BYTES, |current| {
        let current_revision = match current {
            Some(bytes) => parse_document(home, family, bytes)?.revision,
            None => AuthorityRevision::ABSENT,
        };
        if current_revision != expected_revision {
            return Err(ConfigError::for_path(
                ConfigErrorKind::RevisionConflict,
                &target,
            ));
        }
        let revision = current_revision
            .checked_next()
            .map_err(|error| error.with_path(&target))?;
        let document = ManagerReceiptDocument {
            revision,
            family,
            active: active.clone(),
        };
        let bytes = serialize_document(&document)?;
        written = Some(document);
        Ok(bytes)
    })
    .map_err(|error| error.with_path(&target))?;
    written.ok_or_else(|| ConfigError::new(ConfigErrorKind::Serialization))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SerializedDocument<'a> {
    format_version: u32,
    kind: &'static str,
    revision: u64,
    family: &'static str,
    active: &'a ArtifactRef,
}

fn serialize_document(document: &ManagerReceiptDocument) -> Result<Vec<u8>, ConfigError> {
    let serialized = SerializedDocument {
        format_version: MANAGER_RECEIPT_FORMAT_VERSION,
        kind: MANAGER_RECEIPT_KIND,
        revision: document.revision.get(),
        family: document.family.directory_name(),
        active: &document.active,
    };
    let mut bytes = serde_json::to_vec(&serialized)
        .map_err(|_| ConfigError::new(ConfigErrorKind::Serialization))?;
    bytes.push(b'\n');
    if bytes.len() > MAX_MANAGER_RECEIPT_BYTES {
        return Err(ConfigError::new(ConfigErrorKind::Serialization));
    }
    Ok(bytes)
}

fn parse_document(
    home: &HomeLayout,
    family: PluginFamily,
    bytes: &[u8],
) -> Result<ManagerReceiptDocument, ConfigError> {
    let target = home.manager_installation_json(family);
    let value = parse_strict_value(bytes, receipt_limits(), &ReloadCancellation::new())
        .map_err(|error| error.without_pointer().with_path(&target))?;
    parse_document_value(value, family).map_err(|error| error.with_path(&target))
}

fn parse_document_value(
    value: Value,
    expected_family: PluginFamily,
) -> Result<ManagerReceiptDocument, ConfigError> {
    let mut root = exact_object(
        value,
        &["formatVersion", "kind", "revision", "family", "active"],
    )?;
    let format_version = root
        .remove("formatVersion")
        .and_then(|value| value.as_u64())
        .and_then(|value| u32::try_from(value).ok())
        .ok_or_else(authority_error)?;
    if format_version != MANAGER_RECEIPT_FORMAT_VERSION
        || take_string(&mut root, "kind")? != MANAGER_RECEIPT_KIND
        || take_string(&mut root, "family")? != expected_family.directory_name()
    {
        return Err(authority_error());
    }
    let revision = take_positive_revision(&mut root, "revision")?;
    let active = root.remove("active").ok_or_else(authority_error)?;
    Ok(ManagerReceiptDocument {
        revision,
        family: expected_family,
        active: parse_active(active)?,
    })
}

fn receipt_relative_path(family: PluginFamily) -> PathBuf {
    PathBuf::from("plugins")
        .join(family.directory_name())
        .join("manager")
        .join("installation.json")
}

fn receipt_limits() -> ConfigLimits {
    ConfigLimits {
        max_source_bytes: MAX_MANAGER_RECEIPT_BYTES,
        max_total_bytes: MAX_MANAGER_RECEIPT_BYTES,
        max_depth: RECEIPT_MAX_DEPTH,
        max_nodes: RECEIPT_MAX_NODES,
        max_sources: 1,
        max_diagnostics: 1,
    }
}

fn authority_error() -> ConfigError {
    ConfigError::new(ConfigErrorKind::AuthorityValidation)
}

//! Strict typed authority for the exact 12-family Manager registry.
//!
//! The registry is a standalone owned document. It does not participate in
//! layered configuration, merge patch, project configuration, or migration.

// Rust guideline compliant 2026-08-29

use std::fmt::{self, Display, Formatter};

use semver::Version;
use serde::ser::{SerializeMap, SerializeStruct};
use serde::{Serialize, Serializer};
use serde_json::{Map, Value};

use crate::parse::parse_strict_value;
use crate::secure_fs::owned_file::{locked_update_owned_file, read_owned_file};
use crate::{
    ConfigError, ConfigErrorKind, ConfigLimits, HomeLayout, PluginFamily, ReloadCancellation,
};

/// Maximum encoded size of `plugins.json`.
pub const MAX_MANAGER_REGISTRY_BYTES: usize = 64 * 1024;
/// Exact Manager registry document kind.
pub const MANAGER_REGISTRY_KIND: &str = "mcode-manager-registry";
/// Exact Manager registry format version.
pub const MANAGER_REGISTRY_FORMAT_VERSION: u32 = 1;

// The schema's deepest valid value is shallow; extra depth and nodes are
// rejected before typed authority validation to bound adversarial documents.
const REGISTRY_MAX_DEPTH: usize = 8;
const REGISTRY_MAX_NODES: usize = 512;
const REGISTRY_PATH: &str = "plugins.json";
const MAX_CANONICAL_VERSION_BYTES: usize = 128;
const MAX_AUTHORITY_REVISION: u64 = i64::MAX as u64;
const SHA256_PREFIX: &str = "sha256:";
const SHA256_HEX_LENGTH: usize = 64;

/// Identifies a logical or persisted authority document revision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AuthorityRevision(u64);

impl AuthorityRevision {
    /// Logical revision used when the authority document is absent.
    pub const ABSENT: Self = Self(0);

    /// Creates a bounded authority revision, including logical absence.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigErrorKind::AuthorityValidation`] above `i64::MAX`.
    pub fn new(value: u64) -> Result<Self, ConfigError> {
        if value > MAX_AUTHORITY_REVISION {
            return Err(authority_error());
        }
        Ok(Self(value))
    }

    /// Returns the numeric revision.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }

    fn checked_next(self) -> Result<Self, ConfigError> {
        if self.0 >= MAX_AUTHORITY_REVISION {
            return Err(ConfigError::new(ConfigErrorKind::RevisionExhausted));
        }
        Ok(Self(self.0 + 1))
    }
}

/// Identifies one signed source binding without assigning signed semantics.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SourceBindingId(String);

impl SourceBindingId {
    /// Parses a strict portable source binding ID.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigErrorKind::AuthorityValidation`] when `value` is not 1
    /// through 64 ASCII bytes matching the frozen lowercase grammar or is a
    /// DOS reserved device name.
    pub fn parse(value: impl AsRef<str>) -> Result<Self, ConfigError> {
        let value = value.as_ref();
        let bytes = value.as_bytes();
        let valid_length = (1..=64).contains(&bytes.len());
        let valid_first = bytes.first().is_some_and(u8::is_ascii_lowercase);
        let valid_tail = bytes.last().is_some_and(u8::is_ascii_alphanumeric);
        let valid_bytes = bytes
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-');
        let valid_hyphens = !bytes.windows(2).any(|pair| pair == b"--");
        if !valid_length
            || !valid_first
            || !valid_tail
            || !valid_bytes
            || !valid_hyphens
            || is_dos_reserved(value)
        {
            return Err(authority_error());
        }
        Ok(Self(value.to_owned()))
    }

    /// Returns the opaque source binding ID.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SourceBindingId {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("SourceBindingId")
            .field(&self.0)
            .finish()
    }
}

impl Display for SourceBindingId {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Serialize for SourceBindingId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

/// Contains a canonical SemVer spelling.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CanonicalVersion(String);

impl CanonicalVersion {
    /// Parses an exact canonical SemVer value.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigErrorKind::AuthorityValidation`] for invalid SemVer or
    /// any accepted spelling that differs from its canonical rendering.
    pub fn parse(value: impl AsRef<str>) -> Result<Self, ConfigError> {
        let value = value.as_ref();
        if value.is_empty() || value.len() > MAX_CANONICAL_VERSION_BYTES || !value.is_ascii() {
            return Err(authority_error());
        }
        let parsed = Version::parse(value).map_err(|_| authority_error())?;
        if parsed.to_string() != value {
            return Err(authority_error());
        }
        Ok(Self(value.to_owned()))
    }

    /// Returns the canonical SemVer spelling.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Display for CanonicalVersion {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        Display::fmt(&self.0, formatter)
    }
}

impl Serialize for CanonicalVersion {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

/// Contains a lowercase `sha256:` artifact digest.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Sha256Digest(String);

impl Sha256Digest {
    /// Parses the exact lowercase SHA-256 digest spelling.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigErrorKind::AuthorityValidation`] unless `value` is
    /// `sha256:` followed by exactly 64 lowercase hexadecimal digits.
    pub fn parse(value: impl AsRef<str>) -> Result<Self, ConfigError> {
        let value = value.as_ref();
        let Some(hex) = value.strip_prefix(SHA256_PREFIX) else {
            return Err(authority_error());
        };
        if hex.len() != SHA256_HEX_LENGTH
            || !hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(authority_error());
        }
        Ok(Self(value.to_owned()))
    }

    /// Returns the canonical digest spelling.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Display for Sha256Digest {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Serialize for Sha256Digest {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

/// Selects one canonical active Manager artifact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactRef {
    version: CanonicalVersion,
    digest: Sha256Digest,
}

impl ArtifactRef {
    /// Creates an active Manager artifact selection.
    #[must_use]
    pub fn new(version: CanonicalVersion, digest: Sha256Digest) -> Self {
        Self { version, digest }
    }

    /// Returns the selected canonical version.
    #[must_use]
    pub fn version(&self) -> &CanonicalVersion {
        &self.version
    }

    /// Returns the selected artifact digest.
    #[must_use]
    pub fn digest(&self) -> &Sha256Digest {
        &self.digest
    }
}

impl Serialize for ArtifactRef {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("ArtifactRef", 2)?;
        state.serialize_field("version", &self.version)?;
        state.serialize_field("digest", &self.digest)?;
        state.end()
    }
}

/// Records the accepted signed manifest high-water mark.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustHighWater {
    sequence: u64,
    manifest_digest: Sha256Digest,
}

impl TrustHighWater {
    /// Creates a positive signed-manifest high-water mark.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigErrorKind::AuthorityValidation`] when `sequence` is not
    /// in `1..=i64::MAX`.
    pub fn new(sequence: u64, manifest_digest: Sha256Digest) -> Result<Self, ConfigError> {
        if sequence == 0 || sequence > MAX_AUTHORITY_REVISION {
            return Err(authority_error());
        }
        Ok(Self {
            sequence,
            manifest_digest,
        })
    }

    /// Returns the signed sequence number.
    #[must_use]
    pub fn sequence(&self) -> u64 {
        self.sequence
    }

    /// Returns the signed manifest digest.
    #[must_use]
    pub fn manifest_digest(&self) -> &Sha256Digest {
        &self.manifest_digest
    }
}

impl Serialize for TrustHighWater {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("TrustHighWater", 2)?;
        state.serialize_field("sequence", &self.sequence)?;
        state.serialize_field("manifestDigest", &self.manifest_digest)?;
        state.end()
    }
}

/// Holds one valid absent or fully installed Manager registry record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagerRecord {
    enabled: bool,
    source: Option<SourceBindingId>,
    active: Option<ArtifactRef>,
    trust_high_water: Option<TrustHighWater>,
}

impl ManagerRecord {
    /// Creates the only valid absent Manager state.
    #[must_use]
    pub const fn absent() -> Self {
        Self {
            enabled: false,
            source: None,
            active: None,
            trust_high_water: None,
        }
    }

    /// Creates a fully installed Manager state.
    #[must_use]
    pub fn installed(
        enabled: bool,
        source: SourceBindingId,
        active: ArtifactRef,
        trust_high_water: TrustHighWater,
    ) -> Self {
        Self {
            enabled,
            source: Some(source),
            active: Some(active),
            trust_high_water: Some(trust_high_water),
        }
    }

    /// Returns whether Host loading is enabled.
    #[must_use]
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Returns the opaque source binding for an installed Manager.
    #[must_use]
    pub fn source(&self) -> Option<&SourceBindingId> {
        self.source.as_ref()
    }

    /// Returns the active artifact for an installed Manager.
    #[must_use]
    pub fn active(&self) -> Option<&ArtifactRef> {
        self.active.as_ref()
    }

    /// Returns the trust high-water mark for an installed Manager.
    #[must_use]
    pub fn trust_high_water(&self) -> Option<&TrustHighWater> {
        self.trust_high_water.as_ref()
    }

    /// Changes enablement while preserving the valid-state invariant.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigErrorKind::AuthorityValidation`] when enabling an absent
    /// Manager.
    pub fn set_enabled(&mut self, enabled: bool) -> Result<(), ConfigError> {
        if enabled && self.source.is_none() {
            return Err(authority_error());
        }
        self.enabled = enabled;
        Ok(())
    }
}

impl Default for ManagerRecord {
    fn default() -> Self {
        Self::absent()
    }
}

impl Serialize for ManagerRecord {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("ManagerRecord", 4)?;
        state.serialize_field("enabled", &self.enabled)?;
        state.serialize_field("source", &self.source)?;
        state.serialize_field("active", &self.active)?;
        state.serialize_field("trustHighWater", &self.trust_high_water)?;
        state.end()
    }
}

/// Stores exactly one valid record for each frozen Plugin family.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagerRegistry {
    records: [ManagerRecord; 12],
}

impl ManagerRegistry {
    /// Creates the exact-12 all-absent registry.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            records: std::array::from_fn(|_| ManagerRecord::absent()),
        }
    }

    /// Returns one family record.
    #[must_use]
    pub fn manager(&self, family: PluginFamily) -> &ManagerRecord {
        &self.records[family.index()]
    }

    /// Replaces one family record with another validated value.
    pub fn set_manager(&mut self, family: PluginFamily, record: ManagerRecord) {
        self.records[family.index()] = record;
    }
}

impl Default for ManagerRegistry {
    fn default() -> Self {
        Self::empty()
    }
}

impl Serialize for ManagerRegistry {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut map = serializer.serialize_map(Some(PluginFamily::ALL.len()))?;
        for family in PluginFamily::ALL {
            map.serialize_entry(family.directory_name(), self.manager(family))?;
        }
        map.end()
    }
}

/// Contains one validated persisted Manager registry revision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagerRegistryDocument {
    revision: AuthorityRevision,
    registry: ManagerRegistry,
}

impl ManagerRegistryDocument {
    /// Returns the positive persisted revision.
    #[must_use]
    pub fn revision(&self) -> AuthorityRevision {
        self.revision
    }

    /// Returns the exact-12 registry value.
    #[must_use]
    pub fn registry(&self) -> &ManagerRegistry {
        &self.registry
    }
}

/// Reads and validates `plugins.json` without creating filesystem objects.
///
/// # Errors
///
/// Returns [`ConfigError`] for owned-path security, bounded strict JSON, or
/// typed authority validation failures.
pub fn read_manager_registry(
    home: &HomeLayout,
) -> Result<Option<ManagerRegistryDocument>, ConfigError> {
    let bytes = read_owned_file(home, REGISTRY_PATH, MAX_MANAGER_REGISTRY_BYTES)?;
    bytes
        .as_deref()
        .map(|bytes| parse_document(home, bytes))
        .transpose()
}

/// Replaces `plugins.json` using one lock-held revision compare-and-swap.
///
/// A missing document has logical revision zero. The current document is fully
/// read and validated before comparing `expected_revision`; successful writes
/// serialize the complete supplied registry at current revision plus one.
///
/// # Errors
///
/// Returns [`ConfigErrorKind::RevisionConflict`] for a stale expectation,
/// [`ConfigErrorKind::RevisionExhausted`] at `i64::MAX`, and [`ConfigError`] for
/// strict authority, serialization, or owned-file transaction failures.
pub fn replace_manager_registry(
    home: &HomeLayout,
    expected_revision: AuthorityRevision,
    registry: &ManagerRegistry,
) -> Result<ManagerRegistryDocument, ConfigError> {
    let mut written = None;
    locked_update_owned_file(home, REGISTRY_PATH, MAX_MANAGER_REGISTRY_BYTES, |current| {
        let current_revision = match current {
            Some(bytes) => parse_document(home, bytes)?.revision,
            None => AuthorityRevision::ABSENT,
        };
        if current_revision != expected_revision {
            return Err(ConfigError::for_path(
                ConfigErrorKind::RevisionConflict,
                &home.plugins_json(),
            ));
        }
        let revision = current_revision
            .checked_next()
            .map_err(|error| error.with_path(&home.plugins_json()))?;
        let document = ManagerRegistryDocument {
            revision,
            registry: registry.clone(),
        };
        let bytes = serialize_document(&document)?;
        written = Some(document);
        Ok(bytes)
    })?;
    written.ok_or_else(|| ConfigError::new(ConfigErrorKind::Serialization))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SerializedDocument<'a> {
    format_version: u32,
    kind: &'static str,
    revision: u64,
    managers: &'a ManagerRegistry,
}

fn serialize_document(document: &ManagerRegistryDocument) -> Result<Vec<u8>, ConfigError> {
    let serialized = SerializedDocument {
        format_version: MANAGER_REGISTRY_FORMAT_VERSION,
        kind: MANAGER_REGISTRY_KIND,
        revision: document.revision.get(),
        managers: &document.registry,
    };
    let mut bytes = serde_json::to_vec(&serialized)
        .map_err(|_| ConfigError::new(ConfigErrorKind::Serialization))?;
    bytes.push(b'\n');
    if bytes.len() > MAX_MANAGER_REGISTRY_BYTES {
        return Err(ConfigError::new(ConfigErrorKind::Serialization));
    }
    Ok(bytes)
}

fn parse_document(home: &HomeLayout, bytes: &[u8]) -> Result<ManagerRegistryDocument, ConfigError> {
    let value = parse_strict_value(bytes, registry_limits(), &ReloadCancellation::new())
        .map_err(|error| error.without_pointer().with_path(&home.plugins_json()))?;
    parse_document_value(value).map_err(|error| error.with_path(&home.plugins_json()))
}

fn parse_document_value(value: Value) -> Result<ManagerRegistryDocument, ConfigError> {
    let mut root = exact_object(value, &["formatVersion", "kind", "revision", "managers"])?;
    if take_u32(&mut root, "formatVersion")? != MANAGER_REGISTRY_FORMAT_VERSION
        || take_string(&mut root, "kind")? != MANAGER_REGISTRY_KIND
    {
        return Err(authority_error());
    }
    let revision = take_positive_revision(&mut root, "revision")?;
    let managers = root.remove("managers").ok_or_else(authority_error)?;
    let registry = parse_registry(managers)?;
    Ok(ManagerRegistryDocument { revision, registry })
}

fn parse_registry(value: Value) -> Result<ManagerRegistry, ConfigError> {
    let mut managers = value.as_object().cloned().ok_or_else(authority_error)?;
    if managers.len() != PluginFamily::ALL.len() {
        return Err(authority_error());
    }
    let mut registry = ManagerRegistry::empty();
    for family in PluginFamily::ALL {
        let value = managers
            .remove(family.directory_name())
            .ok_or_else(authority_error)?;
        registry.set_manager(family, parse_manager_record(value)?);
    }
    if !managers.is_empty() {
        return Err(authority_error());
    }
    Ok(registry)
}

fn parse_manager_record(value: Value) -> Result<ManagerRecord, ConfigError> {
    let mut record = exact_object(value, &["enabled", "source", "active", "trustHighWater"])?;
    let enabled = take_bool(&mut record, "enabled")?;
    let source = take_nullable(&mut record, "source", |value| {
        SourceBindingId::parse(value.as_str().ok_or_else(authority_error)?)
    })?;
    let active = take_nullable(&mut record, "active", parse_active)?;
    let trust = take_nullable(&mut record, "trustHighWater", parse_trust_high_water)?;
    match (source, active, trust) {
        (None, None, None) if !enabled => Ok(ManagerRecord::absent()),
        (Some(source), Some(active), Some(trust)) => {
            Ok(ManagerRecord::installed(enabled, source, active, trust))
        }
        _ => Err(authority_error()),
    }
}

fn parse_active(value: Value) -> Result<ArtifactRef, ConfigError> {
    let mut active = exact_object(value, &["version", "digest"])?;
    let version = CanonicalVersion::parse(take_string(&mut active, "version")?)?;
    let digest = Sha256Digest::parse(take_string(&mut active, "digest")?)?;
    Ok(ArtifactRef::new(version, digest))
}

fn parse_trust_high_water(value: Value) -> Result<TrustHighWater, ConfigError> {
    let mut trust = exact_object(value, &["sequence", "manifestDigest"])?;
    let sequence = take_positive_u64(&mut trust, "sequence")?;
    let digest = Sha256Digest::parse(take_string(&mut trust, "manifestDigest")?)?;
    TrustHighWater::new(sequence, digest)
}

fn exact_object(value: Value, fields: &[&str]) -> Result<Map<String, Value>, ConfigError> {
    let Value::Object(object) = value else {
        return Err(authority_error());
    };
    if object.len() != fields.len() || !fields.iter().all(|field| object.contains_key(*field)) {
        return Err(authority_error());
    }
    Ok(object)
}

fn take_nullable<T>(
    object: &mut Map<String, Value>,
    field: &str,
    parse: impl FnOnce(Value) -> Result<T, ConfigError>,
) -> Result<Option<T>, ConfigError> {
    match object.remove(field).ok_or_else(authority_error)? {
        Value::Null => Ok(None),
        value => parse(value).map(Some),
    }
}

fn take_string(object: &mut Map<String, Value>, field: &str) -> Result<String, ConfigError> {
    object
        .remove(field)
        .and_then(|value| value.as_str().map(str::to_owned))
        .ok_or_else(authority_error)
}

fn take_bool(object: &mut Map<String, Value>, field: &str) -> Result<bool, ConfigError> {
    object
        .remove(field)
        .and_then(|value| value.as_bool())
        .ok_or_else(authority_error)
}

fn take_u32(object: &mut Map<String, Value>, field: &str) -> Result<u32, ConfigError> {
    object
        .remove(field)
        .and_then(|value| value.as_u64())
        .and_then(|value| u32::try_from(value).ok())
        .ok_or_else(authority_error)
}

fn take_positive_revision(
    object: &mut Map<String, Value>,
    field: &str,
) -> Result<AuthorityRevision, ConfigError> {
    AuthorityRevision::new(take_positive_u64(object, field)?)
}

fn take_positive_u64(object: &mut Map<String, Value>, field: &str) -> Result<u64, ConfigError> {
    let value = object
        .remove(field)
        .and_then(|value| value.as_u64())
        .ok_or_else(authority_error)?;
    if value == 0 || value > MAX_AUTHORITY_REVISION {
        return Err(authority_error());
    }
    Ok(value)
}

fn registry_limits() -> ConfigLimits {
    ConfigLimits {
        max_source_bytes: MAX_MANAGER_REGISTRY_BYTES,
        max_total_bytes: MAX_MANAGER_REGISTRY_BYTES,
        max_depth: REGISTRY_MAX_DEPTH,
        max_nodes: REGISTRY_MAX_NODES,
        max_sources: 1,
        max_diagnostics: 1,
    }
}

fn authority_error() -> ConfigError {
    ConfigError::new(ConfigErrorKind::AuthorityValidation)
}

fn is_dos_reserved(value: &str) -> bool {
    matches!(value, "con" | "prn" | "aux" | "nul")
        || value
            .strip_prefix("com")
            .or_else(|| value.strip_prefix("lpt"))
            .is_some_and(|unit| matches!(unit, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9"))
}

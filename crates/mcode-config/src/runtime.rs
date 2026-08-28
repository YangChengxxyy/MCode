//! Immutable snapshots, layered reloads, validation hooks, and publication.

// Rust guideline compliant 2026-08-26

use std::collections::BTreeMap;
use std::fmt::{self, Debug, Display, Formatter};
use std::sync::{Arc, Mutex, RwLock};

use serde_json::Value;

use crate::merge::{ProvenanceMap, merge_patch};
use crate::parse::parse_envelope;
use crate::security::{
    validate_material_credentials, validate_patch_credentials, validate_value_limits,
};
use crate::{
    ConfigError, ConfigErrorKind, ConfigLayer, ConfigLimits, ConfigScope, ConfigSource,
    JsonPointer, ReloadCancellation, SourceTrust,
};

/// Identifies a non-fatal condition retained in a successful snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ConfigDiagnosticCode {
    /// An untrusted project layer was deliberately not read or merged.
    UntrustedProjectSkipped,
}

impl Display for ConfigDiagnosticCode {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::UntrustedProjectSkipped => formatter.write_str("untrusted-project-skipped"),
        }
    }
}

/// Reports a bounded, value-free condition from a successful load.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigDiagnostic {
    code: ConfigDiagnosticCode,
    source: ConfigSource,
}

impl ConfigDiagnostic {
    /// Returns the machine-readable diagnostic category.
    #[must_use]
    pub fn code(&self) -> ConfigDiagnosticCode {
        self.code
    }

    /// Returns the source associated with the diagnostic.
    #[must_use]
    pub fn source(&self) -> &ConfigSource {
        &self.source
    }
}

impl Display for ConfigDiagnostic {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        Display::fmt(&self.code, formatter)
    }
}

// BLAKE3 has one fixed 256-bit default digest, independent of crate features.
const DIGEST_BYTES: usize = 32;
const _: [(); DIGEST_BYTES] = [(); blake3::OUT_LEN];

/// A stable 32-byte BLAKE3 digest of canonical merged JSON.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ConfigDigest([u8; DIGEST_BYTES]);

impl ConfigDigest {
    /// Returns the digest bytes.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl Display for ConfigDigest {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            write!(formatter, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl Debug for ConfigDigest {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("ConfigDigest")
            .field(&self.to_string())
            .finish()
    }
}

/// An immutable, atomically published configuration view.
///
/// The merged value is exposed for typed deserialization by the composition
/// root. Its [`Debug`] implementation never renders that value. Provenance has
/// one entry for every final JSON Pointer, including array elements and the
/// document root.
pub struct ConfigSnapshot {
    generation: u64,
    digest: ConfigDigest,
    value: Value,
    provenance: ProvenanceMap,
    diagnostics: Vec<ConfigDiagnostic>,
}

impl ConfigSnapshot {
    /// Returns the value-change generation, starting at one.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Returns the canonical digest of [`Self::value`].
    #[must_use]
    pub fn digest(&self) -> ConfigDigest {
        self.digest
    }

    /// Returns the merged JSON value for caller-owned typed validation/use.
    #[must_use]
    pub fn value(&self) -> &Value {
        &self.value
    }

    /// Returns final per-pointer source provenance.
    #[must_use]
    pub fn provenance(&self) -> &BTreeMap<JsonPointer, ConfigSource> {
        &self.provenance
    }

    /// Returns the source for an encoded RFC 6901 pointer.
    #[must_use]
    pub fn source_at(&self, pointer: &str) -> Option<&ConfigSource> {
        self.provenance
            .iter()
            .find_map(|(candidate, source)| (candidate.as_str() == pointer).then_some(source))
    }

    /// Returns bounded non-fatal diagnostics from this publication.
    #[must_use]
    pub fn diagnostics(&self) -> &[ConfigDiagnostic] {
        &self.diagnostics
    }
}

impl Debug for ConfigSnapshot {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ConfigSnapshot")
            .field("generation", &self.generation)
            .field("digest", &self.digest)
            .field("value", &"<redacted>")
            .field("provenance_entries", &self.provenance.len())
            .field("diagnostics", &self.diagnostics)
            .finish()
    }
}

/// Opaque rejection returned by a domain validation hook.
///
/// The type intentionally carries no message or source value. This keeps typed
/// serde/schema failures out of configuration errors and diagnostics.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ValidationFailure;

impl ValidationFailure {
    /// Creates an opaque domain-validation rejection.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

/// Validates the merged domain before a snapshot can be published.
pub trait ConfigValidator: Send + Sync {
    /// Validates `value` without mutating it.
    ///
    /// # Errors
    ///
    /// Returns [`ValidationFailure`] when typed serde or schema validation
    /// rejects the domain. The runtime discards any upstream validator detail.
    fn validate(&self, value: &Value) -> Result<(), ValidationFailure>;
}

impl<F> ConfigValidator for F
where
    F: Fn(&Value) -> Result<(), ValidationFailure> + Send + Sync,
{
    fn validate(&self, value: &Value) -> Result<(), ValidationFailure> {
        self(value)
    }
}

/// Explicitly accepts every domain value after foundation checks.
#[derive(Debug, Clone, Copy, Default)]
pub struct AcceptAllConfig;

impl ConfigValidator for AcceptAllConfig {
    fn validate(&self, _value: &Value) -> Result<(), ValidationFailure> {
        Ok(())
    }
}

/// Summarizes a successful reload publication.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReloadOutcome {
    generation: u64,
    digest: ConfigDigest,
    changed: bool,
}

impl ReloadOutcome {
    /// Returns the published generation.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Returns the published value digest.
    #[must_use]
    pub fn digest(&self) -> ConfigDigest {
        self.digest
    }

    /// Reports whether the merged value digest changed.
    #[must_use]
    pub fn changed(&self) -> bool {
        self.changed
    }
}

struct RuntimeInner {
    limits: ConfigLimits,
    snapshot: RwLock<Arc<ConfigSnapshot>>,
    reload: Mutex<()>,
}

/// Owns the current immutable snapshot and serializes reload publication.
///
/// Clones share one runtime. Readers clone an [`Arc<ConfigSnapshot>`] while
/// holding a read lock only briefly; parsing, merging, and domain validation run
/// outside the snapshot lock. Publication replaces the complete `Arc` under a
/// write lock, so readers observe either the old or the new snapshot.
#[derive(Clone)]
pub struct ConfigRuntime {
    inner: Arc<RuntimeInner>,
}

impl ConfigRuntime {
    /// Loads and validates an initial snapshot with default resource limits.
    ///
    /// Sources are stably ordered by [`ConfigScope`], establishing this fixed
    /// precedence: compiled defaults, global, trusted project, then explicit
    /// overrides. Input order is retained within one scope.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] for trust, I/O, strict JSON, envelope, resource,
    /// credential, cancellation, or domain-validation failures. At least one
    /// compiled-defaults layer must participate.
    pub fn load(
        sources: &[ConfigLayer],
        validator: &impl ConfigValidator,
    ) -> Result<Self, ConfigError> {
        Self::load_with_options(
            sources,
            validator,
            ConfigLimits::default(),
            &ReloadCancellation::new(),
        )
    }

    /// Loads and validates an initial snapshot with explicit controls.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] under the same conditions as [`Self::load`], and
    /// also when `limits` is internally inconsistent.
    pub fn load_with_options(
        sources: &[ConfigLayer],
        validator: &impl ConfigValidator,
        limits: ConfigLimits,
        cancellation: &ReloadCancellation,
    ) -> Result<Self, ConfigError> {
        let candidate = build_candidate(sources, validator, limits, cancellation)?;
        let snapshot = Arc::new(ConfigSnapshot {
            generation: 1,
            digest: candidate.digest,
            value: candidate.value,
            provenance: candidate.provenance,
            diagnostics: candidate.diagnostics,
        });
        Ok(Self {
            inner: Arc::new(RuntimeInner {
                limits,
                snapshot: RwLock::new(snapshot),
                reload: Mutex::new(()),
            }),
        })
    }

    /// Returns the current immutable snapshot.
    #[must_use]
    pub fn snapshot(&self) -> Arc<ConfigSnapshot> {
        read_snapshot(&self.inner.snapshot)
    }

    /// Returns the resource limits fixed when this runtime was created.
    #[must_use]
    pub fn limits(&self) -> ConfigLimits {
        self.inner.limits
    }

    /// Reloads all caller-provided sources and atomically publishes on success.
    ///
    /// A failure leaves the old snapshot untouched. Equal value digests retain
    /// the generation while still publishing refreshed provenance and
    /// diagnostics. Reload calls are serialized; readers remain concurrent.
    /// This API is independent of any filesystem watcher.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] for any read, parse, merge, security, resource,
    /// cancellation, or domain-validation failure. A changed value also fails
    /// if the generation counter is exhausted.
    pub fn reload(
        &self,
        sources: &[ConfigLayer],
        validator: &impl ConfigValidator,
        cancellation: &ReloadCancellation,
    ) -> Result<ReloadOutcome, ConfigError> {
        let _reload_guard = self
            .inner
            .reload
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let candidate = build_candidate(sources, validator, self.inner.limits, cancellation)?;
        if cancellation.is_cancelled() {
            return Err(ConfigError::new(ConfigErrorKind::Cancelled));
        }

        let current = self.snapshot();
        let changed = candidate.digest != current.digest;
        let generation = if changed {
            current
                .generation
                .checked_add(1)
                .ok_or_else(|| ConfigError::new(ConfigErrorKind::GenerationExhausted))?
        } else {
            current.generation
        };
        let digest = candidate.digest;
        let next = Arc::new(ConfigSnapshot {
            generation,
            digest,
            value: candidate.value,
            provenance: candidate.provenance,
            diagnostics: candidate.diagnostics,
        });
        let mut publication = self
            .inner
            .snapshot
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        ensure_active(cancellation)?;
        *publication = next;

        Ok(ReloadOutcome {
            generation,
            digest,
            changed,
        })
    }
}

impl Debug for ConfigRuntime {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        let snapshot = self.snapshot();
        formatter
            .debug_struct("ConfigRuntime")
            .field("generation", &snapshot.generation)
            .field("digest", &snapshot.digest)
            .field("value", &"<redacted>")
            .field("limits", &self.inner.limits)
            .finish()
    }
}

struct Candidate {
    digest: ConfigDigest,
    value: Value,
    provenance: ProvenanceMap,
    diagnostics: Vec<ConfigDiagnostic>,
}

fn build_candidate(
    sources: &[ConfigLayer],
    validator: &impl ConfigValidator,
    limits: ConfigLimits,
    cancellation: &ReloadCancellation,
) -> Result<Candidate, ConfigError> {
    if !limits.are_valid() {
        return Err(ConfigError::new(ConfigErrorKind::InvalidLimits));
    }
    if sources.len() > limits.max_sources {
        return Err(ConfigError::new(ConfigErrorKind::TooManySources));
    }
    ensure_active(cancellation)?;

    let mut ordered: Vec<(usize, &ConfigLayer)> = sources.iter().enumerate().collect();
    ordered.sort_by_key(|(input_index, layer)| (layer.source().scope.rank(), *input_index));

    let mut value = Value::Null;
    let mut provenance = ProvenanceMap::new();
    let mut diagnostics = Vec::new();
    let mut total_bytes = 0_usize;
    let mut compiled_defaults_loaded = false;

    for (_, layer) in ordered {
        ensure_active(cancellation)?;
        let source = layer.source();
        match (source.scope, source.trust) {
            (ConfigScope::Project, SourceTrust::Untrusted) => {
                if diagnostics.len() < limits.max_diagnostics {
                    diagnostics.push(ConfigDiagnostic {
                        code: ConfigDiagnosticCode::UntrustedProjectSkipped,
                        source: source.clone(),
                    });
                }
                continue;
            }
            (_, SourceTrust::Untrusted) => {
                return Err(ConfigError::for_source(
                    ConfigErrorKind::UntrustedSource,
                    source,
                ));
            }
            (_, SourceTrust::Trusted) => {}
        }

        let Some(bytes) = layer.read(limits, cancellation)? else {
            continue;
        };
        total_bytes = total_bytes
            .checked_add(bytes.len())
            .ok_or_else(|| ConfigError::for_source(ConfigErrorKind::Oversized, source))?;
        if total_bytes > limits.max_total_bytes {
            return Err(ConfigError::for_source(ConfigErrorKind::Oversized, source));
        }
        if source.scope == ConfigScope::CompiledDefaults {
            compiled_defaults_loaded = true;
        }

        let patch = parse_envelope(&bytes, source, limits, cancellation)?;
        validate_patch_credentials(&patch, Some(source), cancellation)?;
        merge_patch(&mut value, &patch, source, &mut provenance, cancellation)?;
        validate_value_limits(&value, limits, cancellation)?;
    }

    if !compiled_defaults_loaded {
        return Err(ConfigError::new(ConfigErrorKind::MissingCompiledDefaults));
    }
    validate_material_credentials(&value, cancellation)?;
    ensure_active(cancellation)?;
    validator
        .validate(&value)
        .map_err(|_| ConfigError::new(ConfigErrorKind::DomainValidation))?;
    ensure_active(cancellation)?;
    let digest = digest_value(&value, cancellation)?;
    ensure_active(cancellation)?;

    Ok(Candidate {
        digest,
        value,
        provenance,
        diagnostics,
    })
}

fn digest_value(
    value: &Value,
    cancellation: &ReloadCancellation,
) -> Result<ConfigDigest, ConfigError> {
    let mut canonical = Vec::new();
    write_canonical(value, &mut canonical, cancellation)?;
    Ok(ConfigDigest(*blake3::hash(&canonical).as_bytes()))
}

fn write_canonical(
    value: &Value,
    output: &mut Vec<u8>,
    cancellation: &ReloadCancellation,
) -> Result<(), ConfigError> {
    ensure_active(cancellation)?;
    match value {
        Value::Null => output.extend_from_slice(b"null"),
        Value::Bool(true) => output.extend_from_slice(b"true"),
        Value::Bool(false) => output.extend_from_slice(b"false"),
        Value::Number(number) => output.extend_from_slice(number.to_string().as_bytes()),
        Value::String(string) => serde_json::to_writer(output, string)
            .map_err(|_| ConfigError::new(ConfigErrorKind::Serialization))?,
        Value::Array(array) => {
            output.push(b'[');
            for (index, child) in array.iter().enumerate() {
                if index > 0 {
                    output.push(b',');
                }
                write_canonical(child, output, cancellation)?;
            }
            output.push(b']');
        }
        Value::Object(object) => {
            output.push(b'{');
            let mut keys: Vec<&String> = object.keys().collect();
            keys.sort_unstable();
            for (index, key) in keys.into_iter().enumerate() {
                if index > 0 {
                    output.push(b',');
                }
                serde_json::to_writer(&mut *output, key)
                    .map_err(|_| ConfigError::new(ConfigErrorKind::Serialization))?;
                output.push(b':');
                write_canonical(&object[key], output, cancellation)?;
            }
            output.push(b'}');
        }
    }
    Ok(())
}

fn ensure_active(cancellation: &ReloadCancellation) -> Result<(), ConfigError> {
    if cancellation.is_cancelled() {
        Err(ConfigError::new(ConfigErrorKind::Cancelled))
    } else {
        Ok(())
    }
}

fn read_snapshot(snapshot: &RwLock<Arc<ConfigSnapshot>>) -> Arc<ConfigSnapshot> {
    snapshot
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

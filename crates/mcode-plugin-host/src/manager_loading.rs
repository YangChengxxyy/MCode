//! Loads the authoritative fixed set of Manager component candidates.
//!
//! `plugins.json` is the only selection authority. Enabled records select one
//! canonical component path and exact-byte digest; disabled records cause no
//! artifact read. The returned candidate set is complete or no set is returned.

// Rust guideline compliant 2026-08-31.

use std::fmt::{self, Display, Formatter};

use mcode_config::{
    ArtifactRef, AuthorityRevision, HomeLayout, MAX_MANAGER_COMPONENT_BYTES, ManagerRecord,
    PluginFamily, Sha256Digest, read_manager_component, read_manager_registry,
};
use sha2::{Digest, Sha256};

use crate::runtime::{CompiledManagerComponent, PluginRuntime};
use crate::{ComponentLimits, MAX_COMPONENT_BYTES};

pub(crate) const MANAGER_SLOT_COUNT: usize = 12;
const _: [(); MANAGER_SLOT_COUNT] = [(); PluginFamily::ALL.len()];
const _: () = assert!(MAX_MANAGER_COMPONENT_BYTES == MAX_COMPONENT_BYTES);

/// Reports an authoritative Manager candidate loading failure.
///
/// Errors expose only a stable category and the affected frozen family. They
/// never include artifact bytes, filesystem paths, or dependency diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManagerLoadError {
    /// The exact-12 Manager registry could not be read or validated.
    Registry,
    /// The selected family artifact could not be read safely within its bound.
    ComponentRead(PluginFamily),
    /// The selected family artifact was absent.
    ComponentMissing(PluginFamily),
    /// The selected family artifact did not match its exact-byte digest.
    DigestMismatch(PluginFamily),
    /// The selected family artifact failed bounded Manager compilation.
    Compilation(PluginFamily),
}

impl ManagerLoadError {
    /// Returns the affected frozen family when the failure is family-specific.
    #[must_use]
    pub const fn family(self) -> Option<PluginFamily> {
        match self {
            Self::Registry => None,
            Self::ComponentRead(family)
            | Self::ComponentMissing(family)
            | Self::DigestMismatch(family)
            | Self::Compilation(family) => Some(family),
        }
    }
}

impl Display for ManagerLoadError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Registry => formatter.write_str("Manager registry is unavailable"),
            Self::ComponentRead(family) => write!(
                formatter,
                "Manager component read failed for {}",
                family.directory_name()
            ),
            Self::ComponentMissing(family) => write!(
                formatter,
                "Manager component is missing for {}",
                family.directory_name()
            ),
            Self::DigestMismatch(family) => write!(
                formatter,
                "Manager component digest mismatched for {}",
                family.directory_name()
            ),
            Self::Compilation(family) => write!(
                formatter,
                "Manager component compilation failed for {}",
                family.directory_name()
            ),
        }
    }
}

impl std::error::Error for ManagerLoadError {}

/// Holds one compiled Manager candidate and its registry identity.
///
/// The executable component remains opaque. It can only be consumed as the
/// policy-bound [`CompiledManagerComponent`] needed by runtime publication.
pub struct CompiledManagerCandidate {
    family: PluginFamily,
    artifact: ArtifactRef,
    component: CompiledManagerComponent,
}

impl CompiledManagerCandidate {
    /// Returns the frozen family selected for this candidate.
    #[must_use]
    pub const fn family(&self) -> PluginFamily {
        self.family
    }

    /// Returns the exact active artifact identity from the registry.
    #[must_use]
    pub fn artifact(&self) -> &ArtifactRef {
        &self.artifact
    }

    /// Consumes this candidate and returns its opaque compiled component.
    #[must_use]
    pub(crate) fn into_component(self) -> CompiledManagerComponent {
        self.component
    }
}

/// Contains the complete fixed-12 Manager compilation candidate set.
///
/// Each slot is keyed by one frozen [`PluginFamily`]. Disabled or absent
/// records leave their slot empty; enabled records contain one fully compiled
/// candidate. Construction is atomic: loading failure returns no set.
pub struct ManagerCandidates {
    revision: AuthorityRevision,
    authority: [ManagerRecord; MANAGER_SLOT_COUNT],
    slots: [Option<CompiledManagerCandidate>; MANAGER_SLOT_COUNT],
}

impl ManagerCandidates {
    fn empty(revision: AuthorityRevision) -> Self {
        Self {
            revision,
            authority: std::array::from_fn(|_| ManagerRecord::absent()),
            slots: std::array::from_fn(|_| None),
        }
    }

    fn with_authority(
        revision: AuthorityRevision,
        authority: [ManagerRecord; MANAGER_SLOT_COUNT],
    ) -> Self {
        Self {
            revision,
            authority,
            slots: std::array::from_fn(|_| None),
        }
    }

    /// Returns the registry revision that selected every slot.
    #[must_use]
    pub const fn revision(&self) -> AuthorityRevision {
        self.revision
    }

    /// Returns the compiled candidate for `family`, when enabled.
    #[must_use]
    pub fn get(&self, family: PluginFamily) -> Option<&CompiledManagerCandidate> {
        self.slots[family_index(family)].as_ref()
    }

    /// Removes and returns the compiled candidate for `family`, when enabled.
    #[must_use]
    pub(crate) fn take(&mut self, family: PluginFamily) -> Option<CompiledManagerCandidate> {
        self.slots[family_index(family)].take()
    }

    pub(crate) fn authority_record(&self, family: PluginFamily) -> &ManagerRecord {
        &self.authority[family_index(family)]
    }

    /// Iterates enabled candidates in frozen family order.
    pub fn iter(&self) -> impl Iterator<Item = &CompiledManagerCandidate> {
        self.slots.iter().filter_map(Option::as_ref)
    }

    /// Returns the number of enabled compiled candidates.
    #[must_use]
    pub fn len(&self) -> usize {
        self.iter().count()
    }

    /// Returns whether no Manager candidate is enabled.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.slots.iter().all(Option::is_none)
    }
}

/// Loads all enabled Manager components selected by `plugins.json`.
///
/// A missing registry yields the fixed all-empty set at
/// [`AuthorityRevision::ABSENT`]. Every enabled record reads only its active
/// canonical component path. All SHA-256 checks over exact bytes finish before
/// the runtime scanner is reachable; the verified binaries then use one atomic
/// Manager compilation batch with the fixed default bound. Disabled records do
/// not read component bytes.
///
/// # Errors
///
/// Returns [`ManagerLoadError::Registry`] when the registry cannot be read,
/// or a family-specific error when an enabled component cannot be read, is
/// missing, mismatches its digest, or fails bounded Manager compilation. Any
/// error discards the entire candidate set.
pub fn load_manager_candidates(
    home: &HomeLayout,
    runtime: &PluginRuntime,
) -> Result<ManagerCandidates, ManagerLoadError> {
    let Some(document) = read_manager_registry(home).map_err(|_| ManagerLoadError::Registry)?
    else {
        return Ok(ManagerCandidates::empty(AuthorityRevision::ABSENT));
    };

    let mut verified = Vec::with_capacity(MANAGER_SLOT_COUNT);
    for family in PluginFamily::ALL {
        let record = document.registry().manager(family);
        if !record.enabled() {
            continue;
        }
        let Some(active) = record.active() else {
            return Err(ManagerLoadError::Registry);
        };
        let bytes = read_manager_component(home, family, active.version())
            .map_err(|_| ManagerLoadError::ComponentRead(family))?
            .ok_or(ManagerLoadError::ComponentMissing(family))?;
        if !digest_matches(bytes.as_slice(), active.digest()) {
            return Err(ManagerLoadError::DigestMismatch(family));
        }
        verified.push((family, active.clone(), bytes));
    }

    let binaries = verified
        .iter()
        .map(|(_, _, bytes)| bytes.as_slice())
        .collect::<Vec<_>>();
    let compiled = runtime
        .compile_manager_batch(&binaries, ComponentLimits::default())
        .map_err(|error| {
            verified
                .get(error.index())
                .map_or(ManagerLoadError::Registry, |(family, _, _)| {
                    ManagerLoadError::Compilation(*family)
                })
        })?;
    if compiled.len() != verified.len() {
        return Err(ManagerLoadError::Registry);
    }

    let authority = PluginFamily::ALL.map(|family| document.registry().manager(family).clone());
    let mut candidates = ManagerCandidates::with_authority(document.revision(), authority);
    for ((family, artifact, _bytes), component) in verified.into_iter().zip(compiled) {
        candidates.slots[family_index(family)] = Some(CompiledManagerCandidate {
            family,
            artifact,
            component,
        });
    }
    Ok(candidates)
}

/// Compares bytes with one canonical lowercase SHA-256 authority digest.
pub(crate) fn digest_matches(bytes: &[u8], expected: &Sha256Digest) -> bool {
    const PREFIX: &[u8; 7] = b"sha256:";
    const LOWER_HEX: &[u8; 16] = b"0123456789abcdef";

    let digest = Sha256::digest(bytes);
    let mut encoded = [0_u8; 71];
    encoded[..PREFIX.len()].copy_from_slice(PREFIX);
    for (index, byte) in digest.iter().copied().enumerate() {
        let offset = PREFIX.len() + index * 2;
        encoded[offset] = LOWER_HEX[usize::from(byte >> 4)];
        encoded[offset + 1] = LOWER_HEX[usize::from(byte & 0x0f)];
    }
    expected.as_str().as_bytes() == encoded
}

pub(crate) const fn family_index(family: PluginFamily) -> usize {
    match family {
        PluginFamily::Providers => 0,
        PluginFamily::Session => 1,
        PluginFamily::Compaction => 2,
        PluginFamily::Resources => 3,
        PluginFamily::Ask => 4,
        PluginFamily::Todo => 5,
        PluginFamily::Web => 6,
        PluginFamily::Mcp => 7,
        PluginFamily::Usage => 8,
        PluginFamily::Subagents => 9,
        PluginFamily::Workspace => 10,
        PluginFamily::Ui => 11,
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;

    pub(crate) fn candidates(
        revision: AuthorityRevision,
        candidates: Vec<(PluginFamily, ManagerRecord, CompiledManagerComponent)>,
    ) -> ManagerCandidates {
        let mut set = ManagerCandidates::empty(revision);
        for (family, record, component) in candidates {
            assert!(record.enabled(), "test candidate must be enabled");
            let artifact = record
                .active()
                .expect("enabled test candidate has an artifact")
                .clone();
            let slot = &mut set.slots[family_index(family)];
            assert!(slot.is_none(), "test candidate family must be unique");
            set.authority[family_index(family)] = record;
            *slot = Some(CompiledManagerCandidate {
                family,
                artifact,
                component,
            });
        }
        set
    }
}

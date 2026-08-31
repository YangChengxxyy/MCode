//! Loads one exact Pack candidate for one exact current Manager generation.
//!
//! The loader never discovers Pack IDs, scans directories, accepts caller
//! paths, or treats the selected bundle digest as the component digest.

// Rust guideline compliant 2026-08-31.

use std::fmt::{self, Display, Formatter};

use mcode_config::{
    ArtifactRef, AuthorityRevision, HomeLayout, MAX_PACK_COMPONENT_BYTES, PackId, PluginFamily,
    Sha256Digest, SourceBindingId, TrustHighWater, read_pack_component, read_pack_installation,
};

use crate::manager_director::{
    CurrentManagerGeneration, ManagerGenerationCallError, ManagerGenerationDirector,
};
use crate::manager_loading::digest_matches;
use crate::runtime::{CompiledPackComponent, PluginRuntime};
use crate::{ComponentLimits, ComponentWorld, MAX_COMPONENT_BYTES};

const _: () = assert!(MAX_PACK_COMPONENT_BYTES == MAX_COMPONENT_BYTES);

#[cfg(test)]
pub(super) struct PackLoadCheckpoint {
    reached: std::sync::mpsc::SyncSender<()>,
    resume: std::sync::mpsc::Receiver<()>,
}

#[cfg(test)]
impl PackLoadCheckpoint {
    pub(super) const fn new(
        reached: std::sync::mpsc::SyncSender<()>,
        resume: std::sync::mpsc::Receiver<()>,
    ) -> Self {
        Self { reached, resume }
    }

    fn pause(&self) {
        self.reached
            .send(())
            .expect("Pack load checkpoint receiver must remain available");
        self.resume
            .recv()
            .expect("Pack load checkpoint resume sender must remain available");
    }
}

/// Reports one stable, non-sensitive Pack loading failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PackLoadError {
    /// The bound Manager generation is no longer current.
    StaleManager,
    /// The Manager director has begun final shutdown.
    Closed,
    /// Current Manager selection is unavailable.
    Unavailable,
    /// The exact Pack installation authority could not be read.
    InstallationRead,
    /// The exact Pack installation authority is absent.
    InstallationMissing,
    /// The canonical component is not declared in the inventory.
    ComponentUndeclared,
    /// The exact canonical component could not be read securely.
    ComponentRead,
    /// The exact canonical component is absent.
    ComponentMissing,
    /// Component bytes do not match the canonical inventory digest.
    DigestMismatch,
    /// The component does not compile for the bound family world.
    Compilation,
}

impl Display for PackLoadError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::StaleManager => "bound Manager generation is stale",
            Self::Closed => "Manager generation director is closed",
            Self::Unavailable => "Manager generation selection is unavailable",
            Self::InstallationRead => "Pack installation authority is unreadable",
            Self::InstallationMissing => "Pack installation authority is missing",
            Self::ComponentUndeclared => "Pack component is not declared by its inventory",
            Self::ComponentRead => "Pack component is unreadable",
            Self::ComponentMissing => "Pack component is missing",
            Self::DigestMismatch => "Pack component digest does not match its inventory",
            Self::Compilation => "Pack component compilation failed",
        })
    }
}

impl std::error::Error for PackLoadError {}

/// Retains one verified Pack candidate without creating a Store or instance.
pub(crate) struct CompiledPackCandidate {
    manager: CurrentManagerGeneration,
    verified: VerifiedPackCandidate,
}

pub(crate) struct VerifiedPackCandidate {
    family: PluginFamily,
    pack_id: PackId,
    installation_revision: AuthorityRevision,
    source: SourceBindingId,
    selected: ArtifactRef,
    trust_high_water: TrustHighWater,
    component_digest: Sha256Digest,
    world: ComponentWorld,
    component: CompiledPackComponent,
}

impl CompiledPackCandidate {
    /// Returns the exact Manager selection stamped at final validation.
    pub(crate) const fn manager(&self) -> &CurrentManagerGeneration {
        &self.manager
    }

    /// Returns the Manager-derived Pack family.
    pub(crate) const fn family(&self) -> PluginFamily {
        self.verified.family
    }

    /// Returns the exact explicitly requested Pack ID.
    pub(crate) const fn pack_id(&self) -> &PackId {
        &self.verified.pack_id
    }

    /// Returns the installation revision used for this candidate.
    pub(crate) const fn installation_revision(&self) -> AuthorityRevision {
        self.verified.installation_revision
    }

    /// Returns the installation source binding.
    pub(crate) const fn source(&self) -> &SourceBindingId {
        &self.verified.source
    }

    /// Returns the mechanically selected bundle artifact.
    pub(crate) const fn selected(&self) -> &ArtifactRef {
        &self.verified.selected
    }

    /// Returns the installation trust high-water.
    pub(crate) const fn trust_high_water(&self) -> &TrustHighWater {
        &self.verified.trust_high_water
    }

    /// Returns the canonical component inventory digest.
    pub(crate) const fn component_digest(&self) -> &Sha256Digest {
        &self.verified.component_digest
    }

    /// Returns the Manager-derived typed Pack world.
    pub(crate) const fn world(&self) -> ComponentWorld {
        self.verified.world
    }

    /// Consumes this candidate into its opaque compiled component.
    pub(crate) fn into_component(self) -> CompiledPackComponent {
        self.verified.component
    }
}

impl VerifiedPackCandidate {
    pub(crate) const fn component(&self) -> &CompiledPackComponent {
        &self.component
    }
}

/// Loads exact Pack IDs for one opaque current Manager generation.
pub(crate) struct CurrentManagerPackService<'a> {
    director: &'a ManagerGenerationDirector,
    runtime: &'a PluginRuntime,
    home: &'a HomeLayout,
    expected: CurrentManagerGeneration,
    #[cfg(test)]
    checkpoint: Option<PackLoadCheckpoint>,
}

impl CurrentManagerPackService<'_> {
    /// Loads one exact Pack candidate without discovery or instantiation.
    ///
    /// # Errors
    ///
    /// Returns [`PackLoadError`] for a stale Manager, unreadable exact
    /// authority or bytes, digest mismatch, or exact-world compilation failure.
    pub(crate) fn load_candidate(
        &self,
        pack_id: &PackId,
    ) -> Result<CompiledPackCandidate, PackLoadError> {
        let initial = self
            .director
            .select_current(&self.expected)
            .map_err(map_selection_error)?;
        let family = initial.generation().family();
        let verified = load_verified_pack(self.runtime, self.home, family, pack_id)?;
        #[cfg(test)]
        if let Some(checkpoint) = &self.checkpoint {
            checkpoint.pause();
        }
        let final_selection = self
            .director
            .select_current(&self.expected)
            .map_err(map_selection_error)?;
        let manager = final_selection.generation().clone();
        drop(initial);
        Ok(CompiledPackCandidate { manager, verified })
    }

    #[cfg(test)]
    pub(super) fn with_checkpoint(mut self, checkpoint: PackLoadCheckpoint) -> Self {
        self.checkpoint = Some(checkpoint);
        self
    }
}

pub(crate) fn load_verified_pack(
    runtime: &PluginRuntime,
    home: &HomeLayout,
    family: PluginFamily,
    pack_id: &PackId,
) -> Result<VerifiedPackCandidate, PackLoadError> {
    let world = pack_world(family);
    let document = read_pack_installation(home, family, pack_id)
        .map_err(|_| PackLoadError::InstallationRead)?
        .ok_or(PackLoadError::InstallationMissing)?;
    let installation = document.installation();
    let component_digest = installation
        .component_digest()
        .ok_or(PackLoadError::ComponentUndeclared)?
        .clone();
    let bytes = read_pack_component(home, family, pack_id, installation.selected().version())
        .map_err(|_| PackLoadError::ComponentRead)?
        .ok_or(PackLoadError::ComponentMissing)?;
    if !digest_matches(&bytes, &component_digest) {
        return Err(PackLoadError::DigestMismatch);
    }
    let component = runtime
        .compile_pack(&bytes, world, ComponentLimits::default())
        .map_err(|_| PackLoadError::Compilation)?;
    Ok(VerifiedPackCandidate {
        family,
        pack_id: pack_id.clone(),
        installation_revision: document.revision(),
        source: installation.source().clone(),
        selected: installation.selected().clone(),
        trust_high_water: installation.trust_high_water().clone(),
        component_digest,
        world,
        component,
    })
}

impl ManagerGenerationDirector {
    /// Binds typed Pack loading to one exact current Manager generation.
    ///
    /// # Errors
    ///
    /// Returns [`PackLoadError`] when `expected` is foreign, stale, closed, or
    /// unavailable at the binding boundary.
    pub(crate) fn bind_pack_service<'a>(
        &'a self,
        expected: &CurrentManagerGeneration,
    ) -> Result<CurrentManagerPackService<'a>, PackLoadError> {
        let selection = self.select_current(expected).map_err(map_selection_error)?;
        drop(selection);
        Ok(CurrentManagerPackService {
            director: self,
            runtime: self.runtime(),
            home: self.pack_home(),
            expected: expected.clone(),
            #[cfg(test)]
            checkpoint: None,
        })
    }
}

const fn pack_world(family: PluginFamily) -> ComponentWorld {
    match family {
        PluginFamily::Providers => ComponentWorld::Provider,
        PluginFamily::Session => ComponentWorld::Session,
        PluginFamily::Compaction => ComponentWorld::Compaction,
        PluginFamily::Resources => ComponentWorld::Resources,
        PluginFamily::Ask => ComponentWorld::Ask,
        PluginFamily::Todo => ComponentWorld::Todo,
        PluginFamily::Web => ComponentWorld::Web,
        PluginFamily::Mcp => ComponentWorld::Mcp,
        PluginFamily::Usage => ComponentWorld::Usage,
        PluginFamily::Subagents => ComponentWorld::Subagents,
        PluginFamily::Workspace => ComponentWorld::Workspace,
        PluginFamily::Ui => ComponentWorld::Ui,
    }
}

fn map_selection_error(error: ManagerGenerationCallError) -> PackLoadError {
    match error {
        ManagerGenerationCallError::Stale => PackLoadError::StaleManager,
        ManagerGenerationCallError::Closed => PackLoadError::Closed,
        ManagerGenerationCallError::Unavailable
        | ManagerGenerationCallError::Cancelled(_)
        | ManagerGenerationCallError::Runtime(_)
        | ManagerGenerationCallError::SelectedUnavailable(_) => PackLoadError::Unavailable,
    }
}

#[cfg(test)]
pub(crate) mod tests;

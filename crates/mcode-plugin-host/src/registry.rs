//! Transactional registry with immutable generation snapshots.

// Rust guideline compliant 2026-08-26.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::{Debug, Formatter};
use std::sync::{Arc, Mutex, MutexGuard};

use mcode_plugin_api::{
    CommandDescriptor, EventSubscriptionDescriptor, Identifier, MAX_HOST_BINDINGS_BYTES,
    ModalDescriptor, PluginId, PluginManifest, PromptDescriptor, Provenance, ResourceDescriptor,
    TimelineDescriptor, ToolDescriptor, ViewDescriptor, WidgetDescriptor,
};
use semver::Version;
use serde::{Deserialize, Serialize};

/// Host binding JSON format version.
pub const HOST_BINDINGS_VERSION: u32 = 1;

/// One exact owner selected for a colliding tool name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ToolBindingTarget {
    plugin_id: PluginId,
    version: String,
}

impl ToolBindingTarget {
    /// Returns the selected plugin id.
    #[must_use]
    pub fn plugin_id(&self) -> &PluginId {
        &self.plugin_id
    }

    /// Returns the selected plugin version.
    #[must_use]
    pub fn version(&self) -> &str {
        &self.version
    }
}

/// Strict host-owned collision bindings loaded from JSON.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HostBindings {
    tool_binding: BTreeMap<Identifier, ToolBindingTarget>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RawHostBindings {
    binding_version: u32,
    #[serde(default)]
    tool_binding: BTreeMap<Identifier, ToolBindingTarget>,
}

impl HostBindings {
    /// Returns host bindings with no collision overrides.
    #[must_use]
    pub fn empty() -> Self {
        Self::default()
    }

    /// Parses strict host JSON bindings.
    ///
    /// # Errors
    ///
    /// Returns [`HostBindingsError`] for oversized or invalid JSON.
    pub fn parse_json(bytes: &[u8]) -> Result<Self, HostBindingsError> {
        if bytes.len() > MAX_HOST_BINDINGS_BYTES {
            return Err(HostBindingsError::TooLarge);
        }
        let value = serde_json::from_slice::<serde_json::Value>(bytes)
            .map_err(|_| HostBindingsError::InvalidJson)?;
        let raw: RawHostBindings =
            serde_json::from_value(value).map_err(|_| HostBindingsError::InvalidJson)?;
        if raw.binding_version != HOST_BINDINGS_VERSION {
            return Err(HostBindingsError::UnsupportedVersion);
        }
        if raw
            .tool_binding
            .values()
            .any(|target| Version::parse(&target.version).is_err())
        {
            return Err(HostBindingsError::InvalidVersion);
        }
        Ok(Self {
            tool_binding: raw.tool_binding,
        })
    }
}

/// Host binding parse failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum HostBindingsError {
    /// JSON exceeded the hard byte limit.
    #[error("host plugin bindings exceed their size limit")]
    TooLarge,
    /// JSON was malformed or contained unknown fields.
    #[error("host plugin bindings JSON is invalid")]
    InvalidJson,
    /// Binding schema version is unsupported.
    #[error("host plugin bindings version is unsupported")]
    UnsupportedVersion,
    /// A binding target version was invalid.
    #[error("host plugin binding target version is invalid")]
    InvalidVersion,
}

/// One manifest plus immutable provenance prepared for registration.
#[derive(Debug, Clone, PartialEq)]
pub struct PluginRegistration {
    manifest: PluginManifest,
    provenance: Provenance,
}

impl PluginRegistration {
    /// Creates a registration whose identity matches its manifest.
    ///
    /// # Errors
    ///
    /// Returns [`RegistryError::ProvenanceMismatch`] when plugin id or version
    /// differs from the validated manifest.
    pub fn new(manifest: PluginManifest, provenance: Provenance) -> Result<Self, RegistryError> {
        if manifest.id() != provenance.plugin_id() || manifest.version() != provenance.version() {
            return Err(RegistryError::ProvenanceMismatch);
        }
        Ok(Self {
            manifest,
            provenance,
        })
    }

    /// Returns the validated manifest.
    #[must_use]
    pub fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    /// Returns immutable provenance.
    #[must_use]
    pub fn provenance(&self) -> &Provenance {
        &self.provenance
    }
}

/// Descriptor paired with stable plugin provenance.
#[derive(Debug, Clone, PartialEq)]
pub struct RegisteredContribution<T> {
    descriptor: T,
    provenance: Provenance,
}

impl<T> RegisteredContribution<T> {
    /// Returns the typed descriptor.
    #[must_use]
    pub fn descriptor(&self) -> &T {
        &self.descriptor
    }

    /// Returns stable owner provenance.
    #[must_use]
    pub fn provenance(&self) -> &Provenance {
        &self.provenance
    }
}

#[derive(Debug, Clone, Default)]
struct SnapshotData {
    generation: u64,
    plugins: BTreeMap<PluginId, PluginRegistration>,
    tools: BTreeMap<Identifier, RegisteredContribution<ToolDescriptor>>,
    commands: BTreeMap<Identifier, RegisteredContribution<CommandDescriptor>>,
    prompts: BTreeMap<Identifier, RegisteredContribution<PromptDescriptor>>,
    resources: BTreeMap<Identifier, RegisteredContribution<ResourceDescriptor>>,
    views: BTreeMap<Identifier, RegisteredContribution<ViewDescriptor>>,
    timelines: BTreeMap<Identifier, RegisteredContribution<TimelineDescriptor>>,
    modals: BTreeMap<Identifier, RegisteredContribution<ModalDescriptor>>,
    widgets: BTreeMap<Identifier, RegisteredContribution<WidgetDescriptor>>,
    subscriptions: BTreeMap<Identifier, RegisteredContribution<EventSubscriptionDescriptor>>,
}

/// Immutable registry view at one monotonic generation.
#[derive(Debug, Clone)]
pub struct RegistrySnapshot {
    inner: Arc<SnapshotData>,
}

impl RegistrySnapshot {
    /// Returns the monotonic generation.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.inner.generation
    }

    /// Returns one registered plugin by id.
    #[must_use]
    pub fn plugin(&self, id: &str) -> Option<&PluginRegistration> {
        self.inner.plugins.get(id)
    }

    /// Returns one effective tool by callable name.
    #[must_use]
    pub fn tool(&self, name: &str) -> Option<&RegisteredContribution<ToolDescriptor>> {
        self.inner.tools.get(name)
    }

    /// Iterates plugins in stable id order.
    pub fn plugins(&self) -> impl Iterator<Item = &PluginRegistration> {
        self.inner.plugins.values()
    }

    /// Iterates effective tools in stable name order.
    pub fn tools(&self) -> impl Iterator<Item = &RegisteredContribution<ToolDescriptor>> {
        self.inner.tools.values()
    }

    /// Iterates commands in stable name order.
    pub fn commands(&self) -> impl Iterator<Item = &RegisteredContribution<CommandDescriptor>> {
        self.inner.commands.values()
    }

    /// Iterates prompt descriptors.
    pub fn prompts(&self) -> impl Iterator<Item = &RegisteredContribution<PromptDescriptor>> {
        self.inner.prompts.values()
    }

    /// Iterates resources.
    pub fn resources(&self) -> impl Iterator<Item = &RegisteredContribution<ResourceDescriptor>> {
        self.inner.resources.values()
    }

    /// Iterates views.
    pub fn views(&self) -> impl Iterator<Item = &RegisteredContribution<ViewDescriptor>> {
        self.inner.views.values()
    }

    /// Iterates timelines.
    pub fn timelines(&self) -> impl Iterator<Item = &RegisteredContribution<TimelineDescriptor>> {
        self.inner.timelines.values()
    }

    /// Iterates modals.
    pub fn modals(&self) -> impl Iterator<Item = &RegisteredContribution<ModalDescriptor>> {
        self.inner.modals.values()
    }

    /// Iterates widgets.
    pub fn widgets(&self) -> impl Iterator<Item = &RegisteredContribution<WidgetDescriptor>> {
        self.inner.widgets.values()
    }

    /// Iterates event subscriptions.
    pub fn event_subscriptions(
        &self,
    ) -> impl Iterator<Item = &RegisteredContribution<EventSubscriptionDescriptor>> {
        self.inner.subscriptions.values()
    }
}

#[derive(Debug)]
struct RegistryState {
    snapshot: Arc<SnapshotData>,
}

/// Transactional plugin registry.
#[derive(Clone)]
pub struct PluginRegistry {
    inner: Arc<Mutex<RegistryState>>,
}

impl PluginRegistry {
    /// Creates an empty generation-zero registry.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(RegistryState {
                snapshot: Arc::new(SnapshotData::default()),
            })),
        }
    }

    /// Returns a cheap immutable snapshot handle.
    #[must_use]
    pub fn snapshot(&self) -> RegistrySnapshot {
        RegistrySnapshot {
            inner: self.lock().snapshot.clone(),
        }
    }

    /// Prepares one or more mutations against the current generation.
    ///
    /// # Errors
    ///
    /// Returns [`RegistryError`] for duplicate registrations or missing targets.
    pub fn prepare(
        &self,
        changes: impl IntoIterator<Item = RegistryChange>,
        bindings: HostBindings,
    ) -> Result<RegistryTransaction, RegistryError> {
        let snapshot = self.lock().snapshot.clone();
        let mut plugins = snapshot.plugins.clone();
        let mut touched = BTreeSet::new();
        for change in changes {
            let id = change.plugin_id().clone();
            if !touched.insert(id.clone()) {
                return Err(RegistryError::DuplicateMutation(id));
            }
            match change {
                RegistryChange::Register(registration) => {
                    if plugins.contains_key(&id) {
                        return Err(RegistryError::PluginAlreadyRegistered(id));
                    }
                    plugins.insert(id, registration);
                }
                RegistryChange::Unregister { .. } => {
                    if plugins.remove(&id).is_none() {
                        return Err(RegistryError::PluginNotRegistered(id));
                    }
                }
                RegistryChange::Reload(registration) => {
                    if !plugins.contains_key(&id) {
                        return Err(RegistryError::PluginNotRegistered(id));
                    }
                    plugins.insert(id, registration);
                }
            }
        }
        Ok(RegistryTransaction {
            registry: self.clone(),
            base_generation: snapshot.generation,
            plugins,
            bindings,
            candidate: None,
            status: TransactionStatus::Prepared,
        })
    }

    fn lock(&self) -> MutexGuard<'_, RegistryState> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl Default for PluginRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl Debug for PluginRegistry {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        let snapshot = self.snapshot();
        formatter
            .debug_struct("PluginRegistry")
            .field("generation", &snapshot.generation())
            .field("plugin_count", &snapshot.plugins().count())
            .finish()
    }
}

/// One atomic registry mutation.
#[derive(Debug, Clone, PartialEq)]
pub enum RegistryChange {
    /// Add a previously absent plugin.
    Register(PluginRegistration),
    /// Remove a plugin by id.
    Unregister {
        /// Plugin id to remove.
        plugin_id: PluginId,
    },
    /// Atomically replace one plugin version and all its descriptors.
    Reload(PluginRegistration),
}

impl RegistryChange {
    fn plugin_id(&self) -> &PluginId {
        match self {
            Self::Register(registration) | Self::Reload(registration) => {
                registration.provenance().plugin_id()
            }
            Self::Unregister { plugin_id } => plugin_id,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TransactionStatus {
    Prepared,
    Validated,
    Committed,
    RolledBack,
}

/// Prepared registry transaction.
pub struct RegistryTransaction {
    registry: PluginRegistry,
    base_generation: u64,
    plugins: BTreeMap<PluginId, PluginRegistration>,
    bindings: HostBindings,
    candidate: Option<Arc<SnapshotData>>,
    status: TransactionStatus,
}

impl RegistryTransaction {
    /// Validates trust, descriptors, collisions, and explicit tool bindings.
    ///
    /// # Errors
    ///
    /// Returns [`RegistryError`] for untrusted plugins or unresolved collisions.
    pub fn validate(&mut self) -> Result<(), RegistryError> {
        if self.status != TransactionStatus::Prepared {
            return Err(RegistryError::InvalidTransactionState);
        }
        let generation = self
            .base_generation
            .checked_add(1)
            .ok_or(RegistryError::GenerationExhausted)?;
        self.candidate = Some(Arc::new(build_snapshot(
            generation,
            self.plugins.clone(),
            &self.bindings,
        )?));
        self.status = TransactionStatus::Validated;
        Ok(())
    }

    /// Atomically publishes the validated snapshot.
    ///
    /// # Errors
    ///
    /// Returns [`RegistryError::NotValidated`] or a generation conflict.
    pub fn commit(mut self) -> Result<RegistrySnapshot, RegistryError> {
        if self.status != TransactionStatus::Validated {
            return Err(RegistryError::NotValidated);
        }
        let candidate = self.candidate.take().ok_or(RegistryError::NotValidated)?;
        {
            let mut state = self.registry.lock();
            if state.snapshot.generation != self.base_generation {
                return Err(RegistryError::GenerationConflict {
                    expected: self.base_generation,
                    actual: state.snapshot.generation,
                });
            }
            state.snapshot = candidate.clone();
        }
        self.status = TransactionStatus::Committed;
        Ok(RegistrySnapshot { inner: candidate })
    }

    /// Explicitly rolls back without changing the registry.
    #[must_use]
    pub fn rollback(mut self) -> RegistrySnapshot {
        self.status = TransactionStatus::RolledBack;
        self.registry.snapshot()
    }
}

fn build_snapshot(
    generation: u64,
    plugins: BTreeMap<PluginId, PluginRegistration>,
    bindings: &HostBindings,
) -> Result<SnapshotData, RegistryError> {
    for registration in plugins.values() {
        if !registration.provenance().is_trusted() {
            return Err(RegistryError::UntrustedPlugin(
                registration.provenance().plugin_id().clone(),
            ));
        }
        if registration.manifest().id() != registration.provenance().plugin_id()
            || registration.manifest().version() != registration.provenance().version()
        {
            return Err(RegistryError::ProvenanceMismatch);
        }
        registration
            .manifest()
            .contributions()
            .validate(
                registration.manifest().plugin_root(),
                registration.manifest().capabilities(),
            )
            .map_err(|_| RegistryError::InvalidContribution)?;
    }

    let mut tool_groups: BTreeMap<Identifier, Vec<RegisteredContribution<ToolDescriptor>>> =
        BTreeMap::new();
    let mut commands = BTreeMap::new();
    let mut prompts = BTreeMap::new();
    let mut resources = BTreeMap::new();
    let mut views = BTreeMap::new();
    let mut timelines = BTreeMap::new();
    let mut modals = BTreeMap::new();
    let mut widgets = BTreeMap::new();
    let mut subscriptions = BTreeMap::new();

    for registration in plugins.values() {
        let provenance = registration.provenance().clone();
        let contributions = registration.manifest().contributions();
        for descriptor in &contributions.tools {
            tool_groups
                .entry(descriptor.name.clone())
                .or_default()
                .push(registered(descriptor.clone(), &provenance));
        }
        for descriptor in &contributions.commands {
            insert_unique(
                &mut commands,
                descriptor.name.clone(),
                registered(descriptor.clone(), &provenance),
                ContributionKind::Command,
            )?;
        }
        for descriptor in &contributions.prompts {
            insert_unique(
                &mut prompts,
                descriptor.id.clone(),
                registered(descriptor.clone(), &provenance),
                ContributionKind::Prompt,
            )?;
        }
        for descriptor in &contributions.resources {
            insert_unique(
                &mut resources,
                descriptor.id.clone(),
                registered(descriptor.clone(), &provenance),
                ContributionKind::Resource,
            )?;
        }
        for descriptor in &contributions.views {
            insert_unique(
                &mut views,
                descriptor.metadata.id.clone(),
                registered(descriptor.clone(), &provenance),
                ContributionKind::View,
            )?;
        }
        for descriptor in &contributions.timelines {
            insert_unique(
                &mut timelines,
                descriptor.metadata.id.clone(),
                registered(descriptor.clone(), &provenance),
                ContributionKind::Timeline,
            )?;
        }
        for descriptor in &contributions.modals {
            insert_unique(
                &mut modals,
                descriptor.metadata.id.clone(),
                registered(descriptor.clone(), &provenance),
                ContributionKind::Modal,
            )?;
        }
        for descriptor in &contributions.widgets {
            insert_unique(
                &mut widgets,
                descriptor.metadata.id.clone(),
                registered(descriptor.clone(), &provenance),
                ContributionKind::Widget,
            )?;
        }
        for descriptor in &contributions.event_subscriptions {
            insert_unique(
                &mut subscriptions,
                descriptor.id.clone(),
                registered(descriptor.clone(), &provenance),
                ContributionKind::EventSubscription,
            )?;
        }
    }

    let mut tools = BTreeMap::new();
    let mut used_bindings = BTreeSet::new();
    for (name, candidates) in tool_groups {
        if candidates.len() == 1 {
            let Some(candidate) = candidates.into_iter().next() else {
                return Err(RegistryError::InvalidContribution);
            };
            tools.insert(name, candidate);
            continue;
        }
        let target = bindings
            .tool_binding
            .get(&name)
            .ok_or_else(|| RegistryError::Collision {
                kind: ContributionKind::Tool,
                name: name.clone(),
            })?;
        let mut matching = candidates.into_iter().filter(|candidate| {
            candidate.provenance().plugin_id() == &target.plugin_id
                && candidate.provenance().version() == target.version
        });
        let selected = matching
            .next()
            .ok_or_else(|| RegistryError::InvalidToolBinding(name.clone()))?;
        if matching.next().is_some() {
            return Err(RegistryError::InvalidToolBinding(name));
        }
        used_bindings.insert(name.clone());
        tools.insert(name, selected);
    }
    if let Some(stale) = bindings
        .tool_binding
        .keys()
        .find(|name| !used_bindings.contains(*name))
    {
        return Err(RegistryError::StaleToolBinding(stale.clone()));
    }

    Ok(SnapshotData {
        generation,
        plugins,
        tools,
        commands,
        prompts,
        resources,
        views,
        timelines,
        modals,
        widgets,
        subscriptions,
    })
}

fn registered<T: Clone>(descriptor: T, provenance: &Provenance) -> RegisteredContribution<T> {
    RegisteredContribution {
        descriptor,
        provenance: provenance.clone(),
    }
}

fn insert_unique<T>(
    map: &mut BTreeMap<Identifier, RegisteredContribution<T>>,
    name: Identifier,
    value: RegisteredContribution<T>,
    kind: ContributionKind,
) -> Result<(), RegistryError> {
    if map.insert(name.clone(), value).is_some() {
        return Err(RegistryError::Collision { kind, name });
    }
    Ok(())
}

/// Contribution namespace used in collision diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContributionKind {
    /// Tool callable name.
    Tool,
    /// Command callable name.
    Command,
    /// Prompt contribution id.
    Prompt,
    /// Resource id.
    Resource,
    /// View id.
    View,
    /// Timeline id.
    Timeline,
    /// Modal id.
    Modal,
    /// Widget id.
    Widget,
    /// Event subscription id.
    EventSubscription,
}

/// Transaction preparation, validation, or commit failure.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RegistryError {
    /// Manifest and provenance identity differed.
    #[error("plugin registration provenance does not match its manifest")]
    ProvenanceMismatch,
    /// Plugin id was already registered.
    #[error("plugin {0} is already registered")]
    PluginAlreadyRegistered(PluginId),
    /// Plugin id was not registered.
    #[error("plugin {0} is not registered")]
    PluginNotRegistered(PluginId),
    /// A transaction touched one plugin more than once.
    #[error("plugin {0} appears more than once in one registry transaction")]
    DuplicateMutation(PluginId),
    /// Untrusted project/user resources cannot activate.
    #[error("plugin {0} is untrusted and cannot activate contributions")]
    UntrustedPlugin(PluginId),
    /// A descriptor failed validation.
    #[error("plugin contribution failed validation")]
    InvalidContribution,
    /// A same-name contribution was unresolved.
    #[error("{kind:?} contribution name collision for {name}")]
    Collision {
        /// Colliding namespace.
        kind: ContributionKind,
        /// Colliding public name.
        name: Identifier,
    },
    /// Explicit tool binding did not select exactly one colliding owner.
    #[error("host toolBinding for {0} does not select a colliding owner")]
    InvalidToolBinding(Identifier),
    /// Host JSON included a binding for a non-colliding tool.
    #[error("host toolBinding for {0} is stale or unnecessary")]
    StaleToolBinding(Identifier),
    /// Transaction validation was skipped.
    #[error("registry transaction must be validated before commit")]
    NotValidated,
    /// Transaction method was called in the wrong state.
    #[error("registry transaction is not in the required state")]
    InvalidTransactionState,
    /// A different transaction committed first.
    #[error("registry generation conflict (expected {expected}, actual {actual})")]
    GenerationConflict {
        /// Generation observed during preparation.
        expected: u64,
        /// Current live generation.
        actual: u64,
    },
    /// Monotonic generation counter overflowed.
    #[error("plugin registry generation is exhausted")]
    GenerationExhausted,
}

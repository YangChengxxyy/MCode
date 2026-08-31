//! Directs authoritative fixed-set Manager generations.
//!
//! Reconciliation prepares every changed enabled family before atomically
//! publishing one fixed-12 snapshot. Candidate service gates stay closed until
//! every replacement slot is installed. Replaced generations become stale at
//! the publication boundary, then cancel, drain, receive one bounded shutdown
//! attempt, and relinquish their Store ownership. An exact all-disabled ABSENT
//! snapshot does not lower the retained positive authority high-water.

// Rust guideline compliant 2026-08-31.

use std::fmt::{self, Display, Formatter};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as SyncMutex, MutexGuard};

use mcode_config::{
    ArtifactRef, AuthorityRevision, HomeLayout, ManagerRecord, PluginFamily, SourceBindingId,
    TrustHighWater,
};
use mcode_plugin_api::{MAX_TASK_GENERATION, TaskGeneration};
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};

use crate::manager_loading::{MANAGER_SLOT_COUNT, ManagerCandidates, family_index};
use crate::pack_selection::PackSelectionAuthority;
use crate::runtime::{LifecycleState, PluginRuntime};

mod cleanup;
mod dispatch;
mod generation;
mod pack_configuration;

use cleanup::CleanupWorker;
pub use dispatch::{CurrentManagerPoll, ManagerGenerationCallError};
use generation::{ActiveGeneration, GenerationHostBindings};
#[cfg(test)]
use generation::{
    GENERATION_ACTIVITY_INCREMENT, GENERATION_CURRENT, MAX_GENERATION_ACTIVITIES,
    generation_activity_count,
};
pub(crate) use generation::{GenerationActivity, GenerationCommitError, GenerationFence};

const PUBLICATION_OPEN: u64 = 0;
#[cfg(test)]
const PUBLICATION_IN_PROGRESS: u64 = 1;
const PUBLICATION_CLOSED: u64 = u64::MAX;

/// Directs the sole current fixed-12 Manager generation set.
///
/// Call [`Self::shutdown`] to wait for graceful final quiescence. `Drop` closes
/// admission and transfers retained generations to the cleanup worker, but does
/// not wait for that work to finish.
pub struct ManagerGenerationDirector {
    runtime: Arc<PluginRuntime>,
    pack_home: HomeLayout,
    cleanup: CleanupWorker,
    reconciliation: Arc<AsyncMutex<()>>,
    publication_state: Arc<AtomicU64>,
    pack_selections: Arc<PackSelectionAuthority>,
    identity: DirectorIdentity,
    state: Arc<SyncMutex<DirectorState>>,
}

struct DirectorState {
    closed: bool,
    current_epoch: u64,
    revision: AuthorityRevision,
    authority_revision: AuthorityRevision,
    authority_target: [ManagerRecord; MANAGER_SLOT_COUNT],
    current: [Option<Arc<ActiveGeneration>>; MANAGER_SLOT_COUNT],
    high_water: [u64; MANAGER_SLOT_COUNT],
    preparation: Option<PreparedSet>,
}

/// Describes one current Manager generation without exposing runtime ownership.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CurrentManagerGeneration {
    identity: DirectorIdentity,
    family: PluginFamily,
    record: ManagerRecord,
    revision: AuthorityRevision,
    generation: TaskGeneration,
}

#[derive(Clone)]
struct DirectorIdentity(Arc<()>);

impl fmt::Debug for DirectorIdentity {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str("DirectorIdentity")
    }
}

impl PartialEq for DirectorIdentity {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for DirectorIdentity {}

impl CurrentManagerGeneration {
    /// Returns the frozen family of this current generation.
    #[must_use]
    pub const fn family(&self) -> PluginFamily {
        self.family
    }

    /// Returns the exact artifact identity of this current generation.
    #[must_use]
    pub fn artifact(&self) -> &ArtifactRef {
        self.record
            .active()
            .expect("a current Manager generation has an active artifact")
    }

    /// Returns the exact source binding of this current generation.
    #[must_use]
    pub fn source(&self) -> &SourceBindingId {
        self.record
            .source()
            .expect("a current Manager generation has a source binding")
    }

    /// Returns the signed trust high-water bound to this generation.
    #[must_use]
    pub fn trust_high_water(&self) -> &TrustHighWater {
        self.record
            .trust_high_water()
            .expect("a current Manager generation has a trust high-water")
    }

    /// Returns the authority revision observed when this view was selected.
    ///
    /// Revision is not generation identity. A view remains a valid expected
    /// tag across a revision-only advance while its exact generation stays
    /// current; a poll result is stamped with the newer selection revision.
    #[must_use]
    pub const fn revision(&self) -> AuthorityRevision {
        self.revision
    }

    /// Returns the strictly increasing family generation.
    #[must_use]
    pub const fn generation(&self) -> TaskGeneration {
        self.generation
    }
}

/// Holds one immutable fixed-12 current-generation snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManagerGenerationSnapshot {
    revision: AuthorityRevision,
    slots: [Option<CurrentManagerGeneration>; MANAGER_SLOT_COUNT],
}

impl ManagerGenerationSnapshot {
    /// Returns the authority revision shared by every snapshot slot.
    #[must_use]
    pub const fn revision(&self) -> AuthorityRevision {
        self.revision
    }

    /// Returns the current generation for `family`, when enabled.
    #[must_use]
    pub fn current(&self, family: PluginFamily) -> Option<&CurrentManagerGeneration> {
        self.slots[family_index(family)].as_ref()
    }
}

/// Reports caller-driven preparation progress for one complete candidate set.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreparationProgress {
    revision: AuthorityRevision,
    pending: [bool; MANAGER_SLOT_COUNT],
}

impl PreparationProgress {
    /// Returns the candidate-set authority revision being prepared.
    #[must_use]
    pub const fn revision(&self) -> AuthorityRevision {
        self.revision
    }

    /// Returns whether `family` still requires an explicit lifecycle poll.
    #[must_use]
    pub const fn is_pending(&self, family: PluginFamily) -> bool {
        self.pending[family_index(family)]
    }

    /// Returns the number of families still pending.
    #[must_use]
    pub fn pending_count(&self) -> usize {
        self.pending.iter().filter(|pending| **pending).count()
    }
}

/// Reports one authoritative reconciliation result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReconciliationOutcome {
    /// The supplied revision and exact set were already current.
    NoChange {
        /// The unchanged authority revision.
        revision: AuthorityRevision,
    },
    /// The complete supplied snapshot was published.
    Published {
        /// The newly current authority revision.
        revision: AuthorityRevision,
    },
    /// At least one initialized candidate needs an explicit lifecycle poll.
    PreparationPending(PreparationProgress),
}

impl ReconciliationOutcome {
    /// Returns the authority revision represented by this outcome.
    #[must_use]
    pub const fn revision(&self) -> AuthorityRevision {
        match self {
            Self::NoChange { revision } | Self::Published { revision } => *revision,
            Self::PreparationPending(progress) => progress.revision(),
        }
    }
}

/// Reports one stable, non-sensitive reconciliation failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReconciliationError {
    /// The runtime is already bound to its sole generation director.
    RuntimeAlreadyDirected,
    /// The director has begun its final shutdown.
    Closed,
    /// The supplied authority revision was older than accepted state.
    RevisionRegression,
    /// The supplied same-revision set differed from accepted state.
    RevisionConflict,
    /// Director synchronization or cleanup execution is unavailable.
    Unavailable,
    /// No retained pending preparation exists to poll.
    NoPreparation,
    /// This family's generation range is exhausted.
    GenerationExhausted(PluginFamily),
    /// Current topology changed before the complete prepared set published.
    PreparationInvalidated,
    /// Runtime preparation failed for this family.
    Runtime(PluginFamily),
    /// The guest rejected lifecycle preparation for this family.
    LifecycleRejected(PluginFamily),
    /// The guest entered a terminal lifecycle state during preparation.
    LifecycleTerminal(PluginFamily),
}

impl Display for ReconciliationError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::RuntimeAlreadyDirected => {
                formatter.write_str("plugin runtime already has a Manager generation director")
            }
            Self::Closed => formatter.write_str("Manager generation director is closed"),
            Self::RevisionRegression => formatter.write_str("Manager authority revision regressed"),
            Self::RevisionConflict => {
                formatter.write_str("Manager authority revision conflicts with its exact set")
            }
            Self::Unavailable => formatter.write_str("Manager generation director is unavailable"),
            Self::NoPreparation => formatter.write_str("no Manager preparation is pending"),
            Self::GenerationExhausted(family) => write!(
                formatter,
                "Manager generation is exhausted for {}",
                family.directory_name()
            ),
            Self::PreparationInvalidated => {
                formatter.write_str("Manager preparation was invalidated by current retirement")
            }
            Self::Runtime(family) => write!(
                formatter,
                "Manager runtime preparation failed for {}",
                family.directory_name()
            ),
            Self::LifecycleRejected(family) => write!(
                formatter,
                "Manager lifecycle preparation was rejected for {}",
                family.directory_name()
            ),
            Self::LifecycleTerminal(family) => write!(
                formatter,
                "Manager lifecycle became terminal for {}",
                family.directory_name()
            ),
        }
    }
}

impl std::error::Error for ReconciliationError {}

impl ManagerGenerationDirector {
    /// Returns the sole runtime already claimed by this director.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "T8.3 consumes the generation-bound Pack candidate boundary"
        )
    )]
    pub(crate) fn runtime(&self) -> &PluginRuntime {
        &self.runtime
    }

    pub(crate) const fn pack_home(&self) -> &HomeLayout {
        &self.pack_home
    }

    /// Creates an all-disabled director over one shared runtime and Pack home.
    ///
    /// # Errors
    ///
    /// Returns [`ReconciliationError::RuntimeAlreadyDirected`] when another
    /// director has already claimed `runtime`, or
    /// [`ReconciliationError::Unavailable`] when its cleanup worker cannot
    /// start.
    pub fn new(
        runtime: Arc<PluginRuntime>,
        pack_home: HomeLayout,
    ) -> Result<Self, ReconciliationError> {
        let cleanup = CleanupWorker::start()?;
        if !runtime.claim_manager_director() {
            return Err(ReconciliationError::RuntimeAlreadyDirected);
        }
        Ok(Self {
            runtime,
            pack_home,
            cleanup,
            reconciliation: Arc::new(AsyncMutex::new(())),
            publication_state: Arc::new(AtomicU64::new(PUBLICATION_OPEN)),
            pack_selections: PackSelectionAuthority::new(),
            identity: DirectorIdentity(Arc::new(())),
            state: Arc::new(SyncMutex::new(DirectorState {
                closed: false,
                current_epoch: 0,
                revision: AuthorityRevision::ABSENT,
                authority_revision: AuthorityRevision::ABSENT,
                authority_target: std::array::from_fn(|_| ManagerRecord::absent()),
                current: std::array::from_fn(|_| None),
                high_water: [0; MANAGER_SLOT_COUNT],
                preparation: None,
            })),
        })
    }

    /// Returns one immutable, atomically captured current snapshot.
    ///
    /// # Errors
    ///
    /// Returns [`ReconciliationError::Closed`] after shutdown begins, or
    /// [`ReconciliationError::Unavailable`] when director state is unavailable
    /// after synchronization failure.
    pub fn snapshot(&self) -> Result<ManagerGenerationSnapshot, ReconciliationError> {
        let state = self.lock_state()?;
        ensure_open(&state)?;
        Ok(ManagerGenerationSnapshot {
            revision: state.revision,
            slots: std::array::from_fn(|index| {
                state.current[index]
                    .as_ref()
                    .map(|entry| entry.view(state.revision))
            }),
        })
    }

    /// Returns the current generation for `family`, when enabled.
    ///
    /// # Errors
    ///
    /// Returns [`ReconciliationError::Closed`] after shutdown begins, or
    /// [`ReconciliationError::Unavailable`] when director state is unavailable
    /// after synchronization failure.
    pub fn current(
        &self,
        family: PluginFamily,
    ) -> Result<Option<CurrentManagerGeneration>, ReconciliationError> {
        let state = self.lock_state()?;
        ensure_open(&state)?;
        Ok(state.current[family_index(family)]
            .as_ref()
            .map(|entry| entry.view(state.revision)))
    }

    /// Reconciles one exact candidate set against current authority.
    ///
    /// Changed enabled families are instantiated and initialized before any
    /// slot is published. A pending initialization is retained without hidden
    /// polling; call [`Self::poll_preparation`] to advance it once.
    ///
    /// # Errors
    ///
    /// Returns [`ReconciliationError`] for revision regression or conflict,
    /// generation exhaustion, runtime failure, lifecycle rejection, or a
    /// terminal preparation state. Any preparation error leaves the complete
    /// current snapshot unchanged.
    pub async fn reconcile(
        &self,
        mut candidates: ManagerCandidates,
    ) -> Result<ReconciliationOutcome, ReconciliationError> {
        let serialized = Arc::clone(&self.reconciliation).lock_owned().await;
        let target_revision = candidates.revision();
        let target = candidate_authority(&candidates);

        match self.resolve_pending_input(target_revision, &target)? {
            PendingInput::None => {}
            PendingInput::Retained(outcome) => return Ok(outcome),
            PendingInput::Superseded(prepared) => retire_prepared(*prepared).await,
        }

        let (changed, base_current_epoch) = {
            let mut state = self.lock_state()?;
            validate_revision(&state, target_revision, &target)?;
            if target_revision > state.authority_revision {
                state.authority_revision = target_revision;
                state.authority_target = target.clone();
            }
            let changed = changed_slots(&state.current, &target);
            if !changed.iter().any(|value| *value) {
                if target_revision == state.revision {
                    return Ok(ReconciliationOutcome::NoChange {
                        revision: state.revision,
                    });
                }
                state.revision = target_revision;
                return Ok(ReconciliationOutcome::Published {
                    revision: target_revision,
                });
            }
            let reserved = reserve_generations(&mut state, &changed, &target)?;
            (reserved, state.current_epoch)
        };
        let mut prepared =
            PreparedSet::new(target_revision, target, changed.changed, base_current_epoch);
        for family in PluginFamily::ALL {
            let index = family_index(family);
            let Some(generation) = changed.generations[index] else {
                continue;
            };
            let candidate = candidates
                .take(family)
                .expect("each changed enabled target retains its Manager candidate");
            let prepared_generation = ActiveGeneration::prepare(
                &self.runtime,
                GenerationHostBindings::new(
                    Arc::clone(&self.publication_state),
                    Arc::clone(&self.pack_selections),
                    self.pack_home.clone(),
                ),
                self.identity.clone(),
                family,
                prepared.target[index].clone(),
                generation,
                candidate.into_component(),
            )
            .await;
            let (entry, pending) = match prepared_generation {
                Ok(generation) => generation,
                Err(error) => {
                    retire_prepared(prepared).await;
                    return Err(error);
                }
            };
            prepared.slots[index] = Some(entry);
            prepared.pending[index] = pending;
        }

        if prepared.has_pending() {
            let progress = prepared.progress();
            if let Err((error, prepared)) = self.retain_preparation(prepared) {
                retire_prepared(*prepared).await;
                return Err(error);
            }
            return Ok(ReconciliationOutcome::PreparationPending(progress));
        }
        self.publish(prepared, serialized).await
    }

    /// Polls each retained pending candidate once, then publishes if all ready.
    ///
    /// This method never loops on a guest `Pending` result.
    ///
    /// # Errors
    ///
    /// Returns [`ReconciliationError::Closed`] after shutdown begins or
    /// [`ReconciliationError::NoPreparation`] when no pending set is retained.
    /// Runtime failure, lifecycle rejection, or a terminal state aborts the
    /// complete preparation and leaves current state unchanged.
    pub async fn poll_preparation(&self) -> Result<ReconciliationOutcome, ReconciliationError> {
        let serialized = Arc::clone(&self.reconciliation).lock_owned().await;
        let mut prepared = {
            let mut state = self.lock_state()?;
            ensure_open(&state)?;
            state
                .preparation
                .take()
                .ok_or(ReconciliationError::NoPreparation)?
        };

        for family in PluginFamily::ALL {
            let index = family_index(family);
            let Some(entry) = prepared.slots[index].as_ref() else {
                continue;
            };
            if !prepared.pending[index] {
                continue;
            }
            prepared.pending[index] = match entry.poll_preparation().await {
                Ok(LifecycleState::Ready) => false,
                Ok(LifecycleState::Pending) => true,
                Ok(LifecycleState::Stopping | LifecycleState::Stopped) => {
                    retire_prepared(prepared).await;
                    return Err(ReconciliationError::LifecycleTerminal(family));
                }
                Err(error) => {
                    retire_prepared(prepared).await;
                    return Err(error);
                }
            };
        }

        if prepared.has_pending() {
            let progress = prepared.progress();
            if let Err((error, prepared)) = self.retain_preparation(prepared) {
                retire_prepared(*prepared).await;
                return Err(error);
            }
            return Ok(ReconciliationOutcome::PreparationPending(progress));
        }
        self.publish(prepared, serialized).await
    }

    /// Closes the director and retires every retained generation.
    ///
    /// New snapshots, reconciliations, and lifecycle polls fail closed as soon
    /// as shutdown begins. The future completes after current and preparing
    /// Stores have quiesced and received one bounded shutdown attempt.
    ///
    /// # Errors
    ///
    /// Returns [`ReconciliationError::Unavailable`] when director state is
    /// unavailable after synchronization failure.
    pub async fn shutdown(&self) -> Result<(), ReconciliationError> {
        let serialized = Arc::clone(&self.reconciliation).lock_owned().await;
        self.pack_selections.close();
        if self.lock_state()?.closed {
            return Ok(());
        }
        let retired = {
            let mut state = self.lock_state()?;
            self.publication_state
                .store(PUBLICATION_CLOSED, Ordering::SeqCst);
            state.closed = true;
            take_retained_generations(&mut state)
        };
        signal_cancellation(&retired);
        if retired.is_empty() {
            drop(serialized);
            self.cleanup.stop();
            return Ok(());
        }
        self.cleanup
            .retire_for_shutdown(retired, serialized)?
            .await
            .map_err(|_| ReconciliationError::Unavailable)
    }

    fn resolve_pending_input(
        &self,
        revision: AuthorityRevision,
        target: &[ManagerRecord; MANAGER_SLOT_COUNT],
    ) -> Result<PendingInput, ReconciliationError> {
        let mut state = self.lock_state()?;
        validate_revision(&state, revision, target)?;
        let Some(prepared) = state.preparation.as_mut() else {
            return Ok(PendingInput::None);
        };
        if *target == prepared.target {
            prepared.revision = revision;
            let progress = prepared.progress();
            if revision > state.authority_revision {
                state.authority_revision = revision;
                state.authority_target = target.clone();
            }
            return Ok(PendingInput::Retained(
                ReconciliationOutcome::PreparationPending(progress),
            ));
        }
        Ok(PendingInput::Superseded(Box::new(
            state
                .preparation
                .take()
                .expect("the checked preparation remains retained"),
        )))
    }

    async fn publish(
        &self,
        mut prepared: PreparedSet,
        serialized: OwnedMutexGuard<()>,
    ) -> Result<ReconciliationOutcome, ReconciliationError> {
        let retired = match self.commit_prepared(&mut prepared) {
            Ok(retired) => retired,
            Err(error) => {
                retire_prepared(prepared).await;
                return Err(error);
            }
        };
        signal_cancellation(&retired);

        if retired.is_empty() {
            drop(serialized);
        } else {
            self.cleanup.retire_after_publication(retired, serialized)?;
        }
        Ok(ReconciliationOutcome::Published {
            revision: prepared.revision,
        })
    }

    fn commit_prepared(
        &self,
        prepared: &mut PreparedSet,
    ) -> Result<Vec<Arc<ActiveGeneration>>, ReconciliationError> {
        let mut state = self.lock_state()?;
        ensure_open(&state)?;
        if prepared.base_current_epoch != state.current_epoch {
            return Err(ReconciliationError::PreparationInvalidated);
        }
        let open_epoch = self.publication_state.load(Ordering::SeqCst);
        if open_epoch == PUBLICATION_CLOSED || !open_epoch.is_multiple_of(2) {
            return Err(ReconciliationError::Unavailable);
        }
        let publication_epoch = open_epoch
            .checked_add(1)
            .filter(|epoch| *epoch != PUBLICATION_CLOSED)
            .ok_or(ReconciliationError::Unavailable)?;
        self.publication_state
            .compare_exchange(
                open_epoch,
                publication_epoch,
                Ordering::SeqCst,
                Ordering::SeqCst,
            )
            .map_err(|_| ReconciliationError::Unavailable)?;
        let mut retired = Vec::with_capacity(MANAGER_SLOT_COUNT);
        for family in PluginFamily::ALL {
            let index = family_index(family);
            if !prepared.changed[index] {
                continue;
            }
            let old = std::mem::replace(&mut state.current[index], prepared.slots[index].take());
            if let Some(old) = old {
                retired.push(old);
            }
        }
        for family in PluginFamily::ALL {
            let index = family_index(family);
            if prepared.changed[index]
                && let Some(entry) = state.current[index].as_ref()
            {
                entry.fence.mark_current();
            }
        }
        state.revision = prepared.revision;
        state.current_epoch = state
            .current_epoch
            .checked_add(1)
            .expect("the current topology epoch cannot exhaust");
        for entry in &retired {
            entry.fence.mark_retired();
        }
        let next_open_epoch = publication_epoch
            .checked_add(1)
            .expect("the publication epoch cannot exhaust");
        self.publication_state
            .compare_exchange(
                publication_epoch,
                next_open_epoch,
                Ordering::SeqCst,
                Ordering::SeqCst,
            )
            .expect("one serialized publication owns the admission gate");
        Ok(retired)
    }

    fn retain_preparation(
        &self,
        prepared: PreparedSet,
    ) -> Result<(), (ReconciliationError, Box<PreparedSet>)> {
        let mut state = match self.lock_state() {
            Ok(state) => state,
            Err(error) => return Err((error, Box::new(prepared))),
        };
        if let Err(error) = ensure_open(&state) {
            return Err((error, Box::new(prepared)));
        }
        if prepared.base_current_epoch != state.current_epoch {
            return Err((
                ReconciliationError::PreparationInvalidated,
                Box::new(prepared),
            ));
        }
        state.preparation = Some(prepared);
        Ok(())
    }

    fn lock_state(&self) -> Result<MutexGuard<'_, DirectorState>, ReconciliationError> {
        match self.state.lock() {
            Ok(state) => Ok(state),
            Err(error) => {
                self.publication_state
                    .store(PUBLICATION_CLOSED, Ordering::SeqCst);
                let mut state = error.into_inner();
                state.closed = true;
                let retired = take_retained_generations(&mut state);
                signal_cancellation(&retired);
                if !retired.is_empty() {
                    let _ = self.cleanup.retire_after_cancelled_call(retired);
                }
                Err(ReconciliationError::Unavailable)
            }
        }
    }

    #[cfg(test)]
    fn current_entry(
        &self,
        family: PluginFamily,
    ) -> Result<Option<Arc<ActiveGeneration>>, ReconciliationError> {
        let state = self.lock_state()?;
        ensure_open(&state)?;
        Ok(state.current[family_index(family)].clone())
    }
}

impl Drop for ManagerGenerationDirector {
    fn drop(&mut self) {
        self.pack_selections.close();
        self.publication_state
            .store(PUBLICATION_CLOSED, Ordering::SeqCst);
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.closed = true;
        let retired = take_retained_generations(&mut state);
        drop(state);
        for entry in &retired {
            entry.fence.signal_cancellation();
        }
        if retired.is_empty() {
            self.cleanup.stop();
        } else {
            self.cleanup.retire_for_drop(retired);
        }
    }
}

async fn retire_generation_entries(retired: Vec<Arc<ActiveGeneration>>) {
    signal_cancellation(&retired);
    for entry in retired {
        entry.retire().await;
    }
}

fn signal_cancellation(retired: &[Arc<ActiveGeneration>]) {
    for entry in retired {
        entry.fence.signal_cancellation();
    }
}

async fn retire_prepared(mut prepared: PreparedSet) {
    let retired = prepared
        .slots
        .iter_mut()
        .filter_map(Option::take)
        .collect::<Vec<_>>();
    for entry in &retired {
        entry.fence.mark_retired();
    }
    retire_generation_entries(retired).await;
}

fn candidate_authority(candidates: &ManagerCandidates) -> [ManagerRecord; MANAGER_SLOT_COUNT] {
    PluginFamily::ALL.map(|family| candidates.authority_record(family).clone())
}

fn validate_revision(
    state: &DirectorState,
    revision: AuthorityRevision,
    target: &[ManagerRecord; MANAGER_SLOT_COUNT],
) -> Result<(), ReconciliationError> {
    ensure_open(state)?;
    if revision == AuthorityRevision::ABSENT {
        return if target
            .iter()
            .all(|record| *record == ManagerRecord::absent())
        {
            Ok(())
        } else {
            Err(ReconciliationError::RevisionConflict)
        };
    }
    if revision < state.authority_revision {
        return Err(ReconciliationError::RevisionRegression);
    }
    if revision == state.authority_revision && *target != state.authority_target {
        return Err(ReconciliationError::RevisionConflict);
    }
    Ok(())
}

fn changed_slots(
    current: &[Option<Arc<ActiveGeneration>>; MANAGER_SLOT_COUNT],
    target: &[ManagerRecord; MANAGER_SLOT_COUNT],
) -> [bool; MANAGER_SLOT_COUNT] {
    std::array::from_fn(|index| match &current[index] {
        Some(entry) => !target[index].enabled() || entry.record != target[index],
        None => target[index].enabled(),
    })
}

fn ensure_open(state: &DirectorState) -> Result<(), ReconciliationError> {
    if state.closed {
        Err(ReconciliationError::Closed)
    } else {
        Ok(())
    }
}

fn take_retained_generations(state: &mut DirectorState) -> Vec<Arc<ActiveGeneration>> {
    let mut retained = Vec::with_capacity(MANAGER_SLOT_COUNT * 2);
    retained.extend(state.current.iter_mut().filter_map(Option::take));
    if let Some(mut prepared) = state.preparation.take() {
        retained.extend(prepared.slots.iter_mut().filter_map(Option::take));
    }
    for entry in &retained {
        entry.fence.mark_retired();
    }
    retained
}

struct ReservedGenerations {
    changed: [bool; MANAGER_SLOT_COUNT],
    generations: [Option<TaskGeneration>; MANAGER_SLOT_COUNT],
}

fn reserve_generations(
    state: &mut DirectorState,
    changed: &[bool; MANAGER_SLOT_COUNT],
    target: &[ManagerRecord; MANAGER_SLOT_COUNT],
) -> Result<ReservedGenerations, ReconciliationError> {
    let mut generations = [None; MANAGER_SLOT_COUNT];
    for family in PluginFamily::ALL {
        let index = family_index(family);
        if !changed[index] || !target[index].enabled() {
            continue;
        }
        let next = state.high_water[index]
            .checked_add(1)
            .filter(|value| *value <= MAX_TASK_GENERATION)
            .ok_or(ReconciliationError::GenerationExhausted(family))?;
        generations[index] = Some(
            TaskGeneration::new(next)
                .expect("a checked nonzero generation within the frozen maximum is valid"),
        );
    }
    for (index, generation) in generations.iter().enumerate() {
        if let Some(generation) = generation {
            state.high_water[index] = generation.get();
        }
    }
    Ok(ReservedGenerations {
        changed: *changed,
        generations,
    })
}

struct PreparedSet {
    base_current_epoch: u64,
    revision: AuthorityRevision,
    target: [ManagerRecord; MANAGER_SLOT_COUNT],
    changed: [bool; MANAGER_SLOT_COUNT],
    slots: [Option<Arc<ActiveGeneration>>; MANAGER_SLOT_COUNT],
    pending: [bool; MANAGER_SLOT_COUNT],
}

enum PendingInput {
    None,
    Retained(ReconciliationOutcome),
    Superseded(Box<PreparedSet>),
}

impl PreparedSet {
    fn new(
        revision: AuthorityRevision,
        target: [ManagerRecord; MANAGER_SLOT_COUNT],
        changed: [bool; MANAGER_SLOT_COUNT],
        base_current_epoch: u64,
    ) -> Self {
        Self {
            base_current_epoch,
            revision,
            target,
            changed,
            slots: std::array::from_fn(|_| None),
            pending: [false; MANAGER_SLOT_COUNT],
        }
    }

    fn has_pending(&self) -> bool {
        self.pending.iter().any(|pending| *pending)
    }

    fn progress(&self) -> PreparationProgress {
        PreparationProgress {
            revision: self.revision,
            pending: self.pending,
        }
    }
}

#[cfg(test)]
#[path = "manager_director_audit_tests.rs"]
mod audit_tests;
#[cfg(test)]
#[path = "manager_director_dispatch_tests.rs"]
mod dispatch_tests;
#[cfg(test)]
#[path = "manager_director_test_support.rs"]
pub(crate) mod test_support;
#[cfg(test)]
#[path = "manager_director_tests.rs"]
mod tests;

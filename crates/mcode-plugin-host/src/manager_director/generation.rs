//! Owns one Manager generation's lifecycle, admission fence, and Store.
//!
//! Preparation cannot admit Host work. Publication and retirement use one
//! atomic phase-plus-count state so retirement linearizes with every activity
//! reservation before quiescent Store cleanup.

// Rust guideline compliant 2026-08-31.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as SyncMutex, MutexGuard};

use mcode_config::{AuthorityRevision, HomeLayout, ManagerRecord, PluginFamily};
use mcode_plugin_api::{MAX_MANAGER_TASK_WIRE_BYTES, TaskGeneration};
use tokio::sync::{Mutex as AsyncMutex, Notify};

use super::dispatch::CurrentGenerationCall;
use super::{CurrentManagerGeneration, DirectorIdentity, PUBLICATION_CLOSED, ReconciliationError};
use crate::pack_activation::PackActivationClient;
use crate::pack_selection::PackSelectionAuthority;
use crate::runtime::{
    LifecycleErrorCode, LifecycleOutcome, LifecycleState, ManagerInstance, ManagerTaskCall,
    ManagerTaskCallError, PluginOwner, PluginRuntime,
};

pub(super) struct GenerationOwner {
    owner: PluginOwner,
    instance: ManagerInstance,
}

pub(super) struct GenerationHostBindings {
    publication_state: Arc<AtomicU64>,
    pack_selections: Arc<PackSelectionAuthority>,
    pack_home: HomeLayout,
}

impl GenerationHostBindings {
    pub(super) const fn new(
        publication_state: Arc<AtomicU64>,
        pack_selections: Arc<PackSelectionAuthority>,
        pack_home: HomeLayout,
    ) -> Self {
        Self {
            publication_state,
            pack_selections,
            pack_home,
        }
    }
}

pub(super) struct ActiveGeneration {
    identity: DirectorIdentity,
    pub(super) family: PluginFamily,
    pub(super) record: ManagerRecord,
    generation: TaskGeneration,
    pub(super) fence: Arc<GenerationFence>,
    pub(super) owner: AsyncMutex<Option<GenerationOwner>>,
    #[cfg(test)]
    pub(super) shutdown_observation: SyncMutex<Option<LifecycleOutcome>>,
}

impl ActiveGeneration {
    pub(super) async fn prepare(
        runtime: &Arc<PluginRuntime>,
        host_bindings: GenerationHostBindings,
        identity: DirectorIdentity,
        family: PluginFamily,
        record: ManagerRecord,
        generation: TaskGeneration,
        component: crate::runtime::CompiledManagerComponent,
    ) -> Result<(Arc<Self>, bool), ReconciliationError> {
        let fence = Arc::new(GenerationFence::new(host_bindings.publication_state));
        let pack_selection = host_bindings.pack_selections.client(family);
        let pack_activation = PackActivationClient::new(
            Arc::clone(runtime),
            host_bindings.pack_home,
            family,
            pack_selection,
        );
        let mut owner = runtime
            .new_owner()
            .map_err(|_| ReconciliationError::Runtime(family))?;
        owner
            .bind_generation_context(Arc::clone(&fence), pack_activation)
            .map_err(|_| ReconciliationError::Runtime(family))?;
        let instance = owner
            .instantiate_manager(&component)
            .await
            .map_err(|_| ReconciliationError::Runtime(family))?;
        let mut operation = owner
            .open_operation()
            .map_err(|_| ReconciliationError::Runtime(family))?;
        let preparation = match instance
            .initialize(&mut owner, &mut operation, generation.get())
            .await
        {
            Ok(outcome) => preparation_state(family, outcome),
            Err(_) => Err(ReconciliationError::Runtime(family)),
        };
        drop(operation);
        let state = match preparation {
            Ok(state) => state,
            Err(error) => {
                fence.mark_retired();
                fence.signal_cancellation();
                if owner.is_available() {
                    // Shutdown is best effort, but always receives a fresh bounded lease.
                    if let Ok(mut shutdown_operation) = owner.open_operation() {
                        let _ = instance.shutdown(&mut owner, &mut shutdown_operation).await;
                    }
                }
                return Err(error);
            }
        };
        let pending = state == LifecycleState::Pending;
        Ok((
            Arc::new(Self {
                identity,
                family,
                record,
                generation,
                fence,
                owner: AsyncMutex::new(Some(GenerationOwner { owner, instance })),
                #[cfg(test)]
                shutdown_observation: SyncMutex::new(None),
            }),
            pending,
        ))
    }

    pub(super) fn view(&self, revision: AuthorityRevision) -> CurrentManagerGeneration {
        CurrentManagerGeneration {
            identity: self.identity.clone(),
            family: self.family,
            record: self.record.clone(),
            revision,
            generation: self.generation,
        }
    }

    pub(super) async fn poll_preparation(&self) -> Result<LifecycleState, ReconciliationError> {
        let mut guard = self.owner.lock().await;
        let generation = guard
            .as_mut()
            .ok_or(ReconciliationError::Runtime(self.family))?;
        let GenerationOwner { owner, instance } = generation;
        let mut operation = owner
            .open_operation()
            .map_err(|_| ReconciliationError::Runtime(self.family))?;
        let outcome = instance
            .poll(owner, &mut operation)
            .await
            .map_err(|_| ReconciliationError::Runtime(self.family))?;
        preparation_state(self.family, outcome)
    }

    pub(super) async fn poll_current(
        &self,
        call: &mut CurrentGenerationCall,
    ) -> Result<LifecycleOutcome, GenerationCallError> {
        let _activity = call.take_activity();
        let cancellation = self.fence.cancelled();
        tokio::pin!(cancellation);
        let mut guard = tokio::select! {
            biased;
            () = &mut cancellation => return Err(GenerationCallError::Cancelled),
            guard = self.owner.lock() => guard,
        };
        call.arm_retirement();
        let generation = guard.as_mut().ok_or(GenerationCallError::Unavailable)?;
        let GenerationOwner { owner, instance } = generation;
        let mut operation = owner
            .open_operation()
            .map_err(|_| GenerationCallError::Unavailable)?;
        let guest_call = instance.poll(owner, &mut operation);
        tokio::pin!(guest_call);
        let outcome = tokio::select! {
            biased;
            () = &mut cancellation => return Err(GenerationCallError::Cancelled),
            outcome = &mut guest_call => outcome.map_err(|_| GenerationCallError::Runtime)?,
        };
        if matches!(outcome, Ok(LifecycleState::Ready | LifecycleState::Pending)) {
            call.keep_current();
        }
        Ok(outcome)
    }

    pub(super) async fn call_task(
        &self,
        call: &mut CurrentGenerationCall,
        task_call: ManagerTaskCall,
        request: &str,
    ) -> Result<String, GenerationCallError> {
        let _activity = call.take_activity();
        if request.len() > MAX_MANAGER_TASK_WIRE_BYTES {
            return Err(GenerationCallError::InvalidInput);
        }
        let cancellation = self.fence.cancelled();
        tokio::pin!(cancellation);
        let mut guard = tokio::select! {
            biased;
            () = &mut cancellation => return Err(GenerationCallError::Cancelled),
            guard = self.owner.lock() => guard,
        };
        call.arm_retirement();
        let generation = guard.as_mut().ok_or(GenerationCallError::Unavailable)?;
        let GenerationOwner { owner, instance } = generation;
        let mut operation = owner
            .open_operation()
            .map_err(|_| GenerationCallError::Unavailable)?;
        let guest_call = instance.call_task(owner, &mut operation, task_call, request);
        tokio::pin!(guest_call);
        let result = tokio::select! {
            biased;
            () = &mut cancellation => return Err(GenerationCallError::Cancelled),
            result = &mut guest_call => result,
        };
        match result {
            Ok(response) => {
                call.keep_current();
                Ok(response)
            }
            Err(ManagerTaskCallError::InputTooLarge) => {
                call.keep_current();
                Err(GenerationCallError::InvalidInput)
            }
            Err(ManagerTaskCallError::Runtime) => Err(GenerationCallError::Runtime),
        }
    }

    pub(super) async fn retire(&self) {
        self.fence.drain().await;
        let mut guard = self.owner.lock().await;
        let Some(mut generation) = guard.take() else {
            return;
        };
        if generation.owner.is_available() {
            let GenerationOwner { owner, instance } = &mut generation;
            let outcome = match owner.open_operation() {
                Ok(mut operation) => instance.shutdown(owner, &mut operation).await,
                Err(error) => Err(error),
            };
            #[cfg(test)]
            if let Ok(outcome) = outcome
                && let Ok(mut observation) = self.shutdown_observation.lock()
            {
                *observation = Some(outcome);
            }
            #[cfg(not(test))]
            let _ = outcome;
        }
    }
}

fn preparation_state(
    family: PluginFamily,
    outcome: LifecycleOutcome,
) -> Result<LifecycleState, ReconciliationError> {
    match outcome {
        Ok(LifecycleState::Ready) => Ok(LifecycleState::Ready),
        Ok(LifecycleState::Pending) => Ok(LifecycleState::Pending),
        Ok(LifecycleState::Stopping | LifecycleState::Stopped) => {
            Err(ReconciliationError::LifecycleTerminal(family))
        }
        Err(
            LifecycleErrorCode::InvalidState
            | LifecycleErrorCode::FeatureUnavailable
            | LifecycleErrorCode::Failed,
        ) => Err(ReconciliationError::LifecycleRejected(family)),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum GenerationCallError {
    Cancelled,
    InvalidInput,
    Unavailable,
    Runtime,
}

pub(crate) struct GenerationFence {
    publication_state: Arc<AtomicU64>,
    pub(super) state: AtomicUsize,
    cancellation: Notify,
    drained: Notify,
    commit: SyncMutex<()>,
    #[cfg(test)]
    admission_attempts: AtomicUsize,
}

// The low two bits carry the phase so retirement and activity admission share
// one CAS linearization point. All remaining bits carry the activity count.
pub(super) const GENERATION_PHASE_MASK: usize = 0b11;
const GENERATION_PREPARING: usize = 0;
pub(super) const GENERATION_CURRENT: usize = 1;
const GENERATION_RETIRED: usize = 2;
pub(super) const GENERATION_ACTIVITY_INCREMENT: usize = GENERATION_PHASE_MASK + 1;
pub(super) const MAX_GENERATION_ACTIVITIES: usize = usize::MAX >> 2;

impl GenerationFence {
    pub(crate) fn new(publication_state: Arc<AtomicU64>) -> Self {
        Self {
            publication_state,
            state: AtomicUsize::new(GENERATION_PREPARING),
            cancellation: Notify::new(),
            drained: Notify::new(),
            commit: SyncMutex::new(()),
            #[cfg(test)]
            admission_attempts: AtomicUsize::new(0),
        }
    }

    pub(crate) fn enter(self: &Arc<Self>) -> Option<GenerationActivity> {
        self.enter_after_gate_load(|| {})
    }

    #[cfg(test)]
    pub(super) fn enter_after_gate_load_for_test(
        self: &Arc<Self>,
        after_gate_load: impl FnOnce(),
    ) -> Option<GenerationActivity> {
        self.enter_after_gate_load(after_gate_load)
    }

    fn enter_after_gate_load(
        self: &Arc<Self>,
        after_gate_load: impl FnOnce(),
    ) -> Option<GenerationActivity> {
        #[cfg(test)]
        self.admission_attempts.fetch_add(1, Ordering::Relaxed);
        let publication_epoch = self.publication_state.load(Ordering::SeqCst);
        if publication_epoch == PUBLICATION_CLOSED || !publication_epoch.is_multiple_of(2) {
            return None;
        }
        after_gate_load();
        let mut state = self.state.load(Ordering::Acquire);
        loop {
            if generation_phase(state) != GENERATION_CURRENT
                || generation_activity_count(state) == MAX_GENERATION_ACTIVITIES
            {
                return None;
            }
            match self.state.compare_exchange_weak(
                state,
                state + GENERATION_ACTIVITY_INCREMENT,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => break,
                Err(observed) => state = observed,
            }
        }
        if self.publication_state.load(Ordering::SeqCst) != publication_epoch {
            self.release();
            return None;
        }
        Some(GenerationActivity {
            fence: Arc::clone(self),
        })
    }

    pub(crate) fn mark_current(&self) {
        self.state
            .compare_exchange(
                GENERATION_PREPARING,
                GENERATION_CURRENT,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .expect("a generation fence becomes current exactly once");
    }

    pub(crate) fn mark_retired(&self) {
        let _commit = self
            .commit
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut state = self.state.load(Ordering::Acquire);
        loop {
            match generation_phase(state) {
                GENERATION_RETIRED => return,
                GENERATION_PREPARING | GENERATION_CURRENT => {}
                _ => unreachable!("generation fence phase uses two frozen bits"),
            }
            let retired = (state & !GENERATION_PHASE_MASK) | GENERATION_RETIRED;
            match self.state.compare_exchange_weak(
                state,
                retired,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return,
                Err(observed) => state = observed,
            }
        }
    }

    pub(super) fn signal_cancellation(&self) {
        self.cancellation.notify_waiters();
    }

    async fn cancelled(&self) {
        if generation_phase(self.state.load(Ordering::Acquire)) != GENERATION_CURRENT {
            return;
        }
        let notified = self.cancellation.notified();
        if generation_phase(self.state.load(Ordering::Acquire)) != GENERATION_CURRENT {
            return;
        }
        notified.await;
    }

    pub(super) async fn drain(&self) {
        loop {
            if generation_activity_count(self.state.load(Ordering::Acquire)) == 0 {
                return;
            }
            let notified = self.drained.notified();
            if generation_activity_count(self.state.load(Ordering::Acquire)) == 0 {
                return;
            }
            notified.await;
        }
    }

    #[cfg(test)]
    pub(super) fn admission_attempts(&self) -> usize {
        self.admission_attempts.load(Ordering::Relaxed)
    }

    fn release(&self) {
        let previous = self
            .state
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |active| {
                (generation_activity_count(active) > 0)
                    .then(|| active - GENERATION_ACTIVITY_INCREMENT)
            })
            .expect("a generation activity releases its reservation exactly once");
        if generation_activity_count(previous) == 1 {
            self.drained.notify_one();
        }
    }
}

const fn generation_phase(state: usize) -> usize {
    state & GENERATION_PHASE_MASK
}

pub(super) const fn generation_activity_count(state: usize) -> usize {
    state >> 2
}

pub(crate) struct GenerationActivity {
    fence: Arc<GenerationFence>,
}

impl GenerationActivity {
    pub(crate) fn begin_commit(&self) -> Result<GenerationCommit<'_>, GenerationCommitError> {
        let commit = self
            .fence
            .commit
            .lock()
            .map_err(|_| GenerationCommitError::Unavailable)?;
        if generation_phase(self.fence.state.load(Ordering::Acquire)) != GENERATION_CURRENT {
            return Err(GenerationCommitError::Stale);
        }
        Ok(GenerationCommit { _commit: commit })
    }
}

pub(crate) struct GenerationCommit<'a> {
    _commit: MutexGuard<'a, ()>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GenerationCommitError {
    Stale,
    Unavailable,
}

impl Drop for GenerationActivity {
    fn drop(&mut self) {
        self.fence.release();
    }
}

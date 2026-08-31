//! Owns one Manager generation's lifecycle, admission fence, and Store.
//!
//! Preparation cannot admit Host work. Publication and retirement use one
//! atomic phase-plus-count state so retirement linearizes with every activity
//! reservation before quiescent Store cleanup.

// Rust guideline compliant 2026-08-31.

use std::sync::Arc;
#[cfg(test)]
use std::sync::Mutex as SyncMutex;
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};

use mcode_config::{AuthorityRevision, ManagerRecord, PluginFamily};
use mcode_plugin_api::TaskGeneration;
use tokio::sync::{Mutex as AsyncMutex, Notify};

use super::{CurrentManagerGeneration, PUBLICATION_OPEN, ReconciliationError};
use crate::runtime::{
    LifecycleErrorCode, LifecycleOutcome, LifecycleState, ManagerInstance, OperationLease,
    PluginOwner, PluginRuntime,
};

pub(super) struct GenerationOwner {
    owner: PluginOwner,
    instance: ManagerInstance,
    operation: OperationLease,
}

pub(super) struct ActiveGeneration {
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
        runtime: &PluginRuntime,
        publication_state: Arc<AtomicU8>,
        family: PluginFamily,
        record: ManagerRecord,
        generation: TaskGeneration,
        component: crate::runtime::CompiledManagerComponent,
    ) -> Result<(Arc<Self>, bool), ReconciliationError> {
        let fence = Arc::new(GenerationFence::new(publication_state));
        let mut owner = runtime
            .new_owner()
            .map_err(|_| ReconciliationError::Runtime(family))?;
        owner
            .bind_generation_fence(Arc::clone(&fence))
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
        let state = match preparation {
            Ok(state) => state,
            Err(error) => {
                fence.mark_retired();
                fence.signal_cancellation();
                if owner.is_available() {
                    let _ = instance.shutdown(&mut owner, &mut operation).await;
                }
                return Err(error);
            }
        };
        let pending = state == LifecycleState::Pending;
        Ok((
            Arc::new(Self {
                family,
                record,
                generation,
                fence,
                owner: AsyncMutex::new(Some(GenerationOwner {
                    owner,
                    instance,
                    operation,
                })),
                #[cfg(test)]
                shutdown_observation: SyncMutex::new(None),
            }),
            pending,
        ))
    }

    pub(super) fn view(&self, revision: AuthorityRevision) -> CurrentManagerGeneration {
        CurrentManagerGeneration {
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
        let GenerationOwner {
            owner,
            instance,
            operation,
        } = generation;
        let outcome = instance
            .poll(owner, operation)
            .await
            .map_err(|_| ReconciliationError::Runtime(self.family))?;
        preparation_state(self.family, outcome)
    }

    #[cfg(test)]
    pub(super) async fn poll_lifecycle(&self) -> Result<LifecycleState, GenerationCallError> {
        let _activity = self.fence.enter().ok_or(GenerationCallError::Retired)?;
        let cancellation = self.fence.cancelled();
        tokio::pin!(cancellation);
        let mut guard = tokio::select! {
            biased;
            () = &mut cancellation => return Err(GenerationCallError::Cancelled),
            guard = self.owner.lock() => guard,
        };
        let generation = guard.as_mut().ok_or(GenerationCallError::Unavailable)?;
        let GenerationOwner {
            owner,
            instance,
            operation,
        } = generation;
        let call = instance.poll(owner, operation);
        tokio::pin!(call);
        let outcome = tokio::select! {
            biased;
            () = &mut cancellation => return Err(GenerationCallError::Cancelled),
            outcome = &mut call => outcome.map_err(|_| GenerationCallError::Runtime)?,
        };
        match outcome {
            Ok(state) => Ok(state),
            Err(_) => Err(GenerationCallError::Rejected),
        }
    }

    pub(super) async fn retire(&self) {
        self.fence.drain().await;
        let mut guard = self.owner.lock().await;
        let Some(mut generation) = guard.take() else {
            return;
        };
        if generation.owner.is_available() {
            let GenerationOwner {
                owner,
                instance,
                operation,
            } = &mut generation;
            let outcome = instance.shutdown(owner, operation).await;
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
#[cfg(test)]
pub(super) enum GenerationCallError {
    Retired,
    Cancelled,
    Unavailable,
    Runtime,
    Rejected,
}

pub(crate) struct GenerationFence {
    publication_state: Arc<AtomicU8>,
    pub(super) state: AtomicUsize,
    cancellation: Notify,
    drained: Notify,
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
    pub(super) fn new(publication_state: Arc<AtomicU8>) -> Self {
        Self {
            publication_state,
            state: AtomicUsize::new(GENERATION_PREPARING),
            cancellation: Notify::new(),
            drained: Notify::new(),
            #[cfg(test)]
            admission_attempts: AtomicUsize::new(0),
        }
    }

    pub(crate) fn enter(self: &Arc<Self>) -> Option<GenerationActivity> {
        #[cfg(test)]
        self.admission_attempts.fetch_add(1, Ordering::Relaxed);
        if self.publication_state.load(Ordering::SeqCst) != PUBLICATION_OPEN {
            return None;
        }
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
        Some(GenerationActivity {
            fence: Arc::clone(self),
        })
    }

    pub(super) fn mark_current(&self) {
        self.state
            .compare_exchange(
                GENERATION_PREPARING,
                GENERATION_CURRENT,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .expect("a generation fence becomes current exactly once");
    }

    pub(super) fn mark_retired(&self) {
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

    #[cfg(test)]
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

impl Drop for GenerationActivity {
    fn drop(&mut self) {
        self.fence.release();
    }
}

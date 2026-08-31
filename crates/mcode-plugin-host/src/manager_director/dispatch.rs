//! Dispatches one exact published Manager generation without exposing ownership.

// Rust guideline compliant 2026-08-31.

use std::fmt::{self, Display, Formatter};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as SyncMutex};

use super::generation::{ActiveGeneration, GenerationCallError};
use super::{
    CurrentManagerGeneration, DirectorState, GenerationActivity, ManagerGenerationDirector,
    PUBLICATION_CLOSED, ReconciliationError, ensure_open, take_retained_generations,
};
use crate::manager_loading::family_index;
use crate::runtime::{LifecycleOutcome, LifecycleState};

pub(super) struct CurrentGenerationCall {
    pub(super) entry: Arc<ActiveGeneration>,
    generation: CurrentManagerGeneration,
    activity: Option<GenerationActivity>,
    cleanup: super::CleanupWorker,
    state: Arc<SyncMutex<DirectorState>>,
    publication_state: Arc<AtomicU64>,
    retire_on_drop: bool,
}

impl CurrentGenerationCall {
    pub(super) fn take_activity(&mut self) -> GenerationActivity {
        self.activity
            .take()
            .expect("one current call consumes its activity exactly once")
    }

    pub(super) fn arm_retirement(&mut self) {
        self.retire_on_drop = true;
    }

    pub(super) fn keep_current(&mut self) {
        self.retire_on_drop = false;
    }

    fn retire(&mut self) -> Result<(), ManagerGenerationCallError> {
        if !self.retire_on_drop {
            return Ok(());
        }
        let mut unavailable = false;
        {
            let mut state = match self.state.lock() {
                Ok(state) => state,
                Err(error) => {
                    self.publication_state
                        .store(PUBLICATION_CLOSED, Ordering::SeqCst);
                    let mut state = error.into_inner();
                    state.closed = true;
                    let retired = take_retained_generations(&mut state);
                    for entry in &retired {
                        entry.fence.signal_cancellation();
                    }
                    if !retired.is_empty() {
                        let _ = self.cleanup.retire_after_cancelled_call(retired);
                    }
                    self.retire_on_drop = false;
                    return Err(ManagerGenerationCallError::SelectedUnavailable(Box::new(
                        self.generation.clone(),
                    )));
                }
            };
            let slot = &mut state.current[family_index(self.entry.family)];
            if slot
                .as_ref()
                .is_some_and(|entry| Arc::ptr_eq(entry, &self.entry))
            {
                let current = slot
                    .take()
                    .expect("the exact checked generation remains current");
                state.current_epoch = state
                    .current_epoch
                    .checked_add(1)
                    .expect("the current topology epoch cannot exhaust");
                let mut retired = vec![current];
                if let Some(mut prepared) = state.preparation.take() {
                    retired.extend(prepared.slots.iter_mut().filter_map(Option::take));
                }
                for entry in &retired {
                    entry.fence.mark_retired();
                    entry.fence.signal_cancellation();
                }
                if self.cleanup.retire_after_cancelled_call(retired).is_err() {
                    unavailable = true;
                }
            }
        }
        self.retire_on_drop = false;
        if unavailable {
            Err(ManagerGenerationCallError::SelectedUnavailable(Box::new(
                self.generation.clone(),
            )))
        } else {
            Ok(())
        }
    }
}

impl Drop for CurrentGenerationCall {
    fn drop(&mut self) {
        if self.retire_on_drop {
            drop(self.retire());
        }
    }
}

/// Reports one generation-stamped current Manager lifecycle poll.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CurrentManagerPoll {
    generation: CurrentManagerGeneration,
    outcome: LifecycleOutcome,
}

impl CurrentManagerPoll {
    /// Returns the exact generation selected for this lifecycle call.
    #[must_use]
    pub const fn generation(&self) -> &CurrentManagerGeneration {
        &self.generation
    }

    /// Returns the Manager's stable lifecycle outcome.
    pub const fn outcome(&self) -> LifecycleOutcome {
        self.outcome
    }
}

/// Reports one stable, non-sensitive current Manager call failure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ManagerGenerationCallError {
    /// The expected generation is no longer current.
    Stale,
    /// The director has begun its final shutdown.
    Closed,
    /// Director state is unavailable before generation selection.
    Unavailable,
    /// The selected call was cancelled by generation retirement.
    Cancelled(Box<CurrentManagerGeneration>),
    /// The selected Manager failed during lifecycle execution.
    Runtime(Box<CurrentManagerGeneration>),
    /// Post-selection synchronization or cleanup execution is unavailable.
    SelectedUnavailable(Box<CurrentManagerGeneration>),
}

impl Display for ManagerGenerationCallError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Stale => formatter.write_str("expected Manager generation is stale"),
            Self::Closed => formatter.write_str("Manager generation director is closed"),
            Self::Unavailable => formatter.write_str("Manager generation call is unavailable"),
            Self::Cancelled(generation) => write!(
                formatter,
                "Manager generation call was cancelled for {}",
                generation.family().directory_name()
            ),
            Self::Runtime(generation) => write!(
                formatter,
                "Manager lifecycle execution failed for {}",
                generation.family().directory_name()
            ),
            Self::SelectedUnavailable(generation) => write!(
                formatter,
                "selected Manager generation became unavailable for {}",
                generation.family().directory_name()
            ),
        }
    }
}

impl std::error::Error for ManagerGenerationCallError {}

impl ManagerGenerationDirector {
    /// Polls one exact published Manager generation once.
    ///
    /// `expected` is an opaque generation tag previously returned by this
    /// director. Selection, tag validation, and activity admission happen
    /// together before the state lock is released. The call never retries or
    /// polls a replacement generation. Its returned stamp records selection,
    /// not proof that the generation remains current when the future resolves.
    /// Revision is observational, so an older-revision tag still selects the
    /// same live generation and the returned stamp carries the current revision.
    /// Dropping a call that has entered guest code retires that exact generation.
    ///
    /// # Errors
    ///
    /// Returns [`ManagerGenerationCallError::Stale`] before guest entry when
    /// `expected` is no longer current. Shutdown and pre-selection state
    /// failures are unstamped; every post-selection failure is generation
    /// stamped.
    pub async fn poll_current(
        &self,
        expected: &CurrentManagerGeneration,
    ) -> Result<CurrentManagerPoll, ManagerGenerationCallError> {
        let mut call = self.acquire_current(expected)?;
        let entry = Arc::clone(&call.entry);
        let generation = call.generation.clone();
        match entry.poll_current(&mut call).await {
            Ok(outcome) => {
                if !matches!(outcome, Ok(LifecycleState::Ready | LifecycleState::Pending)) {
                    call.retire()?;
                }
                Ok(CurrentManagerPoll {
                    generation,
                    outcome,
                })
            }
            Err(GenerationCallError::Cancelled) => {
                call.retire()?;
                Err(ManagerGenerationCallError::Cancelled(Box::new(generation)))
            }
            Err(GenerationCallError::Unavailable | GenerationCallError::Runtime) => {
                call.retire()?;
                Err(ManagerGenerationCallError::Runtime(Box::new(generation)))
            }
        }
    }

    fn acquire_current(
        &self,
        expected: &CurrentManagerGeneration,
    ) -> Result<CurrentGenerationCall, ManagerGenerationCallError> {
        if expected.identity != self.identity {
            return Err(ManagerGenerationCallError::Stale);
        }
        let state = self.lock_state().map_err(call_access_error)?;
        ensure_open(&state).map_err(call_access_error)?;
        let entry = state.current[family_index(expected.family())]
            .as_ref()
            .ok_or(ManagerGenerationCallError::Stale)?;
        let generation = entry.view(state.revision);
        if generation.family != expected.family
            || generation.record != expected.record
            || generation.generation != expected.generation
        {
            return Err(ManagerGenerationCallError::Stale);
        }
        let activity = entry
            .fence
            .enter()
            .ok_or(ManagerGenerationCallError::Stale)?;
        Ok(CurrentGenerationCall {
            entry: Arc::clone(entry),
            generation,
            activity: Some(activity),
            cleanup: self.cleanup.clone(),
            state: Arc::clone(&self.state),
            publication_state: Arc::clone(&self.publication_state),
            retire_on_drop: false,
        })
    }
}

fn call_access_error(error: ReconciliationError) -> ManagerGenerationCallError {
    match error {
        ReconciliationError::Closed => ManagerGenerationCallError::Closed,
        ReconciliationError::Unavailable => ManagerGenerationCallError::Unavailable,
        _ => ManagerGenerationCallError::Unavailable,
    }
}

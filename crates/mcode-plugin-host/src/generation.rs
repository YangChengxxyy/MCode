//! Host-owned generation fence for atomic Pack set publication.
//!
//! One fence owns one family's generation lifecycle. Preparation cannot admit
//! Host work. Publication and retirement use one atomic phase-plus-count state
//! so retirement linearizes with every activity reservation before quiescent
//! Store cleanup.

// Rust guideline compliant 2026-09-05.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as SyncMutex, MutexGuard};

use mcode_config::PluginFamily;
use tokio::sync::Notify;

const MAX_HOST_GENERATION: u64 = 9_007_199_254_740_991;

/// Identifies one Host generation of one family's active Pack set.
///
/// Values start at one. Zero is the reserved [`HostGeneration::ABSENT`]
/// binding used before any generation has been published.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct HostGeneration(u64);

impl HostGeneration {
    /// Creates one generation value in the exact JSON-safe positive range.
    pub(crate) const fn new(value: u64) -> Option<Self> {
        if value == 0 || value > MAX_HOST_GENERATION {
            None
        } else {
            Some(Self(value))
        }
    }

    /// Returns the exact generation value.
    pub(crate) const fn get(self) -> u64 {
        self.0
    }
}

/// Marks a publication authority as finally closed.
pub(crate) const PUBLICATION_CLOSED: u64 = u64::MAX;


pub(crate) struct GenerationFence {
    publication_state: Arc<AtomicU64>,
    family: PluginFamily,
    generation: HostGeneration,
    state: AtomicUsize,
    drained: Notify,
    commit: SyncMutex<()>,
}

// The low two bits carry the phase so retirement and activity admission share
// one CAS linearization point. All remaining bits carry the activity count.
const GENERATION_PHASE_MASK: usize = 0b11;
const GENERATION_PREPARING: usize = 0;
const GENERATION_CURRENT: usize = 1;
const GENERATION_RETIRED: usize = 2;
const GENERATION_ACTIVITY_INCREMENT: usize = GENERATION_PHASE_MASK + 1;
const MAX_GENERATION_ACTIVITIES: usize = usize::MAX >> 2;

impl GenerationFence {
    pub(crate) fn new(
        publication_state: Arc<AtomicU64>,
        family: PluginFamily,
        generation: HostGeneration,
    ) -> Self {
        Self {
            publication_state,
            family,
            generation,
            state: AtomicUsize::new(GENERATION_PREPARING),
            drained: Notify::new(),
            commit: SyncMutex::new(()),
        }
    }

    /// Returns the family this fence gates.
    pub(crate) const fn family(&self) -> PluginFamily {
        self.family
    }

    /// Returns the generation this fence gates.
    pub(crate) const fn generation(&self) -> HostGeneration {
        self.generation
    }

    /// Returns whether the publication authority is finally closed.
    pub(crate) fn publication_closed(&self) -> bool {
        self.publication_state.load(Ordering::SeqCst) == PUBLICATION_CLOSED
    }

    /// Closes the shared publication authority forever.
    pub(crate) fn close_publication(&self) {
        self.publication_state
            .store(PUBLICATION_CLOSED, Ordering::SeqCst);
    }

    pub(crate) fn enter(self: &Arc<Self>) -> Option<GenerationActivity> {
        let publication_epoch = self.publication_state.load(Ordering::SeqCst);
        if publication_epoch == PUBLICATION_CLOSED || !publication_epoch.is_multiple_of(2) {
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

const fn generation_activity_count(state: usize) -> usize {
    state >> 2
}

/// One admitted activity reservation on a current generation fence.
pub(crate) struct GenerationActivity {
    fence: Arc<GenerationFence>,
}

impl GenerationActivity {
    /// Begins one exclusive commit window on the still-current generation.
    ///
    /// The returned guard linearizes with retirement: a fence cannot retire
    /// while the commit window is open.
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

/// Holds one generation's exclusive commit window.
pub(crate) struct GenerationCommit<'a> {
    _commit: MutexGuard<'a, ()>,
}

/// Reports why a generation commit window could not open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GenerationCommitError {
    /// The generation retired before the commit began.
    Stale,
    /// The commit mutex was poisoned.
    Unavailable,
}

impl Drop for GenerationActivity {
    fn drop(&mut self) {
        self.fence.release();
    }
}

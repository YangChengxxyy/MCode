//! Cooperative cancellation for watcher-independent configuration reloads.

// Rust guideline compliant 2026-08-26

use std::fmt::{self, Debug, Formatter};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Cancels one or more configuration reload attempts cooperatively.
///
/// Clones observe the same one-way cancellation state. Cancellation is checked
/// while reading, parsing, merging, validating, and immediately before
/// publication. A caller-provided validation hook cannot be interrupted while
/// it is executing; cancellation is observed as soon as the hook returns.
#[derive(Clone, Default)]
pub struct ReloadCancellation {
    cancelled: Arc<AtomicBool>,
}

impl ReloadCancellation {
    /// Creates a token in the active state.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Requests cancellation for every clone of this token.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    /// Reports whether cancellation has been requested.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}

impl Debug for ReloadCancellation {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ReloadCancellation")
            .field("cancelled", &self.is_cancelled())
            .finish()
    }
}

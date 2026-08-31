//! Publishes root Pack configuration independently from Manager authority.

// Rust guideline compliant 2026-08-31.

use std::sync::Arc;

use mcode_config::RootCompositionDocument;

use super::{ManagerGenerationDirector, ReconciliationError, ensure_open};
use crate::pack_selection::PackConfigurationError;

impl ManagerGenerationDirector {
    /// Publishes one validated root composition to every Manager generation.
    ///
    /// A missing document is the empty revision-zero configuration. Manager
    /// reconciliation and Pack configuration publication share one ordering
    /// mutex, while their authority revisions remain independent.
    ///
    /// # Errors
    ///
    /// Returns [`PackConfigurationError`] for revision regression or conflict,
    /// after shutdown begins, or when synchronization is unavailable.
    pub async fn publish_pack_configuration(
        &self,
        document: Option<RootCompositionDocument>,
    ) -> Result<(), PackConfigurationError> {
        let _serialized = Arc::clone(&self.reconciliation).lock_owned().await;
        let state = self.lock_state().map_err(map_director_error)?;
        ensure_open(&state).map_err(map_director_error)?;
        drop(state);
        self.pack_selections.publish(document)
    }
}

fn map_director_error(error: ReconciliationError) -> PackConfigurationError {
    match error {
        ReconciliationError::Closed => PackConfigurationError::Closed,
        _ => PackConfigurationError::Unavailable,
    }
}

#[cfg(test)]
#[path = "pack_configuration_tests.rs"]
mod tests;

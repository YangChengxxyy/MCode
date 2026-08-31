//! Dispatches bounded task calls to one exact current Manager generation.

// Rust guideline compliant 2026-08-31.

use std::sync::Arc;

use super::generation::GenerationCallError;
use super::{CurrentManagerGeneration, ManagerGenerationCallError, ManagerGenerationDirector};
use crate::runtime::ManagerTaskCall;

/// Reports one generation-stamped Manager task response.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CurrentManagerTaskReply {
    generation: CurrentManagerGeneration,
    response: String,
}

impl CurrentManagerTaskReply {
    /// Returns the exact generation selected for this task call.
    #[must_use]
    pub const fn generation(&self) -> &CurrentManagerGeneration {
        &self.generation
    }

    /// Returns the bounded Manager response.
    #[must_use]
    pub fn response(&self) -> &str {
        &self.response
    }
}

impl ManagerGenerationDirector {
    /// Calls `manager-tasks.start-task` on one exact current generation.
    ///
    /// # Errors
    ///
    /// Returns [`ManagerGenerationCallError`] when selection, bounds, guest
    /// execution, cancellation, or cleanup rejects the call.
    pub async fn start_current_task(
        &self,
        expected: &CurrentManagerGeneration,
        request: &str,
    ) -> Result<CurrentManagerTaskReply, ManagerGenerationCallError> {
        self.call_current_task(expected, ManagerTaskCall::Start, request)
            .await
    }

    /// Calls `manager-tasks.poll-task` on one exact current generation.
    ///
    /// # Errors
    ///
    /// Returns [`ManagerGenerationCallError`] when selection, bounds, guest
    /// execution, cancellation, or cleanup rejects the call.
    pub async fn poll_current_task(
        &self,
        expected: &CurrentManagerGeneration,
        request: &str,
    ) -> Result<CurrentManagerTaskReply, ManagerGenerationCallError> {
        self.call_current_task(expected, ManagerTaskCall::Poll, request)
            .await
    }

    /// Calls `manager-tasks.cancel-task` on one exact current generation.
    ///
    /// # Errors
    ///
    /// Returns [`ManagerGenerationCallError`] when selection, bounds, guest
    /// execution, cancellation, or cleanup rejects the call.
    pub async fn cancel_current_task(
        &self,
        expected: &CurrentManagerGeneration,
        request: &str,
    ) -> Result<CurrentManagerTaskReply, ManagerGenerationCallError> {
        self.call_current_task(expected, ManagerTaskCall::Cancel, request)
            .await
    }

    async fn call_current_task(
        &self,
        expected: &CurrentManagerGeneration,
        task_call: ManagerTaskCall,
        request: &str,
    ) -> Result<CurrentManagerTaskReply, ManagerGenerationCallError> {
        let mut call = self.acquire_current(expected)?;
        let entry = Arc::clone(&call.entry);
        let generation = call.generation.clone();
        match entry.call_task(&mut call, task_call, request).await {
            Ok(response) => Ok(CurrentManagerTaskReply {
                generation,
                response,
            }),
            Err(GenerationCallError::InvalidInput) => Err(
                ManagerGenerationCallError::InvalidRequest(Box::new(generation)),
            ),
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
}

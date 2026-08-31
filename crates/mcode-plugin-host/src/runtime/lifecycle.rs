//! Typed, owner-bound Manager lifecycle execution.

// Rust guideline compliant 2026-08-31.

use mcode_plugin_api::TaskGeneration;

use crate::wit::exports::mcode::plugin::manager_lifecycle::{
    ErrorCode as GuestErrorCode, InitializationContext, State as GuestState,
};

use super::owner::ManagerInstance;
use super::segment::SegmentExecution;
use super::{OperationLease, PluginOwner, RuntimeError};

/// Reports one stable Manager lifecycle state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LifecycleState {
    /// The Manager can serve work.
    Ready,
    /// The Manager is still progressing asynchronously.
    Pending,
    /// The Manager is shutting down.
    Stopping,
    /// The Manager has stopped.
    Stopped,
}

/// Reports one stable, non-sensitive Manager lifecycle rejection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LifecycleErrorCode {
    /// The requested transition was invalid for the current state.
    InvalidState,
    /// A required feature is unavailable.
    FeatureUnavailable,
    /// The Manager rejected the transition without sensitive detail.
    Failed,
}

/// Contains a typed Manager lifecycle state or stable guest rejection.
pub type LifecycleOutcome = Result<LifecycleState, LifecycleErrorCode>;

impl ManagerInstance {
    /// Initializes this Manager for one validated generation.
    ///
    /// The call consumes fuel from `operation` and executes only in `owner`'s
    /// Store. Generation validation runs before guest entry.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError::InvalidGeneration`] outside
    /// `1..=9_007_199_254_740_991`. Returns an ownership, Store policy, fuel,
    /// or guest error when that fail-closed boundary rejects execution.
    pub async fn initialize(
        &self,
        owner: &mut PluginOwner,
        operation: &mut OperationLease,
        generation: u64,
    ) -> Result<LifecycleOutcome, RuntimeError> {
        let generation =
            TaskGeneration::new(generation).map_err(|_| RuntimeError::InvalidGeneration)?;
        self.verify_owners(owner, operation)?;

        let context = InitializationContext {
            generation: generation.get(),
        };
        let mut execution = SegmentExecution::start_plugin_call(owner, operation)?;
        let guest_result = self
            .bindings
            .mcode_plugin_manager_lifecycle()
            .call_initialize(execution.store_mut(), context)
            .await;
        finish_call(execution, guest_result)
    }

    /// Polls this Manager's typed lifecycle state.
    ///
    /// The call consumes the same total fuel remainder carried by `operation`.
    ///
    /// # Errors
    ///
    /// Returns an ownership, Store policy, fuel, or guest error when that
    /// fail-closed boundary rejects execution.
    pub async fn poll(
        &self,
        owner: &mut PluginOwner,
        operation: &mut OperationLease,
    ) -> Result<LifecycleOutcome, RuntimeError> {
        self.verify_owners(owner, operation)?;

        let mut execution = SegmentExecution::start_plugin_call(owner, operation)?;
        let guest_result = self
            .bindings
            .mcode_plugin_manager_lifecycle()
            .call_poll(execution.store_mut())
            .await;
        finish_call(execution, guest_result)
    }

    /// Requests this Manager's typed lifecycle shutdown.
    ///
    /// The call consumes the same total fuel remainder carried by `operation`.
    ///
    /// # Errors
    ///
    /// Returns an ownership, Store policy, fuel, or guest error when that
    /// fail-closed boundary rejects execution.
    pub async fn shutdown(
        &self,
        owner: &mut PluginOwner,
        operation: &mut OperationLease,
    ) -> Result<LifecycleOutcome, RuntimeError> {
        self.verify_owners(owner, operation)?;

        let mut execution = SegmentExecution::start_plugin_call(owner, operation)?;
        let guest_result = self
            .bindings
            .mcode_plugin_manager_lifecycle()
            .call_shutdown(execution.store_mut())
            .await;
        let outcome = finish_call(execution, guest_result);
        #[cfg(test)]
        self.runtime.observe_shutdown(outcome);
        outcome
    }

    pub(super) fn verify_owners(
        &self,
        owner: &PluginOwner,
        operation: &OperationLease,
    ) -> Result<(), RuntimeError> {
        if operation.owner != owner.identity {
            return Err(RuntimeError::OwnerMismatch);
        }
        if self.owner != owner.identity {
            return Err(RuntimeError::InstanceMismatch);
        }
        Ok(())
    }
}

fn finish_call(
    execution: SegmentExecution<'_>,
    result: wasmtime::Result<Result<GuestState, GuestErrorCode>>,
) -> Result<LifecycleOutcome, RuntimeError> {
    let outcome = match result {
        Ok(outcome) => {
            execution.complete()?;
            outcome
        }
        Err(_) => {
            // A runtime trap can leave component-owned state unusable or
            // partially advanced. Account for fuel, then dispose the Store.
            execution.dispose()?;
            return Err(RuntimeError::Guest);
        }
    };
    Ok(match outcome {
        Ok(GuestState::Ready) => Ok(LifecycleState::Ready),
        Ok(GuestState::Pending) => Ok(LifecycleState::Pending),
        Ok(GuestState::Stopping) => Ok(LifecycleState::Stopping),
        Ok(GuestState::Stopped) => Ok(LifecycleState::Stopped),
        Err(GuestErrorCode::InvalidState) => Err(LifecycleErrorCode::InvalidState),
        Err(GuestErrorCode::FeatureUnavailable) => Err(LifecycleErrorCode::FeatureUnavailable),
        Err(GuestErrorCode::Failed) => Err(LifecycleErrorCode::Failed),
    })
}

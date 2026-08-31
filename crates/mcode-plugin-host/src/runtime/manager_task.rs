//! Bounded owner-bound Manager task export execution.

// Rust guideline compliant 2026-08-31.

use mcode_plugin_api::MAX_MANAGER_TASK_WIRE_BYTES;

use super::owner::ManagerInstance;
use super::segment::SegmentExecution;
use super::{OperationLease, PluginOwner};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ManagerTaskCall {
    Start,
    Poll,
    Cancel,
}

#[derive(Debug)]
pub(crate) enum ManagerTaskCallError {
    InputTooLarge,
    Runtime,
}

impl ManagerInstance {
    pub(crate) async fn call_task(
        &self,
        owner: &mut PluginOwner,
        operation: &mut OperationLease,
        call: ManagerTaskCall,
        request: &str,
    ) -> Result<String, ManagerTaskCallError> {
        if request.len() > MAX_MANAGER_TASK_WIRE_BYTES {
            return Err(ManagerTaskCallError::InputTooLarge);
        }
        self.verify_owners(owner, operation)
            .map_err(|_| ManagerTaskCallError::Runtime)?;

        let mut execution = SegmentExecution::start_plugin_call(owner, operation)
            .map_err(|_| ManagerTaskCallError::Runtime)?;
        let guest_result = match call {
            ManagerTaskCall::Start => {
                self.bindings
                    .mcode_plugin_manager_tasks()
                    .call_start_task(execution.store_mut(), request)
                    .await
            }
            ManagerTaskCall::Poll => {
                self.bindings
                    .mcode_plugin_manager_tasks()
                    .call_poll_task(execution.store_mut(), request)
                    .await
            }
            ManagerTaskCall::Cancel => {
                self.bindings
                    .mcode_plugin_manager_tasks()
                    .call_cancel_task(execution.store_mut(), request)
                    .await
            }
        };
        match guest_result {
            Ok(response) if response.len() <= MAX_MANAGER_TASK_WIRE_BYTES => {
                execution
                    .complete()
                    .map_err(|_| ManagerTaskCallError::Runtime)?;
                Ok(response)
            }
            Ok(_) | Err(_) => {
                execution
                    .dispose()
                    .map_err(|_| ManagerTaskCallError::Runtime)?;
                Err(ManagerTaskCallError::Runtime)
            }
        }
    }
}

//! Resources FeatureService task-wire dispatch for one bound Manager generation.

// Rust guideline compliant 2026-08-31.

use std::sync::OnceLock;

use mcode_config::PluginFamily;
use mcode_plugin_api::{
    FeatureTaskClosed, FeatureTaskCompleted, FeatureTaskControl, FeatureTaskError,
    FeatureTaskHandle, FeatureTaskProgress, FeatureTaskRejection, OperationId,
    ResourcesTaskRequest, TaskErrorCode, TaskFailure, TaskWireError, validate_resources_operation,
};
use tokio::time::Instant;

use crate::decode_bound_feature_task;
use crate::pack_activation::{ResourcesTaskError, ResourcesTaskPoll};
use crate::runtime::ResourcesPackError;

use super::owner::StoreData;

pub(super) async fn start_task(data: &mut StoreData, request: String) -> String {
    let Some(_activity) = data.enter_current_generation() else {
        return rejection(TaskErrorCode::StaleGeneration);
    };
    let Some(caller) = data.feature_caller() else {
        return rejection(TaskErrorCode::FeatureUnavailable);
    };
    if caller.family() != PluginFamily::Resources {
        return rejection(TaskErrorCode::FeatureUnavailable);
    }
    let Some(duration) = data.feature_deadline(PluginFamily::Resources) else {
        return rejection(TaskErrorCode::FeatureUnavailable);
    };
    let decoded = match decode_bound_feature_task::<ResourcesTaskRequest>(
        &caller,
        PluginFamily::Resources,
        declared_resources_operations(),
        request.as_bytes(),
    ) {
        Ok(decoded) => decoded,
        Err(error) => return rejection(map_wire_error(error)),
    };
    if let Err(error) = validate_resources_operation(decoded.operation_id(), decoded.request()) {
        return rejection(error);
    }
    let operation_id = decoded.operation_id().clone();
    let generation = decoded.generation();
    let request = decoded.into_request();
    let Some(deadline) = Instant::now().checked_add(duration) else {
        return rejection(TaskErrorCode::FeatureUnavailable);
    };
    let Some(activation) = data.pack_activation_mut() else {
        return rejection(TaskErrorCode::FeatureUnavailable);
    };
    match activation
        .start_resources_task(operation_id.clone(), generation, request, deadline)
        .await
    {
        Ok(task_id) => {
            encode_or_rejection(FeatureTaskHandle::new(operation_id, task_id, generation).encode())
        }
        Err(error) => rejection(map_task_error(error)),
    }
}

pub(super) async fn poll_task(data: &mut StoreData, request: String) -> String {
    let control = match FeatureTaskControl::decode(request.as_bytes()) {
        Ok(control) => control,
        Err(_) => return rejection(TaskErrorCode::InvalidRequest),
    };
    if let Some(error) = validate_control_binding(data, &control) {
        return assigned_error(&control, error);
    }
    let Some(_activity) = data.enter_current_generation() else {
        return assigned_error(&control, TaskErrorCode::StaleGeneration);
    };
    if data.feature_deadline(PluginFamily::Resources).is_none() {
        return assigned_error(&control, TaskErrorCode::FeatureUnavailable);
    }
    let Some(activation) = data.pack_activation_mut() else {
        return assigned_error(&control, TaskErrorCode::FeatureUnavailable);
    };
    match activation
        .poll_resources_task(
            control.operation_id(),
            control.task_id(),
            control.generation(),
        )
        .await
    {
        Ok(ResourcesTaskPoll::Open) => encode_or_assigned_error(
            &control,
            FeatureTaskHandle::new(
                control.operation_id().clone(),
                control.task_id().clone(),
                control.generation(),
            )
            .encode(),
        ),
        Ok(ResourcesTaskPoll::Progress(progress)) => encode_or_assigned_error(
            &control,
            FeatureTaskProgress::new(
                control.operation_id().clone(),
                control.task_id().clone(),
                control.generation(),
                progress,
            )
            .encode(),
        ),
        Ok(ResourcesTaskPoll::Complete(result)) => encode_or_assigned_error(
            &control,
            FeatureTaskCompleted::new(
                control.operation_id().clone(),
                control.task_id().clone(),
                control.generation(),
                result,
            )
            .encode(),
        ),
        Err(error) => assigned_error(&control, map_task_error(error)),
    }
}

pub(super) async fn cancel_task(data: &mut StoreData, request: String) -> String {
    let control = match FeatureTaskControl::decode(request.as_bytes()) {
        Ok(control) => control,
        Err(_) => return rejection(TaskErrorCode::InvalidRequest),
    };
    if let Some(error) = validate_control_binding(data, &control) {
        return assigned_error(&control, error);
    }
    let Some(_activity) = data.enter_current_generation() else {
        return assigned_error(&control, TaskErrorCode::StaleGeneration);
    };
    let Some(activation) = data.pack_activation_mut() else {
        return assigned_error(&control, TaskErrorCode::FeatureUnavailable);
    };
    match activation
        .cancel_resources_task(
            control.operation_id(),
            control.task_id(),
            control.generation(),
        )
        .await
    {
        Ok(()) => encode_or_assigned_error(
            &control,
            FeatureTaskClosed::new(
                control.operation_id().clone(),
                control.task_id().clone(),
                control.generation(),
            )
            .encode(),
        ),
        Err(error) => assigned_error(&control, map_task_error(error)),
    }
}

fn validate_control_binding(
    data: &StoreData,
    control: &FeatureTaskControl,
) -> Option<TaskErrorCode> {
    let Some(caller) = data.feature_caller() else {
        return Some(TaskErrorCode::FeatureUnavailable);
    };
    if caller.family() != PluginFamily::Resources {
        return Some(TaskErrorCode::CallerMismatch);
    }
    if caller.generation() != control.generation() {
        return Some(TaskErrorCode::StaleGeneration);
    }
    if !declared_resources_operations()
        .iter()
        .any(|operation| operation == control.operation_id())
    {
        return Some(TaskErrorCode::UndeclaredOperation);
    }
    None
}

fn declared_resources_operations() -> &'static [OperationId; 4] {
    static OPERATIONS: OnceLock<[OperationId; 4]> = OnceLock::new();
    OPERATIONS.get_or_init(|| {
        ["catalog", "read", "render-prompt", "contributions"].map(|operation| {
            OperationId::parse(operation).expect("frozen Resources operation ID is canonical")
        })
    })
}

const fn map_wire_error(error: TaskWireError) -> TaskErrorCode {
    match error {
        TaskWireError::BindingRejected(error) => error,
        TaskWireError::TooLarge
        | TaskWireError::InvalidEncoding
        | TaskWireError::InvalidDocument
        | TaskWireError::InvalidBody
        | TaskWireError::EncodeFailed => TaskErrorCode::InvalidRequest,
    }
}

const fn map_task_error(error: ResourcesTaskError) -> TaskErrorCode {
    match error {
        ResourcesTaskError::InvalidRequest => TaskErrorCode::InvalidRequest,
        ResourcesTaskError::TaskLimitReached => TaskErrorCode::TaskLimitReached,
        ResourcesTaskError::UnknownTask => TaskErrorCode::UnknownTask,
        ResourcesTaskError::FeatureUnavailable => TaskErrorCode::FeatureUnavailable,
        ResourcesTaskError::OperationClosed => TaskErrorCode::Cancelled,
        ResourcesTaskError::ActorUnavailable => TaskErrorCode::FeatureUnavailable,
        ResourcesTaskError::Task(error) => error,
        ResourcesTaskError::Guest(error) => match error {
            ResourcesPackError::InvalidArgument => TaskErrorCode::InvalidRequest,
            ResourcesPackError::Cancelled => TaskErrorCode::Cancelled,
            ResourcesPackError::Unavailable => TaskErrorCode::FeatureUnavailable,
            ResourcesPackError::NotFound | ResourcesPackError::Limit => TaskErrorCode::Failed,
        },
    }
}

fn rejection(error: TaskErrorCode) -> String {
    FeatureTaskRejection::new(TaskFailure::new(error))
        .encode()
        .expect("a fixed FeatureService rejection fits the wire bound")
}

fn assigned_error(control: &FeatureTaskControl, error: TaskErrorCode) -> String {
    FeatureTaskError::new(
        control.operation_id().clone(),
        control.task_id().clone(),
        control.generation(),
        TaskFailure::new(error),
    )
    .encode()
    .expect("a fixed FeatureService assigned error fits the wire bound")
}

fn encode_or_rejection(encoded: Result<String, TaskWireError>) -> String {
    encoded.unwrap_or_else(|_| rejection(TaskErrorCode::Failed))
}

fn encode_or_assigned_error(
    control: &FeatureTaskControl,
    encoded: Result<String, TaskWireError>,
) -> String {
    encoded.unwrap_or_else(|_| assigned_error(control, TaskErrorCode::Failed))
}

//! FeatureService task-wire contract tests.

use std::sync::atomic::{AtomicUsize, Ordering};

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize};

use super::{
    FeatureTaskBody, FeatureTaskClosed, FeatureTaskCompleted, FeatureTaskControl, FeatureTaskError,
    FeatureTaskHandle, FeatureTaskProgress, FeatureTaskRejection, FeatureTaskRequest,
    FeatureTaskStart, FeatureTaskTerminal, FeatureTaskUpdate, MAX_DECLARED_OPERATIONS,
    MAX_MANAGER_TASK_WIRE_BYTES, TaskWireError, decode_feature_task_request, sealed,
    validate_declared_operation,
};
use crate::{
    FEATURE_SERVICE_INTERFACE_ID, MANAGER_JSON_ABI_VERSION, MANAGER_LIFECYCLE_INTERFACE_ID,
    MANAGER_WIT_PACKAGE, MANAGER_WORLD_ID, OperationId, TaskErrorCode, TaskFailure, TaskGeneration,
    TaskId,
};

const OPERATION_ID: &str = "read";
const TASK_ID: &str = "task1-fedcba9876543210fedcba9876543210";
const OPEN_RESPONSE: &str = r#"{"abiVersion":2,"kind":"featureService","operationId":"read","taskId":"task1-fedcba9876543210fedcba9876543210","generation":7,"state":"open"}"#;
const PROGRESS_RESPONSE: &str = r#"{"abiVersion":2,"kind":"featureService","operationId":"read","taskId":"task1-fedcba9876543210fedcba9876543210","generation":7,"state":"progress","progress":{"completedUnits":1,"totalUnits":2}}"#;
const COMPLETED_RESPONSE: &str = r#"{"abiVersion":2,"kind":"featureService","operationId":"read","taskId":"task1-fedcba9876543210fedcba9876543210","generation":7,"state":"completed","result":{"accepted":true}}"#;
const CLOSED_RESPONSE: &str = r#"{"abiVersion":2,"kind":"featureService","operationId":"read","taskId":"task1-fedcba9876543210fedcba9876543210","generation":7,"state":"closed"}"#;
const REJECTION_RESPONSE: &str =
    r#"{"abiVersion":2,"kind":"featureService","state":"error","error":{"code":"invalidRequest"}}"#;
const ASSIGNED_ERROR_RESPONSE: &str = r#"{"abiVersion":2,"kind":"featureService","operationId":"read","taskId":"task1-fedcba9876543210fedcba9876543210","generation":7,"state":"error","error":{"code":"invalidRequest"}}"#;
const POLL_ERROR_RESPONSE: &str = r#"{"abiVersion":2,"kind":"featureService","operationId":"read","taskId":"task1-fedcba9876543210fedcba9876543210","generation":7,"state":"error","error":{"code":"failed"}}"#;
const CANCEL_ERROR_RESPONSE: &str = r#"{"abiVersion":2,"kind":"featureService","operationId":"read","taskId":"task1-fedcba9876543210fedcba9876543210","generation":7,"state":"error","error":{"code":"cancelled"}}"#;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct GoldenRequest {
    attempt: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct GoldenProgress {
    completed_units: u8,
    total_units: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct GoldenResult {
    accepted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct OversizedRequest {
    value: String,
}

macro_rules! task_body {
    ($type:ty) => {
        impl sealed::Sealed for $type {}
        impl FeatureTaskBody for $type {}
    };
}

task_body!(GoldenRequest);
task_body!(GoldenProgress);
task_body!(GoldenResult);
task_body!(OversizedRequest);

fn operation_id() -> OperationId {
    OperationId::parse(OPERATION_ID).expect("operation ID")
}

fn task_id() -> TaskId {
    TaskId::parse(TASK_ID).expect("task ID")
}

fn generation() -> TaskGeneration {
    TaskGeneration::new(7).expect("generation")
}

#[test]
fn current_golden_freezes_ids_task_shapes_rejections_and_assigned_errors() {
    let request =
        FeatureTaskRequest::new(operation_id(), generation(), GoldenRequest { attempt: 1 });
    let control = FeatureTaskControl::new(operation_id(), task_id(), generation());
    let handle = FeatureTaskHandle::new(operation_id(), task_id(), generation());
    let progress = FeatureTaskProgress::new(
        operation_id(),
        task_id(),
        generation(),
        GoldenProgress {
            completed_units: 1,
            total_units: 2,
        },
    );
    let completed = FeatureTaskCompleted::new(
        operation_id(),
        task_id(),
        generation(),
        GoldenResult { accepted: true },
    );
    let closed = FeatureTaskClosed::new(operation_id(), task_id(), generation());
    let mut lines = vec![
        format!(
            r#"{{"package":"{}","world":"{}","featureService":"{}","managerLifecycle":"{}","jsonAbiVersion":{}}}"#,
            MANAGER_WIT_PACKAGE,
            MANAGER_WORLD_ID,
            FEATURE_SERVICE_INTERFACE_ID,
            MANAGER_LIFECYCLE_INTERFACE_ID,
            MANAGER_JSON_ABI_VERSION
        ),
        request.encode().expect("request"),
        control.encode().expect("control"),
        handle.encode().expect("handle"),
        progress.encode().expect("progress"),
        completed.encode().expect("completed"),
        closed.encode().expect("closed"),
    ];
    for code in [
        TaskErrorCode::InvalidRequest,
        TaskErrorCode::CallerMismatch,
        TaskErrorCode::UndeclaredOperation,
        TaskErrorCode::StaleGeneration,
        TaskErrorCode::TaskLimitReached,
        TaskErrorCode::FeatureUnavailable,
        TaskErrorCode::Failed,
    ] {
        lines.push(
            FeatureTaskRejection::new(TaskFailure::new(code))
                .encode()
                .expect("rejection"),
        );
    }
    for code in [TaskErrorCode::UnknownTask, TaskErrorCode::Cancelled] {
        lines.push(
            FeatureTaskError::new(
                operation_id(),
                task_id(),
                generation(),
                TaskFailure::new(code),
            )
            .encode()
            .expect("assigned error"),
        );
    }
    let actual = lines.join("\n") + "\n";

    assert_eq!(actual, include_str!("../goldens/manager_current.jsonl"));
}

#[test]
fn strict_request_decode_rejects_duplicate_unknown_trailing_and_invalid_bytes() {
    let valid = FeatureTaskRequest::new(operation_id(), generation(), GoldenRequest { attempt: 1 })
        .encode()
        .expect("valid request");
    let duplicate = valid.replacen(r#""abiVersion":2"#, r#""abiVersion":2,"abiVersion":2"#, 1);
    let nested_duplicate = valid.replacen(r#""attempt":1"#, r#""attempt":1,"attempt":2"#, 1);
    let unknown = valid.replacen(r#""request""#, r#""unknown":true,"request""#, 1);
    let unknown_body = valid.replacen(r#""attempt":1"#, r#""attempt":1,"unknown":true"#, 1);
    let trailing = format!("{valid}{{}}");
    let trailing_whitespace = format!("{valid} ");

    for bytes in [
        duplicate.as_bytes(),
        nested_duplicate.as_bytes(),
        unknown.as_bytes(),
        trailing.as_bytes(),
        trailing_whitespace.as_bytes(),
    ] {
        assert_eq!(
            decode_feature_task_request::<GoldenRequest>(bytes, |_| Ok(())),
            Err(TaskWireError::InvalidDocument)
        );
    }
    assert_eq!(
        decode_feature_task_request::<GoldenRequest>(unknown_body.as_bytes(), |_| Ok(())),
        Err(TaskWireError::InvalidBody)
    );
    assert_eq!(
        decode_feature_task_request::<GoldenRequest>(&[0xff], |_| Ok(())),
        Err(TaskWireError::InvalidEncoding)
    );
    let oversized = vec![b' '; MAX_MANAGER_TASK_WIRE_BYTES + 1];
    assert_eq!(
        decode_feature_task_request::<GoldenRequest>(&oversized, |_| Ok(())),
        Err(TaskWireError::TooLarge)
    );
    let oversized_request = FeatureTaskRequest::new(
        operation_id(),
        generation(),
        OversizedRequest {
            value: "x".repeat(MAX_MANAGER_TASK_WIRE_BYTES),
        },
    );
    assert_eq!(oversized_request.encode(), Err(TaskWireError::TooLarge));
}

#[test]
fn task_control_round_trips_and_rejects_unknown_fields() {
    let control = FeatureTaskControl::new(operation_id(), task_id(), generation());
    let wire = control.encode().expect("control");
    assert_eq!(
        FeatureTaskControl::decode(wire.as_bytes()).expect("decoded control"),
        control
    );

    let unknown = wire.replacen(r#""generation":7"#, r#""generation":7,"unknown":true"#, 1);
    assert_eq!(
        FeatureTaskControl::decode(unknown.as_bytes()),
        Err(TaskWireError::InvalidDocument)
    );
}

#[test]
fn unassigned_rejection_is_start_only() {
    let FeatureTaskStart::Rejected(rejection) =
        FeatureTaskStart::decode(REJECTION_RESPONSE.as_bytes()).expect("start rejection")
    else {
        panic!("start rejection decoded as a handle");
    };
    assert_eq!(rejection.error().code(), TaskErrorCode::InvalidRequest);
    assert_eq!(
        FeatureTaskUpdate::<GoldenProgress, GoldenResult>::decode(REJECTION_RESPONSE.as_bytes()),
        Err(TaskWireError::InvalidDocument)
    );
    assert_eq!(
        FeatureTaskTerminal::decode(REJECTION_RESPONSE.as_bytes()),
        Err(TaskWireError::InvalidDocument)
    );
}

#[test]
fn start_decoder_rejects_assigned_task_error() {
    assert_eq!(
        FeatureTaskStart::decode(ASSIGNED_ERROR_RESPONSE.as_bytes()),
        Err(TaskWireError::InvalidDocument)
    );
}

#[test]
fn poll_decoder_accepts_each_literal_state() {
    type Update = FeatureTaskUpdate<GoldenProgress, GoldenResult>;

    let Update::Open(open) = Update::decode(OPEN_RESPONSE.as_bytes()).expect("open update") else {
        panic!("open response decoded as another state");
    };
    assert_eq!(open.operation_id(), &operation_id());
    assert_eq!(open.task_id(), &task_id());
    assert_eq!(open.generation(), generation());

    let Update::Progress(progress) =
        Update::decode(PROGRESS_RESPONSE.as_bytes()).expect("progress update")
    else {
        panic!("progress response decoded as another state");
    };
    assert_eq!(progress.operation_id(), &operation_id());
    assert_eq!(progress.task_id(), &task_id());
    assert_eq!(progress.generation(), generation());
    assert_eq!(
        progress.progress(),
        &GoldenProgress {
            completed_units: 1,
            total_units: 2,
        }
    );

    let Update::Completed(completed) =
        Update::decode(COMPLETED_RESPONSE.as_bytes()).expect("completed update")
    else {
        panic!("completed response decoded as another state");
    };
    assert_eq!(completed.operation_id(), &operation_id());
    assert_eq!(completed.task_id(), &task_id());
    assert_eq!(completed.generation(), generation());
    assert_eq!(completed.result(), &GoldenResult { accepted: true });

    let Update::Closed(closed) = Update::decode(CLOSED_RESPONSE.as_bytes()).expect("closed update")
    else {
        panic!("closed response decoded as another state");
    };
    assert_eq!(closed.operation_id(), &operation_id());
    assert_eq!(closed.task_id(), &task_id());
    assert_eq!(closed.generation(), generation());

    let Update::Error(error) =
        Update::decode(POLL_ERROR_RESPONSE.as_bytes()).expect("error update")
    else {
        panic!("error response decoded as another state");
    };
    assert_eq!(error.operation_id(), &operation_id());
    assert_eq!(error.task_id(), &task_id());
    assert_eq!(error.generation(), generation());
    assert_eq!(error.error().code(), TaskErrorCode::Failed);
}

#[test]
fn cancel_decoder_accepts_each_literal_terminal_state() {
    let FeatureTaskTerminal::Closed(closed) =
        FeatureTaskTerminal::decode(CLOSED_RESPONSE.as_bytes()).expect("cancel closed")
    else {
        panic!("closed cancel response decoded as an error");
    };
    assert_eq!(closed.operation_id(), &operation_id());
    assert_eq!(closed.task_id(), &task_id());
    assert_eq!(closed.generation(), generation());

    let FeatureTaskTerminal::Error(error) =
        FeatureTaskTerminal::decode(CANCEL_ERROR_RESPONSE.as_bytes()).expect("cancel error")
    else {
        panic!("error cancel response decoded as closed");
    };
    assert_eq!(error.operation_id(), &operation_id());
    assert_eq!(error.task_id(), &task_id());
    assert_eq!(error.generation(), generation());
    assert_eq!(error.error().code(), TaskErrorCode::Cancelled);
}

#[test]
fn poll_decoder_rejects_invalid_body_placement() {
    type Update = FeatureTaskUpdate<GoldenProgress, GoldenResult>;

    let invalid = [
        r#"{"abiVersion":2,"kind":"featureService","operationId":"read","taskId":"task1-fedcba9876543210fedcba9876543210","generation":7,"state":"progress"}"#,
        r#"{"abiVersion":2,"kind":"featureService","operationId":"read","taskId":"task1-fedcba9876543210fedcba9876543210","generation":7,"state":"completed"}"#,
        r#"{"abiVersion":2,"kind":"featureService","operationId":"read","taskId":"task1-fedcba9876543210fedcba9876543210","generation":7,"state":"error"}"#,
        r#"{"abiVersion":2,"kind":"featureService","operationId":"read","taskId":"task1-fedcba9876543210fedcba9876543210","generation":7,"state":"progress","result":{"completedUnits":1,"totalUnits":2}}"#,
        r#"{"abiVersion":2,"kind":"featureService","operationId":"read","taskId":"task1-fedcba9876543210fedcba9876543210","generation":7,"state":"completed","progress":{"accepted":true}}"#,
        r#"{"abiVersion":2,"kind":"featureService","operationId":"read","taskId":"task1-fedcba9876543210fedcba9876543210","generation":7,"state":"progress","error":{"code":"failed"}}"#,
        r#"{"abiVersion":2,"kind":"featureService","operationId":"read","taskId":"task1-fedcba9876543210fedcba9876543210","generation":7,"state":"error","error":{"code":"failed"},"progress":{"completedUnits":1,"totalUnits":2}}"#,
        r#"{"abiVersion":2,"kind":"featureService","operationId":"read","taskId":"task1-fedcba9876543210fedcba9876543210","generation":7,"state":"error","error":{"code":"failed"},"result":{"accepted":true}}"#,
        r#"{"abiVersion":2,"kind":"featureService","operationId":"read","taskId":"task1-fedcba9876543210fedcba9876543210","generation":7,"state":"progress","progress":{"completedUnits":1,"totalUnits":2},"result":{"accepted":true}}"#,
        r#"{"abiVersion":2,"kind":"featureService","operationId":"read","taskId":"task1-fedcba9876543210fedcba9876543210","generation":7,"state":"completed","progress":{"completedUnits":1,"totalUnits":2},"result":{"accepted":true}}"#,
    ];

    for document in invalid {
        assert_eq!(
            Update::decode(document.as_bytes()),
            Err(TaskWireError::InvalidDocument),
            "{document}"
        );
    }
}

#[test]
fn poll_decoder_rejects_crossed_explicit_null_fields() {
    type Update = FeatureTaskUpdate<GoldenProgress, GoldenResult>;

    for document in [
        r#"{"abiVersion":2,"kind":"featureService","operationId":"read","taskId":"task1-fedcba9876543210fedcba9876543210","generation":7,"state":"progress","progress":{"completedUnits":1,"totalUnits":2},"result":null}"#,
        r#"{"abiVersion":2,"kind":"featureService","operationId":"read","taskId":"task1-fedcba9876543210fedcba9876543210","generation":7,"state":"completed","progress":null,"result":{"accepted":true}}"#,
    ] {
        assert_eq!(
            Update::decode(document.as_bytes()),
            Err(TaskWireError::InvalidDocument),
            "{document}"
        );
    }
}

static BODY_DECODES: AtomicUsize = AtomicUsize::new(0);

#[derive(Debug, PartialEq, Eq, Serialize)]
struct DecodeSpy;

impl<'de> Deserialize<'de> for DecodeSpy {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        BODY_DECODES.fetch_add(1, Ordering::AcqRel);
        let _ = GoldenRequest::deserialize(deserializer)
            .map_err(|_| D::Error::custom("spy body rejected"))?;
        Ok(Self)
    }
}

task_body!(DecodeSpy);

static DECLARED_BODY_DECODES: AtomicUsize = AtomicUsize::new(0);

#[derive(Debug, PartialEq, Eq, Serialize)]
struct DeclaredDecodeSpy;

impl<'de> Deserialize<'de> for DeclaredDecodeSpy {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        DECLARED_BODY_DECODES.fetch_add(1, Ordering::AcqRel);
        let _ = GoldenRequest::deserialize(deserializer)
            .map_err(|_| D::Error::custom("declared spy body rejected"))?;
        Ok(Self)
    }
}

task_body!(DeclaredDecodeSpy);

static PACK_CALLS: AtomicUsize = AtomicUsize::new(0);
static TRANSPORT_CALLS: AtomicUsize = AtomicUsize::new(0);

fn dispatch_declared(
    document: &str,
    declared_operations: &[OperationId],
) -> Result<(), TaskWireError> {
    decode_feature_task_request::<DeclaredDecodeSpy>(document.as_bytes(), |metadata| {
        validate_declared_operation(declared_operations, metadata.operation_id())
    })?;
    PACK_CALLS.fetch_add(1, Ordering::AcqRel);
    TRANSPORT_CALLS.fetch_add(1, Ordering::AcqRel);
    Ok(())
}

#[test]
fn declared_operation_gate_precedes_body_pack_and_transport() {
    let read = OperationId::parse("read").expect("read operation");
    let read_request = r#"{"abiVersion":2,"kind":"featureService","operationId":"read","generation":7,"request":{"attempt":1}}"#;
    let write_request = r#"{"abiVersion":2,"kind":"featureService","operationId":"write","generation":7,"request":{"attempt":1}}"#;

    DECLARED_BODY_DECODES.store(0, Ordering::Release);
    PACK_CALLS.store(0, Ordering::Release);
    TRANSPORT_CALLS.store(0, Ordering::Release);
    dispatch_declared(read_request, std::slice::from_ref(&read)).expect("declared read");
    assert_eq!(DECLARED_BODY_DECODES.load(Ordering::Acquire), 1);
    assert_eq!(PACK_CALLS.load(Ordering::Acquire), 1);
    assert_eq!(TRANSPORT_CALLS.load(Ordering::Acquire), 1);

    DECLARED_BODY_DECODES.store(0, Ordering::Release);
    PACK_CALLS.store(0, Ordering::Release);
    TRANSPORT_CALLS.store(0, Ordering::Release);
    assert_eq!(
        dispatch_declared(write_request, std::slice::from_ref(&read)),
        Err(TaskWireError::BindingRejected(
            TaskErrorCode::UndeclaredOperation
        ))
    );
    assert_eq!(DECLARED_BODY_DECODES.load(Ordering::Acquire), 0);
    assert_eq!(PACK_CALLS.load(Ordering::Acquire), 0);
    assert_eq!(TRANSPORT_CALLS.load(Ordering::Acquire), 0);

    let over_limit = vec![read; MAX_DECLARED_OPERATIONS + 1];
    assert_eq!(
        dispatch_declared(read_request, &over_limit),
        Err(TaskWireError::BindingRejected(
            TaskErrorCode::UndeclaredOperation
        ))
    );
    assert_eq!(DECLARED_BODY_DECODES.load(Ordering::Acquire), 0);
    assert_eq!(PACK_CALLS.load(Ordering::Acquire), 0);
    assert_eq!(TRANSPORT_CALLS.load(Ordering::Acquire), 0);
}

#[test]
fn binding_rejection_precedes_family_body_deserialization() {
    BODY_DECODES.store(0, Ordering::Release);
    let wire = FeatureTaskRequest::new(operation_id(), generation(), GoldenRequest { attempt: 1 })
        .encode()
        .expect("wire");

    assert_eq!(
        decode_feature_task_request::<DecodeSpy>(wire.as_bytes(), |_| {
            Err(TaskErrorCode::CallerMismatch)
        }),
        Err(TaskWireError::BindingRejected(
            TaskErrorCode::CallerMismatch
        ))
    );
    assert_eq!(BODY_DECODES.load(Ordering::Acquire), 0);

    decode_feature_task_request::<DecodeSpy>(wire.as_bytes(), |_| Ok(())).expect("bound decode");
    assert_eq!(BODY_DECODES.load(Ordering::Acquire), 1);
}

#[test]
fn start_decoder_distinguishes_handle_from_progress() {
    let handle = FeatureTaskHandle::new(operation_id(), task_id(), generation())
        .encode()
        .expect("handle");
    assert!(matches!(
        FeatureTaskStart::decode(handle.as_bytes()).expect("start"),
        FeatureTaskStart::Handle(_)
    ));

    assert_eq!(
        FeatureTaskStart::decode(PROGRESS_RESPONSE.as_bytes()),
        Err(TaskWireError::InvalidDocument)
    );
}

//! Strict strongly typed FeatureService task envelopes.

// Rust guideline compliant 2026-08-29.

use std::io::{self, Write};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use crate::identity::{OperationId, TaskErrorCode, TaskFailure, TaskGeneration, TaskId};
use crate::{MANAGER_JSON_ABI_VERSION, strict_json};

/// Maximum operations declared by one active Manager.
pub const MAX_DECLARED_OPERATIONS: usize = 128;
/// Maximum encoded bytes accepted for one Manager task message.
pub const MAX_MANAGER_TASK_WIRE_BYTES: usize = 64 * 1024;

mod sealed {
    pub trait Sealed {}
}

/// Marks a family-specific typed task request, progress, or result body.
///
/// This trait is sealed. Only concrete family DTOs published by this crate can
/// cross the task boundary; generic JSON values and downstream opaque wrappers
/// cannot implement it.
///
/// ```compile_fail
/// use mcode_plugin_api::FeatureTaskRequest;
///
/// fn raw_json_is_not_a_task_body(_: FeatureTaskRequest<serde_json::Value>) {}
/// ```
pub trait FeatureTaskBody: sealed::Sealed + Serialize + DeserializeOwned {}

macro_rules! identity_accessors {
    () => {
        /// Returns the canonical operation ID.
        #[must_use]
        pub const fn operation_id(&self) -> &OperationId {
            &self.operation_id
        }

        /// Returns the Host-issued task ID.
        #[must_use]
        pub const fn task_id(&self) -> &TaskId {
            &self.task_id
        }

        /// Returns the bound Manager generation.
        #[must_use]
        pub const fn generation(&self) -> TaskGeneration {
            self.generation
        }
    };
}

/// Fixed FeatureService task state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TaskState {
    /// A task was accepted and remains open.
    Open,
    /// A task emitted typed progress.
    Progress,
    /// A task produced its typed result.
    Completed,
    /// A task is closed without a result body.
    Closed,
    /// A request was rejected or an assigned task closed with a stable code.
    Error,
}

/// Exposes validated request metadata before family body decoding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskRequestMetadata {
    operation_id: OperationId,
    generation: TaskGeneration,
}

impl TaskRequestMetadata {
    /// Returns the canonical operation ID.
    #[must_use]
    pub const fn operation_id(&self) -> &OperationId {
        &self.operation_id
    }

    /// Returns the request generation.
    #[must_use]
    pub const fn generation(&self) -> TaskGeneration {
        self.generation
    }
}

/// Validates one operation against a bounded Host declaration slice.
///
/// Over-limit declarations and absent keys are indistinguishable and fail
/// closed without allocating.
///
/// # Errors
///
/// Returns [`TaskErrorCode::UndeclaredOperation`] when the declaration slice
/// exceeds [`MAX_DECLARED_OPERATIONS`] or does not contain `operation_id`.
pub fn validate_declared_operation(
    declared_operations: &[OperationId],
    operation_id: &OperationId,
) -> Result<(), TaskErrorCode> {
    if declared_operations.len() > MAX_DECLARED_OPERATIONS
        || !declared_operations
            .iter()
            .any(|declared| declared == operation_id)
    {
        return Err(TaskErrorCode::UndeclaredOperation);
    }
    Ok(())
}

/// Contains one typed FeatureService start request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeatureTaskRequest<B: FeatureTaskBody> {
    operation_id: OperationId,
    generation: TaskGeneration,
    request: B,
}

impl<B: FeatureTaskBody> FeatureTaskRequest<B> {
    /// Creates one typed task request.
    #[must_use]
    pub const fn new(operation_id: OperationId, generation: TaskGeneration, request: B) -> Self {
        Self {
            operation_id,
            generation,
            request,
        }
    }

    /// Returns the canonical operation ID.
    #[must_use]
    pub const fn operation_id(&self) -> &OperationId {
        &self.operation_id
    }

    /// Returns the request generation.
    #[must_use]
    pub const fn generation(&self) -> TaskGeneration {
        self.generation
    }

    /// Returns the family-specific request body.
    #[must_use]
    pub const fn request(&self) -> &B {
        &self.request
    }

    /// Consumes the envelope and returns its typed request body.
    #[must_use]
    pub fn into_request(self) -> B {
        self.request
    }

    /// Encodes this request as bounded canonical task JSON.
    ///
    /// # Errors
    ///
    /// Returns [`TaskWireError::EncodeFailed`] when body serialization fails,
    /// or [`TaskWireError::TooLarge`] when the encoded message exceeds its
    /// bound.
    pub fn encode(&self) -> Result<String, TaskWireError> {
        encode(&RequestRef {
            abi_version: MANAGER_JSON_ABI_VERSION,
            kind: WireKind::FeatureService,
            operation_id: &self.operation_id,
            generation: self.generation,
            request: &self.request,
        })
    }
}

/// Decodes a start request after the Host binds caller identity.
///
/// The `bind` callback receives only validated operation metadata. It runs
/// before `B` is deserialized, so family and generation rejection cannot enter
/// a family body decoder.
///
/// # Errors
///
/// Returns [`TaskWireError`] for bounds, encoding, structure, body, or binding
/// failures.
pub fn decode_feature_task_request<B>(
    bytes: &[u8],
    bind: impl FnOnce(&TaskRequestMetadata) -> Result<(), TaskErrorCode>,
) -> Result<FeatureTaskRequest<B>, TaskWireError>
where
    B: FeatureTaskBody,
{
    let RequestCarrier {
        abi_version,
        kind,
        operation_id,
        generation,
        request,
    } = decode_value(bytes)?;
    validate_header(abi_version, kind)?;
    let metadata = TaskRequestMetadata {
        operation_id,
        generation,
    };
    bind(&metadata).map_err(TaskWireError::BindingRejected)?;
    let request = serde_json::from_value(request).map_err(|_| TaskWireError::InvalidBody)?;
    Ok(FeatureTaskRequest::new(
        metadata.operation_id,
        metadata.generation,
        request,
    ))
}

/// Contains one accepted Host-issued task handle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeatureTaskHandle {
    operation_id: OperationId,
    task_id: TaskId,
    generation: TaskGeneration,
}

impl FeatureTaskHandle {
    /// Creates one open task handle.
    #[must_use]
    pub const fn new(
        operation_id: OperationId,
        task_id: TaskId,
        generation: TaskGeneration,
    ) -> Self {
        Self {
            operation_id,
            task_id,
            generation,
        }
    }

    identity_accessors!();

    /// Encodes this open handle.
    ///
    /// # Errors
    ///
    /// Returns [`TaskWireError`] when serialization fails or exceeds its bound.
    pub fn encode(&self) -> Result<String, TaskWireError> {
        encode(&StateRef::<()> {
            abi_version: MANAGER_JSON_ABI_VERSION,
            kind: WireKind::FeatureService,
            operation_id: &self.operation_id,
            task_id: &self.task_id,
            generation: self.generation,
            state: TaskState::Open,
            body: None,
        })
    }

    fn decode_value(value: Value) -> Result<Self, TaskWireError> {
        let wire: EmptyStateWire = from_value(value)?;
        validate_state_header(&wire, TaskState::Open)?;
        Ok(Self::new(wire.operation_id, wire.task_id, wire.generation))
    }
}

/// Identifies one task for polling or cancellation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeatureTaskControl {
    operation_id: OperationId,
    task_id: TaskId,
    generation: TaskGeneration,
}

impl FeatureTaskControl {
    /// Creates one task control request.
    #[must_use]
    pub const fn new(
        operation_id: OperationId,
        task_id: TaskId,
        generation: TaskGeneration,
    ) -> Self {
        Self {
            operation_id,
            task_id,
            generation,
        }
    }

    identity_accessors!();

    /// Encodes this task control request.
    ///
    /// # Errors
    ///
    /// Returns [`TaskWireError`] when serialization fails or exceeds its bound.
    pub fn encode(&self) -> Result<String, TaskWireError> {
        encode(&ControlRef {
            abi_version: MANAGER_JSON_ABI_VERSION,
            kind: WireKind::FeatureService,
            operation_id: &self.operation_id,
            task_id: &self.task_id,
            generation: self.generation,
        })
    }

    /// Strictly decodes one task control request.
    ///
    /// # Errors
    ///
    /// Returns [`TaskWireError`] for oversized, malformed, duplicate, unknown,
    /// trailing, or invalid fields.
    pub fn decode(bytes: &[u8]) -> Result<Self, TaskWireError> {
        let wire: ControlWire = decode_value(bytes)?;
        validate_header(wire.abi_version, wire.kind)?;
        Ok(Self::new(wire.operation_id, wire.task_id, wire.generation))
    }
}

/// Contains one typed task progress update.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeatureTaskProgress<P: FeatureTaskBody> {
    operation_id: OperationId,
    task_id: TaskId,
    generation: TaskGeneration,
    progress: P,
}

impl<P: FeatureTaskBody> FeatureTaskProgress<P> {
    /// Creates one typed progress update.
    #[must_use]
    pub const fn new(
        operation_id: OperationId,
        task_id: TaskId,
        generation: TaskGeneration,
        progress: P,
    ) -> Self {
        Self {
            operation_id,
            task_id,
            generation,
            progress,
        }
    }

    identity_accessors!();

    /// Returns the family-specific progress body.
    #[must_use]
    pub const fn progress(&self) -> &P {
        &self.progress
    }

    /// Encodes this typed progress update.
    ///
    /// # Errors
    ///
    /// Returns [`TaskWireError`] when body serialization fails or exceeds its
    /// bound.
    pub fn encode(&self) -> Result<String, TaskWireError> {
        encode(&StateRef {
            abi_version: MANAGER_JSON_ABI_VERSION,
            kind: WireKind::FeatureService,
            operation_id: &self.operation_id,
            task_id: &self.task_id,
            generation: self.generation,
            state: TaskState::Progress,
            body: Some(NamedBody::Progress(&self.progress)),
        })
    }
}

/// Contains one typed completed task result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeatureTaskCompleted<R: FeatureTaskBody> {
    operation_id: OperationId,
    task_id: TaskId,
    generation: TaskGeneration,
    result: R,
}

impl<R: FeatureTaskBody> FeatureTaskCompleted<R> {
    /// Creates one typed completed result.
    #[must_use]
    pub const fn new(
        operation_id: OperationId,
        task_id: TaskId,
        generation: TaskGeneration,
        result: R,
    ) -> Self {
        Self {
            operation_id,
            task_id,
            generation,
            result,
        }
    }

    identity_accessors!();

    /// Returns the family-specific result body.
    #[must_use]
    pub const fn result(&self) -> &R {
        &self.result
    }

    /// Encodes this typed completed result.
    ///
    /// # Errors
    ///
    /// Returns [`TaskWireError`] when body serialization fails or exceeds its
    /// bound.
    pub fn encode(&self) -> Result<String, TaskWireError> {
        encode(&StateRef {
            abi_version: MANAGER_JSON_ABI_VERSION,
            kind: WireKind::FeatureService,
            operation_id: &self.operation_id,
            task_id: &self.task_id,
            generation: self.generation,
            state: TaskState::Completed,
            body: Some(NamedBody::Result(&self.result)),
        })
    }
}

/// Contains one closed task identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeatureTaskClosed {
    operation_id: OperationId,
    task_id: TaskId,
    generation: TaskGeneration,
}

impl FeatureTaskClosed {
    /// Creates one closed task response.
    #[must_use]
    pub const fn new(
        operation_id: OperationId,
        task_id: TaskId,
        generation: TaskGeneration,
    ) -> Self {
        Self {
            operation_id,
            task_id,
            generation,
        }
    }

    identity_accessors!();

    /// Encodes this closed task response.
    ///
    /// # Errors
    ///
    /// Returns [`TaskWireError`] when serialization fails or exceeds its bound.
    pub fn encode(&self) -> Result<String, TaskWireError> {
        encode(&StateRef::<()> {
            abi_version: MANAGER_JSON_ABI_VERSION,
            kind: WireKind::FeatureService,
            operation_id: &self.operation_id,
            task_id: &self.task_id,
            generation: self.generation,
            state: TaskState::Closed,
            body: None,
        })
    }

    fn decode_value(value: Value) -> Result<Self, TaskWireError> {
        let wire: EmptyStateWire = from_value(value)?;
        validate_state_header(&wire, TaskState::Closed)?;
        Ok(Self::new(wire.operation_id, wire.task_id, wire.generation))
    }
}

/// Contains a stable rejection emitted before the Host allocates a task.
///
/// This shape intentionally carries no operation, task, or generation
/// identity because none was accepted for execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FeatureTaskRejection {
    error: TaskFailure,
}

impl FeatureTaskRejection {
    /// Creates one unassigned task rejection.
    #[must_use]
    pub const fn new(error: TaskFailure) -> Self {
        Self { error }
    }

    /// Returns the stable non-sensitive failure.
    #[must_use]
    pub const fn error(self) -> TaskFailure {
        self.error
    }

    /// Encodes this unassigned rejection.
    ///
    /// # Errors
    ///
    /// Returns [`TaskWireError`] when serialization fails or exceeds its bound.
    pub fn encode(self) -> Result<String, TaskWireError> {
        encode(&RejectionWire {
            abi_version: MANAGER_JSON_ABI_VERSION,
            kind: WireKind::FeatureService,
            state: TaskState::Error,
            error: self.error,
        })
    }

    fn decode_value(value: Value) -> Result<Self, TaskWireError> {
        let wire: RejectionWire = from_value(value)?;
        validate_header(wire.abi_version, wire.kind)?;
        if wire.state != TaskState::Error {
            return Err(TaskWireError::InvalidDocument);
        }
        Ok(Self::new(wire.error))
    }
}

/// Contains one stable error for a Host-issued task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeatureTaskError {
    operation_id: OperationId,
    task_id: TaskId,
    generation: TaskGeneration,
    error: TaskFailure,
}

impl FeatureTaskError {
    /// Creates one error response for an assigned task.
    #[must_use]
    pub const fn new(
        operation_id: OperationId,
        task_id: TaskId,
        generation: TaskGeneration,
        error: TaskFailure,
    ) -> Self {
        Self {
            operation_id,
            task_id,
            generation,
            error,
        }
    }

    identity_accessors!();

    /// Returns the stable non-sensitive failure.
    #[must_use]
    pub const fn error(&self) -> TaskFailure {
        self.error
    }

    /// Encodes this task error response.
    ///
    /// # Errors
    ///
    /// Returns [`TaskWireError`] when serialization fails or exceeds its bound.
    pub fn encode(&self) -> Result<String, TaskWireError> {
        encode(&ErrorRef {
            abi_version: MANAGER_JSON_ABI_VERSION,
            kind: WireKind::FeatureService,
            operation_id: &self.operation_id,
            task_id: &self.task_id,
            generation: self.generation,
            state: TaskState::Error,
            error: self.error,
        })
    }

    fn decode_value(value: Value) -> Result<Self, TaskWireError> {
        let wire: ErrorWire = from_value(value)?;
        validate_header(wire.abi_version, wire.kind)?;
        if wire.state != TaskState::Error {
            return Err(TaskWireError::InvalidDocument);
        }
        Ok(Self::new(
            wire.operation_id,
            wire.task_id,
            wire.generation,
            wire.error,
        ))
    }
}

/// Start-task response shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FeatureTaskStart {
    /// The task was accepted and assigned a Host-issued identity.
    Handle(FeatureTaskHandle),
    /// The request was rejected before task allocation.
    Rejected(FeatureTaskRejection),
}

impl FeatureTaskStart {
    /// Strictly decodes one start-task response.
    ///
    /// # Errors
    ///
    /// Returns [`TaskWireError`] when the response is not exactly an open
    /// handle or unassigned rejection shape.
    pub fn decode(bytes: &[u8]) -> Result<Self, TaskWireError> {
        let value = parse_value(bytes)?;
        match state(&value)? {
            TaskState::Open => FeatureTaskHandle::decode_value(value).map(Self::Handle),
            TaskState::Error => FeatureTaskRejection::decode_value(value).map(Self::Rejected),
            _ => Err(TaskWireError::InvalidDocument),
        }
    }
}

/// Poll-task response shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FeatureTaskUpdate<P: FeatureTaskBody, R: FeatureTaskBody> {
    /// The task remains open without a new progress body.
    Open(FeatureTaskHandle),
    /// The task emitted typed progress.
    Progress(FeatureTaskProgress<P>),
    /// The task completed with a typed result.
    Completed(FeatureTaskCompleted<R>),
    /// The task closed without a result.
    Closed(FeatureTaskClosed),
    /// The task closed with a stable failure.
    Error(FeatureTaskError),
}

impl<P: FeatureTaskBody, R: FeatureTaskBody> FeatureTaskUpdate<P, R> {
    /// Strictly decodes one poll-task response.
    ///
    /// # Errors
    ///
    /// Returns [`TaskWireError`] for any non-current or invalid response.
    pub fn decode(bytes: &[u8]) -> Result<Self, TaskWireError> {
        let value = parse_value(bytes)?;
        match state(&value)? {
            TaskState::Open => FeatureTaskHandle::decode_value(value).map(Self::Open),
            TaskState::Progress => decode_typed_state(value, TaskState::Progress)
                .map(|(operation_id, task_id, generation, body)| {
                    FeatureTaskProgress::new(operation_id, task_id, generation, body)
                })
                .map(Self::Progress),
            TaskState::Completed => decode_typed_state(value, TaskState::Completed)
                .map(|(operation_id, task_id, generation, body)| {
                    FeatureTaskCompleted::new(operation_id, task_id, generation, body)
                })
                .map(Self::Completed),
            TaskState::Closed => FeatureTaskClosed::decode_value(value).map(Self::Closed),
            TaskState::Error => FeatureTaskError::decode_value(value).map(Self::Error),
        }
    }
}

/// Cancel-task response shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FeatureTaskTerminal {
    /// The task is closed.
    Closed(FeatureTaskClosed),
    /// Cancellation failed with a stable code.
    Error(FeatureTaskError),
}

impl FeatureTaskTerminal {
    /// Strictly decodes one cancel-task response.
    ///
    /// # Errors
    ///
    /// Returns [`TaskWireError`] unless the response is exactly closed or
    /// error.
    pub fn decode(bytes: &[u8]) -> Result<Self, TaskWireError> {
        let value = parse_value(bytes)?;
        match state(&value)? {
            TaskState::Closed => FeatureTaskClosed::decode_value(value).map(Self::Closed),
            TaskState::Error => FeatureTaskError::decode_value(value).map(Self::Error),
            _ => Err(TaskWireError::InvalidDocument),
        }
    }
}

/// Reports strict Manager task-wire failures without retaining input text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum TaskWireError {
    /// The encoded message exceeded its fixed byte bound.
    #[error("Manager task message exceeds its size limit")]
    TooLarge,
    /// Input bytes were not UTF-8.
    #[error("Manager task message is not UTF-8")]
    InvalidEncoding,
    /// The envelope was duplicate, unknown, trailing, malformed, or non-current.
    #[error("Manager task message is invalid")]
    InvalidDocument,
    /// The family-specific body failed strict typed decoding.
    #[error("Manager task body is invalid")]
    InvalidBody,
    /// The Host rejected caller identity before body decoding.
    #[error("Manager task caller binding was rejected")]
    BindingRejected(TaskErrorCode),
    /// Typed body serialization failed without exposing its source error.
    #[error("Manager task message could not be encoded")]
    EncodeFailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
enum WireKind {
    FeatureService,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RequestRef<'a, B> {
    abi_version: u16,
    kind: WireKind,
    operation_id: &'a OperationId,
    generation: TaskGeneration,
    request: &'a B,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RequestCarrier {
    abi_version: u16,
    kind: WireKind,
    operation_id: OperationId,
    generation: TaskGeneration,
    request: Value,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ControlRef<'a> {
    abi_version: u16,
    kind: WireKind,
    operation_id: &'a OperationId,
    task_id: &'a TaskId,
    generation: TaskGeneration,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ControlWire {
    abi_version: u16,
    kind: WireKind,
    operation_id: OperationId,
    task_id: TaskId,
    generation: TaskGeneration,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StateRef<'a, B> {
    abi_version: u16,
    kind: WireKind,
    operation_id: &'a OperationId,
    task_id: &'a TaskId,
    generation: TaskGeneration,
    state: TaskState,
    #[serde(flatten, skip_serializing_if = "Option::is_none")]
    body: Option<NamedBody<'a, B>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
enum NamedBody<'a, B> {
    Progress(&'a B),
    Result(&'a B),
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EmptyStateWire {
    abi_version: u16,
    kind: WireKind,
    operation_id: OperationId,
    task_id: TaskId,
    generation: TaskGeneration,
    state: TaskState,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TypedStateCarrier {
    abi_version: u16,
    kind: WireKind,
    operation_id: OperationId,
    task_id: TaskId,
    generation: TaskGeneration,
    state: TaskState,
    #[serde(default, deserialize_with = "deserialize_present_value")]
    progress: Option<Value>,
    #[serde(default, deserialize_with = "deserialize_present_value")]
    result: Option<Value>,
}

fn deserialize_present_value<'de, D>(deserializer: D) -> Result<Option<Value>, D::Error>
where
    D: Deserializer<'de>,
{
    Value::deserialize(deserializer).map(Some)
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RejectionWire {
    abi_version: u16,
    kind: WireKind,
    state: TaskState,
    error: TaskFailure,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ErrorRef<'a> {
    abi_version: u16,
    kind: WireKind,
    operation_id: &'a OperationId,
    task_id: &'a TaskId,
    generation: TaskGeneration,
    state: TaskState,
    error: TaskFailure,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ErrorWire {
    abi_version: u16,
    kind: WireKind,
    operation_id: OperationId,
    task_id: TaskId,
    generation: TaskGeneration,
    state: TaskState,
    error: TaskFailure,
}

fn validate_header(abi_version: u16, kind: WireKind) -> Result<(), TaskWireError> {
    if abi_version != MANAGER_JSON_ABI_VERSION || kind != WireKind::FeatureService {
        return Err(TaskWireError::InvalidDocument);
    }
    Ok(())
}

fn validate_state_header(wire: &EmptyStateWire, expected: TaskState) -> Result<(), TaskWireError> {
    validate_header(wire.abi_version, wire.kind)?;
    if wire.state != expected {
        return Err(TaskWireError::InvalidDocument);
    }
    Ok(())
}

fn decode_typed_state<B>(
    value: Value,
    expected: TaskState,
) -> Result<(OperationId, TaskId, TaskGeneration, B), TaskWireError>
where
    B: DeserializeOwned,
{
    let carrier: TypedStateCarrier = from_value(value)?;
    validate_header(carrier.abi_version, carrier.kind)?;
    if carrier.state != expected {
        return Err(TaskWireError::InvalidDocument);
    }
    let body = match expected {
        TaskState::Progress if carrier.result.is_none() => carrier.progress,
        TaskState::Completed if carrier.progress.is_none() => carrier.result,
        _ => return Err(TaskWireError::InvalidDocument),
    }
    .ok_or(TaskWireError::InvalidDocument)?;
    let body = serde_json::from_value(body).map_err(|_| TaskWireError::InvalidBody)?;
    Ok((
        carrier.operation_id,
        carrier.task_id,
        carrier.generation,
        body,
    ))
}

fn state(value: &Value) -> Result<TaskState, TaskWireError> {
    let Some(state) = value.get("state") else {
        return Err(TaskWireError::InvalidDocument);
    };
    serde_json::from_value(state.clone()).map_err(|_| TaskWireError::InvalidDocument)
}

fn encode<T: Serialize>(value: &T) -> Result<String, TaskWireError> {
    let mut writer = BoundedWriter::default();
    if serde_json::to_writer(&mut writer, value).is_err() {
        return if writer.overflowed {
            Err(TaskWireError::TooLarge)
        } else {
            Err(TaskWireError::EncodeFailed)
        };
    }
    String::from_utf8(writer.bytes).map_err(|_| TaskWireError::EncodeFailed)
}

#[derive(Default)]
struct BoundedWriter {
    bytes: Vec<u8>,
    overflowed: bool,
}

impl Write for BoundedWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let remaining = MAX_MANAGER_TASK_WIRE_BYTES - self.bytes.len();
        if bytes.len() > remaining {
            self.overflowed = true;
            return Err(io::Error::other(
                "Manager task message exceeds its size limit",
            ));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn parse_value(bytes: &[u8]) -> Result<Value, TaskWireError> {
    if bytes.len() > MAX_MANAGER_TASK_WIRE_BYTES {
        return Err(TaskWireError::TooLarge);
    }
    if std::str::from_utf8(bytes).is_err() {
        return Err(TaskWireError::InvalidEncoding);
    }
    if bytes.first() != Some(&b'{') || bytes.last() != Some(&b'}') {
        return Err(TaskWireError::InvalidDocument);
    }
    strict_json::parse(bytes).map_err(|_| TaskWireError::InvalidDocument)
}

fn decode_value<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, TaskWireError> {
    from_value(parse_value(bytes)?)
}

fn from_value<T: DeserializeOwned>(value: Value) -> Result<T, TaskWireError> {
    serde_json::from_value(value).map_err(|_| TaskWireError::InvalidDocument)
}

#[cfg(test)]
#[path = "task_wire_tests.rs"]
mod tests;

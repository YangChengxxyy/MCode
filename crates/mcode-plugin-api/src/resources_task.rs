//! Typed Resources family bodies for the Manager task wire.

// Rust guideline compliant 2026-08-31.

use serde::{Deserialize, Serialize};

use crate::task_wire::{FeatureTaskBody, sealed};
use crate::{OperationId, TaskErrorCode};

/// Resources request carried by a Manager start-task message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "case",
    content = "value",
    rename_all = "camelCase",
    deny_unknown_fields
)]
pub enum ResourcesTaskRequest {
    /// Lists embedded resources and prompt templates.
    Catalog(ResourcesCatalogRequest),
    /// Reads embedded resource text.
    Read(ResourcesReadRequest),
    /// Renders one embedded prompt template.
    RenderPrompt(ResourcesRenderPromptRequest),
    /// Lists contribution declarations.
    Contributions,
}

impl ResourcesTaskRequest {
    /// Returns the sole canonical operation ID for this request case.
    #[must_use]
    pub const fn operation_id(&self) -> &'static str {
        match self {
            Self::Catalog(_) => "catalog",
            Self::Read(_) => "read",
            Self::RenderPrompt(_) => "render-prompt",
            Self::Contributions => "contributions",
        }
    }
}

/// Validates that a Resources request case matches its declarative operation.
///
/// # Errors
///
/// Returns [`TaskErrorCode::InvalidRequest`] when the operation and case differ.
pub fn validate_resources_operation(
    operation_id: &OperationId,
    request: &ResourcesTaskRequest,
) -> Result<(), TaskErrorCode> {
    if operation_id.as_str() != request.operation_id() {
        return Err(TaskErrorCode::InvalidRequest);
    }
    Ok(())
}

/// Validates that optional Resources progress matches its request case.
///
/// # Errors
///
/// Returns [`TaskErrorCode::Failed`] for crossed guest progress.
pub const fn validate_resources_progress(
    request: &ResourcesTaskRequest,
    progress: ResourcesTaskProgress,
) -> Result<(), TaskErrorCode> {
    let matches = matches!(
        (request, progress),
        (
            ResourcesTaskRequest::Catalog(_)
                | ResourcesTaskRequest::Read(_)
                | ResourcesTaskRequest::Contributions,
            ResourcesTaskProgress::Loading
        ) | (
            ResourcesTaskRequest::RenderPrompt(_),
            ResourcesTaskProgress::Rendering
        )
    );
    if matches {
        Ok(())
    } else {
        Err(TaskErrorCode::Failed)
    }
}

/// Validates that a Resources success result matches its request case.
///
/// # Errors
///
/// Returns [`TaskErrorCode::Failed`] for crossed guest terminal output.
pub const fn validate_resources_result(
    request: &ResourcesTaskRequest,
    result: &ResourcesTaskResult,
) -> Result<(), TaskErrorCode> {
    let matches = matches!(
        (request, result),
        (
            ResourcesTaskRequest::Catalog(_),
            ResourcesTaskResult::Catalog(_)
        ) | (ResourcesTaskRequest::Read(_), ResourcesTaskResult::Read(_))
            | (
                ResourcesTaskRequest::RenderPrompt(_),
                ResourcesTaskResult::Prompt(_)
            )
            | (
                ResourcesTaskRequest::Contributions,
                ResourcesTaskResult::Contributions(_)
            )
    );
    if matches {
        Ok(())
    } else {
        Err(TaskErrorCode::Failed)
    }
}

/// Catalog page request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResourcesCatalogRequest {
    /// Zero-based catalog offset.
    pub offset: u32,
    /// Maximum number of entries requested.
    pub limit: u16,
}

/// Embedded resource read request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResourcesReadRequest {
    /// Resource-local identifier.
    pub id: String,
    /// UTF-8 byte offset.
    pub offset: u64,
    /// Maximum returned UTF-8 bytes.
    pub max_bytes: u32,
}

/// Embedded prompt render request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResourcesRenderPromptRequest {
    /// Prompt-local identifier.
    pub id: String,
    /// Canonically ordered prompt arguments.
    pub args: Vec<ResourcesPromptArg>,
}

/// One prompt argument.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResourcesPromptArg {
    /// Declared parameter name.
    pub name: String,
    /// Parameter value.
    pub value: String,
}

/// Optional Resources operation progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ResourcesTaskProgress {
    /// Catalog, read, or contributions data is loading.
    Loading,
    /// A prompt template is rendering.
    Rendering,
}

/// Successful Resources operation result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "case",
    content = "value",
    rename_all = "camelCase",
    deny_unknown_fields
)]
pub enum ResourcesTaskResult {
    /// Catalog page result.
    Catalog(ResourcesCatalogResult),
    /// Resource text result.
    Read(ResourcesReadResult),
    /// Rendered prompt result.
    Prompt(ResourcesPromptResult),
    /// Contribution declarations result.
    Contributions(ResourcesContributionsResult),
}

/// Catalog page result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResourcesCatalogResult {
    /// Catalog entries in canonical order.
    pub items: Vec<ResourcesCatalogEntry>,
    /// Next catalog offset, or `None` at EOF.
    pub next_offset: Option<u32>,
}

/// One catalog entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "case",
    content = "value",
    rename_all = "camelCase",
    deny_unknown_fields
)]
pub enum ResourcesCatalogEntry {
    /// Embedded resource declaration.
    Resource(ResourcesResourceEntry),
    /// Embedded prompt declaration.
    Prompt(ResourcesPromptEntry),
}

/// Embedded resource declaration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResourcesResourceEntry {
    /// Resource-local identifier.
    pub id: String,
    /// Display title.
    pub title: String,
    /// Resource media type.
    pub media: ResourcesMedia,
    /// Optional UTF-8 byte length hint.
    pub size_hint: Option<u64>,
}

/// Embedded resource media type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ResourcesMedia {
    /// Plain text.
    Text,
    /// Markdown text.
    Markdown,
}

/// Embedded prompt declaration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResourcesPromptEntry {
    /// Prompt-local identifier.
    pub id: String,
    /// Display title.
    pub title: String,
    /// Canonically ordered parameter declarations.
    pub params: Vec<ResourcesPromptParam>,
}

/// Embedded prompt parameter declaration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResourcesPromptParam {
    /// Parameter-local identifier.
    pub name: String,
    /// Display label.
    pub label: String,
    /// Whether the parameter must be supplied.
    pub required: bool,
}

/// Embedded resource read result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResourcesReadResult {
    /// Returned resource text.
    pub text: String,
    /// Next UTF-8 byte offset, or `None` at EOF.
    pub next_offset: Option<u64>,
}

/// Rendered prompt result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResourcesPromptResult {
    /// Prompt-local identifier copied from the request.
    pub id: String,
    /// Rendered messages in template declaration order.
    pub messages: Vec<ResourcesPromptMessage>,
}

/// One rendered prompt message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResourcesPromptMessage {
    /// Message role.
    pub role: ResourcesMessageRole,
    /// Rendered message text.
    pub text: String,
}

/// Rendered prompt message role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ResourcesMessageRole {
    /// System message.
    System,
    /// User message.
    User,
    /// Assistant message.
    Assistant,
}

/// Contribution declarations result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResourcesContributionsResult {
    /// Contribution declarations in canonical order.
    pub items: Vec<ResourcesContribution>,
}

/// One contribution declaration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResourcesContribution {
    /// Contribution-local identifier.
    pub id: String,
    /// Contribution kind.
    pub kind: ResourcesContributionKind,
}

/// Contribution kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ResourcesContributionKind {
    /// Status contribution.
    Status,
    /// Panel contribution.
    Panel,
}

macro_rules! task_body {
    ($type:ty) => {
        impl sealed::Sealed for $type {}
        impl FeatureTaskBody for $type {}
    };
}

task_body!(ResourcesTaskRequest);
task_body!(ResourcesTaskProgress);
task_body!(ResourcesTaskResult);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        FeatureTaskCompleted, FeatureTaskProgress, FeatureTaskRequest, FeatureTaskUpdate,
        TaskGeneration, TaskId, TaskWireError, decode_feature_task_request,
    };

    fn operation(value: &str) -> OperationId {
        OperationId::parse(value).expect("operation ID")
    }

    fn generation() -> TaskGeneration {
        TaskGeneration::new(7).expect("generation")
    }

    #[test]
    fn request_cases_have_exact_task_json_and_operation_mapping() {
        let cases = [
            (
                "catalog",
                ResourcesTaskRequest::Catalog(ResourcesCatalogRequest {
                    offset: 2,
                    limit: 16,
                }),
                r#"{"abiVersion":"0.0.1","kind":"featureService","operationId":"catalog","generation":7,"request":{"case":"catalog","value":{"offset":2,"limit":16}}}"#,
            ),
            (
                "read",
                ResourcesTaskRequest::Read(ResourcesReadRequest {
                    id: "guide".into(),
                    offset: 3,
                    max_bytes: 64,
                }),
                r#"{"abiVersion":"0.0.1","kind":"featureService","operationId":"read","generation":7,"request":{"case":"read","value":{"id":"guide","offset":3,"maxBytes":64}}}"#,
            ),
            (
                "render-prompt",
                ResourcesTaskRequest::RenderPrompt(ResourcesRenderPromptRequest {
                    id: "explain".into(),
                    args: vec![ResourcesPromptArg {
                        name: "topic".into(),
                        value: "Rust".into(),
                    }],
                }),
                r#"{"abiVersion":"0.0.1","kind":"featureService","operationId":"render-prompt","generation":7,"request":{"case":"renderPrompt","value":{"id":"explain","args":[{"name":"topic","value":"Rust"}]}}}"#,
            ),
            (
                "contributions",
                ResourcesTaskRequest::Contributions,
                r#"{"abiVersion":"0.0.1","kind":"featureService","operationId":"contributions","generation":7,"request":{"case":"contributions"}}"#,
            ),
        ];

        for (operation_name, request, expected) in cases {
            let operation_id = operation(operation_name);
            let envelope = FeatureTaskRequest::new(operation_id.clone(), generation(), request);
            assert_eq!(envelope.encode().expect("request JSON"), expected);

            let decoded = decode_feature_task_request::<ResourcesTaskRequest>(
                expected.as_bytes(),
                |_| Ok(()),
            )
            .expect("typed Resources request");
            assert_eq!(decoded, envelope);
            assert_eq!(
                validate_resources_operation(decoded.operation_id(), decoded.request()),
                Ok(())
            );
        }
    }

    #[test]
    fn every_crossed_operation_and_request_case_is_rejected() {
        let requests = [
            ResourcesTaskRequest::Catalog(ResourcesCatalogRequest {
                offset: 0,
                limit: 1,
            }),
            ResourcesTaskRequest::Read(ResourcesReadRequest {
                id: "guide".into(),
                offset: 0,
                max_bytes: 4,
            }),
            ResourcesTaskRequest::RenderPrompt(ResourcesRenderPromptRequest {
                id: "explain".into(),
                args: Vec::new(),
            }),
            ResourcesTaskRequest::Contributions,
        ];

        for operation_name in ["catalog", "read", "render-prompt", "contributions"] {
            for request in &requests {
                let result = validate_resources_operation(&operation(operation_name), request);
                assert_eq!(
                    result,
                    (operation_name == request.operation_id())
                        .then_some(())
                        .ok_or(TaskErrorCode::InvalidRequest)
                );
            }
        }
    }

    #[test]
    fn request_cases_accept_only_their_progress_and_result_cases() {
        let cases = [
            (
                ResourcesTaskRequest::Catalog(ResourcesCatalogRequest {
                    offset: 0,
                    limit: 1,
                }),
                ResourcesTaskProgress::Loading,
                ResourcesTaskResult::Catalog(ResourcesCatalogResult {
                    items: Vec::new(),
                    next_offset: None,
                }),
            ),
            (
                ResourcesTaskRequest::Read(ResourcesReadRequest {
                    id: "guide".into(),
                    offset: 0,
                    max_bytes: 4,
                }),
                ResourcesTaskProgress::Loading,
                ResourcesTaskResult::Read(ResourcesReadResult {
                    text: String::new(),
                    next_offset: None,
                }),
            ),
            (
                ResourcesTaskRequest::RenderPrompt(ResourcesRenderPromptRequest {
                    id: "explain".into(),
                    args: Vec::new(),
                }),
                ResourcesTaskProgress::Rendering,
                ResourcesTaskResult::Prompt(ResourcesPromptResult {
                    id: "explain".into(),
                    messages: Vec::new(),
                }),
            ),
            (
                ResourcesTaskRequest::Contributions,
                ResourcesTaskProgress::Loading,
                ResourcesTaskResult::Contributions(ResourcesContributionsResult {
                    items: Vec::new(),
                }),
            ),
        ];

        for (index, (request, progress, result)) in cases.iter().enumerate() {
            assert_eq!(validate_resources_progress(request, *progress), Ok(()));
            assert_eq!(validate_resources_result(request, result), Ok(()));
            let crossed = &cases[(index + 1) % cases.len()];
            assert_eq!(
                validate_resources_result(request, &crossed.2),
                Err(TaskErrorCode::Failed)
            );
        }
        assert_eq!(
            validate_resources_progress(&cases[0].0, ResourcesTaskProgress::Rendering),
            Err(TaskErrorCode::Failed)
        );
        assert_eq!(
            validate_resources_progress(&cases[2].0, ResourcesTaskProgress::Loading),
            Err(TaskErrorCode::Failed)
        );
    }

    #[test]
    fn request_bodies_reject_noncanonical_shapes() {
        for document in [
            r#"{"abiVersion":"0.0.1","kind":"featureService","operationId":"read","generation":7,"request":{"case":"read","value":{"id":"guide","offset":0,"maxBytes":4,"extra":true}}}"#,
            r#"{"abiVersion":"0.0.1","kind":"featureService","operationId":"read","generation":7,"request":{"case":"read"}}"#,
            r#"{"abiVersion":"0.0.1","kind":"featureService","operationId":"contributions","generation":7,"request":{"case":"contributions","value":{}}}"#,
            r#"{"abiVersion":"0.0.1","kind":"featureService","operationId":"render-prompt","generation":7,"request":{"case":"render-prompt","value":{"id":"explain","args":[]}}}"#,
        ] {
            assert_eq!(
                decode_feature_task_request::<ResourcesTaskRequest>(
                    document.as_bytes(),
                    |_| Ok(())
                ),
                Err(TaskWireError::InvalidBody)
            );
        }
    }

    #[test]
    fn progress_and_results_use_exact_typed_state_bodies() {
        let task_id = TaskId::parse("task1-fedcba9876543210fedcba9876543210").expect("task ID");
        let progress = FeatureTaskProgress::new(
            operation("render-prompt"),
            task_id.clone(),
            generation(),
            ResourcesTaskProgress::Rendering,
        );
        assert_eq!(
            progress.encode().expect("progress JSON"),
            r#"{"abiVersion":"0.0.1","kind":"featureService","operationId":"render-prompt","taskId":"task1-fedcba9876543210fedcba9876543210","generation":7,"state":"progress","progress":"rendering"}"#
        );

        let results = [
            (
                "catalog",
                ResourcesTaskResult::Catalog(ResourcesCatalogResult {
                    items: vec![
                        ResourcesCatalogEntry::Resource(ResourcesResourceEntry {
                            id: "guide".into(),
                            title: "Guide".into(),
                            media: ResourcesMedia::Markdown,
                            size_hint: Some(5),
                        }),
                        ResourcesCatalogEntry::Prompt(ResourcesPromptEntry {
                            id: "explain".into(),
                            title: "Explain".into(),
                            params: vec![ResourcesPromptParam {
                                name: "topic".into(),
                                label: "Topic".into(),
                                required: true,
                            }],
                        }),
                    ],
                    next_offset: Some(2),
                }),
                r#"{"case":"catalog","value":{"items":[{"case":"resource","value":{"id":"guide","title":"Guide","media":"markdown","sizeHint":5}},{"case":"prompt","value":{"id":"explain","title":"Explain","params":[{"name":"topic","label":"Topic","required":true}]}}],"nextOffset":2}}"#,
            ),
            (
                "read",
                ResourcesTaskResult::Read(ResourcesReadResult {
                    text: "hello".into(),
                    next_offset: None,
                }),
                r#"{"case":"read","value":{"text":"hello","nextOffset":null}}"#,
            ),
            (
                "render-prompt",
                ResourcesTaskResult::Prompt(ResourcesPromptResult {
                    id: "explain".into(),
                    messages: vec![ResourcesPromptMessage {
                        role: ResourcesMessageRole::Assistant,
                        text: "Hello".into(),
                    }],
                }),
                r#"{"case":"prompt","value":{"id":"explain","messages":[{"role":"assistant","text":"Hello"}]}}"#,
            ),
            (
                "contributions",
                ResourcesTaskResult::Contributions(ResourcesContributionsResult {
                    items: vec![ResourcesContribution {
                        id: "sidebar".into(),
                        kind: ResourcesContributionKind::Panel,
                    }],
                }),
                r#"{"case":"contributions","value":{"items":[{"id":"sidebar","kind":"panel"}]}}"#,
            ),
        ];

        for (operation_name, result, expected_body) in results {
            let completed = FeatureTaskCompleted::new(
                operation(operation_name),
                task_id.clone(),
                generation(),
                result,
            );
            let expected = format!(
                "{{\"abiVersion\":\"0.0.1\",\"kind\":\"featureService\",\"operationId\":\"{operation_name}\",\"taskId\":\"task1-fedcba9876543210fedcba9876543210\",\"generation\":7,\"state\":\"completed\",\"result\":{expected_body}}}"
            );
            let encoded = completed.encode().expect("completed JSON");
            assert_eq!(encoded, expected);
            assert_eq!(
                FeatureTaskUpdate::<ResourcesTaskProgress, ResourcesTaskResult>::decode(
                    encoded.as_bytes()
                ),
                Ok(FeatureTaskUpdate::Completed(completed))
            );
        }
    }
}

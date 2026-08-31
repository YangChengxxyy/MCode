//! Typed Resources Pack operation ownership and guest dispatch.

// Rust guideline compliant 2026-08-31.

use mcode_plugin_api::{
    ResourcesCatalogEntry, ResourcesCatalogResult, ResourcesContribution,
    ResourcesContributionKind, ResourcesContributionsResult, ResourcesMedia, ResourcesMessageRole,
    ResourcesPromptArg, ResourcesPromptEntry, ResourcesPromptMessage, ResourcesPromptParam,
    ResourcesPromptResult, ResourcesReadResult, ResourcesResourceEntry, ResourcesTaskProgress,
    ResourcesTaskRequest, ResourcesTaskResult,
};
use wasmtime::component::ResourceAny;

use crate::pack_wit;
use crate::pack_wit::resources::exports::mcode::feature_pack::resources_pack as guest;

use super::admission::OperationPermit;
use super::owner::OwnerIdentity;
use super::segment::SegmentExecution;
use super::{OperationLease, PackInstance, PluginOwner, ResourcePermit, RuntimeError};

/// Owns one active Resources Pack Store and serializes its guest calls.
pub(crate) struct ResourcesPackActor {
    owner: PluginOwner,
    bindings: pack_wit::resources::Resources,
}

/// Owns one guest Resources operation and its total fuel/admission leases.
pub(crate) struct ResourcesOperation {
    owner: OwnerIdentity,
    resource: ResourceAny,
    operation: OperationLease,
    resource_admission: Option<ResourcePermit>,
}

pub(super) struct ResourcesOperationAdmission {
    _operation: OperationPermit,
    _resource: ResourcePermit,
}

/// One typed Resources Pack pull receipt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ResourcesPackPull {
    Pending,
    Progress(ResourcesTaskProgress),
    Complete(ResourcesTaskResult),
    Failed(ResourcesPackError),
}

/// Stable Resources guest error without guest-provided text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResourcesPackError {
    InvalidArgument,
    NotFound,
    Limit,
    Unavailable,
    Cancelled,
}

/// Reports a typed Resources guest rejection or a fail-closed runtime error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResourcesPackCallError {
    Guest(ResourcesPackError),
    OperationMismatch,
    Runtime,
}

impl ResourcesPackActor {
    pub(crate) fn from_parts(
        owner: PluginOwner,
        instance: PackInstance,
    ) -> Result<Self, RuntimeError> {
        let bindings = instance.into_resources(&owner)?;
        Ok(Self { owner, bindings })
    }

    pub(crate) const fn is_available(&self) -> bool {
        self.owner.is_available()
    }

    pub(crate) async fn invoke(
        &mut self,
        request: &ResourcesTaskRequest,
    ) -> Result<ResourcesOperation, ResourcesPackCallError> {
        let resource_permit = self
            .owner
            .admit_resource()
            .map_err(|_| ResourcesPackCallError::Runtime)?;
        let mut operation = self
            .owner
            .open_operation()
            .map_err(|_| ResourcesPackCallError::Runtime)?;
        let owner = self.owner.identity.clone();
        let mut execution = SegmentExecution::start_plugin_call(&mut self.owner, &mut operation)
            .map_err(|_| ResourcesPackCallError::Runtime)?;
        let result = self
            .bindings
            .mcode_feature_pack_resources_pack()
            .call_invoke(execution.store_mut(), &lower_request(request))
            .await;
        match result {
            Ok(Ok(resource)) => {
                execution
                    .complete()
                    .map_err(|_| ResourcesPackCallError::Runtime)?;
                Ok(ResourcesOperation {
                    owner,
                    resource,
                    operation,
                    resource_admission: Some(resource_permit),
                })
            }
            Ok(Err(error)) => {
                execution
                    .complete()
                    .map_err(|_| ResourcesPackCallError::Runtime)?;
                Err(ResourcesPackCallError::Guest(map_error(error)))
            }
            Err(_) => {
                execution
                    .dispose()
                    .map_err(|_| ResourcesPackCallError::Runtime)?;
                Err(ResourcesPackCallError::Runtime)
            }
        }
    }

    pub(crate) async fn pull(
        &mut self,
        operation: &mut ResourcesOperation,
    ) -> Result<ResourcesPackPull, ResourcesPackCallError> {
        if operation.owner != self.owner.identity {
            return Err(ResourcesPackCallError::OperationMismatch);
        }
        let mut execution =
            SegmentExecution::start_plugin_call(&mut self.owner, &mut operation.operation)
                .map_err(|_| ResourcesPackCallError::Runtime)?;
        let result = self
            .bindings
            .mcode_feature_pack_resources_pack()
            .resources_operation()
            .call_pull(execution.store_mut(), operation.resource)
            .await;
        match result {
            Ok(pull) => {
                execution
                    .complete()
                    .map_err(|_| ResourcesPackCallError::Runtime)?;
                Ok(lift_pull(pull))
            }
            Err(_) => {
                execution
                    .dispose()
                    .map_err(|_| ResourcesPackCallError::Runtime)?;
                Err(ResourcesPackCallError::Runtime)
            }
        }
    }

    pub(crate) async fn drop_operation(
        &mut self,
        operation: ResourcesOperation,
    ) -> Result<(), ResourcesPackCallError> {
        if operation.owner != self.owner.identity {
            return Err(ResourcesPackCallError::OperationMismatch);
        }
        let ResourcesOperation {
            resource,
            mut operation,
            resource_admission,
            ..
        } = operation;
        let mut execution = SegmentExecution::start_plugin_call(&mut self.owner, &mut operation)
            .map_err(|_| ResourcesPackCallError::Runtime)?;
        let result = match resource.resource_drop_async(execution.store_mut()).await {
            Ok(()) => execution
                .complete()
                .map_err(|_| ResourcesPackCallError::Runtime),
            Err(_) => {
                execution
                    .dispose()
                    .map_err(|_| ResourcesPackCallError::Runtime)?;
                Err(ResourcesPackCallError::Runtime)
            }
        };
        drop(resource_admission);
        result
    }
}

impl ResourcesOperation {
    pub(super) fn take_admission(&mut self) -> Option<ResourcesOperationAdmission> {
        Some(ResourcesOperationAdmission::new(
            self.operation.take_admission()?,
            self.resource_admission.take()?,
        ))
    }
}

impl ResourcesOperationAdmission {
    pub(super) const fn new(operation: OperationPermit, resource: ResourcePermit) -> Self {
        Self {
            _operation: operation,
            _resource: resource,
        }
    }
}

fn lower_request(request: &ResourcesTaskRequest) -> guest::ResourcesRequest {
    match request {
        ResourcesTaskRequest::Catalog(request) => {
            guest::ResourcesRequest::Catalog(guest::CatalogRequest {
                offset: request.offset,
                limit: request.limit,
            })
        }
        ResourcesTaskRequest::Read(request) => guest::ResourcesRequest::Read(guest::ReadRequest {
            id: request.id.clone(),
            offset: request.offset,
            max_bytes: request.max_bytes,
        }),
        ResourcesTaskRequest::RenderPrompt(request) => {
            guest::ResourcesRequest::RenderPrompt(guest::RenderPromptRequest {
                id: request.id.clone(),
                args: request.args.iter().map(lower_prompt_arg).collect(),
            })
        }
        ResourcesTaskRequest::Contributions => guest::ResourcesRequest::Contributions,
    }
}

fn lower_prompt_arg(argument: &ResourcesPromptArg) -> guest::PromptArg {
    guest::PromptArg {
        name: argument.name.clone(),
        value: argument.value.clone(),
    }
}

const fn map_error(error: guest::ResourcesError) -> ResourcesPackError {
    match error {
        guest::ResourcesError::InvalidArgument => ResourcesPackError::InvalidArgument,
        guest::ResourcesError::NotFound => ResourcesPackError::NotFound,
        guest::ResourcesError::Limit => ResourcesPackError::Limit,
        guest::ResourcesError::Unavailable => ResourcesPackError::Unavailable,
        guest::ResourcesError::Cancelled => ResourcesPackError::Cancelled,
    }
}

fn lift_pull(pull: guest::ResourcesPull) -> ResourcesPackPull {
    match pull {
        guest::ResourcesPull::Pending => ResourcesPackPull::Pending,
        guest::ResourcesPull::Progress(progress) => {
            ResourcesPackPull::Progress(lift_progress(progress))
        }
        guest::ResourcesPull::Complete(result) => ResourcesPackPull::Complete(lift_result(result)),
        guest::ResourcesPull::Failed(error) => ResourcesPackPull::Failed(map_error(error)),
    }
}

const fn lift_progress(progress: guest::ResourcesProgress) -> ResourcesTaskProgress {
    match progress {
        guest::ResourcesProgress::Loading => ResourcesTaskProgress::Loading,
        guest::ResourcesProgress::Rendering => ResourcesTaskProgress::Rendering,
    }
}

fn lift_result(result: guest::ResourcesResult) -> ResourcesTaskResult {
    match result {
        guest::ResourcesResult::Catalog(result) => {
            ResourcesTaskResult::Catalog(ResourcesCatalogResult {
                items: result.items.into_iter().map(lift_catalog_entry).collect(),
                next_offset: result.next_offset,
            })
        }
        guest::ResourcesResult::Read(result) => ResourcesTaskResult::Read(ResourcesReadResult {
            text: result.text,
            next_offset: result.next_offset,
        }),
        guest::ResourcesResult::Prompt(result) => {
            ResourcesTaskResult::Prompt(ResourcesPromptResult {
                id: result.id,
                messages: result.messages.into_iter().map(lift_message).collect(),
            })
        }
        guest::ResourcesResult::Contributions(result) => {
            ResourcesTaskResult::Contributions(ResourcesContributionsResult {
                items: result.items.into_iter().map(lift_contribution).collect(),
            })
        }
    }
}

fn lift_catalog_entry(entry: guest::CatalogEntry) -> ResourcesCatalogEntry {
    match entry {
        guest::CatalogEntry::Resource(entry) => {
            ResourcesCatalogEntry::Resource(ResourcesResourceEntry {
                id: entry.id,
                title: entry.title,
                media: match entry.media {
                    guest::ResourceMedia::Text => ResourcesMedia::Text,
                    guest::ResourceMedia::Markdown => ResourcesMedia::Markdown,
                },
                size_hint: entry.size_hint,
            })
        }
        guest::CatalogEntry::Prompt(entry) => ResourcesCatalogEntry::Prompt(ResourcesPromptEntry {
            id: entry.id,
            title: entry.title,
            params: entry.params.into_iter().map(lift_prompt_param).collect(),
        }),
    }
}

fn lift_prompt_param(parameter: guest::PromptParam) -> ResourcesPromptParam {
    ResourcesPromptParam {
        name: parameter.name,
        label: parameter.label,
        required: parameter.required,
    }
}

fn lift_message(message: guest::PromptMessage) -> ResourcesPromptMessage {
    ResourcesPromptMessage {
        role: match message.role {
            guest::MessageRole::System => ResourcesMessageRole::System,
            guest::MessageRole::User => ResourcesMessageRole::User,
            guest::MessageRole::Assistant => ResourcesMessageRole::Assistant,
        },
        text: message.text,
    }
}

fn lift_contribution(contribution: guest::Contribution) -> ResourcesContribution {
    ResourcesContribution {
        id: contribution.id,
        kind: match contribution.kind {
            guest::ContributionKind::Status => ResourcesContributionKind::Status,
            guest::ContributionKind::Panel => ResourcesContributionKind::Panel,
        },
    }
}

#[cfg(test)]
#[path = "resources_tests.rs"]
mod tests;

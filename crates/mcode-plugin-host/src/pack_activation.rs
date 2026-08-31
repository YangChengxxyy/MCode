//! Generation-bound atomic Pack set preparation and publication.

// Rust guideline compliant 2026-08-31.

use std::sync::Arc;

use mcode_config::{HomeLayout, PluginFamily};
use mcode_plugin_api::{
    FeatureTaskControl, OperationId, ResourcesTaskProgress, ResourcesTaskRequest,
    ResourcesTaskResult, TaskErrorCode, TaskGeneration, TaskId,
};
use tokio::time::Instant;

use crate::manager_director::{GenerationActivity, GenerationCommitError};
use crate::pack_loading::{VerifiedPackCandidate, load_verified_pack};
use crate::pack_selection::{
    ConfiguredPackSelection, PackActivationError as SelectionActivationError, PackActivationTarget,
    PackSelectionClient, PackSelectionIssueError,
};
use crate::resources_validation::{
    validate_resources_progress_body, validate_resources_request, validate_resources_result_body,
};
use crate::runtime::{
    PackInstance, PluginOwner, PluginRuntime, ResourcesActorClient, ResourcesActorError,
    ResourcesActorOperationId, ResourcesCloseSignal, ResourcesPackActor, ResourcesPackCallError,
    ResourcesPackError, ResourcesPackPull, RuntimeError,
};

mod resources_sentinel;
mod resources_tasks;

pub(crate) use resources_sentinel::{
    ResourcesCancelConsume, ResourcesCancelSignal, ResourcesPullFinish, ResourcesPullStart,
    ResourcesTaskRegisterError, ResourcesTaskSentinel,
};
use resources_tasks::{ResourcesTaskTable, ResourcesTaskTableError};

pub(crate) struct PackActivationClient {
    runtime: Arc<PluginRuntime>,
    home: HomeLayout,
    family: PluginFamily,
    selection: PackSelectionClient,
    task_sentinel: Arc<ResourcesTaskSentinel>,
    active: Option<ActivePackSet>,
}

struct ActivePackSet {
    target: PackActivationTarget,
    resources_tasks: ResourcesTaskTable<ResourcesActorOperationId>,
    packs: Vec<ActivePack>,
}

struct ActivePack {
    _candidate: VerifiedPackCandidate,
    runtime: ActivePackRuntime,
}

enum ActivePackRuntime {
    Resources(ResourcesActorClient),
    Other {
        _instance: PackInstance,
        _owner: PluginOwner,
    },
}

impl PackActivationClient {
    pub(crate) const fn new(
        runtime: Arc<PluginRuntime>,
        home: HomeLayout,
        family: PluginFamily,
        selection: PackSelectionClient,
        task_sentinel: Arc<ResourcesTaskSentinel>,
    ) -> Self {
        Self {
            runtime,
            home,
            family,
            selection,
            task_sentinel,
            active: None,
        }
    }

    pub(crate) fn configured_selection(
        &mut self,
    ) -> Result<ConfiguredPackSelection, PackSelectionIssueError> {
        self.selection.issue()
    }

    pub(crate) async fn activate(
        &mut self,
        activity: &GenerationActivity,
        selection_stamp: &str,
    ) -> Result<String, PackActivationError> {
        let target = self
            .selection
            .begin_activation(selection_stamp)
            .map_err(PackActivationError::from)?;
        if self
            .active
            .as_ref()
            .is_some_and(|active| active.target == target && active.is_healthy())
        {
            return self.commit(activity, target, None);
        }

        let mut packs = Vec::with_capacity(target.pack_ids().len());
        for pack_id in target.pack_ids() {
            let candidate = load_verified_pack(&self.runtime, &self.home, self.family, pack_id)
                .map_err(|_| PackActivationError::Failed)?;
            let mut owner = self.runtime.new_owner().map_err(map_runtime_error)?;
            let instance = owner
                .instantiate_pack(candidate.component())
                .await
                .map_err(map_runtime_error)?;
            let runtime = if self.family == PluginFamily::Resources {
                ActivePackRuntime::Resources(ResourcesActorClient::start(
                    ResourcesPackActor::from_parts(owner, instance).map_err(map_runtime_error)?,
                ))
            } else {
                ActivePackRuntime::Other {
                    _instance: instance,
                    _owner: owner,
                }
            };
            packs.push(ActivePack {
                _candidate: candidate,
                runtime,
            });
        }
        self.commit(activity, target, Some(packs))
    }

    fn commit(
        &mut self,
        activity: &GenerationActivity,
        target: PackActivationTarget,
        replacement: Option<Vec<ActivePack>>,
    ) -> Result<String, PackActivationError> {
        let generation_commit = activity.begin_commit().map_err(|error| match error {
            GenerationCommitError::Stale => PackActivationError::StaleGeneration,
            GenerationCommitError::Unavailable => PackActivationError::Unavailable,
        })?;
        let selection_commit = self
            .selection
            .commit_activation(&target)
            .map_err(PackActivationError::from)?;
        let previous = replacement.and_then(|packs| {
            self.task_sentinel.invalidate_open();
            self.active.replace(ActivePackSet {
                target: target.clone(),
                resources_tasks: ResourcesTaskTable::new(),
                packs,
            })
        });
        let selection_stamp = self
            .active
            .as_ref()
            .map_or_else(
                || target.selection_stamp(),
                |active| active.target.selection_stamp(),
            )
            .to_owned();
        drop(selection_commit);
        drop(generation_commit);
        drop(previous);
        Ok(selection_stamp)
    }

    pub(crate) async fn start_resources_task(
        &mut self,
        operation_id: OperationId,
        generation: TaskGeneration,
        request: ResourcesTaskRequest,
        deadline: Instant,
    ) -> Result<TaskId, ResourcesTaskError> {
        validate_resources_request(&request).map_err(|_| ResourcesTaskError::InvalidRequest)?;
        let task_sentinel = Arc::clone(&self.task_sentinel);
        let active = self
            .active
            .as_mut()
            .ok_or(ResourcesTaskError::FeatureUnavailable)?;
        let ActivePackSet {
            packs,
            resources_tasks,
            ..
        } = active;
        let actor = resources_actor(packs)?;
        resources_tasks.retain(|row| actor.is_operation_open(row.operation()));
        let task_id = resources_tasks.mint().map_err(ResourcesTaskError::from)?;
        let control = FeatureTaskControl::new(operation_id.clone(), task_id.clone(), generation);
        let close = ResourcesCloseSignal::new();
        task_sentinel
            .register(&control, close.clone())
            .map_err(ResourcesTaskError::from)?;
        let operation = match actor.invoke(request.clone(), deadline, close).await {
            Ok(operation) => operation,
            Err(error) => {
                task_sentinel.remove(&control);
                let error = ResourcesTaskError::from(error);
                if matches!(error, ResourcesTaskError::ActorUnavailable) {
                    task_sentinel.invalidate_open();
                    self.active.take();
                }
                return Err(error);
            }
        };
        resources_tasks.insert(
            task_id.clone(),
            operation_id,
            generation,
            request,
            deadline,
            operation,
        );
        Ok(task_id)
    }

    pub(crate) async fn poll_resources_task(
        &mut self,
        operation_id: &OperationId,
        task_id: &TaskId,
        generation: TaskGeneration,
    ) -> Result<ResourcesTaskPoll, ResourcesTaskError> {
        let control = FeatureTaskControl::new(operation_id.clone(), task_id.clone(), generation);
        match self.task_sentinel.begin_pull(&control) {
            ResourcesPullStart::Started => {}
            ResourcesPullStart::Busy => {
                return Err(ResourcesTaskError::Task(TaskErrorCode::Failed));
            }
            ResourcesPullStart::Cancelling => return Err(ResourcesTaskError::OperationClosed),
            ResourcesPullStart::Expired => {
                if let Some(active) = self.active.as_mut() {
                    let ActivePackSet {
                        packs,
                        resources_tasks,
                        ..
                    } = active;
                    if let Some(row) = resources_tasks.remove(task_id, operation_id, generation) {
                        resources_actor(packs)?.close(row.into_operation());
                    }
                }
                return Err(ResourcesTaskError::Task(TaskErrorCode::Cancelled));
            }
            ResourcesPullStart::Missing => return Err(ResourcesTaskError::UnknownTask),
            ResourcesPullStart::WrongGeneration => {
                return Err(ResourcesTaskError::Task(TaskErrorCode::StaleGeneration));
            }
        }
        let (outcome, unhealthy) = {
            let Some(active) = self.active.as_mut() else {
                self.task_sentinel.finish_terminal(&control);
                return Err(ResourcesTaskError::UnknownTask);
            };
            let ActivePackSet {
                packs,
                resources_tasks,
                ..
            } = active;
            let actor = resources_actor(packs)?;
            let Some(row) = resources_tasks.get_mut(task_id, operation_id, generation) else {
                self.task_sentinel.finish_terminal(&control);
                return Err(ResourcesTaskError::UnknownTask);
            };
            if !actor.is_operation_open(row.operation()) {
                resources_tasks
                    .remove(task_id, operation_id, generation)
                    .expect("a worker-closed Resources task remains until invalidation");
                self.task_sentinel.finish_terminal(&control);
                return Err(ResourcesTaskError::OperationClosed);
            }
            if row.reserve_pull().is_err() {
                let operation = resources_tasks
                    .remove(task_id, operation_id, generation)
                    .expect("an over-limit Resources task remains until cleanup")
                    .into_operation();
                actor.close(operation);
                self.task_sentinel.finish_terminal(&control);
                return Err(ResourcesTaskError::Task(TaskErrorCode::Failed));
            }
            let request = row.request().clone();
            let pull = actor.pull(row.operation(), row.deadline()).await;
            let (outcome, terminal) = match pull {
                Ok(ResourcesPackPull::Pending) => (Ok(ResourcesTaskPoll::Open), false),
                Ok(ResourcesPackPull::Progress(progress)) => {
                    if row.accept_progress().is_err()
                        || validate_resources_progress_body(&request, progress).is_err()
                    {
                        (Err(ResourcesTaskError::Task(TaskErrorCode::Failed)), true)
                    } else {
                        (Ok(ResourcesTaskPoll::Progress(progress)), false)
                    }
                }
                Ok(ResourcesPackPull::Complete(result)) => {
                    if validate_resources_result_body(&request, &result).is_err() {
                        (Err(ResourcesTaskError::Task(TaskErrorCode::Failed)), true)
                    } else {
                        (Ok(ResourcesTaskPoll::Complete(result)), true)
                    }
                }
                Ok(ResourcesPackPull::Failed(error)) => {
                    (Err(ResourcesTaskError::Guest(error)), true)
                }
                Err(error) => (Err(ResourcesTaskError::from(error)), true),
            };
            let unhealthy = matches!(outcome, Err(ResourcesTaskError::ActorUnavailable));
            let finish = if terminal {
                self.task_sentinel.finish_terminal(&control)
            } else {
                self.task_sentinel.finish_nonterminal(&control)
            };
            if !terminal && finish == ResourcesPullFinish::Publish {
                return outcome;
            }
            let operation = resources_tasks
                .remove(task_id, operation_id, generation)
                .expect("a terminal Resources task remains until cleanup")
                .into_operation();
            actor.close(operation);
            let outcome = match finish {
                ResourcesPullFinish::Publish if terminal => outcome,
                ResourcesPullFinish::Cancelled => Err(ResourcesTaskError::OperationClosed),
                ResourcesPullFinish::Publish
                | ResourcesPullFinish::Missing
                | ResourcesPullFinish::InvalidState
                | ResourcesPullFinish::WrongGeneration => {
                    Err(ResourcesTaskError::Task(TaskErrorCode::Failed))
                }
            };
            (outcome, unhealthy)
        };
        if unhealthy {
            self.task_sentinel.invalidate_open();
            self.active.take();
        }
        outcome
    }

    pub(crate) async fn cancel_resources_task(
        &mut self,
        operation_id: &OperationId,
        task_id: &TaskId,
        generation: TaskGeneration,
    ) -> Result<(), ResourcesTaskError> {
        let control = FeatureTaskControl::new(operation_id.clone(), task_id.clone(), generation);
        match self.task_sentinel.signal_cancel(&control) {
            ResourcesCancelSignal::Won | ResourcesCancelSignal::AlreadyCancelling => {}
            ResourcesCancelSignal::WrongGeneration => {
                return Err(ResourcesTaskError::Task(TaskErrorCode::StaleGeneration));
            }
            ResourcesCancelSignal::Expired => {
                if let Some(active) = self.active.as_mut() {
                    let ActivePackSet {
                        packs,
                        resources_tasks,
                        ..
                    } = active;
                    if let Some(row) = resources_tasks.remove(task_id, operation_id, generation) {
                        resources_actor(packs)?.close(row.into_operation());
                    }
                }
                return match self.task_sentinel.consume_cancel(&control) {
                    ResourcesCancelConsume::Expired => {
                        Err(ResourcesTaskError::Task(TaskErrorCode::Cancelled))
                    }
                    ResourcesCancelConsume::Consumed => Ok(()),
                    ResourcesCancelConsume::Missing => Err(ResourcesTaskError::UnknownTask),
                    ResourcesCancelConsume::WrongGeneration => {
                        Err(ResourcesTaskError::Task(TaskErrorCode::StaleGeneration))
                    }
                };
            }
            ResourcesCancelSignal::Missing => {
                return Err(ResourcesTaskError::UnknownTask);
            }
        }

        if let Some(active) = self.active.as_mut() {
            let ActivePackSet {
                packs,
                resources_tasks,
                ..
            } = active;
            if let Some(row) = resources_tasks.remove(task_id, operation_id, generation) {
                resources_actor(packs)?.close(row.into_operation());
            }
        }
        match self.task_sentinel.consume_cancel(&control) {
            ResourcesCancelConsume::Consumed => Ok(()),
            ResourcesCancelConsume::Expired => {
                Err(ResourcesTaskError::Task(TaskErrorCode::Cancelled))
            }
            ResourcesCancelConsume::Missing => Err(ResourcesTaskError::UnknownTask),
            ResourcesCancelConsume::WrongGeneration => {
                Err(ResourcesTaskError::Task(TaskErrorCode::StaleGeneration))
            }
        }
    }
}

fn resources_actor(
    packs: &mut [ActivePack],
) -> Result<&mut ResourcesActorClient, ResourcesTaskError> {
    let [pack] = packs else {
        return Err(ResourcesTaskError::FeatureUnavailable);
    };
    match &mut pack.runtime {
        ActivePackRuntime::Resources(actor) => Ok(actor),
        ActivePackRuntime::Other { .. } => Err(ResourcesTaskError::FeatureUnavailable),
    }
}

pub(crate) enum ResourcesTaskPoll {
    Open,
    Progress(ResourcesTaskProgress),
    Complete(ResourcesTaskResult),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ResourcesTaskError {
    InvalidRequest,
    TaskLimitReached,
    UnknownTask,
    FeatureUnavailable,
    OperationClosed,
    ActorUnavailable,
    Guest(ResourcesPackError),
    Task(TaskErrorCode),
}

impl From<ResourcesTaskTableError> for ResourcesTaskError {
    fn from(error: ResourcesTaskTableError) -> Self {
        match error {
            ResourcesTaskTableError::Limit => Self::TaskLimitReached,
            ResourcesTaskTableError::Unavailable => Self::FeatureUnavailable,
        }
    }
}

impl From<ResourcesTaskRegisterError> for ResourcesTaskError {
    fn from(error: ResourcesTaskRegisterError) -> Self {
        match error {
            ResourcesTaskRegisterError::WrongGeneration => {
                Self::Task(TaskErrorCode::StaleGeneration)
            }
            ResourcesTaskRegisterError::Limit => Self::TaskLimitReached,
            ResourcesTaskRegisterError::Closed | ResourcesTaskRegisterError::Duplicate => {
                Self::FeatureUnavailable
            }
        }
    }
}

impl From<ResourcesPackCallError> for ResourcesTaskError {
    fn from(error: ResourcesPackCallError) -> Self {
        match error {
            ResourcesPackCallError::Guest(error) => Self::Guest(error),
            ResourcesPackCallError::OperationMismatch => Self::InvalidRequest,
            ResourcesPackCallError::Runtime => Self::FeatureUnavailable,
        }
    }
}

impl From<ResourcesActorError> for ResourcesTaskError {
    fn from(error: ResourcesActorError) -> Self {
        match error {
            ResourcesActorError::UnknownOperation => Self::OperationClosed,
            ResourcesActorError::Unavailable => Self::ActorUnavailable,
            ResourcesActorError::Pack(error) => Self::from(error),
        }
    }
}

impl ActivePackSet {
    fn is_healthy(&self) -> bool {
        self.packs.iter().all(|pack| match &pack.runtime {
            ActivePackRuntime::Resources(actor) => actor.is_available(),
            ActivePackRuntime::Other { .. } => true,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PackActivationError {
    InvalidSelection,
    StaleGeneration,
    Limit,
    Unavailable,
    Failed,
}

impl From<SelectionActivationError> for PackActivationError {
    fn from(error: SelectionActivationError) -> Self {
        match error {
            SelectionActivationError::InvalidSelection => Self::InvalidSelection,
            SelectionActivationError::Unavailable => Self::Unavailable,
        }
    }
}

fn map_runtime_error(error: RuntimeError) -> PackActivationError {
    match error {
        RuntimeError::Admission(_) => PackActivationError::Limit,
        RuntimeError::Engine
        | RuntimeError::RuntimeUninitialized
        | RuntimeError::EpochTicker
        | RuntimeError::Fuel => PackActivationError::Unavailable,
        _ => PackActivationError::Failed,
    }
}

#[cfg(test)]
#[path = "pack_activation/tests.rs"]
mod tests;

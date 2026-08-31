//! Generation-bound Resources task close linearization.

// Rust guideline compliant 2026-08-31.

use std::collections::HashMap;
use std::sync::Mutex;

use mcode_plugin_api::{FeatureTaskControl, OperationId, TaskGeneration, TaskId};

use crate::runtime::{MAX_OPEN_OPERATIONS, ResourcesCloseSignal};

/// Tracks exact Resources tasks outside the Manager Store lock.
pub(crate) struct ResourcesTaskSentinel {
    generation: TaskGeneration,
    entries: Mutex<HashMap<ResourcesTaskKey, ResourcesTaskEntry>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct ResourcesTaskKey {
    operation_id: OperationId,
    task_id: TaskId,
    generation: TaskGeneration,
}

struct ResourcesTaskEntry {
    phase: ResourcesTaskPhase,
    close: ResourcesCloseSignal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ResourcesTaskPhase {
    Open,
    Pulling,
    Cancelling,
    Terminal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ResourcesTaskRegisterError {
    WrongGeneration,
    Closed,
    Duplicate,
    Limit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ResourcesPullStart {
    Started,
    Busy,
    Cancelling,
    Expired,
    Missing,
    WrongGeneration,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ResourcesPullFinish {
    Publish,
    Cancelled,
    Missing,
    InvalidState,
    WrongGeneration,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ResourcesCancelSignal {
    Won,
    AlreadyCancelling,
    Expired,
    Missing,
    WrongGeneration,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ResourcesCancelConsume {
    Consumed,
    Expired,
    Missing,
    WrongGeneration,
}

impl ResourcesTaskSentinel {
    pub(crate) fn new(generation: TaskGeneration) -> Self {
        Self {
            generation,
            entries: Mutex::new(HashMap::new()),
        }
    }

    pub(crate) fn register(
        &self,
        control: &FeatureTaskControl,
        close: ResourcesCloseSignal,
    ) -> Result<(), ResourcesTaskRegisterError> {
        let key = self.key(control)?;
        let mut entries = self.entries.lock().expect("Resources task sentinel lock");
        if close.is_closed() {
            return Err(ResourcesTaskRegisterError::Closed);
        }
        if entries.contains_key(&key) {
            return Err(ResourcesTaskRegisterError::Duplicate);
        }
        if entries.len() >= MAX_OPEN_OPERATIONS {
            entries.retain(|_, entry| {
                entry.phase == ResourcesTaskPhase::Cancelling || !entry.close.is_closed()
            });
        }
        if entries.len() >= MAX_OPEN_OPERATIONS {
            return Err(ResourcesTaskRegisterError::Limit);
        }
        entries.insert(
            key,
            ResourcesTaskEntry {
                phase: ResourcesTaskPhase::Open,
                close,
            },
        );
        Ok(())
    }

    pub(crate) fn begin_pull(&self, control: &FeatureTaskControl) -> ResourcesPullStart {
        let Ok(key) = self.key(control) else {
            return ResourcesPullStart::WrongGeneration;
        };
        let mut entries = self.entries.lock().expect("Resources task sentinel lock");
        if remove_closed_non_cancelling(&mut entries, &key) {
            return ResourcesPullStart::Expired;
        }
        let Some(entry) = entries.get_mut(&key) else {
            return ResourcesPullStart::Missing;
        };
        match entry.phase {
            ResourcesTaskPhase::Open => {
                entry.phase = ResourcesTaskPhase::Pulling;
                ResourcesPullStart::Started
            }
            ResourcesTaskPhase::Pulling => ResourcesPullStart::Busy,
            ResourcesTaskPhase::Cancelling => ResourcesPullStart::Cancelling,
            ResourcesTaskPhase::Terminal => ResourcesPullStart::Missing,
        }
    }

    pub(crate) fn finish_nonterminal(&self, control: &FeatureTaskControl) -> ResourcesPullFinish {
        let Ok(key) = self.key(control) else {
            return ResourcesPullFinish::WrongGeneration;
        };
        let mut entries = self.entries.lock().expect("Resources task sentinel lock");
        if remove_closed_non_cancelling(&mut entries, &key) {
            return ResourcesPullFinish::Cancelled;
        }
        let Some(entry) = entries.get_mut(&key) else {
            return ResourcesPullFinish::Missing;
        };
        match entry.phase {
            ResourcesTaskPhase::Pulling => {
                entry.phase = ResourcesTaskPhase::Open;
                ResourcesPullFinish::Publish
            }
            ResourcesTaskPhase::Cancelling => ResourcesPullFinish::Cancelled,
            ResourcesTaskPhase::Open | ResourcesTaskPhase::Terminal => {
                ResourcesPullFinish::InvalidState
            }
        }
    }

    pub(crate) fn finish_terminal(&self, control: &FeatureTaskControl) -> ResourcesPullFinish {
        let Ok(key) = self.key(control) else {
            return ResourcesPullFinish::WrongGeneration;
        };
        let mut entries = self.entries.lock().expect("Resources task sentinel lock");
        if remove_closed_non_cancelling(&mut entries, &key) {
            return ResourcesPullFinish::Cancelled;
        }
        let Some(entry) = entries.get_mut(&key) else {
            return ResourcesPullFinish::Missing;
        };
        match entry.phase {
            ResourcesTaskPhase::Pulling => {
                entry.phase = ResourcesTaskPhase::Terminal;
                entries
                    .remove(&key)
                    .expect("terminal Resources task remains registered");
                ResourcesPullFinish::Publish
            }
            ResourcesTaskPhase::Cancelling => ResourcesPullFinish::Cancelled,
            ResourcesTaskPhase::Open | ResourcesTaskPhase::Terminal => {
                ResourcesPullFinish::InvalidState
            }
        }
    }

    pub(crate) fn signal_cancel(&self, control: &FeatureTaskControl) -> ResourcesCancelSignal {
        let Ok(key) = self.key(control) else {
            return ResourcesCancelSignal::WrongGeneration;
        };
        let (result, close) = {
            let mut entries = self.entries.lock().expect("Resources task sentinel lock");
            let Some(entry) = entries.get_mut(&key) else {
                return ResourcesCancelSignal::Missing;
            };
            if entry.phase != ResourcesTaskPhase::Cancelling && entry.close.is_closed() {
                return ResourcesCancelSignal::Expired;
            }
            let result = match entry.phase {
                ResourcesTaskPhase::Open | ResourcesTaskPhase::Pulling => {
                    entry.phase = ResourcesTaskPhase::Cancelling;
                    ResourcesCancelSignal::Won
                }
                ResourcesTaskPhase::Cancelling => ResourcesCancelSignal::AlreadyCancelling,
                ResourcesTaskPhase::Terminal => ResourcesCancelSignal::Missing,
            };
            (result, entry.close.clone())
        };
        close.close();
        result
    }

    pub(crate) fn consume_cancel(&self, control: &FeatureTaskControl) -> ResourcesCancelConsume {
        let Ok(key) = self.key(control) else {
            return ResourcesCancelConsume::WrongGeneration;
        };
        let mut entries = self.entries.lock().expect("Resources task sentinel lock");
        let outcome = match entries.get(&key) {
            Some(entry) if entry.phase == ResourcesTaskPhase::Cancelling => {
                ResourcesCancelConsume::Consumed
            }
            Some(entry) if entry.close.is_closed() => ResourcesCancelConsume::Expired,
            Some(_) | None => ResourcesCancelConsume::Missing,
        };
        if matches!(
            outcome,
            ResourcesCancelConsume::Consumed | ResourcesCancelConsume::Expired
        ) {
            entries
                .remove(&key)
                .expect("consumable Resources task remains registered");
        }
        outcome
    }

    pub(crate) fn is_cancelling(&self, control: &FeatureTaskControl) -> bool {
        let Ok(key) = self.key(control) else {
            return false;
        };
        self.entries
            .lock()
            .expect("Resources task sentinel lock")
            .get(&key)
            .is_some_and(|entry| entry.phase == ResourcesTaskPhase::Cancelling)
    }

    pub(crate) fn invalidate_open(&self) -> usize {
        let invalidated = {
            let entries = self.entries.lock().expect("Resources task sentinel lock");
            entries
                .values()
                .filter(|entry| {
                    entry.phase != ResourcesTaskPhase::Cancelling && !entry.close.is_closed()
                })
                .map(|entry| entry.close.clone())
                .collect::<Vec<_>>()
        };
        let count = invalidated.len();
        for close in invalidated {
            close.close();
        }
        count
    }

    pub(crate) fn remove(&self, control: &FeatureTaskControl) -> bool {
        let Ok(key) = self.key(control) else {
            return false;
        };
        self.entries
            .lock()
            .expect("Resources task sentinel lock")
            .remove(&key)
            .is_some()
    }

    pub(crate) fn retire(&self) {
        let entries = {
            let mut retained = self.entries.lock().expect("Resources task sentinel lock");
            std::mem::take(&mut *retained)
        };
        for entry in entries.into_values() {
            entry.close.close();
        }
    }

    fn key(
        &self,
        control: &FeatureTaskControl,
    ) -> Result<ResourcesTaskKey, ResourcesTaskRegisterError> {
        if control.generation() != self.generation {
            return Err(ResourcesTaskRegisterError::WrongGeneration);
        }
        Ok(ResourcesTaskKey {
            operation_id: control.operation_id().clone(),
            task_id: control.task_id().clone(),
            generation: control.generation(),
        })
    }
}

fn remove_closed_non_cancelling(
    entries: &mut HashMap<ResourcesTaskKey, ResourcesTaskEntry>,
    key: &ResourcesTaskKey,
) -> bool {
    let closed = entries.get(key).is_some_and(|entry| {
        entry.phase != ResourcesTaskPhase::Cancelling && entry.close.is_closed()
    });
    if closed {
        entries
            .remove(key)
            .expect("closed Resources task remains registered");
    }
    closed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn generation(value: u64) -> TaskGeneration {
        TaskGeneration::new(value).expect("task generation")
    }

    fn control(task: &str, generation: u64) -> FeatureTaskControl {
        FeatureTaskControl::new(
            OperationId::parse("read").expect("operation ID"),
            TaskId::parse(task).expect("task ID"),
            self::generation(generation),
        )
    }

    #[test]
    fn cancel_winner_suppresses_a_late_pull_and_leaves_one_tombstone() {
        let sentinel = ResourcesTaskSentinel::new(generation(7));
        let task = control("task1-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", 7);
        let close = ResourcesCloseSignal::new();
        sentinel
            .register(&task, close.clone())
            .expect("register task");

        assert_eq!(sentinel.begin_pull(&task), ResourcesPullStart::Started);
        assert_eq!(sentinel.signal_cancel(&task), ResourcesCancelSignal::Won);
        assert!(close.is_closed());
        assert_eq!(
            sentinel.finish_terminal(&task),
            ResourcesPullFinish::Cancelled
        );
        assert_eq!(
            sentinel.consume_cancel(&task),
            ResourcesCancelConsume::Consumed
        );
        assert_eq!(
            sentinel.consume_cancel(&task),
            ResourcesCancelConsume::Missing
        );
    }

    #[test]
    fn terminal_winner_removes_the_exact_task_before_cancel() {
        let sentinel = ResourcesTaskSentinel::new(generation(7));
        let task = control("task1-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", 7);
        sentinel
            .register(&task, ResourcesCloseSignal::new())
            .expect("register task");

        assert_eq!(sentinel.begin_pull(&task), ResourcesPullStart::Started);
        assert_eq!(
            sentinel.finish_terminal(&task),
            ResourcesPullFinish::Publish
        );
        assert_eq!(
            sentinel.signal_cancel(&task),
            ResourcesCancelSignal::Missing
        );
    }

    #[test]
    fn crossed_generation_never_mutates_the_registered_task() {
        let sentinel = ResourcesTaskSentinel::new(generation(7));
        let task = control("task1-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", 7);
        let crossed = control("task1-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", 8);
        sentinel
            .register(&task, ResourcesCloseSignal::new())
            .expect("register task");

        assert_eq!(
            sentinel.signal_cancel(&crossed),
            ResourcesCancelSignal::WrongGeneration
        );
        assert_eq!(sentinel.begin_pull(&task), ResourcesPullStart::Started);
        assert_eq!(
            sentinel.finish_nonterminal(&task),
            ResourcesPullFinish::Publish
        );
    }

    #[test]
    fn invalidation_closes_open_work_but_preserves_cancel_tombstones() {
        let sentinel = ResourcesTaskSentinel::new(generation(7));
        let open = control("task1-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", 7);
        let cancelling = control("task1-bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb", 7);
        let open_close = ResourcesCloseSignal::new();
        let cancelling_close = ResourcesCloseSignal::new();
        sentinel
            .register(&open, open_close.clone())
            .expect("register open task");
        sentinel
            .register(&cancelling, cancelling_close.clone())
            .expect("register cancelling task");
        assert_eq!(
            sentinel.signal_cancel(&cancelling),
            ResourcesCancelSignal::Won
        );

        assert_eq!(sentinel.invalidate_open(), 1);
        assert!(open_close.is_closed());
        assert!(cancelling_close.is_closed());
        assert!(sentinel.is_cancelling(&cancelling));
        assert_eq!(sentinel.begin_pull(&open), ResourcesPullStart::Expired);
        assert_eq!(
            sentinel.consume_cancel(&cancelling),
            ResourcesCancelConsume::Consumed
        );
    }

    #[test]
    fn cancel_after_deadline_cannot_create_a_cancel_tombstone() {
        let sentinel = ResourcesTaskSentinel::new(generation(7));
        let task = control("task1-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", 7);
        let close = ResourcesCloseSignal::new();
        sentinel
            .register(&task, close.clone())
            .expect("register task");
        assert!(close.close());

        assert_eq!(
            sentinel.signal_cancel(&task),
            ResourcesCancelSignal::Expired
        );
        assert_eq!(
            sentinel.consume_cancel(&task),
            ResourcesCancelConsume::Expired
        );
    }

    #[test]
    fn deadline_is_published_once_before_the_task_becomes_unknown() {
        let sentinel = ResourcesTaskSentinel::new(generation(7));
        let task = control("task1-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", 7);
        let close = ResourcesCloseSignal::new();
        sentinel
            .register(&task, close.clone())
            .expect("register task");
        assert!(close.close());

        assert_eq!(sentinel.begin_pull(&task), ResourcesPullStart::Expired);
        assert_eq!(sentinel.begin_pull(&task), ResourcesPullStart::Missing);
    }

    #[test]
    fn abandoned_closed_tombstones_do_not_consume_live_task_capacity() {
        let sentinel = ResourcesTaskSentinel::new(generation(7));
        for value in 0..MAX_OPEN_OPERATIONS {
            let task = control(&format!("task1-{value:032x}"), 7);
            let close = ResourcesCloseSignal::new();
            sentinel
                .register(&task, close.clone())
                .expect("register task");
            assert!(close.close());
        }

        let replacement = control("task1-ffffffffffffffffffffffffffffffff", 7);
        sentinel
            .register(&replacement, ResourcesCloseSignal::new())
            .expect("closed tombstones release live task capacity");
        assert_eq!(
            sentinel.begin_pull(&replacement),
            ResourcesPullStart::Started
        );
    }
}

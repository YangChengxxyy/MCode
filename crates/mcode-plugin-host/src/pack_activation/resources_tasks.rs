//! Bounded Host task identities retained by one active Resources Pack set.

// Rust guideline compliant 2026-08-31.

use std::collections::HashMap;

use mcode_plugin_api::{OperationId, ResourcesTaskRequest, TaskGeneration, TaskId};
use tokio::time::Instant;

use crate::runtime::MAX_OPEN_OPERATIONS;

const TASK_ID_PREFIX: &str = "task1-";
const TASK_ID_RANDOM_BYTES: usize = 16;
const TASK_ID_MINT_ATTEMPTS: usize = 8;
const MAX_RESOURCES_PULLS: u32 = 65_536;
const LOWER_HEX: &[u8; 16] = b"0123456789abcdef";

type RandomFill = dyn Fn(&mut [u8; TASK_ID_RANDOM_BYTES]) -> Result<(), ()> + Send + Sync;

pub(super) struct ResourcesTaskTable<T> {
    rows: HashMap<TaskId, ResourcesTaskRow<T>>,
    random_fill: Box<RandomFill>,
}

pub(super) struct ResourcesTaskRow<T> {
    operation_id: OperationId,
    generation: TaskGeneration,
    request: ResourcesTaskRequest,
    deadline: Instant,
    pull_count: u32,
    progress_emitted: bool,
    operation: T,
}

impl<T> ResourcesTaskTable<T> {
    pub(super) fn new() -> Self {
        Self::with_random(|bytes| getrandom::fill(bytes).map_err(|_| ()))
    }

    fn with_random(
        random_fill: impl Fn(&mut [u8; TASK_ID_RANDOM_BYTES]) -> Result<(), ()> + Send + Sync + 'static,
    ) -> Self {
        Self {
            rows: HashMap::new(),
            random_fill: Box::new(random_fill),
        }
    }

    pub(super) fn mint(&self) -> Result<TaskId, ResourcesTaskTableError> {
        if self.rows.len() >= MAX_OPEN_OPERATIONS {
            return Err(ResourcesTaskTableError::Limit);
        }
        for _ in 0..TASK_ID_MINT_ATTEMPTS {
            let mut random = [0_u8; TASK_ID_RANDOM_BYTES];
            (self.random_fill)(&mut random).map_err(|()| ResourcesTaskTableError::Unavailable)?;
            let task_id = encode_task_id(random);
            if !self.rows.contains_key(&task_id) {
                return Ok(task_id);
            }
        }
        Err(ResourcesTaskTableError::Unavailable)
    }

    pub(super) fn insert(
        &mut self,
        task_id: TaskId,
        operation_id: OperationId,
        generation: TaskGeneration,
        request: ResourcesTaskRequest,
        deadline: Instant,
        operation: T,
    ) {
        let previous = self.rows.insert(
            task_id,
            ResourcesTaskRow {
                operation_id,
                generation,
                request,
                deadline,
                pull_count: 0,
                progress_emitted: false,
                operation,
            },
        );
        debug_assert!(previous.is_none(), "a minted task ID is inserted once");
    }

    pub(super) fn get_mut(
        &mut self,
        task_id: &TaskId,
        operation_id: &OperationId,
        generation: TaskGeneration,
    ) -> Option<&mut ResourcesTaskRow<T>> {
        self.rows
            .get_mut(task_id)
            .filter(|row| row.operation_id == *operation_id && row.generation == generation)
    }

    pub(super) fn remove(
        &mut self,
        task_id: &TaskId,
        operation_id: &OperationId,
        generation: TaskGeneration,
    ) -> Option<ResourcesTaskRow<T>> {
        let matches = self
            .rows
            .get(task_id)
            .is_some_and(|row| row.operation_id == *operation_id && row.generation == generation);
        matches.then(|| {
            self.rows
                .remove(task_id)
                .expect("the exact checked Resources task remains present")
        })
    }

    pub(super) fn retain(&mut self, mut keep: impl FnMut(&ResourcesTaskRow<T>) -> bool) {
        self.rows.retain(|_, row| keep(row));
    }
}

impl<T> ResourcesTaskRow<T> {
    pub(super) const fn request(&self) -> &ResourcesTaskRequest {
        &self.request
    }

    pub(super) const fn deadline(&self) -> Instant {
        self.deadline
    }

    pub(super) fn reserve_pull(&mut self) -> Result<(), ()> {
        if self.pull_count >= MAX_RESOURCES_PULLS {
            return Err(());
        }
        self.pull_count += 1;
        Ok(())
    }

    pub(super) fn accept_progress(&mut self) -> Result<(), ()> {
        if self.progress_emitted {
            return Err(());
        }
        self.progress_emitted = true;
        Ok(())
    }

    pub(super) fn into_operation(self) -> T {
        self.operation
    }
}

impl<T: Copy> ResourcesTaskRow<T> {
    pub(super) const fn operation(&self) -> T {
        self.operation
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ResourcesTaskTableError {
    Limit,
    Unavailable,
}

fn encode_task_id(random: [u8; TASK_ID_RANDOM_BYTES]) -> TaskId {
    let mut value = String::with_capacity(TASK_ID_PREFIX.len() + 2 * TASK_ID_RANDOM_BYTES);
    value.push_str(TASK_ID_PREFIX);
    for byte in random {
        value.push(LOWER_HEX[usize::from(byte >> 4)] as char);
        value.push(LOWER_HEX[usize::from(byte & 0x0f)] as char);
    }
    TaskId::parse(value).expect("Host-generated task ID is canonical")
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    fn request() -> ResourcesTaskRequest {
        ResourcesTaskRequest::Contributions
    }

    fn deadline() -> Instant {
        Instant::now() + std::time::Duration::from_secs(1)
    }

    fn operation(value: &str) -> OperationId {
        OperationId::parse(value).expect("operation ID")
    }

    fn generation(value: u64) -> TaskGeneration {
        TaskGeneration::new(value).expect("task generation")
    }

    #[test]
    fn task_identity_is_host_minted_and_bound_to_complete_control_identity() {
        let next = Arc::new(AtomicUsize::new(1));
        let random = Arc::clone(&next);
        let mut table = ResourcesTaskTable::with_random(move |bytes| {
            bytes.fill(random.fetch_add(1, Ordering::Relaxed) as u8);
            Ok(())
        });
        let task_id = table.mint().expect("task identity");
        table.insert(
            task_id.clone(),
            operation("read"),
            generation(7),
            request(),
            deadline(),
            41_u8,
        );

        assert_eq!(
            table
                .get_mut(&task_id, &operation("read"), generation(7))
                .map(|row| row.operation()),
            Some(41)
        );
        assert!(
            table
                .get_mut(&task_id, &operation("catalog"), generation(7))
                .is_none()
        );
        assert!(
            table
                .get_mut(&task_id, &operation("read"), generation(8))
                .is_none()
        );
        assert_eq!(
            table
                .remove(&task_id, &operation("read"), generation(7))
                .map(ResourcesTaskRow::into_operation),
            Some(41)
        );
        assert!(
            table
                .remove(&task_id, &operation("read"), generation(7))
                .is_none()
        );
    }

    #[test]
    fn task_mint_is_bounded_and_collision_failure_does_not_replace_a_live_row() {
        let mut table = ResourcesTaskTable::with_random(|bytes| {
            bytes.fill(0xa5);
            Ok(())
        });
        let task_id = table.mint().expect("first identity");
        table.insert(
            task_id.clone(),
            operation("read"),
            generation(7),
            request(),
            deadline(),
            11_u8,
        );

        assert_eq!(table.mint(), Err(ResourcesTaskTableError::Unavailable));
        assert_eq!(
            table
                .remove(&task_id, &operation("read"), generation(7))
                .map(ResourcesTaskRow::into_operation),
            Some(11)
        );
    }

    #[test]
    fn task_capacity_rejects_before_random_or_row_mutation() {
        let calls = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&calls);
        let mut table = ResourcesTaskTable::with_random(move |bytes| {
            let value = observed.fetch_add(1, Ordering::Relaxed);
            bytes[..usize::BITS as usize / 8].copy_from_slice(&value.to_le_bytes());
            Ok(())
        });
        for value in 0..MAX_OPEN_OPERATIONS {
            let task_id = table.mint().expect("within task capacity");
            table.insert(
                task_id,
                operation("read"),
                generation(7),
                request(),
                deadline(),
                value,
            );
        }

        assert_eq!(table.mint(), Err(ResourcesTaskTableError::Limit));
        assert_eq!(calls.load(Ordering::Relaxed), MAX_OPEN_OPERATIONS);
    }

    #[test]
    fn pull_and_progress_limits_reject_before_an_extra_guest_entry() {
        let mut table = ResourcesTaskTable::with_random(|bytes| {
            bytes.fill(0x5a);
            Ok(())
        });
        let task_id = table.mint().expect("task identity");
        table.insert(
            task_id.clone(),
            operation("read"),
            generation(7),
            request(),
            deadline(),
            (),
        );
        let row = table
            .get_mut(&task_id, &operation("read"), generation(7))
            .expect("task row");
        row.pull_count = MAX_RESOURCES_PULLS - 1;

        assert_eq!(row.reserve_pull(), Ok(()));
        assert_eq!(row.reserve_pull(), Err(()));
        assert_eq!(row.accept_progress(), Ok(()));
        assert_eq!(row.accept_progress(), Err(()));
    }
}

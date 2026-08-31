//! Serialized Resources Pack owner loop and non-blocking operation cleanup.

// Rust guideline compliant 2026-08-31.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use mcode_plugin_api::ResourcesTaskRequest;
use tokio::sync::{Notify, mpsc, oneshot};
use tokio::time::{Instant, timeout_at};

use super::resources::ResourcesOperationAdmission;
use super::{ResourcesOperation, ResourcesPackActor, ResourcesPackCallError, ResourcesPackPull};

const COMMAND_CAPACITY: usize = super::MAX_OPEN_OPERATIONS + 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct ResourcesActorOperationId(u64);

pub(crate) struct ResourcesActorClient {
    sender: mpsc::Sender<Command>,
    worker: tokio::task::AbortHandle,
    live: Arc<Mutex<HashMap<ResourcesActorOperationId, ResourcesCloseSignal>>>,
}

enum Command {
    Invoke {
        invocation: InvokeCommand,
        reply: oneshot::Sender<Result<ResourcesActorOperationId, ResourcesActorError>>,
    },
    Pull {
        operation: ResourcesActorOperationId,
        reply: oneshot::Sender<Result<ResourcesPackPull, ResourcesActorError>>,
    },
    Close(ResourcesActorOperationId),
}

struct InvokeCommand {
    request: ResourcesTaskRequest,
    deadline: Instant,
    close: ResourcesCloseSignal,
}

struct WorkerOperation {
    deadline: Instant,
    operation: ResourcesOperation,
    close: ResourcesCloseSignal,
}

#[derive(Clone)]
pub(crate) struct ResourcesCloseSignal {
    state: Arc<ResourcesCloseState>,
}

struct ResourcesCloseState {
    closed: AtomicBool,
    notification: Notify,
    admission: Mutex<Option<ResourcesOperationAdmission>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ResourcesActorError {
    UnknownOperation,
    Pack(ResourcesPackCallError),
    Unavailable,
}

impl ResourcesActorClient {
    pub(crate) fn start(actor: ResourcesPackActor) -> Self {
        let (sender, receiver) = mpsc::channel(COMMAND_CAPACITY);
        let live = Arc::new(Mutex::new(HashMap::new()));
        let worker = tokio::spawn(run_worker(
            actor,
            receiver,
            sender.clone(),
            Arc::clone(&live),
        ))
        .abort_handle();
        Self {
            sender,
            worker,
            live,
        }
    }

    pub(crate) fn is_available(&self) -> bool {
        !self.sender.is_closed()
    }

    pub(crate) fn is_operation_open(&self, operation: ResourcesActorOperationId) -> bool {
        self.live
            .lock()
            .expect("Resources actor live-operation lock")
            .get(&operation)
            .is_some_and(|close| !close.is_closed())
    }

    pub(crate) async fn invoke(
        &self,
        request: ResourcesTaskRequest,
        deadline: Instant,
        close: ResourcesCloseSignal,
    ) -> Result<ResourcesActorOperationId, ResourcesActorError> {
        let (reply, response) = oneshot::channel();
        timeout_at(
            deadline,
            self.sender.send(Command::Invoke {
                invocation: InvokeCommand {
                    request,
                    deadline,
                    close,
                },
                reply,
            }),
        )
        .await
        .map_err(|_| ResourcesActorError::Unavailable)?
        .map_err(|_| ResourcesActorError::Unavailable)?;
        timeout_at(deadline, response)
            .await
            .map_err(|_| ResourcesActorError::Unavailable)?
            .map_err(|_| ResourcesActorError::Unavailable)?
    }

    pub(crate) async fn pull(
        &self,
        operation: ResourcesActorOperationId,
        deadline: Instant,
    ) -> Result<ResourcesPackPull, ResourcesActorError> {
        let (reply, response) = oneshot::channel();
        timeout_at(
            deadline,
            self.sender.send(Command::Pull { operation, reply }),
        )
        .await
        .map_err(|_| ResourcesActorError::Unavailable)?
        .map_err(|_| ResourcesActorError::Unavailable)?;
        timeout_at(deadline, response)
            .await
            .map_err(|_| ResourcesActorError::Unavailable)?
            .map_err(|_| ResourcesActorError::Unavailable)?
    }

    pub(crate) fn close(&self, operation: ResourcesActorOperationId) {
        if let Some(close) = self
            .live
            .lock()
            .expect("Resources actor live-operation lock")
            .get(&operation)
            .cloned()
        {
            close.close();
        }
    }
}

impl ResourcesCloseSignal {
    pub(crate) fn new() -> Self {
        Self {
            state: Arc::new(ResourcesCloseState {
                closed: AtomicBool::new(false),
                notification: Notify::new(),
                admission: Mutex::new(None),
            }),
        }
    }

    pub(crate) fn close(&self) -> bool {
        if self.state.closed.swap(true, Ordering::AcqRel) {
            return false;
        }
        drop(
            self.state
                .admission
                .lock()
                .expect("Resources close admission lock")
                .take(),
        );
        self.state.notification.notify_waiters();
        true
    }

    pub(crate) fn is_closed(&self) -> bool {
        self.state.closed.load(Ordering::Acquire)
    }

    pub(crate) async fn closed(&self) {
        if self.is_closed() {
            return;
        }
        let notification = self.state.notification.notified();
        tokio::pin!(notification);
        notification.as_mut().enable();
        if self.is_closed() {
            return;
        }
        notification.await;
    }

    fn bind_admission(&self, admission: ResourcesOperationAdmission) {
        let mut slot = self
            .state
            .admission
            .lock()
            .expect("Resources close admission lock");
        if self.is_closed() {
            drop(admission);
        } else {
            debug_assert!(slot.is_none());
            *slot = Some(admission);
        }
    }
}

impl Drop for ResourcesActorClient {
    fn drop(&mut self) {
        let live = self
            .live
            .lock()
            .expect("Resources actor live-operation lock")
            .values()
            .cloned()
            .collect::<Vec<_>>();
        for close in live {
            close.close();
        }
        self.worker.abort();
    }
}

async fn run_worker(
    mut actor: ResourcesPackActor,
    mut receiver: mpsc::Receiver<Command>,
    sender: mpsc::Sender<Command>,
    live: Arc<Mutex<HashMap<ResourcesActorOperationId, ResourcesCloseSignal>>>,
) {
    let mut next_operation = 1_u64;
    let mut operations = HashMap::new();
    while let Some(command) = receiver.recv().await {
        let keep_running = match command {
            Command::Invoke { invocation, reply } => {
                let result = invoke_operation(
                    &mut actor,
                    &mut operations,
                    &mut next_operation,
                    &sender,
                    &live,
                    invocation,
                )
                .await;
                match result {
                    Ok(operation) => {
                        if reply.send(Ok(operation)).is_err() {
                            close_operation(&mut actor, &mut operations, &live, operation).await
                        } else {
                            true
                        }
                    }
                    Err(error) => {
                        let keep_running = error != ResourcesActorError::Unavailable;
                        let _ = reply.send(Err(error));
                        keep_running
                    }
                }
            }
            Command::Pull { operation, reply } => {
                let result = pull_operation(&mut actor, &mut operations, operation).await;
                match result {
                    Ok(pull) => {
                        if reply.send(Ok(pull)).is_err() {
                            close_operation(&mut actor, &mut operations, &live, operation).await
                        } else {
                            true
                        }
                    }
                    Err(error) => {
                        let keep_running = error != ResourcesActorError::Unavailable;
                        let _ = reply.send(Err(error));
                        keep_running
                    }
                }
            }
            Command::Close(operation) => {
                close_operation(&mut actor, &mut operations, &live, operation).await
            }
        };
        if !keep_running || !actor.is_available() {
            break;
        }
    }
    for operation in operations.into_values() {
        operation.close.close();
    }
    live.lock()
        .expect("Resources actor live-operation lock")
        .clear();
}

async fn invoke_operation(
    actor: &mut ResourcesPackActor,
    operations: &mut HashMap<ResourcesActorOperationId, WorkerOperation>,
    next_operation: &mut u64,
    sender: &mpsc::Sender<Command>,
    live: &Mutex<HashMap<ResourcesActorOperationId, ResourcesCloseSignal>>,
    invocation: InvokeCommand,
) -> Result<ResourcesActorOperationId, ResourcesActorError> {
    let InvokeCommand {
        request,
        deadline,
        close,
    } = invocation;
    if *next_operation == u64::MAX {
        return Err(ResourcesActorError::Unavailable);
    }
    let invocation = timeout_at(deadline, actor.invoke(&request));
    tokio::pin!(invocation);
    let mut operation = tokio::select! {
        biased;
        () = close.closed() => return Err(ResourcesActorError::Unavailable),
        result = &mut invocation => result
            .map_err(|_| ResourcesActorError::Unavailable)?
            .map_err(map_pack_error)?,
    };
    let admission = operation
        .take_admission()
        .ok_or(ResourcesActorError::Unavailable)?;
    close.bind_admission(admission);
    if close.is_closed() {
        return Err(ResourcesActorError::Unavailable);
    }
    let id = ResourcesActorOperationId(*next_operation);
    *next_operation += 1;
    operations.insert(
        id,
        WorkerOperation {
            deadline,
            operation,
            close: close.clone(),
        },
    );
    live.lock()
        .expect("Resources actor live-operation lock")
        .insert(id, close.clone());
    let expiry = sender.clone();
    tokio::spawn(async move {
        tokio::select! {
            () = tokio::time::sleep_until(deadline) => {
                close.close();
                let _ = expiry.send(Command::Close(id)).await;
            }
            () = close.closed() => {
                let _ = expiry.send(Command::Close(id)).await;
            }
            () = expiry.closed() => {}
        }
    });
    Ok(id)
}

async fn pull_operation(
    actor: &mut ResourcesPackActor,
    operations: &mut HashMap<ResourcesActorOperationId, WorkerOperation>,
    id: ResourcesActorOperationId,
) -> Result<ResourcesPackPull, ResourcesActorError> {
    let operation = operations
        .get_mut(&id)
        .ok_or(ResourcesActorError::UnknownOperation)?;
    let pull = timeout_at(operation.deadline, actor.pull(&mut operation.operation));
    tokio::pin!(pull);
    tokio::select! {
        biased;
        () = operation.close.closed() => Err(ResourcesActorError::Unavailable),
        result = &mut pull => result
            .map_err(|_| ResourcesActorError::Unavailable)?
            .map_err(map_pack_error),
    }
}

async fn close_operation(
    actor: &mut ResourcesPackActor,
    operations: &mut HashMap<ResourcesActorOperationId, WorkerOperation>,
    live: &Mutex<HashMap<ResourcesActorOperationId, ResourcesCloseSignal>>,
    id: ResourcesActorOperationId,
) -> bool {
    let Some(operation) = operations.remove(&id) else {
        return true;
    };
    operation.close.close();
    live.lock()
        .expect("Resources actor live-operation lock")
        .remove(&id);
    actor.drop_operation(operation.operation).await.is_ok() && actor.is_available()
}

const fn map_pack_error(error: ResourcesPackCallError) -> ResourcesActorError {
    match error {
        ResourcesPackCallError::Runtime | ResourcesPackCallError::OperationMismatch => {
            ResourcesActorError::Unavailable
        }
        ResourcesPackCallError::Guest(_) => ResourcesActorError::Pack(error),
    }
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::pin::Pin;
    use std::task::{Context, Poll, Waker};

    use super::{ResourcesCloseSignal, ResourcesOperationAdmission};
    use crate::runtime::admission::AdmissionLedger;
    use crate::runtime::{AdmissionError, MAX_LIVE_RESOURCES, MAX_OPEN_OPERATIONS};

    fn poll_once<F: Future>(mut future: Pin<&mut F>) -> Poll<F::Output> {
        future
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
    }

    #[tokio::test]
    async fn close_before_the_first_wait_is_observed() {
        let close = ResourcesCloseSignal::new();
        assert!(close.close());

        close.closed().await;
        assert!(!close.close());
    }

    #[tokio::test]
    async fn close_wakes_an_enabled_waiter() {
        let close = ResourcesCloseSignal::new();
        let mut closed = Box::pin(close.closed());
        assert!(poll_once(closed.as_mut()).is_pending());

        assert!(close.close());
        closed.await;
    }

    #[test]
    fn close_releases_admission_before_asynchronous_guest_cleanup() {
        let admission = AdmissionLedger::new();
        let close = ResourcesCloseSignal::new();
        close.bind_admission(ResourcesOperationAdmission::new(
            admission.open_operation().expect("operation admission"),
            admission.admit_resource().expect("resource admission"),
        ));
        let operations = (1..MAX_OPEN_OPERATIONS)
            .map(|_| admission.open_operation().expect("remaining operation"))
            .collect::<Vec<_>>();
        let resources = (1..MAX_LIVE_RESOURCES)
            .map(|_| admission.admit_resource().expect("remaining resource"))
            .collect::<Vec<_>>();
        assert_eq!(
            admission.open_operation().err(),
            Some(AdmissionError::OperationCapacity)
        );
        assert_eq!(
            admission.admit_resource().err(),
            Some(AdmissionError::ResourceCapacity)
        );

        assert!(close.close());
        let replacement_operation = admission
            .open_operation()
            .expect("close synchronously releases operation admission");
        let replacement_resource = admission
            .admit_resource()
            .expect("close synchronously releases resource admission");

        drop((replacement_operation, replacement_resource));
        drop((operations, resources));
    }
}

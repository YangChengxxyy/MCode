//! Runs generation cleanup independently of caller Tokio runtimes.
//!
//! One persistent worker owns every retired-generation job in FIFO order, so
//! a caller may stop driving its runtime immediately after publication without
//! freezing cancellation, quiescence, or reconciliation serialization.

// Rust guideline compliant 2026-08-31.

use std::sync::Arc;

use tokio::sync::{OwnedMutexGuard, mpsc, oneshot};

use super::{ActiveGeneration, ReconciliationError, retire_generation_entries};

pub(super) struct CleanupWorker {
    sender: mpsc::UnboundedSender<CleanupJob>,
}

struct CleanupJob {
    retired: Vec<Arc<ActiveGeneration>>,
    serialized: Option<OwnedMutexGuard<()>>,
    completion: Option<oneshot::Sender<()>>,
    stop_after: bool,
}

impl CleanupWorker {
    pub(super) fn start() -> Result<Self, ReconciliationError> {
        let (sender, receiver) = mpsc::unbounded_channel();
        let (ready_sender, ready_receiver) = std::sync::mpsc::sync_channel(1);
        drop(
            std::thread::Builder::new()
                .name("mcode-manager-cleanup".to_owned())
                .spawn(move || run_cleanup_thread(receiver, ready_sender))
                .map_err(|_| ReconciliationError::Unavailable)?,
        );
        ready_receiver
            .recv()
            .map_err(|_| ReconciliationError::Unavailable)??;
        Ok(Self { sender })
    }

    pub(super) fn retire_after_publication(
        &self,
        retired: Vec<Arc<ActiveGeneration>>,
        serialized: OwnedMutexGuard<()>,
    ) -> Result<(), ReconciliationError> {
        self.send(CleanupJob {
            retired,
            serialized: Some(serialized),
            completion: None,
            stop_after: false,
        })
    }

    pub(super) fn retire_for_shutdown(
        &self,
        retired: Vec<Arc<ActiveGeneration>>,
        serialized: OwnedMutexGuard<()>,
    ) -> Result<oneshot::Receiver<()>, ReconciliationError> {
        let (completion, receiver) = oneshot::channel();
        self.send(CleanupJob {
            retired,
            serialized: Some(serialized),
            completion: Some(completion),
            stop_after: true,
        })?;
        Ok(receiver)
    }

    pub(super) fn retire_for_drop(&self, retired: Vec<Arc<ActiveGeneration>>) {
        let _ = self.send(CleanupJob {
            retired,
            serialized: None,
            completion: None,
            stop_after: true,
        });
    }

    pub(super) fn stop(&self) {
        let _ = self.send(CleanupJob {
            retired: Vec::new(),
            serialized: None,
            completion: None,
            stop_after: true,
        });
    }

    fn send(&self, job: CleanupJob) -> Result<(), ReconciliationError> {
        self.sender
            .send(job)
            .map_err(|_| ReconciliationError::Unavailable)
    }
}

fn run_cleanup_thread(
    receiver: mpsc::UnboundedReceiver<CleanupJob>,
    ready: std::sync::mpsc::SyncSender<Result<(), ReconciliationError>>,
) {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(_) => {
            let _ = ready.send(Err(ReconciliationError::Unavailable));
            return;
        }
    };
    if ready.send(Ok(())).is_err() {
        return;
    }
    runtime.block_on(run_cleanup_loop(receiver));
}

async fn run_cleanup_loop(mut receiver: mpsc::UnboundedReceiver<CleanupJob>) {
    while let Some(job) = receiver.recv().await {
        retire_generation_entries(job.retired).await;
        drop(job.serialized);
        if let Some(completion) = job.completion {
            let _ = completion.send(());
        }
        if job.stop_after {
            return;
        }
    }
}

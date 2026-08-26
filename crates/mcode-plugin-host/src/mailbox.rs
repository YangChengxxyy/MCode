//! Bounded mailbox for invoke, event, and render jobs.

// Rust guideline compliant 2026-08-26.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};

use crate::error::HostError;

/// Result of a nonblocking mailbox submission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventDelivery {
    /// Job was queued.
    Queued,
    /// Bounded mailbox was full.
    Full,
    /// Runtime is not accepting jobs.
    Closed,
    /// Job belongs to an old plugin generation.
    Stale,
}

pub(crate) struct Admission {
    open: AtomicBool,
    generation: AtomicU64,
}

impl Admission {
    pub(crate) fn new(generation: u64) -> Self {
        Self {
            open: AtomicBool::new(true),
            generation: AtomicU64::new(generation),
        }
    }

    pub(crate) fn is_open(&self) -> bool {
        self.open.load(Ordering::Acquire)
    }

    pub(crate) fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    pub(crate) fn close(&self) {
        self.open.store(false, Ordering::Release);
    }
}

pub(crate) enum Job {
    Invoke {
        generation: u64,
        request: String,
        reply: SyncSender<Result<String, HostError>>,
        cancelled: Arc<AtomicBool>,
    },
    Event {
        generation: u64,
        payload: String,
    },
    Render {
        generation: u64,
        request: String,
        reply: SyncSender<Result<String, HostError>>,
        cancelled: Arc<AtomicBool>,
    },
}

impl Job {
    fn generation(&self) -> u64 {
        match self {
            Self::Invoke { generation, .. }
            | Self::Event { generation, .. }
            | Self::Render { generation, .. } => *generation,
        }
    }

    pub(crate) fn payload_bytes(&self) -> usize {
        match self {
            Self::Invoke { request, .. } | Self::Render { request, .. } => request.len(),
            Self::Event { payload, .. } => payload.len(),
        }
    }
}

#[derive(Clone)]
pub(crate) struct MailboxSender {
    tx: Arc<Mutex<Option<SyncSender<Job>>>>,
    admission: Arc<Admission>,
    queued_bytes: Arc<AtomicUsize>,
    max_bytes: usize,
}

impl MailboxSender {
    pub(crate) fn try_enqueue(&self, job: Job) -> EventDelivery {
        if !self.admission.is_open() {
            return EventDelivery::Closed;
        }
        if job.generation() != self.admission.generation() {
            return EventDelivery::Stale;
        }
        let bytes = job.payload_bytes();
        if !self.try_account(bytes) {
            return EventDelivery::Full;
        }
        let sender = self
            .tx
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(tx) = sender.as_ref() else {
            self.release(bytes);
            return EventDelivery::Closed;
        };
        match tx.try_send(job) {
            Ok(()) => EventDelivery::Queued,
            Err(TrySendError::Full(job)) => {
                self.release(job.payload_bytes());
                EventDelivery::Full
            }
            Err(TrySendError::Disconnected(_)) => {
                self.release(bytes);
                EventDelivery::Closed
            }
        }
    }

    pub(crate) fn disconnect(&self) {
        *self
            .tx
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    }

    fn try_account(&self, bytes: usize) -> bool {
        loop {
            let current = self.queued_bytes.load(Ordering::Acquire);
            let Some(next) = current.checked_add(bytes) else {
                return false;
            };
            if next > self.max_bytes {
                return false;
            }
            if self
                .queued_bytes
                .compare_exchange(current, next, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                return true;
            }
        }
    }

    pub(crate) fn release(&self, bytes: usize) {
        self.queued_bytes.fetch_sub(bytes, Ordering::AcqRel);
    }
}

pub(crate) fn channel(
    capacity: usize,
    max_bytes: usize,
    admission: Arc<Admission>,
) -> (MailboxSender, Receiver<Job>, Arc<AtomicUsize>) {
    let (tx, rx) = mpsc::sync_channel(capacity);
    let queued_bytes = Arc::new(AtomicUsize::new(0));
    (
        MailboxSender {
            tx: Arc::new(Mutex::new(Some(tx))),
            admission,
            queued_bytes: queued_bytes.clone(),
            max_bytes,
        },
        rx,
        queued_bytes,
    )
}

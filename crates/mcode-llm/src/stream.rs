//! `EventStream` — the push/async-iterator channel that carries
//! [`StreamEvent`]s from a provider to its consumer.
//!
//! Mirrors pi's `EventStream`: producers push events; consumers iterate
//! asynchronously. The stream terminates after yielding `Done` or `Error`
//! (or when all senders are dropped), and — when constructed with a
//! cancellation token — as soon as the token fires while no events are
//! queued, yielding `Error(Cancelled)`.
//!
//! Backpressure is deliberately absent in M1 (unbounded channel); revisit
//! with a bounded channel + drop policy if memory ever becomes a problem
//! (design doc `07-m1-plan.md`, risk table).

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll};

use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio_stream::{Stream, StreamExt};
use tokio_util::sync::CancellationToken;

use crate::error::LlmError;
use crate::provider::StreamEvent;

/// Returns true for the terminal events that end a stream.
fn is_terminal(event: &StreamEvent) -> bool {
    matches!(event, StreamEvent::Done { .. } | StreamEvent::Error(_))
}

/// Producer half of an [`EventStream`]. Cloneable; safe to share between
/// tasks. Once a terminal event (`Done`/`Error`) has been pushed, further
/// pushes are ignored.
#[derive(Clone)]
pub struct EventStreamSender {
    tx: UnboundedSender<StreamEvent>,
    done: Arc<AtomicBool>,
}

impl EventStreamSender {
    /// Push an event onto the stream. Returns `false` (and drops the
    /// event) when the stream already terminated or the receiver is gone,
    /// so producers can stop early.
    pub fn push(&self, event: StreamEvent) -> bool {
        if self.done.load(Ordering::Acquire) {
            return false;
        }
        if is_terminal(&event) {
            self.done.store(true, Ordering::Release);
        }
        self.tx.send(event).is_ok()
    }

    /// Whether a terminal event has been pushed.
    pub fn is_done(&self) -> bool {
        self.done.load(Ordering::Acquire) || self.tx.is_closed()
    }

    /// Resolves once the receiving [`EventStream`] is gone, letting
    /// producers abandon work (e.g. stop reading a network response)
    /// whose consumer disappeared.
    pub fn closed(&self) -> impl Future<Output = ()> + '_ {
        self.tx.closed()
    }
}

/// Consumer half: an async iterator of [`StreamEvent`]s.
///
/// * Terminates after yielding `Done` or `Error`.
/// * Terminates when all senders are dropped (even without a terminal
///   event — a producer that exited early must not hang its consumer).
/// * If built with a cancellation token, terminates promptly once the
///   token fires and the queue is drained, yielding
///   `Error(Cancelled)` first. Queued events (including a queued `Done`)
///   are drained before cancellation is considered, so a stream that
///   completed successfully is not retroactively turned into a
///   cancellation.
#[derive(Debug)]
pub struct EventStream {
    rx: UnboundedReceiver<StreamEvent>,
    cancel: Option<CancellationToken>,
    done: bool,
}

impl EventStream {
    /// Create an uncancellable channel.
    pub fn channel() -> (EventStreamSender, Self) {
        Self::channel_inner(None)
    }

    /// Create a channel whose consumer side stops when `cancel` fires.
    pub fn channel_with_cancel(cancel: CancellationToken) -> (EventStreamSender, Self) {
        Self::channel_inner(Some(cancel))
    }

    fn channel_inner(cancel: Option<CancellationToken>) -> (EventStreamSender, Self) {
        let (tx, rx) = mpsc::unbounded_channel();
        (
            EventStreamSender {
                tx,
                done: Arc::new(AtomicBool::new(false)),
            },
            Self {
                rx,
                cancel,
                done: false,
            },
        )
    }

    /// Consume the stream and return the final assistant message, or the
    /// error that ended it. The canonical consumer fold: deltas can be
    /// observed by iterating manually when UIs need live updates.
    pub async fn into_final_message(self) -> Result<mcode_core::AssistantMessage, LlmError> {
        let mut stream = self;
        let mut saw_terminal = false;
        let mut result = Err(LlmError::Sse("stream ended without Done".into()));
        while let Some(event) = stream.next().await {
            match event {
                StreamEvent::Done { message } => {
                    result = Ok(message);
                    saw_terminal = true;
                    break;
                }
                StreamEvent::Error(err) => {
                    result = Err(err);
                    saw_terminal = true;
                    break;
                }
                _ => {}
            }
        }
        let _ = saw_terminal;
        result
    }
}

impl Stream for EventStream {
    type Item = StreamEvent;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<StreamEvent>> {
        let this = self.get_mut();
        if this.done {
            return Poll::Ready(None);
        }

        // Deliver anything already queued first (see the cancellation
        // note on the type).
        if let Poll::Ready(option) = this.rx.poll_recv(cx) {
            return Poll::Ready(match option {
                Some(event) => {
                    if is_terminal(&event) {
                        this.done = true;
                    }
                    Some(event)
                }
                None => {
                    this.done = true;
                    None
                }
            });
        }

        // Nothing queued: wait on the channel, or on cancellation.
        if let Some(cancel) = &this.cancel {
            let cancelled = cancel.cancelled();
            tokio::pin!(cancelled);
            if cancelled.poll(cx).is_ready() {
                this.done = true;
                return Poll::Ready(Some(StreamEvent::Error(LlmError::Cancelled)));
            }
        }
        Poll::Pending
    }
}

// All fields are `Unpin`, so the stream can be iterated via
// `StreamExt::next` without pinning gymnastics.
impl Unpin for EventStream {}

#[cfg(test)]
mod tests {
    use super::*;
    use mcode_core::message::{AssistantMessage, ContentBlock, StopReason};

    fn done_event() -> StreamEvent {
        StreamEvent::Done {
            message: AssistantMessage {
                blocks: vec![ContentBlock::Text("hi".into())],
                usage: None,
                stop_reason: StopReason::Stop,
            },
        }
    }

    #[tokio::test]
    async fn yields_all_pushed_events_in_order() {
        let (tx, mut stream) = EventStream::channel();
        tx.push(StreamEvent::Start);
        tx.push(StreamEvent::TextDelta("a".into()));
        tx.push(StreamEvent::TextDelta("b".into()));
        drop(tx);

        let mut collected = Vec::new();
        while let Some(event) = stream.next().await {
            collected.push(event);
        }
        assert_eq!(collected.len(), 3);
        assert_eq!(collected[0], StreamEvent::Start);
        assert_eq!(collected[1], StreamEvent::TextDelta("a".into()));
        assert_eq!(collected[2], StreamEvent::TextDelta("b".into()));
    }

    #[tokio::test]
    async fn terminates_after_done_and_ignores_later_pushes() {
        let (tx, mut stream) = EventStream::channel();
        tx.push(StreamEvent::Start);
        assert!(tx.push(done_event()));
        // Pushes after the terminal event are ignored.
        assert!(!tx.push(StreamEvent::TextDelta("late".into())));
        assert!(tx.is_done());

        let mut collected = Vec::new();
        while let Some(event) = stream.next().await {
            collected.push(event);
        }
        assert_eq!(collected.len(), 2);
        assert!(matches!(collected[1], StreamEvent::Done { .. }));
    }

    #[tokio::test]
    async fn error_propagates_and_terminates() {
        let (tx, mut stream) = EventStream::channel();
        tx.push(StreamEvent::TextDelta("partial".into()));
        tx.push(StreamEvent::Error(LlmError::Http {
            status: 500,
            body: "boom".into(),
        }));

        let first = stream.next().await;
        assert_eq!(first, Some(StreamEvent::TextDelta("partial".into())));
        let second = stream.next().await;
        assert!(matches!(
            second,
            Some(StreamEvent::Error(LlmError::Http { status: 500, .. }))
        ));
        assert!(stream.next().await.is_none());
    }

    #[tokio::test]
    async fn ends_when_all_senders_drop_without_terminal_event() {
        let (tx, mut stream) = EventStream::channel();
        tx.push(StreamEvent::TextDelta("x".into()));
        drop(tx);
        assert_eq!(
            stream.next().await,
            Some(StreamEvent::TextDelta("x".into()))
        );
        assert!(stream.next().await.is_none());
    }

    #[tokio::test]
    async fn cancel_token_stops_iteration_when_idle() {
        let cancel = CancellationToken::new();
        let (tx, mut stream) = EventStream::channel_with_cancel(cancel.clone());
        tx.push(StreamEvent::Start);
        cancel.cancel();

        // Queued events drain first…
        assert_eq!(stream.next().await, Some(StreamEvent::Start));
        // …then cancellation surfaces as an Error and ends the stream.
        assert_eq!(
            stream.next().await,
            Some(StreamEvent::Error(LlmError::Cancelled))
        );
        assert!(stream.next().await.is_none());
    }

    #[tokio::test]
    async fn cancel_after_done_does_not_retroactively_fail() {
        let cancel = CancellationToken::new();
        let (tx, mut stream) = EventStream::channel_with_cancel(cancel.clone());
        tx.push(done_event());
        cancel.cancel();

        assert!(matches!(
            stream.next().await,
            Some(StreamEvent::Done { .. })
        ));
        assert!(stream.next().await.is_none());
    }

    #[tokio::test]
    async fn into_final_message_returns_done_payload() {
        let (tx, stream) = EventStream::channel();
        tx.push(StreamEvent::Start);
        tx.push(StreamEvent::TextDelta("hi".into()));
        tx.push(done_event());
        let message = stream.into_final_message().await.unwrap();
        assert_eq!(message.stop_reason, StopReason::Stop);
        assert_eq!(message.blocks.len(), 1);
    }

    #[tokio::test]
    async fn into_final_message_propagates_error() {
        let (tx, stream) = EventStream::channel();
        tx.push(StreamEvent::Error(LlmError::Timeout));
        let err = stream.into_final_message().await.unwrap_err();
        assert_eq!(err, LlmError::Timeout);
    }
}

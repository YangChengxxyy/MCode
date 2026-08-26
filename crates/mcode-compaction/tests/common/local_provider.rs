//! Test-local scripted provider. Not part of the production crate.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use mcode_core::message::{AssistantMessage, ContentBlock};
use mcode_llm::{
    CancellationToken, EventStream, EventStreamSender, LlmError, Provider, Request, StreamEvent,
};

const SHARD_CHARS: usize = 16;

/// One scripted turn: stream a message or fail.
#[derive(Debug, Clone, PartialEq)]
pub enum LocalTurn {
    /// Stream this assistant message as one full turn.
    Message(AssistantMessage),
    /// Terminate the turn with this error.
    Fail(LlmError),
}

/// In-process [`Provider`] that replays a fixed sequence of turns.
#[derive(Debug)]
pub struct LocalProvider {
    id: String,
    turns: Arc<Mutex<VecDeque<LocalTurn>>>,
    requests: Arc<Mutex<Vec<Request>>>,
    delay: Duration,
}

impl LocalProvider {
    /// Builds from an inline script.
    pub fn new(turns: Vec<LocalTurn>) -> Self {
        Self {
            id: "local".into(),
            turns: Arc::new(Mutex::new(turns.into_iter().collect())),
            requests: Arc::new(Mutex::new(Vec::new())),
            delay: Duration::ZERO,
        }
    }

    /// Delay between emitted events (steer/abort test support).
    pub fn with_delay(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }

    /// All requests received so far, in order.
    pub fn recorded_requests(&self) -> Vec<Request> {
        self.requests.lock().expect("local requests lock").clone()
    }

    /// Number of unplayed turns left in the script.
    pub fn remaining_turns(&self) -> usize {
        self.turns.lock().expect("local turns lock").len()
    }
}

fn shard(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    chars
        .chunks(SHARD_CHARS)
        .map(|chunk| chunk.iter().collect())
        .collect()
}

async fn emit(
    tx: &EventStreamSender,
    event: StreamEvent,
    delay: Duration,
    cancel: &CancellationToken,
) -> bool {
    if delay > Duration::ZERO {
        tokio::select! {
            _ = tokio::time::sleep(delay) => {}
            _ = cancel.cancelled() => return false,
        }
    }
    tx.push(event)
}

#[async_trait]
impl Provider for LocalProvider {
    fn id(&self) -> &str {
        &self.id
    }

    async fn stream(
        &self,
        req: &Request,
        cancel: CancellationToken,
    ) -> Result<EventStream, LlmError> {
        {
            let mut requests = self.requests.lock().expect("local requests lock");
            requests.push(req.clone());
        }
        let turn = self
            .turns
            .lock()
            .expect("local turns lock")
            .pop_front()
            .ok_or_else(|| LlmError::Config("local provider script exhausted".into()))?;

        let (tx, stream) = EventStream::channel_with_cancel(cancel.clone());
        let delay = self.delay;
        tokio::spawn(async move {
            if !emit(&tx, StreamEvent::Start, delay, &cancel).await {
                return;
            }
            match turn {
                LocalTurn::Message(message) => {
                    for block in &message.blocks {
                        match block {
                            ContentBlock::Text(text) => {
                                for piece in shard(&text.text) {
                                    if !emit(&tx, StreamEvent::TextDelta(piece), delay, &cancel)
                                        .await
                                    {
                                        return;
                                    }
                                }
                            }
                            ContentBlock::Thinking(thinking) => {
                                for piece in shard(&thinking.text) {
                                    if !emit(&tx, StreamEvent::ThinkingDelta(piece), delay, &cancel)
                                        .await
                                    {
                                        return;
                                    }
                                }
                            }
                            ContentBlock::ToolCall(call) => {
                                let arguments = call.arguments.to_string();
                                for piece in shard(&arguments) {
                                    if !emit(
                                        &tx,
                                        StreamEvent::ToolCallDelta {
                                            id: call.id.clone(),
                                            partial_json: piece,
                                        },
                                        delay,
                                        &cancel,
                                    )
                                    .await
                                    {
                                        return;
                                    }
                                }
                                if !emit(
                                    &tx,
                                    StreamEvent::ToolCallEnd(call.clone()),
                                    delay,
                                    &cancel,
                                )
                                .await
                                {
                                    return;
                                }
                            }
                            ContentBlock::Image(_) => {}
                        }
                    }
                    tx.push(StreamEvent::Done { message });
                }
                LocalTurn::Fail(error) => {
                    tx.push(StreamEvent::Error(error));
                }
            }
        });
        Ok(stream)
    }
}

// Rust guideline compliant 2026-08-26

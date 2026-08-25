//! Session events and commands — the actor protocol between a session and
//! its observers (UI, telemetry). Skeleton per design doc `01-agent-core.md`
//! §4; the payloads here are the T1 skeleton shapes and will be refined as
//! `mcode-session` (M1 T5) and the permission engine (T3) land.
//!
//! `SessionEvent` must stay `Clone`: it is fan-out via `tokio::broadcast`,
//! which requires it.

use serde::{Deserialize, Serialize};

use crate::error::McodeError;
use crate::ids::{CallId, MessageId, SessionId};
use crate::message::{Message, ToolResultMessage};

/// Events emitted by a session actor; UIs and telemetry subscribe to these.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SessionEvent {
    /// A new turn started processing.
    TurnStarted,
    /// Incremental delta while an assistant message streams in.
    MessageDelta(MessageDelta),
    /// A complete message was appended to the session history.
    MessageAdded(Message),
    /// A tool call started executing.
    ToolStarted { call_id: CallId, name: String },
    /// Progress update from a running tool.
    ToolProgress { call_id: CallId, message: String },
    /// A tool call finished.
    ToolCompleted {
        call_id: CallId,
        result: ToolResultMessage,
    },
    /// A tool call requires a permission decision from the user
    /// (headless UIs answer from settings or stdin).
    PermissionRequested {
        request_id: String,
        tool_name: String,
        arguments: serde_json::Value,
    },
    /// A pending permission request was decided.
    PermissionResolved { request_id: String, allowed: bool },
    /// The current turn ended.
    TurnEnded(TurnOutcome),
    /// A non-fatal error occurred within the session.
    Error(McodeError),
    /// History was compacted: `before` messages collapsed into `after`.
    Compacted { before: usize, after: usize },
}

/// Incremental assistant content while streaming (mirrors the provider
/// stream events of design doc `01-agent-core.md` §2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum MessageDelta {
    /// Partial text content.
    TextDelta(String),
    /// Partial thinking content.
    ThinkingDelta(String),
    /// Partial tool-call arguments (raw JSON fragment).
    ToolCallDelta { id: String, partial_json: String },
}

/// Why a turn ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TurnOutcome {
    /// The model stopped without further tool calls.
    Completed,
    /// A steer message took over; a new turn follows.
    Steered,
    /// The turn was aborted via cancellation.
    Aborted,
}

/// Commands accepted by a session actor (design doc `01-agent-core.md` §4).
/// The doc's original `Rewind` was superseded by `Resume` (per the M1 plan T5).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SessionCommand {
    /// Start a new turn from a user prompt.
    Prompt(Message),
    /// Interrupt the current turn; the message is injected at the next
    /// model boundary.
    Steer(Message),
    /// Queue a message to continue the session when it would otherwise stop.
    FollowUp(Message),
    /// Abort the in-flight turn.
    Abort,
    /// Fork the conversation tree at the given message entry.
    Fork { at: MessageId },
    /// Load and continue a persisted session.
    Resume { session: SessionId },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::{ContentBlock, UserMessage};
    use serde_json::json;

    fn assert_roundtrip<T>(value: &T)
    where
        T: Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug,
    {
        let json = serde_json::to_string(value).expect("serialize");
        let back: T = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(&back, value);
    }

    fn sample_events() -> Vec<SessionEvent> {
        vec![
            SessionEvent::TurnStarted,
            SessionEvent::MessageDelta(MessageDelta::TextDelta("partial".into())),
            SessionEvent::MessageDelta(MessageDelta::ThinkingDelta("hmm".into())),
            SessionEvent::MessageDelta(MessageDelta::ToolCallDelta {
                id: "call_1".into(),
                partial_json: "{\"path\":".into(),
            }),
            SessionEvent::MessageAdded(Message::User(UserMessage::text("hello"))),
            SessionEvent::ToolStarted {
                call_id: CallId::from("call_1"),
                name: "read".into(),
            },
            SessionEvent::ToolProgress {
                call_id: CallId::from("call_1"),
                message: "1024 bytes".into(),
            },
            SessionEvent::ToolCompleted {
                call_id: CallId::from("call_1"),
                result: ToolResultMessage {
                    tool_call_id: "call_1".into(),
                    content: vec![ContentBlock::Text("done".into())],
                    is_error: false,
                    details: None,
                },
            },
            SessionEvent::PermissionRequested {
                request_id: "perm_1".into(),
                tool_name: "bash".into(),
                arguments: json!({"command": "rm -rf /tmp/x"}),
            },
            SessionEvent::PermissionResolved {
                request_id: "perm_1".into(),
                allowed: false,
            },
            SessionEvent::TurnEnded(TurnOutcome::Completed),
            SessionEvent::TurnEnded(TurnOutcome::Steered),
            SessionEvent::TurnEnded(TurnOutcome::Aborted),
            SessionEvent::Error(McodeError::Tool("boom".into())),
            SessionEvent::Compacted {
                before: 120,
                after: 12,
            },
        ]
    }

    #[test]
    fn session_event_roundtrip_all_variants() {
        for event in sample_events() {
            assert_roundtrip(&event);
        }
    }

    #[test]
    fn session_command_roundtrip_all_variants() {
        let commands = vec![
            SessionCommand::Prompt(Message::User(UserMessage::text("run tests"))),
            SessionCommand::Steer(Message::User(UserMessage::text("actually stop"))),
            SessionCommand::FollowUp(Message::User(UserMessage::text("and also lint"))),
            SessionCommand::Abort,
            SessionCommand::Fork {
                at: MessageId::from("a2"),
            },
            SessionCommand::Resume {
                session: SessionId::new(),
            },
        ];
        for command in commands {
            assert_roundtrip(&command);
        }
    }

    #[test]
    fn message_delta_and_turn_outcome_roundtrip() {
        assert_roundtrip(&MessageDelta::TextDelta("x".into()));
        assert_roundtrip(&TurnOutcome::Aborted);
    }

    #[test]
    fn events_are_clone_for_broadcast() {
        // tokio::broadcast requires Clone; guard against accidental removal.
        fn require_clone<T: Clone>(_: &T) {}
        for event in sample_events() {
            require_clone(&event);
        }
    }
}

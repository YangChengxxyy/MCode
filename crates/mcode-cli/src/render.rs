//! Headless rendering of the [`SessionEvent`] stream (M1 T6).
//!
//! Output contract (documented so scripts can rely on it):
//!
//! * **stdout** carries everything meant to be read as the transcript:
//!   assistant `TextDelta`s are written raw (streamed, unprefixed), and
//!   status lines carry prefixes so they stay distinguishable from —
//!   and never merge into — model text:
//!   * `==> tool <name> <args ≤120 chars>` — a tool call started;
//!   * `<== ok <first line of the result ≤120>` / `<== error <…>` — a
//!     tool call finished.
//! * **stderr** carries the ambient channel: thinking deltas (raw),
//!   tool progress, permission decisions, error events, and lag
//!   warnings. Nothing a consumer of the transcript needs.
//!
//! A newline is inserted before any prefixed line when raw text is
//! still "open" (no trailing newline yet), so the streamed text and the
//! status lines never share a line. Tool-call arguments are recovered
//! from the accumulated [`MessageDelta::ToolCallDelta`] fragments (the
//! complete call of a `MessageAdded` event is the fallback), because
//! [`SessionEvent::ToolStarted`] carries only the call id and name.

use std::collections::HashMap;
use std::io::{self, Stderr, Stdout, Write};

use mcode_core::events::{MessageDelta, SessionEvent};
use mcode_core::message::{ContentBlock, Message};

/// Characters of tool-call arguments shown on `==> tool` lines.
pub const ARGS_WIDTH: usize = 120;

/// Characters of tool-result summary shown on `<==` lines.
pub const SUMMARY_WIDTH: usize = 120;

/// Truncate `text` to at most `max` characters on char boundaries,
/// marking the cut with an ellipsis (`…`). Short strings pass through.
pub fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let cut: String = text.chars().take(max.saturating_sub(1)).collect();
    format!("{cut}…")
}

/// First-line summary of a tool result: the first text block's first
/// line, truncated; a placeholder when the result has no text.
pub fn summarize_result(result: &mcode_core::message::ToolResultMessage) -> String {
    let first_text = result.content.iter().find_map(|block| match block {
        ContentBlock::Text(text) => Some(text.text.as_str()),
        _ => None,
    });
    match first_text {
        Some(text) => truncate_chars(text.lines().next().unwrap_or(""), SUMMARY_WIDTH),
        None => format!("<{} non-text block(s)>", result.content.len()),
    }
}

/// Renders [`SessionEvent`]s to a stdout/stderr pair as they arrive.
pub struct HeadlessRenderer<O: Write, E: Write> {
    out: O,
    err: E,
    /// Raw text has been written since the last newline on stdout.
    line_dirty: bool,
    /// call id → accumulated `partial_json` fragments (the streamed
    /// arguments of that call).
    tool_args: HashMap<String, String>,
}

impl HeadlessRenderer<Stdout, Stderr> {
    /// A renderer on the process's real stdout/stderr.
    pub fn stdio() -> Self {
        Self::new(io::stdout(), io::stderr())
    }
}

impl<O: Write, E: Write> HeadlessRenderer<O, E> {
    /// A renderer on explicit writers (unit tests capture these).
    pub fn new(out: O, err: E) -> Self {
        Self {
            out,
            err,
            line_dirty: false,
            tool_args: HashMap::new(),
        }
    }

    /// Render one event.
    pub fn render(&mut self, event: &SessionEvent) -> io::Result<()> {
        match event {
            SessionEvent::TurnStarted => Ok(()),
            SessionEvent::MessageDelta(delta) => self.render_delta(delta),
            SessionEvent::MessageAdded(message) => {
                self.record_call_args(message);
                Ok(())
            }
            SessionEvent::ToolStarted { call_id, name } => {
                let args = self.tool_args.get(call_id.as_str()).cloned();
                self.start_line()?;
                match args {
                    Some(args) if !args.is_empty() => self.write_line(format!(
                        "==> tool {name} {}",
                        truncate_chars(&args, ARGS_WIDTH)
                    )),
                    _ => self.write_line(format!("==> tool {name}")),
                }
            }
            SessionEvent::ToolProgress { message, .. } => {
                self.write_err_line(format!("… {message}"))
            }
            SessionEvent::ToolCompleted { result, .. } => {
                let status = if result.is_error { "error" } else { "ok" };
                self.write_line(format!("<== {status} {}", summarize_result(result)))
            }
            SessionEvent::PermissionRequested { .. } => Ok(()), // the stage-3 callback prints the question
            SessionEvent::PermissionResolved { allowed, .. } => {
                let decision = if *allowed { "allowed" } else { "denied" };
                self.write_err_line(format!("permission: {decision}"))
            }
            SessionEvent::TurnEnded(_) => self.finish_line(),
            SessionEvent::Error(err) => self.write_err_line(format!("error: {err}")),
            SessionEvent::Compacted { before, after } => {
                self.write_err_line(format!("context compacted: {before} → {after} messages"))
            }
        }
    }

    fn render_delta(&mut self, delta: &MessageDelta) -> io::Result<()> {
        match delta {
            MessageDelta::TextDelta(text) => {
                self.out.write_all(text.as_bytes())?;
                self.out.flush()?; // streaming, not block-buffered
                self.line_dirty = !text.ends_with('\n');
                Ok(())
            }
            MessageDelta::ThinkingDelta(text) => self.err.write_all(text.as_bytes()),
            MessageDelta::ToolCallDelta { id, partial_json } => {
                self.tool_args
                    .entry(id.clone())
                    .or_default()
                    .push_str(partial_json);
                Ok(())
            }
        }
    }

    /// Keep the complete `ToolCall` arguments of assistant messages as
    /// the authoritative fallback for calls whose deltas never streamed.
    fn record_call_args(&mut self, message: &Message) {
        if let Message::Assistant(assistant) = message {
            for block in &assistant.blocks {
                if let ContentBlock::ToolCall(call) = block {
                    self.tool_args
                        .entry(call.id.clone())
                        .or_insert_with(|| call.arguments.to_string());
                }
            }
        }
    }

    /// Break the open text line (if any) so a prefixed line starts
    /// fresh.
    fn start_line(&mut self) -> io::Result<()> {
        if self.line_dirty {
            self.out.write_all(b"\n")?;
            self.line_dirty = false;
        }
        Ok(())
    }

    /// End the open text line (turn boundary).
    fn finish_line(&mut self) -> io::Result<()> {
        self.start_line()
    }

    /// Write one prefixed status line to stdout (always newline-ended).
    fn write_line(&mut self, line: String) -> io::Result<()> {
        self.out.write_all(line.as_bytes())?;
        self.out.write_all(b"\n")?;
        self.out.flush()
    }

    /// Write one line to the ambient stderr channel.
    fn write_err_line(&mut self, line: String) -> io::Result<()> {
        self.err.write_all(line.as_bytes())?;
        self.err.write_all(b"\n")
    }

    /// The stdout writer, consumed back by the caller.
    pub fn into_writers(self) -> (O, E) {
        (self.out, self.err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mcode_core::message::{ToolResultMessage, UserMessage};

    fn renderer() -> HeadlessRenderer<Vec<u8>, Vec<u8>> {
        HeadlessRenderer::new(Vec::new(), Vec::new())
    }

    fn outputs(r: HeadlessRenderer<Vec<u8>, Vec<u8>>) -> (String, String) {
        let (out, err) = r.into_writers();
        (
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
        )
    }

    fn tool_result(is_error: bool, text: &str) -> mcode_core::message::ToolResultMessage {
        ToolResultMessage {
            tool_call_id: "c1".into(),
            content: vec![ContentBlock::Text(text.into())],
            is_error,
            details: None,
        }
    }

    #[test]
    fn truncate_respects_char_boundaries_and_marks_cuts() {
        assert_eq!(truncate_chars("short", 10), "short");
        let long = "x".repeat(200);
        let cut = truncate_chars(&long, 120);
        assert_eq!(cut.chars().count(), 120);
        assert!(cut.ends_with('…'));
        // Multi-byte characters are not split.
        let unicode: String = "é".repeat(200);
        let cut = truncate_chars(&unicode, 120);
        assert_eq!(cut.chars().count(), 120);
        assert!(cut.ends_with('…')); // 119 é + the ellipsis mark
    }

    #[test]
    fn summarize_takes_first_line_of_first_text_block() {
        assert_eq!(
            summarize_result(&tool_result(false, "line one\nline two\nline three")),
            "line one"
        );
        let long = format!("{}\nsecond", "y".repeat(300));
        assert_eq!(
            summarize_result(&tool_result(false, &long)).chars().count(),
            120
        );
        // Non-text results get a placeholder.
        let mut result = tool_result(false, "");
        result.content = vec![ContentBlock::Image(mcode_core::message::BinaryData {
            mime_type: "image/png".into(),
            data: String::new(),
        })];
        assert_eq!(summarize_result(&result), "<1 non-text block(s)>");
    }

    #[test]
    fn text_then_tool_lines_never_share_a_line() {
        let mut r = renderer();
        r.render(&SessionEvent::MessageDelta(MessageDelta::TextDelta(
            "reading now".into(),
        )))
        .unwrap();
        r.render(&SessionEvent::MessageDelta(MessageDelta::ToolCallDelta {
            id: "c1".into(),
            partial_json: "{\"path\":".into(),
        }))
        .unwrap();
        r.render(&SessionEvent::MessageDelta(MessageDelta::ToolCallDelta {
            id: "c1".into(),
            partial_json: "\"Cargo.toml\"}".into(),
        }))
        .unwrap();
        r.render(&SessionEvent::ToolStarted {
            call_id: "c1".into(),
            name: "read".into(),
        })
        .unwrap();
        r.render(&SessionEvent::ToolCompleted {
            call_id: "c1".into(),
            result: tool_result(false, "[workspace]\nmembers = []"),
        })
        .unwrap();
        r.render(&SessionEvent::TurnEnded(mcode_core::TurnOutcome::Completed))
            .unwrap();

        assert_eq!(
            outputs(r).0,
            "reading now\n==> tool read {\"path\":\"Cargo.toml\"}\n<== ok [workspace]\n"
        );
    }

    #[test]
    fn error_results_render_as_error_status() {
        let mut r = renderer();
        r.render(&SessionEvent::ToolStarted {
            call_id: "c1".into(),
            name: "bash".into(),
        })
        .unwrap();
        r.render(&SessionEvent::ToolCompleted {
            call_id: "c1".into(),
            result: tool_result(true, "permission denied: the request was declined"),
        })
        .unwrap();
        assert_eq!(
            outputs(r).0,
            "==> tool bash\n<== error permission denied: the request was declined\n"
        );
    }

    #[test]
    fn errors_and_permission_decisions_go_to_stderr() {
        let mut r = renderer();
        r.render(&SessionEvent::Error(mcode_core::McodeError::Tool(
            "boom".into(),
        )))
        .unwrap();
        r.render(&SessionEvent::PermissionResolved {
            request_id: "p1".into(),
            allowed: false,
        })
        .unwrap();
        r.render(&SessionEvent::ToolProgress {
            call_id: "c1".into(),
            message: "scanned 3 files".into(),
        })
        .unwrap();
        let (_, err) = outputs(r);
        assert!(err.contains("error: tool error: boom"));
        assert!(err.contains("permission: denied"));
        assert!(err.contains("… scanned 3 files"));
    }

    #[test]
    fn turn_end_closes_an_open_text_line() {
        let mut r = renderer();
        r.render(&SessionEvent::MessageDelta(MessageDelta::TextDelta(
            "final answer".into(),
        )))
        .unwrap();
        r.render(&SessionEvent::TurnEnded(mcode_core::TurnOutcome::Completed))
            .unwrap();
        assert_eq!(outputs(r).0, "final answer\n");
    }

    #[test]
    fn message_added_fallback_supplies_tool_args() {
        use mcode_core::message::{AssistantMessage, ToolCall};
        let mut r = renderer();
        // No ToolCallDelta streamed at all.
        r.render(&SessionEvent::MessageAdded(Message::Assistant(
            AssistantMessage {
                blocks: vec![ContentBlock::ToolCall(ToolCall::new(
                    "c9",
                    "read",
                    serde_json::json!({"path": "x.rs"}),
                ))],
                usage: None,
                stop_reason: mcode_core::StopReason::ToolUse,
            },
        )))
        .unwrap();
        r.render(&SessionEvent::ToolStarted {
            call_id: "c9".into(),
            name: "read".into(),
        })
        .unwrap();
        assert_eq!(outputs(r).0, "==> tool read {\"path\":\"x.rs\"}\n");
    }

    #[test]
    fn user_messages_render_nothing() {
        let mut r = renderer();
        r.render(&SessionEvent::MessageAdded(Message::User(
            UserMessage::text("hi"),
        )))
        .unwrap();
        let (out, err) = outputs(r);
        assert_eq!(out, "");
        assert_eq!(err, "");
    }
}

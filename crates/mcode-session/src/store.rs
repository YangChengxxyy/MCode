//! JSONL session storage (design doc `01-agent-core.md` §4, the pi v3
//! format with a version header).
//!
//! ```jsonl
//! {"type":"header","format_version":1,"session_id":"…","cwd":"…","created_at":"…"}
//! {"type":"message","id":"a1","parent_id":null,"message":{…}}
//! {"type":"message","id":"a2","parent_id":"a1","message":{…}}
//! {"type":"label","id":"a2","label":"探索实现方案"}
//! {"type":"custom","id":"a3","parent_id":"a2","kind":"plugin:plan","data":{…}}
//! ```
//!
//! * The **header** is always the first non-empty line and carries
//!   [`FORMAT_VERSION`]; a missing header or an unsupported version is
//!   a hard load error (future versions migrate at load time).
//! * `parent_id` links each entry into a tree — forking a conversation
//!   appends new entries with an existing id as parent instead of
//!   writing a new file (see [`crate::tree`]).
//! * The `custom` entry type is the persistence channel for plugin
//!   [`CustomMessage`](mcode_core::CustomMessage)s (M2 plugin bridge).
//! * Writing is append-only; every entry is flushed to the OS the
//!   moment it is written. That survives a process crash, but not an
//!   OS or power crash: the line reaches the kernel, not necessarily
//!   the disk (per-entry `fsync` is deliberately skipped).
//!
//! # Corruption policy
//!
//! A corrupt line — invalid JSON, a known entry shape with wrong field
//! types, or an entry type this version does not know — never fails the
//! load: it is skipped with a `warn!` log and counted in
//! [`LoadedSession::skipped_lines`]. Only a broken header is fatal.

use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use mcode_core::McodeError;
use mcode_core::ids::{MessageId, SessionId};
use mcode_core::message::{CustomMessage, Message};
use serde::{Deserialize, Serialize};

/// The session-log format version this build writes and reads. Bump on
/// any breaking change to the line shapes; loading migrates from older
/// versions, rejecting newer ones.
pub const FORMAT_VERSION: u32 = 1;

/// The `"type"` value of the header line.
const HEADER_TYPE: &str = "header";

/// First line of every session file: format version plus identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionHeader {
    /// Log format version; must be [`FORMAT_VERSION`] to load here.
    pub format_version: u32,
    /// The session's id.
    pub session_id: SessionId,
    /// Working directory the session ran in (stringified path).
    pub cwd: String,
    /// Creation time (UTC).
    pub created_at: DateTime<Utc>,
}

impl SessionHeader {
    /// A header for a new session: current format version, `now`.
    pub fn new(session_id: SessionId, cwd: impl Into<String>) -> Self {
        Self {
            format_version: FORMAT_VERSION,
            session_id,
            cwd: cwd.into(),
            created_at: Utc::now(),
        }
    }

    /// Serialize as the header JSONL line.
    pub fn to_line(&self) -> Result<String, McodeError> {
        #[derive(Serialize)]
        struct Line<'a> {
            r#type: &'a str,
            format_version: u32,
            session_id: &'a SessionId,
            cwd: &'a str,
            created_at: &'a DateTime<Utc>,
        }
        let line = Line {
            r#type: HEADER_TYPE,
            format_version: self.format_version,
            session_id: &self.session_id,
            cwd: &self.cwd,
            created_at: &self.created_at,
        };
        Ok(serde_json::to_string(&line)?)
    }

    /// Parse a header JSONL line. Errors unless the line is valid JSON
    /// *and* carries `"type": "header"`.
    pub fn from_line(line: &str) -> Result<Self, McodeError> {
        #[derive(Deserialize)]
        struct Line {
            format_version: u32,
            session_id: SessionId,
            cwd: String,
            created_at: DateTime<Utc>,
        }
        let value: serde_json::Value = serde_json::from_str(line)
            .map_err(|err| McodeError::Session(format!("invalid header JSON: {err}")))?;
        let found_type = value.get("type").and_then(serde_json::Value::as_str);
        if found_type != Some(HEADER_TYPE) {
            return Err(McodeError::Session(format!(
                "not a header line (type={found_type:?}, expected {HEADER_TYPE:?})"
            )));
        }
        let line: Line = serde_json::from_value(value)
            .map_err(|err| McodeError::Session(format!("invalid header fields: {err}")))?;
        Ok(Self {
            format_version: line.format_version,
            session_id: line.session_id,
            cwd: line.cwd,
            created_at: line.created_at,
        })
    }
}

/// One append-only log entry below the header. `parent_id` forms the
/// conversation tree; [`SessionEntry::Label`] annotates an existing
/// entry without occupying a tree node of its own.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SessionEntry {
    /// A conversation message at a tree position.
    Message {
        /// Id of this entry.
        id: MessageId,
        /// Parent entry (`None` = root).
        parent_id: Option<MessageId>,
        /// The message itself.
        message: Message,
    },
    /// A human label attached to an existing entry id.
    Label {
        /// Id of the annotated entry.
        id: MessageId,
        /// The label text.
        label: String,
    },
    /// Plugin-defined tree node (the persisted form of
    /// [`CustomMessage`]): arbitrary `kind` + JSON `data`, replayed
    /// verbatim (M2 plugin bridge).
    Custom {
        /// Id of this entry.
        id: MessageId,
        /// Parent entry (`None` = root).
        parent_id: Option<MessageId>,
        /// Plugin-scoped kind discriminator, e.g. `"plugin:plan"`.
        kind: String,
        /// Arbitrary plugin payload, preserved verbatim.
        data: serde_json::Value,
    },
}

impl SessionEntry {
    /// The tree entry for `message` at `id`: `Message::Custom` maps to
    /// the dedicated [`SessionEntry::Custom`] form, everything else to
    /// [`SessionEntry::Message`].
    pub fn from_message(id: MessageId, parent_id: Option<MessageId>, message: Message) -> Self {
        match message {
            Message::Custom(CustomMessage { kind, data }) => Self::Custom {
                id,
                parent_id,
                kind,
                data,
            },
            message => Self::Message {
                id,
                parent_id,
                message,
            },
        }
    }

    /// The id field of this entry (for labels: the annotated entry).
    pub fn entry_id(&self) -> &MessageId {
        match self {
            Self::Message { id, .. } | Self::Label { id, .. } | Self::Custom { id, .. } => id,
        }
    }

    /// The tree-node id, or `None` for labels (annotations, not nodes).
    pub fn node_id(&self) -> Option<&MessageId> {
        match self {
            Self::Message { id, .. } | Self::Custom { id, .. } => Some(id),
            Self::Label { .. } => None,
        }
    }

    /// The parent link, or `None` for labels and roots.
    pub fn parent_id(&self) -> Option<&MessageId> {
        match self {
            Self::Message { parent_id, .. } | Self::Custom { parent_id, .. } => parent_id.as_ref(),
            Self::Label { .. } => None,
        }
    }

    /// Replay this entry as a [`Message`]: message entries yield their
    /// message, custom entries rebuild the [`CustomMessage`], labels
    /// replay as nothing.
    pub fn as_message(&self) -> Option<Message> {
        match self {
            Self::Message { message, .. } => Some(message.clone()),
            Self::Custom { kind, data, .. } => Some(Message::Custom(CustomMessage {
                kind: kind.clone(),
                data: data.clone(),
            })),
            Self::Label { .. } => None,
        }
    }
}

/// The result of loading a session file: validated header, every
/// parseable entry in file order, and how many lines were skipped.
#[derive(Debug, Clone, PartialEq)]
pub struct LoadedSession {
    /// The file's (validated) header.
    pub header: SessionHeader,
    /// Entries in file order.
    pub entries: Vec<SessionEntry>,
    /// How many non-empty lines were skipped as corrupt.
    pub skipped_lines: usize,
}

/// Append-only writer for one session file.
pub struct SessionStore {
    path: PathBuf,
    writer: BufWriter<File>,
}

impl SessionStore {
    /// Create a new session file at `path` (parent directories are
    /// created) and write the header line.
    pub fn create(path: impl Into<PathBuf>, header: &SessionHeader) -> Result<Self, McodeError> {
        let path = path.into();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|err| {
                McodeError::Session(format!("cannot create {}: {err}", parent.display()))
            })?;
        }
        let file = File::create(&path).map_err(|err| {
            McodeError::Session(format!("cannot create {}: {err}", path.display()))
        })?;
        let mut store = Self {
            writer: BufWriter::new(file),
            path,
        };
        let line = header.to_line()?;
        store.write_line(&line)?;
        Ok(store)
    }

    /// Open an existing session file for appending. The file must
    /// already exist — loading is [`load_session`]'s job.
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, McodeError> {
        let path = path.into();
        let file = File::options().append(true).open(&path).map_err(|err| {
            McodeError::Session(format!("cannot open session {}: {err}", path.display()))
        })?;
        Ok(Self {
            writer: BufWriter::new(file),
            path,
        })
    }

    /// Append one entry, flushed to the OS immediately (the design's
    /// flush-per-entry policy). That survives a process crash but
    /// not an OS crash: the line reaches the kernel, not necessarily
    /// the disk.
    pub fn append(&mut self, entry: &SessionEntry) -> Result<(), McodeError> {
        let line = serde_json::to_string(entry)?;
        self.write_line(&line)
    }

    /// The file this store writes to.
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn write_line(&mut self, line: &str) -> Result<(), McodeError> {
        self.writer.write_all(line.as_bytes())?;
        self.writer.write_all(b"\n")?;
        self.writer.flush()?;
        Ok(())
    }
}

/// Load and validate a session file: header first, then every
/// parseable entry; corrupt lines are skipped with a warning (see the
/// module docs). A missing header or an unsupported `format_version`
/// is a hard error.
pub fn load_session(path: impl AsRef<Path>) -> Result<LoadedSession, McodeError> {
    let path = path.as_ref();
    let file = File::open(path).map_err(|err| {
        McodeError::Session(format!("cannot open session {}: {err}", path.display()))
    })?;
    let reader = BufReader::new(file);

    let mut header: Option<SessionHeader> = None;
    let mut entries = Vec::new();
    let mut skipped_lines = 0usize;

    for (index, line) in reader.lines().enumerate() {
        let line = line
            .map_err(|err| McodeError::Session(format!("cannot read {}: {err}", path.display())))?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if header.is_none() {
            let parsed = SessionHeader::from_line(trimmed).map_err(|err| {
                McodeError::Session(format!(
                    "{}: first line must be the session header: {err}",
                    path.display()
                ))
            })?;
            if parsed.format_version != FORMAT_VERSION {
                return Err(McodeError::Session(format!(
                    "unsupported session log format_version {} in {} (this build supports \
                        {FORMAT_VERSION})",
                    parsed.format_version,
                    path.display()
                )));
            }
            header = Some(parsed);
            continue;
        }
        match serde_json::from_str::<SessionEntry>(trimmed) {
            Ok(entry) => entries.push(entry),
            Err(err) => {
                skipped_lines += 1;
                tracing::warn!(
                    path = %path.display(),
                    line = index + 1,
                    %err,
                    "skipping corrupt session log line"
                );
            }
        }
    }

    let Some(header) = header else {
        return Err(McodeError::Session(format!(
            "session file {} has no header line",
            path.display()
        )));
    };
    Ok(LoadedSession {
        header,
        entries,
        skipped_lines,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use mcode_core::message::{ContentBlock, StopReason, ToolResultMessage, UserMessage};
    use serde_json::json;

    fn user(text: &str) -> Message {
        Message::User(UserMessage::text(text))
    }

    #[test]
    fn header_line_roundtrip() {
        let header = SessionHeader {
            format_version: FORMAT_VERSION,
            session_id: SessionId::from("s-42"),
            cwd: "/Users/cc/app".into(),
            created_at: DateTime::parse_from_rfc3339("2025-06-14T10:15:30Z")
                .unwrap()
                .with_timezone(&Utc),
        };
        let line = header.to_line().unwrap();
        assert!(line.contains(r#""type":"header""#), "{line}");
        assert!(line.contains(r#""format_version":1"#), "{line}");
        assert_eq!(SessionHeader::from_line(&line).unwrap(), header);
    }

    #[test]
    fn from_line_rejects_non_header_lines() {
        assert!(SessionHeader::from_line(r#"{"type":"message","id":"a1"}"#).is_err());
        assert!(SessionHeader::from_line("not json").is_err());
        // Valid JSON, wrong shape.
        assert!(SessionHeader::from_line(r#"{"format_version":1}"#).is_err());
    }

    #[test]
    fn entry_serde_tag_shapes() {
        let entries = vec![
            SessionEntry::Message {
                id: MessageId::from("a1"),
                parent_id: None,
                message: user("hi"),
            },
            SessionEntry::Message {
                id: MessageId::from("a2"),
                parent_id: Some(MessageId::from("a1")),
                message: Message::ToolResult(ToolResultMessage {
                    tool_call_id: "c1".into(),
                    content: vec![ContentBlock::Text("ok".into())],
                    is_error: false,
                    details: Some(json!({"diff": 3})),
                }),
            },
            SessionEntry::Label {
                id: MessageId::from("a2"),
                label: "探索实现方案".into(),
            },
            SessionEntry::Custom {
                id: MessageId::from("a3"),
                parent_id: Some(MessageId::from("a2")),
                kind: "plugin:plan".into(),
                data: json!({"steps": [1, 2]}),
            },
        ];
        for entry in &entries {
            let json = serde_json::to_string(entry).unwrap();
            let back: SessionEntry = serde_json::from_str(&json).unwrap();
            assert_eq!(&back, entry, "{json}");
        }
        let shapes = serde_json::to_string(&entries[0]).unwrap();
        assert!(
            shapes.starts_with(r#"{"type":"message","id":"a1""#),
            "{shapes}"
        );
        let label = serde_json::to_string(&entries[2]).unwrap();
        assert!(label.starts_with(r#"{"type":"label""#), "{label}");
        let custom = serde_json::to_string(&entries[3]).unwrap();
        assert!(custom.starts_with(r#"{"type":"custom""#), "{custom}");
    }

    #[test]
    fn from_message_routes_custom_messages_to_custom_entries() {
        let custom = SessionEntry::from_message(
            MessageId::from("a1"),
            None,
            Message::Custom(CustomMessage {
                kind: "plugin:plan".into(),
                data: json!({"n": 1}),
            }),
        );
        assert!(matches!(custom, SessionEntry::Custom { .. }));
        assert_eq!(
            custom.as_message(),
            Some(Message::Custom(CustomMessage {
                kind: "plugin:plan".into(),
                data: json!({"n": 1})
            }))
        );

        let plain = SessionEntry::from_message(
            MessageId::from("a2"),
            Some(MessageId::from("a1")),
            Message::Assistant(mcode_core::AssistantMessage {
                blocks: vec![ContentBlock::Text("done".into())],
                usage: None,
                stop_reason: StopReason::Stop,
            }),
        );
        assert!(matches!(plain, SessionEntry::Message { .. }));
        assert!(plain.as_message().is_some());
        assert_eq!(plain.parent_id(), Some(&MessageId::from("a1")));
    }

    #[test]
    fn label_accessors_reflect_annotation_semantics() {
        let label = SessionEntry::Label {
            id: MessageId::from("a2"),
            label: "L".into(),
        };
        assert_eq!(label.entry_id(), &MessageId::from("a2"));
        assert!(label.node_id().is_none());
        assert!(label.parent_id().is_none());
        assert_eq!(label.as_message(), None);
    }

    #[test]
    fn unknown_entry_type_fails_entry_parse() {
        // Forward compatibility: an entry type from a future version is
        // a parse error (skipped by `load_session`, not fatal).
        let err = serde_json::from_str::<SessionEntry>(r#"{"type":"ai-summary","id":"a9"}"#);
        assert!(err.is_err());
    }
}

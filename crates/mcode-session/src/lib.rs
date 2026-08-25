//! `mcode-session` — the session layer of MCode (M1 T5; design doc
//! `01-agent-core.md` §4): a tokio actor driving `mcode-agent`'s
//! [`Agent`](mcode_agent::Agent), an append-only JSONL tree store, and
//! fork/resume over that tree.
//!
//! ```text
//! UI / CLI ──SessionCommand──► SessionHandle ──mpsc──► SessionActor (task)
//!    ▲                                                      │
//!    └──────────── broadcast ◄───────────────────────────────┤ each turn runs
//!                 SessionEvent                               ▼ mcode-agent's
//!                                                 Agent::prompt(msg, &TurnEnv)
//! ```
//!
//! * [`store`] — the JSONL format: a `format_version` header line,
//!   then `message` / `label` / `custom` entries linked by `parent_id`
//!   into a tree. Append-only, flushed per entry, corrupt-line
//!   tolerant on load.
//! * [`tree`] — tree operations over those entries: latest branch,
//!   ancestry replay ([`SessionTree::messages_to`]), fork points.
//! * [`actor`] — [`SessionHandle`] / [`SessionActor`]: commands
//!   (Prompt / Steer / FollowUp / Abort / Fork / Resume), event
//!   broadcast, and the persistence wiring that lands every message
//!   the agent loop produces into the store.
//! * [`paths`] — `~/.mcode/sessions/<cwd-slug>/<timestamp>_<uuid>.jsonl`
//!   with the `$MCODE_HOME` override.
//!
//! The crate stays provider-agnostic: everything ambient is injected
//! via [`SessionEnv`](actor::SessionEnv) (an `Arc<dyn Provider>` plus
//! tools/permissions/hooks) and the agent itself is built by an
//! injectable [`AgentFactory`](actor::AgentFactory) closure.

pub mod actor;
pub mod paths;
pub mod store;
pub mod tree;

pub use actor::{AgentFactory, SessionEnv, SessionHandle, SessionMeta, default_agent_factory};
pub use paths::{
    cwd_slug, find_session_by_id, home_from, latest_session_file_from, mcode_home,
    new_session_file, resolve_session, resolve_session_from, session_dir, session_dir_from,
    session_file_name, sessions_root, sessions_root_from,
};
pub use store::{
    FORMAT_VERSION, LoadedSession, SessionEntry, SessionHeader, SessionStore, load_session,
};
pub use tree::{ForkPoint, SessionTree};

//! The `parent_id` conversation tree (design doc `01-agent-core.md`
//! §4): fork/branch without new files — every append names its parent,
//! and any entry id is a valid fork point.
//!
//! * [`SessionTree::latest_branch`] / [`SessionTree::branch_messages`]
//!   follow the **latest leaf** (the most recently appended node) back
//!   to the root — the "current" branch a resumed session continues.
//! * [`SessionTree::messages_to`] replays any entry's ancestry into
//!   the [`Message`] sequence the agent loop consumes.
//! * [`SessionTree::fork_at`] validates a fork point; afterwards the
//!   caller simply appends with that id as `parent_id`.
//!
//! # Corruption tolerance
//!
//! Nodes whose `parent_id` is missing from the tree (its line was
//! corrupt and skipped on load) become de-facto roots: ancestry walks
//! stop there with a `warn!` instead of failing the replay. Parent
//! cycles — impossible in honest appends, possible in hand-edited
//! logs — are detected and cut the same way.

use std::collections::{HashMap, HashSet, VecDeque};

use mcode_core::McodeError;
use mcode_core::ids::MessageId;
use mcode_core::message::Message;

use crate::store::SessionEntry;

/// A validated fork point: subsequent appends name this entry as their
/// `parent_id`, growing a new branch that shares the prefix up to here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForkPoint(MessageId);

impl ForkPoint {
    /// The entry id future appends attach to.
    pub fn id(&self) -> &MessageId {
        &self.0
    }
}

/// In-memory index over a session's entries: the parent/child graph,
/// node insertion order (which defines the latest leaf), and labels.
pub struct SessionTree {
    /// Every entry in file order (labels included) — the roundtrip view.
    entries: Vec<SessionEntry>,
    /// Message/custom ids in insertion order (excludes labels).
    order: Vec<MessageId>,
    /// Node id → its parent (None = root).
    parents: HashMap<MessageId, Option<MessageId>>,
    /// Node id → children in insertion order.
    children: HashMap<MessageId, Vec<MessageId>>,
    /// Root node ids in insertion order.
    roots: Vec<MessageId>,
    /// Entry id → latest label.
    labels: HashMap<MessageId, String>,
}

impl SessionTree {
    /// An empty tree.
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            order: Vec::new(),
            parents: HashMap::new(),
            children: HashMap::new(),
            roots: Vec::new(),
            labels: HashMap::new(),
        }
    }

    /// Build a tree from loaded entries (file order preserved).
    pub fn from_entries(entries: Vec<SessionEntry>) -> Self {
        let mut tree = Self::new();
        for entry in entries {
            tree.insert(entry);
        }
        tree
    }

    /// Insert one entry, linking it into the graph. Duplicate node ids
    /// (corruption) are warned about and left unlinked; the entry still
    /// stays in [`SessionTree::entries`] for roundtrip fidelity.
    pub fn insert(&mut self, entry: SessionEntry) {
        let Some(node_id) = entry.node_id().cloned() else {
            if let SessionEntry::Label { id, label } = &entry {
                self.labels.insert(id.clone(), label.clone());
            }
            self.entries.push(entry);
            return;
        };
        let parent = entry.parent_id().cloned();
        if self.parents.contains_key(&node_id) {
            tracing::warn!(id = %node_id, "duplicate entry id in session log; keeping the first node");
            self.entries.push(entry);
            return;
        }
        self.order.push(node_id.clone());
        self.parents.insert(node_id.clone(), parent.clone());
        match &parent {
            Some(parent_id) => self
                .children
                .entry(parent_id.clone())
                .or_default()
                .push(node_id.clone()),
            None => self.roots.push(node_id.clone()),
        }
        self.entries.push(entry);
    }

    /// Whether `id` names a tree node (message or custom entry).
    pub fn contains(&self, id: &MessageId) -> bool {
        self.parents.contains_key(id)
    }

    /// The latest leaf: the most recently inserted node with no
    /// children — the tip the "current" branch ends at. After a fork
    /// and fresh appends this is the new branch's tip.
    pub fn latest_leaf(&self) -> Option<MessageId> {
        self.order
            .iter()
            .rev()
            .find(|id| self.children.get(*id).is_none_or(Vec::is_empty))
            .cloned()
    }

    /// The current branch, root → latest leaf.
    pub fn latest_branch(&self) -> Vec<MessageId> {
        self.latest_leaf()
            .map(|leaf| self.chain_to_root(&leaf))
            .unwrap_or_default()
    }

    /// The message replay of the current (latest) branch.
    pub fn branch_messages(&self) -> Vec<Message> {
        self.latest_leaf()
            .and_then(|leaf| self.messages_to(&leaf))
            .unwrap_or_default()
    }

    /// Replay the ancestry of `at` (root → `at`) as the message
    /// sequence: message entries yield their message, custom entries
    /// rebuild their `CustomMessage`, labels replay as nothing.
    /// `None` when `at` is not a tree node.
    pub fn messages_to(&self, at: &MessageId) -> Option<Vec<Message>> {
        if !self.contains(at) {
            return None;
        }
        Some(
            self.chain_to_root(at)
                .iter()
                .filter_map(|id| self.entry_of(id).and_then(|entry| entry.as_message()))
                .collect(),
        )
    }

    /// Validate `at` as a fork point: the returned [`ForkPoint`] is the
    /// id subsequent appends attach to.
    pub fn fork_at(&self, at: &MessageId) -> Result<ForkPoint, McodeError> {
        if self.contains(at) {
            Ok(ForkPoint(at.clone()))
        } else {
            Err(McodeError::Session(format!(
                "unknown fork point: no entry with id {at} in this session"
            )))
        }
    }

    /// The latest label attached to `id`, if any.
    pub fn label_of(&self, id: &MessageId) -> Option<&str> {
        self.labels.get(id).map(String::as_str)
    }

    /// All entries in insertion order — exactly what was appended.
    pub fn entries(&self) -> &[SessionEntry] {
        &self.entries
    }

    /// Number of tree nodes (labels excluded).
    pub fn node_count(&self) -> usize {
        self.order.len()
    }

    /// Children of `id` in insertion order (empty for unknown ids).
    pub fn children_of(&self, id: &MessageId) -> &[MessageId] {
        self.children.get(id).map(Vec::as_slice).unwrap_or(&[])
    }

    /// The entry owning tree node `id`.
    fn entry_of(&self, id: &MessageId) -> Option<&SessionEntry> {
        self.entries
            .iter()
            .find(|entry| entry.node_id() == Some(id))
    }

    /// Walk `from` up to its root, returning root → `from`. A missing
    /// parent (corrupt line was skipped) or a parent cycle truncates
    /// the walk with a warning — the replay stays usable.
    fn chain_to_root(&self, from: &MessageId) -> Vec<MessageId> {
        let mut chain = VecDeque::from([from.clone()]);
        let mut visited: HashSet<MessageId> = HashSet::from([from.clone()]);
        let mut cursor = from.clone();
        while let Some(parent) = self.parents.get(&cursor).cloned().flatten() {
            if !self.contains(&parent) {
                tracing::warn!(parent = %parent, "dangling parent link; truncating branch replay");
                break;
            }
            if !visited.insert(parent.clone()) {
                tracing::warn!(parent = %parent, "parent cycle in session log; cutting the walk");
                break;
            }
            chain.push_front(parent.clone());
            cursor = parent;
        }
        chain.into()
    }
}

impl Default for SessionTree {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mcode_core::message::UserMessage;
    use serde_json::json;

    fn node(id: &str, parent: Option<&str>, text: &str) -> SessionEntry {
        SessionEntry::Message {
            id: MessageId::from(id),
            parent_id: parent.map(MessageId::from),
            message: Message::User(UserMessage::text(text)),
        }
    }

    fn ids(values: &[&str]) -> Vec<MessageId> {
        values.iter().map(|value| MessageId::from(*value)).collect()
    }

    fn texts(messages: &[Message]) -> Vec<String> {
        messages
            .iter()
            .map(|msg| match msg {
                Message::User(user) => match &user.content[0] {
                    mcode_core::ContentBlock::Text(text) => text.text.clone(),
                    block => panic!("unexpected block: {block:?}"),
                },
                other => panic!("unexpected message: {other:?}"),
            })
            .collect()
    }

    #[test]
    fn latest_branch_follows_the_latest_leaf() {
        let mut tree = SessionTree::new();
        for entry in [
            node("a1", None, "one"),
            node("a2", Some("a1"), "two"),
            node("a3", Some("a2"), "three"),
        ] {
            tree.insert(entry);
        }
        assert_eq!(tree.latest_leaf(), Some(MessageId::from("a3")));
        assert_eq!(tree.latest_branch(), ids(&["a1", "a2", "a3"]));
        assert_eq!(texts(&tree.branch_messages()), ["one", "two", "three"]);
    }

    #[test]
    fn messages_to_replays_any_ancestry_and_skips_labels() {
        let mut tree = SessionTree::new();
        for entry in [
            node("a1", None, "one"),
            node("a2", Some("a1"), "two"),
            SessionEntry::Label {
                id: MessageId::from("a2"),
                label: "checkpoint".into(),
            },
            node("a3", Some("a2"), "three"),
        ] {
            tree.insert(entry);
        }
        assert_eq!(
            texts(&tree.messages_to(&MessageId::from("a2")).unwrap()),
            ["one", "two"]
        );
        assert_eq!(tree.messages_to(&MessageId::from("nope")), None);
        // Labels annotate but never join the replay.
        assert_eq!(tree.node_count(), 3);
        assert_eq!(tree.label_of(&MessageId::from("a2")), Some("checkpoint"));
    }

    #[test]
    fn custom_entries_replay_as_custom_messages() {
        let mut tree = SessionTree::new();
        tree.insert(node("a1", None, "one"));
        tree.insert(SessionEntry::Custom {
            id: MessageId::from("a2"),
            parent_id: Some(MessageId::from("a1")),
            kind: "plugin:plan".into(),
            data: json!({"steps": 2}),
        });
        let messages = tree.messages_to(&MessageId::from("a2")).unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(
            messages[1],
            Message::Custom(mcode_core::CustomMessage {
                kind: "plugin:plan".into(),
                data: json!({"steps": 2}),
            })
        );
    }

    #[test]
    fn fork_at_validates_and_allows_branch_growth() {
        let mut tree = SessionTree::new();
        for entry in [node("a1", None, "one"), node("a2", Some("a1"), "two")] {
            tree.insert(entry);
        }
        let fork = tree.fork_at(&MessageId::from("a1")).unwrap();
        assert_eq!(fork.id(), &MessageId::from("a1"));
        assert!(tree.fork_at(&MessageId::from("zz")).is_err());
        // Grow a branch off the fork point.
        tree.insert(node("b1", Some(fork.id().as_str()), "branch"));
        assert_eq!(
            texts(&tree.messages_to(&MessageId::from("b1")).unwrap()),
            ["one", "branch"]
        );
        // The old tip is untouched.
        assert_eq!(
            texts(&tree.messages_to(&MessageId::from("a2")).unwrap()),
            ["one", "two"]
        );
        assert_eq!(
            tree.children_of(&MessageId::from("a1")),
            &ids(&["a2", "b1"])
        );
        // The newest leaf is now the branch tip.
        assert_eq!(tree.latest_leaf(), Some(MessageId::from("b1")));
    }

    #[test]
    fn dangling_parent_truncates_replay_instead_of_failing() {
        let mut tree = SessionTree::new();
        tree.insert(node("a1", None, "one"));
        tree.insert(node("a3", Some("a2"), "three")); // a2 was a corrupt line
        assert_eq!(
            texts(&tree.messages_to(&MessageId::from("a3")).unwrap()),
            ["three"]
        );
        // The dangling node is its own de-facto root.
        assert_eq!(tree.latest_branch(), vec![MessageId::from("a3")]);
    }

    #[test]
    fn parent_cycle_is_cut() {
        let mut tree = SessionTree::new();
        tree.insert(node("a1", Some("a2"), "one"));
        tree.insert(node("a2", Some("a1"), "two"));
        // The walk must terminate; both sides of the cycle replay.
        let messages = tree.messages_to(&MessageId::from("a2")).unwrap();
        assert_eq!(texts(&messages), ["one", "two"]);
    }

    #[test]
    fn duplicate_ids_keep_the_first_node() {
        let mut tree = SessionTree::new();
        tree.insert(node("a1", None, "first"));
        tree.insert(node("a1", None, "second"));
        assert_eq!(tree.node_count(), 1);
        assert_eq!(tree.entries().len(), 2);
        assert_eq!(texts(&tree.branch_messages()), ["first"]);
    }
}

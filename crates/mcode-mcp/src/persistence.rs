//! Minimal persistence DTOs for reconnect-on-resume semantics.

// Rust guideline compliant 2026-08-20.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{
    catalog::Generation,
    identity::{NamespacedId, ServerName},
};

/// Persisted MCP engine state format.
pub const PERSISTED_MCP_STATE_VERSION: u32 = 1;

/// References needed to reconstruct one server after session resume.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PersistedServerState {
    /// Reference to the authoritative JSON server configuration.
    pub config_ref: String,
    /// Last catalog generation observed by the session.
    pub catalog_generation: Generation,
    /// Optional reference to a separate terminal-call ledger.
    pub call_ledger_ref: Option<String>,
}

/// Persisted MCP state containing no live transport or credentials.
///
/// Resume logic must resolve each `configRef` and establish a new connection.
/// OAuth access/refresh tokens remain exclusively in [`crate::AuthHost`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PersistedMcpState {
    /// Persisted format version.
    pub version: u32,
    /// One first-party `mcode.mcp` plugin's server references.
    pub servers: BTreeMap<ServerName, PersistedServerState>,
}

impl PersistedMcpState {
    /// Creates empty current-version persistence state.
    #[must_use]
    pub fn new() -> Self {
        Self {
            version: PERSISTED_MCP_STATE_VERSION,
            servers: BTreeMap::new(),
        }
    }
}

impl Default for PersistedMcpState {
    fn default() -> Self {
        Self::new()
    }
}

/// Terminal-ledger state for one side-effect-sensitive call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub enum CallTerminalState {
    /// Call started but did not record a terminal result yet.
    Running,
    /// Call recorded a successful terminal result.
    Completed,
    /// Call recorded a terminal tool or protocol failure.
    Failed,
    /// Caller cancellation reached a terminal state.
    Cancelled,
    /// Process/session recovery found the call without a terminal record.
    Interrupted,
}

/// Minimal entry in an external call terminal ledger.
///
/// Arguments, output, transport handles, and authorization material are not
/// stored here. In particular, interrupted side-effecting calls are evidence,
/// never retry instructions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CallLedgerEntry {
    /// Host call identifier.
    pub call_id: String,
    /// Stable tool identity.
    pub tool: NamespacedId,
    /// Whether the caller classified this operation as potentially side-effecting.
    pub side_effecting: bool,
    /// Last terminal-ledger state.
    pub state: CallTerminalState,
}

impl CallLedgerEntry {
    /// Marks an unfinished call as interrupted during recovery.
    ///
    /// Returns `true` only when this call changed. No retry is scheduled.
    pub fn mark_interrupted(&mut self) -> bool {
        if self.state == CallTerminalState::Running {
            self.state = CallTerminalState::Interrupted;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovery_marks_running_call_without_retry_metadata() {
        let mut entry = CallLedgerEntry {
            call_id: "call-1".into(),
            tool: NamespacedId::new(ServerName::new("github").unwrap(), "issue.create").unwrap(),
            side_effecting: true,
            state: CallTerminalState::Running,
        };
        assert!(entry.mark_interrupted());
        assert_eq!(entry.state, CallTerminalState::Interrupted);
        let json = serde_json::to_value(entry).unwrap();
        assert!(json.get("arguments").is_none());
        assert!(json.get("retry").is_none());
        assert!(json.get("token").is_none());
    }
}

//! Immutable end-to-end FeaturePack deadline policy.

// Rust guideline compliant 2026-08-31.

use std::num::NonZeroU32;
use std::time::Duration;

use mcode_config::PluginFamily;

/// Process-start deadline snapshot for non-Web FeaturePack operations.
///
/// Every field is nonzero by construction. Web operations use their accepted
/// authority binding instead, while Providers use the ProviderPack protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FeatureDeadlinePolicyV1 {
    /// Session operation duration in milliseconds.
    pub session_ms: NonZeroU32,
    /// Compaction operation duration in milliseconds.
    pub compaction_ms: NonZeroU32,
    /// Resources operation duration in milliseconds.
    pub resources_ms: NonZeroU32,
    /// Ask operation duration in milliseconds.
    pub ask_ms: NonZeroU32,
    /// Todo operation duration in milliseconds.
    pub todo_ms: NonZeroU32,
    /// MCP operation duration in milliseconds.
    pub mcp_ms: NonZeroU32,
    /// Usage operation duration in milliseconds.
    pub usage_ms: NonZeroU32,
    /// Subagents operation duration in milliseconds.
    pub subagents_ms: NonZeroU32,
    /// Workspace operation duration in milliseconds.
    pub workspace_ms: NonZeroU32,
    /// UI operation duration in milliseconds.
    pub ui_ms: NonZeroU32,
}

impl FeatureDeadlinePolicyV1 {
    /// Returns the immutable duration for a non-Web FeaturePack family.
    ///
    /// Providers and Web deliberately return `None` because their protocols
    /// do not use this snapshot.
    #[must_use]
    pub const fn duration(self, family: PluginFamily) -> Option<Duration> {
        let milliseconds = match family {
            PluginFamily::Providers | PluginFamily::Web => return None,
            PluginFamily::Session => self.session_ms,
            PluginFamily::Compaction => self.compaction_ms,
            PluginFamily::Resources => self.resources_ms,
            PluginFamily::Ask => self.ask_ms,
            PluginFamily::Todo => self.todo_ms,
            PluginFamily::Mcp => self.mcp_ms,
            PluginFamily::Usage => self.usage_ms,
            PluginFamily::Subagents => self.subagents_ms,
            PluginFamily::Workspace => self.workspace_ms,
            PluginFamily::Ui => self.ui_ms,
        };
        Some(Duration::from_millis(milliseconds.get() as u64))
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;
    use std::time::Duration;

    use mcode_config::PluginFamily;

    use super::FeatureDeadlinePolicyV1;
    use crate::runtime::PluginRuntime;

    #[test]
    fn every_non_web_feature_has_its_exact_nonzero_duration() {
        let values = [1, 2, 3, 4, 5, 6, 7, 8, 9, u32::MAX];
        let policy = FeatureDeadlinePolicyV1 {
            session_ms: nonzero(values[0]),
            compaction_ms: nonzero(values[1]),
            resources_ms: nonzero(values[2]),
            ask_ms: nonzero(values[3]),
            todo_ms: nonzero(values[4]),
            mcp_ms: nonzero(values[5]),
            usage_ms: nonzero(values[6]),
            subagents_ms: nonzero(values[7]),
            workspace_ms: nonzero(values[8]),
            ui_ms: nonzero(values[9]),
        };
        let families = [
            PluginFamily::Session,
            PluginFamily::Compaction,
            PluginFamily::Resources,
            PluginFamily::Ask,
            PluginFamily::Todo,
            PluginFamily::Mcp,
            PluginFamily::Usage,
            PluginFamily::Subagents,
            PluginFamily::Workspace,
            PluginFamily::Ui,
        ];

        for (family, milliseconds) in families.into_iter().zip(values) {
            assert_eq!(
                policy.duration(family),
                Some(Duration::from_millis(milliseconds.into()))
            );
        }
        assert_eq!(policy.duration(PluginFamily::Providers), None);
        assert_eq!(policy.duration(PluginFamily::Web), None);

        let unconfigured = PluginRuntime::new();
        assert_eq!(unconfigured.inner.feature_deadline_policy(), None);
        let configured = PluginRuntime::with_feature_deadline_policy(policy);
        assert_eq!(configured.inner.feature_deadline_policy(), Some(policy));
    }

    fn nonzero(value: u32) -> NonZeroU32 {
        NonZeroU32::new(value).expect("nonzero test duration")
    }
}

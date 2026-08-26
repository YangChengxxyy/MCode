//! Host import implementation with length limits and typed errors.

// Rust guideline compliant 2026-08-26.

use mcode_plugin_api::{
    CapabilityGrants, CapabilityKind, MAX_GUEST_OUTPUT_BYTES, MAX_HOST_ACTION_RECORDS,
    MAX_HOST_LOG_BYTES, MAX_HOST_LOG_RECORDS, MAX_HOST_VIEW_RECORDS, MAX_UI_ACTION_BYTES,
    MAX_UI_VIEW_BYTES, UiView, validate_ui_action,
};
use wasmtime::StoreLimits;

use crate::wit::mcode::plugin::host::Host;

/// Store-owned host state for one plugin generation.
pub struct PluginStore {
    /// Wasmtime resource limiter.
    pub limits: StoreLimits,
    /// Captured log messages for tests.
    pub logs: Vec<(String, String)>,
    /// Captured view JSON for tests.
    pub views: Vec<String>,
    /// Captured action JSON for tests.
    pub actions: Vec<String>,
    grants: CapabilityGrants,
    ui_declared: bool,
}

impl PluginStore {
    pub(crate) fn new(limits: StoreLimits, grants: CapabilityGrants, ui_declared: bool) -> Self {
        Self {
            limits,
            logs: Vec::new(),
            views: Vec::new(),
            actions: Vec::new(),
            grants,
            ui_declared,
        }
    }

    fn ui_permitted(&self) -> bool {
        self.ui_declared && self.grants.allows(CapabilityKind::Ui)
    }
}

impl Host for PluginStore {
    fn log(&mut self, level: String, message: String) -> String {
        if level.len() > 32 || message.len() > MAX_HOST_LOG_BYTES {
            return "oversized".into();
        }
        if !matches!(
            level.as_str(),
            "error" | "warn" | "info" | "debug" | "trace"
        ) {
            return "invalid-level".into();
        }
        if self.logs.len() >= MAX_HOST_LOG_RECORDS {
            return "resource-limit".into();
        }
        self.logs.push((level, message));
        String::new()
    }

    fn publish_view(&mut self, view_json: String) -> String {
        if !self.ui_permitted() {
            return "forbidden".into();
        }
        if view_json.len() > MAX_UI_VIEW_BYTES || view_json.len() > MAX_GUEST_OUTPUT_BYTES {
            return "oversized".into();
        }
        if self.views.len() >= MAX_HOST_VIEW_RECORDS {
            return "resource-limit".into();
        }
        match serde_json::from_str::<UiView>(&view_json) {
            Ok(view) if view.validate().is_ok() => {
                self.views.push(view_json);
                String::new()
            }
            _ => "invalid-view".into(),
        }
    }

    fn emit_action(&mut self, action_json: String) -> String {
        if !self.ui_permitted() {
            return "forbidden".into();
        }
        if action_json.len() > MAX_UI_ACTION_BYTES || action_json.len() > MAX_GUEST_OUTPUT_BYTES {
            return "oversized".into();
        }
        if self.actions.len() >= MAX_HOST_ACTION_RECORDS {
            return "resource-limit".into();
        }
        match serde_json::from_str(&action_json) {
            Ok(action) if validate_ui_action(&action).is_ok() => {
                self.actions.push(action_json);
                String::new()
            }
            _ => "invalid-action".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use mcode_plugin_api::{
        CapabilityGrants, CapabilityKind, MAX_HOST_ACTION_RECORDS, MAX_HOST_LOG_RECORDS,
        MAX_HOST_VIEW_RECORDS,
    };
    use wasmtime::StoreLimitsBuilder;

    use super::{Host, PluginStore};

    const VIEW: &str = r#"{"kind":"widget","metadata":{"id":"status.main","region":"global","priority":0,"width":{"min":1,"max":80},"invalidation":{"mode":"manual"}},"content":{"type":"text","text":"ok","tone":"normal","emphasized":false}}"#;
    const ACTION: &str =
        r#"{"id":"act.main","viewId":"status.main","kind":"dismiss","payload":{}}"#;

    fn store(ui_granted: bool, ui_declared: bool) -> PluginStore {
        let mut grants = CapabilityGrants::none();
        if ui_granted {
            grants.allow(CapabilityKind::Ui);
        }
        PluginStore::new(StoreLimitsBuilder::new().build(), grants, ui_declared)
    }

    #[test]
    fn ui_host_imports_require_declared_and_granted_capability() {
        let mut none = store(false, false);
        assert_eq!(none.publish_view(VIEW.into()), "forbidden");
        assert_eq!(none.emit_action(ACTION.into()), "forbidden");
        assert!(none.views.is_empty());
        assert!(none.actions.is_empty());

        let mut declared_only = store(false, true);
        assert_eq!(declared_only.publish_view(VIEW.into()), "forbidden");
        assert_eq!(declared_only.emit_action(ACTION.into()), "forbidden");

        let mut granted_only = store(true, false);
        assert_eq!(granted_only.publish_view(VIEW.into()), "forbidden");
        assert_eq!(granted_only.emit_action(ACTION.into()), "forbidden");

        let mut allowed = store(true, true);
        assert_eq!(allowed.publish_view(VIEW.into()), "");
        assert_eq!(allowed.emit_action(ACTION.into()), "");
        assert_eq!(allowed.views.len(), 1);
        assert_eq!(allowed.actions.len(), 1);
    }

    #[test]
    fn host_captures_are_bounded() {
        let mut allowed = store(true, true);
        for _ in 0..MAX_HOST_LOG_RECORDS {
            assert_eq!(allowed.log("info".into(), "n".into()), "");
        }
        assert_eq!(allowed.log("info".into(), "n".into()), "resource-limit");
        assert_eq!(allowed.logs.len(), MAX_HOST_LOG_RECORDS);

        for _ in 0..MAX_HOST_VIEW_RECORDS {
            assert_eq!(allowed.publish_view(VIEW.into()), "");
        }
        assert_eq!(allowed.publish_view(VIEW.into()), "resource-limit");
        assert_eq!(allowed.views.len(), MAX_HOST_VIEW_RECORDS);

        for _ in 0..MAX_HOST_ACTION_RECORDS {
            assert_eq!(allowed.emit_action(ACTION.into()), "");
        }
        assert_eq!(allowed.emit_action(ACTION.into()), "resource-limit");
        assert_eq!(allowed.actions.len(), MAX_HOST_ACTION_RECORDS);
    }
}

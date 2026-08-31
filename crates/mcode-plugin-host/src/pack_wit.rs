//! Private Wasmtime bindings for the eleven current FeaturePack worlds.

// Rust guideline compliant 2026-08-31.

macro_rules! feature_bindings {
    ($module:ident, $world:literal) => {
        #[allow(missing_docs, reason = "generated Wasmtime bindings")]
        pub(crate) mod $module {
            wasmtime::component::bindgen!({
                path: "../mcode-plugin-api/wit/feature-pack",
                world: $world,
                exports: { default: async },
            });
        }
    };
}

feature_bindings!(session, "session");
feature_bindings!(compaction, "compaction");
feature_bindings!(resources, "resources");
feature_bindings!(ask, "ask");
feature_bindings!(todo, "todo");
feature_bindings!(web, "web");
feature_bindings!(mcp, "mcp");
feature_bindings!(usage, "usage");
feature_bindings!(subagents, "subagents");
feature_bindings!(workspace, "workspace");
feature_bindings!(ui, "ui");

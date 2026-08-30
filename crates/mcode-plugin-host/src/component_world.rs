//! Closed selection of sole-current component worlds.

// Rust guideline compliant 2026-08-30.

/// One sole-current plugin component world accepted by static preflight.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ComponentWorld {
    /// Manager lifecycle world.
    Manager,
    /// Session FeaturePack world.
    Session,
    /// Compaction FeaturePack world.
    Compaction,
    /// Resources FeaturePack world.
    Resources,
    /// Ask FeaturePack world.
    Ask,
    /// Todo FeaturePack world.
    Todo,
    /// Web FeaturePack world.
    Web,
    /// MCP FeaturePack world.
    Mcp,
    /// Usage FeaturePack world.
    Usage,
    /// Subagents FeaturePack world.
    Subagents,
    /// Workspace FeaturePack world.
    Workspace,
    /// UI FeaturePack world.
    Ui,
    /// ProviderPack world.
    Provider,
}

impl ComponentWorld {
    /// Every accepted world in stable family order.
    pub const ALL: [Self; 13] = [
        Self::Manager,
        Self::Session,
        Self::Compaction,
        Self::Resources,
        Self::Ask,
        Self::Todo,
        Self::Web,
        Self::Mcp,
        Self::Usage,
        Self::Subagents,
        Self::Workspace,
        Self::Ui,
        Self::Provider,
    ];

    pub(crate) const fn index(self) -> usize {
        self as usize
    }

    pub(crate) const fn imports(self) -> &'static [&'static str] {
        match self {
            Self::Manager => &["mcode:plugin/feature-service@0.0.1"],
            Self::Session => &["mcode:feature-pack/session-host@0.0.1"],
            Self::Compaction => &["mcode:feature-pack/compaction-host@0.0.1"],
            Self::Resources | Self::Ui | Self::Provider => &[],
            Self::Ask => &["mcode:feature-pack/ask-host@0.0.1"],
            Self::Todo => &["mcode:feature-pack/todo-host@0.0.1"],
            Self::Web => &["mcode:feature-pack/web-host@0.0.1"],
            Self::Mcp => &["mcode:feature-pack/mcp-host@0.0.1"],
            Self::Usage => &["mcode:feature-pack/usage-host@0.0.1"],
            Self::Subagents => &["mcode:feature-pack/subagents-host@0.0.1"],
            Self::Workspace => &["mcode:feature-pack/workspace-host@0.0.1"],
        }
    }

    pub(crate) const fn exports(self) -> &'static [&'static str] {
        match self {
            Self::Manager => &["mcode:plugin/manager-lifecycle@0.0.1"],
            Self::Session => &["mcode:feature-pack/session-pack@0.0.1"],
            Self::Compaction => &["mcode:feature-pack/compaction-pack@0.0.1"],
            Self::Resources => &["mcode:feature-pack/resources-pack@0.0.1"],
            Self::Ask => &["mcode:feature-pack/ask-pack@0.0.1"],
            Self::Todo => &["mcode:feature-pack/todo-pack@0.0.1"],
            Self::Web => &["mcode:feature-pack/web-pack@0.0.1"],
            Self::Mcp => &["mcode:feature-pack/mcp-pack@0.0.1"],
            Self::Usage => &["mcode:feature-pack/usage-pack@0.0.1"],
            Self::Subagents => &["mcode:feature-pack/subagents-pack@0.0.1"],
            Self::Workspace => &["mcode:feature-pack/workspace-pack@0.0.1"],
            Self::Ui => &["mcode:feature-pack/ui-pack@0.0.1"],
            Self::Provider => &["mcode:provider-pack/provider-api@0.0.1"],
        }
    }

    pub(crate) const fn reference_bytes(self) -> &'static [u8] {
        match self {
            Self::Manager => include_bytes!(concat!(env!("OUT_DIR"), "/manager.wasm")),
            Self::Session => include_bytes!(concat!(env!("OUT_DIR"), "/session.wasm")),
            Self::Compaction => include_bytes!(concat!(env!("OUT_DIR"), "/compaction.wasm")),
            Self::Resources => include_bytes!(concat!(env!("OUT_DIR"), "/resources.wasm")),
            Self::Ask => include_bytes!(concat!(env!("OUT_DIR"), "/ask.wasm")),
            Self::Todo => include_bytes!(concat!(env!("OUT_DIR"), "/todo.wasm")),
            Self::Web => include_bytes!(concat!(env!("OUT_DIR"), "/web.wasm")),
            Self::Mcp => include_bytes!(concat!(env!("OUT_DIR"), "/mcp.wasm")),
            Self::Usage => include_bytes!(concat!(env!("OUT_DIR"), "/usage.wasm")),
            Self::Subagents => include_bytes!(concat!(env!("OUT_DIR"), "/subagents.wasm")),
            Self::Workspace => include_bytes!(concat!(env!("OUT_DIR"), "/workspace.wasm")),
            Self::Ui => include_bytes!(concat!(env!("OUT_DIR"), "/ui.wasm")),
            Self::Provider => include_bytes!(concat!(env!("OUT_DIR"), "/provider.wasm")),
        }
    }
}

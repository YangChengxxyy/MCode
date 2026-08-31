//! Typed instantiation for the twelve current Pack worlds.

// Rust guideline compliant 2026-08-31.

use wasmtime::component::{HasSelf, Linker, Resource};

use crate::ComponentWorld;
use crate::pack_wit;
use crate::pack_wit::ask::mcode::feature_pack::ask_host;
use crate::pack_wit::compaction::mcode::feature_pack::compaction_host;
use crate::pack_wit::mcp::mcode::feature_pack::mcp_host;
use crate::pack_wit::session::mcode::feature_pack::session_host;
use crate::pack_wit::subagents::mcode::feature_pack::subagents_host;
use crate::pack_wit::todo::mcode::feature_pack::todo_host;
use crate::pack_wit::usage::mcode::feature_pack::usage_host;
use crate::pack_wit::web::mcode::feature_pack::web_host;
use crate::pack_wit::workspace::mcode::feature_pack::workspace_host;
use crate::provider_wit;

use super::owner::{CompiledPackComponent, InstantiationExecution, OwnerIdentity, StoreData};
use super::{PluginOwner, RuntimeError};

macro_rules! instantiate_with {
    ($owner:expr, $component:expr, $linker:expr, $bindings:path, $variant:path) => {{
        let mut execution = InstantiationExecution::start($owner)?;
        let result =
            <$bindings>::instantiate_async(execution.store_mut(), $component.component(), &$linker)
                .await;
        match result {
            Ok(bindings) => {
                execution.complete()?;
                Ok($variant(bindings))
            }
            Err(_) => Err(RuntimeError::Instantiation),
        }
    }};
}

macro_rules! instantiate_importing {
    ($owner:expr, $component:expr, $bindings:path, $variant:path) => {{
        let mut linker = Linker::new($owner.runtime.engine()?);
        <$bindings>::add_to_linker::<_, HasSelf<_>>(&mut linker, |data| data)
            .map_err(|_| RuntimeError::Instantiation)?;
        instantiate_with!($owner, $component, linker, $bindings, $variant)
    }};
}

macro_rules! instantiate_zero_import {
    ($owner:expr, $component:expr, $bindings:path, $variant:path) => {{
        let linker = Linker::new($owner.runtime.engine()?);
        instantiate_with!($owner, $component, linker, $bindings, $variant)
    }};
}

/// Holds one typed Pack instance bound to its exclusive Store owner.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the generation-bound Pack activation layer consumes typed instances"
    )
)]
pub(crate) struct PackInstance {
    #[cfg_attr(
        test,
        expect(
            dead_code,
            reason = "the execution layer will enforce owner identity before typed guest calls"
        )
    )]
    owner: OwnerIdentity,
    bindings: PackBindings,
}

#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the generation-bound Pack activation layer consumes typed instances"
    )
)]
impl PackInstance {
    /// Returns the exact typed world instantiated in this Store.
    pub(crate) const fn world(&self) -> ComponentWorld {
        self.bindings.world()
    }
}

#[expect(
    dead_code,
    reason = "typed bindings must remain alive for the later guest-call boundary"
)]
enum PackBindings {
    Session(pack_wit::session::Session),
    Compaction(pack_wit::compaction::Compaction),
    Resources(pack_wit::resources::Resources),
    Ask(pack_wit::ask::Ask),
    Todo(pack_wit::todo::Todo),
    Web(pack_wit::web::Web),
    Mcp(pack_wit::mcp::Mcp),
    Usage(pack_wit::usage::Usage),
    Subagents(pack_wit::subagents::Subagents),
    Workspace(pack_wit::workspace::Workspace),
    Ui(pack_wit::ui::Ui),
    Provider(provider_wit::Provider),
}

#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the generation-bound Pack activation layer observes the sealed world"
    )
)]
impl PackBindings {
    const fn world(&self) -> ComponentWorld {
        match self {
            Self::Session(_) => ComponentWorld::Session,
            Self::Compaction(_) => ComponentWorld::Compaction,
            Self::Resources(_) => ComponentWorld::Resources,
            Self::Ask(_) => ComponentWorld::Ask,
            Self::Todo(_) => ComponentWorld::Todo,
            Self::Web(_) => ComponentWorld::Web,
            Self::Mcp(_) => ComponentWorld::Mcp,
            Self::Usage(_) => ComponentWorld::Usage,
            Self::Subagents(_) => ComponentWorld::Subagents,
            Self::Workspace(_) => ComponentWorld::Workspace,
            Self::Ui(_) => ComponentWorld::Ui,
            Self::Provider(_) => ComponentWorld::Provider,
        }
    }
}

impl PluginOwner {
    /// Instantiates one typed FeaturePack or ProviderPack component.
    ///
    /// The compiled component supplies its sealed world and runtime identity.
    /// Any failed or cancelled Wasmtime instantiation disposes this owner's Store.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError::RuntimeMismatch`] for an artifact from another
    /// runtime, [`RuntimeError::InstanceActive`] after this Store has already
    /// instantiated a component, or a Store, fuel, linker, or instantiation
    /// error from the fail-closed runtime boundary.
    pub(crate) async fn instantiate_pack(
        &mut self,
        component: &CompiledPackComponent,
    ) -> Result<PackInstance, RuntimeError> {
        if !std::sync::Arc::ptr_eq(&self.runtime, component.runtime()) {
            return Err(RuntimeError::RuntimeMismatch);
        }
        if self.store.is_none() {
            return Err(RuntimeError::StoreDisposed);
        }
        if self.instance_instantiated {
            return Err(RuntimeError::InstanceActive);
        }

        let owner = self.identity.clone();
        let bindings = match component.world() {
            ComponentWorld::Manager => return Err(RuntimeError::InvalidPackWorld),
            ComponentWorld::Session => instantiate_importing!(
                self,
                component,
                pack_wit::session::Session,
                PackBindings::Session
            ),
            ComponentWorld::Compaction => instantiate_importing!(
                self,
                component,
                pack_wit::compaction::Compaction,
                PackBindings::Compaction
            ),
            ComponentWorld::Resources => instantiate_zero_import!(
                self,
                component,
                pack_wit::resources::Resources,
                PackBindings::Resources
            ),
            ComponentWorld::Ask => {
                instantiate_importing!(self, component, pack_wit::ask::Ask, PackBindings::Ask)
            }
            ComponentWorld::Todo => {
                instantiate_importing!(self, component, pack_wit::todo::Todo, PackBindings::Todo)
            }
            ComponentWorld::Web => {
                instantiate_importing!(self, component, pack_wit::web::Web, PackBindings::Web)
            }
            ComponentWorld::Mcp => {
                instantiate_importing!(self, component, pack_wit::mcp::Mcp, PackBindings::Mcp)
            }
            ComponentWorld::Usage => {
                instantiate_importing!(self, component, pack_wit::usage::Usage, PackBindings::Usage)
            }
            ComponentWorld::Subagents => instantiate_importing!(
                self,
                component,
                pack_wit::subagents::Subagents,
                PackBindings::Subagents
            ),
            ComponentWorld::Workspace => instantiate_importing!(
                self,
                component,
                pack_wit::workspace::Workspace,
                PackBindings::Workspace
            ),
            ComponentWorld::Ui => {
                instantiate_zero_import!(self, component, pack_wit::ui::Ui, PackBindings::Ui)
            }
            ComponentWorld::Provider => instantiate_zero_import!(
                self,
                component,
                provider_wit::Provider,
                PackBindings::Provider
            ),
        }?;
        self.instance_instantiated = true;
        Ok(PackInstance { owner, bindings })
    }
}

impl session_host::Host for StoreData {
    fn load_ledger(
        &mut self,
        _request: session_host::LedgerRead,
    ) -> Result<session_host::LedgerPage, session_host::SessionHostError> {
        Err(session_host::SessionHostError::Unavailable)
    }

    fn commit_ledger(
        &mut self,
        _mutation: session_host::LedgerMutation,
    ) -> Result<session_host::LedgerCommit, session_host::SessionHostError> {
        Err(session_host::SessionHostError::Unavailable)
    }
}

impl compaction_host::Host for StoreData {
    fn summarize(
        &mut self,
        _request: compaction_host::SummaryRequest,
    ) -> Result<compaction_host::SummaryOutput, compaction_host::CompactionHostError> {
        Err(compaction_host::CompactionHostError::Unavailable)
    }
}

impl ask_host::Host for StoreData {
    fn present(
        &mut self,
        _request: ask_host::InteractionRequest,
    ) -> Result<ask_host::InteractionOutput, ask_host::AskHostError> {
        Err(ask_host::AskHostError::InteractionUnavailable)
    }
}

impl todo_host::Host for StoreData {
    fn load_tasks(
        &mut self,
        _request: todo_host::TaskRead,
    ) -> Result<todo_host::TaskPage, todo_host::TodoHostError> {
        Err(todo_host::TodoHostError::Unavailable)
    }

    fn commit_task_event(
        &mut self,
        _mutation: todo_host::TaskMutation,
    ) -> Result<todo_host::TaskCommit, todo_host::TodoHostError> {
        Err(todo_host::TodoHostError::Unavailable)
    }
}

impl web_host::HostWebExchange for StoreData {
    fn drop(&mut self, _rep: Resource<web_host::WebExchange>) -> wasmtime::Result<()> {
        Ok(())
    }

    fn pull(&mut self, _self_: Resource<web_host::WebExchange>) -> web_host::WebExchangePull {
        web_host::WebExchangePull::Pending
    }
}

impl web_host::Host for StoreData {
    fn start_search(
        &mut self,
        _request: web_host::TypedSearch,
    ) -> Result<Resource<web_host::WebExchange>, web_host::WebHostError> {
        Err(web_host::WebHostError::RemoteUnavailable)
    }

    fn start_fetch(
        &mut self,
        _request: web_host::TypedFetch,
    ) -> Result<Resource<web_host::WebExchange>, web_host::WebHostError> {
        Err(web_host::WebHostError::RemoteUnavailable)
    }
}

impl mcp_host::HostMcpExchange for StoreData {
    fn drop(&mut self, _rep: Resource<mcp_host::McpExchange>) -> wasmtime::Result<()> {
        Ok(())
    }

    fn pull(&mut self, _self_: Resource<mcp_host::McpExchange>) -> mcp_host::McpExchangePull {
        mcp_host::McpExchangePull::Pending
    }
}

impl mcp_host::Host for StoreData {
    fn start_invoke(
        &mut self,
        _request: mcp_host::TypedInvocation,
    ) -> Result<Resource<mcp_host::McpExchange>, mcp_host::McpHostError> {
        Err(mcp_host::McpHostError::TransportUnavailable)
    }
}

impl usage_host::HostUsageExchange for StoreData {
    fn drop(&mut self, _rep: Resource<usage_host::UsageExchange>) -> wasmtime::Result<()> {
        Ok(())
    }

    fn pull(
        &mut self,
        _self_: Resource<usage_host::UsageExchange>,
    ) -> usage_host::UsageExchangePull {
        usage_host::UsageExchangePull::Pending
    }
}

impl usage_host::Host for StoreData {
    fn start_refresh(
        &mut self,
    ) -> Result<Resource<usage_host::UsageExchange>, usage_host::UsageHostError> {
        Err(usage_host::UsageHostError::SourceUnavailable)
    }
}

impl subagents_host::Host for StoreData {
    fn run_step(
        &mut self,
        _request: subagents_host::StepRequest,
    ) -> Result<subagents_host::StepOutput, subagents_host::SubagentsHostError> {
        Err(subagents_host::SubagentsHostError::Unavailable)
    }

    fn recover_step(
        &mut self,
        _request: subagents_host::RecoveryRequest,
    ) -> Result<subagents_host::RecoveryOutput, subagents_host::SubagentsHostError> {
        Err(subagents_host::SubagentsHostError::Unavailable)
    }
}

impl workspace_host::Host for StoreData {
    fn scan(
        &mut self,
        _request: workspace_host::ScanRequest,
    ) -> Result<workspace_host::ScanPage, workspace_host::WorkspaceHostError> {
        Err(workspace_host::WorkspaceHostError::Unavailable)
    }

    fn apply_rollback(
        &mut self,
        _request: workspace_host::RollbackRequest,
    ) -> Result<workspace_host::RollbackOutput, workspace_host::WorkspaceHostError> {
        Err(workspace_host::WorkspaceHostError::Unavailable)
    }
}

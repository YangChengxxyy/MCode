//! Exclusive Store ownership, compilation binding, and safe admission APIs.

// Rust guideline compliant 2026-08-31.

use std::sync::Arc;
use std::time::Duration;

use mcode_config::PluginFamily;
use wasmtime::Store;
use wasmtime::component::{Access, Component, HasSelf, Linker as ComponentLinker, ResourceTable};
#[cfg(test)]
use wasmtime::{Instance, Linker, Module};

use crate::FeatureCaller;
use crate::manager_director::{GenerationActivity, GenerationFence};
use crate::pack_activation::{PackActivationClient, PackActivationError};
use crate::wit::Manager;
use crate::wit::mcode::plugin::feature_service::{
    Host as GatewayHost, HostWithStore as GatewayHostWithStore, PackServiceError,
};

use super::admission::{AdmissionLedger, OperationPermit};
use super::epoch::{arm_guest_deadline, park_guest_deadline};
use super::limits::StoreResourceLimiter;
use super::{
    FeatureDeadlinePolicyV1, HOSTCALL_FUEL, OPERATION_FUEL_BUDGET, RESOURCE_TABLE_CAPACITY,
    ResourcePermit, RuntimeError, RuntimeInner,
};

const INSTANTIATION_FUEL: u64 = OPERATION_FUEL_BUDGET;
// Yield often enough for cancellation to preempt CPU-bound Wasm without
// turning every instruction into an executor handoff.
const ASYNC_YIELD_FUEL_INTERVAL: u64 = 100_000;

#[derive(Clone)]
pub(super) struct OwnerIdentity(Arc<OwnerToken>);

impl OwnerIdentity {
    fn new() -> Self {
        Self(Arc::new(OwnerToken))
    }
}

impl PartialEq for OwnerIdentity {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for OwnerIdentity {}

struct OwnerToken;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct OperationIdentity(u64);

#[derive(Clone, PartialEq, Eq)]
pub(super) struct ActiveSegment {
    pub(super) owner: OwnerIdentity,
    pub(super) operation: OperationIdentity,
    pub(super) installed: u64,
}

pub(super) struct StoreData {
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "future typed Host adapters will push resources")
    )]
    pub(super) resources: ResourceTable,
    pub(super) admission: AdmissionLedger,
    pub(super) limiter: StoreResourceLimiter,
    pub(super) active_segment: Option<ActiveSegment>,
    generation_fence: Option<Arc<GenerationFence>>,
    pack_activation: Option<PackActivationClient>,
    feature_caller: Option<FeatureCaller>,
    feature_deadlines: Option<FeatureDeadlinePolicyV1>,
}

impl StoreData {
    fn new(feature_deadlines: Option<FeatureDeadlinePolicyV1>) -> Self {
        let mut resources = ResourceTable::new();
        resources.set_max_capacity(RESOURCE_TABLE_CAPACITY);
        Self {
            resources,
            admission: AdmissionLedger::new(),
            limiter: StoreResourceLimiter::new(),
            active_segment: None,
            generation_fence: None,
            pack_activation: None,
            feature_caller: None,
            feature_deadlines,
        }
    }

    pub(super) const fn feature_deadline(&self, family: PluginFamily) -> Option<Duration> {
        match self.feature_deadlines {
            Some(policy) => policy.duration(family),
            None => None,
        }
    }

    pub(super) const fn feature_caller(&self) -> Option<FeatureCaller> {
        self.feature_caller
    }

    pub(super) fn pack_activation_mut(&mut self) -> Option<&mut PackActivationClient> {
        self.pack_activation.as_mut()
    }

    pub(super) fn enter_current_generation(&self) -> Option<GenerationActivity> {
        self.generation_fence.as_ref()?.enter()
    }

    fn configured_pack_selection(
        &mut self,
    ) -> Result<crate::wit::mcode::plugin::feature_service::PackSelectionView, PackServiceError>
    {
        let _activity = self
            .enter_current_generation()
            .ok_or(PackServiceError::StaleGeneration)?;
        let selection = self
            .pack_activation
            .as_mut()
            .ok_or(PackServiceError::Unavailable)?
            .configured_selection()
            .map_err(|_| PackServiceError::Unavailable)?;
        let (selection_stamp, pack_ids) = selection.into_wire();
        Ok(
            crate::wit::mcode::plugin::feature_service::PackSelectionView {
                selection_stamp,
                pack_ids,
            },
        )
    }
}

impl GatewayHost for StoreData {}

impl GatewayHostWithStore<StoreData> for HasSelf<StoreData> {
    async fn configured_packs(
        mut host: Access<'_, StoreData, Self>,
    ) -> Result<crate::wit::mcode::plugin::feature_service::PackSelectionView, PackServiceError>
    {
        host.get().configured_pack_selection()
    }

    async fn activate_packs(
        mut host: Access<'_, StoreData, Self>,
        selection_stamp: String,
    ) -> Result<crate::wit::mcode::plugin::feature_service::ActivatedPackSet, PackServiceError>
    {
        let activity = host
            .get()
            .enter_current_generation()
            .ok_or(PackServiceError::StaleGeneration)?;
        let selection_stamp = host
            .get()
            .pack_activation
            .as_mut()
            .ok_or(PackServiceError::Unavailable)?
            .activate(&activity, &selection_stamp)
            .await
            .map_err(map_pack_activation_error)?;
        Ok(crate::wit::mcode::plugin::feature_service::ActivatedPackSet { selection_stamp })
    }

    async fn start_task(mut host: Access<'_, StoreData, Self>, request: String) -> String {
        super::resources_gateway::start_task(host.get(), request).await
    }

    async fn poll_task(mut host: Access<'_, StoreData, Self>, request: String) -> String {
        super::resources_gateway::poll_task(host.get(), request).await
    }

    async fn cancel_task(mut host: Access<'_, StoreData, Self>, request: String) -> String {
        super::resources_gateway::cancel_task(host.get(), request).await
    }
}

const fn map_pack_activation_error(error: PackActivationError) -> PackServiceError {
    match error {
        PackActivationError::InvalidSelection => PackServiceError::InvalidSelection,
        PackActivationError::StaleGeneration => PackServiceError::StaleGeneration,
        PackActivationError::Limit => PackServiceError::Limit,
        PackActivationError::Unavailable => PackServiceError::Unavailable,
        PackActivationError::Failed => PackServiceError::Failed,
    }
}

/// Holds an exact Manager component compiled by one [`super::PluginRuntime`].
///
/// Its validated Wasmtime component and engine binding are private and cannot
/// be replaced.
pub struct CompiledManagerComponent {
    runtime: Arc<RuntimeInner>,
    component: Component,
}

impl CompiledManagerComponent {
    pub(super) fn new(runtime: Arc<RuntimeInner>, component: Component) -> Self {
        Self { runtime, component }
    }
}

/// Holds one exact FeaturePack or ProviderPack component compiled by a runtime.
///
/// The component remains crate-private until the typed Pack instantiation
/// boundary consumes it. Compilation alone never creates a Store or executes
/// guest code.
pub(crate) struct CompiledPackComponent {
    runtime: Arc<RuntimeInner>,
    world: crate::ComponentWorld,
    component: Component,
}

impl CompiledPackComponent {
    pub(super) fn new(
        runtime: Arc<RuntimeInner>,
        world: crate::ComponentWorld,
        component: Component,
    ) -> Self {
        Self {
            runtime,
            world,
            component,
        }
    }

    pub(super) const fn runtime(&self) -> &Arc<RuntimeInner> {
        &self.runtime
    }

    pub(super) const fn world(&self) -> crate::ComponentWorld {
        self.world
    }

    pub(super) const fn component(&self) -> &Component {
        &self.component
    }
}

#[cfg(test)]
pub(super) struct CompiledTestModule {
    runtime: Arc<RuntimeInner>,
    module: Module,
}

#[cfg(test)]
impl CompiledTestModule {
    pub(super) fn new(runtime: Arc<RuntimeInner>, module: Module) -> Self {
        Self { runtime, module }
    }
}

/// Owns one Store and all mutable execution policy attached to it.
///
/// There is no Store accessor and the wrapper implements no raw conversion:
///
/// ```compile_fail
/// use mcode_plugin_host::runtime::PluginRuntime;
/// let runtime = PluginRuntime::new();
/// let mut owner = runtime.new_owner().unwrap();
/// owner.set_fuel(1).unwrap();
/// ```
///
/// ```compile_fail
/// use mcode_plugin_host::runtime::PluginRuntime;
/// let runtime = PluginRuntime::new();
/// let mut owner = runtime.new_owner().unwrap();
/// owner.set_hostcall_fuel(usize::MAX);
/// ```
///
/// ```compile_fail
/// use mcode_plugin_host::runtime::PluginRuntime;
/// let runtime = PluginRuntime::new();
/// let mut owner = runtime.new_owner().unwrap();
/// owner.limiter_async(|_| unreachable!());
/// ```
///
/// ```compile_fail
/// use mcode_plugin_host::runtime::PluginRuntime;
/// let runtime = PluginRuntime::new();
/// let mut owner = runtime.new_owner().unwrap();
/// owner.resources_mut().set_max_capacity(usize::MAX);
/// ```
///
/// ```compile_fail
/// use mcode_plugin_host::runtime::PluginRuntime;
/// let runtime = PluginRuntime::new();
/// let owner = runtime.new_owner().unwrap();
/// let _raw_store = owner.store();
/// ```
///
/// ```compile_fail
/// use mcode_plugin_host::runtime::PluginRuntime;
/// fn takes_store(_: &mut wasmtime::Store<()>) {}
/// let runtime = PluginRuntime::new();
/// let mut owner = runtime.new_owner().unwrap();
/// takes_store(&mut owner);
/// ```
pub struct PluginOwner {
    pub(super) runtime: Arc<RuntimeInner>,
    pub(super) identity: OwnerIdentity,
    pub(super) store: Option<Store<StoreData>>,
    pub(super) instance_instantiated: bool,
    next_operation: u64,
}

impl PluginOwner {
    pub(super) fn new(runtime: Arc<RuntimeInner>) -> Result<Self, RuntimeError> {
        runtime.ensure_epoch_ticker()?;
        let mut store = Store::new(
            runtime.engine()?,
            StoreData::new(runtime.feature_deadline_policy()),
        );
        store.limiter_async(|data| &mut data.limiter);
        store.set_fuel(0).map_err(|_| RuntimeError::Fuel)?;
        store
            .fuel_async_yield_interval(Some(ASYNC_YIELD_FUEL_INTERVAL))
            .map_err(|_| RuntimeError::Fuel)?;
        store.set_hostcall_fuel(HOSTCALL_FUEL);
        park_guest_deadline(&mut store);
        Ok(Self {
            runtime,
            identity: OwnerIdentity::new(),
            store: Some(store),
            instance_instantiated: false,
            next_operation: 1,
        })
    }

    /// Returns whether this owner still has a usable Store.
    ///
    /// Failed instantiation, trapped or cancelled Manager execution, and any
    /// policy invariant failure dispose the Store permanently.
    #[must_use]
    pub const fn is_available(&self) -> bool {
        self.store.is_some()
    }

    pub(crate) fn bind_generation_context(
        &mut self,
        fence: Arc<GenerationFence>,
        pack_activation: PackActivationClient,
        feature_caller: FeatureCaller,
    ) -> Result<(), RuntimeError> {
        let store = self.store.as_ref().ok_or(RuntimeError::StoreDisposed)?;
        if store.data().generation_fence.is_some()
            || store.data().pack_activation.is_some()
            || store.data().feature_caller.is_some()
        {
            drop(self.store.take());
            return Err(RuntimeError::GenerationBound);
        }
        let store = self.store.as_mut().ok_or(RuntimeError::StoreDisposed)?;
        store.data_mut().generation_fence = Some(fence);
        store.data_mut().pack_activation = Some(pack_activation);
        store.data_mut().feature_caller = Some(feature_caller);
        Ok(())
    }

    /// Reserves one Host-visible resource slot until the permit is dropped.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError::StoreDisposed`] after Store disposal, or
    /// [`RuntimeError::Admission`] when all 4,096 slots are live.
    pub fn admit_resource(&self) -> Result<ResourcePermit, RuntimeError> {
        let store = self.store.as_ref().ok_or(RuntimeError::StoreDisposed)?;
        store
            .data()
            .admission
            .admit_resource()
            .map_err(RuntimeError::from)
    }

    /// Mints an owner-bound operation lease with exactly 100,000,000 fuel.
    ///
    /// Capacity remains reserved until the returned lease is dropped. The lease
    /// is opaque, non-cloneable, and accepted only by its original owner.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError::StoreDisposed`] after Store disposal,
    /// [`RuntimeError::IdentityExhausted`] if operation identities wrap, or
    /// [`RuntimeError::Admission`] when all 1,024 operation slots are open.
    pub fn open_operation(&mut self) -> Result<OperationLease, RuntimeError> {
        let store = self.store.as_ref().ok_or(RuntimeError::StoreDisposed)?;
        let permit = store.data().admission.open_operation()?;
        let operation = self.next_operation;
        if operation == 0 || operation == u64::MAX {
            return Err(RuntimeError::IdentityExhausted);
        }
        self.next_operation += 1;
        Ok(OperationLease {
            owner: self.identity.clone(),
            operation: OperationIdentity(operation),
            remaining: OPERATION_FUEL_BUDGET,
            permit: Some(permit),
        })
    }

    /// Instantiates a preflighted Manager component asynchronously.
    ///
    /// Any failed or cancelled instantiation disposes this owner's Store because
    /// Wasmtime can leave allowed initial allocations without paired failure
    /// callbacks. A fresh owner starts with a fresh aggregate ledger.
    /// FeatureService imports fail closed until the lifecycle layer binds an
    /// active Manager caller.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError::RuntimeMismatch`] for an artifact from another
    /// runtime, [`RuntimeError::InstanceActive`] after this Store successfully
    /// instantiated its one component, [`RuntimeError::StoreDisposed`] if this owner
    /// is unavailable, [`RuntimeError::Fuel`] if instantiation fuel cannot be
    /// parked, or [`RuntimeError::Instantiation`] when linking or instantiation
    /// fails.
    pub async fn instantiate_manager(
        &mut self,
        component: &CompiledManagerComponent,
    ) -> Result<ManagerInstance, RuntimeError> {
        if !Arc::ptr_eq(&self.runtime, &component.runtime) {
            return Err(RuntimeError::RuntimeMismatch);
        }
        if self.store.is_none() {
            return Err(RuntimeError::StoreDisposed);
        }
        if self.instance_instantiated {
            return Err(RuntimeError::InstanceActive);
        }
        let mut linker = ComponentLinker::new(self.runtime.engine()?);
        Manager::add_to_linker::<_, HasSelf<_>>(&mut linker, |data| data)
            .map_err(|_| RuntimeError::Instantiation)?;

        let identity = self.identity.clone();
        #[cfg(test)]
        let runtime = Arc::clone(&self.runtime);
        let mut execution = InstantiationExecution::start(self)?;
        let result =
            Manager::instantiate_async(execution.store_mut(), &component.component, &linker).await;
        match result {
            Ok(bindings) => {
                execution.complete()?;
                self.instance_instantiated = true;
                Ok(ManagerInstance {
                    owner: identity,
                    bindings,
                    #[cfg(test)]
                    runtime,
                })
            }
            Err(_) => Err(RuntimeError::Instantiation),
        }
    }

    #[cfg(test)]
    pub(super) async fn instantiate_test_module(
        &mut self,
        module: &CompiledTestModule,
    ) -> Result<CorePluginInstance, RuntimeError> {
        if !Arc::ptr_eq(&self.runtime, &module.runtime) {
            return Err(RuntimeError::RuntimeMismatch);
        }
        let identity = self.identity.clone();
        let mut execution = InstantiationExecution::start(self)?;
        let result = Instance::new_async(execution.store_mut(), &module.module, &[]).await;
        match result {
            Ok(instance) => {
                execution.complete()?;
                Ok(CorePluginInstance {
                    owner: identity,
                    instance,
                })
            }
            Err(_) => Err(RuntimeError::Instantiation),
        }
    }

    #[cfg(test)]
    pub(super) async fn instantiate_with_linker(
        &mut self,
        module: &CompiledTestModule,
        linker: &Linker<StoreData>,
    ) -> Result<CorePluginInstance, RuntimeError> {
        if !Arc::ptr_eq(&self.runtime, &module.runtime) {
            return Err(RuntimeError::RuntimeMismatch);
        }
        let identity = self.identity.clone();
        let mut execution = InstantiationExecution::start(self)?;
        let result = linker
            .instantiate_async(execution.store_mut(), &module.module)
            .await;
        match result {
            Ok(instance) => {
                execution.complete()?;
                Ok(CorePluginInstance {
                    owner: identity,
                    instance,
                })
            }
            Err(_) => Err(RuntimeError::Instantiation),
        }
    }

    pub(super) fn take_store(&mut self) -> Result<Store<StoreData>, RuntimeError> {
        self.store.take().ok_or(RuntimeError::StoreDisposed)
    }

    pub(super) fn restore_store(&mut self, store: Store<StoreData>) {
        debug_assert!(self.store.is_none());
        self.store = Some(store);
    }
}

/// Holds one Manager instance bound to its exclusive Store owner.
///
/// Lifecycle entry points are asynchronous; no synchronous guest API exists:
///
/// ```compile_fail
/// use mcode_plugin_host::runtime::{ManagerInstance, OperationLease, PluginOwner};
/// fn call_sync(
///     instance: &ManagerInstance,
///     owner: &mut PluginOwner,
///     operation: &mut OperationLease,
/// ) {
///     instance.poll_sync(owner, operation);
/// }
/// ```
///
/// Generated Wasmtime bindings remain private:
///
/// ```compile_fail
/// use mcode_plugin_host::runtime::ManagerInstance;
/// fn expose_bindings(instance: &ManagerInstance) {
///     let _ = &instance.bindings;
/// }
/// ```
pub struct ManagerInstance {
    pub(super) owner: OwnerIdentity,
    pub(super) bindings: Manager,
    #[cfg(test)]
    pub(super) runtime: Arc<RuntimeInner>,
}

#[cfg(test)]
pub(super) struct CorePluginInstance {
    pub(super) owner: OwnerIdentity,
    pub(super) instance: Instance,
}

/// Carries one owner-bound operation identity and its saved fuel remainder.
///
/// The lease exposes no fuel state or controls. Dropping it releases one
/// operation admission slot.
///
/// ```compile_fail
/// use mcode_plugin_host::runtime::PluginRuntime;
/// let runtime = PluginRuntime::new();
/// let mut owner = runtime.new_owner().unwrap();
/// let lease = owner.open_operation().unwrap();
/// let _ = lease.remaining();
/// ```
#[must_use = "dropping the lease closes its operation admission"]
pub struct OperationLease {
    pub(super) owner: OwnerIdentity,
    pub(super) operation: OperationIdentity,
    pub(super) remaining: u64,
    permit: Option<OperationPermit>,
}

impl OperationLease {
    pub(super) fn take_admission(&mut self) -> Option<OperationPermit> {
        self.permit.take()
    }

    #[cfg(test)]
    pub(super) const fn remaining(&self) -> u64 {
        self.remaining
    }
}

pub(super) struct InstantiationExecution<'a> {
    owner: &'a mut PluginOwner,
    store: Option<Store<StoreData>>,
}

impl<'a> InstantiationExecution<'a> {
    pub(super) fn start(owner: &'a mut PluginOwner) -> Result<Self, RuntimeError> {
        let mut store = owner.take_store()?;
        if store.data().active_segment.is_some() || store.data().limiter.is_poisoned() {
            return Err(RuntimeError::StoreDisposed);
        }
        store
            .set_fuel(INSTANTIATION_FUEL)
            .map_err(|_| RuntimeError::Fuel)?;
        arm_guest_deadline(&mut store);
        Ok(Self {
            owner,
            store: Some(store),
        })
    }

    pub(super) fn store_mut(&mut self) -> &mut Store<StoreData> {
        self.store
            .as_mut()
            .expect("instantiation guard owns its Store until completion")
    }

    pub(super) fn complete(mut self) -> Result<(), RuntimeError> {
        let mut store = self
            .store
            .take()
            .expect("instantiation guard completes exactly once");
        if store.set_fuel(0).is_err() || store.data().limiter.is_poisoned() {
            return Err(RuntimeError::StoreDisposed);
        }
        park_guest_deadline(&mut store);
        self.owner.restore_store(store);
        Ok(())
    }
}

impl Drop for InstantiationExecution<'_> {
    fn drop(&mut self) {
        // An incomplete or failed instantiation drops the Store and its whole
        // monotone aggregate ledger. It is never returned to the owner.
        drop(self.store.take());
    }
}

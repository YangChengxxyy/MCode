//! Exclusive Store ownership, compilation binding, and safe admission APIs.

// Rust guideline compliant 2026-08-31.

use std::sync::Arc;

use mcode_plugin_api::{FeatureTaskRejection, TaskErrorCode, TaskFailure};
use wasmtime::Store;
use wasmtime::component::{Access, Component, HasSelf, Linker as ComponentLinker, ResourceTable};
#[cfg(test)]
use wasmtime::{Instance, Linker, Module};

use crate::wit::Manager;
use crate::wit::mcode::plugin::feature_service::{
    Host as GatewayHost, HostWithStore as GatewayHostWithStore,
};

use super::admission::{AdmissionLedger, OperationPermit};
use super::epoch::{arm_guest_deadline, park_guest_deadline};
use super::limits::StoreResourceLimiter;
use super::{
    HOSTCALL_FUEL, OPERATION_FUEL_BUDGET, RESOURCE_TABLE_CAPACITY, ResourcePermit, RuntimeError,
    RuntimeInner,
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
}

impl StoreData {
    fn new() -> Self {
        let mut resources = ResourceTable::new();
        resources.set_max_capacity(RESOURCE_TABLE_CAPACITY);
        Self {
            resources,
            admission: AdmissionLedger::new(),
            limiter: StoreResourceLimiter::new(),
            active_segment: None,
        }
    }
}

impl GatewayHost for StoreData {}

impl GatewayHostWithStore<StoreData> for HasSelf<StoreData> {
    async fn start_task(_host: Access<'_, StoreData, Self>, _request: String) -> String {
        unavailable_feature_response()
    }

    async fn poll_task(_host: Access<'_, StoreData, Self>, _request: String) -> String {
        unavailable_feature_response()
    }

    async fn cancel_task(_host: Access<'_, StoreData, Self>, _request: String) -> String {
        unavailable_feature_response()
    }
}

fn unavailable_feature_response() -> String {
    FeatureTaskRejection::new(TaskFailure::new(TaskErrorCode::FeatureUnavailable))
        .encode()
        .expect("the fixed FeatureService rejection must fit its wire bound")
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
    pack_instantiated: bool,
    next_operation: u64,
}

impl PluginOwner {
    pub(super) fn new(runtime: Arc<RuntimeInner>) -> Result<Self, RuntimeError> {
        runtime.ensure_epoch_ticker()?;
        let mut store = Store::new(runtime.engine()?, StoreData::new());
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
            pack_instantiated: false,
            next_operation: 1,
        })
    }

    /// Returns whether this owner still has a usable Store.
    ///
    /// Failed instantiation, trapped or cancelled lifecycle execution, and any
    /// policy invariant failure dispose the Store permanently.
    #[must_use]
    pub const fn is_available(&self) -> bool {
        self.store.is_some()
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
            _permit: permit,
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
    /// instantiated its one Pack, [`RuntimeError::StoreDisposed`] if this owner
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
        if self.pack_instantiated {
            return Err(RuntimeError::InstanceActive);
        }
        let mut linker = ComponentLinker::new(self.runtime.engine()?);
        Manager::add_to_linker::<_, HasSelf<_>>(&mut linker, |data| data)
            .map_err(|_| RuntimeError::Instantiation)?;

        let identity = self.identity.clone();
        let mut execution = InstantiationExecution::start(self)?;
        let result =
            Manager::instantiate_async(execution.store_mut(), &component.component, &linker).await;
        match result {
            Ok(bindings) => {
                execution.complete()?;
                self.pack_instantiated = true;
                Ok(ManagerInstance {
                    owner: identity,
                    bindings,
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
    _permit: OperationPermit,
}

impl OperationLease {
    #[cfg(test)]
    pub(super) const fn remaining(&self) -> u64 {
        self.remaining
    }
}

struct InstantiationExecution<'a> {
    owner: &'a mut PluginOwner,
    store: Option<Store<StoreData>>,
}

impl<'a> InstantiationExecution<'a> {
    fn start(owner: &'a mut PluginOwner) -> Result<Self, RuntimeError> {
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

    fn store_mut(&mut self) -> &mut Store<StoreData> {
        self.store
            .as_mut()
            .expect("instantiation guard owns its Store until completion")
    }

    fn complete(mut self) -> Result<(), RuntimeError> {
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

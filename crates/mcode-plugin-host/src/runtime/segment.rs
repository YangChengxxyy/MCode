//! Cancellation-safe guest-active segment ownership.

// Rust guideline compliant 2026-08-30.

use wasmtime::{Store, TypedFunc, WasmParams, WasmResults};

use super::epoch::{arm_guest_deadline, park_guest_deadline};
use super::owner::{ActiveSegment, CorePluginInstance, OperationLease, OwnerIdentity, StoreData};
use super::{PluginOwner, RuntimeError};

pub(super) struct GuestFunction<Params, Results> {
    owner: OwnerIdentity,
    function: TypedFunc<Params, Results>,
}

impl CorePluginInstance {
    pub(super) fn typed_function<Params, Results>(
        &self,
        owner: &mut PluginOwner,
        name: &str,
    ) -> Result<GuestFunction<Params, Results>, RuntimeError>
    where
        Params: WasmParams,
        Results: WasmResults,
    {
        if self.owner != owner.identity {
            return Err(RuntimeError::InstanceMismatch);
        }
        let store = owner.store.as_mut().ok_or(RuntimeError::StoreDisposed)?;
        let function = self
            .instance
            .get_typed_func(store, name)
            .map_err(|_| RuntimeError::Guest)?;
        Ok(GuestFunction {
            owner: self.owner.clone(),
            function,
        })
    }
}

impl PluginOwner {
    pub(super) async fn call_typed<Params, Results>(
        &mut self,
        lease: &mut OperationLease,
        function: &GuestFunction<Params, Results>,
        params: Params,
    ) -> Result<Results, RuntimeError>
    where
        Params: WasmParams + Sync,
        Results: WasmResults + Sync,
    {
        if lease.owner != self.identity {
            return Err(RuntimeError::OwnerMismatch);
        }
        if function.owner != self.identity {
            return Err(RuntimeError::InstanceMismatch);
        }

        let mut segment = SegmentExecution::start(self, lease)?;
        let guest_result = function
            .function
            .call_async(segment.store_mut(), params)
            .await;
        segment.complete()?;
        guest_result.map_err(|_| RuntimeError::Guest)
    }
}

struct SegmentExecution<'a> {
    owner: &'a mut PluginOwner,
    lease: &'a mut OperationLease,
    store: Option<Store<StoreData>>,
}

impl<'a> SegmentExecution<'a> {
    fn start(
        owner: &'a mut PluginOwner,
        lease: &'a mut OperationLease,
    ) -> Result<Self, RuntimeError> {
        let mut store = owner.take_store()?;
        if store.data().active_segment.is_some() {
            return Err(RuntimeError::SegmentActive);
        }
        if store.data().limiter.is_poisoned() {
            return Err(RuntimeError::StoreDisposed);
        }
        let installed = lease.remaining;
        if store.set_fuel(installed).is_err() {
            return Err(RuntimeError::Fuel);
        }
        arm_guest_deadline(&mut store);
        store.data_mut().active_segment = Some(ActiveSegment {
            owner: owner.identity.clone(),
            operation: lease.operation,
            installed,
        });
        Ok(Self {
            owner,
            lease,
            store: Some(store),
        })
    }

    fn store_mut(&mut self) -> &mut Store<StoreData> {
        self.store
            .as_mut()
            .expect("segment guard owns its Store until completion")
    }

    fn complete(mut self) -> Result<(), RuntimeError> {
        self.complete_inner()
    }

    fn complete_inner(&mut self) -> Result<(), RuntimeError> {
        let mut store = self
            .store
            .take()
            .expect("segment guard completes exactly once");
        let actual = store.get_fuel();
        let fuel_parked = store.set_fuel(0);
        park_guest_deadline(&mut store);
        let active = store.data_mut().active_segment.take();

        let Ok(actual) = actual else {
            return Err(RuntimeError::Fuel);
        };
        if fuel_parked.is_err() {
            return Err(RuntimeError::Fuel);
        }
        if store.data().limiter.is_poisoned() {
            return Err(RuntimeError::StoreDisposed);
        }
        let Some(active) = active else {
            return Err(RuntimeError::SegmentMismatch);
        };
        if active.owner != self.owner.identity
            || active.owner != self.lease.owner
            || active.operation != self.lease.operation
            || active.installed != self.lease.remaining
        {
            return Err(RuntimeError::SegmentMismatch);
        }
        if actual > active.installed {
            return Err(RuntimeError::FuelIncreased);
        }

        self.lease.remaining = actual;
        self.owner.restore_store(store);
        Ok(())
    }
}

impl Drop for SegmentExecution<'_> {
    fn drop(&mut self) {
        if self.store.is_some() {
            // Dropping a pending call future, unwinding a panic, or returning
            // early reaches this synchronous finalizer. Any failed invariant
            // leaves the owner without a Store, disposing the entire ledger.
            let _ = self.complete_inner();
        }
    }
}

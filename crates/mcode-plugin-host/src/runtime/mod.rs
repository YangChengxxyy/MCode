//! Fail-closed Wasmtime ownership and Host admission contracts.
//!
//! The public API deliberately exposes only policy-bound wrappers. Wasmtime
//! engines, stores, fuel controls, limiters, and resource tables stay inside
//! this module so callers cannot bypass the runtime invariants.

// Rust guideline compliant 2026-08-31.

mod admission;
mod deadline;
mod epoch;
mod lifecycle;
mod limits;
mod manager_task;
mod owner;
mod pack;
mod resources;
mod resources_gateway;
mod segment;
mod task_worker;

#[cfg(test)]
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use epoch::EpochTicker;
use wasmtime::Engine;
#[cfg(test)]
use wasmtime::Module;

use crate::component::{ComponentCache, scan_bounded_component};
use crate::{ComponentLimits, ComponentWorld, PreflightError};

pub use admission::{AdmissionError, MAX_LIVE_RESOURCES, MAX_OPEN_OPERATIONS, ResourcePermit};
pub use deadline::FeatureDeadlinePolicyV1;
pub use lifecycle::{LifecycleErrorCode, LifecycleOutcome, LifecycleState};
pub(crate) use manager_task::{ManagerTaskCall, ManagerTaskCallError};
pub(crate) use owner::CompiledPackComponent;
pub use owner::{CompiledManagerComponent, ManagerInstance, OperationLease, PluginOwner};
pub(crate) use pack::PackInstance;
pub(crate) use resources::{
    ResourcesPackActor, ResourcesPackCallError, ResourcesPackError, ResourcesPackPull,
};
pub(crate) use task_worker::TaskCloseSignal as ResourcesCloseSignal;
pub(crate) type ResourcesActorClient = task_worker::TaskActorClient<ResourcesPackActor>;
pub(crate) type ResourcesActorError = task_worker::TaskActorError<ResourcesPackCallError>;
pub(crate) type ResourcesActorOperationId = task_worker::TaskActorOperationId;

/// Deterministic total fuel budget shared by all segments of one operation.
pub const OPERATION_FUEL_BUDGET: u64 = 100_000_000;
/// Component hostcall fuel assigned independently to each Host call.
pub const HOSTCALL_FUEL: usize = 16 * 1024 * 1024;
/// Maximum entries retained by one Wasmtime component resource table.
pub const RESOURCE_TABLE_CAPACITY: usize = 4_096;

/// Reports failure at a fail-closed runtime boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RuntimeError {
    /// Wasmtime could not create the policy-configured engine.
    #[error("plugin runtime engine is unavailable")]
    Engine,
    /// No bounded, exact-shape component has initialized this runtime yet.
    #[error("plugin runtime has not compiled an exact component")]
    RuntimeUninitialized,
    /// The private monotonic epoch ticker could not be started.
    #[error("plugin runtime epoch ticker is unavailable")]
    EpochTicker,
    /// The Manager component failed bounded exact preflight.
    #[error(transparent)]
    Preflight(#[from] PreflightError),
    /// Manager initialization generation was outside the JSON-safe range.
    #[error("Manager generation is outside 1..=9,007,199,254,740,991")]
    InvalidGeneration,
    /// The Pack compilation boundary was given the Manager-only world.
    #[error("Manager world cannot be compiled as a Pack")]
    InvalidPackWorld,
    /// An operation identity could not be minted without wrapping.
    #[error("plugin operation identity space is exhausted")]
    IdentityExhausted,
    /// An artifact was compiled by a different runtime.
    #[error("plugin artifact belongs to a different runtime")]
    RuntimeMismatch,
    /// The Store already contains its one component instance.
    #[error("plugin Store already contains a component instance")]
    InstanceActive,
    /// The Store is already bound to one Manager generation.
    #[error("plugin Store already has a Manager generation binding")]
    GenerationBound,
    /// An operation lease belongs to a different Store owner.
    #[error("plugin operation belongs to a different Store owner")]
    OwnerMismatch,
    /// A guest function belongs to a different Store owner.
    #[error("plugin guest function belongs to a different Store owner")]
    InstanceMismatch,
    /// Another guest-active segment is already installed.
    #[error("plugin Store already has an active guest segment")]
    SegmentActive,
    /// The active segment identity or saved remainder was inconsistent.
    #[error("plugin guest segment identity is inconsistent")]
    SegmentMismatch,
    /// Wasmtime could not read, install, or park fuel.
    #[error("plugin runtime fuel is unavailable")]
    Fuel,
    /// Wasmtime reported more fuel than the operation installed.
    #[error("plugin operation fuel increased during execution")]
    FuelIncreased,
    /// The Store was disposed after a fail-closed boundary.
    #[error("plugin Store is disposed")]
    StoreDisposed,
    /// Wasmtime rejected asynchronous guest instantiation.
    #[error("plugin guest instantiation failed")]
    Instantiation,
    /// Guest execution trapped or returned a runtime error.
    #[error("plugin guest execution failed")]
    Guest,
    /// A Host admission class reached its fixed capacity.
    #[error(transparent)]
    Admission(#[from] AdmissionError),
}

/// Lazily owns one scanner-gated, policy-configured Wasmtime engine.
///
/// Raw engine access is intentionally absent:
///
/// ```compile_fail
/// use mcode_plugin_host::runtime::PluginRuntime;
/// let runtime = PluginRuntime::new();
/// let _: &wasmtime::Engine = runtime.engine();
/// ```
///
/// The wrapper also does not implement `Deref` or `AsRef<Engine>`:
///
/// ```compile_fail
/// use mcode_plugin_host::runtime::PluginRuntime;
/// fn takes_engine(_: &wasmtime::Engine) {}
/// let runtime = PluginRuntime::new();
/// takes_engine(&runtime);
/// ```
///
/// Generic core-module compilation is not part of the public boundary:
///
/// ```compile_fail
/// use mcode_plugin_host::runtime::PluginRuntime;
/// let runtime = PluginRuntime::new();
/// let _ = runtime.compile(b"\0asm\x01\0\0\0");
/// ```
pub struct PluginRuntime {
    inner: Arc<RuntimeInner>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ManagerBatchCompileError {
    index: usize,
    error: RuntimeError,
}

impl ManagerBatchCompileError {
    pub(crate) const fn index(self) -> usize {
        self.index
    }

    fn into_runtime_error(self) -> RuntimeError {
        self.error
    }
}

impl PluginRuntime {
    /// Creates an empty runtime without creating a Wasmtime engine.
    #[must_use]
    pub fn new() -> Self {
        Self::with_optional_feature_deadline_policy(None)
    }

    /// Creates an empty runtime with one immutable FeaturePack deadline
    /// snapshot.
    #[must_use]
    pub fn with_feature_deadline_policy(policy: FeatureDeadlinePolicyV1) -> Self {
        Self::with_optional_feature_deadline_policy(Some(policy))
    }

    fn with_optional_feature_deadline_policy(
        feature_deadlines: Option<FeatureDeadlinePolicyV1>,
    ) -> Self {
        Self {
            inner: Arc::new(RuntimeInner {
                components: ComponentCache::runtime(),
                component_ready: OnceLock::new(),
                epoch_ticker: OnceLock::new(),
                manager_director_claimed: AtomicBool::new(false),
                feature_deadlines,
                #[cfg(test)]
                shutdown_observations: Mutex::new(Vec::new()),
            }),
        }
    }

    /// Compiles one bounded, exact-shape Manager component.
    ///
    /// Scanner validation precedes initialization. Exact-shape validation and
    /// executable compilation then use this runtime's same hidden engine.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError::Preflight`] when `bytes` exceeds `limits`, is
    /// not a binary Component Model artifact, or does not exactly implement the
    /// sole-current Manager world.
    pub fn compile_manager(
        &self,
        bytes: impl AsRef<[u8]>,
        limits: ComponentLimits,
    ) -> Result<CompiledManagerComponent, RuntimeError> {
        let bytes = bytes.as_ref();
        let mut compiled = self
            .compile_manager_batch(&[bytes], limits)
            .map_err(ManagerBatchCompileError::into_runtime_error)?;
        compiled.pop().ok_or(RuntimeError::RuntimeUninitialized)
    }

    /// Compiles one bounded, exact-shape Pack component.
    ///
    /// The selected world must be a FeaturePack or ProviderPack world. Scanner
    /// validation precedes private-engine initialization, and this method never
    /// creates a Store, instantiates the component, or executes guest code.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError::InvalidPackWorld`] for the Manager world, or
    /// [`RuntimeError::Preflight`] when `bytes` exceeds `limits`, is not a
    /// binary Component Model artifact, or does not exactly implement `world`.
    pub(crate) fn compile_pack(
        &self,
        bytes: impl AsRef<[u8]>,
        world: ComponentWorld,
        limits: ComponentLimits,
    ) -> Result<CompiledPackComponent, RuntimeError> {
        if world == ComponentWorld::Manager {
            return Err(RuntimeError::InvalidPackWorld);
        }
        let scanned = scan_bounded_component(bytes.as_ref(), world, limits)?;
        let component = self
            .inner
            .components
            .compile(scanned)
            .map_err(runtime_compile_error)?;
        let compiled = CompiledPackComponent::new(Arc::clone(&self.inner), world, component);
        self.inner.component_ready.get_or_init(|| ());
        Ok(compiled)
    }

    pub(crate) fn compile_manager_batch(
        &self,
        binaries: &[&[u8]],
        limits: ComponentLimits,
    ) -> Result<Vec<CompiledManagerComponent>, ManagerBatchCompileError> {
        let scanned = binaries
            .iter()
            .enumerate()
            .map(|(index, bytes)| {
                scan_bounded_component(bytes, ComponentWorld::Manager, limits).map_err(|error| {
                    ManagerBatchCompileError {
                        index,
                        error: RuntimeError::Preflight(error),
                    }
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let compiled = scanned
            .into_iter()
            .enumerate()
            .map(|(index, scanned)| {
                let component = self
                    .inner
                    .components
                    .compile(scanned)
                    .map_err(runtime_compile_error)
                    .map_err(|error| ManagerBatchCompileError { index, error })?;
                Ok(CompiledManagerComponent::new(
                    Arc::clone(&self.inner),
                    component,
                ))
            })
            .collect::<Result<Vec<_>, _>>()?;
        if !compiled.is_empty() {
            self.inner.component_ready.get_or_init(|| ());
        }
        Ok(compiled)
    }

    #[cfg(test)]
    fn compile_test_module(
        &self,
        wasm: impl AsRef<[u8]>,
    ) -> wasmtime::Result<owner::CompiledTestModule> {
        let engine = self
            .inner
            .engine()
            .map_err(|error| wasmtime::Error::msg(error.to_string()))?;
        let module = Module::new(engine, wasm)?;
        Ok(owner::CompiledTestModule::new(
            Arc::clone(&self.inner),
            module,
        ))
    }

    /// Creates one exclusive, policy-configured Store owner.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError::RuntimeUninitialized`] before a bounded,
    /// exact-shape component initializes the runtime,
    /// [`RuntimeError::EpochTicker`] if the
    /// private ticker thread cannot start, or [`RuntimeError::Fuel`] if the
    /// initial zero-fuel state or cooperative async-yield policy cannot be
    /// installed.
    pub fn new_owner(&self) -> Result<PluginOwner, RuntimeError> {
        PluginOwner::new(Arc::clone(&self.inner))
    }

    pub(crate) fn claim_manager_director(&self) -> bool {
        self.inner
            .manager_director_claimed
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    #[cfg(test)]
    pub(crate) fn shutdown_observations(&self) -> Vec<Result<LifecycleOutcome, RuntimeError>> {
        self.inner
            .shutdown_observations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

impl Default for PluginRuntime {
    fn default() -> Self {
        Self::new()
    }
}

pub(super) struct RuntimeInner {
    components: ComponentCache,
    component_ready: OnceLock<()>,
    epoch_ticker: OnceLock<Result<EpochTicker, RuntimeError>>,
    manager_director_claimed: AtomicBool,
    feature_deadlines: Option<FeatureDeadlinePolicyV1>,
    #[cfg(test)]
    shutdown_observations: Mutex<Vec<Result<LifecycleOutcome, RuntimeError>>>,
}

impl RuntimeInner {
    pub(super) const fn feature_deadline_policy(&self) -> Option<FeatureDeadlinePolicyV1> {
        self.feature_deadlines
    }

    #[cfg(test)]
    fn observe_shutdown(&self, outcome: Result<LifecycleOutcome, RuntimeError>) {
        self.shutdown_observations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(outcome);
    }

    fn engine(&self) -> Result<&Engine, RuntimeError> {
        self.components
            .engine()
            .map_err(runtime_compile_error)?
            .ok_or(RuntimeError::RuntimeUninitialized)
    }

    fn ensure_epoch_ticker(&self) -> Result<(), RuntimeError> {
        let engine = self.engine()?.clone();
        if self.component_ready.get().is_none() {
            return Err(RuntimeError::RuntimeUninitialized);
        }
        match self
            .epoch_ticker
            .get_or_init(move || EpochTicker::start(engine))
        {
            Ok(_) => Ok(()),
            Err(error) => Err(*error),
        }
    }
}

fn runtime_compile_error(error: PreflightError) -> RuntimeError {
    if error == PreflightError::Engine {
        RuntimeError::Engine
    } else {
        RuntimeError::Preflight(error)
    }
}

#[cfg(test)]
mod lifecycle_tests;
#[cfg(test)]
mod pack_tests;
#[cfg(test)]
mod tests;

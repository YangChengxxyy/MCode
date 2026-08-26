//! StoreLimits, fuel, and epoch configuration for untrusted guests.

// Rust guideline compliant 2026-08-26.

use wasmtime::{Config, Engine, StoreLimits, StoreLimitsBuilder};

use crate::error::HostError;

/// Default fuel budget for one guest export.
///
/// Chosen to allow ordinary JSON handling while bounding busy loops. Lowering
/// it will fail legitimate plugins; raising it extends worst-case CPU per call.
pub const DEFAULT_CALL_FUEL: u64 = 50_000_000;

/// Default WASM memory ceiling in bytes (64 MiB).
pub const DEFAULT_MEMORY_SIZE: usize = 64 * 1024 * 1024;

/// Default table element ceiling.
pub const DEFAULT_TABLE_ELEMENTS: usize = 10_000;

/// Default instance ceiling per store.
pub const DEFAULT_INSTANCES: usize = 8;

/// Host-enforced WASM resource limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SandboxLimits {
    /// Fuel consumed per guest export.
    pub call_fuel: u64,
    /// Maximum linear memory bytes.
    pub memory_size: usize,
    /// Maximum table elements.
    pub table_elements: usize,
    /// Maximum instances in the store.
    pub instances: usize,
    /// Maximum tables in the store.
    pub tables: usize,
    /// Maximum memories in the store.
    pub memories: usize,
}

impl SandboxLimits {
    /// Creates validated sandbox limits.
    ///
    /// # Errors
    ///
    /// Returns [`HostError::InvalidLimits`] when any field is zero.
    pub fn new(
        call_fuel: u64,
        memory_size: usize,
        table_elements: usize,
        instances: usize,
        tables: usize,
        memories: usize,
    ) -> Result<Self, HostError> {
        if call_fuel == 0
            || memory_size == 0
            || table_elements == 0
            || instances == 0
            || tables == 0
            || memories == 0
        {
            return Err(HostError::InvalidLimits);
        }
        Ok(Self {
            call_fuel,
            memory_size,
            table_elements,
            instances,
            tables,
            memories,
        })
    }

    /// Builds Wasmtime [`StoreLimits`] from this sandbox policy.
    #[must_use]
    pub fn store_limits(self) -> StoreLimits {
        StoreLimitsBuilder::new()
            .memory_size(self.memory_size)
            .table_elements(self.table_elements)
            .instances(self.instances)
            .tables(self.tables)
            .memories(self.memories)
            .trap_on_grow_failure(true)
            .build()
    }
}

impl Default for SandboxLimits {
    fn default() -> Self {
        Self {
            call_fuel: DEFAULT_CALL_FUEL,
            memory_size: DEFAULT_MEMORY_SIZE,
            table_elements: DEFAULT_TABLE_ELEMENTS,
            instances: DEFAULT_INSTANCES,
            tables: DEFAULT_INSTANCES,
            memories: DEFAULT_INSTANCES,
        }
    }
}

/// Builds an engine config with fuel and epoch interruption and no WASI.
///
/// # Errors
///
/// Returns [`HostError::Engine`] if Wasmtime rejects the configuration.
pub fn engine_config() -> Result<Config, HostError> {
    let mut config = Config::new();
    config.wasm_component_model(true);
    config.consume_fuel(true);
    config.epoch_interruption(true);
    Ok(config)
}

/// Creates a dedicated engine for one plugin generation.
///
/// Epoch interrupts are per-engine, so generations do not share engines.
///
/// # Errors
///
/// Returns [`HostError::Engine`] if the engine cannot be created.
pub fn new_engine() -> Result<Engine, HostError> {
    Engine::new(&engine_config()?).map_err(|error| {
        tracing::event!(
            name: "plugin.engine.failed",
            tracing::Level::ERROR,
            error.type = "wasmtime",
            "plugin wasm engine failed"
        );
        let _ = error;
        HostError::Engine
    })
}

//! Scanner-first binary preflight for all sole-current component worlds.

// Rust guideline compliant 2026-08-30.

use std::sync::OnceLock;

use wasmtime::component::Component;
use wasmtime::{Config, Engine};

use crate::component_scanner::{ScannedComponent, scan_component};
use crate::component_shape::validate_shape;
use crate::component_world::ComponentWorld;
use crate::error::PreflightError;

/// Hard maximum encoded component size (4 MiB).
pub const MAX_COMPONENT_BYTES: usize = 4 * 1024 * 1024;

/// Bounds one component preflight attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ComponentLimits {
    encoded_bytes: usize,
}

impl ComponentLimits {
    /// Creates a nonzero compile bound no larger than the hard maximum.
    ///
    /// # Errors
    ///
    /// Returns [`PreflightError::InvalidLimits`] for zero or a value greater
    /// than [`MAX_COMPONENT_BYTES`].
    pub const fn new(encoded_bytes: usize) -> Result<Self, PreflightError> {
        if encoded_bytes == 0 || encoded_bytes > MAX_COMPONENT_BYTES {
            return Err(PreflightError::InvalidLimits);
        }
        Ok(Self { encoded_bytes })
    }

    /// Returns the maximum accepted encoded bytes.
    #[must_use]
    pub const fn encoded_bytes(self) -> usize {
        self.encoded_bytes
    }
}

impl Default for ComponentLimits {
    fn default() -> Self {
        Self {
            encoded_bytes: MAX_COMPONENT_BYTES,
        }
    }
}

/// Preflights one binary for an explicitly selected current world.
///
/// The scanner validates and bounds every nested core module before a private
/// engine can be created. Private Wasmtime component types are then compared
/// against the build-time canonical WIT projection. This function never
/// creates a store, instantiates a component, or calls a guest function.
///
/// # Errors
///
/// Returns [`PreflightError`] for an invalid bound or binary, a disabled
/// feature, an unbounded core resource, ambient surface, or any selected-world
/// mismatch.
pub fn preflight_component(
    bytes: &[u8],
    world: ComponentWorld,
    limits: ComponentLimits,
) -> Result<(), PreflightError> {
    if bytes.len() > limits.encoded_bytes {
        return Err(PreflightError::ComponentTooLarge);
    }
    if !wasmparser::Parser::is_component(bytes) {
        return Err(PreflightError::InvalidComponent);
    }
    let scanned = scan_component(bytes, world)?;
    compile_scanned_component(scanned)
}

/// Compiles and statically preflights one current Manager component.
///
/// This preserves the Manager-only public entry point while delegating to the
/// closed multi-world preflight. It never creates a store, instantiates the
/// component, or calls a guest function.
///
/// # Errors
///
/// Returns [`PreflightError`] for invalid limits, text or core-module input,
/// invalid core resources, ambient surface, or any current Manager mismatch.
pub fn preflight_manager_component(
    bytes: &[u8],
    limits: ComponentLimits,
) -> Result<(), PreflightError> {
    preflight_component(bytes, ComponentWorld::Manager, limits)
}

// Requiring the scanner-issued token makes Engine creation and compilation
// structurally unreachable before the complete scanner succeeds.
fn compile_scanned_component(scanned: ScannedComponent<'_>) -> Result<(), PreflightError> {
    let (bytes, world) = scanned.into_parts();
    let trusted = trusted_components()?;
    let component = Component::from_binary(&trusted.engine, bytes)
        .map_err(|_| PreflightError::InvalidComponent)?;
    validate_shape(&component, trusted.reference(world), world)
}

struct TrustedComponents {
    engine: Engine,
    references: [Component; ComponentWorld::ALL.len()],
}

impl TrustedComponents {
    fn build() -> Result<Self, PreflightError> {
        let engine = private_engine()?;
        let references = ComponentWorld::ALL
            .into_iter()
            .map(|world| {
                Component::from_binary(&engine, world.reference_bytes())
                    .map_err(|_| PreflightError::Engine)
            })
            .collect::<Result<Vec<_>, _>>()?
            .try_into()
            .map_err(|_| PreflightError::Engine)?;
        Ok(Self { engine, references })
    }

    fn reference(&self, world: ComponentWorld) -> &Component {
        &self.references[world.index()]
    }
}

fn trusted_components() -> Result<&'static TrustedComponents, PreflightError> {
    static TRUSTED: OnceLock<Result<TrustedComponents, PreflightError>> = OnceLock::new();
    match TRUSTED.get_or_init(TrustedComponents::build) {
        Ok(trusted) => Ok(trusted),
        Err(error) => Err(*error),
    }
}

fn private_engine() -> Result<Engine, PreflightError> {
    let mut config = Config::new();
    config.wasm_component_model(true);
    config.wasm_threads(false);
    config.wasm_memory64(false);
    config.wasm_shared_everything_threads(false);
    config.wasm_component_model_memory64(false);
    Engine::new(&config).map_err(|_| PreflightError::Engine)
}

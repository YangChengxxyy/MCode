//! Bounded Manager component compilation and exact static shape preflight.

// Rust guideline compliant 2026-08-29.

use mcode_plugin_api::{FEATURE_SERVICE_INTERFACE_ID, MANAGER_LIFECYCLE_INTERFACE_ID};
use wasmtime::component::types::{ComponentFunc, ComponentInstance, ComponentItem, Type};
use wasmtime::component::{Component, HasSelf, Linker};
use wasmtime::{Config, Engine};

use crate::error::{ImportCategory, PreflightError};
use crate::wit::Manager;
use crate::wit::mcode::plugin::feature_service::Host as GatewayHost;

// Component Model binaries use encoding version 0x0d and layer 0x01.
const WASM_COMPONENT_BINARY_HEADER: [u8; 8] = *b"\0asm\x0d\0\x01\0";

/// Hard maximum encoded Manager component size (4 MiB).
pub const MAX_MANAGER_COMPONENT_BYTES: usize = 4 * 1024 * 1024;

/// Bounds one Manager component compile attempt.
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
    /// than [`MAX_MANAGER_COMPONENT_BYTES`].
    pub const fn new(encoded_bytes: usize) -> Result<Self, PreflightError> {
        if encoded_bytes == 0 || encoded_bytes > MAX_MANAGER_COMPONENT_BYTES {
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
            encoded_bytes: MAX_MANAGER_COMPONENT_BYTES,
        }
    }
}

/// Compiles and statically preflights one current Manager component.
///
/// This function validates the encoded-byte bound and exact Component Model
/// binary header before Wasmtime compilation, then validates exact top-level
/// import and export names, generated FeatureService import bindings, and the
/// typed lifecycle export. It never creates a store, instantiates the
/// component, or calls a guest function.
///
/// # Errors
///
/// Returns [`PreflightError`] for invalid limits, text or core-module input,
/// compile failure, any ambient or extra surface, or any current-world shape
/// mismatch.
pub fn preflight_manager_component(
    bytes: &[u8],
    limits: ComponentLimits,
) -> Result<(), PreflightError> {
    if bytes.len() > limits.encoded_bytes {
        return Err(PreflightError::ComponentTooLarge);
    }
    if !bytes.starts_with(&WASM_COMPONENT_BINARY_HEADER) {
        return Err(PreflightError::InvalidComponent);
    }

    let mut config = Config::new();
    config.wasm_component_model(true);
    let engine = Engine::new(&config).map_err(|_| PreflightError::Engine)?;
    let component =
        Component::from_binary(&engine, bytes).map_err(|_| PreflightError::InvalidComponent)?;
    validate_top_level_names(&component)?;
    validate_import_members(&component)?;
    validate_generated_import_shape(&component)?;
    validate_export_members(&component)?;

    Ok(())
}

fn validate_top_level_names(component: &Component) -> Result<(), PreflightError> {
    let component_type = component.component_type();
    let imports = component_type.imports(component.engine());
    let import_count = imports.len();
    if let Some(name) = imports
        .map(|(name, _)| name)
        .find(|name| *name != FEATURE_SERVICE_INTERFACE_ID)
    {
        return Err(PreflightError::DeniedImport(classify_import(name)));
    }
    if import_count == 0 {
        return Err(PreflightError::MissingImport);
    }
    if import_count != 1 {
        return Err(PreflightError::DeniedImport(ImportCategory::Extra));
    }

    let exports = component_type.exports(component.engine());
    let export_count = exports.len();
    if exports
        .map(|(name, _)| name)
        .any(|name| name != MANAGER_LIFECYCLE_INTERFACE_ID)
    {
        return Err(PreflightError::UnexpectedExport);
    }
    if export_count == 0 {
        return Err(PreflightError::MissingExport);
    }
    if export_count != 1 {
        return Err(PreflightError::UnexpectedExport);
    }
    Ok(())
}

fn validate_import_members(component: &Component) -> Result<(), PreflightError> {
    const FEATURE_MEMBERS: [&str; 3] = ["start-task", "poll-task", "cancel-task"];

    let component_type = component.component_type();
    let feature = component_type
        .get_import(component.engine(), FEATURE_SERVICE_INTERFACE_ID)
        .ok_or(PreflightError::MissingImport)?;
    let ComponentItem::ComponentInstance(feature) = feature.ty else {
        return Err(PreflightError::ImportShape);
    };
    if !has_exact_members(
        feature.exports(component.engine()).map(|(name, _)| name),
        &FEATURE_MEMBERS,
    ) {
        return Err(PreflightError::ImportShape);
    }
    for name in FEATURE_MEMBERS {
        let item = feature
            .get_export(component.engine(), name)
            .ok_or(PreflightError::ImportShape)?;
        let ComponentItem::ComponentFunc(function) = item.ty else {
            return Err(PreflightError::ImportShape);
        };
        if !is_feature_service_function(&function) {
            return Err(PreflightError::ImportShape);
        }
    }
    Ok(())
}

fn is_feature_service_function(function: &ComponentFunc) -> bool {
    let mut params = function.params();
    let mut results = function.results();
    !function.async_()
        && params.len() == 1
        && params
            .next()
            .is_some_and(|(name, value)| name == "request" && value == Type::String)
        && results.len() == 1
        && results.next() == Some(Type::String)
}

fn validate_export_members(component: &Component) -> Result<(), PreflightError> {
    const LIFECYCLE_MEMBERS: [&str; 6] = [
        "initialization-context",
        "state",
        "error-code",
        "initialize",
        "poll",
        "shutdown",
    ];
    const STATE_CASES: [&str; 4] = ["ready", "pending", "stopping", "stopped"];
    const ERROR_CASES: [&str; 3] = ["invalid-state", "feature-unavailable", "failed"];

    let component_type = component.component_type();
    let lifecycle = component_type
        .get_export(component.engine(), MANAGER_LIFECYCLE_INTERFACE_ID)
        .ok_or(PreflightError::MissingExport)?;
    let ComponentItem::ComponentInstance(lifecycle) = lifecycle.ty else {
        return Err(PreflightError::ExportShape);
    };
    if !has_exact_members(
        lifecycle.exports(component.engine()).map(|(name, _)| name),
        &LIFECYCLE_MEMBERS,
    ) {
        return Err(PreflightError::ExportShape);
    }

    // Pre-instantiated bindings defer exported function type checks until a
    // Store exists, so static preflight must inspect the nested types directly.
    let context = exported_type(&lifecycle, component.engine(), "initialization-context")?;
    let state = exported_type(&lifecycle, component.engine(), "state")?;
    let error = exported_type(&lifecycle, component.engine(), "error-code")?;
    let initialize = exported_function(&lifecycle, component.engine(), "initialize")?;
    let poll = exported_function(&lifecycle, component.engine(), "poll")?;
    let shutdown = exported_function(&lifecycle, component.engine(), "shutdown")?;

    if !is_initialization_context(&context)
        || !is_exact_enum(&state, &STATE_CASES)
        || !is_exact_enum(&error, &ERROR_CASES)
        || !is_initialize_function(&initialize, &context, &state, &error)
        || !is_lifecycle_function(&poll, &state, &error)
        || !is_lifecycle_function(&shutdown, &state, &error)
    {
        return Err(PreflightError::ExportShape);
    }
    Ok(())
}

fn exported_type(
    lifecycle: &ComponentInstance,
    engine: &Engine,
    name: &str,
) -> Result<Type, PreflightError> {
    let item = lifecycle
        .get_export(engine, name)
        .ok_or(PreflightError::ExportShape)?;
    match item.ty {
        ComponentItem::Type(value) => Ok(value),
        _ => Err(PreflightError::ExportShape),
    }
}

fn exported_function(
    lifecycle: &ComponentInstance,
    engine: &Engine,
    name: &str,
) -> Result<ComponentFunc, PreflightError> {
    let item = lifecycle
        .get_export(engine, name)
        .ok_or(PreflightError::ExportShape)?;
    match item.ty {
        ComponentItem::ComponentFunc(function) => Ok(function),
        _ => Err(PreflightError::ExportShape),
    }
}

fn is_initialization_context(context: &Type) -> bool {
    let Type::Record(record) = context else {
        return false;
    };
    let mut fields = record.fields();
    fields.len() == 1
        && fields
            .next()
            .is_some_and(|field| field.name == "generation" && field.ty == Type::U64)
}

fn is_exact_enum(value: &Type, expected: &[&str]) -> bool {
    let Type::Enum(value) = value else {
        return false;
    };
    value.names().eq(expected.iter().copied())
}

fn is_initialize_function(
    function: &ComponentFunc,
    context: &Type,
    state: &Type,
    error: &Type,
) -> bool {
    let mut params = function.params();
    !function.async_()
        && params.len() == 1
        && params
            .next()
            .is_some_and(|(name, value)| name == "context" && &value == context)
        && returns_lifecycle_result(function, state, error)
}

fn is_lifecycle_function(function: &ComponentFunc, state: &Type, error: &Type) -> bool {
    !function.async_()
        && function.params().len() == 0
        && returns_lifecycle_result(function, state, error)
}

fn returns_lifecycle_result(function: &ComponentFunc, state: &Type, error: &Type) -> bool {
    let mut results = function.results();
    if results.len() != 1 {
        return false;
    }
    let Some(Type::Result(result)) = results.next() else {
        return false;
    };
    result.ok().as_ref() == Some(state) && result.err().as_ref() == Some(error)
}

fn has_exact_members<'a>(
    mut members: impl ExactSizeIterator<Item = &'a str>,
    expected: &[&str],
) -> bool {
    members.len() == expected.len() && members.all(|name| expected.contains(&name))
}

fn validate_generated_import_shape(component: &Component) -> Result<(), PreflightError> {
    let mut linker = Linker::<ShapeImports>::new(component.engine());
    Manager::add_to_linker::<_, HasSelf<_>>(&mut linker, |imports| imports)
        .map_err(|_| PreflightError::ImportShape)?;
    linker
        .instantiate_pre(component)
        .map_err(|_| PreflightError::ImportShape)?;
    Ok(())
}

fn classify_import(name: &str) -> ImportCategory {
    let name = name.to_ascii_lowercase();
    if name.starts_with("mcode:plugin/host@0.1") || name.contains("mcode:plugin@0.1") {
        return ImportCategory::Legacy;
    }
    if name.contains("terminal") || name.contains("stdin") || name.contains("stdout") {
        return ImportCategory::Terminal;
    }
    if name.contains("filesystem") || name.contains("file-system") {
        return ImportCategory::Filesystem;
    }
    if name.contains("http") {
        return ImportCategory::Http;
    }
    if name.contains("socket") || name.contains("network") || name.contains("dns") {
        return ImportCategory::Network;
    }
    if name.contains("random") {
        return ImportCategory::Random;
    }
    if name.contains("clock") {
        return ImportCategory::Clocks;
    }
    if name.contains("secret")
        || name.contains("credential")
        || name.contains("keyvalue")
        || name.contains("key-value")
    {
        return ImportCategory::Secret;
    }
    if name.contains("logging") || name.contains("/log") {
        return ImportCategory::Logging;
    }
    if name.contains("/ui") || name.contains("render") || name.contains("view") {
        return ImportCategory::UserInterface;
    }
    if name.contains("raw") || name.contains("handle") {
        return ImportCategory::RawHost;
    }
    if name.contains("process") || name.contains("environment") || name.contains(":cli/") {
        return ImportCategory::Process;
    }
    if name.starts_with("wasi:")
        || name.starts_with("wasi_")
        || name.starts_with("wasi-")
        || name == "env"
    {
        return ImportCategory::Wasi;
    }
    ImportCategory::Extra
}

struct ShapeImports;

impl GatewayHost for ShapeImports {
    fn start_task(&mut self, _request: String) -> String {
        unreachable!("component preflight never calls Host imports")
    }

    fn poll_task(&mut self, _request: String) -> String {
        unreachable!("component preflight never calls Host imports")
    }

    fn cancel_task(&mut self, _request: String) -> String {
        unreachable!("component preflight never calls Host imports")
    }
}

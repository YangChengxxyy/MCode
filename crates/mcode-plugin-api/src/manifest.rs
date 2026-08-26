//! Strict, versioned `plugin.json` manifest parsing.
//!
//! Unknown fields are rejected at every typed boundary. `configSchema` and
//! descriptor JSON schemas remain intentionally open JSON Schema objects.

// Rust guideline compliant 2026-08-26.

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use semver::Version;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::capability::{CapabilityDeclaration, CapabilityKind, validate_capabilities};
use crate::contribution::Contributions;
use crate::guest::{HOST_INTERFACE_ID, WIT_WORLD_ID};
use crate::ids::PluginId;
use crate::limits::{MAX_DESCRIPTOR_JSON_BYTES, MAX_MANIFEST_BYTES};
use crate::path::resolve_contained_path;
use crate::state::StateDeclarations;
use crate::validation::{parse_strict_json, valid_public_text, validate_json_value};

/// Manifest schema version supported by this SDK.
pub const MANIFEST_VERSION: u32 = 1;

/// Public plugin SDK protocol version supported by this crate.
pub const SDK_VERSION: &str = "1.0.0";

/// Canonical JSON Schema identifier for `plugin.json`.
pub const PLUGIN_MANIFEST_SCHEMA_ID: &str =
    "https://mcode.dev/schemas/plugin/v1/plugin.schema.json";

/// Canonical JSON Schema for `plugin.json`.
pub const PLUGIN_MANIFEST_SCHEMA_JSON: &str = include_str!("../schema/plugin.schema.json");

/// Unknown-field behavior for the current manifest schema.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnknownFieldPolicy {
    /// Reject unknown fields rather than guessing their semantics.
    Reject,
}

/// Fully validated `plugin.json` manifest.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginManifest {
    #[serde(rename = "$schema", skip_serializing_if = "Option::is_none")]
    schema: Option<String>,
    manifest_version: u32,
    id: PluginId,
    name: String,
    version: String,
    sdk_version: String,
    wit_world: String,
    component: String,
    imports: Vec<String>,
    capabilities: Vec<CapabilityDeclaration>,
    contributions: Contributions,
    #[serde(skip_serializing_if = "Option::is_none")]
    config_schema: Option<Value>,
    state: StateDeclarations,
    #[serde(skip)]
    plugin_root: PathBuf,
    #[serde(skip)]
    resolved_component: PathBuf,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RawManifest {
    #[serde(rename = "$schema", default)]
    schema: Option<String>,
    manifest_version: u32,
    id: String,
    name: String,
    version: String,
    sdk_version: String,
    wit_world: String,
    component: String,
    #[serde(default)]
    imports: Vec<String>,
    #[serde(default)]
    capabilities: Vec<CapabilityDeclaration>,
    #[serde(default)]
    contributions: Contributions,
    #[serde(default)]
    config_schema: Option<Value>,
    #[serde(default)]
    state: StateDeclarations,
}

impl PluginManifest {
    /// Parses and validates a bounded `plugin.json` byte slice.
    ///
    /// `plugin_root` must exist so symlink containment can be checked.
    ///
    /// # Errors
    ///
    /// Returns [`ManifestError`] for size, syntax, unknown-field, version,
    /// identifier, path, capability, contribution, config-schema, or state
    /// declaration failures.
    pub fn parse_json(bytes: &[u8], plugin_root: impl AsRef<Path>) -> Result<Self, ManifestError> {
        if bytes.len() > MAX_MANIFEST_BYTES {
            return Err(ManifestError::TooLarge {
                actual: bytes.len(),
                maximum: MAX_MANIFEST_BYTES,
            });
        }
        let value: Value =
            parse_strict_json(bytes).map_err(|error| ManifestError::InvalidJson {
                message: safe_json_error(&error),
            })?;
        inspect_capability_kinds(&value)?;
        inspect_forbidden_hooks(&value)?;
        let raw: RawManifest = serde_json::from_value(value).map_err(classify_decode_error)?;
        Self::validate_raw(raw, plugin_root.as_ref())
    }

    /// Reads `<plugin_root>/plugin.json` with a hard byte cap.
    ///
    /// # Errors
    ///
    /// Returns [`ManifestError`] when the file cannot be read or fails strict
    /// parsing and validation.
    pub fn from_plugin_root(plugin_root: impl AsRef<Path>) -> Result<Self, ManifestError> {
        let plugin_root = plugin_root.as_ref();
        let path = plugin_root.join("plugin.json");
        let file = File::open(&path).map_err(|_| ManifestError::ReadFailed)?;
        let mut bytes = Vec::new();
        file.take((MAX_MANIFEST_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|_| ManifestError::ReadFailed)?;
        Self::parse_json(&bytes, plugin_root)
    }

    fn validate_raw(raw: RawManifest, plugin_root: &Path) -> Result<Self, ManifestError> {
        if raw
            .schema
            .as_deref()
            .is_some_and(|schema| schema != PLUGIN_MANIFEST_SCHEMA_ID)
        {
            return Err(ManifestError::InvalidSchemaReference);
        }
        if raw.manifest_version != MANIFEST_VERSION {
            return Err(ManifestError::UnsupportedManifestVersion {
                found: raw.manifest_version,
                supported: MANIFEST_VERSION,
            });
        }
        let id = PluginId::parse(raw.id).map_err(|_| ManifestError::InvalidId)?;
        if !valid_public_text(&raw.name, 128) {
            return Err(ManifestError::InvalidName);
        }
        let version = Version::parse(&raw.version).map_err(|_| ManifestError::InvalidVersion)?;
        let requested_sdk =
            Version::parse(&raw.sdk_version).map_err(|_| ManifestError::InvalidSdkVersion)?;
        let supported_sdk =
            Version::parse(SDK_VERSION).map_err(|_| ManifestError::InvalidSdkVersion)?;
        if requested_sdk.major != supported_sdk.major || requested_sdk > supported_sdk {
            return Err(ManifestError::UnsupportedSdkVersion);
        }
        if raw.wit_world != WIT_WORLD_ID {
            return Err(ManifestError::UnsupportedWitWorld);
        }
        validate_imports(&raw.imports)?;
        let resolved_component = resolve_contained_path(plugin_root, &raw.component)
            .map_err(|_| ManifestError::UnsafePath { field: "component" })?;
        reject_non_wasm_component(&raw.component)?;
        validate_capabilities(&raw.capabilities).map_err(|_| ManifestError::InvalidCapabilities)?;
        raw.state
            .validate()
            .map_err(|_| ManifestError::InvalidStateDeclarations)?;
        validate_state_capabilities(&raw.capabilities, &raw.state)?;
        raw.contributions
            .validate(plugin_root, &raw.capabilities)
            .map_err(|_| ManifestError::InvalidContributions)?;
        if let Some(schema) = &raw.config_schema
            && (!schema.is_object()
                || validate_json_value(schema, MAX_DESCRIPTOR_JSON_BYTES).is_err())
        {
            return Err(ManifestError::InvalidConfigSchema);
        }
        let plugin_root =
            std::fs::canonicalize(plugin_root).map_err(|_| ManifestError::UnsafePath {
                field: "pluginRoot",
            })?;
        Ok(Self {
            schema: raw.schema,
            manifest_version: raw.manifest_version,
            id,
            name: raw.name,
            version: version.to_string(),
            sdk_version: requested_sdk.to_string(),
            wit_world: raw.wit_world,
            component: raw.component,
            imports: raw.imports,
            capabilities: raw.capabilities,
            contributions: raw.contributions,
            config_schema: raw.config_schema,
            state: raw.state,
            plugin_root,
            resolved_component,
        })
    }

    /// Returns the optional canonical JSON Schema reference.
    #[must_use]
    pub fn schema(&self) -> Option<&str> {
        self.schema.as_deref()
    }

    /// Returns the strict unknown-field policy for this schema version.
    #[must_use]
    pub fn unknown_field_policy(&self) -> UnknownFieldPolicy {
        UnknownFieldPolicy::Reject
    }

    /// Returns the manifest schema version.
    #[must_use]
    pub fn manifest_version(&self) -> u32 {
        self.manifest_version
    }

    /// Returns the stable plugin id.
    #[must_use]
    pub fn id(&self) -> &PluginId {
        &self.id
    }

    /// Returns the user-facing plugin name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the normalized plugin semantic version.
    #[must_use]
    pub fn version(&self) -> &str {
        &self.version
    }

    /// Returns the normalized SDK protocol version.
    #[must_use]
    pub fn sdk_version(&self) -> &str {
        &self.sdk_version
    }

    /// Returns the declared WIT world id.
    #[must_use]
    pub fn wit_world(&self) -> &str {
        &self.wit_world
    }

    /// Returns declared component import names.
    #[must_use]
    pub fn imports(&self) -> &[String] {
        &self.imports
    }

    /// Returns the portable WASM component path.
    #[must_use]
    pub fn component(&self) -> &str {
        &self.component
    }

    /// Returns the safely resolved WASM component path.
    #[must_use]
    pub fn resolved_component(&self) -> &Path {
        &self.resolved_component
    }

    /// Returns the canonical plugin root.
    #[must_use]
    pub fn plugin_root(&self) -> &Path {
        &self.plugin_root
    }

    /// Returns least-privilege capability declarations.
    #[must_use]
    pub fn capabilities(&self) -> &[CapabilityDeclaration] {
        &self.capabilities
    }

    /// Returns typed contribution descriptors.
    #[must_use]
    pub fn contributions(&self) -> &Contributions {
        &self.contributions
    }

    /// Returns the plugin configuration JSON schema, when declared.
    #[must_use]
    pub fn config_schema(&self) -> Option<&Value> {
        self.config_schema.as_ref()
    }

    /// Returns portable and secret state declarations.
    #[must_use]
    pub fn state(&self) -> &StateDeclarations {
        &self.state
    }

    /// Serializes the validated manifest for a safe runtime transport.
    ///
    /// Host-only resolved paths are omitted.
    ///
    /// # Errors
    ///
    /// Returns [`ManifestError`] if serialization unexpectedly fails.
    pub fn to_wire_value(&self) -> Result<Value, ManifestError> {
        serde_json::to_value(self).map_err(|_| ManifestError::SerializationFailed)
    }
}

fn validate_imports(imports: &[String]) -> Result<(), ManifestError> {
    let mut seen = std::collections::BTreeSet::new();
    for import in imports {
        if import != HOST_INTERFACE_ID {
            return Err(ManifestError::ForbiddenImport);
        }
        if !seen.insert(import) {
            return Err(ManifestError::ForbiddenImport);
        }
    }
    Ok(())
}

fn validate_state_capabilities(
    capabilities: &[CapabilityDeclaration],
    state: &StateDeclarations,
) -> Result<(), ManifestError> {
    let has_session_state = capabilities
        .iter()
        .any(|capability| capability.kind() == CapabilityKind::SessionState);
    if !state.portable.is_empty() && !has_session_state {
        return Err(ManifestError::InvalidStateDeclarations);
    }
    let declared_secrets: std::collections::BTreeSet<_> =
        state.secret.iter().map(|secret| &secret.name).collect();
    for capability in capabilities {
        if let CapabilityDeclaration::Secrets { names } = capability
            && names.iter().any(|name| !declared_secrets.contains(name))
        {
            return Err(ManifestError::InvalidStateDeclarations);
        }
    }
    Ok(())
}

fn inspect_capability_kinds(value: &Value) -> Result<(), ManifestError> {
    let Some(capabilities) = value.get("capabilities").and_then(Value::as_array) else {
        return Ok(());
    };
    const KNOWN: &[&str] = &[
        "filesystem",
        "network",
        "secrets",
        "sessionState",
        "ui",
        "promptContribution",
    ];
    for capability in capabilities {
        if capability
            .get("kind")
            .and_then(Value::as_str)
            .is_some_and(|kind| !KNOWN.contains(&kind))
        {
            return Err(ManifestError::UnknownCapability);
        }
    }
    Ok(())
}

fn inspect_forbidden_hooks(value: &Value) -> Result<(), ManifestError> {
    let Some(contributions) = value.get("contributions") else {
        return Ok(());
    };
    if contributions.get("compactionHooks").is_some()
        || contributions.get("transcriptHooks").is_some()
        || contributions.get("ansi").is_some()
    {
        return Err(ManifestError::ForbiddenHook);
    }
    Ok(())
}

fn reject_non_wasm_component(component: &str) -> Result<(), ManifestError> {
    let extension = Path::new(component)
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default();
    // Native library detection is case-insensitive so `plugin.DLL` cannot slip
    // through as a generic invalid entry. The WASM suffix matches the schema
    // and must be lowercase `.wasm`.
    let lower = extension.to_ascii_lowercase();
    if matches!(lower.as_str(), "dll" | "so" | "dylib" | "rlib") {
        return Err(ManifestError::NativeDynamicLibraryForbidden);
    }
    if extension != "wasm" {
        return Err(ManifestError::InvalidWasmEntry);
    }
    Ok(())
}

fn classify_decode_error(error: serde_json::Error) -> ManifestError {
    let is_unknown_field = error.to_string().contains("unknown field");
    let message = safe_json_error(&error);
    if is_unknown_field {
        ManifestError::UnknownField { message }
    } else {
        ManifestError::InvalidJson { message }
    }
}

fn safe_json_error(error: &serde_json::Error) -> String {
    format!(
        "JSON error at line {}, column {}",
        error.line(),
        error.column()
    )
}

/// Strict manifest parsing or validation failure.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ManifestError {
    /// Manifest exceeded its hard byte cap.
    #[error("plugin manifest is {actual} bytes; maximum is {maximum}")]
    TooLarge {
        /// Observed bytes.
        actual: usize,
        /// Maximum accepted bytes.
        maximum: usize,
    },
    /// Manifest file could not be read.
    #[error("plugin.json could not be read")]
    ReadFailed,
    /// JSON syntax or a typed value was invalid.
    #[error("plugin manifest is invalid: {message}")]
    InvalidJson {
        /// Location-only diagnostic that never includes source JSON.
        message: String,
    },
    /// A typed manifest object contained an unknown field.
    #[error("plugin manifest contains an unknown field: {message}")]
    UnknownField {
        /// Location-only diagnostic that never includes source JSON.
        message: String,
    },
    /// Capability kind is not in this SDK version.
    #[error("plugin manifest declares an unknown capability")]
    UnknownCapability,
    /// Optional `$schema` did not equal the canonical schema id.
    #[error("plugin manifest $schema reference is invalid")]
    InvalidSchemaReference,
    /// Manifest schema version is unsupported.
    #[error("plugin manifest version {found} is unsupported; expected {supported}")]
    UnsupportedManifestVersion {
        /// Manifest value.
        found: u32,
        /// Supported value.
        supported: u32,
    },
    /// Plugin id was malformed.
    #[error("plugin manifest id is invalid")]
    InvalidId,
    /// User-facing name was malformed.
    #[error("plugin manifest name is invalid")]
    InvalidName,
    /// Plugin version was not semantic versioning.
    #[error("plugin manifest version is invalid")]
    InvalidVersion,
    /// SDK version was not semantic versioning.
    #[error("plugin manifest sdkVersion is invalid")]
    InvalidSdkVersion,
    /// SDK version is newer or from another major protocol.
    #[error("plugin manifest sdkVersion is unsupported")]
    UnsupportedSdkVersion,
    /// WIT world id is not the current host world.
    #[error("plugin manifest witWorld is unsupported")]
    UnsupportedWitWorld,
    /// Manifest listed an import outside the WIT world.
    #[error("plugin manifest import is not part of the WIT world")]
    ForbiddenImport,
    /// A path was unsafe or the plugin root was unavailable.
    #[error("plugin manifest path in {field} is unsafe")]
    UnsafePath {
        /// Field whose path failed validation.
        field: &'static str,
    },
    /// Native Rust dynamic libraries are outside the stable SDK boundary.
    #[error("native Rust dynamic libraries are forbidden by the plugin SDK")]
    NativeDynamicLibraryForbidden,
    /// Component path was not a `.wasm` WebAssembly component.
    #[error("plugin component must be a .wasm WebAssembly component")]
    InvalidWasmEntry,
    /// Capability declarations were invalid.
    #[error("plugin manifest capability declarations are invalid")]
    InvalidCapabilities,
    /// Contribution descriptors were invalid.
    #[error("plugin manifest contributions are invalid")]
    InvalidContributions,
    /// Configuration schema was not a bounded JSON object.
    #[error("plugin manifest configSchema is invalid")]
    InvalidConfigSchema,
    /// Portable or secret state declarations were invalid.
    #[error("plugin manifest state declarations are invalid")]
    InvalidStateDeclarations,
    /// Compaction, transcript, or ANSI hooks are forbidden.
    #[error("plugin manifest declares a forbidden hook")]
    ForbiddenHook,
    /// A validated manifest could not be serialized.
    #[error("validated plugin manifest could not be serialized")]
    SerializationFailed,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{ManifestError, PLUGIN_MANIFEST_SCHEMA_JSON, PluginManifest};
    use crate::limits::MAX_MANIFEST_BYTES;

    fn base_manifest() -> serde_json::Value {
        json!({
            "manifestVersion": 1,
            "id": "com.mcode.example",
            "name": "Example",
            "version": "1.2.3",
            "sdkVersion": "1.0.0",
            "witWorld": "mcode:plugin/plugin@0.1.0",
            "component": "plugin.wasm",
            "imports": ["mcode:plugin/host@0.1.0"],
            "capabilities": [],
            "contributions": {},
            "state": {"portable": [], "secret": []}
        })
    }

    #[test]
    fn parses_minimal_strict_manifest() {
        let root = tempfile::tempdir().expect("tempdir");
        let mut value = base_manifest();
        value["$schema"] = json!(super::PLUGIN_MANIFEST_SCHEMA_ID);
        let bytes = serde_json::to_vec(&value).expect("json");
        let manifest = PluginManifest::parse_json(&bytes, root.path()).expect("manifest");
        assert_eq!(manifest.component(), "plugin.wasm");
        assert_eq!(manifest.schema(), Some(super::PLUGIN_MANIFEST_SCHEMA_ID));
        assert_eq!(manifest.id().as_str(), "com.mcode.example");
        assert_eq!(manifest.wit_world(), crate::WIT_WORLD_ID);
    }

    #[test]
    fn rejects_unknown_fields_capabilities_and_malicious_paths() {
        let root = tempfile::tempdir().expect("tempdir");

        let mut unknown = base_manifest();
        unknown["surprise"] = json!(true);
        assert!(matches!(
            PluginManifest::parse_json(&serde_json::to_vec(&unknown).expect("json"), root.path()),
            Err(ManifestError::UnknownField { .. })
        ));

        let mut capability = base_manifest();
        capability["capabilities"] = json!([{"kind": "rootShell"}]);
        assert_eq!(
            PluginManifest::parse_json(
                &serde_json::to_vec(&capability).expect("json"),
                root.path()
            ),
            Err(ManifestError::UnknownCapability)
        );

        let mut traversal = base_manifest();
        traversal["component"] = json!("../outside.wasm");
        assert!(matches!(
            PluginManifest::parse_json(&serde_json::to_vec(&traversal).expect("json"), root.path()),
            Err(ManifestError::UnsafePath { .. })
        ));
    }

    #[test]
    fn schema_is_json_and_manifest_size_is_bounded_before_decode() {
        let schema: serde_json::Value =
            serde_json::from_str(PLUGIN_MANIFEST_SCHEMA_JSON).expect("schema JSON");
        assert_eq!(schema["additionalProperties"], json!(false));
        assert!(schema["properties"].get("runtime").is_none());
        assert!(schema["properties"].get("entry").is_none());
        assert!(schema["properties"].get("component").is_some());

        let root = tempfile::tempdir().expect("tempdir");
        let oversized = vec![b' '; MAX_MANIFEST_BYTES + 1];
        assert!(matches!(
            PluginManifest::parse_json(&oversized, root.path()),
            Err(ManifestError::TooLarge { .. })
        ));
    }

    #[test]
    fn rejects_duplicate_json_object_keys() {
        let root = tempfile::tempdir().expect("tempdir");
        let bytes = br#"{
            "manifestVersion": 1,
            "id": "com.mcode.one",
            "id": "com.mcode.two",
            "name": "Duplicate",
            "version": "1.0.0",
            "sdkVersion": "1.0.0",
            "witWorld": "mcode:plugin/plugin@0.1.0",
            "component": "plugin.wasm"
        }"#;
        assert!(matches!(
            PluginManifest::parse_json(bytes, root.path()),
            Err(ManifestError::InvalidJson { .. })
        ));
    }

    #[test]
    fn strict_schema_rejects_runtime_and_non_wasm_component_fields() {
        let root = tempfile::tempdir().expect("tempdir");

        for runtime in ["wasm", "firstParty", "external", "nativeDylib", "mcp"] {
            let mut value = base_manifest();
            value["runtime"] = json!(runtime);
            assert!(
                matches!(
                    PluginManifest::parse_json(
                        &serde_json::to_vec(&value).expect("json"),
                        root.path()
                    ),
                    Err(ManifestError::UnknownField { .. })
                ),
                "runtime {runtime} must be rejected"
            );
        }

        let mut legacy_entry = base_manifest();
        legacy_entry["entry"] = json!("plugin.wasm");
        assert!(matches!(
            PluginManifest::parse_json(
                &serde_json::to_vec(&legacy_entry).expect("json"),
                root.path()
            ),
            Err(ManifestError::UnknownField { .. })
        ));

        let mut native = base_manifest();
        native["component"] = json!("plugin.dll");
        assert_eq!(
            PluginManifest::parse_json(&serde_json::to_vec(&native).expect("json"), root.path()),
            Err(ManifestError::NativeDynamicLibraryForbidden)
        );

        let mut script = base_manifest();
        script["component"] = json!("plugin.js");
        assert_eq!(
            PluginManifest::parse_json(&serde_json::to_vec(&script).expect("json"), root.path()),
            Err(ManifestError::InvalidWasmEntry)
        );

        let mut upper_wasm = base_manifest();
        upper_wasm["component"] = json!("plugin.WASM");
        assert_eq!(
            PluginManifest::parse_json(
                &serde_json::to_vec(&upper_wasm).expect("json"),
                root.path()
            ),
            Err(ManifestError::InvalidWasmEntry)
        );
    }

    #[test]
    fn rejects_wasi_imports_and_compaction_hooks() {
        let root = tempfile::tempdir().expect("tempdir");
        let mut imports = base_manifest();
        imports["imports"] = json!(["wasi:filesystem/types@0.2.0"]);
        assert_eq!(
            PluginManifest::parse_json(&serde_json::to_vec(&imports).expect("json"), root.path()),
            Err(ManifestError::ForbiddenImport)
        );

        let mut hooks = base_manifest();
        hooks["contributions"] = json!({"compactionHooks": []});
        assert_eq!(
            PluginManifest::parse_json(&serde_json::to_vec(&hooks).expect("json"), root.path()),
            Err(ManifestError::ForbiddenHook)
        );
    }
}

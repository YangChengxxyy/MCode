//! Shared resource limits for plugin-controlled data.

// Rust guideline compliant 2026-08-26.

/// Maximum accepted `plugin.json` size in bytes.
pub const MAX_MANIFEST_BYTES: usize = 256 * 1024;

/// Maximum accepted host binding JSON size in bytes.
pub const MAX_HOST_BINDINGS_BYTES: usize = 64 * 1024;

/// Maximum size of one JSON schema or descriptor-owned JSON value.
pub const MAX_DESCRIPTOR_JSON_BYTES: usize = 64 * 1024;

/// Maximum nesting depth for plugin-controlled JSON values.
pub const MAX_JSON_DEPTH: usize = 32;

/// Maximum nodes traversed in one plugin-controlled JSON value.
pub const MAX_JSON_NODES: usize = 16_384;

/// Maximum number of contributions in one manifest.
pub const MAX_CONTRIBUTIONS: usize = 256;

/// Maximum number of capability declarations in one manifest.
pub const MAX_CAPABILITIES: usize = 32;

/// Maximum bytes stored in one portable state value.
pub const MAX_STATE_VALUE_BYTES: usize = 64 * 1024;

/// Maximum bytes in one custom session event payload.
pub const MAX_CUSTOM_EVENT_BYTES: usize = 64 * 1024;

/// Maximum bytes returned by one prompt contribution.
pub const MAX_PROMPT_CONTRIBUTION_BYTES: usize = 16 * 1024;

/// Maximum bytes in one declarative UI view.
pub const MAX_UI_VIEW_BYTES: usize = 64 * 1024;

/// Maximum bytes in one UI action DTO.
pub const MAX_UI_ACTION_BYTES: usize = 16 * 1024;

/// Maximum UTF-8 bytes accepted from one guest export or host import string.
pub const MAX_GUEST_OUTPUT_BYTES: usize = 256 * 1024;

/// Maximum UTF-8 bytes accepted by the host `log` import.
pub const MAX_HOST_LOG_BYTES: usize = 8 * 1024;

/// Maximum retained host `log` records for one plugin generation.
///
/// Bounds host-side capture independently of WASM store limits. Combined with
/// [`MAX_HOST_LOG_BYTES`], worst-case retained log memory is 2 MiB per
/// generation. Lowering this drops diagnostics; raising it grows host RSS.
pub const MAX_HOST_LOG_RECORDS: usize = 256;

/// Maximum retained `publish-view` documents for one plugin generation.
///
/// Combined with [`MAX_UI_VIEW_BYTES`], worst-case retained view memory is
/// 2 MiB per generation. These records are host-owned and not covered by
/// WASM `StoreLimits`.
pub const MAX_HOST_VIEW_RECORDS: usize = 32;

/// Maximum retained `emit-action` documents for one plugin generation.
///
/// Combined with [`MAX_UI_ACTION_BYTES`], worst-case retained action memory
/// is 1 MiB per generation.
pub const MAX_HOST_ACTION_RECORDS: usize = 64;

/// Maximum descriptors of any one contribution kind.
pub const MAX_DESCRIPTORS_PER_KIND: usize = 128;

/// Maximum length of a relative plugin path in UTF-8 bytes.
pub const MAX_PLUGIN_PATH_BYTES: usize = 4096;

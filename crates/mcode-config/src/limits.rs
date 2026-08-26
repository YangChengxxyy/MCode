//! Resource limits applied before a configuration can be published.

// Rust guideline compliant 2026-08-26

/// Highest accepted JSON nesting limit.
///
/// Keeping this below `serde_json`'s own recursion guard ensures this crate can
/// classify depth failures deterministically before stack usage becomes risky.
pub const MAX_SUPPORTED_DEPTH: usize = 64;

/// Bounds external input and the merged configuration.
///
/// Every field must be nonzero. `max_depth` cannot exceed
/// [`MAX_SUPPORTED_DEPTH`]. Limits are checked before every reload, so callers
/// may construct this type directly and receive a regular configuration error
/// instead of a panic for an invalid limit set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConfigLimits {
    /// Maximum bytes read from any one source envelope.
    pub max_source_bytes: usize,
    /// Maximum bytes read from all participating source envelopes combined.
    pub max_total_bytes: usize,
    /// Maximum nesting depth, counting the configuration root as depth one.
    pub max_depth: usize,
    /// Maximum JSON values plus object member names in one document.
    pub max_nodes: usize,
    /// Maximum number of source descriptors accepted for one load.
    pub max_sources: usize,
    /// Maximum number of non-fatal diagnostics retained in a snapshot.
    pub max_diagnostics: usize,
}

impl ConfigLimits {
    pub(crate) fn are_valid(self) -> bool {
        self.max_source_bytes > 0
            && self.max_total_bytes > 0
            && self.max_depth > 0
            && self.max_depth <= MAX_SUPPORTED_DEPTH
            && self.max_nodes > 0
            && self.max_sources > 0
            && self.max_diagnostics > 0
    }
}

impl Default for ConfigLimits {
    fn default() -> Self {
        // Configuration is control-plane data. These limits leave ample room
        // for normal settings while bounding memory, CPU, and diagnostic use
        // for files controlled by an untrusted project checkout.
        Self {
            max_source_bytes: 1024 * 1024,
            max_total_bytes: 4 * 1024 * 1024,
            max_depth: 64,
            max_nodes: 100_000,
            max_sources: 32,
            max_diagnostics: 32,
        }
    }
}

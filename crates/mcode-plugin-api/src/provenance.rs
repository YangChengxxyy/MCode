//! Stable plugin provenance and trust metadata.

// Rust guideline compliant 2026-08-26.

use std::path::PathBuf;

use semver::Version;
use serde::{Deserialize, Serialize};

use crate::ids::PluginId;

/// Coarse source scope used for trust decisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SourceScope {
    /// WASM component bundled with MCode and still replaceable.
    Bundled,
    /// User-scoped plugin source.
    User,
    /// Project-controlled plugin source.
    Project,
    /// Explicit command-line plugin source.
    CommandLine,
}

/// Concrete source of a plugin registration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "scope", rename_all = "camelCase", deny_unknown_fields)]
pub enum PluginSource {
    /// A WASM component shipped with MCode.
    Bundled {
        /// Stable bundle label.
        bundle: String,
    },
    /// A plugin below a user-controlled root.
    User {
        /// Canonical plugin root.
        root: PathBuf,
    },
    /// A plugin below a project-controlled root.
    Project {
        /// Canonical plugin root.
        root: PathBuf,
    },
    /// A plugin root explicitly supplied by the user.
    CommandLine {
        /// Canonical plugin root.
        root: PathBuf,
    },
}

impl PluginSource {
    /// Returns the source's coarse trust scope.
    #[must_use]
    pub fn scope(&self) -> SourceScope {
        match self {
            Self::Bundled { .. } => SourceScope::Bundled,
            Self::User { .. } => SourceScope::User,
            Self::Project { .. } => SourceScope::Project,
            Self::CommandLine { .. } => SourceScope::CommandLine,
        }
    }
}

/// Trust decision attached to a plugin source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TrustLevel {
    /// No execution or prompt/resource contribution is permitted.
    Untrusted,
    /// A bundled WASM component shipped with MCode.
    BuiltIn,
    /// A user-approved user or command-line source.
    TrustedUser,
    /// A user-approved project source.
    TrustedProject,
}

impl TrustLevel {
    /// Returns whether this decision authorizes a source scope.
    #[must_use]
    pub fn permits(self, scope: SourceScope) -> bool {
        matches!(
            (self, scope),
            (Self::BuiltIn, SourceScope::Bundled)
                | (
                    Self::TrustedUser,
                    SourceScope::User | SourceScope::CommandLine
                )
                | (Self::TrustedProject, SourceScope::Project)
        )
    }
}

/// Immutable ownership metadata carried with every contribution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Provenance {
    plugin_id: PluginId,
    version: String,
    source: PluginSource,
    trust: TrustLevel,
}

impl Provenance {
    /// Creates validated provenance.
    ///
    /// # Errors
    ///
    /// Returns [`ProvenanceError`] when `version` is not a complete semantic
    /// version or the trust decision does not match the source scope.
    pub fn new(
        plugin_id: PluginId,
        version: impl AsRef<str>,
        source: PluginSource,
        trust: TrustLevel,
    ) -> Result<Self, ProvenanceError> {
        let parsed =
            Version::parse(version.as_ref()).map_err(|_| ProvenanceError::InvalidVersion)?;
        if trust != TrustLevel::Untrusted && !trust.permits(source.scope()) {
            return Err(ProvenanceError::TrustSourceMismatch);
        }
        Ok(Self {
            plugin_id,
            version: parsed.to_string(),
            source,
            trust,
        })
    }

    /// Returns the owning plugin id.
    #[must_use]
    pub fn plugin_id(&self) -> &PluginId {
        &self.plugin_id
    }

    /// Returns the normalized semantic version.
    #[must_use]
    pub fn version(&self) -> &str {
        &self.version
    }

    /// Returns the concrete discovery source.
    #[must_use]
    pub fn source(&self) -> &PluginSource {
        &self.source
    }

    /// Returns the trust decision.
    #[must_use]
    pub fn trust(&self) -> TrustLevel {
        self.trust
    }

    /// Returns whether this provenance may activate contributions.
    #[must_use]
    pub fn is_trusted(&self) -> bool {
        self.trust.permits(self.source.scope())
    }
}

/// Provenance construction failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ProvenanceError {
    /// The version was not a complete semantic version.
    #[error("plugin provenance version is not valid semantic versioning")]
    InvalidVersion,
    /// The trust level was issued for a different source scope.
    #[error("plugin trust level does not match its source scope")]
    TrustSourceMismatch,
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{PluginSource, Provenance, TrustLevel};
    use crate::ids::PluginId;

    #[test]
    fn trust_must_match_source() {
        let id = PluginId::parse("com.mcode.test").expect("id");
        assert!(
            Provenance::new(
                id.clone(),
                "1.0.0",
                PluginSource::Project {
                    root: PathBuf::from("project"),
                },
                TrustLevel::TrustedUser,
            )
            .is_err()
        );
        assert!(
            Provenance::new(
                id,
                "1.0.0",
                PluginSource::Project {
                    root: PathBuf::from("project"),
                },
                TrustLevel::Untrusted,
            )
            .is_ok()
        );
    }
}

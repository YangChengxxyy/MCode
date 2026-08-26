//! Deterministic `plugin.json` filesystem discovery.

// Rust guideline compliant 2026-08-26.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use mcode_plugin_api::{
    ManifestError, PluginManifest, PluginSource, Provenance, SourceScope, TrustLevel,
};

/// One validated plugin found on disk.
#[derive(Debug, Clone, PartialEq)]
pub struct DiscoveredPlugin {
    manifest: PluginManifest,
    provenance: Provenance,
}

impl DiscoveredPlugin {
    /// Returns the validated manifest.
    #[must_use]
    pub fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    /// Returns stable source and trust metadata.
    #[must_use]
    pub fn provenance(&self) -> &Provenance {
        &self.provenance
    }

    /// Consumes this result into its manifest and provenance.
    #[must_use]
    pub fn into_parts(self) -> (PluginManifest, Provenance) {
        (self.manifest, self.provenance)
    }
}

/// Non-fatal issue collected while scanning a plugin directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveryIssue {
    /// Candidate path that failed.
    pub path: PathBuf,
    /// Sanitized issue kind.
    pub kind: DiscoveryIssueKind,
}

/// Sanitized discovery issue category.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoveryIssueKind {
    /// Parent directory could not be read safely.
    DirectoryUnreadable,
    /// Candidate canonical path escaped the configured discovery root.
    EscapesDiscoveryRoot,
    /// Candidate had no `plugin.json`.
    ManifestMissing,
    /// Candidate manifest was invalid.
    ManifestInvalid,
    /// Source and trust metadata were inconsistent.
    InvalidProvenance,
}

/// Complete deterministic discovery result.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DiscoveryReport {
    /// Validated plugins sorted by canonical path.
    pub plugins: Vec<DiscoveredPlugin>,
    /// Non-fatal candidate failures.
    pub issues: Vec<DiscoveryIssue>,
}

/// Discovers immediate plugin subdirectories below `parent`.
///
/// Symlink aliases are canonicalized and deduplicated. A symlink escaping
/// `parent` is rejected. Bundled scope is not valid for filesystem discovery.
#[must_use]
pub fn discover_directory(
    parent: impl AsRef<Path>,
    scope: SourceScope,
    trust: TrustLevel,
) -> DiscoveryReport {
    let parent = parent.as_ref();
    let canonical_parent = match std::fs::canonicalize(parent) {
        Ok(path) => path,
        Err(_) => {
            return DiscoveryReport {
                plugins: vec![],
                issues: vec![DiscoveryIssue {
                    path: parent.to_path_buf(),
                    kind: DiscoveryIssueKind::DirectoryUnreadable,
                }],
            };
        }
    };
    let entries = match std::fs::read_dir(parent) {
        Ok(entries) => entries,
        Err(_) => {
            return DiscoveryReport {
                plugins: vec![],
                issues: vec![DiscoveryIssue {
                    path: parent.to_path_buf(),
                    kind: DiscoveryIssueKind::DirectoryUnreadable,
                }],
            };
        }
    };
    let mut candidates: Vec<_> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    candidates.sort();

    let mut report = DiscoveryReport::default();
    let mut seen = BTreeSet::new();
    for candidate in candidates {
        let canonical = match std::fs::canonicalize(&candidate) {
            Ok(path) if path.starts_with(&canonical_parent) => path,
            _ => {
                report.issues.push(DiscoveryIssue {
                    path: candidate,
                    kind: DiscoveryIssueKind::EscapesDiscoveryRoot,
                });
                continue;
            }
        };
        if !seen.insert(canonical.clone()) {
            continue;
        }
        let source = source_for(scope, canonical.clone());
        match discover_plugin_root(&canonical, source, trust) {
            Ok(plugin) => report.plugins.push(plugin),
            Err(kind) => report.issues.push(DiscoveryIssue {
                path: canonical,
                kind,
            }),
        }
    }
    report
}

/// Discovers one explicit plugin root.
///
/// # Errors
///
/// Returns [`DiscoveryIssueKind`] when the root has no manifest, the manifest
/// is invalid, or provenance is inconsistent.
pub fn discover_plugin_root(
    root: impl AsRef<Path>,
    source: PluginSource,
    trust: TrustLevel,
) -> Result<DiscoveredPlugin, DiscoveryIssueKind> {
    let root = root.as_ref();
    if !root.join("plugin.json").is_file() {
        return Err(DiscoveryIssueKind::ManifestMissing);
    }
    let manifest = PluginManifest::from_plugin_root(root).map_err(map_manifest_error)?;
    let provenance = Provenance::new(manifest.id().clone(), manifest.version(), source, trust)
        .map_err(|_| DiscoveryIssueKind::InvalidProvenance)?;
    Ok(DiscoveredPlugin {
        manifest,
        provenance,
    })
}

fn source_for(scope: SourceScope, root: PathBuf) -> PluginSource {
    match scope {
        SourceScope::Bundled => PluginSource::Bundled {
            bundle: root.to_string_lossy().into_owned(),
        },
        SourceScope::User => PluginSource::User { root },
        SourceScope::Project => PluginSource::Project { root },
        SourceScope::CommandLine => PluginSource::CommandLine { root },
    }
}

fn map_manifest_error(_error: ManifestError) -> DiscoveryIssueKind {
    DiscoveryIssueKind::ManifestInvalid
}

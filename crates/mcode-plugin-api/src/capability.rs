//! Least-privilege capability declarations and host grants.

// Rust guideline compliant 2026-08-26.

use std::collections::{BTreeSet, HashSet};
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::ids::Identifier;
use crate::limits::MAX_CAPABILITIES;
use crate::path::resolve_contained_path;

const MAX_SCOPE_ITEMS: usize = 128;

/// Coarse capability kinds granted by host policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CapabilityKind {
    /// Bounded filesystem access mediated by a future host adapter.
    Filesystem,
    /// Network access mediated by a future host adapter.
    Network,
    /// Opaque secret handles.
    Secrets,
    /// Session-scoped extension JSON state.
    SessionState,
    /// Declarative UI publication.
    Ui,
    /// Bounded prompt text.
    PromptContribution,
}

/// Filesystem operations that can be declared.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FilesystemAccess {
    /// Read files or metadata.
    Read,
    /// Create or modify files.
    Write,
}

/// Manifest-declared capability with a minimal scope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum CapabilityDeclaration {
    /// Filesystem access limited to session-cwd-relative paths.
    Filesystem {
        /// Allowed operations.
        access: Vec<FilesystemAccess>,
        /// Allowed roots relative to the session cwd; `.` means the whole cwd.
        roots: Vec<String>,
    },
    /// Network access limited to exact hosts or leading `*.` suffix matches.
    Network {
        /// Allowed lowercase host patterns without scheme, path, or port.
        hosts: Vec<String>,
    },
    /// Secret lookup limited to declared names.
    Secrets {
        /// Secret names for which opaque handles may be requested.
        names: Vec<Identifier>,
    },
    /// Session-scoped extension JSON state.
    SessionState {},
    /// Declarative UI publication.
    Ui {},
    /// Bounded prompt text.
    PromptContribution {},
}

impl CapabilityDeclaration {
    /// Returns the declaration's coarse kind.
    #[must_use]
    pub fn kind(&self) -> CapabilityKind {
        match self {
            Self::Filesystem { .. } => CapabilityKind::Filesystem,
            Self::Network { .. } => CapabilityKind::Network,
            Self::Secrets { .. } => CapabilityKind::Secrets,
            Self::SessionState {} => CapabilityKind::SessionState,
            Self::Ui {} => CapabilityKind::Ui,
            Self::PromptContribution {} => CapabilityKind::PromptContribution,
        }
    }
}

/// Exact capability use for which a token is requested.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CapabilityUse {
    /// One filesystem operation on a session-cwd-relative path.
    Filesystem {
        /// Requested operation.
        access: FilesystemAccess,
        /// Portable path relative to the session cwd.
        path: String,
    },
    /// Access one host through a host network adapter.
    Network {
        /// Lowercase host without scheme, path, credentials, or port.
        host: String,
    },
    /// Open one declared secret as an opaque handle.
    Secret {
        /// Declared secret name.
        name: Identifier,
    },
    /// Read or update state through the session-owned adapter.
    SessionState,
    /// Publish a declarative UI view or action.
    Ui,
    /// Return a bounded prompt contribution.
    PromptContribution,
}

impl CapabilityUse {
    /// Returns the coarse grant required by this exact use.
    #[must_use]
    pub fn kind(&self) -> CapabilityKind {
        match self {
            Self::Filesystem { .. } => CapabilityKind::Filesystem,
            Self::Network { .. } => CapabilityKind::Network,
            Self::Secret { .. } => CapabilityKind::Secrets,
            Self::SessionState => CapabilityKind::SessionState,
            Self::Ui => CapabilityKind::Ui,
            Self::PromptContribution => CapabilityKind::PromptContribution,
        }
    }
}

/// Host-selected coarse grants intersected with manifest declarations.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CapabilityGrants {
    allowed: BTreeSet<CapabilityKind>,
}

impl CapabilityGrants {
    /// Creates an empty grant set.
    #[must_use]
    pub fn none() -> Self {
        Self::default()
    }

    /// Creates grants for the supplied capability kinds.
    #[must_use]
    pub fn from_kinds(kinds: impl IntoIterator<Item = CapabilityKind>) -> Self {
        Self {
            allowed: kinds.into_iter().collect(),
        }
    }

    /// Adds one coarse capability grant.
    pub fn allow(&mut self, kind: CapabilityKind) {
        self.allowed.insert(kind);
    }

    /// Returns whether the coarse capability was granted.
    #[must_use]
    pub fn allows(&self, kind: CapabilityKind) -> bool {
        self.allowed.contains(&kind)
    }
}

/// Validates all capability declarations and portable cwd-relative scopes.
///
/// Declaring a capability never authorizes ambient WASI imports.
///
/// # Errors
///
/// Returns [`CapabilityValidationError`] for duplicates, empty or excessive
/// scopes, unsafe paths, or malformed hosts.
pub fn validate_capabilities(
    declarations: &[CapabilityDeclaration],
) -> Result<(), CapabilityValidationError> {
    if declarations.len() > MAX_CAPABILITIES {
        return Err(CapabilityValidationError::TooMany);
    }
    let mut kinds = HashSet::new();
    for declaration in declarations {
        if !kinds.insert(declaration.kind()) {
            return Err(CapabilityValidationError::DuplicateKind);
        }
        match declaration {
            CapabilityDeclaration::Filesystem { access, roots } => {
                if access.is_empty()
                    || roots.is_empty()
                    || access.len() > 2
                    || roots.len() > MAX_SCOPE_ITEMS
                {
                    return Err(CapabilityValidationError::InvalidScope);
                }
                let unique_access: BTreeSet<_> = access.iter().copied().collect();
                if unique_access.len() != access.len() {
                    return Err(CapabilityValidationError::InvalidScope);
                }
                for root in roots {
                    if !valid_scope_path(root) {
                        return Err(CapabilityValidationError::UnsafePath);
                    }
                }
            }
            CapabilityDeclaration::Network { hosts } => {
                validate_string_scope(hosts, validate_host)?;
            }
            CapabilityDeclaration::Secrets { names } => {
                if names.is_empty() || names.len() > MAX_SCOPE_ITEMS {
                    return Err(CapabilityValidationError::InvalidScope);
                }
                let unique: BTreeSet<_> = names.iter().collect();
                if unique.len() != names.len() {
                    return Err(CapabilityValidationError::InvalidScope);
                }
            }
            CapabilityDeclaration::SessionState {}
            | CapabilityDeclaration::Ui {}
            | CapabilityDeclaration::PromptContribution {} => {}
        }
    }
    Ok(())
}

/// Returns whether declarations authorize one exact use.
///
/// The caller must separately enforce [`CapabilityGrants`] and trust.
#[must_use]
pub fn declaration_allows(
    declarations: &[CapabilityDeclaration],
    scope_root: &Path,
    requested: &CapabilityUse,
) -> bool {
    declarations
        .iter()
        .any(|declaration| match (declaration, requested) {
            (
                CapabilityDeclaration::Filesystem { access, roots },
                CapabilityUse::Filesystem {
                    access: requested_access,
                    path,
                },
            ) => {
                if !access.contains(requested_access) {
                    return false;
                }
                let Ok(requested_path) = resolve_scope_path(scope_root, path) else {
                    return false;
                };
                roots.iter().any(|root| {
                    resolve_scope_path(scope_root, root)
                        .is_ok_and(|allowed_root| requested_path.starts_with(allowed_root))
                })
            }
            (CapabilityDeclaration::Network { hosts }, CapabilityUse::Network { host }) => {
                validate_host(host) && hosts.iter().any(|pattern| host_matches(pattern, host))
            }
            (CapabilityDeclaration::Secrets { names }, CapabilityUse::Secret { name }) => {
                names.contains(name)
            }
            (CapabilityDeclaration::SessionState {}, CapabilityUse::SessionState)
            | (CapabilityDeclaration::Ui {}, CapabilityUse::Ui)
            | (CapabilityDeclaration::PromptContribution {}, CapabilityUse::PromptContribution) => {
                true
            }
            _ => false,
        })
}

fn valid_scope_path(relative: &str) -> bool {
    if relative == "." {
        return true;
    }
    !relative.is_empty()
        && relative.len() <= crate::limits::MAX_PLUGIN_PATH_BYTES
        && !relative.contains(['\\', '\0'])
        && !Path::new(relative).is_absolute()
        && Path::new(relative).components().all(|component| {
            !matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
        && Path::new(relative)
            .components()
            .any(|component| matches!(component, Component::Normal(_)))
}

fn resolve_scope_path(root: &Path, relative: &str) -> Result<PathBuf, ()> {
    if relative == "." {
        return std::fs::canonicalize(root).map_err(|_| ());
    }
    resolve_contained_path(root, relative).map_err(|_| ())
}

fn validate_string_scope(
    values: &[String],
    validator: fn(&str) -> bool,
) -> Result<(), CapabilityValidationError> {
    if values.is_empty()
        || values.len() > MAX_SCOPE_ITEMS
        || values.iter().any(|value| !validator(value))
    {
        return Err(CapabilityValidationError::InvalidScope);
    }
    let unique: BTreeSet<_> = values.iter().collect();
    if unique.len() != values.len() {
        return Err(CapabilityValidationError::InvalidScope);
    }
    Ok(())
}

fn validate_host(host: &str) -> bool {
    let bare = host.strip_prefix("*.").unwrap_or(host);
    !bare.is_empty()
        && bare.len() <= 253
        && bare.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'-' | b':')
        })
        && !bare.contains("..")
        && !bare.starts_with(['.', '-'])
        && !bare.ends_with(['.', '-'])
}

fn host_matches(pattern: &str, host: &str) -> bool {
    match pattern.strip_prefix("*.") {
        Some(suffix) => {
            host.len() > suffix.len()
                && host.ends_with(suffix)
                && host.as_bytes()[host.len() - suffix.len() - 1] == b'.'
        }
        None => pattern == host,
    }
}

/// Invalid capability declaration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CapabilityValidationError {
    /// Too many capability kinds were declared.
    #[error("plugin declares too many capabilities")]
    TooMany,
    /// A capability kind was declared more than once.
    #[error("plugin declares a capability kind more than once")]
    DuplicateKind,
    /// A capability scope was empty, duplicated, malformed, or excessive.
    #[error("plugin capability scope is invalid")]
    InvalidScope,
    /// A filesystem scope escaped the plugin root.
    #[error("plugin filesystem capability contains an unsafe path")]
    UnsafePath,
}

#[cfg(test)]
mod tests {
    use super::{
        CapabilityDeclaration, CapabilityUse, FilesystemAccess, declaration_allows,
        validate_capabilities,
    };

    #[test]
    fn filesystem_scope_is_contained_and_operation_specific() {
        let root = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir(root.path().join("data")).expect("data dir");
        let declarations = vec![CapabilityDeclaration::Filesystem {
            access: vec![FilesystemAccess::Read],
            roots: vec!["data".into()],
        }];
        validate_capabilities(&declarations).expect("valid declaration");
        assert!(declaration_allows(
            &declarations,
            root.path(),
            &CapabilityUse::Filesystem {
                access: FilesystemAccess::Read,
                path: "data/file.txt".into(),
            }
        ));
        assert!(!declaration_allows(
            &declarations,
            root.path(),
            &CapabilityUse::Filesystem {
                access: FilesystemAccess::Write,
                path: "data/file.txt".into(),
            }
        ));
        assert!(!declaration_allows(
            &declarations,
            root.path(),
            &CapabilityUse::Filesystem {
                access: FilesystemAccess::Read,
                path: "../outside.txt".into(),
            }
        ));
    }
}

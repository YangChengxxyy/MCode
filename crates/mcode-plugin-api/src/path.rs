//! Plugin-root path containment helpers.

// Rust guideline compliant 2026-08-26.

use std::path::{Component, Path, PathBuf};

use crate::limits::MAX_PLUGIN_PATH_BYTES;

/// Resolves a portable relative path while enforcing plugin-root containment.
///
/// Existing symlinks are inspected and canonicalized. Dangling symlinks are
/// rejected. For a missing leaf, the nearest existing ancestor is canonicalized
/// so a symlinked parent cannot escape.
///
/// # Errors
///
/// Returns [`PathValidationError`] for empty, absolute, parent-relative,
/// backslash-containing, oversized, unresolvable, or escaping paths.
pub fn resolve_contained_path(
    plugin_root: &Path,
    relative: &str,
) -> Result<PathBuf, PathValidationError> {
    if relative.is_empty() || relative.len() > MAX_PLUGIN_PATH_BYTES || relative.contains('\0') {
        return Err(PathValidationError::InvalidRelativePath);
    }
    // Manifest paths are portable JSON paths. Requiring `/` avoids a value
    // being safe on Unix but traversing on Windows after distribution.
    if relative.contains('\\') {
        return Err(PathValidationError::InvalidRelativePath);
    }
    let relative_path = Path::new(relative);
    if relative_path.is_absolute()
        || relative_path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(PathValidationError::InvalidRelativePath);
    }
    if !relative_path
        .components()
        .any(|component| matches!(component, Component::Normal(_)))
    {
        return Err(PathValidationError::InvalidRelativePath);
    }

    let canonical_root =
        std::fs::canonicalize(plugin_root).map_err(|_| PathValidationError::RootUnavailable)?;
    let candidate = canonical_root.join(relative_path);
    let mut existing = candidate.as_path();
    loop {
        match std::fs::symlink_metadata(existing) {
            Ok(_metadata) => break,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                existing = existing
                    .parent()
                    .ok_or(PathValidationError::PathUnavailable)?;
            }
            Err(_error) => return Err(PathValidationError::PathUnavailable),
        }
    }
    let canonical_existing =
        std::fs::canonicalize(existing).map_err(|_| PathValidationError::PathUnavailable)?;
    if !canonical_existing.starts_with(&canonical_root) {
        return Err(PathValidationError::EscapesRoot);
    }
    Ok(candidate)
}

/// Plugin path containment failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PathValidationError {
    /// The JSON path was not a portable relative path.
    #[error("plugin path must be a portable contained relative path")]
    InvalidRelativePath,
    /// The plugin root could not be canonicalized.
    #[error("plugin root cannot be canonicalized")]
    RootUnavailable,
    /// The nearest existing path could not be canonicalized.
    #[error("plugin path cannot be resolved safely")]
    PathUnavailable,
    /// A symlink or path component escaped the plugin root.
    #[error("plugin path escapes its root")]
    EscapesRoot,
}

#[cfg(test)]
mod tests {
    use super::{PathValidationError, resolve_contained_path};

    #[cfg(unix)]
    fn create_dir_link(target: &std::path::Path, link: &std::path::Path) -> std::io::Result<()> {
        std::os::unix::fs::symlink(target, link)
    }

    #[cfg(windows)]
    fn create_dir_link(target: &std::path::Path, link: &std::path::Path) -> std::io::Result<()> {
        // Directory junctions exercise dangling reparse-point handling without
        // requiring SeCreateSymbolicLinkPrivilege in Windows test processes.
        let status = std::process::Command::new("cmd")
            .arg("/c")
            .arg("mklink")
            .arg("/J")
            .arg(link)
            .arg(target)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()?;
        if status.success() {
            Ok(())
        } else {
            Err(std::io::Error::other(format!(
                "mklink /J exited with {status}"
            )))
        }
    }

    #[test]
    fn rejects_portable_traversal_forms() {
        let root = tempfile::tempdir().expect("tempdir");
        assert_eq!(
            resolve_contained_path(root.path(), "../outside"),
            Err(PathValidationError::InvalidRelativePath)
        );
        assert_eq!(
            resolve_contained_path(root.path(), "..\\outside"),
            Err(PathValidationError::InvalidRelativePath)
        );
        assert!(resolve_contained_path(root.path(), "nested/entry.wasm").is_ok());
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn rejects_dangling_symlink_ancestors() {
        let root = tempfile::tempdir().expect("tempdir");
        let outside = tempfile::tempdir().expect("outside tempdir");
        let future_outside_target = outside.path().join("created-later");
        let link = root.path().join("link");
        create_dir_link(&future_outside_target, &link).expect("dangling directory link");

        assert_eq!(
            resolve_contained_path(root.path(), "link/entry.wasm"),
            Err(PathValidationError::PathUnavailable)
        );

        std::fs::create_dir(&future_outside_target).expect("future outside target");
        assert_eq!(
            resolve_contained_path(root.path(), "link/entry.wasm"),
            Err(PathValidationError::EscapesRoot)
        );
    }
}

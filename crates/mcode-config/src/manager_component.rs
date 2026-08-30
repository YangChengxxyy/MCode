//! Reads one bounded opaque Manager component from its canonical artifact path.
//!
//! This surface does not inspect receipts, manifests, inventories, signatures,
//! trust state, or activation state. Callers select the typed family and
//! canonical version; this module owns the sole artifact path mapping.

// Rust guideline compliant 2026-08-29

use std::path::PathBuf;

use zeroize::Zeroizing;

use crate::secure_fs::owned_file::read_owned_file;
use crate::{CanonicalVersion, ConfigError, HomeLayout, PluginFamily};

/// Maximum byte length of one Manager component artifact.
pub const MAX_MANAGER_COMPONENT_BYTES: usize = 4 * 1024 * 1024;

/// Reads one Manager component artifact without creating filesystem objects.
///
/// The sole mapping is
/// `plugins/<family>/manager/versions/<canonical-semver>/component.wasm` below
/// `home`. Returned bytes are exact and opaque.
///
/// # Errors
///
/// Returns [`ConfigError`] for owned-path security, file I/O, or when the
/// artifact exceeds [`MAX_MANAGER_COMPONENT_BYTES`].
pub fn read_manager_component(
    home: &HomeLayout,
    family: PluginFamily,
    version: &CanonicalVersion,
) -> Result<Option<Zeroizing<Vec<u8>>>, ConfigError> {
    let relative = component_relative_path(family, version);
    let target = home.root().join(&relative);
    read_owned_file(home, relative, MAX_MANAGER_COMPONENT_BYTES)
        .map_err(|error| error.with_path(&target))
}

fn component_relative_path(family: PluginFamily, version: &CanonicalVersion) -> PathBuf {
    PathBuf::from("plugins")
        .join(family.directory_name())
        .join("manager")
        .join("versions")
        .join(version.as_str())
        .join("component.wasm")
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use super::{MAX_MANAGER_COMPONENT_BYTES, read_manager_component};
    use crate::secure_fs::owned_file::replace_owned_file;
    use crate::{CanonicalVersion, ConfigErrorKind, HomeLayout, PluginFamily};

    fn layout() -> (tempfile::TempDir, HomeLayout) {
        let parent = tempfile::tempdir().expect("temporary parent");
        let home = HomeLayout::from_root(parent.path().join("home")).expect("valid layout");
        (parent, home)
    }

    fn version(value: &str) -> CanonicalVersion {
        CanonicalVersion::parse(value).expect("canonical version")
    }

    #[test]
    fn exact_canonical_path_returns_exact_opaque_bytes() {
        let (_parent, home) = layout();
        let relative =
            PathBuf::from("plugins/web/manager/versions/1.2.3-alpha.1+build.7/component.wasm");
        let artifact = b"\0asm\x01\0\0\0opaque-manager-component";
        replace_owned_file(&home, &relative, artifact).expect("write component fixture");

        let bytes =
            read_manager_component(&home, PluginFamily::Web, &version("1.2.3-alpha.1+build.7"))
                .expect("read component")
                .expect("present component");

        assert_eq!(bytes.as_slice(), artifact);
    }

    #[test]
    fn missing_component_creates_nothing() {
        let (parent, home) = layout();

        let bytes = read_manager_component(&home, PluginFamily::Providers, &version("1.0.0"))
            .expect("missing read");

        assert!(bytes.is_none());
        assert_eq!(
            fs::read_dir(parent.path()).expect("parent listing").count(),
            0
        );
    }

    #[test]
    fn four_mib_component_is_accepted() {
        let (_parent, home) = layout();
        let relative = "plugins/ui/manager/versions/2.0.0/component.wasm";
        let artifact = vec![0x5a; MAX_MANAGER_COMPONENT_BYTES];
        replace_owned_file(&home, relative, &artifact).expect("write boundary fixture");

        let bytes = read_manager_component(&home, PluginFamily::Ui, &version("2.0.0"))
            .expect("read boundary component")
            .expect("present boundary component");

        assert_eq!(bytes.as_slice(), artifact);
    }

    #[test]
    fn component_larger_than_four_mib_is_rejected() {
        let (_parent, home) = layout();
        let relative = PathBuf::from("plugins/ask/manager/versions/3.0.0/component.wasm");
        replace_owned_file(
            &home,
            &relative,
            &vec![0xa5; MAX_MANAGER_COMPONENT_BYTES + 1],
        )
        .expect("write oversized fixture");

        let error = read_manager_component(&home, PluginFamily::Ask, &version("3.0.0"))
            .expect_err("oversized component");

        assert_eq!(error.kind(), ConfigErrorKind::Oversized);
        assert_eq!(error.path(), Some(home.root().join(relative).as_path()));
    }
}

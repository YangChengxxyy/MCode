//! Coordinates validated owned-file reads and locked atomic updates.
//!
//! This module deliberately contains no document schema. It validates every
//! relative path through [`HomeLayout`], then delegates handle-relative native
//! operations to the active platform implementation.

// Rust guideline compliant 2026-08-29

use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

use crate::{ConfigError, ConfigErrorKind, HomeLayout};

#[cfg(unix)]
use super::unix::unix_file as platform;
#[cfg(windows)]
use super::windows::windows_file as platform;

/// Creates only the owned directories named by `relative`.
pub(crate) fn ensure_owned_directory(
    home: &HomeLayout,
    relative: impl AsRef<Path>,
) -> Result<(), ConfigError> {
    let path = OwnedPath::new(home, relative.as_ref())?;
    platform::ensure_directory(&path.root, &path.components)
}

/// Reads a private regular file without creating any filesystem object.
pub(crate) fn read_owned_file(
    home: &HomeLayout,
    relative: impl AsRef<Path>,
    maximum_bytes: usize,
) -> Result<Option<Vec<u8>>, ConfigError> {
    let path = OwnedPath::new(home, relative.as_ref())?;
    require_file_name(&path)?;
    platform::read_file(&path.root, &path.components, maximum_bytes)
}

/// Replaces a private regular file while holding its persistent lock.
pub(crate) fn replace_owned_file(
    home: &HomeLayout,
    relative: impl AsRef<Path>,
    bytes: &[u8],
) -> Result<(), ConfigError> {
    let path = OwnedPath::new(home, relative.as_ref())?;
    require_file_name(&path)?;
    let mut transaction = platform::Transaction::begin(&path.root, &path.components)?;
    transaction.replace(bytes)
}

/// Runs one read-modify-replace callback under a persistent advisory lock.
pub(crate) fn locked_update_owned_file(
    home: &HomeLayout,
    relative: impl AsRef<Path>,
    maximum_bytes: usize,
    update: impl FnOnce(Option<&[u8]>) -> Result<Vec<u8>, ConfigError>,
) -> Result<(), ConfigError> {
    let path = OwnedPath::new(home, relative.as_ref())?;
    require_file_name(&path)?;
    let mut transaction = platform::Transaction::begin(&path.root, &path.components)?;
    let current = transaction.read(maximum_bytes)?;
    let replacement = update(current.as_deref())?;
    transaction.replace(&replacement)
}

struct OwnedPath {
    root: PathBuf,
    components: Vec<OsString>,
}

impl OwnedPath {
    fn new(home: &HomeLayout, relative: &Path) -> Result<Self, ConfigError> {
        let joined = home.owned_join(relative)?;
        let relative = joined
            .strip_prefix(home.root())
            .map_err(|_| ConfigError::new(ConfigErrorKind::PathEscape))?;
        let components = relative
            .components()
            .map(|component| match component {
                Component::Normal(name) => Ok(name.to_os_string()),
                _ => Err(ConfigError::new(ConfigErrorKind::PathEscape)),
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            root: home.root().to_path_buf(),
            components,
        })
    }
}

fn require_file_name(path: &OwnedPath) -> Result<(), ConfigError> {
    if path.components.is_empty() {
        return Err(ConfigError::new(ConfigErrorKind::PathEscape));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::process::Command;
    use std::sync::{Arc, Barrier};

    use super::{
        OwnedPath, ensure_owned_directory, locked_update_owned_file, platform, read_owned_file,
        replace_owned_file,
    };
    use crate::{
        AccessControlEvidence, ConfigError, ConfigErrorKind, HomeLayout, OwnedKind,
        probe_access_control,
    };

    fn layout() -> (tempfile::TempDir, HomeLayout) {
        let parent = tempfile::tempdir().expect("parent");
        let layout = HomeLayout::from_root(parent.path().join("home")).expect("layout");
        (parent, layout)
    }

    #[test]
    fn missing_read_creates_nothing() {
        let (parent, layout) = layout();
        let value =
            read_owned_file(&layout, "plugins/providers/host/auth.json", 64).expect("missing read");
        assert!(value.is_none());
        assert_eq!(
            fs::read_dir(parent.path()).expect("parent listing").count(),
            0
        );
    }

    #[test]
    fn wrong_case_owned_root_alias_is_rejected() {
        let parent = tempfile::tempdir().expect("parent");
        fs::create_dir(parent.path().join("Home")).expect("root alias");
        let layout = HomeLayout::from_root(parent.path().join("home")).expect("layout");

        let error = read_owned_file(&layout, "config.json", 64).expect_err("root alias");

        assert_eq!(error.kind(), ConfigErrorKind::AccessControl);
        let names = fs::read_dir(parent.path())
            .expect("parent listing")
            .map(|entry| entry.expect("entry").file_name())
            .collect::<Vec<_>>();
        assert_eq!(names, [std::ffi::OsString::from("Home")]);
    }

    #[test]
    fn mutation_creates_only_required_ancestors_and_persistent_lock() {
        let (_parent, layout) = layout();
        replace_owned_file(&layout, "plugins/providers/host/auth.json", b"value").expect("replace");

        assert_eq!(
            fs::read(layout.provider_auth_json()).expect("target"),
            b"value"
        );
        let host = layout.provider_host_dir();
        let names = fs::read_dir(&host)
            .expect("host listing")
            .map(|entry| entry.expect("entry").file_name())
            .collect::<Vec<_>>();
        assert_eq!(names.len(), 2);
        assert!(names.iter().any(|name| name == "auth.json"));
        assert!(names.iter().any(|name| name == "auth.json.lock"));
        assert_eq!(
            fs::read_dir(layout.root()).expect("root listing").count(),
            1
        );
        assert_eq!(
            fs::read_dir(layout.plugins_dir())
                .expect("plugins listing")
                .count(),
            1
        );
    }

    #[test]
    fn root_file_mutation_does_not_create_eager_plugin_child() {
        let (_parent, layout) = layout();
        replace_owned_file(&layout, "config.json", b"value").expect("replace");

        assert!(layout.root().is_dir());
        assert!(layout.config_json().is_file());
        assert!(layout.root().join("config.json.lock").is_file());
        assert!(!layout.plugins_dir().exists());
        assert_eq!(
            fs::read_dir(layout.root()).expect("root listing").count(),
            2
        );
    }

    #[test]
    fn directories_target_and_lock_have_exact_private_access() {
        let (_parent, layout) = layout();
        replace_owned_file(&layout, "plugins/providers/host/auth.json", b"value").expect("replace");

        for directory in [
            layout.root().to_path_buf(),
            layout.plugins_dir(),
            layout.plugin_dir("providers").expect("provider directory"),
            layout.provider_host_dir(),
        ] {
            assert_private_evidence(&directory, OwnedKind::Directory);
        }
        assert_private_evidence(&layout.provider_auth_json(), OwnedKind::File);
        assert_private_evidence(
            &layout.provider_host_dir().join("auth.json.lock"),
            OwnedKind::File,
        );
    }

    #[test]
    fn explicit_directory_creation_stops_at_requested_component() {
        let (_parent, layout) = layout();
        ensure_owned_directory(&layout, "plugins/session/packs").expect("directory");
        assert!(layout.root().join("plugins/session/packs").is_dir());
        assert_eq!(
            fs::read_dir(layout.root().join("plugins/session/packs"))
                .expect("packs listing")
                .count(),
            0
        );
    }

    #[test]
    fn bounded_reads_reject_oversized_content() {
        let (_parent, layout) = layout();
        replace_owned_file(&layout, "config.json", b"12345").expect("replace");
        let error = read_owned_file(&layout, "config.json", 4).expect_err("oversized");
        assert_eq!(error.kind(), ConfigErrorKind::Oversized);
        assert_eq!(
            read_owned_file(&layout, "config.json", 5).expect("bounded read"),
            Some(b"12345".to_vec())
        );
    }

    #[test]
    fn permissive_target_and_lock_are_replaced_or_tightened() {
        let (_parent, layout) = layout();
        replace_owned_file(&layout, "config.json", b"old").expect("initial replace");
        platform::make_permissive_for_test(&layout.config_json());
        platform::make_permissive_for_test(&layout.root().join("config.json.lock"));
        let error = read_owned_file(&layout, "config.json", 64)
            .expect_err("permissive target read must fail closed");
        assert_eq!(error.kind(), ConfigErrorKind::AccessControl);

        replace_owned_file(&layout, "config.json", b"new").expect("private replacement");

        assert_eq!(fs::read(layout.config_json()).expect("target"), b"new");
        assert_private_evidence(&layout.config_json(), OwnedKind::File);
        assert_private_evidence(&layout.root().join("config.json.lock"), OwnedKind::File);
    }

    #[test]
    fn locked_update_rejects_permissive_target_without_change() {
        let (_parent, layout) = layout();
        replace_owned_file(&layout, "config.json", b"old").expect("initial replace");
        platform::make_permissive_for_test(&layout.config_json());

        let error = locked_update_owned_file(&layout, "config.json", 64, |_| Ok(b"new".to_vec()))
            .expect_err("permissive locked update must fail closed");

        assert_eq!(error.kind(), ConfigErrorKind::AccessControl);
        assert_eq!(fs::read(layout.config_json()).expect("target"), b"old");
    }

    #[test]
    fn wrong_types_and_wrong_case_aliases_are_rejected() {
        let (_parent, layout) = layout();
        ensure_owned_directory(&layout, "plugins/providers/host").expect("host directory");
        fs::create_dir(layout.provider_auth_json()).expect("wrong target type");
        let error = read_owned_file(&layout, "plugins/providers/host/auth.json", 64)
            .expect_err("directory target");
        assert_eq!(error.kind(), ConfigErrorKind::Io);
        fs::remove_dir(layout.provider_auth_json()).expect("remove wrong type");

        fs::write(layout.provider_host_dir().join("Auth.JSON"), b"alias")
            .expect("wrong-case alias");
        let error = read_owned_file(&layout, "plugins/providers/host/auth.json", 64)
            .expect_err("wrong-case alias");
        assert_eq!(error.kind(), ConfigErrorKind::AccessControl);
        let names = fs::read_dir(layout.provider_host_dir())
            .expect("host listing")
            .map(|entry| entry.expect("entry").file_name())
            .collect::<Vec<_>>();
        assert!(names.iter().any(|name| name == "Auth.JSON"));
        assert!(names.iter().all(|name| name != "auth.json"));

        fs::create_dir(layout.plugins_dir().join("Session")).expect("intermediate alias");
        let error = read_owned_file(&layout, "plugins/session/state", 64)
            .expect_err("wrong-case intermediate alias");
        assert_eq!(error.kind(), ConfigErrorKind::AccessControl);
    }

    #[cfg(unix)]
    #[test]
    fn unix_external_prefix_link_is_followed() {
        use std::os::unix::fs::symlink;

        let parent = tempfile::tempdir().expect("parent");
        let real = parent.path().join("real");
        fs::create_dir(&real).expect("real prefix");
        let linked = parent.path().join("linked");
        symlink(&real, &linked).expect("prefix link");
        let layout = HomeLayout::from_root(linked.join("home")).expect("layout");

        replace_owned_file(&layout, "config.json", b"value").expect("replace through prefix");

        assert_eq!(
            fs::read(real.join("home/config.json")).expect("target"),
            b"value"
        );
    }

    #[cfg(unix)]
    #[test]
    fn unix_intermediate_and_final_links_are_rejected() {
        use std::os::unix::fs::symlink;

        let (parent, layout) = layout();
        ensure_owned_directory(&layout, "plugins").expect("plugins");
        let outside = parent.path().join("outside");
        fs::create_dir(&outside).expect("outside");
        symlink(&outside, layout.plugins_dir().join("providers")).expect("intermediate link");
        let error = read_owned_file(&layout, "plugins/providers/auth.json", 64)
            .expect_err("intermediate link");
        assert_eq!(error.kind(), ConfigErrorKind::LinkEscape);
        fs::remove_file(layout.plugins_dir().join("providers")).expect("remove link");

        ensure_owned_directory(&layout, "plugins/providers/host").expect("host");
        let outside_file = outside.join("auth.json");
        fs::write(&outside_file, b"outside").expect("outside file");
        symlink(&outside_file, layout.provider_auth_json()).expect("final link");
        let error = read_owned_file(&layout, "plugins/providers/host/auth.json", 64)
            .expect_err("final link");
        assert_eq!(error.kind(), ConfigErrorKind::LinkEscape);
    }

    #[cfg(unix)]
    #[test]
    fn unix_foreign_owned_target_is_rejected_without_change() {
        use std::os::unix::fs::{MetadataExt, chown};

        if rustix::process::geteuid().as_raw() != 0 {
            eprintln!("skip: safe foreign-owner fixture requires euid 0");
            return;
        }
        let (_parent, layout) = layout();
        replace_owned_file(&layout, "config.json", b"old").expect("initial replace");
        chown(layout.config_json(), Some(65_534), None).expect("foreign owner fixture");

        let error = replace_owned_file(&layout, "config.json", b"new")
            .expect_err("foreign owner must fail");

        assert_eq!(error.kind(), ConfigErrorKind::AccessControl);
        assert_eq!(
            fs::read(layout.config_json()).expect("preserved target"),
            b"old"
        );
        assert_eq!(
            fs::metadata(layout.config_json())
                .expect("target metadata")
                .uid(),
            65_534
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_external_prefix_junction_is_followed() {
        let parent = tempfile::tempdir().expect("parent");
        let real = parent.path().join("real");
        fs::create_dir(&real).expect("real prefix");
        let linked = parent.path().join("linked");
        junction::create(&real, &linked).expect("prefix junction");
        let layout = HomeLayout::from_root(linked.join("home")).expect("layout");

        replace_owned_file(&layout, "config.json", b"value").expect("replace through prefix");

        assert_eq!(
            fs::read(real.join("home/config.json")).expect("target"),
            b"value"
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_intermediate_and_final_reparse_points_are_rejected() {
        let (parent, layout) = layout();
        ensure_owned_directory(&layout, "plugins").expect("plugins");
        let outside = parent.path().join("outside");
        fs::create_dir(&outside).expect("outside");
        let providers = layout.plugins_dir().join("providers");
        junction::create(&outside, &providers).expect("intermediate junction");
        let error = read_owned_file(&layout, "plugins/providers/auth.json", 64)
            .expect_err("intermediate reparse");
        assert_eq!(error.kind(), ConfigErrorKind::LinkEscape);
        junction::delete(&providers).expect("remove junction reparse data");
        fs::remove_dir(&providers).expect("remove junction fixture directory");

        ensure_owned_directory(&layout, "plugins/providers/host").expect("host");
        junction::create(&outside, layout.provider_auth_json()).expect("final junction");
        let error = read_owned_file(&layout, "plugins/providers/host/auth.json", 64)
            .expect_err("final reparse");
        assert_eq!(error.kind(), ConfigErrorKind::LinkEscape);
    }

    #[test]
    fn concurrent_locked_updates_do_not_lose_changes() {
        let (_parent, layout) = layout();
        let layout = Arc::new(layout);
        let barrier = Arc::new(Barrier::new(9));
        let threads = (0..8)
            .map(|_| {
                let layout = Arc::clone(&layout);
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    for _ in 0..16 {
                        locked_update_owned_file(&layout, "counter", 32, |current| {
                            let value = current
                                .map(|bytes| {
                                    std::str::from_utf8(bytes)
                                        .expect("UTF-8 counter")
                                        .parse::<u64>()
                                        .expect("counter")
                                })
                                .unwrap_or(0);
                            Ok((value + 1).to_string().into_bytes())
                        })
                        .expect("locked update");
                    }
                })
            })
            .collect::<Vec<_>>();
        barrier.wait();
        for thread in threads {
            thread.join().expect("update thread");
        }
        assert_eq!(
            read_owned_file(&layout, "counter", 32).expect("final read"),
            Some(b"128".to_vec())
        );
    }

    #[test]
    fn advisory_lock_excludes_a_cooperating_process() {
        let (_parent, layout) = layout();
        let path =
            OwnedPath::new(&layout, std::path::Path::new("config.json")).expect("owned path");
        let transaction = platform::Transaction::begin(&path.root, &path.components)
            .expect("parent transaction lock");
        let status = Command::new(std::env::current_exe().expect("test executable"))
            .arg("secure_fs::owned_file::tests::cross_process_lock_helper")
            .arg("--exact")
            .env(
                "MCODE_CONFIG_LOCK_TEST_PATH",
                layout.root().join("config.json.lock"),
            )
            .status()
            .expect("child test process");
        drop(transaction);
        assert!(status.success(), "child lock probe failed: {status}");
    }

    #[test]
    fn cross_process_lock_helper() {
        let Some(path) = std::env::var_os("MCODE_CONFIG_LOCK_TEST_PATH") else {
            return;
        };
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .expect("open persistent lock");
        match file.try_lock() {
            Err(fs::TryLockError::WouldBlock) => {}
            Ok(()) => panic!("cross-process lock was not held"),
            Err(fs::TryLockError::Error(error)) => {
                panic!("cross-process lock probe failed: {error}")
            }
        }
    }

    #[test]
    fn callback_failure_preserves_target_without_temporary_file() {
        let (_parent, layout) = layout();
        replace_owned_file(&layout, "config.json", b"old").expect("initial replace");

        let error = locked_update_owned_file(&layout, "config.json", 64, |_| {
            Err(ConfigError::new(ConfigErrorKind::DomainValidation))
        })
        .expect_err("callback failure");

        assert_eq!(error.kind(), ConfigErrorKind::DomainValidation);
        assert_eq!(fs::read(layout.config_json()).expect("target"), b"old");
        assert!(
            fs::read_dir(layout.root())
                .expect("root listing")
                .map(|entry| entry.expect("entry").file_name())
                .all(|name| !name.to_string_lossy().ends_with(".tmp"))
        );
    }

    #[test]
    fn injected_pre_rename_failure_preserves_target_and_cleans_temp() {
        let (_parent, layout) = layout();
        replace_owned_file(&layout, "config.json", b"old").expect("initial replace");
        platform::fail_before_rename_for_test();

        let error = replace_owned_file(&layout, "config.json", b"new")
            .expect_err("injected pre-rename failure");

        assert_eq!(error.kind(), ConfigErrorKind::AtomicReplace);
        assert_eq!(fs::read(layout.config_json()).expect("target"), b"old");
        let names = fs::read_dir(layout.root())
            .expect("root listing")
            .map(|entry| entry.expect("entry").file_name())
            .collect::<Vec<_>>();
        assert!(
            names
                .iter()
                .all(|name| !name.to_string_lossy().ends_with(".tmp")),
            "temporary file leaked: {names:?}"
        );
    }

    #[test]
    fn injected_parent_durability_failure_is_propagated() {
        let (_parent, layout) = layout();
        platform::fail_parent_barrier_for_test();
        let error = replace_owned_file(&layout, "config.json", b"new")
            .expect_err("parent durability failure");
        assert_eq!(error.kind(), ConfigErrorKind::Io);
    }

    fn assert_private_evidence(path: &std::path::Path, kind: OwnedKind) {
        #[cfg(unix)]
        assert_eq!(
            probe_access_control(path),
            AccessControlEvidence::UnixMode {
                kind,
                mode: if kind == OwnedKind::Directory {
                    0o700
                } else {
                    0o600
                },
            }
        );
        #[cfg(windows)]
        assert!(matches!(
            probe_access_control(path),
            AccessControlEvidence::WindowsProtectedDacl {
                kind: actual_kind,
                owner_allowed: true,
                owner_current_user: true,
                current_user: true,
                system: true,
                protected: true,
                ace_count: 1 | 2,
                extra_aces: 0,
                ..
            } if actual_kind == kind
        ));
    }
}

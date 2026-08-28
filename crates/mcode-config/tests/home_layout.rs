// Rust guideline compliant 2026-08-26

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use mcode_config::{
    ConfigErrorKind, HomeEnv, HomeLayout, MCODE_DIR_NAME, MCODE_HOME_ENV, PackFamily,
};

#[test]
fn frozen_hierarchy_is_exact() {
    let root = absolute_dummy_path("hierarchy");
    let layout = HomeLayout::from_root(&root).expect("valid root");

    assert_eq!(layout.root(), root);
    assert_eq!(layout.config_json(), root.join("config.json"));
    assert_eq!(layout.plugins_json(), root.join("plugins.json"));
    assert_eq!(layout.plugins_dir(), root.join("plugins"));
    assert_eq!(layout.provider_plugins_dir(), root.join("provider_plugins"));
    assert_eq!(
        layout.provider_auth_json(),
        root.join("provider_plugins").join("auth.json")
    );

    let manager = root.join("plugins").join("com.mcode.providers");
    assert_eq!(
        layout.manager_dir("com.mcode.providers").expect("manager"),
        manager
    );
    assert_eq!(
        layout
            .manager_config_json("com.mcode.providers")
            .expect("manager config"),
        manager.join("config.json")
    );
    assert_eq!(
        layout
            .manager_installation_json("com.mcode.providers")
            .expect("manager installation"),
        manager.join("installation.json")
    );
    assert_eq!(
        layout
            .manager_data_dir("com.mcode.providers")
            .expect("manager data"),
        manager.join("data")
    );
    assert_eq!(
        layout
            .manager_versions_dir("com.mcode.providers")
            .expect("manager versions"),
        manager.join("versions")
    );

    let families = [
        (PackFamily::Provider, "provider_plugins"),
        (PackFamily::Session, "session_plugins"),
        (PackFamily::Compaction, "compaction_plugins"),
        (PackFamily::Resource, "resource_plugins"),
        (PackFamily::Ask, "ask_plugins"),
        (PackFamily::Todo, "todo_plugins"),
        (PackFamily::Web, "web_plugins"),
        (PackFamily::Mcp, "mcp_plugins"),
        (PackFamily::Usage, "usage_plugins"),
        (PackFamily::Subagent, "subagent_plugins"),
        (PackFamily::Workspace, "workspace_plugins"),
        (PackFamily::Ui, "ui_plugins"),
    ];
    for (family, directory_name) in families {
        let family_root = root.join(directory_name);
        let pack = family_root.join("pack.example");
        assert_eq!(layout.pack_family_dir(family), family_root);
        assert_eq!(layout.pack_dir(family, "pack.example").expect("pack"), pack);
        assert_eq!(
            layout
                .pack_installation_json(family, "pack.example")
                .expect("pack installation"),
            pack.join("installation.json")
        );
        assert_eq!(
            layout
                .pack_data_dir(family, "pack.example")
                .expect("pack data"),
            pack.join("data")
        );
        assert_eq!(
            layout
                .pack_versions_dir(family, "pack.example")
                .expect("pack versions"),
            pack.join("versions")
        );
    }

    assert_eq!(layout.staging_dir(), root.join(".staging"));
    assert_eq!(
        layout
            .transaction_staging_dir("transaction-1")
            .expect("transaction"),
        root.join(".staging").join("transaction-1")
    );
}

#[test]
fn path_construction_creates_nothing() {
    let root = absolute_dummy_path("must-not-exist").join("nested-owned-home");
    assert!(!root.exists(), "dummy root must start absent");

    let layout = HomeLayout::from_root(&root).expect("valid root");
    let _ = layout.config_json();
    let _ = layout.plugins_json();
    let _ = layout.plugins_dir();
    let _ = layout.provider_plugins_dir();
    let _ = layout.provider_auth_json();
    let _ = layout.manager_dir("manager.example").expect("manager");
    let _ = layout
        .pack_dir(PackFamily::Session, "pack.example")
        .expect("pack");
    let _ = layout
        .transaction_staging_dir("transaction-1")
        .expect("transaction");
    let _ = layout.owned_join("controlled/relative/path").expect("join");

    assert!(!root.exists(), "path construction must not create the root");
}

#[test]
fn environment_precedence_is_fail_closed() {
    let override_root = absolute_dummy_path("override");
    let user_home = absolute_dummy_path("user-home");
    let profile = absolute_dummy_path("profile");

    let layout = HomeLayout::from_env(HomeEnv {
        mcode_home: Some(override_root.clone().into_os_string()),
        home: Some(user_home.clone().into_os_string()),
        user_profile: Some(profile.clone().into_os_string()),
    })
    .expect("override");
    assert_eq!(layout.root(), override_root);

    let error = HomeLayout::from_env(HomeEnv {
        mcode_home: Some(OsString::from("relative-override")),
        home: Some(user_home.clone().into_os_string()),
        user_profile: Some(profile.clone().into_os_string()),
    })
    .expect_err("invalid override must not fall back");
    assert_eq!(error.kind(), ConfigErrorKind::InvalidHome);
    assert_eq!(error.path(), Some(Path::new("relative-override")));

    let from_home = HomeLayout::from_env(HomeEnv {
        mcode_home: Some(OsString::new()),
        home: Some(user_home.clone().into_os_string()),
        user_profile: Some(profile.clone().into_os_string()),
    })
    .expect("empty override");
    assert_eq!(from_home.root(), user_home.join(MCODE_DIR_NAME));

    assert_eq!(MCODE_HOME_ENV, "MCODE_HOME");
    assert_eq!(MCODE_DIR_NAME, ".mcode");
}

#[cfg(windows)]
#[test]
fn windows_userprofile_is_only_the_last_nonempty_choice() {
    let profile = absolute_dummy_path("profile");

    let from_profile = HomeLayout::from_env(HomeEnv {
        mcode_home: None,
        home: Some(OsString::new()),
        user_profile: Some(profile.clone().into_os_string()),
    })
    .expect("profile fallback");
    assert_eq!(from_profile.root(), profile.join(MCODE_DIR_NAME));

    let error = HomeLayout::from_env(HomeEnv {
        mcode_home: None,
        home: Some(OsString::from("relative-home")),
        user_profile: Some(profile.into_os_string()),
    })
    .expect_err("invalid HOME must not fall back");
    assert_eq!(error.kind(), ConfigErrorKind::InvalidHome);

    let error = HomeLayout::from_env(HomeEnv {
        mcode_home: None,
        home: None,
        user_profile: Some(OsString::from("relative-profile")),
    })
    .expect_err("invalid profile");
    assert_eq!(error.kind(), ConfigErrorKind::InvalidHome);
}

#[cfg(not(windows))]
#[test]
fn non_windows_ignores_userprofile() {
    let error = HomeLayout::from_env(HomeEnv {
        mcode_home: None,
        home: Some(OsString::new()),
        user_profile: Some(absolute_dummy_path("profile").into_os_string()),
    })
    .expect_err("profile is Windows-only");
    assert_eq!(error.kind(), ConfigErrorKind::InvalidHome);
    assert!(error.path().is_none());
}

#[test]
fn missing_environment_values_are_invalid() {
    let error = HomeLayout::from_env(HomeEnv::default()).expect_err("missing home");
    assert_eq!(error.kind(), ConfigErrorKind::InvalidHome);
    assert!(error.path().is_none());
}

#[test]
fn process_environment_snapshot_matches_platform_contract() {
    let env = HomeEnv::from_process();
    assert_eq!(env.mcode_home, std::env::var_os(MCODE_HOME_ENV));
    assert_eq!(env.home, std::env::var_os("HOME"));
    #[cfg(windows)]
    assert_eq!(env.user_profile, std::env::var_os("USERPROFILE"));
    #[cfg(not(windows))]
    assert!(env.user_profile.is_none());
}

#[test]
fn roots_are_absolute_normalized_and_cwd_independent() {
    for root in [
        PathBuf::from("relative"),
        PathBuf::from("."),
        PathBuf::from(".."),
        absolute_dummy_path("a").join("..").join("b"),
    ] {
        let error = HomeLayout::from_root(&root).expect_err("invalid root");
        assert_eq!(error.kind(), ConfigErrorKind::InvalidHome, "{root:?}");
    }

    let root = absolute_dummy_path("cwd-stable");
    let layout = HomeLayout::from_root(&root).expect("absolute root");
    let expected_config = root.join("config.json");
    let original = std::env::current_dir().expect("current directory");
    let alternate = original.parent().expect("current directory has a parent");
    let _guard = CurrentDirGuard::enter(alternate);
    assert_eq!(layout.root(), root);
    assert_eq!(layout.config_json(), expected_config);
}

#[cfg(windows)]
#[test]
fn windows_roots_accept_only_safe_drive_unc_and_verbatim_forms() {
    for root in [
        PathBuf::from(r"C:\mcode\home"),
        PathBuf::from(r"\\server\share\mcode\home"),
        PathBuf::from(r"\\?\C:\mcode\home"),
        PathBuf::from(r"\\?\UNC\server\share\mcode\home"),
    ] {
        assert_eq!(
            HomeLayout::from_root(&root)
                .expect("valid Windows root")
                .root(),
            root
        );
    }

    let dotted = HomeLayout::from_root(r"C:\mcode\.\home").expect("normal dot");
    assert_eq!(dotted.root(), Path::new(r"C:\mcode\home"));

    for root in [
        PathBuf::from(r"C:\"),
        PathBuf::from(r"\\server\share\"),
        PathBuf::from(r"\\?\C:\"),
        PathBuf::from(r"\\?\UNC\server\share\"),
        PathBuf::from(r"C:relative"),
        PathBuf::from(r"\root-relative"),
        PathBuf::from("/root-relative"),
        PathBuf::from(r"\\server"),
        PathBuf::from(r"\\.\C:\mcode\home"),
        PathBuf::from(r"\\?\GLOBALROOT\Device\HarddiskVolume1\home"),
        PathBuf::from(r"C:\mcode\..\home"),
        PathBuf::from(r"\\?\C:\mcode\.\home"),
        PathBuf::from(r"C:\mcode\name."),
        PathBuf::from("C:\\mcode\\name "),
        PathBuf::from(r"C:\mcode\foo:bar"),
        PathBuf::from(r"C:\mcode\na*me"),
        PathBuf::from("C:\\mcode\\nul\0name"),
        PathBuf::from(r"C:\mcode\CON"),
        PathBuf::from(r"C:\mcode\NuL.json"),
        PathBuf::from(r"C:\mcode\com1.cache"),
        PathBuf::from("C:\\mcode\\COM\u{00B9}"),
        PathBuf::from(r"C:\mcode\CLOCK$"),
        PathBuf::from(r"C:\mcode\clock$.txt"),
        PathBuf::from(r"\\server.\share\home"),
        PathBuf::from(r"\\server\share.\home"),
        PathBuf::from(r"\\con\share\home"),
    ] {
        let error = HomeLayout::from_root(&root).expect_err("unsafe Windows root");
        assert_eq!(error.kind(), ConfigErrorKind::InvalidHome, "{root:?}");
    }
}

#[cfg(not(windows))]
#[test]
fn unix_filesystem_root_is_not_an_owned_home() {
    let error = HomeLayout::from_root("/").expect_err("filesystem root");
    assert_eq!(error.kind(), ConfigErrorKind::InvalidHome);

    let dotted = HomeLayout::from_root("/mcode/./home").expect("normalized dot");
    assert_eq!(dotted.root(), Path::new("/mcode/home"));
}

#[test]
fn portable_ids_enforce_all_boundaries() {
    let layout = HomeLayout::from_root(absolute_dummy_path("ids")).expect("layout");
    let maximum = format!("a{}", "b".repeat(127));
    assert_eq!(maximum.len(), 128);

    for valid in [
        "a",
        "a0",
        "a.b_c-d9",
        "auth.json",
        "com.mcode.providers",
        &maximum,
    ] {
        assert!(layout.manager_dir(valid).is_ok(), "rejected {valid:?}");
        assert!(
            layout.pack_dir(PackFamily::Web, valid).is_ok(),
            "rejected {valid:?}"
        );
        assert!(
            layout.transaction_staging_dir(valid).is_ok(),
            "rejected {valid:?}"
        );
    }

    let too_long = format!("a{}", "b".repeat(128));
    for invalid in [
        "",
        "0abc",
        "Aabc",
        "abc.",
        "abc-",
        "abc_",
        ".abc",
        "a/b",
        "a\\b",
        "a:b",
        "a*b",
        "a?b",
        "a\0b",
        "a\nb",
        "a b",
        "café",
        "con",
        "nul.json",
        "com1",
        "lpt9.cache",
        "com\u{00B9}",
        &too_long,
    ] {
        let manager_error = layout.manager_dir(invalid).expect_err("invalid manager ID");
        assert_eq!(
            manager_error.kind(),
            ConfigErrorKind::PathEscape,
            "{invalid:?}"
        );
        let pack_error = layout
            .pack_dir(PackFamily::Resource, invalid)
            .expect_err("invalid Pack ID");
        assert_eq!(
            pack_error.kind(),
            ConfigErrorKind::PathEscape,
            "{invalid:?}"
        );
        let transaction_error = layout
            .transaction_staging_dir(invalid)
            .expect_err("invalid transaction ID");
        assert_eq!(
            transaction_error.kind(),
            ConfigErrorKind::PathEscape,
            "{invalid:?}"
        );
    }

    let auth_store = layout
        .pack_dir(PackFamily::Provider, "auth.json")
        .expect_err("auth store is not a Pack ID");
    assert_eq!(auth_store.kind(), ConfigErrorKind::PathEscape);
    assert_eq!(
        layout.provider_auth_json(),
        absolute_dummy_path("ids")
            .join("provider_plugins")
            .join("auth.json")
    );
}

#[test]
fn owned_join_rejects_every_unsafe_component() {
    let root = absolute_dummy_path("owned-join");
    let layout = HomeLayout::from_root(&root).expect("layout");
    assert_eq!(
        layout
            .owned_join("controlled/relative/file.json")
            .expect("controlled path"),
        root.join("controlled").join("relative").join("file.json")
    );

    for invalid in [
        "",
        ".",
        "..",
        "a/./b",
        "a/../b",
        "/absolute",
        r"C:\absolute",
        r"\\server\share\path",
        "a//b",
        "a\\\\b",
        "a/",
        "a\\",
        "name.",
        "name ",
        "name:stream",
        "na*me",
        "na?me",
        "na\0me",
        "na\nme",
        "CON",
        "NuL.json",
        "com1.cache",
        "COM\u{00B9}",
        "CLOCK$",
        "clock$.txt",
    ] {
        let error = layout.owned_join(invalid).expect_err("unsafe join");
        assert_eq!(error.kind(), ConfigErrorKind::PathEscape, "{invalid:?}");
        assert!(error.path().is_none());
    }
}

#[test]
fn path_error_display_is_value_free() {
    let value = "relative-private-path";
    let error = HomeLayout::from_root(value).expect_err("relative root");
    assert_eq!(error.path(), Some(Path::new(value)));
    assert!(!error.to_string().contains(value));

    let escape = HomeLayout::from_root(absolute_dummy_path("display"))
        .expect("layout")
        .manager_dir("private/value")
        .expect_err("invalid ID");
    assert!(!escape.to_string().contains("private"));
}

fn absolute_dummy_path(name: &str) -> PathBuf {
    #[cfg(windows)]
    {
        PathBuf::from(r"C:\mcode-home-layout-dummy").join(name)
    }
    #[cfg(not(windows))]
    {
        PathBuf::from("/mcode-home-layout-dummy").join(name)
    }
}

struct CurrentDirGuard {
    original: PathBuf,
}

impl CurrentDirGuard {
    fn enter(path: &Path) -> Self {
        let original = std::env::current_dir().expect("current directory");
        std::env::set_current_dir(path).expect("change current directory");
        Self { original }
    }
}

impl Drop for CurrentDirGuard {
    fn drop(&mut self) {
        std::env::set_current_dir(&self.original).expect("restore current directory");
    }
}

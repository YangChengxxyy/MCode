//! Bounded native writer for Host staging transactions.
//!
//! The writer establishes only durable, private, same-volume mechanical state.
//! It does not recover abandoned transactions or verify bundle trust, signatures,
//! digests, inventory completeness, installation, or activation.

// Rust guideline compliant 2026-08-29

use std::collections::{BTreeMap, BTreeSet};

use crate::{BundlePath, ConfigError, ConfigErrorKind, HomeLayout, TransactionId};

use crate::secure_fs::staging_platform as platform;

/// Maximum encoded staging journal size: 1 KiB.
pub const MAX_STAGING_JOURNAL_BYTES: usize = 1024;
/// Maximum number of direct entries in `.staging/`.
pub const MAX_STAGING_ROOT_ENTRIES: usize = 1024;
/// Maximum number of payload regular files.
pub const MAX_STAGING_FILES: usize = 4096;
/// Maximum number of structural payload directories.
pub const MAX_STAGING_DIRECTORIES: usize = 4096;
/// Maximum combined number of payload files and directories.
pub const MAX_STAGING_ENTRIES: usize = 8192;
/// Maximum byte length of one payload file: 256 MiB.
pub const MAX_STAGING_FILE_BYTES: u64 = 256 * 1024 * 1024;
/// Maximum logical byte length of one payload: 512 MiB.
pub const MAX_STAGING_TOTAL_BYTES: u64 = 512 * 1024 * 1024;

const STAGING_JOURNAL_KIND: &str = "mcode-staging-transaction";

pub(crate) struct WriteFailure {
    pub(crate) error: ConfigError,
    pub(crate) mutation_started: bool,
}

#[derive(Clone, Copy)]
struct LedgerLimits {
    files: usize,
    directories: usize,
    entries: usize,
    file_bytes: u64,
    total_bytes: u64,
}

const PUBLIC_LIMITS: LedgerLimits = LedgerLimits {
    files: MAX_STAGING_FILES,
    directories: MAX_STAGING_DIRECTORIES,
    entries: MAX_STAGING_ENTRIES,
    file_bytes: MAX_STAGING_FILE_BYTES,
    total_bytes: MAX_STAGING_TOTAL_BYTES,
};

#[derive(Default)]
struct PayloadLedger {
    files: BTreeMap<String, u64>,
    directories: BTreeSet<String>,
    total_bytes: u64,
}

#[derive(Debug)]
struct PlannedWrite {
    path: String,
    new_directories: Vec<String>,
    size: u64,
}

impl PayloadLedger {
    fn plan(
        &self,
        path: &BundlePath,
        size: usize,
        limits: LedgerLimits,
    ) -> Result<PlannedWrite, ConfigError> {
        let size = u64::try_from(size).map_err(|_| oversized())?;
        if size > limits.file_bytes {
            return Err(oversized());
        }
        let path = path.as_str();
        if self.files.contains_key(path) || self.directories.contains(path) {
            return Err(validation_error());
        }

        let components = path.split('/').collect::<Vec<_>>();
        let mut new_directories = Vec::new();
        let mut prefix = String::new();
        for component in &components[..components.len().saturating_sub(1)] {
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(component);
            if self.files.contains_key(&prefix) {
                return Err(validation_error());
            }
            if !self.directories.contains(&prefix) {
                new_directories.push(prefix.clone());
            }
        }
        let descendant_prefix = format!("{path}/");
        if self
            .files
            .range(descendant_prefix.clone()..)
            .next()
            .is_some_and(|(entry, _)| entry.starts_with(&descendant_prefix))
            || self
                .directories
                .range(descendant_prefix.clone()..)
                .next()
                .is_some_and(|entry| entry.starts_with(&descendant_prefix))
        {
            return Err(validation_error());
        }

        let file_count = self.files.len().checked_add(1).ok_or_else(oversized)?;
        let directory_count = self
            .directories
            .len()
            .checked_add(new_directories.len())
            .ok_or_else(oversized)?;
        let entry_count = file_count
            .checked_add(directory_count)
            .ok_or_else(oversized)?;
        let total_bytes = self.total_bytes.checked_add(size).ok_or_else(oversized)?;
        if file_count > limits.files
            || directory_count > limits.directories
            || entry_count > limits.entries
            || total_bytes > limits.total_bytes
        {
            return Err(oversized());
        }
        Ok(PlannedWrite {
            path: path.to_owned(),
            new_directories,
            size,
        })
    }

    fn apply(&mut self, plan: PlannedWrite) {
        self.files.insert(plan.path, plan.size);
        self.directories.extend(plan.new_directories);
        self.total_bytes += plan.size;
    }
}

/// Holds an exclusively locked transaction while payload files are written.
#[must_use = "dropping the writer abandons its transaction and releases the transaction lock"]
pub struct StagingTransaction {
    id: TransactionId,
    native: platform::Transaction,
    ledger: PayloadLedger,
    poisoned: bool,
}

impl StagingTransaction {
    /// Returns the Host-generated transaction identifier.
    #[must_use]
    pub fn id(&self) -> &TransactionId {
        &self.id
    }

    /// Exclusively creates and durably writes one canonical payload file.
    ///
    /// Lexical and ledger failures happen before native mutation and may be
    /// retried. A failure after native payload mutation poisons this writer.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigErrorKind::Oversized`] for fixed count or byte bounds,
    /// [`ConfigErrorKind::AuthorityValidation`] for duplicate and prefix
    /// conflicts, and native security, lock, or I/O failures otherwise.
    pub fn write_file(&mut self, path: &BundlePath, bytes: &[u8]) -> Result<(), ConfigError> {
        if self.poisoned {
            return Err(validation_error());
        }
        let plan = self.ledger.plan(path, bytes.len(), PUBLIC_LIMITS)?;
        match self
            .native
            .write_file(&plan.path, &plan.new_directories, bytes, plan.size)
        {
            Ok(()) => {
                self.ledger.apply(plan);
                Ok(())
            }
            Err(failure) => {
                self.poisoned |= failure.mutation_started;
                Err(failure.error)
            }
        }
    }

    /// Publishes `staged` after revalidating the complete written payload.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigErrorKind::AuthorityValidation`] for an empty or
    /// poisoned writer and [`ConfigError`] for failed identity, access,
    /// journal, or durability validation.
    pub fn finish(mut self) -> Result<StagedTransaction, ConfigError> {
        if self.poisoned || self.ledger.files.is_empty() {
            return Err(validation_error());
        }
        let writing = journal_bytes(&self.id, "writing")?;
        let staged = journal_bytes(&self.id, "staged")?;
        if let Err(error) = self.native.finish(
            &self.ledger.files,
            &self.ledger.directories,
            self.ledger.total_bytes,
            &writing,
            &staged,
        ) {
            self.poisoned = true;
            return Err(error);
        }
        Ok(StagedTransaction {
            id: self.id,
            _native: self.native,
        })
    }
}

/// Retains the exclusive transaction lock for a proven staged payload.
#[must_use = "dropping the staged guard releases the transaction lock before a durable claim"]
pub struct StagedTransaction {
    id: TransactionId,
    _native: platform::Transaction,
}

impl StagedTransaction {
    /// Returns the Host-generated transaction identifier.
    #[must_use]
    pub fn id(&self) -> &TransactionId {
        &self.id
    }
}

/// Begins one lazy native staging transaction under the global lock.
///
/// The operation verifies the retained home and plugins anchors, creates only
/// staging protocol objects, enforces the bounded direct-root capacity, and
/// publishes `writing` before releasing the global lock.
///
/// # Errors
///
/// Returns [`ConfigError`] for missing or invalid owned anchors, staging-root
/// capacity, random generation, lock, access, identity, or durability failure.
pub fn begin_staging(home: &HomeLayout) -> Result<StagingTransaction, ConfigError> {
    let (id, native) = platform::Transaction::begin(home, |id| journal_bytes(id, "writing"))?;
    Ok(StagingTransaction {
        id,
        native,
        ledger: PayloadLedger::default(),
        poisoned: false,
    })
}

fn journal_bytes(id: &TransactionId, state: &str) -> Result<Vec<u8>, ConfigError> {
    let bytes = format!(
        "{{\"formatVersion\":1,\"kind\":\"{STAGING_JOURNAL_KIND}\",\"transactionId\":\"{}\",\"state\":\"{state}\"}}\n",
        id.as_str()
    )
    .into_bytes();
    if bytes.len() > MAX_STAGING_JOURNAL_BYTES {
        return Err(ConfigError::new(ConfigErrorKind::Serialization));
    }
    Ok(bytes)
}

fn validation_error() -> ConfigError {
    ConfigError::new(ConfigErrorKind::AuthorityValidation)
}

fn oversized() -> ConfigError {
    ConfigError::new(ConfigErrorKind::Oversized)
}

#[cfg(test)]
mod tests {
    use super::{LedgerLimits, PayloadLedger};
    use crate::{BundlePath, ConfigErrorKind, HomeLayout, ensure_home_layout};

    const SMALL: LedgerLimits = LedgerLimits {
        files: 2,
        directories: 2,
        entries: 4,
        file_bytes: 4,
        total_bytes: 6,
    };

    fn path(value: &str) -> BundlePath {
        BundlePath::parse(value).expect("bundle path")
    }

    fn add(ledger: &mut PayloadLedger, value: &str, size: usize) {
        let plan = ledger.plan(&path(value), size, SMALL).expect("plan");
        ledger.apply(plan);
    }

    #[test]
    fn public_limits_are_frozen_and_wired_to_the_ledger() {
        assert_eq!(super::MAX_STAGING_JOURNAL_BYTES, 1_024);
        assert_eq!(super::MAX_STAGING_ROOT_ENTRIES, 1_024);
        assert_eq!(super::MAX_STAGING_FILES, 4_096);
        assert_eq!(super::MAX_STAGING_DIRECTORIES, 4_096);
        assert_eq!(super::MAX_STAGING_ENTRIES, 8_192);
        assert_eq!(super::MAX_STAGING_FILE_BYTES, 256 * 1_024 * 1_024);
        assert_eq!(super::MAX_STAGING_TOTAL_BYTES, 512 * 1_024 * 1_024);
        assert_eq!(super::PUBLIC_LIMITS.files, 4_096);
        assert_eq!(super::PUBLIC_LIMITS.directories, 4_096);
        assert_eq!(super::PUBLIC_LIMITS.entries, 8_192);
        assert_eq!(super::PUBLIC_LIMITS.file_bytes, 256 * 1_024 * 1_024);
        assert_eq!(super::PUBLIC_LIMITS.total_bytes, 512 * 1_024 * 1_024);
    }

    #[test]
    fn ledger_rejects_duplicates_and_both_prefix_conflict_directions() {
        let mut ledger = PayloadLedger::default();
        add(&mut ledger, "a/b", 1);
        for value in ["a/b", "a", "a/b/c"] {
            assert_eq!(
                ledger
                    .plan(&path(value), 1, SMALL)
                    .expect_err("conflict")
                    .kind(),
                ConfigErrorKind::AuthorityValidation
            );
        }
    }

    #[test]
    fn ledger_checks_each_independent_bound_without_large_allocations() {
        for (limits, existing, candidate, size, reason) in [
            (
                LedgerLimits {
                    file_bytes: 0,
                    ..SMALL
                },
                vec![],
                "a",
                1,
                "file bytes",
            ),
            (
                LedgerLimits {
                    total_bytes: 1,
                    ..SMALL
                },
                vec![("a", 1)],
                "b",
                1,
                "total bytes",
            ),
            (
                LedgerLimits { files: 1, ..SMALL },
                vec![("a", 0)],
                "b",
                0,
                "files",
            ),
            (
                LedgerLimits {
                    directories: 1,
                    ..SMALL
                },
                vec![],
                "a/b/c",
                0,
                "directories",
            ),
            (
                LedgerLimits {
                    entries: 1,
                    ..SMALL
                },
                vec![],
                "a/b",
                0,
                "combined entries",
            ),
        ] {
            let mut ledger = PayloadLedger::default();
            for (path, size) in existing {
                let plan = ledger
                    .plan(&self::path(path), size, SMALL)
                    .expect("fixture plan");
                ledger.apply(plan);
            }
            assert_eq!(
                ledger
                    .plan(&path(candidate), size, limits)
                    .expect_err(reason)
                    .kind(),
                ConfigErrorKind::Oversized,
                "{reason} bound"
            );
        }
    }

    #[test]
    fn ledger_accepts_exact_bounds_and_rejects_one_over() {
        for (label, exact_limits, exact_existing, exact_path, over_limits) in [
            (
                "file bytes",
                LedgerLimits {
                    file_bytes: 1,
                    ..SMALL
                },
                vec![],
                ("a", 1),
                LedgerLimits {
                    file_bytes: 0,
                    ..SMALL
                },
            ),
            (
                "total bytes",
                LedgerLimits {
                    total_bytes: 2,
                    ..SMALL
                },
                vec![("a", 1)],
                ("b", 1),
                LedgerLimits {
                    total_bytes: 1,
                    ..SMALL
                },
            ),
            (
                "file count",
                LedgerLimits { files: 2, ..SMALL },
                vec![("a", 0)],
                ("b", 0),
                LedgerLimits { files: 1, ..SMALL },
            ),
            (
                "directory count",
                LedgerLimits {
                    directories: 2,
                    ..SMALL
                },
                vec![],
                ("a/b/c", 0),
                LedgerLimits {
                    directories: 1,
                    ..SMALL
                },
            ),
            (
                "combined entries",
                LedgerLimits {
                    entries: 2,
                    ..SMALL
                },
                vec![],
                ("a/b", 0),
                LedgerLimits {
                    entries: 1,
                    ..SMALL
                },
            ),
        ] {
            let mut exact = PayloadLedger::default();
            let mut over = PayloadLedger::default();
            for (path, size) in exact_existing {
                let exact_plan = exact
                    .plan(&self::path(path), size, SMALL)
                    .expect("exact fixture");
                exact.apply(exact_plan);
                let over_plan = over
                    .plan(&self::path(path), size, SMALL)
                    .expect("over fixture");
                over.apply(over_plan);
            }
            assert!(
                exact
                    .plan(&path(exact_path.0), exact_path.1, exact_limits)
                    .is_ok(),
                "{label} exact"
            );
            assert_eq!(
                over.plan(&path(exact_path.0), exact_path.1, over_limits)
                    .expect_err(label)
                    .kind(),
                ConfigErrorKind::Oversized,
                "{label} one over"
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn finish_journal_prepare_failure_keeps_writing_and_cleans_temp() {
        let parent = tempfile::tempdir().expect("parent");
        let layout = HomeLayout::from_root(parent.path().join("home")).expect("layout");
        ensure_home_layout(&layout).expect("bootstrap");
        let mut writer = super::begin_staging(&layout).expect("begin");
        let transaction = layout.transaction_staging_dir(writer.id());
        writer.write_file(&path("file"), b"x").expect("write");
        super::platform::fail_next_journal_temp_prepare_for_test();
        assert_eq!(
            writer
                .finish()
                .err()
                .expect("injected finish failure")
                .kind(),
            ConfigErrorKind::Io
        );
        assert!(!transaction.join(".journal.json.tmp").exists());
        let bytes = std::fs::read(transaction.join("journal.json")).expect("writing journal");
        assert!(bytes.ends_with(b"\"state\":\"writing\"}\n"));
    }

    #[cfg(unix)]
    #[test]
    fn unix_journal_prepare_failure_retains_the_owned_temporary() {
        let parent = tempfile::tempdir().expect("parent");
        let layout = HomeLayout::from_root(parent.path().join("home")).expect("layout");
        ensure_home_layout(&layout).expect("bootstrap");
        let mut writer = super::begin_staging(&layout).expect("begin");
        let transaction = layout.transaction_staging_dir(writer.id());
        writer.write_file(&path("file"), b"x").expect("write");
        super::platform::fail_next_journal_temp_prepare_for_test();
        assert_eq!(
            writer
                .finish()
                .err()
                .expect("injected finish failure")
                .kind(),
            ConfigErrorKind::Io
        );
        assert!(transaction.join(".journal.json.tmp").exists());
        let bytes = std::fs::read(transaction.join("journal.json")).expect("writing journal");
        assert!(bytes.ends_with(b"\"state\":\"writing\"}\n"));
    }

    #[test]
    fn native_failure_after_creation_poisons_writer() {
        let parent = tempfile::tempdir().expect("parent");
        let layout = HomeLayout::from_root(parent.path().join("home")).expect("layout");
        ensure_home_layout(&layout).expect("bootstrap");
        let mut writer = super::begin_staging(&layout).expect("begin");
        super::platform::fail_next_payload_write_for_test();
        assert_eq!(
            writer
                .write_file(&path("file"), b"bytes")
                .expect_err("injected write failure")
                .kind(),
            ConfigErrorKind::Io
        );
        assert_eq!(
            writer
                .write_file(&path("retry"), b"bytes")
                .expect_err("poisoned retry")
                .kind(),
            ConfigErrorKind::AuthorityValidation
        );
        assert_eq!(
            writer.finish().err().expect("poisoned finish").kind(),
            ConfigErrorKind::AuthorityValidation
        );
    }

    #[test]
    fn payload_barrier_failure_is_propagated_and_poisons_writer() {
        let parent = tempfile::tempdir().expect("parent");
        let layout = HomeLayout::from_root(parent.path().join("home")).expect("layout");
        ensure_home_layout(&layout).expect("bootstrap");
        let mut writer = super::begin_staging(&layout).expect("begin");
        super::platform::fail_next_payload_barrier_for_test();
        assert_eq!(
            writer
                .write_file(&path("file"), b"bytes")
                .expect_err("injected barrier failure")
                .kind(),
            ConfigErrorKind::Io
        );
        assert_eq!(
            writer
                .write_file(&path("retry"), b"bytes")
                .expect_err("poisoned retry")
                .kind(),
            ConfigErrorKind::AuthorityValidation
        );
    }

    #[test]
    fn failed_validation_does_not_change_ledger() {
        let ledger = PayloadLedger::default();
        assert_eq!(
            ledger
                .plan(
                    &path("a/b/c"),
                    1,
                    LedgerLimits {
                        directories: 1,
                        ..SMALL
                    },
                )
                .expect_err("directory limit")
                .kind(),
            ConfigErrorKind::Oversized
        );
        assert!(ledger.files.is_empty());
        assert!(ledger.directories.is_empty());
        assert_eq!(ledger.total_bytes, 0);
    }
}

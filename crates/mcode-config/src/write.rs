//! Crash-resistant JSON envelope writes with advisory serialization.

// Rust guideline compliant 2026-08-26

use std::ffi::OsString;
use std::fmt::{self, Debug, Formatter};
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;
use uuid::Uuid;

use crate::parse::{CONFIG_FIELD, FORMAT_VERSION_FIELD};
use crate::security::{validate_envelope_value_limits, validate_patch_credentials};
use crate::{ConfigError, ConfigErrorKind, ConfigLimits, FORMAT_VERSION, ReloadCancellation};

/// Atomically writes a current-version JSON configuration envelope.
///
/// This applies default resource and credential-reference checks before any
/// filesystem mutation. It does not perform domain validation; callers should
/// write a value from a successfully validated snapshot or invoke their domain
/// validator first.
///
/// # Errors
///
/// Returns [`ConfigError`] for invalid limits/credentials, serialization, lock,
/// temporary-file, synchronization, or replacement failures. A failure before
/// replacement leaves an existing destination unchanged and removes the random
/// temporary file.
pub fn write_config_file(path: impl AsRef<Path>, value: &Value) -> Result<(), ConfigError> {
    write_config_file_with_limits(path, value, ConfigLimits::default())
}

/// Atomically writes a JSON envelope with explicit resource limits.
///
/// The temporary file is created with `create_new` in the destination's
/// directory. Unix files use mode `0600`; Windows inherits the directory ACL.
/// The file is flushed and `sync_data` completes before platform replacement.
/// Writers using this crate serialize through a persistent sidecar advisory
/// lock. Node limits include the complete serialized envelope, matching reads
/// performed with the same limits; depth still starts at the `config` value.
///
/// # Errors
///
/// Returns [`ConfigError`] under the same conditions as
/// [`write_config_file`], and when `limits` is internally inconsistent.
pub fn write_config_file_with_limits(
    path: impl AsRef<Path>,
    value: &Value,
    limits: ConfigLimits,
) -> Result<(), ConfigError> {
    let path = path.as_ref();
    if !limits.are_valid() {
        return Err(ConfigError::new(ConfigErrorKind::InvalidLimits));
    }
    let cancellation = ReloadCancellation::new();
    validate_envelope_value_limits(value, limits, &cancellation)?;
    validate_patch_credentials(value, None, &cancellation)?;
    let bytes = serialize_envelope(value, limits.max_source_bytes)?;
    atomic_write_bytes(path, &bytes)
}

#[derive(Serialize)]
struct Envelope<'a> {
    #[serde(rename = "formatVersion")]
    format_version: u32,
    config: &'a Value,
}

fn serialize_envelope(value: &Value, maximum: usize) -> Result<Vec<u8>, ConfigError> {
    debug_assert_eq!(FORMAT_VERSION_FIELD, "formatVersion");
    debug_assert_eq!(CONFIG_FIELD, "config");
    let envelope = Envelope {
        format_version: FORMAT_VERSION,
        config: value,
    };
    let mut output = BoundedBuffer::new(maximum);
    if serde_json::to_writer_pretty(&mut output, &envelope).is_err() {
        return Err(ConfigError::new(if output.oversized {
            ConfigErrorKind::Oversized
        } else {
            ConfigErrorKind::Serialization
        }));
    }
    output.write_all(b"\n").map_err(|_| {
        ConfigError::new(if output.oversized {
            ConfigErrorKind::Oversized
        } else {
            ConfigErrorKind::Serialization
        })
    })?;
    Ok(output.bytes)
}

struct BoundedBuffer {
    bytes: Vec<u8>,
    maximum: usize,
    oversized: bool,
}

impl BoundedBuffer {
    fn new(maximum: usize) -> Self {
        Self {
            bytes: Vec::new(),
            maximum,
            oversized: false,
        }
    }
}

impl Write for BoundedBuffer {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let next = self
            .bytes
            .len()
            .checked_add(buffer.len())
            .ok_or_else(|| io::Error::other("bounded configuration buffer overflow"))?;
        if next > self.maximum {
            self.oversized = true;
            return Err(io::Error::other("bounded configuration buffer exceeded"));
        }
        self.bytes.extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn atomic_write_bytes(path: &Path, bytes: &[u8]) -> Result<(), ConfigError> {
    atomic_write_bytes_with(path, bytes, replace_file)
}

fn atomic_write_bytes_with<F>(path: &Path, bytes: &[u8], replace: F) -> Result<(), ConfigError>
where
    F: FnOnce(&Path, &Path) -> io::Result<()>,
{
    let parent = parent_directory(path);
    std::fs::create_dir_all(parent).map_err(|error| {
        ConfigError::for_path(ConfigErrorKind::Io, parent).with_io_kind(error.kind())
    })?;
    let _lock = AdvisoryLock::acquire(path)?;
    let (file, temporary_path) = create_temporary(path, parent)?;
    let mut temporary = TemporaryFile::new(temporary_path);

    let mut file = file;
    file.write_all(bytes).map_err(|error| {
        ConfigError::for_path(ConfigErrorKind::Io, temporary.path()).with_io_kind(error.kind())
    })?;
    file.flush().map_err(|error| {
        ConfigError::for_path(ConfigErrorKind::Io, temporary.path()).with_io_kind(error.kind())
    })?;
    file.sync_data().map_err(|error| {
        ConfigError::for_path(ConfigErrorKind::Io, temporary.path()).with_io_kind(error.kind())
    })?;
    // Windows replacement requires the replacement file handle to be closed.
    drop(file);

    replace(temporary.path(), path).map_err(|error| {
        ConfigError::for_path(ConfigErrorKind::AtomicReplace, path).with_io_kind(error.kind())
    })?;
    temporary.disarm();
    Ok(())
}

fn parent_directory(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

fn create_temporary(path: &Path, parent: &Path) -> Result<(File, PathBuf), ConfigError> {
    let Some(file_name) = path.file_name() else {
        return Err(ConfigError::for_path(ConfigErrorKind::Io, path)
            .with_io_kind(io::ErrorKind::InvalidInput));
    };

    // Sixteen cryptographically random UUID names make collision exhaustion
    // practically impossible while still bounding work under hostile races.
    const MAX_TEMPORARY_ATTEMPTS: usize = 16;
    for _ in 0..MAX_TEMPORARY_ATTEMPTS {
        let mut temporary_name = OsString::from(".");
        temporary_name.push(file_name);
        temporary_name.push(format!(".{}.tmp", Uuid::new_v4().simple()));
        let temporary_path = parent.join(temporary_name);
        match open_private_new(&temporary_path) {
            Ok(file) => return Ok((file, temporary_path)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(ConfigError::for_path(ConfigErrorKind::Io, &temporary_path)
                    .with_io_kind(error.kind()));
            }
        }
    }
    Err(ConfigError::for_path(ConfigErrorKind::Io, path).with_io_kind(io::ErrorKind::AlreadyExists))
}

fn open_private_new(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

fn open_lock_file(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

struct TemporaryFile {
    path: PathBuf,
    armed: bool,
}

impl TemporaryFile {
    fn new(path: PathBuf) -> Self {
        Self { path, armed: true }
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for TemporaryFile {
    fn drop(&mut self) {
        if self.armed {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

struct AdvisoryLock {
    file: File,
    path: PathBuf,
}

impl AdvisoryLock {
    fn acquire(destination: &Path) -> Result<Self, ConfigError> {
        let path = lock_path(destination)?;
        let file = open_lock_file(&path).map_err(|error| {
            ConfigError::for_path(ConfigErrorKind::Lock, &path).with_io_kind(error.kind())
        })?;
        file.lock().map_err(|error| {
            ConfigError::for_path(ConfigErrorKind::Lock, &path).with_io_kind(error.kind())
        })?;
        Ok(Self { file, path })
    }

    #[cfg(test)]
    fn try_acquire(destination: &Path) -> Result<Option<Self>, ConfigError> {
        use std::fs::TryLockError;

        let path = lock_path(destination)?;
        let file = open_lock_file(&path).map_err(|error| {
            ConfigError::for_path(ConfigErrorKind::Lock, &path).with_io_kind(error.kind())
        })?;
        match file.try_lock() {
            Ok(()) => Ok(Some(Self { file, path })),
            Err(TryLockError::WouldBlock) => Ok(None),
            Err(TryLockError::Error(error)) => {
                Err(ConfigError::for_path(ConfigErrorKind::Lock, &path).with_io_kind(error.kind()))
            }
        }
    }
}

impl Debug for AdvisoryLock {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AdvisoryLock")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl Drop for AdvisoryLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

fn lock_path(destination: &Path) -> Result<PathBuf, ConfigError> {
    let Some(file_name) = destination.file_name() else {
        return Err(ConfigError::for_path(ConfigErrorKind::Lock, destination)
            .with_io_kind(io::ErrorKind::InvalidInput));
    };
    let mut lock_name = file_name.to_os_string();
    lock_name.push(".lock");
    Ok(parent_directory(destination).join(lock_name))
}

#[cfg(not(windows))]
fn replace_file(temporary: &Path, destination: &Path) -> io::Result<()> {
    std::fs::rename(temporary, destination)
}

#[cfg(windows)]
fn replace_file(temporary: &Path, destination: &Path) -> io::Result<()> {
    use std::ptr;

    use windows_sys::Win32::Storage::FileSystem::{REPLACEFILE_WRITE_THROUGH, ReplaceFileW};

    if destination.try_exists()? {
        let destination_wide = wide_path(destination)?;
        let temporary_wide = wide_path(temporary)?;
        // SAFETY: Both path buffers are explicitly NUL-terminated UTF-16 and
        // remain alive for the call. Optional pointers are null as documented.
        // ReplaceFileW does not retain any pointer after returning.
        let replaced = unsafe {
            ReplaceFileW(
                destination_wide.as_ptr(),
                temporary_wide.as_ptr(),
                ptr::null(),
                REPLACEFILE_WRITE_THROUGH,
                ptr::null(),
                ptr::null(),
            )
        };
        if replaced != 0 {
            return Ok(());
        }
        // ReplaceFileW documents GetLastError for its zero return value. Capture
        // it before any other OS call can overwrite the thread's error state.
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::NotFound {
            return Err(error);
        }
    }
    std::fs::rename(temporary, destination)
}

#[cfg(windows)]
fn wide_path(path: &Path) -> io::Result<Vec<u16>> {
    use std::os::windows::ffi::OsStrExt;

    let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    if wide.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Windows path contains an interior NUL",
        ));
    }
    wide.push(0);
    Ok(wide)
}

#[cfg(test)]
mod tests {
    use super::{AdvisoryLock, atomic_write_bytes_with};
    use std::io;

    #[test]
    fn advisory_lock_excludes_a_second_writer() {
        let directory = tempfile::tempdir().expect("temp directory");
        let destination = directory.path().join("settings.json");
        let first = AdvisoryLock::acquire(&destination).expect("first lock");
        let second = AdvisoryLock::try_acquire(&destination).expect("second lock attempt");
        assert!(second.is_none());
        drop(first);
        let after_release = AdvisoryLock::try_acquire(&destination).expect("lock after release");
        assert!(after_release.is_some());
    }

    #[test]
    fn replacement_failure_preserves_destination_and_cleans_temporary_file() {
        let directory = tempfile::tempdir().expect("temp directory");
        let destination = directory.path().join("settings.json");
        std::fs::write(&destination, b"old").expect("old destination");

        let error = atomic_write_bytes_with(&destination, b"new", |temporary, target| {
            assert_eq!(target, destination);
            assert_eq!(temporary.parent(), destination.parent());
            assert!(temporary.is_file());
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "injected replacement failure",
            ))
        })
        .expect_err("injected failure");
        assert_eq!(error.kind(), crate::ConfigErrorKind::AtomicReplace);
        assert_eq!(
            std::fs::read(&destination).expect("preserved destination"),
            b"old"
        );
        let temporary_remains = std::fs::read_dir(directory.path())
            .expect("list directory")
            .map(|entry| entry.expect("directory entry").file_name())
            .any(|name| name.to_string_lossy().ends_with(".tmp"));
        assert!(!temporary_remains);
    }
}

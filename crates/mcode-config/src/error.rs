//! Value-redacted errors returned by configuration operations.

// Rust guideline compliant 2026-08-26

use std::backtrace::{Backtrace, BacktraceStatus};
use std::error::Error as StdError;
use std::fmt::{self, Debug, Display, Formatter};
use std::io;
use std::path::{Path, PathBuf};

use crate::{ConfigSource, JsonPointer};

/// Classifies a configuration failure without exposing configuration values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ConfigErrorKind {
    /// An owned home path is missing, relative, or lexically unsafe.
    InvalidHome,
    /// A requested owned path could escape its frozen hierarchy.
    PathEscape,
    /// A caller supplied an unusable resource-limit set.
    InvalidLimits,
    /// More source descriptors were supplied than the configured bound.
    TooManySources,
    /// No compiled-defaults document participated in the load.
    MissingCompiledDefaults,
    /// A non-project source was marked untrusted.
    UntrustedSource,
    /// Cooperative cancellation was observed.
    Cancelled,
    /// A source file could not be opened, read, or inspected.
    Io,
    /// JSON input was not valid UTF-8.
    NonUtf8,
    /// A byte bound was exceeded.
    Oversized,
    /// A JSON nesting-depth bound was exceeded.
    TooDeep,
    /// A JSON node-count bound was exceeded.
    TooManyNodes,
    /// An object contained a duplicate member name.
    DuplicateKey,
    /// Input was not strict JSON, was partial, or had trailing content.
    InvalidJson,
    /// The top-level versioned envelope had the wrong shape.
    InvalidEnvelope,
    /// The envelope did not use this crate's exact format version.
    UnsupportedFormatVersion,
    /// A credential-like field contained an inline value.
    CredentialValue,
    /// The caller's domain validation hook rejected the merged value.
    DomainValidation,
    /// A validated value could not be serialized.
    Serialization,
    /// The advisory write lock could not be acquired.
    Lock,
    /// A temporary file could not replace the destination.
    AtomicReplace,
    /// The snapshot generation counter cannot advance further.
    GenerationExhausted,
}

struct ConfigErrorInner {
    kind: ConfigErrorKind,
    config_source: Option<ConfigSource>,
    path: Option<PathBuf>,
    pointer: Option<JsonPointer>,
    io_kind: Option<io::ErrorKind>,
    backtrace: Backtrace,
}

/// Describes a configuration failure without retaining offending values.
///
/// Parse messages, input snippets, credential data, and domain-validator errors
/// are intentionally discarded. Callers can branch on [`Self::kind`], inspect
/// the source/path and JSON Pointer, and render the bounded summary from
/// [`Display`].
pub struct ConfigError {
    // Keeping contextual fields behind one allocation makes Result's error path
    // small while retaining a captured backtrace and native path information.
    inner: Box<ConfigErrorInner>,
}

impl ConfigError {
    pub(crate) fn new(kind: ConfigErrorKind) -> Self {
        Self {
            inner: Box::new(ConfigErrorInner {
                kind,
                config_source: None,
                path: None,
                pointer: None,
                io_kind: None,
                backtrace: Backtrace::capture(),
            }),
        }
    }

    pub(crate) fn for_source(kind: ConfigErrorKind, source: &ConfigSource) -> Self {
        let mut error = Self::new(kind);
        error.inner.config_source = Some(source.clone());
        error
    }

    pub(crate) fn for_path(kind: ConfigErrorKind, path: &Path) -> Self {
        let mut error = Self::new(kind);
        error.inner.path = Some(path.to_path_buf());
        error
    }

    pub(crate) fn at_pointer(mut self, pointer: JsonPointer) -> Self {
        self.inner.pointer = Some(pointer);
        self
    }

    pub(crate) fn with_io_kind(mut self, kind: io::ErrorKind) -> Self {
        self.inner.io_kind = Some(kind);
        self
    }

    /// Returns this failure's stable category.
    #[must_use]
    pub fn kind(&self) -> ConfigErrorKind {
        self.inner.kind
    }

    /// Returns the source descriptor associated with the failure, if any.
    #[must_use]
    pub fn config_source(&self) -> Option<&ConfigSource> {
        self.inner.config_source.as_ref()
    }

    /// Returns the affected path, if the operation had one.
    #[must_use]
    pub fn path(&self) -> Option<&Path> {
        self.inner.path.as_deref().or_else(|| {
            self.inner
                .config_source
                .as_ref()
                .map(|source| source.path.as_path())
        })
    }

    /// Returns the affected JSON Pointer, if it was safe to retain one.
    #[must_use]
    pub fn pointer(&self) -> Option<&JsonPointer> {
        self.inner.pointer.as_ref()
    }

    /// Returns the operating-system error category, if one was recorded.
    #[must_use]
    pub fn io_kind(&self) -> Option<io::ErrorKind> {
        self.inner.io_kind
    }

    /// Returns the backtrace captured where the error was classified.
    pub fn backtrace(&self) -> &Backtrace {
        &self.inner.backtrace
    }
}

impl Debug for ConfigError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ConfigError")
            .field("kind", &self.inner.kind)
            .field("config_source", &self.inner.config_source)
            .field("path", &self.inner.path)
            .field("pointer", &self.inner.pointer)
            .field("io_kind", &self.inner.io_kind)
            .field("backtrace", &self.inner.backtrace)
            .finish()
    }
}

impl Display for ConfigError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        let summary = match self.inner.kind {
            ConfigErrorKind::InvalidHome => "MCode home path is invalid",
            ConfigErrorKind::PathEscape => "owned path component is invalid",
            ConfigErrorKind::InvalidLimits => "configuration limits are invalid",
            ConfigErrorKind::TooManySources => "too many configuration sources were supplied",
            ConfigErrorKind::MissingCompiledDefaults => {
                "compiled configuration defaults are missing"
            }
            ConfigErrorKind::UntrustedSource => "a non-project configuration source is untrusted",
            ConfigErrorKind::Cancelled => "configuration operation was cancelled",
            ConfigErrorKind::Io => "configuration file I/O failed",
            ConfigErrorKind::NonUtf8 => "configuration JSON is not UTF-8",
            ConfigErrorKind::Oversized => "configuration size limit was exceeded",
            ConfigErrorKind::TooDeep => "configuration nesting limit was exceeded",
            ConfigErrorKind::TooManyNodes => "configuration node limit was exceeded",
            ConfigErrorKind::DuplicateKey => "configuration JSON contains a duplicate key",
            ConfigErrorKind::InvalidJson => "configuration is not strict complete JSON",
            ConfigErrorKind::InvalidEnvelope => "configuration envelope is invalid",
            ConfigErrorKind::UnsupportedFormatVersion => {
                "configuration format version is unsupported"
            }
            ConfigErrorKind::CredentialValue => {
                "credential-like configuration must use a secret reference"
            }
            ConfigErrorKind::DomainValidation => "merged configuration failed domain validation",
            ConfigErrorKind::Serialization => "configuration serialization failed",
            ConfigErrorKind::Lock => "configuration advisory lock failed",
            ConfigErrorKind::AtomicReplace => "configuration file replacement failed",
            ConfigErrorKind::GenerationExhausted => {
                "configuration snapshot generation is exhausted"
            }
        };
        formatter.write_str(summary)?;
        if self.inner.backtrace.status() == BacktraceStatus::Captured {
            write!(formatter, "\n{}", self.inner.backtrace)?;
        }
        Ok(())
    }
}

impl StdError for ConfigError {}

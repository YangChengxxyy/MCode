//! Caller-supplied configuration source metadata and input storage.

// Rust guideline compliant 2026-08-26

use std::fmt::{self, Debug, Display, Formatter};
use std::fs::File;
use std::io::{self, Read};
use std::path::PathBuf;

use crate::{ConfigError, ConfigErrorKind, ConfigLimits, ReloadCancellation};

/// Selects a source's fixed position in the configuration precedence chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ConfigScope {
    /// Defaults compiled or embedded by the composition root.
    CompiledDefaults,
    /// Global `$MCODE_HOME/settings.json` settings.
    Global,
    /// Project-local `.mcode/settings.json` settings.
    Project,
    /// Explicit, ephemeral overrides for one invocation.
    Explicit,
}

impl ConfigScope {
    pub(crate) const fn rank(self) -> u8 {
        match self {
            Self::CompiledDefaults => 0,
            Self::Global => 1,
            Self::Project => 2,
            Self::Explicit => 3,
        }
    }
}

impl Display for ConfigScope {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::CompiledDefaults => "compiled-defaults",
            Self::Global => "global",
            Self::Project => "project",
            Self::Explicit => "explicit",
        };
        formatter.write_str(name)
    }
}

/// Records whether the caller has authorized a source to participate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SourceTrust {
    /// The source may participate in merging.
    Trusted,
    /// The source may not participate in merging.
    Untrusted,
}

impl Display for SourceTrust {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Trusted => formatter.write_str("trusted"),
            Self::Untrusted => formatter.write_str("untrusted"),
        }
    }
}

/// Identifies one configuration source without storing its JSON value.
///
/// `path` is a [`PathBuf`] rather than UTF-8 text. It can therefore represent
/// native paths on every supported platform, including paths that cannot be
/// converted to Unicode. For in-memory sources, callers provide a logical path
/// used only for diagnostics and provenance.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ConfigSource {
    /// Position in the fixed precedence chain.
    pub scope: ConfigScope,
    /// Native file path or caller-chosen logical path.
    pub path: PathBuf,
    /// Trust decision already made by the composition root.
    pub trust: SourceTrust,
}

impl ConfigSource {
    /// Creates source metadata from caller-owned discovery and trust decisions.
    #[must_use]
    pub fn new(scope: ConfigScope, path: impl Into<PathBuf>, trust: SourceTrust) -> Self {
        Self {
            scope,
            path: path.into(),
            trust,
        }
    }
}

#[derive(Clone)]
enum SourceInput {
    File { required: bool },
    Inline(Vec<u8>),
}

/// Couples source metadata with file-backed or immutable in-memory JSON input.
///
/// The crate never discovers project roots or settings paths. Global and
/// project files are normally optional; compiled defaults and explicit
/// overrides are commonly supplied with [`Self::inline`]. Inline
/// bytes are redacted from this type's [`Debug`] implementation.
#[derive(Clone)]
pub struct ConfigLayer {
    source: ConfigSource,
    input: SourceInput,
}

impl ConfigLayer {
    /// Creates a required file-backed layer.
    #[must_use]
    pub fn required_file(source: ConfigSource) -> Self {
        Self {
            source,
            input: SourceInput::File { required: true },
        }
    }

    /// Creates an optional file-backed layer.
    ///
    /// A missing optional file contributes no patch. Every other I/O failure is
    /// fatal, preserving the previous runtime snapshot during reload.
    #[must_use]
    pub fn optional_file(source: ConfigSource) -> Self {
        Self {
            source,
            input: SourceInput::File { required: false },
        }
    }

    /// Creates an immutable in-memory layer by copying `bytes`.
    #[must_use]
    pub fn inline(source: ConfigSource, bytes: impl AsRef<[u8]>) -> Self {
        Self {
            source,
            input: SourceInput::Inline(bytes.as_ref().to_vec()),
        }
    }

    /// Returns this layer's source metadata.
    #[must_use]
    pub fn source(&self) -> &ConfigSource {
        &self.source
    }

    pub(crate) fn read(
        &self,
        limits: ConfigLimits,
        cancellation: &ReloadCancellation,
    ) -> Result<Option<Vec<u8>>, ConfigError> {
        if cancellation.is_cancelled() {
            return Err(ConfigError::for_source(
                ConfigErrorKind::Cancelled,
                &self.source,
            ));
        }

        match &self.input {
            SourceInput::Inline(bytes) => {
                if bytes.len() > limits.max_source_bytes {
                    return Err(ConfigError::for_source(
                        ConfigErrorKind::Oversized,
                        &self.source,
                    ));
                }
                Ok(Some(bytes.clone()))
            }
            SourceInput::File { required } => self.read_file(*required, limits, cancellation),
        }
    }

    fn read_file(
        &self,
        required: bool,
        limits: ConfigLimits,
        cancellation: &ReloadCancellation,
    ) -> Result<Option<Vec<u8>>, ConfigError> {
        let mut file = match File::open(&self.source.path) {
            Ok(file) => file,
            Err(error) if !required && error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(ConfigError::for_source(ConfigErrorKind::Io, &self.source)
                    .with_io_kind(error.kind()));
            }
        };

        let maximum = u64::try_from(limits.max_source_bytes).unwrap_or(u64::MAX);
        let metadata = file.metadata().map_err(|error| {
            ConfigError::for_source(ConfigErrorKind::Io, &self.source).with_io_kind(error.kind())
        })?;
        if metadata.len() > maximum {
            return Err(ConfigError::for_source(
                ConfigErrorKind::Oversized,
                &self.source,
            ));
        }

        // Eight KiB keeps cancellation responsive without issuing tiny reads.
        const READ_CHUNK_BYTES: usize = 8 * 1024;
        let mut bytes = Vec::with_capacity(
            usize::try_from(metadata.len())
                .unwrap_or(limits.max_source_bytes)
                .min(limits.max_source_bytes),
        );
        let mut chunk = [0_u8; READ_CHUNK_BYTES];
        loop {
            if cancellation.is_cancelled() {
                return Err(ConfigError::for_source(
                    ConfigErrorKind::Cancelled,
                    &self.source,
                ));
            }
            let read = file.read(&mut chunk).map_err(|error| {
                ConfigError::for_source(ConfigErrorKind::Io, &self.source)
                    .with_io_kind(error.kind())
            })?;
            if read == 0 {
                break;
            }
            let next_length = bytes
                .len()
                .checked_add(read)
                .ok_or_else(|| ConfigError::for_source(ConfigErrorKind::Oversized, &self.source))?;
            if next_length > limits.max_source_bytes {
                return Err(ConfigError::for_source(
                    ConfigErrorKind::Oversized,
                    &self.source,
                ));
            }
            bytes.extend_from_slice(&chunk[..read]);
        }
        Ok(Some(bytes))
    }
}

impl Debug for ConfigLayer {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        let input = match &self.input {
            SourceInput::File { required: true } => "required-file",
            SourceInput::File { required: false } => "optional-file",
            SourceInput::Inline(_) => "inline-redacted",
        };
        formatter
            .debug_struct("ConfigLayer")
            .field("source", &self.source)
            .field("input", &input)
            .finish()
    }
}

//! Stable preflight and caller-binding failures.

// Rust guideline compliant 2026-08-29.

/// Classifies an ambient or non-contract component import.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportCategory {
    /// A filesystem interface.
    Filesystem,
    /// A network or socket interface.
    Network,
    /// A process, environment, or command interface.
    Process,
    /// A terminal or standard-stream interface.
    Terminal,
    /// An HTTP interface.
    Http,
    /// A random-number interface.
    Random,
    /// A clock interface.
    Clocks,
    /// A secret, key-value, or credential interface.
    Secret,
    /// A logging or diagnostic interface.
    Logging,
    /// A user-interface or rendering interface.
    UserInterface,
    /// A raw Host handle interface.
    RawHost,
    /// Another WASI interface.
    Wasi,
    /// A noncurrent MCode plugin interface version.
    MCodeVersion,
    /// Any other non-contract interface.
    Extra,
}

/// Reports bounded Manager component compile or shape failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PreflightError {
    /// A caller requested an invalid component-size limit.
    #[error("Manager component limits are invalid")]
    InvalidLimits,
    /// Encoded component bytes exceeded the configured hard bound.
    #[error("Manager component exceeds its size limit")]
    ComponentTooLarge,
    /// Wasmtime could not create the isolated component engine.
    #[error("Manager component engine is unavailable")]
    Engine,
    /// Input was not a valid WebAssembly component.
    #[error("Manager component is invalid")]
    InvalidComponent,
    /// The component imported an ambient, noncurrent, or extra interface.
    #[error("Manager component imports a denied interface")]
    DeniedImport(ImportCategory),
    /// The sole FeatureService import was absent.
    #[error("Manager component is missing the FeatureService import")]
    MissingImport,
    /// The FeatureService function shape did not match current bindings.
    #[error("Manager FeatureService import shape is invalid")]
    ImportShape,
    /// The sole lifecycle export was absent.
    #[error("Manager component is missing its lifecycle export")]
    MissingExport,
    /// The component exported another interface or root item.
    #[error("Manager component exports a non-contract item")]
    UnexpectedExport,
    /// The lifecycle function and type shape did not match current bindings.
    #[error("Manager lifecycle export shape is invalid")]
    ExportShape,
}

/// Reports canonical Manager caller binding failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CallerBindingError {
    /// The supplied Manager ID was not canonical for its family.
    #[error("Manager caller identity does not match its family")]
    IdentityMismatch,
}

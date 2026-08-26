//! Typed host runtime errors.

// Rust guideline compliant 2026-08-26.

use mcode_plugin_api::{Identifier, PluginId};

/// Host runtime failure.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HostError {
    /// Runtime limits were invalid.
    #[error("plugin runtime limits are invalid")]
    InvalidLimits,
    /// Wasmtime engine or component setup failed.
    #[error("plugin wasm engine failed")]
    Engine,
    /// Component bytes were not a valid WebAssembly component.
    #[error("plugin wasm component is invalid")]
    InvalidComponent,
    /// Component imports were not a subset of the WIT world and grants.
    #[error("plugin wasm imports are not permitted")]
    ForbiddenImport,
    /// Manifest imports did not match the component or WIT world.
    #[error("plugin manifest imports do not match the component")]
    ImportMismatch,
    /// Instantiation failed after import checks.
    #[error("plugin wasm instantiation failed")]
    Instantiate,
    /// The dedicated actor thread could not be created.
    #[error("plugin actor thread could not be started")]
    ActorSpawn,
    /// A guest export trapped, exhausted fuel, or hit the epoch deadline.
    #[error("plugin wasm guest trapped")]
    Trap,
    /// Guest output exceeded a host length limit.
    #[error("plugin guest output exceeds its size limit")]
    GuestOutputTooLarge,
    /// Guest output was not the expected JSON contract.
    #[error("plugin guest output is invalid")]
    InvalidGuestOutput,
    /// Guest returned a typed error code.
    #[error("plugin guest returned error {code}")]
    Guest {
        /// Stable error code.
        code: Identifier,
    },
    /// Mailbox rejected the job.
    #[error("plugin mailbox rejected the job")]
    MailboxClosed,
    /// Mailbox was full.
    #[error("plugin mailbox is full")]
    MailboxFull,
    /// Job generation did not match the live generation.
    #[error("plugin generation is stale")]
    StaleGeneration,
    /// Stop/disable exceeded its deadline.
    #[error("plugin stop exceeded its deadline")]
    StopTimeout,
    /// Plugin identity did not match the loaded generation.
    #[error("plugin {0} is not the active generation")]
    Identity(PluginId),
    /// Host import payload exceeded its limit.
    #[error("plugin host import exceeded its size limit")]
    HostImportTooLarge,
    /// Published view or action failed validation.
    #[error("plugin host import payload is invalid")]
    InvalidHostPayload,
    /// The actor is not accepting work.
    #[error("plugin runtime is not running")]
    NotRunning,
}

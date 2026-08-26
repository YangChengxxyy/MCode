//! Test-only fake guest implementing the WIT export surface.

// Rust guideline compliant 2026-08-26.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use mcode_plugin_api::PluginId;

use crate::actor::{PluginHandle, RuntimeLimits};
use crate::error::HostError;
use crate::loader::spawn_fake;

/// In-process guest used by lifecycle and race tests.
pub trait FakeGuest: Send {
    /// WIT `construct` export.
    ///
    /// # Errors
    ///
    /// Returns [`HostError`] when the fake guest is configured to fail.
    fn construct(&mut self) -> Result<String, HostError> {
        Ok(String::new())
    }

    /// WIT `invoke` export.
    ///
    /// # Errors
    ///
    /// Returns [`HostError`] when the fake guest is configured to fail.
    fn invoke(&mut self, _request: &str) -> Result<String, HostError> {
        Ok("{}".into())
    }

    /// WIT `on-event` export.
    ///
    /// # Errors
    ///
    /// Returns [`HostError`] when the fake guest is configured to fail.
    fn on_event(&mut self, _event: &str) -> Result<String, HostError> {
        Ok(String::new())
    }

    /// WIT `render` export.
    ///
    /// # Errors
    ///
    /// Returns [`HostError`] when the fake guest is configured to fail.
    fn render(&mut self, _request: &str) -> Result<String, HostError> {
        Ok(r#"{"view":{"kind":"widget","metadata":{"id":"status.main","region":"global","priority":0,"width":{"min":1,"max":80},"invalidation":{"mode":"manual"}},"content":{"type":"text","text":"ok","tone":"normal","emphasized":false}}}"#.into())
    }
}

/// Counting fake that records whether `on_event` entered the guest.
#[derive(Debug, Clone)]
pub struct CountingGuest {
    /// Number of `on_event` entries.
    pub events: Arc<AtomicUsize>,
    /// Number of `invoke` entries.
    pub invokes: Arc<AtomicUsize>,
}

impl CountingGuest {
    /// Creates a zeroed counter pair.
    #[must_use]
    pub fn new() -> Self {
        Self {
            events: Arc::new(AtomicUsize::new(0)),
            invokes: Arc::new(AtomicUsize::new(0)),
        }
    }
}

impl Default for CountingGuest {
    fn default() -> Self {
        Self::new()
    }
}

impl FakeGuest for CountingGuest {
    fn on_event(&mut self, _event: &str) -> Result<String, HostError> {
        self.events.fetch_add(1, Ordering::AcqRel);
        Ok(String::new())
    }

    fn invoke(&mut self, _request: &str) -> Result<String, HostError> {
        self.invokes.fetch_add(1, Ordering::AcqRel);
        Ok("{}".into())
    }
}

/// Blocking fake used to hold the enter lock.
pub struct BlockingGuest {
    /// Signaled when `on_event` is entered.
    pub entered: Arc<std::sync::Mutex<bool>>,
    /// Released to finish `on_event`.
    pub release: Arc<std::sync::Mutex<bool>>,
}

impl FakeGuest for BlockingGuest {
    fn on_event(&mut self, _event: &str) -> Result<String, HostError> {
        *self
            .entered
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
        loop {
            if *self
                .release
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
            {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        Ok(String::new())
    }
}

/// Spawns a fake-component generation.
///
/// # Errors
///
/// Returns [`HostError`] when the engine cannot be created.
pub fn spawn_fake_generation(
    plugin_id: PluginId,
    generation: u64,
    guest: impl FakeGuest + 'static,
    limits: RuntimeLimits,
) -> Result<PluginHandle, HostError> {
    spawn_fake(plugin_id, generation, Box::new(guest), limits)
}

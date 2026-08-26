//! Pure application state consumed by actions and rendering.
//!
//! The state contains only owned data and capability-independent choices. It
//! has no channels, terminal handles, clocks, or callbacks.

// Rust guideline compliant 2026-08-26.

use mcode_render::RenderBlock;

use crate::labels::{INPUT_PLACEHOLDER, STATUS_READY};
use crate::theme::{BackgroundClass, ThemeSelection};

/// Current terminal viewport in columns and rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Viewport {
    /// Width in terminal columns.
    pub width: u16,
    /// Height in terminal rows.
    pub height: u16,
}

impl Viewport {
    /// Creates a viewport from terminal dimensions.
    #[must_use]
    pub const fn new(width: u16, height: u16) -> Self {
        Self { width, height }
    }
}

impl Default for Viewport {
    fn default() -> Self {
        Self::new(80, 24)
    }
}

/// Complete pure state for the foundation view.
#[derive(Debug, Clone, PartialEq)]
pub struct AppState {
    pub(crate) viewport: Viewport,
    pub(crate) blocks: Vec<RenderBlock>,
    pub(crate) status: String,
    pub(crate) input: String,
    pub(crate) input_placeholder: String,
    pub(crate) theme_selection: ThemeSelection,
    pub(crate) detected_background: Option<BackgroundClass>,
    pub(crate) help_visible: bool,
}

impl AppState {
    /// Creates empty ready state for `viewport`.
    #[must_use]
    pub fn new(viewport: Viewport) -> Self {
        Self {
            viewport,
            blocks: Vec::new(),
            status: STATUS_READY.into(),
            input: String::new(),
            input_placeholder: INPUT_PLACEHOLDER.into(),
            theme_selection: ThemeSelection::Auto,
            detected_background: None,
            help_visible: false,
        }
    }

    /// Returns the current viewport.
    #[must_use]
    pub const fn viewport(&self) -> Viewport {
        self.viewport
    }

    /// Returns render blocks in display order.
    #[must_use]
    pub fn blocks(&self) -> &[RenderBlock] {
        &self.blocks
    }

    /// Returns status-bar text supplied by the host.
    #[must_use]
    pub fn status(&self) -> &str {
        &self.status
    }

    /// Returns the current input buffer.
    #[must_use]
    pub fn input(&self) -> &str {
        &self.input
    }

    /// Returns the empty-input placeholder.
    #[must_use]
    pub fn input_placeholder(&self) -> &str {
        &self.input_placeholder
    }

    /// Returns configured theme selection.
    #[must_use]
    pub const fn theme_selection(&self) -> &ThemeSelection {
        &self.theme_selection
    }

    /// Returns the externally detected terminal background.
    #[must_use]
    pub const fn detected_background(&self) -> Option<BackgroundClass> {
        self.detected_background
    }

    /// Returns whether the built-in help panel is visible.
    #[must_use]
    pub const fn is_help_visible(&self) -> bool {
        self.help_visible
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new(Viewport::default())
    }
}

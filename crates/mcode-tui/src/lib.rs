//! Pure-state terminal UI foundation for MCode.
//!
//! The crate supplies a callable library API only. [`AppView`] separates an
//! injectable [`ActionRegistry`] for crossterm input, pure [`Action`] reduction,
//! data-only [`Effect`] requests, semantic theme resolution, and Ratatui
//! drawing. It does not modify the `mcode` binary or connect to a session actor.

// Rust guideline compliant 2026-08-27.

pub mod actions;
pub mod app_view;
pub mod consent;
pub mod editor;
pub mod guard;
pub mod hints;
pub mod labels;
mod layout;
pub mod logo;
pub mod preview;
pub mod render;
pub mod scrollback;
pub mod state;
pub mod terminal;
pub mod theme;

#[doc(inline)]
pub use actions::{
    Action, ActionBinding, ActionId, ActionRegistry, Effect, InputOutcome, Invalidation,
    KeyPattern, Transition, When, reduce,
};
#[doc(inline)]
pub use app_view::{AppView, action_for_event};
#[doc(inline)]
pub use consent::{
    CONSENT_MAX_BODY_COLUMNS, CONSENT_MAX_BODY_LINES, CONSENT_MAX_TITLE_COLUMNS,
    CONSENT_MIN_COLUMNS, CONSENT_MIN_ROWS, ConsentChoice, ConsentPrompt, StatusSurface,
    is_readable as consent_is_readable,
};
#[doc(inline)]
pub use editor::LineEditor;
#[doc(inline)]
pub use guard::{RestoreProbe, TerminalGuard, restore_on_abnormal_exit};
#[doc(inline)]
pub use hints::{help_lines, key_label, pattern_label, status_key_hints};
#[doc(inline)]
pub use logo::{LogoLine, LogoSpan, LogoVariant, TerminalLogo, terminal_logo};
#[doc(inline)]
pub use scrollback::{
    DEFAULT_SCROLLBACK_BLOCKS, MaterializeBudget, MaterializedLine, MaterializedView, Scrollback,
    materialize,
};
#[doc(inline)]
pub use state::{AppState, Viewport};
#[doc(inline)]
pub use terminal::{
    ColorCapability, Osc11ProbeConfig, TerminalCapabilities, classify_background,
    parse_osc11_response, query_background, rgb_to_ansi256,
};
#[doc(inline)]
pub use theme::{
    BackgroundClass, Rgb, SemanticToken, Theme, ThemeAppearance, ThemeResolution, ThemeSelection,
    ThemeSource, contrast_ratio, mcode_dark, mcode_light, relative_luminance, resolve_theme,
};

//! Offline layout-preview adapter with no session or network access.
//!
//! The interactive example owns the terminal event loop. This module seeds
//! synthetic transcript state, maps preview-only keys, and applies local
//! effects so Allow/Deny and Enter never execute tools.

// Rust guideline compliant 2026-08-27.

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use mcode_render::{Diff, DiffHunk, DiffLine, DiffLineKind, ErrorBlock, Progress, RenderBlock};

use crate::actions::{
    Action, ActionBinding, ActionId, ActionRegistry, Effect, InputOutcome, KeyPattern, When,
};
use crate::app_view::AppView;
use crate::consent::{ConsentChoice, ConsentPrompt};
use crate::layout::transcript_viewport;
use crate::state::{AppState, Viewport};
use crate::terminal::{ColorCapability, TerminalCapabilities};

/// Status text that advertises preview keys not bound in the default registry.
pub const PREVIEW_STATUS: &str = "Preview  Esc/q Quit | Up/Dn/Pg Scroll | Ctrl+P Consent";

/// Host token echoed by consent effects; not a permission-engine id.
const PREVIEW_CONSENT_ID: &str = "preview-consent";

/// History rows seeded so a typical 24-row terminal must scroll.
///
/// Kept well below [`crate::DEFAULT_SCROLLBACK_BLOCKS`] so startup stays cheap
/// while still overflowing the transcript panel.
const PREVIEW_HISTORY_LINES: usize = 40;

/// Result of one preview event after local effect handling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[must_use]
pub enum PreviewOutcome {
    /// Keep reading terminal input.
    Continue,
    /// Leave the preview event loop.
    Quit,
}

/// Builds a synthetic layout-preview view for `viewport`.
///
/// The view uses the real foundation renderer and default-plus-preview
/// bindings. It never reads credentials, session files, or `MCODE_HOME`.
///
/// # Examples
///
/// ```
/// use mcode_tui::preview::seed_preview_view;
/// use mcode_tui::Viewport;
///
/// let view = seed_preview_view(Viewport::new(80, 24));
/// assert!(!view.state().blocks().is_empty());
/// assert!(view.state().input().is_empty());
/// ```
#[must_use]
pub fn seed_preview_view(viewport: Viewport) -> AppView {
    let mut view = AppView::new(
        viewport,
        TerminalCapabilities::new(ColorCapability::TrueColor, true),
    )
    .with_action_registry(preview_registry());
    view.dispatch(Action::ReplaceBlocks(preview_blocks()));
    view.dispatch(Action::SetStatus(PREVIEW_STATUS.into()));
    view
}

/// Default bindings plus Esc/`q` quit used by the layout preview.
#[must_use]
pub fn preview_registry() -> ActionRegistry {
    let mut registry = ActionRegistry::default();
    registry.register(
        ActionBinding::new(
            KeyPattern::exact(KeyCode::Esc, KeyModifiers::NONE),
            ActionId::Quit,
        )
        .when(When::CONSENT_HIDDEN),
    );
    for (character, modifiers) in [
        ('q', KeyModifiers::NONE),
        ('Q', KeyModifiers::NONE),
        ('q', KeyModifiers::SHIFT),
        ('Q', KeyModifiers::SHIFT),
    ] {
        registry.register(
            ActionBinding::new(
                KeyPattern::exact(KeyCode::Char(character), modifiers),
                ActionId::Quit,
            )
            .when(When::INPUT_EMPTY),
        );
    }
    registry
}

/// Translates one event through preview keys, then the injected registry.
///
/// Scroll and Ctrl+P are preview-only. Submit and consent answers stay local:
/// they never call a provider or permission engine.
///
/// # Examples
///
/// ```
/// use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
/// use mcode_tui::preview::{handle_preview_event, seed_preview_view, PreviewOutcome};
/// use mcode_tui::Viewport;
///
/// let mut view = seed_preview_view(Viewport::new(80, 24));
/// let quit = Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
/// assert_eq!(handle_preview_event(&mut view, &quit), PreviewOutcome::Quit);
/// ```
pub fn handle_preview_event(view: &mut AppView, event: &Event) -> PreviewOutcome {
    if let Event::Key(key) = event
        && matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat)
    {
        if is_consent_toggle(key) {
            let effects = toggle_consent(view);
            return apply_preview_effects(view, &effects);
        }
        if view.state().consent().is_none()
            && let Some(delta) = scroll_delta(key, view.state())
        {
            let effects = view.dispatch(Action::ScrollBy(delta));
            return apply_preview_effects(view, &effects);
        }
    }

    match view.handle_input(event) {
        InputOutcome::Ignored => PreviewOutcome::Continue,
        InputOutcome::Handled(effects) => apply_preview_effects(view, &effects),
    }
}

fn toggle_consent(view: &mut AppView) -> Vec<Effect> {
    if view.state().consent().is_some() {
        view.dispatch(Action::ResolveConsent(ConsentChoice::Deny))
    } else {
        view.dispatch(Action::PresentConsent(preview_consent()))
    }
}

fn apply_preview_effects(view: &mut AppView, effects: &[Effect]) -> PreviewOutcome {
    let mut submitted = Vec::new();
    let mut consent_choice = None;
    let mut quit = false;
    for effect in effects {
        match effect {
            Effect::RequestQuit => quit = true,
            Effect::SubmitInput(input) => submitted.push(input.clone()),
            Effect::ConsentResolved { choice, .. } => consent_choice = Some(*choice),
            Effect::Redraw(_) => {}
        }
    }

    for input in submitted {
        let mut blocks = view.state().blocks().to_vec();
        blocks.extend(preview_turn(&input));
        view.dispatch(Action::ReplaceBlocks(blocks));
        view.dispatch(Action::SetStatus(PREVIEW_STATUS.into()));
    }
    if let Some(choice) = consent_choice {
        view.dispatch(Action::SetStatus(format!(
            "Preview: {choice} (not executed)  {PREVIEW_STATUS}"
        )));
    }

    if quit {
        PreviewOutcome::Quit
    } else {
        PreviewOutcome::Continue
    }
}

fn is_consent_toggle(key: &KeyEvent) -> bool {
    matches!(key.code, KeyCode::Char('p' | 'P'))
        && (key.modifiers == KeyModifiers::CONTROL
            || key.modifiers == KeyModifiers::CONTROL.union(KeyModifiers::SHIFT))
}

fn scroll_delta(key: &KeyEvent, state: &AppState) -> Option<i32> {
    if key.modifiers != KeyModifiers::NONE {
        return None;
    }
    let page = i32::from(transcript_viewport(state).height.max(1));
    match key.code {
        KeyCode::Up => Some(1),
        KeyCode::Down => Some(-1),
        KeyCode::PageUp => Some(page),
        KeyCode::PageDown => Some(-page),
        _ => None,
    }
}

fn preview_consent() -> ConsentPrompt {
    ConsentPrompt::new(
        PREVIEW_CONSENT_ID,
        "bash",
        "Run `ls` in the layout preview.\nAllow and Deny do not execute anything.",
    )
}

fn preview_turn(input: &str) -> Vec<RenderBlock> {
    vec![
        RenderBlock::Text(format!("You: {input}")),
        RenderBlock::Markdown(format!(
            "## Preview\n\nLocal echo; no model or tool ran.\n\n```\n{input}\n```"
        )),
    ]
}

fn preview_blocks() -> Vec<RenderBlock> {
    let mut blocks = (1..=PREVIEW_HISTORY_LINES)
        .map(|index| {
            RenderBlock::Text(format!(
                "history-{index:02}: synthetic scrollback (capacity {})",
                crate::DEFAULT_SCROLLBACK_BLOCKS
            ))
        })
        .collect::<Vec<_>>();
    blocks.extend([
        RenderBlock::Markdown(String::from(
            "## Layout preview\n\n\
             Esc or q quit (`q` only when input is empty).\n\
             Up/Down and PageUp/PageDown scroll.\n\
             Type to edit. Enter submits a local echo.\n\
             Ctrl+P toggles consent. F1 help. Shift+Enter inserts a newline.",
        )),
        RenderBlock::Markdown(String::from(
            "## Assistant\n\nSynthetic reply. No provider is connected.",
        )),
        RenderBlock::Progress(Progress::running("read_file", 3, Some(4))),
        RenderBlock::Text(String::from(
            "tool:read_file path=crates/mcode-tui/src/lib.rs\npub mod preview;",
        )),
        RenderBlock::Diff(Diff::new(
            "crates/mcode-tui/src/preview.rs",
            vec![DiffHunk::new(
                "@@ preview @@",
                vec![
                    DiffLine::new(
                        DiffLineKind::Context,
                        "fn preview_blocks() -> Vec<RenderBlock> {",
                    ),
                    DiffLine::new(DiffLineKind::Removed, "    Vec::new()"),
                    DiffLine::new(DiffLineKind::Added, "    synthetic_history()"),
                ],
            )],
        )),
        RenderBlock::Error(ErrorBlock::new(
            "Preview isolation",
            "This session never leaves the local TUI crate.",
        )),
    ]);
    blocks
}

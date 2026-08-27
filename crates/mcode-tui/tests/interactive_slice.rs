// Rust guideline compliant 2026-08-27.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Mutex, MutexGuard};

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use mcode_render::{RenderBlock, display_width};
use mcode_tui::{
    Action, AppView, ColorCapability, ConsentChoice, ConsentPrompt, DEFAULT_SCROLLBACK_BLOCKS,
    Effect, InputOutcome, Invalidation, LineEditor, MaterializeBudget, TerminalCapabilities,
    TerminalGuard, Viewport, materialize, restore_on_abnormal_exit,
};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;

fn guard_test_lock() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn default_view() -> AppView {
    AppView::new(
        Viewport::new(80, 24),
        TerminalCapabilities::new(ColorCapability::TrueColor, true),
    )
}

fn buffer_text(buffer: &Buffer) -> String {
    let mut output = String::new();
    for y in buffer.area.y..buffer.area.bottom() {
        for x in buffer.area.x..buffer.area.right() {
            output.push_str(buffer[(x, y)].symbol());
        }
        output.push('\n');
    }
    output
}

#[test]
fn mocked_guard_restores_on_drop() {
    let _lock = guard_test_lock();
    let probe = {
        let (guard, probe) = TerminalGuard::new_mocked().expect("mock guard");
        assert!(!guard.is_restored());
        assert!(probe.is_raw_mode());
        assert!(probe.is_alternate_screen());
        assert!(probe.is_cursor_hidden());
        drop(guard);
        probe
    };
    assert!(probe.is_restored());
    assert_eq!(probe.restore_count(), 1);
    assert!(!probe.is_raw_mode());
    assert!(!probe.is_alternate_screen());
    assert!(!probe.is_cursor_hidden());
}

#[test]
fn mocked_guard_restores_once_after_panic_unwind() {
    let _lock = guard_test_lock();
    let (guard, probe) = TerminalGuard::new_mocked().expect("mock guard");
    let result = catch_unwind(AssertUnwindSafe(|| {
        let _guard = guard;
        panic!("terminal guard unwind");
    }));
    assert!(result.is_err());
    assert!(probe.is_restored());
    assert_eq!(probe.restore_count(), 1);
    assert!(!probe.is_raw_mode());
}

#[test]
fn abnormal_exit_restores_a_forgotten_guard_on_windows_path() {
    let _lock = guard_test_lock();
    let (guard, probe) = TerminalGuard::new_mocked().expect("mock guard");
    std::mem::forget(guard);
    assert!(!probe.is_restored());
    restore_on_abnormal_exit();
    assert!(probe.is_restored());
    assert_eq!(probe.restore_count(), 1);
    restore_on_abnormal_exit();
    assert_eq!(probe.restore_count(), 1);
}

#[test]
fn resize_updates_viewport_and_can_fail_open_consent() {
    let mut view = default_view();
    assert!(
        view.dispatch(Action::Resize(Viewport::new(80, 24)))
            .is_empty()
    );

    assert_eq!(
        view.dispatch(Action::Resize(Viewport::new(0, 0))),
        vec![Effect::Redraw(Invalidation::Layout)]
    );
    assert_eq!(view.state().viewport(), Viewport::new(0, 0));

    view.dispatch(Action::Resize(Viewport::new(80, 24)));
    view.dispatch(Action::PresentConsent(ConsentPrompt::new(
        "req-resize",
        "bash",
        "ls",
    )));
    assert!(view.state().consent().is_some());

    let effects = view.dispatch(Action::Resize(Viewport::new(10, 4)));
    assert!(effects.contains(&Effect::Redraw(Invalidation::Layout)));
    assert!(effects.contains(&Effect::ConsentResolved {
        request_id: "req-resize".into(),
        choice: ConsentChoice::Deny,
    }));
    assert!(view.state().consent().is_none());
}

#[test]
fn unicode_width_and_grapheme_backspace_use_render_helpers() {
    let mut editor = LineEditor::new();
    assert!(editor.insert('Ａ'));
    assert_eq!(editor.display_width(), 2);
    assert_eq!(display_width(editor.as_str()), 2);

    let scientist = "\u{1F469}\u{200D}\u{1F52C}";
    assert!(editor.paste(scientist));
    assert!(editor.as_str().ends_with(scientist));
    assert!(editor.backspace());
    assert_eq!(editor.as_str(), "Ａ");

    let mut view = default_view();
    assert!(
        view.handle_input(&Event::Key(KeyEvent::new(
            KeyCode::Char('Ａ'),
            KeyModifiers::NONE,
        )))
        .is_handled()
    );
    assert_eq!(display_width(view.state().input()), 2);
    view.dispatch(Action::Paste(scientist.into()));
    view.dispatch(Action::Backspace);
    assert_eq!(view.state().input(), "Ａ");
}

#[test]
fn paste_event_fills_the_multiline_editor() {
    let mut view = default_view();
    let pasted = "hello\n世界";
    assert_eq!(
        view.handle_input(&Event::Paste(pasted.into())),
        InputOutcome::Handled(vec![Effect::Redraw(Invalidation::Content)])
    );
    assert_eq!(view.state().input(), pasted);
    assert!(view.state().input().contains('\n'));

    view.dispatch(Action::InsertNewline);
    assert!(view.state().input().ends_with('\n'));

    let shift_enter = view.handle_input(&Event::Key(KeyEvent::new(
        KeyCode::Enter,
        KeyModifiers::SHIFT,
    )));
    assert!(shift_enter.is_handled());
    assert!(view.state().input().ends_with("\n\n"));
}

#[test]
fn zero_viewport_budget_does_not_walk_history() {
    let blocks = (0..5_000)
        .map(|index| RenderBlock::Text(format!("block-{index}")))
        .collect::<Vec<_>>();

    let zero_width = materialize(&blocks, MaterializeBudget::new(0, 24, 0));
    assert!(zero_width.lines().is_empty());
    assert_eq!(zero_width.blocks_examined(), 0);

    let zero_height = materialize(&blocks, MaterializeBudget::new(80, 0, 0));
    assert!(zero_height.lines().is_empty());
    assert_eq!(zero_height.blocks_examined(), 0);

    let zero_viewport = materialize(
        &blocks,
        MaterializeBudget::from_viewport(Viewport::new(0, 0), 0),
    );
    assert_eq!(zero_viewport.blocks_examined(), 0);

    let visible = materialize(&blocks, MaterializeBudget::new(80, 10, 0));
    assert_eq!(visible.lines().len(), 10);
    assert!(visible.blocks_examined() <= 20);
    assert!(visible.blocks_examined() > 0);
    assert_eq!(
        visible.lines().last().map(|line| line.text()),
        Some("block-4999")
    );
}

#[test]
fn replace_blocks_is_capacity_bounded_and_draw_survives_zero_inner_width() {
    let mut view = default_view();
    let blocks = (0..2_000)
        .map(|index| RenderBlock::Text(format!("{index}")))
        .collect::<Vec<_>>();
    view.dispatch(Action::ReplaceBlocks(blocks));
    assert_eq!(view.state().blocks().len(), DEFAULT_SCROLLBACK_BLOCKS);
    assert_eq!(
        view.state().blocks().last(),
        Some(&RenderBlock::Text("1999".into()))
    );

    let many = (0..5_000)
        .map(|index| RenderBlock::Text(format!("row-{index}")))
        .collect::<Vec<_>>();
    let mut narrow = AppView::new(
        Viewport::new(1, 24),
        TerminalCapabilities::from_detection(ColorCapability::TrueColor, false, false),
    );
    narrow.dispatch(Action::ReplaceBlocks(many));
    let backend = TestBackend::new(1, 24);
    let mut terminal = Terminal::new(backend).expect("test terminal must initialize");
    terminal
        .draw(|frame| narrow.draw(frame))
        .expect("zero inner width must not panic");
}

#[test]
fn consent_and_status_are_pure_data_effects() {
    let mut view = default_view();
    assert_eq!(view.state().status_surface().message(), "Ready");
    view.dispatch(Action::SetStatus("Indexing".into()));
    assert_eq!(view.state().status(), "Indexing");

    let prompt = ConsentPrompt::new("req-1", "bash", "Run ls");
    assert_eq!(
        view.dispatch(Action::PresentConsent(prompt)),
        vec![Effect::Redraw(Invalidation::Content)]
    );
    assert_eq!(
        view.state().consent().map(ConsentPrompt::request_id),
        Some("req-1")
    );

    assert_eq!(
        view.dispatch(Action::ResolveConsent(ConsentChoice::AllowOnce)),
        vec![
            Effect::ConsentResolved {
                request_id: "req-1".into(),
                choice: ConsentChoice::AllowOnce,
            },
            Effect::Redraw(Invalidation::Content),
        ]
    );
    assert!(view.state().consent().is_none());

    let mut tiny = AppView::new(Viewport::new(10, 4), TerminalCapabilities::default());
    let effects = tiny.dispatch(Action::PresentConsent(ConsentPrompt::new(
        "req-deny", "bash", "hidden",
    )));
    assert!(tiny.state().consent().is_none());
    assert_eq!(
        effects,
        vec![Effect::ConsentResolved {
            request_id: "req-deny".into(),
            choice: ConsentChoice::Deny,
        }]
    );
}

#[test]
fn from_detection_drives_unicode_and_color_readability() {
    let ascii = TerminalCapabilities::from_detection(ColorCapability::TrueColor, false, false);
    assert!(ascii.supports_color());
    assert!(!ascii.supports_unicode());

    let backend = TestBackend::new(40, 16);
    let mut terminal = Terminal::new(backend).expect("test terminal must initialize");
    let mut view = AppView::new(Viewport::new(40, 16), ascii);
    view.dispatch(Action::ReplaceBlocks(vec![RenderBlock::Text(
        "\u{2764}\u{fe0f} Ａ".into(),
    )]));
    view.dispatch(Action::ReplaceInput("é".into()));
    terminal
        .draw(|frame| view.draw(frame))
        .expect("ASCII fallback must render");
    let screen = buffer_text(terminal.backend().buffer());
    assert!(screen.is_ascii(), "unicode leaked into {screen:?}");
    assert!(!screen.contains('\u{2026}'));

    let no_color = TerminalCapabilities::from_detection(ColorCapability::TrueColor, true, true);
    assert!(!no_color.supports_color());
    assert!(no_color.supports_unicode());
    let buffer = {
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).expect("test terminal must initialize");
        let mut view = AppView::new(Viewport::new(80, 24), no_color);
        terminal
            .draw(|frame| view.draw(frame))
            .expect("no-color unicode view must render");
        terminal.backend().buffer().clone()
    };
    assert_eq!(buffer[(0, 0)].bg, ratatui::style::Color::Reset);
}

#[test]
fn second_guard_is_rejected_without_restoring_the_first() {
    let _lock = guard_test_lock();
    let (first, probe) = TerminalGuard::new_mocked().expect("first guard");
    let second = TerminalGuard::new_mocked();
    assert!(second.is_err(), "nested enter must fail");
    assert!(!probe.is_restored());
    assert!(probe.is_raw_mode());
    assert!(probe.is_bracketed_paste());
    drop(first);
    assert!(probe.is_restored());
    let (third, _) = TerminalGuard::new_mocked().expect("guard after restore");
    drop(third);
}

#[test]
fn consent_layout_rejects_eighty_by_eight() {
    assert!(!mcode_tui::consent_is_readable(Viewport::new(80, 8)));
    assert!(mcode_tui::consent_is_readable(Viewport::new(80, 24)));
    let mut view = default_view();
    view.dispatch(Action::Resize(Viewport::new(80, 8)));
    let effects = view.dispatch(Action::PresentConsent(ConsentPrompt::new(
        "req-small",
        "bash",
        "hidden",
    )));
    assert!(view.state().consent().is_none());
    assert_eq!(
        effects,
        vec![Effect::ConsentResolved {
            request_id: "req-small".into(),
            choice: ConsentChoice::Deny,
        }]
    );
}

#[test]
fn second_consent_is_denied_without_replacing_the_first() {
    let mut view = default_view();
    view.dispatch(Action::PresentConsent(ConsentPrompt::new(
        "req-a", "bash", "one",
    )));
    let effects = view.dispatch(Action::PresentConsent(ConsentPrompt::new(
        "req-b", "write", "two",
    )));
    assert_eq!(
        view.state().consent().map(ConsentPrompt::request_id),
        Some("req-a")
    );
    assert_eq!(
        effects,
        vec![Effect::ConsentResolved {
            request_id: "req-b".into(),
            choice: ConsentChoice::Deny,
        }]
    );
    let first = view.dispatch(Action::ResolveConsent(ConsentChoice::AllowOnce));
    assert!(first.contains(&Effect::ConsentResolved {
        request_id: "req-a".into(),
        choice: ConsentChoice::AllowOnce,
    }));
}

#[test]
fn paste_is_ignored_while_consent_is_visible() {
    let mut view = default_view();
    view.dispatch(Action::Paste("before".into()));
    view.dispatch(Action::PresentConsent(ConsentPrompt::new(
        "req-paste",
        "bash",
        "ls",
    )));
    let outcome = view.handle_input(&Event::Paste("secret\nlines".into()));
    assert!(!outcome.is_handled());
    assert_eq!(view.state().input(), "before");
}

#[test]
fn input_window_keeps_caret_visible_on_long_paste() {
    let mut view = default_view();
    let pasted = format!(
        "{}
{}",
        "line\n".repeat(8).trim_end(),
        "宽".repeat(80)
    );
    view.dispatch(Action::Paste(pasted));
    let backend = TestBackend::new(40, 16);
    let mut terminal = Terminal::new(backend).expect("test terminal must initialize");
    terminal
        .draw(|frame| view.draw(frame))
        .expect("caret window must render");
    let screen = buffer_text(terminal.backend().buffer());
    assert!(screen.contains('宽'), "caret line missing from {screen:?}");
}

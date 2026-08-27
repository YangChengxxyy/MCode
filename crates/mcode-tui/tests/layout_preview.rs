// Rust guideline compliant 2026-08-27.

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use mcode_tui::preview::{
    PREVIEW_STATUS, PreviewOutcome, handle_preview_event, preview_registry, seed_preview_view,
};
use mcode_tui::{
    Action, ActionId, DEFAULT_SCROLLBACK_BLOCKS, Effect, MaterializeBudget, Viewport, materialize,
    reduce,
};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

fn key(code: KeyCode, modifiers: KeyModifiers) -> Event {
    Event::Key(KeyEvent::new(code, modifiers))
}

fn preview_view() -> mcode_tui::AppView {
    seed_preview_view(Viewport::new(80, 24))
}

fn materialized_line_count(view: &mcode_tui::AppView, width: usize) -> usize {
    view.state()
        .blocks()
        .iter()
        .enumerate()
        .map(|(index, block)| {
            block.to_plain_text(width).lines().count().max(1)
                + usize::from(index + 1 < view.state().blocks().len())
        })
        .sum()
}

#[test]
fn seed_uses_real_layout_state_without_a_session() {
    let view = preview_view();
    assert!(view.state().blocks().len() > 24);
    assert!(view.state().blocks().len() < DEFAULT_SCROLLBACK_BLOCKS);
    assert_eq!(view.state().status(), PREVIEW_STATUS);
    assert!(view.state().input().is_empty());
    assert!(view.state().consent().is_none());
    assert_eq!(view.state().scroll_offset(), 0);
}

#[test]
fn seed_renders_on_a_test_backend() {
    let mut view = preview_view();
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).expect("test terminal must initialize");
    terminal
        .draw(|frame| view.draw(frame))
        .expect("seeded layout must render");
}

#[test]
fn esc_and_empty_q_quit_while_typing_q_inserts() {
    let mut view = preview_view();
    assert_eq!(
        handle_preview_event(&mut view, &key(KeyCode::Esc, KeyModifiers::NONE)),
        PreviewOutcome::Quit
    );

    let mut view = preview_view();
    assert_eq!(
        handle_preview_event(&mut view, &key(KeyCode::Char('q'), KeyModifiers::NONE)),
        PreviewOutcome::Quit
    );

    let mut view = preview_view();
    view.dispatch(Action::ReplaceInput("hel".into()));
    assert_eq!(
        handle_preview_event(&mut view, &key(KeyCode::Char('q'), KeyModifiers::NONE)),
        PreviewOutcome::Continue
    );
    assert_eq!(view.state().input(), "helq");
}

#[test]
fn enter_appends_a_deterministic_local_echo() {
    let mut view = preview_view();
    let before = view.state().blocks().len();
    view.dispatch(Action::ReplaceInput("hello preview".into()));
    assert_eq!(
        handle_preview_event(&mut view, &key(KeyCode::Enter, KeyModifiers::NONE)),
        PreviewOutcome::Continue
    );
    assert!(view.state().input().is_empty());
    assert_eq!(view.state().blocks().len(), before + 2);
    let rendered = view
        .state()
        .blocks()
        .iter()
        .map(|block| block.to_plain_text(80))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(rendered.contains("You: hello preview"));
    assert!(rendered.contains("Local echo; no model or tool ran."));
    assert_eq!(view.state().scroll_offset(), 0);
}

#[test]
fn up_down_and_page_keys_scroll_bounded_history() {
    let mut view = preview_view();
    assert_eq!(
        handle_preview_event(&mut view, &key(KeyCode::Up, KeyModifiers::NONE)),
        PreviewOutcome::Continue
    );
    assert_eq!(view.state().scroll_offset(), 1);
    assert_eq!(
        handle_preview_event(&mut view, &key(KeyCode::Down, KeyModifiers::NONE)),
        PreviewOutcome::Continue
    );
    assert_eq!(view.state().scroll_offset(), 0);

    assert_eq!(
        handle_preview_event(&mut view, &key(KeyCode::PageUp, KeyModifiers::NONE)),
        PreviewOutcome::Continue
    );
    assert_eq!(view.state().scroll_offset(), 14);
    let paged = view.state().scroll_offset();
    assert_eq!(
        handle_preview_event(&mut view, &key(KeyCode::PageDown, KeyModifiers::NONE)),
        PreviewOutcome::Continue
    );
    assert!(view.state().scroll_offset() < paged);
}

#[test]
fn page_scroll_uses_exact_transcript_geometry_and_reaches_oldest_line() {
    for (viewport, input, width, height) in [
        (Viewport::new(80, 24), "", 78_usize, 14_usize),
        (Viewport::new(40, 24), "", 38, 14),
        (Viewport::new(80, 24), "a\nb\nc\nd", 78, 11),
    ] {
        let mut view = seed_preview_view(viewport);
        if !input.is_empty() {
            view.dispatch(Action::ReplaceInput(input.into()));
        }
        let max_offset = materialized_line_count(&view, width).saturating_sub(height);
        let first_page = materialize(
            view.state().blocks(),
            MaterializeBudget::new(width, height, height),
        );
        assert!(first_page.blocks_examined() < view.state().blocks().len());

        loop {
            let previous = view.state().scroll_offset();
            assert_eq!(
                handle_preview_event(&mut view, &key(KeyCode::PageUp, KeyModifiers::NONE)),
                PreviewOutcome::Continue
            );
            let expected = previous.saturating_add(height).min(max_offset);
            assert_eq!(view.state().scroll_offset(), expected);
            if expected == previous {
                break;
            }
        }

        assert_eq!(view.state().scroll_offset(), max_offset);
        let oldest = materialize(
            view.state().blocks(),
            MaterializeBudget::new(width, height, max_offset),
        );
        assert!(
            oldest
                .lines()
                .iter()
                .any(|line| line.text().starts_with("history-01:"))
        );

        if !input.is_empty() {
            view.dispatch(Action::ReplaceInput("one line".into()));
            let expanded_max = materialized_line_count(&view, width).saturating_sub(14);
            assert_eq!(view.state().scroll_offset(), expanded_max);
        }
    }
}

#[test]
fn ctrl_p_toggles_consent_without_executing() {
    let mut view = preview_view();
    let toggle = key(KeyCode::Char('p'), KeyModifiers::CONTROL);
    assert_eq!(
        handle_preview_event(&mut view, &toggle),
        PreviewOutcome::Continue
    );
    assert_eq!(
        view.state().consent().map(|prompt| prompt.tool_name()),
        Some("bash")
    );

    let allow = handle_preview_event(&mut view, &key(KeyCode::Char('1'), KeyModifiers::NONE));
    assert_eq!(allow, PreviewOutcome::Continue);
    assert!(view.state().consent().is_none());
    assert!(view.state().status().contains("allow once"));
    assert!(view.state().status().contains("not executed"));

    assert_eq!(
        handle_preview_event(&mut view, &toggle),
        PreviewOutcome::Continue
    );
    assert!(view.state().consent().is_some());
    assert_eq!(
        handle_preview_event(&mut view, &toggle),
        PreviewOutcome::Continue
    );
    assert!(view.state().consent().is_none());
    assert!(view.state().status().contains("deny"));
}

#[test]
fn esc_denies_consent_instead_of_quitting() {
    let mut view = preview_view();
    assert_eq!(
        handle_preview_event(&mut view, &key(KeyCode::Char('p'), KeyModifiers::CONTROL)),
        PreviewOutcome::Continue
    );
    assert_eq!(
        handle_preview_event(&mut view, &key(KeyCode::Esc, KeyModifiers::NONE)),
        PreviewOutcome::Continue
    );
    assert!(view.state().consent().is_none());
    assert!(view.state().status().contains("deny"));
}

#[test]
fn preview_registry_exposes_esc_and_q_quit() {
    let registry = preview_registry();
    let empty = mcode_tui::AppState::default();
    assert_eq!(
        registry.action_for_event(&key(KeyCode::Esc, KeyModifiers::NONE), &empty),
        Some(Action::Quit)
    );
    assert_eq!(
        registry.action_for_event(&key(KeyCode::Char('q'), KeyModifiers::NONE), &empty),
        Some(Action::Quit)
    );

    let typed = reduce(&empty, Action::ReplaceInput("x".into()))
        .into_parts()
        .0;
    assert_eq!(
        registry.action_for_event(&key(KeyCode::Char('q'), KeyModifiers::NONE), &typed),
        Some(Action::Insert('q'))
    );
    assert_eq!(
        registry
            .binding_for(ActionId::Quit, &empty)
            .map(|binding| { binding.action() }),
        Some(ActionId::Quit)
    );
}

#[test]
fn zero_transcript_resize_preserves_scrollback_position() {
    let mut view = preview_view();
    view.dispatch(Action::ScrollBy(10));
    let parked = view.state().scroll_offset();
    assert_eq!(parked, 10);

    view.dispatch(Action::Resize(Viewport::new(1, 2)));
    assert_eq!(view.state().scroll_offset(), parked);
    assert!(view.dispatch(Action::ScrollBy(1)).is_empty());
    assert_eq!(view.state().scroll_offset(), parked);

    view.dispatch(Action::Resize(Viewport::new(80, 24)));
    assert_eq!(view.state().scroll_offset(), parked);
}

#[test]
fn scroll_by_clamps_and_replace_returns_to_the_tail() {
    let mut view = preview_view();
    let maxed = view.dispatch(Action::ScrollBy(i32::MAX));
    assert_eq!(
        maxed,
        vec![Effect::Redraw(mcode_tui::Invalidation::Content)]
    );
    let parked = view.state().scroll_offset();
    assert!(parked > 0);
    assert!(
        view.dispatch(Action::ScrollBy(1)).is_empty(),
        "offset must clamp at the oldest retained line"
    );

    let same_blocks = view.state().blocks().to_vec();
    assert_eq!(
        view.dispatch(Action::ReplaceBlocks(same_blocks)),
        vec![Effect::Redraw(mcode_tui::Invalidation::Content)]
    );
    assert_eq!(view.state().scroll_offset(), 0);

    view.dispatch(Action::ScrollBy(i32::MAX));
    let mut changed_blocks = view.state().blocks().to_vec();
    changed_blocks.push(mcode_render::RenderBlock::Text("fresh".into()));
    view.dispatch(Action::ReplaceBlocks(changed_blocks));
    assert_eq!(view.state().scroll_offset(), 0);
}

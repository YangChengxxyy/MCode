//! Interactive layout preview for `mcode-tui`.
//!
//! Run with `cargo run -p mcode-tui --example layout_preview`.
//!
//! Keys: Esc or q quit (`q` when input is empty), Up/Down and PageUp/PageDown
//! scroll, typing edits input, Enter appends a local echo, Ctrl+P toggles the
//! consent dialog. F1 opens help. No provider, network, or session access.

// Rust guideline compliant 2026-08-27.

use std::io::{self, stdout};

use crossterm::{event, terminal};
use mcode_tui::preview::{PreviewOutcome, handle_preview_event, seed_preview_view};
use mcode_tui::{TerminalGuard, Viewport};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

fn main() -> io::Result<()> {
    let guard = TerminalGuard::enter()?;
    let result = run_preview();
    guard.restore();
    result
}

fn run_preview() -> io::Result<()> {
    let (width, height) = terminal::size()?;
    let mut view = seed_preview_view(Viewport::new(width, height));
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout()))?;
    loop {
        terminal.draw(|frame| view.draw(frame))?;
        let event = event::read()?;
        if handle_preview_event(&mut view, &event) == PreviewOutcome::Quit {
            break;
        }
    }
    Ok(())
}

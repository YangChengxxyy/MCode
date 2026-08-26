//! Ratatui adapter for the pure MCode view state.
//!
//! Rendering consumes [`mcode_render::RenderBlock`] through its bounded plain
//! fallback. When Unicode is disabled, borders and all visible text degrade to
//! ASCII. The adapter performs no input, terminal setup, session calls, or
//! other effects, which keeps `TestBackend` tests deterministic.

// Rust guideline compliant 2026-08-26.

use mcode_render::{MAX_PLAIN_WIDTH, RenderBlock, sanitize_terminal_text, truncate_display_width};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols::border;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};

use crate::actions::ActionRegistry;
use crate::hints;
use crate::labels::{HELP_TITLE, INPUT_TITLE, TRANSCRIPT_TITLE};
use crate::logo::{TerminalLogo, terminal_logo};
use crate::state::AppState;
use crate::terminal::{ColorCapability, TerminalCapabilities, rgb_to_ansi256};
use crate::theme::{Rgb, SemanticToken, Theme};

/// ASCII border used when the host prohibits Unicode terminal glyphs.
const ASCII_BORDER_SET: border::Set = border::Set {
    top_left: "+",
    top_right: "+",
    bottom_left: "+",
    bottom_right: "+",
    vertical_left: "|",
    vertical_right: "|",
    horizontal_top: "-",
    horizontal_bottom: "-",
};

/// Maps one semantic sRGB color to the selected terminal color depth.
#[must_use]
pub fn terminal_color(color: Rgb, capability: ColorCapability) -> Color {
    match capability {
        ColorCapability::NoColor => Color::Reset,
        ColorCapability::Basic => nearest_basic_color(color),
        ColorCapability::Ansi256 => Color::Indexed(rgb_to_ansi256(color)),
        ColorCapability::TrueColor => Color::Rgb(color.red, color.green, color.blue),
    }
}

/// Builds a foreground style for one semantic token.
#[must_use]
pub fn token_style(
    theme: &Theme,
    token: SemanticToken,
    capabilities: TerminalCapabilities,
) -> Style {
    if capabilities.supports_color() {
        Style::default().fg(terminal_color(theme.color(token), capabilities.color()))
    } else {
        Style::default()
    }
}

/// Draws the complete foundation view into a Ratatui frame.
///
/// The layout contains a responsive logo, render-block viewport, status bar,
/// and input placeholder or buffer. Key hints come from `registry`, the same
/// bindings that dispatch input. Small areas are clipped by Ratatui without
/// panicking.
pub fn draw(
    frame: &mut Frame<'_>,
    state: &AppState,
    registry: &ActionRegistry,
    theme: &Theme,
    capabilities: TerminalCapabilities,
) {
    let area = frame.area();
    if area.is_empty() {
        return;
    }

    frame.render_widget(
        Block::new().style(background_style(
            theme,
            SemanticToken::Background,
            capabilities,
        )),
        area,
    );

    let logo = terminal_logo(area.width, capabilities);
    let status_height = u16::from(area.height > 0);
    let input_height = if area.height >= 4 {
        3
    } else {
        area.height.saturating_sub(status_height)
    };
    let minimum_body_height = 1;
    let logo_budget = area
        .height
        .saturating_sub(status_height + input_height + minimum_body_height);
    let logo_height = u16::try_from(logo.lines().len())
        .unwrap_or(u16::MAX)
        .min(logo_budget);

    let [logo_area, body_area, input_area, status_area] = Layout::vertical([
        Constraint::Length(logo_height),
        Constraint::Fill(1),
        Constraint::Length(input_height),
        Constraint::Length(status_height),
    ])
    .areas(area);

    render_logo(frame, logo_area, &logo, theme, capabilities);
    render_body(frame, body_area, state, registry, theme, capabilities);
    render_input(frame, input_area, state, theme, capabilities);
    render_status(frame, status_area, state, registry, theme, capabilities);
}

fn render_logo(
    frame: &mut Frame<'_>,
    area: Rect,
    logo: &TerminalLogo,
    theme: &Theme,
    capabilities: TerminalCapabilities,
) {
    let lines = logo
        .lines()
        .iter()
        .take(usize::from(area.height))
        .map(|line| {
            Line::from(
                line.spans()
                    .iter()
                    .map(|span| {
                        let mut style = token_style(theme, span.token(), capabilities);
                        if span.is_bold() {
                            style = style.add_modifier(Modifier::BOLD);
                        }
                        Span::styled(span.text().to_owned(), style)
                    })
                    .collect::<Vec<_>>(),
            )
        })
        .collect::<Vec<_>>();
    frame.render_widget(Paragraph::new(lines), area);
}

fn render_body(
    frame: &mut Frame<'_>,
    area: Rect,
    state: &AppState,
    registry: &ActionRegistry,
    theme: &Theme,
    capabilities: TerminalCapabilities,
) {
    if area.is_empty() {
        return;
    }

    let panel = bordered_block(capabilities)
        .title(TRANSCRIPT_TITLE)
        .style(panel_style(theme, capabilities));
    let inner = panel.inner(area);
    frame.render_widget(panel, area);

    let lines = if state.is_help_visible() {
        help_lines(theme, capabilities, registry, state, inner.width.into())
    } else {
        block_lines(state, inner, theme, capabilities)
    };
    frame.render_widget(Paragraph::new(lines), inner);
}

fn help_lines(
    theme: &Theme,
    capabilities: TerminalCapabilities,
    registry: &ActionRegistry,
    state: &AppState,
    width: usize,
) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::styled(
            HELP_TITLE,
            token_style(theme, SemanticToken::Accent, capabilities).add_modifier(Modifier::BOLD),
        ),
        Line::raw(""),
    ];
    // Dynamic key labels can contain any bound character, so every entry
    // passes the same sanitize-and-ASCII-degrade path as other text to keep
    // the full-buffer ASCII contract when Unicode is disabled.
    lines.extend(hints::help_lines(registry, state).into_iter().map(|entry| {
        Line::styled(
            bounded_terminal_line(entry, width, capabilities),
            token_style(theme, SemanticToken::TextPrimary, capabilities),
        )
    }));
    lines
}

fn block_lines(
    state: &AppState,
    area: Rect,
    theme: &Theme,
    capabilities: TerminalCapabilities,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let line_limit = usize::from(area.height);
    let width = usize::from(area.width);

    for block in state.blocks() {
        if lines.len() >= line_limit {
            break;
        }
        if !lines.is_empty() {
            lines.push(Line::raw(""));
        }
        let default_token = block_token(block);
        for text in block.to_plain_text(width).lines() {
            if lines.len() >= line_limit {
                break;
            }
            let token = match block {
                RenderBlock::Diff(_) if text.starts_with('+') => SemanticToken::DiffAdded,
                RenderBlock::Diff(_) if text.starts_with('-') => SemanticToken::DiffRemoved,
                _ => default_token,
            };
            lines.push(Line::styled(
                bounded_terminal_line(text, width, capabilities),
                token_style(theme, token, capabilities),
            ));
        }
    }

    lines
}

fn block_token(block: &RenderBlock) -> SemanticToken {
    match block {
        RenderBlock::Text(_) | RenderBlock::Table(_) | RenderBlock::Tree(_) => {
            SemanticToken::TextPrimary
        }
        RenderBlock::Markdown(_) => SemanticToken::MarkdownCode,
        RenderBlock::Diff(_) => SemanticToken::DiffContext,
        RenderBlock::Progress(_) => SemanticToken::ProgressFill,
        RenderBlock::Error(_) => SemanticToken::Error,
        RenderBlock::Widget(_) => SemanticToken::ToolOutput,
        _ => SemanticToken::TextPrimary,
    }
}

fn render_input(
    frame: &mut Frame<'_>,
    area: Rect,
    state: &AppState,
    theme: &Theme,
    capabilities: TerminalCapabilities,
) {
    if area.is_empty() {
        return;
    }

    let panel = bordered_block(capabilities)
        .title(INPUT_TITLE)
        .style(input_panel_style(theme, capabilities));
    let inner = panel.inner(area);
    frame.render_widget(panel, area);

    let (text, token) = if state.input().is_empty() {
        (state.input_placeholder(), SemanticToken::TextDim)
    } else {
        (state.input(), SemanticToken::InputText)
    };
    let text = bounded_terminal_line(text, inner.width.into(), capabilities);
    frame.render_widget(
        Paragraph::new(text).style(token_style(theme, token, capabilities)),
        inner,
    );
}

fn render_status(
    frame: &mut Frame<'_>,
    area: Rect,
    state: &AppState,
    registry: &ActionRegistry,
    theme: &Theme,
    capabilities: TerminalCapabilities,
) {
    if area.is_empty() {
        return;
    }

    let key_hints = hints::status_key_hints(registry, state);
    let status = if key_hints.is_empty() {
        format!(" {}", state.status())
    } else {
        format!(" {}  {key_hints}", state.status())
    };
    let status = bounded_terminal_line(status, area.width.into(), capabilities);
    frame.render_widget(
        Paragraph::new(status).style(foreground_background_style(
            theme,
            SemanticToken::StatusText,
            SemanticToken::StatusBackground,
            capabilities,
        )),
        area,
    );
}

fn bordered_block(capabilities: TerminalCapabilities) -> Block<'static> {
    let block = Block::bordered();
    if capabilities.supports_unicode() {
        block
    } else {
        block.border_set(ASCII_BORDER_SET)
    }
}

fn bounded_terminal_line(
    text: impl AsRef<str>,
    width: usize,
    capabilities: TerminalCapabilities,
) -> String {
    let clean = sanitize_terminal_text(text);
    let line = clean.lines().next().unwrap_or_default();
    if capabilities.supports_unicode() {
        truncate_display_width(line, width)
    } else {
        let ascii = line
            .chars()
            .map(|character| match character {
                '…' => '.',
                _ if character.is_ascii() => character,
                _ => '?',
            })
            .collect::<String>();
        truncate_ascii_line(&ascii, width.min(MAX_PLAIN_WIDTH))
    }
}

fn truncate_ascii_line(text: &str, width: usize) -> String {
    if text.len() <= width {
        return text.to_owned();
    }
    if width == 0 {
        return String::new();
    }

    let mut rendered = String::with_capacity(width);
    rendered.push_str(&text[..width - 1]);
    rendered.push('.');
    rendered
}

fn panel_style(theme: &Theme, capabilities: TerminalCapabilities) -> Style {
    foreground_background_style(
        theme,
        SemanticToken::Border,
        SemanticToken::Surface,
        capabilities,
    )
}

fn input_panel_style(theme: &Theme, capabilities: TerminalCapabilities) -> Style {
    foreground_background_style(
        theme,
        SemanticToken::BorderFocus,
        SemanticToken::InputBackground,
        capabilities,
    )
}

fn background_style(
    theme: &Theme,
    token: SemanticToken,
    capabilities: TerminalCapabilities,
) -> Style {
    if capabilities.supports_color() {
        Style::default().bg(terminal_color(theme.color(token), capabilities.color()))
    } else {
        Style::default()
    }
}

fn foreground_background_style(
    theme: &Theme,
    foreground: SemanticToken,
    background: SemanticToken,
    capabilities: TerminalCapabilities,
) -> Style {
    if capabilities.supports_color() {
        Style::default()
            .fg(terminal_color(
                theme.color(foreground),
                capabilities.color(),
            ))
            .bg(terminal_color(
                theme.color(background),
                capabilities.color(),
            ))
    } else {
        Style::default()
    }
}

fn nearest_basic_color(color: Rgb) -> Color {
    const BASIC: [(Rgb, Color); 16] = [
        (Rgb::new(0x00, 0x00, 0x00), Color::Black),
        (Rgb::new(0x80, 0x00, 0x00), Color::Red),
        (Rgb::new(0x00, 0x80, 0x00), Color::Green),
        (Rgb::new(0x80, 0x80, 0x00), Color::Yellow),
        (Rgb::new(0x00, 0x00, 0x80), Color::Blue),
        (Rgb::new(0x80, 0x00, 0x80), Color::Magenta),
        (Rgb::new(0x00, 0x80, 0x80), Color::Cyan),
        (Rgb::new(0xc0, 0xc0, 0xc0), Color::Gray),
        (Rgb::new(0x80, 0x80, 0x80), Color::DarkGray),
        (Rgb::new(0xff, 0x00, 0x00), Color::LightRed),
        (Rgb::new(0x00, 0xff, 0x00), Color::LightGreen),
        (Rgb::new(0xff, 0xff, 0x00), Color::LightYellow),
        (Rgb::new(0x00, 0x00, 0xff), Color::LightBlue),
        (Rgb::new(0xff, 0x00, 0xff), Color::LightMagenta),
        (Rgb::new(0x00, 0xff, 0xff), Color::LightCyan),
        (Rgb::new(0xff, 0xff, 0xff), Color::White),
    ];

    BASIC
        .iter()
        .min_by_key(|(candidate, _)| color_distance(color, *candidate))
        .map_or(Color::White, |(_, mapped)| *mapped)
}

fn color_distance(first: Rgb, second: Rgb) -> i32 {
    let red = i32::from(first.red) - i32::from(second.red);
    let green = i32::from(first.green) - i32::from(second.green);
    let blue = i32::from(first.blue) - i32::from(second.blue);
    let first_chroma = i32::from(first.red.max(first.green).max(first.blue))
        - i32::from(first.red.min(first.green).min(first.blue));
    let second_chroma = i32::from(second.red.max(second.green).max(second.blue))
        - i32::from(second.red.min(second.green).min(second.blue));
    let chroma = first_chroma - second_chroma;
    let red_green = (i32::from(first.red) - i32::from(first.green))
        - (i32::from(second.red) - i32::from(second.green));
    let green_blue = (i32::from(first.green) - i32::from(first.blue))
        - (i32::from(second.green) - i32::from(second.blue));
    let blue_red = (i32::from(first.blue) - i32::from(first.red))
        - (i32::from(second.blue) - i32::from(second.red));
    red * red
        + green * green
        + blue * blue
        + 2 * chroma * chroma
        + 2 * (red_green * red_green + green_blue * green_blue + blue_red * blue_red)
}

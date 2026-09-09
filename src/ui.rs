//! Rendering.
//!
//! ratatui owns the screen: we draw into an off-screen buffer and it diffs
//! against the previous frame, so redraws touch only the cells that changed
//! and the display never flickers.

use crate::app::{App, MessageKind};
use crate::config::LineNumbers;
use crate::mode::{Mode, VisualKind};
use crate::syntax::to_tui_color;
use crate::text::{self, Position};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

/// Colours pulled from the syntect theme so the chrome matches the text.
struct Palette {
    foreground: Color,
    background: Color,
    selection: Color,
    line_number: Color,
    line_number_active: Color,
    cursorline: Color,
    search: Color,
}

impl Palette {
    fn from(app: &App) -> Self {
        let settings = &app.theme.settings;
        let background = if app.config.appearance.theme_background {
            settings
                .background
                .map(to_tui_color)
                .unwrap_or(Color::Reset)
        } else {
            Color::Reset
        };
        Self {
            foreground: settings
                .foreground
                .map(to_tui_color)
                .unwrap_or(Color::Reset),
            background,
            selection: settings
                .selection
                .map(to_tui_color)
                .unwrap_or(Color::DarkGray),
            line_number: settings
                .gutter_foreground
                .map(to_tui_color)
                .unwrap_or(Color::DarkGray),
            line_number_active: settings
                .foreground
                .map(to_tui_color)
                .unwrap_or(Color::White),
            cursorline: settings
                .line_highlight
                .map(to_tui_color)
                .unwrap_or(Color::Indexed(236)),
            search: settings
                .find_highlight
                .map(to_tui_color)
                .unwrap_or(Color::Rgb(120, 90, 0)),
        }
    }
}

pub fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(area);

    let palette = Palette::from(app);
    let gutter = app.gutter_width();

    // Record the viewport so motions and scrolling agree with what is drawn.
    app.viewport.height = chunks[0].height as usize;
    app.viewport.text_width = (chunks[0].width as usize).saturating_sub(gutter);
    app.scroll_to_cursor();

    if app.buffer().is_empty_scratch() && app.buffers.len() == 1 {
        draw_welcome(frame, chunks[0], &palette);
    } else {
        draw_text(frame, chunks[0], app, &palette, gutter);
        place_cursor(frame, chunks[0], app, gutter);
    }

    draw_status(frame, chunks[1], app, &palette);
    draw_message(frame, chunks[2], app, &palette);

    if app.overlay.is_some() {
        draw_overlay(frame, area, app, &palette);
    }
    if app.mode.is_prompt() {
        place_prompt_cursor(frame, chunks[2], app);
    }
}

fn draw_text(frame: &mut Frame, area: Rect, app: &mut App, palette: &Palette, gutter: usize) {
    let height = area.height as usize;
    let width = area.width as usize;
    let tab_width = app.config.editor.tab_width;
    let view_top = app.buffer().view_top;
    let view_left = app.buffer().view_left;
    let cursor = app.buffer().cursor;
    let total_lines = app.buffer().line_count();
    let last_visible = (view_top + height)
        .saturating_sub(1)
        .min(total_lines.saturating_sub(1));

    // Syntax highlighting for exactly the visible lines.
    let highlights = {
        let syntax = app
            .engine
            .syntax_by_name(&app.buffer().syntax_name)
            .map(|s| s.to_owned());
        match syntax {
            Some(syntax) => {
                let App {
                    buffers,
                    current,
                    engine,
                    theme,
                    ..
                } = app;
                let buffer = &mut buffers[*current];
                let rope = buffer.rope.clone();
                buffer
                    .syntax_cache
                    .highlight(&rope, engine, &syntax, theme, view_top, last_visible)
            }
            None => Vec::new(),
        }
    };

    let selection = app.mode.is_visual().then(|| app.visual_range());
    let selection_lines = app.mode.is_visual().then(|| app.visual_lines());
    let linewise_selection = app.mode == Mode::Visual(VisualKind::Line);

    let mut rows: Vec<Line> = Vec::with_capacity(height);
    for row in 0..height {
        let line_index = view_top + row;
        if line_index >= total_lines {
            rows.push(Line::from(Span::styled(
                "~",
                Style::default().fg(palette.line_number),
            )));
            continue;
        }

        let is_cursor_line = line_index == cursor.line;
        let row_background =
            if app.config.editor.cursorline && is_cursor_line && selection.is_none() {
                Some(palette.cursorline)
            } else {
                None
            };

        let mut spans: Vec<Span> = Vec::new();
        if gutter > 0 {
            spans.push(gutter_span(app, palette, line_index, cursor.line, gutter));
        }

        let content = text::line(&app.buffer().rope, line_index);
        let line_highlights = highlights.get(line_index.saturating_sub(view_top));
        let search_matches = if app.search_highlight && app.search.is_active() {
            app.search.matches_on_line(&app.buffer().rope, line_index)
        } else {
            Vec::new()
        };
        let line_start_char = app.buffer().rope.line_to_char(line_index);

        let text_width = width.saturating_sub(gutter);
        let mut screen_column = 0usize;
        let mut emitted = 0usize;
        let mut pending = String::new();
        let mut pending_style: Option<Style> = None;

        for (column, ch) in content.chars().enumerate() {
            let char_width = text::char_width(ch, screen_column, tab_width);
            let starts_at = screen_column;
            screen_column += char_width;

            // Horizontal clipping, in screen columns.
            if starts_at + char_width <= view_left {
                continue;
            }
            if emitted >= text_width {
                break;
            }

            let mut style = line_highlights
                .and_then(|spans| {
                    spans
                        .iter()
                        .find(|s| column >= s.start && column < s.end)
                        .map(|s| s.style)
                })
                .unwrap_or_else(|| Style::default().fg(palette.foreground));

            if let Some(background) = row_background {
                style = style.bg(background);
            } else if palette.background != Color::Reset {
                style = style.bg(palette.background);
            }

            if search_matches
                .iter()
                .any(|m| column >= m.start && column < m.end)
            {
                style = style.bg(palette.search);
            }

            // Another participant's selection, drawn dimmer than your own.
            if app.config.session.show_remote_cursors {
                for remote in &app.remote_cursors {
                    if remote.id == app.rendering_as || remote.buffer_index != app.current {
                        continue;
                    }
                    if let Some((from, to)) = remote.selection {
                        let here = Position::new(line_index, column);
                        if here >= from && here <= to {
                            style = style.bg(Color::Indexed(238));
                        }
                    }
                    if remote.cursor.line == line_index && remote.cursor.col == column {
                        style = style.bg(Color::Indexed(remote.color)).fg(Color::Black);
                    }
                }
            }

            if let Some(range) = &selection {
                let char_index = line_start_char + column;
                let selected = if linewise_selection {
                    selection_lines
                        .map(|(first, last)| line_index >= first && line_index <= last)
                        .unwrap_or(false)
                } else {
                    char_index >= range.start && char_index < range.end
                };
                if selected {
                    style = style.bg(palette.selection);
                }
            }

            // Tabs and control characters are rendered, not passed through.
            let rendered = match ch {
                '\t' => " ".repeat(char_width),
                c if (c as u32) < 0x20 => "·".to_string(),
                c => c.to_string(),
            };
            let visible_width = if starts_at < view_left {
                // Partially clipped: pad the remainder.
                let overlap = view_left - starts_at;
                " ".repeat(char_width.saturating_sub(overlap))
            } else {
                rendered
            };
            let visible_width_cols = visible_width.chars().count().max(1);
            let room = text_width - emitted;
            let visible = if char_width > room && ch != '\t' {
                " ".repeat(room)
            } else {
                visible_width
            };
            let _ = visible_width_cols;

            if pending_style == Some(style) {
                pending.push_str(&visible);
            } else {
                if let Some(previous) = pending_style.take() {
                    spans.push(Span::styled(std::mem::take(&mut pending), previous));
                }
                pending = visible.clone();
                pending_style = Some(style);
            }
            emitted += char_width.min(room);
        }
        if let Some(style) = pending_style {
            spans.push(Span::styled(pending, style));
        }

        // Tag the line with the names of anyone whose caret is on it, so you
        // can tell at a glance who is where.
        if app.config.session.show_remote_cursors {
            let names: Vec<&crate::session::RemoteCursor> = app
                .remote_cursors
                .iter()
                .filter(|remote| {
                    remote.id != app.rendering_as
                        && remote.buffer_index == app.current
                        && remote.cursor.line == line_index
                })
                .collect();
            for remote in names {
                spans.push(Span::styled(
                    format!(" {} ", remote.name),
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::Indexed(remote.color)),
                ));
                emitted += remote.name.chars().count() + 2;
            }
        }

        // Extend the cursor line's highlight across the rest of the row.
        if let Some(background) = row_background {
            let remaining = width.saturating_sub(gutter + emitted);
            if remaining > 0 {
                spans.push(Span::styled(
                    " ".repeat(remaining),
                    Style::default().bg(background),
                ));
            }
        }

        rows.push(Line::from(spans));
    }

    frame.render_widget(Paragraph::new(rows), area);
}

fn gutter_span<'a>(
    app: &App,
    palette: &Palette,
    line_index: usize,
    cursor_line: usize,
    gutter: usize,
) -> Span<'a> {
    let is_cursor_line = line_index == cursor_line;
    let label = match app.config.editor.line_numbers {
        LineNumbers::None => String::new(),
        LineNumbers::Absolute => (line_index + 1).to_string(),
        LineNumbers::Relative => {
            if is_cursor_line {
                "0".to_string()
            } else {
                line_index.abs_diff(cursor_line).to_string()
            }
        }
        LineNumbers::Hybrid => {
            if is_cursor_line {
                (line_index + 1).to_string()
            } else {
                line_index.abs_diff(cursor_line).to_string()
            }
        }
    };
    let text = format!("{:>width$}  ", label, width = gutter.saturating_sub(2));
    let mut style = Style::default().fg(if is_cursor_line {
        palette.line_number_active
    } else {
        palette.line_number
    });
    if is_cursor_line {
        style = style.add_modifier(Modifier::BOLD);
        if app.config.editor.cursorline {
            style = style.bg(palette.cursorline);
        }
    } else if palette.background != Color::Reset {
        style = style.bg(palette.background);
    }
    Span::styled(text, style)
}

fn place_cursor(frame: &mut Frame, area: Rect, app: &App, gutter: usize) {
    if app.mode.is_prompt() || app.overlay.is_some() {
        return;
    }
    let buffer = app.buffer();
    let column = text::screen_col(
        text::line(&buffer.rope, buffer.cursor.line),
        buffer.cursor.col,
        app.config.editor.tab_width,
    );
    let x = area.x + (gutter + column.saturating_sub(buffer.view_left)) as u16;
    let y = area.y + buffer.cursor.line.saturating_sub(buffer.view_top) as u16;
    if x < area.right() && y < area.bottom() {
        frame.set_cursor_position((x, y));
    }
}

fn draw_welcome(frame: &mut Frame, area: Rect, palette: &Palette) {
    let version = env!("CARGO_PKG_VERSION");
    let headings = [format!("miv {version}"), "a modal text editor".to_string()];
    let hints = [
        (":help", "key reference"),
        (":e {file}", "open a file"),
        (":q", "quit"),
    ];

    // Centre the block as a whole rather than each line, so the two columns
    // of hints stay aligned with each other.
    let key_width = hints.iter().map(|(key, _)| key.len()).max().unwrap_or(0);
    let block_width = hints
        .iter()
        .map(|(_, text)| key_width + 2 + text.len())
        .chain(headings.iter().map(|heading| heading.len()))
        .max()
        .unwrap_or(0);
    let left_pad = " ".repeat((area.width as usize).saturating_sub(block_width) / 2);

    let top = (area.height as usize).saturating_sub(headings.len() + hints.len() + 2) / 3;
    let mut lines: Vec<Line> = vec![Line::from(""); top];
    for (index, heading) in headings.iter().enumerate() {
        let style = if index == 0 {
            Style::default()
                .fg(palette.line_number_active)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(palette.line_number)
        };
        let centred = " ".repeat((area.width as usize).saturating_sub(heading.len()) / 2);
        lines.push(Line::from(Span::styled(
            format!("{centred}{heading}"),
            style,
        )));
        lines.push(Line::from(""));
    }
    for (key, description) in hints {
        lines.push(Line::from(vec![
            Span::styled(
                format!("{left_pad}{key:<key_width$}  "),
                Style::default().fg(palette.line_number_active),
            ),
            Span::styled(description, Style::default().fg(palette.line_number)),
        ]));
    }
    frame.render_widget(Paragraph::new(lines), area);
}

fn draw_status(frame: &mut Frame, area: Rect, app: &App, palette: &Palette) {
    let buffer = app.buffer();
    let mode = app.mode;
    let mode_style = Style::default()
        .fg(Color::Black)
        .bg(mode.color())
        .add_modifier(Modifier::BOLD);
    let bar_style = Style::default()
        .fg(palette.foreground)
        .bg(Color::Indexed(238));

    let mut flags = String::new();
    if buffer.is_modified() {
        flags.push_str(" [+]");
    }
    if buffer.is_new_file {
        flags.push_str(" [New]");
    }

    let left = format!(" {}{}", buffer.display_name(), flags);
    let column_display = text::screen_col(
        text::line(&buffer.rope, buffer.cursor.line),
        buffer.cursor.col,
        app.config.editor.tab_width,
    ) + 1;
    let percentage = if buffer.line_count() <= 1 {
        100
    } else {
        (buffer.cursor.line + 1) * 100 / buffer.line_count()
    };
    let right = format!(
        "{}  {}:{}  {}%  {}/{} ",
        buffer.syntax_name,
        buffer.cursor.line + 1,
        column_display,
        percentage,
        app.current + 1,
        app.buffers.len()
    );

    let mut middle = String::new();
    if let Some(guests) = app.shared_guests {
        middle.push_str(&format!(
            "shared · {guests} guest{}  ",
            if guests == 1 { "" } else { "s" }
        ));
    }
    if let Some(recording) = &app.recording {
        middle.push_str(&format!("recording @{}  ", recording.register));
    }
    if !app.pending.display.is_empty() {
        middle.push_str(&app.pending.display);
        middle.push_str("  ");
    }
    if app.pending.operator.is_some() {
        middle.push_str("op-pending  ");
    }

    let mode_label = format!(" {} ", mode.label());
    let used = mode_label.chars().count() + left.chars().count() + right.chars().count();
    // Always keep a separator, even when the bar is too narrow for
    // everything; ratatui truncates the overflow on the right.
    let gap = (area.width as usize)
        .saturating_sub(used + middle.chars().count())
        .max(1);

    let line = Line::from(vec![
        Span::styled(mode_label, mode_style),
        Span::styled(left, bar_style),
        Span::styled(" ".repeat(gap), bar_style),
        Span::styled(middle, bar_style.fg(Color::Yellow)),
        Span::styled(right, bar_style),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

fn draw_message(frame: &mut Frame, area: Rect, app: &App, palette: &Palette) {
    if let Some(prompt) = &app.prompt {
        let line = Line::from(vec![
            Span::raw(prompt.kind.sigil().to_string()),
            Span::raw(prompt.input.clone()),
        ]);
        frame.render_widget(Paragraph::new(line), area);
        return;
    }

    if let Some(message) = &app.message {
        let style = match message.kind {
            MessageKind::Error => Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            MessageKind::Info => Style::default().fg(palette.foreground),
        };
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(message.text.clone(), style))),
            area,
        );
        return;
    }

    let hint = match app.mode {
        Mode::Insert => "-- INSERT --",
        Mode::Replace => "-- REPLACE --",
        Mode::Visual(VisualKind::Char) => "-- VISUAL --",
        Mode::Visual(VisualKind::Line) => "-- VISUAL LINE --",
        _ => "",
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            hint,
            Style::default().add_modifier(Modifier::BOLD),
        ))),
        area,
    );
}

fn place_prompt_cursor(frame: &mut Frame, area: Rect, app: &App) {
    if let Some(prompt) = &app.prompt {
        let x = area.x + 1 + prompt.cursor as u16;
        if x < area.right() {
            frame.set_cursor_position((x, area.y));
        }
    }
}

fn draw_overlay(frame: &mut Frame, area: Rect, app: &App, palette: &Palette) {
    let Some(overlay) = &app.overlay else {
        return;
    };
    let width = area.width.saturating_sub(4).clamp(20, 90);
    let content_height = overlay.lines.len() as u16 + 2;
    let height = content_height.min(area.height.saturating_sub(2)).max(3);
    let popup = Rect {
        x: area.x + (area.width.saturating_sub(width)) / 2,
        y: area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    };

    let visible = height.saturating_sub(2) as usize;
    let max_scroll = overlay.lines.len().saturating_sub(visible);
    let scroll = overlay.scroll.min(max_scroll);
    let lines: Vec<Line> = overlay
        .lines
        .iter()
        .skip(scroll)
        .take(visible)
        .map(|line| {
            // Section headings in the help list are uppercase with no keys.
            let is_heading = !line.trim().is_empty()
                && line.trim() == line.trim().to_uppercase()
                && line
                    .trim()
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c == ' ');
            let style = if is_heading {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(palette.foreground)
            };
            Line::from(Span::styled(line.clone(), style))
        })
        .collect();

    let block = Block::default()
        .borders(Borders::ALL)
        .title(overlay.title.clone())
        .style(Style::default().bg(Color::Indexed(235)));
    frame.render_widget(Clear, popup);
    frame.render_widget(Paragraph::new(lines).block(block), popup);
}

/// Mode-appropriate cursor shape, so the mode is visible without reading the
/// status line.
pub fn cursor_style(mode: Mode) -> crossterm::cursor::SetCursorStyle {
    use crossterm::cursor::SetCursorStyle;
    match mode {
        Mode::Insert => SetCursorStyle::BlinkingBar,
        Mode::Replace => SetCursorStyle::BlinkingUnderScore,
        _ => SetCursorStyle::SteadyBlock,
    }
}

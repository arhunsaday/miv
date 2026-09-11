//! Drawing the sidebar.

use super::Palette;
use crate::app::App;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

pub fn draw(frame: &mut Frame, area: Rect, app: &App, palette: &Palette) {
    let sidebar = &app.workspace.sidebar;
    let focused = sidebar.focused;
    let border = if focused {
        palette.line_number_active
    } else {
        palette.line_number
    };
    let block = Block::default()
        .borders(Borders::RIGHT)
        .border_style(Style::default().fg(border));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let mut lines: Vec<Line> = Vec::with_capacity(inner.height as usize);
    lines.push(Line::from(Span::styled(
        format!(" {} ", sidebar.view.title().to_uppercase()),
        Style::default()
            .fg(if focused {
                palette.line_number_active
            } else {
                palette.line_number
            })
            .add_modifier(Modifier::BOLD),
    )));

    if sidebar.view == crate::view::sidebar::View::Chat {
        draw_chat(frame, inner, lines, app, palette);
        return;
    }

    let explorer = &sidebar.explorer;
    if let Some(error) = &explorer.error {
        lines.push(Line::from(Span::styled(
            format!(" {error}"),
            Style::default().fg(Color::Red),
        )));
    }

    let rows = inner.height.saturating_sub(1) as usize;
    let start = explorer.scroll.min(explorer.entries().len());
    let width = inner.width as usize;

    for (offset, entry) in explorer.entries().iter().skip(start).take(rows).enumerate() {
        let selected = start + offset == explorer.selected;
        let marker = if entry.is_dir {
            if entry.expanded {
                "▾ "
            } else {
                "▸ "
            }
        } else {
            "  "
        };
        let indent = "  ".repeat(entry.depth);
        let label = format!("{indent}{marker}{}", entry.name);
        let label: String = label.chars().take(width.saturating_sub(1)).collect();
        let padding = width.saturating_sub(label.chars().count() + 1);

        let mut style = Style::default().fg(if entry.is_dir {
            palette.line_number_active
        } else {
            palette.foreground
        });
        if entry.is_dir {
            style = style.add_modifier(Modifier::BOLD);
        }
        if selected {
            style = if focused {
                Style::default()
                    .fg(Color::Black)
                    .bg(palette.line_number_active)
            } else {
                style.bg(Color::Indexed(237))
            };
        }
        lines.push(Line::from(vec![
            Span::styled(format!(" {label}"), style),
            Span::styled(" ".repeat(padding), style),
        ]));
    }

    frame.render_widget(Paragraph::new(lines), inner);
}

/// The AI transcript, wrapped to the sidebar's width.
fn draw_chat(
    frame: &mut Frame,
    area: Rect,
    mut lines: Vec<Line<'static>>,
    app: &App,
    palette: &Palette,
) {
    let width = (area.width as usize).saturating_sub(2).max(8);

    if app.conversation.is_empty() {
        lines.push(Line::from(Span::styled(
            " nothing yet",
            Style::default().fg(palette.line_number),
        )));
        lines.push(Line::from(Span::styled(
            " :ai <what to do>",
            Style::default().fg(palette.line_number),
        )));
    }

    for turn in app.conversation.turns() {
        let colour = match turn {
            crate::ai::Turn::You(_) => palette.line_number_active,
            crate::ai::Turn::Assistant(_) => Color::Green,
            crate::ai::Turn::Note(_) => palette.line_number,
        };
        lines.push(Line::from(Span::styled(
            format!(" {}", turn.speaker()),
            Style::default().fg(colour).add_modifier(Modifier::BOLD),
        )));
        for chunk in wrap(turn.text(), width) {
            lines.push(Line::from(Span::styled(
                format!("  {chunk}"),
                Style::default().fg(palette.foreground),
            )));
        }
    }

    if app.asking {
        lines.push(Line::from(Span::styled(
            " waiting…",
            Style::default().fg(palette.line_number),
        )));
    }

    // Newest at the bottom: keep the tail that fits.
    let height = area.height as usize;
    if lines.len() > height {
        let drop = lines.len() - height;
        lines.drain(1..=drop);
    }
    frame.render_widget(Paragraph::new(lines), area);
}

/// Break text to a width without splitting words where it can be helped.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut out = Vec::new();
    for paragraph in text.lines() {
        let mut current = String::new();
        for word in paragraph.split_whitespace() {
            if !current.is_empty() && current.chars().count() + 1 + word.chars().count() > width {
                out.push(std::mem::take(&mut current));
            }
            if !current.is_empty() {
                current.push(' ');
            }
            // A word longer than the line is cut rather than pushing the
            // layout sideways.
            if word.chars().count() > width {
                out.push(word.chars().take(width).collect());
            } else {
                current.push_str(word);
            }
        }
        if !current.is_empty() {
            out.push(current);
        }
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

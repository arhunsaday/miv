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

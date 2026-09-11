//! The suggestion popup.

use super::Palette;
use crate::app::App;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

/// Rows the popup will use at most; a longer list is scrolled.
const MAX_ROWS: u16 = 8;

/// Draw the popup near `cursor`, which is a screen position.
pub fn draw(frame: &mut Frame, within: Rect, cursor: (u16, u16), app: &App, palette: &Palette) {
    let Some(completion) = &app.completion else {
        return;
    };
    if completion.items.is_empty() {
        return;
    }

    let longest = completion
        .items
        .iter()
        .map(|item| item.chars().count())
        .max()
        .unwrap_or(10);
    let width = (longest as u16 + 4).clamp(12, within.width.saturating_sub(2).max(12));
    let rows = (completion.items.len() as u16).min(MAX_ROWS);
    let height = rows + 2;

    // Below the cursor when there is room, above it when there is not.
    let (x, y) = cursor;
    let below = y + 1;
    let y = if below + height <= within.bottom() {
        below
    } else {
        y.saturating_sub(height)
    };
    let x = x.min(within.right().saturating_sub(width));
    let popup = Rect {
        x,
        y,
        width,
        height,
    };
    if popup.bottom() > within.bottom() || popup.right() > within.right() {
        return;
    }

    // Keep the highlighted entry visible.
    let visible = rows as usize;
    let start = completion
        .selected
        .saturating_sub(visible.saturating_sub(1))
        .min(completion.items.len().saturating_sub(visible));

    let lines: Vec<Line> = completion
        .items
        .iter()
        .enumerate()
        .skip(start)
        .take(visible)
        .map(|(index, item)| {
            let selected = index == completion.selected;
            let style = if selected {
                Style::default()
                    .fg(Color::Black)
                    .bg(palette.line_number_active)
            } else {
                Style::default().fg(palette.foreground)
            };
            // The typed part is shown in bold so you can see what matched.
            let split = completion.prefix.chars().count().min(item.chars().count());
            let typed: String = item.chars().take(split).collect();
            let rest: String = item.chars().skip(split).collect();
            Line::from(vec![
                Span::styled(" ", style),
                Span::styled(typed, style.add_modifier(Modifier::BOLD)),
                Span::styled(rest, style),
            ])
        })
        .collect();

    let title = if completion.items.len() > visible {
        format!(" {}/{} ", completion.selected + 1, completion.items.len())
    } else {
        String::new()
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(Style::default().fg(palette.line_number))
        .style(Style::default().bg(Color::Indexed(236)));

    frame.render_widget(Clear, popup);
    frame.render_widget(Paragraph::new(lines).block(block), popup);
}

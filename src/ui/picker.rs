//! Drawing the picker.

use super::Palette;
use crate::app::App;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

/// Rows of chrome: the border and the input line.
const CHROME: u16 = 3;

pub fn area_for(area: Rect) -> Rect {
    let width = area.width.saturating_sub(8).clamp(30, 100);
    let height = area.height.saturating_sub(4).clamp(6, 20);
    Rect {
        x: area.x + (area.width.saturating_sub(width)) / 2,
        y: area.y + (area.height.saturating_sub(height)) / 4,
        width,
        height,
    }
}

pub fn draw(frame: &mut Frame, area: Rect, app: &App, palette: &Palette) {
    let Some(picker) = &app.picker else {
        return;
    };
    let popup = area_for(area);
    // Two for the border, one so the detail never touches it.
    let inner_width = popup.width.saturating_sub(3) as usize;
    let rows = popup.height.saturating_sub(CHROME) as usize;

    let mut lines: Vec<Line> = Vec::with_capacity(rows + 1);
    lines.push(Line::from(vec![
        Span::styled(" ", Style::default().fg(palette.line_number)),
        Span::styled(
            picker.input.clone(),
            Style::default()
                .fg(palette.foreground)
                .add_modifier(Modifier::BOLD),
        ),
    ]));

    if picker.loading && picker.is_empty() {
        lines.push(Line::from(Span::styled(
            "  searching…",
            Style::default().fg(palette.line_number),
        )));
    } else if picker.matches().is_empty() {
        lines.push(Line::from(Span::styled(
            "  no matches",
            Style::default().fg(palette.line_number),
        )));
    }

    let (visible, highlighted) = picker.visible(rows);
    for (offset, index) in visible.iter().enumerate() {
        let Some(item) = picker.item(*index) else {
            continue;
        };
        let selected = offset == highlighted;
        let (marker, label_style) = if selected {
            (
                "▌ ",
                Style::default()
                    .fg(Color::Black)
                    .bg(palette.line_number_active),
            )
        } else {
            ("  ", Style::default().fg(palette.foreground))
        };

        // Keep the detail flush right, dropping it before the label when the
        // popup is narrow.
        let label_room = inner_width.saturating_sub(marker.len());
        let detail_room = label_room.saturating_sub(item.label.chars().count() + 2);
        let detail: String = if detail_room >= 4 {
            item.detail.chars().take(detail_room).collect()
        } else {
            String::new()
        };
        let label: String = item
            .label
            .chars()
            .take(label_room.saturating_sub(detail.chars().count() + 1))
            .collect();
        let gap = label_room.saturating_sub(label.chars().count() + detail.chars().count());

        lines.push(Line::from(vec![
            Span::styled(marker, label_style),
            Span::styled(label, label_style),
            Span::styled(" ".repeat(gap), label_style),
            Span::styled(
                detail,
                if selected {
                    label_style
                } else {
                    Style::default()
                        .fg(palette.line_number)
                        .add_modifier(Modifier::DIM)
                },
            ),
        ]));
    }

    let title = if picker.total() > 0 && !picker.input.is_empty() {
        format!(
            " {} — {}/{} ",
            picker.title,
            picker.matches().len(),
            picker.total()
        )
    } else {
        format!(" {} ", picker.title)
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(Style::default().fg(palette.line_number_active))
        .style(Style::default().bg(Color::Indexed(235)));

    frame.render_widget(Clear, popup);
    frame.render_widget(Paragraph::new(lines).block(block), popup);
}

/// Put the terminal cursor in the picker's input.
pub fn place_cursor(frame: &mut Frame, area: Rect, app: &App) {
    let Some(picker) = &app.picker else {
        return;
    };
    let popup = area_for(area);
    let x = popup.x + 2 + picker.caret as u16;
    if x < popup.right().saturating_sub(1) {
        frame.set_cursor_position((x, popup.y + 1));
    }
}

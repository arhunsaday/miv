use crate::Position;

use crossterm::{
    cursor::{self, Hide, MoveTo, SetCursorStyle},
    event::{read, Event::Key, KeyEvent},
    execute,
    style::{Color, SetBackgroundColor, SetForegroundColor},
    terminal::{self, Clear, ClearType, SetTitle},
};

use std::io::{self, stdout, Write};

pub struct Size {
    pub width: u16,
    pub height: u16,
}

pub struct Terminal {
    size: Size,
}

impl Terminal {
    pub fn default() -> Result<Self, std::io::Error> {
        let size = terminal::size()?;

        Ok(Self {
            size: Size {
                width: size.0,
                height: size.1.saturating_sub(2),
            },
        })
    }

    pub fn read_key() -> Result<KeyEvent, std::io::Error> {
        loop {
            match read()? {
                Key(event) => return Ok(event),
                _ => (),
                // TODO: Handle all the other events like mouse, resize, etc
            }
        }
    }

    pub fn size(&self) -> &Size {
        &self.size
    }

    pub fn set_title(title: &str) {
        execute!(stdout(), SetTitle(title)).unwrap();
    }

    pub fn clear_screen() {
        execute!(stdout(), Clear(ClearType::All)).unwrap();
    }
    pub fn clear_current_line() {
        execute!(stdout(), Clear(ClearType::CurrentLine)).unwrap();
    }

    pub fn set_cursor_position(position: &Position) {
        let Position { mut x, mut y } = position;

        // Make sure we don't go out of bounds
        if x > u16::MAX as usize {
            x = 0;
        }
        if y > u16::MAX as usize {
            y = 0;
        }

        let x = x as u16;
        let y = y as u16;

        execute!(stdout(), MoveTo(x, y)).unwrap();
    }
    pub fn set_block_cursor() {
        execute!(stdout(), SetCursorStyle::BlinkingBlock).unwrap();
    }
    pub fn set_bar_cursor() {
        execute!(stdout(), SetCursorStyle::BlinkingBar).unwrap();
    }
    pub fn hide_cursor() {
        execute!(stdout(), Hide).unwrap();
    }
    pub fn show_cursor() {
        execute!(stdout(), cursor::Show).unwrap();
    }

    pub fn set_bg_color(color: Color) {
        execute!(stdout(), SetBackgroundColor(color));
    }
    pub fn set_fg_color(color: Color) {
        execute!(stdout(), SetForegroundColor(color));
    }
    pub fn reset_bg_color() {
        execute!(stdout(), SetBackgroundColor(Color::Reset));
    }
    pub fn reset_fg_color() {
        execute!(stdout(), SetForegroundColor(Color::Reset));
    }

    pub fn flush() -> Result<(), std::io::Error> {
        io::stdout().flush()
    }
}

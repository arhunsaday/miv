use crate::Position;

use crossterm::{
    cursor::{self, Hide, MoveTo, SetCursorStyle},
    event::{read, Event::Key, KeyEvent},
    execute,
    style::{Color, SetBackgroundColor, SetForegroundColor},
    terminal::{self, Clear, ClearType, SetTitle},
    Command,
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
    pub fn new() -> Result<Self, std::io::Error> {
        Terminal::set_blinking_block_cursor();
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
                _ => continue,
            }
        }
    }

    pub fn size(&self) -> &Size {
        &self.size
    }

    pub fn set_title(title: &str) {
        execute(SetTitle(title));
    }

    pub fn clear_screen() {
        execute(Clear(ClearType::All));
    }
    pub fn clear_current_line() {
        execute(Clear(ClearType::CurrentLine));
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

        execute(MoveTo(x, y));
    }
    pub fn set_blinking_block_cursor() {
        execute(SetCursorStyle::BlinkingBlock);
    }
    pub fn set_block_cursor() {
        execute(SetCursorStyle::BlinkingBlock);
    }
    pub fn set_bar_cursor() {
        execute(SetCursorStyle::BlinkingBar);
    }
    pub fn hide_cursor() {
        execute(Hide);
    }
    pub fn show_cursor() {
        execute(cursor::Show);
    }

    pub fn set_bg_color(color: Color) {
        execute(SetBackgroundColor(color));
    }
    pub fn set_fg_color(color: Color) {
        execute(SetForegroundColor(color));
    }
    pub fn reset_bg_color() {
        execute(SetBackgroundColor(Color::Reset));
    }
    pub fn reset_fg_color() {
        execute(SetForegroundColor(Color::Reset));
    }

    pub fn flush() -> Result<(), std::io::Error> {
        io::stdout().flush()
    }
}

pub fn execute<C: Command>(command: C) {
    execute!(stdout(), command).unwrap();
}

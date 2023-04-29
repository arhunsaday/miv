use crate::Position;

use crossterm::{
    cursor::{self, Hide, MoveTo},
    event::{read, Event::Key, KeyCode, KeyEvent, KeyModifiers},
    execute,
    terminal::{self, Clear, ClearType, ScrollUp, SetTitle},
};

use std::io::{self, stdout, Read, Write};

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
                height: size.1,
            },
        })
    }

    pub fn size(&self) -> &Size {
        &self.size
    }

    pub fn clear_screen() {
        execute!(stdout(), Clear(ClearType::All)).unwrap();
    }

    pub fn clear_current_line() {
        execute!(stdout(), Clear(ClearType::CurrentLine)).unwrap();
    }

    pub fn set_cursor_position(position: &Position) {
        let Position { mut x, mut y } = position;

        x = x.saturating_add(1);
        y = y.saturating_add(1);

        let x = x.saturating_add(1) as u16;
        let y = y.saturating_add(1) as u16;

        execute!(stdout(), MoveTo(x, y)).unwrap();
    }

    pub fn flush() -> Result<(), std::io::Error> {
        io::stdout().flush()
    }

    pub fn set_title(title: &str) {
        execute!(stdout(), SetTitle(title)).unwrap();
    }

    pub fn hide_cursor() {
        execute!(stdout(), Hide).unwrap();
    }

    pub fn show_cursor() {
        execute!(stdout(), cursor::Show).unwrap();
    }

    pub fn read_key() -> Result<KeyEvent, std::io::Error> {
        loop {
            match read()? {
                Key(event) => return Ok(event),
                _ => (),
            }
        }
    }
}

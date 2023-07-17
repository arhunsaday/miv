use crate::Position;
use crossterm::{
    cursor::{self, MoveTo, SetCursorStyle},
    event::{
        read, DisableMouseCapture, EnableMouseCapture,
        Event::{Key, Mouse},
        KeyEvent, MouseEventKind,
    },
    execute,
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
        Terminal::change_defaults();
        let size = terminal::size()?;

        Ok(Self {
            size: Size {
                width: size.0,
                height: size.1.saturating_sub(2),
            },
        })
    }

    pub fn change_defaults() {
        Terminal::set_cursor(SetCursorStyle::BlinkingBlock);
        terminal::enable_raw_mode().unwrap();
        execute(EnableMouseCapture);
    }

    pub fn restore_defaults() {
        Terminal::show_cursor();
        terminal::disable_raw_mode().unwrap();
        execute(DisableMouseCapture);
    }

    pub fn read_key() -> Result<KeyEvent, std::io::Error> {
        loop {
            match read()? {
                Key(event) => return Ok(event),
                Mouse(event) => match event.kind {
                    MouseEventKind::ScrollDown => {}
                    MouseEventKind::ScrollUp => {}
                    MouseEventKind::Down(key) => match key {
                        crossterm::event::MouseButton::Left => {
                            Terminal::set_cursor_position(&Position {
                                x: event.column as usize,
                                y: event.row as usize,
                            });
                        }
                        crossterm::event::MouseButton::Right => {}
                        crossterm::event::MouseButton::Middle => {}
                    },
                    _ => {}
                },
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

    pub fn set_cursor(cursor: SetCursorStyle) {
        execute(cursor);
    }
    pub fn show_cursor() {
        execute(cursor::Show);
    }
    pub fn hide_cursor() {
        execute(cursor::Hide);
    }

    pub fn flush() -> Result<(), std::io::Error> {
        io::stdout().flush()
    }
}

pub fn execute<C: Command>(command: C) {
    execute!(stdout(), command).unwrap();
}

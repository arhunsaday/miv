use crate::Terminal;

use crossterm::{
    cursor::{self, Hide, MoveTo},
    event::{read, Event::Key, KeyCode, KeyEvent, KeyModifiers},
    execute,
    terminal::{self, Clear, ClearType, ScrollUp, SetTitle},
};

use std::{
    fmt::format,
    io::{self, stdout, Read, Write},
};

const EDITOR_VERSION: &str = env!("CARGO_PKG_VERSION");

pub struct Position {
    pub x: usize,
    pub y: usize,
}

pub struct Editor {
    should_quit: bool,
    terminal: Terminal,
    cursor_position: Position,
}

impl Editor {
    pub fn default() -> Self {
        Editor {
            should_quit: false,
            terminal: Terminal::default().expect("Failed to initialize terminal"),
            cursor_position: Position { x: 0, y: 0 },
        }
    }

    pub fn run(&mut self) {
        terminal::enable_raw_mode().unwrap();

        loop {
            if let Err(e) = self.refresh_screen() {
                die(e);
            }

            if self.should_quit {
                break;
            }

            if let Err(e) = self.process_keypress() {
                die(e);
            }
        }
    }

    fn refresh_screen(&self) -> Result<(), std::io::Error> {
        Terminal::hide_cursor();
        Terminal::clear_screen();
        Terminal::set_cursor_position(&Position { x: 0, y: 0 });
        Terminal::set_title("Miv");

        if self.should_quit {
            println!("Goodbye, world.\r")
        } else {
            self.draw_rows();
            Terminal::set_cursor_position(&self.cursor_position);
        }

        Terminal::show_cursor();
        Terminal::flush()
    }

    fn process_keypress(&mut self) -> Result<(), std::io::Error> {
        let event = Terminal::read_key()?;

        // TODO: write this in termion style
        match (event.code, event.modifiers) {
            (KeyCode::Char('q'), KeyModifiers::CONTROL) => {
                terminal::disable_raw_mode().unwrap();
                self.should_quit = true;
            }
            (KeyCode::Char(c), KeyModifiers::CONTROL) => {
                println!("Ctrl + {}", c);
            }
            (KeyCode::Char(c), KeyModifiers::NONE) => {
                println!("{}", c);
            }
            (KeyCode::Up, _)
            | (KeyCode::Down, _)
            | (KeyCode::Left, _)
            | (KeyCode::Right, _)
            | (KeyCode::PageUp, _)
            | (KeyCode::PageDown, _)
            | (KeyCode::Home, _)
            | (KeyCode::End, _) => self.move_cursor(event.code),
            _ => (),
        }

        Ok(())
    }

    fn move_cursor(&mut self, key: KeyCode) {
        let Position { mut x, mut y } = self.cursor_position;
        let size = self.terminal.size();
        let height = size.height.saturating_sub(1) as usize;
        let width = size.width.saturating_sub(1) as usize;

        match key {
            KeyCode::PageUp => y = 0,
            KeyCode::PageDown => y = height,
            KeyCode::Home => x = 0,
            KeyCode::End => x = width,

            KeyCode::Up => y = y.saturating_sub(1),
            KeyCode::Left => x = x.saturating_sub(1),

            KeyCode::Down => {
                if y < height {
                    y = y.saturating_add(1);
                }
            }
            KeyCode::Right => {
                if x < width {
                    x = x.saturating_add(1);
                }
            }
            _ => (),
        }

        self.cursor_position = Position { x, y };
    }

    fn draw_welcome_message(&self) {
        let mut welcome_message = format!("Miv editor -- version {}", EDITOR_VERSION);
        let width = self.terminal.size().width as usize;
        let message_len = welcome_message.len();
        let padding = width.saturating_sub(message_len) / 2;
        let spaces = " ".repeat(padding.saturating_sub(1));
        welcome_message = format!("~{}{}", spaces, welcome_message);
        welcome_message.truncate(width);
        println!("{}\r", welcome_message);
    }

    fn draw_rows(&self) {
        let height = self.terminal.size().height;
        for row in 0..height {
            Terminal::clear_current_line();
            if row == height / 3 {
                self.draw_welcome_message();
            } else {
                println!("~\r");
            }
        }
    }
}

fn die(e: std::io::Error) {
    println!("{}", e);
    execute!(stdout(), Clear(ClearType::All), cursor::MoveTo(0, 0),).unwrap();

    panic!("Panic");
}

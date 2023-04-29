use crate::Document;
use crate::Row;
use crate::Terminal;
use std::cmp;
use std::env;
use std::time::{Duration, Instant};

use crossterm::{
    cursor,
    event::{KeyCode, KeyModifiers},
    execute,
    style::Color,
    terminal::{self, Clear, ClearType},
};

use std::io::stdout;

const EDITOR_VERSION: &str = env!("CARGO_PKG_VERSION");
const STATUS_FG_COLOR: Color = Color::Black;
const STATUS_BG_COLOR: Color = Color::White;
const QUIT_TIMES: u8 = 2;

#[derive(Default)]
pub struct Position {
    pub x: usize,
    pub y: usize,
}

struct StatusMessage {
    text: String,
    timestamp: Instant,
}

impl StatusMessage {
    fn from(message: String) -> Self {
        Self {
            timestamp: Instant::now(),
            text: message,
        }
    }
}

pub struct Editor {
    should_quit: bool,
    terminal: Terminal,
    document: Document,
    cursor_position: Position,
    offset: Position,
    status_message: StatusMessage,
    quit_times: u8,
}

impl Editor {
    pub fn default() -> Self {
        let args: Vec<String> = env::args().collect();
        let mut initial_status = String::from("HELP: Ctrl-Q to quit | Ctrl-S to save.");

        let document = if args.len() > 1 {
            match Document::open(&args[1]) {
                Ok(doc) => doc,
                Err(e) => {
                    initial_status = format!("ERROR: Could not open file: {e}");
                    Document::open_non_existent(&args[1])
                }
            }
        } else {
            Document::default()
        };

        Self {
            should_quit: false,
            terminal: Terminal::default().expect("Failed to initialize terminal"),
            document,
            cursor_position: Position::default(),
            offset: Position::default(),
            status_message: StatusMessage::from(initial_status),
            quit_times: QUIT_TIMES,
        }
    }

    pub fn run(&mut self) {
        terminal::enable_raw_mode().unwrap();

        loop {
            if let Err(e) = self.refresh_screen() {
                die(e);
            }

            if self.should_quit {
                terminal::disable_raw_mode().unwrap();
                break;
            }

            if let Err(e) = self.process_keypress() {
                die(e);
            }
        }
    }

    fn refresh_screen(&self) -> Result<(), std::io::Error> {
        Terminal::hide_cursor();
        Terminal::set_cursor_position(&Position::default());
        Terminal::set_title("Miv");

        if self.should_quit {
            println!("Goodbye, world.\r")
        } else {
            self.draw_rows();
            self.draw_status_bar();
            self.draw_message_bar();
            Terminal::set_cursor_position(&Position {
                x: self.cursor_position.x.saturating_sub(self.offset.x),
                y: self.cursor_position.y.saturating_sub(self.offset.y),
            })
        }

        Terminal::show_cursor();
        Terminal::flush()
    }

    fn save_file(&mut self) {
        if self.document.file_name.is_none() {
            let new_name = self.prompt("Save as: ").unwrap_or(None);

            if new_name.is_none() {
                self.status_message = StatusMessage::from("INFO: File save aborted.".to_string());
                return;
            }
            self.document.file_name = new_name;
        }

        if self.document.save().is_ok() {
            self.status_message = StatusMessage::from("INFO: File saved successfully.".to_string());
        } else {
            self.status_message = StatusMessage::from("ERROR: Could not save file!".to_string());
        }
    }

    fn process_keypress(&mut self) -> Result<(), std::io::Error> {
        let event = Terminal::read_key()?;

        // TODO: Write this in termion style
        match (event.code, event.modifiers) {
            // Ctrl keys
            (KeyCode::Char(c), KeyModifiers::CONTROL) => {
                if c == 'q' {
                    if self.quit_times > 0 && self.document.is_dirty() {
                        self.status_message = StatusMessage::from(format!(
                            "WARNING: File has unsaved changes. Press Ctrl-Q {} more times to quit.",
                            self.quit_times
                        ));
                        self.quit_times -= 1;
                        return Ok(());
                    } else {
                        self.should_quit = true;
                    }
                } else if c == 's' {
                    self.save_file();
                }
            }
            // Normal keys without ctrl
            (KeyCode::Char(c), KeyModifiers::NONE) => {
                self.document.insert(&self.cursor_position, c);
                self.move_cursor(KeyCode::Right);
            }
            (KeyCode::Enter, _) => {
                self.document.insert_newline(&self.cursor_position);
                self.move_cursor(KeyCode::Right);
            }
            (KeyCode::Delete, _) => {
                self.document.delete(&self.cursor_position);
            }
            (KeyCode::Backspace, _) => {
                if self.cursor_position.x > 0 || self.cursor_position.y > 0 {
                    self.move_cursor(KeyCode::Left);
                    self.document.delete(&self.cursor_position);
                }
            }
            // Handle cursor movement (arrow keys, scroll etc)
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

        self.scroll();

        if self.quit_times <= QUIT_TIMES {
            self.quit_times = QUIT_TIMES;
            self.status_message = StatusMessage::from("".to_string());
        }

        Ok(())
    }

    fn scroll(&mut self) {
        let Position { x, y } = self.cursor_position;
        let width = self.terminal.size().width as usize;
        let height = self.terminal.size().height as usize;
        let mut offset = &mut self.offset;

        if y < offset.y {
            offset.y = y;
        } else if y >= offset.y.saturating_add(height) {
            offset.y = y.saturating_sub(height).saturating_add(1);
        }

        if x < offset.x {
            offset.x = x;
        } else if x >= offset.x.saturating_add(width) {
            offset.x = x.saturating_sub(width).saturating_add(1);
        }
    }

    fn move_cursor(&mut self, key: KeyCode) {
        let terminal_window_height = self.terminal.size().height as usize;
        let Position { mut x, mut y } = self.cursor_position;
        let document_height = self.document.len();
        let width = if let Some(row) = self.document.row(y) {
            row.len()
        } else {
            0
        };

        // Don't allow cursor to go past the end of the line
        if x > width {
            x = width;
        }

        match key {
            KeyCode::Home => x = 0,
            KeyCode::End => x = width,

            // Scrolling
            KeyCode::PageUp => {
                y = if y > terminal_window_height {
                    y - terminal_window_height
                } else {
                    0
                };
            }
            KeyCode::PageDown => {
                y = if y.saturating_add(terminal_window_height) < document_height {
                    y + terminal_window_height
                } else {
                    document_height
                };
            }

            // Arrow key movements
            KeyCode::Up => y = y.saturating_sub(1),
            KeyCode::Left => {
                // Move one to the left
                if x > 0 {
                    x -= 1;
                // Move to the end of the previous line if cursor is at the start of the line
                } else {
                    y -= 1;
                    if let Some(row) = self.document.row(y) {
                        x = row.len();
                    } else {
                        x = 0;
                    }
                }
            }
            KeyCode::Down => {
                if y < document_height {
                    y = y.saturating_add(1);
                }
            }

            KeyCode::Right => match x.cmp(&width) {
                cmp::Ordering::Less => x += 1, // Move one to the right
                cmp::Ordering::Equal => {
                    // Move to the start of the next line if cursor is at the end of the line
                    y += 1;
                    x = 0;
                }
                _ => (),
            },
            _ => (),
        }

        self.cursor_position = Position { x, y };
    }

    fn draw_welcome_message(&self) {
        let mut welcome_message = format!("Miv editor -- version {}", EDITOR_VERSION);
        let message_len = welcome_message.len();

        let width = self.terminal.size().width as usize;
        let padding = width.saturating_sub(message_len) / 2;
        let spaces = " ".repeat(padding.saturating_sub(1));

        welcome_message = format!("~{}{}", spaces, welcome_message);
        welcome_message.truncate(width);

        println!("{}\r", welcome_message);
    }

    fn draw_row(&self, row: &Row) {
        let width = self.terminal.size().width as usize;
        let start = self.offset.x;
        let end = self.offset.x + width;

        let row = row.render(start, end);
        println!("{}\r", row)
    }

    fn draw_rows(&self) {
        let height = self.terminal.size().height;

        for terminal_row in 0..height {
            Terminal::clear_current_line();

            if let Some(row) = self.document.row(terminal_row as usize + self.offset.y) {
                self.draw_row(row);
            } else if self.document.is_empty() && terminal_row == height / 3 {
                self.draw_welcome_message();
            } else {
                println!("~\r");
            }
        }
    }

    fn draw_status_bar(&self) {
        let mut status;

        let width = self.terminal.size().width as usize;
        let modified_indicator = if self.document.is_dirty() {
            "(modified)"
        } else {
            ""
        };

        let mut file_name = "[No Name]".to_string();
        if let Some(name) = &self.document.file_name {
            file_name = name.clone();
            file_name.truncate(width);
        }

        // File name on the left
        status = format!("{} {}", file_name, modified_indicator);

        // Current line indicator on the right
        let line_indicator = format!(
            "Line {}/{}",
            self.cursor_position.y.saturating_add(1),
            self.document.len()
        );

        let len = status.len() + line_indicator.len();

        if width > len {
            status.push_str(&" ".repeat(width - len));
        }

        status = format!("{}{}", status, line_indicator);
        status.truncate(width);

        Terminal::set_bg_color(STATUS_BG_COLOR);
        Terminal::set_fg_color(STATUS_FG_COLOR);
        println!("{status}\r");
        Terminal::reset_bg_color();
        Terminal::reset_fg_color();
    }

    fn draw_message_bar(&self) {
        Terminal::clear_current_line();

        let message = &self.status_message;

        if Instant::now() - message.timestamp < Duration::new(5, 0) {
            let mut text = message.text.clone();
            text.truncate(self.terminal.size().width as usize);
            print!("{text}");
        }
    }

    fn prompt(&mut self, prompt: &str) -> Result<Option<String>, std::io::Error> {
        let mut result = String::new();

        loop {
            self.status_message = StatusMessage::from(format!("{}{}", prompt, result));
            self.refresh_screen()?;

            let event = Terminal::read_key()?;

            match (event.code, event.modifiers) {
                (KeyCode::Backspace, _) => {
                    if !result.is_empty() {
                        result.pop();
                    }
                }
                (KeyCode::Char(c), KeyModifiers::NONE) => {
                    result.push(c);
                }
                (KeyCode::Enter, _) => break,
                (KeyCode::Esc, _) => break,
                _ => (),
            }
        }

        self.status_message = StatusMessage::from(String::new());

        if result.is_empty() {
            Ok(None)
        } else {
            Ok(Some(result))
        }
    }
}

fn die(e: std::io::Error) {
    println!("{}", e);
    execute!(stdout(), Clear(ClearType::All), cursor::MoveTo(0, 0),).unwrap();

    panic!("Panic");
}

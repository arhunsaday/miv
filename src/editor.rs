use crate::{Document, Highlighting, Mode, Position, PossibleModes, Row, Settings, Terminal};
use crossterm::{
    cursor,
    event::{KeyCode, KeyModifiers},
    execute,
    style::Stylize,
    terminal::{Clear, ClearType},
};
use std::cmp;
use std::env;
use std::io::stdout;
use std::time::{Duration, Instant};
use syntect::{easy::HighlightLines, highlighting::Style, util::as_24_bit_terminal_escaped};

const EDITOR_VERSION: &str = env!("CARGO_PKG_VERSION");
const QUIT_TIMES: u8 = 2;

#[derive(PartialEq, Copy, Clone)]
pub enum SearchDirection {
    Forward,
    Backward,
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
    mode: Mode,
    highlighting: Highlighting,
    config: Settings,
}

impl Editor {
    pub fn new(config: Settings) -> Self {
        let args: Vec<String> = env::args().collect();
        let mut initial_status =
            String::from("HELP: Ctrl-Q = quit | Ctrl-S = save | Ctrl+f = search");

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
            terminal: Terminal::new().expect("Failed to initialize terminal"),
            document,
            cursor_position: Position::default(),
            offset: Position::default(),
            status_message: StatusMessage::from(initial_status),
            quit_times: QUIT_TIMES,
            mode: Mode::default(),
            highlighting: Highlighting::default(),
            config,
        }
    }

    pub fn run(&mut self) {
        Terminal::set_title(&format!(
            "{} — Miv {}",
            self.document
                .file_name
                .clone()
                .unwrap_or(String::from("[No Name]")),
            EDITOR_VERSION,
        ));

        // Main loop of the editor
        loop {
            if let Err(e) = self.refresh_screen() {
                die(e);
            }

            if self.should_quit {
                Terminal::restore_defaults();
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

        if !self.should_quit {
            // TODO: Don't rerender rows if they haven't changed
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
            let new_name = self.prompt("Save as: ", |_, _, _| {}).unwrap_or(None);

            if new_name.is_none() {
                self.status_message = StatusMessage::from("File save aborted.".to_string());
                return;
            }
            self.document.file_name = new_name;
        }

        if self.document.save().is_ok() {
            self.status_message = StatusMessage::from("File saved successfully.".to_string());
        } else {
            self.status_message = StatusMessage::from("ERROR: Could not save file!".to_string());
        }
    }

    // TODO: Refactor this
    fn process_keypress(&mut self) -> Result<(), std::io::Error> {
        let event = Terminal::read_key()?;
        match self.mode.current_mode {
            // Normal mode keybindings
            PossibleModes::Normal => match (event.code, event.modifiers) {
                (KeyCode::Char(c), KeyModifiers::NONE) => match c {
                    'h' => {
                        self.move_cursor(KeyCode::Left);
                    }
                    'j' => {
                        self.move_cursor(KeyCode::Down);
                    }
                    'k' => {
                        self.move_cursor(KeyCode::Up);
                    }
                    'l' => {
                        self.move_cursor(KeyCode::Right);
                    }
                    '0' => {
                        self.move_cursor(KeyCode::Home);
                    }
                    '$' => {
                        self.move_cursor(KeyCode::End);
                    }
                    'w' => {
                        self.move_cursor_word();
                    }
                    'b' => {
                        // self.move_cursor_word_back();
                    }
                    'a' => {
                        self.move_cursor(KeyCode::Right);
                        self.mode.switch(PossibleModes::Insert);
                    }
                    'i' => {
                        self.mode.switch(PossibleModes::Insert);
                    }
                    'v' => {
                        self.mode.switch(PossibleModes::Visual);
                    }
                    'o' => {
                        self.move_cursor(KeyCode::Down);
                        self.mode.switch(PossibleModes::Insert);
                        self.document.insert_newline(&self.cursor_position);
                    }
                    ':' => {
                        self.command_mode();
                    }
                    _ => {
                        self.mode.switch(PossibleModes::OperatorPending);
                    }
                },
                (KeyCode::Char(c), KeyModifiers::CONTROL) => match c {
                    'd' => {
                        self.move_cursor(KeyCode::PageDown);
                    }
                    'u' => {
                        self.move_cursor(KeyCode::PageUp);
                    }
                    _ => {}
                },
                _ => {}
            },
            // Insert mode keybindings
            PossibleModes::Insert => match (event.code, event.modifiers) {
                (KeyCode::Esc, _) => {
                    self.mode.switch(PossibleModes::Normal);
                    self.move_cursor(KeyCode::Left);
                }
                (KeyCode::Char(c), KeyModifiers::NONE) => {
                    self.document.insert(&self.cursor_position, c);
                    self.move_cursor(KeyCode::Right);
                }
                (KeyCode::Char(c), KeyModifiers::SHIFT) => {
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
                (KeyCode::Tab, _) => {
                    for _ in 0..self.config.editor.tab_size {
                        self.document.insert(&self.cursor_position, ' ');
                        self.move_cursor(KeyCode::Right);
                    }
                }
                _ => {}
            },
            // Visual mode keybindings
            PossibleModes::Visual => match (event.code, event.modifiers) {
                (KeyCode::Esc, _) => {
                    self.mode.switch(PossibleModes::Normal);
                }
                _ => {}
            },
            // Operator pending mode keybindings
            PossibleModes::OperatorPending => match (event.code, event.modifiers) {
                (KeyCode::Esc, _) => {
                    self.mode.switch(PossibleModes::Normal);
                }
                _ => {}
            },
            _ => {}
        }

        // Keys that work in both modes
        match (event.code, event.modifiers) {
            (KeyCode::Char(c), KeyModifiers::CONTROL) => match c {
                'q' => {
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
                }
                's' => {
                    self.save_file();
                }
                'f' => {
                    self.search_mode();
                }
                _ => {}
            },
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
                    y.saturating_sub(terminal_window_height)
                } else {
                    0
                };
            }
            KeyCode::PageDown => {
                y = if y.saturating_add(terminal_window_height) < document_height {
                    y.saturating_add(terminal_window_height)
                } else {
                    document_height
                };
            }

            // Arrow key movements
            KeyCode::Up => y = y.saturating_sub(1),
            KeyCode::Down => {
                if y < document_height {
                    y = y.saturating_add(1);
                }
            }
            KeyCode::Left => match x.cmp(&0) {
                cmp::Ordering::Greater => x -= 1, // Move one to the left
                cmp::Ordering::Equal => {
                    // Move to the end of the previous line if cursor is at the start of the line
                    if y > 0 {
                        y -= 1;
                        if let Some(row) = self.document.row(y) {
                            x = row.len();
                        } else {
                            x = 0;
                        }
                    }
                }
                _ => (),
            },
            KeyCode::Right => match x.cmp(&width) {
                cmp::Ordering::Less => x += 1, // Move one to the right
                cmp::Ordering::Equal => {
                    // Move to the start of the next line if cursor is at the end of the line
                    if y < document_height {
                        y += 1;
                        x = 0;
                    }
                }
                _ => (),
            },
            _ => (),
        }

        self.cursor_position = Position { x, y };
    }

    fn move_cursor_word(&mut self) {}

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

    fn draw_row(&self, row: &Row, line_number: u16) {
        // TODO: Add line numbers
        // TODO: Cache the syntax highlighting
        let width = self.terminal.size().width as usize;
        let start = self.offset.x;
        let end = self.offset.x.saturating_add(width);
        let row = row.get_display_graphemes(start, end);

        // TODO: cache the syntax highlighting
        let syntax = self
            .highlighting
            .syntax_set
            .find_syntax_by_extension("rs")
            .unwrap();

        let mut h = HighlightLines::new(
            syntax,
            &self.highlighting.theme_set.themes[&self.config.appearance.theme],
        );

        let ranges: Vec<(Style, &str)> = h.highlight(&row, &self.highlighting.syntax_set);
        let escaped = as_24_bit_terminal_escaped(&ranges[..], false);
        println!("{escaped}\r");
    }

    fn draw_rows(&self) {
        let height = self.terminal.size().height;

        for terminal_row in 0..height {
            Terminal::clear_current_line();

            if let Some(row) = self.document.row(terminal_row as usize + self.offset.y) {
                self.draw_row(row, terminal_row);
            } else if self.document.is_empty() && terminal_row == height / 3 {
                self.draw_welcome_message();
            } else {
                println!("~\r");
            }
        }
    }

    fn draw_status_bar(&self) {
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

        let app_name = " Miv ";
        let left_info = format!(
            " {} {} [{}]",
            file_name, modified_indicator, self.mode.current_mode
        );

        let left_content = format!("{}{}", app_name, left_info);
        let right_content = format!(
            "Filetype: {} | Line {}/{}",
            self.document.file_type(),
            self.cursor_position.y.saturating_add(1),
            self.document.len()
        );

        let len = left_content.len() + right_content.len();
        let empty_space: String = " ".repeat(width.saturating_sub(len));
        let status_bar_content = format!("{}{}{}", left_info, empty_space, right_content);

        println!(
            "{}{}\r",
            app_name
                .bold()
                .white()
                .on(self.mode.current_mode.to_color()),
            status_bar_content.black().on_grey()
        );
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

    fn prompt<C>(&mut self, prompt: &str, mut callback: C) -> Result<Option<String>, std::io::Error>
    where
        C: FnMut(&mut Self, KeyCode, &String),
    {
        let mut result = String::new();

        loop {
            self.status_message = StatusMessage::from(format!("{}{}", prompt, result));
            self.refresh_screen()?;

            let event = Terminal::read_key()?;

            match (event.code, event.modifiers) {
                (KeyCode::Backspace, _) => {
                    result.truncate(result.len().saturating_sub(1));
                    if !result.is_empty() {
                        result.pop();
                    }
                }
                (KeyCode::Char(c), KeyModifiers::NONE) => {
                    result.push(c);
                }
                (KeyCode::Enter, _) => break,
                (KeyCode::Esc, _) => {
                    result.truncate(0);
                    break;
                }
                _ => (),
            }
            // TODO: Make variant where callback is optional and not called on every char (for save file for example)
            callback(self, event.code, &result)
        }

        self.status_message = StatusMessage::from(String::new());

        if result.is_empty() {
            Ok(None)
        } else {
            Ok(Some(result))
        }
    }

    fn command_mode(&mut self) {
        let old_position = self.cursor_position.clone();
        let query = self.prompt(":", |editor, key, query| {}).unwrap_or(None);

        match query {
            Some(command) => match command.as_str() {
                "q" | "quit" => {
                    self.should_quit = true;
                }
                "w" | "save" => {
                    self.save_file();
                }
                _ => {
                    self.status_message =
                        StatusMessage::from("Not an editor command: ".to_string() + &command)
                }
            },
            None => {
                self.cursor_position = old_position;
                self.scroll();
            }
        }
    }

    fn search_mode(&mut self) {
        let old_position = self.cursor_position.clone();
        let mut direction = SearchDirection::Forward;

        let query = self
            .prompt(
                "Search (ESC to cancel, arrows to navigate): ",
                |editor, key, query| {
                    let mut moved = false;

                    match key {
                        KeyCode::Right | KeyCode::Down => {
                            direction = SearchDirection::Forward;
                            editor.move_cursor(KeyCode::Right);
                            moved = true;
                        }
                        KeyCode::Left | KeyCode::Up => direction = SearchDirection::Backward,
                        _ => (),
                    }

                    if let Some(position) =
                        editor
                            .document
                            .find(query, &editor.cursor_position, direction)
                    {
                        editor.cursor_position = position;
                        editor.scroll();
                    } else if moved {
                        editor.move_cursor(KeyCode::Left);
                    }

                    // editor.document.highlight(Some(query))
                },
            )
            .unwrap_or(None);

        if query.is_none() {
            self.cursor_position = old_position;
            self.scroll();
        }
        // self.document.highlight(None);
    }
}

fn die(e: std::io::Error) {
    println!("{}", e);
    execute!(stdout(), Clear(ClearType::All), cursor::MoveTo(0, 0),).unwrap();
    panic!("Panic");
}

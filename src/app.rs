//! Editor state and the editing actions the key layer drives.

use crate::buffer::Buffer;
use crate::config::{Config, LineNumbers};
use crate::keymap::Pending;
use crate::mode::{Mode, VisualKind};
use crate::motion::{EditRange, FindTarget};
use crate::operator::EditOptions;
use crate::register::{RegisterContent, RegisterKind, Registers};
use crate::search::Search;
use crate::syntax::SyntaxEngine;
use crate::text::{self, Position};
use anyhow::Result;
use crossterm::event::KeyEvent;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use syntect::highlighting::Theme;

/// Keys a single real keystroke may cause to be replayed. A recursive macro
/// keeps the queue short while never draining it, so a size cap alone would
/// not catch it; this budget bounds the total work instead.
const REPLAY_BUDGET: usize = 200_000;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum MessageKind {
    Info,
    Error,
}

pub struct Message {
    pub text: String,
    pub kind: MessageKind,
}

/// A dismissable list overlay, used by `:ls`, `:registers` and `:help`.
pub struct Overlay {
    pub title: String,
    pub lines: Vec<String>,
    pub scroll: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PromptKind {
    Command,
    SearchForward,
    SearchBackward,
}

pub struct Prompt {
    pub kind: PromptKind,
    pub input: String,
    /// Caret position in characters.
    pub cursor: usize,
    /// Cursor to restore if the prompt is cancelled.
    pub origin: Position,
    pub origin_view: usize,
    pub history_index: Option<usize>,
}

impl PromptKind {
    pub fn sigil(self) -> char {
        match self {
            PromptKind::Command => ':',
            PromptKind::SearchForward => '/',
            PromptKind::SearchBackward => '?',
        }
    }
}

/// Keystrokes of the last buffer-changing command, replayed by `.`.
#[derive(Default)]
pub struct Dot {
    pub last: Vec<KeyEvent>,
    pub recording: Vec<KeyEvent>,
    /// Set by any action that changes the buffer.
    pub changed: bool,
}

pub struct Recording {
    pub register: char,
    pub keys: Vec<KeyEvent>,
}

#[derive(Default, Clone, Copy)]
pub struct Viewport {
    /// Text rows available, excluding status and command lines.
    pub height: usize,
    /// Columns available for text, excluding the gutter.
    pub text_width: usize,
}

pub struct App {
    pub buffers: Vec<Buffer>,
    pub current: usize,
    next_id: usize,
    pub mode: Mode,
    pub registers: Registers,
    pub search: Search,
    pub search_highlight: bool,
    pub config: Config,
    pub engine: SyntaxEngine,
    pub theme: Theme,
    pub message: Option<Message>,
    pub overlay: Option<Overlay>,
    pub pending: Pending,
    pub prompt: Option<Prompt>,
    pub should_quit: bool,
    pub viewport: Viewport,
    pub visual_anchor: Position,
    pub last_find: Option<FindTarget>,
    pub queue: VecDeque<KeyEvent>,
    pub dot: Dot,
    pub recording: Option<Recording>,
    pub command_history: Vec<String>,
    pub search_history: Vec<String>,
    replay_budget: usize,
}

impl App {
    pub fn new(config: Config, files: &[PathBuf], goto_line: Option<usize>) -> Result<Self> {
        let engine = SyntaxEngine::new();
        let theme = match engine.theme(&config.appearance.theme) {
            Some(theme) => theme.clone(),
            None => engine
                .theme("base16-ocean.dark")
                .cloned()
                .unwrap_or_else(|| {
                    engine
                        .theme_names()
                        .first()
                        .and_then(|n| engine.theme(n))
                        .cloned()
                        .expect("syntect ships themes")
                }),
        };
        let bad_theme = engine.theme(&config.appearance.theme).is_none();

        let mut app = Self {
            buffers: Vec::new(),
            current: 0,
            next_id: 1,
            mode: Mode::Normal,
            registers: Registers::default(),
            search: Search::default(),
            search_highlight: true,
            config,
            engine,
            theme,
            message: None,
            overlay: None,
            pending: Pending::default(),
            prompt: None,
            should_quit: false,
            viewport: Viewport::default(),
            visual_anchor: Position::default(),
            last_find: None,
            queue: VecDeque::new(),
            dot: Dot::default(),
            recording: None,
            command_history: Vec::new(),
            search_history: Vec::new(),
            replay_budget: REPLAY_BUDGET,
        };

        if files.is_empty() {
            let id = app.take_id();
            app.buffers.push(Buffer::scratch(id));
        } else {
            for path in files {
                let id = app.take_id();
                match Buffer::open(id, path) {
                    Ok(buffer) => app.buffers.push(buffer),
                    Err(e) => {
                        app.set_error(format!("{e}"));
                        let id = app.take_id();
                        app.buffers.push(Buffer::scratch(id));
                    }
                }
            }
        }
        for i in 0..app.buffers.len() {
            app.detect_syntax(i);
        }
        if bad_theme {
            let name = app.config.appearance.theme.clone();
            app.set_error(format!(
                "unknown theme {name:?}; using base16-ocean.dark (see :help)"
            ));
        }
        if let Some(line) = goto_line {
            let last = app.buffer().line_count().saturating_sub(1);
            let target = line.saturating_sub(1).min(last);
            let col = text::first_non_blank(&app.buffer().rope, target);
            app.buffer_mut().cursor = Position::new(target, col);
        }
        Ok(app)
    }

    fn take_id(&mut self) -> usize {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    pub fn buffer(&self) -> &Buffer {
        &self.buffers[self.current]
    }

    pub fn buffer_mut(&mut self) -> &mut Buffer {
        &mut self.buffers[self.current]
    }

    pub fn edit_options(&self) -> EditOptions {
        EditOptions {
            shift_width: self.config.editor.shift_width,
            expand_tab: self.config.editor.expand_tab,
            tab_width: self.config.editor.tab_width,
        }
    }

    pub fn set_message(&mut self, text: impl Into<String>) {
        self.message = Some(Message {
            text: text.into(),
            kind: MessageKind::Info,
        });
    }

    pub fn set_error(&mut self, text: impl Into<String>) {
        self.message = Some(Message {
            text: text.into(),
            kind: MessageKind::Error,
        });
    }

    pub fn detect_syntax(&mut self, index: usize) {
        let first_line: String = {
            let buffer = &self.buffers[index];
            text::line(&buffer.rope, 0).chars().collect()
        };
        let path = self.buffers[index].path.clone();
        let name = self
            .engine
            .detect(path.as_deref(), &first_line)
            .name
            .clone();
        let buffer = &mut self.buffers[index];
        buffer.syntax_name = name;
        buffer.syntax_cache.clear();
        if buffer.line_count() > self.config.appearance.max_highlight_lines
            || !self.config.appearance.syntax_highlighting
        {
            buffer.syntax_cache.disable();
        }
    }

    pub fn gutter_width(&self) -> usize {
        match self.config.editor.line_numbers {
            LineNumbers::None => 0,
            _ => {
                let digits = self.buffer().line_count().to_string().len().max(3);
                digits + 2
            }
        }
    }

    /// Keep the cursor inside the viewport, honouring `scrolloff`.
    pub fn scroll_to_cursor(&mut self) {
        let height = self.viewport.height.max(1);
        let width = self.viewport.text_width.max(1);
        let scrolloff = self
            .config
            .editor
            .scrolloff
            .min(height.saturating_sub(1) / 2);
        let tab_width = self.config.editor.tab_width;
        let buffer = self.buffer_mut();

        let last = buffer.line_count().saturating_sub(1);
        let line = buffer.cursor.line.min(last);
        if line < buffer.view_top.saturating_add(scrolloff) {
            buffer.view_top = line.saturating_sub(scrolloff);
        }
        if line + scrolloff >= buffer.view_top + height {
            buffer.view_top = (line + scrolloff + 1).saturating_sub(height);
        }
        buffer.view_top = buffer.view_top.min(last);

        let column = text::screen_col(text::line(&buffer.rope, line), buffer.cursor.col, tab_width);
        if column < buffer.view_left {
            buffer.view_left = column;
        } else if column >= buffer.view_left + width {
            buffer.view_left = column + 1 - width;
        }
    }

    /// Called once per keystroke that actually came from the terminal, as
    /// opposed to one replayed by `.` or a macro.
    pub fn begin_input(&mut self) {
        self.message = None;
        self.replay_budget = REPLAY_BUDGET;
    }

    pub fn queue_keys(&mut self, keys: &[KeyEvent]) {
        if keys.len() > self.replay_budget {
            self.queue.clear();
            self.replay_budget = 0;
            self.set_error("E169: Command too recursive (aborted)");
            return;
        }
        self.replay_budget -= keys.len();
        for key in keys {
            self.queue.push_back(*key);
        }
    }

    // -- buffer management --------------------------------------------------

    pub fn open_file(&mut self, path: &Path) -> Result<()> {
        if let Some(index) = self
            .buffers
            .iter()
            .position(|b| b.path.as_deref() == Some(path))
        {
            self.current = index;
            self.set_message(format!("\"{}\"", self.buffer().display_name()));
            return Ok(());
        }
        let id = self.take_id();
        let buffer = Buffer::open(id, path)?;
        let is_new = buffer.is_new_file;
        let lines = buffer.line_count();
        // Replace an untouched scratch buffer rather than stacking onto it.
        let index = if self.buffer().is_empty_scratch() && self.buffers.len() == 1 {
            self.buffers[0] = buffer;
            0
        } else {
            self.buffers.push(buffer);
            self.buffers.len() - 1
        };
        self.current = index;
        self.detect_syntax(index);
        let name = self.buffer().display_name();
        if is_new {
            self.set_message(format!("\"{name}\" [New]"));
        } else {
            self.set_message(format!("\"{name}\" {lines}L"));
        }
        Ok(())
    }

    pub fn switch_to(&mut self, index: usize) {
        if index < self.buffers.len() {
            self.current = index;
        }
    }

    pub fn cycle_buffer(&mut self, forward: bool) {
        if self.buffers.len() < 2 {
            self.set_error("only one buffer");
            return;
        }
        let len = self.buffers.len();
        self.current = if forward {
            (self.current + 1) % len
        } else {
            (self.current + len - 1) % len
        };
    }

    pub fn close_buffer(&mut self, force: bool) {
        if self.buffer().is_modified() && !force {
            self.set_error("E89: No write since last change (add ! to override)");
            return;
        }
        if self.buffers.len() == 1 {
            let id = self.take_id();
            self.buffers[0] = Buffer::scratch(id);
            self.detect_syntax(0);
            return;
        }
        self.buffers.remove(self.current);
        self.current = self.current.min(self.buffers.len() - 1);
    }

    pub fn modified_buffers(&self) -> Vec<&Buffer> {
        self.buffers.iter().filter(|b| b.is_modified()).collect()
    }

    // -- cursor helpers -----------------------------------------------------

    pub fn clamp_cursor(&mut self) {
        let allow_eol = self.mode.allows_eol();
        let buffer = self.buffer_mut();
        buffer.cursor = text::clamp(&buffer.rope, buffer.cursor, allow_eol);
    }

    pub fn set_cursor(&mut self, position: Position) {
        let allow_eol = self.mode.allows_eol();
        let buffer = self.buffer_mut();
        buffer.cursor = text::clamp(&buffer.rope, position, allow_eol);
        buffer.desired_col = buffer.cursor.col;
    }

    /// Move vertically while remembering the column the user was aiming for.
    pub fn move_vertical(&mut self, line: usize) {
        let allow_eol = self.mode.allows_eol();
        let buffer = self.buffer_mut();
        let last = buffer.line_count().saturating_sub(1);
        let line = line.min(last);
        let want = buffer.desired_col;
        buffer.cursor = text::clamp(&buffer.rope, Position::new(line, want), allow_eol);
    }

    // -- editing actions ----------------------------------------------------

    pub fn enter_insert(&mut self) {
        self.mode = Mode::Insert;
        self.buffer_mut().begin();
    }

    pub fn leave_insert(&mut self) {
        let buffer = self.buffer_mut();
        buffer.end();
        buffer.cursor.col = buffer.cursor.col.saturating_sub(1);
        self.mode = Mode::Normal;
        self.clamp_cursor();
        let buffer = self.buffer_mut();
        buffer.desired_col = buffer.cursor.col;
    }

    pub fn indent_of(&self, line: usize) -> String {
        let rope = &self.buffer().rope;
        let end = text::first_non_blank(rope, line);
        text::line(rope, line).chars().take(end).collect()
    }

    /// `o` and `O`.
    pub fn open_line(&mut self, below: bool) {
        let line = self.buffer().cursor.line;
        let indent = self.indent_of(line);
        self.dot.changed = true;
        self.enter_insert();
        let buffer = self.buffer_mut();
        let at = if below {
            let last = buffer.line_count().saturating_sub(1);
            if line >= last {
                buffer.rope.len_chars()
            } else {
                buffer.rope.line_to_char(line + 1)
            }
        } else {
            buffer.rope.line_to_char(line)
        };
        buffer.insert(at, &format!("{indent}\n"));
        let new_line = if below { line + 1 } else { line };
        buffer.cursor = Position::new(new_line, indent.chars().count());
        buffer.desired_col = buffer.cursor.col;
    }

    pub fn insert_char(&mut self, c: char) {
        self.dot.changed = true;
        let at = self.buffer().cursor_char();
        let buffer = self.buffer_mut();
        buffer.insert(at, &c.to_string());
        buffer.cursor.col += 1;
        buffer.desired_col = buffer.cursor.col;
    }

    pub fn insert_text(&mut self, content: &str) {
        if content.is_empty() {
            return;
        }
        self.dot.changed = true;
        let at = self.buffer().cursor_char();
        let buffer = self.buffer_mut();
        buffer.insert(at, content);
        let end = at + content.chars().count();
        buffer.cursor = text::char_to_pos(&buffer.rope, end);
        buffer.desired_col = buffer.cursor.col;
    }

    /// Newline in insert mode, carrying the current line's indentation.
    pub fn insert_newline(&mut self) {
        self.dot.changed = true;
        let line = self.buffer().cursor.line;
        let indent = self.indent_of(line);
        let at = self.buffer().cursor_char();
        let buffer = self.buffer_mut();
        buffer.insert(at, &format!("\n{indent}"));
        buffer.cursor = Position::new(line + 1, indent.chars().count());
        buffer.desired_col = buffer.cursor.col;
    }

    pub fn insert_tab(&mut self) {
        let (expand, width) = (self.config.editor.expand_tab, self.config.editor.tab_width);
        if expand {
            let column = {
                let buffer = self.buffer();
                text::screen_col(
                    text::line(&buffer.rope, buffer.cursor.line),
                    buffer.cursor.col,
                    width,
                )
            };
            let spaces = width - (column % width);
            self.insert_text(&" ".repeat(spaces));
        } else {
            self.insert_text("\t");
        }
    }

    /// Backspace in insert mode: unindent, delete a character, or join lines.
    pub fn insert_backspace(&mut self) {
        let cursor = self.buffer().cursor;
        if cursor.col == 0 {
            if cursor.line == 0 {
                return;
            }
            self.dot.changed = true;
            let previous_len = self.buffer().line_len(cursor.line - 1);
            let at = self.buffer().rope.line_to_char(cursor.line);
            let buffer = self.buffer_mut();
            buffer.remove(at - 1, at);
            buffer.cursor = Position::new(cursor.line - 1, previous_len);
            buffer.desired_col = buffer.cursor.col;
            return;
        }

        self.dot.changed = true;
        // Delete a whole indent step when sitting in leading whitespace.
        let indent_end = text::first_non_blank(&self.buffer().rope, cursor.line);
        let width = self.config.editor.shift_width;
        let count = if self.config.editor.expand_tab && cursor.col <= indent_end && cursor.col > 0 {
            let column = {
                let buffer = self.buffer();
                text::screen_col(
                    text::line(&buffer.rope, cursor.line),
                    cursor.col,
                    self.config.editor.tab_width,
                )
            };
            let back = if column % width == 0 {
                width
            } else {
                column % width
            };
            back.min(cursor.col)
        } else {
            1
        };
        let at = self.buffer().cursor_char();
        let buffer = self.buffer_mut();
        buffer.remove(at - count, at);
        buffer.cursor.col -= count;
        buffer.desired_col = buffer.cursor.col;
    }

    pub fn insert_delete_forward(&mut self) {
        let at = self.buffer().cursor_char();
        let len = self.buffer().rope.len_chars();
        if at + 1 > len {
            return;
        }
        self.dot.changed = true;
        self.buffer_mut().remove(at, at + 1);
    }

    /// Ctrl-W in insert mode.
    pub fn insert_delete_word(&mut self) {
        let at = self.buffer().cursor_char();
        if at == 0 {
            return;
        }
        let start = crate::motion::prev_word_start(&self.buffer().rope, at, false);
        if start >= at {
            return;
        }
        self.dot.changed = true;
        let buffer = self.buffer_mut();
        buffer.remove(start, at);
        buffer.cursor = text::char_to_pos(&buffer.rope, start);
        buffer.desired_col = buffer.cursor.col;
    }

    /// Ctrl-U in insert mode.
    pub fn insert_delete_to_line_start(&mut self) {
        let cursor = self.buffer().cursor;
        if cursor.col == 0 {
            return;
        }
        self.dot.changed = true;
        let line_start = self.buffer().rope.line_to_char(cursor.line);
        let at = self.buffer().cursor_char();
        let buffer = self.buffer_mut();
        buffer.remove(line_start, at);
        buffer.cursor = Position::new(cursor.line, 0);
        buffer.desired_col = 0;
    }

    /// `x` and `X`.
    pub fn delete_chars(&mut self, count: usize, before: bool, register: Option<char>) {
        let cursor = self.buffer().cursor;
        let line_len = self.buffer().line_len(cursor.line);
        if line_len == 0 {
            return;
        }
        let (from_col, to_col) = if before {
            (cursor.col.saturating_sub(count), cursor.col)
        } else {
            (cursor.col, (cursor.col + count).min(line_len))
        };
        if from_col == to_col {
            return;
        }
        self.dot.changed = true;
        let line_start = self.buffer().rope.line_to_char(cursor.line);
        let removed = self
            .buffer()
            .slice(line_start + from_col, line_start + to_col);
        self.registers
            .delete(register, RegisterContent::charwise(removed));
        let buffer = self.buffer_mut();
        buffer.remove(line_start + from_col, line_start + to_col);
        buffer.cursor = text::clamp(&buffer.rope, Position::new(cursor.line, from_col), false);
        buffer.desired_col = buffer.cursor.col;
    }

    /// `p` and `P`.
    pub fn put(&mut self, register: Option<char>, after: bool, count: usize) {
        let Some(content) = self.registers.get(register).cloned() else {
            self.set_error(format!(
                "E353: Nothing in register {}",
                register.unwrap_or('"')
            ));
            return;
        };
        if content.text.is_empty() {
            return;
        }
        self.dot.changed = true;
        let cursor = self.buffer().cursor;

        match content.kind {
            RegisterKind::Linewise => {
                let mut text_to_insert = content.text.repeat(count);
                if !text_to_insert.ends_with('\n') {
                    text_to_insert.push('\n');
                }
                let buffer = self.buffer_mut();
                let at = if after {
                    let last = buffer.line_count().saturating_sub(1);
                    if cursor.line >= last {
                        buffer.rope.len_chars()
                    } else {
                        buffer.rope.line_to_char(cursor.line + 1)
                    }
                } else {
                    buffer.rope.line_to_char(cursor.line)
                };
                buffer.begin();
                buffer.insert(at, &text_to_insert);
                buffer.end();
                let line = buffer.rope.char_to_line(at);
                buffer.cursor = Position::new(line, text::first_non_blank(&buffer.rope, line));
                buffer.desired_col = buffer.cursor.col;
            }
            RegisterKind::Charwise => {
                let text_to_insert = content.text.repeat(count);
                let line_len = self.buffer().line_len(cursor.line);
                let column = if after && line_len > 0 {
                    (cursor.col + 1).min(line_len)
                } else {
                    cursor.col
                };
                let at = self.buffer().rope.line_to_char(cursor.line) + column;
                let inserted = text_to_insert.chars().count();
                let buffer = self.buffer_mut();
                buffer.begin();
                buffer.insert(at, &text_to_insert);
                buffer.end();
                let end = at + inserted;
                buffer.cursor = text::clamp(
                    &buffer.rope,
                    text::char_to_pos(&buffer.rope, end.saturating_sub(1)),
                    false,
                );
                buffer.desired_col = buffer.cursor.col;
            }
        }
    }

    /// `J`: join `count` lines, collapsing whitespace the way Vim does.
    pub fn join_lines(&mut self, count: usize) {
        let joins = count.max(2) - 1;
        let start_line = self.buffer().cursor.line;
        self.buffer_mut().begin();
        let mut final_col = self.buffer().cursor.col;
        for _ in 0..joins {
            let line = start_line;
            if line + 1 >= self.buffer().line_count() {
                break;
            }
            self.dot.changed = true;
            let rope = &self.buffer().rope;
            let trimmed_end = text::line(rope, line)
                .chars()
                .collect::<Vec<_>>()
                .iter()
                .rposition(|c| !c.is_whitespace())
                .map(|i| i + 1)
                .unwrap_or(0);
            let next_indent = text::first_non_blank(rope, line + 1);
            let next_first = text::char_at(rope, Position::new(line + 1, next_indent));
            let line_start = rope.line_to_char(line);
            let next_start = rope.line_to_char(line + 1);

            let separator = match next_first {
                None => "",
                Some(')') => "",
                _ if trimmed_end == 0 => "",
                _ => " ",
            };
            let from = line_start + trimmed_end;
            let to = next_start + next_indent;
            let buffer = self.buffer_mut();
            buffer.replace(from, to, separator);
            final_col = trimmed_end;
        }
        let buffer = self.buffer_mut();
        buffer.cursor = text::clamp(&buffer.rope, Position::new(start_line, final_col), false);
        buffer.desired_col = buffer.cursor.col;
        buffer.end();
    }

    /// `r`: overwrite `count` characters with one character.
    pub fn replace_chars(&mut self, c: char, count: usize) {
        let cursor = self.buffer().cursor;
        let line_len = self.buffer().line_len(cursor.line);
        if cursor.col + count > line_len {
            return;
        }
        self.dot.changed = true;
        let at = self.buffer().cursor_char();
        let buffer = self.buffer_mut();
        buffer.begin();
        buffer.replace(at, at + count, &c.to_string().repeat(count));
        buffer.end();
        buffer.cursor = Position::new(cursor.line, cursor.col + count - 1);
        buffer.desired_col = buffer.cursor.col;
    }

    /// `~`: toggle the case of `count` characters and step past them.
    pub fn toggle_case(&mut self, count: usize) {
        let cursor = self.buffer().cursor;
        let line_len = self.buffer().line_len(cursor.line);
        if line_len == 0 {
            return;
        }
        let end_col = (cursor.col + count).min(line_len);
        let at = self.buffer().cursor_char();
        let end = at + (end_col - cursor.col);
        let original = self.buffer().slice(at, end);
        let flipped: String = original
            .chars()
            .map(|c| {
                if c.is_lowercase() {
                    c.to_uppercase().next().unwrap_or(c)
                } else {
                    c.to_lowercase().next().unwrap_or(c)
                }
            })
            .collect();
        if flipped == original {
            self.set_cursor(Position::new(cursor.line, end_col));
            return;
        }
        self.dot.changed = true;
        let buffer = self.buffer_mut();
        buffer.begin();
        buffer.replace(at, end, &flipped);
        buffer.end();
        buffer.cursor = text::clamp(&buffer.rope, Position::new(cursor.line, end_col), false);
        buffer.desired_col = buffer.cursor.col;
    }

    /// `R` mode: overwrite one character, extending the line if needed.
    pub fn replace_at_cursor(&mut self, c: char) {
        self.dot.changed = true;
        let cursor = self.buffer().cursor;
        let line_len = self.buffer().line_len(cursor.line);
        let at = self.buffer().cursor_char();
        let buffer = self.buffer_mut();
        if cursor.col < line_len {
            buffer.replace(at, at + 1, &c.to_string());
        } else {
            buffer.insert(at, &c.to_string());
        }
        buffer.cursor.col += 1;
        buffer.desired_col = buffer.cursor.col;
    }

    // -- visual mode --------------------------------------------------------

    pub fn enter_visual(&mut self, kind: VisualKind) {
        if self.mode == Mode::Visual(kind) {
            self.mode = Mode::Normal;
            self.clamp_cursor();
            return;
        }
        if !self.mode.is_visual() {
            self.visual_anchor = self.buffer().cursor;
        }
        self.mode = Mode::Visual(kind);
    }

    pub fn visual_range(&self) -> EditRange {
        let buffer = self.buffer();
        let (from, to) = if self.visual_anchor <= buffer.cursor {
            (self.visual_anchor, buffer.cursor)
        } else {
            (buffer.cursor, self.visual_anchor)
        };
        match self.mode {
            Mode::Visual(VisualKind::Line) => {
                let last = buffer.line_count().saturating_sub(1);
                let start = buffer.rope.line_to_char(from.line);
                let end = if to.line >= last {
                    buffer.rope.len_chars()
                } else {
                    buffer.rope.line_to_char(to.line + 1)
                };
                EditRange {
                    start,
                    end,
                    linewise: true,
                }
            }
            _ => {
                let start = text::pos_to_char(&buffer.rope, from);
                let end = (text::pos_to_char(&buffer.rope, to) + 1).min(buffer.rope.len_chars());
                EditRange {
                    start,
                    end,
                    linewise: false,
                }
            }
        }
    }

    /// Selected lines, for `:'<,'>` ranges.
    pub fn visual_lines(&self) -> (usize, usize) {
        let cursor = self.buffer().cursor;
        if self.visual_anchor.line <= cursor.line {
            (self.visual_anchor.line, cursor.line)
        } else {
            (cursor.line, self.visual_anchor.line)
        }
    }

    pub fn leave_visual(&mut self) {
        self.mode = Mode::Normal;
        self.clamp_cursor();
    }
}

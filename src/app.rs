//! Editor state and the editing actions the key layer drives.

use crate::config::{Config, LineNumbers};
use crate::core::buffer::Buffer;
use crate::core::search::Search;
use crate::core::text::{self, Position};
use crate::edit::motion::{EditRange, FindTarget};
use crate::edit::operator::EditOptions;
use crate::edit::register::{RegisterContent, RegisterKind, Registers};
use crate::keymap::Pending;
use crate::mode::{Mode, VisualKind};
use crate::session::{RemoteCursor, Session};
use crate::syntax::SyntaxEngine;
use crate::view::layout::{Direction, WindowId};
use crate::view::picker::{Action, Item, Picker, Source};
use crate::view::window::Workspace;
use anyhow::Result;
use crossterm::event::KeyEvent;
use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use syntect::highlighting::Theme;

/// Keys a single real keystroke may cause to be replayed. A recursive macro
/// keeps the queue short while never draining it, so a size cap alone would
/// not catch it; this budget bounds the total work instead.
const REPLAY_BUDGET: usize = 200_000;

/// Why background tools are being asked to run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trigger {
    Open,
    Save,
    /// Typing paused.
    Change,
    /// `:check`, which ignores the per-trigger settings.
    Manual,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MessageKind {
    Info,
    Error,
}

#[derive(Clone)]
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

#[derive(Clone)]
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
    /// Present while this editor is hosting a shared session.
    pub session: Option<Session>,
    /// Every participant's caret, republished after each command so the
    /// renderer can draw them without borrowing the session.
    pub remote_cursors: Vec<RemoteCursor>,
    /// Whose view is being drawn right now; their own caret is drawn by the
    /// terminal, not as a remote one.
    pub rendering_as: u32,
    /// Guest count when sharing, for the status line. Kept separately because
    /// the session itself is checked out while frames are drawn.
    pub shared_guests: Option<usize>,
    /// This person's windows and how they are arranged.
    pub workspace: Workspace,
    /// The filterable list on top of everything, when one is open.
    pub picker: Option<Picker>,
    /// Suggestions for the word being typed.
    pub completion: Option<crate::edit::complete::Completion>,
    /// Loaded plugins and the commands they contribute.
    pub plugins: crate::plugin::Plugins,
    /// An AI answer waiting to be accepted or thrown away.
    pub proposal: Option<crate::ai::proposal::Proposal>,
    /// The transcript behind the sidebar's chat view.
    pub conversation: crate::ai::Conversation,
    /// A request is in flight.
    pub asking: bool,
    /// Background external tools.
    pub runner: crate::tools::external::Runner,
    pub checkers: Vec<crate::tools::diagnostics::Checker>,
    pub formatters: Vec<crate::tools::format::Formatter>,
    /// When the buffer last changed, so background work waits for a pause in
    /// typing rather than running on every keystroke.
    last_change: Option<Instant>,
    /// Tools already reported as missing, so the message appears once.
    reported_missing: HashSet<String>,
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

        let workspace = Workspace::new(0, &config.sidebar);
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
            session: None,
            remote_cursors: Vec::new(),
            rendering_as: crate::session::HOST_ID,
            shared_guests: None,
            workspace,
            picker: None,
            completion: None,
            plugins: crate::plugin::Plugins::default(),
            proposal: None,
            conversation: crate::ai::Conversation::default(),
            asking: false,
            runner: crate::tools::external::Runner::default(),
            checkers: Vec::new(),
            formatters: Vec::new(),
            last_change: None,
            reported_missing: HashSet::new(),
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
        app.plugins = match crate::plugin::default_directory() {
            Some(directory) => crate::plugin::Plugins::load(&directory),
            None => crate::plugin::Plugins::default(),
        };
        if let Some(problem) = app.plugins.problems.first().cloned() {
            app.set_error(format!("plugin: {problem}"));
        }
        app.rebuild_tools();
        for index in 0..app.buffers.len() {
            app.refresh_buffer(index, Trigger::Open);
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

    /// Compile the configured checkers and formatters. The user's entries come
    /// first so they win the first-match lookup, then the built-ins fill in
    /// filetypes the user did not mention.
    pub fn rebuild_tools(&mut self) {
        let mut problems = Vec::new();

        let mut checker_configs = self.config.diagnostics.checker.clone();
        if self.config.diagnostics.use_builtin {
            checker_configs.extend(crate::tools::diagnostics::builtin_checkers());
        }
        self.checkers = checker_configs
            .iter()
            .filter_map(
                |config| match crate::tools::diagnostics::Checker::compile(config) {
                    Ok(checker) => Some(checker),
                    Err(e) => {
                        problems.push(e);
                        None
                    }
                },
            )
            .collect();

        let mut formatter_configs = self.config.format.formatter.clone();
        if self.config.format.use_builtin {
            formatter_configs.extend(crate::tools::format::builtin_formatters());
        }
        self.formatters = formatter_configs
            .iter()
            .filter_map(
                |config| match crate::tools::format::Formatter::compile(config) {
                    Ok(formatter) => Some(formatter),
                    Err(e) => {
                        problems.push(e);
                        None
                    }
                },
            )
            .collect();

        if let Some(first) = problems.first() {
            self.set_error(first.clone());
        }
    }

    /// Note that the text changed, restarting the debounce.
    pub fn note_change(&mut self) {
        self.last_change = Some(Instant::now());
    }

    /// Tell the plugins something happened.
    pub fn announce(&mut self, event: crate::plugin::Event) {
        self.plugins.dispatch(event);
    }

    /// Start the checkers and the git diff for one buffer.
    ///
    /// The trigger decides what runs: the configuration says whether checkers
    /// follow an open, a save or a pause in typing, while `Manual` is `:check`
    /// and always runs everything.
    pub fn refresh_buffer(&mut self, index: usize, trigger: Trigger) {
        if index >= self.buffers.len() {
            return;
        }
        let revision = self.buffers[index].history.revision();
        let (id, name, path, syntax_name) = {
            let buffer = &self.buffers[index];
            (
                buffer.id,
                buffer.short_name(),
                buffer.path.clone(),
                buffer.syntax_name.clone(),
            )
        };
        let text: String = self.buffers[index].rope.chars().collect();
        let directory = path
            .as_deref()
            .and_then(|p| p.parent())
            .map(Path::to_path_buf);

        let diagnostics = &self.config.diagnostics;
        let check = diagnostics.enabled
            && match trigger {
                Trigger::Manual => true,
                Trigger::Open => diagnostics.on_open,
                Trigger::Save => diagnostics.on_save,
                Trigger::Change => diagnostics.on_change,
            };
        let force = trigger == Trigger::Manual;

        let snapshot = crate::tools::external::Snapshot {
            buffer_id: id,
            revision,
            name,
            text,
            directory,
            timeout: Duration::from_millis(self.config.diagnostics.timeout_ms),
        };

        if check && (force || self.buffers[index].checked_revision != Some(revision)) {
            self.buffers[index].checked_revision = Some(revision);
            let applicable: Vec<crate::tools::diagnostics::Checker> = self
                .checkers
                .iter()
                .filter(|checker| checker.applies_to(&syntax_name, path.as_deref()))
                .filter(|checker| !self.reported_missing.contains(&checker.command[0]))
                .cloned()
                .collect();
            for checker in applicable {
                self.runner.check(checker, snapshot.clone());
            }
        }

        if self.config.signs.enabled && self.config.signs.git {
            if let Some(path) = path {
                if force || self.buffers[index].diffed_revision != Some(revision) {
                    self.buffers[index].diffed_revision = Some(revision);
                    self.runner.diff(path, snapshot);
                }
            }
        }
    }

    /// Once typing pauses, refresh whatever follows a change.
    pub fn tick_background(&mut self) -> bool {
        let Some(changed_at) = self.last_change else {
            return false;
        };
        let debounce = Duration::from_millis(self.config.diagnostics.debounce_ms.max(50));
        if changed_at.elapsed() < debounce {
            return false;
        }
        self.last_change = None;
        self.refresh_buffer(self.current, Trigger::Change);
        false
    }

    /// Apply whatever the background tools have finished.
    pub fn poll_background(&mut self) -> bool {
        let finished = self.runner.poll();
        if finished.is_empty() {
            return false;
        }
        let mut changed = false;
        for item in finished {
            match item {
                crate::tools::external::Finished::Answered { request, result } => {
                    self.asking = false;
                    changed = true;
                    match result {
                        Ok(replacement) => self.receive_proposal(request, replacement),
                        Err(e) => {
                            self.conversation
                                .say(crate::ai::Turn::Note(format!("failed: {e}")));
                            self.set_error(format!("ai: {e}"));
                        }
                    }
                }
                crate::tools::external::Finished::Listed { files, truncated } => {
                    let Some(picker) = self.picker.as_mut() else {
                        continue;
                    };
                    if picker.source != Source::Files {
                        continue;
                    }
                    let items = files
                        .into_iter()
                        .map(|file| {
                            let name = std::path::Path::new(&file)
                                .file_name()
                                .map(|n| n.to_string_lossy().into_owned())
                                .unwrap_or_default();
                            Item::new(
                                file.clone(),
                                name,
                                Action::OpenFile(std::path::PathBuf::from(file)),
                            )
                        })
                        .collect::<Vec<_>>();
                    let count = items.len();
                    picker.set_items(items);
                    picker.title = if truncated {
                        format!("Files ({count}, truncated)")
                    } else {
                        format!("Files ({count})")
                    };
                    changed = true;
                }
                crate::tools::external::Finished::Grepped { pattern, result } => {
                    let Some(picker) = self.picker.as_mut() else {
                        continue;
                    };
                    if picker.source != Source::Grep {
                        continue;
                    }
                    match result {
                        Ok(matches) => {
                            let count = matches.len();
                            let items = matches
                                .into_iter()
                                .map(|hit| {
                                    Item::new(
                                        format!("{}:{}: {}", hit.path, hit.line + 1, hit.text),
                                        String::new(),
                                        Action::Goto {
                                            path: Some(std::path::PathBuf::from(hit.path)),
                                            line: hit.line,
                                            col: hit.col,
                                        },
                                    )
                                })
                                .collect::<Vec<_>>();
                            picker.set_items(items);
                            picker.title = format!("{count} match(es) for {pattern:?}");
                        }
                        Err(e) => {
                            picker.loading = false;
                            picker.title = format!("search failed: {e}");
                        }
                    }
                    changed = true;
                }
                crate::tools::external::Finished::Checked {
                    buffer_id,
                    revision,
                    tool,
                    result,
                } => {
                    let Some(index) = self.buffers.iter().position(|b| b.id == buffer_id) else {
                        continue;
                    };
                    // Discard results for text that has since changed.
                    if self.buffers[index].history.revision() != revision {
                        continue;
                    }
                    match result {
                        Ok(items) => {
                            self.buffers[index].diagnostics.replace(&tool, items);
                            let (errors, warnings, _) = self.buffers[index].diagnostics.counts();
                            let id = self.buffers[index].id;
                            self.announce(crate::plugin::Event::DiagnosticsUpdated {
                                buffer: id,
                                errors,
                                warnings,
                            });
                            changed = true;
                        }
                        Err(failure) => {
                            let key = tool.clone();
                            if failure.missing {
                                if self.reported_missing.insert(key) {
                                    self.set_message(failure.message);
                                    changed = true;
                                }
                            } else {
                                self.set_error(failure.message);
                                changed = true;
                            }
                        }
                    }
                }
                crate::tools::external::Finished::Diffed {
                    buffer_id,
                    revision,
                    result,
                } => {
                    let Some(index) = self.buffers.iter().position(|b| b.id == buffer_id) else {
                        continue;
                    };
                    if self.buffers[index].history.revision() != revision {
                        continue;
                    }
                    if let Ok(statuses) = result {
                        self.buffers[index].line_statuses = statuses;
                        changed = true;
                    }
                }
            }
        }
        changed
    }

    /// Strip trailing spaces and tabs from every line, as one undo step.
    pub fn trim_trailing_whitespace(&mut self) -> usize {
        let mut trimmed = 0;
        let lines = self.buffer().line_count();
        self.buffer_mut().begin();
        for line in (0..lines).rev() {
            let content = text::line(&self.buffer().rope, line);
            let length = content.len_chars();
            let kept = content
                .chars()
                .collect::<Vec<char>>()
                .iter()
                .rposition(|c| *c != ' ' && *c != '\t')
                .map(|i| i + 1)
                .unwrap_or(0);
            if kept < length {
                let start = self.buffer().rope.line_to_char(line) + kept;
                let end = start + (length - kept);
                self.buffer_mut().remove(start, end);
                trimmed += 1;
            }
        }
        self.buffer_mut().end();
        if trimmed > 0 {
            self.clamp_cursor();
        }
        trimmed
    }

    // -- windows ------------------------------------------------------------
    //
    // The focused window's cursor and viewport live on the buffer, so the rest
    // of the editor never has to know about windows. These three move state
    // between the two representations, and every focus change goes through
    // them.

    /// Write the live cursor and viewport into the focused window.
    pub fn sync_window(&mut self) {
        let buffer_index = self.current;
        let anchor = self.visual_anchor;
        let (cursor, desired_col, view_top, view_left) = {
            let buffer = self.buffer();
            (
                buffer.cursor,
                buffer.desired_col,
                buffer.view_top,
                buffer.view_left,
            )
        };
        let window = self.workspace.focused_mut();
        window.buffer_index = buffer_index;
        window.cursor = cursor;
        window.desired_col = desired_col;
        window.view_top = view_top;
        window.view_left = view_left;
        window.visual_anchor = anchor;
    }

    /// Make the focused window's state live.
    pub fn load_window(&mut self) {
        let window = self.workspace.focused().clone();
        self.current = window
            .buffer_index
            .min(self.buffers.len().saturating_sub(1));
        self.visual_anchor = window.visual_anchor;
        let buffer = self.buffer_mut();
        buffer.cursor = window.cursor;
        buffer.desired_col = window.desired_col;
        buffer.view_top = window.view_top;
        buffer.view_left = window.view_left;
        self.clamp_cursor();
    }

    pub fn focus_window(&mut self, id: WindowId) {
        if id == self.workspace.focused_id() {
            return;
        }
        self.sync_window();
        if self.workspace.focus(id) {
            self.load_window();
        }
    }

    pub fn focus_next_window(&mut self, forward: bool) {
        let target = if forward {
            self.workspace.next()
        } else {
            self.workspace.previous()
        };
        self.focus_window(target);
    }

    pub fn focus_toward(&mut self, direction: Direction) {
        match self.workspace.toward(direction) {
            Some(id) => self.focus_window(id),
            None => self.set_error("no window that way"),
        }
    }

    /// `:split` and `:vsplit`. The new window shows the same place in the same
    /// buffer, as Vim does, so a split is a second view rather than a jump.
    pub fn split_window(&mut self, vertical: bool) {
        self.sync_window();
        let new = self.workspace.split(vertical);
        self.workspace.focus(new);
        self.load_window();
    }

    pub fn close_window(&mut self) -> bool {
        self.sync_window();
        if self.workspace.close_focused() {
            self.load_window();
            true
        } else {
            self.set_error("cannot close the last window");
            false
        }
    }

    // -- sidebar ------------------------------------------------------------

    pub fn toggle_sidebar(&mut self) {
        self.workspace.sidebar.toggle();
    }

    /// Move the keyboard between the sidebar and the windows.
    pub fn focus_sidebar(&mut self, focused: bool) {
        let sidebar = &mut self.workspace.sidebar;
        if focused && !sidebar.visible {
            sidebar.visible = true;
            sidebar.explorer.refresh();
        }
        sidebar.focused = focused && sidebar.visible;
    }

    pub fn sidebar_focused(&self) -> bool {
        let sidebar = &self.workspace.sidebar;
        sidebar.visible && sidebar.focused
    }

    /// Open whatever the explorer has selected, and hand the keyboard back to
    /// the window so you can start editing.
    pub fn explorer_activate(&mut self) {
        let chosen = self.workspace.sidebar.explorer.activate();
        if let Some(path) = chosen {
            match self.open_file(&path) {
                Ok(()) => {
                    self.focus_sidebar(false);
                    self.sync_window();
                }
                Err(e) => self.set_error(format!("{e}")),
            }
        }
    }

    pub fn only_window(&mut self) {
        self.sync_window();
        if !self.workspace.only() {
            self.set_message("already the only window");
        }
    }

    // -- completion ---------------------------------------------------------

    /// Recompute the suggestion list for the word under the cursor.
    ///
    /// `forced` is Ctrl-N: it offers suggestions however little has been
    /// typed, where the automatic path waits for `complete_min_chars`.
    pub fn update_completion(&mut self, forced: bool) {
        use crate::edit::complete;

        if !forced && !self.config.editor.auto_complete {
            self.completion = None;
            return;
        }
        let cursor = self.buffer().cursor;
        let (prefix, start) = complete::prefix_at(&self.buffer().rope, cursor);
        let minimum = if forced {
            complete::MIN_PREFIX
        } else {
            self.config
                .editor
                .complete_min_chars
                .max(complete::MIN_PREFIX)
        };
        if prefix.chars().count() < minimum {
            self.completion = None;
            return;
        }

        let current = self.current;
        let syntax_name = self.buffer().syntax_name.clone();
        let ropes: Vec<(&ropey::Rope, bool)> = self
            .buffers
            .iter()
            .enumerate()
            .map(|(index, buffer)| (&buffer.rope, index == current))
            .collect();
        let items = complete::candidates(&ropes, cursor.line, &syntax_name, &prefix);

        self.completion = if items.is_empty() {
            None
        } else {
            Some(complete::Completion {
                items,
                selected: 0,
                start,
                prefix,
            })
        };
    }

    /// Keep an open list in step with what is being typed, without opening one.
    fn refresh_completion(&mut self) {
        if self.completion.is_some() || self.config.editor.auto_complete {
            self.update_completion(false);
        }
    }

    pub fn move_completion(&mut self, delta: isize) {
        if self.completion.is_none() {
            self.update_completion(true);
            return;
        }
        if let Some(completion) = self.completion.as_mut() {
            completion.move_selection(delta);
        }
    }

    /// Replace the typed prefix with the highlighted suggestion.
    pub fn accept_completion(&mut self) -> bool {
        let Some(completion) = self.completion.take() else {
            return false;
        };
        let Some(word) = completion.selection().map(str::to_string) else {
            return false;
        };
        if word == completion.prefix {
            return false;
        }
        self.dot.changed = true;
        let start = text::pos_to_char(&self.buffer().rope, completion.start);
        let end = self.buffer().cursor_char();
        let buffer = self.buffer_mut();
        buffer.begin();
        buffer.replace(start, end, &word);
        buffer.end();
        buffer.cursor = text::char_to_pos(&buffer.rope, start + word.chars().count());
        buffer.desired_col = buffer.cursor.col;
        true
    }

    // -- ai -----------------------------------------------------------------

    /// Ask the provider to rewrite a range of lines, or the current line.
    ///
    /// The range comes from the ex command, so `:'<,'>ai …` works — pressing
    /// `:` in visual mode prefills that range, which is how a selection
    /// reaches here after visual mode has already been left behind.
    pub fn ask_ai(&mut self, range: Option<(usize, usize)>, instruction: &str) {
        if !self.config.ai.enabled {
            self.set_error("ai is off; set ai.enabled and ai.command (see :help)");
            return;
        }
        if instruction.trim().is_empty() {
            self.set_error("usage: :ai <what to do>");
            return;
        }
        if self.asking {
            self.set_error("already waiting for an answer");
            return;
        }

        let (start, end) = match range {
            Some((first, last)) => {
                let buffer = self.buffer();
                let last = last.min(buffer.line_count().saturating_sub(1));
                let start = buffer.rope.line_to_char(first);
                let end = if last + 1 >= buffer.line_count() {
                    buffer.rope.len_chars()
                } else {
                    buffer.rope.line_to_char(last + 1)
                };
                (start, end)
            }
            None if self.mode.is_visual() => {
                let range = self.visual_range();
                self.leave_visual();
                (range.start, range.end)
            }
            None => {
                let buffer = self.buffer();
                let line = buffer.cursor.line;
                let start = buffer.rope.line_to_char(line);
                let end = start + crate::core::text::line_len(&buffer.rope, line);
                (start, end)
            }
        };

        let original = self.buffer().slice(start, end);
        if original.trim().is_empty() {
            self.set_error("nothing selected to rewrite");
            return;
        }

        let request = crate::ai::Request {
            instruction: instruction.to_string(),
            buffer_id: self.buffer().id,
            start,
            end,
            original,
        };
        let context = self.context_around(start, end);
        let filetype = self.buffer().syntax_name.clone();
        let prompt = crate::ai::prompt(&request, &filetype, &context);

        self.conversation
            .say(crate::ai::Turn::You(instruction.to_string()));
        self.asking = true;
        self.set_message(format!("ai: {instruction}…"));
        let command = self.config.ai.command.clone();
        let timeout = crate::ai::timeout(&self.config.ai);
        self.runner.ask(command, prompt, request, timeout);
    }

    /// Lines around the region, so the provider can see what it is editing
    /// inside without being asked to rewrite it.
    fn context_around(&self, start: usize, end: usize) -> String {
        let span = self.config.ai.context_lines;
        if span == 0 {
            return String::new();
        }
        let buffer = self.buffer();
        let first = buffer.rope.char_to_line(start.min(buffer.rope.len_chars()));
        let last = buffer.rope.char_to_line(end.min(buffer.rope.len_chars()));
        let from = first.saturating_sub(span);
        let to = (last + span).min(buffer.line_count().saturating_sub(1));
        let mut out = String::new();
        for line in from..=to {
            if line >= first && line <= last {
                continue;
            }
            out.push_str(
                &crate::core::text::line(&buffer.rope, line)
                    .chars()
                    .collect::<String>(),
            );
            out.push('\n');
        }
        out
    }

    fn receive_proposal(&mut self, request: crate::ai::Request, replacement: String) {
        let replacement = crate::ai::match_line_shape(&request.original, &replacement);
        let proposal = crate::ai::proposal::Proposal {
            instruction: request.instruction.clone(),
            buffer_id: request.buffer_id,
            start: request.start,
            end: request.end,
            original: request.original,
            replacement,
            author: self.config.ai.author.clone(),
        };
        self.conversation.say(crate::ai::Turn::Assistant(format!(
            "{} line(s) to change",
            proposal.changed_lines()
        )));
        if proposal.replacement == proposal.original {
            self.set_message("ai: nothing to change");
            return;
        }
        let changed = proposal.changed_lines();
        self.proposal = Some(proposal);
        self.show_proposal();
        self.set_message(format!(
            "ai: {changed} line(s) proposed — :apply to keep, :discard to drop"
        ));
    }

    /// Show the pending proposal as a diff.
    pub fn show_proposal(&mut self) {
        let Some(proposal) = &self.proposal else {
            self.set_error("no proposal");
            return;
        };
        let mut lines = vec![format!("  {}", proposal.instruction), String::new()];
        lines.extend(proposal.diff().into_iter().map(|line| format!("  {line}")));
        lines.push(String::new());
        lines.push("  :apply to keep it · :discard to drop it".to_string());
        self.overlay = Some(Overlay {
            title: "AI proposal".to_string(),
            lines,
            scroll: 0,
        });
    }

    pub fn apply_proposal(&mut self) {
        let Some(proposal) = self.proposal.take() else {
            self.set_error("no proposal to apply");
            return;
        };
        let Some(index) = self
            .buffers
            .iter()
            .position(|buffer| buffer.id == proposal.buffer_id)
        else {
            self.set_error("the buffer it was written for has gone");
            return;
        };
        if !proposal.still_applies(&self.buffers[index]) {
            self.set_error("the text changed since it was proposed; discarded");
            return;
        }
        self.current = index;
        proposal.apply(&mut self.buffers[index]);
        self.sync_window();
        self.dot.changed = true;
        self.conversation
            .say(crate::ai::Turn::Note("applied".to_string()));
        self.set_message(format!("ai: applied ({})", proposal.author));
    }

    pub fn discard_proposal(&mut self) {
        if self.proposal.take().is_some() {
            self.conversation
                .say(crate::ai::Turn::Note("discarded".to_string()));
            self.set_message("ai: discarded");
        } else {
            self.set_error("no proposal to discard");
        }
    }

    // -- pickers ------------------------------------------------------------

    pub fn close_picker(&mut self) {
        self.picker = None;
    }

    fn project_root(&self) -> PathBuf {
        // The file's directory is a better guess than the process's, but the
        // process's is the right answer when nothing is open yet.
        std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
    }

    pub fn open_file_picker(&mut self) {
        let mut picker = Picker::new("Files", Source::Files);
        picker.loading = true;
        picker.title = "Files (listing…)".to_string();
        self.picker = Some(picker);
        let root = self.project_root();
        let limit = self.config.picker.max_files;
        self.runner.list_files(root, limit);
    }

    pub fn open_buffer_picker(&mut self) {
        let current = self.current;
        let items: Vec<Item> = self
            .buffers
            .iter()
            .enumerate()
            .map(|(index, buffer)| {
                let mut detail = String::new();
                if index == current {
                    detail.push_str("current ");
                }
                if buffer.is_modified() {
                    detail.push_str("modified");
                }
                Item::new(buffer.display_name(), detail, Action::Buffer(index))
            })
            .collect();
        self.picker = Some(Picker::with_items(
            format!("Buffers ({})", items.len()),
            Source::Buffers,
            items,
        ));
    }

    pub fn open_command_palette(&mut self) {
        let items: Vec<Item> = crate::command::COMMANDS
            .iter()
            .map(|spec| {
                let action = if spec.run.ends_with(' ') || spec.run.ends_with('/') {
                    Action::Prompt(spec.run.to_string())
                } else {
                    Action::Ex(spec.run.to_string())
                };
                Item::new(
                    spec.name,
                    format!("{}  ·  :{}", spec.description, spec.run.trim_end()),
                    action,
                )
            })
            .collect();
        let mut items = items;
        for command in self.plugins.commands() {
            items.push(Item::new(
                command.description.clone(),
                format!("{}  ·  :{}", command.plugin, command.name),
                Action::Ex(command.name.clone()),
            ));
        }
        self.picker = Some(Picker::with_items("Commands", Source::Commands, items));
    }

    pub fn open_grep_picker(&mut self, pattern: &str) {
        let mut picker = Picker::new(format!("Searching for {pattern:?}…"), Source::Grep);
        picker.loading = true;
        self.picker = Some(picker);
        let command = self.config.picker.grep_command.clone();
        let root = self.project_root();
        let timeout = Duration::from_millis(self.config.picker.timeout_ms);
        self.runner
            .grep(command, pattern.to_string(), root, timeout);
    }

    pub fn open_diagnostics_picker(&mut self) {
        let items: Vec<Item> = self
            .buffer()
            .diagnostics
            .sorted()
            .iter()
            .map(|diagnostic| {
                Item::new(
                    format!("{}: {}", diagnostic.line + 1, diagnostic.message),
                    format!("{} {}", diagnostic.severity.label(), diagnostic.source),
                    Action::Goto {
                        path: None,
                        line: diagnostic.line,
                        col: diagnostic.col.unwrap_or(0),
                    },
                )
            })
            .collect();
        if self.buffer().diagnostics.is_empty() {
            self.set_message("no diagnostics");
            return;
        }
        self.picker = Some(Picker::with_items(
            format!("Diagnostics ({})", items.len()),
            Source::Diagnostics,
            items,
        ));
    }

    /// Run whatever the highlighted item does.
    pub fn accept_picker(&mut self) {
        let Some(picker) = self.picker.as_ref() else {
            return;
        };
        let Some(action) = picker.selection().map(|item| item.action.clone()) else {
            // Nothing matched, so there is nothing to accept.
            return;
        };
        self.picker = None;

        match action {
            Action::OpenFile(path) => {
                if let Err(e) = self.open_file(&path) {
                    self.set_error(format!("{e}"));
                }
            }
            Action::Goto { path, line, col } => {
                if let Some(path) = path {
                    if let Err(e) = self.open_file(&path) {
                        self.set_error(format!("{e}"));
                        return;
                    }
                }
                crate::keymap::push_jump(self);
                self.set_cursor(Position::new(line, col));
            }
            Action::Ex(command) => crate::command::execute(self, &command),
            Action::Prompt(prefill) => {
                crate::command::open_prompt(self, PromptKind::Command, &prefill)
            }
            Action::Buffer(index) => {
                self.switch_to(index);
                self.sync_window();
            }
        }
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

    /// Width of the sign column, which diagnostics and git status share.
    pub fn sign_width(&self) -> usize {
        let signs = &self.config.signs;
        if signs.enabled && (signs.diagnostics || signs.git) {
            2
        } else {
            0
        }
    }

    pub fn number_width(&self) -> usize {
        match self.config.editor.line_numbers {
            LineNumbers::None => 0,
            _ => self.buffer().line_count().to_string().len().max(3) + 2,
        }
    }

    pub fn gutter_width(&self) -> usize {
        self.sign_width() + self.number_width()
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
        self.refresh_buffer(index, Trigger::Open);
        self.announce(crate::plugin::Event::BufferOpened {
            buffer: self.buffers[index].id,
            path: self.buffers[index].path.clone(),
        });
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
        let removed = self.current;
        self.buffers.remove(removed);
        self.current = removed.min(self.buffers.len() - 1);
        let fallback = self.current;
        self.workspace.buffer_removed(removed, fallback);
        self.sync_window();
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
        self.completion = None;
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
        use crate::edit::pairs::{self, Insertion};

        if self.config.editor.auto_pairs {
            let (rope, cursor) = {
                let buffer = self.buffer();
                (&buffer.rope, buffer.cursor)
            };
            match pairs::on_insert(rope, cursor, c) {
                Insertion::StepOver => {
                    let buffer = self.buffer_mut();
                    buffer.cursor.col += 1;
                    buffer.desired_col = buffer.cursor.col;
                    self.refresh_completion();
                    return;
                }
                Insertion::Surround(close) => {
                    self.dot.changed = true;
                    let at = self.buffer().cursor_char();
                    let buffer = self.buffer_mut();
                    buffer.begin();
                    buffer.insert(at, &format!("{c}{close}"));
                    buffer.end();
                    // Between the two, which is the point.
                    buffer.cursor.col += 1;
                    buffer.desired_col = buffer.cursor.col;
                    self.refresh_completion();
                    return;
                }
                Insertion::Plain => {}
            }
        }

        self.dot.changed = true;
        let at = self.buffer().cursor_char();
        let buffer = self.buffer_mut();
        buffer.insert(at, &c.to_string());
        buffer.cursor.col += 1;
        buffer.desired_col = buffer.cursor.col;
        self.refresh_completion();
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
        self.completion = None;
        self.dot.changed = true;
        let line = self.buffer().cursor.line;
        let indent = self.indent_of(line);

        // Enter between a bracket and its partner opens the block out, which
        // is the half of auto-pairs people actually notice.
        if self.config.editor.auto_pairs
            && crate::edit::pairs::splits_block(&self.buffer().rope, self.buffer().cursor)
        {
            let step = if self.config.editor.expand_tab {
                " ".repeat(self.config.editor.shift_width)
            } else {
                "\t".to_string()
            };
            let inner = format!("{indent}{step}");
            let at = self.buffer().cursor_char();
            let buffer = self.buffer_mut();
            buffer.begin();
            buffer.insert(at, &format!("\n{inner}\n{indent}"));
            buffer.end();
            buffer.cursor = Position::new(line + 1, inner.chars().count());
            buffer.desired_col = buffer.cursor.col;
            return;
        }
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

        // An empty pair goes as a unit, so undoing a bracket you did not want
        // is one keystroke rather than two.
        if self.config.editor.auto_pairs
            && crate::edit::pairs::deletes_pair(&self.buffer().rope, cursor)
        {
            self.dot.changed = true;
            let at = self.buffer().cursor_char();
            let buffer = self.buffer_mut();
            buffer.begin();
            buffer.remove(at - 1, at + 1);
            buffer.end();
            buffer.cursor.col -= 1;
            buffer.desired_col = buffer.cursor.col;
            self.completion = None;
            return;
        }
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
        self.refresh_completion();
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
        let start = crate::edit::motion::prev_word_start(&self.buffer().rope, at, false);
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

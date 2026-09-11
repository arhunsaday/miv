//! The `:` command line and the `/` search prompt.

use crate::app::{App, Overlay, Prompt, PromptKind, Trigger};
use crate::core::search::Direction;
use crate::core::text::{self, Position};
use crate::mode::Mode;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use regex::Regex;
use std::path::PathBuf;

pub fn open_prompt(app: &mut App, kind: PromptKind, initial: &str) {
    let buffer = app.buffer();
    app.prompt = Some(Prompt {
        kind,
        input: initial.to_string(),
        cursor: initial.chars().count(),
        origin: buffer.cursor,
        origin_view: buffer.view_top,
        history_index: None,
    });
    app.mode = match kind {
        PromptKind::Command => Mode::Command,
        _ => Mode::Search,
    };
    app.message = None;
}

pub fn prompt_insert(app: &mut App, text: &str) {
    if let Some(prompt) = app.prompt.as_mut() {
        let byte = char_to_byte(&prompt.input, prompt.cursor);
        prompt.input.insert_str(byte, text);
        prompt.cursor += text.chars().count();
    }
    update_search_preview(app);
}

fn char_to_byte(s: &str, index: usize) -> usize {
    s.char_indices()
        .nth(index)
        .map(|(b, _)| b)
        .unwrap_or(s.len())
}

pub fn prompt_key(app: &mut App, key: KeyEvent) {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let Some(prompt) = app.prompt.as_mut() else {
        app.mode = Mode::Normal;
        return;
    };

    if ctrl {
        match key.code {
            KeyCode::Char('u') => {
                prompt.input.clear();
                prompt.cursor = 0;
                update_search_preview(app);
                return;
            }
            KeyCode::Char('w') => {
                let upto = char_to_byte(&prompt.input, prompt.cursor);
                let head = &prompt.input[..upto];
                let trimmed = head.trim_end();
                let cut = trimmed
                    .rfind(char::is_whitespace)
                    .map(|i| i + 1)
                    .unwrap_or(0);
                let removed = head[cut..].chars().count();
                prompt.input.replace_range(cut..upto, "");
                prompt.cursor -= removed;
                update_search_preview(app);
                return;
            }
            KeyCode::Char('c') => {
                cancel_prompt(app);
                return;
            }
            // Ctrl-J and Ctrl-M are LF and CR, so they submit.
            KeyCode::Char('j') | KeyCode::Char('m') => {
                submit_prompt(app);
                return;
            }
            // Everything else control-modified is not text: typing Ctrl-K into
            // the command line used to leave a literal `k` behind.
            KeyCode::Char(_) => return,
            _ => {}
        }
    }

    match key.code {
        KeyCode::Esc => cancel_prompt(app),
        KeyCode::Enter => submit_prompt(app),
        KeyCode::Char(c) => {
            let byte = char_to_byte(&prompt.input, prompt.cursor);
            prompt.input.insert(byte, c);
            prompt.cursor += 1;
            update_search_preview(app);
        }
        KeyCode::Backspace => {
            if prompt.cursor == 0 {
                // Backspacing past the sigil leaves the prompt, as in Vim.
                if prompt.input.is_empty() {
                    cancel_prompt(app);
                }
                return;
            }
            let byte = char_to_byte(&prompt.input, prompt.cursor - 1);
            prompt.input.remove(byte);
            prompt.cursor -= 1;
            update_search_preview(app);
        }
        KeyCode::Delete => {
            let byte = char_to_byte(&prompt.input, prompt.cursor);
            if byte < prompt.input.len() {
                prompt.input.remove(byte);
                update_search_preview(app);
            }
        }
        KeyCode::Left => prompt.cursor = prompt.cursor.saturating_sub(1),
        KeyCode::Right => prompt.cursor = (prompt.cursor + 1).min(prompt.input.chars().count()),
        KeyCode::Home => prompt.cursor = 0,
        KeyCode::End => prompt.cursor = prompt.input.chars().count(),
        KeyCode::Up => recall_history(app, true),
        KeyCode::Down => recall_history(app, false),
        _ => {}
    }
}

fn history_for(app: &App, kind: PromptKind) -> &Vec<String> {
    match kind {
        PromptKind::Command => &app.command_history,
        _ => &app.search_history,
    }
}

fn recall_history(app: &mut App, older: bool) {
    let kind = match app.prompt.as_ref() {
        Some(prompt) => prompt.kind,
        None => return,
    };
    let len = history_for(app, kind).len();
    if len == 0 {
        return;
    }
    let prompt = app.prompt.as_mut().unwrap();
    let index = match (prompt.history_index, older) {
        (None, true) => Some(len - 1),
        (None, false) => None,
        (Some(i), true) => Some(i.saturating_sub(1)),
        (Some(i), false) => {
            if i + 1 >= len {
                None
            } else {
                Some(i + 1)
            }
        }
    };
    prompt.history_index = index;
    let entry = match index {
        Some(i) => history_for(app, kind)[i].clone(),
        None => String::new(),
    };
    let prompt = app.prompt.as_mut().unwrap();
    prompt.cursor = entry.chars().count();
    prompt.input = entry;
    update_search_preview(app);
}

/// Jump to the first match as the pattern is typed, so search shows its work.
fn update_search_preview(app: &mut App) {
    let Some(prompt) = app.prompt.as_ref() else {
        return;
    };
    let (kind, pattern, origin) = (prompt.kind, prompt.input.clone(), prompt.origin);
    if kind == PromptKind::Command {
        return;
    }
    if pattern.is_empty() {
        let view = app.prompt.as_ref().map(|p| p.origin_view).unwrap_or(0);
        app.buffer_mut().cursor = origin;
        app.buffer_mut().view_top = view;
        return;
    }
    let direction = if kind == PromptKind::SearchForward {
        Direction::Forward
    } else {
        Direction::Backward
    };
    let (ignore_case, smart_case, wrap) = (
        app.config.editor.ignore_case,
        app.config.editor.smart_case,
        app.config.editor.wrap_search,
    );
    if app
        .search
        .set_pattern(&pattern, direction, ignore_case, smart_case)
        .is_err()
    {
        return;
    }
    app.search_highlight = true;
    if let Some(found) = app.search.find(&app.buffer().rope, origin, direction, wrap) {
        app.buffer_mut().cursor = found.position();
        app.scroll_to_cursor();
    }
}

fn cancel_prompt(app: &mut App) {
    if let Some(prompt) = app.prompt.take() {
        if prompt.kind != PromptKind::Command {
            let buffer = app.buffer_mut();
            buffer.cursor = prompt.origin;
            buffer.view_top = prompt.origin_view;
            app.search.clear();
        }
    }
    app.mode = Mode::Normal;
    app.clamp_cursor();
}

fn submit_prompt(app: &mut App) {
    let Some(prompt) = app.prompt.take() else {
        app.mode = Mode::Normal;
        return;
    };
    app.mode = Mode::Normal;
    let input = prompt.input.trim().to_string();

    match prompt.kind {
        PromptKind::Command => {
            if !input.is_empty() {
                app.command_history.push(input.clone());
                execute(app, &input);
            }
        }
        PromptKind::SearchForward | PromptKind::SearchBackward => {
            if input.is_empty() {
                // A bare `/` repeats the previous pattern.
                let direction = if prompt.kind == PromptKind::SearchForward {
                    Direction::Forward
                } else {
                    Direction::Backward
                };
                app.search.direction = direction;
            } else {
                app.search_history.push(input.clone());
                let (ignore_case, smart_case) =
                    (app.config.editor.ignore_case, app.config.editor.smart_case);
                let direction = if prompt.kind == PromptKind::SearchForward {
                    Direction::Forward
                } else {
                    Direction::Backward
                };
                if let Err(e) = app
                    .search
                    .set_pattern(&input, direction, ignore_case, smart_case)
                {
                    app.buffer_mut().cursor = prompt.origin;
                    app.set_error(format!("invalid pattern: {e}"));
                    return;
                }
                app.search_highlight = true;
            }
            let wrap = app.config.editor.wrap_search;
            let origin = prompt.origin;
            let direction = app.search.direction;
            match app.search.find(&app.buffer().rope, origin, direction, wrap) {
                Some(found) => {
                    app.buffer_mut().cursor = found.position();
                    app.clamp_cursor();
                }
                None => {
                    app.buffer_mut().cursor = origin;
                    let pattern = app.search.pattern.clone();
                    app.set_error(format!("E486: Pattern not found: {pattern}"));
                }
            }
        }
    }
    app.clamp_cursor();
    app.scroll_to_cursor();
}

/// One entry in the command palette.
///
/// This table is also the seam a plugin system would extend: everything the
/// palette can run, it runs by name through [`execute`].
pub struct CommandSpec {
    pub name: &'static str,
    pub description: &'static str,
    /// The ex command to run. When it ends in a space, the palette opens the
    /// command line prefilled instead of running it, because it needs an
    /// argument.
    pub run: &'static str,
}

pub const COMMANDS: &[CommandSpec] = &[
    CommandSpec {
        name: "Write file",
        description: "save the current buffer",
        run: "w",
    },
    CommandSpec {
        name: "Write as…",
        description: "save under a new name",
        run: "w ",
    },
    CommandSpec {
        name: "Quit",
        description: "close this window, or the editor",
        run: "q",
    },
    CommandSpec {
        name: "Quit without saving",
        description: "discard changes",
        run: "q!",
    },
    CommandSpec {
        name: "Write and quit",
        description: "save, then close",
        run: "wq",
    },
    CommandSpec {
        name: "Write all",
        description: "save every modified buffer",
        run: "wa",
    },
    CommandSpec {
        name: "Open file…",
        description: "edit a path",
        run: "e ",
    },
    CommandSpec {
        name: "Reload from disk",
        description: "discard and re-read",
        run: "e!",
    },
    CommandSpec {
        name: "Find files",
        description: "fuzzy-find in the project",
        run: "files",
    },
    CommandSpec {
        name: "Search project…",
        description: "grep across the project",
        run: "grep ",
    },
    CommandSpec {
        name: "Switch buffer",
        description: "pick from the open buffers",
        run: "buffers",
    },
    CommandSpec {
        name: "Close buffer",
        description: "remove it from the list",
        run: "bd",
    },
    CommandSpec {
        name: "Split side by side",
        description: "a second view, vertically",
        run: "vsplit",
    },
    CommandSpec {
        name: "Split stacked",
        description: "a second view, horizontally",
        run: "split",
    },
    CommandSpec {
        name: "Close window",
        description: "leave the others open",
        run: "close",
    },
    CommandSpec {
        name: "Only this window",
        description: "close every other window",
        run: "only",
    },
    CommandSpec {
        name: "Toggle file explorer",
        description: "show or hide the sidebar",
        run: "explorer",
    },
    CommandSpec {
        name: "Format buffer",
        description: "run the configured formatter",
        run: "fmt",
    },
    CommandSpec {
        name: "Run checkers",
        description: "refresh diagnostics now",
        run: "check",
    },
    CommandSpec {
        name: "List diagnostics",
        description: "every problem in this buffer",
        run: "diag",
    },
    CommandSpec {
        name: "Clear search highlight",
        description: "stop highlighting matches",
        run: "noh",
    },
    CommandSpec {
        name: "Substitute…",
        description: "search and replace",
        run: "%s/",
    },
    CommandSpec {
        name: "Share this session",
        description: "let others join",
        run: "share",
    },
    CommandSpec {
        name: "Session participants",
        description: "who is connected",
        run: "who",
    },
    CommandSpec {
        name: "End session",
        description: "stop sharing",
        run: "unshare",
    },
    CommandSpec {
        name: "Set option…",
        description: "change a setting",
        run: "set ",
    },
    CommandSpec {
        name: "Registers",
        description: "what is in the registers",
        run: "reg",
    },
    CommandSpec {
        name: "Marks",
        description: "where the marks are",
        run: "marks",
    },
    CommandSpec {
        name: "Help",
        description: "the key reference",
        run: "help",
    },
];

// -- ex commands ------------------------------------------------------------

struct Parsed {
    range: Option<(usize, usize)>,
    name: String,
    bang: bool,
    args: String,
}

pub fn execute(app: &mut App, input: &str) {
    let Some(parsed) = parse(app, input) else {
        app.set_error(format!("E492: Not an editor command: {input}"));
        return;
    };

    // A bare range is a jump: `:42`.
    if parsed.name.is_empty() {
        if let Some((_, last)) = parsed.range {
            let line = last.min(app.buffer().line_count().saturating_sub(1));
            let col = text::first_non_blank(&app.buffer().rope, line);
            crate::keymap::push_jump(app);
            app.set_cursor(Position::new(line, col));
        }
        return;
    }

    let args = parsed.args.trim().to_string();
    match parsed.name.as_str() {
        "w" | "write" => {
            write_file(app, &args, false);
        }
        "wq" | "x" | "xit" => {
            if write_file(app, &args, parsed.bang) {
                app.should_quit = true;
            }
        }
        "wa" | "wall" => write_all(app),
        "wqa" | "xa" | "xall" => {
            write_all(app);
            app.should_quit = true;
        }
        "q" | "quit" | "qa" | "qall" | "quitall" => quit(app, parsed.bang),
        "e" | "edit" => edit_file(app, &args, parsed.bang),
        "bn" | "bnext" => app.cycle_buffer(true),
        "bp" | "bprev" | "bprevious" => app.cycle_buffer(false),
        "bd" | "bdelete" => app.close_buffer(parsed.bang),
        "b" | "buffer" => switch_buffer(app, &args),
        "ls" | "buffers" => app.open_buffer_picker(),
        "files" | "find" => app.open_file_picker(),
        "commands" | "palette" => app.open_command_palette(),
        "grep" | "rg" | "search" => {
            if args.is_empty() {
                app.set_error("usage: :grep <pattern>");
            } else {
                app.open_grep_picker(&args);
            }
        }
        "reg" | "registers" | "di" | "display" => show_registers(app),
        "marks" => show_marks(app),
        "noh" | "nohl" | "nohlsearch" => app.search_highlight = false,
        "s" | "substitute" => substitute(app, parsed.range, &parsed.args),
        "set" | "se" => set_option(app, &args),
        "share" => crate::session::commands::share(app, &parsed.args),
        "unshare" => crate::session::commands::unshare(app),
        "who" | "participants" => crate::session::commands::who(app),
        "grant" => crate::session::commands::set_access(
            app,
            &args,
            crate::session::protocol::Access::Write,
        ),
        "revoke" => {
            crate::session::commands::set_access(app, &args, crate::session::protocol::Access::Read)
        }
        "follow" => crate::session::commands::follow(app, &args),
        "unfollow" => crate::session::commands::follow(app, ""),
        "say" => crate::session::commands::say(app, parsed.args.trim()),
        "sp" | "split" | "new" => {
            app.split_window(false);
            if !args.is_empty() {
                if let Err(e) = app.open_file(&PathBuf::from(&args)) {
                    app.set_error(format!("{e}"));
                }
            }
        }
        "vs" | "vsp" | "vsplit" | "vnew" => {
            app.split_window(true);
            if !args.is_empty() {
                if let Err(e) = app.open_file(&PathBuf::from(&args)) {
                    app.set_error(format!("{e}"));
                }
            }
        }
        "clo" | "close" => {
            app.close_window();
        }
        "on" | "only" => app.only_window(),
        "explorer" | "tree" | "sidebar" => app.toggle_sidebar(),
        "fmt" | "format" => format_buffer(app),
        "check" => {
            let index = app.current;
            app.refresh_buffer(index, Trigger::Manual);
            app.set_message("running checkers…");
        }
        "diag" | "diagnostics" => app.open_diagnostics_picker(),
        "h" | "help" => show_help(app),
        other => app.set_error(format!("E492: Not an editor command: {other}")),
    }
}

fn parse(app: &App, input: &str) -> Option<Parsed> {
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;

    let mut addresses: Vec<usize> = Vec::new();
    let last_line = app.buffer().line_count().saturating_sub(1);

    if chars.first() == Some(&'%') {
        addresses.push(0);
        addresses.push(last_line);
        i = 1;
    } else {
        loop {
            let (address, consumed) = parse_address(app, &chars[i..])?;
            if let Some(address) = address {
                addresses.push(address);
                i += consumed;
                if chars.get(i) == Some(&',') {
                    i += 1;
                    continue;
                }
            }
            break;
        }
    }

    let range = match addresses.len() {
        0 => None,
        1 => Some((addresses[0], addresses[0])),
        _ => {
            let (a, b) = (addresses[0], addresses[addresses.len() - 1]);
            Some((a.min(b), a.max(b)))
        }
    };

    let name_start = i;
    while i < chars.len() && (chars[i].is_ascii_alphabetic() || chars[i] == '&') {
        i += 1;
    }
    let mut name: String = chars[name_start..i].iter().collect();

    // `:s` takes a delimiter immediately after the name.
    if name == "s" || name == "substitute" {
        let args: String = chars[i..].iter().collect();
        return Some(Parsed {
            range,
            name: "s".to_string(),
            bang: false,
            args,
        });
    }

    let bang = chars.get(i) == Some(&'!');
    if bang {
        i += 1;
    }
    if name.is_empty() && bang {
        name = "!".to_string();
    }
    let args: String = chars[i..].iter().collect();
    Some(Parsed {
        range,
        name,
        bang,
        args,
    })
}

/// Parse one line address, returning it plus how many characters it used.
fn parse_address(app: &App, chars: &[char]) -> Option<(Option<usize>, usize)> {
    let last_line = app.buffer().line_count().saturating_sub(1);
    let cursor_line = app.buffer().cursor.line;
    let mut i = 0;

    let mut base = match chars.first() {
        Some('.') => {
            i = 1;
            Some(cursor_line)
        }
        Some('$') => {
            i = 1;
            Some(last_line)
        }
        Some('\'') => {
            let mark = *chars.get(1)?;
            i = 2;
            if mark == '<' || mark == '>' {
                let (first, last) = app.visual_lines();
                Some(if mark == '<' { first } else { last })
            } else {
                Some(app.buffer().marks.get(&mark)?.line)
            }
        }
        Some(c) if c.is_ascii_digit() => {
            while i < chars.len() && chars[i].is_ascii_digit() {
                i += 1;
            }
            let number: usize = chars[..i].iter().collect::<String>().parse().ok()?;
            Some(number.saturating_sub(1))
        }
        Some('+') | Some('-') => Some(cursor_line),
        _ => None,
    };

    // Trailing `+N` / `-N` offsets.
    while let Some(&sign) = chars.get(i) {
        if sign != '+' && sign != '-' {
            break;
        }
        i += 1;
        let digit_start = i;
        while i < chars.len() && chars[i].is_ascii_digit() {
            i += 1;
        }
        let amount: usize = if digit_start == i {
            1
        } else {
            chars[digit_start..i]
                .iter()
                .collect::<String>()
                .parse()
                .ok()?
        };
        let current = base.unwrap_or(cursor_line);
        base = Some(if sign == '+' {
            (current + amount).min(last_line)
        } else {
            current.saturating_sub(amount)
        });
    }

    Some((base, i))
}

fn format_buffer(app: &mut App) {
    match crate::tools::format::format_buffer(app) {
        Ok(Some(formatted)) => app.set_message(format!(
            "{}: {} line(s) changed",
            formatted.tool, formatted.changed_lines
        )),
        Ok(None) => app.set_message("already formatted"),
        Err(e) => app.set_error(e),
    }
}

fn write_file(app: &mut App, args: &str, _force: bool) -> bool {
    let target: Option<PathBuf> = if args.is_empty() {
        app.buffer().path.clone()
    } else {
        Some(PathBuf::from(args))
    };
    let Some(path) = target else {
        app.set_error("E32: No file name");
        return false;
    };
    if app.buffer().path.is_none() {
        app.buffer_mut().path = Some(path.clone());
        let index = app.current;
        app.detect_syntax(index);
    }

    // Tidy-ups run before the write, so what lands on disk is what the
    // configuration asked for. A formatter that fails is reported but does not
    // stop the save: refusing to write because a tool is unhappy would be a
    // good way to lose work.
    if app.config.format.on_save {
        if let Err(e) = crate::tools::format::format_buffer(app) {
            app.set_error(e);
        }
    }
    if app.config.editor.trim_trailing_whitespace {
        app.trim_trailing_whitespace();
    }
    if app.config.editor.ensure_final_newline {
        app.buffer_mut().final_newline = true;
    }

    match app.buffer_mut().write(&path) {
        Ok((bytes, lines)) => {
            app.set_message(format!("\"{}\" {lines}L, {bytes}B written", path.display()));
            let index = app.current;
            app.refresh_buffer(index, Trigger::Save);
            true
        }
        Err(e) => {
            app.set_error(format!("E212: Can't open file for writing: {e}"));
            false
        }
    }
}

fn write_all(app: &mut App) {
    let mut written = 0;
    let mut errors = Vec::new();
    for index in 0..app.buffers.len() {
        if !app.buffers[index].is_modified() {
            continue;
        }
        let Some(path) = app.buffers[index].path.clone() else {
            errors.push("E32: No file name".to_string());
            continue;
        };
        match app.buffers[index].write(&path) {
            Ok(_) => written += 1,
            Err(e) => errors.push(format!("{e}")),
        }
    }
    if errors.is_empty() {
        app.set_message(format!("{written} file(s) written"));
    } else {
        app.set_error(errors.join("; "));
    }
}

/// `:q` and `:qa`. miv has one window, so quitting means leaving the editor;
/// closing a single buffer is `:bd`. Any modified buffer blocks the exit,
/// because with hidden buffers there would otherwise be no warning before
/// losing them.
fn quit(app: &mut App, force: bool) {
    // With the screen split, `:q` closes the window you are in — the editor
    // itself only closes once the last one goes, which is what Vim does.
    if app.workspace.count() > 1 {
        app.close_window();
        return;
    }
    if !force {
        let modified: Vec<String> = app
            .modified_buffers()
            .iter()
            .map(|buffer| buffer.short_name())
            .collect();
        if !modified.is_empty() {
            app.set_error(format!(
                "E37: No write since last change: {} (add ! to override)",
                modified.join(", ")
            ));
            return;
        }
    }
    app.should_quit = true;
}

fn edit_file(app: &mut App, args: &str, force: bool) {
    if args.is_empty() {
        // `:e!` reloads the current file from disk.
        let Some(path) = app.buffer().path.clone() else {
            app.set_error("E32: No file name");
            return;
        };
        if app.buffer().is_modified() && !force {
            app.set_error("E37: No write since last change (add ! to override)");
            return;
        }
        let index = app.current;
        let id = app.buffers[index].id;
        match crate::core::buffer::Buffer::open(id, &path) {
            Ok(buffer) => {
                app.buffers[index] = buffer;
                app.detect_syntax(index);
                app.set_message(format!("\"{}\" reloaded", path.display()));
            }
            Err(e) => app.set_error(format!("{e}")),
        }
        return;
    }
    if let Err(e) = app.open_file(&PathBuf::from(args)) {
        app.set_error(format!("{e}"));
    }
}

fn switch_buffer(app: &mut App, args: &str) {
    if let Ok(number) = args.parse::<usize>() {
        if let Some(index) = app.buffers.iter().position(|b| b.id == number) {
            app.switch_to(index);
            return;
        }
        app.set_error(format!("E86: Buffer {number} does not exist"));
        return;
    }
    if args.is_empty() {
        app.set_error("E86: Buffer name required");
        return;
    }
    let matches: Vec<usize> = app
        .buffers
        .iter()
        .enumerate()
        .filter(|(_, b)| b.display_name().contains(args))
        .map(|(i, _)| i)
        .collect();
    match matches.len() {
        0 => app.set_error(format!("E94: No matching buffer for {args}")),
        1 => app.switch_to(matches[0]),
        _ => app.set_error(format!("E93: More than one match for {args}")),
    }
}

fn show_registers(app: &mut App) {
    let lines: Vec<String> = app
        .registers
        .listing()
        .iter()
        .map(|(name, content)| {
            let preview: String = content
                .text
                .chars()
                .take(60)
                .map(|c| if c == '\n' { '⏎' } else { c })
                .collect();
            format!("\"{name}   {preview}")
        })
        .collect();
    app.overlay = Some(Overlay {
        title: "Registers".to_string(),
        lines,
        scroll: 0,
    });
}

fn show_marks(app: &mut App) {
    let mut marks: Vec<(char, Position)> =
        app.buffer().marks.iter().map(|(c, p)| (*c, *p)).collect();
    marks.sort_by_key(|(c, _)| *c);
    let lines: Vec<String> = marks
        .iter()
        .map(|(name, position)| {
            let content: String = text::line(&app.buffer().rope, position.line)
                .chars()
                .take(60)
                .collect();
            format!(
                " {name}   {:>5} {:>4}  {}",
                position.line + 1,
                position.col,
                content.trim_start()
            )
        })
        .collect();
    app.overlay = Some(Overlay {
        title: "Marks".to_string(),
        lines,
        scroll: 0,
    });
}

/// `:[range]s/pattern/replacement/flags`
fn substitute(app: &mut App, range: Option<(usize, usize)>, args: &str) {
    let mut chars = args.chars();
    let Some(delimiter) = chars.next() else {
        app.set_error("E33: No previous substitute regular expression");
        return;
    };
    if delimiter.is_alphanumeric() || delimiter == '\\' || delimiter == '"' {
        app.set_error(format!("invalid separator: {delimiter}"));
        return;
    }

    // Split on unescaped delimiters.
    let body: String = chars.collect();
    let mut parts: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut escaped = false;
    for c in body.chars() {
        if escaped {
            if c != delimiter {
                current.push('\\');
            }
            current.push(c);
            escaped = false;
        } else if c == '\\' {
            escaped = true;
        } else if c == delimiter {
            parts.push(std::mem::take(&mut current));
        } else {
            current.push(c);
        }
    }
    parts.push(current);

    let pattern = parts.first().cloned().unwrap_or_default();
    let replacement = parts.get(1).cloned().unwrap_or_default();
    let flags = parts.get(2).cloned().unwrap_or_default();

    if pattern.is_empty() {
        app.set_error("E35: No previous regular expression");
        return;
    }
    let global = flags.contains('g');
    let case_insensitive = flags.contains('i')
        || (app.config.editor.ignore_case
            && !(app.config.editor.smart_case && pattern.chars().any(char::is_uppercase)))
            && !flags.contains('I');

    let regex = match regex::RegexBuilder::new(&pattern)
        .case_insensitive(case_insensitive)
        .build()
    {
        Ok(regex) => regex,
        Err(e) => {
            app.set_error(format!("E486: invalid pattern: {e}"));
            return;
        }
    };

    // Vim habits: `\1` refers to a group. Rust wants `${1}`.
    let replacement = translate_replacement(&replacement);

    let (first, last) = range.unwrap_or_else(|| {
        let line = app.buffer().cursor.line;
        (line, line)
    });
    let last = last.min(app.buffer().line_count().saturating_sub(1));

    let mut substitutions = 0usize;
    let mut lines_changed = 0usize;
    let mut last_line_changed = first;

    app.buffer_mut().begin();
    // Work bottom-up so earlier edits cannot shift later line indices.
    for line in (first..=last).rev() {
        let content: String = text::line(&app.buffer().rope, line).chars().collect();
        let count_here = count_matches(&regex, &content, global);
        if count_here == 0 {
            continue;
        }
        let replaced = if global {
            regex
                .replace_all(&content, replacement.as_str())
                .into_owned()
        } else {
            regex.replace(&content, replacement.as_str()).into_owned()
        };
        if replaced == content {
            continue;
        }
        let start = app.buffer().rope.line_to_char(line);
        let end = start + content.chars().count();
        app.buffer_mut().replace(start, end, &replaced);
        substitutions += count_here;
        lines_changed += 1;
        last_line_changed = line;
    }
    app.buffer_mut().end();

    if substitutions == 0 {
        app.set_error(format!("E486: Pattern not found: {pattern}"));
        return;
    }
    let line = last_line_changed.min(app.buffer().line_count().saturating_sub(1));
    let col = text::first_non_blank(&app.buffer().rope, line);
    app.set_cursor(Position::new(line, col));
    app.set_message(format!(
        "{substitutions} substitution(s) on {lines_changed} line(s)"
    ));
}

fn count_matches(regex: &Regex, content: &str, global: bool) -> usize {
    let mut count = 0;
    for m in regex.find_iter(content) {
        if m.start() == m.end() {
            continue;
        }
        count += 1;
        if !global {
            break;
        }
    }
    count
}

fn translate_replacement(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.peek() {
                Some(d) if d.is_ascii_digit() => {
                    out.push_str(&format!("${{{d}}}"));
                    chars.next();
                }
                Some('n') => {
                    out.push('\n');
                    chars.next();
                }
                Some('t') => {
                    out.push('\t');
                    chars.next();
                }
                Some(&other) => {
                    out.push(other);
                    chars.next();
                }
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// `:set option=value`, `:set option`, `:set nooption`, `:set option?`
fn set_option(app: &mut App, args: &str) {
    if args.is_empty() {
        app.set_error("E518: Unknown option");
        return;
    }
    for token in args.split_whitespace() {
        if let Err(message) = set_single_option(app, token) {
            app.set_error(message);
            return;
        }
    }
}

fn set_single_option(app: &mut App, token: &str) -> Result<(), String> {
    use crate::config::LineNumbers;

    if let Some(query) = token.strip_suffix('?') {
        let value = match canonical(query) {
            "number" => format!("{:?}", app.config.editor.line_numbers),
            "tabstop" => app.config.editor.tab_width.to_string(),
            "shiftwidth" => app.config.editor.shift_width.to_string(),
            "expandtab" => app.config.editor.expand_tab.to_string(),
            "scrolloff" => app.config.editor.scrolloff.to_string(),
            "cursorline" => app.config.editor.cursorline.to_string(),
            "ignorecase" => app.config.editor.ignore_case.to_string(),
            "smartcase" => app.config.editor.smart_case.to_string(),
            "wrapscan" => app.config.editor.wrap_search.to_string(),
            "diagnostics" => app.config.diagnostics.enabled.to_string(),
            "virtualtext" => app.config.diagnostics.virtual_text.to_string(),
            "signs" => app.config.signs.enabled.to_string(),
            "gitsigns" => app.config.signs.git.to_string(),
            "formatonsave" => app.config.format.on_save.to_string(),
            "swatches" => app.config.appearance.color_swatches.to_string(),
            "trimwhitespace" => app.config.editor.trim_trailing_whitespace.to_string(),
            "theme" => app.config.appearance.theme.clone(),
            other => return Err(format!("E518: Unknown option: {other}")),
        };
        app.set_message(format!("{query}={value}"));
        return Ok(());
    }

    if let Some((name, value)) = token.split_once('=') {
        let parse_usize = |v: &str| -> Result<usize, String> {
            v.parse::<usize>()
                .map_err(|_| format!("E521: Number required after =: {name}={v}"))
        };
        match canonical(name) {
            "tabstop" => {
                let value = parse_usize(value)?.max(1);
                app.config.editor.tab_width = value;
            }
            "shiftwidth" => {
                let value = parse_usize(value)?.max(1);
                app.config.editor.shift_width = value;
            }
            "scrolloff" => app.config.editor.scrolloff = parse_usize(value)?,
            "number" => {
                app.config.editor.line_numbers = match value {
                    "none" => LineNumbers::None,
                    "absolute" => LineNumbers::Absolute,
                    "relative" => LineNumbers::Relative,
                    "hybrid" => LineNumbers::Hybrid,
                    other => {
                        return Err(format!(
                            "expected none|absolute|relative|hybrid, got {other}"
                        ))
                    }
                }
            }
            "theme" => {
                let Some(theme) = app.engine.theme(value).cloned() else {
                    return Err(format!("E185: Cannot find theme {value}"));
                };
                app.theme = theme;
                app.config.appearance.theme = value.to_string();
            }
            other => return Err(format!("E518: Unknown option: {other}")),
        }
        return Ok(());
    }

    let (name, enable) = match token.strip_prefix("no") {
        Some(rest) if !rest.is_empty() => (canonical(rest), false),
        _ => (canonical(token), true),
    };
    match name {
        "expandtab" => app.config.editor.expand_tab = enable,
        "cursorline" => app.config.editor.cursorline = enable,
        "ignorecase" => app.config.editor.ignore_case = enable,
        "smartcase" => app.config.editor.smart_case = enable,
        "wrapscan" => app.config.editor.wrap_search = enable,
        "number" => {
            app.config.editor.line_numbers = if enable {
                LineNumbers::Absolute
            } else {
                LineNumbers::None
            }
        }
        "relativenumber" => {
            app.config.editor.line_numbers = if enable {
                LineNumbers::Relative
            } else {
                LineNumbers::None
            }
        }
        "hybridnumber" => {
            app.config.editor.line_numbers = if enable {
                LineNumbers::Hybrid
            } else {
                LineNumbers::None
            }
        }
        "syntax" => {
            app.config.appearance.syntax_highlighting = enable;
            for index in 0..app.buffers.len() {
                app.detect_syntax(index);
            }
        }
        "diagnostics" => {
            app.config.diagnostics.enabled = enable;
            if enable {
                let index = app.current;
                app.refresh_buffer(index, Trigger::Manual);
            } else {
                for buffer in &mut app.buffers {
                    buffer.diagnostics.clear();
                }
            }
        }
        "virtualtext" => app.config.diagnostics.virtual_text = enable,
        "signs" => app.config.signs.enabled = enable,
        "gitsigns" => app.config.signs.git = enable,
        "formatonsave" => app.config.format.on_save = enable,
        "swatches" => app.config.appearance.color_swatches = enable,
        "trimwhitespace" => app.config.editor.trim_trailing_whitespace = enable,
        other => return Err(format!("E518: Unknown option: {other}")),
    }
    Ok(())
}

fn canonical(name: &str) -> &str {
    match name {
        "ts" => "tabstop",
        "sw" => "shiftwidth",
        "et" => "expandtab",
        "so" => "scrolloff",
        "nu" => "number",
        "rnu" => "relativenumber",
        "cul" => "cursorline",
        "ic" => "ignorecase",
        "scs" => "smartcase",
        "ws" => "wrapscan",
        "diag" => "diagnostics",
        "vt" => "virtualtext",
        "fos" => "formatonsave",
        "gs" => "gitsigns",
        other => other,
    }
}

fn show_help(app: &mut App) {
    let lines = crate::help::TOPICS
        .iter()
        .map(|(keys, description)| format!("  {keys:<14} {description}"))
        .collect();
    app.overlay = Some(Overlay {
        title: format!(
            "miv {} — press any key to close, j/k to scroll",
            env!("CARGO_PKG_VERSION")
        ),
        lines,
        scroll: 0,
    });
}

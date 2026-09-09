//! Key handling.
//!
//! Normal mode is a small state machine rather than a flat match, because
//! Vim's grammar is compositional: `["x]{count}{operator}{count}{motion}`.
//! Keys accumulate into [`Pending`] until they form a complete command, which
//! is what makes `3dw`, `d2f)` and `"ay}` fall out of one code path instead of
//! needing a case each.

use crate::app::{App, PromptKind};
use crate::command;
use crate::keys;
use crate::mode::{Mode, VisualKind};
use crate::motion::{self, FindTarget, Motion, MotionKind};
use crate::operator::{self, Operator, Outcome};
use crate::register::RegisterContent;
use crate::search::Direction;
use crate::text::{self, Position};
use crate::textobject::{Scope, TextObject};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// A key the state machine is waiting on before it can act.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Awaiting {
    Register,
    FindChar {
        till: bool,
        backward: bool,
    },
    Mark,
    GotoMark {
        linewise: bool,
    },
    ReplaceChar,
    GPrefix,
    ZPrefix,
    /// `ZZ` / `ZQ`.
    ZetPrefix,
    TextObject {
        scope: Scope,
    },
    RecordMacro,
    PlayMacro,
    InsertRegister,
}

#[derive(Clone, Default)]
pub struct Pending {
    pub count: Option<usize>,
    pub register: Option<char>,
    pub operator: Option<Operator>,
    pub operator_count: Option<usize>,
    pub awaiting: Option<Awaiting>,
    /// Keys typed so far, echoed at the right of the status line like Vim's
    /// `showcmd`.
    pub display: String,
}

impl Pending {
    pub fn is_empty(&self) -> bool {
        self.count.is_none()
            && self.register.is_none()
            && self.operator.is_none()
            && self.operator_count.is_none()
            && self.awaiting.is_none()
    }

    pub fn reset(&mut self) {
        *self = Pending::default();
    }

    /// Vim multiplies the counts either side of an operator: `2d3w` is `d6w`.
    pub fn effective_count(&self) -> usize {
        self.count.unwrap_or(1) * self.operator_count.unwrap_or(1)
    }

    pub fn had_count(&self) -> bool {
        self.count.is_some() || self.operator_count.is_some()
    }
}

pub fn handle(app: &mut App, key: KeyEvent) {
    // A terminal encodes Alt+X as Esc followed by X, and delivers both in one
    // read. miv binds no Alt keys, so split it back into the two keystrokes it
    // stands for: without this, `Esc` typed quickly before another key is
    // swallowed and the following character is treated as literal input.
    if key.modifiers.contains(KeyModifiers::ALT) {
        let mut stripped = key;
        stripped.modifiers.remove(KeyModifiers::ALT);
        handle(app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        handle(app, stripped);
        return;
    }

    // Any key dismisses an overlay and does nothing else.
    if app.overlay.is_some() {
        if key.code == KeyCode::Down || key.code == KeyCode::Char('j') {
            if let Some(overlay) = app.overlay.as_mut() {
                overlay.scroll = overlay.scroll.saturating_add(1);
            }
            return;
        }
        if key.code == KeyCode::Up || key.code == KeyCode::Char('k') {
            if let Some(overlay) = app.overlay.as_mut() {
                overlay.scroll = overlay.scroll.saturating_sub(1);
            }
            return;
        }
        app.overlay = None;
        return;
    }

    let stops_recording = app.recording.is_some()
        && app.mode == Mode::Normal
        && app.pending.is_empty()
        && key.code == KeyCode::Char('q');
    if let Some(recording) = app.recording.as_mut() {
        if !stops_recording {
            recording.keys.push(key);
        }
    }

    if app.mode == Mode::Normal && app.pending.is_empty() {
        app.dot.recording.clear();
    }
    if !app.mode.is_prompt() {
        app.dot.recording.push(key);
    }

    match app.mode {
        Mode::Insert => insert_mode(app, key),
        Mode::Replace => replace_mode(app, key),
        Mode::Command | Mode::Search => command::prompt_key(app, key),
        Mode::Normal | Mode::Visual(_) => normal_mode(app, key),
    }

    // A command is complete once nothing is pending and we are back in normal
    // mode; only then is it worth remembering for `.`.
    if app.mode == Mode::Normal && app.pending.is_empty() {
        if app.dot.changed {
            app.dot.last = std::mem::take(&mut app.dot.recording);
        } else {
            app.dot.recording.clear();
        }
        app.dot.changed = false;
    }

    if !app.mode.is_prompt() {
        app.scroll_to_cursor();
    }
}

fn normal_mode(app: &mut App, key: KeyEvent) {
    if let Some(awaiting) = app.pending.awaiting {
        app.pending.awaiting = None;
        handle_awaiting(app, awaiting, key);
        return;
    }

    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let count = app.pending.effective_count();

    if ctrl {
        if let KeyCode::Char(c) = key.code {
            control_key(app, c, count);
            return;
        }
    }

    match key.code {
        KeyCode::Esc => {
            if app.mode.is_visual() {
                app.leave_visual();
            }
            app.pending.reset();
        }
        KeyCode::Char(c) => normal_char(app, c),
        KeyCode::Left => apply_motion(app, Motion::Left),
        KeyCode::Right => apply_motion(app, Motion::Right),
        KeyCode::Up => apply_motion(app, Motion::Up),
        KeyCode::Down => apply_motion(app, Motion::Down),
        KeyCode::Home => apply_motion(app, Motion::LineStart),
        KeyCode::End => apply_motion(app, Motion::LineEnd),
        KeyCode::PageDown => scroll_pages(app, 1, count),
        KeyCode::PageUp => scroll_pages(app, -1, count),
        KeyCode::Enter => apply_motion(app, Motion::NextLine),
        KeyCode::Backspace => apply_motion(app, Motion::Left),
        KeyCode::Delete => {
            app.delete_chars(count, false, app.pending.register);
            app.pending.reset();
        }
        _ => app.pending.reset(),
    }
}

fn normal_char(app: &mut App, c: char) {
    // Digits build the count, except a leading `0`, which is a motion.
    if c.is_ascii_digit() && (c != '0' || current_count_active(app)) {
        let digit = c.to_digit(10).unwrap() as usize;
        let slot = if app.pending.operator.is_some() {
            &mut app.pending.operator_count
        } else {
            &mut app.pending.count
        };
        *slot = Some(slot.unwrap_or(0).saturating_mul(10).saturating_add(digit));
        app.pending.display.push(c);
        return;
    }

    let count = app.pending.effective_count();

    match c {
        '"' => app.pending.awaiting = Some(Awaiting::Register),

        // Operators
        'd' => operator_key(app, Operator::Delete),
        'c' => operator_key(app, Operator::Change),
        'y' => operator_key(app, Operator::Yank),
        '>' => operator_key(app, Operator::Indent),
        '<' => operator_key(app, Operator::Dedent),
        'g' => app.pending.awaiting = Some(Awaiting::GPrefix),
        'z' => app.pending.awaiting = Some(Awaiting::ZPrefix),
        'Z' => app.pending.awaiting = Some(Awaiting::ZetPrefix),

        // Text objects, only meaningful after an operator or in visual mode.
        'i' if app.pending.operator.is_some() || app.mode.is_visual() => {
            app.pending.awaiting = Some(Awaiting::TextObject {
                scope: Scope::Inner,
            });
        }
        'a' if app.pending.operator.is_some() || app.mode.is_visual() => {
            app.pending.awaiting = Some(Awaiting::TextObject {
                scope: Scope::Around,
            });
        }

        // Motions
        'h' => apply_motion(app, Motion::Left),
        'l' | ' ' => apply_motion(app, Motion::Right),
        'j' => apply_motion(app, Motion::Down),
        'k' => apply_motion(app, Motion::Up),
        '+' => apply_motion(app, Motion::NextLine),
        '-' => apply_motion(app, Motion::PrevLine),
        'w' => apply_motion(app, Motion::WordForward { big: false }),
        'W' => apply_motion(app, Motion::WordForward { big: true }),
        'e' => apply_motion(app, Motion::WordEnd { big: false }),
        'E' => apply_motion(app, Motion::WordEnd { big: true }),
        'b' => apply_motion(app, Motion::WordBackward { big: false }),
        'B' => apply_motion(app, Motion::WordBackward { big: true }),
        '0' => apply_motion(app, Motion::LineStart),
        '^' => apply_motion(app, Motion::FirstNonBlank),
        '$' => apply_motion(app, Motion::LineEnd),
        '|' => apply_motion(app, Motion::Column),
        'G' => {
            push_jump(app);
            apply_motion(app, Motion::GotoLine);
        }
        '{' => {
            push_jump(app);
            apply_motion(app, Motion::ParagraphBackward);
        }
        '}' => {
            push_jump(app);
            apply_motion(app, Motion::ParagraphForward);
        }
        '%' => {
            push_jump(app);
            apply_motion(app, Motion::MatchPair);
        }
        'H' => {
            push_jump(app);
            apply_motion(app, Motion::ScreenTop);
        }
        'M' => {
            push_jump(app);
            apply_motion(app, Motion::ScreenMiddle);
        }
        'L' => {
            push_jump(app);
            apply_motion(app, Motion::ScreenBottom);
        }
        'f' => {
            app.pending.awaiting = Some(Awaiting::FindChar {
                till: false,
                backward: false,
            })
        }
        'F' => {
            app.pending.awaiting = Some(Awaiting::FindChar {
                till: false,
                backward: true,
            })
        }
        't' => {
            app.pending.awaiting = Some(Awaiting::FindChar {
                till: true,
                backward: false,
            })
        }
        'T' => {
            app.pending.awaiting = Some(Awaiting::FindChar {
                till: true,
                backward: true,
            })
        }
        ';' => apply_motion(app, Motion::RepeatFind { reverse: false }),
        ',' => apply_motion(app, Motion::RepeatFind { reverse: true }),
        'n' => {
            push_jump(app);
            apply_motion(app, Motion::SearchNext { reverse: false });
        }
        'N' => {
            push_jump(app);
            apply_motion(app, Motion::SearchNext { reverse: true });
        }
        '`' => app.pending.awaiting = Some(Awaiting::GotoMark { linewise: false }),
        '\'' => app.pending.awaiting = Some(Awaiting::GotoMark { linewise: true }),
        'm' => app.pending.awaiting = Some(Awaiting::Mark),

        // Mode changes
        'i' => {
            app.enter_insert();
            app.pending.reset();
        }
        'a' => {
            app.enter_insert();
            let buffer = app.buffer_mut();
            let len = text::line_len(&buffer.rope, buffer.cursor.line);
            buffer.cursor.col = (buffer.cursor.col + 1).min(len);
            buffer.desired_col = buffer.cursor.col;
            app.pending.reset();
        }
        'I' => {
            let line = app.buffer().cursor.line;
            let col = text::first_non_blank(&app.buffer().rope, line);
            app.enter_insert();
            app.set_cursor(Position::new(line, col));
            app.pending.reset();
        }
        'A' => {
            let line = app.buffer().cursor.line;
            let len = app.buffer().line_len(line);
            app.enter_insert();
            app.set_cursor(Position::new(line, len));
            app.pending.reset();
        }
        'o' => {
            if app.mode.is_visual() {
                let cursor = app.buffer().cursor;
                let anchor = app.visual_anchor;
                app.visual_anchor = cursor;
                app.set_cursor(anchor);
            } else {
                app.open_line(true);
            }
            app.pending.reset();
        }
        'O' => {
            app.open_line(false);
            app.pending.reset();
        }
        'R' => {
            app.mode = Mode::Replace;
            app.buffer_mut().begin();
            app.pending.reset();
        }
        'v' => {
            app.enter_visual(VisualKind::Char);
            app.pending.reset();
        }
        'V' => {
            app.enter_visual(VisualKind::Line);
            app.pending.reset();
        }
        // Single-key edits
        'x' => {
            if app.mode.is_visual() {
                visual_operator(app, Operator::Delete);
            } else {
                app.delete_chars(count, false, app.pending.register);
                app.pending.reset();
            }
        }
        'X' => {
            app.delete_chars(count, true, app.pending.register);
            app.pending.reset();
        }
        'D' => to_line_end(app, Operator::Delete),
        'C' => to_line_end(app, Operator::Change),
        'Y' => {
            if app.mode.is_visual() {
                visual_operator(app, Operator::Yank);
            } else {
                linewise_operator(app, Operator::Yank, count);
            }
        }
        'S' => linewise_operator(app, Operator::Change, count),
        's' => {
            if app.mode.is_visual() {
                visual_operator(app, Operator::Change);
            } else {
                let cursor = app.buffer().cursor;
                let len = app.buffer().line_len(cursor.line);
                let start = app.buffer().cursor_char();
                let end = start + (count.min(len.saturating_sub(cursor.col)));
                apply_operator(
                    app,
                    Operator::Change,
                    motion::EditRange {
                        start,
                        end,
                        linewise: false,
                    },
                );
            }
        }
        'p' => {
            if app.mode.is_visual() {
                visual_put(app);
            } else {
                app.put(app.pending.register, true, count);
                app.pending.reset();
            }
        }
        'P' => {
            app.put(app.pending.register, false, count);
            app.pending.reset();
        }
        'J' => {
            if app.mode.is_visual() {
                let (first, last) = app.visual_lines();
                app.leave_visual();
                app.set_cursor(Position::new(first, 0));
                app.join_lines((last - first + 1).max(2));
            } else {
                app.join_lines(count.max(2));
            }
            app.pending.reset();
        }
        'r' => app.pending.awaiting = Some(Awaiting::ReplaceChar),
        '~' => {
            if app.mode.is_visual() {
                visual_operator(app, Operator::ToggleCase);
            } else {
                app.toggle_case(count);
                app.pending.reset();
            }
        }
        'u' => {
            if app.mode.is_visual() {
                visual_operator(app, Operator::Lowercase);
            } else {
                undo(app, count);
            }
        }
        'U' if app.mode.is_visual() => visual_operator(app, Operator::Uppercase),
        '.' => {
            repeat_last_change(app);
        }
        'q' => {
            if let Some(recording) = app.recording.take() {
                let text = keys::encode_all(&recording.keys);
                app.registers
                    .yank(Some(recording.register), RegisterContent::charwise(text));
                app.set_message(format!("recorded into register {}", recording.register));
            } else {
                app.pending.awaiting = Some(Awaiting::RecordMacro);
            }
        }
        '@' => app.pending.awaiting = Some(Awaiting::PlayMacro),
        ':' => {
            let range = if app.mode.is_visual() {
                app.leave_visual();
                "'<,'>".to_string()
            } else {
                String::new()
            };
            command::open_prompt(app, PromptKind::Command, &range);
            app.pending.reset();
        }
        '/' => {
            push_jump(app);
            command::open_prompt(app, PromptKind::SearchForward, "");
            app.pending.reset();
        }
        '?' => {
            push_jump(app);
            command::open_prompt(app, PromptKind::SearchBackward, "");
            app.pending.reset();
        }
        '*' => search_word_under_cursor(app),
        _ => {
            app.pending.reset();
        }
    }
}

fn current_count_active(app: &App) -> bool {
    if app.pending.operator.is_some() {
        app.pending.operator_count.is_some()
    } else {
        app.pending.count.is_some()
    }
}

fn control_key(app: &mut App, c: char, count: usize) {
    match c {
        'd' => scroll_halves(app, 1, count),
        'u' => scroll_halves(app, -1, count),
        'f' => scroll_pages(app, 1, count),
        'b' => scroll_pages(app, -1, count),
        'e' => scroll_lines(app, 1, count),
        'y' => scroll_lines(app, -1, count),
        'r' => redo(app, count),
        'o' => jump_back(app),
        'i' => jump_forward(app),
        'v' => {
            app.set_error("visual block mode is not implemented");
            app.pending.reset();
        }
        'g' => {
            let buffer = app.buffer();
            let message = format!(
                "\"{}\" {} lines --{}%--",
                buffer.display_name(),
                buffer.line_count(),
                (buffer.cursor.line + 1) * 100 / buffer.line_count().max(1)
            );
            app.set_message(message);
            app.pending.reset();
        }
        _ => app.pending.reset(),
    }
}

fn handle_awaiting(app: &mut App, awaiting: Awaiting, key: KeyEvent) {
    let count = app.pending.effective_count();
    let ch = match key.code {
        KeyCode::Char(c) => Some(c),
        KeyCode::Esc => {
            app.pending.reset();
            return;
        }
        _ => None,
    };

    match awaiting {
        Awaiting::Register => match ch {
            Some(c) => app.pending.register = Some(c),
            None => app.pending.reset(),
        },
        Awaiting::FindChar { till, backward } => match ch {
            Some(c) => {
                let target = FindTarget {
                    ch: c,
                    backward,
                    till,
                };
                app.last_find = Some(target);
                apply_motion(app, Motion::Find(target));
            }
            None => app.pending.reset(),
        },
        Awaiting::Mark => {
            if let Some(c) = ch {
                let cursor = app.buffer().cursor;
                app.buffer_mut().marks.insert(c, cursor);
            }
            app.pending.reset();
        }
        Awaiting::GotoMark { linewise } => match ch {
            Some(c) => {
                push_jump(app);
                apply_motion(app, Motion::Mark { ch: c, linewise });
            }
            None => app.pending.reset(),
        },
        Awaiting::ReplaceChar => {
            if let Some(c) = ch {
                if app.mode.is_visual() {
                    visual_replace_char(app, c);
                } else {
                    app.replace_chars(c, count);
                }
            }
            app.pending.reset();
        }
        Awaiting::GPrefix => match ch {
            Some('g') => {
                push_jump(app);
                apply_motion(app, Motion::GotoFirstLine);
            }
            Some('_') => apply_motion(app, Motion::LastNonBlank),
            Some('u') => operator_key(app, Operator::Lowercase),
            Some('U') => operator_key(app, Operator::Uppercase),
            Some('~') => operator_key(app, Operator::ToggleCase),
            Some('I') => {
                let line = app.buffer().cursor.line;
                app.enter_insert();
                app.set_cursor(Position::new(line, 0));
                app.pending.reset();
            }
            Some('v') => {
                app.mode = Mode::Visual(VisualKind::Char);
                app.pending.reset();
            }
            _ => app.pending.reset(),
        },
        Awaiting::ZPrefix => {
            let height = app.viewport.height.max(1);
            let line = app.buffer().cursor.line;
            match ch {
                Some('z') => app.buffer_mut().view_top = line.saturating_sub(height / 2),
                Some('t') => app.buffer_mut().view_top = line,
                Some('b') => app.buffer_mut().view_top = line.saturating_sub(height - 1),
                _ => {}
            }
            app.pending.reset();
        }
        Awaiting::ZetPrefix => {
            match ch {
                Some('Z') => command::execute(app, "x"),
                Some('Q') => command::execute(app, "q!"),
                _ => {}
            }
            app.pending.reset();
        }
        Awaiting::TextObject { scope } => {
            let object = match ch {
                Some('w') => Some(TextObject::Word { big: false }),
                Some('W') => Some(TextObject::Word { big: true }),
                Some('"') => Some(TextObject::Quoted { ch: '"' }),
                Some('\'') => Some(TextObject::Quoted { ch: '\'' }),
                Some('`') => Some(TextObject::Quoted { ch: '`' }),
                Some('(') | Some(')') | Some('b') => Some(TextObject::Delimited {
                    open: '(',
                    close: ')',
                }),
                Some('[') | Some(']') => Some(TextObject::Delimited {
                    open: '[',
                    close: ']',
                }),
                Some('{') | Some('}') | Some('B') => Some(TextObject::Delimited {
                    open: '{',
                    close: '}',
                }),
                Some('<') | Some('>') => Some(TextObject::Delimited {
                    open: '<',
                    close: '>',
                }),
                Some('p') => Some(TextObject::Paragraph),
                _ => None,
            };
            match object {
                Some(object) => {
                    let cursor = app.buffer().cursor;
                    let resolved = crate::textobject::resolve(
                        &app.buffer().rope,
                        cursor,
                        object,
                        scope,
                        count,
                    );
                    match resolved {
                        Some(range) => {
                            if app.mode.is_visual() {
                                // Extend the selection to cover the object.
                                let start = text::char_to_pos(&app.buffer().rope, range.start);
                                let end = text::char_to_pos(
                                    &app.buffer().rope,
                                    range.end.saturating_sub(1),
                                );
                                app.visual_anchor = start;
                                app.set_cursor(end);
                                app.pending.reset();
                            } else if let Some(op) = app.pending.operator {
                                apply_operator(app, op, range);
                            } else {
                                app.pending.reset();
                            }
                        }
                        None => {
                            app.pending.reset();
                        }
                    }
                }
                None => app.pending.reset(),
            }
        }
        Awaiting::RecordMacro => {
            match ch {
                Some(c) if c.is_ascii_alphanumeric() => {
                    app.recording = Some(crate::app::Recording {
                        register: c,
                        keys: Vec::new(),
                    });
                    app.set_message(format!("recording @{c}"));
                }
                _ => app.set_error("invalid register for recording"),
            }
            app.pending.reset();
        }
        Awaiting::PlayMacro => {
            let register = match ch {
                Some('@') => app.registers.get(Some('@')).map(|_| '@'),
                Some(c) => Some(c),
                None => None,
            };
            match register.and_then(|c| app.registers.get(Some(c)).cloned()) {
                Some(content) => {
                    let parsed = keys::parse(&content.text);
                    let repeats = count;
                    for _ in 0..repeats {
                        app.queue_keys(&parsed);
                    }
                }
                None => app.set_error("register is empty"),
            }
            app.pending.reset();
        }
        Awaiting::InsertRegister => {
            if let Some(c) = ch {
                if let Some(content) = app.registers.get(Some(c)).cloned() {
                    app.insert_text(&content.text);
                }
            }
            app.pending.reset();
        }
    }
}

// -- operators --------------------------------------------------------------

fn operator_key(app: &mut App, op: Operator) {
    if app.mode.is_visual() {
        visual_operator(app, op);
        return;
    }
    match app.pending.operator {
        // A doubled operator acts on whole lines: `dd`, `yy`, `>>`.
        Some(existing) if existing == op => {
            let count = app.pending.effective_count();
            linewise_operator(app, op, count);
        }
        Some(_) => app.pending.reset(),
        None => app.pending.operator = Some(op),
    }
}

fn linewise_operator(app: &mut App, op: Operator, count: usize) {
    let buffer = app.buffer();
    let last = buffer.line_count().saturating_sub(1);
    let first_line = buffer.cursor.line;
    let end_line = (first_line + count.max(1) - 1).min(last);
    let start = buffer.rope.line_to_char(first_line);
    let end = if end_line >= last {
        buffer.rope.len_chars()
    } else {
        buffer.rope.line_to_char(end_line + 1)
    };
    apply_operator(
        app,
        op,
        motion::EditRange {
            start,
            end,
            linewise: true,
        },
    );
}

fn to_line_end(app: &mut App, op: Operator) {
    let cursor = app.buffer().cursor;
    let len = app.buffer().line_len(cursor.line);
    let line_start = app.buffer().rope.line_to_char(cursor.line);
    apply_operator(
        app,
        op,
        motion::EditRange {
            start: line_start + cursor.col,
            end: line_start + len,
            linewise: false,
        },
    );
}

fn visual_operator(app: &mut App, op: Operator) {
    let range = app.visual_range();
    app.leave_visual();
    apply_operator(app, op, range);
}

fn visual_put(app: &mut App) {
    let range = app.visual_range();
    let register = app.pending.register;
    app.leave_visual();
    app.dot.changed = true;
    app.buffer_mut().begin();
    let start = range.start;
    app.buffer_mut().remove(range.start, range.end);
    let position = text::char_to_pos(&app.buffer().rope, start);
    app.buffer_mut().cursor = position;
    app.put(register, false, 1);
    app.buffer_mut().end();
    app.pending.reset();
}

fn visual_replace_char(app: &mut App, c: char) {
    let range = app.visual_range();
    app.leave_visual();
    app.dot.changed = true;
    let original = app.buffer().slice(range.start, range.end);
    let replaced: String = original
        .chars()
        .map(|existing| if existing == '\n' { '\n' } else { c })
        .collect();
    let buffer = app.buffer_mut();
    buffer.begin();
    buffer.replace(range.start, range.end, &replaced);
    buffer.end();
    buffer.cursor = text::clamp(
        &buffer.rope,
        text::char_to_pos(&buffer.rope, range.start),
        false,
    );
}

fn apply_operator(app: &mut App, op: Operator, range: motion::EditRange) {
    let register = app.pending.register;
    let options = app.edit_options();
    let outcome = {
        let App {
            buffers,
            current,
            registers,
            ..
        } = app;
        operator::apply(
            op,
            &mut buffers[*current],
            range,
            registers,
            register,
            options,
        )
    };
    if op != Operator::Yank {
        app.dot.changed = true;
    }
    app.pending.reset();
    if outcome == Outcome::EnterInsert {
        // `operator::apply` left the undo transaction open so the typing that
        // follows undoes together with the deletion.
        app.mode = Mode::Insert;
    }
}

/// Resolve a motion and either move the cursor or feed an operator.
fn apply_motion(app: &mut App, motion: Motion) {
    let count = app.pending.effective_count();
    let had_count = app.pending.had_count();
    let resolved = {
        let buffer = app.buffer();
        let ctx = motion::Context {
            rope: &buffer.rope,
            cursor: buffer.cursor,
            count,
            had_count,
            view_top: buffer.view_top,
            view_height: app.viewport.height.max(1),
            scrolloff: app.config.editor.scrolloff,
            last_find: app.last_find,
            search: &app.search,
            marks: &buffer.marks,
            wrap_search: app.config.editor.wrap_search,
            allow_eol: app.mode.allows_eol() || app.pending.operator.is_some(),
        };
        motion::resolve(motion, &ctx)
    };

    if resolved.failed {
        if matches!(motion, Motion::SearchNext { .. }) {
            let pattern = app.search.pattern.clone();
            app.set_error(format!("E486: Pattern not found: {pattern}"));
        }
        app.pending.reset();
        return;
    }

    match app.pending.operator {
        None => {
            let vertical = matches!(resolved.kind, MotionKind::Linewise)
                && matches!(motion, Motion::Up | Motion::Down);
            if vertical {
                app.move_vertical(resolved.target.line);
            } else {
                app.set_cursor(resolved.target);
            }
            app.pending.reset();
        }
        Some(op) => {
            let cursor = app.buffer().cursor;
            let mut resolved = resolved;

            // `cw` on a non-blank behaves like `ce`: Vim's one wart that users
            // actually rely on.
            if op == Operator::Change {
                if let Motion::WordForward { big } = motion {
                    let on_blank = text::char_at(&app.buffer().rope, cursor)
                        .map(char::is_whitespace)
                        .unwrap_or(true);
                    if !on_blank {
                        let mut idx = text::pos_to_char(&app.buffer().rope, cursor);
                        for _ in 0..count {
                            idx = motion::next_word_end(&app.buffer().rope, idx, big);
                        }
                        resolved.target = text::char_to_pos(&app.buffer().rope, idx);
                        resolved.kind = MotionKind::Inclusive;
                    }
                }
            }

            // `dw` on the last word of a line stops at the line end rather
            // than pulling the next line up.
            if matches!(motion, Motion::WordForward { .. })
                && resolved.target.line > cursor.line
                && resolved.kind == MotionKind::Exclusive
            {
                let len = app.buffer().line_len(cursor.line);
                resolved.target = Position::new(cursor.line, len);
            }

            let range = motion::to_range(&app.buffer().rope, cursor, &resolved);
            apply_operator(app, op, range);
        }
    }
}

// -- scrolling and history --------------------------------------------------

fn scroll_lines(app: &mut App, direction: i32, count: usize) {
    let amount = count.max(1);
    let height = app.viewport.height.max(1);
    let last = app.buffer().line_count().saturating_sub(1);
    let buffer = app.buffer_mut();
    buffer.view_top = if direction > 0 {
        (buffer.view_top + amount).min(last)
    } else {
        buffer.view_top.saturating_sub(amount)
    };
    // Drag the cursor along only if it would otherwise leave the viewport.
    let top = buffer.view_top;
    let bottom = (top + height).saturating_sub(1);
    buffer.cursor.line = buffer.cursor.line.clamp(top, bottom.min(last));
    app.clamp_cursor();
    app.pending.reset();
}

fn scroll_halves(app: &mut App, direction: i32, count: usize) {
    let half = (app.viewport.height.max(2) / 2).max(1) * count.max(1);
    move_by_lines(app, direction * half as i32);
}

fn scroll_pages(app: &mut App, direction: i32, count: usize) {
    let page = app.viewport.height.max(2).saturating_sub(2).max(1) * count.max(1);
    move_by_lines(app, direction * page as i32);
}

fn move_by_lines(app: &mut App, delta: i32) {
    let last = app.buffer().line_count().saturating_sub(1);
    let line = app.buffer().cursor.line;
    let target = if delta >= 0 {
        (line + delta as usize).min(last)
    } else {
        line.saturating_sub((-delta) as usize)
    };
    let view_top = app.buffer().view_top;
    let new_top = if delta >= 0 {
        (view_top + delta as usize).min(last)
    } else {
        view_top.saturating_sub((-delta) as usize)
    };
    app.buffer_mut().view_top = new_top;
    app.move_vertical(target);
    app.pending.reset();
}

fn undo(app: &mut App, count: usize) {
    let mut applied = 0;
    for _ in 0..count.max(1) {
        if app.buffer_mut().undo() {
            applied += 1;
        } else {
            break;
        }
    }
    if applied == 0 {
        app.set_error("E32: Already at oldest change");
    } else {
        app.set_message(format!("{applied} change(s) undone"));
    }
    app.pending.reset();
}

fn redo(app: &mut App, count: usize) {
    let mut applied = 0;
    for _ in 0..count.max(1) {
        if app.buffer_mut().redo() {
            applied += 1;
        } else {
            break;
        }
    }
    if applied == 0 {
        app.set_error("E33: Already at newest change");
    } else {
        app.set_message(format!("{applied} change(s) redone"));
    }
    app.pending.reset();
}

/// `.` replays the keystrokes of the last change, with an optional new count
/// replacing the original one.
fn repeat_last_change(app: &mut App) {
    if app.dot.last.is_empty() {
        app.set_error("nothing to repeat");
        app.pending.reset();
        return;
    }
    let mut replay = app.dot.last.clone();
    if let Some(count) = app.pending.count {
        while replay
            .first()
            .and_then(|k| match k.code {
                KeyCode::Char(c) => c.to_digit(10),
                _ => None,
            })
            .is_some()
        {
            replay.remove(0);
        }
        let digits: Vec<KeyEvent> = count
            .to_string()
            .chars()
            .map(|c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE))
            .collect();
        replay.splice(0..0, digits);
    }
    app.pending.reset();
    app.queue_keys(&replay);
}

fn search_word_under_cursor(app: &mut App) {
    let cursor = app.buffer().cursor;
    let range = crate::textobject::resolve(
        &app.buffer().rope,
        cursor,
        TextObject::Word { big: false },
        Scope::Inner,
        1,
    );
    let Some(range) = range else {
        app.pending.reset();
        return;
    };
    let word = app.buffer().slice(range.start, range.end);
    if word.trim().is_empty() {
        app.set_error("E348: No string under cursor");
        app.pending.reset();
        return;
    }
    let pattern = format!(r"\b{}\b", regex::escape(&word));
    let (ignore_case, smart_case) = (app.config.editor.ignore_case, app.config.editor.smart_case);
    if app
        .search
        .set_pattern(&pattern, Direction::Forward, ignore_case, smart_case)
        .is_err()
    {
        app.pending.reset();
        return;
    }
    app.search_highlight = true;
    push_jump(app);
    apply_motion(app, Motion::SearchNext { reverse: false });
}

// -- jump list --------------------------------------------------------------

pub fn push_jump(app: &mut App) {
    let cursor = app.buffer().cursor;
    let buffer = app.buffer_mut();
    buffer.jumps.truncate(buffer.jump_index);
    buffer.jumps.push(cursor);
    if buffer.jumps.len() > 100 {
        buffer.jumps.remove(0);
    }
    buffer.jump_index = buffer.jumps.len();
}

fn jump_back(app: &mut App) {
    let cursor = app.buffer().cursor;
    let buffer = app.buffer_mut();
    if buffer.jump_index == 0 {
        app.pending.reset();
        return;
    }
    if buffer.jump_index == buffer.jumps.len() {
        buffer.jumps.push(cursor);
    }
    buffer.jump_index -= 1;
    let target = buffer.jumps[buffer.jump_index];
    app.set_cursor(target);
    app.pending.reset();
}

fn jump_forward(app: &mut App) {
    let buffer = app.buffer_mut();
    if buffer.jump_index + 1 >= buffer.jumps.len() {
        app.pending.reset();
        return;
    }
    buffer.jump_index += 1;
    let target = buffer.jumps[buffer.jump_index];
    app.set_cursor(target);
    app.pending.reset();
}

// -- insert and replace modes ----------------------------------------------

fn insert_mode(app: &mut App, key: KeyEvent) {
    if let Some(awaiting) = app.pending.awaiting {
        app.pending.awaiting = None;
        handle_awaiting(app, awaiting, key);
        return;
    }

    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    if ctrl {
        match key.code {
            KeyCode::Char('w') => return app.insert_delete_word(),
            KeyCode::Char('u') => return app.insert_delete_to_line_start(),
            KeyCode::Char('h') => return app.insert_backspace(),
            KeyCode::Char('r') => {
                app.pending.awaiting = Some(Awaiting::InsertRegister);
                return;
            }
            KeyCode::Char('c') => return app.leave_insert(),
            // Ctrl-J and Ctrl-M *are* LF and CR.
            KeyCode::Char('j') | KeyCode::Char('m') => return app.insert_newline(),
            // Anything else control-modified is not a character to insert.
            // Without this, Ctrl-K typed a literal `k`.
            KeyCode::Char(_) => return,
            _ => {}
        }
    }

    match key.code {
        KeyCode::Esc => app.leave_insert(),
        KeyCode::Char(c) => app.insert_char(c),
        KeyCode::Enter => app.insert_newline(),
        KeyCode::Tab => app.insert_tab(),
        KeyCode::Backspace => app.insert_backspace(),
        KeyCode::Delete => app.insert_delete_forward(),
        KeyCode::Left => {
            let cursor = app.buffer().cursor;
            app.set_cursor(Position::new(cursor.line, cursor.col.saturating_sub(1)));
        }
        KeyCode::Right => {
            let cursor = app.buffer().cursor;
            let len = app.buffer().line_len(cursor.line);
            app.set_cursor(Position::new(cursor.line, (cursor.col + 1).min(len)));
        }
        KeyCode::Up => {
            let line = app.buffer().cursor.line;
            app.move_vertical(line.saturating_sub(1));
        }
        KeyCode::Down => {
            let line = app.buffer().cursor.line;
            app.move_vertical(line + 1);
        }
        KeyCode::Home => {
            let cursor = app.buffer().cursor;
            app.set_cursor(Position::new(cursor.line, 0));
        }
        KeyCode::End => {
            let cursor = app.buffer().cursor;
            let len = app.buffer().line_len(cursor.line);
            app.set_cursor(Position::new(cursor.line, len));
        }
        _ => {}
    }
}

fn replace_mode(app: &mut App, key: KeyEvent) {
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        return match key.code {
            KeyCode::Char('c') => app.leave_insert(),
            KeyCode::Char('j') | KeyCode::Char('m') => app.insert_newline(),
            _ => {}
        };
    }
    match key.code {
        KeyCode::Esc => app.leave_insert(),
        KeyCode::Char(c) => app.replace_at_cursor(c),
        KeyCode::Enter => app.insert_newline(),
        KeyCode::Backspace => {
            let cursor = app.buffer().cursor;
            app.set_cursor(Position::new(cursor.line, cursor.col.saturating_sub(1)));
        }
        _ => {}
    }
}

/// Text arriving from a bracketed paste, which the terminal delivers whole.
pub fn handle_paste(app: &mut App, text: &str) {
    match app.mode {
        Mode::Insert | Mode::Replace => {
            let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
            app.insert_text(&normalized);
        }
        Mode::Command | Mode::Search => {
            let single_line: String = text.lines().collect::<Vec<_>>().join(" ");
            command::prompt_insert(app, &single_line);
        }
        _ => {
            let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
            app.registers
                .yank(Some('"'), RegisterContent::charwise(normalized));
            app.put(None, true, 1);
        }
    }
}

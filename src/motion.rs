//! Motions: the "where to" half of Vim's `operator + motion` grammar.
//!
//! A motion resolves to a target position plus a [`MotionKind`] that tells an
//! operator how to turn `cursor -> target` into a range: whether the character
//! under the target is included, and whether whole lines are affected.

use crate::search::Search;
use crate::text::{self, char_class, CharClass, Position};
use ropey::Rope;
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MotionKind {
    /// The target character is not affected (`w`, `0`, `}`).
    Exclusive,
    /// The target character is affected (`e`, `f`, `$`).
    Inclusive,
    /// Whole lines are affected (`j`, `G`, `'a`).
    Linewise,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FindTarget {
    pub ch: char,
    pub backward: bool,
    /// `t`/`T` stop before the character; `f`/`F` land on it.
    pub till: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Motion {
    Left,
    Right,
    Up,
    Down,
    NextLine,
    PrevLine,
    WordForward { big: bool },
    WordEnd { big: bool },
    WordBackward { big: bool },
    LineStart,
    FirstNonBlank,
    LineEnd,
    LastNonBlank,
    Column,
    GotoLine,
    GotoFirstLine,
    Find(FindTarget),
    RepeatFind { reverse: bool },
    ParagraphForward,
    ParagraphBackward,
    MatchPair,
    ScreenTop,
    ScreenMiddle,
    ScreenBottom,
    SearchNext { reverse: bool },
    Mark { ch: char, linewise: bool },
}

/// Everything a motion may need to consult beyond the text itself.
pub struct Context<'a> {
    pub rope: &'a Rope,
    pub cursor: Position,
    pub count: usize,
    /// Explicit count typed by the user, which some motions treat specially.
    pub had_count: bool,
    pub view_top: usize,
    pub view_height: usize,
    pub scrolloff: usize,
    pub last_find: Option<FindTarget>,
    pub search: &'a Search,
    pub marks: &'a HashMap<char, Position>,
    pub wrap_search: bool,
    /// Insert and visual mode allow the cursor one column past the last
    /// character; normal mode does not.
    pub allow_eol: bool,
}

pub struct Resolved {
    pub target: Position,
    pub kind: MotionKind,
    /// Set when the motion could not be performed (no such mark, no match).
    pub failed: bool,
}

impl Resolved {
    fn new(target: Position, kind: MotionKind) -> Self {
        Self {
            target,
            kind,
            failed: false,
        }
    }

    fn failed(cursor: Position) -> Self {
        Self {
            target: cursor,
            kind: MotionKind::Exclusive,
            failed: true,
        }
    }
}

pub fn resolve(motion: Motion, ctx: &Context<'_>) -> Resolved {
    let rope = ctx.rope;
    let cursor = ctx.cursor;
    let count = ctx.count.max(1);
    let last_line = text::line_count(rope).saturating_sub(1);

    match motion {
        Motion::Left => {
            let col = cursor.col.saturating_sub(count);
            Resolved::new(Position::new(cursor.line, col), MotionKind::Exclusive)
        }
        Motion::Right => {
            let len = text::line_len(rope, cursor.line);
            let max = if ctx.allow_eol {
                len
            } else {
                len.saturating_sub(1)
            };
            let col = (cursor.col + count).min(max);
            Resolved::new(Position::new(cursor.line, col), MotionKind::Exclusive)
        }
        Motion::Up => {
            let line = cursor.line.saturating_sub(count);
            Resolved::new(Position::new(line, cursor.col), MotionKind::Linewise)
        }
        Motion::Down => {
            let line = (cursor.line + count).min(last_line);
            Resolved::new(Position::new(line, cursor.col), MotionKind::Linewise)
        }
        Motion::NextLine => {
            let line = (cursor.line + count).min(last_line);
            Resolved::new(
                Position::new(line, text::first_non_blank(rope, line)),
                MotionKind::Linewise,
            )
        }
        Motion::PrevLine => {
            let line = cursor.line.saturating_sub(count);
            Resolved::new(
                Position::new(line, text::first_non_blank(rope, line)),
                MotionKind::Linewise,
            )
        }
        Motion::WordForward { big } => {
            let mut idx = text::pos_to_char(rope, cursor);
            for _ in 0..count {
                idx = next_word_start(rope, idx, big);
            }
            Resolved::new(text::char_to_pos(rope, idx), MotionKind::Exclusive)
        }
        Motion::WordEnd { big } => {
            let mut idx = text::pos_to_char(rope, cursor);
            for _ in 0..count {
                idx = next_word_end(rope, idx, big);
            }
            Resolved::new(text::char_to_pos(rope, idx), MotionKind::Inclusive)
        }
        Motion::WordBackward { big } => {
            let mut idx = text::pos_to_char(rope, cursor);
            for _ in 0..count {
                idx = prev_word_start(rope, idx, big);
            }
            Resolved::new(text::char_to_pos(rope, idx), MotionKind::Exclusive)
        }
        Motion::LineStart => Resolved::new(Position::new(cursor.line, 0), MotionKind::Exclusive),
        Motion::FirstNonBlank => Resolved::new(
            Position::new(cursor.line, text::first_non_blank(rope, cursor.line)),
            MotionKind::Exclusive,
        ),
        Motion::LineEnd => {
            // `3$` moves down two lines and then to the end.
            let line = (cursor.line + count - 1).min(last_line);
            let len = text::line_len(rope, line);
            Resolved::new(
                Position::new(line, len.saturating_sub(1).max(0)),
                MotionKind::Inclusive,
            )
        }
        Motion::LastNonBlank => {
            let l = text::line(rope, cursor.line);
            let col = l
                .chars()
                .enumerate()
                .filter(|(_, c)| !c.is_whitespace())
                .map(|(i, _)| i)
                .last()
                .unwrap_or(0);
            Resolved::new(Position::new(cursor.line, col), MotionKind::Inclusive)
        }
        Motion::Column => {
            let len = text::line_len(rope, cursor.line);
            let col = count.saturating_sub(1).min(len.saturating_sub(1));
            Resolved::new(Position::new(cursor.line, col), MotionKind::Exclusive)
        }
        Motion::GotoLine => {
            let line = if ctx.had_count {
                count.saturating_sub(1).min(last_line)
            } else {
                last_line
            };
            Resolved::new(
                Position::new(line, text::first_non_blank(rope, line)),
                MotionKind::Linewise,
            )
        }
        Motion::GotoFirstLine => {
            let line = if ctx.had_count {
                count.saturating_sub(1).min(last_line)
            } else {
                0
            };
            Resolved::new(
                Position::new(line, text::first_non_blank(rope, line)),
                MotionKind::Linewise,
            )
        }
        Motion::Find(target) => find_char(rope, cursor, target, count),
        Motion::RepeatFind { reverse } => match ctx.last_find {
            Some(mut target) => {
                if reverse {
                    target.backward = !target.backward;
                }
                find_char(rope, cursor, target, count)
            }
            None => Resolved::failed(cursor),
        },
        Motion::ParagraphForward => {
            let mut line = cursor.line;
            for _ in 0..count {
                line = next_paragraph(rope, line, true);
            }
            let col = if line >= last_line {
                text::line_len(rope, line)
            } else {
                0
            };
            Resolved::new(Position::new(line, col), MotionKind::Exclusive)
        }
        Motion::ParagraphBackward => {
            let mut line = cursor.line;
            for _ in 0..count {
                line = next_paragraph(rope, line, false);
            }
            Resolved::new(Position::new(line, 0), MotionKind::Exclusive)
        }
        Motion::MatchPair => match match_pair(rope, cursor) {
            Some(pos) => Resolved::new(pos, MotionKind::Inclusive),
            None => Resolved::failed(cursor),
        },
        Motion::ScreenTop => {
            let line = (ctx.view_top + ctx.scrolloff.min(ctx.view_height / 2)).min(last_line);
            let line = if ctx.had_count {
                (ctx.view_top + count - 1).min(last_line)
            } else {
                line
            };
            Resolved::new(
                Position::new(line, text::first_non_blank(rope, line)),
                MotionKind::Linewise,
            )
        }
        Motion::ScreenMiddle => {
            let bottom = (ctx.view_top + ctx.view_height.saturating_sub(1)).min(last_line);
            let line = ctx.view_top + (bottom - ctx.view_top) / 2;
            Resolved::new(
                Position::new(line, text::first_non_blank(rope, line)),
                MotionKind::Linewise,
            )
        }
        Motion::ScreenBottom => {
            let bottom = (ctx.view_top + ctx.view_height.saturating_sub(1)).min(last_line);
            let line = if ctx.had_count {
                bottom.saturating_sub(count - 1)
            } else {
                bottom.saturating_sub(ctx.scrolloff.min(ctx.view_height / 2))
            };
            let line = line.max(ctx.view_top).min(last_line);
            Resolved::new(
                Position::new(line, text::first_non_blank(rope, line)),
                MotionKind::Linewise,
            )
        }
        Motion::SearchNext { reverse } => {
            let base = ctx.search.direction;
            let dir = if reverse { base.reversed() } else { base };
            let mut pos = cursor;
            let mut found = None;
            for _ in 0..count {
                match ctx.search.find(rope, pos, dir, ctx.wrap_search) {
                    Some(m) => {
                        pos = m.position();
                        found = Some(pos);
                    }
                    None => break,
                }
            }
            match found {
                Some(pos) => Resolved::new(pos, MotionKind::Exclusive),
                None => Resolved::failed(cursor),
            }
        }
        Motion::Mark { ch, linewise } => match ctx.marks.get(&ch) {
            Some(pos) => {
                let pos = text::clamp(rope, *pos, ctx.allow_eol);
                let target = if linewise {
                    Position::new(pos.line, text::first_non_blank(rope, pos.line))
                } else {
                    pos
                };
                Resolved::new(
                    target,
                    if linewise {
                        MotionKind::Linewise
                    } else {
                        MotionKind::Exclusive
                    },
                )
            }
            None => Resolved::failed(cursor),
        },
    }
}

fn classify(c: char, big: bool) -> CharClass {
    if big {
        text::big_char_class(c)
    } else {
        char_class(c)
    }
}

/// Start of the next word. An empty line counts as a word, as in Vim.
pub fn next_word_start(rope: &Rope, idx: usize, big: bool) -> usize {
    let len = rope.len_chars();
    if len == 0 {
        return 0;
    }
    let mut i = idx.min(len - 1);
    let class = classify(rope.char(i), big);
    if class != CharClass::Whitespace {
        while i < len && classify(rope.char(i), big) == class {
            i += 1;
        }
    }
    while i < len && classify(rope.char(i), big) == CharClass::Whitespace {
        if rope.char(i) == '\n' && i + 1 < len && rope.char(i + 1) == '\n' {
            return i + 1;
        }
        i += 1;
    }
    i.min(len - 1)
}

/// End of the current or next word.
pub fn next_word_end(rope: &Rope, idx: usize, big: bool) -> usize {
    let len = rope.len_chars();
    if len == 0 {
        return 0;
    }
    let mut i = idx + 1;
    while i < len && classify(rope.char(i), big) == CharClass::Whitespace {
        i += 1;
    }
    if i >= len {
        return len - 1;
    }
    let class = classify(rope.char(i), big);
    while i + 1 < len && classify(rope.char(i + 1), big) == class {
        i += 1;
    }
    i
}

/// Start of the current or previous word.
pub fn prev_word_start(rope: &Rope, idx: usize, big: bool) -> usize {
    if idx == 0 {
        return 0;
    }
    let mut i = idx - 1;
    while i > 0 && classify(rope.char(i), big) == CharClass::Whitespace {
        if rope.char(i) == '\n' && rope.char(i - 1) == '\n' {
            return i;
        }
        i -= 1;
    }
    if classify(rope.char(i), big) == CharClass::Whitespace {
        return i;
    }
    let class = classify(rope.char(i), big);
    while i > 0 && classify(rope.char(i - 1), big) == class {
        i -= 1;
    }
    i
}

fn find_char(rope: &Rope, cursor: Position, target: FindTarget, count: usize) -> Resolved {
    let line = text::line(rope, cursor.line);
    let chars: Vec<char> = line.chars().collect();
    let mut col = cursor.col;

    for _ in 0..count {
        let found = if target.backward {
            // `T` starts one further back so a repeat makes progress.
            let from = if target.till && col > 0 && chars.get(col - 1) == Some(&target.ch) {
                col.saturating_sub(1)
            } else {
                col
            };
            (0..from).rev().find(|&i| chars[i] == target.ch)
        } else {
            let from = if target.till && chars.get(col + 1) == Some(&target.ch) {
                col + 1
            } else {
                col
            };
            (from + 1..chars.len()).find(|&i| chars[i] == target.ch)
        };
        match found {
            Some(i) => col = i,
            None => return Resolved::failed(cursor),
        }
    }

    if target.till {
        col = if target.backward {
            col + 1
        } else {
            col.saturating_sub(1)
        };
    }
    Resolved::new(
        Position::new(cursor.line, col),
        if target.backward {
            MotionKind::Exclusive
        } else {
            MotionKind::Inclusive
        },
    )
}

/// Next blank line in the given direction, clamped to the ends of the file.
fn next_paragraph(rope: &Rope, from: usize, forward: bool) -> usize {
    let last = text::line_count(rope).saturating_sub(1);
    let mut line = from;
    loop {
        if forward {
            if line >= last {
                return last;
            }
            line += 1;
        } else {
            if line == 0 {
                return 0;
            }
            line -= 1;
        }
        if text::is_blank_line(rope, line) && !text::is_blank_line(rope, from) {
            return line;
        }
        if text::is_blank_line(rope, from) && !text::is_blank_line(rope, line) {
            // Started on a blank line: skip the run of blanks, then look for
            // the next blank after the following paragraph.
            let mut scan = line;
            loop {
                if forward {
                    if scan >= last {
                        return last;
                    }
                    scan += 1;
                } else {
                    if scan == 0 {
                        return 0;
                    }
                    scan -= 1;
                }
                if text::is_blank_line(rope, scan) {
                    return scan;
                }
            }
        }
    }
}

const PAIRS: [(char, char); 3] = [('(', ')'), ('[', ']'), ('{', '}')];

/// `%`: from the first bracket at or after the cursor on this line, jump to
/// its partner, respecting nesting.
fn match_pair(rope: &Rope, cursor: Position) -> Option<Position> {
    let line = text::line(rope, cursor.line);
    let chars: Vec<char> = line.chars().collect();
    let (col, open, close, forward) = (cursor.col..chars.len()).find_map(|i| {
        let c = chars[i];
        PAIRS.iter().find_map(|&(o, cl)| {
            if c == o {
                Some((i, o, cl, true))
            } else if c == cl {
                Some((i, o, cl, false))
            } else {
                None
            }
        })
    })?;

    let start = text::pos_to_char(rope, Position::new(cursor.line, col));
    let len = rope.len_chars();
    let mut depth = 0i32;
    if forward {
        for i in start..len {
            let c = rope.char(i);
            if c == open {
                depth += 1;
            } else if c == close {
                depth -= 1;
                if depth == 0 {
                    return Some(text::char_to_pos(rope, i));
                }
            }
        }
    } else {
        for i in (0..=start).rev() {
            let c = rope.char(i);
            if c == close {
                depth += 1;
            } else if c == open {
                depth -= 1;
                if depth == 0 {
                    return Some(text::char_to_pos(rope, i));
                }
            }
        }
    }
    None
}

/// Turn `cursor -> resolved` into a character range for an operator to act on.
pub struct EditRange {
    pub start: usize,
    pub end: usize,
    pub linewise: bool,
}

pub fn to_range(rope: &Rope, cursor: Position, resolved: &Resolved) -> EditRange {
    let (from, to) = if resolved.target < cursor {
        (resolved.target, cursor)
    } else {
        (cursor, resolved.target)
    };

    match resolved.kind {
        MotionKind::Linewise => {
            let last = text::line_count(rope).saturating_sub(1);
            let start = rope.line_to_char(from.line);
            let end_line = to.line.min(last);
            let end = if end_line >= last {
                rope.len_chars()
            } else {
                rope.line_to_char(end_line + 1)
            };
            EditRange {
                start,
                end,
                linewise: true,
            }
        }
        MotionKind::Exclusive => EditRange {
            start: text::pos_to_char(rope, from),
            end: text::pos_to_char(rope, to),
            linewise: false,
        },
        MotionKind::Inclusive => {
            let start = text::pos_to_char(rope, from);
            let end = (text::pos_to_char(rope, to) + 1).min(rope.len_chars());
            EditRange {
                start,
                end,
                linewise: false,
            }
        }
    }
}

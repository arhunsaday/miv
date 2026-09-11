//! Text primitives shared by every layer: positions, rope queries and the
//! character classification the word motions are built on.
//!
//! Invariant upheld by [`crate::core::buffer::Buffer`]: the rope always ends with a
//! newline. `line_count` therefore reports one fewer line than ropey does, and
//! every line index in `0..line_count()` is addressable.

use ropey::{Rope, RopeSlice};
use unicode_width::UnicodeWidthChar;

/// A cursor location. `col` counts *characters* into the line, not screen
/// columns; the renderer is the only layer that cares about the difference.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Position {
    pub line: usize,
    pub col: usize,
}

impl Position {
    pub fn new(line: usize, col: usize) -> Self {
        Self { line, col }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CharClass {
    Whitespace,
    Word,
    Punctuation,
}

/// Vim's `iskeyword` split: alphanumerics and `_` form words, other visible
/// characters are punctuation, and the rest is whitespace.
pub fn char_class(c: char) -> CharClass {
    if c.is_whitespace() {
        CharClass::Whitespace
    } else if c.is_alphanumeric() || c == '_' {
        CharClass::Word
    } else {
        CharClass::Punctuation
    }
}

/// Class used by the `W`/`B`/`E` motions, which only distinguish blank from
/// non-blank.
pub fn big_char_class(c: char) -> CharClass {
    if c.is_whitespace() {
        CharClass::Whitespace
    } else {
        CharClass::Word
    }
}

pub fn line_count(rope: &Rope) -> usize {
    rope.len_lines().saturating_sub(1).max(1)
}

/// The line's content without its trailing newline (or CRLF).
pub fn line(rope: &Rope, line: usize) -> RopeSlice<'_> {
    if line >= line_count(rope) {
        return RopeSlice::from("");
    }
    let slice = rope.line(line);
    let mut end = slice.len_chars();
    if end > 0 && slice.char(end - 1) == '\n' {
        end -= 1;
        if end > 0 && slice.char(end - 1) == '\r' {
            end -= 1;
        }
    }
    slice.slice(..end)
}

pub fn line_len(rope: &Rope, line_idx: usize) -> usize {
    line(rope, line_idx).len_chars()
}

pub fn pos_to_char(rope: &Rope, pos: Position) -> usize {
    let line_idx = pos.line.min(line_count(rope).saturating_sub(1));
    let start = rope.line_to_char(line_idx);
    start + pos.col.min(line_len(rope, line_idx))
}

pub fn char_to_pos(rope: &Rope, idx: usize) -> Position {
    let idx = idx.min(rope.len_chars());
    let line = rope
        .char_to_line(idx)
        .min(line_count(rope).saturating_sub(1));
    let col = idx - rope.line_to_char(line);
    Position::new(line, col.min(line_len(rope, line)))
}

/// Clamp a position into the buffer. `allow_eol` distinguishes insert and
/// visual mode (where the cursor may sit one past the last character) from
/// normal mode (where it may not).
pub fn clamp(rope: &Rope, pos: Position, allow_eol: bool) -> Position {
    let last = line_count(rope).saturating_sub(1);
    let line_idx = pos.line.min(last);
    let len = line_len(rope, line_idx);
    let max_col = if allow_eol {
        len
    } else {
        len.saturating_sub(1)
    };
    Position::new(line_idx, pos.col.min(max_col))
}

pub fn char_at(rope: &Rope, pos: Position) -> Option<char> {
    let l = line(rope, pos.line);
    (pos.col < l.len_chars()).then(|| l.char(pos.col))
}

/// Column of the first non-blank character, or the line length for a blank line.
pub fn first_non_blank(rope: &Rope, line_idx: usize) -> usize {
    let l = line(rope, line_idx);
    l.chars()
        .position(|c| !c.is_whitespace())
        .unwrap_or_else(|| l.len_chars())
}

pub fn is_blank_line(rope: &Rope, line_idx: usize) -> bool {
    line(rope, line_idx).chars().all(char::is_whitespace)
}

/// Screen width of one character placed at `at_col`, expanding tabs to the
/// next tab stop and giving East Asian wide characters their true two cells.
pub fn char_width(c: char, at_col: usize, tab_width: usize) -> usize {
    match c {
        '\t' => tab_width - (at_col % tab_width),
        '\n' | '\r' => 0,
        c => UnicodeWidthChar::width(c).unwrap_or(1).max(1),
    }
}

/// Screen column that character `col` of this line starts at.
pub fn screen_col(l: RopeSlice<'_>, col: usize, tab_width: usize) -> usize {
    let mut width = 0;
    for c in l.chars().take(col) {
        width += char_width(c, width, tab_width);
    }
    width
}

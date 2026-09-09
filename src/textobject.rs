//! Text objects: the `iw`/`a(` half of the grammar, which select a region
//! around the cursor rather than a span from it.

use crate::motion::EditRange;
use crate::text::{self, char_class, CharClass, Position};
use ropey::Rope;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextObject {
    Word { big: bool },
    Quoted { ch: char },
    Delimited { open: char, close: char },
    Paragraph,
}

/// `iw` selects just the object; `aw` includes its surrounding whitespace or
/// delimiters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    Inner,
    Around,
}

pub fn resolve(
    rope: &Rope,
    cursor: Position,
    object: TextObject,
    scope: Scope,
    count: usize,
) -> Option<EditRange> {
    match object {
        TextObject::Word { big } => word(rope, cursor, scope, big, count.max(1)),
        TextObject::Quoted { ch } => quoted(rope, cursor, ch, scope),
        TextObject::Delimited { open, close } => delimited(rope, cursor, open, close, scope),
        TextObject::Paragraph => paragraph(rope, cursor, scope),
    }
}

fn classify(c: char, big: bool) -> CharClass {
    if big {
        text::big_char_class(c)
    } else {
        char_class(c)
    }
}

fn word(rope: &Rope, cursor: Position, scope: Scope, big: bool, count: usize) -> Option<EditRange> {
    let line_start = rope.line_to_char(cursor.line);
    let l = text::line(rope, cursor.line);
    let chars: Vec<char> = l.chars().collect();
    if chars.is_empty() {
        return Some(EditRange {
            start: line_start,
            end: line_start,
            linewise: false,
        });
    }
    let col = cursor.col.min(chars.len() - 1);
    let class = classify(chars[col], big);

    let mut start = col;
    while start > 0 && classify(chars[start - 1], big) == class {
        start -= 1;
    }
    let mut end = col;
    while end + 1 < chars.len() && classify(chars[end + 1], big) == class {
        end += 1;
    }

    // A count extends over that many further words on the same line.
    for _ in 1..count {
        let mut next = end + 1;
        if next >= chars.len() {
            break;
        }
        let next_class = classify(chars[next], big);
        while next + 1 < chars.len() && classify(chars[next + 1], big) == next_class {
            next += 1;
        }
        end = next;
    }

    if scope == Scope::Around {
        // Prefer trailing whitespace, fall back to leading, as Vim does.
        let mut extended = end;
        while extended + 1 < chars.len() && chars[extended + 1].is_whitespace() {
            extended += 1;
        }
        if extended == end && class != CharClass::Whitespace {
            while start > 0 && chars[start - 1].is_whitespace() {
                start -= 1;
            }
        }
        end = extended;
    }

    Some(EditRange {
        start: line_start + start,
        end: line_start + end + 1,
        linewise: false,
    })
}

fn quoted(rope: &Rope, cursor: Position, quote: char, scope: Scope) -> Option<EditRange> {
    let line_start = rope.line_to_char(cursor.line);
    let chars: Vec<char> = text::line(rope, cursor.line).chars().collect();

    // Pair quotes from the start of the line so that the cursor sitting on a
    // closing quote still resolves to the region it closes.
    let mut pairs = Vec::new();
    let mut open: Option<usize> = None;
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '\\' {
            i += 2;
            continue;
        }
        if chars[i] == quote {
            match open.take() {
                Some(start) => pairs.push((start, i)),
                None => open = Some(i),
            }
        }
        i += 1;
    }

    let (start, end) = pairs
        .iter()
        .copied()
        .find(|&(s, e)| cursor.col >= s && cursor.col <= e)
        .or_else(|| pairs.iter().copied().find(|&(s, _)| s >= cursor.col))?;

    Some(match scope {
        Scope::Inner => EditRange {
            start: line_start + start + 1,
            end: line_start + end,
            linewise: false,
        },
        Scope::Around => EditRange {
            start: line_start + start,
            end: line_start + end + 1,
            linewise: false,
        },
    })
}

/// The innermost `open`/`close` pair containing the cursor, across lines.
fn enclosing(rope: &Rope, at: usize, open: char, close: char) -> Option<(usize, usize)> {
    let len = rope.len_chars();
    if len == 0 {
        return None;
    }
    let at = at.min(len - 1);

    let mut depth = 0i32;
    let mut i = at;
    let open_idx = loop {
        let c = rope.char(i);
        if i != at && c == close {
            depth += 1;
        } else if c == open {
            if depth == 0 {
                break i;
            }
            depth -= 1;
        }
        if i == 0 {
            return None;
        }
        i -= 1;
    };

    let mut depth = 0i32;
    for j in open_idx..len {
        let c = rope.char(j);
        if c == open {
            depth += 1;
        } else if c == close {
            depth -= 1;
            if depth == 0 {
                return Some((open_idx, j));
            }
        }
    }
    None
}

fn delimited(
    rope: &Rope,
    cursor: Position,
    open: char,
    close: char,
    scope: Scope,
) -> Option<EditRange> {
    let at = text::pos_to_char(rope, cursor);
    let (start, end) = enclosing(rope, at, open, close)?;
    Some(match scope {
        Scope::Inner => EditRange {
            start: start + 1,
            end,
            linewise: false,
        },
        Scope::Around => EditRange {
            start,
            end: end + 1,
            linewise: false,
        },
    })
}

fn paragraph(rope: &Rope, cursor: Position, scope: Scope) -> Option<EditRange> {
    let last = text::line_count(rope).saturating_sub(1);
    let blank = text::is_blank_line(rope, cursor.line);

    let mut first = cursor.line;
    while first > 0 && text::is_blank_line(rope, first - 1) == blank {
        first -= 1;
    }
    let mut end = cursor.line;
    while end < last && text::is_blank_line(rope, end + 1) == blank {
        end += 1;
    }

    if scope == Scope::Around {
        // Include the run of lines of the opposite kind that follows.
        while end < last && text::is_blank_line(rope, end + 1) != blank {
            end += 1;
        }
    }

    let start_char = rope.line_to_char(first);
    let end_char = if end >= last {
        rope.len_chars()
    } else {
        rope.line_to_char(end + 1)
    };
    Some(EditRange {
        start: start_char,
        end: end_char,
        linewise: true,
    })
}

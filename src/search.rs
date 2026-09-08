//! Pattern search.
//!
//! Patterns are Rust regular expressions rather than Vim's dialect: `\(`-style
//! grouping and `\v` magic are not translated. Matching is per line, so a
//! pattern cannot span a newline.

use crate::text::{self, Position};
use regex::{Regex, RegexBuilder};
use ropey::Rope;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Direction {
    #[default]
    Forward,
    Backward,
}

impl Direction {
    pub fn reversed(self) -> Self {
        match self {
            Direction::Forward => Direction::Backward,
            Direction::Backward => Direction::Forward,
        }
    }
}

/// A match's line and the character range it covers within that line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Match {
    pub line: usize,
    pub start: usize,
    pub end: usize,
}

impl Match {
    pub fn position(&self) -> Position {
        Position::new(self.line, self.start)
    }
}

#[derive(Default)]
pub struct Search {
    pub pattern: String,
    pub direction: Direction,
    regex: Option<Regex>,
}

impl Search {
    /// Compile `pattern`. `smart_case` upgrades a lowercase-only pattern to
    /// case-insensitive, matching Vim's `smartcase`.
    pub fn set_pattern(
        &mut self,
        pattern: &str,
        direction: Direction,
        ignore_case: bool,
        smart_case: bool,
    ) -> Result<(), regex::Error> {
        let insensitive = ignore_case && !(smart_case && pattern.chars().any(|c| c.is_uppercase()));
        let regex = RegexBuilder::new(pattern)
            .case_insensitive(insensitive)
            .build()?;
        self.pattern = pattern.to_string();
        self.direction = direction;
        self.regex = Some(regex);
        Ok(())
    }

    pub fn is_active(&self) -> bool {
        self.regex.is_some()
    }

    pub fn clear(&mut self) {
        self.regex = None;
        self.pattern.clear();
    }

    /// All matches on one line, for highlighting the viewport.
    pub fn matches_on_line(&self, rope: &Rope, line: usize) -> Vec<Match> {
        let Some(regex) = &self.regex else {
            return Vec::new();
        };
        matches_on_line_with(regex, rope, line)
    }

    /// The next match strictly after (or before) `from`.
    pub fn find(
        &self,
        rope: &Rope,
        from: Position,
        direction: Direction,
        wrap: bool,
    ) -> Option<Match> {
        let regex = self.regex.as_ref()?;
        find_with(regex, rope, from, direction, wrap)
    }
}

pub fn matches_on_line_with(regex: &Regex, rope: &Rope, line_idx: usize) -> Vec<Match> {
    if line_idx >= text::line_count(rope) {
        return Vec::new();
    }
    let content: String = text::line(rope, line_idx).chars().collect();
    let mut out = Vec::new();
    for m in regex.find_iter(&content) {
        // Empty matches would make `n` spin in place.
        if m.start() == m.end() {
            continue;
        }
        let start = content[..m.start()].chars().count();
        let end = start + content[m.start()..m.end()].chars().count();
        out.push(Match {
            line: line_idx,
            start,
            end,
        });
    }
    out
}

pub fn find_with(
    regex: &Regex,
    rope: &Rope,
    from: Position,
    direction: Direction,
    wrap: bool,
) -> Option<Match> {
    let total = text::line_count(rope);
    if total == 0 {
        return None;
    }

    // Scan the cursor's own line first (only the part beyond the cursor),
    // then whole lines outward, optionally wrapping around the file.
    let mut order: Vec<usize> = Vec::with_capacity(total);
    match direction {
        Direction::Forward => {
            order.extend(from.line..total);
            if wrap {
                order.extend(0..=from.line.min(total - 1));
            }
        }
        Direction::Backward => {
            order.extend((0..=from.line.min(total - 1)).rev());
            if wrap {
                order.extend((from.line.min(total - 1)..total).rev());
            }
        }
    }

    for (visited, line) in order.iter().copied().enumerate() {
        let matches = matches_on_line_with(regex, rope, line);
        if matches.is_empty() {
            continue;
        }
        let first_pass = visited == 0;
        match direction {
            Direction::Forward => {
                let candidate = matches
                    .iter()
                    .find(|m| !first_pass || m.start > from.col)
                    .copied();
                if let Some(m) = candidate {
                    return Some(m);
                }
            }
            Direction::Backward => {
                let candidate = matches
                    .iter()
                    .rev()
                    .find(|m| !first_pass || m.start < from.col)
                    .copied();
                if let Some(m) = candidate {
                    return Some(m);
                }
            }
        }
    }
    None
}

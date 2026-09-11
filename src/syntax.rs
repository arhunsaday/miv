//! Syntax highlighting.
//!
//! syntect is a stateful line-by-line parser: highlighting line *n* requires
//! the parser state left behind by line *n-1*. Rather than re-parse the file
//! every frame, we keep periodic state checkpoints and resume from the nearest
//! one, so a redraw costs a bounded number of line parses no matter how far
//! into the file the viewport is.

use crate::core::text;
use ratatui::style::{Color as TuiColor, Modifier, Style as TuiStyle};
use ropey::Rope;
use std::path::Path;
use syntect::highlighting::{
    FontStyle, HighlightState, Highlighter as SyntectHighlighter, RangedHighlightIterator, Style,
    Theme, ThemeSet,
};
use syntect::parsing::{ParseState, ScopeStack, SyntaxReference, SyntaxSet};

/// Lines between parser state checkpoints. Small enough that resuming is
/// cheap, large enough that the checkpoint list stays small.
const CHECKPOINT_STRIDE: usize = 64;

/// How many checkpoints the parser will walk forward through to reach the
/// viewport in a single frame.
///
/// syntect needs the state left by the previous line, so reaching line 28,000
/// from a cold cache means parsing 28,000 lines — a visible stall on every
/// `G`. Past this bound we resume from a fresh parser state at the viewport
/// instead. Colours can then be wrong for a construct opened far above (a long
/// block comment, say), and scrolling through the gap repairs them, which is a
/// much better trade than freezing on every jump.
const MAX_CATCHUP_CHECKPOINTS: usize = 24;

pub struct SyntaxEngine {
    syntax_set: SyntaxSet,
    theme_set: ThemeSet,
}

impl SyntaxEngine {
    pub fn new() -> Self {
        Self {
            syntax_set: SyntaxSet::load_defaults_newlines(),
            theme_set: ThemeSet::load_defaults(),
        }
    }

    pub fn theme(&self, name: &str) -> Option<&Theme> {
        self.theme_set.themes.get(name)
    }

    pub fn theme_names(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.theme_set.themes.keys().map(String::as_str).collect();
        names.sort_unstable();
        names
    }

    /// Resolve a syntax from the file name, falling back to the shebang or
    /// first line, then to plain text.
    pub fn detect(&self, path: Option<&Path>, first_line: &str) -> &SyntaxReference {
        if let Some(path) = path {
            if let Ok(Some(syntax)) = self.syntax_set.find_syntax_for_file(path) {
                return syntax;
            }
            if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                if let Some(syntax) = self.syntax_set.find_syntax_by_extension(ext) {
                    return syntax;
                }
            }
        }
        self.syntax_set
            .find_syntax_by_first_line(first_line)
            .unwrap_or_else(|| self.syntax_set.find_syntax_plain_text())
    }

    pub fn syntax_by_name(&self, name: &str) -> Option<&SyntaxReference> {
        self.syntax_set.find_syntax_by_name(name)
    }
}

/// One highlighted span: a style and the character range it covers within the
/// line.
#[derive(Clone, Debug)]
pub struct Span {
    pub style: TuiStyle,
    pub start: usize,
    pub end: usize,
}

type ParserState = (ParseState, HighlightState);

#[derive(Default)]
pub struct SyntaxCache {
    /// `checkpoints[k]`, when present, is the parser state entering line
    /// `k * CHECKPOINT_STRIDE`.
    checkpoints: Vec<Option<ParserState>>,
    /// Set when a file proves too large or too pathological to highlight.
    disabled: bool,
}

impl SyntaxCache {
    /// Drop cached state at or after `line`, which an edit has invalidated.
    pub fn invalidate_from(&mut self, line: usize) {
        self.checkpoints.truncate(line / CHECKPOINT_STRIDE);
    }

    pub fn clear(&mut self) {
        self.checkpoints.clear();
        self.disabled = false;
    }

    pub fn disable(&mut self) {
        self.disabled = true;
    }

    /// Record the state entering a checkpoint line. Always overwrites: a state
    /// reached by walking is at least as trustworthy as whatever was there.
    fn store(&mut self, index: usize, state: ParserState) {
        if self.checkpoints.len() <= index {
            self.checkpoints.resize(index + 1, None);
        }
        self.checkpoints[index] = Some(state);
    }

    /// The newest checkpoint at or before `index` that is close enough to walk
    /// forward from.
    fn resume_point(&self, index: usize) -> Option<(usize, ParserState)> {
        let floor = index.saturating_sub(MAX_CATCHUP_CHECKPOINTS);
        (floor..=index).rev().find_map(|k| {
            self.checkpoints
                .get(k)
                .and_then(|slot| slot.clone())
                .map(|state| (k, state))
        })
    }

    /// Highlight lines `first..=last`, returning one span list per line.
    /// Returns an empty vector when highlighting is off.
    pub fn highlight(
        &mut self,
        rope: &Rope,
        engine: &SyntaxEngine,
        syntax: &SyntaxReference,
        theme: &Theme,
        first: usize,
        last: usize,
    ) -> Vec<Vec<Span>> {
        if self.disabled {
            return Vec::new();
        }
        let highlighter = SyntectHighlighter::new(theme);
        let last = last.min(text::line_count(rope).saturating_sub(1));
        let target = first / CHECKPOINT_STRIDE;

        let (mut line_idx, mut parse_state, mut highlight_state) = match self.resume_point(target) {
            Some((index, (parse_state, highlight_state))) => {
                (index * CHECKPOINT_STRIDE, parse_state, highlight_state)
            }
            None => (
                target * CHECKPOINT_STRIDE,
                ParseState::new(syntax),
                HighlightState::new(&highlighter, ScopeStack::new()),
            ),
        };

        let mut out = Vec::with_capacity(last.saturating_sub(first) + 1);
        while line_idx <= last {
            if line_idx % CHECKPOINT_STRIDE == 0 {
                self.store(
                    line_idx / CHECKPOINT_STRIDE,
                    (parse_state.clone(), highlight_state.clone()),
                );
            }

            // syntect wants a `&str` per line, including its newline.
            let line: String = rope
                .line(line_idx.min(rope.len_lines() - 1))
                .chars()
                .collect();
            let ops = match parse_state.parse_line(&line, &engine.syntax_set) {
                Ok(ops) => ops,
                Err(_) => {
                    // A pathological line disables highlighting rather than
                    // freezing the editor.
                    self.disabled = true;
                    return Vec::new();
                }
            };

            let iter =
                RangedHighlightIterator::new(&mut highlight_state, &ops, &line, &highlighter);
            if line_idx >= first {
                let mut spans = Vec::new();
                let mut char_start = 0usize;
                let mut byte_cursor = 0usize;
                for (style, piece, range) in iter {
                    // Convert syntect's byte ranges to character offsets.
                    debug_assert!(range.start >= byte_cursor);
                    char_start += line[byte_cursor..range.start].chars().count();
                    let len = piece.chars().count();
                    if len > 0 {
                        spans.push(Span {
                            style: to_tui_style(style),
                            start: char_start,
                            end: char_start + len,
                        });
                    }
                    char_start += len;
                    byte_cursor = range.end;
                }
                out.push(spans);
            } else {
                // Skipped lines still have to advance the highlight state.
                let _ = iter.count();
            }
            line_idx += 1;
        }
        out
    }
}

fn to_tui_style(style: Style) -> TuiStyle {
    let mut out = TuiStyle::default().fg(to_tui_color(style.foreground));
    if style.font_style.contains(FontStyle::BOLD) {
        out = out.add_modifier(Modifier::BOLD);
    }
    if style.font_style.contains(FontStyle::ITALIC) {
        out = out.add_modifier(Modifier::ITALIC);
    }
    if style.font_style.contains(FontStyle::UNDERLINE) {
        out = out.add_modifier(Modifier::UNDERLINED);
    }
    out
}

pub fn to_tui_color(color: syntect::highlighting::Color) -> TuiColor {
    if color.a == 0 {
        TuiColor::Reset
    } else {
        TuiColor::Rgb(color.r, color.g, color.b)
    }
}

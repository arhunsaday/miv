//! The picker: a filterable list over anything.
//!
//! One widget serves the file finder, the buffer list, the command palette and
//! grep results, because they differ only in where the items come from and
//! what selecting one does.

use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};
use std::path::PathBuf;

/// Where a picker's items come from. Results arrive asynchronously, so a
/// picker only accepts the kind it asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Files,
    Buffers,
    Commands,
    Grep,
    Diagnostics,
}

/// What selecting an item does.
#[derive(Clone, Debug)]
pub enum Action {
    OpenFile(PathBuf),
    /// Jump to a place, opening the file first if it is not already open.
    Goto {
        path: Option<PathBuf>,
        line: usize,
        col: usize,
    },
    /// Run an ex command.
    Ex(String),
    /// Open the command line prefilled, for commands that need an argument.
    Prompt(String),
    Buffer(usize),
}

#[derive(Clone, Debug)]
pub struct Item {
    /// Matched against, and shown on the left.
    pub label: String,
    /// Shown dimmed on the right; not matched against.
    pub detail: String,
    pub action: Action,
}

impl Item {
    pub fn new(label: impl Into<String>, detail: impl Into<String>, action: Action) -> Self {
        Self {
            label: label.into(),
            detail: detail.into(),
            action,
        }
    }
}

pub struct Picker {
    pub title: String,
    pub source: Source,
    pub input: String,
    /// Caret position in the input, in characters.
    pub caret: usize,
    items: Vec<Item>,
    /// Indices into `items`, best match first.
    matches: Vec<usize>,
    pub selected: usize,
    /// A background source has not delivered yet.
    pub loading: bool,
    matcher: Matcher,
    scratch: Vec<char>,
}

impl Picker {
    pub fn new(title: impl Into<String>, source: Source) -> Self {
        Self {
            title: title.into(),
            source,
            input: String::new(),
            caret: 0,
            items: Vec::new(),
            matches: Vec::new(),
            selected: 0,
            loading: false,
            // Path-aware scoring, which is what makes typing `src/mn` find
            // `src/main.rs` ahead of an unrelated file that happens to match.
            matcher: Matcher::new(Config::DEFAULT.match_paths()),
            scratch: Vec::new(),
        }
    }

    pub fn with_items(title: impl Into<String>, source: Source, items: Vec<Item>) -> Self {
        let mut picker = Self::new(title, source);
        picker.set_items(items);
        picker
    }

    pub fn set_items(&mut self, items: Vec<Item>) {
        self.items = items;
        self.loading = false;
        self.selected = 0;
        self.refilter();
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn total(&self) -> usize {
        self.items.len()
    }

    pub fn matches(&self) -> &[usize] {
        &self.matches
    }

    pub fn item(&self, index: usize) -> Option<&Item> {
        self.items.get(index)
    }

    pub fn selection(&self) -> Option<&Item> {
        self.matches
            .get(self.selected)
            .and_then(|index| self.items.get(*index))
    }

    pub fn move_selection(&mut self, delta: isize) {
        if self.matches.is_empty() {
            self.selected = 0;
            return;
        }
        let count = self.matches.len() as isize;
        // Wrapping is what people expect from a list this short.
        let next = (self.selected as isize + delta).rem_euclid(count);
        self.selected = next as usize;
    }

    pub fn insert(&mut self, c: char) {
        let byte = byte_offset(&self.input, self.caret);
        self.input.insert(byte, c);
        self.caret += 1;
        self.refilter();
    }

    pub fn insert_str(&mut self, text: &str) {
        let byte = byte_offset(&self.input, self.caret);
        self.input.insert_str(byte, text);
        self.caret += text.chars().count();
        self.refilter();
    }

    /// Returns false when there was nothing to delete, which the caller treats
    /// as a request to close.
    pub fn backspace(&mut self) -> bool {
        if self.caret == 0 {
            return false;
        }
        let byte = byte_offset(&self.input, self.caret - 1);
        self.input.remove(byte);
        self.caret -= 1;
        self.refilter();
        true
    }

    pub fn delete_word(&mut self) {
        let upto = byte_offset(&self.input, self.caret);
        let head = &self.input[..upto];
        let trimmed = head.trim_end();
        let cut = trimmed
            .rfind(char::is_whitespace)
            .map(|i| i + 1)
            .unwrap_or(0);
        let removed = head[cut..].chars().count();
        self.input.replace_range(cut..upto, "");
        self.caret -= removed;
        self.refilter();
    }

    pub fn clear_input(&mut self) {
        self.input.clear();
        self.caret = 0;
        self.refilter();
    }

    pub fn move_caret(&mut self, delta: isize) {
        let length = self.input.chars().count() as isize;
        self.caret = (self.caret as isize + delta).clamp(0, length) as usize;
    }

    fn refilter(&mut self) {
        if self.input.is_empty() {
            self.matches = (0..self.items.len()).collect();
        } else {
            let pattern = Pattern::parse(&self.input, CaseMatching::Smart, Normalization::Smart);
            // Disjoint field borrows: the items are read while the matcher,
            // which needs to be mutable, is a separate field.
            let items = &self.items;
            let matcher = &mut self.matcher;
            let scratch = &mut self.scratch;
            let mut scored: Vec<(usize, u32)> = items
                .iter()
                .enumerate()
                .filter_map(|(index, item)| {
                    scratch.clear();
                    let haystack = Utf32Str::new(&item.label, scratch);
                    pattern.score(haystack, matcher).map(|score| (index, score))
                })
                .collect();
            // Higher score first; ties keep the source order so the list does
            // not shuffle as you type.
            scored.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
            self.matches = scored.into_iter().map(|(index, _)| index).collect();
        }
        if self.selected >= self.matches.len() {
            self.selected = self.matches.len().saturating_sub(1);
        }
    }

    /// The window of matches to draw, and where the selection sits in it.
    pub fn visible(&self, height: usize) -> (&[usize], usize) {
        if height == 0 || self.matches.is_empty() {
            return (&[], 0);
        }
        let half = height / 2;
        let start = self.selected.saturating_sub(half).min(
            self.matches
                .len()
                .saturating_sub(height.min(self.matches.len())),
        );
        let end = (start + height).min(self.matches.len());
        (&self.matches[start..end], self.selected - start)
    }
}

fn byte_offset(text: &str, chars: usize) -> usize {
    text.char_indices()
        .nth(chars)
        .map(|(byte, _)| byte)
        .unwrap_or(text.len())
}

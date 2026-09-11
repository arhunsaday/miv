//! Completion from what is already in front of you.
//!
//! No language server: candidates are the words in your open buffers plus the
//! keywords of the language you are in. That covers the common case — the
//! identifier you defined four lines up — without any setup.

use crate::core::text::{self, Position};
use ropey::Rope;
use std::collections::HashSet;

/// Shortest prefix worth completing.
pub const MIN_PREFIX: usize = 1;
/// Most candidates to offer. A longer list is not more useful.
const MAX_ITEMS: usize = 200;
/// Words shorter than this are not worth suggesting.
const MIN_WORD: usize = 3;

pub struct Completion {
    pub items: Vec<String>,
    pub selected: usize,
    /// Where the word being completed starts, so accepting can replace it.
    pub start: Position,
    pub prefix: String,
}

impl Completion {
    pub fn selection(&self) -> Option<&str> {
        self.items.get(self.selected).map(String::as_str)
    }

    pub fn move_selection(&mut self, delta: isize) {
        if self.items.is_empty() {
            return;
        }
        let count = self.items.len() as isize;
        self.selected = ((self.selected as isize + delta).rem_euclid(count)) as usize;
    }
}

/// The word immediately before the cursor, and where it starts.
pub fn prefix_at(rope: &Rope, cursor: Position) -> (String, Position) {
    let line = text::line(rope, cursor.line);
    let chars: Vec<char> = line.chars().collect();
    let mut start = cursor.col.min(chars.len());
    while start > 0 && is_word(chars[start - 1]) {
        start -= 1;
    }
    let prefix: String = chars[start..cursor.col.min(chars.len())].iter().collect();
    (prefix, Position::new(cursor.line, start))
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Candidates for `prefix`, nearest first.
///
/// "Nearest" means: words from the buffer you are in, ordered by how close
/// they are to the cursor, then words from other buffers, then the language's
/// keywords. Proximity matters more than cleverness here — the identifier you
/// want is usually the one you just typed.
pub fn candidates(
    ropes: &[(&Rope, bool)],
    cursor_line: usize,
    syntax_name: &str,
    prefix: &str,
) -> Vec<String> {
    if prefix.chars().count() < MIN_PREFIX {
        return Vec::new();
    }
    let lowered = prefix.to_lowercase();
    let mut seen: HashSet<String> = HashSet::new();
    seen.insert(prefix.to_string());
    let mut scored: Vec<(usize, String)> = Vec::new();

    for (rope, is_current) in ropes {
        let total = text::line_count(rope);
        for line in 0..total {
            let distance = if *is_current {
                line.abs_diff(cursor_line)
            } else {
                // Other buffers sort after everything in this one.
                total + line
            };
            for word in words_in(&text::line(rope, line).chars().collect::<String>()) {
                if word.len() < MIN_WORD || !word.to_lowercase().starts_with(&lowered) {
                    continue;
                }
                if seen.insert(word.clone()) {
                    scored.push((distance, word));
                }
            }
        }
    }

    scored.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    let mut items: Vec<String> = scored.into_iter().map(|(_, word)| word).collect();

    for keyword in keywords(syntax_name) {
        if keyword.to_lowercase().starts_with(&lowered) && seen.insert(keyword.to_string()) {
            items.push(keyword.to_string());
        }
    }

    items.truncate(MAX_ITEMS);
    items
}

fn words_in(line: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    for c in line.chars() {
        if is_word(c) {
            current.push(c);
        } else if !current.is_empty() {
            words.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        words.push(current);
    }
    // A word cannot start with a digit, or every number becomes a candidate.
    words.retain(|word| !word.starts_with(|c: char| c.is_ascii_digit()));
    words
}

/// Keywords per language, matched loosely against the detected syntax name.
pub fn keywords(syntax_name: &str) -> &'static [&'static str] {
    let name = syntax_name.to_lowercase();
    let has = |needle: &str| name.contains(needle);

    if has("rust") {
        &[
            "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum",
            "extern", "false", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod",
            "move", "mut", "pub", "ref", "return", "self", "Self", "static", "struct", "super",
            "trait", "true", "type", "unsafe", "use", "where", "while", "Option", "Result", "Some",
            "None", "Ok", "Err", "String", "Vec", "HashMap", "derive", "println",
        ]
    } else if has("go") {
        &[
            "break",
            "case",
            "chan",
            "const",
            "continue",
            "default",
            "defer",
            "else",
            "fallthrough",
            "for",
            "func",
            "go",
            "goto",
            "if",
            "import",
            "interface",
            "map",
            "package",
            "range",
            "return",
            "select",
            "struct",
            "switch",
            "type",
            "var",
            "error",
            "nil",
            "true",
            "false",
            "make",
            "append",
            "len",
            "cap",
            "string",
            "int",
            "bool",
            "byte",
            "rune",
        ]
    } else if has("python") {
        &[
            "and", "as", "assert", "async", "await", "break", "class", "continue", "def", "del",
            "elif", "else", "except", "finally", "for", "from", "global", "if", "import", "in",
            "is", "lambda", "None", "nonlocal", "not", "or", "pass", "raise", "return", "True",
            "False", "try", "while", "with", "yield", "self", "print", "len", "range", "dict",
            "list", "set", "str", "int", "float", "bool",
        ]
    } else if has("typescript") || has("javascript") {
        &[
            "async",
            "await",
            "break",
            "case",
            "catch",
            "class",
            "const",
            "continue",
            "default",
            "delete",
            "do",
            "else",
            "export",
            "extends",
            "false",
            "finally",
            "for",
            "function",
            "if",
            "import",
            "in",
            "instanceof",
            "interface",
            "let",
            "new",
            "null",
            "return",
            "super",
            "switch",
            "this",
            "throw",
            "true",
            "try",
            "type",
            "typeof",
            "undefined",
            "var",
            "void",
            "while",
            "yield",
            "console",
            "Promise",
            "string",
            "number",
            "boolean",
        ]
    } else if has("shell") || has("bash") {
        &[
            "case", "do", "done", "elif", "else", "esac", "exit", "export", "fi", "for",
            "function", "if", "in", "local", "readonly", "return", "shift", "then", "until",
            "while", "echo", "printf", "source", "unset", "trap", "set",
        ]
    } else if has("terraform") || has("hcl") {
        &[
            "count",
            "data",
            "depends_on",
            "dynamic",
            "for_each",
            "lifecycle",
            "locals",
            "module",
            "output",
            "provider",
            "provisioner",
            "resource",
            "terraform",
            "variable",
            "default",
            "description",
            "type",
            "value",
            "source",
            "version",
        ]
    } else if has("yaml") {
        &["true", "false", "null"]
    } else if has("c++") || has("c#") || has("java") || has("objective-c") || name == "c" {
        &[
            "auto",
            "bool",
            "break",
            "case",
            "catch",
            "char",
            "class",
            "const",
            "continue",
            "default",
            "delete",
            "do",
            "double",
            "else",
            "enum",
            "extern",
            "false",
            "float",
            "for",
            "if",
            "inline",
            "int",
            "long",
            "namespace",
            "new",
            "nullptr",
            "private",
            "protected",
            "public",
            "return",
            "short",
            "signed",
            "sizeof",
            "static",
            "struct",
            "switch",
            "template",
            "this",
            "throw",
            "true",
            "try",
            "typedef",
            "typename",
            "union",
            "unsigned",
            "using",
            "virtual",
            "void",
            "while",
        ]
    } else {
        &[]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_prefix_is_the_word_before_the_cursor() {
        let rope = Rope::from_str("let value = comp\n");
        let (prefix, start) = prefix_at(&rope, Position::new(0, 16));
        assert_eq!(prefix, "comp");
        assert_eq!(start, Position::new(0, 12));

        // Nothing to complete after a space.
        let (prefix, _) = prefix_at(&rope, Position::new(0, 12));
        assert_eq!(prefix, "");
    }

    #[test]
    fn candidates_come_from_the_buffer() {
        let rope = Rope::from_str("let calculation = 1;\nlet calendar = 2;\nlet ca\n");
        let items = candidates(&[(&rope, true)], 2, "Rust", "cal");
        assert!(items.contains(&"calculation".to_string()));
        assert!(items.contains(&"calendar".to_string()));
        assert!(!items.contains(&"cal".to_string()), "not the prefix itself");
    }

    #[test]
    fn nearer_words_come_first() {
        let rope = Rope::from_str("faraway_thing\n\n\n\n\nnearby_thing\nna\n");
        let items = candidates(&[(&rope, true)], 6, "Plain Text", "n");
        assert_eq!(items.first().map(String::as_str), Some("nearby_thing"));
    }

    #[test]
    fn keywords_are_offered_for_the_language() {
        let rope = Rope::from_str("fn\n");
        let items = candidates(&[(&rope, true)], 0, "Rust", "imp");
        assert!(items.contains(&"impl".to_string()), "{items:?}");

        let go = candidates(&[(&rope, true)], 0, "Go", "fun");
        assert!(go.contains(&"func".to_string()), "{go:?}");

        // An unknown language still completes from the buffer.
        let plain = candidates(&[(&rope, true)], 0, "Plain Text", "imp");
        assert!(plain.is_empty());
    }

    #[test]
    fn numbers_and_very_short_words_are_not_offered() {
        let rope = Rope::from_str("let x1 = 123456; let ab = 1; let abcd = 2;\n");
        let items = candidates(&[(&rope, true)], 0, "Rust", "a");
        assert!(items.contains(&"abcd".to_string()));
        assert!(!items.contains(&"ab".to_string()), "too short");
        let numbers = candidates(&[(&rope, true)], 0, "Rust", "1");
        assert!(numbers.is_empty(), "{numbers:?}");
    }

    #[test]
    fn other_buffers_contribute_after_this_one() {
        let current = Rope::from_str("alpha_here\n");
        let other = Rope::from_str("alpha_there\n");
        let items = candidates(&[(&current, true), (&other, false)], 0, "Rust", "alpha");
        assert_eq!(
            items,
            vec!["alpha_here".to_string(), "alpha_there".to_string()]
        );
    }
}

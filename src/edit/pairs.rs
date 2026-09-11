//! Auto-pairs.
//!
//! The rules are deliberately conservative: a bracket only closes itself when
//! what follows it is whitespace, the end of the line, or another closing
//! bracket. Closing in the middle of a word is the behaviour that makes people
//! turn auto-pairs off.

use crate::core::text::{self, Position};
use ropey::Rope;

/// Pairs where the two characters differ.
const BRACKETS: [(char, char); 4] = [('(', ')'), ('[', ']'), ('{', '}'), ('<', '>')];
/// Pairs where they are the same, which need the extra care.
const QUOTES: [char; 3] = ['"', '\'', '`'];

pub fn closing_for(open: char) -> Option<char> {
    BRACKETS
        .iter()
        .find(|(candidate, _)| *candidate == open)
        .map(|(_, close)| *close)
}

fn is_closing(c: char) -> bool {
    BRACKETS.iter().any(|(_, close)| *close == c)
}

/// What typing a character in insert mode should do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Insertion {
    /// Insert it and nothing else.
    Plain,
    /// Insert the character plus its partner, leaving the cursor between them.
    Surround(char),
    /// The partner is already here, so move past it instead of adding another.
    StepOver,
}

pub fn on_insert(rope: &Rope, cursor: Position, typed: char) -> Insertion {
    let next = text::char_at(rope, cursor);
    let previous = (cursor.col > 0)
        .then(|| text::char_at(rope, Position::new(cursor.line, cursor.col - 1)))
        .flatten();

    // Typing the closing half of a pair that is already there: step over it,
    // so finishing a call reads as typing `)` rather than deleting one.
    if is_closing(typed) && next == Some(typed) {
        return Insertion::StepOver;
    }

    if QUOTES.contains(&typed) {
        if next == Some(typed) {
            return Insertion::StepOver;
        }
        // An apostrophe inside a word is an apostrophe, not an opening quote.
        let in_word = previous
            .map(|c| c.is_alphanumeric() || c == '_')
            .unwrap_or(false);
        if in_word {
            return Insertion::Plain;
        }
        // Escaped, or already doubled: leave it alone.
        if previous == Some('\\') || previous == Some(typed) {
            return Insertion::Plain;
        }
        return if opens_here(next) {
            Insertion::Surround(typed)
        } else {
            Insertion::Plain
        };
    }

    match closing_for(typed) {
        // `<` only pairs where it is plausibly a generic or a tag, never after
        // a space, or every `a < b` would gain a `>`.
        Some('>') => Insertion::Plain,
        Some(close) if opens_here(next) => Insertion::Surround(close),
        _ => Insertion::Plain,
    }
}

/// Whether a pair may be opened in front of this character.
fn opens_here(next: Option<char>) -> bool {
    match next {
        None => true,
        Some(c) => {
            c.is_whitespace() || is_closing(c) || QUOTES.contains(&c) || c == ',' || c == ';'
        }
    }
}

/// Whether backspace should take both halves of an empty pair.
pub fn deletes_pair(rope: &Rope, cursor: Position) -> bool {
    if cursor.col == 0 {
        return false;
    }
    let Some(before) = text::char_at(rope, Position::new(cursor.line, cursor.col - 1)) else {
        return false;
    };
    let Some(after) = text::char_at(rope, cursor) else {
        return false;
    };
    if QUOTES.contains(&before) && before == after {
        return true;
    }
    closing_for(before) == Some(after)
}

/// Whether Enter is being pressed between a bracket and its partner, which
/// should open an indented line and leave the closer on its own line.
pub fn splits_block(rope: &Rope, cursor: Position) -> bool {
    if cursor.col == 0 {
        return false;
    }
    let Some(before) = text::char_at(rope, Position::new(cursor.line, cursor.col - 1)) else {
        return false;
    };
    let Some(after) = text::char_at(rope, cursor) else {
        return false;
    };
    closing_for(before) == Some(after)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str, col: usize) -> (Rope, Position) {
        (Rope::from_str(text), Position::new(0, col))
    }

    #[test]
    fn a_bracket_closes_itself_at_the_end_of_a_line() {
        let (rope, cursor) = at("call\n", 4);
        assert_eq!(on_insert(&rope, cursor, '('), Insertion::Surround(')'));
        assert_eq!(on_insert(&rope, cursor, '['), Insertion::Surround(']'));
        assert_eq!(on_insert(&rope, cursor, '{'), Insertion::Surround('}'));
    }

    #[test]
    fn a_bracket_does_not_close_itself_inside_a_word() {
        // Typing `(` before `foo` should not produce `()foo`.
        let (rope, cursor) = at("foo\n", 0);
        assert_eq!(on_insert(&rope, cursor, '('), Insertion::Plain);
    }

    #[test]
    fn a_bracket_closes_itself_before_a_closing_bracket_or_separator() {
        let (rope, cursor) = at("f()\n", 2);
        assert_eq!(on_insert(&rope, cursor, '('), Insertion::Surround(')'));
        // Column 3 sits just before the comma.
        let (rope, cursor) = at("f(a, b)\n", 3);
        assert_eq!(on_insert(&rope, cursor, '['), Insertion::Surround(']'));
    }

    #[test]
    fn typing_the_closing_half_steps_over_it() {
        let (rope, cursor) = at("call()\n", 5);
        assert_eq!(on_insert(&rope, cursor, ')'), Insertion::StepOver);
        // ...but a closing bracket with nothing to step over is just typed.
        let (rope, cursor) = at("call\n", 4);
        assert_eq!(on_insert(&rope, cursor, ')'), Insertion::Plain);
    }

    #[test]
    fn an_apostrophe_inside_a_word_is_left_alone() {
        let (rope, cursor) = at("don\n", 3);
        assert_eq!(on_insert(&rope, cursor, '\''), Insertion::Plain);
    }

    #[test]
    fn a_quote_pairs_at_the_start_of_a_value() {
        let (rope, cursor) = at("name: \n", 6);
        assert_eq!(on_insert(&rope, cursor, '"'), Insertion::Surround('"'));
        // And closing one steps over it.
        let (rope, cursor) = at("name: \"\"\n", 7);
        assert_eq!(on_insert(&rope, cursor, '"'), Insertion::StepOver);
    }

    #[test]
    fn an_escaped_quote_is_not_a_pair() {
        let (rope, cursor) = at("\"a\\\n", 3);
        assert_eq!(on_insert(&rope, cursor, '"'), Insertion::Plain);
    }

    #[test]
    fn angle_brackets_never_auto_close() {
        // `a < b` would otherwise gain a `>`.
        let (rope, cursor) = at("a \n", 2);
        assert_eq!(on_insert(&rope, cursor, '<'), Insertion::Plain);
    }

    #[test]
    fn backspace_takes_both_halves_of_an_empty_pair() {
        let (rope, cursor) = at("f()\n", 2);
        assert!(deletes_pair(&rope, cursor));
        let (rope, cursor) = at("f(a)\n", 3);
        assert!(!deletes_pair(&rope, cursor), "not an empty pair");
        let (rope, cursor) = at("\"\"\n", 1);
        assert!(deletes_pair(&rope, cursor));
    }

    #[test]
    fn enter_between_a_pair_opens_a_block() {
        let (rope, cursor) = at("fn f() {}\n", 8);
        assert!(splits_block(&rope, cursor));
        let (rope, cursor) = at("fn f() {x}\n", 8);
        assert!(!splits_block(&rope, cursor));
    }
}

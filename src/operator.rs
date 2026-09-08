//! Operators: the "what to do" half of the grammar, applied to a resolved
//! character range.

use crate::buffer::Buffer;
use crate::motion::EditRange;
use crate::register::{RegisterContent, RegisterKind, Registers};
use crate::text::{self, Position};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operator {
    Delete,
    Change,
    Yank,
    Indent,
    Dedent,
    Lowercase,
    Uppercase,
    ToggleCase,
}

#[derive(Clone, Copy, Debug)]
pub struct EditOptions {
    pub shift_width: usize,
    pub expand_tab: bool,
    pub tab_width: usize,
}

/// What the caller must do after the operator has run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Done,
    EnterInsert,
}

pub fn apply(
    op: Operator,
    buffer: &mut Buffer,
    range: EditRange,
    registers: &mut Registers,
    register: Option<char>,
    options: EditOptions,
) -> Outcome {
    let kind = if range.linewise {
        RegisterKind::Linewise
    } else {
        RegisterKind::Charwise
    };

    match op {
        Operator::Delete => {
            let text_removed = buffer.slice(range.start, range.end);
            registers.delete(
                register,
                RegisterContent {
                    text: text_removed,
                    kind,
                },
            );
            buffer.begin();
            let start_line = buffer.rope.char_to_line(range.start);
            buffer.remove(range.start, range.end);
            buffer.cursor = if range.linewise {
                let line = start_line.min(buffer.line_count().saturating_sub(1));
                Position::new(line, text::first_non_blank(&buffer.rope, line))
            } else {
                text::clamp(
                    &buffer.rope,
                    text::char_to_pos(&buffer.rope, range.start),
                    false,
                )
            };
            buffer.desired_col = buffer.cursor.col;
            buffer.end();
            Outcome::Done
        }
        Operator::Change => {
            let text_removed = buffer.slice(range.start, range.end);
            registers.delete(
                register,
                RegisterContent {
                    text: text_removed,
                    kind,
                },
            );
            buffer.begin();
            if range.linewise {
                // Keep the line and its indentation, clear the content: this
                // is what makes `cc` usable in indented code.
                let start_line = buffer.rope.char_to_line(range.start);
                let indent_end = text::first_non_blank(&buffer.rope, start_line);
                let indent: String = text::line(&buffer.rope, start_line)
                    .chars()
                    .take(indent_end)
                    .collect();
                buffer.remove(range.start, range.end);
                buffer.insert(range.start, &format!("{indent}\n"));
                buffer.cursor = Position::new(start_line, indent.chars().count());
            } else {
                buffer.remove(range.start, range.end);
                buffer.cursor = text::char_to_pos(&buffer.rope, range.start);
            }
            buffer.desired_col = buffer.cursor.col;
            // The transaction stays open so the insert that follows undoes
            // together with the deletion.
            Outcome::EnterInsert
        }
        Operator::Yank => {
            let text_yanked = buffer.slice(range.start, range.end);
            registers.yank(
                register,
                RegisterContent {
                    text: text_yanked,
                    kind,
                },
            );
            let start = text::char_to_pos(&buffer.rope, range.start);
            if range.linewise {
                buffer.cursor = Position::new(start.line, buffer.cursor.col);
            } else if start < buffer.cursor {
                buffer.cursor = start;
            }
            buffer.cursor = text::clamp(&buffer.rope, buffer.cursor, false);
            Outcome::Done
        }
        Operator::Indent | Operator::Dedent => {
            let first = buffer.rope.char_to_line(range.start);
            let last = last_line_of(buffer, &range);
            buffer.begin();
            for line in first..=last {
                if op == Operator::Indent {
                    if text::line_len(&buffer.rope, line) == 0 {
                        continue;
                    }
                    let at = buffer.rope.line_to_char(line);
                    let indent = if options.expand_tab {
                        " ".repeat(options.shift_width)
                    } else {
                        "\t".to_string()
                    };
                    buffer.insert(at, &indent);
                } else {
                    let removable = dedent_width(buffer, line, &options);
                    if removable > 0 {
                        let at = buffer.rope.line_to_char(line);
                        buffer.remove(at, at + removable);
                    }
                }
            }
            let line = first.min(buffer.line_count().saturating_sub(1));
            buffer.cursor = Position::new(line, text::first_non_blank(&buffer.rope, line));
            buffer.desired_col = buffer.cursor.col;
            buffer.end();
            Outcome::Done
        }
        Operator::Lowercase | Operator::Uppercase | Operator::ToggleCase => {
            let original = buffer.slice(range.start, range.end);
            let transformed: String = original
                .chars()
                .map(|c| match op {
                    Operator::Lowercase => c.to_lowercase().next().unwrap_or(c),
                    Operator::Uppercase => c.to_uppercase().next().unwrap_or(c),
                    _ if c.is_lowercase() => c.to_uppercase().next().unwrap_or(c),
                    _ => c.to_lowercase().next().unwrap_or(c),
                })
                .collect();
            if transformed != original {
                buffer.begin();
                buffer.replace(range.start, range.end, &transformed);
                buffer.end();
            }
            buffer.cursor = text::clamp(
                &buffer.rope,
                text::char_to_pos(&buffer.rope, range.start),
                false,
            );
            buffer.desired_col = buffer.cursor.col;
            Outcome::Done
        }
    }
}

fn last_line_of(buffer: &Buffer, range: &EditRange) -> usize {
    let last = buffer.line_count().saturating_sub(1);
    if range.end <= range.start {
        return buffer.rope.char_to_line(range.start).min(last);
    }
    // A linewise range ends at the start of the line after the last one.
    let end = if range.linewise {
        range.end - 1
    } else {
        range.end
    };
    buffer
        .rope
        .char_to_line(end.min(buffer.rope.len_chars()))
        .min(last)
}

/// How many leading characters make up one shift's worth of indentation.
fn dedent_width(buffer: &Buffer, line: usize, options: &EditOptions) -> usize {
    let l = text::line(&buffer.rope, line);
    let mut columns = 0;
    let mut chars = 0;
    for c in l.chars() {
        if columns >= options.shift_width {
            break;
        }
        match c {
            ' ' => columns += 1,
            '\t' => columns += options.tab_width - (columns % options.tab_width),
            _ => break,
        }
        chars += 1;
    }
    chars
}

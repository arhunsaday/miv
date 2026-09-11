//! A pending, reviewable edit.

use crate::core::text;
use similar::{ChangeTag, TextDiff};

#[derive(Clone, Debug)]
pub struct Proposal {
    pub instruction: String,
    pub buffer_id: usize,
    /// The character range it would replace, as it was when asked.
    pub start: usize,
    pub end: usize,
    pub original: String,
    pub replacement: String,
    /// Who to credit in the history.
    pub author: String,
}

impl Proposal {
    /// A unified-diff view, for showing what would change before it does.
    pub fn diff(&self) -> Vec<String> {
        let diff = TextDiff::from_lines(&self.original, &self.replacement);
        diff.iter_all_changes()
            .map(|change| {
                let marker = match change.tag() {
                    ChangeTag::Delete => '-',
                    ChangeTag::Insert => '+',
                    ChangeTag::Equal => ' ',
                };
                format!("{marker} {}", change.value().trim_end_matches('\n'))
            })
            .collect()
    }

    pub fn changed_lines(&self) -> usize {
        let diff = TextDiff::from_lines(&self.original, &self.replacement);
        diff.iter_all_changes()
            .filter(|change| change.tag() != ChangeTag::Equal)
            .count()
    }

    /// Whether the text it was built against is still there.
    ///
    /// A proposal is only valid against the text it saw; if the region moved
    /// or changed while the provider was thinking, applying it would corrupt
    /// the file.
    pub fn still_applies(&self, buffer: &crate::core::buffer::Buffer) -> bool {
        buffer.id == self.buffer_id
            && self.end <= buffer.rope.len_chars()
            && buffer.slice(self.start, self.end) == self.original
    }

    /// Apply it as one undo step, attributed to its author.
    pub fn apply(&self, buffer: &mut crate::core::buffer::Buffer) {
        buffer.begin_authored(self.author.clone());
        buffer.replace(self.start, self.end, &self.replacement);
        buffer.end();
        let landed = self.start + self.replacement.chars().count();
        let position = text::char_to_pos(&buffer.rope, landed.min(buffer.rope.len_chars()));
        buffer.cursor = text::clamp(&buffer.rope, position, false);
        buffer.desired_col = buffer.cursor.col;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proposal(original: &str, replacement: &str) -> Proposal {
        Proposal {
            instruction: "tidy".to_string(),
            buffer_id: 1,
            start: 0,
            end: original.chars().count(),
            original: original.to_string(),
            replacement: replacement.to_string(),
            author: "ai".to_string(),
        }
    }

    #[test]
    fn the_diff_marks_what_would_change() {
        let proposal = proposal("one\ntwo\n", "one\nTWO\n");
        let diff = proposal.diff();
        assert!(diff.contains(&"  one".to_string()));
        assert!(diff.contains(&"- two".to_string()));
        assert!(diff.contains(&"+ TWO".to_string()));
        assert_eq!(proposal.changed_lines(), 2);
    }

    /// A buffer whose whole contents are exactly `text`.
    ///
    /// The rope always ends with a newline, so the text is inserted without
    /// one and the invariant supplies it.
    fn buffer_of(text: &str) -> crate::core::buffer::Buffer {
        let mut buffer = crate::core::buffer::Buffer::scratch(1);
        buffer.insert(0, text.trim_end_matches('\n'));
        buffer
    }

    #[test]
    fn a_proposal_is_refused_once_the_text_has_moved_on() {
        let mut buffer = buffer_of("one\ntwo\n");
        let proposal = proposal("one\ntwo\n", "one\nTWO\n");
        assert!(proposal.still_applies(&buffer));

        // Someone else edited the region in the meantime.
        buffer.insert(0, "zero\n");
        assert!(
            !proposal.still_applies(&buffer),
            "applying against changed text would corrupt it"
        );
    }

    #[test]
    fn applying_is_one_undo_step_and_is_attributed() {
        let mut buffer = buffer_of("one\ntwo\n");
        let proposal = proposal("one\ntwo\n", "one\nTWO\nthree\n");
        proposal.apply(&mut buffer);
        assert_eq!(buffer.rope.to_string(), "one\nTWO\nthree\n");
        assert_eq!(buffer.history.last_author(), Some("ai"));

        assert!(buffer.undo());
        assert_eq!(buffer.rope.to_string(), "one\ntwo\n");
    }
}

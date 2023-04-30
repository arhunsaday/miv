use std::cmp;
use unicode_segmentation::UnicodeSegmentation;

#[derive(Default)]
pub struct Row {
    string: String,
    len: usize,
}

impl From<&str> for Row {
    fn from(slice: &str) -> Self {
        Self {
            string: String::from(slice),
            len: slice.graphemes(true).count(),
        }
    }
}

impl Row {
    pub fn render(&self, start: usize, end: usize) -> String {
        let end = cmp::min(end, self.string.len());
        let start = cmp::min(start, end);
        let mut result = String::new();

        // Use graphemes instead of chars to handle unicode characters
        for grapheme in self.string[..]
            .graphemes(true)
            .skip(start)
            .take(end - start)
        {
            // Use spaces instead of tabs for now (configurable later)
            if grapheme == "\t" {
                result.push(' ');
            } else {
                result.push_str(grapheme);
            }
        }

        result
    }

    pub fn as_bytes(&self) -> &[u8] {
        self.string.as_bytes()
    }

    pub fn is_empty(&self) -> bool {
        self.string.is_empty()
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn insert(&mut self, at: usize, c: char) {
        // End of line, just append
        if at >= self.len {
            self.string.push(c);
            self.len += 1;
        } else {
            // Not end of line, divide and append in the correct position in the middle
            let mut result: String = String::new();
            let mut length = 0;
            for (index, grapheme) in self.string[..].graphemes(true).enumerate() {
                length += 1;

                if index == at {
                    length += 1;
                    result.push(c);
                }

                result.push_str(grapheme);
            }

            self.len = length;
            self.string = result;
        }
    }

    pub fn delete(&mut self, at: usize) {
        // End of line, do nothing
        if at > self.len {
            return;
        }
        let mut result: String = String::new();
        let mut length = 0;
        for (index, grapheme) in self.string[..].graphemes(true).enumerate() {
            if index != at {
                length += 1;
                result.push_str(grapheme);
            }
        }
        self.len = length;
        self.string = result;
    }

    pub fn append(&mut self, new: &Self) {
        self.string = format!("{}{}", self.string, new.string);
        self.len += new.len;
    }

    pub fn split(&mut self, at: usize) -> Self {
        let mut row: String = String::new();
        let mut length = 0;
        let mut splitted_row: String = String::new();
        let mut splitted_length = 0;

        for (index, grapheme) in self.string[..].graphemes(true).enumerate() {
            if index < at {
                length += 1;
                row.push_str(grapheme);
            } else {
                splitted_length += 1;
                splitted_row.push_str(grapheme);
            }
        }

        self.string = row;
        self.len = length;

        Self {
            string: splitted_row,
            len: splitted_length,
        }
    }
}

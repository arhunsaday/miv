use std::cmp;
use unicode_segmentation::UnicodeSegmentation;

#[derive(Default)]
pub struct Row {
    string: String,
    len: usize,
}

impl From<&str> for Row {
    fn from(slice: &str) -> Self {
        let mut row = Self {
            string: String::from(slice),
            len: 0,
        };

        row.update_len();
        row
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

    pub fn is_empty(&self) -> bool {
        self.string.is_empty()
    }

    pub fn len(&self) -> usize {
        self.len
    }

    fn update_len(&mut self) {
        self.len = self.string[..].graphemes(true).count();
    }

    pub fn insert(&mut self, at: usize, c: char) {
        // End of line, just append
        if at > self.len {
            self.string.push(c);
        } else {
            // Not end of line, divide and append in the correct position in the middle
            let mut result: String = self.string[..].graphemes(true).take(at).collect();
            let rest: String = self.string[..].graphemes(true).skip(at).collect();

            result.push(c);
            result.push_str(&rest);
            self.string = result;
        }

        self.update_len();
    }

    pub fn delete(&mut self, at: usize) {
        // End of line, do nothing
        if at > self.len {
            return;
        } else {
            let mut result: String = self.string[..].graphemes(true).take(at).collect();
            let rest: String = self.string[..].graphemes(true).skip(at + 1).collect();

            result.push_str(&rest);
            self.string = result;
        }

        self.update_len();
    }

    pub fn append(&mut self, new: &Self) {
        self.string = format!("{}{}", self.string, new.string);
        self.update_len();
    }
}

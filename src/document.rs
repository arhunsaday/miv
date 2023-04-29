use crate::Position;
use crate::Row;
use std::fs;
use std::io::{Error, Write};

#[derive(Default)]
pub struct Document {
    pub file_name: Option<String>,
    rows: Vec<Row>,
    dirty: bool,
}

impl Document {
    pub fn open(file_name: &str) -> Result<Self, std::io::Error> {
        let file_contents = fs::read_to_string(file_name)?;
        let mut rows = Vec::new();

        for line in file_contents.lines() {
            rows.push(Row::from(line));
        }

        Ok(Self {
            file_name: Some(file_name.to_string()),
            rows,
            dirty: false,
        })
    }

    pub fn open_non_existent(file_name: &str) -> Self {
        Self {
            file_name: Some(file_name.to_string()),
            rows: Vec::new(),
            dirty: false,
        }
    }

    pub fn save(&mut self) -> Result<(), Error> {
        if let Some(file_name) = &self.file_name {
            let mut file = fs::File::create(file_name)?;

            for row in &self.rows {
                file.write_all(row.as_bytes())?;
                file.write_all(b"\n")?;
            }

            self.dirty = false;
        }

        Ok(())
    }

    pub fn row(&self, index: usize) -> Option<&Row> {
        self.rows.get(index)
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn insert_newline(&mut self, at: &Position) {
        if at.y > self.len() {
            return;
        }

        // Cursor at the end of the line, just insert a new empty row
        if at.y == self.len() {
            self.rows.push(Row::default());
        }

        // Otherwise split the line at the cursor position and insert a new row containing the right half
        let new_row = self.rows.get_mut(at.y).unwrap().split(at.x);
        self.rows.insert(at.y + 1, new_row);
    }

    pub fn insert(&mut self, at: &Position, c: char) {
        if at.y > self.len() {
            return;
        }

        self.dirty = true;

        if at.y == self.len() {
            let mut new_row = Row::default();
            new_row.insert(0, c);
            self.rows.push(new_row);
        } else {
            let row = self.rows.get_mut(at.y).unwrap();
            row.insert(at.x, c);
        }
    }

    pub fn delete(&mut self, at: &Position) {
        let len = self.len();

        // Last line, nothing to delete
        if at.y >= len {
            return;
        }

        self.dirty = true;

        // If we're at the end of a line and there's a line after, append them together
        if at.x == self.rows.get_mut(at.y).unwrap().len() && at.y < len - 1 {
            let next_row = self.rows.remove(at.y + 1);
            let row = self.rows.get_mut(at.y).unwrap();
            row.append(&next_row);
        } else {
            // Otherwise, just delete the single character
            let row = self.rows.get_mut(at.y).unwrap();
            row.delete(at.x);
        }
    }
}

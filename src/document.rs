use crate::Position;
use crate::Row;
use crate::SearchDirection;
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

        if at.y == self.rows.len() {
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
        if at.x == self.rows.get_mut(at.y).unwrap().len() && at.y + 1 < len {
            let next_row = self.rows.remove(at.y + 1);
            let row = self.rows.get_mut(at.y).unwrap();
            row.append(&next_row);
        } else {
            // Otherwise, just delete the single character
            let row = self.rows.get_mut(at.y).unwrap();
            row.delete(at.x);
        }
    }

    pub fn find(&self, query: &str, at: &Position, direction: SearchDirection) -> Option<Position> {
        if at.y > self.rows.len() {
            return None;
        }

        let mut position = Position { x: at.x, y: at.y };

        // If forward search, search from current pos to end of file
        // Else search from start of file to current pos
        let start = if direction == SearchDirection::Forward {
            at.y
        } else {
            0
        };
        let end = if direction == SearchDirection::Forward {
            self.rows.len()
        } else {
            at.y.saturating_add(1)
        };

        for _ in start..end {
            if let Some(row) = self.rows.get(position.y) {
                if let Some(x) = row.find(&query, position.x, direction) {
                    position.x = x;
                    return Some(position);
                }
                if direction == SearchDirection::Forward {
                    position.y = position.y.saturating_add(1);
                    position.x = 0;
                } else {
                    position.y = position.y.saturating_sub(1);
                    position.x = self.rows[position.y].len();
                }
            } else {
                return None;
            }
        }
        None
    }
}

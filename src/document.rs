use crate::Row;
use std::fs;

#[derive(Default)]
pub struct Document {
    rows: Vec<Row>,
}

impl Document {
    pub fn open(file_name: &str) -> Result<Self, std::io::Error> {
        let file_contents = fs::read_to_string(file_name)?;
        let mut rows = Vec::new();

        for line in file_contents.lines() {
            rows.push(Row::from(line));
        }

        Ok(Self { rows })
    }

    pub fn row(&self, index: usize) -> Option<&Row> {
        self.rows.get(index)
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

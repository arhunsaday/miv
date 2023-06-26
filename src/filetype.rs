pub struct FileType {
    name: String,
    extension: String,
}

impl Default for FileType {
    fn default() -> Self {
        Self {
            name: String::from("Unknown or no file type"),
            extension: String::from(""),
        }
    }
}

impl FileType {
    pub fn name(&self) -> String {
        self.name.clone()
    }

    pub fn from(file_name: &str) -> Self {
        let extension = file_name.split('.').last().unwrap_or("");
        let name = match extension {
            "rs" => "Rust",
            "toml" => "TOML",
            "md" => "Markdown",
            "txt" => "Plain text",
            _ => "Unknown or no file type",
        };

        Self {
            name: String::from(name),
            extension: String::from(extension),
        }
    }
}

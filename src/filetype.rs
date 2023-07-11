pub struct FileType {
    name: String,
}

impl Default for FileType {
    fn default() -> Self {
        Self {
            name: String::from("Unknown or no file type"),
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
            "c" => "C",
            "cpp" => "C++",
            "h" => "C header",
            "hpp" => "C++ header",
            "py" => "Python",
            "js" => "JavaScript",
            "html" => "HTML",
            "css" => "CSS",
            "json" => "JSON",
            "sh" => "Shell script",
            "go" => "Go",
            "java" => "Java",
            _ => "Unknown or no file type",
        };

        Self {
            name: String::from(name),
        }
    }
}

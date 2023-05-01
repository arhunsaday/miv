pub struct FileType {
    name: String,
    highlight_options: HighlightOptions,
}

#[derive(Default, Copy, Clone)]
pub struct HighlightOptions {
    numbers: bool,
    strings: bool,
    characters: bool,
    comments: bool,
    multiline_comments: bool,
}

impl Default for FileType {
    fn default() -> Self {
        Self {
            name: String::from("Unknown or no file type"),
            highlight_options: HighlightOptions::default(),
        }
    }
}

impl FileType {
    pub fn name(&self) -> String {
        self.name.clone()
    }

    pub fn highlight_options(&self) -> HighlightOptions {
        self.highlight_options
    }

    pub fn from(file_name: &str) -> Self {
        let extension = file_name.split('.').last().unwrap_or("");

        match extension {
            "rs" => Self {
                name: String::from("Rust"),
                highlight_options: HighlightOptions {
                    numbers: true,
                    strings: true,
                    characters: true,
                    comments: true,
                    multiline_comments: true,
                },
            },
            "ts" => Self {
                name: String::from("Typescript"),
                highlight_options: HighlightOptions {
                    numbers: true,
                    strings: true,
                    characters: true,
                    comments: true,
                    multiline_comments: true,
                },
            },
            _ => Self::default(),
        }
    }
}

impl HighlightOptions {
    pub fn numbers(&self) -> bool {
        self.numbers
    }

    pub fn strings(&self) -> bool {
        self.strings
    }

    pub fn characters(&self) -> bool {
        self.characters
    }

    pub fn comments(&self) -> bool {
        self.comments
    }

    pub fn multiline_comments(&self) -> bool {
        self.multiline_comments
    }
}

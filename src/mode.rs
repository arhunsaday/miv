//! Editor modes and their presentation.

use ratatui::style::Color;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VisualKind {
    Char,
    Line,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Normal,
    Insert,
    /// `R`: typing overwrites instead of inserting.
    Replace,
    Visual(VisualKind),
    /// The `:` prompt.
    Command,
    /// The `/` or `?` prompt.
    Search,
}

impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Mode::Normal => "NORMAL",
            Mode::Insert => "INSERT",
            Mode::Replace => "REPLACE",
            Mode::Visual(VisualKind::Char) => "VISUAL",
            Mode::Visual(VisualKind::Line) => "V-LINE",
            Mode::Command => "COMMAND",
            Mode::Search => "SEARCH",
        }
    }

    pub fn color(self) -> Color {
        match self {
            Mode::Normal => Color::Blue,
            Mode::Insert => Color::Green,
            Mode::Replace => Color::Red,
            Mode::Visual(_) => Color::Magenta,
            Mode::Command | Mode::Search => Color::Yellow,
        }
    }

    /// Whether the cursor may rest one column past the last character.
    pub fn allows_eol(self) -> bool {
        matches!(self, Mode::Insert | Mode::Replace | Mode::Visual(_))
    }

    pub fn is_visual(self) -> bool {
        matches!(self, Mode::Visual(_))
    }

    pub fn is_prompt(self) -> bool {
        matches!(self, Mode::Command | Mode::Search)
    }
}

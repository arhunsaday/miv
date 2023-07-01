use std::fmt::Display;

use crossterm::style::Color;

#[derive(Default)]
pub enum Mode {
    #[default]
    Normal,
    Insert,
    Visual,
    Command,
    Search,
}

impl Display for Mode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mode = match self {
            Mode::Normal => "Normal",
            Mode::Insert => "Insert",
            Mode::Visual => "Visual",
            Mode::Command => "Command",
            Mode::Search => "Search",
        };
        write!(f, "{}", mode)
    }
}

impl Mode {
    pub fn to_color(&self) -> Color {
        match self {
            Mode::Normal => Color::Blue,
            Mode::Insert => Color::Red,
            Mode::Visual => Color::Blue,
            Mode::Command => Color::Yellow,
            Mode::Search => Color::Green,
        }
    }
}

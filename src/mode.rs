use crate::Terminal;
use crossterm::{cursor::SetCursorStyle, style::Color};
use std::fmt::Display;

#[derive(Default)]
pub enum PossibleModes {
    #[default]
    Normal,
    Insert,
    Visual,
    Command,
    Search,
}

#[derive(Default)]
pub struct Mode {
    pub current_mode: PossibleModes,
}

impl Display for PossibleModes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mode = match self {
            PossibleModes::Normal => "Normal",
            PossibleModes::Insert => "Insert",
            PossibleModes::Visual => "Visual",
            PossibleModes::Command => "Command",
            PossibleModes::Search => "Search",
        };
        write!(f, "{}", mode)
    }
}

impl PossibleModes {
    pub fn to_color(&self) -> Color {
        match self {
            PossibleModes::Normal => Color::Blue,
            PossibleModes::Insert => Color::Red,
            PossibleModes::Visual => Color::Green,
            PossibleModes::Command => Color::Magenta,
            PossibleModes::Search => Color::Yellow,
        }
    }
}

impl Mode {
    pub fn switch(&mut self, new_mode: PossibleModes) {
        self.current_mode = match new_mode {
            PossibleModes::Normal => {
                Terminal::set_cursor(SetCursorStyle::BlinkingBlock);
                PossibleModes::Normal
            }
            PossibleModes::Insert => {
                Terminal::set_cursor(SetCursorStyle::BlinkingBar);
                PossibleModes::Insert
            }
            PossibleModes::Visual => {
                Terminal::set_cursor(SetCursorStyle::SteadyBlock);
                PossibleModes::Visual
            }
            _ => PossibleModes::Normal,
        }
    }
}

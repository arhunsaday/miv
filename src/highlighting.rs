#[derive(PartialEq)]
pub enum Type {
    None,
    Number,
    Match,
}

pub enum Color {
    Reset,
    Black,
    Red,
    Green,
    Yellow,
    Blue,
    Magenta,
    Cyan,
    White,
    BrightBlack,
    BrightRed,
    BrightGreen,
    BrightYellow,
    BrightBlue,
    BrightMagenta,
    BrightCyan,
    BrightWhite,
}

impl Type {
    pub fn to_color(&self) -> String {
        match self {
            Type::Number => String::from("\x1b[31m"),
            // Type::Match => String::from("\x1b[7m\x1b[32m"),
            Type::Match => String::from("\x1b[7m\x1b[33m"),
            _ => String::from("\x1b[0m"),
        }
    }
}

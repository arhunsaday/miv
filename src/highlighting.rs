#[derive(PartialEq, Clone, Copy)]
pub enum Type {
    None,
    Match,
    Number,
    Character,
    String,
    Comment,
    MultilineComment,
    PrimaryKeywords,
    SecondaryKeywords,
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
    pub fn to_color(self) -> String {
        match self {
            // TODO: Use crossterm for this or deobfuscate the ANSI codes
            Type::Match => String::from("\x1b[7m\x1b[33m"),
            Type::Number => String::from("\x1b[31m"),
            Type::String => String::from("\x1b[32m"),
            Type::Character => String::from("\x1b[33m"),
            Type::Comment => String::from("\x1b[34m"),
            Type::MultilineComment => String::from("\x1b[35m"),
            Type::PrimaryKeywords => String::from("\x1b[36m"),
            Type::SecondaryKeywords => String::from("\x1b[36m"),
            _ => String::from("\x1b[0m"),
        }
    }
}

use syntect::highlighting::ThemeSet;
use syntect::parsing::SyntaxSet;

pub struct Highlighting {
    pub syntax_set: SyntaxSet,
    pub theme_set: ThemeSet,
}

impl Default for Highlighting {
    fn default() -> Self {
        Self {
            syntax_set: SyntaxSet::load_defaults_newlines(),
            theme_set: ThemeSet::load_defaults(),
        }
    }
}

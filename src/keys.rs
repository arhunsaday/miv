//! Textual key notation, so that macros can live in registers as editable
//! text the way they do in Vim.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

pub fn encode(key: KeyEvent) -> String {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let named = |name: &str| {
        let prefix = match (ctrl, alt) {
            (true, true) => "C-A-",
            (true, false) => "C-",
            (false, true) => "A-",
            (false, false) => "",
        };
        format!("<{prefix}{name}>")
    };

    match key.code {
        KeyCode::Char(c) if !ctrl && !alt => match c {
            '<' => "<lt>".to_string(),
            c => c.to_string(),
        },
        KeyCode::Char(c) => named(&c.to_string()),
        KeyCode::Esc => named("Esc"),
        KeyCode::Enter => named("CR"),
        KeyCode::Tab => named("Tab"),
        KeyCode::BackTab => named("S-Tab"),
        KeyCode::Backspace => named("BS"),
        KeyCode::Delete => named("Del"),
        KeyCode::Left => named("Left"),
        KeyCode::Right => named("Right"),
        KeyCode::Up => named("Up"),
        KeyCode::Down => named("Down"),
        KeyCode::Home => named("Home"),
        KeyCode::End => named("End"),
        KeyCode::PageUp => named("PageUp"),
        KeyCode::PageDown => named("PageDown"),
        KeyCode::Insert => named("Insert"),
        KeyCode::F(n) => named(&format!("F{n}")),
        _ => String::new(),
    }
}

pub fn encode_all(keys: &[KeyEvent]) -> String {
    keys.iter().copied().map(encode).collect()
}

pub fn parse(input: &str) -> Vec<KeyEvent> {
    let chars: Vec<char> = input.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '<' {
            if let Some(close) = (i + 1..chars.len()).find(|&j| chars[j] == '>') {
                let name: String = chars[i + 1..close].iter().collect();
                if let Some(key) = parse_named(&name) {
                    out.push(key);
                    i = close + 1;
                    continue;
                }
            }
        }
        out.push(KeyEvent::new(KeyCode::Char(chars[i]), KeyModifiers::NONE));
        i += 1;
    }
    out
}

fn parse_named(name: &str) -> Option<KeyEvent> {
    let mut modifiers = KeyModifiers::NONE;
    let mut rest = name;
    loop {
        if let Some(stripped) = rest.strip_prefix("C-") {
            modifiers |= KeyModifiers::CONTROL;
            rest = stripped;
        } else if let Some(stripped) = rest.strip_prefix("A-") {
            modifiers |= KeyModifiers::ALT;
            rest = stripped;
        } else if let Some(stripped) = rest.strip_prefix("S-") {
            modifiers |= KeyModifiers::SHIFT;
            rest = stripped;
        } else {
            break;
        }
    }

    let code = match rest {
        "lt" => KeyCode::Char('<'),
        "Esc" => KeyCode::Esc,
        "CR" | "Enter" => KeyCode::Enter,
        "Tab" => KeyCode::Tab,
        "BS" => KeyCode::Backspace,
        "Del" => KeyCode::Delete,
        "Left" => KeyCode::Left,
        "Right" => KeyCode::Right,
        "Up" => KeyCode::Up,
        "Down" => KeyCode::Down,
        "Home" => KeyCode::Home,
        "End" => KeyCode::End,
        "PageUp" => KeyCode::PageUp,
        "PageDown" => KeyCode::PageDown,
        "Insert" => KeyCode::Insert,
        "Space" => KeyCode::Char(' '),
        other => {
            let mut it = other.chars();
            let c = it.next()?;
            if it.next().is_some() {
                let digits = other.strip_prefix('F')?;
                KeyCode::F(digits.parse().ok()?)
            } else {
                KeyCode::Char(c)
            }
        }
    };
    Some(KeyEvent::new(code, modifiers))
}

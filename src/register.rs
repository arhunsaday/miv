//! Vim registers: unnamed, named `a`-`z` (uppercase appends), the yank and
//! delete rings, the blackhole, and `+`/`*` bridged to the system clipboard.

use std::collections::HashMap;
use std::io::Write;

/// Whether a register's contents behave as whole lines when put back.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum RegisterKind {
    #[default]
    Charwise,
    Linewise,
}

#[derive(Clone, Debug, Default)]
pub struct RegisterContent {
    pub text: String,
    pub kind: RegisterKind,
}

impl RegisterContent {
    pub fn charwise(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            kind: RegisterKind::Charwise,
        }
    }
}

#[derive(Default)]
pub struct Registers {
    slots: HashMap<char, RegisterContent>,
}

impl Registers {
    /// Record a yank. Vim puts every yank in `"` and `0`.
    pub fn yank(&mut self, name: Option<char>, content: RegisterContent) {
        match name {
            Some('_') => {}
            Some(c) => self.write_named(c, content),
            None => {
                self.slots.insert('0', content.clone());
                self.slots.insert('"', content);
            }
        }
    }

    /// Record a deletion. Vim shifts `1`-`9` for multiline deletes and uses
    /// the small-delete register `-` otherwise.
    pub fn delete(&mut self, name: Option<char>, content: RegisterContent) {
        match name {
            Some('_') => {}
            Some(c) => self.write_named(c, content),
            None => {
                if content.kind == RegisterKind::Linewise || content.text.contains('\n') {
                    for slot in (1..9).rev() {
                        let from = char::from_digit(slot, 10).unwrap();
                        let to = char::from_digit(slot + 1, 10).unwrap();
                        if let Some(v) = self.slots.get(&from).cloned() {
                            self.slots.insert(to, v);
                        }
                    }
                    self.slots.insert('1', content.clone());
                } else {
                    self.slots.insert('-', content.clone());
                }
                self.slots.insert('"', content);
            }
        }
    }

    fn write_named(&mut self, name: char, content: RegisterContent) {
        if name.is_ascii_uppercase() {
            // Uppercase appends to the lowercase register.
            let lower = name.to_ascii_lowercase();
            let existing = self.slots.entry(lower).or_default();
            if existing.kind == RegisterKind::Linewise && !existing.text.ends_with('\n') {
                existing.text.push('\n');
            }
            existing.text.push_str(&content.text);
            if content.kind == RegisterKind::Linewise {
                existing.kind = RegisterKind::Linewise;
            }
            let merged = existing.clone();
            self.slots.insert('"', merged);
            return;
        }
        if name == '+' || name == '*' {
            set_system_clipboard(&content.text);
        }
        self.slots.insert(name, content.clone());
        self.slots.insert('"', content);
    }

    pub fn get(&self, name: Option<char>) -> Option<&RegisterContent> {
        let name = name.unwrap_or('"');
        if name == '_' {
            return None;
        }
        self.slots.get(&name.to_ascii_lowercase())
    }

    /// Registers holding content, for `:registers`.
    pub fn listing(&self) -> Vec<(char, &RegisterContent)> {
        let mut out: Vec<_> = self.slots.iter().map(|(k, v)| (*k, v)).collect();
        out.sort_by_key(|(k, _)| *k);
        out
    }
}

/// Copy to the terminal's clipboard with OSC 52, which works over SSH and
/// inside tmux where a native clipboard API would not.
fn set_system_clipboard(text: &str) {
    let encoded = base64_encode(text.as_bytes());
    let mut out = std::io::stdout();
    let _ = write!(out, "\x1b]52;c;{encoded}\x07");
    let _ = out.flush();
}

fn base64_encode(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((input.len() + 2) / 3 * 4);
    for chunk in input.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = u32::from_be_bytes([0, b[0], b[1], b[2]]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(TABLE[(n >> (18 - i * 6) & 0x3f) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

//! The wire format shared by the terminal client, the browser client and the
//! session host.
//!
//! Messages are JSON objects, newline-delimited over TCP and carried as text
//! frames over WebSocket, so one parser serves both transports.

use ratatui::buffer::Cell;
use ratatui::style::Color;
use serde::{Deserialize, Serialize};

/// Bumped when a change would confuse an older client. The host refuses a
/// mismatch with a readable message rather than desynchronising.
pub const PROTOCOL_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum Access {
    /// May watch and move their own cursor, but not change the text.
    #[default]
    Read,
    /// May edit.
    Write,
}

impl Access {
    pub fn label(self) -> &'static str {
        match self {
            Access::Read => "read-only",
            Access::Write => "read-write",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    /// First message on every connection.
    Hello {
        version: u32,
        token: String,
        name: String,
        cols: u16,
        rows: u16,
    },
    /// Keystrokes in miv's own notation, e.g. `ciw` or `<Esc>`.
    Keys {
        keys: String,
    },
    Paste {
        text: String,
    },
    Resize {
        cols: u16,
        rows: u16,
    },
    /// Ask the host for write access.
    RequestWrite,
    Bye,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    Welcome {
        version: u32,
        id: u32,
        name: String,
        access: Access,
        host: String,
        file: String,
    },
    /// A screen update. `full` marks a complete repaint, sent on join and
    /// after a resize; otherwise only changed cells are included.
    Frame {
        full: bool,
        cols: u16,
        rows: u16,
        cells: Vec<WireCell>,
        cursor: Option<[u16; 2]>,
    },
    /// Who is currently connected, for the client's participant list.
    Roster {
        participants: Vec<RosterEntry>,
    },
    Notice {
        text: String,
    },
    Closed {
        reason: String,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RosterEntry {
    pub id: u32,
    pub name: String,
    pub color: u8,
    pub access: Access,
    pub mode: String,
    /// How this participant is connected: host, terminal or browser.
    pub via: String,
    pub line: usize,
    pub file: String,
    pub is_host: bool,
}

/// One screen cell. Field names are short because these dominate the traffic.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WireCell {
    pub x: u16,
    pub y: u16,
    #[serde(rename = "s")]
    pub symbol: String,
    #[serde(rename = "f")]
    pub fg: WireColor,
    #[serde(rename = "b")]
    pub bg: WireColor,
    #[serde(rename = "m")]
    pub modifiers: u16,
}

impl WireCell {
    pub fn from_cell(x: u16, y: u16, cell: &Cell) -> Self {
        Self {
            x,
            y,
            symbol: cell.symbol().to_string(),
            fg: WireColor::from(cell.fg),
            bg: WireColor::from(cell.bg),
            modifiers: cell.modifier.bits(),
        }
    }
}

/// A terminal colour in a form both clients can render: the browser needs
/// concrete values, and the terminal client needs to map back to ratatui.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "k", rename_all = "snake_case")]
pub enum WireColor {
    /// The viewer's own default foreground or background.
    Default,
    Indexed {
        i: u8,
    },
    Rgb {
        r: u8,
        g: u8,
        b: u8,
    },
}

impl From<Color> for WireColor {
    fn from(color: Color) -> Self {
        let indexed = |i| WireColor::Indexed { i };
        match color {
            Color::Reset => WireColor::Default,
            Color::Black => indexed(0),
            Color::Red => indexed(1),
            Color::Green => indexed(2),
            Color::Yellow => indexed(3),
            Color::Blue => indexed(4),
            Color::Magenta => indexed(5),
            Color::Cyan => indexed(6),
            Color::Gray => indexed(7),
            Color::DarkGray => indexed(8),
            Color::LightRed => indexed(9),
            Color::LightGreen => indexed(10),
            Color::LightYellow => indexed(11),
            Color::LightBlue => indexed(12),
            Color::LightMagenta => indexed(13),
            Color::LightCyan => indexed(14),
            Color::White => indexed(15),
            Color::Indexed(i) => indexed(i),
            Color::Rgb(r, g, b) => WireColor::Rgb { r, g, b },
        }
    }
}

impl From<WireColor> for Color {
    fn from(color: WireColor) -> Self {
        match color {
            WireColor::Default => Color::Reset,
            WireColor::Indexed { i } => Color::Indexed(i),
            WireColor::Rgb { r, g, b } => Color::Rgb(r, g, b),
        }
    }
}

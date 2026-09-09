//! `miv --attach`: a second terminal joined to someone else's session.
//!
//! The client holds no document state at all. It sends keystrokes and paints
//! the cells the host sends back, which is why a participant sees every
//! feature the host's editor has, including ones the client has never heard
//! of.

use super::protocol::{ClientMessage, ServerMessage, WireColor, PROTOCOL_VERSION};
use crate::keys;
use anyhow::{Context, Result};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::style::{Attribute, Attributes, Color, Print, SetAttributes, SetColors};
use crossterm::terminal::{Clear, ClearType};
use crossterm::{cursor, queue};
use ratatui::style::Modifier;
use std::io::{BufRead, BufReader, ErrorKind, Write};
use std::net::TcpStream;
use std::time::Duration;

/// Detach key. Deliberately one miv itself does not bind, so it always works.
const DETACH: char = '\\';

pub fn attach(address: &str, token: &str, name: &str) -> Result<()> {
    let stream = TcpStream::connect(address).with_context(|| format!("connecting to {address}"))?;
    stream.set_nodelay(true).ok();

    let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));
    let mut writer = stream.try_clone().context("cloning the connection")?;
    send(
        &mut writer,
        &ClientMessage::Hello {
            version: PROTOCOL_VERSION,
            token: token.to_string(),
            name: name.to_string(),
            cols,
            rows,
        },
    )?;

    stream.set_read_timeout(Some(Duration::from_millis(5)))?;
    let mut reader = BufReader::new(stream);

    crossterm::terminal::enable_raw_mode()?;
    crossterm::execute!(
        std::io::stdout(),
        crossterm::terminal::EnterAlternateScreen,
        crossterm::event::EnableBracketedPaste,
        Clear(ClearType::All)
    )?;

    let result = run(&mut reader, &mut writer);

    let _ = send(&mut writer, &ClientMessage::Bye);
    crate::restore_terminal();
    match &result {
        Ok(Some(reason)) => println!("miv: session closed: {reason}"),
        Ok(None) => println!("miv: detached"),
        Err(_) => {}
    }
    result.map(|_| ())
}

fn run(reader: &mut BufReader<TcpStream>, writer: &mut TcpStream) -> Result<Option<String>> {
    let mut line = String::new();
    loop {
        // Local input.
        if event::poll(Duration::from_millis(5))? {
            match event::read()? {
                Event::Key(key) if key.kind != KeyEventKind::Release => {
                    if key.code == KeyCode::Char(DETACH)
                        && key.modifiers.contains(KeyModifiers::CONTROL)
                    {
                        return Ok(None);
                    }
                    let encoded = keys::encode(key);
                    if !encoded.is_empty() {
                        send(writer, &ClientMessage::Keys { keys: encoded })?;
                    }
                }
                Event::Paste(text) => send(writer, &ClientMessage::Paste { text })?,
                Event::Resize(cols, rows) => send(writer, &ClientMessage::Resize { cols, rows })?,
                _ => {}
            }
        }

        // Anything the host has sent.
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) => return Ok(Some("host disconnected".to_string())),
                Ok(_) => {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        break;
                    }
                    match serde_json::from_str::<ServerMessage>(trimmed) {
                        Ok(ServerMessage::Frame { cells, cursor, .. }) => {
                            paint(&cells, cursor)?;
                        }
                        Ok(ServerMessage::Closed { reason }) => return Ok(Some(reason)),
                        Ok(_) => {}
                        Err(_) => {}
                    }
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock || e.kind() == ErrorKind::TimedOut => {
                    break
                }
                Err(e) => return Err(e.into()),
            }
        }
    }
}

fn send(stream: &mut TcpStream, message: &ClientMessage) -> Result<()> {
    let text = serde_json::to_string(message)?;
    stream.write_all(text.as_bytes())?;
    stream.write_all(b"\n")?;
    stream.flush()?;
    Ok(())
}

fn paint(cells: &[super::protocol::WireCell], cursor: Option<[u16; 2]>) -> Result<()> {
    let mut out = std::io::stdout();
    queue!(out, cursor::Hide)?;
    for cell in cells {
        queue!(
            out,
            cursor::MoveTo(cell.x, cell.y),
            SetAttributes(Attributes::from(Attribute::Reset)),
            SetAttributes(attributes(cell.modifiers)),
            SetColors(crossterm::style::Colors::new(
                color(cell.fg),
                color(cell.bg)
            )),
            Print(if cell.symbol.is_empty() {
                " "
            } else {
                &cell.symbol
            })
        )?;
    }
    if let Some([x, y]) = cursor {
        queue!(out, cursor::MoveTo(x, y), cursor::Show)?;
    }
    out.flush()?;
    Ok(())
}

fn color(color: WireColor) -> Color {
    match color {
        WireColor::Default => Color::Reset,
        WireColor::Indexed { i } => Color::AnsiValue(i),
        WireColor::Rgb { r, g, b } => Color::Rgb { r, g, b },
    }
}

fn attributes(bits: u16) -> Attributes {
    let modifier = Modifier::from_bits_truncate(bits);
    let mut attributes = Attributes::default();
    let mut add = |flag: Modifier, attribute: Attribute| {
        if modifier.contains(flag) {
            attributes.set(attribute);
        }
    };
    add(Modifier::BOLD, Attribute::Bold);
    add(Modifier::DIM, Attribute::Dim);
    add(Modifier::ITALIC, Attribute::Italic);
    add(Modifier::UNDERLINED, Attribute::Underlined);
    add(Modifier::REVERSED, Attribute::Reverse);
    add(Modifier::CROSSED_OUT, Attribute::CrossedOut);
    attributes
}

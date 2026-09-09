//! HTTP and WebSocket for the browser client.
//!
//! The page and the socket share one port, so a session is a single address
//! you can hand to someone. Routing happens before the WebSocket upgrade,
//! which is why the handshake is done by hand rather than by handing the
//! socket straight to tungstenite.

use base64::Engine;
use sha1::{Digest, Sha1};
use std::io::{ErrorKind, Read, Write};
use std::net::TcpStream;
use std::time::Duration;
use tungstenite::protocol::{Role, WebSocket};
use tungstenite::Message;

/// From RFC 6455; concatenated with the client key to derive the accept token.
const WEBSOCKET_GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

const CLIENT_HTML: &str = include_str!("client.html");

pub struct WebSocketStream {
    socket: WebSocket<TcpStream>,
}

impl WebSocketStream {
    pub fn receive(&mut self) -> std::io::Result<Option<String>> {
        loop {
            match self.socket.read() {
                Ok(Message::Text(text)) => return Ok(Some(text)),
                Ok(Message::Ping(payload)) => {
                    let _ = self.socket.write(Message::Pong(payload));
                    let _ = self.socket.flush();
                }
                Ok(Message::Close(_)) => return Err(ErrorKind::UnexpectedEof.into()),
                Ok(_) => continue,
                Err(tungstenite::Error::Io(e)) if e.kind() == ErrorKind::WouldBlock => {
                    return Ok(None)
                }
                Err(_) => return Err(ErrorKind::ConnectionAborted.into()),
            }
        }
    }

    pub fn send(&mut self, text: &str) -> std::io::Result<()> {
        match self.socket.write(Message::Text(text.to_string())) {
            Ok(()) => {}
            Err(tungstenite::Error::Io(e)) if e.kind() == ErrorKind::WouldBlock => {}
            Err(_) => return Err(ErrorKind::ConnectionAborted.into()),
        }
        match self.socket.flush() {
            Ok(()) => Ok(()),
            // Left queued; the next call flushes it.
            Err(tungstenite::Error::Io(e)) if e.kind() == ErrorKind::WouldBlock => Ok(()),
            Err(_) => Err(ErrorKind::ConnectionAborted.into()),
        }
    }
}

struct Request {
    path: String,
    websocket_key: Option<String>,
}

/// Serve one HTTP request. Returns a socket when the request was an upgrade,
/// and `None` when a page was served and the connection is finished.
pub fn handle_http(mut stream: TcpStream, token: &str) -> std::io::Result<Option<WebSocketStream>> {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let request = read_request(&mut stream)?;

    if let Some(key) = request.websocket_key {
        let accept = derive_accept(&key);
        let response = format!(
            "HTTP/1.1 101 Switching Protocols\r\n\
             Upgrade: websocket\r\n\
             Connection: Upgrade\r\n\
             Sec-WebSocket-Accept: {accept}\r\n\r\n"
        );
        stream.write_all(response.as_bytes())?;
        stream.flush()?;
        let _ = stream.set_read_timeout(None);
        stream.set_nonblocking(true)?;
        let socket = WebSocket::from_raw_socket(stream, Role::Server, None);
        return Ok(Some(WebSocketStream { socket }));
    }

    // The token is baked into the page so the socket can authenticate itself
    // without the visitor typing anything.
    let page_token = request
        .path
        .strip_prefix("/s/")
        .map(|rest| rest.split('?').next().unwrap_or("").to_string());

    let (status, body, content_type) = match request.path.as_str() {
        "/health" => ("200 OK", "ok".to_string(), "text/plain; charset=utf-8"),
        path if path == "/" || path.starts_with("/s/") => {
            let supplied = page_token.unwrap_or_default();
            // Serving the page is harmless without a token; the socket still
            // checks it. Passing it through just saves a manual paste.
            let injected = if supplied == token { token } else { "" };
            (
                "200 OK",
                CLIENT_HTML.replace("__MIV_TOKEN__", injected),
                "text/html; charset=utf-8",
            )
        }
        _ => (
            "404 Not Found",
            "not found".to_string(),
            "text/plain; charset=utf-8",
        ),
    };

    let response = format!(
        "HTTP/1.1 {status}\r\n\
         Content-Type: {content_type}\r\n\
         Content-Length: {}\r\n\
         Cache-Control: no-store\r\n\
         Connection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes())?;
    stream.flush()?;
    Ok(None)
}

fn read_request(stream: &mut TcpStream) -> std::io::Result<Request> {
    let mut raw = Vec::new();
    let mut chunk = [0u8; 1024];
    // Headers only; a request larger than this is not one we serve.
    while raw.len() < 16 * 1024 {
        let read = stream.read(&mut chunk)?;
        if read == 0 {
            break;
        }
        raw.extend_from_slice(&chunk[..read]);
        if raw.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
    }

    let text = String::from_utf8_lossy(&raw);
    let mut lines = text.split("\r\n");
    let request_line = lines.next().unwrap_or_default();
    let path = request_line
        .split_whitespace()
        .nth(1)
        .unwrap_or("/")
        .to_string();

    let mut websocket_key = None;
    let mut upgrading = false;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let (name, value) = (name.trim().to_ascii_lowercase(), value.trim());
        match name.as_str() {
            "sec-websocket-key" => websocket_key = Some(value.to_string()),
            "upgrade" if value.eq_ignore_ascii_case("websocket") => upgrading = true,
            _ => {}
        }
    }

    Ok(Request {
        path,
        websocket_key: upgrading.then_some(websocket_key).flatten(),
    })
}

fn derive_accept(key: &str) -> String {
    let mut hasher = Sha1::new();
    hasher.update(key.as_bytes());
    hasher.update(WEBSOCKET_GUID.as_bytes());
    base64::engine::general_purpose::STANDARD.encode(hasher.finalize())
}

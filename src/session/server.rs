//! The session listener.
//!
//! One thread accepts connections; each connection gets a thread that both
//! reads client messages and writes queued frames, polling so neither
//! direction can block the other. Everything reaches the editor through a
//! channel, so the main loop never blocks on a slow or hostile client.

use super::protocol::{ClientMessage, ServerMessage, PROTOCOL_VERSION};
use super::{web, SessionEvent};
use std::io::{ErrorKind, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

/// How often a connection thread checks each direction. Small enough to be
/// imperceptible, large enough not to spin a core.
const POLL: Duration = Duration::from_millis(5);

pub struct Listener {
    pub events: Receiver<SessionEvent>,
    pub shutdown: Arc<AtomicBool>,
    pub address: String,
}

/// Bind and start accepting. Returns immediately; work happens on threads.
pub fn start(
    bind: &str,
    port: u16,
    token: String,
    max_participants: usize,
) -> std::io::Result<Listener> {
    let listener = TcpListener::bind((bind, port))?;
    let address = listener.local_addr()?.to_string();
    listener.set_nonblocking(true)?;

    let (events_tx, events_rx) = channel();
    let shutdown = Arc::new(AtomicBool::new(false));
    let next_id = Arc::new(AtomicU32::new(1));

    {
        let shutdown = Arc::clone(&shutdown);
        let next_id = Arc::clone(&next_id);
        thread::spawn(move || {
            while !shutdown.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let id = next_id.fetch_add(1, Ordering::Relaxed);
                        if id as usize > max_participants {
                            reject(stream, "session is full");
                            continue;
                        }
                        let events = events_tx.clone();
                        let token = token.clone();
                        let shutdown = Arc::clone(&shutdown);
                        thread::spawn(move || {
                            serve_connection(stream, id, token, events, shutdown);
                        });
                    }
                    Err(e) if e.kind() == ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(40));
                    }
                    Err(_) => break,
                }
            }
        });
    }

    Ok(Listener {
        events: events_rx,
        shutdown,
        address,
    })
}

fn reject(mut stream: TcpStream, reason: &str) {
    let message = ServerMessage::Closed {
        reason: reason.to_string(),
    };
    if let Ok(text) = serde_json::to_string(&message) {
        let _ = writeln!(stream, "{text}");
    }
}

/// Either a newline-delimited JSON stream (the terminal client) or a
/// WebSocket (the browser). Both carry the same messages.
pub enum Transport {
    Lines(Box<LineStream>),
    Web(Box<web::WebSocketStream>),
}

impl Transport {
    fn receive(&mut self) -> std::io::Result<Option<String>> {
        match self {
            Transport::Lines(stream) => stream.next_line(),
            Transport::Web(stream) => stream.receive(),
        }
    }

    fn send(&mut self, text: &str) -> std::io::Result<()> {
        match self {
            Transport::Lines(stream) => stream.send_line(text),
            Transport::Web(stream) => stream.send(text),
        }
    }
}

/// Newline-delimited JSON over a non-blocking socket.
pub struct LineStream {
    stream: TcpStream,
    pending: Vec<u8>,
}

impl LineStream {
    pub fn new(stream: TcpStream) -> Self {
        Self {
            stream,
            pending: Vec::new(),
        }
    }

    fn next_line(&mut self) -> std::io::Result<Option<String>> {
        loop {
            if let Some(position) = self.pending.iter().position(|b| *b == b'\n') {
                let line: Vec<u8> = self.pending.drain(..=position).collect();
                let text = String::from_utf8_lossy(&line[..line.len() - 1])
                    .trim_end_matches('\r')
                    .to_string();
                if text.is_empty() {
                    continue;
                }
                return Ok(Some(text));
            }
            let mut chunk = [0u8; 8192];
            match self.stream.read(&mut chunk) {
                Ok(0) => return Err(ErrorKind::UnexpectedEof.into()),
                Ok(n) => self.pending.extend_from_slice(&chunk[..n]),
                Err(e) if e.kind() == ErrorKind::WouldBlock => return Ok(None),
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            }
        }
    }

    fn send_line(&mut self, text: &str) -> std::io::Result<()> {
        self.stream.write_all(text.as_bytes())?;
        self.stream.write_all(b"\n")?;
        self.stream.flush()
    }
}

fn serve_connection(
    stream: TcpStream,
    id: u32,
    token: String,
    events: Sender<SessionEvent>,
    shutdown: Arc<AtomicBool>,
) {
    let _ = stream.set_nodelay(true);

    // The first bytes tell us which client this is: a browser sends an HTTP
    // request line, the terminal client sends JSON.
    let is_http = peek_is_http(&stream);
    let mut transport = if is_http {
        match web::handle_http(stream, &token) {
            Ok(Some(socket)) => Transport::Web(Box::new(socket)),
            // A plain page request was served and the connection is done.
            Ok(None) => return,
            Err(_) => return,
        }
    } else {
        if stream.set_nonblocking(true).is_err() {
            return;
        }
        Transport::Lines(Box::new(LineStream::new(stream)))
    };

    let (outbound_tx, outbound_rx) = channel::<ServerMessage>();
    let mut greeted = false;

    loop {
        if shutdown.load(Ordering::Relaxed) {
            let _ = send(
                &mut transport,
                &ServerMessage::Closed {
                    reason: "the host ended the session".to_string(),
                },
            );
            break;
        }

        // Inbound.
        match transport.receive() {
            Ok(Some(line)) => match serde_json::from_str::<ClientMessage>(&line) {
                Ok(ClientMessage::Hello {
                    version,
                    token: offered,
                    name,
                    cols,
                    rows,
                }) => {
                    if version != PROTOCOL_VERSION {
                        let _ = send(&mut transport, &ServerMessage::Closed {
                            reason: format!(
                                "protocol mismatch: host speaks {PROTOCOL_VERSION}, client speaks {version}"
                            ),
                        });
                        break;
                    }
                    if offered != token {
                        let _ = send(
                            &mut transport,
                            &ServerMessage::Closed {
                                reason: "invalid session token".to_string(),
                            },
                        );
                        break;
                    }
                    if greeted {
                        continue;
                    }
                    greeted = true;
                    let is_web = matches!(transport, Transport::Web(_));
                    if events
                        .send(SessionEvent::Joined {
                            id,
                            name: sanitize_name(&name),
                            cols,
                            rows,
                            is_web,
                            outbound: outbound_tx.clone(),
                        })
                        .is_err()
                    {
                        break;
                    }
                }
                Ok(ClientMessage::Bye) => break,
                Ok(message) => {
                    if !greeted {
                        break;
                    }
                    if events.send(SessionEvent::Message { id, message }).is_err() {
                        break;
                    }
                }
                // A malformed frame is ignored rather than killing the
                // connection; a client version skew should degrade, not drop.
                Err(_) => {}
            },
            Ok(None) => {}
            Err(_) => break,
        }

        // Outbound.
        loop {
            match outbound_rx.try_recv() {
                Ok(message) => {
                    if send(&mut transport, &message).is_err() {
                        let _ = events.send(SessionEvent::Left { id });
                        return;
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    let _ = events.send(SessionEvent::Left { id });
                    return;
                }
            }
        }

        thread::sleep(POLL);
    }

    let _ = events.send(SessionEvent::Left { id });
}

fn send(transport: &mut Transport, message: &ServerMessage) -> std::io::Result<()> {
    let text = serde_json::to_string(message)
        .map_err(|e| std::io::Error::new(ErrorKind::InvalidData, e))?;
    transport.send(&text)
}

fn peek_is_http(stream: &TcpStream) -> bool {
    let mut probe = [0u8; 4];
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    match stream.peek(&mut probe) {
        Ok(n) if n >= 3 => probe.starts_with(b"GET") || probe.starts_with(b"POS"),
        _ => false,
    }
}

/// Names are shown to other participants, so keep them short and printable.
fn sanitize_name(name: &str) -> String {
    let cleaned: String = name.chars().filter(|c| !c.is_control()).take(24).collect();
    let cleaned = cleaned.trim().to_string();
    if cleaned.is_empty() {
        "guest".to_string()
    } else {
        cleaned
    }
}

/// An unguessable session token. Anyone who has it can connect, so it is the
/// only thing standing between a listener and the buffer.
pub fn generate_token() -> String {
    let mut bytes = [0u8; 12];
    let seeded = std::fs::File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(&mut bytes).map(|_| ()))
        .is_ok();
    if !seeded {
        // Fallback for platforms without /dev/urandom.
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let pid = std::process::id() as u128;
        let mixed = nanos.wrapping_mul(6364136223846793005).wrapping_add(pid);
        for (index, byte) in bytes.iter_mut().enumerate() {
            *byte = (mixed >> (index * 8)) as u8;
        }
    }
    use std::fmt::Write;
    bytes.iter().fold(String::new(), |mut out, byte| {
        let _ = write!(out, "{byte:02x}");
        out
    })
}

//! Shared editing sessions.
//!
//! A session turns one running editor into a host that other terminals and
//! browsers connect to. There is exactly one copy of the text — the host's —
//! so participants exchange *keystrokes and screens*, never document state.
//! That removes the whole class of convergence problems a peer-to-peer editor
//! has, at the cost of requiring the host to stay up.
//!
//! What makes each participant feel like they have their own editor is
//! [`PerUser`]: the small bundle of state that belongs to a person rather than
//! to the document — cursor, viewport, mode, pending keys, prompt. Swapping
//! that bundle in and out around a command means every existing command works
//! per-participant without knowing sessions exist.

pub mod protocol;
pub mod server;
pub mod web;

use crate::app::{App, Message, Prompt};
use crate::history::Change;
use crate::keymap::Pending;
use crate::mode::Mode;
use crate::motion::FindTarget;
use crate::text::{self, Position};
use protocol::{Access, RosterEntry, ServerMessage, WireCell};
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer as ScreenBuffer;
use ratatui::Terminal;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;

/// The host is always participant zero.
pub const HOST_ID: u32 = 0;

/// Distinct colours for participant cursors, as 256-colour indices.
const CURSOR_COLORS: [u8; 6] = [213, 114, 215, 117, 210, 156];

/// State that belongs to a person rather than to the document.
#[derive(Clone)]
pub struct PerUser {
    pub buffer_index: usize,
    pub cursor: Position,
    pub desired_col: usize,
    pub view_top: usize,
    pub view_left: usize,
    pub mode: Mode,
    pub pending: Pending,
    pub visual_anchor: Position,
    pub last_find: Option<FindTarget>,
    pub prompt: Option<Prompt>,
    pub message: Option<Message>,
}

impl PerUser {
    pub fn at(buffer_index: usize) -> Self {
        Self {
            buffer_index,
            cursor: Position::default(),
            desired_col: 0,
            view_top: 0,
            view_left: 0,
            mode: Mode::Normal,
            pending: Pending::default(),
            visual_anchor: Position::default(),
            last_find: None,
            prompt: None,
            message: None,
        }
    }
}

/// Lift the live editor state into a [`PerUser`].
fn capture(app: &App) -> PerUser {
    let buffer = app.buffer();
    PerUser {
        buffer_index: app.current,
        cursor: buffer.cursor,
        desired_col: buffer.desired_col,
        view_top: buffer.view_top,
        view_left: buffer.view_left,
        mode: app.mode,
        pending: app.pending.clone(),
        visual_anchor: app.visual_anchor,
        last_find: app.last_find,
        prompt: app.prompt.clone(),
        message: app.message.clone(),
    }
}

fn restore(app: &mut App, state: &PerUser) {
    app.current = state.buffer_index.min(app.buffers.len().saturating_sub(1));
    app.mode = state.mode;
    app.pending = state.pending.clone();
    app.visual_anchor = state.visual_anchor;
    app.last_find = state.last_find;
    app.prompt = state.prompt.clone();
    app.message = state.message.clone();
    let buffer = app.buffer_mut();
    buffer.cursor = state.cursor;
    buffer.desired_col = state.desired_col;
    buffer.view_top = state.view_top;
    buffer.view_left = state.view_left;
}

/// Where a participant's caret is, in a form the renderer can read without
/// borrowing the session.
#[derive(Clone)]
pub struct RemoteCursor {
    pub id: u32,
    pub name: String,
    pub color: u8,
    pub buffer_index: usize,
    pub cursor: Position,
    pub selection: Option<(Position, Position)>,
}

pub struct Participant {
    pub id: u32,
    pub name: String,
    pub color: u8,
    pub access: Access,
    pub state: PerUser,
    /// Mirror this participant's viewport instead of keeping your own.
    pub following: Option<u32>,
    /// Absent for the host, who draws on the real terminal.
    pub outbound: Option<Sender<ServerMessage>>,
    pub terminal: Option<Terminal<TestBackend>>,
    pub previous_frame: Option<ScreenBuffer>,
    pub cols: u16,
    pub rows: u16,
    pub is_web: bool,
}

impl Participant {
    fn host(name: String) -> Self {
        Self {
            id: HOST_ID,
            name,
            color: CURSOR_COLORS[0],
            access: Access::Write,
            state: PerUser::at(0),
            following: None,
            outbound: None,
            terminal: None,
            previous_frame: None,
            cols: 0,
            rows: 0,
            is_web: false,
        }
    }

    pub fn is_remote(&self) -> bool {
        self.outbound.is_some()
    }

    fn send(&self, message: ServerMessage) {
        if let Some(outbound) = &self.outbound {
            let _ = outbound.send(message);
        }
    }
}

pub enum SessionEvent {
    Joined {
        id: u32,
        name: String,
        cols: u16,
        rows: u16,
        is_web: bool,
        outbound: Sender<ServerMessage>,
    },
    Message {
        id: u32,
        message: protocol::ClientMessage,
    },
    Left {
        id: u32,
    },
}

pub struct Session {
    pub participants: Vec<Participant>,
    /// Index into `participants` whose state is currently live in the editor.
    live: usize,
    pub token: String,
    pub address: String,
    pub events: Receiver<SessionEvent>,
    pub shutdown: Arc<AtomicBool>,
    pub default_access: Access,
    pub announce: bool,
    pub max_participants: usize,
}

impl Session {
    pub fn new(
        host_name: String,
        token: String,
        address: String,
        events: Receiver<SessionEvent>,
        shutdown: Arc<AtomicBool>,
        default_access: Access,
        announce: bool,
        max_participants: usize,
    ) -> Self {
        Self {
            participants: vec![Participant::host(host_name)],
            live: 0,
            token,
            address,
            events,
            shutdown,
            default_access,
            announce,
            max_participants,
        }
    }

    pub fn url(&self) -> String {
        format!("http://{}/s/{}", self.address, self.token)
    }

    pub fn index_of(&self, id: u32) -> Option<usize> {
        self.participants.iter().position(|p| p.id == id)
    }

    pub fn guests(&self) -> usize {
        self.participants.len() - 1
    }

    pub fn broadcast(&self, message: ServerMessage) {
        for participant in &self.participants {
            participant.send(message.clone());
        }
    }

    pub fn roster(&self, app: &App) -> Vec<RosterEntry> {
        self.participants
            .iter()
            .map(|p| RosterEntry {
                id: p.id,
                name: p.name.clone(),
                color: p.color,
                access: p.access,
                mode: p.state.mode.label().to_string(),
                line: p.state.cursor.line + 1,
                file: app
                    .buffers
                    .get(p.state.buffer_index)
                    .map(|b| b.short_name())
                    .unwrap_or_default(),
                is_host: p.id == HOST_ID,
            })
            .collect()
    }

    pub fn stop(&self) {
        self.shutdown.store(true, Ordering::Relaxed);
        self.broadcast(ServerMessage::Closed {
            reason: "the host ended the session".to_string(),
        });
    }
}

/// Make `target`'s state the live editor state.
fn activate(app: &mut App, session: &mut Session, target: usize) {
    if session.live == target {
        return;
    }
    session.participants[session.live].state = capture(app);
    let state = session.participants[target].state.clone();
    restore(app, &state);
    session.live = target;
    app.rendering_as = session.participants[target].id;
}

/// How an offset in the text moves when a splice is applied before it.
pub fn shift_offset(offset: usize, change: &Change) -> usize {
    let removed = change.removed.chars().count();
    let inserted = change.inserted.chars().count();
    if offset <= change.at {
        offset
    } else if offset >= change.at + removed {
        offset - removed + inserted
    } else {
        // The character it pointed at was deleted; fall back to the splice.
        change.at + inserted.min(offset - change.at)
    }
}

/// Run `action` as the given participant, then move everyone else's caret to
/// keep it on the character it was pointing at.
pub fn dispatch(app: &mut App, actor: u32, action: impl FnOnce(&mut App)) {
    let Some(mut session) = app.session.take() else {
        action(app);
        return;
    };
    let Some(index) = session.index_of(actor) else {
        app.session = Some(session);
        return;
    };

    let buffer_index = session.participants[index].state.buffer_index;
    // Snapshot the others as character offsets against the text as it is now,
    // because a Position means nothing once the lines above it change.
    let mut snapshots: Vec<(usize, usize, usize)> = Vec::new();
    for (i, participant) in session.participants.iter().enumerate() {
        if i == index || participant.state.buffer_index != buffer_index {
            continue;
        }
        let rope = &app.buffers[buffer_index].rope;
        snapshots.push((
            i,
            text::pos_to_char(rope, participant.state.cursor),
            text::pos_to_char(rope, participant.state.visual_anchor),
        ));
    }

    activate(app, &mut session, index);
    action(app);

    let edits = app.buffers[buffer_index].drain_edits();
    if !edits.is_empty() {
        for (i, mut cursor, mut anchor) in snapshots {
            for change in &edits {
                cursor = shift_offset(cursor, change);
                anchor = shift_offset(anchor, change);
            }
            let rope = &app.buffers[buffer_index].rope;
            let allow_eol = session.participants[i].state.mode.allows_eol();
            let moved = text::clamp(rope, text::char_to_pos(rope, cursor), allow_eol);
            let participant = &mut session.participants[i];
            participant.state.cursor = moved;
            participant.state.desired_col = moved.col;
            participant.state.visual_anchor = text::char_to_pos(rope, anchor);
        }
    }

    // Leave the host's state live so the real terminal draws the host's view.
    activate(app, &mut session, 0);
    app.session = Some(session);
    refresh_remote_cursors(app);
}

/// Publish every participant's caret for the renderer, which cannot borrow the
/// session while it is drawing.
pub fn refresh_remote_cursors(app: &mut App) {
    let Some(session) = &app.session else {
        app.remote_cursors.clear();
        return;
    };
    app.remote_cursors = session
        .participants
        .iter()
        .map(|p| RemoteCursor {
            id: p.id,
            name: p.name.clone(),
            color: p.color,
            buffer_index: p.state.buffer_index,
            cursor: p.state.cursor,
            selection: p.state.mode.is_visual().then(|| {
                if p.state.visual_anchor <= p.state.cursor {
                    (p.state.visual_anchor, p.state.cursor)
                } else {
                    (p.state.cursor, p.state.visual_anchor)
                }
            }),
        })
        .collect();
}

/// Draw and send one frame to every connected participant.
///
/// Each gets a genuinely independent view: their own viewport, mode line and
/// overlays, rendered by the same code that draws the host's screen. Only the
/// cells that changed since their last frame go over the wire.
pub fn render_remote_frames(app: &mut App) {
    let Some(mut session) = app.session.take() else {
        return;
    };

    for index in 0..session.participants.len() {
        if !session.participants[index].is_remote() {
            continue;
        }

        // Follow mode: borrow the followed participant's viewport and caret.
        if let Some(target) = session.participants[index].following {
            if let Some(target_index) = session.index_of(target) {
                let followed = session.participants[target_index].state.clone();
                let me = &mut session.participants[index].state;
                me.buffer_index = followed.buffer_index;
                me.cursor = followed.cursor;
                me.view_top = followed.view_top;
                me.view_left = followed.view_left;
            }
        }

        activate(app, &mut session, index);
        let participant = &mut session.participants[index];
        let Some(terminal) = participant.terminal.as_mut() else {
            continue;
        };

        let Ok(completed) = terminal.draw(|frame| crate::ui::draw(frame, app)) else {
            continue;
        };
        let rendered = completed.buffer.clone();
        let cursor = terminal
            .get_cursor_position()
            .ok()
            .map(|position| [position.x, position.y]);

        let full = participant.previous_frame.is_none();
        let cells: Vec<WireCell> = match &participant.previous_frame {
            Some(previous) => previous
                .diff(&rendered)
                .into_iter()
                .map(|(x, y, cell)| WireCell::from_cell(x, y, cell))
                .collect(),
            None => rendered
                .content
                .iter()
                .enumerate()
                .map(|(offset, cell)| {
                    let x = (offset as u16) % participant.cols.max(1);
                    let y = (offset as u16) / participant.cols.max(1);
                    WireCell::from_cell(x, y, cell)
                })
                .collect(),
        };
        participant.previous_frame = Some(rendered);

        if !cells.is_empty() || full {
            participant.send(ServerMessage::Frame {
                full,
                cols: participant.cols,
                rows: participant.rows,
                cells,
                cursor,
            });
        }
    }

    activate(app, &mut session, 0);
    app.session = Some(session);
}

/// Send the participant list to everyone; cheap and only on membership or
/// access changes.
pub fn broadcast_roster(app: &mut App) {
    let Some(session) = app.session.take() else {
        return;
    };
    let roster = session.roster(app);
    session.broadcast(ServerMessage::Roster {
        participants: roster,
    });
    app.session = Some(session);
}

pub fn next_color(existing: usize) -> u8 {
    CURSOR_COLORS[existing % CURSOR_COLORS.len()]
}

/// Make a participant's terminal, sized to what they reported.
pub fn make_terminal(cols: u16, rows: u16) -> Option<Terminal<TestBackend>> {
    let cols = cols.clamp(20, 500);
    let rows = rows.clamp(4, 200);
    Terminal::new(TestBackend::new(cols, rows)).ok()
}

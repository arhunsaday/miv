//! The `:share` family of commands, and the bridge between the listener's
//! events and the editor.

use super::protocol::{Access, ClientMessage, ServerMessage};
use super::{
    broadcast_roster, dispatch, make_terminal, next_color, refresh_remote_cursors, server,
    Participant, PerUser, Session, SessionEvent, HOST_ID,
};
use crate::app::{App, Overlay};
use crate::keymap;
use crate::keys;

/// `:share [--write] [--bind ADDR] [--port N]`
pub fn share(app: &mut App, args: &str) {
    if app.session.is_some() {
        let url = app.session.as_ref().unwrap().url();
        app.set_message(format!("already sharing: {url}"));
        return;
    }

    let settings = app.config.session.clone();
    let mut bind = settings.bind.clone();
    let mut port = settings.port;
    let mut access = settings.default_access;

    let mut tokens = args.split_whitespace().peekable();
    while let Some(argument) = tokens.next() {
        match argument {
            "--write" | "-w" => access = Access::Write,
            "--read" | "-r" => access = Access::Read,
            "--bind" => match tokens.next() {
                Some(value) => bind = value.to_string(),
                None => return app.set_error("--bind needs an address"),
            },
            "--port" => match tokens.next().and_then(|v| v.parse().ok()) {
                Some(value) => port = value,
                None => return app.set_error("--port needs a number"),
            },
            other => return app.set_error(format!("unknown option: {other}")),
        }
    }

    let token = server::generate_token();
    let listener = match server::start(&bind, port, token.clone(), settings.max_participants) {
        Ok(listener) => listener,
        Err(e) => return app.set_error(format!("could not start the session: {e}")),
    };

    let address = listener.address.clone();
    let session = Session::new(
        settings.name.clone(),
        token.clone(),
        address.clone(),
        listener.events,
        listener.shutdown,
        access,
        settings.announce,
        &app.config.sidebar,
    );
    let url = session.url();
    app.session = Some(session);
    refresh_remote_cursors(app);

    let exposed = !bind.starts_with("127.") && bind != "localhost" && bind != "::1";
    let warning = if exposed {
        format!("  ⚠ bound to {bind}, reachable from the network")
    } else {
        String::new()
    };
    app.overlay = Some(Overlay {
        title: "Sharing this buffer".to_string(),
        lines: vec![
            String::new(),
            "  Browser".to_string(),
            format!("    {url}"),
            String::new(),
            "  Another terminal (same machine, or over an SSH tunnel)".to_string(),
            format!("    miv --attach {address} --token {token}"),
            String::new(),
            "  Over SSH from your laptop".to_string(),
            format!(
                "    ssh -L {0}:{1} <host>   then open http://localhost:{0}/s/{2}",
                address.rsplit(':').next().unwrap_or("7420"),
                address,
                token
            ),
            String::new(),
            format!(
                "  Guests join {} by default. :grant <name> to let someone edit.",
                access.label()
            ),
            format!("  :who lists participants · :unshare ends the session{warning}"),
        ],
        scroll: 0,
    });
}

pub fn unshare(app: &mut App) {
    match app.session.take() {
        Some(session) => {
            session.stop();
            refresh_remote_cursors(app);
            app.set_message("session ended");
        }
        None => app.set_error("not sharing"),
    }
}

pub fn who(app: &mut App) {
    let Some(session) = app.session.take() else {
        return app.set_error("not sharing");
    };
    let mut lines = vec![format!("  {}", session.url()), String::new()];
    for entry in session.roster(app) {
        let following = session
            .participants
            .iter()
            .find(|p| p.id == entry.id)
            .and_then(|p| p.following)
            .and_then(|target| session.participants.iter().find(|p| p.id == target))
            .map(|target| format!(" · following {}", target.name))
            .unwrap_or_default();
        lines.push(format!(
            "  {:<16} {:<10} {:<9} {:<12} {}:{}{}",
            entry.name,
            if entry.is_host {
                "host"
            } else {
                entry.access.label()
            },
            entry.via,
            entry.mode.to_lowercase(),
            entry.file,
            entry.line,
            following
        ));
    }
    app.overlay = Some(Overlay {
        title: "Participants".to_string(),
        lines,
        scroll: 0,
    });
    app.session = Some(session);
}

fn find_participant(session: &Session, name: &str) -> Option<usize> {
    session
        .participants
        .iter()
        .position(|p| p.name.eq_ignore_ascii_case(name))
        .or_else(|| name.parse::<u32>().ok().and_then(|id| session.index_of(id)))
}

pub fn set_access(app: &mut App, name: &str, access: Access) {
    let Some(mut session) = app.session.take() else {
        app.set_error("not sharing");
        return;
    };
    match find_participant(&session, name) {
        Some(index) if session.participants[index].id == HOST_ID => {
            app.set_error("the host always has write access");
        }
        Some(index) => {
            session.participants[index].access = access;
            let participant = &session.participants[index];
            participant.send(ServerMessage::Welcome {
                version: super::protocol::PROTOCOL_VERSION,
                id: participant.id,
                name: participant.name.clone(),
                access,
                host: session.participants[0].name.clone(),
                file: app.buffer().short_name(),
            });
            participant.send(ServerMessage::Notice {
                text: format!("the host set you to {}", access.label()),
            });
            app.set_message(format!("{} is now {}", participant.name, access.label()));
        }
        None => app.set_error(format!("no participant called {name}")),
    }
    app.session = Some(session);
    broadcast_roster(app);
}

pub fn follow(app: &mut App, name: &str) {
    let Some(mut session) = app.session.take() else {
        app.set_error("not sharing");
        return;
    };
    if name.is_empty() {
        session.participants[0].following = None;
        app.set_message("stopped following");
    } else {
        match find_participant(&session, name) {
            Some(index) => {
                let target = session.participants[index].id;
                session.participants[0].following = Some(target);
                app.set_message(format!("following {}", session.participants[index].name));
            }
            None => app.set_error(format!("no participant called {name}")),
        }
    }
    app.session = Some(session);
}

/// `:say` — a line of text in everyone's message area. Pairing needs a way to
/// talk that is not a second window.
pub fn say(app: &mut App, text: &str) {
    let Some(session) = app.session.take() else {
        app.set_error("not sharing");
        return;
    };
    let from = session.participants[0].name.clone();
    session.broadcast(ServerMessage::Notice {
        text: format!("{from}: {text}"),
    });
    app.session = Some(session);
    app.set_message(format!("{from}: {text}"));
}

/// Drain everything the listener has queued and apply it.
pub fn poll_events(app: &mut App) -> bool {
    let Some(session) = app.session.as_ref() else {
        return false;
    };
    let mut events = Vec::new();
    while let Ok(event) = session.events.try_recv() {
        events.push(event);
    }
    if events.is_empty() {
        return false;
    }
    for event in events {
        handle_event(app, event);
    }
    refresh_remote_cursors(app);
    broadcast_roster(app);
    true
}

fn handle_event(app: &mut App, event: SessionEvent) {
    match event {
        SessionEvent::Joined {
            id,
            name,
            cols,
            rows,
            is_web,
            outbound,
        } => {
            let Some(mut session) = app.session.take() else {
                return;
            };
            let color = next_color(session.participants.len());
            let access = session.default_access;
            // A guest lands where the host is looking.
            let mut state = PerUser::at(app.current, &app.config.sidebar);
            let cursor = app.buffer().cursor;
            let view_top = app.buffer().view_top;
            state.set_cursor(cursor);
            state.workspace.focused_mut().view_top = view_top;

            let _ = outbound.send(ServerMessage::Welcome {
                version: super::protocol::PROTOCOL_VERSION,
                id,
                name: name.clone(),
                access,
                host: session.participants[0].name.clone(),
                file: app.buffer().short_name(),
            });
            session.participants.push(Participant {
                id,
                name: name.clone(),
                color,
                access,
                state,
                following: None,
                outbound: Some(outbound),
                terminal: make_terminal(cols, rows),
                previous_frame: None,
                cols: cols.clamp(20, 500),
                rows: rows.clamp(4, 200),
                is_web,
            });
            let announce = session.announce;
            app.session = Some(session);
            if announce {
                let via = if is_web { "browser" } else { "terminal" };
                app.set_message(format!(
                    "{name} joined from a {via} ({}) — :grant {name} to let them edit",
                    access.label()
                ));
            }
        }
        SessionEvent::Left { id } => {
            let Some(mut session) = app.session.take() else {
                return;
            };
            let departed = session
                .index_of(id)
                .map(|index| session.participants.remove(index).name);
            // Anyone following the departed participant goes back to their own view.
            for participant in &mut session.participants {
                if participant.following == Some(id) {
                    participant.following = None;
                }
            }
            let announce = session.announce;
            app.session = Some(session);
            if let Some(name) = departed {
                if announce {
                    app.set_message(format!("{name} left"));
                }
            }
        }
        SessionEvent::Message { id, message } => receive(app, id, message),
    }
}

/// Apply one message from a participant. Public so tests can drive a session
/// without a socket.
pub fn receive(app: &mut App, id: u32, message: ClientMessage) {
    match message {
        ClientMessage::Keys { keys } => {
            for key in keys::parse(&keys) {
                // A refused key abandons the rest of the batch: `:w /tmp/x`
                // arrives as one message, and letting the tail run as normal
                // mode commands would be a surprising way to refuse it.
                if !apply_remote_key(app, id, key) {
                    break;
                }
            }
        }
        ClientMessage::Paste { text } => {
            if access_of(app, id) != Some(Access::Write) {
                notify(app, id, "read-only: ask the host for write access");
                return;
            }
            dispatch(app, id, |app| {
                app.begin_input();
                keymap::handle_paste(app, &text);
            });
        }
        ClientMessage::Resize { cols, rows } => {
            let Some(mut session) = app.session.take() else {
                return;
            };
            if let Some(index) = session.index_of(id) {
                let participant = &mut session.participants[index];
                participant.cols = cols.clamp(20, 500);
                participant.rows = rows.clamp(4, 200);
                participant.terminal = make_terminal(cols, rows);
                // Force a full repaint: the client threw its grid away.
                participant.previous_frame = None;
            }
            app.session = Some(session);
        }
        ClientMessage::RequestWrite => {
            let name = name_of(app, id).unwrap_or_else(|| id.to_string());
            app.set_message(format!("{name} is asking to edit — :grant {name}"));
        }
        ClientMessage::Hello { .. } | ClientMessage::Bye => {}
    }
}

fn mode_of(app: &App, id: u32) -> Option<crate::mode::Mode> {
    let session = app.session.as_ref()?;
    let index = session.index_of(id)?;
    Some(session.participants[index].state.mode)
}

fn access_of(app: &App, id: u32) -> Option<Access> {
    let session = app.session.as_ref()?;
    let index = session.index_of(id)?;
    Some(session.participants[index].access)
}

fn name_of(app: &App, id: u32) -> Option<String> {
    let session = app.session.as_ref()?;
    let index = session.index_of(id)?;
    Some(session.participants[index].name.clone())
}

fn notify(app: &mut App, id: u32, text: &str) {
    let Some(session) = app.session.as_ref() else {
        return;
    };
    if let Some(index) = session.index_of(id) {
        session.participants[index].send(ServerMessage::Notice {
            text: text.to_string(),
        });
    }
}

/// Apply one key from a participant.
///
/// A read-only participant may move around freely — that is the point of
/// spectating — but must not change anything. Rather than trying to classify
/// keys, we let the command run and roll back any transaction it produced,
/// which is airtight because every mutation goes through the history.
/// Returns false when the key was refused, so the rest of the batch is dropped.
fn apply_remote_key(app: &mut App, id: u32, key: crossterm::event::KeyEvent) -> bool {
    let read_only = access_of(app, id) != Some(Access::Write);
    // Only refuse a colon that would *open* the command line. In insert mode
    // or inside a search prompt it is just a character, and a guest editing
    // YAML needs to be able to type one.
    let opens_command_line = matches!(
        mode_of(app, id),
        Some(crate::mode::Mode::Normal) | Some(crate::mode::Mode::Visual(_))
    );
    if matches!(key.code, crossterm::event::KeyCode::Char(':'))
        && opens_command_line
        && !app.config.session.guest_commands
    {
        let reason = if read_only {
            "read-only: ex commands are disabled"
        } else {
            "ex commands are disabled for guests (session.guest_commands)"
        };
        notify(app, id, reason);
        return false;
    }

    let mut blocked = false;
    dispatch(app, id, |app| {
        let revision = app.buffer().history.revision();
        app.begin_input();
        keymap::handle(app, key);
        while let Some(queued) = app.queue.pop_front() {
            keymap::handle(app, queued);
        }
        // A guest must never end the host's editor.
        app.should_quit = false;
        if read_only {
            while app.buffer().history.revision() > revision {
                app.buffer_mut().undo();
                blocked = true;
            }
        }
    });
    if blocked {
        notify(
            app,
            id,
            "read-only: ask the host to :grant you write access",
        );
    }
    true
}

//! Session tests.
//!
//! These drive a real [`Session`] without opening a socket, so they cover the
//! parts that actually carry risk: per-participant state, cursor
//! transformation under concurrent edits, and the read-only guarantee.

use super::commands;
use super::protocol::{Access, ClientMessage, ServerMessage};
use super::{dispatch, render_remote_frames, shift_offset, Session, SessionEvent, HOST_ID};
use crate::app::App;
use crate::config::Config;
use crate::core::history::Change;
use crate::core::text::Position;
use crate::mode::Mode;
use ropey::Rope;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

fn hosted(content: &str) -> (App, Sender<SessionEvent>) {
    let mut app = App::new(Config::default(), &[], None).expect("app");
    let mut text = content.to_string();
    if !text.ends_with('\n') {
        text.push('\n');
    }
    app.buffer_mut().rope = Rope::from_str(&text);
    app.viewport = crate::app::Viewport {
        height: 20,
        text_width: 80,
    };

    let (events_tx, events_rx) = channel();
    app.session = Some(Session::new(
        "host".to_string(),
        "token".to_string(),
        "127.0.0.1:0".to_string(),
        events_rx,
        Arc::new(AtomicBool::new(false)),
        Access::Read,
        false,
        &crate::config::SidebarConfig::default(),
    ));
    super::refresh_remote_cursors(&mut app);
    (app, events_tx)
}

fn join(
    app: &mut App,
    events: &Sender<SessionEvent>,
    id: u32,
    name: &str,
) -> Receiver<ServerMessage> {
    let (outbound, inbox) = channel();
    events
        .send(SessionEvent::Joined {
            id,
            name: name.to_string(),
            cols: 80,
            rows: 24,
            is_web: false,
            outbound,
        })
        .unwrap();
    commands::poll_events(app);
    inbox
}

/// Feed keys to a participant exactly as the socket would.
fn keys_from(app: &mut App, id: u32, keys: &str) {
    commands::receive(
        app,
        id,
        ClientMessage::Keys {
            keys: keys.to_string(),
        },
    );
}

/// Feed keys to the host through the same path the main loop uses.
fn keys_from_host(app: &mut App, keys: &str) {
    for key in crate::keys::parse(keys) {
        dispatch(app, HOST_ID, |app| {
            app.begin_input();
            crate::keymap::handle(app, key);
            while let Some(queued) = app.queue.pop_front() {
                crate::keymap::handle(app, queued);
            }
        });
    }
}

fn content(app: &App) -> String {
    app.buffer().rope.to_string()
}

fn cursor_of(app: &App, id: u32) -> Position {
    let session = app.session.as_ref().unwrap();
    let index = session.index_of(id).unwrap();
    session.participants[index].state.cursor()
}

fn grant(app: &mut App, name: &str) {
    commands::set_access(app, name, Access::Write);
}

// -- cursor transformation --------------------------------------------------

#[test]
fn an_offset_before_a_splice_does_not_move() {
    let change = Change {
        at: 10,
        removed: String::new(),
        inserted: "abc".to_string(),
    };
    assert_eq!(shift_offset(5, &change), 5);
    assert_eq!(shift_offset(10, &change), 10);
}

#[test]
fn an_offset_after_an_insert_moves_by_its_length() {
    let change = Change {
        at: 10,
        removed: String::new(),
        inserted: "abc".to_string(),
    };
    assert_eq!(shift_offset(11, &change), 14);
}

#[test]
fn an_offset_after_a_delete_moves_back() {
    let change = Change {
        at: 10,
        removed: "abcde".to_string(),
        inserted: String::new(),
    };
    assert_eq!(shift_offset(20, &change), 15);
}

#[test]
fn an_offset_inside_deleted_text_collapses_to_the_splice() {
    let change = Change {
        at: 10,
        removed: "abcde".to_string(),
        inserted: String::new(),
    };
    assert_eq!(shift_offset(12, &change), 10);
}

#[test]
fn an_offset_inside_replaced_text_stays_within_the_replacement() {
    let change = Change {
        at: 10,
        removed: "abcde".to_string(),
        inserted: "xy".to_string(),
    };
    assert_eq!(shift_offset(14, &change), 12);
}

// -- independent participants ----------------------------------------------

#[test]
fn a_guest_has_its_own_cursor_and_mode() {
    let (mut app, events) = hosted("alpha\nbeta\ngamma\ndelta");
    join(&mut app, &events, 1, "guest");

    keys_from_host(&mut app, "jj");
    keys_from(&mut app, 1, "G");

    assert_eq!(cursor_of(&app, HOST_ID).line, 2);
    assert_eq!(cursor_of(&app, 1).line, 3);

    // Modes are per participant too: the host can be inserting while a guest
    // is in normal mode.
    grant(&mut app, "guest");
    keys_from_host(&mut app, "i");
    assert_eq!(app.mode, Mode::Insert);
    keys_from(&mut app, 1, "v");
    // The host's live mode is untouched by the guest's.
    assert_eq!(app.mode, Mode::Insert);
    let session = app.session.as_ref().unwrap();
    let guest = session.index_of(1).unwrap();
    assert!(session.participants[guest].state.mode.is_visual());
}

#[test]
fn an_edit_moves_everyone_elses_cursor_with_the_text() {
    let (mut app, events) = hosted("one\ntwo\nthree\nfour");
    join(&mut app, &events, 1, "guest");
    grant(&mut app, "guest");

    // Guest sits on "four".
    keys_from(&mut app, 1, "G");
    assert_eq!(cursor_of(&app, 1), Position::new(3, 0));

    // Host deletes the first line; the guest should still be on "four".
    keys_from_host(&mut app, "ggdd");
    assert_eq!(content(&app), "two\nthree\nfour\n");
    assert_eq!(cursor_of(&app, 1), Position::new(2, 0));

    // And an insert above pushes them back down.
    keys_from_host(&mut app, "ggOzero<Esc>");
    assert_eq!(cursor_of(&app, 1), Position::new(3, 0));
}

#[test]
fn a_guests_edit_moves_the_hosts_cursor() {
    let (mut app, events) = hosted("one\ntwo\nthree");
    join(&mut app, &events, 1, "guest");
    grant(&mut app, "guest");

    keys_from_host(&mut app, "G");
    assert_eq!(cursor_of(&app, HOST_ID), Position::new(2, 0));

    keys_from(&mut app, 1, "ggdd");
    assert_eq!(content(&app), "two\nthree\n");
    assert_eq!(cursor_of(&app, HOST_ID), Position::new(1, 0));
}

#[test]
fn a_cursor_inside_deleted_text_lands_on_the_deletion() {
    let (mut app, events) = hosted("alpha beta gamma");
    join(&mut app, &events, 1, "guest");
    keys_from(&mut app, 1, "ww");
    assert_eq!(cursor_of(&app, 1), Position::new(0, 11));

    keys_from_host(&mut app, "dw");
    assert_eq!(content(&app), "beta gamma\n");
    assert_eq!(cursor_of(&app, 1), Position::new(0, 5));
}

// -- access control ---------------------------------------------------------

#[test]
fn a_read_only_guest_cannot_change_the_text() {
    let (mut app, events) = hosted("untouched");
    let inbox = join(&mut app, &events, 1, "watcher");

    keys_from(&mut app, 1, "dd");
    keys_from(&mut app, 1, "ihello<Esc>");
    keys_from(&mut app, 1, "x");
    assert_eq!(content(&app), "untouched\n");

    let notices: Vec<String> = inbox
        .try_iter()
        .filter_map(|message| match message {
            ServerMessage::Notice { text } => Some(text),
            _ => None,
        })
        .collect();
    assert!(
        notices.iter().any(|text| text.contains("read-only")),
        "expected a read-only notice, got {notices:?}"
    );
}

#[test]
fn a_read_only_guest_can_still_move_and_search() {
    let (mut app, events) = hosted("alpha\nbeta\ngamma");
    join(&mut app, &events, 1, "watcher");

    keys_from(&mut app, 1, "G");
    assert_eq!(cursor_of(&app, 1), Position::new(2, 0));
    keys_from(&mut app, 1, "gg");
    assert_eq!(cursor_of(&app, 1), Position::new(0, 0));
    // Searching is harmless and genuinely useful when spectating.
    keys_from(&mut app, 1, "/beta<CR>");
    assert_eq!(cursor_of(&app, 1), Position::new(1, 0));
    assert_eq!(content(&app), "alpha\nbeta\ngamma\n");
}

#[test]
fn a_read_only_guest_cannot_open_the_command_line() {
    let (mut app, events) = hosted("text");
    join(&mut app, &events, 1, "watcher");
    keys_from(&mut app, 1, ":");
    let session = app.session.as_ref().unwrap();
    let index = session.index_of(1).unwrap();
    assert_eq!(session.participants[index].state.mode, Mode::Normal);
}

#[test]
fn write_access_does_not_imply_running_commands() {
    // Editing text is a far smaller grant than being able to `:w` anywhere
    // the host can write, so the two are controlled separately.
    let (mut app, events) = hosted("alpha");
    let inbox = join(&mut app, &events, 1, "guest");
    grant(&mut app, "guest");

    keys_from(&mut app, 1, ":w /tmp/miv-should-not-exist<CR>");
    assert!(!std::path::Path::new("/tmp/miv-should-not-exist").exists());
    // The refused `:` also drops the rest of the batch, so the cursor has not
    // wandered off through `w`, `/tmp` and friends.
    assert_eq!(cursor_of(&app, 1), Position::new(0, 0));
    let notices: Vec<String> = inbox
        .try_iter()
        .filter_map(|message| match message {
            ServerMessage::Notice { text } => Some(text),
            _ => None,
        })
        .collect();
    assert!(
        notices.iter().any(|text| text.contains("guest_commands")),
        "{notices:?}"
    );

    // ...but editing still works.
    keys_from(&mut app, 1, "x");
    assert_eq!(content(&app), "lpha\n");

    // And a host who wants full trust can turn it on.
    app.config.session.guest_commands = true;
    keys_from(&mut app, 1, ":set nonumber<CR>");
    assert_eq!(
        app.config.editor.line_numbers,
        crate::config::LineNumbers::None
    );
}

#[test]
fn a_guest_can_still_type_a_colon() {
    // Blocking the `:` key outright made it impossible for a guest to type a
    // colon at all — which rules out editing YAML, JSON or a URL.
    let (mut app, events) = hosted("key\nother");
    join(&mut app, &events, 1, "guest");
    grant(&mut app, "guest");

    keys_from(&mut app, 1, "A: value<Esc>");
    assert_eq!(content(&app), "key: value\nother\n");

    // And inside a search prompt.
    keys_from(&mut app, 1, "/: v<CR>");
    assert_eq!(cursor_of(&app, 1), Position::new(0, 3));

    // But it still cannot open the command line.
    keys_from(&mut app, 1, ":q!<CR>");
    assert!(!app.should_quit);
}

#[test]
fn granting_write_access_lets_a_guest_edit() {
    let (mut app, events) = hosted("alpha");
    join(&mut app, &events, 1, "guest");
    keys_from(&mut app, 1, "x");
    assert_eq!(content(&app), "alpha\n");

    grant(&mut app, "guest");
    keys_from(&mut app, 1, "x");
    assert_eq!(content(&app), "lpha\n");

    commands::set_access(&mut app, "guest", Access::Read);
    keys_from(&mut app, 1, "x");
    assert_eq!(content(&app), "lpha\n");
}

#[test]
fn the_host_cannot_be_demoted() {
    let (mut app, _events) = hosted("alpha");
    commands::set_access(&mut app, "host", Access::Read);
    let session = app.session.as_ref().unwrap();
    assert_eq!(session.participants[0].access, Access::Write);
}

#[test]
fn a_guest_cannot_quit_the_host() {
    let (mut app, events) = hosted("alpha");
    join(&mut app, &events, 1, "guest");
    grant(&mut app, "guest");
    keys_from(&mut app, 1, ":q!<CR>");
    assert!(
        !app.should_quit,
        "a guest must not be able to end the session"
    );
}

#[test]
fn session_commands_work_when_typed_by_the_host() {
    // The host's keystrokes run inside `dispatch`, which checks the session
    // out of the editor. Commands that inspect it used to see nothing.
    let (mut app, events) = hosted("alpha");
    join(&mut app, &events, 1, "guest");

    keys_from_host(&mut app, ":who<CR>");
    let overlay = app.overlay.as_ref().expect(":who should open an overlay");
    assert_eq!(overlay.title, "Participants");
    assert!(overlay.lines.iter().any(|line| line.contains("guest")));
    app.overlay = None;

    keys_from_host(&mut app, ":grant guest<CR>");
    {
        let session = app.session.as_ref().expect("still sharing");
        let index = session.index_of(1).unwrap();
        assert_eq!(session.participants[index].access, Access::Write);
    }

    keys_from_host(&mut app, ":say hello<CR>");
    assert!(app.session.is_some(), ":say must not drop the session");

    keys_from_host(&mut app, ":unshare<CR>");
    assert!(app.session.is_none(), ":unshare should end the session");
    assert_eq!(app.shared_guests, None);
}

// -- frames -----------------------------------------------------------------

#[test]
fn a_new_guest_gets_a_welcome_and_a_full_frame() {
    let (mut app, events) = hosted("alpha\nbeta");
    let inbox = join(&mut app, &events, 1, "guest");
    render_remote_frames(&mut app);

    let messages: Vec<ServerMessage> = inbox.try_iter().collect();
    assert!(matches!(
        messages.first(),
        Some(ServerMessage::Welcome { .. })
    ));

    let frame = messages
        .iter()
        .find_map(|message| match message {
            ServerMessage::Frame { full, cells, .. } => Some((*full, cells.len())),
            _ => None,
        })
        .expect("a frame");
    assert!(frame.0, "the first frame must be a full repaint");
    assert_eq!(frame.1, 80 * 24, "a full frame covers the whole screen");
}

#[test]
fn later_frames_only_carry_what_changed() {
    let (mut app, events) = hosted("alpha\nbeta\ngamma");
    let inbox = join(&mut app, &events, 1, "guest");
    render_remote_frames(&mut app);
    let _ = inbox.try_iter().count();

    keys_from_host(&mut app, "x");
    render_remote_frames(&mut app);

    let cells = inbox
        .try_iter()
        .find_map(|message| match message {
            ServerMessage::Frame { full, cells, .. } if !full => Some(cells),
            _ => None,
        })
        .expect("an incremental frame");
    assert!(
        !cells.is_empty() && cells.len() < 200,
        "expected a small diff, got {} cells",
        cells.len()
    );
}

#[test]
fn a_guest_sees_the_host_caret_but_not_its_own() {
    let (mut app, events) = hosted("alpha\nbeta");
    join(&mut app, &events, 1, "guest");
    keys_from(&mut app, 1, "j");
    super::refresh_remote_cursors(&mut app);

    // Two carets exist, and the renderer is told which one is "self".
    assert_eq!(app.remote_cursors.len(), 2);
    assert_eq!(app.rendering_as, HOST_ID);
    let guest = app
        .remote_cursors
        .iter()
        .find(|remote| remote.id == 1)
        .unwrap();
    assert_eq!(guest.cursor.line, 1);
    assert_eq!(guest.name, "guest");
}

// -- membership -------------------------------------------------------------

#[test]
fn leaving_removes_the_participant_and_releases_followers() {
    let (mut app, events) = hosted("alpha");
    join(&mut app, &events, 1, "one");
    join(&mut app, &events, 2, "two");

    commands::follow(&mut app, "one");
    {
        let session = app.session.as_ref().unwrap();
        assert_eq!(session.participants[0].following, Some(1));
        assert_eq!(session.guests(), 2);
    }

    events.send(SessionEvent::Left { id: 1 }).unwrap();
    commands::poll_events(&mut app);

    let session = app.session.as_ref().unwrap();
    assert_eq!(session.guests(), 1);
    assert!(session.index_of(1).is_none());
    assert_eq!(
        session.participants[0].following, None,
        "following a departed participant must be released"
    );
}

#[test]
fn following_mirrors_the_other_participants_viewport() {
    let (mut app, events) = hosted(
        &(1..=200)
            .map(|n| n.to_string())
            .collect::<Vec<_>>()
            .join("\n"),
    );
    let inbox = join(&mut app, &events, 1, "guest");
    let _ = inbox;

    // The guest follows the host, then the host jumps to the end.
    {
        let session = app.session.as_mut().unwrap();
        let index = session.index_of(1).unwrap();
        session.participants[index].following = Some(HOST_ID);
    }
    keys_from_host(&mut app, "G");
    render_remote_frames(&mut app);

    let session = app.session.as_ref().unwrap();
    let index = session.index_of(1).unwrap();
    assert_eq!(
        session.participants[index].state.cursor(),
        session.participants[0].state.cursor()
    );
}

#[test]
fn unsharing_stops_the_session() {
    let (mut app, events) = hosted("alpha");
    let inbox = join(&mut app, &events, 1, "guest");
    commands::unshare(&mut app);
    assert!(app.session.is_none());
    assert!(app.remote_cursors.is_empty());
    assert!(inbox
        .try_iter()
        .any(|message| matches!(message, ServerMessage::Closed { .. })));
}

// -- protocol ---------------------------------------------------------------

#[test]
fn protocol_messages_round_trip() {
    let hello = ClientMessage::Hello {
        version: super::protocol::PROTOCOL_VERSION,
        token: "abc".to_string(),
        name: "guest".to_string(),
        cols: 80,
        rows: 24,
    };
    let text = serde_json::to_string(&hello).unwrap();
    assert!(text.contains("\"type\":\"hello\""));
    assert!(matches!(
        serde_json::from_str::<ClientMessage>(&text).unwrap(),
        ClientMessage::Hello { cols: 80, .. }
    ));

    // The browser sends this shape by hand, so it must stay accepted.
    let from_browser = r#"{"type":"keys","keys":"ciw"}"#;
    assert!(matches!(
        serde_json::from_str::<ClientMessage>(from_browser).unwrap(),
        ClientMessage::Keys { .. }
    ));
}

#[test]
fn colours_survive_the_wire() {
    use super::protocol::WireColor;
    use ratatui::style::Color;
    for original in [
        Color::Reset,
        Color::Red,
        Color::Indexed(213),
        Color::Rgb(1, 2, 3),
    ] {
        let wire = WireColor::from(original);
        let text = serde_json::to_string(&wire).unwrap();
        let parsed: WireColor = serde_json::from_str(&text).unwrap();
        assert_eq!(wire, parsed);
        let back: Color = parsed.into();
        match original {
            // Named colours normalise to their palette index.
            Color::Red => assert_eq!(back, Color::Indexed(1)),
            other => assert_eq!(back, other),
        }
    }
}

#[test]
fn tokens_are_long_and_unpredictable() {
    let first = super::server::generate_token();
    let second = super::server::generate_token();
    assert_eq!(first.len(), 24);
    assert_ne!(first, second);
    assert!(first.chars().all(|c| c.is_ascii_hexdigit()));
}

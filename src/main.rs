//! miv — a modal terminal text editor.

mod ai;
mod app;
mod command;
mod config;
mod core;
mod edit;
mod help;
mod keymap;
mod keys;
mod mode;
mod plugin;
mod session;
mod syntax;
#[cfg(test)]
mod tests;
mod tools;
mod ui;
mod view;

use anyhow::{Context, Result};
use app::App;
use clap::Parser;
use config::Config;
use crossterm::event::{self, Event, KeyEventKind};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::{cursor, event::DisableBracketedPaste, event::EnableBracketedPaste, execute};
use mode::Mode;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io::{stdout, Stdout};
use std::path::PathBuf;
use std::time::Duration;

/// How long the main loop waits for a keystroke before looking at the session
/// and redrawing. Short enough that a remote edit appears immediately, long
/// enough that an idle editor costs nothing.
const POLL_INTERVAL: Duration = Duration::from_millis(16);

#[derive(Parser)]
#[command(
    name = "miv",
    version,
    about = "A modal terminal text editor",
    after_help = "Press :help inside the editor for the key reference."
)]
struct Cli {
    /// Files to open
    #[arg(value_name = "FILE")]
    files: Vec<PathBuf>,

    /// Put the cursor on this line of the first file (also accepts +N)
    #[arg(short = 'l', long, value_name = "N")]
    line: Option<usize>,

    /// Use this configuration file instead of the default
    #[arg(long, value_name = "PATH")]
    config: Option<PathBuf>,

    /// Start with built-in defaults, ignoring any configuration file
    #[arg(long)]
    no_config: bool,

    /// List the available syntax highlighting themes and exit
    #[arg(long)]
    list_themes: bool,

    /// Start sharing the session as soon as the editor opens
    #[arg(long)]
    share: bool,

    /// Join someone else's session instead of opening files
    #[arg(long, value_name = "HOST:PORT")]
    attach: Option<String>,

    /// Session token, required with --attach
    #[arg(long, value_name = "TOKEN")]
    token: Option<String>,

    /// The name other participants see
    #[arg(long, value_name = "NAME")]
    name: Option<String>,
}

fn main() -> Result<()> {
    // Vim's `+42 file` form, which clap would reject as an unknown flag.
    let mut args: Vec<String> = std::env::args().collect();
    let mut leading_line = None;
    args.retain(|arg| {
        if let Some(number) = arg.strip_prefix('+') {
            if let Ok(line) = number.parse::<usize>() {
                leading_line = Some(line);
                return false;
            }
        }
        true
    });
    let cli = Cli::parse_from(args);

    if cli.list_themes {
        for name in syntax::SyntaxEngine::new().theme_names() {
            println!("{name}");
        }
        return Ok(());
    }

    if let Some(address) = cli.attach.clone() {
        install_panic_hook();
        let configuration = load_config(&cli).0;
        let name = cli
            .name
            .clone()
            .unwrap_or(configuration.session.name.clone());
        let token = cli.token.clone().unwrap_or_default();
        return session::client::attach(&address, &token, &name);
    }

    let (mut configuration, config_error) = load_config(&cli);
    if let Some(name) = cli.name.clone() {
        configuration.session.name = name;
    }
    let mut app = App::new(configuration, &cli.files, cli.line.or(leading_line))?;
    if let Some(error) = config_error {
        app.set_error(error);
    }

    if cli.share {
        session::commands::share(&mut app, "");
    }

    let mut terminal = enter_terminal().context("preparing the terminal")?;
    let result = run(&mut terminal, &mut app);
    if let Some(session) = app.session.take() {
        session.stop();
    }
    restore_terminal();
    result
}

fn load_config(cli: &Cli) -> (Config, Option<String>) {
    if cli.no_config {
        return (Config::default(), None);
    }
    match &cli.config {
        // An explicitly requested file that cannot be read is a hard error:
        // silently ignoring it would be worse than refusing to start.
        Some(path) => match Config::load(path) {
            Ok(configuration) => (configuration, None),
            Err(e) => {
                eprintln!("miv: {e:#}");
                std::process::exit(1);
            }
        },
        None => match config::default_config_path() {
            Some(path) if path.exists() => match Config::load(&path) {
                Ok(configuration) => (configuration, None),
                Err(e) => (Config::default(), Some(format!("{e:#}"))),
            },
            _ => (Config::default(), None),
        },
    }
}

type Backend = CrosstermBackend<Stdout>;

/// Restore the terminal on panic; a raw-mode panic otherwise leaves the user
/// with an unusable shell.
fn install_panic_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal();
        default_hook(info);
    }));
}

fn enter_terminal() -> Result<Terminal<Backend>> {
    install_panic_hook();
    enable_raw_mode()?;
    execute!(stdout(), EnterAlternateScreen, EnableBracketedPaste)?;
    let terminal = Terminal::new(CrosstermBackend::new(stdout()))?;
    Ok(terminal)
}

pub(crate) fn restore_terminal() {
    let _ = execute!(
        stdout(),
        DisableBracketedPaste,
        LeaveAlternateScreen,
        cursor::SetCursorStyle::DefaultUserShape,
        cursor::Show
    );
    let _ = disable_raw_mode();
}

fn run(terminal: &mut Terminal<Backend>, app: &mut App) -> Result<()> {
    let mut cursor_shape = Mode::Normal;
    let _ = execute!(stdout(), ui::cursor_style(cursor_shape));
    let mut dirty = true;

    loop {
        if dirty {
            terminal.draw(|frame| ui::draw(frame, app))?;
            // Guests are drawn after the host so they see the same state, each
            // through their own viewport.
            session::render_remote_frames(app);
            dirty = false;
        }

        if app.mode != cursor_shape {
            cursor_shape = app.mode;
            let _ = execute!(stdout(), ui::cursor_style(cursor_shape));
        }
        if app.should_quit {
            app.announce(plugin::Event::Quitting);
            return Ok(());
        }

        // Waiting with a timeout rather than blocking is what lets a session
        // guest's keystrokes show up without the host touching the keyboard.
        if event::poll(POLL_INTERVAL)? {
            match event::read()? {
                Event::Key(key) if key.kind != KeyEventKind::Release => {
                    session::dispatch(app, session::HOST_ID, |app| {
                        app.begin_input();
                        keymap::handle(app, key);
                        // Drain keys queued by `.` or a macro without redrawing
                        // between them, so a long macro is not slowed by rendering.
                        while let Some(queued) = app.queue.pop_front() {
                            keymap::handle(app, queued);
                            if app.should_quit {
                                break;
                            }
                        }
                    });
                    app.note_change();
                    dirty = true;
                }
                Event::Paste(text) => {
                    session::dispatch(app, session::HOST_ID, |app| {
                        app.begin_input();
                        keymap::handle_paste(app, &text);
                    });
                    app.note_change();
                    dirty = true;
                }
                // ratatui recomputes the layout from the backend size each draw,
                // so a resize needs nothing beyond the redraw at the loop head.
                Event::Resize(_, _) => dirty = true,
                _ => {}
            }
        }

        if session::commands::poll_events(app) {
            dirty = true;
        }
        // Background tools: start what is due, then apply whatever finished.
        app.tick_background();
        if app.poll_background() {
            dirty = true;
        }
    }
}

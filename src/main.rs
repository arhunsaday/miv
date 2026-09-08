//! miv — a modal terminal text editor.

mod app;
mod buffer;
mod command;
mod config;
mod help;
mod history;
mod keymap;
mod keys;
mod mode;
mod motion;
mod operator;
mod register;
mod search;
mod syntax;
#[cfg(test)]
mod tests;
mod text;
mod textobject;
mod ui;

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

    let (configuration, config_error) = load_config(&cli);
    let mut app = App::new(configuration, &cli.files, cli.line.or(leading_line))?;
    if let Some(error) = config_error {
        app.set_error(error);
    }

    let mut terminal = enter_terminal().context("preparing the terminal")?;
    let result = run(&mut terminal, &mut app);
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

fn enter_terminal() -> Result<Terminal<Backend>> {
    // Restore the terminal on panic; a raw-mode panic otherwise leaves the
    // user with an unusable shell.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal();
        default_hook(info);
    }));

    enable_raw_mode()?;
    execute!(stdout(), EnterAlternateScreen, EnableBracketedPaste)?;
    let terminal = Terminal::new(CrosstermBackend::new(stdout()))?;
    Ok(terminal)
}

fn restore_terminal() {
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

    loop {
        terminal.draw(|frame| ui::draw(frame, app))?;

        if app.mode != cursor_shape {
            cursor_shape = app.mode;
            let _ = execute!(stdout(), ui::cursor_style(cursor_shape));
        }
        if app.should_quit {
            return Ok(());
        }

        match event::read()? {
            Event::Key(key) if key.kind != KeyEventKind::Release => {
                app.begin_input();
                keymap::handle(app, key);
            }
            Event::Paste(text) => {
                app.begin_input();
                keymap::handle_paste(app, &text);
            }
            // ratatui recomputes the layout from the backend size each draw,
            // so a resize needs nothing beyond the redraw at the loop head.
            Event::Resize(_, _) => {}
            _ => continue,
        }

        // Drain keys queued by `.` or a macro without redrawing between them,
        // so replaying a long macro is not slowed down by rendering.
        while let Some(key) = app.queue.pop_front() {
            keymap::handle(app, key);
            if app.should_quit {
                break;
            }
        }
    }
}

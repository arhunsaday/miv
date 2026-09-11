//! What a plugin can ask the editor to do.
//!
//! Today the only caller is the contributed-command runner below, which is
//! enough to make plugins useful without a host process. When one arrives it
//! calls these same operations, so the surface a plugin sees does not change.
//!
//! The operations a host will expose, and which of them exist:
//!
//! | operation | capability | state |
//! | --- | --- | --- |
//! | read the buffer, or a range of it | `read_buffer` | done |
//! | replace a range | `write_buffer` | done |
//! | run an ex command | `run_commands` | done |
//! | show a message | `show_ui` | done |
//! | read the cursor and selection | `read_buffer` | done |
//! | contribute signs and virtual text | `show_ui` | not yet |
//! | open a picker and receive the choice | `show_ui` | not yet |
//! | subscribe to events | — | recorded, not yet forwarded |

use super::{Capability, Kind, Target};
use crate::app::App;
use crate::core::text;
use std::time::Duration;

/// How long a contributed command may take before it is abandoned.
const TIMEOUT: Duration = Duration::from_secs(10);

/// The character range a contributed command acts on.
fn range_for(app: &App, target: Target) -> (usize, usize) {
    let buffer = app.buffer();
    match target {
        Target::Buffer => (0, buffer.rope.len_chars()),
        Target::Line => {
            let line = buffer.cursor.line;
            let start = buffer.rope.line_to_char(line);
            let end = start + text::line_len(&buffer.rope, line);
            (start, end)
        }
        Target::Selection => {
            if app.mode.is_visual() {
                let range = app.visual_range();
                (range.start, range.end)
            } else {
                let line = buffer.cursor.line;
                let start = buffer.rope.line_to_char(line);
                let end = start + text::line_len(&buffer.rope, line);
                (start, end)
            }
        }
    }
}

/// Run a command a plugin contributed. Returns false when no such command
/// exists, so the caller can fall through to "not an editor command".
pub fn run(app: &mut App, name: &str, arguments: &str) -> bool {
    let Some(command) = app.plugins.command(name).cloned() else {
        return false;
    };

    let plugin = app
        .plugins
        .loaded
        .iter()
        .find(|plugin| plugin.manifest.name == command.plugin)
        .cloned();
    let Some(plugin) = plugin else {
        app.set_error(format!("{name}: its plugin is no longer loaded"));
        return true;
    };

    match command.kind {
        Kind::Ex => {
            if !plugin.grants(Capability::RunCommands) {
                app.set_error(format!(
                    "{}: not allowed to run commands",
                    plugin.manifest.name
                ));
                return true;
            }
            let ex = command.command.join(" ").replace("$ARGS", arguments);
            crate::command::execute(app, &ex);
        }
        Kind::Report => {
            if !plugin.grants(Capability::ShowUi) {
                app.set_error(format!(
                    "{}: not allowed to show anything",
                    plugin.manifest.name
                ));
                return true;
            }
            let (start, end) = range_for(app, command.target);
            let text = app.buffer().slice(start, end);
            let name_hint = app.buffer().short_name();
            match crate::tools::external::run(
                &substituted(&command.command, arguments),
                &name_hint,
                &text,
                Some(&plugin.directory),
                TIMEOUT,
            ) {
                Ok(output) => {
                    let message = output
                        .combined()
                        .lines()
                        .find(|line| !line.trim().is_empty())
                        .unwrap_or("(no output)")
                        .to_string();
                    app.set_message(format!("{name}: {message}"));
                }
                Err(e) => app.set_error(format!("{name}: {e}")),
            }
        }
        Kind::Filter => {
            if !plugin.grants(Capability::WriteBuffer) {
                app.set_error(format!(
                    "{}: not allowed to change the buffer",
                    plugin.manifest.name
                ));
                return true;
            }
            let (start, end) = range_for(app, command.target);
            let original = app.buffer().slice(start, end);
            let name_hint = app.buffer().short_name();
            match crate::tools::external::run(
                &substituted(&command.command, arguments),
                &name_hint,
                &original,
                Some(&plugin.directory),
                TIMEOUT,
            ) {
                Ok(output) if output.status == Some(0) => {
                    let replacement = output.stdout;
                    if replacement == original {
                        app.set_message(format!("{name}: nothing changed"));
                    } else {
                        replace_range(app, start, end, &replacement);
                        app.set_message(format!("{name}: applied"));
                    }
                }
                Ok(output) => {
                    let reason = output
                        .stderr
                        .lines()
                        .find(|line| !line.trim().is_empty())
                        .unwrap_or("exited non-zero")
                        .to_string();
                    app.set_error(format!("{name}: {reason}"));
                }
                Err(e) => app.set_error(format!("{name}: {e}")),
            }
        }
    }
    true
}

fn substituted(command: &[String], arguments: &str) -> Vec<String> {
    command
        .iter()
        .map(|part| part.replace("$ARGS", arguments))
        .collect()
}

/// Replace a range as one undo step, leaving the cursor somewhere sensible.
fn replace_range(app: &mut App, start: usize, end: usize, replacement: &str) {
    if app.mode.is_visual() {
        app.leave_visual();
    }
    let buffer = app.buffer_mut();
    buffer.begin();
    buffer.replace(start, end, replacement);
    buffer.end();
    let position = text::char_to_pos(&buffer.rope, start.min(buffer.rope.len_chars()));
    buffer.cursor = text::clamp(&buffer.rope, position, false);
    buffer.desired_col = buffer.cursor.col;
}

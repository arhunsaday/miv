//! Formatting through external tools.
//!
//! Formatters run synchronously: the caller is waiting, and writing an
//! unformatted file and fixing it afterwards would be worse than a short
//! pause. Their output is applied as a minimal set of line splices rather than
//! a wholesale replacement, so undo stays meaningful, the cursor keeps its
//! place, and other participants in a shared session have their carets moved
//! correctly.

use crate::app::App;
use crate::config::FormatterConfig;
use crate::text;
use similar::{DiffOp, TextDiff};
use std::path::Path;
use std::time::Duration;

#[derive(Clone)]
pub struct Formatter {
    pub name: String,
    pub command: Vec<String>,
    filetypes: Vec<String>,
    extensions: Vec<String>,
}

impl Formatter {
    pub fn compile(config: &FormatterConfig) -> Result<Self, String> {
        if config.command.is_empty() {
            return Err("a formatter needs a command".to_string());
        }
        Ok(Self {
            name: config.display_name(),
            command: config.command.clone(),
            filetypes: config
                .filetypes
                .iter()
                .map(|f| f.to_ascii_lowercase())
                .collect(),
            extensions: config
                .extensions
                .iter()
                .map(|e| e.trim_start_matches('.').to_ascii_lowercase())
                .collect(),
        })
    }

    pub fn applies_to(&self, syntax_name: &str, path: Option<&Path>) -> bool {
        crate::external::applies(&self.filetypes, &self.extensions, syntax_name, path)
    }
}

pub struct Formatted {
    pub tool: String,
    pub changed_lines: usize,
}

/// Format the current buffer in place. `Ok(None)` means it was already
/// formatted; `Err` carries something worth putting on the message line.
pub fn format_buffer(app: &mut App) -> Result<Option<Formatted>, String> {
    let (syntax_name, path, name) = {
        let buffer = app.buffer();
        (
            buffer.syntax_name.clone(),
            buffer.path.clone(),
            buffer.short_name(),
        )
    };
    let Some(formatter) = app
        .formatters
        .iter()
        .find(|f| f.applies_to(&syntax_name, path.as_deref()))
        .cloned()
    else {
        return Err(format!("no formatter configured for {syntax_name}"));
    };

    let original: String = app.buffer().rope.chars().collect();
    let directory = path.as_deref().and_then(|p| p.parent());
    let timeout = Duration::from_millis(app.config.format.timeout_ms);

    let output = crate::external::run(&formatter.command, &name, &original, directory, timeout)
        .map_err(|e| format!("{}: {e}", formatter.name))?;

    if output.status != Some(0) {
        let reason = output
            .stderr
            .lines()
            .find(|line| !line.trim().is_empty())
            .unwrap_or("exited non-zero");
        return Err(format!("{}: {reason}", formatter.name));
    }

    let mut formatted = output.stdout;
    if formatted.trim().is_empty() && !original.trim().is_empty() {
        return Err(format!("{} produced no output", formatter.name));
    }
    if !formatted.ends_with('\n') {
        formatted.push('\n');
    }
    if formatted == original {
        return Ok(None);
    }

    let changed = apply(app, &original, &formatted);
    Ok(Some(Formatted {
        tool: formatter.name,
        changed_lines: changed,
    }))
}

/// Replace `original` with `formatted` using the smallest set of line splices,
/// and carry the cursor along with them.
pub fn apply(app: &mut App, original: &str, formatted: &str) -> usize {
    let new_lines: Vec<&str> = formatted.split_inclusive('\n').collect();
    let diff = TextDiff::from_lines(original, formatted);

    let cursor_before = app.buffer().cursor_char();
    let mark = app.buffer().edit_mark();
    let mut changed = 0usize;

    let buffer = app.buffer_mut();
    buffer.begin();
    // Apply back to front so the offsets of earlier splices stay valid.
    for op in diff.ops().iter().rev() {
        let line_start = |buffer: &crate::buffer::Buffer, line: usize| -> usize {
            let limit = buffer.rope.len_lines().saturating_sub(1);
            buffer.rope.line_to_char(line.min(limit))
        };
        match *op {
            DiffOp::Equal { .. } => {}
            DiffOp::Delete {
                old_index, old_len, ..
            } => {
                let from = line_start(buffer, old_index);
                let to = line_start(buffer, old_index + old_len);
                buffer.remove(from, to);
                changed += old_len;
            }
            DiffOp::Insert {
                old_index,
                new_index,
                new_len,
            } => {
                let at = line_start(buffer, old_index);
                let text: String = new_lines[new_index..new_index + new_len].concat();
                buffer.insert(at, &text);
                changed += new_len;
            }
            DiffOp::Replace {
                old_index,
                old_len,
                new_index,
                new_len,
            } => {
                let from = line_start(buffer, old_index);
                let to = line_start(buffer, old_index + old_len);
                let text: String = new_lines[new_index..new_index + new_len].concat();
                buffer.remove(from, to);
                buffer.insert(from, &text);
                changed += new_len.max(old_len);
            }
        }
    }
    buffer.end();

    // Move the cursor through the same splices, so formatting does not fling
    // you to the top of the file.
    let mut cursor = cursor_before;
    let edits: Vec<crate::history::Change> = app.buffer().edits_since(mark).to_vec();
    for change in &edits {
        cursor = crate::session::shift_offset(cursor, change);
    }
    let buffer = app.buffer_mut();
    let position = text::char_to_pos(&buffer.rope, cursor.min(buffer.rope.len_chars()));
    buffer.cursor = text::clamp(&buffer.rope, position, false);
    buffer.desired_col = buffer.cursor.col;
    changed
}

/// Formatters that work out of the box when the tool is installed.
pub fn builtin_formatters() -> Vec<FormatterConfig> {
    let formatter =
        |name: &str, command: &[&str], extensions: &[&str], filetypes: &[&str]| FormatterConfig {
            name: Some(name.to_string()),
            command: command.iter().map(|s| s.to_string()).collect(),
            filetypes: filetypes.iter().map(|s| s.to_string()).collect(),
            extensions: extensions.iter().map(|s| s.to_string()).collect(),
        };

    vec![
        formatter(
            "rustfmt",
            &["rustfmt", "--emit", "stdout"],
            &["rs"],
            &["rust"],
        ),
        formatter("gofmt", &["gofmt"], &["go"], &["go"]),
        formatter(
            "terraform",
            &["terraform", "fmt", "-"],
            &["tf", "tfvars"],
            &["terraform"],
        ),
        formatter(
            "shfmt",
            &["shfmt", "-i", "2"],
            &["sh", "bash"],
            &["shell", "bash"],
        ),
        formatter("black", &["black", "-q", "-"], &["py"], &["python"]),
        formatter(
            "prettier",
            &["prettier", "--stdin-filepath", "$NAME"],
            &[
                "json", "yml", "yaml", "md", "js", "ts", "tsx", "jsx", "css", "scss", "html",
            ],
            &[],
        ),
    ]
}

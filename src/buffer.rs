//! A single open file: its text, cursor, viewport, undo history and marks.

use crate::history::{Change, History};
use crate::syntax::SyntaxCache;
use crate::text::{self, Position};
use anyhow::{Context, Result};
use ropey::Rope;
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineEnding {
    Lf,
    Crlf,
}

impl LineEnding {
    fn as_str(self) -> &'static str {
        match self {
            LineEnding::Lf => "\n",
            LineEnding::Crlf => "\r\n",
        }
    }
}

pub struct Buffer {
    pub id: usize,
    /// Always ends with a newline; see [`crate::text`].
    pub rope: Rope,
    pub path: Option<PathBuf>,
    pub cursor: Position,
    /// Column `j`/`k` aim for, so moving through short lines and back out
    /// returns you to where you started.
    pub desired_col: usize,
    pub view_top: usize,
    pub view_left: usize,
    pub history: History,
    pub marks: HashMap<char, Position>,
    /// Jump list for `Ctrl-O` / `Ctrl-I`.
    pub jumps: Vec<Position>,
    pub jump_index: usize,
    pub line_ending: LineEnding,
    /// Whether the file on disk ended with a newline, so saving round-trips.
    pub final_newline: bool,
    pub syntax_name: String,
    pub syntax_cache: SyntaxCache,
    /// Named a file that does not exist yet.
    pub is_new_file: bool,
    /// Splices applied since the last drain. A shared session uses these to
    /// move every other participant's cursor along with the text, so their
    /// caret stays on the character it was pointing at.
    pub edits: Vec<Change>,
}

impl Buffer {
    pub fn scratch(id: usize) -> Self {
        Self {
            id,
            rope: Rope::from_str("\n"),
            path: None,
            cursor: Position::default(),
            desired_col: 0,
            view_top: 0,
            view_left: 0,
            history: History::default(),
            marks: HashMap::new(),
            jumps: Vec::new(),
            jump_index: 0,
            line_ending: LineEnding::Lf,
            final_newline: true,
            syntax_name: "Plain Text".to_string(),
            syntax_cache: SyntaxCache::default(),
            is_new_file: false,
            edits: Vec::new(),
        }
    }

    /// Open `path`. A path that does not exist yields an empty buffer bound to
    /// it, as Vim does, rather than an error.
    pub fn open(id: usize, path: &Path) -> Result<Self> {
        let mut buffer = Self::scratch(id);
        buffer.path = Some(path.to_path_buf());

        match fs::read(path) {
            Ok(bytes) => {
                let content = String::from_utf8(bytes)
                    .with_context(|| format!("{} is not valid UTF-8", path.display()))?;
                buffer.line_ending = if content.contains("\r\n") {
                    LineEnding::Crlf
                } else {
                    LineEnding::Lf
                };
                buffer.final_newline = content.is_empty() || content.ends_with('\n');
                let mut normalized = content.replace("\r\n", "\n");
                if !normalized.ends_with('\n') {
                    normalized.push('\n');
                }
                buffer.rope = Rope::from_str(&normalized);
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                buffer.is_new_file = true;
            }
            Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
        }
        Ok(buffer)
    }

    pub fn display_name(&self) -> String {
        match &self.path {
            Some(p) => p.display().to_string(),
            None => "[No Name]".to_string(),
        }
    }

    pub fn short_name(&self) -> String {
        match &self.path {
            Some(p) => p
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| p.display().to_string()),
            None => "[No Name]".to_string(),
        }
    }

    pub fn line_count(&self) -> usize {
        text::line_count(&self.rope)
    }

    pub fn is_modified(&self) -> bool {
        self.history.is_modified()
    }

    pub fn cursor_char(&self) -> usize {
        text::pos_to_char(&self.rope, self.cursor)
    }

    pub fn line_len(&self, line: usize) -> usize {
        text::line_len(&self.rope, line)
    }

    /// True for a buffer that has never been touched and holds nothing, which
    /// `:e` may replace in place.
    pub fn is_empty_scratch(&self) -> bool {
        self.path.is_none() && self.rope.len_chars() <= 1 && !self.is_modified()
    }

    /// Take the splices recorded since the last call.
    pub fn drain_edits(&mut self) -> Vec<Change> {
        std::mem::take(&mut self.edits)
    }

    pub fn slice(&self, from: usize, to: usize) -> String {
        let end = to.min(self.rope.len_chars());
        let start = from.min(end);
        self.rope.slice(start..end).chars().collect()
    }

    // -- mutation -----------------------------------------------------------
    //
    // Every edit funnels through `insert`/`remove` so that history recording
    // and syntax invalidation can never be forgotten at a call site.

    pub fn insert(&mut self, at: usize, content: &str) {
        if content.is_empty() {
            return;
        }
        let at = at.min(self.rope.len_chars());
        self.history.start(self.cursor);
        self.history.record(Change {
            at,
            removed: String::new(),
            inserted: content.to_string(),
        });
        let line = self.rope.char_to_line(at);
        self.rope.insert(at, content);
        self.edits.push(Change {
            at,
            removed: String::new(),
            inserted: content.to_string(),
        });
        self.syntax_cache.invalidate_from(line);
        self.history.commit(self.cursor);
    }

    pub fn remove(&mut self, from: usize, to: usize) -> String {
        let end = to.min(self.rope.len_chars());
        let start = from.min(end);
        if start == end {
            return String::new();
        }
        let removed: String = self.rope.slice(start..end).chars().collect();
        self.history.start(self.cursor);
        self.history.record(Change {
            at: start,
            removed: removed.clone(),
            inserted: String::new(),
        });
        let line = self.rope.char_to_line(start);
        self.rope.remove(start..end);
        self.edits.push(Change {
            at: start,
            removed: removed.clone(),
            inserted: String::new(),
        });
        self.ensure_trailing_newline();
        self.syntax_cache.invalidate_from(line);
        self.history.commit(self.cursor);
        removed
    }

    pub fn replace(&mut self, from: usize, to: usize, content: &str) {
        self.history.start(self.cursor);
        self.remove(from, to);
        self.insert(from, content);
        self.history.commit(self.cursor);
    }

    /// Group everything done inside the closure into one undo step.
    pub fn begin(&mut self) {
        let cursor = self.cursor;
        self.history.start(cursor);
    }

    pub fn end(&mut self) {
        let cursor = self.cursor;
        self.history.commit(cursor);
    }

    fn ensure_trailing_newline(&mut self) {
        let len = self.rope.len_chars();
        if len == 0 || self.rope.char(len - 1) != '\n' {
            self.rope.insert(len, "\n");
        }
    }

    fn apply_raw(&mut self, change: &Change) {
        let removed_len = change.removed.chars().count();
        let line = self.rope.char_to_line(change.at.min(self.rope.len_chars()));
        if removed_len > 0 {
            self.rope.remove(change.at..change.at + removed_len);
        }
        if !change.inserted.is_empty() {
            self.rope.insert(change.at, &change.inserted);
        }
        self.edits.push(change.clone());
        self.ensure_trailing_newline();
        self.syntax_cache.invalidate_from(line);
    }

    pub fn undo(&mut self) -> bool {
        let Some(txn) = self.history.pop_undo() else {
            return false;
        };
        for change in txn.changes.iter().rev() {
            self.apply_raw(&change.inverted());
        }
        self.cursor = text::clamp(&self.rope, txn.before, false);
        self.desired_col = self.cursor.col;
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(txn) = self.history.pop_redo() else {
            return false;
        };
        for change in &txn.changes {
            self.apply_raw(change);
        }
        self.cursor = text::clamp(&self.rope, txn.after, false);
        self.desired_col = self.cursor.col;
        true
    }

    // -- persistence --------------------------------------------------------

    /// Serialize with the line endings and final-newline convention the file
    /// arrived with.
    fn to_text(&self) -> String {
        let mut out: String = self.rope.chars().collect();
        if !self.final_newline {
            out.pop();
        }
        if self.line_ending == LineEnding::Crlf {
            out = out.replace('\n', LineEnding::Crlf.as_str());
        }
        out
    }

    /// Write to disk atomically: a temporary file in the same directory,
    /// flushed and fsynced, then renamed over the target. A crash mid-write
    /// cannot leave a truncated file behind.
    pub fn write(&mut self, path: &Path) -> Result<(usize, usize)> {
        let text = self.to_text();
        let dir = path.parent().filter(|p| !p.as_os_str().is_empty());
        let dir = dir.unwrap_or_else(|| Path::new("."));
        let file_name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "buffer".to_string());
        let temp = dir.join(format!(".{file_name}.miv~"));

        let write_result = (|| -> Result<()> {
            let mut file =
                fs::File::create(&temp).with_context(|| format!("creating {}", temp.display()))?;
            file.write_all(text.as_bytes())?;
            file.sync_all()?;
            drop(file);
            // Preserve the original file's permissions across the rename.
            if let Ok(meta) = fs::metadata(path) {
                let _ = fs::set_permissions(&temp, meta.permissions());
            }
            fs::rename(&temp, path).with_context(|| format!("renaming into {}", path.display()))?;
            Ok(())
        })();

        if write_result.is_err() {
            let _ = fs::remove_file(&temp);
        }
        write_result?;

        self.history.mark_saved();
        self.is_new_file = false;
        Ok((text.len(), self.line_count()))
    }
}

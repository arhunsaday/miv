//! Undo/redo built from invertible transactions.
//!
//! Every buffer mutation is recorded as a [`Change`]; changes are grouped into
//! a [`Transaction`] by the command that caused them, which is what gives undo
//! its Vim-like granularity (one whole insert session undoes at once).

use crate::core::text::Position;

/// A single splice: at char offset `at`, `removed` was replaced by `inserted`.
/// Swapping the two fields inverts it.
#[derive(Clone, Debug)]
pub struct Change {
    pub at: usize,
    pub removed: String,
    pub inserted: String,
}

impl Change {
    pub fn inverted(&self) -> Change {
        Change {
            at: self.at,
            removed: self.inserted.clone(),
            inserted: self.removed.clone(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Transaction {
    pub changes: Vec<Change>,
    /// Cursor before the command ran; where undo returns you.
    pub before: Position,
    /// Cursor after the command ran; where redo returns you.
    pub after: Position,
}

#[derive(Default)]
pub struct History {
    undo: Vec<Transaction>,
    redo: Vec<Transaction>,
    open: Option<Transaction>,
    depth: usize,
    /// Undo depth at the last file write, used to answer "is this modified?"
    /// correctly after undoing back past the save point.
    saved_at: usize,
}

impl History {
    /// Open a transaction. Nested calls join the outermost one, so a command
    /// built from smaller primitives still undoes as a single step.
    pub fn start(&mut self, cursor: Position) {
        if self.depth == 0 {
            self.open = Some(Transaction {
                changes: Vec::new(),
                before: cursor,
                after: cursor,
            });
        }
        self.depth += 1;
    }

    /// How deeply transactions are nested. One means the outermost operation
    /// is in progress.
    pub fn depth(&self) -> usize {
        self.depth
    }

    pub fn record(&mut self, change: Change) {
        if let Some(txn) = self.open.as_mut() {
            txn.changes.push(change);
        }
    }

    pub fn commit(&mut self, cursor: Position) {
        self.depth = self.depth.saturating_sub(1);
        if self.depth > 0 {
            return;
        }
        if let Some(mut txn) = self.open.take() {
            if !txn.changes.is_empty() {
                txn.after = cursor;
                self.undo.push(txn);
                self.redo.clear();
            }
        }
    }

    pub fn pop_undo(&mut self) -> Option<Transaction> {
        let txn = self.undo.pop()?;
        self.redo.push(txn.clone());
        Some(txn)
    }

    pub fn pop_redo(&mut self) -> Option<Transaction> {
        let txn = self.redo.pop()?;
        self.undo.push(txn.clone());
        Some(txn)
    }

    /// How many committed transactions exist. Used to detect whether a
    /// command changed the buffer at all.
    pub fn revision(&self) -> usize {
        self.undo.len()
    }

    pub fn mark_saved(&mut self) {
        self.saved_at = self.undo.len();
    }

    pub fn is_modified(&self) -> bool {
        self.undo.len() != self.saved_at
    }
}

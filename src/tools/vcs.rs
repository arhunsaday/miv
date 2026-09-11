//! Git integration: which lines differ from the committed version.
//!
//! Only what the gutter needs. Everything is derived from one `git show` and a
//! line diff, so there is no index to keep in sync and nothing to invalidate
//! beyond the buffer's own revision.

use similar::{DiffOp, TextDiff};
use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineStatus {
    Added,
    Modified,
    /// Lines were deleted immediately above this one.
    RemovedAbove,
}

impl LineStatus {
    pub fn sign(self) -> &'static str {
        match self {
            LineStatus::Added => "┃",
            LineStatus::Modified => "┃",
            LineStatus::RemovedAbove => "▁",
        }
    }

    pub fn color(self) -> ratatui::style::Color {
        match self {
            LineStatus::Added => ratatui::style::Color::Green,
            LineStatus::Modified => ratatui::style::Color::Yellow,
            LineStatus::RemovedAbove => ratatui::style::Color::Red,
        }
    }
}

/// Changed lines, keyed by their zero-based index in the working buffer.
pub type LineStatuses = BTreeMap<usize, LineStatus>;

/// Compare the buffer against `HEAD`.
///
/// A file that git does not know about yet still gets a useful answer: every
/// line reads as added, which is what you want when you have just created it.
pub fn diff_against_head(
    path: &Path,
    text: &str,
    timeout: Duration,
) -> Result<LineStatuses, String> {
    let directory = path.parent().filter(|p| !p.as_os_str().is_empty());
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| "no file name".to_string())?;

    let inside_repository = crate::tools::external::run(
        &[
            "git".to_string(),
            "rev-parse".to_string(),
            "--is-inside-work-tree".to_string(),
        ],
        name,
        "",
        directory,
        timeout,
    )
    .map(|output| output.stdout.trim() == "true")
    .unwrap_or(false);
    if !inside_repository {
        return Err("not a git repository".to_string());
    }

    let show = crate::tools::external::run(
        &[
            "git".to_string(),
            "--no-pager".to_string(),
            "show".to_string(),
            format!("HEAD:./{name}"),
        ],
        name,
        "",
        directory,
        timeout,
    )
    .map_err(|e| e.to_string())?;

    // A missing path at HEAD means the file is new, not that something failed.
    let committed = if show.status == Some(0) {
        show.stdout
    } else {
        String::new()
    };

    Ok(compare(&committed, text))
}

/// Line-level diff, reduced to one status per changed line of the new text.
pub fn compare(old: &str, new: &str) -> LineStatuses {
    let mut statuses = LineStatuses::new();
    let diff = TextDiff::from_lines(old, new);
    for op in diff.ops() {
        match *op {
            DiffOp::Equal { .. } => {}
            DiffOp::Insert {
                new_index, new_len, ..
            } => {
                for line in new_index..new_index + new_len {
                    statuses.insert(line, LineStatus::Added);
                }
            }
            DiffOp::Replace {
                new_index, new_len, ..
            } => {
                for line in new_index..new_index + new_len {
                    statuses.insert(line, LineStatus::Modified);
                }
            }
            DiffOp::Delete { new_index, .. } => {
                // Nothing occupies the deleted lines, so the marker goes on
                // the line that closed over them.
                statuses
                    .entry(new_index)
                    .or_insert(LineStatus::RemovedAbove);
            }
        }
    }
    statuses
}

/// The first line of the next changed run, so `]h` jumps between hunks rather
/// than between lines.
pub fn next_hunk(statuses: &LineStatuses, from: usize, forward: bool) -> Option<usize> {
    let starts: Vec<usize> = statuses
        .keys()
        .copied()
        .filter(|line| !statuses.contains_key(&line.wrapping_sub(1)) || *line == 0)
        .collect();
    if starts.is_empty() {
        return None;
    }
    if forward {
        starts
            .iter()
            .find(|line| **line > from)
            .or_else(|| starts.first())
            .copied()
    } else {
        starts
            .iter()
            .rev()
            .find(|line| **line < from)
            .or_else(|| starts.last())
            .copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn added_lines_are_marked() {
        let statuses = compare("one\ntwo\n", "one\nnew\ntwo\n");
        assert_eq!(statuses.get(&1), Some(&LineStatus::Added));
        assert_eq!(statuses.get(&0), None);
        assert_eq!(statuses.get(&2), None);
    }

    #[test]
    fn changed_lines_are_marked_as_modified_not_added() {
        let statuses = compare("one\ntwo\nthree\n", "one\nTWO\nthree\n");
        assert_eq!(statuses.get(&1), Some(&LineStatus::Modified));
        assert_eq!(statuses.len(), 1);
    }

    #[test]
    fn a_deletion_marks_the_line_that_closed_over_it() {
        let statuses = compare("one\ntwo\nthree\n", "one\nthree\n");
        assert_eq!(statuses.get(&1), Some(&LineStatus::RemovedAbove));
    }

    #[test]
    fn a_file_new_to_git_reads_as_entirely_added() {
        let statuses = compare("", "one\ntwo\n");
        assert_eq!(statuses.get(&0), Some(&LineStatus::Added));
        assert_eq!(statuses.get(&1), Some(&LineStatus::Added));
    }

    #[test]
    fn an_unchanged_file_has_no_signs() {
        assert!(compare("same\n", "same\n").is_empty());
    }

    #[test]
    fn hunk_navigation_groups_contiguous_lines() {
        // Two hunks: lines 1-3 and line 8.
        let mut statuses = LineStatuses::new();
        for line in [1, 2, 3, 8] {
            statuses.insert(line, LineStatus::Modified);
        }
        assert_eq!(next_hunk(&statuses, 0, true), Some(1));
        // From inside the first hunk, forward goes to the next hunk's start.
        assert_eq!(next_hunk(&statuses, 1, true), Some(8));
        assert_eq!(next_hunk(&statuses, 8, true), Some(1), "should wrap");
        assert_eq!(next_hunk(&statuses, 9, false), Some(8));
        assert_eq!(next_hunk(&statuses, 0, false), Some(8), "should wrap");
        assert_eq!(next_hunk(&LineStatuses::new(), 0, true), None);
    }

    #[test]
    fn a_hunk_starting_at_the_first_line_is_found() {
        let mut statuses = LineStatuses::new();
        statuses.insert(0, LineStatus::Added);
        statuses.insert(1, LineStatus::Added);
        assert_eq!(next_hunk(&statuses, 5, true), Some(0));
    }
}

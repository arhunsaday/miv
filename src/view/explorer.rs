//! The file explorer: a lazily expanded directory tree.
//!
//! Only expanded directories are read, so opening the sidebar in a large
//! repository costs one `read_dir` rather than a walk.

use ignore::gitignore::{Gitignore, GitignoreBuilder};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct Entry {
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
    pub depth: usize,
    pub expanded: bool,
}

#[derive(Clone)]
pub struct Explorer {
    pub root: PathBuf,
    entries: Vec<Entry>,
    pub selected: usize,
    pub scroll: usize,
    expanded: BTreeSet<PathBuf>,
    /// Set when a directory could not be read, so the sidebar can say why.
    pub error: Option<String>,
    /// Show what git ignores.
    pub show_ignored: bool,
    ignores: Gitignore,
}

impl Explorer {
    pub fn new(root: PathBuf, show_ignored: bool) -> Self {
        let ignores = build_ignores(&root);
        let mut explorer = Self {
            root,
            show_ignored,
            ignores,
            entries: Vec::new(),
            selected: 0,
            scroll: 0,
            expanded: BTreeSet::new(),
            error: None,
        };
        explorer.refresh();
        explorer
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn selected_entry(&self) -> Option<&Entry> {
        self.entries.get(self.selected)
    }

    /// Rebuild the flattened tree from the root and the expanded set.
    pub fn refresh(&mut self) {
        let remembered = self.selected_entry().map(|entry| entry.path.clone());
        self.entries.clear();
        self.error = None;
        let root = self.root.clone();
        self.push_directory(&root, 0);
        // Keep the selection on the same path across a refresh where possible.
        if let Some(path) = remembered {
            if let Some(index) = self.entries.iter().position(|entry| entry.path == path) {
                self.selected = index;
            }
        }
        if self.selected >= self.entries.len() {
            self.selected = self.entries.len().saturating_sub(1);
        }
    }

    fn push_directory(&mut self, directory: &Path, depth: usize) {
        let listing = match std::fs::read_dir(directory) {
            Ok(listing) => listing,
            Err(e) => {
                if depth == 0 {
                    self.error = Some(format!("{e}"));
                }
                return;
            }
        };

        let mut children: Vec<(bool, String, PathBuf)> = listing
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let path = entry.path();
                let name = entry.file_name().to_string_lossy().into_owned();
                // `.git` is noise; other dotfiles are not — `.github` and
                // `.gitlab-ci.yml` are exactly what you came to edit.
                if name == ".git" {
                    return None;
                }
                let is_dir = entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false);
                if !self.show_ignored
                    && self
                        .ignores
                        .matched_path_or_any_parents(&path, is_dir)
                        .is_ignore()
                {
                    return None;
                }
                Some((is_dir, name, path))
            })
            .collect();
        // Directories first, then case-insensitive by name.
        children.sort_by(|a, b| {
            b.0.cmp(&a.0)
                .then_with(|| a.1.to_lowercase().cmp(&b.1.to_lowercase()))
        });

        for (is_dir, name, path) in children {
            let expanded = is_dir && self.expanded.contains(&path);
            self.entries.push(Entry {
                path: path.clone(),
                name,
                is_dir,
                depth,
                expanded,
            });
            if expanded {
                self.push_directory(&path, depth + 1);
            }
        }
    }

    pub fn move_selection(&mut self, delta: isize) {
        if self.entries.is_empty() {
            return;
        }
        let count = self.entries.len() as isize;
        self.selected = (self.selected as isize + delta).clamp(0, count - 1) as usize;
    }

    /// Expand or collapse the selected directory. Returns the file to open
    /// when the selection is not a directory.
    pub fn activate(&mut self) -> Option<PathBuf> {
        let entry = self.selected_entry()?.clone();
        if !entry.is_dir {
            return Some(entry.path);
        }
        if self.expanded.contains(&entry.path) {
            self.expanded.remove(&entry.path);
        } else {
            self.expanded.insert(entry.path.clone());
        }
        self.refresh();
        None
    }

    pub fn collapse(&mut self) {
        let Some(entry) = self.selected_entry().cloned() else {
            return;
        };
        if entry.is_dir && entry.expanded {
            self.expanded.remove(&entry.path);
            self.refresh();
            return;
        }
        // Already collapsed, or a file: step out to the parent.
        if let Some(parent) = entry.path.parent() {
            if let Some(index) = self.entries.iter().position(|e| e.path == parent) {
                self.selected = index;
            }
        }
    }

    pub fn expand(&mut self) {
        let Some(entry) = self.selected_entry().cloned() else {
            return;
        };
        if entry.is_dir && !entry.expanded {
            self.expanded.insert(entry.path);
            self.refresh();
        } else {
            self.move_selection(1);
        }
    }

    /// Keep the visible window around the selection.
    pub fn scroll_into_view(&mut self, height: usize) {
        if height == 0 {
            return;
        }
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + height {
            self.scroll = self.selected + 1 - height;
        }
    }
}

/// Collect the ignore rules that apply under `root`.
fn build_ignores(root: &Path) -> Gitignore {
    let mut builder = GitignoreBuilder::new(root);
    // Failures here just mean fewer rules, never a broken explorer.
    let _ = builder.add(root.join(".gitignore"));
    let _ = builder.add(root.join(".git/info/exclude"));
    builder.build().unwrap_or_else(|_| Gitignore::empty())
}

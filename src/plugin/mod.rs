//! Extension points.
//!
//! This is the groundwork for a plugin system rather than a finished one, but
//! it is deliberately not a stub: manifests load, commands they declare become
//! real commands, capabilities are recorded and shown, and events are
//! dispatched from the places they actually happen. What is missing is a host
//! process — the transport — and the shape here is chosen so adding one does
//! not change the contract.
//!
//! The design decision worth stating: a plugin is an **external process**, not
//! a dynamically linked library and not an embedded interpreter. miv already
//! runs external tools for checkers, formatters and git, and already speaks a
//! JSON protocol over a socket for shared sessions, so the machinery exists
//! and the failure modes are understood. A crashing plugin must not take the
//! editor with it.

pub mod api;
pub mod manifest;

use manifest::{Manifest, Manifesto};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Events plugins can be told about.
///
/// Dispatched from the code that performs them, so this list doubles as the
/// documentation of what a plugin can hook.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    BufferOpened {
        buffer: usize,
        path: Option<PathBuf>,
    },
    BufferSaved {
        buffer: usize,
        path: PathBuf,
    },
    /// The text changed. Coalesced: one per command, not one per keystroke.
    BufferChanged {
        buffer: usize,
    },
    ModeChanged {
        mode: &'static str,
    },
    DiagnosticsUpdated {
        buffer: usize,
        errors: usize,
        warnings: usize,
    },
    ParticipantJoined {
        name: String,
    },
    Quitting,
}

impl Event {
    pub fn label(&self) -> String {
        match self {
            Event::BufferOpened { path, .. } => format!(
                "buffer opened: {}",
                path.as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "[No Name]".to_string())
            ),
            Event::BufferSaved { path, .. } => format!("buffer saved: {}", path.display()),
            Event::BufferChanged { buffer } => format!("buffer {buffer} changed"),
            Event::ModeChanged { mode } => format!("mode: {mode}"),
            Event::DiagnosticsUpdated {
                errors, warnings, ..
            } => format!("diagnostics: {errors} error(s), {warnings} warning(s)"),
            Event::ParticipantJoined { name } => format!("{name} joined the session"),
            Event::Quitting => "quitting".to_string(),
        }
    }
}

/// What a plugin is allowed to do.
///
/// Declared in the manifest and shown by `:plugins`, so installing something
/// is an informed decision rather than a hopeful one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    ReadBuffer,
    WriteBuffer,
    RunCommands,
    /// Put things on screen: messages, pickers, virtual text.
    ShowUi,
    /// Reach the network. miv cannot enforce this — a subprocess does what it
    /// likes — so it is a disclosure, not a sandbox, and `:plugins` says so.
    Network,
}

impl Capability {
    pub fn label(self) -> &'static str {
        match self {
            Capability::ReadBuffer => "read",
            Capability::WriteBuffer => "write",
            Capability::RunCommands => "commands",
            Capability::ShowUi => "ui",
            Capability::Network => "network",
        }
    }
}

/// How a contributed command does its work.
#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// Pipe the target through a command and replace it with the output. This
    /// one covers a surprising amount: formatting, sorting, base64, jq.
    Filter,
    /// Run a command and show its output as a message.
    Report,
    /// Run a built-in ex command, for plugins that only want to bind things.
    Ex,
}

/// What a contributed command acts on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Target {
    #[default]
    Buffer,
    /// The visual selection, or the current line when there is none.
    Selection,
    Line,
}

#[derive(Clone, Debug)]
pub struct Command {
    pub name: String,
    pub description: String,
    /// Which plugin contributed it, for `:plugins` and for error messages.
    pub plugin: String,
    pub kind: Kind,
    pub target: Target,
    pub command: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct Plugin {
    pub manifest: Manifest,
    pub directory: PathBuf,
}

impl Plugin {
    pub fn grants(&self, capability: Capability) -> bool {
        self.manifest.capabilities.contains(&capability)
    }
}

/// Events kept for `:events`. Bounded: this is a debugging aid, not a log.
const EVENT_HISTORY: usize = 200;

#[derive(Default)]
pub struct Plugins {
    pub loaded: Vec<Plugin>,
    /// Commands contributed by plugins, by name.
    commands: HashMap<String, Command>,
    /// Anything that went wrong while loading, shown by `:plugins`.
    pub problems: Vec<String>,
    recent: Vec<Event>,
}

impl Plugins {
    /// Load every plugin under `directory`.
    ///
    /// A broken manifest is recorded and skipped: one bad plugin must not stop
    /// the editor from starting, or stop the others from loading.
    pub fn load(directory: &Path) -> Self {
        let mut plugins = Plugins::default();
        let Ok(listing) = std::fs::read_dir(directory) else {
            // No plugins directory is the normal case, not a problem.
            return plugins;
        };

        let mut entries: Vec<PathBuf> = listing
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.is_dir())
            .collect();
        entries.sort();

        for directory in entries {
            let path = directory.join("plugin.toml");
            if !path.exists() {
                continue;
            }
            match Manifesto::read(&path) {
                Ok(manifesto) => plugins.install(manifesto, directory),
                Err(e) => plugins.problems.push(format!("{}: {e}", path.display())),
            }
        }
        plugins
    }

    fn install(&mut self, manifesto: Manifesto, directory: PathBuf) {
        let Manifesto { manifest, commands } = manifesto;
        let name = manifest.name.clone();

        for declared in commands {
            // A command that writes needs to have said so.
            if declared.kind == Kind::Filter
                && !manifest.capabilities.contains(&Capability::WriteBuffer)
            {
                self.problems.push(format!(
                    "{name}: command {:?} filters the buffer but the plugin does not declare write_buffer",
                    declared.name
                ));
                continue;
            }
            if declared.kind == Kind::Ex
                && !manifest.capabilities.contains(&Capability::RunCommands)
            {
                self.problems.push(format!(
                    "{name}: command {:?} runs editor commands but the plugin does not declare run_commands",
                    declared.name
                ));
                continue;
            }
            if self.commands.contains_key(&declared.name) {
                self.problems.push(format!(
                    "{name}: command {:?} is already provided by another plugin",
                    declared.name
                ));
                continue;
            }
            self.commands.insert(
                declared.name.clone(),
                Command {
                    name: declared.name,
                    description: declared.description,
                    plugin: name.clone(),
                    kind: declared.kind,
                    target: declared.target,
                    command: declared.command,
                },
            );
        }

        self.loaded.push(Plugin {
            manifest,
            directory,
        });
    }

    pub fn command(&self, name: &str) -> Option<&Command> {
        self.commands.get(name)
    }

    pub fn commands(&self) -> Vec<&Command> {
        let mut all: Vec<&Command> = self.commands.values().collect();
        all.sort_by(|a, b| a.name.cmp(&b.name));
        all
    }

    pub fn is_empty(&self) -> bool {
        self.loaded.is_empty() && self.problems.is_empty()
    }

    /// Record an event. Where a host process exists, this is also where it
    /// would be forwarded to the plugins that asked for it.
    pub fn dispatch(&mut self, event: Event) {
        if self.recent.len() >= EVENT_HISTORY {
            self.recent.remove(0);
        }
        self.recent.push(event);
    }

    pub fn recent(&self) -> &[Event] {
        &self.recent
    }
}

/// Where plugins live: `<config dir>/miv/plugins/<name>/plugin.toml`.
pub fn default_directory() -> Option<PathBuf> {
    crate::config::default_config_path()
        .and_then(|path| path.parent().map(|parent| parent.join("plugins")))
}

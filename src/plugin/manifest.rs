//! `plugin.toml`.

use super::{Capability, Kind, Target};
use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::Path;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub name: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub description: String,
    /// What the plugin needs. Anything it declares is shown by `:plugins`.
    #[serde(default)]
    pub capabilities: Vec<Capability>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeclaredCommand {
    /// The `:name` it becomes. Must not collide with a built-in.
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub kind: Kind,
    #[serde(default)]
    pub target: Target,
    pub command: Vec<String>,
}

/// A manifest file: the plugin itself plus the commands it contributes.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    #[serde(flatten)]
    manifest: Manifest,
    #[serde(default, rename = "command")]
    commands: Vec<DeclaredCommand>,
}

pub struct Manifesto {
    pub manifest: Manifest,
    pub commands: Vec<DeclaredCommand>,
}

impl Manifesto {
    pub fn read(path: &Path) -> Result<Self> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        Self::parse(&text)
    }

    pub fn parse(text: &str) -> Result<Self> {
        let document: Document = toml::from_str(text).context("parsing the manifest")?;
        anyhow::ensure!(
            !document.manifest.name.trim().is_empty(),
            "a plugin needs a name"
        );
        for command in &document.commands {
            anyhow::ensure!(
                !command.name.trim().is_empty(),
                "a contributed command needs a name"
            );
            anyhow::ensure!(
                command.kind == Kind::Ex || !command.command.is_empty(),
                "command {:?} needs something to run",
                command.name
            );
        }
        Ok(Self {
            manifest: document.manifest,
            commands: document.commands,
        })
    }
}

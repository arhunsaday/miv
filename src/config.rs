//! Configuration loaded from a single TOML file.
//!
//! Unknown keys and bad values are hard errors rather than silent fallbacks:
//! a typo in your config should tell you so, not quietly do nothing.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LineNumbers {
    None,
    Absolute,
    Relative,
    /// Relative everywhere except the cursor line, which shows its absolute
    /// number. Vim's `number relativenumber`.
    Hybrid,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct EditorConfig {
    /// Screen columns a tab character occupies.
    pub tab_width: usize,
    /// Insert spaces instead of a tab character.
    pub expand_tab: bool,
    /// Columns shifted by `>>` and `<<`.
    pub shift_width: usize,
    /// Lines kept visible above and below the cursor.
    pub scrolloff: usize,
    pub line_numbers: LineNumbers,
    pub cursorline: bool,
    /// Case-insensitive search.
    pub ignore_case: bool,
    /// ...unless the pattern contains an uppercase character.
    pub smart_case: bool,
    /// Wrap around the end of the file when searching.
    pub wrap_search: bool,
}

impl Default for EditorConfig {
    fn default() -> Self {
        Self {
            tab_width: 4,
            expand_tab: true,
            shift_width: 4,
            scrolloff: 3,
            line_numbers: LineNumbers::Relative,
            cursorline: true,
            ignore_case: true,
            smart_case: true,
            wrap_search: true,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AppearanceConfig {
    /// A syntect theme name, e.g. `base16-ocean.dark`.
    pub theme: String,
    pub syntax_highlighting: bool,
    /// Give up on highlighting files longer than this, to keep editing snappy.
    pub max_highlight_lines: usize,
    /// Paint the theme's background instead of leaving the terminal's own.
    pub theme_background: bool,
}

impl Default for AppearanceConfig {
    fn default() -> Self {
        Self {
            theme: "base16-ocean.dark".to_string(),
            syntax_highlighting: true,
            max_highlight_lines: 50_000,
            theme_background: false,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SessionConfig {
    /// Interface the session listener binds to. Loopback by default: reaching
    /// it from another machine should be a deliberate act (an SSH tunnel, a
    /// VPN address), never the default.
    pub bind: String,
    /// 0 picks a free port, which is usually what you want.
    pub port: u16,
    /// The name other participants see. Defaults to `$USER`.
    pub name: String,
    /// What a participant may do the moment they connect.
    pub default_access: crate::session::protocol::Access,
    /// Announce joins, leaves and access changes in the message line.
    pub announce: bool,
    pub max_participants: usize,
    /// Draw other participants' carets and selections in your own view.
    pub show_remote_cursors: bool,
    /// Let guests run `:` commands.
    ///
    /// Off by default, and deliberately separate from write access: editing
    /// text is a much smaller grant than running `:w /some/other/path` or
    /// `:e` on any file the host can read. Turn this on only for people you
    /// would hand the keyboard to.
    pub guest_commands: bool,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            bind: "127.0.0.1".to_string(),
            port: 0,
            name: std::env::var("USER")
                .or_else(|_| std::env::var("USERNAME"))
                .unwrap_or_else(|_| "host".to_string()),
            default_access: crate::session::protocol::Access::Read,
            announce: true,
            max_participants: 8,
            show_remote_cursors: true,
            guest_commands: false,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub editor: EditorConfig,
    pub appearance: AppearanceConfig,
    pub session: SessionConfig,
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let config: Config =
            toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<()> {
        anyhow::ensure!(
            self.editor.tab_width > 0,
            "editor.tab_width must be at least 1"
        );
        anyhow::ensure!(
            self.editor.shift_width > 0,
            "editor.shift_width must be at least 1"
        );
        anyhow::ensure!(
            !self.session.bind.is_empty(),
            "session.bind must be an address to bind to"
        );
        anyhow::ensure!(
            self.session.max_participants >= 1,
            "session.max_participants must be at least 1"
        );
        Ok(())
    }
}

/// `$MIV_CONFIG`, else `$XDG_CONFIG_HOME/miv/config.toml`, else the platform
/// config directory. Predictable on unix, correct on Windows.
pub fn default_config_path() -> Option<PathBuf> {
    if let Some(explicit) = std::env::var_os("MIV_CONFIG") {
        return Some(PathBuf::from(explicit));
    }
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .or_else(|| std::env::var_os("APPDATA").map(PathBuf::from))?;
    Some(base.join("miv").join("config.toml"))
}

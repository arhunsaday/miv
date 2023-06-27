use anyhow::Result;
use config::{Config, ConfigError, File};
use serde::Deserialize;
use std::env;

#[derive(Debug, Deserialize)]
#[allow(unused)]
pub struct EditorConfig {
    pub indent_size: u8,
    pub tab_size: u8,
    pub line_numbers: String,
}

#[derive(Debug, Deserialize)]
#[allow(unused)]
pub struct AppearanceConfig {
    pub theme: String,
}

#[derive(Debug, Deserialize)]
#[allow(unused)]
pub struct GeneralConfig {
    debug: bool,
    log_level: String,
}

#[derive(Debug, Deserialize)]
#[allow(unused)]
pub struct Settings {
    pub editor: EditorConfig,
    pub appearance: AppearanceConfig,
    pub general: GeneralConfig,
}

impl Settings {
    pub fn new() -> Self {
        let file_name = "config.toml";
        let xdg_config_home = env::var("XDG_CONFIG_HOME");

        match xdg_config_home {
            Ok(path) => {
                let config_path = format!("{}/{}", path, file_name);
                Settings::load_config(config_path).unwrap_or_else(|_| Settings::default())
            }
            Err(_) => Settings::default(),
        }
    }

    pub fn load_config(path: String) -> Result<Self, ConfigError> {
        let config = Config::builder()
            .add_source(File::with_name(&path).required(false))
            .build()?;

        config.try_deserialize()
    }
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            editor: EditorConfig {
                indent_size: 4,
                tab_size: 4,
                line_numbers: "relative".to_string(),
            },
            appearance: AppearanceConfig {
                theme: "base16-ocean.dark".to_string(),
            },
            general: GeneralConfig {
                debug: false,
                log_level: "info".to_string(),
            },
        }
    }
}

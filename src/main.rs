mod document;
mod editor;
mod filetype;
mod highlighting;
mod mode;
mod row;
mod settings;
mod terminal;

pub use document::Document;
pub use editor::Editor;
pub use editor::{Position, SearchDirection};
pub use filetype::FileType;
pub use highlighting::Highlighting;
pub use mode::{Mode, PossibleModes};
pub use row::Row;
pub use settings::Settings;
pub use terminal::Terminal;

use log::info;
use simplelog::{Config, LevelFilter, WriteLogger};
use std::fs::File;

fn main() {
    init_logging();
    info!("Starting Miv");

    Editor::new(Settings::new()).run();
}

fn init_logging() {
    let _ = WriteLogger::init(
        LevelFilter::Info,
        Config::default(),
        File::create("miv.log").unwrap(),
    );
}

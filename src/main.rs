mod editor;

use editor::Editor;
pub use terminal::Terminal;

fn main() {
    Editor::default().run();
}

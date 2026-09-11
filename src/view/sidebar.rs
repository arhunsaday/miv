//! The sidebar: a strip beside the windows holding one of several views.
//!
//! Only the file explorer exists so far, but the view is an enum so adding the
//! next one does not change the layout or the key handling.

use super::explorer::Explorer;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    Explorer,
}

impl View {
    pub fn title(self) -> &'static str {
        match self {
            View::Explorer => "Explorer",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

#[derive(Clone)]
pub struct Sidebar {
    pub visible: bool,
    /// Whether the keyboard is currently going to the sidebar rather than to a
    /// window.
    pub focused: bool,
    pub width: u16,
    pub side: Side,
    pub view: View,
    pub explorer: Explorer,
}

impl Sidebar {
    pub fn new(root: std::path::PathBuf, config: &crate::config::SidebarConfig) -> Self {
        Self {
            visible: config.open_on_start,
            focused: false,
            width: config.width,
            side: match config.side {
                crate::config::SidebarSide::Left => Side::Left,
                crate::config::SidebarSide::Right => Side::Right,
            },
            view: View::Explorer,
            explorer: Explorer::new(root, config.show_ignored),
        }
    }

    /// Show and focus, or hide and give the keyboard back.
    pub fn toggle(&mut self) {
        self.visible = !self.visible;
        self.focused = self.visible;
        if self.visible {
            self.explorer.refresh();
        }
    }

    pub fn width(&self) -> u16 {
        if self.visible {
            self.width
        } else {
            0
        }
    }
}

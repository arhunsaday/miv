//! Windows: views onto buffers.
//!
//! The focused window's cursor and viewport live on the buffer itself, which
//! is what lets every motion, operator and command stay unaware that windows
//! exist. Unfocused windows keep their state here, and changing focus swaps
//! the two — the same trick a shared session uses to give each participant
//! their own cursor.

use super::layout::{neighbour, Direction, Layout, WindowId};
use super::sidebar::Sidebar;
use crate::core::text::Position;
use ratatui::layout::Rect;

#[derive(Clone, Debug)]
pub struct Window {
    pub id: WindowId,
    pub buffer_index: usize,
    pub cursor: Position,
    pub desired_col: usize,
    pub view_top: usize,
    pub view_left: usize,
    pub visual_anchor: Position,
    /// Where this window was last drawn. Directional focus and the mouse both
    /// need to know where things actually are on screen.
    pub area: Rect,
}

impl Window {
    fn new(id: WindowId, buffer_index: usize) -> Self {
        Self {
            id,
            buffer_index,
            cursor: Position::default(),
            desired_col: 0,
            view_top: 0,
            view_left: 0,
            visual_anchor: Position::default(),
            area: Rect::default(),
        }
    }
}

#[derive(Clone)]
pub struct Workspace {
    windows: Vec<Window>,
    pub layout: Layout,
    focused: WindowId,
    next_id: WindowId,
    pub sidebar: Sidebar,
}

impl Workspace {
    pub fn new(buffer_index: usize, sidebar: &crate::config::SidebarConfig) -> Self {
        Self {
            windows: vec![Window::new(0, buffer_index)],
            layout: Layout::Window(0),
            focused: 0,
            next_id: 1,
            sidebar: Sidebar::new(
                std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
                sidebar,
            ),
        }
    }

    pub fn focused_id(&self) -> WindowId {
        self.focused
    }

    pub fn focused(&self) -> &Window {
        self.window(self.focused)
            .expect("the focused window always exists")
    }

    pub fn focused_mut(&mut self) -> &mut Window {
        let focused = self.focused;
        self.window_mut(focused)
            .expect("the focused window always exists")
    }

    pub fn window(&self, id: WindowId) -> Option<&Window> {
        self.windows.iter().find(|window| window.id == id)
    }

    pub fn window_mut(&mut self, id: WindowId) -> Option<&mut Window> {
        self.windows.iter_mut().find(|window| window.id == id)
    }

    pub fn windows(&self) -> &[Window] {
        &self.windows
    }

    pub fn count(&self) -> usize {
        self.windows.len()
    }

    /// Split the focused window, giving the new one the same view. Returns the
    /// new window's id.
    pub fn split(&mut self, vertical: bool) -> WindowId {
        let id = self.next_id;
        self.next_id += 1;
        let mut window = self.focused().clone();
        window.id = id;
        self.layout.split(self.focused, id, vertical);
        self.windows.push(window);
        id
    }

    /// Close the focused window. Fails when it is the last one, because there
    /// is nothing sensible to focus next.
    pub fn close_focused(&mut self) -> bool {
        if self.windows.len() < 2 {
            return false;
        }
        let closing = self.focused;
        self.layout.remove(closing);
        self.windows.retain(|window| window.id != closing);
        self.focused = self.layout.windows().first().copied().unwrap_or(0);
        true
    }

    /// Close everything except the focused window.
    pub fn only(&mut self) -> bool {
        if self.windows.len() < 2 {
            return false;
        }
        let keep = self.focused;
        self.windows.retain(|window| window.id == keep);
        self.layout = Layout::Window(keep);
        true
    }

    pub fn focus(&mut self, id: WindowId) -> bool {
        if self.window(id).is_some() {
            self.focused = id;
            true
        } else {
            false
        }
    }

    /// Cycle in layout order, which is what `Ctrl-w w` does.
    pub fn next(&self) -> WindowId {
        let order = self.layout.windows();
        let position = order.iter().position(|id| *id == self.focused).unwrap_or(0);
        order
            .get((position + 1) % order.len().max(1))
            .copied()
            .unwrap_or(self.focused)
    }

    pub fn previous(&self) -> WindowId {
        let order = self.layout.windows();
        let position = order.iter().position(|id| *id == self.focused).unwrap_or(0);
        let count = order.len().max(1);
        order
            .get((position + count - 1) % count)
            .copied()
            .unwrap_or(self.focused)
    }

    pub fn toward(&self, direction: Direction) -> Option<WindowId> {
        let areas: Vec<(WindowId, Rect)> = self
            .windows
            .iter()
            .map(|window| (window.id, window.area))
            .collect();
        neighbour(&areas, self.focused, direction)
    }

    /// Recompute every window's rectangle, and report the separators to draw.
    pub fn arrange(&mut self, area: Rect) -> Vec<Rect> {
        let (placed, separators) = self.layout.arrange(area);
        for (id, rect) in placed {
            if let Some(window) = self.window_mut(id) {
                window.area = rect;
            }
        }
        separators
    }

    /// Point every window at a different buffer index when one is removed, so
    /// no window is left referring to a buffer that has moved or gone.
    pub fn buffer_removed(&mut self, removed: usize, fallback: usize) {
        for window in &mut self.windows {
            match window.buffer_index.cmp(&removed) {
                std::cmp::Ordering::Equal => {
                    window.buffer_index = fallback;
                    window.cursor = Position::default();
                    window.view_top = 0;
                }
                std::cmp::Ordering::Greater => window.buffer_index -= 1,
                std::cmp::Ordering::Less => {}
            }
        }
    }
}

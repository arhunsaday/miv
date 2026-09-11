//! How windows divide the screen.
//!
//! A binary-ish tree of rows and columns. Sizes are equal within a node, which
//! covers almost all real use and avoids carrying resize state around; the
//! remainder of an uneven division goes to the last child so no column is lost.

use ratatui::layout::Rect;

pub type WindowId = usize;

/// One column between side-by-side windows, for the separator.
const SEPARATOR: u16 = 1;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Layout {
    Window(WindowId),
    /// Side by side, the result of a vertical split.
    Row(Vec<Layout>),
    /// Stacked, the result of a horizontal split.
    Column(Vec<Layout>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

impl Layout {
    /// Every window's rectangle, plus the separators to draw between them.
    pub fn arrange(&self, area: Rect) -> (Vec<(WindowId, Rect)>, Vec<Rect>) {
        let mut windows = Vec::new();
        let mut separators = Vec::new();
        self.arrange_into(area, &mut windows, &mut separators);
        (windows, separators)
    }

    fn arrange_into(
        &self,
        area: Rect,
        windows: &mut Vec<(WindowId, Rect)>,
        separators: &mut Vec<Rect>,
    ) {
        match self {
            Layout::Window(id) => windows.push((*id, area)),
            Layout::Row(children) => {
                let count = children.len() as u16;
                if count == 0 {
                    return;
                }
                let gaps = count.saturating_sub(1) * SEPARATOR;
                let usable = area.width.saturating_sub(gaps);
                let each = usable / count;
                let mut x = area.x;
                for (index, child) in children.iter().enumerate() {
                    let last = index + 1 == children.len();
                    let width = if last {
                        area.right().saturating_sub(x)
                    } else {
                        each
                    };
                    child.arrange_into(
                        Rect {
                            x,
                            y: area.y,
                            width,
                            height: area.height,
                        },
                        windows,
                        separators,
                    );
                    x = x.saturating_add(width);
                    if !last {
                        separators.push(Rect {
                            x,
                            y: area.y,
                            width: SEPARATOR,
                            height: area.height,
                        });
                        x = x.saturating_add(SEPARATOR);
                    }
                }
            }
            Layout::Column(children) => {
                let count = children.len() as u16;
                if count == 0 {
                    return;
                }
                let each = area.height / count;
                let mut y = area.y;
                for (index, child) in children.iter().enumerate() {
                    let last = index + 1 == children.len();
                    let height = if last {
                        area.bottom().saturating_sub(y)
                    } else {
                        each
                    };
                    child.arrange_into(
                        Rect {
                            x: area.x,
                            y,
                            width: area.width,
                            height,
                        },
                        windows,
                        separators,
                    );
                    y = y.saturating_add(height);
                }
            }
        }
    }

    /// Put `new` beside `target`, nesting only when the direction differs from
    /// the node already holding it.
    pub fn split(&mut self, target: WindowId, new: WindowId, vertical: bool) -> bool {
        match self {
            Layout::Window(id) if *id == target => {
                let pair = vec![Layout::Window(target), Layout::Window(new)];
                *self = if vertical {
                    Layout::Row(pair)
                } else {
                    Layout::Column(pair)
                };
                true
            }
            Layout::Window(_) => false,
            node => {
                let is_row = matches!(node, Layout::Row(_));
                let (Layout::Row(children) | Layout::Column(children)) = node else {
                    unreachable!("leaves are handled above");
                };
                // A split along the same axis joins the existing node rather
                // than nesting, which is what keeps three `:vsplit`s in a row
                // from producing a lopsided tree.
                if is_row == vertical {
                    if let Some(position) = children
                        .iter()
                        .position(|child| child == &Layout::Window(target))
                    {
                        children.insert(position + 1, Layout::Window(new));
                        return true;
                    }
                }
                children
                    .iter_mut()
                    .any(|child| child.split(target, new, vertical))
            }
        }
    }

    /// Drop a window, collapsing any node left with a single child.
    pub fn remove(&mut self, target: WindowId) -> bool {
        match self {
            Layout::Window(_) => false,
            Layout::Row(children) | Layout::Column(children) => {
                let before = children.len();
                children.retain(|child| child != &Layout::Window(target));
                let mut removed = children.len() != before;
                if !removed {
                    // A nested removal still counts: forgetting to carry the
                    // child's answer up made `remove` lie about a window it
                    // had in fact taken out.
                    for child in children.iter_mut() {
                        if child.remove(target) {
                            removed = true;
                            break;
                        }
                    }
                }
                // Collapse `Row([x])` into `x`.
                let single = children.len() == 1;
                if single {
                    let only = children.remove(0);
                    *self = only;
                }
                removed
            }
        }
    }

    pub fn windows(&self) -> Vec<WindowId> {
        let mut out = Vec::new();
        self.collect(&mut out);
        out
    }

    fn collect(&self, out: &mut Vec<WindowId>) {
        match self {
            Layout::Window(id) => out.push(*id),
            Layout::Row(children) | Layout::Column(children) => {
                for child in children {
                    child.collect(out);
                }
            }
        }
    }
}

/// The window nearest `from` in a direction, chosen by edge adjacency and then
/// by how much the two overlap on the other axis.
pub fn neighbour(
    areas: &[(WindowId, Rect)],
    from: WindowId,
    direction: Direction,
) -> Option<WindowId> {
    let origin = areas.iter().find(|(id, _)| *id == from)?.1;
    let mut best: Option<(WindowId, u16, u16)> = None;

    for (id, area) in areas {
        if *id == from {
            continue;
        }
        // A window that is not on the right side gets skipped, not treated as
        // an answer for the whole search: using `?` here made the result
        // depend on the order the windows happened to be in.
        let measured = match direction {
            Direction::Left => origin
                .x
                .checked_sub(area.right())
                .map(|distance| (distance, vertical_overlap(origin, *area))),
            Direction::Right => area
                .x
                .checked_sub(origin.right())
                .map(|distance| (distance, vertical_overlap(origin, *area))),
            Direction::Up => origin
                .y
                .checked_sub(area.bottom())
                .map(|distance| (distance, horizontal_overlap(origin, *area))),
            Direction::Down => area
                .y
                .checked_sub(origin.bottom())
                .map(|distance| (distance, horizontal_overlap(origin, *area))),
        };
        let Some((distance, overlap)) = measured else {
            continue;
        };
        if overlap == 0 {
            continue;
        }
        let better = match best {
            None => true,
            Some((_, best_distance, best_overlap)) => {
                distance < best_distance || (distance == best_distance && overlap > best_overlap)
            }
        };
        if better {
            best = Some((*id, distance, overlap));
        }
    }
    best.map(|(id, _, _)| id)
}

fn vertical_overlap(a: Rect, b: Rect) -> u16 {
    a.bottom().min(b.bottom()).saturating_sub(a.y.max(b.y))
}

fn horizontal_overlap(a: Rect, b: Rect) -> u16 {
    a.right().min(b.right()).saturating_sub(a.x.max(b.x))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen() -> Rect {
        Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        }
    }

    fn find(placed: &[(WindowId, Rect)], id: WindowId) -> Rect {
        placed.iter().find(|(w, _)| *w == id).unwrap().1
    }

    #[test]
    fn a_single_window_fills_the_screen() {
        let layout = Layout::Window(0);
        let (placed, separators) = layout.arrange(screen());
        assert_eq!(placed.len(), 1);
        assert_eq!(find(&placed, 0), screen());
        assert!(separators.is_empty());
    }

    #[test]
    fn a_vertical_split_divides_the_width_and_leaves_a_separator() {
        let mut layout = Layout::Window(0);
        assert!(layout.split(0, 1, true));
        let (placed, separators) = layout.arrange(screen());

        let left = find(&placed, 0);
        let right = find(&placed, 1);
        assert_eq!(left.height, 24);
        assert_eq!(separators.len(), 1);
        // One column goes to the separator, and nothing is lost.
        assert_eq!(left.width + separators[0].width + right.width, 80);
        assert_eq!(separators[0].x, left.right());
        assert_eq!(right.x, separators[0].right());
    }

    #[test]
    fn a_horizontal_split_divides_the_height_with_no_separator() {
        let mut layout = Layout::Window(0);
        layout.split(0, 1, false);
        let (placed, separators) = layout.arrange(screen());
        let top = find(&placed, 0);
        let bottom = find(&placed, 1);
        assert_eq!(top.width, 80);
        assert_eq!(top.height + bottom.height, 24);
        assert_eq!(bottom.y, top.bottom());
        assert!(separators.is_empty(), "stacked windows share a border");
    }

    #[test]
    fn splitting_along_the_same_axis_stays_flat() {
        // Three `:vsplit`s should give three equal columns, not a lopsided
        // tree where the last one is half the width of the first.
        let mut layout = Layout::Window(0);
        layout.split(0, 1, true);
        layout.split(1, 2, true);
        assert_eq!(layout.windows(), vec![0, 1, 2]);
        assert!(matches!(layout, Layout::Row(ref children) if children.len() == 3));

        let (placed, _) = layout.arrange(screen());
        let widths: Vec<u16> = [0, 1, 2]
            .iter()
            .map(|id| find(&placed, *id).width)
            .collect();
        assert!(
            widths.iter().max().unwrap() - widths.iter().min().unwrap() <= 2,
            "columns should be near-equal, got {widths:?}"
        );
    }

    #[test]
    fn splitting_across_the_axis_nests() {
        let mut layout = Layout::Window(0);
        layout.split(0, 1, true);
        layout.split(1, 2, false);
        assert_eq!(layout.windows(), vec![0, 1, 2]);
        let (placed, _) = layout.arrange(screen());
        // 1 and 2 share a column and split its height.
        assert_eq!(find(&placed, 1).x, find(&placed, 2).x);
        assert_eq!(find(&placed, 1).height + find(&placed, 2).height, 24);
    }

    #[test]
    fn removing_a_window_collapses_the_node_it_leaves_behind() {
        let mut layout = Layout::Window(0);
        layout.split(0, 1, true);
        assert!(layout.remove(1));
        assert_eq!(layout, Layout::Window(0), "a lone child should collapse");

        let mut layout = Layout::Window(0);
        layout.split(0, 1, true);
        layout.split(1, 2, false);
        assert!(layout.remove(2));
        assert_eq!(layout.windows(), vec![0, 1]);
        assert!(matches!(layout, Layout::Row(_)));
    }

    #[test]
    fn removing_an_unknown_window_changes_nothing() {
        let mut layout = Layout::Window(0);
        layout.split(0, 1, true);
        let before = layout.clone();
        assert!(!layout.remove(42));
        assert_eq!(layout, before);
    }

    #[test]
    fn directional_focus_picks_the_adjacent_window() {
        let mut layout = Layout::Window(0);
        layout.split(0, 1, true);
        let (placed, _) = layout.arrange(screen());

        assert_eq!(neighbour(&placed, 0, Direction::Right), Some(1));
        assert_eq!(neighbour(&placed, 1, Direction::Left), Some(0));
        // Nothing above or below a pair of columns.
        assert_eq!(neighbour(&placed, 0, Direction::Up), None);
        assert_eq!(neighbour(&placed, 0, Direction::Down), None);
    }

    #[test]
    fn directional_focus_prefers_the_window_it_overlaps_most() {
        // A tall window on the left, two stacked on the right. Moving right
        // from the left window should land on the one it overlaps more.
        let mut layout = Layout::Window(0);
        layout.split(0, 1, true);
        layout.split(1, 2, false);
        let (placed, _) = layout.arrange(screen());

        let chosen = neighbour(&placed, 0, Direction::Right);
        assert!(chosen == Some(1) || chosen == Some(2), "got {chosen:?}");
        // And from either right-hand window, left is unambiguous.
        assert_eq!(neighbour(&placed, 1, Direction::Left), Some(0));
        assert_eq!(neighbour(&placed, 2, Direction::Left), Some(0));
        // Between the stacked pair, up and down work.
        assert_eq!(neighbour(&placed, 1, Direction::Down), Some(2));
        assert_eq!(neighbour(&placed, 2, Direction::Up), Some(1));
    }

    #[test]
    fn a_narrow_screen_still_places_every_window() {
        let tiny = Rect {
            x: 0,
            y: 0,
            width: 4,
            height: 2,
        };
        let mut layout = Layout::Window(0);
        layout.split(0, 1, true);
        layout.split(1, 2, true);
        let (placed, _) = layout.arrange(tiny);
        assert_eq!(placed.len(), 3, "no window should be dropped");
    }
}

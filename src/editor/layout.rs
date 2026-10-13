//! Editor splits: a tree of panes, each showing one buffer.
//!
//! The same shape the terminal's panes use, so the two feel alike: a binary
//! tree of splits with a ratio, and a focus path of `false`/`true` steps.

/// Which way a split divides its area.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum SplitDir {
    /// Side by side.
    Right,
    /// Stacked.
    Down,
}

/// A node in the editor's layout tree.
#[derive(Clone, PartialEq, Debug)]
pub(crate) enum LayoutNode {
    /// A pane showing the buffer at this index.
    Leaf(usize),
    Split {
        dir: SplitDir,
        /// The first child's share, clamped to 0.1..=0.9.
        ratio: f32,
        first: Box<LayoutNode>,
        second: Box<LayoutNode>,
    },
}

/// The gap between panes, in points.
pub(crate) const SPLIT_GAP: f32 = 6.0;

/// The editor's pane layout.
#[derive(Clone, Debug)]
pub(crate) struct Layout {
    pub(crate) root: LayoutNode,
    /// The path to the focused leaf: `false` takes the first child.
    pub(crate) focus: Vec<bool>,
}

impl Default for Layout {
    fn default() -> Self {
        Self::new()
    }
}

impl Layout {
    pub(crate) fn new() -> Self {
        Self {
            root: LayoutNode::Leaf(0),
            focus: Vec::new(),
        }
    }

    /// Split the focused pane, showing `buffer` in the new second child.
    pub(crate) fn split(&mut self, dir: SplitDir, buffer: usize) {
        let path = self.focus.clone();
        let node = node_at_mut(&mut self.root, &path);
        let old = std::mem::replace(node, LayoutNode::Leaf(0));
        *node = LayoutNode::Split {
            dir,
            ratio: 0.5,
            first: Box::new(old),
            second: Box::new(LayoutNode::Leaf(buffer)),
        };
        self.focus.push(true);
    }

    /// Close the focused pane. Returns the buffer it was showing, or `None`
    /// when it was the last pane.
    pub(crate) fn close(&mut self) -> Option<usize> {
        if self.focus.is_empty() {
            return None;
        }
        let path = self.focus.clone();
        let parent_path = &path[..path.len() - 1];
        let took_second = *path.last().unwrap();
        let parent = node_at_mut(&mut self.root, parent_path);
        let LayoutNode::Split { first, second, .. } = parent else {
            return None;
        };
        let (keep, drop) = if took_second {
            (first.as_ref().clone(), second.as_ref().clone())
        } else {
            (second.as_ref().clone(), first.as_ref().clone())
        };
        let closed = match drop {
            LayoutNode::Leaf(index) => index,
            _ => return None,
        };
        *parent = keep;
        self.focus.pop();
        Some(closed)
    }

    /// The buffer index of the focused pane.
    pub(crate) fn focused(&self) -> usize {
        match node_at(&self.root, &self.focus) {
            LayoutNode::Leaf(index) => *index,
            LayoutNode::Split { .. } => 0,
        }
    }

    /// Point the focused pane at a buffer.
    pub(crate) fn set_focused(&mut self, buffer: usize) {
        let path = self.focus.clone();
        if let LayoutNode::Leaf(index) = node_at_mut(&mut self.root, &path) {
            *index = buffer;
        }
    }

    /// Move focus to the next or previous pane.
    pub(crate) fn focus_next(&mut self, forward: bool) {
        let mut leaves = Vec::new();
        collect_paths(&self.root, &mut Vec::new(), &mut leaves);
        if leaves.len() < 2 {
            return;
        }
        let current = leaves.iter().position(|p| *p == self.focus).unwrap_or(0);
        let next = if forward {
            (current + 1) % leaves.len()
        } else {
            (current + leaves.len() - 1) % leaves.len()
        };
        self.focus = leaves[next].clone();
    }

    /// Every buffer index shown, in reading order.
    pub(crate) fn leaves(&self) -> Vec<usize> {
        let mut out = Vec::new();
        collect_leaves(&self.root, &mut out);
        out
    }

    /// How many panes there are.
    pub(crate) fn pane_count(&self) -> usize {
        self.leaves().len()
    }

    /// Remap buffer indices after one is removed.
    pub(crate) fn remap(&mut self, removed: usize) {
        remap_node(&mut self.root, removed);
    }

    /// The rect of every pane, given the area to tile.
    pub(crate) fn rects(&self, area: egui::Rect) -> Vec<(usize, egui::Rect)> {
        let mut out = Vec::new();
        rects_of(&self.root, area, &mut out);
        out
    }

    /// The draggable dividers, with the path that identifies each one.
    pub(crate) fn dividers(&self, area: egui::Rect) -> Vec<(Vec<bool>, egui::Rect)> {
        let mut out = Vec::new();
        dividers_of(&self.root, area, &mut Vec::new(), &mut out);
        out
    }

    /// Set the ratio of the split at a path.
    pub(crate) fn set_ratio(&mut self, path: &[bool], ratio: f32) {
        if let LayoutNode::Split { ratio: r, .. } = node_at_mut(&mut self.root, path) {
            *r = ratio.clamp(0.1, 0.9);
        }
    }
}

fn node_at<'a>(node: &'a LayoutNode, path: &[bool]) -> &'a LayoutNode {
    match path.split_first() {
        None => node,
        Some((second, rest)) => match node {
            LayoutNode::Split {
                first, second: s, ..
            } => node_at(if *second { s } else { first }, rest),
            LayoutNode::Leaf(_) => node,
        },
    }
}

fn node_at_mut<'a>(node: &'a mut LayoutNode, path: &[bool]) -> &'a mut LayoutNode {
    match path.split_first() {
        None => node,
        Some((second, rest)) => match node {
            LayoutNode::Split {
                first, second: s, ..
            } => node_at_mut(if *second { s } else { first }, rest),
            LayoutNode::Leaf(_) => node,
        },
    }
}

fn collect_leaves(node: &LayoutNode, out: &mut Vec<usize>) {
    match node {
        LayoutNode::Leaf(index) => out.push(*index),
        LayoutNode::Split { first, second, .. } => {
            collect_leaves(first, out);
            collect_leaves(second, out);
        }
    }
}

fn collect_paths(node: &LayoutNode, path: &mut Vec<bool>, out: &mut Vec<Vec<bool>>) {
    match node {
        LayoutNode::Leaf(_) => out.push(path.clone()),
        LayoutNode::Split { first, second, .. } => {
            path.push(false);
            collect_paths(first, path, out);
            path.pop();
            path.push(true);
            collect_paths(second, path, out);
            path.pop();
        }
    }
}

fn remap_node(node: &mut LayoutNode, removed: usize) {
    match node {
        LayoutNode::Leaf(index) => {
            if *index > removed {
                *index -= 1;
            } else if *index == removed {
                *index = removed.saturating_sub(1);
            }
        }
        LayoutNode::Split { first, second, .. } => {
            remap_node(first, removed);
            remap_node(second, removed);
        }
    }
}

fn rects_of(node: &LayoutNode, area: egui::Rect, out: &mut Vec<(usize, egui::Rect)>) {
    match node {
        LayoutNode::Leaf(index) => out.push((*index, area)),
        LayoutNode::Split {
            dir,
            ratio,
            first,
            second,
        } => {
            let (a, b) = split_rect(area, *dir, *ratio);
            rects_of(first, a, out);
            rects_of(second, b, out);
        }
    }
}

fn split_rect(area: egui::Rect, dir: SplitDir, ratio: f32) -> (egui::Rect, egui::Rect) {
    let half = SPLIT_GAP * 0.5;
    match dir {
        SplitDir::Right => {
            let cut = area.min.x + area.width() * ratio;
            let first = egui::Rect::from_min_max(area.min, egui::pos2(cut - half, area.max.y));
            let second = egui::Rect::from_min_max(egui::pos2(cut + half, area.min.y), area.max);
            (first, second)
        }
        SplitDir::Down => {
            let cut = area.min.y + area.height() * ratio;
            let first = egui::Rect::from_min_max(area.min, egui::pos2(area.max.x, cut - half));
            let second = egui::Rect::from_min_max(egui::pos2(area.min.x, cut + half), area.max);
            (first, second)
        }
    }
}

fn dividers_of(
    node: &LayoutNode,
    area: egui::Rect,
    path: &mut Vec<bool>,
    out: &mut Vec<(Vec<bool>, egui::Rect)>,
) {
    if let LayoutNode::Split {
        dir,
        ratio,
        first,
        second,
    } = node
    {
        let (a, b) = split_rect(area, *dir, *ratio);
        let bar = match dir {
            SplitDir::Right => egui::Rect::from_min_max(
                egui::pos2(a.max.x, area.min.y),
                egui::pos2(b.min.x, area.max.y),
            ),
            SplitDir::Down => egui::Rect::from_min_max(
                egui::pos2(area.min.x, a.max.y),
                egui::pos2(area.max.x, b.min.y),
            ),
        };
        out.push((path.clone(), bar));
        path.push(false);
        dividers_of(first, a, path, out);
        path.pop();
        path.push(true);
        dividers_of(second, b, path, out);
        path.pop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area() -> egui::Rect {
        egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1000.0, 600.0))
    }

    #[test]
    fn a_fresh_layout_has_one_pane() {
        let layout = Layout::new();
        assert_eq!(layout.pane_count(), 1);
        assert_eq!(layout.focused(), 0);
        assert_eq!(layout.rects(area()).len(), 1);
    }

    #[test]
    fn splitting_adds_a_pane_and_focuses_it() {
        let mut layout = Layout::new();
        layout.split(SplitDir::Right, 1);
        assert_eq!(layout.pane_count(), 2);
        assert_eq!(layout.focused(), 1, "the new pane takes focus");
        assert_eq!(layout.leaves(), vec![0, 1]);
    }

    #[test]
    fn panes_tile_the_area_without_overlapping() {
        let mut layout = Layout::new();
        layout.split(SplitDir::Right, 1);
        layout.split(SplitDir::Down, 2);
        let rects = layout.rects(area());
        assert_eq!(rects.len(), 3);
        let total: f32 = rects.iter().map(|(_, r)| r.width() * r.height()).sum();
        let full = area().width() * area().height();
        assert!(total < full, "the gaps between panes are not covered");
        assert!(total > full * 0.9, "the panes should fill most of the area");
        for (_, r) in &rects {
            assert!(area().contains_rect(*r), "a pane escaped the area");
        }
    }

    #[test]
    fn closing_a_pane_keeps_the_other_one() {
        let mut layout = Layout::new();
        layout.split(SplitDir::Right, 1);
        let closed = layout.close();
        assert_eq!(closed, Some(1));
        assert_eq!(layout.pane_count(), 1);
        assert_eq!(layout.focused(), 0);
        // The last pane cannot be closed.
        assert_eq!(layout.close(), None);
    }

    #[test]
    fn focus_cycles_through_panes() {
        let mut layout = Layout::new();
        layout.split(SplitDir::Right, 1);
        layout.split(SplitDir::Down, 2);
        assert_eq!(layout.pane_count(), 3);
        let first = layout.focused();
        layout.focus_next(true);
        assert_ne!(layout.focused(), first);
        layout.focus_next(false);
        assert_eq!(layout.focused(), first);
    }

    #[test]
    fn remap_follows_a_removed_buffer() {
        let mut layout = Layout::new();
        layout.split(SplitDir::Right, 2);
        layout.remap(1);
        assert_eq!(layout.leaves(), vec![0, 1], "buffer 2 became 1");
    }

    #[test]
    fn dividers_sit_between_the_panes() {
        let mut layout = Layout::new();
        layout.split(SplitDir::Right, 1);
        let dividers = layout.dividers(area());
        assert_eq!(dividers.len(), 1);
        let (path, bar) = &dividers[0];
        assert!(path.is_empty(), "the root split has the empty path");
        assert!(bar.width() > 0.0 && bar.height() > 0.0);
        let rects = layout.rects(area());
        let left = rects[0].1;
        let right = rects[1].1;
        assert!(bar.min.x >= left.max.x - 0.01);
        assert!(bar.max.x <= right.min.x + 0.01);
    }

    #[test]
    fn setting_a_ratio_moves_the_divider() {
        let mut layout = Layout::new();
        layout.split(SplitDir::Right, 1);
        layout.set_ratio(&[], 0.25);
        let rects = layout.rects(area());
        assert!(rects[0].1.width() < rects[1].1.width());
        // Out-of-range ratios are clamped, not accepted.
        layout.set_ratio(&[], 5.0);
        let rects = layout.rects(area());
        assert!(rects[0].1.width() < area().width() * 0.95);
    }
}

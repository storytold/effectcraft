//! The output side shared by the PDF and PostScript interpreters: an open-group stack (clipping
//! groups, optional-content layers) that collects shapes into the SVG render tree.

use effectcraft_svg::{Affine, BezPath, FillRule, Geom, Group, Node, Paint, Shape, Stroke};

/// The most elements a path being built holds: an operator that would add more stops the
/// PostScript program, or the PDF content stream.
pub(crate) const MAX_PATH: usize = 1 << 20;

/// The deepest the open groups nest (the render tree and its consumers recurse through them).
const MAX_DEPTH: usize = 256;

/// The most characters of a logical text label kept in a group name.
pub(crate) const MAX_LABEL: usize = 256;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Plain,
    Clip,
    Layer,
}

pub(crate) struct Builder {
    stack: Vec<(Group, Kind)>,
    /// Logical text labels never share the graphics-state group stack.
    text_labels: Vec<String>,
    text_run: bool,
    /// Which of the finished top-level nodes are optional-content layers.
    pub top_layers: Vec<bool>,
    pub skipped: Vec<String>,
    pub shapes: usize,
    /// Groups the finished nodes will sit under once placed (an enclosing builder's open groups
    /// and the pattern or mask groups around them), counted against [`MAX_DEPTH`].
    base: usize,
}

pub(crate) fn group(name: &str) -> Group {
    Group::new(name)
}

impl Builder {
    pub fn new() -> Builder {
        Builder { stack: vec![(group("page"), Kind::Plain)], text_labels: vec![], text_run: false, top_layers: vec![], skipped: vec![], shapes: 0, base: 0 }
    }

    /// A builder for content that will be placed inside this one's open groups and `wrap` more
    /// groups (a pattern's `Pattern` and `Tile`, a soft mask's two), so its nesting limit counts
    /// them.
    pub fn nested(&self, wrap: usize) -> Builder {
        Builder { base: self.base.saturating_add(self.stack.len()).saturating_add(self.text_labels.len()).saturating_add(wrap), ..Builder::new() }
    }

    pub fn depth(&self) -> usize {
        self.stack.len()
    }

    /// `wrap` more groups fit under the open ones within [`MAX_DEPTH`].
    pub fn has_room(&self, wrap: usize) -> bool {
        self.base.saturating_add(self.stack.len()).saturating_add(self.text_labels.len()).saturating_add(wrap) <= MAX_DEPTH
    }

    pub fn skip(&mut self, what: &str) {
        if !self.skipped.iter().any(|s| s == what) {
            self.skipped.push(what.to_string());
        }
    }

    fn add(&mut self, n: Node, layer: bool) {
        self.text_run = false;
        if self.stack.len() == 1 {
            self.top_layers.push(layer);
        }
        if let Some((g, _)) = self.stack.last_mut() {
            g.children.push(n);
        }
    }

    /// Close every group opened above `depth`.
    pub fn close_to(&mut self, depth: usize) {
        if self.stack.len() > depth.max(1) {
            self.text_run = false;
        }
        while self.stack.len() > depth.max(1) {
            let Some((g, k)) = self.stack.pop() else { break };
            // Empty clipping groups draw nothing.
            if g.children.is_empty() {
                continue;
            }
            self.add(Node::Group(g), k == Kind::Layer);
        }
    }

    /// Open `g` as the group drawn into from now on; past [`MAX_DEPTH`] the group is dropped
    /// (what would have been drawn inside it goes to the open group) and noted as skipped.
    fn push_group(&mut self, g: Group, kind: Kind) {
        self.text_run = false;
        if self.base.saturating_add(self.stack.len()) >= MAX_DEPTH {
            self.skip("group nesting limit");
            return;
        }
        self.stack.push((g, kind));
    }

    /// Children drawn from now on are clipped by `path` (page space) until the group closes.
    pub fn clip(&mut self, path: BezPath, rule: FillRule) {
        let mut g = group("Clip Group");
        g.clip.push((path, rule));
        // Clipping is not a transparency group: blend modes inside reach the backdrop.
        g.isolated = false;
        self.push_group(g, Kind::Clip);
    }

    /// Open an optional-content layer (or a named group).
    pub fn open(&mut self, name: &str, layer: bool) {
        self.push_group(group(name), if layer { Kind::Layer } else { Kind::Plain });
    }

    /// Open a group (blend mode, soft mask) closed with the graphics state.
    pub fn open_group(&mut self, g: Group) {
        self.push_group(g, Kind::Plain);
    }

    /// Start a bounded logical annotation independently of graphics-state nesting.
    pub fn begin_text(&mut self, text: &str) -> bool {
        self.text_run = false;
        if self.text_labels.len() >= MAX_DEPTH {
            self.skip("logical text nesting limit");
            return false;
        }
        // Every paint wrapped by the annotation carries its own copy of the label, so a long
        // /ActualText is cut to MAX_LABEL characters rather than copied whole into each.
        let text: String = text.chars().take(MAX_LABEL).collect();
        self.text_labels.push(format!("Text: {text}"));
        true
    }

    pub fn end_text(&mut self) {
        self.text_run = false;
        self.text_labels.pop();
    }

    /// Add a finished node (an image, pattern tiles).
    pub fn push(&mut self, mut n: Node) {
        // Marked content is metadata, not a transparency boundary. Wrap only the
        // painted node, without closing, splitting or reopening any live state group.
        // Finished forms may already contain groups: metadata cannot extend the cap.
        fn group_depth(n: &Node, at: usize) -> usize {
            match n {
                Node::Group(g) => {
                    if at >= MAX_DEPTH {
                        return MAX_DEPTH;
                    }
                    1 + g.children.iter().map(|n| group_depth(n, at + 1)).max().unwrap_or(0)
                }
                _ => 0,
            }
        }
        let contained = if self.text_labels.is_empty() { 0 } else { group_depth(&n, 0) };
        let room = MAX_DEPTH.saturating_sub(self.base.saturating_add(self.stack.len()).saturating_add(contained));
        if self.text_labels.len() > room {
            self.skip("group nesting limit");
        }
        // Keep consecutive glyphs of one annotation together only inside the same
        // graphics-state container. A knockout parent's children must stay separate.
        let coalesce = !self.text_labels.is_empty() && self.text_labels.len() <= room;
        if self.text_run
            && coalesce
            && let Some((parent, _)) = self.stack.last_mut()
            && !parent.knockout
        {
            let mut target = parent.children.last_mut();
            for _ in 1..self.text_labels.len() {
                target = match target {
                    Some(Node::Group(g)) => g.children.first_mut(),
                    _ => None,
                };
            }
            if let Some(Node::Group(g)) = target {
                g.children.push(n);
                return;
            }
        }
        for label in self.text_labels.iter().take(room).rev() {
            let mut g = group(label);
            g.isolated = false;
            g.children.push(n);
            n = Node::Group(g);
        }
        self.add(n, false);
        self.text_run = coalesce;
    }

    pub fn fill_stroke(&mut self, path: BezPath, transform: Affine, fill: Option<(Paint, f64, FillRule)>, stroke: Option<Stroke>) {
        self.fill_stroke_named("Path", path, transform, fill, stroke);
    }

    pub fn fill_stroke_named(&mut self, name: &str, path: BezPath, transform: Affine, fill: Option<(Paint, f64, FillRule)>, stroke: Option<Stroke>) {
        if path.elements().is_empty() || (fill.is_none() && stroke.is_none()) {
            return;
        }
        self.shapes += 1;
        let (f, fo, rule) = match fill {
            Some((p, o, r)) => (Some(p), o, r),
            None => (None, 1.0, FillRule::NonZero),
        };
        let s = Shape { name: name.into(), geom: Geom::Path(path), transform, fill: f, fill_opacity: fo, fill_rule: rule, stroke, opacity: 1.0 };
        self.push(Node::Shape(s));
    }

    /// The finished top-level nodes and their layer flags.
    pub fn finish(mut self) -> (Vec<Node>, Vec<bool>, Vec<String>) {
        self.close_to(1);
        let root = self.stack.pop().map(|(g, _)| g.children).unwrap_or_default();
        let flags = self.top_layers;
        (root, flags, self.skipped)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_long_logical_label_is_cut_once_at_begin_text() {
        let mut b = Builder::new();
        assert!(b.begin_text(&"é".repeat(MAX_LABEL * 4)));
        assert_eq!(b.text_labels[0].chars().count(), "Text: ".len() + MAX_LABEL);
        assert!(b.begin_text("short"));
        assert_eq!(b.text_labels[1], "Text: short");
    }
}

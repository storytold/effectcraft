//! The output side shared by the PDF and PostScript interpreters: an open-group stack (clipping
//! groups, optional-content layers) that collects shapes into the SVG render tree.

use effectcraft_svg::{Affine, BezPath, FillRule, Geom, Group, Node, Paint, Shape, Stroke};

/// The most elements a path being built holds: an operator that would add more stops the
/// PostScript program, or the PDF content stream.
pub(crate) const MAX_PATH: usize = 1 << 20;

/// The deepest the open groups nest (the render tree and its consumers recurse through them).
const MAX_DEPTH: usize = 256;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Plain,
    Clip,
    Layer,
}

pub(crate) struct Builder {
    stack: Vec<(Group, Kind)>,
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
        Builder { stack: vec![(group("page"), Kind::Plain)], top_layers: vec![], skipped: vec![], shapes: 0, base: 0 }
    }

    /// A builder for content that will be placed inside this one's open groups and `wrap` more
    /// groups (a pattern's `Pattern` and `Tile`, a soft mask's two), so its nesting limit counts
    /// them.
    pub fn nested(&self, wrap: usize) -> Builder {
        Builder { base: self.base.saturating_add(self.stack.len()).saturating_add(wrap), ..Builder::new() }
    }

    pub fn depth(&self) -> usize {
        self.stack.len()
    }

    /// `wrap` more groups fit under the open ones within [`MAX_DEPTH`].
    pub fn has_room(&self, wrap: usize) -> bool {
        self.base.saturating_add(self.stack.len()).saturating_add(wrap) <= MAX_DEPTH
    }

    pub fn skip(&mut self, what: &str) {
        if !self.skipped.iter().any(|s| s == what) {
            self.skipped.push(what.to_string());
        }
    }

    fn add(&mut self, n: Node, layer: bool) {
        if self.stack.len() == 1 {
            self.top_layers.push(layer);
        }
        if let Some((g, _)) = self.stack.last_mut() {
            g.children.push(n);
        }
    }

    /// Close every group opened above `depth`.
    pub fn close_to(&mut self, depth: usize) {
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

    /// Add a finished node (an image, pattern tiles).
    pub fn push(&mut self, n: Node) {
        self.add(n, false);
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
        self.add(Node::Shape(s), false);
    }

    /// The finished top-level nodes and their layer flags.
    pub fn finish(mut self) -> (Vec<Node>, Vec<bool>, Vec<String>) {
        self.close_to(1);
        let root = self.stack.pop().map(|(g, _)| g.children).unwrap_or_default();
        let flags = self.top_layers;
        (root, flags, self.skipped)
    }
}

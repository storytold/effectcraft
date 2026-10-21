//! The output side shared by the PDF and PostScript interpreters: an open-group stack (clipping
//! groups, optional-content layers) that collects shapes into the SVG render tree.

use effectcraft_svg::{Affine, BezPath, FillRule, Geom, Group, Node, Paint, Shape, Stroke};

/// The most elements a path being built holds: an operator that would add more stops the
/// PostScript program, or the PDF content stream.
pub(crate) const MAX_PATH: usize = 1 << 20;

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
}

pub(crate) fn group(name: &str) -> Group {
    Group::new(name)
}

impl Builder {
    pub fn new() -> Builder {
        Builder { stack: vec![(group("page"), Kind::Plain)], top_layers: vec![], skipped: vec![], shapes: 0 }
    }

    pub fn depth(&self) -> usize {
        self.stack.len()
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

    /// Children drawn from now on are clipped by `path` (page space) until the group closes.
    pub fn clip(&mut self, path: BezPath, rule: FillRule) {
        let mut g = group("Clip Group");
        g.clip.push((path, rule));
        // Clipping is not a transparency group: blend modes inside reach the backdrop.
        g.isolated = false;
        self.stack.push((g, Kind::Clip));
    }

    /// Open an optional-content layer (or a named group).
    pub fn open(&mut self, name: &str, layer: bool) {
        self.stack.push((group(name), if layer { Kind::Layer } else { Kind::Plain }));
    }

    /// Open a group (blend mode, soft mask) closed with the graphics state.
    pub fn open_group(&mut self, g: Group) {
        self.stack.push((g, Kind::Plain));
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

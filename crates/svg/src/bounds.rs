use kurbo::{Affine, BezPath, Rect, Shape as _};

use crate::{Doc, Group, Node};

impl Doc {
    /// Geometric artwork bounds in document pixels, including strokes and clipping.
    /// Opacity and soft masks do not change the artwork's dimensions.
    pub fn artwork_bounds(&self) -> Option<Rect> {
        let page = Rect::new(0.0, 0.0, self.width, self.height);
        group_bounds(&self.root, Affine::IDENTITY, 0, page)
    }
}

fn path_bounds(path: BezPath) -> Option<Rect> {
    if path.elements().is_empty() || !path.is_finite() {
        return None;
    }
    let r = path.bounding_box();
    (r.is_finite() && r.width() > 0.0 && r.height() > 0.0).then_some(r)
}

fn group_bounds(group: &Group, transform: Affine, depth: usize, page: Rect) -> Option<Rect> {
    if depth > 256 {
        return Some(page);
    }
    let transform = transform * group.transform;
    if !transform.as_coeffs().iter().all(|v| v.is_finite()) {
        return None;
    }
    let mut bounds: Option<Rect> = None;
    for node in &group.children {
        let rect = match node {
            Node::Group(g) => group_bounds(g, transform, depth + 1, page),
            Node::Image(i) => {
                let rect = Rect::new(0.0, 0.0, f64::from(i.width), f64::from(i.height));
                path_bounds(transform * i.transform * rect.to_path(0.01))
            }
            Node::Shape(s) => {
                let m = transform * s.transform;
                let path = s.geom.to_path();
                let mut rect = s.fill.as_ref().and_then(|_| path_bounds(m * &path));
                if let Some(stroke) = &s.stroke {
                    let scale = m.determinant().abs().sqrt();
                    if scale.is_finite()
                        && path.is_finite()
                        && let Some(outline) = effectcraft_path::stroke_outline(std::slice::from_ref(&path), &stroke.style(), scale)
                        && let Some(r) = path_bounds(m * outline)
                    {
                        rect = Some(rect.map_or(r, |b| b.union(r)));
                    }
                }
                rect
            }
        };
        if let Some(r) = rect {
            bounds = Some(bounds.map_or(r, |b| b.union(r)));
        }
    }
    let mut bounds = bounds?;
    for (clip, _) in &group.clip {
        bounds = bounds.intersect(path_bounds(transform * clip)?);
    }
    (bounds.width() > 0.0 && bounds.height() > 0.0).then_some(bounds)
}

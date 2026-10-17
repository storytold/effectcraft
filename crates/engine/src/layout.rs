//! Layer geometry in comp space: where a layer's content lands at a comp time, whether it is drawn
//! at all, how much of it is inside the comp frame and how layers overlap. This is the canonical
//! engine function behind `layer.bounds` (MCP `get_layout`); overlays and other tools that need a
//! layer's on-screen box should call [`layer_geometry`] rather than re-deriving it.
//!
//! The content rectangle is After Effects' `sourceRectAtTime()` (source size for solids, footage
//! and precomps; glyph bounds for text; painted bounds including strokes for shapes), mapped
//! through the layer's transform, its parents and, for 3D layers, the comp's active camera
//! ([`EvalCtx::layer_to_comp`]). [`pixel_bounds`] measures the drawn pixels instead (masks,
//! effects and layer styles included) by rendering the layer.

use effectcraft_geom::{Mat3, vec2};
use effectcraft_project::Layer;
use effectcraft_render::{EvalCtx, Renderer};

/// Where a layer's content rectangle lands in comp space.
#[derive(Clone, Debug, PartialEq)]
pub struct LayerGeometry {
    /// Layer-space content rectangle `[left, top, width, height]` (`sourceRectAtTime`).
    pub source: [f64; 4],
    /// Its corners in comp pixels: top left, top right, bottom right, bottom left.
    pub corners: [[f64; 2]; 4],
    /// Axis-aligned comp-space bounds `[x, y, w, h]` of the corners.
    pub bounds: [f64; 4],
    /// The content rectangle's centre in comp pixels.
    pub center: [f64; 2],
    /// False when part of a 3D layer is behind the camera: the corners can't be projected and
    /// the bounds are not meaningful.
    pub projected: bool,
}

/// [`LayerGeometry`] of `layer` at the context time; `None` for layers without content
/// (cameras, lights, empty text or shape layers).
pub fn layer_geometry(ctx: &EvalCtx, layer: &Layer) -> Option<LayerGeometry> {
    let b = effectcraft_render::content_bounds(ctx, layer)?;
    let (m, _) = ctx.layer_to_comp(layer);
    let mut projected = true;
    let corners = [[b[0], b[1]], [b[2], b[1]], [b[2], b[3]], [b[0], b[3]]].map(|p| {
        let (q, ok) = to_comp(&m, p);
        projected &= ok;
        q
    });
    let (center, ok) = to_comp(&m, [(b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0]);
    projected &= ok;
    Some(LayerGeometry { source: [b[0], b[1], b[2] - b[0], b[3] - b[1]], corners, bounds: aabb(&corners), center, projected })
}

/// A layer-space point in comp pixels through `m` (layer → comp, projective for 3D layers), and
/// whether it is in front of the camera.
pub fn to_comp(m: &Mat3, p: [f64; 2]) -> ([f64; 2], bool) {
    let r = &m.0;
    let w = r[2][0] * p[0] + r[2][1] * p[1] + r[2][2];
    let q = m.apply(vec2(p[0], p[1]));
    ([q.x, q.y], w > 1e-9 && q.x.is_finite() && q.y.is_finite())
}

/// The layer's anchor point in comp pixels (where Position puts it, after parenting and the
/// camera), or `None` for layers without a transform and points behind the camera.
pub fn anchor_in_comp(ctx: &EvalCtx, layer: &Layer) -> Option<[f64; 2]> {
    let tr = layer.transform()?;
    if layer.is_camera() || layer.is_light() {
        return None;
    }
    let a = ctx.v3(layer, tr, "anchor", [0.0; 3]);
    let (q, ok) = to_comp(&ctx.layer_to_comp(layer).0, [a[0], a[1]]);
    ok.then_some(q)
}

/// Whether the compositor draws `layer` at the context time, and if not, why (every reason that
/// applies). Opacity 0 counts as not visible.
pub fn visibility(ctx: &EvalCtx, layer: &Layer) -> (bool, Vec<&'static str>) {
    let t = ctx.time;
    let mut why = vec![];
    if t < layer.in_point {
        why.push("before its in point");
    } else if t >= layer.out_point {
        why.push("at or after its out point");
    }
    if !layer.source.is_av() {
        why.push("cameras, lights and nulls are not drawn");
    } else if !layer.switches.video {
        why.push("Video switch is off");
    }
    let any_solo = ctx.comp.layers.iter().any(|l| l.switches.solo && l.source.is_av() && l.is_active_at(t));
    if any_solo && !layer.switches.solo {
        why.push("another layer is soloed");
    }
    if layer.switches.guide {
        why.push("guide layer (not rendered)");
    }
    if layer.environment {
        why.push("environment layer (lights the scene, not drawn)");
    }
    if layer.source.is_av() && ctx.opacity(layer) <= 0.0 {
        why.push("opacity is 0");
    }
    (why.is_empty(), why)
}

/// Layer-space pixels actually drawn (alpha > 0 after masks, effects and layer styles), as
/// comp-space bounds `[x, y, w, h]`; `None` when nothing is drawn. Renders the layer.
pub fn pixel_bounds(r: &Renderer, ctx: &EvalCtx, layer: &Layer) -> Option<[f64; 4]> {
    let buf = r.layer_buf(ctx, layer)?;
    let (w, h) = (buf.img.width as usize, buf.img.height as usize);
    let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0usize, 0usize);
    for (i, p) in buf.img.data.iter().enumerate() {
        if p[3] > 1.0 / 512.0 {
            let (x, y) = (i % w.max(1), i / w.max(1));
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x + 1);
            y1 = y1.max(y + 1);
        }
    }
    if x0 > x1 || y0 > y1 || w == 0 || h == 0 || buf.scale <= 0.0 {
        return None;
    }
    let (m, _) = ctx.layer_to_comp(layer);
    let px = |x: usize, y: usize| [(x as f64 - buf.offset[0]) / buf.scale, (y as f64 - buf.offset[1]) / buf.scale];
    let corners = [px(x0, y0), px(x1, y0), px(x1, y1), px(x0, y1)].map(|p| to_comp(&m, p).0);
    Some(aabb(&corners))
}

/// Axis-aligned bounds `[x, y, w, h]` of points.
pub fn aabb(pts: &[[f64; 2]]) -> [f64; 4] {
    let (mut x0, mut y0, mut x1, mut y1) = (f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
    for p in pts {
        x0 = x0.min(p[0]);
        y0 = y0.min(p[1]);
        x1 = x1.max(p[0]);
        y1 = y1.max(p[1]);
    }
    if x0 > x1 || y0 > y1 { [0.0; 4] } else { [x0, y0, x1 - x0, y1 - y0] }
}

/// Area of a simple polygon (absolute shoelace).
pub fn polygon_area(poly: &[[f64; 2]]) -> f64 {
    signed_area(poly).abs()
}

fn signed_area(poly: &[[f64; 2]]) -> f64 {
    let n = poly.len();
    if n < 3 {
        return 0.0;
    }
    poly.iter().zip(poly.iter().cycle().skip(1)).take(n).map(|(a, b)| a[0] * b[1] - b[0] * a[1]).sum::<f64>() / 2.0
}

fn cross(o: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])
}

/// `subject` clipped to the convex polygon `clip` (Sutherland–Hodgman); either winding.
pub fn clip_convex(subject: &[[f64; 2]], clip: &[[f64; 2]]) -> Vec<[f64; 2]> {
    let orient = signed_area(clip).signum();
    if orient == 0.0 || subject.len() < 3 {
        return vec![];
    }
    let mut out = subject.to_vec();
    for (&a, &b) in clip.iter().zip(clip.iter().cycle().skip(1)).take(clip.len()) {
        let input = std::mem::take(&mut out);
        let Some(&last) = input.last() else { break };
        let inside = |p: [f64; 2]| orient * cross(a, b, p) >= 0.0;
        let hit = |p: [f64; 2], q: [f64; 2]| {
            let (d, e) = ([q[0] - p[0], q[1] - p[1]], [b[0] - a[0], b[1] - a[1]]);
            let den = e[0] * d[1] - e[1] * d[0];
            if den.abs() < 1e-12 {
                return q;
            }
            let t = (e[0] * (a[1] - p[1]) - e[1] * (a[0] - p[0])) / den;
            [p[0] + t * d[0], p[1] + t * d[1]]
        };
        let mut prev = last;
        for &cur in &input {
            match (inside(prev), inside(cur)) {
                (true, true) => out.push(cur),
                (true, false) => out.push(hit(prev, cur)),
                (false, true) => {
                    out.push(hit(prev, cur));
                    out.push(cur);
                }
                (false, false) => {}
            }
            prev = cur;
        }
    }
    out
}

/// Fraction (0–1) of the quad `corners` inside the `w`×`h` comp frame.
pub fn on_screen_fraction(corners: &[[f64; 2]; 4], w: f64, h: f64) -> f64 {
    let area = polygon_area(corners);
    if area <= 1e-9 {
        return 0.0;
    }
    let frame = [[0.0, 0.0], [w, 0.0], [w, h], [0.0, h]];
    (polygon_area(&clip_convex(corners, &frame)) / area).clamp(0.0, 1.0)
}

/// Area of the intersection of two convex quads.
pub fn overlap_area(a: &[[f64; 2]; 4], b: &[[f64; 2]; 4]) -> f64 {
    polygon_area(&clip_convex(a, b))
}

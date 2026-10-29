//! Bezier shapes for shape layers, masks and text.
//!
//! Geometry is kept in [`kurbo::BezPath`]; anti-aliased coverage comes from tiny-skia's scanline
//! rasteriser (BSD-3) and is converted to our `f32` [`Mask`]. Path operations (trim, round corners,
//! pucker & bloat, zig zag, twist, wiggle, offset, merge) live in [`ops`]; curve-preserving booleans
//! (for Merge Paths) in [`boolean`] and curve offsetting in [`offset`].

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod boolean;
pub mod feather;
mod fit;
pub mod interp;
pub mod offset;
pub mod ops;
pub mod trace;
pub mod varstroke;

use effectcraft_geom::Mat3;
use effectcraft_keyframe::ShapePath;
use effectcraft_raster::Mask;
pub use kurbo::{BezPath, PathEl, Point};
use kurbo::{Shape, Vec2 as KVec2};

/// Convert the AE vertex/tangent model to a kurbo path.
pub fn to_kurbo(p: &ShapePath) -> BezPath {
    let mut out = BezPath::new();
    let n = p.vertices.len();
    if n == 0 {
        return out;
    }
    let v = |i: usize| Point::new(p.vertices[i][0], p.vertices[i][1]);
    let ot = |i: usize| p.out_tangents.get(i).copied().unwrap_or([0.0; 2]);
    let it = |i: usize| p.in_tangents.get(i).copied().unwrap_or([0.0; 2]);
    out.move_to(v(0));
    let seg = |out: &mut BezPath, a: usize, b: usize| {
        let (o, i) = (ot(a), it(b));
        if o == [0.0; 2] && i == [0.0; 2] {
            out.line_to(v(b));
        } else {
            out.curve_to(v(a) + KVec2::new(o[0], o[1]), v(b) + KVec2::new(i[0], i[1]), v(b));
        }
    };
    for i in 0..n - 1 {
        seg(&mut out, i, i + 1);
    }
    if p.closed && n > 1 {
        seg(&mut out, n - 1, 0);
        out.close_path();
    }
    out
}

/// Convert a kurbo path (first subpath) back to the vertex model (quads are elevated).
pub fn from_kurbo(path: &BezPath) -> Vec<ShapePath> {
    let mut out = Vec::new();
    let mut cur: Option<ShapePath> = None;
    let mut last = Point::ZERO;
    let flush = |cur: &mut Option<ShapePath>, out: &mut Vec<ShapePath>| {
        if let Some(mut c) = cur.take() {
            // A closing segment ending on the first vertex duplicates it.
            if c.closed && c.vertices.len() > 1 {
                let f = c.vertices[0];
                let l = *c.vertices.last().unwrap_or(&f);
                if (f[0] - l[0]).abs() < 1e-9 && (f[1] - l[1]).abs() < 1e-9 {
                    let it = c.in_tangents.pop().unwrap_or([0.0; 2]);
                    c.vertices.pop();
                    c.out_tangents.pop();
                    c.in_tangents[0] = it;
                }
            }
            if !c.vertices.is_empty() {
                out.push(c);
            }
        }
    };
    for el in path.elements() {
        match *el {
            PathEl::MoveTo(p) => {
                flush(&mut cur, &mut out);
                cur = Some(ShapePath {
                    vertices: vec![[p.x, p.y]],
                    in_tangents: vec![[0.0; 2]],
                    out_tangents: vec![[0.0; 2]],
                    closed: false,
                    feather: Vec::new(),
                });
                last = p;
            }
            PathEl::LineTo(p) => {
                if let Some(c) = cur.as_mut() {
                    c.vertices.push([p.x, p.y]);
                    c.in_tangents.push([0.0; 2]);
                    c.out_tangents.push([0.0; 2]);
                }
                last = p;
            }
            PathEl::QuadTo(q, p) => {
                let c1 = last + (q - last) * (2.0 / 3.0);
                let c2 = p + (q - p) * (2.0 / 3.0);
                if let Some(c) = cur.as_mut() {
                    if let Some(o) = c.out_tangents.last_mut() {
                        *o = [c1.x - last.x, c1.y - last.y];
                    }
                    c.vertices.push([p.x, p.y]);
                    c.in_tangents.push([c2.x - p.x, c2.y - p.y]);
                    c.out_tangents.push([0.0; 2]);
                }
                last = p;
            }
            PathEl::CurveTo(c1, c2, p) => {
                if let Some(c) = cur.as_mut() {
                    if let Some(o) = c.out_tangents.last_mut() {
                        *o = [c1.x - last.x, c1.y - last.y];
                    }
                    c.vertices.push([p.x, p.y]);
                    c.in_tangents.push([c2.x - p.x, c2.y - p.y]);
                    c.out_tangents.push([0.0; 2]);
                }
                last = p;
            }
            PathEl::ClosePath => {
                if let Some(c) = cur.as_mut() {
                    c.closed = true;
                }
            }
        }
    }
    flush(&mut cur, &mut out);
    out
}

/// Rectangle path centred at `pos` with corner `roundness`, drawn as After Effects draws it
/// (Trim Paths measures from its first point): clockwise from the top-right corner, or with
/// round corners from the end of the top-right arc on the right edge (After Effects 2026,
/// 400 × 300 rectangles trimmed 0–25 %, case 930).
pub fn rect(size: [f64; 2], pos: [f64; 2], roundness: f64) -> BezPath {
    let (w, h) = (size[0].abs(), size[1].abs());
    let (x0, y0, x1, y1) = (pos[0] - w / 2.0, pos[1] - h / 2.0, pos[0] + w / 2.0, pos[1] + h / 2.0);
    let max_rad = w.min(h) / 2.0;
    // `f64::clamp` panics on a NaN bound (both size components NaN): fall back to no roundness.
    let rad = if roundness.is_finite() && !max_rad.is_nan() { roundness.clamp(0.0, max_rad) } else { 0.0 };
    let mut p = BezPath::new();
    if rad <= 0.0 {
        p.move_to((x1, y0));
        p.line_to((x1, y1));
        p.line_to((x0, y1));
        p.line_to((x0, y0));
        p.close_path();
        return p;
    }
    // Quarter circles as cubics (handle 0.5523 × radius).
    let k = rad * 0.552_284_749_8;
    p.move_to((x1, y0 + rad));
    p.line_to((x1, y1 - rad));
    p.curve_to((x1, y1 - rad + k), (x1 - rad + k, y1), (x1 - rad, y1));
    p.line_to((x0 + rad, y1));
    p.curve_to((x0 + rad - k, y1), (x0, y1 - rad + k), (x0, y1 - rad));
    p.line_to((x0, y0 + rad));
    p.curve_to((x0, y0 + rad - k), (x0 + rad - k, y0), (x0 + rad, y0));
    p.line_to((x1 - rad, y0));
    p.curve_to((x1 - rad + k, y0), (x1, y0 + rad - k), (x1, y0 + rad));
    p.close_path();
    p
}

pub fn ellipse(size: [f64; 2], pos: [f64; 2]) -> BezPath {
    to_kurbo(&ShapePath::ellipse(pos, size[0], size[1]))
}

/// Polystar: a star (`inner` radius used) or polygon, rotation in degrees, roundness in percent.
pub struct Polystar {
    pub star: bool,
    pub points: f64,
    pub pos: [f64; 2],
    pub rotation: f64,
    pub inner: f64,
    pub outer: f64,
    pub inner_round: f64,
    pub outer_round: f64,
}

/// Build the path described by the polystar's geometry.
pub fn polystar(spec: Polystar) -> BezPath {
    let Polystar { star, points, pos, rotation, inner, outer, inner_round, outer_round } = spec;
    let n = points.max(3.0).round() as usize;
    let verts = if star { n * 2 } else { n };
    let mut path = BezPath::new();
    let step = std::f64::consts::TAU / verts as f64;
    let start = (rotation - 90.0).to_radians();
    let mut pts = Vec::with_capacity(verts);
    for i in 0..verts {
        let r = if star && i % 2 == 1 { inner } else { outer };
        let a = start + step * i as f64;
        pts.push((Point::new(pos[0] + r * a.cos(), pos[1] + r * a.sin()), a, r, if star && i % 2 == 1 { inner_round } else { outer_round }));
    }
    let tangent = |(p, a, r, round): (Point, f64, f64, f64), sign: f64| -> Point {
        if round == 0.0 {
            return p;
        }
        // Tangent perpendicular to the radius, length proportional to the arc between vertices.
        let len = r * step * 0.25 * round / 100.0 * 4.0 / 3.0 * 0.75;
        let d = KVec2::new(-a.sin(), a.cos()) * len * sign;
        p + d
    };
    path.move_to(pts[0].0);
    for i in 0..verts {
        let a = pts[i];
        let b = pts[(i + 1) % verts];
        if a.3 == 0.0 && b.3 == 0.0 {
            path.line_to(b.0);
        } else {
            path.curve_to(tangent(a, 1.0), tangent(b, -1.0), b.0);
        }
    }
    path.close_path();
    path
}

fn to_skia(path: &BezPath) -> Option<tiny_skia::Path> {
    let mut pb = tiny_skia::PathBuilder::new();
    for el in path.elements() {
        match *el {
            PathEl::MoveTo(p) => pb.move_to(p.x as f32, p.y as f32),
            PathEl::LineTo(p) => pb.line_to(p.x as f32, p.y as f32),
            PathEl::QuadTo(a, p) => pb.quad_to(a.x as f32, a.y as f32, p.x as f32, p.y as f32),
            PathEl::CurveTo(a, b, p) => pb.cubic_to(a.x as f32, a.y as f32, b.x as f32, b.y as f32, p.x as f32, p.y as f32),
            PathEl::ClosePath => pb.close(),
        }
    }
    pb.finish()
}

fn from_skia(p: &tiny_skia::Path) -> BezPath {
    let mut out = BezPath::new();
    let pt = |p: tiny_skia::Point| Point::new(p.x as f64, p.y as f64);
    for seg in p.segments() {
        match seg {
            tiny_skia::PathSegment::MoveTo(a) => out.move_to(pt(a)),
            tiny_skia::PathSegment::LineTo(a) => out.line_to(pt(a)),
            tiny_skia::PathSegment::QuadTo(a, b) => out.quad_to(pt(a), pt(b)),
            tiny_skia::PathSegment::CubicTo(a, b, c) => out.curve_to(pt(a), pt(b), pt(c)),
            tiny_skia::PathSegment::Close => out.close_path(),
        }
    }
    out
}

fn skia_transform(m: &Mat3) -> tiny_skia::Transform {
    let a = &m.0;
    tiny_skia::Transform::from_row(a[0][0] as f32, a[1][0] as f32, a[0][1] as f32, a[1][1] as f32, a[0][2] as f32, a[1][2] as f32)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FillRule {
    #[default]
    NonZero,
    EvenOdd,
}

/// Anti-aliased coverage of `paths` (in local space) transformed by `m` into a `w × h` mask.
pub fn fill_coverage(paths: &[BezPath], m: &Mat3, w: u32, h: u32, rule: FillRule) -> Mask {
    let mut out = Mask::new(w, h, 0.0);
    let Some(mut mask) = tiny_skia::Mask::new(w.max(1), h.max(1)) else { return out };
    let mut all = BezPath::new();
    for p in paths {
        all.extend(p.elements().iter().copied());
    }
    let Some(sp) = to_skia(&all) else { return out };
    let fr = match rule {
        FillRule::NonZero => tiny_skia::FillRule::Winding,
        FillRule::EvenOdd => tiny_skia::FillRule::EvenOdd,
    };
    mask.fill_path(&sp, fr, true, skia_transform(m));
    for (o, &v) in out.data.iter_mut().zip(mask.data()) {
        *o = v as f32 / 255.0;
    }
    out
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Cap {
    #[default]
    Butt,
    Round,
    Square,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Join {
    #[default]
    Miter,
    Round,
    Bevel,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StrokeStyle {
    pub width: f64,
    pub cap: Cap,
    pub join: Join,
    pub miter: f64,
    /// Dash pattern (dash, gap, …) and offset.
    pub dash: Option<(Vec<f64>, f64)>,
    /// Stroke Taper (variable width at the ends).
    pub taper: Option<varstroke::Taper>,
    /// Stroke Wave (periodic width).
    pub wave: Option<varstroke::Wave>,
}

impl StrokeStyle {
    /// Whether the width varies along the path (Taper or Wave active).
    pub fn is_variable(&self) -> bool {
        self.taper.as_ref().is_some_and(|t| t.is_active()) || self.wave.as_ref().is_some_and(|w| w.is_active())
    }
}

impl Default for StrokeStyle {
    fn default() -> Self {
        StrokeStyle { width: 1.0, cap: Cap::Butt, join: Join::Miter, miter: 4.0, dash: None, taper: None, wave: None }
    }
}

/// Outline of a stroke as a fillable path (local space). `res_scale` = device/local scale.
pub fn stroke_outline(paths: &[BezPath], style: &StrokeStyle, res_scale: f64) -> Option<BezPath> {
    if style.width <= 0.0 {
        return None;
    }
    if style.is_variable() {
        return varstroke::outline(paths, style, res_scale);
    }
    let mut all = BezPath::new();
    for p in paths {
        all.extend(p.elements().iter().copied());
    }
    let mut sp = to_skia(&all)?;
    if let Some((d, off)) = &style.dash {
        let arr: Vec<f32> = d.iter().map(|v| v.max(0.0) as f32).collect();
        if arr.len() >= 2 && arr.iter().any(|v| *v > 0.0) {
            let arr = if arr.len() % 2 == 1 { [arr.clone(), arr].concat() } else { arr };
            if let Some(dash) = tiny_skia::StrokeDash::new(arr, *off as f32) {
                sp = sp.dash(&dash, res_scale as f32)?;
            }
        }
    }
    let st = tiny_skia::Stroke {
        width: style.width as f32,
        miter_limit: style.miter.max(1.0) as f32,
        line_cap: match style.cap {
            Cap::Butt => tiny_skia::LineCap::Butt,
            Cap::Round => tiny_skia::LineCap::Round,
            Cap::Square => tiny_skia::LineCap::Square,
        },
        line_join: match style.join {
            Join::Miter => tiny_skia::LineJoin::Miter,
            Join::Round => tiny_skia::LineJoin::Round,
            Join::Bevel => tiny_skia::LineJoin::Bevel,
        },
        dash: None,
    };
    let out = sp.stroke(&st, res_scale.max(0.01) as f32)?;
    Some(from_skia(&out))
}

/// Coverage of a stroke.
pub fn stroke_coverage(paths: &[BezPath], style: &StrokeStyle, m: &Mat3, w: u32, h: u32) -> Mask {
    match stroke_outline(paths, style, m.mean_scale()) {
        Some(o) => fill_coverage(&[o], m, w, h, FillRule::NonZero),
        None => Mask::new(w, h, 0.0),
    }
}

/// Bounding box of paths in local space.
pub fn bounds(paths: &[BezPath]) -> Option<kurbo::Rect> {
    let mut r: Option<kurbo::Rect> = None;
    for p in paths {
        if p.elements().is_empty() {
            continue;
        }
        let b = p.bounding_box();
        r = Some(r.map_or(b, |a| a.union(b)));
    }
    r
}

/// Transform paths by an affine Mat3.
pub fn transform(paths: &[BezPath], m: &Mat3) -> Vec<BezPath> {
    let a = &m.0;
    let k = kurbo::Affine::new([a[0][0], a[1][0], a[0][1], a[1][1], a[0][2], a[1][2]]);
    paths.iter().map(|p| k * p.clone()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// After Effects 2026 (case 930): 400 × 300 rectangles at the origin with Trim Paths. Plain:
    /// 0–25 % runs from the top-right corner down the right edge and 50 px along the bottom.
    /// Round corners 40: from the end of the top-right arc (200, −110) down, round the
    /// bottom-right corner, to x 110 on the bottom. Start 70 / End 30 draws 30–70 %.
    #[test]
    fn rectangles_start_where_after_effects_starts_them() {
        let ends = |p: &BezPath| {
            let mut first = None;
            let mut last = None;
            for el in p.elements() {
                match *el {
                    PathEl::MoveTo(q) if first.is_none() => first = Some(q),
                    PathEl::MoveTo(q) | PathEl::LineTo(q) | PathEl::CurveTo(_, _, q) | PathEl::QuadTo(_, q) => last = Some(q),
                    PathEl::ClosePath => {}
                }
            }
            (first.unwrap_or_default(), last.unwrap_or_default())
        };
        let near = |a: Point, b: (f64, f64)| (a.x - b.0).abs() < 1.0 && (a.y - b.1).abs() < 1.0;
        let plain = ops::trim(&[rect([400.0, 300.0], [0.0, 0.0], 0.0)], 0.0, 25.0, 0.0);
        let (a, b) = ends(&plain[0]);
        assert!(near(a, (200.0, -150.0)) && near(b, (150.0, 150.0)), "{a:?} {b:?}");
        let round = ops::trim(&[rect([400.0, 300.0], [0.0, 0.0], 40.0)], 0.0, 25.0, 0.0);
        let (a, b) = ends(&round[0]);
        assert!(near(a, (200.0, -110.0)) && near(b, (110.0, 150.0)), "{a:?} {b:?}");
        // Start > End: AE draws 30..70 % (plain: from x 80 on the bottom up to y −130 on the left).
        let swapped = ops::trim(&[rect([400.0, 300.0], [0.0, 0.0], 0.0)], 70.0, 30.0, 0.0);
        let (a, b) = ends(&swapped[0]);
        assert!(near(a, (80.0, 150.0)) && near(b, (-200.0, -130.0)), "{a:?} {b:?}");
    }

    #[test]
    fn rect_with_nan_size_does_not_panic() {
        let _ = rect([f64::NAN, f64::NAN], [0.0, 0.0], 5.0);
    }

    #[test]
    fn roundtrip_vertex_model() {
        let sp = ShapePath::ellipse([10.0, 20.0], 40.0, 30.0);
        let k = to_kurbo(&sp);
        let back = from_kurbo(&k);
        assert_eq!(back.len(), 1);
        let b = &back[0];
        assert!(b.closed);
        assert_eq!(b.vertices.len(), 4);
        for i in 0..4 {
            for c in 0..2 {
                assert!((b.vertices[i][c] - sp.vertices[i][c]).abs() < 1e-9);
                assert!((b.in_tangents[i][c] - sp.in_tangents[i][c]).abs() < 1e-9, "{i} {b:?}");
                assert!((b.out_tangents[i][c] - sp.out_tangents[i][c]).abs() < 1e-9);
            }
        }
    }

    #[test]
    fn rect_coverage_area() {
        let r = rect([20.0, 10.0], [50.0, 50.0], 0.0);
        let m = fill_coverage(&[r], &Mat3::IDENTITY, 100, 100, FillRule::NonZero);
        let area: f32 = m.data.iter().sum();
        assert!((area - 200.0).abs() < 1.0, "{area}");
    }

    #[test]
    fn stroke_has_area() {
        let r = rect([40.0, 40.0], [50.0, 50.0], 0.0);
        let st = StrokeStyle { width: 4.0, ..Default::default() };
        let m = stroke_coverage(&[r], &st, &Mat3::IDENTITY, 100, 100);
        let area: f32 = m.data.iter().sum();
        assert!((area - 4.0 * 160.0).abs() < 40.0, "{area}");
    }

    #[test]
    fn star_has_right_vertex_count() {
        let p = polystar(Polystar { star: true, points: 5.0, pos: [0.0, 0.0], rotation: 0.0, inner: 20.0, outer: 50.0, inner_round: 0.0, outer_round: 0.0 });
        let v = from_kurbo(&p);
        assert_eq!(v[0].vertices.len(), 10);
    }
}

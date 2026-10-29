//! Shape-layer contents: groups, parametric paths, fills, strokes, gradients and path operators,
//! with the After Effects stacking semantics:
//!
//! - path operators (Trim Paths, Round Corners, Offset Paths, Pucker & Bloat, Twist, Zig Zag,
//!   Wiggle Paths, Merge Paths, Repeater) act on all paths above them in the same group, including
//!   the paths of nested groups;
//! - paint items (fills, strokes, gradient fills/strokes) paint the paths above them, *as modified
//!   by every operator in the stack* — a Trim Paths below a Stroke still trims it;
//! - items higher in the list draw on top;
//! - surface items (Front, Bevel, Side, Back) colour those surfaces of the Advanced 3D
//!   extrusion of every paint in their group and its nested groups (the innermost wins).

use effectcraft_color::{BlendMode, blend_pixel};
use effectcraft_effects::Buf;
use effectcraft_geom::{Mat3, vec2};
use effectcraft_keyframe::{Gradient, Value};
use effectcraft_path::{BezPath, Cap, FillRule, Join, StrokeStyle, ops, varstroke};
use effectcraft_project::{Layer, Node, PropGroup};
use effectcraft_raster::{Image, Mask};
use rayon::prelude::*;

use crate::eval::EvalCtx;

#[derive(Clone, Debug)]
enum Paint {
    Fill {
        color: [f32; 4],
        rule: FillRule,
    },
    Stroke {
        color: [f32; 4],
        style: StrokeStyle,
    },
    /// Gradient fill, or gradient stroke when `stroke` is set.
    Gradient {
        g: Gradient,
        radial: bool,
        start: [f64; 2],
        end: [f64; 2],
        rule: FillRule,
        stroke: Option<StrokeStyle>,
        /// Radial gradients: Highlight Length (−100…100 %) and Highlight Angle (degrees).
        highlight: (f64, f64),
    },
}

/// A paint item. It references path *slots* rather than copies of the geometry: path operators
/// further down the stack rewrite the slots in place, so (as in After Effects) a Trim Paths or
/// Round Corners placed below a fill or stroke still changes what that paint draws.
#[derive(Clone, Debug)]
struct Draw {
    paint: Paint,
    slots: Vec<usize>,
    /// Paint-group space → layer space, for gradients and stroke scaling.
    xf: Mat3,
    opacity: f32,
    blend: BlendMode,
    /// Front / Bevel / Side / Back colours of the paint's extrusion (Advanced 3D).
    surfaces: SurfaceColors,
    /// Paint item of the contents group this draw was collected in (as opposed to one bubbled up
    /// from a nested group). Merge Paths drops only these paints above it (#1174); a group's own
    /// fill or stroke keeps rendering inside the group.
    local: bool,
}

/// Straight RGB colours of an extrusion's surfaces, by [`effectcraft_model::extrude::Surface`]
/// index (Front, Bevel, Side, Back); `None` keeps the paint's colour.
pub type SurfaceColors = [Option<[f32; 3]>; 4];

fn mat_path(path: &BezPath, m: &Mat3) -> BezPath {
    effectcraft_path::transform(std::slice::from_ref(path), m).pop().unwrap_or_default()
}

fn group_matrix(ctx: &EvalCtx, layer: &Layer, tr: &PropGroup) -> (Mat3, f32) {
    let anchor = ctx.v2(layer, tr, "anchor", [0.0; 2]);
    let pos = ctx.v2(layer, tr, "position", [0.0; 2]);
    let scale = ctx.v2(layer, tr, "scale", [100.0; 2]);
    let rot = ctx.f(layer, tr, "rotation", 0.0);
    let skew = ctx.f(layer, tr, "skew", 0.0);
    let skew_axis = ctx.f(layer, tr, "skewAxis", 0.0);
    let m = Mat3::translate(vec2(pos[0], pos[1]))
        * Mat3::rotate_deg(rot)
        * Mat3::skew_deg(-skew, skew_axis)
        * Mat3::scale(vec2(scale[0] / 100.0, scale[1] / 100.0))
        * Mat3::translate(vec2(-anchor[0], -anchor[1]));
    (m, (ctx.f(layer, tr, "opacity", 100.0) / 100.0) as f32)
}

/// Shape item (a Path, Rectangle… inside nested Shape groups) → layer space: the product of the
/// enclosing groups' transforms. `None` when `uid` is not in the layer's contents.
pub fn item_matrix(ctx: &EvalCtx, layer: &Layer, uid: u64) -> Option<Mat3> {
    fn walk(ctx: &EvalCtx, layer: &Layer, g: &PropGroup, uid: u64, m: Mat3) -> Option<Mat3> {
        for sub in g.groups() {
            if sub.uid == uid {
                return Some(m);
            }
            if sub.match_id == "group"
                && let Some(inner) = sub.sub("contents")
            {
                let gm = sub.sub("transform").map(|t| group_matrix(ctx, layer, t).0).unwrap_or(Mat3::IDENTITY);
                if let Some(r) = walk(ctx, layer, inner, uid, m * gm) {
                    return Some(r);
                }
            }
        }
        None
    }
    walk(ctx, layer, layer.props.sub("contents")?, uid, Mat3::IDENTITY)
}

fn rule(ctx: &EvalCtx, layer: &Layer, g: &PropGroup) -> FillRule {
    if ctx.e(layer, g, "rule") == 1 { FillRule::EvenOdd } else { FillRule::NonZero }
}

fn stroke_style(ctx: &EvalCtx, layer: &Layer, g: &PropGroup) -> StrokeStyle {
    let dash = g.sub("dashes").and_then(|d| {
        let dash = ctx.f(layer, d, "dash", 0.0);
        let gap = ctx.f(layer, d, "gap", 0.0);
        if dash <= 0.0 {
            return None;
        }
        let mut pat = vec![dash, if gap > 0.0 { gap } else { dash }];
        // Dash 2 / Gap 2, Dash 3 / Gap 3 (added with the Dashes "+" button).
        for (dm, gm) in effectcraft_project::build::EXTRA_DASHES {
            if d.get(dm).is_none() {
                break;
            }
            let dv = ctx.f(layer, d, dm, 0.0);
            pat.push(dv);
            pat.push(if d.get(gm).is_some() { ctx.f(layer, d, gm, 0.0) } else { dv });
        }
        Some((pat, ctx.f(layer, d, "offset", 0.0)))
    });
    let taper = g.sub("taper").map(|t| varstroke::Taper {
        percent: ctx.e(layer, t, "units") == 1,
        start_len: ctx.f(layer, t, "startLength", 0.0) / if ctx.e(layer, t, "units") == 1 { 100.0 } else { 1.0 },
        end_len: ctx.f(layer, t, "endLength", 0.0) / if ctx.e(layer, t, "units") == 1 { 100.0 } else { 1.0 },
        start_width: ctx.f(layer, t, "startWidth", 100.0) / 100.0,
        end_width: ctx.f(layer, t, "endWidth", 100.0) / 100.0,
        start_ease: ctx.f(layer, t, "startEase", 0.0) / 100.0,
        end_ease: ctx.f(layer, t, "endEase", 0.0) / 100.0,
    });
    let wave = g.sub("wave").map(|w| varstroke::Wave {
        amount: ctx.f(layer, w, "amount", 0.0) / 100.0,
        cycles: (ctx.e(layer, w, "units") == 1).then(|| ctx.f(layer, w, "cycles", 10.0)),
        wavelength: ctx.f(layer, w, "wavelength", 20.0),
        phase: ctx.f(layer, w, "phase", 0.0),
    });
    StrokeStyle {
        width: ctx.f(layer, g, "width", 2.0),
        cap: [Cap::Butt, Cap::Round, Cap::Square][ctx.e(layer, g, "cap").min(2) as usize],
        join: [Join::Miter, Join::Round, Join::Bevel][ctx.e(layer, g, "join").min(2) as usize],
        miter: ctx.f(layer, g, "miter", 4.0),
        dash,
        taper: taper.filter(|t| t.is_active()),
        wave: wave.filter(|w| w.is_active()),
    }
}

/// Blend mode of a paint item (`blend` holds an index into the Blending Mode menu).
fn blend_mode(ctx: &EvalCtx, layer: &Layer, g: &PropGroup) -> BlendMode {
    BlendMode::ALL.get(ctx.e(layer, g, "blend") as usize).copied().unwrap_or_default()
}

/// Apply a path operator to the live slots in place.
fn apply(arena: &mut [BezPath], live: &[usize], f: impl FnOnce(&[BezPath]) -> Vec<BezPath>) {
    let input: Vec<BezPath> = live.iter().map(|&i| arena[i].clone()).collect();
    for (&i, p) in live.iter().zip(f(&input)) {
        arena[i] = p;
    }
}

/// Push a shape's path as a new slot, honouring its Path Direction toggle (1 = reversed).
fn push_shape(ctx: &EvalCtx, layer: &Layer, g: &PropGroup, p: BezPath, arena: &mut Vec<BezPath>, live: &mut Vec<usize>) {
    let p = if ctx.e(layer, g, "direction") == 1 { p.reverse_subpaths() } else { p };
    arena.push(p);
    live.push(arena.len() - 1);
}

/// Every slot used by `live` or by `draws`, sorted and unique.
fn used_slots(live: &[usize], draws: &[Draw]) -> Vec<usize> {
    let mut v: Vec<usize> = live.iter().copied().chain(draws.iter().flat_map(|d| d.slots.iter().copied())).collect();
    v.sort_unstable();
    v.dedup();
    v
}

/// Collect the draws of a contents group and the path slots live at its end (in group space).
/// `arena` holds every path slot; operators rewrite slots in place.
fn collect(ctx: &EvalCtx, layer: &Layer, contents: &PropGroup, arena: &mut Vec<BezPath>) -> (Vec<Draw>, Vec<usize>) {
    let mut draws: Vec<Draw> = Vec::new();
    let mut live: Vec<usize> = Vec::new();
    let opacity = |g: &PropGroup| (ctx.f(layer, g, "opacity", 100.0) / 100.0) as f32;
    let mut surfaces: SurfaceColors = [None; 4];
    for node in &contents.children {
        let Node::Group(g) = node else { continue };
        if !g.enabled {
            continue;
        }
        match g.match_id.as_str() {
            "rect" => {
                let size = ctx.v2(layer, g, "size", [100.0; 2]);
                let pos = ctx.v2(layer, g, "position", [0.0; 2]);
                let p = effectcraft_path::rect(size, pos, ctx.f(layer, g, "roundness", 0.0));
                push_shape(ctx, layer, g, p, arena, &mut live);
            }
            "ellipse" => {
                let size = ctx.v2(layer, g, "size", [100.0; 2]);
                let pos = ctx.v2(layer, g, "position", [0.0; 2]);
                push_shape(ctx, layer, g, effectcraft_path::ellipse(size, pos), arena, &mut live);
            }
            "star" => {
                let star = ctx.e(layer, g, "type") == 0;
                let p = effectcraft_path::polystar(effectcraft_path::Polystar {
                    star,
                    points: ctx.f(layer, g, "points", 5.0),
                    pos: ctx.v2(layer, g, "position", [0.0; 2]),
                    rotation: ctx.f(layer, g, "rotation", 0.0),
                    inner: ctx.f(layer, g, "innerRadius", 50.0),
                    outer: ctx.f(layer, g, "outerRadius", 100.0),
                    inner_round: ctx.f(layer, g, "innerRoundness", 0.0),
                    outer_round: ctx.f(layer, g, "outerRoundness", 0.0),
                });
                push_shape(ctx, layer, g, p, arena, &mut live);
            }
            "path" => {
                if let Some(Value::Path(p)) = ctx.group_value(layer, g, "path") {
                    push_shape(ctx, layer, g, effectcraft_path::to_kurbo(&p), arena, &mut live);
                }
            }
            "fill" => {
                let mut c = ctx.color(layer, g, "color");
                c[3] = 1.0;
                draws.push(Draw {
                    paint: Paint::Fill { color: c, rule: rule(ctx, layer, g) },
                    slots: live.clone(),
                    xf: Mat3::IDENTITY,
                    opacity: opacity(g),
                    blend: blend_mode(ctx, layer, g),
                    surfaces: [None; 4],
                    local: true,
                });
            }
            "stroke" => {
                let mut c = ctx.color(layer, g, "color");
                c[3] = 1.0;
                let style = stroke_style(ctx, layer, g);
                draws.push(Draw {
                    paint: Paint::Stroke { color: c, style },
                    slots: live.clone(),
                    xf: Mat3::IDENTITY,
                    opacity: opacity(g),
                    blend: blend_mode(ctx, layer, g),
                    surfaces: [None; 4],
                    local: true,
                });
            }
            "gfill" | "gstroke" => {
                let gr = match ctx.group_value(layer, g, "colors") {
                    Some(Value::Gradient(gr)) => gr,
                    _ => Gradient::default(),
                };
                let stroke = (g.match_id == "gstroke").then(|| stroke_style(ctx, layer, g));
                draws.push(Draw {
                    paint: Paint::Gradient {
                        g: gr,
                        radial: ctx.e(layer, g, "type") == 1,
                        start: ctx.v2(layer, g, "start", [0.0; 2]),
                        end: ctx.v2(layer, g, "end", [100.0, 0.0]),
                        rule: rule(ctx, layer, g),
                        stroke,
                        highlight: (ctx.f(layer, g, "highlightLength", 0.0), ctx.f(layer, g, "highlightAngle", 0.0)),
                    },
                    slots: live.clone(),
                    xf: Mat3::IDENTITY,
                    opacity: opacity(g),
                    blend: blend_mode(ctx, layer, g),
                    surfaces: [None; 4],
                    local: true,
                });
            }
            "trim" => {
                let (s, e, o) = (ctx.f(layer, g, "start", 0.0), ctx.f(layer, g, "end", 100.0), ctx.f(layer, g, "offset", 0.0));
                // Trim Multiple Shapes: 0 = Simultaneously (each path by the same fractions),
                // 1 = Individually (one after another, as one continuous length).
                if ctx.e(layer, g, "mode") == 1 {
                    apply(arena, &live, |p| ops::trim_individually(p, s, e, o));
                } else {
                    apply(arena, &live, |p| ops::trim(p, s, e, o));
                }
            }
            "pucker" => {
                let a = ctx.f(layer, g, "amount", 0.0);
                apply(arena, &live, |p| ops::pucker_bloat(p, a));
            }
            "round" => {
                let r = ctx.f(layer, g, "radius", 10.0);
                apply(arena, &live, |p| ops::round_corners(p, r));
            }
            "offset" => {
                let amount = ctx.f(layer, g, "amount", 10.0);
                let join = [Join::Miter, Join::Round, Join::Bevel][ctx.e(layer, g, "join").min(2) as usize];
                let miter = ctx.f(layer, g, "miter", 4.0);
                let copies = ctx.f(layer, g, "copies", 1.0);
                let copy_offset = ctx.f(layer, g, "copyOffset", 0.0);
                apply(arena, &live, |p| ops::offset(p, amount, join, miter, copies, copy_offset));
            }
            "twist" => {
                let angle = ctx.f(layer, g, "angle", 0.0);
                let center = ctx.v2(layer, g, "center", [0.0; 2]);
                apply(arena, &live, |p| ops::twist(p, angle, center));
            }
            "zigzag" => {
                let size = ctx.f(layer, g, "size", 10.0);
                let ridges = ctx.f(layer, g, "ridges", 5.0);
                let smooth = ctx.e(layer, g, "points") == 1;
                apply(arena, &live, |p| ops::zigzag(p, size, ridges, smooth));
            }
            "wiggle" => {
                let w = ops::WiggleParams {
                    size: ctx.f(layer, g, "size", 10.0),
                    detail: ctx.f(layer, g, "detail", 10.0),
                    smooth: ctx.e(layer, g, "points") == 1,
                    speed: ctx.f(layer, g, "speed", 2.0),
                    correlation: ctx.f(layer, g, "correlation", 50.0),
                    phase_deg: ctx.f(layer, g, "phase", 0.0),
                    spatial_phase_deg: ctx.f(layer, g, "spatialPhase", 0.0),
                    seed: ctx.f(layer, g, "seed", 0.0),
                };
                let t = layer.layer_time(ctx.time).seconds();
                apply(arena, &live, |p| ops::wiggle(p, &w, t));
            }
            "merge" => {
                let mode = ops::MergeMode::from_index(ctx.e(layer, g, "mode"));
                let input: Vec<BezPath> = live.iter().map(|&i| arena[i].clone()).collect();
                arena.push(ops::merge(&input, mode));
                live = vec![arena.len() - 1];
                // As in After Effects, fills and strokes above a Merge Paths in its group are not
                // rendered: the merged path is painted by the paint items below it. Paint inside
                // groups above the merge still renders — it belongs to those groups (#1174).
                draws.retain(|d| !d.local);
            }
            "group" => {
                let Some(inner) = g.sub("contents") else { continue };
                let (cd, cl) = collect(ctx, layer, inner, arena);
                let (m, op) = g.sub("transform").map(|t| group_matrix(ctx, layer, t)).unwrap_or((Mat3::IDENTITY, 1.0));
                for i in used_slots(&cl, &cd) {
                    arena[i] = mat_path(&arena[i], &m);
                }
                for mut d in cd {
                    d.xf = m * d.xf;
                    d.opacity *= op;
                    d.local = false;
                    draws.push(d);
                }
                live.extend(cl);
            }
            "repeater" => {
                let copies = ctx.f(layer, g, "copies", 3.0).max(0.0);
                let offset = ctx.f(layer, g, "offset", 0.0);
                let n = copies.ceil() as usize;
                let Some(tr) = g.sub("transform") else { continue };
                let anchor = ctx.v2(layer, tr, "anchor", [0.0; 2]);
                let pos = ctx.v2(layer, tr, "position", [100.0, 0.0]);
                let scale = ctx.v2(layer, tr, "scale", [100.0; 2]);
                let rot = ctx.f(layer, tr, "rotation", 0.0);
                let so = (ctx.f(layer, tr, "startOpacity", 100.0) / 100.0) as f32;
                let eo = (ctx.f(layer, tr, "endOpacity", 100.0) / 100.0) as f32;
                let step = |k: f64| {
                    Mat3::translate(vec2(pos[0] * k, pos[1] * k))
                        * Mat3::translate(vec2(anchor[0], anchor[1]))
                        * Mat3::rotate_deg(rot * k)
                        * Mat3::scale(vec2((scale[0] / 100.0).powf(k), (scale[1] / 100.0).powf(k)))
                        * Mat3::translate(vec2(-anchor[0], -anchor[1]))
                };
                let base_draws = std::mem::take(&mut draws);
                let base_live = std::mem::take(&mut live);
                let base_slots = used_slots(&base_live, &base_draws);
                // Each copy gets its own transformed slots, so operators below the repeater act on
                // the repeated geometry (copy order: original first).
                let mut copy_maps: Vec<std::collections::HashMap<usize, usize>> = Vec::with_capacity(n);
                for i in 0..n {
                    let m = step(i as f64 + offset);
                    let mut map = std::collections::HashMap::new();
                    for &s in &base_slots {
                        arena.push(mat_path(&arena[s], &m));
                        map.insert(s, arena.len() - 1);
                    }
                    live.extend(base_live.iter().map(|s| map[s]));
                    copy_maps.push(map);
                }
                let above = ctx.e(layer, g, "composite") == 1;
                let mut order: Vec<usize> = (0..n).collect();
                if !above {
                    order.reverse();
                }
                for i in order {
                    let m = step(i as f64 + offset);
                    let t = if n > 1 { i as f32 / (n - 1) as f32 } else { 0.0 };
                    let opk = so + (eo - so) * t;
                    let partial = if i + 1 == n && copies.fract() > 0.0 { copies.fract() as f32 } else { 1.0 };
                    for d in &base_draws {
                        let mut d = d.clone();
                        d.slots = d.slots.iter().map(|s| copy_maps[i][s]).collect();
                        d.xf = m * d.xf;
                        d.opacity *= opk * partial;
                        draws.push(d);
                    }
                }
            }
            m => {
                if let Some(i) = effectcraft_project::build::SHAPE_SURFACES.iter().position(|(s, _)| *s == m)
                    && g.get("color").is_some()
                    && let Some(slot) = surfaces.get_mut(i)
                {
                    let c = ctx.color(layer, g, "color");
                    *slot = Some([c[0], c[1], c[2]]);
                }
            }
        }
    }
    // Surface items colour every paint of the group, wherever they sit in it; nested groups'
    // own surface items were applied first and win.
    if surfaces.iter().any(Option::is_some) {
        for d in &mut draws {
            for (s, g) in d.surfaces.iter_mut().zip(surfaces) {
                *s = s.or(g);
            }
        }
    }
    (draws, live)
}

/// Gradient position of a point: linear along start→end, radial by distance from the start over
/// the radius |end − start|. A radial Highlight shifts the focal point towards
/// `highlight_length` % of the radius at `highlight_angle` from the start→end axis (a focal
/// two-point gradient: t is measured from the focal point to the circle along the ray).
#[derive(Clone, Copy, Debug)]
pub(crate) struct GradientRamp {
    radial: bool,
    s: [f64; 2],
    d: [f64; 2],
    len2: f64,
    focal: Option<[f64; 2]>,
}

impl GradientRamp {
    pub(crate) fn new(radial: bool, start: [f64; 2], end: [f64; 2], highlight: (f64, f64)) -> Self {
        let d = [end[0] - start[0], end[1] - start[1]];
        let len2 = (d[0] * d[0] + d[1] * d[1]).max(1e-9);
        let (hl, ha) = highlight;
        let focal = (radial && hl.abs() > 1e-6).then(|| {
            let r = len2.sqrt();
            let a = d[1].atan2(d[0]) + ha.to_radians();
            let dist = r * (hl / 100.0).clamp(-0.99, 0.99);
            [start[0] + dist * a.cos(), start[1] + dist * a.sin()]
        });
        GradientRamp { radial, s: start, d, len2, focal }
    }

    pub(crate) fn t(&self, x: f64, y: f64) -> f64 {
        let (vx, vy) = (x - self.s[0], y - self.s[1]);
        if !self.radial {
            return (vx * self.d[0] + vy * self.d[1]) / self.len2;
        }
        let Some(f) = self.focal else { return ((vx * vx + vy * vy) / self.len2).sqrt() };
        // Ray from the focal point through p meets the circle (centre s, radius r) at λ.
        let (px, py) = (x - f[0], y - f[1]);
        let dist = (px * px + py * py).sqrt();
        if dist < 1e-12 {
            return 0.0;
        }
        let (ux, uy) = (px / dist, py / dist);
        let (cx, cy) = (f[0] - self.s[0], f[1] - self.s[1]);
        let b = ux * cx + uy * cy;
        let c = cx * cx + cy * cy - self.len2;
        let lambda = -b + (b * b - c).max(0.0).sqrt();
        if lambda <= 1e-12 { 1.0 } else { dist / lambda }
    }
}

/// Pixel rectangle of the layer buffer (`x0, y0, width, height`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PxRect {
    x0: u32,
    y0: u32,
    w: u32,
    h: u32,
}

/// Composite `cov`-weighted colour (`color_at(x, y)` in buffer pixels, straight alpha) into the
/// `r` sub-rectangle of a premultiplied image (`cov` is `r.w × r.h`).
fn paint(img: &mut Image, r: PxRect, cov: &Mask, opacity: f32, blend: BlendMode, color_at: impl Fn(usize, usize) -> [f32; 4] + Sync) {
    let iw = img.width as usize;
    let (x0, y0, rw) = (r.x0 as usize, r.y0 as usize, r.w as usize);
    let row = |y: usize, out: &mut [[f32; 4]], crow: &[f32]| {
        for (dx, (px, &c)) in out[x0..x0 + rw].iter_mut().zip(crow).enumerate() {
            if c <= 0.0 {
                continue;
            }
            let x = x0 + dx;
            let col = color_at(x, y);
            let a = c * opacity * col[3];
            if a <= 0.0 {
                continue;
            }
            let src = [col[0] * a, col[1] * a, col[2] * a, a];
            if blend == BlendMode::Normal {
                let k = 1.0 - a;
                *px = [src[0] + px[0] * k, src[1] + px[1] * k, src[2] + px[2] * k, a + px[3] * k];
            } else {
                // Deterministic per-pixel noise for the dissolve modes.
                let h = ((x as u32).wrapping_mul(73_856_093) ^ (y as u32).wrapping_mul(19_349_663)).wrapping_mul(2_654_435_761);
                *px = blend_pixel(blend, *px, src, (h >> 8) as f32 / (1u32 << 24) as f32);
            }
        }
    };
    let rows = img.data.chunks_mut(iw).skip(y0).take(r.h as usize);
    if r.w as usize * r.h as usize >= 64 * 1024 {
        let rows: Vec<&mut [[f32; 4]]> = rows.collect();
        rows.into_par_iter().zip(cov.data.par_chunks(rw)).enumerate().for_each(|(dy, (out, crow))| row(y0 + dy, out, crow));
    } else {
        for (dy, (out, crow)) in rows.zip(cov.data.chunks(rw)).enumerate() {
            row(y0 + dy, out, crow);
        }
    }
}

/// Coverage of one draw inside its pixel rectangle.
fn draw_coverage(d: &Draw, paths: &[BezPath], m: &Mat3, r: PxRect) -> Mask {
    // Rasterise into a small mask: shift the buffer matrix so the rectangle starts at 0,0.
    let sub = Mat3::translate(vec2(-(r.x0 as f64), -(r.y0 as f64))) * *m;
    let stroke = |style: &StrokeStyle| {
        // Strokes scale with their group transform.
        let mut st = style.clone();
        st.width *= d.xf.mean_scale();
        effectcraft_path::stroke_coverage(paths, &st, &sub, r.w, r.h)
    };
    match &d.paint {
        Paint::Fill { rule, .. } => effectcraft_path::fill_coverage(paths, &sub, r.w, r.h, *rule),
        Paint::Stroke { style, .. } => stroke(style),
        Paint::Gradient { stroke: Some(style), .. } => stroke(style),
        Paint::Gradient { rule, .. } => effectcraft_path::fill_coverage(paths, &sub, r.w, r.h, *rule),
    }
}

/// Local-space bounds of what a draw can touch (stroke width, miters and anti-aliasing included).
fn draw_bounds(d: &Draw, paths: &[BezPath]) -> Option<kurbo::Rect> {
    let b = effectcraft_path::bounds(paths)?;
    let stroke = match &d.paint {
        Paint::Stroke { style, .. } => Some(style),
        Paint::Gradient { stroke: Some(style), .. } => Some(style),
        _ => None,
    };
    let grow = stroke.map_or(1.0, |style| style.width * stroke_reach(style, paths) * 0.5 * d.xf.mean_scale().max(1.0) + 1.0);
    Some(b.inflate(grow, grow))
}

/// How far a stroke can reach past its path, in half widths: the miter limit only where a
/// miter join exists (a path of two or more segments, or a closed one), √2 at square cap
/// corners, else one. Padding every stroke by the miter limit made a 1080 px straight line
/// (a wipe matte) a 4326×6366 buffer, ~440 MB per layer per frame.
fn stroke_reach(style: &StrokeStyle, paths: &[BezPath]) -> f64 {
    if style.is_variable() {
        // Taper and Wave can widen the stroke: keep the widest padding.
        return style.miter.max(std::f64::consts::SQRT_2);
    }
    let joins =
        style.join == Join::Miter && paths.iter().any(|p| p.segments().nth(1).is_some() || p.elements().iter().any(|e| matches!(e, kurbo::PathEl::ClosePath)));
    let cap = if style.cap == Cap::Square { std::f64::consts::SQRT_2 } else { 1.0 };
    if joins { style.miter.max(cap) } else { cap }
}

/// Local-space visual bounds of a draw, as After Effects' layer bounds (`sourceRectAtTime` with
/// extents): the path bounds, plus half the stroke width for strokes. No anti-aliasing or miter
/// padding, so snapping and Align put edges exactly together (#352).
fn visual_bounds(d: &Draw, paths: &[BezPath]) -> Option<kurbo::Rect> {
    let b = effectcraft_path::bounds(paths)?;
    let stroke = match &d.paint {
        Paint::Stroke { style, .. } => Some(style),
        Paint::Gradient { stroke: Some(style), .. } => Some(style),
        _ => None,
    };
    let grow = stroke.map_or(0.0, |style| style.width * 0.5 * d.xf.mean_scale());
    Some(if grow.is_finite() && grow > 0.0 { b.inflate(grow, grow) } else { b })
}

/// Maximum number of coverage pixels rasterised concurrently (bounds peak memory).
const BATCH_PIXELS: usize = 8 << 20;

/// Render a shape layer's contents into a layer-space buffer at scale `s`.
///
/// Each draw rasterises and composites only its own pixel rectangle; coverage masks of
/// consecutive draws are rasterised in parallel, then composited in stacking order.
pub fn render(ctx: &EvalCtx, layer: &Layer, contents: &PropGroup, s: f64) -> Buf {
    let mut arena = Vec::new();
    let (draws, _) = collect(ctx, layer, contents, &mut arena);
    // Bottom-most draw first.
    let resolved: Vec<(&Draw, Vec<BezPath>, kurbo::Rect)> = draws
        .iter()
        .rev()
        .filter(|d| d.opacity > 0.0)
        .filter_map(|d| {
            let paths: Vec<BezPath> = d.slots.iter().map(|&i| arena[i].clone()).filter(|p| !p.elements().is_empty()).collect();
            let b = draw_bounds(d, &paths)?;
            Some((d, paths, b))
        })
        .collect();
    let Some(b) = resolved.iter().map(|r| r.2).reduce(|a, b| a.union(b)) else { return Buf { img: Image::new(1, 1), offset: [0.0, 0.0], scale: s } };
    // Keep buffers sane for runaway geometry.
    let b = b.intersect(kurbo::Rect::new(-20000.0, -20000.0, 20000.0, 20000.0));
    let w = ((b.width() * s).ceil() as u32 + 4).clamp(1, 16384);
    let h = ((b.height() * s).ceil() as u32 + 4).clamp(1, 16384);
    let offset = [-b.x0 * s + 2.0, -b.y0 * s + 2.0];
    let m = Mat3::translate(vec2(offset[0], offset[1])) * Mat3::scale(vec2(s, s));
    let px_rect = |r: kurbo::Rect| -> Option<PxRect> {
        let x0 = (r.x0 * s + offset[0]).floor().max(0.0);
        let y0 = (r.y0 * s + offset[1]).floor().max(0.0);
        let x1 = (r.x1 * s + offset[0]).ceil().min(w as f64);
        let y1 = (r.y1 * s + offset[1]).ceil().min(h as f64);
        (x1 > x0 && y1 > y0).then_some(PxRect { x0: x0 as u32, y0: y0 as u32, w: (x1 - x0) as u32, h: (y1 - y0) as u32 })
    };
    let jobs: Vec<(&Draw, &[BezPath], PxRect)> = resolved.iter().filter_map(|(d, p, b)| Some((*d, p.as_slice(), px_rect(*b)?))).collect();
    let mut img = Image::new(w, h);
    let mut i = 0;
    while i < jobs.len() {
        // Next batch: as many draws as fit the pixel budget (at least one).
        let mut j = i;
        let mut px = 0usize;
        while j < jobs.len() && (j == i || px + jobs[j].2.w as usize * jobs[j].2.h as usize <= BATCH_PIXELS) {
            px += jobs[j].2.w as usize * jobs[j].2.h as usize;
            j += 1;
        }
        let covs: Vec<Mask> = jobs[i..j].par_iter().map(|(d, paths, r)| draw_coverage(d, paths, &m, *r)).collect();
        for ((d, _, r), cov) in jobs[i..j].iter().zip(&covs) {
            match &d.paint {
                Paint::Fill { color, .. } | Paint::Stroke { color, .. } => paint(&mut img, *r, cov, d.opacity, d.blend, |_, _| *color),
                Paint::Gradient { g, radial, start, end, highlight, .. } => {
                    let to_local = (m * d.xf).inverse().unwrap_or(Mat3::IDENTITY);
                    let ramp = GradientRamp::new(*radial, *start, *end, *highlight);
                    paint(&mut img, *r, cov, d.opacity, d.blend, |x, y| {
                        let p = to_local.apply(vec2(x as f64 + 0.5, y as f64 + 0.5));
                        g.sample(ramp.t(p.x, p.y))
                    });
                }
            }
        }
        i = j;
    }
    Buf { img, offset, scale: s }
}

#[cfg(test)]
mod tests;

/// The filled outlines of a shape layer in layer space with their straight colours (gradient
/// fills: the colour half way along), without strokes (see [`extrusion_outlines`]).
pub fn fill_outlines(ctx: &EvalCtx, layer: &Layer, contents: &PropGroup) -> Vec<(Vec<BezPath>, [f32; 4])> {
    let mut arena = Vec::new();
    let (draws, _) = collect(ctx, layer, contents, &mut arena);
    draws
        .iter()
        .rev()
        .filter(|d| d.opacity > 0.0)
        .filter_map(|d| {
            let c = match &d.paint {
                Paint::Fill { color, .. } => *color,
                Paint::Gradient { g, stroke: None, .. } => g.sample(0.5),
                _ => return None,
            };
            let paths: Vec<BezPath> = d.slots.iter().map(|&i| arena[i].clone()).filter(|p| !p.elements().is_empty()).collect();
            (!paths.is_empty()).then_some((paths, [c[0], c[1], c[2], c[3] * d.opacity]))
        })
        .collect()
}

/// The painted regions of a shape layer for Advanced 3D extrusion, bottom paint first: fills
/// (gradient fills: the colour half way along) and strokes, each stroke turned into the region
/// it covers (outline of the stroke with its width, joins, caps and dashes, overlaps resolved),
/// with the colours of their extrusion's surfaces.
pub fn extrusion_outlines(ctx: &EvalCtx, layer: &Layer, contents: &PropGroup) -> Vec<(Vec<BezPath>, [f32; 4], SurfaceColors)> {
    let mut arena = Vec::new();
    let (draws, _) = collect(ctx, layer, contents, &mut arena);
    draws
        .iter()
        .rev()
        .filter(|d| d.opacity > 0.0)
        .filter_map(|d| {
            let paths: Vec<BezPath> = d.slots.iter().map(|&i| arena[i].clone()).filter(|p| !p.elements().is_empty()).collect();
            if paths.is_empty() {
                return None;
            }
            let (c, region) = match &d.paint {
                Paint::Fill { color, .. } => (*color, paths),
                Paint::Gradient { g, stroke: None, .. } => (g.sample(0.5), paths),
                Paint::Stroke { color, style } => (*color, vec![stroke_region(&paths, style)?]),
                Paint::Gradient { g, stroke: Some(style), .. } => (g.sample(0.5), vec![stroke_region(&paths, style)?]),
            };
            Some((region, [c[0], c[1], c[2], c[3] * d.opacity], d.surfaces))
        })
        .collect()
}

/// The filled region a stroke covers: its outline with self-overlaps resolved (non-zero).
pub fn stroke_region(paths: &[BezPath], style: &StrokeStyle) -> Option<BezPath> {
    let o = effectcraft_path::stroke_outline(paths, style, 1.0)?;
    let r = effectcraft_path::boolean::normalize(&o, FillRule::NonZero);
    (!r.elements().is_empty()).then_some(r)
}

/// Layer-space visual bounds of a shape layer's painted contents (half the stroke width
/// included, no anti-aliasing padding), without rasterising.
pub fn content_bounds(ctx: &EvalCtx, layer: &Layer, contents: &PropGroup) -> Option<kurbo::Rect> {
    let mut arena = Vec::new();
    let (draws, _) = collect(ctx, layer, contents, &mut arena);
    draws
        .iter()
        .filter_map(|d| {
            let paths: Vec<BezPath> = d.slots.iter().filter_map(|&i| arena.get(i).cloned()).filter(|p| !p.elements().is_empty()).collect();
            visual_bounds(d, &paths)
        })
        .reduce(|a, b| a.union(b))
}

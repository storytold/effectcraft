use super::*;

/// The camera the viewer shows (view camera or the comp's active camera).
pub(super) fn cam_state(ctx: &EvalCtx) -> CameraState {
    VIEW_CAM.with(|c| c.get()).unwrap_or_else(|| three_d::active_camera(ctx))
}

/// Layer → comp (viewer) matrix through the current 3D view.
pub(crate) fn l2c(ctx: &EvalCtx, layer: &Layer) -> (Mat3, f64) {
    match VIEW_CAM.with(|c| c.get()) {
        Some(cam) if layer.is_3d() => (three_d::layer_to_view(ctx, layer, &cam), 0.0),
        _ => ctx.layer_to_comp(layer),
    }
}

/// `l2c` (layer → comp) applied from effect space instead: effect points and Puppet pins,
/// measured from the top-left of the layer's effect bounds (see `effect_bounds`).
pub(crate) fn from_effect_space(ctx: &EvalCtx, layer: &Layer, l2c: Mat3) -> Mat3 {
    let o = ctx.effect_bounds(layer).1;
    l2c * Mat3::translate(gv2(o[0], o[1]))
}

/// Effect space → comp (viewer) matrix through the current 3D view.
pub(crate) fn fx2c(ctx: &EvalCtx, layer: &Layer) -> Mat3 {
    from_effect_space(ctx, layer, l2c(ctx, layer).0)
}

/// The selected, unlocked layers of `comp` other than `layer` (when `layer` is one of them): a
/// handle or Rotation tool drag on one changes them all.
pub(super) fn selected_others<'a>(app: &EffectcraftApp, comp: &'a Comp, layer: LayerId) -> Vec<&'a Layer> {
    let sel = &app.session.state.selected_layers;
    if !sel.contains(&layer) {
        return vec![];
    }
    sel.iter().filter(|id| **id != layer).filter_map(|id| comp.layer(*id)).filter(|l| !l.switches.locked && l.transform().is_some()).collect()
}

/// The least homogeneous depth (`w`: the distance in front of a perspective camera in pixels, 1
/// for 2D layers and orthographic views) a viewer outline is drawn at: points at or behind the
/// camera would flip through infinity and draw long lines across the viewer (#572).
const NEAR_W: f64 = 0.5;

/// `p` through the homography `m` before the perspective divide: (x, y, w).
fn homogeneous(m: &Mat3, p: [f64; 2]) -> [f64; 3] {
    let r = |i: usize| m.0.get(i).map_or(0.0, |row| row[0] * p[0] + row[1] * p[1] + row[2]);
    [r(0), r(1), r(2)]
}

/// The closed polygon `poly` (layer pixels) through `m`, clipped to the part in front of the
/// camera's near plane (comp pixels; empty when none of it is).
pub(super) fn project_clipped(m: &Mat3, poly: &[[f64; 2]]) -> Vec<[f64; 2]> {
    let h: Vec<[f64; 3]> = poly.iter().map(|p| homogeneous(m, *p)).collect();
    let mut out = vec![];
    for (i, a) in h.iter().enumerate() {
        let Some(b) = h.get((i + 1) % h.len()) else { continue };
        let (ina, inb) = (a[2] >= NEAR_W, b[2] >= NEAR_W);
        if ina {
            out.push(*a);
        }
        if ina != inb {
            let t = (NEAR_W - a[2]) / (b[2] - a[2]);
            out.push([a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, NEAR_W]);
        }
    }
    out.iter().map(|q| [q[0] / q[2], q[1] / q[2]]).filter(|q| q[0].is_finite() && q[1].is_finite()).collect()
}

/// The segment a → b (world space) through the projection `pc`, clipped at the near plane.
pub(super) fn project_segment(
    pc: &effectcraft_engine::geom::Mat4,
    a: effectcraft_engine::geom::Vec3,
    b: effectcraft_engine::geom::Vec3,
) -> Option<[[f64; 2]; 2]> {
    let h = |p: effectcraft_engine::geom::Vec3| {
        let r = |i: usize| pc.0.get(i).map_or(0.0, |row| row[0] * p.x + row[1] * p.y + row[2] * p.z + row[3]);
        [r(0), r(1), r(3)]
    };
    let (mut ha, mut hb) = (h(a), h(b));
    if ha[2] < NEAR_W && hb[2] < NEAR_W {
        return None;
    }
    let cut = |p: [f64; 3], q: [f64; 3]| {
        let t = (NEAR_W - p[2]) / (q[2] - p[2]);
        [p[0] + (q[0] - p[0]) * t, p[1] + (q[1] - p[1]) * t, NEAR_W]
    };
    if ha[2] < NEAR_W {
        ha = cut(ha, hb);
    } else if hb[2] < NEAR_W {
        hb = cut(hb, ha);
    }
    let d = |q: [f64; 3]| [q[0] / q[2], q[1] / q[2]];
    let s = [d(ha), d(hb)];
    s.iter().flatten().all(|v| v.is_finite()).then_some(s)
}

/// The content box corners (layer pixels) of `layer`.
pub(super) fn box_corners(b: [f64; 4]) -> [[f64; 2]; 4] {
    [[b[0], b[1]], [b[2], b[1]], [b[2], b[3]], [b[0], b[3]]]
}

/// Layer → comp matrix and its bounds quad (in comp pixels). None for a 3D layer reaching to or
/// behind the camera (its quad would flip; [`layer_outline`] draws the part in front).
pub(crate) fn layer_quad(ctx: &EvalCtx, layer: &Layer) -> Option<(Mat3, [[f64; 2]; 4], [f64; 4])> {
    let b = effectcraft_engine::render::content_bounds(ctx, layer)?;
    let (m, _) = l2c(ctx, layer);
    let corners = box_corners(b);
    if corners.iter().any(|p| homogeneous(&m, *p)[2] < NEAR_W) {
        return None;
    }
    let pts = corners.map(|p| {
        let q = m.apply(gv2(p[0], p[1]));
        [q.x, q.y]
    });
    Some((m, pts, b))
}

/// The outline of a layer's content box in comp pixels, clipped at the camera's near plane.
pub(crate) fn layer_outline(ctx: &EvalCtx, layer: &Layer) -> Vec<[f64; 2]> {
    let Some(b) = effectcraft_engine::render::content_bounds(ctx, layer) else { return vec![] };
    project_clipped(&l2c(ctx, layer).0, &box_corners(b))
}

/// Which axes a bounding-box handle scales: corners (0–3) both; the top and bottom edges (4, 6)
/// y; the right and left edges (5, 7) x.
pub(super) fn handle_axes(handle: usize) -> [bool; 2] {
    match handle {
        4 | 6 => [false, true],
        5 | 7 => [true, false],
        _ => [true, true],
    }
}

/// A bounding-box handle's point in layer pixels (`b`: the content box; handles as in
/// [`handle_axes`], the corners clockwise from the top left, then the top, right, bottom and
/// left edges' midpoints).
pub(super) fn handle_point(b: [f64; 4], handle: usize) -> [f64; 2] {
    let (cx, cy) = ((b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0);
    match handle {
        0 => [b[0], b[1]],
        1 => [b[2], b[1]],
        2 => [b[2], b[3]],
        3 => [b[0], b[3]],
        4 => [cx, b[1]],
        5 => [b[2], cy],
        6 => [cx, b[3]],
        _ => [b[0], cy],
    }
}

/// The resize cursor over bounding-box handle `handle` of the on-screen box `sq` (corners
/// clockwise from the top left, handles as in [`handle_point`]): along the direction the handle
/// scales, so it turns with the layer, as in After Effects. A corner points between its two
/// edges (the box's diagonal whatever its proportions), an edge handle across its edge.
pub(super) fn handle_cursor(sq: &[Pos2], handle: usize) -> egui::CursorIcon {
    let at = |k: usize| sq.get(k % 4).copied().unwrap_or(Pos2::ZERO);
    let dir = if handle < 4 {
        let c = at(handle);
        (c - at(handle + 3)).normalized() + (c - at(handle + 1)).normalized()
    } else {
        let e = at(handle - 3) - at(handle - 4);
        vec2(-e.y, e.x)
    };
    // Screen y points down: 45° is the south-east diagonal.
    let a = dir.y.atan2(dir.x).to_degrees().rem_euclid(180.0);
    if !(22.5..157.5).contains(&a) {
        egui::CursorIcon::ResizeHorizontal
    } else if a < 67.5 {
        egui::CursorIcon::ResizeNwSe
    } else if a < 112.5 {
        egui::CursorIcon::ResizeVertical
    } else {
        egui::CursorIcon::ResizeNeSw
    }
}

/// The scale ratio (x, y) that puts the point grabbed at `start` (relative to the anchor, layer
/// pixels) under the pointer at `cur`. `uniform` (Shift on a corner) keeps the aspect ratio,
/// following the pointer along the handle's diagonal. Dragging past the anchor flips the layer,
/// as in After Effects; an axis whose grabbed point sits on the anchor keeps its scale.
pub(super) fn drag_ratio(start: [f64; 2], cur: [f64; 2], axes: [bool; 2], uniform: bool) -> [f64; 2] {
    if uniform {
        let d2 = start[0] * start[0] + start[1] * start[1];
        let f = if d2 > 1e-6 { (cur[0] * start[0] + cur[1] * start[1]) / d2 } else { 1.0 };
        return [f, f];
    }
    [0, 1].map(|k| if axes[k] && start[k].abs() > 1e-3 { cur[k] / start[k] } else { 1.0 })
}

/// Snap a handle drag (#252): the handle (`h`, layer pixels from the anchor point `a` as the
/// drag began; `l2c` that layer space → comp) at scale ratio `f` goes through `snap` (a comp
/// point → its correction), and the ratio that puts the handle where it snapped comes back.
/// A `uniform` drag slides the handle along its line from the anchor point to the nearer snapped
/// line; otherwise each scaled axis takes the snapped point.
pub(super) fn snap_handle(
    f: [f64; 2],
    h: [f64; 2],
    a: [f64; 2],
    l2c: &Mat3,
    axes: [bool; 2],
    uniform: bool,
    snap: impl FnOnce([f64; 2]) -> [f64; 2],
) -> [f64; 2] {
    let at = |f: [f64; 2]| {
        let q = l2c.apply(gv2(a[0] + f[0] * h[0], a[1] + f[1] * h[1]));
        [q.x, q.y]
    };
    let p = at(f);
    let c = snap(p);
    let s = [p[0] + c[0], p[1] + c[1]];
    if uniform {
        let o = at([0.0; 2]);
        let d = [p[0] - o[0], p[1] - o[1]];
        let k = [0, 1].into_iter().filter(|k| c[*k] != 0.0 && d[*k].abs() > 1e-9).min_by(|x, y| c[*x].abs().total_cmp(&c[*y].abs()));
        return k.map_or(f, |k| f.map(|v| v * (s[k] - o[k]) / d[k]));
    }
    let Some(inv) = l2c.inverse().filter(|_| c != [0.0; 2]) else { return f };
    let q = inv.apply(gv2(s[0], s[1]));
    let q = [q.x - a[0], q.y - a[1]];
    [0, 1].map(|k| if axes[k] && h[k].abs() > 1e-9 { q[k] / h[k] } else { f[k] })
}

/// The turn from angle `from` to angle `to` (degrees), wrapped to (−180°, 180°]: the shortest way
/// round, so summing it frame by frame follows the pointer across atan2's ±180° seam and past full
/// revolutions. Non-finite input turns by nothing.
pub(super) fn turn_delta(from: f64, to: f64) -> f64 {
    let d = to - from;
    if !d.is_finite() {
        return 0.0;
    }
    let w = (d + 180.0).rem_euclid(360.0) - 180.0;
    if w <= -180.0 { w + 360.0 } else { w }
}

/// The Rotation tool's value: the rotation as the drag began plus the turn so far (which may span
/// revolutions), snapped to 45° steps with Shift.
pub(super) fn rotate_value(start_rot: f64, turned: f64, shift: bool) -> f64 {
    let r = start_rot + turned;
    let r = if r.is_finite() { r } else { start_rot };
    if shift { (r / 45.0).round() * 45.0 } else { r }
}

/// A drag `d` (comp pixels), kept to the axis it moves along most when `shift` is held.
pub(super) fn one_axis(d: [f64; 2], shift: bool) -> [f64; 2] {
    if !shift {
        d
    } else if d[0].abs() > d[1].abs() {
        [d[0], 0.0]
    } else {
        [0.0, d[1]]
    }
}

pub(super) fn point_in_quad(p: [f64; 2], q: &[[f64; 2]; 4]) -> bool {
    let mut sign = 0.0;
    for i in 0..4 {
        let a = q[i];
        let b = q[(i + 1) % 4];
        let c = (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0]);
        if c.abs() < 1e-9 {
            continue;
        }
        if sign == 0.0 {
            sign = c.signum();
        } else if c.signum() != sign {
            return false;
        }
    }
    true
}

/// Separating-axis test: a marquee may cross a layer without containing any corner.
pub(super) fn quad_intersects_box(q: &[[f64; 2]; 4], a: [f64; 2], b: [f64; 2]) -> bool {
    if q.iter().flatten().chain(a.iter()).chain(b.iter()).any(|v| !v.is_finite()) {
        return false;
    }
    let lo = [a[0].min(b[0]), a[1].min(b[1])];
    let hi = [a[0].max(b[0]), a[1].max(b[1])];
    let rect = [lo, [hi[0], lo[1]], hi, [lo[0], hi[1]]];
    let axes = [[1.0, 0.0], [0.0, 1.0]].into_iter().chain((0..4).map(|i| {
        let j = (i + 1) % 4;
        [q[j][1] - q[i][1], q[i][0] - q[j][0]]
    }));
    for axis in axes {
        let project = |p: &[f64; 2]| p[0] * axis[0] + p[1] * axis[1];
        let (qmin, qmax) = q.iter().map(project).fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), v| (lo.min(v), hi.max(v)));
        let (rmin, rmax) = rect.iter().map(project).fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), v| (lo.min(v), hi.max(v)));
        if qmax < rmin || rmax < qmin {
            return false;
        }
    }
    true
}

/// Parent-space linear inverse (to convert a comp-space drag delta into a position delta).
pub(super) fn parent_inverse(ctx: &EvalCtx, layer: &Layer) -> Mat3 {
    match layer.parent.and_then(|p| ctx.comp.layer(p)) {
        Some(p) => {
            let w = ctx.world_matrix(p);
            let m = Mat3([[w.0[0][0], w.0[0][1], 0.0], [w.0[1][0], w.0[1][1], 0.0], [0.0, 0.0, 1.0]]);
            m.inverse().unwrap_or(Mat3::IDENTITY)
        }
        None => Mat3::IDENTITY,
    }
}

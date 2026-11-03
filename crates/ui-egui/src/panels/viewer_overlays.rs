//! Composition viewer overlays and their drags: mask and shape paths (vertices, tangents,
//! segments for Add Vertex), position motion paths with keyframe dots and spatial Bezier
//! handles, the path free-transform box and the Mask Feather tool.
//!
//! Every edit is an engine command (`mask.*`, `path.freeTransform`, `keys.set`,
//! `keys.setSpatialTangents`, `prop.set`).

use effectcraft_engine::geom::{Mat3, vec2 as gv2};
use effectcraft_engine::keyframe::ShapePath;
use effectcraft_engine::project::{GroupKind, Layer, LayerId, Property};
use effectcraft_engine::render::EvalCtx;
use effectcraft_engine::time::Tick;
use effectcraft_engine::{KeyRef, VertexRef};
use egui::{Color32, Pos2, Rect, Stroke, StrokeKind, vec2};
use serde_json::json;

use super::viewer::ViewerMap;
use crate::EffectcraftApp;

/// A vertex or tangent handle drawn this frame (masks and shape paths alike; `mask` is the path
/// group's uid).
#[derive(Clone, Copy)]
pub(crate) struct VertexHit {
    pub layer: LayerId,
    pub mask: u64,
    pub index: usize,
    pub pos: Pos2,
    /// None = the vertex itself, Some(out?) = a tangent handle.
    pub tangent: Option<bool>,
    /// Path-space vertex position and path → comp matrix.
    pub vertex: [f64; 2],
    pub l2c: Mat3,
}

/// A vertex marker to draw. Markers are drawn after the layer bounding boxes so a box handle
/// never covers a path point (#1042).
#[derive(Clone, Copy)]
pub(crate) struct VertexMark {
    pub pos: Pos2,
    pub color: Color32,
    pub selected: bool,
}

/// Draw the vertex markers collected by [`draw_paths`], `size` points wide.
pub(crate) fn draw_vertex_marks(painter: &egui::Painter, marks: &[VertexMark], size: f32) {
    for m in marks {
        let r = Rect::from_center_size(m.pos, vec2(size, size));
        if m.selected {
            painter.rect_filled(r, 0.0, m.color);
        } else {
            painter.rect_filled(r, 0.0, Color32::from_black_alpha(160));
            painter.rect_stroke(r, 0.0, Stroke::new(1.0, m.color), StrokeKind::Inside);
        }
    }
}

/// A path drawn this frame (for segment hits and the free-transform box).
#[derive(Clone)]
pub(crate) struct PathInfo {
    pub layer: LayerId,
    pub uid: u64,
    /// Path space → comp.
    pub m: Mat3,
    pub sp: ShapePath,
    pub is_mask: bool,
    /// Mask feather points' radius handles on screen (feather point index, handle position).
    pub feather: Vec<(usize, Pos2)>,
}

fn cubic(p0: [f64; 2], p1: [f64; 2], p2: [f64; 2], p3: [f64; 2], t: f64) -> [f64; 2] {
    let u = 1.0 - t;
    let f = |k: usize| u * u * u * p0[k] + 3.0 * u * u * t * p1[k] + 3.0 * u * t * t * p2[k] + t * t * t * p3[k];
    [f(0), f(1)]
}

/// Control points of segment `i` (vertex i → next) in path space.
pub(crate) fn segment(sp: &ShapePath, i: usize) -> Option<[[f64; 2]; 4]> {
    let n = sp.vertices.len();
    let j = if i + 1 < n {
        i + 1
    } else if sp.closed && n > 1 {
        0
    } else {
        return None;
    };
    let (a, d) = (sp.vertices[i], sp.vertices[j]);
    let to = sp.out_tangents.get(i).copied().unwrap_or_default();
    let ti = sp.in_tangents.get(j).copied().unwrap_or_default();
    Some([a, [a[0] + to[0], a[1] + to[1]], [d[0] + ti[0], d[1] + ti[1]], d])
}

/// The path segment under a screen point: (layer, path uid, segment, t).
pub(crate) fn segment_at(paths: &[PathInfo], map: &ViewerMap, pos: Pos2, tol: f32) -> Option<(LayerId, u64, usize, f64)> {
    let mut best: Option<(f32, (LayerId, u64, usize, f64))> = None;
    for p in paths {
        for i in 0..p.sp.vertices.len() {
            let Some(c) = segment(&p.sp, i) else { continue };
            for k in 1..48 {
                let t = k as f64 / 48.0;
                let q = cubic(c[0], c[1], c[2], c[3], t);
                let cq = p.m.apply(gv2(q[0], q[1]));
                let d = map.to_screen([cq.x, cq.y]).distance(pos);
                if d < tol && best.as_ref().is_none_or(|b| d < b.0) {
                    best = Some((d, (p.layer, p.uid, i, t)));
                }
            }
        }
    }
    best.map(|b| b.1)
}

/// The open path whose last vertex lies within `tol` points of `pos`: the Pen resumes it there.
pub(crate) fn open_end_at(paths: &[PathInfo], map: &ViewerMap, pos: Pos2, tol: f32) -> Option<(LayerId, u64)> {
    paths.iter().find_map(|p| {
        let v = p.sp.vertices.last().filter(|_| !p.sp.closed)?;
        let q = p.m.apply(gv2(v[0], v[1]));
        (map.to_screen([q.x, q.y]).distance(pos) < tol).then_some((p.layer, p.uid))
    })
}

/// A mask or shape path of a layer by group uid, evaluated at the context time.
pub(crate) fn path_of(ectx: &EvalCtx, layer: LayerId, uid: u64) -> Option<PathInfo> {
    let l = ectx.comp.layer(layer)?;
    let g = l.props.find_group(uid)?;
    let sp = g.get("path").map(|pr| ectx.value(l, pr)).and_then(|v| v.as_path().cloned())?;
    let (m, _) = super::viewer::l2c(ectx, l);
    let is_mask = matches!(g.kind, GroupKind::Mask { .. });
    let m = if is_mask { m } else { m * effectcraft_engine::render::shapes::item_matrix(ectx, l, uid).unwrap_or(Mat3::IDENTITY) };
    Some(PathInfo { layer, uid, m, sp, is_mask, feather: Vec::new() })
}

/// The path on screen: `sp` through `m` and the viewer map, flattened, with the closing point
/// repeated for a closed path.
pub(crate) fn flatten(sp: &ShapePath, m: &Mat3, map: &ViewerMap) -> Vec<Pos2> {
    let mut k = effectcraft_engine::render::kurbo_path(sp);
    // A path-space tolerance becomes visible corners when the viewer or a layer/group is
    // magnified. Flatten affine paths in screen space so the error stays below half a point.
    // Projective transforms must still map the flattened points: projecting Bezier control
    // points would change the curve rather than just its tessellation.
    let screen_space = m.is_affine();
    if screen_space {
        let z = f64::from(map.zoom);
        k.apply_affine(kurbo::Affine::new([
            m.0[0][0] * z,
            m.0[1][0] * z,
            m.0[0][1] * z,
            m.0[1][1] * z,
            m.0[0][2] * z + f64::from(map.origin.x),
            m.0[1][2] * z + f64::from(map.origin.y),
        ]));
    }
    if !k.is_finite() {
        return Vec::new();
    }
    let bounds = k.control_box();
    if [bounds.x0, bounds.y0, bounds.x1, bounds.y1].iter().any(|v| v.abs() > f64::from(f32::MAX)) {
        return Vec::new();
    }
    // Bound subdivision work even for extreme finite scales from a malformed project. For
    // ordinary on-screen paths the tolerance remains half a point.
    let tolerance = (bounds.width().max(bounds.height()) / 1_000_000.0).max(0.5);
    let mut pts: Vec<Pos2> = vec![];
    kurbo::flatten(k.iter(), tolerance, |el| match el {
        kurbo::PathEl::MoveTo(q) | kurbo::PathEl::LineTo(q) => {
            let p = if screen_space {
                egui::pos2(q.x as f32, q.y as f32)
            } else {
                let c = m.apply(gv2(q.x, q.y));
                map.to_screen([c.x, c.y])
            };
            if p.is_finite() {
                pts.push(p);
            }
        }
        _ => {}
    });
    if sp.closed
        && let Some(first) = pts.first().copied()
    {
        pts.push(first);
    }
    pts
}

/// Draw the masks and (on shape layers) shape paths of a selected layer with their vertices;
/// selected vertices show their tangent handles. Collects vertex hits and paths.
#[allow(clippy::too_many_arguments)]
pub(crate) fn draw_paths(
    app: &mut EffectcraftApp,
    painter: &egui::Painter,
    map: &ViewerMap,
    ectx: &EvalCtx,
    l: &Layer,
    label_col: Color32,
    sel: &[VertexRef],
    pen: Option<(LayerId, u64)>,
    hover: Option<Pos2>,
    hits: &mut Vec<VertexHit>,
    marks: &mut Vec<VertexMark>,
    paths: &mut Vec<PathInfo>,
) {
    let (m, _) = super::viewer::l2c(ectx, l);
    let mut list: Vec<(u64, String, ShapePath, Mat3, Color32, bool)> = vec![];
    if let Some(masks) = l.masks() {
        for g in masks.groups() {
            let GroupKind::Mask { color, locked, .. } = g.kind else { continue };
            if locked && app.session.state.hide_locked_masks {
                continue;
            }
            let Some(sp) = g.get("path").map(|pr| ectx.value(l, pr)).and_then(|v| v.as_path().cloned()) else { continue };
            list.push((g.uid, g.name.clone(), sp, m, Color32::from_rgb(color[0], color[1], color[2]), true));
        }
    }
    for (uid, sp) in effectcraft_engine::viewer::shape_paths(ectx, l) {
        let im = effectcraft_engine::render::shapes::item_matrix(ectx, l, uid).unwrap_or(Mat3::IDENTITY);
        let name = l.props.find_group(uid).map(|g| g.name.clone()).unwrap_or_default();
        list.push((uid, name, sp, m * im, label_col, false));
    }
    for (uid, name, sp, pm, mc, is_mask) in list {
        painter.add(egui::Shape::line(flatten(&sp, &pm, map), Stroke::new(1.0, mc)));
        let scr = |p: [f64; 2]| {
            let c = pm.apply(gv2(p[0], p[1]));
            map.to_screen([c.x, c.y])
        };
        for (i, v) in sp.vertices.iter().enumerate() {
            let s = scr(*v);
            let selected = sel.iter().any(|x| x.layer == l.id && x.mask == uid && x.index == i);
            if selected {
                for (out, tg) in [(false, sp.in_tangents.get(i)), (true, sp.out_tangents.get(i))] {
                    let Some(tg) = tg.filter(|t| t[0] != 0.0 || t[1] != 0.0) else { continue };
                    let h = scr([v[0] + tg[0], v[1] + tg[1]]);
                    painter.line_segment([s, h], Stroke::new(1.0, mc));
                    painter.circle_filled(h, 3.0, mc);
                    hits.push(VertexHit { layer: l.id, mask: uid, index: i, pos: h, tangent: Some(out), vertex: *v, l2c: pm });
                }
            }
            marks.push(VertexMark { pos: s, color: mc, selected });
            let kind = if is_mask { "mask" } else { "shapePath" };
            app.auto.add(&format!("viewer.{kind}.{uid}.vertex.{i}"), Rect::from_center_size(s, vec2(8.0, 8.0)), &name);
            hits.push(VertexHit { layer: l.id, mask: uid, index: i, pos: s, tangent: None, vertex: *v, l2c: pm });
        }
        // Pen rubber band from the last vertex to the pointer.
        if pen == Some((l.id, uid))
            && let (Some(last), Some(hp)) = (sp.vertices.last(), hover)
        {
            painter.line_segment([scr(*last), hp], Stroke::new(1.0, mc.gamma_multiply(0.6)));
        }
        let feather = if is_mask { draw_feather_points(painter, map, &sp, &pm, mc) } else { Vec::new() };
        for (i, h) in &feather {
            app.auto.add(&format!("viewer.mask.{uid}.feather.{i}"), Rect::from_center_size(*h, vec2(8.0, 8.0)), &name);
        }
        paths.push(PathInfo { layer: l.id, uid, m: pm, sp, is_mask, feather });
    }
}

/// Mask feather points: a hollow dot on the path and a radius handle along the normal (outwards for
/// outer feather, inwards for inner), joined by a line. Returns the handles' screen positions.
fn draw_feather_points(painter: &egui::Painter, map: &ViewerMap, sp: &ShapePath, pm: &Mat3, col: Color32) -> Vec<(usize, Pos2)> {
    let mut out = Vec::new();
    let scr = |p: [f64; 2]| {
        let c = pm.apply(gv2(p[0], p[1]));
        map.to_screen([c.x, c.y])
    };
    let inside = |q: [f64; 2]| {
        // Even-odd test against the vertex polygon (good enough to orient the handle).
        let v = &sp.vertices;
        let mut c = false;
        for i in 0..v.len() {
            let (a, b) = (v[i], v[(i + 1) % v.len()]);
            if (a[1] > q[1]) != (b[1] > q[1]) && q[0] < (b[0] - a[0]) * (q[1] - a[1]) / (b[1] - a[1]) + a[0] {
                c = !c;
            }
        }
        c
    };
    for (i, f) in sp.feather.iter().enumerate() {
        let Some(c) = segment(sp, f.segment) else { continue };
        let at = cubic(c[0], c[1], c[2], c[3], f.t);
        let (t0, t1) = ((f.t - 1e-3).max(0.0), (f.t + 1e-3).min(1.0));
        let (a, b) = (cubic(c[0], c[1], c[2], c[3], t0), cubic(c[0], c[1], c[2], c[3], t1));
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let len = dx.hypot(dy).max(1e-9);
        let mut n = [-dy / len, dx / len];
        if inside([at[0] + n[0] * 2.0, at[1] + n[1] * 2.0]) {
            n = [-n[0], -n[1]];
        }
        let h = [at[0] + n[0] * f.radius, at[1] + n[1] * f.radius];
        let (ps, hs) = (scr(at), scr(h));
        painter.line_segment([ps, hs], Stroke::new(1.0, col));
        painter.circle_stroke(ps, 3.5, Stroke::new(1.0, col));
        painter.circle_filled(hs, 3.0, col);
        out.push((i, hs));
    }
    out
}

// ---------------------------------------------------------------- motion paths

/// A keyframe dot or spatial tangent handle on a motion path.
#[derive(Clone, Copy)]
pub(crate) struct KeyHit {
    pub layer: LayerId,
    pub prop: u64,
    /// Layer time of the key.
    pub time: Tick,
    pub pos: Pos2,
    /// None = the key, Some(out?) = a tangent handle.
    pub tangent: Option<bool>,
    pub value: [f64; 3],
    /// Parent space → comp.
    pub p2c: Mat3,
    /// Separated dimensions: X and Y Position's uids, and whether each has a key at `time`.
    pub axes: Option<[(u64, bool); 2]>,
}

impl KeyHit {
    /// The keys this dot stands for: Position's, or the X and Y Position keys at its time.
    pub(crate) fn key_refs(&self) -> Vec<KeyRef> {
        match self.axes {
            Some(axes) => axes.iter().filter(|(_, has)| *has).map(|&(prop, _)| KeyRef { layer: self.layer, prop, time: self.time }).collect(),
            None => vec![KeyRef { layer: self.layer, prop: self.prop, time: self.time }],
        }
    }

    /// [`Self::key_refs`] as `keys.select` entries.
    pub(crate) fn select_entries(&self) -> Vec<serde_json::Value> {
        self.key_refs().iter().map(|k| json!({"layer": k.layer.0, "prop": k.prop, "time": k.time.seconds()})).collect()
    }
}

/// A frame on a motion path (one of its dots): where the Pen and Add Vertex tools add a
/// keyframe.
#[derive(Clone, Copy)]
pub(crate) struct PathFrame {
    pub layer: LayerId,
    pub prop: u64,
    /// Layer time of the frame.
    pub time: Tick,
    pub pos: Pos2,
    /// Separated dimensions: X and Y Position's uids, which get the key instead of `prop`.
    pub axes: Option<[u64; 2]>,
}

/// Layer times of the keys of separated X and Y Position together, in order.
fn separated_key_times(x: &Property, y: &Property) -> Vec<Tick> {
    let mut times: Vec<Tick> = x.keys.iter().chain(y.keys.iter()).map(|k| k.time).collect();
    times.sort();
    times.dedup();
    times
}

/// Separated X and Y Position at layer time `t` (z 0: motion paths are for 2D layers).
fn separated_value(x: &Property, y: &Property, t: Tick) -> [f64; 3] {
    [x.value_at(t).as_f64(), y.value_at(t).as_f64(), 0.0]
}

/// The keyframe dot on a motion path under `pos`.
pub(crate) fn motion_key_at(hits: &[KeyHit], pos: Pos2) -> Option<KeyHit> {
    hits.iter().rev().find(|h| h.tangent.is_none() && h.pos.distance(pos) < 6.0).copied()
}

/// The motion-path frame nearest `pos` (within a few points) where a keyframe can be added: not
/// on a keyframe dot already there.
pub(crate) fn motion_frame_at(frames: &[PathFrame], hits: &[KeyHit], pos: Pos2) -> Option<PathFrame> {
    if motion_key_at(hits, pos).is_some() {
        return None;
    }
    let near = frames.iter().filter(|f| f.pos.distance(pos) < 5.0).min_by(|a, b| a.pos.distance(pos).total_cmp(&b.pos.distance(pos)))?;
    (!hits.iter().any(|h| h.tangent.is_none() && h.layer == near.layer && h.prop == near.prop && h.time == near.time)).then_some(*near)
}

/// Parent space → comp matrix of a 2D layer at the context time.
pub(crate) fn parent_to_comp(ectx: &EvalCtx, l: &Layer) -> Mat3 {
    match l.parent.and_then(|p| ectx.comp.layer(p)) {
        Some(p) => {
            let w = ectx.world_matrix(p).0;
            Mat3([[w[0][0], w[0][1], w[0][3]], [w[1][0], w[1][1], w[1][3]], [0.0, 0.0, 1.0]])
        }
        None => Mat3::IDENTITY,
    }
}

/// Draw the position motion path of a selected 2D layer: frame dots along the path, keyframe
/// squares (filled when selected) and spatial Bezier tangent handles. Works through parents.
#[allow(clippy::too_many_arguments)]
pub(crate) fn motion_path(
    app: &mut EffectcraftApp,
    painter: &egui::Painter,
    map: &ViewerMap,
    ectx: &EvalCtx,
    l: &Layer,
    col: Color32,
    hits: &mut Vec<KeyHit>,
    frames: &mut Vec<PathFrame>,
) {
    let Some(tr) = l.transform() else { return };
    let Some(pos) = tr.get("position") else { return };
    // Separate Dimensions: the layer follows X / Y Position, so the path is drawn from their keys.
    let sep = tr.get("positionX").zip(tr.get("positionY"));
    let key_times: Vec<Tick> = match sep {
        Some((x, y)) => separated_key_times(x, y),
        None => pos.keys.iter().map(|k| k.time).collect(),
    };
    if key_times.len() < 2 || l.is_3d() {
        return;
    }
    let prop = sep.map_or(pos.uid, |(x, _)| x.uid);
    let frame_axes = sep.map(|(x, y)| [x.uid, y.uid]);
    let value_at = |t: Tick| match sep {
        Some((x, y)) => separated_value(x, y, t),
        None => pos.value_at(t).as_vec3(),
    };
    let p2c = parent_to_comp(ectx, l);
    let to_scr = |v: [f64; 3]| {
        let c = p2c.apply(gv2(v[0], v[1]));
        map.to_screen([c.x, c.y])
    };
    // Settings ▸ Composition ▸ Motion Path: all keyframes, none, or a window around the CTI.
    let Some((lt0, lt1)) = app.session.prefs.motion_path_span(&key_times, l.layer_time(ectx.time)) else { return };
    let fd = ectx.comp.frame_duration();
    let mut pts = vec![];
    let mut lt = lt0;
    while lt <= lt1 && pts.len() < 20_000 {
        let q = to_scr(value_at(lt));
        frames.push(PathFrame { layer: l.id, prop, time: lt, pos: q, axes: frame_axes });
        pts.push(q);
        lt += fd;
    }
    pts.push(to_scr(value_at(lt1)));
    let faint = col.gamma_multiply(0.8);
    painter.add(egui::Shape::line(pts.clone(), Stroke::new(1.0, faint)));
    for q in &pts {
        painter.circle_filled(*q, 1.2, faint);
    }
    let sel = &app.session.state.selected_keys;
    for (i, kt) in key_times.iter().enumerate() {
        if *kt < lt0 || *kt > lt1 {
            continue;
        }
        // The combined Position key at this time; none with separated dimensions.
        let key = pos.keys.get(i).filter(|_| sep.is_none());
        let v = key.map_or_else(|| value_at(*kt), |k| k.value.as_vec3());
        let s = to_scr(v);
        let axes = sep.map(|(x, y)| [x, y].map(|a| (a.uid, a.keys.iter().any(|k| k.time == *kt))));
        let hit = KeyHit { layer: l.id, prop, time: *kt, pos: s, tangent: None, value: v, p2c, axes };
        let selected = hit.key_refs().iter().any(|k| sel.contains(k));
        // A muted position key is off the motion path: no tangents, and a dimmed point.
        let kc = if key.is_some_and(|k| k.muted) { col.gamma_multiply(if selected { 0.55 } else { 0.35 }) } else { col };
        // Separated X / Y have no spatial tangents.
        if let Some(k) = key.filter(|k| !k.muted) {
            // Auto-Bezier keys show handles on a side the path leaves straight too (a two-key path's
            // ends), to drag it into a curve; Linear keys (no tangents) show none.
            let (tin, tout) = if k.spatial_auto {
                effectcraft_engine::keyframe::spatial_handles(&pos.keys, i)
            } else {
                effectcraft_engine::keyframe::spatial_tangents(&pos.keys, i)
            };
            for (out, tg) in [(false, tin), (true, tout)] {
                let has = (out && i + 1 < pos.keys.len()) || (!out && i > 0);
                if !has || (tg[0] == 0.0 && tg[1] == 0.0) {
                    continue;
                }
                let h = to_scr([v[0] + tg[0], v[1] + tg[1], v[2] + tg[2]]);
                painter.line_segment([s, h], Stroke::new(1.0, kc));
                painter.circle_filled(h, 3.0, kc);
                let side = if out { "out" } else { "in" };
                app.auto.add(&format!("viewer.motionPath.{}.{i}.{side}", l.id.0), Rect::from_center_size(h, vec2(8.0, 8.0)), "Spatial tangent");
                hits.push(KeyHit { layer: l.id, prop, time: *kt, pos: h, tangent: Some(out), value: v, p2c, axes: None });
            }
        }
        let r = Rect::from_center_size(s, vec2(7.0, 7.0));
        if selected {
            painter.rect_filled(r, 0.0, kc);
        } else {
            painter.rect_filled(r, 0.0, Color32::from_black_alpha(140));
            painter.rect_stroke(r, 0.0, Stroke::new(1.0, kc), StrokeKind::Inside);
        }
        app.auto.add(&format!("viewer.motionPath.{}.{i}", l.id.0), r.expand(2.0), "Position keyframe");
        hits.push(hit);
    }
}

// ---------------------------------------------------------------- free transform

/// The `egui::Id` under which the free-transform target (layer and mask or path id) is stored.
fn ft_id() -> egui::Id {
    egui::Id::unique("viewer-free-transform")
}

/// Enter Free Transform on a path (double-click a path with the Selection tool).
pub(crate) fn begin_free_transform(ctx: &egui::Context, layer: LayerId, uid: u64) {
    ctx.data_mut(|d| d.insert_temp(ft_id(), (layer, uid)));
}

pub(crate) fn end_free_transform(ctx: &egui::Context) {
    ctx.data_mut(|d| d.remove::<(LayerId, u64)>(ft_id()));
}

pub(crate) fn free_transform_target(ctx: &egui::Context) -> Option<(LayerId, u64)> {
    ctx.data(|d| d.get_temp(ft_id()))
}

/// The free-transform box: path-space bounds of the transformed vertices, its handles on screen
/// (corners 0–3, edge midpoints 4–7) and the path → comp matrix.
pub(crate) struct FtBox {
    pub layer: LayerId,
    pub uid: u64,
    pub b: [f64; 4],
    pub m: Mat3,
    pub handles: [Pos2; 8],
    pub quad: [Pos2; 4],
}

impl FtBox {
    /// Path-space position of handle `i`.
    pub fn handle_local(&self, i: usize) -> [f64; 2] {
        let [x0, y0, x1, y1] = self.b;
        let (cx, cy) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
        [[x0, y0], [x1, y0], [x1, y1], [x0, y1], [cx, y0], [x1, cy], [cx, y1], [x0, cy]][i]
    }
    pub fn center(&self) -> [f64; 2] {
        [(self.b[0] + self.b[2]) / 2.0, (self.b[1] + self.b[3]) / 2.0]
    }
}

/// Draw the free-transform box of the target path (if any) and return it for hit tests.
pub(crate) fn draw_free_transform(
    app: &mut EffectcraftApp,
    ctx: &egui::Context,
    painter: &egui::Painter,
    map: &ViewerMap,
    paths: &[PathInfo],
) -> Option<FtBox> {
    let (layer, uid) = free_transform_target(ctx)?;
    let Some(p) = paths.iter().find(|p| p.layer == layer && p.uid == uid) else {
        end_free_transform(ctx);
        return None;
    };
    let sel: Vec<usize> = app.session.state.selected_vertices.iter().filter(|v| v.layer == layer && v.mask == uid).map(|v| v.index).collect();
    let idx: Vec<usize> = if sel.len() >= 2 { sel } else { (0..p.sp.vertices.len()).collect() };
    let (mut lo, mut hi) = ([f64::MAX; 2], [f64::MIN; 2]);
    for &i in &idx {
        let Some(v) = p.sp.vertices.get(i) else { continue };
        lo = [lo[0].min(v[0]), lo[1].min(v[1])];
        hi = [hi[0].max(v[0]), hi[1].max(v[1])];
    }
    if lo[0] > hi[0] {
        return None;
    }
    let b = [lo[0], lo[1], hi[0].max(lo[0] + 1e-3), hi[1].max(lo[1] + 1e-3)];
    let mut f = FtBox { layer, uid, b, m: p.m, handles: [Pos2::ZERO; 8], quad: [Pos2::ZERO; 4] };
    for i in 0..8 {
        let q = f.handle_local(i);
        let c = p.m.apply(gv2(q[0], q[1]));
        f.handles[i] = map.to_screen([c.x, c.y]);
    }
    f.quad = [f.handles[0], f.handles[1], f.handles[2], f.handles[3]];
    let col = app.tokens.accent;
    painter.add(egui::Shape::closed_line(f.quad.to_vec(), Stroke::new(1.0, col)));
    for (i, h) in f.handles.iter().enumerate() {
        let r = Rect::from_center_size(*h, vec2(7.0, 7.0));
        painter.rect_filled(r, 0.0, Color32::WHITE);
        painter.rect_stroke(r, 0.0, Stroke::new(1.0, col), StrokeKind::Outside);
        app.auto.add(&format!("viewer.freeTransform.handle.{i}"), r, "Free transform handle");
    }
    let c = f.center();
    let cc = p.m.apply(gv2(c[0], c[1]));
    let cs = map.to_screen([cc.x, cc.y]);
    painter.circle_stroke(cs, 4.0, Stroke::new(1.0, col));
    Some(f)
}

fn point_in_quad(p: Pos2, q: &[Pos2; 4]) -> bool {
    let mut sign = 0.0f32;
    for i in 0..4 {
        let (a, b) = (q[i], q[(i + 1) % 4]);
        let c = (b.x - a.x) * (p.y - a.y) - (b.y - a.y) * (p.x - a.x);
        if c.abs() < 1e-6 {
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

// ---------------------------------------------------------------- drags

/// Overlay drags owned by this module.
#[derive(Clone, Debug)]
pub(crate) enum Drag {
    /// A motion-path keyframe: its value moves with the pointer (parent space).
    Key { layer: LayerId, prop: u64, time: Tick, start: [f64; 3], press: [f64; 2], inv: Mat3, axes: Option<[u64; 2]> },
    /// A spatial tangent handle of a motion-path key.
    KeyTangent { layer: LayerId, prop: u64, time: Tick, out: bool, key: [f64; 3], inv: Mat3 },
    /// Free transform: 0 move, 1 scale (anchor, axes), 2 rotate about `anchor`.
    Ft { layer: LayerId, path: u64, mode: u8, anchor: [f64; 2], last: [f64; 2], inv: Mat3, axes: [bool; 2] },
    /// Mask Feather tool: drag a feather point's radius handle (comp → layer matrix `inv`).
    Feather { layer: LayerId, mask: u64, index: usize, inv: Mat3 },
    /// A guide dragged in the viewer.
    Guide { index: usize, vertical: bool },
    /// Drawing the region of interest from `start`; `keep` pins the end's x / y (edge handles).
    Roi { start: [f64; 2], keep: [Option<f64>; 2] },
}

/// Start a motion-path drag at `press` (keys before tangents), selecting the key.
pub(crate) fn begin_key_drag(app: &mut EffectcraftApp, hits: &[KeyHit], press: Pos2, cpt: [f64; 2], shift: bool) -> Option<Drag> {
    let h = hits
        .iter()
        .rev()
        .filter(|h| h.tangent.is_some())
        .find(|h| h.pos.distance(press) < 6.0)
        .or_else(|| hits.iter().rev().find(|h| h.tangent.is_none() && h.pos.distance(press) < 6.0))?;
    let inv = h.p2c.inverse().unwrap_or(Mat3::IDENTITY);
    match h.tangent {
        Some(out) => Some(Drag::KeyTangent { layer: h.layer, prop: h.prop, time: h.time, out, key: h.value, inv }),
        None => {
            // Separated dimensions: both X and Y need a key here before the drag moves them.
            let mut h = *h;
            for (uid, has) in h.axes.iter_mut().flatten().filter(|(_, has)| !*has) {
                let _ = app.session.execute("prop.addKey", json!({"layer": h.layer.0, "prop": *uid, "time": h.time.seconds()}));
                *has = true;
            }
            if !h.key_refs().iter().all(|k| app.session.state.selected_keys.contains(k)) || shift {
                let _ = app.session.execute("keys.select", json!({"keys": h.select_entries(), "add": shift}));
            }
            let axes = h.axes.map(|a| a.map(|(uid, _)| uid));
            Some(Drag::Key { layer: h.layer, prop: h.prop, time: h.time, start: h.value, press: cpt, inv, axes })
        }
    }
}

/// Start a free-transform drag on the box: a handle scales about the opposite one (corners both
/// axes, edges one), inside moves, just outside a corner rotates about the centre.
pub(crate) fn begin_ft_drag(f: &FtBox, press: Pos2, map: &ViewerMap) -> Option<Drag> {
    let inv = f.m.inverse()?;
    let c = map.to_comp(press);
    let lp = inv.apply(gv2(c[0], c[1]));
    let lp = [lp.x, lp.y];
    if let Some(i) = (0..8).find(|i| f.handles[*i].distance(press) < 7.0) {
        let opp = [2, 3, 0, 1, 6, 7, 4, 5][i];
        let axes = match i {
            4 | 6 => [false, true],
            5 | 7 => [true, false],
            _ => [true, true],
        };
        return Some(Drag::Ft { layer: f.layer, path: f.uid, mode: 1, anchor: f.handle_local(opp), last: lp, inv, axes });
    }
    if point_in_quad(press, &f.quad) {
        return Some(Drag::Ft { layer: f.layer, path: f.uid, mode: 0, anchor: f.center(), last: lp, inv, axes: [true, true] });
    }
    if f.quad.iter().any(|q| q.distance(press) < 24.0) {
        return Some(Drag::Ft { layer: f.layer, path: f.uid, mode: 2, anchor: f.center(), last: lp, inv, axes: [true, true] });
    }
    None
}

/// Mask Feather tool: press on a feather point's handle to drag it, or on a mask path to add a
/// feather point there and drag its radius out.
pub(crate) fn begin_feather(app: &mut EffectcraftApp, ectx: &EvalCtx, paths: &[PathInfo], map: &ViewerMap, press: Pos2) -> Option<Drag> {
    let _ = ectx;
    for p in paths.iter().filter(|p| p.is_mask) {
        if let Some((i, _)) = p.feather.iter().find(|(_, h)| h.distance(press) < 7.0) {
            return Some(Drag::Feather { layer: p.layer, mask: p.uid, index: *i, inv: p.m.inverse()? });
        }
    }
    let masks: Vec<PathInfo> = paths.iter().filter(|p| p.is_mask).cloned().collect();
    let (layer, uid, seg, t) = segment_at(&masks, map, press, 10.0)?;
    let inv = masks.iter().find(|p| p.layer == layer && p.uid == uid)?.m.inverse()?;
    let r = app.session.execute("mask.featherPoint.add", json!({"layer": layer.0, "mask": uid, "segment": seg, "t": t, "radius": 0}));
    match r {
        Ok(v) => Some(Drag::Feather { layer, mask: uid, index: v["index"].as_u64()? as usize, inv }),
        Err(e) => {
            app.ui.status = e.to_string();
            None
        }
    }
}

/// Advance an overlay drag to the pointer (`cpt` comp, `pos` screen). `snap` corrects a comp
/// point for snapping. Returns the replacement drag state.
#[allow(clippy::too_many_arguments)]
pub(crate) fn update(
    app: &mut EffectcraftApp,
    d: &Drag,
    cpt: [f64; 2],
    pos: Pos2,
    mods: egui::Modifiers,
    merge: &str,
    zoom: f32,
    snap: &mut dyn FnMut(&EffectcraftApp, [f64; 2], LayerId) -> [f64; 2],
) -> Option<Drag> {
    match d.clone() {
        Drag::Key { layer, prop, time, start, press, inv, axes } => {
            // Snap the key's on-screen position like a layer drag.
            let start_c = inv.inverse().map(|m| m.apply(gv2(start[0], start[1]))).map(|v| [v.x, v.y]).unwrap_or([start[0], start[1]]);
            let mut target = [start_c[0] + cpt[0] - press[0], start_c[1] + cpt[1] - press[1]];
            let s = snap(app, target, layer);
            target = [target[0] + s[0], target[1] + s[1]];
            let p = inv.apply(gv2(target[0], target[1]));
            let edits = match axes {
                Some([ux, uy]) => vec![(ux, json!(p.x)), (uy, json!(p.y))],
                None => vec![(prop, json!([p.x, p.y, start[2]]))],
            };
            for (uid, v) in edits {
                let r = app.session.execute("keys.set", json!({"layer": layer.0, "prop": uid, "time": time.seconds(), "value": v, "merge": merge}));
                if let Err(e) = r {
                    app.ui.status = e.to_string();
                }
            }
            None
        }
        Drag::KeyTangent { layer, prop, time, out, key, inv } => {
            let p = inv.apply(gv2(cpt[0], cpt[1]));
            let tg = json!([p.x - key[0], p.y - key[1], 0.0]);
            let mut q = json!({"layer": layer.0, "prop": prop, "time": time.seconds(), "break": mods.alt, "merge": merge});
            q[if out { "out" } else { "in" }] = tg;
            let _ = app.session.execute("keys.setSpatialTangents", q);
            None
        }
        Drag::Ft { layer, path, mode, anchor, last, inv, axes } => {
            let p = inv.apply(gv2(cpt[0], cpt[1]));
            let cur = [p.x, p.y];
            let mut q = json!({"layer": layer.0, "mask": path, "anchor": anchor, "merge": merge});
            match mode {
                0 => q["offset"] = json!([cur[0] - last[0], cur[1] - last[1]]),
                1 => {
                    let f = |k: usize| {
                        let a = last[k] - anchor[k];
                        if axes[k] && a.abs() > 1e-6 { ((cur[k] - anchor[k]) / a).clamp(-50.0, 50.0) } else { 1.0 }
                    };
                    let (mut fx, mut fy) = (f(0), f(1));
                    if mods.shift && axes[0] && axes[1] {
                        let k = (fx + fy) / 2.0;
                        (fx, fy) = (k, k);
                    }
                    if fx.abs() < 1e-3 || fy.abs() < 1e-3 {
                        return None;
                    }
                    q["scale"] = json!([fx * 100.0, fy * 100.0]);
                }
                _ => {
                    let a0 = (last[1] - anchor[1]).atan2(last[0] - anchor[0]);
                    let a1 = (cur[1] - anchor[1]).atan2(cur[0] - anchor[0]);
                    q["rotation"] = json!((a1 - a0).to_degrees());
                }
            }
            if cur != last {
                let _ = app.session.execute("path.freeTransform", q);
            }
            Some(Drag::Ft { layer, path, mode, anchor, last: cur, inv, axes })
        }
        Drag::Feather { layer, mask, index, inv } => {
            let p = inv.apply(gv2(cpt[0], cpt[1]));
            let _ = (pos, zoom);
            let q = json!({"layer": layer.0, "mask": mask, "index": index, "toward": [p.x, p.y], "merge": merge});
            if let Err(e) = app.session.execute("mask.featherPoint.set", q) {
                app.ui.status = e.to_string();
            }
            None
        }
        Drag::Guide { index, .. } => {
            let _ = index;
            None
        }
        Drag::Roi { .. } => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scalar_prop(uid: u64, keys: &[(i64, f64)]) -> Property {
        use effectcraft_engine::keyframe::{Keyframe, Value};
        let mut p = Property::new(uid, "positionX", "X Position", Value::Scalar(0.0));
        p.keys = keys.iter().map(|&(t, v)| Keyframe::new(Tick(t), Value::Scalar(v))).collect();
        p
    }

    /// #2242: with separated dimensions the motion path follows the keys of X and Y Position.
    #[test]
    fn separated_motion_path_uses_x_and_y_keys() {
        let x = scalar_prop(1, &[(0, 10.0), (100, 30.0)]);
        let y = scalar_prop(2, &[(0, 0.0), (50, 20.0), (100, 0.0)]);
        assert_eq!(separated_key_times(&x, &y), vec![Tick(0), Tick(50), Tick(100)]);
        let mid = separated_value(&x, &y, Tick(50));
        assert!((mid[0] - 20.0).abs() < 1e-9 && (mid[1] - 20.0).abs() < 1e-9 && mid[2] == 0.0, "{mid:?}");
        assert_eq!(separated_value(&x, &y, Tick(100)), [30.0, 0.0, 0.0]);
    }

    /// #2242: a separated dot selects the X and Y keys at its time, not the nearest X key.
    #[test]
    fn separated_key_dot_refers_to_its_axis_keys() {
        let mut h = KeyHit {
            layer: LayerId(7),
            prop: 1,
            time: Tick(50),
            pos: Pos2::ZERO,
            tangent: None,
            value: [0.0; 3],
            p2c: Mat3::IDENTITY,
            axes: Some([(1, false), (2, true)]),
        };
        assert_eq!(h.key_refs(), vec![KeyRef { layer: LayerId(7), prop: 2, time: Tick(50) }]);
        h.axes = None;
        assert_eq!(h.key_refs(), vec![KeyRef { layer: LayerId(7), prop: 1, time: Tick(50) }]);
    }

    fn info(vertices: Vec<[f64; 2]>, closed: bool) -> PathInfo {
        let n = vertices.len();
        let sp = ShapePath { vertices, in_tangents: vec![[0.0; 2]; n], out_tangents: vec![[0.0; 2]; n], closed, feather: Vec::new() };
        PathInfo { layer: LayerId(3), uid: 7, m: Mat3::IDENTITY, sp, is_mask: true, feather: Vec::new() }
    }

    fn circle() -> ShapePath {
        let r = 38.0;
        let h = r * 0.552_284_749_830_793_6;
        ShapePath {
            vertices: vec![[0.0, -r], [r, 0.0], [0.0, r], [-r, 0.0]],
            in_tangents: vec![[-h, 0.0], [0.0, -h], [h, 0.0], [0.0, h]],
            out_tangents: vec![[h, 0.0], [0.0, h], [-h, 0.0], [0.0, -h]],
            closed: true,
            feather: Vec::new(),
        }
    }

    /// Measure the actual screen-space error against dense samples of the original cubics,
    /// independently of the tessellator's chosen subdivisions.
    fn screen_error(sp: &ShapePath, m: &Mat3, map: &ViewerMap, points: &[Pos2]) -> f32 {
        let mut worst = 0.0_f32;
        for i in 0..sp.vertices.len() {
            let Some(c) = segment(sp, i) else { continue };
            for step in 0..=1024 {
                let q = cubic(c[0], c[1], c[2], c[3], f64::from(step) / 1024.0);
                let q = m.apply(gv2(q[0], q[1]));
                let p = map.to_screen([q.x, q.y]);
                let distance = points
                    .windows(2)
                    .map(|pair| {
                        let a = pair[0];
                        let v = pair[1] - a;
                        let t = if v.length_sq() > 0.0 { ((p - a).dot(v) / v.length_sq()).clamp(0.0, 1.0) } else { 0.0 };
                        p.distance(a + v * t)
                    })
                    .fold(f32::INFINITY, f32::min);
                worst = worst.max(distance);
            }
        }
        worst
    }

    #[test]
    fn curve_overlay_stays_smooth_at_viewer_magnifications() {
        let sp = circle();
        let mut counts = Vec::new();
        for zoom in [0.5, 1.0, 5.0] {
            let map = ViewerMap { origin: egui::pos2(121.25, 210.5), zoom, comp: [640.0, 400.0], area: Rect::EVERYTHING };
            let points = flatten(&sp, &Mat3::IDENTITY, &map);
            let error = screen_error(&sp, &Mat3::IDENTITY, &map, &points);
            assert!(error <= 0.51, "{zoom}x: {error} screen points from the curve");
            assert_eq!(points.first(), points.last(), "closed paths keep their closing edge");
            for v in &sp.vertices {
                let anchor = map.to_screen(*v);
                assert!(points.iter().any(|p| p.distance(anchor) < 0.001), "original vertex missing at {zoom}x");
            }
            counts.push(points.len());
        }
        assert!(counts[0] < counts[1] && counts[1] < counts[2], "tessellation follows visible curve size: {counts:?}");
    }

    #[test]
    fn curve_overlay_accounts_for_nonuniform_layer_and_group_transforms() {
        let sp = circle();
        let layer = Mat3::translate(gv2(70.0, 50.0)) * Mat3::rotate_deg(27.0) * Mat3::scale(gv2(3.2, 0.8));
        let group = Mat3::rotate_deg(-19.0) * Mat3::skew_deg(25.0, 11.0) * Mat3::scale(gv2(0.7, 2.5));
        let m = layer * group;
        for zoom in [0.5, 1.0, 5.0] {
            let map = ViewerMap { origin: egui::pos2(121.25, 210.5), zoom, comp: [640.0, 400.0], area: Rect::EVERYTHING };
            let points = flatten(&sp, &m, &map);
            let error = screen_error(&sp, &m, &map, &points);
            assert!(error <= 0.51, "{zoom}x transformed path: {error} screen points from the curve");
        }
    }

    #[test]
    fn open_overlay_preserves_its_transformed_endpoints() {
        let sp = info(vec![[10.0, 20.0], [50.0, 60.0]], false).sp;
        let m = Mat3::translate(gv2(70.0, 50.0)) * Mat3::rotate_deg(27.0) * Mat3::scale(gv2(3.2, 0.8));
        let map = ViewerMap { origin: egui::pos2(121.25, 210.5), zoom: 5.0, comp: [640.0, 400.0], area: Rect::EVERYTHING };
        let points = flatten(&sp, &m, &map);
        assert_ne!(points.first(), points.last(), "an open path must not gain a closing edge");
        for (point, vertex) in [points.first(), points.last()].into_iter().zip([sp.vertices.first(), sp.vertices.last()]) {
            let q = m.apply(gv2(vertex.unwrap()[0], vertex.unwrap()[1]));
            assert!(point.unwrap().distance(map.to_screen([q.x, q.y])) < 0.001);
        }
    }

    #[test]
    fn invalid_overlay_coordinates_are_not_tessellated() {
        let sp = circle();
        for zoom in [f32::NAN, f32::INFINITY] {
            let map = ViewerMap { origin: Pos2::ZERO, zoom, comp: [640.0, 400.0], area: Rect::EVERYTHING };
            assert!(flatten(&sp, &Mat3::IDENTITY, &map).is_empty());
        }
        let map = ViewerMap { origin: Pos2::ZERO, zoom: 5.0, comp: [640.0, 400.0], area: Rect::EVERYTHING };
        assert!(flatten(&sp, &Mat3::scale(gv2(f64::INFINITY, 1.0)), &map).is_empty());
    }

    #[test]
    fn extreme_finite_overlay_scales_keep_subdivision_bounded() {
        let map = ViewerMap { origin: Pos2::ZERO, zoom: 5.0, comp: [640.0, 400.0], area: Rect::EVERYTHING };
        let sp = circle();
        let points = flatten(&sp, &Mat3::scale(gv2(1e12, 1e12)), &map);
        assert!(!points.is_empty());
        assert!(points.len() < 10_000, "extreme scales must not allocate millions of vertices");
        assert!(points.iter().all(|p| p.is_finite()));
        assert!(flatten(&sp, &Mat3::scale(gv2(1e300, 1e300)), &map).is_empty());
    }

    /// #912: clicking the last vertex of an open path with the Pen resumes that path.
    #[test]
    fn pen_resumes_an_open_path_at_its_last_vertex() {
        let map = ViewerMap { origin: Pos2::ZERO, zoom: 1.0, comp: [100.0, 100.0], area: Rect::EVERYTHING };
        let open = info(vec![[10.0, 10.0], [50.0, 10.0], [50.0, 50.0]], false);
        assert_eq!(open_end_at(std::slice::from_ref(&open), &map, egui::pos2(52.0, 49.0), 8.0), Some((LayerId(3), 7)));
        assert_eq!(open_end_at(std::slice::from_ref(&open), &map, egui::pos2(10.0, 10.0), 8.0), None, "first vertex");
        assert_eq!(open_end_at(std::slice::from_ref(&open), &map, egui::pos2(80.0, 80.0), 8.0), None, "far away");
        let closed = info(vec![[10.0, 10.0], [50.0, 10.0], [50.0, 50.0]], true);
        assert_eq!(open_end_at(&[closed], &map, egui::pos2(50.0, 50.0), 8.0), None, "closed path");
    }
}

//! Interactive path Free Transform: session state, geometry, pivot and per-gesture history.

use super::viewer::ViewerMap;
use super::viewer_overlays::{Drag, PathInfo, cubic, segment};
use crate::EffectcraftApp;
use effectcraft_engine::VertexRef;
use effectcraft_engine::geom::{Mat3, vec2 as gv2};
use effectcraft_engine::keyframe::ShapePath;
use effectcraft_engine::project::{GroupKind, LayerId};
use effectcraft_engine::render::EvalCtx;
use effectcraft_engine::time::Tick;
use egui::{Color32, Pos2, Rect, Stroke, StrokeKind, vec2};
use kurbo::Shape as _;
use serde_json::json;

/// All transient tool state lives here; project geometry stays in the engine history.
#[derive(Default)]
pub(crate) struct TransformSession {
    targets: Option<Vec<(LayerId, u64)>>,
    pivot: Option<Pivot>,
    orientation: Option<BoxOrientation>,
    history: TransformHistory,
}

// ---------------------------------------------------------------- free transform

/// Enter Free Transform on a path (double-click a path with the Selection tool).
pub(crate) fn begin_free_transform(app: &mut EffectcraftApp, ctx: &egui::Context, layer: LayerId, uid: u64) {
    reset_free_transform_pivot(app, ctx);
    app.free_transform.targets = Some(vec![(layer, uid)]);
}

/// Enter one shared transform for the selected paths, preserving selected vertex subsets.
pub(crate) fn begin_selected_free_transform(app: &mut EffectcraftApp, ctx: &egui::Context) -> bool {
    if app.session.state.text_edit.is_some() {
        return false;
    }
    let Some(cid) = app.session.active_comp_id() else { return false };
    let Some(comp) = app.session.active_comp() else { return false };
    let mut targets: Vec<(LayerId, u64)> = app.session.state.selected_vertices.iter().map(|v| (v.layer, v.mask)).collect();
    let explicit_vertices = !targets.is_empty();
    for (lid, uid) in app.session.state.selected_props.iter().filter(|_| !explicit_vertices) {
        let Some(layer) = comp.layer(*lid) else { continue };
        let group = layer.props.find_group(*uid).or_else(|| layer.props.parent_of(*uid));
        if let Some(g) = group.filter(|g| g.get("path").is_some_and(|p| g.uid == *uid || p.uid == *uid)) {
            targets.push((*lid, g.uid));
        }
    }
    targets.sort();
    targets.dedup();
    let eval = EvalCtx { expr: app.session.expr.as_deref(), ..EvalCtx::new(&app.session.project, cid, comp, app.session.time()) };
    let mut active = Vec::new();
    let mut vertices = Vec::new();
    for (lid, uid) in targets {
        let Some(layer) = comp.layer(lid).filter(|l| !l.switches.locked) else { continue };
        let Some(group) = layer.props.find_group(uid).filter(|g| !matches!(g.kind, GroupKind::Mask { locked: true, .. })) else { continue };
        let Some(prop) = group.get("path") else { continue };
        let value = eval.value(layer, prop);
        let Some(path) = value.as_path().filter(|p| !p.vertices.is_empty()) else { continue };
        let selected: Vec<_> =
            app.session.state.selected_vertices.iter().filter(|v| v.layer == lid && v.mask == uid && v.index < path.vertices.len()).cloned().collect();
        if !selected.is_empty() {
            vertices.extend(selected);
        } else {
            vertices.extend((0..path.vertices.len()).map(|index| VertexRef { layer: lid, mask: uid, index }));
        }
        active.push((lid, uid));
    }
    if active.is_empty() {
        return false;
    }
    let vertices: Vec<_> = vertices.iter().map(|v| json!({"layer": v.layer.0, "mask": v.mask, "index": v.index})).collect();
    if app.session.execute("mask.selectVertices", json!({"vertices": vertices})).is_err() {
        return false;
    }
    app.ui.tool = crate::state::Tool::Selection;
    reset_free_transform_pivot(app, ctx);
    app.free_transform.targets = Some(active);
    true
}

pub(crate) fn end_free_transform(app: &mut EffectcraftApp, ctx: &egui::Context) {
    reset_free_transform_pivot(app, ctx);
    app.free_transform.targets = None;
}

pub(crate) fn free_transform_target(app: &EffectcraftApp) -> Option<Vec<(LayerId, u64)>> {
    app.free_transform.targets.clone()
}

#[derive(Clone, PartialEq)]
struct Pivot {
    vertices: Vec<VertexRef>,
    point: [f64; 2],
    /// A shared box uses comp-space commands, but its pivot belongs to the reference layer.
    reference: Option<(LayerId, [f64; 2])>,
}

#[derive(Clone, PartialEq)]
struct BoxOrientation {
    vertices: Vec<VertexRef>,
    angle: f64,
}

fn box_angle(app: &EffectcraftApp, _ctx: &egui::Context) -> f64 {
    app.free_transform.orientation.clone().filter(|o| o.vertices == app.session.state.selected_vertices).map_or(0.0, |o| o.angle)
}

pub(crate) fn reset_free_transform_pivot(app: &mut EffectcraftApp, _ctx: &egui::Context) {
    app.free_transform.history = TransformHistory::default();
    app.free_transform.pivot = None;
    app.free_transform.orientation = None;
}

// The document history owns geometry. This bounded session history interleaves UI-only pivot
// gestures with those document steps, and restores the box alongside Undo/Redo inside the tool.
#[derive(Clone)]
struct TransformSnapshot {
    project: std::sync::Arc<effectcraft_engine::project::Project>,
    comp: Option<effectcraft_engine::project::ItemId>,
    time: Tick,
    vertices: Vec<VertexRef>,
    targets: Vec<(LayerId, u64)>,
    pivot: Option<Pivot>,
    orientation: Option<BoxOrientation>,
}

impl TransformSnapshot {
    fn capture(app: &EffectcraftApp, _ctx: &egui::Context) -> Self {
        Self {
            project: app.session.project.clone(),
            comp: app.session.active_comp_id(),
            time: app.session.time(),
            vertices: app.session.state.selected_vertices.clone(),
            targets: free_transform_target(app).unwrap_or_default(),
            pivot: app.free_transform.pivot.clone(),
            orientation: app.free_transform.orientation.clone(),
        }
    }

    fn matches(&self, app: &EffectcraftApp) -> bool {
        self.comp == app.session.active_comp_id()
            && self.time == app.session.time()
            && self.vertices == app.session.state.selected_vertices
            && (std::sync::Arc::ptr_eq(&self.project, &app.session.project) || self.project.same_content(&app.session.project))
    }

    fn restore(&self, app: &mut EffectcraftApp, ctx: &egui::Context) {
        app.session.state.selected_vertices = self.vertices.clone();
        app.ui.tool = crate::state::Tool::Selection;
        app.free_transform.targets = Some(self.targets.clone());
        app.free_transform.pivot = self.pivot.clone();
        app.free_transform.orientation = self.orientation.clone();
        ctx.request_repaint();
    }
}

#[derive(Clone)]
struct TransformStep {
    label: &'static str,
    before: TransformSnapshot,
    after: TransformSnapshot,
    geometry: bool,
}

#[derive(Default)]
pub(crate) struct TransformHistory {
    undo: Vec<TransformStep>,
    redo: Vec<TransformStep>,
    pending: Option<(String, &'static str, TransformSnapshot)>,
}

/// Cheap menu invalidation data; eligibility/content comparison belongs to menu construction,
/// not to the per-frame fingerprint. The engine revision covers external document changes.
pub(crate) fn transform_history_key(app: &EffectcraftApp) -> (bool, usize, usize, Option<&'static str>, Option<&'static str>) {
    let session = &app.free_transform;
    (
        session.targets.is_some(),
        session.history.undo.len(),
        session.history.redo.len(),
        session.history.undo.last().map(|s| s.label),
        session.history.redo.last().map(|s| s.label),
    )
}

pub(crate) fn transform_history_label(app: &EffectcraftApp, redo: bool) -> Option<&'static str> {
    free_transform_target(app)?;
    let history = &app.free_transform.history;
    let step = if redo { history.redo.last() } else { history.undo.last() }?;
    let current = if redo { &step.before } else { &step.after };
    current.matches(app).then_some(step.label)
}

fn begin_transform_step(app: &mut EffectcraftApp, ctx: &egui::Context, merge: &str, label: &'static str) {
    if app.free_transform.history.pending.as_ref().is_some_and(|(key, ..)| key == merge) {
        return;
    }
    finish_transform_step(app, ctx);
    if app.free_transform.history.undo.last().is_some_and(|s| !s.after.matches(app)) {
        app.free_transform.history = TransformHistory::default();
    }
    app.session.begin_edit_gesture();
    let snapshot = TransformSnapshot::capture(app, ctx);
    app.free_transform.history.pending = Some((merge.to_string(), label, snapshot));
}

pub(crate) fn finish_transform_step(app: &mut EffectcraftApp, ctx: &egui::Context) {
    let Some((_, label, before)) = app.free_transform.history.pending.take() else { return };
    let after = TransformSnapshot::capture(app, ctx);
    let geometry = !before.project.same_content(&after.project);
    if !geometry && before.pivot == after.pivot && before.orientation == after.orientation {
        return;
    }
    // A new pivot gesture after Undo is a new future too, although it doesn't dirty the file.
    if !app.free_transform.history.redo.is_empty() {
        app.session.abandon_redo_future();
    }
    app.free_transform.history.redo.clear();
    let limit = (app.session.prefs.general.undo_levels as usize).clamp(1, 1024);
    if app.free_transform.history.undo.len() >= limit {
        app.free_transform.history.undo.remove(0);
    }
    app.free_transform.history.undo.push(TransformStep { label, before, after, geometry });
}

/// None lets ordinary document Undo continue once the session's steps are exhausted or stale.
pub(crate) fn transform_undo(app: &mut EffectcraftApp, ctx: &egui::Context, redo: bool) -> Option<Result<serde_json::Value, String>> {
    free_transform_target(app)?;
    finish_transform_step(app, ctx);
    let step = if redo { app.free_transform.history.redo.last() } else { app.free_transform.history.undo.last() }?.clone();
    let (current, target) = if redo { (&step.before, &step.after) } else { (&step.after, &step.before) };
    if !current.matches(app) {
        app.free_transform.history = TransformHistory::default();
        return None;
    }
    if step.geometry {
        if !app.session.adjacent_history_matches(redo, &target.project) {
            app.free_transform.history = TransformHistory::default();
            return None;
        }
        if let Err(e) = app.session.execute(if redo { "edit.redo" } else { "edit.undo" }, json!({})) {
            return Some(Err(e.to_string()));
        }
    }
    super::viewer::cancel_transform_gesture(ctx);
    target.restore(app, ctx);
    app.session.begin_edit_gesture();
    if redo {
        app.free_transform.history.redo.pop();
        app.free_transform.history.undo.push(step.clone());
    } else {
        app.free_transform.history.undo.pop();
        app.free_transform.history.redo.push(step.clone());
    }
    Some(Ok(json!({if redo {"redone"} else {"undone"}: step.label, "interactive":true})))
}

fn pivot_layer_matrix(app: &EffectcraftApp, layer: LayerId) -> Option<Mat3> {
    let cid = app.session.active_comp_id()?;
    let comp = app.session.active_comp()?;
    let layer = comp.layer(layer)?;
    Some(EvalCtx::new(&app.session.project, cid, comp, app.session.time()).layer_to_comp(layer).0)
}

fn custom_pivot(app: &EffectcraftApp, _ctx: &egui::Context) -> Option<[f64; 2]> {
    let p = app.free_transform.pivot.clone().filter(|p| p.vertices == app.session.state.selected_vertices)?;
    if let Some((layer, point)) = p.reference {
        let point = pivot_layer_matrix(app, layer)?.apply(gv2(point[0], point[1]));
        return Some([point.x, point.y]);
    }
    Some(p.point)
}

fn set_pivot(app: &mut EffectcraftApp, _ctx: &egui::Context, point: [f64; 2]) {
    if point.iter().all(|v| v.is_finite()) {
        let reference = free_transform_target(app).filter(|paths| paths.len() > 1).and_then(|paths| paths.first().copied()).and_then(|(layer, _)| {
            let local = pivot_layer_matrix(app, layer)?.inverse()?.apply(gv2(point[0], point[1]));
            Some((layer, [local.x, local.y]))
        });
        app.free_transform.pivot = Some(Pivot { vertices: app.session.state.selected_vertices.clone(), point, reference });
    }
}

/// Constrain a displacement in composition space, relative to the start of the gesture.
pub(crate) fn constrain_move(mut delta: [f64; 2], shift: bool) -> [f64; 2] {
    if shift {
        if delta[0].abs() >= delta[1].abs() {
            delta[1] = 0.0;
        } else {
            delta[0] = 0.0;
        }
    }
    delta
}

/// The free-transform box: bounds of selected Bézier geometry, its handles on screen
/// (corners 0–3, edge midpoints 4–7) and the path → comp matrix.
pub(crate) struct FtBox {
    pub targets: Vec<(LayerId, u64)>,
    pub comp_space: bool,
    pub b: [f64; 4],
    pub m: Mat3,
    pub handles: [Pos2; 8],
    pub quad: [Pos2; 4],
    pub pivot: [f64; 2],
    pub angle: f64,
}

impl FtBox {
    /// Path-space position of handle `i`.
    pub fn handle_local(&self, i: usize) -> [f64; 2] {
        let [x0, y0, x1, y1] = self.b;
        let (cx, cy) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
        [[x0, y0], [x1, y0], [x1, y1], [x0, y1], [cx, y0], [x1, cy], [cx, y1], [x0, cy]][i]
    }
}

/// Bounds of selected vertices and the Bézier segments whose two endpoints are selected.
/// Cubic extrema give the actual curve bounds, rather than the control polygon's bounds.
fn selected_path_bounds(sp: &ShapePath, indices: impl IntoIterator<Item = usize>, m: &Mat3) -> Option<kurbo::Rect> {
    let indices: std::collections::BTreeSet<_> = indices.into_iter().filter(|i| *i < sp.vertices.len()).collect();
    let mut bounds: Option<kurbo::Rect> = None;
    let mut include = |r: kurbo::Rect| {
        if [r.x0, r.y0, r.x1, r.y1].iter().all(|v| v.is_finite()) {
            bounds = Some(bounds.map_or(r, |b| b.union(r)));
        }
    };
    for &i in &indices {
        let Some(v) = sp.vertices.get(i) else { continue };
        let q = m.apply(gv2(v[0], v[1]));
        include(kurbo::Rect::new(q.x, q.y, q.x, q.y));
        let next = i.saturating_add(1);
        let next = if next < sp.vertices.len() {
            next
        } else if sp.closed {
            0
        } else {
            continue;
        };
        if !indices.contains(&next) {
            continue;
        }
        let Some(points) = segment(sp, i) else { continue };
        if !points.iter().flatten().all(|v| v.is_finite()) {
            continue;
        }
        if m.is_affine() {
            let [a, b, c, d] = points.map(|v| {
                let q = m.apply(gv2(v[0], v[1]));
                kurbo::Point::new(q.x, q.y)
            });
            if [a, b, c, d].iter().all(|p| p.is_finite()) {
                include(kurbo::CubicBez::new(a, b, c, d).bounding_box());
            }
        } else {
            // A 3D projection is rational, so projecting a cubic's four controls is not a
            // cubic transform. Keep its fallback bounded; 2D bounds above are analytic.
            for step in 0..=128 {
                let v = cubic(points[0], points[1], points[2], points[3], step as f64 / 128.0);
                let q = m.apply(gv2(v[0], v[1]));
                include(kurbo::Rect::new(q.x, q.y, q.x, q.y));
            }
        }
    }
    bounds
}

/// Draw the free-transform box of the target path (if any) and return it for hit tests.
pub(crate) fn draw_free_transform(
    app: &mut EffectcraftApp,
    ctx: &egui::Context,
    painter: &egui::Painter,
    map: &ViewerMap,
    paths: &[PathInfo],
) -> Option<FtBox> {
    let targets = free_transform_target(app)?;
    let selected: Vec<_> = targets.iter().filter_map(|(lid, uid)| paths.iter().find(|p| p.layer == *lid && p.uid == *uid)).collect();
    let Some(first) = selected.first() else {
        end_free_transform(app, ctx);
        return None;
    };
    let comp_space = selected.len() > 1;
    let angle = box_angle(app, ctx);
    let basis = Mat3::rotate_deg(angle);
    let unrotate = Mat3::rotate_deg(-angle);
    let m = if comp_space { basis } else { first.m * basis };
    let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
    for p in &selected {
        let indices = app.session.state.selected_vertices.iter().filter(|v| v.layer == p.layer && v.mask == p.uid).map(|v| v.index);
        let matrix = if comp_space { unrotate * p.m } else { unrotate };
        let Some(b) = selected_path_bounds(&p.sp, indices, &matrix) else { continue };
        lo = [lo[0].min(b.x0), lo[1].min(b.y0)];
        hi = [hi[0].max(b.x1), hi[1].max(b.y1)];
    }
    if !lo.iter().chain(hi.iter()).all(|v| v.is_finite()) {
        end_free_transform(app, ctx);
        return None;
    }
    let b = [lo[0], lo[1], hi[0].max(lo[0] + 1e-3), hi[1].max(lo[1] + 1e-3)];
    let targets = selected.iter().map(|p| (p.layer, p.uid)).collect();
    let center = basis.apply(gv2((b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0));
    let center = [center.x, center.y];
    let pivot = custom_pivot(app, ctx).map_or(center, |point| {
        let placement = if comp_space { Mat3::IDENTITY } else { first.m };
        let to_screen = |p: [f64; 2]| {
            let p = placement.apply(gv2(p[0], p[1]));
            map.to_screen([p.x, p.y])
        };
        if to_screen(point).distance(to_screen(center)) <= 0.5 {
            // Returning to the center restores automatic following, regardless of drag history.
            app.free_transform.pivot = None;
            center
        } else {
            point
        }
    });
    let pivot = unrotate.apply(gv2(pivot[0], pivot[1]));
    let pivot = [pivot.x, pivot.y];
    let mut f = FtBox { targets, comp_space, b, m, handles: [Pos2::ZERO; 8], quad: [Pos2::ZERO; 4], pivot, angle };
    for i in 0..8 {
        let q = f.handle_local(i);
        let c = f.m.apply(gv2(q[0], q[1]));
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
    let c = f.pivot;
    let cc = f.m.apply(gv2(c[0], c[1]));
    let cs = map.to_screen([cc.x, cc.y]);
    painter.circle_stroke(cs, 4.0, Stroke::new(1.0, col));
    painter.line_segment([cs - vec2(6.0, 0.0), cs + vec2(6.0, 0.0)], Stroke::new(1.0, col));
    painter.line_segment([cs - vec2(0.0, 6.0), cs + vec2(0.0, 6.0)], Stroke::new(1.0, col));
    app.auto.add("viewer.freeTransform.pivot", Rect::from_center_size(cs, vec2(12.0, 12.0)), "Free Transform pivot");
    if ctx.pointer_hover_pos().is_some_and(|p| p.distance(cs) < 7.0) {
        ctx.set_cursor_icon(egui::CursorIcon::Default);
    }
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

/// Rotation is available along the whole outside edge, measured in screen points.
fn rotation_zone(f: &FtBox, pointer: Pos2) -> bool {
    if point_in_quad(pointer, &f.quad) {
        return false;
    }
    f.quad.iter().zip(f.quad.iter().cycle().skip(1)).take(4).any(|(a, b)| {
        let edge = *b - *a;
        let t = if edge.length_sq() > 1e-6 { ((pointer - *a).dot(edge) / edge.length_sq()).clamp(0.0, 1.0) } else { 0.0 };
        pointer.distance(*a + edge * t) <= 32.0
    })
}

pub(crate) fn ft_cursor(f: &FtBox, pointer: Pos2, map: &ViewerMap) -> Option<egui::CursorIcon> {
    let pivot = f.m.apply(gv2(f.pivot[0], f.pivot[1]));
    if pointer.distance(map.to_screen([pivot.x, pivot.y])) < 7.0 {
        return Some(egui::CursorIcon::Default);
    }
    if let Some(handle) = f.handles.iter().find(|h| h.distance(pointer) < 7.0) {
        let center = f.quad.iter().fold(egui::Vec2::ZERO, |sum, p| sum + p.to_vec2()) / 4.0;
        let direction = handle.to_vec2() - center;
        let octant = (direction.y.atan2(direction.x).to_degrees() / 45.0).round().rem_euclid(4.0) as u8;
        return Some(match octant {
            0 => egui::CursorIcon::ResizeHorizontal,
            1 => egui::CursorIcon::ResizeNwSe,
            2 => egui::CursorIcon::ResizeVertical,
            _ => egui::CursorIcon::ResizeNeSw,
        });
    }
    if point_in_quad(pointer, &f.quad) {
        return Some(egui::CursorIcon::Default);
    }
    rotation_zone(f, pointer).then_some(egui::CursorIcon::Default)
}

pub(crate) fn rotation_cursor(ctx: &egui::Context, f: &FtBox, pointer: Pos2, rotating: bool) {
    if rotating || (rotation_zone(f, pointer) && !f.handles.iter().any(|h| h.distance(pointer) < 7.0)) {
        let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::unique("free-transform-rotation-cursor")));
        let rect = Rect::from_min_size(pointer + vec2(10.0, 8.0), vec2(19.0, 19.0));
        crate::icons::paint(&painter, rect.expand(1.0), crate::icons::Icon::Rotate, Color32::BLACK);
        crate::icons::paint(&painter, rect, crate::icons::Icon::Rotate, Color32::WHITE);
    }
}

fn pivot_snap_targets(f: &FtBox, paths: &[PathInfo]) -> Vec<[f64; 2]> {
    let mut points = Vec::new();
    for i in 0..8 {
        let p = f.handle_local(i);
        let q = f.m.apply(gv2(p[0], p[1]));
        points.push([q.x, q.y]);
    }
    let c = [(f.b[0] + f.b[2]) / 2.0, (f.b[1] + f.b[3]) / 2.0];
    let c = f.m.apply(gv2(c[0], c[1]));
    points.push([c.x, c.y]);
    for path in paths {
        for v in &path.sp.vertices {
            let q = path.m.apply(gv2(v[0], v[1]));
            if q.x.is_finite() && q.y.is_finite() {
                points.push([q.x, q.y]);
            }
        }
    }
    points
}

/// Start a free-transform drag on the box: a handle scales about the opposite one (corners both
/// axes, edges one), inside moves, outside an edge rotates about the stationary pivot.
pub(crate) fn begin_ft_drag(f: &FtBox, press: Pos2, map: &ViewerMap, _mods: egui::Modifiers) -> Option<Drag> {
    let inv = f.m.inverse()?;
    let c = map.to_comp(press);
    let lp = inv.apply(gv2(c[0], c[1]));
    let lp = [lp.x, lp.y];
    let pc = f.m.apply(gv2(f.pivot[0], f.pivot[1]));
    if map.to_screen([pc.x, pc.y]).distance(press) < 7.0 {
        return Some(Drag::FtPivot { inv: Mat3::rotate_deg(f.angle) * inv, targets: pivot_snap_targets(f, &[]) });
    }
    if let Some(i) = (0..8).find(|i| f.handles[*i].distance(press) < 7.0) {
        let opp = [2, 3, 0, 1, 6, 7, 4, 5][i];
        let axes = match i {
            4 | 6 => [false, true],
            5 | 7 => [true, false],
            _ => [true, true],
        };
        return Some(Drag::Ft {
            paths: f.targets.clone(),
            comp_space: f.comp_space,
            angle: f.angle,
            mode: 1,
            anchor: f.pivot,
            opposite: f.handle_local(opp),
            previous: Mat3::IDENTITY,
            last: lp,
            start: c,
            applied: [0.0; 2],
            inv,
            axes,
        });
    }
    if point_in_quad(press, &f.quad) {
        return Some(Drag::Ft {
            paths: f.targets.clone(),
            comp_space: f.comp_space,
            angle: f.angle,
            mode: 0,
            anchor: f.pivot,
            opposite: f.pivot,
            previous: Mat3::IDENTITY,
            last: lp,
            start: c,
            applied: [0.0; 2],
            inv,
            axes: [true, true],
        });
    }
    if rotation_zone(f, press) {
        return Some(Drag::Ft {
            paths: f.targets.clone(),
            comp_space: f.comp_space,
            angle: f.angle,
            mode: 2,
            anchor: f.pivot,
            opposite: f.pivot,
            previous: Mat3::IDENTITY,
            last: lp,
            start: c,
            applied: [0.0; 2],
            inv,
            axes: [true, true],
        });
    }
    None
}

pub(crate) fn begin_ft_drag_with_paths(f: &FtBox, press: Pos2, map: &ViewerMap, mods: egui::Modifiers, paths: &[PathInfo]) -> Option<Drag> {
    match begin_ft_drag(f, press, map, mods)? {
        Drag::FtPivot { inv, .. } => Some(Drag::FtPivot { inv, targets: pivot_snap_targets(f, paths) }),
        other => Some(other),
    }
}

pub(crate) fn update_transform(
    app: &mut EffectcraftApp,
    ctx: &egui::Context,
    drag: &Drag,
    cpt: [f64; 2],
    mods: egui::Modifiers,
    merge: &str,
    zoom: f32,
) -> Option<Drag> {
    match drag.clone() {
        Drag::FtPivot { inv, targets } => {
            begin_transform_step(app, ctx, merge, "Move Free Transform Pivot");
            let mut point = cpt;
            if !mods.alt
                && let Some(q) = targets.iter().min_by(|a, b| ((a[0] - cpt[0]).hypot(a[1] - cpt[1])).total_cmp(&((b[0] - cpt[0]).hypot(b[1] - cpt[1]))))
                && (q[0] - cpt[0]).hypot(q[1] - cpt[1]) * zoom as f64 <= 8.0
            {
                point = *q;
            }
            if targets.get(8).is_some_and(|center| (center[0] - point[0]).hypot(center[1] - point[1]) * zoom as f64 <= 0.5) {
                app.free_transform.pivot = None;
            } else {
                let point = inv.apply(gv2(point[0], point[1]));
                set_pivot(app, ctx, [point.x, point.y]);
            }
            Some(Drag::FtPivot { inv, targets })
        }
        Drag::Ft { paths, comp_space, angle: basis_angle, mode, anchor, opposite, mut previous, last, start, mut applied, inv, axes } => {
            begin_transform_step(
                app,
                ctx,
                merge,
                match mode {
                    0 => "Move Mask Points",
                    1 => "Scale Mask Points",
                    _ => "Rotate Mask Points",
                },
            );
            let p = inv.apply(gv2(cpt[0], cpt[1]));
            let cur = [p.x, p.y];
            let press = inv.apply(gv2(start[0], start[1]));
            let targets: Vec<_> = paths.iter().map(|(l, u)| json!({"layer":l.0,"mask":u})).collect();
            let basis = Mat3::rotate_deg(basis_angle);
            let scale_anchor = if mode == 1 && !mods.command { opposite } else { anchor };
            let a = basis.apply(gv2(scale_anchor[0], scale_anchor[1]));
            let mut q = json!({"paths":targets,"space":if comp_space {"comp"} else {"path"},"anchor":[a.x,a.y],"basisAngle":basis_angle,"fromStart":true,"merge":merge});
            let desired = match mode {
                0 => {
                    let total = constrain_move([cpt[0] - start[0], cpt[1] - start[1]], mods.shift);
                    let delta = inv.apply_vec(gv2(total[0], total[1]));
                    let delta = basis.apply_vec(delta);
                    q["offset"] = json!([delta.x, delta.y]);
                    Mat3::translate(delta)
                }
                1 => {
                    let initial = [press.x, press.y];
                    let f = |k: usize| {
                        let denominator = initial[k] - scale_anchor[k];
                        if axes[k] && denominator.abs() > 1e-6 { ((cur[k] - scale_anchor[k]) / denominator).clamp(-50.0, 50.0) } else { 1.0 }
                    };
                    let (mut fx, mut fy) = (f(0), f(1));
                    if mods.shift && axes[0] && axes[1] {
                        let k = (fx + fy) / 2.0;
                        (fx, fy) = (k, k);
                    }
                    q["scale"] = json!([fx * 100.0, fy * 100.0]);
                    Mat3::translate(a) * basis * Mat3::scale(gv2(fx, fy)) * Mat3::rotate_deg(-basis_angle) * Mat3::translate(gv2(-a.x, -a.y))
                }
                _ => {
                    let a0 = (last[1] - anchor[1]).atan2(last[0] - anchor[0]);
                    let a1 = (cur[1] - anchor[1]).atan2(cur[0] - anchor[0]);
                    let delta = (a1 - a0 + std::f64::consts::PI).rem_euclid(std::f64::consts::TAU) - std::f64::consts::PI;
                    let raw = applied[0] + delta.to_degrees();
                    let rotation = if mods.shift { (raw / 45.0).round() * 45.0 } else { raw };
                    applied = [raw, rotation];
                    q["rotation"] = json!(rotation.rem_euclid(360.0));
                    Mat3::translate(a) * Mat3::rotate_deg(rotation) * Mat3::translate(gv2(-a.x, -a.y))
                }
            };
            let changed = desired.0.iter().flatten().zip(previous.0.iter().flatten()).any(|(a, b)| (a - b).abs() > 1e-12 * (1.0 + a.abs().max(b.abs())));
            if changed {
                match app.session.execute("path.freeTransform", q) {
                    Ok(_) => {
                        previous = desired;
                        if mode == 2 {
                            let angle = (basis_angle + applied[1]).rem_euclid(360.0);
                            app.free_transform.orientation = Some(BoxOrientation { vertices: app.session.state.selected_vertices.clone(), angle });
                        }
                    }
                    Err(e) => {
                        app.ui.status = e.to_string();
                        return None;
                    }
                }
            }
            Some(Drag::Ft { paths, comp_space, angle: basis_angle, mode, anchor, opposite, previous, last: cur, start, applied, inv, axes })
        }
        _ => None,
    }
}
#[cfg(test)]
mod tests {
    use super::super::viewer_overlays::update;
    use super::*;

    fn info(vertices: Vec<[f64; 2]>, closed: bool) -> PathInfo {
        let n = vertices.len();
        let sp = ShapePath { vertices, in_tangents: vec![[0.0; 2]; n], out_tangents: vec![[0.0; 2]; n], closed, feather: Vec::new() };
        PathInfo { layer: LayerId(3), uid: 7, m: Mat3::IDENTITY, sp, is_mask: true, feather: Vec::new() }
    }

    fn ft_app() -> (EffectcraftApp, egui::Context, ViewerMap, LayerId, u64) {
        ft_app_kind(false)
    }

    fn ft_app_kind(shape: bool) -> (EffectcraftApp, egui::Context, ViewerMap, LayerId, u64) {
        let mut s = effectcraft_engine::Session::default();
        s.execute("comp.new", json!({"width":200,"height":100,"duration":2})).unwrap();
        let (lid, uid) = if shape {
            let result = s.execute("shape.newPath", json!({"space":"path","vertices":[[30,30],[70,30],[70,70],[30,70]],"closed":true})).unwrap();
            let lid = result["layer"].as_u64().unwrap();
            let uid = result["path"].as_u64().unwrap();
            s.execute("prop.set", json!({"layer":lid,"path":"transform/position","value":[0,0]})).unwrap();
            s.execute("prop.select", json!({"layer":lid,"prop":uid})).unwrap();
            (lid, uid)
        } else {
            let lid = s.execute("layer.newSolid", json!({"width":200,"height":100})).unwrap()["layer"].as_u64().unwrap();
            let uid = s.execute("mask.new", json!({"layer":lid,"vertices":[[30,30],[70,30],[70,70],[30,70]],"closed":true})).unwrap()["mask"].as_u64().unwrap();
            (lid, uid)
        };
        let mut app = EffectcraftApp::new(s);
        let ctx = egui::Context::default();
        assert!(begin_selected_free_transform(&mut app, &ctx));
        let map = ViewerMap { origin: Pos2::ZERO, zoom: 1.0, comp: [200.0, 100.0], area: Rect::from_min_size(Pos2::ZERO, vec2(400.0, 200.0)) };
        (app, ctx, map, LayerId(lid), uid)
    }

    fn ft_box(app: &mut EffectcraftApp, ctx: &egui::Context, map: &ViewerMap, lid: LayerId, uid: u64) -> FtBox {
        let comp = app.session.active_comp().unwrap();
        let eval = EvalCtx::new(&app.session.project, app.session.active_comp_id().unwrap(), comp, app.session.time());
        let path = super::super::viewer_overlays::path_of(&eval, lid, uid).unwrap();
        let mut result = None;
        ctx.run_ui(Default::default(), |ui| {
            result = draw_free_transform(app, ctx, ui.painter(), map, std::slice::from_ref(&path));
        })
        .textures_delta
        .clear();
        result.unwrap()
    }

    fn first_vertex(app: &EffectcraftApp, lid: LayerId, uid: u64) -> [f64; 2] {
        app.session.active_comp().unwrap().layer(lid).unwrap().props.find_group(uid).unwrap().get("path").unwrap().value.as_path().unwrap().vertices[0]
    }

    #[test]
    fn native_menu_tracks_pivot_only_history_and_tool_exit() {
        for shape in [false, true] {
            let (mut app, ctx, _, _, _) = ft_app_kind(shape);
            let initial = crate::native_menu::state_key(&app);
            let document = app.session.project.clone();
            begin_transform_step(&mut app, &ctx, "pivot-menu", "Move Free Transform Pivot");
            set_pivot(&mut app, &ctx, [20.0, 15.0]);
            finish_transform_step(&mut app, &ctx);
            assert!(std::sync::Arc::ptr_eq(&document, &app.session.project));
            let moved = crate::native_menu::state_key(&app);
            assert_ne!(initial, moved);
            let spec = crate::native_menu::build(&app);
            assert!(spec.items().iter().any(|item| item.command == "edit.undo" && item.enabled && item.label.contains("Move Free Transform Pivot")));
            app.session.set_time(Tick::from_seconds_f64(0.5));
            let away = crate::native_menu::state_key(&app);
            assert_ne!(away, moved, "moving away hides the interactive Undo label");
            assert_eq!(transform_history_label(&app, false), None);
            app.session.set_time(Tick::from_seconds_f64(1.0));
            assert_eq!(crate::native_menu::state_key(&app), away, "the hidden label does not change again during playback");
            app.session.set_time(Tick::ZERO);
            assert_eq!(crate::native_menu::state_key(&app), moved, "returning restores the interactive Undo label");
            transform_undo(&mut app, &ctx, false).unwrap().unwrap();
            let undone = crate::native_menu::state_key(&app);
            assert_ne!(moved, undone);
            let spec = crate::native_menu::build(&app);
            assert!(spec.items().iter().any(|item| item.command == "edit.redo" && item.enabled && item.label.contains("Move Free Transform Pivot")));
            transform_undo(&mut app, &ctx, true).unwrap().unwrap();
            assert_eq!(crate::native_menu::state_key(&app), moved);
            end_free_transform(&mut app, &ctx);
            assert_ne!(crate::native_menu::state_key(&app), moved);
            assert_eq!(transform_history_label(&app, false), None);
            assert_eq!(transform_history_label(&app, true), None);
        }
    }

    #[test]
    fn free_transform_scale_modifiers_recompute_from_the_initial_box() {
        for shape in [false, true] {
            let (mut app, ctx, map, lid, uid) = ft_app_kind(shape);
            let f = ft_box(&mut app, &ctx, &map, lid, uid);
            let mut drag = begin_ft_drag(&f, egui::pos2(70.0, 70.0), &map, egui::Modifiers::NONE).unwrap();
            let command = egui::Modifiers { command: true, ..Default::default() };
            let both = egui::Modifiers { command: true, shift: true, ..Default::default() };
            let mut snap = |_: &EffectcraftApp, _: [f64; 2], _: LayerId| [0.0; 2];
            for (mods, first, last) in [
                (egui::Modifiers::NONE, [30.0, 30.0], [90.0, 80.0]),
                (egui::Modifiers::SHIFT, [30.0, 30.0], [85.0, 85.0]),
                (both, [15.0, 15.0], [85.0, 85.0]),
                (command, [10.0, 20.0], [90.0, 80.0]),
                (egui::Modifiers::NONE, [30.0, 30.0], [90.0, 80.0]),
            ] {
                let point = [90.0, 80.0];
                drag = update(
                    &mut app,
                    &drag,
                    crate::panels::viewer_overlays::OverlayFrame { ctx: &ctx, cpt: point, pos: map.to_screen(point), mods, merge: "modifier-start", zoom: 1.0 },
                    &mut snap,
                )
                .unwrap();
                let sp = app.session.active_comp().unwrap().layer(lid).unwrap().props.find_group(uid).unwrap().get("path").unwrap().value.as_path().unwrap();
                for (actual, expected) in [(sp.vertices[0], first), (sp.vertices[2], last)] {
                    assert!((actual[0] - expected[0]).abs() < 1e-8 && (actual[1] - expected[1]).abs() < 1e-8, "{mods:?}: {actual:?} != {expected:?}");
                }
            }
        }
    }

    #[test]
    fn free_transform_uniform_scale_does_not_depend_on_pointer_event_count() {
        let run = |steps| {
            let (mut app, ctx, map, lid, uid) = ft_app();
            let f = ft_box(&mut app, &ctx, &map, lid, uid);
            let mods = egui::Modifiers { command: true, shift: true, ..Default::default() };
            let mut drag = begin_ft_drag(&f, egui::pos2(70.0, 70.0), &map, mods).unwrap();
            let mut snap = |_: &EffectcraftApp, _: [f64; 2], _: LayerId| [0.0; 2];
            for i in 1..=steps {
                let p = [70.0 + 10.0 * i as f64 / steps as f64, 70.0 - 5.0 * i as f64 / steps as f64];
                drag = update(
                    &mut app,
                    &drag,
                    crate::panels::viewer_overlays::OverlayFrame { ctx: &ctx, cpt: p, pos: map.to_screen(p), mods, merge: "sampling", zoom: 1.0 },
                    &mut snap,
                )
                .unwrap();
            }
            first_vertex(&app, lid, uid)
        };
        let a = run(1);
        let b = run(8);
        assert!((a[0] - b[0]).abs() < 1e-9 && (a[1] - b[1]).abs() < 1e-9, "{a:?} vs {b:?}");
    }

    #[test]
    fn free_transform_scale_crosses_zero_and_returns_without_an_empty_undo_step() {
        for shape in [false, true] {
            let (mut app, ctx, map, lid, uid) = ft_app_kind(shape);
            app.session.mark_saved();
            let original = app.session.project.clone();
            let undo = app.session.history.undo.len();
            let f = ft_box(&mut app, &ctx, &map, lid, uid);
            let mut drag = begin_ft_drag(&f, egui::pos2(70.0, 70.0), &map, egui::Modifiers::NONE).unwrap();
            let mut snap = |_: &EffectcraftApp, _: [f64; 2], _: LayerId| [0.0; 2];
            for point in [[30.0, 50.0], [10.0, 10.0], [70.0, 70.0]] {
                drag = update(
                    &mut app,
                    &drag,
                    crate::panels::viewer_overlays::OverlayFrame {
                        ctx: &ctx,
                        cpt: point,
                        pos: map.to_screen(point),
                        mods: egui::Modifiers::NONE,
                        merge: "zero-return",
                        zoom: 1.0,
                    },
                    &mut snap,
                )
                .unwrap();
                let l = app.session.active_comp().unwrap().layer(lid).unwrap();
                let sp = l.props.find_group(uid).unwrap().get("path").unwrap().value.as_path().unwrap();
                assert!((sp.vertices[2][0] - point[0]).abs() < 1e-8 && (sp.vertices[2][1] - point[1]).abs() < 1e-8);
            }
            finish_transform_step(&mut app, &ctx);
            assert_eq!(app.session.project, original);
            assert_eq!(app.session.history.undo.len(), undo);
            assert!(app.free_transform.history.undo.is_empty());
        }
    }

    #[test]
    fn free_transform_undo_redo_interleaves_pivot_scale_rotation_and_move() {
        for shape in [false, true] {
            let (mut app, ctx, map, lid, uid) = ft_app_kind(shape);
            let original = app.session.project.clone();
            let original_undo = app.session.history.undo.len();
            let mut snap = |_: &EffectcraftApp, _: [f64; 2], _: LayerId| [0.0; 2];
            let f = ft_box(&mut app, &ctx, &map, lid, uid);
            let drag = begin_ft_drag(&f, pos2_for_test(50.0, 50.0), &map, egui::Modifiers::NONE).unwrap();
            update(
                &mut app,
                &drag,
                crate::panels::viewer_overlays::OverlayFrame {
                    ctx: &ctx,
                    cpt: [10.0, 20.0],
                    pos: pos2_for_test(10.0, 20.0),
                    mods: egui::Modifiers::ALT,
                    merge: "pivot",
                    zoom: 1.0,
                },
                &mut snap,
            )
            .unwrap();
            finish_transform_step(&mut app, &ctx);
            assert_eq!(app.session.history.undo.len(), original_undo, "UI-only pivot does not create a document edit");
            let f = ft_box(&mut app, &ctx, &map, lid, uid);
            let drag = begin_ft_drag(&f, f.handles[2], &map, egui::Modifiers::COMMAND).unwrap();
            update(
                &mut app,
                &drag,
                crate::panels::viewer_overlays::OverlayFrame {
                    ctx: &ctx,
                    cpt: [130.0, 120.0],
                    pos: pos2_for_test(130.0, 120.0),
                    mods: egui::Modifiers::COMMAND,
                    merge: "scale",
                    zoom: 1.0,
                },
                &mut snap,
            )
            .unwrap();
            finish_transform_step(&mut app, &ctx);
            let scaled = app.session.project.clone();
            let f = ft_box(&mut app, &ctx, &map, lid, uid);
            let drag = begin_ft_drag(&f, pos2_for_test(145.0, 80.0), &map, egui::Modifiers::NONE).unwrap();
            let p = gv2(135.0, 60.0).rotate_deg(45.0) + gv2(10.0, 20.0);
            update(
                &mut app,
                &drag,
                crate::panels::viewer_overlays::OverlayFrame {
                    ctx: &ctx,
                    cpt: [p.x, p.y],
                    pos: map.to_screen([p.x, p.y]),
                    mods: egui::Modifiers::NONE,
                    merge: "rotate",
                    zoom: 1.0,
                },
                &mut snap,
            )
            .unwrap();
            finish_transform_step(&mut app, &ctx);
            let rotated = app.session.project.clone();
            let f = ft_box(&mut app, &ctx, &map, lid, uid);
            let p = f.m.apply(gv2((f.b[0] + f.b[2]) / 2.0, (f.b[1] + f.b[3]) / 2.0));
            let drag = begin_ft_drag(&f, map.to_screen([p.x, p.y]), &map, egui::Modifiers::NONE).unwrap();
            update(
                &mut app,
                &drag,
                crate::panels::viewer_overlays::OverlayFrame {
                    ctx: &ctx,
                    cpt: [p.x + 10.0, p.y + 5.0],
                    pos: map.to_screen([p.x + 10.0, p.y + 5.0]),
                    mods: egui::Modifiers::NONE,
                    merge: "move",
                    zoom: 1.0,
                },
                &mut snap,
            )
            .unwrap();
            finish_transform_step(&mut app, &ctx);
            let moved = app.session.project.clone();
            for (project, angle, pivot) in
                [(&rotated, 45.0, Some([10.0, 20.0])), (&scaled, 0.0, Some([10.0, 20.0])), (&original, 0.0, Some([10.0, 20.0])), (&original, 0.0, None)]
            {
                crate::menus::invoke(&mut app, &ctx, "edit.undo", json!({})).unwrap();
                assert_eq!(&app.session.project, project);
                assert!((box_angle(&app, &ctx) - angle).abs() < 1e-8);
                assert_eq!(custom_pivot(&app, &ctx), pivot);
                assert!(free_transform_target(&app).is_some(), "Undo keeps the tool active");
            }
            assert_eq!(app.session.history.undo.len(), original_undo);
            let redo = crate::menus::menu_items(&app).into_iter().find(|e| e.id == "edit.redo").unwrap();
            assert!(redo.enabled && redo.label.contains("Pivot"), "UI-only redo is available");
            for (project, angle) in [(&original, 0.0), (&scaled, 0.0), (&rotated, 45.0), (&moved, 45.0)] {
                crate::menus::invoke(&mut app, &ctx, "edit.redo", json!({})).unwrap();
                assert_eq!(&app.session.project, project);
                assert!((box_angle(&app, &ctx) - angle).abs() < 1e-8);
                assert_eq!(custom_pivot(&app, &ctx), Some([10.0, 20.0]));
            }
            crate::menus::invoke(&mut app, &ctx, "edit.undo", json!({})).unwrap();
            let f = ft_box(&mut app, &ctx, &map, lid, uid);
            let drag = begin_ft_drag(&f, map.to_screen([10.0, 20.0]), &map, egui::Modifiers::NONE).unwrap();
            update(
                &mut app,
                &drag,
                crate::panels::viewer_overlays::OverlayFrame {
                    ctx: &ctx,
                    cpt: [20.0, 15.0],
                    pos: map.to_screen([20.0, 15.0]),
                    mods: egui::Modifiers::ALT,
                    merge: "new-future",
                    zoom: 1.0,
                },
                &mut snap,
            )
            .unwrap();
            finish_transform_step(&mut app, &ctx);
            assert!(app.free_transform.history.redo.is_empty());
            assert!(app.session.history.redo.is_empty(), "new pivot gesture abandons the old geometry future");
        }
    }

    #[test]
    fn free_transform_undo_ignores_noops_and_defers_to_external_document_edits() {
        let (mut app, ctx, map, lid, uid) = ft_app();
        let f = ft_box(&mut app, &ctx, &map, lid, uid);
        let mut snap = |_: &EffectcraftApp, _: [f64; 2], _: LayerId| [0.0; 2];
        let drag = begin_ft_drag(&f, pos2_for_test(50.0, 50.0), &map, egui::Modifiers::NONE).unwrap();
        update(
            &mut app,
            &drag,
            crate::panels::viewer_overlays::OverlayFrame {
                ctx: &ctx,
                cpt: [50.0, 50.0],
                pos: pos2_for_test(50.0, 50.0),
                mods: egui::Modifiers::NONE,
                merge: "noop",
                zoom: 1.0,
            },
            &mut snap,
        )
        .unwrap();
        finish_transform_step(&mut app, &ctx);
        assert!(app.free_transform.history.undo.is_empty());
        update(
            &mut app,
            &drag,
            crate::panels::viewer_overlays::OverlayFrame {
                ctx: &ctx,
                cpt: [10.0, 20.0],
                pos: pos2_for_test(10.0, 20.0),
                mods: egui::Modifiers::ALT,
                merge: "pivot",
                zoom: 1.0,
            },
            &mut snap,
        )
        .unwrap();
        finish_transform_step(&mut app, &ctx);
        let before = app.session.project.clone();
        crate::menus::invoke(&mut app, &ctx, "edit.undo", json!({})).unwrap();
        assert!(app.session.history.redo.is_empty(), "pivot Undo creates no document redo entry");
        assert!(crate::menus::menu_items(&app).iter().any(|e| e.id == "edit.redo" && e.enabled));
        crate::menus::invoke(&mut app, &ctx, "edit.redo", json!({})).unwrap();
        assert_eq!(custom_pivot(&app, &ctx), Some([10.0, 20.0]));
        app.session.execute("prop.set", json!({"layer":lid.0,"path":"transform/opacity","value":50})).unwrap();
        crate::menus::invoke(&mut app, &ctx, "edit.undo", json!({})).unwrap();
        assert_eq!(app.session.project, before, "ordinary document Undo wins after an unrelated edit");
        assert!(app.free_transform.history.undo.is_empty());
    }

    #[test]
    fn free_transform_manually_pinned_shared_pivot_follows_its_layer() {
        let (mut app, ctx, _, lid, uid) = ft_app();
        let second =
            app.session.execute("mask.new", json!({"layer":lid.0,"vertices":[[90,30],[100,30],[100,40]],"closed":true})).unwrap()["mask"].as_u64().unwrap();
        app.session.execute("mask.selectVertices", json!({"vertices":[{"layer":lid.0,"mask":uid,"index":0},{"layer":lid.0,"mask":second,"index":0}]})).unwrap();
        assert!(begin_selected_free_transform(&mut app, &ctx));
        assert_eq!(free_transform_target(&app).unwrap().len(), 2);
        set_pivot(&mut app, &ctx, [30.0, 40.0]);
        assert_eq!(custom_pivot(&app, &ctx), Some([30.0, 40.0]));
        app.session.execute("prop.set", json!({"layer":lid.0,"path":"transform/position","value":[200,100]})).unwrap();
        assert_eq!(custom_pivot(&app, &ctx), Some([130.0, 90.0]), "pivot follows layer translation, not comp coordinates");
        app.session.execute("prop.set", json!({"layer":lid.0,"path":"transform/rotation","value":90})).unwrap();
        let p = custom_pivot(&app, &ctx).unwrap();
        assert!((p[0] - 210.0).abs() < 1e-8 && (p[1] - 30.0).abs() < 1e-8, "pivot follows layer rotation: {p:?}");
    }

    #[test]
    fn free_transform_default_pivot_follows_box_until_manually_moved() {
        for shape in [false, true] {
            let (mut app, ctx, map, lid, uid) = ft_app_kind(shape);
            let f = ft_box(&mut app, &ctx, &map, lid, uid);
            assert_eq!(f.pivot, [50.0, 50.0]);
            assert_eq!(custom_pivot(&app, &ctx), None, "drawing the default pivot must not pin it");
            let mut snap = |_: &EffectcraftApp, _: [f64; 2], _: LayerId| [0.0; 2];
            let drag = begin_ft_drag(&f, pos2_for_test(40.0, 40.0), &map, egui::Modifiers::NONE).unwrap();
            update(
                &mut app,
                &drag,
                crate::panels::viewer_overlays::OverlayFrame {
                    ctx: &ctx,
                    cpt: [60.0, 50.0],
                    pos: pos2_for_test(60.0, 50.0),
                    mods: egui::Modifiers::NONE,
                    merge: "move",
                    zoom: 1.0,
                },
                &mut snap,
            )
            .unwrap();
            let f = ft_box(&mut app, &ctx, &map, lid, uid);
            assert_eq!(f.pivot, [70.0, 60.0]);
            let drag = begin_ft_drag(&f, f.handles[2], &map, egui::Modifiers::NONE).unwrap();
            update(
                &mut app,
                &drag,
                crate::panels::viewer_overlays::OverlayFrame {
                    ctx: &ctx,
                    cpt: [110.0, 100.0],
                    pos: pos2_for_test(110.0, 100.0),
                    mods: egui::Modifiers::NONE,
                    merge: "scale",
                    zoom: 1.0,
                },
                &mut snap,
            )
            .unwrap();
            let f = ft_box(&mut app, &ctx, &map, lid, uid);
            assert_eq!(f.pivot, [80.0, 70.0]);
            assert_eq!(custom_pivot(&app, &ctx), None);
            let drag = begin_ft_drag(&f, pos2_for_test(80.0, 70.0), &map, egui::Modifiers::NONE).unwrap();
            update(
                &mut app,
                &drag,
                crate::panels::viewer_overlays::OverlayFrame {
                    ctx: &ctx,
                    cpt: [15.0, 20.0],
                    pos: pos2_for_test(15.0, 20.0),
                    mods: egui::Modifiers::ALT,
                    merge: "pivot",
                    zoom: 1.0,
                },
                &mut snap,
            )
            .unwrap();
            let f = ft_box(&mut app, &ctx, &map, lid, uid);
            let drag = begin_ft_drag(&f, pos2_for_test(70.0, 60.0), &map, egui::Modifiers::NONE).unwrap();
            update(
                &mut app,
                &drag,
                crate::panels::viewer_overlays::OverlayFrame {
                    ctx: &ctx,
                    cpt: [90.0, 70.0],
                    pos: pos2_for_test(90.0, 70.0),
                    mods: egui::Modifiers::NONE,
                    merge: "move-pinned",
                    zoom: 1.0,
                },
                &mut snap,
            )
            .unwrap();
            let f = ft_box(&mut app, &ctx, &map, lid, uid);
            assert_eq!(f.pivot, [15.0, 20.0]);
            let drag = begin_ft_drag(&f, pos2_for_test(15.0, 20.0), &map, egui::Modifiers::NONE).unwrap();
            update(
                &mut app,
                &drag,
                crate::panels::viewer_overlays::OverlayFrame {
                    ctx: &ctx,
                    cpt: [101.0, 81.0],
                    pos: pos2_for_test(101.0, 81.0),
                    mods: egui::Modifiers::NONE,
                    merge: "return-center",
                    zoom: 1.0,
                },
                &mut snap,
            )
            .unwrap();
            let f = ft_box(&mut app, &ctx, &map, lid, uid);
            assert_eq!(f.pivot, [100.0, 80.0], "returning near the center snaps back to automatic");
            assert_eq!(custom_pivot(&app, &ctx), None);
            let drag = begin_ft_drag(&f, pos2_for_test(90.0, 70.0), &map, egui::Modifiers::NONE).unwrap();
            update(
                &mut app,
                &drag,
                crate::panels::viewer_overlays::OverlayFrame {
                    ctx: &ctx,
                    cpt: [110.0, 80.0],
                    pos: pos2_for_test(110.0, 80.0),
                    mods: egui::Modifiers::NONE,
                    merge: "move-again",
                    zoom: 1.0,
                },
                &mut snap,
            )
            .unwrap();
            assert_eq!(ft_box(&mut app, &ctx, &map, lid, uid).pivot, [120.0, 90.0]);
            end_free_transform(&mut app, &ctx);
            assert!(begin_selected_free_transform(&mut app, &ctx));
            assert_eq!(ft_box(&mut app, &ctx, &map, lid, uid).pivot, [120.0, 90.0]);
            assert_eq!(custom_pivot(&app, &ctx), None);
        }
    }

    #[test]
    fn free_transform_keeps_rotated_axes_and_stationary_pivot_between_gestures() {
        for shape in [false, true] {
            let (mut app, ctx, map, lid, uid) = ft_app_kind(shape);
            set_pivot(&mut app, &ctx, [10.0, 20.0]);
            let f = ft_box(&mut app, &ctx, &map, lid, uid);
            let mut snap = |_: &EffectcraftApp, _: [f64; 2], _: LayerId| [0.0; 2];
            let start = [80.0, 50.0];
            let drag = begin_ft_drag(&f, pos2_for_test(80.0, 50.0), &map, egui::Modifiers::NONE).unwrap();
            let p = gv2(start[0] - 10.0, start[1] - 20.0).rotate_deg(45.0) + gv2(10.0, 20.0);
            update(
                &mut app,
                &drag,
                crate::panels::viewer_overlays::OverlayFrame {
                    ctx: &ctx,
                    cpt: [p.x, p.y],
                    pos: map.to_screen([p.x, p.y]),
                    mods: egui::Modifiers::NONE,
                    merge: "rotate",
                    zoom: 1.0,
                },
                &mut snap,
            )
            .unwrap();
            let f = ft_box(&mut app, &ctx, &map, lid, uid);
            assert!((f.angle - 45.0).abs() < 1e-8);
            assert!((f.quad[1].y - f.quad[0].y).abs() > 20.0, "the box follows the rotation");
            assert_eq!(custom_pivot(&app, &ctx), Some([10.0, 20.0]));
            let corner = f.handle_local(2);
            let scaled = [f.pivot[0] + 2.0 * (corner[0] - f.pivot[0]), corner[1]];
            let p = f.m.apply(gv2(scaled[0], scaled[1]));
            let drag = begin_ft_drag(&f, f.handles[2], &map, egui::Modifiers::COMMAND).unwrap();
            update(
                &mut app,
                &drag,
                crate::panels::viewer_overlays::OverlayFrame {
                    ctx: &ctx,
                    cpt: [p.x, p.y],
                    pos: map.to_screen([p.x, p.y]),
                    mods: egui::Modifiers::COMMAND,
                    merge: "scale",
                    zoom: 1.0,
                },
                &mut snap,
            )
            .unwrap();
            let v = first_vertex(&app, lid, uid);
            let expected = gv2(40.0, 10.0).rotate_deg(45.0) + gv2(10.0, 20.0);
            assert!((v[0] - expected.x).abs() < 1e-8 && (v[1] - expected.y).abs() < 1e-8, "scale uses the rotated axes: {v:?}");
            let f = ft_box(&mut app, &ctx, &map, lid, uid);
            let expected_pivot = f.m.apply(gv2(f.pivot[0], f.pivot[1]));
            assert!((expected_pivot.x - 10.0).abs() < 1e-8 && (expected_pivot.y - 20.0).abs() < 1e-8);
            assert!((f.angle - 45.0).abs() < 1e-8, "scale does not reset orientation");
            // An ordinary corner scale is still about its opposite corner, but does not drag the pivot.
            let drag = begin_ft_drag(&f, f.handles[2], &map, egui::Modifiers::NONE).unwrap();
            let p = f.m.apply(gv2(f.b[2] + 10.0, f.b[3] + 5.0));
            update(
                &mut app,
                &drag,
                crate::panels::viewer_overlays::OverlayFrame {
                    ctx: &ctx,
                    cpt: [p.x, p.y],
                    pos: map.to_screen([p.x, p.y]),
                    mods: egui::Modifiers::NONE,
                    merge: "scale-opposite",
                    zoom: 1.0,
                },
                &mut snap,
            )
            .unwrap();
            assert_eq!(custom_pivot(&app, &ctx), Some([10.0, 20.0]));
            assert_eq!(ft_cursor(&f, map.to_screen([10.0, 20.0]), &map), Some(egui::CursorIcon::Default));
            end_free_transform(&mut app, &ctx);
            assert_eq!(box_angle(&app, &ctx), 0.0);
            assert_eq!(custom_pivot(&app, &ctx), None);
        }
    }

    #[test]
    fn free_transform_edge_rotation_snaps_the_total_angle_and_releases_shift() {
        for shape in [false, true] {
            let (mut app, ctx, map, lid, uid) = ft_app_kind(shape);
            let original = app.session.project.clone();
            let f = ft_box(&mut app, &ctx, &map, lid, uid);
            let drag = begin_ft_drag(&f, egui::pos2(80.0, 50.0), &map, egui::Modifiers::NONE).unwrap();
            assert!(matches!(drag, Drag::Ft { mode: 2, .. }), "outside the middle of any edge rotates");
            let mut snap = |_: &EffectcraftApp, _: [f64; 2], _: LayerId| [0.0; 2];
            let point = [50.0 + 30.0 * 30.0_f64.to_radians().cos(), 65.0];
            let drag = update(
                &mut app,
                &drag,
                crate::panels::viewer_overlays::OverlayFrame {
                    ctx: &ctx,
                    cpt: point,
                    pos: map.to_screen(point),
                    mods: egui::Modifiers::SHIFT,
                    merge: "rotate45",
                    zoom: 1.0,
                },
                &mut snap,
            )
            .unwrap();
            let v = first_vertex(&app, lid, uid);
            assert!((v[0] - 50.0).abs() < 1e-8 && (v[1] - (50.0 - 20.0 * 2.0_f64.sqrt())).abs() < 1e-8, "{v:?}");
            update(
                &mut app,
                &drag,
                crate::panels::viewer_overlays::OverlayFrame {
                    ctx: &ctx,
                    cpt: point,
                    pos: map.to_screen(point),
                    mods: egui::Modifiers::NONE,
                    merge: "rotate45",
                    zoom: 1.0,
                },
                &mut snap,
            );
            let v = first_vertex(&app, lid, uid);
            assert!((v[0] - 42.6794919243).abs() < 1e-7, "{v:?}");
            app.session.execute("edit.undo", json!({})).unwrap();
            assert_eq!(app.session.project, original);
        }
    }

    #[test]
    fn free_transform_pivot_snaps_to_box_and_other_path_vertices() {
        let (mut app, ctx, map, lid, uid) = ft_app();
        let f = ft_box(&mut app, &ctx, &map, lid, uid);
        let path = info(vec![[12.0, 23.0], [80.0, 90.0]], false);
        let drag = begin_ft_drag_with_paths(&f, egui::pos2(50.0, 50.0), &map, egui::Modifiers::NONE, &[path]).unwrap();
        let original = app.session.project.clone();
        let mut snap = |_: &EffectcraftApp, _: [f64; 2], _: LayerId| [0.0; 2];
        let drag = update(
            &mut app,
            &drag,
            crate::panels::viewer_overlays::OverlayFrame {
                ctx: &ctx,
                cpt: [14.0, 24.0],
                pos: egui::pos2(14.0, 24.0),
                mods: egui::Modifiers::NONE,
                merge: "pivot",
                zoom: 1.0,
            },
            &mut snap,
        )
        .unwrap();
        assert_eq!(custom_pivot(&app, &ctx), Some([12.0, 23.0]));
        let drag = update(
            &mut app,
            &drag,
            crate::panels::viewer_overlays::OverlayFrame {
                ctx: &ctx,
                cpt: [31.0, 48.0],
                pos: egui::pos2(31.0, 48.0),
                mods: egui::Modifiers::NONE,
                merge: "pivot",
                zoom: 1.0,
            },
            &mut snap,
        )
        .unwrap();
        assert_eq!(custom_pivot(&app, &ctx), Some([30.0, 50.0]));
        update(
            &mut app,
            &drag,
            crate::panels::viewer_overlays::OverlayFrame {
                ctx: &ctx,
                cpt: [31.0, 48.0],
                pos: egui::pos2(31.0, 48.0),
                mods: egui::Modifiers::ALT,
                merge: "pivot",
                zoom: 1.0,
            },
            &mut snap,
        );
        assert_eq!(custom_pivot(&app, &ctx), Some([31.0, 48.0]));
        assert_eq!(app.session.project, original);
    }

    #[test]
    fn free_transform_pivot_moves_without_editing_and_controls_scale_and_rotation() {
        for shape in [false, true] {
            let (mut app, ctx, map, lid, uid) = ft_app_kind(shape);
            let original = app.session.project.clone();
            let undo = app.session.history.undo.len();
            let f = ft_box(&mut app, &ctx, &map, lid, uid);
            let mut snap = |_: &EffectcraftApp, _: [f64; 2], _: LayerId| [0.0; 2];
            let drag = begin_ft_drag(&f, pos2_for_test(50.0, 50.0), &map, egui::Modifiers::NONE).unwrap();
            assert!(matches!(drag, Drag::FtPivot { .. }));
            update(
                &mut app,
                &drag,
                crate::panels::viewer_overlays::OverlayFrame {
                    ctx: &ctx,
                    cpt: [10.0, 20.0],
                    pos: pos2_for_test(10.0, 20.0),
                    mods: egui::Modifiers::NONE,
                    merge: "pivot",
                    zoom: 1.0,
                },
                &mut snap,
            );
            assert_eq!(app.session.project, original);
            assert_eq!(app.session.history.undo.len(), undo);
            let f = ft_box(&mut app, &ctx, &map, lid, uid);
            assert_eq!(f.pivot, [10.0, 20.0]);
            let drag = begin_ft_drag(&f, f.handles[2], &map, egui::Modifiers::COMMAND).unwrap();
            update(
                &mut app,
                &drag,
                crate::panels::viewer_overlays::OverlayFrame {
                    ctx: &ctx,
                    cpt: [130.0, 120.0],
                    pos: pos2_for_test(130.0, 120.0),
                    mods: egui::Modifiers::COMMAND,
                    merge: "pivot-scale",
                    zoom: 1.0,
                },
                &mut snap,
            );
            assert_eq!(first_vertex(&app, lid, uid), [50.0, 40.0]);
            app.session.execute("edit.undo", json!({})).unwrap();
            let f = ft_box(&mut app, &ctx, &map, lid, uid);
            let drag = begin_ft_drag(&f, pos2_for_test(75.0, 25.0), &map, egui::Modifiers::NONE).unwrap();
            assert!(matches!(drag, Drag::Ft { mode: 2, .. }));
            update(
                &mut app,
                &drag,
                crate::panels::viewer_overlays::OverlayFrame {
                    ctx: &ctx,
                    cpt: [5.0, 85.0],
                    pos: pos2_for_test(5.0, 85.0),
                    mods: egui::Modifiers::NONE,
                    merge: "pivot-rotate",
                    zoom: 1.0,
                },
                &mut snap,
            );
            let v = first_vertex(&app, lid, uid);
            assert!(v[0].abs() < 1e-8 && (v[1] - 40.0).abs() < 1e-8, "{v:?}");
            app.session.execute("edit.undo", json!({})).unwrap();
            assert_eq!(app.session.project, original);
            end_free_transform(&mut app, &ctx);
            assert_eq!(custom_pivot(&app, &ctx), None);
        }
    }

    fn pos2_for_test(x: f32, y: f32) -> Pos2 {
        egui::pos2(x, y)
    }

    #[test]
    fn free_transform_shift_move_uses_comp_axes_and_updates_from_gesture_start() {
        for shape in [false, true] {
            let (mut app, ctx, map, lid, uid) = ft_app_kind(shape);
            app.session.execute("prop.set", json!({"layer":lid.0,"path":"transform/rotation","value":90})).unwrap();
            let original = app.session.project.clone();
            let f = ft_box(&mut app, &ctx, &map, lid, uid);
            let press = f.m.apply(gv2(40.0, 40.0));
            let drag = begin_ft_drag(&f, map.to_screen([press.x, press.y]), &map, egui::Modifiers::NONE).unwrap();
            assert!(matches!(drag, Drag::Ft { mode: 0, .. }));
            let mut snap = |_: &EffectcraftApp, _: [f64; 2], _: LayerId| [0.0; 2];
            let mut drag = drag;
            for (delta, mods, expected) in [
                ([30.0, 8.0], egui::Modifiers::SHIFT, [30.0, 0.0]),
                ([40.0, 15.0], egui::Modifiers::SHIFT, [30.0, -10.0]),
                ([40.0, 15.0], egui::Modifiers::NONE, [45.0, -10.0]),
            ] {
                let point = [press.x + delta[0], press.y + delta[1]];
                drag = update(
                    &mut app,
                    &drag,
                    crate::panels::viewer_overlays::OverlayFrame { ctx: &ctx, cpt: point, pos: map.to_screen(point), mods, merge: "shift-ft", zoom: 1.0 },
                    &mut snap,
                )
                .unwrap();
                let v = first_vertex(&app, lid, uid);
                assert!((v[0] - expected[0]).abs() < 1e-8 && (v[1] - expected[1]).abs() < 1e-8, "{v:?} {expected:?}");
            }
            app.session.execute("edit.undo", json!({})).unwrap();
            assert_eq!(app.session.project, original);
            assert_eq!(constrain_move([4.0, 30.0], true), [0.0, 30.0]);
        }
    }

    #[test]
    fn shared_transform_preserves_shape_group_placement_and_unselected_points() {
        let (mut app, ctx, _, lid, shape) = ft_app_kind(true);
        let comp = app.session.active_comp().unwrap();
        let path_parent = comp.layer(lid).unwrap().props.parent_of(shape).unwrap().uid;
        let group = comp.layer(lid).unwrap().props.parent_of(path_parent).unwrap();
        let position = group.sub("transform").unwrap().get("position").unwrap().uid;
        let rotation = group.sub("transform").unwrap().get("rotation").unwrap().uid;
        app.session.execute("prop.set", json!({"layer":lid.0,"prop":position,"value":[25,15]})).unwrap();
        app.session.execute("prop.set", json!({"layer":lid.0,"prop":rotation,"value":35})).unwrap();
        let mask =
            app.session.execute("mask.new", json!({"layer":lid.0,"vertices":[[10,10],[90,10],[90,90]],"closed":true})).unwrap()["mask"].as_u64().unwrap();
        app.session.execute("mask.selectVertices", json!({"vertices":[{"layer":lid.0,"mask":shape,"index":0},{"layer":lid.0,"mask":mask,"index":1}]})).unwrap();
        assert!(begin_selected_free_transform(&mut app, &ctx));
        assert_eq!(free_transform_target(&app).unwrap().len(), 2);
        let read = |app: &EffectcraftApp, uid| {
            let comp = app.session.active_comp().unwrap();
            let eval = EvalCtx::new(&app.session.project, app.session.active_comp_id().unwrap(), comp, app.session.time());
            let p = super::super::viewer_overlays::path_of(&eval, lid, uid).unwrap();
            p.sp.vertices.iter().map(|v| p.m.apply(gv2(v[0], v[1]))).collect::<Vec<_>>()
        };
        let before_shape = read(&app, shape);
        let before_mask = read(&app, mask);
        let project = app.session.project.clone();
        app.session
            .execute(
                "path.freeTransform",
                json!({"paths":[{"layer":lid.0,"mask":shape},{"layer":lid.0,"mask":mask}],"space":"comp","anchor":[20,30],"rotation":90,"scale":[150,75]}),
            )
            .unwrap();
        for (uid, before, selected) in [(shape, before_shape, 0), (mask, before_mask, 1)] {
            for (i, (before, after)) in before.into_iter().zip(read(&app, uid)).enumerate() {
                let expected = if i == selected { gv2(20.0 - (before.y - 30.0) * 0.75, 30.0 + (before.x - 20.0) * 1.5) } else { before };
                assert!((after - expected).length() < 1e-8, "path {uid}, vertex {i}: {after:?} != {expected:?}");
            }
        }
        app.session.execute("edit.undo", json!({})).unwrap();
        assert_eq!(app.session.project, project);
    }

    #[test]
    fn free_transform_bezier_bounds_use_curve_extrema_not_controls() {
        let mut p = info(vec![[0.0, 0.0], [100.0, 0.0]], false).sp;
        p.out_tangents[0] = [0.0, 100.0];
        p.in_tangents[1] = [0.0, 100.0];
        let b = selected_path_bounds(&p, [0, 1], &Mat3::IDENTITY).unwrap();
        assert_eq!(b, kurbo::Rect::new(0.0, 0.0, 100.0, 75.0), "the curve peaks at 75; vertices at 0 and controls at 100 are both wrong");
        let matrix = Mat3::translate(gv2(200.0, 300.0)) * Mat3::rotate_deg(90.0);
        let b = selected_path_bounds(&p, [0, 1], &matrix).unwrap();
        for (actual, expected) in [b.x0, b.y0, b.x1, b.y1].into_iter().zip([125.0, 300.0, 200.0, 400.0]) {
            assert!((actual - expected).abs() < 1e-8);
        }
    }

    #[test]
    fn free_transform_bezier_bounds_include_the_closing_curve() {
        let mut p = info(vec![[0.0, 0.0], [100.0, 0.0], [100.0, 100.0], [0.0, 100.0]], true).sp;
        p.out_tangents[3] = [-100.0, 0.0];
        p.in_tangents[0] = [-100.0, 0.0];
        assert_eq!(selected_path_bounds(&p, [0, 1, 2, 3], &Mat3::IDENTITY), Some(kurbo::Rect::new(-75.0, 0.0, 100.0, 100.0)));
        p.closed = false;
        assert_eq!(selected_path_bounds(&p, [0, 1, 2, 3], &Mat3::IDENTITY), Some(kurbo::Rect::new(0.0, 0.0, 100.0, 100.0)));
    }

    #[test]
    fn free_transform_bezier_bounds_preserve_vertex_subsets() {
        let mut p = info(vec![[0.0, 0.0], [100.0, 0.0], [1000.0, 1000.0]], false).sp;
        p.out_tangents[0] = [0.0, 100.0];
        p.in_tangents[1] = [0.0, 100.0];
        assert_eq!(selected_path_bounds(&p, [0, 1], &Mat3::IDENTITY), Some(kurbo::Rect::new(0.0, 0.0, 100.0, 75.0)));
        assert_eq!(selected_path_bounds(&p, [0], &Mat3::IDENTITY), Some(kurbo::Rect::new(0.0, 0.0, 0.0, 0.0)));
        assert!(selected_path_bounds(&p, [usize::MAX], &Mat3::IDENTITY).is_none());
    }
}

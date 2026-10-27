use super::geometry::{cam_state, l2c, parent_inverse};
use super::gesture::{Gesture, pen_id};
use super::*;

/// The position change per comp pixel of drag (x, y) that moves a layer by `dir` when the
/// pointer covers the axis' on-screen vector `s` (comp px); drag across the axis is ignored.
pub(super) fn axis_drag(dir: [f64; 3], s: [f64; 2]) -> ([f64; 3], [f64; 3]) {
    let n = (s[0] * s[0] + s[1] * s[1]).max(1e-12);
    (dir.map(|v| v * s[0] / n), dir.map(|v| v * s[1] / n))
}

/// The gizmo arrow under screen point `p`.
pub(super) fn axis_at(hits: &[(LayerId, [Pos2; 2], [f64; 3], [f64; 3])], p: Pos2) -> Option<(LayerId, [Pos2; 2], [f64; 3], [f64; 3])> {
    let dist = |[a, b]: [Pos2; 2]| {
        let ab = b - a;
        let t = ((p - a).dot(ab) / ab.length_sq().max(1e-6)).clamp(0.0, 1.0);
        p.distance(a + ab * t)
    };
    hits.iter().filter(|h| dist(h.1) < 6.0).min_by(|a, b| dist(a.1).total_cmp(&dist(b.1))).copied()
}

/// World to parent space for a 3D layer (identity without a parent).
pub(super) fn parent_world_inverse(ctx: &EvalCtx, l: &Layer) -> effectcraft_engine::geom::Mat4 {
    match l.parent.and_then(|p| ctx.comp.layer(p)) {
        Some(p) => ctx.world_matrix(p).inverse().unwrap_or_default(),
        None => effectcraft_engine::geom::Mat4::IDENTITY,
    }
}

/// Parent-space position change per comp pixel of drag (x and y) for a layer: 2D layers map
/// through the parent transform; 3D layers move in the view plane at their depth.
pub(super) fn drag_axes(ctx: &EvalCtx, l: &Layer) -> ([f64; 3], [f64; 3]) {
    if !l.is_3d() {
        let inv = parent_inverse(ctx, l);
        let a = inv.apply_vec(gv2(1.0, 0.0));
        let b = inv.apply_vec(gv2(0.0, 1.0));
        return ([a.x, a.y, 0.0], [b.x, b.y, 0.0]);
    }
    let cam = cam_state(ctx);
    let anchor = l.transform().map(|tr| ctx.v3(l, tr, "anchor", [0.0; 3])).unwrap_or([0.0; 3]);
    let wp = ctx.world_matrix(l).apply(anchor.into());
    let k = if cam.ortho { 1.0 / cam.zoom.max(1e-9) } else { cam.depth(wp).max(1.0) / cam.zoom.max(1e-9) };
    let pinv = parent_world_inverse(ctx, l);
    let a = pinv.apply_vec(cam.right() * k);
    let b = pinv.apply_vec(cam.down() * k);
    ([a.x, a.y, a.z], [b.x, b.y, b.z])
}

/// Wireframes of cameras (frustum, point of interest) and lights (position, direction) as seen
/// through the current 3D view.
/// Camera and light wireframes seen through `cam` (`through_active`: `cam` is the active camera,
/// which is then not drawn). `pane` names the automation ids: `None` for the main view
/// (`viewer.camera.<id>`), `Some(view)` for a view of a 2- or 4-view layout
/// (`viewer.view.<view>.camera.<id>`).
#[allow(clippy::too_many_arguments)]
pub(super) fn draw_rigs(
    app: &mut EffectcraftApp,
    painter: &egui::Painter,
    map: &ViewerMap,
    ectx: &EvalCtx,
    selected: &[LayerId],
    cam: CameraState,
    through_active: bool,
    pane: Option<&str>,
) {
    use effectcraft_engine::geom::Vec3;
    use effectcraft_engine::project::{LayerSource, LightKind};
    let (w, h) = (ectx.comp.width as f64, ectx.comp.height as f64);
    let active = ectx.comp.active_camera(ectx.time).map(|c| c.id);
    let prefix = pane.map(|v| format!("viewer.view.{v}.")).unwrap_or_else(|| "viewer.".to_string());
    let proj = |p: Vec3| cam.project(w, h, p).map(|v| map.to_screen([v.x, v.y]));
    let seg = |a: Vec3, b: Vec3, stroke: Stroke| {
        if let (Some(x), Some(y)) = (proj(a), proj(b)) {
            painter.line_segment([x, y], stroke);
        }
    };
    for l in ectx.comp.layers.iter().filter(|l| (l.is_camera() || l.is_light()) && l.is_active_at(ectx.time)) {
        let sel = selected.contains(&l.id);
        let col = app.tokens.label(l.label).gamma_multiply(if sel { 1.0 } else { 0.6 });
        let stroke = Stroke::new(if sel { 1.5 } else { 1.0 }, col);
        let (eye, fwd, down) = three_d::camera::layer_frame(ectx, l);
        let poi = |l: &Layer| -> Option<Vec3> {
            let pr = l.transform()?.get("poi").filter(|_| three_d::camera::is_two_node(l))?;
            Some(three_d::camera::parent_world(ectx, l).apply(ectx.value(l, pr).as_vec3().into()))
        };
        if l.is_camera() {
            if through_active && Some(l.id) == active {
                continue;
            }
            let zoom = l.props.prop("cameraOptions/zoom").map(|p| ectx.value(l, p).as_f64()).unwrap_or(1000.0);
            let right = down.cross(fwd);
            // Frustum to a plane at a quarter of the zoom distance.
            let k = 0.25;
            let c = eye + fwd * (zoom * k);
            let corners = [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)].map(|(sx, sy)| c + right * (sx * w * 0.5 * k) + down * (sy * h * 0.5 * k));
            for i in 0..4 {
                seg(eye, corners[i], stroke);
                seg(corners[i], corners[(i + 1) % 4], stroke);
            }
            if let Some(poi) = poi(l) {
                seg(eye, poi, Stroke::new(1.0, col.gamma_multiply(0.6)));
                if let Some(s) = proj(poi) {
                    painter.circle_stroke(s, 4.0, stroke);
                }
            }
            if let Some(s) = proj(eye) {
                app.auto.add(&format!("{prefix}camera.{}", l.id.0), Rect::from_center_size(s, vec2(14.0, 14.0)), &l.name);
            }
        } else {
            let LayerSource::Light { kind } = l.source else { continue };
            if kind == LightKind::Ambient {
                continue;
            }
            let Some(s) = proj(eye) else { continue };
            painter.circle_stroke(s, 6.0, stroke);
            for i in 0..8 {
                let a = i as f32 * std::f32::consts::FRAC_PI_4;
                let d = vec2(a.cos(), a.sin());
                painter.line_segment([s + d * 8.0, s + d * 12.0], stroke);
            }
            if matches!(kind, LightKind::Spot | LightKind::Parallel) {
                let target = poi(l).unwrap_or(eye + fwd * 300.0);
                seg(eye, target, Stroke::new(1.0, col.gamma_multiply(0.7)));
                if let Some(q) = proj(target) {
                    painter.circle_stroke(q, 3.0, stroke);
                }
            }
            app.auto.add(&format!("{prefix}light.{}", l.id.0), Rect::from_center_size(s, vec2(16.0, 16.0)), &l.name);
        }
    }
}

/// Pen tool press at `pos`: continue the path in progress (pressing its first vertex closes it),
/// resume an open path whose last vertex is pressed, add a vertex on a visible path's segment, or
/// start a new path: a mask on the selected footage/solid layer (or shape layer, with Tool Creates
/// Mask), a shape path (Shape n group with the Tools bar's Fill and Stroke) on the selected shape
/// layer, or a new shape layer when nothing is selected. The placed point snaps.
/// Add a Position keyframe at a motion path's frame, keeping the path where it is.
pub(super) fn add_motion_key(app: &mut EffectcraftApp, f: ov::PathFrame) {
    if let Err(e) = app.session.execute("prop.addKey", json!({"layer": f.layer.0, "prop": f.prop, "time": f.time.seconds()})) {
        app.ui.status = e.to_string();
    }
}

/// Select only the motion path's keyframe `k` (what the vertex tools act on).
pub(super) fn select_motion_key(app: &mut EffectcraftApp, k: &ov::KeyHit) -> bool {
    app.session.execute("keys.select", json!({"keys": [{"layer": k.layer.0, "prop": k.prop, "time": k.time.seconds()}]})).is_ok()
}

/// The pointer for a Pen slot tool: the tool's own pen, the Pen showing Add Vertex's over a path
/// it would add a point to (After Effects).
pub(super) fn pen_cursor_icon(tool: Tool, over_path: bool) -> crate::icons::Icon {
    use crate::icons::Icon;
    match tool {
        Tool::Pen if over_path => Icon::PenAdd,
        Tool::PenAdd => Icon::PenAdd,
        Tool::PenDelete => Icon::PenDelete,
        Tool::PenConvert => Icon::PenConvert,
        Tool::MaskFeather => Icon::MaskFeather,
        _ => Icon::Pen,
    }
}

/// Paint pen pointer `icon` with its tip at `at`, outlined for any background.
pub(super) fn pen_cursor(ctx: &egui::Context, at: Pos2, icon: crate::icons::Icon) {
    use crate::icons::Icon;
    // Where each glyph's tip sits in its 16 × 16 box.
    let tip = match icon {
        Icon::PenAdd | Icon::PenDelete | Icon::MaskFeather => vec2(6.0, 2.5),
        Icon::PenConvert => vec2(8.0, 3.5),
        _ => vec2(8.0, 1.8),
    };
    let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::unique("viewer-pen-cursor")));
    let rect = Rect::from_min_size(at - tip, vec2(16.0, 16.0));
    for d in [vec2(-1.0, 0.0), vec2(1.0, 0.0), vec2(0.0, -1.0), vec2(0.0, 1.0)] {
        crate::icons::paint(&painter, rect.translate(d), icon, Color32::BLACK);
    }
    crate::icons::paint(&painter, rect, icon, Color32::WHITE);
}

/// Handle a Pen-tool press at `pos`: close the current path when its first vertex is hit; otherwise append a snapped
/// (Shift: 45-degree) vertex, resume an open path's end, add a motion-path keyframe, insert a vertex on a visible
/// path segment, or start a new mask/shape path on the selected layer or a new shape layer. Records the resulting
/// path id and tangent gesture in context data for the drag that follows.
#[allow(clippy::too_many_arguments)]
pub(super) fn pen_press(
    app: &mut EffectcraftApp,
    ui: &mut egui::Ui,
    ectx: &EvalCtx,
    map: &ViewerMap,
    paths: &[ov::PathInfo],
    motion_frame: Option<ov::PathFrame>,
    pos: Pos2,
    mods: egui::Modifiers,
) {
    use effectcraft_engine::project::LayerSource;
    let gid = egui::Id::unique("viewer-gesture");
    let current: Option<(LayerId, u64)> = ui.data(|d| d.get_temp(pen_id()));
    let c0 = map.to_comp(pos);
    let corr = vt::snap(app, ui.ctx(), ectx, map, &[], &[c0], mods);
    let c = [c0[0] + corr[0], c0[1] + corr[1]];
    if let Some((lid, uid)) = current
        && let Some(p) = ov::path_of(ectx, lid, uid)
        && let Some(inv) = p.m.inverse()
    {
        let n = p.sp.vertices.len();
        let first = p.sp.vertices.first().map(|v| {
            let q = p.m.apply(gv2(v[0], v[1]));
            map.to_screen([q.x, q.y])
        });
        if n >= 2 && first.is_some_and(|f| f.distance(pos) < 8.0) {
            let _ = app.session.execute("mask.setClosed", json!({"layer": lid.0, "mask": uid, "closed": true}));
            ui.data_mut(|d| {
                d.remove::<(LayerId, u64)>(pen_id());
                d.remove::<Gesture>(gid);
            });
            return;
        }
        // Shift keeps the new segment at a multiple of 45° from the vertex before it (#1544).
        let last = p.sp.vertices.last().map(|v| p.m.apply(gv2(v[0], v[1])));
        let c = match last {
            Some(q) if mods.shift => constrain_45([q.x, q.y], c),
            _ => c,
        };
        let lp = inv.apply(gv2(c[0], c[1]));
        let lp = [lp.x, lp.y];
        if let Ok(v) = app.session.execute("mask.addVertex", json!({"layer": lid.0, "mask": uid, "point": lp})) {
            let index = v.as_u64().unwrap_or(0) as usize;
            ui.data_mut(|d| d.insert_temp(gid, Gesture::Pen { layer: lid, mask: uid, index, vertex: lp, inv, press: pos }));
        }
        return;
    }
    // Clicking the last vertex of an open path (e.g. after Esc ended it) resumes drawing it.
    if let Some(cur) = ov::open_end_at(paths, map, pos, 8.0) {
        ui.data_mut(|d| d.insert_temp(pen_id(), cur));
        return;
    }
    // On a visible path's segment the Pen adds a vertex there; on a motion path, a keyframe.
    if let Some(f) = motion_frame {
        add_motion_key(app, f);
        return;
    }
    if let Some((l, m, seg, t)) = ov::segment_at(paths, map, pos, 5.0) {
        let _ = app.session.execute("mask.insertVertex", json!({"layer": l.0, "mask": m, "segment": seg, "t": t}));
        return;
    }
    let target = app
        .session
        .state
        .selected_layers
        .iter()
        .copied()
        .find(|id| ectx.comp.layer(*id).is_some_and(|l| !l.switches.locked && (l.masks().is_some() || matches!(l.source, LayerSource::Shape))));
    let layer = target.and_then(|id| ectx.comp.layer(id));
    let (lid, uid, inv) = match layer {
        Some(l) if !matches!(l.source, LayerSource::Shape) || app.session.state.shape_tool.creates_mask => {
            let Some(inv) = l2c(ectx, l).0.inverse() else { return };
            let lp = inv.apply(gv2(c[0], c[1]));
            let Ok(v) = app.session.execute("mask.new", json!({"layer": l.id.0, "vertices": [[lp.x, lp.y]]})) else { return };
            let Some(mask) = v["mask"].as_u64() else { return };
            (l.id, mask, inv)
        }
        Some(l) => {
            let Some(inv) = l2c(ectx, l).0.inverse() else { return };
            let lp = inv.apply(gv2(c[0], c[1]));
            let r = app.session.execute("shape.newPath", json!({"layer": l.id.0, "vertices": [[lp.x, lp.y]], "space": "layer"}));
            let Some(path) = r.ok().and_then(|v| v["path"].as_u64()) else { return };
            (l.id, path, inv)
        }
        None => {
            // A new shape layer at the comp centre (unrotated: comp → layer is a translation).
            let r = app.session.execute("shape.newPath", json!({"vertices": [c], "space": "comp"}));
            let Some(v) = r.ok() else { return };
            let (Some(l), Some(path)) = (v["layer"].as_u64(), v["path"].as_u64()) else { return };
            let inv = Mat3::translate(gv2(-(ectx.comp.width as f64) / 2.0, -(ectx.comp.height as f64) / 2.0));
            (LayerId(l), path, inv)
        }
    };
    let lp = inv.apply(gv2(c[0], c[1]));
    ui.data_mut(|d| {
        d.insert_temp(pen_id(), (lid, uid));
        d.insert_temp(gid, Gesture::Pen { layer: lid, mask: uid, index: 0, vertex: [lp.x, lp.y], inv, press: pos });
    });
}

/// `to` moved onto the nearest line through `from` at a multiple of 45° (horizontal, vertical or
/// diagonal), as far along it as `to` lies: the Pen's Shift-click.
pub(super) fn constrain_45(from: [f64; 2], to: [f64; 2]) -> [f64; 2] {
    let (dx, dy) = (to[0] - from[0], to[1] - from[1]);
    if !(dx.is_finite() && dy.is_finite()) || (dx == 0.0 && dy == 0.0) {
        return to;
    }
    let step = std::f64::consts::FRAC_PI_4;
    let a = (dy.atan2(dx) / step).round() * step;
    let (ux, uy) = (a.cos(), a.sin());
    let along = dx * ux + dy * uy;
    [from[0] + ux * along, from[1] + uy * along]
}

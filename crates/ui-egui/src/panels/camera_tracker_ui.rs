//! 3D Camera Tracker in the Composition viewer and Effect Controls:
//!
//! - while the effect is selected, the solved 3D track points are drawn as coloured targets
//!   sized by depth (or the 2D source tracks with Show Track Points ▸ 2D Source);
//! - hovering between three points shows the target plane they span (a disc in perspective);
//!   click a point (Shift adds / toggles) or drag a marquee to select points, click inside a
//!   triangle to select its three points; the selection's target is drawn;
//! - right-click: Set Ground Plane and Origin, Create Text / Solid / Null / Shadow Catcher …
//!   and Camera, Create Multiple …, Delete Selected Points; Delete removes the selection;
//! - the "Analyzing in background (step 1 of 2)" / "Solving camera" banner;
//! - the Effect Controls block with Analyze / Cancel / Create Camera, Method Used and Average
//!   Error.
//!
//! Everything is dispatched to the `camera.*` commands.

use crate::i18n::{tr, tr_args};
use std::sync::Arc;

use effectcraft_engine::Session;
use effectcraft_engine::effects::camera_tracker as ct;
use effectcraft_engine::geom::{Mat3, vec2 as gv2};
use effectcraft_engine::keyframe::Value;
use effectcraft_engine::project::{Comp, Layer, LayerId, PropGroup, Uid};
use effectcraft_engine::render::EvalCtx;
use effectcraft_engine::track::camtrack::{CameraSolve, CameraTracks, Target, point_color};
use egui::{Align2, Color32, Pos2, Rect, Sense, Stroke, pos2, vec2};
use serde_json::json;

use super::viewer::ViewerMap;
use crate::theme::Tokens;
use crate::{EffectcraftApp, widgets};

/// The tracker whose points the viewer shows.
struct Active {
    layer: LayerId,
    uid: Uid,
    solve: Option<Arc<CameraSolve>>,
    tracks: Option<Arc<CameraTracks>>,
    show_3d: bool,
    point_size: f64,
    target_size: f64,
    /// Undistort Footage is on: points project without the lens distortion.
    undistort: bool,
    /// Layer pixels → comp pixels.
    l2c: Mat3,
    frame: usize,
}

/// A drawn point: (id, screen position, screen radius).
type Hit = (u32, Pos2, f32);

fn tracker_group(l: &Layer, uid: Uid) -> Option<&PropGroup> {
    l.props.find_group(uid).filter(|g| matches!(&g.kind, effectcraft_engine::project::GroupKind::Effect { effect } if effect == ct::ID))
}

/// The selected 3D Camera Tracker effect on an active layer (After Effects shows the points
/// while the effect is selected).
fn active(s: &Session, comp: &Comp, ectx: &EvalCtx, l2c: &dyn Fn(&EvalCtx, &Layer) -> Mat3) -> Option<Active> {
    let time = ectx.time;
    for (lid, uid) in &s.state.selected_props {
        let Some(l) = comp.layer(*lid).filter(|l| l.is_active_at(time)) else { continue };
        let Some(g) = tracker_group(l, *uid) else { continue };
        let params = effectcraft_engine::camera_track::static_params(g);
        let val = |id: &str| effectcraft_engine::effects::warp_stab::param(&params, id).cloned();
        let solve = ct::solve(&params);
        let tracks = ct::tracks(&params);
        let lt = l.layer_time(time).seconds();
        let frame = solve.as_ref().and_then(|sv| sv.frame_at(lt)).or_else(|| {
            let t = tracks.as_ref()?;
            (t.frame_duration > 0.0).then(|| ((lt - t.start) / t.frame_duration).round().clamp(0.0, t.frames.saturating_sub(1) as f64) as usize)
        });
        let undistort = solve.as_ref().is_some_and(|sv| ct::undistorts(&params, sv));
        return Some(Active {
            layer: *lid,
            uid: *uid,
            solve,
            tracks,
            show_3d: val("showTrackPoints").map(|v| v.as_enum()).unwrap_or(1) == 1,
            point_size: val("trackPointSize").map(|v| v.as_f64()).unwrap_or(100.0),
            target_size: val("targetSize").map(|v| v.as_f64()).unwrap_or(100.0),
            undistort,
            l2c: l2c(ectx, l),
            frame: frame.unwrap_or(0),
        });
    }
    None
}

fn to_screen(a: &Active, map: &ViewerMap, q: [f64; 2]) -> Pos2 {
    let c = a.l2c.apply(gv2(q[0], q[1]));
    map.to_screen([c.x, c.y])
}

fn col(id: u32) -> Color32 {
    let c = point_color(id);
    Color32::from_rgb((c[0] * 255.0) as u8, (c[1] * 255.0) as u8, (c[2] * 255.0) as u8)
}

/// Comp-pixel scale of the layer → comp mapping.
fn l2c_scale(m: &Mat3) -> f32 {
    m.0[0][0].hypot(m.0[1][0]) as f32
}

/// Draw the target disc (concentric rings in perspective) of `t` on frame `k`.
fn draw_target(p: &egui::Painter, a: &Active, map: &ViewerMap, sv: &CameraSolve, t: &Target) {
    if sv.frames.get(a.frame).is_none() {
        return;
    }
    let r = t.size * 0.5 * a.target_size / 100.0;
    for (i, frac) in [1.0, 0.66, 0.33].into_iter().enumerate() {
        let ring = ct::target_circle(t.center, t.normal, r * frac, 48);
        let pts: Vec<Pos2> = ring.iter().filter_map(|x| sv.project(a.frame, *x, a.undistort)).map(|q| to_screen(a, map, q)).collect();
        if pts.len() < 3 {
            return;
        }
        let fill = if i % 2 == 0 { Color32::from_rgba_unmultiplied(0xd0, 0x30, 0x30, 70) } else { Color32::from_rgba_unmultiplied(0xff, 0xff, 0xff, 50) };
        p.add(egui::Shape::convex_polygon(pts.clone(), fill, Stroke::new(1.0, Color32::from_rgb(0xe0, 0x40, 0x40))));
    }
    if let Some(c) = sv.project(a.frame, t.center, a.undistort) {
        let c = to_screen(a, map, c);
        p.circle_filled(c, 3.0, Color32::from_rgb(0xff, 0xd0, 0x40));
    }
}

/// The three drawn points forming a triangle around `hp` (nearest first).
fn triangle_at(hits: &[Hit], hp: Pos2) -> Option<[u32; 3]> {
    let mut near: Vec<&Hit> = hits.iter().collect();
    near.sort_by(|x, y| x.1.distance_sq(hp).total_cmp(&y.1.distance_sq(hp)));
    let near: Vec<&Hit> = near.into_iter().take(8).collect();
    let inside = |a: Pos2, b: Pos2, c: Pos2| {
        let s = |p: Pos2, q: Pos2| (q.x - p.x) * (hp.y - p.y) - (q.y - p.y) * (hp.x - p.x);
        let (d1, d2, d3) = (s(a, b), s(b, c), s(c, a));
        !((d1 < 0.0 || d2 < 0.0 || d3 < 0.0) && (d1 > 0.0 || d2 > 0.0 || d3 > 0.0))
    };
    for i in 0..near.len() {
        for j in i + 1..near.len() {
            for k in j + 1..near.len() {
                let (a, b, c) = (near[i], near[j], near[k]);
                // Skip slivers.
                let area = ((b.1 - a.1).x * (c.1 - a.1).y - (b.1 - a.1).y * (c.1 - a.1).x).abs();
                if area > 30.0 && inside(a.1, b.1, c.1) {
                    return Some([a.0, b.0, c.0]);
                }
            }
        }
    }
    None
}

fn run(app: &mut EffectcraftApp, ctx: &egui::Context, cmd: &str, p: serde_json::Value) {
    if let Err(e) = crate::menus::invoke(app, ctx, cmd, p) {
        app.ui.status = e;
    }
}

/// The viewer overlay and its interaction (call after the viewer's own gestures so it takes
/// the clicks while a 3D Camera Tracker is selected).
pub fn viewer_hook(
    app: &mut EffectcraftApp,
    ui: &mut egui::Ui,
    painter: &egui::Painter,
    map: &ViewerMap,
    ectx: &EvalCtx,
    l2c: &dyn Fn(&EvalCtx, &Layer) -> Mat3,
) {
    let ctx = ui.ctx().clone();
    let comp = ectx.comp;
    banner(app, painter, map, ectx, l2c);
    if !app.ui.viewer.show_layer_controls {
        return;
    }
    let Some(a) = active(&app.session, comp, ectx, l2c) else { return };
    let selected = app.session.state.camera_points.clone();
    let lscale = l2c_scale(&a.l2c) * map.zoom;
    let mut hits: Vec<Hit> = vec![];
    match (&a.solve, a.show_3d) {
        (Some(sv), true) => {
            let pts = ct::projected_with(sv, a.frame, a.undistort);
            let med = ct::median_depth(&pts);
            for (id, q, z) in pts {
                let sp = to_screen(&a, map, q);
                if !map.area.expand(20.0).contains(sp) {
                    continue;
                }
                let r = (ct::target_radius(a.point_size, z, med) as f32 * lscale).clamp(2.0, 30.0);
                let c = col(id);
                let sel = selected.contains(&id);
                if sel {
                    painter.circle_filled(sp, r, c);
                    painter.circle_stroke(sp, r + 1.5, Stroke::new(1.5, Color32::WHITE));
                } else {
                    painter.circle_stroke(sp, r, Stroke::new((r * 0.3).clamp(1.0, 3.0), c));
                    painter.circle_filled(sp, (r * 0.25).max(1.0), c);
                }
                hits.push((id, sp, r));
            }
        }
        _ => {
            // 2D source tracks (also shown before the solve finishes).
            if let Some(tr) = &a.tracks {
                for t in &tr.tracks {
                    let Some(q) = t.at(a.frame as u32) else { continue };
                    let sp = to_screen(&a, map, q);
                    let c = col(t.id);
                    let r = (3.0 * a.point_size as f32 / 100.0).clamp(1.5, 12.0);
                    painter.line_segment([sp - vec2(r, r), sp + vec2(r, r)], Stroke::new(1.2, c));
                    painter.line_segment([sp - vec2(r, -r), sp + vec2(r, -r)], Stroke::new(1.2, c));
                    hits.push((t.id, sp, r));
                }
            }
        }
    }
    let Some(sv) = a.solve.clone() else { return };
    if app.ui.tool != crate::state::Tool::Selection {
        return;
    }
    // Interaction over the image.
    // While Space (Hand tool) is held the overlay only hovers, so the viewer underneath gets the pan drag.
    let sense = if super::space_hand(&ctx) { Sense::hover() } else { Sense::click_and_drag() };
    let resp = ui.interact(map.area, egui::Id::unique("viewer-camera-tracker"), sense);
    app.auto.add("viewer.cameraTracker", map.area, "3D Camera Tracker points");
    // The overlay is on top of the viewer, so a middle-button drag lands here: pan the viewer like the Hand tool.
    if resp.dragged_by(egui::PointerButton::Middle) {
        let pid = egui::Id::unique("viewer-camera-tracker-pan");
        if resp.drag_started() {
            let start_pan = app.ui.viewer.pan;
            ui.data_mut(|d| d.insert_temp(pid, start_pan));
        }
        if let (Some(start_pan), Some(d)) = (ui.data(|m| m.get_temp::<[f32; 2]>(pid)), resp.total_drag_delta()) {
            app.ui.viewer.pan = [start_pan[0] + d.x, start_pan[1] + d.y];
        }
        return;
    }
    for (id, sp, r) in hits.iter().take(400) {
        app.auto.add(&format!("viewer.cameraTracker.point.{id}"), Rect::from_center_size(*sp, vec2(r * 2.0 + 4.0, r * 2.0 + 4.0)), "track point");
    }
    let hover = resp.hover_pos();
    let point_at = |p: Pos2| hits.iter().filter(|h| h.1.distance(p) <= h.2 + 3.0).min_by(|x, y| x.1.distance(p).total_cmp(&y.1.distance(p))).map(|h| h.0);
    let tri = hover.filter(|p| point_at(*p).is_none()).and_then(|p| triangle_at(&hits, p));
    let mid = egui::Id::unique("viewer-camera-tracker-marquee");
    let marquee: Option<Pos2> = ui.data(|d| d.get_temp(mid));
    // Hover target / the selection's target.
    if marquee.is_none() {
        if let Some(t3) = tri
            && let Some(t) = effectcraft_engine::camera_track::target_for(&sv, &t3, a.frame)
        {
            let ps: Vec<Pos2> = t3.iter().filter_map(|i| hits.iter().find(|h| h.0 == *i)).map(|h| h.1).collect();
            if ps.len() == 3 {
                painter.add(egui::Shape::closed_line(ps, Stroke::new(1.0, Color32::from_white_alpha(200))));
            }
            draw_target(painter, &a, map, &sv, &t);
        } else if !selected.is_empty()
            && let Some(t) = effectcraft_engine::camera_track::target_for(&sv, &selected, a.frame)
        {
            draw_target(painter, &a, map, &sv, &t);
        }
    }
    let mods = ui.input(|i| i.modifiers);
    if resp.drag_started()
        && let Some(p) = ui.input(|i| i.pointer.press_origin())
    {
        ui.data_mut(|d| d.insert_temp(mid, p));
    }
    if let (Some(start), Some(p)) = (marquee, resp.interact_pointer_pos()) {
        let r = Rect::from_two_pos(start, p);
        let t = app.tokens;
        painter.rect_filled(r, 0.0, t.accent.gamma_multiply(0.12));
        painter.rect_stroke(r, 0.0, Stroke::new(1.0, t.accent), egui::StrokeKind::Middle);
        if resp.drag_stopped() {
            let ids: Vec<u32> = hits.iter().filter(|h| r.contains(h.1)).map(|h| h.0).collect();
            ui.data_mut(|d| d.remove::<Pos2>(mid));
            run(app, &ctx, "camera.selectPoints", json!({"points": ids, "add": mods.shift}));
        }
    } else if resp.drag_stopped() {
        ui.data_mut(|d| d.remove::<Pos2>(mid));
    }
    if resp.clicked()
        && let Some(p) = resp.interact_pointer_pos()
    {
        match (point_at(p), triangle_at(&hits, p)) {
            (Some(id), _) => run(app, &ctx, "camera.selectPoints", json!({"points": [id], "toggle": mods.shift})),
            (None, Some(t3)) => run(app, &ctx, "camera.selectPoints", json!({"points": t3, "add": mods.shift})),
            (None, None) => run(app, &ctx, "camera.selectPoints", json!({"points": []})),
        }
    }
    // Right-click: the points under the cursor (selection, else the hovered triangle).
    let cid = egui::Id::unique("viewer-camera-tracker-menu-points");
    if resp.secondary_clicked() {
        let pts: Vec<u32> = if !selected.is_empty() {
            selected.clone()
        } else if let Some(p) = resp.interact_pointer_pos() {
            point_at(p).map(|i| vec![i]).or_else(|| triangle_at(&hits, p).map(|t| t.to_vec())).unwrap_or_default()
        } else {
            vec![]
        };
        ui.data_mut(|d| d.insert_temp(cid, pts));
    }
    let base = json!({"layer": a.layer.0, "effect": a.uid});
    crate::widgets::context_menu(&resp, |ui| {
        let pts: Vec<u32> = ui.data(|d| d.get_temp(cid)).unwrap_or_default();
        let with = |kind: Option<&str>, multiple: bool| {
            let mut q = base.clone();
            q["points"] = json!(pts);
            if let Some(k) = kind {
                q["kind"] = json!(k);
            }
            if multiple {
                q["multiple"] = json!(true);
            }
            q
        };
        let some = !pts.is_empty();
        let many = pts.len() > 1;
        let mut pick: Option<(&str, serde_json::Value)> = None;
        if ui.add_enabled(some, egui::Button::new(tr("Set Ground Plane and Origin"))).clicked() {
            pick = Some(("camera.setGroundPlane", with(None, false)));
        }
        ui.separator();
        for (label, kind) in [
            ("Create Text and Camera", "text"),
            ("Create Solid and Camera", "solid"),
            ("Create Null and Camera", "null"),
            ("Create Shadow Catcher, Camera and Light", "shadowCatcher"),
        ] {
            if ui.add_enabled(some, egui::Button::new(label)).clicked() {
                pick = Some(("camera.createFromSolve", with(Some(kind), false)));
            }
        }
        ui.separator();
        for (label, kind) in
            [("Create Multiple Text Layers and Camera", "text"), ("Create Multiple Solids and Camera", "solid"), ("Create Multiple Nulls and Camera", "null")]
        {
            if ui.add_enabled(many, egui::Button::new(label)).clicked() {
                pick = Some(("camera.createFromSolve", with(Some(kind), true)));
            }
        }
        ui.separator();
        if ui.add_enabled(some, egui::Button::new(tr("Delete Selected Points"))).clicked() {
            pick = Some(("camera.deletePoints", with(None, false)));
        }
        if let Some((cmd, q)) = pick {
            ui.close();
            run(app, &ctx, cmd, q);
        }
    });
    if resp.hovered()
        && app.dialog.is_none()
        && !selected.is_empty()
        && ui.input(|i| i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace))
        && !ctx.egui_wants_keyboard_input()
    {
        run(app, &ctx, "camera.deletePoints", json!({"layer": a.layer.0, "effect": a.uid, "points": selected}));
    }
}

/// "Analyzing in background (step 1 of 2)" / "Solving camera" over the analysed layer, and a
/// warning when the solve failed (unless Hide Warning Banner).
fn banner(app: &mut EffectcraftApp, painter: &egui::Painter, map: &ViewerMap, ectx: &EvalCtx, l2c: &dyn Fn(&EvalCtx, &Layer) -> Mat3) {
    let comp = ectx.comp;
    let time = ectx.time;
    let cid = ectx.comp_id;
    let draw = |app: &mut EffectcraftApp, layer: &Layer, text: String, color: Color32, id: &str| {
        let m = l2c(ectx, layer);
        let (w, h) = effectcraft_engine::render::source_size(&app.session.project, layer);
        let c = m.apply(gv2(w as f64 / 2.0, h as f64 / 2.0));
        let at = map.to_screen([c.x, c.y]);
        let galley = painter.layout_no_wrap(text.clone(), Tokens::medium(13.0), Color32::WHITE);
        let r = Rect::from_center_size(at, galley.size() + vec2(28.0, 14.0)).intersect(map.area);
        painter.rect_filled(r, 3.0, color);
        painter.galley(pos2(r.center().x - galley.size().x / 2.0, r.center().y - galley.size().y / 2.0), galley, Color32::WHITE);
        app.auto.add(id, r, &text);
    };
    if let Some((cc, cl, _)) = app.session.camera_target()
        && cc == cid
        && let Some(pr) = app.session.camera_progress()
        && let Some(layer) = comp.layer(cl)
        && layer.is_active_at(time)
    {
        draw(app, layer, pr.banner(), Color32::from_rgba_unmultiplied(0x1d, 0x4f, 0x9c, 0xe0), "viewer.cameraBanner");
        return;
    }
    // A failed solve: tracks without a solve while nothing is running or queued.
    for l in comp.layers.iter().filter(|l| l.is_active_at(time)) {
        let Some(fx) = l.effects() else { continue };
        for g in fx.groups().filter(|g| matches!(&g.kind, effectcraft_engine::project::GroupKind::Effect { effect } if effect == ct::ID)) {
            let params = effectcraft_engine::camera_track::static_params(g);
            let hide = effectcraft_engine::effects::warp_stab::param(&params, "advanced/hideWarningBanner").is_some_and(Value::as_bool);
            let solve_key = matches!(g.prop(ct::SOLVE_KEY).map(|p| &p.value), Some(Value::Str(s)) if !s.is_empty());
            let failed = solve_key && ct::tracks(&params).is_some() && ct::solve(&params).is_none();
            if failed && !hide && !app.session.camera_pending.iter().any(|(_, ll, u)| *ll == l.id && *u == g.uid) {
                draw(app, l, "Unable to solve camera".into(), Color32::from_rgba_unmultiplied(0x9c, 0x2a, 0x1d, 0xe0), "viewer.cameraWarning");
                return;
            }
        }
    }
}

/// Effect Controls: Analyze / Cancel, Create Camera and the solve report.
#[allow(clippy::too_many_arguments)]
pub fn editor(
    app: &mut EffectcraftApp,
    ui: &mut egui::Ui,
    p: &egui::Painter,
    layer: &Layer,
    g: &PropGroup,
    r: Rect,
    actions: &mut Vec<(String, serde_json::Value)>,
) {
    let t = app.tokens;
    let running = app.session.camera_target().is_some_and(|(_, l, u)| l == layer.id && u == g.uid) && app.session.is_camera_analyzing();
    let busy = app.session.is_camera_analyzing();
    let me = json!({"layer": layer.id.0, "effect": g.uid});
    let b1 = Rect::from_min_size(pos2(r.min.x + 24.0, r.min.y + 4.0), vec2(84.0, 22.0));
    let b2 = Rect::from_min_size(pos2(b1.max.x + 8.0, b1.min.y), vec2(84.0, 22.0));
    let b3 = Rect::from_min_size(pos2(b2.max.x + 8.0, b1.min.y), vec2(110.0, 22.0));
    if widgets::text_button(ui, b1, crate::i18n::tr("Analyze"), false, &t, egui::Id::unique(("ct-analyze", g.uid))).clicked() && !busy {
        actions.push(("camera.analyze".into(), me.clone()));
    }
    app.auto.add(&format!("effectControls.cameraTracker.{}.analyze", g.uid), b1, "Analyze");
    if running {
        if widgets::text_button(ui, b2, crate::i18n::tr("Cancel"), false, &t, egui::Id::unique(("ct-cancel", g.uid))).clicked() {
            actions.push(("camera.cancel".into(), json!({})));
        }
        app.auto.add(&format!("effectControls.cameraTracker.{}.cancel", g.uid), b2, "Cancel");
    }
    let params = effectcraft_engine::camera_track::static_params(g);
    let solve = ct::solve(&params);
    if solve.is_some() {
        if widgets::text_button(ui, b3, crate::i18n::tr("Create Camera"), false, &t, egui::Id::unique(("ct-camera", g.uid))).clicked() {
            actions.push(("camera.create".into(), me.clone()));
        }
        app.auto.add(&format!("effectControls.cameraTracker.{}.createCamera", g.uid), b3, "Create Camera");
    }
    let status = match app.session.camera_progress() {
        Some(pr) if running => pr.banner(),
        _ => match &solve {
            Some(sv) => tr_args("Method Used: {}    Average Error: {} pixels", &[&(sv.method_used.label()), &format!("{:.2}", sv.average_error)]),
            None if ct::tracks(&params).is_some() && app.session.camera_pending.iter().any(|(_, l, u)| *l == layer.id && *u == g.uid) => {
                "Waiting to solve".into()
            }
            None if ct::tracks(&params).is_some() => "Unable to solve camera: try another Solve Method or Shot Type".into(),
            None if app.session.camera_pending.iter().any(|(_, l, u)| *l == layer.id && *u == g.uid) => "Waiting to analyze".into(),
            None => "Not analyzed: click Analyze".into(),
        },
    };
    p.text(pos2(r.min.x + 24.0, r.min.y + 38.0), Align2::LEFT_CENTER, status, Tokens::ui(11.5), t.text_dim);
    if let Some(sv) = &solve {
        let f = sv.focal();
        p.text(
            pos2(r.min.x + 24.0, r.min.y + 56.0),
            Align2::LEFT_CENTER,
            format!("{} solved points · Horizontal Angle of View {:.1}°", sv.points.len(), sv.hfov(f)),
            Tokens::ui(11.5),
            t.text_faint,
        );
    }
}

/// Height of [`editor`].
pub const EDITOR_HEIGHT: f32 = 68.0;

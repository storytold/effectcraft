//! Wiggler, Smoother, Motion Sketch and Mask Interpolation panels (Window menu). Their settings live in
//! [`crate::state::AnimToolsState`] (serde, so agents can read and set them); Apply runs the
//! engine commands `keys.wiggle`, `keys.smooth` and `motion.sketch`.
//!
//! Motion Sketch capture is an overlay on the Composition viewer ([`sketch_overlay`]): after
//! Start Capture, pressing in the viewer records the pointer path in real time while the comp
//! plays; releasing turns it into Position keys.

use crate::i18n::{tr, tr_args};
use egui::{Align2, Color32, Rect, Sense, Stroke, pos2, vec2};
use serde_json::json;

use crate::theme::Tokens;
use crate::{EffectcraftApp, widgets};

const ROW: f32 = 26.0;

fn label(p: &egui::Painter, x: f32, y: f32, s: &str, t: &Tokens) {
    p.text(pos2(x, y + 9.0), Align2::LEFT_CENTER, s, Tokens::ui(12.0), t.text_dim);
}

/// A dropdown with options; returns the new index when changed.
fn choice(app: &mut EffectcraftApp, ui: &mut egui::Ui, r: Rect, auto: &str, options: &[&str], cur: usize) -> Option<usize> {
    let t = app.tokens;
    let id = egui::Id::unique(("animtool-dd", auto));
    if widgets::dropdown(ui, r, options.get(cur).copied().unwrap_or(""), &t, id).clicked() {
        widgets::open_popup(ui, id);
    }
    app.auto.add(auto, r, options.get(cur).copied().unwrap_or(""));
    let opts: Vec<String> = options.iter().map(|s| s.to_string()).collect();
    widgets::popup_menu(ui, id, r.left_bottom(), &opts, Some(cur)).filter(|i| *i != cur)
}

/// A drag-to-change number field for an anim-tool setting: `v` clamped to `range`, `speed` points
/// per pixel, `dec` decimals and a `suffix`; returns the new value, or `v` when it did not change.
fn number(app: &mut EffectcraftApp, ui: &mut egui::Ui, at: egui::Pos2, auto: &str, v: f64, spec: widgets::HotSpec) -> f64 {
    let t = app.tokens;
    let (r, nv, _) = widgets::hot_number_at(ui, at, egui::Id::unique(("animtool-n", auto)), v, &spec, &t);
    app.auto.add(auto, r, &format!("{v}"));
    nv.unwrap_or(v)
}

/// A checkbox at `at` with its `text` label drawn to its right; returns the state to store, i.e.
/// `!on` when it was clicked and `on` otherwise.
fn checkbox(app: &mut EffectcraftApp, ui: &mut egui::Ui, at: egui::Pos2, auto: &str, text: &str, on: bool) -> bool {
    let t = app.tokens;
    let r = Rect::from_min_size(at, vec2(16.0, 16.0));
    let clicked = widgets::checkbox(ui, r, on, &t, egui::Id::unique(("animtool-cb", auto))).changed();
    ui.painter().text(pos2(r.max.x + 6.0, r.center().y), Align2::LEFT_CENTER, text, Tokens::ui(12.0), t.text);
    app.auto.add(auto, r, text);
    if clicked { !on } else { on }
}

fn status_line(app: &EffectcraftApp, ui: &egui::Ui, rect: Rect, y: f32, need: usize) {
    let t = app.tokens;
    let n = app.session.state.selected_keys.len();
    let msg = if n >= need { format!("{n} keyframes selected") } else { format!("Select at least {need} keyframes of a property") };
    ui.painter().with_clip_rect(rect).text(pos2(rect.min.x + 12.0, y), Align2::LEFT_CENTER, msg, Tokens::ui(11.0), t.text_faint);
}

fn apply(app: &mut EffectcraftApp, cmd: &str, params: serde_json::Value) {
    match app.session.execute(cmd, params) {
        Ok(r) => app.ui.status = format!("{}: {} → {} keyframes", if cmd == "keys.wiggle" { "Wiggler" } else { "Smoother" }, r["before"], r["after"]),
        Err(e) => app.ui.status = e.to_string(),
    }
}

/// Wiggler panel: Apply To, Noise Type, Dimension, Frequency, Magnitude.
pub fn wiggler(app: &mut EffectcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let t = app.tokens;
    let p = ui.painter().with_clip_rect(rect);
    let x0 = rect.min.x + 12.0;
    let xv = x0 + 96.0;
    let w = (rect.max.x - xv - 12.0).clamp(80.0, 200.0);
    let mut y = rect.min.y + 10.0;
    let mut st = app.ui.anim_tools.clone();
    label(&p, x0, y, "Apply To:", &t);
    if let Some(i) =
        choice(app, ui, Rect::from_min_size(pos2(xv, y), vec2(w, 20.0)), "wiggler.applyTo", &["Spatial Path", "Temporal Path"], usize::from(!st.wiggle_spatial))
    {
        st.wiggle_spatial = i == 0;
    }
    y += ROW;
    label(&p, x0, y, "Noise Type:", &t);
    if let Some(i) = choice(app, ui, Rect::from_min_size(pos2(xv, y), vec2(w, 20.0)), "wiggler.noise", &["Smooth", "Jagged"], usize::from(!st.wiggle_smooth)) {
        st.wiggle_smooth = i == 0;
    }
    y += ROW;
    label(&p, x0, y, "Dimension:", &t);
    let dims = ["X", "Y", "Z", "All the Same", "All Independently"];
    let cur = match st.wiggle_dims.as_str() {
        "one" => st.wiggle_dim.min(2),
        "same" => 3,
        _ => 4,
    };
    if let Some(i) = choice(app, ui, Rect::from_min_size(pos2(xv, y), vec2(w, 20.0)), "wiggler.dimension", &dims, cur) {
        match i {
            0..=2 => {
                st.wiggle_dims = "one".into();
                st.wiggle_dim = i;
            }
            3 => st.wiggle_dims = "same".into(),
            _ => st.wiggle_dims = "independent".into(),
        }
    }
    y += ROW + 4.0;
    label(&p, x0, y, "Frequency:", &t);
    st.wiggle_frequency = number(
        app,
        ui,
        pos2(xv, y),
        "wiggler.frequency",
        st.wiggle_frequency,
        widgets::HotSpec { speed: 0.1, range: (0.1, 100.0), decimals: 1, suffix: " per second" },
    );
    y += ROW;
    label(&p, x0, y, "Magnitude:", &t);
    st.wiggle_magnitude = number(
        app,
        ui,
        pos2(xv, y),
        "wiggler.magnitude",
        st.wiggle_magnitude,
        widgets::HotSpec { speed: 0.5, range: (0.0, 100_000.0), decimals: 1, suffix: "" },
    );
    y += ROW + 8.0;
    app.ui.anim_tools = st.clone();
    let b = Rect::from_min_size(pos2(rect.max.x - 92.0, y), vec2(80.0, 24.0));
    let enabled = app.session.state.selected_keys.len() >= 2;
    if widgets::text_button(ui, b, crate::i18n::tr("Apply"), enabled, &t, egui::Id::unique("wg-apply")).clicked() {
        let params = json!({
            "apply": if st.wiggle_spatial { "spatial" } else { "temporal" },
            "noise": if st.wiggle_smooth { "smooth" } else { "jagged" },
            "dimensions": st.wiggle_dims,
            "dimension": st.wiggle_dim,
            "frequency": st.wiggle_frequency,
            "magnitude": st.wiggle_magnitude,
            "seed": app.session.revision,
        });
        apply(app, "keys.wiggle", params);
    }
    app.auto.add("wiggler.apply", b, "Apply");
    status_line(app, ui, rect, b.max.y + 16.0, 2);
}

/// Smoother panel: Apply To (from the selected property), Tolerance.
pub fn smoother(app: &mut EffectcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let t = app.tokens;
    let p = ui.painter().with_clip_rect(rect);
    let x0 = rect.min.x + 12.0;
    let xv = x0 + 96.0;
    let mut y = rect.min.y + 10.0;
    let spatial = app.session.state.selected_keys.first().and_then(|k| {
        let c = app.session.active_comp()?;
        Some(c.layer(k.layer)?.props.find(k.prop)?.spatial)
    });
    label(&p, x0, y, "Apply To:", &t);
    let txt = match spatial {
        Some(true) => "Spatial Path",
        Some(false) => "Temporal Graph",
        None => "—",
    };
    p.text(pos2(xv, y + 9.0), Align2::LEFT_CENTER, txt, Tokens::ui(12.0), t.text);
    y += ROW;
    label(&p, x0, y, "Tolerance:", &t);
    let tol = number(
        app,
        ui,
        pos2(xv, y),
        "smoother.tolerance",
        app.ui.anim_tools.smooth_tolerance,
        widgets::HotSpec { speed: 0.05, range: (0.0, 10_000.0), decimals: 2, suffix: "" },
    );
    app.ui.anim_tools.smooth_tolerance = tol;
    y += ROW + 8.0;
    let b = Rect::from_min_size(pos2(rect.max.x - 92.0, y), vec2(80.0, 24.0));
    let enabled = app.session.state.selected_keys.len() >= 3;
    if widgets::text_button(ui, b, crate::i18n::tr("Apply"), enabled, &t, egui::Id::unique("sm-apply")).clicked() {
        apply(app, "keys.smooth", json!({"tolerance": tol}));
    }
    app.auto.add("smoother.apply", b, "Apply");
    status_line(app, ui, rect, b.max.y + 16.0, 3);
}

/// Motion Sketch panel: Capture speed, Smoothing, Show Wireframe / Background, Start Capture.
pub fn motion_sketch(app: &mut EffectcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let t = app.tokens;
    let p = ui.painter().with_clip_rect(rect);
    let x0 = rect.min.x + 12.0;
    let xv = x0 + 110.0;
    let mut y = rect.min.y + 10.0;
    let mut st = app.ui.anim_tools.clone();
    label(&p, x0, y, "Capture speed at:", &t);
    st.sketch_speed = number(
        app,
        ui,
        pos2(xv, y),
        "motionSketch.captureSpeed",
        st.sketch_speed,
        widgets::HotSpec { speed: 1.0, range: (1.0, 10_000.0), decimals: 0, suffix: " %" },
    );
    y += ROW;
    label(&p, x0, y, "Smoothing:", &t);
    st.sketch_smoothing = number(
        app,
        ui,
        pos2(xv, y),
        "motionSketch.smoothing",
        st.sketch_smoothing,
        widgets::HotSpec { speed: 0.1, range: (0.0, 100.0), decimals: 1, suffix: "" },
    );
    y += ROW + 2.0;
    p.text(pos2(x0, y + 8.0), Align2::LEFT_CENTER, tr("Show:"), Tokens::ui(12.0), t.text_dim);
    y += ROW - 6.0;
    st.sketch_wireframe = checkbox(app, ui, pos2(x0 + 8.0, y), "motionSketch.wireframe", "Wireframe", st.sketch_wireframe);
    y += ROW - 4.0;
    st.sketch_background = checkbox(app, ui, pos2(x0 + 8.0, y), "motionSketch.background", "Background", st.sketch_background);
    y += ROW;
    let Some(c) = app.session.active_comp() else {
        app.ui.anim_tools = st;
        return;
    };
    let fr = c.frame_rate;
    let (s, e) = (app.session.time(), c.work_area.1);
    let tc = |x| crate::panels::timecode(&app.session, c, x);
    p.text(
        pos2(x0, y + 8.0),
        Align2::LEFT_CENTER,
        tr_args("Start: {}   Duration: {}", &[&(tc(s)), &crate::panels::duration(&app.session, c, e - s)]),
        Tokens::ui(11.0),
        t.text_faint,
    );
    let _ = fr;
    y += ROW;
    let b = Rect::from_min_size(pos2(x0, y), vec2(120.0, 24.0));
    let armed = st.sketch_armed;
    let has_layer = !app.session.state.selected_layers.is_empty();
    if widgets::text_button(ui, b, if armed { "Cancel Capture" } else { "Start Capture" }, has_layer && !armed, &t, egui::Id::unique("ms-start")).clicked() {
        if !has_layer {
            app.ui.status = "Select a layer to sketch its motion".into();
        } else {
            st.sketch_armed = !armed;
            if st.sketch_armed {
                app.ui.status = "Motion Sketch: drag in the Composition panel to record".into();
            }
        }
    }
    app.auto.add("motionSketch.start", b, "Start Capture");
    app.ui.anim_tools = st;
}

/// A capture in progress: recording start (ui seconds), comp start time, points (s, x, y).
#[derive(Clone, Debug)]
struct Capture {
    t0: f64,
    start: effectcraft_engine::time::Tick,
    points: Vec<[f64; 3]>,
}

/// The `egui::Id` of the Motion Sketch capture in progress ([`Capture`]): its start time and the
/// recorded points.
fn capture_id() -> egui::Id {
    egui::Id::unique("motion-sketch-capture")
}

/// The Motion Sketch overlay on the Composition viewer (`area`): while armed, a press starts
/// recording, the comp time advances in real time (scaled by the capture speed), and the release
/// creates the keys. Draws the recorded path and, with Wireframe, the layer's box at the pointer.
pub fn sketch_overlay(app: &mut EffectcraftApp, ui: &mut egui::Ui) {
    if !app.ui.anim_tools.sketch_armed {
        return;
    }
    let ctx = ui.ctx().clone();
    let Some(map) = ctx.data(|d| d.get_temp::<crate::panels::viewer::ViewerMap>(egui::Id::unique("viewer-map"))) else { return };
    let area = map.area;
    let resp = ui.interact(area, egui::Id::unique("motion-sketch-overlay"), Sense::drag());
    app.auto.add("motionSketch.overlay", area, "Motion Sketch capture area");
    let now = ctx.input(|i| i.time);
    let painter = ui.painter().with_clip_rect(area);
    if !app.ui.anim_tools.sketch_background {
        painter.rect_filled(Rect::from_min_size(map.origin, vec2(map.comp[0] * map.zoom, map.comp[1] * map.zoom)), 0.0, Color32::from_black_alpha(200));
    }
    ctx.set_cursor_icon(egui::CursorIcon::Crosshair);
    let mut cap: Option<Capture> = ctx.data(|d| d.get_temp(capture_id()));
    if resp.drag_started()
        && let Some(pt) = resp.interact_pointer_pos()
    {
        let q = map.to_comp(pt);
        cap = Some(Capture { t0: now, start: app.session.time(), points: vec![[0.0, q[0], q[1]]] });
    }
    if let Some(c) = cap.as_mut() {
        if let Some(pt) = ctx.input(|i| i.pointer.latest_pos()) {
            let q = map.to_comp(pt);
            let el = now - c.t0;
            if c.points.last().is_none_or(|l| el > l[0]) {
                c.points.push([el, q[0], q[1]]);
            }
            // Play the comp in real time (slowed by the capture speed) while recording.
            let speed = app.ui.anim_tools.sketch_speed.max(1.0) / 100.0;
            let comp_t = c.start + effectcraft_engine::time::Tick::from_seconds_f64(el * speed);
            if let Some(comp) = app.session.active_comp()
                && comp_t < comp.duration
            {
                let ft = comp.frame_rate.snap_nearest(comp_t);
                app.session.set_time(ft);
            }
        }
        let pts: Vec<egui::Pos2> = c.points.iter().map(|p| map.to_screen([p[1], p[2]])).collect();
        painter.add(egui::Shape::line(pts.clone(), Stroke::new(1.5, Color32::from_rgb(0xff, 0x60, 0x40))));
        if app.ui.anim_tools.sketch_wireframe
            && let Some(last) = pts.last()
        {
            painter.rect_stroke(
                Rect::from_center_size(*last, vec2(60.0, 40.0) * map.zoom.max(0.2)),
                0.0,
                Stroke::new(1.0, Color32::WHITE),
                egui::StrokeKind::Inside,
            );
        }
        ctx.request_repaint();
    }
    if resp.drag_stopped()
        && let Some(c) = cap.take()
    {
        ctx.data_mut(|d| d.remove::<Capture>(capture_id()));
        app.ui.anim_tools.sketch_armed = false;
        let st = &app.ui.anim_tools;
        let params = json!({"points": c.points, "start": c.start.seconds(), "captureSpeed": st.sketch_speed, "smoothing": st.sketch_smoothing});
        match app.session.execute("motion.sketch", params) {
            Ok(r) => app.ui.status = format!("Motion Sketch: {} Position keyframes", r["keys"]),
            Err(e) => app.ui.status = e.to_string(),
        }
        app.session.set_time(c.start);
        return;
    }
    match cap {
        Some(c) => {
            ctx.data_mut(|d| d.insert_temp(capture_id(), c));
        }
        None => {
            painter.text(area.center_top() + vec2(0.0, 18.0), Align2::CENTER_CENTER, tr("Motion Sketch: drag to record"), Tokens::ui(12.0), Color32::WHITE);
        }
    }
}

/// Mask Interpolation panel: Keyframe Rate / Fields, Linear Vertex Paths, Bending Resistance,
/// Quality, Add Mask Shape Vertices, Matching Method, 1:1 Vertex Matches, First Vertices Match;
/// Apply runs `mask.interpolate` on the selected Mask Path keyframes. The options live in the
/// editor state (`mask.interpolationOptions`).
pub fn mask_interpolation(app: &mut EffectcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let t = app.tokens;
    let p = ui.painter().with_clip_rect(rect);
    let x0 = rect.min.x + 12.0;
    let xv = x0 + 130.0;
    let w = (rect.max.x - xv - 12.0).clamp(80.0, 200.0);
    let mut y = rect.min.y + 10.0;
    let o = app.session.state.mask_interp.clone();
    let mut changes = serde_json::Map::new();
    label(&p, x0, y, "Keyframe Rate:", &t);
    let auto = o.keyframe_rate.is_none();
    if let Some(i) = choice(app, ui, Rect::from_min_size(pos2(xv, y), vec2(70.0, 20.0)), "maskInterp.rateMode", &["Auto", "Custom"], usize::from(!auto)) {
        let fps = app.session.active_comp().map(|c| c.frame_rate.as_f64()).unwrap_or(30.0);
        changes.insert("keyframeRate".into(), if i == 0 { json!("auto") } else { json!(fps) });
    }
    if let Some(r) = o.keyframe_rate {
        let nr = number(
            app,
            ui,
            pos2(xv + 78.0, y),
            "maskInterp.rate",
            r,
            widgets::HotSpec { speed: 0.5, range: (0.1, 1000.0), decimals: 1, suffix: " per second" },
        );
        if (nr - r).abs() > 1e-9 {
            changes.insert("keyframeRate".into(), json!(nr));
        }
    } else {
        p.text(pos2(xv + 78.0, y + 9.0), Align2::LEFT_CENTER, crate::i18n::tr("per second"), Tokens::ui(12.0), t.text_dim);
    }
    y += ROW;
    let mut check = |app: &mut EffectcraftApp, y: f32, key: &str, text: &str, on: bool| {
        if checkbox(app, ui, pos2(x0, y + 2.0), &format!("maskInterp.{key}"), text, on) != on {
            changes.insert(key.into(), json!(!on));
        }
    };
    check(app, y, "keyframeFields", "Keyframe Fields (doubles rate)", o.keyframe_fields);
    y += ROW;
    check(app, y, "linearVertexPaths", "Use \"Linear\" Vertex Paths", o.linear_vertex_paths);
    y += ROW;
    label(&p, x0, y, "Bending Resistance:", &t);
    let br = number(
        app,
        ui,
        pos2(xv, y),
        "maskInterp.bendingResistance",
        o.bending_resistance,
        widgets::HotSpec { speed: 0.5, range: (0.0, 100.0), decimals: 0, suffix: "" },
    );
    if (br - o.bending_resistance).abs() > 1e-9 {
        changes.insert("bendingResistance".into(), json!(br));
    }
    y += ROW;
    label(&p, x0, y, "Quality:", &t);
    let q = number(app, ui, pos2(xv, y), "maskInterp.quality", o.quality, widgets::HotSpec { speed: 0.5, range: (0.0, 100.0), decimals: 0, suffix: "" });
    if (q - o.quality).abs() > 1e-9 {
        changes.insert("quality".into(), json!(q));
    }
    y += ROW;
    let add_on = o.add_vertices.is_some();
    if checkbox(app, ui, pos2(x0, y + 2.0), "maskInterp.addVerticesOn", "Add Mask Shape Vertices", add_on) != add_on {
        changes.insert("addVertices".into(), if add_on { json!(false) } else { json!(5.0) });
    }
    y += ROW;
    if let Some(v) = o.add_vertices {
        let nv =
            number(app, ui, pos2(x0 + 22.0, y), "maskInterp.addVertices", v, widgets::HotSpec { speed: 0.2, range: (0.1, 2000.0), decimals: 1, suffix: "" });
        if (nv - v).abs() > 1e-9 {
            changes.insert("addVertices".into(), json!(nv));
        }
        let units = ["pixels", "total", "percent"];
        let cur = units.iter().position(|u| *u == o.add_vertices_unit).unwrap_or(0);
        if let Some(i) = choice(
            app,
            ui,
            Rect::from_min_size(pos2(xv, y), vec2(w, 20.0)),
            "maskInterp.addVerticesUnit",
            &["Pixels Between Vertices", "Total Vertices", "Percentage of Outline"],
            cur,
        ) {
            changes.insert("addVerticesUnit".into(), json!(units[i]));
        }
    }
    y += ROW;
    label(&p, x0, y, "Matching Method:", &t);
    let methods = ["auto", "curve", "polyline"];
    let cur = methods.iter().position(|m| *m == o.matching_method).unwrap_or(0);
    if let Some(i) = choice(app, ui, Rect::from_min_size(pos2(xv, y), vec2(w, 20.0)), "maskInterp.matchingMethod", &["Auto", "Curve", "Polyline"], cur) {
        changes.insert("matchingMethod".into(), json!(methods[i]));
    }
    y += ROW;
    let mut check = |app: &mut EffectcraftApp, y: f32, key: &str, text: &str, on: bool| {
        if checkbox(app, ui, pos2(x0, y + 2.0), &format!("maskInterp.{key}"), text, on) != on {
            changes.insert(key.into(), json!(!on));
        }
    };
    check(app, y, "oneToOne", "Use 1:1 Vertex Matches", o.one_to_one);
    y += ROW;
    check(app, y, "firstVerticesMatch", "First Vertices Match", o.first_vertices_match);
    y += ROW + 8.0;
    if !changes.is_empty()
        && let Err(e) = app.session.execute("mask.interpolationOptions", serde_json::Value::Object(changes))
    {
        app.ui.status = e.to_string();
    }
    let b = Rect::from_min_size(pos2(rect.max.x - 92.0, y), vec2(80.0, 24.0));
    let enabled = app.session.state.selected_keys.len() >= 2;
    if widgets::text_button(ui, b, crate::i18n::tr("Apply"), enabled, &t, egui::Id::unique("mi-apply")).clicked() {
        match app.session.execute("mask.interpolate", json!({})) {
            Ok(r) => app.ui.status = format!("Mask Interpolation: {} keyframes added ({} vertices)", r["keys"], r["vertices"]),
            Err(e) => app.ui.status = e.to_string(),
        }
    }
    app.auto.add("maskInterp.apply", b, "Apply");
    let msg = if enabled { "Applies to the selected Mask Path keyframes" } else { "Select two or more Mask Path keyframes" };
    p.text(pos2(x0, b.max.y + 16.0), Align2::LEFT_CENTER, msg, Tokens::ui(11.0), t.text_faint);
    let _ = (Color32::WHITE, Sense::hover(), Stroke::NONE);
}

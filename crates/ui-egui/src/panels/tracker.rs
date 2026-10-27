//! Tracker panel (Window ▸ Tracker), its Motion Tracker Options / Motion Target / Apply Options
//! dialogs, and the track point widgets drawn in the viewer (feature region, search region,
//! attach point crosshair, track path) with their drag gestures.
//!
//! Everything goes through `track.*` commands, so agents can do the same.

use effectcraft_engine::geom::{Mat3, vec2 as gv2};
use effectcraft_engine::project::tracking::{LowConfidence, TrackChannel, TrackKind, TrackerOptions};
use effectcraft_engine::project::{Layer, LayerId, Uid};
use effectcraft_engine::time::Tick;
use effectcraft_engine::tracking::point_values;
use egui::{Align2, Color32, Pos2, Rect, Sense, Stroke, StrokeKind, pos2, vec2};
use serde_json::json;

use super::viewer::ViewerMap;
use crate::theme::Tokens;
use crate::{Dialog, EffectcraftApp, widgets};

const ROW: f32 = 26.0;

/// Motion Tracker Options dialog state.
#[derive(Clone, Debug, Default)]
pub struct OptionsDraft {
    pub name: String,
    pub opts: TrackerOptions,
    pub blur_on: bool,
}

/// Motion Target dialog state.
#[derive(Clone, Debug, Default)]
pub struct TargetDraft {
    pub target: Option<u64>,
}

/// Apply Options dialog state (0 = X and Y, 1 = X only, 2 = Y only).
#[derive(Clone, Debug, Default)]
pub struct ApplyDraft {
    pub dims: usize,
}

/// The current track: (tracked layer, tracker uid), resolved like the engine does.
fn current(app: &EffectcraftApp) -> Option<(LayerId, Uid)> {
    let comp = app.session.active_comp()?;
    if let Some((l, t)) = app.session.state.current_track
        && comp.layer(l).and_then(|l| l.tracker(t)).is_some()
    {
        return Some((l, t));
    }
    let src = motion_source(app)?;
    let l = comp.layer(src)?;
    l.trackers().next().map(|(g, _)| (l.id, g.uid))
}

/// The `egui::Id` of the Motion Source dropdown's transient state; picking a new source removes
/// whatever is stored under it, so the choice starts fresh.
fn source_id() -> egui::Id {
    egui::Id::unique("tracker-motion-source")
}

/// Motion Source: the current track's layer, the chosen source, or the selected layer.
fn motion_source(app: &EffectcraftApp) -> Option<LayerId> {
    let comp = app.session.active_comp()?;
    if let Some((l, _)) = app.session.state.current_track
        && comp.layer(l).is_some()
    {
        return Some(l);
    }
    let chosen: Option<u64> = app.ui.tracker_source;
    if let Some(l) = chosen.map(LayerId).filter(|l| comp.layer(*l).is_some()) {
        return Some(l);
    }
    app.session.state.selected_layers.first().copied().filter(|l| comp.layer(*l).is_some_and(trackable))
}

fn trackable(l: &Layer) -> bool {
    use effectcraft_engine::project::LayerSource;
    l.has_video() && !matches!(l.source, LayerSource::Text | LayerSource::Shape)
}

fn label(p: &egui::Painter, pos: Pos2, s: &str, t: &Tokens) {
    p.text(pos, Align2::LEFT_CENTER, s, Tokens::ui(12.0), t.text_dim);
}

/// A button that can be disabled (drawn dim and inert).
fn button(app: &mut EffectcraftApp, ui: &mut egui::Ui, r: Rect, text: &str, enabled: bool, id: &str) -> bool {
    let t = app.tokens;
    app.auto.add(id, r, text);
    if !enabled {
        ui.painter().rect_filled(r, r.height() / 2.0, Color32::from_rgb(0x2a, 0x2a, 0x2a));
        ui.painter().text(r.center(), Align2::CENTER_CENTER, text, Tokens::medium(12.0), t.text_faint);
        return false;
    }
    widgets::text_button(ui, r, text, false, &t, egui::Id::unique(id)).clicked()
}

fn run(app: &mut EffectcraftApp, ctx: &egui::Context, cmd: &str, p: serde_json::Value) {
    if let Err(e) = crate::menus::invoke(app, ctx, cmd, p) {
        app.ui.status = e;
    }
}

/// Analyze button glyphs: ◀| ◀ ▶ |▶ (and ■ while analysing in that direction).
fn analyze_glyph(p: &egui::Painter, r: Rect, forward: bool, frame: bool, stop: bool, col: Color32) {
    let c = r.center();
    if stop {
        p.rect_filled(Rect::from_center_size(c, vec2(9.0, 9.0)), 1.0, col);
        return;
    }
    let s = if forward { 1.0 } else { -1.0 };
    let tri = vec![pos2(c.x - 4.0 * s, c.y - 5.0), pos2(c.x + 4.0 * s, c.y), pos2(c.x - 4.0 * s, c.y + 5.0)];
    p.add(egui::Shape::convex_polygon(tri, col, Stroke::NONE));
    if frame {
        let x = c.x + 5.5 * s;
        p.line_segment([pos2(x, c.y - 5.0), pos2(x, c.y + 5.0)], Stroke::new(1.6, col));
    }
}

/// The Tracker panel: the camera/warp and motion/stabilize buttons, Motion Source and Current
/// Track pickers, the track type and its channels, Motion Target with Edit Target and Options, the
/// four Analyze buttons (the one whose direction is running offers Stop), Reset and Apply, and the
/// analysis progress. With a mask selected it switches to its mask-tracking mode. Every change runs a
/// `track.*` command; while an analysis is running it polls the job and asks for repaints.
pub fn show(app: &mut EffectcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let t = app.tokens;
    let ctx = ui.ctx().clone();
    if app.session.track_job.is_some() {
        app.session.poll_track();
        ctx.request_repaint_after(std::time::Duration::from_millis(50));
    }
    let p = ui.painter().with_clip_rect(rect);
    let x0 = rect.min.x + 10.0;
    let w = (rect.width() - 20.0).max(120.0);
    let half = (w - 6.0) / 2.0;
    let mut y = rect.min.y + 10.0;
    let has_comp = app.session.active_comp().is_some();
    let source = motion_source(app);
    let cur = current(app);
    let can_track = has_comp && source.is_some();

    // Track Camera / Warp Stabilizer and Track Motion / Stabilize Motion.
    let r1 = Rect::from_min_size(pos2(x0, y), vec2(half, 22.0));
    let r2 = Rect::from_min_size(pos2(x0 + half + 6.0, y), vec2(half, 22.0));
    let src_p = source.map(|l| json!({"layer": l.0})).unwrap_or_else(|| json!({}));
    let can_camera = app.session.is_enabled("track.camera") && !app.session.is_camera_analyzing();
    if button(app, ui, r1, "Track Camera", can_camera, "tracker.trackCamera") {
        run(app, &ctx, "track.camera", json!({}));
    }
    let can_warp = app.session.is_enabled("track.warpStabilizer") && !app.session.is_warp_analyzing();
    if button(app, ui, r2, "Warp Stabilizer", can_warp, "tracker.warpStabilizer") {
        run(app, &ctx, "track.warpStabilizer", json!({}));
    }
    y += 28.0;
    let r1 = Rect::from_min_size(pos2(x0, y), vec2(half, 22.0));
    let r2 = Rect::from_min_size(pos2(x0 + half + 6.0, y), vec2(half, 22.0));
    if button(app, ui, r1, "Track Motion", can_track, "tracker.trackMotion") {
        run(app, &ctx, "track.motion", src_p.clone());
    }
    if button(app, ui, r2, "Stabilize Motion", can_track, "tracker.stabilizeMotion") {
        run(app, &ctx, "track.stabilize", src_p.clone());
    }
    y += 34.0;
    let Some(comp) = app.session.active_comp_arc() else {
        p.text(pos2(x0, y + 8.0), Align2::LEFT_CENTER, crate::i18n::tr("Open a composition to track motion."), Tokens::ui(12.0), t.text_faint);
        return;
    };
    // A selected mask switches the panel to mask tracking.
    if let Some((ml, mu)) = effectcraft_engine::commands::mask_interp::selected_mask(&app.session) {
        mask_mode(app, ui, &ctx, &p, rect, y, ml, mu);
        return;
    }
    let field_x = x0 + 100.0;
    let field_w = (w - 100.0).max(80.0);

    // Motion Source.
    label(&p, pos2(x0, y + 10.0), "Motion Source:", &t);
    let srcs: Vec<(LayerId, String)> =
        comp.layers.iter().enumerate().filter(|(_, l)| trackable(l)).map(|(i, l)| (l.id, format!("{}. {}", i + 1, l.name))).collect();
    let dd = Rect::from_min_size(pos2(field_x, y), vec2(field_w, 20.0));
    let name = source.and_then(|s| srcs.iter().find(|(l, _)| *l == s)).map(|(_, n)| n.clone()).unwrap_or_else(|| "None".into());
    let did = egui::Id::unique("tracker-source-dd");
    if widgets::dropdown(ui, dd, &name, &t, did).clicked() {
        widgets::open_popup(ui, did);
    }
    app.auto.add("tracker.motionSource", dd, "Motion Source");
    let mut opts: Vec<String> = vec!["None".into()];
    opts.extend(srcs.iter().map(|(_, n)| n.clone()));
    let cur_i = source.and_then(|s| srcs.iter().position(|(l, _)| *l == s)).map(|i| i + 1).or(Some(0));
    if let Some(i) = widgets::popup_menu(ui, did, dd.left_bottom(), &opts, cur_i) {
        let chosen = i.checked_sub(1).and_then(|i| srcs.get(i)).map(|(l, _)| *l);
        app.ui.tracker_source = chosen.map(|l| l.0);
        ctx.data_mut(|d| d.remove::<u64>(source_id()));
        match chosen.and_then(|l| comp.layer(l)).and_then(|l| l.trackers().next().map(|(g, _)| (l.id, g.uid))) {
            Some((l, g)) => run(app, &ctx, "track.select", json!({"layer": l.0, "tracker": g})),
            None => app.session.state.current_track = None,
        }
    }
    y += ROW;

    // Current Track.
    label(&p, pos2(x0, y + 10.0), "Current Track:", &t);
    let trackers: Vec<(Uid, String)> =
        source.and_then(|s| comp.layer(s)).map(|l| l.trackers().map(|(g, _)| (g.uid, g.name.clone())).collect()).unwrap_or_default();
    let dd = Rect::from_min_size(pos2(field_x, y), vec2(field_w, 20.0));
    let cname = cur.and_then(|(_, u)| trackers.iter().find(|(g, _)| *g == u)).map(|(_, n)| n.clone()).unwrap_or_else(|| "None".into());
    let did = egui::Id::unique("tracker-current-dd");
    if widgets::dropdown(ui, dd, &cname, &t, did).clicked() && !trackers.is_empty() {
        widgets::open_popup(ui, did);
    }
    app.auto.add("tracker.currentTrack", dd, "Current Track");
    let names: Vec<String> = trackers.iter().map(|(_, n)| n.clone()).collect();
    let ci = cur.and_then(|(_, u)| trackers.iter().position(|(g, _)| *g == u));
    if let Some(i) = widgets::popup_menu(ui, did, dd.left_bottom(), &names, ci)
        && let (Some(src), Some((uid, _))) = (source, trackers.get(i))
    {
        run(app, &ctx, "track.select", json!({"layer": src.0, "tracker": uid}));
    }
    y += ROW;

    let settings = cur.and_then(|(l, u)| comp.layer(l).and_then(|l| l.tracker(u)).map(|(_, s)| s.clone()));
    let enabled = settings.is_some();
    let addr = cur.map(|(l, u)| json!({"layer": l.0, "tracker": u})).unwrap_or_else(|| json!({}));
    let with = |extra: serde_json::Value| {
        let mut v = addr.clone();
        if let (Some(o), Some(e)) = (v.as_object_mut(), extra.as_object()) {
            for (k, x) in e {
                o.insert(k.clone(), x.clone());
            }
        }
        v
    };

    // Track Type.
    label(&p, pos2(x0, y + 10.0), "Track Type:", &t);
    let dd = Rect::from_min_size(pos2(field_x, y), vec2(field_w, 20.0));
    let kind = settings.as_ref().map(|s| s.kind).unwrap_or_default();
    let did = egui::Id::unique("tracker-type-dd");
    if widgets::dropdown(ui, dd, if enabled { kind.label() } else { "" }, &t, did).clicked() && enabled {
        widgets::open_popup(ui, did);
    }
    app.auto.add("tracker.trackType", dd, "Track Type");
    let kinds: Vec<String> = TrackKind::ALL.iter().map(|k| k.label().to_string()).collect();
    if let Some(i) = widgets::popup_menu(ui, did, dd.left_bottom(), &kinds, TrackKind::ALL.iter().position(|k| *k == kind)) {
        run(app, &ctx, "track.setType", with(json!({"kind": TrackKind::ALL[i].id()})));
    }
    y += ROW;

    // Position / Rotation / Scale.
    let transformish = matches!(kind, TrackKind::Transform | TrackKind::Stabilize) && enabled;
    let mut xx = x0;
    for (key, text, on) in [
        ("position", "Position", settings.as_ref().is_some_and(|s| s.position)),
        ("rotation", "Rotation", settings.as_ref().is_some_and(|s| s.rotation)),
        ("scale", "Scale", settings.as_ref().is_some_and(|s| s.scale)),
    ] {
        let cr = Rect::from_min_size(pos2(xx, y + 2.0), vec2(16.0, 16.0));
        let resp = widgets::checkbox(ui, cr, on && transformish, &t, egui::Id::unique(("tracker-cb", key)));
        app.auto.add(&format!("tracker.{key}"), cr, text);
        p.text(pos2(cr.max.x + 4.0, cr.center().y), Align2::LEFT_CENTER, text, Tokens::ui(12.0), if transformish { t.text } else { t.text_faint });
        if resp.clicked() && transformish {
            run(app, &ctx, "track.setType", with(json!({ key: !on })));
        }
        xx += 84.0;
    }
    y += ROW;

    // Motion Target.
    let target_name = settings
        .as_ref()
        .and_then(|s| s.target)
        .and_then(|l| comp.layer(l).map(|l| format!("{}. {}", comp.layers.iter().position(|x| x.id == l.id).unwrap_or(0) + 1, l.name)));
    label(&p, pos2(x0, y + 10.0), "Motion Target:", &t);
    let tr = Rect::from_min_size(pos2(field_x, y), vec2(field_w, 20.0));
    p.text(
        pos2(tr.min.x, tr.center().y),
        Align2::LEFT_CENTER,
        target_name.unwrap_or_else(|| if enabled && kind != TrackKind::Raw { "None".into() } else { String::new() }),
        Tokens::ui(12.0),
        t.text,
    );
    app.auto.add("tracker.motionTarget", tr, "Motion Target");
    y += ROW;
    let r1 = Rect::from_min_size(pos2(x0, y), vec2(half, 22.0));
    let r2 = Rect::from_min_size(pos2(x0 + half + 6.0, y), vec2(half, 22.0));
    if button(app, ui, r1, "Edit Target...", enabled && kind != TrackKind::Raw && kind != TrackKind::Stabilize, "tracker.editTarget") {
        run(app, &ctx, "track.editTargetDialog", json!({}));
    }
    if button(app, ui, r2, "Options...", enabled, "tracker.options") {
        run(app, &ctx, "track.optionsDialog", json!({}));
    }
    y += 32.0;

    // Analyze.
    label(&p, pos2(x0, y + 11.0), "Analyze:", &t);
    let tracking = app.session.is_tracking();
    let running_dir = app.session.track_job.as_ref().map(|j| j.direction);
    let mut bx = field_x;
    for (dir, fwd, frame, id, tip) in [
        (effectcraft_engine::tracking::Direction::FrameBackward, false, true, "frameBackward", "Analyze 1 frame backward"),
        (effectcraft_engine::tracking::Direction::Backward, false, false, "backward", "Analyze backward"),
        (effectcraft_engine::tracking::Direction::Forward, true, false, "forward", "Analyze forward"),
        (effectcraft_engine::tracking::Direction::FrameForward, true, true, "frameForward", "Analyze 1 frame forward"),
    ] {
        let r = Rect::from_min_size(pos2(bx, y), vec2(30.0, 22.0));
        let resp = ui.interact(r, egui::Id::unique(("tracker-analyze", id)), Sense::click()).on_hover_text(tip);
        let stop = tracking && running_dir == Some(dir) && !frame;
        let on = enabled && (!tracking || stop);
        p.rect_filled(r, 4.0, if resp.hovered() && on { t.hover } else { t.field_bg });
        p.rect_stroke(r, 4.0, Stroke::new(1.0, t.field_border), StrokeKind::Inside);
        analyze_glyph(&p, r, fwd, frame, stop, if on { t.text } else { t.text_faint });
        app.auto.add(&format!("tracker.analyze.{id}"), r, tip);
        if resp.clicked() && on {
            if stop {
                run(app, &ctx, "track.stop", json!({}));
            } else {
                run(app, &ctx, "track.analyze", with(json!({ "direction": id })));
            }
        }
        bx += 34.0;
    }
    y += 32.0;

    // Reset / Apply.
    let r1 = Rect::from_min_size(pos2(x0, y), vec2(half, 22.0));
    let r2 = Rect::from_min_size(pos2(x0 + half + 6.0, y), vec2(half, 22.0));
    if button(app, ui, r1, "Reset", enabled && !tracking, "tracker.reset") {
        run(app, &ctx, "track.reset", addr.clone());
    }
    if button(app, ui, r2, "Apply", enabled && !tracking && kind != TrackKind::Raw, "tracker.apply") {
        if matches!(kind, TrackKind::Transform | TrackKind::Stabilize) {
            app.dialog_state.track_apply = ApplyDraft::default();
            app.dialog = Some(Dialog::TrackApply);
        } else {
            run(app, &ctx, "track.apply", addr.clone());
        }
    }
    y += 32.0;

    // Progress.
    if let Some(pr) = app.session.track_progress() {
        let bar = Rect::from_min_size(pos2(x0, y), vec2(w, 6.0));
        p.rect_filled(bar, 3.0, t.field_bg);
        let f = if pr.total > 0 { pr.done as f32 / pr.total as f32 } else { 0.0 };
        p.rect_filled(Rect::from_min_size(bar.min, vec2(bar.width() * f.clamp(0.0, 1.0), 6.0)), 3.0, t.accent);
        app.auto.add("tracker.progress", bar, "Analysis progress");
        p.text(
            pos2(x0, y + 18.0),
            Align2::LEFT_CENTER,
            format!("Analyzing {} / {} frames  ({:.0} fps)", pr.done, pr.total, pr.fps),
            Tokens::ui(11.0),
            t.text_dim,
        );
    }
}

/// The Tracker panel in mask mode: Method and the four track buttons (◀| ◀ ▶ |▶).
#[allow(clippy::too_many_arguments)]
fn mask_mode(app: &mut EffectcraftApp, ui: &mut egui::Ui, ctx: &egui::Context, p: &egui::Painter, rect: Rect, mut y: f32, layer: LayerId, mask: Uid) {
    use effectcraft_engine::mask_track::MaskMethod;
    let t = app.tokens;
    let x0 = rect.min.x + 10.0;
    let w = (rect.width() - 20.0).max(120.0);
    let field_x = x0 + 100.0;
    let field_w = (w - 100.0).max(80.0);
    let name = app
        .session
        .active_comp()
        .and_then(|c| c.layer(layer))
        .and_then(|l| l.props.find_group(mask).map(|g| format!("{} ({})", g.name, l.name)))
        .unwrap_or_default();
    label(p, pos2(x0, y + 10.0), "Track Mask:", &t);
    p.text(pos2(field_x, y + 10.0), Align2::LEFT_CENTER, name, Tokens::ui(12.0), t.text);
    y += ROW;
    label(p, pos2(x0, y + 10.0), "Method:", &t);
    let method = app.session.state.mask_track_method;
    let dd = Rect::from_min_size(pos2(field_x, y), vec2(field_w, 20.0));
    let did = egui::Id::unique("tracker-mask-method-dd");
    if widgets::dropdown(ui, dd, method.label(), &t, did).clicked() {
        widgets::open_popup(ui, did);
    }
    app.auto.add("tracker.mask.method", dd, "Method");
    let labels: Vec<String> = MaskMethod::ALL.iter().map(|m| m.label().to_string()).collect();
    if let Some(i) = widgets::popup_menu(ui, did, dd.left_bottom(), &labels, MaskMethod::ALL.iter().position(|m| *m == method)) {
        run(app, ctx, "track.maskMethod", json!({"method": MaskMethod::ALL[i].id()}));
    }
    y += ROW + 6.0;
    label(p, pos2(x0, y + 11.0), "Track:", &t);
    let running = app.session.is_mask_tracking();
    let running_dir = app.session.mask_job.as_ref().map(|j| j.direction);
    let mut bx = field_x;
    for (dir, fwd, frame, id, tip) in [
        (effectcraft_engine::tracking::Direction::FrameBackward, false, true, "frameBackward", "Track selected masks 1 frame backward"),
        (effectcraft_engine::tracking::Direction::Backward, false, false, "backward", "Track selected masks backward"),
        (effectcraft_engine::tracking::Direction::Forward, true, false, "forward", "Track selected masks forward"),
        (effectcraft_engine::tracking::Direction::FrameForward, true, true, "frameForward", "Track selected masks 1 frame forward"),
    ] {
        let r = Rect::from_min_size(pos2(bx, y), vec2(30.0, 22.0));
        let resp = ui.interact(r, egui::Id::unique(("tracker-mask-track", id)), Sense::click()).on_hover_text(tip);
        let stop = running && running_dir == Some(dir) && !frame;
        let on = !running || stop;
        p.rect_filled(r, 4.0, if resp.hovered() && on { t.hover } else { t.field_bg });
        p.rect_stroke(r, 4.0, Stroke::new(1.0, t.field_border), StrokeKind::Inside);
        analyze_glyph(p, r, fwd, frame, stop, if on { t.text } else { t.text_faint });
        app.auto.add(&format!("tracker.mask.track.{id}"), r, tip);
        if resp.clicked() && on {
            if stop {
                run(app, ctx, "track.stop", json!({}));
            } else {
                run(app, ctx, "track.mask", json!({"layer": layer.0, "mask": mask, "direction": id}));
            }
        }
        bx += 34.0;
    }
    y += 34.0;
    // Face Tracking (Detailed Features): Extract & Copy Face Measurements.
    let has_points = app
        .session
        .active_comp()
        .and_then(|c| c.layer(layer))
        .and_then(|l| l.effects())
        .is_some_and(|fx| fx.groups().any(|g| g.match_id == effectcraft_engine::effects::face_track::POINTS_ID));
    if method == MaskMethod::FaceDetailed || has_points {
        let r = Rect::from_min_size(pos2(x0, y), vec2(w.min(260.0), 24.0));
        if button(app, ui, r, "Extract & Copy Face Measurements", has_points && !running, "tracker.mask.extractFace") {
            match crate::menus::invoke(app, ctx, "track.extractFaceMeasurements", json!({"layer": layer.0})) {
                Ok(v) => {
                    if let Some(text) = v.get("clipboard").and_then(|t| t.as_str()) {
                        ctx.copy_text(text.to_string());
                    }
                }
                Err(e) => app.ui.status = e,
            }
        }
        y += 32.0;
    }
    if let Some(pr) = app.session.mask_track_progress() {
        let bar = Rect::from_min_size(pos2(x0, y), vec2(w, 6.0));
        p.rect_filled(bar, 3.0, t.field_bg);
        let f = if pr.total > 0 { pr.done as f32 / pr.total as f32 } else { 0.0 };
        p.rect_filled(Rect::from_min_size(bar.min, vec2(bar.width() * f.clamp(0.0, 1.0), 6.0)), 3.0, t.accent);
        app.auto.add("tracker.mask.progress", bar, "Mask tracking progress");
        p.text(
            pos2(x0, y + 18.0),
            Align2::LEFT_CENTER,
            format!("Tracking {} / {} frames  ({:.0} fps)", pr.done, pr.total, pr.fps),
            Tokens::ui(11.0),
            t.text_dim,
        );
        ctx.request_repaint_after(std::time::Duration::from_millis(50));
    } else {
        let hint = if method.is_face() {
            "Finds the face inside the mask and keys its outline (and, with Detailed Features, Face Track Points)."
        } else {
            "Tracks the pixels inside the mask and keys its Mask Path."
        };
        p.text(pos2(x0, y + 8.0), Align2::LEFT_CENTER, hint, Tokens::ui(11.0), t.text_faint);
    }
}

// ---------------------------------------------------------------- dialogs

pub fn open_options(app: &mut EffectcraftApp) -> Result<(), String> {
    let (l, u) = current(app).ok_or("no current track")?;
    let comp = app.session.active_comp().ok_or("no composition")?;
    let (g, s) = comp.layer(l).and_then(|l| l.tracker(u)).ok_or("no tracker")?;
    app.dialog_state.track_options = OptionsDraft { name: g.name.clone(), opts: s.options.clone(), blur_on: s.options.blur > 0.0 };
    if app.dialog_state.track_options.opts.blur <= 0.0 {
        app.dialog_state.track_options.opts.blur = 2.0;
    }
    app.dialog = Some(Dialog::TrackOptions);
    Ok(())
}

pub fn open_target(app: &mut EffectcraftApp) -> Result<(), String> {
    let (l, u) = current(app).ok_or("no current track")?;
    let comp = app.session.active_comp().ok_or("no composition")?;
    let (_, s) = comp.layer(l).and_then(|l| l.tracker(u)).ok_or("no tracker")?;
    app.dialog_state.track_target = TargetDraft { target: s.target.map(|l| l.0) };
    app.dialog = Some(Dialog::TrackTarget);
    Ok(())
}

fn ok_cancel(ui: &mut egui::Ui, app: &mut EffectcraftApp, t: &Tokens, prefix: &str) -> (bool, bool) {
    let (mut ok, mut cancel) = (false, false);
    ui.add_space(14.0);
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        let r = ui.add(egui::Button::new(egui::RichText::new(crate::i18n::tr("   OK   ")).color(Color32::WHITE)).fill(t.accent));
        app.auto.add(&format!("{prefix}.ok"), r.rect, "OK");
        ok = r.clicked();
        let r = ui.button(crate::i18n::tr("Cancel"));
        app.auto.add(&format!("{prefix}.cancel"), r.rect, "Cancel");
        cancel = r.clicked();
    });
    (ok, cancel)
}

pub fn options_dialog(app: &mut EffectcraftApp, ctx: &egui::Context, t: &Tokens) {
    let mut d = app.dialog_state.track_options.clone();
    let (mut ok, mut cancel) = (false, false);
    super::dialogs::modal(ctx, "Motion Tracker Options", vec2(460.0, 470.0), t, |ui| {
        ui.horizontal(|ui| {
            ui.label(crate::i18n::tr("Track Name:"));
            let r = ui.text_edit_singleline(&mut d.name);
            app.auto.add("dialog.trackOptions.name", r.rect, "Track Name");
        });
        ui.add_space(8.0);
        ui.label(egui::RichText::new(crate::i18n::tr("Channel")).font(Tokens::medium(12.5)));
        ui.horizontal(|ui| {
            for (c, l) in [(TrackChannel::Rgb, "RGB"), (TrackChannel::Luminance, "Luminance"), (TrackChannel::Saturation, "Saturation")] {
                let r = ui.radio_value(&mut d.opts.channel, c, l);
                app.auto.add(&format!("dialog.trackOptions.channel.{}", l.to_ascii_lowercase()), r.rect, l);
            }
        });
        ui.add_space(8.0);
        ui.label(egui::RichText::new(crate::i18n::tr("Process Before Match")).font(Tokens::medium(12.5)));
        ui.horizontal(|ui| {
            let r = ui.checkbox(&mut d.blur_on, crate::i18n::tr("Blur"));
            app.auto.add("dialog.trackOptions.blurOn", r.rect, "Blur");
            let r =
                ui.add_enabled(d.blur_on, crate::widgets::drag_value(egui::DragValue::new(&mut d.opts.blur).speed(0.1).range(0.5..=50.0).suffix(" pixels")));
            app.auto.add("dialog.trackOptions.blur", r.rect, "Blur pixels");
            let r = ui.checkbox(&mut d.opts.enhance, crate::i18n::tr("Enhance"));
            app.auto.add("dialog.trackOptions.enhance", r.rect, "Enhance");
        });
        ui.add_space(8.0);
        let r = ui.checkbox(&mut d.opts.subpixel, crate::i18n::tr("Subpixel Positioning"));
        app.auto.add("dialog.trackOptions.subpixel", r.rect, "Subpixel Positioning");
        let r = ui.checkbox(&mut d.opts.adapt_every_frame, crate::i18n::tr("Adapt Feature On Every Frame"));
        app.auto.add("dialog.trackOptions.adaptEveryFrame", r.rect, "Adapt Feature On Every Frame");
        let r = ui.checkbox(&mut d.opts.track_shape, crate::i18n::tr("Follow Feature Rotation and Scale"));
        app.auto.add("dialog.trackOptions.trackShape", r.rect, "Follow Feature Rotation and Scale");
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            let actions = [
                (LowConfidence::Continue, "Continue Tracking"),
                (LowConfidence::Stop, "Stop Tracking"),
                (LowConfidence::Extrapolate, "Extrapolate Motion"),
                (LowConfidence::Adapt, "Adapt Feature"),
            ];
            let cur = actions.iter().find(|(a, _)| *a == d.opts.action).map(|(_, l)| *l).unwrap_or("Adapt Feature");
            let r = craft_ui::ComboBox::from_id_salt("track-action")
                .selected_text(cur)
                .show_ui(ui, |ui| {
                    for (a, l) in actions {
                        ui.selectable_value(&mut d.opts.action, a, l);
                    }
                })
                .response;
            app.auto.add("dialog.trackOptions.action", r.rect, "If Confidence is Below action");
            ui.label(crate::i18n::tr("If Confidence is Below"));
            let r = ui.add(crate::widgets::drag_value(egui::DragValue::new(&mut d.opts.threshold).speed(0.5).range(0.0..=100.0).suffix(" %")));
            app.auto.add("dialog.trackOptions.threshold", r.rect, "Confidence threshold");
        });
        (ok, cancel) = ok_cancel(ui, app, t, "dialog.trackOptions");
    });
    app.dialog_state.track_options = d.clone();
    if ok || super::dialogs::ui_enter(ctx) {
        let o = &d.opts;
        let action = match o.action {
            LowConfidence::Continue => "continue",
            LowConfidence::Stop => "stop",
            LowConfidence::Extrapolate => "extrapolate",
            LowConfidence::Adapt => "adapt",
        };
        let channel = match o.channel {
            TrackChannel::Rgb => "rgb",
            TrackChannel::Luminance => "luminance",
            TrackChannel::Saturation => "saturation",
        };
        let p = json!({
            "name": d.name, "channel": channel, "blur": if d.blur_on { o.blur } else { 0.0 }, "enhance": o.enhance,
            "subpixel": o.subpixel, "adaptEveryFrame": o.adapt_every_frame, "threshold": o.threshold, "action": action, "trackShape": o.track_shape,
        });
        if let Err(e) = app.session.execute("track.options", p) {
            app.ui.status = e.to_string();
        }
        cancel = true;
    }
    if cancel {
        app.dialog = None;
    }
}

pub fn target_dialog(app: &mut EffectcraftApp, ctx: &egui::Context, t: &Tokens) {
    let mut d = app.dialog_state.track_target.clone();
    let (mut ok, mut cancel) = (false, false);
    let layers: Vec<(u64, String)> =
        app.session.active_comp().map(|c| c.layers.iter().enumerate().map(|(i, l)| (l.id.0, format!("{}. {}", i + 1, l.name))).collect()).unwrap_or_default();
    super::dialogs::modal(ctx, "Motion Target", vec2(400.0, 200.0), t, |ui| {
        ui.label(crate::i18n::tr("Apply Motion To:"));
        ui.horizontal(|ui| {
            ui.label(crate::i18n::tr("Layer:"));
            let cur = d.target.and_then(|id| layers.iter().find(|(l, _)| *l == id)).map(|(_, n)| n.clone()).unwrap_or_else(|| "None".into());
            let r = craft_ui::ComboBox::from_id_salt("track-target")
                .selected_text(cur)
                .width(240.0)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut d.target, None, crate::i18n::tr("None"));
                    for (id, n) in &layers {
                        ui.selectable_value(&mut d.target, Some(*id), n);
                    }
                })
                .response;
            app.auto.add("dialog.trackTarget.layer", r.rect, "Layer");
        });
        (ok, cancel) = ok_cancel(ui, app, t, "dialog.trackTarget");
    });
    app.dialog_state.track_target = d.clone();
    if ok || super::dialogs::ui_enter(ctx) {
        if let Err(e) = app.session.execute("track.setTarget", json!({"target": d.target})) {
            app.ui.status = e.to_string();
        }
        cancel = true;
    }
    if cancel {
        app.dialog = None;
    }
}

pub fn apply_dialog(app: &mut EffectcraftApp, ctx: &egui::Context, t: &Tokens) {
    let mut d = app.dialog_state.track_apply.clone();
    let (mut ok, mut cancel) = (false, false);
    super::dialogs::modal(ctx, "Motion Tracker Apply Options", vec2(380.0, 170.0), t, |ui| {
        ui.horizontal(|ui| {
            ui.label(crate::i18n::tr("Apply Dimensions:"));
            let labels = ["X and Y", "X only", "Y only"];
            let r = craft_ui::ComboBox::from_id_salt("track-apply-dims")
                .selected_text(labels[d.dims.min(2)])
                .show_ui(ui, |ui| {
                    for (i, l) in labels.iter().enumerate() {
                        ui.selectable_value(&mut d.dims, i, *l);
                    }
                })
                .response;
            app.auto.add("dialog.trackApply.dimensions", r.rect, "Apply Dimensions");
        });
        (ok, cancel) = ok_cancel(ui, app, t, "dialog.trackApply");
    });
    app.dialog_state.track_apply = d.clone();
    if ok || super::dialogs::ui_enter(ctx) {
        let dims = ["xy", "x", "y"][d.dims.min(2)];
        if let Err(e) = app.session.execute("track.apply", json!({"dimensions": dims})) {
            app.ui.status = e.to_string();
        }
        cancel = true;
    }
    if cancel {
        app.dialog = None;
    }
}

// ---------------------------------------------------------------- viewer widgets

/// Which part of a track point a drag grabbed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    /// Inside the feature region: move the whole point.
    Point,
    /// Inside the search region: move only the search region.
    Search,
    /// A feature region corner (0..4: UL, UR, LR, LL).
    FeatureCorner(usize),
    SearchCorner(usize),
    Attach,
}

/// A track point widget hit area (screen space).
#[derive(Clone, Copy, Debug)]
pub struct Hit {
    pub point: usize,
    pub part: Part,
    pub rect: Rect,
    /// Screen quad for inside tests (feature / search region).
    pub quad: Option<[Pos2; 4]>,
}

/// A track point drag in progress.
#[derive(Clone, Debug)]
pub struct Drag {
    pub point: usize,
    pub part: Part,
    /// Comp → tracked-layer matrix and the layer-space press point.
    pub inv: Mat3,
    pub start: [f64; 2],
    pub center: [f64; 2],
    pub feature: [f64; 2],
    pub search_offset: [f64; 2],
    pub search: [f64; 2],
    pub attach: [f64; 2],
}

const TRACK_COL: Color32 = Color32::from_rgb(0xf0, 0xc8, 0x3c);

fn quad_contains(q: &[Pos2; 4], p: Pos2) -> bool {
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

/// Draw the current track's points (layer → comp matrix `m`) and return their hit areas, from the
/// most to the least specific.
pub fn draw_overlay(app: &mut EffectcraftApp, painter: &egui::Painter, map: &ViewerMap, m: &Mat3, layer: &Layer, tracker: Uid, comp_time: Tick) -> Vec<Hit> {
    let Some((g, _)) = layer.tracker(tracker) else { return vec![] };
    let lt = layer.layer_time(comp_time);
    let scr = |p: [f64; 2]| {
        let c = m.apply(gv2(p[0], p[1]));
        map.to_screen([c.x, c.y])
    };
    let mut hits = vec![];
    let mut inside = vec![];
    for (i, tp) in g.track_points().enumerate() {
        let v = point_values(tp, lt);
        let c = v.spec.center;
        let attach = tp
            .get("attachPoint")
            .filter(|p| !p.keys.is_empty())
            .map(|p| p.value_at(lt).as_vec2())
            .unwrap_or([c[0] + v.attach_offset[0], c[1] + v.attach_offset[1]]);
        let rect_q = |cx: [f64; 2], s: [f64; 2]| {
            let (hx, hy) = (s[0] / 2.0, s[1] / 2.0);
            [scr([cx[0] - hx, cx[1] - hy]), scr([cx[0] + hx, cx[1] - hy]), scr([cx[0] + hx, cx[1] + hy]), scr([cx[0] - hx, cx[1] + hy])]
        };
        // Track path (feature centre keys).
        if let Some(fc) = tp.get("featureCenter")
            && fc.keys.len() > 1
        {
            let pts: Vec<Pos2> = fc.keys.iter().map(|k| scr(k.value.as_vec2())).collect();
            painter.add(egui::Shape::line(pts.clone(), Stroke::new(1.0, TRACK_COL.gamma_multiply(0.6))));
            for q in &pts {
                painter.circle_filled(*q, 1.6, TRACK_COL);
            }
        }
        let fq = rect_q(c, v.spec.feature_size);
        let sc = [c[0] + v.spec.search_offset[0], c[1] + v.spec.search_offset[1]];
        let sq = rect_q(sc, v.spec.search_size);
        painter.add(egui::Shape::closed_line(sq.to_vec(), Stroke::new(1.0, TRACK_COL.gamma_multiply(0.7))));
        painter.add(egui::Shape::closed_line(fq.to_vec(), Stroke::new(1.0, TRACK_COL)));
        painter.circle_filled(scr(c), 2.0, TRACK_COL);
        let a = scr(attach);
        for d in [vec2(7.0, 0.0), vec2(0.0, 7.0)] {
            painter.line_segment([a - d, a + d], Stroke::new(1.4, Color32::BLACK));
            painter.line_segment([a - d, a + d], Stroke::new(1.0, TRACK_COL));
        }
        painter.text(sq[0] + vec2(0.0, -3.0), Align2::LEFT_BOTTOM, &tp.name, Tokens::ui(10.5), TRACK_COL);
        // Corner handles only when the region is big enough on screen to see inside it.
        let fbig = fq[0].distance(fq[2]) > 28.0;
        let sbig = sq[0].distance(sq[2]) > 28.0;
        let n = i + 1;
        // Zoomed out the feature region can be smaller than the attach point's grab area: keep the
        // grab to the region's inner half so the rest of it still grabs the point.
        let fr = Rect::from_points(&fq);
        let grab = if quad_contains(&fq, a) { 12.0f32.min(fr.width().min(fr.height()) * 0.5) } else { 12.0 };
        let hr = Rect::from_center_size(a, vec2(grab, grab));
        hits.push(Hit { point: n, part: Part::Attach, rect: hr, quad: None });
        app.auto.add(&format!("viewer.track.{n}.attach"), hr, "Attach Point");
        for (k, q) in fq.iter().enumerate() {
            let r = Rect::from_center_size(*q, vec2(5.0, 5.0));
            if fbig {
                painter.rect_filled(r, 0.0, TRACK_COL);
            }
            hits.push(Hit { point: n, part: Part::FeatureCorner(k), rect: r.expand(2.0), quad: None });
            app.auto.add(&format!("viewer.track.{n}.feature.{k}"), r, "Feature region corner");
        }
        for (k, q) in sq.iter().enumerate() {
            let r = Rect::from_center_size(*q, vec2(5.0, 5.0));
            if sbig {
                painter.rect_stroke(r, 0.0, Stroke::new(1.0, TRACK_COL), StrokeKind::Middle);
            }
            hits.push(Hit { point: n, part: Part::SearchCorner(k), rect: r.expand(2.0), quad: None });
            app.auto.add(&format!("viewer.track.{n}.search.{k}"), r, "Search region corner");
        }
        let sr = Rect::from_points(&sq);
        app.auto.add(&format!("viewer.track.{n}.feature"), fr, "Feature region");
        app.auto.add(&format!("viewer.track.{n}.search"), sr, "Search region");
        inside.push(Hit { point: n, part: Part::Point, rect: fr, quad: Some(fq) });
        inside.push(Hit { point: n, part: Part::Search, rect: sr, quad: Some(sq) });
    }
    // Regions after handles; feature regions before search regions.
    inside.sort_by_key(|h| h.part == Part::Search);
    hits.extend(inside);
    hits
}

/// The hit under `pos`.
pub fn hit_at(hits: &[Hit], pos: Pos2) -> Option<Hit> {
    hits.iter()
        .find(|h| match h.quad {
            Some(q) => quad_contains(&q, pos),
            None => h.rect.contains(pos),
        })
        .copied()
}

/// Start dragging `hit` (press at comp point `cpt`).
pub fn begin_drag(layer: &Layer, tracker: Uid, comp_time: Tick, l2c: &Mat3, hit: Hit, cpt: [f64; 2]) -> Option<Drag> {
    let (g, _) = layer.tracker(tracker)?;
    let tp = g.track_points().nth(hit.point - 1)?;
    let v = point_values(tp, layer.layer_time(comp_time));
    let inv = l2c.inverse()?;
    let s = inv.apply(gv2(cpt[0], cpt[1]));
    Some(Drag {
        point: hit.point,
        part: hit.part,
        inv,
        start: [s.x, s.y],
        center: v.spec.center,
        feature: v.spec.feature_size,
        search_offset: v.spec.search_offset,
        search: v.spec.search_size,
        attach: v.attach_offset,
    })
}

/// The `track.setPoint` parameters for the drag at comp point `cpt`.
pub fn drag_params(d: &Drag, cpt: [f64; 2]) -> serde_json::Value {
    let q = d.inv.apply(gv2(cpt[0], cpt[1]));
    let lp = [q.x, q.y];
    let dl = [lp[0] - d.start[0], lp[1] - d.start[1]];
    match d.part {
        Part::Point => json!({"point": d.point, "center": [d.center[0] + dl[0], d.center[1] + dl[1]]}),
        Part::Search => json!({"point": d.point, "searchOffset": [d.search_offset[0] + dl[0], d.search_offset[1] + dl[1]]}),
        Part::Attach => json!({"point": d.point, "attachOffset": [d.attach[0] + dl[0], d.attach[1] + dl[1]]}),
        Part::FeatureCorner(_) => {
            let size = [(2.0 * (lp[0] - d.center[0]).abs()).max(4.0), (2.0 * (lp[1] - d.center[1]).abs()).max(4.0)];
            json!({"point": d.point, "featureSize": size})
        }
        Part::SearchCorner(_) => {
            let sc = [d.center[0] + d.search_offset[0], d.center[1] + d.search_offset[1]];
            let size = [(2.0 * (lp[0] - sc[0]).abs()).max(4.0), (2.0 * (lp[1] - sc[1]).abs()).max(4.0)];
            json!({"point": d.point, "searchSize": size})
        }
    }
}

/// The tracked layer and tracker the viewer shows widgets for.
pub fn viewer_track(app: &EffectcraftApp) -> Option<(LayerId, Uid)> {
    current(app)
}

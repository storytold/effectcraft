use super::common::{Actions, gesture_key};
use super::{Align2, Color32, EffectcraftApp, EvalCtx, Layer, PropGroup, Rect, Sense, Tokens, Value, fw, json, pos2, vec2, widgets};

/// Warp Stabilizer's Analyze / Cancel buttons and analysis status.
pub(super) fn warp_editor(app: &mut EffectcraftApp, ui: &mut egui::Ui, p: &egui::Painter, layer: &Layer, g: &PropGroup, r: Rect, actions: &mut Actions) {
    let t = app.tokens;
    let running = app.session.warp_target().is_some_and(|(_, l, u)| l == layer.id && u == g.uid) && app.session.is_warp_analyzing();
    let busy = app.session.is_warp_analyzing();
    let b1 = Rect::from_min_size(pos2(r.min.x + 24.0, r.min.y + 4.0), vec2(84.0, 22.0));
    let b2 = Rect::from_min_size(pos2(b1.max.x + 8.0, b1.min.y), vec2(84.0, 22.0));
    if widgets::text_button(ui, b1, crate::i18n::tr("Analyze"), false, &t, egui::Id::unique(("warp-analyze", g.uid))).clicked() && !busy {
        actions.push(("warp.analyze".into(), json!({"layer": layer.id.0, "effect": g.uid})));
    }
    app.auto.add(&format!("effectControls.warp.{}.analyze", g.uid), b1, "Analyze");
    if running {
        if widgets::text_button(ui, b2, crate::i18n::tr("Cancel"), false, &t, egui::Id::unique(("warp-cancel", g.uid))).clicked() {
            actions.push(("warp.cancel".into(), json!({})));
        }
        app.auto.add(&format!("effectControls.warp.{}.cancel", g.uid), b2, "Cancel");
    }
    let analysed = matches!(g.get(effectcraft_engine::effects::warp_stab::ANALYSIS).map(|p| &p.value), Some(Value::Str(s)) if !s.is_empty());
    let status = match app.session.warp_progress() {
        Some(pr) if running => pr.banner(),
        _ if analysed => "Analysis complete".to_string(),
        _ if app.session.warp_pending.iter().any(|(_, l, u)| *l == layer.id && *u == g.uid) => "Waiting to analyze".to_string(),
        _ => "Not analyzed: click Analyze".to_string(),
    };
    p.text(pos2(r.min.x + 24.0, r.min.y + 38.0), Align2::LEFT_CENTER, status, Tokens::ui(11.5), t.text_dim);
}

/// Roto Brush & Refine Edge: Freeze / Unfreeze and Propagate buttons and the segmentation status.
pub(super) fn roto_editor(app: &mut EffectcraftApp, ui: &mut egui::Ui, p: &egui::Painter, layer: &Layer, g: &PropGroup, r: Rect, actions: &mut Actions) {
    use effectcraft_engine::effects::roto as fx;
    let t = app.tokens;
    let params = effectcraft_engine::effects::flatten_params(g, &mut |pr| pr.value.clone());
    let frozen = fx::is_frozen(&params);
    let d = fx::data(&params);
    let running = app.session.roto_target().is_some_and(|(_, l, u, _)| l == layer.id && u == g.uid) && app.session.is_roto_running();
    let b1 = Rect::from_min_size(pos2(r.min.x + 24.0, r.min.y + 4.0), vec2(84.0, 22.0));
    let b2 = Rect::from_min_size(pos2(b1.max.x + 8.0, b1.min.y), vec2(84.0, 22.0));
    let label = if frozen { "Unfreeze" } else { "Freeze" };
    if widgets::text_button(ui, b1, label, frozen, &t, egui::Id::unique(("roto-freeze-ec", g.uid))).clicked() && !running {
        let id = if frozen { "roto.unfreeze" } else { "roto.freeze" };
        actions.push((id.into(), json!({"layer": layer.id.0, "effect": g.uid})));
    }
    app.auto.add(&format!("effectControls.roto.{}.freeze", g.uid), b1, label);
    let (l2, id2) = if running { ("Stop", "roto.cancel") } else { ("Propagate", "roto.propagate") };
    if widgets::text_button(ui, b2, l2, false, &t, egui::Id::unique(("roto-prop-ec", g.uid))).clicked() && !frozen {
        actions.push((id2.into(), json!({"layer": layer.id.0, "effect": g.uid})));
    }
    app.auto.add(&format!("effectControls.roto.{}.propagate", g.uid), b2, l2);
    let status = match (app.session.roto_progress(), d.base) {
        (Some(pr), _) if running => pr.banner(),
        (_, None) => "No strokes: paint with the Roto Brush tool (Alt+W) in the Layer panel".to_string(),
        (_, Some(b)) => format!("Base frame {b}, span {}–{}{}", d.span[0], d.span[1], if frozen { " (frozen)" } else { "" }),
    };
    p.text(pos2(r.min.x + 24.0, r.min.y + 38.0), Align2::LEFT_CENTER, status, Tokens::ui(11.5), t.text_dim);
}

/// Object Matte: Freeze / Unfreeze and Track / Stop buttons (the background job that computes
/// the matte over the tracked frames) and its status.
pub(super) fn object_matte_editor(
    app: &mut EffectcraftApp,
    ui: &mut egui::Ui,
    p: &egui::Painter,
    layer: &Layer,
    g: &PropGroup,
    r: Rect,
    actions: &mut Actions,
) {
    use effectcraft_engine::effects::object_matte as om;
    let t = app.tokens;
    let params = effectcraft_engine::effects::flatten_params(g, &mut |pr| pr.value.clone());
    let frozen = om::is_frozen(&params);
    let mine = app.session.roto_target().is_some_and(|(_, l, u, _)| l == layer.id && u == g.uid);
    let running = mine && app.session.is_roto_running();
    let busy = app.session.is_roto_running();
    let b1 = Rect::from_min_size(pos2(r.min.x + 24.0, r.min.y + 4.0), vec2(84.0, 22.0));
    let b2 = Rect::from_min_size(pos2(b1.max.x + 8.0, b1.min.y), vec2(84.0, 22.0));
    let label = if frozen { "Unfreeze" } else { "Freeze" };
    if widgets::text_button(ui, b1, crate::i18n::tr(label), frozen, &t, egui::Id::unique(("om-freeze-ec", g.uid))).clicked() && !busy {
        let id = if frozen { "objectMatte.unfreeze" } else { "objectMatte.freeze" };
        actions.push((id.into(), json!({"layer": layer.id.0, "effect": g.uid})));
    }
    app.auto.add(&format!("effectControls.objectMatte.{}.freeze", g.uid), b1, label);
    let (l2, id2) = if running { ("Stop", "roto.cancel") } else { ("Track", "objectMatte.propagate") };
    if widgets::text_button(ui, b2, crate::i18n::tr(l2), false, &t, egui::Id::unique(("om-track-ec", g.uid))).clicked() && !frozen && (running || !busy) {
        actions.push((id2.into(), json!({"layer": layer.id.0, "effect": g.uid})));
    }
    app.auto.add(&format!("effectControls.objectMatte.{}.track", g.uid), b2, l2);
    let base = om::reference_frame(&params);
    let status = match app.session.roto_progress() {
        Some(pr) if running => pr.banner(),
        _ if frozen => format!("Frozen from reference frame {base}"),
        _ if app.session.roto_pending.iter().any(|(_, l, u)| *l == layer.id && *u == g.uid) => "Waiting to track".to_string(),
        _ => format!("Reference frame {base}: Track computes the matte ahead of playback"),
    };
    p.text(pos2(r.min.x + 24.0, r.min.y + 38.0), Align2::LEFT_CENTER, &status, Tokens::ui(11.5), t.text_dim);
    app.auto.add(
        &format!("effectControls.objectMatte.{}.status", g.uid),
        Rect::from_min_size(pos2(r.min.x + 24.0, r.min.y + 30.0), vec2(r.width() - 24.0, 16.0)),
        &status,
    );
}

// ---------------------------------------------------------------------------------------------
// Curves / Levels editors

/// Height of the custom editor shown under an effect's header (0 when none).
pub(super) fn editor_height(effect: &str, g: &PropGroup, width: f32) -> f32 {
    match effect {
        crate::panels::fx_editors::GLOW | crate::panels::fx_editors::RESHAPE | crate::panels::fx_editors::EXTRACTOR => {
            crate::panels::fx_editors::header_height(effect, g, width)
        }
        "ec.color.curves" => 34.0 + curves_size(width) + 26.0,
        effectcraft_engine::effects::warp_stab::ID => 50.0,
        effectcraft_engine::effects::camera_tracker::ID => crate::panels::camera_tracker_ui::EDITOR_HEIGHT,
        effectcraft_engine::effects::roto::ID | effectcraft_engine::effects::object_matte::ID => 50.0,
        "ec.color.levels" | "ec.color.levelsic" => 34.0 + 80.0 + 58.0,
        "ec.color.autolevels" | "ec.color.autocontrast" | "ec.color.autocolor" => 24.0 + 80.0 + 12.0,
        _ => 0.0,
    }
}

fn curves_size(width: f32) -> f32 {
    (width - 72.0).clamp(120.0, 256.0)
}

/// Cached histogram of the displayed frame (recomputed when the frame changes).
fn frame_histogram(app: &mut EffectcraftApp, ctx: &egui::Context) -> Option<std::sync::Arc<[[u32; 256]; 5]>> {
    let img = app.viewer_pixels()?;
    let key = std::sync::Arc::as_ptr(&img) as usize;
    let id = egui::Id::unique("ec-histogram");
    if let Some((k, h)) = ctx.data(|d| d.get_temp::<(usize, std::sync::Arc<[[u32; 256]; 5]>)>(id))
        && k == key
    {
        return Some(h);
    }
    let h = std::sync::Arc::new(fw::histogram(&img));
    ctx.data_mut(|d| d.insert_temp(id, (key, h.clone())));
    Some(h)
}

fn channel_color(ch: usize) -> Color32 {
    match ch {
        1 => Color32::from_rgb(0xe0, 0x50, 0x50),
        2 => Color32::from_rgb(0x50, 0xc8, 0x60),
        3 => Color32::from_rgb(0x50, 0x80, 0xf0),
        4 => Color32::from_gray(0x90),
        _ => Color32::from_gray(0xb8),
    }
}

/// A "Channel: [RGB ▾]" popup; returns the chosen channel.
#[allow(clippy::too_many_arguments)]
fn channel_popup(
    app: &mut EffectcraftApp,
    ui: &mut egui::Ui,
    p: &egui::Painter,
    x: f32,
    cy: f32,
    cur: usize,
    names: &[&str],
    id: egui::Id,
    auto: &str,
) -> Option<usize> {
    let t = app.tokens;
    p.text(pos2(x, cy), Align2::LEFT_CENTER, crate::i18n::tr("Channel:"), Tokens::ui(12.0), t.text_dim);
    let dr = Rect::from_min_size(pos2(x + 58.0, cy - 9.0), vec2(80.0, 18.0));
    let pop = id.with("pop");
    if widgets::dropdown(ui, dr, names.get(cur).copied().unwrap_or(""), &t, id).clicked() {
        widgets::open_popup(ui, pop);
    }
    app.auto.add(auto, dr, "Channel");
    let opts: Vec<String> = names.iter().map(|s| s.to_string()).collect();
    widgets::popup_menu(ui, pop, dr.left_bottom(), &opts, Some(cur))
}

/// Curves: channel popup, Bezier/pencil mode, Reset, and the graph bound to the channel's
/// hidden point-list parameter.
#[allow(clippy::too_many_arguments)]
pub(super) fn curves_editor(
    app: &mut EffectcraftApp,
    ui: &mut egui::Ui,
    p: &egui::Painter,
    layer: &Layer,
    g: &PropGroup,
    ectx: &EvalCtx,
    r: Rect,
    actions: &mut Actions,
) {
    let t = app.tokens;
    let euid = g.uid;
    // The channel lives in the effect's hidden Channel parameter (UI state for instances
    // saved before it existed).
    let chan_prop = g.get("channel");
    let ch = match chan_prop {
        Some(pr) => ectx.value(layer, pr).as_enum() as usize,
        None => app.ui.fx_curve_channel.get(&euid).copied().unwrap_or(0),
    }
    .min(4);
    let x0 = r.min.x + 30.0;
    let cy = r.min.y + 17.0;
    let names: [&str; 5] = [crate::i18n::tr("RGB"), crate::i18n::tr("Red"), crate::i18n::tr("Green"), crate::i18n::tr("Blue"), crate::i18n::tr("Alpha")];
    if let Some(n) = channel_popup(app, ui, p, x0, cy, ch, &names, egui::Id::unique(("ec-cch", euid)), &format!("effectControls.effect.{euid}.curves.channel"))
    {
        match chan_prop {
            Some(pr) => actions.push(("prop.set".into(), json!({"layer": layer.id.0, "prop": pr.uid, "value": n}))),
            None => {
                app.ui.fx_curve_channel.insert(euid, n);
            }
        }
    }
    let pencil = app.ui.fx_curve_pencil.contains(&euid);
    let mr = Rect::from_min_size(pos2(x0 + 146.0, cy - 9.0), vec2(56.0, 18.0));
    if widgets::dropdown(ui, mr, if pencil { "Pencil" } else { "Bezier" }, &t, egui::Id::unique(("ec-cmode", euid))).clicked() {
        if pencil {
            app.ui.fx_curve_pencil.remove(&euid);
        } else {
            app.ui.fx_curve_pencil.insert(euid);
        }
    }
    app.auto.add(&format!("effectControls.effect.{euid}.curves.mode"), mr, "Curve mode");
    let Some(prop) = g.get(fw::CURVE_CHANNELS[ch].0) else { return };
    let cur = match ectx.value(layer, prop) {
        Value::Str(s) => fw::CurvePoints::parse(&s),
        _ => fw::CurvePoints::identity(),
    };
    let rr = Rect::from_min_size(pos2(mr.max.x + 8.0, cy - 9.0), vec2(44.0, 18.0));
    let rresp = ui.interact(rr, egui::Id::unique(("ec-creset", euid)), Sense::click());
    p.text(rr.center(), Align2::CENTER_CENTER, crate::i18n::tr("Reset"), Tokens::ui(11.5), if rresp.hovered() { t.hot_text } else { t.text_dim });
    app.auto.add(&format!("effectControls.effect.{euid}.curves.reset"), rr, "Reset curve");
    if rresp.clicked() {
        actions.push(("prop.set".into(), json!({"layer": layer.id.0, "prop": prop.uid, "value": fw::CurvePoints::identity().format()})));
    }
    let size = curves_size(r.width());
    let gr = Rect::from_min_size(pos2(x0 + 8.0, r.min.y + 38.0), vec2(size, size));
    // Input/output gradients along the axes.
    let col = channel_color(ch);
    p.rect_filled(Rect::from_min_max(pos2(gr.min.x, gr.max.y + 3.0), pos2(gr.max.x, gr.max.y + 8.0)), 0.0, col.gamma_multiply(0.5));
    let id = egui::Id::unique(("ec-curves", euid, ch));
    let (resp, edit) = fw::curves_graph(ui, gr, &cur, ch, fw::CurveMode { pencil, natural: true }, id, &t);
    app.auto.add(&format!("effectControls.effect.{euid}.curves.graph"), gr, "Curves");
    let started = resp.drag_started() || resp.clicked();
    if let Some(np) = edit.changed {
        let key = gesture_key(ui, id, started);
        actions.push(("prop.set".into(), json!({"layer": layer.id.0, "prop": prop.uid, "value": np.format(), "merge": key})));
    }
    p.text(
        pos2(gr.min.x, gr.max.y + 18.0),
        Align2::LEFT_CENTER,
        if pencil { "Drag to draw the curve" } else { "Click adds a point; drag one off to remove" },
        Tokens::ui(10.5),
        t.text_faint,
    );
}

/// Levels parameter ids for a channel: input black, input white, gamma, output black, output white.
fn levels_ids(effect: &str, ch: usize) -> [String; 5] {
    if effect == "ec.color.levels" {
        return effectcraft_engine::effects::levels_channel_ids(ch);
    }
    // Levels (Individual Controls) nests each channel's controls in its twirl-down group.
    let pre = fw::LEVELS_CHANNELS[ch.min(4)].0;
    ["InBlack", "InWhite", "Gamma", "OutBlack", "OutWhite"].map(|s| format!("{pre}/{pre}{s}"))
}

/// Levels: (channel popup,) histogram, input black/gamma/white triangles, output bar with
/// black/white triangles.
#[allow(clippy::too_many_arguments)]
pub(super) fn levels_editor(
    app: &mut EffectcraftApp,
    ui: &mut egui::Ui,
    p: &egui::Painter,
    layer: &Layer,
    g: &PropGroup,
    effect: &str,
    ectx: &EvalCtx,
    r: Rect,
    actions: &mut Actions,
) {
    let t = app.tokens;
    let euid = g.uid;
    let x0 = r.min.x + 30.0;
    let cy = r.min.y + 17.0;
    // Levels keeps its channel in the Channel parameter; Levels (Individual Controls) shows
    // every channel and the popup only picks the histogram.
    let ch = if effect == "ec.color.levelsic" {
        app.ui.fx_levels_channel.get(&euid).copied().unwrap_or(0).min(4)
    } else {
        g.get("channel").map(|pr| ectx.value(layer, pr).as_enum() as usize).unwrap_or(0).min(4)
    };
    if effect == "ec.color.levelsic" {
        let names: [&str; 5] = [crate::i18n::tr("RGB"), crate::i18n::tr("Red"), crate::i18n::tr("Green"), crate::i18n::tr("Blue"), crate::i18n::tr("Alpha")];
        if let Some(n) =
            channel_popup(app, ui, p, x0, cy, ch, &names, egui::Id::unique(("ec-lch", euid)), &format!("effectControls.effect.{euid}.levels.channel"))
        {
            app.ui.fx_levels_channel.insert(euid, n);
        }
    } else {
        p.text(pos2(x0, cy), Align2::LEFT_CENTER, crate::i18n::tr("Histogram"), Tokens::ui(12.0), t.text_dim);
    }
    let w = (r.width() - 72.0).clamp(120.0, 256.0);
    let hr = Rect::from_min_size(pos2(x0 + 8.0, r.min.y + 34.0), vec2(w, 80.0));
    match frame_histogram(app, ui.ctx()) {
        // Channel 0 (RGB) shows luminance; the others their own channel.
        Some(h) => fw::draw_histogram(p, hr, &h[ch], channel_color(ch)),
        None => {
            p.rect_filled(hr, 0.0, Color32::from_rgb(0x1c, 0x1c, 0x1c));
        }
    }
    app.auto.add(&format!("effectControls.effect.{euid}.levels.histogram"), hr, "Histogram");
    let ids = levels_ids(effect, ch);
    let val = |id: &str| g.prop(id).map(|pr| ectx.value(layer, pr).as_f64()).unwrap_or(0.0);
    let (ib, iw, gm, ob, ow) = (val(&ids[0]), val(&ids[1]), val(&ids[2]), val(&ids[3]), val(&ids[4]));
    let mut set = |ui: &egui::Ui, id: &str, v: f64, started: bool| {
        if let Some(pr) = g.prop(id) {
            let key = gesture_key(ui, egui::Id::unique(("ec-lv", euid, id)), started);
            actions.push(("prop.set".into(), json!({"layer": layer.id.0, "prop": pr.uid, "value": v, "merge": key})));
        }
    };
    let track = Rect::from_min_size(pos2(hr.min.x, hr.max.y + 1.0), vec2(w, 10.0));
    let started = |ui: &egui::Ui| ui.input(|i| i.pointer.any_pressed());
    let (r0, nv) = fw::level_handle(ui, track, ib, Color32::BLACK, egui::Id::unique(("ec-lib", euid, ch)), &t);
    app.auto.add(&format!("effectControls.effect.{euid}.levels.inBlack"), r0, "Input Black");
    if let Some(v) = nv {
        set(ui, &ids[0], v.min(iw - 0.005), started(ui));
    }
    let gx = fw::gamma_handle(ib, iw, gm);
    let (r1, nv) = fw::level_handle(ui, track, gx, Color32::from_gray(128), egui::Id::unique(("ec-lg", euid, ch)), &t);
    app.auto.add(&format!("effectControls.effect.{euid}.levels.gamma"), r1, "Gamma");
    if let Some(v) = nv {
        set(ui, &ids[2], (fw::gamma_from_handle(ib, iw, v) * 100.0).round() / 100.0, started(ui));
    }
    let (r2, nv) = fw::level_handle(ui, track, iw, Color32::WHITE, egui::Id::unique(("ec-liw", euid, ch)), &t);
    app.auto.add(&format!("effectControls.effect.{euid}.levels.inWhite"), r2, "Input White");
    if let Some(v) = nv {
        set(ui, &ids[1], v.max(ib + 0.005), started(ui));
    }
    // Output: a black→white bar with its two triangles.
    let bar = Rect::from_min_size(pos2(hr.min.x, track.max.y + 10.0), vec2(w, 9.0));
    let n = 32;
    for k in 0..n {
        let v = (k as f32 / (n - 1) as f32 * 255.0) as u8;
        let x0 = bar.min.x + bar.width() * k as f32 / n as f32;
        let x1 = bar.min.x + bar.width() * (k + 1) as f32 / n as f32;
        p.rect_filled(Rect::from_min_max(pos2(x0, bar.min.y), pos2(x1, bar.max.y)), 0.0, Color32::from_gray(v));
    }
    let otrack = Rect::from_min_size(pos2(bar.min.x, bar.max.y + 1.0), vec2(w, 10.0));
    let (r3, nv) = fw::level_handle(ui, otrack, ob, Color32::BLACK, egui::Id::unique(("ec-lob", euid, ch)), &t);
    app.auto.add(&format!("effectControls.effect.{euid}.levels.outBlack"), r3, "Output Black");
    if let Some(v) = nv {
        set(ui, &ids[3], v, started(ui));
    }
    let (r4, nv) = fw::level_handle(ui, otrack, ow, Color32::WHITE, egui::Id::unique(("ec-low", euid, ch)), &t);
    app.auto.add(&format!("effectControls.effect.{euid}.levels.outWhite"), r4, "Output White");
    if let Some(v) = nv {
        set(ui, &ids[4], v, started(ui));
    }
}

/// Auto Levels / Auto Contrast / Auto Color: the histogram they work from.
pub(super) fn histogram_only(app: &mut EffectcraftApp, ui: &mut egui::Ui, p: &egui::Painter, g: &PropGroup, r: Rect) {
    let t = app.tokens;
    let x0 = r.min.x + 30.0;
    p.text(pos2(x0, r.min.y + 12.0), Align2::LEFT_CENTER, crate::i18n::tr("Histogram"), Tokens::ui(12.0), t.text_dim);
    let w = (r.width() - 72.0).clamp(120.0, 256.0);
    let hr = Rect::from_min_size(pos2(x0 + 8.0, r.min.y + 24.0), vec2(w, 80.0));
    match frame_histogram(app, ui.ctx()) {
        Some(h) => {
            fw::draw_histogram(p, hr, &h[1], channel_color(1).gamma_multiply(0.6));
            fw::draw_histogram_overlay(p, hr, &h[2], channel_color(2).gamma_multiply(0.6));
            fw::draw_histogram_overlay(p, hr, &h[3], channel_color(3).gamma_multiply(0.6));
        }
        None => {
            p.rect_filled(hr, 0.0, Color32::from_rgb(0x1c, 0x1c, 0x1c));
        }
    }
    app.auto.add(&format!("effectControls.effect.{}.histogram", g.uid), hr, "Histogram");
}

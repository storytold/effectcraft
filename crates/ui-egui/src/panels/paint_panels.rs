//! Paint and Brushes panels (Window ▸ Paint ⌘8, Window ▸ Brushes ⌘9).
//!
//! Both edit the session's paint options through `paint.options` / `paint.brushPreset`, so
//! agents see and set exactly what the panels show.

use crate::i18n::tr;
use effectcraft_engine::effects::paint::{BRUSH_PRESETS, CHANNELS, DURATIONS, ERASE_MODES, MODES};
use egui::{Align2, Color32, Rect, Sense, Stroke, StrokeKind, pos2, vec2};
use serde_json::{Value, json};

use crate::state::Tool;
use crate::theme::Tokens;
use crate::{EffectcraftApp, widgets};

fn set(app: &mut EffectcraftApp, v: Value) {
    if let Err(e) = app.session.execute("paint.options", v) {
        app.ui.status = e.to_string();
    }
}

/// A label + hot number row; returns the new value when changed.
#[allow(clippy::too_many_arguments)]
fn number(
    app: &mut EffectcraftApp,
    ui: &mut egui::Ui,
    p: &egui::Painter,
    x: f32,
    y: f32,
    label: &str,
    key: &str,
    v: f64,
    range: (f64, f64),
    suffix: &str,
    enabled: bool,
) -> Option<f64> {
    let t = app.tokens;
    p.text(pos2(x, y + 9.0), Align2::LEFT_CENTER, label, Tokens::ui(11.5), if enabled { t.text_dim } else { t.text_faint });
    let lw = p.layout_no_wrap(label.to_string(), Tokens::ui(11.5), t.text).size().x;
    let (r, nv, _) = widgets::hot_number_at(
        ui,
        pos2(x + lw + 6.0, y),
        egui::Id::unique(("paint-num", key)),
        v,
        &widgets::HotSpec { speed: 0.5, range, decimals: 0, suffix },
        &t,
    );
    app.auto.add(key, r, label);
    nv.filter(|_| enabled)
}

/// A label at `x`, `y` and, to its right, a dropdown over `options` with `cur` selected. The
/// dropdown only opens while `enabled`; otherwise only its label is dimmed. Returns the index
/// picked from the open menu, if any.
#[allow(clippy::too_many_arguments)]
fn popup_row(
    app: &mut EffectcraftApp,
    ui: &mut egui::Ui,
    p: &egui::Painter,
    x: f32,
    y: f32,
    w: f32,
    label: &str,
    key: &str,
    options: &[String],
    cur: usize,
    enabled: bool,
) -> Option<usize> {
    let t = app.tokens;
    p.text(pos2(x, y + 10.0), Align2::LEFT_CENTER, label, Tokens::ui(11.5), if enabled { t.text_dim } else { t.text_faint });
    let r = Rect::from_min_size(pos2(x + 70.0, y), vec2((w - 70.0).max(60.0), 20.0));
    let pop = egui::Id::unique(("paint-pop", key));
    if widgets::dropdown(ui, r, options.get(cur).map(String::as_str).unwrap_or(""), &t, egui::Id::unique(("paint-dd", key))).clicked() && enabled {
        widgets::open_popup(ui, pop);
    }
    app.auto.add(key, r, label);
    widgets::popup_menu(ui, pop, r.left_bottom(), options, Some(cur))
}

/// The RGB components of an RGBA paint colour as `f32`, for the swatches and the colour picker.
fn rgb(c: [f64; 4]) -> [f32; 3] {
    [c[0] as f32, c[1] as f32, c[2] as f32]
}

/// The swap-colours icon: one arrow right above one arrow left.
fn paint_swap_icon(p: &egui::Painter, rect: Rect, color: Color32) {
    let stroke = Stroke::new(1.0, color);
    let (left, right) = (rect.left() + 2.0, rect.right() - 2.0);
    let (top, bottom) = (rect.top() + 3.0, rect.bottom() - 3.0);
    p.arrow(pos2(left, top), vec2(right - left, 0.0), stroke);
    p.arrow(pos2(right, bottom), vec2(left - right, 0.0), stroke);
}

/// The foreground-over-background swatches (each opens a colour popup) with Swap and Reset.
fn colour_swatches(app: &mut EffectcraftApp, ui: &mut egui::Ui, p: &egui::Painter, x0: f32, y: f32, color: [f64; 4], background: [f64; 4]) {
    let t = app.tokens;
    let fg = Rect::from_min_size(pos2(x0, y), vec2(26.0, 20.0));
    let bg = Rect::from_min_size(pos2(x0 + 14.0, y + 10.0), vec2(26.0, 20.0));
    widgets::swatch(ui, bg, [background[0] as f32, background[1] as f32, background[2] as f32, 1.0], egui::Id::unique("paint-bg"), &t);
    let bresp = ui.interact(Rect::from_min_max(pos2(fg.max.x, bg.min.y), bg.max), egui::Id::unique("paint-bg-hit"), Sense::click());
    if bresp.clicked() {
        widgets::open_popup(ui, egui::Id::unique("paint-bg-pop"));
    }
    if widgets::swatch(ui, fg, [color[0] as f32, color[1] as f32, color[2] as f32, 1.0], egui::Id::unique("paint-fg"), &t).clicked() {
        widgets::open_popup(ui, egui::Id::unique("paint-fg-pop"));
    }
    app.auto.add("paint.color", fg, "Foreground color");
    app.auto.add("paint.background", bg, "Background color");
    let mut fc = rgb(color);
    if crate::widgets::color_popup(ui, egui::Id::unique("paint-fg-pop"), fg.left_bottom(), &mut fc) {
        set(app, json!({"color": [fc[0], fc[1], fc[2], 1.0]}));
    }
    let mut bc = rgb(background);
    if crate::widgets::color_popup(ui, egui::Id::unique("paint-bg-pop"), bg.left_bottom(), &mut bc) {
        set(app, json!({"background": [bc[0], bc[1], bc[2], 1.0]}));
    }
    let swap = Rect::from_min_size(pos2(x0 + 46.0, y), vec2(18.0, 14.0));
    let sresp = ui.interact(swap, egui::Id::unique("paint-swap"), Sense::click());
    // Two arrows (foreground over background): drawn, so the icon does not depend on a font
    // carrying U+21C4 (the bundled fonts do not).
    paint_swap_icon(p, swap, if sresp.hovered() { t.text } else { t.text_dim });
    app.auto.add("paint.swapColors", swap, "Swap colors");
    if sresp.on_hover_text(tr("Swap Paint Colors (X)")).clicked() {
        set(app, json!({"color": background, "background": color}));
    }
    let reset = Rect::from_min_size(pos2(x0 + 46.0, y + 18.0), vec2(18.0, 14.0));
    let rresp = ui.interact(reset, egui::Id::unique("paint-reset-colors"), Sense::click());
    p.rect_filled(Rect::from_min_size(reset.min + vec2(4.0, 3.0), vec2(7.0, 6.0)), 0.0, Color32::WHITE);
    p.rect_stroke(Rect::from_min_size(reset.min + vec2(8.0, 6.0), vec2(7.0, 6.0)), 0.0, Stroke::new(1.0, t.text_dim), StrokeKind::Inside);
    app.auto.add("paint.resetColors", reset, "Default colors");
    if rresp.on_hover_text(tr("Set Paint Colors to Black and White (D)")).clicked() {
        set(app, json!({"color": [1.0, 1.0, 1.0, 1.0], "background": [0.0, 0.0, 0.0, 1.0]}));
    }
}

/// The Paint panel: opacity and flow, foreground/background colours with swap and reset, mode,
/// channels, duration, erase mode and the Clone Stamp options. Every change is sent as a
/// `paint.options` command. Mode and Channels are disabled for the Eraser; Erase is enabled
/// only for it. The Clone Options heading is dimmed for other tools, but its controls stay enabled.
pub fn paint(app: &mut EffectcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let t = app.tokens;
    let p = ui.painter().with_clip_rect(rect);
    let o = app.session.state.paint.clone();
    let x0 = rect.min.x + 10.0;
    let w = rect.width() - 20.0;
    let mut y = rect.min.y + 10.0;
    let eraser = app.ui.tool == Tool::Eraser;
    let clone = app.ui.tool == Tool::Clone;

    // Opacity / Flow.
    if let Some(v) = number(app, ui, &p, x0, y, "Opacity:", "paint.opacity", o.opacity, (0.0, 100.0), " %", true) {
        set(app, json!({"opacity": v}));
    }
    if let Some(v) = number(app, ui, &p, x0 + w / 2.0, y, "Flow:", "paint.flow", o.flow, (0.0, 100.0), " %", true) {
        set(app, json!({"flow": v}));
    }
    y += 28.0;
    // Colour swatches (foreground over background) with swap / reset.
    colour_swatches(app, ui, &p, x0, y, o.color, o.background);
    y += 40.0;

    // Mode / Channels / Duration.
    let modes: Vec<String> = MODES.iter().map(|m| m.label().to_string()).collect();
    if let Some(i) = popup_row(app, ui, &p, x0, y, w, "Mode:", "paint.mode", &modes, o.mode as usize, !eraser) {
        set(app, json!({"mode": i}));
    }
    y += 26.0;
    let chans: Vec<String> = CHANNELS.iter().map(|s| s.to_string()).collect();
    if let Some(i) = popup_row(app, ui, &p, x0, y, w, "Channels:", "paint.channels", &chans, o.channels as usize, !eraser) {
        set(app, json!({"channels": i}));
    }
    y += 26.0;
    let durs: Vec<String> = DURATIONS.iter().map(|s| s.to_string()).collect();
    let dw = if o.duration == 3 { w - 60.0 } else { w };
    if let Some(i) = popup_row(app, ui, &p, x0, y, dw, "Duration:", "paint.duration", &durs, o.duration as usize, true) {
        set(app, json!({"durationMode": i}));
    }
    if o.duration == 3 {
        let (r, nv, _) = widgets::hot_number_at(
            ui,
            pos2(x0 + w - 52.0, y + 1.0),
            egui::Id::unique("paint-custom-frames"),
            o.custom_frames as f64,
            &widgets::HotSpec { speed: 0.2, range: (1.0, 10000.0), decimals: 0, suffix: " f" },
            &t,
        );
        app.auto.add("paint.customFrames", r, "Custom duration (frames)");
        if let Some(v) = nv {
            set(app, json!({"customFrames": v.round() as u64}));
        }
    }
    y += 26.0;
    let erase: Vec<String> = ERASE_MODES.iter().map(|s| s.to_string()).collect();
    if let Some(i) = popup_row(app, ui, &p, x0, y, w, "Erase:", "paint.erase", &erase, o.erase as usize, eraser) {
        set(app, json!({"eraseMode": i}));
    }
    y += 32.0;

    // Clone Options.
    p.line_segment([pos2(x0, y), pos2(x0 + w, y)], Stroke::new(1.0, t.separator));
    y += 8.0;
    p.text(pos2(x0, y + 8.0), Align2::LEFT_CENTER, tr("Clone Options"), Tokens::semibold(11.5), if clone { t.text } else { t.text_faint });
    y += 22.0;
    p.text(pos2(x0, y + 10.0), Align2::LEFT_CENTER, tr("Preset:"), Tokens::ui(11.5), t.text_dim);
    for i in 1..=5u32 {
        let r = Rect::from_min_size(pos2(x0 + 56.0 + (i - 1) as f32 * 26.0, y), vec2(22.0, 20.0));
        let resp = ui.interact(r, egui::Id::unique(("clone-preset", i)), Sense::click());
        let on = o.clone_preset == i;
        p.rect_filled(
            r,
            3.0,
            if on {
                t.accent
            } else if resp.hovered() {
                t.hover
            } else {
                t.field_bg
            },
        );
        p.text(r.center(), Align2::CENTER_CENTER, i.to_string(), Tokens::ui(11.0), if on { Color32::WHITE } else { t.text_dim });
        app.auto.add(&format!("paint.clonePreset.{i}"), r, &format!("Clone preset {i}"));
        if resp.clicked() {
            set(app, json!({"clonePreset": i}));
        }
    }
    y += 26.0;
    // Source layer.
    let comp_layers: Vec<(u64, String)> = app.session.active_comp().map(|c| c.layers.iter().map(|l| (l.id.0, l.name.clone())).collect()).unwrap_or_default();
    let mut names = vec!["Current Layer".to_string()];
    names.extend(comp_layers.iter().map(|(_, n)| n.clone()));
    let cur = o.clone_source.and_then(|id| comp_layers.iter().position(|(l, _)| *l == id).map(|i| i + 1)).unwrap_or(0);
    if let Some(i) = popup_row(app, ui, &p, x0, y, w, "Source:", "paint.cloneSource", &names, cur, true) {
        let v = if i == 0 { Value::Null } else { json!(comp_layers[i - 1].0) };
        set(app, json!({"cloneSource": v}));
    }
    y += 26.0;
    for (i, (label, key, on, k)) in
        [("Aligned", "paint.aligned", o.aligned, "aligned"), ("Lock Source Time", "paint.lockSourceTime", o.lock_source_time, "lockSourceTime")]
            .into_iter()
            .enumerate()
    {
        let r = Rect::from_min_size(pos2(x0 + i as f32 * (w / 2.0), y), vec2(18.0, 20.0));
        if widgets::checkbox(ui, r, on, &t, egui::Id::unique(key)).changed() {
            set(app, json!({k: !on}));
        }
        p.text(pos2(r.max.x + 4.0, r.center().y), Align2::LEFT_CENTER, label, Tokens::ui(11.5), t.text_dim);
        app.auto.add(key, r, label);
    }
    y += 26.0;
    let off = o.clone_offset.unwrap_or([0.0, 0.0]);
    p.text(pos2(x0, y + 9.0), Align2::LEFT_CENTER, tr("Offset:"), Tokens::ui(11.5), t.text_dim);
    for (d, v) in off.iter().enumerate() {
        let (r, nv, _) = widgets::hot_number_at(
            ui,
            pos2(x0 + 56.0 + d as f32 * 64.0, y),
            egui::Id::unique(("clone-off", d)),
            *v,
            &widgets::HotSpec { speed: 1.0, range: (-1e5, 1e5), decimals: 0, suffix: "" },
            &t,
        );
        app.auto.add(&format!("paint.cloneOffset.{d}"), r, "Clone offset");
        if let Some(nv) = nv {
            let mut no = off;
            no[d] = nv;
            set(app, json!({"cloneOffset": no}));
        }
    }
    y += 26.0;
    let (label, key, v) =
        if o.lock_source_time { ("Source Time:", "paint.cloneTime", o.clone_time) } else { ("Source Time Shift:", "paint.cloneTimeShift", o.clone_time_shift) };
    let k = if o.lock_source_time { "cloneTime" } else { "cloneTimeShift" };
    if let Some(nv) = number(app, ui, &p, x0, y, label, key, v, (-1e5, 1e5), " s", true) {
        set(app, json!({k: nv}));
    }
    y += 26.0;
    let r = Rect::from_min_size(pos2(x0, y), vec2(18.0, 20.0));
    if widgets::checkbox(ui, r, o.show_overlay, &t, egui::Id::unique("paint.showOverlay")).changed() {
        set(app, json!({"showOverlay": !o.show_overlay}));
    }
    app.auto.add("paint.showOverlay", r, "Clone Source Overlay");
    p.text(pos2(r.max.x + 4.0, r.center().y), Align2::LEFT_CENTER, tr("Clone Source Overlay:"), Tokens::ui(11.5), t.text_dim);
    if let Some(nv) = number(app, ui, &p, x0 + 150.0, y, "", "paint.overlayOpacity", o.overlay_opacity, (0.0, 100.0), " %", true) {
        set(app, json!({"overlayOpacity": nv}));
    }
    let dr = Rect::from_min_size(pos2(x0 + w - 18.0, y), vec2(18.0, 20.0));
    if widgets::checkbox(ui, dr, o.overlay_difference, &t, egui::Id::unique("paint.overlayDifference")).changed() {
        set(app, json!({"overlayDifference": !o.overlay_difference}));
    }
    app.auto.add("paint.overlayDifference", dr, "Difference mode overlay");
}

/// Draw a brush tip preview in `r`.
fn tip_preview(p: &egui::Painter, r: Rect, diameter: f64, angle: f64, roundness: f64, hardness: f64, col: Color32) {
    let max = r.width().min(r.height()) * 0.42;
    let rad = ((diameter / 2.0).sqrt() as f32 * 2.2).clamp(1.5, max);
    let rnd = (roundness / 100.0).clamp(0.05, 1.0) as f32;
    let ang = (angle as f32).to_radians();
    // Soft tips: concentric rings with falling alpha.
    let rings = if hardness >= 99.0 { 1 } else { 6 };
    for k in (0..rings).rev() {
        let f = (k + 1) as f32 / rings as f32;
        let h = (hardness / 100.0) as f32;
        let a = if rings == 1 { 1.0 } else { (1.0 - ((f - h) / (1.0 - h).max(1e-3)).clamp(0.0, 1.0)).max(0.15) };
        let pts: Vec<egui::Pos2> = (0..32)
            .map(|i| {
                let t = i as f32 / 32.0 * std::f32::consts::TAU;
                let (x, y) = (t.cos() * rad * f, t.sin() * rad * f * rnd);
                r.center() + vec2(x * ang.cos() - y * ang.sin(), x * ang.sin() + y * ang.cos())
            })
            .collect();
        p.add(egui::Shape::convex_polygon(pts, col.gamma_multiply(a / rings as f32 * if rings == 1 { 1.0 } else { 2.5 }), Stroke::NONE));
    }
}

/// The Brushes panel: a grid of built-in brush presets (each runs `paint.brushPreset`), a tip
/// preview, diameter/angle/roundness/hardness/spacing fields and the size/opacity/flow pressure
/// options. Values are sent as `paint.options` commands and failures land in `app.ui.status`.
pub fn brushes(app: &mut EffectcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let t = app.tokens;
    let p = ui.painter().with_clip_rect(rect);
    let o = app.session.state.paint.clone();
    let x0 = rect.min.x + 10.0;
    let w = rect.width() - 20.0;
    let mut y = rect.min.y + 8.0;
    // Preset grid (thumbnails with size labels).
    let cell = 38.0;
    let cols = ((w / cell).floor() as usize).max(1);
    for (i, tip) in BRUSH_PRESETS.iter().enumerate() {
        let r = Rect::from_min_size(pos2(x0 + (i % cols) as f32 * cell, y + (i / cols) as f32 * cell), vec2(cell - 2.0, cell - 2.0));
        let resp = ui.interact(r, egui::Id::unique(("brush-preset", i)), Sense::click());
        let on = o.preset == Some(i);
        p.rect_filled(
            r,
            3.0,
            if on {
                t.row_selected
            } else if resp.hovered() {
                t.hover
            } else {
                t.field_bg
            },
        );
        tip_preview(&p, Rect::from_min_size(r.min, vec2(r.width(), r.height() - 9.0)), tip.diameter, tip.angle, tip.roundness, tip.hardness, t.text);
        p.text(pos2(r.center().x, r.max.y - 5.0), Align2::CENTER_CENTER, format!("{}", tip.diameter), Tokens::ui(8.5), t.text_dim);
        app.auto.add(&format!("brushes.preset.{i}"), r, tip.name);
        if resp.on_hover_text(tip.name).clicked()
            && let Err(e) = app.session.execute("paint.brushPreset", json!({"preset": i}))
        {
            app.ui.status = e.to_string();
        }
    }
    y += (BRUSH_PRESETS.len().div_ceil(cols)) as f32 * cell + 8.0;
    p.line_segment([pos2(x0, y), pos2(x0 + w, y)], Stroke::new(1.0, t.separator));
    y += 8.0;
    // Tip preview + values.
    let prev = Rect::from_min_size(pos2(x0 + w - 56.0, y), vec2(56.0, 56.0));
    p.rect_filled(prev, 3.0, t.field_bg);
    tip_preview(&p, prev, o.diameter.min(60.0), o.angle, o.roundness, o.hardness, t.text);
    app.auto.add("brushes.tipPreview", prev, "Brush tip");
    for (label, key, v, range, suffix) in [
        ("Diameter:", "diameter", o.diameter, (1.0, 2500.0), " px"),
        ("Angle:", "angle", o.angle, (-180.0, 180.0), "°"),
        ("Roundness:", "roundness", o.roundness, (0.0, 100.0), " %"),
        ("Hardness:", "hardness", o.hardness, (0.0, 100.0), " %"),
    ] {
        if let Some(nv) = number(app, ui, &p, x0, y, label, &format!("brushes.{key}"), v, range, suffix, true) {
            set(app, json!({key: nv}));
        }
        y += 24.0;
    }
    let r = Rect::from_min_size(pos2(x0, y), vec2(18.0, 20.0));
    // Spacing (checkbox in AE turns spacing on; we always space dabs, so it shows the value).
    p.text(pos2(x0, y + 9.0), Align2::LEFT_CENTER, tr("Spacing:"), Tokens::ui(11.5), t.text_dim);
    let _ = r;
    let (sr, nv, _) = widgets::hot_number_at(
        ui,
        pos2(x0 + 58.0, y),
        egui::Id::unique("brushes-spacing"),
        o.spacing,
        &widgets::HotSpec { speed: 0.5, range: (1.0, 1000.0), decimals: 0, suffix: " %" },
        &t,
    );
    app.auto.add("brushes.spacing", sr, "Spacing");
    if let Some(nv) = nv {
        set(app, json!({"spacing": nv}));
    }
    y += 30.0;
    // Brush Dynamics.
    p.text(pos2(x0, y + 8.0), Align2::LEFT_CENTER, tr("Brush Dynamics"), Tokens::semibold(11.5), t.text);
    y += 22.0;
    let opts = vec!["Off".to_string(), "Pen Pressure".to_string()];
    for (label, key, on) in
        [("Size:", "sizePressure", o.size_pressure), ("Opacity:", "opacityPressure", o.opacity_pressure), ("Flow:", "flowPressure", o.flow_pressure)]
    {
        if let Some(i) = popup_row(app, ui, &p, x0, y, w, label, &format!("brushes.{key}"), &opts, on as usize, true) {
            set(app, json!({key: i == 1}));
        }
        y += 26.0;
        if key == "sizePressure"
            && let Some(nv) = number(app, ui, &p, x0 + 12.0, y, "Minimum Size:", "brushes.minSize", o.min_size, (0.0, 100.0), " %", o.size_pressure)
        {
            set(app, json!({"minSize": nv}));
        }
        if key == "sizePressure" {
            y += 26.0;
        }
    }
}

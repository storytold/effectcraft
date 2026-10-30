//! Smaller panels: Preview, Audio, History, Markers and placeholders.

use crate::i18n::{tr, tr_args};
use egui::{Align2, Color32, Rect, Sense, Stroke, pos2, vec2};
use serde_json::json;

use crate::dock::PanelKind;
use crate::icons::Icon;
use crate::theme::Tokens;
use crate::{EffectcraftApp, widgets};

pub fn placeholder(app: &mut EffectcraftApp, ui: &mut egui::Ui, rect: Rect, p: PanelKind) {
    let t = app.tokens;
    let name = crate::i18n::label(app, "window.panel", p.title());
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, tr_args("{} — coming soon", &[&name]), Tokens::ui(12.0), t.text_faint);
}

/// Preview panel: transport controls, the shortcut whose options are shown, and that
/// shortcut's options (each Preview shortcut keeps its own; see `effectcraft_engine::preview`).
/// Every change runs `playback.settings.set`.
pub fn preview(app: &mut EffectcraftApp, ui: &mut egui::Ui, rect: Rect) {
    use effectcraft_engine::preview::{PlayFrom, PreviewRange, PreviewResolution, PreviewShortcut};
    let t = app.tokens;
    let ctx = ui.ctx().clone();
    let p = ui.painter().with_clip_rect(rect);
    // The panel scrolls when it is shorter than its controls (the default workspace shows just
    // the transport row, as in After Effects).
    let scroll = widgets::PanelScroll::begin(ui, egui::Id::unique("preview-scroll"), rect);
    let panel_clip = ui.clip_rect();
    ui.set_clip_rect(rect.intersect(panel_clip));
    let y = rect.min.y + 12.0 - scroll.offset;
    let bw = 30.0;
    let total = bw * 5.0 + 16.0;
    let mut x = rect.center().x - total / 2.0;
    let buttons: [(Icon, &str, &str); 5] = [
        (Icon::First, "time.start", "First Frame"),
        (Icon::StepBack, "time.previousFrame", "Previous Frame"),
        (Icon::Play, "playback.toggle", "Play/Stop"),
        (Icon::StepFwd, "time.nextFrame", "Next Frame"),
        (Icon::Last, "time.end", "Last Frame"),
    ];
    let current = app.session.prefs.preview.current;
    for (icon, cmd, tip) in buttons {
        let r = Rect::from_min_size(pos2(x, y), vec2(bw, 28.0));
        let icon = if cmd == "playback.toggle" && app.playback.playing { Icon::Pause } else { icon };
        if widgets::icon_button(ui, r, icon, cmd == "playback.toggle" && app.playback.playing, &t, egui::Id::unique(("pv", cmd)))
            .on_hover_text(tr(tip))
            .clicked()
        {
            // The play button plays with the options of the shortcut shown below.
            let params = if cmd == "playback.toggle" { json!({"shortcut": current.id()}) } else { json!({}) };
            let _ = crate::menus::invoke(app, &ctx, cmd, params);
        }
        app.auto.add(&format!("preview.{}", tip.replace([' ', '/'], "")), r, tip);
        x += bw + 4.0;
    }
    let o = app.session.prefs.preview.get(current).clone();
    let mut changes: Vec<serde_json::Value> = vec![];
    let x0 = rect.min.x + 12.0;
    let xv = rect.min.x + 92.0;
    let wv = (rect.max.x - xv - 12.0).clamp(80.0, 240.0);
    let label = |yy: f32, k: &str| {
        p.text(pos2(x0, yy), Align2::LEFT_CENTER, k, Tokens::ui(12.0), t.text_dim);
    };
    let mut yy = y + 46.0;

    // Shortcut.
    label(yy, tr("Shortcut"));
    let names: Vec<String> = PreviewShortcut::ALL.iter().map(|s| s.label().to_string()).collect();
    if let Some(i) = dropdown_row(
        app,
        ui,
        Rect::from_min_size(pos2(xv, yy - 10.0), vec2(wv, 20.0)),
        "shortcut",
        current.label(),
        &names,
        PreviewShortcut::ALL.iter().position(|s| *s == current),
    ) {
        let s = PreviewShortcut::ALL[i];
        changes.push(json!({"shortcut": s.id(), "current": s.id()}));
    }
    yy += 26.0;

    // Include Video / Audio / Overlays / Layer Controls, Loop, Cache Before Playback.
    let checks: [(&str, &str, bool); 6] = [
        ("includeVideo", "Video", o.include_video),
        ("includeAudio", "Audio", o.include_audio),
        ("includeOverlays", "Overlays", o.include_overlays),
        ("includeLayerControls", "Layer Controls", o.include_layer_controls),
        ("loop", "Loop", o.loop_),
        ("cacheBeforePlayback", "Cache Before Playback", o.cache_before_playback),
    ];
    for (i, (key, text, on)) in checks.into_iter().enumerate() {
        let col = if i < 4 { i % 2 } else { 0 };
        let row = if i < 4 { i / 2 } else { i - 2 };
        let cx = x0 + col as f32 * 110.0 - 2.0;
        let cy = yy + row as f32 * 22.0;
        if check_row(app, ui, &p, pos2(cx, cy), key, tr(text), on) {
            changes.push(json!({"shortcut": current.id(), key: !on}));
        }
    }
    yy += 4.0 * 22.0 + 6.0;

    // Range (and the pre/post-roll of Play Around Current Time).
    label(yy, tr("Range"));
    let ranges: Vec<String> = PreviewRange::ALL.iter().map(|r| r.label().to_string()).collect();
    let range_ids = ["workArea", "workAreaExtended", "entireDuration", "aroundCurrentTime"];
    if let Some(i) = dropdown_row(
        app,
        ui,
        Rect::from_min_size(pos2(xv, yy - 10.0), vec2(wv, 20.0)),
        "range",
        o.range.label(),
        &ranges,
        PreviewRange::ALL.iter().position(|r| *r == o.range),
    ) {
        changes.push(json!({"shortcut": current.id(), "range": range_ids[i]}));
    }
    yy += 24.0;
    if o.range == PreviewRange::AroundCurrentTime {
        for (k, (key, text, v)) in [("preRoll", "Pre-roll", o.pre_roll), ("postRoll", "Post-roll", o.post_roll)].into_iter().enumerate() {
            let lx = xv + k as f32 * 100.0;
            p.text(pos2(lx, yy), Align2::LEFT_CENTER, tr(text), Tokens::ui(11.5), t.text_dim);
            let (r, nv, _) = widgets::hot_number_at(
                ui,
                pos2(lx + 52.0, yy - 9.0),
                egui::Id::unique(("pv-roll", key)),
                v,
                &widgets::HotSpec { speed: 0.05, range: (0.0, 600.0), decimals: 1, suffix: "s" },
                &t,
            );
            app.auto.add(&format!("preview.{key}"), r, text);
            if let Some(nv) = nv {
                changes.push(json!({"shortcut": current.id(), key: nv}));
            }
        }
        yy += 22.0;
    }

    // Play From.
    label(yy, tr("Play From"));
    let froms: Vec<String> = PlayFrom::ALL.iter().map(|f| f.label().to_string()).collect();
    if let Some(i) = dropdown_row(
        app,
        ui,
        Rect::from_min_size(pos2(xv, yy - 10.0), vec2(wv, 20.0)),
        "playFrom",
        o.play_from.label(),
        &froms,
        PlayFrom::ALL.iter().position(|f| *f == o.play_from),
    ) {
        changes.push(json!({"shortcut": current.id(), "playFrom": (["rangeStart", "currentTime"][i])}));
    }
    yy += 26.0;

    // Frame Rate, Skip, Resolution.
    let comp_fps = app.session.active_comp().map(|c| c.frame_rate.as_f64());
    label(yy, tr("Frame Rate"));
    const RATES: [f64; 12] = [8.0, 10.0, 12.0, 12.5, 15.0, 23.976, 24.0, 25.0, 29.97, 30.0, 50.0, 60.0];
    let auto = format!("({:.2}) Auto", comp_fps.unwrap_or(0.0));
    let mut rates = vec![auto.clone()];
    rates.extend(RATES.iter().map(|r| fmt_rate(*r)));
    let rate_text = o.frame_rate.map(fmt_rate).unwrap_or(auto);
    let rate_cur = match o.frame_rate {
        None => Some(0),
        Some(r) => RATES.iter().position(|x| (x - r).abs() < 1e-3).map(|i| i + 1),
    };
    if let Some(i) = dropdown_row(app, ui, Rect::from_min_size(pos2(xv, yy - 10.0), vec2(96.0, 20.0)), "frameRate", &rate_text, &rates, rate_cur) {
        let v = if i == 0 { json!("auto") } else { json!(RATES[i - 1]) };
        changes.push(json!({"shortcut": current.id(), "frameRate": v}));
    }
    let sx = xv + 106.0;
    p.text(pos2(sx, yy), Align2::LEFT_CENTER, tr("Skip"), Tokens::ui(12.0), t.text_dim);
    let (r, nv, _) = widgets::hot_number_at(
        ui,
        pos2(sx + 30.0, yy - 9.0),
        egui::Id::unique("pv-skip"),
        o.skip as f64,
        &widgets::HotSpec { speed: 0.1, range: (0.0, 99.0), decimals: 0, suffix: "" },
        &t,
    );
    app.auto.add("preview.skip", r, "Skip");
    if let Some(nv) = nv {
        changes.push(json!({"shortcut": current.id(), "skip": nv.round() as u32}));
    }
    yy += 24.0;
    label(yy, tr("Resolution"));
    let ress: Vec<String> = PreviewResolution::ALL.iter().map(|r| r.label().to_string()).collect();
    let res_ids = ["auto", "full", "half", "third", "quarter", "custom"];
    if let Some(i) = dropdown_row(
        app,
        ui,
        Rect::from_min_size(pos2(xv, yy - 10.0), vec2(96.0, 20.0)),
        "resolution",
        o.resolution.label(),
        &ress,
        PreviewResolution::ALL.iter().position(|r| *r == o.resolution),
    ) {
        changes.push(json!({"shortcut": current.id(), "resolution": res_ids[i]}));
    }
    if o.resolution == PreviewResolution::Custom {
        let (r, nv, _) = widgets::hot_number_at(
            ui,
            pos2(xv + 106.0, yy - 9.0),
            egui::Id::unique("pv-custom-res"),
            o.custom_resolution as f64,
            &widgets::HotSpec { speed: 0.1, range: (1.0, 40.0), decimals: 0, suffix: "" },
            &t,
        );
        app.auto.add("preview.customResolution", r, "Custom Resolution");
        if let Some(nv) = nv {
            changes.push(json!({"shortcut": current.id(), "customResolution": nv.round() as u32}));
        }
    }
    yy += 24.0;
    if check_row(app, ui, &p, pos2(x0 - 2.0, yy), "fullScreen", tr("Full Screen"), o.full_screen) {
        changes.push(json!({"shortcut": current.id(), "fullScreen": !o.full_screen}));
    }
    yy += 24.0;

    // What stopping does.
    p.text(pos2(x0, yy), Align2::LEFT_CENTER, tr_args("On ({}) stop:", &[&(current.label())]), Tokens::ui(11.5), t.text_dim);
    yy += 20.0;
    if check_row(app, ui, &p, pos2(x0 - 2.0, yy), "playCachedFrames", tr("If caching, play cached frames"), o.play_cached_frames) {
        changes.push(json!({"shortcut": current.id(), "playCachedFrames": !o.play_cached_frames}));
    }
    yy += 22.0;
    if check_row(app, ui, &p, pos2(x0 - 2.0, yy), "moveTimeToPreviewTime", "Move time to preview time", o.move_time_to_preview_time) {
        changes.push(json!({"shortcut": current.id(), "moveTimeToPreviewTime": !o.move_time_to_preview_time}));
    }
    yy += 24.0;
    for c in changes {
        if let Err(e) = app.session.execute("playback.settings.set", c) {
            app.ui.status = e.to_string();
        }
    }

    // Cache bar over the preview range.
    if let (Some(c), Some(cid)) = (app.session.active_comp_arc(), app.session.active_comp_id()) {
        let pl = match app.playback.plan.filter(|_| app.playback.playing) {
            Some(pl) => pl,
            None => effectcraft_engine::preview::plan(&o, &c, app.session.time()),
        };
        let cached_set = app.frames.cached_frames(&app.shown_series(cid));
        let total = pl.frames().count();
        let cached = pl.frames().filter(|f| cached_set.binary_search(f).is_ok()).count();
        let bar = Rect::from_min_size(pos2(rect.min.x + 12.0, yy), vec2(rect.width() - 24.0, 6.0));
        p.rect_filled(bar, 3.0, t.field_bg);
        let f = (cached as f32 / total.max(1) as f32).min(1.0);
        p.rect_filled(Rect::from_min_size(bar.min, vec2(bar.width() * f, bar.height())), 3.0, t.cache_green);
        let status = if app.playback.playing && app.playback.caching { " — caching before playback" } else { "" };
        p.text(pos2(bar.min.x, bar.max.y + 12.0), Align2::LEFT_CENTER, format!("{cached} / {total} frames cached{status}"), Tokens::ui(11.0), t.text_faint);
    }
    ui.set_clip_rect(panel_clip);
    let content = yy + 30.0 - (rect.min.y - scroll.offset);
    scroll.end(ui, &mut app.auto, "preview.scroll", content, &t);
}

fn fmt_rate(r: f64) -> String {
    if (r - r.round()).abs() < 1e-6 { format!("{r:.0}") } else { format!("{r}") }
}

/// A checkbox with its label; returns true when clicked.
fn check_row(app: &mut EffectcraftApp, ui: &mut egui::Ui, p: &egui::Painter, at: egui::Pos2, key: &str, text: &str, on: bool) -> bool {
    let t = app.tokens;
    let r = Rect::from_min_size(pos2(at.x, at.y - 8.0), vec2(16.0, 16.0));
    let clicked = widgets::checkbox(ui, r, on, &t, egui::Id::unique(("pv-check", key))).clicked();
    p.text(pos2(r.max.x + 5.0, at.y), Align2::LEFT_CENTER, text, Tokens::ui(12.0), t.text);
    app.auto.add(&format!("preview.{key}"), r, text);
    clicked
}

/// A dropdown with its popup list (automation ids `preview.<key>` and `preview.<key>.<i>`);
/// returns the chosen index.
fn dropdown_row(app: &mut EffectcraftApp, ui: &mut egui::Ui, r: Rect, key: &str, text: &str, items: &[String], current: Option<usize>) -> Option<usize> {
    let t = app.tokens;
    let pid = egui::Id::unique(("pv-pop", key));
    if widgets::dropdown(ui, r, text, &t, pid.with("btn")).clicked() {
        let open: bool = ui.data(|d| d.get_temp(pid.with("open")).unwrap_or(false));
        ui.data_mut(|d| d.insert_temp(pid.with("open"), !open));
    }
    app.auto.add(&format!("preview.{key}"), r, text);
    if !widgets::popup_is_open(ui, pid) {
        return None;
    }
    widgets::popup_list(ui, pid, r.left_bottom() + vec2(0.0, 2.0), r, items.len(), 0, |ui| {
        ui.set_min_width(r.width().max(140.0));
        let mut chosen = None;
        for (i, label) in items.iter().enumerate() {
            let resp = ui.selectable_label(current == Some(i), label.as_str());
            app.auto.add(&format!("preview.{key}.{i}"), resp.rect, label);
            if resp.clicked() {
                chosen = Some(i);
            }
        }
        chosen
    })
}

/// Audio panel: L/R VU meters (dBFS, 0 to -48) with peak hold and clip indicators, fed by the
/// audio preview; the selected layer's Audio Levels on the right.
pub fn audio(app: &mut EffectcraftApp, ui: &mut egui::Ui, rect: Rect) {
    use crate::audio::METER_FLOOR;
    let t = app.tokens;
    let p = ui.painter().with_clip_rect(rect);
    let top = rect.min.y + 26.0;
    let bottom = rect.max.y - 22.0;
    if bottom - top < 40.0 {
        return;
    }
    let y_of = |db: f32| top + (bottom - top) * (db.clamp(METER_FLOOR, 0.0) / METER_FLOOR);
    let m = app.meter;
    let green = Color32::from_rgb(0x3c, 0xc8, 0x5a);
    let yellow = Color32::from_rgb(0xe6, 0xc8, 0x3c);
    let red = Color32::from_rgb(0xe6, 0x46, 0x3c);
    let seg_col = |db: f32| {
        if db > -3.0 {
            red
        } else if db > -12.0 {
            yellow
        } else {
            green
        }
    };
    for c in 0..2 {
        let x = rect.min.x + 18.0 + c as f32 * 16.0;
        let bar = Rect::from_min_max(pos2(x, top), pos2(x + 12.0, bottom));
        p.rect_filled(bar, 1.0, t.field_bg);
        // Lit segments, 1.5 dB each, coloured by their level.
        let lvl = m.level_db[c];
        let mut db = METER_FLOOR;
        while db < lvl.min(0.0) {
            let hi = (db + 1.5).min(lvl);
            p.rect_filled(Rect::from_min_max(pos2(bar.min.x + 1.0, y_of(hi)), pos2(bar.max.x - 1.0, y_of(db) - 0.5)), 0.0, seg_col(db));
            db += 1.5;
        }
        if m.peak_db[c] > METER_FLOOR {
            let py = y_of(m.peak_db[c]);
            p.line_segment([pos2(bar.min.x, py), pos2(bar.max.x, py)], Stroke::new(2.0, seg_col(m.peak_db[c])));
        }
        // Clip indicator (click to reset).
        let clip = Rect::from_min_max(pos2(bar.min.x, top - 14.0), pos2(bar.max.x, top - 4.0));
        p.rect_filled(clip, 1.0, if m.clipped[c] { red } else { t.field_bg });
        let id = if c == 0 { "audio.clipLeft" } else { "audio.clipRight" };
        if ui.interact(clip, egui::Id::unique(id), Sense::click()).clicked() {
            app.meter.clipped[c] = false;
        }
        app.auto.add(id, clip, if c == 0 { "Left clip indicator" } else { "Right clip indicator" });
        app.auto.add(if c == 0 { "audio.meterLeft" } else { "audio.meterRight" }, bar, &format!("{:.1} dB", m.level_db[c]));
        p.text(pos2(bar.center().x, bottom + 9.0), Align2::CENTER_CENTER, if c == 0 { "L" } else { "R" }, Tokens::ui(10.0), t.text_dim);
    }
    let sx = rect.min.x + 54.0;
    for db in [0, -6, -12, -18, -24, -30, -36, -42, -48] {
        let y = y_of(db as f32);
        p.line_segment([pos2(sx - 4.0, y), pos2(sx - 1.0, y)], Stroke::new(1.0, t.text_faint));
        p.text(pos2(sx + 2.0, y), Align2::LEFT_CENTER, format!("{db:.1}"), Tokens::ui(10.0), t.text_faint);
    }
    p.text(pos2(sx + 30.0, top - 9.0), Align2::LEFT_CENTER, "dB", Tokens::ui(10.0), t.text_dim);
    // Selected layer's Audio Levels.
    let lx = rect.min.x + 120.0;
    if rect.max.x - lx < 80.0 {
        return;
    }
    let sel = app.session.state.selected_layers.first().copied();
    let levels = sel.and_then(|id| {
        let c = app.session.active_comp()?;
        let l = c.layer(id)?;
        let pr = l.props.sub("audio")?.get("levels")?;
        Some((l.name.clone(), pr.value.as_vec2()))
    });
    match levels {
        Some((name, [a, b])) => {
            p.text(pos2(lx, top - 9.0), Align2::LEFT_CENTER, &name, Tokens::ui(11.0), t.text_dim);
            for (i, (k, v)) in [("Left", a), ("Right", b)].into_iter().enumerate() {
                let y = top + 12.0 + i as f32 * 20.0;
                p.text(pos2(lx, y), Align2::LEFT_CENTER, k, Tokens::ui(12.0), t.text_dim);
                p.text(pos2(lx + 44.0, y), Align2::LEFT_CENTER, format!("{v:+.1} dB"), Tokens::ui(12.0), t.hot_text);
            }
        }
        None => {
            p.text(pos2(lx, top + 12.0), Align2::LEFT_CENTER, tr("No audio layer selected"), Tokens::ui(11.0), t.text_faint);
        }
    }
    if app.meter.active() || app.audio.is_some() {
        ui.ctx().request_repaint();
    }
}

/// The History panel: every state of the session's branching history, indented under the state it
/// grew from, with the current state highlighted and future/abandoned branches dimmed. Only the
/// rows in view are drawn; the current state is scrolled into view when it changes. Clicking a
/// state runs `edit.history.goto`, reporting a failure in `app.ui.status`.
pub fn history(app: &mut EffectcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let t = app.tokens;
    let p = ui.painter().with_clip_rect(rect);
    // The branching history (EffectCraft's History panel): every state, branches indented under
    // the state they grew from; click any state to go there (`edit.history.goto`).
    let nodes = app.session.history_tree();
    let row_h = 22.0;
    let cur = nodes.iter().position(|n| n.current).unwrap_or(0);
    let scroll_id = egui::Id::unique("history-scroll");
    let mut scroll = widgets::PanelScroll::begin(ui, scroll_id, rect);
    // The current state scrolls into view when it changes (undo, redo, a new step).
    if ui.data(|d| d.get_temp::<usize>(scroll_id.with("current"))) != Some(cur) {
        ui.data_mut(|d| d.insert_temp(scroll_id.with("current"), cur));
        let top = cur as f32 * row_h;
        if top < scroll.offset {
            scroll.offset = top;
        } else if top + row_h + 12.0 > scroll.offset + rect.height() {
            scroll.offset = top + row_h + 12.0 - rect.height();
        }
    }
    // Only the rows in view are drawn.
    let first = ((scroll.offset / row_h) as usize).min(nodes.len());
    let mut y = rect.min.y + 6.0 - (scroll.offset - first as f32 * row_h);
    let visible = (rect.height() / row_h) as usize + 2;
    let mut jump: Option<usize> = None;
    for n in nodes.iter().skip(first).take(visible) {
        let r = Rect::from_min_size(pos2(rect.min.x, y), vec2(rect.width(), row_h));
        y += row_h;
        let resp = ui.interact(r, egui::Id::unique(("hist", n.id.as_str())), Sense::click());
        if n.current {
            p.rect_filled(r, 0.0, t.row_selected);
        } else if resp.hovered() {
            p.rect_filled(r, 0.0, t.hover);
        }
        let x = r.min.x + 12.0 + n.depth as f32 * 14.0;
        if n.depth > 0 {
            // Branch marker.
            let c = pos2(x - 7.0, r.center().y);
            p.line_segment([pos2(c.x, r.min.y), c], egui::Stroke::new(1.0, t.text_faint));
            p.line_segment([c, pos2(c.x + 5.0, c.y)], egui::Stroke::new(1.0, t.text_faint));
        }
        let col = if !n.current && (n.future || !n.line) { t.text_faint } else { t.text };
        p.text(pos2(x, r.center().y), Align2::LEFT_CENTER, &n.label, Tokens::ui(12.0), col);
        app.auto.add(&format!("history.state.{}", n.index), r, &n.label);
        if resp.clicked() {
            jump = Some(n.index);
        }
    }
    scroll.end(ui, &mut app.auto, "history.scroll", nodes.len() as f32 * row_h + 12.0, &t);
    if let Some(i) = jump {
        let ctx = ui.ctx().clone();
        if let Err(e) = crate::menus::invoke(app, &ctx, "edit.history.goto", serde_json::json!({"index": i})) {
            app.ui.status = e;
        }
    }
}

/// The Markers panel: the active comp's markers in time order, each with its timecode and comment;
/// clicking a row moves the current time to that marker. Shows a hint when the comp has none and
/// draws nothing without an active comp.
pub fn markers(app: &mut EffectcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let t = app.tokens;
    let p = ui.painter().with_clip_rect(rect);
    let Some(c) = app.session.active_comp_arc() else { return };
    let scroll = widgets::PanelScroll::begin(ui, egui::Id::unique("markers-scroll"), rect);
    let mut y = rect.min.y + 8.0 - scroll.offset;
    if c.markers.is_empty() {
        p.text(rect.center(), Align2::CENTER_CENTER, tr("No composition markers"), Tokens::ui(12.0), t.text_faint);
    }
    for m in &c.markers {
        let r = Rect::from_min_size(pos2(rect.min.x, y), vec2(rect.width(), 22.0));
        y += 22.0;
        if !r.intersects(rect) {
            continue;
        }
        let tc = crate::panels::timecode(&app.session, &c, m.time);
        p.text(pos2(r.min.x + 12.0, r.center().y), Align2::LEFT_CENTER, tc, Tokens::mono(11.5), t.timecode);
        p.text(pos2(r.min.x + 120.0, r.center().y), Align2::LEFT_CENTER, &m.comment, Tokens::ui(12.0), t.text);
        if ui.interact(r, egui::Id::unique(("mk", m.time.0)), Sense::click()).clicked() {
            app.session.set_time(m.time);
        }
    }
    let content = y + 8.0 - (rect.min.y - scroll.offset);
    scroll.end(ui, &mut app.auto, "markers.scroll", content, &t);
}

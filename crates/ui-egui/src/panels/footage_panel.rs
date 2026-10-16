//! The Footage panel (double-click footage in the Project panel): the source on its own time
//! ruler, play / pause, In and Out marks, and the Edit Target buttons — Overlay Edit and Ripple
//! Insert Edit — that cut the marked range into the active composition at its current time.
//! Everything runs `footage.*` commands (state in `EditorState::footage_panel`).

use crate::i18n::{tr, tr_args};
use effectcraft_engine::project::{Footage, FootageKind, ItemId, ItemKind};
use effectcraft_engine::render::FootageSource;
use effectcraft_engine::time::Tick;
use egui::{Align2, Color32, Rect, Sense, Stroke, pos2, vec2};
use serde_json::json;
use std::hash::Hasher;
use std::sync::{Arc, OnceLock};

use super::panel_kit as kit;
use crate::EffectcraftApp;
use crate::theme::Tokens;

pub fn show(app: &mut EffectcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let t = app.tokens;
    let ctx = ui.ctx().clone();
    let p = ui.painter().with_clip_rect(rect);
    p.rect_filled(rect, 0.0, t.pasteboard);
    let Some(view) = app.session.state.footage_panel.clone() else {
        // Wrapped to the panel, so a narrow panel breaks the hint into lines instead of clipping it.
        let hint = tr("Double-click footage in the Project panel to open it here");
        let mut job = egui::text::LayoutJob::simple(hint.to_owned(), Tokens::ui(12.0), t.text_faint, (rect.width() - 20.0).max(40.0));
        job.halign = egui::Align::Center;
        let galley = p.layout_job(job);
        p.galley(pos2(rect.center().x, rect.center().y - galley.size().y * 0.5), galley, t.text_faint);
        app.auto.add("footage.empty", rect, "Footage panel (empty)");
        return;
    };
    // Borrowed from a project snapshot, not copied (an image sequence lists every frame's file).
    let project = app.session.project.clone();
    let Some(item) = project.item(view.item) else {
        app.session.state.footage_panel = None;
        return;
    };
    let footage = match &item.kind {
        ItemKind::Footage(f) => Some(f),
        _ => None,
    };
    let (dur, fr) = match &footage {
        Some(f) if f.kind != FootageKind::Still && f.duration > Tick::ZERO => (f.duration, f.frame_rate),
        Some(f) => (Tick::from_seconds_f64(10.0), f.frame_rate),
        None => (Tick::from_seconds_f64(10.0), effectcraft_engine::time::FrameRate::FPS_30),
    };
    // Title strip.
    let top = Rect::from_min_size(rect.min, vec2(rect.width(), 24.0));
    p.rect_filled(top, 0.0, t.panel_bg);
    p.text(pos2(top.min.x + 10.0, top.center().y), Align2::LEFT_CENTER, tr_args("Footage: {}", &[&item.name]), Tokens::semibold(12.0), t.text);
    app.auto.add("footage.title", top, &item.name);
    // Bottom: ruler and controls.
    // Rows, bottom up: Edit Target buttons, transport, ruler.
    let edit_row = Rect::from_min_max(pos2(rect.min.x, rect.max.y - 30.0), rect.max);
    let bar = Rect::from_min_max(pos2(rect.min.x, edit_row.min.y - 28.0), pos2(rect.max.x, edit_row.min.y));
    let ruler = Rect::from_min_max(pos2(rect.min.x + 10.0, bar.min.y - 26.0), pos2(rect.max.x - 10.0, bar.min.y - 4.0));
    p.rect_filled(Rect::from_min_max(pos2(rect.min.x, ruler.min.y - 4.0), rect.max), 0.0, t.panel_bg);
    // Canvas.
    let area = Rect::from_min_max(pos2(rect.min.x, top.max.y), pos2(rect.max.x, ruler.min.y - 4.0));
    if let Some(f) = footage
        && f.has_video
    {
        // Decoded once per footage frame (playback here advances the time every UI frame), on a
        // worker thread and averaged down to the panel's size; the last frame stays up meanwhile.
        let frame = if f.kind == FootageKind::Still { 0 } else { f.frame_rate.frame_at(view.time) };
        let fit = fitted_area(area);
        let ppp = ctx.pixels_per_point();
        let fs = ((fit.x / f.width.max(1) as f32).min(fit.y / f.height.max(1) as f32) * ppp).max(1e-6);
        let k = ((1.0 / fs).floor() as usize).max(1);
        let key = FrameKey { item: view.item.0, frame, footage: footage_fingerprint(&ctx, &project, view.item.0, f), k };
        let tex = frame_texture(app, &ctx, key, view.time);
        if let Some((tex, [w, h])) = tex {
            let s = (fit.x / w as f32).min(fit.y / h as f32).max(0.01);
            let r = Rect::from_center_size(area.center(), vec2(w as f32 * s, h as f32 * s));
            super::viewer::checker(&p.with_clip_rect(area), r);
            p.image(tex.id(), r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        }
    } else if let ItemKind::Solid(so) = &item.kind {
        let c = so.color;
        p.rect_filled(area.shrink(20.0), 0.0, Color32::from_rgb((c[0] * 255.0) as u8, (c[1] * 255.0) as u8, (c[2] * 255.0) as u8));
    } else {
        p.text(area.center(), Align2::CENTER_CENTER, tr("No video"), Tokens::ui(12.0), t.text_faint);
    }
    // Ruler: ticks, In/Out span, CTI; click or drag to scrub.
    let x_of = |tt: Tick| ruler.min.x + (tt.seconds() / dur.seconds().max(1e-9)) as f32 * ruler.width();
    p.rect_filled(ruler, 2.0, t.tl_ruler_bg);
    let secs = dur.seconds().max(0.001);
    let step = [0.04, 0.2, 1.0, 5.0, 10.0, 30.0, 60.0, 300.0].into_iter().find(|s| (secs / s) as f32 * 50.0 < ruler.width()).unwrap_or(600.0);
    let mut k = 0.0;
    while k <= secs {
        let x = ruler.min.x + (k / secs) as f32 * ruler.width();
        p.line_segment([pos2(x, ruler.max.y - 6.0), pos2(x, ruler.max.y)], Stroke::new(1.0, t.tl_ruler_tick));
        p.text(pos2(x + 2.0, ruler.min.y + 1.0), Align2::LEFT_TOP, kit::seconds(k), Tokens::ui(9.0), t.tl_ruler_text);
        k += step;
    }
    let (a, b) = (view.in_point.unwrap_or(Tick::ZERO), view.out_point.unwrap_or(dur));
    p.rect_filled(Rect::from_min_max(pos2(x_of(a), ruler.max.y - 4.0), pos2(x_of(b), ruler.max.y)), 0.0, t.work_area);
    let cx = x_of(view.time);
    p.line_segment([pos2(cx, ruler.min.y), pos2(cx, ruler.max.y)], Stroke::new(1.5, t.cti));
    let resp = ui.interact(ruler, egui::Id::new("footage-ruler"), Sense::click_and_drag());
    app.auto.add("footage.ruler", ruler, "Time ruler");
    if (resp.clicked() || resp.dragged())
        && let Some(pt) = resp.interact_pointer_pos()
    {
        let s = ((pt.x - ruler.min.x) / ruler.width()).clamp(0.0, 1.0) as f64 * dur.seconds();
        kit::exec(app, "footage.setTime", json!({"time": s}));
    }
    // Playback.
    let play_id = egui::Id::new("footage-playing");
    let playing: bool = ctx.data(|d| d.get_temp(play_id).unwrap_or(false));
    if playing {
        let dt = ctx.input(|i| i.stable_dt).min(0.1) as f64;
        let next = view.time.seconds() + dt;
        let next = if next >= dur.seconds() - fr.frame_duration().seconds() { 0.0 } else { next };
        let _ = app.session.execute("footage.setTime", json!({"time": next}));
        ctx.request_repaint();
    }
    // Controls.
    let y = bar.min.y + 5.0;
    let mut x = bar.min.x + 10.0;
    p.text(
        pos2(x, bar.center().y),
        Align2::LEFT_CENTER,
        effectcraft_engine::commands::footage_panel::timecode(Some(item), fr, fr.frame_at(view.time)),
        Tokens::mono(12.0),
        t.timecode,
    );
    x += 92.0;
    if kit::button(app, ui, Rect::from_min_size(pos2(x, y), vec2(54.0, 20.0)), "footage.play", if playing { "Pause" } else { "Play" }, false) {
        ctx.data_mut(|d| d.insert_temp(play_id, !playing));
    }
    x += 60.0;
    if kit::button(app, ui, Rect::from_min_size(pos2(x, y), vec2(28.0, 20.0)), "footage.setIn", "{", false) {
        kit::exec(app, "footage.setIn", json!({}));
    }
    x += 32.0;
    if kit::button(app, ui, Rect::from_min_size(pos2(x, y), vec2(28.0, 20.0)), "footage.setOut", "}", false) {
        kit::exec(app, "footage.setOut", json!({}));
    }
    x += 36.0;
    let span = format!("Δ {}", kit::seconds((b - a).seconds()));
    p.text(pos2(x, bar.center().y), Align2::LEFT_CENTER, span, Tokens::ui(11.0), t.text_dim);
    let comp_ok = app.session.active_comp_id().is_some();
    let target = app.session.active_comp_id().and_then(|c| app.session.project.item(c)).map(|i| i.name.clone()).unwrap_or_default();
    let ey = edit_row.min.y + 5.0;
    let bx = (edit_row.max.x - 238.0).max(edit_row.min.x + 10.0);
    if !target.is_empty() && bx > edit_row.min.x + 140.0 {
        p.text(pos2(bx - 8.0, edit_row.center().y), Align2::RIGHT_CENTER, tr_args("Edit Target: {}", &[&target]), Tokens::ui(11.0), t.text_dim);
    }
    if kit::button(app, ui, Rect::from_min_size(pos2(bx, ey), vec2(110.0, 20.0)), "footage.rippleInsertEdit", "Ripple Insert Edit", false) && comp_ok {
        kit::exec(app, "footage.rippleInsertEdit", json!({}));
    }
    if kit::button(app, ui, Rect::from_min_size(pos2(bx + 116.0, ey), vec2(110.0, 20.0)), "footage.overlayEdit", "Overlay Edit", true) && comp_ok {
        kit::exec(app, "footage.overlayEdit", json!({}));
    }
}

/// The room the frame is fitted into (the canvas less a 10 pt margin).
fn fitted_area(area: Rect) -> egui::Vec2 {
    vec2(area.width() - 20.0, area.height() - 20.0)
}

/// What the panel's texture shows: an item's frame, decoded from the footage as interpreted
/// (`footage`, a fingerprint), averaged down by `k`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct FrameKey {
    item: u64,
    frame: i64,
    footage: u64,
    k: usize,
}

/// A frame's pixels, averaged down, and its size in the file (`None`: not decodable).
type FramePixels = Option<(egui::ColorImage, [u32; 2])>;

/// A frame being decoded on a worker thread (set when done).
type FrameJob = (FrameKey, Arc<OnceLock<FramePixels>>);

/// The texture shown and what it shows (`None`: that frame could not be decoded).
type Shown = (FrameKey, Option<(egui::TextureHandle, [u32; 2])>);

/// Hashes what is written to it (fingerprints a value's `Debug` form without building it).
struct HashWriter(std::collections::hash_map::DefaultHasher);

impl std::fmt::Write for HashWriter {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        self.0.write(s.as_bytes());
        Ok(())
    }
}

/// A fingerprint of `f` (its file and interpretation), worked out once per project snapshot:
/// replacing or reinterpreting footage decodes again, other edits don't.
fn footage_fingerprint(ctx: &egui::Context, project: &Arc<effectcraft_engine::project::Project>, item: u64, f: &Footage) -> u64 {
    let id = egui::Id::new("footage-panel-fingerprint");
    let snap = (Arc::as_ptr(project) as usize, item);
    if let Some((s, fp)) = ctx.data(|d| d.get_temp::<((usize, u64), u64)>(id))
        && s == snap
    {
        return fp;
    }
    let mut w = HashWriter(std::collections::hash_map::DefaultHasher::new());
    let _ = std::fmt::Write::write_fmt(&mut w, format_args!("{f:?}"));
    let fp = w.0.finish();
    ctx.data_mut(|d| d.insert_temp(id, (snap, fp)));
    fp
}

/// Decode frame `key` of footage `f` at `time` and average it down by `key.k`.
fn decode_frame(source: &dyn FootageSource, item: ItemId, f: &Footage, time: Tick, key: FrameKey) -> FramePixels {
    let img = source.frame(item, f, time)?;
    let ci = egui::ColorImage::from_rgba_unmultiplied([img.width as usize, img.height as usize], &img.to_rgba8());
    Some((crate::frames::fit_texture_by(ci, usize::MAX, key.k), [img.width, img.height]))
}

/// The texture for frame `key` (at `time`): uploaded when its worker is done, the frame shown
/// before it meanwhile (as long as it is the same item's). Decoding never runs on the UI thread
/// (on the web, which has no threads, it does).
fn frame_texture(app: &EffectcraftApp, ctx: &egui::Context, key: FrameKey, time: Tick) -> Option<(egui::TextureHandle, [u32; 2])> {
    let tid = egui::Id::new("footage-panel-tex");
    let jid = egui::Id::new("footage-panel-job");
    if let Some((jkey, job)) = ctx.data(|d| d.get_temp::<FrameJob>(jid))
        && let Some(done) = job.get()
    {
        let tex = done.as_ref().map(|(ci, size)| (crate::frames::load_fitted(ctx, "footage-panel", ci.clone(), egui::TextureOptions::LINEAR), *size));
        ctx.data_mut(|d| {
            d.insert_temp::<Shown>(tid, (jkey, tex));
            d.remove::<FrameJob>(jid);
        });
    }
    let shown: Option<Shown> = ctx.data(|d| d.get_temp(tid));
    let running = ctx.data(|d| d.get_temp::<FrameJob>(jid)).is_some();
    if !running && shown.as_ref().is_none_or(|(k, _)| *k != key) {
        start_decode(app, ctx, key, time);
    }
    shown.filter(|(k, _)| k.item == key.item).and_then(|(_, tex)| tex)
}

/// Start decoding frame `key` on a worker thread (in place on the web).
fn start_decode(app: &EffectcraftApp, ctx: &egui::Context, key: FrameKey, time: Tick) {
    let jid = egui::Id::new("footage-panel-job");
    let job: Arc<OnceLock<FramePixels>> = Arc::default();
    let (project, source) = (app.session.project.clone(), app.session.footage.clone());
    let run = move |job: &OnceLock<FramePixels>| {
        let px = match project.item(ItemId(key.item)).map(|i| &i.kind) {
            Some(ItemKind::Footage(f)) => decode_frame(&*source, ItemId(key.item), f, time, key),
            _ => None,
        };
        let _ = job.set(px);
    };
    ctx.data_mut(|d| d.insert_temp::<FrameJob>(jid, (key, job.clone())));
    #[cfg(target_arch = "wasm32")]
    {
        run(&job);
        ctx.request_repaint();
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let ctx2 = ctx.clone();
        let spawned = std::thread::Builder::new().name("ec-footage-panel".into()).spawn(move || {
            run(&job);
            ctx2.request_repaint();
        });
        if spawned.is_err() {
            ctx.data_mut(|d| d.remove::<FrameJob>(jid));
        }
    }
}

//! Settings the desktop frontend applies every frame: dropped files (Import ▸ Default Drag
//! Import As), the memory watch (Memory & CPU ▸ RAM Reserved / Reduce Cache Size When System Is
//! Low on Memory), Startup ▸ Show Home Screen When Opening a Project, and Video ▸ Video Preview
//! output (a second window showing the composition frame, Mercury Transmit-style).

use egui::{Color32, Rect};
use serde_json::Value;
#[cfg(not(target_arch = "wasm32"))]
use serde_json::json;

use crate::EffectcraftApp;
use crate::frames::FrameKey;

/// Settings read once per tick, before any paint (they must keep working while hidden).
pub fn tick(app: &mut EffectcraftApp, ctx: &egui::Context, now: f64) {
    memory_watch(app, ctx, now);
    home_on_open(app, ctx);
}

/// Settings that show or draw something: run in the UI pass, before the panels draw.
pub fn frame(app: &mut EffectcraftApp, ctx: &egui::Context) {
    video_preview(app, ctx);
}

/// Import native file drops through the same command as the file picker. Run after the dock
/// layout to use the current viewer rectangle for drops that also add layers.
#[cfg(not(target_arch = "wasm32"))]
pub fn dropped_files(app: &mut EffectcraftApp, ui: &egui::Ui, bounds: Rect) {
    use egui::{Align2, Stroke, StrokeKind, vec2};

    let ctx = ui.ctx().clone();
    let hovered = ctx.input(|i| i.raw.hovered_files.len());
    if hovered != 0 {
        let label =
            if hovered == 1 { crate::i18n::tr("Drop file to import").to_string() } else { crate::i18n::tr_args("Drop {} files to import", &[&hovered]) };
        let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::unique("native-file-drop")));
        painter.rect_stroke(bounds.shrink(8.0), 6.0, Stroke::new(2.0, app.tokens.accent), StrokeKind::Inside);
        let hint = Rect::from_center_size(bounds.center(), vec2(300.0, 46.0));
        painter.rect_filled(hint, 6.0, app.tokens.panel_bg);
        painter.text(hint.center(), Align2::CENTER_CENTER, &label, crate::theme::Tokens::medium(14.0), app.tokens.text);
        app.auto.add("file.dropTarget", hint, &label);
    }

    let (paths, skipped) = ctx.input(|i| {
        let mut paths = Vec::new();
        let mut skipped = 0;
        for file in &i.raw.dropped_files {
            if file.path().is_absolute() {
                paths.push(file.path().to_string_lossy().into_owned());
            } else {
                skipped += 1;
            }
        }
        (paths, skipped)
    });

    if paths.is_empty() && skipped == 0 {
        return;
    }

    let mut errors = Vec::new();
    if skipped != 0 {
        errors.push(crate::i18n::tr_args("Skipped {} dropped file(s) without a local path. Use File ▸ Import ▸ File…", &[&skipped]));
    }

    let (proj, rest): (Vec<String>, Vec<String>) = paths.into_iter().partition(|p| {
        let l = p.to_ascii_lowercase();
        l.ends_with(".ecproj") || l.ends_with(".ecprojx")
    });

    if proj.len() > 1 {
        errors.push(crate::i18n::tr_args("Only one project can be opened per drop; skipped {} more", &[&proj.len().saturating_sub(1)]));
    }

    if let Some(p) = proj.first()
        && let Err(e) = crate::menus::invoke(app, &ctx, "file.open", json!({"path": p}))
    {
        errors.push(crate::i18n::tr_args("Cannot open dropped project {}: {}", &[&p, &e]));
    }

    if !rest.is_empty() {
        let viewer = app
            .auto
            .elements
            .iter()
            .find(|e| e.id == "viewer.area")
            .map(|e| Rect::from_min_size(egui::pos2(e.rect[0], e.rect[1]), egui::vec2(e.rect[2], e.rect[3])));

        let at = if proj.is_empty() {
            ctx.input(|i| i.pointer.hover_pos()).filter(|p| viewer.is_some_and(|v| v.contains(*p))).and_then(|p| crate::panels::viewer::screen_to_comp(&ctx, p))
        } else {
            None
        };

        // Dropped files import as stills and folders' numbered stills as sequences; Alt-drag
        // imports a folder's files one by one (After Effects).
        let alt = ctx.input(|i| i.modifiers.alt);
        // A dropped Photoshop file asks how to import it (the Import Photoshop File dialog, its
        // Import Kind starting at Default Drag Import As), as File ▸ Import ▸ File… does (#678).
        let layered = rest.iter().any(|p| p.rsplit('.').next().is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "psd" | "psb")));
        let mut params = json!({
            "paths": rest,
            "drag": true,
            "addToComp": at.is_some(),
            "position": at,
            "background": true
        });
        if !layered {
            params["importAs"] = json!(app.session.prefs.drag_import_as());
        }
        if alt {
            params["sequence"] = json!(false);
        }

        if let Err(e) = crate::menus::invoke(app, &ctx, "file.import", params) {
            errors.push(crate::i18n::tr_args("Cannot import dropped files: {}", &[&e]));
        }
    }

    if !errors.is_empty() {
        app.ui.status = errors.join("; ");
    }
}

/// Re-read the system's free memory every 30 s (cache budgets follow it; on Windows each
/// reading starts PowerShell in the background, so it isn't done more often).
pub(crate) fn memory_watch(app: &mut EffectcraftApp, ctx: &egui::Context, now: f64) {
    let id = egui::Id::unique("prefs-memory-watch");
    let last = ctx.data(|d| d.get_temp::<f64>(id));
    if last.is_some_and(|t| now - t < 30.0) {
        return;
    }
    ctx.data_mut(|d| d.insert_temp(id, now));
    // The first frame only records the time: tests and short runs don't query the system.
    if last.is_some() {
        app.session.memory_tick();
    }
}

/// Startup ▸ Show Home Screen When Opening a Project.
pub(crate) fn home_on_open(app: &mut EffectcraftApp, ctx: &egui::Context) {
    let id = egui::Id::unique("prefs-last-project");
    let path = app.session.path.clone();
    let last = ctx.data(|d| d.get_temp::<Option<String>>(id));
    ctx.data_mut(|d| d.insert_temp(id, path.clone()));
    let Some(last) = last else { return };
    let opened = app.session.journal.last().is_some_and(|(c, _)| matches!(c.as_str(), "file.open" | "file.openRecent" | "file.revert"));
    if path.is_some() && path != last && opened && app.session.prefs.startup.show_home_on_open_project {
        app.ui.start_screen = true;
    }
}

/// Video ▸ Mirror on Computer Monitor off: during playback the Composition panel leaves the
/// frame to the Video Preview window.
pub fn main_viewer_hidden(app: &EffectcraftApp) -> bool {
    let v = &app.session.prefs.video;
    v.enable_output && !v.mirror_on_monitor && app.playback.playing
}

/// Whether the Video Preview window shows now (Disable Video Output When in Background).
pub fn video_preview_active(app: &EffectcraftApp, focused: bool) -> bool {
    let v = &app.session.prefs.video;
    v.enable_output && (focused || !v.disable_when_background)
}

/// Video ▸ Enable Video Preview Output: a second native window with the current composition
/// frame, letterboxed on black. Video Device "Full Screen" opens it full screen (move it to the
/// external display first); Video Output During Playback off holds the last frame while playing.
fn video_preview(app: &mut EffectcraftApp, ctx: &egui::Context) {
    let focused = ctx.input(|i| i.focused);
    if !video_preview_active(app, focused) {
        return;
    }
    let v = app.session.prefs.video.clone();
    let tex_id = egui::Id::unique("video-preview-tex");
    let update = !app.playback.playing || v.output_during_playback;
    let key = app.viewer_shown.as_ref().map(|(_, k)| *k);
    let cur: Option<(FrameKey, egui::TextureHandle)> = ctx.data(|d| d.get_temp(tex_id));
    let tex = match (update, key, cur) {
        (true, Some(k), Some((ck, t))) if ck == k => Some(t),
        (true, Some(k), prev) => match app.viewer_pixels() {
            Some(img) => {
                let opts = crate::panels::viewer::zoom_texture_options(app.session.prefs.viewer_zoom_smooth());
                let t = match prev {
                    Some((_, mut t)) => {
                        t.set(crate::frames::fit_texture((*img).clone(), crate::frames::max_texture_side(ctx)), opts);
                        t
                    }
                    None => crate::frames::load_fitted(ctx, "video-preview", (*img).clone(), opts),
                };
                ctx.data_mut(|d| d.insert_temp(tex_id, (k, t.clone())));
                Some(t)
            }
            None => prev.map(|p| p.1),
        },
        (_, _, prev) => prev.map(|p| p.1),
    };

    let title =
        format!("Video Preview — {}", app.session.active_comp_id().and_then(|c| app.session.project.item(c)).map(|i| i.name.clone()).unwrap_or_default());

    let mut builder = egui::ViewportBuilder::default().with_title(title).with_inner_size([960.0, 540.0]);

    if v.device == "fullscreen" {
        builder = builder.with_fullscreen(true);
    }

    let mut closed = false;
    ctx.show_viewport_immediate(egui::ViewportId::from_hash_of("effectcraft-video-preview"), builder, |vctx, _| {
        egui::CentralPanel::default().frame(egui::Frame::NONE.fill(Color32::BLACK)).show(vctx, |ui| {
            let area = ui.max_rect();
            if let Some(t) = &tex {
                let [w, h] = t.size().map(|x| x.max(1) as f32);
                let s = (area.width() / w).min(area.height() / h);
                let r = Rect::from_center_size(area.center(), egui::vec2(w * s, h * s));
                ui.painter().image(t.id(), r, Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
            }
        });
        if vctx.input(|i| i.viewport().close_requested()) {
            closed = true;
        }
    });

    if closed && let Err(e) = app.set_pref("video.enableOutput", Value::Bool(false)) {
        app.ui.status = e;
    }
}

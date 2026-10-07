//! Settings the desktop frontend applies every frame: dropped files (Import ▸ Default Drag
//! Import As), the memory watch (Memory & CPU ▸ RAM Reserved / Reduce Cache Size When System Is
//! Low on Memory), Startup ▸ Show Home Screen When Opening a Project, and Video ▸ Video Preview
//! output (a second window showing the composition frame, Mercury Transmit-style).

use egui::{Color32, Rect};
use serde_json::{Value, json};

use crate::EffectcraftApp;
use crate::frames::FrameKey;

/// Run once per frame, before the panels draw.
pub fn frame(app: &mut EffectcraftApp, ctx: &egui::Context) {
    dropped_files(app, ctx);
    memory_watch(app, ctx);
    home_on_open(app, ctx);
    video_preview(app, ctx);
}

/// Files dropped on the window are imported (layered files as Default Drag Import As says).
/// Dropped on the Composition viewer, they also become layers centred there, as in After
/// Effects (#85), when the platform reports where the pointer is during file drags.
fn dropped_files(app: &mut EffectcraftApp, ctx: &egui::Context) {
    let paths: Vec<String> =
        ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_string_lossy().to_string()).filter(|p| std::path::Path::new(p).is_absolute()).collect());
    if paths.is_empty() {
        return;
    }
    let (proj, rest): (Vec<String>, Vec<String>) = paths.into_iter().partition(|p| {
        let l = p.to_ascii_lowercase();
        l.ends_with(".ecproj") || l.ends_with(".ecprojx")
    });
    if let Some(p) = proj.first()
        && let Err(e) = crate::menus::invoke(app, ctx, "file.open", json!({"path": p}))
    {
        app.ui.status = e;
    }
    if !rest.is_empty() {
        let viewer = app.auto.find("viewer.area").map(|e| Rect::from_min_size(egui::pos2(e.rect[0], e.rect[1]), egui::vec2(e.rect[2], e.rect[3])));
        let at =
            ctx.input(|i| i.pointer.hover_pos()).filter(|p| viewer.is_some_and(|v| v.contains(*p))).and_then(|p| crate::panels::viewer::screen_to_comp(ctx, p));
        let params = json!({"paths": rest, "drag": true, "importAs": app.session.prefs.drag_import_as(), "addToComp": at.is_some(), "position": at});
        if let Err(e) = crate::menus::invoke(app, ctx, "file.import", params) {
            app.ui.status = e;
        }
    }
}

/// Re-read the system's free memory every 30 s (cache budgets follow it; on Windows each
/// reading starts PowerShell in the background, so it isn't done more often).
fn memory_watch(app: &mut EffectcraftApp, ctx: &egui::Context) {
    let now = ctx.input(|i| i.time);
    let id = egui::Id::new("prefs-memory-watch");
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
fn home_on_open(app: &mut EffectcraftApp, ctx: &egui::Context) {
    let id = egui::Id::new("prefs-last-project");
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
    let tex_id = egui::Id::new("video-preview-tex");
    let update = !app.playback.playing || v.output_during_playback;
    let key = app.viewer_shown.as_ref().map(|(_, k)| *k);
    let cur: Option<(FrameKey, egui::TextureHandle)> = ctx.data(|d| d.get_temp(tex_id));
    let tex = match (update, key, cur) {
        (true, Some(k), Some((ck, t))) if ck == k => Some(t),
        (true, Some(k), prev) => match app.viewer_pixels() {
            Some(img) => {
                if let Err(error) = crate::frames::presentation_image(ctx, &img) {
                    app.ui.status = error.message().to_owned();
                    ctx.data_mut(|d| d.remove::<(FrameKey, egui::TextureHandle)>(tex_id));
                    return;
                }
                let opts = crate::panels::viewer::zoom_texture_options(app.session.prefs.viewer_zoom_smooth());
                let t = match prev {
                    Some((_, mut t)) => {
                        t.set((*img).clone(), opts);
                        t
                    }
                    None => ctx.load_texture("video-preview", (*img).clone(), opts),
                };
                ctx.data_mut(|d| d.insert_temp(tex_id, (k, t.clone())));
                Some(t)
            }
            None => prev.map(|p| p.1),
        },
        (_, _, prev) => prev.map(|p| p.1),
    };
    let tex = tex.filter(|t| crate::frames::presentation_check(ctx, t.size(), &mut app.ui.status));
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
            if let Some(t) = &tex
                && crate::frames::presentation_check(ui.ctx(), t.size(), &mut app.ui.status)
            {
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

#[cfg(test)]
mod presentation_tests {
    use super::*;

    // Headless tests inspect real output deltas but have no texture renderer.
    // Clear them even when an assertion unwinds, as required by egui 0.36.
    struct TestOutput(egui::FullOutput);
    impl std::ops::Deref for TestOutput {
        type Target = egui::FullOutput;
        fn deref(&self) -> &Self::Target {
            &self.0
        }
    }
    impl Drop for TestOutput {
        fn drop(&mut self) {
            self.0.textures_delta.clear();
        }
    }

    #[test]
    fn malformed_video_pixels_stop_before_upload_or_viewport_creation() {
        let ctx = egui::Context::default();
        let mut app = EffectcraftApp::new(effectcraft_engine::Session::default());
        app.session.prefs.video.enable_output = true;
        app.session.prefs.video.disable_when_background = false;
        let key = FrameKey { revision: 1, content: 1, comp: 1, frame: 0, scale: 1000, view: 0, opts: 0 };
        app.viewer_shown = Some((egui::TextureId::Managed(0), key));
        app.viewer_image = Some(std::sync::Arc::new(egui::ColorImage { size: [2, 1], pixels: vec![Color32::WHITE], source_size: egui::vec2(2.0, 1.0) }));
        let revision = app.session.revision;
        let output = TestOutput(ctx.run_ui(egui::RawInput::default(), |ui| video_preview(&mut app, ui.ctx())));
        assert!(app.ui.status.contains("pixel count"));
        assert!(ctx.data(|d| d.get_temp::<(FrameKey, egui::TextureHandle)>(egui::Id::new("video-preview-tex"))).is_none());
        assert!(!output.viewport_output.contains_key(&egui::ViewportId::from_hash_of("effectcraft-video-preview")));
        assert_eq!(app.session.revision, revision);
        assert!(app.frames.failure(&key).is_none(), "pane-local conversion error does not poison main renderer");
    }
}

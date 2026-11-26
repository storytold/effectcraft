//! UI-side everyday-operation timings for `effectcraft-cli bench --ops` (see
//! `effectcraft_engine::perf` for the engine side): the app's first frame, and steady frames of
//! the Timeline (scrolling a 5,000-layer comp) and the Project panel (300+ items), measured
//! headless — layout, painting and tessellation, without a window or GPU upload.

use effectcraft_engine::Session;
use effectcraft_engine::perf::Measure;

use crate::EffectcraftApp;
use crate::dock::PanelKind;

/// A headless egui driver for [`EffectcraftApp`].
pub struct Driver {
    pub ctx: egui::Context,
    pub app: EffectcraftApp,
    frame: eframe::Frame,
    time: f64,
    size: egui::Vec2,
}

impl Driver {
    pub fn new(app: EffectcraftApp, size: egui::Vec2) -> Driver {
        let ctx = egui::Context::default();
        crate::framework::install_headless(&ctx);
        Driver { ctx, app, frame: eframe::Frame::_new_kittest(), time: 0.0, size }
    }

    /// Run one frame (layout + paint + tessellation); its wall time in ms.
    pub fn step(&mut self) -> f64 {
        self.step_with(vec![])
    }

    pub fn step_with(&mut self, events: Vec<egui::Event>) -> f64 {
        self.time += 1.0 / 60.0;
        let mut input =
            egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, self.size)), time: Some(self.time), events, ..Default::default() };
        let t0 = web_time::Instant::now();
        let (app, frame) = (&mut self.app, &mut self.frame);
        eframe::App::raw_input_hook(app, &self.ctx, &mut input);
        let mut out = self.ctx.run_ui(input, |ui| {
            eframe::App::logic(app, ui.ctx(), frame);
            eframe::App::ui(app, ui, frame);
        });
        // No GPU here: texture uploads are dropped (counted in the frame time up to here).
        out.textures_delta.clear();
        let _ = self.ctx.tessellate(out.shapes, out.pixels_per_point);
        t0.elapsed().as_secs_f64() * 1000.0
    }

    /// Mean and max of `n` frames, calling `before` ahead of each.
    pub fn frames(&mut self, n: usize, mut before: impl FnMut(&mut EffectcraftApp, usize)) -> (f64, f64) {
        let (mut sum, mut max) = (0.0, 0.0f64);
        for i in 0..n {
            before(&mut self.app, i);
            let ms = self.step();
            sum += ms;
            max = max.max(ms);
        }
        (sum / n.max(1) as f64, max)
    }
}

/// The UI timings for a session holding the large project (`perf::large_project`), opened by
/// `open` (whose time counts as part of startup).
pub fn ui_ops(open: impl FnOnce() -> Session) -> Vec<Measure> {
    let mut out = vec![];
    let t0 = web_time::Instant::now();
    let session = open();
    let opened = t0.elapsed().as_secs_f64() * 1000.0;
    let t1 = web_time::Instant::now();
    let app = EffectcraftApp::new(session);
    let built = t1.elapsed().as_secs_f64() * 1000.0;
    let mut d = Driver::new(app, egui::vec2(1680.0, 1020.0));
    // The first pass installs the theme and fonts; the second draws the workspace.
    let f1 = d.step();
    let f2 = d.step();
    out.push(Measure {
        name: "startupToFirstFrame",
        ms: opened + built + f1 + f2,
        note: format!("open {opened:.1} + app {built:.1} + frames {f1:.1}/{f2:.1} ms"),
    });
    let (mean, max) = d.frames(10, |_, _| {});
    out.push(Measure { name: "workspaceFrame", ms: mean, note: format!("Standard workspace, steady (max {max:.1} ms)") });

    // Timeline maximized, scrolling through every layer of the main comp.
    let layers = d.app.session.active_comp().map_or(0, |c| c.layers.len());
    d.app.show_panel(PanelKind::Timeline);
    d.app.ui.maximized = Some(PanelKind::Timeline);
    d.step();
    let step = (layers as f32 * 20.0 / 60.0).max(40.0);
    let (mean, max) = d.frames(60, |app, i| app.ui.timeline.scroll_y = i as f32 * step);
    out.push(Measure { name: "timelineScrollFrame", ms: mean, note: format!("{layers} layers, 60 frames scrolling to the end (max {max:.1} ms)") });
    // Every 10th layer twirled open (property rows).
    let open: Vec<u64> = d.app.session.active_comp().map(|c| c.layers.iter().step_by(10).map(|l| l.id.0).collect()).unwrap_or_default();
    d.app.ui.timeline.open_layers.extend(open.iter().copied());
    let (mean, max) = d.frames(30, |app, i| app.ui.timeline.scroll_y = i as f32 * step * 2.0);
    out.push(Measure { name: "timelineOpenFrame", ms: mean, note: format!("{} layers twirled open (max {max:.1} ms)", open.len()) });
    d.app.ui.timeline.open_layers.clear();
    d.app.ui.timeline.scroll_y = 0.0;

    // Project panel maximized with every folder open.
    let items = d.app.session.project.items.len();
    let folders: Vec<u64> = d.app.session.project.items.values().filter(|i| i.is_folder()).map(|i| i.id.0).collect();
    d.app.ui.project_open_folders.extend(folders);
    d.app.show_panel(PanelKind::Project);
    d.app.ui.maximized = Some(PanelKind::Project);
    d.step();
    let (mean, max) = d.frames(30, |app, i| app.ui.project_scroll = i as f32 * 400.0);
    out.push(Measure { name: "projectPanelFrame", ms: mean, note: format!("{items} items, folders open, scrolling (max {max:.1} ms)") });
    d.app.ui.maximized = None;
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ui_ops_runs_on_a_small_project() {
        let m = ui_ops(|| {
            let mut s = Session::default();
            s.replace_project(effectcraft_engine::perf::large_project(&effectcraft_engine::perf::LargeSpec::small()), None);
            s
        });
        let names: Vec<&str> = m.iter().map(|m| m.name).collect();
        assert_eq!(names, ["startupToFirstFrame", "workspaceFrame", "timelineScrollFrame", "timelineOpenFrame", "projectPanelFrame"]);
        assert!(m.iter().all(|m| m.ms.is_finite() && m.ms > 0.0), "{m:?}");
    }
}

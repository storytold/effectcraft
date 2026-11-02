//! Composition Profiler: an explicit CPU reference capture, separate from live preview timing.

use std::sync::{Arc, Mutex, PoisonError};

use effectcraft_engine::profiler::ProfileReport;
use effectcraft_engine::project::Project;
use effectcraft_engine::{Session, time::Tick};
use egui::{Rect, pos2, vec2};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{EffectcraftApp, dock::PanelKind, theme::Tokens, widgets};

#[derive(Clone, Debug)]
struct Stamp {
    project: Arc<Project>,
    revision: u64,
    comp: Option<u64>,
    time: Tick,
    switches: bool,
    shadows: bool,
}

impl Stamp {
    #[cfg(not(target_arch = "wasm32"))]
    fn new(s: &Session) -> Self {
        Self {
            project: s.project.clone(),
            revision: s.revision,
            comp: s.active_comp_id().map(|c| c.0),
            time: s.time(),
            switches: s.prefs.general.switches_affect_nested_comps,
            shadows: s.prefs.three_d.realtime_shadows,
        }
    }
    fn matches(&self, s: &Session) -> bool {
        Arc::ptr_eq(&self.project, &s.project)
            && self.revision == s.revision
            && self.comp == s.active_comp_id().map(|c| c.0)
            && self.time == s.time()
            && self.switches == s.prefs.general.switches_affect_nested_comps
            && self.shadows == s.prefs.three_d.realtime_shadows
    }
}

#[derive(Debug)]
struct Pending {
    result: Mutex<Option<Result<ProfileReport, String>>>,
}

/// Automation can read the captured report through ui.inspect. Reports and running work never
/// restore from a saved workspace; the scale control does.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ProfilerState {
    pub scale: f64,
    #[serde(skip_deserializing)]
    pub report: Option<ProfileReport>,
    #[serde(skip_deserializing)]
    pub error: Option<String>,
    #[serde(skip_deserializing)]
    pub busy: bool,
    #[serde(skip_deserializing)]
    pub stale: bool,
    #[serde(skip)]
    pending: Option<Arc<Pending>>,
    #[serde(skip)]
    stamp: Option<Stamp>,
}

impl Default for ProfilerState {
    fn default() -> Self {
        Self { scale: 1.0, report: None, error: None, busy: false, stale: false, pending: None, stamp: None }
    }
}

/// UI invocation of comp.profile starts one owned job. Headless engine execution uses the same
/// job synchronously and returns the completed typed report as JSON.
pub fn capture(app: &mut EffectcraftApp, ctx: &egui::Context, params: Value) -> Result<Value, String> {
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (app, ctx, params);
        Err("Background composition profiling is unavailable in the web app; use comp.profile in a native session".into())
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        if app.ui.profiler.pending.is_some() {
            return Err("A composition profile is already being captured".into());
        }
        let job = app.session.profile_job(&params).map_err(|e| e.to_string())?;
        let stamp = Stamp::new(&app.session);
        let pending = Arc::new(Pending { result: Mutex::new(None) });
        let result = pending.clone();
        let repaint = ctx.clone();
        let _worker = std::thread::Builder::new()
            .name("ec-composition-profiler".into())
            .spawn(move || {
                let report = job().map_err(|e| e.to_string());
                *result.result.lock().unwrap_or_else(PoisonError::into_inner) = Some(report);
                repaint.request_repaint();
            })
            .map_err(|e| format!("Could not start composition profiling: {e}"))?;
        app.ui.profiler.report = None;
        app.ui.profiler.error = None;
        app.ui.profiler.busy = true;
        app.ui.profiler.stale = false;
        app.ui.profiler.stamp = Some(stamp);
        app.ui.profiler.pending = Some(pending);
        app.show_panel(PanelKind::CompositionProfiler);
        Ok(json!({"started": true, "pending": true, "result": "ui.inspect.ui.profiler"}))
    }
}

/// Consume completion without joining a worker. Runs even while the panel is hidden, so an
/// automation reader receives the same stale/current state that the panel shows.
pub fn tick(app: &mut EffectcraftApp) {
    let state = &mut app.ui.profiler;
    let done = state.pending.as_ref().and_then(|p| p.result.lock().unwrap_or_else(PoisonError::into_inner).take());
    if let Some(done) = done {
        state.pending = None;
        state.busy = false;
        match done {
            Ok(report) => state.report = Some(report),
            Err(error) => state.error = Some(error),
        }
    }
    state.stale = state.stamp.as_ref().is_some_and(|s| !s.matches(&app.session))
        || state.report.as_ref().is_some_and(|r| Some(r.comp) != app.session.active_comp_id().map(|c| c.0) || r.time != app.session.time());
}

pub fn show(app: &mut EffectcraftApp, ui: &mut egui::Ui, rect: Rect) {
    tick(app);
    let t = app.tokens;
    let button = Rect::from_min_size(rect.min + vec2(8.0, 8.0), vec2(128.0, 24.0));
    let enabled = !cfg!(target_arch = "wasm32") && !app.ui.profiler.busy && app.session.active_comp_id().is_some();
    let response = ui.add_enabled_ui(enabled, |ui| widgets::text_button(ui, button, "Capture Frame", true, &t, egui::Id::unique("profiler.capture"))).inner;
    app.auto.add_response("profiler.capture", &response, "Capture CPU reference frame");
    if response.clicked() {
        let scale = app.ui.profiler.scale;
        if let Err(e) = crate::menus::invoke(app, ui.ctx(), "comp.profile", json!({"scale": scale})) {
            app.ui.profiler.error = Some(e);
        }
    }
    let scales = ["Full", "Half", "Quarter", "Sixteenth"];
    let scale_values = [1.0, 0.5, 0.25, 0.0625];
    let current = scale_values.iter().position(|v| *v == app.ui.profiler.scale).unwrap_or(0);
    let dd = Rect::from_min_size(pos2(button.right() + 8.0, button.top()), vec2(92.0, 24.0));
    if let Some(index) = super::panel_kit::dropdown(app, ui, dd, "profiler.scale", &scales, current)
        && let Some(scale) = scale_values.get(index)
    {
        if let Err(error) = crate::menus::invoke(app, ui.ctx(), "comp.profileOptions", json!({"scale": scale})) {
            app.ui.profiler.error = Some(error);
        }
    }
    let body = Rect::from_min_max(pos2(rect.left(), button.bottom() + 8.0), rect.max);
    let content_rect = Rect::from_min_max(body.min, pos2((body.right() - 12.0).max(body.left()), body.bottom()));
    let p = ui.painter().with_clip_rect(content_rect);
    let scroll = widgets::PanelScroll::begin(ui, egui::Id::unique("profiler.scroll"), body);
    let mut y = body.top() + 8.0 - scroll.offset;
    let x = body.left() + 10.0;
    let width = (body.width() - 28.0).max(1.0);
    let mut line = |text: &str, strong: bool, wrap: bool| {
        let font = if strong { Tokens::semibold(12.0) } else { Tokens::ui(11.5) };
        let color = if strong { t.text } else { t.text_dim };
        if wrap {
            let galley = p.layout(text.to_string(), font, color, width);
            let height = galley.size().y;
            if y + height >= body.top() && y <= body.bottom() {
                p.galley(pos2(x, y), galley, color);
            }
            y += height + 6.0;
        } else {
            // Timing rows have fixed height. Only visible rows allocate galleys even when a
            // large profile is scrolled; the full names remain available in ui.inspect.
            let height = 18.0 * text.lines().count().max(1) as f32;
            if y + height >= body.top() && y <= body.bottom() {
                p.text(pos2(x, y), egui::Align2::LEFT_TOP, text, font, color);
            }
            y += height + 4.0;
        }
    };
    line("CPU reference • processed-layer cache bypassed", true, true);
    if cfg!(target_arch = "wasm32") {
        line("Background capture is unavailable in the web app. Use comp.profile in a native session.", false, true);
    } else if app.ui.profiler.busy {
        line("Capturing frame…", false, true);
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
    } else if app.session.active_comp_id().is_none() {
        line("Open a composition to capture its render costs.", false, true);
    }
    if let Some(error) = &app.ui.profiler.error {
        line(error, true, true);
    }
    if let Some(report) = &app.ui.profiler.report {
        if app.ui.profiler.stale {
            line("Older capture — the project, frame or render settings have changed. Capture again to update.", true, true);
        }
        line(&format!("{} • frame {} • {:.6} s", report.name, report.frame, report.time.seconds()), true, true);
        line(&format!("{} × {} • {:.2}% • {:.3} ms total", report.width, report.height, report.scale * 100.0, report.total_ms), true, true);
        for row in &report.layers {
            let kind = if row.processing_only { "preparation only" } else { "inclusive" };
            line(
                &format!(
                    "{}  [comp {}, layer {}, depth {}]\n{:.3} ms ({kind})",
                    row.layer,
                    row.comp,
                    row.layer_id,
                    row.depth,
                    row.process_ms + row.composite_ms
                ),
                true,
                false,
            );
            for effect in &row.effect_instances {
                line(&format!("  #{} {} [@{}]  {:.3} ms", effect.index.saturating_add(1), effect.name, effect.uid, effect.ms), false, false);
            }
            if row.truncated {
                line("Effect details truncated.", false, true);
            }
        }
        for row in &report.unmeasured {
            line(&format!("{} [layer {}] — not individually timed", row.name, row.layer_id), false, false);
        }
        for note in &report.notes {
            line(note, false, true);
        }
    } else if !app.ui.profiler.busy {
        line("Capture a frame to see layer and effect render times.", false, true);
    }
    let content = y - (body.top() - scroll.offset);
    scroll.end(ui, &mut app.auto, "profiler.scroll", content, &t);
}

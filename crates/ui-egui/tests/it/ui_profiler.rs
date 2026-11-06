//! Composition Profiler capture is asynchronous, inspectable, and never silently current after
//! scrubbing or replacing the project.

use effectcraft_engine::{Session, time::Tick};
use effectcraft_ui_egui::{EffectcraftApp, dock::PanelKind};
use egui_kittest::Harness;
use serde_json::json;

#[test]
fn captures_a_frame_off_thread_and_marks_it_old_after_scrubbing() {
    let mut session = Session::default();
    session.execute("comp.new", json!({"name": "Tiny", "width": 16, "height": 16, "frameRate": 25, "duration": 2})).unwrap();
    session.execute("layer.newSolid", json!({"color": "#406080"})).unwrap();
    let original = session.project.clone();
    let mut app = EffectcraftApp::new(session);
    app.show_panel(PanelKind::CompositionProfiler);
    let mut h = Harness::builder().with_size(egui::vec2(1200.0, 800.0)).build_eframe(|_| app);
    h.run_steps(3);
    assert!(h.state().auto.find("profiler.capture").is_some());
    let ctx = h.ctx.clone();
    let started = effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, "comp.profile", json!({})).unwrap();
    assert_eq!(started["pending"], true);
    assert!(effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, "comp.profile", json!({})).is_err(), "only one capture can run");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while h.state().ui.profiler.busy && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(1));
        h.run_steps(1);
    }
    assert!(!h.state().ui.profiler.busy, "capture did not complete");
    assert!(h.state().ui.profiler.error.is_none(), "{:?}", h.state().ui.profiler.error);
    assert!(h.state().ui.profiler.report.is_some());
    assert!(!h.state().ui.profiler.stale);
    assert!(std::sync::Arc::ptr_eq(&original, &h.state().session.project));
    h.state_mut().session.set_time(Tick::from_seconds_f64(0.4));
    h.run_steps(2);
    assert!(h.state().ui.profiler.stale);
    assert_eq!(h.state().ui.profiler.report.as_ref().unwrap().time, Tick::ZERO);
    // A restored state can keep the panel's options, never timings or a running worker.
    let saved = serde_json::to_value(&h.state().ui.profiler).unwrap();
    let restored: effectcraft_ui_egui::panels::profiler::ProfilerState = serde_json::from_value(saved).unwrap();
    assert!(restored.report.is_none());
    assert!(!restored.busy);
}

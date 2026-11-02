//! Headless look at the Render Queue panel (wgpu offscreen). Ignored by default (needs a GPU
//! adapter); run with
//! `RQ_SNAPSHOT=/abs/out.png cargo test -p effectcraft-ui-egui --test it render_queue_snapshot:: -- --ignored`.

use crate::gpu_test::RenderSerial;
use effectcraft_engine::Session;
use effectcraft_engine::project::render_queue::RenderStatus;
use effectcraft_ui_egui::EffectcraftApp;
use egui_kittest::Harness;
use serde_json::json;

#[test]
#[ignore]
fn render_queue_panel_snapshot() {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    s.execute("renderQueue.add", json!({"output": "/renders/[compName].[fileExtension]", "resolution": "half", "timeSpan": "comp"})).unwrap();
    s.execute("renderQueue.add", json!({"format": "prores", "channels": "rgba", "output": "/renders/[compName]_alpha.mov"})).unwrap();
    s.execute("renderQueue.add", json!({"format": "png", "output": "/renders/frames/[compName]_[#####].png", "timeSpan": "workArea"})).unwrap();
    s.execute("renderQueue.add", json!({"format": "gif", "resolution": "quarter"})).unwrap();
    {
        let p = std::sync::Arc::make_mut(&mut s.project);
        p.render_queue[0].status = RenderStatus::Done;
        p.render_queue[0].started = Some(1_790_000_000);
        p.render_queue[0].render_time = Some(12.4);
        p.render_queue[1].status = RenderStatus::Failed("disk full".into());
        p.render_queue[1].started = Some(1_790_000_013);
        p.render_queue[1].render_time = Some(0.8);
        p.render_queue[3].render = false;
        p.render_queue[3].status = RenderStatus::Unqueued;
    }
    let ids: Vec<u64> = s.project.render_queue.iter().take(3).map(|i| i.id).collect();
    let mut app = Some(EffectcraftApp::new(s));
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_eframe(|_| app.take().expect("app"));
    h.ctx.data_mut(|d| d.insert_temp(egui::Id::unique("rq-ui"), (ids, None::<u64>)));
    h.state_mut().show_panel(effectcraft_ui_egui::dock::PanelKind::RenderQueue);
    h.run_steps(4);
    let img = h.render_serialized().expect("render");
    let out = std::env::var("RQ_SNAPSHOT").unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../target/test-out/render_queue.png").into());
    if let Some(d) = std::path::Path::new(&out).parent() {
        std::fs::create_dir_all(d).unwrap();
    }
    img.save(&out).unwrap();
}

/// Edit ▸ Templates ▸ Output Module… (`RQ_TEMPLATES_SNAPSHOT=/abs/out.png`).
#[test]
#[ignore]
fn templates_dialog_snapshot() {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    s.execute("renderQueue.add", json!({"output": "/renders/[compName].[fileExtension]"})).unwrap();
    s.execute("renderQueue.saveTemplate", json!({"kind": "outputModule", "name": "Web Delivery", "item": s.project.render_queue[0].id, "params": {"format": "webm", "resize": {"preset": "HDTV 720"}}}))
        .unwrap();
    let kind = std::env::var("RQ_TEMPLATES_KIND").unwrap_or_else(|_| "outputModule".into());
    s.execute("app.templates", json!({"kind": kind})).unwrap();
    let mut app = Some(EffectcraftApp::new(s));
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_| app.take().expect("app"));
    h.run_steps(4);
    assert!(h.state().dialog.is_some(), "the Templates dialog is open");
    let img = h.render_serialized().expect("render");
    let out = std::env::var("RQ_TEMPLATES_SNAPSHOT").unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../target/test-out/rq_templates.png").into());
    if let Some(d) = std::path::Path::new(&out).parent() {
        std::fs::create_dir_all(d).unwrap();
    }
    img.save(&out).unwrap();
}

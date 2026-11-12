//! The window title the shared app reports (project name, ` *` while modified), which hosts that
//! cannot use `ViewportCommand::Title` (the browser tab) read through `window_title`.

use effectcraft_engine::Session;
use effectcraft_ui_egui::EffectcraftApp;
use egui::vec2;
use egui_kittest::Harness;
use serde_json::json;

#[test]
fn the_reported_title_follows_the_open_document() {
    let mut h = Harness::builder().with_size(vec2(800.0, 600.0)).build_eframe(|_| EffectcraftApp::new(Session::default()));
    h.run_steps(3);
    let clean = h.state().window_title().to_owned();
    assert!(clean.starts_with("EffectCraft - ") && !clean.ends_with(" *"), "{clean}");

    h.state_mut().session.execute("comp.new", json!({"name": "Edited", "width": 64, "height": 64, "duration": 1})).unwrap();
    h.run_steps(3);
    let dirty = h.state().window_title().to_owned();
    assert!(dirty.starts_with("EffectCraft - ") && dirty.ends_with(" *"), "{dirty}");
}

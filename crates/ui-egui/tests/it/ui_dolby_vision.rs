//! The Project panel's details say what Dolby Vision configuration a selected movie carries, on
//! its codec's line, inside the details header.

use effectcraft_engine::Session;
use effectcraft_engine::project::{DolbyVisionInfo, Footage, FootageKind, ItemKind};
use effectcraft_time::Tick;
use effectcraft_ui_egui::EffectcraftApp;
use egui_kittest::Harness;
use serde_json::json;

/// The Project panel with movie `dolby_vision` selected.
fn harness(dolby_vision: Option<DolbyVisionInfo>) -> Harness<'static, EffectcraftApp> {
    let mut s = Session::default();
    let footage = Footage {
        kind: FootageKind::Video,
        path: "/m/iphone.mov".into(),
        width: 64,
        height: 36,
        has_video: true,
        duration: Tick::from_seconds_f64(2.0),
        codec: "HEVC".into(),
        dolby_vision,
        ..Default::default()
    };
    let clip = std::sync::Arc::make_mut(&mut s.project).add_item("iphone.mov", Default::default(), None, ItemKind::Footage(footage));
    s.execute("project.select", json!({"items": [clip.0]})).unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_eframe(|_| EffectcraftApp::new(s));
    h.run_steps(4);
    h
}

#[test]
fn the_project_panel_details_show_the_dolby_vision_configuration() {
    let h = harness(Some(DolbyVisionInfo { profile: 8, level: 4, compatibility: 4 }));
    let line = h.state().auto.find("project.details.2").expect("the codec line").clone();
    assert_eq!(line.label, "HEVC, Dolby Vision 8.4 (HLG base layer)");
    let panel = h.state().auto.find("panel.Project").expect("the Project panel").clone();
    // Inside the 74-point details header, with the other lines.
    assert!(line.rect[1] + line.rect[3] <= panel.rect[1] + 74.0, "{line:?} in {panel:?}");
    assert!(h.state().auto.find("project.details.3").is_none(), "no fourth line");

    let h = harness(None);
    assert_eq!(h.state().auto.find("project.details.2").expect("the codec line").label, "HEVC");
}

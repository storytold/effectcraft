//! Layout collisions must not dispatch both the physical and logical shortcuts.
use effectcraft_engine::Session;
use effectcraft_ui_egui::EffectcraftApp;
use egui::{Event, Key, Modifiers, vec2};
use egui_kittest::Harness;
use serde_json::json;

#[test]
fn physical_shortcuts_distinguish_azerty_zoom_keys() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"width": 320, "height": 180, "duration": 60})).unwrap();
    let mut app = EffectcraftApp::new(s);
    app.ui.timeline.pps = Some(100.0);
    for (command, keys) in [("timeline.zoomIn", "Physical+6"), ("timeline.zoomOut", "Physical+Minus")] {
        app.session.execute("shortcuts.set", json!({"command": command, "keys": keys})).unwrap();
    }
    let mut h = Harness::builder().with_size(vec2(1200.0, 800.0)).build_eframe(|_| app);
    h.run_steps(3);
    h.state_mut().ui.timeline.pps = Some(100.0);
    for (physical, expected) in [(Key::Num6, 150.0), (Key::Minus, 100.0)] {
        h.event(Event::Key { key: Key::Minus, physical_key: Some(physical), pressed: true, repeat: false, modifiers: Modifiers::NONE });
        h.step();
        h.event(Event::Key { key: Key::Minus, physical_key: Some(physical), pressed: false, repeat: false, modifiers: Modifiers::NONE });
        h.step();
        assert!(
            (h.state().ui.timeline.pps.unwrap() - expected).abs() < 0.01,
            "physical={physical:?} expected={expected} actual={:?}",
            h.state().ui.timeline.pps
        );
    }
}

#[test]
fn work_area_navigation_uses_last_included_frame() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"width": 320, "height": 180, "duration": 10, "frameRate": 30})).unwrap();
    s.execute("comp.workArea", json!({"start": 2.0, "end": 5.0})).unwrap();
    assert_eq!(s.execute("time.workAreaStart", json!({})).unwrap()["frame"], 60);
    assert_eq!(s.execute("time.workAreaEnd", json!({})).unwrap()["frame"], 149);
}

#[test]
fn shifted_one_toggles_only_selected_video_and_iso_key_maximizes() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"width": 320, "height": 180})).unwrap();
    s.execute("layer.newSolid", json!({"name": "Unselected"})).unwrap();
    let selected = s.execute("layer.newSolid", json!({"name": "Selected"})).unwrap()["layer"].as_u64().unwrap();
    s.execute("layer.select", json!({"layers": [selected]})).unwrap();
    let mut app = EffectcraftApp::new(s);
    app.session.execute("shortcuts.set", json!({"command": "layer.setSwitch", "params": {"switch": "video"}, "keys": "Shift+1"})).unwrap();
    app.session.execute("shortcuts.set", json!({"command": "window.maximizePanel", "keys": "IntlBackslash"})).unwrap();
    let mut h = Harness::builder().with_size(vec2(1200.0, 800.0)).build_eframe(|_| app);
    h.run_steps(3);
    for visible in [false, true] {
        h.event(Event::Key { key: Key::Num1, physical_key: Some(Key::Num1), pressed: true, repeat: false, modifiers: Modifiers::SHIFT });
        h.step();
        h.event(Event::Key { key: Key::Num1, physical_key: Some(Key::Num1), pressed: false, repeat: false, modifiers: Modifiers::SHIFT });
        h.step();
        for layer in &h.state().session.active_comp().unwrap().layers {
            assert_eq!(layer.switches.video, layer.id.0 != selected || visible);
        }
    }
    h.event(Event::Key { key: Key::IntlBackslash, physical_key: Some(Key::IntlBackslash), pressed: true, repeat: false, modifiers: Modifiers::NONE });
    h.step();
    assert!(h.state().ui.maximized.is_some());
}

#[test]
fn timeline_shift_p_reveals_instead_of_adding_a_text_animator() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"width": 320, "height": 180})).unwrap();
    s.execute("layer.newSolid", json!({"name": "Box"})).unwrap();
    let mut app = EffectcraftApp::new(s);
    app.session.execute("shortcuts.set", json!({"command": "layer.addTextAnimator", "params": {"properties": ["position"]}, "keys": "Shift+P"})).unwrap();
    let mut h = Harness::builder().with_size(vec2(1200.0, 800.0)).build_eframe(|_| app);
    h.run_steps(3);
    h.state_mut().ui.focused = effectcraft_ui_egui::dock::PanelKind::Timeline;
    h.event(Event::Key { key: Key::P, physical_key: Some(Key::P), pressed: true, repeat: false, modifiers: Modifiers::SHIFT });
    h.step();
    assert!(h.state().ui.timeline.reveal.contains(&"position".to_string()));
    assert!(h.state().ui.status.is_empty());
}

//! Info panel and the 3D Camera Tracker block of Effect Controls (docs/gaps.md gap 25): what these
//! paint as text is also in the accessibility tree, as read-outs a screen reader can speak, and
//! the Analyze / Create Camera buttons are named and work from the keyboard.

use effectcraft_engine::Session;
use effectcraft_ui_egui::EffectcraftApp;
use effectcraft_ui_egui::dock::PanelKind;
use egui::accesskit::Role;
use egui::{Event, Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use serde_json::json;

fn harness(panel: PanelKind) -> Harness<'static, EffectcraftApp> {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Main", "width": 320, "height": 180, "duration": 4})).unwrap();
    s.execute("layer.newSolid", json!({"name": "Red", "color": "#c04040"})).unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_eframe(|_| EffectcraftApp::new(s));
    h.state_mut().show_panel(panel);
    h.state_mut().ui.maximized = Some(panel);
    h.run_steps(4);
    h
}

fn labels(h: &Harness<'_, EffectcraftApp>) -> Vec<String> {
    h.query_all_by_role(Role::Label).filter_map(|n| n.accesskit_node().label()).filter(|l| !l.trim().is_empty()).collect()
}

#[test]
fn info_describes_the_colour_the_selection_and_the_time() {
    let mut h = harness(PanelKind::Info);
    let l = labels(&h);
    for want in ["Red: 0", "Green: 0", "Blue: 0", "Alpha: 0"] {
        assert!(l.iter().any(|x| x == want), "`{want}` in {l:?}");
    }
    assert!(l.iter().any(|x| x == "Selected layers: Red"), "the selected layer: {l:?}");
    assert!(l.iter().any(|x| x.starts_with("Time: ")), "the time: {l:?}");
    // With one layer selected: its Duration, In and Out.
    assert!(l.iter().any(|x| x.starts_with("Duration: ")) && l.iter().any(|x| x.starts_with("In: ")), "{l:?}");
    // Nothing selected: the panel says so.
    h.state_mut().session.execute("layer.select", json!({"layers": []})).unwrap();
    h.run_steps(4);
    assert!(labels(&h).iter().any(|x| x == "Selected layers: No layer selected"), "{:?}", labels(&h));
    assert!(!labels(&h).iter().any(|x| x.starts_with("Duration: ")));
}

#[test]
fn the_camera_tracker_block_is_named_and_works_from_the_keyboard() {
    let mut h = harness(PanelKind::EffectControls);
    h.state_mut().session.execute("effect.apply", json!({"effect": effectcraft_engine::effects::camera_tracker::ID})).unwrap();
    h.run_steps(4);
    // Applying the effect starts its analysis; while it runs, Analyze is unavailable and Cancel is
    // there. Wait for it to end.
    if h.state().session.is_camera_analyzing() {
        assert!(h.get_by_role_and_label(Role::Button, "Analyze").accesskit_node().is_disabled(), "Analyze is unavailable while the analysis runs");
    }
    for _ in 0..200 {
        if !h.state().session.is_camera_analyzing() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
        h.run_steps(2);
    }
    assert!(!h.state().session.is_camera_analyzing(), "the analysis ended");
    h.run_steps(4);
    let analyze = h.get_by_role_and_label(Role::Button, "Analyze");
    assert!(!analyze.accesskit_node().is_disabled());
    assert!(labels(&h).iter().any(|x| x.contains("Analyze") || x.contains("solve") || x.contains("Method Used")), "the status is a read-out: {:?}", labels(&h));
    h.get_by_role_and_label(Role::Button, "Analyze").focus();
    h.run_steps(3);
    assert_eq!(h.root().children_recursive().find(|n| n.is_focused()).and_then(|n| n.accesskit_node().label()).as_deref(), Some("Analyze"));
    // Enter on the focused button starts a new analysis (the job exists for the pass it ends in).
    assert!(h.state().session.camera_job.is_none());
    h.input_mut().events.push(Event::Key { key: Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE });
    h.step();
    assert!(h.state().session.camera_job.is_some() || h.state().session.is_camera_analyzing(), "Enter on Analyze starts the analysis");
}

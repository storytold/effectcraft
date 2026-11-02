//! Stepping the current time from the keyboard: Page Down / Page Up and Cmd+Right / Cmd+Left
//! Arrow move one frame, with Shift ten.

use effectcraft_engine::Session;
use effectcraft_ui_egui::EffectcraftApp;
use effectcraft_ui_egui::dock::PanelKind;
use egui::{Event, Key, Modifiers};
use egui_kittest::Harness;
use serde_json::json;

fn key(h: &mut Harness<'_, EffectcraftApp>, key: Key, modifiers: Modifiers) {
    h.event(Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers });
    h.step();
    h.event(Event::Key { key, physical_key: None, pressed: false, repeat: false, modifiers });
    h.step();
}

#[test]
fn cmd_arrows_step_the_current_frame() {
    let mut session = Session::default();
    session.execute("comp.new", json!({"name": "Main", "width": 320, "height": 180, "frameRate": 30.0, "duration": 4.0})).unwrap();
    let cid = session.state.active_comp.unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_| EffectcraftApp::new(session));
    h.run_steps(2);
    let frame = |h: &Harness<'_, EffectcraftApp>| {
        let app = h.state();
        app.session.project.comp(cid).unwrap().frame_rate.frame_at(app.session.time())
    };
    assert_eq!(frame(&h), 0);
    let cmd = Modifiers::COMMAND;
    let cmd_shift = Modifiers::COMMAND | Modifiers::SHIFT;
    key(&mut h, Key::ArrowRight, cmd);
    assert_eq!(frame(&h), 1);
    key(&mut h, Key::PageDown, Modifiers::NONE);
    assert_eq!(frame(&h), 2);
    key(&mut h, Key::ArrowRight, cmd_shift);
    assert_eq!(frame(&h), 12);
    key(&mut h, Key::ArrowLeft, cmd);
    assert_eq!(frame(&h), 11);
    key(&mut h, Key::ArrowLeft, cmd_shift);
    assert_eq!(frame(&h), 1);
    // The plain arrows still nudge layers, not the time.
    key(&mut h, Key::ArrowRight, Modifiers::NONE);
    assert_eq!(frame(&h), 1);
}

/// With a layer selected, the plain arrow keys nudge it (1 screen pixel at the viewer
/// magnification, 10 with Shift), as After Effects does (#961).
#[test]
fn plain_arrows_nudge_the_selected_layer() {
    use effectcraft_engine::project::LayerId;
    let mut session = Session::default();
    session.execute("comp.new", json!({"name": "Main", "width": 320, "height": 180, "frameRate": 30.0, "duration": 4.0})).unwrap();
    let lid = session.execute("layer.newSolid", json!({"name": "Box", "width": 40, "height": 40, "color": "#ff0000"})).unwrap()["layer"].as_u64().unwrap();
    session.execute("layer.select", json!({"layers": [lid]})).unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_| EffectcraftApp::new(session));
    h.run_steps(2);
    let x = |h: &Harness<'_, EffectcraftApp>| {
        h.state().session.active_comp().unwrap().layer(LayerId(lid)).unwrap().props.prop("transform/position").unwrap().value.as_vec3()[0]
    };
    let frame = |h: &Harness<'_, EffectcraftApp>| {
        let app = h.state();
        app.session.time()
    };
    let (x0, t0) = (x(&h), frame(&h));
    key(&mut h, Key::ArrowRight, Modifiers::NONE);
    assert!(x(&h) > x0, "ArrowRight nudges the layer right (from {x0})");
    assert_eq!(frame(&h), t0, "nudging does not move the time");
    let x1 = x(&h);
    key(&mut h, Key::ArrowRight, Modifiers::SHIFT);
    let x2 = x(&h);
    assert!(x2 > x1 + (x1 - x0) * 5.0, "Shift+ArrowRight nudges ten times further (from {x1})");
    key(&mut h, Key::ArrowLeft, Modifiers::NONE);
    assert!(x(&h) < x2, "ArrowLeft nudges back (from {x2})");
}

/// The realistic path: clicking a Timeline row selects the layer, and the arrow keys then
/// nudge it (#961).
#[test]
fn clicking_a_timeline_row_then_arrows_nudge_the_layer() {
    use effectcraft_engine::project::LayerId;
    let mut session = Session::default();
    session.execute("comp.new", json!({"name": "Main", "width": 320, "height": 180, "frameRate": 30.0, "duration": 4.0})).unwrap();
    let lid = session.execute("layer.newSolid", json!({"name": "Box", "width": 40, "height": 40, "color": "#ff0000"})).unwrap()["layer"].as_u64().unwrap();
    session.execute("edit.deselectAll", json!({})).unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_eframe(|_| EffectcraftApp::new(session));
    h.run_steps(3);
    let row = {
        let e = h.state().auto.find(&format!("timeline.layer.{lid}.row")).expect("timeline row");
        egui::Rect::from_min_size(egui::pos2(e.rect[0], e.rect[1]), egui::vec2(e.rect[2], e.rect[3]))
    };
    let p = egui::pos2(row.min.x + 210.0, row.center().y);
    h.event(Event::PointerMoved(p));
    h.step();
    h.event(Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.step();
    h.event(Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(3);
    assert_eq!(h.state().session.state.selected_layers, vec![LayerId(lid)]);
    let x = |h: &Harness<'_, EffectcraftApp>| {
        h.state().session.active_comp().unwrap().layer(LayerId(lid)).unwrap().props.prop("transform/position").unwrap().value.as_vec3()[0]
    };
    let x0 = x(&h);
    key(&mut h, Key::ArrowRight, Modifiers::NONE);
    assert!(x(&h) > x0, "ArrowRight nudges the clicked layer (from {x0})");
}

/// The arrows nudge the selected layer from any panel except Project, which owns them for
/// its own keyboard navigation (#961).
#[test]
fn arrows_nudge_from_any_panel_but_project() {
    use effectcraft_engine::project::LayerId;
    let mut session = Session::default();
    session.execute("comp.new", json!({"name": "Main", "width": 320, "height": 180, "frameRate": 30.0, "duration": 4.0})).unwrap();
    let lid = session.execute("layer.newSolid", json!({"name": "Box", "width": 40, "height": 40, "color": "#ff0000"})).unwrap()["layer"].as_u64().unwrap();
    session.execute("layer.select", json!({"layers": [lid]})).unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_| EffectcraftApp::new(session));
    h.run_steps(2);
    let x = |h: &Harness<'_, EffectcraftApp>| {
        h.state().session.active_comp().unwrap().layer(LayerId(lid)).unwrap().props.prop("transform/position").unwrap().value.as_vec3()[0]
    };
    h.state_mut().ui.focused = PanelKind::EffectControls;
    h.run_steps(2);
    let x0 = x(&h);
    key(&mut h, Key::ArrowRight, Modifiers::NONE);
    assert!(x(&h) > x0, "arrows nudge with Effect Controls focused (from {x0})");
    // Project owns the arrows for its item navigation: the layer stays put.
    h.state_mut().ui.focused = PanelKind::Project;
    h.run_steps(2);
    let x1 = x(&h);
    key(&mut h, Key::ArrowRight, Modifiers::NONE);
    assert_eq!(x(&h), x1, "arrows do not nudge with Project focused");
}

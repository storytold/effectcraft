//! The Track Matte column's pick whip (After Effects 2023 and later): dragged onto another layer
//! it makes that layer the track matte, in one undo step, through `layer.setTrackMatte`.

use effectcraft_engine::Session;
use effectcraft_engine::project::{LayerId, MatteKind};
use effectcraft_ui_egui::EffectcraftApp;
use egui::{Event, Modifiers, PointerButton, Pos2, pos2};
use egui_kittest::Harness;
use serde_json::json;

fn harness() -> Harness<'static, EffectcraftApp> {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Main", "width": 320, "height": 180, "duration": 4, "fps": 24})).unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_eframe(|_| EffectcraftApp::new(s));
    // The Track Matte column is shown with the Modes (F4).
    h.state_mut().ui.timeline.show_modes = true;
    h.run_steps(4);
    h
}

fn center(h: &Harness<'_, EffectcraftApp>, id: &str) -> Pos2 {
    let e = h.state().auto.find(id).unwrap_or_else(|| panic!("no {id}"));
    pos2(e.rect[0] + e.rect[2] / 2.0, e.rect[1] + e.rect[3] / 2.0)
}

/// A primary-button drag from `from` to `to`, in steps.
fn drag(h: &mut Harness<'_, EffectcraftApp>, from: Pos2, to: Pos2) {
    h.input_mut().events.push(Event::PointerMoved(from));
    h.step();
    h.input_mut().events.push(Event::PointerButton { pos: from, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.step();
    for k in 1..=8 {
        h.input_mut().events.push(Event::PointerMoved(from + (to - from) * (k as f32 / 8.0)));
        h.step();
    }
    h.input_mut().events.push(Event::PointerButton { pos: to, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(3);
}

fn solid(h: &mut Harness<'_, EffectcraftApp>, name: &str) -> u64 {
    h.state_mut().session.execute("layer.newSolid", json!({"name": name, "color": "#406080"})).unwrap()["layer"].as_u64().unwrap()
}

fn matte(h: &Harness<'_, EffectcraftApp>, layer: u64) -> Option<(u64, MatteKind)> {
    h.state().session.active_comp().unwrap().layer(LayerId(layer)).unwrap().track_matte.map(|m| (m.layer.0, m.kind))
}

fn video(h: &Harness<'_, EffectcraftApp>, layer: u64) -> bool {
    h.state().session.active_comp().unwrap().layer(LayerId(layer)).unwrap().switches.video
}

fn whip(h: &mut Harness<'_, EffectcraftApp>, from: u64, to: Pos2) {
    let p = center(h, &format!("timeline.layer.{from}.trackMatte.whip"));
    drag(h, p, to);
}

#[test]
fn the_track_matte_pick_whip_sets_the_layer_it_is_dropped_on_as_the_matte() {
    let mut h = harness();
    let fill = solid(&mut h, "Fill");
    let shape = solid(&mut h, "Shape");
    h.state_mut().session.execute("layer.select", json!({"layers": []})).unwrap();
    h.run_steps(3);
    let steps = |h: &Harness<'_, EffectcraftApp>| h.state().session.history.undo.len();
    let before = steps(&h);
    // Onto itself or below the last layer: nothing.
    let own = center(&h, &format!("timeline.layer.{fill}.row"));
    whip(&mut h, fill, own);
    // (The two rows are at the top of a 1000-point-high window: well below them is empty.)
    whip(&mut h, fill, pos2(own.x, own.y + 120.0));
    assert_eq!(matte(&h, fill), None);
    assert_eq!(steps(&h), before, "no empty undo step");
    // Onto another layer: it is the matte, its video is switched off, as the menu does.
    let to = center(&h, &format!("timeline.layer.{shape}.row"));
    whip(&mut h, fill, to);
    assert_eq!(matte(&h, fill), Some((shape, MatteKind::Alpha)));
    assert!(!video(&h, shape));
    assert_eq!(steps(&h), before + 1);
    // One undo step.
    h.state_mut().session.execute("edit.undo", json!({})).unwrap();
    assert_eq!(matte(&h, fill), None);
    assert!(video(&h, shape));
}

#[test]
fn the_track_matte_pick_whip_refuses_a_cycle_and_a_camera() {
    let mut h = harness();
    let fill = solid(&mut h, "Fill");
    let shape = solid(&mut h, "Shape");
    let cam = h.state_mut().session.execute("layer.newCamera", json!({"name": "Cam"})).unwrap()["layer"].as_u64().unwrap();
    let s = &mut h.state_mut().session;
    s.execute("layer.setTrackMatte", json!({"layer": shape, "matte": fill})).unwrap();
    s.execute("layer.select", json!({"layers": []})).unwrap();
    h.run_steps(3);
    // Shape is matted by Fill: Fill can't be matted by Shape.
    let to = center(&h, &format!("timeline.layer.{shape}.row"));
    whip(&mut h, fill, to);
    assert_eq!(matte(&h, fill), None);
    // A camera is no matte.
    let to = center(&h, &format!("timeline.layer.{cam}.row"));
    whip(&mut h, fill, to);
    assert_eq!(matte(&h, fill), None);
}

#[test]
fn with_several_layers_selected_the_track_matte_pick_whip_sets_all_of_them() {
    let mut h = harness();
    let a = solid(&mut h, "A");
    let b = solid(&mut h, "B");
    let m = solid(&mut h, "Matte");
    let other = solid(&mut h, "Other");
    let s = &mut h.state_mut().session;
    s.execute("layer.trackMatte", json!({"layers": [b], "op": "luma"})).unwrap();
    s.execute("layer.select", json!({"layers": [a, b]})).unwrap();
    h.run_steps(3);
    let to = center(&h, &format!("timeline.layer.{m}.row"));
    whip(&mut h, a, to);
    assert_eq!(matte(&h, a), Some((m, MatteKind::Alpha)));
    assert_eq!(matte(&h, b), Some((m, MatteKind::Luma)), "each keeps its mode");
    assert_eq!(matte(&h, other), None);
    h.state_mut().session.execute("edit.undo", json!({})).unwrap();
    assert_eq!(matte(&h, a), None, "one undo step for all of them");
    assert_eq!(matte(&h, b).map(|(_, k)| k), Some(MatteKind::Luma));
    // The whip of a layer that isn't selected sets its own matte only.
    let to = center(&h, &format!("timeline.layer.{m}.row"));
    whip(&mut h, other, to);
    assert_eq!(matte(&h, other), Some((m, MatteKind::Alpha)));
    assert_eq!(matte(&h, a), None);
}

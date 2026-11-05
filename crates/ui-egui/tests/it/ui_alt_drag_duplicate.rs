//! Alt/Option-drag in the Composition viewer (#2968) duplicates the dragged layers and moves the
//! copies, copy and move together one undo step. A plain drag still moves the layer itself.

use effectcraft_engine::Session;
use effectcraft_engine::project::LayerId;
use effectcraft_ui_egui::EffectcraftApp;
use effectcraft_ui_egui::panels::viewer::comp_to_screen;
use egui::{Event, Modifiers, Pos2, vec2};
use egui_kittest::Harness;
use serde_json::json;

/// A 640×360 comp with two solids, A (left) and B (right), both selected.
fn harness() -> (Harness<'static, EffectcraftApp>, u64, u64) {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Main", "width": 640, "height": 360, "duration": 4})).unwrap();
    let a = s.execute("layer.newSolid", json!({"name": "A", "color": "#e04020", "width": 100, "height": 100})).unwrap()["layer"].as_u64().unwrap();
    let b = s.execute("layer.newSolid", json!({"name": "B", "color": "#2040e0", "width": 100, "height": 100})).unwrap()["layer"].as_u64().unwrap();
    s.execute("prop.set", json!({"layer": a, "path": "transform/position", "value": [150.0, 180.0]})).unwrap();
    s.execute("prop.set", json!({"layer": b, "path": "transform/position", "value": [450.0, 180.0]})).unwrap();
    s.execute("layer.select", json!({"layers": [a, b]})).unwrap();
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_eframe(|_| EffectcraftApp::new(s));
    h.run_steps(4);
    (h, a, b)
}

fn at(h: &Harness<'_, EffectcraftApp>, comp: [f32; 2]) -> Pos2 {
    comp_to_screen(&h.ctx, comp).unwrap()
}

fn zoom(h: &Harness<'_, EffectcraftApp>) -> f32 {
    (at(h, [100.0, 0.0]).x - at(h, [0.0, 0.0]).x) / 100.0
}

fn drag(h: &mut Harness<'_, EffectcraftApp>, from: Pos2, to: Pos2, modifiers: Modifiers) {
    h.input_mut().events.push(Event::PointerMoved(from));
    h.step();
    h.input_mut().events.push(Event::ModifiersChanged(modifiers));
    h.input_mut().events.push(Event::PointerButton { pos: from, button: egui::PointerButton::Primary, pressed: true, modifiers });
    h.step();
    for i in 1..=12 {
        h.input_mut().events.push(Event::PointerMoved(from + (to - from) * (i as f32 / 12.0)));
        h.step();
    }
    h.input_mut().events.push(Event::PointerButton { pos: to, button: egui::PointerButton::Primary, pressed: false, modifiers });
    h.step();
    h.input_mut().events.push(Event::ModifiersChanged(Modifiers::default()));
    h.run_steps(3);
}

fn pos(h: &Harness<'_, EffectcraftApp>, id: LayerId) -> [f64; 2] {
    let v = h.state().session.active_comp().unwrap().layer(id).unwrap().props.prop("transform/position").unwrap().value.as_vec2();
    [v[0], v[1]]
}

fn count(h: &Harness<'_, EffectcraftApp>) -> usize {
    h.state().session.active_comp().unwrap().layers.len()
}

#[test]
fn alt_drag_duplicates_the_selected_layers_and_moves_the_copies() {
    let (mut h, a, b) = harness();
    let undo = h.state().session.history.undo.len();
    let z = zoom(&h);
    let from = at(&h, [150.0, 180.0]);
    drag(&mut h, from, from + vec2(0.0, 60.0) * z, Modifiers::ALT);
    assert_eq!(count(&h), 4, "both selected layers were copied");
    assert_eq!(pos(&h, LayerId(a)), [150.0, 180.0], "the originals stay");
    assert_eq!(pos(&h, LayerId(b)), [450.0, 180.0]);
    let sel = h.state().session.state.selected_layers.clone();
    assert_eq!(sel.len(), 2, "the copies are selected");
    let mut moved: Vec<[f64; 2]> = sel.iter().map(|l| pos(&h, *l)).collect();
    moved.sort_by(|p, q| p[0].total_cmp(&q[0]));
    for (got, want) in moved.iter().zip([[150.0, 240.0], [450.0, 240.0]]) {
        assert!((got[0] - want[0]).abs() < 1.5 && (got[1] - want[1]).abs() < 1.5, "copy at {got:?}, wanted {want:?}");
    }
    assert_eq!(h.state().session.history.undo.len(), undo + 1, "copy and drag are one undo step");
    h.state_mut().session.execute("edit.undo", json!({})).unwrap();
    assert_eq!(count(&h), 2, "one undo removes the copies");
    assert_eq!(pos(&h, LayerId(a)), [150.0, 180.0]);
}

#[test]
fn a_plain_drag_moves_the_layers_without_copying() {
    let (mut h, a, _) = harness();
    let z = zoom(&h);
    let from = at(&h, [150.0, 180.0]);
    drag(&mut h, from, from + vec2(0.0, 60.0) * z, Modifiers::NONE);
    assert_eq!(count(&h), 2);
    let p = pos(&h, LayerId(a));
    assert!((p[1] - 240.0).abs() < 1.5, "the layer moved: {p:?}");
}

/// A second Alt-drag right after the first is its own undo step: its copy doesn't fold into the
/// first drag's step.
#[test]
fn a_second_alt_drag_is_its_own_undo_step() {
    let (mut h, _, _) = harness();
    let z = zoom(&h);
    let from = at(&h, [150.0, 180.0]);
    drag(&mut h, from, from + vec2(0.0, 60.0) * z, Modifiers::ALT);
    let undo = h.state().session.history.undo.len();
    let from = at(&h, [150.0, 240.0]);
    drag(&mut h, from, from + vec2(0.0, 40.0) * z, Modifiers::ALT);
    assert_eq!(count(&h), 6);
    assert_eq!(h.state().session.history.undo.len(), undo + 1);
    h.state_mut().session.execute("edit.undo", json!({})).unwrap();
    assert_eq!(count(&h), 4, "one undo removes only the second copies");
}

/// A selected child under a selected parent follows the parent: its own Position doesn't get the
/// drag a second time, with or without Alt (the child's copy is parented to the parent's copy).
#[test]
fn a_selected_child_keeps_its_place_under_its_dragged_parent() {
    for modifiers in [Modifiers::NONE, Modifiers::ALT] {
        let (mut h, a, b) = harness();
        h.state_mut().session.execute("layer.setParent", json!({"layers": [b], "parent": a})).unwrap();
        h.state_mut().session.execute("layer.select", json!({"layers": [a, b]})).unwrap();
        h.run_steps(2);
        let local = pos(&h, LayerId(b));
        let z = zoom(&h);
        let from = at(&h, [150.0, 180.0]);
        drag(&mut h, from, from + vec2(0.0, 60.0) * z, modifiers);
        let comp = h.state().session.active_comp().unwrap();
        let sel = h.state().session.state.selected_layers.clone();
        let parent = *sel.iter().find(|l| comp.layer(**l).unwrap().parent.is_none()).unwrap();
        let child = *sel.iter().find(|l| comp.layer(**l).unwrap().parent == Some(parent)).expect("the child follows the moved parent");
        let p = pos(&h, parent);
        assert!((p[1] - 240.0).abs() < 1.5, "{modifiers:?}: the parent moved: {p:?}");
        let c = pos(&h, child);
        assert!((c[0] - local[0]).abs() < 1e-6 && (c[1] - local[1]).abs() < 1e-6, "{modifiers:?}: child at {c:?}, was {local:?} under its parent");
    }
}

/// Alt-dragging a 3D layer's gizmo arrow copies the layer and moves the copy along that axis.
#[test]
fn alt_dragging_a_gizmo_arrow_moves_a_copy() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "C", "width": 640, "height": 360, "duration": 2})).unwrap();
    let solid = s.execute("layer.newSolid", json!({"color": "#ff0000", "width": 200, "height": 200})).unwrap()["layer"].as_u64().unwrap();
    s.execute("layer.setSwitch", json!({"layer": solid, "switch": "threeD", "value": true})).unwrap();
    s.execute("layer.select", json!({"layers": [solid]})).unwrap();
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_eframe(|_| EffectcraftApp::new(s));
    h.run_steps(6);
    let p0 = pos(&h, LayerId(solid));
    let [x, y, w, hh] = h.state().auto.find(&format!("viewer.gizmo.{solid}.x")).expect("x arrow registered").rect;
    let tip = Pos2::new(x + w / 2.0, y + hh / 2.0);
    drag(&mut h, tip, tip + vec2(40.0, 25.0), Modifiers::ALT);
    assert_eq!(count(&h), 2, "the layer was copied");
    assert_eq!(pos(&h, LayerId(solid)), p0, "the original stays");
    let copy = h.state().session.state.selected_layers[0];
    assert_ne!(copy, LayerId(solid), "the copy is selected");
    let p = pos(&h, copy);
    assert!(p[0] > p0[0] + 10.0 && p[1] == p0[1], "the copy moved along x: {p0:?} -> {p:?}");
}

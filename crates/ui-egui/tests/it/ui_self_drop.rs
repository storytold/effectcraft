//! #2589: a Project selection that contains the composition being dropped into must not fail the
//! whole drop. Rendering writes a file named after the composition, so the Project panel holds
//! both and selects both; the composition itself is not a layer it can hold.

use effectcraft_engine::Session;
use effectcraft_ui_egui::EffectcraftApp;
use egui::{Event, Modifiers, Pos2, Rect, pos2, vec2};
use egui_kittest::Harness;
use serde_json::json;

/// Compositions "Main" (active, 4 s at 30 fps, one solid) and "Clip".
fn harness() -> (Harness<'static, EffectcraftApp>, u64, u64) {
    let mut s = Session::default();
    let clip = s.execute("comp.new", json!({"name": "Clip", "width": 160, "height": 90, "frameRate": 30, "duration": 1})).unwrap()["comp"].as_u64().unwrap();
    let main = s.execute("comp.new", json!({"name": "Main", "width": 320, "height": 180, "frameRate": 30, "duration": 4})).unwrap()["comp"].as_u64().unwrap();
    s.execute("layer.newSolid", json!({"name": "Bottom", "color": "#406080"})).unwrap();
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_eframe(|_| EffectcraftApp::new(s));
    h.run_steps(3);
    (h, main, clip)
}

fn rect(h: &Harness<'_, EffectcraftApp>, id: &str) -> Rect {
    let e = h.state().auto.find(id).unwrap_or_else(|| panic!("no {id}")).clone();
    Rect::from_min_size(pos2(e.rect[0], e.rect[1]), vec2(e.rect[2], e.rect[3]))
}

/// Press on `from`, move to `to` in steps, release there.
fn drag(h: &mut Harness<'_, EffectcraftApp>, from: Pos2, to: Pos2) {
    h.event(Event::PointerMoved(from));
    h.step();
    h.event(Event::PointerButton { pos: from, button: egui::PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.step();
    for k in 1..=10 {
        h.event(Event::PointerMoved(from + (to - from) * (k as f32 / 10.0)));
        h.step();
    }
    h.event(Event::PointerButton { pos: to, button: egui::PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(3);
}

fn stack(h: &Harness<'_, EffectcraftApp>) -> Vec<String> {
    h.state().session.active_comp().unwrap().layers.iter().map(|l| l.name.clone()).collect()
}

fn layer_id(h: &Harness<'_, EffectcraftApp>, name: &str) -> u64 {
    h.state().session.active_comp().unwrap().layers.iter().find(|l| l.name == name).unwrap().id.0
}

/// Both the composition and the render output are selected: dragging the file onto the Timeline
/// still lands the file, and the composition is not carried along into itself.
#[test]
fn a_selection_holding_the_drop_target_does_not_fail_the_drop() {
    let (mut h, main, clip) = harness();
    h.state_mut().session.execute("project.select", json!({"items": [main, clip]})).unwrap();
    h.run_steps(2);
    let from = rect(&h, &format!("project.item.{clip}.name")).center();
    let row = rect(&h, &format!("timeline.layer.{}.row", layer_id(&h, "Bottom")));
    drag(&mut h, from, row.center());
    assert_eq!(stack(&h), ["Bottom", "Clip"], "the composition the drop lands in is left out");
    assert!(!h.state().ui.status.contains("contain itself"), "the drop did not fail: {}", h.state().ui.status);
}

/// Dragging the composition's own row onto its Timeline adds nothing and reports nothing, whether
/// the row was selected first or the drag started on an unselected row (#2589).
#[test]
fn the_composition_dropped_on_itself_adds_nothing() {
    let (mut h, main, _clip) = harness();
    h.state_mut().session.execute("project.select", json!({"items": [main]})).unwrap();
    h.run_steps(2);
    let before = stack(&h);
    let from = rect(&h, &format!("project.item.{main}.name")).center();
    let row = rect(&h, &format!("timeline.layer.{}.row", layer_id(&h, "Bottom")));
    drag(&mut h, from, row.center());
    assert_eq!(stack(&h), before, "nothing is added");
    assert!(!h.state().ui.status.contains("contain itself"), "no error: {}", h.state().ui.status);

    // The same row dragged while something else is selected: the early return for a single
    // unselected item must not hand the composition to the engine either.
    let (mut h, main, clip) = harness();
    h.state_mut().session.execute("project.select", json!({"items": [clip]})).unwrap();
    h.run_steps(2);
    let before = stack(&h);
    let from = rect(&h, &format!("project.item.{main}.name")).center();
    let row = rect(&h, &format!("timeline.layer.{}.row", layer_id(&h, "Bottom")));
    drag(&mut h, from, row.center());
    assert_eq!(stack(&h), before, "nothing is added from an unselected row either");
    assert!(!h.state().ui.status.contains("contain itself"), "no error: {}", h.state().ui.status);
}

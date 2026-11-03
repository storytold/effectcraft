//! Spacebar-drag panning in the Timeline: the Hand tool scrolls the layer rows vertically as well
//! as the time graph horizontally (#2083). A middle-drag does the same, from the layer list too
//! (#2854).

use effectcraft_engine::Session;
use effectcraft_ui_egui::EffectcraftApp;
use egui::{Event, Modifiers, Pos2, pos2, vec2};
use egui_kittest::Harness;
use serde_json::json;

/// A 10 s comp with 40 solids, enough rows to scroll.
fn harness() -> (Harness<'static, EffectcraftApp>, u64) {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Many", "width": 320, "height": 180, "frameRate": 30, "duration": 10})).unwrap();
    let mut top = 0;
    for i in 0..40 {
        top = s.execute("layer.newSolid", json!({"name": format!("Layer {i}"), "color": "#406080"})).unwrap()["layer"].as_u64().unwrap();
    }
    s.execute("layer.select", json!({"layers": []})).unwrap();
    let app = EffectcraftApp::new(s);
    let mut h = Harness::builder().with_size(vec2(1200.0, 800.0)).build_eframe(|_| app);
    h.run_steps(4);
    (h, top)
}

fn space(h: &mut Harness<'_, EffectcraftApp>, pressed: bool) {
    h.event(Event::Key { key: egui::Key::Space, physical_key: None, pressed, repeat: false, modifiers: Modifiers::NONE });
    h.step();
}

fn drag(h: &mut Harness<'_, EffectcraftApp>, from: Pos2, to: Pos2) {
    drag_with(h, egui::PointerButton::Primary, from, to);
}

fn drag_with(h: &mut Harness<'_, EffectcraftApp>, button: egui::PointerButton, from: Pos2, to: Pos2) {
    h.event(Event::PointerMoved(from));
    h.step();
    h.event(Event::PointerButton { pos: from, button, pressed: true, modifiers: Modifiers::NONE });
    h.step();
    for k in 1..=10 {
        h.event(Event::PointerMoved(from + (to - from) * (k as f32 / 10.0)));
        h.step();
    }
    h.event(Event::PointerButton { pos: to, button, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(3);
}

#[test]
fn middle_drag_pans_from_the_layer_list_and_the_bars() {
    let (mut h, layer) = harness();
    h.state_mut().ui.timeline.pps = Some(400.0);
    h.state_mut().ui.timeline.start = 1.0;
    h.run_steps(3);
    let order = |h: &Harness<'_, EffectcraftApp>| h.state().session.active_comp().unwrap().layers.iter().map(|l| l.id).collect::<Vec<_>>();
    let before = order(&h);
    let center = |r: [f32; 4]| pos2(r[0] + r[2] / 2.0, r[1] + r[3] / 2.0);
    let row = center(h.state().auto.find(&format!("timeline.layer.{layer}.row")).expect("layer row").rect);
    drag_with(&mut h, egui::PointerButton::Middle, row, row - vec2(0.0, 180.0));
    let y = h.state().ui.timeline.scroll_y;
    assert!(y > 100.0, "the list scrolled: {y}");
    assert_eq!(h.state().ui.timeline.start, 1.0, "a vertical drag leaves time alone");
    assert_eq!(order(&h), before, "the layers stay in order");
    assert!(h.state().session.state.selected_layers.is_empty(), "the press selects nothing");
    // In the time graph, just right of the row, whatever the zoom has left of its bar.
    h.state_mut().ui.timeline.scroll_y = 0.0;
    h.run_steps(2);
    let r = h.state().auto.find(&format!("timeline.layer.{layer}.row")).expect("layer row").rect;
    let graph = pos2(r[0] + r[2] + 100.0, r[1] + r[3] / 2.0);
    drag_with(&mut h, egui::PointerButton::Middle, graph, graph - vec2(80.0, 0.0));
    let start = h.state().ui.timeline.start;
    assert!(start > 1.1, "time panned: {start}");
    assert_eq!(order(&h), before);
}

#[test]
fn spacebar_drag_pans_the_rows_vertically() {
    let (mut h, layer) = harness();
    assert_eq!(h.state().ui.timeline.scroll_y, 0.0);
    let bar = h.state().auto.find(&format!("timeline.layer.{layer}.bar")).expect("layer bar").rect;
    let from = pos2(bar[0] + 40.0, bar[1] + bar[3] / 2.0);
    let timing = |h: &Harness<'_, EffectcraftApp>| {
        let l = &h.state().session.active_comp().unwrap().layers[0];
        (l.in_point.seconds(), l.out_point.seconds(), l.start_time.seconds())
    };
    let before = timing(&h);
    space(&mut h, true);
    drag(&mut h, from, from - vec2(0.0, 180.0));
    space(&mut h, false);
    let y = h.state().ui.timeline.scroll_y;
    assert!(y > 100.0, "the rows scrolled vertically: {y}");
    assert_eq!(h.state().ui.timeline.start, 0.0, "the time graph did not pan sideways");
    assert_eq!(timing(&h), before, "the layer under the hand did not move");
}

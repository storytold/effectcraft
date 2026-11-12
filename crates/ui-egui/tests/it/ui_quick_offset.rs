//! Quick Offset in the Timeline (#1012): Ctrl/Cmd+Alt dragging selected layer bars or keys
//! staggers the layers in selection order, onto subframes, as one undo step; a plain drag still
//! moves them together and Alt alone still scales a key group.

use effectcraft_engine::Session;
use effectcraft_engine::project::LayerId;
use effectcraft_engine::time::Tick;
use effectcraft_ui_egui::EffectcraftApp;
use egui::{Event, Modifiers, Pos2, pos2, vec2};
use egui_kittest::Harness;
use serde_json::json;

/// One frame at 30 fps.
const F: i64 = 8_467_200_000;

/// A 4 s, 30 fps comp with three solids, each with Opacity keys at 0 s and 1 s, revealed.
fn harness() -> (Harness<'static, EffectcraftApp>, Vec<u64>) {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Stagger", "width": 320, "height": 180, "frameRate": 30, "duration": 4})).unwrap();
    let mut ids = vec![];
    for i in 0..3 {
        let id =
            s.execute("layer.newSolid", json!({"name": format!("L{i}"), "color": "#e04020", "width": 80, "height": 80})).unwrap()["layer"].as_u64().unwrap();
        for (t, v) in [(0.0, 0.0), (1.0, 100.0)] {
            s.execute("prop.addKey", json!({"layer": id, "path": "transform/opacity", "time": t, "value": v})).unwrap();
        }
        ids.push(id);
    }
    s.execute("layer.select", json!({"layers": ids})).unwrap();
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_eframe(|_| EffectcraftApp::new(s));
    h.run_steps(3);
    let ctx = h.ctx.clone();
    effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, "timeline.reveal.opacity", json!({})).unwrap();
    h.run_steps(4);
    (h, ids)
}

fn center(h: &Harness<'_, EffectcraftApp>, id: &str) -> Pos2 {
    let e = h.state().auto.find(id).unwrap_or_else(|| panic!("no {id}"));
    pos2(e.rect[0] + e.rect[2] / 2.0, e.rect[1] + e.rect[3] / 2.0)
}

/// Press at `from`, move to `to` in small steps with `modifiers` held (a real drag), release.
fn drag(h: &mut Harness<'_, EffectcraftApp>, from: Pos2, to: Pos2, modifiers: Modifiers) {
    h.input_mut().events.push(Event::PointerMoved(from));
    h.step();
    h.input_mut().events.push(Event::ModifiersChanged(modifiers));
    h.input_mut().events.push(Event::PointerButton { pos: from, button: egui::PointerButton::Primary, pressed: true, modifiers });
    h.step();
    for i in 1..=24 {
        h.input_mut().events.push(Event::PointerMoved(from + (to - from) * (i as f32 / 24.0)));
        h.step();
    }
    h.input_mut().events.push(Event::PointerButton { pos: to, button: egui::PointerButton::Primary, pressed: false, modifiers });
    h.step();
    h.input_mut().events.push(Event::ModifiersChanged(Modifiers::default()));
    h.run_steps(2);
}

fn ctrl_alt() -> Modifiers {
    Modifiers { alt: true, ..Modifiers::COMMAND }
}

fn start(h: &Harness<'_, EffectcraftApp>, id: u64) -> i64 {
    h.state().session.active_comp().unwrap().layer(LayerId(id)).unwrap().start_time.0
}

/// The comp times (ticks) of a layer's Opacity keys.
fn key_times(h: &Harness<'_, EffectcraftApp>, id: u64) -> Vec<i64> {
    let l = h.state().session.active_comp().unwrap().layer(LayerId(id)).unwrap().clone();
    l.props.prop("transform/opacity").unwrap().keys.iter().map(|k| l.comp_time(k.time).0).collect()
}

fn opacity_uid(h: &Harness<'_, EffectcraftApp>, id: u64) -> u64 {
    h.state().session.active_comp().unwrap().layer(LayerId(id)).unwrap().props.prop("transform/opacity").unwrap().uid
}

#[test]
fn ctrl_alt_dragging_layer_bars_staggers_them_in_selection_order() {
    let (mut h, ids) = harness();
    let order = [ids[2], ids[0], ids[1]];
    for (i, id) in order.iter().enumerate() {
        h.state_mut().session.execute("layer.select", json!({"layers": [id], "add": i > 0})).unwrap();
    }
    h.run_steps(2);
    let before = h.state().session.project.clone();
    // Drag the middle layer's bar: the drag moves the last selected layer.
    let from = center(&h, &format!("timeline.layer.{}.bar", ids[0]));
    drag(&mut h, from, from + vec2(160.0, 0.0), ctrl_alt());
    let d = start(&h, order[2]);
    assert!(d > 0 && d % F == 0, "the last selected layer moves by whole frames: {d}");
    assert_eq!([start(&h, order[0]), start(&h, order[1])], [0, d / 2], "the first stays, the middle one moves half");
    assert_eq!(key_times(&h, order[1]), vec![d / 2, Tick::from_seconds_f64(1.0).0 + d / 2], "its keys come along");
    h.state_mut().session.execute("edit.undo", json!({})).unwrap();
    assert!(h.state().session.project.same_content(&before), "one undo step");
    // A plain drag still moves every selected layer together.
    h.run_steps(2);
    drag(&mut h, from, from + vec2(160.0, 0.0), Modifiers::default());
    let moved: Vec<i64> = order.iter().map(|id| start(&h, *id)).collect();
    assert!(moved[0] > 0 && moved.iter().all(|m| *m == moved[0]), "{moved:?}");
}

#[test]
fn ctrl_alt_dragging_keys_staggers_each_layers_keys_and_alt_still_scales() {
    let (mut h, ids) = harness();
    // Layer 1's keys first, then layer 0's, then layer 2's.
    let mut picks = vec![];
    for id in [ids[1], ids[0], ids[2]] {
        for t in [0.0, 1.0] {
            picks.push(json!({"layer": id, "path": "transform/opacity", "time": t}));
        }
    }
    for (i, k) in picks.iter().enumerate() {
        h.state_mut().session.execute("keys.select", json!({"keys": [k], "add": i > 0})).unwrap();
    }
    h.run_steps(2);
    let before = h.state().session.project.clone();
    let one = Tick::from_seconds_f64(1.0).0;
    let grab = center(&h, &format!("timeline.keyAt.{}.{one}", opacity_uid(&h, ids[0])));
    drag(&mut h, grab, grab + vec2(160.0, 0.0), ctrl_alt());
    let d = key_times(&h, ids[2])[0];
    assert!(d > 0 && d % F == 0, "the last layer's keys move by whole frames: {d}");
    assert_eq!(key_times(&h, ids[1]), vec![0, one], "the first layer's keys stay");
    assert_eq!(key_times(&h, ids[0]), vec![d / 2, one + d / 2], "the middle layer's keys move half, together");
    assert_eq!(key_times(&h, ids[2]), vec![d, one + d]);
    let layers: Vec<u64> = h.state().session.state.selected_keys.iter().map(|k| k.layer.0).collect();
    assert_eq!(layers, vec![ids[1], ids[1], ids[0], ids[0], ids[2], ids[2]], "the selection keeps its order");
    h.state_mut().session.execute("edit.undo", json!({})).unwrap();
    assert!(h.state().session.project.same_content(&before), "one undo step");
    // Alt alone still scales a key group in time: the first key stays, the last follows.
    h.state_mut().session.execute("keys.select", json!({"keys": [picks[2], picks[3]]})).unwrap();
    h.run_steps(2);
    let last = center(&h, &format!("timeline.keyAt.{}.{one}", opacity_uid(&h, ids[0])));
    drag(&mut h, last, last + vec2(160.0, 0.0), Modifiers { alt: true, ..Default::default() });
    let scaled = key_times(&h, ids[0]);
    assert!(scaled[0] == 0 && scaled[1] > one, "Alt scales: {scaled:?}");
    assert_eq!(key_times(&h, ids[1]), vec![0, one], "and staggers nothing");
}

#[test]
fn a_quick_offset_drag_shows_the_total_and_per_layer_offset() {
    let (mut h, ids) = harness();
    let from = center(&h, &format!("timeline.layer.{}.bar", ids[1]));
    h.input_mut().events.push(Event::PointerMoved(from));
    h.step();
    h.input_mut().events.push(Event::ModifiersChanged(ctrl_alt()));
    h.input_mut().events.push(Event::PointerButton { pos: from, button: egui::PointerButton::Primary, pressed: true, modifiers: ctrl_alt() });
    h.step();
    for i in 1..=12 {
        h.input_mut().events.push(Event::PointerMoved(from + vec2(10.0 * i as f32, 0.0)));
        h.step();
    }
    let shapes = h.output().shapes.clone();
    let text: Vec<String> = shapes
        .iter()
        .filter_map(|c| match &c.shape {
            egui::Shape::Text(t) => Some(t.galley.text().to_string()),
            _ => None,
        })
        .collect();
    assert!(text.iter().any(|t| t.starts_with("Total Offset: +") && t.contains("Per Layer: +")), "{text:?}");
}

fn click(h: &mut Harness<'_, EffectcraftApp>, at: Pos2, modifiers: Modifiers) {
    h.input_mut().events.push(Event::PointerMoved(at));
    h.step();
    h.input_mut().events.push(Event::ModifiersChanged(modifiers));
    h.input_mut().events.push(Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: true, modifiers });
    h.step();
    h.input_mut().events.push(Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: false, modifiers });
    h.step();
    h.input_mut().events.push(Event::ModifiersChanged(Modifiers::default()));
    h.run_steps(2);
}

/// A Shift-click range of layers and a box of keys, drawn from the bottom right up to the top left,
/// stagger in the comp's layer order (top first), whichever way they were made (#1012).
#[test]
fn shift_ranges_and_key_boxes_stagger_in_comp_order() {
    let (mut h, ids) = harness();
    // ids[2] is the top layer, ids[0] the bottom one. Click the bottom bar, Shift-click the top.
    let bar = |h: &Harness<'_, EffectcraftApp>, id: u64| center(h, &format!("timeline.layer.{id}.bar"));
    let (bottom, top) = (bar(&h, ids[0]), bar(&h, ids[2]));
    click(&mut h, bottom, Modifiers::default());
    click(&mut h, top, Modifiers::SHIFT);
    let selected: Vec<u64> = h.state().session.state.selected_layers.iter().map(|l| l.0).collect();
    assert_eq!(selected, vec![ids[2], ids[1], ids[0]], "a Shift range is in comp order");
    h.state_mut().session.execute("layer.quickOffset", json!({"delta": 2.0 / 30.0})).unwrap();
    assert_eq!([start(&h, ids[2]), start(&h, ids[1]), start(&h, ids[0])], [0, F, 2 * F]);
    h.state_mut().session.execute("edit.undo", json!({})).unwrap();
    h.run_steps(2);
    // A box around the 1 s keys, drawn upwards and to the left.
    let one = Tick::from_seconds_f64(1.0).0;
    let key = |h: &Harness<'_, EffectcraftApp>, id: u64| center(h, &format!("timeline.keyAt.{}.{one}", opacity_uid(h, id)));
    let (low, high) = (key(&h, ids[0]), key(&h, ids[2]));
    drag(&mut h, low + vec2(30.0, 5.0), high - vec2(30.0, 5.0), Modifiers::default());
    let order: Vec<u64> = h.state().session.state.selected_keys.iter().map(|k| k.layer.0).collect();
    assert_eq!(order, vec![ids[2], ids[1], ids[0]], "a key box is in comp order");
    h.state_mut().session.execute("keys.quickOffset", json!({"delta": 2.0 / 30.0})).unwrap();
    assert_eq!([key_times(&h, ids[2])[1], key_times(&h, ids[1])[1], key_times(&h, ids[0])[1]], [one, one + F, one + 2 * F]);
}

/// Releasing a Quick Offset drag with its row scrolled out of view ends it: a later plain drag of
/// that bar moves the selection together, as its own undo step.
#[test]
fn a_quick_offset_released_off_screen_ends() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Many", "width": 320, "height": 180, "frameRate": 30, "duration": 4})).unwrap();
    let ids: Vec<u64> =
        (0..40).map(|i| s.execute("layer.newSolid", json!({"name": format!("L{i}"), "color": "#e04020"})).unwrap()["layer"].as_u64().unwrap()).collect();
    let (top, next) = (ids[39], ids[38]);
    s.execute("layer.select", json!({"layers": [top, next]})).unwrap();
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_eframe(|_| EffectcraftApp::new(s));
    h.run_steps(3);
    let from = center(&h, &format!("timeline.layer.{top}.bar"));
    h.input_mut().events.push(Event::PointerMoved(from));
    h.step();
    h.input_mut().events.push(Event::ModifiersChanged(ctrl_alt()));
    h.input_mut().events.push(Event::PointerButton { pos: from, button: egui::PointerButton::Primary, pressed: true, modifiers: ctrl_alt() });
    h.step();
    for i in 1..=12 {
        h.input_mut().events.push(Event::PointerMoved(from + vec2(10.0 * i as f32, 0.0)));
        h.step();
    }
    // Let go of Ctrl+Alt, scroll the row away, then release the button.
    h.input_mut().events.push(Event::ModifiersChanged(Modifiers::default()));
    h.state_mut().ui.timeline.scroll_y = 10_000.0;
    h.run_steps(2);
    assert!(h.state().auto.find(&format!("timeline.layer.{top}.bar")).is_none(), "the row is out of view");
    let to = from + vec2(120.0, 0.0);
    h.input_mut().events.push(Event::PointerButton { pos: to, button: egui::PointerButton::Primary, pressed: false, modifiers: Modifiers::default() });
    h.run_steps(2);
    let staggered = (start(&h, next), start(&h, top));
    assert!(staggered.0 > 0 && staggered.1 == 0, "the Quick Offset applied: {staggered:?}");
    h.state_mut().ui.timeline.scroll_y = 0.0;
    h.run_steps(3);
    let from = center(&h, &format!("timeline.layer.{top}.bar"));
    drag(&mut h, from, from + vec2(160.0, 0.0), Modifiers::default());
    let d = start(&h, top);
    assert!(d > 0 && start(&h, next) == staggered.0 + d, "a plain drag moves both by the same amount");
    h.state_mut().session.execute("edit.undo", json!({})).unwrap();
    assert_eq!((start(&h, next), start(&h, top)), staggered, "as its own undo step");
}

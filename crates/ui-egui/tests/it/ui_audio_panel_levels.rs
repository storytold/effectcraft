//! The Audio panel's level fields (#3227): they show the selected layer's Audio Levels at the
//! current time, keyframes and expression included, as the Timeline does; typing into one on a
//! keyframed property sets a key at the current time and leaves the static value alone, and with
//! the Timeline's Link Channels on the other channel follows (#2485).

use effectcraft_engine::Session;
use effectcraft_engine::project::LayerId;
use effectcraft_ui_egui::EffectcraftApp;
use effectcraft_ui_egui::dock::PanelKind;
use egui::{Event, Key, Modifiers, PointerButton, Pos2, pos2, vec2};
use egui_kittest::Harness;
use serde_json::json;

const LEFT: &str = "audio.levelLeft";
const RIGHT: &str = "audio.levelRight";

fn center(h: &Harness<'_, EffectcraftApp>, id: &str) -> Pos2 {
    let e = h.state().auto.find(id).unwrap_or_else(|| panic!("the Audio panel shows no {id}"));
    pos2(e.rect[0] + e.rect[2] / 2.0, e.rect[1] + e.rect[3] / 2.0)
}

fn label(h: &Harness<'_, EffectcraftApp>, id: &str) -> String {
    h.state().auto.find(id).unwrap_or_else(|| panic!("the Audio panel shows no {id}")).label.clone()
}

fn click(h: &mut Harness<'_, EffectcraftApp>, id: &str) {
    let p = center(h, id);
    h.input_mut().events.push(Event::PointerMoved(p));
    h.step();
    h.input_mut().events.push(Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.step();
    h.input_mut().events.push(Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(3);
}

fn type_value(h: &mut Harness<'_, EffectcraftApp>, id: &str, v: &str) {
    click(h, id);
    h.input_mut().events.push(Event::Text(v.into()));
    h.step();
    h.input_mut().events.push(Event::Key { key: Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE });
    h.run_steps(3);
}

/// Comps Sound and Main, and a precomp layer of Sound (which has Audio Levels) in Main at
/// `levels`; its layer id.
fn sound_layer(s: &mut Session, levels: [f64; 2]) -> u64 {
    s.execute("comp.new", json!({"name": "Sound", "width": 64, "height": 36, "duration": 4})).unwrap();
    s.execute("comp.new", json!({"name": "Main", "width": 320, "height": 180, "duration": 4})).unwrap();
    let l = s.execute("layer.addItem", json!({"item": "Sound"})).unwrap()["layer"].as_u64().unwrap();
    s.execute("prop.set", json!({"layer": l, "path": "audio/levels", "value": levels})).unwrap();
    l
}

/// The harness with layer `l` selected and the Audio panel up.
fn harness(mut s: Session, l: u64) -> Harness<'static, EffectcraftApp> {
    s.execute("layer.select", json!({"layers": [l]})).unwrap();
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_eframe(|_| EffectcraftApp::new(s));
    h.state_mut().show_panel(PanelKind::Audio);
    h.run_steps(4);
    h
}

fn levels(h: &Harness<'_, EffectcraftApp>, l: u64) -> effectcraft_engine::project::Property {
    h.state().session.active_comp().unwrap().layer(LayerId(l)).unwrap().props.prop("audio/levels").unwrap().clone()
}

/// `prop.get`'s `key` (`value`: keyframed, `evaluated`: after the expression) at the current time.
fn engine_value(h: &mut Harness<'_, EffectcraftApp>, l: u64, key: &str) -> Vec<f64> {
    let r = h.state_mut().session.execute("prop.get", json!({"layer": l, "path": "audio/levels"})).unwrap();
    r[key].as_array().unwrap_or_else(|| panic!("no {key} in {r}")).iter().map(|v| v.as_f64().unwrap()).collect()
}

fn db(v: f64) -> String {
    format!("{v:.1} dB")
}

/// Keyframed Audio Levels on a layer that starts 1 s into the comp, with the current time between
/// the keys: the panel shows the value interpolated at layer time, not the static one (nor the one
/// at comp time, past the last key); typing sets a key at the current time.
#[test]
fn keyframed_levels_show_the_value_at_the_current_time_and_typing_keys_it() {
    let mut s = Session::default();
    let l = sound_layer(&mut s, [-20.0, -20.0]);
    // Keys at layer time 0 s and 1 s (the layer still starts at 0 s, so comp time = layer time).
    s.execute("prop.addKey", json!({"layer": l, "path": "audio/levels", "time": 0.0, "value": [-6.0, -12.0]})).unwrap();
    s.execute("prop.addKey", json!({"layer": l, "path": "audio/levels", "time": 1.0, "value": [0.0, 0.0]})).unwrap();
    // The layer moves 1 s later: its keys sit at comp 1 s and 2 s.
    let cid = s.active_comp_id().unwrap();
    let one = effectcraft_engine::time::Tick::from_seconds_f64(1.0);
    let layer = std::sync::Arc::make_mut(&mut s.project).comp_mut(cid).unwrap().layer_mut(LayerId(l)).unwrap();
    layer.start_time += one;
    layer.in_point += one;
    layer.out_point += one;
    s.execute("time.set", json!({"time": 1.5})).unwrap();
    let mut h = harness(s, l);

    let now = engine_value(&mut h, l, "value");
    assert!(now[0] > -6.0 && now[0] < 0.0 && now[1] > -12.0 && now[1] < 0.0, "between the keys: {now:?}");
    assert_eq!(label(&h, LEFT), db(now[0]), "Left at the current time");
    assert_eq!(label(&h, RIGHT), db(now[1]), "Right at the current time");
    assert_ne!(label(&h, LEFT), db(-20.0), "not the static value");

    type_value(&mut h, LEFT, "-1");
    let pr = levels(&h, l);
    assert_eq!(pr.value.components(), vec![-20.0, -20.0], "the static value stays");
    let keys: Vec<(f64, Vec<f64>)> = pr.keys.iter().map(|k| (k.time.seconds(), k.value.components())).collect();
    assert_eq!(keys.len(), 3, "a key at the current time: {keys:?}");
    assert_eq!(keys[0].1, vec![-6.0, -12.0], "{keys:?}");
    let lt = h.state().session.time().seconds() - 1.0;
    assert!((keys[1].0 - lt).abs() < 1e-6, "the new key is at the current layer time {lt}: {keys:?}");
    assert_eq!(keys[1].1[0], -1.0, "the typed Left: {keys:?}");
    assert!((keys[1].1[1] - now[1]).abs() < 1e-9, "Right keeps its value at that time: {keys:?}");
    assert_eq!(keys[2].1, vec![0.0, 0.0], "{keys:?}");
    assert_eq!(label(&h, LEFT), db(-1.0), "the panel shows the new key");
}

/// An expression drives Audio Levels: the panel shows what it evaluates to.
#[test]
fn expression_driven_levels_show_the_evaluated_value() {
    let mut s = effectcraft_host::session();
    let l = sound_layer(&mut s, [-6.0, -12.0]);
    s.execute("prop.setExpression", json!({"layer": l, "path": "audio/levels", "expression": "[-1, -2]"})).unwrap();
    let mut h = harness(s, l);
    assert_eq!(engine_value(&mut h, l, "evaluated"), vec![-1.0, -2.0], "the expression runs");
    assert_eq!(label(&h, LEFT), db(-1.0));
    assert_eq!(label(&h, RIGHT), db(-2.0));
}

/// Static Audio Levels: typing sets the value and adds no key.
#[test]
fn static_levels_take_a_typed_value_without_a_key() {
    let mut s = Session::default();
    let l = sound_layer(&mut s, [-6.0, -12.0]);
    let mut h = harness(s, l);
    assert_eq!(label(&h, LEFT), db(-6.0));
    assert_eq!(label(&h, RIGHT), db(-12.0));
    type_value(&mut h, RIGHT, "-3");
    let pr = levels(&h, l);
    assert_eq!(pr.value.components(), vec![-6.0, -3.0]);
    assert!(pr.keys.is_empty(), "no key on a static property");
}

/// With the Timeline's Link Channels on (#2485), typing into one channel moves the other by the
/// same dB, as in the Timeline.
#[test]
fn linked_levels_move_both_channels_by_the_same_db() {
    let mut s = Session::default();
    let l = sound_layer(&mut s, [-6.0, -12.0]);
    let mut h = harness(s, l);
    let uid = levels(&h, l).uid;
    h.state_mut().ui.timeline.toggle_link(uid, false);
    h.run_steps(2);
    assert!(h.state().ui.timeline.is_linked(uid, false));
    type_value(&mut h, LEFT, "-3");
    assert_eq!(levels(&h, l).value.components(), vec![-3.0, -9.0], "both rise 3 dB and keep their 6 dB balance");
    h.state_mut().session.execute("edit.undo", json!({})).unwrap();
    h.run_steps(2);
    assert_eq!(levels(&h, l).value.components(), vec![-6.0, -12.0], "one Undo takes back both channels");
}

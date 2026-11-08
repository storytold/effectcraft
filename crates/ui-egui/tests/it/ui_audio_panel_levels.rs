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

/// Proportional Scrubbing (After Effects 26.2): with three layers selected, a Ctrl+Alt scrub of
/// Left in the Audio panel (which shows the first selected layer) leaves that layer as it is,
/// moves the last selected by the whole change and the middle one by half, as in the Timeline.
#[test]
fn ctrl_alt_scrub_is_proportional_over_the_selected_layers() {
    let mut s = Session::default();
    let first = sound_layer(&mut s, [-20.0, -20.0]);
    let mut ids = vec![first];
    for _ in 0..2 {
        let l = s.execute("layer.addItem", json!({"item": "Sound"})).unwrap()["layer"].as_u64().unwrap();
        s.execute("prop.set", json!({"layer": l, "path": "audio/levels", "value": [-20.0, -20.0]})).unwrap();
        ids.push(l);
    }
    // Selection order: the first created (the one the panel shows), then the second, then the third.
    let mut h = harness(s, first);
    for l in &ids[1..] {
        h.state_mut().session.execute("layer.select", json!({"layers": [l], "add": true})).unwrap();
    }
    h.run_steps(2);
    assert_eq!(h.state().session.state.selected_layers.first().map(|l| l.0), Some(first), "the panel shows the first selected layer");
    let from = center(&h, LEFT);
    let ctrl_alt = Modifiers { alt: true, ctrl: true, command: true, ..Default::default() };
    h.input_mut().events.push(Event::PointerMoved(from));
    h.step();
    h.input_mut().events.push(Event::ModifiersChanged(ctrl_alt));
    h.input_mut().events.push(Event::PointerButton { pos: from, button: PointerButton::Primary, pressed: true, modifiers: ctrl_alt });
    h.step();
    let to = from + vec2(40.0, 0.0);
    for k in 1..=24 {
        h.input_mut().events.push(Event::PointerMoved(from + (to - from) * (k as f32 / 24.0)));
        h.step();
    }
    h.input_mut().events.push(Event::PointerButton { pos: to, button: PointerButton::Primary, pressed: false, modifiers: ctrl_alt });
    h.step();
    h.input_mut().events.push(Event::ModifiersChanged(Modifiers::NONE));
    h.run_steps(2);
    let moved: Vec<f64> = ids.iter().map(|l| levels(&h, *l).value.components()[0] + 20.0).collect();
    assert!(moved[2] > 1.0, "the last selected layer moved: {moved:?}");
    assert!(moved[0].abs() < 1e-9, "the first selected layer stays: {moved:?}");
    assert!((moved[1] - moved[2] / 2.0).abs() < 1e-6, "the middle one moves by half: {moved:?}");
}

/// Scrub Left in the Audio panel 40 px to the right with `modifiers`.
fn scrub_left(h: &mut Harness<'_, EffectcraftApp>, modifiers: Modifiers) {
    let from = center(h, LEFT);
    let to = from + vec2(40.0, 0.0);
    h.input_mut().events.push(Event::PointerMoved(from));
    h.step();
    h.input_mut().events.push(Event::ModifiersChanged(modifiers));
    h.input_mut().events.push(Event::PointerButton { pos: from, button: PointerButton::Primary, pressed: true, modifiers });
    h.step();
    for k in 1..=24 {
        h.input_mut().events.push(Event::PointerMoved(from + (to - from) * (k as f32 / 24.0)));
        h.step();
    }
    h.input_mut().events.push(Event::PointerButton { pos: to, button: PointerButton::Primary, pressed: false, modifiers });
    h.step();
    h.input_mut().events.push(Event::ModifiersChanged(Modifiers::NONE));
    h.run_steps(2);
}

/// An expression shifts the shown levels: a scrub moves the keyframed value under it (of the
/// shown layer and of every other selected layer) by the scrub's change, the same as without the
/// expression, not by the change plus the expression's offset; Ctrl+Alt spreads that change.
#[test]
fn a_scrub_of_expression_driven_levels_moves_every_selected_layer_by_the_scrub() {
    let mut s = effectcraft_host::session();
    let first = sound_layer(&mut s, [-20.0, -20.0]);
    let mut ids = vec![first];
    for _ in 0..2 {
        let l = s.execute("layer.addItem", json!({"item": "Sound"})).unwrap()["layer"].as_u64().unwrap();
        s.execute("prop.set", json!({"layer": l, "path": "audio/levels", "value": [-30.0, -30.0]})).unwrap();
        ids.push(l);
    }
    let mut h = harness(s, first);
    for l in &ids[1..] {
        h.state_mut().session.execute("layer.select", json!({"layers": [l], "add": true})).unwrap();
    }
    h.run_steps(2);
    let left = |h: &Harness<'_, EffectcraftApp>| -> Vec<f64> { ids.iter().map(|l| levels(h, *l).value.components()[0]).collect() };
    let undo = |h: &mut Harness<'_, EffectcraftApp>| {
        h.state_mut().session.execute("edit.undo", json!({})).unwrap();
        h.run_steps(2);
    };
    let before = left(&h);
    // Without an expression: the change a 40 px scrub makes.
    scrub_left(&mut h, Modifiers::NONE);
    let change = left(&h)[0] - before[0];
    assert!(change > 1.0, "the scrub raised Left: {before:?} -> {:?}", left(&h));
    undo(&mut h);
    assert_eq!(left(&h), before);

    for l in &ids {
        h.state_mut().session.execute("prop.setExpression", json!({"layer": l, "path": "audio/levels", "expression": "value + [10, 10]"})).unwrap();
    }
    h.run_steps(2);
    assert_eq!(label(&h, LEFT), db(-10.0), "the panel shows the expression's value");
    scrub_left(&mut h, Modifiers::NONE);
    let after = left(&h);
    for (i, (a, b)) in after.iter().zip(&before).enumerate() {
        assert!((a - b - change).abs() < 1e-6, "layer {i} moved by the scrub ({change}), not by the expression's offset too: {before:?} -> {after:?}");
    }
    assert_eq!(label(&h, LEFT), db(after[0] + 10.0), "the panel shows the moved value through the expression");
    undo(&mut h);

    // Proportional: the first selected stays, the middle takes half, the last all of it.
    let ctrl_alt = Modifiers { alt: true, ctrl: true, command: true, ..Default::default() };
    scrub_left(&mut h, ctrl_alt);
    let moved: Vec<f64> = left(&h).iter().zip(&before).map(|(a, b)| a - b).collect();
    assert!(moved[0].abs() < 1e-9, "the first selected layer stays: {moved:?}");
    assert!((moved[2] - change).abs() < 1e-6, "the last selected moves by the scrub's change ({change}): {moved:?}");
    assert!((moved[1] - change / 2.0).abs() < 1e-6, "the middle one moves by half: {moved:?}");
}

/// With Link Channels on, a scrub of Left moves Right by the same dB; Alt scrubs Left alone.
#[test]
fn a_linked_scrub_moves_both_channels_and_alt_moves_one() {
    let mut s = Session::default();
    let l = sound_layer(&mut s, [-6.0, -12.0]);
    let mut h = harness(s, l);
    let uid = levels(&h, l).uid;
    h.state_mut().ui.timeline.toggle_link(uid, false);
    h.run_steps(2);
    scrub_left(&mut h, Modifiers::NONE);
    let c = levels(&h, l).value.components();
    assert!(c[0] > -6.0, "the scrub raised Left: {c:?}");
    assert!((c[0] - c[1] - 6.0).abs() < 1e-9, "Right moved by the same dB: {c:?}");
    h.state_mut().session.execute("edit.undo", json!({})).unwrap();
    h.run_steps(2);
    assert_eq!(levels(&h, l).value.components(), vec![-6.0, -12.0], "one Undo takes back the scrub");
    scrub_left(&mut h, Modifiers { alt: true, ..Default::default() });
    let c = levels(&h, l).value.components();
    assert!(c[0] > -6.0, "the Alt scrub raised Left: {c:?}");
    assert_eq!(c[1], -12.0, "Alt leaves Right alone: {c:?}");
}

/// Typed into an expression-driven field, the value is the keyframed value under the expression
/// (what the expression reads as `value`), as in the Timeline.
#[test]
fn typing_into_expression_driven_levels_sets_the_value_under_the_expression() {
    let mut s = effectcraft_host::session();
    let l = sound_layer(&mut s, [-6.0, -12.0]);
    s.execute("prop.setExpression", json!({"layer": l, "path": "audio/levels", "expression": "value + [10, 10]"})).unwrap();
    let mut h = harness(s, l);
    assert_eq!(label(&h, LEFT), db(4.0));
    type_value(&mut h, LEFT, "-5");
    assert_eq!(levels(&h, l).value.components(), vec![-5.0, -12.0], "Left as typed, Right's own value under the expression kept");
    assert_eq!(label(&h, LEFT), db(5.0), "shown through the expression");
    assert_eq!(label(&h, RIGHT), db(-2.0));
}

/// A narrow Audio panel (200-230 px) still shows the level fields, labelled "L" / "R", and keeps
/// them inside the panel.
#[test]
fn a_narrow_audio_panel_keeps_the_level_fields_inside_it() {
    for width in [200.0, 215.0, 230.0, 320.0] {
        let mut s = Session::default();
        let l = sound_layer(&mut s, [-20.0, -20.0]);
        s.execute("layer.select", json!({"layers": [l]})).unwrap();
        let panel = egui::Rect::from_min_size(pos2(0.0, 0.0), vec2(width, 400.0));
        let mut h = Harness::builder().with_size(vec2(width + 100.0, 400.0)).build_ui_state(
            move |ui, app: &mut EffectcraftApp| {
                app.auto.begin_frame();
                effectcraft_ui_egui::panels::misc::audio(app, ui, panel);
            },
            EffectcraftApp::new(s),
        );
        h.run_steps(3);
        for id in [LEFT, RIGHT] {
            let e = h.state().auto.find(id).unwrap_or_else(|| panic!("{width} px: the Audio panel shows no {id}")).clone();
            assert!(e.rect[0] + e.rect[2] <= width, "{width} px: {id} ends at {} px, past the panel", e.rect[0] + e.rect[2]);
            assert_eq!(e.label, db(-20.0), "{width} px");
        }
    }
}

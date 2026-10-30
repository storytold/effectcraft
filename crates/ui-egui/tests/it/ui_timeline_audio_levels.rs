//! Audio Levels' channel link in the Timeline (#2485): off by default; linked, an edit to one
//! channel moves the other by the same dB, so the stereo balance stays, typed or scrubbed, on a
//! static or animated property, and one Undo takes it back.

use effectcraft_engine::Session;
use effectcraft_engine::project::LayerId;
use effectcraft_ui_egui::EffectcraftApp;
use egui::{Event, Key, Modifiers, PointerButton, Pos2, pos2, vec2};
use egui_kittest::Harness;
use serde_json::json;

fn center(h: &Harness<'_, EffectcraftApp>, id: &str) -> Pos2 {
    let e = h.state().auto.find(id).unwrap_or_else(|| panic!("no {id}"));
    pos2(e.rect[0] + e.rect[2] / 2.0, e.rect[1] + e.rect[3] / 2.0)
}

/// Press on `from`, move to `to` in `steps`, release there.
fn drag(h: &mut Harness<'_, EffectcraftApp>, from: Pos2, to: Pos2, steps: u32) {
    h.input_mut().events.push(Event::PointerMoved(from));
    h.step();
    h.input_mut().events.push(Event::PointerButton { pos: from, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.step();
    for k in 1..=steps {
        h.input_mut().events.push(Event::PointerMoved(from + (to - from) * (k as f32 / steps as f32)));
        h.step();
    }
    h.input_mut().events.push(Event::PointerButton { pos: to, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(3);
}

fn click(h: &mut Harness<'_, EffectcraftApp>, id: &str) {
    let p = center(h, id);
    drag(h, p, p, 1);
}

fn type_value(h: &mut Harness<'_, EffectcraftApp>, id: &str, v: &str) {
    click(h, id);
    h.input_mut().events.push(Event::Text(v.into()));
    h.step();
    h.input_mut().events.push(Event::Key { key: Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE });
    h.run_steps(3);
}

/// A precomp layer (which has Audio Levels) in comp Main at `levels`; its layer id and the
/// property's uid.
fn add_sound_layer(s: &mut Session, levels: [f64; 2]) -> (u64, u64) {
    let l = s.execute("layer.addItem", json!({"item": "Sound"})).unwrap()["layer"].as_u64().unwrap();
    s.execute("prop.set", json!({"layer": l, "path": "audio/levels", "value": levels})).unwrap();
    (l, s.active_comp().unwrap().layer(LayerId(l)).unwrap().props.prop("audio/levels").unwrap().uid)
}

/// Comps Sound and Main.
fn comps(s: &mut Session) {
    s.execute("comp.new", json!({"name": "Sound", "width": 64, "height": 36, "duration": 4})).unwrap();
    s.execute("comp.new", json!({"name": "Main", "width": 320, "height": 180, "duration": 4})).unwrap();
}

/// Twirl layer `l` open to its property `uid`.
fn reveal(h: &mut Harness<'_, EffectcraftApp>, l: u64, uid: u64) {
    h.state_mut().ui.timeline.open_layers.insert(l);
    h.state_mut().ui.timeline.layer_reveal.insert(l, vec!["props".into()]);
    h.state_mut().ui.timeline.reveal_props.insert(uid);
    h.run_steps(3);
}

fn levels_of(h: &Harness<'_, EffectcraftApp>, l: u64) -> Vec<f64> {
    h.state().session.active_comp().unwrap().layer(LayerId(l)).unwrap().props.prop("audio/levels").unwrap().value.components()
}

#[test]
fn linked_audio_levels_move_both_channels_by_the_same_db() {
    let mut s = Session::default();
    comps(&mut s);
    let (l, uid) = add_sound_layer(&mut s, [-6.0, -12.0]);
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_eframe(|_| EffectcraftApp::new(s));
    reveal(&mut h, l, uid);
    let levels =
        |h: &Harness<'_, EffectcraftApp>| h.state().session.active_comp().unwrap().layer(LayerId(l)).unwrap().props.prop("audio/levels").unwrap().clone();
    let set = |h: &mut Harness<'_, EffectcraftApp>, v: [f64; 2]| {
        h.state_mut().session.execute("prop.set", json!({"layer": l, "path": "audio/levels", "value": v})).unwrap();
        h.run_steps(2);
    };
    let undo = |h: &mut Harness<'_, EffectcraftApp>| {
        h.state_mut().session.execute("edit.undo", json!({})).unwrap();
        h.run_steps(2);
    };
    let (left, right, link) = (format!("timeline.prop.{uid}.value.0"), format!("timeline.prop.{uid}.value.1"), format!("timeline.prop.{uid}.link"));
    assert_eq!(h.state().auto.find(&link).unwrap().label, "Link Channels");

    type_value(&mut h, &left, "-3");
    assert_eq!(levels(&h).value.components(), vec![-3.0, -12.0], "unlinked by default, one channel moves alone");
    set(&mut h, [-6.0, -12.0]);

    click(&mut h, &link);
    type_value(&mut h, &left, "-3");
    assert_eq!(levels(&h).value.components(), vec![-3.0, -9.0], "linked, both rise 3 dB and keep their 6 dB balance");
    undo(&mut h);
    assert_eq!(levels(&h).value.components(), vec![-6.0, -12.0], "one Undo takes back both channels");

    type_value(&mut h, &right, "0");
    assert_eq!(levels(&h).value.components(), vec![6.0, 0.0], "the right channel leads just the same");
    set(&mut h, [0.0, 0.0]);
    type_value(&mut h, &left, "-6");
    assert_eq!(levels(&h).value.components(), vec![-6.0, -6.0], "equal channels at 0 dB stay equal");

    set(&mut h, [-6.0, -12.0]);
    let from = center(&h, &left);
    drag(&mut h, from, from + vec2(30.0, 0.0), 6);
    let c = levels(&h).value.components();
    assert!(c[0] > -6.0, "the scrub raised the left channel: {c:?}");
    assert!((c[0] - c[1] - 6.0).abs() < 1e-9, "a linked scrub keeps the balance: {c:?}");
    undo(&mut h);
    assert_eq!(levels(&h).value.components(), vec![-6.0, -12.0], "one Undo takes back the whole scrub");

    // Animated: the edit at the current time changes that key's two channels together.
    h.state_mut().session.execute("prop.addKey", json!({"layer": l, "path": "audio/levels", "time": 0.0, "value": [-6.0, -12.0]})).unwrap();
    h.state_mut().session.execute("prop.addKey", json!({"layer": l, "path": "audio/levels", "time": 1.0, "value": [0.0, 0.0]})).unwrap();
    h.run_steps(2);
    type_value(&mut h, &left, "-10");
    let keys: Vec<Vec<f64>> = levels(&h).keys.iter().map(|k| k.value.components()).collect();
    assert_eq!(keys, vec![vec![-10.0, -16.0], vec![0.0, 0.0]], "the key at the current time moves both channels");

    click(&mut h, &link);
    type_value(&mut h, &left, "-4");
    let keys: Vec<Vec<f64>> = levels(&h).keys.iter().map(|k| k.value.components()).collect();
    assert_eq!(keys[0], vec![-4.0, -16.0], "unlinked again, the right channel stays");
}

/// With several layers selected, a linked typed Left sets each layer's Left and moves its Right
/// by that layer's own change: every layer keeps its own balance.
#[test]
fn linked_typing_keeps_each_selected_layers_own_balance() {
    let mut s = Session::default();
    comps(&mut s);
    let (a, uid) = add_sound_layer(&mut s, [-6.0, -12.0]);
    let (b, other) = add_sound_layer(&mut s, [-20.0, -30.0]);
    s.execute("layer.select", json!({"layers": [a, b]})).unwrap();
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_eframe(|_| EffectcraftApp::new(s));
    reveal(&mut h, a, uid);
    reveal(&mut h, b, other);
    click(&mut h, &format!("timeline.prop.{uid}.link"));
    type_value(&mut h, &format!("timeline.prop.{uid}.value.0"), "-3");
    assert_eq!(levels_of(&h, a), vec![-3.0, -9.0]);
    assert_eq!(levels_of(&h, b), vec![-3.0, -13.0], "B rises 17 dB on both channels, keeping its 10 dB balance");
}

/// A link turned on does not carry into a new project whose Audio Levels reuse the property uid.
#[test]
fn a_new_project_starts_with_its_audio_levels_unlinked() {
    let mut s = Session::default();
    comps(&mut s);
    let (l, uid) = add_sound_layer(&mut s, [0.0, 0.0]);
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_eframe(|_| EffectcraftApp::new(s));
    reveal(&mut h, l, uid);
    click(&mut h, &format!("timeline.prop.{uid}.link"));
    assert!(h.state().ui.timeline.is_linked(uid, false));

    h.state_mut().session.execute("file.newProject", json!({})).unwrap();
    comps(&mut h.state_mut().session);
    let (l, again) = add_sound_layer(&mut h.state_mut().session, [0.0, 0.0]);
    assert_eq!(again, uid, "the new project reuses the property uid");
    reveal(&mut h, l, again);
    assert!(!h.state().ui.timeline.is_linked(again, false), "the new project's levels start unlinked");
    type_value(&mut h, &format!("timeline.prop.{again}.value.0"), "-3");
    assert_eq!(levels_of(&h, l), vec![-3.0, 0.0]);
}

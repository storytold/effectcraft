//! Enter renames the selected effect, text animator, mask or shape group, not only the layer
//! (#1061). Enter commits, Escape cancels.

use effectcraft_engine::Session;
use effectcraft_ui_egui::EffectcraftApp;
use egui::{Event, Key, Modifiers};
use egui_kittest::Harness;
use serde_json::json;

struct Fx {
    text: u64,
    shape: u64,
    effect: u64,
    anim: u64,
    mask: u64,
    group: u64,
}

fn harness() -> (Harness<'static, EffectcraftApp>, Fx) {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Main", "width": 320, "height": 180, "duration": 4, "fps": 24})).unwrap();
    let text = s.execute("layer.newText", json!({"text": "ABCD"})).unwrap()["layer"].as_u64().unwrap();
    s.execute("layer.addTextAnimator", json!({"layer": text, "properties": ["tracking"]})).unwrap();
    let effect = s.execute("effect.apply", json!({"layer": text, "effect": "Gaussian Blur"})).unwrap()["effects"][0].as_u64().unwrap();
    let shape = s.execute("layer.newShape", json!({"kind": "star"})).unwrap()["layer"].as_u64().unwrap();
    s.execute("layer.addMask", json!({"layer": shape, "shape": "ellipse"})).unwrap();
    let first = |s: &Session, l: u64, path: &str| {
        s.active_comp().unwrap().layer(effectcraft_engine::project::LayerId(l)).unwrap().props.group(path).unwrap().groups().next().unwrap().uid
    };
    let (anim, mask, group) = (first(&s, text, "text/animators"), first(&s, shape, "masks"), first(&s, shape, "contents"));
    let mut h = Harness::builder().with_size(egui::vec2(1700.0, 1100.0)).with_step_dt(1.0 / 60.0).build_eframe(|_| EffectcraftApp::new(s));
    h.run_steps(6);
    (h, Fx { text, shape, effect, anim, mask, group })
}

fn key(h: &mut Harness<'_, EffectcraftApp>, key: Key) {
    h.input_mut().events.push(Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE });
    h.input_mut().events.push(Event::Key { key, physical_key: None, pressed: false, repeat: false, modifiers: Modifiers::NONE });
    h.run_steps(3);
}

fn name(h: &Harness<'_, EffectcraftApp>, layer: u64, uid: u64) -> String {
    h.state().session.active_comp().unwrap().layer(effectcraft_engine::project::LayerId(layer)).unwrap().props.find_group(uid).unwrap().name.clone()
}

fn layer_name(h: &Harness<'_, EffectcraftApp>, layer: u64) -> String {
    h.state().session.active_comp().unwrap().layer(effectcraft_engine::project::LayerId(layer)).unwrap().name.clone()
}

fn select(h: &mut Harness<'_, EffectcraftApp>, layer: u64, uid: u64) {
    let s = &mut h.state_mut().session;
    s.execute("layer.select", json!({"layers": [layer]})).unwrap();
    s.execute("prop.select", json!({"layer": layer, "prop": uid, "selectKeys": false})).unwrap();
    h.run_steps(3);
}

fn editing(h: &Harness<'_, EffectcraftApp>, uid: u64) -> bool {
    let ids = [format!("timeline.group.{uid}.nameEdit"), format!("effectControls.effect.{uid}.nameEdit")];
    h.state().auto.previous.iter().chain(h.state().auto.elements.iter()).any(|e| ids.contains(&e.id))
}

fn rename_with_enter(h: &mut Harness<'_, EffectcraftApp>, layer: u64, uid: u64, new: &str) {
    select(h, layer, uid);
    key(h, Key::Enter);
    assert!(editing(h, uid), "Enter opens a rename field on group {uid}");
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    h.event(Event::Text(new.into()));
    h.step();
    key(h, Key::Enter);
    assert!(!editing(h, uid), "Enter closes the field");
}

#[test]
fn enter_renames_the_selected_group_not_the_layer() {
    let (mut h, x) = harness();
    let before = (layer_name(&h, x.text), layer_name(&h, x.shape));
    for (layer, uid, new) in [(x.text, x.effect, "Delay"), (x.text, x.anim, "Wiggle"), (x.shape, x.mask, "Reveal"), (x.shape, x.group, "Body")] {
        rename_with_enter(&mut h, layer, uid, new);
        assert_eq!(name(&h, layer, uid), new);
    }
    assert_eq!((layer_name(&h, x.text), layer_name(&h, x.shape)), before, "the layers keep their names");
}

#[test]
fn escape_cancels_a_group_rename_and_a_layer_still_renames_without_a_group() {
    let (mut h, x) = harness();
    let old = name(&h, x.text, x.anim);
    select(&mut h, x.text, x.anim);
    key(&mut h, Key::Enter);
    assert!(editing(&h, x.anim));
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    h.event(Event::Text("Nope".into()));
    h.step();
    key(&mut h, Key::Escape);
    assert!(!editing(&h, x.anim));
    assert_eq!(name(&h, x.text, x.anim), old);
    // No group selected: Enter renames the layer as before.
    h.state_mut().session.execute("layer.select", json!({"layers": [x.shape]})).unwrap();
    h.run_steps(3);
    key(&mut h, Key::Enter);
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    h.event(Event::Text("Plate".into()));
    h.step();
    key(&mut h, Key::Enter);
    assert_eq!(layer_name(&h, x.shape), "Plate");
}

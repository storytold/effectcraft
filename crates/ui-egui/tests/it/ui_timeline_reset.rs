//! The Timeline's Reset buttons on the Transform and effect rows (#2726): one click resets that
//! row through the existing command, as one Undo step, without touching the selection.

use effectcraft_engine::Session;
use effectcraft_engine::project::LayerId;
use effectcraft_ui_egui::EffectcraftApp;
use effectcraft_ui_egui::dock::PanelKind;
use egui::{Event, Modifiers, PointerButton, pos2};
use egui_kittest::Harness;
use serde_json::json;

struct Fx {
    layer: u64,
    other: u64,
    transform: u64,
    effect: u64,
}

fn harness() -> (Harness<'static, EffectcraftApp>, Fx) {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Main", "width": 320, "height": 180, "duration": 4, "fps": 24})).unwrap();
    let layer = s.execute("layer.newSolid", json!({"name": "A", "color": "#406080"})).unwrap()["layer"].as_u64().unwrap();
    let other = s.execute("layer.newSolid", json!({"name": "B", "color": "#804060"})).unwrap()["layer"].as_u64().unwrap();
    let effect = s.execute("effect.apply", json!({"layer": layer, "effect": "Gaussian Blur"})).unwrap()["effects"][0].as_u64().unwrap();
    let transform = s.active_comp().unwrap().layer(LayerId(layer)).unwrap().transform().unwrap().uid;
    for l in [layer, other] {
        s.execute("prop.set", json!({"layer": l, "path": "transform/opacity", "value": 40})).unwrap();
    }
    s.execute("prop.set", json!({"layer": layer, "path": "effects/#1/blurriness", "value": 25})).unwrap();
    s.execute("layer.select", json!({"layers": [other]})).unwrap();
    let mut app = EffectcraftApp::new(s);
    app.show_panel(PanelKind::Timeline);
    app.ui.timeline.open_layers.insert(layer);
    let effects = app.session.active_comp().and_then(|c| c.layer(LayerId(layer))).and_then(|l| l.props.sub("effects")).map(|g| g.uid);
    app.ui.timeline.open_groups.extend(effects);
    app.ui.timeline.open_groups.insert(transform);
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).with_step_dt(1.0 / 60.0).build_eframe(|_| app);
    h.run_steps(6);
    (h, Fx { layer, other, transform, effect })
}

fn click_auto(h: &mut Harness<'_, EffectcraftApp>, id: &str) {
    let e = h.state().auto.find(id).unwrap_or_else(|| panic!("missing {id}")).clone();
    let pos = pos2(e.rect[0] + e.rect[2] * 0.5, e.rect[1] + e.rect[3] * 0.5);
    h.event(Event::PointerMoved(pos));
    h.step();
    for pressed in [true, false] {
        h.event(Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
        h.step();
    }
    h.run_steps(2);
}

fn num(h: &Harness<'_, EffectcraftApp>, layer: u64, path: &str) -> f64 {
    h.state().session.active_comp().unwrap().layer(LayerId(layer)).unwrap().props.prop(path).unwrap().value_at(effectcraft_time::Tick::ZERO).as_f64()
}

fn selected(h: &Harness<'_, EffectcraftApp>) -> Vec<u64> {
    h.state().session.state.selected_layers.iter().map(|l| l.0).collect()
}

#[test]
fn transform_reset_resets_only_that_layer_and_keeps_the_selection() {
    let (mut h, x) = harness();
    click_auto(&mut h, &format!("timeline.group.{}.reset", x.transform));
    assert_eq!(num(&h, x.layer, "transform/opacity"), 100.0);
    assert_eq!(num(&h, x.other, "transform/opacity"), 40.0, "other layers are untouched");
    assert_eq!(selected(&h), vec![x.other], "the selection does not change");
    h.state_mut().session.execute("edit.undo", json!({})).unwrap();
    assert_eq!(num(&h, x.layer, "transform/opacity"), 40.0, "one Undo step takes it back");
}

#[test]
fn effect_reset_resets_that_effect() {
    let (mut h, x) = harness();
    assert_eq!(num(&h, x.layer, "effects/#1/blurriness"), 25.0);
    click_auto(&mut h, &format!("timeline.group.{}.reset", x.effect));
    assert_eq!(num(&h, x.layer, "effects/#1/blurriness"), 0.0);
    assert_eq!(num(&h, x.layer, "transform/opacity"), 40.0, "the Transform is untouched");
}

#[test]
fn locked_layers_do_not_reset() {
    let (mut h, x) = harness();
    h.state_mut().session.execute("layer.setSwitch", json!({"layers": [x.layer], "switch": "locked", "value": true})).unwrap();
    h.run_steps(3);
    click_auto(&mut h, &format!("timeline.group.{}.reset", x.transform));
    assert_eq!(num(&h, x.layer, "transform/opacity"), 40.0);
}

#[test]
fn an_animated_property_takes_the_default_at_the_current_time_and_keeps_its_other_keys() {
    let (mut h, x) = harness();
    let s = &mut h.state_mut().session;
    for (t, v) in [(0.0, 20.0), (3.0, 60.0)] {
        s.execute("prop.addKey", json!({"layer": x.layer, "path": "transform/opacity", "time": t, "value": v})).unwrap();
    }
    s.execute("time.set", json!({"time": 1.5})).unwrap();
    h.run_steps(3);
    click_auto(&mut h, &format!("timeline.group.{}.reset", x.transform));
    let comp = h.state().session.active_comp().unwrap();
    let opacity = comp.layer(LayerId(x.layer)).unwrap().props.prop("transform/opacity").unwrap();
    let keys: Vec<(f64, f64)> = opacity.keys.iter().map(|k| ((k.time.seconds() * 24.0).round() / 24.0, k.value.as_f64())).collect();
    assert_eq!(keys, [(0.0, 20.0), (1.5, 100.0), (3.0, 60.0)], "the default is keyed at 1.5 s and the other keys stay");
}

/// A translation too long to end before an effect's fx switch moves the link after the switches:
/// Spanish "Restablecer" would otherwise cover the switch.
#[test]
fn a_long_translation_does_not_cover_the_fx_switch() {
    let (mut h, x) = harness();
    h.state_mut().session.prefs.general.language = "es".into();
    h.run_steps(4);
    let rect = |id: String| {
        let r = h.state().auto.find(&id).unwrap_or_else(|| panic!("missing {id}")).rect;
        egui::Rect::from_min_size(pos2(r[0], r[1]), egui::vec2(r[2], r[3]))
    };
    let (reset, fx) = (rect(format!("timeline.group.{}.reset", x.effect)), rect(format!("timeline.group.{}.fx", x.effect)));
    assert!(!reset.intersects(fx), "Reset {reset:?} overlaps fx {fx:?}");
    click_auto(&mut h, &format!("timeline.group.{}.reset", x.effect));
    assert_eq!(num(&h, x.layer, "effects/#1/blurriness"), 0.0, "the moved link still resets");
}

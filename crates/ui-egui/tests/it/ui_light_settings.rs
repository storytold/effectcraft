//! Light Settings dialog (Layer ▸ New ▸ Light, Layer Settings on a light): every light type the
//! engine has, Environment included, and an existing light's type kept when it's reopened (#1588).

use effectcraft_engine::Session;
use effectcraft_engine::project::{Layer, LayerId, LayerSource, LightKind};
use effectcraft_ui_egui::{Dialog, EffectcraftApp};
use egui_kittest::{Harness, kittest::Queryable as _};
use serde_json::{Value, json};

fn harness() -> Harness<'static, EffectcraftApp> {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Lights", "width": 640, "height": 360, "duration": 2})).unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_eframe(|_| EffectcraftApp::new(s));
    h.step();
    h
}

/// An automation element's rect in the last frame drawn (a dialog settles its place in its
/// first frames, so the frame before can be stale).
fn element(h: &Harness<'_, EffectcraftApp>, id: &str) -> Option<effectcraft_ui_egui::automation::Element> {
    let auto = &h.state().auto;
    auto.elements.iter().chain(auto.previous.iter()).find(|e| e.id == id).cloned()
}

fn click_id(h: &mut Harness<'_, EffectcraftApp>, id: &str) {
    let [x, y, w, hh] = element(h, id).unwrap_or_else(|| panic!("{id} is registered")).rect;
    let p = egui::pos2(x + w / 2.0, y + hh / 2.0);
    h.input_mut().events.push(egui::Event::PointerMoved(p));
    h.step();
    h.input_mut().events.push(egui::Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE });
    h.step();
    h.input_mut().events.push(egui::Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE });
    h.run_steps(3);
}

fn invoke(h: &mut Harness<'_, EffectcraftApp>, id: &str, params: Value) {
    let ctx = h.ctx.clone();
    effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, id, params).unwrap();
    h.run_steps(2);
}

/// Picks the dialog's Light Type `kind` from its menu.
fn choose(h: &mut Harness<'_, EffectcraftApp>, kind: LightKind) {
    click_id(h, "dialog.light.type");
    let i = LightKind::ALL.iter().position(|k| *k == kind).unwrap();
    click_id(h, &format!("dialog.light.typeItem.{i}"));
}

fn ok(h: &mut Harness<'_, EffectcraftApp>) {
    click_id(h, "dialog.light.ok");
    assert_eq!(h.state().dialog, None);
}

fn layer(h: &Harness<'_, EffectcraftApp>, id: u64) -> Layer {
    h.state().session.active_comp().unwrap().layer(LayerId(id)).unwrap().clone()
}

fn kind(h: &Harness<'_, EffectcraftApp>, id: u64) -> LightKind {
    match layer(h, id).source {
        LayerSource::Light { kind } => kind,
        other => panic!("not a light: {other:?}"),
    }
}

fn last_layer(h: &Harness<'_, EffectcraftApp>) -> u64 {
    h.state().session.active_comp().unwrap().layers.iter().map(|l| l.id.0).max().unwrap()
}

/// Opens Light Settings for an existing light, as Layer Settings does.
fn reopen(h: &mut Harness<'_, EffectcraftApp>, id: u64) {
    invoke(h, "layer.settings", json!({"layer": id}));
    assert_eq!(h.state().dialog, Some(Dialog::LightSettings));
}

/// An Environment light lit from a solid, its image turned 30°.
fn environment_light(h: &mut Harness<'_, EffectcraftApp>) -> (u64, u64) {
    let s = &mut h.state_mut().session;
    let sky = s.execute("layer.newSolid", json!({"width": 256, "height": 128, "name": "Sky"})).unwrap()["layer"].as_u64().unwrap();
    let env = s.execute("layer.newLight", json!({"kind": "Environment", "name": "Env", "intensity": 80})).unwrap()["layer"].as_u64().unwrap();
    s.execute("prop.set", json!({"layer": env, "path": "lightOptions/source", "value": sky})).unwrap();
    s.execute("prop.set", json!({"layer": env, "path": "lightOptions/rotation", "value": 30})).unwrap();
    h.run_steps(2);
    (env, sky)
}

#[test]
fn the_light_type_menu_lists_every_light_type() {
    let mut h = harness();
    invoke(&mut h, "layer.newLight", json!({}));
    click_id(&mut h, "dialog.light.type");
    for (i, k) in LightKind::ALL.iter().enumerate() {
        let item = element(&h, &format!("dialog.light.typeItem.{i}")).map(|e| e.label);
        assert_eq!(item.as_deref(), Some(k.label()));
    }
}

#[test]
fn new_light_creates_an_environment_light() {
    let mut h = harness();
    invoke(&mut h, "layer.newLight", json!({}));
    choose(&mut h, LightKind::Environment);
    ok(&mut h);
    let id = last_layer(&h);
    assert_eq!(kind(&h, id), LightKind::Environment);
    let l = layer(&h, id);
    assert!(l.props.prop("lightOptions/source").is_some() && l.props.prop("lightOptions/falloff").is_none());
    // One undoable step, redone as Environment.
    h.state_mut().session.execute("edit.undo", json!({})).unwrap();
    assert!(h.state().session.active_comp().unwrap().layer(LayerId(id)).is_none());
    h.state_mut().session.execute("edit.redo", json!({})).unwrap();
    assert_eq!(kind(&h, id), LightKind::Environment);
}

#[test]
fn reopening_an_environment_light_and_pressing_ok_keeps_it() {
    let mut h = harness();
    let (env, sky) = environment_light(&mut h);
    let before = layer(&h, env);
    reopen(&mut h, env);
    ok(&mut h);
    let after = layer(&h, env);
    assert_eq!(kind(&h, env), LightKind::Environment, "not turned into a Spot light");
    assert_eq!(after.props.prop("lightOptions/source").unwrap().value.as_layer(), Some(sky));
    for path in ["lightOptions/rotation", "lightOptions/intensity"] {
        assert_eq!(after.props.prop(path).unwrap().value, before.props.prop(path).unwrap().value, "{path}");
    }
}

#[test]
fn cancelling_light_settings_changes_nothing() {
    let mut h = harness();
    let (env, _) = environment_light(&mut h);
    let before = serde_json::to_value(layer(&h, env)).unwrap();
    let steps = h.state().session.history.undo.len();
    reopen(&mut h, env);
    choose(&mut h, LightKind::Spot);
    click_id(&mut h, "dialog.light.cancel");
    assert_eq!(h.state().dialog, None);
    assert_eq!(serde_json::to_value(layer(&h, env)).unwrap(), before);
    assert_eq!(h.state().session.history.undo.len(), steps);
}

#[test]
fn changing_a_light_to_environment_undoes_and_redoes() {
    let mut h = harness();
    let id = h.state_mut().session.execute("layer.newLight", json!({"kind": "Spot", "coneAngle": 45})).unwrap()["layer"].as_u64().unwrap();
    h.run_steps(2);
    reopen(&mut h, id);
    choose(&mut h, LightKind::Environment);
    ok(&mut h);
    assert_eq!(kind(&h, id), LightKind::Environment);
    h.state_mut().session.execute("edit.undo", json!({})).unwrap();
    assert_eq!(kind(&h, id), LightKind::Spot);
    assert_eq!(layer(&h, id).props.prop("lightOptions/coneAngle").unwrap().value.as_f64(), 45.0);
    h.state_mut().session.execute("edit.redo", json!({})).unwrap();
    assert_eq!(kind(&h, id), LightKind::Environment);
}

#[test]
fn the_other_light_types_are_created_and_kept() {
    let mut h = harness();
    for k in [LightKind::Parallel, LightKind::Spot, LightKind::Point, LightKind::Ambient] {
        invoke(&mut h, "layer.newLight", json!({}));
        choose(&mut h, k);
        ok(&mut h);
        let id = last_layer(&h);
        assert_eq!(kind(&h, id), k);
        reopen(&mut h, id);
        ok(&mut h);
        assert_eq!(kind(&h, id), k, "{} kept on reopening", k.label());
    }
}

#[test]
fn falloff_and_shadow_options_show_only_for_lights_that_have_them() {
    let mut h = harness();
    invoke(&mut h, "layer.newLight", json!({}));
    for k in LightKind::ALL {
        choose(&mut h, k);
        let shown = !matches!(k, LightKind::Ambient | LightKind::Environment);
        for label in ["Falloff", "Casts Shadows", "Shadow Darkness"] {
            assert_eq!(h.query_by_label(label).is_some(), shown, "{label} for {}", k.label());
        }
    }
}

#[test]
fn changing_an_environment_light_to_spot_undoes_to_its_source_and_rotation() {
    let mut h = harness();
    let (env, sky) = environment_light(&mut h);
    reopen(&mut h, env);
    choose(&mut h, LightKind::Spot);
    ok(&mut h);
    assert_eq!(kind(&h, env), LightKind::Spot);
    assert!(layer(&h, env).props.prop("lightOptions/source").is_none());
    h.state_mut().session.execute("edit.undo", json!({})).unwrap();
    let l = layer(&h, env);
    assert_eq!(kind(&h, env), LightKind::Environment);
    assert_eq!(l.props.prop("lightOptions/source").unwrap().value.as_layer(), Some(sky));
    assert_eq!(l.props.prop("lightOptions/rotation").unwrap().value.as_f64(), 30.0);
}

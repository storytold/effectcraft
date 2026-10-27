//! Picking a layer-valued property with the shared drop-down: the Timeline draws a layer-valued
//! Light Options property (an Environment light's Source) as a drop-down of the comp's layers,
//! Effect Controls' layer parameters (a Displacement Map's layer) pick through the same widget,
//! and both write an undoable `prop.set`.

use effectcraft_engine::Session;
use effectcraft_engine::keyframe::Value;
use effectcraft_engine::project::LayerId;
use effectcraft_ui_egui::EffectcraftApp;
use effectcraft_ui_egui::dock::PanelKind;
use egui::{Pos2, Rect, pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::NodeT;
use egui_kittest::kittest::Queryable;
use serde_json::json;

fn click(h: &mut Harness<'_, EffectcraftApp>, at: Pos2) {
    h.event(egui::Event::PointerMoved(at));
    h.step();
    h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() });
    h.step();
    h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() });
    h.run_steps(3);
}

fn rect_of(h: &Harness<'_, EffectcraftApp>, id: &str) -> Rect {
    let e = h.state().auto.find(id).unwrap_or_else(|| panic!("no automation element {id}")).clone();
    Rect::from_min_size(pos2(e.rect[0], e.rect[1]), vec2(e.rect[2], e.rect[3]))
}

/// Whether `want` is painted inside `row` (the drop-down's current-value text).
fn painted_in(h: &Harness<'_, EffectcraftApp>, row: Rect, want: &str) -> bool {
    h.output().shapes.iter().any(|c| match &c.shape {
        egui::Shape::Text(t) => t.galley.text() == want && row.expand(3.0).contains(t.pos),
        _ => false,
    })
}

/// The current value of a layer-valued property, by uid.
fn layer_value(h: &Harness<'_, EffectcraftApp>, layer: u64, prop: u64) -> Option<u64> {
    let comp = h.state().session.active_comp().unwrap();
    match &comp.layer(LayerId(layer)).unwrap().props.find(prop).unwrap().value {
        Value::Layer(v) => *v,
        other => panic!("property {prop} is not layer-valued: {other:?}"),
    }
}

/// The "N. name" a drop-down shows for a layer.
fn entry_for(h: &Harness<'_, EffectcraftApp>, layer: u64) -> String {
    let comp = h.state().session.active_comp().unwrap();
    let i = comp.layers.iter().position(|l| l.id.0 == layer).unwrap();
    format!("{}. {}", i + 1, comp.layers[i].name)
}

/// Puts the layer-valued `prop` of `layer` back to `value` with a direct `prop.set` (no merge key,
/// so the pick below records its own step instead of folding into a picker write), picks `none`
/// from the drop-down at `value_id`, and checks the clear takes one undo step that restores `value`.
#[track_caller]
fn none_pick_clears(h: &mut Harness<'_, EffectcraftApp>, value_id: &str, none: &str, layer: u64, prop: u64, value: u64) {
    h.state_mut().session.execute("prop.set", json!({"layer": layer, "prop": prop, "value": value})).unwrap();
    h.run_steps(2);
    assert_eq!(layer_value(h, layer, prop), Some(value));
    let steps = h.state().session.history.undo.len();
    let at = rect_of(h, value_id).center();
    click(h, at);
    h.get_by_role_and_label(egui::accesskit::Role::Button, none).click();
    h.run_steps(3);
    assert_eq!(layer_value(h, layer, prop), None, "selecting None clears the value");
    assert_eq!(h.state().session.history.undo.len(), steps + 1, "one clear undo step");
    h.state_mut().session.execute("edit.undo", json!({})).unwrap();
    h.run_steps(2);
    assert_eq!(layer_value(h, layer, prop), Some(value), "undo restores the selected layer");
}

/// A light's Source (a layer-valued Light Options property) picks a comp layer from the
/// Timeline and undo restores None.
#[test]
fn timeline_source_drop_down_picks_the_layer_and_undo_restores_none() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Main", "width": 640, "height": 360, "frameRate": 30, "duration": 4})).unwrap();
    s.execute("comp.settings", json!({"renderer": "advanced3D"})).unwrap();
    let sky = s.execute("layer.newSolid", json!({"name": "Sky HDRI", "color": "#66aaff"})).unwrap()["layer"].as_u64().unwrap();
    let light = s.execute("layer.newLight", json!({"kind": "Environment", "name": "Env Light"})).unwrap()["layer"].as_u64().unwrap();
    let (group, source) = {
        let comp = s.active_comp().unwrap();
        let opts = comp.layer(LayerId(light)).unwrap().props.sub("lightOptions").unwrap();
        (opts.uid, opts.get("source").unwrap().uid)
    };
    let mut app = EffectcraftApp::new(s);
    app.ui.timeline.open_layers.insert(light);
    app.ui.timeline.open_groups.insert(group);
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_eframe(|_| app);
    h.run_steps(4);

    let value_id = format!("timeline.prop.{source}.value");
    // Unset: the row is a drop-down reading "None" — an element to click, not bare text.
    assert_eq!(layer_value(&h, light, source), None);
    assert!(painted_in(&h, rect_of(&h, &value_id), "None"), "the Source row shows a None drop-down");

    // Opening it lists "None" and every layer of the comp, by index and name.
    let at = rect_of(&h, &value_id).center();
    click(&mut h, at);
    let entry = entry_for(&h, sky);
    assert!(h.query_by_role_and_label(egui::accesskit::Role::Button, "None").is_some(), "the list starts with None");
    assert!(h.query_by_role_and_label(egui::accesskit::Role::Button, &entry).is_some(), "the comp's layers are listed ({entry})");

    let steps = h.state().session.history.undo.len();
    h.get_by_role_and_label(egui::accesskit::Role::Button, &entry).click();
    h.run_steps(3);
    assert_eq!(layer_value(&h, light, source), Some(sky), "the Source holds the layer id");
    assert_eq!(h.state().session.history.undo.len(), steps + 1, "one undo step");
    // The row names the layer (index and name), not its internal id.
    assert!(painted_in(&h, rect_of(&h, &value_id), &entry), "the Source row shows the layer's name");

    // Dismissing the list without choosing writes nothing; reopening marks the chosen layer as
    // the current entry (and "None" as not current).
    click(&mut h, pos2(1500.0, 300.0));
    h.run_steps(2);
    assert_eq!(layer_value(&h, light, source), Some(sky), "dismissing the list writes nothing");
    let reopen = rect_of(&h, &value_id).center();
    click(&mut h, reopen);
    assert_eq!(
        h.get_by_role_and_label(egui::accesskit::Role::Button, &entry).accesskit_node().toggled(),
        Some(egui::accesskit::Toggled::True),
        "the current entry is highlighted"
    );
    assert_eq!(
        h.get_by_role_and_label(egui::accesskit::Role::Button, "None").accesskit_node().toggled(),
        Some(egui::accesskit::Toggled::False),
        "None is not the current entry"
    );
    click(&mut h, pos2(1500.0, 300.0));
    h.run_steps(2);

    h.state_mut().session.execute("edit.undo", json!({})).unwrap();
    h.run_steps(2);
    assert_eq!(layer_value(&h, light, source), None, "undo restores None");

    none_pick_clears(&mut h, &value_id, "None", light, source, sky);
}

/// The shared widget is the one Effect Controls already used: a Displacement Map's layer
/// parameter still picks from the drop-down and still undoes.
#[test]
fn effect_controls_layer_param_picks_through_the_shared_widget() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Main", "width": 640, "height": 360, "frameRate": 30, "duration": 4})).unwrap();
    let map = s.execute("layer.newSolid", json!({"name": "Disp", "color": "#ff0000"})).unwrap()["layer"].as_u64().unwrap();
    let base = s.execute("layer.newSolid", json!({"name": "Base", "color": "#20c040"})).unwrap()["layer"].as_u64().unwrap();
    let fx = s.execute("effect.apply", json!({"layers": [base], "effect": "Displacement Map"})).unwrap()["effects"][0].as_u64().unwrap();
    let prop = {
        let comp = s.active_comp().unwrap();
        comp.layer(LayerId(base)).unwrap().effects().unwrap().find_group(fx).unwrap().get("displacementMapLayer").unwrap().uid
    };
    s.state.selected_layers = vec![LayerId(base)];
    let mut app = EffectcraftApp::new(s);
    app.show_panel(PanelKind::EffectControls);
    let mut h = Harness::builder().with_size(vec2(1600.0, 1100.0)).build_eframe(|_| app);
    h.run_steps(4);

    let value_id = format!("effectControls.prop.{prop}.value");
    let row = rect_of(&h, &value_id);
    click(&mut h, row.center());
    let entry = entry_for(&h, map);
    assert!(h.query_by_role_and_label(egui::accesskit::Role::Button, &entry).is_some(), "the comp's layers are listed ({entry})");
    let steps = h.state().session.history.undo.len();
    h.get_by_role_and_label(egui::accesskit::Role::Button, &entry).click();
    h.run_steps(3);
    assert_eq!(layer_value(&h, base, prop), Some(map), "the parameter holds the layer id");
    assert_eq!(h.state().session.history.undo.len(), steps + 1, "one undo step");
    // The row names the picked layer, as the Timeline's Source row does.
    assert!(painted_in(&h, rect_of(&h, &value_id), &entry), "the parameter row shows the layer's name");
    h.state_mut().session.execute("edit.undo", json!({})).unwrap();
    h.run_steps(2);
    assert_eq!(layer_value(&h, base, prop), None, "undo restores None");

    none_pick_clears(&mut h, &value_id, "None", base, prop, map);
}

/// The visual record (the `ui_3d` ignored-snapshot convention): the Source drop-down open over
/// the layer list, and the row after a layer is chosen. Renders with wgpu when `EC_SNAPSHOT_DIR`
/// is set: `EC_SNAPSHOT_DIR=shots cargo test -p effectcraft-ui-egui --test it ui_layer_source_picker:: -- --ignored`.
#[test]
#[ignore = "renders with wgpu; run with --ignored (set EC_SNAPSHOT_DIR to keep PNGs)"]
fn snapshot_source_dropdown_open_and_chosen() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Main", "width": 1280, "height": 720, "frameRate": 30, "duration": 5})).unwrap();
    s.execute("comp.settings", json!({"renderer": "advanced3D"})).unwrap();
    let sky = s.execute("layer.newSolid", json!({"name": "Sky HDRI", "color": "#66aaff"})).unwrap()["layer"].as_u64().unwrap();
    let light = s.execute("layer.newLight", json!({"kind": "Environment", "name": "Env Light"})).unwrap()["layer"].as_u64().unwrap();
    let (group, source) = {
        let comp = s.active_comp().unwrap();
        let opts = comp.layer(LayerId(light)).unwrap().props.sub("lightOptions").unwrap();
        (opts.uid, opts.get("source").unwrap().uid)
    };
    let mut app = EffectcraftApp::new(s);
    app.ui.timeline.open_layers.insert(light);
    app.ui.timeline.open_groups.insert(group);
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).with_pixels_per_point(2.0).wgpu().build_eframe(|_| app);
    h.run_steps(4);
    let save = |h: &mut Harness<'_, EffectcraftApp>, name: &str| {
        if let Ok(dir) = std::env::var("EC_SNAPSHOT_DIR") {
            h.render().expect("render").save(format!("{dir}/{name}")).expect("save");
        }
    };
    save(&mut h, "source_unset.png");
    let at = rect_of(&h, &format!("timeline.prop.{source}.value")).center();
    click(&mut h, at);
    save(&mut h, "source_dropdown_open.png");
    let entry = entry_for(&h, sky);
    h.get_by_role_and_label(egui::accesskit::Role::Button, &entry).click();
    h.run_steps(3);
    assert_eq!(layer_value(&h, light, source), Some(sky));
    save(&mut h, "source_chosen.png");
}

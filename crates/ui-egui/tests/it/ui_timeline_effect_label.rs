//! An effect's colour label (26.5) shows on its Timeline row as well as in Effect Controls.

use effectcraft_engine::Session;
use effectcraft_engine::project::LayerId;
use effectcraft_ui_egui::EffectcraftApp;
use effectcraft_ui_egui::dock::PanelKind;
use egui::vec2;
use egui_kittest::Harness;
use serde_json::json;

fn harness() -> (Harness<'static, EffectcraftApp>, u64, u64) {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Main", "width": 320, "height": 180, "frameRate": 30, "duration": 4})).unwrap();
    let layer = s.execute("layer.newSolid", json!({"name": "A", "color": "#406080"})).unwrap()["layer"].as_u64().unwrap();
    let fx = s.execute("effect.apply", json!({"layers": [layer], "effect": "ec.blur.gaussian"})).unwrap()["effects"][0].as_u64().unwrap();
    s.state.selected_layers = vec![LayerId(layer)];
    s.state.selected_props = vec![(LayerId(layer), fx)];
    let mut app = EffectcraftApp::new(s);
    app.show_panel(PanelKind::Timeline);
    app.ui.timeline.open_layers.insert(layer);
    let effects = app.session.active_comp().and_then(|c| c.layer(LayerId(layer))).and_then(|l| l.props.sub("effects")).map(|g| g.uid);
    app.ui.timeline.open_groups.extend(effects);
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_eframe(|_| app);
    h.run_steps(4);
    (h, layer, fx)
}

/// The label bar appears on the effect's row once the effect has a label, carries the label's
/// name for agents, and goes away when the label is cleared.
#[test]
fn the_effect_row_wears_its_colour_label() {
    let (mut h, layer, fx) = harness();
    let id = format!("timeline.group.{fx}.label");
    assert!(h.state().auto.find(&format!("timeline.group.{fx}.fx")).is_some(), "the effect's row is on screen");
    assert!(h.state().auto.find(&id).is_none(), "an unlabelled effect has no bar");

    h.state_mut().session.execute("effect.setLabel", json!({"layer": layer, "effect": fx, "label": "Blue"})).unwrap();
    h.run_steps(3);
    let blue = h.state().session.prefs.label_name(effectcraft_engine::color::Label::Blue);
    assert_eq!(h.state().auto.find(&id).map(|e| e.label.clone()), Some(blue));

    h.state_mut().session.execute("effect.setLabel", json!({"layer": layer, "effect": fx, "label": 0})).unwrap();
    h.run_steps(3);
    assert!(h.state().auto.find(&id).is_none(), "clearing the label removes the bar");
}

use effectcraft_engine::{
    Session,
    color::Label,
    project::{LayerId, Project},
};
use serde_json::json;

fn setup() -> (Session, u64, u64) {
    let mut s = Session::default();
    s.execute("comp.new", json!({"width": 64, "height": 64, "duration": 1})).unwrap();
    let layer = s.execute("layer.newSolid", json!({"name": "Original"})).unwrap()["layer"].as_u64().unwrap();
    let fx = s.execute("effect.apply", json!({"layers": [layer], "effect": "Gaussian Blur"})).unwrap()["effects"][0].as_u64().unwrap();
    (s, layer, fx)
}
fn label(s: &Session, layer: u64, fx: u64) -> Label {
    s.active_comp().unwrap().layer(LayerId(layer)).unwrap().effects().unwrap().groups().find(|g| g.uid == fx).unwrap().effect_label
}

#[test]
fn effect_label_history_persistence_and_copy_reset_retention() {
    let (mut s, layer, fx) = setup();
    let values = s.active_comp().unwrap().layer(LayerId(layer)).unwrap().props.find_group(fx).unwrap().children.clone();
    s.execute("effect.setLabel", json!({"layer": layer, "effect": fx, "label": 10})).unwrap();
    assert_eq!(label(&s, layer, fx), Label::Purple);
    assert_eq!(s.active_comp().unwrap().layer(LayerId(layer)).unwrap().props.find_group(fx).unwrap().children, values);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(label(&s, layer, fx), Label::None);
    s.execute("edit.redo", json!({})).unwrap();
    assert_eq!(label(&s, layer, fx), Label::Purple);
    let loaded = Project::from_json(&s.project.to_file_json().unwrap()).unwrap();
    assert_eq!(loaded.comp(s.active_comp_id().unwrap()).unwrap().layer(LayerId(layer)).unwrap().props.find_group(fx).unwrap().effect_label, Label::Purple);
    let dupe = s.execute("effect.duplicate", json!({"layer": layer, "effect": fx})).unwrap().as_u64().unwrap();
    assert_ne!(dupe, fx);
    assert_eq!(label(&s, layer, dupe), Label::Purple);
    s.execute("effect.copy", json!({"layer": layer, "effect": fx})).unwrap();
    s.execute("effect.paste", json!({"layers": [layer]})).unwrap();
    assert!(s.active_comp().unwrap().layer(LayerId(layer)).unwrap().effects().unwrap().groups().all(|g| g.effect_label == Label::Purple));
    s.execute("effect.reset", json!({"layer": layer, "effect": fx})).unwrap();
    assert_eq!(label(&s, layer, fx), Label::Purple);
}

#[test]
fn effect_label_invalid_input_is_atomic_and_custom_names_work() {
    let (mut s, layer, fx) = setup();
    let before = s.project.clone();
    let revision = s.revision;
    for value in [json!(-1), json!(17), json!(1.5), json!(u64::MAX), json!(true), json!(null), json!("missing")] {
        assert!(s.execute("effect.setLabel", json!({"layer": layer, "effect": fx, "label": value})).is_err());
        assert_eq!(s.project, before);
        assert_eq!(s.revision, revision);
    }
    for value in [json!(-1), json!(0), json!(1.5), json!(u64::MAX), json!(true), json!(null)] {
        assert!(s.execute("effect.setLabel", json!({"layer": layer, "effect": value, "label": "Red"})).is_err());
        assert_eq!(s.project, before);
        assert_eq!(s.revision, revision);
    }
    s.execute("prefs.set", json!({"key": "labels.0.name", "value": "Priority"})).unwrap();
    s.execute("effect.setLabel", json!({"layer": layer, "effect": fx, "label": "Priority"})).unwrap();
    assert_eq!(label(&s, layer, fx), Label::Red);
    s.execute("effect.setLabel", json!({"layer": layer, "effect": fx, "label": 0})).unwrap();
    assert_eq!(label(&s, layer, fx), Label::None);
}

#[test]
fn effect_label_display_preferences_default_and_roundtrip_independently() {
    let mut s = Session::default();
    let legacy = effectcraft_engine::prefs::Prefs::from_json("{}");
    assert!(legacy.appearance.show_effect_label_swatch);
    assert!(legacy.appearance.use_label_color_for_effect_background);
    assert!(s.prefs.appearance.show_effect_label_swatch);
    assert!(s.prefs.appearance.use_label_color_for_effect_background);
    for key in ["appearance.showEffectLabelSwatch", "appearance.useLabelColorForEffectBackground"] {
        s.execute("prefs.set", json!({"key": key, "value": false})).unwrap();
    }
    let loaded = effectcraft_engine::prefs::Prefs::from_json(&s.prefs.to_json());
    assert!(!loaded.appearance.show_effect_label_swatch);
    assert!(!loaded.appearance.use_label_color_for_effect_background);
}

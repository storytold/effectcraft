use effectcraft_color::Label;
use effectcraft_project::{GroupKind, PropGroup};
use serde_json::json;

#[test]
fn legacy_effect_groups_start_unlabeled_and_roundtrip_labels() {
    let mut group: PropGroup = serde_json::from_value(
        json!({"uid": 9, "match": "ec.blur.gaussian", "name": "Blur", "kind": {"kind": "Effect", "effect": "ec.blur.gaussian"}, "children": []}),
    )
    .unwrap();
    assert_eq!(group.effect_label, Label::None);
    assert_eq!(PropGroup::new(1, "plain", "Plain").effect_label, Label::None);
    assert!(matches!(group.kind, GroupKind::Effect { .. }));
    assert!(serde_json::to_value(&group).unwrap().get("effect_label").is_none());
    group.effect_label = Label::Purple;
    let loaded: PropGroup = serde_json::from_str(&serde_json::to_string(&group).unwrap()).unwrap();
    assert_eq!(loaded, group);
    assert_eq!(loaded.clone().effect_label, Label::Purple);
}

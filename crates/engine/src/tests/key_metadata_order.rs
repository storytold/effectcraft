//! Key attributes preserve the order used by the next stagger gesture.

use std::sync::Arc;

use effectcraft_keyframe::Interp;
use effectcraft_project::LayerId;
use serde_json::json;

use crate::{KeyRef, Session, time::FrameRate};

fn setup() -> (Session, u64) {
    let mut s = Session::default();
    s.execute_checked("comp.new", json!({"width": 64, "height": 64, "frameRate": 30, "duration": 4})).unwrap();
    let layer = s.execute_checked("layer.newSolid", json!({"name": "Box", "color": "#fff"})).unwrap()["layer"].as_u64().unwrap();
    (s, layer)
}

fn add(s: &mut Session, layer: u64, path: &str, time: i64, value: serde_json::Value) {
    s.execute_checked("prop.addKey", json!({"layer": layer, "path": path, "time": time, "value": value})).unwrap();
}

fn uid(s: &Session, layer: u64, path: &str) -> u64 {
    s.active_comp().unwrap().layer(LayerId(layer)).unwrap().props.prop(path).unwrap().uid
}

fn key(layer: u64, prop: u64, frame: i64) -> KeyRef {
    KeyRef { layer: LayerId(layer), prop, time: FrameRate::FPS_30.tick_of(frame) }
}

#[test]
fn easing_preserves_interleaved_layer_and_property_order_with_one_undo() {
    let (mut s, first) = setup();
    let second = s.execute_checked("layer.newSolid", json!({"name": "Other", "color": "#fff"})).unwrap()["layer"].as_u64().unwrap();
    for layer in [first, second] {
        for time in [0, 1] {
            add(&mut s, layer, "transform/opacity", time, json!(time * 25));
        }
    }
    add(&mut s, first, "transform/position", 1, json!([10, 20]));
    let a = uid(&s, first, "transform/opacity");
    let b = uid(&s, second, "transform/opacity");
    let position = uid(&s, first, "transform/position");
    s.execute_checked(
        "keys.select",
        json!({"keys": [
            {"layer": second, "prop": b, "time": 1},
            {"layer": first, "prop": position, "time": 1},
            {"layer": first, "prop": a, "time": 0},
            {"layer": second, "prop": b, "time": 0}
        ]}),
    )
    .unwrap();
    let order = s.state.selected_keys.clone();
    let before = s.project.clone();
    s.history.clear();
    s.execute_checked("keys.easyEase", json!({})).unwrap();
    assert_eq!(s.state.selected_keys, order);
    assert_eq!(s.history.undo.len(), 1);
    for selected in &order {
        let keys = &s.active_comp().unwrap().layer(selected.layer).unwrap().props.find(selected.prop).unwrap().keys;
        assert_eq!(keys.iter().find(|k| k.time == selected.time).unwrap().out_interp, Interp::Bezier);
    }
    s.execute_checked("edit.undo", json!({})).unwrap();
    assert!(Arc::ptr_eq(&s.project, &before));
    assert_eq!(s.state.selected_keys, order);
}

#[test]
fn roving_keeps_the_selected_key_identity_and_arbitrary_order() {
    let (mut s, layer) = setup();
    for (time, x) in [(0, 0), (1, 25), (2, 100)] {
        add(&mut s, layer, "transform/position", time, json!([x, 0]));
    }
    let prop = uid(&s, layer, "transform/position");
    s.execute_checked(
        "keys.select",
        json!({"keys": [
            {"layer": layer, "prop": prop, "time": 1},
            {"layer": layer, "prop": prop, "time": 2},
            {"layer": layer, "prop": prop, "time": 0}
        ]}),
    )
    .unwrap();
    s.execute_checked("keys.interpolation", json!({"roving": true})).unwrap();
    assert_eq!(s.state.selected_keys, vec![key(layer, prop, 15), key(layer, prop, 60), key(layer, prop, 0)]);
    let position = s.active_comp().unwrap().layer(LayerId(layer)).unwrap().props.find(prop).unwrap();
    assert_eq!(position.keys[1].value.as_vec2(), [25.0, 0.0]);
    assert_eq!(position.keys[1].time, FrameRate::FPS_30.tick_of(15));
    assert!(position.keys[1].roving);
    assert!(!position.keys[0].roving && !position.keys[2].roving);
}

#[test]
fn attribute_edit_deduplicates_legacy_refs_and_drops_stale_refs_without_snapping() {
    let (mut s, layer) = setup();
    for time in [0, 1] {
        add(&mut s, layer, "transform/opacity", time, json!(20));
    }
    let prop = uid(&s, layer, "transform/opacity");
    let first = key(layer, prop, 0);
    let second = key(layer, prop, 30);
    s.state.selected_keys = vec![second, key(layer, prop, 10), first, second, key(layer, u64::MAX, 0), first, key(u64::MAX, prop, 0)];
    s.history.clear();
    let result = s.execute_checked("keys.toggleHold", json!({})).unwrap();
    assert_eq!(result, json!(2));
    assert_eq!(s.state.selected_keys, vec![second, first]);
    assert_eq!(s.history.undo.len(), 1);
    let opacity = s.active_comp().unwrap().layer(LayerId(layer)).unwrap().props.find(prop).unwrap();
    assert!(opacity.keys.iter().all(|k| k.out_interp == Interp::Hold));
    s.execute_checked("edit.undo", json!({})).unwrap();
    let opacity = s.active_comp().unwrap().layer(LayerId(layer)).unwrap().props.find(prop).unwrap();
    assert!(opacity.keys.iter().all(|k| k.out_interp == Interp::Linear));
}

#[test]
fn moving_keys_keeps_the_selection_order() {
    let (mut s, first) = setup();
    let second = s.execute_checked("layer.newSolid", json!({"name": "Other", "color": "#fff"})).unwrap()["layer"].as_u64().unwrap();
    for layer in [first, second] {
        add(&mut s, layer, "transform/opacity", 0, json!(0));
    }
    let (a, b) = (uid(&s, first, "transform/opacity"), uid(&s, second, "transform/opacity"));
    s.execute_checked("keys.select", json!({"keys": [{"layer": second, "prop": b, "time": 0}, {"layer": first, "prop": a, "time": 0}]})).unwrap();
    s.execute_checked("keys.move", json!({"delta": 2.0 / 30.0})).unwrap();
    assert_eq!(s.state.selected_keys, vec![key(second, b, 2), key(first, a, 2)], "not sorted by layer");
}

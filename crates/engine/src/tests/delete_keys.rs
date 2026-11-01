//! `keys.delete` reports the keys it removed, not the keys it was asked to remove.

use effectcraft_project::LayerId;
use serde_json::json;

use crate::{KeyRef, Session};

fn setup() -> (Session, u64, u64) {
    let mut s = Session::default();
    s.execute_checked("comp.new", json!({"width": 64, "height": 64, "frameRate": 30, "duration": 4})).unwrap();
    let layer = s.execute_checked("layer.newSolid", json!({"name": "Box", "color": "#fff"})).unwrap()["layer"].as_u64().unwrap();
    for t in 0..3 {
        s.execute_checked("prop.addKey", json!({"layer": layer, "path": "transform/opacity", "time": t, "value": t * 25})).unwrap();
    }
    let prop = s.active_comp().unwrap().layer(LayerId(layer)).unwrap().props.prop("transform/opacity").unwrap().uid;
    (s, layer, prop)
}

fn keys_of(s: &Session, layer: u64, prop: u64) -> Vec<KeyRef> {
    let keys = &s.active_comp().unwrap().layer(LayerId(layer)).unwrap().props.find(prop).unwrap().keys;
    keys.iter().map(|k| KeyRef { layer: LayerId(layer), prop, time: k.time }).collect()
}

#[test]
fn delete_keys_counts_only_the_keys_it_removed() {
    let (mut s, layer, prop) = setup();
    let keys = keys_of(&s, layer, prop);
    assert_eq!(keys.len(), 3);
    // Two real keys, one of them listed twice, and a key of a property that does not exist.
    let missing = KeyRef { layer: LayerId(layer), prop: u64::MAX, time: keys[1].time };
    s.state.selected_keys = vec![keys[0], keys[2], keys[0], missing];
    let removed = s.execute_checked("keys.delete", json!({})).unwrap();
    assert_eq!(removed, json!(2));
    assert_eq!(keys_of(&s, layer, prop), vec![keys[1]]);
}

#[test]
fn delete_keys_reports_zero_when_no_selected_key_exists() {
    let (mut s, layer, prop) = setup();
    let keys = keys_of(&s, layer, prop);
    s.state.selected_keys = vec![KeyRef { layer: LayerId(layer), prop: u64::MAX, time: keys[0].time }];
    let removed = s.execute_checked("keys.delete", json!({})).unwrap();
    assert_eq!(removed, json!(0));
    assert_eq!(keys_of(&s, layer, prop), keys);
}

/// A layer locked after its keys were selected keeps them: Delete and Edit Value leave them alone.
#[test]
fn a_locked_layers_keys_cannot_be_deleted_or_edited() {
    let (mut s, layer, prop) = setup();
    let keys = keys_of(&s, layer, prop);
    s.state.selected_keys = keys.clone();
    s.execute_checked("layer.setSwitch", json!({"layers": [layer], "switch": "locked", "value": true})).unwrap();
    let removed = s.execute_checked("keys.delete", json!({})).unwrap();
    assert_eq!(removed, json!(0));
    assert_eq!(keys_of(&s, layer, prop), keys);
    assert_eq!(s.state.selected_keys, keys, "the selection stays");
    let e = s.execute("keys.set", json!({"layer": layer, "prop": prop, "time": 0, "value": 90})).unwrap_err().to_string();
    assert!(e.contains("locked"), "{e}");
    let value = s.active_comp().unwrap().layer(LayerId(layer)).unwrap().props.find(prop).unwrap().keys[0].value.as_f64();
    assert_eq!(value, 0.0);
    // Unlocked again, both work.
    s.execute_checked("layer.setSwitch", json!({"layers": [layer], "switch": "locked", "value": false})).unwrap();
    s.execute_checked("keys.set", json!({"layer": layer, "prop": prop, "time": 0, "value": 90})).unwrap();
    assert_eq!(s.execute_checked("keys.delete", json!({})).unwrap(), json!(3));
}

/// Easy Ease, Toggle Hold and Time-Reverse leave a locked layer's keys as they are and keep them
/// selected; unlocked, they change them.
#[test]
fn selection_based_key_edits_skip_a_locked_layer() {
    let (mut s, layer, prop) = setup();
    let keys = keys_of(&s, layer, prop);
    let stored = |s: &Session| s.active_comp().unwrap().layer(LayerId(layer)).unwrap().props.find(prop).unwrap().keys.clone();
    let before = stored(&s);
    s.state.selected_keys = keys.clone();
    s.execute_checked("layer.setSwitch", json!({"layers": [layer], "switch": "locked", "value": true})).unwrap();
    for command in ["keys.easyEase", "keys.toggleHold", "keys.timeReverse"] {
        s.execute_checked(command, json!({})).unwrap();
        assert_eq!(stored(&s), before, "{command} changed a locked layer's keys");
        assert_eq!(s.state.selected_keys, keys, "{command} dropped the selection of a locked layer's keys");
    }
    s.execute_checked("layer.setSwitch", json!({"layers": [layer], "switch": "locked", "value": false})).unwrap();
    s.execute_checked("keys.easyEase", json!({})).unwrap();
    assert_ne!(stored(&s), before, "unlocked, Easy Ease changes the keys");
}

#[test]
fn spatial_handles_cannot_edit_a_locked_layer() {
    let (mut s, layer, _) = setup();
    for (time, value) in [(0, [10, 20, 0]), (1, [80, 60, 0])] {
        s.execute("prop.addKey", json!({"layer": layer, "path": "transform/position", "time": time, "value": value})).unwrap();
    }
    let prop = s.active_comp().unwrap().layer(LayerId(layer)).unwrap().props.prop("transform/position").unwrap().uid;
    s.execute("keys.select", json!({"keys": [{"layer": layer, "prop": prop, "time": 0}]})).unwrap();
    s.execute("layer.setSwitch", json!({"layers": [layer], "switch": "locked", "value": true})).unwrap();
    let before = s.project.to_json();
    let selected = s.state.selected_keys.clone();
    let undo_len = s.history.undo.len();
    let params = json!({"layer": layer, "prop": prop, "time": 0, "out": [15, 25, 0]});
    let error = s.execute("keys.setSpatialTangents", params.clone()).unwrap_err().to_string();
    assert!(error.contains("locked"), "{error}");
    assert_eq!(s.project.to_json(), before);
    assert_eq!(s.state.selected_keys, selected);
    assert_eq!(s.history.undo.len(), undo_len);
    s.execute("layer.setSwitch", json!({"layers": [layer], "switch": "locked", "value": false})).unwrap();
    let unlocked = s.project.to_json();
    s.execute("keys.setSpatialTangents", params).unwrap();
    let edited = s.project.to_json();
    assert_ne!(edited, unlocked);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.project.to_json(), unlocked);
    s.execute("edit.redo", json!({})).unwrap();
    assert_eq!(s.project.to_json(), edited);
}

#[test]
fn pasting_keys_or_motion_paths_cannot_edit_locked_targets() {
    for path_clip in [false, true] {
        for explicit in [false, true] {
            let (mut s, layer, opacity) = setup();
            let target_path = if path_clip { "transform/position" } else { "transform/opacity" };
            let source = if path_clip {
                s.execute("mask.new", json!({"layer": layer, "vertices": [[0, 0], [20, 10], [40, 0]], "closed": false})).unwrap();
                s.execute("prop.toggleAnimation", json!({"layer": layer, "path": "masks/#1/path"})).unwrap();
                s.active_comp().unwrap().layer(LayerId(layer)).unwrap().props.prop("masks/#1/path").unwrap().uid
            } else {
                opacity
            };
            s.execute("keys.select", json!({"keys": [{"layer": layer, "prop": source, "time": 0}]})).unwrap();
            s.execute("keys.copy", json!({})).unwrap();
            s.execute("prop.select", json!({"layer": layer, "path": target_path})).unwrap();
            s.execute("time.set", json!({"time": 3})).unwrap();
            s.execute("layer.setSwitch", json!({"layers": [layer], "switch": "locked", "value": true})).unwrap();
            let params = if explicit { json!({"layer": layer, "path": target_path}) } else { json!({}) };
            let before = s.project.to_json();
            let selected = s.state.selected_keys.clone();
            let undo_len = s.history.undo.len();
            let result = s.execute("keys.paste", params.clone());
            assert!(result.is_err(), "path={path_clip}, explicit={explicit}: {result:?}");
            assert_eq!(s.project.to_json(), before);
            assert_eq!(s.state.selected_keys, selected);
            assert_eq!(s.history.undo.len(), undo_len);
            s.execute("layer.setSwitch", json!({"layers": [layer], "switch": "locked", "value": false})).unwrap();
            let unlocked = s.project.to_json();
            s.execute("keys.paste", params).unwrap();
            let pasted = s.project.to_json();
            assert_ne!(pasted, unlocked);
            s.execute("edit.undo", json!({})).unwrap();
            assert_eq!(s.project.to_json(), unlocked);
            s.execute("edit.redo", json!({})).unwrap();
            assert_eq!(s.project.to_json(), pasted);
        }
    }
}

#[test]
fn key_paste_skips_locked_targets_without_changing_layer_pairing() {
    let (mut s, locked, locked_prop) = setup();
    let other = s.execute("layer.newSolid", json!({"name": "Other", "color": "#fff"})).unwrap()["layer"].as_u64().unwrap();
    for (time, value) in [(0, 100), (1, 80), (2, 60)] {
        s.execute("prop.addKey", json!({"layer": other, "path": "transform/opacity", "time": time, "value": value})).unwrap();
    }
    let other_prop = s.active_comp().unwrap().layer(LayerId(other)).unwrap().props.prop("transform/opacity").unwrap().uid;
    let locked_keys = keys_of(&s, locked, locked_prop);
    s.state.selected_keys = locked_keys.iter().chain(keys_of(&s, other, other_prop).iter()).copied().collect();
    s.execute("keys.copy", json!({})).unwrap();
    s.execute("layer.setSwitch", json!({"layers": [locked], "switch": "locked", "value": true})).unwrap();
    s.execute("time.set", json!({"time": 3})).unwrap();
    let before = s.project.to_json();
    assert_eq!(s.execute("keys.paste", json!({"layers": [locked, other]})).unwrap(), json!(3), "only newly pasted keys are counted");
    assert_eq!(keys_of(&s, locked, locked_prop), locked_keys);
    assert!(locked_keys.iter().all(|k| s.state.selected_keys.contains(k)));
    assert_eq!(s.state.selected_keys.len(), 6);
    let keys = &s.active_comp().unwrap().layer(LayerId(other)).unwrap().props.find(other_prop).unwrap().keys;
    assert_eq!(keys.iter().filter(|k| k.time.seconds() >= 3.0).map(|k| k.value.as_f64()).collect::<Vec<_>>(), [100.0, 80.0, 60.0]);
    let pasted = s.project.to_json();
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.project.to_json(), before);
    s.execute("edit.redo", json!({})).unwrap();
    assert_eq!(s.project.to_json(), pasted);
}

#[test]
fn motion_path_paste_preserves_locked_source_selection() {
    for explicit in [false, true] {
        let (mut s, source, _) = setup();
        s.execute("mask.new", json!({"layer": source, "vertices": [[0, 0], [20, 10], [40, 0]], "closed": false})).unwrap();
        s.execute("prop.toggleAnimation", json!({"layer": source, "path": "masks/#1/path"})).unwrap();
        let uid = s.active_comp().unwrap().layer(LayerId(source)).unwrap().props.prop("masks/#1/path").unwrap().uid;
        s.execute("keys.select", json!({"keys": [{"layer": source, "prop": uid, "time": 0}]})).unwrap();
        s.execute("keys.copy", json!({})).unwrap();
        let locked_keys = s.state.selected_keys.clone();
        let target = s.execute("layer.newSolid", json!({"name": "Target", "color": "#fff"})).unwrap()["layer"].as_u64().unwrap();
        s.execute("prop.select", json!({"layer": target, "path": "transform/position"})).unwrap();
        s.execute("layer.setSwitch", json!({"layers": [source], "switch": "locked", "value": true})).unwrap();
        s.state.selected_keys = locked_keys.clone();
        let params = if explicit { json!({"layer": target, "path": "transform/position"}) } else { json!({}) };
        let before = s.project.to_json();
        assert_eq!(s.execute("keys.paste", params).unwrap(), json!(3));
        assert!(locked_keys.iter().all(|k| s.state.selected_keys.contains(k)));
        assert_eq!(s.state.selected_keys.len(), 4);
        assert_eq!(s.state.selected_keys.iter().filter(|k| k.layer == LayerId(target)).count(), 3);
        let pasted = s.project.to_json();
        s.execute("edit.undo", json!({})).unwrap();
        assert_eq!(s.project.to_json(), before);
        s.execute("edit.redo", json!({})).unwrap();
        assert_eq!(s.project.to_json(), pasted);
    }
}

#[test]
fn key_transform_preserves_locked_keys_in_a_mixed_selection() {
    let (mut s, locked, locked_prop) = setup();
    let unlocked = s.execute("layer.newSolid", json!({"name": "Unlocked", "color": "#fff"})).unwrap()["layer"].as_u64().unwrap();
    for t in 0..3 {
        s.execute("prop.addKey", json!({"layer": unlocked, "path": "transform/opacity", "time": t, "value": t * 25})).unwrap();
    }
    let unlocked_prop = s.active_comp().unwrap().layer(LayerId(unlocked)).unwrap().props.prop("transform/opacity").unwrap().uid;
    let locked_keys = keys_of(&s, locked, locked_prop);
    s.state.selected_keys = locked_keys.iter().chain(keys_of(&s, unlocked, unlocked_prop).iter()).copied().collect();
    s.execute("layer.setSwitch", json!({"layers": [locked], "switch": "locked", "value": true})).unwrap();
    let stored = |s: &Session, layer, prop| s.active_comp().unwrap().layer(LayerId(layer)).unwrap().props.find(prop).unwrap().keys.clone();
    let before = stored(&s, locked, locked_prop);
    let undo = s.history.undo.len();
    for offset in [0.5, 1.0] {
        s.execute("keys.transform", json!({"timeOffset": offset, "valueOffset": offset * 10.0, "merge": "mixed-transform", "fromStart": true})).unwrap();
        assert_eq!(stored(&s, locked, locked_prop), before);
        assert_eq!(s.state.selected_keys.len(), 6);
        assert!(locked_keys.iter().all(|k| s.state.selected_keys.contains(k)), "locked selections stay at their original times");
        assert!(keys_of(&s, unlocked, unlocked_prop).iter().all(|k| s.state.selected_keys.contains(k)), "unlocked selections follow the keys");
        let moved = stored(&s, unlocked, unlocked_prop);
        for (i, key) in moved.iter().enumerate() {
            assert_eq!(key.time.seconds(), i as f64 + offset);
            assert_eq!(key.value.as_f64(), i as f64 * 25.0 + offset * 10.0);
        }
    }
    assert_eq!(s.history.undo.len(), undo + 1, "one merged drag");
    let after = stored(&s, unlocked, unlocked_prop);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(stored(&s, locked, locked_prop), before);
    assert_eq!(stored(&s, unlocked, unlocked_prop), before);
    s.execute("edit.redo", json!({})).unwrap();
    assert_eq!(stored(&s, locked, locked_prop), before);
    assert_eq!(stored(&s, unlocked, unlocked_prop), after);
    s.execute("layer.setSwitch", json!({"layers": [locked], "switch": "locked", "value": false})).unwrap();
    s.execute("keys.transform", json!({"timeOffset": 0.5, "valueOffset": 10})).unwrap();
    assert_ne!(stored(&s, locked, locked_prop), before, "unlocking makes the keys editable again");
}

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

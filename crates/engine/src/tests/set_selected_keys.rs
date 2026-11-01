//! `prop.set` with the CTI on one of several selected keys moves all the selected keys by the same change.

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

fn values(s: &Session, layer: u64, prop: u64) -> Vec<f64> {
    s.active_comp().unwrap().layer(LayerId(layer)).unwrap().props.find(prop).unwrap().keys.iter().map(|k| k.value.as_f64()).collect()
}

fn select_all(s: &mut Session, layer: u64, prop: u64) {
    let keys = s.active_comp().unwrap().layer(LayerId(layer)).unwrap().props.find(prop).unwrap().keys.iter().map(|k| k.time).collect::<Vec<_>>();
    s.state.selected_keys = keys.into_iter().map(|time| KeyRef { layer: LayerId(layer), prop, time }).collect();
}

#[test]
fn value_edit_on_a_selected_key_offsets_all_selected_keys() {
    let (mut s, layer, prop) = setup();
    select_all(&mut s, layer, prop);
    s.execute_checked("time.set", json!({"time": 1.0})).unwrap();
    s.execute_checked("prop.set", json!({"layer": layer, "path": "transform/opacity", "value": 40})).unwrap();
    assert_eq!(values(&s, layer, prop), vec![15.0, 40.0, 65.0]);
}

#[test]
fn offset_selected_keys_stay_in_the_slider_range() {
    let (mut s, layer, prop) = setup();
    select_all(&mut s, layer, prop);
    s.execute_checked("time.set", json!({"time": 1.0})).unwrap();
    s.execute_checked("prop.set", json!({"layer": layer, "path": "transform/opacity", "value": 90})).unwrap();
    assert_eq!(values(&s, layer, prop), vec![65.0, 90.0, 100.0]);
}

#[test]
fn value_edit_between_keys_only_adds_a_key() {
    let (mut s, layer, prop) = setup();
    select_all(&mut s, layer, prop);
    s.execute_checked("time.set", json!({"time": 0.5})).unwrap();
    s.execute_checked("prop.set", json!({"layer": layer, "path": "transform/opacity", "value": 40})).unwrap();
    assert_eq!(values(&s, layer, prop), vec![0.0, 40.0, 25.0, 50.0]);
}

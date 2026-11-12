//! An edit copies only the layer it changes: the others stay shared with the undo state, so an
//! edit's cost doesn't grow with the size of the comp (scripts setting many keys went quadratic).

use std::sync::Arc;

use serde_json::json;

use crate::Session;

#[test]
fn a_key_on_one_layer_leaves_the_other_layers_shared_with_undo() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Big", "duration": 10, "frameRate": 30})).unwrap();
    let ids: Vec<u64> = (0..20).map(|i| s.execute("layer.newNull", json!({"name": format!("N{i}")})).unwrap()["layer"].as_u64().unwrap()).collect();
    let cid = s.active_comp_id().unwrap();
    let before = s.project.clone();
    s.execute("prop.addKey", json!({"layer": ids[7], "path": "transform/position", "time": 1.0, "value": [5, 5]})).unwrap();
    let (old, new) = (before.comp(cid).unwrap(), s.project.comp(cid).unwrap());
    assert_eq!(old.layers.len(), new.layers.len());
    let edited = new.layers.iter().position(|l| l.id.0 == ids[7]).unwrap();
    for i in 0..new.layers.len() {
        assert_eq!(new.layers.same_at(&old.layers, i), i != edited, "layer {i} shared: {}", new.layers.same_at(&old.layers, i));
    }
    // Undo brings back the very same state, and the edited layer is the only one that differed.
    s.execute("edit.undo", json!({})).unwrap();
    assert!(Arc::ptr_eq(&s.project, &before));
}

#[test]
fn layers_save_and_load_as_a_plain_list() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Round trip", "duration": 5, "frameRate": 25})).unwrap();
    for i in 0..3 {
        s.execute("layer.newNull", json!({"name": format!("N{i}")})).unwrap();
    }
    let cid = s.active_comp_id().unwrap();
    let comp = s.project.comp(cid).unwrap();
    let text = serde_json::to_value(&comp.layers).unwrap();
    assert_eq!(text.as_array().map(Vec::len), Some(3), "a JSON array of layers, as before");
    let back: effectcraft_project::Layers = serde_json::from_value(text).unwrap();
    assert_eq!(&back, &comp.layers);
}

//! Agent keyframe times on offset, stretched and reversed layers (#257).

use effectcraft_project::LayerId;
use effectcraft_time::Tick;
use serde_json::{Value, json};

use crate::{Session, tests_timeline::prop};

fn offset_layer(start: f64, stretch: f64) -> (Session, u64) {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Offset", "duration": 20, "frameRate": 30})).unwrap();
    let layer = s.execute("layer.newSolid", json!({"name": "A", "color": "#ffffff"})).unwrap()["layer"].as_u64().unwrap();
    s.execute("layer.timing", json!({"layer": layer, "start": start, "in": 10.5, "out": 20})).unwrap();
    let cid = s.active_comp_id().unwrap();
    std::sync::Arc::make_mut(&mut s.project).comp_mut(cid).unwrap().layer_mut(LayerId(layer)).unwrap().stretch = stretch;
    (s, layer)
}

fn assert_seconds(value: &Value, seconds: f64) {
    assert!((value.as_f64().unwrap() - seconds).abs() < 1e-8, "{value} != {seconds}");
}

#[test]
fn comp_key_times_on_an_offset_layer_read_at_the_intended_time_and_undo() {
    let (mut s, layer) = offset_layer(10.5, 100.0);
    for (time, value) in [(15.3, 0), (16.0, 100)] {
        let reply = s.execute("prop.addKey", json!({"layer": layer, "path": "transform/opacity", "time": time, "timeBase": "comp", "value": value})).unwrap();
        assert_seconds(&reply["compTime"], time);
        assert_seconds(&reply["layerTime"], time - 10.5);
        assert!(reply.get("warning").is_none());
    }
    let read = s.execute("prop.get", json!({"layer": layer, "path": "transform/opacity", "time": 15.65})).unwrap();
    assert_seconds(&read["value"], 50.0);
    assert_seconds(&read["keys"][0]["compTime"], 15.3);
    let reply = s.execute("prop.set", json!({"layer": layer, "path": "transform/opacity", "time": 15.5, "timeBase": "comp", "value": 45})).unwrap();
    assert_seconds(&reply["compTime"], 15.5);
    assert_eq!(prop(&s, layer, "transform/opacity").keys.len(), 3);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(prop(&s, layer, "transform/opacity").keys.len(), 2);
    s.execute("edit.redo", json!({})).unwrap();
    assert_eq!(prop(&s, layer, "transform/opacity").keys.len(), 3);
}

#[test]
fn default_layer_times_are_preserved_and_outside_keys_warn() {
    let (mut s, layer) = offset_layer(10.5, 100.0);
    let legacy = s.execute("prop.addKey", json!({"layer": layer, "path": "transform/opacity", "time": 4.8, "value": 0})).unwrap();
    assert_eq!(legacy, json!(1), "ordinary legacy replies stay scalar");
    let outside = s.execute("prop.addKey", json!({"layer": layer, "path": "transform/opacity", "time": 15.3, "value": 100})).unwrap();
    assert_eq!(outside, json!(2), "legacy replies keep their raw count outside the active range");
    let read = s.execute("prop.get", json!({"layer": layer, "path": "transform/opacity"})).unwrap();
    assert_seconds(&read["keys"][1]["time"], 15.3);
    assert_seconds(&read["keys"][1]["compTime"], 25.8);
    assert!(read["keys"][1].get("warning").is_some());
    let selected = s.execute("keys.select", json!({"keys": [{"layer": layer, "path": "transform/opacity", "time": 25.8}], "timeBase": "comp"})).unwrap();
    assert_seconds(&selected["keys"][0]["compTime"], 25.8);
    assert!(selected.get("warning").is_some());
    let moved = s.execute("keys.set", json!({"layer": layer, "path": "transform/opacity", "time": 15.3, "newTime": 9.5, "timeBase": "layer"})).unwrap();
    assert_seconds(&moved["compTime"], 20.0);
    assert!(moved.get("warning").is_none(), "a key on the out point is accepted");
    let past = s.execute("prop.addKey", json!({"layer": layer, "path": "transform/rotation", "time": 20.1, "timeBase": "comp", "value": 45})).unwrap();
    assert_seconds(&past["compTime"], 20.1);
    assert!(past.get("warning").is_some(), "a key past the out point warns");
}

#[test]
fn stretched_and_reversed_keys_use_the_comp_frame_grid_for_both_bases() {
    for (start, stretch) in [(10.5, 200.0), (18.0, -200.0)] {
        let (mut s, layer) = offset_layer(start, stretch);
        for time in [15.3, 16.0] {
            let layer_time = (time - start) * 100.0 / stretch;
            let comp_reply =
                s.execute("prop.addKey", json!({"layer": layer, "path": "transform/opacity", "time": time, "timeBase": "comp", "value": 20})).unwrap();
            assert_seconds(&comp_reply["layerTime"], layer_time);
            let layer_reply =
                s.execute("prop.addKey", json!({"layer": layer, "path": "transform/rotation", "time": layer_time, "timeBase": "layer", "value": 20})).unwrap();
            assert_seconds(&layer_reply["compTime"], time);
        }
        let selected = s.execute("keys.select", json!({"keys": [{"layer": layer, "path": "transform/opacity", "time": 15.3}], "timeBase": "comp"})).unwrap();
        assert_seconds(&selected["keys"][0]["compTime"], 15.3);
        let moved = s
            .execute("keys.set", json!({"layer": layer, "path": "transform/opacity", "time": 15.3, "newTime": 15.41, "timeBase": "comp", "value": 60}))
            .unwrap();
        assert_seconds(&moved["compTime"], 15.4);
        let ly = s.active_comp().unwrap().layer(LayerId(layer)).unwrap();
        assert_eq!(ly.comp_time(s.state.selected_keys[0].time), Tick::from_seconds_f64(15.4));
        let layer_reply = s.execute("keys.set", json!({"layer": layer, "path": "transform/rotation", "time": (15.3 - start) * 100.0 / stretch, "newTime": (15.41 - start) * 100.0 / stretch, "timeBase": "layer"})).unwrap();
        assert_seconds(&layer_reply["compTime"], 15.4);
        s.execute("edit.undo", json!({})).unwrap();
        s.execute("edit.undo", json!({})).unwrap();
        let ly = s.active_comp().unwrap().layer(LayerId(layer)).unwrap();
        let restored = prop(&s, layer, "transform/opacity");
        assert!(restored.keys.iter().any(|k| ly.comp_time(k.time) == Tick::from_seconds_f64(15.3)));
    }
}

#[test]
fn invalid_key_time_arguments_leave_properties_and_history_unchanged() {
    let (mut s, layer) = offset_layer(10.5, 100.0);
    let before = prop(&s, layer, "transform/opacity");
    let history_len = s.history.undo.len();
    for cmd in ["prop.addKey", "prop.set", "keys.set"] {
        for extra in [json!({"time": "oops"}), json!({"time": 1e300}), json!({"timeBase": "seconds", "time": 0})] {
            let mut p = json!({"layer": layer, "path": "transform/opacity", "value": 10});
            p.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
            assert!(s.execute(cmd, p).is_err(), "{cmd}: {extra}");
            assert_eq!(prop(&s, layer, "transform/opacity"), before);
            assert_eq!(s.history.undo.len(), history_len);
        }
    }
    assert!(s.execute("keys.select", json!({"keys": [], "timeBase": null})).is_err());
}

#[test]
fn omitted_key_times_and_reads_use_the_target_comps_cti() {
    let (mut s, layer) = offset_layer(10.5, 100.0);
    let target = s.active_comp_id().unwrap().0;
    s.execute("time.set", json!({"time": 15.3})).unwrap();
    s.execute("comp.new", json!({"name": "Other", "duration": 20})).unwrap();
    s.execute("time.set", json!({"time": 1})).unwrap();
    let reply = s.execute("prop.addKey", json!({"comp": target, "layer": layer, "path": "transform/opacity", "timeBase": "comp", "value": 25})).unwrap();
    assert_seconds(&reply["compTime"], 15.3);
    let read = s.execute("prop.get", json!({"comp": target, "layer": layer, "path": "transform/opacity"})).unwrap();
    assert_seconds(&read["time"], 15.3);
    assert_seconds(&read["value"], 25.0);
}

#[test]
fn excessive_stretch_or_start_conversions_fail_without_clamping_keys() {
    for (start, stretch, base, time) in [(10.5, 1e300, "layer", 1.0), (10.5, 1e-300, "comp", 15.3), (Tick::MAX.seconds(), 100.0, "layer", 1.0)] {
        let (mut s, layer) = offset_layer(10.5, 100.0);
        let cid = s.active_comp_id().unwrap();
        let ly = std::sync::Arc::make_mut(&mut s.project).comp_mut(cid).unwrap().layer_mut(LayerId(layer)).unwrap();
        ly.start_time = Tick::from_seconds_f64(start);
        ly.stretch = stretch;
        let before = s.history.undo.len();
        let result = s.execute("prop.addKey", json!({"layer": layer, "path": "transform/opacity", "time": time, "timeBase": base, "value": 10}));
        assert!(result.is_err(), "start {start}, stretch {stretch}: {result:?}");
        assert_eq!(s.history.undo.len(), before);
        assert!(prop(&s, layer, "transform/opacity").keys.is_empty());
    }
}

#[test]
fn explicit_base_on_static_property_reports_timing_without_enabling_animation() {
    let (mut s, layer) = offset_layer(10.5, 100.0);
    let reply = s.execute("prop.set", json!({"layer": layer, "path": "transform/opacity", "time": 15.3, "timeBase": "comp", "value": 25})).unwrap();
    assert_seconds(&reply["compTime"], 15.3);
    assert_eq!(reply["animated"], false);
    assert_eq!(reply["value"], 25.0);
    assert!(prop(&s, layer, "transform/opacity").keys.is_empty());
}

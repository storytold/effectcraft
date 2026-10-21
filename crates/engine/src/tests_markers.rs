//! Composition / layer markers: dialog fields, move, duration, delete, convert, update from
//! source, all with undo.

use serde_json::json;

use crate::tests_timeline::setup;

#[test]
fn comp_marker_dialog_fields_and_undo() {
    let (mut s, _) = setup();
    s.execute("time.set", json!({"time": 1.0})).unwrap();
    s.execute("comp.addMarker", json!({"comment": "a"})).unwrap();
    let r = s
        .execute(
            "markers.set",
            json!({"index": 0, "time": 1.51, "duration": 0.5, "comment": "Intro", "chapter": "One", "url": "https://getartcraft.com", "frameTarget": "_blank",
                   "cuePoint": {"name": "cue", "navigation": true, "params": [["k", "v"]]}, "protected": true, "label": "Aqua"}),
        )
        .unwrap();
    assert_eq!(r["index"], 0);
    let l = s.execute("markers.list", json!({})).unwrap();
    let m = &l[0];
    // Snapped to whole frames (30 fps).
    assert!((m["time"].as_f64().unwrap() - 1.5).abs() < 1e-9, "{m}");
    assert!((m["duration"].as_f64().unwrap() - 0.5).abs() < 1e-9);
    assert_eq!(m["comment"], "Intro");
    assert_eq!(m["chapter"], "One");
    assert_eq!(m["url"], "https://getartcraft.com");
    assert_eq!(m["frameTarget"], "_blank");
    assert_eq!(m["cuePoint"]["name"], "cue");
    assert_eq!(m["cuePoint"]["navigation"], true);
    assert_eq!(m["cuePoint"]["params"][0][1], "v");
    assert_eq!(m["protected"], true);
    assert_eq!(m["label"], "Aqua");
    s.execute("edit.undo", json!({})).unwrap();
    let l = s.execute("markers.list", json!({})).unwrap();
    assert_eq!(l[0]["comment"], "a");
    assert_eq!(l[0]["time"], 1.0);
    s.execute("edit.redo", json!({})).unwrap();
    // Find by time, clear the cue point, delete with undo.
    s.execute("markers.set", json!({"at": 1.5, "cuePoint": null})).unwrap();
    assert!(s.execute("markers.list", json!({})).unwrap()[0]["cuePoint"].is_null());
    s.execute("markers.delete", json!({"at": 1.5})).unwrap();
    assert_eq!(s.execute("markers.list", json!({})).unwrap().as_array().unwrap().len(), 0);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.execute("markers.list", json!({})).unwrap().as_array().unwrap().len(), 1);
    assert!(s.execute("markers.set", json!({"index": 5, "comment": "x"})).is_err());
}

#[test]
fn drag_merges_into_one_undo_step_and_markers_stay_sorted() {
    let (mut s, _) = setup();
    for t in [0.5, 2.0] {
        s.execute("markers.set", json!({"new": true, "time": t})).unwrap();
    }
    let before = s.history.undo.len();
    // A drag: several moves merged under one key; passing the other marker re-sorts.
    let mut idx = 0;
    for t in [1.0, 1.5, 2.5] {
        idx = s.execute("markers.set", json!({"index": idx, "time": t, "merge": "mk-drag"})).unwrap()["index"].as_u64().unwrap();
    }
    assert_eq!(idx, 1);
    assert_eq!(s.history.undo.len(), before + 1);
    let l = s.execute("markers.list", json!({})).unwrap();
    assert_eq!(l[0]["time"], 2.0);
    assert_eq!(l[1]["time"], 2.5);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.execute("markers.list", json!({})).unwrap()[0]["time"], 0.5);
}

#[test]
fn layer_markers_convert_and_update_from_source() {
    let (mut s, l) = setup();
    // Layer starts at 1 s: layer markers are stored in layer time but addressed in comp time.
    s.execute("layer.timing", json!({"layers": [l], "start": 1.0})).unwrap();
    s.execute("markers.set", json!({"layer": l, "new": true, "time": 2.0, "comment": "hit"})).unwrap();
    let c = s.active_comp().unwrap();
    let lay = c.layer(effectcraft_project::LayerId(l)).unwrap();
    assert_eq!(lay.markers[0].time.seconds(), 1.0);
    assert_eq!(s.execute("markers.list", json!({"layer": l})).unwrap()[0]["time"], 2.0);
    // Layer → comp marker keeps the comp time.
    s.execute("markers.convert", json!({"layer": l, "index": 0})).unwrap();
    assert_eq!(s.execute("markers.list", json!({})).unwrap()[0]["time"], 2.0);
    assert_eq!(s.execute("markers.list", json!({"layer": l})).unwrap().as_array().unwrap().len(), 0);
    // And back.
    s.execute("markers.convert", json!({"index": 0, "toLayer": l})).unwrap();
    assert_eq!(s.execute("markers.list", json!({"layer": l})).unwrap()[0]["comment"], "hit");
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.execute("markers.list", json!({})).unwrap().as_array().unwrap().len(), 1);
    // Locked markers refuse edits.
    s.execute("layer.markersLock", json!({"layers": [l], "value": true})).unwrap();
    assert!(s.execute("markers.set", json!({"layer": l, "new": true})).is_err());
    s.execute("layer.markersLock", json!({"layers": [l], "value": false})).unwrap();

    // Update Markers From Source on a precomp layer.
    let r = s.execute("layer.precompose", json!({"layers": [l], "name": "Inner"})).unwrap();
    let inner = r["comp"].as_u64().unwrap();
    let outer_layer = r["layer"].as_u64().unwrap();
    s.execute("markers.set", json!({"comp": inner, "new": true, "time": 0.5, "comment": "src"})).unwrap();
    s.execute("layer.select", json!({"layers": [outer_layer]})).unwrap();
    let r = s.execute("layer.updateMarkersFromSource", json!({})).unwrap();
    assert_eq!(r["added"], 1);
    let lm = s.execute("markers.list", json!({"layer": outer_layer})).unwrap();
    assert!(lm.as_array().unwrap().iter().any(|m| m["comment"] == "src"));
    // Idempotent; undoable.
    assert_eq!(s.execute("layer.updateMarkersFromSource", json!({})).unwrap()["added"], 0);
    s.execute("edit.undo", json!({})).unwrap();
    s.execute("edit.undo", json!({})).unwrap();
    assert!(!s.execute("markers.list", json!({"layer": outer_layer})).unwrap().as_array().unwrap().iter().any(|m| m["comment"] == "src"));
}

#[test]
fn precompose_leave_and_move_attributes() {
    use effectcraft_project::{ItemId, LayerId, LayerSource};
    let (mut s, l) = setup();
    s.execute("prop.set", json!({"layer": l, "path": "transform/position", "value": [50, 60]})).unwrap();
    let outer = s.active_comp_id().unwrap();
    // Leave all attributes: same layer, keeps its transform; the new comp is the source size and
    // holds a default layer of the original source.
    let r = s.execute("layer.precompose", json!({"layers": [l], "name": "Leave", "mode": "leave"})).unwrap();
    assert_eq!(r["layer"], l);
    let inner = ItemId(r["comp"].as_u64().unwrap());
    let lay = s.project.comp(outer).unwrap().layer(LayerId(l)).unwrap().clone();
    assert_eq!(lay.source, LayerSource::Comp { item: inner });
    assert_eq!(lay.props.prop("transform/position").unwrap().value.as_vec2(), [50.0, 60.0]);
    let ic = s.project.comp(inner).unwrap();
    assert_eq!((ic.width, ic.height), (100, 100));
    assert_eq!(ic.layers.len(), 1);
    assert_eq!(ic.layers[0].props.prop("transform/position").unwrap().value.as_vec2(), [50.0, 50.0]);
    assert!(matches!(ic.layers[0].source, LayerSource::Solid { .. }));
    s.execute("edit.undo", json!({})).unwrap();
    assert!(s.project.comp(inner).is_none());
    assert!(matches!(s.project.comp(outer).unwrap().layer(LayerId(l)).unwrap().source, LayerSource::Solid { .. }));
    // Leave needs one layer with a source item.
    let t = s.execute("layer.newText", json!({"text": "Hi"})).unwrap()["layer"].as_u64().unwrap();
    assert!(s.execute("layer.precompose", json!({"layers": [t], "mode": "leave"})).is_err());
    assert!(s.execute("layer.precompose", json!({"layers": [t, l], "mode": "leave"})).is_err());
    // Move all attributes + adjust duration + open: the layer moves inside, trimmed to its span.
    s.execute("layer.timing", json!({"layers": [l], "in": 1.0, "out": 3.0})).unwrap();
    let r = s.execute("layer.precompose", json!({"layers": [l], "name": "Move", "adjustDuration": true, "open": true})).unwrap();
    let inner = ItemId(r["comp"].as_u64().unwrap());
    let ic = s.project.comp(inner).unwrap();
    assert!((ic.duration.seconds() - 2.0).abs() < 1e-9);
    assert_eq!(ic.layers[0].props.prop("transform/position").unwrap().value.as_vec2(), [50.0, 60.0]);
    assert_eq!(ic.layers[0].in_point.seconds(), 0.0);
    let nl = s.project.comp(outer).unwrap().layer(LayerId(r["layer"].as_u64().unwrap())).unwrap().clone();
    assert_eq!((nl.in_point.seconds(), nl.out_point.seconds()), (1.0, 3.0));
    assert_eq!(s.active_comp_id(), Some(inner));
}

/// A precomp layer shows its comp's markers on its bar, mapped through its timing.
#[test]
fn nested_comp_markers_follow_the_precomp_layers_timing() {
    let mut s = crate::Session::default();
    let pre = s.execute("comp.new", json!({"name": "Pre", "width": 32, "height": 32, "frameRate": 10, "duration": 4})).unwrap()["comp"].as_u64().unwrap();
    for (t, c) in [(1.0, "beat"), (3.0, "drop")] {
        s.execute("markers.set", json!({"new": true, "time": t, "comment": c})).unwrap();
    }
    s.execute("comp.new", json!({"name": "Main", "width": 32, "height": 32, "frameRate": 10, "duration": 20})).unwrap();
    let l = s.execute("layer.addItem", json!({"item": pre})).unwrap()["layer"].as_u64().unwrap();
    s.execute("layer.timing", json!({"layers": [l], "start": 2.0})).unwrap();
    let times = |s: &mut crate::Session| -> Vec<(f64, String)> {
        let v = s.execute_checked("markers.nested", json!({"layer": l})).unwrap();
        v.as_array().unwrap().iter().map(|m| (m["time"].as_f64().unwrap(), m["comment"].as_str().unwrap().to_string())).collect()
    };
    assert_eq!(times(&mut s), vec![(3.0, "beat".to_string()), (5.0, "drop".to_string())]);
    // Stretched to 200 %: twice as far from the start.
    s.execute("layer.timeStretch", json!({"layers": [l], "percent": 200})).unwrap();
    assert_eq!(times(&mut s), vec![(4.0, "beat".to_string()), (8.0, "drop".to_string())]);
    s.execute("layer.timeStretch", json!({"layers": [l], "percent": 100})).unwrap();
    // Trimmed: markers outside the In–Out range are not on the bar.
    s.execute("layer.timing", json!({"layers": [l], "out": 4.0})).unwrap();
    assert_eq!(times(&mut s), vec![(3.0, "beat".to_string())]);
    s.execute("layer.timing", json!({"layers": [l], "out": 6.0})).unwrap();
    // Time remapping at half speed: nested 1 s is reached 2 s into the layer.
    s.execute("layer.enableTimeRemap", json!({"layers": [l]})).unwrap();
    s.execute("prop.addKey", json!({"layer": l, "path": "timeRemap", "time": 4.0, "value": 2.0})).unwrap();
    let t = times(&mut s);
    assert_eq!(t.first(), Some(&(4.0, "beat".to_string())), "{t:?}");
    // Not a precomp layer.
    let solid = s.execute("layer.newSolid", json!({"color": "#ffffff"})).unwrap()["layer"].as_u64().unwrap();
    assert!(s.execute("markers.nested", json!({"layer": solid})).is_err());
}

#[test]
fn layer_add_marker_without_selection_adds_comp_marker() {
    let (mut s, _) = setup();
    s.execute("time.set", json!({"time": 1.5})).unwrap();
    s.execute("edit.deselectAll", json!({})).unwrap();
    assert!(s.state.selected_layers.is_empty());
    s.execute("layer.addMarker", json!({"comment": "timeline marker"})).unwrap();
    let comp = s.active_comp().unwrap();
    assert_eq!(comp.markers.len(), 1);
    assert!((comp.markers[0].time.seconds() - 1.5).abs() < 1e-9);
    assert_eq!(comp.markers[0].comment, "timeline marker");
}

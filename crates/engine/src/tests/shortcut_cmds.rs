//! The keyboard-only commands: time navigation, the work area, selection and layer timing.

use effectcraft_time::Tick;
use serde_json::json;

use crate::Session;

/// A 10 s, 30 fps comp with three solids (#1 on top), each trimmed to `spans` (seconds).
fn session(spans: [(f64, f64); 3]) -> (Session, Vec<u64>) {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Main", "width": 100, "height": 100, "frameRate": 30, "duration": 10})).unwrap();
    for _ in 0..3 {
        s.execute("layer.newSolid", json!({"color": "#ff0000"})).unwrap();
    }
    let ids: Vec<u64> = s.active_comp().unwrap().layers.iter().map(|l| l.id.0).collect();
    for (id, (a, b)) in ids.iter().zip(spans) {
        s.execute("layer.timing", json!({"layers": [id], "in": a, "out": b})).unwrap();
    }
    (s, ids)
}

fn secs(s: &Session) -> f64 {
    s.time().seconds()
}

fn span(s: &Session, id: u64) -> (f64, f64) {
    let c = s.active_comp().unwrap();
    let l = c.layers.iter().find(|l| l.id.0 == id).unwrap();
    (l.in_point.seconds(), l.out_point.seconds())
}

#[test]
fn time_goes_to_the_work_area_and_to_in_and_out_points() {
    let (mut s, ids) = session([(1.0, 3.0), (2.0, 6.0), (0.0, 10.0)]);
    s.execute("comp.workArea", json!({"start": 2.0, "end": 4.0})).unwrap();
    s.execute("time.workAreaStart", json!({})).unwrap();
    assert_eq!(secs(&s), 2.0);
    s.execute("time.workAreaEnd", json!({})).unwrap();
    let frame = 1.0 / 30.0;
    assert!((secs(&s) - (4.0 - frame)).abs() < 1e-9);
    // In and Out points of the selected layers (an Out point is the layer's last frame).
    s.execute("layer.select", json!({"layers": [ids[0], ids[1]]})).unwrap();
    s.execute("time.set", json!({"time": 0.0})).unwrap();
    let mut seen = vec![];
    for _ in 0..5 {
        s.execute("time.nextInOut", json!({})).unwrap();
        seen.push(secs(&s));
    }
    let want = [1.0, 2.0, 3.0 - frame, 6.0 - frame, 6.0 - frame];
    assert!(seen.iter().zip(want).all(|(a, b)| (a - b).abs() < 1e-9), "{seen:?}");
    s.execute("time.previousInOut", json!({})).unwrap();
    assert!((secs(&s) - (3.0 - frame)).abs() < 1e-9);
    // With no layer selected: every layer's points.
    s.execute("edit.deselectAll", json!({})).unwrap();
    s.execute("time.set", json!({"time": 8.0})).unwrap();
    s.execute("time.nextInOut", json!({})).unwrap();
    assert!((secs(&s) - (10.0 - frame)).abs() < 1e-9);
}

#[test]
fn work_area_follows_the_selected_layers() {
    let (mut s, ids) = session([(1.0, 3.0), (2.0, 6.0), (0.0, 10.0)]);
    s.execute("layer.select", json!({"layers": [ids[0], ids[1]]})).unwrap();
    s.execute("comp.workAreaToSelection", json!({})).unwrap();
    let wa = s.active_comp().unwrap().work_area;
    assert_eq!((wa.0.seconds(), wa.1.seconds()), (1.0, 6.0));
    // With nothing selected: the whole composition.
    s.execute("edit.deselectAll", json!({})).unwrap();
    s.execute("comp.workAreaToSelection", json!({})).unwrap();
    let wa = s.active_comp().unwrap().work_area;
    assert_eq!((wa.0, wa.1.seconds()), (Tick::ZERO, 10.0));
    // One undo step each.
    assert!(s.undo());
    assert_eq!(s.active_comp().unwrap().work_area.1.seconds(), 6.0);
    // Layers reaching outside the comp don't take the work area with them.
    s.execute("layer.timing", json!({"layers": [ids[0]], "delta": -5.0})).unwrap();
    s.execute("layer.select", json!({"layers": [ids[0]]})).unwrap();
    s.execute("comp.workAreaToSelection", json!({})).unwrap();
    let wa = s.active_comp().unwrap().work_area;
    assert!(wa.0 == Tick::ZERO && wa.1 > wa.0);
}

#[test]
fn the_most_recent_comp_of_the_hierarchy_comes_back() {
    let mut s = Session::default();
    let new = |s: &mut Session, name: &str| s.execute("comp.new", json!({"name": name, "duration": 4})).unwrap()["comp"].as_u64().unwrap();
    let main = new(&mut s, "Main");
    let nested = new(&mut s, "Nested");
    let other = new(&mut s, "Other");
    // Main contains Nested; Other stands alone.
    s.execute("comp.open", json!({"comp": main})).unwrap();
    s.execute("layer.addItem", json!({"item": nested})).unwrap();
    s.execute("comp.open", json!({"comp": nested})).unwrap();
    s.execute("comp.open", json!({"comp": other})).unwrap();
    // Other has no related comp to go back to.
    assert!(s.execute("comp.activateRecent", json!({})).unwrap().is_null());
    s.execute("comp.open", json!({"comp": main})).unwrap();
    // From Main: Other was active last, but Nested is the one in Main's hierarchy.
    s.execute("comp.activateRecent", json!({})).unwrap();
    assert_eq!(s.state.active_comp.map(|c| c.0), Some(nested));
    s.execute("comp.activateRecent", json!({})).unwrap();
    assert_eq!(s.state.active_comp.map(|c| c.0), Some(main));
}

#[test]
fn keyframes_and_properties_deselect_but_layers_stay() {
    let (mut s, ids) = session([(0.0, 10.0); 3]);
    s.execute("layer.select", json!({"layers": [ids[0]]})).unwrap();
    s.execute("prop.addKey", json!({"layer": ids[0], "path": "transform/opacity", "time": 1.0, "value": 50.0})).unwrap();
    s.execute("keys.selectAll", json!({})).unwrap();
    assert!(!s.state.selected_keys.is_empty());
    s.execute("keys.deselectAll", json!({})).unwrap();
    assert!(s.state.selected_keys.is_empty() && s.state.selected_props.is_empty());
    assert_eq!(s.state.selected_layers.len(), 1);
}

#[test]
fn selection_extends_through_the_stack() {
    let (mut s, ids) = session([(0.0, 10.0); 3]);
    s.execute("layer.select", json!({"layers": [ids[0]]})).unwrap();
    s.execute("layer.extendSelectionNext", json!({})).unwrap();
    s.execute("layer.extendSelectionNext", json!({})).unwrap();
    let mut sel: Vec<u64> = s.state.selected_layers.iter().map(|l| l.0).collect();
    sel.sort();
    let mut all = ids.clone();
    all.sort();
    assert_eq!(sel, all);
    // At the end of the stack the selection stays.
    s.execute("layer.extendSelectionNext", json!({})).unwrap();
    assert_eq!(s.state.selected_layers.len(), 3);
    s.execute("layer.select", json!({"layers": [ids[2]]})).unwrap();
    s.execute("layer.extendSelectionPrevious", json!({})).unwrap();
    assert_eq!(s.state.selected_layers.len(), 2);
    assert!(s.state.selected_layers.iter().any(|l| l.0 == ids[1]));
}

#[test]
fn layers_move_to_the_comp_ends_and_by_ten_frames() {
    let (mut s, ids) = session([(1.0, 3.0), (2.0, 6.0), (0.0, 10.0)]);
    s.execute("layer.select", json!({"layers": [ids[0], ids[1]]})).unwrap();
    s.execute("layer.moveInToStart", json!({})).unwrap();
    assert_eq!((span(&s, ids[0]), span(&s, ids[1])), ((0.0, 2.0), (0.0, 4.0)));
    s.execute("layer.moveOutToEnd", json!({})).unwrap();
    assert_eq!((span(&s, ids[0]), span(&s, ids[1])), ((8.0, 10.0), (6.0, 10.0)));
    s.execute("layer.moveEarlier10", json!({})).unwrap();
    let (a, b) = span(&s, ids[0]);
    assert!((a - (8.0 - 10.0 / 30.0)).abs() < 1e-9 && (b - (10.0 - 10.0 / 30.0)).abs() < 1e-9);
    s.execute("layer.moveLater10", json!({})).unwrap();
    assert_eq!(span(&s, ids[0]), (8.0, 10.0));
    s.execute("layer.moveLater", json!({})).unwrap();
    s.execute("layer.moveEarlier", json!({})).unwrap();
    assert_eq!(span(&s, ids[0]), (8.0, 10.0));
    // The unselected layer never moved.
    assert_eq!(span(&s, ids[2]), (0.0, 10.0));
    for (id, keys) in [
        ("layer.moveEarlier", "Alt+PageUp"),
        ("layer.moveLater", "Alt+PageDown"),
        ("layer.moveEarlier10", "Alt+Shift+PageUp"),
        ("layer.moveLater10", "Alt+Shift+PageDown"),
    ] {
        assert_eq!(crate::find_command(id).and_then(|c| c.shortcut), Some(keys));
    }
}

#[test]
fn in_and_out_points_stretch_to_the_current_time() {
    let (mut s, ids) = session([(2.0, 4.0), (0.0, 10.0), (0.0, 10.0)]);
    let stretch = |s: &Session| s.active_comp().unwrap().layers.iter().find(|l| l.id.0 == ids[0]).unwrap().stretch;
    s.execute("layer.select", json!({"layers": [ids[0]]})).unwrap();
    // In point to 1 s: the Out point holds, the layer is 3 s long at 150 %.
    s.execute("time.set", json!({"time": 1.0})).unwrap();
    s.execute("layer.stretchInToTime", json!({})).unwrap();
    assert_eq!(span(&s, ids[0]), (1.0, 4.0));
    assert!((stretch(&s) - 150.0).abs() < 1e-6);
    // Out point to 7 s: the In point holds, 6 s at 300 %.
    s.execute("time.set", json!({"time": 7.0})).unwrap();
    s.execute("layer.stretchOutToTime", json!({})).unwrap();
    assert_eq!(span(&s, ids[0]), (1.0, 7.0));
    assert!((stretch(&s) - 300.0).abs() < 1e-6);
    // A current time before the In point can't be the Out point: the layer is left alone.
    s.execute("time.set", json!({"time": 0.5})).unwrap();
    s.execute("layer.stretchOutToTime", json!({})).unwrap();
    assert_eq!(span(&s, ids[0]), (1.0, 7.0));
    assert!(s.execute("layer.timeStretch", json!({"to": "middle"})).is_err());
}

#[test]
fn trimming_out_to_the_playhead_keeps_its_frame_visible() {
    let (mut s, ids) = session([(1.0, 9.0); 3]);
    let frame = 1.0 / 30.0;
    // Alt+] at 4 s: the Out edge lands one frame after the playhead, so 4 s is the layer's last
    // visible frame — it must not be cut away at the playhead (as in After Effects).
    s.execute("time.set", json!({"time": 4.0})).unwrap();
    s.execute("layer.timing", json!({"layers": [ids[0]], "op": "trimOutToTime"})).unwrap();
    assert!((span(&s, ids[0]).1 - (4.0 + frame)).abs() < 1e-9, "{:?}", span(&s, ids[0]));
    // Alt+[ at 2 s: the playhead's frame becomes the layer's first visible one.
    s.execute("time.set", json!({"time": 2.0})).unwrap();
    s.execute("layer.timing", json!({"layers": [ids[1]], "op": "trimInToTime"})).unwrap();
    assert!((span(&s, ids[1]).0 - 2.0).abs() < 1e-9, "{:?}", span(&s, ids[1]));
    // Trimmed before the In point, the Out point keeps one frame of layer.
    s.execute("time.set", json!({"time": 0.5})).unwrap();
    s.execute("layer.timing", json!({"layers": [ids[2]], "op": "trimOutToTime"})).unwrap();
    assert!((span(&s, ids[2]).1 - (1.0 + frame)).abs() < 1e-9, "{:?}", span(&s, ids[2]));
    // One undo step back to the original span.
    assert!(s.undo());
    assert!((span(&s, ids[2]).1 - 9.0).abs() < 1e-9);
}

#[test]
fn masks_are_stepped_through_and_wrap() {
    let (mut s, ids) = session([(0.0, 10.0); 3]);
    s.execute("layer.select", json!({"layers": [ids[0]]})).unwrap();
    assert!(s.execute("layer.mask.selectNext", json!({})).is_err());
    for _ in 0..3 {
        s.execute("layer.addMask", json!({"shape": "rect"})).unwrap();
    }
    s.execute("keys.deselectAll", json!({})).unwrap();
    let step = |s: &mut Session, cmd: &str| s.execute(cmd, json!({})).unwrap()["index"].as_u64().unwrap();
    assert_eq!(step(&mut s, "layer.mask.selectNext"), 1);
    assert_eq!(step(&mut s, "layer.mask.selectNext"), 2);
    assert_eq!(step(&mut s, "layer.mask.selectNext"), 3);
    assert_eq!(step(&mut s, "layer.mask.selectNext"), 1);
    assert_eq!(step(&mut s, "layer.mask.selectPrevious"), 3);
    assert_eq!(s.state.selected_props.len(), 1);
    s.execute("keys.deselectAll", json!({})).unwrap();
    assert_eq!(step(&mut s, "layer.mask.selectPrevious"), 3);
}

#[test]
fn casts_shadows_toggles_on_3d_layers_only() {
    let (mut s, ids) = session([(0.0, 10.0); 3]);
    s.execute("layer.select", json!({"layers": [ids[0], ids[1]]})).unwrap();
    assert!(s.execute("layer.castsShadows", json!({})).is_err());
    s.execute("layer.setSwitch", json!({"layers": [ids[0]], "switch": "threeD", "value": true})).unwrap();
    let before = s.execute("layer.castsShadows", json!({})).unwrap();
    assert_eq!(before["layers"], json!([ids[0]]));
    let after = s.execute("layer.castsShadows", json!({})).unwrap();
    assert_ne!(before["castsShadows"], after["castsShadows"]);
    assert_eq!(s.execute("layer.castsShadows", json!({"value": true})).unwrap()["castsShadows"], json!(true));
}

//! Timeline depth: keyframe clipboard, easing, velocity/interpolation, roving, nudging, graph
//! editor key edits, time remapping/stretch/reverse/freeze, separate dimensions, pick-whips,
//! expressions and masks (pen tool). Every command is checked with undo/redo.

use effectcraft_keyframe::{Interp, Value as KV};
use effectcraft_project::Property;
use serde_json::{Value, json};

use crate::Session;

/// A 4 s, 30 fps comp with one solid; returns (session, layer id).
pub(crate) fn setup() -> (Session, u64) {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "T", "width": 400, "height": 300, "frameRate": 30, "duration": 4})).unwrap();
    let r = s.execute("layer.newSolid", json!({"color": "#ffffff", "width": 100, "height": 100})).unwrap();
    (s, r["layer"].as_u64().unwrap())
}

pub(crate) fn prop(s: &Session, layer: u64, path: &str) -> Property {
    s.active_comp().unwrap().layer(effectcraft_project::LayerId(layer)).unwrap().props.prop(path).unwrap().clone()
}

/// Key `path` at the given (time, value) pairs.
pub(crate) fn animate(s: &mut Session, layer: u64, path: &str, keys: &[(f64, Value)]) {
    for (t, v) in keys {
        s.execute("prop.addKey", json!({"layer": layer, "path": path, "time": t, "value": v})).unwrap();
    }
}

fn select_prop_keys(s: &mut Session, layer: u64, path: &str) {
    s.execute("prop.select", json!({"layer": layer, "path": path})).unwrap();
}

fn undo_redo_roundtrip(s: &mut Session, check_after: impl Fn(&Session), check_before: impl Fn(&Session)) {
    check_after(s);
    s.execute("edit.undo", json!({})).unwrap();
    check_before(s);
    s.execute("edit.redo", json!({})).unwrap();
    check_after(s);
}

#[test]
fn easy_ease_family_sets_influence_and_zero_speed() {
    let (mut s, l) = setup();
    animate(&mut s, l, "transform/opacity", &[(0.0, json!(0)), (1.0, json!(100)), (2.0, json!(0))]);
    select_prop_keys(&mut s, l, "transform/opacity");
    s.execute("keys.easyEase", json!({})).unwrap();
    undo_redo_roundtrip(
        &mut s,
        |s| {
            let p = prop(s, l, "transform/opacity");
            assert!(p.keys.iter().all(|k| k.in_interp == Interp::Bezier && k.out_interp == Interp::Bezier));
            assert!((p.keys[1].in_ease[0].influence - 1.0 / 3.0).abs() < 1e-9 && p.keys[1].in_ease[0].speed == 0.0);
        },
        |s| assert!(prop(s, l, "transform/opacity").keys.iter().all(|k| k.in_interp == Interp::Linear)),
    );
    // Easy Ease In only touches the incoming side; Out only the outgoing side.
    s.execute("keys.interpolation", json!({"interpolation": "linear"})).unwrap();
    s.execute("keys.easyEaseIn", json!({})).unwrap();
    let p = prop(&s, l, "transform/opacity");
    assert!(p.keys.iter().all(|k| k.in_interp == Interp::Bezier && k.out_interp == Interp::Linear));
    s.execute("keys.interpolation", json!({"interpolation": "linear"})).unwrap();
    s.execute("keys.easyEaseOut", json!({})).unwrap();
    let p = prop(&s, l, "transform/opacity");
    assert!(p.keys.iter().all(|k| k.in_interp == Interp::Linear && k.out_interp == Interp::Bezier));
    // Eased motion is slow near the key.
    s.execute("keys.easyEase", json!({})).unwrap();
    let p = prop(&s, l, "transform/opacity");
    let early = p.value_at(effectcraft_time::Tick::from_seconds_f64(0.1)).as_f64();
    assert!(early < 5.0, "{early}");
}

#[test]
fn keyframe_copy_paste_keeps_relative_timing_and_crosses_properties() {
    let (mut s, l) = setup();
    animate(&mut s, l, "transform/rotation", &[(0.5, json!(0)), (1.0, json!(90)), (2.0, json!(45))]);
    select_prop_keys(&mut s, l, "transform/rotation");
    s.execute("keys.easyEase", json!({})).unwrap();
    assert_eq!(s.execute("edit.copy", json!({})).unwrap(), json!(3));
    assert!(s.state.clip_is_keys);
    // Paste into the same property at 2.5 s: keys at 2.5, 3.0 and 4.0 (clamped by nothing).
    s.execute("time.set", json!({"time": 2.5})).unwrap();
    s.execute("keys.select", json!({"keys": []})).unwrap();
    s.execute("edit.paste", json!({})).unwrap();
    let after = |s: &Session| {
        let p = prop(s, l, "transform/rotation");
        let times: Vec<f64> = p.keys.iter().map(|k| (k.time.seconds() * 30.0).round() / 30.0).collect();
        assert_eq!(times, vec![0.5, 1.0, 2.0, 2.5, 3.0, 4.0]);
        assert_eq!(p.keys[4].value, KV::Scalar(90.0));
        assert_eq!(p.keys[4].in_interp, Interp::Bezier, "interpolation travels with the keys");
        assert_eq!(s.state.selected_keys.len(), 3, "pasted keys are selected");
    };
    undo_redo_roundtrip(&mut s, after, |s| assert_eq!(prop(s, l, "transform/rotation").keys.len(), 3));
    // Cross-property paste of one property's keys into another of the same type (Scalar).
    let r = s.execute("layer.newSolid", json!({"color": "#ff0000"})).unwrap();
    let l2 = r["layer"].as_u64().unwrap();
    select_prop_keys(&mut s, l, "transform/rotation");
    s.execute("keys.select", json!({"keys": [{"layer": l, "prop": prop(&s, l, "transform/rotation").uid, "time": 1.0}]})).unwrap();
    s.execute("keys.copy", json!({})).unwrap();
    s.execute("time.set", json!({"time": 1.0})).unwrap();
    s.execute("prop.select", json!({"layer": l2, "path": "transform/opacity", "selectKeys": false})).unwrap();
    s.execute("keys.paste", json!({"layers": [l2]})).unwrap();
    let op = prop(&s, l2, "transform/opacity");
    assert_eq!(op.keys.len(), 1);
    assert_eq!(op.keys[0].value, KV::Scalar(90.0));
    // Incompatible target (Vec3 position) is refused when explicitly given.
    let e = s.execute("keys.paste", json!({"layers": [l2], "path": "transform/position"}));
    assert!(e.is_err() || prop(&s, l2, "transform/position").keys.is_empty());
}

#[test]
fn copy_selected_keys_excludes_deselected_properties_and_stale_values() {
    let (mut s, source) = setup();
    s.execute("prop.set", json!({"layer": source, "path": "transform/opacity", "value": 25})).unwrap();
    select_prop_keys(&mut s, source, "transform/opacity");
    s.execute("edit.copy", json!({})).unwrap();
    assert_eq!(s.state.value_clipboard.len(), 1);
    animate(&mut s, source, "transform/position", &[(0.0, json!([10, 20, 0])), (1.0, json!([30, 40, 0]))]);
    animate(&mut s, source, "transform/rotation", &[(0.0, json!(15)), (1.0, json!(90))]);
    animate(&mut s, source, "transform/scale", &[(0.0, json!([50, 60, 100])), (1.0, json!([150, 160, 100]))]);
    select_prop_keys(&mut s, source, "transform/scale");
    s.execute(
        "keys.select",
        json!({"keys": [{"layer": source, "path": "transform/position", "time": 0}, {"layer": source, "path": "transform/rotation", "time": 1}], "add": true, "selectProperties": true}),
    )
    .unwrap();
    s.execute(
        "keys.select",
        json!({"keys": [{"layer": source, "path": "transform/scale", "time": 0}, {"layer": source, "path": "transform/scale", "time": 1}], "toggle": true, "selectProperties": true}),
    )
    .unwrap();
    s.execute("keys.easyEase", json!({})).unwrap();
    assert_eq!(s.state.selected_keys.len(), 2);
    assert_eq!(s.state.selected_props.len(), 3);
    assert_eq!(s.execute("edit.copy", json!({})).unwrap(), json!(2));
    assert!(s.state.value_clipboard.is_empty());
    let target = s.execute("layer.newSolid", json!({"color": "#ff0000"})).unwrap()["layer"].as_u64().unwrap();
    s.execute("time.set", json!({"time": 2})).unwrap();
    s.execute("layer.select", json!({"layers": [target]})).unwrap();
    s.execute("edit.paste", json!({})).unwrap();
    undo_redo_roundtrip(
        &mut s,
        |s| {
            for (path, time, value) in [("transform/position", 2.0, KV::Vec3([10.0, 20.0, 0.0])), ("transform/rotation", 3.0, KV::Scalar(90.0))] {
                let p = prop(s, target, path);
                assert_eq!(p.keys.len(), 1);
                let k = &p.keys[0];
                assert_eq!((k.time.seconds(), &k.value), (time, &value));
                assert_eq!((k.in_interp, k.out_interp), (Interp::Bezier, Interp::Bezier));
                assert_eq!(k.in_ease[0], effectcraft_keyframe::Ease::EASY);
                assert_eq!(k.out_ease[0], effectcraft_keyframe::Ease::EASY);
            }
            assert_eq!(s.state.selected_keys.len(), 2);
            assert_eq!(prop(s, target, "transform/scale").value, KV::Vec3([100.0; 3]));
            assert!(prop(s, target, "transform/scale").keys.is_empty());
            assert_eq!(prop(s, target, "transform/opacity").value, KV::Scalar(100.0));
        },
        |s| {
            assert!(prop(s, target, "transform/position").keys.is_empty());
            assert!(prop(s, target, "transform/rotation").keys.is_empty());
        },
    );
}

#[test]
fn copy_mixed_animated_and_static_properties_keeps_the_static_value() {
    let (mut s, source) = setup();
    animate(&mut s, source, "transform/position", &[(0.0, json!([10, 20, 0])), (1.0, json!([30, 40, 0]))]);
    s.execute("prop.set", json!({"layer": source, "path": "transform/rotation", "value": 45})).unwrap();
    select_prop_keys(&mut s, source, "transform/position");
    s.execute("prop.select", json!({"layer": source, "path": "transform/rotation", "add": true})).unwrap();
    assert_eq!(s.execute("edit.copy", json!({})).unwrap(), json!({"keys": 2, "values": 1}));
    let target = s.execute("layer.newSolid", json!({"color": "#ff0000"})).unwrap()["layer"].as_u64().unwrap();
    s.execute("time.set", json!({"time": 2})).unwrap();
    s.execute("layer.select", json!({"layers": [target]})).unwrap();
    let undo = s.history.undo.len();
    s.execute("edit.paste", json!({})).unwrap();
    assert_eq!(s.history.undo.len(), undo + 1);
    assert_eq!(prop(&s, target, "transform/rotation").value, KV::Scalar(45.0));
    let position = prop(&s, target, "transform/position");
    assert_eq!(
        position.keys.iter().map(|k| (k.time.seconds(), k.value.clone())).collect::<Vec<_>>(),
        vec![(2.0, KV::Vec3([10.0, 20.0, 0.0])), (3.0, KV::Vec3([30.0, 40.0, 0.0]))]
    );
    s.execute("edit.undo", json!({})).unwrap();
    assert!(prop(&s, target, "transform/position").keys.is_empty());
    assert_eq!(prop(&s, target, "transform/rotation").value, KV::Scalar(0.0));
}

#[test]
fn cut_removes_keys_and_paste_restores_them() {
    let (mut s, l) = setup();
    animate(&mut s, l, "transform/opacity", &[(0.0, json!(10)), (1.0, json!(20))]);
    select_prop_keys(&mut s, l, "transform/opacity");
    s.execute("edit.cut", json!({})).unwrap();
    assert!(prop(&s, l, "transform/opacity").keys.is_empty());
    s.execute("time.set", json!({"time": 0.0})).unwrap();
    s.execute("layer.select", json!({"layers": [l]})).unwrap();
    s.execute("edit.paste", json!({})).unwrap();
    assert_eq!(prop(&s, l, "transform/opacity").keys.len(), 2);
}

#[test]
fn select_all_and_nudge_keys() {
    let (mut s, l) = setup();
    animate(&mut s, l, "transform/opacity", &[(0.0, json!(10)), (1.0, json!(20))]);
    animate(&mut s, l, "transform/rotation", &[(0.5, json!(10))]);
    s.state.selected_props.clear();
    s.execute("layer.select", json!({"layers": [l]})).unwrap();
    assert_eq!(s.execute("keys.selectAll", json!({})).unwrap(), json!(3));
    s.execute("keys.nudgeForward", json!({})).unwrap();
    let after = |s: &Session| {
        let p = prop(s, l, "transform/opacity");
        assert!((p.keys[0].time.seconds() - 1.0 / 30.0).abs() < 1e-6);
        assert_eq!(s.state.selected_keys.len(), 3);
    };
    undo_redo_roundtrip(&mut s, after, |s| assert_eq!(prop(s, l, "transform/opacity").keys[0].time.0, 0));
    s.execute("keys.nudgeBackward10", json!({})).unwrap();
    let p = prop(&s, l, "transform/opacity");
    assert!((p.keys[0].time.seconds() + 9.0 / 30.0).abs() < 1e-6, "{}", p.keys[0].time.seconds());
}

#[test]
fn velocity_dialog_values_per_dimension_and_continuous() {
    let (mut s, l) = setup();
    animate(&mut s, l, "transform/scale", &[(0.0, json!([100, 100])), (1.0, json!([200, 50])), (2.0, json!([100, 100]))]);
    s.execute("keys.select", json!({"keys": [{"layer": l, "prop": prop(&s, l, "transform/scale").uid, "time": 1.0}]})).unwrap();
    let info = s.execute("keys.info", json!({})).unwrap();
    assert_eq!(info[0]["units"], "%/sec");
    assert_eq!(info[0]["dims"], 2);
    // Linear sides present the segment slope: +100 %/s in X on the incoming side.
    assert!((info[0]["inEase"][0]["speed"].as_f64().unwrap() - 100.0).abs() < 1e-6, "{info}");
    s.execute("keys.velocity", json!({"inSpeed": [10, -20], "inInfluence": 50, "continuous": true})).unwrap();
    let after = |s: &Session| {
        let k = &prop(s, l, "transform/scale").keys[1];
        assert_eq!(k.in_ease[0].speed, 10.0);
        assert_eq!(k.in_ease[1].speed, -20.0);
        assert_eq!(k.out_ease[1].speed, -20.0, "continuous mirrors incoming");
        assert!((k.out_ease[0].influence - 0.5).abs() < 1e-9);
        assert!(k.continuous);
    };
    undo_redo_roundtrip(&mut s, after, |s| assert_eq!(prop(s, l, "transform/scale").keys[1].in_interp, Interp::Linear));
}

/// Spatial Bezier keeps the handles a key has and gives a straight linear path handles a third of
/// the way along it (they had none), so the path can be pulled into a curve; the motion stays the
/// same (#290). Auto-Bezier keys keep their tangents, a sixth of the way to the neighbour here.
#[test]
fn spatial_bezier_gives_a_straight_path_handles() {
    let (mut s, l) = setup();
    animate(&mut s, l, "transform/position", &[(0.0, json!([0, 0])), (2.0, json!([300, 0]))]);
    let mid = prop(&s, l, "transform/position").value_at(effectcraft_time::Tick::from_seconds_f64(0.7)).as_vec3();
    select_prop_keys(&mut s, l, "transform/position");
    for (from, handle) in [("autoBezier", 50.0), ("linear", 100.0)] {
        s.execute("keys.interpolation", json!({"spatial": from})).unwrap();
        s.execute("keys.interpolation", json!({"spatial": "bezier"})).unwrap();
        let p = prop(&s, l, "transform/position");
        assert!(p.keys.iter().all(|k| !k.spatial_auto));
        assert_eq!((p.keys[0].spatial_out, p.keys[1].spatial_in), ([handle, 0.0, 0.0], [-handle, 0.0, 0.0]), "from {from}");
        let now = p.value_at(effectcraft_time::Tick::from_seconds_f64(0.7)).as_vec3();
        assert!((now[0] - mid[0]).abs() < 0.01 && now[1].abs() < 1e-9, "from {from}: {mid:?} → {now:?}");
    }
}

#[test]
fn interpolation_dialog_temporal_spatial_and_roving() {
    let (mut s, l) = setup();
    animate(&mut s, l, "transform/position", &[(0.0, json!([0, 0])), (0.3, json!([300, 0])), (2.0, json!([400, 0]))]);
    select_prop_keys(&mut s, l, "transform/position");
    s.execute("keys.interpolation", json!({"interpolation": "hold"})).unwrap();
    assert!(prop(&s, l, "transform/position").keys.iter().all(|k| k.out_interp == Interp::Hold));
    s.execute("keys.interpolation", json!({"interpolation": "autoBezier"})).unwrap();
    assert!(prop(&s, l, "transform/position").keys.iter().all(|k| k.auto_bezier));
    s.execute("keys.interpolation", json!({"interpolation": "continuousBezier", "spatial": "linear"})).unwrap();
    let p = prop(&s, l, "transform/position");
    assert!(p.keys.iter().all(|k| k.continuous && !k.auto_bezier && !k.spatial_auto && k.spatial_out == [0.0; 3]));
    // Rove the middle key: its time follows the path length (300 of 400 px → 1.5 s).
    let uid = p.uid;
    s.execute("keys.select", json!({"keys": [{"layer": l, "prop": uid, "time": 0.3}]})).unwrap();
    s.execute("keys.interpolation", json!({"roving": true})).unwrap();
    let after = |s: &Session| {
        let p = prop(s, l, "transform/position");
        assert!(p.keys[1].roving);
        assert!((p.keys[1].time.seconds() - 1.5).abs() < 0.01, "{}", p.keys[1].time.seconds());
        // The selection follows the roving key.
        assert!((s.state.selected_keys[0].time.seconds() - 1.5).abs() < 0.01);
    };
    undo_redo_roundtrip(&mut s, after, |s| assert!(!prop(s, l, "transform/position").keys[1].roving));
    // Moving the last key re-times the roving key.
    s.execute("keys.set", json!({"layer": l, "path": "transform/position", "time": 2.0, "newTime": 4.0})).unwrap();
    let p = prop(&s, l, "transform/position");
    assert!((p.keys[1].time.seconds() - 3.0).abs() < 0.02, "{}", p.keys[1].time.seconds());
    // Toggle hold on a key and back.
    s.execute("keys.select", json!({"keys": [{"layer": l, "prop": uid, "time": 0.0}]})).unwrap();
    s.execute("keys.toggleHold", json!({})).unwrap();
    assert_eq!(prop(&s, l, "transform/position").keys[0].out_interp, Interp::Hold);
    s.execute("keys.toggleHold", json!({})).unwrap();
    assert_ne!(prop(&s, l, "transform/position").keys[0].out_interp, Interp::Hold);
}

/// Editing another key's value or time keeps the roving key at its share of the path (#1284: the
/// roving key went back to where it was after edits to other keys).
#[test]
fn roving_key_follows_value_and_time_edits_to_other_keys() {
    let (mut s, l) = setup();
    animate(&mut s, l, "transform/position", &[(0.0, json!([0, 0])), (0.3, json!([300, 0])), (2.0, json!([400, 0]))]);
    let uid = prop(&s, l, "transform/position").uid;
    s.execute("keys.select", json!({"keys": [{"layer": l, "prop": uid, "time": 0.3}]})).unwrap();
    s.execute("keys.interpolation", json!({"roving": true})).unwrap();
    let roving_at = |s: &Session| prop(s, l, "transform/position").keys[1].time.seconds();
    assert!((roving_at(&s) - 1.5).abs() < 0.01, "start: {}", roving_at(&s));
    // The last key's value moves to 800 px: the path is 300 + 500 px, so the roving key is at 300 / 800.
    s.execute("keys.set", json!({"layer": l, "path": "transform/position", "time": 2.0, "value": [800, 0]})).unwrap();
    assert!((roving_at(&s) - 0.75).abs() < 0.02, "value edit: {}", roving_at(&s));
    // The last key moves to 3 s: the same 300 / 800 share of 3 s.
    s.execute("keys.set", json!({"layer": l, "path": "transform/position", "time": 2.0, "newTime": 3.0})).unwrap();
    assert!((roving_at(&s) - 1.125).abs() < 0.02, "time edit: {}", roving_at(&s));
}

#[test]
fn graph_editor_key_and_handle_edits() {
    let (mut s, l) = setup();
    animate(&mut s, l, "transform/opacity", &[(0.0, json!(0)), (2.0, json!(100))]);
    // Drag a key in the value graph: new time + value, merged into one undo step.
    s.execute("keys.set", json!({"layer": l, "path": "transform/opacity", "time": 2.0, "newTime": 1.0, "value": 50, "merge": "g1"})).unwrap();
    s.execute("keys.set", json!({"layer": l, "path": "transform/opacity", "time": 1.0, "newTime": 1.5, "value": 60, "merge": "g1"})).unwrap();
    let p = prop(&s, l, "transform/opacity");
    assert!((p.keys[1].time.seconds() - 1.5).abs() < 1e-6);
    assert_eq!(p.keys[1].value, KV::Scalar(60.0));
    s.execute("edit.undo", json!({})).unwrap();
    assert!((prop(&s, l, "transform/opacity").keys[1].time.seconds() - 2.0).abs() < 1e-6);
    s.execute("edit.redo", json!({})).unwrap();
    // A key can't be dragged past its neighbour.
    s.execute("keys.set", json!({"layer": l, "path": "transform/opacity", "time": 1.5, "newTime": -1.0})).unwrap();
    let p = prop(&s, l, "transform/opacity");
    assert!(p.keys[1].time > p.keys[0].time);
    // Handle drag: out side of key 0 → speed 200 %/s, 60 % influence.
    s.execute("keys.setEase", json!({"layer": l, "path": "transform/opacity", "time": 0.0, "side": "out", "speed": 200, "influence": 60})).unwrap();
    let k = &prop(&s, l, "transform/opacity").keys[0];
    assert_eq!(k.out_interp, Interp::Bezier);
    assert_eq!(k.out_ease[0].speed, 200.0);
    assert!((k.out_ease[0].influence - 0.6).abs() < 1e-9);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(prop(&s, l, "transform/opacity").keys[0].out_interp, Interp::Linear);
}

#[test]
fn graph_editor_ease_edits_reject_a_locked_layer() {
    let (mut s, l) = setup();
    animate(&mut s, l, "transform/opacity", &[(0.0, json!(0)), (1.0, json!(100)), (2.0, json!(0))]);
    select_prop_keys(&mut s, l, "transform/opacity");
    s.execute("layer.setSwitch", json!({"layers": [l], "switch": "locked", "value": true})).unwrap();
    let before = prop(&s, l, "transform/opacity");
    let selected = s.state.selected_keys.clone();
    let undo = s.history.undo.len();
    for side in ["in", "out"] {
        let error =
            s.execute("keys.setEase", json!({"layer": l, "path": "transform/opacity", "time": 1.0, "side": side, "speed": 200, "influence": 60})).unwrap_err();
        assert!(error.to_string().contains("locked"), "{error}");
        assert_eq!(prop(&s, l, "transform/opacity").keys, before.keys);
        assert_eq!(s.state.selected_keys, selected);
        assert_eq!(s.history.undo.len(), undo);
    }
    s.execute("layer.setSwitch", json!({"layers": [l], "switch": "locked", "value": false})).unwrap();
    s.execute("keys.setEase", json!({"layer": l, "path": "transform/opacity", "time": 1.0, "side": "out", "speed": 200, "influence": 60})).unwrap();
    let after = prop(&s, l, "transform/opacity");
    assert_eq!(after.keys[1].out_interp, Interp::Bezier);
    assert_eq!(after.keys[1].out_ease[0].speed, 200.0);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(prop(&s, l, "transform/opacity").keys, before.keys);
    s.execute("edit.redo", json!({})).unwrap();
    assert_eq!(prop(&s, l, "transform/opacity").keys, after.keys);
}

#[test]
fn convert_expression_to_keyframes_on_a_reversed_layer_keeps_values() {
    let (mut s, l) = setup();
    // These tests run without an expression host, so the conversion bakes the keyed ramp under the expression.
    animate(&mut s, l, "transform/rotation", &[(0.0, json!(0)), (3.0, json!(30))]);
    s.execute("prop.setExpression", json!({"layer": l, "path": "transform/rotation", "expression": "value"})).unwrap();
    s.execute("layer.timeReverse", json!({"layers": [l]})).unwrap();
    let at = |t: f64| json!({"layer": l, "path": "transform/rotation", "time": t});
    let sample = |s: &mut Session| [0.5, 1.0, 2.0].map(|t| s.execute("prop.get", at(t)).unwrap()["value"].as_f64().unwrap());
    let before = sample(&mut s);
    assert!(before[0] > before[1] && before[1] > before[2], "{before:?}");
    s.execute("prop.convertExpressionToKeyframes", json!({"layer": l, "path": "transform/rotation"})).unwrap();
    let keys = prop(&s, l, "transform/rotation").keys;
    assert!(keys.len() > 1);
    assert!(keys.windows(2).all(|w| w[0].time < w[1].time));
    let after = sample(&mut s);
    assert!(before.iter().zip(after).all(|(b, a)| (b - a).abs() < 1e-6), "{before:?} -> {after:?}");
}

#[test]
fn time_reverse_keyframes() {
    let (mut s, l) = setup();
    animate(&mut s, l, "transform/opacity", &[(0.0, json!(0)), (1.0, json!(30)), (3.0, json!(100))]);
    select_prop_keys(&mut s, l, "transform/opacity");
    s.execute("keys.timeReverse", json!({})).unwrap();
    let after = |s: &Session| {
        let p = prop(s, l, "transform/opacity");
        assert_eq!(p.keys[0].value, KV::Scalar(100.0));
        assert!((p.keys[1].time.seconds() - 2.0).abs() < 1e-6);
    };
    undo_redo_roundtrip(&mut s, after, |s| assert_eq!(prop(s, l, "transform/opacity").keys[0].value, KV::Scalar(0.0)));
}

#[test]
fn pen_tool_draws_edits_and_closes_a_mask() {
    let (mut s, l) = setup();
    // Click (corner), click, drag (Bezier), click on the first vertex (close).
    let r = s.execute("mask.new", json!({"layer": l, "vertices": [[10, 10]]})).unwrap();
    let m = r["mask"].as_u64().unwrap();
    s.execute("mask.addVertex", json!({"layer": l, "mask": m, "point": [90, 10]})).unwrap();
    s.execute("mask.addVertex", json!({"layer": l, "mask": m, "point": [90, 90]})).unwrap();
    s.execute("mask.setVertex", json!({"layer": l, "mask": m, "index": 2, "in": [0, -20], "out": [0, 20], "merge": "pen"})).unwrap();
    s.execute("mask.addVertex", json!({"layer": l, "mask": m, "point": [10, 90]})).unwrap();
    s.execute("mask.setClosed", json!({"layer": l, "mask": m, "closed": true})).unwrap();
    let path = |s: &Session| prop(s, l, "masks/#1/path").value.as_path().unwrap().clone();
    let p = path(&s);
    assert_eq!(p.vertices, vec![[10.0, 10.0], [90.0, 10.0], [90.0, 90.0], [10.0, 90.0]]);
    assert_eq!(p.out_tangents[2], [0.0, 20.0]);
    assert!(p.closed);
    assert_eq!(s.state.selected_vertices.last().unwrap().index, 3);
    // The mask cuts the 100×100 solid (layer centred in the 400×300 comp at 150..250, 100..200).
    let cid = s.active_comp_id().unwrap();
    let img = s.render(cid, effectcraft_time::Tick::ZERO, effectcraft_render::RenderOpts { scale: 1.0, ..Default::default() });
    assert!(img.get(200, 150)[3] > 0.99 && img.get(152, 102)[3] < 0.01);
    // Undo removes the close, redo restores it.
    s.execute("edit.undo", json!({})).unwrap();
    assert!(!path(&s).closed);
    s.execute("edit.redo", json!({})).unwrap();
    // Select two vertices and drag them; Delete removes them.
    s.execute("mask.selectVertices", json!({"vertices": [{"layer": l, "mask": m, "index": 0}, {"layer": l, "mask": m, "index": 1}]})).unwrap();
    s.execute("mask.moveVertices", json!({"delta": [5, -5], "merge": "v1"})).unwrap();
    s.execute("mask.moveVertices", json!({"delta": [5, -5], "merge": "v1"})).unwrap();
    assert_eq!(path(&s).vertices[1], [100.0, 0.0]);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(path(&s).vertices[1], [90.0, 10.0], "a drag is one undo step");
    s.execute("edit.clear", json!({})).unwrap();
    assert_eq!(path(&s).vertices.len(), 2);
    assert!(!path(&s).closed);
    // Animated mask paths get a key at the current time.
    s.execute("prop.toggleAnimation", json!({"layer": l, "path": "masks/#1/path"})).unwrap();
    s.execute("time.set", json!({"time": 1.0})).unwrap();
    s.execute("mask.setVertex", json!({"layer": l, "mask": 1, "index": 0, "point": [0, 0]})).unwrap();
    assert_eq!(prop(&s, l, "masks/#1/path").keys.len(), 2);
    // Mode / feather / opacity / expansion through the timeline commands.
    s.execute("layer.setMask", json!({"layer": l, "mask": m, "mode": "Subtract", "inverted": true})).unwrap();
    s.execute("prop.set", json!({"layer": l, "path": "masks/#1/feather", "value": [4, 4]})).unwrap();
    s.execute("prop.set", json!({"layer": l, "path": "masks/#1/expansion", "value": 3})).unwrap();
    s.execute("prop.set", json!({"layer": l, "path": "masks/#1/opacity", "value": 50})).unwrap();
    assert_eq!(prop(&s, l, "masks/#1/opacity").value, KV::Scalar(50.0));
    s.execute("mask.removeAll", json!({"layer": l})).unwrap();
    assert!(s.active_comp().unwrap().layers[0].masks().unwrap().children.is_empty());
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.active_comp().unwrap().layers[0].masks().unwrap().children.len(), 1);
}

/// Cut every mask path's tangent arrays in the saved project to one entry (static values and
/// keyframes alike), then open the result: the shape of a hand-edited or foreign file (#1250).
fn reopen_with_short_tangents(s: &mut Session) {
    fn cut(v: &mut Value) {
        match v {
            Value::Object(o) => {
                if o.contains_key("vertices") {
                    for k in ["in_tangents", "out_tangents"] {
                        if let Some(Value::Array(a)) = o.get_mut(k) {
                            a.truncate(1);
                        }
                    }
                }
                o.values_mut().for_each(cut);
            }
            Value::Array(a) => a.iter_mut().for_each(cut),
            _ => {}
        }
    }
    let mut j: Value = serde_json::from_str(&s.project.to_json()).unwrap();
    cut(&mut j);
    let p = effectcraft_project::Project::from_json(&j.to_string()).unwrap();
    s.replace_project(p, None);
}

#[test]
fn vertex_commands_fill_in_missing_tangents_of_a_loaded_path() {
    let (mut s, l) = setup();
    let m = s.execute("mask.new", json!({"layer": l, "vertices": [[10, 10], [90, 10], [90, 90]]})).unwrap()["mask"].as_u64().unwrap();
    reopen_with_short_tangents(&mut s);
    let path = |s: &Session| prop(s, l, "masks/#1/path").value.as_path().unwrap().clone();
    assert_eq!(path(&s).in_tangents.len(), 1, "the file really has short tangents");
    // Both used to index the tangent arrays by vertex and panic.
    s.execute("mask.setVertex", json!({"layer": l, "mask": m, "index": 2, "in": [0, -20], "out": [0, 20]})).unwrap();
    s.execute("mask.addVertex", json!({"layer": l, "mask": m, "point": [10, 90]})).unwrap();
    let p = path(&s);
    assert_eq!(p.vertices.len(), 4);
    assert_eq!((p.in_tangents.len(), p.out_tangents.len()), (4, 4));
    assert_eq!((p.in_tangents[2], p.out_tangents[2]), ([0.0, -20.0], [0.0, 20.0]));
    assert_eq!(p.in_tangents[1], [0.0, 0.0], "a missing tangent is a corner");
}

#[test]
fn deleting_vertices_of_a_loaded_path_with_short_tangents_does_not_panic() {
    let (mut s, l) = setup();
    let m = s.execute("mask.new", json!({"layer": l, "vertices": [[10, 10], [90, 10], [90, 90]]})).unwrap()["mask"].as_u64().unwrap();
    reopen_with_short_tangents(&mut s);
    s.execute("mask.selectVertices", json!({"vertices": [{"layer": l, "mask": m, "index": 2}]})).unwrap();
    s.execute("mask.deleteVertices", json!({})).unwrap();
    let p = prop(&s, l, "masks/#1/path").value.as_path().unwrap().clone();
    assert_eq!((p.vertices.len(), p.in_tangents.len(), p.out_tangents.len()), (2, 2, 2));
}

#[test]
fn vertex_commands_fill_in_missing_tangents_of_a_loaded_animated_path() {
    let (mut s, l) = setup();
    let m = s.execute("mask.new", json!({"layer": l, "vertices": [[10, 10], [90, 10], [90, 90]]})).unwrap()["mask"].as_u64().unwrap();
    s.execute("prop.toggleAnimation", json!({"layer": l, "path": "masks/#1/path"})).unwrap();
    s.execute("time.set", json!({"time": 1.0})).unwrap();
    s.execute("mask.setVertex", json!({"layer": l, "mask": m, "index": 0, "point": [0, 0]})).unwrap();
    reopen_with_short_tangents(&mut s);
    s.execute("time.set", json!({"time": 1.0})).unwrap();
    s.execute("mask.setVertex", json!({"layer": l, "mask": m, "index": 2, "in": [5, 5]})).unwrap();
    s.execute("mask.addVertex", json!({"layer": l, "mask": m, "index": 3, "point": [10, 90]})).unwrap();
    for k in &prop(&s, l, "masks/#1/path").keys {
        let p = k.value.as_path().unwrap();
        assert_eq!(p.vertices.len(), 4, "the other key gains the vertex too");
        assert!(p.in_tangents.len() == 4 && p.out_tangents.len() == 4, "{p:?}");
    }
}

#[test]
fn alt_click_stopwatch_toggles_a_self_reference_expression() {
    let (mut s, l) = setup();
    s.execute("prop.setExpression", json!({"layer": l, "path": "transform/opacity"})).unwrap();
    assert_eq!(prop(&s, l, "transform/opacity").expr.unwrap().text, "transform.opacity");
    s.execute("prop.setExpression", json!({"layer": l, "path": "transform/opacity", "enabled": false})).unwrap();
    assert!(!prop(&s, l, "transform/opacity").expr.unwrap().enabled);
    s.execute("prop.setExpression", json!({"layer": l, "path": "transform/opacity", "enabled": true})).unwrap();
    s.execute("prop.setExpression", json!({"layer": l, "path": "transform/opacity"})).unwrap();
    assert!(prop(&s, l, "transform/opacity").expr.is_none());
    s.execute("edit.undo", json!({})).unwrap();
    assert!(prop(&s, l, "transform/opacity").expr.unwrap().enabled);
}

#[test]
fn add_expression_without_path_uses_the_selected_property() {
    let (mut s, l) = setup();
    // Nothing selected: the same hint as Add Property to Essential Graphics.
    let err = s.execute("prop.setExpression", json!({})).unwrap_err().to_string();
    assert!(err.contains("select a property in the timeline"), "{err}");
    assert!(prop(&s, l, "transform/opacity").expr.is_none());
    // Animation > Add Expression sends no parameters.
    s.execute("prop.select", json!({"layer": l, "path": "transform/opacity"})).unwrap();
    s.execute("prop.setExpression", json!({})).unwrap();
    assert_eq!(prop(&s, l, "transform/opacity").expr.unwrap().text, "transform.opacity");
    // One undo step removes it.
    s.execute("edit.undo", json!({})).unwrap();
    assert!(prop(&s, l, "transform/opacity").expr.is_none());
}

#[test]
fn layer_comp_and_key_times_stay_frame_aligned() {
    let aligned = |fr: effectcraft_time::FrameRate, t: effectcraft_time::Tick| fr.snap(t) == t;
    // The demo project: every comp duration, layer in/out/start and key time is on a frame.
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    for (_, c) in s.project.comps() {
        let fr = c.frame_rate;
        assert!(aligned(fr, c.duration), "duration {:?}", c.duration);
        for l in &c.layers {
            for t in [l.in_point, l.out_point, l.start_time] {
                assert!(aligned(fr, t), "{} {t:?}", l.name);
            }
            l.props.walk("", &mut |_, pr| {
                for k in &pr.keys {
                    assert!(aligned(fr, l.comp_time(k.time)), "{} {}", l.name, pr.name);
                }
            });
        }
    }
    let lower = s.active_comp().unwrap().layer_by_name("Lower Third").unwrap();
    let fr = s.active_comp().unwrap().frame_rate;
    assert_eq!(fr.frame_at(lower.in_point), 150, "5 s at 29.97 is frame 150 (0:00:05:00)");
    // Commands snap: comp duration, layer timing, key times.
    s.execute("comp.new", json!({"name": "F", "frameRate": 29.97, "duration": 10.0})).unwrap();
    let c = s.active_comp().unwrap();
    assert_eq!(c.frame_rate.frame_at(c.duration), 300);
    assert!(aligned(c.frame_rate, c.duration));
    let l = s.execute("layer.newSolid", json!({"color": "#fff"})).unwrap()["layer"].as_u64().unwrap();
    s.execute("layer.timing", json!({"layers": [l], "in": 1.01, "out": 3.3333, "delta": 0.02})).unwrap();
    let ly = s.active_comp().unwrap().layers[0].clone();
    let fr = s.active_comp().unwrap().frame_rate;
    for t in [ly.in_point, ly.out_point, ly.start_time] {
        assert!(aligned(fr, t), "{t:?}");
    }
    s.execute("prop.addKey", json!({"layer": l, "path": "transform/opacity", "time": 1.2345, "value": 10})).unwrap();
    let k = prop(&s, l, "transform/opacity").keys[0].time;
    assert!(aligned(fr, ly.comp_time(k)));
    // Layer shifted one frame (delta 0.02 s rounds to 1 frame): 1.2345 s + 1 frame -> frame 38.
    assert_eq!(fr.frame_at(ly.comp_time(k)), 38);
}

#[test]
fn time_set_snaps_to_the_nearest_frame_and_ae_timecode() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "T", "width": 100, "height": 100, "frameRate": 29.97, "duration": 10})).unwrap();
    let r = s.execute("time.set", json!({"time": 2.5})).unwrap();
    assert_eq!(r["frame"], 75);
    let fr = s.active_comp().unwrap().frame_rate;
    assert_eq!(effectcraft_time::format_timecode_ae(75, fr, false), "0:00:02:15");
    assert_eq!(effectcraft_time::format_timecode_ae(75, fr, true), "0;00;02;15");
}

/// #890: at 29.97 / 59.94 fps the displayed (non-drop) timecode was read back as drop-frame, so
/// committing the current-time field moved the playhead to another frame from minute 1 on.
#[test]
fn time_set_round_trips_the_displayed_timecode_at_drop_frame_rates() {
    for fps in [29.97, 59.94] {
        let mut s = Session::default();
        s.execute("comp.new", json!({"name": "T", "width": 64, "height": 64, "frameRate": fps, "duration": 4000})).unwrap();
        for frame in [0_i64, 1799, 1800, 1801, 3600, 17982, 17983, 100_000] {
            let r = s.execute("time.set", json!({"frame": frame})).unwrap();
            let display = r["display"].as_str().unwrap().to_string();
            let r = s.execute("time.set", json!({"timecode": display})).unwrap();
            assert_eq!(r["frame"], frame, "{fps} fps: displayed {display}");
        }
        // Explicit drop-frame fields still mean drop-frame.
        let r = s.execute("time.set", json!({"timecode": "0;01;00;02"})).unwrap();
        assert_eq!(r["frame"], if fps > 30.0 { 3598 } else { 1800 });
    }
}

#[test]
fn time_set_keeps_the_frame_shown_in_the_current_time_field() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Example", "width": 64, "height": 64, "frameRate": 30, "duration": 0.6, "startTimecode": "0:00:00:05"})).unwrap();
    let r = s.execute("time.set", json!({"frame": 0})).unwrap();
    assert_eq!(r["display"], "0:00:00:05");
    let r = s.execute("time.set", json!({"timecode": "0:00:00:05"})).unwrap();
    assert_eq!((r["frame"].as_i64(), r["time"].as_f64()), (Some(0), Some(0.0)));

    s.execute("file.projectSettings", json!({"timeDisplay": "frames"})).unwrap();
    std::sync::Arc::make_mut(&mut s.project).settings.frame_start = 1;
    let r = s.execute("time.set", json!({"frame": 12})).unwrap();
    assert_eq!(r["display"], "00013");
    let r = s.execute("time.set", json!({"timecode": "00013"})).unwrap();
    assert_eq!((r["frame"].as_i64(), r["time"].as_f64()), (Some(12), Some(0.4)));
    let r = s.execute("time.set", json!({"timecode": "+1"})).unwrap();
    assert_eq!(r["frame"], 13);
}

/// Frames display style: typed digits are a frame count, so 00100 is frame 100, not 1 s (#530).
#[test]
fn time_set_reads_frames_style_digits_as_a_frame_count() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Long", "width": 64, "height": 64, "frameRate": 30, "duration": 10})).unwrap();
    s.execute("file.projectSettings", json!({"timeDisplay": "frames"})).unwrap();
    let r = s.execute("time.set", json!({"timecode": "00100"})).unwrap();
    assert_eq!(r["frame"], 100);
    assert_eq!(r["display"], "00100");
}

/// Project Settings ▸ Time Display Style ▸ Frames ▸ Frame Count: Start at 0 / Start at 1
/// (`app.project.framesCountType`), undoable and saved with the project (#1807).
#[test]
fn project_settings_frame_count_starts_at_one() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Count", "width": 64, "height": 64, "frameRate": 30, "duration": 2})).unwrap();
    s.execute("file.projectSettings", json!({"timeDisplay": "frames"})).unwrap();
    assert_eq!(s.execute("time.set", json!({"frame": 12})).unwrap()["display"], "00012");

    s.execute("file.projectSettings", json!({"frameStart": 1})).unwrap();
    assert_eq!(s.execute("time.set", json!({"frame": 12})).unwrap()["display"], "00013");
    for bad in [json!(2), json!(-1), json!("one")] {
        assert!(s.execute("file.projectSettings", json!({"frameStart": bad})).is_err(), "{bad}");
    }
    assert_eq!(s.project.settings.frame_start, 1);

    let dir = std::env::temp_dir().join(format!("ec-frame-start-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("count.ecproj").to_string_lossy().to_string();
    s.execute("file.save", json!({"path": path})).unwrap();
    let mut reopened = Session::default();
    reopened.execute("file.open", json!({"path": path})).unwrap();
    let _ = std::fs::remove_dir_all(dir);
    assert_eq!(reopened.project.settings.frame_start, 1);

    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.execute("time.set", json!({"frame": 12})).unwrap()["display"], "00012");
}

/// A Duration is a count of frames: Start at 1 and the comp's Start Timecode number positions,
/// not lengths (#1807).
#[test]
fn durations_ignore_the_frame_count_and_the_start_timecode() {
    use crate::commands::time::display_duration;
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Len", "width": 64, "height": 64, "frameRate": 30, "duration": 10, "startTimecode": "0:00:01:00"})).unwrap();
    s.execute("file.projectSettings", json!({"frameStart": 1})).unwrap();
    let comp = s.active_comp().unwrap().clone();
    let len = comp.frame_rate.tick_of(30);
    for (style, want) in [("frames", "00030"), ("feet35", "0001+14"), ("timecode", "0:00:01:00")] {
        s.execute("file.projectSettings", json!({"timeDisplay": style})).unwrap();
        assert_eq!(display_duration(&s, &comp, len), want, "{style}");
    }
}

#[test]
fn separate_dimensions_split_and_rejoin() {
    let (mut s, l) = setup();
    animate(&mut s, l, "transform/position", &[(0.0, json!([0, 100])), (2.0, json!([200, 50]))]);
    s.execute("prop.separateDimensions", json!({"layer": l})).unwrap();
    let after = |s: &Session| {
        let x = prop(s, l, "transform/positionX");
        let y = prop(s, l, "transform/positionY");
        assert_eq!(x.keys.len(), 2);
        assert_eq!(y.keys[1].value, KV::Scalar(50.0));
        // The layer still renders at the same place (render sees the separated values).
        let cid = s.active_comp_id().unwrap();
        let comp = s.project.comp(cid).unwrap();
        let layer = &comp.layers[0];
        let ctx = effectcraft_render::EvalCtx::new(&s.project, cid, comp, effectcraft_time::Tick::from_seconds_f64(1.0));
        let p = ctx.value(layer, layer.transform().unwrap().get("position").unwrap()).as_vec3();
        assert!((p[0] - 100.0).abs() < 1e-6 && (p[1] - 75.0).abs() < 1e-6, "{p:?}");
    };
    undo_redo_roundtrip(&mut s, after, |s| assert!(s.active_comp().unwrap().layers[0].transform().unwrap().get("positionX").is_none()));
    // X and Y animate independently.
    s.execute("keys.select", json!({"keys": []})).unwrap();
    s.execute("prop.select", json!({"layer": l, "path": "transform/positionX"})).unwrap();
    s.execute("keys.easyEase", json!({})).unwrap();
    assert_eq!(prop(&s, l, "transform/positionY").keys[0].out_interp, Interp::Linear);
    // Setting Position (viewer drags) writes X/Y.
    s.execute("time.set", json!({"time": 3.0})).unwrap();
    s.execute("prop.set", json!({"layer": l, "path": "transform/position", "value": [7, 8, 0]})).unwrap();
    assert_eq!(prop(&s, l, "transform/positionX").keys.len(), 3);
    // Re-join: one Position with keys at the union of times.
    s.execute("prop.separateDimensions", json!({"layer": l})).unwrap();
    let p = prop(&s, l, "transform/position");
    assert_eq!(p.keys.len(), 3);
    assert_eq!(p.keys[2].value, KV::Vec3([7.0, 8.0, 0.0]));
    assert!(s.active_comp().unwrap().layers[0].transform().unwrap().get("positionX").is_none());
}

#[test]
fn parenting_keeps_the_layer_in_place() {
    let (mut s, child) = setup();
    let r = s.execute("layer.newNull", json!({})).unwrap();
    let par = r["layer"].as_u64().unwrap();
    s.execute("prop.set", json!({"layer": par, "path": "transform/position", "value": [300, 200]})).unwrap();
    s.execute("prop.set", json!({"layer": par, "path": "transform/rotation", "value": 90})).unwrap();
    s.execute("prop.set", json!({"layer": par, "path": "transform/scale", "value": [200, 200]})).unwrap();
    s.execute("prop.set", json!({"layer": child, "path": "transform/position", "value": [100, 50]})).unwrap();
    let world = |s: &Session| {
        let cid = s.active_comp_id().unwrap();
        let comp = s.project.comp(cid).unwrap();
        let l = comp.layer(effectcraft_project::LayerId(child)).unwrap();
        let ctx = effectcraft_render::EvalCtx::new(&s.project, cid, comp, effectcraft_time::Tick::ZERO);
        let (m, _) = ctx.layer_to_comp(l);
        let a = m.apply(effectcraft_geom::vec2(50.0, 50.0));
        let b = m.apply(effectcraft_geom::vec2(100.0, 50.0));
        (a, (b.y - a.y).atan2(b.x - a.x).to_degrees(), (b.x - a.x).hypot(b.y - a.y))
    };
    let before = world(&s);
    s.execute("layer.setParent", json!({"layers": [child], "parent": par})).unwrap();
    let after = world(&s);
    assert!((before.0.x - after.0.x).abs() < 1e-6 && (before.0.y - after.0.y).abs() < 1e-6, "{before:?} {after:?}");
    assert!((before.1 - after.1).abs() < 1e-6 && (before.2 - after.2).abs() < 1e-6);
    assert!((prop(&s, child, "transform/rotation").value.as_f64() + 90.0).abs() < 1e-9);
    // Unparenting restores comp-space values.
    s.execute("layer.setParent", json!({"layers": [child], "parent": null})).unwrap();
    let p = prop(&s, child, "transform/position").value.as_vec3();
    assert!((p[0] - 100.0).abs() < 1e-6 && (p[1] - 50.0).abs() < 1e-6, "{p:?}");
    s.execute("edit.undo", json!({})).unwrap();
    assert!(s.active_comp().unwrap().layer(effectcraft_project::LayerId(child)).unwrap().parent.is_some());
}

/// #284: toggling stopwatches with one `merge` key (dragging over them) is one undo step.
#[test]
fn stopwatch_toggles_with_a_merge_key_are_one_undo_step() {
    let (mut s, l) = setup();
    let steps = s.history.undo.len();
    for path in ["transform/position", "transform/scale", "transform/opacity"] {
        s.execute("prop.toggleAnimation", json!({"layer": l, "path": path, "value": true, "merge": "drag"})).unwrap();
    }
    assert_eq!(s.history.undo.len(), steps + 1);
    s.execute("edit.undo", json!({})).unwrap();
    assert!(["transform/position", "transform/scale", "transform/opacity"].iter().all(|p| !prop(&s, l, p).is_animated()));
}

#[test]
fn pick_whip_generates_ae_reference_expressions() {
    let (mut s, l) = setup();
    let r = s.execute("layer.newSolid", json!({"name": "Other", "color": "#00ff00"})).unwrap();
    let o = r["layer"].as_u64().unwrap();
    s.execute("effect.apply", json!({"layer": o, "effect": "Gaussian Blur"})).unwrap();
    s.execute("layer.addMask", json!({"layer": o})).unwrap();
    let cases = [
        ("transform/opacity", json!({"layer": o, "path": "transform/opacity"}), "thisComp.layer(\"Other\").transform.opacity"),
        ("transform/rotation", json!({"layer": l, "path": "transform/opacity"}), "transform.opacity"),
        ("transform/rotation", json!({"layer": o, "path": "effects/#1/blurriness"}), "thisComp.layer(\"Other\").effect(\"Gaussian Blur\")(\"Blurriness\")"),
        ("transform/opacity", json!({"layer": o, "path": "masks/#1/feather"}), "thisComp.layer(\"Other\").mask(\"Mask 1\").maskFeather[0]"),
        ("transform/scale", json!({"layer": o, "path": "transform/rotation"}), "temp = thisComp.layer(\"Other\").transform.rotation;\n[temp, temp]"),
        ("transform/position", json!({"layer": o, "path": "transform/position"}), "thisComp.layer(\"Other\").transform.position"),
    ];
    for (path, target, want) in cases {
        let got = s.execute("prop.pickWhip", json!({"layer": l, "path": path, "target": target})).unwrap();
        assert_eq!(got, json!(want));
        assert_eq!(prop(&s, l, path).expr.unwrap().text, want);
    }
    s.execute("edit.undo", json!({})).unwrap();
    assert_ne!(prop(&s, l, "transform/position").expr.map(|e| e.text), Some("thisComp.layer(\"Other\").transform.position".into()));
    assert!(s.execute("prop.pickWhip", json!({"layer": l, "path": "transform/opacity", "target": {"layer": l, "path": "transform/opacity"}})).is_err());
}

/// #1072: an Alt pick whip (`byIndex`) selects an effect's properties by their index,
/// `effect("Color Control")(1)`, its place among its group's children (nested groups too). The
/// effect keeps its name; other properties, dimensions, insertion into an expression and match
/// names (Compact English off) work as before. (`effectcraft-host`'s `pick_whip` tests check the
/// references read the right values.)
#[test]
fn pick_whip_by_index_selects_effect_properties_by_their_index() {
    let (mut s, l) = setup();
    let o = s.execute("layer.newSolid", json!({"name": "Other", "color": "#00ff00"})).unwrap()["layer"].as_u64().unwrap();
    for fx in ["Color Control", "Fractal Noise", "Point Control"] {
        s.execute("effect.apply", json!({"layer": o, "effect": fx})).unwrap();
    }
    // Fractal Noise ▸ Transform ▸ Scale, by its places in the property tree.
    let fx = s.active_comp().unwrap().layer(effectcraft_project::LayerId(o)).unwrap().props.sub("effects").unwrap().groups().nth(1).unwrap().clone();
    let group = fx.children.iter().position(|c| c.match_id() == "transform").unwrap() + 1;
    let scale = fx.sub("transform").unwrap().children.iter().position(|c| c.match_id() == "scale").unwrap() + 1;
    let whip = |s: &mut Session, path: &str, target: Value, extra: Value| {
        let mut p = json!({"layer": l, "path": path, "target": target, "byIndex": true});
        p.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        s.execute("prop.pickWhip", p).unwrap()
    };
    let other = "thisComp.layer(\"Other\")";
    let undo = s.history.undo.len();
    assert_eq!(
        whip(&mut s, "transform/rotation", json!({"layer": o, "path": "effects/#1/color"}), json!({})),
        json!(format!("{other}.effect(\"Color Control\")(1)[0]")),
        "a colour into one value: its first"
    );
    assert_eq!(s.history.undo.len(), undo + 1, "one undo step");
    let nested = format!("{other}.effect(\"Fractal Noise\")({group})({scale})");
    assert_eq!(whip(&mut s, "transform/rotation", json!({"layer": o, "path": "effects/#2/transform/scale"}), json!({})), json!(nested));
    assert_eq!(
        whip(&mut s, "transform/opacity", json!({"layer": o, "path": "effects/#3/point", "dimension": 1}), json!({})),
        json!(format!("{other}.effect(\"Point Control\")(1)[1]"))
    );
    assert_eq!(
        whip(&mut s, "transform/rotation", json!({"layer": o, "path": "effects/#2/transform/scale"}), json!({"expression": "1 + ", "range": [4, 4]})),
        json!(format!("1 + {nested}")),
        "inserted at the cursor"
    );
    assert_eq!(
        whip(&mut s, "transform/opacity", json!({"layer": o, "path": "transform/opacity"}), json!({})),
        json!(format!("{other}.transform.opacity")),
        "only an effect's properties go by index"
    );
    // Match names (Compact English off): the effect by its match name, its properties by index.
    s.execute("prefs.set", json!({"key": "general.expressionPickWhipCompact", "value": false})).unwrap();
    let e = whip(&mut s, "transform/rotation", json!({"layer": o, "path": "effects/#2/transform/scale"}), json!({}));
    assert!(e.as_str().unwrap().ends_with(&format!("({group})({scale})")), "{e}");
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(prop(&s, l, "transform/rotation").expr.unwrap().text, format!("1 + {nested}"), "one undo per pick");
}

/// #407: Reset without a value gives a property its own default (a Transform property's, an
/// effect parameter's) besides removing its keyframes and expression.
#[test]
fn reset_gives_a_property_its_default() {
    let (mut s, l) = setup();
    s.execute("effect.apply", json!({"layer": l, "effect": "Gaussian Blur"})).unwrap();
    let blur = prop(&s, l, "effects/#1/blurriness").value;
    let set = [
        ("transform/anchor", json!([10, 20, 0])),
        ("transform/position", json!([10, 20, 0])),
        ("transform/scale", json!([50, 70, 100])),
        ("transform/rotation", json!(30)),
        ("transform/opacity", json!(40)),
        ("effects/#1/blurriness", json!(25)),
    ];
    for (path, v) in &set {
        s.execute("prop.set", json!({"layer": l, "path": path, "value": v})).unwrap();
    }
    animate(&mut s, l, "transform/rotation", &[(0.0, json!(10)), (1.0, json!(20))]);
    s.execute("prop.setExpression", json!({"layer": l, "path": "transform/opacity", "expression": "50"})).unwrap();
    for (path, _) in &set {
        s.execute("prop.reset", json!({"layer": l, "path": path})).unwrap();
    }
    let v = |s: &Session, path: &str| prop(s, l, path).value;
    assert_eq!(v(&s, "transform/anchor"), KV::Vec3([50.0, 50.0, 0.0]), "the solid's centre");
    assert_eq!(v(&s, "transform/position"), KV::Vec3([200.0, 150.0, 0.0]), "the comp's centre");
    assert_eq!(v(&s, "transform/scale"), KV::Vec3([100.0; 3]));
    // Animated: the default becomes the key at the current time (0 s), the 1 s key stays (#1078).
    let rot = prop(&s, l, "transform/rotation");
    assert_eq!(rot.keys.len(), 2);
    assert_eq!(rot.value_at(effectcraft_time::Tick::ZERO), KV::Scalar(0.0));
    assert_eq!(v(&s, "transform/opacity"), KV::Scalar(100.0));
    assert!(prop(&s, l, "transform/opacity").expr.is_none());
    assert_eq!(v(&s, "effects/#1/blurriness"), blur);
    // A given default still wins.
    s.execute("prop.reset", json!({"layer": l, "path": "transform/opacity", "default": 30})).unwrap();
    assert_eq!(v(&s, "transform/opacity"), KV::Scalar(30.0));
}

/// #1078: resetting an animated property keeps its keyframes and keys the default at the
/// current time, as in After Effects; undo brings the old keys back.
#[test]
fn reset_keys_the_default_on_an_animated_property() {
    let (mut s, l) = setup();
    animate(&mut s, l, "transform/scale", &[(0.0, json!([50, 50, 100])), (2.0, json!([150, 150, 100]))]);
    s.execute("time.set", json!({"time": 1})).unwrap();
    s.execute("prop.reset", json!({"layer": l, "path": "transform/scale"})).unwrap();
    let pr = prop(&s, l, "transform/scale");
    let at = |sec: f64| pr.value_at(effectcraft_time::Tick::from_seconds_f64(sec));
    assert_eq!(pr.keys.len(), 3, "the 0 s and 2 s keys stay, one is added at 1 s");
    assert_eq!(at(0.0), KV::Vec3([50.0, 50.0, 100.0]));
    assert_eq!(at(1.0), KV::Vec3([100.0; 3]));
    assert_eq!(at(2.0), KV::Vec3([150.0, 150.0, 100.0]));
    // On a key, the key takes the default.
    s.execute("time.set", json!({"time": 2})).unwrap();
    s.execute("prop.reset", json!({"layer": l, "path": "transform/scale"})).unwrap();
    let pr = prop(&s, l, "transform/scale");
    assert_eq!(pr.keys.len(), 3);
    assert_eq!(pr.value_at(effectcraft_time::Tick::from_seconds_f64(2.0)), KV::Vec3([100.0; 3]));
    s.execute("edit.undo", json!({})).unwrap();
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(prop(&s, l, "transform/scale").keys.len(), 2);
}

/// #363: dropped on one of the target's values the pick whip picks that dimension
/// (`position[0]`), as in After Effects; a multi-dimension property follows it on every axis.
#[test]
fn pick_whip_picks_one_dimension_of_the_target() {
    let (mut s, l) = setup();
    let o = s.execute("layer.newSolid", json!({"name": "Other", "color": "#00ff00"})).unwrap()["layer"].as_u64().unwrap();
    let whip = |s: &mut Session, path: &str, d: u64| {
        s.execute("prop.pickWhip", json!({"layer": l, "path": path, "target": {"layer": o, "path": "transform/position", "dimension": d}}))
    };
    let r = "thisComp.layer(\"Other\").transform.position";
    assert_eq!(whip(&mut s, "transform/rotation", 0).unwrap(), json!(format!("{r}[0]")));
    assert_eq!(whip(&mut s, "transform/scale", 1).unwrap(), json!(format!("temp = {r}[1];\n[temp, temp]")));
    assert_eq!(prop(&s, l, "transform/scale").expr.unwrap().text, format!("temp = {r}[1];\n[temp, temp]"));
    // A 2D layer's Position has two.
    assert!(whip(&mut s, "transform/rotation", 2).is_err());
}

/// #284: with the expression being edited, the pick whip puts the reference at the cursor,
/// replacing the selected characters (After Effects), in one undo step.
#[test]
fn pick_whip_inserts_into_the_expression_being_edited() {
    let (mut s, l) = setup();
    let o = s.execute("layer.newSolid", json!({"name": "Ö", "color": "#00ff00"})).unwrap()["layer"].as_u64().unwrap();
    let whip = |s: &mut Session, base: &str, range: Value| {
        let mut p = json!({"layer": l, "path": "transform/opacity", "target": {"layer": o, "path": "transform/rotation"}, "expression": base});
        if !range.is_null() {
            p["range"] = range;
        }
        s.execute("prop.pickWhip", p)
    };
    let r = "thisComp.layer(\"Ö\").transform.rotation";
    // Characters, not bytes: "é" before the cursor.
    assert_eq!(whip(&mut s, "é + 5", json!([1, 1])).unwrap(), json!(format!("é{r} + 5")));
    assert_eq!(prop(&s, l, "transform/opacity").expr.unwrap().text, format!("é{r} + 5"));
    // A selection (either way round) is replaced; no range: at the end.
    assert_eq!(whip(&mut s, "a*2+b", json!([4, 2])).unwrap(), json!(format!("a*{r}b")));
    assert_eq!(whip(&mut s, "x + ", Value::Null).unwrap(), json!(format!("x + {r}")));
    // Past the end clamps; a malformed range is an error.
    assert_eq!(whip(&mut s, "7", json!([9, 99])).unwrap(), json!(format!("7{r}")));
    assert!(whip(&mut s, "7", json!([1])).is_err());
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(prop(&s, l, "transform/opacity").expr.unwrap().text, format!("x + {r}"));
}

/// Outer comp (4 s) containing a precomp whose 10×10 white solid moves x = 10 + 20·t (t in s).
/// Returns (session, precomp layer id in the outer comp).
fn precomp_setup() -> (Session, u64) {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Inner", "width": 100, "height": 100, "frameRate": 30, "duration": 4})).unwrap();
    let r = s.execute("layer.newSolid", json!({"color": "#ffffff", "width": 10, "height": 10})).unwrap();
    let l = r["layer"].as_u64().unwrap();
    animate(&mut s, l, "transform/position", &[(0.0, json!([10, 50])), (4.0, json!([90, 50]))]);
    s.execute("comp.new", json!({"name": "Outer", "width": 100, "height": 100, "frameRate": 30, "duration": 4})).unwrap();
    let r = s.execute("layer.addItem", json!({"item": "Inner"})).unwrap();
    let pl = r["layer"].as_u64().unwrap();
    s.execute("layer.select", json!({"layers": [pl]})).unwrap();
    (s, pl)
}

/// X of the moving square's centre in the outer comp at `t`.
fn square_x(s: &Session, t: f64) -> f64 {
    let cid = s.active_comp_id().unwrap();
    let img = s.render(cid, effectcraft_time::Tick::from_seconds_f64(t), effectcraft_render::RenderOpts { scale: 1.0, ..Default::default() });
    let xs: Vec<usize> = (0..img.width as usize).filter(|x| img.get(*x as i64, 50)[3] > 0.5).collect();
    assert!(!xs.is_empty(), "nothing drawn at {t}");
    (xs[0] + xs[xs.len() - 1]) as f64 / 2.0 + 0.5
}

#[test]
fn time_remap_enable_freeze_and_disable() {
    let (mut s, l) = precomp_setup();
    assert!((square_x(&s, 1.0) - 30.0).abs() < 1.5);
    s.execute("layer.enableTimeRemap", json!({})).unwrap();
    let after = |s: &Session| {
        let p = prop(s, l, "timeRemap");
        assert_eq!(p.keys.len(), 2);
        assert_eq!(p.keys[1].value, KV::Scalar(4.0));
        // Remapping at identity shows the same frames.
        assert!((square_x(s, 1.0) - 30.0).abs() < 1.5);
    };
    undo_redo_roundtrip(&mut s, after, |s| assert!(s.active_comp().unwrap().layers[0].props.get("timeRemap").is_none()));
    // Remap the first key to show source time 3 s at comp time 0.
    s.execute("keys.set", json!({"layer": l, "path": "timeRemap", "time": 0.0, "value": 3.0})).unwrap();
    assert!((square_x(&s, 0.0) - 70.0).abs() < 1.5, "{}", square_x(&s, 0.0));
    // Freeze Frame at 2 s: one hold key; every frame shows source time 2 s.
    s.execute("time.set", json!({"time": 2.0})).unwrap();
    s.execute("keys.set", json!({"layer": l, "path": "timeRemap", "time": 0.0, "value": 0.0})).unwrap();
    s.execute("layer.freezeFrame", json!({})).unwrap();
    let p = prop(&s, l, "timeRemap");
    assert_eq!(p.keys.len(), 1);
    assert_eq!(p.keys[0].out_interp, Interp::Hold);
    for t in [0.0, 1.0, 3.5] {
        assert!((square_x(&s, t) - 50.0).abs() < 1.5, "{t}: {}", square_x(&s, t));
    }
    // Ctrl+Alt+T again disables remapping.
    s.execute("layer.enableTimeRemap", json!({})).unwrap();
    assert!((square_x(&s, 1.0) - 30.0).abs() < 1.5);
    // Solids can't be remapped.
    let r = s.execute("layer.newSolid", json!({"color": "#ff0000"})).unwrap();
    assert!(s.execute("layer.enableTimeRemap", json!({"layers": [r["layer"]]})).is_err());
}

/// Edit ▸ Clear with Time Remap selected (clicking its name selects all its keys) turns time
/// remapping off instead of deleting the layer (#290); one selected key is still just a key.
#[test]
fn deleting_time_remap_turns_time_remapping_off() {
    let (mut s, l) = precomp_setup();
    s.execute("layer.enableTimeRemap", json!({})).unwrap();
    let uid = prop(&s, l, "timeRemap").uid;
    s.execute("keys.select", json!({"keys": [{"layer": l, "prop": uid, "time": 0.0}], "selectProperties": true})).unwrap();
    s.execute("edit.clear", json!({})).unwrap();
    assert_eq!(prop(&s, l, "timeRemap").keys.len(), 1, "only the selected key went");
    s.execute("prop.select", json!({"layer": l, "prop": uid})).unwrap();
    s.execute("edit.clear", json!({})).unwrap();
    let comp = s.active_comp().unwrap();
    assert_eq!(comp.layers.len(), 1, "the layer stays");
    assert!(comp.layers[0].props.get("timeRemap").is_none(), "time remapping is off");
    assert!(s.state.selected_keys.is_empty() && !s.state.selected_props.iter().any(|(_, u)| *u == uid));
    assert!((square_x(&s, 1.0) - 30.0).abs() < 1.5);
    // One undo step brings it back.
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(prop(&s, l, "timeRemap").keys.len(), 1);
}

/// U (keyframes), the animation reveal and UU (modified) take in Time Remap, which sits at the
/// top of the layer's tree rather than in a group.
#[test]
fn reveal_shortcuts_take_in_time_remap() {
    let (mut s, l) = precomp_setup();
    let revealed = |s: &mut Session, kind: &str| -> Vec<u64> {
        let r = s.execute("anim.reveal", json!({"kind": kind})).unwrap();
        r["props"].as_array().unwrap().iter().filter(|p| p["layer"] == l).filter_map(|p| p["prop"].as_u64()).collect()
    };
    for kind in ["keyframes", "animation", "modified"] {
        assert!(revealed(&mut s, kind).is_empty(), "{kind}: nothing to reveal before remapping");
    }
    s.execute("layer.enableTimeRemap", json!({})).unwrap();
    let uid = prop(&s, l, "timeRemap").uid;
    for kind in ["keyframes", "animation", "modified"] {
        assert_eq!(revealed(&mut s, kind), vec![uid], "{kind}");
    }
}

/// U (and the animation reveal) still show Time Remap after its keys are gone: enabling
/// remapping is itself the modification, so a keyless remap must not vanish from U (#990).
#[test]
fn reveal_shortcuts_take_in_keyless_time_remap() {
    let (mut s, l) = precomp_setup();
    let revealed = |s: &mut Session, kind: &str| -> Vec<u64> {
        let r = s.execute("anim.reveal", json!({"kind": kind})).unwrap();
        r["props"].as_array().unwrap().iter().filter(|p| p["layer"] == l).filter_map(|p| p["prop"].as_u64()).collect()
    };
    s.execute("layer.enableTimeRemap", json!({})).unwrap();
    let uid = prop(&s, l, "timeRemap").uid;
    assert_eq!(prop(&s, l, "timeRemap").keys.len(), 2);
    // Delete both keys one at a time; remapping stays enabled.
    for _ in 0..2 {
        let t = prop(&s, l, "timeRemap").keys[0].time.seconds();
        s.execute("keys.select", json!({"keys": [{"layer": l, "prop": uid, "time": t}]})).unwrap();
        s.execute("edit.clear", json!({})).unwrap();
    }
    let tr = prop(&s, l, "timeRemap");
    assert!(tr.keys.is_empty(), "remapping is still enabled, with no keys");
    for kind in ["keyframes", "animation", "modified"] {
        assert_eq!(revealed(&mut s, kind), vec![uid], "{kind}");
    }
}

/// UU on a layer nothing was changed on shows nothing, whatever its kind: cameras, lights, text
/// and nulls have their own Transform defaults, and Separate Dimensions keeps the comp centre.
#[test]
fn modified_reveal_is_empty_for_untouched_layers() {
    let (mut s, solid) = setup();
    let mut layers = vec![("solid", solid)];
    for (cmd, params) in [
        ("layer.newText", json!({"text": "Hi"})),
        ("layer.newShape", json!({})),
        ("layer.newNull", json!({})),
        ("layer.newCamera", json!({})),
        ("layer.newLight", json!({})),
        ("layer.newLight", json!({"type": "ambient"})),
    ] {
        let r = s.execute(cmd, params.clone()).unwrap_or_else(|e| panic!("{cmd} {params}: {e}"));
        layers.push((cmd, r["layer"].as_u64().unwrap()));
    }
    s.execute("prop.separateDimensions", json!({"layer": solid})).unwrap();
    for (what, l) in layers {
        s.execute("layer.select", json!({"layers": [l]})).unwrap();
        let r = s.execute("anim.reveal", json!({"kind": "modified"})).unwrap();
        let paths: Vec<String> = r["props"].as_array().unwrap().iter().filter_map(|p| p["prop"].as_u64()).map(|u| path_of(&s, l, u)).collect();
        assert!(paths.is_empty(), "{what}: {paths:?}");
    }
}

/// Match-id path of property or group `uid` of `layer`.
fn path_of(s: &Session, layer: u64, uid: u64) -> String {
    let l = s.active_comp().unwrap().layer(effectcraft_project::LayerId(layer)).unwrap();
    let mut out = String::new();
    for n in l.props.node_chain(uid).unwrap_or_default() {
        out.push('/');
        out.push_str(n.match_id());
    }
    out
}

/// UU takes in static changes outside Transform: Material, Camera and Light Options, a
/// separated position, and layer styles in full.
#[test]
fn modified_reveal_takes_in_options_and_layer_styles() {
    let (mut s, solid) = setup();
    let modified = |s: &mut Session, l: u64| -> Vec<String> {
        s.execute("layer.select", json!({"layers": [l]})).unwrap();
        let r = s.execute("anim.reveal", json!({"kind": "modified"})).unwrap();
        r["props"].as_array().unwrap().iter().filter_map(|p| p["prop"].as_u64()).map(|u| path_of(s, l, u)).collect()
    };
    s.execute("prop.set", json!({"layer": solid, "path": "materialOptions/diffuse", "value": 80})).unwrap();
    assert_eq!(modified(&mut s, solid), ["/materialOptions/diffuse"]);
    s.execute("prop.separateDimensions", json!({"layer": solid})).unwrap();
    s.execute("prop.set", json!({"layer": solid, "path": "transform/positionX", "value": 10})).unwrap();
    assert_eq!(modified(&mut s, solid), ["/transform/positionX", "/materialOptions/diffuse"]);
    s.execute("layer.style.dropShadow", json!({"layers": [solid]})).unwrap();
    assert!(modified(&mut s, solid).contains(&"/layerStyles/dropShadow".to_string()), "{:?}", modified(&mut s, solid));

    let cam = s.execute("layer.newCamera", json!({})).unwrap()["layer"].as_u64().unwrap();
    s.execute("prop.set", json!({"layer": cam, "path": "cameraOptions/blurLevel", "value": 50})).unwrap();
    s.execute("prop.set", json!({"layer": cam, "path": "transform/rotation", "value": 15})).unwrap();
    assert_eq!(modified(&mut s, cam), ["/transform/rotation", "/cameraOptions/blurLevel"]);
    let light = s.execute("layer.newLight", json!({})).unwrap()["layer"].as_u64().unwrap();
    s.execute("prop.set", json!({"layer": light, "path": "lightOptions/intensity", "value": 40})).unwrap();
    assert_eq!(modified(&mut s, light), ["/lightOptions/intensity"]);
}

/// UU shows the parameters of an effect that differ from a new instance's, not the whole effect:
/// a just-applied effect shows nothing, and a reset parameter goes again.
#[test]
fn modified_reveal_shows_changed_effect_parameters_only() {
    let (mut s, l) = setup();
    s.execute("layer.select", json!({"layers": [l]})).unwrap();
    s.execute("effect.apply", json!({"effect": "ec.blur.gaussian"})).unwrap();
    let modified = |s: &mut Session| -> Vec<String> {
        let r = s.execute("anim.reveal", json!({"kind": "modified"})).unwrap();
        r["props"].as_array().unwrap().iter().filter_map(|p| p["prop"].as_u64()).map(|u| path_of(s, l, u)).collect()
    };
    assert!(modified(&mut s).is_empty(), "{:?}", modified(&mut s));
    s.execute("prop.set", json!({"layer": l, "path": "effects/#1/blurriness", "value": 12})).unwrap();
    assert_eq!(modified(&mut s), ["/effects/ec.blur.gaussian/blurriness"]);
    // Keyframed at its default value: animated, so still shown.
    s.execute("prop.set", json!({"layer": l, "path": "effects/#1/blurriness", "value": 0})).unwrap();
    assert!(modified(&mut s).is_empty(), "{:?}", modified(&mut s));
    s.execute("prop.toggleAnimation", json!({"layer": l, "path": "effects/#1/blurriness"})).unwrap();
    assert_eq!(modified(&mut s), ["/effects/ec.blur.gaussian/blurriness"]);
}

/// Enable Time Remapping on a layer extended past its source's end (its last frame frozen) puts
/// the end key at the source's end, not at the extended out point (#290).
#[test]
fn time_remap_on_an_extended_layer_ends_at_the_source_end() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Inner", "width": 100, "height": 100, "frameRate": 30, "duration": 2})).unwrap();
    s.execute("comp.new", json!({"name": "Outer", "width": 100, "height": 100, "frameRate": 30, "duration": 4})).unwrap();
    let l = s.execute("layer.addItem", json!({"item": "Inner"})).unwrap()["layer"].as_u64().unwrap();
    s.execute("layer.timing", json!({"layers": [l], "out": 4.0})).unwrap();
    s.execute("layer.enableTimeRemap", json!({"layers": [l]})).unwrap();
    let p = prop(&s, l, "timeRemap");
    let keys: Vec<(f64, KV)> = p.keys.iter().map(|k| (k.time.seconds(), k.value.clone())).collect();
    assert_eq!(keys, vec![(0.0, KV::Scalar(0.0)), (2.0, KV::Scalar(2.0))]);
    assert_eq!(s.active_comp().unwrap().layers[0].out_point.seconds(), 4.0, "the layer stays extended");
}

#[test]
fn freeze_on_last_frame_extends_the_layer() {
    let (mut s, l) = precomp_setup();
    s.execute("layer.timing", json!({"layers": [l], "out": 2.0})).unwrap();
    s.execute("layer.freezeOnLastFrame", json!({})).unwrap();
    let layer = s.active_comp().unwrap().layers[0].clone();
    assert_eq!(layer.out_point, s.active_comp().unwrap().duration);
    // Plays normally, then holds the frame before the old out point.
    assert!((square_x(&s, 1.0) - 30.0).abs() < 1.5);
    let hold = 10.0 + 20.0 * (2.0 - 1.0 / 30.0);
    assert!((square_x(&s, 3.5) - hold).abs() < 1.5, "{}", square_x(&s, 3.5));
}

#[test]
fn time_reverse_layer_plays_backwards_and_round_trips() {
    let (mut s, l) = precomp_setup();
    let before = s.active_comp().unwrap().layers[0].clone();
    s.execute("layer.timeReverse", json!({})).unwrap();
    let after = |s: &Session| {
        let ly = &s.active_comp().unwrap().layers[0];
        assert_eq!(ly.stretch, -100.0);
        assert_eq!((ly.in_point, ly.out_point), (before.in_point, before.out_point));
        // First frame shows the last source frame; the end shows the start.
        let last = 10.0 + 20.0 * (4.0 - 1.0 / 30.0);
        assert!((square_x(s, 0.0) - last).abs() < 1.5, "{}", square_x(s, 0.0));
        assert!((square_x(s, 4.0 - 1.0 / 30.0) - 10.0).abs() < 1.5);
    };
    undo_redo_roundtrip(&mut s, after, |s| assert_eq!(s.active_comp().unwrap().layers[0].stretch, 100.0));
    // Reversing twice restores normal playback.
    s.execute("layer.timeReverse", json!({"layers": [l]})).unwrap();
    let ly = &s.active_comp().unwrap().layers[0];
    assert_eq!(ly.stretch, 100.0);
    assert_eq!(ly.start_time, before.start_time);
    assert!((square_x(&s, 1.0) - 30.0).abs() < 1.5);
}

#[test]
fn time_stretch_holds_in_current_or_out() {
    let (mut s, l) = precomp_setup();
    s.execute("layer.timeStretch", json!({"percent": 50})).unwrap();
    let ly = s.active_comp().unwrap().layers[0].clone();
    assert_eq!(ly.stretch, 50.0);
    assert!((ly.out_point.seconds() - 2.0).abs() < 1e-6);
    // Twice as fast: comp 1 s shows source 2 s.
    assert!((square_x(&s, 1.0) - 50.0).abs() < 1.5);
    s.execute("edit.undo", json!({})).unwrap();
    // Hold the current frame (2 s) in place while doubling the duration.
    s.execute("time.set", json!({"time": 2.0})).unwrap();
    s.execute("layer.timeStretch", json!({"layers": [l], "duration": 8.0, "hold": "current"})).unwrap();
    let ly = s.active_comp().unwrap().layers[0].clone();
    assert!((ly.stretch - 200.0).abs() < 1e-6);
    assert!((ly.in_point.seconds() + 2.0).abs() < 1e-6, "{}", ly.in_point.seconds());
    assert!((square_x(&s, 2.0) - 50.0).abs() < 1.5);
    // Hold the out point.
    s.execute("edit.undo", json!({})).unwrap();
    s.execute("layer.timeStretch", json!({"layers": [l], "percent": 50, "hold": "out"})).unwrap();
    let ly = s.active_comp().unwrap().layers[0].clone();
    assert!((ly.out_point.seconds() - 4.0).abs() < 1e-6 && (ly.in_point.seconds() - 2.0).abs() < 1e-6);
    assert!(s.execute("layer.timeStretch", json!({"layers": [l], "percent": 0})).is_err());
}

#[test]
fn arrange_above_moves_layers_to_a_place_in_the_stack() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Stack", "width": 100, "height": 100, "duration": 2})).unwrap();
    for n in ["D", "C", "B", "A"] {
        s.execute("layer.newSolid", json!({"name": n, "width": 10, "height": 10})).unwrap();
    }
    let names = |s: &Session| s.active_comp().unwrap().layers.iter().map(|l| l.name.clone()).collect::<Vec<_>>();
    assert_eq!(names(&s), ["A", "B", "C", "D"]);
    // A below C (one undo step), then B and D together above A, then C to the bottom.
    s.execute("layer.arrange", json!({"layers": ["A"], "above": "D"})).unwrap();
    assert_eq!(names(&s), ["B", "C", "A", "D"]);
    s.execute("layer.arrange", json!({"layers": ["B", "D"], "above": "A"})).unwrap();
    assert_eq!(names(&s), ["C", "B", "D", "A"]);
    s.execute("layer.arrange", json!({"layers": ["C"], "to": "back"})).unwrap();
    assert_eq!(names(&s), ["B", "D", "A", "C"]);
    s.undo();
    assert_eq!(names(&s), ["C", "B", "D", "A"]);
    // A layer can't go above itself; unknown targets fail.
    assert!(s.execute("layer.arrange", json!({"layers": ["B"], "above": "B"})).is_err());
    assert!(s.execute("layer.arrange", json!({"layers": ["B"], "above": "Nope"})).is_err());
}

#[test]
fn toggle_transform_key_covers_selected_layers_separated_position_and_3d_rotation() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "K", "width": 100, "height": 100, "duration": 2})).unwrap();
    let a = s.execute("layer.newSolid", json!({"name": "A", "width": 10, "height": 10})).unwrap()["layer"].as_u64().unwrap();
    let b = s.execute("layer.newSolid", json!({"name": "B", "width": 10, "height": 10})).unwrap()["layer"].as_u64().unwrap();
    s.execute("layer.select", json!({"layers": [a, b]})).unwrap();
    let keys = |s: &Session, l: u64, path: &str| prop(s, l, path).keys.len();
    // Both selected layers get an Opacity key, in one undo step; again removes them.
    let steps = s.history.undo.len();
    s.execute("keys.toggleTransform", json!({"prop": "opacity"})).unwrap();
    assert_eq!((keys(&s, a, "transform/opacity"), keys(&s, b, "transform/opacity")), (1, 1));
    assert_eq!(s.history.undo.len(), steps + 1);
    s.execute("keys.toggleTransform", json!({"prop": "opacity"})).unwrap();
    assert_eq!(keys(&s, a, "transform/opacity"), 0);
    // A 3D layer's Rotation key covers Orientation and X/Y/Z Rotation.
    s.execute("layer.setSwitch", json!({"layers": [a], "switch": "threeD", "value": true})).unwrap();
    s.execute("keys.toggleTransform", json!({"layers": [a], "prop": "rotation"})).unwrap();
    for p in ["orientation", "rotationX", "rotationY", "rotation"] {
        assert_eq!(keys(&s, a, &format!("transform/{p}")), 1, "{p}");
    }
    assert!(s.execute("keys.toggleTransform", json!({"prop": "skew"})).is_err());
}

/// #355, #491: with several layers selected, the stopwatch, the keyframe button and value edits
/// (`selected: true`, as the panels send them) apply to every selected layer in one undo step; a
/// scrub (`offset`) moves each layer by the same amount, a typed value sets them all.
#[test]
fn property_edits_apply_to_every_selected_layer() {
    let (mut s, a) = setup();
    let b = s.execute("layer.newSolid", json!({"color": "#ff0000", "width": 50, "height": 50})).unwrap()["layer"].as_u64().unwrap();
    let c = s.execute("layer.newSolid", json!({"color": "#00ff00", "width": 50, "height": 50})).unwrap()["layer"].as_u64().unwrap();
    s.execute("prop.set", json!({"layer": b, "path": "transform/position", "value": [10, 20]})).unwrap();
    s.execute("layer.select", json!({"layers": [a, b]})).unwrap();
    let pos = |s: &Session, l: u64| prop(s, l, "transform/position").value_at(effectcraft_time::Tick::ZERO).components();
    let uid = prop(&s, a, "transform/position").uid;

    // Stopwatch on all selected layers, one undo step; the unselected layer is untouched.
    let undo = s.history.undo.len();
    s.execute("prop.toggleAnimation", json!({"layer": a, "prop": uid, "selected": true})).unwrap();
    assert!(prop(&s, a, "transform/position").is_animated() && prop(&s, b, "transform/position").is_animated());
    assert!(!prop(&s, c, "transform/position").is_animated());
    assert_eq!(s.history.undo.len(), undo + 1);
    s.execute("edit.undo", json!({})).unwrap();
    assert!(!prop(&s, a, "transform/position").is_animated() && !prop(&s, b, "transform/position").is_animated());

    // A scrub moves both by the same change (merged into one step while it continues).
    let (pa, pb) = (pos(&s, a), pos(&s, b));
    for k in 1..=3 {
        let v = [pa[0] + 5.0 * k as f64, pa[1]];
        s.execute("prop.set", json!({"layer": a, "prop": uid, "value": v, "selected": true, "offset": true, "merge": "scrub-1"})).unwrap();
    }
    assert_eq!(pos(&s, a)[..2], [pa[0] + 15.0, pa[1]]);
    assert_eq!(pos(&s, b)[..2], [pb[0] + 15.0, pb[1]]);
    assert_eq!(s.history.undo.len(), undo + 1, "one undo step for the scrub");
    // A typed value sets them all.
    s.execute("prop.set", json!({"layer": a, "path": "transform/opacity", "value": 40, "selected": true})).unwrap();
    assert_eq!(prop(&s, b, "transform/opacity").value.as_f64(), 40.0);
    assert_eq!(prop(&s, c, "transform/opacity").value.as_f64(), 100.0);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(prop(&s, a, "transform/opacity").value.as_f64(), 100.0);
    assert_eq!(prop(&s, b, "transform/opacity").value.as_f64(), 100.0);

    // The keyframe button adds a key on each selected layer.
    s.execute("prop.toggleKey", json!({"layer": a, "path": "transform/scale", "selected": true})).unwrap();
    assert_eq!(prop(&s, a, "transform/scale").keys.len(), 1);
    assert_eq!(prop(&s, b, "transform/scale").keys.len(), 1);
    // Without `selected` only the given layer changes.
    s.execute("prop.set", json!({"layer": a, "path": "transform/rotation", "value": 30})).unwrap();
    assert_eq!(prop(&s, b, "transform/rotation").value.as_f64(), 0.0);
}

/// #770: Proportional Scrubbing (After Effects 26.2, Ctrl+Alt / Cmd+Opt scrub). The change is
/// spread over the selected layers by selection order (not stacking order): the first keeps its
/// value, the last takes all of it, the one between half; whichever layer is scrubbed. One undo
/// step; a keyframed property gets a key at the current time, the unchanged first layer none.
#[test]
fn proportional_scrub_spreads_the_change_by_selection_order() {
    let (mut s, a) = setup();
    let b = s.execute("layer.newSolid", json!({"color": "#ff0000", "width": 50, "height": 50})).unwrap()["layer"].as_u64().unwrap();
    let c = s.execute("layer.newSolid", json!({"color": "#00ff00", "width": 50, "height": 50})).unwrap()["layer"].as_u64().unwrap();
    s.execute("prop.set", json!({"layer": b, "path": "transform/rotation", "value": 20})).unwrap();
    // Selection order c, a, b.
    s.execute("layer.select", json!({"layers": [c]})).unwrap();
    s.execute("layer.select", json!({"layers": [a], "add": true})).unwrap();
    s.execute("layer.select", json!({"layers": [b], "toggle": true})).unwrap();
    let rot = |s: &Session, l: u64| prop(s, l, "transform/rotation").value.as_f64();
    let uid = prop(&s, a, "transform/rotation").uid;

    // Scrub layer a (the middle one) by +10 a frame, three frames: the UI sends a's shown value
    // plus the drag.
    let undo = s.history.undo.len();
    for _ in 0..3 {
        let v = rot(&s, a) + 10.0;
        s.execute("prop.set", json!({"layer": a, "prop": uid, "value": v, "selected": true, "offset": true, "proportional": true, "merge": "scrub-r"}))
            .unwrap();
    }
    assert_eq!(rot(&s, c), 0.0, "the first selected layer keeps its value");
    assert_eq!(rot(&s, a), 15.0, "the middle one moves by half");
    assert_eq!(rot(&s, b), 50.0, "the last one moves by the whole change");
    assert_eq!(s.history.undo.len(), undo + 1, "one undo step for the scrub");
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!((rot(&s, c), rot(&s, a), rot(&s, b)), (0.0, 0.0, 20.0));

    // Vectors and keyframes: Position animated on every layer, scrubbed on the last layer at 1 s.
    for l in [a, b, c] {
        s.execute("prop.set", json!({"layer": l, "path": "transform/position", "value": [100, 100], "time": 0.0})).unwrap();
        s.execute("prop.toggleAnimation", json!({"layer": l, "path": "transform/position"})).unwrap();
    }
    s.execute("time.set", json!({"time": 1.0})).unwrap();
    let pos_uid = prop(&s, b, "transform/position").uid;
    s.execute("prop.set", json!({"layer": b, "prop": pos_uid, "value": [140, 80], "selected": true, "offset": true, "proportional": true})).unwrap();
    let at1 = |s: &Session, l: u64| {
        let p = prop(s, l, "transform/position");
        (p.keys.len(), p.value_at(effectcraft_time::Tick::from_seconds_f64(1.0)).components()[..2].to_vec())
    };
    assert_eq!(at1(&s, c), (1, vec![100.0, 100.0]), "no key on the first selected layer");
    assert_eq!(at1(&s, a), (2, vec![120.0, 90.0]));
    assert_eq!(at1(&s, b), (2, vec![140.0, 80.0]));

    // Without `proportional` a scrub still moves them all by the same change.
    s.execute("prop.set", json!({"layer": a, "prop": uid, "value": 5, "selected": true, "offset": true})).unwrap();
    assert_eq!((rot(&s, c), rot(&s, a), rot(&s, b)), (5.0, 5.0, 25.0));
}

#[test]
fn proportional_share_is_linear_in_selection_order() {
    use crate::commands::prop::proportional_share;
    assert_eq!((0..5).map(|i| proportional_share(i, 5)).collect::<Vec<_>>(), [0.0, 0.25, 0.5, 0.75, 1.0]);
    // One layer (or a bad count) takes the whole change; an index past the end the last share.
    assert_eq!(proportional_share(0, 1), 1.0);
    assert_eq!(proportional_share(0, 0), 1.0);
    assert_eq!(proportional_share(usize::MAX, 3), 1.0);
}

#[test]
fn add_item_keeps_footage_longer_than_the_comp_at_full_length() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "T", "width": 400, "height": 300, "frameRate": 30, "duration": 4})).unwrap();
    let f = effectcraft_project::Footage {
        width: 100,
        height: 100,
        kind: effectcraft_project::FootageKind::Video,
        has_video: true,
        duration: effectcraft_time::Tick::from_seconds_f64(10.0),
        ..Default::default()
    };
    let item = std::sync::Arc::make_mut(&mut s.project).add_item("long.mp4", effectcraft_color::Label::None, None, effectcraft_project::ItemKind::Footage(f));
    let l = s.execute("layer.addItem", json!({"item": item.0})).unwrap()["layer"].as_u64().unwrap();
    let layer = s.active_comp().unwrap().layer(effectcraft_project::LayerId(l)).unwrap();
    assert_eq!(layer.out_point.seconds(), 10.0);
}

/// View > Go to Time's dialog sends its Frame field as a float; `time.set` used to reject it
/// with "need `time`, `frame` or `timecode`".
#[test]
fn go_to_time_accepts_a_float_frame_from_the_dialog() {
    let (mut s, _) = setup();
    let r = s.execute("time.set", json!({"frame": 12.0})).unwrap();
    assert_eq!(r["frame"], json!(12));
    let r = s.execute("time.set", json!({"frame": 7})).unwrap();
    assert_eq!(r["frame"], json!(7));
}

/// Dragging a trim edge with several layers selected trims every selected layer by the same
/// amount from its own in/out point (as in AE), clamped to the comp and to one frame of layer;
/// a whole merged drag is one undo step.
#[test]
fn relative_trim_moves_every_selected_layer_by_the_same_delta() {
    let (mut s, a) = setup();
    let b = s.execute("layer.newSolid", json!({"color": "#ff0000", "width": 100, "height": 100})).unwrap()["layer"].as_u64().unwrap();
    s.execute("layer.timing", json!({"layers": [a], "in": 0.5, "out": 2.0})).unwrap();
    s.execute("layer.timing", json!({"layers": [b], "in": 1.0, "out": 3.5})).unwrap();
    let io = |s: &Session, l: u64| {
        let l = s.active_comp().unwrap().layer(effectcraft_project::LayerId(l)).unwrap();
        (l.in_point.seconds(), l.out_point.seconds())
    };
    let near = |x: f64, y: f64| (x - y).abs() < 1e-9;
    let undo_len = s.history.undo.len();
    // In-point drag in frame steps, merged into one edit.
    for _ in 0..3 {
        s.execute("layer.timing", json!({"layers": [a, b], "inDelta": 0.1, "merge": "bar-trim"})).unwrap();
    }
    assert_eq!(s.history.undo.len(), undo_len + 1);
    let ((ai, ao), (bi, bo)) = (io(&s, a), io(&s, b));
    assert!(near(ai, 0.8) && near(ao, 2.0) && near(bi, 1.3) && near(bo, 3.5), "{ai} {ao} {bi} {bo}");
    s.execute("edit.undo", json!({})).unwrap();
    assert!(near(io(&s, a).0, 0.5) && near(io(&s, b).0, 1.0));
    s.history.merge_key = None;
    // Out-point drag: b hits the comp end (4 s) while a keeps going.
    s.execute("layer.timing", json!({"layers": [a, b], "outDelta": 1.0})).unwrap();
    assert!(near(io(&s, a).1, 3.0) && near(io(&s, b).1, 4.0), "{:?} {:?}", io(&s, a), io(&s, b));
    // In-point drag left: a stops at the comp start.
    s.execute("layer.timing", json!({"layers": [a, b], "inDelta": -0.8})).unwrap();
    assert!(near(io(&s, a).0, 0.0) && near(io(&s, b).0, 0.2), "{:?} {:?}", io(&s, a), io(&s, b));
    // Trimming past the other edge keeps one frame of layer.
    s.execute("layer.timing", json!({"layers": [a, b], "inDelta": 10.0})).unwrap();
    let fd = 1.0 / 30.0;
    assert!(near(io(&s, a).0, 3.0 - fd) && near(io(&s, b).0, 4.0 - fd), "{:?} {:?}", io(&s, a), io(&s, b));
    s.execute("layer.timing", json!({"layers": [a, b], "outDelta": -10.0})).unwrap();
    assert!(near(io(&s, a).1, 3.0) && near(io(&s, b).1, 4.0), "{:?} {:?}", io(&s, a), io(&s, b));
    // A point already outside the comp trims from where it is instead of jumping onto the edge.
    s.execute("layer.timing", json!({"layers": [a], "in": -1.0})).unwrap();
    s.execute("layer.timing", json!({"layers": [a], "inDelta": 0.1})).unwrap();
    assert!(near(io(&s, a).0, -0.9), "{:?}", io(&s, a));
}

/// #1434: with several 3D layers selected, typing a value into one component (Z Position) sets
/// that component on each of them; their other components stay their own. Before, every
/// selected layer took the edited layer's whole value and they piled up on top of it.
#[test]
fn a_typed_component_applies_only_that_component_to_the_selected_layers() {
    let (mut s, a) = setup();
    let b = s.execute("layer.newSolid", json!({"color": "#ff0000", "width": 50, "height": 50})).unwrap()["layer"].as_u64().unwrap();
    let c = s.execute("layer.newSolid", json!({"color": "#00ff00", "width": 50, "height": 50})).unwrap()["layer"].as_u64().unwrap();
    for (l, xy) in [(a, [10.0, 20.0]), (b, [110.0, 120.0]), (c, [210.0, 220.0])] {
        s.execute("layer.setSwitch", json!({"layers": [l], "switch": "threeD", "value": true})).unwrap();
        s.execute("prop.set", json!({"layer": l, "path": "transform/position", "value": [xy[0], xy[1], 5.0]})).unwrap();
    }
    s.execute("layer.select", json!({"layers": [a, b, c]})).unwrap();
    let pos = |s: &Session, l: u64| prop(s, l, "transform/position").value.components();
    let uid = prop(&s, a, "transform/position").uid;
    // The panels send the edited layer's value with the typed component, and which one it was.
    let undo = s.history.undo.len();
    s.execute_checked("prop.set", json!({"layer": a, "prop": uid, "value": [10.0, 20.0, -300.0], "selected": true, "offset": false, "dims": [2]})).unwrap();
    assert_eq!(pos(&s, a), vec![10.0, 20.0, -300.0]);
    assert_eq!(pos(&s, b), vec![110.0, 120.0, -300.0], "B keeps its X and Y");
    assert_eq!(pos(&s, c), vec![210.0, 220.0, -300.0], "C keeps its X and Y");
    assert_eq!(s.history.undo.len(), undo + 1, "one undo step");
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(pos(&s, b), vec![110.0, 120.0, 5.0]);

    // Linked Scale: both edited components go to every layer, Z stays each layer's own.
    s.execute("prop.set", json!({"layer": c, "path": "transform/scale", "value": [50.0, 50.0, 30.0]})).unwrap();
    let scale_uid = prop(&s, a, "transform/scale").uid;
    s.execute("prop.set", json!({"layer": a, "prop": scale_uid, "value": [200.0, 200.0, 100.0], "selected": true, "dims": [0, 1]})).unwrap();
    assert_eq!(prop(&s, c, "transform/scale").value.components(), vec![200.0, 200.0, 30.0]);

    // Position with Separate Dimensions on one of the other layers.
    s.execute("prop.separateDimensions", json!({"layer": b, "value": true})).unwrap();
    s.execute("prop.set", json!({"layer": a, "prop": uid, "value": [10.0, 20.0, 40.0], "selected": true, "dims": [2]})).unwrap();
    let axis = |m: &str| prop(&s, b, &format!("transform/{m}")).value.as_f64();
    assert_eq!((axis("positionX"), axis("positionY"), axis("positionZ")), (110.0, 120.0, 40.0));

    // Without `dims` (agents setting a whole value) every selected layer takes the value as is.
    s.execute("prop.set", json!({"layer": a, "prop": uid, "value": [1.0, 2.0, 3.0], "selected": true})).unwrap();
    assert_eq!(pos(&s, c), vec![1.0, 2.0, 3.0]);
}

/// #2485: linked Audio Levels typed with several layers selected: each takes the typed Left, and
/// its Right moves by its own change, so every layer keeps its own stereo balance.
#[test]
fn a_typed_component_moves_its_follow_components_by_each_layers_own_change() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Sound", "width": 64, "height": 36, "duration": 4})).unwrap();
    s.execute("comp.new", json!({"name": "Main", "width": 320, "height": 180, "duration": 4})).unwrap();
    let a = s.execute("layer.addItem", json!({"item": "Sound"})).unwrap()["layer"].as_u64().unwrap();
    let b = s.execute("layer.addItem", json!({"item": "Sound"})).unwrap()["layer"].as_u64().unwrap();
    s.execute("prop.set", json!({"layer": a, "path": "audio/levels", "value": [-6.0, -12.0]})).unwrap();
    s.execute("prop.set", json!({"layer": b, "path": "audio/levels", "value": [-20.0, -30.0]})).unwrap();
    s.execute("layer.select", json!({"layers": [a, b]})).unwrap();
    let uid = prop(&s, a, "audio/levels").uid;
    let undo = s.history.undo.len();
    s.execute_checked("prop.set", json!({"layer": a, "prop": uid, "value": [-3.0, -9.0], "selected": true, "dims": [0], "follow": [1]})).unwrap();
    assert_eq!(prop(&s, a, "audio/levels").value.components(), vec![-3.0, -9.0]);
    assert_eq!(prop(&s, b, "audio/levels").value.components(), vec![-3.0, -13.0], "B rises 17 dB on both channels");
    assert_eq!(s.history.undo.len(), undo + 1, "one undo step");

    // Keyed at a time other than the CTI: B's change is measured on its key there.
    animate(&mut s, b, "audio/levels", &[(0.0, json!([-20.0, -30.0])), (1.0, json!([-10.0, -40.0]))]);
    s.execute("prop.set", json!({"layer": a, "prop": uid, "value": [-4.0, -10.0], "time": 1.0, "selected": true, "dims": [0], "follow": [1]})).unwrap();
    let keys: Vec<Vec<f64>> = prop(&s, b, "audio/levels").keys.iter().map(|k| k.value.components()).collect();
    assert_eq!(keys, vec![vec![-20.0, -30.0], vec![-4.0, -34.0]], "B's 1 s key rises 6 dB on both channels");
}

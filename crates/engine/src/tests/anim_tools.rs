//! Wiggler, Smoother and Motion Sketch (`keys.wiggle`, `keys.smooth`, `motion.sketch`).

use effectcraft_keyframe::evaluate;
use serde_json::json;

use crate::tests::timeline::{animate, prop, setup};

fn select(s: &mut crate::Session, layer: u64, path: &str) {
    s.execute("prop.select", json!({"layer": layer, "path": path})).unwrap();
}

fn selection_command_skips_locked_keys(command: &str, path: &str, values: [serde_json::Value; 3], mut params: serde_json::Value) {
    let (mut s, locked) = setup();
    let unlocked = s.execute("layer.newSolid", json!({"name": "Editable", "width": 100, "height": 100})).unwrap()["layer"].as_u64().unwrap();
    for layer in [locked, unlocked] {
        animate(&mut s, layer, path, &[(0.0, values[0].clone()), (0.5, values[1].clone()), (1.0, values[2].clone())]);
    }
    let before = prop(&s, locked, path).keys;
    let selected: Vec<_> = [locked, unlocked]
        .into_iter()
        .flat_map(|layer| {
            let pr = prop(&s, layer, path);
            pr.keys.into_iter().map(move |key| crate::KeyRef { layer: effectcraft_project::LayerId(layer), prop: pr.uid, time: key.time })
        })
        .collect();
    s.execute("layer.setSwitch", json!({"layers": [locked], "switch": "locked", "value": true})).unwrap();
    s.state.selected_keys = selected.clone();
    if params.get("keys").is_some() {
        params["keys"] = serde_json::to_value(&selected).unwrap();
    }
    s.execute_checked(command, params.clone()).unwrap();
    assert_eq!(prop(&s, locked, path).keys, before, "{command} changed a locked layer");
    let after = prop(&s, unlocked, path).keys;
    assert_ne!(after, before, "{command} must still edit unlocked keys");
    let after_selection = s.state.selected_keys.clone();
    assert!(selected.iter().filter(|k| k.layer.0 == locked).all(|key| after_selection.contains(key)));
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(prop(&s, unlocked, path).keys, before);
    assert!(selected.iter().filter(|key| key.layer.0 == locked).all(|key| s.state.selected_keys.contains(key)));
    s.execute("edit.redo", json!({})).unwrap();
    assert_eq!(prop(&s, unlocked, path).keys, after);
    assert_eq!(s.state.selected_keys, after_selection);
    // All-locked input must preserve the selection and document as well.
    s.state.selected_keys.retain(|k| k.layer.0 == locked);
    if params.get("keys").is_some() {
        params["keys"] = serde_json::to_value(&s.state.selected_keys).unwrap();
    }
    let locked_selection = s.state.selected_keys.clone();
    let project = serde_json::to_value(&*s.project).unwrap();
    let _ = s.execute_checked(command, params);
    assert_eq!(serde_json::to_value(&*s.project).unwrap(), project);
    assert_eq!(s.state.selected_keys, locked_selection);
}

#[test]
fn wiggler_preserves_locked_keys_in_mixed_selection() {
    selection_command_skips_locked_keys(
        "keys.wiggle",
        "transform/opacity",
        [json!(10), json!(90), json!(20)],
        json!({"apply": "temporal", "frequency": 8, "magnitude": 10}),
    );
}

#[test]
fn smoother_preserves_locked_keys_in_mixed_selection() {
    selection_command_skips_locked_keys("keys.smooth", "transform/opacity", [json!(10), json!(50), json!(90)], json!({"tolerance": 1}));
}

#[test]
fn exponential_scale_preserves_locked_keys_in_mixed_selection() {
    selection_command_skips_locked_keys(
        "keys.exponentialScale",
        "transform/scale",
        [json!([100, 100, 100]), json!([150, 150, 150]), json!([400, 400, 400])],
        json!({}),
    );
}

#[test]
fn edit_label_preserves_locked_keys_in_mixed_selection() {
    selection_command_skips_locked_keys("edit.label", "transform/opacity", [json!(10), json!(50), json!(90)], json!({"label": "Red"}));
    selection_command_skips_locked_keys("edit.label", "transform/opacity", [json!(10), json!(50), json!(90)], json!({"label": "Red", "keys": true}));
}

#[test]
fn wiggle_adds_keys_at_frequency_within_magnitude() {
    let (mut s, l) = setup();
    animate(&mut s, l, "transform/position", &[(0.0, json!([100, 100])), (2.0, json!([300, 100]))]);
    let orig = prop(&s, l, "transform/position");
    select(&mut s, l, "transform/position");
    let opts = json!({"apply": "spatial", "noise": "jagged", "dimensions": "independent", "frequency": 5, "magnitude": 20, "seed": 7});
    let r = s.execute("keys.wiggle", opts.clone()).unwrap();
    let p = prop(&s, l, "transform/position");
    // 2 s at 5 keys/s: 9 keys between the two originals (one every 6 frames at 30 fps).
    assert_eq!(p.keys.len(), 11, "{r}");
    assert_eq!(r["after"], 11);
    let step = p.keys[2].time - p.keys[1].time;
    assert!((step.seconds() - 0.2).abs() < 1e-9);
    for k in &p.keys[1..10] {
        let base = evaluate(&orig.keys, k.time, true).unwrap().as_vec2();
        let v = k.value.as_vec2();
        assert!((v[0] - base[0]).abs() <= 20.0 + 1e-9 && (v[1] - base[1]).abs() <= 20.0 + 1e-9, "{v:?} vs {base:?}");
    }
    assert!(p.keys[1..10].iter().any(|k| (k.value.as_vec2()[1] - 100.0).abs() > 1.0), "noise was applied");
    // End keys untouched; deterministic for the same seed; undo restores.
    assert_eq!(p.keys[0].value, orig.keys[0].value);
    assert_eq!(p.keys[10].value, orig.keys[1].value);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(prop(&s, l, "transform/position").keys.len(), 2);
    select(&mut s, l, "transform/position");
    s.execute("keys.wiggle", opts).unwrap();
    assert_eq!(prop(&s, l, "transform/position").keys, p.keys);
}

#[test]
fn wiggle_one_dimension_and_smooth_noise() {
    let (mut s, l) = setup();
    animate(&mut s, l, "transform/position", &[(0.0, json!([100, 100])), (1.0, json!([100, 100]))]);
    select(&mut s, l, "transform/position");
    s.execute("keys.wiggle", json!({"dimensions": "one", "dimension": 1, "frequency": 10, "magnitude": 50})).unwrap();
    let p = prop(&s, l, "transform/position");
    assert_eq!(p.keys.len(), 11);
    for k in &p.keys {
        let v = k.value.as_vec2();
        assert!((v[0] - 100.0).abs() < 1e-9);
        assert!((v[1] - 100.0).abs() <= 50.0);
    }
    // Opacity (non-spatial): the temporal path deviates the value.
    animate(&mut s, l, "transform/opacity", &[(0.0, json!(50)), (1.0, json!(50))]);
    select(&mut s, l, "transform/opacity");
    s.execute("keys.wiggle", json!({"apply": "temporal", "frequency": 4, "magnitude": 10})).unwrap();
    let o = prop(&s, l, "transform/opacity");
    assert!(o.keys.len() >= 5);
    assert!(o.keys.iter().all(|k| (k.value.as_f64() - 50.0).abs() <= 10.0));
    assert!(s.execute("keys.wiggle", json!({"noise": "wobbly"})).is_err());
}

#[test]
fn smoother_reduces_keys_within_tolerance() {
    let (mut s, l) = setup();
    // A ramp keyed every frame with a little jitter.
    let keys: Vec<(f64, serde_json::Value)> = (0..=60).map(|f| (f as f64 / 30.0, json!(f as f64 * 1.5 + if f % 2 == 0 { 0.2 } else { -0.2 }))).collect();
    animate(&mut s, l, "transform/opacity", &keys);
    let orig = prop(&s, l, "transform/opacity");
    assert_eq!(orig.keys.len(), 61);
    select(&mut s, l, "transform/opacity");
    let r = s.execute("keys.smooth", json!({"tolerance": 1.0})).unwrap();
    let p = prop(&s, l, "transform/opacity");
    assert!(p.keys.len() < 10, "{} keys ({r})", p.keys.len());
    for f in 0..=60 {
        let t = s.active_comp().unwrap().frame_rate.tick_of(f);
        let a = evaluate(&orig.keys, t, false).unwrap().as_f64();
        let b = evaluate(&p.keys, t, false).unwrap().as_f64();
        assert!((a - b).abs() <= 1.0 + 1e-6, "frame {f}: {a} vs {b}");
    }
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(prop(&s, l, "transform/opacity").keys.len(), 61);
}

#[test]
fn motion_sketch_records_position_keys() {
    let (mut s, l) = setup();
    // 1 s of capture: a straight line from (0,0) to (300,150), sampled every 0.1 s.
    let pts: Vec<serde_json::Value> = (0..=10).map(|i| json!([i as f64 * 0.1, i as f64 * 30.0, i as f64 * 15.0])).collect();
    let r = s.execute("motion.sketch", json!({"layer": l, "points": pts, "start": 0.5, "smoothing": 0})).unwrap();
    let p = prop(&s, l, "transform/position");
    assert_eq!(r["keys"], 31);
    assert_eq!(p.keys.len(), 31);
    assert_eq!(p.keys[0].time.seconds(), 0.5);
    assert_eq!(p.keys[0].value.as_vec2(), [0.0, 0.0]);
    let last = p.keys[30].value.as_vec2();
    assert!((last[0] - 300.0).abs() < 1e-6 && (last[1] - 150.0).abs() < 1e-6);
    // Smoothing collapses the straight line to (almost) its end points; 200 % capture speed
    // plays back over twice the time.
    s.execute("edit.undo", json!({})).unwrap();
    s.execute("motion.sketch", json!({"layer": l, "points": pts, "start": 0, "smoothing": 2, "captureSpeed": 200})).unwrap();
    let p = prop(&s, l, "transform/position");
    assert!(p.keys.len() <= 4, "{}", p.keys.len());
    assert!((p.keys.last().unwrap().time.seconds() - 2.0).abs() < 1e-9);
    // [x, y] points: one per frame.
    s.execute("motion.sketch", json!({"layer": l, "points": [[0, 0], [10, 0], [20, 0]], "start": 3, "smoothing": 0})).unwrap();
    let p = prop(&s, l, "transform/position");
    let t = s.active_comp().unwrap().frame_rate.tick_of(92);
    assert!(p.keys.iter().any(|k| k.time == t && k.value.as_vec2() == [20.0, 0.0]));
    assert!(s.execute("motion.sketch", json!({"layer": l, "points": [[1, 2]]})).is_err());
}

//! Quick Offset (#1012): `layer.quickOffset` and `keys.quickOffset` stagger layers and each layer's
//! selected keys in selection order, onto subframes, as one undo step.

use effectcraft_keyframe::Interp;
use effectcraft_project::{Layer, LayerId};
use effectcraft_time::Tick;
use serde_json::{Value, json};

use crate::{KeyRef, Session};

/// One frame at 30 fps.
const F: i64 = 8_467_200_000;

fn setup(n: usize) -> (Session, Vec<u64>) {
    let mut s = Session::default();
    s.execute_checked("comp.new", json!({"width": 64, "height": 64, "frameRate": 30, "duration": 10})).unwrap();
    let ids =
        (0..n).map(|i| s.execute_checked("layer.newSolid", json!({"name": format!("L{i}"), "color": "#fff"})).unwrap()["layer"].as_u64().unwrap()).collect();
    (s, ids)
}

fn layer(s: &Session, id: u64) -> &Layer {
    s.active_comp().unwrap().layer(LayerId(id)).unwrap()
}

/// (start, in, out) in ticks.
fn timing(s: &Session, id: u64) -> (i64, i64, i64) {
    let l = layer(s, id);
    (l.start_time.0, l.in_point.0, l.out_point.0)
}

/// Click the first, Ctrl/Cmd-click the rest.
fn select(s: &mut Session, ids: &[u64]) {
    for (i, id) in ids.iter().enumerate() {
        s.execute_checked("layer.select", json!({"layers": [id], "add": i > 0})).unwrap();
    }
}

fn add_key(s: &mut Session, id: u64, path: &str, seconds: f64, value: Value) {
    s.execute_checked("prop.addKey", json!({"layer": id, "path": path, "time": seconds, "value": value})).unwrap();
}

fn select_keys(s: &mut Session, keys: &[(u64, &str, f64)]) {
    for (i, (id, path, t)) in keys.iter().enumerate() {
        s.execute_checked("keys.select", json!({"keys": [{"layer": id, "path": path, "time": t}], "add": i > 0})).unwrap();
    }
}

/// The stored key times (ticks) of a property.
fn key_times(s: &Session, id: u64, path: &str) -> Vec<i64> {
    layer(s, id).props.prop(path).unwrap().keys.iter().map(|k| k.time.0).collect()
}

fn uid(s: &Session, id: u64, path: &str) -> u64 {
    layer(s, id).props.prop(path).unwrap().uid
}

#[test]
fn layers_stagger_in_selection_order_onto_subframes_as_one_undo() {
    let (mut s, ids) = setup(4);
    let order = [ids[2], ids[0], ids[3], ids[1]];
    select(&mut s, &order);
    let before = s.project.clone();
    let reply = s.execute_checked("layer.quickOffset", json!({"delta": 1.0 / 30.0})).unwrap();
    // The first selected stays, the last moves the whole frame, the ones between a third each.
    for (i, id) in order.iter().enumerate() {
        let d = F * i as i64 / 3;
        let (st, inp, out) = timing(&s, *id);
        assert_eq!((st, inp, out - inp), (d, d, 10 * 30 * F), "layer {i} in selection order");
    }
    assert_eq!(reply["layers"].as_array().unwrap().len(), 4);
    // Negative drags mirror it.
    s.execute_checked("layer.quickOffset", json!({"delta": -2.0 / 30.0})).unwrap();
    for (i, id) in order.iter().enumerate() {
        assert_eq!(timing(&s, *id).0, -F * i as i64 / 3, "layer {i} after dragging back two frames");
    }
    s.execute_checked("edit.undo", json!({})).unwrap();
    s.execute_checked("edit.undo", json!({})).unwrap();
    assert!(s.project.same_content(&before), "two offsets, two undo steps");
    s.execute_checked("edit.redo", json!({})).unwrap();
    assert_eq!(timing(&s, order[3]).0, F);
}

#[test]
fn a_drag_restarts_from_the_layers_before_it_and_is_one_undo_step() {
    let (mut s, ids) = setup(3);
    select(&mut s, &ids);
    // An offset layer, a stretched one and a reversed one move their whole timing; their keys
    // (in layer time) come along untouched.
    s.execute_checked("layer.timing", json!({"layers": [ids[0]], "start": 1.0})).unwrap();
    s.execute_checked("layer.timeStretch", json!({"layers": [ids[1]], "percent": 200})).unwrap();
    s.execute_checked("layer.timeStretch", json!({"layers": [ids[2]], "op": "reverse"})).unwrap();
    add_key(&mut s, ids[2], "transform/opacity", 1.0, json!(50));
    let keys = key_times(&s, ids[2], "transform/opacity");
    let start: Vec<_> = ids.iter().map(|id| timing(&s, *id)).collect();
    let before = s.project.clone();
    for frames in [2.0, 5.0, -3.0, 2.0] {
        s.execute_checked("layer.quickOffset", json!({"delta": frames / 30.0, "merge": "qo", "fromStart": true})).unwrap();
    }
    for (i, id) in ids.iter().enumerate() {
        let d = 2 * F * i as i64 / 2;
        let (a, b, c) = start[i];
        assert_eq!(timing(&s, *id), (a + d, b + d, c + d), "layer {i}: only the last step's whole drag counts");
    }
    assert_eq!(key_times(&s, ids[2], "transform/opacity"), keys);
    s.execute_checked("edit.undo", json!({})).unwrap();
    assert!(s.project.same_content(&before), "the drag is one undo step");
}

#[test]
fn locked_layers_bad_arguments_and_out_of_range_moves_change_nothing() {
    let (mut s, ids) = setup(3);
    select(&mut s, &ids);
    // A locked layer is skipped: the other two are first and last.
    s.execute_checked("layer.setSwitch", json!({"layers": [ids[1]], "switch": "locked", "value": true})).unwrap();
    s.execute_checked("layer.quickOffset", json!({"layers": ids, "delta": 0.1})).unwrap();
    assert_eq!((timing(&s, ids[0]).0, timing(&s, ids[1]).0), (0, 0));
    assert_eq!(timing(&s, ids[2]).0, Tick::from_seconds_f64(0.1).0);
    let before = (s.project.clone(), s.revision);
    for p in [
        json!({"layers": [ids[0]], "delta": 0.1}),
        json!({"layers": [ids[0], ids[1]], "delta": 0.1}),
        json!({"layers": ids}),
        json!({"layers": ids, "delta": "far"}),
        json!({"layers": ids, "delta": 1e12}),
        json!({"layers": [ids[0], 999], "delta": 0.1}),
    ] {
        assert!(s.execute_checked("layer.quickOffset", p.clone()).is_err(), "{p}");
    }
    assert!(s.project.same_content(&before.0) && s.revision == before.1, "refused calls change nothing");
    // The last layer's destination is past the end of time: nothing moves.
    s.execute_checked("layer.timing", json!({"layers": [ids[2]], "start": 9.0e6})).unwrap();
    let before = s.project.clone();
    assert!(s.execute_checked("layer.quickOffset", json!({"layers": [ids[0], ids[2]], "delta": 1.0e5})).is_err());
    assert!(s.project.same_content(&before));
}

#[test]
fn keys_stagger_per_layer_in_the_order_their_layers_were_selected() {
    let (mut s, ids) = setup(3);
    for id in &ids {
        add_key(&mut s, *id, "transform/opacity", 0.0, json!(0));
        add_key(&mut s, *id, "transform/opacity", 1.0, json!(100));
    }
    add_key(&mut s, ids[0], "transform/position", 0.5, json!([10, 20]));
    s.execute_checked("keys.select", json!({"keys": [{"layer": ids[0], "path": "transform/opacity", "time": 1.0}]})).unwrap();
    s.execute_checked("keys.interpolation", json!({"interpolation": "hold"})).unwrap();
    // Layer 1's keys first, then layer 0's two properties, then layer 2's.
    let picks = [
        (ids[1], "transform/opacity", 0.0),
        (ids[0], "transform/opacity", 1.0),
        (ids[1], "transform/opacity", 1.0),
        (ids[0], "transform/position", 0.5),
        (ids[2], "transform/opacity", 1.0),
    ];
    select_keys(&mut s, &picks);
    let before = s.project.clone();
    let reply = s.execute_checked("keys.quickOffset", json!({"delta": 3.0 / 30.0})).unwrap();
    assert_eq!(reply["keys"], 5);
    let sec = |t: f64| Tick::from_seconds_f64(t).0;
    assert_eq!(key_times(&s, ids[1], "transform/opacity"), vec![0, sec(1.0)], "the first layer stays");
    assert_eq!(key_times(&s, ids[0], "transform/opacity"), vec![0, sec(1.0) + 3 * F / 2], "the middle layer: 1.5 frames");
    assert_eq!(key_times(&s, ids[0], "transform/position"), vec![sec(0.5) + 3 * F / 2], "both its properties move as one group");
    assert_eq!(key_times(&s, ids[2], "transform/opacity"), vec![0, sec(1.0) + 3 * F], "the last layer: the whole drag");
    // The keys keep their values and interpolation, and the selection its order.
    let moved = layer(&s, ids[0]).props.prop("transform/opacity").unwrap().keys[1].clone();
    assert_eq!((moved.value.components(), moved.out_interp), (vec![100.0], Interp::Hold));
    let order: Vec<(u64, u64)> = s.state.selected_keys.iter().map(|k| (k.layer.0, k.prop)).collect();
    let want: Vec<(u64, u64)> = picks.iter().map(|(id, path, _)| (*id, uid(&s, *id, path))).collect();
    assert_eq!(order, want);
    s.execute_checked("edit.undo", json!({})).unwrap();
    assert!(s.project.same_content(&before));
}

#[test]
fn keys_of_stretched_and_reversed_layers_follow_the_comp_time_offset() {
    let (mut s, ids) = setup(3);
    for id in &ids {
        add_key(&mut s, *id, "transform/opacity", 1.0, json!(10));
    }
    s.execute_checked("layer.timeStretch", json!({"layers": [ids[1]], "percent": 200})).unwrap();
    s.execute_checked("layer.timeStretch", json!({"layers": [ids[2]], "op": "reverse"})).unwrap();
    let comp_time = |s: &Session, id: u64| {
        let l = layer(s, id);
        l.comp_time(l.props.prop("transform/opacity").unwrap().keys[0].time).0
    };
    let before: Vec<i64> = ids.iter().map(|id| comp_time(&s, *id)).collect();
    select_keys(&mut s, &ids.iter().map(|id| (*id, "transform/opacity", 1.0)).collect::<Vec<_>>());
    s.execute_checked("keys.quickOffset", json!({"delta": 4.0 / 30.0})).unwrap();
    for (i, id) in ids.iter().enumerate() {
        let moved = comp_time(&s, *id) - before[i];
        assert!((moved - 2 * F * i as i64).abs() <= 1, "layer {i}: moved {moved} ticks in comp time");
    }
}

#[test]
fn a_key_drag_over_another_key_and_back_keeps_it() {
    let (mut s, ids) = setup(2);
    for id in &ids {
        add_key(&mut s, *id, "transform/opacity", 0.0, json!(0));
    }
    add_key(&mut s, ids[1], "transform/opacity", 0.5, json!(50));
    select_keys(&mut s, &[(ids[0], "transform/opacity", 0.0), (ids[1], "transform/opacity", 0.0)]);
    let before = s.project.clone();
    s.execute_checked("keys.quickOffset", json!({"delta": 0.5, "merge": "qk", "fromStart": true})).unwrap();
    assert_eq!(key_times(&s, ids[1], "transform/opacity").len(), 1, "the dragged key replaces the one it lands on");
    s.execute_checked("keys.quickOffset", json!({"delta": 0.0, "merge": "qk", "fromStart": true})).unwrap();
    assert!(s.project.same_content(&before), "dragged back, the key passed over is there again");
    assert_eq!(s.state.selected_keys.len(), 2);
}

#[test]
fn keys_need_two_unlocked_layers() {
    let (mut s, ids) = setup(3);
    for id in &ids {
        add_key(&mut s, *id, "transform/opacity", 0.0, json!(0));
        add_key(&mut s, *id, "transform/opacity", 1.0, json!(100));
    }
    select_keys(&mut s, &[(ids[0], "transform/opacity", 0.0), (ids[0], "transform/opacity", 1.0)]);
    let before = s.project.clone();
    assert!(s.execute_checked("keys.quickOffset", json!({"delta": 0.1})).is_err(), "keys on one layer");
    // Locked after selecting: that layer's keys stay; the other two still stagger.
    select_keys(&mut s, &ids.iter().map(|id| (*id, "transform/opacity", 1.0)).collect::<Vec<_>>());
    s.execute_checked("layer.setSwitch", json!({"layers": [ids[1]], "switch": "locked", "value": true})).unwrap();
    s.execute_checked("keys.quickOffset", json!({"delta": 0.1})).unwrap();
    let sec = |t: f64| Tick::from_seconds_f64(t).0;
    assert_eq!(key_times(&s, ids[0], "transform/opacity")[1], sec(1.0));
    assert_eq!(key_times(&s, ids[1], "transform/opacity")[1], sec(1.0));
    assert_eq!(key_times(&s, ids[2], "transform/opacity")[1], sec(1.1));
    assert!(s.state.selected_keys.contains(&KeyRef { layer: LayerId(ids[1]), prop: uid(&s, ids[1], "transform/opacity"), time: Tick(sec(1.0)) }));
    assert!(!before.same_content(&s.project));
}

#[test]
fn roving_keys_follow_and_the_selection_stays_on_the_moved_keys() {
    let (mut s, ids) = setup(2);
    for (t, x) in [(0.0, 0), (1.0, 50), (2.0, 100)] {
        add_key(&mut s, ids[0], "transform/position", t, json!([x, 0]));
    }
    add_key(&mut s, ids[1], "transform/position", 2.0, json!([0, 0]));
    s.execute_checked("keys.select", json!({"keys": [{"layer": ids[0], "path": "transform/position", "time": 1.0}]})).unwrap();
    s.execute_checked("keys.interpolation", json!({"roving": true})).unwrap();
    // Only the last key of the roving layer is selected, with a key on the other layer first.
    select_keys(&mut s, &[(ids[1], "transform/position", 2.0), (ids[0], "transform/position", 2.0)]);
    s.execute_checked("keys.quickOffset", json!({"delta": 1.0})).unwrap();
    let pos = layer(&s, ids[0]).props.prop("transform/position").unwrap();
    let times: Vec<i64> = pos.keys.iter().map(|k| k.time.0).collect();
    assert_eq!((times[0], times[2]), (0, Tick::from_seconds_f64(3.0).0));
    assert!(times[1] > 0 && times[1] < times[2], "the roving key retimed between its neighbours: {times:?}");
    for k in &s.state.selected_keys {
        assert!(layer(&s, k.layer.0).props.find(k.prop).unwrap().keys.iter().any(|x| x.time == k.time), "{k:?} points at a key");
    }
    assert_eq!(s.state.selected_keys.iter().map(|k| k.layer.0).collect::<Vec<_>>(), vec![ids[1], ids[0]]);
}

#[test]
fn moving_staggered_keys_keeps_their_subframe_spacing() {
    let (mut s, ids) = setup(4);
    for id in &ids {
        add_key(&mut s, *id, "transform/opacity", 0.0, json!(0));
    }
    select_keys(&mut s, &ids.iter().map(|id| (*id, "transform/opacity", 0.0)).collect::<Vec<_>>());
    s.execute_checked("keys.quickOffset", json!({"delta": 1.0 / 30.0})).unwrap();
    // A plain move (and a nudge) translates them: it used to put each key on its nearest frame.
    s.execute_checked("keys.move", json!({"delta": 1.0 / 30.0})).unwrap();
    s.execute_checked("keys.nudgeForward", json!({})).unwrap();
    for (i, id) in ids.iter().enumerate() {
        assert_eq!(key_times(&s, *id, "transform/opacity"), vec![2 * F + F * i as i64 / 3], "layer {i}");
    }
}

#[test]
fn a_refused_step_does_not_count_towards_the_drag() {
    let (mut s, ids) = setup(2);
    for id in &ids {
        add_key(&mut s, *id, "transform/opacity", 1.0, json!(0));
    }
    select_keys(&mut s, &ids.iter().map(|id| (*id, "transform/opacity", 1.0)).collect::<Vec<_>>());
    let before = s.project.clone();
    // The second step's last key would land past the end of time (one second short of it, its
    // own delta isn't); with it counted, the third would still move the key that far.
    let far = (Tick::MAX.seconds() - 1.0).floor();
    s.execute_checked("keys.quickOffset", json!({"delta": 1.0, "merge": "steps"})).unwrap();
    assert!(s.execute_checked("keys.quickOffset", json!({"delta": far, "merge": "steps"})).is_err());
    s.execute_checked("keys.quickOffset", json!({"delta": -1.0, "merge": "steps"})).unwrap();
    assert!(s.project.same_content(&before), "only the steps that applied count: {:?}", key_times(&s, ids[1], "transform/opacity"));
}

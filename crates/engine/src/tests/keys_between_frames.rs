//! Graph Editor ▸ Allow Keyframes Between Frames: `keys.transform {betweenFrames: true}` keeps the
//! exact time a move or scale gives, to the tick. Without it (the default) a move goes by whole
//! frames and a scale puts the keys on the nearest frame, at the comp's frame rate.

use effectcraft_project::LayerId;
use effectcraft_time::{FrameRate, Tick};
use serde_json::{Value, json};

use crate::Session;

/// One frame at 30 fps, in ticks.
const F: i64 = 8_467_200_000;
/// One second, in ticks.
const S: i64 = 30 * F;
/// One frame at 29.97 fps (30000/1001), in ticks.
const NTSC: i64 = 8_475_667_200;

/// A comp at `fps` with one solid whose Opacity has keys at `seconds`; returns the session and
/// the layer id.
fn setup_at(fps: f64, seconds: &[f64]) -> (Session, u64) {
    let mut s = Session::default();
    s.execute_checked("comp.new", json!({"width": 64, "height": 64, "frameRate": fps, "duration": 10})).unwrap();
    let id = s.execute_checked("layer.newSolid", json!({"name": "Box", "color": "#fff"})).unwrap()["layer"].as_u64().unwrap();
    for (i, t) in seconds.iter().enumerate() {
        s.execute_checked("prop.addKey", json!({"layer": id, "path": "transform/opacity", "time": t, "value": i as f64 * 10.0})).unwrap();
    }
    (s, id)
}

/// A 30 fps comp, as [`setup_at`].
fn setup(seconds: &[f64]) -> (Session, u64) {
    setup_at(30.0, seconds)
}

/// Select the Opacity keys at these layer times (ticks).
fn select(s: &mut Session, id: u64, ticks: &[i64]) {
    let keys: Vec<Value> = ticks.iter().map(|t| json!({"layer": id, "path": "transform/opacity", "time": Tick(*t).seconds()})).collect();
    s.execute_checked("keys.select", json!({"keys": keys})).unwrap();
}

/// The stored Opacity key times (layer ticks).
fn times(s: &Session, id: u64) -> Vec<i64> {
    s.active_comp().unwrap().layer(LayerId(id)).unwrap().props.prop("transform/opacity").unwrap().keys.iter().map(|k| k.time.0).collect()
}

/// `keys.transform` with these parameters, once with frames and once between frames, each from
/// the same starting keys; returns (with frames, between frames).
fn both(seconds: &[f64], sel: &[i64], params: Value) -> (Vec<i64>, Vec<i64>) {
    let run = |between: bool| {
        let (mut s, id) = setup(seconds);
        select(&mut s, id, sel);
        let mut p = params.clone();
        p["betweenFrames"] = json!(between);
        s.execute_checked("keys.transform", p).unwrap();
        times(&s, id)
    };
    (run(false), run(true))
}

#[test]
fn a_move_keeps_the_exact_offset_between_frames_and_goes_by_whole_frames_without() {
    // 0.4 of a frame later.
    let (frames, between) = both(&[0.0, 1.0, 2.0], &[S], json!({"timeOffset": 0.4 / 30.0}));
    assert_eq!(frames, vec![0, S, 2 * S], "rounded to no move at all");
    assert_eq!(between, vec![0, S + 2 * F / 5, 2 * S], "exactly 0.4 frame later");
    // Without the parameter the keys land on frames, as before.
    let (mut s, id) = setup(&[0.0, 1.0, 2.0]);
    select(&mut s, id, &[S]);
    s.execute_checked("keys.transform", json!({"timeOffset": 0.6 / 30.0})).unwrap();
    assert_eq!(times(&s, id), vec![0, S + F, 2 * S], "the nearest frame, 31");
}

#[test]
fn a_scale_puts_the_keys_between_frames() {
    // 105 % about 0 s: 1 s → 1.05 s (frame 31.5), 2 s → 2.1 s (frame 63).
    let (frames, between) = both(&[0.0, 1.0, 2.0], &[0, S, 2 * S], json!({"timeScale": 1.05, "timeAnchor": 0.0}));
    let fr = FrameRate::new(30, 1);
    assert_eq!(frames, vec![0, fr.tick_of(31).0, fr.tick_of(63).0], "rounded: 31.5 goes down to 31");
    assert_eq!(between, vec![0, S * 105 / 100, S * 210 / 100]);
}

#[test]
fn ntsc_frames_are_counted_at_the_comp_rate_not_in_seconds() {
    // 29.97 fps: frame 30 is at 1.001 s, not 1 s.
    let fr = FrameRate::FPS_29_97;
    assert_eq!(fr.frame_duration().0, NTSC);
    let (mut s, id) = setup_at(29.97, &[0.0, 30.0 * 1001.0 / 30000.0, 60.0 * 1001.0 / 30000.0]);
    assert_eq!(times(&s, id), vec![0, 30 * NTSC, 60 * NTSC], "the keys sit on frames 0, 30 and 60");
    select(&mut s, id, &[30 * NTSC]);
    // 2.6 frames of 29.97 later: 3 whole frames, exactly on frame 33.
    let off = 2.6 * 1001.0 / 30000.0;
    s.execute_checked("keys.transform", json!({"timeOffset": off})).unwrap();
    assert_eq!(times(&s, id), vec![0, fr.tick_of(33).0, 60 * NTSC]);
    s.undo();
    select(&mut s, id, &[30 * NTSC]);
    s.execute_checked("keys.transform", json!({"timeOffset": off, "betweenFrames": true})).unwrap();
    assert_eq!(times(&s, id), vec![0, 30 * NTSC + 13 * NTSC / 5, 60 * NTSC], "frame 32.6, to the tick");
    s.undo();
    // 110 % about frame 0: frame 30 → 33, frame 60 → 66, whatever the rate.
    select(&mut s, id, &[30 * NTSC, 60 * NTSC]);
    s.execute_checked("keys.transform", json!({"timeScale": 1.1, "timeAnchor": 0.0})).unwrap();
    assert_eq!(times(&s, id), vec![0, fr.tick_of(33).0, fr.tick_of(66).0]);
    s.undo();
    select(&mut s, id, &[30 * NTSC, 60 * NTSC]);
    // 105 %: frame 31.5 rounds down to 31 at 29.97 too (frame 63 is exact).
    s.execute_checked("keys.transform", json!({"timeScale": 1.05, "timeAnchor": 0.0})).unwrap();
    assert_eq!(times(&s, id), vec![0, fr.tick_of(31).0, fr.tick_of(63).0]);
}

#[test]
fn a_group_scaled_to_a_frame_ends_exactly_on_it_between_frames() {
    // As the Timeline's Alt-drag sends it: the last key from frame 60 to frame 61 (29.97 fps),
    // scaled about the first key. The last key lands on the frame to the tick; the middle key at
    // frame 30.5.
    let fr = FrameRate::FPS_29_97;
    let (mut s, id) = setup_at(29.97, &[0.0, 30.0 * 1001.0 / 30000.0, 60.0 * 1001.0 / 30000.0]);
    select(&mut s, id, &[0, 30 * NTSC, 60 * NTSC]);
    let (anchor, end, target) = (0.0, Tick(60 * NTSC).seconds(), fr.tick_of(61).seconds());
    let p = json!({"timeScale": (target - anchor) / (end - anchor), "timeAnchor": anchor, "merge": "key-alt-scale", "fromStart": true, "betweenFrames": true});
    s.execute_checked("keys.transform", p).unwrap();
    assert_eq!(times(&s, id), vec![0, 30 * NTSC + NTSC / 2, 61 * NTSC]);
}

#[test]
fn keys_a_fraction_of_a_frame_apart_stay_two_keys() {
    // Frame 31 moved 0.6 frame earlier: with frames it goes one frame back, onto frame 30, and
    // replaces that key.
    let (frames, between) = both(&[1.0, 31.0 / 30.0], &[S + F], json!({"timeOffset": -0.6 / 30.0}));
    assert_eq!(frames, vec![S]);
    assert_eq!(between, vec![S, S + 2 * F / 5]);
}

/// Three layers whose Opacity keys at 1 s were staggered by a Quick Offset of one frame: frame
/// 30, 30.5 and 31. Returns the session and the layers, the keys selected.
fn quick_offset_keys() -> (Session, Vec<u64>) {
    let (mut s, a) = setup(&[1.0]);
    let mut ids = vec![a];
    for name in ["B", "C"] {
        let id = s.execute_checked("layer.newSolid", json!({"name": name, "color": "#fff"})).unwrap()["layer"].as_u64().unwrap();
        s.execute_checked("prop.addKey", json!({"layer": id, "path": "transform/opacity", "time": 1.0, "value": 50})).unwrap();
        ids.push(id);
    }
    let keys: Vec<Value> = ids.iter().map(|id| json!({"layer": id, "path": "transform/opacity", "time": 1.0})).collect();
    s.execute_checked("keys.select", json!({"keys": keys})).unwrap();
    s.execute_checked("keys.quickOffset", json!({"delta": 1.0 / 30.0})).unwrap();
    let at: Vec<i64> = ids.iter().map(|id| times(&s, *id)[0]).collect();
    assert_eq!(at, vec![S, S + F / 2, S + F], "staggered onto a subframe");
    (s, ids)
}

#[test]
fn moving_quick_offset_keys_with_frames_keeps_their_stagger() {
    let (mut s, ids) = quick_offset_keys();
    let at = |s: &Session| ids.iter().map(|id| times(s, *id)[0]).collect::<Vec<_>>();
    let sel = s.state.selected_keys.clone();
    // 1.3 frames later: one whole frame each, as a Timeline drag moves them; the middle key
    // stays half a frame from its neighbours instead of being rounded onto one of them.
    s.execute_checked("keys.transform", json!({"timeOffset": 1.3 / 30.0, "merge": "graph-key", "fromStart": true})).unwrap();
    assert_eq!(at(&s), vec![S + F, S + 3 * F / 2, S + 2 * F]);
    s.undo();
    s.state.selected_keys = sel.clone();
    // Between frames: exactly 1.3 frames each.
    s.execute_checked("keys.transform", json!({"timeOffset": 1.3 / 30.0, "betweenFrames": true})).unwrap();
    assert_eq!(at(&s), vec![S + 13 * F / 10, S + 18 * F / 10, S + 23 * F / 10]);
    s.undo();
    s.state.selected_keys = sel;
    // A scale with frames puts each key it moves on the nearest frame (frame 30.5 · 2 = 61).
    s.execute_checked("keys.transform", json!({"timeScale": 2.0, "timeAnchor": 0.0})).unwrap();
    assert_eq!(at(&s), vec![2 * S, 2 * S + F, 2 * S + 2 * F]);
}

#[test]
fn changing_only_values_never_moves_a_key_in_time() {
    // A Shift-drag along the value axis, the transform box's value handles: keys between frames
    // stay where they are, with or without frames.
    for between in [false, true] {
        let (mut s, ids) = quick_offset_keys();
        for p in [json!({"valueOffset": 5.0}), json!({"valueScale": 2.0, "valueAnchor": 0.0}), json!({"timeOffset": 0.0, "valueOffset": 5.0})] {
            let mut p = p.clone();
            p["betweenFrames"] = json!(between);
            s.execute_checked("keys.transform", p).unwrap();
            assert_eq!(times(&s, ids[1]), vec![S + F / 2], "between: {between}");
        }
        assert_ne!(s.active_comp().unwrap().layer(LayerId(ids[1])).unwrap().props.prop("transform/opacity").unwrap().keys[0].value.as_f64(), 50.0);
    }
}

#[test]
fn a_drag_between_frames_is_one_undo_step_and_undo_restores_the_frames() {
    let (mut s, id) = setup(&[0.0, 1.0, 2.0]);
    select(&mut s, id, &[S, 2 * S]);
    let undo = s.history.undo.len();
    // A drag: every step gives the whole move since the drag started.
    for quarter in 1..=5 {
        let p = json!({"timeOffset": quarter as f64 * 0.25 / 30.0, "merge": "graph-key", "fromStart": true, "betweenFrames": true});
        s.execute_checked("keys.transform", p).unwrap();
    }
    assert_eq!(times(&s, id), vec![0, S + 5 * F / 4, 2 * S + 5 * F / 4], "1.25 frames later, both keys");
    assert_eq!(s.history.undo.len(), undo + 1, "one undo step");
    assert_eq!(s.state.selected_keys.iter().map(|k| k.time.0).collect::<Vec<_>>(), vec![S + 5 * F / 4, 2 * S + 5 * F / 4], "still selected");
    s.undo();
    assert_eq!(times(&s, id), vec![0, S, 2 * S]);
    s.redo();
    assert_eq!(times(&s, id), vec![0, S + 5 * F / 4, 2 * S + 5 * F / 4]);
}

#[test]
fn keys_between_frames_survive_save_and_reopen() {
    let (mut s, id) = setup_at(29.97, &[0.0, 30.0 * 1001.0 / 30000.0]);
    select(&mut s, id, &[30 * NTSC]);
    s.execute_checked("keys.transform", json!({"timeOffset": 0.37 / 29.97, "betweenFrames": true})).unwrap();
    let moved = times(&s, id);
    assert_ne!(moved[1] % NTSC, 0, "between frames");
    let dir = std::env::temp_dir().join(format!("ec-keys-between-frames-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("between.ecproj").to_string_lossy().to_string();
    s.execute_checked("file.saveAs", json!({"path": path})).unwrap();
    let mut reopened = Session::default();
    reopened.execute_checked("file.open", json!({"path": path})).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(times(&reopened, id), moved, "the exact ticks");
}

#[test]
fn a_time_stretched_layer_keeps_the_exact_comp_offset() {
    let (mut s, id) = setup(&[0.0, 1.0]);
    s.execute_checked("layer.timeStretch", json!({"layers": [id], "percent": 200})).unwrap();
    // Layer time 1 s is comp time 2 s; move it 0.4 frame (comp time) later.
    select(&mut s, id, &[S]);
    s.execute_checked("keys.transform", json!({"timeOffset": 0.4 / 30.0, "betweenFrames": true})).unwrap();
    let l = s.active_comp().unwrap().layer(LayerId(id)).unwrap();
    let k = l.props.prop("transform/opacity").unwrap().keys[1].time;
    assert_eq!(l.comp_time(k).0, 2 * S + 2 * F / 5);
    assert_eq!(k.0, S + F / 5, "half the comp offset in layer time");
}

#[test]
fn moves_before_zero_or_far_past_the_end_are_clamped() {
    for between in [false, true] {
        let (mut s, id) = setup(&[0.0, 1.0]);
        select(&mut s, id, &[S]);
        s.execute_checked("keys.transform", json!({"timeOffset": -5.0, "betweenFrames": between})).unwrap();
        assert_eq!(times(&s, id), vec![0], "clamped to 0 s, where it replaces the key there");
        for p in [
            json!({"timeOffset": 1e300}),
            json!({"timeScale": 1e300, "timeAnchor": 0.0}),
            json!({"timeOffset": -1e300}),
            json!({"timeAnchor": -1e300, "timeScale": 2.0}),
        ] {
            let (mut s, id) = setup(&[0.0, 1.0]);
            select(&mut s, id, &[S]);
            let mut p = p.clone();
            p["betweenFrames"] = json!(between);
            s.execute_checked("keys.transform", p.clone()).unwrap();
            let ts = times(&s, id);
            assert!(ts.iter().all(|t| (0..=Tick::MAX.0).contains(t)), "{p}: {ts:?}");
        }
    }
}

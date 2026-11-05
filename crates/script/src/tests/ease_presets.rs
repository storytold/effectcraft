//! The scripting API the bundled Ease Presets ScriptUI panel uses (After Effects parity:
//! `app.settings` that last between runs, per-key methods that keep the key selection, eases and
//! spatial tangents reported as they play, `selectedProperties` with keyed properties,
//! `isInterpolationTypeValid`), and the panel itself (`extensions/scriptui-panels/Ease
//! Presets.jsx`) easing selected keyframes, keeping user presets and taking over the presets the
//! core panel of v0.6.0 saved.

use std::sync::Arc;

use effectcraft_engine::Session;
use effectcraft_engine::commands::scripts::SCRIPT_SETTINGS_FILE;
use effectcraft_engine::config::{ConfigStore, MemoryConfig};
use effectcraft_engine::project::{LayerId, Property};
use effectcraft_keyframe::{Ease, Interp, ease_progress};
use effectcraft_time::Tick;
use serde_json::{Value, json};

use crate::run_code;

const PANEL: &str = "Ease Presets.jsx";

fn session(store: &Arc<MemoryConfig>) -> Session {
    let mut s = Session { expr: Some(Arc::new(effectcraft_expr::Expressions)), ..Default::default() };
    crate::install(&mut s);
    s.config = Some(store.clone());
    s.load_settings();
    s
}

/// A comp with one solid; returns its id.
fn setup(s: &mut Session) -> u64 {
    s.execute("comp.new", json!({"name": "Ease", "width": 640, "height": 360, "frameRate": 30, "duration": 10})).unwrap();
    s.execute("layer.newSolid", json!({"name": "Box", "color": "#ff0000", "width": 100, "height": 100})).unwrap()["layer"].as_u64().unwrap()
}

fn animate(s: &mut Session, l: u64, path: &str, keys: &[(f64, Value)]) {
    for (t, v) in keys {
        s.execute("prop.addKey", json!({"layer": l, "path": path, "time": t, "value": v})).unwrap();
    }
}

fn prop(s: &Session, l: u64, path: &str) -> Property {
    s.active_comp().unwrap().layer(LayerId(l)).unwrap().props.prop(path).unwrap().clone()
}

fn close(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() < tol
}

fn ok(s: &mut Session, code: &str) -> Value {
    let out = run_code(s, code, "test.jsx");
    assert!(out.error.is_none(), "{:?}", out.error);
    out.result
}

fn at(s: f64) -> Tick {
    Tick::from_seconds_f64(s)
}

#[test]
fn app_settings_last_between_runs() {
    let store = Arc::new(MemoryConfig::default());
    let mut s = session(&store);
    ok(&mut s, r#"app.settings.saveSetting("My Tool", "size", 12); app.settings.saveSetting("My Tool", "name", "Box");"#);
    // A new run (the next launch) reads them back.
    let mut s = session(&store);
    let r = ok(
        &mut s,
        r#"[app.settings.haveSetting("My Tool", "size"), app.settings.getSetting("My Tool", "size"), app.settings.getSetting("My Tool", "name"), app.settings.haveSetting("My Tool", "nope")]"#,
    );
    assert_eq!(r, json!([true, "12", "Box", false]));
    assert_eq!(s.execute("script.settings.get", json!({"section": "My Tool", "key": "name"})).unwrap(), json!({"have": true, "value": "Box"}));
    // A corrupt settings file loads as none, never an error.
    store.write(SCRIPT_SETTINGS_FILE, "{oops").unwrap();
    let mut s = session(&store);
    assert_eq!(ok(&mut s, r#"app.settings.haveSetting("My Tool", "size")"#), json!(false));
    assert!(s.execute("script.settings.save", json!({"section": "x"})).is_err());
}

#[test]
fn per_key_methods_keep_the_key_selection_and_report_eases_as_they_play() {
    let store = Arc::new(MemoryConfig::default());
    let mut s = session(&store);
    let l = setup(&mut s);
    animate(&mut s, l, "transform/opacity", &[(0.0, json!(0)), (1.0, json!(100)), (2.0, json!(0))]);
    let key = |t: f64| json!({"layer": l, "path": "transform/opacity", "time": t});
    s.execute("edit.deselectAll", json!({})).unwrap();
    s.execute("keys.select", json!({"keys": [key(0.0), key(2.0)]})).unwrap();
    let before = s.state.selected_keys.clone();
    let r = ok(
        &mut s,
        r#"var comp = app.project.activeItem, p = comp.layer(1).transform.opacity;
        // A linear side reports the speed it plays at (33.3 % influence).
        var lin = p.keyInTemporalEase(3)[0];
        p.setTemporalEaseAtKey(2, [new KeyframeEase(0, 50)], [new KeyframeEase(0, 75)]);
        p.setInterpolationTypeAtKey(3, KeyframeInterpolationType.HOLD, KeyframeInterpolationType.HOLD);
        // Keyed properties are selected properties, as in After Effects.
        var sel = comp.selectedProperties;
        [lin.speed, lin.influence, p.selectedKeys, sel.length, sel.length ? sel[0].name : ""]"#,
    );
    assert!(close(r[0].as_f64().unwrap(), -100.0, 1e-9) && close(r[1].as_f64().unwrap(), 100.0 / 3.0, 1e-9), "{r}");
    assert_eq!(r[2], json!([1, 3]), "the selection is the user's");
    assert_eq!((r[3].clone(), r[4].clone()), (json!(1), json!("Opacity")));
    assert_eq!(s.state.selected_keys, before);
    let op = prop(&s, l, "transform/opacity");
    assert_eq!((op.keys[1].in_ease[0], op.keys[1].out_ease[0]), (Ease { speed: 0.0, influence: 0.5 }, Ease { speed: 0.0, influence: 0.75 }));
    assert_eq!(op.keys[2].in_interp, Interp::Hold);
    // removeKey drops the removed key from the selection, keeps the rest.
    ok(&mut s, "app.project.activeItem.layer(1).transform.opacity.removeKey(3)");
    assert_eq!(s.state.selected_keys, before[..1]);
    // Values that don't interpolate only hold.
    let r = ok(
        &mut s,
        r#"var comp = app.project.activeItem, o = comp.layer(1).transform.opacity, t = comp.layers.addText("Hi").text.sourceText;
        [t.isInterpolationTypeValid(KeyframeInterpolationType.BEZIER), t.isInterpolationTypeValid(KeyframeInterpolationType.HOLD), o.isInterpolationTypeValid(KeyframeInterpolationType.BEZIER)]"#,
    );
    assert_eq!(r, json!([false, true, true]));
}

#[test]
fn spatial_tangents_are_reported_as_the_path_plays() {
    let store = Arc::new(MemoryConfig::default());
    let mut s = session(&store);
    let l = setup(&mut s);
    animate(&mut s, l, "transform/position", &[(0.0, json!([0, 0, 0])), (1.0, json!([100, 0, 0])), (2.0, json!([100, 100, 0]))]);
    let pos = prop(&s, l, "transform/position");
    let (inn, out) = effectcraft_keyframe::spatial_tangents(&pos.keys, 1);
    let r = ok(&mut s, "var p = app.project.activeItem.layer(1).transform.position; [p.keyInSpatialTangent(2), p.keyOutSpatialTangent(2)]");
    for (got, want) in [(&r[0], inn), (&r[1], out)] {
        for d in 0..2 {
            assert!(close(got[d].as_f64().unwrap(), want[d], 1e-9), "{r} vs {inn:?} {out:?}");
        }
    }
    assert!(out[0].abs() + out[1].abs() > 1.0, "the middle key's path curves: {out:?}");
    assert_ne!(pos.keys[1].spatial_out, out, "auto-Bezier: the stored tangent isn't the one that plays");
}

/// The panel's window and a control's description by name.
fn open_panel(s: &mut Session) -> u64 {
    let r = s.execute("window.scriptPanel", json!({"name": PANEL})).unwrap();
    r["window"].as_u64().unwrap()
}

fn control(s: &mut Session, win: u64, name: &str) -> Value {
    fn find(w: &Value, name: &str) -> Option<Value> {
        if w["name"] == name {
            return Some(w.clone());
        }
        w["children"].as_array()?.iter().find_map(|c| find(c, name))
    }
    let w = s.execute("scriptui.get", json!({"window": win})).unwrap();
    find(&w["root"], name).unwrap_or_else(|| panic!("no control {name}"))
}

fn set(s: &mut Session, win: u64, widget: &str, value: Value) {
    s.execute("scriptui.set", json!({"window": win, "widget": widget, "value": value})).unwrap();
}

fn click(s: &mut Session, win: u64, widget: &str) {
    s.execute("scriptui.click", json!({"window": win, "widget": widget})).unwrap();
}

fn status(s: &mut Session, win: u64) -> String {
    control(s, win, "status")["text"].as_str().unwrap().to_string()
}

#[test]
fn the_ease_presets_panel_eases_every_selected_pair_in_one_undo_step() {
    let store = Arc::new(MemoryConfig::default());
    let mut s = session(&store);
    let l = setup(&mut s);
    animate(&mut s, l, "transform/opacity", &[(0.0, json!(0)), (1.0, json!(100)), (3.0, json!(20))]);
    animate(&mut s, l, "transform/rotation", &[(0.0, json!(0)), (2.0, json!(90))]);
    s.execute("prop.select", json!({"layer": l, "path": "transform/opacity"})).unwrap();
    s.execute("prop.select", json!({"layer": l, "path": "transform/rotation", "add": true})).unwrap();
    let selected = s.state.selected_keys.clone();
    let win = open_panel(&mut s);
    // It is a bundled extension listed with the ScriptUI panels.
    let list = s.execute("file.scripts.list", json!({})).unwrap();
    assert!(list.as_array().unwrap().iter().any(|e| e["name"] == PANEL && e["panel"] == true && e["source"] == "extension"));
    let steps = s.history.undo.len();
    set(&mut s, win, "presets", json!("Ease In-Out"));
    // In-Out (the default) on every key: each pair gets the whole curve, as before.
    assert_eq!(status(&mut s, win), "Eased 5 keyframe(s) (In-Out) with \u{201c}Ease In-Out\u{201d}");
    assert_eq!(s.history.undo.len(), steps + 1, "one undo step");
    assert_eq!(s.history.undo.last().map(|(label, _)| label.as_str()), Some("Apply Ease Preset"));
    assert_eq!(s.state.selected_keys, selected, "the keys stay selected for the next preset");
    let eased = Ease { speed: 0.0, influence: 0.5 };
    let op = prop(&s, l, "transform/opacity");
    assert_eq!((op.keys[0].out_interp, op.keys[0].out_ease[0]), (Interp::Bezier, eased));
    assert_eq!((op.keys[1].in_ease[0], op.keys[1].out_ease[0]), (eased, eased));
    assert_eq!((op.keys[2].in_interp, op.keys[2].in_ease[0]), (Interp::Bezier, eased));
    // The first key's in side and the last key's out side are not part of a pair.
    assert_eq!((op.keys[0].in_interp, op.keys[2].out_interp), (Interp::Linear, Interp::Linear));
    // A symmetric curve passes the middle value at mid-time, slow near the keys.
    assert!(close(op.value_at(at(2.0)).as_f64(), 60.0, 1e-6));
    let quarter = op.value_at(at(1.5)).as_f64();
    assert!(close(quarter, 100.0 - 80.0 * ease_progress(0.0, 0.5, 0.0, 0.5, 0.25), 1e-6), "{quarter}");
    assert_eq!(prop(&s, l, "transform/rotation").keys[1].in_interp, Interp::Bezier);
    s.execute("edit.undo", json!({})).unwrap();
    let op = prop(&s, l, "transform/opacity");
    assert!(op.keys.iter().all(|k| k.in_interp == Interp::Linear && k.out_interp == Interp::Linear));
    // Nothing selected: nothing changes, and the panel says why.
    s.execute("edit.deselectAll", json!({})).unwrap();
    let steps = s.history.undo.len();
    set(&mut s, win, "presets", json!("Linear"));
    assert_eq!(status(&mut s, win), "Select keyframes with another keyframe next to it");
    assert_eq!(s.history.undo.len(), steps);
}

/// Select only the keys at `keys` (property path, time in seconds).
fn select_keys(s: &mut Session, l: u64, keys: &[(&str, f64)]) {
    s.execute("edit.deselectAll", json!({})).unwrap();
    let keys: Vec<Value> = keys.iter().map(|(path, t)| json!({"layer": l, "path": path, "time": t})).collect();
    s.execute("keys.select", json!({"keys": keys})).unwrap();
}

/// Type a curve (Out %, Out speed, In %, In speed), choose the sides to ease and click Apply.
fn apply_typed(s: &mut Session, win: u64, mode: &str, c: [f64; 4]) {
    for (name, v) in ["outInfluence", "outSpeed", "inInfluence", "inSpeed"].iter().zip(c) {
        set(s, win, name, json!(v.to_string()));
    }
    set(s, win, "mode", json!(mode));
    click(s, win, "apply");
}

fn eased(e: Ease, speed: f64, influence: f64) -> bool {
    close(e.speed, speed, 1e-9) && close(e.influence, influence, 1e-9)
}

/// #1329: one selected keyframe eases its incoming side (In), its outgoing side (Out) or both,
/// each scaled to the segment on that side (+100/s in, −40/s out here), without changing its
/// neighbours, key times, values or the selection; one undo step restores it.
#[test]
fn one_selected_keyframe_eases_its_in_out_or_both_sides() {
    let store = Arc::new(MemoryConfig::default());
    let mut s = session(&store);
    let l = setup(&mut s);
    animate(&mut s, l, "transform/opacity", &[(0.0, json!(0)), (1.0, json!(100)), (3.0, json!(20))]);
    let win = open_panel(&mut s);
    // In-Out is the default.
    assert_eq!(control(&mut s, win, "mode")["selection"], json!([2]));
    let before = prop(&s, l, "transform/opacity");
    for mode in ["In", "Out", "In-Out"] {
        select_keys(&mut s, l, &[("transform/opacity", 1.0)]);
        let selected = s.state.selected_keys.clone();
        let steps = s.history.undo.len();
        apply_typed(&mut s, win, mode, [30.0, 0.5, 60.0, 2.0]);
        assert_eq!(status(&mut s, win), format!("Eased 1 keyframe(s) ({mode}) with the curve"));
        assert_eq!(s.history.undo.len(), steps + 1, "{mode}: one undo step");
        assert_eq!(s.state.selected_keys, selected, "{mode}: the selection stays");
        let op = prop(&s, l, "transform/opacity");
        assert_eq!((&op.keys[0], &op.keys[2]), (&before.keys[0], &before.keys[2]), "{mode}: the neighbours don't change");
        assert_eq!((op.keys[1].time, op.keys[1].value.as_f64()), (before.keys[1].time, 100.0), "{mode}");
        let k = &op.keys[1];
        let (ins, outs) = (mode != "Out", mode != "In");
        assert_eq!(k.in_interp, if ins { Interp::Bezier } else { Interp::Linear }, "{mode}");
        assert_eq!(k.out_interp, if outs { Interp::Bezier } else { Interp::Linear }, "{mode}");
        if ins {
            assert!(eased(k.in_ease[0], 2.0 * 100.0, 0.6), "{mode}: {:?}", k.in_ease);
        } else {
            assert!(close(op.value_at(at(0.5)).as_f64(), 50.0, 1e-6), "{mode}: the in side still plays linearly");
        }
        if outs {
            assert!(eased(k.out_ease[0], 0.5 * -40.0, 0.3), "{mode}: {:?}", k.out_ease);
        } else {
            assert!(close(op.value_at(at(2.0)).as_f64(), 60.0, 1e-6), "{mode}: the out side still plays linearly");
        }
        s.execute("edit.undo", json!({})).unwrap();
        assert_eq!(prop(&s, l, "transform/opacity").keys, before.keys, "{mode}: undone");
    }
}

/// A side without a neighbouring key isn't eased; when no selected key has one, nothing changes
/// (no undo step) and the panel says why. Keys that aren't neighbours, and keys of several
/// properties, each ease on their own.
#[test]
fn first_last_and_scattered_keyframes_ease_the_sides_that_have_a_segment() {
    let store = Arc::new(MemoryConfig::default());
    let mut s = session(&store);
    let l = setup(&mut s);
    animate(&mut s, l, "transform/opacity", &[(0.0, json!(0)), (1.0, json!(100)), (3.0, json!(20))]);
    animate(&mut s, l, "transform/rotation", &[(0.0, json!(0)), (2.0, json!(90))]);
    let win = open_panel(&mut s);
    let before = prop(&s, l, "transform/opacity");
    let c = [30.0, 0.5, 60.0, 2.0];
    for (mode, t, why) in [("In", 0.0, "before it"), ("Out", 3.0, "after it")] {
        select_keys(&mut s, l, &[("transform/opacity", t)]);
        let steps = s.history.undo.len();
        apply_typed(&mut s, win, mode, c);
        assert_eq!(status(&mut s, win), format!("Select keyframes with another keyframe {why}"));
        assert_eq!(s.history.undo.len(), steps, "{mode}: no undo step");
        assert_eq!(prop(&s, l, "transform/opacity").keys, before.keys);
    }
    // In-Out on the first key eases only its out side.
    select_keys(&mut s, l, &[("transform/opacity", 0.0)]);
    apply_typed(&mut s, win, "In-Out", c);
    let op = prop(&s, l, "transform/opacity");
    assert_eq!((op.keys[0].in_interp, op.keys[0].out_interp), (Interp::Linear, Interp::Bezier));
    assert!(eased(op.keys[0].out_ease[0], 0.5 * 100.0, 0.3), "{:?}", op.keys[0].out_ease);
    assert_eq!(op.keys[1..], before.keys[1..]);
    s.execute("edit.undo", json!({})).unwrap();
    // The first and last Opacity keys (not neighbours) and the last Rotation key.
    select_keys(&mut s, l, &[("transform/opacity", 0.0), ("transform/opacity", 3.0), ("transform/rotation", 2.0)]);
    let steps = s.history.undo.len();
    apply_typed(&mut s, win, "In-Out", c);
    assert_eq!(status(&mut s, win), "Eased 3 keyframe(s) (In-Out) with the curve");
    assert_eq!(s.history.undo.len(), steps + 1);
    let op = prop(&s, l, "transform/opacity");
    assert!(eased(op.keys[0].out_ease[0], 0.5 * 100.0, 0.3), "{:?}", op.keys[0].out_ease);
    assert!(eased(op.keys[2].in_ease[0], 2.0 * -40.0, 0.6), "{:?}", op.keys[2].in_ease);
    assert_eq!(op.keys[1], before.keys[1], "the unselected middle key");
    let rot = prop(&s, l, "transform/rotation");
    assert!(eased(rot.keys[1].in_ease[0], 2.0 * 45.0, 0.6), "{:?}", rot.keys[1].in_ease);
    assert_eq!(rot.keys[0].out_interp, Interp::Linear, "the unselected neighbour");
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(prop(&s, l, "transform/opacity").keys, before.keys);
    assert!(prop(&s, l, "transform/rotation").keys.iter().all(|k| k.in_interp == Interp::Linear && k.out_interp == Interp::Linear));
}

/// A change from the pair-only panel: In-Out on a selected pair also eases the pair's outer sides
/// that face an unselected key (the selected keys' own sides), and Out on the pair eases only
/// out sides.
#[test]
fn a_selected_pair_eases_the_sides_its_mode_names() {
    let store = Arc::new(MemoryConfig::default());
    let mut s = session(&store);
    let l = setup(&mut s);
    animate(&mut s, l, "transform/opacity", &[(0.0, json!(0)), (1.0, json!(100)), (3.0, json!(20))]);
    let win = open_panel(&mut s);
    let before = prop(&s, l, "transform/opacity");
    let c = [30.0, 0.5, 60.0, 2.0];
    select_keys(&mut s, l, &[("transform/opacity", 0.0), ("transform/opacity", 1.0)]);
    apply_typed(&mut s, win, "In-Out", c);
    let op = prop(&s, l, "transform/opacity");
    assert!(eased(op.keys[0].out_ease[0], 50.0, 0.3) && eased(op.keys[1].in_ease[0], 200.0, 0.6));
    assert!(eased(op.keys[1].out_ease[0], -20.0, 0.3), "the side facing the unselected key: {:?}", op.keys[1].out_ease);
    assert_eq!(op.keys[2], before.keys[2]);
    s.execute("edit.undo", json!({})).unwrap();
    apply_typed(&mut s, win, "Out", c);
    let op = prop(&s, l, "transform/opacity");
    assert!(eased(op.keys[0].out_ease[0], 50.0, 0.3) && eased(op.keys[1].out_ease[0], -20.0, 0.3));
    assert_eq!((op.keys[0].in_interp, op.keys[1].in_interp), (Interp::Linear, Interp::Linear));
}

/// Easing one side of a key leaves the other playing as it did: a Hold stays a Hold, and an
/// auto-Bezier side keeps its shape. A preset click uses the chosen sides too.
#[test]
fn the_side_not_eased_keeps_playing_hold_and_auto_bezier() {
    let store = Arc::new(MemoryConfig::default());
    let mut s = session(&store);
    let l = setup(&mut s);
    animate(&mut s, l, "transform/opacity", &[(0.0, json!(0)), (1.0, json!(100)), (3.0, json!(20))]);
    animate(&mut s, l, "transform/rotation", &[(0.0, json!(0)), (1.0, json!(90)), (3.0, json!(30))]);
    ok(
        &mut s,
        r#"var t = app.project.activeItem.layer(1).transform, K = KeyframeInterpolationType;
        t.opacity.setInterpolationTypeAtKey(2, K.LINEAR, K.HOLD);
        t.rotation.setInterpolationTypeAtKey(2, K.BEZIER, K.BEZIER);
        t.rotation.setTemporalAutoBezierAtKey(2, true);"#,
    );
    let rot = prop(&s, l, "transform/rotation");
    let after: Vec<f64> = [1.5, 2.0, 2.5].iter().map(|t| rot.value_at(at(*t)).as_f64()).collect();
    let win = open_panel(&mut s);
    set(&mut s, win, "mode", json!("In"));
    select_keys(&mut s, l, &[("transform/opacity", 1.0), ("transform/rotation", 1.0)]);
    set(&mut s, win, "presets", json!("Ease In-Out"));
    assert_eq!(status(&mut s, win), "Eased 2 keyframe(s) (In) with \u{201c}Ease In-Out\u{201d}");
    let op = prop(&s, l, "transform/opacity");
    assert_eq!((op.keys[1].in_interp, op.keys[1].out_interp), (Interp::Bezier, Interp::Hold));
    assert!(eased(op.keys[1].in_ease[0], 0.0, 0.5), "{:?}", op.keys[1].in_ease);
    assert_eq!(op.value_at(at(2.0)).as_f64(), 100.0, "still holds");
    let eased_rot = prop(&s, l, "transform/rotation");
    assert!(eased(eased_rot.keys[1].in_ease[0], 0.0, 0.5), "{:?}", eased_rot.keys[1].in_ease);
    for (t, want) in [1.5, 2.0, 2.5].iter().zip(after) {
        assert!(close(eased_rot.value_at(at(*t)).as_f64(), want, 1e-6), "the auto-Bezier out side at {t}");
    }
    assert!(!close(eased_rot.value_at(at(0.25)).as_f64(), rot.value_at(at(0.25)).as_f64(), 1e-3), "the in side changed");
}

/// Roving keys pass speed straight through (the engine keeps their sides linear), so an ease set
/// on one wouldn't play: they are skipped and the panel says so, rather than reporting them eased.
#[test]
fn roving_keyframes_are_skipped_and_reported() {
    let store = Arc::new(MemoryConfig::default());
    let mut s = session(&store);
    let l = setup(&mut s);
    animate(&mut s, l, "transform/position", &[(0.0, json!([0, 0, 0])), (1.0, json!([100, 0, 0])), (2.0, json!([200, 0, 0]))]);
    animate(&mut s, l, "transform/opacity", &[(0.0, json!(0)), (1.0, json!(100)), (3.0, json!(20))]);
    ok(&mut s, "app.project.activeItem.layer(1).transform.position.setRovingAtKey(2, true)");
    let pos = prop(&s, l, "transform/position");
    assert!(pos.keys[1].roving);
    let roving_at = pos.keys[1].time.seconds();
    let win = open_panel(&mut s);
    // Only the roving key: nothing changes, no undo step.
    select_keys(&mut s, l, &[("transform/position", roving_at)]);
    let steps = s.history.undo.len();
    set(&mut s, win, "presets", json!("Ease In-Out"));
    assert_eq!(status(&mut s, win), "Nothing eased: skipped 1 roving keyframe(s); turn off Roving to ease them");
    assert_eq!(s.history.undo.len(), steps);
    assert_eq!(prop(&s, l, "transform/position").keys, pos.keys);
    // With an ordinary key: that one is eased, the roving one is counted as skipped.
    select_keys(&mut s, l, &[("transform/position", roving_at), ("transform/opacity", 1.0)]);
    set(&mut s, win, "presets", json!("Linear"));
    set(&mut s, win, "presets", json!("Ease In-Out"));
    assert_eq!(
        status(&mut s, win),
        "Eased 1 keyframe(s) (In-Out) with \u{201c}Ease In-Out\u{201d}; skipped 1 roving keyframe(s); turn off Roving to ease them"
    );
    let k = &prop(&s, l, "transform/position").keys[1];
    assert!(k.roving && k.in_interp == Interp::Linear && k.out_interp == Interp::Linear, "{k:?}");
    let op = prop(&s, l, "transform/opacity");
    assert!(eased(op.keys[1].in_ease[0], 0.0, 0.5) && eased(op.keys[1].out_ease[0], 0.0, 0.5));
}

/// Out alone leaves an auto-Bezier in side playing as it did, and undo and redo restore each state.
#[test]
fn out_alone_keeps_the_in_side_playing_through_undo_and_redo() {
    let store = Arc::new(MemoryConfig::default());
    let mut s = session(&store);
    let l = setup(&mut s);
    animate(&mut s, l, "transform/rotation", &[(0.0, json!(0)), (1.0, json!(90)), (3.0, json!(30))]);
    ok(
        &mut s,
        r#"var r = app.project.activeItem.layer(1).transform.rotation, K = KeyframeInterpolationType;
        r.setInterpolationTypeAtKey(2, K.BEZIER, K.BEZIER);
        r.setTemporalAutoBezierAtKey(2, true);"#,
    );
    let before = prop(&s, l, "transform/rotation");
    let win = open_panel(&mut s);
    select_keys(&mut s, l, &[("transform/rotation", 1.0)]);
    apply_typed(&mut s, win, "Out", [60.0, 0.0, 33.0, 1.0]);
    let after = prop(&s, l, "transform/rotation");
    assert!(eased(after.keys[1].out_ease[0], 0.0, 0.6), "{:?}", after.keys[1].out_ease);
    for t in [0.25, 0.5, 0.75] {
        assert!(close(after.value_at(at(t)).as_f64(), before.value_at(at(t)).as_f64(), 1e-6), "the in side at {t}");
    }
    assert!(!close(after.value_at(at(2.0)).as_f64(), before.value_at(at(2.0)).as_f64(), 1e-3), "the out side changed");
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(prop(&s, l, "transform/rotation").keys, before.keys);
    s.execute("edit.redo", json!({})).unwrap();
    assert_eq!(prop(&s, l, "transform/rotation").keys, after.keys);
}

/// On a curved path, a single key's in and out speeds follow the length of the path on each side
/// (unequal lengths and durations), and the side not eased plays as it did.
#[test]
fn a_single_key_on_a_curved_path_eases_along_each_side() {
    let store = Arc::new(MemoryConfig::default());
    let mut s = session(&store);
    let l = setup(&mut s);
    animate(&mut s, l, "transform/position", &[(0.0, json!([0, 0, 0])), (1.0, json!([100, 0, 0])), (3.0, json!([100, 300, 0]))]);
    let before = prop(&s, l, "transform/position");
    let (len_in, len_out) = (effectcraft_keyframe::spatial_segment_length(&before.keys, 0), effectcraft_keyframe::spatial_segment_length(&before.keys, 1));
    let win = open_panel(&mut s);
    select_keys(&mut s, l, &[("transform/position", 1.0)]);
    apply_typed(&mut s, win, "In", [40.0, 1.5, 50.0, 2.0]);
    let pos = prop(&s, l, "transform/position");
    let k = &pos.keys[1];
    // In speed: twice the incoming segment's average (its path length over 1 s).
    assert!(k.in_ease.iter().all(|e| close(e.speed, 2.0 * len_in, 0.05 * len_in) && close(e.influence, 0.5, 1e-9)), "{:?} vs {len_in}", k.in_ease);
    // The out side (2 s along a longer path) plays as before.
    for t in [1.5, 2.0, 2.5] {
        let (a, b) = (pos.value_at(at(t)).as_vec2(), before.value_at(at(t)).as_vec2());
        assert!(close(a[0], b[0], 1e-6) && close(a[1], b[1], 1e-6), "out side at {t}: {a:?} vs {b:?}");
    }
    // Playback on the in side: arriving at twice the average speed, the last tenth of a second
    // covers more of the path than a constant speed would (len_in / 10).
    let (p9, p10) = (pos.value_at(at(0.9)).as_vec2(), pos.value_at(at(1.0)).as_vec2());
    let step = ((p10[0] - p9[0]).powi(2) + (p10[1] - p9[1]).powi(2)).sqrt();
    assert!(step > len_in / 10.0 * 1.2, "{step} vs {len_in}");
    assert!(len_out > len_in, "{len_out} {len_in}");
}

/// On one key, each side scales to its own segment: per dimension, and along the motion path.
#[test]
fn single_keys_scale_each_side_to_its_dimensions_and_motion_path() {
    let store = Arc::new(MemoryConfig::default());
    let mut s = session(&store);
    let l = setup(&mut s);
    // Scale: +100/−50 per second into the key, 0/+50 per second out of it.
    animate(&mut s, l, "transform/scale", &[(0.0, json!([100, 100])), (1.0, json!([200, 50])), (3.0, json!([200, 150]))]);
    // Position on a straight path: 100 px/s into the key, 50 px/s out of it.
    animate(&mut s, l, "transform/position", &[(0.0, json!([0, 0, 0])), (1.0, json!([100, 0, 0])), (3.0, json!([200, 0, 0]))]);
    let win = open_panel(&mut s);
    select_keys(&mut s, l, &[("transform/scale", 1.0), ("transform/position", 1.0)]);
    apply_typed(&mut s, win, "In-Out", [40.0, 1.5, 50.0, 2.0]);
    assert_eq!(status(&mut s, win), "Eased 2 keyframe(s) (In-Out) with the curve");
    let k = prop(&s, l, "transform/scale").keys[1].clone();
    assert!(eased(k.in_ease[0], 200.0, 0.5) && eased(k.in_ease[1], -100.0, 0.5), "{:?}", k.in_ease);
    assert!(eased(k.out_ease[0], 0.0, 0.4) && eased(k.out_ease[1], 75.0, 0.4), "{:?}", k.out_ease);
    let k = prop(&s, l, "transform/position").keys[1].clone();
    assert!(k.in_ease.iter().all(|e| close(e.speed, 200.0, 0.5) && close(e.influence, 0.5, 1e-9)), "{:?}", k.in_ease);
    assert!(k.out_ease.iter().all(|e| close(e.speed, 75.0, 0.5) && close(e.influence, 0.4, 1e-9)), "{:?}", k.out_ease);
    let pos = prop(&s, l, "transform/position");
    assert_eq!(pos.value_at(at(1.0)).as_vec2()[0], 100.0, "the key's value stays");
}

#[test]
fn a_typed_curve_scales_to_each_dimension_and_follows_motion_paths() {
    let store = Arc::new(MemoryConfig::default());
    let mut s = session(&store);
    let l = setup(&mut s);
    animate(&mut s, l, "transform/scale", &[(0.0, json!([100, 100])), (1.0, json!([200, 50]))]);
    animate(&mut s, l, "transform/position", &[(0.0, json!([0, 0, 0])), (2.0, json!([300, 400, 0]))]);
    s.execute("prop.select", json!({"layer": l, "path": "transform/scale"})).unwrap();
    let win = open_panel(&mut s);
    let typed = |s: &mut Session, c: [f64; 4]| {
        for (name, v) in ["outInfluence", "outSpeed", "inInfluence", "inSpeed"].iter().zip(c) {
            set(s, win, name, json!(v.to_string()));
        }
        click(s, win, "apply");
    };
    typed(&mut s, [60.0, 0.0, 20.0, 2.0]);
    let sc = prop(&s, l, "transform/scale");
    // X rises 100/s and Y falls 50/s: the in speed (2× average) is per dimension.
    assert_eq!(sc.keys[1].in_ease[..2], [Ease { speed: 200.0, influence: 0.2 }, Ease { speed: -100.0, influence: 0.2 }]);
    let f = ease_progress(0.0, 0.6, 2.0, 0.2, 0.5);
    let v = sc.value_at(at(0.5)).as_vec2();
    assert!(close(v[0], 100.0 + 100.0 * f, 1e-6) && close(v[1], 100.0 - 50.0 * f, 1e-6), "{v:?}");
    // On Position the speed is along the 500 px path.
    s.execute("prop.select", json!({"layer": l, "path": "transform/position"})).unwrap();
    typed(&mut s, [25.0, 0.4, 75.0, 0.0]);
    let pos = prop(&s, l, "transform/position");
    assert!(pos.keys[0].out_ease.iter().all(|e| close(e.speed, 0.4 * 250.0, 1e-6) && close(e.influence, 0.25, 1e-9)), "{:?}", pos.keys[0].out_ease);
    let f = ease_progress(0.4, 0.25, 0.0, 0.75, 0.5);
    let p = pos.value_at(at(1.0)).as_vec2();
    assert!(close(p[0], 300.0 * f, 0.05) && close(p[1], 400.0 * f, 0.05), "{p:?} {f}");
    // From Keys reads the applied curve back into the fields.
    for name in ["outInfluence", "outSpeed", "inInfluence", "inSpeed"] {
        set(&mut s, win, name, json!("1"));
    }
    click(&mut s, win, "fromKeys");
    let fields: Vec<String> =
        ["outInfluence", "outSpeed", "inInfluence", "inSpeed"].iter().map(|n| control(&mut s, win, n)["text"].as_str().unwrap().to_string()).collect();
    assert_eq!(fields, ["25", "0.4", "75", "0"]);
}

#[test]
fn user_presets_save_rename_delete_and_persist() {
    let store = Arc::new(MemoryConfig::default());
    let mut s = session(&store);
    let l = setup(&mut s);
    animate(&mut s, l, "transform/opacity", &[(0.0, json!(0)), (1.0, json!(100))]);
    s.execute("prop.select", json!({"layer": l, "path": "transform/opacity"})).unwrap();
    let win = open_panel(&mut s);
    set(&mut s, win, "presets", json!("Decelerate"));
    // Save keeps the curve in the fields under the typed name.
    set(&mut s, win, "name", json!("  Settle "));
    click(&mut s, win, "save");
    assert_eq!(status(&mut s, win), "Saved the ease preset \u{201c}Settle\u{201d}");
    // Built-in names are refused.
    set(&mut s, win, "name", json!("linear"));
    click(&mut s, win, "save");
    assert!(status(&mut s, win).contains("is a built-in preset"));
    let items = |s: &mut Session| control(s, win, "presets")["items"].clone();
    assert_eq!(items(&mut s).as_array().unwrap().last().unwrap(), "Settle");
    // Rename and Delete act on the selected user preset; built-in presets can't change.
    set(&mut s, win, "presets", json!("Linear"));
    set(&mut s, win, "name", json!("Straight"));
    click(&mut s, win, "rename");
    assert_eq!(status(&mut s, win), "Select one of your own presets first");
    set(&mut s, win, "presets", json!("Settle"));
    set(&mut s, win, "name", json!("Soft Landing"));
    click(&mut s, win, "rename");
    let names = items(&mut s);
    assert!(names.as_array().unwrap().contains(&json!("Soft Landing")) && !names.as_array().unwrap().contains(&json!("Settle")), "{names}");
    // Kept with app.settings: a new run of the panel (the next launch) lists them.
    let stored: Value = serde_json::from_str(&store.read(SCRIPT_SETTINGS_FILE).unwrap()).unwrap();
    let presets: Value = serde_json::from_str(stored["Ease Presets"]["userPresets"].as_str().unwrap()).unwrap();
    assert_eq!(presets["presets"][0]["name"], "Soft Landing");
    assert!(close(presets["presets"][0]["curve"]["outSpeed"].as_f64().unwrap(), 1.5, 1e-9), "{presets}");
    let mut s2 = session(&store);
    setup(&mut s2);
    let win2 = open_panel(&mut s2);
    assert_eq!(control(&mut s2, win2, "presets")["items"].as_array().unwrap().len(), 13);
    set(&mut s2, win2, "presets", json!("Soft Landing"));
    click(&mut s2, win2, "delete");
    assert_eq!(control(&mut s2, win2, "presets")["items"].as_array().unwrap().len(), 12, "the built-in presets");
}

/// User presets the core Ease Presets panel of v0.6.0 saved (`ease_presets.json`) move to the
/// panel's settings when settings load, once; unusable entries are skipped by the panel.
#[test]
fn presets_saved_by_the_old_core_panel_carry_over() {
    let store = Arc::new(MemoryConfig::default());
    let c = json!({"outInfluence": 30, "outSpeed": 0, "inInfluence": 30, "inSpeed": 0});
    let old = json!({"version": 1, "presets": [
        {"name": "Snap", "curve": c},
        {"name": "Linear", "curve": c},
        {"name": "", "curve": c},
        {"name": "No curve"},
        {"name": "snap", "curve": c},
        {"name": "Bad", "curve": {"outInfluence": "x", "outSpeed": 0, "inInfluence": 30, "inSpeed": 0}}
    ]});
    store.write("ease_presets.json", &old.to_string()).unwrap();
    let mut s = session(&store);
    setup(&mut s);
    let win = open_panel(&mut s);
    let items = control(&mut s, win, "presets")["items"].clone();
    assert_eq!(items.as_array().unwrap().len(), 13, "{items}");
    assert_eq!(items[12], "Snap");
    // Deleting it there is final: the old file doesn't come back.
    set(&mut s, win, "presets", json!("Snap"));
    click(&mut s, win, "delete");
    let mut s = session(&store);
    setup(&mut s);
    let win = open_panel(&mut s);
    assert_eq!(control(&mut s, win, "presets")["items"].as_array().unwrap().len(), 12);
}

/// #2465: the Linear preset sets Linear interpolation on every side it applies to, including the
/// unused first-in and last-out sides; editing its speeds makes a Bezier curve.
#[test]
fn the_linear_preset_sets_linear_interpolation_and_edited_speeds_give_bezier() {
    let store = Arc::new(MemoryConfig::default());
    let mut s = session(&store);
    let l = setup(&mut s);
    animate(&mut s, l, "transform/opacity", &[(0.0, json!(0)), (1.0, json!(100)), (3.0, json!(20))]);
    s.execute("prop.select", json!({"layer": l, "path": "transform/opacity"})).unwrap();
    let win = open_panel(&mut s);
    // Start from Bezier keys so Linear has something to restore.
    ok(
        &mut s,
        r#"var p = app.project.activeItem.layer(1).transform.opacity, K = KeyframeInterpolationType;
        for (var i = 1; i <= p.numKeys; i++) p.setInterpolationTypeAtKey(i, K.BEZIER, K.BEZIER);"#,
    );
    let op = prop(&s, l, "transform/opacity");
    for (i, k) in op.keys.iter().enumerate() {
        assert_eq!((k.in_interp, k.out_interp), (Interp::Bezier, Interp::Bezier), "initial key {i}");
    }
    let steps = s.history.undo.len();
    set(&mut s, win, "presets", json!("Linear"));
    assert_eq!(s.history.undo.len(), steps + 1, "one undo step");
    let op = prop(&s, l, "transform/opacity");
    for (i, k) in op.keys.iter().enumerate() {
        assert_eq!((k.in_interp, k.out_interp), (Interp::Linear, Interp::Linear), "key {i}");
    }
    // Edited speeds make a custom Bezier curve even though Linear is still the selected preset.
    select_keys(&mut s, l, &[("transform/opacity", 1.0)]);
    apply_typed(&mut s, win, "In-Out", [33.0, 2.0, 33.0, 2.0]);
    let k = &prop(&s, l, "transform/opacity").keys[1];
    assert_eq!((k.in_interp, k.out_interp), (Interp::Bezier, Interp::Bezier));
}

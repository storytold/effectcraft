//! Animation presets that hold text animators (#2530): an animator is saved whole and made again
//! on the layer it is applied to, with its keys at the playhead.

use effectcraft_keyframe::{Keyframe, Value as KV};
use effectcraft_project::{LayerId, PropGroup};
use effectcraft_time::Tick;
use serde_json::{Value, json};

use crate::Session;

fn session() -> Session {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "C", "width": 640, "height": 360, "frameRate": 30, "duration": 10})).unwrap();
    s
}

fn new_text(s: &mut Session, text: &str) -> u64 {
    s.execute("layer.newText", json!({"text": text, "size": 60})).unwrap()["layer"].as_u64().unwrap()
}

fn animators(s: &Session, layer: u64) -> Vec<PropGroup> {
    let l = s.active_comp().unwrap().layer(LayerId(layer)).unwrap();
    l.props.group("text/animators").unwrap().groups().cloned().collect()
}

fn names(s: &Session, layer: u64) -> Vec<String> {
    animators(s, layer).into_iter().map(|g| g.name).collect()
}

/// No two properties or groups on any layer share a uid.
fn assert_uids_are_unique(s: &Session) {
    fn collect(v: &Value, out: &mut Vec<u64>) {
        match v {
            Value::Object(o) => {
                out.extend(o.get("uid").and_then(Value::as_u64));
                o.values().for_each(|c| collect(c, out));
            }
            Value::Array(a) => a.iter().for_each(|c| collect(c, out)),
            _ => {}
        }
    }
    let mut uids = vec![];
    for l in &s.active_comp().unwrap().layers {
        collect(&serde_json::to_value(&l.props).unwrap(), &mut uids);
    }
    let all = uids.len();
    uids.sort_unstable();
    uids.dedup();
    assert_eq!(uids.len(), all);
}

/// A named animator that fades characters on: Opacity 0, a Range Selector's End keyed from 0 at
/// 1s to 100 at 2s, a Based On setting, an expression on its Offset, and a Wiggly Selector.
fn fade_on(s: &mut Session, layer: u64, name: &str) -> u64 {
    let animator = s.execute("layer.addTextAnimator", json!({"layer": layer, "properties": ["opacity"], "name": name})).unwrap()["animator"].as_u64().unwrap();
    s.execute("layer.addTextSelector", json!({"layer": layer, "animator": animator, "kind": "wiggly"})).unwrap();
    let g = animators(s, layer).into_iter().find(|g| g.uid == animator).unwrap();
    let selector = |path: &str| g.prop(&format!("selectors/rangeSelector/{path}")).unwrap().uid;
    s.execute("prop.set", json!({"layer": layer, "prop": g.prop("properties/opacity").unwrap().uid, "value": 0})).unwrap();
    s.execute("prop.set", json!({"layer": layer, "prop": selector("advanced/basedOn"), "value": 2})).unwrap();
    s.execute("prop.setExpression", json!({"layer": layer, "prop": selector("offset"), "expression": "time * 10"})).unwrap();
    for (time, value) in [(1.0, 0.0), (2.0, 100.0)] {
        s.execute("prop.addKey", json!({"layer": layer, "prop": selector("end"), "time": time, "value": value})).unwrap();
    }
    animator
}

fn preset_path(name: &str) -> String {
    std::env::temp_dir().join(format!("ec-2530-{name}-{}.ecpreset", std::process::id())).to_string_lossy().to_string()
}

/// Select each property or group in `selected` on `layer` and save them to a new preset file.
fn save(s: &mut Session, name: &str, layer: u64, selected: &[u64]) -> String {
    for (i, prop) in selected.iter().enumerate() {
        s.execute("prop.select", json!({"layer": layer, "prop": prop, "add": i > 0})).unwrap();
    }
    let path = preset_path(name);
    let _ = std::fs::remove_file(&path);
    s.execute("anim.savePreset", json!({"path": path})).unwrap();
    path
}

fn apply(s: &mut Session, path: &str, layer: u64, at: f64) -> Value {
    s.execute("time.set", json!({"time": at})).unwrap();
    s.execute("anim.applyPreset", json!({"path": path, "layers": [layer]})).unwrap()
}

/// `g` as JSON without its uids, with its keys moved `by` seconds: what must match between a
/// saved animator and the animator made from it.
fn content(g: &PropGroup, by: f64) -> Value {
    fn drop_uids(v: &mut Value) {
        match v {
            Value::Object(o) => {
                o.remove("uid");
                o.values_mut().for_each(drop_uids);
            }
            Value::Array(a) => a.iter_mut().for_each(drop_uids),
            _ => {}
        }
    }
    let mut g = g.clone();
    g.walk_mut(&mut |p| p.keys.iter_mut().for_each(|k| k.time += Tick::from_seconds_f64(by)));
    let mut v = serde_json::to_value(g).unwrap();
    drop_uids(&mut v);
    v
}

/// #2530, #3114: the steps in the reports. The whole animator is selected and saved, then applied
/// to a new text layer with the playhead at 3s.
#[test]
fn a_saved_animator_applies_to_a_new_text_layer() {
    let mut s = session();
    let source = new_text(&mut s, "Source");
    let animator = fade_on(&mut s, source, "Fade In");
    let path = save(&mut s, "new-layer", source, &[animator]);
    let saved = animators(&s, source);
    let target = new_text(&mut s, "Target");

    let r = apply(&mut s, &path, target, 3.0);
    let _ = std::fs::remove_file(&path);

    assert_eq!(r["skipped"], json!([]), "{r}");
    let made = animators(&s, target);
    assert_eq!(made.len(), 1, "{r}");
    // The earliest key, at 1s on the source, lands on the playhead: 2s later.
    assert_eq!(content(&made[0], 0.0), content(&saved[0], 2.0));
    let keys: Vec<f64> = made[0].prop("selectors/rangeSelector/end").unwrap().keys.iter().map(|k| k.time.seconds()).collect();
    assert_eq!(keys, [3.0, 4.0]);
}

/// One selected property brings its whole animator: End alone would key a selector with nothing
/// to reveal. The file holds the animator, not the property as well.
#[test]
fn one_selected_property_saves_its_whole_animator() {
    let mut s = session();
    let source = new_text(&mut s, "Source");
    fade_on(&mut s, source, "Fade In");
    let saved = animators(&s, source);
    let end = saved[0].prop("selectors/rangeSelector/end").unwrap().uid;
    let path = save(&mut s, "one-property", source, &[end]);
    let file: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(file["effectcraftAnimationPreset"], 3);
    assert_eq!(file["properties"], json!([]));
    assert_eq!(file["textAnimators"].as_array().unwrap().len(), 1);
    let target = new_text(&mut s, "Target");

    let r = apply(&mut s, &path, target, 0.0);
    let _ = std::fs::remove_file(&path);

    assert_eq!(r["applied"], 1, "{r}");
    assert_eq!(content(&animators(&s, target)[0], 0.0), content(&saved[0], -1.0));
}

/// A preset adds to the animators a layer has, which stay as they are, in the order the animators
/// were saved in whatever order they were selected, with new uids, as one Undo step.
#[test]
fn a_preset_adds_to_the_animators_a_layer_has() {
    let mut s = session();
    let source = new_text(&mut s, "Source");
    let one = fade_on(&mut s, source, "One");
    let two = fade_on(&mut s, source, "Two");
    let path = save(&mut s, "existing", source, &[two, one]);
    let target = new_text(&mut s, "Target");
    let mine = fade_on(&mut s, target, "Mine");
    s.execute("prop.select", json!({"layer": target, "prop": mine})).unwrap();
    let before = animators(&s, target);
    let selected = s.state.selected_props.clone();
    let steps = s.history.undo.len();

    apply(&mut s, &path, target, 3.0);

    assert_eq!(names(&s, target), ["Mine", "One", "Two"]);
    assert_eq!(animators(&s, target)[0], before[0]);
    assert_eq!(s.state.selected_props, selected);
    assert_eq!(s.history.undo.len(), steps + 1);
    assert_uids_are_unique(&s);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(animators(&s, target), before);
    s.execute("edit.redo", json!({})).unwrap();
    assert_eq!(names(&s, target), ["Mine", "One", "Two"]);

    // The layer it was saved from gets another copy, as it does of a saved effect.
    apply(&mut s, &path, source, 3.0);
    let _ = std::fs::remove_file(&path);
    assert_eq!(names(&s, source), ["One", "Two", "One", "Two"]);
    assert_uids_are_unique(&s);
}

/// An animator fits the Per-character 3D setting of the layer it lands on and nothing else
/// changes. X / Y rotation saved with a 3D animator stays on a 2D layer, which doesn't draw it,
/// and is there when the layer turns 3D.
#[test]
fn an_animator_fits_the_per_character_3d_setting_of_its_layer() {
    let mut s = session();
    let round = new_text(&mut s, "Round");
    s.execute("layer.enablePerChar3D", json!({"layer": round, "enabled": true})).unwrap();
    let spin =
        s.execute("layer.addTextAnimator", json!({"layer": round, "properties": ["rotation", "scale"], "name": "Spin"})).unwrap()["animator"].as_u64().unwrap();
    let x = animators(&s, round)[0].prop("properties/rotationX").unwrap().uid;
    for (time, value) in [(1.0, 0.0), (2.0, 90.0)] {
        s.execute("prop.addKey", json!({"layer": round, "prop": x, "time": time, "value": value})).unwrap();
    }
    let round_preset = save(&mut s, "round", round, &[spin]);
    let flat = new_text(&mut s, "Flat");
    let tilt =
        s.execute("layer.addTextAnimator", json!({"layer": flat, "properties": ["rotation", "scale"], "name": "Tilt"})).unwrap()["animator"].as_u64().unwrap();
    let flat_preset = save(&mut s, "flat", flat, &[tilt]);
    let get = |g: &PropGroup, path: &str| g.prop(&format!("properties/{path}")).unwrap().clone();

    // 3D animator, 2D layer.
    let keeps = animators(&s, flat)[0].clone();
    apply(&mut s, &round_preset, flat, 0.0);
    assert_eq!(animators(&s, flat)[0], keeps);
    let made = animators(&s, flat)[1].clone();
    assert_eq!(get(&made, "rotationX").keys.len(), 2);
    assert_eq!(get(&made, "rotation").name, "Rotation");
    assert_eq!(get(&made, "scale").shown_dims, 2);
    let l = s.active_comp().unwrap().layer(LayerId(flat)).unwrap();
    assert!(!l.switches.three_d && !l.props.prop("text/perChar3d").unwrap().value.as_bool());
    s.execute("layer.enablePerChar3D", json!({"layer": flat, "enabled": true})).unwrap();
    let made = animators(&s, flat)[1].clone();
    assert_eq!(get(&made, "rotationX").keys.len(), 2);
    assert_eq!(made.sub("properties").unwrap().children.iter().filter(|c| c.match_id() == "rotationX").count(), 1);

    // 2D animator, 3D layer.
    let keeps = animators(&s, round);
    apply(&mut s, &flat_preset, round, 0.0);
    let _ = std::fs::remove_file(&round_preset);
    let _ = std::fs::remove_file(&flat_preset);
    assert_eq!(animators(&s, round)[0], keeps[0]);
    let made = animators(&s, round)[1].clone();
    assert_eq!(get(&made, "rotation").name, "Z Rotation");
    assert_eq!(get(&made, "rotationY").name, "Y Rotation");
    assert_eq!(get(&made, "scale").shown_dims, 3);
    assert_uids_are_unique(&s);
}

/// With the Per-character 3D setting saved too (the whole Text group is selected), a layer turns
/// 3D as `layer.enablePerChar3D` makes it, in the one Undo step: the setting alone wouldn't draw.
#[test]
fn a_saved_per_character_3d_setting_makes_the_layer_3d() {
    let mut s = session();
    let round = new_text(&mut s, "Round");
    s.execute("layer.enablePerChar3D", json!({"layer": round, "enabled": true})).unwrap();
    s.execute("layer.addTextAnimator", json!({"layer": round, "properties": ["rotation"]})).unwrap();
    let text = s.active_comp().unwrap().layer(LayerId(round)).unwrap().props.sub("text").unwrap().uid;
    let path = save(&mut s, "per-char", round, &[text]);
    let target = new_text(&mut s, "Target");
    s.execute("layer.addTextAnimator", json!({"layer": target, "properties": ["rotation"], "name": "Existing"})).unwrap();
    let existing = animators(&s, target)[0].clone();
    assert!(existing.prop("properties/rotationX").is_none());
    let is_3d = |s: &Session| {
        let l = s.active_comp().unwrap().layer(LayerId(target)).unwrap();
        (l.switches.three_d, l.props.prop("text/perChar3d").unwrap().value.as_bool())
    };

    apply(&mut s, &path, target, 0.0);
    let _ = std::fs::remove_file(&path);

    assert_eq!(is_3d(&s), (true, true));
    let after = animators(&s, target);
    assert_eq!(after.len(), 2);
    assert!(after[0].prop("properties/rotationX").is_some());
    assert!(after[0].prop("properties/rotationY").is_some());
    assert!(after[1].prop("properties/rotationX").is_some());
    assert_uids_are_unique(&s);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(is_3d(&s), (false, false));
    assert_eq!(animators(&s, target), vec![existing]);
}

/// Turning per-character 3D off through a saved Text group also updates existing controls.
#[test]
fn a_saved_2d_setting_updates_existing_3d_animators_without_losing_rotation() {
    let mut s = session();
    let source = new_text(&mut s, "Flat source");
    let text = s.active_comp().unwrap().layer(LayerId(source)).unwrap().props.sub("text").unwrap().uid;
    let path = save(&mut s, "disable-3d", source, &[text]);
    let target = new_text(&mut s, "3D target");
    s.execute("layer.enablePerChar3D", json!({"layer": target, "enabled": true})).unwrap();
    s.execute("layer.addTextAnimator", json!({"layer": target, "properties": ["rotation", "scale"]})).unwrap();
    let before = animators(&s, target)[0].clone();
    let rotation_x = before.prop("properties/rotationX").unwrap().uid;
    s.execute("prop.addKey", json!({"layer": target, "prop": rotation_x, "time": 1.0, "value": 45})).unwrap();
    apply(&mut s, &path, target, 0.0);
    let _ = std::fs::remove_file(&path);
    let after = animators(&s, target)[0].clone();
    assert_eq!(after.prop("properties/scale").unwrap().shown_dims, 2);
    assert_eq!(after.prop("properties/rotation").unwrap().name, "Rotation");
    assert_eq!(after.prop("properties/rotationX").unwrap().keys.len(), 1);
    assert_uids_are_unique(&s);
}

/// A preset that fits no layer says so and leaves the project and history as they were. With a
/// text layer among the targets it applies there and reports the other layer.
#[test]
fn a_preset_that_fits_no_layer_is_an_error() {
    let mut s = session();
    let source = new_text(&mut s, "Source");
    let animator = fade_on(&mut s, source, "Fade In");
    let path = save(&mut s, "no-fit", source, &[animator]);
    let solid = s.execute("layer.newSolid", json!({"color": "#ff0000", "width": 100, "height": 100})).unwrap()["layer"].as_u64().unwrap();
    let steps = s.history.undo.len();
    let recent = s.prefs.recent_presets.clone();
    let revision = s.prefs_revision;

    let r = s.execute("anim.applyPreset", json!({"path": path, "layers": [solid]}));
    assert!(r.unwrap_err().to_string().contains("nothing in this preset fits"));
    assert_eq!(s.history.undo.len(), steps);
    assert_eq!(s.prefs.recent_presets, recent);
    assert_eq!(s.prefs_revision, revision);

    let r = s.execute("anim.applyPreset", json!({"path": path, "layers": [solid, source]})).unwrap();
    let _ = std::fs::remove_file(&path);
    assert_eq!((r["applied"].as_u64(), r["skipped"].clone()), (Some(1), json!(["text/animators/Fade In"])));
}

/// A preset saved by an earlier version holds loose paths. It updates an animator a layer has as
/// it always did, and on a layer without one it says to save the preset again.
#[test]
fn an_older_preset_updates_animators_and_asks_to_be_saved_again_without_them() {
    let mut s = session();
    let source = new_text(&mut s, "Source");
    fade_on(&mut s, source, "Fade In");
    let path = "text/animators/animator/properties/opacity";
    let preset = json!({"effectcraftAnimationPreset": 2, "properties": [{"path": path, "value": KV::Scalar(40.0), "keys": []}], "effects": []});

    let r = s.execute("anim.applyPreset", json!({"preset": preset, "layers": [source]})).unwrap();
    assert_eq!(r["applied"], 1, "{r}");
    assert_eq!(animators(&s, source).len(), 1);
    assert_eq!(s.active_comp().unwrap().layer(LayerId(source)).unwrap().props.prop(path).unwrap().value, KV::Scalar(40.0));

    let fresh = new_text(&mut s, "Fresh");
    let steps = s.history.undo.len();
    let r = s.execute("anim.applyPreset", json!({"preset": preset, "layers": [fresh]}));
    assert!(r.unwrap_err().to_string().contains("save it again"));
    assert_eq!(animators(&s, fresh), vec![]);
    assert_eq!(s.history.undo.len(), steps);
}

/// A malformed file, or a key that can't move to a later layer's playhead, is an error that
/// changes nothing, on the layers before it too.
#[test]
fn a_bad_preset_changes_nothing() {
    let mut s = session();
    let source = new_text(&mut s, "Source");
    fade_on(&mut s, source, "Fade In");
    let good = serde_json::to_value(&animators(&s, source)[0]).unwrap();
    let mut other = good.clone();
    other["match"] = json!("rangeSelector");
    let early = new_text(&mut s, "Early");
    let late = new_text(&mut s, "Late");
    let steps = s.history.undo.len();
    let unchanged = |s: &Session| {
        assert_eq!((animators(s, early), animators(s, late)), (vec![], vec![]));
        assert_eq!(s.history.undo.len(), steps);
    };
    for list in [json!("not a list"), json!([good, 5]), json!([good, other])] {
        let preset = json!({"effectcraftAnimationPreset": 3, "textAnimators": list});
        let r = s.execute("anim.applyPreset", json!({"preset": preset, "layers": [early]}));
        assert!(r.unwrap_err().to_string().contains("bad text animator"));
        unchanged(&s);
    }

    // The parser refuses a file nested past its limit, before anything walks it.
    let path = preset_path("deep");
    let deep = format!("{{\"effectcraftAnimationPreset\":3,\"textAnimators\":{}{}}}", "[".repeat(20_000), "]".repeat(20_000));
    std::fs::write(&path, deep).unwrap();
    let r = s.execute("anim.applyPreset", json!({"path": path, "layers": [early]}));
    let _ = std::fs::remove_file(&path);
    assert!(r.unwrap_err().to_string().contains("not a preset"));
    unchanged(&s);

    // `late` starts before `early`, so only `late` has a layer-time playhead past 0.
    let cid = s.active_comp_id().unwrap();
    s.edit("Start Earlier", None, |proj, _| {
        proj.comp_mut(cid).unwrap().layer_mut(LayerId(late)).unwrap().start_time = Tick::from_seconds_f64(-1.0);
        Ok(())
    })
    .unwrap();
    let steps = s.history.undo.len();
    let mut last = animators(&s, source)[0].clone();
    last.walk_mut(&mut |p| {
        if p.match_id == "end" {
            p.keys = vec![Keyframe::new(Tick(i64::MAX), KV::Scalar(0.0))];
        }
    });
    let preset = json!({"effectcraftAnimationPreset": 3, "textAnimators": [last]});
    assert!(s.execute("anim.applyPreset", json!({"preset": preset, "layers": [early, late]})).is_err());
    assert_eq!((animators(&s, early), animators(&s, late)), (vec![], vec![]));
    assert_eq!(s.history.undo.len(), steps);
}

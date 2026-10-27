//! Paint and Puppet commands: undoable edits, options, rendering through the session.

use effectcraft_render::RenderOpts;
use effectcraft_time::Tick;
use serde_json::{Value, json};

use crate::Session;

/// 200×100 comp, 2 s at 30 fps, with a 100×60 blue solid centred (comp 50..150 × 20..80).
fn setup() -> (Session, u64) {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "C", "width": 200, "height": 100, "duration": 2, "frameRate": 30})).unwrap();
    let id = s.execute("layer.newSolid", json!({"width": 100, "height": 60, "color": [0, 0, 1]})).unwrap()["layer"].as_u64().unwrap();
    (s, id)
}

fn px(s: &Session, t: f64, x: i64, y: i64) -> [f32; 4] {
    let cid = s.active_comp_id().unwrap();
    s.render(cid, Tick::from_seconds_f64(t), RenderOpts::default()).get(x, y)
}

fn layer(s: &Session, id: u64) -> effectcraft_project::Layer {
    s.active_comp().unwrap().layer(effectcraft_project::LayerId(id)).unwrap().clone()
}

#[test]
fn brush_stroke_is_an_undoable_effect() {
    let (mut s, id) = setup();
    s.execute("paint.options", json!({"color": [1, 0, 0, 1], "diameter": 8, "sizePressure": false})).unwrap();
    let r = s.execute("paint.stroke", json!({"layer": id, "points": [[10, 30], [90, 30]]})).unwrap();
    let fx = r["effect"].as_u64().unwrap();
    let l = layer(&s, id);
    let g = l.props.find_group(fx).unwrap();
    assert_eq!(g.name, "Paint");
    assert_eq!(effectcraft_effects::paint::strokes(g).count(), 1);
    assert_eq!(g.groups().next().unwrap().name, "Brush 1");
    // Layer (10..90, 30) → comp (60..140, 50).
    assert_eq!(px(&s, 0.0, 100, 50), [1.0, 0.0, 0.0, 1.0]);
    assert_eq!(px(&s, 0.0, 100, 40), [0.0, 0.0, 1.0, 1.0]);
    // Second stroke goes into the same Paint effect.
    let r2 = s.execute("paint.stroke", json!({"layer": id, "points": [[50, 5]], "kind": "eraser", "diameter": 20})).unwrap();
    assert_eq!(r2["effect"].as_u64(), Some(fx));
    assert_eq!(px(&s, 0.0, 100, 25)[3], 0.0);
    assert!(s.undo());
    assert_eq!(px(&s, 0.0, 100, 25), [0.0, 0.0, 1.0, 1.0]);
    assert!(s.undo());
    assert!(layer(&s, id).effects().unwrap().children.is_empty());
    assert_eq!(px(&s, 0.0, 100, 50), [0.0, 0.0, 1.0, 1.0]);
    assert!(s.redo());
    assert_eq!(px(&s, 0.0, 100, 50), [1.0, 0.0, 0.0, 1.0]);
    // Another effect after Paint: the next stroke starts a new Paint instance.
    s.execute("effect.apply", json!({"layer": id, "effect": "Invert"})).unwrap();
    let r3 = s.execute("paint.stroke", json!({"layer": id, "points": [[20, 20]]})).unwrap();
    assert_ne!(r3["effect"].as_u64(), Some(fx));
    assert_eq!(layer(&s, id).props.find_group(r3["effect"].as_u64().unwrap()).unwrap().name, "Paint 2");
}

#[test]
fn durations_set_spans_and_write_on_keys() {
    let (mut s, id) = setup();
    s.set_time(Tick::from_seconds_f64(0.5));
    let r = s.execute("paint.stroke", json!({"layer": id, "points": [[10, 30], [90, 30]], "durationMode": "writeOn", "duration": 0.5})).unwrap();
    let l = layer(&s, id);
    let g = l.props.find_group(r["stroke"].as_u64().unwrap()).unwrap();
    let end = g.sub("stroke_options").unwrap().get("end").unwrap();
    assert_eq!(end.keys.len(), 2);
    assert_eq!(end.keys[0].time, Tick::from_seconds_f64(0.5));
    assert_eq!(end.keys[1].time, Tick::from_seconds_f64(1.0));
    // Written on over 0.5–1.0 s: invisible before, half at 0.75 s.
    assert_eq!(px(&s, 0.25, 100, 50), [0.0, 0.0, 1.0, 1.0]);
    assert_eq!(px(&s, 0.75, 70, 50)[1], 1.0);
    assert_eq!(px(&s, 0.75, 130, 50), [0.0, 0.0, 1.0, 1.0]);
    assert_eq!(px(&s, 1.5, 130, 50)[1], 1.0);

    // Single Frame: visible on its frame only.
    s.set_time(Tick::from_seconds_f64(1.0));
    s.execute("paint.stroke", json!({"layer": id, "points": [[50, 50]], "durationMode": "singleFrame", "color": [1, 0, 0, 1], "diameter": 6})).unwrap();
    assert_eq!(px(&s, 1.0, 100, 70), [1.0, 0.0, 0.0, 1.0]);
    assert_eq!(px(&s, 1.0 + 1.0 / 30.0, 100, 70), [0.0, 0.0, 1.0, 1.0]);
    assert_eq!(px(&s, 0.9, 100, 70), [0.0, 0.0, 1.0, 1.0]);
}

#[test]
fn eraser_last_stroke_only_targets_previous_stroke() {
    let (mut s, id) = setup();
    s.execute("paint.stroke", json!({"layer": id, "points": [[40, 30]], "color": [0, 1, 0, 1], "diameter": 30})).unwrap();
    let b = s.execute("paint.stroke", json!({"layer": id, "points": [[50, 30]], "color": [1, 0, 0, 1], "diameter": 30})).unwrap()["stroke"].as_u64().unwrap();
    let e = s.execute("paint.stroke", json!({"layer": id, "kind": "eraser", "eraseMode": "lastStrokeOnly", "points": [[50, 30]], "diameter": 10})).unwrap();
    let l = layer(&s, id);
    let eg = l.props.find_group(e["stroke"].as_u64().unwrap()).unwrap();
    assert_eq!(eg.get("target").unwrap().value.as_f64() as u64, b);
    assert_eq!(px(&s, 0.0, 100, 50), [0.0, 1.0, 0.0, 1.0]);
}

#[test]
fn clone_stroke_uses_source_point_and_aligned_offset() {
    let (mut s, id) = setup();
    // Paint a red dot at layer (20, 20), then clone it to (70, 40).
    s.execute("paint.stroke", json!({"layer": id, "points": [[20, 20]], "color": [1, 0, 0, 1], "diameter": 10, "sizePressure": false})).unwrap();
    s.execute("paint.setCloneSource", json!({"layer": id, "point": [20, 20]})).unwrap();
    let r = s.execute("paint.stroke", json!({"layer": id, "kind": "clone", "points": [[70, 40]], "diameter": 12})).unwrap();
    let l = layer(&s, id);
    let g = l.props.find_group(r["stroke"].as_u64().unwrap()).unwrap();
    assert_eq!(g.sub("stroke_options").unwrap().get("clone_position").unwrap().value.as_vec2(), [20.0, 20.0]);
    assert_eq!(s.state.paint.clone_offset, Some([-50.0, -20.0]));
    // Clone source is this layer before paint: blue copied over blue. With Paint on the
    // source layer itself the clone samples the unpainted source.
    assert_eq!(px(&s, 0.0, 120, 60), [0.0, 0.0, 1.0, 1.0]);
    // Aligned: the next stroke keeps the offset.
    let r = s.execute("paint.stroke", json!({"layer": id, "kind": "clone", "points": [[80, 45]]})).unwrap();
    let l = layer(&s, id);
    let g = l.props.find_group(r["stroke"].as_u64().unwrap()).unwrap();
    assert_eq!(g.sub("stroke_options").unwrap().get("clone_position").unwrap().value.as_vec2(), [30.0, 25.0]);
}

#[test]
fn clone_from_another_layer() {
    let (mut s, id) = setup();
    let red = s.execute("layer.newSolid", json!({"width": 100, "height": 60, "color": [1, 0, 0]})).unwrap()["layer"].as_u64().unwrap();
    // Hide the red layer by moving it out of view instead of relying on a switch command.
    s.execute("prop.set", json!({"layer": red, "path": "transform/position", "value": [1000, 1000, 0]})).unwrap();
    s.execute("paint.stroke", json!({"layer": id, "kind": "clone", "cloneSource": red, "clonePosition": [50, 30], "points": [[50, 30]], "diameter": 10}))
        .unwrap();
    assert_eq!(px(&s, 0.0, 100, 50), [1.0, 0.0, 0.0, 1.0]);
    assert_eq!(px(&s, 0.0, 70, 50), [0.0, 0.0, 1.0, 1.0]);
}

#[test]
fn paint_options_and_presets() {
    let mut s = Session::default();
    let o = s.execute("paint.options", json!({"opacity": 50, "mode": "Multiply", "channels": "RGB", "durationMode": "custom", "customFrames": 5})).unwrap();
    assert_eq!(o["opacity"], json!(50.0));
    assert_eq!(s.state.paint.mode, 2);
    assert_eq!(s.state.paint.channels, 1);
    assert_eq!(s.state.paint.duration, 3);
    assert!(s.execute("paint.options", json!({"mode": "Nope"})).is_err());
    let r = s.execute("paint.brushPreset", json!({"preset": "Soft Round 45 px"})).unwrap();
    assert_eq!(r["preset"], json!(13));
    assert_eq!(s.state.paint.diameter, 45.0);
    assert_eq!(s.state.paint.hardness, 0.0);
    let list = s.execute("paint.presets", json!({})).unwrap();
    assert!(list.as_array().unwrap().len() >= 20);
    // State is serde (agents read it).
    let j = serde_json::to_value(&s.state).unwrap();
    let back: crate::EditorState = serde_json::from_value(j).unwrap();
    assert_eq!(back.paint, s.state.paint);
}

fn pin_count(s: &Session, id: u64) -> usize {
    let l = layer(s, id);
    l.effects().unwrap().groups().map(effectcraft_effects::puppet::pin_count).sum()
}

#[test]
fn puppet_pins_deform_and_undo() {
    let (mut s, id) = setup();
    let a = s.execute("puppet.addPin", json!({"layer": id, "position": [10, 30]})).unwrap();
    let b = s.execute("puppet.addPin", json!({"layer": id, "position": [90, 30]})).unwrap();
    assert_eq!(a["mesh"], b["mesh"], "second pin lands on the same mesh");
    assert_eq!(pin_count(&s, id), 2);
    let l = layer(&s, id);
    let pin = l.props.find_group(a["pin"].as_u64().unwrap()).unwrap();
    assert_eq!(pin.name, "Puppet Pin 1");
    assert_eq!(pin.get("position").unwrap().keys.len(), 1, "position pins are keyframed");
    // Pins at rest: the layer looks the same.
    assert_eq!(px(&s, 0.0, 100, 50), [0.0, 0.0, 1.0, 1.0]);
    let info = s.execute("puppet.info", json!({"layer": id})).unwrap();
    assert!(info["meshes"][0]["triangles"].as_u64().unwrap() > 50);
    // Move both pins down 15 px at 1 s: the whole layer follows.
    s.set_time(Tick::from_seconds_f64(1.0));
    s.execute("puppet.movePin", json!({"layer": id, "pin": a["pin"], "position": [10, 45]})).unwrap();
    s.execute("puppet.movePin", json!({"layer": id, "pin": "Puppet Pin 2", "position": [90, 45]})).unwrap();
    let l = layer(&s, id);
    assert_eq!(l.props.find_group(a["pin"].as_u64().unwrap()).unwrap().get("position").unwrap().keys.len(), 2);
    assert_eq!(px(&s, 1.0, 100, 90), [0.0, 0.0, 1.0, 1.0]);
    assert_eq!(px(&s, 1.0, 100, 25)[3], 0.0);
    assert_eq!(px(&s, 0.0, 100, 25), [0.0, 0.0, 1.0, 1.0]);
    // A third pin added while deformed maps back to the rest mesh.
    let c = s.execute("puppet.addPin", json!({"layer": id, "position": [50, 45]})).unwrap();
    let rest = c["rest"].as_array().unwrap();
    assert!((rest[1].as_f64().unwrap() - 30.0).abs() < 0.5, "{rest:?}");
    s.undo();
    s.undo();
    s.undo();
    assert_eq!(pin_count(&s, id), 2);
    assert_eq!(px(&s, 1.0, 100, 25), [0.0, 0.0, 1.0, 1.0]);
}

#[test]
fn first_puppet_pin_leaves_the_layer_still() {
    // #313: an 800×600 solid in a 1920×1080 comp turned about the first pin at these spots.
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "C", "width": 1920, "height": 1080, "duration": 2, "frameRate": 30})).unwrap();
    let id = s.execute("layer.newSolid", json!({"width": 800, "height": 600, "color": [0, 0, 1]})).unwrap()["layer"].as_u64().unwrap();
    let cid = s.active_comp_id().unwrap();
    let before = s.render(cid, Tick::ZERO, RenderOpts::default());
    for at in [[40, 40], [400, 300], [123, 456]] {
        s.execute("puppet.addPin", json!({"layer": id, "position": at})).unwrap();
        let after = s.render(cid, Tick::ZERO, RenderOpts::default());
        let moved = after.data.iter().zip(&before.data).filter(|(a, b)| (0..4).any(|c| (a[c] - b[c]).abs() > 1e-3)).count();
        assert_eq!(moved, 0, "pin at {at:?} changed {moved} pixels");
        s.undo();
    }
}

/// #318: a pin added to an already-bent mesh holds the mesh where it is: nothing jumps.
#[test]
fn a_pin_added_to_a_bent_mesh_moves_nothing() {
    let (mut s, id) = setup();
    s.execute("puppet.addPin", json!({"layer": id, "position": [5, 30]})).unwrap();
    s.execute("puppet.addPin", json!({"layer": id, "position": [95, 30]})).unwrap();
    s.set_time(Tick::from_seconds_f64(1.0));
    s.execute("puppet.movePin", json!({"layer": id, "pin": "Puppet Pin 2", "position": [80, 70]})).unwrap();
    let cid = s.active_comp_id().unwrap();
    let t = Tick::from_seconds_f64(1.0);
    let before = s.render(cid, t, RenderOpts::default());
    for at in [[50, 54], [30, 42], [70, 62]] {
        s.execute("puppet.addPin", json!({"layer": id, "position": at})).unwrap();
        let after = s.render(cid, t, RenderOpts::default());
        let moved = after.data.iter().zip(&before.data).filter(|(a, b)| (a[3] - b[3]).abs() > 0.25).count();
        // (Before #318 the pin at [50, 54] moved about 250 edge pixels; the solver's round-off
        // still touches a few.)
        assert!(moved <= 16, "a pin at {at:?} moved {moved} pixels");
        s.undo();
    }
}

#[test]
fn puppet_pin_kinds_and_mesh_options() {
    let (mut s, id) = setup();
    for (k, pos) in [("position", [10, 30]), ("starch", [50, 30]), ("bend", [90, 30]), ("overlap", [30, 30]), ("advanced", [70, 30])] {
        s.execute("puppet.addPin", json!({"layer": id, "kind": k, "position": pos})).unwrap();
    }
    let l = layer(&s, id);
    let names: Vec<String> = {
        let fx = l.effects().unwrap().groups().next().unwrap();
        effectcraft_effects::puppet::meshes(fx).flat_map(|m| effectcraft_effects::puppet::pins(m).map(|p| p.name.clone())).collect()
    };
    assert_eq!(names, ["Puppet Pin 1", "Starch 1", "Puppet Pin 2", "Overlap 1", "Puppet Pin 3"]);
    assert!(s.execute("puppet.movePin", json!({"layer": id, "pin": "Puppet Pin 2", "position": [1, 1]})).is_err(), "bend pins have no position");
    s.execute("puppet.setPin", json!({"layer": id, "pin": "Puppet Pin 2", "rotation": 30})).unwrap();
    s.execute("puppet.setPin", json!({"layer": id, "pin": "Starch 1", "amount": 80, "extent": 25})).unwrap();
    s.execute("puppet.mesh", json!({"layer": id, "density": 80, "expansion": 5})).unwrap();
    let l = layer(&s, id);
    let fx = l.effects().unwrap().groups().next().unwrap();
    let m = effectcraft_effects::puppet::meshes(fx).next().unwrap();
    assert_eq!(m.get("density").unwrap().value.as_f64(), 80.0);
    assert_eq!(m.get("expansion").unwrap().value.as_f64(), 5.0);
    // Renders, deterministically.
    let cid = s.active_comp_id().unwrap();
    let a = s.render(cid, Tick::ZERO, RenderOpts::default());
    let b = s.render(cid, Tick::ZERO, RenderOpts::default());
    assert_eq!(a, b);
    s.execute("puppet.removePin", json!({"layer": id, "pin": "Overlap 1"})).unwrap();
    assert_eq!(pin_count(&s, id), 4);
}

#[test]
fn puppet_pins_select_and_delete_without_touching_the_layer() {
    let (mut s, id) = setup();
    let a = s.execute("puppet.addPin", json!({"layer": id, "position": [10, 30]})).unwrap()["pin"].clone();
    let b = s.execute("puppet.addPin", json!({"layer": id, "position": [90, 30]})).unwrap()["pin"].clone();
    s.execute("puppet.addPin", json!({"layer": id, "position": [50, 30]})).unwrap();
    // A new pin is selected: Delete removes it, and the layer stays.
    assert_eq!(s.execute("edit.clear", json!({})).unwrap(), json!({"removed": 1}));
    assert_eq!(pin_count(&s, id), 2);
    assert!(s.active_comp().unwrap().layer(effectcraft_project::LayerId(id)).is_some());
    // Click, Shift-click (toggle) and add by name.
    let sel = |s: &mut Session, q: Value| s.execute("puppet.selectPins", q).unwrap()["pins"].clone();
    assert_eq!(sel(&mut s, json!({"layer": id, "pins": [a, b]})), json!([a, b]));
    assert_eq!(sel(&mut s, json!({"layer": id, "pins": [b], "toggle": true})), json!([a]));
    assert_eq!(sel(&mut s, json!({"layer": id, "pins": ["Puppet Pin 2"], "add": true})), json!([a, b]));
    assert!(s.execute("puppet.selectPins", json!({"layer": id, "pins": ["Puppet Pin 9"]})).is_err());
    // Deleting both is one undo step.
    assert_eq!(s.execute("edit.clear", json!({})).unwrap(), json!({"removed": 2}));
    assert_eq!(pin_count(&s, id), 0);
    s.undo();
    assert_eq!(pin_count(&s, id), 2);
    // Nothing selected: removing needs a pin.
    assert_eq!(sel(&mut s, json!({"layer": id, "pins": []})), json!([]));
    assert!(s.execute("puppet.removePin", json!({"layer": id})).is_err());
}

#[test]
fn puppet_select_all_takes_the_pin_kind_and_several_pins_record_together() {
    let (mut s, id) = setup();
    let a = s.execute("puppet.addPin", json!({"layer": id, "position": [10, 30]})).unwrap()["pin"].clone();
    let b = s.execute("puppet.addPin", json!({"layer": id, "position": [90, 30]})).unwrap()["pin"].clone();
    let st = s.execute("puppet.addPin", json!({"layer": id, "kind": "starch", "position": [50, 30]})).unwrap()["pin"].clone();
    // Ctrl+A with a Position pin selected: every Position pin, not the Starch pin.
    s.execute("puppet.selectPins", json!({"layer": id, "pins": [a]})).unwrap();
    assert_eq!(s.execute("edit.selectAll", json!({})).unwrap()["pins"], json!([a, b]));
    s.execute("puppet.selectPins", json!({"layer": id, "pins": [st]})).unwrap();
    assert_eq!(s.execute("edit.selectAll", json!({})).unwrap()["pins"], json!([st]));
    // Without pins selected, Select All selects layers as before.
    s.execute("puppet.selectPins", json!({"layer": id, "pins": []})).unwrap();
    assert_eq!(s.execute("edit.selectAll", json!({})).unwrap(), json!(1));
    // Recording a drag of pin A with B selected too: B moves by the same displacement.
    let steps = s.history.undo.len();
    let samples: Vec<Value> = [0.0, 0.25, 0.5].iter().map(|t| json!([t, 10.0, 30.0 + 40.0 * t])).collect();
    let r = s.execute("puppet.recordPin", json!({"layer": id, "pin": a, "pins": [b], "samples": samples, "smoothing": 0})).unwrap();
    assert_eq!(r["pins"], json!([a, b]));
    assert_eq!(s.history.undo.len(), steps + 1, "one undo step");
    let last = |s: &Session, pin: &Value| pin_keys(s, id, pin.as_u64().unwrap()).last().unwrap().1;
    assert_eq!(last(&s, &a), [10.0, 50.0]);
    assert_eq!(last(&s, &b), [90.0, 50.0]);
    // Starch pins don't record.
    assert!(s.execute("puppet.recordPin", json!({"layer": id, "pin": a, "pins": [st], "samples": samples})).is_err());
}

fn expr_of(s: &Session, layer: u64, uid: u64) -> String {
    let l = layer_of(s, layer);
    l.props.find(uid).and_then(|p| p.expr.as_ref()).map(|e| e.text.clone()).unwrap_or_default()
}

fn layer_of(s: &Session, id: u64) -> effectcraft_project::Layer {
    s.active_comp().unwrap().layer(effectcraft_project::LayerId(id)).unwrap().clone()
}

#[test]
fn puppet_pins_rig_to_nulls_both_ways() {
    let (mut s, id) = setup();
    s.execute("puppet.addPin", json!({"layer": id, "position": [10, 30]})).unwrap();
    s.execute("puppet.addPin", json!({"layer": id, "position": [90, 30]})).unwrap();
    let bend = s.execute("puppet.addPin", json!({"layer": id, "kind": "bend", "position": [50, 30]})).unwrap()["pin"].clone();
    // Only a Bend pin selected: nothing to rig.
    assert!(s.execute("paths.pointsFollowNulls", json!({})).is_err());
    // No pin selected and no path: every Position pin of the layer gets a null on it, and the
    // pin's Position follows the null; one undo step.
    s.execute("puppet.selectPins", json!({"layer": id, "pins": []})).unwrap();
    let steps = s.history.undo.len();
    let r = s.execute("paths.pointsFollowNulls", json!({"layer": id})).unwrap();
    assert_eq!(s.history.undo.len(), steps + 1);
    let nulls: Vec<u64> = r["nulls"].as_array().unwrap().iter().map(|v| v.as_u64().unwrap()).collect();
    let pins: Vec<u64> = r["pins"].as_array().unwrap().iter().map(|v| v.as_u64().unwrap()).collect();
    assert_eq!((nulls.len(), pins.len()), (2, 2));
    let solid = layer_of(&s, id).name;
    let null = layer_of(&s, nulls[0]);
    assert_eq!(null.name, format!("{solid}: Puppet Pin 1"));
    // Layer (10, 30) is comp (60, 50).
    assert_eq!(null.props.prop("transform/position").unwrap().value.components()[..2], [60.0, 50.0]);
    let e = expr_of(&s, id, pins[0]);
    assert!(e.contains(&format!("thisComp.layer(\"{solid}: Puppet Pin 1\")")) && e.contains("fromComp(n.toComp(n.transform.anchorPoint))"), "{e}");
    // Nulls Follow Points: a null riding on a pin, through the pin's Position.
    let r = s.execute("paths.nullsFollowPoints", json!({"layer": id, "pins": ["Puppet Pin 2"]})).unwrap();
    let rider = layer_of(&s, r["nulls"][0].as_u64().unwrap());
    let e = rider.props.prop("transform/position").unwrap().expr.as_ref().unwrap().text.clone();
    assert!(e.contains(&format!("thisComp.layer(\"{solid}\")")) && e.contains("(\"Puppet Pin 2\")(\"Position\")") && e.contains("toComp"), "{e}");
    assert!(s.execute("paths.nullsFollowPoints", json!({"layer": id, "pins": [bend]})).is_err());
    // Trace Path still needs a path.
    assert!(s.execute("paths.tracePath", json!({"layer": id})).is_err());
}

#[test]
fn puppet_follow_through_trails_the_leader_by_cascading_delays() {
    let (mut s, id) = setup();
    let lead = s.execute("puppet.addPin", json!({"layer": id, "position": [10, 30]})).unwrap()["pin"].clone();
    let far = s.execute("puppet.addPin", json!({"layer": id, "position": [90, 30]})).unwrap()["pin"].clone();
    let near = s.execute("puppet.addPin", json!({"layer": id, "position": [50, 30]})).unwrap()["pin"].clone();
    let pos_uid = |s: &Session, pin: &Value| layer_of(s, id).props.find_group(pin.as_u64().unwrap()).unwrap().get("position").unwrap().uid;
    // Leader first, then the followers (in any order): nearer pins trail less.
    s.execute("puppet.selectPins", json!({"layer": id, "pins": [lead, far, near]})).unwrap();
    let steps = s.history.undo.len();
    let r = s.execute("puppet.follow", json!({"delay": 0.2})).unwrap();
    assert_eq!(s.history.undo.len(), steps + 1, "one undo step");
    assert_eq!(r["leader"], lead);
    assert_eq!(r["pins"], json!([{"pin": near, "delay": 0.2}, {"pin": far, "delay": 0.4}]));
    let e = expr_of(&s, id, pos_uid(&s, &far));
    assert!(e.contains("(\"Puppet Pin 1\")(\"Position\")") && e.contains("valueAtTime(time - 0.4)") && e.contains("- [10, 30]) * 1"), "{e}");
    // Without cascade every follower trails by the same delay; `amount` scales the motion.
    s.execute("puppet.follow", json!({"layer": id, "leader": "Puppet Pin 1", "pins": [far, near], "delay": 0.25, "amount": 50, "cascade": false})).unwrap();
    assert!(expr_of(&s, id, pos_uid(&s, &far)).contains("valueAtTime(time - 0.25)") && expr_of(&s, id, pos_uid(&s, &far)).contains("* 0.5"));
    // Errors: no followers, or a pin without a Position.
    assert!(s.execute("puppet.follow", json!({"layer": id, "leader": "Puppet Pin 1", "pins": [lead]})).is_err());
    let bend = s.execute("puppet.addPin", json!({"layer": id, "kind": "bend", "position": [30, 30]})).unwrap()["pin"].clone();
    assert!(s.execute("puppet.follow", json!({"layer": id, "leader": "Puppet Pin 1", "pins": [bend]})).is_err());
}

#[test]
fn project_with_paint_and_puppet_round_trips() {
    let (mut s, id) = setup();
    s.execute("paint.stroke", json!({"layer": id, "points": [[10, 30, 0.5], [90, 30, 1.0]], "durationMode": "writeOn"})).unwrap();
    s.execute("puppet.addPin", json!({"layer": id, "position": [10, 30]})).unwrap();
    let j = s.project.to_json();
    let back = effectcraft_project::Project::from_json(&j).unwrap();
    assert_eq!(&back, s.project.as_ref());
    let v: Value = serde_json::from_str(&j).unwrap();
    assert!(v.to_string().contains("ec.paint.paint") && v.to_string().contains("ec.distort.puppet"));
}

#[test]
fn liquify_stroke_adds_the_effect_and_warps() {
    let (mut s, id) = setup();
    // Layer (5, 30) is comp (55, 50). Before: opaque blue.
    assert!(px(&s, 0.0, 55, 50)[3] > 0.99);
    let r = s.execute("liquify.stroke", json!({"layer": id, "tool": "warp", "points": [[-20, 30], [10, 30]], "size": 40, "pressure": 100})).unwrap();
    assert_eq!(r["strokes"], 1);
    let l = layer(&s, id);
    let fx = l.effects().unwrap().groups().next().unwrap().clone();
    assert!(matches!(&fx.kind, effectcraft_project::GroupKind::Effect { effect } if effect == "ec.distort.liquify"));
    // The warp pulls transparent pixels from outside the layer over its left edge.
    assert!(px(&s, 0.0, 55, 50)[3] < 0.9, "{:?}", px(&s, 0.0, 55, 50));
    // A second stroke goes into the same effect.
    let r2 = s.execute("liquify.stroke", json!({"layer": id, "tool": "reconstruction", "points": [[0, 30]], "size": 60, "pressure": 100})).unwrap();
    assert_eq!(r2["strokes"], 2);
    assert_eq!(r2["effect"], r["effect"]);
    assert!(s.execute("liquify.stroke", json!({"layer": id, "tool": "smudge", "points": [[0, 0]]})).is_err());
    s.execute("edit.undo", json!({})).unwrap();
    s.execute("edit.undo", json!({})).unwrap();
    assert!(layer(&s, id).effects().unwrap().groups().next().is_none());
    assert!(px(&s, 0.0, 55, 50)[3] > 0.99);
}

fn pin_keys(s: &Session, id: u64, pin: u64) -> Vec<(f64, [f64; 2])> {
    let l = layer(s, id);
    let pos = l.props.find_group(pin).unwrap().get("position").unwrap().clone();
    pos.keys.iter().map(|k| (k.time.seconds(), k.value.as_vec2())).collect()
}

#[test]
fn puppet_pin_recording_makes_keys_at_the_frame_rate() {
    let (mut s, id) = setup();
    let a = s.execute("puppet.addPin", json!({"layer": id, "position": [10, 30]})).unwrap();
    s.execute("puppet.addPin", json!({"layer": id, "position": [90, 30]})).unwrap();
    let pin = a["pin"].as_u64().unwrap();
    s.set_time(Tick::from_seconds_f64(0.5));
    // A 0.5 s drag sampled at irregular wall-clock times: right 30 px, linear in time.
    let samples: Vec<Value> = [0.0, 0.07, 0.2, 0.33, 0.41, 0.5].iter().map(|t| json!([t, 10.0 + 60.0 * t, 30.0])).collect();
    let r = s.execute("puppet.recordPin", json!({"layer": id, "pin": pin, "samples": samples, "smoothing": 0})).unwrap();
    // 0.5 s at 30 fps from 0.5 s: frames 15..=30, one key per frame.
    assert_eq!(r["frames"], 16, "{r}");
    let keys = pin_keys(&s, id, pin);
    let rec: Vec<&(f64, [f64; 2])> = keys.iter().filter(|k| k.0 >= 0.5 - 1e-9).collect();
    assert_eq!(rec.len(), 16, "{keys:?}");
    for (i, (t, v)) in rec.iter().enumerate() {
        assert!((t - (0.5 + i as f64 / 30.0)).abs() < 1e-6, "{t}");
        assert!((v[0] - (10.0 + 60.0 * i as f64 / 30.0)).abs() < 1e-6, "{v:?}");
    }
    assert_eq!(keys.len(), 17, "the pin's first key (0 s) is kept");
    assert_eq!(s.time().seconds(), 0.5, "the current time returns to where recording began");
    // The recording deforms the layer: by 1 s the left edge has followed the pin 30 px right.
    assert!(px(&s, 0.0, 55, 50)[3] > 0.99);
    assert!(px(&s, 1.0, 55, 50)[3] < 0.5);
    // Speed 200 %: the same drag plays back over twice the frames; recording again replaces
    // the keys in the span.
    let r = s.execute("puppet.recordPin", json!({"layer": id, "pin": pin, "samples": samples, "speed": 200, "smoothing": 0})).unwrap();
    assert_eq!(r["frames"], 31);
    let keys = pin_keys(&s, id, pin);
    assert_eq!(keys.len(), 32);
    let k = keys.iter().find(|k| (k.0 - (0.5 + 10.0 / 30.0)).abs() < 1e-6).unwrap();
    assert!((k.1[0] - (10.0 + 60.0 * 5.0 / 30.0)).abs() < 1e-6, "{k:?}");
    // Smoothing removes the extraneous keys of a straight-line drag.
    let r = s.execute("puppet.recordPin", json!({"layer": id, "pin": pin, "samples": samples})).unwrap();
    assert!(r["keys"].as_u64().unwrap() < 6, "{r}");
    // One undo step per recording.
    s.undo();
    assert_eq!(pin_keys(&s, id, pin).len(), 32);
    // Record Options are tool state (serde for agents).
    let o = s.execute("puppet.recordOptions", json!({"speed": 50, "smoothing": 0, "useDraftDeformation": true, "showMesh": true})).unwrap();
    assert_eq!(o["speed"], 50.0);
    assert!(s.state.puppet.record_draft && s.state.puppet.record_show_mesh);
    let r = s.execute("puppet.recordPin", json!({"layer": id, "pin": pin, "samples": samples})).unwrap();
    assert_eq!(r["frames"], 9, "50 %: half the frames");
    assert!(s.execute("puppet.recordPin", json!({"layer": id, "pin": pin, "samples": []})).is_err());
}

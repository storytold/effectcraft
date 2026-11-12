//! Warp Stabilizer and 3D Camera Tracker analyses follow their input: the layer's frames, its
//! masks, the effects above them and a precomp's contents (#3535). Edits that leave the input
//! as it was (effects below, names, labels) keep them.

use effectcraft_effects::camera_tracker::{TRACKS, TRACKS_KEY};
use effectcraft_effects::warp_stab::{ANALYSIS, ANALYSIS_KEY};
use effectcraft_keyframe::Value;
use effectcraft_project::{ItemId, LayerId, Project};
use serde_json::{Value as Json, json};

use crate::Session;

const COMP: ItemId = ItemId(2);
const CLIP: LayerId = LayerId(3);
const WARP: u64 = 26;
const TRACKER: u64 = 54;

/// `analysed-0.24.ecproj`: the clip of `tracked-0.22.ecproj` with a mask and, in order, a Warp Stabilizer, a 3D Camera
/// Tracker, a Gaussian Blur and a Roto Brush.
const ANALYSED: &str = include_str!("fixtures/analysed-0.24.ecproj");

fn open(json: &str) -> Session {
    let mut s = Session::default();
    s.replace_project(Project::from_json(json).unwrap(), None);
    s.open_comp(COMP);
    s
}

/// Which keys an analysis saves.
#[derive(Clone, Copy, Debug)]
enum Saved {
    /// Today's: the input to the instance.
    Input,
    /// Before #3535: the layer's frames only (still recognised).
    Frames,
}

/// The key an analysis of instance `uid` (no Detailed Analysis) saves.
fn key(p: &Project, uid: u64, saved: Saved) -> String {
    let layer = p.comp(COMP).unwrap().layer(CLIP).unwrap();
    match saved {
        Saved::Input => crate::analysis::frames_key::analysis_key(p, COMP, layer, uid),
        Saved::Frames => crate::analysis::frames_key::signature(p, COMP, layer),
    }
}

/// Give the Warp Stabilizer and the tracker an analysis of the clip as it is now, saved with
/// `saved` keys, and reopen the project.
fn analyse(s: &mut Session, saved: Saved) {
    let mut p = (*s.project).clone();
    let set = |p: &mut Project, uid: u64, data: &str, id: &str, key: &str| {
        let g = p.comp_mut(COMP).unwrap().layer_mut(CLIP).unwrap().props.find_group_mut(uid).unwrap();
        g.prop_mut(data).unwrap().value = Value::Str(format!("saved {data}"));
        g.prop_mut(id).unwrap().value = Value::Str(key.into());
    };
    // The Warp Stabilizer is above the tracker: its analysis is part of the tracker's input.
    let k = key(&p, WARP, saved);
    set(&mut p, WARP, ANALYSIS, ANALYSIS_KEY, &k);
    let k = key(&p, TRACKER, saved);
    set(&mut p, TRACKER, TRACKS, TRACKS_KEY, &k);
    *s = open(&p.to_json());
    run(s, "renderQueue.add", json!({"comp": COMP.0, "format": "png", "output": "frame.png"}));
    assert_eq!(analysed(s), [true, true], "{saved:?}");
}

/// Whether the Warp Stabilizer and the tracker still have their analysis.
fn analysed(s: &Session) -> [bool; 2] {
    let layer = s.project.comp(COMP).unwrap().layer(CLIP).unwrap();
    let has = |uid: u64, id: &str| matches!(&layer.props.find_group(uid).unwrap().prop(id).unwrap().value, Value::Str(s) if !s.is_empty());
    [has(WARP, ANALYSIS), has(TRACKER, TRACKS)]
}

fn run(s: &mut Session, cmd: &str, params: Json) -> Json {
    s.execute_checked(cmd, params).unwrap()
}

fn undo(s: &mut Session) {
    run(s, "edit.undo", json!({}));
}

/// An edit, its effect on the two analyses, and that undo brings them back.
fn check(s: &mut Session, cmd: &str, params: Json, kept: [bool; 2]) {
    assert_eq!(analysed(s), [true, true], "before {cmd} {params}");
    run(s, cmd, params.clone());
    assert_eq!(analysed(s), kept, "{cmd} {params}");
    undo(s);
    assert_eq!(analysed(s), [true, true], "undo of {cmd} {params}");
}

#[test]
fn masks_and_effects_above_are_part_of_the_input() {
    for saved in [Saved::Input, Saved::Frames] {
        masks_and_effects_above(saved);
    }
}

fn masks_and_effects_above(saved: Saved) {
    let mut s = open(ANALYSED);
    analyse(&mut s, saved);
    // The layer's mask is under every effect.
    check(&mut s, "prop.set", json!({"layer": CLIP.0, "path": "masks/#1/expansion", "value": 3}), [false, false]);
    // The Warp Stabilizer is above the tracker: its settings change the tracker's input only.
    check(&mut s, "prop.set", json!({"layer": CLIP.0, "path": "effects/#1/stabilization/smoothness", "value": 20}), [true, false]);
    // The Gaussian Blur and the Roto Brush are below both.
    check(&mut s, "prop.set", json!({"layer": CLIP.0, "path": "effects/#3/blurriness", "value": 4}), [true, true]);
    run(&mut s, "layer.rename", json!({"layer": CLIP.0, "name": "Renamed"}));
    assert_eq!(analysed(&s), [true, true]);
    // The Effects switch turns the Warp Stabilizer (above the tracker) off.
    check(&mut s, "layer.setSwitch", json!({"layers": [CLIP.0], "switch": "effects", "value": false}), [true, false]);
    // While it is off, the effects above draw nothing: editing them keeps the analyses.
    run(&mut s, "layer.setSwitch", json!({"layers": [CLIP.0], "switch": "effects", "value": false}));
    analyse(&mut s, saved);
    check(&mut s, "prop.set", json!({"layer": CLIP.0, "path": "effects/#1/stabilization/smoothness", "value": 20}), [true, true]);
    run(&mut s, "layer.setSwitch", json!({"layers": [CLIP.0], "switch": "effects", "value": true}));
    assert_eq!(analysed(&s), [true, false]);
    analyse(&mut s, saved);

    // An effect moved above the Warp Stabilizer is part of both inputs, and its settings too.
    s.edit("blur first", None, |p, _| {
        let fx = p.comp_mut(COMP).unwrap().layer_mut(CLIP).unwrap().props.sub_mut("effects").unwrap();
        let blur = fx.children.remove(2);
        fx.children.insert(0, blur);
        Ok(())
    })
    .unwrap();
    assert_eq!(analysed(&s), [false, false]);
    analyse(&mut s, saved);
    check(&mut s, "prop.set", json!({"layer": CLIP.0, "path": "effects/#1/blurriness", "value": 4}), [false, false]);

    // Saved and reopened, they still match, and a mask edit still clears them.
    let mut s = open(&s.project.to_json());
    run(&mut s, "renderQueue.add", json!({"comp": COMP.0, "format": "png", "output": "frame.png"}));
    assert_eq!(analysed(&s), [true, true]);
    check(&mut s, "prop.set", json!({"layer": CLIP.0, "path": "masks/#1/expansion", "value": 3}), [false, false]);
}
#[test]
fn a_precomps_contents_are_part_of_the_input() {
    for saved in [Saved::Input, Saved::Frames] {
        precomp_contents(saved);
    }
}

fn precomp_contents(saved: Saved) {
    let mut s = open(ANALYSED);
    // Pre-compose the clip, leaving its masks and effects on the precomp layer.
    let r = run(&mut s, "layer.precompose", json!({"layers": [CLIP.0], "mode": "leave"}));
    let inner = ItemId(r["comp"].as_u64().unwrap());
    let inner_layer = |s: &Session, comp: ItemId| s.project.comp(comp).unwrap().layers[0].id.0;
    let clip = inner_layer(&s, inner);
    analyse(&mut s, saved);
    // Edits inside the precomp change the frames.
    check(&mut s, "prop.set", json!({"comp": inner.0, "layer": clip, "path": "transform/opacity", "value": 50}), [false, false]);
    check(&mut s, "effect.apply", json!({"comp": inner.0, "layers": [clip], "effect": "ec.blur.gaussian"}), [false, false]);
    check(&mut s, "layer.timing", json!({"comp": inner.0, "layers": [clip], "out": 0.25}), [false, false]);
    // A label or comment inside it doesn't. A name does: expressions find layers by name.
    s.edit("label", None, |p, _| {
        let l = p.comp_mut(inner).unwrap().layer_mut(LayerId(clip)).unwrap();
        l.label = effectcraft_color::Label::Pink;
        l.comment = "a note".into();
        Ok(())
    })
    .unwrap();
    assert_eq!(analysed(&s), [true, true]);
    check(&mut s, "layer.rename", json!({"comp": inner.0, "layer": clip, "name": "Inner"}), [false, false]);

    // A precomp inside the precomp: its contents too.
    let r = run(&mut s, "layer.precompose", json!({"comp": inner.0, "layers": [clip], "mode": "leave"}));
    let deeper = ItemId(r["comp"].as_u64().unwrap());
    assert_eq!(analysed(&s), [false, false], "pre-composing inside the precomp changes its contents");
    analyse(&mut s, saved);
    let clip = inner_layer(&s, deeper);
    check(&mut s, "prop.set", json!({"comp": deeper.0, "layer": clip, "path": "transform/opacity", "value": 50}), [false, false]);

    // Saved and reopened, they still match, and an edit inside still clears them.
    let mut s = open(&s.project.to_json());
    run(&mut s, "renderQueue.add", json!({"comp": COMP.0, "format": "png", "output": "frame.png"}));
    assert_eq!(analysed(&s), [true, true]);
    check(&mut s, "prop.set", json!({"comp": deeper.0, "layer": clip, "path": "transform/opacity", "value": 50}), [false, false]);
}

/// Precomps used many times, nested deeply or (in a broken project) in a cycle are keyed in
/// bounded time, and an edit deep inside still changes the key.
#[test]
fn shared_deep_and_cyclic_precomps_are_keyed_in_bounded_time() {
    use effectcraft_color::Label;
    use effectcraft_project::{ItemKind, LayerSource};
    let mut s = open(ANALYSED);
    let r = run(&mut s, "layer.precompose", json!({"layers": [CLIP.0], "mode": "leave"}));
    let inner = ItemId(r["comp"].as_u64().unwrap());
    let mut p = (*s.project).clone();
    let template = p.comp(inner).unwrap().clone();
    // 40 comps, each drawing the next one twice (2^40 paths if each were walked), the last
    // drawing the first.
    let ids: Vec<ItemId> = (0..40).map(|i| p.add_item(&format!("Nest {i}"), Label::default(), None, ItemKind::Comp(template.clone().into()))).collect();
    for (i, id) in ids.iter().enumerate() {
        let next = ids[(i + 1) % ids.len()];
        let mut layer = template.layers[0].clone();
        layer.source = LayerSource::Comp { item: next };
        let c = p.comp_mut(*id).unwrap();
        c.layers.push(layer.clone());
        layer.id = LayerId(layer.id.0 + 1000);
        c.layers.push(layer);
    }
    p.comp_mut(inner).unwrap().layers[0].source = LayerSource::Comp { item: ids[0] };
    let key = |p: &Project| crate::analysis::frames_key::analysis_key(p, COMP, p.comp(COMP).unwrap().layer(CLIP).unwrap(), WARP);
    let start = std::time::Instant::now();
    let before = key(&p);
    assert!(start.elapsed() < std::time::Duration::from_secs(2), "{:?}", start.elapsed());
    p.comp_mut(ids[10]).unwrap().background = [1.0, 0.0, 0.0];
    assert_ne!(key(&p), before);
}

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

/// `tracked-0.22.ecproj`'s clip with a mask and, in order, a Warp Stabilizer, a 3D Camera
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

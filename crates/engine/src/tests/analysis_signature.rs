//! Warp Stabilizer, 3D Camera Tracker and Roto Brush keys saved by 0.24 keep matching their
//! layer's frames after edits that don't change them, and a real change still clears them.

use effectcraft_effects::camera_tracker::{TRACKS, TRACKS_KEY};
use effectcraft_effects::roto::{FROZEN, INPUT_KEY};
use effectcraft_effects::warp_stab::{ANALYSIS, ANALYSIS_KEY};
use effectcraft_keyframe::Value;
use effectcraft_project::{ItemId, LayerId, Project};
use serde_json::json;

use crate::Session;

const COMP: ItemId = ItemId(2);
const CLIP: LayerId = LayerId(3);
const WARP: u64 = 26;
const TRACKER: u64 = 54;
const ROTO: u64 = 86;

/// Saved by 0.24.1 (before #3534/#3536): the clip of `tracked-0.22.ecproj` with Time Remap, a
/// mask, a Gaussian Blur and a Roto Brush, its analyses replaced by short stand-ins. The keys
/// are the ones that version computed for it.
const ANALYSED_0_24: &str = include_str!("fixtures/analysed-0.24.ecproj");
const SIGNATURE_0_24: &str = "03efb7c4b0f27b65";
const ROTO_INPUT_0_24: &str = "7b319ce93027617c";

fn open(json: &str) -> Session {
    let mut s = Session::default();
    s.replace_project(Project::from_json(json).unwrap(), None);
    s.open_comp(COMP);
    s
}

/// The (data, key) of the Warp Stabilizer, the 3D Camera Tracker and the Roto Brush.
fn saved(s: &Session) -> [(String, String); 3] {
    let layer = s.project.comp(COMP).unwrap().layer(CLIP).unwrap();
    let get = |uid: u64, data: &str, key: &str| {
        let params = crate::camera_track::static_params(layer.props.find_group(uid).unwrap());
        let text = |id: &str| match params.get(id) {
            Some(Value::Str(s)) => s.clone(),
            other => panic!("{id}: {other:?}"),
        };
        (text(data), text(key))
    };
    [get(WARP, ANALYSIS, ANALYSIS_KEY), get(TRACKER, TRACKS, TRACKS_KEY), get(ROTO, FROZEN, INPUT_KEY)]
}

fn add_to_render_queue(s: &mut Session) {
    s.execute_checked("renderQueue.add", json!({"comp": COMP.0, "format": "png", "output": "frame.png"})).unwrap();
}

#[test]
fn analyses_saved_by_0_24_survive_edits_that_keep_their_frames() {
    let mut s = open(ANALYSED_0_24);
    let before = saved(&s);
    assert_eq!(before[0], ("saved analysis".into(), SIGNATURE_0_24.into()));
    assert_eq!(before[1], ("saved tracks".into(), SIGNATURE_0_24.into()));
    assert_eq!(before[2], ("saved frozen".into(), ROTO_INPUT_0_24.into()));

    add_to_render_queue(&mut s);
    assert_eq!(saved(&s), before);
    s.execute_checked("edit.undo", json!({})).unwrap();
    assert_eq!(saved(&s), before);

    // Saved and reopened, the keys still match.
    let mut s = open(&s.project.to_json());
    add_to_render_queue(&mut s);
    assert_eq!(saved(&s), before);

    // Other frames: all three are cleared (the Roto Brush gets a new key), and undo brings them back.
    s.execute_checked("layer.timing", json!({"layer": CLIP.0, "out": 0.25})).unwrap();
    let after = saved(&s);
    assert_eq!(after[0], (String::new(), String::new()));
    assert_eq!(after[1], (String::new(), String::new()));
    assert_eq!(after[2].0, "");
    assert_ne!(after[2].1, ROTO_INPUT_0_24);
    s.execute_checked("edit.undo", json!({})).unwrap();
    assert_eq!(saved(&s), before);

    // Time Remap is part of the frames.
    s.execute_checked("prop.set", json!({"layer": CLIP.0, "path": "timeRemap", "value": 0.3})).unwrap();
    assert!(saved(&s).iter().all(|(data, _)| data.is_empty()));
    s.execute_checked("edit.undo", json!({})).unwrap();

    // The Roto Brush's input also has the layer's masks and the effects above it.
    s.execute_checked("prop.set", json!({"layer": CLIP.0, "path": "effects/#3/blurriness", "value": 4})).unwrap();
    let after = saved(&s);
    assert_eq!(after[2].0, "");
    assert_ne!(after[2].1, ROTO_INPUT_0_24);
    s.execute_checked("edit.undo", json!({})).unwrap();
    s.execute_checked("prop.set", json!({"layer": CLIP.0, "path": "masks/#1/expansion", "value": 3})).unwrap();
    assert_eq!(saved(&s)[2].0, "");
    s.execute_checked("edit.undo", json!({})).unwrap();
    assert_eq!(saved(&s), before);
}

/// The text keys are made from today, written field by field (#3536). It changes only when this
/// test is changed with it: a new field of a type it writes doesn't compile until it is decided
/// in `analysis::frames_key`, and renaming a field or variant doesn't change it.
#[test]
fn signature_text_is_pinned() {
    let s = open(ANALYSED_0_24);
    let layer = s.project.comp(COMP).unwrap().layer(CLIP).unwrap();
    let key = r#"frames/1;footage=1;footage;path="/tmp/effectcraft-3456-shake.mp4";kind=video;w=160;h=120;par=1.0;rate=12/1;nativeRate=none;duration=254016000000;video=true;audio=false;alpha=ignore;matte=[0.0, 0.0, 0.0];loop=1;codec="H.264 (High)";frames=0;profile{rec709;};layer=none;fields=off;invertAlpha=false;linearLight=false;preserveRgb=false;cineon;log=false;points=[0.0, 1023.0, 95.0, 685.0, 1.7, 20.0];units=10bit;data=none;page=0;alphabetical=false;startTimecode=none;fileTimecode=none;placedPage=false;in=0;out=254016000000;start=0;stretch=100.0;timeRemap{prop="timeRemap";scalar=0.0;spatial=false;key=0;scalar=0.2;interp=linear/linear;continuous=false;autoBezier=false;spatialIn=[0.0, 0.0, 0.0];spatialOut=[0.0, 0.0, 0.0];spatialAuto=true;spatialContinuous=false;roving=false;muted=false;key=254016000000;scalar=1.0;interp=linear/linear;continuous=false;autoBezier=false;spatialIn=[0.0, 0.0, 0.0];spatialOut=[0.0, 0.0, 0.0];spatialAuto=true;spatialContinuous=false;roving=false;muted=false;expr=none;};compRate{rate=12/1;};"#;
    assert_eq!(crate::analysis::frames_key::signature_text(&s.project, COMP, layer), key);
}

/// Keys made today (after analysing again) match through edits, undo and save/reopen too, and
/// renaming or relabelling never drops them.
#[test]
fn todays_keys_survive_edits_that_keep_their_frames() {
    let mut s = open(ANALYSED_0_24);
    // Analysing again writes today's keys.
    let layer = s.project.comp(COMP).unwrap().layer(CLIP).unwrap();
    let signature = crate::analysis::frames_key::signature(&s.project, COMP, layer);
    let input = crate::roto::input_key(&s.project, COMP, layer, ROTO);
    assert_ne!(signature, SIGNATURE_0_24);
    s.edit("analyse", None, |p, _| {
        let l = p.comp_mut(COMP).unwrap().layer_mut(CLIP).unwrap();
        for (uid, id, key) in [(WARP, ANALYSIS_KEY, &signature), (TRACKER, TRACKS_KEY, &signature), (ROTO, INPUT_KEY, &input)] {
            l.props.find_group_mut(uid).unwrap().get_mut(id).unwrap().value = Value::Str(key.clone());
        }
        Ok(())
    })
    .unwrap();
    // The Roto Brush's key covers the effects above it: the Warp Stabilizer's new key changed it.
    let layer = s.project.comp(COMP).unwrap().layer(CLIP).unwrap();
    let input = crate::roto::input_key(&s.project, COMP, layer, ROTO);
    s.edit("roto key", None, |p, _| {
        let g = p.comp_mut(COMP).unwrap().layer_mut(CLIP).unwrap().props.find_group_mut(ROTO).unwrap();
        g.get_mut(INPUT_KEY).unwrap().value = Value::Str(input.clone());
        g.get_mut(FROZEN).unwrap().value = Value::Str("saved frozen".into());
        Ok(())
    })
    .unwrap();
    let before = saved(&s);
    assert_eq!(before[0].1, signature);
    assert_eq!(before[2], ("saved frozen".into(), input));

    add_to_render_queue(&mut s);
    assert_eq!(saved(&s), before);
    s.execute_checked("edit.undo", json!({})).unwrap();
    assert_eq!(saved(&s), before);
    let mut s = open(&s.project.to_json());
    add_to_render_queue(&mut s);
    assert_eq!(saved(&s), before);
    // Offline and back online (#3534).
    for missing in [true, false] {
        if let Some(effectcraft_project::ItemKind::Footage(f)) = std::sync::Arc::make_mut(&mut s.project).item_mut(ItemId(1)).map(|i| &mut i.kind) {
            f.missing = missing;
        }
        add_to_render_queue(&mut s);
        assert_eq!(saved(&s), before);
    }
    // Names and labels only show things.
    s.execute_checked("layer.rename", json!({"layer": CLIP.0, "name": "Renamed"})).unwrap();
    s.edit("rename", None, |p, _| {
        let l = p.comp_mut(COMP).unwrap().layer_mut(CLIP).unwrap();
        l.props.find_group_mut(82).unwrap().name = "Soft".into();
        if let Some(effectcraft_project::ItemKind::Footage(f)) = p.item_mut(ItemId(1)).map(|i| &mut i.kind) {
            f.markers.push(effectcraft_project::Marker::default());
        }
        Ok(())
    })
    .unwrap();
    assert_eq!(saved(&s), before);

    // A real change still clears them.
    s.execute_checked("file.interpretFootage", json!({"items": [1], "invertAlpha": true})).unwrap();
    assert!(saved(&s).iter().all(|(data, _)| data.is_empty()));
}

/// Footage going offline (or coming back) is an observation, not an edit: the next edit keeps
/// every analysis (#3534).
#[test]
fn offline_footage_keeps_analyses() {
    let mut s = open(ANALYSED_0_24);
    let before = saved(&s);
    let missing = |s: &Session| matches!(&s.project.item(ItemId(1)).unwrap().kind, effectcraft_project::ItemKind::Footage(f) if f.missing);
    // The fixture's footage path doesn't exist here: the check marks it missing.
    let r = s.execute_checked("footage.check", json!({"wait": true})).unwrap();
    assert_eq!(r["missing"], 1, "{r}");
    assert!(missing(&s));
    add_to_render_queue(&mut s);
    assert_eq!(saved(&s), before);

    // Saved and reopened while offline, then back online.
    let mut s = open(&s.project.to_json());
    assert!(missing(&s));
    add_to_render_queue(&mut s);
    assert_eq!(saved(&s), before);
    if let Some(effectcraft_project::ItemKind::Footage(f)) = std::sync::Arc::make_mut(&mut s.project).item_mut(ItemId(1)).map(|i| &mut i.kind) {
        f.missing = false;
    }
    add_to_render_queue(&mut s);
    assert_eq!(saved(&s), before);

    // A real change while offline still clears them.
    s.execute_checked("file.interpretFootage", json!({"items": [1], "invertAlpha": true})).unwrap();
    assert!(saved(&s).iter().all(|(data, _)| data.is_empty()));
}

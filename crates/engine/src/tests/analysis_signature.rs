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

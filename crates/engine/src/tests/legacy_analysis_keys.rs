//! Warp Stabilizer and 3D Camera Tracker analyses saved by an earlier release keep matching
//! their footage after an edit that doesn't change it (#3456), and a real change still clears them.

use effectcraft_effects::camera_tracker::{SOLVE, TRACKS, TRACKS_KEY};
use effectcraft_effects::warp_stab::{ANALYSIS, ANALYSIS_KEY};
use effectcraft_keyframe::Value;
use effectcraft_project::{ItemId, LayerId, Project};
use serde_json::json;

use crate::Session;

const FOOTAGE: ItemId = ItemId(1);
const COMP: ItemId = ItemId(2);
const CLIP: LayerId = LayerId(3);
const WARP: u64 = 26;
const TRACKER: u64 = 54;

/// The analysis (as its length and a hash: it's long) and key of the Warp Stabilizer and the
/// 3D Camera Tracker.
fn saved(s: &Session) -> [(usize, u64, String); 2] {
    use std::hash::{Hash, Hasher};
    let layer = s.project.comp(COMP).unwrap().layer(CLIP).unwrap();
    let get = |uid: u64, data: &str, key: &str| {
        let params = crate::camera_track::static_params(layer.props.find_group(uid).unwrap());
        let text = |id: &str| match params.get(id) {
            Some(Value::Str(s)) => s.clone(),
            other => panic!("{id}: {other:?}"),
        };
        let analysis = text(data);
        let mut h = std::collections::hash_map::DefaultHasher::new();
        analysis.hash(&mut h);
        (analysis.len(), h.finish(), text(key))
    };
    [get(WARP, ANALYSIS, ANALYSIS_KEY), get(TRACKER, TRACKS, TRACKS_KEY)]
}

fn open(json: &str) -> Session {
    let mut s = Session::default();
    s.replace_project(Project::from_json(json).unwrap(), None);
    s.open_comp(COMP);
    s
}

#[test]
fn analyses_saved_by_0_22_survive_edits_that_keep_their_frames() {
    // Saved by the released 0.22.0 CLI after analysing both effects; its footage is offline here.
    let mut s = open(include_str!("fixtures/tracked-0.22.ecproj"));
    let before = saved(&s);
    let layer = s.project.comp(COMP).unwrap().layer(CLIP).unwrap();
    for (len, _, key) in &before {
        assert!(*len > 0);
        // The key predates Footage::placed_page, so it isn't today's signature.
        assert_ne!(key, &crate::analysis::frames_key::signature(&s.project, COMP, layer));
    }

    // Adding to the render queue edits the project, as `effectcraft-cli render` does.
    s.execute_checked("renderQueue.add", json!({"comp": COMP.0, "format": "png", "output": "frame.png"})).unwrap();
    assert_eq!(saved(&s), before);
    s.execute_checked("edit.undo", json!({})).unwrap();
    assert_eq!(saved(&s), before);

    // Saved and reopened (by this version), the old keys are kept and still match.
    let mut s = open(&s.project.to_json());
    s.execute_checked("renderQueue.add", json!({"comp": COMP.0, "format": "png", "output": "frame.png"})).unwrap();
    assert_eq!(saved(&s), before);

    // Other frames, from other timing or a reinterpreted footage item: both analyses are
    // cleared, and undo brings them back.
    let cleared = |s: &Session| saved(s).iter().all(|(len, _, key)| *len == 0 && key.is_empty());
    s.execute_checked("layer.timing", json!({"layer": CLIP.0, "out": 0.5})).unwrap();
    assert!(cleared(&s));
    s.execute_checked("edit.undo", json!({})).unwrap();
    assert_eq!(saved(&s), before);
    s.execute_checked("file.interpretFootage", json!({"items": [FOOTAGE.0], "invertAlpha": true})).unwrap();
    assert!(cleared(&s));
    s.execute_checked("edit.undo", json!({})).unwrap();
    assert_eq!(saved(&s), before);
    // Footage framed as a placed page has other frames too, though 0.22 had no such field.
    s.edit("place page", None, |p, _| {
        if let Some(effectcraft_project::ItemKind::Footage(f)) = p.item_mut(FOOTAGE).map(|i| &mut i.kind) {
            f.placed_page = true;
        }
        Ok(())
    })
    .unwrap();
    assert!(cleared(&s));
}

/// Solving again reuses the 0.22 tracks rather than tracking the (here offline) footage again.
#[test]
fn camera_tracker_solves_again_from_tracks_saved_by_0_22() {
    let mut s = open(include_str!("fixtures/tracked-0.22.ecproj"));
    let tracks = saved(&s)[1].clone();
    let solve = |s: &Session| {
        let g = s.project.comp(COMP).unwrap().layer(CLIP).unwrap().props.find_group(TRACKER).unwrap();
        crate::camera_track::static_params(g).get(SOLVE).cloned().unwrap()
    };
    // Another solve method, and back, clears the solve and keeps the tracks.
    for method in [1, 0] {
        s.execute_checked("prop.set", json!({"layer": CLIP.0, "path": "effects/#2/advanced/solveMethod", "value": method})).unwrap();
    }
    assert_eq!(solve(&s), Value::Str(String::new()));
    assert_eq!(saved(&s)[1], tracks);
    s.execute_checked("camera.analyze", json!({"layer": CLIP.0, "effect": TRACKER, "wait": true})).unwrap();
    assert!(matches!(solve(&s), Value::Str(v) if !v.is_empty()));
    assert_eq!(saved(&s)[1], tracks);
}

/// The footage text analysis keys are hashed from, as 0.23 saved it. A new `Footage` field must
/// not change it (or every saved analysis is dropped again).
#[test]
fn footage_signature_text_is_pinned() {
    use effectcraft_project::{Footage, FootageKind};
    let f = Footage {
        path: "clip.mp4".into(),
        kind: FootageKind::Video,
        width: 160,
        height: 120,
        invert_alpha: true,
        page: 2,
        placed_page: true,
        ..Footage::default()
    };
    let fields = r#"path: "clip.mp4", kind: Video, width: 160, height: 120, pixel_aspect: 1.0, frame_rate: FrameRate { num: 30, den: 1 }, native_rate: None, duration: Tick(0), has_video: false, has_audio: false, alpha: Straight, premul_color: [0.0, 0.0, 0.0], loop_count: 1, codec: "", missing: false, sequence: [], color_profile: None, layer: None, fields: Off, invert_alpha: true, linear_light: false, preserve_rgb: false, cineon: CineonSettings { log_conversion: false, converted_black: 0.0, converted_white: 1023.0, black: 95.0, white: 685.0, gamma: 1.7, rolloff: 20.0, units: TenBit }, data: None, page: 2, alphabetical: false, start_timecode: None, file_timecode: None, markers: []"#;
    assert_eq!(crate::analysis::frames_key::legacy::footage_text(&f, false, true), format!("Footage {{ {fields}, placed_page: true }}"));
    assert_eq!(crate::analysis::frames_key::legacy::footage_text(&f, false, false), format!("Footage {{ {fields} }}"));
}

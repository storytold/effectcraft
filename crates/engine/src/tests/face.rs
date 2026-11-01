//! Face tracking end to end through the commands, on synthetic face footage generated in-test
//! (a moving, rotating face that opens its mouth and blinks).

use effectcraft_keyframe::{ShapePath, Value as KV};
use effectcraft_track::face::{FaceParams, LANDMARKS, MEASUREMENTS, render_synthetic, synth_landmarks};
use serde_json::json;

use crate::mask_track::MaskMethod;
use crate::tests::track::{H, W, frame_time, setup};

fn face(f: u32) -> FaceParams {
    let t = f as f64;
    FaceParams {
        center: [160.0 + 25.0 * (t * 0.2).sin(), 120.0 + 8.0 * (t * 0.15).cos()],
        height: 70.0 * (1.0 + 0.05 * (t * 0.1).sin()),
        roll: 12.0 * (t * 0.18).sin(),
        yaw: 10.0 * (t * 0.12).sin(),
        mouth_open: 0.5 + 0.45 * (t * 0.3).sin(),
        eye_open: [0.85, 0.85],
        ..Default::default()
    }
}

#[test]
fn face_tracking_keys_outline_points_and_measurements() {
    let (mut s, clip, _) = setup(|f| render_synthetic(W, H, &face(f), 5));
    assert_eq!(MaskMethod::from_name("faceDetailed"), Some(MaskMethod::FaceDetailed));
    assert_eq!(MaskMethod::from_name("Face Tracking (Outline Only)"), Some(MaskMethod::FaceOutline));
    // A rough mask around the face.
    let c = face(0).center;
    let r = s.execute("mask.new", json!({"layer": clip.0, "vertices": ShapePath::rect(c, 120.0, 160.0).vertices, "closed": true})).unwrap();
    let mask = r["mask"].as_u64().unwrap();
    s.execute("time.set", json!({"time": 0})).unwrap();
    let r = s.execute_checked("track.mask", json!({"layer": clip.0, "mask": mask, "method": "faceDetailed", "direction": "forward", "wait": true})).unwrap();
    assert_eq!(r["frames"], 24, "{r}");
    assert_eq!(s.state.mask_track_method, MaskMethod::FaceDetailed);
    let comp = s.active_comp().unwrap();
    let layer = comp.layer(clip).unwrap();
    // The mask follows the face outline.
    let path = layer.props.find_group(mask).unwrap().get("path").unwrap();
    assert_eq!(path.keys.len(), 25);
    for f in [0, 12, 24] {
        let KV::Path(p) = path.value_at(frame_time(f)) else { panic!() };
        let fp = face(f);
        let (sn, cs) = fp.roll.to_radians().sin_cos();
        for v in &p.vertices {
            let d = [(v[0] - fp.center[0]) / fp.height, (v[1] - fp.center[1]) / fp.height];
            let (u, w) = (cs * d[0] + sn * d[1], -sn * d[0] + cs * d[1]);
            let r = ((u / 0.75).powi(2) + w * w).sqrt();
            assert!((r - 1.0).abs() < 0.06, "frame {f}: vertex {v:?} at {r}");
        }
    }
    // Face Track Points: one keyed point per landmark, on the face.
    let fx = layer.effects().unwrap();
    let pts = fx.groups().find(|g| g.match_id == effectcraft_effects::face_track::POINTS_ID).expect("Face Track Points");
    let mut worst: f64 = 0.0;
    for f in [0, 6, 12, 18, 24] {
        let truth = synth_landmarks(&face(f));
        for (i, (id, _, _)) in LANDMARKS.iter().enumerate() {
            let pr = pts.get(id).unwrap();
            assert_eq!(pr.keys.len(), 25, "{id}");
            let KV::Vec2(v) = pr.value_at(frame_time(f)) else { panic!() };
            worst = worst.max((v[0] - truth[i][0]).hypot(v[1] - truth[i][1]) / face(f).height);
        }
    }
    assert!(worst < 0.1, "worst landmark error {worst} face heights");
    // Extract & Copy Face Measurements.
    assert!(s.is_enabled("track.extractFaceMeasurements"));
    let r = s.execute_checked("track.extractFaceMeasurements", json!({"layer": clip.0})).unwrap();
    assert_eq!(r["frames"], 25);
    assert!(r["clipboard"].as_str().unwrap().contains("Mouth Openness"));
    assert_eq!(s.state.key_clipboard.len(), MEASUREMENTS.len());
    assert!(s.state.clip_is_keys);
    let layer = s.active_comp().unwrap().layer(clip).unwrap();
    let m = layer.effects().unwrap().groups().find(|g| g.match_id == effectcraft_effects::face_track::MEASUREMENTS_ID).expect("Face Measurements");
    let mo = m.get("mouthOpenness").unwrap();
    assert_eq!(mo.keys.len(), 25);
    // Mouth openness follows the footage: widest open where the generator opens it most.
    let open = |f: u32| mo.value_at(frame_time(f)).as_f64();
    let (fmax, fmin) = (
        (0..25).max_by(|a, b| face(*a).mouth_open.total_cmp(&face(*b).mouth_open)).unwrap(),
        (0..25).min_by(|a, b| face(*a).mouth_open.total_cmp(&face(*b).mouth_open)).unwrap(),
    );
    assert!(open(fmax) > open(fmin) + 20.0, "{} vs {}", open(fmax), open(fmin));
    let roll = m.get("headOrientationZ").unwrap();
    for f in [0, 10, 20] {
        assert!((roll.value_at(frame_time(f)).as_f64() - face(f).roll).abs() < 3.0);
    }
    // One undo step for the extraction.
    assert!(s.undo());
    assert!(!s.active_comp().unwrap().layer(clip).unwrap().effects().unwrap().groups().any(|g| g.match_id == effectcraft_effects::face_track::MEASUREMENTS_ID));
    // Outline Only keys just the mask.
    let r = s.execute("mask.new", json!({"layer": clip.0, "vertices": ShapePath::rect(c, 120.0, 160.0).vertices, "closed": true})).unwrap();
    let m2 = r["mask"].as_u64().unwrap();
    s.execute("time.set", json!({"time": 0})).unwrap();
    s.execute_checked("track.mask", json!({"layer": clip.0, "mask": m2, "method": "faceOutline", "direction": "frameForward", "wait": true})).unwrap();
    let layer = s.active_comp().unwrap().layer(clip).unwrap();
    assert_eq!(layer.props.find_group(m2).unwrap().get("path").unwrap().keys.len(), 2);
    // A mask without a face reports it.
    let r = s.execute("mask.new", json!({"layer": clip.0, "vertices": ShapePath::rect([20.0, 20.0], 30.0, 30.0).vertices, "closed": true})).unwrap();
    let m3 = r["mask"].as_u64().unwrap();
    s.execute("time.set", json!({"time": 0})).unwrap();
    let _ = s.execute("track.mask", json!({"layer": clip.0, "mask": m3, "method": "faceOutline", "direction": "forward", "wait": true}));
    let layer = s.active_comp().unwrap().layer(clip).unwrap();
    assert!(layer.props.find_group(m3).unwrap().get("path").unwrap().keys.is_empty());
}

/// With MediaPipe Face Landmarker installed (`EFFECTCRAFT_FACE_LANDMARKER` = path to
/// `face_landmarker.task`, else skipped): Track Mask ▸ Face Tracking uses it, and the pupils and
/// eye corners it keys sit on the synthetic face's.
#[test]
fn face_tracking_uses_the_chosen_model() {
    let Ok(model) = std::env::var("EFFECTCRAFT_FACE_LANDMARKER") else { return };
    let (mut s, clip, _) = setup(|f| render_synthetic(W, H, &face(f), 5));
    let dir = std::env::temp_dir().join(format!("ec-face-model-{}", std::process::id()));
    s.models_dir = Some(dir.clone());
    s.execute_checked("face.model.install", json!({"path": model})).unwrap();
    // `wait`: loaded before the track starts (as scripts need).
    let r = s.execute_checked("face.model.select", json!({"id": "mediapipe-face", "wait": true})).unwrap();
    assert_eq!(r["active"], "mediapipe-face");
    let c = face(0).center;
    let r = s.execute("mask.new", json!({"layer": clip.0, "vertices": ShapePath::rect(c, 120.0, 160.0).vertices, "closed": true})).unwrap();
    let mask = r["mask"].as_u64().unwrap();
    s.execute("time.set", json!({"time": 0})).unwrap();
    let r = s.execute_checked("track.mask", json!({"layer": clip.0, "mask": mask, "method": "faceDetailed", "direction": "forward", "wait": true})).unwrap();
    assert_eq!((r["frames"].as_u64(), r["faceModel"].as_str()), (Some(24), Some("mediapipe-face")), "{r}");
    let layer = s.active_comp().unwrap().layer(clip).unwrap().clone();
    let pts = layer.effects().unwrap().groups().find(|g| g.match_id == effectcraft_effects::face_track::POINTS_ID).expect("Face Track Points");
    let mut worst: f64 = 0.0;
    for f in [0, 8, 16, 24] {
        let truth = synth_landmarks(&face(f));
        for id in ["leftPupil", "rightPupil", "leftEyeOuter", "rightEyeOuter"] {
            let i = effectcraft_track::face::landmark(id).unwrap();
            let KV::Vec2(v) = pts.get(id).unwrap().value_at(frame_time(f)) else { panic!() };
            worst = worst.max((v[0] - truth[i][0]).hypot(v[1] - truth[i][1]) / face(f).height);
        }
    }
    assert!(worst < 0.05, "worst eye error {worst} face heights");
    let _ = std::fs::remove_dir_all(dir);
}

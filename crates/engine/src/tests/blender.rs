//! Synthetic camera interchange: projection, timing, DOF and hostile inputs. Optional public
//! Blender-API oracle: EFFECTCRAFT_BLENDER=/path/to/blender cargo test -p effectcraft-engine tests::blender -- --nocapture.

use effectcraft_effects::camera_tracker as ct;
use effectcraft_keyframe::Value as KV;
use effectcraft_project::{Footage, FootageKind, ItemKind, LayerId, LayerSource, build};
use effectcraft_time::{FrameRate, Tick};
use effectcraft_track::camtrack::{CameraSolve, Distortion, Ground, ShotType, SolveMethod, SolvedFrame, SolvedPoint};
use serde_json::{Value, json};

use crate::Session;

fn path(name: &str) -> String {
    let dir = std::env::temp_dir().join(format!("effectcraft-blender-tests-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name).to_string_lossy().into_owned()
}

fn fixture(ground: bool) -> (Session, LayerId, u64) {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Shot '\\ — câmera", "width": 640, "height": 360, "duration": 6})).unwrap();
    let cid = s.active_comp_id().unwrap();
    let rate = FrameRate::new(30000, 1001);
    let lid = s
        .edit("fixture", None, |p, _| {
            p.comp_mut(cid).unwrap().frame_rate = rate;
            let item = p.add_item(
                "Plate",
                Default::default(),
                None,
                ItemKind::Footage(Footage {
                    path: std::env::var("EFFECTCRAFT_BLENDER_PLATE").unwrap_or_else(|_| path("plate '\\ — vídeo.mp4")),
                    width: 640,
                    height: 360,
                    frame_rate: FrameRate::FPS_30,
                    duration: Tick::from_seconds_f64(6.0),
                    kind: FootageKind::Video,
                    has_video: true,
                    ..Default::default()
                }),
            );
            let comp = p.comp(cid).unwrap().clone();
            let mut layer = build::layer(p, &comp, "Tracked Video", LayerSource::Footage { item }, (640, 360), None);
            layer.in_point = rate.tick_of(60);
            layer.out_point = rate.tick_of(63);
            layer.start_time = rate.tick_of(30);
            layer.stretch = 200.0;
            let id = layer.id;
            p.comp_mut(cid).unwrap().layers.push(layer);
            Ok(id)
        })
        .unwrap();
    let uid = s.execute("effect.apply", json!({"effect": ct::ID, "layers": [lid.0]})).unwrap()["effects"][0].as_u64().unwrap();
    let solve = CameraSolve {
        version: 1,
        size: [640.0, 360.0],
        start: rate.tick_of(30).seconds() / 2.0,
        frame_duration: rate.frame_duration().seconds() / 2.0,
        shot: ShotType::VariableZoom,
        method_used: SolveMethod::Typical,
        average_error: 0.12,
        frames: (0..3)
            .map(|i| SolvedFrame {
                rot: [0.02 * i as f64, -0.03 * i as f64, 0.01 * i as f64],
                center: [0.1 * i as f64, -0.01 * i as f64, 0.02 * i as f64],
                focal: 600.0 + i as f64 * 25.0,
                solved: true,
            })
            .collect(),
        points: vec![
            SolvedPoint { id: 7, pos: [0.1, 0.2, 2.0], error: 0.13, first: 0, last: 2 },
            SolvedPoint { id: 9, pos: [-0.3, -0.1, 1.2], error: 0.11, first: 0, last: 1 },
        ],
        ground: ground.then_some(Ground { origin: [0.1, 0.2, 2.0], normal: [0.0, -1.0, 0.0], x_axis: [1.0, 0.0, 0.0] }),
        distortion: ground.then_some(Distortion { k1: 0.01, k2: -0.002, radius: 320.0_f64.hypot(180.0) }),
    };
    // Directly install a deterministic solve; no analysis or decoder is needed by these tests.
    s.execute("prop.set", json!({"layer": lid.0, "path": "effects/#1/solve", "value": solve.to_json()})).unwrap();
    s.camera_pending.clear();
    s.execute("layer.select", json!({"layers": [lid.0]})).unwrap();
    (s, lid, uid)
}

fn data(script: &str) -> Value {
    let line = script.lines().find_map(|l| l.strip_prefix("DATA = json.loads(")).unwrap().strip_suffix(')').unwrap();
    let encoded: String = serde_json::from_str(line).unwrap();
    serde_json::from_str(&encoded).unwrap()
}

#[test]
fn blender_export_bakes_projection_focus_and_retimed_background() {
    let (mut s, lid, uid) = fixture(true);
    let result = s.execute("camera.create", json!({"layer": lid.0, "effect": uid})).unwrap();
    let camera = result["camera"].as_u64().unwrap();
    s.execute("layer.cameraSettings", json!({"layer": camera, "focusDistance": 800.0, "dof": true, "fStop": 4.0, "lockToZoom": false})).unwrap();
    for (frame, value) in [(60, 800.0), (62, 1000.0)] {
        let time = FrameRate::new(30000, 1001).tick_of(frame).seconds();
        s.execute("prop.addKey", json!({"layer": camera, "path": "cameraOptions/focusDistance", "time": time, "value": value})).unwrap();
    }
    let camera_layer = s.active_comp().unwrap().layer(LayerId(camera)).unwrap();
    assert!(camera_layer.props.prop("cameraOptions/dof").unwrap().value.as_bool());
    let output = path("oracle-scene.py");
    let before = s.project.to_json();
    let r = s.execute_checked("camera.exportBlender", json!({"layer": lid.0, "effect": uid, "path": output, "startFrame": 101})).unwrap();
    assert_eq!(s.project.to_json(), before, "export must not edit the document");
    assert_eq!(r["frames"], 3);
    assert_eq!(r["points"], 2);
    let script = std::fs::read_to_string(&output).unwrap();
    let d = data(&script);
    assert_eq!(d["resolution"], json!([640.0, 360.0]));
    assert_eq!(d["fpsNumerator"], 30000);
    assert_eq!(d["fpsDenominator"], 1001);
    assert_eq!(d["frames"][0]["sourceFrame"], 15, "trim + 200% stretch, not scene frame 101");
    assert_eq!(d["frames"][2]["sourceFrame"], 16);
    assert_eq!(d["frames"][0]["focus"], 8.0);
    assert_eq!(d["frames"][2]["focus"], 10.0);
    assert_eq!(d["frames"][0]["dof"], true);
    assert!((d["frames"][0]["fstop"].as_f64().unwrap() - 4.0).abs() < 1e-6);
    assert_eq!(d["frames"][0]["lens"], 33.75);
    assert_eq!(d["frames"][2]["lens"], 36.5625);
    assert_eq!(d["ground"], true);
    assert_eq!(d["solve"]["distortion"]["k1"], 0.01);
    assert_eq!(d["points"][0]["location"][2], 0.0, "ground point lies on Blender XY plane");

    // Execute the actual exported artifact in a separately installed Blender, when requested.
    if let Ok(blender) = std::env::var("EFFECTCRAFT_BLENDER") {
        let oracle = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/blender_scene_oracle.py");
        let output = std::process::Command::new(blender)
            .args(["--background", "--factory-startup", "--python-exit-code", "1", "--python"])
            .arg(oracle)
            .arg("--")
            .arg(&output)
            .output()
            .unwrap();
        assert!(output.status.success(), "Blender oracle failed:\n{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        assert!(String::from_utf8_lossy(&output.stdout).contains("EFFECTCRAFT_BLENDER_ORACLE_OK"));
    }
}

#[test]
fn blender_export_bakes_reverse_time_remap_and_footage_loops() {
    let (mut s, lid, _) = fixture(false);
    let cid = s.active_comp_id().unwrap();
    let rate = FrameRate::new(30000, 1001);
    s.edit("retimed fixture", None, |p, _| {
        let layer = p.comp(cid).unwrap().layer(lid).unwrap();
        let item = layer.source.item().unwrap();
        let keys = [(60, 8.0), (61, 6.0), (62, -1.0)]
            .map(|(frame, time)| effectcraft_keyframe::Keyframe::new(layer.layer_time(rate.tick_of(frame)), KV::Scalar(time)));
        let mut remap = build::Ids(&mut p.next_id).prop("timeRemap", "Time Remap", KV::Scalar(8.0));
        remap.keys = keys.to_vec();
        p.comp_mut(cid).unwrap().layer_mut(lid).unwrap().props.children.push(remap.into());
        if let ItemKind::Footage(f) = &mut p.item_mut(item).unwrap().kind {
            f.loop_count = 2;
        }
        Ok(())
    })
    .unwrap();
    let output = path("remap.py");
    s.execute_checked("camera.exportBlender", json!({"layer": lid.0, "path": output})).unwrap();
    let d = data(&std::fs::read_to_string(output).unwrap());
    let indices: Vec<i64> = d["frames"].as_array().unwrap().iter().map(|f| f["sourceFrame"].as_i64().unwrap()).collect();
    assert_eq!(indices, vec![60, 0, 0], "reverse remap wraps the 6-second video and clamps negative source time, as the media pool does");
}

#[test]
fn blender_export_defaults_and_script_escaping() {
    let (mut s, lid, _) = fixture(false);
    let output = path("escaped.py");
    let r = s
        .execute_checked("camera.exportBlender", json!({"layer": lid.0, "path": output, "includePoints": false, "sensorWidth": 24.0, "unitScale": 0.1}))
        .unwrap();
    assert_eq!(r["points"], 0);
    let script = std::fs::read_to_string(&output).unwrap();
    let d = data(&script);
    assert!(d["name"].as_str().unwrap().contains("'\\ — câmera"));
    assert_eq!(d["frames"][0]["lens"], 22.5);
    assert_eq!(d["frames"][0]["dof"], false);
    assert!(r["warnings"].as_array().unwrap().iter().any(|w| w.as_str().unwrap().contains("cannot be recovered")));
    match std::process::Command::new("python3").args(["-c", "import ast,sys; ast.parse(open(sys.argv[1], encoding='utf-8').read())", &output]).status() {
        Ok(status) => assert!(status.success(), "generated script must parse even with quoted/Unicode paths"),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => eprintln!("Python AST oracle skipped: python3 is not installed"),
        Err(e) => panic!("Python AST oracle failed: {e}"),
    }
}

#[test]
fn blender_export_rejects_invalid_arguments_and_write_failures() {
    let (mut s, lid, _) = fixture(false);
    for params in [
        json!({"sensorWidth": 0}),
        json!({"sensorWidth": "wide"}),
        json!({"unitScale": -1}),
        json!({"startFrame": -1}),
        json!({"startFrame": 1.5}),
        json!({"startFrame": 1048574}),
        json!({"includePoints": "yes"}),
        json!({"bogus": true}),
        json!({"path": path("bad.blend")}),
    ] {
        let mut p = json!({"layer": lid.0, "path": path("invalid.py")});
        p.as_object_mut().unwrap().extend(params.as_object().unwrap().clone());
        assert!(s.execute_checked("camera.exportBlender", p.clone()).is_err(), "must reject {p}");
    }
    let blocker = path("blocker");
    std::fs::write(&blocker, b"not a directory").unwrap();
    let p = format!("{blocker}/scene.py");
    assert!(s.execute_checked("camera.exportBlender", json!({"layer": lid.0, "path": p})).unwrap_err().to_string().contains("cannot write"));
}

#[test]
fn blender_export_rejects_malformed_solves_without_panicking() {
    for variant in ["empty", "unsolved", "zero-focal", "bad-ground", "huge-resolution", "mismatched-range"] {
        let (mut s, lid, uid) = fixture(false);
        let cid = s.active_comp_id().unwrap();
        let mut solve = ct::solve(&crate::camera_track::static_params(s.active_comp().unwrap().layer(lid).unwrap().props.find_group(uid).unwrap()))
            .unwrap()
            .as_ref()
            .clone();
        match variant {
            "empty" => solve.frames.clear(),
            "unsolved" => solve.frames[0].solved = false,
            "zero-focal" => solve.frames[0].focal = 0.0,
            "bad-ground" => solve.ground = Some(Ground::default()),
            "huge-resolution" => solve.size[0] = 1e10,
            _ => {
                solve.frames.pop();
            }
        }
        s.edit("corrupt solve", None, |p, _| {
            p.comp_mut(cid).unwrap().layer_mut(lid).unwrap().props.find_group_mut(uid).unwrap().prop_mut(ct::SOLVE).unwrap().value = KV::Str(solve.to_json());
            Ok(())
        })
        .unwrap();
        s.camera_pending.clear();
        let result = s.execute_checked("camera.exportBlender", json!({"layer": lid.0, "path": path("corrupt.py")}));
        assert!(result.is_err(), "must reject {variant}");
    }
}

#[test]
fn blender_export_respects_script_write_permission() {
    let (mut s, lid, _) = fixture(false);
    let output = path("script-permissions.py");
    let params = json!({"layer": lid.0, "path": output});
    for (from_script, allowed) in [(true, false), (true, true), (false, false), (false, true)] {
        std::fs::write(&output, b"existing file").unwrap();
        s.prefs.scripting.allow_scripts_write_files = allowed;
        let result =
            if from_script { s.execute_for_script("camera.exportBlender", params.clone()) } else { s.execute_checked("camera.exportBlender", params.clone()) };
        assert!(!s.running_for_script(), "script origin must be restored after success or error");
        let contents = std::fs::read_to_string(&output).unwrap();
        if from_script && !allowed {
            assert!(result.unwrap_err().to_string().contains("Allow Scripts to Write Files"));
            assert_eq!(contents, "existing file", "blocked scripts must not replace an existing file");
        } else {
            assert_eq!(result.unwrap()["frames"], 3);
            assert!(contents.contains("def import_scene():"));
        }
    }
}

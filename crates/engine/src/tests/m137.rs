//! M13.7 menu items: File ▸ Watch Folder, Import ▸ Vanishing Point (disabled), Create Nulls From
//! Paths, VR Comp Editor and Help ▸ In-App / Online Tutorials.

use std::sync::Arc;

use serde_json::json;

use crate::rq_tests::MockExporter;
use crate::{Event, Session};

fn out_dir(name: &str) -> std::path::PathBuf {
    let d = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-out").join(name);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn watch_folder_renders_queued_projects_and_writes_status() {
    let dir = out_dir("watch-folder");
    let ex = Arc::new(MockExporter { log: Default::default() });
    // A project with a queued render, and one (in a subfolder) with nothing queued.
    let mut a = Session { exporter: Some(ex.clone()), ..Default::default() };
    a.execute("comp.new", json!({"name": "Shot", "width": 32, "height": 32, "frameRate": 10, "duration": 0.5})).unwrap();
    a.execute("renderQueue.add", json!({})).unwrap();
    let pa = dir.join("a.ecproj");
    a.execute("file.saveAs", json!({"path": pa.to_string_lossy()})).unwrap();
    std::fs::create_dir_all(dir.join("collected")).unwrap();
    let mut b = Session::default();
    b.execute("comp.new", json!({"name": "Idle", "width": 32, "height": 32, "duration": 1})).unwrap();
    let pb = dir.join("collected/b.ecproj");
    b.execute("file.saveAs", json!({"path": pb.to_string_lossy()})).unwrap();

    let mut host = Session { exporter: Some(ex.clone()), ..Default::default() };
    assert!(!host.is_enabled("file.watchFolder.poll"));
    let r = host.execute("file.watchFolder", json!({"folder": dir.to_string_lossy()})).unwrap();
    let done = r["rendered"].as_array().unwrap();
    assert_eq!(done.len(), 2, "{r}");
    assert_eq!(host.state.watch_folder.as_deref(), Some(dir.to_string_lossy().as_ref()));
    let st: serde_json::Value = serde_json::from_slice(&std::fs::read(dir.join("a.ecproj.status.json")).unwrap()).unwrap();
    assert_eq!(st["state"], "done", "{st}");
    assert_eq!(st["items"][0]["comp"], "Shot");
    assert_eq!(st["items"][0]["status"], "Done");
    assert_eq!(ex.log.lock().unwrap().len(), 1);
    let st: serde_json::Value = serde_json::from_slice(&std::fs::read(dir.join("collected/b.ecproj.status.json")).unwrap()).unwrap();
    assert_eq!(st["note"], "nothing queued");
    // Rendered projects are not picked up again; deleting the status file re-renders.
    assert!(host.execute("file.watchFolder.poll", json!({})).unwrap()["rendered"].as_array().unwrap().is_empty());
    std::fs::remove_file(dir.join("a.ecproj.status.json")).unwrap();
    assert_eq!(host.execute("file.watchFolder.poll", json!({})).unwrap()["rendered"].as_array().unwrap().len(), 1);
    assert_eq!(ex.log.lock().unwrap().len(), 2);
    // A broken project fails with a status, not an error.
    std::fs::write(dir.join("broken.ecproj"), b"not a project").unwrap();
    let r = host.execute("file.watchFolder.poll", json!({})).unwrap();
    assert_eq!(r["rendered"][0]["state"], "failed");
    host.execute("file.watchFolder", json!({"stop": true})).unwrap();
    assert!(host.state.watch_folder.is_none());
    assert!(host.execute("file.watchFolder", json!({"folder": "/no/such/folder"})).is_err());
}

#[test]
fn vanishing_point_import_is_disabled_with_a_reason() {
    let mut s = Session::default();
    assert!(!s.is_enabled("file.importVanishingPoint"));
    let spec = crate::commands::find("file.importVanishingPoint").unwrap();
    assert!((spec.enabled)(&s).unwrap_err().contains("public specification"));
    assert!(s.execute("file.importVanishingPoint", json!({})).is_err());
}

#[test]
fn help_tutorials() {
    let mut s = Session::default();
    s.drain_events();
    s.execute("help.onlineTutorials", json!({})).unwrap();
    s.execute("help.inAppTutorials", json!({})).unwrap();
    let ev = s.drain_events();
    assert!(ev.iter().any(|e| matches!(e, Event::OpenUrl(u) if u == crate::links::APP_PAGE)));
    assert!(ev.iter().any(|e| matches!(e, Event::Frontend { command, .. } if command == "help.inAppTutorials")));
}

/// A comp with a 2D solid at (50, 40) carrying a square mask.
fn masked() -> (Session, u64) {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Paths", "width": 100, "height": 80, "frameRate": 10, "duration": 1})).unwrap();
    let l = s.execute("layer.newSolid", json!({"name": "Plate", "color": "#808080", "width": 40, "height": 40})).unwrap()["layer"].as_u64().unwrap();
    s.execute("mask.new", json!({"layer": l, "vertices": [[0, 0], [40, 0], [40, 40], [0, 40]], "closed": true})).unwrap();
    (s, l)
}

#[test]
fn nulls_follow_points_and_points_follow_nulls() {
    let (mut s, l) = masked();
    s.execute("layer.select", json!({"layers": [l]})).unwrap();
    let r = s.execute("paths.nullsFollowPoints", json!({})).unwrap();
    let nulls: Vec<u64> = r["nulls"].as_array().unwrap().iter().map(|v| v.as_u64().unwrap()).collect();
    assert_eq!(nulls.len(), 4);
    let comp = s.active_comp().unwrap().clone();
    let n2 = comp.layer(effectcraft_project::LayerId(nulls[2])).unwrap();
    assert_eq!(n2.name, "Plate: Mask 1 [3]");
    // Placed on the vertex in comp space (the 40×40 solid is centred at (50, 40)).
    let pos = n2.props.prop("transform/position").unwrap();
    assert_eq!(&pos.value.components()[..2], &[70.0, 60.0]);
    let e = pos.expr.as_ref().unwrap().text.clone();
    assert!(e.contains("thisComp.layer(\"Plate\")") && e.contains(".points()[2]"), "{e}");
    // One undo step.
    assert!(s.undo());
    assert_eq!(s.active_comp().unwrap().layers.len(), 1);

    let r = s.execute("paths.pointsFollowNulls", json!({"layer": l, "path": "masks/#1/path"})).unwrap();
    assert_eq!(r["nulls"].as_array().unwrap().len(), 4);
    let comp = s.active_comp().unwrap().clone();
    let plate = comp.layer(effectcraft_project::LayerId(l)).unwrap();
    let e = plate.props.prop("masks/#1/path").unwrap().expr.as_ref().unwrap().text.clone();
    assert!(e.contains("createPath(pts") && e.contains("\"Plate: Mask 1 [1]\"") && e.contains("fromComp"), "{e}");
    // Not a path → error.
    assert!(s.execute("paths.nullsFollowPoints", json!({"layer": l, "path": "transform/opacity"})).is_err());
}

#[test]
fn trace_path_adds_a_progress_null() {
    let (mut s, l) = masked();
    let r = s.execute("paths.tracePath", json!({"layer": l, "loop": true})).unwrap();
    let n = r["null"].as_u64().unwrap();
    let comp = s.active_comp().unwrap().clone();
    let null = comp.layer(effectcraft_project::LayerId(n)).unwrap();
    let slider = null.props.prop("effects/#1/slider").unwrap();
    assert_eq!(slider.keys.len(), 2);
    assert_eq!(slider.keys[1].value.as_f64(), 100.0);
    assert!(slider.expr.as_ref().unwrap().text.contains("loopOut"));
    assert_eq!(null.props.group("effects/#1").unwrap().name, "Progress");
    assert!(null.props.prop("transform/position").unwrap().expr.as_ref().unwrap().text.contains("pointOnPath(effect(\"Progress\")(\"Slider\") / 100)"));
}

#[test]
fn vr_comp_editor_turns_the_face_cameras_together() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Scene", "width": 64, "height": 64, "frameRate": 10, "duration": 1})).unwrap();
    s.execute("layer.newSolid", json!({"name": "Wall", "color": "#ff0000"})).unwrap();
    s.execute("comp.vr.createEnvironment", json!({"size": 64})).unwrap();
    let envs = s.execute("comp.vr.environments", json!({})).unwrap();
    assert_eq!(envs.as_array().unwrap().len(), 1, "{envs}");
    let env = &envs[0];
    assert_eq!(env["name"], "Scene VR Output");
    assert_eq!(env["faces"].as_array().unwrap().len(), 6);
    let view = env["view"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect::<Vec<_>>();
    assert!(view.iter().all(|v| v.abs() < 1e-6 || (v - 360.0).abs() < 1e-6), "{view:?}");
    let out = env["output"].as_u64().unwrap();
    let r = s.execute("comp.vr.setView", json!({"comp": out, "pan": 30.0})).unwrap();
    let v = r["view"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect::<Vec<_>>();
    assert!((v[1] - 30.0).abs() < 1e-6 && v[0].abs() < 1e-6 && v[2].abs() < 1e-6, "{v:?}");
    // Every face camera turned: Front by 30° about Y.
    let front = &env["faces"][4];
    let fc = s.project.comp(effectcraft_project::ItemId(front["comp"].as_u64().unwrap())).unwrap();
    let cam = fc.layer(effectcraft_project::LayerId(front["camera"].as_u64().unwrap())).unwrap();
    let o = cam.props.prop("transform/orientation").unwrap().value.components();
    assert!((o[1] - 30.0).abs() < 1e-6, "{o:?}");
    // One undo step back to the start.
    assert!(s.undo());
    let envs = s.execute("comp.vr.environments", json!({})).unwrap();
    assert!(envs[0]["view"][1].as_f64().unwrap().abs() < 1e-6);
    // Round trip of the Euler decomposition.
    for o in [[10.0, 20.0, 30.0], [350.0, 45.0, 5.0], [0.0, 0.0, 270.0]] {
        let m = effectcraft_geom::Mat4::orientation(effectcraft_geom::vec3(o[0], o[1], o[2]));
        let e = crate::commands::vr_editor::euler(m);
        assert!(e.iter().zip(o).all(|(a, b)| (a - b).abs() < 1e-6), "{o:?} → {e:?}");
    }
}

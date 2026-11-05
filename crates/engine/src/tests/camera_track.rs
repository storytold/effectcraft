//! The 3D Camera Tracker end to end through the commands, on synthetic footage rendered in-test:
//! textured planes (a floor, a back wall and a box face) seen through a known moving camera.

use effectcraft_effects::camera_tracker as ct;
use effectcraft_geom::vec3;
use effectcraft_project::{Comp, Layer, LayerId, PropGroup};
use effectcraft_raster::Image;
use effectcraft_render::EvalCtx;
use effectcraft_render::three_d::camera::active_camera;
use effectcraft_track::camtrack::linalg::{self, M3, V3};
use serde_json::{Value, json};

use crate::Session;
use crate::camera_track::Placed;
use crate::tests::track::{H, W, frame_time, setup, texture};

const F: f64 = 260.0;

/// The true camera on frame `f`: (world → camera rotation, centre).
fn true_camera(f: u32) -> (M3, V3) {
    let t = f as f64 / 24.0;
    let c = [-1.0 + 2.0 * t, -0.3 + 0.2 * (t * 3.0).sin(), 0.5 * t];
    let fwd = linalg::normalize(linalg::sub([0.2 * t, 0.4, 8.0], c));
    let right = linalg::normalize(linalg::cross([0.0, 1.0, 0.0], fwd));
    ([right, linalg::cross(fwd, right), fwd], c)
}

/// Planes: (point, normal, u axis, v axis, half extent or 0 = infinite).
const PLANES: [(V3, V3, V3, V3, f64); 3] = [
    ([0.0, 1.5, 0.0], [0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0], 0.0),
    ([0.0, 0.0, 10.0], [0.0, 0.0, -1.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], 0.0),
    ([-1.2, 0.2, 6.0], [0.3, 0.0, -1.0], [0.957_826, 0.0, 0.287_348], [0.0, 1.0, 0.0], 1.1),
];

fn scene_frame(f: u32) -> Image {
    let (r, c) = true_camera(f);
    let mut img = Image::new(W, H);
    for y in 0..H {
        for x in 0..W {
            let d = linalg::mtv(&r, [(x as f64 + 0.5 - W as f64 / 2.0) / F, (y as f64 + 0.5 - H as f64 / 2.0) / F, 1.0]);
            let mut best = (f64::INFINITY, 0.2f32);
            for (i, (p0, n, ua, va, ext)) in PLANES.iter().enumerate() {
                let den = linalg::dot(*n, d);
                if den.abs() < 1e-9 {
                    continue;
                }
                let tt = linalg::dot(*n, linalg::sub(*p0, c)) / den;
                if tt <= 0.1 || tt >= best.0 {
                    continue;
                }
                let hit = linalg::add(c, linalg::scale(d, tt));
                let (u, v) = (linalg::dot(linalg::sub(hit, *p0), *ua), linalg::dot(linalg::sub(hit, *p0), *va));
                if *ext > 0.0 && (u.abs() > *ext || v.abs() > *ext) {
                    continue;
                }
                best = (tt, texture(u * 40.0, v * 40.0, 11 + i as u32));
            }
            img.set(x, y, [best.1, best.1, best.1, 1.0]);
        }
    }
    img
}

/// The comp with the clip tracked (blocking) with `params`.
fn tracked(params: Value) -> (Session, LayerId) {
    let (mut s, clip, _) = setup(scene_frame);
    s.execute("layer.select", json!({"layers": [clip.0]})).unwrap();
    assert!(s.is_enabled("track.camera"));
    let mut p = params;
    p["wait"] = json!(true);
    let r = s.execute_checked("track.camera", p).unwrap();
    assert_eq!(r["frames"], 25, "{r}");
    assert!(s.camera_job.is_none());
    (s, clip)
}

fn status(s: &mut Session) -> Value {
    s.execute("camera.solveStatus", json!({})).unwrap()
}

/// Solved with a sane error and focal length; returns the Camera Tracker effect group.
fn assert_solved(s: &mut Session, clip: LayerId) -> PropGroup {
    let st = status(s);
    assert_eq!(st["solved"], true, "{st}");
    let err = st["averageError"].as_f64().unwrap();
    assert!(err < 0.5, "average error {err}");
    let f = st["focalLength"].as_f64().unwrap();
    assert!((f - F).abs() / F < 0.05, "focal {f}");
    assert_ne!(st["methodUsed"], "Tripod Pan");
    // Method Used / Average Error are reported in the effect.
    let g = s.active_comp().unwrap().layer(clip).unwrap().effects().unwrap().groups().next().unwrap().clone();
    assert!(matches!(g.prop(ct::METHOD_USED).map(|p| &p.value), Some(effectcraft_keyframe::Value::Str(m)) if !m.is_empty()));
    g
}

/// Pick floor points on frame 12 (their 3D positions lie on y = 1.5 in the true scene: the
/// ones seen below the horizon at the bottom of the frame).
fn select_floor_points(s: &mut Session) -> Vec<u64> {
    s.execute("time.set", json!({"time": frame_time(12).seconds()})).unwrap();
    let pts = s.execute("camera.points", json!({})).unwrap();
    let list = pts["points"].as_array().unwrap();
    assert!(list.len() > 30, "{}", list.len());
    let floor: Vec<u64> = list.iter().filter(|p| p["comp"][1].as_f64().unwrap() > H as f64 * 0.8).map(|p| p["id"].as_u64().unwrap()).take(8).collect();
    assert!(floor.len() >= 3, "{floor:?}");
    s.execute("camera.selectPoints", json!({"points": floor})).unwrap();
    assert_eq!(s.state.camera_points.len(), floor.len());
    floor
}

/// Every solved point projects through the created camera exactly where the solve puts it,
/// and onto its tracked position (the footage fills the comp, so layer pixels are comp
/// pixels): within 1 px for 95 % of the observations (the rest are the solve's outliers:
/// features on occlusion edges), 0.5 px on average.
fn assert_reprojection(s: &Session, comp: &Comp, pl: &Placed, g: &PropGroup) {
    let cid = s.active_comp_id().unwrap();
    let tracks = ct::tracks(&crate::camera_track::static_params(g)).unwrap();
    let mut errs = vec![];
    for k in [0u32, 6, 12, 18, 24] {
        let ctx = EvalCtx::new(&s.project, cid, comp, frame_time(k));
        let camst = active_camera(&ctx);
        for sp in pl.solve.visible(k as usize) {
            let Some(obs) = tracks.track(sp.id).and_then(|t| t.at(k)) else { continue };
            let w = pl.to_world(sp.pos);
            let q = camst.project(W as f64, H as f64, vec3(w[0], w[1], w[2])).unwrap();
            let own = pl.solve.frames[k as usize].project(pl.solve.size, sp.pos).unwrap();
            assert!((q.x - own[0]).hypot(q.y - own[1]) < 0.01, "camera layer disagrees with the solve");
            errs.push((q.x - obs[0]).hypot(q.y - obs[1]));
        }
    }
    assert!(errs.len() > 100, "{}", errs.len());
    errs.sort_by(f64::total_cmp);
    let p95 = errs[errs.len() * 95 / 100];
    let mean = errs.iter().sum::<f64>() / errs.len() as f64;
    assert!(p95 < 1.0 && mean < 0.5, "p95 {p95} px, mean {mean} px");
}

/// The solid lies on the selected points' plane and faces the camera.
fn assert_solid_on_plane(s: &Session, comp: &Comp, sl: &Layer, pl: &Placed, size: f64) {
    let cid = s.active_comp_id().unwrap();
    let tr = sl.props.sub("transform").unwrap();
    let pos = tr.get("position").unwrap().value.as_vec3();
    let ori = tr.get("orientation").unwrap().value.as_vec3();
    let normal = linalg::mv(&linalg::from_euler_xyz(ori), [0.0, 0.0, -1.0]);
    for id in &s.state.camera_points {
        let w = pl.to_world(pl.solve.point(*id).unwrap().pos);
        let d = linalg::dot(linalg::sub(w, pos), normal).abs();
        assert!(d < 0.05 * size.max(1.0) + 1.0, "point {id} is {d} px off the plane");
    }
    let ctx = EvalCtx::new(&s.project, cid, comp, frame_time(12));
    let eye = active_camera(&ctx).eye;
    assert!(linalg::dot(normal, linalg::sub([eye.x, eye.y, eye.z], pos)) > 0.0);
}

/// A second create reuses the camera (Create Multiple Nulls and Camera), then Text and
/// Shadow Catcher (+ light).
fn assert_create_kinds(s: &mut Session, cam: LayerId, n_floor: usize) {
    let r = s.execute_checked("camera.createFromSolve", json!({"kind": "null", "multiple": true})).unwrap();
    assert_eq!(r["camera"].as_u64().unwrap(), cam.0);
    assert_eq!(r["layers"].as_array().unwrap().len(), n_floor);
    // Text and Shadow Catcher (+ light).
    let r = s.execute_checked("camera.createFromSolve", json!({"kind": "text"})).unwrap();
    assert_eq!(r["layers"].as_array().unwrap().len(), 1);
    let r = s.execute_checked("camera.createFromSolve", json!({"kind": "shadowCatcher"})).unwrap();
    let ids: Vec<LayerId> = r["layers"].as_array().unwrap().iter().map(|v| LayerId(v.as_u64().unwrap())).collect();
    let comp = s.active_comp().unwrap();
    assert!(comp.layer(ids[0]).unwrap().name.starts_with("Shadow Catcher"));
    assert_eq!(comp.layer(ids[0]).unwrap().props.prop("materialOptions/acceptsShadows").unwrap().value.as_enum(), 2);
    assert!(comp.layer(ids[1]).unwrap().is_light());
    assert!(s.execute_checked("camera.createFromSolve", json!({"kind": "teapot"})).is_err());
}

/// Set Ground Plane and Origin, then an explicit target.
fn assert_ground_and_target(s: &mut Session) {
    s.execute_checked("camera.setGroundPlane", json!({})).unwrap();
    assert_eq!(status(s)["groundPlane"], true);
    let r = s.execute_checked("camera.createFromSolve", json!({"kind": "null"})).unwrap();
    let null = LayerId(r["layers"][0].as_u64().unwrap());
    let tr = s.active_comp().unwrap().layer(null).unwrap().props.sub("transform").unwrap().clone();
    let pos = tr.get("position").unwrap().value.as_vec3();
    assert!(linalg::norm(pos) < 1e-6, "{pos:?}");
    let up = linalg::mv(&linalg::from_euler_xyz(tr.get("orientation").unwrap().value.as_vec3()), [0.0, 0.0, -1.0]);
    assert!(linalg::norm(linalg::sub(up, [0.0, -1.0, 0.0])) < 1e-6, "{up:?}");

    // An explicit target (comp world) places the layer there, its front facing the normal.
    let r = s
        .execute_checked("camera.createFromSolve", json!({"kind": "solid", "target": {"center": [10.0, 0.0, 20.0], "normal": [0.0, 0.0, -1.0], "size": 80.0}}))
        .unwrap();
    let l = s.active_comp().unwrap().layer(LayerId(r["layers"][0].as_u64().unwrap())).unwrap().clone();
    let tr = l.props.sub("transform").unwrap();
    let pos = tr.get("position").unwrap().value.as_vec3();
    assert!(linalg::norm(linalg::sub(pos, [10.0, 0.0, 20.0])) < 1e-6, "{pos:?}");
    let front = linalg::mv(&linalg::from_euler_xyz(tr.get("orientation").unwrap().value.as_vec3()), [0.0, 0.0, -1.0]);
    assert!(linalg::norm(linalg::sub(front, [0.0, 0.0, -1.0])) < 1e-6, "{front:?}");
}

#[test]
fn track_camera_solves_and_creates_layers_on_the_plane() {
    let (mut s, clip) = tracked(json!({}));
    let g = assert_solved(&mut s, clip);
    let floor = select_floor_points(&mut s);

    // Create Solid and Camera: one undo step, camera + solid.
    let r = s.execute_checked("camera.createFromSolve", json!({"kind": "solid"})).unwrap();
    let cam = LayerId(r["camera"].as_u64().unwrap());
    let solid = LayerId(r["layers"][0].as_u64().unwrap());
    let comp = s.active_comp().unwrap().clone();
    assert!(comp.layer(cam).unwrap().is_camera());
    assert_eq!(comp.layer(cam).unwrap().name, "3D Tracker Camera");
    assert_eq!(comp.layers[0].id, cam);
    let sl = comp.layer(solid).unwrap();
    assert!(sl.switches.three_d);
    assert!(sl.name.starts_with("Track Solid"));
    // The camera is keyed on every frame.
    assert_eq!(comp.layer(cam).unwrap().props.prop("transform/position").unwrap().keys.len(), 25);

    let cid = s.active_comp_id().unwrap();
    let uid = g.uid;
    let pl = crate::camera_track::placed(&s.project, cid, clip, uid).unwrap();
    assert_reprojection(&s, &comp, &pl, &g);
    assert_solid_on_plane(&s, &comp, sl, &pl, r["target"]["size"].as_f64().unwrap());

    // Undo removes both layers; redo brings them back.
    let before = comp.layers.len();
    assert!(s.undo());
    assert_eq!(s.active_comp().unwrap().layers.len(), before - 2);
    assert!(s.redo());
    assert_eq!(s.active_comp().unwrap().layers.len(), before);

    assert_create_kinds(&mut s, cam, floor.len());

    assert_ground_and_target(&mut s);

    // Serde keeps the analysis.
    let back = effectcraft_project::Project::from_json(&s.project.to_json()).unwrap();
    let g2 = back.comp(cid).unwrap().layer(clip).unwrap().props.find_group(uid).unwrap();
    let sv = ct::solve(&crate::camera_track::static_params(g2)).unwrap();
    assert!(sv.ground.is_some());
    assert_eq!(sv.points.len(), pl.solve.points.len());
}

#[test]
fn invalidation_resolve_cancel_and_undo() {
    let (mut s, clip) = tracked(json!({"shotType": "specify", "aov": 2.0 * (W as f64 / 2.0 / F).atan().to_degrees()}));
    let st = status(&mut s);
    assert_eq!(st["solved"], true, "{st}");
    assert_eq!(st["shotType"], "Specify Angle of View");
    assert!((st["focalLength"].as_f64().unwrap() - F).abs() < 0.01 * F);
    let cid = s.active_comp_id().unwrap();
    let uid = st["effect"].as_u64().unwrap();
    // Undo removes the analysis (one step); redo restores it.
    assert!(s.undo());
    assert_eq!(status(&mut s)["analyzed"], false);
    assert!(s.redo());
    assert_eq!(status(&mut s)["solved"], true);

    // Changing the Solve Method keeps the tracks and only re-solves.
    s.execute_checked("prop.set", json!({"layer": clip.0, "path": "effects/#1/advanced/solveMethod", "value": 1})).unwrap();
    let st = status(&mut s);
    assert_eq!(st["analyzed"], true);
    assert_eq!(st["solved"], false);
    assert_eq!(st["pending"], true);
    s.execute_checked("camera.analyze", json!({"wait": true})).unwrap();
    let st = status(&mut s);
    assert_eq!(st["solved"], true, "{st}");
    assert_eq!(st["methodUsed"], "Typical");

    // Deleting points re-solves without them.
    let pts = s.execute("camera.points", json!({})).unwrap();
    let del: Vec<u64> = pts["points"].as_array().unwrap().iter().take(3).map(|p| p["id"].as_u64().unwrap()).collect();
    s.execute_checked("camera.deletePoints", json!({"points": del, "wait": true})).unwrap();
    s.poll_camera(false);
    let st = status(&mut s);
    assert_eq!(st["solved"], true, "{st}");
    assert!(st["deleted"].as_u64().unwrap() >= 3);
    let g = s.project.comp(cid).unwrap().layer(clip).unwrap().props.find_group(uid).unwrap().clone();
    let sv = ct::solve(&crate::camera_track::static_params(&g)).unwrap();
    assert!(sv.points.iter().all(|p| !del.contains(&(p.id as u64))));

    // Trimming the layer changes its frames: tracks and solve are cleared and queued.
    s.edit("trim", None, |p, _| {
        p.comp_mut(cid).unwrap().layer_mut(clip).unwrap().in_point = frame_time(2);
        Ok(())
    })
    .unwrap();
    let st = status(&mut s);
    assert_eq!(st["analyzed"], false);
    assert_eq!(st["pending"], true);

    // A background analysis can be cancelled: nothing is written.
    s.execute_checked("camera.analyze", json!({})).unwrap();
    assert!(s.is_camera_analyzing() || s.camera_job.is_some());
    let banner = status(&mut s)["banner"].as_str().map(str::to_string);
    if s.is_camera_analyzing() {
        assert!(banner.unwrap().starts_with("Analyzing in background (step 1 of 2)"));
    }
    s.execute("camera.cancel", json!({})).ok();
    let t0 = std::time::Instant::now();
    while s.camera_job.is_some() {
        assert!(t0.elapsed().as_secs() < 120);
        std::thread::sleep(std::time::Duration::from_millis(5));
        s.poll_camera(false);
    }
    // (When the job finished before the cancel landed it wrote a valid analysis.)
    let st = status(&mut s);
    assert_eq!(st["running"], false);
    assert!(s.execute_checked("track.camera", json!({"layer": clip.0, "shotType": "bogus"})).is_err());
    assert!(s.execute_checked("track.camera", json!({"layer": clip.0, "aov": 500})).is_err());
}

#[test]
fn effect_parameters_match_after_effects() {
    let spec = effectcraft_effects::registry().iter().find(|e| e.id == ct::ID).unwrap();
    assert_eq!(spec.name, "3D Camera Tracker");
    assert_eq!(spec.category, "Perspective");
    let names: Vec<&str> = spec.params.iter().filter(|p| !matches!(p.ui, effectcraft_project::ParamUi::Hidden)).map(|p| p.name).collect();
    for n in [
        "Shot Type",
        "Horizontal Angle of View",
        "Show Track Points",
        "Render Track Points",
        "Track Point Size",
        "Target Size",
        "Solve Method",
        "Detailed Analysis",
        "Auto-delete Points Across Time",
        "Hide Warning Banner",
    ] {
        assert!(names.contains(&n), "{n}");
    }
    // The Animation menu entry is a real command now.
    let c = crate::find_command("track.camera").unwrap();
    assert_eq!(c.label, "Track Camera");
}

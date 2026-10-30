//! Camera and light rigs (M7.7): Create Stereo 3D Rig, Create Orbit Null, Control Light with
//! Camera, Create Cameras / Lights from 3D Model and Create Environment Light Background Layer
//! (behaviour, values, pixels, undo/redo, serde).

use std::sync::Arc;

use effectcraft_geom::{Vec3, vec3};
use effectcraft_keyframe::Value as KV;
use effectcraft_project::{Footage, FootageKind, GroupKind, ItemId, Layer, LayerId, LayerSource, LightKind, Project};
use effectcraft_render::three_d::camera::{is_two_node, layer_frame};
use effectcraft_render::{EvalCtx, FootageSource, RenderOpts};
use effectcraft_time::{FrameRate, Tick};
use serde_json::{Value, json};

use crate::{Importer, Session};

fn session() -> Session {
    let mut s = Session { footage: Arc::new(DiskModels), importer: Some(Arc::new(ModelImporter)), ..Default::default() };
    s.execute("comp.new", json!({"name": "Scene", "width": 640, "height": 360, "frameRate": 30, "duration": 2})).unwrap();
    s
}

fn cid(s: &Session) -> ItemId {
    s.active_comp_id().unwrap()
}

fn layer_in(s: &Session, comp: ItemId, id: u64) -> Layer {
    s.project.comp(comp).unwrap().layer(LayerId(id)).unwrap().clone()
}

fn v3(l: &Layer, path: &str) -> [f64; 3] {
    l.props.prop(path).unwrap().value.as_vec3()
}

fn close3(a: [f64; 3], b: [f64; 3], tol: f64) -> bool {
    (0..3).all(|i| (a[i] - b[i]).abs() <= tol)
}

/// World (eye, forward) of a layer in the active comp at time 0.
fn frame(s: &Session, id: u64) -> (Vec3, Vec3) {
    let c = cid(s);
    let comp = s.project.comp(c).unwrap();
    let ctx = EvalCtx { project: &s.project, comp_id: c, comp, time: Tick::ZERO, expr: None, footage: None, property_overrides: &[], media_overrides: &[] };
    let (e, f, _) = layer_frame(&ctx, comp.layer(LayerId(id)).unwrap());
    (e, f)
}

fn camera(s: &mut Session) -> u64 {
    s.execute("layer.newCamera", json!({"name": "Cam", "position": [320, 180, -800], "poi": [320, 180, 0], "zoom": 900})).unwrap()["layer"].as_u64().unwrap()
}

/// Controls on a hidden null in the source comp.
fn assert_stereo_controls(ctl: &Layer) {
    assert_eq!(ctl.name, "Stereo 3D Controls");
    assert!(!ctl.switches.video);
    let fx = ctl.effects().unwrap().groups().next().unwrap().clone();
    assert!(matches!(&fx.kind, GroupKind::Effect { effect } if effect == effectcraft_effects::controls::STEREO_CONTROLS));
    assert_eq!(fx.get("sceneDepth").unwrap().value, KV::Scalar(4.0));
}

/// An eye comp: an eye camera (two-node, linked by expressions) at `x` and the collapsed source comp.
fn assert_eye_comp(s: &Session, src: ItemId, ec: ItemId, name: &str, x: f64) {
    assert_eq!(s.project.item(ec).unwrap().name, name);
    let c = s.project.comp(ec).unwrap();
    assert_eq!((c.width, c.height), (640, 360));
    assert_eq!(c.layers.len(), 2);
    let cam = &c.layers[0];
    assert!(cam.is_camera() && effectcraft_render::three_d::camera::is_two_node(cam));
    assert!(close3(v3(cam, "transform/position"), [x, 180.0, -800.0], 1e-6), "{:?}", v3(cam, "transform/position"));
    // Parallel eyes (no convergence): each looks straight ahead.
    assert!(close3(v3(cam, "transform/poi"), [x, 180.0, 0.0], 1e-6), "{:?}", v3(cam, "transform/poi"));
    assert_eq!(cam.props.prop("cameraOptions/zoom").unwrap().value, KV::Scalar(900.0));
    for p in ["transform/position", "transform/poi", "cameraOptions/zoom"] {
        let e = cam.props.prop(p).unwrap().expr.clone().unwrap();
        assert!(e.text.contains("comp(\"Scene\")") && e.text.contains("Cam"), "{p}: {}", e.text);
    }
    let nest = &c.layers[1];
    assert_eq!(nest.source, LayerSource::Comp { item: src });
    assert!(nest.switches.collapse && nest.switches.three_d);
}

/// The stereo comp: left eye on top with 3D Glasses fed by both eyes; right eye hidden.
fn assert_stereo_comp(s: &Session, r: &serde_json::Value) {
    let oc = ItemId(r["stereoComp"].as_u64().unwrap());
    let out = s.project.comp(oc).unwrap();
    assert_eq!(s.project.item(oc).unwrap().name, "Scene Stereo 3D");
    let (l, rr) = (r["leftLayer"].as_u64().unwrap(), r["rightLayer"].as_u64().unwrap());
    assert_eq!(out.layers[0].id.0, l);
    assert!(!out.layers[1].switches.video);
    let g = out.layers[0].effects().unwrap().groups().next().unwrap().clone();
    assert!(matches!(&g.kind, GroupKind::Effect { effect } if effect == "ec.perspective.3dglasses"));
    assert_eq!(g.get("leftView").unwrap().value, KV::Layer(Some(l)));
    assert_eq!(g.get("rightView").unwrap().value, KV::Layer(Some(rr)));
}

#[test]
fn stereo_rig_builds_eye_comps_and_glasses() {
    let mut s = session();
    let src = cid(&s);
    let cam = camera(&mut s);
    let items_before = s.project.items.len();
    let r = s.execute("camera.stereoRig", json!({"sceneDepth": 4})).unwrap();
    assert_eq!(s.project.items.len(), items_before + 3, "left, right and stereo comps");
    assert_eq!(r["master"].as_u64(), Some(cam));
    assert_stereo_controls(&layer_in(&s, src, r["controls"].as_u64().unwrap()));
    let d = 0.04 * 640.0;
    for (key, name, x) in [("leftComp", "Scene Left Eye", 320.0 - d / 2.0), ("rightComp", "Scene Right Eye", 320.0 + d / 2.0)] {
        assert_eye_comp(&s, src, ItemId(r[key].as_u64().unwrap()), name, x);
    }
    assert_stereo_comp(&s, &r);
    // One undo step; redo; serde.
    let json = serde_json::to_string(&*s.project).unwrap();
    let back: Project = serde_json::from_str(&json).unwrap();
    assert_eq!(back, *s.project);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.project.items.len(), items_before);
    assert_eq!(s.project.comp(src).unwrap().layers.len(), 1);
    s.execute("edit.redo", json!({})).unwrap();
    assert_eq!(s.project.items.len(), items_before + 3);
}

#[test]
fn stereo_rig_configurations_and_convergence() {
    let mut s = session();
    camera(&mut s);
    let r = s.execute("camera.stereoRig", json!({"configuration": "centerRight", "sceneDepth": 5, "convergence": true})).unwrap();
    let d = 0.05 * 640.0;
    let eye = |key: &str| s.project.comp(ItemId(r[key].as_u64().unwrap())).unwrap().layers[0].clone();
    let (l, rr) = (eye("leftComp"), eye("rightComp"));
    assert!(close3(v3(&l, "transform/position"), [320.0, 180.0, -800.0], 1e-6));
    assert!(close3(v3(&rr, "transform/position"), [320.0 + d, 180.0, -800.0], 1e-6));
    // Converged on the master's point of interest.
    assert!(close3(v3(&l, "transform/poi"), [320.0, 180.0, 0.0], 1e-6));
    assert!(close3(v3(&rr, "transform/poi"), [320.0, 180.0, 0.0], 1e-6));
    // A comp without a camera gets a Master Camera.
    let mut s = session();
    let r = s.execute("camera.stereoRig", json!({"configuration": "centerLeft"})).unwrap();
    let m = layer_in(&s, cid(&s), r["master"].as_u64().unwrap());
    assert_eq!(m.name, "Master Camera");
    assert!(s.execute("camera.stereoRig", json!({"configuration": "sideways"})).is_err());
}

/// The eye cameras' expressions reproduce the static values (and follow the master).
#[test]
fn stereo_rig_maths() {
    use crate::commands::rig3d::{StereoParams, eye_camera};
    let sp = StereoParams { configuration: 0, scene_depth: 10.0, convergence: false, convergence_of: 0, z_offset: 0.0 };
    let master = (vec3(0.0, 0.0, -100.0), vec3(0.0, 0.0, 1.0), vec3(1.0, 0.0, 0.0));
    let (p0, q0) = eye_camera(&sp, 0, 1000.0, master, 100.0, 500.0);
    assert!(close3([p0.x, p0.y, p0.z], [-50.0, 0.0, -100.0], 1e-9) && close3([q0.x, q0.y, q0.z], [-50.0, 0.0, 0.0], 1e-9));
    let sp = StereoParams { convergence: true, convergence_of: 1, z_offset: 50.0, ..sp };
    let (_, q1) = eye_camera(&sp, 1, 1000.0, master, 100.0, 500.0);
    // Zoom plane (500) + Z offset (50) in front of the master eye.
    assert!(close3([q1.x, q1.y, q1.z], [0.0, 0.0, 450.0], 1e-9), "{q1:?}");
}

#[test]
fn orbit_null_parents_the_camera_at_its_point_of_interest() {
    let mut s = session();
    let cam = camera(&mut s);
    let (eye0, f0) = frame(&s, cam);
    let r = s.execute("camera.orbitNull", json!({})).unwrap();
    let nid = r["null"].as_u64().unwrap();
    let n = layer_in(&s, cid(&s), nid);
    assert!(n.is_3d() && matches!(n.source, LayerSource::Null));
    assert_eq!(n.name, "Cam Orbit Null");
    assert!(close3(v3(&n, "transform/position"), [320.0, 180.0, 0.0], 1e-9));
    let c = layer_in(&s, cid(&s), cam);
    assert_eq!(c.parent, Some(LayerId(nid)));
    // The camera stays where it was.
    let (eye1, f1) = frame(&s, cam);
    assert!((eye1 - eye0).length() < 1e-6 && (f1 - f0).length() < 1e-6, "{eye0:?} {eye1:?}");
    // Turning the null orbits the camera around the point of interest.
    s.execute("prop.set", json!({"layer": nid, "path": "transform/rotationY", "value": 90})).unwrap();
    let (eye2, f2) = frame(&s, cam);
    let poi = vec3(320.0, 180.0, 0.0);
    assert!(((eye2 - poi).length() - 800.0).abs() < 1e-6, "{eye2:?}");
    assert!((eye2.z).abs() < 1e-6 && (eye2.x - 320.0).abs() > 700.0, "{eye2:?}");
    assert!(((poi - eye2).normalize() - f2).length() < 1e-6, "still looks at the POI: {f2:?}");
    // A parented camera can't get another orbit null; undo restores the camera.
    assert!(s.execute("camera.orbitNull", json!({"layer": cam})).is_err());
    s.execute("edit.undo", json!({})).unwrap();
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(layer_in(&s, cid(&s), cam).parent, None);
    assert!(s.active_comp().unwrap().layer(LayerId(nid)).is_none());
}

#[test]
fn control_light_with_camera_follows_and_releases() {
    let mut s = session();
    let cam = camera(&mut s);
    let light = s.execute("layer.newLight", json!({"kind": "Spot", "position": [0, 0, -500], "poi": [100, 100, 0]})).unwrap()["layer"].as_u64().unwrap();
    s.execute("layer.select", json!({"layers": [light]})).unwrap();
    let r = s.execute("light.controlWithCamera", json!({})).unwrap();
    assert_eq!(r["controlled"], true);
    let l = layer_in(&s, cid(&s), light);
    assert_eq!(l.parent, Some(LayerId(cam)));
    let (ce, cf) = frame(&s, cam);
    let (le, lf) = frame(&s, light);
    assert!((le - ce).length() < 1e-6 && (lf - cf).length() < 1e-6, "light at the camera: {le:?} {lf:?}");
    // Moving the camera carries the light.
    s.execute("prop.set", json!({"layer": cam, "path": "transform/position", "value": [100, 180, -800]})).unwrap();
    let (le2, _) = frame(&s, light);
    assert!((le2 - vec3(100.0, 180.0, -800.0)).length() < 1e-6, "{le2:?}");
    // Again: released where it is (unparented, aimed the same way).
    let r = s.execute("light.controlWithCamera", json!({"layers": [light]})).unwrap();
    assert_eq!(r["controlled"], false);
    let l = layer_in(&s, cid(&s), light);
    assert_eq!(l.parent, None);
    let (le3, lf3) = frame(&s, light);
    assert!((le3 - le2).length() < 1e-6, "{le3:?}");
    let (_, cf2) = frame(&s, cam);
    assert!((lf3 - cf2).length() < 1e-6, "{lf3:?} {cf2:?}");
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(layer_in(&s, cid(&s), light).parent, Some(LayerId(cam)));
    // Not a light → error.
    assert!(s.execute("light.controlWithCamera", json!({"layers": [cam]})).is_err());
}

/// Parenting keeps a camera where it is (After Effects' Parent & Link): Position and Point of
/// Interest are re-expressed in the null's space, so the frame does not move.
#[test]
fn parented_camera_keeps_its_frame_under_an_offset_null() {
    let mut s = session();
    let cam = camera(&mut s);
    let (eye0, f0) = frame(&s, cam);
    let nul = s.execute("layer.newNull", json!({})).unwrap()["layer"].as_u64().unwrap();
    s.execute("prop.set", json!({"layer": nul, "path": "transform/position", "value": [100, 60, 0]})).unwrap();
    s.execute("layer.setParent", json!({"layers": [cam], "parent": nul})).unwrap();
    let (eye1, f1) = frame(&s, cam);
    assert!((eye1 - eye0).length() < 1e-6 && (f1 - f0).length() < 1e-6, "{eye0:?} {eye1:?}");
    // The null's origin is Position minus its default anchor (50, 50): (50, 10) in the comp.
    let c = layer_in(&s, cid(&s), cam);
    assert!(close3(v3(&c, "transform/position"), [270.0, 170.0, -800.0], 1e-6), "{:?}", v3(&c, "transform/position"));
    assert!(close3(v3(&c, "transform/poi"), [270.0, 170.0, 0.0], 1e-6), "{:?}", v3(&c, "transform/poi"));
    // Moving the null carries the camera and keeps its aim.
    s.execute("prop.set", json!({"layer": nul, "path": "transform/position", "value": [140, 90, 0]})).unwrap();
    let (eye2, f2) = frame(&s, cam);
    assert!((eye2 - (eye0 + vec3(40.0, 30.0, 0.0))).length() < 1e-6, "{eye2:?}");
    assert!((f2 - f0).length() < 1e-6, "{f2:?}");
}

/// World (eye, forward, down) of a layer in the active comp at time `t`.
fn frame_at(s: &Session, id: u64, t: Tick) -> (Vec3, Vec3, Vec3) {
    let c = cid(s);
    let comp = s.project.comp(c).unwrap();
    let ctx = EvalCtx { project: &s.project, comp_id: c, comp, time: t, expr: None, footage: None, property_overrides: &[], media_overrides: &[] };
    layer_frame(&ctx, comp.layer(LayerId(id)).unwrap())
}

/// Whether two frames agree on eye, forward and down.
fn frames_match(a: (Vec3, Vec3, Vec3), b: (Vec3, Vec3, Vec3)) -> bool {
    (a.0 - b.0).length() < 1e-6 && (a.1 - b.1).length() < 1e-6 && (a.2 - b.2).length() < 1e-6
}

/// A 3D null off-centre in the comp, with an Orientation and a Z rotation.
fn rotated_null(s: &mut Session, orientation: [f64; 3], turn: f64) -> u64 {
    let nul = s.execute("layer.newNull", json!({})).unwrap()["layer"].as_u64().unwrap();
    s.execute("layer.setSwitch", json!({"layers": [nul], "switch": "threeD", "value": true})).unwrap();
    s.execute("prop.set", json!({"layer": nul, "path": "transform/position", "value": [100, 60, 40]})).unwrap();
    s.execute("prop.set", json!({"layer": nul, "path": "transform/orientation", "value": orientation})).unwrap();
    s.execute("prop.set", json!({"layer": nul, "path": "transform/rotation", "value": turn})).unwrap();
    nul
}

/// A two-node camera under a rotated null keeps its eye, aim and roll: its Orientation is
/// re-expressed against the new aim, so Up does not drift.
#[test]
fn two_node_camera_under_rotated_null_keeps_its_roll() {
    let mut s = session();
    let cam = camera(&mut s);
    s.execute("prop.set", json!({"layer": cam, "path": "transform/rotation", "value": 25})).unwrap();
    assert!(is_two_node(&layer_in(&s, cid(&s), cam)));
    let before = frame_at(&s, cam, Tick::ZERO);
    let nul = rotated_null(&mut s, [20.0, -35.0, 60.0], 30.0);
    s.execute("layer.setParent", json!({"layers": [cam], "parent": nul})).unwrap();
    let after = frame_at(&s, cam, Tick::ZERO);
    assert!(frames_match(before, after), "{before:?} {after:?}");
}

#[test]
fn parented_point_light_keeps_its_direction_under_a_rotated_null() {
    let mut s = session();
    let light = s.execute("layer.newLight", json!({"kind": "Point", "position": [320, 180, -500]})).unwrap()["layer"].as_u64().unwrap();
    s.execute("prop.set", json!({"layer": light, "path": "transform/orientation", "value": [30, 0, 0]})).unwrap();
    let (eye0, f0) = frame(&s, light);
    let nul = s.execute("layer.newNull", json!({})).unwrap()["layer"].as_u64().unwrap();
    s.execute("prop.set", json!({"layer": nul, "path": "transform/position", "value": [100, 60, 0]})).unwrap();
    s.execute("prop.set", json!({"layer": nul, "path": "transform/rotation", "value": 30})).unwrap();
    s.execute("layer.setParent", json!({"layers": [light], "parent": nul})).unwrap();
    let (eye1, f1) = frame(&s, light);
    assert!((eye1 - eye0).length() < 1e-6 && (f1 - f0).length() < 1e-6, "{eye0:?} {eye1:?} {f0:?} {f1:?}");
}

/// Keyed Orientation under a Z-only turn is shifted exactly, so the light keeps its direction at
/// every time, between the keys too.
#[test]
fn keyed_orientation_under_a_z_turn_keeps_the_direction_at_every_time() {
    let mut s = session();
    let light = s.execute("layer.newLight", json!({"kind": "Point", "position": [320, 180, -500]})).unwrap()["layer"].as_u64().unwrap();
    for (time, value) in [(0.0, json!([-8.0, 12.0, -13.0])), (1.0, json!([5.0, 18.0, 14.0]))] {
        s.execute("prop.addKey", json!({"layer": light, "path": "transform/orientation", "time": time, "value": value})).unwrap();
    }
    let times = [0.0, 0.25, 0.5, 0.75, 1.0].map(Tick::from_seconds_f64);
    let before = times.map(|t| frame_at(&s, light, t));
    let nul = rotated_null(&mut s, [0.0, 0.0, 0.0], 30.0);
    s.execute("layer.setParent", json!({"layers": [light], "parent": nul})).unwrap();
    for (t, b) in times.into_iter().zip(before) {
        assert!(frames_match(b, frame_at(&s, light, t)), "a Z-only turn keeps the light's direction");
    }
}

/// Under a tilted null the keys alone cannot follow (Euler angles are blended between them), so keys are
/// added between them: the light keeps its direction within a small angle at every time.
#[test]
fn keyed_orientation_under_a_tilted_null_follows_between_the_keys() {
    {
        let mut s = session();
        let light = s.execute("layer.newLight", json!({"kind": "Point", "position": [320, 180, -500]})).unwrap()["layer"].as_u64().unwrap();
        for (time, value) in [(0.0, json!([-8.0, 12.0, -13.0])), (1.5, json!([50.0, 118.0, 74.0]))] {
            s.execute("prop.addKey", json!({"layer": light, "path": "transform/orientation", "time": time, "value": value})).unwrap();
        }
        let times = [0.0, 0.2, 0.5, 0.9, 1.3, 1.5].map(Tick::from_seconds_f64);
        let before = times.map(|t| frame_at(&s, light, t));
        let nul = rotated_null(&mut s, [20.0, -35.0, 60.0], 30.0);
        s.execute("layer.setParent", json!({"layers": [light], "parent": nul})).unwrap();
        for (t, b) in times.into_iter().zip(before) {
            let a = frame_at(&s, light, t);
            // 0.05 degrees is about 9e-4 of a radian on a unit vector.
            assert!((a.0 - b.0).length() < 1e-6 && (a.1 - b.1).length() < 2e-3, "at {t:?}: {:?} vs {:?}", a.1, b.1);
        }
        assert!(layer_in(&s, cid(&s), light).props.prop("transform/orientation").unwrap().keys.len() > 2, "keys were added between the two");
    }
}

/// A one-node light parented under a mirrored null (Z scale -100%) keeps the Position and
/// Orientation it was authored with: it has no Scale to carry the mirror, so none of it is
/// compensated rather than reversing its frame (a half-compensated light would point backwards).
#[test]
fn one_node_light_under_a_mirrored_null_keeps_its_authored_values() {
    let mut s = session();
    let light = s.execute("layer.newLight", json!({"kind": "Point", "position": [320, 180, -500]})).unwrap()["layer"].as_u64().unwrap();
    s.execute("prop.set", json!({"layer": light, "path": "transform/orientation", "value": [30, 0, 0]})).unwrap();
    let before = layer_in(&s, cid(&s), light);
    let nul = rotated_null(&mut s, [0.0, 0.0, 0.0], 0.0);
    s.execute("prop.set", json!({"layer": nul, "path": "transform/scale", "value": [100, 100, -100]})).unwrap();
    s.execute("layer.setParent", json!({"layers": [light], "parent": nul})).unwrap();
    let after = layer_in(&s, cid(&s), light);
    assert_eq!(after.parent, Some(LayerId(nul)));
    assert_eq!(v3(&after, "transform/position"), v3(&before, "transform/position"));
    assert_eq!(v3(&after, "transform/orientation"), v3(&before, "transform/orientation"));
}

/// The cap on keys added by baking is for the whole Orientation property, not for each segment:
/// many strongly eased segments under a tilted parent stay within it.
#[test]
fn baked_orientation_keys_are_capped_across_the_whole_property() {
    let mut s = session();
    let light = s.execute("layer.newLight", json!({"kind": "Point", "position": [320, 180, -500]})).unwrap()["layer"].as_u64().unwrap();
    let count = 120;
    for i in 0..count {
        let swing = if i % 2 == 0 { json!([0.0, 0.0, 0.0]) } else { json!([170.0, 60.0, 120.0]) };
        s.execute("prop.addKey", json!({"layer": light, "path": "transform/orientation", "time": i as f64 * 0.05, "value": swing})).unwrap();
    }
    let uid = layer_in(&s, cid(&s), light).props.prop("transform/orientation").unwrap().uid;
    let keys: Vec<_> = (0..count).map(|i| json!({"layer": light, "prop": uid, "time": i as f64 * 0.05})).collect();
    s.execute("keys.select", json!({"keys": keys})).unwrap();
    s.execute("keys.easyEase", json!({})).unwrap();
    let nul = rotated_null(&mut s, [20.0, -35.0, 60.0], 30.0);
    s.execute("layer.setParent", json!({"layers": [light], "parent": nul})).unwrap();
    let n = layer_in(&s, cid(&s), light).props.prop("transform/orientation").unwrap().keys.len();
    assert!(n <= 4096, "{n} keys: the cap is for the property, not per segment");
    // the segments the cap left unchecked keep their authored easing instead of turning into straight blends
    let orientation = layer_in(&s, cid(&s), light);
    let keys = &orientation.props.prop("transform/orientation").unwrap().keys;
    let last = keys.iter().rev().nth(1).expect("the penultimate key");
    assert_ne!(last.out_interp, effectcraft_keyframe::Interp::Linear, "the last segment keeps its authored easing");
}

// ---------------------------------------------------------------- from 3D model

/// Easy Ease agrees with a straight blend at the midpoint, but not at the quarter points.
#[test]
fn eased_orientation_under_x_parent_keeps_motion_between_keys() {
    let mut s = session();
    let light = s.execute("layer.newLight", json!({"kind": "Point", "position": [320, 180, -500]})).unwrap()["layer"].as_u64().unwrap();
    for (time, value) in [(0.0, json!([0.0, 0.0, 0.0])), (1.0, json!([90.0, 0.0, 0.0]))] {
        s.execute("prop.addKey", json!({"layer": light, "path": "transform/orientation", "time": time, "value": value})).unwrap();
    }
    let uid = layer_in(&s, cid(&s), light).props.prop("transform/orientation").unwrap().uid;
    s.execute("keys.select", json!({"keys": [{"layer": light, "prop": uid, "time": 0.0}, {"layer": light, "prop": uid, "time": 1.0}]})).unwrap();
    s.execute("keys.easyEase", json!({})).unwrap();
    let times: Vec<_> = (0..=100).map(|i| Tick::from_seconds_f64(i as f64 / 100.0)).collect();
    let before: Vec<_> = times.iter().map(|t| frame_at(&s, light, *t)).collect();
    let parent = rotated_null(&mut s, [30.0, 0.0, 0.0], 0.0);
    s.execute("layer.setParent", json!({"layers": [light], "parent": parent})).unwrap();
    for (t, b) in times.into_iter().zip(before) {
        let a = frame_at(&s, light, t);
        assert!((a.0 - b.0).length() < 1e-6 && (a.1 - b.1).length() < 1e-3, "at {t:?}: {:?} vs {:?}", a.1, b.1);
    }
}

struct DiskModels;

impl FootageSource for DiskModels {
    fn frame(&self, _: ItemId, _: &Footage, _: Tick) -> Option<Arc<effectcraft_raster::Image>> {
        None
    }
    fn model(&self, _: ItemId, f: &Footage) -> Option<Arc<effectcraft_model::Model>> {
        let bytes = std::fs::read(&f.path).ok()?;
        effectcraft_model::load(&f.path, &bytes, &|_| None).ok().map(Arc::new)
    }
}

struct ModelImporter;

impl Importer for ModelImporter {
    fn probe(&self, path: &str) -> Result<Footage, String> {
        effectcraft_model::format_of(path).ok_or("not a model")?;
        Ok(Footage { path: path.into(), kind: FootageKind::Model, frame_rate: FrameRate::FPS_30, codec: "glTF".into(), ..Default::default() })
    }
}

const RIGGED: &str = r#"{
  "asset": {"version": "2.0"},
  "scenes": [{"nodes": [0, 1, 2]}],
  "nodes": [
    {"name": "ShotCam", "camera": 0, "translation": [0, 0, 5]},
    {"name": "Key", "translation": [0, 3, 0], "rotation": [-0.7071068, 0, 0, 0.7071068], "extensions": {"KHR_lights_punctual": {"light": 0}}},
    {"name": "Fill", "translation": [2, 0, 0], "extensions": {"KHR_lights_punctual": {"light": 1}}}
  ],
  "cameras": [{"type": "perspective", "name": "Shot", "perspective": {"yfov": 0.6, "znear": 0.1}}],
  "extensions": {"KHR_lights_punctual": {"lights": [
    {"type": "spot", "name": "Key Light", "color": [1, 0.8, 0.6], "intensity": 1.5, "spot": {"innerConeAngle": 0.25, "outerConeAngle": 0.5}},
    {"type": "point", "name": "Fill Light", "intensity": 0.5, "range": 4}
  ]}}
}"#;

fn rigged_model(s: &mut Session) -> u64 {
    let dir = std::env::temp_dir().join(format!("ec-rig3d-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("rigged.gltf");
    std::fs::write(&path, RIGGED).unwrap();
    let r = s.execute("layer.newModel", json!({"path": path.to_string_lossy()})).unwrap();
    r["layer"].as_u64().unwrap()
}

#[test]
fn cameras_and_lights_from_model() {
    let mut s = session();
    let model = rigged_model(&mut s);
    let m = layer_in(&s, cid(&s), model);
    let unit = m.props.prop("geometryOptions/unitScale").unwrap().value.as_f64();
    let pos = v3(&m, "transform/position");
    s.execute("layer.select", json!({"layers": [model]})).unwrap();
    let r = s.execute("camera.fromModel", json!({})).unwrap();
    let cams = r["layers"].as_array().unwrap();
    assert_eq!(cams.len(), 1);
    let c = layer_in(&s, cid(&s), cams[0].as_u64().unwrap());
    assert_eq!(c.name, "Shot");
    // glTF +Z (towards the viewer) is AE −Z; the camera looks into the scene (+Z).
    let want = [pos[0], pos[1], pos[2] - 5.0 * unit];
    assert!(close3(v3(&c, "transform/position"), want, 1e-6), "{:?} vs {want:?}", v3(&c, "transform/position"));
    let zoom = 180.0 / (0.3f64).tan();
    assert!((c.props.prop("cameraOptions/zoom").unwrap().value.as_f64() - zoom).abs() < 1e-6);
    let (_, f) = frame(&s, c.id.0);
    assert!((f - vec3(0.0, 0.0, 1.0)).length() < 1e-6, "{f:?}");
    // Lights: a spot (cone 2 × outer, feather from inner/outer) and a point light.
    s.execute("layer.select", json!({"layers": [model]})).unwrap();
    let r = s.execute("light.fromModel", json!({})).unwrap();
    let ls: Vec<Layer> = r["layers"].as_array().unwrap().iter().map(|v| layer_in(&s, cid(&s), v.as_u64().unwrap())).collect();
    assert_eq!(ls.len(), 2);
    let key = ls.iter().find(|l| l.name == "Key Light").unwrap();
    assert_eq!(key.source, LayerSource::Light { kind: LightKind::Spot });
    assert!((key.props.prop("lightOptions/coneAngle").unwrap().value.as_f64() - 1.0f64.to_degrees()).abs() < 1e-6);
    assert!((key.props.prop("lightOptions/coneFeather").unwrap().value.as_f64() - 50.0).abs() < 1e-6);
    assert!((key.props.prop("lightOptions/intensity").unwrap().value.as_f64() - 150.0).abs() < 1e-9);
    // Points down in glTF (−Y) = down on screen in AE (+Y).
    let (ke, kf) = frame(&s, key.id.0);
    assert!((kf - vec3(0.0, 1.0, 0.0)).length() < 1e-5, "{kf:?}");
    assert!((ke.y - (pos[1] - 3.0 * unit)).abs() < 1e-6, "{ke:?}");
    let fill = ls.iter().find(|l| l.name == "Fill Light").unwrap();
    assert_eq!(fill.source, LayerSource::Light { kind: LightKind::Point });
    assert!((fill.props.prop("lightOptions/falloffDistance").unwrap().value.as_f64() - 400.0).abs() < 1e-9);
    s.execute("edit.undo", json!({})).unwrap();
    assert!(s.active_comp().unwrap().layer(key.id).is_none());
    // Not a model layer → error.
    assert!(s.execute("camera.fromModel", json!({"layers": [c.id.0]})).is_err());
}

// ---------------------------------------------------------------- environment background

fn px(img: &effectcraft_raster::Image, x: u32, y: u32) -> [f32; 4] {
    img.data[(y * img.width + x) as usize]
}

#[test]
fn environment_background_draws_the_sky_through_the_camera() {
    let mut s = session();
    let scene = cid(&s);
    // An equirectangular "sky": red upper hemisphere, blue lower.
    let sky = s.execute("comp.new", json!({"name": "Sky", "width": 200, "height": 100, "frameRate": 30, "duration": 2})).unwrap()["comp"].as_u64().unwrap();
    s.execute("layer.newSolid", json!({"color": "#0000ff", "width": 200, "height": 100})).unwrap();
    let top = s.execute("layer.newSolid", json!({"color": "#ff0000", "width": 200, "height": 50})).unwrap()["layer"].as_u64().unwrap();
    s.execute("prop.set", json!({"layer": top, "path": "transform/position", "value": [100, 25]})).unwrap();
    s.execute("comp.open", json!({"comp": scene})).unwrap();
    let src = s.execute("layer.addItem", json!({"item": sky})).unwrap()["layer"].as_u64().unwrap();
    s.execute("layer.environment", json!({"layers": [src], "on": true})).unwrap();
    // No environment light yet.
    assert!(s.execute("light.environmentBackground", json!({})).is_err());
    let env = s.execute("layer.newLight", json!({"kind": "Environment"})).unwrap()["layer"].as_u64().unwrap();
    let r = s.execute("light.environmentBackground", json!({})).unwrap();
    assert_eq!(r["light"].as_u64(), Some(env));
    assert_eq!(r["source"].as_u64(), Some(src));
    let bg = layer_in(&s, scene, r["layer"].as_u64().unwrap());
    assert!(bg.environment_background && bg.is_3d());
    assert_eq!(bg.source, LayerSource::Comp { item: ItemId(sky) });
    assert_eq!(s.active_comp().unwrap().layers.last().unwrap().id, bg.id, "at the bottom");
    let img = s.render(scene, Tick::ZERO, RenderOpts::default());
    // Looking at the horizon: rays above it see red, below it blue.
    let (t, b) = (px(&img, 320, 5), px(&img, 320, 354));
    assert!(t[0] > 0.9 && t[2] < 0.1 && t[3] > 0.99, "top {t:?}");
    assert!(b[2] > 0.9 && b[0] < 0.1, "bottom {b:?}");
    // Serde keeps the flag; undo removes the layer.
    let json = serde_json::to_value(&bg).unwrap();
    assert_eq!(json["environment_background"], Value::Bool(true));
    s.execute("edit.undo", json!({})).unwrap();
    assert!(s.active_comp().unwrap().layer(bg.id).is_none());
}

//! Camera and light rigs (M7.7): Create Stereo 3D Rig, Create Orbit Null, Control Light with
//! Camera, Create Cameras / Lights from 3D Model and Create Environment Light Background Layer
//! (behaviour, values, pixels, undo/redo, serde).

use std::sync::Arc;

use effectcraft_geom::{Vec3, vec3};
use effectcraft_keyframe::Value as KV;
use effectcraft_project::{Footage, FootageKind, GroupKind, ItemId, Layer, LayerId, LayerSource, LightKind, Project};
use effectcraft_render::three_d::camera::layer_frame;
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
    let ctx = EvalCtx { project: &s.project, comp_id: c, comp, time: Tick::ZERO, expr: None, footage: None };
    let (e, f, _) = layer_frame(&ctx, comp.layer(LayerId(id)).unwrap());
    (e, f)
}

fn camera(s: &mut Session) -> u64 {
    s.execute("layer.newCamera", json!({"name": "Cam", "position": [320, 180, -800], "poi": [320, 180, 0], "zoom": 900})).unwrap()["layer"].as_u64().unwrap()
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
    // Controls on a hidden null in the source comp.
    let ctl = layer_in(&s, src, r["controls"].as_u64().unwrap());
    assert_eq!(ctl.name, "Stereo 3D Controls");
    assert!(!ctl.switches.video);
    let fx = ctl.effects().unwrap().groups().next().unwrap().clone();
    assert!(matches!(&fx.kind, GroupKind::Effect { effect } if effect == effectcraft_effects::controls::STEREO_CONTROLS));
    assert_eq!(fx.get("sceneDepth").unwrap().value, KV::Scalar(4.0));
    // Eye comps: an eye camera (two-node, linked by expressions) and the collapsed source comp.
    let d = 0.04 * 640.0;
    for (key, name, x) in [("leftComp", "Scene Left Eye", 320.0 - d / 2.0), ("rightComp", "Scene Right Eye", 320.0 + d / 2.0)] {
        let ec = ItemId(r[key].as_u64().unwrap());
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
    // The stereo comp: left eye on top with 3D Glasses fed by both eyes; right eye hidden.
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

// ---------------------------------------------------------------- from 3D model

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

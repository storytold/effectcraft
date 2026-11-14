//! 3D commands: cameras, lights, views, camera tools (with undo/redo), and the 3D demo comp.

use effectcraft_project::{AutoOrient, LayerSource, LightKind, Renderer};
use effectcraft_render::three_d::View3D;
use serde_json::json;

use crate::Session;

fn session() -> Session {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "C", "width": 640, "height": 360, "duration": 2})).unwrap();
    s
}

#[test]
fn auto_orient_command() {
    let mut s = session();
    let id = s.execute("layer.newSolid", json!({"width": 100, "height": 100})).unwrap()["layer"].as_u64().unwrap();
    s.execute("layer.autoOrient", json!({"layer": id, "mode": "towardsCamera"})).unwrap();
    assert_eq!(s.active_comp().unwrap().layers[0].auto_orient, AutoOrient::TowardsCamera);
    s.undo();
    assert_eq!(s.active_comp().unwrap().layers[0].auto_orient, AutoOrient::Off);
    let cam = s.execute("layer.newCamera", json!({})).unwrap()["layer"].as_u64().unwrap();
    assert!(s.execute("layer.autoOrient", json!({"layer": cam, "mode": "towardsCamera"})).is_err());
    s.execute("layer.autoOrient", json!({"layer": cam, "mode": "off"})).unwrap();
    assert!(!effectcraft_render::three_d::camera::is_two_node(&s.active_comp().unwrap().layers[0]));
}

/// Layers in the active comp.
pub(crate) fn layer_count(s: &Session) -> usize {
    s.active_comp().unwrap().layers.len()
}

fn prop(s: &Session, layer: u64, path: &str) -> effectcraft_keyframe::Value {
    let l = s.active_comp().unwrap().layer(effectcraft_project::LayerId(layer)).unwrap();
    l.props.prop(path).unwrap_or_else(|| panic!("no {path}")).value.clone()
}

#[test]
fn new_camera_with_preset_undo_redo() {
    let mut s = session();
    let r = s.execute("layer.newCamera", json!({"preset": "35mm"})).unwrap();
    let id = r["layer"].as_u64().unwrap();
    assert_eq!(layer_count(&s), 1);
    let zoom = prop(&s, id, "cameraOptions/zoom").as_f64();
    assert!((zoom - 640.0 * 35.0 / 36.0).abs() < 1e-9);
    assert_eq!(prop(&s, id, "cameraOptions/focusDistance").as_f64(), zoom);
    let l = s.active_comp().unwrap().layers[0].clone();
    assert_eq!(l.source, LayerSource::Camera);
    assert_eq!(l.auto_orient, AutoOrient::TowardsPointOfInterest);
    assert!(s.undo());
    assert_eq!(layer_count(&s), 0);
    assert!(s.redo());
    assert_eq!(layer_count(&s), 1);
    assert!(s.execute("layer.newCamera", json!({"preset": "13mm-fisheye"})).is_err());
}

#[test]
fn camera_settings_one_node_keeps_view() {
    let mut s = session();
    let id = s.execute("layer.newCamera", json!({"poi": [100, 50, 0], "position": [320, 180, -400]})).unwrap()["layer"].as_u64().unwrap();
    let cid = s.active_comp_id().unwrap();
    let fwd = |s: &Session| {
        let c = s.project.comp(cid).unwrap();
        let ctx = effectcraft_render::EvalCtx::new(&s.project, cid, c, s.time());
        effectcraft_render::three_d::active_camera(&ctx).forward()
    };
    let before = fwd(&s);
    s.execute("layer.cameraSettings", json!({"layer": id, "type": "oneNode", "dof": true, "aperture": 30})).unwrap();
    let l = s.active_comp().unwrap().layers[0].clone();
    assert_eq!(l.auto_orient, AutoOrient::Off);
    assert!((fwd(&s) - before).length() < 1e-9, "one-node camera keeps looking the same way");
    assert!(prop(&s, id, "cameraOptions/dof").as_bool());
    // Layer Settings (Cmd+Shift+Y) on a camera opens Camera Settings.
    s.execute("layer.settings", json!({"layer": id, "zoom": 500})).unwrap();
    assert_eq!(prop(&s, id, "cameraOptions/zoom").as_f64(), 500.0);
    s.undo();
    s.undo();
    let l = s.active_comp().unwrap().layers[0].clone();
    assert_eq!(l.auto_orient, AutoOrient::TowardsPointOfInterest);
    assert!(!prop(&s, id, "cameraOptions/dof").as_bool());
}

#[test]
fn new_light_and_change_type() {
    let mut s = session();
    let id = s
        .execute(
            "layer.newLight",
            json!({"kind": "Spot", "intensity": 150, "color": [1, 0.5, 0.25], "coneAngle": 60, "castsShadows": true, "falloff": "Smooth"}),
        )
        .unwrap()["layer"]
        .as_u64()
        .unwrap();
    assert_eq!(prop(&s, id, "lightOptions/intensity").as_f64(), 150.0);
    assert_eq!(prop(&s, id, "lightOptions/coneAngle").as_f64(), 60.0);
    assert_eq!(prop(&s, id, "lightOptions/falloff").as_enum(), 1);
    assert!(prop(&s, id, "lightOptions/castsShadows").as_bool());
    s.execute("layer.lightSettings", json!({"layer": id, "kind": "Point"})).unwrap();
    let l = s.active_comp().unwrap().layers[0].clone();
    assert_eq!(l.source, LayerSource::Light { kind: LightKind::Point });
    assert!(l.props.prop("lightOptions/coneAngle").is_none());
    assert_eq!(prop(&s, id, "lightOptions/intensity").as_f64(), 150.0, "shared values carry over");
    s.undo();
    let l = s.active_comp().unwrap().layers[0].clone();
    assert_eq!(l.source, LayerSource::Light { kind: LightKind::Spot });
    assert!(s.execute("layer.newLight", json!({"kind": "Laser"})).is_err());
}

/// The Environment light is listed in `layer.newLight`'s help and in the kind errors, and takes
/// its equirectangular source as a layer id (#261).
#[test]
fn environment_light_is_listed_and_takes_a_source_layer() {
    let mut s = session();
    let spec = crate::command_specs().iter().find(|c| c.id == "layer.newLight").unwrap();
    assert!(spec.params.contains("lightOptions/source"), "{}", spec.params);
    let e = s.execute("layer.newLight", json!({"kind": "Laser"})).unwrap_err().to_string();
    let point = s.execute("layer.newLight", json!({"kind": "Point"})).unwrap()["layer"].as_u64().unwrap();
    let e2 = s.execute("layer.lightSettings", json!({"layer": point, "kind": "Laser"})).unwrap_err().to_string();
    for k in LightKind::ALL {
        assert!(spec.params.contains(k.label()), "help lists {}: {}", k.label(), spec.params);
        assert!(e.contains(k.label()) && e2.contains(k.label()), "{e} / {e2}");
    }
    let sky = s.execute("layer.newSolid", json!({"width": 256, "height": 128, "name": "Sky"})).unwrap()["layer"].as_u64().unwrap();
    let env = s.execute("layer.newLight", json!({"kind": "Environment", "name": "Env"})).unwrap()["layer"].as_u64().unwrap();
    let source = s.active_comp().unwrap().layer(effectcraft_project::LayerId(env)).unwrap().source.clone();
    assert_eq!(source, LayerSource::Light { kind: LightKind::Environment });
    assert_eq!(prop(&s, env, "lightOptions/source").as_layer(), None, "empty: the comp's Environment Layer");
    s.execute("prop.set", json!({"layer": env, "path": "lightOptions/source", "value": sky})).unwrap();
    assert_eq!(prop(&s, env, "lightOptions/source").as_layer(), Some(sky));
    s.undo();
    assert_eq!(prop(&s, env, "lightOptions/source").as_layer(), None);
}

/// Deleting the layer a Source points at leaves no dangling id: "None" means the comp's
/// Environment Layer, and a dead id rendered as nothing at all (#2153).
#[test]
fn deleting_a_source_layer_clears_the_reference() {
    let mut s = session();
    let sky = s.execute("layer.newSolid", json!({"width": 256, "height": 128, "name": "Sky"})).unwrap()["layer"].as_u64().unwrap();
    let env = s.execute("layer.newLight", json!({"kind": "Environment", "name": "Env"})).unwrap()["layer"].as_u64().unwrap();
    s.execute("prop.set", json!({"layer": env, "path": "lightOptions/source", "value": sky})).unwrap();
    assert_eq!(prop(&s, env, "lightOptions/source").as_layer(), Some(sky));
    s.execute("edit.clear", json!({"layers": [sky]})).unwrap();
    assert_eq!(prop(&s, env, "lightOptions/source").as_layer(), None, "the reference goes with the layer");
    // One history step with the delete, so undo brings both back.
    s.undo();
    assert_eq!(prop(&s, env, "lightOptions/source").as_layer(), Some(sky));
    assert!(s.active_comp().unwrap().layer(effectcraft_project::LayerId(sky)).is_some());
}

/// #2436: only the Advanced 3D renderer lights with Environment lights, and new comps are
/// Classic 3D, so one had no visible effect. Adding one (or changing a light into one) switches
/// the comp to Advanced 3D (in the same undo step, with a toast), as After Effects does.
#[test]
fn environment_lights_switch_classic_comps_to_advanced_3d() {
    let toasts = |s: &mut Session| -> Vec<String> {
        s.drain_events().into_iter().filter_map(|e| if let crate::Event::Toast { message, .. } = e { Some(message) } else { None }).collect()
    };
    let mut s = session();
    let text = s.execute("layer.newText", json!({"text": "Hi"})).unwrap()["layer"].as_u64().unwrap();
    assert_eq!(s.active_comp().unwrap().renderer, Renderer::Classic3D);
    toasts(&mut s);
    let r = s.execute("layer.newLight", json!({"kind": "Environment"})).unwrap();
    assert_eq!(r["advanced3d"], true);
    assert_eq!(s.active_comp().unwrap().renderer, Renderer::Advanced3D);
    // As with Composition Settings: the text layer gets Geometry Options.
    assert!(s.active_comp().unwrap().layer(effectcraft_project::LayerId(text)).unwrap().props.sub("geometryOptions").is_some());
    assert!(toasts(&mut s).iter().any(|m| m.contains("Advanced 3D")));
    // A second Environment light leaves the comp (and the user) alone.
    s.execute("layer.newLight", json!({"kind": "Environment"})).unwrap();
    assert!(toasts(&mut s).is_empty());
    // Undoing the creations takes the lights and the switch back together.
    s.undo();
    s.undo();
    assert_eq!(s.active_comp().unwrap().renderer, Renderer::Classic3D);
    assert_eq!(layer_count(&s), 1);
    // Changing a light to Environment switches too, and undoes with the type.
    let spot = s.execute("layer.newLight", json!({"kind": "Spot"})).unwrap()["layer"].as_u64().unwrap();
    assert_eq!(s.active_comp().unwrap().renderer, Renderer::Classic3D);
    s.execute("layer.lightSettings", json!({"layer": spot, "kind": "Environment"})).unwrap();
    assert_eq!(s.active_comp().unwrap().renderer, Renderer::Advanced3D);
    s.undo();
    assert_eq!(s.active_comp().unwrap().renderer, Renderer::Classic3D);
    assert_eq!(s.active_comp().unwrap().layer(effectcraft_project::LayerId(spot)).unwrap().source, LayerSource::Light { kind: LightKind::Spot });
    // Other light types never switch it.
    s.execute("layer.newLight", json!({"kind": "Point"})).unwrap();
    assert_eq!(s.active_comp().unwrap().renderer, Renderer::Classic3D);
    // Switching back to Classic 3D with an Environment light in the comp explains itself.
    let env = s.execute("layer.newLight", json!({"kind": "Environment"})).unwrap()["layer"].as_u64().unwrap();
    toasts(&mut s);
    s.execute("comp.renderer", json!({"renderer": "classic3d"})).unwrap();
    assert!(toasts(&mut s).iter().any(|m| m.contains("Environment light")), "the Classic 3D limit is explained");
    // Editing that Environment light (no type change) leaves the renderer the user chose.
    s.execute("layer.lightSettings", json!({"layer": env, "intensity": 80})).unwrap();
    assert_eq!(s.active_comp().unwrap().renderer, Renderer::Classic3D, "an ordinary edit doesn't re-switch");
    assert!(toasts(&mut s).is_empty(), "and doesn't toast either");
}

/// An Environment light takes Casts Shadows (with Shadow Darkness and Diffusion) like every
/// other light — the Light Settings dialog offers them for all types but Ambient — and keeps no
/// Falloff, which an environment light shining from every direction has no use for (#2436).
#[test]
fn environment_light_takes_shadow_options_but_no_falloff() {
    let mut s = session();
    let env = s.execute("layer.newLight", json!({"kind": "Environment", "castsShadows": true, "shadowDarkness": 60, "shadowDiffusion": 12})).unwrap()["layer"]
        .as_u64()
        .unwrap();
    assert!(prop(&s, env, "lightOptions/castsShadows").as_bool());
    assert_eq!(prop(&s, env, "lightOptions/shadowDarkness").as_f64(), 60.0);
    assert_eq!(prop(&s, env, "lightOptions/shadowDiffusion").as_f64(), 12.0);
    assert!(
        s.active_comp().unwrap().layer(effectcraft_project::LayerId(env)).unwrap().props.prop("lightOptions/falloff").is_none(),
        "an environment light has no Falloff"
    );
    // The shadow values carry over when the type changes, both ways.
    s.execute("layer.lightSettings", json!({"layer": env, "kind": "Spot", "coneAngle": 70})).unwrap();
    assert!(prop(&s, env, "lightOptions/castsShadows").as_bool());
    assert_eq!(prop(&s, env, "lightOptions/shadowDarkness").as_f64(), 60.0);
    assert_eq!(prop(&s, env, "lightOptions/coneAngle").as_f64(), 70.0);
    s.execute("layer.lightSettings", json!({"layer": env, "kind": "Environment"})).unwrap();
    assert!(prop(&s, env, "lightOptions/castsShadows").as_bool());
    assert_eq!(prop(&s, env, "lightOptions/source").as_layer(), None, "back to Environment: it has a Source again");
    s.undo();
    assert!(
        s.active_comp().unwrap().layer(effectcraft_project::LayerId(env)).unwrap().props.prop("lightOptions/coneAngle").is_some(),
        "back to Spot: it has a Cone Angle again"
    );
}

#[test]
fn three_d_switch_undo_redo() {
    let mut s = session();
    let id = s.execute("layer.newSolid", json!({"width": 100, "height": 100})).unwrap()["layer"].as_u64().unwrap();
    s.execute("layer.setSwitch", json!({"layer": id, "switch": "threeD", "value": true})).unwrap();
    assert!(s.active_comp().unwrap().layers[0].is_3d());
    s.undo();
    assert!(!s.active_comp().unwrap().layers[0].is_3d());
    s.redo();
    assert!(s.active_comp().unwrap().layers[0].is_3d());
    // Turning 3D off discards Z and the 3D-only rotations.
    s.execute("prop.set", json!({"layer": id, "path": "transform/position", "value": [10, 20, 300]})).unwrap();
    s.execute("prop.set", json!({"layer": id, "path": "transform/rotationX", "value": 45})).unwrap();
    s.execute("layer.setSwitch", json!({"layer": id, "switch": "threeD", "value": false})).unwrap();
    assert_eq!(prop(&s, id, "transform/position").as_vec3(), [10.0, 20.0, 0.0]);
    assert_eq!(prop(&s, id, "transform/rotationX").as_f64(), 0.0);
    s.undo();
    assert_eq!(prop(&s, id, "transform/position").as_vec3(), [10.0, 20.0, 300.0]);
}

#[test]
fn draft_3d_turns_off_lighting() {
    let mut s = session();
    let cid = s.active_comp_id().unwrap();
    let id = s.execute("layer.newSolid", json!({"width": 640, "height": 360, "color": [1, 1, 1]})).unwrap()["layer"].as_u64().unwrap();
    s.execute("layer.setSwitch", json!({"layer": id, "switch": "threeD", "value": true})).unwrap();
    s.execute("layer.newLight", json!({"kind": "Point", "intensity": 50, "position": [320, 180, -400]})).unwrap();
    let px = |s: &Session| s.render(cid, s.time(), effectcraft_render::RenderOpts::default()).get(320, 180)[0];
    assert!(px(&s) < 0.5);
    s.execute("comp.setSwitch", json!({"switch": "draft3d", "value": true})).unwrap();
    assert!((px(&s) - 1.0).abs() < 1e-5);
}

#[test]
fn views_switch_and_override_camera() {
    let mut s = session();
    let cid = s.active_comp_id().unwrap();
    let id = s.execute("layer.newSolid", json!({"width": 100, "height": 100})).unwrap()["layer"].as_u64().unwrap();
    s.execute("layer.setSwitch", json!({"layer": id, "switch": "threeD", "value": true})).unwrap();
    assert!(s.view_camera(cid).is_none());
    s.execute("view.3d.top", json!({})).unwrap();
    let cam = s.view_camera(cid).unwrap();
    assert!(cam.ortho);
    assert!((cam.forward().y - 1.0).abs() < 1e-9);
    s.execute("view.set3DView", json!({"view": "Custom View 2"})).unwrap();
    assert!(!s.view_camera(cid).unwrap().ortho);
    s.execute("view.3d.last", json!({})).unwrap();
    assert_eq!(s.state.views3d[&cid].current, View3D::Top);
    // Orbit a custom view: edits the view camera, not the project.
    s.execute("view.3d.custom1", json!({})).unwrap();
    let rev = s.revision;
    let before = s.view_camera(cid).unwrap().eye;
    s.execute("camera.orbit", json!({"yaw": 20})).unwrap();
    assert!((s.view_camera(cid).unwrap().eye - before).length() > 1.0);
    assert_eq!(s.revision, rev);
    s.execute("view.reset3DView", json!({})).unwrap();
    assert!((s.view_camera(cid).unwrap().eye - before).length() < 1e-9);
    s.execute("view.3d.activeCamera", json!({})).unwrap();
    assert!(s.view_camera(cid).is_none());
    let q = s.execute("view.get3D", json!({})).unwrap();
    assert_eq!(q["view"], "activeCamera");
    assert!(s.execute("view.set3DView", json!({"view": "sideways"})).is_err());
}

#[test]
fn camera_tools_edit_active_camera_with_undo() {
    let mut s = session();
    // No camera layer: the tools explain what is needed.
    assert!(s.execute("camera.orbit", json!({"yaw": 10})).is_err());
    let id = s.execute("layer.newCamera", json!({})).unwrap()["layer"].as_u64().unwrap();
    let p0 = prop(&s, id, "transform/position").as_vec3();
    let poi = prop(&s, id, "transform/poi").as_vec3();
    let dist = |a: [f64; 3], b: [f64; 3]| ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt();
    // A drag = several merged steps → one undo.
    let undo_len = s.history.undo.len();
    for _ in 0..5 {
        s.execute("camera.orbit", json!({"yaw": 6, "pitch": -2, "merge": "drag-1"})).unwrap();
    }
    assert_eq!(s.history.undo.len(), undo_len + 1);
    let p1 = prop(&s, id, "transform/position").as_vec3();
    assert!((dist(p1, poi) - dist(p0, poi)).abs() < 1e-6, "orbit keeps the distance to the POI");
    assert!(dist(p1, p0) > 10.0);
    s.execute("camera.pan", json!({"dx": 50, "dy": 0})).unwrap();
    let poi2 = prop(&s, id, "transform/poi").as_vec3();
    assert!(dist(poi2, poi) > 1.0, "pan moves the POI too");
    s.execute("camera.dolly", json!({"amount": 100})).unwrap();
    s.undo();
    s.undo();
    s.undo();
    assert_eq!(prop(&s, id, "transform/position").as_vec3(), p0);
    // One-node cameras orbit by re-orienting.
    s.execute("layer.cameraSettings", json!({"layer": id, "type": "oneNode"})).unwrap();
    s.execute("camera.orbit", json!({"yaw": 30})).unwrap();
    let o = prop(&s, id, "transform/orientation").as_vec3();
    assert!(o[1].abs() > 1.0, "{o:?}");
}

/// Returns the Euclidean distance between two 3D points.
fn dist3(a: [f64; 3], b: [f64; 3]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// Reads a JSON array of three numbers as a point; panics on anything else.
fn arr3(v: &serde_json::Value) -> [f64; 3] {
    let a = v.as_array().unwrap_or_else(|| panic!("not a point: {v}"));
    [a[0].as_f64().unwrap(), a[1].as_f64().unwrap(), a[2].as_f64().unwrap()]
}

/// Orbit Around Scene (#2586) turns a 3D view about the centre of the comp's 3D layers, not
/// about the view's point of interest.
#[test]
fn orbit_around_scene_turns_the_view_about_the_layers_centre() {
    let mut s = session();
    let cid = s.active_comp_id().unwrap();
    let id = s.execute("layer.newSolid", json!({"width": 100, "height": 100})).unwrap()["layer"].as_u64().unwrap();
    s.execute("layer.setSwitch", json!({"layer": id, "switch": "threeD", "value": true})).unwrap();
    s.execute("prop.set", json!({"layer": id, "path": "transform/position", "value": [500, 100, 300]})).unwrap();
    s.execute("view.3d.custom1", json!({})).unwrap();
    let cam0 = s.state.views3d[&cid].cam(View3D::Custom1, 640.0, 360.0);
    let r = s.execute("camera.orbit", json!({"yaw": 25, "mode": "aroundScene"})).unwrap();
    let pivot = arr3(&r["pivot"]);
    assert!(dist3(pivot, [500.0, 100.0, 300.0]) < 1e-6, "{pivot:?}");
    let cam1 = s.state.views3d[&cid].cam(View3D::Custom1, 640.0, 360.0);
    assert!(dist3(cam1.poi, cam0.poi) > 1.0, "the point of interest goes round the scene too");
    assert!((dist3(cam1.eye, pivot) - dist3(cam0.eye, pivot)).abs() < 1e-6);
    // Around Camera POI (the default; the old mode name still works) keeps the point of interest.
    s.execute("camera.orbit", json!({"yaw": 25, "mode": "aroundCameraTarget"})).unwrap();
    assert!(dist3(s.state.views3d[&cid].cam(View3D::Custom1, 640.0, 360.0).poi, cam1.poi) < 1e-9);
    assert!(s.execute("camera.orbit", json!({"yaw": 5, "mode": "sideways"})).is_err());
    assert!(s.execute("camera.pan", json!({"dx": 5, "mode": "aroundScene"})).is_err(), "Pan has no scene variant");
}

/// The cursor variants on a camera layer pick the point under the cursor on a 3D layer;
/// Dolly to Cursor and Dolly to Camera POI stop short of their point (#2586).
#[test]
fn cursor_and_target_camera_tools_on_a_camera_layer() {
    let mut s = session();
    let id = s.execute("layer.newSolid", json!({"width": 640, "height": 360})).unwrap()["layer"].as_u64().unwrap();
    s.execute("layer.setSwitch", json!({"layer": id, "switch": "threeD", "value": true})).unwrap();
    let cam = s.execute("layer.newCamera", json!({})).unwrap()["layer"].as_u64().unwrap();
    let poi0 = prop(&s, cam, "transform/poi").as_vec3();
    // The cursor off centre picks a point on the solid (z = 0), not the point of interest.
    let r = s.execute("camera.pan", json!({"dx": 0, "dy": 0, "mode": "underCursor", "cursor": [100, 60]})).unwrap();
    let pivot = arr3(&r["pivot"]);
    assert!(pivot[2].abs() < 1e-6, "{pivot:?}");
    assert!(dist3(pivot, poi0) > 50.0, "{pivot:?}");
    // Dolly to Cursor: a long dolly ends just short of the point, with the aim kept.
    let aim0 = {
        let (p, q) = (prop(&s, cam, "transform/position").as_vec3(), prop(&s, cam, "transform/poi").as_vec3());
        [q[0] - p[0], q[1] - p[1], q[2] - p[2]]
    };
    s.execute("camera.dolly", json!({"amount": 1.0e7, "mode": "toCursor", "point": pivot})).unwrap();
    let (p1, q1) = (prop(&s, cam, "transform/position").as_vec3(), prop(&s, cam, "transform/poi").as_vec3());
    assert!((dist3(p1, pivot) - 1.0).abs() < 1e-6, "{p1:?}");
    assert!(dist3([q1[0] - p1[0], q1[1] - p1[1], q1[2] - p1[2]], aim0) < 1e-6);
    s.undo();
    // Dolly to Camera POI: the point of interest stays and the camera stops 1 px from it.
    s.execute("camera.dolly", json!({"amount": 1.0e7, "mode": "toCameraTarget"})).unwrap();
    assert_eq!(prop(&s, cam, "transform/poi").as_vec3(), poi0);
    assert!((dist3(prop(&s, cam, "transform/position").as_vec3(), poi0) - 1.0).abs() < 1e-6);
    s.undo();
    // Orbit Around Cursor keeps both distances to the picked point.
    let p0 = prop(&s, cam, "transform/position").as_vec3();
    s.execute("camera.orbit", json!({"yaw": 20, "pitch": -5, "mode": "aroundCursor", "point": pivot})).unwrap();
    let p2 = prop(&s, cam, "transform/position").as_vec3();
    assert!((dist3(p2, pivot) - dist3(p0, pivot)).abs() < 1e-6);
    assert!(dist3(prop(&s, cam, "transform/poi").as_vec3(), poi0) > 1.0);
}

/// A camera parented to a moved and turned null: the tools pick their point in world space and
/// move the camera about it, converting into the camera's parent space (#2586).
#[test]
fn camera_tools_on_a_parented_camera_use_the_world_point() {
    let mut s = session();
    let id = s.execute("layer.newSolid", json!({"width": 100, "height": 100})).unwrap()["layer"].as_u64().unwrap();
    s.execute("layer.setSwitch", json!({"layer": id, "switch": "threeD", "value": true})).unwrap();
    s.execute("prop.set", json!({"layer": id, "path": "transform/position", "value": [400, 120, 200]})).unwrap();
    let parent = s.execute("layer.newNull", json!({})).unwrap()["layer"].as_u64().unwrap();
    s.execute("layer.setSwitch", json!({"layer": parent, "switch": "threeD", "value": true})).unwrap();
    s.execute("prop.set", json!({"layer": parent, "path": "transform/position", "value": [150, -60, 250]})).unwrap();
    s.execute("prop.set", json!({"layer": parent, "path": "transform/rotationY", "value": 30})).unwrap();
    s.execute("prop.set", json!({"layer": parent, "path": "transform/rotationX", "value": 15})).unwrap();
    let cam = s.execute("layer.newCamera", json!({})).unwrap()["layer"].as_u64().unwrap();
    s.execute("layer.setParent", json!({"layers": [cam], "parent": parent})).unwrap();
    let world_eye = |s: &mut Session| arr3(&s.execute("view.get3D", json!({})).unwrap()["camera"]["eye"]);
    let e0 = world_eye(&mut s);
    let r = s.execute("camera.orbit", json!({"yaw": 25, "pitch": -10, "mode": "aroundScene"})).unwrap();
    let pivot = arr3(&r["pivot"]);
    assert!(dist3(pivot, [400.0, 120.0, 200.0]) < 1e-6, "the scene centre is a world point: {pivot:?}");
    let e1 = world_eye(&mut s);
    assert!(dist3(e1, e0) > 10.0);
    assert!((dist3(e1, pivot) - dist3(e0, pivot)).abs() < 1e-6, "{e0:?} → {e1:?} about {pivot:?}");
    s.undo();
    s.execute("camera.dolly", json!({"amount": 1.0e7, "mode": "toCursor", "point": pivot})).unwrap();
    assert!((dist3(world_eye(&mut s), pivot) - 1.0).abs() < 1e-6);
}

/// The comp's active camera as rendered (world space).
fn active_cam(s: &Session) -> effectcraft_render::three_d::CameraState {
    let cid = s.active_comp_id().unwrap();
    let comp = s.project.comp(cid).unwrap();
    let ctx = effectcraft_render::EvalCtx {
        project: &s.project,
        comp_id: cid,
        comp,
        time: s.time(),
        expr: s.expr.as_deref(),
        footage: None,
        property_overrides: &[],
        media_overrides: &[],
        reads: None,
    };
    effectcraft_render::three_d::camera::active_camera(&ctx)
}

fn solid3d(s: &mut Session, w: u32, h: u32, pos: [f64; 3]) -> u64 {
    let id = s.execute("layer.newSolid", json!({"width": w, "height": h})).unwrap()["layer"].as_u64().unwrap();
    s.execute("layer.setSwitch", json!({"layer": id, "switch": "threeD", "value": true})).unwrap();
    s.execute("prop.set", json!({"layer": id, "path": "transform/position", "value": pos})).unwrap();
    id
}

/// Pan Under Cursor on a camera layer: the grabbed point, nearer than the point of interest,
/// follows the drag exactly (#2586).
#[test]
fn pan_under_cursor_moves_the_grabbed_point_by_the_drag() {
    let mut s = session();
    solid3d(&mut s, 640, 360, [320.0, 180.0, -300.0]);
    s.execute("layer.newCamera", json!({})).unwrap();
    let r = s.execute("camera.pan", json!({"dx": 0, "dy": 0, "mode": "underCursor", "cursor": [200, 100]})).unwrap();
    let pivot = arr3(&r["pivot"]);
    assert!((pivot[2] + 300.0).abs() < 1e-6, "on the nearer solid: {pivot:?}");
    let at = effectcraft_geom::Vec3::from(pivot);
    let before = active_cam(&s).project(640.0, 360.0, at).unwrap();
    s.execute("camera.pan", json!({"dx": 30, "dy": -12, "mode": "underCursor", "point": pivot})).unwrap();
    let after = active_cam(&s).project(640.0, 360.0, at).unwrap();
    assert!((after.x - before.x - 30.0).abs() < 1e-6 && (after.y - before.y + 12.0).abs() < 1e-6, "{before:?} → {after:?}");
}

/// The point under the cursor is on a visible 3D layer: guides, layers with Video off, nulls,
/// cameras and lights in front of it are not picked (#2586).
#[test]
fn the_point_under_the_cursor_skips_hidden_and_guide_layers() {
    let mut s = session();
    solid3d(&mut s, 640, 360, [320.0, 180.0, 0.0]);
    let guide = solid3d(&mut s, 640, 360, [320.0, 180.0, -200.0]);
    s.execute("layer.setSwitch", json!({"layer": guide, "switch": "guide", "value": true})).unwrap();
    let hidden = solid3d(&mut s, 640, 360, [320.0, 180.0, -300.0]);
    s.execute("layer.setSwitch", json!({"layer": hidden, "switch": "video", "value": false})).unwrap();
    let nul = s.execute("layer.newNull", json!({})).unwrap()["layer"].as_u64().unwrap();
    s.execute("layer.setSwitch", json!({"layer": nul, "switch": "threeD", "value": true})).unwrap();
    s.execute("prop.set", json!({"layer": nul, "path": "transform/position", "value": [320, 180, -400]})).unwrap();
    s.execute("layer.newCamera", json!({})).unwrap();
    let r = s.execute("camera.orbit", json!({"yaw": 0, "mode": "aroundCursor", "cursor": [300, 170]})).unwrap();
    let pivot = arr3(&r["pivot"]);
    assert!(pivot[2].abs() < 1e-6, "the visible solid at z = 0: {pivot:?}");
    // The scene's centre counts only that solid too.
    let r = s.execute("camera.orbit", json!({"yaw": 0, "mode": "aroundScene"})).unwrap();
    assert!(dist3(arr3(&r["pivot"]), [320.0, 180.0, 0.0]) < 1e-6, "{}", r["pivot"]);
}

/// With a layer soloed, a nearer non-solo plane is not drawn and so not picked: the cursor modes
/// take the soloed plane's depth, and Orbit Around Scene's centre ignores the hidden plane (#2586).
#[test]
fn the_camera_tool_pivot_respects_solo() {
    let mut s = session();
    let far = solid3d(&mut s, 640, 360, [320.0, 180.0, 0.0]);
    let near = solid3d(&mut s, 200, 200, [320.0, 180.0, -300.0]);
    s.execute("layer.newCamera", json!({})).unwrap();
    // Nothing soloed: the nearer plane under the cursor is picked.
    let r = s.execute("camera.orbit", json!({"yaw": 0, "mode": "aroundCursor", "cursor": [320, 180]})).unwrap();
    assert!((arr3(&r["pivot"])[2] + 300.0).abs() < 1e-6, "{}", r["pivot"]);
    s.execute("layer.setSwitch", json!({"layer": far, "switch": "solo", "value": true})).unwrap();
    assert_ne!(far, near);
    for (cmd, mode) in [("camera.orbit", "aroundCursor"), ("camera.pan", "underCursor"), ("camera.dolly", "towardsCursor")] {
        let r = s.execute(cmd, json!({"yaw": 0, "dx": 0, "dy": 0, "amount": 0, "mode": mode, "cursor": [320, 180]})).unwrap();
        let pivot = arr3(&r["pivot"]);
        assert!(pivot[2].abs() < 1e-6, "{cmd}: the soloed plane's depth, not the hidden one's: {pivot:?}");
    }
    // The scene centre is the soloed plane's, not the middle of both.
    let r = s.execute("camera.orbit", json!({"yaw": 0, "mode": "aroundScene"})).unwrap();
    assert!(dist3(arr3(&r["pivot"]), [320.0, 180.0, 0.0]) < 1e-6, "{}", r["pivot"]);
}

/// Orbit Around Scene without 3D layers turns about the comp's centre (#2586).
#[test]
fn orbit_around_scene_without_3d_layers_uses_the_comp_centre() {
    let mut s = session();
    s.execute("layer.newSolid", json!({"width": 100, "height": 100})).unwrap();
    s.execute("layer.newCamera", json!({})).unwrap();
    let r = s.execute("camera.orbit", json!({"yaw": 10, "mode": "aroundScene"})).unwrap();
    assert!(dist3(arr3(&r["pivot"]), [320.0, 180.0, 0.0]) < 1e-9, "{}", r["pivot"]);
}

/// A camera whose parent cannot be inverted (scale 0) works about its point of interest, stays
/// finite, and reports that point as the one used (#2586).
#[test]
fn a_parent_that_cannot_be_inverted_falls_back_to_the_point_of_interest() {
    let mut s = session();
    solid3d(&mut s, 100, 100, [400.0, 120.0, 200.0]);
    let parent = s.execute("layer.newNull", json!({})).unwrap()["layer"].as_u64().unwrap();
    s.execute("layer.setSwitch", json!({"layer": parent, "switch": "threeD", "value": true})).unwrap();
    let cam = s.execute("layer.newCamera", json!({})).unwrap()["layer"].as_u64().unwrap();
    s.execute("layer.setParent", json!({"layers": [cam], "parent": parent, "compensate": false})).unwrap();
    s.execute("prop.set", json!({"layer": parent, "path": "transform/scale", "value": [0, 0, 0]})).unwrap();
    let poi0 = prop(&s, cam, "transform/poi").as_vec3();
    // Through a scale-0 parent every point of the camera lands on the parent's position.
    let world_poi = prop(&s, parent, "transform/position").as_vec3();
    let r = s.execute("camera.orbit", json!({"yaw": 20, "pitch": -5, "mode": "aroundScene"})).unwrap();
    let pivot = arr3(&r["pivot"]);
    assert!(dist3(pivot, world_poi) < 1e-9, "the point of interest, not the scene's centre: {pivot:?}");
    assert_eq!(prop(&s, cam, "transform/poi").as_vec3(), poi0, "Orbit Around Camera POI keeps the point of interest");
    let pos = prop(&s, cam, "transform/position").as_vec3();
    assert!(pos.iter().all(|v| v.is_finite()), "{pos:?}");
    // Sent back as `point`, it falls back the same way.
    let r2 = s.execute("camera.orbit", json!({"yaw": 5, "mode": "aroundScene", "point": pivot})).unwrap();
    assert!(dist3(arr3(&r2["pivot"]), pivot) < 1e-9);
}

/// A one-node camera under a scale-0 parent: Pan Under Cursor and Dolly to Cursor work as their
/// Camera POI variants, and report the rig's point of interest through the parent, which is
/// where the parent puts every point (#2586).
#[test]
fn a_one_node_camera_under_a_scale_0_parent_pans_and_dollies_about_its_poi() {
    let setup = || {
        let mut s = session();
        solid3d(&mut s, 100, 100, [400.0, 120.0, 200.0]);
        let parent = s.execute("layer.newNull", json!({})).unwrap()["layer"].as_u64().unwrap();
        s.execute("layer.setSwitch", json!({"layer": parent, "switch": "threeD", "value": true})).unwrap();
        s.execute("prop.set", json!({"layer": parent, "path": "transform/position", "value": [100, 50, 30]})).unwrap();
        let cam = s.execute("layer.newCamera", json!({"type": "oneNode"})).unwrap()["layer"].as_u64().unwrap();
        s.execute("layer.setParent", json!({"layers": [cam], "parent": parent, "compensate": false})).unwrap();
        s.execute("prop.set", json!({"layer": parent, "path": "transform/scale", "value": [0, 0, 0]})).unwrap();
        (s, cam)
    };
    for (cmd, params, poi_mode) in [
        ("camera.pan", json!({"dx": 30, "dy": -12, "mode": "underCursor", "cursor": [200, 100]}), "cameraPoi"),
        ("camera.dolly", json!({"amount": 80, "mode": "toCursor", "cursor": [200, 100]}), "toCameraPoi"),
    ] {
        let (mut s, cam) = setup();
        let r = s.execute(cmd, params.clone()).unwrap();
        let pivot = arr3(&r["pivot"]);
        assert!(dist3(pivot, [100.0, 50.0, 30.0]) < 1e-9, "{cmd}: {pivot:?}");
        let moved = (prop(&s, cam, "transform/position").as_vec3(), prop(&s, cam, "transform/orientation").as_vec3());
        assert!(moved.0.iter().chain(moved.1.iter()).all(|v| v.is_finite()), "{cmd}: {moved:?}");
        let (mut t, cam_t) = setup();
        let mut poi_params = params;
        poi_params["mode"] = json!(poi_mode);
        t.execute(cmd, poi_params).unwrap();
        let want = (prop(&t, cam_t, "transform/position").as_vec3(), prop(&t, cam_t, "transform/orientation").as_vec3());
        assert!(dist3(moved.0, want.0) < 1e-9 && dist3(moved.1, want.1) < 1e-9, "{cmd}: {moved:?} vs {want:?}");
    }
}

#[test]
fn demo_has_3d_showcase() {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    let item = s.project.find_by_name(crate::demo::SHOWCASE_3D).unwrap();
    let cid = item.id;
    let c = s.project.comp(cid).unwrap();
    assert!(c.layers.iter().any(|l| l.is_camera()));
    assert!(c.layers.iter().any(|l| l.is_light()));
    assert!(c.layers.iter().filter(|l| l.switches.three_d).count() >= 4);
    let img =
        s.render(cid, effectcraft_time::Tick::from_seconds_f64(4.0), effectcraft_render::RenderOpts { scale: 0.125, motion_blur: false, ..Default::default() });
    let lit = img.data.iter().filter(|p| p[3] > 0.99 && p[0] + p[1] + p[2] > 0.3).count();
    assert!(lit > 2_000, "{lit}");
}

/// #670: the views of the panes of a multi-view layout are chosen per pane, kept per comp, and
/// editor states saved before panes existed load with the default Top / Front / Right.
#[test]
fn layout_panes_choose_their_view() {
    use effectcraft_render::three_d::{View3D, Views3D};
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "C", "width": 640, "height": 360, "duration": 2})).unwrap();
    let cid = s.active_comp_id().unwrap();
    assert_eq!(s.state.views3d.get(&cid).cloned().unwrap_or_default().pane(0), View3D::Top);
    let r = s.execute("view.setPaneView", json!({"pane": 2, "view": "bottom"})).unwrap();
    assert_eq!(r["views"], json!(["activeCamera", "top", "bottom", "right"]));
    assert_eq!(s.state.views3d[&cid].current, View3D::ActiveCamera, "the main view is untouched");
    s.execute("view.setPaneView", json!({"pane": 0, "view": "custom1"})).unwrap();
    assert_eq!(s.state.views3d[&cid].current, View3D::Custom1);
    assert!(s.execute("view.setPaneView", json!({"pane": 4, "view": "top"})).is_err());
    assert!(s.execute("view.setPaneView", json!({"pane": 1, "view": "nope"})).is_err());
    let back: crate::EditorState = serde_json::from_value(serde_json::to_value(&s.state).unwrap()).unwrap();
    assert_eq!(back.views3d[&cid].pane(1), View3D::Bottom);
    let old: Views3D = serde_json::from_value(json!({"current": "Front"})).unwrap();
    assert_eq!((old.current, old.pane(0), old.pane(1), old.pane(2)), (View3D::Front, View3D::Top, View3D::Front, View3D::Right));
}

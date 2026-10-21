//! Advanced 3D commands: model import → model layers, primitives, the renderer switch with
//! Geometry Options, Environment Layer and Material commands (undo/redo, serde).

use std::sync::Arc;

use effectcraft_keyframe::Value as KV;
use effectcraft_project::{Footage, FootageKind, ItemId, LayerId, LayerSource, PrimitiveKind, Project, Renderer};
use effectcraft_render::FootageSource;
use effectcraft_time::{FrameRate, Tick};
use serde_json::json;

use crate::{Importer, Session};

fn fixture(name: &str) -> String {
    format!("{}/../model/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))
}

/// Loads models from disk (what `effectcraft-media` does in the app).
struct DiskModels;

impl FootageSource for DiskModels {
    fn frame(&self, _: ItemId, _: &Footage, _: Tick) -> Option<Arc<effectcraft_raster::Image>> {
        None
    }
    fn model(&self, _: ItemId, f: &Footage) -> Option<Arc<effectcraft_model::Model>> {
        let dir = std::path::Path::new(&f.path).parent()?.to_path_buf();
        let bytes = std::fs::read(&f.path).ok()?;
        effectcraft_model::load(&f.path, &bytes, &|u| std::fs::read(dir.join(u)).ok()).ok().map(Arc::new)
    }
}

struct ModelImporter;

impl Importer for ModelImporter {
    fn probe(&self, path: &str) -> Result<Footage, String> {
        effectcraft_model::format_of(path).ok_or("not a model")?;
        Ok(Footage {
            path: path.into(),
            kind: FootageKind::Model,
            width: 0,
            height: 0,
            pixel_aspect: 1.0,
            frame_rate: FrameRate::FPS_30,
            native_rate: None,
            duration: Tick::ZERO,
            has_video: false,
            has_audio: false,
            alpha: Default::default(),
            premul_color: [0.0; 3],
            loop_count: 1,
            codec: "glTF".into(),
            missing: false,
            sequence: vec![],
            color_profile: None,
            ..Default::default()
        })
    }
}

fn session() -> Session {
    let mut s = Session { footage: Arc::new(DiskModels), importer: Some(Arc::new(ModelImporter)), ..Default::default() };
    s.execute("comp.new", json!({"name": "C", "width": 640, "height": 360, "duration": 2})).unwrap();
    s
}

fn layer(s: &Session, id: u64) -> effectcraft_project::Layer {
    s.active_comp().unwrap().layer(LayerId(id)).unwrap().clone()
}

#[test]
fn import_gltf_and_add_model_layer() {
    let mut s = session();
    let r = s.execute("file.import", json!({"paths": [fixture("quad.gltf")]})).unwrap();
    let item = r["items"][0].as_u64().unwrap();
    assert_eq!(s.project.item(ItemId(item)).unwrap().type_name(), "3D Model");
    // Adding the item to the comp makes a model layer (3D, fitted, with the clip popup).
    let id = s.execute("layer.addItem", json!({"item": item})).unwrap()["layer"].as_u64().unwrap();
    let l = layer(&s, id);
    assert_eq!(l.source, LayerSource::Model { item: ItemId(item) });
    assert!(l.switches.three_d);
    assert!(l.props.sub("masks").is_none() && l.props.sub("materialOptions").is_some());
    // Bounds 2 units tall → half the comp height.
    let scale = l.props.prop("geometryOptions/unitScale").unwrap().value.as_f64();
    assert!((scale - 90.0).abs() < 1e-6, "{scale}");
    let anim = l.props.prop("geometryOptions/animation").unwrap();
    assert_eq!(anim.value, KV::Enum(1));
    match &anim.ui {
        effectcraft_project::ParamUi::Popup { options } => assert_eq!(options, &vec!["None".to_string(), "Move".to_string()]),
        u => panic!("{u:?}"),
    }
    s.undo();
    assert!(s.active_comp().unwrap().layers.is_empty());
    s.redo();
    assert_eq!(s.active_comp().unwrap().layers.len(), 1);
    // layer.newModel imports and places in one step.
    let r = s.execute("layer.newModel", json!({"path": fixture("cube.obj")})).unwrap();
    assert!(r["layer"].as_u64().is_some());
    assert!(s.execute("layer.newModel", json!({"path": "/nonexistent/thing.png"})).is_err());
}

#[test]
fn primitives_commands() {
    let mut s = session();
    for k in PrimitiveKind::ALL {
        let r = s.execute("layer.new3dPrimitive", json!({"kind": k.label().to_lowercase()})).unwrap();
        let l = layer(&s, r["layer"].as_u64().unwrap());
        assert_eq!(l.source, LayerSource::Primitive { kind: k });
        assert_eq!(l.name, k.label());
        assert!(l.switches.three_d);
        assert!(l.props.sub("geometryOptions").is_some());
    }
    let r = s
        .execute("layer.newPrimitive", json!({"kind": "sphere", "radius": 80, "segments": 12, "baseColor": [1, 0, 0], "metallic": 100, "castsShadows": "on"}))
        .unwrap();
    let l = layer(&s, r["layer"].as_u64().unwrap());
    assert_eq!(l.props.prop("geometryOptions/radius").unwrap().value.as_f64(), 80.0);
    assert_eq!(l.props.prop("materialOptions/metallic").unwrap().value.as_f64(), 100.0);
    assert_eq!(l.props.prop("materialOptions/castsShadows").unwrap().value, KV::Enum(1));
    assert!(s.execute("layer.newPrimitive", json!({"kind": "teapot"})).is_err());
    let n = s.active_comp().unwrap().layers.len();
    s.undo();
    assert_eq!(s.active_comp().unwrap().layers.len(), n - 1);
    // Menu entries point at the real command now.
    assert!(s.is_enabled("layer.new3dPrimitive"));
}

#[test]
fn renderer_switch_adds_geometry_options() {
    let mut s = session();
    let t = s.execute("layer.newText", json!({"text": "Hi"})).unwrap()["layer"].as_u64().unwrap();
    assert!(layer(&s, t).props.sub("geometryOptions").is_none());
    s.execute("comp.renderer", json!({"renderer": "advanced3d"})).unwrap();
    assert_eq!(s.active_comp().unwrap().renderer, Renderer::Advanced3D);
    let g = layer(&s, t);
    let g = g.props.sub("geometryOptions").unwrap();
    let ids: Vec<&str> = g.children.iter().map(|c| c.match_id()).collect();
    assert_eq!(ids, vec!["bevelStyle", "bevelDepth", "holeBevelDepth", "extrusionDepth"]);
    // New shape layers in the Advanced 3D comp get them too.
    let sh = s.execute("layer.newShape", json!({})).unwrap()["layer"].as_u64().unwrap();
    assert!(layer(&s, sh).props.sub("geometryOptions").is_some());
    assert_eq!(s.execute("comp.renderer", json!({})).unwrap()["renderer"], "advanced3d");
    s.undo();
    s.undo();
    assert_eq!(s.active_comp().unwrap().renderer, Renderer::Classic3D);
    assert!(layer(&s, t).props.sub("geometryOptions").is_none());
    s.redo();
    assert_eq!(s.active_comp().unwrap().renderer, Renderer::Advanced3D);
    assert!(s.execute("comp.renderer", json!({"renderer": "raytraced"})).is_err());
    // Extrusion values are ordinary properties.
    s.execute("prop.set", json!({"layer": t, "path": "geometryOptions/extrusionDepth", "value": 40})).unwrap();
    assert_eq!(layer(&s, t).props.prop("geometryOptions/extrusionDepth").unwrap().value.as_f64(), 40.0);
}

/// Contents ▸ Add ▸ Front / Bevel / Side / Back ▸ Color (#315): one surface item per group,
/// reused by later adds, one undo step each; text gets the same as animator properties.
#[test]
fn surface_colors_for_extruded_shapes_and_text() {
    let mut s = session();
    s.execute("comp.renderer", json!({"renderer": "advanced3d"})).unwrap();
    let sh = s.execute("layer.newShape", json!({})).unwrap()["layer"].as_u64().unwrap();
    let r = s.execute("layer.addShapeItem", json!({"layer": sh, "kind": "sideColor"})).unwrap();
    assert_eq!(r["path"], "contents/side");
    let side = r["uid"].as_u64().unwrap();
    s.execute("prop.set", json!({"layer": sh, "path": "contents/side/color", "value": [0.0, 1.0, 0.0, 1.0]})).unwrap();
    // Adding Side ▸ Color again reuses the item (and keeps its colour).
    assert_eq!(s.execute("layer.addShapeItem", json!({"layer": sh, "kind": "sideColor"})).unwrap()["uid"].as_u64(), Some(side));
    let l = layer(&s, sh);
    let contents = l.props.sub("contents").unwrap();
    assert_eq!(contents.groups().filter(|g| g.match_id == "side").count(), 1);
    assert_eq!(l.props.prop("contents/side/color").unwrap().value, KV::Color([0.0, 1.0, 0.0, 1.0]));
    // Into a shape group.
    s.execute("layer.addShapeItem", json!({"layer": sh, "kind": "group"})).unwrap();
    let r = s.execute("layer.addShapeItem", json!({"layer": sh, "kind": "frontColor", "group": "contents/group"})).unwrap();
    assert_eq!(r["path"], "contents/group/contents/front");
    assert!(s.execute("layer.addShapeItem", json!({"layer": sh, "kind": "sideShininess"})).is_err());
    s.undo();
    assert!(layer(&s, sh).props.group("contents/group/contents/front").is_none());
    let cid = s.active_comp_id().unwrap();
    s.execute("prop.set", json!({"layer": sh, "path": "geometryOptions/extrusionDepth", "value": 40})).unwrap();
    let _ = s.render(cid, s.time(), Default::default());
    // Text: Animate ▸ Front / Bevel / Side / Back ▸ Color.
    let t = s.execute("layer.newText", json!({"text": "Hi"})).unwrap()["layer"].as_u64().unwrap();
    for k in ["frontColor", "bevelColor", "sideColor", "backColor"] {
        s.execute("layer.addTextAnimator", json!({"layer": t, "property": k})).unwrap();
    }
    let l = layer(&s, t);
    assert!(l.props.prop("text/animators/#3/properties/sideColor").is_some());
    s.execute("prop.set", json!({"layer": t, "path": "geometryOptions/extrusionDepth", "value": 20})).unwrap();
    let _ = s.render(cid, s.time(), Default::default());
}

#[test]
fn environment_layer_toggle() {
    let mut s = session();
    let id = s.execute("layer.newSolid", json!({"width": 64, "height": 32})).unwrap()["layer"].as_u64().unwrap();
    s.execute("layer.environment", json!({"layer": id})).unwrap();
    assert!(layer(&s, id).environment && layer(&s, id).switches.three_d);
    s.execute("layer.environment", json!({"layer": id})).unwrap();
    assert!(!layer(&s, id).environment);
    s.undo();
    assert!(layer(&s, id).environment);
    let cam = s.execute("layer.newCamera", json!({})).unwrap()["layer"].as_u64().unwrap();
    assert!(s.execute("layer.environment", json!({"layer": cam})).is_err());
    // An Environment light sources a layer.
    let li = s.execute("layer.newLight", json!({"kind": "Environment"})).unwrap()["layer"].as_u64().unwrap();
    assert!(layer(&s, li).props.prop("lightOptions/source").is_some());
}

#[test]
fn material_commands_and_serde() {
    let mut s = session();
    let a = s.execute("layer.newPrimitive", json!({"kind": "cube"})).unwrap()["layer"].as_u64().unwrap();
    let b = s.execute("layer.newPrimitive", json!({"kind": "torus"})).unwrap()["layer"].as_u64().unwrap();
    s.execute("material.set", json!({"layers": [a], "baseColor": "#3366ff", "roughness": 20, "acceptsLights": false})).unwrap();
    assert_eq!(layer(&s, a).props.prop("materialOptions/roughness").unwrap().value.as_f64(), 20.0);
    assert!(s.execute("material.set", json!({"layers": [a], "castsShadows": "sometimes"})).is_err());
    s.execute("material.duplicateAssign", json!({"layers": [a, b]})).unwrap();
    assert_eq!(layer(&s, b).props.prop("materialOptions/roughness").unwrap().value.as_f64(), 20.0);
    assert_eq!(layer(&s, b).props.prop("materialOptions/acceptsLights").unwrap().value, KV::Bool(false));
    s.execute("material.reset", json!({"layers": [a]})).unwrap();
    assert_eq!(layer(&s, a).props.prop("materialOptions/roughness").unwrap().value.as_f64(), 50.0);
    s.undo();
    assert_eq!(layer(&s, a).props.prop("materialOptions/roughness").unwrap().value.as_f64(), 20.0);
    assert!(s.execute("material.revealSource", json!({"layer": a})).is_err());
    // The project (model and primitive sources, environment flag) round-trips through JSON.
    let r = s.execute("file.import", json!({"paths": [fixture("quad.glb")]})).unwrap();
    let item = r["items"][0].as_u64().unwrap();
    let m = s.execute("layer.addItem", json!({"item": item})).unwrap()["layer"].as_u64().unwrap();
    assert_eq!(s.execute("material.revealSource", json!({"layer": m})).unwrap()["item"], item);
    let json = s.project.to_json();
    let back = Project::from_json(&json).unwrap();
    assert_eq!(&back, &*s.project);
}

/// #264: only Advanced 3D draws models and primitives, and new comps are Classic 3D, so an
/// imported model was invisible. Adding one switches the comp to Advanced 3D (in the same undo
/// step, with a toast), as After Effects does.
#[test]
fn adding_models_switches_classic_comps_to_advanced_3d() {
    let toasts = |s: &mut Session| -> Vec<String> {
        s.drain_events().into_iter().filter_map(|e| if let crate::Event::Toast { message, .. } = e { Some(message) } else { None }).collect()
    };
    let mut s = session();
    let text = s.execute("layer.newText", json!({"text": "Hi"})).unwrap()["layer"].as_u64().unwrap();
    let item = s.execute("file.import", json!({"paths": [fixture("quad.glb")]})).unwrap()["items"][0].as_u64().unwrap();
    assert_eq!(s.active_comp().unwrap().renderer, Renderer::Classic3D);
    toasts(&mut s);
    let r = s.execute("layer.addItem", json!({"item": item})).unwrap();
    assert_eq!(r["advanced3d"], true);
    assert_eq!(s.active_comp().unwrap().renderer, Renderer::Advanced3D);
    // As with Composition Settings: the text layer gets Geometry Options.
    assert!(layer(&s, text).props.sub("geometryOptions").is_some());
    assert!(toasts(&mut s).iter().any(|m| m.contains("Advanced 3D")));
    // A second model leaves the comp (and the user) alone.
    s.execute("layer.addItem", json!({"item": item})).unwrap();
    assert!(toasts(&mut s).is_empty());
    s.undo();
    s.undo();
    assert_eq!(s.active_comp().unwrap().layers.len(), 1);
    assert_eq!(s.active_comp().unwrap().renderer, Renderer::Classic3D);
    assert!(layer(&s, text).props.sub("geometryOptions").is_none());
    // Primitives too.
    s.execute("layer.newPrimitive", json!({"kind": "cube"})).unwrap();
    assert_eq!(s.active_comp().unwrap().renderer, Renderer::Advanced3D);
    // Switching back to Classic 3D says why the model layers disappear.
    toasts(&mut s);
    s.execute("comp.renderer", json!({"renderer": "classic3d"})).unwrap();
    assert!(toasts(&mut s).iter().any(|m| m.contains("Classic 3D doesn't draw")));
}

/// Importing a 3D model doesn't ask how to interpret its alpha (Interpret Footage): a model
/// renders its own.
#[test]
fn importing_a_model_does_not_ask_about_alpha() {
    let mut s = session();
    assert_eq!(s.prefs.unlabeled_alpha(false), None, "Ask User");
    let r = s.execute("file.import", json!({"paths": [fixture("quad.glb")]})).unwrap();
    assert_eq!(r["unlabeledAlpha"], json!([]));
    assert!(!s.drain_events().iter().any(|e| matches!(e, crate::Event::Frontend { command, .. } if command == "file.interpretFootage")));
}

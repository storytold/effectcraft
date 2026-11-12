//! Create 3D Layer Instance (Layer ▸ Create): a comp-sized solid at the top of the layer stack
//! with a 3D Layer Instance effect referencing the selected 3D model layer — a 2D copy of its
//! render that 2D effects can be applied to (behaviour, pixels, undo/redo, serde).

use std::sync::Arc;

use effectcraft_keyframe::Value as KV;
use effectcraft_project::{Footage, FootageKind, GroupKind, ItemId, Layer, LayerId, Project};
use effectcraft_render::{FootageSource, RenderOpts};
use effectcraft_time::{FrameRate, Tick};
use serde_json::json;

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

fn px(img: &effectcraft_raster::Image, x: u32, y: u32) -> [f32; 4] {
    img.data[(y * img.width + x) as usize]
}

fn close(a: &effectcraft_raster::Image, b: &effectcraft_raster::Image, tol: f32) -> bool {
    a.data.iter().zip(b.data.iter()).all(|(p, q)| p.iter().zip(q.iter()).all(|(x, y)| (x - y).abs() <= tol))
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

/// A red triangle in the Z = 0 plane (buffer embedded as a data URI).
const TRIANGLE: &str = r#"{
  "asset": {"version": "2.0"},
  "scenes": [{"nodes": [0]}],
  "nodes": [{"mesh": 0}],
  "meshes": [{"primitives": [{"attributes": {"POSITION": 0, "NORMAL": 1}, "material": 0}]}],
  "materials": [{"pbrMetallicRoughness": {"baseColorFactor": [1.0, 0.0, 0.0, 1.0]}}],
  "buffers": [{"byteLength": 72, "uri": "data:application/octet-stream;base64,AACAvwAAgL8AAAAAAACAPwAAgL8AAAAAAAAAAAAAgD8AAAAAAAAAAAAAAAAAAIA/AAAAAAAAAAAAAIA/AAAAAAAAAAAAAIA/"}],
  "bufferViews": [
    {"buffer": 0, "byteOffset": 0, "byteLength": 36},
    {"buffer": 0, "byteOffset": 36, "byteLength": 36}
  ],
  "accessors": [
    {"bufferView": 0, "componentType": 5126, "type": "VEC3", "count": 3, "min": [-1.0, -1.0, 0.0], "max": [1.0, 1.0, 0.0]},
    {"bufferView": 1, "componentType": 5126, "type": "VEC3", "count": 3}
  ]
}"#;

fn triangle_model(s: &mut Session) -> u64 {
    let dir = std::env::temp_dir().join(format!("ec-instance3d-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("Triangle.gltf");
    std::fs::write(&path, TRIANGLE).unwrap();
    s.execute("layer.newModel", json!({"path": path.to_string_lossy(), "name": "Triangle"})).unwrap()["layer"].as_u64().unwrap()
}

#[test]
fn instance3d_creates_a_2d_copy_of_the_model() {
    let mut s = session();
    let model = triangle_model(&mut s);
    // The model renders through the comp's camera.
    let before = s.render(cid(&s), Tick::ZERO, RenderOpts::default());
    assert!(px(&before, 320, 180)[3] > 0.9, "the model covers the centre: {:?}", px(&before, 320, 180));
    assert!(px(&before, 4, 4)[3] < 0.1, "the comp is transparent around it: {:?}", px(&before, 4, 4));
    // Create 3D Layer Instance.
    s.execute("layer.select", json!({"layers": [model]})).unwrap();
    let r = s.execute("layer.create", json!({"op": "instance3d"})).unwrap();
    let solid = r["layers"][0].as_u64().unwrap();
    let comp = cid(&s);
    let l = layer_in(&s, comp, solid);
    assert_eq!(l.name, "Triangle Instance");
    assert_eq!(s.active_comp().unwrap().layers[0].id.0, solid, "at the top of the layer stack");
    // A 3D Layer Instance effect referencing the 3D layer.
    let g = l.effects().unwrap().groups().next().unwrap().clone();
    assert!(matches!(&g.kind, GroupKind::Effect { effect } if effect == "ec.3d.instance3d"), "{:?}", g.kind);
    assert_eq!(g.get("layer").unwrap().value, KV::Layer(Some(model)));
    // The model is switched off, the copy selected.
    assert!(!layer_in(&s, comp, model).switches.video, "the referenced 3D layer is off, as After Effects does");
    assert_eq!(s.state.selected_layers, vec![LayerId(solid)]);
    // The comp renders the same through the copy: a 2D copy of the 3D layer, transparent
    // around it.
    let after = s.render(comp, Tick::ZERO, RenderOpts::default());
    assert!(close(&before, &after, 2e-2), "the copy shows the model:\n{:?}\n{:?}", px(&before, 320, 180), px(&after, 320, 180));
    assert!(px(&after, 320, 180)[3] > 0.9 && px(&after, 4, 4)[3] < 0.1, "{:?} {:?}", px(&after, 320, 180), px(&after, 4, 4));
    // Undo removes the copy and switches the model back on; redo returns it.
    s.execute("edit.undo", json!({})).unwrap();
    assert!(s.active_comp().unwrap().layer(LayerId(solid)).is_none());
    assert!(layer_in(&s, comp, model).switches.video);
    s.execute("edit.redo", json!({})).unwrap();
    assert!(s.active_comp().unwrap().layer(LayerId(solid)).is_some());
    assert!(!layer_in(&s, comp, model).switches.video);
    // Serde round-trips the layer parameter.
    let j = serde_json::to_string(&*s.project).unwrap();
    let back: Project = serde_json::from_str(&j).unwrap();
    assert_eq!(back, *s.project);
    // 2D effects can be applied to the copy (the model layer itself takes none): Fill in blue
    // recolours the triangle.
    s.execute("effect.apply", json!({"layer": solid, "effect": "ec.generate.fill"})).unwrap();
    s.execute("prop.set", json!({"layer": solid, "path": "effects/#2/color", "value": [0.0, 0.0, 1.0, 1.0]})).unwrap();
    let with_fx = s.render(comp, Tick::ZERO, RenderOpts::default());
    let p = px(&with_fx, 320, 180);
    assert!(p[2] > 0.9 && p[0] < 0.1, "the fill recolours the copy: {p:?}");
}

#[test]
fn instance3d_also_takes_3d_primitives() {
    let mut s = session();
    let prim = s.execute("layer.new3dPrimitive", json!({"kind": "cube", "width": 200, "height": 200, "depth": 200})).unwrap()["layer"].as_u64().unwrap();
    let before = s.render(cid(&s), Tick::ZERO, RenderOpts::default());
    let r = s.execute("layer.create", json!({"op": "instance3d", "layers": [prim]})).unwrap();
    let solid = r["layers"][0].as_u64().unwrap();
    let l = layer_in(&s, cid(&s), solid);
    assert_eq!(l.name, "Cube Instance");
    assert!(!layer_in(&s, cid(&s), prim).switches.video);
    let after = s.render(cid(&s), Tick::ZERO, RenderOpts::default());
    assert!(close(&before, &after, 2e-2), "the copy shows the cube");
}

#[test]
fn instance3d_needs_a_3d_layer() {
    let mut s = session();
    let solid = s.execute("layer.newSolid", json!({"color": "#808080", "width": 100, "height": 100})).unwrap()["layer"].as_u64().unwrap();
    let err = s.execute_checked("layer.create", json!({"op": "instance3d", "layers": [solid]}));
    assert!(err.is_err());
    let text = s.execute("layer.newText", json!({"text": "hi"})).unwrap()["layer"].as_u64().unwrap();
    assert!(s.execute_checked("layer.create", json!({"op": "instance3d", "layers": [text]})).is_err());
    // A 3D-switched text layer is not a model layer either (it has 2D pixels of its own).
    s.execute("layer.setSwitch", json!({"layers": [text], "switch": "threeD", "value": true})).unwrap();
    assert!(s.execute_checked("layer.create", json!({"op": "instance3d", "layers": [text]})).is_err());
}

#[test]
fn instance3d_creates_every_selected_copy_in_one_undo() {
    let mut s = session();
    let a = s.execute("layer.new3dPrimitive", json!({"kind": "cube", "width": 120, "height": 120, "depth": 120})).unwrap()["layer"].as_u64().unwrap();
    let b = s.execute("layer.new3dPrimitive", json!({"kind": "cube", "width": 120, "height": 120, "depth": 120})).unwrap()["layer"].as_u64().unwrap();
    let r = s.execute("layer.create", json!({"op": "instance3d", "layers": [a, b]})).unwrap();
    let copies = r["layers"].as_array().unwrap();
    assert_eq!(copies.len(), 2);
    let (ia, ib) = (copies[0].as_u64().unwrap(), copies[1].as_u64().unwrap());
    let comp = cid(&s);
    assert_ne!(ia, ib);
    assert_ne!(layer_in(&s, comp, ia).name, layer_in(&s, comp, ib).name, "each copy has a name of its own");
    // Both sources are off, both copies selected, above their sources in the stack.
    assert!(!layer_in(&s, comp, a).switches.video && !layer_in(&s, comp, b).switches.video);
    let mut sel = s.state.selected_layers.clone();
    let mut want = vec![LayerId(ia), LayerId(ib)];
    sel.sort_by_key(|l| l.0);
    want.sort_by_key(|l| l.0);
    assert_eq!(sel, want);
    let c = s.active_comp().unwrap();
    let pos = |id: u64| c.layers.iter().position(|l| l.id.0 == id).unwrap();
    assert!(pos(ia) < pos(a) && pos(ib) < pos(b), "the copies sit above their sources");
    // One undo removes both copies and switches both models back on; one redo returns them.
    s.execute("edit.undo", json!({})).unwrap();
    let c = s.active_comp().unwrap();
    assert!(c.layer(LayerId(ia)).is_none() && c.layer(LayerId(ib)).is_none(), "one undo removes every instance");
    assert!(layer_in(&s, comp, a).switches.video && layer_in(&s, comp, b).switches.video);
    s.execute("edit.redo", json!({})).unwrap();
    let c = s.active_comp().unwrap();
    assert!(c.layer(LayerId(ia)).is_some() && c.layer(LayerId(ib)).is_some(), "one redo returns every instance");
    assert!(!layer_in(&s, comp, a).switches.video && !layer_in(&s, comp, b).switches.video);
}

#[test]
fn instance3d_in_a_mixed_selection_only_takes_the_models() {
    let mut s = session();
    let model = triangle_model(&mut s);
    let solid = s.execute("layer.newSolid", json!({"color": "#808080", "width": 100, "height": 100})).unwrap()["layer"].as_u64().unwrap();
    let r = s.execute("layer.create", json!({"op": "instance3d", "layers": [model, solid]})).unwrap();
    let copies = r["layers"].as_array().unwrap();
    assert_eq!(copies.len(), 1, "only the model gets a copy: {r}");
    let copy = copies[0].as_u64().unwrap();
    let comp = cid(&s);
    assert!(!layer_in(&s, comp, model).switches.video, "the model is switched off");
    assert!(layer_in(&s, comp, solid).switches.video, "the plain solid is untouched");
    let skipped = r["skipped"].as_array().unwrap();
    assert_eq!(skipped.len(), 1);
    assert!(skipped[0].as_str().unwrap().contains("not a 3D model layer"), "{skipped:?}");
    // The copy references the model, not the solid.
    let l = layer_in(&s, comp, copy);
    let g = l.effects().unwrap().groups().next().unwrap().clone();
    assert_eq!(g.get("layer").unwrap().value, KV::Layer(Some(model)));
}

#[test]
fn instance3d_shows_nothing_outside_the_source_layer_range() {
    let mut s = session();
    let model = triangle_model(&mut s);
    // The model's range ends at 1 s; the comp runs to 2 s.
    let comp = cid(&s);
    s.edit("trim", None, |p, _| {
        p.comp_mut(comp).unwrap().layer_mut(LayerId(model)).unwrap().out_point = Tick::from_seconds_f64(1.0);
        Ok(())
    })
    .unwrap();
    s.execute("layer.select", json!({"layers": [model]})).unwrap();
    let r = s.execute("layer.create", json!({"op": "instance3d"})).unwrap();
    let _copy = r["layers"][0].as_u64().unwrap();
    // Inside the model's range the copy shows it, transparent around it.
    let inside = s.render(comp, Tick::from_seconds_f64(0.5), RenderOpts::default());
    assert!(px(&inside, 320, 180)[3] > 0.9, "the copy shows the model: {:?}", px(&inside, 320, 180));
    // After its out point the source has nothing to read: the copy is transparent.
    let outside = s.render(comp, Tick::from_seconds_f64(1.5), RenderOpts::default());
    assert!(outside.data.iter().all(|p| p[3] < 1e-3), "transparent after the source's out point: {:?}", px(&outside, 320, 180));
}

#[test]
fn instance3d_keeps_the_copy_visible_when_the_source_is_soloed() {
    let mut s = session();
    let model = triangle_model(&mut s);
    let comp = cid(&s);
    // Solo the model, then create the instance: any_solo hides non-soloed layers and the
    // source's video goes off, so the copy has to take the Solo over.
    s.edit("solo", None, |p, _| {
        p.comp_mut(comp).unwrap().layer_mut(LayerId(model)).unwrap().switches.solo = true;
        Ok(())
    })
    .unwrap();
    let before = s.render(comp, Tick::ZERO, RenderOpts::default());
    assert!(px(&before, 320, 180)[3] > 0.9, "the soloed model shows: {:?}", px(&before, 320, 180));
    s.execute("layer.select", json!({"layers": [model]})).unwrap();
    let r = s.execute("layer.create", json!({"op": "instance3d"})).unwrap();
    let solid = r["layers"][0].as_u64().unwrap();
    assert!(layer_in(&s, comp, solid).switches.solo, "the copy takes the source's Solo switch");
    assert!(!layer_in(&s, comp, model).switches.video, "the source keeps its video off");
    let after = s.render(comp, Tick::ZERO, RenderOpts::default());
    assert!(px(&after, 320, 180)[3] > 0.9, "the soloed copy shows the model: {:?}", px(&after, 320, 180));
    assert!(close(&before, &after, 2e-2), "the comp renders the same through the copy:\n{:?}\n{:?}", px(&before, 320, 180), px(&after, 320, 180));
}

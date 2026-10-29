//! Menu entries that used to be disabled stubs (M3.8 burn-down): mask and shape path commands,
//! mask options, Look at Layers, view layouts, Create Camera from 3D View, output modules,
//! Pre-render and Save Current Preview.

use std::sync::{Arc, Mutex};

use effectcraft_keyframe::Value as KV;
use effectcraft_project::render_queue::{OutputFormat, PostRenderAction};
use effectcraft_project::{FeatherFalloff, Footage, FootageKind, GroupKind, LayerId, LayerSource, MaskMotionBlur};
use effectcraft_time::{FrameRate, Tick};
use serde_json::json;

use crate::{Event, ExportJob, ExportResult, Exporter, Importer, Session};

fn comp() -> Session {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Main", "width": 400, "height": 300, "frameRate": 30, "duration": 4})).unwrap();
    s
}

fn layer(s: &Session, id: u64) -> effectcraft_project::Layer {
    s.active_comp().unwrap().layer(LayerId(id)).unwrap().clone()
}

/// A solid with one closed square mask; returns (layer id, mask uid).
fn masked(s: &mut Session) -> (u64, u64) {
    let l = s.execute("layer.newSolid", json!({"color": "#ffffff", "width": 400, "height": 300})).unwrap()["layer"].as_u64().unwrap();
    s.execute("mask.new", json!({"layer": l, "vertices": [[100, 100], [300, 100], [300, 200], [100, 200]], "closed": true})).unwrap();
    let m = layer(s, l).masks().unwrap().groups().next().unwrap().uid;
    (l, m)
}

fn mask_path(s: &Session, l: u64, m: u64) -> effectcraft_keyframe::ShapePath {
    let ly = layer(s, l);
    let g = ly.masks().unwrap().find_group(m).unwrap();
    g.get("path").unwrap().value.as_path().unwrap().clone()
}

fn mask_kind(s: &Session, l: u64) -> GroupKind {
    layer(s, l).masks().unwrap().groups().next().unwrap().kind.clone()
}

#[test]
fn roto_bezier_and_convert_to_bezier() {
    let mut s = comp();
    let (l, m) = masked(&mut s);
    assert!(mask_path(&s, l, m).out_tangents.iter().all(|t| *t == [0.0, 0.0]));
    s.execute("path.rotoBezier", json!({"layer": l})).unwrap();
    assert!(matches!(mask_kind(&s, l), GroupKind::Mask { roto_bezier: true, .. }));
    let p = mask_path(&s, l, m);
    // Vertex 0 at (100,100): neighbours (100,200) and (300,100) → tangent (200,-100)/6.
    assert!((p.out_tangents[0][0] - 200.0 / 6.0).abs() < 1e-9 && (p.out_tangents[0][1] + 100.0 / 6.0).abs() < 1e-9, "{:?}", p.out_tangents);
    s.execute("layer.select", json!({"layers": [l]})).unwrap();
    assert_eq!(crate::menus::checked(&s, "path.rotoBezier", &json!({})), Some(true));
    // Moving a vertex keeps the tangents automatic.
    s.execute("mask.selectVertices", json!({"vertices": [{"layer": l, "mask": m, "index": 2}]})).unwrap();
    s.execute("mask.moveVertices", json!({"delta": [50, 50]})).unwrap();
    let q = mask_path(&s, l, m);
    assert_ne!(q.out_tangents[1], p.out_tangents[1]);
    s.execute("path.convertToBezier", json!({"layer": l})).unwrap();
    assert!(matches!(mask_kind(&s, l), GroupKind::Mask { roto_bezier: false, .. }));
    assert_eq!(mask_path(&s, l, m), q, "convert keeps the tangents");
    s.execute("edit.undo", json!({})).unwrap();
    assert!(matches!(mask_kind(&s, l), GroupKind::Mask { roto_bezier: true, .. }));
}

#[test]
fn set_first_vertex_and_free_transform() {
    let mut s = comp();
    let (l, m) = masked(&mut s);
    s.state.selected_vertices.clear();
    assert!(!s.is_enabled("path.setFirstVertex"));
    s.execute("mask.selectVertices", json!({"vertices": [{"layer": l, "mask": m, "index": 2}]})).unwrap();
    assert!(s.is_enabled("path.setFirstVertex"));
    s.execute("path.setFirstVertex", json!({})).unwrap();
    let p = mask_path(&s, l, m);
    assert_eq!(p.vertices[0], [300.0, 200.0]);
    assert_eq!(p.vertices[3], [300.0, 100.0]);
    assert_eq!(s.state.selected_vertices[0].index, 0);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(mask_path(&s, l, m).vertices[0], [100.0, 100.0]);
    // Free Transform the whole mask: scale 50 % about its centre, then move.
    s.state.selected_vertices.clear();
    s.execute("path.freeTransform", json!({"layer": l, "scale": 50, "offset": [10, 0]})).unwrap();
    let p = mask_path(&s, l, m);
    assert_eq!(p.vertices[0], [160.0, 125.0]);
    assert_eq!(p.vertices[2], [260.0, 175.0]);
    // Rotation of selected vertices only.
    s.execute("mask.selectVertices", json!({"vertices": [{"layer": l, "mask": m, "index": 0}, {"layer": l, "mask": m, "index": 1}]})).unwrap();
    s.execute("path.freeTransform", json!({"rotation": 180})).unwrap();
    let q = mask_path(&s, l, m);
    assert!((q.vertices[0][0] - 260.0).abs() < 1e-9 && (q.vertices[1][0] - 160.0).abs() < 1e-9);
    assert_eq!(q.vertices[2], p.vertices[2]);
    assert!(s.execute("path.freeTransform", json!({})).is_err());
}

#[test]
fn group_and_ungroup_shapes() {
    let mut s = comp();
    let l = s.execute("layer.newShape", json!({"kind": "rect"})).unwrap()["layer"].as_u64().unwrap();
    let contents = layer(&s, l).props.sub("contents").unwrap().clone();
    let outer = contents.groups().next().unwrap().clone();
    let inner: Vec<u64> = outer.sub("contents").unwrap().groups().map(|g| g.uid).collect();
    assert!(inner.len() >= 2, "{outer:?}");
    s.execute("layer.select", json!({"layers": [l]})).unwrap();
    s.state.selected_props = inner.iter().map(|u| (LayerId(l), *u)).collect();
    assert!(s.is_enabled("path.groupShapes"));
    let g = s.execute("path.groupShapes", json!({})).unwrap()["group"].as_u64().unwrap();
    let ly = layer(&s, l);
    let o = ly.props.sub("contents").unwrap().find_group(outer.uid).unwrap();
    let kids: Vec<&str> = o.sub("contents").unwrap().groups().map(|x| x.match_id.as_str()).collect();
    assert_eq!(kids, ["group"]);
    assert_eq!(o.find_group(g).unwrap().sub("contents").unwrap().groups().count(), inner.len());
    let before = s.render_rgba8(s.active_comp_id().unwrap(), Tick::ZERO, 200).unwrap();
    // Move the new group, then ungroup: the offset is baked into its items.
    let gp = ly.props.sub("contents").unwrap().find_group(g).unwrap().sub("transform").unwrap().get("position").unwrap().uid;
    s.edit("t", None, |proj, _| {
        let cid = proj.comps().next().map(|(i, _)| *i).unwrap();
        let l = proj.comp_mut(cid).unwrap().layer_mut(LayerId(l)).unwrap();
        l.props.find_mut(gp).unwrap().value = KV::Vec2([0.0, 0.0]);
        Ok(())
    })
    .unwrap();
    s.state.selected_props = vec![(LayerId(l), g)];
    s.execute("path.ungroupShapes", json!({})).unwrap();
    let ly = layer(&s, l);
    let o = ly.props.sub("contents").unwrap().find_group(outer.uid).unwrap();
    assert_eq!(o.sub("contents").unwrap().groups().count(), inner.len());
    assert_eq!(s.render_rgba8(s.active_comp_id().unwrap(), Tick::ZERO, 200).unwrap(), before);
    s.execute("edit.undo", json!({})).unwrap();
    assert!(layer(&s, l).props.find_group(g).is_some());
}

#[test]
fn mask_motion_blur_feather_falloff_and_hide_locked() {
    let mut s = comp();
    let (l, _) = masked(&mut s);
    s.execute("layer.mask.set", json!({"layer": l, "field": "feather", "value": 40})).unwrap();
    let cid = s.active_comp_id().unwrap();
    let smooth = s.render_rgba8(cid, Tick::ZERO, 0).unwrap();
    s.execute("layer.mask.featherFalloff", json!({"layer": l, "mode": "linear"})).unwrap();
    assert!(matches!(mask_kind(&s, l), GroupKind::Mask { feather_falloff: FeatherFalloff::Linear, .. }));
    let linear = s.render_rgba8(cid, Tick::ZERO, 0).unwrap();
    assert_ne!(smooth, linear);
    s.execute("layer.select", json!({"layers": [l]})).unwrap();
    assert_eq!(crate::menus::checked(&s, "layer.mask.featherFalloff", &json!({"mode": "linear"})), Some(true));
    s.execute("layer.mask.motionBlur", json!({"layer": l, "mode": "on"})).unwrap();
    assert!(matches!(mask_kind(&s, l), GroupKind::Mask { motion_blur: MaskMotionBlur::On, .. }));
    assert!(s.execute("layer.mask.motionBlur", json!({"layer": l, "mode": "sometimes"})).is_err());
    // Mask motion blur blurs an animated mask path.
    s.execute("layer.mask.set", json!({"layer": l, "field": "feather", "value": 0})).unwrap();
    let still = s.render_rgba8(cid, Tick::from_seconds_f64(0.5), 0).unwrap();
    s.execute("prop.addKey", json!({"layer": l, "path": "masks/#1/path", "time": 0.0})).unwrap();
    s.execute("time.set", json!({"time": 1.0})).unwrap();
    s.execute("mask.selectVertices", json!({"vertices": []})).unwrap();
    s.execute("path.freeTransform", json!({"layer": l, "offset": [80, 0]})).unwrap();
    let blurred = s.render_rgba8(cid, Tick::from_seconds_f64(0.5), 0).unwrap();
    let soft = |img: &(u32, u32, Vec<u8>)| img.2.chunks(4).filter(|p| p[0] > 10 && p[0] < 245).count();
    assert!(soft(&blurred) > soft(&still) + 100, "{} vs {}", soft(&blurred), soft(&still));
    s.execute("layer.mask.motionBlur", json!({"layer": l, "mode": "off"})).unwrap();
    let sharp = s.render_rgba8(cid, Tick::from_seconds_f64(0.5), 0).unwrap();
    assert!(soft(&sharp) < soft(&blurred));
    s.execute("edit.undo", json!({})).unwrap();
    assert!(matches!(mask_kind(&s, l), GroupKind::Mask { motion_blur: MaskMotionBlur::On, .. }));
    // Hide Locked Masks is a viewer toggle.
    assert!(s.execute("layer.mask.hideLocked", json!({})).unwrap().as_bool().unwrap());
    assert_eq!(crate::menus::checked(&s, "layer.mask.hideLocked", &json!({})), Some(true));
}

#[test]
fn look_at_layers_2d_and_3d() {
    let mut s = comp();
    let a = s.execute("layer.newSolid", json!({"color": "#ff0000", "width": 40, "height": 20})).unwrap()["layer"].as_u64().unwrap();
    s.execute("layer.setTransform", json!({"layer": a, "prop": "position", "value": [50, 60]})).unwrap();
    s.drain_events();
    s.execute("layer.select", json!({"layers": [a]})).unwrap();
    let r = s.execute("view.lookAtSelected", json!({})).unwrap();
    assert_eq!(r["rect"], json!([30.0, 50.0, 70.0, 70.0]));
    assert!(s.drain_events().iter().any(|e| matches!(e, Event::Frontend { command, .. } if command == "view.lookAt")));
    // 3D: a custom view frames the layers; the active camera view moves the camera (undoable).
    s.execute("layer.setSwitch", json!({"layer": a, "switch": "threeD", "value": true})).unwrap();
    s.execute("layer.newCamera", json!({})).unwrap();
    let cid = s.active_comp_id().unwrap();
    s.execute("view.3d.custom1", json!({})).unwrap();
    let r = s.execute("view.lookAtAll", json!({})).unwrap();
    let poi = r["poi"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect::<Vec<_>>();
    assert!((poi[0] - 50.0).abs() < 1e-6 && (poi[1] - 60.0).abs() < 1e-6, "{r}");
    s.execute("view.3d.activeCamera", json!({})).unwrap();
    let cam = s.active_comp().unwrap().layers.iter().find(|l| l.is_camera()).unwrap().id;
    let before = s.active_comp().unwrap().layer(cam).unwrap().props.prop("transform/position").unwrap().value.clone();
    s.execute("layer.select", json!({"layers": [a]})).unwrap();
    s.execute("view.lookAtSelected", json!({})).unwrap();
    let c = s.project.comp(cid).unwrap().layer(cam).unwrap();
    assert_eq!(c.props.prop("transform/poi").unwrap().value.as_vec3()[..2], [50.0, 60.0]);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.project.comp(cid).unwrap().layer(cam).unwrap().props.prop("transform/position").unwrap().value, before);
}

#[test]
fn camera_from_view_and_view_layout() {
    let mut s = comp();
    s.execute("layer.newSolid", json!({"color": "#ff0000"})).unwrap();
    assert!(s.execute("camera.fromView", json!({})).is_err(), "needs a 3D view");
    s.execute("view.3d.custom2", json!({})).unwrap();
    let cid = s.active_comp_id().unwrap();
    let vc = s.state.views3d[&cid].cam(effectcraft_render::three_d::View3D::Custom2, 400.0, 300.0);
    let r = s.execute("camera.fromView", json!({})).unwrap();
    let cam = layer(&s, r["layer"].as_u64().unwrap());
    assert!(cam.is_camera());
    let pos = cam.props.prop("transform/position").unwrap().value.as_vec3();
    assert!((0..3).all(|i| (pos[i] - vc.eye[i]).abs() < 1e-6), "{pos:?} {:?}", vc.eye);
    assert_eq!(s.state.views3d[&cid].current, effectcraft_render::three_d::View3D::ActiveCamera);
    s.execute("edit.undo", json!({})).unwrap();
    assert!(!s.active_comp().unwrap().layers.iter().any(|l| l.is_camera()));
    // Switch View Layout.
    assert_eq!(s.state.view_layout, 1);
    s.execute("view.layout", json!({"views": 2})).unwrap();
    assert_eq!(s.state.view_layout, 2);
    assert_eq!(crate::menus::checked(&s, "view.layout", &json!({"views": 2})), Some(true));
    assert!(s.execute("view.layout", json!({"views": 3})).is_err());
    assert!(s.execute("view.shareViewOptions", json!({})).unwrap().as_bool().unwrap());
}

struct MockExporter {
    log: Mutex<Vec<String>>,
}

impl Exporter for MockExporter {
    fn formats(&self) -> Vec<OutputFormat> {
        OutputFormat::ALL.to_vec()
    }
    fn export(&self, job: &ExportJob, _: &mut dyn FnMut(u64, u64) -> bool) -> Result<ExportResult, String> {
        self.log.lock().unwrap().push(format!("{:?}:{}", job.item.output.format, job.path));
        Ok(ExportResult { path: job.path.to_string(), frames: 1, ..Default::default() })
    }
}

struct MockImporter;

impl Importer for MockImporter {
    fn probe(&self, path: &str) -> Result<Footage, String> {
        Ok(Footage {
            path: path.into(),
            kind: FootageKind::Video,
            width: 400,
            height: 300,
            pixel_aspect: 1.0,
            frame_rate: FrameRate::FPS_30,
            native_rate: None,
            duration: Tick::from_seconds_f64(4.0),
            has_video: true,
            has_audio: false,
            alpha: Default::default(),
            premul_color: [0.0; 3],
            loop_count: 1,
            codec: "mock".into(),
            missing: false,
            sequence: vec![],
            color_profile: None,
            ..Default::default()
        })
    }
}

fn rq() -> (Session, Arc<MockExporter>) {
    let mut s = comp();
    let ex = Arc::new(MockExporter { log: Default::default() });
    s.exporter = Some(ex.clone());
    s.importer = Some(Arc::new(MockImporter));
    (s, ex)
}

/// An output path that is absolute on every platform. `/tmp/...` has no drive letter on Windows,
/// where it means "the current drive" to the OS but resolves against the home folder's drive in
/// the render queue, so a test cannot know which drive to expect.
pub(crate) fn tmp_out(rel: &str) -> String {
    let base = std::env::temp_dir().join("effectcraft-rq-tests");
    rel.split('/').fold(base, |p, part| p.join(part)).to_string_lossy().into_owned()
}

#[test]
fn add_output_module_encodes_twice() {
    let (mut s, ex) = rq();
    assert!(!s.is_enabled("render.addOutputModule"));
    s.execute("renderQueue.add", json!({"output": tmp_out("om/main.mp4")})).unwrap();
    let r = s.execute("render.addOutputModule", json!({"format": "png"})).unwrap();
    assert_eq!(r["module"], 2);
    assert_eq!(r["outputModules"], 2);
    let extra = r["extraOutputs"][0]["outputPath"].as_str().unwrap().to_string();
    assert!(extra.ends_with(".png"), "{extra}");
    // Module 2 is editable on its own.
    s.execute("renderQueue.setOutput", json!({"index": 1, "module": 2, "path": tmp_out("om/second.mov")})).unwrap();
    assert!(s.execute("renderQueue.setOutput", json!({"index": 1, "module": 3, "path": "/tmp/x.mov"})).is_err());
    s.execute("renderQueue.render", json!({})).unwrap();
    assert_eq!(
        ex.log
            .lock()
            .unwrap()
            .iter()
            .map(|entry| {
                let (format, path) = entry.split_once(':').unwrap();
                (format.to_string(), std::path::PathBuf::from(path))
            })
            .collect::<Vec<_>>(),
        [("H264".to_string(), std::path::PathBuf::from(tmp_out("om/main.mp4"))), ("ProRes".to_string(), std::path::PathBuf::from(tmp_out("om/second.mov")))]
    );
    assert_eq!(s.project.render_queue[0].status.label(), "Done");
    assert_eq!(std::path::Path::new(s.project.render_queue[0].last_output.as_deref().unwrap()), std::path::PathBuf::from(tmp_out("om/main.mp4")));
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.project.render_queue[0].extra_outputs.len(), 1, "undo the Output To edit");
}

#[test]
fn pre_render_imports_and_replaces() {
    let (mut s, ex) = rq();
    let inner = s.active_comp_id().unwrap();
    s.execute("layer.newSolid", json!({"color": "#00ff00"})).unwrap();
    s.execute("comp.new", json!({"name": "Outer", "width": 400, "height": 300, "frameRate": 30, "duration": 4})).unwrap();
    let outer = s.active_comp_id().unwrap();
    s.execute("layer.addItem", json!({"item": inner.0})).unwrap();
    s.open_comp(inner);
    let r = s.execute("render.preRender", json!({"output": tmp_out("pre/inner.mov")})).unwrap();
    assert_eq!(r["postRenderAction"], "Import & Replace Usage");
    assert_eq!(s.project.render_queue[0].post_render, PostRenderAction::ImportAndReplace);
    s.execute("renderQueue.render", json!({})).unwrap();
    assert_eq!(ex.log.lock().unwrap().len(), 1);
    let l = &s.project.comp(outer).unwrap().layers[0];
    let LayerSource::Footage { item } = l.source else { panic!("not replaced: {:?}", l.source) };
    assert!(
        matches!(&s.project.item(item).unwrap().kind, effectcraft_project::ItemKind::Footage(f) if std::path::Path::new(&f.path) == std::path::PathBuf::from(tmp_out("pre/inner.mov")))
    );
    s.execute("edit.undo", json!({})).unwrap();
    assert!(matches!(s.project.comp(outer).unwrap().layers[0].source, LayerSource::Comp { .. }));
}

#[test]
fn post_render_actions_import_and_set_proxy() {
    let (mut s, _) = rq();
    let cid = s.active_comp_id().unwrap();
    let items_before = s.project.items.len();
    // Import: the rendered file becomes a footage item.
    let a = s.execute("renderQueue.add", json!({"output": tmp_out("post/a.mov")})).unwrap();
    s.execute("renderQueue.setOutputModule", json!({"item": a["item"], "postRenderAction": "import"})).unwrap();
    assert_eq!(s.project.render_queue[0].post_render, PostRenderAction::Import);
    s.execute("renderQueue.render", json!({})).unwrap();
    assert_eq!(s.project.items.len(), items_before + 1);
    assert!(s.project.items.values().any(
        |i| matches!(&i.kind, effectcraft_project::ItemKind::Footage(f) if std::path::Path::new(&f.path) == std::path::PathBuf::from(tmp_out("post/a.mov")))
    ));
    // Set Proxy: the comp gets the render as its proxy.
    let b = s.execute("renderQueue.add", json!({"output": tmp_out("post/b.mov")})).unwrap();
    s.execute("renderQueue.setOutputModule", json!({"item": b["item"], "postRenderAction": "Set Proxy"})).unwrap();
    s.execute("renderQueue.render", json!({})).unwrap();
    let px = s.project.item(cid).unwrap().proxy.clone().expect("proxy set");
    assert_eq!(std::path::Path::new(&px.footage.path), std::path::PathBuf::from(tmp_out("post/b.mov")));
    // None: nothing happens.
    let n = s.project.items.len();
    let c = s.execute("renderQueue.add", json!({"output": tmp_out("post/c.mov")})).unwrap();
    s.execute("renderQueue.setOutputModule", json!({"item": c["item"], "postRenderAction": "none"})).unwrap();
    s.execute("renderQueue.render", json!({})).unwrap();
    assert_eq!(s.project.items.len(), n);
    assert!(s.execute("renderQueue.setOutputModule", json!({"item": c["item"], "postRenderAction": "explode"})).is_err());
}

#[test]
fn templates_save_apply_default_and_persist() {
    let (mut s, _) = rq();
    let t = s.execute("renderQueue.templates", json!({})).unwrap();
    let names: Vec<&str> = t["renderSettings"].as_array().unwrap().iter().map(|r| r["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["Best Settings", "Draft Settings", "DV Settings", "Multi-Machine Settings", "Current Settings"]);
    assert!(t["outputModules"].as_array().unwrap().iter().any(|m| m["name"] == "High Quality with Alpha"));
    assert_eq!(t["defaults"]["renderSettings"]["Movie"], "Best Settings");
    // A new item starts from the Movie defaults.
    let a = s.execute("renderQueue.add", json!({"output": "/tmp/t/a.mp4"})).unwrap();
    assert_eq!(a["renderSettingsSummary"], "Best Settings");
    assert_eq!(a["outputModuleSummary"], "H.264 - Match Render Settings - 15 Mbps");
    // Edit it: Custom; save templates from it; make them the defaults.
    let r = s
        .execute(
            "renderQueue.setRenderSettings",
            json!({"item": a["item"], "fieldRender": "upper", "pulldown": "WSSWW", "colorDepth": 16, "effects": "allOff", "solo": "allOff", "guideLayers": "current", "frameBlending": "offForAll", "motionBlur": "onForChecked", "duration": 0.5, "storageOverflow": false}),
        )
        .unwrap();
    assert_eq!(r["renderSettingsSummary"], "Custom");
    assert_eq!(r["settings"]["field_render"], "UpperFirst");
    s.execute(
        "renderQueue.setOutputModule",
        json!({"item": a["item"], "format": "png", "channels": "rgba", "color": "premultiplied", "crop": {"top": 2, "bottom": 2}, "resize": {"width": 320, "height": 200, "lockAspect": false, "quality": "low"}}),
    )
    .unwrap();
    s.execute("renderQueue.saveTemplate", json!({"kind": "renderSettings", "name": "Interlaced 16", "item": a["item"]})).unwrap();
    s.execute("renderQueue.saveTemplate", json!({"kind": "outputModule", "name": "Small PNG", "item": a["item"], "params": {"quality": 50}})).unwrap();
    s.execute("renderQueue.setTemplateDefault", json!({"kind": "renderSettings", "slot": "movie", "name": "Interlaced 16"})).unwrap();
    s.execute("renderQueue.setTemplateDefault", json!({"kind": "outputModule", "slot": "movie", "name": "Small PNG"})).unwrap();
    assert!(s.execute("renderQueue.setTemplateDefault", json!({"kind": "outputModule", "slot": "movie", "name": "Nope"})).is_err());
    let b = s.execute("renderQueue.add", json!({"output": "/tmp/t/b_[####].png"})).unwrap();
    assert_eq!(b["renderSettingsSummary"], "Interlaced 16");
    assert_eq!(b["outputModuleSummary"], "Small PNG");
    let it = s.project.render_queue.last().unwrap().clone();
    assert_eq!(it.output.resize.width, 320);
    assert_eq!(it.output.output, "/tmp/t/b_[####].png");
    assert_eq!((b["width"].as_u64(), b["height"].as_u64()), (Some(320), Some(200)));
    // Apply built-ins to an item; the output path keeps its name with the new extension.
    let r = s.execute("renderQueue.applyTemplate", json!({"item": a["item"], "renderSettings": "DV Settings", "outputModule": "High Quality"})).unwrap();
    assert_eq!(r["renderSettingsSummary"], "DV Settings");
    assert_eq!(r["outputModuleSummary"], "High Quality");
    assert!(r["outputPath"].as_str().unwrap().ends_with("a.mov"), "{r}");
    // Persisted with the project.
    let p = effectcraft_project::Project::from_json(&s.project.to_json()).unwrap();
    assert_eq!(p.render_templates, s.project.render_templates);
    assert_eq!(p.render_queue, s.project.render_queue);
    // Delete: saved ones go; built-ins stay. Undo restores.
    s.execute("renderQueue.deleteTemplate", json!({"kind": "outputModule", "name": "Small PNG"})).unwrap();
    assert!(s.project.render_templates.output_module("Small PNG").is_none());
    assert!(s.execute("renderQueue.deleteTemplate", json!({"kind": "renderSettings", "name": "Best Settings"})).is_err());
    s.execute("edit.undo", json!({})).unwrap();
    assert!(s.project.render_templates.output_module("Small PNG").is_some());
}

#[test]
fn save_frame_as_queues_with_the_frame_defaults() {
    let (mut s, _) = rq();
    s.execute("renderQueue.setTemplateDefault", json!({"kind": "outputModule", "slot": "still", "name": "TIFF Sequence with Alpha"})).unwrap();
    let r = s.execute("comp.saveFrameAs", json!({"queue": true, "time": 1.0})).unwrap();
    assert_eq!(r["queued"], true);
    let it = s.project.render_queue.last().unwrap();
    assert_eq!(it.output.format, OutputFormat::TiffSequence);
    assert_eq!(it.settings.name, "Current Settings");
    let comp = s.project.comp(it.comp).unwrap();
    assert_eq!(it.settings.frame_count(comp), 1);
    assert_eq!(it.settings.frame_time(comp, 0), Tick::from_seconds_f64(1.0));
}

#[test]
fn log_notify_and_overflow_settings() {
    let (mut s, _) = rq();
    let a = s.execute("renderQueue.add", json!({"output": tmp_out("n/a.mp4"), "log": "plusPerFrameInfo"})).unwrap();
    assert_eq!(a["logLabel"], "Plus Per Frame Info");
    assert_eq!(std::path::Path::new(a["logPath"].as_str().unwrap()), std::path::PathBuf::from(tmp_out("n/a_RenderLog.txt")));
    s.execute("renderQueue.setLog", json!({"item": a["item"], "log": "plusSettings"})).unwrap();
    assert_eq!(s.project.render_queue[0].log, effectcraft_project::render_queue::RenderLog::PlusSettings);
    s.execute("renderQueue.setOverflowFolders", json!({"folders": ["/tmp/o1", " ", "/tmp/o2"]})).unwrap();
    assert_eq!(s.project.render_prefs.overflow_folders, ["/tmp/o1", "/tmp/o2"]);
    assert_eq!(s.execute("renderQueue.setNotify", json!({})).unwrap()["notify"], true);
    s.drain_events();
    s.execute("renderQueue.render", json!({})).unwrap();
    let ev = s.drain_events();
    assert!(ev.iter().any(|e| matches!(e, Event::Frontend { command, .. } if command == "renderQueue.notify")), "{ev:?}");
    s.execute("renderQueue.setNotify", json!({"notify": false})).unwrap();
    s.execute("renderQueue.setRender", json!({"index": 1, "render": true})).unwrap();
    s.execute("renderQueue.render", json!({})).unwrap();
    assert!(!s.drain_events().iter().any(|e| matches!(e, Event::Frontend { command, .. } if command == "renderQueue.notify")));
}

#[test]
fn save_current_preview_writes_a_movie() {
    let (mut s, ex) = rq();
    let r = s.execute("render.saveCurrentPreview", json!({"path": "/tmp/prev/clip.mov"})).unwrap();
    assert_eq!(r["path"], "/tmp/prev/clip.mov");
    assert_eq!(*ex.log.lock().unwrap(), vec!["ProRes:/tmp/prev/clip.mov".to_string()]);
    assert!(s.project.render_queue.is_empty(), "not queued");
}

#[test]
fn implemented_stubs_are_enabled_and_workspaces_are_frontend() {
    let mut s = comp();
    for id in ["view.layout", "view.lookAtAll", "camera.fromView", "render.saveCurrentPreview", "render.preRender", "layer.mask.hideLocked"] {
        assert!(s.is_enabled(id), "{id}");
    }
    for id in ["window.saveWorkspace", "window.saveWorkspaceAs", "window.editWorkspaces"] {
        s.execute(id, json!({"name": "Mine"})).unwrap();
        assert!(s.drain_events().iter().any(|e| matches!(e, Event::Frontend { command, .. } if command == id)));
    }
    // What remains a stub is still disabled.
    for id in ["layer.autoTrace", "layer.sceneEditDetection"] {
        assert!(!s.is_enabled(id), "{id}");
    }
}

/// The first path item in a shape layer's Contents, at any depth: (uid, match_id). A new shape
/// layer nests it in a group ("Rectangle 1" ▸ "Rectangle Path 1"), as After Effects does.
fn first_shape(s: &Session, l: u64) -> (u64, String) {
    fn walk(g: &effectcraft_project::PropGroup) -> Option<(u64, String)> {
        if ["rect", "ellipse", "star", "path"].contains(&g.match_id.as_str()) {
            return Some((g.uid, g.match_id.clone()));
        }
        g.groups().find_map(walk)
    }
    walk(layer(s, l).props.sub("contents").unwrap()).unwrap()
}

fn shape_path_of(s: &Session, l: u64, uid: u64) -> effectcraft_keyframe::ShapePath {
    let ly = layer(s, l);
    let g = ly.props.sub("contents").unwrap().find_group(uid).unwrap();
    let _ = &g;
    g.get("path").unwrap().value.as_path().unwrap().clone()
}

/// Convert To Bezier Path on a shape layer's parametric path gives an editable Path 1 with the
/// same geometry, as After Effects does (#932). The command only turned RotoBezier off for masks,
/// so a Rectangle, Ellipse or Polystar path could not be converted at all.
#[test]
fn convert_to_bezier_turns_a_parametric_shape_into_an_editable_path() {
    let convert = |args: serde_json::Value| {
        let mut s = comp();
        let l = s.execute("layer.newShape", args).unwrap()["layer"].as_u64().unwrap();
        let (uid, was) = first_shape(&s, l);
        assert_ne!(was, "path", "it starts out parametric");
        s.state.selected_props = vec![(LayerId(l), uid)];
        let out = s.execute("path.convertToBezier", json!({})).unwrap();
        assert_eq!(out["paths"].as_u64(), Some(1));
        let (new_uid, now) = first_shape(&s, l);
        assert_eq!(now, "path", "it is a bezier path now");
        // The new path is selected, so its vertices can be edited straight away.
        assert_eq!(s.state.selected_props, vec![(LayerId(l), new_uid)]);
        let path = shape_path_of(&s, l, new_uid);
        assert!(path.closed, "a closed shape stays closed");
        (s, l, was, path)
    };
    let radius = |v: [f64; 2]| v[0].hypot(v[1]);

    // A 120 x 80 rectangle is its four corners, joined by straight edges, from the top-right
    // corner clockwise as After Effects draws it.
    let (mut s, l, was, rect) = convert(json!({"kind": "rect", "size": [120, 80]}));
    assert_eq!(rect.vertices, vec![[60.0, -40.0], [60.0, 40.0], [-60.0, 40.0], [-60.0, -40.0]]);
    assert!(rect.in_tangents.iter().chain(&rect.out_tangents).all(|t| *t == [0.0, 0.0]), "straight edges: {rect:?}");
    // Undo brings the parametric shape back.
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(first_shape(&s, l).1, was, "undo restores the parametric shape");

    // An ellipse is four cubic arcs: a vertex on each axis, with tangents of 0.5523 x the radius
    // (the usual circle approximation). Straight lines here would be a diamond.
    let (_, _, _, ell) = convert(json!({"kind": "ellipse", "size": [120, 80]}));
    assert_eq!(ell.vertices, vec![[0.0, -40.0], [60.0, 0.0], [0.0, 40.0], [-60.0, 0.0]]);
    let kappa = 4.0 / 3.0 * (std::f64::consts::SQRT_2 - 1.0);
    for (i, want) in [60.0 * kappa, 40.0 * kappa, 60.0 * kappa, 40.0 * kappa].into_iter().enumerate() {
        let t = ell.in_tangents[i];
        assert!((radius(t) - want).abs() < 1e-6, "vertex {i}: tangent {t:?} should be {want}");
    }

    // A five-point star alternates the outer and inner radius, so ten vertices.
    let (_, _, _, star) = convert(json!({"kind": "star", "size": [120, 120]}));
    assert_eq!(star.vertices.len(), 10);
    for (i, v) in star.vertices.iter().enumerate() {
        let want = if i % 2 == 0 { 60.0 } else { 30.0 };
        assert!((radius(*v) - want).abs() < 1e-6, "vertex {i} at {v:?} should be {want} from the centre");
    }

    // A polygon has one vertex per point, all at the outer radius.
    let (_, _, _, poly) = convert(json!({"kind": "polygon", "size": [120, 120]}));
    assert!(poly.vertices.len() >= 3);
    assert!(poly.vertices.iter().all(|v| (radius(*v) - 60.0).abs() < 1e-6), "{:?}", poly.vertices);
    assert_ne!(poly.vertices.len(), star.vertices.len(), "a polygon is not a star");
}

/// The shape's own values are read, not defaults: Roundness rounds the corners and Position moves
/// the path, both at the time the conversion is made.
#[test]
fn convert_to_bezier_reads_the_shape_as_it_is_set() {
    let mut s = comp();
    let l = s.execute("layer.newShape", json!({"kind": "rect", "size": [120, 80]})).unwrap()["layer"].as_u64().unwrap();
    let (uid, _) = first_shape(&s, l);
    // Properties are addressed by their own uid.
    let pr_uid = |s: &Session, id: &str| {
        let ly = layer(s, l);
        ly.props.sub("contents").unwrap().find_group(uid).unwrap().get(id).unwrap().uid
    };
    let (round, pos) = (pr_uid(&s, "roundness"), pr_uid(&s, "position"));
    s.execute("prop.set", json!({"layer": l, "path": format!("@{round}"), "value": 20.0})).unwrap();
    s.execute("prop.set", json!({"layer": l, "path": format!("@{pos}"), "value": [10.0, 5.0]})).unwrap();

    s.state.selected_props = vec![(LayerId(l), uid)];
    s.execute("path.convertToBezier", json!({})).unwrap();
    let (new_uid, now) = first_shape(&s, l);
    assert_eq!(now, "path");
    let path = shape_path_of(&s, l, new_uid);

    // Rounded corners need more than four vertices, and curves between them.
    assert!(path.vertices.len() > 4, "roundness rounds the corners: {} vertices", path.vertices.len());
    assert!(path.in_tangents.iter().chain(&path.out_tangents).any(|t| *t != [0.0, 0.0]), "with curves");
    // Position moves the path: the bounds are centred on it, not on the origin.
    let xs: Vec<f64> = path.vertices.iter().map(|v| v[0]).collect();
    let ys: Vec<f64> = path.vertices.iter().map(|v| v[1]).collect();
    let mid = |v: &[f64]| (v.iter().copied().fold(f64::MAX, f64::min) + v.iter().copied().fold(f64::MIN, f64::max)) / 2.0;
    assert!((mid(&xs) - 10.0).abs() < 1e-6 && (mid(&ys) - 5.0).abs() < 1e-6, "centred on the position: {:?} {:?}", mid(&xs), mid(&ys));
}

/// #2201: a viewer drag on the free-transform box sends one `path.freeTransform` per frame with a
/// shared merge key; one undo must revert the whole drag, not the last frame.
#[test]
fn free_transform_drag_undoes_in_one_step() {
    let mut s = comp();
    let (l, m) = masked(&mut s);
    s.state.selected_vertices.clear();
    let before = mask_path(&s, l, m);
    for k in [90, 90, 90, 90] {
        s.execute("path.freeTransform", json!({"layer": l, "scale": k, "merge": "viewer-drag-1"})).unwrap();
    }
    let after = mask_path(&s, l, m);
    assert_ne!(after, before);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(mask_path(&s, l, m), before, "one undo reverts the whole drag");
    s.execute("edit.redo", json!({})).unwrap();
    assert_eq!(mask_path(&s, l, m), after, "redo restores the whole drag");
}

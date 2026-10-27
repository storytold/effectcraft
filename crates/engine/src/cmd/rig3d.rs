//! Camera and light rigs: Layer ▸ Camera ▸ Create Stereo 3D Rig / Create Orbit Null / Create
//! Cameras from 3D Model, Layer ▸ Light ▸ Create Lights from 3D Model / Control Light with
//! Camera / Create Environment Light Background Layer.

use effectcraft_color::Label;
use effectcraft_effects::controls::{STEREO_CONFIGURATIONS, STEREO_CONTROLS, STEREO_CONVERGENCE};
use effectcraft_geom::{Mat4, Vec3, vec3};
use effectcraft_keyframe::Value as KV;
use effectcraft_project::build::{self, Ids};
use effectcraft_project::{AutoOrient, Comp, Expression, ItemId, ItemKind, Layer, LayerId, LayerSource, LightKind, Project};
use effectcraft_render::EvalCtx;
use effectcraft_render::three_d::adv::scene::model_to_layer;
use effectcraft_render::three_d::camera::{self, layer_frame};
use serde_json::{Value, json};

use super::layer::insert_layer;
use super::{CommandSpec, b_p, bad, comp_id, f_p, has_comp, resolve_layer};
use crate::{EngineError, Result, Session, cmd};

fn arr(v: Vec3) -> [f64; 3] {
    [v.x, v.y, v.z]
}

/// Set a property's static value (clears its keyframes and expression).
fn set(l: &mut Layer, path: &str, v: KV) {
    if let Some(pr) = l.props.prop_mut(path) {
        pr.value = v;
        pr.keys.clear();
        pr.expr = None;
    }
}

fn set_expr(l: &mut Layer, path: &str, text: String) {
    if let Some(pr) = l.props.prop_mut(path) {
        pr.expr = Some(Expression { text, enabled: true });
    }
}

/// What evaluation needs from the session (cloned so edits can evaluate their own project).
struct Env {
    time: effectcraft_time::Tick,
    expr: Option<std::sync::Arc<dyn effectcraft_render::ExprHost>>,
    footage: std::sync::Arc<dyn effectcraft_render::FootageSource>,
}

impl Env {
    fn of(s: &Session, cid: ItemId) -> Env {
        Env { time: s.time_of(cid), expr: s.expr.clone(), footage: s.footage.clone() }
    }
    fn ctx<'a>(&'a self, proj: &'a Project, cid: ItemId, comp: &'a Comp) -> EvalCtx<'a> {
        EvalCtx { project: proj, comp_id: cid, comp, time: self.time, expr: self.expr.as_deref(), footage: Some(self.footage.as_ref()) }
    }
}

/// The camera a rig command acts on: `layer`, else a selected camera, else the active camera.
fn camera_p(s: &Session, p: &Value, cid: ItemId, cmd: &str) -> Result<LayerId> {
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    let id = match p.get("layer").or_else(|| p.get("camera")) {
        Some(v) => resolve_layer(comp, v).ok_or_else(|| bad(cmd, "no such layer"))?,
        None => s
            .state
            .selected_layers
            .iter()
            .copied()
            .find(|id| comp.layer(*id).is_some_and(Layer::is_camera))
            .or_else(|| comp.active_camera(s.time_of(cid)).map(|l| l.id))
            .ok_or_else(|| bad(cmd, "the composition has no camera (Layer ▸ New ▸ Camera)"))?,
    };
    if !comp.layer(id).is_some_and(Layer::is_camera) {
        return Err(bad(cmd, "the layer is not a camera"));
    }
    Ok(id)
}

/// Distance from a camera to what it looks at: its point of interest (two-node), else its zoom.
fn focus_distance(ctx: &EvalCtx, cam: &Layer) -> f64 {
    let st = camera::layer_camera(ctx, cam);
    if camera::is_two_node(cam)
        && let Some(tr) = cam.transform()
    {
        let poi = Vec3::from(ctx.v3(cam, tr, "poi", [0.0; 3]));
        let poi = camera::parent_world(ctx, cam).apply(poi);
        return (poi - st.eye).length().max(1.0);
    }
    st.zoom
}

// ---------------------------------------------------------------- stereo 3D rig

/// Stereo rig settings (Stereo 3D Controls).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StereoParams {
    /// 0 Stereo Pair (Left & Right), 1 Center & Right, 2 Center & Left.
    pub configuration: u32,
    /// Interaxial separation, % of the comp width.
    pub scene_depth: f64,
    pub convergence: bool,
    /// 0 the camera's point of interest, 1 its zoom plane.
    pub convergence_of: u32,
    pub z_offset: f64,
}

/// Eye offsets along the camera's x axis, in units of the interaxial separation.
pub fn eye_offsets(configuration: u32) -> [f64; 2] {
    match configuration {
        1 => [0.0, 1.0],
        2 => [-1.0, 0.0],
        _ => [-0.5, 0.5],
    }
}

/// (position, point of interest) of the left (`eye` 0) or right (1) camera, from the master
/// camera's eye, axes, focus distance and zoom.
pub fn eye_camera(sp: &StereoParams, eye: usize, comp_w: f64, master: (Vec3, Vec3, Vec3), focus: f64, zoom: f64) -> (Vec3, Vec3) {
    let (e, fwd, right) = master;
    let d = sp.scene_depth / 100.0 * comp_w;
    let pos = e + right * (eye_offsets(sp.configuration)[eye.min(1)] * d);
    let poi = if sp.convergence {
        let dc = if sp.convergence_of == 1 { zoom } else { focus } + sp.z_offset;
        e + fwd * dc.max(1.0)
    } else {
        pos + fwd * focus
    };
    (pos, poi)
}

/// Expression snippets that rebuild [`eye_camera`] from the master camera and the controls.
fn eye_exprs(src_comp: &str, master: &str, controls: &str, two_node: bool, eye: usize) -> (String, String, String) {
    let q = |s: &str| serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into());
    let k = eye_offsets(0)[eye.min(1)];
    let k1 = eye_offsets(1)[eye.min(1)];
    let k2 = eye_offsets(2)[eye.min(1)];
    let head = format!(
        "var src = comp({c});\nvar m = src.layer({m});\nvar fx = src.layer({x}).effect(\"Stereo 3D Controls\");\n\
         var d = fx(\"Stereo Scene Depth\").value / 100 * src.width;\nvar cfg = fx(\"Configuration\").value;\n\
         var k = cfg == 2 ? {k1} : (cfg == 3 ? {k2} : {k});\n\
         var E = m.toWorld([0, 0, 0]);\nvar F = normalize(m.toWorldVec([0, 0, 1]));\n\
         var eye = add(E, mul(normalize(m.toWorldVec([1, 0, 0])), k * d));\n",
        c = q(src_comp),
        m = q(master),
        x = q(controls),
    );
    let dm = if two_node { "Math.max(length(sub(m.transform.pointOfInterest, m.transform.position)), 1)" } else { "m.cameraOption.zoom" };
    let pos = format!("{head}eye;");
    let poi = format!(
        "{head}var dm = {dm};\nvar dc = (fx(\"Convergence Of\").value == 2 ? m.cameraOption.zoom : dm) + fx(\"Convergence Z Offset\").value;\n\
         fx(\"Enable Convergence\").value ? add(E, mul(F, Math.max(dc, 1))) : add(eye, mul(F, dm));"
    );
    let zoom = format!("comp({}).layer({}).cameraOption.zoom;", q(src_comp), q(master));
    (pos, poi, zoom)
}

fn stereo_p(p: &Value, cmd: &str) -> Result<StereoParams> {
    let pick = |k: &str, opts: &[&str], ids: &[&str]| -> Result<Option<u32>> {
        match p.get(k) {
            None => Ok(None),
            Some(Value::Number(n)) => Ok(n.as_u64().map(|v| (v as u32).min(opts.len() as u32 - 1))),
            Some(Value::String(s)) => {
                let n = s.to_ascii_lowercase().replace([' ', '-', '_', '&', '(', ')'], "");
                let i = ids
                    .iter()
                    .position(|v| v.to_ascii_lowercase() == n)
                    .or_else(|| opts.iter().position(|o| o.to_ascii_lowercase().replace([' ', '-', '_', '&', '(', ')'], "") == n));
                i.map(|i| Some(i as u32)).ok_or_else(|| bad(cmd, format!("{k}: {}", ids.join("|"))))
            }
            _ => Err(bad(cmd, format!("{k}: {}", ids.join("|")))),
        }
    };
    Ok(StereoParams {
        configuration: pick("configuration", &STEREO_CONFIGURATIONS, &["stereoPair", "centerRight", "centerLeft"])?.unwrap_or(0),
        scene_depth: f_p(p, "sceneDepth").unwrap_or(3.0).clamp(0.0, 100.0),
        convergence: b_p(p, "convergence").unwrap_or(false),
        convergence_of: pick("convergenceOf", &STEREO_CONVERGENCE, &["poi", "zoom"])?.unwrap_or(0),
        z_offset: f_p(p, "zOffset").unwrap_or(0.0),
    })
}

fn unique_item_name(proj: &Project, base: &str) -> String {
    let mut name = base.to_string();
    let mut k = 2;
    while proj.items.values().any(|i| i.name == name) {
        name = format!("{base} {k}");
        k += 1;
    }
    name
}

/// An empty comp with the settings of `src`.
fn sibling_comp(src: &Comp) -> Comp {
    let mut c = src.clone();
    c.layers.clear();
    c.markers.clear();
    c
}

fn stereo_rig(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "camera.stereoRig";
    let cid = comp_id(s, p)?;
    let sp = stereo_p(p, cmd)?;
    let view3d = p.get("view3d").and_then(Value::as_u64).unwrap_or(5) as u32;
    let src = s.project.comp(cid).ok_or(EngineError::NoComp)?.clone();
    let src_name = s.project.item(cid).map(|i| i.name.clone()).unwrap_or_default();
    let t = s.time_of(cid);
    let w = src.width as f64;
    let existing = src.active_camera(t).map(|l| l.id);
    let env = Env::of(s, cid);
    let r = s.edit("Create Stereo 3D Rig", None, |proj, st| {
        // The master camera: the comp's active camera, else a new one.
        let master_id = match existing {
            Some(id) => id,
            None => {
                let name = src.unique_layer_name("Master Camera");
                let l = build::layer(proj, &src, &name, LayerSource::Camera, (src.width, src.height), None);
                insert_layer(proj, st, cid, l)?
            }
        };
        let controls_name = proj.comp(cid).map(|c| c.unique_layer_name("Stereo 3D Controls")).unwrap_or_default();
        let src_now = proj.comp(cid).cloned().ok_or(EngineError::NoComp)?;
        let mut ctl = build::layer(proj, &src_now, &controls_name, LayerSource::Null, (src.width, src.height), None);
        ctl.switches.video = false;
        let spec = effectcraft_effects::find(STEREO_CONTROLS).ok_or_else(|| bad(cmd, "Stereo 3D Controls effect missing"))?;
        let mut fx = effectcraft_effects::instantiate(spec, &mut Ids(&mut proj.next_id), spec.name, [100.0, 100.0]);
        for (k, v) in [
            ("configuration", KV::Enum(sp.configuration)),
            ("sceneDepth", KV::Scalar(sp.scene_depth)),
            ("convergence", KV::Bool(sp.convergence)),
            ("convergenceOf", KV::Enum(sp.convergence_of)),
            ("zOffset", KV::Scalar(sp.z_offset)),
        ] {
            if let Some(pr) = fx.get_mut(k) {
                pr.value = v;
            }
        }
        if let Some(g) = ctl.props.sub_mut("effects") {
            g.children.push(fx.into());
        }
        let ctl_id = ctl.id;
        let src_now = proj.comp(cid).cloned().ok_or(EngineError::NoComp)?;
        let at = src_now.layers.iter().position(|l| l.id == master_id).unwrap_or(0);
        proj.comp_mut(cid).ok_or(EngineError::NoComp)?.layers.insert(at, ctl);
        // Master camera frame at the current time.
        let src_now = proj.comp(cid).cloned().ok_or(EngineError::NoComp)?;
        let master = src_now.layer(master_id).cloned().ok_or(EngineError::NoComp)?;
        let ctx = env.ctx(proj, cid, &src_now);
        let (e, fwd, down) = layer_frame(&ctx, &master);
        let right = down.cross(fwd).normalize();
        let focus = focus_distance(&ctx, &master);
        let zoom = camera::layer_camera(&ctx, &master).zoom;
        let two = camera::is_two_node(&master);
        // Left and right eye comps: the source comp collapsed, seen by an eye camera.
        let mut eye_comps = vec![];
        for (eye, label) in [(0usize, "Left"), (1, "Right")] {
            let name = unique_item_name(proj, &format!("{src_name} {label} Eye"));
            let ec = sibling_comp(&src_now);
            let eid = proj.add_item(&name, Label::Sandstone, proj.item(cid).and_then(|i| i.parent), ItemKind::Comp(ec.clone().into()));
            let mut nest = build::layer(proj, &ec, &src_name, LayerSource::Comp { item: cid }, (src.width, src.height), Some(src.duration));
            nest.switches.three_d = true;
            nest.switches.collapse = true;
            let (pos, poi) = eye_camera(&sp, eye, w, (e, fwd, right), focus, zoom);
            let mut cam = build::layer(proj, &ec, &format!("{label} Eye Camera"), LayerSource::Camera, (src.width, src.height), None);
            set(&mut cam, "transform/position", KV::Vec3(arr(pos)));
            set(&mut cam, "transform/poi", KV::Vec3(arr(poi)));
            set(&mut cam, "cameraOptions/zoom", KV::Scalar(zoom));
            set(&mut cam, "cameraOptions/focusDistance", KV::Scalar(focus));
            let (pe, poe, ze) = eye_exprs(&src_name, &master.name, &controls_name, two, eye);
            set_expr(&mut cam, "transform/position", pe);
            set_expr(&mut cam, "transform/poi", poe);
            set_expr(&mut cam, "cameraOptions/zoom", ze);
            cam.auto_orient = AutoOrient::TowardsPointOfInterest;
            let comp = proj.comp_mut(eid).ok_or(EngineError::NoComp)?;
            comp.layers = vec![cam, nest];
            eye_comps.push(eid);
        }
        // The output comp: both eyes through 3D Glasses.
        let out_name = unique_item_name(proj, &format!("{src_name} Stereo 3D"));
        let oc = sibling_comp(&src_now);
        let oid = proj.add_item(&out_name, Label::Sandstone, proj.item(cid).and_then(|i| i.parent), ItemKind::Comp(oc.clone().into()));
        let left_name = proj.item(eye_comps[0]).map(|i| i.name.clone()).unwrap_or_default();
        let right_name = proj.item(eye_comps[1]).map(|i| i.name.clone()).unwrap_or_default();
        let mut left = build::layer(proj, &oc, &left_name, LayerSource::Comp { item: eye_comps[0] }, (src.width, src.height), Some(src.duration));
        let mut right = build::layer(proj, &oc, &right_name, LayerSource::Comp { item: eye_comps[1] }, (src.width, src.height), Some(src.duration));
        right.switches.video = false;
        let glasses = effectcraft_effects::find("ec.perspective.3dglasses").ok_or_else(|| bad(cmd, "3D Glasses effect missing"))?;
        let mut g = effectcraft_effects::instantiate(glasses, &mut Ids(&mut proj.next_id), glasses.name, [w, src.height as f64]);
        for (k, v) in [("leftView", KV::Layer(Some(left.id.0))), ("rightView", KV::Layer(Some(right.id.0))), ("view3d", KV::Enum(view3d.min(8)))] {
            if let Some(pr) = g.get_mut(k) {
                pr.value = v;
            }
        }
        if let Some(fxg) = left.props.sub_mut("effects") {
            fxg.children.push(g.into());
        }
        let (lid, rid) = (left.id, right.id);
        proj.comp_mut(oid).ok_or(EngineError::NoComp)?.layers = vec![left, right];
        st.project_selection = vec![oid];
        Ok(json!({
            "controls": ctl_id.0,
            "master": master_id.0,
            "leftComp": eye_comps[0].0,
            "rightComp": eye_comps[1].0,
            "stereoComp": oid.0,
            "leftLayer": lid.0,
            "rightLayer": rid.0,
        }))
    })?;
    Ok(r)
}

// ---------------------------------------------------------------- orbit null

/// Create Orbit Null: a 3D null at the camera's point of interest becomes the camera's parent,
/// so rotating the null orbits the camera around what it looks at.
fn orbit_null(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "camera.orbitNull";
    let cid = comp_id(s, p)?;
    let cam_id = camera_p(s, p, cid, cmd)?;
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?.clone();
    let cam = comp.layer(cam_id).cloned().ok_or(EngineError::NoComp)?;
    if cam.parent.is_some() {
        return Err(bad(cmd, format!("`{}` already has a parent", cam.name)));
    }
    let env = Env::of(s, cid);
    let ctx = env.ctx(&s.project, cid, &comp);
    let (eye, fwd, _) = layer_frame(&ctx, &cam);
    let target = eye + fwd * focus_distance(&ctx, &cam);
    let name = comp.unique_layer_name(&format!("{} Orbit Null", cam.name));
    let r = s.edit("Create Orbit Null", None, |proj, st| {
        let mut null = build::layer(proj, &comp, &name, LayerSource::Null, (comp.width, comp.height), None);
        null.switches.three_d = true;
        null.switches.video = true;
        set(&mut null, "transform/position", KV::Vec3(arr(target)));
        // World → null space (a translated, unrotated, unscaled null: p − P + anchor).
        let anchor = Vec3::from(null.props.prop("transform/anchor").map(|a| a.value.as_vec3()).unwrap_or([0.0; 3]));
        let to_null = |v: [f64; 3]| arr(Vec3::from(v) - target + anchor);
        let nid = null.id;
        let c = proj.comp_mut(cid).ok_or(EngineError::NoComp)?;
        let at = c.layers.iter().position(|l| l.id == cam_id).unwrap_or(0);
        c.layers.insert(at, null);
        let l = c.layer_mut(cam_id).ok_or(EngineError::NoComp)?;
        l.parent = Some(nid);
        for path in ["transform/position", "transform/poi"] {
            if let Some(pr) = l.props.prop_mut(path) {
                pr.value = KV::Vec3(to_null(pr.value.as_vec3()));
                for k in &mut pr.keys {
                    k.value = KV::Vec3(to_null(k.value.as_vec3()));
                }
            }
        }
        st.selected_layers = vec![nid];
        Ok(json!({"null": nid.0, "camera": cam_id.0, "position": arr(target)}))
    })?;
    Ok(r)
}

// ---------------------------------------------------------------- control light with camera

/// Control Light with Camera: the light rides on the camera (parented at the camera's eye,
/// shining where it looks). Again (or `on: false`) releases it where it is.
fn control_light(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "light.controlWithCamera";
    let cid = comp_id(s, p)?;
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?.clone();
    let lights: Vec<LayerId> = match p.get("layers").or_else(|| p.get("layer")) {
        Some(Value::Array(a)) => a.iter().filter_map(|v| resolve_layer(&comp, v)).collect(),
        Some(v) => resolve_layer(&comp, v).into_iter().collect(),
        None => s.state.selected_layers.iter().copied().filter(|id| comp.layer(*id).is_some_and(Layer::is_light)).collect(),
    };
    if lights.is_empty() || lights.iter().any(|id| !comp.layer(*id).is_some_and(Layer::is_light)) {
        return Err(bad(cmd, "select a light layer"));
    }
    let cam_id = match p.get("camera") {
        Some(v) => resolve_layer(&comp, v).filter(|id| comp.layer(*id).is_some_and(Layer::is_camera)).ok_or_else(|| bad(cmd, "no such camera"))?,
        None => comp.active_camera(s.time_of(cid)).map(|l| l.id).ok_or_else(|| bad(cmd, "the composition has no camera (Layer ▸ New ▸ Camera)"))?,
    };
    let on = b_p(p, "on").unwrap_or_else(|| comp.layer(lights[0]).is_none_or(|l| l.parent != Some(cam_id)));
    let env = Env::of(s, cid);
    let ctx = env.ctx(&s.project, cid, &comp);
    // Released lights keep their current world placement.
    let frames: Vec<(LayerId, Vec3, Vec3)> = lights
        .iter()
        .filter_map(|id| comp.layer(*id))
        .map(|l| {
            let (e, f, _) = layer_frame(&ctx, l);
            (l.id, e, f)
        })
        .collect();
    let (w, h) = (comp.width as f64, comp.height as f64);
    s.edit(if on { "Control Light with Camera" } else { "Release Light from Camera" }, None, |proj, _| {
        let c = proj.comp_mut(cid).ok_or(EngineError::NoComp)?;
        for (id, e, f) in &frames {
            let l = c.layer_mut(*id).ok_or(EngineError::NoComp)?;
            if on {
                l.parent = Some(cam_id);
                set(l, "transform/position", KV::Vec3([0.0; 3]));
                set(l, "transform/poi", KV::Vec3([0.0, 0.0, 1000.0]));
                set(l, "transform/orientation", KV::Vec3([0.0; 3]));
                for k in ["transform/rotationX", "transform/rotationY", "transform/rotation"] {
                    set(l, k, KV::Scalar(0.0));
                }
                l.auto_orient = AutoOrient::Off;
            } else if l.parent == Some(cam_id) {
                l.parent = None;
                set(l, "transform/position", KV::Vec3(arr(*e)));
                set(l, "transform/poi", KV::Vec3(arr(*e + *f * w.max(h))));
                if l.transform().is_some_and(|t| t.get("poi").is_some()) {
                    l.auto_orient = AutoOrient::TowardsPointOfInterest;
                }
            }
        }
        Ok(())
    })?;
    Ok(json!({"controlled": on, "camera": cam_id.0, "lights": lights.iter().map(|l| l.0).collect::<Vec<_>>()}))
}

// ---------------------------------------------------------------- cameras and lights from models

/// Model layers a "from 3D Model" command reads: `layers`/`layer`, else the selected model layers.
fn model_layers(s: &Session, p: &Value, comp: &Comp, cmd: &str) -> Result<Vec<LayerId>> {
    let ids: Vec<LayerId> = match p.get("layers").or_else(|| p.get("layer")) {
        Some(Value::Array(a)) => a.iter().filter_map(|v| resolve_layer(comp, v)).collect(),
        Some(v) => resolve_layer(comp, v).into_iter().collect(),
        None => s.state.selected_layers.iter().copied().filter(|id| comp.layer(*id).is_some_and(|l| matches!(l.source, LayerSource::Model { .. }))).collect(),
    };
    if ids.is_empty() || ids.iter().any(|id| !comp.layer(*id).is_some_and(|l| matches!(l.source, LayerSource::Model { .. }))) {
        return Err(bad(cmd, "select a 3D model layer"));
    }
    Ok(ids)
}

/// A camera or light found in a model: (name, world eye, world forward, the model data).
enum FromModel {
    Camera { name: String, eye: Vec3, fwd: Vec3, zoom: f64 },
    Light { name: String, eye: Vec3, fwd: Vec3, light: effectcraft_model::ModelLight },
}

fn from_model(s: &mut Session, p: &Value, lights: bool) -> Result<Value> {
    let cmd = if lights { "light.fromModel" } else { "camera.fromModel" };
    let cid = comp_id(s, p)?;
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?.clone();
    let ids = model_layers(s, p, &comp, cmd)?;
    let env = Env::of(s, cid);
    let ctx = env.ctx(&s.project, cid, &comp);
    let mut found = vec![];
    for id in &ids {
        let l = comp.layer(*id).ok_or(EngineError::NoComp)?;
        let LayerSource::Model { item } = l.source else { continue };
        let Some(ItemKind::Footage(f)) = s.project.item(item).map(|i| &i.kind) else { continue };
        let Some(m) = s.footage.model(item, f) else { return Err(bad(cmd, format!("can't read the model of `{}`", l.name))) };
        let unit = l.props.sub("geometryOptions").map_or(1.0, |g| ctx.f(l, g, "unitScale", 1.0));
        let to_world = ctx.world_matrix(l) * model_to_layer(unit);
        for pl in m.placed(lights, None, 0.0) {
            let w: Mat4 = to_world * pl.world;
            let eye = w.apply(Vec3::ZERO);
            let fwd = w.apply_vec(vec3(0.0, 0.0, -1.0)).normalize();
            if lights {
                let ml = m.lights[pl.index].clone();
                found.push(FromModel::Light { name: ml.name.clone(), eye, fwd, light: ml });
            } else {
                let mc = &m.cameras[pl.index];
                let zoom = match mc.projection {
                    effectcraft_model::CameraProjection::Perspective { yfov, .. } => comp.height as f64 * 0.5 / (yfov * 0.5).tan(),
                    // Orthographic cameras become perspective ones framing the same height.
                    effectcraft_model::CameraProjection::Orthographic { .. } => effectcraft_geom::default_camera_zoom(comp.width as f64),
                };
                found.push(FromModel::Camera { name: mc.name.clone(), eye, fwd, zoom });
            }
        }
    }
    if found.is_empty() {
        return Err(bad(cmd, if lights { "the model has no lights (KHR_lights_punctual)" } else { "the model has no cameras" }));
    }
    let made = s.edit(if lights { "Create Lights from 3D Model" } else { "Create Cameras from 3D Model" }, None, |proj, st| {
        let mut made = vec![];
        for f in &found {
            let c = proj.comp(cid).cloned().ok_or(EngineError::NoComp)?;
            let l = match f {
                FromModel::Camera { name, eye, fwd, zoom } => {
                    let mut l = build::layer(proj, &c, &c.unique_layer_name(name), LayerSource::Camera, (c.width, c.height), None);
                    set(&mut l, "transform/position", KV::Vec3(arr(*eye)));
                    set(&mut l, "transform/poi", KV::Vec3(arr(*eye + *fwd * *zoom)));
                    set(&mut l, "cameraOptions/zoom", KV::Scalar(*zoom));
                    set(&mut l, "cameraOptions/focusDistance", KV::Scalar(*zoom));
                    l
                }
                FromModel::Light { name, eye, fwd, light } => {
                    let kind = match light.kind {
                        effectcraft_model::ModelLightKind::Directional => LightKind::Parallel,
                        effectcraft_model::ModelLightKind::Point => LightKind::Point,
                        effectcraft_model::ModelLightKind::Spot { .. } => LightKind::Spot,
                    };
                    let mut l = build::layer(proj, &c, &c.unique_layer_name(name), LayerSource::Light { kind }, (c.width, c.height), None);
                    set(&mut l, "transform/position", KV::Vec3(arr(*eye)));
                    set(&mut l, "transform/poi", KV::Vec3(arr(*eye + *fwd * c.width.max(c.height) as f64)));
                    let col = light.color;
                    set(&mut l, "lightOptions/color", KV::Color([col[0], col[1], col[2], 1.0]));
                    // Intensity: 1 unit of the file (lux / candela) = 100%.
                    set(&mut l, "lightOptions/intensity", KV::Scalar((light.intensity * 100.0).clamp(0.0, 1000.0)));
                    if let effectcraft_model::ModelLightKind::Spot { inner, outer } = light.kind {
                        set(&mut l, "lightOptions/coneAngle", KV::Scalar((outer * 2.0).to_degrees().clamp(0.0, 180.0)));
                        set(&mut l, "lightOptions/coneFeather", KV::Scalar(((1.0 - inner / outer.max(1e-9)) * 100.0).clamp(0.0, 100.0)));
                    }
                    if let Some(r) = light.range {
                        set(&mut l, "lightOptions/falloff", KV::Enum(1));
                        set(&mut l, "lightOptions/falloffDistance", KV::Scalar(r * 100.0));
                    }
                    l
                }
            };
            made.push(insert_layer(proj, st, cid, l)?.0);
        }
        Ok(made)
    })?;
    Ok(json!({"layers": made}))
}

// ---------------------------------------------------------------- environment background

/// Create Environment Light Background Layer: a layer showing the environment light's image as
/// the scene's backdrop (an equirectangular sky drawn through the camera).
fn environment_background(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "light.environmentBackground";
    let cid = comp_id(s, p)?;
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?.clone();
    let t = s.time_of(cid);
    let is_env = |l: &Layer| matches!(l.source, LayerSource::Light { kind: LightKind::Environment });
    let light = match p.get("layer") {
        Some(v) => resolve_layer(&comp, v).and_then(|id| comp.layer(id)).filter(|l| is_env(l)).ok_or_else(|| bad(cmd, "not an environment light"))?,
        None => s
            .state
            .selected_layers
            .iter()
            .filter_map(|id| comp.layer(*id))
            .find(|l| is_env(l))
            .or_else(|| comp.layers.iter().find(|l| is_env(l)))
            .ok_or_else(|| bad(cmd, "the composition has no environment light (Layer ▸ New ▸ Light ▸ Environment)"))?,
    };
    let src_layer = light
        .props
        .prop("lightOptions/source")
        .and_then(|pr| pr.value_at(light.layer_time(t)).as_layer())
        .and_then(|id| comp.layer(LayerId(id)))
        .or_else(|| comp.layers.iter().find(|l| l.environment))
        .ok_or_else(|| bad(cmd, "the environment light has no Source (or set an Environment Layer)"))?
        .clone();
    if !matches!(src_layer.source, LayerSource::Footage { .. } | LayerSource::Comp { .. } | LayerSource::Solid { .. }) {
        return Err(bad(cmd, "the environment source must be a footage, comp or solid layer"));
    }
    let light_id = light.id;
    let name = comp.unique_layer_name(&format!("{} Background", src_layer.name));
    let size = effectcraft_render::source_size(&s.project, &src_layer);
    let id = s.edit("Create Environment Light Background Layer", None, |proj, st| {
        let mut l = build::layer(proj, &comp, &name, src_layer.source.clone(), size, None);
        l.switches.three_d = true;
        l.environment_background = true;
        // The backdrop sits at the bottom of the stack.
        let c = proj.comp_mut(cid).ok_or(EngineError::NoComp)?;
        let id = l.id;
        c.layers.push(l);
        st.selected_layers = vec![id];
        Ok(id)
    })?;
    Ok(json!({"layer": id.0, "light": light_id.0, "source": src_layer.id.0}))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "camera.stereoRig",
            "Create Stereo 3D Rig",
            ["Layer", "Camera"],
            None,
            "{comp?, configuration?: stereoPair|centerRight|centerLeft, sceneDepth? (% of comp width, default 3), convergence?: bool, convergenceOf?: poi|zoom, zOffset?, view3d?: 3D Glasses view index (default 5 Balanced Colored Red Blue)}",
            has_comp,
            stereo_rig
        ),
        cmd!("camera.orbitNull", "Create Orbit Null", ["Layer", "Camera"], None, "{comp?, layer?: camera (default selected/active)}", has_comp, orbit_null),
        cmd!(
            "camera.fromModel",
            "Create Cameras from 3D Model",
            ["Layer", "Camera"],
            None,
            "{comp?, layers?: 3D model layers (default selected)}",
            has_comp,
            |s, p| { from_model(s, p, false) }
        ),
        cmd!(
            "light.fromModel",
            "Create Lights from 3D Model",
            ["Layer", "Light"],
            None,
            "{comp?, layers?: 3D model layers (default selected)}",
            has_comp,
            |s, p| { from_model(s, p, true) }
        ),
        cmd!(
            "light.controlWithCamera",
            "Control Light with Camera",
            ["Layer", "Light"],
            None,
            "{comp?, layers?: lights (default selected), camera? (default active), on?: bool (default toggle)}",
            has_comp,
            control_light
        ),
        cmd!(
            "light.environmentBackground",
            "Create Environment Light Background Layer",
            ["Layer", "Light"],
            None,
            "{comp?, layer?: environment light (default selected / first)}",
            has_comp,
            environment_background
        ),
    ]
}

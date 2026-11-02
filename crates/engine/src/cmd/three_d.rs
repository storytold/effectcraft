//! 3D: cameras (Layer ▸ New ▸ Camera, Camera Settings), lights (Layer ▸ New ▸ Light, Light
//! Settings), the viewer's 3D views (View ▸ Switch 3D View, Reset 3D View) and the camera tools
//! (Orbit, Pan, Dolly) acting on the current 3D view.

use effectcraft_geom::{Mat4, Vec3, vec3};
use effectcraft_keyframe::Value as KV;
use effectcraft_project::build;
use effectcraft_project::{AutoOrient, Comp, ItemId, Layer, LayerId, LayerSource, LightKind};
use effectcraft_render::EvalCtx;
use effectcraft_render::three_d::camera::{self, PRESETS, Rig, View3D, ViewCam, Views3D, orientation_for, zoom_for_focal};
use effectcraft_time::Tick;
use serde_json::{Value, json};

use super::layer::{color_p, insert_layer};
use super::model3d::{report_switch, use_advanced_3d};
use super::{CommandSpec, b_p, bad, comp_id, f_p, has_comp, layer_mut, layer_p, merge_p, str_p, time_p};
use crate::{EngineError, Result, Session, cmd, query};

fn v3_p(p: &Value, k: &str) -> Option<[f64; 3]> {
    let a = p.get(k)?.as_array()?;
    let g = |i: usize| a.get(i).and_then(Value::as_f64);
    Some([g(0)?, g(1)?, g(2).unwrap_or(0.0)])
}

/// Set a property at comp time `t` (adds a key when it is animated).
fn set_at(l: &mut Layer, path: &str, t: Tick, v: KV) {
    let lt = l.layer_time(t);
    if let Some(pr) = l.props.prop_mut(path) {
        pr.set_value_at(lt, v);
    }
}

fn get_at(l: &Layer, path: &str, t: Tick) -> Option<KV> {
    l.props.prop(path).map(|pr| pr.value_at(l.layer_time(t)))
}

// ---------------------------------------------------------------- cameras

/// Parse a camera type: `oneNode`/`twoNode` (or "One-Node Camera"…). Some(true) = two-node.
fn two_node_p(p: &Value) -> Option<bool> {
    let t = str_p(p, "type")?.to_ascii_lowercase().replace(['-', ' ', '_'], "");
    if t.starts_with("one") {
        Some(false)
    } else if t.starts_with("two") {
        Some(true)
    } else {
        None
    }
}

/// Zoom from `zoom`, `preset` ("50mm"), `focalLength` (mm) or `angleOfView` (degrees).
fn zoom_p(p: &Value, comp_w: f64) -> Result<Option<f64>> {
    if let Some(z) = f_p(p, "zoom") {
        return Ok(Some(z.max(0.1)));
    }
    if let Some(pr) = p.get("preset") {
        let focal = match pr {
            Value::Number(n) => n.as_f64(),
            Value::String(s) => {
                let k = s.trim().to_ascii_lowercase();
                PRESETS.iter().find(|(n, _)| n.eq_ignore_ascii_case(&k)).map(|(_, f)| *f).or_else(|| k.trim_end_matches("mm").trim().parse().ok())
            }
            _ => None,
        }
        .ok_or_else(|| bad("layer.newCamera", format!("unknown preset {pr}; use one of 15mm, 20mm, 24mm, 28mm, 35mm, 50mm, 80mm, 135mm, 200mm")))?;
        return Ok(Some(zoom_for_focal(comp_w, focal)));
    }
    if let Some(f) = f_p(p, "focalLength") {
        return Ok(Some(zoom_for_focal(comp_w, f)));
    }
    if let Some(a) = f_p(p, "angleOfView") {
        return Ok(Some(comp_w * 0.5 / (a.clamp(0.1, 179.0).to_radians() * 0.5).tan()));
    }
    Ok(None)
}

/// Apply Camera Settings parameters to a camera layer at comp time `t`.
fn apply_camera(l: &mut Layer, comp: &Comp, p: &Value, t: Tick, cmd: &str) -> Result<()> {
    let w = comp.width as f64;
    if let Some(z) = zoom_p(p, w)? {
        set_at(l, "cameraOptions/zoom", t, KV::Scalar(z));
        // "Lock to Zoom" (default): the focus distance follows the zoom unless one is given.
        if b_p(p, "lockToZoom").unwrap_or(true) && f_p(p, "focusDistance").is_none() {
            set_at(l, "cameraOptions/focusDistance", t, KV::Scalar(z));
        }
    }
    if let Some(v) = b_p(p, "dof").or_else(|| b_p(p, "depthOfField")) {
        set_at(l, "cameraOptions/dof", t, KV::Bool(v));
    }
    for (k, path) in [("focusDistance", "cameraOptions/focusDistance"), ("aperture", "cameraOptions/aperture"), ("blurLevel", "cameraOptions/blurLevel")] {
        if let Some(v) = f_p(p, k) {
            set_at(l, path, t, KV::Scalar(v.max(0.0)));
        }
    }
    if let Some(fs) = f_p(p, "fStop") {
        // F-Stop = focal length / aperture, apertures in mm scaled like the presets (25.3 mm at 1920 px).
        let z = get_at(l, "cameraOptions/zoom", t).map(|v| v.as_f64()).unwrap_or(1000.0);
        let focal = camera::focal_for_zoom(w, z);
        set_at(l, "cameraOptions/aperture", t, KV::Scalar(focal / fs.max(0.01) * w / 1920.0));
    }
    if let Some(pos) = v3_p(p, "position") {
        set_at(l, "transform/position", t, KV::Vec3(pos));
    }
    if let Some(poi) = v3_p(p, "poi").or_else(|| v3_p(p, "pointOfInterest")) {
        set_at(l, "transform/poi", t, KV::Vec3(poi));
    }
    if let Some(two) = two_node_p(p) {
        let was = l.auto_orient == AutoOrient::TowardsPointOfInterest;
        if two != was {
            let pos = Vec3::from(get_at(l, "transform/position", t).map(|v| v.as_vec3()).unwrap_or([0.0; 3]));
            let zoom = get_at(l, "cameraOptions/zoom", t).map(|v| v.as_f64()).unwrap_or(1000.0);
            if two {
                // Keep the view: put the POI where the camera looks, clear the orientation.
                let o = get_at(l, "transform/orientation", t).map(|v| v.as_vec3()).unwrap_or([0.0; 3]);
                let fwd = Mat4::orientation(Vec3::from(o)).apply_vec(vec3(0.0, 0.0, 1.0));
                let poi = pos + fwd * zoom;
                set_at(l, "transform/poi", t, KV::Vec3([poi.x, poi.y, poi.z]));
                set_at(l, "transform/orientation", t, KV::Vec3([0.0; 3]));
                l.auto_orient = AutoOrient::TowardsPointOfInterest;
            } else {
                let poi = Vec3::from(get_at(l, "transform/poi", t).map(|v| v.as_vec3()).unwrap_or([0.0; 3]));
                set_at(l, "transform/orientation", t, KV::Vec3(orientation_for(poi - pos)));
                l.auto_orient = AutoOrient::Off;
            }
        }
    } else if p.get("type").is_some() {
        return Err(bad(cmd, "type: oneNode|twoNode"));
    }
    Ok(())
}

fn new_camera(s: &mut Session, p: &Value) -> Result<Value> {
    let cid = comp_id(s, p)?;
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?.clone();
    let name = str_p(p, "name").unwrap_or("Camera 1").to_string();
    let t = time_p(s, p, Some(&comp));
    let id = s.edit("New Camera", None, |proj, st| {
        let mut l = build::layer(proj, &comp, &name, LayerSource::Camera, (comp.width, comp.height), None);
        apply_camera(&mut l, &comp, p, t, "layer.newCamera")?;
        insert_layer(proj, st, cid, l)
    })?;
    Ok(json!({"layer": id.0}))
}

/// The camera or light layer a settings command targets.
fn target(s: &Session, p: &Value, cmd: &str, want_camera: bool) -> Result<(ItemId, LayerId)> {
    let (cid, lid) = layer_p(s, p, cmd)?;
    let l = s.project.comp(cid).and_then(|c| c.layer(lid)).ok_or(EngineError::NoComp)?;
    let ok = if want_camera { l.is_camera() } else { l.is_light() };
    if !ok {
        return Err(bad(cmd, format!("layer `{}` is not a {}", l.name, if want_camera { "camera" } else { "light" })));
    }
    Ok((cid, lid))
}

fn camera_settings(s: &mut Session, p: &Value) -> Result<Value> {
    let (cid, lid) = target(s, p, "layer.cameraSettings", true)?;
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?.clone();
    let t = time_p(s, p, Some(&comp));
    let name = str_p(p, "name").map(str::to_string);
    s.edit("Camera Settings", None, |proj, _| {
        let l = layer_mut(proj, cid, lid)?;
        if let Some(n) = &name {
            l.name = n.clone();
        }
        apply_camera(l, &comp, p, t, "layer.cameraSettings")
    })?;
    Ok(Value::Null)
}

// ---------------------------------------------------------------- lights

fn light_kind_p(p: &Value, k: &str) -> Option<LightKind> {
    let v = str_p(p, k)?;
    LightKind::ALL.into_iter().find(|l| l.label().eq_ignore_ascii_case(v.trim()))
}

fn apply_light(l: &mut Layer, p: &Value, t: Tick, cmd: &str) -> Result<()> {
    if let Some(c) = color_p(p, "color") {
        set_at(l, "lightOptions/color", t, KV::Color([c[0] as f64, c[1] as f64, c[2] as f64, 1.0]));
    }
    for k in ["intensity", "coneAngle", "coneFeather", "radius", "falloffDistance", "shadowDarkness", "shadowDiffusion"] {
        if let Some(v) = f_p(p, k) {
            set_at(l, &format!("lightOptions/{k}"), t, KV::Scalar(v));
        }
    }
    if let Some(v) = b_p(p, "castsShadows") {
        set_at(l, "lightOptions/castsShadows", t, KV::Bool(v));
    }
    if let Some(f) = p.get("falloff") {
        let i = match f {
            Value::Number(n) => n.as_u64().map(|v| v as u32),
            Value::String(s) => {
                let k = s.to_ascii_lowercase();
                if k.starts_with("none") {
                    Some(0)
                } else if k.starts_with("smooth") {
                    Some(1)
                } else if k.starts_with("inverse") {
                    Some(2)
                } else {
                    None
                }
            }
            _ => None,
        }
        .filter(|i| *i <= 2)
        .ok_or_else(|| bad(cmd, "falloff: None|Smooth|Inverse Square Clamped"))?;
        set_at(l, "lightOptions/falloff", t, KV::Enum(i));
    }
    if let Some(pos) = v3_p(p, "position") {
        set_at(l, "transform/position", t, KV::Vec3(pos));
    }
    if let Some(poi) = v3_p(p, "poi").or_else(|| v3_p(p, "pointOfInterest")) {
        set_at(l, "transform/poi", t, KV::Vec3(poi));
    }
    Ok(())
}

fn new_light(s: &mut Session, p: &Value) -> Result<Value> {
    let cid = comp_id(s, p)?;
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?.clone();
    let kind = match p.get("kind").or_else(|| p.get("type")) {
        Some(_) => {
            light_kind_p(p, "kind").or_else(|| light_kind_p(p, "type")).ok_or_else(|| bad("layer.newLight", "kind: Parallel|Spot|Point|Ambient|Environment"))?
        }
        None => LightKind::Spot,
    };
    let name = str_p(p, "name").unwrap_or("Light 1").to_string();
    let t = time_p(s, p, Some(&comp));
    // Only the Advanced 3D renderer lights with Environment lights, so adding one switches a
    // Classic 3D comp to it, like After Effects (inside the adding edit: one undo step).
    let env = kind == LightKind::Environment;
    let (id, switched) = s.edit("New Light", None, |proj, st| {
        let mut l = build::layer(proj, &comp, &name, LayerSource::Light { kind }, (comp.width, comp.height), None);
        apply_light(&mut l, p, t, "layer.newLight")?;
        let id = insert_layer(proj, st, cid, l)?;
        Ok((id, if env { use_advanced_3d(proj, cid)? } else { false }))
    })?;
    report_switch(s, cid, switched, "which Environment lights need to light the scene");
    Ok(json!({"layer": id.0, "advanced3d": env}))
}

fn light_settings(s: &mut Session, p: &Value) -> Result<Value> {
    let (cid, lid) = target(s, p, "layer.lightSettings", false)?;
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?.clone();
    let t = time_p(s, p, Some(&comp));
    let name = str_p(p, "name").map(str::to_string);
    let kind = match p.get("kind").or_else(|| p.get("type")) {
        Some(_) => Some(
            light_kind_p(p, "kind")
                .or_else(|| light_kind_p(p, "type"))
                .ok_or_else(|| bad("layer.lightSettings", "kind: Parallel|Spot|Point|Ambient|Environment"))?,
        ),
        None => None,
    };
    let switched = s.edit("Light Settings", None, |proj, _| {
        let old = proj.comp(cid).and_then(|c| c.layer(lid)).cloned().ok_or(EngineError::NoComp)?;
        let mut l = old.clone();
        if let Some(k) = kind
            && old.source != (LayerSource::Light { kind: k })
        {
            // Light Type change: rebuild the light's groups, carrying over shared values.
            let fresh = build::layer(proj, &comp, &old.name, LayerSource::Light { kind: k }, (comp.width, comp.height), None);
            l.source = fresh.source.clone();
            l.auto_orient = fresh.auto_orient;
            l.props = fresh.props;
            for path in [
                "transform/position",
                "transform/poi",
                "transform/orientation",
                "transform/rotationX",
                "transform/rotationY",
                "transform/rotation",
                "lightOptions/intensity",
                "lightOptions/color",
                "lightOptions/coneAngle",
                "lightOptions/coneFeather",
                "lightOptions/falloff",
                "lightOptions/radius",
                "lightOptions/falloffDistance",
                "lightOptions/castsShadows",
                "lightOptions/shadowDarkness",
                "lightOptions/shadowDiffusion",
            ] {
                if let (Some(src), Some(dst)) = (old.props.prop(path), l.props.prop_mut(path)) {
                    dst.value = src.value.clone();
                    dst.keys = src.keys.clone();
                    dst.expr = src.expr.clone();
                }
            }
        }
        if let Some(n) = &name {
            l.name = n.clone();
        }
        apply_light(&mut l, p, t, "layer.lightSettings")?;
        // A light that becomes an Environment light switches a Classic 3D comp to Advanced 3D,
        // like adding one does; editing one that already is leaves the renderer to the user.
        let was_env = matches!(old.source, LayerSource::Light { kind: LightKind::Environment });
        let is_env = matches!(l.source, LayerSource::Light { kind: LightKind::Environment });
        *layer_mut(proj, cid, lid)? = l;
        Ok(if !was_env && is_env { use_advanced_3d(proj, cid)? } else { false })
    })?;
    report_switch(s, cid, switched, "which Environment lights need to light the scene");
    Ok(Value::Null)
}

// ---------------------------------------------------------------- 3D views

fn views(s: &mut Session, cid: ItemId) -> &mut Views3D {
    s.state.views3d.entry(cid).or_default()
}

fn view_p(p: &Value, cmd: &str) -> Result<View3D> {
    let v = str_p(p, "view").ok_or_else(|| bad(cmd, "missing `view` (activeCamera|default|front|left|top|back|right|bottom|custom1|custom2|custom3)"))?;
    View3D::from_id(v).ok_or_else(|| bad(cmd, format!("unknown view `{v}`")))
}

fn switch_view(s: &mut Session, cid: ItemId, v: View3D) -> Value {
    let vs = views(s, cid);
    if vs.current != v {
        vs.last = vs.current;
        vs.current = v;
    }
    json!({"view": v.id()})
}

fn set_3d_view(s: &mut Session, p: &Value) -> Result<Value> {
    let cid = comp_id(s, p)?;
    let v = view_p(p, "view.set3DView")?;
    Ok(switch_view(s, cid, v))
}

/// The view of one pane of the View Layout: pane 0 is the main view (the same as Switch 3D View),
/// panes 1 to 3 are the extra views of a 2- or 4-view layout. UI state, not part of the project.
fn set_pane_view(s: &mut Session, p: &Value) -> Result<Value> {
    let cid = comp_id(s, p)?;
    let v = view_p(p, "view.setPaneView")?;
    let pane = p.get("pane").and_then(Value::as_u64).filter(|n| *n <= 3).ok_or_else(|| bad("view.setPaneView", "pane: 0 (main view) to 3"))? as usize;
    if pane == 0 {
        switch_view(s, cid, v);
    } else {
        views(s, cid).set_pane(pane - 1, v);
    }
    let vs = views(s, cid);
    Ok(json!({"pane": pane, "view": v.id(), "views": [vs.current.id(), vs.pane(0).id(), vs.pane(1).id(), vs.pane(2).id()]}))
}

macro_rules! view_cmd {
    ($f:ident, $v:expr) => {
        fn $f(s: &mut Session, p: &Value) -> Result<Value> {
            let cid = comp_id(s, p)?;
            Ok(switch_view(s, cid, $v))
        }
    };
}
view_cmd!(view_active, View3D::ActiveCamera);
view_cmd!(view_front, View3D::Front);
view_cmd!(view_left, View3D::Left);
view_cmd!(view_top, View3D::Top);
view_cmd!(view_back, View3D::Back);
view_cmd!(view_right, View3D::Right);
view_cmd!(view_bottom, View3D::Bottom);
view_cmd!(view_custom1, View3D::Custom1);
view_cmd!(view_custom2, View3D::Custom2);
view_cmd!(view_custom3, View3D::Custom3);
view_cmd!(view_default, View3D::Default);

fn last_view(s: &mut Session, p: &Value) -> Result<Value> {
    let cid = comp_id(s, p)?;
    let last = views(s, cid).last;
    Ok(switch_view(s, cid, last))
}

fn reset_view(s: &mut Session, p: &Value) -> Result<Value> {
    let cid = comp_id(s, p)?;
    let cur = views(s, cid).current;
    if cur != View3D::ActiveCamera {
        views(s, cid).cams.remove(&cur);
        return Ok(json!({"view": cur.id()}));
    }
    // Active camera: back to the default framing (50 mm-style distance, looking at the centre).
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?.clone();
    let t = s.state.times.get(&cid).copied().unwrap_or(Tick::ZERO);
    let cam = comp.active_camera(t).map(|l| l.id).ok_or_else(|| bad("view.reset3DView", "the comp has no camera layer"))?;
    let (w, h) = (comp.width as f64, comp.height as f64);
    s.edit("Reset 3D View", None, |proj, _| {
        let l = layer_mut(proj, cid, cam)?;
        let zoom = get_at(l, "cameraOptions/zoom", t).map(|v| v.as_f64()).unwrap_or(effectcraft_geom::default_camera_zoom(w));
        set_at(l, "transform/position", t, KV::Vec3([w / 2.0, h / 2.0, -zoom]));
        set_at(l, "transform/poi", t, KV::Vec3([w / 2.0, h / 2.0, 0.0]));
        set_at(l, "transform/orientation", t, KV::Vec3([0.0; 3]));
        Ok(())
    })?;
    Ok(json!({"view": "activeCamera"}))
}

fn set_view_camera(s: &mut Session, p: &Value) -> Result<Value> {
    let cid = comp_id(s, p)?;
    let v = view_p(p, "view.set3DViewCamera")?;
    if v == View3D::ActiveCamera {
        return Err(bad("view.set3DViewCamera", "edit the camera layer for the active camera view"));
    }
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    let (w, h) = (comp.width as f64, comp.height as f64);
    let vs = views(s, cid);
    let mut c = vs.cam(v, w, h);
    if let Some(e) = v3_p(p, "eye") {
        c.eye = e;
    }
    if let Some(e) = v3_p(p, "poi") {
        c.poi = e;
    }
    if let Some(z) = f_p(p, "zoom") {
        c.zoom = z.max(1e-3);
    }
    vs.cams.insert(v, c);
    Ok(json!({"view": v.id()}))
}

fn get_3d(s: &mut Session, p: &Value) -> Result<Value> {
    let cid = comp_id(s, p)?;
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    let (w, h) = (comp.width as f64, comp.height as f64);
    let vs = s.state.views3d.get(&cid).cloned().unwrap_or_default();
    let t = s.state.times.get(&cid).copied().unwrap_or(Tick::ZERO);
    let ctx =
        EvalCtx { project: &s.project, comp_id: cid, comp, time: t, expr: s.expr.as_deref(), footage: None, property_overrides: &[], media_overrides: &[] };
    let cam = vs.override_camera(w, h).unwrap_or_else(|| camera::active_camera(&ctx));
    let f = cam.forward();
    let lights: Vec<Value> = comp
        .layers
        .iter()
        .filter_map(|l| match l.source {
            LayerSource::Light { kind } => Some(json!({"id": l.id.0, "name": l.name, "kind": kind.label()})),
            _ => None,
        })
        .collect();
    let active = comp.active_camera(t).map(|l| json!({"id": l.id.0, "name": l.name, "twoNode": camera::is_two_node(l)}));
    Ok(json!({
        "view": vs.current.id(),
        "label": vs.current.label(),
        "views": View3D::ALL.iter().map(|v| v.id()).collect::<Vec<_>>(),
        "camera": {"eye": [cam.eye.x, cam.eye.y, cam.eye.z], "forward": [f.x, f.y, f.z], "zoom": cam.zoom, "ortho": cam.ortho},
        "activeCameraLayer": active,
        "lights": lights,
    }))
}

// ---------------------------------------------------------------- camera tools

enum ToolOp {
    Orbit(f64, f64),
    Pan(f64, f64),
    Dolly(f64),
}

fn camera_tool(s: &mut Session, p: &Value, op: ToolOp, cmd: &str, label: &str) -> Result<Value> {
    let cid = comp_id(s, p)?;
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?.clone();
    let (w, h) = (comp.width as f64, comp.height as f64);
    let cur = s.state.views3d.get(&cid).map(|v| v.current).unwrap_or_default();
    let apply = |r: &mut Rig, layer: bool| match op {
        ToolOp::Orbit(yaw, pitch) => r.orbit(yaw, pitch),
        ToolOp::Pan(dx, dy) => r.pan(dx, dy),
        ToolOp::Dolly(a) => r.dolly(a, layer),
    };
    if cur != View3D::ActiveCamera {
        let vs = views(s, cid);
        let vc = vs.cam(cur, w, h);
        let mut r = Rig::from_view(&vc);
        apply(&mut r, false);
        let nc = ViewCam { eye: camera::arr(r.eye), poi: camera::arr(r.poi), zoom: r.zoom, ..vc };
        vs.cams.insert(cur, nc);
        return Ok(json!({"view": cur.id(), "eye": nc.eye, "poi": nc.poi, "zoom": nc.zoom}));
    }
    let t = s.state.times.get(&cid).copied().unwrap_or(Tick::ZERO);
    let cam = match p.get("layer") {
        Some(v) => super::resolve_layer(&comp, v).filter(|id| comp.layer(*id).is_some_and(|l| l.is_camera())),
        None => comp.active_camera(t).map(|l| l.id),
    }
    .ok_or_else(|| bad(cmd, "the comp has no active camera layer (Layer ▸ New ▸ Camera), or switch to a custom 3D view"))?;
    let merge = merge_p(p).map(str::to_string);
    let r = s.edit(label, merge.as_deref(), |proj, _| {
        let l = layer_mut(proj, cid, cam)?;
        let pos = Vec3::from(get_at(l, "transform/position", t).map(|v| v.as_vec3()).unwrap_or([w / 2.0, h / 2.0, -1000.0]));
        let zoom = get_at(l, "cameraOptions/zoom", t).map(|v| v.as_f64()).unwrap_or(1000.0);
        let two = camera::is_two_node(l);
        let o = get_at(l, "transform/orientation", t).map(|v| v.as_vec3()).unwrap_or([0.0; 3]);
        let rx = get_at(l, "transform/rotationX", t).map(|v| v.as_f64()).unwrap_or(0.0);
        let ry = get_at(l, "transform/rotationY", t).map(|v| v.as_f64()).unwrap_or(0.0);
        let poi = if two {
            Vec3::from(get_at(l, "transform/poi", t).map(|v| v.as_vec3()).unwrap_or([w / 2.0, h / 2.0, 0.0]))
        } else {
            let fwd = (Mat4::orientation(Vec3::from(o)) * Mat4::rotate_y(ry) * Mat4::rotate_x(rx)).apply_vec(vec3(0.0, 0.0, 1.0));
            pos + fwd * zoom
        };
        let mut rig = Rig { eye: pos, poi, zoom, ortho: false };
        apply(&mut rig, true);
        set_at(l, "transform/position", t, KV::Vec3(camera::arr(rig.eye)));
        if two {
            set_at(l, "transform/poi", t, KV::Vec3(camera::arr(rig.poi)));
        } else {
            set_at(l, "transform/orientation", t, KV::Vec3(orientation_for(rig.poi - rig.eye)));
            set_at(l, "transform/rotationX", t, KV::Scalar(0.0));
            set_at(l, "transform/rotationY", t, KV::Scalar(0.0));
        }
        Ok(json!({"layer": cam.0, "position": camera::arr(rig.eye), "poi": camera::arr(rig.poi)}))
    })?;
    Ok(r)
}

fn orbit(s: &mut Session, p: &Value) -> Result<Value> {
    camera_tool(
        s,
        p,
        ToolOp::Orbit(f_p(p, "yaw").or_else(|| f_p(p, "dx")).unwrap_or(0.0), f_p(p, "pitch").or_else(|| f_p(p, "dy")).unwrap_or(0.0)),
        "camera.orbit",
        "Orbit Camera",
    )
}
fn pan(s: &mut Session, p: &Value) -> Result<Value> {
    camera_tool(s, p, ToolOp::Pan(f_p(p, "dx").unwrap_or(0.0), f_p(p, "dy").unwrap_or(0.0)), "camera.pan", "Pan Camera")
}
fn dolly(s: &mut Session, p: &Value) -> Result<Value> {
    camera_tool(s, p, ToolOp::Dolly(f_p(p, "amount").or_else(|| f_p(p, "dz")).unwrap_or(0.0)), "camera.dolly", "Dolly Camera")
}

/// Layer ▸ Transform ▸ Auto-Orient: off | alongPath | towardsCamera (3D layers) |
/// towardsPointOfInterest (cameras and spot/parallel lights).
fn auto_orient(s: &mut Session, p: &Value) -> Result<Value> {
    let (cid, ids) = super::layers_p(s, p)?;
    if ids.is_empty() {
        return Err(bad("layer.autoOrient", "select a layer first"));
    }
    let raw = str_p(p, "mode").ok_or_else(|| bad("layer.autoOrient", "missing `mode` (off|alongPath|towardsCamera|towardsPointOfInterest)"))?;
    let k = raw.to_ascii_lowercase().replace([' ', '-', '_'], "");
    let mode = match k.as_str() {
        "off" => AutoOrient::Off,
        "alongpath" | "orientalongpath" => AutoOrient::AlongPath,
        "towardscamera" | "orienttowardscamera" => AutoOrient::TowardsCamera,
        "towardspointofinterest" | "towardspoi" | "orienttowardspointofinterest" => AutoOrient::TowardsPointOfInterest,
        _ => return Err(bad("layer.autoOrient", format!("unknown mode `{raw}`"))),
    };
    s.edit("Auto-Orient", None, |proj, _| {
        let comp = proj.comp_mut(cid).ok_or(EngineError::NoComp)?;
        for l in comp.layers.iter_mut_where(|l| ids.contains(&l.id)) {
            let rig = l.is_camera() || l.is_light();
            let ok = match mode {
                AutoOrient::Off | AutoOrient::AlongPath => true,
                AutoOrient::TowardsCamera => !rig,
                AutoOrient::TowardsPointOfInterest => rig && l.transform().is_some_and(|t| t.get("poi").is_some()),
            };
            if !ok {
                return Err(bad("layer.autoOrient", format!("`{raw}` is not available for layer `{}`", l.name)));
            }
            l.auto_orient = mode;
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

// ---------------------------------------------------------------- look at / camera from view

/// World-space points spanning the layers' content at comp time `t` (content-box corners
/// through each layer's world matrix; cameras and lights contribute their position).
fn layer_points(s: &Session, cid: ItemId, ids: &[LayerId], t: Tick) -> Vec<Vec3> {
    let Some(comp) = s.project.comp(cid) else { return vec![] };
    let ctx =
        EvalCtx { project: &s.project, comp_id: cid, comp, time: t, expr: s.expr.as_deref(), footage: None, property_overrides: &[], media_overrides: &[] };
    let mut out = vec![];
    for l in comp.layers.iter().filter(|l| ids.contains(&l.id)) {
        let m = ctx.world_matrix(l);
        match effectcraft_render::content_bounds(&ctx, l) {
            Some([x0, y0, x1, y1]) => {
                for (x, y) in [(x0, y0), (x1, y0), (x1, y1), (x0, y1)] {
                    out.push(m.apply(vec3(x, y, 0.0)));
                }
            }
            None => out.push(m.apply(Vec3::ZERO)),
        }
    }
    out
}

fn look_at(s: &mut Session, p: &Value, all: bool) -> Result<Value> {
    let cmd = if all { "view.lookAtAll" } else { "view.lookAtSelected" };
    let cid = comp_id(s, p)?;
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?.clone();
    let t = s.state.times.get(&cid).copied().unwrap_or(Tick::ZERO);
    let ids: Vec<LayerId> = if all {
        comp.layers.iter().filter(|l| l.is_active_at(t) && !l.is_camera() && !l.is_light()).map(|l| l.id).collect()
    } else {
        let (_, sel) = super::layers_p(s, p)?;
        sel
    };
    if ids.is_empty() {
        return Err(bad(cmd, if all { "the composition has no visible layers" } else { "select layers first" }));
    }
    let pts = layer_points(s, cid, &ids, t);
    let (mut lo, mut hi) = (vec3(f64::MAX, f64::MAX, f64::MAX), vec3(f64::MIN, f64::MIN, f64::MIN));
    for q in &pts {
        lo = vec3(lo.x.min(q.x), lo.y.min(q.y), lo.z.min(q.z));
        hi = vec3(hi.x.max(q.x), hi.y.max(q.y), hi.z.max(q.z));
    }
    let c = (lo + hi) * 0.5;
    let r = ((hi - lo).length() * 0.5).max(1.0);
    let (w, h) = (comp.width as f64, comp.height as f64);
    let fit = w.min(h);
    let cur = s.state.views3d.get(&cid).map(|v| v.current).unwrap_or_default();
    if !comp.has_3d() {
        // 2D: frame the layers in the viewer (zoom and pan to their comp-space bounds).
        let rect = [lo.x, lo.y, hi.x, hi.y];
        super::frontend(s, "view.lookAt", &json!({"rect": rect}))?;
        return Ok(json!({"rect": rect}));
    }
    if cur != View3D::ActiveCamera {
        let vs = views(s, cid);
        let mut vc = vs.cam(cur, w, h);
        let fwd = (Vec3::from(vc.poi) - Vec3::from(vc.eye)).normalize();
        if vc.ortho {
            vc.zoom = fit / (2.2 * r);
            vc.eye = camera::arr(c - fwd * 10_000.0);
        } else {
            vc.eye = camera::arr(c - fwd * (vc.zoom * r * 2.2 / fit));
        }
        vc.poi = camera::arr(c);
        vs.cams.insert(cur, vc);
        return Ok(json!({"view": cur.id(), "eye": vc.eye, "poi": vc.poi, "zoom": vc.zoom}));
    }
    let cam = comp.active_camera(t).map(|l| l.id).ok_or_else(|| bad(cmd, "the comp has no camera layer: switch to a 3D view (View ▸ Switch 3D View)"))?;
    s.edit(if all { "Look at All Layers" } else { "Look at Selected Layers" }, None, |proj, _| {
        let l = layer_mut(proj, cid, cam)?;
        let lt = l.layer_time(t);
        let pos = Vec3::from(get_at(l, "transform/position", lt).map(|v| v.as_vec3()).unwrap_or([w / 2.0, h / 2.0, -1000.0]));
        let zoom = get_at(l, "cameraOptions/zoom", lt).map(|v| v.as_f64()).unwrap_or(1000.0);
        let two = camera::is_two_node(l);
        let fwd = if two {
            let poi = Vec3::from(get_at(l, "transform/poi", lt).map(|v| v.as_vec3()).unwrap_or([w / 2.0, h / 2.0, 0.0]));
            (poi - pos).normalize()
        } else {
            let o = get_at(l, "transform/orientation", lt).map(|v| v.as_vec3()).unwrap_or([0.0; 3]);
            Mat4::orientation(Vec3::from(o)).apply_vec(vec3(0.0, 0.0, 1.0)).normalize()
        };
        let eye = c - fwd * (zoom * r * 2.2 / fit);
        set_at(l, "transform/position", lt, KV::Vec3(camera::arr(eye)));
        if two {
            set_at(l, "transform/poi", lt, KV::Vec3(camera::arr(c)));
        }
        Ok(json!({"layer": cam.0, "position": camera::arr(eye), "poi": camera::arr(c)}))
    })
}

/// Layer ▸ Camera ▸ Create Camera from 3D View: a two-node camera matching the current 3D view.
fn camera_from_view(s: &mut Session, p: &Value) -> Result<Value> {
    let cid = comp_id(s, p)?;
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?.clone();
    let (w, h) = (comp.width as f64, comp.height as f64);
    let cur = s.state.views3d.get(&cid).map(|v| v.current).unwrap_or_default();
    if cur == View3D::ActiveCamera {
        return Err(bad("camera.fromView", "switch to a 3D view first (View ▸ Switch 3D View)"));
    }
    let vc = s.state.views3d.get(&cid).cloned().unwrap_or_default().cam(cur, w, h);
    let poi = Vec3::from(vc.poi);
    let fwd = (poi - Vec3::from(vc.eye)).normalize();
    let zoom = if vc.ortho { effectcraft_geom::default_camera_zoom(w) } else { vc.zoom };
    let eye = if vc.ortho { poi - fwd * zoom } else { Vec3::from(vc.eye) };
    let t = s.state.times.get(&cid).copied().unwrap_or(Tick::ZERO);
    let name = comp.unique_layer_name("Camera 1");
    let params = json!({"position": camera::arr(eye), "poi": camera::arr(poi), "zoom": zoom});
    let id = s.edit("Create Camera from 3D View", None, |proj, st| {
        let mut l = build::layer(proj, &comp, &name, LayerSource::Camera, (comp.width, comp.height), None);
        apply_camera(&mut l, &comp, &params, t, "camera.fromView")?;
        insert_layer(proj, st, cid, l)
    })?;
    switch_view(s, cid, View3D::ActiveCamera);
    Ok(json!({"layer": id.0, "from": cur.id()}))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "layer.autoOrient",
            "Auto-Orient...",
            ["Layer", "Transform"],
            Some("Cmd+Alt+O"),
            "{mode: off|alongPath|towardsCamera|towardsPointOfInterest, layers?}",
            super::has_layers,
            auto_orient
        ),
        cmd!(
            "layer.newLight",
            "Light...",
            ["Layer", "New"],
            Some("Cmd+Alt+Shift+L"),
            "{kind?: Parallel|Spot|Point|Ambient|Environment (default Spot; Environment lights and reflects from an equirectangular image layer: prop.set its lightOptions/source to that layer's id, or leave it empty for the comp's Environment Layer or a flat default environment of its colour and intensity, see layer.environment; an Environment light switches a Classic 3D comp to Advanced 3D), name?, color?, intensity?, coneAngle?, coneFeather?, falloff?: None|Smooth|Inverse Square Clamped, radius?, falloffDistance?, castsShadows?, shadowDarkness?, shadowDiffusion?, position? [x,y,z], poi? [x,y,z]}",
            has_comp,
            new_light
        ),
        cmd!(
            "layer.newCamera",
            "Camera...",
            ["Layer", "New"],
            Some("Cmd+Alt+Shift+C"),
            "{name?, type?: oneNode|twoNode (default twoNode), preset?: 15mm|20mm|24mm|28mm|35mm|50mm|80mm|135mm|200mm, zoom?, focalLength? mm, angleOfView? deg, dof?, focusDistance?, aperture?, fStop?, blurLevel?, lockToZoom?, position? [x,y,z], poi? [x,y,z]}",
            has_comp,
            new_camera
        ),
        cmd!(
            "layer.cameraSettings",
            "Camera Settings...",
            ["Layer", "Camera"],
            None,
            "{layer?, name?, type?, preset?, zoom?, focalLength?, angleOfView?, dof?, focusDistance?, aperture?, fStop?, blurLevel?, position?, poi?}",
            has_comp,
            camera_settings
        ),
        cmd!(
            "layer.lightSettings",
            "Light Settings...",
            [],
            None,
            "{layer?, name?, kind?: Parallel|Spot|Point|Ambient|Environment (changing to Environment switches a Classic 3D comp to Advanced 3D), color?, intensity?, coneAngle?, coneFeather?, falloff?, radius?, falloffDistance?, castsShadows?, shadowDarkness?, shadowDiffusion?, position?, poi?}",
            has_comp,
            light_settings
        ),
        cmd!(
            "view.set3DView",
            "Switch 3D View",
            [],
            None,
            "{view: activeCamera|default|front|left|top|back|right|bottom|custom1|custom2|custom3, comp?}",
            has_comp,
            set_3d_view
        ),
        cmd!(
            "view.setPaneView",
            "Set View of Pane",
            [],
            None,
            "{pane: 0 (main) | 1 | 2 | 3, view: activeCamera|default|front|left|top|back|right|bottom|custom1|custom2|custom3, comp?}",
            has_comp,
            set_pane_view
        ),
        cmd!("view.3d.activeCamera", "Active Camera", ["View", "Switch 3D View"], Some("F12"), "{comp?}", has_comp, view_active),
        cmd!("view.3d.default", "Default", ["View", "Switch 3D View"], None, "{comp?}", has_comp, view_default),
        cmd!("view.3d.front", "Front", ["View", "Switch 3D View"], Some("F10"), "{comp?}", has_comp, view_front),
        cmd!("view.3d.left", "Left", ["View", "Switch 3D View"], None, "{comp?}", has_comp, view_left),
        cmd!("view.3d.top", "Top", ["View", "Switch 3D View"], None, "{comp?}", has_comp, view_top),
        cmd!("view.3d.back", "Back", ["View", "Switch 3D View"], None, "{comp?}", has_comp, view_back),
        cmd!("view.3d.right", "Right", ["View", "Switch 3D View"], None, "{comp?}", has_comp, view_right),
        cmd!("view.3d.bottom", "Bottom", ["View", "Switch 3D View"], None, "{comp?}", has_comp, view_bottom),
        cmd!("view.3d.custom1", "Custom View 1", ["View", "Switch 3D View"], Some("F11"), "{comp?}", has_comp, view_custom1),
        cmd!("view.3d.custom2", "Custom View 2", ["View", "Switch 3D View"], None, "{comp?}", has_comp, view_custom2),
        cmd!("view.3d.custom3", "Custom View 3", ["View", "Switch 3D View"], None, "{comp?}", has_comp, view_custom3),
        cmd!("view.3d.last", "Switch to Last 3D View", ["View"], None, "{comp?}", has_comp, last_view),
        cmd!("view.lookAtSelected", "Look at Selected Layers", ["View"], Some("Cmd+Alt+Shift+\\"), "{comp?, layers?}", super::has_layers, |s, p| look_at(
            s, p, false
        )),
        cmd!("view.lookAtAll", "Look at All Layers", ["View"], None, "{comp?}", has_comp, |s, p| look_at(s, p, true)),
        cmd!("camera.fromView", "Create Camera from 3D View", ["Layer", "Camera"], None, "{comp?}", has_comp, camera_from_view),
        cmd!("view.reset3DView", "Reset 3D View", ["View"], None, "{comp?}", has_comp, reset_view),
        cmd!("view.set3DViewCamera", "Set 3D View Camera", [], None, "{view, eye? [x,y,z], poi? [x,y,z], zoom?}", has_comp, set_view_camera),
        cmd!(
            "camera.orbit",
            "Orbit Camera",
            [],
            None,
            "{yaw|dx deg, pitch|dy deg, layer?, merge?} — current 3D view (camera layer in Active Camera)",
            has_comp,
            orbit
        ),
        cmd!("camera.pan", "Pan Camera", [], None, "{dx, dy (comp px), layer?, merge?}", has_comp, pan),
        cmd!("camera.dolly", "Dolly Camera", [], None, "{amount (px, + = forward), layer?, merge?}", has_comp, dolly),
        query!("view.get3D", "3D View State", "{comp?} → current 3D view, view camera, active camera layer, lights", get_3d),
    ]
}

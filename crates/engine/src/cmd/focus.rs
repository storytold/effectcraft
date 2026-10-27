//! Layer ▸ Camera focus commands: Link Focus Distance to Point of Interest, Link Focus Distance to
//! Layer (both add an expression to Focus Distance, as After Effects does) and Set Focus Distance
//! to Layer (a one-off value).

use effectcraft_geom::vec3;
use effectcraft_keyframe::Value as KV;
use effectcraft_project::{Comp, Expression, ItemId, LayerId};
use serde_json::{Value, json};

use super::{CommandSpec, bad, comp_id, layer_mut, resolve_layer};
use crate::{EngineError, Result, Session, cmd};

/// (comp, camera, target?) from `{camera?, layer?}` or the selection (a camera plus another layer).
fn refs(s: &Session, p: &Value, cmd: &str, need_target: bool) -> Result<(ItemId, LayerId, Option<LayerId>)> {
    let cid = comp_id(s, p)?;
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    let sel: Vec<LayerId> = s.state.selected_layers.iter().copied().filter(|l| comp.layer(*l).is_some()).collect();
    let is_cam = |c: &Comp, l: LayerId| c.layer(l).is_some_and(|l| l.is_camera());
    let cam = match p.get("camera") {
        Some(v) => resolve_layer(comp, v).ok_or_else(|| bad(cmd, format!("no layer {v}")))?,
        None => sel.iter().copied().find(|l| is_cam(comp, *l)).ok_or_else(|| bad(cmd, "select a camera layer"))?,
    };
    if !is_cam(comp, cam) {
        return Err(bad(cmd, "`camera` is not a camera layer"));
    }
    let target = match p.get("layer") {
        Some(v) => Some(resolve_layer(comp, v).ok_or_else(|| bad(cmd, format!("no layer {v}")))?),
        None => sel.iter().copied().find(|l| *l != cam),
    };
    if need_target && target.is_none() {
        return Err(bad(cmd, "select the camera and the layer to focus on"));
    }
    Ok((cid, cam, target))
}

fn set_focus_expr(s: &mut Session, cid: ItemId, cam: LayerId, label: &str, text: String) -> Result<Value> {
    s.edit(label, None, |proj, _| {
        let l = layer_mut(proj, cid, cam)?;
        let pr = l.props.prop_mut("cameraOptions/focusDistance").ok_or_else(|| bad("camera", "the camera has no Focus Distance"))?;
        pr.expr = Some(Expression { text: text.clone(), enabled: true });
        Ok(json!({"expression": text}))
    })
}

fn link_poi(s: &mut Session, p: &Value) -> Result<Value> {
    let (cid, cam, _) = refs(s, p, "camera.linkFocusToPoi", false)?;
    set_focus_expr(s, cid, cam, "Link Focus Distance to Point of Interest", "length(transform.pointOfInterest, transform.position)".into())
}

/// The focus expression for a target layer: its anchor point's distance along the camera's view
/// axis.
fn layer_expr(name: &str) -> String {
    let n = serde_json::to_string(name).unwrap_or_else(|_| "\"\"".into());
    format!(
        "var m = thisComp.layer({n});\nvar c = toWorld([0, 0, 0]);\nvar f = normalize(sub(toWorld([0, 0, 1]), c));\ndot(sub(m.toWorld(m.transform.anchorPoint), c), f)"
    )
}

fn link_layer(s: &mut Session, p: &Value) -> Result<Value> {
    let (cid, cam, target) = refs(s, p, "camera.linkFocusToLayer", true)?;
    let name = s.project.comp(cid).and_then(|c| c.layer(target?)).map(|l| l.name.clone()).ok_or(EngineError::NoComp)?;
    set_focus_expr(s, cid, cam, "Link Focus Distance to Layer", layer_expr(&name))
}

/// Camera depth of `target`'s anchor point at the current time.
pub(crate) fn focus_depth(s: &Session, cid: ItemId, cam: LayerId, target: LayerId) -> Option<f64> {
    let comp = s.project.comp(cid)?;
    let mut ctx = effectcraft_render::EvalCtx::new(&s.project, cid, comp, s.time_of(cid));
    ctx.expr = s.expr.as_deref();
    let c = ctx.comp.layer(cam)?;
    let t = ctx.comp.layer(target)?;
    let cs = effectcraft_render::three_d::camera::layer_camera(&ctx, c);
    let a = t.transform().map(|tr| ctx.v3(t, tr, "anchor", [0.0; 3])).unwrap_or([0.0; 3]);
    Some(cs.depth(ctx.world_matrix(t).apply(vec3(a[0], a[1], a[2]))))
}

fn set_to_layer(s: &mut Session, p: &Value) -> Result<Value> {
    let c = "camera.setFocusToLayer";
    let (cid, cam, target) = refs(s, p, c, true)?;
    let d = focus_depth(s, cid, cam, target.ok_or_else(|| bad(c, "no layer"))?).ok_or_else(|| bad(c, "can't evaluate the layers"))?;
    if d <= 0.0 {
        return Err(bad(c, "the layer is behind the camera"));
    }
    let t = s.time();
    s.edit("Set Focus Distance to Layer", None, |proj, _| {
        let l = layer_mut(proj, cid, cam)?;
        let lt = l.layer_time(t);
        let pr = l.props.prop_mut("cameraOptions/focusDistance").ok_or_else(|| bad(c, "the camera has no Focus Distance"))?;
        pr.set_value_at(lt, KV::Scalar(d));
        Ok(json!({"focusDistance": d}))
    })
}

fn camera_selected(s: &Session) -> std::result::Result<(), String> {
    let c = s.active_comp().ok_or("no composition is open")?;
    if s.state.selected_layers.iter().any(|l| c.layer(*l).is_some_and(|l| l.is_camera())) { Ok(()) } else { Err("select a camera layer".into()) }
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!("camera.linkFocusToPoi", "Link Focus Distance to Point of Interest", ["Layer", "Camera"], None, "{comp?, camera?}", camera_selected, link_poi),
        cmd!("camera.linkFocusToLayer", "Link Focus Distance to Layer", ["Layer", "Camera"], None, "{comp?, camera?, layer?}", camera_selected, link_layer),
        cmd!("camera.setFocusToLayer", "Set Focus Distance to Layer", ["Layer", "Camera"], None, "{comp?, camera?, layer?}", camera_selected, set_to_layer),
    ]
}

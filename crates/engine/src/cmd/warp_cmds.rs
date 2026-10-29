//! Warp Stabilizer commands (Effect ▸ Distort ▸ Warp Stabilizer's Analyze / Cancel, Animation ▸
//! Warp Stabilizer VFX, the Tracker panel button) and its status query.

use effectcraft_project::{GroupKind, ItemId, Layer, LayerId, PropGroup, Uid};
use serde_json::{Value, json};

use super::{CommandSpec, b_p, bad, has_comp, layer_p};
use crate::offload::{JobKind, WorkerJob};
use crate::{EngineError, Result, Session, cmd, query};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!("track.warpStabilizer", "Warp Stabilizer VFX", ["Animation"], None, "{layer?, wait?}", can_stabilize, warp_apply),
        cmd!("warp.analyze", "Analyze", [], None, "{layer?, effect?: uid|name|index, wait?: block until done}", has_comp, warp_analyze),
        cmd!("warp.cancel", "Cancel", [], None, "{}", is_analyzing, |s, _| Ok(json!({"stopped": s.stop_warp()}))),
        query!("warp.status", "Warp Stabilizer Status", "{layer?, effect?}", warp_status),
    ]
}

// ---------------------------------------------------------------- Warp Stabilizer

fn can_stabilize(s: &Session) -> std::result::Result<(), String> {
    has_comp(s)?;
    let c = s.active_comp().ok_or("no composition is open")?;
    match s.state.selected_layers.first().and_then(|l| c.layer(*l)) {
        Some(l) if l.has_video() && !l.switches.adjustment => Ok(()),
        Some(_) => Err("Warp Stabilizer needs a footage or composition layer".into()),
        None => Err("select a layer to stabilize".into()),
    }
}

fn is_analyzing(s: &Session) -> std::result::Result<(), String> {
    if s.is_warp_analyzing() { Ok(()) } else { Err("no Warp Stabilizer analysis is running".into()) }
}

fn is_warp(g: &PropGroup) -> bool {
    matches!(&g.kind, GroupKind::Effect { effect } if effect == effectcraft_effects::warp_stab::ID)
}

/// (comp, layer, effect uid) of a Warp Stabilizer: `layer` / `effect` (uid, name or 1-based
/// index among the layer's effects), else a selected one, else the layer's first.
fn warp_p(s: &Session, p: &Value, cmd: &str) -> Result<(ItemId, LayerId, Uid)> {
    let cid = super::comp_id(s, p)?;
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    let candidates: Vec<LayerId> = match p.get("layer") {
        Some(v) => vec![super::resolve_layer(comp, v).ok_or_else(|| bad(cmd, format!("no layer {v}")))?],
        None => {
            let mut v: Vec<LayerId> = s.state.selected_props.iter().map(|(l, _)| *l).collect();
            v.extend(s.state.selected_layers.iter().copied());
            v.extend(comp.layers.iter().map(|l| l.id));
            v
        }
    };
    for lid in candidates {
        let Some(fx) = comp.layer(lid).and_then(Layer::effects) else { continue };
        let g = match p.get("effect") {
            Some(Value::Number(n)) => {
                let n = n.as_u64().unwrap_or(0);
                fx.groups().find(|g| g.uid == n).or_else(|| fx.groups().nth((n as usize).saturating_sub(1)))
            }
            Some(Value::String(name)) => fx.groups().find(|g| &g.name == name),
            _ => s
                .state
                .selected_props
                .iter()
                .find_map(|(l, u)| (*l == lid).then(|| fx.groups().find(|g| g.uid == *u && is_warp(g))).flatten())
                .or_else(|| fx.groups().find(|g| is_warp(g))),
        };
        if let Some(g) = g.filter(|g| is_warp(g)) {
            return Ok((cid, lid, g.uid));
        }
        if p.get("layer").is_some() {
            break;
        }
    }
    Err(bad(cmd, "no Warp Stabilizer found (apply Effect ▸ Distort ▸ Warp Stabilizer first)"))
}

fn warp_analyze(s: &mut Session, p: &Value) -> Result<Value> {
    let (cid, lid, uid) = warp_p(s, p, "warp.analyze")?;
    let wait = b_p(p, "wait").unwrap_or(false);
    let n = s.start_warp(cid, lid, uid, wait).map_err(EngineError::Other)?;
    let mut out = json!({"layer": lid.0, "effect": uid, "frames": n, "running": s.is_warp_analyzing(), "progress": s.warp_progress()});
    if wait {
        out["status"] = warp_status(s, &json!({"layer": lid.0, "effect": uid, "comp": cid.0}))?;
    }
    Ok(out)
}

/// Animation ▸ Warp Stabilizer VFX / the Tracker panel button: apply the effect to the selected
/// layer and start analysing it.
fn warp_apply(s: &mut Session, p: &Value) -> Result<Value> {
    let (cid, lid) = layer_p(s, p, "track.warpStabilizer")?;
    let r = s.execute("effect.apply", json!({"effect": effectcraft_effects::warp_stab::ID, "layers": [lid.0], "comp": cid.0}))?;
    let uid = r["effects"].get(0).and_then(Value::as_u64).ok_or_else(|| EngineError::Other("the effect could not be applied".into()))?;
    let wait = b_p(p, "wait").unwrap_or(false);
    let n = s.start_warp(cid, lid, uid, wait).map_err(EngineError::Other)?;
    Ok(json!({"layer": lid.0, "effect": uid, "frames": n, "running": s.is_warp_analyzing(), "progress": s.warp_progress()}))
}

fn warp_status(s: &mut Session, p: &Value) -> Result<Value> {
    s.poll_warp(false);
    let mut out = json!({"running": s.is_warp_analyzing(), "progress": s.warp_progress(), "banner": s.warp_progress().map(|p| p.banner())});
    let Ok((cid, lid, uid)) = warp_p(s, p, "warp.status") else { return Ok(out) };
    // (the project's `Arc`: the borrows outlive starting a job below)
    let project = s.project.clone();
    let comp = project.comp(cid).ok_or(EngineError::NoComp)?;
    let layer = comp.layer(lid).ok_or(EngineError::NoComp)?;
    let g = layer.props.find_group(uid).ok_or(EngineError::NoComp)?;
    let ctx = effectcraft_render::EvalCtx::new(&project, cid, comp, s.time_of(cid));
    let params = effectcraft_effects::flatten_params(g, &mut |pr| ctx.value(layer, pr));
    let analysis = effectcraft_effects::warp_stab::analysis(&params);
    out["layer"] = json!(lid.0);
    out["effect"] = json!(uid);
    out["analyzed"] = json!(analysis.is_some());
    out["pending"] = json!(s.warp_pending.contains(&(cid, lid, uid)));
    if let Some(a) = analysis {
        out["frames"] = json!(a.frames.len());
        out["size"] = json!(a.size);
        let lt = layer.layer_time(s.time_of(cid)).seconds();
        // Solving the plan takes seconds for a long Subspace Warp clip: where jobs run
        // elsewhere (the browser) it is never solved here but in a job worker, and the status
        // says "stabilizing" until the summary arrives (`WorkerReply::WarpPlan`).
        let plan = if s.offloads() {
            let cached = effectcraft_effects::warp_stab::cached_summary_at(&params, lt);
            if cached.is_none() {
                out["stabilizing"] = json!(true);
                if s.offloaded(JobKind::Warp).is_none() && s.offloaded(JobKind::WarpPlan).is_none() {
                    let t = s.time_of(cid);
                    if let Err(e) = s.offload_analysis(WorkerJob::WarpPlan { comp: cid, layer: lid, effect: uid, time: t }) {
                        log::warn!("warp.status: {e}");
                    }
                }
            }
            cached
        } else {
            effectcraft_effects::warp_stab::summary_at(&params, lt)
        };
        if let Some((k, plan)) = plan {
            out["frame"] = json!(k);
            out["autoScale"] = json!(plan.auto_scale * 100.0);
            out["crop"] = json!(plan.crop);
            out["validFraction"] = json!(plan.valid_fraction);
            out["warp"] = json!(plan.warps[k]);
            let pts = &a.frames[k];
            out["features"] = json!(pts.features);
            out["inliers"] = json!(pts.inliers);
        }
    }
    Ok(out)
}

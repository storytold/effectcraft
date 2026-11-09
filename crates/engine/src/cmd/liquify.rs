//! Liquify brush strokes (`liquify.stroke`, `liquify.clear`).
//!
//! A stroke is appended to the layer's Liquify effect (added when the layer has none) as one
//! line of its hidden Distortion Mesh data (see `effectcraft_effects::distort4`). Brush size,
//! pressure, turbulent jitter and clone offset default to the effect's own tool options. When
//! the Distortion Mesh is animated, the stroke goes into a keyframe at the current time (copying
//! the mesh in effect there), as After Effects records mesh keyframes.

use effectcraft_effects::distort4::{LIQUIFY_TOOLS, LiquifyStroke, parse_strokes};
use effectcraft_keyframe::{Keyframe, Value as KV};
use effectcraft_project::build::Ids;
use effectcraft_project::{GroupKind, PropGroup, Uid};
use serde_json::{Value, json};

use super::{CommandSpec, bad, f_p, has_comp, layer_mut, layer_p, str_p};
use crate::{EngineError, Result, Session, cmd};

const ID: &str = "ec.distort.liquify";

fn is_liquify(g: &PropGroup) -> bool {
    matches!(&g.kind, GroupKind::Effect { effect } if effect == ID)
}

fn liquify_effect(fx: &mut PropGroup, ids: &mut Ids, layer_size: [f64; 2], want: Option<Uid>) -> Result<Uid> {
    if let Some(u) = want {
        return fx.groups().find(|g| g.uid == u && is_liquify(g)).map(|g| g.uid).ok_or_else(|| bad("liquify.stroke", "no such Liquify effect"));
    }
    if let Some(g) = fx.groups().filter(|g| is_liquify(g)).last() {
        return Ok(g.uid);
    }
    let spec = effectcraft_effects::find(ID).ok_or_else(|| EngineError::Other("Liquify effect missing".into()))?;
    let g = effectcraft_effects::instantiate(spec, ids, "Liquify", layer_size);
    let uid = g.uid;
    fx.children.push(g.into());
    Ok(uid)
}

fn num_of(g: &PropGroup, id: &str, default: f64) -> f64 {
    g.get(id).map(|p| p.value.as_f64()).unwrap_or(default)
}

fn stroke(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "liquify.stroke";
    let (cid, lid) = layer_p(s, p, cmd)?;
    let tool_name = str_p(p, "tool").unwrap_or("warp");
    let tool = LIQUIFY_TOOLS
        .iter()
        .position(|t| t.eq_ignore_ascii_case(tool_name))
        .ok_or_else(|| bad(cmd, format!("unknown tool `{tool_name}` ({})", LIQUIFY_TOOLS.join(" | "))))?;
    let raw = p.get("points").and_then(Value::as_array).ok_or_else(|| bad(cmd, "missing `points` [[x, y], …] (effect space)"))?;
    let mut points = vec![];
    for q in raw {
        let a = q.as_array().ok_or_else(|| bad(cmd, "each point is [x, y]"))?;
        let (Some(x), Some(y)) = (a.first().and_then(Value::as_f64), a.get(1).and_then(Value::as_f64)) else {
            return Err(bad(cmd, "each point is [x, y]"));
        };
        points.push([x, y]);
    }
    if points.is_empty() {
        return Err(bad(cmd, "`points` is empty"));
    }
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    let t = comp.frame_rate.snap_nearest(super::time_p(s, p, Some(comp)));
    let layer = comp.layer(lid).ok_or_else(|| bad(cmd, "no layer"))?;
    if layer.effects().is_none() || !layer.source.is_av() {
        return Err(bad(cmd, "Liquify works on footage, solid, text, shape and precomp layers"));
    }
    let lt = layer.layer_time(t);
    let size = effectcraft_render::effect_bounds(&s.project, comp, layer).0;
    let want = p.get("effect").and_then(Value::as_u64);
    let (fx_uid, count) = s.edit("Liquify Stroke", None, |proj, _st| {
        let mut next = proj.next_id;
        let l = layer_mut(proj, cid, lid)?;
        let fx = l.props.sub_mut("effects").ok_or_else(|| bad(cmd, "this layer has no effects"))?;
        let mut ids = Ids(&mut next);
        let fx_uid = liquify_effect(fx, &mut ids, size, want)?;
        proj.next_id = next;
        let l = layer_mut(proj, cid, lid)?;
        let g = l.props.sub_mut("effects").and_then(|fx| fx.find_group_mut(fx_uid)).ok_or_else(|| bad(cmd, "Liquify effect gone"))?;
        let co = g.get("cloneOffset").map(|p| p.value.as_vec2()).unwrap_or([0.0; 2]);
        let st = LiquifyStroke {
            tool,
            size: f_p(p, "size").unwrap_or_else(|| num_of(g, "brushSize", 64.0)).max(1.0),
            pressure: f_p(p, "pressure").unwrap_or_else(|| num_of(g, "brushPressure", 50.0)).clamp(1.0, 100.0),
            jitter: f_p(p, "jitter").unwrap_or_else(|| num_of(g, "turbulentJitter", 70.0)).clamp(1.0, 100.0),
            clone_offset: match p.get("cloneOffset").and_then(Value::as_array) {
                Some(a) => [a.first().and_then(Value::as_f64).unwrap_or(0.0), a.get(1).and_then(Value::as_f64).unwrap_or(0.0)],
                None => co,
            },
            points: points.clone(),
        };
        let prop = g.get_mut("distortionMesh").ok_or_else(|| bad(cmd, "Liquify effect has no Distortion Mesh"))?;
        let append = |cur: &str| if cur.trim().is_empty() { st.to_line() } else { format!("{}\n{}", cur.trim_end(), st.to_line()) };
        let text = if prop.keys.is_empty() {
            let cur = match &prop.value {
                KV::Str(x) => x.clone(),
                _ => String::new(),
            };
            let v = append(&cur);
            prop.value = KV::Str(v.clone());
            v
        } else {
            // Hold keyframes: the mesh in effect at `lt` is the last key at or before it.
            let cur = prop.keys.iter().rfind(|k| k.time <= lt).or(prop.keys.first()).map(|k| k.value.clone());
            let cur = match cur {
                Some(KV::Str(x)) => x,
                _ => String::new(),
            };
            let v = append(&cur);
            match prop.keys.iter_mut().find(|k| k.time == lt) {
                Some(k) => k.value = KV::Str(v.clone()),
                None => {
                    prop.keys.push(Keyframe::new(lt, KV::Str(v.clone())));
                    prop.keys.sort_by_key(|k| k.time);
                }
            }
            v
        };
        Ok((fx_uid, parse_strokes(&text).len()))
    })?;
    Ok(json!({"effect": fx_uid, "strokes": count}))
}

fn clear(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "liquify.clear";
    let (cid, lid) = layer_p(s, p, cmd)?;
    let want = p.get("effect").and_then(Value::as_u64);
    s.edit("Clear Liquify Mesh", None, |proj, _| {
        let l = layer_mut(proj, cid, lid)?;
        let fx = l.props.sub_mut("effects").ok_or_else(|| bad(cmd, "no effects"))?;
        let uid =
            fx.groups().filter(|g| is_liquify(g) && want.is_none_or(|u| u == g.uid)).last().map(|g| g.uid).ok_or_else(|| bad(cmd, "no Liquify effect"))?;
        let g = fx.find_group_mut(uid).ok_or_else(|| bad(cmd, "no Liquify effect"))?;
        let prop = g.get_mut("distortionMesh").ok_or_else(|| bad(cmd, "no Distortion Mesh"))?;
        prop.keys.clear();
        prop.value = KV::Str(String::new());
        Ok(())
    })?;
    Ok(Value::Null)
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "liquify.stroke",
            "Liquify Stroke",
            [],
            None,
            "{layer?, effect?: uid, tool?: warp|turbulence|twirlClockwise|twirlCounterclockwise|pucker|bloat|shiftPixels|reflection|clone|reconstruction|freeze|thaw, points: [[x,y],…] (effect space), size?, pressure? (1-100), jitter? (1-100), cloneOffset?: [dx,dy], time? (s) | frame?}",
            has_comp,
            stroke
        ),
        cmd!("liquify.clear", "Clear Liquify Mesh", [], None, "{layer?, effect?: uid}", has_comp, clear),
    ]
}

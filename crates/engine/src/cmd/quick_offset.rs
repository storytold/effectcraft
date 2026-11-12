//! Quick Offset (After Effects 25.4): Ctrl/Cmd+Alt dragging selected layers or keyframes in the
//! Timeline staggers them in the order they were selected. The first stays, the last moves by the
//! whole drag and the ones between spread evenly, so they land on subframes; each layer's
//! selected keys move together.

use std::collections::HashMap;

use effectcraft_project::LayerId;
use effectcraft_time::{TICKS_PER_SECOND, Tick};
use serde_json::{Value, json};

use super::prop::{key_gesture, layer_shift, move_key_refs};
use super::{CommandSpec, b_p, bad, f_p, has_keys, has_layers, layers_p, merge_p};
use crate::{EngineError, Result, Session, cmd};

/// `delta` (comp seconds) in ticks; out of range is an error, not a clamped drag.
fn delta_p(p: &Value, name: &str) -> Result<Tick> {
    let d = f_p(p, "delta").ok_or_else(|| bad(name, "missing `delta` (comp seconds)"))?;
    let t = (d * TICKS_PER_SECOND as f64).round();
    if !t.is_finite() || t.abs() > Tick::MAX.0 as f64 {
        return Err(bad(name, "`delta` is out of range"));
    }
    Ok(Tick(t as i64))
}

/// How far each of `n` layers moves, in order, when the last moves by `total`: `total × i / (n − 1)`
/// to the nearest tick, so the steps between them are equal.
fn shares(total: Tick, n: usize) -> Vec<Tick> {
    let last = n.saturating_sub(1).max(1) as i128;
    (0..n)
        .map(|i| {
            let x = total.0 as i128 * i as i128;
            Tick(((x + x.signum() * (last / 2)) / last) as i64)
        })
        .collect()
}

/// `t + d`, or an error (`what` names the time) when that leaves the range of times.
fn shifted(t: Tick, d: Tick, name: &str, what: &str) -> Result<Tick> {
    let v = t.0 as i128 + d.0 as i128;
    if v < Tick::MIN.0 as i128 || v > Tick::MAX.0 as i128 {
        return Err(bad(name, format!("{what} would move out of range")));
    }
    Ok(Tick(v as i64))
}

/// The layers to stagger, in order: unlocked, each once. At least two.
fn order(name: &str, layers: impl IntoIterator<Item = LayerId>, unlocked: impl Fn(LayerId) -> bool) -> Result<Vec<LayerId>> {
    let mut out: Vec<LayerId> = vec![];
    for l in layers {
        if unlocked(l) && !out.contains(&l) {
            out.push(l);
        }
    }
    if out.len() < 2 {
        return Err(bad(name, "select at least two unlocked layers (or keys on two layers), in the order to offset them"));
    }
    Ok(out)
}

/// `layer.quickOffset`: move the layers (`layers`, else the selection, in that order) by their
/// share of `delta`, the last layer's move. With `merge` and `fromStart`, `delta` is the whole drag
/// and every step applies to the layers as they were before it.
fn layers(s: &mut Session, p: &Value) -> Result<Value> {
    const NAME: &str = "layer.quickOffset";
    let d = delta_p(p, NAME)?;
    let (cid, ids) = layers_p(s, p)?;
    let merge = merge_p(p);
    let gesture = b_p(p, "fromStart").unwrap_or(false) && merge.is_some_and(|m| s.history.merge_key.as_deref() == Some(m));
    let base = s.history.undo.last().filter(|_| gesture).map(|(_, b)| b.clone());
    let start = base.clone().unwrap_or_else(|| s.project.clone());
    let comp = start.comp(cid).ok_or(EngineError::NoComp)?;
    let order = order(NAME, ids, |id| comp.layer(id).is_some_and(|l| !l.switches.locked))?;
    let moves: Vec<(LayerId, Tick)> = order.iter().copied().zip(shares(d, order.len())).collect();
    s.edit("Quick Offset", merge, |proj, _| {
        if let Some(b) = &base {
            *proj = (**b).clone();
        }
        let comp = proj.comp_mut(cid).ok_or(EngineError::NoComp)?;
        for (id, d) in &moves {
            let l = comp.layer_mut(*id).ok_or(EngineError::NoComp)?;
            let what = format!("layer \"{}\"", l.name);
            let times = (shifted(l.start_time, *d, NAME, &what)?, shifted(l.in_point, *d, NAME, &what)?, shifted(l.out_point, *d, NAME, &what)?);
            (l.start_time, l.in_point, l.out_point) = times;
        }
        Ok(json!({"layers": moves.iter().map(|(id, d)| json!({"layer": id.0, "offset": d.seconds()})).collect::<Vec<_>>()}))
    })
}

/// `keys.quickOffset`: the selected keys of each layer move together by the layer's share of
/// `delta` (comp seconds, the last layer's move), the layers in the order their first key was
/// selected. Like `keys.move`, steps sharing `merge` are one drag; with `fromStart`, `delta` is the
/// whole drag.
fn keys(s: &mut Session, p: &Value) -> Result<Value> {
    const NAME: &str = "keys.quickOffset";
    let d = delta_p(p, NAME)?;
    let cid = s.active_comp_id().ok_or(EngineError::NoComp)?;
    let merge = merge_p(p);
    let (from, total, base) = key_gesture(s, merge, d, b_p(p, "fromStart").unwrap_or(false));
    let start = base.clone().unwrap_or_else(|| s.project.clone());
    let comp = start.comp(cid).ok_or(EngineError::NoComp)?;
    let order = order(NAME, from.iter().map(|k| k.layer), |id| comp.layer(id).is_some_and(|l| !l.switches.locked))?;
    let by_layer: HashMap<LayerId, Tick> = order.iter().copied().zip(shares(total, order.len())).collect();
    let next = merge.map(|m| crate::KeyMove { merge: m.to_string(), from: from.clone(), total });
    let moved = s.edit("Quick Offset Keyframes", merge, |proj, st| {
        if let Some(b) = &base {
            *proj = (**b).clone();
        }
        let comp = proj.comp_mut(cid).ok_or(EngineError::NoComp)?;
        st.selected_keys = move_key_refs(comp, &from, |l, t| {
            let Some(d) = by_layer.get(&l.id) else { return Ok(t) };
            let what = format!("a key of \"{}\"", l.name);
            shifted(l.start_time, *d, NAME, &what)?;
            shifted(t, layer_shift(l, *d), NAME, &what)
        })?;
        Ok(json!({"keys": st.selected_keys.len(), "layers": order.iter().map(|id| json!({"layer": id.0, "offset": by_layer.get(id).copied().unwrap_or_default().seconds()})).collect::<Vec<_>>()}))
    })?;
    // Only a step that applied counts towards the drag (a refused one is as if never sent).
    s.key_move = next;
    Ok(moved)
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "layer.quickOffset",
            "Quick Offset",
            [],
            None,
            "{delta (comp s: the last layer's move), layers? (in order; default the selection), merge?, fromStart?: bool (with merge: delta is the whole drag)}",
            has_layers,
            layers
        ),
        cmd!(
            "keys.quickOffset",
            "Quick Offset Keyframes",
            [],
            None,
            "{delta (comp s: the last layer's keys' move), merge?, fromStart?: bool (with merge: delta is the whole drag)}",
            has_keys,
            keys
        ),
    ]
}

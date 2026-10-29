//! Keyframe geometry edits from the viewer and the Graph Editor: the Graph Editor transform box
//! (scale / move selected keys in time and value; Alt-drag of a key group in the timeline) and
//! spatial Bezier tangents dragged on a motion path.

use effectcraft_keyframe::Keyframe;
use effectcraft_project::{LayerId, Uid};
use effectcraft_time::Tick;
use serde_json::{Value, json};

use super::prop::prop_ref;
use super::{CommandSpec, b_p, bad, f_p, has_comp, has_keys, merge_p};
use crate::{EngineError, KeyRef, Result, Session, cmd};

fn v3(v: Option<&Value>) -> Option<[f64; 3]> {
    let a = v?.as_array()?;
    Some([a.first()?.as_f64()?, a.get(1)?.as_f64()?, a.get(2).and_then(Value::as_f64).unwrap_or(0.0)])
}

/// Transform the selected keys: times scale about `timeAnchor` (comp seconds, default the first
/// selected key) by `timeScale` and move by `timeOffset`; values scale about `valueAnchor` by
/// `valueScale` and move by `valueOffset` (every dimension, or only `dim`). Times land on frames.
/// With `fromStart` (drags), each step of one merged gesture gives the whole transform since the
/// drag started and applies to the keys as they were then: keys squeezed onto one frame on the
/// way, or rounded steps, are never lost; only the final position counts.
fn transform(s: &mut Session, p: &Value) -> Result<Value> {
    let cid = s.active_comp_id().ok_or(EngineError::NoComp)?;
    let merge = merge_p(p);
    let from_start = b_p(p, "fromStart").unwrap_or(false) && merge.is_some();
    let gesture = from_start && merge.is_some_and(|m| s.history.merge_key.as_deref() == Some(m)) && s.key_move.as_ref().map(|g| g.merge.as_str()) == merge;
    let (sel, base) = match (&s.key_move, s.history.undo.last()) {
        (Some(g), Some((_, b))) if gesture => (g.from.clone(), Some(b.clone())),
        _ => (s.state.selected_keys.clone(), None),
    };
    if from_start {
        s.key_move = merge.map(|m| crate::KeyMove { merge: m.to_string(), from: sel.clone(), total: Tick::ZERO });
    }
    // The comp as it was when the drag started (its keys are what the transform applies to).
    let start = base.clone().unwrap_or_else(|| s.project.clone());
    let comp = start.comp(cid).ok_or(EngineError::NoComp)?;
    let fr = comp.frame_rate;
    let ts = f_p(p, "timeScale").unwrap_or(1.0);
    let toff = f_p(p, "timeOffset").unwrap_or(0.0);
    let vs = f_p(p, "valueScale").unwrap_or(1.0);
    let voff = f_p(p, "valueOffset").unwrap_or(0.0);
    let va = f_p(p, "valueAnchor").unwrap_or(0.0);
    // `dim` (one dimension) or `dims` (several); none = every dimension.
    let dims: Option<Vec<usize>> = match (p.get("dim").and_then(Value::as_u64), p.get("dims").and_then(Value::as_array)) {
        (Some(d), _) => Some(vec![d as usize]),
        (None, Some(a)) => Some(a.iter().filter_map(Value::as_u64).map(|d| d as usize).collect()),
        _ => None,
    };
    if !ts.is_finite() || ts <= 0.0 {
        return Err(bad("keys.transform", "`timeScale` must be positive"));
    }
    // Default time anchor: the earliest selected key (comp time).
    let first = sel.iter().filter_map(|k| comp.layer(k.layer).map(|l| l.comp_time(k.time))).min().unwrap_or(Tick::ZERO);
    let ta = f_p(p, "timeAnchor").map(Tick::from_seconds_f64).unwrap_or(first);
    let map_t = |ct: Tick| -> Tick {
        let x = ta.seconds() + (ct.seconds() - ta.seconds()) * ts + toff;
        fr.snap_nearest(Tick::from_seconds_f64(x.max(0.0)))
    };
    let touch_values = vs != 1.0 || voff != 0.0;
    s.edit("Transform Keyframes", merge, |proj, st| {
        if let Some(b) = base {
            *proj = (*b).clone();
        }
        let comp = proj.comp_mut(cid).ok_or(EngineError::NoComp)?;
        let mut groups: std::collections::BTreeMap<(LayerId, Uid), Vec<Tick>> = Default::default();
        for k in &sel {
            groups.entry((k.layer, k.prop)).or_default().push(k.time);
        }
        let mut new_sel = vec![];
        for ((lid, uid), times) in groups {
            let Some(layer) = comp.layer_mut(lid) else { continue };
            let (start, stretch) = (layer.start_time, layer.stretch);
            let to_comp = |lt: Tick| if (stretch - 100.0).abs() < 1e-9 { start + lt } else { start + Tick((lt.0 as f64 * stretch / 100.0) as i64) };
            let to_layer = |ct: Tick| {
                let d = ct - start;
                if (stretch - 100.0).abs() < 1e-9 { d } else { Tick((d.0 as f64 * 100.0 / stretch) as i64) }
            };
            let Some(pr) = layer.props.find_mut(uid) else { continue };
            let (mut moved, mut kept): (Vec<Keyframe>, Vec<Keyframe>) = pr.keys.drain(..).partition(|k| times.contains(&k.time));
            for k in &mut moved {
                k.time = to_layer(map_t(to_comp(k.time)));
                if touch_values {
                    let mut c = k.value.components();
                    for (d, v) in c.iter_mut().enumerate() {
                        if dims.as_ref().is_none_or(|x| x.contains(&d)) {
                            *v = va + (*v - va) * vs + voff;
                        }
                    }
                    k.value = k.value.with_components(&c);
                }
            }
            // Transformed keys win over unselected keys at the same time.
            kept.retain(|k| !moved.iter().any(|m| m.time == k.time));
            moved.sort_by_key(|k| k.time);
            moved.dedup_by_key(|k| k.time);
            for k in &moved {
                new_sel.push(KeyRef { layer: lid, prop: uid, time: k.time });
            }
            kept.extend(moved);
            kept.sort_by_key(|k| k.time);
            pr.keys = kept;
            effectcraft_keyframe::retime_roving(&mut pr.keys, pr.spatial);
        }
        st.selected_keys = new_sel;
        Ok(json!({"keys": st.selected_keys.len()}))
    })
}

/// Spatial tangents of one key of a spatial property (motion path handles), relative to the key
/// value. Setting one side only mirrors it to the other unless `break` is true; the key stops
/// being auto Bezier.
fn spatial_tangents(s: &mut Session, p: &Value) -> Result<Value> {
    let (cid, lid, uid) = prop_ref(s, p, "keys.setSpatialTangents")?;
    let t = Tick::from_seconds_f64(f_p(p, "time").ok_or_else(|| bad("keys.setSpatialTangents", "missing `time` (layer s)"))?);
    let (tin, tout) = (v3(p.get("in")), v3(p.get("out")));
    if tin.is_none() && tout.is_none() {
        return Err(bad("keys.setSpatialTangents", "pass `in` and/or `out` [dx, dy, dz?]"));
    }
    let brk = b_p(p, "break").unwrap_or(false);
    s.edit("Edit Spatial Tangents", merge_p(p), |proj, _| {
        let l = super::layer_mut(proj, cid, lid)?;
        let pr = l.props.find_mut(uid).ok_or_else(|| bad("keys.setSpatialTangents", "no property"))?;
        if !pr.spatial {
            return Err(bad("keys.setSpatialTangents", "not a spatial property"));
        }
        let i = pr.keys.iter().position(|k| k.time == t).or_else(|| pr.keys.iter().enumerate().min_by_key(|(_, k)| (k.time.0 - t.0).abs()).map(|(i, _)| i));
        let i = i.ok_or_else(|| bad("keys.setSpatialTangents", "the property has no keyframes"))?;
        let (cur_in, cur_out) = effectcraft_keyframe::spatial_tangents(&pr.keys, i);
        let k = &mut pr.keys[i];
        let neg = |v: [f64; 3]| [-v[0], -v[1], -v[2]];
        let (ni, no) = match (tin, tout) {
            (Some(a), Some(b)) => (a, b),
            (Some(a), None) => (a, if brk { cur_out } else { neg(a) }),
            (None, Some(b)) => (if brk { cur_in } else { neg(b) }, b),
            (None, None) => (cur_in, cur_out),
        };
        k.spatial_auto = false;
        k.spatial_continuous = !brk && tin.is_some() != tout.is_some();
        k.spatial_in = ni;
        k.spatial_out = no;
        effectcraft_keyframe::retime_roving(&mut pr.keys, true);
        Ok(json!({"in": ni, "out": no}))
    })
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "keys.transform",
            "Transform Keyframes",
            [],
            None,
            "{timeScale?, timeAnchor? (comp s), timeOffset? (s), valueScale?, valueAnchor?, valueOffset?, dim? | dims?: [d…], merge?, fromStart?: bool (with merge: values are the whole transform since the drag started)}",
            has_keys,
            transform
        ),
        cmd!(
            "keys.setSpatialTangents",
            "Edit Spatial Tangents",
            [],
            None,
            "{layer?, path|prop, time (layer s), in?: [dx,dy,dz?], out?, break?, merge?}",
            has_comp,
            spatial_tangents
        ),
    ]
}

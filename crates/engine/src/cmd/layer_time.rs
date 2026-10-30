//! Layer ▸ Time: Time Stretch, Time-Reverse Layer, Enable Time Remapping, Freeze Frame and
//! Freeze on Last Frame.
//!
//! Time Remap is a `timeRemap` property at the top of the layer's tree whose value is the
//! source time (seconds) to show; its keys live in layer time like every other property.

use effectcraft_keyframe::{Keyframe, Value as KV};
use effectcraft_project::build::Ids;
use effectcraft_project::{Layer, LayerId, LayerSource, Node, ParamUi, Property};
use effectcraft_time::Tick;
use serde_json::{Value, json};

use super::{CommandSpec, b_p, bad, f_p, has_layers, layers_p, str_p};
use crate::{EngineError, KeyRef, Result, Session, cmd};

/// Layers whose source has its own time (precomps and footage) can be remapped.
fn remappable(l: &Layer) -> bool {
    matches!(l.source, LayerSource::Comp { .. } | LayerSource::Footage { .. })
}

fn time_stretch(s: &mut Session, p: &Value) -> Result<Value> {
    let (cid, ids) = layers_p(s, p)?;
    let reverse = str_p(p, "op") == Some("reverse");
    let pct = f_p(p, "percent");
    let dur = f_p(p, "duration");
    let hold = str_p(p, "hold").unwrap_or("in").to_string();
    // `to`: stretch so the In (or Out) point lands on the current time, the other point held.
    let to = str_p(p, "to").map(str::to_string);
    if to.as_deref().is_some_and(|t| t != "in" && t != "out") {
        return Err(bad("layer.timeStretch", "to: in|out"));
    }
    let cti = s.time();
    let comp_rate = s.project.comp(cid).ok_or(EngineError::NoComp)?.frame_rate;
    let fd = comp_rate.frame_duration();
    s.edit(if reverse { "Time-Reverse Layer" } else { "Time Stretch" }, None, |proj, _| {
        let comp = proj.comp_mut(cid).ok_or(EngineError::NoComp)?;
        for l in comp.layers.iter_mut_where(|l| ids.contains(&l.id)) {
            let old = l.stretch;
            if reverse {
                // Keep the in/out points; play the visible span backwards.
                let last = l.layer_time(l.out_point - fd);
                l.stretch = -old;
                l.start_time = l.in_point - Tick((last.0 as f64 * l.stretch / 100.0).round() as i64);
                continue;
            }
            let span = (l.out_point - l.in_point).seconds();
            let (dur, hold) = match to.as_deref() {
                Some("in") => (Some((l.out_point - cti).seconds()), "out"),
                Some(_) => (Some((cti - l.in_point).seconds()), "in"),
                None => (dur, hold.as_str()),
            };
            // The current time is on the wrong side of the held point: nothing to stretch to.
            if to.is_some() && dur.is_some_and(|d| d < fd.seconds()) {
                continue;
            }
            let pct = if to.is_some() { None } else { pct };
            let new = match (pct, dur) {
                (Some(v), _) => v,
                (None, Some(d)) if span > 0.0 => old * d / span,
                _ => old,
            };
            if new.abs() < 1.0 {
                return Err(bad("layer.timeStretch", "the stretch factor must be at least 1 %"));
            }
            // The hold point keeps showing the same layer time.
            let h = match hold {
                "current" => cti,
                "out" => l.out_point,
                _ => l.in_point,
            };
            let lt_h = l.layer_time(h);
            let (lin, lout) = (l.layer_time(l.in_point), l.layer_time(l.out_point));
            l.stretch = new;
            l.start_time = h - Tick((lt_h.0 as f64 * new / 100.0).round() as i64);
            let (a, b) = (l.comp_time(lin), l.comp_time(lout));
            let fr = comp_rate;
            l.in_point = fr.snap_nearest(a.min(b));
            l.out_point = fr.snap_nearest(a.max(b)).max(l.in_point + fr.frame_duration());
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

fn remap_prop(ids: &mut Ids) -> Property {
    let mut p = ids.prop("timeRemap", "Time Remap", KV::Scalar(0.0)).with_ui(ParamUi::Number);
    p.ui = ParamUi::Number;
    p
}

/// Source time (seconds) a layer shows at comp time `t` without remapping.
fn source_secs(l: &Layer, t: Tick) -> f64 {
    l.layer_time(t).seconds()
}

fn enable_time_remap(s: &mut Session, p: &Value) -> Result<Value> {
    let (cid, ids) = layers_p(s, p)?;
    let want = b_p(p, "value");
    let fd = s.project.comp(cid).ok_or(EngineError::NoComp)?.frame_duration();
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    if !comp.layers.iter().filter(|l| ids.contains(&l.id)).any(remappable) {
        return Err(bad("layer.enableTimeRemap", "time remapping needs a footage or composition layer"));
    }
    // Each layer's source duration (stills have none): a layer extended past it ends its remap
    // at the source's end, where its last frame starts to hold (After Effects).
    let src_end: Vec<(LayerId, Tick)> =
        comp.layers.iter().filter(|l| ids.contains(&l.id)).filter_map(|l| Some((l.id, l.source.item().and_then(|i| s.project.item(i))?.duration()?))).collect();
    let on = s.edit("Enable Time Remapping", None, |proj, _| {
        let mut next = proj.next_id;
        let comp = proj.comp_mut(cid).ok_or(EngineError::NoComp)?;
        let mut on = false;
        for l in comp.layers.iter_mut_where(|l| ids.contains(&l.id) && remappable(l)) {
            let has = l.props.get("timeRemap").is_some();
            let target = want.unwrap_or(!has);
            on |= target;
            if target && !has {
                let mut pr = remap_prop(&mut Ids(&mut next));
                let (lin, lout) = (l.layer_time(l.in_point), l.layer_time(l.out_point));
                let (a, mut b) = (lin.min(lout), lin.max(lout));
                if let Some((_, end)) = src_end.iter().find(|(id, _)| *id == l.id)
                    && lin <= lout
                    && *end > a
                {
                    b = b.min(*end);
                }
                pr.value = KV::Scalar(source_secs(l, l.in_point));
                pr.keys = vec![Keyframe::new(a, KV::Scalar(a.seconds())), Keyframe::new(b, KV::Scalar(b.seconds()))];
                if lin > lout {
                    // Reversed layers keep showing what they showed.
                    pr.keys[0].value = KV::Scalar(source_secs(l, l.out_point - fd));
                    pr.keys[1].value = KV::Scalar(source_secs(l, l.in_point));
                }
                l.props.children.insert(0, Node::Prop(pr));
            } else if !target && has {
                l.props.children.retain(|c| c.match_id() != "timeRemap");
            }
        }
        proj.next_id = next;
        Ok(on)
    })?;
    Ok(json!(on))
}

/// Layers of the active comp whose Time Remap property is selected with all of its keys (clicking
/// its name selects them all, and a key selects its property): Edit ▸ Clear turns their time
/// remapping off instead of deleting the keys or the layer.
pub(crate) fn selected_time_remap(s: &Session) -> Vec<LayerId> {
    let Some(comp) = s.active_comp() else { return vec![] };
    let st = &s.state;
    st.selected_props
        .iter()
        .filter(|(lid, uid)| {
            comp.layer(*lid)
                .and_then(|l| l.props.get("timeRemap"))
                .filter(|pr| pr.uid == *uid)
                .is_some_and(|pr| pr.keys.iter().all(|k| st.selected_keys.contains(&KeyRef { layer: *lid, prop: *uid, time: k.time })))
        })
        .map(|(lid, _)| *lid)
        .collect()
}

/// Turn time remapping off on `layers` (one undo step) and drop the removed property and its
/// keys from the selection.
pub(crate) fn disable_time_remap(s: &mut Session, layers: &[LayerId]) -> Result<Value> {
    let ids: Vec<u64> = layers.iter().map(|l| l.0).collect();
    let r = s.execute("layer.enableTimeRemap", json!({"layers": ids, "value": false}))?;
    let comp = s.active_comp();
    let gone = |lid: LayerId, uid: u64| layers.contains(&lid) && comp.and_then(|c| c.layer(lid)).is_none_or(|l| l.props.find(uid).is_none());
    let (props, keys): (Vec<_>, Vec<_>) = (
        s.state.selected_props.iter().copied().filter(|(l, u)| !gone(*l, *u)).collect(),
        s.state.selected_keys.iter().copied().filter(|k| !gone(k.layer, k.prop)).collect(),
    );
    s.state.selected_props = props;
    s.state.selected_keys = keys;
    Ok(r)
}

/// Freeze Frame: time remapping holding the current source frame for the whole layer.
fn freeze(s: &mut Session, p: &Value, last: bool) -> Result<Value> {
    let (cid, ids) = layers_p(s, p)?;
    let cti = s.time();
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    let (fd, cdur) = (comp.frame_duration(), comp.duration);
    if !comp.layers.iter().filter(|l| ids.contains(&l.id)).any(remappable) {
        return Err(bad("layer.freezeFrame", "freezing needs a footage or composition layer"));
    }
    // What each layer shows now (honouring an existing remap) — evaluated before the edit.
    let ectx = effectcraft_render::EvalCtx {
        project: &s.project,
        comp_id: cid,
        comp,
        time: cti,
        expr: s.expr.as_deref(),
        footage: Some(s.footage.as_ref()),
        property_overrides: &[],
        media_overrides: &[],
    };
    let now: Vec<(effectcraft_project::LayerId, f64)> = comp
        .layers
        .iter()
        .filter(|l| ids.contains(&l.id) && remappable(l))
        .map(|l| {
            let at = if last { ectx.at(l.out_point - fd) } else { ectx };
            (l.id, at.source_time(l).seconds())
        })
        .collect();
    s.edit(if last { "Freeze On Last Frame" } else { "Freeze Frame" }, None, |proj, _| {
        let mut next = proj.next_id;
        let comp = proj.comp_mut(cid).ok_or(EngineError::NoComp)?;
        for (lid, src) in &now {
            let Some(l) = comp.layer_mut(*lid) else { continue };
            if l.props.get("timeRemap").is_none() {
                l.props.children.insert(0, Node::Prop(remap_prop(&mut Ids(&mut next))));
            }
            let lt = if last { l.layer_time(l.out_point - fd) } else { l.layer_time(cti) };
            let lin = l.layer_time(l.in_point);
            let pr = l.props.get_mut("timeRemap").ok_or(EngineError::NoComp)?;
            pr.value = KV::Scalar(*src);
            if last {
                // Play normally up to the last frame, then hold it to the end of the comp.
                let mut end = Keyframe::new(lt.max(lin), KV::Scalar(*src));
                end.out_interp = effectcraft_keyframe::Interp::Hold;
                pr.keys = vec![Keyframe::new(lin.min(lt), KV::Scalar(lin.min(lt).seconds())), end];
                pr.keys.dedup_by_key(|k| k.time);
                l.out_point = cdur;
            } else {
                pr.keys = vec![Keyframe::new(lt, KV::Scalar(*src)).hold()];
            }
        }
        proj.next_id = next;
        Ok(())
    })?;
    Ok(Value::Null)
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "layer.timeStretch",
            "Time Stretch...",
            ["Layer", "Time"],
            None,
            "{layers?, percent?|duration? (s), hold?: in|current|out, op?: reverse}",
            has_layers,
            time_stretch
        ),
        cmd!("layer.stretchInToTime", "Stretch Layer In Point to Current Time", [], Some("Cmd+Shift+,"), "{layers?}", has_layers, |s, p| {
            let mut p = if p.is_object() { p.clone() } else { json!({}) };
            p["to"] = json!("in");
            time_stretch(s, &p)
        }),
        cmd!("layer.stretchOutToTime", "Stretch Layer Out Point to Current Time", [], Some("Cmd+Alt+,"), "{layers?}", has_layers, |s, p| {
            let mut p = if p.is_object() { p.clone() } else { json!({}) };
            p["to"] = json!("out");
            time_stretch(s, &p)
        }),
        cmd!("layer.timeReverse", "Time-Reverse Layer", ["Layer", "Time"], Some("Cmd+Alt+R"), "{layers?}", has_layers, |s, p| {
            let mut p = p.clone();
            p["op"] = json!("reverse");
            time_stretch(s, &p)
        }),
        cmd!("layer.enableTimeRemap", "Enable Time Remapping", ["Layer", "Time"], Some("Cmd+Alt+T"), "{layers?, value?}", has_layers, enable_time_remap),
        cmd!("layer.freezeFrame", "Freeze Frame", ["Layer", "Time"], None, "{layers?}", has_layers, |s, p| freeze(s, p, false)),
        cmd!("layer.freezeOnLastFrame", "Freeze On Last Frame", ["Layer", "Time"], None, "{layers?}", has_layers, |s, p| freeze(s, p, true)),
    ]
}

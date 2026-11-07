use effectcraft_keyframe::{Interp, Keyframe, Value as KV};
use effectcraft_project::Node;
use effectcraft_project::build::Ids;
use effectcraft_render::EvalCtx;
use effectcraft_time::Tick;
use serde_json::Value;

use super::{remap_prop, remappable};
use crate::cmd::{bad, layers_p};
use crate::{EngineError, Result, Session};

fn last_frame_keys(first_time: Tick, freeze_time: Tick, source: f64, reversed_start: Option<f64>) -> Vec<Keyframe> {
    if let Some(start_source) = reversed_start {
        let freeze_key = Keyframe::new(freeze_time, KV::Scalar(source));
        return if freeze_time < first_time { vec![freeze_key, Keyframe::new(first_time, KV::Scalar(start_source))] } else { vec![freeze_key.hold()] };
    }

    let start = first_time.min(freeze_time);
    let mut end = Keyframe::new(first_time.max(freeze_time), KV::Scalar(source));
    end.out_interp = Interp::Hold;
    let mut keys = vec![Keyframe::new(start, KV::Scalar(start.seconds())), end];
    keys.dedup_by_key(|key| key.time);
    keys
}

pub(super) fn freeze(s: &mut Session, p: &Value, last: bool) -> Result<Value> {
    let (cid, ids) = layers_p(s, p)?;
    let cti = s.time();
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    let (frame_duration, comp_duration) = (comp.frame_duration(), comp.duration);
    let context = EvalCtx {
        project: &s.project,
        comp_id: cid,
        comp,
        time: cti,
        expr: s.expr.as_deref(),
        footage: Some(s.footage.as_ref()),
        property_overrides: &[],
        media_overrides: &[],
    };
    let samples: Vec<_> = comp
        .layers
        .iter()
        .filter(|layer| ids.contains(&layer.id) && remappable(layer))
        .map(|layer| {
            let at = if last { layer.out_point - frame_duration } else { cti };
            let source = context.at(at).source_time(layer).seconds();
            let reversed_start = (last && layer.stretch < 0.0).then(|| context.at(layer.in_point).source_time(layer).seconds());
            (layer.id, layer.layer_time(at), source, reversed_start)
        })
        .collect();
    if samples.is_empty() {
        return Err(bad("layer.freezeFrame", "freezing needs a footage or composition layer"));
    }
    s.edit(if last { "Freeze On Last Frame" } else { "Freeze Frame" }, None, |proj, _| {
        let mut next = proj.next_id;
        let comp = proj.comp_mut(cid).ok_or(EngineError::NoComp)?;
        for (id, freeze_time, source, reversed_start) in samples {
            let Some(layer) = comp.layer_mut(id) else { continue };
            if layer.props.get("timeRemap").is_none() {
                layer.props.children.insert(0, Node::Prop(remap_prop(&mut Ids(&mut next))));
            }
            let first_time = layer.layer_time(layer.in_point);
            let property = layer.props.get_mut("timeRemap").ok_or(EngineError::NoComp)?;
            property.value = KV::Scalar(source);
            if last {
                property.keys = last_frame_keys(first_time, freeze_time, source, reversed_start);
                layer.out_point = comp_duration;
            } else {
                property.keys = vec![Keyframe::new(freeze_time, KV::Scalar(source)).hold()];
            }
        }
        proj.next_id = next;
        Ok(())
    })?;
    Ok(Value::Null)
}

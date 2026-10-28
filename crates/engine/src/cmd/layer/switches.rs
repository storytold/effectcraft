use super::{
    BlendMode, EngineError, FrameBlend, ItemId, KV, Layer, LayerId, MatteKind, Quality, Result, Session, TrackMatte, Value, b_p, bad, json, layer_mut, layer_p,
    layers_p, merge_p, resolve_layer, str_p,
};

pub(super) fn set_switch(s: &mut Session, p: &Value) -> Result<Value> {
    let (cid, ids) = layers_p(s, p)?;
    let name = str_p(p, "switch").ok_or_else(|| bad("layer.setSwitch", "missing `switch`"))?.to_string();
    let v = b_p(p, "value");
    let label = format!("Layer Switch ({name})");
    if b_p(p, "others") == Some(true) {
        return set_switch_others(s, cid, &ids, &name, v, &label);
    }
    // `merge`: a switch dragged over several layers in the Timeline is one undo step.
    let r = s.edit(&label, merge_p(p), |proj, _| {
        let comp = proj.comp_mut(cid).ok_or(EngineError::NoComp)?;
        let mut last = false;
        // Toggle relative to the first layer so a multi-selection ends up consistent.
        let first = comp.layers.iter().find(|l| ids.contains(&l.id)).map(|l| switch_value(l, &name)).unwrap_or(Some(false));
        let Some(first) = first else { return Err(bad("layer.setSwitch", format!("unknown switch `{name}`"))) };
        let target = v.unwrap_or(!first);
        for l in comp.layers.iter_mut().filter(|l| ids.contains(&l.id)) {
            set_switch_value(l, &name, target);
            last = target;
        }
        Ok(last)
    })?;
    Ok(json!(r))
}

/// `layer.setSwitch {others: true}` (Alt-click on an A/V feature in the Timeline): the named
/// layers stand alone, shown / heard / soloed / unlocked while every other layer the switch
/// applies to is the opposite; when they already do, the other layers come back. `value` sets
/// the other layers directly.
fn set_switch_others(s: &mut Session, cid: ItemId, ids: &[LayerId], name: &str, value: Option<bool>, label: &str) -> Result<Value> {
    let r = s.edit(label, None, |proj, _| {
        let comp = proj.comp_mut(cid).ok_or(EngineError::NoComp)?;
        // The state that sets a layer apart: unlocked for Lock, on for the rest.
        let active = !matches!(name, "lock" | "locked");
        // Track mattes keep their video off by design.
        let mattes: Vec<LayerId> =
            if matches!(name, "video" | "eye") { comp.layers.iter().filter_map(|l| l.track_matte.map(|m| m.layer)).collect() } else { Vec::new() };
        let applies = |l: &Layer| match name {
            "audio" => l.props.sub("audio").is_some(),
            "lock" | "locked" => true,
            // Every layer has a Video switch (it hides a null's outline in the viewer and turns a
            // camera or light off), as the Timeline's eye cell shows; track mattes keep theirs off
            // by design.
            "video" | "eye" => !mattes.contains(&l.id),
            _ => l.source.is_av() && !mattes.contains(&l.id),
        };
        let mut alone = true;
        for l in comp.layers.iter().filter(|l| applies(l)) {
            let Some(on) = switch_value(l, name) else { return Err(bad("layer.setSwitch", format!("unknown switch `{name}`"))) };
            alone &= on == (ids.contains(&l.id) == active);
        }
        let others = value.unwrap_or(alone == active);
        for l in comp.layers.iter_mut().filter(|l| applies(l)) {
            let v = if ids.contains(&l.id) { active } else { others };
            set_switch_value(l, name, v);
        }
        Ok(others)
    })?;
    Ok(json!(r))
}

fn switch_value(l: &Layer, name: &str) -> Option<bool> {
    let sw = &l.switches;
    Some(match name {
        "video" | "eye" => sw.video,
        "audio" => sw.audio,
        "solo" => sw.solo,
        "lock" | "locked" => sw.locked,
        "shy" => sw.shy,
        "collapse" => sw.collapse,
        "quality" => sw.quality == Quality::Best,
        "fx" | "effects" => sw.effects,
        "frameBlend" => sw.frame_blend != FrameBlend::Off,
        "motionBlur" => sw.motion_blur,
        "adjustment" => sw.adjustment,
        "threeD" | "3d" | "3D" => sw.three_d,
        "guide" => sw.guide,
        "preserveTransparency" => l.preserve_transparency,
        _ => return None,
    })
}

fn set_switch_value(l: &mut Layer, name: &str, v: bool) {
    let sw = &mut l.switches;
    match name {
        "video" | "eye" => sw.video = v,
        "audio" => sw.audio = v,
        "solo" => sw.solo = v,
        "lock" | "locked" => sw.locked = v,
        "shy" => sw.shy = v,
        "collapse" => sw.collapse = v,
        "quality" => sw.quality = if v { Quality::Best } else { Quality::Draft },
        "fx" | "effects" => sw.effects = v,
        "frameBlend" => sw.frame_blend = if v { FrameBlend::FrameMix } else { FrameBlend::Off },
        "motionBlur" => sw.motion_blur = v,
        "adjustment" => sw.adjustment = v,
        "threeD" | "3d" | "3D" => {
            if sw.three_d && !v {
                drop_3d_values(l);
            }
            l.switches.three_d = v;
        }
        "guide" => sw.guide = v,
        "preserveTransparency" => l.preserve_transparency = v,
        _ => {}
    }
}

/// Turning the 3D switch off discards Z values, Orientation and X/Y Rotation (as in AE).
fn drop_3d_values(l: &mut Layer) {
    let Some(tr) = l.transform_mut() else { return };
    for (m, keep) in [("position", 0.0), ("anchor", 0.0), ("scale", 100.0)] {
        if let Some(pr) = tr.get_mut(m) {
            let flat = |v: &mut KV| {
                if let KV::Vec3(a) = v {
                    a[2] = keep;
                }
            };
            flat(&mut pr.value);
            for k in &mut pr.keys {
                flat(&mut k.value);
            }
        }
    }
    for (m, zero) in [("orientation", KV::Vec3([0.0; 3])), ("rotationX", KV::Scalar(0.0)), ("rotationY", KV::Scalar(0.0))] {
        if let Some(pr) = tr.get_mut(m) {
            pr.keys.clear();
            pr.expr = None;
            pr.value = zero;
        }
    }
}

pub(super) fn rename(s: &mut Session, p: &Value) -> Result<Value> {
    let (cid, lid) = layer_p(s, p, "layer.rename")?;
    let name = str_p(p, "name").ok_or_else(|| bad("layer.rename", "missing `name`"))?.to_string();
    s.edit("Rename Layer", None, |proj, _| {
        layer_mut(proj, cid, lid)?.name = name.clone();
        Ok(())
    })?;
    Ok(Value::Null)
}

/// The Timeline's Comment column.
pub(super) fn set_comment(s: &mut Session, p: &Value) -> Result<Value> {
    let (cid, ids) = layers_p(s, p)?;
    let text = str_p(p, "comment").ok_or_else(|| bad("layer.setComment", "missing `comment`"))?.to_string();
    s.edit("Layer Comment", None, |proj, _| {
        let comp = proj.comp_mut(cid).ok_or(EngineError::NoComp)?;
        for l in comp.layers.iter_mut().filter(|l| ids.contains(&l.id)) {
            l.comment = text.clone();
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

pub(super) fn blend(s: &mut Session, p: &Value) -> Result<Value> {
    let (cid, ids) = layers_p(s, p)?;
    let mode = str_p(p, "mode").and_then(BlendMode::from_name);
    let step = p.get("step").and_then(Value::as_i64).unwrap_or(0);
    if mode.is_none() && step == 0 {
        return Err(bad("layer.setBlendMode", "need `mode` (e.g. Multiply) or `step` ±1"));
    }
    s.edit("Blending Mode", None, |proj, _| {
        let comp = proj.comp_mut(cid).ok_or(EngineError::NoComp)?;
        for l in comp.layers.iter_mut().filter(|l| ids.contains(&l.id)) {
            l.blend_mode = match mode {
                Some(m) => m,
                None if step > 0 => l.blend_mode.next(),
                None => l.blend_mode.previous(),
            };
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

pub(super) fn track_matte(s: &mut Session, p: &Value) -> Result<Value> {
    let (cid, lid) = layer_p(s, p, "layer.setTrackMatte")?;
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    let matte = match p.get("matte") {
        None | Some(Value::Null) => None,
        Some(v) => Some(resolve_layer(comp, v).ok_or_else(|| bad("layer.setTrackMatte", "no such matte layer"))?),
    };
    let kind = str_p(p, "kind")
        .map(|k| match k.to_ascii_lowercase().replace([' ', '_', '-'], "").as_str() {
            "alphainverted" | "alphainv" => MatteKind::AlphaInverted,
            "luma" => MatteKind::Luma,
            "lumainverted" | "lumainv" => MatteKind::LumaInverted,
            _ => MatteKind::Alpha,
        })
        .unwrap_or(MatteKind::Alpha);
    s.edit("Track Matte", None, |proj, _| {
        let comp = proj.comp_mut(cid).ok_or(EngineError::NoComp)?;
        if let Some(m) = matte {
            if m == lid {
                return Err(bad("layer.setTrackMatte", "a layer can't be its own matte"));
            }
            if let Some(ml) = comp.layer_mut(m) {
                ml.switches.video = false;
            }
        }
        comp.layer_mut(lid).ok_or(EngineError::NoComp)?.track_matte = matte.map(|layer| TrackMatte { layer, kind });
        Ok(())
    })?;
    Ok(Value::Null)
}

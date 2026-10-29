use super::{
    EngineError, ItemKind, Label, Layer, LayerId, LayerSource, Result, Session, Tick, Value, b_p, bad, build, f_p, json, layer_mut, layers_p, merge_p,
    resolve_layer, str_p, unlocked, with,
};

pub(super) fn timing(s: &mut Session, p: &Value) -> Result<Value> {
    let (cid, ids) = layers_p(s, p)?;
    let t = s.time();
    let op = str_p(p, "op").unwrap_or("set").to_string();
    // Layer times stay on frame boundaries of the comp (as in AE).
    let fr = s.project.comp(cid).ok_or(EngineError::NoComp)?.frame_rate;
    let snap = |k: &str| f_p(p, k).map(|v| fr.snap_nearest(Tick::from_seconds_f64(v)));
    let (delta, start, inp, outp) = (snap("delta"), snap("start"), snap("in"), snap("out"));
    // Relative trims: each layer moves its own in/out point by the same amount, so dragging an
    // edge of several selected layers trims them all together (as in AE).
    let (in_delta, out_delta) = (snap("inDelta"), snap("outDelta"));
    s.edit("Layer Timing", merge_p(p), |proj, _| {
        let comp = proj.comp_mut(cid).ok_or(EngineError::NoComp)?;
        let fd = comp.frame_duration();
        let comp_dur = comp.duration;
        for l in comp.layers.iter_mut().filter(|l| ids.contains(&l.id)) {
            let shift = |l: &mut Layer, d: Tick| {
                l.start_time += d;
                l.in_point += d;
                l.out_point += d;
            };
            match op.as_str() {
                "moveInToTime" => {
                    let d = t - l.in_point;
                    shift(l, d);
                }
                "moveOutToTime" => {
                    let d = t - l.out_point;
                    shift(l, d);
                }
                "moveInToStart" => {
                    let d = Tick::ZERO - l.in_point;
                    shift(l, d);
                }
                "moveOutToEnd" => {
                    let d = comp_dur - l.out_point;
                    shift(l, d);
                }
                "trimInToTime" => l.in_point = t.min(l.out_point - fd),
                "trimOutToTime" => l.out_point = t.max(l.in_point + fd),
                _ => {
                    if let Some(d) = delta {
                        shift(l, d);
                    }
                    if let Some(st) = start {
                        let d = st - l.start_time;
                        shift(l, d);
                    }
                    if let Some(i) = inp {
                        l.in_point = i.min(l.out_point - fd);
                    }
                    if let Some(o) = outp {
                        l.out_point = o.max(l.in_point + fd);
                    }
                    // Stops at the comp start/end (a point already past it doesn't jump onto
                    // it), keeping at least one frame of layer.
                    if let Some(d) = in_delta {
                        l.in_point = (l.in_point + d).max(Tick::ZERO.min(l.in_point)).min(l.out_point - fd);
                    }
                    if let Some(d) = out_delta {
                        l.out_point = (l.out_point + d).min(comp_dur.max(l.out_point)).max(l.in_point + fd);
                    }
                }
            }
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Move the layers `frames` frames in time (Alt+Page Up / Page Down, with Shift ten frames).
pub(super) fn move_frames(s: &mut Session, p: &Value, frames: i64) -> Result<Value> {
    let (cid, _) = layers_p(s, p)?;
    let delta = s.project.comp(cid).ok_or(EngineError::NoComp)?.frame_rate.tick_of(frames).seconds();
    timing(s, &with(p, "delta", json!(delta)))
}

/// Slip edit: move a layer's source (footage or precomp) under its fixed in and out points, like
/// dragging the source-duration bar in After Effects. `delta` in seconds or `frames`; positive
/// moves the source later. Clamped so the source still covers the in–out span. Keyframes and
/// layer markers are in layer time, so they travel with the source.
pub(super) fn slip(s: &mut Session, p: &Value) -> Result<Value> {
    let (cid, ids) = layers_p(s, p)?;
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    let fr = comp.frame_rate;
    let delta = match (p.get("frames").and_then(Value::as_i64), f_p(p, "delta")) {
        (Some(f), _) => fr.tick_of(f),
        (None, Some(d)) => fr.snap_nearest(Tick::from_seconds_f64(d)),
        _ => return Err(bad("layer.slip", "delta (seconds) or frames required")),
    };
    // Allowed shift per layer: the source span [lo, hi] in comp time must keep covering in..out.
    let mut shifts: Vec<(LayerId, Tick)> = Vec::new();
    for l in comp.layers.iter().filter(|l| ids.contains(&l.id)) {
        if l.props.get("timeRemap").is_some() {
            continue;
        }
        let dur = match &l.source {
            LayerSource::Footage { item } => match s.project.item(*item).map(|i| &i.kind) {
                Some(ItemKind::Footage(f)) if f.kind != effectcraft_project::FootageKind::Still => Some(f.looped_duration()),
                _ => None,
            },
            LayerSource::Comp { item } => s.project.comp(*item).map(|c| c.duration),
            _ => None,
        };
        let Some(dur) = dur else { continue };
        let (a, b) = (l.comp_time(Tick::ZERO), l.comp_time(dur));
        let (lo, hi) = (a.min(b), a.max(b));
        let (min_d, max_d) = (l.out_point - hi, l.in_point - lo);
        if min_d > max_d {
            continue;
        }
        shifts.push((l.id, delta.max(min_d).min(max_d)));
    }
    if shifts.is_empty() {
        return Err(bad("layer.slip", "select a footage or precomp layer (without time remapping)"));
    }
    s.edit("Slip Edit", merge_p(p), |proj, _| {
        let comp = proj.comp_mut(cid).ok_or(EngineError::NoComp)?;
        for (id, d) in &shifts {
            if let Some(l) = comp.layer_mut(*id) {
                l.start_time += *d;
            }
        }
        Ok(())
    })?;
    Ok(json!({"slipped": shifts.iter().map(|(id, d)| json!({"layer": id.0, "delta": d.seconds()})).collect::<Vec<_>>()}))
}

pub(super) fn arrange(s: &mut Session, p: &Value) -> Result<Value> {
    let (cid, ids) = layers_p(s, p)?;
    let ids = unlocked(s, cid, ids, "layer.arrange")?;
    let how = str_p(p, "to").unwrap_or("front").to_string();
    let index = p.get("index").and_then(Value::as_u64).map(|i| i as usize);
    // `above`: put the layers right above that one (the timeline's drag to reorder).
    let above = match p.get("above") {
        Some(v) => {
            let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
            let a = resolve_layer(comp, v).ok_or_else(|| bad("layer.arrange", "no such layer for `above`"))?;
            if ids.contains(&a) {
                return Err(bad("layer.arrange", "`above` must be a layer that isn't being moved"));
            }
            Some(a)
        }
        None => None,
    };
    s.edit("Arrange Layers", None, |proj, _| {
        let comp = proj.comp_mut(cid).ok_or(EngineError::NoComp)?;
        // Bring Forward / Send Backward move each layer one step past the next layer that isn't
        // moving (a run of selected layers moves as one; one already at the end stays).
        if above.is_none() && index.is_none() && matches!(how.as_str(), "forward" | "backward") {
            let picked = |l: &Layer| ids.contains(&l.id);
            let n = comp.layers.len();
            if how == "forward" {
                for i in 1..n {
                    if picked(&comp.layers[i]) && !picked(&comp.layers[i - 1]) {
                        let run_end = (i..n).find(|j| !picked(&comp.layers[*j])).unwrap_or(n);
                        comp.layers[i - 1..run_end].rotate_left(1);
                    }
                }
            } else {
                for i in (0..n.saturating_sub(1)).rev() {
                    if picked(&comp.layers[i]) && !picked(&comp.layers[i + 1]) {
                        let run_start = (0..=i).rev().find(|j| !picked(&comp.layers[*j])).map_or(0, |j| j + 1);
                        comp.layers[run_start..=i + 1].rotate_right(1);
                    }
                }
            }
            return Ok(());
        }
        let mut picked: Vec<Layer> = Vec::new();
        comp.layers.retain(|l| {
            if ids.contains(&l.id) {
                picked.push(l.clone());
                false
            } else {
                true
            }
        });
        let at = match (how.as_str(), index) {
            _ if above.is_some() => comp.layers.iter().position(|l| Some(l.id) == above).unwrap_or(comp.layers.len()),
            (_, Some(i)) => i.saturating_sub(1).min(comp.layers.len()),
            ("back", _) => comp.layers.len(),
            _ => 0,
        };
        for (k, l) in picked.into_iter().enumerate() {
            comp.layers.insert((at + k).min(comp.layers.len()), l);
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Layer ▸ Pre-compose. `mode: move` (default; "Move all attributes into the new composition")
/// nests the selected layers as they are; `mode: leave` ("Leave all attributes in", one footage,
/// solid or precomp layer) nests only the layer's source, sized to it, and keeps the layer's
/// transform, masks, effects and timing in the current comp. `adjustDuration` (move) trims the
/// new comp to the selected layers' span; `open` opens the new comp.
pub(super) fn precompose(s: &mut Session, p: &Value) -> Result<Value> {
    let c = "layer.precompose";
    let (cid, ids) = layers_p(s, p)?;
    if ids.is_empty() {
        return Err(bad(c, "no layers"));
    }
    let name = str_p(p, "name").unwrap_or("Pre-comp 1").to_string();
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?.clone();
    let leave = match str_p(p, "mode").unwrap_or("move") {
        "move" => false,
        "leave" => true,
        x => return Err(bad(c, format!("mode: move|leave, got `{x}`"))),
    };
    let adjust = b_p(p, "adjustDuration").unwrap_or(false);
    if leave {
        let [lid] = ids[..] else { return Err(bad(c, "\"Leave all attributes\" needs exactly one layer")) };
        let layer = comp.layer(lid).ok_or(EngineError::NoComp)?.clone();
        let Some(src_item) = layer.source.item() else {
            return Err(bad(c, "\"Leave all attributes\" is available for footage, solid and precomp layers only"));
        };
        let (w, h) = effectcraft_render::source_size(&s.project, &layer);
        let (w, h) = if w == 0 || h == 0 { (comp.width, comp.height) } else { (w, h) };
        let dur = match s.project.item(src_item).map(|i| &i.kind) {
            Some(ItemKind::Comp(nc)) => nc.duration,
            Some(ItemKind::Footage(f)) if f.duration > Tick(0) => f.duration,
            _ => comp.duration,
        };
        let new = s.edit("Pre-compose", None, |proj, st| {
            let mut inner = comp.nested_like(w, h, dur);
            let mut il = build::layer(proj, &inner, &layer.name, layer.source.clone(), (w, h), Some(dur));
            il.name = layer.name.clone();
            inner.layers.push(il);
            let iid = proj.add_item(&name, Label::Sandstone, None, ItemKind::Comp(inner.into()));
            let l = layer_mut(proj, cid, lid)?;
            l.source = LayerSource::Comp { item: iid };
            l.name = name.clone();
            st.selected_layers = vec![lid];
            Ok((iid, lid))
        })?;
        if b_p(p, "open").unwrap_or(false) {
            s.open_comp(new.0);
        }
        return Ok(json!({"comp": new.0.0, "layer": new.1.0}));
    }
    let span = {
        let sel: Vec<&effectcraft_project::Layer> = comp.layers.iter().filter(|l| ids.contains(&l.id)).collect();
        let a = sel.iter().map(|l| l.in_point).min().unwrap_or(Tick(0)).max(Tick(0));
        let b = sel.iter().map(|l| l.out_point).max().unwrap_or(comp.duration).min(comp.duration);
        (a, b)
    };
    let (offset, dur) = if adjust && span.1 > span.0 { (span.0, span.1 - span.0) } else { (Tick(0), comp.duration) };
    let new = s.edit("Pre-compose", None, |proj, st| {
        let mut inner = comp.nested_like(comp.width, comp.height, dur);
        inner.layers = comp.layers.iter().filter(|l| ids.contains(&l.id)).cloned().collect();
        for l in &mut inner.layers {
            if l.parent.is_some_and(|p| !ids.contains(&p)) {
                l.parent = None;
            }
            if l.track_matte.is_some_and(|m| !ids.contains(&m.layer)) {
                l.track_matte = None;
            }
            l.start_time -= offset;
            l.in_point -= offset;
            l.out_point -= offset;
        }
        let iid = proj.add_item(&name, Label::Sandstone, None, ItemKind::Comp(inner.into()));
        let mut l = build::layer(proj, &comp, &name, LayerSource::Comp { item: iid }, (comp.width, comp.height), Some(dur));
        l.name = name.clone();
        l.start_time = offset;
        l.in_point = offset;
        l.out_point = offset + dur;
        let c = proj.comp_mut(cid).ok_or(EngineError::NoComp)?;
        let at = c.layers.iter().position(|l| ids.contains(&l.id)).unwrap_or(0);
        c.layers.retain(|l| !ids.contains(&l.id));
        let lid = l.id;
        c.layers.insert(at.min(c.layers.len()), l);
        st.selected_layers = vec![lid];
        Ok((iid, lid))
    })?;
    if b_p(p, "open").unwrap_or(false) {
        s.open_comp(new.0);
    }
    Ok(json!({"comp": new.0.0, "layer": new.1.0}))
}

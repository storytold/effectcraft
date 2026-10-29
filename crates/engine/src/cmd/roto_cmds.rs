//! Roto Brush & Refine Edge commands: strokes (the Roto Brush and Refine Edge tools in the Layer
//! panel), propagation, the segmentation span, Freeze / Unfreeze, tool options and status.
//!
//! `objectMatte.apply` adds an Object Matte effect (Effect ▸ Matte) from a point / box prompt on
//! a frame; it segments with the same engine and model as Roto Brush.
//!
//! `roto.stroke` records a stroke into the layer's Roto Brush & Refine Edge effect (applying the
//! effect on the first stroke, as After Effects does); the first foreground stroke sets the base
//! frame and a span of 20 frames each side. Strokes on a frozen effect are refused (Unfreeze
//! first). Every edit is one undo step.

use effectcraft_effects::roto::{self as fx, FROZEN, Stroke, StrokeKind};
use effectcraft_keyframe::Value as KV;
use effectcraft_project::build::Ids;
use effectcraft_project::{ItemId, Layer, LayerId, PropGroup, Uid};
use effectcraft_track::roto::{RotoData, iou, rle};
use serde_json::{Value, json};

use super::{CommandSpec, always, b_p, bad, f_p, has_comp, layer_mut, layer_p, str_p};
use crate::roto::{self as rt, Direction, RotoOptions, RotoTask, VIEWS, is_roto, params_static};
use crate::{EngineError, Result, Session, cmd, query};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "roto.stroke",
            "Roto Brush Stroke",
            [],
            None,
            "{layer?, kind?: fg|bg|refine|refineErase, points: [[x, y], …] (layer pixels), frame? (layer frame; default current), radius? (layer px; default the tool's diameter / 2), effect?}",
            has_comp,
            stroke
        ),
        cmd!(
            "roto.propagate",
            "Propagate Roto Brush",
            [],
            None,
            "{layer?, effect?, direction?: forward|backward|both, to? (layer frame: extend the span), wait?: block until done}",
            has_comp,
            propagate
        ),
        cmd!("roto.span", "Set Segmentation Span", [], None, "{layer?, effect?, start?, end? (layer frames)}", has_comp, span),
        cmd!("roto.freeze", "Freeze", [], None, "{layer?, effect?, wait?}", has_comp, freeze),
        cmd!("roto.unfreeze", "Unfreeze", [], None, "{layer?, effect?}", has_comp, unfreeze),
        cmd!("roto.clearStrokes", "Remove Roto Brush Strokes", [], None, "{layer?, effect?, frame? (only this layer frame), kind?}", has_comp, clear_strokes),
        cmd!("roto.cancel", "Stop Roto Brush", [], None, "{}", is_running, |s, _| Ok(json!({"stopped": s.stop_roto()}))),
        cmd!(
            "roto.options",
            "Roto Brush Options",
            [],
            None,
            "{diameter?, refineDiameter?, view?: alphaBoundary|alpha|alphaOverlay|none, overlayColor?: [r,g,b], overlayOpacity?, boundaryColor?: [r,g,b], autoPropagate?}",
            always,
            options
        ),
        cmd!(
            "objectMatte.apply",
            "Object Matte",
            [],
            None,
            "{layer?, effect? (uid: re-prompt this Object Matte instead of adding one), point: [x, y] (layer pixels: the object), box?: [x0, y0, x1, y1] (layer pixels, around the object), frame? (reference layer frame; default current), track?: follow the object from that frame (default true), before?, after? (frames to follow it; default 30, up to 600)}",
            has_comp,
            object_matte_apply
        ),
        cmd!(
            "objectMatte.propagate",
            "Track Object Matte",
            [],
            None,
            "{layer?, effect?, direction?: forward|backward|both, wait?: block until done} (a background job like Roto Brush propagation; roto.cancel stops it)",
            has_comp,
            object_matte_propagate
        ),
        cmd!("objectMatte.freeze", "Freeze Object Matte", [], None, "{layer?, effect?, wait?}", has_comp, object_matte_freeze),
        cmd!("objectMatte.unfreeze", "Unfreeze Object Matte", [], None, "{layer?, effect?}", has_comp, object_matte_unfreeze),
        query!(
            "objectMatte.status",
            "Object Matte Status",
            "{layer?, effect?}: the job's progress, whether the instance is frozen, its frames and which are computed",
            object_matte_status
        ),
        query!(
            "roto.status",
            "Roto Brush Status",
            "{layer?, effect?, frame? (layer frame: also report its matte), matte?: include the matte (RLE, base64), compute?: compute the frame if needed, compareTo?: RLE matte to report the IoU against}",
            status
        ),
    ]
}

fn is_running(s: &Session) -> std::result::Result<(), String> {
    if s.is_roto_running() { Ok(()) } else { Err("no Roto Brush job is running".into()) }
}

/// The Roto Brush instance targeted by `p`: `layer` / `effect` (uid, name or 1-based index),
/// else a selected one, else the first on the selected (or any) layer.
pub(crate) fn roto_p(s: &Session, p: &Value, cmd: &str) -> Result<(ItemId, LayerId, Uid)> {
    instance_lookup(s, p, cmd, is_roto)?.ok_or_else(|| bad(cmd, "no Roto Brush & Refine Edge effect found (paint a stroke with the Roto Brush tool first)"))
}

/// The Object Matte instance targeted by `p` (as [`roto_p`]).
fn object_matte_p(s: &Session, p: &Value, cmd: &str) -> Result<(ItemId, LayerId, Uid)> {
    instance_lookup(s, p, cmd, rt::is_object_matte)?.ok_or_else(|| bad(cmd, "no Object Matte effect found (click an object with the Object Matte tool first)"))
}

/// The instance of a kind (`is`) targeted by `p`; `None` when there is none.
fn instance_lookup(s: &Session, p: &Value, cmd: &str, is: fn(&PropGroup) -> bool) -> Result<Option<(ItemId, LayerId, Uid)>> {
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
        let Some(fxg) = comp.layer(lid).and_then(Layer::effects) else { continue };
        let g = match p.get("effect") {
            Some(Value::Number(n)) => {
                let n = n.as_u64().unwrap_or(0);
                fxg.groups().find(|g| g.uid == n).or_else(|| fxg.groups().nth((n as usize).saturating_sub(1)))
            }
            Some(Value::String(name)) => fxg.groups().find(|g| &g.name == name),
            _ => s
                .state
                .selected_props
                .iter()
                .find_map(|(l, u)| (*l == lid).then(|| fxg.groups().find(|g| g.uid == *u && is(g))).flatten())
                .or_else(|| fxg.groups().find(|g| is(g))),
        };
        if let Some(g) = g.filter(|g| is(g)) {
            return Ok(Some((cid, lid, g.uid)));
        }
        if p.get("layer").is_some() {
            break;
        }
    }
    Ok(None)
}

fn group(s: &Session, cid: ItemId, lid: LayerId, uid: Uid) -> Result<(&Layer, &PropGroup)> {
    let l = s.project.comp(cid).and_then(|c| c.layer(lid)).ok_or(EngineError::NoComp)?;
    let g = l.props.find_group(uid).ok_or_else(|| EngineError::Other("the effect is gone".into()))?;
    Ok((l, g))
}

fn data_of(g: &PropGroup) -> RotoData {
    fx::data(&params_static(g)).as_ref().clone()
}

fn frozen_of(g: &PropGroup) -> bool {
    matches!(g.get(FROZEN).map(|p| &p.value), Some(KV::Str(s)) if !s.is_empty())
}

/// Write an instance's data (one undo step).
fn write_data(s: &mut Session, label: &str, cid: ItemId, lid: LayerId, uid: Uid, d: &RotoData) -> Result<()> {
    let j = d.to_json();
    s.edit(label, None, |proj, _| {
        let g = layer_mut(proj, cid, lid)?.props.find_group_mut(uid).ok_or_else(|| EngineError::Other("the effect is gone".into()))?;
        if let Some(pr) = g.get_mut(fx::STROKES) {
            pr.value = KV::Str(j);
        }
        Ok(())
    })
}

/// Current layer frame of `layer` (or `frame` from the params).
fn frame_p(s: &Session, p: &Value, cid: ItemId, layer: &Layer) -> i64 {
    if let Some(f) = f_p(p, "frame") {
        return f.round() as i64;
    }
    let comp = s.project.comp(cid);
    let fps = comp.map(|c| c.frame_rate.as_f64()).unwrap_or(30.0);
    fx::frame_of(layer.layer_time(s.time_of(cid)).seconds(), fps)
}

fn points_p(p: &Value, cmd: &str) -> Result<Vec<[f64; 2]>> {
    let raw = p.get("points").and_then(Value::as_array).ok_or_else(|| bad(cmd, "missing `points` [[x, y], …] (layer pixels)"))?;
    let mut v = vec![];
    for q in raw {
        let a = q.as_array().ok_or_else(|| bad(cmd, "each point is [x, y]"))?;
        let (Some(x), Some(y)) = (a.first().and_then(Value::as_f64), a.get(1).and_then(Value::as_f64)) else {
            return Err(bad(cmd, "each point is [x, y]"));
        };
        v.push([x, y]);
    }
    if v.is_empty() {
        return Err(bad(cmd, "`points` is empty"));
    }
    Ok(v)
}

fn stroke(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "roto.stroke";
    let (cid, lid) = match roto_p(s, p, cmd) {
        Ok((c, l, _)) => (c, l),
        Err(_) => layer_p(s, p, cmd)?,
    };
    let kind = match str_p(p, "kind") {
        Some(k) => StrokeKind::from_name(k).ok_or_else(|| bad(cmd, format!("unknown kind `{k}` (fg | bg | refine | refineErase)")))?,
        None => StrokeKind::Fg,
    };
    let points = points_p(p, cmd)?;
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    let layer = comp.layer(lid).ok_or_else(|| bad(cmd, "no layer"))?;
    if !layer.has_video() || layer.switches.adjustment || layer.effects().is_none() {
        return Err(bad(cmd, "the Roto Brush works on footage, solid and composition layers"));
    }
    let frame = frame_p(s, p, cid, layer);
    let limits = rt::frame_limits(comp, layer);
    if frame < limits[0] || frame > limits[1] {
        return Err(bad(cmd, format!("frame {frame} is outside the layer ({}–{})", limits[0], limits[1])));
    }
    let o = &s.state.roto;
    let def_r = if matches!(kind, StrokeKind::Refine | StrokeKind::RefineErase) { o.refine_diameter / 2.0 } else { o.diameter / 2.0 };
    let radius = f_p(p, "radius").unwrap_or(def_r).clamp(0.5, 2500.0);
    let size = rt::layer_size(&s.project, comp, layer);
    // The instance: the requested / first one, else a new one.
    let existing = roto_p(s, p, cmd).ok().filter(|(c, l, _)| *c == cid && *l == lid).map(|x| x.2);
    if let Some(uid) = existing {
        let (_, g) = group(s, cid, lid, uid)?;
        if frozen_of(g) {
            return Err(bad(cmd, "the Roto Brush is frozen: Unfreeze it to edit strokes"));
        }
    }
    if existing.is_none() && !kind.is_segmentation() {
        return Err(bad(cmd, "draw a foreground stroke with the Roto Brush tool first"));
    }
    let st = Stroke { kind, frame, radius, points };
    let label = if kind.is_segmentation() { "Roto Brush Stroke" } else { "Refine Edge Stroke" };
    let (uid, d) = s.edit(label, None, |proj, _| {
        let mut next = proj.next_id;
        let l = layer_mut(proj, cid, lid)?;
        let fxg = l.props.sub_mut("effects").ok_or_else(|| bad(cmd, "this layer has no effects"))?;
        let uid = match existing {
            Some(u) => u,
            None => {
                let spec = effectcraft_effects::find(fx::ID).ok_or_else(|| EngineError::Other("Roto Brush effect missing".into()))?;
                let n = fxg.groups().filter(|g| is_roto(g)).count();
                let name = if n == 0 { spec.name.to_string() } else { format!("{} {}", spec.name, n + 1) };
                let g = effectcraft_effects::instantiate(spec, &mut Ids(&mut next), &name, size);
                let u = g.uid;
                fxg.children.push(g.into());
                u
            }
        };
        let g = fxg.find_group_mut(uid).ok_or_else(|| bad(cmd, "effect gone"))?;
        let mut d = data_of(g);
        d.add(st, fx::DEFAULT_SPAN, limits);
        if let Some(pr) = g.get_mut(fx::STROKES) {
            pr.value = KV::Str(d.to_json());
        }
        proj.next_id = proj.next_id.max(next);
        Ok((uid, d))
    })?;
    if !s.state.selected_props.iter().any(|(l, u)| *l == lid && *u == uid) {
        s.state.selected_layers = vec![lid];
    }
    Ok(json!({
        "layer": lid.0, "effect": uid, "frame": frame, "kind": kind, "radius": radius,
        "base": d.base, "span": d.span, "strokes": d.strokes.len(),
    }))
}

fn propagate(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "roto.propagate";
    let (cid, lid, uid) = roto_p(s, p, cmd)?;
    let dir = match str_p(p, "direction") {
        Some(d) => Direction::from_name(d).ok_or_else(|| bad(cmd, format!("unknown direction `{d}` (forward | backward | both)")))?,
        None => Direction::Both,
    };
    if let Some(to) = f_p(p, "to") {
        let to = to.round() as i64;
        let (l, g) = group(s, cid, lid, uid)?;
        let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
        let lim = rt::frame_limits(comp, l);
        let mut d = data_of(g);
        let to = to.clamp(lim[0], lim[1]);
        let before = d.span;
        d.span[0] = d.span[0].min(to);
        d.span[1] = d.span[1].max(to);
        if d.span != before {
            if frozen_of(g) {
                return Err(bad(cmd, "the Roto Brush is frozen: Unfreeze it to change the span"));
            }
            write_data(s, "Extend Segmentation Span", cid, lid, uid, &d)?;
        }
    }
    let wait = b_p(p, "wait").unwrap_or(false);
    let n = s.start_roto(cid, lid, uid, RotoTask::Propagate, dir, wait).map_err(|e| bad(cmd, e))?;
    let mut out = json!({"layer": lid.0, "effect": uid, "frames": n, "running": s.is_roto_running(), "progress": s.roto_progress()});
    if wait {
        out["status"] = status(s, &json!({"layer": lid.0, "effect": uid, "comp": cid.0}))?;
    }
    Ok(out)
}

fn span(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "roto.span";
    let (cid, lid, uid) = roto_p(s, p, cmd)?;
    let (l, g) = group(s, cid, lid, uid)?;
    if frozen_of(g) {
        return Err(bad(cmd, "the Roto Brush is frozen: Unfreeze it to change the span"));
    }
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    let [lo, hi] = rt::frame_limits(comp, l);
    let mut d = data_of(g);
    let base = d.base.ok_or_else(|| bad(cmd, "no strokes yet"))?;
    let first = d.strokes.iter().map(|s| s.frame).min().unwrap_or(base);
    let last = d.strokes.iter().map(|s| s.frame).max().unwrap_or(base);
    // Trimming or shortening the comp can leave saved strokes outside the layer. No span
    // inside its new bounds can contain those strokes; keep them for undo or a later untrim.
    if first < lo || last > hi {
        return Err(bad(cmd, "strokes outside the layer: restore the layer/comp timing or clear those strokes before changing the span"));
    }
    let [start, end] = &mut d.span;
    if let Some(a) = f_p(p, "start") {
        *start = (a.round() as i64).clamp(lo, first);
    }
    if let Some(b) = f_p(p, "end") {
        *end = (b.round() as i64).clamp(last, hi);
    }
    write_data(s, "Segmentation Span", cid, lid, uid, &d)?;
    Ok(json!({"layer": lid.0, "effect": uid, "span": d.span, "base": base}))
}

fn freeze(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "roto.freeze";
    let (cid, lid, uid) = roto_p(s, p, cmd)?;
    let wait = b_p(p, "wait").unwrap_or(false);
    let n = s.start_roto(cid, lid, uid, RotoTask::Freeze, Direction::Both, wait).map_err(|e| bad(cmd, e))?;
    let (_, g) = group(s, cid, lid, uid)?;
    Ok(json!({"layer": lid.0, "effect": uid, "frames": n, "running": s.is_roto_running(), "frozen": frozen_of(g), "progress": s.roto_progress()}))
}

fn unfreeze(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "roto.unfreeze";
    let (cid, lid, uid) = roto_p(s, p, cmd)?;
    let (_, g) = group(s, cid, lid, uid)?;
    if !frozen_of(g) {
        return Ok(json!({"layer": lid.0, "effect": uid, "frozen": false}));
    }
    s.edit("Unfreeze", None, |proj, _| {
        let g = layer_mut(proj, cid, lid)?.props.find_group_mut(uid).ok_or_else(|| EngineError::Other("the effect is gone".into()))?;
        if let Some(pr) = g.get_mut(FROZEN) {
            pr.value = KV::Str(String::new());
        }
        Ok(())
    })?;
    Ok(json!({"layer": lid.0, "effect": uid, "frozen": false}))
}

fn clear_strokes(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "roto.clearStrokes";
    let (cid, lid, uid) = roto_p(s, p, cmd)?;
    let (_, g) = group(s, cid, lid, uid)?;
    if frozen_of(g) {
        return Err(bad(cmd, "the Roto Brush is frozen: Unfreeze it to edit strokes"));
    }
    let mut d = data_of(g);
    let frame = f_p(p, "frame").map(|f| f.round() as i64);
    let kind = match str_p(p, "kind") {
        Some(k) => Some(StrokeKind::from_name(k).ok_or_else(|| bad(cmd, format!("unknown kind `{k}`")))?),
        None => None,
    };
    let n0 = d.strokes.len();
    d.strokes.retain(|st| !(frame.is_none_or(|f| st.frame == f) && kind.is_none_or(|k| st.kind == k)));
    if d.strokes.iter().all(|s| !s.kind.is_segmentation()) {
        d.base = None;
    } else if d.base.is_some_and(|b| !d.strokes.iter().any(|s| s.frame == b && s.kind.is_segmentation())) {
        // The base moves to the earliest remaining stroked frame.
        d.base = d.strokes.iter().filter(|s| s.kind.is_segmentation()).map(|s| s.frame).min();
    }
    let removed = n0 - d.strokes.len();
    if removed > 0 {
        write_data(s, "Remove Roto Brush Strokes", cid, lid, uid, &d)?;
    }
    Ok(json!({"layer": lid.0, "effect": uid, "removed": removed, "strokes": d.strokes.len(), "base": d.base}))
}

fn options(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "roto.options";
    let mut o: RotoOptions = s.state.roto.clone();
    for (k, f) in [("diameter", &mut o.diameter as &mut f64), ("refineDiameter", &mut o.refine_diameter), ("overlayOpacity", &mut o.overlay_opacity)] {
        if let Some(v) = f_p(p, k) {
            *f = v;
        }
    }
    o.diameter = o.diameter.clamp(1.0, 5000.0);
    o.refine_diameter = o.refine_diameter.clamp(1.0, 5000.0);
    o.overlay_opacity = o.overlay_opacity.clamp(0.0, 100.0);
    if let Some(v) = str_p(p, "view") {
        let v = VIEWS.iter().find(|x| x.eq_ignore_ascii_case(v)).ok_or_else(|| bad(cmd, format!("unknown view `{v}` ({})", VIEWS.join(" | "))))?;
        o.view = v.to_string();
    }
    for (k, c) in [("overlayColor", &mut o.overlay_color), ("boundaryColor", &mut o.boundary_color)] {
        if let Some(a) = p.get(k).and_then(Value::as_array) {
            if a.len() < 3 {
                return Err(bad(cmd, format!("`{k}` is [r, g, b] (0–1)")));
            }
            for (i, v) in a.iter().take(3).enumerate() {
                c[i] = v.as_f64().unwrap_or(0.0).clamp(0.0, 1.0);
            }
        }
    }
    if let Some(b) = b_p(p, "autoPropagate") {
        o.auto_propagate = b;
    }
    s.state.roto = o.clone();
    Ok(serde_json::to_value(o).unwrap_or(Value::Null))
}

fn status(s: &mut Session, p: &Value) -> Result<Value> {
    s.poll_roto(false);
    let mut out =
        json!({"running": s.is_roto_running(), "progress": s.roto_progress(), "banner": s.roto_progress().map(|p| p.banner()), "options": s.state.roto});
    let Ok((cid, lid, uid)) = roto_p(s, p, "roto.status") else { return Ok(out) };
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?.clone();
    let (layer, g) = group(s, cid, lid, uid)?;
    let (layer, g) = (layer.clone(), g.clone());
    let d = data_of(&g);
    out["layer"] = json!(lid.0);
    out["effect"] = json!(uid);
    out["base"] = json!(d.base);
    out["span"] = json!(d.span);
    out["strokes"] = json!(d.strokes.len());
    out["strokeFrames"] = json!(d.stroke_frames());
    out["frozen"] = json!(frozen_of(&g));
    out["pending"] = json!(s.roto_pending.contains(&(cid, lid, uid)));
    out["limits"] = json!(rt::frame_limits(&comp, &layer));
    let frame = frame_p(s, p, cid, &layer);
    out["frame"] = json!(frame);
    let want_matte = b_p(p, "matte").unwrap_or(false);
    let compute = b_p(p, "compute").unwrap_or(want_matte);
    if (p.get("frame").is_some() || want_matte) && d.in_span(frame) {
        let size = rt::layer_size(&s.project, &comp, &layer);
        let fps = comp.frame_rate.as_f64();
        let ps = params_static(&g);
        let chain = fx::Chain::new(&ps, size, fps, 1.0);
        let seg = match chain.keys.get(&frame).and_then(|k| fx::cached(*k, 1.0)) {
            Some(sg) => Some(sg),
            None if compute => {
                let index = layer.effects().and_then(|f| f.groups().position(|x| x.uid == uid)).unwrap_or(0);
                let mut src = rt::FrameSource::new(&s.project, s.footage.as_ref(), s.expr.as_deref(), &s.layer_cache, cid, &comp, &layer, index, size);
                chain.ensure(frame, &mut |f| src.get(f), &|| false)
            }
            None => None,
        };
        if let Some(sg) = seg {
            out["area"] = json!(sg.area());
            out["centroid"] = json!(sg.centroid());
            out["size"] = json!([sg.w, sg.h]);
            out["refineArea"] = json!(sg.refine.iter().filter(|v| **v != 0).count());
            if want_matte {
                out["matte"] = json!(rle::encode(&sg.matte));
            }
            if let Some(gt) = p.get("compareTo").and_then(Value::as_str).and_then(rle::decode) {
                out["iou"] = json!(iou(&sg.matte, &gt));
            }
        }
    }
    let computed = s.roto_computed(cid, lid, uid);
    out["computed"] = json!(computed.iter().filter(|(_, c)| **c).map(|(f, _)| *f).collect::<Vec<_>>());
    Ok(out)
}

/// Two finite numbers.
fn xy_p(p: &Value, k: &str, n: usize, cmd: &str) -> Result<Option<Vec<f64>>> {
    let Some(v) = p.get(k).filter(|v| !v.is_null()) else { return Ok(None) };
    let a = v.as_array().filter(|a| a.len() == n).ok_or_else(|| bad(cmd, format!("`{k}` is {n} numbers (layer pixels)")))?;
    let out: Vec<f64> = a.iter().filter_map(Value::as_f64).filter(|x| x.is_finite()).collect();
    if out.len() != n {
        return Err(bad(cmd, format!("`{k}` is {n} numbers (layer pixels)")));
    }
    Ok(Some(out))
}

fn object_matte_apply(s: &mut Session, p: &Value) -> Result<Value> {
    use effectcraft_effects::object_matte as om;
    let cmd = "objectMatte.apply";
    let (cid, lid) = layer_p(s, p, cmd)?;
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    let layer = comp.layer(lid).ok_or_else(|| bad(cmd, "no layer"))?;
    if !layer.has_video() || layer.switches.adjustment || layer.effects().is_none() {
        return Err(bad(cmd, "Object Matte works on footage, solid and composition layers"));
    }
    // Re-prompt an existing instance (the Object Matte tool on a layer that has one).
    let existing = match p.get("effect").filter(|v| !v.is_null()) {
        Some(v) => {
            let uid = v.as_u64().ok_or_else(|| bad(cmd, "`effect` is an Object Matte effect's uid"))?;
            let g = layer
                .effects()
                .and_then(|f| f.groups().find(|g| g.uid == uid && rt::is_object_matte(g)))
                .ok_or_else(|| bad(cmd, format!("no Object Matte effect {uid} on this layer")))?;
            if frozen_of(g) {
                return Err(bad(cmd, "the Object Matte is frozen: Unfreeze it to change the prompt"));
            }
            Some(uid)
        }
        None => None,
    };
    let size = rt::layer_size(&s.project, comp, layer);
    let point = xy_p(p, "point", 2, cmd)?.ok_or_else(|| bad(cmd, "missing `point` [x, y] (layer pixels)"))?;
    let inside = |x: f64, y: f64| (0.0..=size[0]).contains(&x) && (0.0..=size[1]).contains(&y);
    if !inside(point[0], point[1]) {
        return Err(bad(cmd, format!("`point` is outside the layer ({}×{})", size[0], size[1])));
    }
    let bx = xy_p(p, "box", 4, cmd)?;
    if let Some(b) = &bx {
        let (x0, x1, y0, y1) = (b[0].min(b[2]), b[0].max(b[2]), b[1].min(b[3]), b[1].max(b[3]));
        if !(point[0] > x0 && point[0] < x1 && point[1] > y0 && point[1] < y1) {
            return Err(bad(cmd, "`point` must be inside `box`"));
        }
    }
    let frame = frame_p(s, p, cid, layer);
    let limits = rt::frame_limits(comp, layer);
    if frame < limits[0] || frame > limits[1] {
        return Err(bad(cmd, format!("frame {frame} is outside the layer ({}–{})", limits[0], limits[1])));
    }
    // A new instance gets the defaults; a re-prompted one keeps what is not given.
    let track = b_p(p, "track").or(existing.is_none().then_some(true));
    let reach = |k: &str| {
        let v = f_p(p, k).filter(|v| v.is_finite());
        let v = if existing.is_none() { Some(v.unwrap_or(om::DEFAULT_REACH)) } else { v };
        v.map(|v| v.round().clamp(0.0, om::MAX_REACH))
    };
    let (before, after) = (reach("before"), reach("after"));
    let spec = effectcraft_effects::find(om::ID).ok_or_else(|| EngineError::Other("Object Matte effect missing".into()))?;
    let label = if existing.is_some() { "Object Matte Prompt" } else { "Object Matte" };
    let uid = s.edit(label, None, |proj, _| {
        let mut next = proj.next_id;
        let l = layer_mut(proj, cid, lid)?;
        let fxg = l.props.sub_mut("effects").ok_or_else(|| bad(cmd, "this layer has no effects"))?;
        let mut fresh = None;
        let g = match existing {
            Some(u) => fxg.find_group_mut(u).ok_or_else(|| bad(cmd, "the Object Matte effect is gone"))?,
            None => {
                let n = fxg.groups().filter(|g| rt::is_object_matte(g)).count();
                let name = if n == 0 { spec.name.to_string() } else { format!("{} {}", spec.name, n + 1) };
                fresh.insert(effectcraft_effects::instantiate(spec, &mut Ids(&mut next), &name, size))
            }
        };
        let mut set = |path: &str, v: KV| {
            let (grp, leaf) = path.rsplit_once('/').map_or((None, path), |(a, b)| (Some(a), b));
            let target = match grp {
                Some(sub) => g.sub_mut(sub),
                None => Some(&mut *g),
            };
            // The prompt is static: a re-prompt replaces any keyframes.
            if let Some(pr) = target.and_then(|t| t.get_mut(leaf)) {
                pr.value = v;
                pr.keys.clear();
            }
        };
        set("objectPoint", KV::Vec2([point[0], point[1]]));
        set("useBox", KV::Bool(bx.is_some()));
        if let Some(b) = &bx {
            set("boxTopLeft", KV::Vec2([b[0].min(b[2]), b[1].min(b[3])]));
            set("boxBottomRight", KV::Vec2([b[0].max(b[2]), b[1].max(b[3])]));
        }
        if let Some(t) = track {
            set("objectTracking/trackObject", KV::Bool(t));
        }
        set("objectTracking/referenceFrame", KV::Scalar(frame as f64));
        if let Some(v) = before {
            set("objectTracking/framesBefore", KV::Scalar(v));
        }
        if let Some(v) = after {
            set("objectTracking/framesAfter", KV::Scalar(v));
        }
        let u = g.uid;
        if let Some(g) = fresh {
            fxg.children.push(g.into());
        }
        proj.next_id = proj.next_id.max(next);
        Ok(u)
    })?;
    s.state.selected_layers = vec![lid];
    let (track, before, after) = object_matte_reach(s, cid, lid, uid);
    Ok(json!({"layer": lid.0, "effect": uid, "frame": frame, "track": track, "updated": existing.is_some(), "span": [frame - before, frame + after]}))
}

/// Track Object and Frames Before / After of an Object Matte instance (static values).
fn object_matte_reach(s: &Session, cid: ItemId, lid: LayerId, uid: Uid) -> (bool, i64, i64) {
    use effectcraft_effects::object_matte as om;
    let Ok((_, g)) = group(s, cid, lid, uid) else { return (true, 0, 0) };
    let ps = params_static(g);
    let [before, after] = om::reach(&ps);
    (om::tracks(&ps), before, after)
}

fn object_matte_propagate(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "objectMatte.propagate";
    let (cid, lid, uid) = object_matte_p(s, p, cmd)?;
    let dir = match str_p(p, "direction") {
        Some(d) => Direction::from_name(d).ok_or_else(|| bad(cmd, format!("unknown direction `{d}` (forward | backward | both)")))?,
        None => Direction::Both,
    };
    let wait = b_p(p, "wait").unwrap_or(false);
    let n = s.start_roto(cid, lid, uid, RotoTask::Propagate, dir, wait).map_err(|e| bad(cmd, e))?;
    let mut out = json!({"layer": lid.0, "effect": uid, "frames": n, "running": s.is_roto_running(), "progress": s.roto_progress()});
    if wait {
        out["status"] = object_matte_status(s, &json!({"layer": lid.0, "effect": uid, "comp": cid.0}))?;
    }
    Ok(out)
}

fn object_matte_freeze(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "objectMatte.freeze";
    let (cid, lid, uid) = object_matte_p(s, p, cmd)?;
    let wait = b_p(p, "wait").unwrap_or(false);
    let n = s.start_roto(cid, lid, uid, RotoTask::Freeze, Direction::Both, wait).map_err(|e| bad(cmd, e))?;
    let (_, g) = group(s, cid, lid, uid)?;
    Ok(json!({"layer": lid.0, "effect": uid, "frames": n, "running": s.is_roto_running(), "frozen": frozen_of(g), "progress": s.roto_progress()}))
}

fn object_matte_unfreeze(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "objectMatte.unfreeze";
    let (cid, lid, uid) = object_matte_p(s, p, cmd)?;
    let (_, g) = group(s, cid, lid, uid)?;
    if frozen_of(g) {
        s.edit("Unfreeze", None, |proj, _| {
            let g = layer_mut(proj, cid, lid)?.props.find_group_mut(uid).ok_or_else(|| EngineError::Other("the effect is gone".into()))?;
            if let Some(pr) = g.get_mut(FROZEN) {
                pr.value = KV::Str(String::new());
            }
            Ok(())
        })?;
    }
    Ok(json!({"layer": lid.0, "effect": uid, "frozen": false}))
}

fn object_matte_status(s: &mut Session, p: &Value) -> Result<Value> {
    s.poll_roto(false);
    let running = s.is_roto_running();
    let mut out = json!({"running": running, "progress": s.roto_progress(), "banner": s.roto_progress().map(|p| p.banner())});
    let Ok((cid, lid, uid)) = object_matte_p(s, p, "objectMatte.status") else { return Ok(out) };
    let (_, g) = group(s, cid, lid, uid)?;
    let frozen = frozen_of(g);
    let (track, before, after) = object_matte_reach(s, cid, lid, uid);
    let computed = s.roto_computed(cid, lid, uid);
    out["layer"] = json!(lid.0);
    out["effect"] = json!(uid);
    out["frozen"] = json!(frozen);
    out["track"] = json!(track);
    out["reach"] = json!([before, after]);
    out["mine"] = json!(running && s.roto_target().is_some_and(|(c, l, u, _)| (c, l, u) == (cid, lid, uid)));
    out["pending"] = json!(s.roto_pending.contains(&(cid, lid, uid)));
    out["frames"] = json!(computed.keys().collect::<Vec<_>>());
    out["computed"] = json!(computed.iter().filter(|(_, c)| **c).map(|(f, _)| *f).collect::<Vec<_>>());
    Ok(out)
}

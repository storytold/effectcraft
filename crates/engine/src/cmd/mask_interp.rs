//! Mask tracking (Layer ▸ Mask ▸ Track Mask, the Tracker panel in mask mode, including the Face
//! Tracking methods and Extract & Copy Face Measurements) and Smart Mask Interpolation
//! (Window ▸ Mask Interpolation).

use effectcraft_keyframe::{Keyframe, ShapePath, Value as KV};
use effectcraft_path::interp::{self, AddVertices, InterpOpts, Matching};
use effectcraft_project::{Layer, LayerId, Uid};
use effectcraft_time::Tick;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{CommandSpec, b_p, bad, f_p, has_comp, layer_mut, layer_p, str_p};
use crate::mask_track::{MaskMethod, MaskWork};
use crate::tracking::Direction;
use crate::{EngineError, Result, Session, cmd};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "track.mask",
            "Track Mask",
            ["Animation"],
            None,
            "{layer?, mask?: uid|index|name, method?: position|positionScale|positionScaleRotation|positionScaleRotationSkew|perspective|faceOutline|faceDetailed, direction?: forward|backward|frameForward|frameBackward, start? (s), end? (s), wait?: block until done}",
            has_mask,
            track_mask
        ),
        cmd!(
            "track.maskMethod",
            "Mask Tracking Method",
            [],
            None,
            "{method: position|positionScale|positionScaleRotation|positionScaleRotationSkew|perspective|faceOutline|faceDetailed}",
            super::always,
            mask_method
        ),
        cmd!(
            "track.extractFaceMeasurements",
            "Extract & Copy Face Measurements",
            [],
            None,
            "{layer?} → keys a Face Measurements effect from the layer's Face Track Points (one key per tracked frame) and copies those keys",
            has_face_points,
            extract_face
        ),
        cmd!(
            "mask.interpolate",
            "Apply Mask Interpolation",
            [],
            None,
            "{layer?, mask?, times?: [from, to] (s; default the selected Mask Path keys), keyframeRate?: n|auto, keyframeFields?, linearVertexPaths?, bendingResistance? (0-100), quality? (0-100), addVertices?: n|false, addVerticesUnit?: pixels|total|percent, matchingMethod?: auto|curve|polyline, oneToOne?, firstVerticesMatch?}",
            has_comp,
            interpolate
        ),
        cmd!(
            "mask.interpolationOptions",
            "Mask Interpolation Options",
            [],
            None,
            "{keyframeRate?: n|auto, keyframeFields?, linearVertexPaths?, bendingResistance?, quality?, addVertices?: n|false, addVerticesUnit?: pixels|total|percent, matchingMethod?: auto|curve|polyline, oneToOne?, firstVerticesMatch?}",
            super::always,
            interp_options
        ),
    ]
}

// ---------------------------------------------------------------- mask tracking

/// The mask a command works on: `mask` (uid, 1-based index or name), else the selected mask
/// (Timeline selection or selected vertices), else the layer's only mask.
pub(crate) fn mask_uid(s: &Session, layer: &Layer, p: &Value) -> Option<Uid> {
    let masks = layer.masks()?;
    if let Some(k) = p.get("mask") {
        return match k {
            Value::Number(n) => {
                let n = n.as_u64()?;
                masks.groups().find(|g| g.uid == n).or_else(|| masks.groups().nth((n as usize).checked_sub(1)?)).map(|g| g.uid)
            }
            Value::String(name) => masks.groups().find(|g| &g.name == name).map(|g| g.uid),
            _ => None,
        };
    }
    let is_mask = |u: Uid| masks.groups().any(|g| g.uid == u);
    if let Some(u) = s.state.selected_props.iter().filter(|(l, _)| *l == layer.id).map(|(_, u)| *u).find(|u| is_mask(*u)) {
        return Some(u);
    }
    // A selected Mask Path property selects its mask.
    if let Some(u) = s
        .state
        .selected_props
        .iter()
        .filter(|(l, _)| *l == layer.id)
        .find_map(|(_, u)| masks.groups().find(|g| g.get("path").is_some_and(|pr| pr.uid == *u)).map(|g| g.uid))
    {
        return Some(u);
    }
    if let Some(v) = s.state.selected_vertices.iter().find(|v| v.layer == layer.id && is_mask(v.mask)) {
        return Some(v.mask);
    }
    let mut it = masks.groups();
    match (it.next(), it.next()) {
        (Some(g), None) => Some(g.uid),
        _ => None,
    }
}

/// The selected layer and mask, if any (the Tracker panel's mask mode).
pub fn selected_mask(s: &Session) -> Option<(LayerId, Uid)> {
    let comp = s.active_comp()?;
    for lid in &s.state.selected_layers {
        let Some(l) = comp.layer(*lid) else { continue };
        let explicit =
            s.state.selected_props.iter().any(|(pl, u)| *pl == *lid && l.masks().is_some_and(|m| m.find_group(*u).is_some() || m.find(*u).is_some()))
                || s.state.selected_vertices.iter().any(|v| v.layer == *lid);
        if explicit && let Some(u) = mask_uid(s, l, &json!({})) {
            return Some((*lid, u));
        }
    }
    None
}

fn has_mask(s: &Session) -> std::result::Result<(), String> {
    has_comp(s)?;
    selected_mask(s).map(|_| ()).ok_or_else(|| "select a mask to track".into())
}

fn method_p(p: &Value, cmd: &str) -> Result<Option<MaskMethod>> {
    match str_p(p, "method") {
        Some(m) => MaskMethod::from_name(m).map(Some).ok_or_else(|| {
            bad(
                cmd,
                format!("unknown method `{m}` (position|positionScale|positionScaleRotation|positionScaleRotationSkew|perspective|faceOutline|faceDetailed)"),
            )
        }),
        None => Ok(None),
    }
}

fn mask_method(s: &mut Session, p: &Value) -> Result<Value> {
    let m = method_p(p, "track.maskMethod")?.ok_or_else(|| bad("track.maskMethod", "missing `method`"))?;
    s.state.mask_track_method = m;
    s.bump();
    Ok(json!(m.id()))
}

fn track_mask(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "track.mask";
    let (cid, lid) = match p.get("layer") {
        Some(_) => layer_p(s, p, CMD)?,
        None => match selected_mask(s) {
            Some((l, _)) => (s.active_comp_id().ok_or(EngineError::NoComp)?, l),
            None => layer_p(s, p, CMD)?,
        },
    };
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    let layer = comp.layer(lid).ok_or(EngineError::NoComp)?;
    let uid = mask_uid(s, layer, p).ok_or_else(|| bad(CMD, format!("layer `{}` has no such mask (select one or pass `mask`)", layer.name)))?;
    if !layer.has_video() || matches!(layer.source, effectcraft_project::LayerSource::Text | effectcraft_project::LayerSource::Shape) {
        return Err(bad(CMD, "the layer has no pixels to track"));
    }
    let method = method_p(p, CMD)?.unwrap_or(s.state.mask_track_method);
    let dir = match str_p(p, "direction") {
        Some(d) => Direction::from_name(d).ok_or_else(|| bad(CMD, "direction: forward|backward|frameForward|frameBackward"))?,
        None => Direction::Forward,
    };
    let now = comp.frame_rate.snap_nearest(if s.state.active_comp == Some(cid) { s.time() } else { Tick::ZERO });
    let start = f_p(p, "start").map(|v| comp.frame_rate.snap_nearest(Tick::from_seconds_f64(v)));
    let end = f_p(p, "end").map(|v| comp.frame_rate.snap_nearest(Tick::from_seconds_f64(v)));
    let times = super::track::analysis_times(comp, layer, dir, now, start, end);
    if times.len() < 2 {
        return Err(bad(CMD, "nothing to track in that direction (at the layer's end)"));
    }
    let mg = layer.props.find_group(uid).ok_or_else(|| bad(CMD, "no mask"))?;
    let KV::Path(path) = mg.get("path").map(|pr| pr.value_at(layer.layer_time(times[0]))).unwrap_or(KV::Scalar(0.0)) else {
        return Err(bad(CMD, "the mask has no path"));
    };
    if path.len() < 2 {
        return Err(bad(CMD, "the mask needs at least two vertices"));
    }
    let work = MaskWork {
        project: s.project.clone(),
        footage: s.footage.clone(),
        expr: s.expr.clone(),
        cache: s.layer_cache.clone(),
        comp: cid,
        layer: lid,
        path,
        method,
        times: times.clone(),
        face_model: if method.is_face() { s.models.face() } else { None },
    };
    s.state.mask_track_method = method;
    let wait = b_p(p, "wait").unwrap_or(false);
    s.start_mask_track(work, uid, dir, wait).map_err(EngineError::Other)?;
    let prog = s.mask_track_progress();
    Ok(json!({
        "mask": uid,
        "method": method.id(),
        // The engine face tracking tries first (the classic one when the model finds no face).
        "faceModel": method.is_face().then(|| s.models.face().map_or(effectcraft_segment::CLASSICAL, |m| m.info().id)),
        "frames": times.len() - 1,
        "start": times[0].seconds(),
        "end": times.last().map(|t| t.seconds()),
        "running": s.is_mask_tracking(),
        "progress": prog,
    }))
}

// ---------------------------------------------------------------- face measurements

fn face_points_layer(s: &Session, p: &Value) -> Option<(effectcraft_project::ItemId, LayerId)> {
    use effectcraft_effects::face_track::POINTS_ID;
    let has = |l: &Layer| l.effects().is_some_and(|fx| fx.groups().any(|g| g.match_id == POINTS_ID));
    if p.get("layer").is_some() {
        return layer_p(s, p, "track.extractFaceMeasurements").ok();
    }
    let cid = s.active_comp_id()?;
    let comp = s.project.comp(cid)?;
    let sel = s.state.selected_layers.iter().filter_map(|l| comp.layer(*l)).find(|l| has(l));
    sel.or_else(|| comp.layers.iter().find(|l| has(l))).map(|l| (cid, l.id))
}

fn has_face_points(s: &Session) -> std::result::Result<(), String> {
    has_comp(s)?;
    face_points_layer(s, &json!({})).map(|_| ()).ok_or_else(|| "no Face Track Points: track a mask with Face Tracking (Detailed Features) first".into())
}

/// Extract & Copy Face Measurements: measure the Face Track Points at every keyed time, key them
/// into the layer's Face Measurements effect and put those keys on the keyframe clipboard.
fn extract_face(s: &mut Session, p: &Value) -> Result<Value> {
    use effectcraft_effects::face_track::{MEASUREMENTS_ID, POINTS_ID};
    use effectcraft_track::face::{LANDMARKS, MEASUREMENTS, N, face_frame, measure};
    const CMD: &str = "track.extractFaceMeasurements";
    let (cid, lid) = face_points_layer(s, p).ok_or_else(|| bad(CMD, "no Face Track Points: track a mask with Face Tracking (Detailed Features) first"))?;
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    let layer = comp.layer(lid).ok_or(EngineError::NoComp)?;
    let g = layer.effects().and_then(|fx| fx.groups().find(|g| g.match_id == POINTS_ID)).ok_or_else(|| bad(CMD, "the layer has no Face Track Points"))?;
    let mut times: Vec<Tick> = LANDMARKS.iter().filter_map(|l| g.get(l.0)).flat_map(|pr| pr.keys.iter().map(|k| k.time)).collect();
    times.sort();
    times.dedup();
    if times.is_empty() {
        times.push(layer.layer_time(s.time_of(cid)));
    }
    let rows: Vec<(Tick, [f64; 14])> = {
        let pts_at = |t: Tick| {
            let mut l = [[0.0; 2]; N];
            for (i, (id, _, _)) in LANDMARKS.iter().enumerate() {
                if let Some(KV::Vec2(v)) = g.get(id).map(|pr| pr.value_at(t)) {
                    l[i] = v;
                }
            }
            l
        };
        let reference = face_frame(&pts_at(times[0])).1;
        times.iter().map(|t| (*t, measure(&pts_at(*t), reference))).collect()
    };
    let comp_times: Vec<Tick> = times.iter().map(|t| layer.comp_time(*t)).collect();
    let fd = comp.frame_duration();
    let uid = s.edit("Extract Face Measurements", None, |proj, _| {
        let uid = crate::mask_track::face_effect(proj, cid, lid, MEASUREMENTS_ID).ok_or_else(|| bad(CMD, "Face Measurements is not available"))?;
        let g = proj.comp_mut(cid).and_then(|c| c.layer_mut(lid)).and_then(|l| l.props.find_group_mut(uid)).ok_or(EngineError::NoComp)?;
        for (k, (id, _)) in MEASUREMENTS.iter().enumerate() {
            let Some(pr) = g.get_mut(id) else { continue };
            pr.keys.clear();
            for (t, m) in &rows {
                effectcraft_keyframe::set_key(&mut pr.keys, Keyframe::new(*t, KV::Scalar(m[k])));
            }
        }
        Ok(uid)
    })?;
    // Copy the keys (Edit ▸ Paste pastes them at the CTI, like copied keyframes).
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    let layer = comp.layer(lid).ok_or(EngineError::NoComp)?;
    let mg = layer.props.find_group(uid).ok_or(EngineError::NoComp)?;
    let earliest = comp_times.iter().copied().min().unwrap_or(Tick::ZERO);
    let mut clip = vec![];
    for (id, _) in MEASUREMENTS {
        let Some(pr) = mg.get(id) else { continue };
        let Some(path) = layer.props.match_path_of(pr.uid) else { continue };
        let keys = pr
            .keys
            .iter()
            .map(|k| {
                let mut k = k.clone();
                k.time = layer.comp_time(k.time) - earliest;
                k
            })
            .collect();
        clip.push(crate::KeyClip { layer: lid, path, keys });
    }
    s.state.key_clipboard = clip;
    s.state.clip_is_keys = true;
    // The same data as text (tab-separated, one row per frame) for other applications.
    let mut text = String::from("EffectCraft Face Measurements\n\nFrame\tTime");
    for (_, name) in MEASUREMENTS {
        text.push('\t');
        text.push_str(name);
    }
    text.push('\n');
    let mut table = vec![];
    for ((_, m), ct) in rows.iter().zip(&comp_times) {
        let frame = if fd > Tick::ZERO { (ct.seconds() / fd.seconds()).round() as i64 } else { 0 };
        text.push_str(&format!("{frame}\t{:.4}", ct.seconds()));
        for v in m {
            text.push_str(&format!("\t{v:.3}"));
        }
        text.push('\n');
        let obj: serde_json::Map<String, Value> = MEASUREMENTS.iter().zip(m).map(|((id, _), v)| (id.to_string(), json!(v))).collect();
        table.push(json!({"time": ct.seconds(), "frame": frame, "values": obj}));
    }
    s.bump();
    Ok(json!({"layer": lid.0, "effect": uid, "frames": rows.len(), "measurements": table, "clipboard": text}))
}

// ---------------------------------------------------------------- mask interpolation

/// Mask Interpolation panel options (kept in the editor state, serde for agents).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct MaskInterpOptions {
    /// Keyframes per second; `None` = Auto (the comp frame rate).
    pub keyframe_rate: Option<f64>,
    /// Keyframe Fields (doubles the rate).
    pub keyframe_fields: bool,
    pub linear_vertex_paths: bool,
    pub bending_resistance: f64,
    pub quality: f64,
    /// Add Mask Shape Vertices (value, unit: pixels | total | percent).
    pub add_vertices: Option<f64>,
    pub add_vertices_unit: String,
    pub matching_method: String,
    pub one_to_one: bool,
    pub first_vertices_match: bool,
}

impl Default for MaskInterpOptions {
    fn default() -> Self {
        MaskInterpOptions {
            keyframe_rate: None,
            keyframe_fields: false,
            linear_vertex_paths: false,
            bending_resistance: 50.0,
            quality: 50.0,
            add_vertices: Some(5.0),
            add_vertices_unit: "pixels".into(),
            matching_method: "auto".into(),
            one_to_one: false,
            first_vertices_match: true,
        }
    }
}

impl MaskInterpOptions {
    pub fn interp(&self) -> InterpOpts {
        InterpOpts {
            linear: self.linear_vertex_paths,
            bending_resistance: self.bending_resistance.clamp(0.0, 100.0),
            quality: self.quality.clamp(0.0, 100.0),
            add_vertices: self.add_vertices.map(|v| match self.add_vertices_unit.as_str() {
                "total" => AddVertices::Total(v.round().max(2.0) as usize),
                "percent" => AddVertices::Percent(v),
                _ => AddVertices::Pixels(v),
            }),
            matching: match self.matching_method.as_str() {
                "curve" => Matching::Curve,
                "polyline" => Matching::Polyline,
                _ => Matching::Auto,
            },
            one_to_one: self.one_to_one,
            first_vertices_match: self.first_vertices_match,
        }
    }
}

/// Merge option parameters into `o`.
fn apply_options(o: &mut MaskInterpOptions, p: &Value, cmd: &str) -> Result<()> {
    match p.get("keyframeRate") {
        Some(Value::String(s)) if s.eq_ignore_ascii_case("auto") => o.keyframe_rate = None,
        Some(Value::Null) => o.keyframe_rate = None,
        Some(v) => o.keyframe_rate = Some(v.as_f64().filter(|r| *r > 0.0).ok_or_else(|| bad(cmd, "keyframeRate: a positive number or \"auto\""))?.min(1000.0)),
        None => {}
    }
    for (k, f) in [
        ("keyframeFields", &mut o.keyframe_fields),
        ("linearVertexPaths", &mut o.linear_vertex_paths),
        ("oneToOne", &mut o.one_to_one),
        ("firstVerticesMatch", &mut o.first_vertices_match),
    ] {
        if let Some(v) = b_p(p, k) {
            *f = v;
        }
    }
    if let Some(v) = f_p(p, "bendingResistance") {
        o.bending_resistance = v.clamp(0.0, 100.0);
    }
    if let Some(v) = f_p(p, "quality") {
        o.quality = v.clamp(0.0, 100.0);
    }
    match p.get("addVertices") {
        Some(Value::Bool(false)) | Some(Value::Null) => o.add_vertices = None,
        Some(Value::Bool(true)) => o.add_vertices = Some(o.add_vertices.unwrap_or(5.0)),
        Some(v) => o.add_vertices = Some(v.as_f64().filter(|x| *x > 0.0).ok_or_else(|| bad(cmd, "addVertices: a positive number or false"))?),
        None => {}
    }
    if let Some(u) = str_p(p, "addVerticesUnit") {
        let u = u.to_ascii_lowercase();
        o.add_vertices_unit = match u.as_str() {
            "pixels" | "px" | "pixelsbetweenvertices" => "pixels",
            "total" | "totalvertices" => "total",
            "percent" | "%" | "percentageofoutline" => "percent",
            _ => return Err(bad(cmd, "addVerticesUnit: pixels|total|percent")),
        }
        .into();
    }
    if let Some(m) = str_p(p, "matchingMethod") {
        let m = m.to_ascii_lowercase();
        if !["auto", "curve", "polyline"].contains(&m.as_str()) {
            return Err(bad(cmd, "matchingMethod: auto|curve|polyline"));
        }
        o.matching_method = m;
    }
    Ok(())
}

fn interp_options(s: &mut Session, p: &Value) -> Result<Value> {
    let mut o = s.state.mask_interp.clone();
    apply_options(&mut o, p, "mask.interpolationOptions")?;
    s.state.mask_interp = o.clone();
    s.bump();
    Ok(serde_json::to_value(o).unwrap_or(Value::Null))
}

fn interpolate(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "mask.interpolate";
    let mut o = s.state.mask_interp.clone();
    apply_options(&mut o, p, CMD)?;
    // The layer: given, else the layer of selected Mask Path keys, else the selection.
    let cid = super::comp_id(s, p)?;
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    let lid = match p.get("layer") {
        Some(v) => super::resolve_layer(comp, v).ok_or_else(|| bad(CMD, format!("no layer {v}")))?,
        None => match s.state.selected_keys.iter().find(|k| comp.layer(k.layer).is_some_and(|l| l.masks().is_some_and(|m| m.find(k.prop).is_some()))) {
            Some(k) => k.layer,
            None => layer_p(s, p, CMD)?.1,
        },
    };
    let layer = comp.layer(lid).ok_or(EngineError::NoComp)?;
    let masks = layer.masks().ok_or_else(|| bad(CMD, "the layer has no masks"))?;
    let uid = match p.get("mask") {
        Some(_) => mask_uid(s, layer, p),
        None => s
            .state
            .selected_keys
            .iter()
            .filter(|k| k.layer == lid)
            .find_map(|k| masks.groups().find(|g| g.get("path").is_some_and(|pr| pr.uid == k.prop)).map(|g| g.uid))
            .or_else(|| mask_uid(s, layer, p)),
    }
    .ok_or_else(|| bad(CMD, "no such mask (select two Mask Path keyframes or pass `mask`)"))?;
    let mg = layer.props.find_group(uid).ok_or_else(|| bad(CMD, "no mask"))?;
    let path_prop = mg.get("path").ok_or_else(|| bad(CMD, "the mask has no path"))?;
    // Key times (layer time): explicit, else selected keys on this Mask Path.
    let mut times: Vec<Tick> = match p.get("times").and_then(Value::as_array) {
        Some(a) => {
            let ts: Vec<Tick> = a.iter().filter_map(Value::as_f64).map(|t| layer.layer_time(comp.frame_rate.snap_nearest(Tick::from_seconds_f64(t)))).collect();
            ts.into_iter().filter_map(|t| path_prop.keys.iter().find(|k| (k.time.0 - t.0).abs() <= comp.frame_duration().0 / 2).map(|k| k.time)).collect()
        }
        None => s.state.selected_keys.iter().filter(|k| k.layer == lid && k.prop == path_prop.uid).map(|k| k.time).collect(),
    };
    times.sort();
    times.dedup();
    if times.len() < 2 {
        return Err(bad(CMD, "select two or more Mask Path keyframes (or pass `times` of existing keys)"));
    }
    let rate = o.keyframe_rate.unwrap_or_else(|| comp.frame_rate.as_f64()) * if o.keyframe_fields { 2.0 } else { 1.0 };
    let step = Tick::from_seconds_f64(1.0 / rate.max(0.01));
    let opts = o.interp();
    let shapes: Vec<ShapePath> = times.iter().filter_map(|t| path_prop.keys.iter().find(|k| k.time == *t).and_then(|k| k.value.as_path().cloned())).collect();
    if shapes.len() != times.len() {
        return Err(bad(CMD, "the selected keys must be Mask Path keys"));
    }
    // (time, shape) for every new or replaced key.
    let mut out: Vec<(Tick, ShapePath)> = vec![];
    let mut vertices = 0;
    for i in 0..times.len() - 1 {
        let (t0, t1) = (times[i], times[i + 1]);
        let (a, b) = interp::correspond(&shapes[i], &shapes[i + 1], &opts);
        vertices = vertices.max(a.len());
        out.push((t0, a.clone()));
        let mut t = t0 + step;
        while t1.0 - t.0 > step.0 / 4 {
            let u = (t.0 - t0.0) as f64 / (t1.0 - t0.0) as f64;
            out.push((t, interp::interpolate(&a, &b, u, &opts)));
            t += step;
        }
        out.push((t1, b));
    }
    let created = out.len() - times.len().min(out.len());
    let (first, last) = (times[0], times[times.len() - 1]);
    s.state.mask_interp = o;
    s.edit("Mask Interpolation", None, |proj, _| {
        let l = layer_mut(proj, cid, lid)?;
        let pr = l.props.find_group_mut(uid).and_then(|g| g.get_mut("path")).ok_or_else(|| bad(CMD, "no mask path"))?;
        // Keys strictly inside the interpolated span are replaced.
        pr.keys.retain(|k| k.time <= first || k.time >= last || times.contains(&k.time));
        for (t, sp) in &out {
            effectcraft_keyframe::set_key(&mut pr.keys, Keyframe::new(*t, KV::Path(sp.clone())));
        }
        Ok(())
    })?;
    Ok(json!({"mask": uid, "keys": created, "vertices": vertices, "rate": rate}))
}

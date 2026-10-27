//! Composition and layer markers: the Composition/Layer Marker dialog (`markers.set`), dragging
//! and Alt-dragging the duration in the Timeline (`markers.set` with `merge`), deleting
//! (`markers.delete`, Ctrl/Cmd-click), converting between layer and comp markers
//! (`markers.convert`) and Layer ▸ Markers ▸ Update Markers From Source. Times in parameters are
//! comp seconds; layer markers are stored in layer time. All commands are undoable.

use effectcraft_color::Label;
use effectcraft_project::{Comp, CuePoint, ItemId, Layer, LayerId, LayerSource, Marker, Project};
use effectcraft_time::Tick;
use serde_json::{Value, json};

use super::{CommandSpec, b_p, bad, comp_id, f_p, has_comp, has_layers, layers_p, merge_p, resolve_layer, str_p};
use crate::{EngineError, Result, Session, cmd, query};

/// Where a marker lives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Owner {
    Comp,
    Layer(LayerId),
}

fn owner(s: &Session, p: &Value, cmd: &str) -> Result<(ItemId, Owner)> {
    let cid = comp_id(s, p)?;
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    match p.get("layer") {
        None | Some(Value::Null) => Ok((cid, Owner::Comp)),
        Some(v) => resolve_layer(comp, v).map(|l| (cid, Owner::Layer(l))).ok_or_else(|| bad(cmd, format!("no layer {v}"))),
    }
}

fn markers_of(c: &Comp, o: Owner) -> Option<&Vec<Marker>> {
    match o {
        Owner::Comp => Some(&c.markers),
        Owner::Layer(l) => c.layer(l).map(|l| &l.markers),
    }
}

/// Comp time ↔ marker time (layer markers are in layer time).
fn to_marker_time(c: &Comp, o: Owner, comp_t: Tick) -> Tick {
    match o {
        Owner::Comp => comp_t,
        Owner::Layer(l) => c.layer(l).map_or(comp_t, |l| l.layer_time(comp_t)),
    }
}
fn to_comp_time(c: &Comp, o: Owner, t: Tick) -> Tick {
    match o {
        Owner::Comp => t,
        Owner::Layer(l) => c.layer(l).map_or(t, |l| l.comp_time(t)),
    }
}

/// `index` (0-based) or `at` (comp seconds, within half a frame).
fn find_index(c: &Comp, o: Owner, p: &Value, cmd: &str) -> Result<usize> {
    let ms = markers_of(c, o).ok_or_else(|| bad(cmd, "no such layer"))?;
    if let Some(i) = p.get("index").and_then(Value::as_u64) {
        return (i < ms.len() as u64).then_some(i as usize).ok_or_else(|| bad(cmd, format!("no marker #{i} ({} markers)", ms.len())));
    }
    if let Some(at) = f_p(p, "at") {
        let t = Tick::from_seconds_f64(at);
        let half = Tick(c.frame_duration().0 / 2);
        return ms.iter().position(|m| (to_comp_time(c, o, m.time) - t).0.abs() <= half.0).ok_or_else(|| bad(cmd, format!("no marker at {at} s")));
    }
    Err(bad(cmd, "give the marker `index` (0-based) or `at` (comp seconds)"))
}

fn marker_json(c: &Comp, o: Owner, i: usize, m: &Marker) -> Value {
    json!({
        "index": i,
        "time": to_comp_time(c, o, m.time).seconds(),
        "duration": m.duration.seconds(),
        "comment": m.comment,
        "chapter": m.chapter,
        "url": m.url,
        "frameTarget": m.frame_target,
        "cuePoint": m.cue_point.as_ref().map(|q| json!({"name": q.name, "navigation": q.navigation, "params": q.params})),
        "protected": m.protected,
        "label": m.label.name(),
    })
}

fn list(s: &mut Session, p: &Value) -> Result<Value> {
    let (cid, o) = owner(s, p, "markers.list")?;
    let c = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    let ms = markers_of(c, o).cloned().unwrap_or_default();
    Ok(Value::Array(ms.iter().enumerate().map(|(i, m)| marker_json(c, o, i, m)).collect()))
}

/// The nested comp's markers on a precomp layer, at their times in the containing comp `comp`
/// and within the layer's In–Out range, as After Effects shows them on the precomp layer's bar:
/// `(comp time, index in the nested comp, marker)`. The layer's start time and stretch map them;
/// with time remapping, or a stretch over protected regions (Responsive Design), a marker sits at
/// the first frame the layer shows its time.
pub fn nested_markers<'a>(project: &'a Project, comp_id: ItemId, comp: &Comp, layer: &Layer) -> Vec<(Tick, usize, &'a Marker)> {
    let LayerSource::Comp { item } = layer.source else { return vec![] };
    let Some(nc) = project.comp(item) else { return vec![] };
    if nc.markers.is_empty() || item == comp_id {
        return vec![];
    }
    let shown = |t: Tick| t >= layer.in_point && t < layer.out_point;
    let stretched_regions = (layer.stretch - 100.0).abs() > 1e-9 && nc.markers.iter().any(|m| m.protected);
    if layer.props.get("timeRemap").is_none() && !stretched_regions {
        return nc.markers.iter().enumerate().map(|(i, m)| (layer.comp_time(m.time), i, m)).filter(|(t, _, _)| shown(*t)).collect();
    }
    // The layer's source time at every frame of its range; each marker at the first frame that
    // reaches it (within rounding: a thousandth of a nested frame).
    let fr = comp.frame_rate;
    let (f0, f1) = (fr.frame_at(layer.in_point), fr.frame_at(layer.out_point));
    let frames: Vec<(Tick, Tick)> = (f0..f1.min(f0.saturating_add(100_000)))
        .map(|f| {
            let t = fr.tick_of(f);
            (t, effectcraft_render::EvalCtx::new(project, comp_id, comp, t).source_time(layer))
        })
        .collect();
    let eps = nc.frame_duration().0 / 1000;
    nc.markers
        .iter()
        .enumerate()
        .filter_map(|(i, m)| frames.iter().find(|(_, src)| src.0.saturating_add(eps) >= m.time.0).map(|(t, _)| (*t, i, m)))
        .filter(|(t, _, _)| shown(*t))
        .collect()
}

/// `markers.nested`: the nested comp's markers on a precomp layer (see [`nested_markers`]).
fn nested(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "markers.nested";
    let cid = comp_id(s, p)?;
    let c = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    let lid = p.get("layer").and_then(|v| resolve_layer(c, v)).ok_or_else(|| bad(cmd, "missing `layer` (a precomp layer)"))?;
    let l = c.layer(lid).ok_or_else(|| bad(cmd, "no such layer"))?;
    let LayerSource::Comp { item } = l.source else { return Err(bad(cmd, "not a precomp layer")) };
    let v: Vec<Value> = nested_markers(&s.project, cid, c, l)
        .into_iter()
        .map(|(t, i, m)| json!({"time": t.seconds(), "comp": item.0, "index": i, "nestedTime": m.time.seconds(), "duration": m.duration.seconds(), "comment": m.comment, "label": m.label.name()}))
        .collect();
    Ok(json!(v))
}

fn locked(c: &Comp, o: Owner) -> bool {
    matches!(o, Owner::Layer(l) if c.layer(l).is_some_and(|l| l.markers_locked))
}

/// Apply the dialog fields in `p` to `m`. `time`/`duration` are comp seconds.
fn apply_fields(c: &Comp, o: Owner, m: &mut Marker, p: &Value, cmd: &str) -> Result<()> {
    let fr = c.frame_rate;
    if let Some(t) = f_p(p, "time") {
        let ct = fr.snap_nearest(Tick::from_seconds_f64(t.max(0.0)));
        m.time = to_marker_time(c, o, ct);
    }
    if let Some(d) = f_p(p, "duration") {
        m.duration = fr.snap_nearest(Tick::from_seconds_f64(d.max(0.0)));
    }
    if let Some(v) = str_p(p, "comment") {
        m.comment = v.into();
    }
    if let Some(v) = str_p(p, "chapter") {
        m.chapter = v.into();
    }
    if let Some(v) = str_p(p, "url") {
        m.url = v.into();
    }
    if let Some(v) = str_p(p, "frameTarget") {
        m.frame_target = v.into();
    }
    if let Some(v) = b_p(p, "protected") {
        m.protected = v;
    }
    if let Some(l) = p.get("label") {
        m.label = match l {
            Value::Number(n) => Label::ALL.get(n.as_u64().unwrap_or(0) as usize).copied(),
            Value::String(s) => Label::from_name(s),
            _ => None,
        }
        .ok_or_else(|| bad(cmd, format!("unknown label {l}")))?;
    }
    match p.get("cuePoint") {
        Some(Value::Null) => m.cue_point = None,
        Some(q @ Value::Object(_)) => {
            let name = q.get("name").and_then(Value::as_str).unwrap_or_default().to_string();
            let navigation = q.get("navigation").and_then(Value::as_bool).unwrap_or(false) || q.get("kind").and_then(Value::as_str) == Some("navigation");
            let params = q
                .get("params")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|e| match e {
                            Value::Array(kv) => Some((kv.first()?.as_str()?.to_string(), kv.get(1).and_then(Value::as_str).unwrap_or_default().to_string())),
                            Value::Object(o) => {
                                Some((o.get("name")?.as_str()?.to_string(), o.get("value").and_then(Value::as_str).unwrap_or_default().to_string()))
                            }
                            _ => None,
                        })
                        .collect()
                })
                .unwrap_or_default();
            m.cue_point = Some(CuePoint { name, navigation, params });
        }
        Some(v) => return Err(bad(cmd, format!("cuePoint: {{name, navigation?, params?: [[name, value]…]}} or null, got {v}"))),
        None => {}
    }
    Ok(())
}

fn markers_mut(c: &mut Comp, o: Owner) -> Option<&mut Vec<Marker>> {
    match o {
        Owner::Comp => Some(&mut c.markers),
        Owner::Layer(l) => c.layer_mut(l).map(|l| &mut l.markers),
    }
}

/// Edit (or with `new: true`, add) a marker. Returns its new index.
fn set(s: &mut Session, p: &Value) -> Result<Value> {
    let c = "markers.set";
    let (cid, o) = owner(s, p, c)?;
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    if locked(comp, o) {
        return Err(bad(c, "the layer's markers are locked"));
    }
    let adding = b_p(p, "new").unwrap_or(false);
    let idx = if adding { None } else { Some(find_index(comp, o, p, c)?) };
    let now = s.time();
    let label = if adding { "Add Marker" } else { "Marker Settings" };
    let i = s.edit(label, merge_p(p), |proj, _| {
        let comp = proj.comp_mut(cid).ok_or(EngineError::NoComp)?;
        let snapshot = comp.clone();
        let mut m = match idx {
            Some(i) => markers_of(&snapshot, o).and_then(|v| v.get(i)).cloned().ok_or_else(|| bad(c, "marker vanished"))?,
            None => Marker { time: to_marker_time(&snapshot, o, snapshot.frame_rate.snap_nearest(now)), ..Default::default() },
        };
        apply_fields(&snapshot, o, &mut m, p, c)?;
        let v = markers_mut(comp, o).ok_or_else(|| bad(c, "no such layer"))?;
        if let Some(i) = idx {
            v.remove(i);
        }
        v.push(m.clone());
        v.sort_by_key(|x| x.time);
        Ok(v.iter().position(|x| *x == m).unwrap_or(0))
    })?;
    Ok(json!({"index": i}))
}

fn delete(s: &mut Session, p: &Value) -> Result<Value> {
    let c = "markers.delete";
    let (cid, o) = owner(s, p, c)?;
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    if locked(comp, o) {
        return Err(bad(c, "the layer's markers are locked"));
    }
    let i = find_index(comp, o, p, c)?;
    s.edit("Delete Marker", None, |proj, _| {
        markers_mut(proj.comp_mut(cid).ok_or(EngineError::NoComp)?, o).ok_or_else(|| bad(c, "no such layer"))?.remove(i);
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Layer marker → comp marker (when `layer` is given), or comp marker → layer marker on
/// `toLayer` (default: the selected layers).
fn convert(s: &mut Session, p: &Value) -> Result<Value> {
    let c = "markers.convert";
    let (cid, o) = owner(s, p, c)?;
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?.clone();
    let i = find_index(&comp, o, p, c)?;
    let m = markers_of(&comp, o).and_then(|v| v.get(i)).cloned().ok_or_else(|| bad(c, "no marker"))?;
    let ct = to_comp_time(&comp, o, m.time);
    let targets: Vec<LayerId> = match o {
        Owner::Layer(_) => vec![],
        Owner::Comp => match p.get("toLayer") {
            Some(v) => vec![resolve_layer(&comp, v).ok_or_else(|| bad(c, format!("no layer {v}")))?],
            None => s.state.selected_layers.iter().copied().filter(|l| comp.layer(*l).is_some()).collect(),
        },
    };
    if o == Owner::Comp && targets.is_empty() {
        return Err(bad(c, "select the layer to receive the marker (or give `toLayer`)"));
    }
    let label = if o == Owner::Comp { "Convert to Layer Marker" } else { "Convert to Composition Marker" };
    s.edit(label, None, |proj, _| {
        let cm = proj.comp_mut(cid).ok_or(EngineError::NoComp)?;
        markers_mut(cm, o).ok_or_else(|| bad(c, "no such layer"))?.remove(i);
        match o {
            Owner::Layer(_) => {
                cm.markers.push(Marker { time: ct, ..m.clone() });
                cm.markers.sort_by_key(|x| x.time);
            }
            Owner::Comp => {
                for lid in &targets {
                    if let Some(l) = cm.layer_mut(*lid) {
                        let lt = l.layer_time(ct);
                        l.markers.push(Marker { time: lt, ..m.clone() });
                        l.markers.sort_by_key(|x| x.time);
                    }
                }
            }
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Layer ▸ Markers ▸ Update Markers From Source: copy a precomp's composition markers onto the
/// layer (layer time is the nested comp's time), skipping ones already there.
fn update_from_source(s: &mut Session, p: &Value) -> Result<Value> {
    let (cid, ids) = layers_p(s, p)?;
    let src: Vec<(LayerId, Vec<Marker>)> = {
        let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
        ids.iter()
            .filter_map(|id| {
                let l = comp.layer(*id)?;
                match &l.source {
                    LayerSource::Comp { item } => Some((*id, s.project.comp(*item)?.markers.clone())),
                    _ => None,
                }
            })
            .collect()
    };
    if src.is_empty() {
        return Err(bad("layer.updateMarkersFromSource", "select a precomposition layer"));
    }
    let n = s.edit("Update Markers From Source", None, |proj, _| {
        let c = proj.comp_mut(cid).ok_or(EngineError::NoComp)?;
        let mut n = 0;
        for (lid, ms) in &src {
            let Some(l) = c.layer_mut(*lid) else { continue };
            if l.markers_locked {
                continue;
            }
            for m in ms {
                if !l.markers.iter().any(|x| x.time == m.time) {
                    l.markers.push(m.clone());
                    n += 1;
                }
            }
            l.markers.sort_by_key(|x| x.time);
        }
        Ok(n)
    })?;
    Ok(json!({"added": n}))
}

fn has_precomp_layer(s: &Session) -> std::result::Result<(), String> {
    has_layers(s)?;
    let c = s.active_comp().ok_or("no composition")?;
    if s.state.selected_layers.iter().filter_map(|l| c.layer(*l)).any(|l| matches!(l.source, LayerSource::Comp { .. })) {
        Ok(())
    } else {
        Err("select a precomposition layer".into())
    }
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        query!("markers.list", "List Markers", "{layer? (omit for composition markers)}", list),
        query!(
            "markers.nested",
            "Nested Comp Markers",
            "{layer: a precomp layer, comp?} — its comp's markers at their times in this comp (as on the layer bar)",
            nested
        ),
        cmd!(
            "markers.set",
            "Marker Settings",
            [],
            None,
            "{layer? (omit: comp marker), index? (0-based) | at? (comp s) | new?: true, time? (comp s), duration? (s), comment?, chapter?, url?, frameTarget?, cuePoint?: {name, navigation?, params?: [[name, value]…]} | null, protected?, label?}",
            has_comp,
            set
        ),
        cmd!("markers.delete", "Delete Marker", [], None, "{layer?, index? | at? (comp s)}", has_comp, delete),
        cmd!(
            "markers.convert",
            "Convert Marker",
            [],
            None,
            "{layer? (layer marker → comp marker), index? | at?, toLayer? (comp marker → layer marker)}",
            has_comp,
            convert
        ),
        cmd!("layer.updateMarkersFromSource", "Update Markers From Source", ["Layer", "Markers"], None, "{layers?}", has_precomp_layer, update_from_source),
    ]
}

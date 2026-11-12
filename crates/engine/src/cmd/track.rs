//! Motion tracking commands (Animation ▸ Track Motion / Stabilize Motion / Track this Property and
//! the Tracker panel): create trackers, set type / target / options, move track points, analyze
//! (background job or blocking), apply, reset.
//!
//! A tracker is addressed by `layer` (the tracked layer, a.k.a. Motion Source) and `tracker`
//! (group uid, name or 1-based index); both default to the Tracker panel's Current Track.

use effectcraft_project::build::Ids;
use effectcraft_project::tracking::{self, LowConfidence, TrackChannel, TrackKind, TrackerSettings};
use effectcraft_project::{ItemId, Layer, LayerId, Node, PropGroup, Uid, Value};
use effectcraft_time::Tick;
use serde_json::json;

use super::{CommandSpec, b_p, bad, comp_id, f_p, frontend, has_comp, layer_mut, layer_p, resolve_layer, str_p};
use crate::tracking::{Dims, Direction, Work, point_values};
use crate::{EngineError, Result, Session, cmd, query};

type V = serde_json::Value;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!("track.motion", "Track Motion", ["Animation"], None, "{layer?}", can_track, |s, p| new_with(s, p, TrackKind::Transform, "track.motion")),
        cmd!("track.stabilize", "Stabilize Motion", ["Animation"], None, "{layer?}", can_track, |s, p| new_with(s, p, TrackKind::Stabilize, "track.stabilize")),
        cmd!(
            "track.new",
            "New Tracker",
            [],
            None,
            "{layer?, kind?: transform|stabilize|affine|perspective|raw, position?, rotation?, scale?, target?: layer}",
            can_track,
            new_tracker
        ),
        cmd!("track.property", "Track this Property", ["Animation"], None, "{layer?, source?: layer}", has_prop_selected, track_property),
        cmd!("track.select", "Current Track", [], None, "{layer?, tracker?: uid|name|#n}", has_comp, select),
        cmd!(
            "track.setType",
            "Track Type",
            [],
            None,
            "{layer?, tracker?, kind?: transform|stabilize|affine|perspective|raw, position?, rotation?, scale?}",
            has_track,
            set_type
        ),
        cmd!("track.setTarget", "Edit Target", [], None, "{layer?, tracker?, target: layer|null}", has_track, set_target),
        cmd!(
            "track.options",
            "Motion Tracker Options",
            [],
            None,
            "{layer?, tracker?, name?, channel?: rgb|luminance|saturation, blur? (px, 0 = off), enhance?, subpixel?, adaptEveryFrame?, threshold? (%), action?: continue|stop|extrapolate|adapt, trackShape?}",
            has_track,
            options
        ),
        cmd!(
            "track.setPoint",
            "Move Track Point",
            [],
            None,
            "{layer?, tracker?, point: n (1-based), center?: [x,y], featureSize?: [w,h], searchOffset?: [x,y], searchSize?: [w,h], attachOffset?: [x,y], move?: [dx,dy], time? (s)}",
            has_track,
            set_point
        ),
        cmd!(
            "track.analyze",
            "Analyze",
            [],
            None,
            "{layer?, tracker?, direction?: forward|backward|frameForward|frameBackward, start? (s), end? (s), wait?: block until done}",
            has_track,
            analyze
        ),
        cmd!("track.stop", "Stop Analysis", [], None, "{}", is_tracking, |s, _| Ok(json!({"stopped": s.stop_track() | s.stop_mask_track() | s.stop_warp()}))),
        cmd!("track.apply", "Apply", [], None, "{layer?, tracker?, dimensions?: xy|x|y}", has_track, apply),
        cmd!("track.reset", "Reset", [], None, "{layer?, tracker?}", has_track, reset),
        cmd!("track.delete", "Delete Tracker", [], None, "{layer?, tracker?}", has_track, delete),
        cmd!("track.editTargetDialog", "Edit Target...", [], None, "{}", has_track, |s, p| frontend(s, "track.editTargetDialog", p)),
        cmd!("track.optionsDialog", "Options...", [], None, "{}", has_track, |s, p| frontend(s, "track.optionsDialog", p)),
        query!("track.status", "Tracker Status", "{layer?, tracker?}", status),
    ]
}

// ---------- enablement ----------

fn trackable(l: &Layer) -> bool {
    l.has_video() && !matches!(l.source, effectcraft_project::LayerSource::Text | effectcraft_project::LayerSource::Shape)
}

fn can_track(s: &Session) -> std::result::Result<(), String> {
    has_comp(s)?;
    let c = s.active_comp().ok_or("no composition is open")?;
    match s.state.selected_layers.first().and_then(|l| c.layer(*l)) {
        Some(l) if trackable(l) => Ok(()),
        Some(_) => Err("select a footage, composition or solid layer to track".into()),
        None => Err("select a layer to track".into()),
    }
}

fn has_prop_selected(s: &Session) -> std::result::Result<(), String> {
    has_comp(s)?;
    if s.state.selected_props.is_empty() { Err("select a property to track".into()) } else { Ok(()) }
}

fn has_track(s: &Session) -> std::result::Result<(), String> {
    current(s, &json!({}), "track").map(|_| ()).map_err(|e| e.to_string().replace("invalid parameters for `track`: ", ""))
}

fn is_tracking(s: &Session) -> std::result::Result<(), String> {
    if s.is_tracking() || s.is_mask_tracking() || s.is_warp_analyzing() { Ok(()) } else { Err("no track analysis is running".into()) }
}

// ---------- addressing ----------

/// (comp, tracked layer, tracker uid) from `layer` / `tracker` or the Current Track.
pub(crate) fn current(s: &Session, p: &V, cmd: &str) -> Result<(ItemId, LayerId, Uid)> {
    let cid = comp_id(s, p)?;
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    let lid = match p.get("layer") {
        Some(v) => resolve_layer(comp, v).ok_or_else(|| bad(cmd, format!("no layer {v}")))?,
        None => match s.state.current_track {
            Some((l, _)) if comp.layer(l).is_some() => l,
            _ => s
                .state
                .selected_layers
                .iter()
                .copied()
                .find(|l| comp.layer(*l).is_some_and(|l| l.trackers().next().is_some()))
                .ok_or_else(|| bad(cmd, "no current track: use Track Motion first"))?,
        },
    };
    let layer = comp.layer(lid).ok_or_else(|| bad(cmd, "no layer"))?;
    let uid = match p.get("tracker") {
        Some(V::Number(n)) => {
            let n = n.as_u64().unwrap_or(0);
            layer.tracker(n).map(|(g, _)| g.uid).or_else(|| layer.trackers().nth((n as usize).saturating_sub(1)).map(|(g, _)| g.uid))
        }
        Some(V::String(name)) => match name.strip_prefix('#').and_then(|i| i.parse::<usize>().ok()) {
            Some(i) => layer.trackers().nth(i.saturating_sub(1)).map(|(g, _)| g.uid),
            None => layer.trackers().find(|(g, _)| &g.name == name).map(|(g, _)| g.uid),
        },
        _ => match s.state.current_track {
            Some((l, t)) if l == lid && layer.tracker(t).is_some() => Some(t),
            _ => layer.trackers().next().map(|(g, _)| g.uid),
        },
    }
    .ok_or_else(|| bad(cmd, format!("layer `{}` has no such tracker", layer.name)))?;
    Ok((cid, lid, uid))
}

fn tracker_mut(proj: &mut effectcraft_project::Project, cid: ItemId, lid: LayerId, uid: Uid) -> Result<&mut PropGroup> {
    layer_mut(proj, cid, lid)?.props.find_group_mut(uid).ok_or_else(|| EngineError::Other("tracker gone".into()))
}

fn layer_size(s: &Session, cid: ItemId, l: &Layer) -> [f64; 2] {
    let (w, h) = effectcraft_render::source_size(&s.project, l);
    if w == 0 {
        let c = s.project.comp(cid);
        c.map(|c| [c.width as f64, c.height as f64]).unwrap_or([1920.0, 1080.0])
    } else {
        [w as f64, h as f64]
    }
}

/// The layer directly above `lid` (the default Motion Target).
fn layer_above(comp: &effectcraft_project::Comp, lid: LayerId) -> Option<LayerId> {
    let i = comp.layers.iter().position(|l| l.id == lid)?;
    i.checked_sub(1).map(|i| comp.layers[i].id)
}

fn kind_p(p: &V, cmd: &str) -> Result<Option<TrackKind>> {
    match str_p(p, "kind") {
        Some(k) => TrackKind::from_name(k).map(Some).ok_or_else(|| bad(cmd, format!("unknown kind `{k}` (transform|stabilize|affine|perspective|raw)"))),
        None => Ok(None),
    }
}

// ---------- create ----------

fn new_with(s: &mut Session, p: &V, kind: TrackKind, cmd: &str) -> Result<V> {
    let mut q = p.clone();
    q["kind"] = json!(kind.id());
    create(s, &q, cmd)
}

fn new_tracker(s: &mut Session, p: &V) -> Result<V> {
    create(s, p, "track.new")
}

fn create(s: &mut Session, p: &V, cmd: &str) -> Result<V> {
    let (cid, lid) = layer_p(s, p, cmd)?;
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    let layer = comp.layer(lid).ok_or(EngineError::NoComp)?;
    if !trackable(layer) {
        return Err(bad(cmd, format!("layer `{}` has no pixels to track", layer.name)));
    }
    let kind = kind_p(p, cmd)?.unwrap_or_default();
    let mut st = TrackerSettings::new(kind);
    st.position = b_p(p, "position").unwrap_or(true);
    st.rotation = b_p(p, "rotation").unwrap_or(false);
    st.scale = b_p(p, "scale").unwrap_or(false);
    // Motion Target: the layer above the tracked layer (Stabilize applies to the layer itself).
    st.target = match p.get("target") {
        Some(V::Null) => None,
        Some(v) => Some(resolve_layer(comp, v).ok_or_else(|| bad(cmd, format!("no target layer {v}")))?),
        None if kind == TrackKind::Stabilize => Some(lid),
        None => layer_above(comp, lid),
    };
    let size = layer_size(s, cid, layer);
    let n_existing = layer.trackers().count();
    let name = format!("Tracker {}", n_existing + 1);
    let pts = tracking::default_points(kind, st.point_count(), size);
    let uid = s.edit(&format!("New {}", name), None, |proj, state| {
        let mut next = proj.next_id;
        let l = layer_mut(proj, cid, lid)?;
        let mut ids = Ids(&mut next);
        let g = tracking::tracker(&mut ids, &name, st, &pts);
        let uid = g.uid;
        l.motion_trackers_mut(&mut ids).ok_or_else(|| bad(cmd, "the layer has no Motion Trackers group"))?.children.push(g.into());
        proj.next_id = next;
        state.current_track = Some((lid, uid));
        state.selected_layers = vec![lid];
        Ok(uid)
    })?;
    let _ = frontend(s, "window.panel", &json!({"panel": "tracker"}));
    Ok(json!({"tracker": uid, "name": name, "layer": lid.0, "points": pts.len()}))
}

/// Track this Property: track another layer and drive the selected property's layer.
fn track_property(s: &mut Session, p: &V) -> Result<V> {
    let cid = comp_id(s, p)?;
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    let target = match p.get("layer") {
        Some(v) => resolve_layer(comp, v).ok_or_else(|| bad("track.property", format!("no layer {v}")))?,
        None => s.state.selected_props.first().map(|(l, _)| *l).ok_or_else(|| bad("track.property", "select a property first"))?,
    };
    let source = match p.get("source") {
        Some(v) => resolve_layer(comp, v).ok_or_else(|| bad("track.property", format!("no layer {v}")))?,
        None => {
            let ti = comp.layers.iter().position(|l| l.id == target).unwrap_or(0);
            comp.layers
                .iter()
                .skip(ti + 1)
                .chain(comp.layers.iter().take(ti))
                .find(|l| l.id != target && trackable(l))
                .map(|l| l.id)
                .ok_or_else(|| bad("track.property", "no layer to track"))?
        }
    };
    create(s, &json!({"layer": source.0, "kind": "transform", "target": target.0, "comp": cid.0}), "track.property")
}

// ---------- settings ----------

fn select(s: &mut Session, p: &V) -> Result<V> {
    let (_, lid, uid) = current(s, p, "track.select")?;
    s.state.current_track = Some((lid, uid));
    s.bump();
    Ok(json!({"layer": lid.0, "tracker": uid}))
}

fn set_type(s: &mut Session, p: &V) -> Result<V> {
    let (cid, lid, uid) = current(s, p, "track.setType")?;
    let kind = kind_p(p, "track.setType")?;
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    let layer = comp.layer(lid).ok_or(EngineError::NoComp)?;
    let size = layer_size(s, cid, layer);
    let above = layer_above(comp, lid);
    let n = s.edit("Track Type", None, |proj, _| {
        let mut next = proj.next_id;
        let g = tracker_mut(proj, cid, lid, uid)?;
        let st = g.tracker_settings_mut().ok_or_else(|| bad("track.setType", "not a tracker"))?;
        if let Some(k) = kind {
            if k == TrackKind::Stabilize {
                st.target = Some(lid);
            } else if st.kind == TrackKind::Stabilize && st.target == Some(lid) {
                st.target = above;
            }
            st.kind = k;
        }
        for (k, f) in [("position", &mut st.position), ("rotation", &mut st.rotation), ("scale", &mut st.scale)] {
            if let Some(v) = b_p(p, k) {
                *f = v;
            }
        }
        let (kind, n) = (st.kind, st.point_count());
        // Add or remove Track Points to match the type.
        let have = g.track_points().count();
        if have > n {
            let mut seen = 0;
            g.children.retain(|c| match c {
                Node::Group(tp) if tp.match_id == tracking::TRACK_POINT => {
                    seen += 1;
                    seen <= n
                }
                _ => true,
            });
        } else if have < n {
            let defaults = tracking::default_points(kind, n, size);
            let mut ids = Ids(&mut next);
            for (i, d) in defaults.iter().enumerate().skip(have) {
                g.children.push(tracking::track_point(&mut ids, &format!("Track Point {}", i + 1), d).into());
            }
        }
        proj.next_id = next;
        Ok(n)
    })?;
    Ok(json!({"points": n}))
}

fn set_target(s: &mut Session, p: &V) -> Result<V> {
    let (cid, lid, uid) = current(s, p, "track.setTarget")?;
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    let target = match p.get("target") {
        None | Some(V::Null) => None,
        Some(v) => Some(resolve_layer(comp, v).ok_or_else(|| bad("track.setTarget", format!("no layer {v}")))?),
    };
    s.edit("Edit Target", None, |proj, _| {
        let st = tracker_mut(proj, cid, lid, uid)?.tracker_settings_mut().ok_or_else(|| bad("track.setTarget", "not a tracker"))?;
        if st.kind == TrackKind::Stabilize && target.is_some_and(|t| t != lid) {
            return Err(bad("track.setTarget", "Stabilize applies to the tracked layer"));
        }
        st.target = target;
        Ok(())
    })?;
    Ok(json!({"target": target.map(|t| t.0)}))
}

fn options(s: &mut Session, p: &V) -> Result<V> {
    let (cid, lid, uid) = current(s, p, "track.options")?;
    let channel = match str_p(p, "channel").map(|c| c.to_ascii_lowercase()) {
        None => None,
        Some(c) => Some(match c.as_str() {
            "rgb" => TrackChannel::Rgb,
            "luminance" | "luma" => TrackChannel::Luminance,
            "saturation" => TrackChannel::Saturation,
            _ => return Err(bad("track.options", "channel: rgb|luminance|saturation")),
        }),
    };
    let action = match str_p(p, "action") {
        None => None,
        Some(a) => Some(match effectcraft_track::ConfidenceAction::from_name(a) {
            Some(effectcraft_track::ConfidenceAction::Continue) => LowConfidence::Continue,
            Some(effectcraft_track::ConfidenceAction::Stop) => LowConfidence::Stop,
            Some(effectcraft_track::ConfidenceAction::Extrapolate) => LowConfidence::Extrapolate,
            Some(effectcraft_track::ConfidenceAction::Adapt) => LowConfidence::Adapt,
            None => return Err(bad("track.options", "action: continue|stop|extrapolate|adapt")),
        }),
    };
    let r = s.edit("Motion Tracker Options", None, |proj, _| {
        let g = tracker_mut(proj, cid, lid, uid)?;
        if let Some(n) = str_p(p, "name") {
            g.name = n.to_string();
        }
        let o = &mut g.tracker_settings_mut().ok_or_else(|| bad("track.options", "not a tracker"))?.options;
        if let Some(c) = channel {
            o.channel = c;
        }
        if let Some(b) = f_p(p, "blur") {
            o.blur = b.clamp(0.0, 100.0);
        }
        if let Some(v) = b_p(p, "enhance") {
            o.enhance = v;
        }
        if let Some(v) = b_p(p, "subpixel") {
            o.subpixel = v;
        }
        if let Some(v) = b_p(p, "adaptEveryFrame") {
            o.adapt_every_frame = v;
        }
        if let Some(v) = f_p(p, "threshold") {
            o.threshold = v.clamp(0.0, 100.0);
        }
        if let Some(a) = action {
            o.action = a;
        }
        if let Some(v) = b_p(p, "trackShape") {
            o.track_shape = v;
        }
        Ok(serde_json::to_value(&*o).unwrap_or(V::Null))
    })?;
    Ok(r)
}

fn vec2_p(p: &V, k: &str) -> Option<[f64; 2]> {
    let a = p.get(k)?.as_array()?;
    Some([a.first()?.as_f64()?, a.get(1)?.as_f64()?])
}

fn set_point(s: &mut Session, p: &V) -> Result<V> {
    let (cid, lid, uid) = current(s, p, "track.setPoint")?;
    let n = p.get("point").and_then(V::as_u64).unwrap_or(1).max(1) as usize;
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    let layer = comp.layer(lid).ok_or(EngineError::NoComp)?;
    let t = f_p(p, "time").map(Tick::from_seconds_f64).map(|t| comp.frame_rate.snap_nearest(t)).unwrap_or_else(|| s.time());
    let lt = layer.layer_time(t);
    let merge = super::merge_p(p).map(str::to_string);
    s.edit("Move Track Point", merge.as_deref(), |proj, _| {
        let g = tracker_mut(proj, cid, lid, uid)?;
        let tp_uid = g.track_points().nth(n - 1).map(|g| g.uid).ok_or_else(|| bad("track.setPoint", format!("no track point {n}")))?;
        let tp = g.find_group_mut(tp_uid).ok_or_else(|| bad("track.setPoint", "no track point"))?;
        let cur = point_values(tp, lt);
        let mut center = vec2_p(p, "center").unwrap_or(cur.spec.center);
        if let Some(d) = vec2_p(p, "move") {
            center = [center[0] + d[0], center[1] + d[1]];
        }
        let set = |tp: &mut PropGroup, m: &str, v: [f64; 2]| {
            if let Some(pr) = tp.get_mut(m) {
                pr.set_value_at(lt, Value::Vec2(v));
            }
        };
        set(tp, "featureCenter", center);
        for (k, m) in [("featureSize", "featureSize"), ("searchOffset", "searchOffset"), ("searchSize", "searchSize"), ("attachOffset", "attachPointOffset")] {
            if let Some(v) = vec2_p(p, k) {
                let v = if k.ends_with("Size") { [v[0].max(4.0), v[1].max(4.0)] } else { v };
                set(tp, m, v);
            }
        }
        let off = tp.get("attachPointOffset").map(|pr| pr.value_at(lt).as_vec2()).unwrap_or([0.0; 2]);
        set(tp, "attachPoint", [center[0] + off[0], center[1] + off[1]]);
        Ok(())
    })?;
    Ok(V::Null)
}

// ---------- analyze / apply / reset ----------

fn analyze(s: &mut Session, p: &V) -> Result<V> {
    let (cid, lid, uid) = current(s, p, "track.analyze")?;
    let dir = match str_p(p, "direction") {
        Some(d) => Direction::from_name(d).ok_or_else(|| bad("track.analyze", "direction: forward|backward|frameForward|frameBackward"))?,
        None => Direction::Forward,
    };
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    let layer = comp.layer(lid).ok_or(EngineError::NoComp)?;
    let (tg, settings) = layer.tracker(uid).ok_or_else(|| bad("track.analyze", "no tracker"))?;
    let now = comp.frame_rate.snap_nearest(if s.state.active_comp == Some(cid) { s.time() } else { Tick::ZERO });
    let start = f_p(p, "start").map(|v| comp.frame_rate.snap_nearest(Tick::from_seconds_f64(v)));
    let end = f_p(p, "end").map(|v| comp.frame_rate.snap_nearest(Tick::from_seconds_f64(v)));
    let times = analysis_times(comp, layer, dir, now, start, end);
    if times.len() < 2 {
        return Err(bad("track.analyze", "nothing to analyze in that direction (at the layer's end)"));
    }
    let lt0 = layer.layer_time(times[0]);
    let points: Vec<_> = tg.track_points().map(|g| point_values(g, lt0)).collect();
    if points.is_empty() {
        return Err(bad("track.analyze", "the tracker has no track points"));
    }
    let work = Work {
        project: s.project.clone(),
        footage: s.footage.clone(),
        expr: s.expr.clone(),
        cache: s.layer_cache.clone(),
        comp: cid,
        layer: lid,
        settings: settings.clone(),
        points,
        times: times.clone(),
    };
    s.state.current_track = Some((lid, uid));
    let wait = b_p(p, "wait").unwrap_or(false);
    s.start_track(work, uid, dir, wait).map_err(EngineError::Other)?;
    let prog = s.track_progress();
    Ok(json!({
        "frames": times.len() - 1,
        "start": times[0].seconds(),
        "end": times.last().map(|t| t.seconds()),
        "running": s.is_tracking(),
        "progress": prog,
    }))
}

/// Comp times to analyse from the CTI (`now`) in `dir`, bounded by `start` / `end` and the
/// layer's In/Out points; the first is the start frame.
pub(crate) fn analysis_times(comp: &effectcraft_project::Comp, layer: &Layer, dir: Direction, now: Tick, start: Option<Tick>, end: Option<Tick>) -> Vec<Tick> {
    let fd = comp.frame_duration();
    let lo = layer.in_point.max(Tick::ZERO);
    let hi = layer.out_point.min(comp.duration) - fd;
    let mut times = vec![];
    match dir {
        Direction::Forward | Direction::FrameForward => {
            let mut t = start.unwrap_or(now).clamp(lo, hi);
            let stop = if dir == Direction::FrameForward { t + fd } else { end.unwrap_or(hi).min(hi) };
            while t <= stop && t <= hi {
                times.push(t);
                t += fd;
            }
        }
        Direction::Backward | Direction::FrameBackward => {
            let mut t = end.unwrap_or(now).clamp(lo, hi);
            let stop = if dir == Direction::FrameBackward { t - fd } else { start.unwrap_or(lo).max(lo) };
            while t >= stop && t >= lo {
                times.push(t);
                t -= fd;
            }
        }
    }
    times
}

fn apply(s: &mut Session, p: &V) -> Result<V> {
    let (cid, lid, uid) = current(s, p, "track.apply")?;
    let dims = match str_p(p, "dimensions") {
        Some(d) => Dims::from_name(d).ok_or_else(|| bad("track.apply", "dimensions: xy|x|y"))?,
        None => Dims::XY,
    };
    if s.is_tracking() {
        return Err(bad("track.apply", "wait for the analysis to finish"));
    }
    let n = s.edit("Apply Track", None, |proj, _| crate::tracking::apply(proj, cid, lid, uid, dims).map_err(|e| bad("track.apply", e)))?;
    Ok(json!({"frames": n}))
}

fn reset(s: &mut Session, p: &V) -> Result<V> {
    let (cid, lid, uid) = current(s, p, "track.reset")?;
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    let layer = comp.layer(lid).ok_or(EngineError::NoComp)?;
    let size = layer_size(s, cid, layer);
    s.edit("Reset Track", None, |proj, _| {
        let g = tracker_mut(proj, cid, lid, uid)?;
        let st = g.tracker_settings().cloned().unwrap_or_default();
        let n = g.track_points().count();
        let defaults = tracking::default_points(st.kind, n, size);
        let mut k = 0;
        for c in &mut g.children {
            let Node::Group(tp) = c else { continue };
            if tp.match_id != tracking::TRACK_POINT {
                continue;
            }
            let d = defaults.get(k).copied().unwrap_or(defaults[0]);
            k += 1;
            for (m, v) in [
                ("featureCenter", Value::Vec2(d.center)),
                ("featureSize", Value::Vec2(d.feature_size)),
                ("searchOffset", Value::Vec2(d.search_offset)),
                ("searchSize", Value::Vec2(d.search_size)),
                ("confidence", Value::Scalar(0.0)),
                ("attachPoint", Value::Vec2(d.center)),
                ("attachPointOffset", Value::Vec2(d.attach_offset)),
            ] {
                if let Some(pr) = tp.get_mut(m) {
                    pr.keys.clear();
                    pr.value = v;
                }
            }
        }
        Ok(())
    })?;
    Ok(V::Null)
}

fn delete(s: &mut Session, p: &V) -> Result<V> {
    let (cid, lid, uid) = current(s, p, "track.delete")?;
    s.edit("Delete Tracker", None, |proj, st| {
        let l = layer_mut(proj, cid, lid)?;
        if let Some(mt) = l.props.sub_mut(tracking::MOTION_TRACKERS) {
            mt.children.retain(|c| c.uid() != uid);
            if mt.children.is_empty() {
                l.props.children.retain(|c| c.match_id() != tracking::MOTION_TRACKERS);
            }
        }
        if st.current_track == Some((lid, uid)) {
            st.current_track = None;
        }
        Ok(())
    })?;
    Ok(V::Null)
}

fn status(s: &mut Session, p: &V) -> Result<V> {
    s.poll_track();
    s.poll_mask_track();
    let progress = s.track_progress();
    let cur = current(s, p, "track.status").ok();
    let mut out = json!({"running": s.is_tracking() || s.is_mask_tracking(), "progress": progress, "mask": s.mask_track_progress(), "maskMethod": s.state.mask_track_method.id()});
    if let Some((cid, lid, uid)) = cur
        && let Some(layer) = s.project.comp(cid).and_then(|c| c.layer(lid))
        && let Some((g, st)) = layer.tracker(uid)
    {
        let points: Vec<V> = g
            .track_points()
            .map(|tp| {
                let keys = tp.get("featureCenter").map(|p| p.keys.len()).unwrap_or(0);
                let v = point_values(tp, layer.layer_time(s.time()));
                json!({"name": tp.name, "uid": tp.uid, "center": v.spec.center, "featureSize": v.spec.feature_size, "searchOffset": v.spec.search_offset, "searchSize": v.spec.search_size, "attachOffset": v.attach_offset, "keys": keys})
            })
            .collect();
        let trackers: Vec<V> = layer.trackers().map(|(g, _)| json!({"uid": g.uid, "name": g.name})).collect();
        out["layer"] = json!(lid.0);
        out["tracker"] = json!({"uid": uid, "name": g.name, "settings": st, "points": points});
        out["trackers"] = json!(trackers);
    }
    Ok(out)
}

//! The Footage panel (double-click footage in the Project panel): plays a footage item on its own
//! time ruler, marks In and Out points in source time, and edits the marked range into the active
//! composition with **Overlay Edit** (a new layer at the current time, on top) or **Ripple Insert
//! Edit** (the same, after pushing later layers — and the later part of layers crossing the
//! current time — back by the clip's duration).

use effectcraft_project::{ItemId, ItemKind, LayerSource, build};
use effectcraft_time::Tick;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{CommandSpec, always, bad, comp_id, f_p};
use crate::{EngineError, Result, Session, cmd};

/// What the Footage panel shows (source time).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FootageView {
    pub item: ItemId,
    pub time: Tick,
    #[serde(default)]
    pub in_point: Option<Tick>,
    #[serde(default)]
    pub out_point: Option<Tick>,
}

fn footage_item(s: &Session, p: &Value, cmd: &str) -> Result<ItemId> {
    let id = match p.get("item") {
        Some(Value::Number(n)) => n.as_u64().map(ItemId),
        Some(Value::String(name)) => s.project.find_by_name(name).map(|i| i.id),
        _ => s.state.footage_panel.as_ref().map(|f| f.item).or_else(|| s.state.project_selection.first().copied()),
    }
    .ok_or_else(|| bad(cmd, "no footage: pass `item` or open one in the Footage panel"))?;
    match s.project.item(id).map(|i| &i.kind) {
        Some(ItemKind::Footage(f)) if f.kind != effectcraft_project::FootageKind::Data => Ok(id),
        Some(ItemKind::Solid(_)) => Ok(id),
        _ => Err(bad(cmd, format!("item {} is not footage", id.0))),
    }
}

/// Source duration and frame rate of a footage / solid item (stills: `None`).
fn source_timing(s: &Session, item: ItemId) -> (Option<Tick>, effectcraft_time::FrameRate) {
    match s.project.item(item).map(|i| &i.kind) {
        Some(ItemKind::Footage(f)) if f.kind != effectcraft_project::FootageKind::Still && f.duration > Tick::ZERO => (Some(f.duration), f.frame_rate),
        Some(ItemKind::Footage(f)) => (None, f.frame_rate),
        _ => (None, s.active_comp().map(|c| c.frame_rate).unwrap_or(effectcraft_time::FrameRate::FPS_30)),
    }
}

/// The Footage panel's current time: footage shows its timecode (Start Timecode), anything else
/// time from 0.
pub fn timecode(item: Option<&effectcraft_project::Item>, fr: effectcraft_time::FrameRate, frame: i64) -> String {
    match item.map(|i| &i.kind) {
        Some(ItemKind::Footage(f)) => f.timecode(frame),
        _ => effectcraft_time::format_timecode_ae(frame, fr, false),
    }
}

fn source_time(s: &Session, p: &Value, item: ItemId) -> Option<Tick> {
    let (_, fr) = source_timing(s, item);
    if let Some(t) = f_p(p, "time") {
        return Some(fr.snap_nearest(Tick::from_seconds_f64(t.max(0.0))));
    }
    p.get("frame").and_then(Value::as_i64).map(|f| fr.tick_of(f.max(0)))
}

fn open(s: &mut Session, p: &Value) -> Result<Value> {
    let item = footage_item(s, p, "footage.open")?;
    if s.state.footage_panel.as_ref().is_none_or(|f| f.item != item) {
        s.state.footage_panel = Some(FootageView { item, time: Tick::ZERO, in_point: None, out_point: None });
    }
    s.events.push(crate::Event::Frontend { command: "window.panel".into(), params: json!({"panel": "footage"}) });
    Ok(json!(s.state.footage_panel))
}

fn view_mut<'a>(s: &'a mut Session, cmd: &str) -> Result<&'a mut FootageView> {
    s.state.footage_panel.as_mut().ok_or_else(|| bad(cmd, "the Footage panel shows no footage (footage.open)"))
}

fn clamp_time(s: &Session, item: ItemId, t: Tick) -> Tick {
    let (dur, fr) = source_timing(s, item);
    match dur {
        Some(d) => t.clamp(Tick::ZERO, (d - fr.frame_duration()).max(Tick::ZERO)),
        None => t.max(Tick::ZERO),
    }
}

fn set_time(s: &mut Session, p: &Value) -> Result<Value> {
    let item = view_mut(s, "footage.setTime")?.item;
    let t = source_time(s, p, item).ok_or_else(|| bad("footage.setTime", "missing `time` or `frame`"))?;
    let t = clamp_time(s, item, t);
    view_mut(s, "footage.setTime")?.time = t;
    Ok(json!(t.seconds()))
}

fn set_mark(s: &mut Session, p: &Value, out: bool) -> Result<Value> {
    let c = if out { "footage.setOut" } else { "footage.setIn" };
    let v = view_mut(s, c)?.clone();
    let t = source_time(s, p, v.item).unwrap_or(v.time);
    let t = clamp_time(s, v.item, t);
    let (_, fr) = source_timing(s, v.item);
    let view = view_mut(s, c)?;
    if out {
        // The Out point marks the last frame shown: the range ends after it.
        let end = t + fr.frame_duration();
        view.out_point = Some(end);
        if view.in_point.is_some_and(|i| i >= end) {
            view.in_point = None;
        }
    } else {
        view.in_point = Some(t);
        if view.out_point.is_some_and(|o| o <= t) {
            view.out_point = None;
        }
    }
    Ok(json!(s.state.footage_panel))
}

fn clear_marks(s: &mut Session, _: &Value) -> Result<Value> {
    let v = view_mut(s, "footage.clearInOut")?;
    v.in_point = None;
    v.out_point = None;
    Ok(Value::Null)
}

/// Overlay / Ripple Insert Edit of the marked range at the comp's current time.
fn edit_into_comp(s: &mut Session, p: &Value, ripple: bool) -> Result<Value> {
    let c = if ripple { "footage.rippleInsertEdit" } else { "footage.overlayEdit" };
    let item = footage_item(s, p, c)?;
    let cid = comp_id(s, p)?;
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?.clone();
    let view = s.state.footage_panel.clone().filter(|v| v.item == item);
    let (dur, _) = source_timing(s, item);
    let src_in = f_p(p, "in").map(Tick::from_seconds_f64).or(view.as_ref().and_then(|v| v.in_point)).unwrap_or(Tick::ZERO);
    let fr = comp.frame_rate;
    let at = fr.snap_nearest(f_p(p, "at").map(Tick::from_seconds_f64).unwrap_or_else(|| s.time_of(cid))).max(Tick::ZERO);
    let src_out = f_p(p, "out").map(Tick::from_seconds_f64).or(view.as_ref().and_then(|v| v.out_point)).or(dur).unwrap_or(src_in + (comp.duration - at));
    if src_out <= src_in {
        return Err(bad(c, "the Out point must be after the In point"));
    }
    let len = fr.snap_nearest(src_out - src_in).max(fr.frame_duration());
    if at >= comp.duration {
        return Err(bad(c, "the current time is past the end of the composition"));
    }
    let it = s.project.item(item).cloned().ok_or_else(|| bad(c, "no such item"))?;
    let (src, size) = match &it.kind {
        ItemKind::Footage(f) => (LayerSource::Footage { item }, (f.width, f.height)),
        ItemKind::Solid(so) => (LayerSource::Solid { item }, (so.width, so.height)),
        _ => return Err(bad(c, "not footage")),
    };
    let label = if ripple { "Ripple Insert Edit" } else { "Overlay Edit" };
    let id = s.edit(label, None, |proj, st| {
        let mut next = proj.next_id;
        let cm = proj.comp_mut(cid).ok_or(EngineError::NoComp)?;
        if ripple {
            let mut added = vec![];
            for i in 0..cm.layers.len() {
                let l = &mut cm.layers[i];
                if l.in_point >= at {
                    l.start_time += len;
                    l.in_point += len;
                    l.out_point += len;
                } else if l.out_point > at {
                    // Split: the part after the current time moves back.
                    let mut b = l.clone();
                    super::edit::reid(&mut b, &mut next);
                    l.out_point = at;
                    b.in_point = at + len;
                    b.out_point += len;
                    b.start_time += len;
                    added.push((i, b));
                }
            }
            for (i, b) in added.into_iter().rev() {
                cm.layers.insert(i, b);
            }
        }
        proj.next_id = next;
        let mut l = build::layer(proj, &comp, &it.name, src, size, Some(len));
        l.start_time = at - src_in;
        l.in_point = at;
        l.out_point = (at + len).min(comp.duration);
        let lid = l.id;
        let cm = proj.comp_mut(cid).ok_or(EngineError::NoComp)?;
        cm.layers.insert(0, l);
        st.selected_layers = vec![lid];
        st.selected_props.clear();
        st.selected_keys.clear();
        Ok(lid)
    })?;
    Ok(json!({"layer": id.0, "in": at.seconds(), "duration": len.seconds()}))
}

fn has_view_or_footage(s: &Session) -> std::result::Result<(), String> {
    if s.state.footage_panel.is_some() { Ok(()) } else { Err("open footage in the Footage panel first".into()) }
}

fn can_edit(s: &Session) -> std::result::Result<(), String> {
    has_view_or_footage(s)?;
    super::has_comp(s)
}

fn info(s: &mut Session, _: &Value) -> Result<Value> {
    let Some(v) = &s.state.footage_panel else { return Ok(Value::Null) };
    let (dur, fr) = source_timing(s, v.item);
    let item = s.project.item(v.item);
    let name = item.map(|i| i.name.clone()).unwrap_or_default();
    let frame = fr.frame_at(v.time);
    Ok(
        json!({"item": v.item.0, "name": name, "time": v.time.seconds(), "frame": frame, "timecode": timecode(item, fr, frame), "in": v.in_point.map(|t| t.seconds()), "out": v.out_point.map(|t| t.seconds()), "duration": dur.map(|d| d.seconds()), "frameRate": fr.as_f64()}),
    )
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!("footage.open", "Open in Footage Panel", [], None, "{item: id|name}", always, open),
        crate::query!("footage.info", "Footage Panel State", "{}", info),
        cmd!("footage.setTime", "Footage Panel Time", [], None, "{time? (s, source) | frame?}", has_view_or_footage, set_time),
        cmd!("footage.setIn", "Set In Point", [], None, "{time? | frame? (default: the panel's time)}", has_view_or_footage, |s, p| set_mark(s, p, false)),
        cmd!("footage.setOut", "Set Out Point", [], None, "{time? | frame? (default: the panel's time)}", has_view_or_footage, |s, p| set_mark(s, p, true)),
        cmd!("footage.clearInOut", "Clear In and Out", [], None, "{}", has_view_or_footage, clear_marks),
        cmd!("footage.overlayEdit", "Overlay Edit", [], None, "{item?, comp?, in? (s), out? (s), at? (comp s; default the current time)}", can_edit, |s, p| {
            edit_into_comp(s, p, false)
        }),
        cmd!(
            "footage.rippleInsertEdit",
            "Ripple Insert Edit",
            [],
            None,
            "{item?, comp?, in? (s), out? (s), at? (comp s; default the current time)}",
            can_edit,
            |s, p| edit_into_comp(s, p, true)
        ),
    ]
}

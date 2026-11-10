//! Time navigation (CTI) commands.

use effectcraft_project::{Comp, TimeDisplayStyle};
use effectcraft_time::Tick;
use serde_json::{Value, json};

use super::{CommandSpec, f_p, has_comp, str_p};
use crate::{EngineError, Result, Session, cmd};

fn report(s: &Session) -> Value {
    let t = s.time();
    let frame = s.active_comp().map(|c| c.frame_rate.frame_at(t)).unwrap_or(0);
    let display = s.active_comp().map(|c| display_time(s, c, t));
    json!({"time": t.seconds(), "frame": frame, "display": display})
}

/// A comp time as the project's Time Display Style shows it: timecode (`0:00:02:15`, `;` for
/// drop-frame), frames (`00063`, from Start Numbering Frames At) or Feet + Frames (`0003+15`).
pub fn display_time(s: &Session, comp: &Comp, t: Tick) -> String {
    let fr = comp.frame_rate;
    let st = &s.project.settings;
    match st.time_display {
        TimeDisplayStyle::Frames => format!("{:05}", fr.frame_at(t) + st.frame_start),
        TimeDisplayStyle::Feet35 | TimeDisplayStyle::Feet16 => {
            effectcraft_time::format_feet_frames(fr.frame_at(t + comp.display_start) + st.frame_start, st.time_display.frames_per_foot().unwrap_or(16))
        }
        TimeDisplayStyle::Timecode => effectcraft_time::format_timecode_ae(fr.frame_at(t + comp.display_start), fr, false),
    }
}

/// A length (a layer's Duration) in the project's Time Display Style: a count of frames, so
/// neither Start Numbering Frames At nor the comp's Start Timecode moves it.
pub fn display_duration(s: &Session, comp: &Comp, len: Tick) -> String {
    let fr = comp.frame_rate;
    let n = fr.frame_at(len);
    match s.project.settings.time_display {
        TimeDisplayStyle::Frames => format!("{n:05}"),
        TimeDisplayStyle::Feet35 | TimeDisplayStyle::Feet16 => {
            effectcraft_time::format_feet_frames(n, s.project.settings.time_display.frames_per_foot().unwrap_or(16))
        }
        TimeDisplayStyle::Timecode => effectcraft_time::format_timecode_ae(n, fr, false),
    }
}

/// Parse a time typed in the project's Time Display Style (relative `+n`/`-n` from frame
/// `current`) into a comp frame.
pub fn parse_display_time(s: &Session, comp: &Comp, text: &str, current: i64) -> std::result::Result<i64, String> {
    let fr = comp.frame_rate;
    let st = &s.project.settings;
    match st.time_display.frames_per_foot() {
        Some(pf) => {
            let start = fr.frame_at(comp.display_start) + st.frame_start;
            effectcraft_time::parse_feet_frames(text, pf, current + start).map(|f| f - start).map_err(|e| e.0)
        }
        None => {
            // The Timeline shows non-drop timecode (`:`), so typed `:`/`.` text is read as non-drop
            // too; only explicit `;` fields mean drop-frame (#890).
            let drop_frame = fr.supports_drop_frame() && text.contains(';');
            let parsed = effectcraft_time::parse_timecode(text, fr, drop_frame, current).map_err(|e| e.0)?;
            let relative = text.trim().starts_with('+') || text.trim().starts_with('-');
            if relative {
                Ok(parsed)
            } else if st.time_display == TimeDisplayStyle::Frames
                && let Ok(frame) = text.trim().parse::<i64>()
            {
                // Displayed digits are a plain frame count (00100 is frame 100, not 1 s) plus
                // Start Numbering Frames At.
                Ok(frame.saturating_sub(st.frame_start))
            } else if st.time_display == TimeDisplayStyle::Frames {
                Ok(parsed.saturating_sub(st.frame_start))
            } else {
                // Displayed timecode includes the composition Start Timecode offset.
                Ok(parsed.saturating_sub(fr.frame_at(comp.display_start)))
            }
        }
    }
}

fn set(s: &mut Session, p: &Value) -> Result<Value> {
    let comp = s.active_comp().ok_or(EngineError::NoComp)?;
    // Dialog number fields send floats (`5.0`), so a frame is any JSON number, rounded.
    let t = if let Some(f) = f_p(p, "frame") {
        comp.frame_rate.tick_of(f.round() as i64)
    } else if let Some(tc) = str_p(p, "timecode") {
        let cur = comp.frame_rate.frame_at(s.time());
        let f = parse_display_time(s, comp, tc, cur).map_err(|e| super::bad("time.set", e))?;
        comp.frame_rate.tick_of(f)
    } else {
        Tick::from_seconds_f64(f_p(p, "time").ok_or_else(|| super::bad("time.set", "need `time`, `frame` or `timecode`"))?)
    };
    s.set_time(t);
    Ok(report(s))
}

fn step(s: &mut Session, p: &Value) -> Result<Value> {
    let n = p.get("frames").and_then(Value::as_i64).unwrap_or(1);
    let comp = s.active_comp().ok_or(EngineError::NoComp)?;
    let f = comp.frame_rate.frame_at(s.time()) + n;
    let t = comp.frame_rate.tick_of(f);
    s.set_time(t);
    Ok(report(s))
}

fn go(s: &mut Session, p: &Value) -> Result<Value> {
    let comp = s.active_comp().ok_or(EngineError::NoComp)?.clone();
    let t = s.time();
    let to = str_p(p, "to").unwrap_or("start");
    let sel_layer = s.state.selected_layers.first().and_then(|id| comp.layer(*id));
    let visible = super::visible_p(p);
    let target = match to {
        "start" => Tick::ZERO,
        "end" => comp.duration - comp.frame_duration(),
        "workStart" => comp.work_area.0,
        "workEnd" => comp.work_area.1 - comp.frame_duration(),
        "layerIn" => sel_layer.map(|l| l.in_point).unwrap_or(t),
        "layerOut" => sel_layer.map(|l| l.out_point - comp.frame_duration()).unwrap_or(t),
        "nextInOut" | "prevInOut" => {
            // In and Out points (its last frame) of the selected layers, of every layer with none
            // selected.
            let sel = &s.state.selected_layers;
            let times =
                comp.layers.iter().filter(|l| sel.is_empty() || sel.contains(&l.id)).flat_map(|l| [l.in_point, l.out_point - comp.frame_duration()]).collect();
            step_to(times, t, comp.frame_duration(), to == "nextInOut")
        }
        "nextKey" | "prevKey" if visible.is_some() => {
            // What the Timeline shows: keys of its revealed properties, layer and comp markers
            // and the work area (After Effects' "visible items").
            let mut times: Vec<Tick> = comp.markers.iter().map(|m| m.time).collect();
            times.extend([comp.work_area.0, comp.work_area.1 - comp.frame_duration()]);
            for l in comp.layers.iter().filter(|l| !(comp.hide_shy && l.switches.shy)) {
                times.extend(l.markers.iter().map(|m| l.comp_time(m.time)));
            }
            for (lid, uid) in visible.iter().flatten() {
                if let Some(l) = comp.layer(*lid)
                    && let Some(pr) = l.props.find(*uid)
                {
                    times.extend(pr.keys.iter().map(|k| l.comp_time(k.time)));
                }
            }
            step_to(times, t, comp.frame_duration(), to == "nextKey")
        }
        "nextKey" | "prevKey" => {
            // Keyframes (of visible/selected layers), markers and the work area, in comp time.
            let mut times: Vec<Tick> = comp.markers.iter().map(|m| m.time).collect();
            let layers: Vec<_> = if s.state.selected_layers.is_empty() {
                comp.layers.iter().collect()
            } else {
                comp.layers.iter().filter(|l| s.state.selected_layers.contains(&l.id)).collect()
            };
            // `prop` (uid) narrows the search to one property: the Properties panel's ◀ ▶ arrows.
            let only = p.get("prop").and_then(Value::as_u64);
            if only.is_some() {
                times.clear();
            } else {
                // The work area's start and end too, as in After Effects.
                times.extend([comp.work_area.0, comp.work_area.1 - comp.frame_duration()]);
            }
            for l in layers {
                l.props.walk("", &mut |_, pr| {
                    if only.is_some_and(|u| u != pr.uid) {
                        return;
                    }
                    for k in &pr.keys {
                        times.push(l.comp_time(k.time));
                    }
                });
                if only.is_none() {
                    times.extend(l.markers.iter().map(|m| l.comp_time(m.time)));
                }
            }
            step_to(times, t, comp.frame_duration(), to == "nextKey")
        }
        _ => return Err(super::bad("time.go", "to: start|end|workStart|workEnd|layerIn|layerOut|nextInOut|prevInOut|nextKey|prevKey")),
    };
    s.set_time(target);
    Ok(report(s))
}

/// The nearest time in `times` after (or before) `t`, more than half a frame away; `t` itself
/// when there is none.
fn step_to(mut times: Vec<Tick>, t: Tick, frame: Tick, next: bool) -> Tick {
    times.sort();
    times.dedup();
    let half = frame.0 / 2;
    if next { times.into_iter().find(|x| x.0 > t.0 + half).unwrap_or(t) } else { times.into_iter().rev().find(|x| x.0 < t.0 - half).unwrap_or(t) }
}

/// `p` with `to` set (J / K pass their other parameters on).
fn with_to(p: &Value, to: &str) -> Value {
    let mut p = if p.is_object() { p.clone() } else { json!({}) };
    p["to"] = json!(to);
    p
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!("time.set", "Go to Time...", ["View"], Some("Alt+Shift+J"), "{time? (s) | frame? | timecode?}", has_comp, set),
        cmd!("time.nextFrame", "Next Frame", [], ["PageDown", "Cmd+ArrowRight"], "{}", has_comp, |s, _| step(s, &json!({"frames": 1}))),
        cmd!("time.previousFrame", "Previous Frame", [], ["PageUp", "Cmd+ArrowLeft"], "{}", has_comp, |s, _| step(s, &json!({"frames": -1}))),
        cmd!("time.forward10", "Forward 10 Frames", [], ["Shift+PageDown", "Cmd+Shift+ArrowRight"], "{}", has_comp, |s, _| step(s, &json!({"frames": 10}))),
        cmd!("time.back10", "Back 10 Frames", [], ["Shift+PageUp", "Cmd+Shift+ArrowLeft"], "{}", has_comp, |s, _| step(s, &json!({"frames": -10}))),
        cmd!("time.step", "Step Frames", [], None, "{frames}", has_comp, step),
        cmd!("time.start", "Go to Start", [], ["Home", "Cmd+Alt+ArrowLeft"], "{}", has_comp, |s, _| go(s, &json!({"to": "start"}))),
        cmd!("time.end", "Go to End", [], ["End", "Cmd+Alt+ArrowRight"], "{}", has_comp, |s, _| go(s, &json!({"to": "end"}))),
        cmd!("time.workAreaStart", "Go to Work Area Start", [], Some("Shift+Home"), "{}", has_comp, |s, _| go(s, &json!({"to": "workStart"}))),
        cmd!("time.workAreaEnd", "Go to Work Area End", [], Some("Shift+End"), "{}", has_comp, |s, _| go(s, &json!({"to": "workEnd"}))),
        cmd!("time.previousInOut", "Go to Previous In or Out Point", [], Some("Cmd+Alt+Shift+ArrowLeft"), "{}", has_comp, |s, _| go(
            s,
            &json!({"to": "prevInOut"})
        )),
        cmd!("time.nextInOut", "Go to Next In or Out Point", [], Some("Cmd+Alt+Shift+ArrowRight"), "{}", has_comp, |s, _| go(s, &json!({"to": "nextInOut"}))),
        cmd!("time.layerIn", "Go to Layer In Point", [], Some("I"), "{}", has_comp, |s, _| go(s, &json!({"to": "layerIn"}))),
        cmd!("time.layerOut", "Go to Layer Out Point", [], Some("O"), "{}", has_comp, |s, _| go(s, &json!({"to": "layerOut"}))),
        cmd!(
            "time.nextKey",
            "Go to Next Keyframe or Marker",
            [],
            Some("K"),
            "{visible?: [{layer, prop}] (only these properties' keys, as the Timeline shows them)}",
            has_comp,
            |s, p| go(s, &with_to(p, "nextKey"))
        ),
        cmd!("time.previousKey", "Go to Previous Keyframe or Marker", [], Some("J"), "{visible?: [{layer, prop}]}", has_comp, |s, p| go(
            s,
            &with_to(p, "prevKey")
        )),
        cmd!(
            "time.go",
            "Go To",
            [],
            None,
            "{to: start|end|workStart|workEnd|layerIn|layerOut|nextInOut|prevInOut|nextKey|prevKey, prop?: uid, visible?: [{layer, prop}]}",
            has_comp,
            go
        ),
    ]
}

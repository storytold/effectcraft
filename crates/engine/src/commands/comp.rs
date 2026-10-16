//! Composition menu.

use effectcraft_color::Label;
use effectcraft_project::{Comp, ItemId, ItemKind, Marker, limits};
use effectcraft_time::{FrameRate, Tick};
use serde_json::{Value, json};

use super::{CommandSpec, always, b_p, bad, comp_id, f_p, has_comp, str_p, time_p};
use crate::{EngineError, Result, Session, cmd};

pub(crate) fn rate_p(p: &Value) -> Option<FrameRate> {
    f_p(p, "frameRate").or(f_p(p, "fps")).map(FrameRate::from_f64)
}

/// A zero, negative or tiny rate would make a zero-length frame (and divide by zero later).
fn check_rate(p: &Value, cmd: &str) -> Result<()> {
    match f_p(p, "frameRate").or(f_p(p, "fps")) {
        Some(f) if !(limits::MIN_FPS..=limits::MAX_FPS).contains(&f) => Err(bad(cmd, "frameRate: fps from 0.001 to 1000")),
        _ => Ok(()),
    }
}

fn apply_settings(c: &mut Comp, p: &Value) {
    let (ow, oh) = (c.width, c.height);
    if let Some(w) = p.get("width").and_then(Value::as_u64) {
        c.width = limits::size(w).max(4);
    }
    if let Some(h) = p.get("height").and_then(Value::as_u64) {
        c.height = limits::size(h).max(4);
    }
    // Advanced › Anchor: where the old frame sits in the resized one (0 = top left … 4 = center …
    // 8 = bottom right). Unparented layers (and all their position keys) move by that offset.
    if (c.width, c.height) != (ow, oh) {
        let a = p.get("anchor").and_then(Value::as_u64).unwrap_or(4).min(8);
        let dx = (c.width as f64 - ow as f64) * (a % 3) as f64 / 2.0;
        let dy = (c.height as f64 - oh as f64) * (a / 3) as f64 / 2.0;
        if dx != 0.0 || dy != 0.0 {
            for l in c.layers.iter_mut().filter(|l| l.parent.is_none()) {
                if let Some(tr) = l.transform_mut() {
                    super::layer::offset_position(tr, dx, dy);
                }
            }
        }
    }
    if let Some(t) = f_p(p, "startTime") {
        c.display_start = Tick::from_seconds_f64(t);
    } else if let Some(tc) = str_p(p, "startTimecode")
        && let Ok(f) = effectcraft_time::parse_timecode(tc, c.frame_rate, c.frame_rate.supports_drop_frame(), 0)
    {
        c.display_start = c.frame_rate.tick_of(f);
    }
    match str_p(p, "renderer").map(str::to_ascii_lowercase).as_deref() {
        Some("classic3d" | "classic") => c.renderer = effectcraft_project::Renderer::Classic3D,
        Some("advanced3d" | "advanced") => c.renderer = effectcraft_project::Renderer::Advanced3D,
        _ => {}
    }
    if let Some(r) = rate_p(p) {
        c.frame_rate = r;
    }
    if let Some(d) = f_p(p, "duration") {
        let old = c.duration;
        // Whole frames, at least one.
        c.duration = c.frame_rate.snap_nearest(Tick::from_seconds_f64(d)).max(c.frame_rate.frame_duration());
        if c.work_area.1 == old {
            c.work_area.1 = c.duration;
        }
        c.work_area.1 = c.work_area.1.min(c.duration);
        // A shorter composition must not leave the start past the end.
        keep_work_area_ordered(c);
    }
    if let Some(pa) = f_p(p, "pixelAspect") {
        c.pixel_aspect = limits::pixel_aspect(pa.max(limits::MIN_PIXEL_ASPECT));
    }
    if let Some(Value::Array(bg)) = p.get("background") {
        let g = |i: usize| bg.get(i).and_then(Value::as_f64).unwrap_or(0.0) as f32;
        c.background = [g(0), g(1), g(2)];
    } else if let Some(hex) = str_p(p, "background").and_then(effectcraft_color::Rgba::from_hex) {
        c.background = [hex.r, hex.g, hex.b];
    }
    if let Some(a) = f_p(p, "shutterAngle") {
        c.shutter_angle = a.clamp(0.0, 720.0);
    }
    if let Some(a) = f_p(p, "shutterPhase") {
        c.shutter_phase = a.clamp(-360.0, 360.0);
    }
    if let Some(n) = p.get("motionBlurSamples").and_then(Value::as_u64) {
        c.motion_blur_samples = n.clamp(limits::MOTION_BLUR_SAMPLES.0.into(), limits::MOTION_BLUR_SAMPLES.1.into()) as u32;
    }
    if let Some(n) = p.get("adaptiveSampleLimit").and_then(Value::as_u64) {
        c.motion_blur_adaptive_limit = n.clamp(limits::ADAPTIVE_SAMPLE_LIMIT.0.into(), limits::ADAPTIVE_SAMPLE_LIMIT.1.into()) as u32;
    }
    if let Some(b) = p.get("preserveFrameRate").and_then(Value::as_bool) {
        c.preserve_frame_rate = b;
    }
    if let Some(b) = p.get("preserveResolution").and_then(Value::as_bool) {
        c.preserve_resolution = b;
    }
}

fn new_comp(s: &mut Session, p: &Value) -> Result<Value> {
    check_rate(p, "comp.new")?;
    let name = str_p(p, "name").unwrap_or("Comp 1").to_string();
    let rate = rate_p(p).unwrap_or(FrameRate::FPS_29_97);
    let mut c = Comp::new(1920, 1080, rate, rate.snap_nearest(Tick::from_seconds_f64(10.0)));
    // Settings ▸ 3D ▸ Default 3D Renderer.
    if s.prefs.three_d.default_renderer == "advanced" {
        c.renderer = effectcraft_project::Renderer::Advanced3D;
    }
    apply_settings(&mut c, p);
    let id = s.edit("New Composition", None, |proj, st| {
        let name = proj.unique_item_name(&name, |_| false);
        let id = proj.add_item(&name, Label::Sandstone, None, ItemKind::Comp(c.into()));
        st.project_selection = vec![id];
        Ok(id)
    })?;
    if b_p(p, "open") != Some(false) {
        s.open_comp(id);
    }
    Ok(json!({"comp": id.0}))
}

fn settings(s: &mut Session, p: &Value) -> Result<Value> {
    check_rate(p, "comp.settings")?;
    let cid = comp_id(s, p)?;
    s.edit("Composition Settings", None, |proj, _| {
        if let Some(n) = str_p(p, "name")
            && let Some(it) = proj.item_mut(cid)
        {
            it.name = n.to_string();
        }
        let c = proj.comp_mut(cid).ok_or(EngineError::NoComp)?;
        apply_settings(c, p);
        super::model3d::sync_geometry_options(proj, cid);
        Ok(())
    })?;
    let c = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    Ok(json!({"width": c.width, "height": c.height, "frameRate": c.frame_rate.as_f64(), "duration": c.duration.seconds()}))
}

fn open(s: &mut Session, p: &Value) -> Result<Value> {
    let id = match p.get("comp").or(p.get("item")) {
        Some(Value::Number(n)) => ItemId(n.as_u64().unwrap_or(0)),
        Some(Value::String(name)) => s.project.items.values().find(|i| &i.name == name && i.as_comp().is_some()).map(|i| i.id).ok_or(EngineError::NoComp)?,
        _ => *s.state.project_selection.iter().find(|i| s.project.comp(**i).is_some()).ok_or_else(|| bad("comp.open", "missing `comp`"))?,
    };
    if s.project.comp(id).is_none() {
        return Err(EngineError::NoComp);
    }
    s.open_comp(id);
    Ok(json!({"comp": id.0}))
}

fn close(s: &mut Session, p: &Value) -> Result<Value> {
    let id = comp_id(s, p)?;
    s.state.open_comps.retain(|c| *c != id);
    if s.state.active_comp == Some(id) {
        if let Some(next) = s.state.open_comps.last().copied() {
            s.activate_comp(next);
        } else {
            s.activate_comp_none();
        }
    }
    Ok(Value::Null)
}

fn work_area(s: &mut Session, p: &Value) -> Result<Value> {
    let cid = comp_id(s, p)?;
    let t = s.time();
    let begin = f_p(p, "start").map(Tick::from_seconds_f64);
    let end = f_p(p, "end").map(Tick::from_seconds_f64);
    let which = str_p(p, "set");
    s.edit("Work Area", super::merge_p(p), |proj, _| {
        let c = proj.comp_mut(cid).ok_or(EngineError::NoComp)?;
        let fd = c.frame_duration();
        match which {
            Some("begin") => c.work_area.0 = t.min(c.work_area.1 - fd),
            Some("end") => c.work_area.1 = (t + fd).max(c.work_area.0 + fd).min(c.duration),
            _ => {}
        }
        if let Some(b) = begin {
            // min/max, not clamp: the bounds cross on a comp shorter than a frame or an inverted area.
            c.work_area.0 = b.min(c.duration - fd).max(Tick::ZERO);
        }
        if let Some(e) = end {
            c.work_area.1 = e.max(c.work_area.0 + fd).min(c.duration);
        }
        // A start on its own can land after the current end. Pull it back so the interval
        // still covers one frame, the same rule as `set: "begin"`.
        keep_work_area_ordered(c);
        Ok(())
    })?;
    let c = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    Ok(json!({"start": c.work_area.0.seconds(), "end": c.work_area.1.seconds()}))
}

/// Keep `work_area` inside the composition and at least one frame long.
fn keep_work_area_ordered(c: &mut Comp) {
    let fd = c.frame_duration();
    c.work_area.1 = c.work_area.1.min(c.duration);
    if c.work_area.0 < Tick::ZERO {
        c.work_area.0 = Tick::ZERO;
    }
    if c.work_area.0 + fd > c.work_area.1 {
        c.work_area.0 = (c.work_area.1 - fd).max(Tick::ZERO);
    }
    if c.work_area.0 + fd > c.work_area.1 {
        c.work_area.1 = (c.work_area.0 + fd).min(c.duration);
    }
}

fn trim_to_wa(s: &mut Session, p: &Value) -> Result<Value> {
    let cid = comp_id(s, p)?;
    s.edit("Trim Comp to Work Area", None, |proj, _| {
        let c = proj.comp_mut(cid).ok_or(EngineError::NoComp)?;
        let (a, b) = c.work_area;
        let fd = c.frame_duration();
        // An interval that is already inverted must not become a negative duration.
        if a < Tick::ZERO || b > c.duration || a + fd > b {
            return Err(bad("comp.trimToWorkArea", "the work area must cover at least one frame inside the composition"));
        }
        for l in &mut c.layers {
            l.start_time -= a;
            l.in_point = (l.in_point - a).max(Tick::ZERO);
            l.out_point = (l.out_point - a).min(b - a);
        }
        c.duration = b - a;
        c.work_area = (Tick::ZERO, b - a);
        Ok(())
    })?;
    Ok(Value::Null)
}

fn comp_switch(s: &mut Session, p: &Value) -> Result<Value> {
    let cid = comp_id(s, p)?;
    let name = str_p(p, "switch").ok_or_else(|| bad("comp.setSwitch", "missing `switch`"))?.to_string();
    let v = b_p(p, "value");
    let r = s.edit("Composition Switch", None, |proj, _| {
        let c = proj.comp_mut(cid).ok_or(EngineError::NoComp)?;
        let slot = match name.as_str() {
            "hideShy" | "shy" => &mut c.hide_shy,
            "motionBlur" => &mut c.enable_motion_blur,
            "frameBlending" => &mut c.enable_frame_blending,
            "draft3d" | "draft3D" => &mut c.draft_3d,
            _ => return Err(bad("comp.setSwitch", "switch: hideShy|motionBlur|frameBlending|draft3d")),
        };
        *slot = v.unwrap_or(!*slot);
        Ok(*slot)
    })?;
    Ok(json!(r))
}

fn add_marker(s: &mut Session, p: &Value) -> Result<Value> {
    let cid = comp_id(s, p)?;
    let t = time_p(s, p, s.project.comp(cid));
    let comment = str_p(p, "comment").unwrap_or("").to_string();
    s.edit("Add Marker", None, |proj, _| {
        let c = proj.comp_mut(cid).ok_or(EngineError::NoComp)?;
        c.markers.push(Marker { time: t, comment, ..Default::default() });
        c.markers.sort_by_key(|m| m.time);
        Ok(())
    })?;
    Ok(json!({"time": t.seconds()}))
}

fn poster(s: &mut Session, p: &Value) -> Result<Value> {
    let cid = comp_id(s, p)?;
    let t = s.time();
    s.edit("Set Poster Time", None, |proj, _| {
        proj.comp_mut(cid).ok_or(EngineError::NoComp)?.poster_time = t;
        Ok(())
    })?;
    Ok(Value::Null)
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "comp.new",
            "New Composition...",
            ["Composition"],
            Some("Cmd+N"),
            "{name?, width?, height?, frameRate?, duration? (s), startTime? (s) | startTimecode?, background? [r,g,b]|#hex, pixelAspect?, shutterAngle?, shutterPhase?, motionBlurSamples?, adaptiveSampleLimit? (16–256), preserveFrameRate?: bool, preserveResolution?: bool, renderer?: classic3D|advanced3D, anchor?, open?}",
            always,
            new_comp
        ),
        cmd!(
            "comp.settings",
            "Composition Settings...",
            ["Composition"],
            Some("Cmd+K"),
            "{comp?, name?, width?, height?, anchor? 0-8 (resize anchor, 4 = center), frameRate?, duration?, startTime? (s) | startTimecode?, background?, shutterAngle?, shutterPhase?, motionBlurSamples?, adaptiveSampleLimit? (16–256), preserveFrameRate?: bool (nested or in the render queue it shows only its own frames), preserveResolution?: bool (nested, it renders at full size), pixelAspect?, renderer?: classic3D|advanced3D}",
            has_comp,
            settings
        ),
        cmd!("comp.setPosterTime", "Set Poster Time", ["Composition"], None, "{}", has_comp, poster),
        cmd!("comp.trimToWorkArea", "Trim Comp to Work Area", ["Composition"], Some("Cmd+Shift+X"), "{comp?}", has_comp, trim_to_wa),
        cmd!("comp.open", "Open Composition", [], None, "{comp: id|name}", always, open),
        cmd!("comp.close", "Close Composition", [], None, "{comp?}", has_comp, close),
        cmd!("comp.workArea", "Set Work Area", [], None, "{start?, end?, set?: begin|end (at CTI)}", has_comp, work_area),
        cmd!("comp.setSwitch", "Composition Switch", [], None, "{switch: hideShy|motionBlur|frameBlending|draft3d, value?}", has_comp, comp_switch),
        cmd!("comp.addMarker", "Add Marker", [], Some("Num*"), "{time?, comment?}", has_comp, add_marker),
    ]
}

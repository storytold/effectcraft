//! Paint tools (Brush, Clone Stamp, Eraser) and the Paint / Brushes panel options.
//!
//! `paint.stroke` records one stroke into the layer's Paint effect (adding the effect when the
//! last effect in the stack isn't Paint, as After Effects does). Defaults come from the Paint
//! and Brushes panel options in [`PaintOptions`] (`paint.options`, `paint.brushPreset`).

use effectcraft_effects::paint::{self, BRUSH_PRESETS, CHANNELS, DURATIONS, ERASE_MODES, MODES, StrokeKind, StrokeSpec};
use effectcraft_keyframe::{Keyframe, Value as KV};
use effectcraft_project::build::Ids;
use effectcraft_project::{GroupKind, PropGroup, Uid};
use effectcraft_time::Tick;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{CommandSpec, always, b_p, bad, f_p, has_comp, layer_mut, layer_p, str_p};
use crate::{EngineError, Result, Session, cmd};

/// Paint and Brushes panel state (serde: agents read and set it with `paint.options`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PaintOptions {
    /// Foreground (paint) and background colours, straight RGBA 0–1.
    pub color: [f64; 4],
    pub background: [f64; 4],
    /// Percent.
    pub opacity: f64,
    pub flow: f64,
    /// Index into the paint mode list (`paint::MODES`).
    pub mode: u32,
    /// Index into RGBA / RGB / Alpha.
    pub channels: u32,
    /// Index into Constant / Write On / Single Frame / Custom.
    pub duration: u32,
    /// Custom duration in frames.
    pub custom_frames: u32,
    /// Index into Layer Source & Paint / Paint Only / Last Stroke Only.
    pub erase: u32,
    // Brushes panel.
    pub diameter: f64,
    pub angle: f64,
    pub roundness: f64,
    pub hardness: f64,
    pub spacing: f64,
    /// Selected preset tip (index into `paint::BRUSH_PRESETS`).
    pub preset: Option<usize>,
    /// Brush dynamics (pen pressure).
    pub size_pressure: bool,
    pub min_size: f64,
    pub opacity_pressure: bool,
    pub flow_pressure: bool,
    // Clone options.
    /// Clone preset slot (1–5).
    pub clone_preset: u32,
    /// Source layer (None = the layer being painted).
    pub clone_source: Option<u64>,
    /// Source point set with Alt/Option-click (effect space of the source layer).
    pub clone_point: Option<[f64; 2]>,
    /// Aligned: keep the source/destination offset between strokes.
    pub aligned: bool,
    /// Offset (source − destination) remembered by Aligned.
    pub clone_offset: Option<[f64; 2]>,
    pub lock_source_time: bool,
    /// Source Time Shift in seconds (or the locked Source Time).
    pub clone_time_shift: f64,
    pub clone_time: f64,
    /// Clone Source Overlay.
    pub show_overlay: bool,
    pub overlay_opacity: f64,
    pub overlay_difference: bool,
}

impl Default for PaintOptions {
    fn default() -> Self {
        PaintOptions {
            color: [1.0, 1.0, 1.0, 1.0],
            background: [0.0, 0.0, 0.0, 1.0],
            opacity: 100.0,
            flow: 100.0,
            mode: 0,
            channels: 0,
            duration: 0,
            custom_frames: 1,
            erase: 0,
            diameter: 10.0,
            angle: 0.0,
            roundness: 100.0,
            hardness: 100.0,
            spacing: 25.0,
            preset: None,
            size_pressure: true,
            min_size: 1.0,
            opacity_pressure: false,
            flow_pressure: false,
            clone_preset: 1,
            clone_source: None,
            clone_point: None,
            aligned: true,
            clone_offset: None,
            lock_source_time: false,
            clone_time_shift: 0.0,
            clone_time: 0.0,
            show_overlay: false,
            overlay_opacity: 50.0,
            overlay_difference: false,
        }
    }
}

fn norm(s: &str) -> String {
    s.chars().filter(char::is_ascii_alphanumeric).collect::<String>().to_ascii_lowercase()
}

/// Index of `v` in `names` (by number or case/space-insensitive name).
fn pick(v: Option<&Value>, names: &[&str], cmd: &str, what: &str) -> Result<Option<u32>> {
    match v {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(n)) => {
            let i = n.as_u64().unwrap_or(u64::MAX);
            if (i as usize) < names.len() { Ok(Some(i as u32)) } else { Err(bad(cmd, format!("`{what}` index out of range"))) }
        }
        Some(Value::String(s)) => {
            let k = norm(s);
            names
                .iter()
                .position(|n| norm(n) == k || (k == "writeon" && norm(n) == "writeon"))
                .map(|i| Some(i as u32))
                .ok_or_else(|| bad(cmd, format!("unknown {what} `{s}`; one of: {}", names.join(", "))))
        }
        Some(_) => Err(bad(cmd, format!("`{what}` must be a name or index"))),
    }
}

fn mode_names() -> Vec<&'static str> {
    MODES.iter().map(|m| m.label()).collect()
}

fn color_p(v: Option<&Value>) -> Option<[f64; 4]> {
    let a = v?.as_array()?;
    let g = |i: usize, d: f64| a.get(i).and_then(Value::as_f64).unwrap_or(d);
    Some([g(0, 0.0), g(1, 0.0), g(2, 0.0), g(3, 1.0)])
}

fn pt(v: Option<&Value>) -> Option<[f64; 2]> {
    let a = v?.as_array()?;
    Some([a.first()?.as_f64()?, a.get(1)?.as_f64()?])
}

/// Apply option keys from `p` onto `o` (shared by `paint.options` and `paint.stroke`).
fn apply_options(o: &mut PaintOptions, p: &Value, cmd: &str) -> Result<()> {
    if let Some(c) = color_p(p.get("color")) {
        o.color = c;
    }
    if let Some(c) = color_p(p.get("background")) {
        o.background = c;
    }
    for (k, f) in [
        ("opacity", &mut o.opacity as &mut f64),
        ("flow", &mut o.flow),
        ("diameter", &mut o.diameter),
        ("angle", &mut o.angle),
        ("roundness", &mut o.roundness),
        ("hardness", &mut o.hardness),
        ("spacing", &mut o.spacing),
        ("minSize", &mut o.min_size),
        ("cloneTimeShift", &mut o.clone_time_shift),
        ("cloneTime", &mut o.clone_time),
        ("overlayOpacity", &mut o.overlay_opacity),
    ] {
        if let Some(v) = f_p(p, k) {
            *f = v;
        }
    }
    o.opacity = o.opacity.clamp(0.0, 100.0);
    o.flow = o.flow.clamp(0.0, 100.0);
    o.diameter = o.diameter.clamp(0.0, 2500.0);
    o.roundness = o.roundness.clamp(0.0, 100.0);
    o.hardness = o.hardness.clamp(0.0, 100.0);
    o.spacing = o.spacing.clamp(1.0, 1000.0);
    o.min_size = o.min_size.clamp(0.0, 100.0);
    for (k, f) in [
        ("sizePressure", &mut o.size_pressure as &mut bool),
        ("opacityPressure", &mut o.opacity_pressure),
        ("flowPressure", &mut o.flow_pressure),
        ("aligned", &mut o.aligned),
        ("lockSourceTime", &mut o.lock_source_time),
        ("showOverlay", &mut o.show_overlay),
        ("overlayDifference", &mut o.overlay_difference),
    ] {
        if let Some(v) = b_p(p, k) {
            *f = v;
        }
    }
    let modes = mode_names();
    if let Some(i) = pick(p.get("mode"), &modes, cmd, "mode")? {
        o.mode = i;
    }
    if let Some(i) = pick(p.get("channels"), &CHANNELS, cmd, "channels")? {
        o.channels = i;
    }
    let durs = ["constant", "writeOn", "singleFrame", "custom"];
    if let Some(i) = pick(p.get("durationMode"), &DURATIONS, cmd, "durationMode").or_else(|_| pick(p.get("durationMode"), &durs, cmd, "durationMode"))? {
        o.duration = i;
    }
    if let Some(n) = p.get("customFrames").and_then(Value::as_u64) {
        o.custom_frames = (n as u32).max(1);
    }
    let erase = ["layerSourceAndPaint", "paintOnly", "lastStrokeOnly"];
    if let Some(i) = pick(p.get("eraseMode"), &ERASE_MODES, cmd, "eraseMode").or_else(|_| pick(p.get("eraseMode"), &erase, cmd, "eraseMode"))? {
        o.erase = i;
    }
    if let Some(n) = p.get("clonePreset").and_then(Value::as_u64) {
        o.clone_preset = (n as u32).clamp(1, 5);
    }
    match p.get("cloneSource") {
        Some(Value::Null) => o.clone_source = None,
        Some(Value::Number(n)) => o.clone_source = n.as_u64(),
        _ => {}
    }
    if let Some(v) = p.get("clonePoint") {
        o.clone_point = pt(Some(v));
        o.clone_offset = None;
    }
    if let Some(v) = p.get("cloneOffset") {
        o.clone_offset = pt(Some(v));
    }
    if let Some(i) = p.get("preset").and_then(Value::as_u64) {
        set_preset(o, i as usize, cmd)?;
    }
    Ok(())
}

fn set_preset(o: &mut PaintOptions, i: usize, cmd: &str) -> Result<()> {
    let t = BRUSH_PRESETS.get(i).ok_or_else(|| bad(cmd, format!("no brush preset {i} (0–{})", BRUSH_PRESETS.len() - 1)))?;
    o.diameter = t.diameter;
    o.angle = t.angle;
    o.roundness = t.roundness;
    o.hardness = t.hardness;
    o.spacing = t.spacing;
    o.preset = Some(i);
    Ok(())
}

fn options(s: &mut Session, p: &Value) -> Result<Value> {
    let mut o = s.state.paint.clone();
    apply_options(&mut o, p, "paint.options")?;
    // Editing the tip by hand deselects the preset.
    if p.get("preset").is_none() && ["diameter", "angle", "roundness", "hardness", "spacing"].iter().any(|k| p.get(k).is_some()) {
        o.preset = None;
    }
    if b_p(p, "reset") == Some(true) {
        o = PaintOptions::default();
    }
    s.state.paint = o;
    // View state only: no project revision bump.
    Ok(serde_json::to_value(&s.state.paint).unwrap_or(Value::Null))
}

fn preset(s: &mut Session, p: &Value) -> Result<Value> {
    let i = match p.get("preset").or_else(|| p.get("name")) {
        Some(Value::Number(n)) => n.as_u64().unwrap_or(u64::MAX) as usize,
        Some(Value::String(name)) => {
            BRUSH_PRESETS.iter().position(|t| norm(t.name) == norm(name)).ok_or_else(|| bad("paint.brushPreset", format!("no preset `{name}`")))?
        }
        _ => return Ok(json!(BRUSH_PRESETS.iter().map(|t| t.name).collect::<Vec<_>>())),
    };
    let mut o = s.state.paint.clone();
    set_preset(&mut o, i, "paint.brushPreset")?;
    s.state.paint = o;
    // View state only: no project revision bump.
    Ok(json!({"preset": i, "name": BRUSH_PRESETS[i].name}))
}

fn set_clone_source(s: &mut Session, p: &Value) -> Result<Value> {
    let point = pt(p.get("point")).ok_or_else(|| bad("paint.setCloneSource", "missing `point` [x, y] (effect space)"))?;
    let (_, lid) = layer_p(s, p, "paint.setCloneSource")?;
    s.state.paint.clone_source = Some(lid.0);
    s.state.paint.clone_point = Some(point);
    s.state.paint.clone_offset = None;
    Ok(json!({"layer": lid.0, "point": point}))
}

/// The layer's Paint effect to record into: the last effect when it is Paint, else a new one.
fn paint_effect(fx: &mut PropGroup, ids: &mut Ids, layer_size: [f64; 2]) -> Result<Uid> {
    if let Some(g) = fx.groups().last().filter(|g| paint::is_paint(g)) {
        return Ok(g.uid);
    }
    let spec = effectcraft_effects::find(paint::ID).ok_or_else(|| EngineError::Other("Paint effect missing".into()))?;
    let n = fx.groups().filter(|g| paint::is_paint(g)).count();
    let name = if n == 0 { "Paint".to_string() } else { format!("Paint {}", n + 1) };
    let g = effectcraft_effects::instantiate(spec, ids, &name, layer_size);
    let uid = g.uid;
    fx.children.push(g.into());
    Ok(uid)
}

fn stroke(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "paint.stroke";
    let (cid, lid) = layer_p(s, p, cmd)?;
    let kind = match str_p(p, "kind") {
        Some(k) => StrokeKind::from_name(k).ok_or_else(|| bad(cmd, format!("unknown kind `{k}` (brush | clone | eraser)")))?,
        None => StrokeKind::Brush,
    };
    let raw = p.get("points").and_then(Value::as_array).ok_or_else(|| bad(cmd, "missing `points` [[x, y, pressure?], …] (effect space)"))?;
    let mut points = vec![];
    let mut pressure = vec![];
    for q in raw {
        let a = q.as_array().ok_or_else(|| bad(cmd, "each point is [x, y] or [x, y, pressure]"))?;
        let (Some(x), Some(y)) = (a.first().and_then(Value::as_f64), a.get(1).and_then(Value::as_f64)) else {
            return Err(bad(cmd, "each point is [x, y] or [x, y, pressure]"));
        };
        points.push([x, y]);
        pressure.push(a.get(2).and_then(Value::as_f64).unwrap_or(1.0).clamp(0.0, 1.0));
    }
    if points.is_empty() {
        return Err(bad(cmd, "`points` is empty"));
    }
    if pressure.iter().all(|v| *v == 1.0) {
        pressure.clear();
    }
    let mut o = s.state.paint.clone();
    apply_options(&mut o, p, cmd)?;
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    let fr = comp.frame_rate;
    let fd = fr.frame_duration();
    let t = fr.snap_nearest(super::time_p(s, p, Some(comp)));
    let layer = comp.layer(lid).ok_or_else(|| bad(cmd, "no layer"))?;
    let lt = layer.layer_time(t);
    let size = effectcraft_render::effect_bounds(&s.project, comp, layer).0;
    if layer.effects().is_none() || !layer.source.is_av() {
        return Err(bad(cmd, "this layer can't be painted on"));
    }
    let in_t = lt.seconds();
    let (out_t, write_on) = match o.duration {
        1 => (paint::FOREVER, true),
        2 => ((lt + fd).seconds(), false),
        3 => ((lt + Tick(fd.0 * o.custom_frames.max(1) as i64)).seconds(), false),
        _ => (paint::FOREVER, false),
    };
    // Write On: End animates 0 → 100 % over the time the stroke took to draw.
    let draw = f_p(p, "duration").unwrap_or(points.len() as f64 / fr.as_f64().max(1.0)).max(0.0);
    let draw_end = lt + fr.snap_nearest(Tick::from_seconds_f64(draw)).max(fd);
    // Clone position: explicit, Aligned offset, or the source point.
    let first = points[0];
    let mut clone_position = pt(p.get("clonePosition"));
    if kind == StrokeKind::Clone && clone_position.is_none() {
        clone_position = Some(match (o.aligned, o.clone_offset, o.clone_point) {
            (true, Some(off), _) => [first[0] + off[0], first[1] + off[1]],
            (_, _, Some(sp)) => sp,
            _ => first,
        });
    }
    let clone_source = match p.get("cloneSource") {
        Some(Value::Number(n)) => n.as_u64(),
        Some(Value::Null) => None,
        _ => o.clone_source.filter(|id| *id != lid.0),
    };
    let spec = StrokeSpec {
        kind,
        points,
        pressure,
        color: o.color,
        diameter: o.diameter,
        angle: o.angle,
        hardness: o.hardness,
        roundness: o.roundness,
        spacing: o.spacing,
        channels: o.channels,
        opacity: o.opacity,
        flow: o.flow,
        mode: o.mode,
        size_pressure: o.size_pressure,
        min_size: o.min_size,
        opacity_pressure: o.opacity_pressure,
        flow_pressure: o.flow_pressure,
        erase_mode: o.erase,
        target: None,
        clone_source,
        clone_position: clone_position.unwrap_or(first),
        clone_time_shift: o.clone_time_shift,
        lock_time: o.lock_source_time,
        clone_time: o.clone_time,
        in_time: in_t,
        out_time: out_t,
        duration: o.duration,
    };
    let label = format!("{} Stroke", kind.label());
    let (fx_uid, uid) = s.edit(&label, None, |proj, st| {
        let mut next = proj.next_id;
        let l = layer_mut(proj, cid, lid)?;
        let fx = l.props.sub_mut("effects").ok_or_else(|| bad(cmd, "this layer has no effects"))?;
        let mut ids = Ids(&mut next);
        let fx_uid = paint_effect(fx, &mut ids, size)?;
        let pg = fx.find_group_mut(fx_uid).ok_or_else(|| bad(cmd, "Paint effect gone"))?;
        let mut spec = spec;
        if spec.kind == StrokeKind::Eraser && spec.erase_mode == 2 {
            spec.target = paint::strokes(pg).filter(|g| g.match_id != "eraser").last().map(|g| g.uid);
        }
        let name = paint::next_name(pg, spec.kind);
        let mut g = paint::stroke_group(&mut ids, &name, &spec);
        if write_on && let Some(end) = g.sub_mut("stroke_options").and_then(|o| o.get_mut("end")) {
            end.keys = vec![Keyframe::new(lt, KV::Scalar(0.0)), Keyframe::new(draw_end, KV::Scalar(100.0))];
        }
        let uid = g.uid;
        pg.children.push(g.into());
        proj.next_id = next;
        st.selected_props = vec![(lid, uid)];
        if spec.kind == StrokeKind::Clone && st.paint.aligned && st.paint.clone_offset.is_none() {
            st.paint.clone_offset = Some([spec.clone_position[0] - spec.points[0][0], spec.clone_position[1] - spec.points[0][1]]);
        }
        Ok((fx_uid, uid))
    })?;
    Ok(json!({"effect": fx_uid, "stroke": uid}))
}

/// Find a stroke group by uid or name across the layer's Paint effects.
fn remove_stroke(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "paint.removeStroke";
    let (cid, lid) = layer_p(s, p, cmd)?;
    let key = p.get("stroke").cloned().ok_or_else(|| bad(cmd, "missing `stroke` (uid or name)"))?;
    s.edit("Delete Stroke", None, |proj, st| {
        let l = layer_mut(proj, cid, lid)?;
        let fx = l.props.sub_mut("effects").ok_or_else(|| bad(cmd, "no effects"))?;
        for c in fx.children.iter_mut() {
            let Some(g) = c.as_group_mut() else { continue };
            if !matches!(&g.kind, GroupKind::Effect { effect } if effect == paint::ID) {
                continue;
            }
            let before = g.children.len();
            g.children.retain(|n| {
                let Some(sg) = n.as_group() else { return true };
                let hit = match &key {
                    Value::Number(u) => u.as_u64() == Some(sg.uid),
                    Value::String(name) => &sg.name == name,
                    _ => false,
                };
                !(hit && matches!(sg.match_id.as_str(), "brush" | "clone" | "eraser"))
            });
            if g.children.len() != before {
                st.selected_props.clear();
                return Ok(());
            }
        }
        Err(bad(cmd, "no such stroke"))
    })?;
    Ok(Value::Null)
}

fn presets(_: &mut Session, _: &Value) -> Result<Value> {
    Ok(json!(
        BRUSH_PRESETS
            .iter()
            .enumerate()
            .map(|(i, t)| json!({"index": i, "name": t.name, "diameter": t.diameter, "angle": t.angle, "roundness": t.roundness, "hardness": t.hardness, "spacing": t.spacing}))
            .collect::<Vec<_>>()
    ))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "paint.stroke",
            "Paint Stroke",
            [],
            None,
            "{layer?, kind?: brush|clone|eraser, points: [[x,y,pressure?],…] (effect space), time? (s) | frame?, duration? (s, drawing time for Write On), color?: [r,g,b,a], diameter?, angle?, hardness?, roundness?, spacing?, opacity?, flow?, mode?, channels?: RGBA|RGB|Alpha, durationMode?: constant|writeOn|singleFrame|custom, customFrames?, eraseMode?: layerSourceAndPaint|paintOnly|lastStrokeOnly, cloneSource?: layer id, clonePosition?: [x,y], cloneTimeShift? (s), lockSourceTime?, cloneTime? (s), sizePressure?, minSize?, opacityPressure?, flowPressure?}",
            has_comp,
            stroke
        ),
        cmd!(
            "paint.options",
            "Paint Options",
            [],
            None,
            "{color?, background?, opacity?, flow?, mode?, channels?, durationMode?, customFrames?, eraseMode?, diameter?, angle?, roundness?, hardness?, spacing?, preset?, sizePressure?, minSize?, opacityPressure?, flowPressure?, clonePreset?, cloneSource?, clonePoint?, cloneOffset?, aligned?, lockSourceTime?, cloneTimeShift?, cloneTime?, showOverlay?, overlayOpacity?, overlayDifference?, reset?}",
            always,
            options
        ),
        cmd!("paint.brushPreset", "Brush Preset", [], None, "{preset?: index | name}", always, preset),
        cmd!("paint.setCloneSource", "Set Clone Source", [], None, "{layer?, point: [x,y]}", has_comp, set_clone_source),
        cmd!("paint.removeStroke", "Delete Paint Stroke", [], None, "{layer?, stroke: uid | name}", has_comp, remove_stroke),
        crate::query!("paint.presets", "Brush Presets", "{}", presets),
    ]
}

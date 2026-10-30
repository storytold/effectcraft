//! Composition menu, part two: crop to region of interest / layer bounds, Save Frame As ▸ File,
//! Copy Frame to Clipboard, Responsive Design — Time.

use effectcraft_project::Marker;
use effectcraft_time::Tick;
use serde_json::{Value, json};

use super::{CommandSpec, always, b_p, bad, comp_id, f_p, has_comp, has_layers, layers_p, str_p};
use crate::prefs::CompPreset;
use crate::{EngineError, Result, Session, cmd, query};

fn has_roi(s: &Session) -> std::result::Result<(), String> {
    has_comp(s)?;
    if s.state.region_of_interest.is_some() { Ok(()) } else { Err("draw a region of interest in the viewer first".into()) }
}

/// Crop the comp to `[x, y, w, h]`: the comp shrinks and every unparented layer moves by -x, -y
/// (all position keys too) so nothing moves on screen.
fn crop(s: &mut Session, cid: effectcraft_project::ItemId, r: [f64; 4], label: &str) -> Result<Value> {
    let (x, y) = (r[0].round(), r[1].round());
    // At most 30,000 px a side, like any composition (#788).
    let (w, h) = (effectcraft_project::limits::size_f64(r[2]), effectcraft_project::limits::size_f64(r[3]));
    s.edit(label, None, |proj, st| {
        let c = proj.comp_mut(cid).ok_or(EngineError::NoComp)?;
        let (old_w, old_h) = (c.width, c.height);
        c.width = w;
        c.height = h;
        for l in c.layers.iter_mut_where(|l| l.parent.is_none()) {
            if let Some(tr) = l.transform_mut() {
                super::layer::offset_position(tr, -x, -y);
            }
        }
        for g in &mut c.guides {
            let px = g.px(old_w, old_h) - if g.vertical { x } else { y };
            g.set_px(px, w, h);
        }
        st.region_of_interest = None;
        Ok(())
    })?;
    Ok(json!({"width": w, "height": h, "offset": [x, y]}))
}

fn crop_to_roi(s: &mut Session, p: &Value) -> Result<Value> {
    let cid = comp_id(s, p)?;
    let r = s.state.region_of_interest.ok_or_else(|| bad("comp.cropToRegionOfInterest", "no region of interest"))?;
    crop(s, cid, r, "Crop Comp to Region of Interest")
}

fn crop_to_layers(s: &mut Session, p: &Value) -> Result<Value> {
    let (cid, ids) = layers_p(s, p)?;
    let t = s.time();
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    let ctx = effectcraft_render::EvalCtx::new(&s.project, cid, comp, t);
    let mut b = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
    for l in comp.layers.iter().filter(|l| ids.contains(&l.id)) {
        let Some([x0, y0, x1, y1]) = effectcraft_render::content_bounds(&ctx, l) else { continue };
        let (m, _) = ctx.layer_to_comp(l);
        for (px, py) in [(x0, y0), (x1, y0), (x1, y1), (x0, y1)] {
            let v = m.0;
            let wz = v[2][0] * px + v[2][1] * py + v[2][2];
            let wz = if wz.abs() < 1e-9 { 1.0 } else { wz };
            let cx = (v[0][0] * px + v[0][1] * py + v[0][2]) / wz;
            let cy = (v[1][0] * px + v[1][1] * py + v[1][2]) / wz;
            b = [b[0].min(cx), b[1].min(cy), b[2].max(cx), b[3].max(cy)];
        }
    }
    if b[0] > b[2] {
        return Err(bad("comp.cropToLayerBounds", "the selected layers have no visible bounds"));
    }
    let (x0, y0) = (b[0].floor(), b[1].floor());
    crop(s, cid, [x0, y0, b[2].ceil() - x0, b[3].ceil() - y0], "Crop Comp to Selected Layer(s) Bounds")
}

/// Encode straight 8-bit RGBA as PNG.
pub(crate) fn encode_png(rgba: &[u8], w: u32, h: u32) -> std::result::Result<Vec<u8>, String> {
    use image::ImageEncoder;
    let mut out = vec![];
    image::codecs::png::PngEncoder::new(&mut out).write_image(rgba, w, h, image::ExtendedColorType::Rgba8).map_err(|e| e.to_string())?;
    Ok(out)
}

/// Save Frame As ▸ File with `queue`: a Render Queue item for the current frame, from the Frame
/// Default templates (Edit ▸ Templates).
fn queue_frame(s: &mut Session, p: &Value) -> Result<Value> {
    use effectcraft_project::render_queue::{Channels, RenderQueueItem, TimeSpan};
    use effectcraft_project::render_templates::TemplateSlot;
    let cid = comp_id(s, p)?;
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    let t = f_p(p, "time").map(Tick::from_seconds_f64).unwrap_or(s.time());
    let mut it = RenderQueueItem::new(0, cid);
    it.settings = s.project.render_templates.default_render_settings(TemplateSlot::Still);
    it.settings.time_span = TimeSpan::Custom { start: t, end: t + comp.frame_duration() };
    it.output = s.project.render_templates.default_output_module(TemplateSlot::Still);
    if b_p(p, "transparent") == Some(false) {
        it.output.channels = Channels::Rgb;
    }
    if let Some(path) = str_p(p, "path") {
        it.output.output = path.to_string();
    } else if !it.output.format.is_sequence() {
        it.output.output = "[compName]_frame.[fileExtension]".into();
    }
    let id = s.edit("Save Frame As", None, |proj, _| {
        it.id = proj.render_queue.iter().map(|i| i.id).max().unwrap_or(0) + 1;
        let id = it.id;
        proj.render_queue.push(it);
        Ok(id)
    })?;
    let path = s.project.render_queue.last().and_then(|i| s.resolve_output(i));
    Ok(json!({"item": id, "queued": true, "output": path, "time": t.seconds()}))
}

fn save_frame(s: &mut Session, p: &Value) -> Result<Value> {
    if b_p(p, "queue") == Some(true) {
        return queue_frame(s, p);
    }
    let path = str_p(p, "path").ok_or_else(|| bad("comp.saveFrameAs", "missing `path` (.png)"))?.to_string();
    let cid = comp_id(s, p)?;
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    let t = f_p(p, "time").map(Tick::from_seconds_f64).unwrap_or(s.time());
    let scale = f_p(p, "scale").unwrap_or(1.0).clamp(0.01, 4.0);
    let bg = comp.background;
    let img = s.render(cid, t, effectcraft_render::RenderOpts { scale, backend: effectcraft_render::Backend::Auto, ..Default::default() });
    let rgba = if b_p(p, "transparent") == Some(false) { img.to_rgba8_over(bg) } else { img.to_rgba8() };
    let png = encode_png(&rgba, img.width, img.height).map_err(|e| EngineError::Other(format!("PNG encoding failed: {e}")))?;
    s.services.write_file(&path, &png).map_err(|e| EngineError::Other(format!("cannot write {path}: {e}")))?;
    s.toast(format!("Saved frame to {path}"));
    Ok(json!({"path": path, "width": img.width, "height": img.height, "time": t.seconds()}))
}

/// Composition ▸ Copy Frame to Clipboard (After Effects 26.3): render the frame as Save Frame As
/// ▸ File does and hand it to the frontend, which puts it on the system clipboard.
fn copy_frame(s: &mut Session, p: &Value) -> Result<Value> {
    let cid = comp_id(s, p)?;
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    let t = f_p(p, "time").map(Tick::from_seconds_f64).unwrap_or(s.time());
    let scale = f_p(p, "scale").unwrap_or(1.0).clamp(0.01, 4.0);
    let bg = comp.background;
    let img = s.render(cid, t, effectcraft_render::RenderOpts { scale, backend: effectcraft_render::Backend::Auto, ..Default::default() });
    let (width, height) = (img.width, img.height);
    let rgba = img.to_rgba8_over(bg);
    let expected = (width as usize).checked_mul(height as usize).and_then(|n| n.checked_mul(4));
    if width == 0 || height == 0 || expected != Some(rgba.len()) {
        return Err(EngineError::Other(format!("cannot copy an empty or malformed {width}×{height} frame")));
    }
    s.events.push(crate::Event::CopyImage { width, height, rgba });
    s.toast("Copied frame to the clipboard");
    Ok(json!({"width": width, "height": height, "time": t.seconds()}))
}

/// Responsive Design — Time: protected comp markers for an intro, an outro or the work area.
fn responsive_time(s: &mut Session, p: &Value) -> Result<Value> {
    let cid = comp_id(s, p)?;
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    let t = s.time();
    let (start, end, comment) = match str_p(p, "op").unwrap_or("workArea") {
        "intro" => (Tick::ZERO, t, "Intro"),
        "outro" => (t, comp.duration, "Outro"),
        "workArea" => (comp.work_area.0, comp.work_area.1, "Protected Region"),
        o => return Err(bad("comp.responsiveTime", format!("op: intro|outro|workArea, not `{o}`"))),
    };
    if end <= start {
        return Err(bad("comp.responsiveTime", "move the current time first (the region would be empty)"));
    }
    s.edit("Create Protected Region", None, |proj, _| {
        let c = proj.comp_mut(cid).ok_or(EngineError::NoComp)?;
        c.markers.push(Marker { time: start, duration: end - start, comment: comment.into(), protected: true, ..Default::default() });
        c.markers.sort_by_key(|m| m.time);
        Ok(())
    })?;
    Ok(json!({"start": start.seconds(), "end": end.seconds()}))
}

// ---------------------------------------------------------------- composition presets

/// `comp.savePreset`: remember a frame size, pixel aspect and frame rate under a name in the
/// settings (shown in Composition Settings ▸ Preset). A preset with the same name is replaced.
fn save_preset(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "comp.savePreset";
    let name = str_p(p, "name").map(str::trim).filter(|n| !n.is_empty()).ok_or_else(|| bad(C, "missing `name`"))?;
    let num = |k: &str| f_p(p, k).ok_or_else(|| bad(C, format!("missing `{k}`")));
    let dim = |k: &str| {
        num(k).and_then(|v| {
            if v.fract() == 0.0 && (0.0..=f64::from(u32::MAX)).contains(&v) { Ok(v as u32) } else { Err(bad(C, format!("`{k}` must be a whole number"))) }
        })
    };
    let preset =
        CompPreset { name: name.to_string(), width: dim("width")?, height: dim("height")?, pixel_aspect: num("pixelAspect")?, frame_rate: num("frameRate")? };
    if !preset.is_valid() {
        return Err(bad(
            C,
            format!(
                "not a valid preset: width and height 4–30000 px, pixel aspect 0.01–100, frame rate 1–999, a name of at most {} characters",
                CompPreset::MAX_NAME
            ),
        ));
    }
    let presets = &mut s.prefs.comp_presets;
    let replaced = match presets.iter_mut().find(|q| q.name == preset.name) {
        Some(q) => {
            *q = preset;
            true
        }
        None => {
            presets.push(preset);
            false
        }
    };
    s.save_prefs();
    s.prefs_revision += 1;
    Ok(json!({"name": name, "replaced": replaced}))
}

/// `comp.deletePreset`: forget a saved composition preset.
fn delete_preset(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "comp.deletePreset";
    let name = str_p(p, "name").ok_or_else(|| bad(C, "missing `name`"))?;
    let before = s.prefs.comp_presets.len();
    s.prefs.comp_presets.retain(|q| q.name != name);
    if s.prefs.comp_presets.len() == before {
        return Err(bad(C, format!("no saved preset named `{name}`")));
    }
    s.save_prefs();
    s.prefs_revision += 1;
    Ok(Value::Null)
}

/// `comp.presets`: the saved composition presets.
fn list_presets(s: &mut Session, _: &Value) -> Result<Value> {
    serde_json::to_value(&s.prefs.comp_presets).map_err(|e| bad("comp.presets", e.to_string()))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!("comp.cropToRegionOfInterest", "Crop Comp to Region of Interest", ["Composition"], None, "{comp?}", has_roi, crop_to_roi),
        cmd!("comp.cropToLayerBounds", "Crop Comp to Selected Layer(s) Bounds", ["Composition"], None, "{layers?}", has_layers, crop_to_layers),
        cmd!(
            "comp.saveFrameAs",
            "File...",
            ["Composition", "Save Frame As"],
            Some("Cmd+Alt+S"),
            "{path (.png; with queue: an output path or template), time?, scale?, queue?: bool (add a Render Queue item from the Frame Default templates instead of writing now), transparent?: bool (default true)}",
            has_comp,
            save_frame
        ),
        cmd!("comp.copyFrame", "Copy Frame to Clipboard", ["Composition"], None, "{comp?, time?, scale?}", has_comp, copy_frame),
        cmd!(
            "comp.savePreset",
            "Save Composition Preset",
            [],
            None,
            "{name, width, height, pixelAspect, frameRate} (replaces a saved preset with the same name)",
            always,
            save_preset
        ),
        cmd!("comp.deletePreset", "Delete Composition Preset", [], None, "{name}", always, delete_preset),
        query!("comp.presets", "Composition Presets", "{}", list_presets),
        cmd!("comp.responsiveTime", "Responsive Design — Time", [], None, "{op: intro|outro|workArea}", has_comp, responsive_time),
    ]
}

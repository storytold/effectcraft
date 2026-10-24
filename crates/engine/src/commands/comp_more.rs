//! Composition menu, part two: crop to region of interest / layer bounds, Scale Composition, Save
//! Frame As ▸ File, Copy Frame to Clipboard, Responsive Design — Time.

use effectcraft_project::Marker;
use effectcraft_time::Tick;
use serde_json::{Value, json};

use super::{CommandSpec, always, bad, comp_id, f_p, has_comp, has_layers, layers_p, str_p};
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
        for l in c.layers.iter_mut().filter(|l| l.parent.is_none()) {
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

/// Multiply a property's value, every key, its spatial tangents and its speeds by `f`.
fn scale_prop(pr: &mut effectcraft_project::Property, f: f64) {
    use effectcraft_keyframe::Value as KV;
    let mul = |v: &KV| match v {
        KV::Scalar(x) => KV::Scalar(x * f),
        KV::Vec2(a) => KV::Vec2(a.map(|x| x * f)),
        KV::Vec3(a) => KV::Vec3(a.map(|x| x * f)),
        other => other.clone(),
    };
    pr.value = mul(&pr.value);
    for k in &mut pr.keys {
        k.value = mul(&k.value);
        k.spatial_in = k.spatial_in.map(|x| x * f);
        k.spatial_out = k.spatial_out.map(|x| x * f);
        for e in k.in_ease.iter_mut().chain(&mut k.out_ease) {
            e.speed *= f;
        }
    }
}

/// Multiply only the Z of a 3D value (and of its keys and spatial tangents) by `f`. Speeds follow:
/// a spatial side's speed by how much the path's direction there stretches, so the ease keeps
/// its shape; a per-dimension one (Scale) by `f` for Z only.
fn scale_prop_z(pr: &mut effectcraft_project::Property, f: f64) {
    use effectcraft_keyframe::Value as KV;
    let stretch = |v: [f64; 3]| {
        let n = v[0].hypot(v[1]).hypot(v[2]);
        if n > 1e-12 && n.is_finite() { v[0].hypot(v[1]).hypot(v[2] * f) / n } else { 1.0 }
    };
    let pos = |k: &effectcraft_project::Keyframe| k.value.as_vec3();
    let sub = |a: [f64; 3], b: [f64; 3]| [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    let is_zero = |v: [f64; 3]| v.iter().all(|x| x.abs() <= 1e-12);
    // Read every side's direction before anything changes (auto tangents come from the neighbours).
    let sides: Vec<(f64, f64)> = (0..pr.keys.len())
        .map(|i| {
            let (tin, tout) = effectcraft_keyframe::spatial_tangents(&pr.keys, i);
            let here = pr.keys.get(i).map(pos).unwrap_or_default();
            let chord = |j: Option<usize>| j.and_then(|j| pr.keys.get(j)).map(|k| sub(pos(k), here)).unwrap_or_default();
            let tin = if is_zero(tin) { chord(i.checked_sub(1)) } else { tin };
            let tout = if is_zero(tout) { chord(Some(i + 1)) } else { tout };
            (stretch(tin), stretch(tout))
        })
        .collect();
    let spatial = pr.spatial;
    let mul = |v: &mut KV| {
        if let KV::Vec3(a) = v {
            a[2] *= f;
        }
    };
    mul(&mut pr.value);
    for (k, (s_in, s_out)) in pr.keys.iter_mut().zip(sides) {
        mul(&mut k.value);
        k.spatial_in[2] *= f;
        k.spatial_out[2] *= f;
        if spatial {
            k.in_ease.iter_mut().for_each(|e| e.speed *= s_in);
            k.out_ease.iter_mut().for_each(|e| e.speed *= s_out);
        } else {
            // A shorter list lends its first ease to Z (`effective_ease`): give Z its own.
            for list in [&mut k.in_ease, &mut k.out_ease] {
                if let Some(&first) = list.first() {
                    list.resize(list.len().max(3), first);
                }
                if let Some(e) = list.get_mut(2) {
                    e.speed *= f;
                }
            }
        }
    }
}

/// Scale Composition (After Effects ships it as File ▸ Scripts ▸ Scale Composition.jsx): the comp
/// and its contents grow or shrink by one factor, keys included, in one undo step. Layers whose
/// parent has a Scale follow that parent; everything else scales its Position (and Point of
/// Interest) and Scale. Anchor Point, masks, shapes and effects live in layer space and follow the
/// layer's Scale. Cameras scale their Zoom so 3D layers keep their place in the frame; a 2D parent
/// carries no depth, so its 3D children scale their Z themselves.
fn scale_comp(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "comp.scale";
    let cid = comp_id(s, p)?;
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    let (ow, oh) = (f64::from(comp.width), f64::from(comp.height));
    let f = match (f_p(p, "factor"), f_p(p, "width"), f_p(p, "height")) {
        (Some(f), ..) => f,
        (None, Some(w), _) => w / ow,
        (None, None, Some(h)) => h / oh,
        _ => return Err(bad(C, "pass `factor`, `width` or `height`")),
    };
    let (w, h) = ((ow * f).round(), (oh * f).round());
    let max = f64::from(effectcraft_project::limits::MAX_SIZE);
    if !f.is_finite() || f <= 0.0 || !(4.0..=max).contains(&w) || !(4.0..=max).contains(&h) {
        return Err(bad(C, format!("the composition would be {w}×{h}: width and height run from 4 to 30,000 px")));
    }
    let (w, h) = (w as u32, h as u32);
    let active = s.active_comp_id() == Some(cid);
    s.edit("Scale Composition", None, |proj, st| {
        let c = proj.comp_mut(cid).ok_or(EngineError::NoComp)?;
        // An ancestor's Scale carries its descendants (X and Y only when that ancestor is 2D),
        // through cameras and lights too: the renderer applies every ancestor's transform.
        let chain: std::collections::HashMap<_, _> =
            c.layers.iter().map(|l| (l.id, (l.transform().is_some_and(|tr| tr.get("scale").is_some()), l.is_3d(), l.parent))).collect();
        // (X and Y carried, Z carried); bounded, as parent chains in a file can loop.
        let carried = |mut cur: Option<effectcraft_project::LayerId>| {
            let (mut xy, mut z) = (false, false);
            for _ in 0..64 {
                let Some(&(has_scale, three_d, up)) = cur.and_then(|id| chain.get(&id)) else { break };
                xy |= has_scale;
                z |= has_scale && three_d;
                cur = up;
            }
            (xy, z)
        };
        for l in &mut c.layers {
            let (xy_carried, z_carried) = carried(l.parent);
            let three_d = l.is_3d();
            let Some(tr) = l.transform_mut() else { continue };
            if !xy_carried {
                for name in ["position", "positionX", "positionY", "positionZ", "poi", "scale"] {
                    if let Some(pr) = tr.get_mut(name) {
                        scale_prop(pr, f);
                    }
                }
            } else if three_d && !z_carried {
                if let Some(pr) = tr.get_mut("positionZ") {
                    scale_prop(pr, f);
                }
                for name in ["position", "poi", "scale"] {
                    if let Some(pr) = tr.get_mut(name) {
                        scale_prop_z(pr, f);
                    }
                }
            }
            if let Some(cam) = l.props.sub_mut("cameraOptions") {
                for name in ["zoom", "focusDistance", "aperture"] {
                    if let Some(pr) = cam.get_mut(name) {
                        scale_prop(pr, f);
                    }
                }
            }
        }
        // Percentage guides keep their percent.
        for g in c.guides.iter_mut().filter(|g| !g.percent) {
            g.position *= f;
        }
        c.width = w;
        c.height = h;
        if active && let Some(r) = &mut st.region_of_interest {
            *r = r.map(|v| v * f);
        }
        Ok(())
    })?;
    Ok(json!({"width": w, "height": h, "factor": f}))
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
    use effectcraft_project::render_queue::{RenderQueueItem, TimeSpan};
    use effectcraft_project::render_templates::TemplateSlot;
    let cid = comp_id(s, p)?;
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    let t = f_p(p, "time").map(Tick::from_seconds_f64).unwrap_or(s.time());
    let mut it = RenderQueueItem::new(0, cid);
    it.settings = s.project.render_templates.default_render_settings(TemplateSlot::Still);
    it.settings.time_span = TimeSpan::Custom { start: t, end: t + comp.frame_duration() };
    it.output = s.project.render_templates.default_output_module(TemplateSlot::Still);
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
    if p.get("queue").and_then(Value::as_bool) == Some(true) {
        return queue_frame(s, p);
    }
    let path = str_p(p, "path").ok_or_else(|| bad("comp.saveFrameAs", "missing `path` (.png)"))?.to_string();
    let cid = comp_id(s, p)?;
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    let t = f_p(p, "time").map(Tick::from_seconds_f64).unwrap_or(s.time());
    let scale = f_p(p, "scale").unwrap_or(1.0).clamp(0.01, 4.0);
    let bg = comp.background;
    let img = s.render(cid, t, effectcraft_render::RenderOpts { scale, backend: effectcraft_render::Backend::Auto, ..Default::default() });
    let rgba = img.to_rgba8_over(bg);
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
            "comp.scale",
            "Scale Composition",
            [],
            None,
            "{comp?, factor? (2 = twice the size) | width? | height? (the other side keeps the aspect ratio)}",
            has_comp,
            scale_comp
        ),
        cmd!(
            "comp.saveFrameAs",
            "File...",
            ["Composition", "Save Frame As"],
            Some("Cmd+Alt+S"),
            "{path (.png; with queue: an output path or template), time?, scale?, queue?: bool (add a Render Queue item from the Frame Default templates instead of writing now)}",
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

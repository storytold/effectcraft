//! Composition ▸ Save Frame As ▸ Photoshop Layers… and ProEXR…: the current frame with every
//! visible layer kept apart.
//!
//! - **Photoshop Layers** writes a layered PSD (crates/psd's writer): one pixel layer per comp
//!   layer, each rendered in isolation (solo, Normal mode, 100% opacity) and cropped to its
//!   pixels, carrying the layer's blend mode and opacity, plus the flattened frame as the merged
//!   image. Photoshop (and EffectCraft's PSD import) rebuild the frame from the layers.
//! - **ProEXR** writes one multi-layer OpenEXR (half-float-free, 32-bit float, linear light,
//!   premultiplied): the composite as `R`, `G`, `B`, `A` plus one `<layer name>.R/G/B/A` channel
//!   set per layer (the layer-prefixed channel convention multi-layer EXR readers use).

use std::collections::{HashMap, HashSet};

use effectcraft_color::BlendMode;
use effectcraft_project::{ItemId, LayerId, Project};
use effectcraft_raster::Image;
use effectcraft_render::{Backend, RenderOpts, Renderer};
use effectcraft_time::Tick;
use serde_json::{Value, json};

use super::{CommandSpec, bad, comp_id, f_p, has_comp, str_p};
use crate::{EngineError, Result, Session, cmd};

/// One isolated layer of a frame: name, blend mode, opacity (0..1), premultiplied pixels.
struct Isolated {
    name: String,
    blend: BlendMode,
    opacity: f64,
    image: Image,
}

/// `skip_hdr_step`: no HDR step, for a float output (ProEXR; [`RenderOpts::skip_hdr_step`]).
fn render_with(s: &Session, project: &Project, comp: ItemId, t: Tick, scale: f64, skip_hdr_step: bool) -> Image {
    let opts = RenderOpts {
        scale,
        backend: Backend::Auto,
        nested_switches: s.prefs.general.switches_affect_nested_comps,
        draft_shadows: s.prefs.three_d.realtime_shadows,
        skip_hdr_step,
        ..Default::default()
    };
    let mut r = Renderer::new(project, s.footage.as_ref(), opts);
    r.expr = s.expr.as_deref();
    r.accel = s.accel.as_deref();
    r.comp_frame(comp, t)
}

/// The comp frame and each visible layer rendered alone (top layer first). `skip_hdr_step`: as
/// [`render_with`].
fn isolate(s: &Session, cid: ItemId, t: Tick, scale: f64, skip_hdr_step: bool) -> Result<(Image, Vec<Isolated>)> {
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    let full = render_with(s, &s.project, cid, t, scale, skip_hdr_step);
    let any_solo = comp.layers.iter().any(|l| l.switches.solo && l.has_video());
    let ids: Vec<LayerId> = comp
        .layers
        .iter()
        .filter(|l| l.has_video() && l.is_active_at(t) && !l.switches.adjustment && !l.switches.guide && !l.environment && (!any_solo || l.switches.solo))
        .map(|l| l.id)
        .collect();
    let mut out = vec![];
    for id in ids {
        let mut p: Project = (*s.project).clone();
        let Some(c) = p.comp_mut(cid) else { continue };
        let mut info = None;
        for l in c.layers.iter_mut() {
            l.switches.solo = l.id == id;
            if l.id == id {
                let lt = l.layer_time(t);
                let opacity = l.props.prop("transform/opacity").map(|o| o.value_at(lt).as_f64() / 100.0).unwrap_or(1.0);
                info = Some((l.name.clone(), l.blend_mode, opacity.clamp(0.0, 1.0)));
                l.blend_mode = BlendMode::Normal;
                if let Some(o) = l.props.prop_mut("transform/opacity") {
                    o.keys.clear();
                    o.expr = None;
                    o.value = effectcraft_keyframe::Value::Scalar(100.0);
                }
            }
        }
        let Some((name, blend, opacity)) = info else { continue };
        let image = render_with(s, &p, cid, t, scale, skip_hdr_step);
        out.push(Isolated { name, blend, opacity, image });
    }
    Ok((full, out))
}

fn straight(p: [f32; 4]) -> [f32; 4] {
    if p[3] <= 0.0 { [0.0; 4] } else { [p[0] / p[3], p[1] / p[3], p[2] / p[3], p[3].min(1.0)] }
}

/// The Photoshop key of a blend mode (Normal for modes Photoshop lacks).
pub fn psd_blend_key(m: BlendMode) -> [u8; 4] {
    const KEYS: [&[u8; 4]; 26] = [
        b"diss", b"dark", b"mul ", b"idiv", b"lbrn", b"dkCl", b"lite", b"scrn", b"div ", b"lddg", b"lgCl", b"over", b"sLit", b"hLit", b"vLit", b"lLit",
        b"pLit", b"hMix", b"diff", b"smud", b"fsub", b"fdiv", b"hue ", b"sat ", b"colr", b"lum ",
    ];
    KEYS.iter().find(|k| crate::psd_import::blend_mode(std::str::from_utf8(&k[..]).unwrap_or("")) == m).map(|k| **k).unwrap_or(*b"norm")
}

/// The bounding box of non-transparent pixels (`None` when empty).
fn alpha_bounds(img: &Image) -> Option<(u32, u32, u32, u32)> {
    let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
    for y in 0..img.height {
        for x in 0..img.width {
            if img.data[(y * img.width + x) as usize][3] > 0.5 / 255.0 {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x + 1);
                y1 = y1.max(y + 1);
            }
        }
    }
    (x1 > x0).then_some((x0, y0, x1, y1))
}

/// The layered PSD bytes of a frame.
pub fn layered_psd(s: &Session, cid: ItemId, t: Tick, scale: f64) -> Result<(Vec<u8>, usize, u32, u32)> {
    use effectcraft_psd::Rect;
    use effectcraft_psd::write::{WDoc, WLayer, write};
    let (full, layers) = isolate(s, cid, t, scale, false)?;
    let mut doc = WDoc::new(full.width, full.height);
    let n = layers.len();
    // File order is bottom first.
    for l in layers.into_iter().rev() {
        let mut w = match alpha_bounds(&l.image) {
            Some((x0, y0, x1, y1)) => {
                let mut px = Vec::with_capacity(((x1 - x0) * (y1 - y0)) as usize);
                for y in y0..y1 {
                    for x in x0..x1 {
                        px.push(straight(l.image.data[(y * l.image.width + x) as usize]));
                    }
                }
                WLayer::pixels(&l.name, Rect::new(x0 as i32, y0 as i32, (x1 - x0) as i32, (y1 - y0) as i32), px)
            }
            None => WLayer::pixels(&l.name, Rect::default(), vec![]),
        };
        w.blend = psd_blend_key(l.blend);
        w.opacity = (l.opacity * 255.0).round() as u8;
        doc.layers.push(w);
    }
    doc.composite = Some(full.data.iter().map(|p| straight(*p)).collect());
    Ok((write(&doc), n, full.width, full.height))
}

/// A unique EXR layer name (no dots: they separate the layer from the channel). Printable ASCII only:
/// the `exr` crate panics on characters it cannot store in a channel name (#1702).
/// `used` holds the names given so far, `next` the suffix each base's search resumes at: many
/// layers of one name take a name each without retrying every suffix before theirs.
fn exr_name(name: &str, used: &mut HashSet<String>, next: &mut HashMap<String, usize>) -> String {
    let base: String = name.chars().map(|c| if c == '.' || !(c == ' ' || c.is_ascii_graphic()) { '_' } else { c }).collect();
    let base = if base.trim().is_empty() { "Layer".to_string() } else { base };
    if used.insert(base.clone()) {
        return base;
    }
    let k = next.entry(base.clone()).or_insert(2);
    loop {
        let n = format!("{base} {k}");
        *k += 1;
        if used.insert(n.clone()) {
            return n;
        }
    }
}

/// The multi-layer EXR bytes of a frame and the channel names.
pub fn layered_exr(s: &Session, cid: ItemId, t: Tick, scale: f64) -> Result<(Vec<u8>, Vec<String>, u32, u32)> {
    use exr::prelude::*;
    // The composite and every layer without the HDR step, decoded with the output space's
    // curve (linear light in the output space's primaries).
    let (full, layers) = isolate(s, cid, t, scale, true)?;
    let pipe = effectcraft_render::color::Pipe::of(&s.project.settings);
    let dec = pipe.output_linear();
    let (w, h) = (full.width as usize, full.height as usize);
    let planes = |img: &effectcraft_raster::Image| -> [Vec<f32>; 4] {
        let mut out: [Vec<f32>; 4] = Default::default();
        for p in &img.data {
            let sp = straight(*p);
            let a = sp[3];
            let lin = dec.apply([sp[0].max(0.0), sp[1].max(0.0), sp[2].max(0.0)]);
            for c in 0..3 {
                out[c].push(lin[c] * a);
            }
            out[3].push(a);
        }
        out
    };
    let mut channels = vec![];
    let mut names = vec![];
    let mut push = |prefix: &str, img: &effectcraft_raster::Image| {
        for (c, data) in ["R", "G", "B", "A"].iter().zip(planes(img)) {
            let n = if prefix.is_empty() { c.to_string() } else { format!("{prefix}.{c}") };
            names.push(n.clone());
            channels.push(AnyChannel::new(n.as_str(), FlatSamples::F32(data)));
        }
    };
    push("", &full);
    let (mut used, mut next) = (HashSet::new(), HashMap::new());
    for l in &layers {
        let n = exr_name(&l.name, &mut used, &mut next);
        push(&n, &l.image);
    }
    let layer = Layer::new((w, h), LayerAttributes::default(), Encoding::SMALL_LOSSLESS, AnyChannels::sort(channels.into()));
    let mut buf = std::io::Cursor::new(Vec::new());
    let mut image = Image::from_layer(layer);
    // The primaries and white the values are in (the output space's), for Nuke, AE and Blender.
    let [red, green, blue, white] = pipe.output_linear_space().exr_chromaticities().map(|[x, y]| Vec2(x, y));
    image.attributes.chromaticities = Some(exr::meta::attribute::Chromaticities { red, green, blue, white });
    image.write().to_buffered(&mut buf).map_err(|e| EngineError::Other(format!("EXR encoding failed: {e}")))?;
    Ok((buf.into_inner(), names, full.width, full.height))
}

fn frame_args(s: &Session, p: &Value, cmd: &str, ext: &str) -> Result<(String, ItemId, Tick, f64)> {
    let path = str_p(p, "path").ok_or_else(|| bad(cmd, format!("missing `path` (.{ext})")))?.to_string();
    s.refuse_export(&path)?;
    let cid = comp_id(s, p)?;
    let t = f_p(p, "time").map(Tick::from_seconds_f64).unwrap_or(s.time());
    let scale = f_p(p, "scale").unwrap_or(1.0).clamp(0.01, 4.0);
    Ok((path, cid, t, scale))
}

fn save_psd(s: &mut Session, p: &Value) -> Result<Value> {
    let (path, cid, t, scale) = frame_args(s, p, "comp.saveFrameAsPsd", "psd")?;
    let (bytes, n, w, h) = layered_psd(s, cid, t, scale)?;
    s.write_output(&path, &bytes)?;
    s.toast(format!("Saved {n} layers to {path}"));
    Ok(json!({"path": path, "layers": n, "width": w, "height": h, "time": t.seconds()}))
}

fn save_exr(s: &mut Session, p: &Value) -> Result<Value> {
    let (path, cid, t, scale) = frame_args(s, p, "comp.saveFrameAsExr", "exr")?;
    let (bytes, channels, w, h) = layered_exr(s, cid, t, scale)?;
    s.write_output(&path, &bytes)?;
    s.toast(format!("Saved a multi-layer EXR to {path}"));
    Ok(json!({"path": path, "channels": channels, "width": w, "height": h, "time": t.seconds()}))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "comp.saveFrameAsPsd",
            "Photoshop Layers...",
            ["Composition", "Save Frame As"],
            None,
            "{path (.psd), comp?, time?, scale?} → a layered PSD: one layer per visible comp layer (blend mode, opacity) + the merged frame",
            has_comp,
            save_psd
        ),
        cmd!(
            "comp.saveFrameAsExr",
            "ProEXR...",
            ["Composition", "Save Frame As"],
            None,
            "{path (.exr), comp?, time?, scale?} → a multi-layer OpenEXR: composite R,G,B,A + `<layer>.R/G/B/A` per layer (linear, premultiplied)",
            has_comp,
            save_exr
        ),
    ]
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};

    use super::exr_name;

    #[test]
    fn many_layers_of_one_name_take_a_name_each_in_order() {
        let (mut used, mut next) = (HashSet::new(), HashMap::new());
        // `A 3` is taken by a layer of its own before the repeats reach it.
        let names: Vec<String> = ["A", "A", "A 3", "A", "A.b"].iter().map(|n| exr_name(n, &mut used, &mut next)).collect();
        assert_eq!(names, ["A", "A 2", "A 3", "A 4", "A_b"]);
        // 20 000 layers of one name: each found without retrying the suffixes before it.
        for _ in 0..20_000 {
            exr_name("Same", &mut used, &mut next);
        }
        assert_eq!(next["Same"], 20_001);
    }
}

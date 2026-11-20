//! Composition ▸ Save Frame As ▸ Photoshop Layers… and ProEXR…: the current frame with every
//! visible layer kept apart.
//!
//! - **Photoshop Layers** writes a layered PSD (crates/psd's writer): one pixel layer per comp
//!   layer, each rendered in isolation (solo, Normal mode, 100% opacity) and cropped to its
//!   pixels, carrying the layer's blend mode and opacity, plus the flattened frame as the merged
//!   image. Photoshop (and EffectCraft's PSD import) rebuild the frame from the layers.
//! - **ProEXR** writes one multi-layer OpenEXR with 32-bit float channels. By default it
//!   includes the composite and every visible layer as linear, premultiplied RGBA. A caller can
//!   select channel groups, include camera-space depth (`Z`), and write each group as a named part.

use std::collections::HashSet;

use effectcraft_color::BlendMode;
use effectcraft_export::exr_out::{Channels, isolate_layer, layer_names};
use effectcraft_project::render_queue::{ExrBitDepth, ExrCompression, ExrSelection};
use effectcraft_project::{Comp, ItemId, Layer, Project};
use effectcraft_raster::Image;
use effectcraft_render::{Backend, RenderOpts, Renderer, frame_size};
use effectcraft_time::Tick;
use serde::Deserialize;
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
fn renderer<'a>(s: &'a Session, project: &'a Project, scale: f64, skip_hdr_step: bool) -> Renderer<'a> {
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
    r
}

fn visible_layers(comp: &Comp, t: Tick) -> impl Iterator<Item = &Layer> {
    let any_solo = comp.layers.iter().any(|l| l.switches.solo && l.has_video());
    comp.layers.iter().filter(move |l| ExrSelection::can_export_layer(l) && l.is_active_at(t) && (!any_solo || l.switches.solo))
}

/// The comp frame and each visible layer rendered alone (top layer first).
fn isolate(s: &Session, cid: ItemId, t: Tick, scale: f64) -> Result<(Image, Vec<Isolated>)> {
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    let full = renderer(s, &s.project, scale, false).comp_frame(cid, t);
    let layers = visible_layers(comp, t)
        .filter_map(|l| isolate_layer(&s.project, cid, l.id, t))
        .map(|l| {
            let image = renderer(s, &l.project, scale, false).comp_frame(cid, t);
            Isolated { name: l.name, blend: l.blend, opacity: l.opacity, image }
        })
        .collect();
    Ok((full, layers))
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
    let (full, layers) = isolate(s, cid, t, scale)?;
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

const SAVE_EXR: &str = "comp.saveFrameAsExr";

/// The multi-layer EXR bytes of a frame and the channel names.
fn layered_exr(s: &Session, cid: ItemId, t: Tick, scale: f64, selection: Option<&ExrSelection>) -> Result<(Vec<u8>, Vec<String>, u32, u32)> {
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    if let Some(sel) = selection {
        sel.validate(comp).map_err(|e| bad(SAVE_EXR, e))?;
    }
    let r = renderer(s, &s.project, scale, true);
    let pipe = effectcraft_render::color::Pipe::of(&s.project.settings);
    let dec = pipe.output_linear();
    let (w, h) = frame_size(comp.width, comp.height, scale);
    let multipart = selection.is_some_and(|sel| sel.multipart);
    let mut channels = if multipart { Channels::new_multipart((w, h)) } else { Channels::new((w, h)) };
    let composite = selection.is_none_or(|sel| sel.composite);
    let depth = selection.is_some_and(|sel| sel.depth);
    let selected = selection.map(|sel| sel.layer_ids(comp).into_iter().collect::<HashSet<_>>());
    let layer_count = selected.as_ref().map_or_else(|| visible_layers(comp, t).count(), HashSet::len) as u64;
    let channel_count = layer_count.saturating_add(u64::from(composite)).saturating_mul(4).saturating_add(u64::from(depth));
    channels.check_channel_count(channel_count).map_err(|e| EngineError::Other(e.to_string()))?;
    let mut push_color = |prefix: &str, mut img: Image| -> Result<()> {
        if (img.width, img.height) != (w, h) {
            return Err(bad(SAVE_EXR, "rendered color dimensions differ from the OpenEXR frame"));
        }
        // The render skipped the HDR step. Decode the output curve in straight color before
        // premultiplying again, leaving values above 1 intact.
        for p in &mut img.data {
            let sp = straight(*p);
            let a = sp[3];
            let [r, g, b] = dec.apply([sp[0].max(0.0), sp[1].max(0.0), sp[2].max(0.0)]);
            *p = [r * a, g * a, b * a, a];
        }
        channels.push_rgba(prefix, img.data, ExrBitDepth::Float).map_err(|e| EngineError::Other(e.to_string()))
    };
    if composite {
        push_color("", r.comp_frame(cid, t))?;
    }
    // Explicit selection gets stable names from the eligible stack, including layers that
    // sit outside this frame's time or selection. Preserve legacy names for the old command.
    let names = match selection {
        Some(_) => layer_names(comp.layers.iter().filter(|l| ExrSelection::can_export_layer(l)), multipart),
        None => layer_names(visible_layers(comp, t), multipart),
    };
    for l in &comp.layers {
        if selected.as_ref().is_none_or(|ids| ids.contains(&l.id))
            && let Some(prefix) = names.get(&l.id)
        {
            let isolated = isolate_layer(&s.project, cid, l.id, t).ok_or_else(|| bad(SAVE_EXR, "missing OpenEXR layer"))?;
            push_color(prefix, renderer(s, &isolated.project, scale, true).comp_frame(cid, t))?;
        }
    }
    if depth {
        let aux = r.comp_aux(cid, t).ok_or_else(|| bad(SAVE_EXR, "composition depth is unavailable"))?;
        if (aux.width, aux.height) != (w, h) {
            return Err(bad(SAVE_EXR, "rendered depth dimensions differ from the OpenEXR frame"));
        }
        let depth = aux
            .channels
            .into_iter()
            .find_map(|(name, values)| (name == "Z").then_some(values))
            .ok_or_else(|| bad(SAVE_EXR, "composition depth is unavailable"))?;
        channels.push_depth(depth).map_err(|e| EngineError::Other(e.to_string()))?;
    }
    let mut buf = std::io::Cursor::new(Vec::new());
    let chroma = Some(pipe.output_linear_space().exr_chromaticities());
    channels.write(&mut buf, ExrCompression::Zip, chroma).map_err(|e| EngineError::Other(e.to_string()))?;
    Ok((buf.into_inner(), channels.names().map(str::to_owned).collect(), w, h))
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
    let (path, cid, t, scale) = frame_args(s, p, SAVE_EXR, "exr")?;
    let selection = p
        .get("exrSelection")
        .map(|value| {
            let object = value.as_object().ok_or_else(|| bad(SAVE_EXR, "exrSelection must be an object"))?;
            ExrSelection::deserialize(object).map_err(|e| bad(SAVE_EXR, format!("invalid exrSelection: {e}")))
        })
        .transpose()?;
    let (bytes, channels, w, h) = layered_exr(s, cid, t, scale, selection.as_ref())?;
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
            "{path (.exr), comp?, time?, scale?, exrSelection?: {composite?, allLayers?: bool, layers?: [layer IDs], depth?, multipart?}} → OpenEXR (linear premultiplied RGBA, camera-space Z). Multipart writes named parts. Omit selection for composite and all visible layers.",
            has_comp,
            save_exr
        ),
    ]
}

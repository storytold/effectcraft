//! Layer ▸ Auto-trace…: masks from a layer's alpha, a colour channel or its luminance, at the
//! current frame or over the work area (one Mask Path key per frame). The tracing is
//! `effectcraft_path::trace` (marching squares, Douglas–Peucker, Bezier fitting).

use effectcraft_color::Label;
use effectcraft_keyframe::{Interp, Keyframe};
use effectcraft_path::trace::{TraceOpts, Traced, trace};
use effectcraft_project::build::{self, Ids};
use effectcraft_project::{ItemKind, LayerSource, MaskMode, Solid, Value as KV};
use effectcraft_raster::Image;
use effectcraft_render::{RenderOpts, Renderer};
use effectcraft_time::Tick;
use serde_json::{Value, json};

use super::{CommandSpec, b_p, bad, f_p, has_layers, layer_p, str_p};
use crate::{EngineError, Result, Session, cmd};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Channel {
    Alpha,
    Red,
    Green,
    Blue,
    Luminance,
}

impl Channel {
    fn from_name(s: &str) -> Option<Channel> {
        Some(match s.to_ascii_lowercase().as_str() {
            "alpha" | "a" => Channel::Alpha,
            "red" | "r" => Channel::Red,
            "green" | "g" => Channel::Green,
            "blue" | "b" => Channel::Blue,
            "luminance" | "luma" | "l" => Channel::Luminance,
            _ => return None,
        })
    }
}

/// The traced scalar field of a frame (straight channel values, 0…1).
pub fn channel_field(img: &Image, ch: Channel, invert: bool) -> Vec<f32> {
    img.data
        .iter()
        .map(|p| {
            let a = p[3];
            let s = |v: f32| if a > 1e-6 { v / a } else { 0.0 };
            let v = match ch {
                Channel::Alpha => a,
                Channel::Red => s(p[0]),
                Channel::Green => s(p[1]),
                Channel::Blue => s(p[2]),
                Channel::Luminance => 0.2126 * s(p[0]) + 0.7152 * s(p[1]) + 0.0722 * s(p[2]),
            }
            .clamp(0.0, 1.0);
            if invert { 1.0 - v } else { v }
        })
        .collect()
}

/// Trace one frame: outlines in layer space.
fn trace_frame(img: &Image, offset: [f64; 2], ch: Channel, invert: bool, blur: f64, o: &TraceOpts) -> Vec<Traced> {
    let src = if blur > 0.0 { effectcraft_raster::gaussian_blur(img, blur / 2.0, blur / 2.0, false) } else { img.clone() };
    let field = channel_field(&src, ch, invert);
    let mut out = trace(&field, img.width as usize, img.height as usize, o);
    for t in &mut out {
        for v in &mut t.path.vertices {
            v[0] += offset[0];
            v[1] += offset[1];
        }
    }
    out
}

fn run(s: &mut Session, p: &Value) -> Result<Value> {
    let c = "layer.autoTrace";
    let (cid, lid) = layer_p(s, p, c)?;
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?.clone();
    let layer = comp.layer(lid).ok_or(EngineError::NoComp)?.clone();
    if !layer.source.is_av() {
        return Err(bad(c, "auto-trace needs a footage, solid, precomp, text or shape layer"));
    }
    let ch = match str_p(p, "channel") {
        Some(x) => Channel::from_name(x).ok_or_else(|| bad(c, "channel: alpha|red|green|blue|luminance"))?,
        None => Channel::Alpha,
    };
    let invert = b_p(p, "invert").unwrap_or(false);
    let blur = f_p(p, "blur").unwrap_or(1.0).max(0.0);
    let o = TraceOpts {
        threshold: (f_p(p, "threshold").unwrap_or(50.0) / 100.0).clamp(0.0, 1.0) as f32,
        tolerance: f_p(p, "tolerance").unwrap_or(1.0).max(0.01),
        min_area: f_p(p, "minimumArea").unwrap_or(10.0).max(0.0),
        corner_roundness: (f_p(p, "cornerRoundness").unwrap_or(50.0) / 100.0).clamp(0.0, 1.0),
    };
    let fr = comp.frame_rate;
    let work_area = matches!(str_p(p, "timeSpan"), Some("workArea" | "work area" | "work_area"));
    let times: Vec<Tick> = if work_area {
        let a = comp.work_area.0.max(layer.in_point);
        let b = comp.work_area.1.min(layer.out_point);
        let (fa, fb) = (fr.frame_at(fr.snap_nearest(a)), fr.frame_at(b - Tick(1)));
        (fa..=fb).map(|f| fr.tick_of(f)).collect()
    } else {
        vec![s.time()]
    };
    if times.is_empty() {
        return Err(bad(c, "the work area doesn't overlap the layer"));
    }
    // Trace every frame (in parallel).
    let frames: Vec<Vec<Traced>> = {
        let mut r = Renderer::new(&s.project, s.footage.as_ref(), RenderOpts::default());
        r.expr = s.expr.as_deref();
        let r = &r;
        let (project, expr) = (&s.project, s.expr.as_deref());
        use rayon::prelude::*;
        times
            .par_iter()
            .map(|t| match crate::tracking::source_frame(r, project, cid, &comp, &layer, *t, expr) {
                Some((img, off)) => trace_frame(&img, off, ch, invert, blur, &o),
                None => vec![],
            })
            .collect()
    };
    let n_masks = frames.iter().map(Vec::len).max().unwrap_or(0);
    if n_masks == 0 {
        return Err(bad(c, "nothing to trace: no pixels pass the threshold"));
    }
    let new_layer = b_p(p, "applyToNewLayer").unwrap_or(false);
    let (sw, sh) = effectcraft_render::source_size(&s.project, &layer);
    let layer_times: Vec<Tick> = times.iter().map(|t| layer.layer_time(*t)).collect();
    let target = s.edit("Auto-trace", None, |proj, st| {
        let target = if new_layer {
            // A solid the size of the layer, in the same place, carries the masks.
            let folder = proj.folder_named("Solids").unwrap_or_else(|| proj.add_item("Solids", Label::Yellow, None, ItemKind::Folder));
            let name = format!("Auto-traced {}", layer.name);
            let sid = proj.add_item(
                &name,
                Label::Red,
                Some(folder),
                ItemKind::Solid(Solid { color: [1.0, 1.0, 1.0], width: sw.max(1), height: sh.max(1), pixel_aspect: comp.pixel_aspect }),
            );
            let mut l = build::layer(proj, &comp, &name, LayerSource::Solid { item: sid }, (sw.max(1), sh.max(1)), None);
            l.start_time = layer.start_time;
            l.in_point = layer.in_point;
            l.out_point = layer.out_point;
            l.stretch = layer.stretch;
            l.parent = layer.parent;
            if let (Some(dst), Some(src)) = (l.props.sub_mut("transform"), layer.transform()) {
                let mut tr = src.clone();
                tr.reassign_uids(&mut proj.next_id);
                *dst = tr;
            }
            let id = l.id;
            let cm = proj.comp_mut(cid).ok_or(EngineError::NoComp)?;
            let at = cm.layers.iter().position(|x| x.id == lid).unwrap_or(0);
            cm.layers.insert(at, l);
            id
        } else {
            lid
        };
        let mut next = proj.next_id;
        let l = super::layer_mut(proj, cid, target)?;
        let masks = l.props.sub_mut("masks").ok_or_else(|| bad(c, "this layer can't have masks"))?;
        let base = masks.children.len();
        for k in 0..n_masks {
            // Depth decides the mode: outlines add, holes subtract.
            let Some(first) = frames.iter().find_map(|f| f.get(k)) else { continue };
            let mode = if first.depth % 2 == 0 { MaskMode::Add } else { MaskMode::Subtract };
            let mode = if k == 0 { MaskMode::Add } else { mode };
            let mut g = build::mask(
                &mut Ids(&mut next),
                &format!("Auto-trace {}", k + 1),
                first.path.clone(),
                mode,
                build::MASK_COLORS[(base + k) % build::MASK_COLORS.len()],
            );
            if frames.len() > 1 {
                if let Some(path) = g.get_mut("path") {
                    for (f, lt) in frames.iter().zip(&layer_times) {
                        if let Some(tr) = f.get(k) {
                            effectcraft_keyframe::set_key(&mut path.keys, Keyframe::new(*lt, KV::Path(tr.path.clone())));
                        }
                    }
                }
                if let Some(op) = g.get_mut("opacity")
                    && frames.iter().any(|f| f.get(k).is_none())
                {
                    for (f, lt) in frames.iter().zip(&layer_times) {
                        let mut key = Keyframe::new(*lt, KV::Scalar(if f.get(k).is_some() { 100.0 } else { 0.0 }));
                        key.in_interp = Interp::Hold;
                        key.out_interp = Interp::Hold;
                        effectcraft_keyframe::set_key(&mut op.keys, key);
                    }
                }
            }
            masks.children.push(g.into());
        }
        proj.next_id = next;
        st.selected_layers = vec![target];
        Ok(target)
    })?;
    Ok(json!({"layer": target.0, "masks": n_masks, "frames": times.len()}))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![cmd!(
        "layer.autoTrace",
        "Auto-trace...",
        ["Layer"],
        None,
        "{layer?, timeSpan?: currentFrame|workArea, channel?: alpha|red|green|blue|luminance, invert?, blur? (px, 1), tolerance? (px, 1), threshold? (%, 50), minimumArea? (px, 10), cornerRoundness? (%, 50), applyToNewLayer?}",
        has_layers,
        run
    )]
}

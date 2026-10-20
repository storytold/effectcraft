//! Layer masks: path coverage with expansion, feather, opacity, invert, combined by mode.

use effectcraft_effects::Buf;
use effectcraft_geom::{Mat3, vec2};
use effectcraft_keyframe::Value;
use effectcraft_path::{FillRule, Join};
use effectcraft_project::{FeatherFalloff, GroupKind, Layer, MaskMode, MaskMotionBlur};
use effectcraft_raster::{Mask, box_blur, gaussian_blur};
use rayon::prelude::*;

use crate::eval::EvalCtx;

/// What mask motion blur may do in a render (from the renderer's motion blur gate).
#[derive(Clone, Copy, Debug, Default)]
pub struct MaskBlur {
    /// Motion blur is on in this render: the render options, the comp's switch and the Motion
    /// Blur switches of the precomp layers above (when switches affect nested comps).
    pub allowed: bool,
    /// The layer's own Motion Blur switch, as those precomp layers limit it (Same As Layer).
    pub layer: bool,
    /// Sub-frame samples (the comp's Samples Per Frame; fewer in Draft).
    pub samples: usize,
}

/// Coverage of one mask in buffer pixels (before mode/opacity/invert). With mask motion blur
/// (Layer ▸ Mask ▸ Motion Blur: On, or Same As Layer with the layer's switch on) an animated
/// path is averaged over the comp's shutter.
fn coverage(ctx: &EvalCtx, layer: &Layer, g: &effectcraft_project::PropGroup, buf: &Buf, blur: MaskBlur) -> Option<Mask> {
    let mb = blur.allowed
        && match g.kind {
            GroupKind::Mask { motion_blur, .. } => match motion_blur {
                MaskMotionBlur::On => true,
                MaskMotionBlur::Off => false,
                MaskMotionBlur::SameAsLayer => blur.layer,
            },
            _ => false,
        };
    let animated = g.get("path").is_some_and(|p| p.keys.len() > 1 || p.has_expression());
    if mb && animated {
        let fd = ctx.comp.frame_duration().seconds();
        let (angle, phase) = (ctx.comp.shutter_angle / 360.0, ctx.comp.shutter_phase / 360.0);
        let mut acc: Option<Mask> = None;
        let n = blur.samples.max(2);
        let k = 1.0 / n as f32;
        for i in 0..n {
            let f = phase + angle * i as f64 / (n - 1) as f64;
            let sub = ctx.at(ctx.time + effectcraft_time::Tick::from_seconds_f64(f * fd));
            let Some(c) = coverage_at(&sub, layer, g, buf) else { continue };
            match &mut acc {
                None => {
                    let mut c = c;
                    c.data.iter_mut().for_each(|v| *v *= k);
                    acc = Some(c);
                }
                Some(a) => a.data.iter_mut().zip(&c.data).for_each(|(a, c)| *a += c * k),
            }
        }
        return acc;
    }
    coverage_at(ctx, layer, g, buf)
}

fn coverage_at(ctx: &EvalCtx, layer: &Layer, g: &effectcraft_project::PropGroup, buf: &Buf) -> Option<Mask> {
    let Value::Path(sp) = ctx.group_value(layer, g, "path")? else { return None };
    let path = effectcraft_path::to_kurbo(&sp);
    let (w, h) = (buf.img.width, buf.img.height);
    if !buf.scale.is_finite() || buf.scale <= 0.0 {
        return None;
    }
    let m = Mat3::translate(vec2(buf.offset[0], buf.offset[1])) * Mat3::scale(vec2(buf.scale, buf.scale));
    let exp = ctx.f(layer, g, "expansion", 0.0);
    let linear = matches!(g.kind, GroupKind::Mask { feather_falloff: FeatherFalloff::Linear, .. });
    let variable = !sp.feather.is_empty();
    let path = if !variable && exp.abs() > 0.01 { effectcraft_path::offset::offset_path(&path, exp, Join::Round, 4.0) } else { path };
    let raster = |transform: &Mat3, width, height| {
        if variable {
            effectcraft_path::feather::variable_feather_coverage(&sp, transform, width, height, exp, linear)
        } else {
            effectcraft_path::fill_coverage(std::slice::from_ref(&path), transform, width, height, FillRule::NonZero)
        }
    };
    let sigma = ctx.v2(layer, g, "feather", [0.0; 2]).map(|f| if !f.is_finite() || f <= 0.0 { 0.0 } else { (f / 2.0 * buf.scale).min(f64::MAX / 16.0) });
    if sigma == [0.0; 2] || w == 0 || h == 0 {
        return Some(raster(&m, w, h));
    }
    // Rasterize the real mask beyond the source before convolving. Transparent padding of
    // an already clipped coverage would still invent a feather edge at the source boundary.
    let padding = |sigma: [f64; 2]| {
        sigma.map(|s| {
            if s == 0.0 {
                0
            } else if linear {
                (s.round() as u32).max(1)
            } else {
                (3.0 * s + 3.0).ceil() as u32
            }
        })
    };
    let full_pad = padding(sigma);
    let full_size = full_pad[0].checked_mul(2).and_then(|p| w.checked_add(p)).zip(full_pad[1].checked_mul(2).and_then(|p| h.checked_add(p)));
    let fits = full_size.is_some_and(|(w, h)| w <= 2048 && h <= 2048);
    let tile = if fits { w.max(h) as usize } else { 256 };
    // Bound temporary work to at most 2048² pixels. Large frames use exact full-resolution
    // tiles; exceptionally broad feather uses a lower-resolution coverage within each tile.
    let work_scale = if fits { 1.0 } else { (128.0 / sigma[0].max(sigma[1])).min(1.0) };
    let work_sigma = sigma.map(|s| s * work_scale);
    let pad = padding(work_sigma);
    let mut out = Mask::new(w, h, 0.0);
    for y0 in (0..h).step_by(tile) {
        for x0 in (0..w).step_by(tile) {
            let tw = (w - x0).min(tile as u32);
            let th = (h - y0).min(tile as u32);
            let ww = ((f64::from(tw) * work_scale).ceil() as u32).max(1).checked_add(pad[0].checked_mul(2)?)?;
            let wh = ((f64::from(th) * work_scale).ceil() as u32).max(1).checked_add(pad[1].checked_mul(2)?)?;
            if ww > 2048 || wh > 2048 {
                return None;
            }
            let transform = Mat3::translate(vec2(f64::from(pad[0]) - f64::from(x0) * work_scale, f64::from(pad[1]) - f64::from(y0) * work_scale))
                * Mat3::scale(vec2(work_scale, work_scale))
                * m;
            let coverage = raster(&transform, ww, wh).to_image();
            let blurred = if linear {
                box_blur(&coverage, pad[0] as usize, pad[1] as usize, 1, false)
            } else {
                gaussian_blur(&coverage, work_sigma[0], work_sigma[1], false)
            };
            for (y, row) in out.data.chunks_mut(w as usize).skip(y0 as usize).take(th as usize).enumerate() {
                let Some(dst) = row.get_mut(x0 as usize..x0 as usize + tw as usize) else { continue };
                for (x, alpha) in dst.iter_mut().enumerate() {
                    *alpha = blurred.sample_bilinear(f64::from(pad[0]) + (x as f64 + 0.5) * work_scale, f64::from(pad[1]) + (y as f64 + 0.5) * work_scale)[3];
                }
            }
        }
    }
    Some(out)
}

/// The layer's enabled masks at the context time, flattened to polylines in layer space (for
/// effects that use masks as paths: Stroke, Scribble, Inner/Outer Key, Reshape…).
pub fn shapes(ctx: &EvalCtx, layer: &Layer) -> Vec<effectcraft_effects::MaskShape> {
    let Some(masks) = layer.masks() else { return Vec::new() };
    let mut out = Vec::new();
    for g in masks.groups().filter(|g| g.enabled) {
        let GroupKind::Mask { inverted, .. } = g.kind else { continue };
        let Some(Value::Path(sp)) = ctx.group_value(layer, g, "path") else { continue };
        let path = effectcraft_path::to_kurbo(&sp);
        let mut pts: Vec<[f64; 2]> = Vec::new();
        let mut closed = false;
        kurbo::flatten(path.iter(), 0.25, |el| match el {
            effectcraft_path::PathEl::MoveTo(p) | effectcraft_path::PathEl::LineTo(p) if pts.last() != Some(&[p.x, p.y]) => {
                pts.push([p.x, p.y]);
            }
            effectcraft_path::PathEl::ClosePath => closed = true,
            _ => {}
        });
        if closed && pts.len() > 1 && pts.first() == pts.last() {
            pts.pop();
        }
        out.push(effectcraft_effects::MaskShape { name: g.name.clone(), points: pts, closed: closed || sp.closed, inverted });
    }
    out
}

/// Apply the layer's masks to its buffer.
/// Returns whether any mask was applied.
pub fn apply(ctx: &EvalCtx, layer: &Layer, buf: &mut Buf, blur: MaskBlur) -> bool {
    let Some(masks) = layer.masks() else { return false };
    // An open mask (a path the Pen is still drawing) makes no transparency: the layer shows
    // whole until the path is closed (After Effects; effects still use it as a path).
    let closed = |g: &effectcraft_project::PropGroup| matches!(ctx.group_value(layer, g, "path"), Some(Value::Path(sp)) if sp.closed);
    let list: Vec<_> = masks.groups().filter(|g| g.enabled && matches!(g.kind, GroupKind::Mask { mode, .. } if mode != MaskMode::None) && closed(g)).collect();
    if list.is_empty() {
        return false;
    }
    // Coverage is feathered beyond the source, then cropped; source pixels stay source-sized.
    let n = (buf.img.width * buf.img.height) as usize;
    let first_sub = matches!(list[0].kind, GroupKind::Mask { mode: MaskMode::Subtract, .. });
    let mut acc = vec![if first_sub { 1.0f32 } else { 0.0 }; n];
    // The first mask has nothing to combine with. Intersect and Darken would multiply or
    // minimise against that empty coverage and hide the layer; they cover like Add instead.
    // Lighten and Difference already do. Filling the accumulator first would make Add show
    // the whole layer.
    let mut first = true;
    for g in list {
        let GroupKind::Mask { mode, inverted, .. } = g.kind else { continue };
        let Some(mut cov) = coverage(ctx, layer, g, buf, blur) else { continue };
        let mode = if first && matches!(mode, MaskMode::Intersect | MaskMode::Darken) { MaskMode::Add } else { mode };
        first = false;
        let op = (ctx.f(layer, g, "opacity", 100.0) / 100.0) as f32;
        cov.data.par_iter_mut().for_each(|c| {
            if inverted {
                *c = 1.0 - *c;
            }
            *c *= op;
        });
        acc.par_iter_mut().zip(cov.data.par_iter()).for_each(|(a, &c)| {
            *a = match mode {
                MaskMode::Add => *a + c - *a * c,
                MaskMode::Subtract => *a * (1.0 - c),
                MaskMode::Intersect => *a * c,
                MaskMode::Lighten => a.max(c),
                MaskMode::Darken => a.min(c),
                MaskMode::Difference => (*a - c).abs(),
                MaskMode::None => *a,
            };
        });
    }
    buf.img.mul_mask(&acc);
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use effectcraft_keyframe::ShapePath;
    use effectcraft_project::{Comp, ItemId, Project, build};
    use effectcraft_raster::Image;
    use effectcraft_time::{FrameRate, Tick};

    #[test]
    fn feather_tiles_match_shifted_canvas_and_bound_extreme_parameters() {
        let mut project = Project::default();
        let comp = Comp::new(2100, 20, FrameRate::FPS_30, Tick::from_seconds_f64(1.0));
        let layer = build::layer(&mut project, &comp, "Mask", effectcraft_project::LayerSource::Null, (2100, 20), None);
        let mut mask = build::mask(&mut build::Ids(&mut project.next_id), "Mask", ShapePath::rect([550.0, 10.0], 1140.0, 60.0), MaskMode::Add, [0; 3]);
        mask.get_mut("feather").unwrap().value = Value::Vec2([24.0, 12.0]);
        let ctx = EvalCtx::new(&project, ItemId(0), &comp, Tick::ZERO);
        let buf = Buf { img: Image::new(2100, 20), offset: [0.0; 2], scale: 1.0 };
        let reference = Buf { img: Image::new(2300, 220), offset: [100.0; 2], scale: 1.0 };
        let a = coverage_at(&ctx, &layer, &mask, &buf).unwrap();
        let b = coverage_at(&ctx, &layer, &mask, &reference).unwrap();
        for y in [0, 10, 19] {
            for x in [0, 255, 256, 257, 1023, 1024, 1100, 1119, 1120, 2047, 2048, 2099] {
                let ai = y * 2100 + x;
                let bi = (y + 100) * 2300 + x + 100;
                assert!((a.data[ai] - b.data[bi]).abs() < 1e-5, "tile seam at {x},{y}");
            }
        }
        for value in [f64::MAX, f64::NAN, f64::INFINITY] {
            mask.get_mut("feather").unwrap().value = Value::Vec2([value, value]);
            let buf = Buf { img: Image::new(16, 16), offset: [0.0; 2], scale: 1.0 };
            let a = coverage_at(&ctx, &layer, &mask, &buf).unwrap();
            assert_eq!(a.data.len(), 256);
            assert!(a.data.iter().all(|a| a.is_finite() && *a >= 0.0 && *a <= 1.0001));
        }
    }
}

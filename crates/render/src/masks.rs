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
    let m = Mat3::translate(vec2(buf.offset[0], buf.offset[1])) * Mat3::scale(vec2(buf.scale, buf.scale));
    let exp = ctx.f(layer, g, "expansion", 0.0);
    let linear = matches!(g.kind, GroupKind::Mask { feather_falloff: FeatherFalloff::Linear, .. });
    // Variable-width feather (Mask Feather tool points): expansion and the per-point feather
    // are applied together from the signed distance to the path.
    let variable = !sp.feather.is_empty();
    let mut cov = if variable {
        effectcraft_path::feather::variable_feather_coverage(&sp, &m, w, h, exp, linear)
    } else if exp.abs() > 0.01 {
        // Expansion offsets the outline itself (round joins). Offset Paths' geometry stays right
        // when the expansion exceeds a curvature radius, where a fill + stroke blend leaves a
        // ring around an uncovered middle (growing) or islands (shrinking).
        let grown = effectcraft_path::offset::offset_path(&path, exp, Join::Round, 4.0);
        effectcraft_path::fill_coverage(std::slice::from_ref(&grown), &m, w, h, FillRule::NonZero)
    } else {
        effectcraft_path::fill_coverage(std::slice::from_ref(&path), &m, w, h, FillRule::NonZero)
    };
    let feather = ctx.v2(layer, g, "feather", [0.0; 2]);
    if feather[0] > 0.0 || feather[1] > 0.0 {
        let img = cov.to_image();
        let b = if linear {
            // One box pass: a straight ramp `feather` pixels wide across the edge.
            let r = |f: f64| ((f * buf.scale / 2.0).round() as usize).max(if f > 0.0 { 1 } else { 0 });
            box_blur(&img, r(feather[0]), r(feather[1]), 1, false)
        } else {
            gaussian_blur(&img, feather[0] * buf.scale / 2.0, feather[1] * buf.scale / 2.0, false)
        };
        cov = Mask::from_alpha(&b);
    }
    Some(cov)
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

/// Layer-space bounds `[x0, y0, x1, y1]` outside which the layer's masks hide everything, or
/// `None` when they don't limit it (no closed masks, a Subtract or inverted mask first…). The
/// bounds are conservative: expansion and feather pad each mask's path bounds.
pub fn visible_bounds(ctx: &EvalCtx, layer: &Layer) -> Option<[f64; 4]> {
    let masks = layer.masks()?;
    let union = |a: [f64; 4], b: [f64; 4]| [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])];
    let inter = |a: [f64; 4], b: [f64; 4]| [a[0].max(b[0]), a[1].max(b[1]), a[2].min(b[2]), a[3].min(b[3])];
    // `None`: unbounded; `Some(None)`: nothing visible yet; `Some(Some(r))`: within `r`.
    let mut acc: Option<Option<[f64; 4]>> = None;
    let mut first = true;
    for g in masks.groups().filter(|g| g.enabled) {
        let GroupKind::Mask { mode, inverted, .. } = g.kind else { continue };
        if mode == MaskMode::None {
            continue;
        }
        let Some(Value::Path(sp)) = ctx.group_value(layer, g, "path") else { continue };
        if !sp.closed {
            continue;
        }
        // As in `apply`: the first mask starts from nothing (or everything for Subtract), and
        // a first Intersect or Darken covers like Add.
        let mode = if first && matches!(mode, MaskMode::Intersect | MaskMode::Darken) { MaskMode::Add } else { mode };
        if first {
            first = false;
            if mode == MaskMode::Subtract {
                continue;
            }
            acc = Some(None);
        }
        // Region this mask's coverage can reach (`None`: anywhere).
        let cov = if inverted || !sp.feather.is_empty() {
            None
        } else if ctx.f(layer, g, "opacity", 100.0) <= 0.0 {
            Some(None)
        } else {
            let exp = ctx.f(layer, g, "expansion", 0.0);
            let feather = ctx.v2(layer, g, "feather", [0.0; 2]);
            let pad = |f: f64| exp + if f > 0.0 { f * 1.5 } else { 0.0 };
            let (px, py) = (pad(feather[0]), pad(feather[1]));
            let path = effectcraft_path::to_kurbo(&sp);
            Some(
                effectcraft_path::bounds(std::slice::from_ref(&path))
                    .map(|r| [r.x0 - px, r.y0 - py, r.x1 + px, r.y1 + py])
                    .filter(|r| r[0] < r[2] && r[1] < r[3]),
            )
        };
        acc = match mode {
            MaskMode::Subtract | MaskMode::None => acc,
            MaskMode::Add | MaskMode::Lighten | MaskMode::Difference => match (acc, cov) {
                (Some(a), Some(c)) => Some(match (a, c) {
                    (Some(a), Some(c)) => Some(union(a, c)),
                    (a, c) => a.or(c),
                }),
                _ => None,
            },
            MaskMode::Intersect | MaskMode::Darken => match (acc, cov) {
                (Some(Some(a)), Some(Some(c))) => Some(Some(inter(a, c)).filter(|r| r[0] < r[2] && r[1] < r[3])),
                (Some(None), _) | (_, Some(None)) => Some(None),
                (a, None) => a,
                (None, c) => c,
            },
        };
    }
    acc.flatten()
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
    // Masks may extend past the source: grow the buffer to cover them when feather/expansion pads.
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

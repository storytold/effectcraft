//! Layer masks: path coverage with expansion, feather, opacity, invert, combined by mode.

use effectcraft_effects::Buf;
use effectcraft_geom::{Mat3, vec2};
use effectcraft_keyframe::Value;
use effectcraft_path::{FillRule, Join};
use effectcraft_project::{FeatherFalloff, GroupKind, Layer, MaskMode, MaskMotionBlur};
use effectcraft_raster::{Mask, box_blur};
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

/// The largest feather taken, in buffer pixels (larger and non-finite values are clamped).
const MAX_FEATHER_PX: f64 = 1.0e6;
/// Smooth falloff (After Effects' default), measured on After Effects 2026 renders (feather 100,
/// 300 and 1000; red value along rows, columns and the corner diagonal of a feathered rect mask):
/// the edge is blurred separably by three box passes, each `2r + 1` pixels wide with
/// `r = round(SMOOTH_BOX_RADIUS * feather)`. That is a quadratic B-spline reaching about
/// 1.11 x feather either side of the path (finite support; a Gaussian of any width misses the
/// measured profile by 3/255 or more, this kernel by less than 1/255).
const SMOOTH_BOX_RADIUS: f64 = 0.37;
/// Up to this box radius (buffer pixels) a feather is blurred at the buffer's resolution. A wider
/// one is blurred on a grid coarser per axis, about `COARSE_RADIUS` grid pixels per box: a wide
/// blur is smooth, so the coarse grid loses nothing visible, and the padding stays about
/// 3 x `COARSE_RADIUS` grid pixels. (Box passes cost the same at any radius; the coarse grid
/// bounds the padded area, i.e. memory.)
const FULL_RES_RADIUS: usize = 48;
const COARSE_RADIUS: usize = 32;
/// The largest padded grid (pixels). A grid that would be bigger (a huge buffer) is coarsened
/// (`grid`) until it fits, so the padding, and with it the feather of a mask edge outside the
/// buffer, is never dropped.
const MAX_GRID_PX: usize = 1 << 26;
/// The coarsest factor `grid` tries before giving up on the padding (unreachable for any buffer
/// that fits in memory: 1024 x 1024 buffer pixels per grid pixel).
const MAX_COARSEN: usize = 1024;
/// Padding for a variable-width feather, which is rasterised at the buffer's resolution.
const MAX_VARIABLE_PAD: usize = 256;

/// One axis of the grid a feathered mask is rasterised and blurred on: `step` buffer pixels per
/// grid pixel, `pad` grid pixels on each side of the buffer, and the box radii in grid pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Axis {
    step: usize,
    pad: usize,
    /// Radii of the box passes (Smooth: three; Linear: one, the others 0).
    radii: [usize; 3],
}

impl Axis {
    /// `feather` in buffer pixels. The pad covers the blur's whole support, so the coverage of
    /// the path outside the buffer reaches into it as on an unbounded plane.
    fn new(feather: f64, linear: bool, full_res_only: bool) -> Axis {
        let f = if feather.is_finite() { feather.clamp(0.0, MAX_FEATHER_PX) } else { 0.0 };
        let none = Axis { step: 1, pad: 0, radii: [0; 3] };
        if f <= 0.0 {
            return none;
        }
        let cap = |pad: usize| if full_res_only { pad.min(MAX_VARIABLE_PAD) } else { pad };
        if linear {
            // Linear falloff (not measured against After Effects): one box pass, a straight ramp
            // `feather` pixels wide across the edge, as before. Not quite unchanged: a ramp wider
            // than about `4 x FULL_RES_RADIUS` pixels is now blurred on the coarse grid and
            // resampled bilinearly (a smooth ramp, so the difference is a rounding of the ramp).
            let r = (f / 2.0).round().max(1.0) as usize;
            let step = if full_res_only || r <= 2 * FULL_RES_RADIUS { 1 } else { r.div_ceil(2 * COARSE_RADIUS) };
            let rg = ((r as f64 / step as f64).round() as usize).max(1);
            return Axis { step, pad: cap(rg), radii: [rg, 0, 0] };
        }
        let r = (SMOOTH_BOX_RADIUS * f).round() as usize;
        if r == 0 {
            return none;
        }
        if full_res_only || r <= FULL_RES_RADIUS {
            return Axis { step: 1, pad: cap(3 * r + 1), radii: [r; 3] };
        }
        // On a coarse grid the three windows (2 rg + 1 grid pixels each) should still add up to
        // 3 (2r + 1) buffer pixels: of a few steps take the one that comes closest, and spread
        // the radii over the passes.
        let want = 3 * (2 * r + 1);
        let s0 = r.div_ceil(COARSE_RADIUS);
        let fit = |s: usize| -> (usize, usize) {
            let sum = ((want as f64 / s as f64 - 3.0) / 2.0).round().max(0.0) as usize;
            let got = (2 * sum + 3).saturating_mul(s);
            (sum, got.abs_diff(want))
        };
        let (step, sum) = (s0..=s0 + (s0 / 2).min(8)).map(|s| (s, fit(s))).min_by_key(|&(s, (_, e))| (e, s)).map_or((s0, fit(s0).0), |(s, (sum, _))| (s, sum));
        let radii = [0, 1, 2].map(|i| sum / 3 + usize::from(i < sum % 3));
        Axis { step, pad: sum + 1, radii }
    }

    fn blurs(&self) -> bool {
        self.radii.iter().any(|&r| r > 0)
    }

    /// The same blur on a grid `k` times coarser: radii and padding scaled down (a blurring
    /// axis keeps at least one grid pixel of radius), the padding still covering the support.
    fn coarser(self, k: usize) -> Axis {
        if k <= 1 {
            return self;
        }
        let blurs = self.blurs();
        let mut radii = self.radii.map(|r| (r as f64 / k as f64).round() as usize);
        if blurs && radii.iter().all(|&r| r == 0) {
            radii[0] = 1;
        }
        let support = radii.iter().fold(0usize, |a, &r| a.saturating_add(r)).saturating_add(1);
        let pad = if self.pad == 0 { 0 } else { self.pad.div_ceil(k).max(support) };
        Axis { step: self.step.saturating_mul(k), pad, radii }
    }

    /// Grid pixels for `n` buffer pixels.
    fn len(&self, n: u32) -> Option<usize> {
        (n as usize).div_ceil(self.step).checked_add(self.pad.checked_mul(2)?)
    }
}

/// Resamples a grid coverage (`gw` wide) back to the `w x h` buffer: bilinear between the grid
/// pixel centres (an exact copy when both steps are 1).
fn crop(grid: &Mask, ax: Axis, ay: Axis, w: u32, h: u32) -> Mask {
    let (gw, gh) = (grid.width as usize, grid.height as usize);
    let mut out = Mask::new(w, h, 0.0);
    let at = |x: usize, y: usize| grid.data.get(y.saturating_mul(gw).saturating_add(x)).copied().unwrap_or(0.0);
    // Grid coordinate of a buffer pixel centre, as (index, fraction) on that axis.
    let map = |i: u32, a: Axis, n: usize| -> (usize, usize, f32) {
        let u = (i as f64 + 0.5) / a.step as f64 + a.pad as f64 - 0.5;
        let u = u.clamp(0.0, n.saturating_sub(1) as f64);
        let i0 = u.floor() as usize;
        (i0, (i0 + 1).min(n.saturating_sub(1)), (u - i0 as f64) as f32)
    };
    let xs: Vec<(usize, usize, f32)> = (0..w).map(|x| map(x, ax, gw)).collect();
    out.data.par_chunks_mut(w.max(1) as usize).enumerate().for_each(|(y, row)| {
        let (y0, y1, fy) = map(y as u32, ay, gh);
        for (o, &(x0, x1, fx)) in row.iter_mut().zip(&xs) {
            let top = at(x0, y0) + (at(x1, y0) - at(x0, y0)) * fx;
            let bottom = at(x0, y1) + (at(x1, y1) - at(x0, y1)) * fx;
            *o = top + (bottom - top) * fy;
        }
    });
    out
}

fn coverage_at(ctx: &EvalCtx, layer: &Layer, g: &effectcraft_project::PropGroup, buf: &Buf) -> Option<Mask> {
    let Value::Path(sp) = ctx.group_value(layer, g, "path")? else { return None };
    let m = Mat3::translate(vec2(buf.offset[0], buf.offset[1])) * Mat3::scale(vec2(buf.scale, buf.scale));
    let exp = ctx.f(layer, g, "expansion", 0.0);
    let linear = matches!(g.kind, GroupKind::Mask { feather_falloff: FeatherFalloff::Linear, .. });
    let feather = ctx.v2(layer, g, "feather", [0.0; 2]);
    let feather = [feather[0] * buf.scale, feather[1] * buf.scale];
    Some(feathered(&sp, &m, (buf.img.width, buf.img.height), exp, feather, linear, MAX_GRID_PX))
}

/// The padded grid for a `w x h` buffer: the axes as given, or coarsened together (by 2, 4, …)
/// until the grid has at most `max_px` pixels. `None` without a blur (or, unreachably, when even
/// the coarsest grid is too big).
fn grid(w: u32, h: u32, ax: Axis, ay: Axis, max_px: usize) -> Option<(u32, u32, Axis, Axis)> {
    if !(ax.blurs() || ay.blurs()) {
        return None;
    }
    let mut k = 1usize;
    while k <= MAX_COARSEN {
        let (cx, cy) = (ax.coarser(k), ay.coarser(k));
        if let (Some(gw), Some(gh)) = (cx.len(w), cy.len(h))
            && gw.checked_mul(gh).is_some_and(|n| n <= max_px)
            && let (Ok(gw), Ok(gh)) = (u32::try_from(gw), u32::try_from(gh))
        {
            return Some((gw, gh, cx, cy));
        }
        k = k.saturating_mul(2);
    }
    None
}

/// Coverage of the mask path `sp` (layer -> buffer `m`) on a `w x h` buffer, expanded by `exp`
/// and feathered by `feather` buffer pixels per axis, on a padded grid of at most `max_px` pixels.
fn feathered(sp: &effectcraft_keyframe::ShapePath, m: &Mat3, (w, h): (u32, u32), exp: f64, feather: [f64; 2], linear: bool, max_px: usize) -> Mask {
    let path = effectcraft_path::to_kurbo(sp);
    // Variable-width feather (Mask Feather tool points): expansion and the per-point feather
    // are applied together from the signed distance to the path.
    let variable = !sp.feather.is_empty();
    // The feather blurs the mask's own edge, also where the path lies outside the layer (After
    // Effects: a feathered mask just below a layer still fades into it). So a feathered mask is
    // rasterised on a grid padded by the blur's reach, blurred there and cropped to the buffer.
    // A variable feather scales its point radii by the mean scale: it stays at full resolution
    // (unless the grid has to be coarsened to fit `max_px`).
    let (ax, ay) = (Axis::new(feather[0], linear, variable), Axis::new(feather[1], linear, variable));
    let blurred = ax.blurs() || ay.blurs();
    let grid = grid(w, h, ax, ay, max_px);
    // Without the grid (no blur, or, unreachably, a grid too big even at the coarsest step):
    // blurred within the buffer at its resolution.
    let unpadded = |f: f64| Axis { pad: 0, ..Axis::new(f, linear, true) };
    let (gw, gh, gx, gy) = grid.unwrap_or_else(|| (w, h, unpadded(feather[0]), unpadded(feather[1])));
    // Buffer pixels -> grid pixels.
    let to_grid = Mat3::scale(vec2(1.0 / gx.step as f64, 1.0 / gy.step as f64)) * Mat3::translate(vec2((gx.pad * gx.step) as f64, (gy.pad * gy.step) as f64));
    let mg = to_grid * *m;
    let mut cov = if variable {
        effectcraft_path::feather::variable_feather_coverage(sp, &mg, gw, gh, exp, linear)
    } else if exp.abs() > 0.01 {
        // Expansion offsets the outline itself (round joins). Offset Paths' geometry stays right
        // when the expansion exceeds a curvature radius, where a fill + stroke blend leaves a
        // ring around an uncovered middle (growing) or islands (shrinking).
        let grown = effectcraft_path::offset::offset_path(&path, exp, Join::Round, 4.0);
        effectcraft_path::fill_coverage(std::slice::from_ref(&grown), &mg, gw, gh, FillRule::NonZero)
    } else {
        effectcraft_path::fill_coverage(std::slice::from_ref(&path), &mg, gw, gh, FillRule::NonZero)
    };
    if blurred {
        // Separable box passes (the corner of a feathered rect is the product of its edges).
        let mut img = cov.to_image();
        for (rx, ry) in gx.radii.into_iter().zip(gy.radii) {
            if rx > 0 || ry > 0 {
                img = box_blur(&img, rx, ry, 1, false);
            }
        }
        cov = Mask::from_alpha(&img);
    }
    if grid.is_some() {
        cov = crop(&cov, gx, gy, w, h);
    }
    cov
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
    // Masks may extend past the source: the buffer keeps its size (pixels outside the layer stay
    // empty), but a feathered mask is blurred on a padded grid, so the feather of an edge outside
    // the buffer still reaches into it (`coverage_at`).
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

    /// CodeRabbit on #1874: an 8192 x 8192 layer with a 40 px Smooth feather needs a padded
    /// grid larger than `MAX_GRID_PX`; it used to fall back to the unpadded buffer, so an Add mask
    /// just outside the layer covered nothing and the whole layer vanished. The grid is now
    /// coarsened instead and keeps a pad that covers the blur.
    #[test]
    fn a_grid_too_big_is_coarsened_and_keeps_its_padding() {
        let a = Axis::new(40.0, false, false);
        assert!(a.len(8192).zip(a.len(8192)).is_some_and(|(x, y)| x * y > MAX_GRID_PX), "the full-resolution grid is over the limit");
        let (gw, gh, gx, gy) = grid(8192, 8192, a, a, MAX_GRID_PX).expect("a padded grid");
        assert!((gw as usize) * (gh as usize) <= MAX_GRID_PX);
        for g in [gx, gy] {
            assert!(g.step > 1 && g.pad > 0 && g.blurs(), "{g:?}");
            // The pad covers the blur's support (the three boxes' radii) in buffer pixels.
            assert!(g.pad * g.step >= g.radii.iter().sum::<usize>() * g.step, "{g:?}");
        }
        // A grid that fits is not touched.
        assert_eq!(grid(200, 100, a, a, MAX_GRID_PX).map(|g| g.2), Some(a));
    }

    /// The same failure at a testable size: a 200 x 100 buffer, a 40 px feather and a grid limit
    /// just below the full-resolution padded grid. The Add mask lies just right of the buffer
    /// (x 210..260); its feather still fades into the layer, close to the full-resolution result.
    #[test]
    fn a_coarsened_grid_still_feathers_a_mask_outside_the_buffer() {
        let sp = ShapePath::rect([235.0, 50.0], 50.0, 200.0);
        let m = Mat3::IDENTITY;
        let full = feathered(&sp, &m, (200, 100), 0.0, [40.0, 40.0], false, MAX_GRID_PX);
        let a = Axis::new(40.0, false, false);
        let limit = a.len(200).zip(a.len(100)).map_or(0, |(x, y)| x * y) - 1;
        let coarse = feathered(&sp, &m, (200, 100), 0.0, [40.0, 40.0], false, limit);
        let at = |c: &Mask, x: usize, y: usize| c.data.get(y * 200 + x).copied().unwrap_or(-1.0);
        assert!(at(&full, 199, 50) > 0.25, "full resolution: {}", at(&full, 199, 50));
        assert!(at(&coarse, 199, 50) > 0.2, "the soft fade survives the coarse grid: {}", at(&coarse, 199, 50));
        for x in [150, 170, 180, 190, 199] {
            assert!((at(&coarse, x, 50) - at(&full, x, 50)).abs() < 0.05, "x {x}: coarse {} full {}", at(&coarse, x, 50), at(&full, x, 50));
        }
        assert!(at(&coarse, 100, 50) < 0.01, "far from the mask stays clear");
    }
}

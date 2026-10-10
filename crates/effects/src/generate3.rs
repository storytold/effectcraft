//! Generate effects, batch 3: Fractal (Mandelbrot/Julia), mask-driven Scribble and Stroke,
//! Vegas (animated segments along image contours or masks), Write-on, Paint Bucket (flood
//! fill), Eyedropper Fill, CC Glue Gun, CC Threads, and the audio visualisers Audio Spectrum
//! and Audio Waveform (fed by the host's audio).
//!
//! Also home of the shared anti-aliased segment rasteriser ([`raster_segs`]) used by the text
//! effects.

use std::f64::consts::PI;

use effectcraft_color::{hsl_to_rgb, luminance, rgb_to_hsl};
use effectcraft_keyframe::Value;
use effectcraft_project::ParamUi;
use effectcraft_raster::{Image, Px};
use rayon::prelude::*;

use crate::util::{Plane, gauss_plane, hash1, layer_or_self, morph_frac, poly_length, premul, smoothstep, unpremul};
use crate::{Buf, EffectCtx, EffectSpec, Params, col, num, p, popup, slider};

fn spec(id: &'static str, name: &'static str, params: Vec<crate::ParamSpec>, render: crate::RenderFn) -> EffectSpec {
    EffectSpec { id, name, category: "Generate", params, render, gpu: false, float: true }
}

fn pt(x: f64, y: f64) -> Value {
    Value::Vec2([x, y])
}

fn pct() -> ParamUi {
    slider(0.0, 100.0, 0.0, 100.0, 1)
}

// ---------------------------------------------------------------- shared rasterisation

/// A round-capped line segment in buffer pixels with radius `r` and intensity `v`.
#[derive(Clone, Copy, Debug)]
pub struct Seg {
    pub a: [f64; 2],
    pub b: [f64; 2],
    pub r: f64,
    pub v: f32,
}

/// Rasterise segments into a coverage plane (max-combined). `hardness` (0..1) is the fraction
/// of the radius that is fully opaque; the rest falls off smoothly.
pub(crate) fn raster_segs(w: usize, h: usize, segs: &[Seg], hardness: f64) -> Plane {
    let mut out = Plane::new(w, h);
    if w == 0 || h == 0 || segs.is_empty() {
        return out;
    }
    let mut buckets: Vec<Vec<u32>> = vec![Vec::new(); h];
    for (i, s) in segs.iter().enumerate() {
        let pad = s.r + 1.0;
        let y0 = (s.a[1].min(s.b[1]) - pad).floor().max(0.0) as usize;
        let y1 = (s.a[1].max(s.b[1]) + pad).ceil().min(h as f64);
        if y1 <= 0.0 || y0 >= h {
            continue;
        }
        for row in buckets.iter_mut().take(y1 as usize).skip(y0) {
            row.push(i as u32);
        }
    }
    let hard = hardness.clamp(0.0, 1.0);
    out.data.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let py = y as f64 + 0.5;
        for &i in &buckets[y] {
            let s = segs[i as usize];
            let pad = s.r + 1.0;
            let x0 = (s.a[0].min(s.b[0]) - pad).floor().max(0.0) as usize;
            let x1 = ((s.a[0].max(s.b[0]) + pad).ceil().max(0.0) as usize).min(w);
            let (dx, dy) = (s.b[0] - s.a[0], s.b[1] - s.a[1]);
            let l2 = dx * dx + dy * dy;
            let inner = s.r * hard - 0.5;
            let outer = s.r + 0.5;
            for (x, o) in row.iter_mut().enumerate().take(x1).skip(x0) {
                let px = x as f64 + 0.5;
                let t = if l2 > 1e-12 { (((px - s.a[0]) * dx + (py - s.a[1]) * dy) / l2).clamp(0.0, 1.0) } else { 0.0 };
                let d = ((px - s.a[0] - dx * t).powi(2) + (py - s.a[1] - dy * t).powi(2)).sqrt();
                if d >= outer {
                    continue;
                }
                let c = if d <= inner { 1.0 } else { 1.0 - smoothstep(inner as f32, outer as f32, d as f32) } * s.v;
                if c > *o {
                    *o = c;
                }
            }
        }
    });
    out
}

/// Segments along a polyline (buffer px).
pub fn poly_segs(pts: &[[f64; 2]], closed: bool, r: f64, v: f32, out: &mut Vec<Seg>) {
    let n = pts.len();
    if n == 1 {
        out.push(Seg { a: pts[0], b: pts[0], r, v });
    }
    if n < 2 {
        return;
    }
    let segs = if closed { n } else { n - 1 };
    for i in 0..segs {
        out.push(Seg { a: pts[i], b: pts[(i + 1) % n], r, v });
    }
}

/// Portion `[s0, s1]` (arc length) of a polyline.
pub(crate) fn trim_poly(pts: &[[f64; 2]], closed: bool, s0: f64, s1: f64) -> Vec<[f64; 2]> {
    let n = pts.len();
    let mut out = Vec::new();
    if n < 2 || s1 <= s0 {
        return out;
    }
    let segs = if closed { n } else { n - 1 };
    let mut acc = 0.0;
    for i in 0..segs {
        let (a, b) = (pts[i], pts[(i + 1) % n]);
        let l = ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt();
        let (e0, e1) = (acc, acc + l);
        acc = e1;
        if e1 < s0 || e0 > s1 || l <= 0.0 {
            continue;
        }
        let at = |s: f64| {
            let t = ((s - e0) / l).clamp(0.0, 1.0);
            [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]
        };
        let p0 = at(s0.max(e0));
        if out.last() != Some(&p0) {
            out.push(p0);
        }
        out.push(at(s1.min(e1)));
    }
    out
}

/// How painted coverage meets the layer: 0 On Original Image, 1 On Transparent, 2 Reveal
/// Original Image.
pub(crate) fn paint(img: &mut Image, cov: &Plane, color: [f32; 4], opacity: f32, style: u32) {
    img.data.par_iter_mut().zip(cov.data.par_iter()).for_each(|(px, &c)| {
        let a = (c * opacity * color[3]).clamp(0.0, 1.0);
        let s = [color[0] * a, color[1] * a, color[2] * a, a];
        *px = match style {
            1 => s,
            2 => [px[0] * a, px[1] * a, px[2] * a, px[3] * a],
            _ => over(*px, s),
        };
    });
}

/// Coverage of a plan-drawn generator: round-capped segments rasterised by [`raster_segs`]
/// with `hardness`, or the union of convex polygons ([`raster_convex`]).
#[derive(Clone, Debug)]
pub enum Coverage {
    Segs { segs: Vec<Seg>, hardness: f64 },
    Convex(Vec<Vec<[f64; 2]>>),
}

impl Coverage {
    fn raster(&self, w: usize, h: usize) -> Plane {
        match self {
            Coverage::Segs { segs, hardness } => raster_segs(w, h, segs, *hardness),
            Coverage::Convex(polys) => raster_convex(w, h, polys),
        }
    }
}

/// Stroke, Scribble, Vegas and Write-on, shared with the GPU compositor (effectcraft-gpu
/// `fx_gen2`): the coverage and how it is painted. `style` 0–2 as [`paint`] (On Original
/// Image, On Transparent, Reveal Original Image), 3 = under the layer (Vegas' Composite Under).
#[derive(Clone, Debug)]
pub struct PaintPlan {
    pub cov: Coverage,
    pub color: [f32; 4],
    pub opacity: f32,
    pub style: u32,
}

/// Draw a [`PaintPlan`].
fn draw_paint(b: &mut Buf, plan: &PaintPlan) {
    let cov = plan.cov.raster(b.img.width as usize, b.img.height as usize);
    if plan.style == 3 {
        let color = plan.color;
        b.img.data.par_iter_mut().zip(cov.data.par_iter()).for_each(|(px, &c)| {
            let a = c * color[3];
            *px = over([color[0] * a, color[1] * a, color[2] * a, a], *px);
        });
    } else {
        paint(&mut b.img, &cov, plan.color, plan.opacity, plan.style);
    }
}

/// Generators drawn from a plan that need the layer's own pixels (`b.img`) to build it.
pub fn plan_reads_pixels(id: &str, ctx: &EffectCtx) -> bool {
    match id {
        // Image Contours read the layer itself when no (resolvable) Input Layer is chosen.
        "ec.generate.vegas" => ctx.params.e("stroke") == 0,
        "ec.generate.paintbucket" | "ec.generate.eyedropperfill" => true,
        "ec.generate.advancedlightning" => ctx.params.f("alphaObstacle").clamp(-10.0, 10.0) != 0.0,
        _ => false,
    }
}

/// [`PaintPlan`] of Stroke, Scribble, Vegas or Write-on on `b`'s geometry (`b.img` is read
/// only when [`plan_reads_pixels`]). `None`: Scribble without a usable mask (the layer is
/// cleared when it composites On Transparent, else kept).
pub fn paint_plan(id: &str, ctx: &EffectCtx, b: &Buf) -> Option<PaintPlan> {
    match id {
        "ec.generate.stroke" => Some(stroke_plan(ctx, b)),
        "ec.generate.scribble" => scribble_plan(ctx, b),
        "ec.generate.vegas" => Some(vegas_plan(ctx, b)),
        "ec.generate.writeon" => Some(write_on_plan(ctx, b)),
        _ => None,
    }
}

/// Scribble's Composite as a [`paint`] style.
pub fn scribble_style(ctx: &EffectCtx) -> u32 {
    match ctx.params.e("composite") {
        0 => 0,
        2 => 2,
        _ => 1,
    }
}

#[inline]
fn over(dst: Px, src: Px) -> Px {
    let k = 1.0 - src[3];
    [src[0] + dst[0] * k, src[1] + dst[1] * k, src[2] + dst[2] * k, src[3] + dst[3] * k]
}

/// Mask `idx` (1-based; 0 = none) mapped to buffer pixels.
pub(crate) fn mask_px(ctx: &EffectCtx, b: &Buf, idx: usize) -> Option<(Vec<[f64; 2]>, bool, bool)> {
    let m = ctx.env.masks.get(idx.checked_sub(1)?)?;
    if m.points.is_empty() {
        return None;
    }
    Some((m.points.iter().map(|&q| [q[0] * b.scale + b.offset[0], q[1] * b.scale + b.offset[1]]).collect(), m.closed, m.inverted))
}

/// All masks mapped to buffer pixels.
fn all_masks_px(ctx: &EffectCtx, b: &Buf) -> Vec<(Vec<[f64; 2]>, bool, bool)> {
    (1..=ctx.env.masks.len()).filter_map(|i| mask_px(ctx, b, i)).collect()
}

/// Chaikin corner cutting (open polyline, endpoints kept).
fn chaikin(pts: &[[f64; 2]], iters: usize) -> Vec<[f64; 2]> {
    let mut cur = pts.to_vec();
    for _ in 0..iters {
        if cur.len() < 3 {
            return cur;
        }
        let mut nx = Vec::with_capacity(cur.len() * 2);
        nx.push(cur[0]);
        for w in cur.windows(2) {
            let (a, b) = (w[0], w[1]);
            nx.push([a[0] * 0.75 + b[0] * 0.25, a[1] * 0.75 + b[1] * 0.25]);
            nx.push([a[0] * 0.25 + b[0] * 0.75, a[1] * 0.25 + b[1] * 0.75]);
        }
        nx.push(*cur.last().unwrap_or(&[0.0; 2]));
        cur = nx;
    }
    cur
}

// ---------------------------------------------------------------- Fractal

#[derive(Clone, Copy)]
struct FracSet {
    /// Mandelbrot iterations start at the Julia centre (the "Over Julia" set choices).
    over_julia: bool,
    julia: bool,
    inverse: bool,
}

/// Smooth escape count, or `None` inside the set.
fn escape(z0: (f64, f64), c: (f64, f64), power: u32, limit: f64, max_iter: u32) -> Option<f64> {
    let (mut x, mut y) = z0;
    let lim2 = limit * limit;
    for i in 0..max_iter {
        let (nx, ny) = match power {
            3 => (x * x * x - 3.0 * x * y * y, 3.0 * x * x * y - y * y * y),
            4 => {
                let (a, b) = (x * x - y * y, 2.0 * x * y);
                (a * a - b * b, 2.0 * a * b)
            }
            _ => (x * x - y * y, 2.0 * x * y),
        };
        x = nx + c.0;
        y = ny + c.1;
        let m2 = x * x + y * y;
        if m2 > lim2 {
            let lz = m2.ln() * 0.5;
            let nu = (lz / limit.max(1.0001).ln()).max(1e-9).ln() / (power as f64).ln();
            return Some((i as f64 + 1.0 - nu).max(0.0));
        }
    }
    None
}

/// Fractal Set Choice options.
pub(crate) const FRACTAL_SETS: [&str; 6] =
    ["Mandelbrot", "Mandelbrot Inverse", "Mandelbrot Over Julia", "Mandelbrot Inverse Over Julia", "Julia", "Julia Inverse"];
/// Fractal Palette options.
pub(crate) const FRACTAL_PALETTES: [&str; 4] = ["Lightness Gradient", "Hue Wheel", "Black And White", "Solid Color"];
/// Fractal Oversample Method options.
pub(crate) const FRACTAL_OVERSAMPLE: [&str; 2] = ["Edge Detect-Fast-May Miss Pixels", "Brute Force-Slow-Every Pixel"];

fn fractal(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let pr = ctx.params;
    let set_choice = pr.e("setChoice");
    let power = 2 + pr.e("equation").min(2);
    let mc = (pr.f("mandelbrot/mandelbrotX"), pr.f("mandelbrot/mandelbrotY"));
    let mmag = pr.f("mandelbrot/mandelbrotMagnification");
    let mlim = pr.f("mandelbrot/mandelbrotEscapeLimit").max(1.01);
    let jc = (pr.f("julia/juliaX"), pr.f("julia/juliaY"));
    let jmag = pr.f("julia/juliaMagnification");
    let jlim = pr.f("julia/juliaEscapeLimit").max(1.01);
    let post = (pr.f("postInversionOffset/postInversionX"), pr.f("postInversionOffset/postInversionY"));
    let overlay = pr.b("fractalColor/overlay");
    let transparency = pr.b("fractalColor/transparency");
    let transparent_inside = transparency || overlay;
    let palette = pr.e("fractalColor/palette");
    let hue = (pr.f("fractalColor/hue") / 360.0) as f32;
    let steps = pr.f("fractalColor/cycleSteps").max(1.0);
    let cyc_off = pr.f("fractalColor/cycleOffset") / 360.0;
    let edge = pr.b("fractalColor/edgeHighlight");
    let every = pr.e("highQualitySettings/samplingMethod") == 1;
    let factor = pr.f("highQualitySettings/samplingFactor").round().clamp(1.0, 9.0) as usize;
    let (w, h) = (b.img.width as usize, b.img.height as usize);
    let lw = ctx.layer_size[0].max(1.0);
    let lh = ctx.layer_size[1].max(1.0);
    let inv = 1.0 / b.scale.max(1e-9);
    let (off_x, off_y) = (b.offset[0], b.offset[1]);
    // FRACTAL_SETS order.
    let set = |julia, inverse, over_julia| FracSet { over_julia, julia, inverse };
    let sets: (FracSet, Option<FracSet>) = match set_choice {
        1 => (set(false, true, false), None),
        2 => (set(false, false, true), None),
        3 => (set(false, true, true), None),
        4 => (set(true, false, false), None),
        5 => (set(true, true, false), None),
        _ => (set(false, false, false), None),
    };
    let iter_for = |mag: f64| (96.0 + 48.0 * mag.max(0.0)).clamp(64.0, 3000.0) as u32;
    let eval_set = |s: FracSet, lx: f64, ly: f64| -> Option<f64> {
        let (center, mag, lim) = if s.julia { (jc, jmag, jlim) } else { (mc, mmag, mlim) };
        let span = 3.5 / 2f64.powf(mag);
        let mut u = (lx - lw * 0.5) / lw * span + center.0;
        let mut v = (ly - lh * 0.5) / lw * span + center.1;
        if s.inverse {
            let d = (u * u + v * v).max(1e-12);
            u = u / d + post.0;
            v = -v / d + post.1;
        }
        if s.julia {
            escape((u, v), mc, power, lim, iter_for(mag))
        } else {
            let z0 = if s.over_julia { jc } else { (0.0, 0.0) };
            escape(z0, (u, v), power, lim, iter_for(mag))
        }
    };
    let eval = |lx: f64, ly: f64| -> Option<f64> {
        let a = eval_set(sets.0, lx, ly);
        match (a, sets.1) {
            (None, Some(s2)) => eval_set(s2, lx, ly),
            _ => a,
        }
    };
    let shade = |n: Option<f64>| -> Px {
        // Solid Color: only the inside of the set is drawn (Transparency flips the sides).
        if palette == 3 {
            let (r, g, bl) = hsl_to_rgb(hue, 1.0, 0.5);
            return if n.is_none() != transparency { [r, g, bl, 1.0] } else { [0.0; 4] };
        }
        let Some(n) = n else {
            return if transparent_inside { [0.0; 4] } else { [0.0, 0.0, 0.0, 1.0] };
        };
        let u = n / steps + cyc_off;
        let band = (u.rem_euclid(1.0)) as f32;
        let (r, g, bl) = match palette {
            // Hue Wheel: the full hue circle at full saturation and brightness.
            1 => hsl_to_rgb((hue + band).rem_euclid(1.0), 1.0, 0.5),
            // Black And White: alternating bands.
            2 => {
                let v = if (u.floor() as i64).rem_euclid(2) == 0 { 0.0 } else { 1.0 };
                (v, v, v)
            }
            // Lightness Gradient: black → hue → white, the hue moving 45° each cycle.
            _ => hsl_to_rgb((hue + u.floor() as f32 * 0.125).rem_euclid(1.0), 0.85, band.clamp(0.0, 1.0)),
        };
        let mut c = [r, g, bl];
        if edge && n.fract() < 0.12 {
            c = [c[0] * 0.4 + 0.6, c[1] * 0.4 + 0.6, c[2] * 0.4 + 0.6];
        }
        [c[0], c[1], c[2], 1.0]
    };
    // First pass: one sample per pixel.
    let mut first: Vec<Option<f64>> = vec![None; w * h];
    first.par_chunks_mut(w.max(1)).enumerate().for_each(|(y, row)| {
        for (x, v) in row.iter_mut().enumerate() {
            *v = eval((x as f64 + 0.5 - off_x) * inv, (y as f64 + 0.5 - off_y) * inv);
        }
    });
    let src = if overlay { Some(b.img.clone()) } else { None };
    b.img.rows_mut().for_each(|(y, row)| {
        for (x, px) in row.iter_mut().enumerate() {
            let here = first[y * w + x];
            let needs_ss = factor > 1
                && (every || {
                    let mut diff = false;
                    for (dx, dy) in [(-1i64, 0i64), (1, 0), (0, -1), (0, 1)] {
                        let (nx, ny) = (x as i64 + dx, y as i64 + dy);
                        if nx < 0 || ny < 0 || nx >= w as i64 || ny >= h as i64 {
                            continue;
                        }
                        let o = first[ny as usize * w + nx as usize];
                        diff |= match (here, o) {
                            (Some(a), Some(c)) => (a - c).abs() > steps * 0.25,
                            (None, None) => false,
                            _ => true,
                        };
                    }
                    diff
                });
            let c = if needs_ss {
                let mut acc = [0.0f32; 4];
                for sy in 0..factor {
                    for sx in 0..factor {
                        let fx = x as f64 + (sx as f64 + 0.5) / factor as f64;
                        let fy = y as f64 + (sy as f64 + 0.5) / factor as f64;
                        let s = shade(eval((fx - off_x) * inv, (fy - off_y) * inv));
                        let a = s[3];
                        for k in 0..3 {
                            acc[k] += s[k] * a;
                        }
                        acc[3] += a;
                    }
                }
                let n = (factor * factor) as f32;
                [acc[0] / n, acc[1] / n, acc[2] / n, acc[3] / n]
            } else {
                let s = shade(here);
                [s[0] * s[3], s[1] * s[3], s[2] * s[3], s[3]]
            };
            *px = match &src {
                Some(o) => over(o.data[y * w + x], c),
                None => c,
            };
        }
    });
    b
}

// ---------------------------------------------------------------- Scribble

/// Scan runs (start, end along direction `d`) of the fill region on the line `n·p = s`.
#[allow(clippy::too_many_arguments)]
fn scan_runs(polys: &[(Vec<[f64; 2]>, bool, bool)], fill: u32, ew: f64, d: [f64; 2], n: [f64; 2], s: f64, out: &mut Vec<(f64, f64)>) {
    out.clear();
    for (pts, _closed, inverted) in polys {
        let m = pts.len();
        if m < 2 {
            continue;
        }
        // Crossings: (t along d, |sin| of crossing angle).
        let mut xs: Vec<(f64, f64)> = Vec::new();
        for i in 0..m {
            let (a, b) = (pts[i], pts[(i + 1) % m]);
            let (na, nb) = (a[0] * n[0] + a[1] * n[1] - s, b[0] * n[0] + b[1] * n[1] - s);
            if (na > 0.0) == (nb > 0.0) {
                continue;
            }
            let t = na / (na - nb);
            let q = [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t];
            let (ex, ey) = (b[0] - a[0], b[1] - a[1]);
            let el = (ex * ex + ey * ey).sqrt().max(1e-9);
            let sin = ((ex * n[0] + ey * n[1]) / el).abs().max(0.2);
            xs.push((q[0] * d[0] + q[1] * d[1], sin));
        }
        xs.sort_by(|a, b| a.0.total_cmp(&b.0));
        // Signed area decides which side is "left".
        let area: f64 = (0..m).map(|i| pts[i][0] * pts[(i + 1) % m][1] - pts[(i + 1) % m][0] * pts[i][1]).sum();
        let left_is_inside = area < 0.0;
        let mode = match fill {
            4 => {
                if left_is_inside {
                    2
                } else {
                    3
                }
            }
            5 => {
                if left_is_inside {
                    3
                } else {
                    2
                }
            }
            f => f,
        };
        let mode = if *inverted && mode == 0 { 6 } else { mode };
        for (k, &(t, sin)) in xs.iter().enumerate() {
            let entering = k % 2 == 0;
            let l = ew / sin;
            match mode {
                0 if entering && k + 1 < xs.len() => {
                    out.push((t, xs[k + 1].0));
                }
                1 => out.push((t - l * 0.5, t + l * 0.5)),
                2 => out.push(if entering { (t, t + l) } else { (t - l, t) }),
                3 => out.push(if entering { (t - l, t) } else { (t, t + l) }),
                _ => {}
            }
        }
        if mode == 6 {
            // Inverted Inside: outside of the polygon within the bounds.
            let lo = pts.iter().map(|q| q[0] * d[0] + q[1] * d[1]).fold(f64::INFINITY, f64::min) - ew;
            let hi = pts.iter().map(|q| q[0] * d[0] + q[1] * d[1]).fold(f64::NEG_INFINITY, f64::max) + ew;
            let mut cur = lo;
            for pair in xs.chunks(2) {
                out.push((cur, pair[0].0));
                cur = pair.get(1).map(|v| v.0).unwrap_or(hi);
            }
            out.push((cur, hi));
        }
    }
    out.sort_by(|a, b| a.0.total_cmp(&b.0));
    // Merge overlaps.
    let mut merged: Vec<(f64, f64)> = Vec::with_capacity(out.len());
    for &(a, b) in out.iter() {
        if let Some(last) = merged.last_mut()
            && a <= last.1
        {
            last.1 = last.1.max(b);
            continue;
        }
        merged.push((a, b));
    }
    *out = merged;
}

/// End Cap and Join options (Scribble's Edge Options).
const END_CAPS: [&str; 3] = ["Round", "Butt", "Projecting"];
const JOINS: [&str; 3] = ["Round", "Miter", "Bevel"];

/// Convex polygons whose union is the stroke of the polylines `lines` with half width `r`:
/// one quad per segment, plus the joins (Round discs, Miter wedges up to `miter_limit` × the
/// half width, Bevel triangles) and the caps (Round discs, Butt none, Projecting square ends).
pub(crate) fn stroke_outline(lines: &[Vec<[f64; 2]>], r: f64, cap: u32, join: u32, miter_limit: f64) -> Vec<Vec<[f64; 2]>> {
    let disc = |c: [f64; 2]| -> Vec<[f64; 2]> {
        (0..16).map(|k| (k as f64 / 16.0 * std::f64::consts::TAU).sin_cos()).map(|(s, co)| [c[0] + co * r, c[1] + s * r]).collect()
    };
    let mut out = Vec::new();
    for l in lines {
        let pts: Vec<[f64; 2]> = l.iter().copied().fold(Vec::new(), |mut v, p| {
            if v.last().is_none_or(|q: &[f64; 2]| (q[0] - p[0]).hypot(q[1] - p[1]) > 1e-9) {
                v.push(p);
            }
            v
        });
        if pts.len() < 2 {
            if let (Some(p), 0) = (pts.first(), cap) {
                out.push(disc(*p));
            }
            continue;
        }
        let dir = |a: [f64; 2], b: [f64; 2]| {
            let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
            let l = dx.hypot(dy).max(1e-12);
            [dx / l, dy / l]
        };
        let n = pts.len();
        for k in 0..n - 1 {
            let (mut a, mut b) = (pts[k], pts[k + 1]);
            let d = dir(a, b);
            // Projecting caps extend the end segments by the half width.
            if cap == 2 && k == 0 {
                a = [a[0] - d[0] * r, a[1] - d[1] * r];
            }
            if cap == 2 && k == n - 2 {
                b = [b[0] + d[0] * r, b[1] + d[1] * r];
            }
            let nn = [-d[1] * r, d[0] * r];
            out.push(vec![[a[0] + nn[0], a[1] + nn[1]], [b[0] + nn[0], b[1] + nn[1]], [b[0] - nn[0], b[1] - nn[1]], [a[0] - nn[0], a[1] - nn[1]]]);
        }
        // Joins.
        for k in 1..n - 1 {
            let (p, d0, d1) = (pts[k], dir(pts[k - 1], pts[k]), dir(pts[k], pts[k + 1]));
            let cross = d0[0] * d1[1] - d0[1] * d1[0];
            if cross.abs() < 1e-9 && d0[0] * d1[0] + d0[1] * d1[1] > 0.0 {
                continue;
            }
            if join == 0 {
                out.push(disc(p));
                continue;
            }
            // The outer side of the turn.
            let s = if cross > 0.0 { -1.0 } else { 1.0 };
            let o0 = [p[0] - d0[1] * r * s, p[1] + d0[0] * r * s];
            let o1 = [p[0] - d1[1] * r * s, p[1] + d1[0] * r * s];
            let bevel = vec![p, o0, o1];
            if join == 1 {
                // Miter point: where the two offset edges meet.
                let bis = [d0[0] - d1[0], d0[1] - d1[1]];
                let bl = bis[0].hypot(bis[1]);
                let cos_half = ((1.0 + d0[0] * d1[0] + d0[1] * d1[1]) * 0.5).max(0.0).sqrt();
                let len = if cos_half > 1e-6 { r / cos_half } else { f64::INFINITY };
                if bl > 1e-9 && len <= miter_limit * r {
                    let m = [p[0] + (o0[0] + o1[0] - 2.0 * p[0]) * 0.5, p[1] + (o0[1] + o1[1] - 2.0 * p[1]) * 0.5];
                    let ml = (m[0] - p[0]).hypot(m[1] - p[1]).max(1e-12);
                    let tip = [p[0] + (m[0] - p[0]) / ml * len, p[1] + (m[1] - p[1]) / ml * len];
                    out.push(vec![p, o0, tip, o1]);
                    continue;
                }
            }
            out.push(bevel);
        }
        if cap == 0 {
            out.push(disc(pts[0]));
            out.push(disc(pts[n - 1]));
        }
    }
    out
}

/// Coverage (union, 4×4 samples per pixel) of convex polygons on a `w`×`h` grid.
pub(crate) fn raster_convex(w: usize, h: usize, polys: &[Vec<[f64; 2]>]) -> Plane {
    let mut out = Plane::new(w, h);
    let inside = |poly: &[[f64; 2]], x: f64, y: f64| {
        let n = poly.len();
        let mut sign = 0.0;
        for i in 0..n {
            let (a, b) = (poly[i], poly[(i + 1) % n]);
            let c = (b[0] - a[0]) * (y - a[1]) - (b[1] - a[1]) * (x - a[0]);
            if c.abs() < 1e-12 {
                continue;
            }
            if sign == 0.0 {
                sign = c.signum();
            } else if c.signum() != sign {
                return false;
            }
        }
        true
    };
    for poly in polys {
        if poly.len() < 3 {
            continue;
        }
        let (mut x0, mut y0, mut x1, mut y1) = (f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
        for p in poly {
            x0 = x0.min(p[0]);
            y0 = y0.min(p[1]);
            x1 = x1.max(p[0]);
            y1 = y1.max(p[1]);
        }
        let (xa, xb) = (x0.floor().max(0.0) as usize, (x1.ceil().max(0.0) as usize).min(w));
        let (ya, yb) = (y0.floor().max(0.0) as usize, (y1.ceil().max(0.0) as usize).min(h));
        for y in ya..yb {
            for x in xa..xb {
                let mut hit = 0;
                for sy in 0..4 {
                    for sx in 0..4 {
                        if inside(poly, x as f64 + (sx as f64 + 0.5) / 4.0, y as f64 + (sy as f64 + 0.5) / 4.0) {
                            hit += 1;
                        }
                    }
                }
                let v = &mut out.data[y * w + x];
                *v = v.max(hit as f32 / 16.0);
            }
        }
    }
    out
}

fn scribble(ctx: &EffectCtx, mut b: Buf) -> Buf {
    match scribble_plan(ctx, &b) {
        Some(plan) => draw_paint(&mut b, &plan),
        None if scribble_style(ctx) == 1 => b.img = Image::new(b.img.width, b.img.height),
        None => {}
    }
    b
}

fn scribble_plan(ctx: &EffectCtx, b: &Buf) -> Option<PaintPlan> {
    let pr = ctx.params;
    let which = pr.e("scribble");
    let polys: Vec<_> = if which == 0 { mask_px(ctx, b, pr.f("mask").round().max(1.0) as usize).into_iter().collect() } else { all_masks_px(ctx, b) };
    let polys: Vec<_> = polys.into_iter().filter(|p| p.0.len() >= 3).map(|(p, c, i)| (p, c, i && which == 2)).collect();
    let style = scribble_style(ctx);
    if polys.is_empty() {
        return None;
    }
    let sc = b.scale;
    let fill = pr.e("fillType");
    let ew = pr.f("edgeOptions/edgeWidth").max(0.1) * sc;
    let ang = pr.f("angle").to_radians();
    let d = [ang.cos(), ang.sin()];
    let n = [-d[1], d[0]];
    let width = (pr.f("strokeWidth") * sc).max(0.1);
    let curv = pr.f("strokeOptions/curviness") / 100.0;
    let curv_var = pr.f("strokeOptions/curvinessVariation") / 100.0;
    let spacing = (pr.f("strokeOptions/spacing") * sc).max(0.75).max(width * 0.25);
    let spacing_var = pr.f("strokeOptions/spacingVariation") / 100.0;
    let overlap = pr.f("strokeOptions/pathOverlap") / 100.0;
    let overlap_var = pr.f("strokeOptions/pathOverlapVariation") / 100.0;
    let start = pr.f("start").clamp(0.0, 100.0) / 100.0;
    let end = pr.f("end").clamp(0.0, 100.0) / 100.0;
    let sequential = pr.b("fillPathsSequentially");
    let wiggle = pr.e("wiggleType");
    let wps = pr.f("wigglesPerSecond").max(0.0);
    let seed = pr.f("randomSeed") as i64 as u32 ^ ctx.seed;
    let (step, frac) = match wiggle {
        0 => (0u32, 0.0f32),
        1 => ((ctx.time * wps).floor().max(0.0) as u32, 0.0),
        _ => {
            let u = (ctx.time * wps).max(0.0);
            (u.floor() as u32, smoothstep(0.0, 1.0, u.fract() as f32))
        }
    };
    let rnd = |i: u32, k: u32| {
        let a = hash1(i, k.wrapping_mul(31).wrapping_add(step.wrapping_mul(7919)), seed) * 2.0 - 1.0;
        if frac > 0.0 {
            let c = hash1(i, k.wrapping_mul(31).wrapping_add((step + 1).wrapping_mul(7919)), seed) * 2.0 - 1.0;
            a + (c - a) * frac
        } else {
            a
        }
    };
    let groups: Vec<Vec<(Vec<[f64; 2]>, bool, bool)>> =
        if which == 0 || sequential { vec![polys.clone()] } else { polys.iter().map(|p| vec![p.clone()]).collect() };
    // Per group a zig-zag polyline (in buffer px).
    let mut lines: Vec<Vec<Vec<[f64; 2]>>> = Vec::new();
    let mut runs = Vec::new();
    for (gi, g) in groups.iter().enumerate() {
        let mut nmin = f64::INFINITY;
        let mut nmax = f64::NEG_INFINITY;
        for (pts, _, _) in g {
            for q in pts {
                let v = q[0] * n[0] + q[1] * n[1];
                nmin = nmin.min(v);
                nmax = nmax.max(v);
            }
        }
        nmin -= ew;
        nmax += ew;
        // Chains of zig-zags: each run joins the nearest chain that ended on the previous
        // scan line (so separate regions get separate strokes).
        let mut chains: Vec<(Vec<[f64; 2]>, u32)> = Vec::new();
        let mut s = nmin + spacing * 0.5;
        let mut li = 0u32;
        let reach = spacing * 3.0 + ew;
        while s < nmax && li < 20_000 {
            scan_runs(g, fill, ew, d, n, s, &mut runs);
            for (ri, &(a, bb)) in runs.iter().enumerate() {
                let k = (gi as u32) << 20 | ri as u32;
                let ov = spacing * (overlap + overlap_var * rnd(li, k * 4) as f64 * 0.5).max(-0.45);
                let ov2 = spacing * (overlap + overlap_var * rnd(li, k * 4 + 1) as f64 * 0.5).max(-0.45);
                let (a, bb) = (a - ov, bb + ov2);
                if bb <= a {
                    continue;
                }
                let at = |t: f64| [d[0] * t + n[0] * s, d[1] * t + n[1] * s];
                let (pa, pb) = (at(a), at(bb));
                let dist = |p: [f64; 2], q: [f64; 2]| ((p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2)).sqrt();
                let best = chains
                    .iter()
                    .enumerate()
                    .filter(|(_, c)| li > 0 && c.1 == li - 1)
                    .map(|(i, c)| {
                        let last = *c.0.last().unwrap_or(&pa);
                        (i, dist(last, pa).min(dist(last, pb)))
                    })
                    .filter(|&(_, dd)| dd < reach)
                    .min_by(|x, y| x.1.total_cmp(&y.1));
                match best {
                    Some((ci, _)) => {
                        let last = *chains[ci].0.last().unwrap_or(&pa);
                        let (p0, p1, dir) = if dist(last, pa) <= dist(last, pb) { (pa, pb, -1.0) } else { (pb, pa, 1.0) };
                        // Curved turn: an extra point pushed past the edge.
                        let c = (curv + curv_var * rnd(li, k * 4 + 2) as f64 * 0.5).max(0.0);
                        let mid = [(last[0] + p0[0]) * 0.5 + d[0] * dir * c * spacing, (last[1] + p0[1]) * 0.5 + d[1] * dir * c * spacing];
                        let ch = &mut chains[ci];
                        ch.0.extend([mid, p0, p1]);
                        ch.1 = li;
                    }
                    None => chains.push((vec![pa, pb], li)),
                }
            }
            s += spacing * (1.0 + spacing_var * rnd(li, 3) as f64 * 0.5).max(0.25);
            li += 1;
        }
        lines.push(chains.into_iter().map(|(z, _)| if curv > 0.0 { chaikin(&z, 2) } else { z }).collect());
    }
    let r = width * 0.5;
    // Start / End Apply To: all of a mask's strokes as one path, or each stroke on its own.
    let each = pr.e("startEndApplyTo") == 1;
    let mut parts: Vec<Vec<[f64; 2]>> = Vec::new();
    for group in &lines {
        let lens: Vec<f64> = group.iter().map(|l| poly_length(l, false)).collect();
        let total: f64 = lens.iter().sum();
        let (g0, g1) = (total * start, total * end);
        let mut acc = 0.0;
        for (l, len) in group.iter().zip(lens) {
            let part =
                if each { trim_poly(l, false, len * start, len * end) } else { trim_poly(l, false, (g0 - acc).clamp(0.0, len), (g1 - acc).clamp(0.0, len)) };
            acc += len;
            parts.push(part);
        }
    }
    let cap = pr.e("edgeOptions/endCap");
    let join = pr.e("edgeOptions/join");
    let cov = if cap == 0 && join == 0 {
        let mut segs = Vec::new();
        for part in &parts {
            poly_segs(part, false, r, 1.0, &mut segs);
        }
        Coverage::Segs { segs, hardness: 0.9 }
    } else {
        Coverage::Convex(stroke_outline(&parts, r, cap, join, pr.f("edgeOptions/miterLimit").max(1.0)))
    };
    Some(PaintPlan { cov, color: pr.color("color"), opacity: (pr.f("opacity") / 100.0) as f32, style })
}

// ---------------------------------------------------------------- Stroke

fn stroke(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let plan = stroke_plan(ctx, &b);
    draw_paint(&mut b, &plan);
    b
}

fn stroke_plan(ctx: &EffectCtx, b: &Buf) -> PaintPlan {
    let pr = ctx.params;
    let all = pr.b("allMasks");
    let polys: Vec<_> = if all { all_masks_px(ctx, b) } else { mask_px(ctx, b, pr.f("path").round() as usize).into_iter().collect() };
    let style = pr.e("paintStyle");
    let size = (pr.f("brushSize") * b.scale).max(0.1);
    let r = size * 0.5;
    let hard = pr.f("brushHardness") / 100.0;
    let start = pr.f("start").clamp(0.0, 100.0) / 100.0;
    let end = pr.f("end").clamp(0.0, 100.0) / 100.0;
    let spacing = pr.f("spacing") / 100.0;
    let sequential = pr.b("strokeSequentially");
    let lens: Vec<f64> = polys.iter().map(|(p, c, _)| poly_length(p, *c)).collect();
    let total: f64 = lens.iter().sum();
    let mut segs = Vec::new();
    let mut acc = 0.0;
    for ((pts, closed, _), &len) in polys.iter().zip(&lens) {
        let (s0, s1) = if all && sequential {
            let (g0, g1) = (total * start, total * end);
            ((g0 - acc).clamp(0.0, len), (g1 - acc).clamp(0.0, len))
        } else {
            (len * start, len * end)
        };
        acc += len;
        if s1 <= s0 {
            continue;
        }
        if spacing <= 0.5 {
            let part = trim_poly(pts, *closed, s0, s1);
            poly_segs(&part, false, r, 1.0, &mut segs);
        } else {
            let step = (size * spacing).max(0.5);
            let mut s = s0;
            while s <= s1 + 1e-9 && segs.len() < 200_000 {
                let (q, _) = crate::util::poly_point_at(pts, *closed, s);
                segs.push(Seg { a: q, b: q, r, v: 1.0 });
                s += step;
            }
        }
    }
    PaintPlan { cov: Coverage::Segs { segs, hardness: hard }, color: pr.color("color"), opacity: (pr.f("opacity") / 100.0) as f32, style }
}

// ---------------------------------------------------------------- Vegas

/// Marching-squares iso-contours of `pl` at `thr`, chained into polylines (closed flag).
fn contours(pl: &Plane, thr: f32) -> Vec<(Vec<[f64; 2]>, bool)> {
    use std::collections::HashMap;
    let (w, h) = (pl.w, pl.h);
    if w < 2 || h < 2 {
        return Vec::new();
    }
    // Edge key: horizontal edge (x,y)-(x+1,y) = 2*(y*w+x), vertical (x,y)-(x,y+1) = 2*(y*w+x)+1.
    let hk = |x: usize, y: usize| 2 * (y * w + x) as u64;
    let vk = |x: usize, y: usize| 2 * (y * w + x) as u64 + 1;
    let mut pos: HashMap<u64, [f64; 2]> = HashMap::new();
    let mut adj: HashMap<u64, Vec<u64>> = HashMap::new();
    let v = |x: usize, y: usize| pl.data[y * w + x];
    let interp = |a: f32, b: f32| if (b - a).abs() > 1e-9 { ((thr - a) / (b - a)).clamp(0.0, 1.0) as f64 } else { 0.5 };
    for y in 0..h - 1 {
        for x in 0..w - 1 {
            let (a, bb, c, d) = (v(x, y), v(x + 1, y), v(x + 1, y + 1), v(x, y + 1));
            let idx = (a > thr) as u8 | ((bb > thr) as u8) << 1 | ((c > thr) as u8) << 2 | ((d > thr) as u8) << 3;
            if idx == 0 || idx == 15 {
                continue;
            }
            // Edge points (pixel centres at +0.5).
            let top = (hk(x, y), [x as f64 + 0.5 + interp(a, bb), y as f64 + 0.5]);
            let right = (vk(x + 1, y), [x as f64 + 1.5, y as f64 + 0.5 + interp(bb, c)]);
            let bottom = (hk(x, y + 1), [x as f64 + 0.5 + interp(d, c), y as f64 + 1.5]);
            let left = (vk(x, y), [x as f64 + 0.5, y as f64 + 0.5 + interp(a, d)]);
            let pairs: &[(usize, usize)] = match idx {
                1 | 14 => &[(3, 0)],
                2 | 13 => &[(0, 1)],
                3 | 12 => &[(3, 1)],
                4 | 11 => &[(1, 2)],
                5 => &[(3, 0), (1, 2)],
                6 | 9 => &[(0, 2)],
                7 | 8 => &[(3, 2)],
                10 => &[(0, 1), (2, 3)],
                _ => &[],
            };
            let e = [top, right, bottom, left];
            for &(i, j) in pairs {
                pos.insert(e[i].0, e[i].1);
                pos.insert(e[j].0, e[j].1);
                adj.entry(e[i].0).or_default().push(e[j].0);
                adj.entry(e[j].0).or_default().push(e[i].0);
            }
        }
    }
    let mut keys: Vec<u64> = adj.keys().copied().collect();
    keys.sort_unstable();
    // Start open chains at endpoints (degree 1) first, then closed loops.
    keys.sort_by_key(|k| adj[k].len() != 1);
    let mut used: std::collections::HashSet<(u64, u64)> = std::collections::HashSet::new();
    let mut out = Vec::new();
    for &k0 in &keys {
        for &first in &adj[&k0] {
            if used.contains(&(k0.min(first), k0.max(first))) {
                continue;
            }
            let mut chain = vec![pos[&k0]];
            let (mut prev, mut cur) = (k0, first);
            used.insert((prev.min(cur), prev.max(cur)));
            let mut closed = false;
            loop {
                chain.push(pos[&cur]);
                if cur == k0 {
                    closed = true;
                    chain.pop();
                    break;
                }
                let next = adj[&cur].iter().copied().find(|&n| n != prev && !used.contains(&(cur.min(n), cur.max(n))));
                match next {
                    Some(n) => {
                        used.insert((cur.min(n), cur.max(n)));
                        prev = cur;
                        cur = n;
                    }
                    None => break,
                }
            }
            if chain.len() >= 2 {
                out.push((chain, closed));
            }
        }
    }
    out
}

fn vegas(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let plan = vegas_plan(ctx, &b);
    draw_paint(&mut b, &plan);
    b
}

fn vegas_plan(ctx: &EffectCtx, b: &Buf) -> PaintPlan {
    let pr = ctx.params;
    let (w, h) = (b.img.width as usize, b.img.height as usize);
    let sc = b.scale;
    let paths: Vec<(Vec<[f64; 2]>, bool)> = if pr.e("stroke") == 1 {
        mask_px(ctx, b, pr.f("path").round() as usize).map(|(p, c, _)| vec![(p, c)]).unwrap_or_default()
    } else {
        let src = layer_or_self(ctx, b, "inputLayer", true, pr.e("ifLayerSizesDiffer") == 1);
        let ch = pr.e("channel");
        let inv = pr.b("invertInput");
        let mut pl = Plane::from_image(&src, |px| {
            let (c, a) = unpremul(px);
            let v = match ch {
                1 => c[0],
                2 => c[1],
                3 => c[2],
                4 => a,
                _ => luminance(c[0], c[1], c[2]) * a,
            };
            if inv { 1.0 - v } else { v }
        });
        let blur = pr.f("preBlur") * sc;
        if blur > 0.05 {
            pl = gauss_plane(&pl, blur, blur);
        }
        let mut cs = contours(&pl, (pr.f("threshold") / 100.0) as f32);
        // Simplify by tolerance (drop points closer than tol × size).
        let tol = pr.f("tolerance").max(0.0) * (w.max(h) as f64) * 0.1;
        if tol > 0.0 {
            for (c, _) in cs.iter_mut() {
                let mut keep: Vec<[f64; 2]> = Vec::with_capacity(c.len());
                for &q in c.iter() {
                    if keep.last().is_none_or(|l: &[f64; 2]| ((l[0] - q[0]).powi(2) + (l[1] - q[1]).powi(2)).sqrt() >= tol) {
                        keep.push(q);
                    }
                }
                *c = keep;
            }
        }
        cs.sort_by(|a, b| poly_length(&b.0, b.1).total_cmp(&poly_length(&a.0, a.1)));
        if pr.e("renderMode") == 1 {
            let sel = (pr.f("selectedContour").round().max(1.0) as usize) - 1;
            cs.into_iter().skip(sel).take(1).collect()
        } else {
            cs
        }
    };
    let nseg = pr.f("segments").round().clamp(1.0, 1000.0) as usize;
    // Shorter Contours Have Fewer Segments: segment count scales with contour length.
    let fewer = pr.e("stroke") == 0 && pr.e("shorterContoursHave") == 1;
    let longest = paths.iter().map(|(p, c)| poly_length(p, *c)).fold(0.0, f64::max);
    let seg_len = pr.f("length").clamp(0.0, 1.0);
    let bunched = pr.e("segmentDistribution") == 0;
    let rot = pr.f("rotation") / 360.0;
    let random_phase = pr.b("randomPhase");
    let seed = pr.f("randomSeed") as i64 as u32 ^ ctx.seed;
    let width = (pr.f("width") * sc).max(0.1);
    let hard = pr.f("hardness").clamp(0.0, 1.0);
    let (o0, om, o1, mp) = (pr.f("startOpacity") as f32, pr.f("midpointOpacity") as f32, pr.f("endOpacity") as f32, pr.f("midpointPosition").clamp(0.01, 0.99));
    let mut segs = Vec::new();
    for (ci, (pts, closed)) in paths.iter().enumerate() {
        let len = poly_length(pts, *closed);
        if len <= 0.0 {
            continue;
        }
        let phase = rot + if random_phase { hash1(ci as u32, 1, seed) as f64 } else { 0.0 };
        let nseg = if fewer && longest > 0.0 { ((nseg as f64 * len / longest).round() as usize).max(1) } else { nseg };
        let each = len / nseg as f64 * seg_len;
        for i in 0..nseg {
            let base = if bunched { (i as f64 + (hash1(ci as u32, i as u32 + 7, seed) as f64 - 0.5) * 0.8) / nseg as f64 } else { i as f64 / nseg as f64 };
            let s0 = (base + phase).rem_euclid(1.0) * len;
            // Sub-sample the segment so opacity can ramp along it.
            let sub = 12usize;
            let mut prev: Option<[f64; 2]> = None;
            for k in 0..=sub {
                let f = k as f64 / sub as f64;
                let mut s = s0 + each * f;
                if *closed {
                    s = s.rem_euclid(len);
                } else if s > len {
                    break;
                }
                let (q, _) = crate::util::poly_point_at(pts, *closed, s);
                let op = if f < mp { o0 + (om - o0) * (f / mp) as f32 } else { om + (o1 - om) * ((f - mp) / (1.0 - mp)) as f32 };
                if let Some(pv) = prev
                    && ((pv[0] - q[0]).powi(2) + (pv[1] - q[1]).powi(2)) < (each * 0.5).powi(2) + 4.0
                {
                    segs.push(Seg { a: pv, b: q, r: width * 0.5, v: op.clamp(0.0, 1.0) });
                }
                prev = Some(q);
            }
        }
    }
    let style = match pr.e("blendMode") {
        1 => 0,
        2 => 3,
        3 => 2,
        _ => 1,
    };
    PaintPlan { cov: Coverage::Segs { segs, hardness: hard }, color: pr.color("color"), opacity: 1.0, style }
}

// ---------------------------------------------------------------- Write-on / Glue Gun

/// Write-on draws a stroke along the animated Brush Position, as in After Effects: a dab every
/// Brush Spacing seconds from the layer's start up to now, each one lasting Stroke Length
/// seconds (0: for ever). The dabs' positions come from the host's parameters at their times;
/// with Paint Time Properties ▸ Opacity or Brush Time Properties ▸ Size each dab keeps its own
/// opacity or size, else the current ones apply to every dab. (Colour and hardness are one per
/// stroke: the current ones.) Without a host (or dabs) the current dab draws. Deterministic.
fn write_on(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let plan = write_on_plan(ctx, &b);
    draw_paint(&mut b, &plan);
    b
}

/// The most dabs one Write-on frame draws; longer strokes lay them further apart, joined.
const WRITE_ON_MAX_DABS: i64 = 4096;

fn write_on_plan(ctx: &EffectCtx, b: &Buf) -> PaintPlan {
    let pr = ctx.params;
    let own_opacity = pr.e("paintTimeProperties") == 1;
    let own_size = matches!(pr.e("brushTimeProperties"), 1 | 3);
    let opacity = |p: &Params| (p.f("brushOpacity") / 100.0).clamp(0.0, 1.0) as f32;
    let radius = |p: &Params| (p.f("brushSize") * b.scale * 0.5).max(0.05);
    let dab = |p: &Params| {
        let q = b.to_px(p.v2("brushPosition"));
        ([q.0, q.1], radius(if own_size { p } else { pr }), if own_opacity { opacity(p) } else { 1.0 })
    };
    let mut segs = vec![];
    let t = ctx.time;
    let spacing = pr.f("brushSpacing").max(0.001);
    let life = pr.f("strokeLength").max(0.0);
    if let Some(host) = ctx.env.host
        && t.is_finite()
        && t >= 0.0
    {
        let start = if life > 0.0 { (t - life).max(0.0) } else { 0.0 };
        let k0 = (start / spacing - 1e-9).ceil().max(0.0) as i64;
        let k1 = (t / spacing + 1e-9).floor().max(0.0) as i64;
        let n = k1.saturating_sub(k0).saturating_add(1);
        let stride = (n / WRITE_ON_MAX_DABS).saturating_add(1).max(1);
        let mut prev: Option<[f64; 2]> = None;
        let mut k = k0;
        while k <= k1 {
            let tk = k as f64 * spacing;
            let at = if (tk - t).abs() < 1e-9 { Some(dab(pr)) } else { host.params_at(tk).map(|p| dab(&p)) };
            if let Some((q, r, v)) = at {
                // Dabs laid further apart than the brush spacing are joined into a line.
                let a = if stride > 1 { prev.unwrap_or(q) } else { q };
                segs.push(Seg { a, b: q, r, v });
                prev = Some(q);
            }
            k = k.saturating_add(stride);
        }
    }
    if segs.is_empty() {
        let (q, r, v) = dab(pr);
        segs.push(Seg { a: q, b: q, r, v });
    }
    PaintPlan {
        cov: Coverage::Segs { segs, hardness: pr.f("brushHardness") / 100.0 },
        color: pr.color("color"),
        opacity: if own_opacity { 1.0 } else { opacity(pr) },
        style: pr.e("paintStyle"),
    }
}

fn glue_gun(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let pr = ctx.params;
    let q = b.to_px(pr.v2("brushPosition"));
    let rad = (pr.f("strokeWidth") * b.scale * 0.5).max(0.5);
    let density = pr.f("density").max(0.0);
    let refl = pr.f("reflection").to_radians();
    let strength = (pr.f("strength") / 100.0) as f32;
    let glue_only = pr.e("style") == 1;
    let src = b.img.clone();
    let light = [refl.cos() * 0.6, -refl.sin() * 0.6, 0.53];
    let beads = (density.round() as usize).clamp(1, 32);
    let seed = ctx.seed;
    let centers: Vec<([f64; 2], f64)> = (0..beads)
        .map(|i| {
            let a = hash1(i as u32, 1, seed) as f64 * 2.0 * PI;
            let d = if i == 0 { 0.0 } else { hash1(i as u32, 2, seed) as f64 * rad * 0.35 };
            ([q.0 + a.cos() * d, q.1 + a.sin() * d], rad * (0.75 + 0.25 * hash1(i as u32, 3, seed) as f64))
        })
        .collect();
    b.img.rows_mut().for_each(|(y, row)| {
        for (x, px) in row.iter_mut().enumerate() {
            let (fx, fy) = (x as f64 + 0.5, y as f64 + 0.5);
            // Height field: union of hemispheres.
            let mut best = (0.0f64, [0.0f64; 2], 1.0f64);
            for &(c, r) in &centers {
                let (dx, dy) = ((fx - c[0]) / r, (fy - c[1]) / r);
                let d2 = dx * dx + dy * dy;
                if d2 < 1.0 {
                    let hgt = (1.0 - d2).sqrt();
                    if hgt > best.0 {
                        best = (hgt, [dx, dy], r);
                    }
                }
            }
            if best.0 <= 0.0 {
                if glue_only {
                    *px = [0.0; 4];
                }
                continue;
            }
            let nrm = [best.1[0], best.1[1], best.0];
            let ndl = (nrm[0] * light[0] + nrm[1] * light[1] + nrm[2] * light[2]).max(0.0);
            let spec = ndl.powi(24) as f32;
            // Refraction: look through the bead.
            let s = src.sample_bilinear_clamped(fx - nrm[0] * best.2 * 0.5 * strength as f64, fy - nrm[1] * best.2 * 0.5 * strength as f64);
            let edge = smoothstep(0.0, 0.15, best.0 as f32);
            let shade = 0.65 + 0.35 * ndl as f32;
            let g = [s[0] * shade + spec, s[1] * shade + spec, s[2] * shade + spec, s[3].max(0.6)];
            let gp = [g[0].min(g[3] + spec), g[1].min(g[3] + spec), g[2].min(g[3] + spec), g[3].min(1.0)];
            let base = if glue_only { [0.0; 4] } else { *px };
            *px = crate::util::lerp4(base, gp, edge * strength.clamp(0.0, 1.0).max(0.3));
            px[3] = px[3].clamp(0.0, 1.0);
        }
    });
    b
}

// ---------------------------------------------------------------- Paint Bucket

/// Paint Bucket's filled region (1 inside) on `b`: the flood fill from the Fill Point over the
/// layer's pixels (shared with the GPU compositor, which reads the layer back for it).
pub fn paint_bucket_region(ctx: &EffectCtx, b: &Buf) -> Plane {
    let pr = ctx.params;
    let (w, h) = (b.img.width as usize, b.img.height as usize);
    let fp = b.to_px(pr.v2("fillPoint"));
    let (sx, sy) = (fp.0.floor() as i64, fp.1.floor() as i64);
    let sel = pr.e("fillSelector");
    let tol = (pr.f("tolerance") / 255.0) as f32;
    let key = |p: Px| -> [f32; 4] {
        let (c, a) = unpremul(p);
        match sel {
            1 => [c[0], c[1], c[2], 0.0],
            2 => [0.0, 0.0, 0.0, if a <= 1e-3 { 0.0 } else { 1.0 }],
            3 | 4 => [0.0, 0.0, 0.0, a],
            _ => p,
        }
    };
    let mut region = Plane::new(w, h);
    if sx >= 0 && sy >= 0 && (sx as usize) < w && (sy as usize) < h {
        let seed = key(b.img.data[sy as usize * w + sx as usize]);
        let within = |i: usize| {
            let k = key(b.img.data[i]);
            if sel == 2 {
                return k[3] == seed[3];
            }
            (0..4).all(|j| (k[j] - seed[j]).abs() <= tol + 1e-6)
        };
        let mut seen = vec![false; w * h];
        let mut stack = vec![(sx as usize, sy as usize)];
        while let Some((x, y)) = stack.pop() {
            let row = y * w;
            if seen[row + x] || !within(row + x) {
                continue;
            }
            let mut l = x;
            while l > 0 && !seen[row + l - 1] && within(row + l - 1) {
                l -= 1;
            }
            let mut r = x;
            while r + 1 < w && !seen[row + r + 1] && within(row + r + 1) {
                r += 1;
            }
            for i in l..=r {
                seen[row + i] = true;
                region.data[row + i] = 1.0;
                if y > 0 && !seen[row - w + i] {
                    stack.push((i, y - 1));
                }
                if y + 1 < h && !seen[row + w + i] {
                    stack.push((i, y + 1));
                }
            }
        }
    }
    region
}

fn paint_bucket(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let pr = ctx.params;
    let (w, h) = (b.img.width as usize, b.img.height as usize);
    if w == 0 || h == 0 {
        return b;
    }
    let region = paint_bucket_region(ctx, &b);
    if pr.b("viewThreshold") {
        b.img.data.par_iter_mut().zip(region.data.par_iter()).for_each(|(px, &v)| *px = [v, v, v, 1.0]);
        return b;
    }
    let sc = b.scale;
    let mut cov = match pr.e("stroke") {
        1 => {
            let s = pr.f("featherSoftness") * sc * 0.5;
            if s > 0.05 { gauss_plane(&region, s, s) } else { region.clone() }
        }
        2 => morph_frac(&region, pr.f("spreadRadius") * sc, true),
        3 => morph_frac(&region, pr.f("spreadRadius") * sc, false),
        4 => {
            let r = pr.f("strokeWidth") * sc;
            let out = morph_frac(&region, r * 0.5, true);
            let inn = morph_frac(&region, r * 0.5, false);
            out.zip_map(&inn, |a, b| (a - b).max(0.0))
        }
        _ => gauss_plane(&region, 0.5, 0.5),
    };
    if pr.e("stroke") == 0 {
        // Antialias: keep the hard interior, soften only the boundary.
        cov = cov.zip_map(&region, |a, r| if r > 0.0 { a.max(0.5) } else { a });
    }
    let color = pr.color("color");
    let op = (pr.f("opacity") / 100.0) as f32;
    let mode = pr.e("blendingMode");
    b.img.data.par_iter_mut().zip(cov.data.par_iter()).for_each(|(px, &c)| {
        let k = (c * op * color[3]).clamp(0.0, 1.0);
        if k <= 0.0 {
            if mode == 9 {
                *px = [0.0; 4];
            }
            return;
        }
        let (dc, da) = unpremul(*px);
        let fc = [color[0], color[1], color[2]];
        let mixed = match mode {
            1 => [dc[0] + fc[0], dc[1] + fc[1], dc[2] + fc[2]],
            2 => [dc[0] * fc[0], dc[1] * fc[1], dc[2] * fc[2]],
            3 => [1.0 - (1.0 - dc[0]) * (1.0 - fc[0]), 1.0 - (1.0 - dc[1]) * (1.0 - fc[1]), 1.0 - (1.0 - dc[2]) * (1.0 - fc[2])],
            4..=7 => {
                let (dh, ds, dl) = rgb_to_hsl(dc[0], dc[1], dc[2]);
                let (fh, fs, fl) = rgb_to_hsl(fc[0], fc[1], fc[2]);
                let (r, g, bl) = match mode {
                    4 => hsl_to_rgb(fh, ds, dl),
                    5 => hsl_to_rgb(dh, fs, dl),
                    6 => hsl_to_rgb(fh, fs, dl),
                    _ => hsl_to_rgb(dh, ds, fl),
                };
                [r, g, bl]
            }
            _ => fc,
        };
        *px = match mode {
            8 => over(premul(fc, k), *px),
            9 => premul(fc, k),
            0 => over(*px, premul(fc, k)),
            _ => {
                if da <= 1e-6 {
                    return;
                }
                let c3 = crate::util::lerp3(dc, mixed, k);
                premul(c3, da)
            }
        };
    });
    b
}

// ---------------------------------------------------------------- Eyedropper Fill

fn eyedropper_fill(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let pr = ctx.params;
    let fill = eyedropper_color(ctx, &b);
    let keep_alpha = pr.b("maintainOriginalAlpha");
    let blend = (pr.f("blendWithOriginal") / 100.0) as f32;
    b.img.data.par_iter_mut().for_each(|px| {
        let a = if keep_alpha { px[3] } else { fill[3] };
        let f = premul([fill[0], fill[1], fill[2]], a);
        *px = crate::util::lerp4(f, *px, blend);
    });
    b
}

/// Eyedropper Fill's sampled colour (straight RGB, alpha) from the layer's pixels (shared with
/// the GPU compositor, which reads the layer back for it).
pub fn eyedropper_color(ctx: &EffectCtx, b: &Buf) -> Px {
    let pr = ctx.params;
    let (w, h) = (b.img.width as i64, b.img.height as i64);
    let c = b.to_px(pr.v2("samplePoint"));
    let r = (pr.f("sampleRadius") * b.scale).max(0.0);
    let mode = pr.e("averagePixelColors");
    let (cx, cy) = (c.0.floor() as i64, c.1.floor() as i64);
    let ri = r.ceil() as i64;
    let mut acc = [0.0f64; 4];
    let mut n = 0.0f64;
    for y in (cy - ri).max(0)..=(cy + ri).min(h - 1) {
        for x in (cx - ri).max(0)..=(cx + ri).min(w - 1) {
            if ((x - cx).pow(2) + (y - cy).pow(2)) as f64 > r * r + 0.5 {
                continue;
            }
            let px = b.img.data[(y * w + x) as usize];
            match mode {
                0 => {
                    if px[3] > 1e-4 {
                        let (s, _) = unpremul(px);
                        for k in 0..3 {
                            acc[k] += s[k] as f64;
                        }
                        acc[3] += 1.0;
                        n += 1.0;
                    }
                }
                1 => {
                    let (s, _) = unpremul(px);
                    for k in 0..3 {
                        acc[k] += s[k] as f64;
                    }
                    acc[3] += 1.0;
                    n += 1.0;
                }
                2 => {
                    for k in 0..3 {
                        acc[k] += px[k] as f64;
                    }
                    acc[3] += 1.0;
                    n += 1.0;
                }
                _ => {
                    for k in 0..4 {
                        acc[k] += px[k] as f64;
                    }
                    n += 1.0;
                }
            }
        }
    }
    if n > 0.0 {
        if mode == 3 {
            let a = (acc[3] / n) as f32;
            let s = if a > 1e-6 { [(acc[0] / n) as f32 / a, (acc[1] / n) as f32 / a, (acc[2] / n) as f32 / a] } else { [0.0; 3] };
            [s[0], s[1], s[2], a]
        } else {
            [(acc[0] / n) as f32, (acc[1] / n) as f32, (acc[2] / n) as f32, 1.0]
        }
    } else {
        [0.0, 0.0, 0.0, 0.0]
    }
}

// ---------------------------------------------------------------- CC Threads

fn threads(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let pr = ctx.params;
    let sc = b.scale;
    let tw = (pr.f("width") * sc).max(1.0);
    let th = (pr.f("height") * sc).max(1.0);
    let overlaps = pr.f("overlaps").round().clamp(1.0, 20.0) as i64;
    let ang = pr.f("direction").to_radians();
    let (sn, cs) = ang.sin_cos();
    let c = b.to_px(pr.v2("center"));
    let coverage = (pr.f("coverage") / 100.0).clamp(0.0, 1.0);
    let shadow = (pr.f("shadowing") / 100.0) as f32;
    let texture = (pr.f("texture") / 100.0) as f32;
    let src = b.img.clone();
    let to_px = |u: f64, v: f64| (c.0 + u * cs - v * sn, c.1 + u * sn + v * cs);
    b.img.rows_mut().for_each(|(y, row)| {
        for (x, px) in row.iter_mut().enumerate() {
            let (dx, dy) = (x as f64 + 0.5 - c.0, y as f64 + 0.5 - c.1);
            let u = dx * cs + dy * sn;
            let v = -dx * sn + dy * cs;
            let (ci, cj) = ((u / tw).floor() as i64, (v / th).floor() as i64);
            let (fu, fv) = (u / tw - ci as f64, v / th - cj as f64);
            // Horizontal thread j occupies the band |fv-0.5| < coverage/2; vertical thread i the
            // band |fu-0.5| < coverage/2.
            let in_h = (fv - 0.5).abs() < coverage * 0.5;
            let in_v = (fu - 0.5).abs() < coverage * 0.5;
            // Twill: horizontal is on top when (i + j) mod (2·overlaps) < overlaps.
            let h_top = (ci + cj).rem_euclid(2 * overlaps) < overlaps;
            let which = match (in_h, in_v) {
                (true, true) => Some(h_top),
                (true, false) => Some(true),
                (false, true) => Some(false),
                _ => None,
            };
            let Some(horizontal) = which else {
                *px = [0.0; 4];
                continue;
            };
            // Thread colour: the source sampled along the thread centre line.
            let (sx, sy) = if horizontal { to_px(u, (cj as f64 + 0.5) * th) } else { to_px((ci as f64 + 0.5) * tw, v) };
            let s = src.sample_bilinear_clamped(sx, sy);
            // Round profile across the thread + darkening where it dives under its neighbour.
            let across = if horizontal { (fv - 0.5) / (coverage * 0.5).max(1e-6) } else { (fu - 0.5) / (coverage * 0.5).max(1e-6) };
            let along = if horizontal { fu } else { fv };
            let profile = (1.0 - across * across).max(0.0).sqrt() as f32;
            let dive = 1.0 - (along - 0.5).abs() as f32 * 2.0;
            let shade = 1.0 - shadow * (1.0 - profile) * 0.8 - shadow * 0.25 * (1.0 - dive) * if horizontal == h_top { 0.0 } else { 1.0 };
            let fib =
                if texture > 0.0 { 1.0 - texture * 0.3 * hash1((along * 64.0) as u32, (across * 8.0 + 8.0) as u32, ci as u32 ^ (cj as u32) << 8) } else { 1.0 };
            let k = (shade * fib).max(0.0);
            *px = [s[0] * k, s[1] * k, s[2] * k, s[3]];
        }
    });
    b
}

// ---------------------------------------------------------------- Audio Spectrum / Waveform

/// In-place iterative radix-2 FFT (`re`, `im` lengths a power of two).
pub(crate) fn fft(re: &mut [f64], im: &mut [f64]) {
    let n = re.len();
    if n < 2 {
        return;
    }
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let ang = -2.0 * PI / len as f64;
        let (ws, wc) = ang.sin_cos();
        for start in (0..n).step_by(len) {
            let (mut cr, mut ci) = (1.0, 0.0);
            for k in 0..len / 2 {
                let (a, bb) = (start + k, start + k + len / 2);
                let tr = re[bb] * cr - im[bb] * ci;
                let ti = re[bb] * ci + im[bb] * cr;
                re[bb] = re[a] - tr;
                im[bb] = im[a] - ti;
                re[a] += tr;
                im[a] += ti;
                let ncr = cr * wc - ci * ws;
                ci = cr * ws + ci * wc;
                cr = ncr;
            }
        }
        len <<= 1;
    }
}

const AUDIO_RATE: u32 = 48_000;

/// Fetch mono (or one channel) samples of the audio layer around the current time.
fn audio_window(ctx: &EffectCtx, dur_ms: f64, offset_ms: f64, channel: u32) -> Option<Vec<f64>> {
    let id = ctx.params.get("audioLayer")?.as_layer()?;
    let host = ctx.env.host?;
    let dur = (dur_ms / 1000.0).max(0.001);
    let frames = ((dur * AUDIO_RATE as f64) as usize).clamp(16, 1 << 16);
    let start = ctx.env.comp_time + offset_ms / 1000.0 - dur * 0.5;
    let s = host.audio(id, start, frames, AUDIO_RATE)?;
    Some(
        (0..frames)
            .map(|i| {
                let (l, r) = (*s.get(2 * i).unwrap_or(&0.0) as f64, *s.get(2 * i + 1).unwrap_or(&0.0) as f64);
                match channel {
                    1 => l,
                    2 => r,
                    _ => (l + r) * 0.5,
                }
            })
            .collect(),
    )
}

/// The drawing baseline: a polyline in buffer px (mask path or start→end line), sampled at
/// parameter u∈[0,1] → (point, unit normal).
struct Baseline {
    pts: Vec<[f64; 2]>,
    closed: bool,
    len: f64,
    polar: bool,
    center: [f64; 2],
    radius: f64,
}

impl Baseline {
    fn new(ctx: &EffectCtx, b: &Buf) -> Baseline {
        let pr = ctx.params;
        let polar = pr.b("usePolarPath");
        let (pts, closed) = match mask_px(ctx, b, pr.f("path").round() as usize) {
            Some((p, c, _)) if !polar => (p, c),
            _ => {
                let s = b.to_px(pr.v2("startPoint"));
                let e = b.to_px(pr.v2("endPoint"));
                (vec![[s.0, s.1], [e.0, e.1]], false)
            }
        };
        let len = poly_length(&pts, closed);
        let center = pts[0];
        let radius = if pts.len() > 1 { ((pts[1][0] - pts[0][0]).powi(2) + (pts[1][1] - pts[0][1]).powi(2)).sqrt() } else { 1.0 };
        Baseline { pts, closed, len, polar, center, radius }
    }
    fn at(&self, u: f64) -> ([f64; 2], [f64; 2]) {
        if self.polar {
            let a = u * 2.0 * PI - PI * 0.5;
            let (s, c) = a.sin_cos();
            return ([self.center[0] + c * self.radius * 0.5, self.center[1] + s * self.radius * 0.5], [c, s]);
        }
        let (q, d) = crate::util::poly_point_at(&self.pts, self.closed, u * self.len);
        (q, [d[1], -d[0]])
    }
}

/// Draw line/dot marks with core (inside colour) and soft halo (outside colour).
fn draw_marks(b: &mut Buf, segs_core: &[Seg], thickness: f64, softness: f64, inside: [f32; 4], outside: [f32; 4], composite: bool) {
    let (w, h) = (b.img.width as usize, b.img.height as usize);
    let core = raster_segs(w, h, segs_core, 1.0);
    let halo_segs: Vec<Seg> = segs_core.iter().map(|s| Seg { r: s.r + thickness * softness.max(0.0) + 0.5, ..*s }).collect();
    let halo = if softness > 0.0 { raster_segs(w, h, &halo_segs, 0.0) } else { core.clone() };
    b.img.data.par_iter_mut().enumerate().for_each(|(i, px)| {
        let (c, hl) = (core.data[i], halo.data[i]);
        let a = c.max(hl);
        let base = if composite { *px } else { [0.0; 4] };
        if a <= 0.0 {
            *px = base;
            return;
        }
        let t = if a > 0.0 { c / a } else { 0.0 };
        let col = crate::util::lerp3([outside[0], outside[1], outside[2]], [inside[0], inside[1], inside[2]], t);
        *px = over(base, premul(col, a));
    });
}

/// Audio Spectrum / Audio Waveform, shared with the GPU compositor (effectcraft-gpu `fx_gen2`):
/// the marks (the audio analysis runs on the CPU) and how [`draw_marks`] colours them.
#[derive(Clone, Debug)]
pub struct MarksPlan {
    pub segs: Vec<Seg>,
    pub thickness: f64,
    pub softness: f64,
    pub inside: [f32; 4],
    pub outside: [f32; 4],
    pub composite: bool,
}

/// [`MarksPlan`] of Audio Spectrum or Audio Waveform on `b`'s geometry (`None` = no audio:
/// the layer passes through).
pub fn marks_plan(id: &str, ctx: &EffectCtx, b: &Buf) -> Option<MarksPlan> {
    match id {
        "ec.generate.audiospectrum" => spectrum_plan(ctx, b),
        "ec.generate.audiowaveform" => waveform_plan(ctx, b),
        _ => None,
    }
}

fn marks(ctx: &EffectCtx, segs: Vec<Seg>, thick: f64, soft: f64) -> MarksPlan {
    let pr = ctx.params;
    MarksPlan {
        segs,
        thickness: thick,
        softness: soft * 2.0,
        inside: pr.color("insideColor"),
        outside: pr.color("outsideColor"),
        composite: pr.b("compositeOnOriginal"),
    }
}

fn audio_spectrum(ctx: &EffectCtx, mut b: Buf) -> Buf {
    if let Some(m) = spectrum_plan(ctx, &b) {
        draw_marks(&mut b, &m.segs, m.thickness, m.softness, m.inside, m.outside, m.composite);
    }
    b
}

fn audio_waveform(ctx: &EffectCtx, mut b: Buf) -> Buf {
    if let Some(m) = waveform_plan(ctx, &b) {
        draw_marks(&mut b, &m.segs, m.thickness, m.softness, m.inside, m.outside, m.composite);
    }
    b
}

fn compute_fft_mags(samples: &[f64]) -> (usize, f64, Vec<f64>) {
    let m = samples.len();
    let n = m.next_power_of_two().max(64);
    let mut re = vec![0.0; n];
    let mut im = vec![0.0; n];
    let denom = (m.max(2) - 1) as f64;
    for (i, s) in samples.iter().enumerate() {
        let w = 0.5 - 0.5 * (2.0 * PI * i as f64 / denom).cos();
        if let Some(r) = re.get_mut(i) {
            *r = s * w;
        }
    }
    fft(&mut re, &mut im);
    let hz_per_bin = AUDIO_RATE as f64 / n as f64;
    let scale = 4.0 / m.max(1) as f64;
    let num_bins = n / 2 + 1;
    let mut mags = Vec::with_capacity(num_bins);
    for k in 0..num_bins {
        let r = re.get(k).copied().unwrap_or(0.0);
        let im_val = im.get(k).copied().unwrap_or(0.0);
        let mag = (r * r + im_val * im_val).sqrt() * scale;
        mags.push(mag);
    }
    (n, hz_per_bin, mags)
}

fn spectrum_plan(ctx: &EffectCtx, b: &Buf) -> Option<MarksPlan> {
    let pr = ctx.params;
    let samples = audio_window(ctx, pr.f("audioDuration"), pr.f("audioOffset"), 0)?;
    let (_n, hz_per_bin, mags) = compute_fft_mags(&samples);

    let bands = pr.f("frequencyBands").round().clamp(1.0, 4096.0) as usize;
    let f0 = pr.f("startFrequency").max(0.0);
    let f1 = pr.f("endFrequency").max(f0 + 1.0);
    let max_bin = mags.len().saturating_sub(1);

    let get_band_mag = |fc: f64| -> f64 {
        if mags.is_empty() || hz_per_bin <= 0.0 {
            return 0.0;
        }
        let k_float = (fc / hz_per_bin).clamp(0.0, max_bin as f64);
        let k0 = (k_float.floor() as usize).min(max_bin.saturating_sub(1));
        let k1 = (k0 + 1).min(max_bin);
        let frac = (k_float - k0 as f64).clamp(0.0, 1.0);
        let m0 = mags.get(k0).copied().unwrap_or(0.0);
        let m1 = mags.get(k1).copied().unwrap_or(0.0);
        m0 * (1.0 - frac) + m1 * frac
    };

    let max_h = pr.f("maximumHeight") * b.scale;
    let thick = (pr.f("thickness") * b.scale).max(0.1);
    let soft = pr.f("softness") / 100.0;
    let disp = pr.e("displayOptions");
    let side = pr.e("sideOptions");
    let base = Baseline::new(ctx, b);
    let mut tops = Vec::with_capacity(bands);
    let mut segs = Vec::new();
    for i in 0..bands {
        let u = if bands > 1 { i as f64 / (bands - 1) as f64 } else { 0.5 };
        let fc = f0 + (f1 - f0) * u;
        let v = get_band_mag(fc);
        let hgt = if (v * max_h).is_finite() { (v * max_h).clamp(0.0, max_h.max(0.0)) } else { 0.0 };
        let (q, nrm) = base.at(u);
        let (ua, ub) = match side {
            0 => (0.0, hgt),
            1 => (-hgt, 0.0),
            _ => (-hgt, hgt),
        };
        let pa = [q[0] + nrm[0] * ua, q[1] + nrm[1] * ua];
        let pb = [q[0] + nrm[0] * ub, q[1] + nrm[1] * ub];
        match disp {
            0 => segs.push(Seg { a: pa, b: pb, r: thick * 0.5, v: 1.0 }),
            _ => tops.push((pa, pb)),
        }
    }
    if disp == 1 {
        for w2 in tops.windows(2) {
            segs.push(Seg { a: w2[0].1, b: w2[1].1, r: thick * 0.5, v: 1.0 });
            if side == 2 {
                segs.push(Seg { a: w2[0].0, b: w2[1].0, r: thick * 0.5, v: 1.0 });
            }
        }
    } else if disp == 2 {
        for &(pa, pb) in &tops {
            segs.push(Seg { a: pb, b: pb, r: thick * 0.5, v: 1.0 });
            if side == 2 {
                segs.push(Seg { a: pa, b: pa, r: thick * 0.5, v: 1.0 });
            }
        }
    }
    Some(marks(ctx, segs, thick, soft))
}

fn waveform_plan(ctx: &EffectCtx, b: &Buf) -> Option<MarksPlan> {
    let pr = ctx.params;
    let samples = audio_window(ctx, pr.f("audioDuration"), pr.f("audioOffset"), pr.e("waveformOptions"))?;
    let count = pr.f("displayedSamples").round().clamp(2.0, 4096.0) as usize;
    let max_h = pr.f("maximumHeight") * b.scale;
    let thick = (pr.f("thickness") * b.scale).max(0.1);
    let soft = pr.f("softness") / 100.0;
    let disp = pr.e("displayOptions");
    let base = Baseline::new(ctx, b);
    let m = samples.len();
    let mut pts = Vec::with_capacity(count);
    let mut segs = Vec::new();
    for i in 0..count {
        let u = i as f64 / (count - 1) as f64;
        let a = ((u * (m - 1) as f64) as usize).min(m - 1);
        let bnd = (((i + 1) as f64 / count as f64 * (m - 1) as f64) as usize).clamp(a, m - 1);
        // Peak with sign of the window for a stable digital display.
        let (mut lo, mut hi) = (0.0f64, 0.0f64);
        for s in &samples[a..=bnd] {
            lo = lo.min(*s);
            hi = hi.max(*s);
        }
        let (q, nrm) = base.at(u);
        match disp {
            0 => segs.push(Seg {
                a: [q[0] + nrm[0] * lo * max_h, q[1] + nrm[1] * lo * max_h],
                b: [q[0] + nrm[0] * hi * max_h, q[1] + nrm[1] * hi * max_h],
                r: thick * 0.5,
                v: 1.0,
            }),
            _ => {
                let v = samples[a] * max_h;
                pts.push([q[0] + nrm[0] * v, q[1] + nrm[1] * v]);
            }
        }
    }
    if disp == 1 {
        poly_segs(&pts, false, thick * 0.5, 1.0, &mut segs);
    } else if disp == 2 {
        segs.extend(pts.iter().map(|&q| Seg { a: q, b: q, r: thick * 0.5, v: 1.0 }));
    }
    Some(marks(ctx, segs, thick, soft))
}

// ---------------------------------------------------------------- registry

pub fn specs() -> Vec<EffectSpec> {
    let ang = || ParamUi::Angle;
    let check = |id: &'static str, name: &'static str, v: bool| p(id, name, Value::Bool(v), ParamUi::Checkbox);
    let paint_style = || popup(&["On Original Image", "On Transparent", "Reveal Original Image"]);
    let mask_idx = |id: &'static str, name: &'static str, d: f64| p(id, name, num(d), ParamUi::Mask);
    let display = || popup(&["Digital", "Analog lines", "Analog dots"]);
    vec![
        spec(
            "ec.generate.fractal",
            "Fractal",
            vec![
                p("setChoice", "Set Choice", Value::Enum(0), popup(&FRACTAL_SETS)),
                p("equation", "Equation", Value::Enum(0), popup(&["z = z^2 + c", "z = z^3 + c", "z = z^4 + c"])),
                p("mandelbrot/mandelbrotX", "X (Real)", num(-0.74), slider(-2.0, 2.0, -2.0, 2.0, 4)),
                p("mandelbrot/mandelbrotY", "Y (Imaginary)", num(0.15), slider(-2.0, 2.0, -2.0, 2.0, 4)),
                p("mandelbrot/mandelbrotMagnification", "Magnification", num(0.0), slider(-5.0, 30.0, -5.0, 30.0, 2)),
                p("mandelbrot/mandelbrotEscapeLimit", "Escape Limit", num(4.0), slider(1.0, 1000.0, 1.0, 100.0, 1)),
                p("julia/juliaX", "X (Real)", num(0.0), slider(-2.0, 2.0, -2.0, 2.0, 4)),
                p("julia/juliaY", "Y (Imaginary)", num(0.0), slider(-2.0, 2.0, -2.0, 2.0, 4)),
                p("julia/juliaMagnification", "Magnification", num(0.0), slider(-5.0, 30.0, -5.0, 30.0, 2)),
                p("julia/juliaEscapeLimit", "Escape Limit", num(4.0), slider(1.0, 1000.0, 1.0, 100.0, 1)),
                p("postInversionOffset/postInversionX", "X Offset", num(0.0), slider(-2.0, 2.0, -2.0, 2.0, 4)),
                p("postInversionOffset/postInversionY", "Y Offset", num(0.0), slider(-2.0, 2.0, -2.0, 2.0, 4)),
                check("fractalColor/overlay", "Overlay", false),
                check("fractalColor/transparency", "Transparency", false),
                p("fractalColor/palette", "Palette", Value::Enum(0), popup(&FRACTAL_PALETTES)),
                p("fractalColor/hue", "Hue", num(0.0), ang()),
                p("fractalColor/cycleSteps", "Cycle Steps", num(32.0), slider(1.0, 1000.0, 1.0, 256.0, 0)),
                p("fractalColor/cycleOffset", "Cycle Offset", num(0.0), ang()),
                check("fractalColor/edgeHighlight", "Edge Highlight", false),
                p("highQualitySettings/samplingMethod", "Oversample Method", Value::Enum(0), popup(&FRACTAL_OVERSAMPLE)),
                p("highQualitySettings/samplingFactor", "Oversample Factor", num(2.0), slider(1.0, 9.0, 1.0, 9.0, 0)),
            ],
            fractal,
        ),
        spec(
            "ec.generate.scribble",
            "Scribble",
            vec![
                p("scribble", "Scribble", Value::Enum(0), popup(&["Single Mask", "All Masks", "All Masks Using Modes"])),
                mask_idx("mask", "Mask", 1.0),
                p("fillType", "Fill Type", Value::Enum(0), popup(&["Inside", "Centered Edge", "Inside Edge", "Outside Edge", "Left Edge", "Right Edge"])),
                p("edgeOptions/edgeWidth", "Edge Width", num(10.0), slider(0.0, 1000.0, 0.0, 100.0, 1)),
                p("edgeOptions/endCap", "End Cap", Value::Enum(0), popup(&END_CAPS)),
                p("edgeOptions/join", "Join", Value::Enum(0), popup(&JOINS)),
                p("edgeOptions/miterLimit", "Miter Limit", num(4.0), slider(1.0, 100.0, 1.0, 20.0, 1)),
                p("color", "Color", col(1.0, 0.0, 0.0), ParamUi::Color),
                p("opacity", "Opacity", num(100.0), pct()),
                p("angle", "Angle", num(45.0), ang()),
                p("strokeWidth", "Stroke Width", num(2.0), slider(0.1, 50.0, 0.1, 50.0, 1)),
                p("strokeOptions/curviness", "Curviness", num(50.0), pct()),
                p("strokeOptions/curvinessVariation", "Curviness Variation", num(50.0), pct()),
                p("strokeOptions/spacing", "Spacing", num(5.0), slider(0.1, 100.0, 0.1, 50.0, 1)),
                p("strokeOptions/spacingVariation", "Spacing Variation", num(50.0), pct()),
                p("strokeOptions/pathOverlap", "Path Overlap", num(0.0), slider(-100.0, 100.0, -100.0, 100.0, 1)),
                p("strokeOptions/pathOverlapVariation", "Path Overlap Variation", num(50.0), pct()),
                p("start", "Start", num(0.0), pct()),
                p("end", "End", num(100.0), pct()),
                p("startEndApplyTo", "Start/End Apply To", Value::Enum(0), popup(&["All Strokes", "Each Stroke"])),
                check("fillPathsSequentially", "Fill Paths Sequentially", true),
                p("wiggleType", "Wiggle Type", Value::Enum(1), popup(&["Static", "Jumpy", "Smooth"])),
                p("wigglesPerSecond", "Wiggles/Second", num(5.0), slider(0.0, 100.0, 0.0, 30.0, 2)),
                p("randomSeed", "Random Seed", num(1.0), slider(0.0, 10000.0, 0.0, 100.0, 0)),
                p("composite", "Composite", Value::Enum(1), popup(&["On Original Image", "On Transparent", "Reveal Original Image"])),
            ],
            scribble,
        ),
        spec(
            "ec.generate.stroke",
            "Stroke",
            vec![
                mask_idx("path", "Path", 1.0),
                check("allMasks", "All Masks", false),
                check("strokeSequentially", "Stroke Sequentially", true),
                p("color", "Color", col(1.0, 1.0, 1.0), ParamUi::Color),
                p("brushSize", "Brush Size", num(2.0), slider(0.0, 50.0, 0.0, 50.0, 1)),
                p("brushHardness", "Brush Hardness", num(75.0), pct()),
                p("opacity", "Opacity", num(100.0), pct()),
                p("start", "Start", num(0.0), pct()),
                p("end", "End", num(100.0), pct()),
                p("spacing", "Spacing", num(15.0), slider(0.0, 100.0, 0.0, 100.0, 1)),
                p("paintStyle", "Paint Style", Value::Enum(0), paint_style()),
            ],
            stroke,
        ),
        spec(
            "ec.generate.vegas",
            "Vegas",
            vec![
                p("stroke", "Stroke", Value::Enum(0), popup(&["Image Contours", "Mask/Path"])),
                p("inputLayer", "Image Contours: Input Layer", Value::Layer(None), ParamUi::Layer),
                check("invertInput", "Image Contours: Invert Input", false),
                p("ifLayerSizesDiffer", "Image Contours: If Layer Sizes Differ", Value::Enum(0), popup(&["Center", "Stretch to Fit"])),
                p("channel", "Image Contours: Channel", Value::Enum(0), popup(&["Intensity", "Red", "Green", "Blue", "Alpha"])),
                p("threshold", "Image Contours: Threshold", num(50.0), pct()),
                p("preBlur", "Image Contours: Pre-Blur", num(1.0), slider(0.0, 100.0, 0.0, 20.0, 1)),
                p("tolerance", "Image Contours: Tolerance", num(0.01), slider(0.0, 1.0, 0.0, 0.1, 3)),
                p("renderMode", "Image Contours: Render", Value::Enum(0), popup(&["All Contours", "Selected Contour"])),
                p("selectedContour", "Image Contours: Selected Contour", num(1.0), slider(1.0, 1000.0, 1.0, 20.0, 0)),
                p("shorterContoursHave", "Image Contours: Shorter Contours Have", Value::Enum(0), popup(&["Same Number of Segments", "Fewer Segments"])),
                mask_idx("path", "Mask/Path: Path", 1.0),
                p("segments", "Segments: Segments", num(32.0), slider(1.0, 1000.0, 1.0, 100.0, 0)),
                p("length", "Segments: Length", num(0.5), slider(0.0, 1.0, 0.0, 1.0, 3)),
                p("segmentDistribution", "Segments: Segment Distribution", Value::Enum(0), popup(&["Bunched", "Even"])),
                p("rotation", "Segments: Rotation", num(0.0), ang()),
                check("randomPhase", "Segments: Random Phase", false),
                p("randomSeed", "Segments: Random Seed", num(1.0), slider(1.0, 10000.0, 1.0, 100.0, 0)),
                p("blendMode", "Rendering: Blend Mode", Value::Enum(1), popup(&["Transparent", "Over", "Under", "Stencil"])),
                p("color", "Rendering: Color", col(1.0, 1.0, 1.0), ParamUi::Color),
                p("width", "Rendering: Width", num(2.0), slider(0.0, 100.0, 0.0, 20.0, 2)),
                p("hardness", "Rendering: Hardness", num(0.5), slider(0.0, 1.0, 0.0, 1.0, 3)),
                p("startOpacity", "Rendering: Start Opacity", num(1.0), slider(0.0, 1.0, 0.0, 1.0, 3)),
                p("midpointOpacity", "Rendering: Mid-point Opacity", num(0.5), slider(0.0, 1.0, 0.0, 1.0, 3)),
                p("midpointPosition", "Rendering: Mid-point Position", num(0.5), slider(0.0, 1.0, 0.0, 1.0, 3)),
                p("endOpacity", "Rendering: End Opacity", num(0.0), slider(0.0, 1.0, 0.0, 1.0, 3)),
            ],
            vegas,
        ),
        spec(
            "ec.generate.writeon",
            "Write-on",
            vec![
                p("brushPosition", "Brush Position", pt(0.5, 0.5), ParamUi::Point),
                p("color", "Color", col(1.0, 1.0, 1.0), ParamUi::Color),
                p("brushSize", "Brush Size", num(6.0), slider(0.0, 50.0, 0.0, 50.0, 1)),
                p("brushHardness", "Brush Hardness", num(75.0), pct()),
                p("brushOpacity", "Brush Opacity", num(100.0), pct()),
                p("strokeLength", "Stroke Length (secs)", num(0.0), slider(0.0, 30.0, 0.0, 30.0, 1)),
                p("brushSpacing", "Brush Spacing (secs)", num(0.001), slider(0.001, 5.0, 0.001, 5.0, 3)),
                p("paintTimeProperties", "Paint Time Properties", Value::Enum(0), popup(&["None", "Opacity", "Color"])),
                p("brushTimeProperties", "Brush Time Properties", Value::Enum(0), popup(&["None", "Size", "Hardness", "Size & Hardness"])),
                p("paintStyle", "Paint Style", Value::Enum(0), paint_style()),
            ],
            write_on,
        ),
        spec(
            "ec.generate.paintbucket",
            "Paint Bucket",
            vec![
                p("fillPoint", "Fill Point", pt(0.5, 0.5), ParamUi::Point),
                p("fillSelector", "Fill Selector", Value::Enum(0), popup(&["Color & Alpha", "Straight Color", "Transparency", "Opacity", "Alpha Channel"])),
                p("tolerance", "Tolerance", num(10.0), slider(0.0, 255.0, 0.0, 100.0, 1)),
                check("viewThreshold", "View Threshold", false),
                p("stroke", "Stroke", Value::Enum(0), popup(&["Antialias", "Feather", "Spread", "Choke", "Stroke"])),
                p("featherSoftness", "Feather Softness", num(10.0), slider(0.0, 100.0, 0.0, 50.0, 1)),
                p("spreadRadius", "Spread Radius", num(3.0), slider(0.0, 100.0, 0.0, 20.0, 1)),
                p("strokeWidth", "Stroke Width", num(2.0), slider(0.0, 100.0, 0.0, 20.0, 1)),
                p("color", "Color", col(1.0, 0.0, 0.0), ParamUi::Color),
                p("opacity", "Opacity", num(100.0), pct()),
                p(
                    "blendingMode",
                    "Blending Mode",
                    Value::Enum(0),
                    popup(&["Normal", "Add", "Multiply", "Screen", "Hue", "Saturation", "Color", "Luminosity", "Behind", "Fill Only"]),
                ),
            ],
            paint_bucket,
        ),
        spec(
            "ec.generate.eyedropperfill",
            "Eyedropper Fill",
            vec![
                p("samplePoint", "Sample Point", pt(0.5, 0.5), ParamUi::Point),
                p("sampleRadius", "Sample Radius", num(0.0), slider(0.0, 1000.0, 0.0, 100.0, 1)),
                p("averagePixelColors", "Average Pixel Colors", Value::Enum(0), popup(&["Skip Empty", "All", "All Premultiplied", "Including Alpha"])),
                check("maintainOriginalAlpha", "Maintain Original Alpha", false),
                p("blendWithOriginal", "Blend With Original", num(0.0), pct()),
            ],
            eyedropper_fill,
        ),
        spec(
            "ec.generate.ccgluegun",
            "CC Glue Gun",
            vec![
                p("brushPosition", "Brush Position", pt(0.5, 0.5), ParamUi::Point),
                p("strokeWidth", "Stroke Width", num(25.0), slider(0.0, 500.0, 0.0, 100.0, 1)),
                p("density", "Density", num(5.0), slider(0.0, 32.0, 0.0, 32.0, 1)),
                p("timeSpan", "Time Span (sec)", num(1.0), slider(0.0, 60.0, 0.0, 10.0, 2)),
                p("reflection", "Reflection", num(45.0), ang()),
                p("strength", "Strength", num(100.0), pct()),
                p("style", "Style", Value::Enum(0), popup(&["Over Layer", "Glue Only"])),
            ],
            glue_gun,
        ),
        spec(
            "ec.generate.ccthreads",
            "CC Threads",
            vec![
                p("width", "Width", num(50.0), slider(1.0, 1000.0, 1.0, 200.0, 1)),
                p("height", "Height", num(50.0), slider(1.0, 1000.0, 1.0, 200.0, 1)),
                p("overlaps", "Overlaps", num(1.0), slider(1.0, 20.0, 1.0, 10.0, 0)),
                p("direction", "Direction", num(0.0), ang()),
                p("center", "Center", pt(0.5, 0.5), ParamUi::Point),
                p("coverage", "Coverage", num(80.0), pct()),
                p("shadowing", "Shadowing", num(50.0), pct()),
                p("texture", "Texture", num(10.0), pct()),
            ],
            threads,
        ),
        spec(
            "ec.generate.audiospectrum",
            "Audio Spectrum",
            vec![
                p("audioLayer", "Audio Layer", Value::Layer(None), ParamUi::Layer),
                p("startPoint", "Start Point", pt(0.1, 0.5), ParamUi::Point),
                p("endPoint", "End Point", pt(0.9, 0.5), ParamUi::Point),
                mask_idx("path", "Path", 0.0),
                check("usePolarPath", "Use Polar Path", false),
                p("startFrequency", "Start Frequency", num(20.0), slider(1.0, 20000.0, 1.0, 2000.0, 0)),
                p("endFrequency", "End Frequency", num(2000.0), slider(1.0, 20000.0, 1.0, 10000.0, 0)),
                p("frequencyBands", "Frequency bands", num(64.0), slider(1.0, 4096.0, 1.0, 256.0, 0)),
                p("maximumHeight", "Maximum Height", num(50.0), slider(0.0, 10000.0, 0.0, 500.0, 1)),
                p("audioDuration", "Audio Duration (milliseconds)", num(90.0), slider(1.0, 30000.0, 1.0, 1000.0, 1)),
                p("audioOffset", "Audio Offset (milliseconds)", num(0.0), slider(-30000.0, 30000.0, -1000.0, 1000.0, 1)),
                p("thickness", "Thickness", num(3.0), slider(0.0, 1000.0, 0.0, 50.0, 2)),
                p("softness", "Softness", num(50.0), pct()),
                p("insideColor", "Inside Color", col(1.0, 1.0, 1.0), ParamUi::Color),
                p("outsideColor", "Outside Color", col(0.2, 0.4, 1.0), ParamUi::Color),
                check("blendOverlappingColors", "Blend Overlapping Colors", true),
                p("hueInterpolation", "Hue Interpolation", num(0.0), ang()),
                check("dynamicHuePhase", "Dynamic Hue Phase", false),
                check("colorSymmetry", "Color Symmetry", false),
                p("displayOptions", "Display Options", Value::Enum(0), display()),
                p("sideOptions", "Side Options", Value::Enum(0), popup(&["Side A", "Side B", "Side A & B"])),
                check("durationAveraging", "Duration Averaging", false),
                check("compositeOnOriginal", "Composite On Original", false),
            ],
            audio_spectrum,
        ),
        spec(
            "ec.generate.audiowaveform",
            "Audio Waveform",
            vec![
                p("audioLayer", "Audio Layer", Value::Layer(None), ParamUi::Layer),
                p("startPoint", "Start Point", pt(0.1, 0.5), ParamUi::Point),
                p("endPoint", "End Point", pt(0.9, 0.5), ParamUi::Point),
                mask_idx("path", "Path", 0.0),
                check("usePolarPath", "Use Polar Path", false),
                p("displayedSamples", "Displayed Samples", num(200.0), slider(2.0, 4096.0, 2.0, 1000.0, 0)),
                p("maximumHeight", "Maximum Height", num(100.0), slider(0.0, 10000.0, 0.0, 500.0, 1)),
                p("audioDuration", "Audio Duration (milliseconds)", num(200.0), slider(1.0, 30000.0, 1.0, 1000.0, 1)),
                p("audioOffset", "Audio Offset (milliseconds)", num(0.0), slider(-30000.0, 30000.0, -1000.0, 1000.0, 1)),
                p("thickness", "Thickness", num(2.0), slider(0.0, 1000.0, 0.0, 50.0, 2)),
                p("softness", "Softness", num(10.0), pct()),
                p("randomSeed", "Random Seed (Analog)", num(1.0), slider(1.0, 10000.0, 1.0, 100.0, 0)),
                p("insideColor", "Inside Color", col(1.0, 1.0, 1.0), ParamUi::Color),
                p("outsideColor", "Outside Color", col(0.2, 0.4, 1.0), ParamUi::Color),
                p("waveformOptions", "Waveform Options", Value::Enum(0), popup(&["Mono", "Left", "Right"])),
                p("displayOptions", "Display Options", Value::Enum(1), display()),
                check("compositeOnOriginal", "Composite On Original", false),
            ],
            audio_waveform,
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EffectEnv, EffectHost, LayerPixels, MaskShape, Params};

    /// Run with point defaults scaled to the layer size.
    pub(crate) fn run_env(id: &str, set: &[(&str, Value)], img: Image, time: f64, env: EffectEnv) -> Buf {
        let s = crate::find(id).unwrap();
        let ls = [img.width as f64, img.height as f64];
        let mut params = Params { values: s.params.iter().map(|p| (p.id.to_string(), p.default.clone())).collect() };
        for p in &s.params {
            if let (ParamUi::Point, Value::Vec2(v)) = (&p.ui, &p.default) {
                params.values.insert(p.id.to_string(), Value::Vec2([v[0] * ls[0], v[1] * ls[1]]));
            }
        }
        for (k, v) in set {
            assert!(params.values.contains_key(*k), "{id}: unknown {k}");
            params.values.insert(k.to_string(), v.clone());
        }
        let ctx = EffectCtx { params: &params, time, layer_size: ls, seed: 3, adjustment: false, env };
        crate::apply(s, &ctx, Buf { img, offset: [0.0, 0.0], scale: 1.0 })
    }

    fn run(id: &str, set: &[(&str, Value)], img: Image) -> Buf {
        run_env(id, set, img, 0.5, EffectEnv::default())
    }

    fn square_mask() -> Vec<MaskShape> {
        vec![MaskShape { name: "Mask 1".into(), points: vec![[16.0, 16.0], [48.0, 16.0], [48.0, 48.0], [16.0, 48.0]], closed: true, inverted: false }]
    }

    fn alpha_sum(img: &Image, x0: usize, y0: usize, x1: usize, y1: usize) -> f32 {
        let mut s = 0.0;
        for y in y0..y1 {
            for x in x0..x1 {
                s += img.data[y * img.width as usize + x][3];
            }
        }
        s
    }

    #[test]
    fn audio_spectrum_linear_frequency_and_linear_amplitude() {
        let host = Tone; // Tone produces 440 Hz
        let env = EffectEnv { host: Some(&host), comp_time: 1.0, ..Default::default() };
        let img = Image::new(200, 100);
        let out = run_env(
            "ec.generate.audiospectrum",
            &[
                ("audioLayer", Value::Layer(Some(1))),
                ("startPoint", Value::Vec2([0.0, 50.0])),
                ("endPoint", Value::Vec2([200.0, 50.0])),
                ("startFrequency", num(20.0)),
                ("endFrequency", num(2000.0)),
                ("frequencyBands", num(100.0)),
                ("maximumHeight", num(40.0)),
                ("thickness", num(2.0)),
                ("softness", num(0.0)),
            ],
            img,
            1.0,
            env,
        );
        // Verify 440 Hz tone produces a peak above baseline, while high frequencies have zero bar height
        let peak_zone = alpha_sum(&out.img, 36, 10, 48, 48);
        let high_zone = alpha_sum(&out.img, 120, 10, 150, 48);
        let silent_zone = alpha_sum(&out.img, 170, 10, 195, 48);
        assert!(peak_zone > 5.0, "peak zone has energy: {peak_zone}");
        assert!(peak_zone > high_zone * 2.0, "linear peak is distinctly taller than far frequencies: {peak_zone} vs {high_zone}");
        assert_eq!(silent_zone, 0.0, "silent end has zero height, not boosted by dB scale");
    }

    #[test]
    fn fractal_deterministic_and_has_inside_set() {
        let img = Image::new(64, 48);
        let a = run("ec.generate.fractal", &[], img.clone());
        let b = run("ec.generate.fractal", &[], img.clone());
        assert_eq!(a.img.data, b.img.data);
        // The default view contains the main cardioid (black, opaque) and escaped (coloured) points.
        assert!(a.img.data.iter().any(|p| p[3] > 0.99 && p[0] + p[1] + p[2] < 0.01));
        assert!(a.img.data.iter().any(|p| p[0] + p[1] + p[2] > 0.3));
        let t = run("ec.generate.fractal", &[("fractalColor/transparency", Value::Bool(true))], img);
        assert!(t.img.data.iter().any(|p| p[3] == 0.0));
    }

    #[test]
    fn fractal_palettes_and_set_choices() {
        let img = Image::new(64, 48);
        // Solid Color: only the inside of the set is drawn; Transparency flips it.
        let solid = run("ec.generate.fractal", &[("fractalColor/palette", Value::Enum(3))], img.clone());
        assert!(solid.img.data.iter().any(|p| p[3] == 0.0) && solid.img.data.iter().any(|p| p[3] > 0.99 && p[0] > 0.9));
        let flipped = run("ec.generate.fractal", &[("fractalColor/palette", Value::Enum(3)), ("fractalColor/transparency", Value::Bool(true))], img.clone());
        for (a, b) in solid.img.data.iter().zip(&flipped.img.data) {
            assert!((a[3] > 0.5) != (b[3] > 0.5) || (a[3] - 0.5).abs() < 0.49);
        }
        // Black And White: escaped pixels are pure black or white (away from supersampled edges).
        let bw = run("ec.generate.fractal", &[("fractalColor/palette", Value::Enum(2)), ("highQualitySettings/samplingFactor", num(1.0))], img.clone());
        assert!(bw.img.data.iter().all(|p| p[0] == p[1] && p[1] == p[2] && (p[0] == 0.0 || p[0] == 1.0)));
        // Mandelbrot Over Julia depends on the Julia centre.
        let o1 = run("ec.generate.fractal", &[("setChoice", Value::Enum(2)), ("julia/juliaX", num(0.3))], img.clone());
        let o2 = run("ec.generate.fractal", &[("setChoice", Value::Enum(2)), ("julia/juliaX", num(-0.2))], img.clone());
        assert_ne!(o1.img.data, o2.img.data);
        let plain = run("ec.generate.fractal", &[("julia/juliaX", num(0.3))], img);
        assert_ne!(o1.img.data, plain.img.data);
    }

    #[test]
    fn scribble_fills_mask_and_animates() {
        let masks = square_mask();
        let env = EffectEnv { masks: &masks, ..Default::default() };
        let img = Image::filled(64, 64, [0.0, 0.0, 1.0, 1.0]);
        let a = run_env("ec.generate.scribble", &[], img.clone(), 0.5, env);
        let b = run_env("ec.generate.scribble", &[], img.clone(), 0.5, env);
        assert_eq!(a.img.data, b.img.data);
        let inside = alpha_sum(&a.img, 18, 18, 46, 46);
        let outside = alpha_sum(&a.img, 0, 0, 10, 64);
        assert!(inside > 100.0 && outside < 1.0, "{inside} {outside}");
        let c = run_env("ec.generate.scribble", &[], img.clone(), 1.5, env);
        assert_ne!(a.img.data, c.img.data, "jumpy wiggle changes over time");
        let s = run_env("ec.generate.scribble", &[("wiggleType", Value::Enum(0))], img.clone(), 0.5, env);
        let s2 = run_env("ec.generate.scribble", &[("wiggleType", Value::Enum(0))], img.clone(), 3.5, env);
        assert_eq!(s.img.data, s2.img.data, "static wiggle is constant");
        let half = run_env("ec.generate.scribble", &[("end", num(50.0))], img, 0.5, env);
        assert!(alpha_sum(&half.img, 0, 0, 64, 64) < alpha_sum(&a.img, 0, 0, 64, 64));
    }

    #[test]
    fn scribble_edge_types() {
        let masks = square_mask();
        let env = EffectEnv { masks: &masks, ..Default::default() };
        let img = Image::new(64, 64);
        let o = run_env("ec.generate.scribble", &[("fillType", Value::Enum(3)), ("edgeOptions/edgeWidth", num(6.0))], img, 0.5, env);
        // Outside edge: paint near the border outside, nothing in the centre.
        assert!(alpha_sum(&o.img, 26, 26, 38, 38) < 0.5);
        assert!(alpha_sum(&o.img, 8, 8, 16, 56) > 5.0);
    }

    #[test]
    fn stroke_draws_along_mask() {
        let masks = square_mask();
        let env = EffectEnv { masks: &masks, ..Default::default() };
        let img = Image::new(64, 64);
        let a = run_env("ec.generate.stroke", &[("brushSize", num(4.0)), ("paintStyle", Value::Enum(1))], img.clone(), 0.0, env);
        assert!(a.img.get(32, 16)[3] > 0.9, "on the top edge");
        assert!(a.img.get(32, 32)[3] < 0.01, "centre empty");
        let b = run_env("ec.generate.stroke", &[("brushSize", num(4.0)), ("end", num(25.0))], img.clone(), 0.0, env);
        assert!(b.img.get(32, 16)[3] > 0.9 && b.img.get(48, 32)[3] < 0.01, "first quarter only");
        let d = run_env("ec.generate.stroke", &[("brushSize", num(4.0)), ("spacing", num(300.0))], img, 0.0, env);
        assert!(alpha_sum(&d.img, 0, 0, 64, 64) < alpha_sum(&a.img, 0, 0, 64, 64), "spaced dabs");
        // No masks: unchanged.
        let src = Image::filled(8, 8, [0.2, 0.3, 0.4, 1.0]);
        assert_eq!(run("ec.generate.stroke", &[], src.clone()).img.data, src.data);
    }

    #[test]
    fn vegas_traces_contours() {
        let mut img = Image::new(64, 64);
        for y in 16..48 {
            for x in 16..48 {
                img.set(x, y, [1.0, 1.0, 1.0, 1.0]);
            }
        }
        let a = run(
            "ec.generate.vegas",
            &[("blendMode", Value::Enum(0)), ("length", num(1.0)), ("midpointOpacity", num(1.0)), ("endOpacity", num(1.0))],
            img.clone(),
        );
        let b = run(
            "ec.generate.vegas",
            &[("blendMode", Value::Enum(0)), ("length", num(1.0)), ("midpointOpacity", num(1.0)), ("endOpacity", num(1.0))],
            img.clone(),
        );
        assert_eq!(a.img.data, b.img.data);
        assert!(alpha_sum(&a.img, 14, 14, 50, 50) > 50.0, "edges drawn");
        assert!(alpha_sum(&a.img, 26, 26, 38, 38) < 0.5, "interior untouched");
        let r = run("ec.generate.vegas", &[("blendMode", Value::Enum(0)), ("rotation", num(90.0))], img);
        assert_ne!(r.img.data, a.img.data);
    }

    #[test]
    fn contours_of_square_are_closed() {
        let mut pl = Plane::new(20, 20);
        for y in 5..15 {
            for x in 5..15 {
                pl.data[y * 20 + x] = 1.0;
            }
        }
        let c = contours(&pl, 0.5);
        assert_eq!(c.len(), 1);
        assert!(c[0].1);
        let l = poly_length(&c[0].0, true);
        assert!((l - 40.0).abs() < 4.0, "{l}");
    }

    #[test]
    fn write_on_and_glue_gun_draw_at_brush() {
        let img = Image::new(32, 32);
        let a = run("ec.generate.writeon", &[], img.clone());
        assert!(a.img.get(16, 16)[3] > 0.9 && a.img.get(2, 2)[3] == 0.0);
        assert_eq!(a.img.data, run("ec.generate.writeon", &[], img.clone()).img.data);
        let src = Image::filled(32, 32, [0.5, 0.2, 0.1, 1.0]);
        let g = run("ec.generate.ccgluegun", &[], src.clone());
        assert_eq!(g.img.data, run("ec.generate.ccgluegun", &[], src.clone()).img.data);
        assert_ne!(g.img.get(16, 16), src.get(16, 16));
        assert_eq!(g.img.get(0, 0), src.get(0, 0));
    }

    /// #536: an animated Brush Position draws a stroke that stays (Stroke Length 0), expires
    /// after Stroke Length, and lays its dabs Brush Spacing seconds apart.
    #[test]
    fn write_on_accumulates_a_stroke_along_the_animated_brush() {
        // Brush Position from [8, 16] at 0 s to [56, 16] at 1 s; red, hard, 6 px.
        struct Brush(Vec<(&'static str, Value)>);
        impl Brush {
            fn at(&self, t: f64) -> Vec<(&'static str, Value)> {
                let mut v = self.0.clone();
                v.push(("brushPosition", Value::Vec2([8.0 + 48.0 * t.clamp(0.0, 1.0), 16.0])));
                v
            }
        }
        impl EffectHost for Brush {
            fn layer(&self, _: u64, _: bool) -> Option<LayerPixels> {
                None
            }
            fn audio(&self, _: u64, _: f64, _: usize, _: u32) -> Option<Vec<f32>> {
                None
            }
            fn params_at(&self, t: f64) -> Option<Params> {
                let s = crate::find("ec.generate.writeon").unwrap();
                let mut p = Params { values: s.params.iter().map(|p| (p.id.to_string(), p.default.clone())).collect() };
                for (k, v) in self.at(t) {
                    p.values.insert(k.to_string(), v);
                }
                Some(p)
            }
        }
        let render = |extra: &[(&'static str, Value)], t: f64| {
            let mut set = vec![("color", col(1.0, 0.0, 0.0)), ("brushSize", num(6.0)), ("brushHardness", num(100.0)), ("brushSpacing", num(0.05))];
            set.extend(extra.iter().cloned());
            let host = Brush(set);
            let env = EffectEnv { host: Some(&host), ..Default::default() };
            let at = host.at(t);
            run_env("ec.generate.writeon", &at, Image::new(64, 32), t, env).img
        };
        let on = |img: &Image, x: i64| img.get(x, 16)[3] > 0.9;
        let half = render(&[], 0.5);
        assert!(on(&half, 8) && on(&half, 20) && on(&half, 32) && !on(&half, 44) && !on(&half, 56));
        let end = render(&[], 1.0);
        assert!([8, 20, 32, 44, 56].iter().all(|x| on(&end, *x)));
        // Stroke Length 0.25 s: only the last quarter second's trail.
        let trail = render(&[("strokeLength", num(0.25))], 1.0);
        assert!(!on(&trail, 8) && !on(&trail, 32) && on(&trail, 48) && on(&trail, 56));
        // Brush Spacing 0.5 s: three separate marks.
        let spaced = render(&[("brushSpacing", num(0.5))], 1.0);
        assert!(on(&spaced, 8) && on(&spaced, 32) && on(&spaced, 56) && !on(&spaced, 20) && !on(&spaced, 44));
        // Paint Time Properties ▸ Opacity: each dab keeps the opacity it was laid with.
        struct Fade;
        impl EffectHost for Fade {
            fn layer(&self, _: u64, _: bool) -> Option<LayerPixels> {
                None
            }
            fn audio(&self, _: u64, _: f64, _: usize, _: u32) -> Option<Vec<f32>> {
                None
            }
            fn params_at(&self, t: f64) -> Option<Params> {
                let mut p = Brush(vec![("brushHardness", num(100.0)), ("brushSpacing", num(0.05)), ("paintTimeProperties", Value::Enum(1))]).params_at(t)?;
                p.values.insert("brushOpacity".into(), num(if t < 0.5 { 100.0 } else { 20.0 }));
                Some(p)
            }
        }
        let p1 = Fade.params_at(1.0).unwrap();
        let set: Vec<(&str, Value)> = p1.values.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
        let faded = run_env("ec.generate.writeon", &set, Image::new(64, 32), 1.0, EffectEnv { host: Some(&Fade), ..Default::default() }).img;
        assert!(faded.get(8, 16)[3] > 0.9 && (faded.get(56, 16)[3] - 0.2).abs() < 0.05, "{:?} {:?}", faded.get(8, 16), faded.get(56, 16));
    }

    #[test]
    fn paint_bucket_fills_enclosed_region_only() {
        let mut img = Image::filled(32, 32, [1.0, 1.0, 1.0, 1.0]);
        for i in 8..24 {
            for &(x, y) in &[(i, 8), (i, 23), (8, i), (23, i)] {
                img.set(x, y, [0.0, 0.0, 0.0, 1.0]);
            }
        }
        let out = run("ec.generate.paintbucket", &[], img.clone());
        let inside = out.img.get(16, 16);
        assert!(inside[0] > 0.99 && inside[1] < 0.01, "{inside:?}");
        assert_eq!(out.img.get(2, 2), [1.0, 1.0, 1.0, 1.0], "outside untouched");
        assert_eq!(out.img.get(8, 16), [0.0, 0.0, 0.0, 1.0], "border untouched");
        assert_eq!(out.img.data, run("ec.generate.paintbucket", &[], img).img.data);
    }

    #[test]
    fn eyedropper_fill_uses_sample() {
        let mut img = Image::filled(16, 16, [0.0, 0.0, 1.0, 1.0]);
        img.set(8, 8, [1.0, 0.0, 0.0, 1.0]);
        let out = run("ec.generate.eyedropperfill", &[], img.clone());
        assert!(out.img.data.iter().all(|p| *p == [1.0, 0.0, 0.0, 1.0]));
        let blended = run("ec.generate.eyedropperfill", &[("blendWithOriginal", num(100.0))], img.clone());
        assert_eq!(blended.img.data, img.data);
    }

    #[test]
    fn threads_weave_is_deterministic_and_gappy() {
        let src = Image::filled(64, 64, [0.4, 0.6, 0.2, 1.0]);
        let a = run("ec.generate.ccthreads", &[("width", num(16.0)), ("height", num(16.0)), ("coverage", num(50.0))], src.clone());
        assert_eq!(a.img.data, run("ec.generate.ccthreads", &[("width", num(16.0)), ("height", num(16.0)), ("coverage", num(50.0))], src).img.data);
        assert!(a.img.data.iter().any(|p| p[3] == 0.0), "gaps between threads");
        assert!(a.img.data.iter().any(|p| p[3] > 0.99), "threads");
    }

    struct Tone;
    impl EffectHost for Tone {
        fn layer(&self, _: u64, _: bool) -> Option<LayerPixels> {
            None
        }
        fn audio(&self, _: u64, start: f64, frames: usize, rate: u32) -> Option<Vec<f32>> {
            Some(
                (0..frames)
                    .flat_map(|i| {
                        let t = start + i as f64 / rate as f64;
                        let v = (2.0 * PI * 440.0 * t).sin() as f32 * 0.8;
                        [v, v]
                    })
                    .collect(),
            )
        }
    }

    #[test]
    fn audio_effects_without_audio_pass_through() {
        let src = Image::filled(16, 16, [0.3, 0.3, 0.3, 1.0]);
        for id in ["ec.generate.audiospectrum", "ec.generate.audiowaveform"] {
            assert_eq!(run(id, &[], src.clone()).img.data, src.data, "{id}");
        }
    }

    #[test]
    fn audio_effects_draw_with_host() {
        let host = Tone;
        let env = EffectEnv { host: Some(&host), comp_time: 1.0, ..Default::default() };
        let img = Image::new(96, 64);
        for id in ["ec.generate.audiospectrum", "ec.generate.audiowaveform"] {
            let a = run_env(id, &[("audioLayer", Value::Layer(Some(1)))], img.clone(), 1.0, env);
            let b = run_env(id, &[("audioLayer", Value::Layer(Some(1)))], img.clone(), 1.0, env);
            assert_eq!(a.img.data, b.img.data, "{id}");
            assert!(alpha_sum(&a.img, 0, 0, 96, 64) > 10.0, "{id} drew nothing");
        }
    }

    #[test]
    fn fft_finds_tone() {
        let n = 256;
        let mut re: Vec<f64> = (0..n).map(|i| (2.0 * PI * 8.0 * i as f64 / n as f64).cos()).collect();
        let mut im = vec![0.0; n];
        fft(&mut re, &mut im);
        let mags: Vec<f64> = (0..n / 2).map(|k| (re[k] * re[k] + im[k] * im[k]).sqrt()).collect();
        let peak = mags.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).unwrap().0;
        assert_eq!(peak, 8);
    }

    #[test]
    fn stroke_outline_caps_and_joins() {
        let line = vec![vec![[10.0, 10.0], [30.0, 10.0]]];
        let cov = |cap: u32| raster_convex(40, 20, &stroke_outline(&line, 3.0, cap, 0, 4.0));
        // Round caps reach 3 px past the ends (on the axis), Butt stops there, Projecting is square.
        let (round, butt, proj) = (cov(0), cov(1), cov(2));
        assert!(round.get(8, 10) > 0.9 && butt.get(8, 10) < 0.1 && proj.get(8, 10) > 0.9);
        assert!(proj.get(7, 7) > 0.9 && round.get(7, 7) < 0.5, "square corner");
        // A right-angle turn: Miter fills the outer corner, Bevel cuts it.
        let corner = vec![vec![[5.0, 30.0], [30.0, 30.0], [30.0, 5.0]]];
        let j = |join: u32| raster_convex(40, 40, &stroke_outline(&corner, 3.0, 1, join, 4.0));
        let (miter, bevel) = (j(1), j(2));
        assert!(miter.get(32, 32) > 0.9 && bevel.get(32, 32) < 0.1, "{} {}", miter.get(32, 32), bevel.get(32, 32));
        // A tight miter limit falls back to a bevel.
        let limited = raster_convex(40, 40, &stroke_outline(&corner, 3.0, 1, 1, 1.0));
        assert!(limited.get(32, 32) < 0.1);
    }

    #[test]
    fn scribble_caps_and_start_end_apply_to() {
        let masks = square_mask();
        let env = EffectEnv { masks: &masks, ..Default::default() };
        let img = Image::new(64, 64);
        let base = [("wiggleType", Value::Enum(0)), ("strokeWidth", num(3.0))];
        let round = run_env("ec.generate.scribble", &base, img.clone(), 0.0, env);
        let butt = run_env(
            "ec.generate.scribble",
            &[base.as_slice(), &[("edgeOptions/endCap", Value::Enum(1)), ("edgeOptions/join", Value::Enum(2))]].concat(),
            img.clone(),
            0.0,
            env,
        );
        assert_ne!(round.img.data, butt.img.data);
        // Each Stroke: every stroke is trimmed on its own, so the half-way image has ink all over.
        let two = vec![
            crate::MaskShape { name: String::new(), points: vec![[4.0, 4.0], [28.0, 4.0], [28.0, 60.0], [4.0, 60.0]], closed: true, inverted: false },
            crate::MaskShape { name: String::new(), points: vec![[36.0, 4.0], [60.0, 4.0], [60.0, 60.0], [36.0, 60.0]], closed: true, inverted: false },
        ];
        let env2 = EffectEnv { masks: &two, ..Default::default() };
        let seq = [("scribble", Value::Enum(1)), ("end", num(50.0))];
        let all = run_env("ec.generate.scribble", &[base.as_slice(), &seq].concat(), img.clone(), 0.0, env2);
        let each = run_env("ec.generate.scribble", &[base.as_slice(), &seq, &[("startEndApplyTo", Value::Enum(1))]].concat(), img, 0.0, env2);
        assert_ne!(all.img.data, each.img.data);
    }
}

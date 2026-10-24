//! Blur & Sharpen, Stylize, Perspective, Channel, Noise and Transition effects.

use effectcraft_color::luminance;
use effectcraft_keyframe::Value;
use effectcraft_project::ParamUi;
use effectcraft_raster::{Image, gaussian_blur, hash_noise};
use rayon::prelude::*;

use crate::{Buf, EffectCtx, EffectSpec, col, num, p, popup, slider};

fn spec(id: &'static str, name: &'static str, category: &'static str, params: Vec<crate::ParamSpec>, render: crate::RenderFn) -> EffectSpec {
    EffectSpec { id, name, category, params, render, gpu: false, float: true }
}

/// AE's Blurriness is roughly a radius; our Gaussian uses sigma = blurriness / 2.
fn blur_sigma(blurriness: f64) -> f64 {
    blurriness.max(0.0) * 0.5
}

fn dims_xy(dim: u32) -> (f64, f64) {
    match dim {
        1 => (1.0, 0.0),
        2 => (0.0, 1.0),
        _ => (1.0, 1.0),
    }
}

fn gaussian(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let s = blur_sigma(ctx.params.f("blurriness")) * b.scale;
    if s <= 0.0 {
        return b;
    }
    let (kx, ky) = dims_xy(ctx.params.e("dimensions"));
    let repeat = ctx.params.b("repeatEdge");
    if !repeat && !ctx.adjustment {
        b.pad((s * 3.0).ceil() as u32);
    }
    let (sx, sy) = (s * kx, s * ky);
    // Small sigmas: integer-radius box passes would step (and do nothing below ~0.6 px), so sample the kernel (#1939).
    b.img = if effectcraft_raster::gaussian_is_exact(sx, sy) {
        effectcraft_raster::gaussian_blur_exact(&b.img, sx, sy, repeat || ctx.adjustment)
    } else {
        gaussian_blur(&b.img, sx, sy, repeat || ctx.adjustment)
    };
    b
}

fn box_blur(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let radius = (ctx.params.f("radius") * b.scale).max(0.0);
    let r = radius.floor() as usize;
    let frac = (radius - r as f64) as f32;
    // Fractional radius: blend the two neighbouring integer radii so the result changes continuously (#1939).
    let frac = if frac < 1e-4 { 0.0 } else { frac };
    if r == 0 && frac == 0.0 {
        return b;
    }
    let it = ctx.params.f("iterations").round().clamp(1.0, 50.0) as usize;
    let (kx, ky) = dims_xy(ctx.params.e("dimensions"));
    let repeat = ctx.params.b("repeatEdge");
    if !repeat && !ctx.adjustment {
        let reach = if frac > 0.0 { r + 1 } else { r };
        b.pad((reach * it) as u32 + 1);
    }
    let edge = repeat || ctx.adjustment;
    let lo = effectcraft_raster::box_blur(&b.img, r * kx as usize, r * ky as usize, it, edge);
    if frac == 0.0 {
        b.img = lo;
        return b;
    }
    let hi = effectcraft_raster::box_blur(&b.img, (r + 1) * kx as usize, (r + 1) * ky as usize, it, edge);
    for ((l, h), o) in lo.data.iter().zip(hi.data.iter()).zip(b.img.data.iter_mut()) {
        for c in 0..4 {
            o[c] = l[c] + (h[c] - l[c]) * frac;
        }
    }
    b
}

fn directional(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let len = ctx.params.f("length") * b.scale;
    if len < 0.5 {
        return b;
    }
    if !ctx.adjustment {
        b.pad(len.ceil() as u32 + 1);
    }
    b.img = effectcraft_raster::directional_blur(&b.img, ctx.params.f("direction"), len);
    b
}

fn radial(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let amt = ctx.params.f("amount");
    let zoom = ctx.params.e("type") == 1;
    let c = b.to_px(ctx.params.v2("center"));
    b.img = effectcraft_raster::radial_blur(&b.img, c, if zoom { amt / 100.0 } else { amt }, zoom);
    b
}

fn unsharp_core(img: &Image, amount: f32, sigma: f64, threshold: f32) -> Image {
    let blurred = gaussian_blur(img, sigma, sigma, true);
    let mut out = img.clone();
    out.data.par_iter_mut().zip(blurred.data.par_iter()).for_each(|(o, bl)| {
        for c in 0..3 {
            let d = o[c] - bl[c];
            if d.abs() * 255.0 >= threshold {
                o[c] = (o[c] + d * amount).max(0.0);
            }
        }
    });
    out
}

fn sharpen(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let a = ctx.params.f("amount") as f32 / 50.0;
    b.img = unsharp_core(&b.img, a, 1.0 * b.scale, 0.0);
    b
}

fn unsharp(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let a = ctx.params.f("amount") as f32 / 100.0;
    b.img = unsharp_core(&b.img, a, (ctx.params.f("radius") * b.scale).max(0.1), ctx.params.f("threshold") as f32);
    b
}

/// Position (0 = Color A, 1 = Color B) in Glow's A & B gradient for brightness `l`: `loops`
/// cycles starting at `phase` (revolutions), shaped by Color Looping (0 Sawtooth A>B,
/// 1 Sawtooth B>A, 2 Triangle A>B>A, 3 Triangle B>A>B) and biased so `mid` maps to the middle.
pub fn glow_ab_t(l: f32, looping: u32, loops: f32, phase: f32, mid: f32) -> f32 {
    let u = l.clamp(0.0, 1.0) * loops.max(0.0) + phase;
    let mut t = u - u.floor();
    if u > 0.0 && t <= 0.0 {
        t = 1.0;
    }
    let tri = 1.0 - (2.0 * t - 1.0).abs();
    let t = match looping {
        0 => t,
        1 => 1.0 - t,
        3 => 1.0 - tri,
        _ => tri,
    };
    let m = mid.clamp(0.01, 0.99);
    t.max(0.0).powf(0.5f32.ln() / m.ln())
}

/// Glow Operation options (blend modes the glow is applied with).
pub const GLOW_OPERATIONS: [&str; 17] = [
    "Normal",
    "Add",
    "Multiply",
    "Screen",
    "Overlay",
    "Soft Light",
    "Hard Light",
    "Color Dodge",
    "Color Burn",
    "Darken",
    "Lighten",
    "Difference",
    "Exclusion",
    "Hue",
    "Saturation",
    "Color",
    "Luminosity",
];

/// Glow's Glow Operation as a blend mode (Add for projects saved before it existed).
pub fn glow_operation(ctx: &EffectCtx) -> effectcraft_color::BlendMode {
    let name = GLOW_OPERATIONS.get(ctx.params.get("glowOperation").map(|v| v.as_enum()).unwrap_or(1) as usize).copied().unwrap_or("Add");
    effectcraft_color::BlendMode::ALL.iter().copied().find(|m| m.label() == name).unwrap_or(effectcraft_color::BlendMode::Add)
}

/// Glow's Arbitrary Map: red, green and blue curves of the glow brightness, as Curves point
/// lists `x,y …` separated by `|` (an empty or missing curve is the identity).
fn glow_map(s: &str) -> Vec<Option<crate::Curve>> {
    let mut parts = s.split('|');
    (0..3).map(|_| parts.next().and_then(crate::Curve::parse)).collect()
}

fn glow(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let thr = ctx.params.f("threshold") as f32 / 100.0;
    let radius = ctx.params.f("radius") * b.scale;
    let intensity = ctx.params.f("intensity") as f32;
    let alpha_based = ctx.params.e("based") == 0;
    let use_colors = ctx.params.e("colors") == 1;
    let arbitrary = (ctx.params.e("colors") == 2).then(|| glow_map(ctx.params.s("arbitraryMap")));
    let glow_op = glow_operation(ctx);
    let looping = ctx.params.e("colorLooping");
    let loops = ctx.params.f("colorLoops") as f32;
    let phase = (ctx.params.f("colorPhase") / 360.0) as f32;
    let mid = ctx.params.f("abMidpoint") as f32 / 100.0;
    let ca = ctx.params.color("colorA");
    let cb = ctx.params.color("colorB");
    let (kx, ky) = dims_xy(ctx.params.e("glowDimensions"));
    if !ctx.adjustment {
        b.pad((radius * 1.5).ceil() as u32 + 2);
    }
    // Bright pass.
    let mut bright = b.img.clone();
    bright.data.par_iter_mut().for_each(|px| {
        let a = px[3];
        if a <= 0.0 {
            *px = [0.0; 4];
            return;
        }
        // Glow Based On: brightness from the colours, or from the alpha channel.
        let l = if alpha_based { a } else { luminance(px[0] / a, px[1] / a, px[2] / a) };
        let k = ((l - thr) / (1.0 - thr).max(1e-3)).clamp(0.0, 1.0);
        if let Some(map) = &arbitrary {
            let t = glow_ab_t(l, looping, loops, phase, mid);
            let c = map.iter().map(|cv| cv.as_ref().map_or(t, |cv| cv.eval(t))).collect::<Vec<f32>>();
            *px = [c[0] * k * a, c[1] * k * a, c[2] * k * a, k * a];
        } else if use_colors {
            let t = glow_ab_t(l, looping, loops, phase, mid);
            let c = [ca[0] + (cb[0] - ca[0]) * t, ca[1] + (cb[1] - ca[1]) * t, ca[2] + (cb[2] - ca[2]) * t];
            *px = [c[0] * k * a, c[1] * k * a, c[2] * k * a, k * a];
        } else {
            *px = px.map(|v| v * k);
        }
    });
    let s = (radius / 2.0).max(0.5);
    let blurred = gaussian_blur(&bright, s * kx, s * ky, false);
    let mode = ctx.params.e("operation");
    b.img.data.par_iter_mut().zip(blurred.data.par_iter()).for_each(|(o, g)| {
        let g = g.map(|v| v * intensity);
        match mode {
            // Behind: the glow shows only where the original is transparent.
            1 => {
                let k = 1.0 - o[3];
                for c in 0..4 {
                    o[c] += g[c] * k;
                }
            }
            // None: the glow alone.
            2 => *o = [g[0], g[1], g[2], g[3]],
            // On Top with another Glow Operation: that blend mode.
            _ if glow_op != effectcraft_color::BlendMode::Add => *o = effectcraft_color::blend_pixel(glow_op, *o, g, 0.0),
            // On Top: add (screen-like add on premultiplied colour; alpha grows by the glow's alpha).
            _ => {
                for c in 0..3 {
                    o[c] += g[c];
                }
                o[3] = (o[3] + g[3] * (1.0 - o[3])).min(1.0);
            }
        }
        o[3] = o[3].clamp(0.0, 1.0);
    });
    b
}

/// Physical Glow's settings in buffer pixels, shared by the CPU effect and the GPU kernels.
pub struct PhysicalGlowPlan {
    /// Transparent pixels to add on every side (0 on adjustment layers).
    pub pad: u32,
    /// Per octave: the Gaussian's sigmas (x, y) and its RGBA weights, exposure and tint folded in.
    pub octaves: Vec<([f64; 2], [f32; 4])>,
    /// Bright pass: 0 = everything glows, 1 = hard threshold at `thr[0]`, 2 = smoothstep from
    /// `thr[1]` to `thr[2]`.
    pub threshold_mode: u32,
    pub thr: [f32; 3],
    /// 1 Screen, 2 Glow Only, anything else Add.
    pub composite: u32,
    /// Fast Mode: blur each octave on a downsampled pyramid level ([`Self::fast_octaves`]).
    pub fast: bool,
}

/// One Fast Mode octave, per axis (x, y): the pyramid level (halvings), the Gaussian's radius in
/// taps at that level (0 = no blur) and its `1 / (2 sigma^2)`; plus the octave's RGBA weights.
///
/// Level pixel `i` at level `m` is centred at buffer coordinate `(i + 0.5) * 2^m`. A level above 0
/// stores one extra pixel on the low side (array index = `i + 1`): the edge pixel's outer
/// [1, 3, 3, 1] tap lands on level pixel −1, and keeping it conserves energy at both edges.
pub struct PhysicalGlowOctave {
    pub level: [u32; 2],
    pub taps: [u32; 2],
    pub k: [f32; 2],
    pub weights: [f32; 4],
}

impl PhysicalGlowPlan {
    /// The bright pass of one premultiplied pixel (non-finite channels count as 0).
    fn bright(&self, px: [f32; 4]) -> [f32; 4] {
        let px = px.map(|v| if v.is_finite() { v } else { 0.0 });
        let m = px[0].max(px[1]).max(px[2]);
        let w = match self.threshold_mode {
            0 => 1.0,
            1 => {
                if m >= self.thr[0] {
                    1.0
                } else {
                    0.0
                }
            }
            _ => crate::util::smoothstep(self.thr[1], self.thr[2], m),
        };
        px.map(|v| v * w)
    }

    /// Fast Mode's octaves for a `w` × `h` buffer, smallest sigma first, so levels only grow.
    pub fn fast_octaves(&self, w: u32, h: u32) -> Vec<PhysicalGlowOctave> {
        let axis = |s: f64, n: u32| {
            if !s.is_finite() || s <= 0.05 {
                return (0, 0, 0.0);
            }
            // Deepest level that keeps at least 8 px on this axis.
            let (mut cap, mut size) = (0u32, n);
            while size.div_ceil(2) >= 8 {
                size = size.div_ceil(2);
                cap += 1;
            }
            let m = ((s / 2.0).log2().floor().max(0.0) as u32).min(cap);
            let s4 = 4f64.powi(m as i32);
            // Each [1, 3, 3, 1] halving adds 3/4 of its input's px², (4^m - 1) / 4 px² in all; the
            // bilinear upsample from a level below adds about 4^m / 6 (none at level 0: no upsample).
            let up = if m > 0 { s4 / 6.0 } else { 0.0 };
            let v = ((s * s - (s4 - 1.0) / 4.0 - up) / s4).max(0.0);
            let sl = v.sqrt();
            if sl <= 0.05 {
                return (m, 0, 0.0);
            }
            (m, (3.0 * sl).ceil().min(128.0) as u32, (0.5 / v) as f32)
        };
        self.octaves
            .iter()
            .rev()
            .map(|(s, weights)| {
                let (x, y) = (axis(s[0], w), axis(s[1], h));
                PhysicalGlowOctave { level: [x.0, y.0], taps: [x.1, y.1], k: [x.2, y.2], weights: *weights }
            })
            .collect()
    }

    /// The source `s` with the glow `g`; non-finite channels become 0.
    fn composite(&self, s: [f32; 4], g: [f32; 4]) -> [f32; 4] {
        let o = match self.composite {
            1 => {
                let sc = |c: usize| s[c] + g[c].clamp(0.0, 1.0) * (1.0 - s[c].min(1.0));
                [sc(0), sc(1), sc(2), s[3] + g[3].clamp(0.0, 1.0) * (1.0 - s[3])]
            }
            2 => [g[0], g[1], g[2], g[3].min(1.0)],
            _ => [s[0] + g[0], s[1] + g[1], s[2] + g[2], (s[3] + g[3]).min(1.0)],
        };
        o.map(|v| if v.is_finite() { v } else { 0.0 })
    }
}

/// [`PhysicalGlowPlan`] for an instance at buffer scale `scale`.
pub fn physical_glow_plan(ctx: &EffectCtx, scale: f64) -> PhysicalGlowPlan {
    // Non-finite values fall back; finite ones are held to the slider's valid range.
    let f = |id: &str, fallback: f64, lo: f64, hi: f64| {
        let v = ctx.params.f(id);
        if v.is_finite() { v.clamp(lo, hi) } else { fallback }
    };
    let smax = f("radius", 0.0, 0.0, 4000.0) * scale / 2.0;
    let sigmas: Vec<f64> = if smax.is_finite() && smax >= 0.5 {
        let n = ((smax / 0.5).log2().floor() as usize + 1).min(12);
        (0..n).map(|k| smax / 2f64.powi(k as i32)).collect()
    } else {
        vec![0.0]
    };
    let sa = f("aspectRatio", 1.0, 0.05, 20.0).sqrt();
    let alpha = f("falloff", 2.0, 1.0, 4.0);
    let d = f("dispersion", 0.0, -100.0, 100.0) / 100.0;
    let exps = [alpha - d, alpha, alpha + d, alpha].map(|a| a.clamp(0.5, 4.5));
    // Octave weights per channel: sigma_k^(2 - alpha) up to a constant, normalised to sum to 1.
    let raw = |k: usize, a: f64| 2f64.powf(-(k as f64) * (2.0 - a));
    let sums = exps.map(|a| (0..sigmas.len()).map(|k| raw(k, a)).sum::<f64>());
    let g = 2f64.powf(f("exposure", 0.0, -10.0, 10.0));
    let t = f("tintAmount", 0.0, 0.0, 100.0) / 100.0;
    let tint = ctx.params.color("tint").map(|v| if v.is_finite() { f64::from(v) } else { 1.0 });
    let gain = [0, 1, 2].map(|c| g * ((1.0 - t) + t * tint[c]));
    let gain = [gain[0], gain[1], gain[2], g];
    let octaves: Vec<([f64; 2], [f32; 4])> =
        sigmas.iter().enumerate().map(|(k, &s)| ([s * sa, s / sa], [0, 1, 2, 3].map(|c| (raw(k, exps[c]) / sums[c] * gain[c]) as f32))).collect();
    let s0 = sigmas.first().map_or(0.0, |s| s * sa.max(1.0 / sa));
    let pad = if ctx.adjustment { 0 } else { ((3.0 * s0).ceil() + 2.0).min(4096.0) as u32 };
    let thr = f("threshold", 0.0, 0.0, 100.0);
    let soft = f("thresholdSoftness", 50.0, 0.0, 100.0) / 100.0;
    let (lo, hi) = (thr * (1.0 - soft), thr * (1.0 + soft));
    let threshold_mode = if thr <= 0.0 {
        0
    } else if hi - lo < 1e-6 {
        1
    } else {
        2
    };
    PhysicalGlowPlan {
        pad,
        octaves,
        threshold_mode,
        thr: [thr as f32, lo as f32, hi as f32],
        composite: ctx.params.e("composite"),
        fast: ctx.params.b("fastMode"),
    }
}

/// Physical Glow: an energy-conserving glow whose falloff follows a power law (inverse square by
/// default), built from a sum of Gaussians whose widths double per octave.
fn physical_glow(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let plan = physical_glow_plan(ctx, b.scale);
    b.pad(plan.pad);
    let mut bright = b.img.clone();
    bright.data.par_iter_mut().for_each(|px| *px = plan.bright(*px));
    let mut glow = Image::new(b.img.width, b.img.height);
    if plan.fast {
        let (mut level, mut lv) = (bright, [0u32; 2]);
        for o in plan.fast_octaves(b.img.width, b.img.height) {
            for (axis, vertical) in [(0, false), (1, true)] {
                while lv[axis] < o.level[axis] {
                    level = pglow_halve(&level, vertical, i64::from(lv[axis] > 0));
                    lv[axis] += 1;
                }
            }
            let bx = pglow_gauss(&level, false, o.taps[0], o.k[0]);
            let by = pglow_gauss(bx.as_ref().unwrap_or(&level), true, o.taps[1], o.k[1]);
            let blurred = by.as_ref().or(bx.as_ref()).unwrap_or(&level);
            let inv = o.level.map(|m| 0.5f64.powi(m as i32));
            let shift = o.level.map(|m| if m > 0 { 1.0 } else { 0.0 });
            glow.rows_mut().for_each(|(y, row)| {
                for (x, g) in row.iter_mut().enumerate() {
                    let v = blurred.sample_bilinear((x as f64 + 0.5) * inv[0] + shift[0], (y as f64 + 0.5) * inv[1] + shift[1]);
                    for c in 0..4 {
                        g[c] += o.weights[c] * v[c];
                    }
                }
            });
        }
    } else {
        for (s, w) in &plan.octaves {
            let blurred = gaussian_blur(&bright, s[0], s[1], false);
            glow.data.par_iter_mut().zip(blurred.data.par_iter()).for_each(|(g, v)| {
                for c in 0..4 {
                    g[c] += w[c] * v[c];
                }
            });
        }
    }
    b.img.data.par_iter_mut().zip(glow.data.par_iter()).for_each(|(o, g)| *o = plan.composite(*o, *g));
    b
}

/// Fast Mode: halve `img` along one axis with the centroid-preserving [1, 3, 3, 1] / 8 filter
/// (the transpose of the bilinear ×2 upsample): level pixel `i` takes input pixels `2i - 1 ..=
/// 2i + 2`, transparent outside. `shift` is 1 when `img` is itself a level above 0 (see
/// [`PhysicalGlowOctave`]); the result always is.
fn pglow_halve(img: &Image, vertical: bool, shift: i64) -> Image {
    let half = |n: u32| n.saturating_sub(shift as u32) / 2 + 2;
    let (w, h) = if vertical { (img.width, half(img.height)) } else { (half(img.width), img.height) };
    let mut out = Image::new(w, h);
    out.rows_mut().for_each(|(y, row)| {
        for (x, px) in row.iter_mut().enumerate() {
            let (x, y) = (x as i64, y as i64);
            let tap = |t: i64| if vertical { img.get(x, 2 * y - 3 + shift + t) } else { img.get(2 * x - 3 + shift + t, y) };
            let (a, b, c, d) = (tap(0), tap(1), tap(2), tap(3));
            *px = [0, 1, 2, 3].map(|i| (a[i] + 3.0 * b[i] + 3.0 * c[i] + d[i]) / 8.0);
        }
    });
    out
}

/// Fast Mode: a sampled Gaussian along one axis, taps `exp(-i² k)` for |i| ≤ `taps`, normalised;
/// transparent outside. `None` when `taps` is 0 (no blur). The GPU kernel computes the same taps.
fn pglow_gauss(img: &Image, vertical: bool, taps: u32, k: f32) -> Option<Image> {
    if taps == 0 {
        return None;
    }
    let r = i64::from(taps);
    let w: Vec<f32> = (-r..=r).map(|i| (-((i * i) as f32) * k).exp()).collect();
    let sum: f32 = w.iter().sum();
    let mut out = Image::new(img.width, img.height);
    out.rows_mut().for_each(|(y, row)| {
        for (x, px) in row.iter_mut().enumerate() {
            let mut acc = [0.0f32; 4];
            for (j, wj) in w.iter().enumerate() {
                let d = j as i64 - r;
                let p = if vertical { img.get(x as i64, y as i64 + d) } else { img.get(x as i64 + d, y as i64) };
                for c in 0..4 {
                    acc[c] += wj * p[c];
                }
            }
            *px = acc.map(|v| v / sum);
        }
    });
    Some(out)
}

fn drop_shadow(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let color = ctx.params.color("color");
    let opacity = ctx.params.f("opacity") as f32 / 100.0;
    let dir = ctx.params.f("direction").to_radians();
    let dist = ctx.params.f("distance") * b.scale;
    let soft = ctx.params.f("softness") * b.scale;
    let only = ctx.params.b("shadowOnly");
    let pad = (dist + soft * 1.5).ceil() as u32 + 2;
    if !ctx.adjustment {
        b.pad(pad);
    }
    let (dx, dy) = (dir.sin() * dist, -dir.cos() * dist);
    let mut sh = Image::new(b.img.width, b.img.height);
    let src = &b.img;
    sh.rows_mut().for_each(|(y, row)| {
        for (x, px) in row.iter_mut().enumerate() {
            let a = src.sample_bilinear(x as f64 + 0.5 - dx, y as f64 + 0.5 - dy)[3] * opacity * color[3];
            *px = [color[0] * a, color[1] * a, color[2] * a, a];
        }
    });
    if soft > 0.0 {
        sh = gaussian_blur(&sh, soft / 2.0, soft / 2.0, false);
    }
    if !only {
        sh.data.par_iter_mut().zip(b.img.data.par_iter()).for_each(|(s, o)| {
            let k = 1.0 - o[3];
            for c in 0..4 {
                s[c] = o[c] + s[c] * k;
            }
        });
    }
    b.img = sh;
    b
}

/// Invert's Channel options (After Effects order): RGB / HLS / YIQ groups, each followed by its
/// single channels, then Alpha.
pub const INVERT_CHANNELS: [&str; 13] =
    ["RGB", "Red", "Green", "Blue", "HLS", "Hue", "Lightness", "Saturation", "YIQ", "Luminance", "In Phase Chrominance", "Quadrature Chrominance", "Alpha"];
/// Index of "Alpha" in [`INVERT_CHANNELS`].
pub const INVERT_ALPHA: u32 = 12;

fn rgb_to_yiq(c: [f32; 3]) -> [f32; 3] {
    [0.299 * c[0] + 0.587 * c[1] + 0.114 * c[2], 0.596 * c[0] - 0.274 * c[1] - 0.322 * c[2], 0.211 * c[0] - 0.523 * c[1] + 0.312 * c[2]]
}

fn yiq_to_rgb(c: [f32; 3]) -> [f32; 3] {
    [c[0] + 0.956 * c[1] + 0.621 * c[2], c[0] - 0.272 * c[1] - 0.647 * c[2], c[0] - 1.106 * c[1] + 1.703 * c[2]]
}

/// Invert straight colour `s` in the colour space / channel `ch` (an [`INVERT_CHANNELS`] index
/// other than Alpha).
fn invert_color(ch: u32, s: [f32; 3]) -> [f32; 3] {
    match ch {
        1 => [1.0 - s[0], s[1], s[2]],
        2 => [s[0], 1.0 - s[1], s[2]],
        3 => [s[0], s[1], 1.0 - s[2]],
        4..=7 => {
            let (mut h, mut sat, mut l) = effectcraft_color::rgb_to_hsl(s[0], s[1], s[2]);
            if matches!(ch, 4 | 5) {
                h = 1.0 - h;
            }
            if matches!(ch, 4 | 6) {
                l = 1.0 - l;
            }
            if matches!(ch, 4 | 7) {
                sat = 1.0 - sat;
            }
            let (r, g, b) = effectcraft_color::hsl_to_rgb(h, sat, l);
            [r, g, b]
        }
        8..=11 => {
            // Y inverts around 1, the (signed) chrominance axes around 0.
            let mut q = rgb_to_yiq(s);
            if matches!(ch, 8 | 9) {
                q[0] = 1.0 - q[0];
            }
            if matches!(ch, 8 | 10) {
                q[1] = -q[1];
            }
            if matches!(ch, 8 | 11) {
                q[2] = -q[2];
            }
            yiq_to_rgb(q).map(|v| v.max(0.0))
        }
        _ => [1.0 - s[0], 1.0 - s[1], 1.0 - s[2]],
    }
}

fn invert(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let ch = ctx.params.e("channel");
    let blend = 1.0 - ctx.params.f("blend") as f32 / 100.0;
    b.img.data.par_iter_mut().for_each(|px| {
        let a = px[3];
        if ch == INVERT_ALPHA {
            let na = 1.0 - a;
            let k = if a > 0.0 { na / a } else { 0.0 };
            let inv = [px[0] * k, px[1] * k, px[2] * k, na];
            for c in 0..4 {
                px[c] += (inv[c] - px[c]) * blend;
            }
            return;
        }
        if a <= 0.0 {
            return;
        }
        let s = [px[0] / a, px[1] / a, px[2] / a];
        let o = invert_color(ch, s);
        for c in 0..3 {
            px[c] = (s[c] + (o[c] - s[c]) * blend) * a;
        }
    });
    b
}

fn mosaic(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let hb = ctx.params.f("horizontal").round().max(1.0) as usize;
    let vb = ctx.params.f("vertical").round().max(1.0) as usize;
    let (w, h) = (b.img.width as usize, b.img.height as usize);
    let cw = (w as f64 / hb as f64).max(1.0);
    let ch = (h as f64 / vb as f64).max(1.0);
    let src = b.img.clone();
    if ctx.params.b("sharpColors") {
        // Each tile takes the colour of the pixel at its centre.
        b.img.rows_mut().for_each(|(y, row)| {
            let cy = ((y as f64 / ch).floor() + 0.5) * ch;
            for (x, px) in row.iter_mut().enumerate() {
                let cx = ((x as f64 / cw).floor() + 0.5) * cw;
                *px = src.get_clamped(cx as i64, cy as i64);
            }
        });
        return b;
    }
    // Each tile takes the average colour of its region.
    let (nx, ny) = ((w as f64 / cw).ceil() as usize, (h as f64 / ch).ceil() as usize);
    let mut sums = vec![[0.0f64; 5]; nx * ny];
    for y in 0..h {
        let ty = ((y as f64 / ch) as usize).min(ny - 1);
        for x in 0..w {
            let tx = ((x as f64 / cw) as usize).min(nx - 1);
            let p = src.get(x as i64, y as i64);
            let s = &mut sums[ty * nx + tx];
            for c in 0..4 {
                s[c] += p[c] as f64;
            }
            s[4] += 1.0;
        }
    }
    b.img.rows_mut().for_each(|(y, row)| {
        let ty = ((y as f64 / ch) as usize).min(ny - 1);
        for (x, px) in row.iter_mut().enumerate() {
            let tx = ((x as f64 / cw) as usize).min(nx - 1);
            let s = sums[ty * nx + tx];
            *px = [0, 1, 2, 3].map(|c| (s[c] / s[4].max(1.0)) as f32);
        }
    });
    b
}

fn posterize(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let lv = ctx.params.f("level").round().clamp(2.0, 255.0) as f32 - 1.0;
    b.img.map_straight(|c| c.map(|v| (v.clamp(0.0, 1.0) * lv).round() / lv));
    b
}

fn threshold(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let t = ctx.params.f("level") as f32 / 255.0;
    b.img.map_straight(|c| {
        let v = if luminance(c[0], c[1], c[2]) >= t { 1.0 } else { 0.0 };
        [v, v, v]
    });
    b
}

fn find_edges(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let inv = ctx.params.b("invert");
    let blend = ctx.params.f("blend") as f32 / 100.0;
    let src = b.img.clone();
    b.img.rows_mut().for_each(|(y, row)| {
        for (x, px) in row.iter_mut().enumerate() {
            let l = |dx: i64, dy: i64| {
                let p = src.get_clamped(x as i64 + dx, y as i64 + dy);
                luminance(p[0], p[1], p[2])
            };
            let gx = l(1, -1) + 2.0 * l(1, 0) + l(1, 1) - l(-1, -1) - 2.0 * l(-1, 0) - l(-1, 1);
            let gy = l(-1, 1) + 2.0 * l(0, 1) + l(1, 1) - l(-1, -1) - 2.0 * l(0, -1) - l(1, -1);
            let e = (gx * gx + gy * gy).sqrt().min(1.0);
            let v = if inv { e } else { 1.0 - e };
            let a = px[3];
            *px = blend_with(*px, [v * a, v * a, v * a, a], blend);
        }
    });
    b
}

/// Blend With Original: `orig` over the effect result `fx` by `k` (0 = effect only).
fn blend_with(orig: [f32; 4], fx: [f32; 4], k: f32) -> [f32; 4] {
    [0, 1, 2, 3].map(|c| fx[c] + (orig[c] - fx[c]) * k)
}

fn emboss(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let ang = ctx.params.f("direction").to_radians();
    let relief = ctx.params.f("relief") * b.scale;
    let contrast = ctx.params.f("contrast") as f32 / 100.0;
    let blend = ctx.params.f("blend") as f32 / 100.0;
    let (dx, dy) = (ang.cos() * relief, -ang.sin() * relief);
    let src = b.img.clone();
    b.img.rows_mut().for_each(|(y, row)| {
        for (x, px) in row.iter_mut().enumerate() {
            let a = src.sample_bilinear_clamped(x as f64 + 0.5 + dx, y as f64 + 0.5 + dy);
            let c = src.sample_bilinear_clamped(x as f64 + 0.5 - dx, y as f64 + 0.5 - dy);
            let d = luminance(a[0], a[1], a[2]) - luminance(c[0], c[1], c[2]);
            let v = (0.5 + d * contrast).clamp(0.0, 1.0);
            let al = px[3];
            *px = blend_with(*px, [v * al, v * al, v * al, al], blend);
        }
    });
    b
}

fn noise(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let amt = ctx.params.f("amount") as f32 / 100.0;
    let color = ctx.params.b("color");
    // Old saves lack the switch: clipping was always on.
    let clip = ctx.params.get("clip").is_none_or(|v| v.as_bool());
    let frame_seed = (ctx.time * 1000.0) as u32 ^ ctx.seed;
    let w = b.img.width;
    b.img.rows_mut().for_each(|(y, row)| {
        for (x, px) in row.iter_mut().enumerate() {
            let a = px[3];
            if a <= 0.0 {
                continue;
            }
            let n = |k: u32| (hash_noise(x as u32 + k * w, y as u32, frame_seed) - 0.5) * amt;
            let (r, g, bl) = if color {
                (n(0), n(1), n(2))
            } else {
                let v = n(0);
                (v, v, v)
            };
            if clip {
                px[0] = (px[0] + r * a).clamp(0.0, a);
                px[1] = (px[1] + g * a).clamp(0.0, a);
                px[2] = (px[2] + bl * a).clamp(0.0, a);
            } else {
                // Unclipped: values past black / white wrap around (integer overflow look).
                for (c, n) in [r, g, bl].into_iter().enumerate() {
                    px[c] = ((px[c] / a + n).rem_euclid(1.0)) * a;
                }
            }
        }
    });
    b
}

fn linear_wipe(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let done = ctx.params.f("completion") / 100.0;
    let ang = ctx.params.f("angle").to_radians();
    let feather = (ctx.params.f("feather") * b.scale).max(0.001);
    let (w, h) = (b.img.width as f64, b.img.height as f64);
    // Direction the wipe travels: angle 90° wipes left→right.
    let (dx, dy) = (ang.sin(), -ang.cos());
    let corners = [(0.0, 0.0), (w, 0.0), (0.0, h), (w, h)];
    let proj: Vec<f64> = corners.iter().map(|(x, y)| x * dx + y * dy).collect();
    let (lo, hi) = (proj.iter().cloned().fold(f64::INFINITY, f64::min), proj.iter().cloned().fold(f64::NEG_INFINITY, f64::max));
    let edge = lo - feather + (hi - lo + 2.0 * feather) * done;
    b.img.rows_mut().for_each(|(y, row)| {
        for (x, px) in row.iter_mut().enumerate() {
            let d = (x as f64 + 0.5) * dx + (y as f64 + 0.5) * dy;
            let k = ((d - edge) / feather + 0.5).clamp(0.0, 1.0) as f32;
            for c in px.iter_mut() {
                *c *= k;
            }
        }
    });
    b
}

fn radial_wipe(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let done = ctx.params.f("completion") / 100.0;
    let start = ctx.params.f("startAngle");
    let c = b.to_px(ctx.params.v2("center"));
    let dir = ctx.params.e("wipe");
    let feather = (ctx.params.f("feather")).max(0.01);
    b.img.rows_mut().for_each(|(y, row)| {
        for (x, px) in row.iter_mut().enumerate() {
            let a = ((x as f64 + 0.5 - c.0).atan2(-(y as f64 + 0.5 - c.1)).to_degrees() - start).rem_euclid(360.0);
            // Both: the wipe opens in both directions from the start angle at once.
            let (a, edge) = match dir {
                1 => (360.0 - a, done * 360.0),
                2 => (a.min(360.0 - a), done * 180.0),
                _ => (a, done * 360.0),
            };
            let k = ((a - edge) / feather + 0.5).clamp(0.0, 1.0) as f32;
            for ch in px.iter_mut() {
                *ch *= k;
            }
        }
    });
    b
}

fn venetian(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let done = ctx.params.f("completion") / 100.0;
    let ang = ctx.params.f("direction").to_radians();
    let width = (ctx.params.f("width") * b.scale).max(1.0);
    let feather = (ctx.params.f("feather") * b.scale).max(0.001);
    let (dx, dy) = (ang.cos(), ang.sin());
    b.img.rows_mut().for_each(|(y, row)| {
        for (x, px) in row.iter_mut().enumerate() {
            let d = ((x as f64 + 0.5) * dx + (y as f64 + 0.5) * dy).rem_euclid(width);
            let k = ((d - done * width) / feather + 0.5).clamp(0.0, 1.0) as f32;
            for c in px.iter_mut() {
                *c *= k;
            }
        }
    });
    b
}

pub fn specs() -> Vec<EffectSpec> {
    let dims = || popup(&["Horizontal and Vertical", "Horizontal", "Vertical"]);
    vec![
        spec(
            "ec.blur.gaussian",
            "Gaussian Blur",
            "Blur & Sharpen",
            vec![
                p("blurriness", "Blurriness", num(0.0), slider(0.0, 3000.0, 0.0, 50.0, 1)),
                p("dimensions", "Blur Dimensions", Value::Enum(0), dims()),
                p("repeatEdge", "Repeat Edge Pixels", Value::Bool(false), ParamUi::Checkbox),
            ],
            gaussian,
        ),
        spec(
            "ec.blur.fastbox",
            "Fast Box Blur",
            "Blur & Sharpen",
            vec![
                p("radius", "Blur Radius", num(0.0), slider(0.0, 3000.0, 0.0, 50.0, 1)),
                p("iterations", "Iterations", num(3.0), slider(1.0, 50.0, 1.0, 5.0, 0)),
                p("dimensions", "Blur Dimensions", Value::Enum(0), dims()),
                p("repeatEdge", "Repeat Edge Pixels", Value::Bool(false), ParamUi::Checkbox),
            ],
            box_blur,
        ),
        spec(
            "ec.blur.directional",
            "Directional Blur",
            "Blur & Sharpen",
            vec![p("direction", "Direction", num(0.0), ParamUi::Angle), p("length", "Blur Length", num(0.0), slider(0.0, 1000.0, 0.0, 100.0, 1))],
            directional,
        ),
        spec(
            "ec.blur.radial",
            "Radial Blur",
            "Blur & Sharpen",
            vec![
                p("amount", "Amount", num(10.0), slider(0.0, 100.0, 0.0, 100.0, 1)),
                p("center", "Center", Value::Vec2([0.5, 0.5]), ParamUi::Point),
                p("type", "Type", Value::Enum(0), popup(&["Spin", "Zoom"])),
                // Our blur is always fully anti-aliased; the choice is kept for compatibility.
                p("antialiasing", "Antialiasing (Best Quality)", Value::Enum(0), popup(&["Low", "High"])),
            ],
            radial,
        ),
        spec("ec.blur.sharpen", "Sharpen", "Blur & Sharpen", vec![p("amount", "Sharpen Amount", num(0.0), slider(0.0, 500.0, 0.0, 100.0, 0))], sharpen),
        spec(
            "ec.blur.unsharp",
            "Unsharp Mask",
            "Blur & Sharpen",
            vec![
                p("amount", "Amount", num(50.0), slider(0.0, 500.0, 0.0, 500.0, 0)),
                p("radius", "Radius", num(1.0), slider(0.1, 500.0, 0.1, 50.0, 1)),
                p("threshold", "Threshold", num(0.0), slider(0.0, 255.0, 0.0, 255.0, 0)),
            ],
            unsharp,
        ),
        spec(
            "ec.stylize.glow",
            "Glow",
            "Stylize",
            vec![
                p("based", "Glow Based On", Value::Enum(1), popup(&["Alpha Channel", "Color Channels"])),
                p("threshold", "Glow Threshold", num(60.0), slider(0.0, 100.0, 0.0, 100.0, 1)),
                p("radius", "Glow Radius", num(10.0), slider(0.0, 1000.0, 0.0, 100.0, 1)),
                p("intensity", "Glow Intensity", num(1.0), slider(0.0, 255.0, 0.0, 4.0, 1)),
                p("operation", "Composite Original", Value::Enum(0), popup(&["On Top", "Behind", "None"])),
                p("glowOperation", "Glow Operation", Value::Enum(1), popup(&GLOW_OPERATIONS)),
                p("colors", "Glow Colors", Value::Enum(0), popup(&["Original Colors", "A & B Colors", "Arbitrary Map"])),
                // Arbitrary Map: "red points | green points | blue points" (Curves format).
                p("arbitraryMap", "Arbitrary Map", Value::Str(String::new()), ParamUi::Hidden),
                p("colorLooping", "Color Looping", Value::Enum(2), popup(&["Sawtooth A>B", "Sawtooth B>A", "Triangle A>B>A", "Triangle B>A>B"])),
                p("colorLoops", "Color Loops", num(1.0), slider(1.0, 127.0, 1.0, 10.0, 1)),
                p("colorPhase", "Color Phase", num(0.0), ParamUi::Angle),
                p("abMidpoint", "A & B Midpoint", num(50.0), slider(0.0, 100.0, 0.0, 100.0, 1)),
                p("colorA", "Color A", col(1.0, 1.0, 1.0), ParamUi::Color),
                p("colorB", "Color B", col(0.0, 0.0, 0.0), ParamUi::Color),
                p("glowDimensions", "Glow Dimensions", Value::Enum(0), dims()),
            ],
            glow,
        ),
        spec(
            "ec.stylize.physicalglow",
            "Physical Glow",
            "Stylize",
            vec![
                p("radius", "Radius", num(100.0), slider(0.0, 4000.0, 0.0, 500.0, 1)),
                p("exposure", "Exposure", num(0.0), slider(-10.0, 10.0, -4.0, 4.0, 2)),
                p("falloff", "Falloff", num(2.0), slider(1.0, 4.0, 1.0, 4.0, 2)),
                p("threshold", "Threshold", num(0.0), slider(0.0, 100.0, 0.0, 2.0, 2)),
                p("thresholdSoftness", "Threshold Softness", num(50.0), slider(0.0, 100.0, 0.0, 100.0, 1)),
                p("aspectRatio", "Aspect Ratio", num(1.0), slider(0.05, 20.0, 0.25, 4.0, 2)),
                p("dispersion", "Dispersion", num(0.0), slider(-100.0, 100.0, -100.0, 100.0, 1)),
                p("tint", "Tint", col(1.0, 1.0, 1.0), ParamUi::Color),
                p("tintAmount", "Tint Amount", num(0.0), slider(0.0, 100.0, 0.0, 100.0, 1)),
                p("composite", "Composite", Value::Enum(0), popup(&["Add", "Screen", "Glow Only"])),
                p("fastMode", "Fast Mode", Value::Bool(false), ParamUi::Checkbox),
                // The CPU always renders this instance (see catalog::CPU_ONLY_CONTROLS).
                p("renderMethod", "Render Method", Value::Enum(0), popup(&["GPU", "CPU"])),
            ],
            physical_glow,
        ),
        spec(
            "ec.stylize.mosaic",
            "Mosaic",
            "Stylize",
            vec![
                p("horizontal", "Horizontal Blocks", num(10.0), slider(1.0, 4000.0, 1.0, 200.0, 0)),
                p("vertical", "Vertical Blocks", num(10.0), slider(1.0, 4000.0, 1.0, 200.0, 0)),
                p("sharpColors", "Sharp Colors", Value::Bool(false), ParamUi::Checkbox),
            ],
            mosaic,
        ),
        spec("ec.stylize.posterize", "Posterize", "Stylize", vec![p("level", "Level", num(6.0), slider(2.0, 255.0, 2.0, 32.0, 0))], posterize),
        spec("ec.stylize.threshold", "Threshold", "Stylize", vec![p("level", "Level", num(128.0), slider(0.0, 255.0, 0.0, 255.0, 0))], threshold),
        spec(
            "ec.stylize.findedges",
            "Find Edges",
            "Stylize",
            vec![p("invert", "Invert", Value::Bool(false), ParamUi::Checkbox), p("blend", "Blend With Original", num(0.0), slider(0.0, 100.0, 0.0, 100.0, 0))],
            find_edges,
        ),
        spec(
            "ec.stylize.emboss",
            "Emboss",
            "Stylize",
            vec![
                p("direction", "Direction", num(45.0), ParamUi::Angle),
                p("relief", "Relief", num(1.5), slider(0.0, 10.0, 0.0, 10.0, 2)),
                p("contrast", "Contrast", num(100.0), slider(0.0, 500.0, 0.0, 500.0, 0)),
                p("blend", "Blend With Original", num(0.0), slider(0.0, 100.0, 0.0, 100.0, 0)),
            ],
            emboss,
        ),
        spec(
            "ec.perspective.dropshadow",
            "Drop Shadow",
            "Perspective",
            vec![
                p("color", "Shadow Color", col(0.0, 0.0, 0.0), ParamUi::Color),
                p("opacity", "Opacity", num(50.0), slider(0.0, 100.0, 0.0, 100.0, 0)),
                p("direction", "Direction", num(135.0), ParamUi::Angle),
                p("distance", "Distance", num(5.0), slider(0.0, 32000.0, 0.0, 120.0, 1)),
                p("softness", "Softness", num(0.0), slider(0.0, 1000.0, 0.0, 250.0, 1)),
                p("shadowOnly", "Shadow Only", Value::Bool(false), ParamUi::Checkbox),
            ],
            drop_shadow,
        ),
        spec(
            "ec.channel.invert",
            "Invert",
            "Channel",
            vec![
                p("channel", "Channel", Value::Enum(0), popup(&INVERT_CHANNELS)),
                p("blend", "Blend With Original", num(0.0), slider(0.0, 100.0, 0.0, 100.0, 0)),
            ],
            invert,
        ),
        spec(
            "ec.noise.noise",
            "Noise",
            "Noise & Grain",
            vec![
                p("amount", "Amount of Noise", num(0.0), slider(0.0, 100.0, 0.0, 100.0, 1)),
                p("color", "Use Color Noise", Value::Bool(true), ParamUi::Checkbox),
                p("clip", "Clip Result Values", Value::Bool(true), ParamUi::Checkbox),
            ],
            noise,
        ),
        spec(
            "ec.transition.linearwipe",
            "Linear Wipe",
            "Transition",
            vec![
                p("completion", "Transition Completion", num(0.0), slider(0.0, 100.0, 0.0, 100.0, 0)),
                p("angle", "Wipe Angle", num(90.0), ParamUi::Angle),
                p("feather", "Feather", num(0.0), slider(0.0, 32000.0, 0.0, 500.0, 1)),
            ],
            linear_wipe,
        ),
        spec(
            "ec.transition.radialwipe",
            "Radial Wipe",
            "Transition",
            vec![
                p("completion", "Transition Completion", num(0.0), slider(0.0, 100.0, 0.0, 100.0, 0)),
                p("startAngle", "Start Angle", num(0.0), ParamUi::Angle),
                p("center", "Wipe Center", Value::Vec2([0.5, 0.5]), ParamUi::Point),
                p("wipe", "Wipe", Value::Enum(0), popup(&["Clockwise", "Counterclockwise", "Both"])),
                p("feather", "Feather", num(0.0), slider(0.0, 360.0, 0.0, 50.0, 1)),
            ],
            radial_wipe,
        ),
        spec(
            "ec.transition.venetian",
            "Venetian Blinds",
            "Transition",
            vec![
                p("completion", "Transition Completion", num(0.0), slider(0.0, 100.0, 0.0, 100.0, 0)),
                p("direction", "Direction", num(90.0), ParamUi::Angle),
                p("width", "Width", num(30.0), slider(2.0, 1000.0, 2.0, 200.0, 0)),
                p("feather", "Feather", num(0.0), slider(0.0, 100.0, 0.0, 100.0, 1)),
            ],
            venetian,
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EffectEnv, run_fx};

    /// White left half, black right half (one vertical edge at x = 20).
    fn edge() -> effectcraft_raster::Image {
        let mut img = effectcraft_raster::Image::filled(40, 8, [0.0, 0.0, 0.0, 1.0]);
        for y in 0..8 {
            for x in 0..20 {
                img.set(x, y, [1.0, 1.0, 1.0, 1.0]);
            }
        }
        img
    }

    fn blurred_row(id: &str, param: &str, v: f64) -> Vec<f32> {
        let out = run_fx(id, &[(param, num(v)), ("repeatEdge", Value::Bool(true))], edge(), 0.0, EffectEnv::default());
        (16..24).map(|x| out.img.get(x, 4)[0]).collect()
    }

    #[test]
    fn small_gaussian_blurriness_is_continuous() {
        // #1939: 0.9 used to be identical to 0, and 1.2 / 1.6 / 2.0 identical to each other.
        let row = |v| blurred_row("ec.blur.gaussian", "blurriness", v);
        assert_ne!(row(0.0), row(0.9));
        assert_ne!(row(0.9), row(1.2));
        assert_ne!(row(1.2), row(1.6));
        assert_ne!(row(1.6), row(2.0));
        assert_ne!(row(2.0), row(2.4));
    }

    #[test]
    fn fractional_box_blur_radius_is_continuous() {
        let row = |v| blurred_row("ec.blur.fastbox", "radius", v);
        assert_ne!(row(0.0), row(0.5));
        assert_ne!(row(0.5), row(0.9));
        assert_ne!(row(1.2), row(1.6));
        assert_ne!(row(1.6), row(2.4));
    }

    /// A bright square on mid grey.
    fn scene() -> effectcraft_raster::Image {
        let mut img = effectcraft_raster::Image::filled(24, 24, [0.3, 0.3, 0.3, 1.0]);
        for y in 8..16 {
            for x in 8..16 {
                img.set(x, y, [1.0, 1.0, 1.0, 1.0]);
            }
        }
        img
    }

    #[test]
    fn glow_operation_and_arbitrary_map() {
        // Pixels addressed in layer space (Glow pads its buffer).
        struct Out(Buf);
        impl Out {
            fn get(&self, x: i64, y: i64) -> [f32; 4] {
                self.0.img.get(x + self.0.offset[0] as i64, y + self.0.offset[1] as i64)
            }
        }
        let run = |vals: &[(&str, Value)]| Out(run_fx("ec.stylize.glow", vals, scene(), 0.0, EffectEnv::default()));
        let add = run(&[]);
        assert_eq!(GLOW_OPERATIONS[1], "Add");
        // Multiply darkens around the square where Add brightens.
        let mul = run(&[("glowOperation", Value::Enum(2))]);
        assert!(add.get(6, 12)[0] > 0.31 && mul.get(6, 12)[0] <= 0.3 + 1e-4, "{:?} {:?}", add.get(6, 12), mul.get(6, 12));
        assert_eq!(
            glow_operation(&EffectCtx {
                params: &crate::Params::default(),
                time: 0.0,
                layer_size: [1.0; 2],
                seed: 0,
                adjustment: false,
                env: Default::default()
            }),
            effectcraft_color::BlendMode::Add
        );
        // Arbitrary Map: the glow takes the mapped colour (here pure red for every brightness).
        let red = run(&[("colors", Value::Enum(2)), ("arbitraryMap", Value::Str("0,1 1,1|0,0 1,0|0,0 1,0".into())), ("operation", Value::Enum(2))]);
        let p = red.get(6, 12);
        assert!(p[0] > 0.05 && p[1] < 1e-4 && p[2] < 1e-4, "{p:?}");
    }

    fn pglow(vals: &[(&str, Value)], img: Image) -> Buf {
        run_fx("ec.stylize.physicalglow", vals, img, 0.0, EffectEnv::default())
    }

    /// Pixel (x, y) in layer space (Physical Glow pads its buffer).
    fn at(b: &Buf, x: i64, y: i64) -> [f32; 4] {
        b.img.get(x + b.offset[0] as i64, y + b.offset[1] as i64)
    }

    /// A transparent `w` × `h` layer with one opaque white pixel at (x, y).
    fn point(w: u32, h: u32, x: u32, y: u32) -> Image {
        let mut img = Image::new(w, h);
        img.set(x, y, [1.0; 4]);
        img
    }

    const GLOW_ONLY: (&str, Value) = ("composite", Value::Enum(2));

    #[test]
    fn physical_glow_is_deterministic() {
        let vals = [
            ("aspectRatio", num(2.5)),
            ("dispersion", num(30.0)),
            ("threshold", num(0.5)),
            ("thresholdSoftness", num(20.0)),
            ("tint", col(1.0, 0.6, 0.2)),
            ("tintAmount", num(40.0)),
            ("composite", Value::Enum(1)),
        ];
        let a = pglow(&vals, scene());
        let b = pglow(&vals, scene());
        assert_eq!(a.img, b.img);
        assert!(a.img.data.iter().all(|p| p.iter().all(|v| v.is_finite())));
    }

    #[test]
    fn physical_glow_conserves_energy() {
        let mut img = Image::new(64, 64);
        for y in 30..34 {
            for x in 30..34 {
                img.set(x, y, [1.0, 0.5, 0.25, 1.0]);
            }
        }
        let src: f64 = img.data.iter().map(|p| f64::from(p[0] + p[1] + p[2])).sum();
        let out = pglow(&[GLOW_ONLY], img);
        let glow: f64 = out.img.data.iter().map(|p| f64::from(p[0] + p[1] + p[2])).sum();
        assert!((glow / src - 1.0).abs() < 0.02, "{glow} vs {src}");
    }

    #[test]
    fn physical_glow_falls_off_as_a_power_law() {
        let ratios = |falloff: f64| {
            let out = pglow(&[("radius", num(400.0)), ("falloff", num(falloff)), GLOW_ONLY], point(32, 32, 16, 16));
            [16, 32, 64].map(|r| at(&out, 16 + 2 * r, 16)[0] / at(&out, 16 + r, 16)[0])
        };
        let (square, cube) = (ratios(2.0), ratios(3.0));
        for (k, r) in square.iter().enumerate() {
            assert!((0.15..=0.40).contains(r), "{square:?}");
            assert!(cube[k] < *r, "{cube:?} vs {square:?}");
        }
    }

    #[test]
    fn physical_glow_threshold_drops_dark_pixels() {
        // Threshold 1, softness 50 %: below 0.5 nothing glows.
        let vals = [("threshold", num(1.0)), GLOW_ONLY];
        let dim = Image::filled(24, 24, [0.4, 0.4, 0.4, 1.0]);
        assert!(pglow(&vals, dim.clone()).img.data.iter().all(|p| *p == [0.0; 4]));
        let mut lit = dim;
        lit.set(12, 12, [2.0, 2.0, 2.0, 1.0]);
        let out = pglow(&vals, lit);
        assert!(at(&out, 2, 2)[0] > 0.0);
    }

    #[test]
    fn physical_glow_aspect_and_dispersion() {
        let out = pglow(&[("radius", num(40.0)), ("aspectRatio", num(4.0)), GLOW_ONLY], point(40, 40, 20, 20));
        assert!(at(&out, 30, 20)[0] > at(&out, 20, 30)[0]);
        // Wavelength-dependent spread: far from a white source, red outreaches blue at +50 %.
        for (d, red_wins) in [(50.0, true), (-50.0, false)] {
            let out = pglow(&[("radius", num(200.0)), ("dispersion", num(d)), GLOW_ONLY], point(40, 40, 20, 20));
            let p = at(&out, 80, 20);
            assert_eq!(p[0] > p[2], red_wins, "{d}: {p:?}");
        }
    }

    #[test]
    fn physical_glow_screen_stays_in_range() {
        let out = pglow(&[("radius", num(20.0)), ("exposure", num(4.0)), ("composite", Value::Enum(1))], scene());
        assert!(out.img.data.iter().all(|p| p.iter().all(|v| *v <= 1.0)), "screen went over 1");
    }

    #[test]
    fn physical_glow_survives_hostile_values() {
        let mut img = Image::filled(4, 4, [0.5, 0.2, 0.9, 1.0]);
        img.set(1, 1, [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 1.0]);
        let finite = |b: &Buf| b.img.data.iter().all(|p| p.iter().all(|v| v.is_finite()));
        for v in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -1e300, 1e300] {
            let vals = [
                ("radius", num(if v.is_finite() { 3.0 } else { v })),
                ("exposure", num(v)),
                ("falloff", num(v)),
                ("threshold", num(v)),
                ("thresholdSoftness", num(v)),
                ("aspectRatio", num(v)),
                ("dispersion", num(v)),
                ("tint", Value::Color([v, v, v, 1.0])),
                ("tintAmount", num(v)),
                ("composite", Value::Enum(99)),
            ];
            assert!(finite(&pglow(&vals, img.clone())), "{v}");
        }
        // The widest radius on a tiny layer: the padding stays capped. Checked on the plan, since
        // rendering it allocates gigabytes.
        let s = crate::find("ec.stylize.physicalglow").unwrap();
        let mut params = crate::Params { values: s.params.iter().map(|p| (p.id.to_string(), crate::default_value(p, [4.0, 4.0]))).collect() };
        params.values.insert("radius".into(), num(4000.0));
        params.values.insert("aspectRatio".into(), num(0.05));
        let ctx = EffectCtx { params: &params, time: 0.0, layer_size: [4.0, 4.0], seed: 1, adjustment: false, env: EffectEnv::default() };
        let plan = physical_glow_plan(&ctx, 1.0);
        assert_eq!(plan.pad, 4096);
        assert_eq!(plan.octaves.len(), 12);
        assert!(plan.octaves.iter().all(|(s, w)| s.iter().all(|v| v.is_finite()) && w.iter().all(|v| v.is_finite())));
    }

    const FAST: (&str, Value) = ("fastMode", Value::Bool(true));

    #[test]
    fn physical_glow_fast_mode_is_deterministic() {
        let vals = [FAST, ("aspectRatio", num(2.5)), ("dispersion", num(30.0)), ("radius", num(40.0)), ("composite", Value::Enum(1))];
        let a = pglow(&vals, scene());
        assert_eq!(a.img, pglow(&vals, scene()).img);
        assert!(a.img.data.iter().all(|p| p.iter().all(|v| v.is_finite())));
    }

    #[test]
    fn physical_glow_fast_mode_conserves_energy() {
        let mut img = Image::new(64, 64);
        for y in 30..34 {
            for x in 30..34 {
                img.set(x, y, [1.0, 0.5, 0.25, 1.0]);
            }
        }
        let src: f64 = img.data.iter().map(|p| f64::from(p[0] + p[1] + p[2])).sum();
        let out = pglow(&[GLOW_ONLY, FAST], img);
        let glow: f64 = out.img.data.iter().map(|p| f64::from(p[0] + p[1] + p[2])).sum();
        assert!((glow / src - 1.0).abs() < 0.02, "{glow} vs {src}");
    }

    /// Red glow at (x, y) in layer space.
    fn red(b: &Buf, x: i64, y: i64) -> f32 {
        at(b, x, y)[0]
    }

    #[test]
    fn physical_glow_fast_mode_matches_the_reference() {
        let run = |fast: bool| pglow(&[("radius", num(200.0)), GLOW_ONLY, ("fastMode", Value::Bool(fast))], point(32, 32, 16, 16));
        let (reference, fast) = (run(false), run(true));
        let sum = |b: &Buf| b.img.data.iter().map(|p| f64::from(p[0])).sum::<f64>();
        assert!((sum(&fast) / sum(&reference) - 1.0).abs() < 0.02, "{} vs {}", sum(&fast), sum(&reference));
        // Fast Mode against true sampled Gaussians (sigma 100 / 2^k, k < 8, equal weights at
        // falloff 2) along the source's row, everywhere they are above 1e-4 of the peak.
        let sigmas: Vec<f64> = (0..8).map(|k| 100.0 / 2f64.powi(k)).collect();
        let norms: Vec<f64> = sigmas.iter().map(|s| (-700..=700).map(|j: i64| (-((j * j) as f64) / (2.0 * s * s)).exp()).sum()).collect();
        let ideal = |d: i64| sigmas.iter().zip(&norms).map(|(s, n)| (-((d * d) as f64) / (2.0 * s * s)).exp() / (n * n)).sum::<f64>() / 8.0;
        for d in -320..320 {
            if ideal(d) > 1e-4 * ideal(0) {
                let f = f64::from(red(&fast, 16 + d, 16));
                assert!((f / ideal(d) - 1.0).abs() < 0.03, "{d}: {f} vs {}", ideal(d));
            }
        }
        // Against the box-pass reference from 8 px out: nearer, the reference's small octaves are
        // far from Gaussian (43 % below one at the centre), and it is up to 11 % off further out.
        let peak = red(&reference, 16, 16);
        for d in (-320..=-8).chain(8..320) {
            let (f, r) = (red(&fast, 16 + d, 16), red(&reference, 16 + d, 16));
            if r > 1e-4 * peak {
                assert!((f / r - 1.0).abs() < 0.15, "{d}: {f} vs {r}");
            }
        }
    }

    #[test]
    fn physical_glow_fast_mode_falls_off_as_a_power_law() {
        let out = pglow(&[("radius", num(400.0)), GLOW_ONLY, FAST], point(32, 32, 16, 16));
        let ratios = [16, 32, 64].map(|r| red(&out, 16 + 2 * r, 16) / red(&out, 16 + r, 16));
        assert!(ratios.iter().all(|r| (0.15..=0.40).contains(r)), "{ratios:?}");
    }

    #[test]
    fn physical_glow_fast_mode_is_round() {
        let out = pglow(&[("radius", num(200.0)), GLOW_ONLY, FAST], point(32, 32, 16, 16));
        // On an axis at d vs at (a, b) near the diagonal, a² + b² = d².
        for (d, a, b) in [(5, 3, 4), (29, 20, 21), (58, 40, 42)] {
            let (axis, diagonal) = (red(&out, 16 + d, 16), red(&out, 16 + a, 16 + b));
            assert!((axis / diagonal - 1.0).abs() < 0.03, "{d}: {axis} vs {diagonal}");
        }
    }

    /// A moving highlight's glow must not swim: wherever the source sits on the coarse levels'
    /// grid, the glow's centre of mass stays on it and its profile keeps its shape.
    #[test]
    fn physical_glow_fast_mode_does_not_swim() {
        let offsets = [0, 2, 8, 24, 64, 128, -8, -64];
        let mut profiles = vec![];
        for x0 in (100..108).chain([111, 115, 119]) {
            let out = pglow(&[("radius", num(200.0)), GLOW_ONLY, FAST], point(208, 32, x0, 16));
            let (mut m, mut mx) = (0.0f64, 0.0f64);
            for (i, p) in out.img.data.iter().enumerate() {
                let x = (i % out.img.width as usize) as f64 + 0.5 - out.offset[0];
                m += f64::from(p[0]);
                mx += f64::from(p[0]) * x;
            }
            let centre = f64::from(x0) + 0.5;
            assert!((mx / m - centre).abs() < 0.05, "{x0}: centroid {} vs {centre}", mx / m);
            profiles.push(offsets.map(|d| red(&out, i64::from(x0) + d, 16)));
        }
        for (k, d) in offsets.iter().enumerate() {
            let (lo, hi) = profiles.iter().map(|p| p[k]).fold((f32::MAX, 0.0f32), |(lo, hi), v| (lo.min(v), hi.max(v)));
            assert!(hi / lo - 1.0 < 0.03, "offset {d}: {lo} .. {hi}");
        }
    }

    /// Without padding (an adjustment layer), a source on the left edge and one on the right edge
    /// keep the same glow energy: the levels keep the edge pixels' outer taps on both sides.
    #[test]
    fn physical_glow_fast_mode_edges_are_symmetric() {
        let spec = crate::find("ec.stylize.physicalglow").unwrap();
        let mut params = crate::Params { values: spec.params.iter().map(|p| (p.id.to_string(), crate::default_value(p, [40.0, 9.0]))).collect() };
        params.values.insert("fastMode".into(), Value::Bool(true));
        params.values.insert("composite".into(), Value::Enum(2));
        params.values.insert("radius".into(), num(30.0));
        let ctx = EffectCtx { params: &params, time: 0.0, layer_size: [40.0, 9.0], seed: 1, adjustment: true, env: EffectEnv::default() };
        let energy = |x: u32| {
            let out = (spec.render)(&ctx, Buf { img: point(40, 9, x, 4), offset: [0.0; 2], scale: 1.0 });
            out.img.data.iter().map(|p| f64::from(p[0])).sum::<f64>()
        };
        let (left, right) = (energy(0), energy(39));
        assert!((left / right - 1.0).abs() < 1e-4, "{left} vs {right}");
    }

    #[test]
    fn physical_glow_fast_mode_keeps_narrow_axes_narrow() {
        for aspect in [20.0, 0.05] {
            let run =
                |fast: bool| pglow(&[("radius", num(100.0)), ("aspectRatio", num(aspect)), GLOW_ONLY, ("fastMode", Value::Bool(fast))], point(32, 32, 16, 16));
            let (reference, fast) = (run(false), run(true));
            assert!(fast.img.data.iter().all(|p| p.iter().all(|v| v.is_finite())));
            // (narrow, wide) offsets: y is the narrow axis at aspect 20, x at 0.05.
            let axes = |d: i64| if aspect > 1.0 { ((0, d), (d, 0)) } else { ((d, 0), (0, d)) };
            for d in [2, 4, 8] {
                let (n, w) = axes(d);
                let (f, r) = (red(&fast, 16 + n.0, 16 + n.1), red(&reference, 16 + n.0, 16 + n.1));
                assert!((f / r - 1.0).abs() < 0.2, "{aspect} {d}: {f} vs {r}");
                assert!(f < 0.05 * red(&fast, 16 + w.0, 16 + w.1), "{aspect} {d}");
            }
        }
    }

    #[test]
    fn physical_glow_fast_mode_on_tiny_buffers() {
        let spec = crate::find("ec.stylize.physicalglow").unwrap();
        for (w, h) in [(1, 1), (3, 5), (7, 6)] {
            for adjustment in [false, true] {
                for radius in [0.6, 100.0] {
                    let size = [f64::from(w), f64::from(h)];
                    let mut params = crate::Params { values: spec.params.iter().map(|p| (p.id.to_string(), crate::default_value(p, size))).collect() };
                    params.values.insert("fastMode".into(), Value::Bool(true));
                    params.values.insert("radius".into(), num(radius));
                    let ctx = EffectCtx { params: &params, time: 0.0, layer_size: size, seed: 1, adjustment, env: EffectEnv::default() };
                    let out = (spec.render)(&ctx, Buf { img: Image::filled(w, h, [0.8, 0.5, 0.2, 1.0]), offset: [0.0; 2], scale: 1.0 });
                    assert!(out.img.data.iter().all(|p| p.iter().all(|v| v.is_finite())), "{w}x{h} adj {adjustment} r {radius}");
                }
            }
        }
    }
}

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
    } else if repeat && !ctx.adjustment {
        // The edge it repeats is the layer's, not the painted content's (#1205).
        b.cover(ctx.layer_size);
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
    } else if repeat && !ctx.adjustment {
        // The edge it repeats is the layer's, not the painted content's (#1205).
        b.cover(ctx.layer_size);
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

/// Glow's blur: a Gaussian of σ = 0.384 × Glow Radius. Fitted to After Effects 2026 (cases
/// 807 and 808, Glow Radius 10 and 30 on a grey solid): the edge profile of the glow's
/// coverage is an erfc step of that σ within 1.7/255 (it was σ = radius / 2).
pub fn glow_sigma(radius: f64) -> f64 {
    (radius * 0.384).max(0.5)
}

/// Glow's bright pass for the original colours, per channel of the straight colour `v`:
/// `8 × (v − threshold + 0.0611)`, nothing below. Fitted to After Effects 2026 renders of grey
/// 0.65 / 0.8 / 0.9 / 1, orange and light-blue solids at Threshold 40 / 60 / 80 % and Radius 10
/// / 30 (36 channel values, exact to the 8-bit capture): the glow is linear in each channel,
/// its gain is the same at every threshold and radius, and channels below the threshold add
/// nothing (an orange solid glows red only).
pub fn glow_bright_channel(v: f32, thr: f32) -> f32 {
    (GLOW_GAIN * (v - thr + GLOW_KNEE)).max(0.0)
}

/// [`glow_bright_channel`]'s gain and threshold offset. `kernels.wgsl` repeats both as literals;
/// the GPU test `glow_constants_match_the_cpu` checks that they are these.
pub const GLOW_GAIN: f32 = 8.0;
pub const GLOW_KNEE: f32 = 0.0611;

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
        // Based on the colour channels, the glow's coverage is the layer's whole alpha, below
        // the threshold too: After Effects darkens what lies under a dim layer's edge (glow_alpha).
        let ga = if alpha_based { k * a } else { a };
        if let Some(map) = &arbitrary {
            let t = glow_ab_t(l, looping, loops, phase, mid);
            let c = map.iter().map(|cv| cv.as_ref().map_or(t, |cv| cv.eval(t))).collect::<Vec<f32>>();
            *px = [c[0] * k * a, c[1] * k * a, c[2] * k * a, ga];
        } else if use_colors {
            let t = glow_ab_t(l, looping, loops, phase, mid);
            let c = [ca[0] + (cb[0] - ca[0]) * t, ca[1] + (cb[1] - ca[1]) * t, ca[2] + (cb[2] - ca[2]) * t];
            *px = [c[0] * k * a, c[1] * k * a, c[2] * k * a, ga];
        } else if alpha_based {
            *px = [px[0] * k, px[1] * k, px[2] * k, ga];
        } else {
            // Original colours: each channel of the straight colour against the threshold.
            let c = [px[0] / a, px[1] / a, px[2] / a].map(|v| glow_bright_channel(v, thr) * a);
            *px = [c[0], c[1], c[2], ga];
        }
    });
    let s = glow_sigma(radius);
    let blurred = gaussian_blur(&bright, s * kx, s * ky, false);
    let mode = ctx.params.e("operation");
    b.img.data.par_iter_mut().zip(blurred.data.par_iter()).for_each(|(o, g)| {
        // Intensity scales the glow's colour; with the colour channels its coverage stays.
        let g = [g[0] * intensity, g[1] * intensity, g[2] * intensity, if alpha_based { g[3] * intensity } else { g[3] }];
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

/// Standard normal CDF Φ (Abramowitz & Stegun 7.1.26 erf, |error| < 1.5e-7).
fn norm_cdf(z: f64) -> f64 {
    let x = z.abs() / std::f64::consts::SQRT_2;
    let t = 1.0 / (1.0 + 0.327_591_1 * x);
    let poly = t * (0.254_829_592 + t * (-0.284_496_736 + t * (1.421_413_741 + t * (-1.453_152_027 + t * 1.061_405_429))));
    let erf = 1.0 - poly * (-x * x).exp();
    if z >= 0.0 { 0.5 * (1.0 + erf) } else { 0.5 * (1.0 - erf) }
}

/// Radial Wipe's wiped wedge relative to the Start Angle: (first ray, span), degrees clockwise
/// from up. Clockwise (0) wipes from the start angle on, Counterclockwise (1) back from it,
/// Both (2) half each way.
pub fn radial_wipe_wedge(wipe: u32, done: f64) -> (f64, f64) {
    let span = done.clamp(0.0, 1.0) * 360.0;
    match wipe {
        1 => (-span, span),
        2 => (-span / 2.0, span),
        _ => (0.0, span),
    }
}

/// Radial Wipe's feather: a Gaussian of σ = 0.4 × Feather pixels across both edges of the wedge
/// (After Effects 2026, Feather 20: the edge profile is the same width in pixels at every
/// radius, σ = 8.0 px, mean error 0.0002 over the frame).
pub fn radial_wipe_sigma(feather: f64) -> f64 {
    feather.max(0.0) * 0.4
}

/// Visible fraction of a pixel at `(ux, uy)` from the centre (y down) for a wiped wedge from
/// `ray0` (absolute degrees clockwise from up) over `span` degrees: the signed distance to the
/// wedge's nearer edge, through the feather's Gaussian.
pub fn radial_wipe_alpha(ux: f64, uy: f64, ray0: f64, span: f64, sigma: f64) -> f64 {
    let a = (ux.atan2(-uy).to_degrees() - ray0).rem_euclid(360.0);
    let wiped = a < span;
    let ray_dist = |deg: f64| {
        let (s, c) = deg.to_radians().sin_cos();
        // Ray direction (sin, −cos) in y-down pixels.
        let along = ux * s - uy * c;
        if along > 0.0 { (ux * c + uy * s).abs() } else { ux.hypot(uy) }
    };
    let d = ray_dist(ray0).min(ray_dist(ray0 + span));
    let sd = if wiped { -d } else { d };
    if sigma < 1e-3 {
        return if wiped { 0.0 } else { 1.0 };
    }
    norm_cdf(sd / sigma)
}

fn radial_wipe(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let done = (ctx.params.f("completion") / 100.0).clamp(0.0, 1.0);
    if done <= 0.0 {
        return b;
    }
    let start = ctx.params.f("startAngle");
    let c = b.to_px(ctx.params.v2("center"));
    let (w0, span) = radial_wipe_wedge(ctx.params.e("wipe"), done);
    let sigma = radial_wipe_sigma(ctx.params.f("feather")) * b.scale;
    let full = done >= 1.0;
    b.img.rows_mut().for_each(|(y, row)| {
        for (x, px) in row.iter_mut().enumerate() {
            let k = if full { 0.0 } else { radial_wipe_alpha(x as f64 + 0.5 - c.0, y as f64 + 0.5 - c.1, start + w0, span, sigma) as f32 };
            for ch in px.iter_mut() {
                *ch *= k;
            }
        }
    });
    b
}

/// Venetian Blinds: how much of a pixel at phase `d` (0..`width`) a band of `done × width`
/// covers, its two edges softened by a Gaussian of σ = Feather / 2 (σ in the units of `width`).
/// Fitted to After Effects 2026 (Completion 40, Direction 30, Width 50, Feather 5 on 960 × 540
/// and 480 × 270 layers): band from phase 0 to done × width, both edges Gaussian, within 6/255.
pub fn venetian_cover(d: f64, width: f64, done: f64, sigma: f64) -> f64 {
    let band = done * width;
    if sigma < 1e-3 {
        return if d < band { 1.0 } else { 0.0 };
    }
    // Wider than a few blinds the stripes blur into an even veil.
    if sigma > 8.0 * width {
        return done.clamp(0.0, 1.0);
    }
    let n = ((4.0 * sigma / width).ceil() as i64 + 1).clamp(1, 40);
    let mut t = 0.0;
    for k in -n..=n {
        let u = d + k as f64 * width;
        t += norm_cdf(u / sigma) - norm_cdf((u - band) / sigma);
    }
    t.clamp(0.0, 1.0)
}

/// Venetian Blinds' phase origin in layer pixels: the layer centre plus 0.6 px (fitted to the
/// AE renders above; the stripes are measured from the layer's centre, not its corner).
pub fn venetian_origin(layer_size: [f64; 2]) -> [f64; 2] {
    [layer_size[0] / 2.0 + 0.6, layer_size[1] / 2.0 + 0.6]
}

fn venetian(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let done = (ctx.params.f("completion") / 100.0).clamp(0.0, 1.0);
    if done <= 0.0 {
        return b;
    }
    let ang = ctx.params.f("direction").to_radians();
    let width = (ctx.params.f("width") * b.scale).max(1.0);
    let sigma = ctx.params.f("feather").max(0.0) * b.scale / 2.0;
    let (dx, dy) = (ang.cos(), ang.sin());
    let (ox, oy) = b.to_px(venetian_origin(ctx.layer_size));
    b.img.rows_mut().for_each(|(y, row)| {
        let ry = (y as f64 + 0.5 - oy) * dy;
        for (x, px) in row.iter_mut().enumerate() {
            let d = ((x as f64 + 0.5 - ox) * dx + ry).rem_euclid(width);
            let k = (1.0 - venetian_cover(d, width, done, sigma)) as f32;
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

    /// After Effects 2026 (cases 524, 807, 808, 809): Glow on a 50 % grey solid, which is below
    /// the 60 % threshold, still covers a blurred copy of the layer's alpha with black, so the
    /// red layer underneath darkens outside the edge. The halo is the same at Intensity 1 and 3
    /// and with Composite Original On Top or Behind; its width follows Glow Radius.
    #[test]
    fn glow_halo_matches_after_effects() {
        let grey = effectcraft_raster::Image::filled(200, 120, [0.5, 0.5, 0.5, 1.0]);
        // AE's red under the halo at 0.5, 1.5, 3.5, 5.5 and 7.5 px outside the left edge
        // (radius 10) and at 0.5, 5.5, 10.5, 20.5 px (radius 30); halo alpha = 1 − red.
        let r10 = [(0.5, 140.0), (1.5, 165.0), (3.5, 208.0), (5.5, 234.0), (7.5, 248.0)];
        let r30 = [(0.5, 132.0), (5.5, 173.0), (10.5, 208.0), (20.5, 245.0)];
        for (radius, intensity, operation, profile) in [(10.0, 1.0, 1, &r10[..]), (10.0, 3.0, 1, &r10[..]), (10.0, 1.0, 0, &r10[..]), (30.0, 1.0, 1, &r30[..])]
        {
            let set = [
                ("based", Value::Enum(1)),
                ("threshold", num(60.0)),
                ("radius", num(radius)),
                ("intensity", num(intensity)),
                ("operation", Value::Enum(operation)),
            ];
            let o = run_fx("ec.stylize.glow", &set, grey.clone(), 0.0, EffectEnv::default());
            for &(d, red) in profile {
                // Layer x = −d (pixel centre), mid-height; over AE's red: R = 1 − α, colour 0.
                let (bx, by) = o.to_px([-d, 60.0]);
                let p = o.img.get(bx.floor() as i64, by.floor() as i64);
                let ae = 1.0 - red / 255.0;
                assert!((p[3] - ae).abs() < 3.0 / 255.0, "radius {radius} intensity {intensity} op {operation} at {d} px: alpha {} vs AE {ae}", p[3]);
                assert!(p[0].abs() < 1e-6, "the halo is black: {p:?}");
            }
        }
    }

    /// After Effects 2026, Glow at Threshold 60 %, Radius 10, Intensity 0.25 on 200 × 200
    /// solids over red: the glow 1.5 px outside the left edge, premultiplied (the green / blue
    /// channels of the render, where the red background adds nothing), and its alpha 0.353.
    #[test]
    fn glow_brightness_matches_after_effects() {
        // (solid colour, channel, AE glow value 0..255)
        let cases = [
            ([0.65, 0.65, 0.65], 1, 20.0),
            ([0.8, 0.8, 0.8], 1, 47.0),
            ([0.9, 0.9, 0.9], 1, 65.0),
            ([1.0, 1.0, 1.0], 1, 83.0),
            ([1.0, 0.5, 0.0], 1, 0.0),
            ([0.2, 0.6, 1.0], 1, 11.0),
            ([0.2, 0.6, 1.0], 2, 83.0),
        ];
        for (c, ch, ae) in cases {
            let img = effectcraft_raster::Image::filled(200, 200, [c[0], c[1], c[2], 1.0]);
            let set = [("threshold", num(60.0)), ("radius", num(10.0)), ("intensity", num(0.25))];
            let o = run_fx("ec.stylize.glow", &set, img, 0.0, EffectEnv::default());
            let (bx, by) = o.to_px([-1.5, 100.0]);
            let p = o.img.get(bx.floor() as i64, by.floor() as i64);
            assert!((p[ch] * 255.0 - ae).abs() <= 2.5, "{c:?} channel {ch}: {} vs AE {ae}", p[ch] * 255.0);
            assert!((p[3] - 0.353).abs() < 3.0 / 255.0, "{c:?}: alpha {}", p[3]);
        }
        // Intensity scales the colour linearly: white at Intensity 1 is 3.7 × its alpha.
        let white = effectcraft_raster::Image::filled(200, 200, [1.0, 1.0, 1.0, 1.0]);
        let o = run_fx("ec.stylize.glow", &[("threshold", num(60.0)), ("radius", num(10.0))], white, 0.0, EffectEnv::default());
        let (bx, by) = o.to_px([-1.5, 100.0]);
        let p = o.img.get(bx.floor() as i64, by.floor() as i64);
        assert!((p[1] / p[3] - 3.69).abs() < 0.05, "{p:?}");
    }

    /// After Effects 2026: Radial Blur keeps the layer's edges. Zoom 20 on a flat grey 960 × 540
    /// solid (case 713) leaves it flat and opaque to the edge (EffectCraft faded the edges toward
    /// transparency, letting the red layer below show through); on a full-frame horizontal ramp
    /// (case 829, centre [960, 540]) row 540 reads 2/255 at x = 2 and 253/255 at x = 1917.
    #[test]
    fn radial_blur_keeps_the_layer_edges_as_after_effects() {
        let zoom = |c: [f64; 2]| [("amount", num(20.0)), ("type", Value::Enum(1)), ("center", Value::Vec2(c))];
        let grey = effectcraft_raster::Image::filled(960, 540, [0.5, 0.5, 0.5, 1.0]);
        let o = run_fx("ec.blur.radial", &zoom([600.0, 300.0]), grey, 0.0, EffectEnv::default());
        for (x, y) in [(0, 270), (1, 270), (959, 270), (480, 0), (480, 539), (0, 0), (959, 539)] {
            let p = o.img.get(x, y);
            assert!((p[3] - 1.0).abs() < 1e-4 && (p[0] - 0.5).abs() < 1e-4, "({x}, {y}): {p:?}");
        }
        let mut ramp = effectcraft_raster::Image::new(1920, 1080);
        ramp.rows_mut().for_each(|(_, row)| {
            for (x, px) in row.iter_mut().enumerate() {
                let v = (x as f32 + 0.5) / 1920.0;
                *px = [v, v, v, 1.0];
            }
        });
        let o = run_fx("ec.blur.radial", &zoom([960.0, 540.0]), ramp, 0.0, EffectEnv::default());
        for (x, ae) in [(2, 2.0), (1917, 253.0)] {
            let p = o.img.get(x, 540);
            assert!((p[0] * 255.0 - ae).abs() <= 2.0 && (p[3] - 1.0).abs() < 1e-4, "x {x}: {p:?} vs AE {ae}");
        }
    }

    /// After Effects 2026, Venetian Blinds Completion 40, Direction 30, Width 50, Feather 5 on
    /// an opaque 480 × 270 layer: alpha (0..255) along row y = 135. The transparent band starts
    /// at the layer centre's phase, both edges are Gaussian (σ = Feather / 2).
    #[test]
    fn venetian_blinds_match_after_effects() {
        let img = effectcraft_raster::Image::filled(480, 270, [0.5, 0.5, 0.5, 1.0]);
        let vals = [("completion", Value::Scalar(40.0)), ("direction", Value::Scalar(30.0)), ("width", Value::Scalar(50.0)), ("feather", Value::Scalar(5.0))];
        let o = run_fx("ec.transition.venetian", &vals, img, 0.0, EffectEnv::default());
        let ae = [
            (200, 5.0),
            (203, 47.0),
            (206, 145.0),
            (209, 228.0),
            (212, 253.0),
            (224, 255.0),
            (236, 238.0),
            (239, 168.0),
            (242, 66.0),
            (245, 10.0),
            (251, 0.0),
            (257, 2.0),
            (260, 31.0),
        ];
        for (x, a) in ae {
            let got = o.img.get(x, 135)[3] * 255.0;
            assert!((got - a).abs() <= 6.0, "x {x}: alpha {got} vs AE {a}");
        }
        // No completion leaves the layer; full completion clears it; no feather is a hard edge.
        let full = [("completion", Value::Scalar(100.0)), ("feather", Value::Scalar(5.0))];
        let img = effectcraft_raster::Image::filled(40, 20, [0.5, 0.5, 0.5, 1.0]);
        assert!(run_fx("ec.transition.venetian", &full, img.clone(), 0.0, EffectEnv::default()).img.data.iter().all(|p| p[3] < 1e-4));
        assert_eq!(run_fx("ec.transition.venetian", &[("completion", Value::Scalar(0.0))], img.clone(), 0.0, EffectEnv::default()).img, img);
        let hard = run_fx(
            "ec.transition.venetian",
            &[("completion", Value::Scalar(50.0)), ("width", Value::Scalar(10.0)), ("feather", Value::Scalar(0.0))],
            img,
            0.0,
            EffectEnv::default(),
        );
        assert!(hard.img.data.iter().all(|p| p[3] == 0.0 || p[3] == 1.0));
        // A feather far wider than the blinds is an even veil.
        assert_eq!(venetian_cover(3.0, 10.0, 0.5, 1e9), 0.5);
    }

    /// After Effects 2026, Radial Wipe Completion 40, Start Angle 30, Wipe Counterclockwise,
    /// Feather 20, centre (800, 500): visible fraction at pixel offsets from the centre on both
    /// edges (8-bit captures of dark steps, so ±0.05).
    #[test]
    fn radial_wipe_feather_matches_after_effects() {
        let img = effectcraft_raster::Image::filled(400, 300, [1.0, 1.0, 1.0, 1.0]);
        let vals = [
            ("completion", Value::Scalar(40.0)),
            ("startAngle", Value::Scalar(30.0)),
            ("center", Value::Vec2([200.0, 150.0])),
            ("wipe", Value::Enum(1)),
            ("feather", Value::Scalar(20.0)),
        ];
        let o = run_fx("ec.transition.radialwipe", &vals, img.clone(), 0.0, EffectEnv::default());
        let ae = [
            ((-50, 34), 0.922),
            ((-52, 30), 0.797),
            ((-54, 26), 0.609),
            ((-55, 24), 0.5),
            ((-56, 22), 0.406),
            ((-57, 19), 0.266),
            ((-58, 15), 0.125),
            ((-104, 60), 0.951),
            ((-108, 53), 0.732),
            ((-111, 45), 0.341),
            ((-114, 37), 0.062),
            ((22, -56), 0.156),
            ((26, -54), 0.312),
            ((30, -52), 0.531),
            ((34, -50), 0.729),
            ((37, -47), 0.875),
            ((53, -108), 0.177),
            ((67, -99), 0.875),
        ];
        for ((dx, dy), a) in ae {
            let got = o.img.get(200 + dx, 150 + dy)[3];
            assert!((got - a).abs() < 0.05, "offset ({dx}, {dy}): {got} vs AE {a}");
        }
        // 0 % leaves the layer, 100 % clears it, Feather 0 is a hard edge.
        let at = |c: f64, f: f64| {
            let v = [("completion", Value::Scalar(c)), ("feather", Value::Scalar(f))];
            run_fx("ec.transition.radialwipe", &v, img.clone(), 0.0, EffectEnv::default()).img
        };
        assert_eq!(at(0.0, 20.0), img);
        assert!(at(100.0, 20.0).data.iter().all(|p| p[3] == 0.0));
        assert!(at(50.0, 0.0).data.iter().all(|p| p[3] == 0.0 || p[3] == 1.0));
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
}

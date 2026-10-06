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

/// Modern Gaussian Blur's sigma in layer pixels, before the render scale is applied.
/// Enforce its declared numeric range for evaluated/imported values too; the narrower
/// slider range only controls dragging. NaN keeps the existing zero-blur fallback.
pub fn gaussian_blur_sigma(blurriness: f64) -> f64 {
    blurriness.max(0.0).clamp(0.0, 3000.0) * 0.5
}

fn dims_xy(dim: u32) -> (f64, f64) {
    match dim {
        1 => (1.0, 0.0),
        2 => (0.0, 1.0),
        _ => (1.0, 1.0),
    }
}

fn gaussian(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let s = gaussian_blur_sigma(ctx.params.f("blurriness")) * b.scale;
    if s <= 0.0 {
        return b;
    }
    let (kx, ky) = dims_xy(ctx.params.e("dimensions"));
    let repeat = ctx.params.b("repeatEdge");
    if !repeat && !ctx.adjustment {
        b.pad((s * 3.0).ceil() as u32);
    }
    b.img = gaussian_blur(&b.img, s * kx, s * ky, repeat || ctx.adjustment);
    b
}

fn box_blur(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let r = (ctx.params.f("radius") * b.scale).round().max(0.0) as usize;
    if r == 0 {
        return b;
    }
    let it = ctx.params.f("iterations").round().clamp(1.0, 50.0) as usize;
    let (kx, ky) = dims_xy(ctx.params.e("dimensions"));
    let repeat = ctx.params.b("repeatEdge");
    if !repeat && !ctx.adjustment {
        b.pad((r * it) as u32 + 1);
    }
    b.img = effectcraft_raster::box_blur(&b.img, r * kx as usize, r * ky as usize, it, repeat || ctx.adjustment);
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

    fn tiny_blur_impulse() -> Image {
        let mut img = Image::new(3, 1);
        img.set(1, 0, [0.25, 0.5, 1.0, 1.0]);
        img
    }

    #[test]
    fn gaussian_sigma_preserves_valid_values_and_bounds_invalid_inputs() {
        for (value, expected) in [
            (f64::NEG_INFINITY, 0.0),
            (-1.0, 0.0),
            (0.0, 0.0),
            (0.1, 0.05),
            (37.0, 18.5),
            (3000.0, 1500.0),
            (3001.0, 1500.0),
            (2e100, 1500.0),
            (f64::INFINITY, 1500.0),
            (f64::NAN, 0.0),
        ] {
            assert_eq!(gaussian_blur_sigma(value), expected, "blurriness {value}");
        }
    }

    #[test]
    fn gaussian_clamps_evaluated_blurriness_to_its_declared_hard_range() {
        let spec = crate::find("ec.blur.gaussian").unwrap();
        let param = spec.params.iter().find(|p| p.id == "blurriness").unwrap();
        let ParamUi::Slider { min, max, slider_min, slider_max, .. } = &param.ui else { panic!("Gaussian blurriness must declare its range") };
        assert_eq!((*min, *max), (0.0, 3000.0));
        assert_eq!((*slider_min, *slider_max), (0.0, 50.0), "the narrow drag range is distinct from the valid numeric range");
        let run = |blurriness| {
            run_fx("ec.blur.gaussian", &[("blurriness", num(blurriness)), ("repeatEdge", Value::Bool(true))], tiny_blur_impulse(), 0.0, EffectEnv::default())
        };
        let expected = run(*max);
        // This value overflows the old radius calculation immediately in debug builds;
        // in release its wrapped radii produce an unblurred impulse. Repeat edges avoids
        // any padding allocation, and the valid maximum needs only tiny row sums.
        let actual = run(2e100);
        assert_eq!(actual.img, expected.img);
        assert_eq!(actual.offset, expected.offset);
        assert_eq!(actual.scale, expected.scale);
    }

    #[test]
    fn gaussian_preserves_ordinary_blurriness_results() {
        let img = tiny_blur_impulse();
        let expected = gaussian_blur(&img, 18.5, 18.5, true);
        let actual = run_fx("ec.blur.gaussian", &[("blurriness", num(37.0)), ("repeatEdge", Value::Bool(true))], img, 0.0, EffectEnv::default());
        assert_eq!(actual.img, expected);
        assert_eq!(actual.offset, [0.0, 0.0]);
        assert_eq!(actual.scale, 1.0);
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
}

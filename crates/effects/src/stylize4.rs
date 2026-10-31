//! Stylize effects, batch 4: RGB Split, Halftone, Pixel Sort and Dither.
//!
//! EffectCraft's own effects: looks motion designers usually reach for third-party plug-ins
//! for, and which After Effects doesn't ship. The parameters are ours. The maths is standard
//! image processing: shifted colour channels (lateral chromatic aberration), an amplitude-
//! modulated dot screen, sorting spans of pixels by a key, and ordered (Bayer) and error-diffusion
//! (Floyd–Steinberg) dithering.

use effectcraft_color::{luminance, rgb_to_hsl};
use effectcraft_keyframe::Value;
use effectcraft_project::ParamUi;
use effectcraft_raster::Px;
use rayon::prelude::*;

use crate::util::{lerp4, premul, smoothstep, unpremul};
use crate::{Buf, EffectCtx, EffectSpec, col, num, p, popup, slider};

/// A CPU-only, 32-bit float Stylize effect.
fn spec(id: &'static str, name: &'static str, params: Vec<crate::ParamSpec>, render: crate::RenderFn) -> EffectSpec {
    EffectSpec { id, name, category: "Stylize", params, render, gpu: false, float: true }
}

/// A 0–100 % slider.
fn pct() -> ParamUi {
    slider(0.0, 100.0, 0.0, 100.0, 1)
}

/// Blend With Original: `orig` over the effect result `fx` by `k` (0 = effect only).
fn blend_with(orig: Px, fx: Px, k: f32) -> Px {
    if k <= 0.0 { fx } else { lerp4(fx, orig, k.min(1.0)) }
}

// ------------------------------------------------------------------ RGB Split

/// The most RGB Split moves a channel (px).
const MAX_SPLIT: f64 = 2000.0;

/// The largest channel weight RGB Split takes (×`amount`; Red/Green/Blue Shift ±200%).
const MAX_WEIGHT: f64 = 2.0;

/// RGB Split: the red and blue channels move apart along a direction (Linear) or away from a
/// centre (Radial, growing with the distance from it, like a lens' lateral chromatic
/// aberration); green stays. The layer grows by the shift.
fn rgb_split(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let amount = (ctx.params.f("amount") * b.scale).clamp(-MAX_SPLIT, MAX_SPLIT);
    if !amount.is_finite() || amount.abs() < 1e-3 {
        return b;
    }
    // Shift weights (−2–2; a channel moves `amount × weight`).
    let weight = |id| {
        let k = ctx.params.f(id) / 100.0;
        if k.is_finite() { k.clamp(-MAX_WEIGHT, MAX_WEIGHT) } else { 0.0 }
    };
    let weights = [weight("red"), weight("green"), weight("blue")];
    if !ctx.adjustment {
        // Grow by the farthest any channel moves, so heavily weighted channels aren't clipped.
        let far = weights.iter().fold(0.0f64, |m, k| m.max(k.abs()));
        b.pad((amount.abs() * far).ceil() as u32 + 1);
    }
    let radial = ctx.params.e("type") == 1;
    let ang = ctx.params.f("direction").to_radians();
    let ang = if ang.is_finite() { ang } else { 0.0 };
    let (dx, dy) = (ang.sin(), -ang.cos());
    let (cx, cy) = b.to_px(ctx.params.v2("center"));
    let (w, h) = (b.img.width as f64, b.img.height as f64);
    // Radial: the farthest corner moves by the full amount.
    let reach = [(0.0, 0.0), (w, 0.0), (0.0, h), (w, h)].iter().map(|(x, y)| ((x - cx).powi(2) + (y - cy).powi(2)).sqrt()).fold(1.0f64, f64::max);
    let src = b.img.clone();
    b.img.rows_mut().for_each(|(y, row)| {
        for (x, o) in row.iter_mut().enumerate() {
            let (px, py) = (x as f64 + 0.5, y as f64 + 0.5);
            let (ox, oy) = if radial { ((px - cx) / reach * amount, (py - cy) / reach * amount) } else { (dx * amount, dy * amount) };
            // Each channel is read from where it came from (premultiplied, with its own alpha).
            let mut out = [0.0f32; 4];
            for (c, k) in weights.iter().enumerate() {
                let s = src.sample_bilinear(px - ox * k, py - oy * k);
                out[c] = s[c];
                out[3] = out[3].max(s[3]);
            }
            *o = out;
        }
    });
    b
}

// ------------------------------------------------------------------ Halftone

/// Ink coverage (0–1) of the dot of halftone cell `(i, j)` at screen point `(u, v)`, for a cell
/// `s` px wide whose ink amount is `ink` (0–1); dots grow until they fill the cell.
fn dot_coverage(shape: u32, u: f64, v: f64, i: f64, j: f64, s: f64, ink: f64) -> f64 {
    let (du, dv) = (u - (i + 0.5) * s, v - (j + 0.5) * s);
    let ink = ink.clamp(0.0, 1.0);
    // Full ink fills the cell (anti-aliased edges would leave seams between full dots).
    if ink >= 0.999 {
        return 1.0;
    }
    // Signed distance outside the dot (negative inside), in px. Each shape's area is the ink.
    let d = match shape {
        // Lines: bands along u, `ink × s` wide.
        1 => dv.abs() - ink * s * 0.5,
        // Squares: side s √ink.
        2 => du.abs().max(dv.abs()) - ink.sqrt() * s * 0.5,
        // Circles: area ink × s² until they touch their neighbours (radius s / 2, ink π/4),
        // then growing to cover the cell's corners (radius s / √2) at full ink.
        _ => {
            let touch = std::f64::consts::FRAC_PI_4;
            let r = if ink <= touch {
                s * (ink / std::f64::consts::PI).sqrt()
            } else {
                s * (0.5 + (std::f64::consts::FRAC_1_SQRT_2 - 0.5) * (ink - touch) / (1.0 - touch))
            };
            (du * du + dv * dv).sqrt() - r
        }
    };
    1.0 - smoothstep(-0.5, 0.5, d as f32) as f64
}

/// Halftone: the layer as a printed dot screen, `dotSize` px cells at `angle`, the dots sized by
/// each cell's darkness (Ink on Paper) or each channel on its own screen (Color Channels: red,
/// green and blue light dots at the angle, +30° and +60°, as RGB screens do). Every cell reads
/// the layer at its centre; a dot may spill into the neighbouring cells, so those are checked too.
fn halftone(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let s = (ctx.params.f("dotSize") * b.scale).max(1.0);
    let base = ctx.params.f("angle").to_radians();
    let base = if base.is_finite() { base } else { 0.0 };
    let shape = ctx.params.e("shape");
    let channels = ctx.params.e("colorMode") == 1;
    let ink_c = ctx.params.color("inkColor");
    let paper_c = ctx.params.color("paperColor");
    let blend = (ctx.params.f("blend") / 100.0) as f32;
    let src = b.img.clone();
    // Coverage of the screen at `angle` for pixel (px, py), its ink taken from `ink_at`.
    let screen = |px: f64, py: f64, angle: f64, ink_at: &dyn Fn(Px) -> f64| -> f64 {
        let (sn, cs) = angle.sin_cos();
        let (u, v) = (px * cs + py * sn, -px * sn + py * cs);
        let (ci, cj) = ((u / s).floor(), (v / s).floor());
        let mut cov = 0.0f64;
        for dj in -1..=1 {
            for di in -1..=1 {
                let (i, j) = (ci + di as f64, cj + dj as f64);
                // The cell centre back in the image.
                let (cu, cv) = ((i + 0.5) * s, (j + 0.5) * s);
                let (x, y) = (cu * cs - cv * sn, cu * sn + cv * cs);
                let ink = ink_at(src.sample_bilinear_clamped(x, y));
                if ink > 0.0 {
                    cov = cov.max(dot_coverage(shape, u, v, i, j, s, ink));
                }
            }
        }
        cov
    };
    b.img.rows_mut().for_each(|(y, row)| {
        for (x, o) in row.iter_mut().enumerate() {
            let a = o[3];
            if a <= 0.0 {
                continue;
            }
            let (px, py) = (x as f64 + 0.5, y as f64 + 0.5);
            let fx = if channels {
                // Light dots: a bright channel is a big dot of that colour on black.
                let c: [f32; 3] = std::array::from_fn(|k| {
                    screen(px, py, base + k as f64 * std::f64::consts::FRAC_PI_6, &|q| {
                        let (c, qa) = unpremul(q);
                        if qa > 0.0 { c[k].clamp(0.0, 1.0) as f64 } else { 0.0 }
                    }) as f32
                });
                premul(c, a)
            } else {
                let cov = screen(px, py, base, &|q| {
                    let (c, qa) = unpremul(q);
                    if qa > 0.0 { 1.0 - luminance(c[0], c[1], c[2]).clamp(0.0, 1.0) as f64 } else { 0.0 }
                }) as f32;
                let c: [f32; 3] = std::array::from_fn(|k| paper_c[k] + (ink_c[k] - paper_c[k]) * cov);
                premul(c, a)
            };
            *o = blend_with(*o, fx, blend);
        }
    });
    b
}

// ------------------------------------------------------------------ Pixel Sort

/// The key Pixel Sort orders by and selects spans with: Brightness, Hue or Saturation (0–1).
fn sort_key(by: u32, p: Px) -> f32 {
    let (c, a) = unpremul(p);
    if a <= 0.0 {
        return 0.0;
    }
    match by {
        1 => rgb_to_hsl(c[0], c[1], c[2]).0,
        2 => rgb_to_hsl(c[0], c[1], c[2]).1,
        _ => luminance(c[0], c[1], c[2]).clamp(0.0, 1.0),
    }
}

/// Sort the spans of `line` whose key lies within `lo..=hi` (shorter than `max` px when `max` >
/// 0) by that key, ascending or descending.
fn sort_line(line: &mut [Px], by: u32, lo: f32, hi: f32, max: usize, reverse: bool) {
    let key = |p: Px| Some(sort_key(by, p)).filter(|k| p[3] > 0.0 && (lo..=hi).contains(k));
    sort_spans(line, max, reverse, &key);
}

/// Sort the spans of `line` whose pixels have a `key` (at most `max` px when `max` > 0) by it.
/// Linear: every pixel's key is taken at most three times, plus once per span.
fn sort_spans(line: &mut [Px], max: usize, reverse: bool, key: &dyn Fn(Px) -> Option<f32>) {
    let mut start = 0;
    while start < line.len() {
        let inside = |p: &Px| key(*p).is_some();
        let Some(first) = line.get(start..).and_then(|r| r.iter().position(inside)) else { break };
        let a = start + first;
        // Count no further than the span can reach: counting the whole run for every span made
        // a long run with a short Maximum Span Length quadratic.
        let reach = if max > 0 { max } else { usize::MAX };
        let run = line.get(a..).map_or(0, |r| r.iter().take(reach).take_while(|p| inside(p)).count());
        let b = a.saturating_add(run);
        if let Some(span) = line.get_mut(a..b) {
            let mut keyed: Vec<(f32, Px)> = span.iter().map(|p| (key(*p).unwrap_or(0.0), *p)).collect();
            keyed.sort_by(|x, y| if reverse { y.0.total_cmp(&x.0) } else { x.0.total_cmp(&y.0) });
            for (o, (_, p)) in span.iter_mut().zip(keyed) {
                *o = p;
            }
        }
        start = b.max(a + 1);
    }
}

/// Pixel Sort: along each row (Horizontal) or column (Vertical), the runs of pixels whose
/// brightness, hue or saturation lies between Lower and Upper Threshold are sorted by it — the
/// streaky "glitch" look. Pixels outside the thresholds stay where they are.
fn pixel_sort(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let by = ctx.params.e("sortBy");
    let lo = (ctx.params.f("lowerThreshold") / 100.0) as f32;
    let hi = (ctx.params.f("upperThreshold") / 100.0) as f32;
    let (lo, hi) = (lo.min(hi), lo.max(hi));
    let max = (ctx.params.f("maxLength") * b.scale).round().max(0.0) as usize;
    let reverse = ctx.params.b("reverse");
    let vertical = ctx.params.e("direction") == 1;
    let (w, h) = (b.img.width as usize, b.img.height as usize);
    if w == 0 || h == 0 {
        return b;
    }
    if vertical {
        let mut cols: Vec<Vec<Px>> = (0..w).map(|x| (0..h).map(|y| b.img.data.get(y * w + x).copied().unwrap_or_default()).collect()).collect();
        cols.par_iter_mut().for_each(|c| sort_line(c, by, lo, hi, max, reverse));
        for (x, c) in cols.iter().enumerate() {
            for (y, p) in c.iter().enumerate() {
                if let Some(o) = b.img.data.get_mut(y * w + x) {
                    *o = *p;
                }
            }
        }
    } else {
        b.img.data.par_chunks_mut(w).for_each(|row| sort_line(row, by, lo, hi, max, reverse));
    }
    b
}

// ------------------------------------------------------------------ Dither

/// The 8 × 8 Bayer matrix (thresholds 0–63).
const BAYER8: [[u8; 8]; 8] = [
    [0, 32, 8, 40, 2, 34, 10, 42],
    [48, 16, 56, 24, 50, 18, 58, 26],
    [12, 44, 4, 36, 14, 46, 6, 38],
    [60, 28, 52, 20, 62, 30, 54, 22],
    [3, 35, 11, 43, 1, 33, 9, 41],
    [51, 19, 59, 27, 49, 17, 57, 25],
    [15, 47, 7, 39, 13, 45, 5, 37],
    [63, 31, 55, 23, 61, 29, 53, 21],
];

/// The ordered-dither threshold (−0.5–0.5) of cell `(x, y)` with an `n` × `n` Bayer matrix
/// (2, 4 or 8; the smaller ones are the top-left corners of the larger, renumbered).
fn bayer(n: usize, x: usize, y: usize) -> f32 {
    let n = n.clamp(2, 8);
    let v = BAYER8.get(y % n).and_then(|r| r.get(x % n)).copied().unwrap_or(0) as f32;
    // The corner of the 8 × 8 matrix holds every (64 / n²)-th value: rank it among n² levels.
    let step = (64 / (n * n)) as f32;
    ((v / step).floor() + 0.5) / (n * n) as f32 - 0.5
}

/// `v` (0–1) to the nearest of `levels` evenly spaced levels.
fn quantize(v: f32, levels: f32) -> f32 {
    ((v.clamp(0.0, 1.0) * (levels - 1.0)).round() / (levels - 1.0)).clamp(0.0, 1.0)
}

/// Dither: reduce the layer to `levels` levels per channel (or of grey, Monochrome), with an
/// ordered Bayer pattern (2 × 2, 4 × 4, 8 × 8) or Floyd–Steinberg error diffusion, on cells
/// `pixelSize` px wide for a chunky, retro look. Alpha is kept.
fn dither(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let method = ctx.params.e("method");
    let levels = ctx.params.f("levels").round();
    let levels = if levels.is_finite() { levels.clamp(2.0, 256.0) as f32 } else { 2.0 };
    let mono = ctx.params.e("color") == 1;
    let cell = (ctx.params.f("pixelSize") * b.scale).round().max(1.0) as usize;
    let blend = (ctx.params.f("blend") / 100.0) as f32;
    let (w, h) = (b.img.width as usize, b.img.height as usize);
    if w == 0 || h == 0 {
        return b;
    }
    // The cells' straight colours (their means), with the cell grid's size.
    let (gw, gh) = (w.div_ceil(cell), h.div_ceil(cell));
    let mut grid = vec![[0.0f32; 3]; gw * gh];
    grid.par_chunks_mut(gw).enumerate().for_each(|(gy, row)| {
        for (gx, g) in row.iter_mut().enumerate() {
            let (mut acc, mut wsum) = ([0.0f32; 3], 0.0f32);
            for y in gy * cell..((gy + 1) * cell).min(h) {
                for x in gx * cell..((gx + 1) * cell).min(w) {
                    let p = b.img.data.get(y * w + x).copied().unwrap_or_default();
                    for c in 0..3 {
                        acc[c] += p[c];
                    }
                    wsum += p[3];
                }
            }
            let c = if wsum > 0.0 { acc.map(|v| v / wsum) } else { [0.0; 3] };
            *g = if mono { [luminance(c[0], c[1], c[2]); 3] } else { c };
        }
    });
    match method {
        // Floyd–Steinberg: each cell's rounding error goes on to its unvisited neighbours (7/16
        // right, 3/16 down-left, 5/16 down, 1/16 down-right), row by row.
        3 => {
            for gy in 0..gh {
                for gx in 0..gw {
                    let Some(old) = grid.get(gy * gw + gx).copied() else { continue };
                    let new = old.map(|v| quantize(v, levels));
                    if let Some(g) = grid.get_mut(gy * gw + gx) {
                        *g = new;
                    }
                    let err: [f32; 3] = std::array::from_fn(|c| old[c] - new[c]);
                    for (ox, oy, k) in [(1i64, 0i64, 7.0f32), (-1, 1, 3.0), (0, 1, 5.0), (1, 1, 1.0)] {
                        let (nx, ny) = (gx as i64 + ox, gy as i64 + oy);
                        if nx < 0 || nx >= gw as i64 || ny >= gh as i64 {
                            continue;
                        }
                        if let Some(g) = grid.get_mut(ny as usize * gw + nx as usize) {
                            for c in 0..3 {
                                g[c] += err[c] * k / 16.0;
                            }
                        }
                    }
                }
            }
        }
        // Ordered: a threshold from the Bayer matrix nudges each cell before rounding.
        _ => {
            let n = [2usize, 4, 8].get(method as usize).copied().unwrap_or(4);
            let spread = 1.0 / (levels - 1.0);
            grid.par_chunks_mut(gw).enumerate().for_each(|(gy, row)| {
                for (gx, g) in row.iter_mut().enumerate() {
                    let t = bayer(n, gx, gy) * spread;
                    *g = g.map(|v| quantize(v + t, levels));
                }
            });
        }
    }
    b.img.rows_mut().for_each(|(y, row)| {
        for (x, o) in row.iter_mut().enumerate() {
            let a = o[3];
            if a <= 0.0 {
                continue;
            }
            let c = grid.get((y / cell) * gw + x / cell).copied().unwrap_or_default();
            *o = blend_with(*o, premul(c, a), blend);
        }
    });
    b
}

/// The batch's effects, for the registry.
pub fn specs() -> Vec<EffectSpec> {
    vec![
        spec(
            "ec.stylize.rgbsplit",
            "RGB Split",
            vec![
                p("type", "Split Type", Value::Enum(0), popup(&["Linear", "Radial"])),
                p("amount", "Amount", num(10.0), slider(-MAX_SPLIT, MAX_SPLIT, -100.0, 100.0, 1)),
                p("direction", "Direction", num(90.0), ParamUi::Angle),
                p("center", "Center", Value::Vec2([0.5, 0.5]), ParamUi::Point),
                p("red", "Red Shift", num(100.0), slider(-200.0, 200.0, -100.0, 100.0, 1)),
                p("green", "Green Shift", num(0.0), slider(-200.0, 200.0, -100.0, 100.0, 1)),
                p("blue", "Blue Shift", num(-100.0), slider(-200.0, 200.0, -100.0, 100.0, 1)),
            ],
            rgb_split,
        ),
        spec(
            "ec.stylize.halftone",
            "Halftone",
            vec![
                p("dotSize", "Dot Size", num(8.0), slider(1.0, 500.0, 2.0, 64.0, 1)),
                p("angle", "Screen Angle", num(45.0), ParamUi::Angle),
                p("shape", "Dot Shape", Value::Enum(0), popup(&["Circle", "Line", "Square"])),
                p("colorMode", "Color Mode", Value::Enum(0), popup(&["Ink on Paper", "Color Channels"])),
                p("inkColor", "Ink Color", col(0.0, 0.0, 0.0), ParamUi::Color),
                p("paperColor", "Paper Color", col(1.0, 1.0, 1.0), ParamUi::Color),
                p("blend", "Blend With Original", num(0.0), pct()),
            ],
            halftone,
        ),
        spec(
            "ec.stylize.pixelsort",
            "Pixel Sort",
            vec![
                p("direction", "Direction", Value::Enum(0), popup(&["Horizontal", "Vertical"])),
                p("sortBy", "Sort By", Value::Enum(0), popup(&["Brightness", "Hue", "Saturation"])),
                p("lowerThreshold", "Lower Threshold", num(25.0), pct()),
                p("upperThreshold", "Upper Threshold", num(80.0), pct()),
                p("maxLength", "Maximum Span Length", num(0.0), slider(0.0, 10000.0, 0.0, 1000.0, 0)),
                p("reverse", "Reverse Order", Value::Bool(false), ParamUi::Checkbox),
            ],
            pixel_sort,
        ),
        spec(
            "ec.stylize.dither",
            "Dither",
            vec![
                p("method", "Method", Value::Enum(1), popup(&["Bayer 2×2", "Bayer 4×4", "Bayer 8×8", "Floyd–Steinberg"])),
                p("levels", "Levels", num(2.0), slider(2.0, 256.0, 2.0, 16.0, 0)),
                p("color", "Color", Value::Enum(0), popup(&["Color", "Monochrome"])),
                p("pixelSize", "Pixel Size", num(1.0), slider(1.0, 100.0, 1.0, 16.0, 0)),
                p("blend", "Blend With Original", num(0.0), pct()),
            ],
            dither,
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EffectEnv, run_fx};
    use effectcraft_raster::Image;

    /// A horizontal grey ramp with a red square in the middle, fully opaque.
    fn scene(w: u32, h: u32) -> Image {
        let mut img = Image::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let v = x as f32 / (w - 1) as f32;
                let red = (w / 3..2 * w / 3).contains(&x) && (h / 3..2 * h / 3).contains(&y);
                img.set(x, y, if red { [0.9, 0.1, 0.1, 1.0] } else { [v, v, v, 1.0] });
            }
        }
        img
    }

    /// Effect `id` with `vals` over its defaults, at time 0.
    fn run(id: &str, vals: &[(&str, Value)], img: Image) -> Buf {
        run_fx(id, vals, img, 0.0, EffectEnv::default())
    }

    /// The pixel at layer position (x, y) of a (possibly grown) result.
    fn at(b: &Buf, x: i64, y: i64) -> Px {
        b.img.get(x + b.offset[0] as i64, y + b.offset[1] as i64)
    }

    /// The same input renders the same pixels, and the defaults change the image.
    #[test]
    fn new_effects_are_deterministic() {
        for id in ["ec.stylize.rgbsplit", "ec.stylize.halftone", "ec.stylize.pixelsort", "ec.stylize.dither"] {
            let a = run(id, &[], scene(40, 24));
            let b = run(id, &[], scene(40, 24));
            assert_eq!(a.img, b.img, "{id}");
            assert_ne!(a.img, scene(40, 24), "{id} changes the image with its defaults");
        }
        let fs = [("method", Value::Enum(3))];
        assert_eq!(run("ec.stylize.dither", &fs, scene(40, 24)).img, run("ec.stylize.dither", &fs, scene(40, 24)).img);
    }

    /// Linear moves red and blue opposite ways along Direction; Radial along the radius.
    #[test]
    fn rgb_split_moves_red_and_blue_apart() {
        // A white dot: red moves 4 px right (Direction 90°), blue 4 px left, green stays.
        let mut dot = Image::new(21, 9);
        dot.set(10, 4, [1.0; 4]);
        let out = run("ec.stylize.rgbsplit", &[("amount", num(4.0))], dot.clone());
        assert_eq!(out.offset, [5.0, 5.0], "the layer grows by the shift");
        assert_eq!(at(&out, 14, 4), [1.0, 0.0, 0.0, 1.0]);
        assert_eq!(at(&out, 6, 4), [0.0, 0.0, 1.0, 1.0]);
        assert_eq!(at(&out, 10, 4), [0.0, 1.0, 0.0, 1.0]);
        // Premultiplied colour never exceeds alpha.
        assert!(out.img.data.iter().all(|p| p[..3].iter().all(|c| *c <= p[3] + 1e-6)));
        // Radial: a dot at the centre doesn't move; one off-centre splits along the radius.
        let mut two = Image::new(41, 41);
        two.set(20, 20, [1.0; 4]);
        two.set(36, 20, [1.0; 4]);
        let out = run("ec.stylize.rgbsplit", &[("type", Value::Enum(1)), ("amount", num(10.0))], two);
        assert_eq!(at(&out, 20, 20), [1.0; 4]);
        assert!(at(&out, 36, 20)[1] > 0.99 && at(&out, 36, 20)[0] < 0.01);
        let red_x = (36..48).max_by(|a, b| at(&out, *a, 20)[0].total_cmp(&at(&out, *b, 20)[0])).unwrap();
        assert!(red_x > 36, "red moves outwards: {red_x}");
        // No shift: the image is unchanged.
        assert_eq!(run("ec.stylize.rgbsplit", &[("amount", num(0.0))], scene(10, 6)).img, scene(10, 6));
    }

    #[test]
    fn rgb_split_pads_for_the_heaviest_channel() {
        // Red Shift 200%: red moves 2 × 4 = 8 px, past a pad sized by Amount alone.
        let mut dot = Image::new(5, 5);
        dot.set(4, 2, [1.0; 4]);
        let out = run("ec.stylize.rgbsplit", &[("amount", num(4.0)), ("red", num(200.0))], dot);
        assert_eq!(out.offset, [9.0, 9.0], "the layer grows by the largest shift");
        assert_eq!(at(&out, 12, 2), [1.0, 0.0, 0.0, 1.0], "red isn't clipped at the edge");
        // NaN weights count as no shift.
        let out = run("ec.stylize.rgbsplit", &[("amount", num(4.0)), ("red", num(f64::NAN)), ("blue", num(f64::NAN))], scene(10, 6));
        assert!(out.img.data.iter().all(|p| p.iter().all(|c| c.is_finite())));
    }

    /// Darker areas get more ink, in each dot shape and colour mode.
    #[test]
    fn halftone_dots_follow_the_tone() {
        let ink = |v: f32| {
            let out = run("ec.stylize.halftone", &[("dotSize", num(8.0))], Image::filled(64, 64, [v, v, v, 1.0]));
            // Fraction of ink (black) across the image.
            out.img.data.iter().map(|p| 1.0 - p[0]).sum::<f32>() / out.img.data.len() as f32
        };
        let (white, mid, dark, black) = (ink(1.0), ink(0.5), ink(0.2), ink(0.0));
        assert!(white < 1e-3, "{white}");
        assert!(black > 0.99, "{black}");
        assert!(mid > 0.3 && mid < 0.7, "{mid}");
        assert!(dark > mid, "{dark} {mid}");
        // Ink and paper colours, and the dots are made only of them.
        let vals = [("inkColor", col(0.0, 0.0, 1.0)), ("paperColor", col(1.0, 1.0, 0.0)), ("angle", num(0.0))];
        let out = run("ec.stylize.halftone", &vals, Image::filled(32, 32, [0.5, 0.5, 0.5, 1.0]));
        assert!(out.img.data.iter().all(|p| (p[0] - p[1]).abs() < 1e-6 && (p[0] + p[2] - 1.0).abs() < 1e-5));
        // Alpha is kept; Color Channels gives light dots of each channel.
        let mut half = Image::filled(16, 16, [0.0; 4]);
        half.set(3, 3, [0.5, 0.0, 0.0, 0.5]);
        let out = run("ec.stylize.halftone", &[("colorMode", Value::Enum(1))], half.clone());
        assert!(out.img.data.iter().zip(&half.data).all(|(o, i)| o[3] == i[3]));
        for shape in 0..3 {
            let out = run("ec.stylize.halftone", &[("shape", Value::Enum(shape)), ("dotSize", num(6.0))], scene(30, 20));
            assert!(out.img.data.iter().all(|p| p.iter().all(|c| c.is_finite() && *c >= 0.0 && *c <= 1.0 + 1e-5)), "shape {shape}");
        }
    }

    /// Only spans within the thresholds are sorted, ascending or reversed, at most Maximum Span
    /// Length long; pixels move but never change.
    #[test]
    fn pixel_sort_sorts_spans_within_the_thresholds() {
        // One row: dark, then mid tones out of order, then white.
        let vals = [0.0f32, 0.6, 0.3, 0.5, 0.4, 1.0];
        let mut row = Image::new(vals.len() as u32, 1);
        for (x, v) in vals.iter().enumerate() {
            row.set(x as u32, 0, [*v, *v, *v, 1.0]);
        }
        let read = |b: &Buf| b.img.data.iter().map(|p| (p[0] * 10.0).round() / 10.0).collect::<Vec<_>>();
        let out = run("ec.stylize.pixelsort", &[], row.clone());
        assert_eq!(read(&out), [0.0, 0.3, 0.4, 0.5, 0.6, 1.0], "the 25–80 % span is sorted, the ends stay");
        let out = run("ec.stylize.pixelsort", &[("reverse", Value::Bool(true))], row.clone());
        assert_eq!(read(&out), [0.0, 0.6, 0.5, 0.4, 0.3, 1.0]);
        let out = run("ec.stylize.pixelsort", &[("maxLength", num(2.0))], row.clone());
        assert_eq!(read(&out), [0.0, 0.3, 0.6, 0.4, 0.5, 1.0], "spans of at most two pixels");
        // Vertical sorts columns; the pixels are only moved, never changed.
        let img = scene(12, 9);
        let out = run("ec.stylize.pixelsort", &[("direction", Value::Enum(1)), ("lowerThreshold", num(0.0)), ("upperThreshold", num(100.0))], img.clone());
        for x in 0..12 {
            let col = |i: &Image| (0..9).map(|y| luminance(i.get(x, y)[0], i.get(x, y)[1], i.get(x, y)[2])).collect::<Vec<_>>();
            let (mut before, after) = (col(&img), col(&out.img));
            assert!(after.windows(2).all(|w| w[0] <= w[1]), "column {x} sorted");
            before.sort_by(f32::total_cmp);
            assert_eq!(before, after);
        }
        // Thresholds swapped are the same range.
        let a = run("ec.stylize.pixelsort", &[("lowerThreshold", num(80.0)), ("upperThreshold", num(25.0))], row);
        assert_eq!(read(&a), [0.0, 0.3, 0.4, 0.5, 0.6, 1.0]);
    }

    /// A long run sorted in short spans: each span used to count the whole remaining run, which
    /// was quadratic. The keys taken are counted, so a regression fails here deterministically
    /// (not by running long).
    #[test]
    fn pixel_sort_short_spans_of_a_long_run_are_linear() {
        let n = 20_000u32;
        let mut row = Image::new(n, 1);
        for x in 0..n {
            let v = 0.3 + 0.4 * ((x * 7919) % 1000) as f32 / 1000.0;
            row.set(x, 0, [v, v, v, 1.0]);
        }
        for max in [1usize, 3, 0] {
            let calls = std::cell::Cell::new(0usize);
            let key = |p: Px| {
                calls.set(calls.get() + 1);
                Some(p[0])
            };
            let mut line = row.data.clone();
            sort_spans(&mut line, max, false, &key);
            // The quadratic scan took about n² / (2 max) keys: 2 × 10⁸ for one-pixel spans.
            assert!(calls.get() <= 4 * n as usize, "max {max}: {} keys for {n} pixels", calls.get());
            match max {
                1 => assert_eq!(line, row.data, "one-pixel spans leave the row as it is"),
                0 => assert!(line.windows(2).all(|w| w[0][0] <= w[1][0]), "one span: the whole row sorted"),
                _ => assert!(line.chunks(max).all(|s| s.windows(2).all(|w| w[0][0] <= w[1][0])), "{max}-pixel spans sorted"),
            }
        }
    }

    /// Every method keeps a flat grey's average brightness with two levels.
    #[test]
    fn dither_keeps_the_average_with_two_levels() {
        let grey = Image::filled(32, 32, [0.3, 0.3, 0.3, 1.0]);
        for method in 0..4 {
            let out = run("ec.stylize.dither", &[("method", Value::Enum(method))], grey.clone());
            assert!(out.img.data.iter().all(|p| p[0] == 0.0 || p[0] == 1.0), "method {method}: two levels only");
            let mean = out.img.data.iter().map(|p| p[0]).sum::<f32>() / out.img.data.len() as f32;
            // (a 2 × 2 pattern has only five tones: 0, ¼, ½, ¾, 1)
            let tolerance = if method == 0 { 0.13 } else { 0.05 };
            assert!((mean - 0.3).abs() < tolerance, "method {method}: mean {mean}");
        }
        // Monochrome: grey levels; Pixel Size: cells of the same colour.
        let out = run("ec.stylize.dither", &[("color", Value::Enum(1)), ("pixelSize", num(4.0))], scene(32, 16));
        for p in &out.img.data {
            assert!(p[0] == p[1] && p[1] == p[2]);
        }
        for y in 0..16 {
            for x in 0..32 {
                assert_eq!(out.img.get(x, y), out.img.get(x / 4 * 4, y / 4 * 4), "({x}, {y})");
            }
        }
        // Many levels barely change the image; alpha is kept.
        let mut img = scene(24, 12);
        img.data[0] = [0.25, 0.25, 0.25, 0.5];
        let out = run("ec.stylize.dither", &[("levels", num(256.0))], img.clone());
        assert!(out.img.data.iter().zip(&img.data).all(|(o, i)| (o[0] - i[0]).abs() < 0.01 && o[3] == i[3]));
        assert_eq!(bayer(2, 0, 0), -0.375);
        assert_eq!(bayer(2, 1, 1), -0.125);
        assert_eq!(bayer(8, 7, 7), (21.0 + 0.5) / 64.0 - 0.5);
    }

    /// Hostile values: huge, negative and non-finite parameters render without panicking and
    /// keep alpha in range.
    #[test]
    fn extreme_parameters_are_safe() {
        let cases: [(&str, &[(&str, Value)]); 8] = [
            ("ec.stylize.rgbsplit", &[("amount", num(1e9)), ("direction", num(f64::NAN))]),
            ("ec.stylize.rgbsplit", &[("type", Value::Enum(1)), ("amount", num(-1e9)), ("center", Value::Vec2([1e12, -1e12]))]),
            ("ec.stylize.halftone", &[("dotSize", num(-5.0)), ("angle", num(1e18))]),
            ("ec.stylize.halftone", &[("dotSize", num(1e9)), ("shape", Value::Enum(99))]),
            ("ec.stylize.pixelsort", &[("maxLength", num(-1e9)), ("lowerThreshold", num(f64::NAN))]),
            ("ec.stylize.pixelsort", &[("sortBy", Value::Enum(7)), ("direction", Value::Enum(1)), ("maxLength", num(1e12))]),
            ("ec.stylize.dither", &[("levels", num(f64::NAN)), ("pixelSize", num(1e9)), ("method", Value::Enum(3))]),
            ("ec.stylize.dither", &[("levels", num(-3.0)), ("pixelSize", num(-1.0)), ("method", Value::Enum(42))]),
        ];
        for (id, vals) in cases {
            let out = run(id, vals, scene(20, 12));
            assert!(!out.img.is_empty(), "{id} {vals:?}");
            assert!(out.img.data.iter().all(|p| p[3].is_finite() && (0.0..=1.0 + 1e-4).contains(&p[3])), "{id} {vals:?}");
        }
    }
}

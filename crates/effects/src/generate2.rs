//! Generate effects, batch 2: Cell Pattern (Worley), Ellipse, Lens Flare, Beam, Radio Waves,
//! Advanced Lightning, CC Light Rays, CC Light Burst 2.5, CC Light Sweep.
//!
//! All designs are original procedural renderings written from the public descriptions of what
//! each effect produces.

use std::f64::consts::PI;

use effectcraft_color::luminance;
use effectcraft_keyframe::Value;
use effectcraft_project::ParamUi;
use effectcraft_raster::Px;
use rayon::prelude::*;

use crate::util::{Plane, Rng, gauss_plane, hash1, layer_rect, lerp3, map_xy, smoothstep, unpremul};
use crate::{Buf, EffectCtx, EffectSpec, col, num, p, popup, slider};

fn spec(id: &'static str, name: &'static str, params: Vec<crate::ParamSpec>, render: crate::RenderFn) -> EffectSpec {
    EffectSpec { id, name, category: "Generate", params, render, gpu: false, float: true }
}

fn pt(x: f64, y: f64) -> Value {
    Value::Vec2([x, y])
}

fn pct() -> ParamUi {
    slider(0.0, 100.0, 0.0, 100.0, 1)
}

/// Premultiplied "over": `src` on top of `dst`.
#[inline]
fn over(dst: Px, src: Px) -> Px {
    let k = 1.0 - src[3];
    [src[0] + dst[0] * k, src[1] + dst[1] * k, src[2] + dst[2] * k, src[3] + dst[3] * k]
}

/// Add light (straight rgb `l`, coverage `la`) onto a premultiplied pixel.
#[inline]
fn add_light(px: Px, l: [f32; 3], la: f32) -> Px {
    let la = la.clamp(0.0, 1.0);
    [px[0] + l[0], px[1] + l[1], px[2] + l[2], (px[3] + la * (1.0 - px[3])).clamp(0.0, 1.0)]
}

// ---------------------------------------------------------------- Cell Pattern

/// Cell Pattern options. The HQ variants share the shaping of their standard counterparts but
/// search a wider neighbourhood of cells (no artefacts at high Disperse) and anti-alias the
/// cell edges (4×4 samples per pixel).
const CELL_PATTERNS: [&str; 12] = [
    "Bubbles",
    "Crystals",
    "Plates",
    "Static Plates",
    "Crystallize",
    "Pillow",
    "Crystals HQ",
    "Plates HQ",
    "Static Plates HQ",
    "Crystallize HQ",
    "Mixed Crystals HQ",
    "Tubular",
];

/// Shaping used for Cell Pattern option `i` (see `cell_value`).
pub fn pattern_kind(i: u32) -> u32 {
    match i {
        0..=5 => i,
        6 => 1,
        7 => 2,
        8 => 3,
        9 => 4,
        10 => 6,
        _ => 8,
    }
}

/// Worley distances for point (u, v) in cell units: (F1, F2, hash of the nearest cell).
/// `tile` repeats the cell layout every (columns, rows) cells.
fn worley_r(u: f64, v: f64, disperse: f64, evo: f64, seed: u32, tile: Option<(i64, i64)>, reach: i64) -> (f64, f64, f32) {
    let (cx, cy) = (u.floor() as i64, v.floor() as i64);
    let mut f1 = f64::INFINITY;
    let mut f2 = f64::INFINITY;
    let mut id = 0.0f32;
    for j in -reach..=reach {
        for i in -reach..=reach {
            let (gx, gy) = (cx + i, cy + j);
            let (tx, ty) = match tile {
                Some((nx, ny)) => (gx.rem_euclid(nx), gy.rem_euclid(ny)),
                None => (gx, gy),
            };
            let (hx, hy) = (tx as i32 as u32, ty as i32 as u32);
            let h1 = hash1(hx, hy, seed) as f64;
            let h2 = hash1(hx, hy, seed.wrapping_add(1)) as f64;
            let h3 = hash1(hx, hy, seed.wrapping_add(2)) as f64;
            let ang = 2.0 * PI * (h3 + evo);
            let px = gx as f64 + 0.5 + (h1 - 0.5) * disperse + 0.15 * disperse * ang.cos();
            let py = gy as f64 + 0.5 + (h2 - 0.5) * disperse + 0.15 * disperse * ang.sin();
            let d = ((px - u).powi(2) + (py - v).powi(2)).sqrt();
            if d < f1 {
                f2 = f1;
                f1 = d;
                id = hash1(hx, hy, seed.wrapping_add(3));
            } else if d < f2 {
                f2 = d;
            }
        }
    }
    (f1, f2, id)
}

fn cell_value(pattern: u32, f1: f64, f2: f64, id: f32) -> f32 {
    let (f1, f2) = (f1 as f32, f2 as f32);
    let e = f2 - f1;
    match pattern {
        0 => (1.0 - f1 * 1.3).clamp(0.0, 1.0),               // Bubbles
        1 => id * 0.8 + 0.2 * (1.0 - f1).clamp(0.0, 1.0),    // Crystals
        2 => (e * 2.0).clamp(0.0, 1.0),                      // Plates
        3 => id * smoothstep(0.0, 0.06, e),                  // Static Plates
        4 => id,                                             // Crystallize
        5 => (1.0 - f1 * f1 * 2.0).clamp(0.0, 1.0),          // Pillow
        6 => id * (e * 3.0).clamp(0.0, 1.0),                 // Mixed Crystals
        _ => 1.0 - ((e - 0.15).abs() * 5.0).clamp(0.0, 1.0), // Tubular
    }
}

fn overflow(v: f32, mode: u32) -> f32 {
    match mode {
        1 => 0.5 + 0.5 * ((v - 0.5) * 2.0).tanh(),
        2 => {
            let t = v.rem_euclid(2.0);
            if t > 1.0 { 2.0 - t } else { t }
        }
        _ => v.clamp(0.0, 1.0),
    }
}

fn cell_pattern(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let pattern = pattern_kind(ctx.params.e("cellPattern"));
    let hq = (6..=10).contains(&ctx.params.e("cellPattern"));
    let invert = ctx.params.b("invert");
    let contrast = ctx.params.f("contrast") as f32 / 100.0;
    let ov = ctx.params.e("overflow");
    let disperse = ctx.params.f("disperse").clamp(0.0, 1.5);
    let size = (ctx.params.f("size") * b.scale).max(1.0);
    let off = b.to_px(ctx.params.v2("offset"));
    // Static Plates keep their shades still while the cells move.
    let mut evo = ctx.params.f("evolution") / 360.0;
    // Cycle Evolution loops the evolution every `cycle` revolutions.
    if ctx.params.b("evolutionOptions/cycleEvolution") {
        evo = evo.rem_euclid(ctx.params.f("evolutionOptions/cycle").round().max(1.0));
    }
    let seed = (ctx.params.f("evolutionOptions/randomSeed") as i64 as u32).wrapping_mul(7919) ^ 0xce11;
    let tile = ctx
        .params
        .b("tilingOptions/enableTiling")
        .then(|| (ctx.params.f("tilingOptions/cellsHorizontal").round().max(1.0) as i64, ctx.params.f("tilingOptions/cellsVertical").round().max(1.0) as i64));
    let subs: Vec<f64> = if hq { (0..4).map(|k| (k as f64 + 0.5) / 4.0).collect() } else { vec![0.5] };
    let reach = if hq { 2 } else { 1 };
    map_xy(&mut b.img, |x, y, _| {
        let mut val = 0.0;
        for sy in &subs {
            for sx in &subs {
                let u = (x as f64 + sx - off.0) / size;
                let v = (y as f64 + sy - off.1) / size;
                let (f1, f2, id) = worley_r(u, v, disperse, evo, seed, tile, reach);
                val += cell_value(pattern, f1, f2, id);
            }
        }
        let mut val = val / (subs.len() * subs.len()) as f32;
        val = overflow((val - 0.5) * contrast + 0.5, ov);
        if invert {
            val = 1.0 - val;
        }
        [val, val, val, 1.0]
    });
    b
}

// ---------------------------------------------------------------- Ellipse

fn ellipse(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let c = b.to_px(ctx.params.v2("center"));
    let rx = (ctx.params.f("width") * b.scale * 0.5).max(0.5);
    let ry = (ctx.params.f("height") * b.scale * 0.5).max(0.5);
    let half = (ctx.params.f("thickness") * b.scale * 0.5).max(0.25);
    let soft = (ctx.params.f("softness") / 100.0).clamp(0.0, 1.0);
    let inside = ctx.params.color("insideColor");
    let outside = ctx.params.color("outsideColor");
    let composite = ctx.params.b("compositeOnOriginal");
    map_xy(&mut b.img, |x, y, px| {
        let (dx, dy) = (x as f64 + 0.5 - c.0, y as f64 + 0.5 - c.1);
        let q = ((dx / rx).powi(2) + (dy / ry).powi(2)).sqrt();
        let dist = if q < 1e-9 {
            rx.min(ry)
        } else {
            let gx = dx / (rx * rx) / q;
            let gy = dy / (ry * ry) / q;
            ((q - 1.0) / (gx * gx + gy * gy).sqrt().max(1e-9)).abs()
        };
        let t = (dist / half) as f32;
        let a = if soft > 1e-3 { 1.0 - smoothstep(1.0 - soft as f32, 1.0, t) } else { (half - dist + 0.5).clamp(0.0, 1.0) as f32 };
        let cc = lerp3([inside[0], inside[1], inside[2]], [outside[0], outside[1], outside[2]], smoothstep(0.0, 1.0, t));
        let g = [cc[0] * a, cc[1] * a, cc[2] * a, a];
        if composite { over(px, g) } else { g }
    });
    b
}

// ---------------------------------------------------------------- Lens Flare

/// A Lens Flare ghost: position along the flare axis, radius (fraction of the diagonal), colour,
/// strength, ring or disc.
pub struct Ghost {
    pub t: f64,
    pub r: f64,
    pub c: [f32; 3],
    pub k: f32,
    pub ring: bool,
}

/// The ghosts of Lens Type `lens`.
pub fn flare_ghosts(lens: u32) -> Vec<Ghost> {
    let g = |t, r, c: [f32; 3], k, ring| Ghost { t, r, c, k, ring };
    match lens {
        1 => vec![
            g(0.45, 0.03, [0.4, 0.7, 1.0], 0.20, false),
            g(0.9, 0.06, [0.5, 1.0, 0.6], 0.12, false),
            g(1.3, 0.12, [1.0, 0.6, 0.3], 0.10, true),
            g(1.6, 0.04, [0.8, 0.5, 1.0], 0.15, false),
        ],
        2 => vec![g(0.6, 0.02, [1.0, 0.8, 0.5], 0.25, false), g(1.1, 0.05, [0.6, 0.6, 1.0], 0.12, false), g(1.4, 0.09, [0.4, 0.9, 1.0], 0.08, true)],
        _ => vec![
            g(0.25, 0.02, [1.0, 0.9, 0.6], 0.25, false),
            g(0.5, 0.04, [0.3, 1.0, 0.5], 0.15, false),
            g(0.75, 0.025, [0.5, 0.6, 1.0], 0.2, false),
            g(1.15, 0.07, [1.0, 0.5, 0.3], 0.10, false),
            g(1.45, 0.1, [0.6, 0.4, 1.0], 0.08, true),
            g(1.8, 0.05, [0.4, 0.9, 1.0], 0.15, false),
            g(2.1, 0.14, [1.0, 0.8, 0.4], 0.06, true),
        ],
    }
}

fn lens_flare(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let f = b.to_px(ctx.params.v2("flareCenter"));
    let bright = (ctx.params.f("flareBrightness") / 100.0) as f32;
    let lens = ctx.params.e("lensType");
    let blend = (ctx.params.f("blendWithOriginal") / 100.0) as f32;
    let (x0, y0, w, h) = layer_rect(ctx, &b);
    let c = (x0 + w * 0.5, y0 + h * 0.5);
    let diag = (w * w + h * h).sqrt().max(1.0);
    let gs = flare_ghosts(lens);
    let (core_k, rays, halo_r) = match lens {
        1 => (0.035, 6.0, 0.18),
        2 => (0.02, 12.0, 0.3),
        _ => (0.03, 8.0, 0.25),
    };
    map_xy(&mut b.img, |x, y, px| {
        let (dx, dy) = (x as f64 + 0.5 - f.0, y as f64 + 0.5 - f.1);
        let r = (dx * dx + dy * dy).sqrt() / diag;
        let th = dy.atan2(dx);
        let core = 1.5 * (-(r / core_k).powi(2)).exp() + 0.25 * (-r / 0.12).exp();
        let streak = (rays * 0.5 * th).cos().abs().powi(40) * (-r / 0.3).exp() * 0.35;
        let halo = (-((r - halo_r) / 0.012).powi(2)).exp() * 0.12;
        let base = (core + streak) as f32;
        let mut l = [base, base * 0.95, base * 0.85];
        let hal = halo as f32;
        l[0] += hal * 0.6;
        l[1] += hal * 0.8;
        l[2] += hal;
        for g in &gs {
            let gx = f.0 + (c.0 - f.0) * g.t;
            let gy = f.1 + (c.1 - f.1) * g.t;
            let d = ((x as f64 + 0.5 - gx).powi(2) + (y as f64 + 0.5 - gy).powi(2)).sqrt() / diag;
            let v = if g.ring { (-((d - g.r) / (g.r * 0.12)).powi(2)).exp() as f32 } else { smoothstep(g.r as f32, g.r as f32 * 0.8, d as f32) };
            for i in 0..3 {
                l[i] += g.c[i] * g.k * v;
            }
        }
        let l = l.map(|v| v * bright);
        let la = l[0].max(l[1]).max(l[2]).min(1.0);
        let o = add_light(px, l, la);
        crate::util::lerp4(o, px, blend)
    });
    b
}

// ---------------------------------------------------------------- Beam

fn beam(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let s = b.to_px(ctx.params.v2("startPoint"));
    let e = b.to_px(ctx.params.v2("endPoint"));
    let len = (ctx.params.f("length") / 100.0).clamp(0.0, 1.0);
    let time = (ctx.params.f("time") / 100.0).clamp(0.0, 1.0);
    let st = ctx.params.f("startThickness") * b.scale;
    let et = ctx.params.f("endThickness") * b.scale;
    let soft = (ctx.params.f("softness") / 100.0).clamp(0.0, 1.0) as f32;
    let inside = ctx.params.color("insideColor");
    let outside = ctx.params.color("outsideColor");
    let composite = ctx.params.b("compositeOnOriginal");
    // 3D Perspective: thickness follows the beam's position between the two points (it
    // grows / shrinks as it travels); off: the beam's own tail and head take the starting and
    // ending thickness.
    let persp = ctx.params.b("perspective3d");
    let t0 = time * (1.0 - len);
    let t1 = t0 + len;
    let (dx, dy) = (e.0 - s.0, e.1 - s.1);
    let l2 = (dx * dx + dy * dy).max(1e-9);
    map_xy(&mut b.img, |x, y, px| {
        let (vx, vy) = (x as f64 + 0.5 - s.0, y as f64 + 0.5 - s.1);
        let tc = ((vx * dx + vy * dy) / l2).clamp(t0, t1);
        let (qx, qy) = (s.0 + dx * tc, s.1 + dy * tc);
        let dist = ((x as f64 + 0.5 - qx).powi(2) + (y as f64 + 0.5 - qy).powi(2)).sqrt();
        let tk = if persp || t1 - t0 < 1e-9 { tc } else { (tc - t0) / (t1 - t0) };
        let half = ((st + (et - st) * tk) * 0.5).max(0.25);
        let tt = (dist / half) as f32;
        let a = if len <= 0.0 {
            0.0
        } else if soft > 1e-3 {
            1.0 - smoothstep(1.0 - soft, 1.0, tt)
        } else {
            (half - dist + 0.5).clamp(0.0, 1.0) as f32
        };
        let cc = lerp3([inside[0], inside[1], inside[2]], [outside[0], outside[1], outside[2]], smoothstep(0.0, 1.0, tt));
        let g = [cc[0] * a, cc[1] * a, cc[2] * a, a];
        if composite { over(px, g) } else { g }
    });
    b
}

// ---------------------------------------------------------------- Radio Waves

/// Distance from `p` (relative to the wave centre) to a closed outline `pts`, measured along
/// the ray from the centre and corrected to the edge normal.
fn outline_dist(pts: &[(f64, f64)], dx: f64, dy: f64) -> f64 {
    let r = (dx * dx + dy * dy).sqrt();
    if r < 1e-9 {
        return pts.iter().map(|p| (p.0 * p.0 + p.1 * p.1).sqrt()).fold(f64::INFINITY, f64::min);
    }
    let dir = (dx / r, dy / r);
    let n = pts.len();
    let mut best = f64::INFINITY;
    for k in 0..n {
        let p0 = pts[k];
        let p1 = pts[(k + 1) % n];
        let e = (p1.0 - p0.0, p1.1 - p0.1);
        let den = dir.0 * e.1 - dir.1 * e.0;
        if den.abs() < 1e-12 {
            continue;
        }
        let t = (p0.0 * e.1 - p0.1 * e.0) / den;
        // The ray must cross this edge between its endpoints.
        let q = (dir.0 * t - p0.0, dir.1 * t - p0.1);
        let el2 = (e.0 * e.0 + e.1 * e.1).max(1e-12);
        let s = (q.0 * e.0 + q.1 * e.1) / el2;
        if t <= 0.0 || !(-1e-9..=1.0 + 1e-9).contains(&s) {
            continue;
        }
        let d = (r - t).abs() * den.abs() / el2.sqrt();
        best = best.min(d);
    }
    best
}

/// Radio Waves' Wave Type options.
const RW_TYPES: [&str; 3] = ["Polygon", "Image Contours", "Mask"];
/// Radio Waves' Stroke Profile options.
const RW_PROFILES: [&str; 7] = ["Square", "Taper Front", "Taper Back", "Taper Both", "Sine", "Sawtooth Out", "Sawtooth In"];
/// Image Contour's Value Channel options.
const RW_CHANNELS: [&str; 8] = ["Red", "Green", "Blue", "Alpha", "Luminance", "Hue", "Lightness", "Saturation"];

/// Stroke Profile weight across the stroke: `u` from −1 (inner edge) to 1 (outer edge).
fn rw_profile(kind: u32, u: f64) -> f64 {
    let u = u.clamp(-1.0, 1.0);
    match kind {
        1 => (1.0 - u) * 0.5,
        2 => (1.0 + u) * 0.5,
        3 => 1.0 - u.abs(),
        4 => (u * PI * 0.5).cos(),
        5 if u < 0.0 => 1.0 + u,
        6 if u > 0.0 => 1.0 - u,
        _ => 1.0,
    }
}

/// Fold a coordinate into `[lo, hi]` by bouncing off the ends (Reflection).
fn reflect(v: f64, lo: f64, hi: f64) -> f64 {
    let span = hi - lo;
    if span <= 0.0 {
        return lo;
    }
    let t = (v - lo).rem_euclid(2.0 * span);
    lo + if t > span { 2.0 * span - t } else { t }
}

/// The source shape of a contour wave: signed distance (buffer px, negative inside) on the
/// buffer grid, and the point of the shape that sits on the Producer Point.
pub struct Contour {
    pub sdf: Plane,
    pub anchor: (f64, f64),
}

/// Image Contours: the region of the source layer whose Value Channel passes Value Threshold.
fn image_contour(ctx: &EffectCtx, b: &Buf) -> Option<Contour> {
    let pr = ctx.params;
    let lp = ctx.layer_param("imageContour/sourceLayer", true)?;
    let img = crate::util::fit_layer(ctx, b, &lp, false);
    let (w, h) = (img.width as usize, img.height as usize);
    let ch = pr.e("imageContour/valueChannel");
    let mut v = Plane::from_image(&img, |px| {
        let (c, a) = unpremul(px);
        let (hh, s, l) = effectcraft_color::rgb_to_hsl(c[0], c[1], c[2]);
        match ch {
            0 => c[0] * a,
            1 => c[1] * a,
            2 => c[2] * a,
            3 => a,
            5 => hh,
            6 => l * a,
            7 => s,
            _ => luminance(c[0], c[1], c[2]) * a,
        }
    });
    let blur = pr.f("imageContour/preBlur").max(0.0) * b.scale;
    if blur > 0.0 {
        v = gauss_plane(&v, blur * 0.5, blur * 0.5);
    }
    let thr = (pr.f("imageContour/valueThreshold") / 255.0) as f32;
    let invert = pr.b("imageContour/invertInput");
    let mut inside: Vec<bool> = v.data.iter().map(|&x| (x >= thr) != invert).collect();
    // Contour: 0 = every region; n = the n-th largest connected region.
    let pick = pr.f("imageContour/contour").round() as usize;
    if pick > 0 {
        let mut label = vec![0u32; w * h];
        let mut sizes = vec![0usize];
        for start in 0..w * h {
            if !inside[start] || label[start] != 0 {
                continue;
            }
            let id = sizes.len() as u32;
            let mut stack = vec![start];
            label[start] = id;
            let mut n = 0;
            while let Some(i) = stack.pop() {
                n += 1;
                let (x, y) = (i % w, i / w);
                let mut nb = |j: usize| {
                    if inside[j] && label[j] == 0 {
                        label[j] = id;
                        stack.push(j);
                    }
                };
                if x > 0 {
                    nb(i - 1);
                }
                if x + 1 < w {
                    nb(i + 1);
                }
                if y > 0 {
                    nb(i - w);
                }
                if y + 1 < h {
                    nb(i + w);
                }
            }
            sizes.push(n);
        }
        let mut order: Vec<usize> = (1..sizes.len()).collect();
        order.sort_by(|a, c| sizes[*c].cmp(&sizes[*a]).then(a.cmp(c)));
        let keep = order.get(pick - 1).copied().unwrap_or(usize::MAX) as u32;
        inside.iter_mut().zip(&label).for_each(|(v, l)| *v = *v && *l == keep);
    }
    if !inside.iter().any(|v| *v) {
        return None;
    }
    let mut sdf = crate::util::signed_distance(&inside, w, h);
    // Tolerance smooths the traced outline.
    let tol = pr.f("imageContour/tolerance").max(0.0) * b.scale;
    if tol > 0.0 {
        sdf = gauss_plane(&sdf, tol * 0.5, tol * 0.5);
    }
    Some(Contour { sdf, anchor: b.to_px(pr.v2("imageContour/sourceCenter")) })
}

/// Mask: the chosen mask of this layer.
fn mask_contour(ctx: &EffectCtx, b: &Buf) -> Option<Contour> {
    let idx = ctx.params.f("waveMask/mask").round() as usize;
    let (pts, _, inverted) = crate::generate3::mask_px(ctx, b, idx)?;
    let (w, h) = (b.img.width as usize, b.img.height as usize);
    let inside: Vec<bool> = (0..w * h).map(|i| crate::util::point_in_poly(&pts, (i % w) as f64 + 0.5, (i / w) as f64 + 0.5) != inverted).collect();
    if !inside.iter().any(|v| *v) {
        return None;
    }
    // Mask waves start where the mask is (anchored at the Producer Point: no shift).
    let anchor = b.to_px(ctx.params.v2("producerPoint"));
    Some(Contour { sdf: crate::util::signed_distance(&inside, w, h), anchor })
}

/// One live wave.
pub struct Wave {
    pub centre: (f64, f64),
    pub radius: f64,
    pub half_width: f64,
    pub rotation: f64,
    pub fade: f32,
    pub color: [f32; 4],
    pub opacity: f32,
    /// Polygon outline (unit shape rotated and scaled), or None for a circle / contour.
    pub outline: Option<Vec<(f64, f64)>>,
}

/// Radio Waves, shared with the GPU compositor (effectcraft-gpu `fx_gen2`): the live waves,
/// oldest first, and the contour they ripple from (Image Contours / Mask).
pub struct RadioPlan {
    pub waves: Vec<Wave>,
    pub contour: Option<Contour>,
    /// Anti-aliasing width (px) from Render Quality.
    pub aa: f64,
    /// Stroke Profile.
    pub profile: u32,
}

/// Radio Waves: waves are born at Frequency per second at the Producer Point and grow at
/// Expansion px/s for Lifespan seconds, moving at Velocity towards Direction (bouncing off the
/// layer edges with Reflection) and spinning. Polygon waves are regular polygons or stars;
/// Image Contours waves ripple outward from the outline of a source layer's thresholded
/// channel, Mask waves from a mask. With Parameters Are Set At Birth each wave keeps the values
/// it was born with; Each Frame applies the current values to every wave. Stroke draws each wave
/// with its Profile across the width.
/// [`RadioPlan`] of Radio Waves on `b`'s geometry (`None` = no contour: the layer passes
/// through).
pub fn radio_plan(ctx: &EffectCtx, b: &Buf) -> Option<RadioPlan> {
    let cur = ctx.params;
    let t = ctx.time.max(0.0);
    let freq = cur.f("waveMotion/frequency").max(0.01);
    let life_now = cur.f("waveMotion/lifespan").max(0.01);
    let birth = cur.e("parametersAreSetAt") == 0;
    let wave_type = cur.e("waveType");
    let quality = cur.get("renderQuality").map(Value::as_f64).unwrap_or(4.0).clamp(1.0, 10.0);
    let aa = 0.5 + 1.5 / quality;
    let profile = cur.e("waveStroke/profile");
    let reflection = cur.b("waveMotion/reflection");
    let (lx, ly, lw, lh) = layer_rect(ctx, b);
    let contour = match wave_type {
        1 => image_contour(ctx, b),
        2 => mask_contour(ctx, b),
        _ => None,
    };
    if wave_type != 0 && contour.is_none() {
        return None;
    }
    let k_hi = (t * freq).floor() as i64;
    let k_lo = (((t - life_now) * freq).floor() as i64 - 1).max(0).max(k_hi - 256);
    let mut cache: std::collections::HashMap<i64, crate::Params> = std::collections::HashMap::new();
    let waves: Vec<Wave> = (k_lo..=k_hi)
        .filter_map(|k| {
            let born = k as f64 / freq;
            let age = t - born;
            // Birth: the values the wave was born with (when the host can tell).
            let pr: &crate::Params = if birth {
                if let Some(h) = ctx.env.host
                    && let std::collections::hash_map::Entry::Vacant(e) = cache.entry(k)
                    && let Some(p) = h.params_at(born)
                {
                    e.insert(p);
                }
                cache.get(&k).unwrap_or(cur)
            } else {
                cur
            };
            let life = pr.f("waveMotion/lifespan").max(0.01);
            if !(0.0..life).contains(&age) {
                return None;
            }
            let c = b.to_px(pr.v2("producerPoint"));
            let fin = pr.f("waveStroke/fadeInTime").max(0.0);
            let fout = pr.f("waveStroke/fadeOutTime").max(0.0);
            let fi = if fin > 0.0 { (age / fin).min(1.0) } else { 1.0 };
            let fo = if fout > 0.0 { ((life - age) / fout).min(1.0) } else { 1.0 };
            let dirn = pr.f("waveMotion/direction").to_radians();
            let vel = pr.f("waveMotion/velocity") * b.scale;
            let mut centre = (c.0 + dirn.sin() * vel * age, c.1 - dirn.cos() * vel * age);
            if reflection {
                centre = (reflect(centre.0, lx, lx + lw), reflect(centre.1, ly, ly + lh));
            }
            let sw = pr.f("waveStroke/startWidth") * b.scale;
            let ew = pr.f("waveStroke/endWidth") * b.scale;
            let radius = pr.f("waveMotion/expansion") * b.scale * age;
            let rotation = pr.f("waveMotion/orientation").to_radians() + pr.f("waveMotion/spin").to_radians() * age;
            let outline = (wave_type == 0).then(|| {
                let sides = pr.f("polygon/sides").round().clamp(3.0, 64.0);
                let star = pr.b("polygon/star");
                let depth = pr.f("polygon/starDepth").clamp(-1.0, 1.0);
                if sides >= 64.0 && !star {
                    return None;
                }
                let n = sides as usize;
                let unit: Vec<(f64, f64)> = if star {
                    (0..2 * n).map(|i| (PI * i as f64 / n as f64 - PI / 2.0, if i % 2 == 0 { 1.0 } else { (1.0 + depth).max(0.0) })).collect()
                } else {
                    (0..n).map(|i| (2.0 * PI * i as f64 / n as f64 - PI / 2.0, 1.0)).collect()
                };
                Some(unit.iter().map(|&(a, k)| ((a + rotation).cos() * radius * k, (a + rotation).sin() * radius * k)).collect())
            });
            Some(Wave {
                centre,
                radius,
                half_width: ((sw + (ew - sw) * age / life) * 0.5).max(0.25),
                rotation,
                fade: (fi * fo) as f32,
                color: pr.color("waveStroke/color"),
                opacity: (pr.f("waveStroke/opacity") / 100.0) as f32,
                outline: outline.flatten(),
            })
        })
        .collect();
    Some(RadioPlan { waves, contour, aa, profile })
}

fn radio_waves(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let Some(RadioPlan { waves, contour, aa, profile }) = radio_plan(ctx, &b) else { return b };
    map_xy(&mut b.img, |x, y, px| {
        let mut out = px;
        // Older waves first, so newer ones sit on top.
        for wv in &waves {
            let (dx, dy) = (x as f64 + 0.5 - wv.centre.0, y as f64 + 0.5 - wv.centre.1);
            // Signed distance from the wave line (negative inside the wave).
            let d = match (&contour, &wv.outline) {
                (Some(ct), _) => {
                    // The contour placed with its anchor on the wave centre, rotated.
                    let (s, c) = (-(wv.rotation)).sin_cos();
                    let (rx, ry) = (dx * c - dy * s, dx * s + dy * c);
                    ct.sdf.sample(ct.anchor.0 + rx, ct.anchor.1 + ry) as f64 - wv.radius
                }
                (None, Some(o)) => {
                    let ud = outline_dist(o, dx, dy);
                    let poly: Vec<[f64; 2]> = o.iter().map(|p| [p.0, p.1]).collect();
                    if crate::util::point_in_poly(&poly, dx, dy) { -ud } else { ud }
                }
                (None, None) => (dx * dx + dy * dy).sqrt() - wv.radius,
            };
            let ad = d.abs();
            let cov = ((wv.half_width - ad) / aa + 0.5).clamp(0.0, 1.0);
            if cov <= 0.0 {
                continue;
            }
            let u = (d / wv.half_width).clamp(-1.0, 1.0);
            let a = (cov * rw_profile(profile, u)) as f32 * wv.fade * wv.opacity * wv.color[3];
            out = over(out, [wv.color[0] * a, wv.color[1] * a, wv.color[2] * a, a]);
        }
        out
    });
    b
}

// ---------------------------------------------------------------- Advanced Lightning

/// A bolt segment of Advanced Lightning: start, end (buffer px) and intensity.
pub type BoltSeg = ((f64, f64), (f64, f64), f32);
type Seg = BoltSeg;

/// Advanced Lightning's Lightning Type options.
const LIGHTNING_TYPES: [&str; 8] = ["Direction", "Strike", "Breaking", "Bouncy", "Omni", "Anywhere", "Vertical", "Two-Way Strike"];

/// Advanced Lightning's Fractal Type options.
const FRACTAL_TYPES: [&str; 3] = ["Linear", "Semi-Linear", "Spline"];

/// How bolts grow: the general and Expert Settings.
struct BoltCfg<'a> {
    turb: f64,
    fork: f64,
    decay: f32,
    /// The layer's alpha (buffer grid) and Alpha Obstacle (−10..10; positive avoids opaque
    /// areas, negative is drawn to them).
    alpha: Option<&'a Plane>,
    obstacle: f64,
    main_only: bool,
    /// Min Fork Distance (px): shorter stretches do not fork.
    min_fork: f64,
    /// Termination Threshold: branches fainter than this stop.
    term: f32,
    /// Zig-zag scale from Fractal Type (Linear 1, Semi-Linear 0.7, Spline 0.45 with an extra
    /// subdivision level).
    shape: f64,
    fork_strength: f32,
    fork_var: f32,
}

impl BoltCfg<'_> {
    fn alpha_at(&self, p: (f64, f64)) -> f32 {
        self.alpha.map_or(0.0, |a| a.sample(p.0, p.1))
    }
}

/// One branch's geometry and recursive growth state.
struct BoltBranch {
    start: (f64, f64),
    end: (f64, f64),
    depth: u32,
    intensity: f32,
    main: bool,
}

/// Segments and their matching main-core flags, kept in the same traversal order.
#[derive(Default)]
struct BoltOutput {
    segments: Vec<Seg>,
    mains: Vec<bool>,
}

/// Grow a branch by midpoint displacement, recursively appending its segments and forks.
fn bolt(cfg: &BoltCfg, branch: BoltBranch, rng: &mut Rng, output: &mut BoltOutput) {
    let BoltBranch { start: a, end: z, depth, intensity: inten, main } = branch;
    if output.segments.len() > 20_000 {
        return;
    }
    let (dx, dy) = (z.0 - a.0, z.1 - a.1);
    let len = (dx * dx + dy * dy).sqrt();
    if depth == 0 || len < 1.0 {
        output.segments.push((a, z, inten));
        output.mains.push(main);
        return;
    }
    let (nx, ny) = (-dy / len.max(1e-9), dx / len.max(1e-9));
    let mut off = rng.s() * len * 0.22 * cfg.turb * cfg.shape;
    // Alpha Obstacle: of the two mirror positions for the midpoint, prefer the one with less
    // (positive) or more (negative) alpha, the more strongly the larger the setting.
    if cfg.obstacle != 0.0 && cfg.alpha.is_some() && (main || !cfg.main_only) {
        let mid = ((a.0 + z.0) * 0.5, (a.1 + z.1) * 0.5);
        let p1 = (mid.0 + nx * off, mid.1 + ny * off);
        let p2 = (mid.0 - nx * off, mid.1 - ny * off);
        let (a1, a2) = (cfg.alpha_at(p1), cfg.alpha_at(p2));
        let worse = if cfg.obstacle > 0.0 { a1 > a2 } else { a1 < a2 };
        if worse && rng.f() < cfg.obstacle.abs() / 10.0 {
            off = -off;
        }
    }
    let m = ((a.0 + z.0) * 0.5 + nx * off, (a.1 + z.1) * 0.5 + ny * off);
    // Positive obstacles stop branches that run into opaque areas.
    if cfg.obstacle > 0.0 && !main && cfg.alpha_at(m) > 0.5 && rng.f() < cfg.obstacle / 10.0 {
        return;
    }
    bolt(cfg, BoltBranch { start: a, end: m, depth: depth - 1, intensity: inten, main }, rng, output);
    bolt(cfg, BoltBranch { start: m, end: z, depth: depth - 1, intensity: inten, main }, rng, output);
    if rng.f() < cfg.fork && inten > cfg.term && len >= cfg.min_fork {
        let ang = rng.s() * 0.7;
        let (s, c) = ang.sin_cos();
        let (fx, fy) = ((m.0 - a.0) * c - (m.1 - a.1) * s, (m.0 - a.0) * s + (m.1 - a.1) * c);
        let k = 1.2;
        // Fork Strength 50 % keeps the decayed intensity; Fork Variation randomises it.
        let fi = inten * (1.0 - cfg.decay) * cfg.fork_strength * 2.0 * if cfg.fork_var > 0.0 { 1.0 - cfg.fork_var * rng.f() as f32 } else { 1.0 };
        bolt(cfg, BoltBranch { start: m, end: (m.0 + fx * k, m.1 + fy * k), depth: depth.saturating_sub(1), intensity: fi.min(1.0), main: false }, rng, output);
    }
}

/// Advanced Lightning's bolt on `b` (shared with the GPU compositor; `b.img` is read only for
/// Alpha Obstacle).
pub fn lightning_segments(ctx: &EffectCtx, b: &Buf) -> Vec<Seg> {
    let kind = ctx.params.e("lightningType");
    let decay_main = ctx.params.b("decayMainCore");
    let o = b.to_px(ctx.params.v2("origin"));
    let d = b.to_px(ctx.params.v2("direction"));
    let turb = ctx.params.f("turbulence").max(0.0);
    let fork = (ctx.params.f("forking") / 100.0).clamp(0.0, 1.0);
    let decay = ctx.params.f("decay").clamp(0.0, 1.0) as f32;
    let fractal = ctx.params.e("expertSettings/fractalType");
    let depth = ctx.params.f("expertSettings/complexity").round().clamp(1.0, 12.0) as u32 + u32::from(fractal == 2);
    let state = ctx.params.f("conductivityState").floor() as i64 as u64;
    let mut rng = Rng::new(state ^ ((ctx.seed as u64) << 32));
    let obstacle = ctx.params.f("alphaObstacle").clamp(-10.0, 10.0);
    let alpha_plane = (obstacle != 0.0).then(|| Plane::alpha(&b.img));
    let g = |id: &str, dflt: f64| ctx.params.get(id).map(Value::as_f64).unwrap_or(dflt);
    let cfg = BoltCfg {
        turb,
        fork,
        decay,
        alpha: alpha_plane.as_ref(),
        obstacle,
        main_only: ctx.params.b("expertSettings/mainCoreCollisionOnly"),
        min_fork: g("expertSettings/minForkDistance", 0.0).max(0.0) * b.scale,
        term: (g("expertSettings/terminationThreshold", 5.0) / 100.0).clamp(0.0, 1.0) as f32,
        shape: match fractal {
            1 => 0.7,
            2 => 0.45,
            _ => 1.0,
        },
        fork_strength: (g("expertSettings/forkStrength", 50.0) / 100.0).clamp(0.0, 1.0) as f32,
        fork_var: (g("expertSettings/forkVariation", 0.0) / 100.0).clamp(0.0, 1.0) as f32,
    };
    let mut output = BoltOutput::default();
    let (h, w) = (b.img.height as f64, b.img.width as f64);
    let mut go = |a, z, depth, cfg: &BoltCfg, rng: &mut Rng| bolt(cfg, BoltBranch { start: a, end: z, depth, intensity: 1.0, main: true }, rng, &mut output);
    // LIGHTNING_TYPES order.
    match kind {
        // Direction: travels toward the Direction point and beyond.
        0 => {
            let z = (o.0 + (d.0 - o.0) * 3.0, o.1 + (d.1 - o.1) * 3.0);
            go(o, z, depth, &cfg, &mut rng);
        }
        // Breaking: more branching as the points move apart.
        2 => {
            let dist = ((d.0 - o.0).powi(2) + (d.1 - o.1).powi(2)).sqrt();
            let f = (fork * (0.5 + dist / w.max(h).max(1.0))).min(1.0);
            go(o, d, depth, &BoltCfg { fork: f, ..cfg }, &mut rng);
        }
        // Bouncy: a strike with a stronger zig-zag.
        3 => go(o, d, depth, &BoltCfg { turb: turb * 1.6, ..cfg }, &mut rng),
        // Omni / Anywhere: bolts in all directions out to the Outer Radius.
        4 | 5 => {
            let len = ((d.0 - o.0).powi(2) + (d.1 - o.1).powi(2)).sqrt().max(w.min(h) * 0.25);
            for _ in 0..4 {
                let a = rng.f() * 2.0 * PI;
                let l = if kind == 5 { len * (0.3 + 0.7 * rng.f()) } else { len };
                let z = (o.0 + a.cos() * l, o.1 + a.sin() * l);
                go(o, z, depth, &cfg, &mut rng);
            }
        }
        6 => go(o, (o.0, h), depth, &cfg, &mut rng),
        // Two-Way Strike: from both ends toward the middle.
        7 => {
            let m = ((o.0 + d.0) * 0.5, (o.1 + d.1) * 0.5);
            go(o, m, depth.saturating_sub(1).max(1), &cfg, &mut rng);
            go(d, m, depth.saturating_sub(1).max(1), &cfg, &mut rng);
        }
        // Strike.
        _ => go(o, d, depth, &cfg, &mut rng),
    }
    let BoltOutput { segments: mut segs, mains } = output;
    let dist = |p: (f64, f64)| ((p.0 - o.0).powi(2) + (p.1 - o.1).powi(2)).sqrt();
    let far = segs.iter().map(|(_, z, _)| dist(*z)).fold(1e-9, f64::max);
    // Decay Main Core: the whole bolt fades with distance from the origin.
    if decay_main {
        for (a, _, i) in &mut segs {
            let t = (dist(*a) / far) as f32;
            *i *= (1.0 - decay * t).max(0.0);
        }
    }
    // Core Drain: the main core loses intensity along its length.
    let drain = (g("expertSettings/coreDrain", 0.0) / 100.0).clamp(0.0, 1.0) as f32;
    if drain > 0.0 {
        for ((a, _, i), m) in segs.iter_mut().zip(&mains) {
            if *m {
                *i *= (1.0 - drain * (dist(*a) / far) as f32).max(0.0);
            }
        }
    }
    segs
}

fn advanced_lightning(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let core_r = (ctx.params.f("coreSettings/coreRadius") * b.scale).max(0.3);
    let core_op = (ctx.params.f("coreSettings/coreOpacity") / 100.0) as f32;
    let core_c = ctx.params.color("coreSettings/coreColor");
    let glow_r = ctx.params.f("glowSettings/glowRadius") * b.scale;
    let glow_op = (ctx.params.f("glowSettings/glowOpacity") / 100.0) as f32;
    let glow_c = ctx.params.color("glowSettings/glowColor");
    let composite = ctx.params.b("compositeOnOriginal");
    let segs = lightning_segments(ctx, &b);
    let (w, h) = (b.img.width as usize, b.img.height as usize);
    // Bucket segments per row (by bbox) so rows rasterise in parallel.
    let mut buckets: Vec<Vec<usize>> = vec![Vec::new(); h];
    for (i, (a, z, _)) in segs.iter().enumerate() {
        let y0 = ((a.1.min(z.1) - core_r - 1.0).floor().max(0.0)) as usize;
        let y1 = ((a.1.max(z.1) + core_r + 1.0).ceil()).min(h as f64) as i64;
        for y in y0..(y1.max(0) as usize).min(h) {
            buckets[y].push(i);
        }
    }
    let mut core = Plane::new(w, h);
    core.data.par_chunks_mut(w.max(1)).enumerate().for_each(|(y, row)| {
        let py = y as f64 + 0.5;
        for &i in &buckets[y] {
            let (a, z, inten) = segs[i];
            let x0 = ((a.0.min(z.0) - core_r - 1.0).floor().max(0.0)) as usize;
            let x1 = ((a.0.max(z.0) + core_r + 1.0).ceil().max(0.0) as usize).min(w);
            let (dx, dy) = (z.0 - a.0, z.1 - a.1);
            let l2 = (dx * dx + dy * dy).max(1e-12);
            for x in x0..x1 {
                let px = x as f64 + 0.5;
                let t = (((px - a.0) * dx + (py - a.1) * dy) / l2).clamp(0.0, 1.0);
                let d = ((px - a.0 - dx * t).powi(2) + (py - a.1 - dy * t).powi(2)).sqrt();
                let v = (core_r - d + 0.5).clamp(0.0, 1.0) as f32 * inten;
                if v > row[x] {
                    row[x] = v;
                }
            }
        }
    });
    let glow = if glow_r > 0.5 { gauss_plane(&core, glow_r / 3.0, glow_r / 3.0).map(|v| (v * 3.0).min(1.0)) } else { Plane::new(w, h) };
    b.img.data.par_iter_mut().enumerate().for_each(|(i, px)| {
        let cv = core.data[i] * core_op;
        let gv = glow.data[i] * glow_op;
        let l = [core_c[0] * cv + glow_c[0] * gv, core_c[1] * cv + glow_c[1] * gv, core_c[2] * cv + glow_c[2] * gv];
        let la = cv.max(gv).min(1.0);
        *px = if composite { add_light(*px, l, la) } else { [l[0], l[1], l[2], la] };
    });
    b
}

// ---------------------------------------------------------------- CC Light Rays / Burst

const RAY_STEPS: usize = 32;

fn light_rays(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let c = b.to_px(ctx.params.v2("center"));
    let k = (ctx.params.f("intensity") / 100.0 * 0.5).clamp(0.0, 0.95);
    let gain = (ctx.params.f("intensity") / 100.0 * 2.0) as f32;
    let rad = (ctx.params.f("radius") * b.scale).max(0.5);
    let ws = (ctx.params.f("warpSoftness") / 50.0).max(0.0);
    let square = ctx.params.e("shape") == 1;
    let from_src = ctx.params.b("colorFromSource");
    let color = ctx.params.color("color");
    let mode = ctx.params.e("transferMode");
    let src = b.img.clone();
    let mask = |x: f64, y: f64| {
        let (dx, dy) = ((x - c.0).abs(), (y - c.1).abs());
        let d = if square { dx.max(dy) } else { (dx * dx + dy * dy).sqrt() };
        1.0 - smoothstep(rad as f32, (rad * (1.0 + ws)) as f32 + 1e-3, d as f32)
    };
    map_xy(&mut b.img, |x, y, px| {
        let (pxx, pyy) = (x as f64 + 0.5, y as f64 + 0.5);
        let mut acc = [0.0f32; 4];
        for i in 0..RAY_STEPS {
            let s = 1.0 - k * i as f64 / RAY_STEPS as f64;
            let (qx, qy) = (c.0 + (pxx - c.0) * s, c.1 + (pyy - c.1) * s);
            let m = mask(qx, qy);
            if m <= 0.0 {
                continue;
            }
            let v = src.sample_bilinear(qx, qy);
            for j in 0..4 {
                acc[j] += v[j] * m;
            }
        }
        let mut ray = acc.map(|v| v / RAY_STEPS as f32 * gain);
        if !from_src {
            let (sc, _) = unpremul(ray);
            let l = luminance(sc[0], sc[1], sc[2]) * ray[3];
            ray = [color[0] * l, color[1] * l, color[2] * l, ray[3]];
        }
        let la = ray[3].min(1.0);
        match mode {
            1 => [px[0].max(ray[0]), px[1].max(ray[1]), px[2].max(ray[2]), px[3].max(la)],
            2 => {
                let s = |a: f32, b: f32| a + b - a * b;
                [s(px[0], ray[0]), s(px[1], ray[1]), s(px[2], ray[2]), s(px[3], la)]
            }
            3 => [ray[0], ray[1], ray[2], la],
            _ => add_light(px, [ray[0], ray[1], ray[2]], la),
        }
    });
    b
}

fn light_burst(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let c = b.to_px(ctx.params.v2("center"));
    let inten = (ctx.params.f("intensity") / 100.0) as f32;
    let k = (ctx.params.f("rayLength") / 100.0).clamp(0.0, 1.0);
    let mode = ctx.params.e("burst");
    let set_color = ctx.params.b("setColor");
    let color = ctx.params.color("color");
    if k <= 0.0 && (inten - 1.0).abs() < 1e-6 && !set_color {
        return b;
    }
    let src = b.img.clone();
    map_xy(&mut b.img, |x, y, _| {
        let (pxx, pyy) = (x as f64 + 0.5, y as f64 + 0.5);
        let mut acc = [0.0f32; 4];
        let mut wsum = 0.0f32;
        let mut best = [0.0f32; 4];
        let mut best_l = -1.0f32;
        for i in 0..RAY_STEPS {
            let f = i as f64 / RAY_STEPS as f64;
            let s = 1.0 - k * f;
            let v = src.sample_bilinear(c.0 + (pxx - c.0) * s, c.1 + (pyy - c.1) * s);
            match mode {
                0 => {
                    let l = luminance(v[0], v[1], v[2]) + v[3] * 1e-3;
                    if l > best_l {
                        best_l = l;
                        best = v;
                    }
                }
                1 => {
                    let w = 1.0 - f as f32;
                    for j in 0..4 {
                        acc[j] += v[j] * w;
                    }
                    wsum += w;
                }
                _ => {
                    for j in 0..4 {
                        acc[j] += v[j];
                    }
                    wsum += 1.0;
                }
            }
        }
        let mut o = if mode == 0 { best } else { acc.map(|v| v / wsum.max(1e-6)) };
        if set_color {
            let (sc, a) = unpremul(o);
            let l = luminance(sc[0], sc[1], sc[2]) * a;
            o = [color[0] * l, color[1] * l, color[2] * l, a];
        }
        [o[0] * inten, o[1] * inten, o[2] * inten, (o[3] * inten).clamp(0.0, 1.0)]
    });
    b
}

// ---------------------------------------------------------------- CC Light Sweep

fn light_sweep(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let c = b.to_px(ctx.params.v2("center"));
    let dir = ctx.params.f("direction").to_radians();
    let shape = ctx.params.e("shape");
    let half = (ctx.params.f("width") * b.scale * 0.5).max(0.5);
    let sweep = (ctx.params.f("sweepIntensity") / 100.0) as f32;
    let edge_i = (ctx.params.f("edgeIntensity") / 100.0) as f32;
    let edge_t = ctx.params.f("edgeThickness") * b.scale;
    let lc = ctx.params.color("lightColor");
    let mode = ctx.params.e("lightReceptionMode");
    let (nx, ny) = (dir.cos(), dir.sin());
    let alpha = Plane::alpha(&b.img);
    let soft_a = if edge_t > 0.05 { gauss_plane(&alpha, edge_t * 0.5, edge_t * 0.5) } else { alpha };
    map_xy(&mut b.img, |x, y, px| {
        let d = ((x as f64 + 0.5 - c.0) * nx + (y as f64 + 0.5 - c.1) * ny).abs();
        let band = match shape {
            0 => (1.0 - d / half).max(0.0) as f32,
            2 => (half - d + 0.5).clamp(0.0, 1.0) as f32,
            _ => smoothstep(half as f32, 0.0, d as f32),
        };
        if band <= 0.0 {
            return if mode == 2 { [0.0; 4] } else { px };
        }
        let (xi, yi) = (x as i64, y as i64);
        let gx = soft_a.get_clamped(xi + 1, yi) - soft_a.get_clamped(xi - 1, yi);
        let gy = soft_a.get_clamped(xi, yi + 1) - soft_a.get_clamped(xi, yi - 1);
        let edge = ((gx * gx + gy * gy).sqrt() * 2.0).min(1.0);
        let light = band * sweep + band * edge * edge_i;
        let a = px[3];
        match mode {
            1 => {
                let la = (band * sweep).min(1.0);
                [px[0] + lc[0] * light, px[1] + lc[1] * light, px[2] + lc[2] * light, (a + la * (1.0 - a)).min(1.0)]
            }
            2 => {
                let la = (light * a).min(1.0);
                [lc[0] * la, lc[1] * la, lc[2] * la, la]
            }
            _ => [px[0] + lc[0] * light * a, px[1] + lc[1] * light * a, px[2] + lc[2] * light * a, a],
        }
    });
    b
}

pub fn specs() -> Vec<EffectSpec> {
    let ang = || ParamUi::Angle;
    vec![
        spec(
            "ec.generate.cellpattern",
            "Cell Pattern",
            vec![
                p("cellPattern", "Cell Pattern", Value::Enum(0), popup(&CELL_PATTERNS)),
                p("invert", "Invert", Value::Bool(false), ParamUi::Checkbox),
                p("contrast", "Contrast", num(100.0), slider(0.0, 10000.0, 0.0, 400.0, 1)),
                p("overflow", "Overflow", Value::Enum(0), popup(&["Clip", "Soft Clamp", "Wrap Back"])),
                p("disperse", "Disperse", num(1.0), slider(0.0, 1.5, 0.0, 1.5, 2)),
                p("size", "Size", num(60.0), slider(2.0, 4000.0, 2.0, 400.0, 1)),
                p("offset", "Offset", pt(0.5, 0.5), ParamUi::Point),
                p("tilingOptions/enableTiling", "Enable Tiling", Value::Bool(false), ParamUi::Checkbox),
                p("tilingOptions/cellsHorizontal", "Cells Horizontal", num(16.0), slider(1.0, 1000.0, 1.0, 100.0, 0)),
                p("tilingOptions/cellsVertical", "Cells Vertical", num(16.0), slider(1.0, 1000.0, 1.0, 100.0, 0)),
                p("evolution", "Evolution", num(0.0), ang()),
                p("evolutionOptions/cycleEvolution", "Cycle Evolution", Value::Bool(false), ParamUi::Checkbox),
                p("evolutionOptions/cycle", "Cycle (in Revolutions)", num(1.0), slider(1.0, 1000.0, 1.0, 20.0, 0)),
                p("evolutionOptions/randomSeed", "Random Seed", num(0.0), slider(0.0, 100000.0, 0.0, 1000.0, 0)),
            ],
            cell_pattern,
        ),
        spec(
            "ec.generate.ellipse",
            "Ellipse",
            vec![
                p("center", "Center", pt(0.5, 0.5), ParamUi::Point),
                p("width", "Width", num(100.0), slider(0.0, 30000.0, 0.0, 1000.0, 1)),
                p("height", "Height", num(100.0), slider(0.0, 30000.0, 0.0, 1000.0, 1)),
                p("thickness", "Thickness", num(10.0), slider(0.0, 3000.0, 0.0, 100.0, 1)),
                p("softness", "Softness", num(0.0), pct()),
                p("insideColor", "Inside Color", col(1.0, 1.0, 1.0), ParamUi::Color),
                p("outsideColor", "Outside Color", col(0.0, 0.6, 1.0), ParamUi::Color),
                p("compositeOnOriginal", "Composite On Original", Value::Bool(false), ParamUi::Checkbox),
            ],
            ellipse,
        ),
        spec(
            "ec.generate.lensflare",
            "Lens Flare",
            vec![
                p("flareCenter", "Flare Center", pt(0.25, 0.25), ParamUi::Point),
                p("flareBrightness", "Flare Brightness", num(100.0), slider(0.0, 300.0, 10.0, 300.0, 0)),
                p("lensType", "Lens Type", Value::Enum(0), popup(&["50-300mm Zoom", "35mm Prime", "105mm Prime"])),
                p("blendWithOriginal", "Blend With Original", num(0.0), pct()),
            ],
            lens_flare,
        ),
        spec(
            "ec.generate.beam",
            "Beam",
            vec![
                p("startPoint", "Starting Point", pt(0.25, 0.5), ParamUi::Point),
                p("endPoint", "Ending Point", pt(0.75, 0.5), ParamUi::Point),
                p("length", "Length", num(25.0), pct()),
                p("time", "Time", num(0.0), pct()),
                p("startThickness", "Starting Thickness", num(8.0), slider(0.0, 1000.0, 0.0, 100.0, 1)),
                p("endThickness", "Ending Thickness", num(8.0), slider(0.0, 1000.0, 0.0, 100.0, 1)),
                p("softness", "Softness", num(20.0), pct()),
                p("insideColor", "Inside Color", col(1.0, 1.0, 1.0), ParamUi::Color),
                p("outsideColor", "Outside Color", col(0.4, 0.4, 1.0), ParamUi::Color),
                p("perspective3d", "3D Perspective", Value::Bool(true), ParamUi::Checkbox),
                p("compositeOnOriginal", "Composite On Original", Value::Bool(true), ParamUi::Checkbox),
            ],
            beam,
        ),
        spec(
            "ec.generate.radiowaves",
            "Radio Waves",
            vec![
                p("producerPoint", "Producer Point", pt(0.5, 0.5), ParamUi::Point),
                p("parametersAreSetAt", "Parameters Are Set At", Value::Enum(1), popup(&["Birth", "Each Frame"])),
                p("renderQuality", "Render Quality", num(4.0), slider(1.0, 10.0, 1.0, 10.0, 0)),
                p("waveType", "Wave Type", Value::Enum(0), popup(&RW_TYPES)),
                p("polygon/sides", "Sides", num(64.0), slider(3.0, 64.0, 3.0, 64.0, 0)),
                p("polygon/star", "Star", Value::Bool(false), ParamUi::Checkbox),
                p("polygon/starDepth", "Star Depth", num(-0.3), slider(-1.0, 1.0, -1.0, 1.0, 2)),
                p("imageContour/sourceLayer", "Source Layer", Value::Layer(None), ParamUi::Layer),
                p("imageContour/sourceCenter", "Source Center", pt(0.5, 0.5), ParamUi::Point),
                p("imageContour/valueChannel", "Value Channel", Value::Enum(4), popup(&RW_CHANNELS)),
                p("imageContour/invertInput", "Invert Input", Value::Bool(false), ParamUi::Checkbox),
                p("imageContour/valueThreshold", "Value Threshold", num(128.0), slider(0.0, 255.0, 0.0, 255.0, 0)),
                p("imageContour/preBlur", "Pre-Blur", num(1.0), slider(0.0, 100.0, 0.0, 20.0, 1)),
                p("imageContour/tolerance", "Tolerance", num(1.0), slider(0.0, 100.0, 0.0, 20.0, 1)),
                p("imageContour/contour", "Contour", num(0.0), slider(0.0, 100.0, 0.0, 10.0, 0)),
                p("waveMask/mask", "Mask", num(0.0), ParamUi::Mask),
                p("waveMotion/frequency", "Frequency", num(1.0), slider(0.01, 100.0, 0.01, 10.0, 2)),
                p("waveMotion/expansion", "Expansion", num(100.0), slider(0.0, 10000.0, 0.0, 1000.0, 1)),
                p("waveMotion/orientation", "Orientation", num(0.0), ang()),
                p("waveMotion/direction", "Direction", num(90.0), ang()),
                p("waveMotion/velocity", "Velocity", num(0.0), slider(0.0, 10000.0, 0.0, 1000.0, 1)),
                p("waveMotion/spin", "Spin", num(0.0), slider(-3600.0, 3600.0, -360.0, 360.0, 1)),
                p("waveMotion/lifespan", "Lifespan (sec)", num(2.0), slider(0.01, 100.0, 0.01, 10.0, 2)),
                p("waveMotion/reflection", "Reflection", Value::Bool(false), ParamUi::Checkbox),
                p("waveStroke/profile", "Profile", Value::Enum(0), popup(&RW_PROFILES)),
                p("waveStroke/color", "Color", col(1.0, 1.0, 1.0), ParamUi::Color),
                p("waveStroke/opacity", "Opacity", num(100.0), pct()),
                p("waveStroke/fadeInTime", "Fade-in Time", num(0.0), slider(0.0, 100.0, 0.0, 5.0, 2)),
                p("waveStroke/fadeOutTime", "Fade-out Time", num(0.0), slider(0.0, 100.0, 0.0, 5.0, 2)),
                p("waveStroke/startWidth", "Start Width", num(5.0), slider(0.0, 1000.0, 0.0, 100.0, 1)),
                p("waveStroke/endWidth", "End Width", num(5.0), slider(0.0, 1000.0, 0.0, 100.0, 1)),
            ],
            radio_waves,
        ),
        spec(
            "ec.generate.advancedlightning",
            "Advanced Lightning",
            vec![
                p("lightningType", "Lightning Type", Value::Enum(1), popup(&LIGHTNING_TYPES)),
                p("origin", "Origin", pt(0.25, 0.2), ParamUi::Point),
                p("direction", "Direction", pt(0.75, 0.8), ParamUi::Point),
                p("conductivityState", "Conductivity State", num(10.0), slider(0.0, 100000.0, 0.0, 100.0, 1)),
                p("coreSettings/coreRadius", "Core Radius", num(3.0), slider(0.0, 100.0, 0.0, 20.0, 1)),
                p("coreSettings/coreOpacity", "Core Opacity", num(75.0), pct()),
                p("coreSettings/coreColor", "Core Color", col(1.0, 1.0, 1.0), ParamUi::Color),
                p("glowSettings/glowRadius", "Glow Radius", num(50.0), slider(0.0, 1000.0, 0.0, 200.0, 1)),
                p("glowSettings/glowOpacity", "Glow Opacity", num(50.0), pct()),
                p("glowSettings/glowColor", "Glow Color", col(0.25, 0.35, 1.0), ParamUi::Color),
                p("turbulence", "Turbulence", num(1.0), slider(0.0, 10.0, 0.0, 4.0, 2)),
                p("forking", "Forking", num(25.0), pct()),
                p("decay", "Decay", num(0.3), slider(0.0, 1.0, 0.0, 1.0, 2)),
                p("decayMainCore", "Decay Main Core", Value::Bool(false), ParamUi::Checkbox),
                p("alphaObstacle", "Alpha Obstacle", num(0.0), slider(-10.0, 10.0, -10.0, 10.0, 2)),
                p("compositeOnOriginal", "Composite On Original", Value::Bool(true), ParamUi::Checkbox),
                p("expertSettings/complexity", "Complexity", num(6.0), slider(1.0, 12.0, 1.0, 12.0, 0)),
                p("expertSettings/minForkDistance", "Min Fork Distance", num(0.0), slider(0.0, 1000.0, 0.0, 100.0, 1)),
                p("expertSettings/terminationThreshold", "Termination Threshold", num(5.0), slider(0.0, 100.0, 0.0, 100.0, 1)),
                p("expertSettings/mainCoreCollisionOnly", "Main Core Collision Only", Value::Bool(false), ParamUi::Checkbox),
                p("expertSettings/fractalType", "Fractal Type", Value::Enum(0), popup(&FRACTAL_TYPES)),
                p("expertSettings/coreDrain", "Core Drain", num(0.0), slider(0.0, 100.0, 0.0, 100.0, 1)),
                p("expertSettings/forkStrength", "Fork Strength", num(50.0), slider(0.0, 100.0, 0.0, 100.0, 1)),
                p("expertSettings/forkVariation", "Fork Variation", num(0.0), slider(0.0, 100.0, 0.0, 100.0, 1)),
            ],
            advanced_lightning,
        ),
        spec(
            "ec.generate.cclightrays",
            "CC Light Rays",
            vec![
                p("intensity", "Intensity", num(100.0), slider(0.0, 600.0, 0.0, 200.0, 1)),
                p("center", "Center", pt(0.5, 0.5), ParamUi::Point),
                p("radius", "Radius", num(40.0), slider(0.0, 4000.0, 0.0, 400.0, 1)),
                p("warpSoftness", "Warp Softness", num(50.0), slider(0.0, 400.0, 0.0, 100.0, 1)),
                p("shape", "Shape", Value::Enum(0), popup(&["Round", "Square"])),
                p("colorFromSource", "Color from Source", Value::Bool(true), ParamUi::Checkbox),
                p("color", "Color", col(1.0, 1.0, 1.0), ParamUi::Color),
                p("transferMode", "Transfer Mode", Value::Enum(0), popup(&["Add", "Lighten", "Screen", "None"])),
            ],
            light_rays,
        ),
        spec(
            "ec.generate.cclightburst",
            "CC Light Burst 2.5",
            vec![
                p("center", "Center", pt(0.5, 0.5), ParamUi::Point),
                p("intensity", "Intensity", num(100.0), slider(0.0, 600.0, 0.0, 200.0, 1)),
                p("rayLength", "Ray Length", num(50.0), slider(0.0, 100.0, 0.0, 100.0, 1)),
                p("burst", "Burst", Value::Enum(0), popup(&["Straight", "Fade", "Center"])),
                p("setColor", "Set Color", Value::Bool(false), ParamUi::Checkbox),
                p("color", "Color", col(1.0, 1.0, 1.0), ParamUi::Color),
            ],
            light_burst,
        ),
        spec(
            "ec.generate.cclightsweep",
            "CC Light Sweep",
            vec![
                p("center", "Center", pt(0.5, 0.5), ParamUi::Point),
                p("direction", "Direction", num(-30.0), ang()),
                p("shape", "Shape", Value::Enum(1), popup(&["Linear", "Smooth", "Sharp"])),
                p("width", "Width", num(50.0), slider(0.0, 4000.0, 0.0, 400.0, 1)),
                p("sweepIntensity", "Sweep Intensity", num(50.0), slider(0.0, 500.0, 0.0, 200.0, 1)),
                p("edgeIntensity", "Edge Intensity", num(100.0), slider(0.0, 500.0, 0.0, 200.0, 1)),
                p("edgeThickness", "Edge Thickness", num(1.0), slider(0.0, 100.0, 0.0, 10.0, 1)),
                p("lightColor", "Light Color", col(1.0, 1.0, 1.0), ParamUi::Color),
                p("lightReceptionMode", "Light Reception", Value::Enum(0), popup(&["Add", "Composite", "Cutout"])),
            ],
            light_sweep,
        ),
    ]
}

#[cfg(test)]
mod tests;

//! Distort effects, batch 3: CC Bender, CC Blobbylize, CC Power Pin, CC Ripple Pulse, CC Slant,
//! CC Smear, CC Split, CC Split 2, Smear and Reshape (mask driven), Twirl (Legacy), Warp (the
//! fifteen warp styles) and Detail-preserving Upscale.
//!
//! All warps are inverse mapped: every output pixel finds the source point that lands on it.
//! Pixel-sized parameters are multiplied by the buffer scale (downsampled previews).

use std::f64::consts::{PI, TAU};

use effectcraft_geom::{Mat3, vec2};
use effectcraft_keyframe::Value;
use effectcraft_project::ParamUi;
use effectcraft_raster::{Image, Px};
use rayon::prelude::*;

use crate::util::{Plane, dist_to_poly, gauss_plane, layer_or_self, pick, point_in_poly, poly_length, poly_point_at, premul, remap, src_at, unpremul};
use crate::{Buf, EffectCtx, EffectSpec, col, num, p, popup, slider};

fn spec(id: &'static str, name: &'static str, params: Vec<crate::ParamSpec>, render: crate::RenderFn) -> EffectSpec {
    EffectSpec { id, name, category: "Distort", params, render, gpu: false, float: true }
}

fn pt(x: f64, y: f64) -> Value {
    Value::Vec2([x, y])
}

/// Mask choices for mask popups ("None", "Mask 1" … "Mask 8").
const MASK_CHOICES: [&str; 9] = ["None", "Mask 1", "Mask 2", "Mask 3", "Mask 4", "Mask 5", "Mask 6", "Mask 7", "Mask 8"];

/// The chosen mask as a polyline in buffer pixels (`None` for "None" or a missing mask).
fn mask_px(ctx: &EffectCtx, b: &Buf, id: &str) -> Option<(Vec<[f64; 2]>, bool)> {
    let i = ctx.params.e(id) as usize;
    if i == 0 {
        return None;
    }
    let m = ctx.env.masks.get(i - 1)?;
    if m.points.len() < 2 {
        return None;
    }
    Some((m.points.iter().map(|q| [q[0] * b.scale + b.offset[0], q[1] * b.scale + b.offset[1]]).collect(), m.closed))
}

/// Resample a polyline to `n` points evenly spaced by arc length.
fn resample_poly(pts: &[[f64; 2]], closed: bool, n: usize) -> Vec<[f64; 2]> {
    let len = poly_length(pts, closed);
    let div = if closed { n as f64 } else { (n - 1).max(1) as f64 };
    (0..n).map(|i| poly_point_at(pts, closed, len * i as f64 / div).0).collect()
}

fn centroid(pts: &[[f64; 2]]) -> [f64; 2] {
    let n = pts.len().max(1) as f64;
    let s = pts.iter().fold([0.0, 0.0], |a, q| [a[0] + q[0], a[1] + q[1]]);
    [s[0] / n, s[1] / n]
}

/// Elasticity popup (Stiff … Super Fluid) → inverse-distance weighting exponent.
const ELASTICITY: [&str; 8] = ["Stiff", "Less Stiff", "Below Normal", "Absolutely Normal", "Above Average", "Loose", "Liquid", "Super Fluid"];
fn elastic_exp(i: u32) -> f64 {
    [1.0, 1.25, 1.5, 2.0, 2.5, 3.0, 3.5, 4.5][(i as usize).min(7)]
}

// ---------------------------------------------------------------- CC Bender

fn cc_bender(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let amount = ctx.params.f("amount") / 100.0;
    if amount == 0.0 {
        return b;
    }
    let style = ctx.params.e("style");
    let top = b.to_px(ctx.params.v2("top"));
    let base = b.to_px(ctx.params.v2("base"));
    let (ax, ay) = (top.0 - base.0, top.1 - base.1);
    let len = (ax * ax + ay * ay).sqrt().max(1e-6);
    let (ux, uy) = (ax / len, ay / len);
    // Perpendicular (to the right of the axis when it points up).
    let (nx, ny) = (-uy, ux);
    let reference = if ctx.params.b("adjustToDistance") { len } else { ctx.layer_size[1] * b.scale };
    let f = move |t: f64| -> f64 {
        let t = t.clamp(0.0, 1.0);
        match style {
            1 => 0.5 * (PI * t).sin() * t,                              // Marilyn: billowing middle
            2 => 0.5 * t,                                               // Sharp: straight lean from the base
            3 => 0.25 * (1.0 - (PI * t).cos()) - 0.1 * (TAU * t).sin(), // Boxer: S-shaped
            _ => 0.5 * t * t,                                           // Bend: smooth curve
        }
    };
    b.img = remap(&b.img, false, |x, y| {
        let (rx, ry) = (x - base.0, y - base.1);
        let t = (rx * ux + ry * uy) / len;
        let d = amount * reference * f(t);
        Some((x - nx * d, y - ny * d))
    });
    b
}

// ---------------------------------------------------------------- shading helpers

/// Unit light vector from AE-style direction (0° = up, clockwise) and height (0–100 %).
fn distant_light(dir_deg: f64, height_pct: f64) -> [f64; 3] {
    let el = (height_pct / 100.0).clamp(0.0, 1.0) * PI * 0.5;
    let d = dir_deg.to_radians();
    [d.sin() * el.cos(), -d.cos() * el.cos(), el.sin()]
}

fn norm3(v: [f64; 3]) -> [f64; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-12);
    [v[0] / l, v[1] / l, v[2] / l]
}

/// f32 normal/dot helpers for the CC Blobbylize lighting pass. The GPU kernel runs in f32;
/// matching its precision here keeps specular highlights within the CPU/GPU parity tolerance.
fn norm3f(v: [f32; 3]) -> [f32; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-12);
    [v[0] / l, v[1] / l, v[2] / l]
}

fn dot3f(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

// ---------------------------------------------------------------- CC Blobbylize

fn cc_blobbylize(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let soft = ctx.params.f("softness").max(0.0) * b.scale;
    let cut = (ctx.params.f("cutAway") / 100.0).clamp(0.0, 1.0) as f32;
    if cut >= 1.0 {
        // Endpoint guard: at Cut Away 100 the gradient antialias must not leave a half-covered
        // pixel where an opaque source reaches exactly 1.0.
        b.img.data.fill([0.0; 4]);
        return b;
    }
    let blob = layer_or_self(ctx, &b, "blobLayer", true, false);
    let prop = src_at(ctx.params.e("property"));
    let raw = Plane::from_image(&blob, |px| {
        let (c, a) = unpremul(px);
        pick(prop, c, a) * if matches!(prop, crate::util::Src::Alpha) { 1.0 } else { a }
    });
    // Oracle (AE 26.2, 16 bpc): a step thresholded at Cut 25/75 with Softness 20 moves the
    // 50% contour by 5.55 px. For a Gaussian that is 0.67449 sigma, so sigma = 0.42 Softness.
    // The same probe shows the matte stays a ~1 px antialiased threshold, not a broad ramp.
    let h = gauss_plane(&raw, soft * 0.42, soft * 0.42);
    let intensity = ctx.params.f("lightIntensity") / 100.0;
    let lc = ctx.params.color("lightColor");
    let point = ctx.params.e("lightType") == 1;
    let lpos = b.to_px(ctx.params.v2("lightPosition"));
    let lheight = ctx.params.f("lightHeight");
    let ldir = distant_light(ctx.params.f("lightDirection"), lheight);
    let ambient = ctx.params.f("ambient") / 100.0;
    let diffuse = ctx.params.f("diffuse") / 100.0;
    let specular = ctx.params.f("specular") / 100.0;
    let rough = ctx.params.f("roughness").clamp(0.001, 1.0);
    let metal = (ctx.params.f("metal") / 100.0) as f32;
    let shininess = (1.0 / rough).min(500.0);
    let src = b.img.clone();
    let (w, hh) = (b.img.width as usize, b.img.height as usize);
    let span = (w.max(hh) as f64).max(1.0);

    // The source matte and the RGB refraction are separate in the oracle:
    // - the matte edge of a self step moves 5/9/14/18 px at Softness 5/10/20/40. A normal
    //   with z = 1, a ~30 px depth at 1920 and a slope scale of 1.5 Softness reproduces that;
    // - RGB follows a height-field refraction with z = height: on a linear ramp the measured
    //   horizontal offset is dx = -0.34 / h, independent of Softness.
    let matte_slope = soft * 1.5;
    let matte_depth = if soft > 0.05 { span / 64.0 } else { 0.0 };
    let rgb_depth = span * 0.34 * b.scale;
    let light_slope = (soft * 0.65).max(1.0);
    let limit = span * 0.5;
    b.img.rows_mut().for_each(|(y, row)| {
        for (x, px) in row.iter_mut().enumerate() {
            let (xi, yi) = (x as i64, y as i64);
            let hv = h.get(x, y);
            let gx = (h.get_clamped(xi + 1, yi) - h.get_clamped(xi - 1, yi)) as f64 * 0.5;
            let gy = (h.get_clamped(xi, yi + 1) - h.get_clamped(xi, yi - 1)) as f64 * 0.5;
            let grad = (gx * gx + gy * gy).sqrt();
            // One-pixel antialiased threshold. A flat field is either fully in or fully out.
            let aa = (1.25 * grad).max(5e-4);
            let m = if grad > 1e-9 {
                (0.5 + (hv - cut) as f64 / aa).clamp(0.0, 1.0) as f32
            } else if hv > cut {
                1.0
            } else {
                0.0
            };
            if m <= 0.0 {
                *px = [0.0; 4];
                continue;
            }
            // Warp the source matte with the softness-scaled bevel normal (z = 1).
            let na = norm3([-gx * matte_slope, -gy * matte_slope, 1.0]);
            let sa = src.sample_bilinear(x as f64 + 0.5 + na[0] * matte_depth, y as f64 + 0.5 + na[1] * matte_depth);
            let alpha = sa[3].max(0.0) * m;
            if alpha <= 0.0 {
                *px = [0.0; 4];
                continue;
            }
            // Refract the colour through the height field (z = height for this pass).
            let hz = (hv as f64).max(1e-3);
            let rx = (-rgb_depth * gx / hz).clamp(-limit, limit);
            let ry = (-rgb_depth * gy / hz).clamp(-limit, limit);
            let srgb = src.sample_bilinear(x as f64 + 0.5 + rx, y as f64 + 0.5 + ry);
            let (c, _) = unpremul(srgb);
            let (gx32, gy32) = (gx as f32, gy as f32);
            let n = norm3f([-gx32 * light_slope as f32, -gy32 * light_slope as f32, 1.0]);
            let l = if point {
                norm3f([lpos.0 as f32 - (x as f32 + 0.5), lpos.1 as f32 - (y as f32 + 0.5), (lheight / 100.0 * span) as f32])
            } else {
                [ldir[0] as f32, ldir[1] as f32, ldir[2] as f32]
            };
            let ndl = dot3f(n, l).max(0.0);
            let hv3 = norm3f([l[0], l[1], l[2] + 1.0]);
            let sp = dot3f(n, hv3).max(0.0).powf(shininess as f32) * specular as f32;
            let mut out = [0.0f32; 3];
            for i in 0..3 {
                let lit = (ambient + diffuse * ndl as f64 * intensity) as f32 * lc[i];
                let spec_col = (1.0 - metal) * lc[i] + metal * c[i] * lc[i];
                out[i] = c[i] * lit + (sp as f64 * intensity) as f32 * spec_col;
            }
            *px = premul(out, alpha);
        }
    });
    b
}

// ---------------------------------------------------------------- CC Power Pin

fn cc_power_pin(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let (lw, lh) = (ctx.layer_size[0].max(1.0), ctx.layer_size[1].max(1.0));
    let ex = [ctx.params.f("expandTop"), ctx.params.f("expandLeft"), ctx.params.f("expandRight"), ctx.params.f("expandBottom")];
    // Source rectangle (layer space), grown by the Expand percentages.
    let (x0, y0) = (-ex[1] / 100.0 * lw, -ex[0] / 100.0 * lh);
    let (x1, y1) = (lw + ex[2] / 100.0 * lw, lh + ex[3] / 100.0 * lh);
    let corners_layer = [ctx.params.v2("topLeft"), ctx.params.v2("topRight"), ctx.params.v2("bottomRight"), ctx.params.v2("bottomLeft")];
    let unstretch = ctx.params.b("unstretch");
    // Grow the buffer to hold the pinned quad (Unstretch writes inside the layer rectangle, so it needs no room).
    if !ctx.adjustment && !unstretch {
        let mut need = 0.0f64;
        for c in &corners_layer {
            let (px, py) = b.to_px(*c);
            need = need.max(-px).max(-py).max(px - b.img.width as f64).max(py - b.img.height as f64);
        }
        if need > 0.0 {
            b.pad(need.ceil().min(4096.0) as u32 + 1);
        }
    }
    let q = corners_layer.map(|c| {
        let p = b.to_px(c);
        vec2(p.0, p.1)
    });
    let persp = (ctx.params.f("perspective") / 100.0).clamp(0.0, 1.0);
    let fwd = Mat3::square_to_quad(q);
    let Some(inv) = fwd.inverse() else { return b };
    let src = b.img.clone();
    let (sc, off) = (b.scale, b.offset);
    let bil = move |u: f64, v: f64| -> (f64, f64) {
        let top = (q[0].x + (q[1].x - q[0].x) * u, q[0].y + (q[1].y - q[0].y) * u);
        let bot = (q[3].x + (q[2].x - q[3].x) * u, q[3].y + (q[2].y - q[3].y) * u);
        (top.0 + (bot.0 - top.0) * v, top.1 + (bot.1 - top.1) * v)
    };
    b.img = crate::util::gen_image(src.width, src.height, |x, y| {
        let (px, py) = (x as f64 + 0.5, y as f64 + 0.5);
        if unstretch {
            // Reverse pinning: the layer rectangle is the target, and the pinned quad is the region that fills it.
            let u = ((px - off[0]) / sc - x0) / (x1 - x0);
            let v = ((py - off[1]) / sc - y0) / (y1 - y0);
            if !(-1e-9..=1.0 + 1e-9).contains(&u) || !(-1e-9..=1.0 + 1e-9).contains(&v) {
                return [0.0; 4];
            }
            let m = fwd.apply(vec2(u, v));
            let (bx, by) = bil(u, v);
            return src.sample_bilinear(bx + (m.x - bx) * persp, by + (m.y - by) * persp);
        }
        let m = inv.apply(vec2(px, py));
        let (mut u, mut v) = (m.x, m.y);
        if persp < 1.0 {
            // Bilinear (non-perspective) inverse by Newton iterations from the projective guess.
            let (mut bu, mut bv) = (u, v);
            for _ in 0..8 {
                let (fx, fy) = bil(bu, bv);
                let (rx, ry) = (fx - px, fy - py);
                if rx.abs() + ry.abs() < 1e-6 {
                    break;
                }
                let e = 1e-4;
                let (ax, ay) = bil(bu + e, bv);
                let (cx, cy) = bil(bu, bv + e);
                let (j00, j10, j01, j11) = ((ax - fx) / e, (ay - fy) / e, (cx - fx) / e, (cy - fy) / e);
                let det = j00 * j11 - j01 * j10;
                if det.abs() < 1e-12 {
                    break;
                }
                bu -= (j11 * rx - j01 * ry) / det;
                bv -= (-j10 * rx + j00 * ry) / det;
            }
            u = bu + (u - bu) * persp;
            v = bv + (v - bv) * persp;
        }
        if !(-1e-9..=1.0 + 1e-9).contains(&u) || !(-1e-9..=1.0 + 1e-9).contains(&v) {
            return [0.0; 4];
        }
        let lx = x0 + u * (x1 - x0);
        let ly = y0 + v * (y1 - y0);
        src.sample_bilinear(lx * sc + off[0], ly * sc + off[1])
    });
    b
}

// ---------------------------------------------------------------- CC Ripple Pulse

fn cc_ripple_pulse(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let level = ctx.params.f("pulseLevel") / 100.0;
    let amp = ctx.params.f("amplitude") / 100.0;
    if level == 0.0 || amp == 0.0 {
        return b;
    }
    let c = b.to_px(ctx.params.v2("center"));
    let span = ctx.params.f("timeSpan").max(0.01);
    let size = ctx.layer_size[0].max(ctx.layer_size[1]) * b.scale;
    let speed = size * 0.5 / span; // the ring crosses half the layer in one time span
    let lambda = (size * 0.06).max(2.0);
    let height = level * amp * lambda * 0.5;
    let front = speed * ctx.time.max(0.0);
    let bump = ctx.params.b("renderBump");
    let wave = move |r: f64| -> f64 {
        // A pulse train trailing the front, fading with age.
        let age = (front - r) / speed.max(1e-9);
        if age < 0.0 || age > span {
            return 0.0;
        }
        let fade = 1.0 - age / span;
        (TAU * (r - front) / lambda).sin() * fade
    };
    let src = b.img.clone();
    b.img.rows_mut().for_each(|(y, row)| {
        for (x, px) in row.iter_mut().enumerate() {
            let (dx, dy) = (x as f64 + 0.5 - c.0, y as f64 + 0.5 - c.1);
            let r = (dx * dx + dy * dy).sqrt();
            let d = wave(r) * height;
            let (ux, uy) = if r > 1e-9 { (dx / r, dy / r) } else { (0.0, 0.0) };
            let mut s = src.sample_bilinear(x as f64 + 0.5 - ux * d, y as f64 + 0.5 - uy * d);
            if bump && d != 0.0 {
                let slope = (wave(r + 0.5) - wave(r - 0.5)) * height;
                let k = (1.0 - slope * 0.5).clamp(0.0, 2.0) as f32;
                for v in s.iter_mut().take(3) {
                    *v *= k;
                }
            }
            *px = s;
        }
    });
    b
}

// ---------------------------------------------------------------- CC Slant

fn cc_slant(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let k = ctx.params.f("slant") / 100.0;
    let height = ctx.params.f("height") / 100.0;
    if k == 0.0 && (height - 1.0).abs() < 1e-12 {
        return b;
    }
    let stretch = ctx.params.b("stretching");
    let xs = if stretch { 1.0 / (1.0 + k * k).sqrt() } else { 1.0 };
    let hinv = if height.abs() < 1e-6 { 1e6 } else { 1.0 / height };
    if !ctx.adjustment {
        let lean = (k * ctx.layer_size[1] * b.scale * height.max(1.0)).abs();
        let grow = ((height - 1.0).max(0.0) * ctx.layer_size[1] * b.scale).max(lean);
        b.pad(grow.ceil().min(4096.0) as u32);
    }
    let floor = b.to_px(ctx.params.v2("floor"));
    b.img = remap(&b.img, false, |x, y| {
        let sy = floor.1 + (y - floor.1) * hinv;
        let sx = floor.0 + (x - floor.0 - (floor.1 - y) * k) / xs;
        Some((sx, sy))
    });
    b
}

// ---------------------------------------------------------------- CC Smear

fn cc_smear(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let reach = ctx.params.f("reach") / 100.0;
    if !ctx.adjustment {
        b.pad(cc_smear_pad(ctx, [b.img.width, b.img.height], b.offset, b.scale));
    }
    let from = b.to_px(ctx.params.v2("from"));
    let to = b.to_px(ctx.params.v2("to"));
    let radius = (ctx.params.f("radius") * b.scale).max(0.5);
    let (dx, dy) = (to.0 - from.0, to.1 - from.1);
    let l2 = dx * dx + dy * dy;
    if l2 < 1e-12 || reach == 0.0 {
        return b;
    }
    b.img = remap(&b.img, false, |x, y| {
        let t = (((x - from.0) * dx + (y - from.1) * dy) / l2).clamp(0.0, 1.0);
        let (px, py) = (from.0 + dx * t, from.1 + dy * t);
        let d = ((x - px).powi(2) + (y - py).powi(2)).sqrt();
        let w = (1.0 - d / radius).max(0.0);
        let w = w * w * (3.0 - 2.0 * w);
        let k = reach * w * t;
        Some((x - dx * k, y - dy * k))
    });
    b
}

/// The transparent pixels to add on every side of a buffer `size` pixels big (with `offset` and
/// `scale`) for CC Smear: it moves pixels by up to Reach × From→To, and they may move out to the
/// layer bounds. A text or shape layer's buffer ends at its content while its bounds are
/// comp-sized, so the smear was cut off at the content's box (#659).
pub fn cc_smear_pad(ctx: &EffectCtx, size: [u32; 2], offset: [f64; 2], scale: f64) -> u32 {
    let reach = ctx.params.f("reach") / 100.0;
    let (from, to) = (ctx.params.v2("from"), ctx.params.v2("to"));
    let len = ((to[0] - from[0]).powi(2) + (to[1] - from[1]).powi(2)).sqrt() * scale;
    let moved = (reach.abs() * len).ceil();
    if !moved.is_finite() || moved <= 0.0 {
        return 0;
    }
    (moved.min(4096.0) as u32).min(crate::util::layer_cover_pad(ctx, size, offset, scale))
}

// ---------------------------------------------------------------- CC Split / CC Split 2

fn split_impl(ctx: &EffectCtx, mut b: Buf, s1: f64, s2: f64) -> Buf {
    if s1 == 0.0 && s2 == 0.0 {
        return b;
    }
    let a = b.to_px(ctx.params.v2("pointA"));
    let c = b.to_px(ctx.params.v2("pointB"));
    let (dx, dy) = (c.0 - a.0, c.1 - a.1);
    let len = (dx * dx + dy * dy).sqrt();
    if len < 1e-6 {
        return b;
    }
    let (ux, uy) = (dx / len, dy / len);
    let (nx, ny) = (-uy, ux);
    let reach = len * 0.5;
    b.img = remap(&b.img, false, |x, y| {
        let (rx, ry) = (x - a.0, y - a.1);
        let t = (rx * ux + ry * uy) / len;
        let s = rx * nx + ry * ny;
        if !(0.0..=1.0).contains(&t) {
            return Some((x, y));
        }
        let amt = if s >= 0.0 { s1 } else { s2 };
        let g = amt * reach * (PI * t).sin();
        let ad = s.abs();
        if ad < g {
            return None;
        }
        // Push the sides apart, fading out over `reach` beyond the gap.
        let push = g * (1.0 - (ad - g) / reach).max(0.0);
        let ns = s.signum() * (ad - push);
        Some((x + nx * (ns - s), y + ny * (ns - s)))
    });
    b
}

fn cc_split(ctx: &EffectCtx, b: Buf) -> Buf {
    let s = ctx.params.f("split") / 100.0;
    split_impl(ctx, b, s, s)
}

fn cc_split2(ctx: &EffectCtx, b: Buf) -> Buf {
    split_impl(ctx, b, ctx.params.f("split1") / 100.0, ctx.params.f("split2") / 100.0)
}

// ---------------------------------------------------------------- Smear (mask based)

/// Smear's set-up in buffer pixels (shared with the GPU kernel).
pub struct SmearSetup {
    /// Boundary outline and the source outline moved by offset / rotation / scale.
    pub bound: Vec<[f64; 2]>,
    pub moved: Vec<[f64; 2]>,
    /// Source outline centroid, rotation (cos, sin), scale, offset (buffer pixels).
    pub c: [f64; 2],
    pub cs: f64,
    pub sn: f64,
    pub scl: f64,
    pub off: [f64; 2],
    pub pct: f64,
    /// Elasticity exponent.
    pub kexp: f64,
}

/// `None` when Smear leaves the buffer as it is (a mask missing, Percent 0).
pub fn smear_setup(ctx: &EffectCtx, b: &Buf) -> Option<SmearSetup> {
    let (Some((src_m, _)), Some((bound, _))) = (mask_px(ctx, b, "sourceMask"), mask_px(ctx, b, "boundaryMask")) else { return None };
    let pct = ctx.params.f("percent") / 100.0;
    let off = ctx.params.v2("maskOffset");
    let (ox, oy) = (off[0] * b.scale, off[1] * b.scale);
    let rot = ctx.params.f("maskRotation").to_radians();
    let scl = (ctx.params.f("maskScale") / 100.0).max(1e-3);
    let kexp = elastic_exp(ctx.params.e("elasticity"));
    let c = centroid(&src_m);
    let (sn, cs) = rot.sin_cos();
    let fwd = |p: [f64; 2]| -> [f64; 2] {
        let (dx, dy) = ((p[0] - c[0]) * scl, (p[1] - c[1]) * scl);
        [c[0] + dx * cs - dy * sn + ox, c[1] + dx * sn + dy * cs + oy]
    };
    let moved: Vec<[f64; 2]> = src_m.iter().map(|&q| fwd(q)).collect();
    if pct == 0.0 {
        return None;
    }
    Some(SmearSetup { bound, moved, c, cs, sn, scl, off: [ox, oy], pct, kexp })
}

fn smear(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let Some(s) = smear_setup(ctx, &b) else { return b };
    let (c, cs, sn, scl, [ox, oy]) = (s.c, s.cs, s.sn, s.scl, s.off);
    let inv = |x: f64, y: f64| -> (f64, f64) {
        let (dx, dy) = (x - ox - c[0], y - oy - c[1]);
        (c[0] + (dx * cs + dy * sn) / scl, c[1] + (-dx * sn + dy * cs) / scl)
    };
    let (bound, moved, pct, kexp) = (&s.bound, &s.moved, s.pct, s.kexp);
    b.img = remap(&b.img, false, |x, y| {
        if !point_in_poly(bound, x, y) {
            return Some((x, y));
        }
        let w = if point_in_poly(moved, x, y) {
            1.0
        } else {
            let dm = dist_to_poly(moved, true, x, y);
            let db = dist_to_poly(bound, true, x, y);
            (db / (db + dm).max(1e-9)).powf(1.0 / kexp)
        };
        let (ix, iy) = inv(x, y);
        let k = (w * pct).clamp(0.0, 1.0);
        Some((x + (ix - x) * k, y + (iy - y) * k))
    });
    b
}

// ---------------------------------------------------------------- Reshape

/// Reshape's correspondence points: "s,d s,d …" (outline fractions, sorted by destination).
fn parse_correspondence(s: &str) -> Vec<(f64, f64)> {
    let mut v: Vec<(f64, f64)> = s
        .split_whitespace()
        .filter_map(|t| {
            let (a, b) = t.split_once(',')?;
            Some((a.trim().parse::<f64>().ok()?.rem_euclid(1.0), b.trim().parse::<f64>().ok()?.rem_euclid(1.0)))
        })
        .collect();
    v.sort_by(|a, b| a.1.total_cmp(&b.1));
    v
}

/// The source outline fraction matching destination fraction `u` (piecewise linear between
/// correspondence pairs; around the loop for closed shapes).
fn map_fraction(pairs: &[(f64, f64)], u: f64, closed: bool) -> f64 {
    let n = pairs.len();
    if n == 1 {
        return (pairs[0].0 + (u - pairs[0].1)).rem_euclid(1.0);
    }
    if !closed {
        // Open paths: clamp beyond the first / last pair (ends map to ends).
        let ext: Vec<(f64, f64)> = std::iter::once((0.0, 0.0)).chain(pairs.iter().copied()).chain(std::iter::once((1.0, 1.0))).collect();
        let k = ext.iter().rposition(|p| p.1 <= u).unwrap_or(0).min(ext.len() - 2);
        let (a, b) = (ext[k], ext[k + 1]);
        let t = if b.1 > a.1 { (u - a.1) / (b.1 - a.1) } else { 0.0 };
        return (a.0 + (b.0 - a.0) * t).clamp(0.0, 1.0);
    }
    let k = pairs.iter().rposition(|p| p.1 <= u).unwrap_or(n - 1);
    let (a, b) = (pairs[k], pairs[(k + 1) % n]);
    let du = (b.1 - a.1).rem_euclid(1.0).max(1e-9);
    let ds = (b.0 - a.0).rem_euclid(1.0);
    let t = (u - a.1).rem_euclid(1.0) / du;
    (a.0 + ds * t).rem_euclid(1.0)
}

/// Points Reshape samples each outline at.
pub const RESHAPE_POINTS: usize = 48;

/// Reshape's set-up in buffer pixels (shared with the GPU kernel).
pub struct ReshapeSetup {
    /// The destination outline's sample points and the displacement back to the source
    /// outline at each.
    pub dest: Vec<[f64; 2]>,
    pub disp: Vec<[f64; 2]>,
    pub bound: Option<Vec<[f64; 2]>>,
    /// Elasticity exponent.
    pub kexp: f64,
    pub pct: f64,
    /// Smooth interpolation of the boundary fade.
    pub smooth: bool,
}

/// `None` when Reshape leaves the buffer as it is (a mask missing, Percent 0).
pub fn reshape_setup(ctx: &EffectCtx, b: &Buf) -> Option<ReshapeSetup> {
    let (Some((sm, sc)), Some((dm, dc))) = (mask_px(ctx, b, "sourceMask"), mask_px(ctx, b, "destinationMask")) else { return None };
    let pct = ctx.params.f("percent") / 100.0;
    if pct == 0.0 {
        return None;
    }
    let bound = mask_px(ctx, b, "boundaryMask").map(|m| m.0);
    let kexp = elastic_exp(ctx.params.e("elasticity"));
    const N: usize = RESHAPE_POINTS;
    let s = resample_poly(&sm, sc, N);
    let d = resample_poly(&dm, dc, N);
    // Align the starting correspondence point (closed shapes): best cyclic rotation.
    let rot = if sc && dc {
        (0..N)
            .min_by(|&r1, &r2| {
                let cost = |r: usize| (0..N).map(|i| (s[(i + r) % N][0] - d[i][0]).powi(2) + (s[(i + r) % N][1] - d[i][1]).powi(2)).sum::<f64>();
                cost(r1).total_cmp(&cost(r2))
            })
            .unwrap_or(0)
    } else {
        0
    };
    // Correspondence points ("source,destination" pairs of outline fractions 0..1) pin which
    // point of the source outline goes to which point of the destination outline; in between
    // the outlines are matched proportionally by length. Without them the starting point is
    // chosen automatically (above).
    let pairs = parse_correspondence(ctx.params.s("correspondencePoints"));
    let src_at = |i: usize| -> [f64; 2] {
        if pairs.is_empty() {
            return s[(i + rot) % N];
        }
        let u = map_fraction(&pairs, i as f64 / N as f64, sc && dc);
        let len = crate::util::poly_length(&sm, sc);
        crate::util::poly_point_at(&sm, sc, u * len).0
    };
    let disp: Vec<[f64; 2]> = (0..N)
        .map(|i| {
            let q = src_at(i);
            [q[0] - d[i][0], q[1] - d[i][1]]
        })
        .collect();
    let smooth = ctx.params.e("interpolation") == 2;
    Some(ReshapeSetup { dest: d, disp, bound, kexp, pct, smooth })
}

fn reshape(ctx: &EffectCtx, mut b: Buf) -> Buf {
    const N: usize = RESHAPE_POINTS;
    let Some(ReshapeSetup { dest: d, disp, bound, kexp, pct, smooth }) = reshape_setup(ctx, &b) else { return b };
    b.img = remap(&b.img, false, |x, y| {
        if let Some(bd) = &bound
            && !point_in_poly(bd, x, y)
        {
            return Some((x, y));
        }
        let (mut wx, mut wy, mut ws) = (0.0, 0.0, 0.0);
        let mut dmin = f64::INFINITY;
        for i in 0..N {
            let d2 = (x - d[i][0]).powi(2) + (y - d[i][1]).powi(2);
            dmin = dmin.min(d2);
            let w = 1.0 / (d2 + 1e-6).powf(kexp * 0.5);
            wx += w * disp[i][0];
            wy += w * disp[i][1];
            ws += w;
        }
        let mut fade = 1.0;
        if let Some(bd) = &bound {
            let db = dist_to_poly(bd, true, x, y);
            let dq = dmin.sqrt();
            fade = db / (db + dq).max(1e-9);
            if smooth {
                fade = fade * fade * (3.0 - 2.0 * fade);
            }
        }
        let k = fade * pct / ws;
        Some((x + wx * k, y + wy * k))
    });
    b
}

// ---------------------------------------------------------------- Twirl (Legacy)

fn twirl_legacy(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let ang = ctx.params.f("angle").to_radians();
    if ang == 0.0 {
        return b;
    }
    let (lw, lh) = (ctx.layer_size[0] * b.scale, ctx.layer_size[1] * b.scale);
    let r = (ctx.params.f("radius") / 100.0 * lw.min(lh) * 0.5).max(1.0);
    let c = b.to_px(ctx.params.v2("center"));
    b.img = remap(&b.img, false, |x, y| {
        let (dx, dy) = (x - c.0, y - c.1);
        let d = (dx * dx + dy * dy).sqrt();
        if d >= r {
            return Some((x, y));
        }
        // The legacy twirl falls off linearly with distance from the centre.
        let a = -ang * (1.0 - d / r);
        let (s, co) = a.sin_cos();
        Some((c.0 + dx * co - dy * s, c.1 + dx * s + dy * co))
    });
    b
}

// ---------------------------------------------------------------- Warp

const WARP_STYLES: [&str; 15] =
    ["Arc", "Arc Lower", "Arc Upper", "Arch", "Bulge", "Shell Lower", "Shell Upper", "Flag", "Wave", "Fish", "Rise", "Fisheye", "Inflate", "Squeeze", "Twist"];

/// Forward warp in normalised coordinates (u across the warp axis' length, v across it, both
/// −1…1); `k` is Bend (−1…1). Returns the warped point.
fn warp_style(style: u32, k: f64, u: f64, v: f64) -> (f64, f64) {
    let bell = 1.0 - u * u;
    match style {
        0 => (u * (1.0 - 0.3 * k * v), v - k * bell),          // Arc
        1 => (u, v - k * bell * (v + 1.0) * 0.5),              // Arc Lower
        2 => (u, v - k * bell * (1.0 - v) * 0.5),              // Arc Upper
        3 => (u, v - k * bell),                                // Arch
        4 => (u, v * (1.0 + k * bell)),                        // Bulge
        5 => (u, v + k * bell * bell * (v + 1.0) * 0.5),       // Shell Lower
        6 => (u, v - k * bell * bell * (1.0 - v) * 0.5),       // Shell Upper
        7 => (u, v - 0.5 * k * (PI * u).sin()),                // Flag
        8 => (u, v - 0.5 * k * (PI * u + 0.5 * PI * v).sin()), // Wave
        9 => (u, v * (1.0 + 0.5 * k * (PI * u).sin())),        // Fish
        10 => (u, v - 0.5 * k * (0.5 * PI * u).sin()),         // Rise
        11 => {
            // Fisheye: radial bulge inside the unit circle.
            let r2 = u * u + v * v;
            if r2 >= 1.0 {
                (u, v)
            } else {
                let f = 1.0 + k * (1.0 - r2);
                (u * f, v * f)
            }
        }
        12 => (u * (1.0 + 0.5 * k * (1.0 - v * v)), v * (1.0 + 0.5 * k * bell)), // Inflate
        13 => (u * (1.0 - 0.5 * k * (1.0 - v * v)), v * (1.0 + 0.5 * k * bell)), // Squeeze
        _ => {
            // Twist: rotation decreasing towards the edge of the unit circle.
            let r = (u * u + v * v).sqrt();
            if r >= 1.0 {
                (u, v)
            } else {
                let a = k * PI * (1.0 - r);
                let (s, c) = a.sin_cos();
                (u * c - v * s, u * s + v * c)
            }
        }
    }
}

fn warp(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let k = ctx.params.f("bend") / 100.0;
    let hd = ctx.params.f("horizontalDistortion") / 100.0;
    let vd = ctx.params.f("verticalDistortion") / 100.0;
    if k == 0.0 && hd == 0.0 && vd == 0.0 {
        return b;
    }
    let (hx, hy) = (ctx.layer_size[0] * b.scale * 0.5, ctx.layer_size[1] * b.scale * 0.5);
    if !ctx.adjustment {
        let pad = ((k.abs() + hd.abs() + vd.abs()) * 0.6 * hx.max(hy)).ceil().min(4096.0);
        b.pad(pad as u32 + 1);
    }
    b.img = remap(&b.img, false, warp_inverse(ctx, &b));
    b
}

/// Warp's inverse map for buffer geometry `b` (padded): the source point of an output point
/// (buffer px), by Newton iterations on the forward warp; `None` = transparent.
fn warp_inverse(ctx: &EffectCtx, b: &Buf) -> impl Fn(f64, f64) -> Option<(f64, f64)> + Sync + use<> {
    let k = ctx.params.f("bend") / 100.0;
    let hd = ctx.params.f("horizontalDistortion") / 100.0;
    let vd = ctx.params.f("verticalDistortion") / 100.0;
    let style = ctx.params.e("warpStyle");
    let vertical = ctx.params.e("warpAxis") == 1;
    let (hx, hy) = (ctx.layer_size[0] * b.scale * 0.5, ctx.layer_size[1] * b.scale * 0.5);
    let c = b.to_px([ctx.layer_size[0] * 0.5, ctx.layer_size[1] * 0.5]);
    let (hx, hy) = (hx.max(1e-6), hy.max(1e-6));
    let fwd = move |x: f64, y: f64| -> (f64, f64) {
        let (u0, v0) = ((x - c.0) / hx, (y - c.1) / hy);
        let (u, v) = if vertical { (v0, u0) } else { (u0, v0) };
        let (wu, wv) = warp_style(style, k, u, v);
        let (mut x1, mut y1) = if vertical { (wv, wu) } else { (wu, wv) };
        // Horizontal / vertical distortion: perspective-like size change across the layer.
        y1 *= 1.0 + 0.5 * hd * x1;
        x1 *= 1.0 + 0.5 * vd * y1;
        (c.0 + x1 * hx, c.1 + y1 * hy)
    };
    move |x, y| {
        let (mut sx, mut sy) = (x, y);
        for _ in 0..12 {
            let (fx, fy) = fwd(sx, sy);
            let (rx, ry) = (fx - x, fy - y);
            if rx.abs() + ry.abs() < 1e-7 {
                break;
            }
            let e = 0.01;
            let (ax, ay) = fwd(sx + e, sy);
            let (bx, by) = fwd(sx, sy + e);
            let (j00, j10, j01, j11) = ((ax - fx) / e, (ay - fy) / e, (bx - fx) / e, (by - fy) / e);
            let det = j00 * j11 - j01 * j10;
            if det.abs() < 1e-9 {
                return None;
            }
            sx -= (j11 * rx - j01 * ry) / det;
            sy -= (-j10 * rx + j00 * ry) / det;
        }
        let (fx, fy) = fwd(sx, sy);
        if (fx - x).abs() + (fy - y).abs() > 0.5 {
            return None;
        }
        Some((sx, sy))
    }
}

/// Warp's inverse map over a `w` × `h` buffer with geometry `b` (its pixels are not read), as
/// an image of (source x, source y, 1, 0) per pixel centre, transparent where the warp leaves
/// the pixel empty: the plan of the GPU compositor's Warp where the Newton iterations must run
/// in f64 to land on the CPU's pixels (strong Fisheye / Twist bends).
pub fn warp_inverse_map(ctx: &EffectCtx, b: &Buf, w: u32, h: u32) -> Image {
    let f = warp_inverse(ctx, b);
    crate::util::gen_image(w, h, |x, y| match f(x as f64 + 0.5, y as f64 + 0.5) {
        Some((sx, sy)) => [sx as f32, sy as f32, 1.0, 0.0],
        None => [0.0; 4],
    })
}

// ---------------------------------------------------------------- Detail-preserving Upscale

/// Catmull-Rom weight.
fn cubic_w(t: f64) -> f64 {
    let t = t.abs();
    if t < 1.0 {
        1.5 * t * t * t - 2.5 * t * t + 1.0
    } else if t < 2.0 {
        -0.5 * t * t * t + 2.5 * t * t - 4.0 * t + 2.0
    } else {
        0.0
    }
}

fn bicubic(img: &Image, x: f64, y: f64) -> Px {
    let fx = x - 0.5;
    let fy = y - 0.5;
    let (x0, y0) = (fx.floor(), fy.floor());
    let (tx, ty) = (fx - x0, fy - y0);
    let wx = [cubic_w(1.0 + tx), cubic_w(tx), cubic_w(1.0 - tx), cubic_w(2.0 - tx)];
    let wy = [cubic_w(1.0 + ty), cubic_w(ty), cubic_w(1.0 - ty), cubic_w(2.0 - ty)];
    let mut o = [0.0f64; 4];
    for (j, wyj) in wy.iter().enumerate() {
        for (i, wxi) in wx.iter().enumerate() {
            let p = img.get_clamped(x0 as i64 - 1 + i as i64, y0 as i64 - 1 + j as i64);
            let w = wxi * wyj;
            for c in 0..4 {
                o[c] += p[c] as f64 * w;
            }
        }
    }
    let a = o[3].clamp(0.0, 1.0) as f32;
    [(o[0] as f32).clamp(0.0, a.max(o[0] as f32)), (o[1] as f32).clamp(0.0, a.max(o[1] as f32)), (o[2] as f32).clamp(0.0, a.max(o[2] as f32)), a]
}

fn upscale(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let k = (ctx.params.f("scale") / 100.0).clamp(1.0, 10.0);
    if (k - 1.0).abs() < 1e-9 {
        return b;
    }
    let detail = (ctx.params.f("detail") / 100.0) as f32;
    let noise = ctx.params.f("reduceNoise") / 100.0;
    let src = if noise > 0.0 { effectcraft_raster::gaussian_blur(&b.img, noise * 1.5, noise * 1.5, true) } else { b.img.clone() };
    let lc = [ctx.layer_size[0] * 0.5, ctx.layer_size[1] * 0.5];
    // Alpha: Bilinear (index 0) resamples the alpha channel bilinearly, Bicubic like the colour.
    let bilinear_alpha = ctx.params.e("alpha") == 0;
    let up = |img: &Image, x: f64, y: f64| -> Px {
        let p = bicubic(img, x, y);
        if !bilinear_alpha {
            return p;
        }
        let a = img.sample_bilinear_clamped(x, y)[3];
        let k = if p[3] > 1e-6 { a / p[3] } else { 0.0 };
        [p[0] * k, p[1] * k, p[2] * k, a]
    };
    let mut out = if ctx.adjustment {
        // Scale in place about the layer centre.
        let c = b.to_px(lc);
        let mut o = Image::new(src.width, src.height);
        o.rows_mut().for_each(|(y, row)| {
            for (x, px) in row.iter_mut().enumerate() {
                let sx = c.0 + (x as f64 + 0.5 - c.0) / k;
                let sy = c.1 + (y as f64 + 0.5 - c.1) / k;
                *px = if sx < 0.0 || sy < 0.0 || sx > src.width as f64 || sy > src.height as f64 { [0.0; 4] } else { up(&src, sx, sy) };
            }
        });
        o
    } else {
        let w = ((src.width as f64 * k).round() as u32).clamp(1, 16384);
        let h = ((src.height as f64 * k).round() as u32).clamp(1, 16384);
        let (kx, ky) = (w as f64 / src.width as f64, h as f64 / src.height as f64);
        let mut o = Image::new(w, h);
        o.rows_mut().for_each(|(y, row)| {
            for (x, px) in row.iter_mut().enumerate() {
                *px = up(&src, (x as f64 + 0.5) / kx, (y as f64 + 0.5) / ky);
            }
        });
        let c_sc = [lc[0] * b.scale, lc[1] * b.scale];
        b.offset = [b.offset[0] * kx + c_sc[0] * (kx - 1.0), b.offset[1] * ky + c_sc[1] * (ky - 1.0)];
        o
    };
    if detail > 0.0 {
        let sigma = 0.6 * k;
        let blur = effectcraft_raster::gaussian_blur(&out, sigma, sigma, true);
        out.data.par_iter_mut().zip(blur.data.par_iter()).for_each(|(p, q)| {
            let a = p[3];
            for c in 0..3 {
                p[c] = (p[c] + (p[c] - q[c]) * detail * 1.5).clamp(0.0, a.max(p[c]));
            }
        });
    }
    b.img = out;
    b
}

// ---------------------------------------------------------------- registry

pub fn specs() -> Vec<EffectSpec> {
    let pct = || slider(0.0, 100.0, 0.0, 100.0, 1);
    vec![
        spec(
            "ec.distort.ccbender",
            "CC Bender",
            vec![
                p("amount", "Amount", num(0.0), slider(-200.0, 200.0, -100.0, 100.0, 1)),
                p("style", "Style", Value::Enum(0), popup(&["Bend", "Marilyn", "Sharp", "Boxer"])),
                p("adjustToDistance", "Adjust To Distance", Value::Bool(false), ParamUi::Checkbox),
                p("top", "Top", pt(0.5, 0.0), ParamUi::Point),
                p("base", "Base", pt(0.5, 1.0), ParamUi::Point),
            ],
            cc_bender,
        ),
        spec(
            "ec.distort.ccblobbylize",
            "CC Blobbylize",
            vec![
                p("blobLayer", "Blob Layer", Value::Layer(None), ParamUi::Layer),
                p("property", "Property", Value::Enum(0), popup(&crate::util::SRC_NAMES[..8])),
                p("softness", "Softness", num(20.0), slider(0.0, 500.0, 0.0, 50.0, 1)),
                p("cutAway", "Cut Away", num(25.0), pct()),
                p("lightIntensity", "Light Intensity", num(100.0), slider(0.0, 1000.0, 0.0, 200.0, 1)),
                p("lightColor", "Light Color", col(1.0, 1.0, 1.0), ParamUi::Color),
                p("lightType", "Light Type", Value::Enum(0), popup(&["Distant Light", "Point Light"])),
                p("lightHeight", "Light Height", num(65.0), slider(0.0, 100.0, 0.0, 100.0, 1)),
                p("lightPosition", "Light Position", pt(0.5, 0.5), ParamUi::Point),
                p("lightDirection", "Light Direction", num(-45.0), ParamUi::Angle),
                p("ambient", "Ambient", num(75.0), slider(0.0, 200.0, 0.0, 100.0, 1)),
                p("diffuse", "Diffuse", num(25.0), slider(0.0, 200.0, 0.0, 100.0, 1)),
                p("specular", "Specular", num(100.0), slider(0.0, 200.0, 0.0, 100.0, 1)),
                p("roughness", "Roughness", num(0.025), slider(0.001, 1.0, 0.001, 1.0, 3)),
                p("metal", "Metal", num(100.0), pct()),
            ],
            cc_blobbylize,
        ),
        spec(
            "ec.distort.ccpowerpin",
            "CC Power Pin",
            vec![
                p("topLeft", "Top Left", pt(0.0, 0.0), ParamUi::Point),
                p("topRight", "Top Right", pt(1.0, 0.0), ParamUi::Point),
                p("bottomLeft", "Bottom Left", pt(0.0, 1.0), ParamUi::Point),
                p("bottomRight", "Bottom Right", pt(1.0, 1.0), ParamUi::Point),
                p("perspective", "Perspective", num(100.0), pct()),
                p("unstretch", "Unstretch", Value::Bool(false), ParamUi::Checkbox),
                p("expandTop", "Top", num(0.0), slider(-100.0, 1000.0, 0.0, 100.0, 1)),
                p("expandLeft", "Left", num(0.0), slider(-100.0, 1000.0, 0.0, 100.0, 1)),
                p("expandRight", "Right", num(0.0), slider(-100.0, 1000.0, 0.0, 100.0, 1)),
                p("expandBottom", "Bottom", num(0.0), slider(-100.0, 1000.0, 0.0, 100.0, 1)),
            ],
            cc_power_pin,
        ),
        spec(
            "ec.distort.ccripplepulse",
            "CC Ripple Pulse",
            vec![
                p("center", "Center", pt(0.5, 0.5), ParamUi::Point),
                p("pulseLevel", "Pulse Level (animate)", num(0.0), slider(-1000.0, 1000.0, -100.0, 100.0, 1)),
                p("timeSpan", "Time Span (sec)", num(1.0), slider(0.01, 100.0, 0.01, 10.0, 2)),
                p("amplitude", "Amplitude", num(100.0), slider(-1000.0, 1000.0, -200.0, 200.0, 1)),
                p("renderBump", "Render Bump As Well", Value::Bool(false), ParamUi::Checkbox),
            ],
            cc_ripple_pulse,
        ),
        spec(
            "ec.distort.ccslant",
            "CC Slant",
            vec![
                p("slant", "Slant", num(0.0), slider(-1000.0, 1000.0, -100.0, 100.0, 1)),
                p("stretching", "Stretching", Value::Bool(false), ParamUi::Checkbox),
                p("height", "Height", num(100.0), slider(-1000.0, 1000.0, 0.0, 200.0, 1)),
                p("floor", "Floor", pt(0.5, 1.0), ParamUi::Point),
            ],
            cc_slant,
        ),
        spec(
            "ec.distort.ccsmear",
            "CC Smear",
            vec![
                p("from", "From", pt(0.35, 0.5), ParamUi::Point),
                p("to", "To", pt(0.65, 0.5), ParamUi::Point),
                p("reach", "Reach", num(50.0), slider(-100.0, 100.0, -100.0, 100.0, 1)),
                p("radius", "Radius", num(50.0), slider(0.0, 4000.0, 0.0, 400.0, 1)),
            ],
            cc_smear,
        ),
        spec(
            "ec.distort.ccsplit",
            "CC Split",
            vec![
                p("pointA", "Point A", pt(0.25, 0.5), ParamUi::Point),
                p("pointB", "Point B", pt(0.75, 0.5), ParamUi::Point),
                p("split", "Split", num(50.0), slider(0.0, 1000.0, 0.0, 200.0, 1)),
            ],
            cc_split,
        ),
        spec(
            "ec.distort.ccsplit2",
            "CC Split 2",
            vec![
                p("pointA", "Point A", pt(0.25, 0.5), ParamUi::Point),
                p("pointB", "Point B", pt(0.75, 0.5), ParamUi::Point),
                p("split1", "Split 1", num(50.0), slider(0.0, 1000.0, 0.0, 200.0, 1)),
                p("split2", "Split 2", num(50.0), slider(0.0, 1000.0, 0.0, 200.0, 1)),
            ],
            cc_split2,
        ),
        spec(
            "ec.distort.smear",
            "Smear",
            vec![
                p("sourceMask", "Source Mask", Value::Enum(0), popup(&MASK_CHOICES)),
                p("boundaryMask", "Boundary Mask", Value::Enum(0), popup(&MASK_CHOICES)),
                p("maskOffset", "Mask Offset", pt(0.0, 0.0), ParamUi::Point),
                p("maskRotation", "Mask Rotation", num(0.0), ParamUi::Angle),
                p("maskScale", "Mask Scale", num(100.0), slider(1.0, 1000.0, 1.0, 400.0, 1)),
                p("percent", "Percent", num(100.0), pct()),
                p("elasticity", "Elasticity", Value::Enum(3), popup(&ELASTICITY)),
                p("interpolation", "Interpolation Method", Value::Enum(1), popup(&["Discrete", "Linear", "Smooth"])),
            ],
            smear,
        ),
        spec(
            "ec.distort.reshape",
            "Reshape",
            vec![
                p("sourceMask", "Source Mask", Value::Enum(0), popup(&MASK_CHOICES)),
                p("destinationMask", "Destination Mask", Value::Enum(0), popup(&MASK_CHOICES)),
                p("boundaryMask", "Boundary Mask", Value::Enum(0), popup(&MASK_CHOICES)),
                p("percent", "Percent", num(100.0), pct()),
                p("elasticity", "Elasticity", Value::Enum(0), popup(&ELASTICITY)),
                p("interpolation", "Interpolation Method", Value::Enum(1), popup(&["Discrete", "Linear", "Smooth"])),
                // Correspondence points: "source,destination" outline fractions (0..1) per point.
                p("correspondencePoints", "Correspondence Points", Value::Str(String::new()), ParamUi::Hidden),
            ],
            reshape,
        ),
        spec(
            "ec.distort.twirllegacy",
            "Twirl (Legacy)",
            vec![
                p("angle", "Angle", num(0.0), ParamUi::Angle),
                p("radius", "Twirl Radius", num(30.0), slider(0.0, 100.0, 0.0, 100.0, 1)),
                p("center", "Twirl Center", pt(0.5, 0.5), ParamUi::Point),
            ],
            twirl_legacy,
        ),
        spec(
            "ec.distort.warp",
            "Warp",
            vec![
                p("warpStyle", "Warp Style", Value::Enum(0), popup(&WARP_STYLES)),
                p("warpAxis", "Warp Axis", Value::Enum(0), popup(&["Horizontal", "Vertical"])),
                p("bend", "Bend", num(0.0), slider(-100.0, 100.0, -100.0, 100.0, 0)),
                p("horizontalDistortion", "Horizontal Distortion", num(0.0), slider(-100.0, 100.0, -100.0, 100.0, 0)),
                p("verticalDistortion", "Vertical Distortion", num(0.0), slider(-100.0, 100.0, -100.0, 100.0, 0)),
            ],
            warp,
        ),
        spec(
            "ec.distort.upscale",
            "Detail-preserving Upscale",
            vec![
                p("scale", "Scale", num(100.0), slider(100.0, 1000.0, 100.0, 400.0, 1)),
                p("reduceNoise", "Reduce Noise", num(0.0), pct()),
                p("detail", "Detail", num(20.0), pct()),
                p("alpha", "Alpha", Value::Enum(1), popup(&["Bilinear", "Bicubic"])),
            ],
            upscale,
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EffectEnv, MaskShape, Params};

    fn img(w: u32, h: u32) -> Image {
        crate::util::gen_image(w, h, |x, y| [x as f32 / w as f32, y as f32 / h as f32, ((x * 7 + y * 3) % 11) as f32 / 11.0, 1.0])
    }

    /// Run with Point defaults scaled to the layer (like `instantiate`).
    fn run(id: &str, set: &[(&str, Value)], im: Image, masks: &[MaskShape]) -> Buf {
        let s = crate::find(id).unwrap();
        let ls = [im.width as f64, im.height as f64];
        let mut params = Params { values: s.params.iter().map(|p| (p.id.to_string(), p.default.clone())).collect() };
        for ps in &s.params {
            if let (ParamUi::Point, Value::Vec2(v)) = (&ps.ui, &ps.default) {
                params.values.insert(ps.id.to_string(), Value::Vec2([v[0] * ls[0], v[1] * ls[1]]));
            }
        }
        for (k, v) in set {
            assert!(params.values.contains_key(*k), "{id}: {k}");
            params.values.insert(k.to_string(), v.clone());
        }
        let env = EffectEnv { masks, ..Default::default() };
        let ctx = EffectCtx { params: &params, time: 0.5, layer_size: ls, seed: 1, adjustment: false, env };
        crate::apply(s, &ctx, Buf { img: im, offset: [0.0, 0.0], scale: 1.0 })
    }

    fn maxd(a: &Image, b: &Image) -> f32 {
        assert_eq!((a.width, a.height), (b.width, b.height));
        a.data.iter().zip(&b.data).map(|(p, q)| (0..4).map(|i| (p[i] - q[i]).abs()).fold(0.0, f32::max)).fold(0.0, f32::max)
    }

    fn rect_mask(x0: f64, y0: f64, x1: f64, y1: f64) -> MaskShape {
        MaskShape { name: "m".into(), points: vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]], closed: true, inverted: false }
    }

    #[test]
    fn identity_at_neutral_settings() {
        let im = img(40, 30);
        let cases: &[(&str, Vec<(&str, Value)>)] = &[
            ("ec.distort.ccbender", vec![]),
            ("ec.distort.ccpowerpin", vec![]),
            ("ec.distort.ccripplepulse", vec![]),
            ("ec.distort.ccslant", vec![]),
            ("ec.distort.ccsmear", vec![("reach", num(0.0))]),
            ("ec.distort.ccsplit", vec![("split", num(0.0))]),
            ("ec.distort.ccsplit2", vec![("split1", num(0.0)), ("split2", num(0.0))]),
            ("ec.distort.smear", vec![]),
            ("ec.distort.reshape", vec![]),
            ("ec.distort.twirllegacy", vec![]),
            ("ec.distort.warp", vec![("warpStyle", Value::Enum(5))]),
            ("ec.distort.upscale", vec![]),
        ];
        for (id, set) in cases {
            let o = run(id, set, im.clone(), &[]);
            assert!(maxd(&o.img, &im) < 1e-5, "{id} not identity");
            assert_eq!(o.offset, [0.0, 0.0], "{id}");
        }
        // Power pin with perspective 0 at default corners is also identity.
        let o = run("ec.distort.ccpowerpin", &[("perspective", num(0.0))], im.clone(), &[]);
        assert!(maxd(&o.img, &im) < 1e-4);
    }

    #[test]
    fn all_deterministic() {
        let im = img(36, 28);
        let masks = [rect_mask(8.0, 8.0, 16.0, 16.0), rect_mask(12.0, 10.0, 22.0, 20.0), rect_mask(2.0, 2.0, 34.0, 26.0)];
        let cases: &[(&str, Vec<(&str, Value)>)] = &[
            ("ec.distort.ccbender", vec![("amount", num(40.0)), ("style", Value::Enum(3))]),
            ("ec.distort.ccblobbylize", vec![]),
            ("ec.distort.ccpowerpin", vec![("topLeft", pt(5.0, 3.0)), ("perspective", num(40.0))]),
            ("ec.distort.ccripplepulse", vec![("pulseLevel", num(80.0))]),
            ("ec.distort.ccslant", vec![("slant", num(30.0)), ("stretching", Value::Bool(true))]),
            ("ec.distort.ccsmear", vec![]),
            ("ec.distort.ccsplit", vec![]),
            ("ec.distort.ccsplit2", vec![("split2", num(10.0))]),
            ("ec.distort.smear", vec![("sourceMask", Value::Enum(1)), ("boundaryMask", Value::Enum(3)), ("maskOffset", pt(4.0, 2.0))]),
            ("ec.distort.reshape", vec![("sourceMask", Value::Enum(1)), ("destinationMask", Value::Enum(2)), ("boundaryMask", Value::Enum(3))]),
            ("ec.distort.twirllegacy", vec![("angle", num(90.0))]),
            ("ec.distort.warp", vec![("bend", num(50.0)), ("horizontalDistortion", num(20.0))]),
            ("ec.distort.upscale", vec![("scale", num(200.0))]),
        ];
        for (id, set) in cases {
            let a = run(id, set, im.clone(), &masks);
            let b = run(id, set, im.clone(), &masks);
            assert_eq!(a.img, b.img, "{id}");
            assert!(a.img.data.iter().all(|p| p.iter().all(|v| v.is_finite())), "{id}");
            assert!(a.img.width != im.width || a.img.height != im.height || maxd(&a.img, &im) > 1e-3, "{id} did nothing");
        }
    }

    #[test]
    fn blobbylize_defaults_match_ae() {
        let s = crate::find("ec.distort.ccblobbylize").unwrap();
        let default = |id: &str| s.params.iter().find(|p| p.id == id).unwrap().default.clone();
        assert_eq!(default("property"), Value::Enum(0), "Property defaults to Red");
        assert_eq!(default("softness"), num(20.0));
        assert_eq!(default("cutAway"), num(25.0));
        assert_eq!(default("lightHeight"), num(65.0));
        assert_eq!(default("lightPosition"), pt(0.5, 0.5));
        assert_eq!(default("ambient"), num(75.0));
        assert_eq!(default("diffuse"), num(25.0));
        assert_eq!(default("specular"), num(100.0));
        assert_eq!(default("roughness"), num(0.025));
        assert_eq!(default("metal"), num(100.0));
    }

    #[test]
    fn blobbylize_cut_away_thresholds_a_height_ramp() {
        // AE oracle: Cut Away is a linear height threshold with a ~1 px antialiased edge.
        let im = crate::util::gen_image(1920, 8, |x, _| {
            let h = x as f32 / 1920.0;
            [h, h, h, 1.0]
        });
        let o = run("ec.distort.ccblobbylize", &[("property", Value::Enum(0)), ("softness", num(0.0)), ("cutAway", num(25.0))], im, &[]);
        assert_eq!(o.img.get(400, 4)[3], 0.0, "below Cut Away must be transparent");
        assert_eq!(o.img.get(560, 4)[3], 1.0, "above Cut Away must be opaque");
        let edge = o.img.get(480, 4)[3];
        assert!((0.45..=0.55).contains(&edge), "edge alpha {edge} is not a one-pixel threshold");
        let partial = (450..510).filter(|&x| (0.001..0.999).contains(&o.img.get(x, 4)[3])).count();
        assert!(partial <= 2, "{partial} antialiased columns instead of one");
    }

    #[test]
    fn blobbylize_cut_away_100_is_empty() {
        // Regression: the old clamp to 0.99 left the top of the ramp opaque at Cut Away 100.
        let im = crate::util::gen_image(1920, 8, |x, _| {
            let h = x as f32 / 1920.0;
            [h, h, h, 1.0]
        });
        let o = run("ec.distort.ccblobbylize", &[("property", Value::Enum(0)), ("softness", num(0.0)), ("cutAway", num(100.0))], im, &[]);
        assert!(o.img.data.iter().all(|p| p[3] == 0.0), "Cut Away 100 left opaque pixels");
    }

    #[test]
    fn blobbylize_cut_away_100_clears_an_opaque_edge() {
        // An opaque source reaches h = 1 on the high side of an edge; the gradient antialias
        // used to leave m = 0.5 there, so Cut Away 100 still produced visible pixels.
        let im = crate::util::gen_image(64, 8, |x, _| if x < 32 { [1.0; 4] } else { [0.0; 4] });
        let o = run("ec.distort.ccblobbylize", &[("property", Value::Enum(3)), ("softness", num(0.0)), ("cutAway", num(100.0))], im, &[]);
        assert!(o.img.data.iter().all(|p| p[3] == 0.0), "Cut Away 100 left an opaque edge");
    }

    #[test]
    fn blobbylize_softness_warps_a_step_matte_inward() {
        // AE oracle T2: a self alpha step at x = 960, Softness 20, Cut Away 0 puts the matte
        // edge at about x = 946, not at the source edge. The PR's matte-preserving version
        // left it at 960; the effect must warp the source matte.
        let im = crate::util::gen_image(1920, 32, |x, _| if x < 960 { [1.0; 4] } else { [0.0; 4] });
        let o = run(
            "ec.distort.ccblobbylize",
            &[
                ("property", Value::Enum(3)),
                ("softness", num(20.0)),
                ("cutAway", num(0.0)),
                ("ambient", num(100.0)),
                ("diffuse", num(0.0)),
                ("specular", num(0.0)),
                ("metal", num(0.0)),
            ],
            im,
            &[],
        );
        assert!(o.img.get(945, 16)[3] > 0.95, "the matte did not move inward: {}", o.img.get(945, 16)[3]);
        assert!(o.img.get(949, 16)[3] < 0.05, "source alpha leaked past the warped edge");
        assert_eq!(o.img.get(960, 16)[3], 0.0, "the source matte was preserved instead of warped");
    }

    /// A text layer's buffer: 20 × 10 opaque pixels at (90, 45)–(110, 55) of its comp-sized
    /// (200 × 100) layer bounds, after `id` with `set`.
    fn on_text(id: &str, set: &[(&str, Value)]) -> Buf {
        let s = crate::find(id).unwrap();
        let mut params = Params { values: s.params.iter().map(|p| (p.id.to_string(), p.default.clone())).collect() };
        for (k, v) in set {
            assert!(params.values.contains_key(*k), "{id}: {k}");
            params.values.insert(k.to_string(), v.clone());
        }
        let mut im = Image::new(20, 10);
        im.data.iter_mut().for_each(|p| *p = [1.0; 4]);
        let ctx = EffectCtx { params: &params, time: 0.0, layer_size: [200.0, 100.0], seed: 1, adjustment: false, env: Default::default() };
        crate::apply(s, &ctx, Buf { img: im, offset: [-90.0, -45.0], scale: 1.0 })
    }

    /// Alpha at layer point `p` (0 outside the buffer).
    fn alpha_at(b: &Buf, p: [f64; 2]) -> f32 {
        let (x, y) = b.to_px(p);
        b.img.get(x.floor() as i64, y.floor() as i64)[3]
    }

    /// #659: CC Smear and CC Tiler on a text layer reach past the text's box to the layer bounds
    /// (comp-sized, as in After Effects), not cut off where the glyphs end.
    #[test]
    fn effects_on_text_reach_past_its_box() {
        let smear = on_text("ec.distort.ccsmear", &[("from", pt(100.0, 50.0)), ("to", pt(140.0, 50.0)), ("reach", num(100.0)), ("radius", num(30.0))]);
        assert!(alpha_at(&smear, [130.0, 50.0]) > 0.9, "the smear is cut off at the text's box");
        assert!(smear.img.width < 200, "it grows only as far as pixels move");
        let tiles = on_text("ec.distort.cctiler", &[("scale", num(50.0)), ("center", pt(100.0, 50.0))]);
        assert!(alpha_at(&tiles, [100.0, 50.0]) > 0.9);
        assert!(alpha_at(&tiles, [2.0, 50.0]) > 0.9, "tiles fill the layer bounds");
        assert!(alpha_at(&tiles, [50.0, 50.0]) < 0.1);
        // A footage layer's buffer is its bounds already: no change.
        let o = run("ec.distort.ccsmear", &[], img(40, 30), &[]);
        assert_eq!((o.img.width, o.img.height, o.offset), (40, 30, [0.0, 0.0]));
    }

    #[test]
    fn every_warp_style_runs_and_bends() {
        let im = img(40, 30);
        for s in 0..WARP_STYLES.len() as u32 {
            for axis in 0..2 {
                let o = run("ec.distort.warp", &[("warpStyle", Value::Enum(s)), ("warpAxis", Value::Enum(axis)), ("bend", num(50.0))], im.clone(), &[]);
                let pad = o.offset[0] as u32;
                // The layer centre's neighbourhood is still made of source pixels.
                assert!(o.img.get(pad as i64 + 20, pad as i64 + 15)[3] > 0.5, "style {s}");
                assert!(o.img.width > im.width);
            }
        }
        // Arc with positive bend raises the middle of the layer: the top edge in the middle
        // moves up (opaque pixels above the original top).
        let o = run("ec.distort.warp", &[("warpStyle", Value::Enum(3)), ("bend", num(50.0))], im.clone(), &[]);
        let pad = o.offset[1] as i64;
        assert!(o.img.get(o.offset[0] as i64 + 20, pad - 3)[3] > 0.5);
        assert!(o.img.get(o.offset[0] as i64 + 1, pad - 3)[3] < 0.5);
    }

    #[test]
    fn power_pin_moves_corners() {
        let im = img(20, 20);
        // Pin to the right half: output x in 10..20 shows the whole layer squeezed.
        let set = [("topLeft", pt(10.0, 0.0)), ("bottomLeft", pt(10.0, 20.0))];
        let o = run("ec.distort.ccpowerpin", &set, im.clone(), &[]);
        assert_eq!(o.img.get(3, 10)[3], 0.0);
        let px = o.img.get(15, 10);
        assert!((px[0] - 0.5).abs() < 0.1, "{px:?}");
    }

    #[test]
    fn power_pin_unstretch_expands_pinned_region() {
        let im = crate::util::gen_image(24, 16, |_, _| [1.0, 0.0, 0.0, 1.0]);
        let inset = [("topLeft", pt(6.0, 4.0)), ("topRight", pt(18.0, 4.0)), ("bottomRight", pt(18.0, 12.0)), ("bottomLeft", pt(6.0, 12.0))];
        let opaque = |o: &Buf| o.img.data.iter().filter(|p| p[3] > 0.99).count();
        let off = run("ec.distort.ccpowerpin", &inset, im.clone(), &[]);
        assert_eq!(opaque(&off), 96);
        let mut set = inset.to_vec();
        set.push(("unstretch", Value::Bool(true)));
        let on = run("ec.distort.ccpowerpin", &set, im.clone(), &[]);
        assert_eq!(opaque(&on), 24 * 16);
        assert!(on.img.get(0, 0)[3] > 0.99);
        // Default corners are the identity either way.
        let id = run("ec.distort.ccpowerpin", &[("unstretch", Value::Bool(true))], im.clone(), &[]);
        assert!(maxd(&id.img, &im) < 1e-4);
    }

    #[test]
    fn split_opens_a_gap() {
        let im = img(40, 30);
        let o = run("ec.distort.ccsplit", &[("split", num(40.0))], im.clone(), &[]);
        assert_eq!(o.img.get(20, 15)[3], 0.0);
        assert_eq!(o.img.get(2, 2), im.get(2, 2));
    }

    #[test]
    fn slant_shears_from_floor() {
        let im = img(20, 20);
        let o = run("ec.distort.ccslant", &[("slant", num(50.0))], im.clone(), &[]);
        let pad = o.offset[0] as i64;
        // Bottom row stays put; the top row shifts right by ~10 px.
        assert!((o.img.get(pad + 5, pad + 19)[0] - im.get(5, 19)[0]).abs() < 0.05);
        assert!((o.img.get(pad + 15, pad)[0] - im.get(5, 0)[0]).abs() < 0.08);
    }

    #[test]
    fn reshape_moves_source_to_destination() {
        // A bright square at the source mask; reshape to a shifted destination mask.
        let im = crate::util::gen_image(40, 40, |x, y| if (10..20).contains(&x) && (10..20).contains(&y) { [1.0; 4] } else { [0.0, 0.0, 0.0, 1.0] });
        let masks = [rect_mask(10.0, 10.0, 20.0, 20.0), rect_mask(16.0, 10.0, 26.0, 20.0), rect_mask(0.0, 0.0, 40.0, 40.0)];
        let o = run("ec.distort.reshape", &[("sourceMask", Value::Enum(1)), ("destinationMask", Value::Enum(2)), ("boundaryMask", Value::Enum(3))], im, &masks);
        assert!(o.img.get(24, 15)[0] > 0.8, "{:?}", o.img.get(24, 15));
        assert!(o.img.get(11, 15)[0] < 0.3, "{:?}", o.img.get(11, 15));
    }

    #[test]
    fn reshape_correspondence_points_steer_the_match() {
        // Matching outlines with a zero pair change nothing; a quarter-turn offset twists.
        let im = img(40, 40);
        let masks = [rect_mask(10.0, 10.0, 30.0, 30.0), rect_mask(10.0, 10.0, 30.0, 30.0), rect_mask(0.0, 0.0, 40.0, 40.0)];
        let base = [("sourceMask", Value::Enum(1)), ("destinationMask", Value::Enum(2)), ("boundaryMask", Value::Enum(3))];
        let same = run("ec.distort.reshape", &[base.as_slice(), &[("correspondencePoints", Value::Str("0,0".into()))]].concat(), im.clone(), &masks);
        assert!(maxd(&same.img, &im) < 1e-3, "matching outlines: no change");
        let twisted = run("ec.distort.reshape", &[base.as_slice(), &[("correspondencePoints", Value::Str("0.25,0".into()))]].concat(), im.clone(), &masks);
        assert!(maxd(&twisted.img, &im) > 0.1);
        // Fraction mapping.
        assert!((map_fraction(&[(0.0, 0.0), (0.5, 0.25)], 0.125, true) - 0.25).abs() < 1e-9);
        assert!((map_fraction(&[(0.2, 0.0)], 0.9, true) - 0.1).abs() < 1e-9);
        assert!((map_fraction(&[(0.5, 0.5)], 0.25, false) - 0.25).abs() < 1e-9);
    }

    #[test]
    fn upscale_grows_about_centre() {
        let im = img(20, 10);
        let o = run("ec.distort.upscale", &[("scale", num(200.0)), ("detail", num(0.0))], im.clone(), &[]);
        assert_eq!((o.img.width, o.img.height), (40, 20));
        // Grown about the layer centre: layer point (0,0) now sits at buffer pixel (10,5), so the
        // centre (10,5) lands in the middle of the 40×20 buffer.
        assert_eq!(o.offset, [10.0, 5.0]);
        let c = o.img.sample_bilinear(20.0, 10.0);
        assert!((c[0] - im.sample_bilinear(10.0, 5.0)[0]).abs() < 0.03);
    }

    #[test]
    fn twirl_legacy_rotates_inside_radius() {
        let im = img(40, 40);
        let o = run("ec.distort.twirllegacy", &[("angle", num(180.0)), ("radius", num(100.0))], im.clone(), &[]);
        assert!(maxd(&o.img, &im) > 0.1);
        // Corners lie outside the radius.
        assert_eq!(o.img.get(0, 0), im.get(0, 0));
    }

    #[test]
    fn half_resolution_matches_scaled_geometry() {
        // Bender at scale 0.5: the buffer is half size, parameters in layer pixels.
        let s = crate::find("ec.distort.ccbender").unwrap();
        let mut params = Params { values: s.params.iter().map(|p| (p.id.to_string(), p.default.clone())).collect() };
        params.values.insert("amount".into(), num(30.0));
        params.values.insert("top".into(), pt(20.0, 0.0));
        params.values.insert("base".into(), pt(20.0, 40.0));
        let ctx = EffectCtx { params: &params, time: 0.0, layer_size: [40.0, 40.0], seed: 1, adjustment: false, env: Default::default() };
        let full = crate::apply(s, &ctx, Buf { img: img(40, 40), offset: [0.0; 2], scale: 1.0 });
        let half = crate::apply(s, &ctx, Buf { img: effectcraft_raster::resample(&img(40, 40), 20, 20), offset: [0.0; 2], scale: 0.5 });
        let a = full.img.sample_bilinear(20.0, 4.0)[3];
        let b = half.img.sample_bilinear(10.0, 2.0)[3];
        assert!((a - b).abs() < 0.3);
    }
}

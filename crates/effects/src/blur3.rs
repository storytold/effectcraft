//! Blur & Sharpen: Variable Radial Blur and Camera-Shake Deblur.
//!
//! * **Variable Radial Blur** — zoom (straight, fading, centred both ways) and rotational (plain or
//!   fading) blurs around a centre point, sampled along the ray / arc through each pixel with a
//!   sample count set by Quality.
//! * **Camera-Shake Deblur** — frame-based restoration in the spirit of published video deblurring
//!   by "lucky frames": the frame is split into patches; for each patch the sharpness (mean
//!   squared gradient) is compared with the same patch in the neighbouring frames within Blur
//!   Duration; when a neighbour is clearly sharper, its patch is aligned to the current frame by
//!   a small translation search (sum of squared differences on luma) and substituted with
//!   feathered patch blending. Deterministic; neighbours come from [`crate::EffectHost::self_at`].

use effectcraft_keyframe::Value;
use effectcraft_project::ParamUi;
use effectcraft_raster::{Image, Px};
use rayon::prelude::*;

use crate::util::Plane;
use crate::{Buf, EffectCtx, EffectSpec, num, p, popup, slider};

fn spec(id: &'static str, name: &'static str, params: Vec<crate::ParamSpec>, render: crate::RenderFn) -> EffectSpec {
    EffectSpec { id, name, category: "Blur & Sharpen", params, render, gpu: false, float: true }
}

// ---------------------------------------------------------------- Variable Radial Blur

pub const RADIAL_TYPES: [&str; 5] = ["Straight Zoom", "Fading Zoom", "Centered", "Rotate", "Rotate Fading"];

fn cc_radial_blur(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let amount = ctx.params.f("amount");
    if amount == 0.0 {
        return b;
    }
    let kind = ctx.params.e("type");
    let quality = ctx.params.f("quality").clamp(1.0, 100.0);
    let c = b.to_px(ctx.params.v2("center"));
    let src = b.img.clone();
    // Zoom: fraction of the distance to the centre; rotate: degrees of arc.
    let zoom = (amount / 100.0).clamp(-1.0, 1.0) * 0.5;
    let arc = amount.to_radians() * 0.25;
    b.img.rows_mut().for_each(|(y, row)| {
        for (x, o) in row.iter_mut().enumerate() {
            let (dx, dy) = (x as f64 + 0.5 - c.0, y as f64 + 0.5 - c.1);
            let r = (dx * dx + dy * dy).sqrt();
            let span = if kind >= 3 { r * arc.abs() } else { r * zoom.abs() };
            let n = ((span * quality / 50.0).ceil() as usize).clamp(1, 256);
            if n <= 1 {
                *o = src.get(x as i64, y as i64);
                continue;
            }
            let mut acc = [0.0f32; 4];
            let mut wsum = 0.0f32;
            for i in 0..n {
                let t = i as f64 / (n - 1) as f64;
                let (sx, sy, w) = match kind {
                    // Toward the centre, equal weights.
                    0 => {
                        let k = 1.0 - zoom * t;
                        (c.0 + dx * k, c.1 + dy * k, 1.0)
                    }
                    // Toward the centre, weights fading with distance along the streak.
                    1 => {
                        let k = 1.0 - zoom * t;
                        (c.0 + dx * k, c.1 + dy * k, (1.0 - t) as f32 + 0.05)
                    }
                    // Both ways around the pixel.
                    2 => {
                        let k = 1.0 + zoom * (t - 0.5);
                        (c.0 + dx * k, c.1 + dy * k, 1.0)
                    }
                    _ => {
                        let a = arc * (t - 0.5);
                        let (s, co) = a.sin_cos();
                        let w = if kind == 4 { (1.0 - (2.0 * t - 1.0).abs()) as f32 + 0.05 } else { 1.0 };
                        (c.0 + dx * co - dy * s, c.1 + dx * s + dy * co, w)
                    }
                };
                let q = src.sample_bilinear(sx, sy);
                for k in 0..4 {
                    acc[k] += q[k] * w;
                }
                wsum += w;
            }
            *o = acc.map(|v| v / wsum);
        }
    });
    b
}

// ---------------------------------------------------------------- Camera-Shake Deblur

/// Mean squared luma gradient over a rectangle (sharpness).
fn sharpness(l: &Plane, x0: usize, y0: usize, x1: usize, y1: usize) -> f64 {
    let mut s = 0.0f64;
    let mut n = 0usize;
    for y in y0..y1.saturating_sub(1) {
        for x in x0..x1.saturating_sub(1) {
            let v = l.get(x, y);
            let gx = l.get(x + 1, y) - v;
            let gy = l.get(x, y + 1) - v;
            s += (gx * gx + gy * gy) as f64;
            n += 1;
        }
    }
    if n == 0 { 0.0 } else { s / n as f64 }
}

/// Best integer translation (dx, dy) within ±`r` aligning `b` to `a` on a patch (SSD).
fn align(a: &Plane, bp: &Plane, x0: usize, y0: usize, x1: usize, y1: usize, r: i64) -> (i64, i64) {
    let mut best = (f64::INFINITY, 0i64, 0i64);
    let step = ((x1 - x0).max(y1 - y0) / 24).max(1);
    for dy in -r..=r {
        for dx in -r..=r {
            let mut s = 0.0f64;
            for y in (y0..y1).step_by(step) {
                for x in (x0..x1).step_by(step) {
                    let d = a.get(x, y) - bp.get_clamped(x as i64 + dx, y as i64 + dy);
                    s += (d * d) as f64;
                }
            }
            // Prefer the smallest shift on ties.
            let s = s * (1.0 + 1e-9 * (dx * dx + dy * dy) as f64);
            if s < best.0 {
                best = (s, dx, dy);
            }
        }
    }
    (best.1, best.2)
}

fn camera_shake_deblur(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let Some(host) = ctx.env.host else { return b };
    let dur = ctx.params.f("blurDuration").clamp(1.0, 10.0).round() as i64;
    // Standard (index 1) aligns substitutes over a smaller search window than High Quality.
    let mut search = (ctx.params.f("searchRange").max(0.0) * b.scale).round() as i64;
    if ctx.params.e("deblurMethod") == 1 {
        search /= 2;
    }
    let patch = (ctx.params.f("patchSize").max(8.0) * b.scale).round().max(4.0) as usize;
    // Shake Sensitivity scales how much sharper a neighbour must be (50 = the Threshold as set,
    // 100 = any sharper neighbour counts, 0 = twice the Threshold).
    let sens = ctx.params.f("shakeSensitivity").clamp(0.0, 100.0);
    let thresh = 1.0 + ctx.params.f("threshold").max(0.0) / 100.0 * (100.0 - sens) / 50.0;
    let strength = (ctx.params.f("strength") / 100.0).clamp(1.0, 2.0) as f32;
    let show = ctx.params.b("showBlurryFrames");
    let fps = ctx.fps();
    let (w, h) = (b.img.width as usize, b.img.height as usize);
    if w == 0 || h == 0 {
        return b;
    }
    // Neighbouring frames in the effect's own input, resampled into this buffer's grid.
    let mut neigh: Vec<(Image, Plane)> = vec![];
    for k in (-dur..=dur).filter(|k| *k != 0) {
        let Some(nb) = host.self_at(ctx.time + k as f64 / fps, ctx.env.effect_index) else { continue };
        let img = if nb.img.width == b.img.width && nb.img.height == b.img.height && nb.offset == b.offset && nb.scale == b.scale {
            nb.img
        } else {
            let k = nb.scale / b.scale.max(1e-9);
            crate::util::gen_image(b.img.width, b.img.height, |x, y| {
                let lx = (x as f64 + 0.5 - b.offset[0]) * k + nb.offset[0];
                let ly = (y as f64 + 0.5 - b.offset[1]) * k + nb.offset[1];
                nb.img.sample_bilinear(lx, ly)
            })
        };
        let l = Plane::luma(&img);
        neigh.push((img, l));
    }
    if neigh.is_empty() {
        return b;
    }
    let cur_l = Plane::luma(&b.img);
    let (nx, ny) = (w.div_ceil(patch), h.div_ceil(patch));
    // Per patch: (neighbour index, shift) of the substitute, or None to keep.
    let choice: Vec<Option<(usize, i64, i64)>> = (0..nx * ny)
        .into_par_iter()
        .map(|i| {
            let (px, py) = (i % nx, i / nx);
            let (x0, y0) = (px * patch, py * patch);
            let (x1, y1) = ((x0 + patch).min(w), (y0 + patch).min(h));
            let s0 = sharpness(&cur_l, x0, y0, x1, y1);
            let mut best: Option<(f64, usize)> = None;
            for (k, (_, l)) in neigh.iter().enumerate() {
                let s = sharpness(l, x0, y0, x1, y1);
                if s > s0 * thresh && best.is_none_or(|(bs, _)| s > bs) {
                    best = Some((s, k));
                }
            }
            let (_, k) = best?;
            let (dx, dy) = align(&cur_l, &neigh[k].1, x0, y0, x1, y1, search);
            Some((k, dx, dy))
        })
        .collect();
    if choice.iter().all(Option::is_none) {
        return b;
    }
    let src = b.img.clone();
    // Feathered blending between patch decisions: each pixel mixes the four nearest patch
    // centres bilinearly.
    let pick = |i: usize, x: usize, y: usize| -> Px {
        match choice[i] {
            None => src.data[y * w + x],
            Some((k, dx, dy)) => neigh[k].0.get_clamped(x as i64 + dx, y as i64 + dy),
        }
    };
    b.img.rows_mut().for_each(|(y, row)| {
        let fy = ((y as f64 + 0.5) / patch as f64 - 0.5).clamp(0.0, (ny - 1) as f64);
        let (y0, ty) = (fy.floor() as usize, (fy - fy.floor()) as f32);
        let y1 = (y0 + 1).min(ny - 1);
        for (x, o) in row.iter_mut().enumerate() {
            let fx = ((x as f64 + 0.5) / patch as f64 - 0.5).clamp(0.0, (nx - 1) as f64);
            let (x0, tx) = (fx.floor() as usize, (fx - fx.floor()) as f32);
            let x1 = (x0 + 1).min(nx - 1);
            let ids = [y0 * nx + x0, y0 * nx + x1, y1 * nx + x0, y1 * nx + x1];
            if ids.iter().all(|&i| choice[i].is_none()) {
                continue;
            }
            let ws = [(1.0 - tx) * (1.0 - ty), tx * (1.0 - ty), (1.0 - tx) * ty, tx * ty];
            let mut acc = [0.0f32; 4];
            for (i, wgt) in ids.iter().zip(ws) {
                let q = pick(*i, x, y);
                for c in 0..4 {
                    acc[c] += q[c] * wgt;
                }
            }
            if strength != 1.0 {
                // Strength above 100 % pushes past the substitute (away from the blurry pixel).
                let s0 = src.data[y * w + x];
                for c in 0..4 {
                    acc[c] = s0[c] + (acc[c] - s0[c]) * strength;
                }
                acc[3] = acc[3].clamp(0.0, 1.0);
                for c in 0..3 {
                    acc[c] = acc[c].max(0.0);
                }
            }
            if show {
                acc[0] = (acc[0] + 0.3 * acc[3]).min(acc[3].max(acc[0]));
            }
            *o = acc;
        }
    });
    b
}

pub fn specs() -> Vec<EffectSpec> {
    vec![
        spec(
            "ec.blur.ccradial",
            "Variable Radial Blur",
            vec![
                p("type", "Type", Value::Enum(1), popup(&RADIAL_TYPES)),
                p("amount", "Amount", num(10.0), slider(-100.0, 100.0, -100.0, 100.0, 1)),
                p("quality", "Quality", num(50.0), slider(1.0, 100.0, 1.0, 100.0, 1)),
                p("center", "Center", Value::Vec2([0.5, 0.5]), ParamUi::Point),
            ],
            cc_radial_blur,
        ),
        spec(
            "ec.blur.camerashakedeblur",
            "Camera-Shake Deblur",
            vec![
                p("blurDuration", "Blur Duration", num(2.0), slider(1.0, 10.0, 1.0, 10.0, 0)),
                p("deblurMethod", "Deblur Method", Value::Enum(0), popup(&["High Quality (slower)", "Standard"])),
                p("strength", "Strength", num(100.0), slider(100.0, 200.0, 100.0, 200.0, 0)),
                p("shakeSensitivity", "Shake Sensitivity", num(50.0), slider(0.0, 100.0, 0.0, 100.0, 0)),
                p("searchRange", "Search Range", num(8.0), slider(0.0, 64.0, 0.0, 32.0, 0)),
                p("patchSize", "Patch Size", num(32.0), slider(8.0, 256.0, 8.0, 128.0, 0)),
                p("threshold", "Threshold", num(20.0), slider(0.0, 500.0, 0.0, 100.0, 1)),
                p("showBlurryFrames", "Show Blurry Frames", Value::Bool(false), ParamUi::Checkbox),
            ],
            camera_shake_deblur,
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EffectEnv, EffectHost, LayerPixels, run_fx};

    fn checker(w: u32, h: u32, cell: u32) -> Image {
        crate::util::gen_image(w, h, |x, y| {
            let v = if ((x as u32 / cell) + (y as u32 / cell)).is_multiple_of(2) { 1.0 } else { 0.0 };
            [v, v, v, 1.0]
        })
    }

    #[test]
    fn cc_radial_blur_keeps_centre_and_smears_edges() {
        let img = checker(64, 64, 4);
        for kind in 0..5u32 {
            let out = run_fx(
                "ec.blur.ccradial",
                &[("type", Value::Enum(kind)), ("amount", num(40.0)), ("center", Value::Vec2([32.0, 32.0]))],
                img.clone(),
                0.0,
                EffectEnv::default(),
            );
            // Far from the centre the checker is smeared toward grey.
            let edge = out.img.get(2, 2)[0];
            assert!(edge > 0.05 && edge < 0.95, "type {kind}: {edge}");
            let again = run_fx(
                "ec.blur.ccradial",
                &[("type", Value::Enum(kind)), ("amount", num(40.0)), ("center", Value::Vec2([32.0, 32.0]))],
                img.clone(),
                0.0,
                EffectEnv::default(),
            );
            assert_eq!(out.img.data, again.img.data);
        }
        // Zero amount is identity; a constant image stays constant.
        let flat = Image::filled(16, 16, [0.4, 0.4, 0.4, 1.0]);
        let o = run_fx("ec.blur.ccradial", &[("amount", num(80.0))], flat.clone(), 0.0, EffectEnv::default());
        assert!(o.img.data.iter().all(|p| (p[0] - 0.4).abs() < 1e-5));
        let o = run_fx("ec.blur.ccradial", &[("amount", num(0.0))], img.clone(), 0.0, EffectEnv::default());
        assert_eq!(o.img.data, img.data);
    }

    /// Frames: sharp checker at even frames, a blurred one at odd frames (shifted by 2 px).
    struct Shaky;
    impl EffectHost for Shaky {
        fn layer(&self, _: u64, _: bool) -> Option<LayerPixels> {
            None
        }
        fn audio(&self, _: u64, _: f64, _: usize, _: u32) -> Option<Vec<f32>> {
            None
        }
        fn self_at(&self, t: f64, _: usize) -> Option<Buf> {
            let f = (t * 30.0).round() as i64;
            Some(Buf { img: frame(f), offset: [0.0; 2], scale: 1.0 })
        }
    }

    fn frame(f: i64) -> Image {
        let sharp = crate::util::gen_image(48, 48, |x, y| {
            let v = if (((x + 2) / 6) + (y / 6)) % 2 == 0 { 1.0 } else { 0.0 };
            [v, v, v, 1.0]
        });
        if f % 2 == 0 { sharp } else { effectcraft_raster::gaussian_blur(&sharp, 2.5, 2.5, true) }
    }

    #[test]
    fn camera_shake_deblur_substitutes_sharp_neighbours() {
        let host = Shaky;
        let env = EffectEnv { host: Some(&host), frame_rate: 30.0, ..Default::default() };
        let blurry = frame(1);
        let out = run_fx("ec.blur.camerashakedeblur", &[("patchSize", num(16.0)), ("searchRange", num(3.0))], blurry.clone(), 1.0 / 30.0, env);
        let sharp = frame(0);
        let err = |a: &Image| a.data.iter().zip(&sharp.data).map(|(p, q)| (p[0] - q[0]).abs()).sum::<f32>() / a.data.len() as f32;
        assert!(err(&out.img) < err(&blurry) * 0.5, "{} vs {}", err(&out.img), err(&blurry));
        // A sharp frame is left alone (its neighbours are blurrier).
        let keep = run_fx("ec.blur.camerashakedeblur", &[], sharp.clone(), 0.0, env);
        assert_eq!(keep.img.data, sharp.data);
        // Deterministic; no host = pass-through.
        let again = run_fx("ec.blur.camerashakedeblur", &[("patchSize", num(16.0)), ("searchRange", num(3.0))], blurry.clone(), 1.0 / 30.0, env);
        assert_eq!(out.img.data, again.img.data);
        let none = run_fx("ec.blur.camerashakedeblur", &[], blurry.clone(), 0.0, EffectEnv::default());
        assert_eq!(none.img.data, blurry.data);
    }

    #[test]
    fn camera_shake_deblur_sensitivity_and_strength() {
        let host = Shaky;
        let env = EffectEnv { host: Some(&host), frame_rate: 30.0, ..Default::default() };
        let blurry = frame(1);
        let base = [("patchSize", num(16.0)), ("searchRange", num(3.0)), ("threshold", num(500.0))];
        let dist = |a: &Image| a.data.iter().zip(&blurry.data).map(|(p, q)| (p[0] - q[0]).abs()).sum::<f32>();
        // Full sensitivity deblurs whatever the threshold; lower sensitivity never deblurs more.
        let low = run_fx(
            "ec.blur.camerashakedeblur",
            &[base[0].clone(), base[1].clone(), base[2].clone(), ("shakeSensitivity", num(0.0))],
            blurry.clone(),
            1.0 / 30.0,
            env,
        );
        let high = run_fx(
            "ec.blur.camerashakedeblur",
            &[base[0].clone(), base[1].clone(), base[2].clone(), ("shakeSensitivity", num(100.0))],
            blurry.clone(),
            1.0 / 30.0,
            env,
        );
        assert_ne!(high.img.data, blurry.data);
        assert!(dist(&high.img) >= dist(&low.img));
        // Strength 200 % moves further from the blurry frame than 100 %.
        let s1 = run_fx("ec.blur.camerashakedeblur", &[base[0].clone(), base[1].clone()], blurry.clone(), 1.0 / 30.0, env);
        let s2 = run_fx("ec.blur.camerashakedeblur", &[base[0].clone(), base[1].clone(), ("strength", num(200.0))], blurry.clone(), 1.0 / 30.0, env);
        assert!(dist(&s2.img) > dist(&s1.img) * 1.05, "{} vs {}", dist(&s2.img), dist(&s1.img));
    }
}

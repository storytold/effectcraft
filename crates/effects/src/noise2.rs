//! Noise & Grain effects, batch 2: Match Grain (grain statistics measured from another layer
//! and synthesised onto this one), Median (Legacy) and Curl Noise (divergence-free flow
//! distortion from the curl of a fractal noise potential).

use effectcraft_keyframe::Value;
use effectcraft_project::ParamUi;
use effectcraft_raster::Image;
use rayon::prelude::*;

use crate::noise::{GrainLook, draw_boxes, fbm, grain_params, median_image, preview_params, sampling_boxes, sampling_params};
use crate::util::{Plane, gauss_plane, layer_or_self, premul, unpremul};
use crate::{Buf, EffectCtx, EffectSpec, num, p, popup, slider};

fn spec(id: &'static str, name: &'static str, params: Vec<crate::ParamSpec>, render: crate::RenderFn) -> EffectSpec {
    EffectSpec { id, name, category: "Noise & Grain", params, render, gpu: false, float: true }
}

fn pct() -> ParamUi {
    slider(0.0, 100.0, 0.0, 100.0, 1)
}

// ---------------------------------------------------------------- Match Grain

/// Grain statistics of an image: per-channel standard deviation of the high-pass residual
/// (over opaque pixels) and a grain size from the lag-1 autocorrelation.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct GrainStats {
    pub std: [f32; 3],
    pub size: f32,
}

#[cfg(test)]
pub(crate) fn grain_stats(img: &Image) -> GrainStats {
    grain_stats_masked(img, None)
}

/// [`grain_stats`] over the pixels `mask` selects (all opaque pixels without one).
pub(crate) fn grain_stats_masked(img: &Image, mask: Option<&[bool]>) -> GrainStats {
    let sel = |i: usize| mask.is_none_or(|m| m[i]);
    let (w, h) = (img.width as usize, img.height as usize);
    if w < 3 || h < 3 {
        return GrainStats::default();
    }
    let straight: Vec<([f32; 3], f32)> = img.data.par_iter().map(|&p| unpremul(p)).collect();
    let mut std = [0.0f32; 3];
    let mut rho_num = 0.0f64;
    let mut rho_den = 0.0f64;
    for (k, s) in std.iter_mut().enumerate() {
        let pl = Plane { w, h, data: straight.iter().map(|(c, _)| c[k]).collect() };
        let low = gauss_plane(&pl, 1.5, 1.5);
        let hp: Vec<f32> = pl.data.iter().zip(low.data.iter()).map(|(a, b)| a - b).collect();
        let (mut sum, mut sq, mut n) = (0.0f64, 0.0f64, 0.0f64);
        for (i, v) in hp.iter().enumerate() {
            if straight[i].1 > 0.5 && sel(i) {
                sum += *v as f64;
                sq += (*v as f64) * (*v as f64);
                n += 1.0;
            }
        }
        if n > 1.0 {
            let mean = sum / n;
            *s = ((sq / n - mean * mean).max(0.0)).sqrt() as f32;
        }
        for y in 0..h {
            for x in 0..w - 1 {
                let i = y * w + x;
                if straight[i].1 > 0.5 && straight[i + 1].1 > 0.5 && sel(i) && sel(i + 1) {
                    rho_num += (hp[i] * hp[i + 1]) as f64;
                    rho_den += (hp[i] * hp[i]) as f64;
                }
            }
        }
    }
    let rho = if rho_den > 1e-12 { (rho_num / rho_den).clamp(0.0, 0.95) } else { 0.0 };
    GrainStats { std, size: (1.0 / (1.0 - rho as f32)).clamp(0.5, 20.0) * 0.6 }
}

/// Grain statistics over sample `boxes` (in image pixels), measured from small crops around each
/// box instead of the whole image. Empty `boxes` measures every opaque pixel, like
/// [`grain_stats_masked`] without a mask. Overlapping boxes are deduplicated, so the sums match
/// the masked version.
pub(crate) fn grain_stats_boxes(img: &Image, boxes: &[(usize, usize, usize)]) -> GrainStats {
    if boxes.is_empty() {
        return grain_stats_masked(img, None);
    }
    let (w, h) = (img.width as usize, img.height as usize);
    if w < 3 || h < 3 {
        return GrainStats::default();
    }
    // The Gaussian's three box passes at sigma 1.5 have radius 1 each, so 3 px of halo make the
    // residual exact inside a box; one more column covers the right neighbour used by the lag-1
    // correlation.
    const HALO: usize = 4;
    let mut mask = vec![false; w * h];
    for &(x, y, s) in boxes {
        for yy in y..y.saturating_add(s).min(h) {
            for xx in x..x.saturating_add(s).min(w) {
                mask[yy * w + xx] = true;
            }
        }
    }
    let mut counted = vec![false; w * h];
    let mut n = [0.0f64; 3];
    let mut sum = [0.0f64; 3];
    let mut sq = [0.0f64; 3];
    let mut rho_num = 0.0f64;
    let mut rho_den = 0.0f64;
    let mut crop: Vec<([f32; 3], f32)> = Vec::new();
    let mut planes: Vec<Plane> = Vec::new();
    for &(bx, by, bs) in boxes {
        let x0 = bx.saturating_sub(HALO);
        let y0 = by.saturating_sub(HALO);
        let x1 = bx.saturating_add(bs).saturating_add(HALO).min(w);
        let y1 = by.saturating_add(bs).saturating_add(HALO).min(h);
        if x1 <= x0 || y1 <= y0 {
            continue;
        }
        let (cw, ch) = (x1 - x0, y1 - y0);
        crop.clear();
        crop.extend((y0..y1).flat_map(|y| (x0..x1).map(move |x| unpremul(img.data[y * w + x]))));
        planes.clear();
        for k in 0..3 {
            let pl = Plane { w: cw, h: ch, data: crop.iter().map(|s| s.0[k]).collect() };
            planes.push(gauss_plane(&pl, 1.5, 1.5));
        }
        for ly in 0..bs {
            let y = by + ly;
            if y >= h {
                break;
            }
            for lx in 0..bs {
                let x = bx + lx;
                if x >= w {
                    break;
                }
                let g = y * w + x;
                if !mask[g] || counted[g] {
                    continue;
                }
                counted[g] = true;
                let l = (y - y0) * cw + (x - x0);
                let (_, a) = crop[l];
                if a <= 0.5 {
                    continue;
                }
                for k in 0..3 {
                    let hp = crop[l].0[k] - planes[k].data[l];
                    sum[k] += hp as f64;
                    sq[k] += (hp as f64) * (hp as f64);
                    n[k] += 1.0;
                }
                if x + 1 < w && mask[g + 1] && crop[l + 1].1 > 0.5 {
                    for k in 0..3 {
                        let hp = crop[l].0[k] - planes[k].data[l];
                        let hn = crop[l + 1].0[k] - planes[k].data[l + 1];
                        rho_num += (hp * hn) as f64;
                        rho_den += (hp * hp) as f64;
                    }
                }
            }
        }
    }
    let mut std = [0.0f32; 3];
    for k in 0..3 {
        if n[k] > 1.0 {
            let mean = sum[k] / n[k];
            std[k] = ((sq[k] / n[k] - mean * mean).max(0.0)).sqrt() as f32;
        }
    }
    let rho = if rho_den > 1e-12 { (rho_num / rho_den).clamp(0.0, 0.95) } else { 0.0 };
    GrainStats { std, size: (1.0 / (1.0 - rho as f32)).clamp(0.5, 20.0) * 0.6 }
}

/// Match Grain's Viewing Mode options.
const MATCH_GRAIN_VIEWS: [&str; 5] = ["Preview", "Noise Samples", "Compensation Samples", "Blending Matte", "Final Output"];

fn match_grain(ctx: &EffectCtx, mut b: Buf) -> Buf {
    if ctx.params.get("noiseSourceLayer").and_then(Value::as_layer).is_none() {
        return b;
    }
    // MATCH_GRAIN_VIEWS order.
    let view = ctx.params.e("viewingMode");
    let source = layer_or_self(ctx, &b, "noiseSourceLayer", false, false);
    let src_boxes = sampling_boxes(ctx, &source, b.scale);
    let box_col = ctx.params.get("sampling/sampleBoxColor").map(|v| v.as_color()).unwrap_or([1.0, 1.0, 0.0, 1.0]);
    if view == 1 || view == 2 {
        // Noise Samples (on the source) / Compensation Samples (on this layer).
        let (mut img, boxes) = if view == 1 { (source, src_boxes) } else { (b.img.clone(), sampling_boxes(ctx, &b.img, b.scale)) };
        draw_boxes(&mut img, &boxes, box_col);
        b.img = img;
        return b;
    }
    let src_stats = grain_stats_boxes(&source, &src_boxes);
    let comp = (ctx.params.f("compensateForExistingNoise") / 100.0) as f32;
    // The own-layer boxes only matter for the compensation term; skip them when it is off.
    let own = if comp > 0.0 {
        let own_boxes = sampling_boxes(ctx, &b.img, b.scale);
        grain_stats_boxes(&b.img, &own_boxes)
    } else {
        GrainStats::default()
    };
    let orig = (view == 0).then(|| b.img.clone());
    let look = GrainLook::from_params(ctx);
    let amp: [f32; 3] = [0, 1, 2].map(|k| {
        let s = src_stats.std[k];
        let e = own.std[k] * comp;
        (s * s - e * e).max(0.0).sqrt() * look.intensity * look.channel_intensity[k]
    });
    let size = (src_stats.size.max(0.5) * look.size * b.scale as f32).max(0.05);
    let (w, h) = (b.img.width as usize, b.img.height as usize);
    let mut planes = look.planes(w, h, size, b.scale, 131);
    // Normalise each synthetic plane to unit standard deviation so the measured stats transfer.
    for pl in planes.iter_mut() {
        let n = pl.data.len().max(1) as f64;
        let mean = pl.data.iter().map(|&v| v as f64).sum::<f64>() / n;
        let var = pl.data.iter().map(|&v| (v as f64 - mean).powi(2)).sum::<f64>() / n;
        let k = if var > 1e-12 { 1.0 / var.sqrt() } else { 0.0 };
        *pl = pl.map(|v| ((v as f64 - mean) * k) as f32);
    }
    b.img.data.par_iter_mut().enumerate().for_each(|(i, px)| {
        let (c, a) = unpremul(*px);
        let raw = if look.mono { [planes[0].data[i]; 3] } else { [planes[0].data[i], planes[1].data[i], planes[2].data[i]] };
        let g = look.color(raw);
        let weight = look.weight(c);
        match view {
            3 => *px = [weight.clamp(0.0, 1.0), weight.clamp(0.0, 1.0), weight.clamp(0.0, 1.0), 1.0],
            _ => {
                if a <= 0.0 {
                    return;
                }
                let o = [0, 1, 2].map(|k| look.blend(c[k], g[k] * amp[k] * weight));
                *px = premul(o, a);
            }
        }
    });
    if let Some(orig) = &orig {
        crate::noise::preview_compose(ctx, &mut b, orig);
    }
    b
}

// ---------------------------------------------------------------- Median (Legacy)

fn median_legacy(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let r = (ctx.params.f("radius") * b.scale).round().max(0.0) as usize;
    if r == 0 {
        return b;
    }
    if ctx.params.b("operateOnAlphaChannel") {
        b.img = median_image(&b.img, r);
        return b;
    }
    // Colour only: median of straight colour, alpha kept.
    let mut straight = b.img.clone();
    straight.data.par_iter_mut().for_each(|px| {
        let (c, _) = unpremul(*px);
        *px = [c[0], c[1], c[2], 1.0];
    });
    let m = median_image(&straight, r);
    b.img.data.par_iter_mut().zip(m.data.par_iter()).for_each(|(px, q)| {
        *px = premul([q[0], q[1], q[2]], px[3]);
    });
    b
}

// ---------------------------------------------------------------- Curl Noise

fn curl_noise(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let amount = ctx.params.f("displacementAmount") * b.scale;
    let view = ctx.params.e("view");
    if amount.abs() < 1e-6 && view == 0 {
        return b;
    }
    let scale = (ctx.params.f("scale").max(1.0) * b.scale) as f32;
    let complexity = ctx.params.f("complexity") as f32;
    let rot = ctx.params.f("rotation").to_radians() as f32;
    let off = ctx.params.v2("offset");
    let (ox, oy) = ((off[0] * b.scale + b.offset[0]) as f32, (off[1] * b.scale + b.offset[1]) as f32);
    let evo = (ctx.params.f("evolution") / 360.0) as f32;
    let seed = ctx.params.f("randomSeed") as i64 as u32 ^ 0x51_7cc1;
    let steps = ctx.params.f("steps").clamp(1.0, 16.0) as usize;
    let edge_wrap = ctx.params.e("edgeBehavior") == 1;
    let (w, h) = (b.img.width as usize, b.img.height as usize);
    let (cr, sr) = (rot.cos(), rot.sin());
    // Potential field on the pixel grid.
    let mut pot = Plane::new(w, h);
    pot.data.par_chunks_mut(w.max(1)).enumerate().for_each(|(y, row)| {
        for (x, v) in row.iter_mut().enumerate() {
            let (dx, dy) = (x as f32 + 0.5 - ox, y as f32 + 0.5 - oy);
            let u = (dx * cr + dy * sr) / scale;
            let vv = (-dx * sr + dy * cr) / scale;
            *v = fbm(u, vv, evo * 4.0, seed, complexity);
        }
    });
    // Curl: v = (dψ/dy, -dψ/dx), normalised so the typical speed is ~1 px per unit amount.
    let mut vx = Plane::new(w, h);
    let mut vy = Plane::new(w, h);
    vx.data.par_chunks_mut(w.max(1)).zip(vy.data.par_chunks_mut(w.max(1))).enumerate().for_each(|(y, (rx, ry))| {
        for x in 0..w {
            let (xi, yi) = (x as i64, y as i64);
            let gx = (pot.get_clamped(xi + 1, yi) - pot.get_clamped(xi - 1, yi)) * 0.5;
            let gy = (pot.get_clamped(xi, yi + 1) - pot.get_clamped(xi, yi - 1)) * 0.5;
            rx[x] = gy * scale;
            ry[x] = -gx * scale;
        }
    });
    if view == 1 {
        b.img.data.par_iter_mut().enumerate().for_each(|(i, px)| {
            let (a, c) = (vx.data[i], vy.data[i]);
            *px = [(0.5 + a * 0.5).clamp(0.0, 1.0), (0.5 + c * 0.5).clamp(0.0, 1.0), 0.5, 1.0];
        });
        return b;
    }
    let src = b.img.clone();
    let dt = amount as f32 / steps as f32;
    let (wf, hf) = (w as f64, h as f64);
    b.img.rows_mut().for_each(|(y, row)| {
        for (x, px) in row.iter_mut().enumerate() {
            let (mut sx, mut sy) = (x as f64 + 0.5, y as f64 + 0.5);
            for _ in 0..steps {
                // Trace the streamline backwards (semi-Lagrangian advection).
                let (u, v) = (vx.sample(sx, sy), vy.sample(sx, sy));
                sx -= (u * dt) as f64;
                sy -= (v * dt) as f64;
            }
            *px = if edge_wrap { src.sample_bilinear_clamped(sx.rem_euclid(wf), sy.rem_euclid(hf)) } else { src.sample_bilinear_clamped(sx, sy) };
        }
    });
    b
}

pub fn specs() -> Vec<EffectSpec> {
    let (tweaking, color, application, animation) = grain_params();
    vec![
        spec(
            "ec.noise.matchgrain",
            "Match Grain",
            vec![
                p("viewingMode", "Viewing Mode", Value::Enum(4), popup(&MATCH_GRAIN_VIEWS)),
                p("noiseSourceLayer", "Noise Source Layer", Value::Layer(None), ParamUi::Layer),
                p("noiseSourceLayerSource", "Noise Source Layer Source", Value::Enum(0), ParamUi::Hidden),
                p("compensateForExistingNoise", "Compensate for Existing Noise", num(0.0), pct()),
            ]
            .into_iter()
            .chain([preview_params(), sampling_params(), tweaking, color, application, animation].into_iter().flatten())
            .collect(),
            match_grain,
        ),
        spec(
            "ec.noise.medianlegacy",
            "Median (Legacy)",
            vec![
                p("radius", "Radius", num(1.0), slider(0.0, 255.0, 0.0, 20.0, 0)),
                p("operateOnAlphaChannel", "Operate On Alpha Channel", Value::Bool(false), ParamUi::Checkbox),
            ],
            median_legacy,
        ),
        spec(
            "ec.noise.curlnoise",
            "Curl Noise",
            vec![
                p("view", "View", Value::Enum(0), popup(&["Final Output", "Flow Field"])),
                p("displacementAmount", "Displacement Amount", num(20.0), slider(-1000.0, 1000.0, -100.0, 100.0, 1)),
                p("scale", "Scale", num(100.0), slider(1.0, 10000.0, 10.0, 600.0, 1)),
                p("rotation", "Rotation", num(0.0), ParamUi::Angle),
                p("offset", "Offset Turbulence", Value::Vec2([0.5, 0.5]), ParamUi::Point),
                p("complexity", "Complexity", num(3.0), slider(1.0, 20.0, 1.0, 10.0, 1)),
                p("evolution", "Evolution", num(0.0), ParamUi::Angle),
                p("steps", "Flow Steps", num(4.0), slider(1.0, 16.0, 1.0, 16.0, 0)),
                p("edgeBehavior", "Edge Behavior", Value::Enum(0), popup(&["Clamp", "Wrap"])),
                p("randomSeed", "Random Seed", num(0.0), slider(0.0, 100000.0, 0.0, 1000.0, 0)),
            ],
            curl_noise,
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channel2::tests::FakeHost;
    use crate::{EffectEnv, run_fx};

    fn noisy(w: u32, h: u32, amp: f32, seed: u32) -> Image {
        let mut img = Image::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let n = (crate::util::hash1(x, y, seed) - 0.5) * 2.0 * amp;
                img.set(x, y, [0.5 + n, 0.5 + n, 0.5 + n, 1.0]);
            }
        }
        img
    }

    #[test]
    fn boxed_grain_stats_match_the_masked_ones() {
        // Noisy image with alpha holes; the boxes cover overlapping, edge-adjacent and
        // bottom-right corner cases (clamped crops).
        let mut img = noisy(64, 48, 0.2, 9);
        for y in 0..img.height {
            for x in 0..img.width {
                if (x / 7 + y / 5) % 6 == 0 {
                    let mut p = img.get(x as i64, y as i64);
                    p[3] = 0.0;
                    img.set(x, y, p);
                }
            }
        }
        let boxes = [(3usize, 4usize, 9usize), (8, 6, 9), (40, 20, 12), (39, 21, 12), (55, 39, 9)];
        let w = img.width as usize;
        let mut mask = vec![false; img.data.len()];
        for &(x, y, s) in &boxes {
            for yy in y..y + s {
                for xx in x..x + s {
                    mask[yy * w + xx] = true;
                }
            }
        }
        let masked = grain_stats_masked(&img, Some(&mask));
        let boxed = grain_stats_boxes(&img, &boxes);
        for k in 0..3 {
            assert!((masked.std[k] - boxed.std[k]).abs() < 1e-6, "{k}: {masked:?} vs {boxed:?}");
        }
        assert!((masked.size - boxed.size).abs() <= 1e-7, "size: {masked:?} vs {boxed:?}");
        // A denser grid where every box overlaps its neighbours (mask dedup across boxes).
        let img2 = noisy(96, 64, 0.15, 21);
        let mut boxes2 = Vec::new();
        for gy in 0..5 {
            for gx in 0..6 {
                boxes2.push((gx * 11 + (gy % 3) * 4, gy * 10 + gx % 2, 12));
            }
        }
        let w2 = img2.width as usize;
        let mut mask2 = vec![false; img2.data.len()];
        for &(x, y, s) in &boxes2 {
            for yy in y..y + s {
                for xx in x..x + s {
                    mask2[yy * w2 + xx] = true;
                }
            }
        }
        let masked2 = grain_stats_masked(&img2, Some(&mask2));
        let boxed2 = grain_stats_boxes(&img2, &boxes2);
        for k in 0..3 {
            assert!((masked2.std[k] - boxed2.std[k]).abs() < 1e-6, "{k}: {masked2:?} vs {boxed2:?}");
        }
        assert!((masked2.size - boxed2.size).abs() <= 1e-7, "size: {masked2:?} vs {boxed2:?}");
        // No boxes measures the whole image, like no mask.
        assert_eq!(grain_stats_boxes(&img, &[]), grain_stats(&img));
    }

    #[test]
    fn match_grain_transfers_grain_strength() {
        let flat = Image::filled(48, 48, [0.5, 0.5, 0.5, 1.0]);
        let none = run_fx("ec.noise.matchgrain", &[], flat.clone(), 0.0, EffectEnv::default());
        assert_eq!(none.img.data, flat.data);
        let host = FakeHost(noisy(48, 48, 0.1, 3));
        let env = EffectEnv { host: Some(&host), ..Default::default() };
        let out = run_fx("ec.noise.matchgrain", &[("noiseSourceLayer", Value::Layer(Some(7)))], flat.clone(), 0.5, env);
        let target = grain_stats(&host.0).std[0];
        let got = grain_stats(&out.img).std[0];
        assert!(got > target * 0.4 && got < target * 2.5, "target {target} got {got}");
        let again = run_fx("ec.noise.matchgrain", &[("noiseSourceLayer", Value::Layer(Some(7)))], flat, 0.5, env);
        assert_eq!(out.img.data, again.img.data);
    }

    #[test]
    fn median_legacy_radius_zero_identity_and_removes_speck() {
        let mut img = Image::filled(9, 9, [0.2, 0.2, 0.2, 1.0]);
        img.set(4, 4, [1.0, 1.0, 1.0, 1.0]);
        let id = run_fx("ec.noise.medianlegacy", &[("radius", num(0.0))], img.clone(), 0.0, EffectEnv::default());
        assert_eq!(id.img.data, img.data);
        let out = run_fx("ec.noise.medianlegacy", &[("radius", num(1.0))], img.clone(), 0.0, EffectEnv::default());
        assert!((out.img.get(4, 4)[0] - 0.2).abs() < 0.01);
        let again = run_fx("ec.noise.medianlegacy", &[("radius", num(1.0))], img, 0.0, EffectEnv::default());
        assert_eq!(out.img.data, again.img.data);
    }

    #[test]
    fn curl_noise_zero_is_identity_and_flow_preserves_flat() {
        let img = crate::channel2::tests::sample();
        let id = run_fx("ec.noise.curlnoise", &[("displacementAmount", num(0.0))], img.clone(), 0.0, EffectEnv::default());
        assert_eq!(id.img.data, img.data);
        let flat = Image::filled(16, 16, [0.3, 0.6, 0.9, 1.0]);
        let out = run_fx("ec.noise.curlnoise", &[("displacementAmount", num(40.0)), ("scale", num(10.0))], flat.clone(), 0.0, EffectEnv::default());
        assert!(out.img.data.iter().all(|p| (p[1] - 0.6).abs() < 1e-5));
        let mut grad = Image::new(32, 32);
        for y in 0..32 {
            for x in 0..32 {
                grad.set(x, y, [x as f32 / 32.0, y as f32 / 32.0, 0.0, 1.0]);
            }
        }
        let a = run_fx("ec.noise.curlnoise", &[("displacementAmount", num(40.0)), ("scale", num(10.0))], grad.clone(), 0.0, EffectEnv::default());
        let b = run_fx("ec.noise.curlnoise", &[("displacementAmount", num(40.0)), ("scale", num(10.0))], grad.clone(), 0.0, EffectEnv::default());
        assert_eq!(a.img.data, b.img.data);
        assert!(a.img.data.iter().zip(grad.data.iter()).any(|(p, q)| (p[0] - q[0]).abs() > 0.01));
    }
}

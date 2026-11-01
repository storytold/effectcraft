//! Blur & Sharpen, batch 2: edge-preserving blurs (Bilateral, Smart), per-channel and map-driven
//! blurs (Channel, Compound, CC Vector), bokeh (Camera Lens Blur), radial streaks and small
//! utilities (Reduce Interlace Flicker, CC Cross Blur).

use effectcraft_color::luminance;
use effectcraft_keyframe::Value;
use effectcraft_project::ParamUi;
use effectcraft_raster::{Image, Px, gaussian_blur};
use rayon::prelude::*;

use crate::util::{Plane, gauss_plane, premul, unpremul};
use crate::{Buf, EffectCtx, EffectSpec, num, p, popup, slider};

fn spec(id: &'static str, name: &'static str, params: Vec<crate::ParamSpec>, render: crate::RenderFn) -> EffectSpec {
    EffectSpec { id, name, category: "Blur & Sharpen", params, render, gpu: false, float: true }
}

#[inline]
fn plum(p: Px) -> f32 {
    luminance(p[0], p[1], p[2])
}

// ---------------------------------------------------------------------------------------------
// Bilateral / Smart Blur

/// Edge-preserving neighbourhood average. `hard` = box spatial kernel + hard luminance range test
/// (Smart Blur); otherwise Gaussian spatial × Gaussian range weights (bilateral filter, Tomasi &
/// Manduchi 1998). Large radii are sampled on a stride to stay fast.
fn edge_preserving(src: &Image, r: f64, range: f32, hard: bool) -> Image {
    let ri = r.ceil() as i64;
    let stride = ((r / 6.0).ceil() as i64).max(1);
    let sig_s2 = 2.0 * (r / 2.0).max(0.5).powi(2);
    let sig_r2 = 2.0 * range.max(1e-3).powi(2);
    let mut out = Image::new(src.width, src.height);
    out.rows_mut().for_each(|(y, row)| {
        for (x, o) in row.iter_mut().enumerate() {
            let c = src.data[y * src.width as usize + x];
            let cl = plum(c);
            let mut acc = [0.0f32; 4];
            let mut wsum = 0.0f32;
            let mut dy = -ri;
            while dy <= ri {
                let mut dx = -ri;
                while dx <= ri {
                    let d2 = (dx * dx + dy * dy) as f64;
                    if d2 <= r * r + 0.5 {
                        let q = src.get_clamped(x as i64 + dx, y as i64 + dy);
                        let w = if hard {
                            if (plum(q) - cl).abs() <= range { 1.0 } else { 0.0 }
                        } else {
                            let dc = (q[0] - c[0]).powi(2) + (q[1] - c[1]).powi(2) + (q[2] - c[2]).powi(2) + (q[3] - c[3]).powi(2);
                            ((-d2 / sig_s2) as f32 - dc / sig_r2).exp()
                        };
                        if w > 0.0 {
                            for k in 0..4 {
                                acc[k] += q[k] * w;
                            }
                            wsum += w;
                        }
                    }
                    dx += stride;
                }
                dy += stride;
            }
            *o = if wsum > 0.0 { acc.map(|v| v / wsum) } else { c };
        }
    });
    out
}

fn bilateral(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let r = ctx.params.f("radius") * b.scale;
    if r < 0.5 {
        return b;
    }
    let range = ctx.params.f("threshold") as f32 / 100.0;
    b.img = edge_preserving(&b.img, r, range, false);
    if !ctx.params.b("colorize") {
        b.img.map_straight(|c| {
            let l = luminance(c[0], c[1], c[2]);
            [l, l, l]
        });
    }
    b
}

/// Sobel gradient magnitude of premultiplied luminance.
fn sobel(img: &Image) -> Plane {
    let l = Plane::from_image(img, plum);
    let mut out = Plane::new(l.w, l.h);
    if l.w == 0 {
        return out;
    }
    out.data.par_chunks_mut(l.w).enumerate().for_each(|(y, row)| {
        let y = y as i64;
        for (x, o) in row.iter_mut().enumerate() {
            let x = x as i64;
            let g = |dx: i64, dy: i64| l.get_clamped(x + dx, y + dy);
            let gx = g(1, -1) + 2.0 * g(1, 0) + g(1, 1) - g(-1, -1) - 2.0 * g(-1, 0) - g(-1, 1);
            let gy = g(-1, 1) + 2.0 * g(0, 1) + g(1, 1) - g(-1, -1) - 2.0 * g(0, -1) - g(1, -1);
            *o = (gx * gx + gy * gy).sqrt() * 0.25;
        }
    });
    out
}

fn smart_blur(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let r = ctx.params.f("radius") * b.scale;
    let thr = ctx.params.f("threshold") as f32 / 100.0;
    let mode = ctx.params.e("mode");
    let blurred = if r >= 0.5 { edge_preserving(&b.img, r, thr, true) } else { b.img.clone() };
    if mode == 0 {
        b.img = blurred;
        return b;
    }
    let edges = sobel(&blurred);
    let et = thr.max(0.01) * 0.5;
    let mut out = blurred;
    out.data.par_iter_mut().zip(edges.data.par_iter()).for_each(|(px, &e)| {
        let a = px[3];
        let on = e > et;
        if mode == 1 {
            let v = if on { 1.0 } else { 0.0 };
            *px = premul([v; 3], a);
        } else if on {
            *px = premul([1.0; 3], a);
        }
    });
    b.img = out;
    b
}

// ---------------------------------------------------------------------------------------------
// Channel Blur

fn dims_xy(dim: u32) -> (f64, f64) {
    match dim {
        1 => (1.0, 0.0),
        2 => (0.0, 1.0),
        _ => (1.0, 1.0),
    }
}

fn channel_blur(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let sig = ["redBlurriness", "greenBlurriness", "blueBlurriness", "alphaBlurriness"].map(|id| ctx.params.f(id).max(0.0) * 0.5 * b.scale);
    if sig.iter().all(|&s| s < 0.05) {
        return b;
    }
    let (kx, ky) = dims_xy(ctx.params.e("dimensions"));
    let repeat = ctx.params.b("repeatEdge") || ctx.adjustment;
    let maxs = sig.iter().cloned().fold(0.0, f64::max);
    if !repeat {
        b.pad((maxs * 3.0).ceil() as u32 + 1);
    }
    // The buffer is padded with transparency (or edges repeat), so an edge-clamped plane blur fits both.
    let blur = |pl: &Plane, s: f64| -> Plane { gauss_plane(pl, s * kx, s * ky) };
    let chans: [Plane; 4] = crate::util::split(&b.img);
    let alpha = &chans[3];
    let mut alpha_cache: Vec<(u64, Plane)> = Vec::new();
    let mut get_alpha = |s: f64| -> Plane {
        let key = s.to_bits();
        if let Some((_, pl)) = alpha_cache.iter().find(|(k, _)| *k == key) {
            return pl.clone();
        }
        let pl = if s < 0.05 { alpha.clone() } else { blur(alpha, s) };
        alpha_cache.push((key, pl.clone()));
        pl
    };
    let new_a = get_alpha(sig[3]);
    let mut straight: Vec<Plane> = Vec::with_capacity(3);
    for c in 0..3 {
        if sig[c] < 0.05 {
            straight.push(chans[c].zip_map(alpha, |v, a| if a > 1e-6 { v / a } else { 0.0 }));
        } else {
            let bc = blur(&chans[c], sig[c]);
            let ba = get_alpha(sig[c]);
            straight.push(bc.zip_map(&ba, |v, a| if a > 1e-6 { v / a } else { 0.0 }));
        }
    }
    b.img.data.par_iter_mut().enumerate().for_each(|(i, px)| {
        let a = new_a.data[i].clamp(0.0, 1.0);
        *px = premul([straight[0].data[i], straight[1].data[i], straight[2].data[i]], a);
    });
    b
}

// ---------------------------------------------------------------------------------------------
// Compound Blur

fn compound_blur(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let max_sigma = ctx.params.f("maximumBlur").max(0.0) * 0.5 * b.scale;
    if max_sigma < 0.05 {
        return b;
    }
    let invert = ctx.params.b("invertBlur");
    if !ctx.adjustment {
        b.pad((max_sigma * 3.0).ceil() as u32 + 1);
    }
    // The blur map: the Blur Layer's luminance (stretched to fit or centred), else the layer's own.
    let map_img = crate::util::layer_or_self(ctx, &b, "blurLayer", true, ctx.params.b("stretchMapToFit"));
    let map = Plane::from_image(&map_img, |p| {
        let (c, a) = unpremul(p);
        let l = luminance(c[0], c[1], c[2]) * a;
        if invert { 1.0 - l } else { l }
    });
    const N: usize = 5;
    let levels: Vec<Image> = (0..N)
        .into_par_iter()
        .map(|k| {
            if k == 0 {
                b.img.clone()
            } else {
                gaussian_blur(&b.img, max_sigma * k as f64 / (N - 1) as f64, max_sigma * k as f64 / (N - 1) as f64, ctx.adjustment)
            }
        })
        .collect();
    b.img.data.par_iter_mut().enumerate().for_each(|(i, px)| {
        let t = map.data[i].clamp(0.0, 1.0) * (N - 1) as f32;
        let k = (t.floor() as usize).min(N - 2);
        let f = t - k as f32;
        let a = levels[k].data[i];
        let c = levels[k + 1].data[i];
        *px = crate::util::lerp4(a, c, f);
    });
    b
}

// ---------------------------------------------------------------------------------------------
// Camera Lens Blur

/// Half-open horizontal spans (dy, x_left, x_right) of a polygonal / round iris of radius `r`.
fn iris_spans(r: f64, sides: u32, rotation_deg: f64, roundness: f64) -> Vec<(i64, i64, i64)> {
    let ri = r.ceil() as i64;
    let rot = rotation_deg.to_radians();
    let verts: Vec<(f64, f64)> = (0..sides)
        .map(|k| {
            let a = rot - std::f64::consts::FRAC_PI_2 + k as f64 * std::f64::consts::TAU / sides.max(3) as f64;
            (r * a.cos(), r * a.sin())
        })
        .collect();
    let mut spans = Vec::new();
    for dy in -ri..=ri {
        let y = dy as f64;
        let circ = if y.abs() <= r { Some((r * r - y * y).max(0.0).sqrt()) } else { None };
        let poly = if sides >= 3 {
            let mut lo = f64::INFINITY;
            let mut hi = f64::NEG_INFINITY;
            for k in 0..verts.len() {
                let (x0, y0) = verts[k];
                let (x1, y1) = verts[(k + 1) % verts.len()];
                if (y0 - y) * (y1 - y) <= 0.0 && (y1 - y0).abs() > 1e-12 {
                    let x = x0 + (y - y0) / (y1 - y0) * (x1 - x0);
                    lo = lo.min(x);
                    hi = hi.max(x);
                }
            }
            if lo <= hi { Some((lo, hi)) } else { None }
        } else {
            circ.map(|c| (-c, c))
        };
        let span = match (poly, circ) {
            (Some((lo, hi)), Some(c)) => Some((lo + (-c - lo) * roundness, hi + (c - hi) * roundness)),
            (Some(s), None) => Some(s),
            (None, Some(c)) if roundness > 0.5 => Some((-c, c)),
            _ => None,
        };
        if let Some((lo, hi)) = span {
            let (l, h) = (lo.round() as i64, hi.round() as i64);
            if l <= h {
                spans.push((dy, l, h));
            }
        }
    }
    if spans.is_empty() {
        spans.push((0, 0, 0));
    }
    spans
}

/// Camera Lens Blur's iris spans at blur radius `r` pixels (shape, rotation, roundness and
/// aspect ratio applied) and the iris' horizontal reach (shared with the GPU kernel).
pub fn camera_lens_spans(ctx: &EffectCtx, r: f64) -> (Vec<(i64, i64, i64)>, f64) {
    let shape = ctx.params.e("irisProperties/irisShape");
    let sides = if shape >= 8 { 0 } else { shape + 3 };
    let roundness = ctx.params.f("irisProperties/irisRoundness").clamp(0.0, 100.0) / 100.0;
    // Aspect Ratio stretches the iris horizontally (> 1) or squeezes it (< 1).
    let aspect = ctx.params.f("irisProperties/irisAspectRatio");
    let mut spans = iris_spans(r, sides, ctx.params.f("irisProperties/irisRotation"), roundness);
    if aspect > 0.0 && (aspect - 1.0).abs() > 1e-6 {
        for s in spans.iter_mut() {
            s.1 = (s.1 as f64 * aspect).round() as i64;
            s.2 = (s.2 as f64 * aspect).round() as i64;
        }
    }
    (spans, r * aspect.max(1.0))
}

/// Whether Camera Lens Blur needs more than plain iris spans (Diffraction Fringe or a Blur
/// Map): the GPU kernel handles the plain blur only.
pub fn camera_lens_plain(ctx: &EffectCtx) -> bool {
    ctx.params.f("irisProperties/diffractionFringe") <= 0.0 && ctx.params.get("blurMap/blurMapLayer").and_then(|v| v.as_layer()).is_none()
}

fn camera_lens_blur(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let r = ctx.params.f("blurRadius").max(0.0) * b.scale;
    if r < 0.5 {
        return b;
    }
    let shape = ctx.params.e("irisProperties/irisShape");
    let sides = if shape >= 8 { 0 } else { shape + 3 };
    let roundness = ctx.params.f("irisProperties/irisRoundness").clamp(0.0, 100.0) / 100.0;
    // Aspect Ratio stretches the iris horizontally (> 1) or squeezes it (< 1).
    let aspect = ctx.params.f("irisProperties/irisAspectRatio");
    let reach = r * aspect.max(1.0);
    let repeat = ctx.params.b("repeatEdge") || ctx.adjustment;
    if !repeat {
        b.pad(reach.ceil() as u32 + 1);
    }
    let linear = ctx.params.b("useLinear");
    let to_lin = |px: &mut [f32; 4]| {
        let (c, a) = unpremul(*px);
        *px = premul(c.map(effectcraft_color::srgb_to_linear), a);
    };
    // Highlights: boost bright pixels before the blur so they bloom into bokeh shapes; Saturation
    // keeps their colour (100) or pushes the added energy towards white (0).
    let thr = ctx.params.f("highlight/specularThreshold") as f32 / 255.0;
    let boost = 1.0 + ctx.params.f("highlight/specularBrightness").max(0.0) as f32 / 100.0 * 4.0;
    let sat = (ctx.params.f("highlight/highlightSaturation") / 100.0).clamp(0.0, 1.0) as f32;
    let mut src = b.img.clone();
    if linear {
        src.data.par_iter_mut().for_each(to_lin);
    }
    if boost > 1.0 {
        src.data.par_iter_mut().for_each(|px| {
            let (c, a) = unpremul(*px);
            if a > 0.0 && luminance(c[0], c[1], c[2]) >= thr {
                let m = c[0].max(c[1]).max(c[2]);
                for k in 0..3 {
                    let tinted = c[k] + (m - c[k]) * (1.0 - sat);
                    px[k] = (c[k] + tinted * (boost - 1.0)) * a;
                }
            }
        });
    }
    let (w, h) = (src.width as usize, src.height as usize);
    // Per-row prefix sums (f64 for precision).
    let prefix: Vec<Vec<[f64; 4]>> = src
        .data
        .par_chunks(w.max(1))
        .map(|row| {
            let mut v = Vec::with_capacity(w + 1);
            let mut acc = [0.0f64; 4];
            v.push(acc);
            for px in row {
                for c in 0..4 {
                    acc[c] += px[c] as f64;
                }
                v.push(acc);
            }
            v
        })
        .collect();
    // Sum of the source over the iris spans centred on (x, y).
    let span_sum = |spans: &[(i64, i64, i64)], x: usize, y: usize| -> [f64; 4] {
        let mut acc = [0.0f64; 4];
        for &(dy, l, rr) in spans {
            let mut yy = y as i64 + dy;
            if yy < 0 || yy >= h as i64 {
                if !repeat {
                    continue;
                }
                yy = yy.clamp(0, h as i64 - 1);
            }
            let pre = &prefix[yy as usize];
            let (mut a, mut bnd) = (x as i64 + l, x as i64 + rr);
            if repeat {
                if a < 0 {
                    let n = (-a).min(bnd - a + 1) as f64;
                    let first = src.data[yy as usize * w];
                    for c in 0..4 {
                        acc[c] += first[c] as f64 * n;
                    }
                    a = 0;
                }
                if bnd >= w as i64 {
                    let n = (bnd - (w as i64 - 1)).min(bnd - a + 1) as f64;
                    let last = src.data[yy as usize * w + w - 1];
                    for c in 0..4 {
                        acc[c] += last[c] as f64 * n;
                    }
                    bnd = w as i64 - 1;
                }
            } else {
                a = a.max(0);
                bnd = bnd.min(w as i64 - 1);
            }
            if a <= bnd {
                for c in 0..4 {
                    acc[c] += pre[bnd as usize + 1][c] - pre[a as usize][c];
                }
            }
        }
        acc
    };
    // Diffraction Fringe: the iris edge (outer 20 % of the radius) gets more of the energy:
    // 0 = an even disc, 100 = a natural halo, 500 = all of it in the ring.
    let fringe = (ctx.params.f("irisProperties/diffractionFringe") / 500.0).clamp(0.0, 1.0);
    let (w_in, w_ring) = (1.0 - fringe, 1.0 + 4.0 * fringe);
    let iris = |radius: f64| -> (Vec<(i64, i64, i64)>, Vec<(i64, i64, i64)>) {
        let stretch = |mut s: Vec<(i64, i64, i64)>| {
            if aspect > 0.0 && (aspect - 1.0).abs() > 1e-6 {
                for v in s.iter_mut() {
                    v.1 = (v.1 as f64 * aspect).round() as i64;
                    v.2 = (v.2 as f64 * aspect).round() as i64;
                }
            }
            s
        };
        let outer = stretch(iris_spans(radius, sides, ctx.params.f("irisProperties/irisRotation"), roundness));
        let inner = if fringe > 0.0 && radius * 0.8 >= 1.0 {
            stretch(iris_spans(radius * 0.8, sides, ctx.params.f("irisProperties/irisRotation"), roundness))
        } else {
            vec![]
        };
        (outer, inner)
    };
    let count = |s: &[(i64, i64, i64)]| s.iter().map(|(_, l, r)| (r - l + 1) as f64).sum::<f64>();
    // One blurred pixel with the given iris.
    let blur_px = |sp: &(Vec<(i64, i64, i64)>, Vec<(i64, i64, i64)>), x: usize, y: usize| -> [f32; 4] {
        let (outer, inner) = sp;
        let acc = if inner.is_empty() {
            let n = count(outer).max(1.0);
            span_sum(outer, x, y).map(|v| v / n)
        } else {
            let (so, si) = (span_sum(outer, x, y), span_sum(inner, x, y));
            let (no, ni) = (count(outer), count(inner));
            let norm = (w_in * ni + w_ring * (no - ni)).max(1e-9);
            std::array::from_fn(|c| (w_in * si[c] + w_ring * (so[c] - si[c])) / norm)
        };
        let mut px = acc.map(|v| v as f32);
        px[3] = px[3].clamp(0.0, 1.0);
        if linear {
            let (c, a) = unpremul(px);
            px = premul(c.map(|v| effectcraft_color::linear_to_srgb(v.max(0.0))), a);
        }
        px
    };
    // Blur Map: per-pixel radius from a map layer (no blur at Blur Focal Distance, full radius
    // at the farthest value).
    let map = ctx.layer_param("blurMap/blurMapLayer", true).map(|o| {
        let stretch = ctx.params.e("blurMap/placement") == 1;
        let img = crate::util::fit_layer(ctx, &b, &o, stretch);
        let ch = ctx.params.e("blurMap/channel");
        let invert = ctx.params.b("blurMap/invertBlurMap");
        let focal = (ctx.params.f("blurMap/blurFocalDistance") / 255.0).clamp(0.0, 1.0) as f32;
        let span = focal.max(1.0 - focal).max(1e-3);
        img.data
            .iter()
            .map(|p| {
                let (c, a) = unpremul(*p);
                let mut v = match ch {
                    1 => a,
                    2 => c[0].max(c[1]).max(c[2]) * a,
                    _ => luminance(c[0], c[1], c[2]) * a,
                };
                if invert {
                    v = 1.0 - v;
                }
                ((v - focal).abs() / span).clamp(0.0, 1.0)
            })
            .collect::<Vec<f32>>()
    });
    let mut out = Image::new(src.width, src.height);
    match &map {
        None => {
            let sp = iris(r);
            out.rows_mut().for_each(|(y, row)| {
                for (x, o) in row.iter_mut().enumerate() {
                    *o = blur_px(&sp, x, y);
                }
            });
        }
        Some(m) => {
            // Radii quantised to levels; each pixel blends the two nearest.
            let levels = (r.ceil() as usize).clamp(1, 8);
            let irises: Vec<_> = (0..=levels).map(|k| iris(r * k as f64 / levels as f64)).collect();
            let srcd = &b.img.data;
            out.rows_mut().for_each(|(y, row)| {
                for (x, o) in row.iter_mut().enumerate() {
                    let f = m[y * w + x] * levels as f32;
                    let k0 = (f.floor() as usize).min(levels);
                    let t = f - k0 as f32;
                    let at = |k: usize| if k == 0 || r * k as f64 / (levels as f64) < 0.5 { srcd[y * w + x] } else { blur_px(&irises[k], x, y) };
                    let a = at(k0);
                    *o = if t > 1e-4 && k0 < levels {
                        let bq = at(k0 + 1);
                        std::array::from_fn(|c| a[c] + (bq[c] - a[c]) * t)
                    } else {
                        a
                    };
                }
            });
        }
    }
    b.img = out;
    b
}

// ---------------------------------------------------------------------------------------------
// CC Radial Fast Blur

/// Transparent pixels CC Radial Fast Blur adds on every side of a `w` × `h` buffer with Center
/// at `c` (buffer pixels) and smear `s`: content `r` from Center streaks out to `r / (1 − s)`,
/// so the farthest corner reaches `d · s / (1 − s)` past itself (at most 4096, as the other
/// growing effects). Shared with the GPU path so both grow the same way.
pub fn radial_fast_pad(c: (f64, f64), w: u32, h: u32, s: f64) -> u32 {
    let (w, h) = (f64::from(w), f64::from(h));
    let far = [(0.0, 0.0), (w, 0.0), (0.0, h), (w, h)].iter().map(|&(x, y)| (x - c.0).hypot(y - c.1)).fold(0.0, f64::max);
    ((far * s / (1.0 - s)).ceil() + 1.0).min(4096.0) as u32
}

fn radial_fast_blur(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let s = (ctx.params.f("amount") / 100.0).clamp(0.0, 1.0) * 0.5;
    if s <= 0.0 {
        return b;
    }
    if !ctx.adjustment {
        b.pad(radial_fast_pad(b.to_px(ctx.params.v2("center")), b.img.width, b.img.height, s));
    }
    let c = b.to_px(ctx.params.v2("center"));
    let mode = ctx.params.e("zoom");
    let src = b.img.clone();
    b.img.rows_mut().for_each(|(y, row)| {
        for (x, o) in row.iter_mut().enumerate() {
            let (dx, dy) = (x as f64 + 0.5 - c.0, y as f64 + 0.5 - c.1);
            let len = (dx * dx + dy * dy).sqrt() * s;
            let n = (len.ceil() as usize).clamp(2, 64);
            let mut acc = match mode {
                1 => [f32::NEG_INFINITY; 4],
                2 => [f32::INFINITY; 4],
                _ => [0.0; 4],
            };
            for i in 0..n {
                let k = 1.0 - s * i as f64 / (n - 1) as f64;
                let q = src.sample_bilinear(c.0 + dx * k, c.1 + dy * k);
                for ch in 0..4 {
                    acc[ch] = match mode {
                        1 => acc[ch].max(q[ch]),
                        2 => acc[ch].min(q[ch]),
                        _ => acc[ch] + q[ch] / n as f32,
                    };
                }
            }
            for ch in 0..3 {
                acc[ch] = acc[ch].min(acc[3]).max(0.0);
            }
            acc[3] = acc[3].clamp(0.0, 1.0);
            *o = acc;
        }
    });
    b
}

// ---------------------------------------------------------------------------------------------
// CC Vector Blur

fn vector_blur(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let amount = ctx.params.f("amount").max(0.0) * b.scale;
    if amount < 0.5 {
        return b;
    }
    let kind = ctx.params.e("type");
    let off = ctx.params.f("angleOffset").to_radians();
    let soft = ctx.params.f("mapSoftness").max(0.0) * 0.5 * b.scale;
    let map = gauss_plane(&Plane::from_image(&b.img, plum), soft, soft);
    let src = b.img.clone();
    let (w, h) = (map.w as i64, map.h as i64);
    b.img.rows_mut().for_each(|(y, row)| {
        let y = y as i64;
        for (x, o) in row.iter_mut().enumerate() {
            let x = x as i64;
            let gx = (map.get_clamped((x + 1).min(w - 1), y) - map.get_clamped((x - 1).max(0), y)) * 0.5;
            let gy = (map.get_clamped(x, (y + 1).min(h - 1)) - map.get_clamped(x, (y - 1).max(0))) * 0.5;
            let mag = (gx * gx + gy * gy).sqrt() as f64;
            let m = map.get_clamped(x, y) as f64;
            let (ang, len) = match kind {
                0..=2 => {
                    if mag < 1e-6 {
                        continue;
                    }
                    let g = (gy as f64).atan2(gx as f64);
                    let base = if kind == 2 { g } else { g + std::f64::consts::FRAC_PI_2 };
                    let len = if kind == 1 { amount } else { amount * (mag * 10.0).min(1.0) };
                    (base + off, len)
                }
                3 => (m * std::f64::consts::TAU + off, amount),
                _ => (m * std::f64::consts::TAU + off, amount * m),
            };
            if len < 0.5 {
                continue;
            }
            let (dx, dy) = (ang.cos() * len, ang.sin() * len);
            let n = ((len * 2.0).ceil() as usize).clamp(2, 48);
            let mut acc = [0.0f32; 4];
            for i in 0..n {
                let t = i as f64 / (n - 1) as f64 - 0.5;
                let q = src.sample_bilinear_clamped(x as f64 + 0.5 + dx * t, y as f64 + 0.5 + dy * t);
                for c in 0..4 {
                    acc[c] += q[c] / n as f32;
                }
            }
            *o = acc;
        }
    });
    b
}

// ---------------------------------------------------------------------------------------------
// Reduce Interlace Flicker / CC Cross Blur

fn reduce_flicker(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let s = ctx.params.f("softness").max(0.0) * b.scale;
    if s < 0.05 {
        return b;
    }
    b.img = gaussian_blur(&b.img, 0.0, s, true);
    b
}

fn cross_blur(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let rx = (ctx.params.f("radiusX").max(0.0) * b.scale).round() as usize;
    let ry = (ctx.params.f("radiusY").max(0.0) * b.scale).round() as usize;
    if rx == 0 && ry == 0 {
        return b;
    }
    if !ctx.adjustment {
        b.pad((rx.max(ry) * 3) as u32 + 1);
    }
    let repeat = ctx.adjustment;
    let hz = effectcraft_raster::box_blur(&b.img, rx, 0, 3, repeat);
    let vt = effectcraft_raster::box_blur(&b.img, 0, ry, 3, repeat);
    let mode = ctx.params.e("transferMode");
    b.img.data.par_iter_mut().enumerate().for_each(|(i, px)| {
        let (a, v) = (hz.data[i], vt.data[i]);
        let mut o = [0.0f32; 4];
        for c in 0..4 {
            o[c] = match mode {
                1 => a[c] + v[c],
                2 => a[c] + v[c] - a[c] * v[c],
                3 => a[c].max(v[c]),
                _ => (a[c] + v[c]) * 0.5,
            };
        }
        o[3] = o[3].clamp(0.0, 1.0);
        *px = o;
    });
    b
}

pub fn specs() -> Vec<EffectSpec> {
    let dims = || popup(&["Horizontal and Vertical", "Horizontal", "Vertical"]);
    let blur_s = || slider(0.0, 3000.0, 0.0, 50.0, 1);
    vec![
        spec(
            "ec.blur.bilateral",
            "Bilateral Blur",
            vec![
                p("radius", "Radius", num(5.0), slider(0.0, 200.0, 0.0, 50.0, 1)),
                p("threshold", "Threshold", num(10.0), slider(0.0, 100.0, 0.0, 100.0, 1)),
                p("colorize", "Colorize", Value::Bool(true), ParamUi::Checkbox),
            ],
            bilateral,
        ),
        spec(
            "ec.blur.smart",
            "Smart Blur",
            vec![
                p("radius", "Radius", num(3.0), slider(0.0, 100.0, 0.0, 20.0, 1)),
                p("threshold", "Threshold", num(25.0), slider(0.0, 100.0, 0.0, 100.0, 1)),
                p("mode", "Mode", Value::Enum(0), popup(&["Normal", "Edge Only", "Overlay Edge"])),
            ],
            smart_blur,
        ),
        spec(
            "ec.blur.channel",
            "Channel Blur",
            vec![
                p("redBlurriness", "Red Blurriness", num(0.0), blur_s()),
                p("greenBlurriness", "Green Blurriness", num(0.0), blur_s()),
                p("blueBlurriness", "Blue Blurriness", num(0.0), blur_s()),
                p("alphaBlurriness", "Alpha Blurriness", num(0.0), blur_s()),
                p("repeatEdge", "Repeat Edge Pixels", Value::Bool(false), ParamUi::Checkbox),
                p("dimensions", "Blur Dimensions", Value::Enum(0), dims()),
            ],
            channel_blur,
        ),
        spec(
            "ec.blur.compound",
            "Compound Blur",
            vec![
                p("blurLayer", "Blur Layer", Value::Layer(None), ParamUi::Layer),
                p("maximumBlur", "Maximum Blur", num(20.0), slider(0.0, 1000.0, 0.0, 100.0, 1)),
                p("stretchMapToFit", "Stretch Map to Fit", Value::Bool(true), ParamUi::Checkbox),
                p("invertBlur", "Invert Blur", Value::Bool(false), ParamUi::Checkbox),
            ],
            compound_blur,
        ),
        spec(
            "ec.blur.cameralens",
            "Camera Lens Blur",
            vec![
                p("blurRadius", "Blur Radius", num(10.0), slider(0.0, 500.0, 0.0, 100.0, 1)),
                p(
                    "irisProperties/irisShape",
                    "Shape",
                    Value::Enum(3),
                    popup(&["Triangle", "Square", "Pentagon", "Hexagon", "Heptagon", "Octagon", "Nonagon", "Decagon", "Circle"]),
                ),
                p("irisProperties/irisRoundness", "Roundness", num(0.0), slider(0.0, 100.0, 0.0, 100.0, 1)),
                p("irisProperties/irisAspectRatio", "Aspect Ratio", num(1.0), slider(0.01, 100.0, 0.25, 4.0, 2)),
                p("irisProperties/irisRotation", "Rotation", num(0.0), ParamUi::Angle),
                p("irisProperties/diffractionFringe", "Diffraction Fringe", num(0.0), slider(0.0, 500.0, 0.0, 500.0, 1)),
                p("blurMap/blurMapLayer", "Layer", Value::Layer(None), ParamUi::Layer),
                p("blurMap/channel", "Channel", Value::Enum(0), popup(&["Luminance", "Alpha", "Color"])),
                p("blurMap/placement", "Placement", Value::Enum(1), popup(&["Center", "Stretch"])),
                p("blurMap/blurFocalDistance", "Blur Focal Distance", num(0.0), slider(0.0, 255.0, 0.0, 255.0, 0)),
                p("blurMap/invertBlurMap", "Invert Blur Map", Value::Bool(false), ParamUi::Checkbox),
                p("highlight/specularBrightness", "Gain", num(0.0), slider(0.0, 100.0, 0.0, 100.0, 1)),
                p("highlight/specularThreshold", "Threshold", num(255.0), slider(0.0, 255.0, 0.0, 255.0, 0)),
                p("highlight/highlightSaturation", "Saturation", num(100.0), slider(0.0, 100.0, 0.0, 100.0, 1)),
                p("repeatEdge", "Repeat Edge Pixels", Value::Bool(false), ParamUi::Checkbox),
                p("useLinear", "Use Linear Working Space", Value::Bool(false), ParamUi::Checkbox),
            ],
            camera_lens_blur,
        ),
        spec(
            "ec.blur.ccradialfast",
            "CC Radial Fast Blur",
            vec![
                p("center", "Center", Value::Vec2([0.5, 0.5]), ParamUi::Point),
                p("amount", "Amount", num(50.0), slider(0.0, 100.0, 0.0, 100.0, 1)),
                p("zoom", "Zoom", Value::Enum(0), popup(&["Standard", "Brightest", "Darkest"])),
            ],
            radial_fast_blur,
        ),
        spec(
            "ec.blur.ccvector",
            "CC Vector Blur",
            vec![
                p("type", "Type", Value::Enum(0), popup(&["Natural", "Constant Length", "Perpendicular", "Direction Center", "Direction Fading"])),
                p("amount", "Amount", num(10.0), slider(0.0, 500.0, 0.0, 100.0, 1)),
                p("angleOffset", "Angle Offset", num(0.0), ParamUi::Angle),
                p("mapSoftness", "Map Softness", num(15.0), slider(0.0, 100.0, 0.0, 100.0, 1)),
            ],
            vector_blur,
        ),
        EffectSpec {
            category: "Obsolete",
            ..spec(
                "ec.blur.reduceflicker",
                "Reduce Interlace Flicker",
                vec![p("softness", "Softness", num(0.0), slider(0.0, 100.0, 0.0, 10.0, 2))],
                reduce_flicker,
            )
        },
        spec(
            "ec.blur.cccross",
            "CC Cross Blur",
            vec![
                p("radiusX", "Radius X", num(10.0), slider(0.0, 500.0, 0.0, 100.0, 1)),
                p("radiusY", "Radius Y", num(10.0), slider(0.0, 500.0, 0.0, 100.0, 1)),
                p("transferMode", "Transfer Mode", Value::Enum(0), popup(&["Blend", "Add", "Screen", "Lighten"])),
            ],
            cross_blur,
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Params, find};

    fn run(id: &str, img: &Image, over: &[(&str, Value)]) -> Buf {
        let s = find(id).unwrap();
        let mut params = Params { values: s.params.iter().map(|p| (p.id.to_string(), p.default.clone())).collect() };
        for (k, v) in over {
            params.values.insert(k.to_string(), v.clone());
        }
        let ctx =
            EffectCtx { params: &params, time: 0.0, layer_size: [img.width as f64, img.height as f64], seed: 1, adjustment: false, env: Default::default() };
        (s.render)(&ctx, Buf { img: img.clone(), offset: [0.0, 0.0], scale: 1.0 })
    }

    fn edge_img() -> Image {
        let mut img = Image::new(32, 16);
        for y in 0..16 {
            for x in 0..32 {
                let v = if x < 16 { 0.1 } else { 0.9 };
                img.set(x, y, [v, v, v, 1.0]);
            }
        }
        img
    }

    #[test]
    fn bilateral_keeps_edge_sharper_than_gaussian() {
        let img = edge_img();
        let bl = run("ec.blur.bilateral", &img, &[("radius", num(4.0))]).img;
        let g = gaussian_blur(&img, 2.0, 2.0, true);
        let jump_b = bl.get(16, 8)[0] - bl.get(15, 8)[0];
        let jump_g = g.get(16, 8)[0] - g.get(15, 8)[0];
        assert!(jump_b > 0.7, "{jump_b}");
        assert!(jump_b > jump_g + 0.3);
    }

    #[test]
    fn channel_blur_zero_is_identity() {
        let img = edge_img();
        let out = run("ec.blur.channel", &img, &[]);
        assert_eq!(out.img, img);
    }

    #[test]
    fn channel_blur_only_touches_selected_channel() {
        let img = edge_img();
        let out = run("ec.blur.channel", &img, &[("redBlurriness", num(6.0)), ("repeatEdge", Value::Bool(true))]).img;
        let (l, r) = (out.get(15, 8), out.get(16, 8));
        assert!(r[0] - l[0] < 0.6);
        assert!((l[1] - 0.1).abs() < 1e-5 && (r[1] - 0.9).abs() < 1e-5);
    }

    #[test]
    fn lens_blur_preserves_constant_and_energy() {
        let img = Image::filled(24, 20, [0.4, 0.3, 0.2, 1.0]);
        let out = run("ec.blur.cameralens", &img, &[("repeatEdge", Value::Bool(true))]).img;
        assert!(out.data.iter().all(|p| (p[0] - 0.4).abs() < 1e-4 && (p[3] - 1.0).abs() < 1e-4));
        let mut dot = Image::new(9, 9);
        dot.set(4, 4, [1.0, 1.0, 1.0, 1.0]);
        let out = run("ec.blur.cameralens", &dot, &[("blurRadius", num(5.0))]);
        let total: f32 = out.img.data.iter().map(|p| p[3]).sum();
        assert!((total - 1.0).abs() < 1e-3, "{total}");
        assert!(out.img.width > 9);
    }

    #[test]
    fn lens_blur_aspect_ratio_widens_the_bokeh() {
        let mut dot = Image::new(41, 41);
        dot.set(20, 20, [1.0, 1.0, 1.0, 1.0]);
        let wide = run("ec.blur.cameralens", &dot, &[("blurRadius", num(5.0)), ("irisProperties/irisAspectRatio", num(2.0))]);
        let (o, d) = (wide.offset[0] as i64, wide.offset[1] as i64);
        // Lit 8 px to the side but not 8 px above.
        assert!(wide.img.get(20 + o + 8, 20 + d)[3] > 0.0);
        assert_eq!(wide.img.get(20 + o, 20 + d - 8)[3], 0.0);
    }

    #[test]
    fn lens_blur_highlight_saturation_whitens_boosted_colour() {
        let mut img = Image::new(21, 21);
        img.set(10, 10, [1.0, 0.2, 0.2, 1.0]);
        let over = |s: f64| {
            [
                ("blurRadius", num(3.0)),
                ("highlight/specularBrightness", num(100.0)),
                ("highlight/specularThreshold", num(0.0)),
                ("highlight/highlightSaturation", num(s)),
            ]
        };
        let keep = run("ec.blur.cameralens", &img, &over(100.0));
        let white = run("ec.blur.cameralens", &img, &over(0.0));
        let c = |b: &Buf| b.img.get(10 + b.offset[0] as i64, 10 + b.offset[1] as i64);
        // Same red energy, more green with Saturation 0.
        assert!((c(&keep)[0] - c(&white)[0]).abs() < 1e-5);
        assert!(c(&white)[1] > c(&keep)[1] + 1e-4);
    }

    #[test]
    fn compound_blur_reads_the_blur_layer() {
        use crate::{EffectEnv, EffectHost, LayerPixels};
        struct Map;
        impl EffectHost for Map {
            fn layer(&self, _: u64, _: bool) -> Option<LayerPixels> {
                // Left half black (sharp), right half white (full blur).
                let mut m = Image::new(20, 20);
                for y in 0..20 {
                    for x in 10..20 {
                        m.set(x, y, [1.0, 1.0, 1.0, 1.0]);
                    }
                }
                Some(LayerPixels { buf: Buf { img: m, offset: [0.0; 2], scale: 1.0 }, size: [20.0, 20.0] })
            }
            fn audio(&self, _: u64, _: f64, _: usize, _: u32) -> Option<Vec<f32>> {
                None
            }
        }
        let mut img = Image::filled(20, 20, [0.0, 0.0, 0.0, 1.0]);
        for y in 0..20 {
            img.set(4, y, [1.0, 1.0, 1.0, 1.0]);
            img.set(15, y, [1.0, 1.0, 1.0, 1.0]);
        }
        let out = crate::run_fx(
            "ec.blur.compound",
            &[("blurLayer", Value::Layer(Some(1))), ("maximumBlur", num(6.0))],
            img,
            0.0,
            EffectEnv { host: Some(&Map), ..Default::default() },
        );
        let (o, d) = (out.offset[0] as i64, out.offset[1] as i64);
        assert!((out.img.get(4 + o, 10 + d)[0] - 1.0).abs() < 1e-4, "dark map keeps the line sharp");
        assert!(out.img.get(15 + o, 10 + d)[0] < 0.9, "bright map blurs");
    }

    #[test]
    fn iris_spans_hexagon_is_symmetric() {
        let s = iris_spans(10.0, 6, 0.0, 0.0);
        let s0 = s.iter().find(|t| t.0 == 0).unwrap();
        assert_eq!(s0.1, -s0.2);
        let circle = iris_spans(10.0, 0, 0.0, 0.0);
        assert_eq!(circle.len(), 21);
    }

    #[test]
    fn radial_fast_blur_streaks_past_the_layer_edge() {
        // Center left of an opaque square: the streaks run right, beyond the layer rectangle (#2095).
        let img = Image::filled(20, 20, [1.0, 1.0, 1.0, 1.0]);
        let out = run("ec.blur.ccradialfast", &img, &[("center", Value::Vec2([-20.0, 10.0])), ("amount", num(80.0))]);
        assert!(out.img.width > 20 && out.img.height > 20, "{}x{}", out.img.width, out.img.height);
        let (o, d) = (out.offset[0] as i64, out.offset[1] as i64);
        assert!(out.img.get(30 + o, 10 + d)[3] > 0.1, "{:?}", out.img.get(30 + o, 10 + d));
        // Its streak samples only the square: still opaque, so the content keeps its place.
        assert!((out.img.get(19 + o, 10 + d)[3] - 1.0).abs() < 1e-5, "{:?}", out.img.get(19 + o, 10 + d));
        // An adjustment layer is comp-sized already; its buffer is not grown.
        let s = find("ec.blur.ccradialfast").unwrap();
        let params = Params { values: [("center".into(), Value::Vec2([-20.0, 10.0])), ("amount".into(), num(80.0))].into_iter().collect() };
        let ctx = EffectCtx { params: &params, time: 0.0, layer_size: [20.0, 20.0], seed: 1, adjustment: true, env: Default::default() };
        assert_eq!((s.render)(&ctx, Buf { img, offset: [0.0, 0.0], scale: 1.0 }).img.width, 20);
    }

    #[test]
    fn reduce_flicker_blurs_vertically_only() {
        let mut img = Image::filled(8, 8, [0.0, 0.0, 0.0, 1.0]);
        for x in 0..8 {
            img.set(x, 4, [1.0, 1.0, 1.0, 1.0]);
        }
        let out = run("ec.blur.reduceflicker", &img, &[("softness", num(1.0))]).img;
        assert!(out.get(3, 4)[0] < 0.9 && out.get(3, 3)[0] > 0.05);
        assert!((out.get(0, 4)[0] - out.get(7, 4)[0]).abs() < 1e-6);
    }

    #[test]
    fn compound_blur_dark_pixels_stay_sharp() {
        let mut img = Image::new(16, 16);
        for y in 0..16 {
            for x in 0..16 {
                img.set(x, y, if (x + y) % 2 == 0 { [0.0, 0.0, 0.0, 1.0] } else { [0.02, 0.02, 0.02, 1.0] });
            }
        }
        let out = run("ec.blur.compound", &img, &[("maximumBlur", num(10.0))]);
        // Luminance ≈ 0 → (almost) no blur.
        let pad = (out.img.width - 16) / 2;
        assert!(out.img.get(pad as i64, pad as i64)[0] < 0.005);
    }

    #[test]
    fn camera_lens_blur_diffraction_fringe_and_blur_map() {
        use crate::{EffectEnv, EffectHost, LayerPixels};
        // A single bright point spreads into a disc; with Diffraction Fringe its rim is brighter.
        let mut dot = Image::filled(41, 41, [0.0, 0.0, 0.0, 1.0]);
        dot.set(20, 20, [1.0, 1.0, 1.0, 1.0]);
        let v = [("blurRadius", num(10.0)), ("irisProperties/irisShape", Value::Enum(8)), ("repeatEdge", Value::Bool(true))];
        let plain = run("ec.blur.cameralens", &dot, &v).img;
        let ring = run("ec.blur.cameralens", &dot, &[v.as_slice(), &[("irisProperties/diffractionFringe", num(300.0))]].concat()).img;
        assert!((plain.get(20, 20)[0] - plain.get(29, 20)[0]).abs() < 1e-5, "even disc");
        assert!(ring.get(29, 20)[0] > ring.get(20, 20)[0] * 2.0, "{:?} {:?}", ring.get(29, 20), ring.get(20, 20));
        // Blur Map: black (focal distance 0) stays sharp, white gets the full radius.
        struct Half;
        impl EffectHost for Half {
            fn layer(&self, _: u64, _: bool) -> Option<LayerPixels> {
                let mut m = Image::filled(40, 20, [0.0, 0.0, 0.0, 1.0]);
                for y in 0..20 {
                    for x in 20..40 {
                        m.set(x, y, [1.0, 1.0, 1.0, 1.0]);
                    }
                }
                Some(LayerPixels { buf: Buf { img: m, offset: [0.0; 2], scale: 1.0 }, size: [40.0, 20.0] })
            }
            fn audio(&self, _: u64, _: f64, _: usize, _: u32) -> Option<Vec<f32>> {
                None
            }
        }
        let stripes = crate::util::gen_image(40, 20, |x, _| if x % 4 < 2 { [1.0, 1.0, 1.0, 1.0] } else { [0.0, 0.0, 0.0, 1.0] });
        let vals = [("blurRadius", num(6.0)), ("repeatEdge", Value::Bool(true)), ("blurMap/blurMapLayer", Value::Layer(Some(1)))];
        let out = crate::run_fx("ec.blur.cameralens", &vals, stripes.clone(), 0.0, EffectEnv { host: Some(&Half), ..Default::default() }).img;
        assert_eq!(out.get(4, 10), stripes.get(4, 10), "sharp where the map is at the focal distance");
        assert!((out.get(30, 10)[0] - 0.5).abs() < 0.2, "{:?}", out.get(30, 10));
    }
}

//! Projective warps composited straight into a destination (the layer transform step).

use effectcraft_color::{BlendMode, blend_pixel};
use effectcraft_geom::{Mat3, Rect, vec2};
use rayon::prelude::*;

use crate::{Image, Px, hash_noise};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Sampling {
    Nearest,
    #[default]
    Bilinear,
    Bicubic,
}

#[derive(Clone, Copy, Debug)]
pub struct WarpOpts {
    pub sampling: Sampling,
    pub opacity: f32,
    pub mode: BlendMode,
    pub seed: u32,
    /// Only touch destination pixels inside this rect (dst pixel coords).
    pub clip: Option<Rect>,
}

impl Default for WarpOpts {
    fn default() -> Self {
        WarpOpts { sampling: Sampling::Bilinear, opacity: 1.0, mode: BlendMode::Normal, seed: 0, clip: None }
    }
}

/// Box-filter halving (for minification).
fn half(src: &Image) -> Image {
    let w = src.width.div_ceil(2).max(1);
    let h = src.height.div_ceil(2).max(1);
    let mut out = Image::new(w, h);
    out.rows_mut().for_each(|(y, row)| {
        for (x, px) in row.iter_mut().enumerate() {
            let (sx, sy) = (2 * x as i64, 2 * y as i64);
            let mut acc = [0.0f32; 4];
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let p = src.get_clamped(sx + dx, sy + dy);
                for c in 0..4 {
                    acc[c] += p[c];
                }
            }
            *px = acc.map(|v| v * 0.25);
        }
    });
    out
}

/// Draw `src` transformed by `m` (src pixel space → dst pixel space) onto `dst` with blending.
///
/// Affine transforms with non-stencil modes take a fast path (per-row span clipping, interior
/// bilinear without bounds checks, integer-translation copies, inlined Normal blending); it
/// matches [`composite_warp_reference`] to float rounding.
pub fn composite_warp(dst: &mut Image, src: &Image, m: &Mat3, opts: &WarpOpts) {
    composite_warp_impl(dst, src, m, opts, true, None);
}

/// `dst += warp(src) · weight` (plain sum, no blending): motion-blur sub-samples accumulate
/// straight into one buffer instead of one full-size image per sample.
pub fn accumulate_warp(dst: &mut Image, src: &Image, m: &Mat3, sampling: Sampling, weight: f32) {
    let opts = WarpOpts { sampling, ..Default::default() };
    if m.is_affine() && m.inverse().is_some_and(|i| i.is_affine()) {
        composite_warp_impl(dst, src, m, &opts, true, Some(weight));
        return;
    }
    let mut tmp = Image::new(dst.width, dst.height);
    composite_warp(&mut tmp, src, m, &opts);
    dst.data.par_iter_mut().zip(tmp.data.par_iter()).for_each(|(d, s)| {
        for c in 0..4 {
            d[c] += s[c] * weight;
        }
    });
}

/// The straightforward per-pixel implementation (reference for the fast path's tests).
pub fn composite_warp_reference(dst: &mut Image, src: &Image, m: &Mat3, opts: &WarpOpts) {
    composite_warp_impl(dst, src, m, opts, false, None);
}

/// Bilinear blend of four taps (same operation order as [`Image::sample_bilinear`]).
#[inline(always)]
fn bilerp(a: Px, b: Px, c: Px, d: Px, tx: f32, ty: f32) -> Px {
    let mut o = [0.0; 4];
    for i in 0..4 {
        let top = a[i] + (b[i] - a[i]) * tx;
        let bot = c[i] + (d[i] - c[i]) * tx;
        o[i] = top + (bot - top) * ty;
    }
    o
}

/// Bilinear sample with an unchecked interior fast path.
#[inline(always)]
fn sample_bilinear_fast(img: &Image, x: f64, y: f64) -> Px {
    let fx = x - 0.5;
    let fy = y - 0.5;
    let x0 = fx.floor();
    let y0 = fy.floor();
    let (xi, yi) = (x0 as i64, y0 as i64);
    let (w, h) = (img.width as i64, img.height as i64);
    if xi >= 0 && yi >= 0 && xi + 1 < w && yi + 1 < h {
        let tx = (fx - x0) as f32;
        let ty = (fy - y0) as f32;
        let i = yi as usize * w as usize + xi as usize;
        let row0 = &img.data[i..i + 2];
        let row1 = &img.data[i + w as usize..i + w as usize + 2];
        return bilerp(row0[0], row0[1], row1[0], row1[1], tx, ty);
    }
    img.sample_bilinear(x, y)
}

/// Index range `[lo, hi)` of `i` in `0..n` for which `p0 + i·s` may lie in `[min, max]`
/// (conservative by one step on each side; callers still check per pixel).
fn span(p0: f64, s: f64, min: f64, max: f64, n: usize) -> (usize, usize) {
    if s.abs() < 1e-12 {
        return if p0 >= min - 1e-9 && p0 <= max + 1e-9 { (0, n) } else { (0, 0) };
    }
    let (a, b) = ((min - p0) / s, (max - p0) / s);
    let (a, b) = if a < b { (a, b) } else { (b, a) };
    let lo = (a.floor() - 1.0).max(0.0);
    let hi = (b.ceil() + 2.0).min(n as f64);
    if hi <= lo { (0, 0) } else { (lo as usize, hi as usize) }
}

/// Fast affine rows: `blend(dst, src, x, y)` is only called for visible source pixels.
#[allow(clippy::too_many_arguments)]
fn affine_rows(
    dst: &mut Image,
    img: &Image,
    inv: &Mat3,
    sampling: Sampling,
    op: f32,
    x0: usize,
    x1: usize,
    y0: usize,
    y1: usize,
    blend: impl Fn(Px, Px, u32, u32) -> Px + Sync,
) {
    let w = dst.width as usize;
    let m = inv.0;
    let (iw, ih) = (img.width as f64, img.height as f64);
    let step = vec2(m[0][0], m[1][0]);
    // Pure integer translation: every sample lands on a pixel centre, so all filters return the
    // source pixel itself (bicubic still clamps negatives, as in the reference).
    let int_tx = m[0][0] == 1.0 && m[1][1] == 1.0 && m[0][1] == 0.0 && m[1][0] == 0.0 && m[0][2].fract() == 0.0 && m[1][2].fract() == 0.0;
    dst.data.par_chunks_mut(w).enumerate().skip(y0).take(y1 - y0).for_each(|(y, row)| {
        let fy = y as f64 + 0.5;
        let p0 = vec2(m[0][0] * (x0 as f64 + 0.5) + m[0][1] * fy + m[0][2], m[1][0] * (x0 as f64 + 0.5) + m[1][1] * fy + m[1][2]);
        let n = x1 - x0;
        let (ax, bx) = span(p0.x, step.x, -1.0, iw + 1.0, n);
        let (ay, by) = span(p0.y, step.y, -1.0, ih + 1.0, n);
        let (lo, hi) = (ax.max(ay), bx.min(by));
        if lo >= hi {
            return;
        }
        if int_tx {
            let sy = y as i64 + m[1][2] as i64;
            if sy < 0 || sy >= img.height as i64 {
                return;
            }
            let dx = m[0][2] as i64;
            let srow = &img.data[sy as usize * img.width as usize..(sy as usize + 1) * img.width as usize];
            let xa = (x0 as i64 + lo as i64).max(-dx).max(0) as usize;
            let xb = ((x0 + hi) as i64).min(img.width as i64 - dx).max(0) as usize;
            for x in xa..xb.max(xa) {
                let mut s = srow[(x as i64 + dx) as usize];
                if sampling == Sampling::Bicubic {
                    s = [s[0].max(0.0), s[1].max(0.0), s[2].max(0.0), s[3].max(0.0)];
                }
                if s[3] <= 0.0 {
                    continue;
                }
                if op < 1.0 {
                    for c in s.iter_mut() {
                        *c *= op;
                    }
                }
                row[x] = blend(row[x], s, x as u32, y as u32);
            }
            return;
        }
        for i in lo..hi {
            let x = x0 + i;
            let p = vec2(p0.x + step.x * i as f64, p0.y + step.y * i as f64);
            if p.x < -1.0 || p.y < -1.0 || p.x > iw + 1.0 || p.y > ih + 1.0 {
                continue;
            }
            let mut s = match sampling {
                Sampling::Nearest => img.get(p.x.floor() as i64, p.y.floor() as i64),
                Sampling::Bilinear => sample_bilinear_fast(img, p.x, p.y),
                Sampling::Bicubic => img.sample_bicubic(p.x, p.y),
            };
            if s[3] <= 0.0 {
                continue;
            }
            if op < 1.0 {
                for c in s.iter_mut() {
                    *c *= op;
                }
            }
            row[x] = blend(row[x], s, x as u32, y as u32);
        }
    });
}

fn composite_warp_impl(dst: &mut Image, src: &Image, m: &Mat3, opts: &WarpOpts, allow_fast: bool, accumulate: Option<f32>) {
    if src.is_empty() || dst.is_empty() || opts.opacity <= 0.0 {
        return;
    }
    let Some(inv) = m.inverse() else { return };
    // Minification: pick a pre-filtered level so sampling doesn't alias.
    let scale = m.mean_scale();
    let mut level_img;
    let mut img = src;
    let mut inv = inv;
    if scale < 0.5 && m.is_affine() {
        let mut s = scale;
        let mut f = 1.0;
        level_img = half(src);
        f *= 0.5;
        s *= 2.0;
        while s < 0.5 && level_img.width > 1 && level_img.height > 1 {
            level_img = half(&level_img);
            f *= 0.5;
            s *= 2.0;
        }
        img = &level_img;
        inv = Mat3::scale(vec2(f, f)) * inv;
    }
    let bounds = m.map_rect(&Rect::from_size(src.width as f64, src.height as f64)).inflate(1.0).round_out();
    let full = Rect::from_size(dst.width as f64, dst.height as f64);
    let mut area = bounds.intersect(&full);
    if let Some(c) = opts.clip {
        area = area.intersect(&c);
    }
    if area.is_empty() {
        return;
    }
    let (x0, x1) = (area.x0.max(0.0) as usize, area.x1.min(dst.width as f64) as usize);
    let (y0, y1) = (area.y0.max(0.0) as usize, area.y1.min(dst.height as f64) as usize);
    let w = dst.width as usize;
    let affine = inv.is_affine();
    let mode = opts.mode;
    let op = opts.opacity;
    if let Some(k) = accumulate
        && affine
    {
        affine_rows(dst, img, &inv, opts.sampling, 1.0, x0, x1, y0, y1, |d, s, _, _| [d[0] + s[0] * k, d[1] + s[1] * k, d[2] + s[2] * k, d[3] + s[3] * k]);
        return;
    }
    if allow_fast && affine && !mode.is_stencil() {
        match mode {
            BlendMode::Normal => affine_rows(dst, img, &inv, opts.sampling, op, x0, x1, y0, y1, |d, s, _, _| {
                let k = 1.0 - s[3];
                [s[0] + d[0] * k, s[1] + d[1] * k, s[2] + d[2] * k, s[3] + d[3] * k]
            }),
            BlendMode::Dissolve | BlendMode::DancingDissolve => {
                let seed = opts.seed;
                affine_rows(dst, img, &inv, opts.sampling, op, x0, x1, y0, y1, |d, s, x, y| blend_pixel(mode, d, s, hash_noise(x, y, seed)))
            }
            _ => affine_rows(dst, img, &inv, opts.sampling, op, x0, x1, y0, y1, |d, s, _, _| blend_pixel(mode, d, s, 0.5)),
        }
        return;
    }
    dst.data.par_chunks_mut(w).enumerate().skip(y0).take(y1 - y0).for_each(|(y, row)| {
        let fy = y as f64 + 0.5;
        let mut sp = inv.apply(vec2(x0 as f64 + 0.5, fy));
        let step = inv.apply_vec(vec2(1.0, 0.0));
        for x in x0..x1 {
            let p = if affine {
                let p = sp;
                sp += step;
                p
            } else {
                inv.apply(vec2(x as f64 + 0.5, fy))
            };
            if p.x < -1.0 || p.y < -1.0 || p.x > img.width as f64 + 1.0 || p.y > img.height as f64 + 1.0 {
                if mode.is_stencil() {
                    row[x] = blend_pixel(mode, row[x], [0.0; 4], 0.5);
                }
                continue;
            }
            let mut s = match opts.sampling {
                Sampling::Nearest => img.get(p.x.floor() as i64, p.y.floor() as i64),
                Sampling::Bilinear => img.sample_bilinear(p.x, p.y),
                Sampling::Bicubic => img.sample_bicubic(p.x, p.y),
            };
            if s[3] <= 0.0 && !mode.is_stencil() {
                continue;
            }
            if op < 1.0 {
                for c in s.iter_mut() {
                    *c *= op;
                }
            }
            let n = if matches!(mode, BlendMode::Dissolve | BlendMode::DancingDissolve) { hash_noise(x as u32, y as u32, opts.seed) } else { 0.5 };
            row[x] = blend_pixel(mode, row[x], s, n);
        }
    });
    // Stencil modes also clear everything outside the layer bounds.
    if mode.is_stencil() && matches!(mode, BlendMode::StencilAlpha | BlendMode::StencilLuma) {
        for (y, row) in dst.data.chunks_mut(w).enumerate() {
            for (x, px) in row.iter_mut().enumerate() {
                if x < x0 || x >= x1 || y < y0 || y >= y1 {
                    *px = [0.0; 4];
                }
            }
        }
    }
}

/// Resize with bilinear filtering (box pre-filter when shrinking).
pub fn resample(src: &Image, w: u32, h: u32) -> Image {
    let mut out = Image::new(w, h);
    if src.is_empty() || w == 0 || h == 0 {
        return out;
    }
    let m = Mat3::scale(vec2(w as f64 / src.width as f64, h as f64 / src.height as f64));
    composite_warp(&mut out, src, &m, &WarpOpts::default());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_warp_copies() {
        let mut src = Image::new(4, 4);
        src.set(1, 2, [0.5, 0.5, 0.5, 1.0]);
        let mut dst = Image::new(4, 4);
        composite_warp(&mut dst, &src, &Mat3::IDENTITY, &WarpOpts::default());
        assert_eq!(dst.get(1, 2), [0.5, 0.5, 0.5, 1.0]);
        assert_eq!(dst.get(0, 0), [0.0; 4]);
    }

    #[test]
    fn minification_prefilters_high_frequency_details() {
        let mut src = Image::new(8, 8);
        for y in 0..8 {
            for x in 0..8 {
                let v = if (x + y) % 2 == 0 { 1.0 } else { 0.0 };
                src.set(x, y, [v, v, v, 1.0]);
            }
        }
        let out = resample(&src, 2, 2);
        for px in out.data {
            assert!((px[0] - 0.5).abs() < 1e-6, "{px:?}");
            assert_eq!(px[3], 1.0);
        }
    }

    #[test]
    fn translate_by_integer() {
        let mut src = Image::new(4, 4);
        src.set(0, 0, [1.0; 4]);
        let mut dst = Image::new(8, 8);
        composite_warp(&mut dst, &src, &Mat3::translate(vec2(3.0, 2.0)), &WarpOpts::default());
        assert_eq!(dst.get(3, 2), [1.0; 4]);
    }

    fn test_image(w: u32, h: u32, seed: u32) -> Image {
        let mut img = Image::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let a = if (x + y) % 7 == 0 { 0.0 } else { hash_noise(x, y, seed) };
                img.set(x, y, [hash_noise(x, y, seed + 1) * a, hash_noise(x, y, seed + 2) * a, hash_noise(x, y, seed + 3) * a, a]);
            }
        }
        img
    }

    /// The fast affine path equals the reference per-pixel path (modes, filters, transforms).
    #[test]
    fn fast_path_matches_reference() {
        let src = test_image(37, 23, 5);
        let mats = [
            Mat3::IDENTITY,
            Mat3::translate(vec2(5.0, -3.0)),
            Mat3::translate(vec2(-40.0, 2.0)),
            Mat3::translate(vec2(3.25, 7.5)),
            Mat3::translate(vec2(20.0, 10.0)) * Mat3::rotate_deg(23.0) * Mat3::scale(vec2(1.3, 0.8)),
            Mat3::translate(vec2(30.0, 5.0)) * Mat3::rotate_deg(-120.0) * Mat3::scale(vec2(0.7, 0.7)),
            Mat3::translate(vec2(10.0, 10.0)) * Mat3::scale(vec2(-1.0, 1.0)),
            Mat3::scale(vec2(0.3, 0.3)),
        ];
        let modes = [BlendMode::Normal, BlendMode::Add, BlendMode::Multiply, BlendMode::Screen, BlendMode::Dissolve, BlendMode::Overlay];
        for (mi, m) in mats.iter().enumerate() {
            for mode in modes {
                for sampling in [Sampling::Nearest, Sampling::Bilinear, Sampling::Bicubic] {
                    for opacity in [1.0, 0.6] {
                        let opts = WarpOpts { sampling, opacity, mode, seed: 9, clip: None };
                        let mut a = test_image(48, 40, 77);
                        let mut b = a.clone();
                        composite_warp(&mut a, &src, m, &opts);
                        composite_warp_reference(&mut b, &src, m, &opts);
                        for (i, (p, q)) in a.data.iter().zip(&b.data).enumerate() {
                            for c in 0..4 {
                                assert!((p[c] - q[c]).abs() < 1e-4, "mat {mi} {mode:?} {sampling:?} op {opacity} px {i}: {p:?} vs {q:?}");
                            }
                        }
                    }
                }
            }
        }
    }

    /// Accumulating sub-samples equals warping each into its own image and summing.
    #[test]
    fn accumulate_matches_per_sample_images() {
        let src = test_image(30, 20, 11);
        let persp = Mat3::square_to_quad([vec2(5.0, 4.0), vec2(40.0, 2.0), vec2(44.0, 30.0), vec2(2.0, 35.0)]) * Mat3::scale(vec2(1.0 / 30.0, 1.0 / 20.0));
        let mats = [Mat3::translate(vec2(3.0, 4.0)), Mat3::translate(vec2(5.5, 4.25)) * Mat3::rotate_deg(12.0), persp];
        let k = 1.0 / mats.len() as f32;
        let mut fast = Image::new(48, 40);
        let mut slow = Image::new(48, 40);
        for m in &mats {
            accumulate_warp(&mut fast, &src, m, Sampling::Bilinear, k);
            let mut img = Image::new(48, 40);
            composite_warp_reference(&mut img, &src, m, &WarpOpts::default());
            for (a, b) in slow.data.iter_mut().zip(&img.data) {
                for c in 0..4 {
                    a[c] += b[c] * k;
                }
            }
        }
        for (p, q) in fast.data.iter().zip(&slow.data) {
            for c in 0..4 {
                assert!((p[c] - q[c]).abs() < 1e-4, "{p:?} vs {q:?}");
            }
        }
    }

    #[test]
    fn downscale_preserves_average() {
        let src = Image::filled(64, 64, [0.25, 0.5, 0.75, 1.0]);
        let out = resample(&src, 8, 8);
        let p = out.get(4, 4);
        assert!((p[1] - 0.5).abs() < 1e-4 && (p[3] - 1.0).abs() < 1e-4, "{p:?}");
    }
}

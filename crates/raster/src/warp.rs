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

/// Half the width and height (in source pixels) of what a destination pixel averages when `m`
/// shrinks the source on either of its own axes: a box of `1 / scale` on a shrunken axis and of
/// one pixel (which is a bilinear read) on any other. `None` when neither axis shrinks. The GPU
/// compositor samples with the same footprint.
pub fn box_footprint(m: &Mat3) -> Option<(f64, f64)> {
    let scale = |v: effectcraft_geom::Vec2| v.x.hypot(v.y);
    let (sx, sy) = (scale(m.apply_vec(vec2(1.0, 0.0))), scale(m.apply_vec(vec2(0.0, 1.0))));
    let shrunk = |s: f64| s < 1.0 - 1e-6;
    let half = |s: f64| if shrunk(s) { 0.5 / s } else { 0.5 };
    (shrunk(sx) || shrunk(sy)).then(|| (half(sx), half(sy)))
}

/// The average of `img` over the box of half-sizes `(hx, hy)` centred on `(x, y)`, the box lined
/// up with the source's own axes however the layer is rotated, as After Effects filters a layer
/// it shrinks. What lies outside the image is transparent.
fn sample_box(img: &Image, x: f64, y: f64, (hx, hy): (f64, f64)) -> Px {
    let (w, h) = (img.width as usize, img.height as usize);
    let (l, r, t, b) = (x - hx, x + hx, y - hy, y + hy);
    let range = |lo: f64, hi: f64, n: usize| ((lo.floor().max(0.0) as usize).min(n), (hi.ceil().max(0.0) as usize).min(n));
    let (x0, x1) = range(l, r, w);
    let (y0, y1) = range(t, b, h);
    let per_area = 1.0 / (4.0 * hx * hy);
    // Summed in f64: the weights of a solid area must add up to exactly 1.
    let mut out = [0.0f64; 4];
    for j in y0..y1 {
        let cover_y = b.min((j + 1) as f64) - t.max(j as f64);
        let line = &img.data[j * w..(j + 1) * w];
        for (i, p) in line.iter().enumerate().take(x1).skip(x0) {
            let k = (r.min((i + 1) as f64) - l.max(i as f64)) * cover_y * per_area;
            for (c, v) in out.iter_mut().zip(p) {
                *c += *v as f64 * k;
            }
        }
    }
    out.map(|v| v as f32)
}

/// Draw `src` transformed by `m` (src pixel space → dst pixel space) onto `dst` with blending.
///
/// Affine transforms with non-stencil modes take a fast path (per-row span clipping, interior
/// bilinear without bounds checks, integer-translation copies, inlined Normal blending); it
/// matches [`composite_warp_reference`] to float rounding.
pub fn composite_warp(dst: &mut Image, src: &Image, m: &Mat3, opts: &WarpOpts) {
    composite_warp_impl(dst, src, m, opts, Path::Fast, None, None);
}

/// [`composite_warp`], also returning the destination pixels it changed when it knows them:
/// `Some([x0, y0, x1, y1])` (half-open, empty when nothing was drawn) on the fast affine path,
/// `None` otherwise. With `quantize` (an 8/16 bpc destination's levels), the fast path also
/// quantises each pixel it blends ([`crate::quantize_px`]): exactly when this returns `Some`.
pub fn composite_warp_changed(dst: &mut Image, src: &Image, m: &Mat3, opts: &WarpOpts, quantize: Option<f32>) -> Option<[u32; 4]> {
    composite_warp_impl(dst, src, m, opts, Path::Fast, None, quantize)
}

/// `dst += warp(src) · weight` (plain sum, no blending): motion-blur sub-samples accumulate
/// straight into one buffer instead of one full-size image per sample.
pub fn accumulate_warp(dst: &mut Image, src: &Image, m: &Mat3, sampling: Sampling, weight: f32) {
    let opts = WarpOpts { sampling, ..Default::default() };
    if m.is_affine() && m.inverse().is_some_and(|i| i.is_affine()) {
        composite_warp_impl(dst, src, m, &opts, Path::Fast, Some(weight), None);
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
    composite_warp_impl(dst, src, m, opts, Path::Reference, None, None);
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

/// Side of the square source tiles of [`ClearTiles`].
const TILE: usize = 16;

/// Which `TILE`×`TILE` source tiles are fully transparent, the one-pixel apron to their right
/// and below included: the four taps of a bilinear sample whose top-left tap lies in such a
/// tile all have zero alpha, so the sample is transparent and draws nothing.
struct ClearTiles {
    cols: usize,
    clear: Vec<bool>,
}

impl ClearTiles {
    fn new(img: &Image) -> ClearTiles {
        let (w, h) = (img.width as usize, img.height as usize);
        let (cols, rows) = (w.div_ceil(TILE), h.div_ceil(TILE));
        let clear = (0..rows)
            .into_par_iter()
            .flat_map_iter(|ty| {
                let mut row_clear = vec![true; cols];
                for y in ty * TILE..(ty * TILE + TILE + 1).min(h) {
                    let row = &img.data[y * w..(y + 1) * w];
                    for (tx, c) in row_clear.iter_mut().enumerate() {
                        if *c {
                            *c = row[tx * TILE..(tx * TILE + TILE + 1).min(w)].iter().all(|p| p[3] == 0.0);
                        }
                    }
                }
                row_clear
            })
            .collect();
        ClearTiles { cols, clear }
    }

    #[inline(always)]
    fn is_clear(&self, x: usize, y: usize) -> bool {
        self.clear.get((y / TILE) * self.cols + x / TILE).copied().unwrap_or(false)
    }
}

/// Bilinear sample with an unchecked interior fast path; `None` when every interior tap is in a
/// [`ClearTiles`] tile (the sample would have zero alpha).
#[inline(always)]
fn sample_bilinear_fast(img: &Image, clear: &ClearTiles, x: f64, y: f64) -> Option<Px> {
    let fx = x - 0.5;
    let fy = y - 0.5;
    let x0 = fx.floor();
    let y0 = fy.floor();
    let (xi, yi) = (x0 as i64, y0 as i64);
    let (w, h) = (img.width as i64, img.height as i64);
    if xi >= 0 && yi >= 0 && xi + 1 < w && yi + 1 < h {
        if clear.is_clear(xi as usize, yi as usize) {
            return None;
        }
        let tx = (fx - x0) as f32;
        let ty = (fy - y0) as f32;
        let i = yi as usize * w as usize + xi as usize;
        let row0 = &img.data[i..i + 2];
        let row1 = &img.data[i + w as usize..i + w as usize + 2];
        return Some(bilerp(row0[0], row0[1], row1[0], row1[1], tx, ty));
    }
    Some(img.sample_bilinear(x, y))
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

/// Columns `[a, b)` of one destination row that a warp changed (`a == b`: none).
#[derive(Clone, Copy)]
struct RowSpan {
    a: usize,
    b: usize,
}

impl RowSpan {
    const NONE: RowSpan = RowSpan { a: usize::MAX, b: 0 };
    #[inline(always)]
    fn add(&mut self, x: usize) {
        self.a = self.a.min(x);
        self.b = x + 1;
    }
}

/// Union of changed pixels as `[x0, y0, x1, y1]` (half-open; `x0 >= x1` when empty).
fn union(r: [usize; 4], q: [usize; 4]) -> [usize; 4] {
    match (r[0] < r[2], q[0] < q[2]) {
        (false, _) => q,
        (_, false) => r,
        _ => [r[0].min(q[0]), r[1].min(q[1]), r[2].max(q[2]), r[3].max(q[3])],
    }
}

/// Sampling, blending and destination bounds for an affine row operation.
struct AffineRows {
    sampling: Sampling,
    /// Bilinear sampling of a shrunken source: see [`box_footprint`].
    footprint: Option<(f64, f64)>,
    opacity: f32,
    quantize: Option<f32>,
    bounds: [usize; 4],
}

/// Blend visible source samples and return the rectangle of changed pixels.
fn affine_rows(dst: &mut Image, img: &Image, inv: &Mat3, rows: AffineRows, blend: impl Fn(Px, Px, u32, u32) -> Px + Sync) -> [usize; 4] {
    let AffineRows { sampling, footprint, opacity: op, quantize, bounds: [x0, x1, y0, y1] } = rows;
    let levels = quantize.map(|l| (l, 1.0 / l));
    let w = dst.width as usize;
    let m = inv.0;
    let (iw, ih) = (img.width as f64, img.height as f64);
    let step = vec2(m[0][0], m[1][0]);
    // Pure integer translation: every sample lands on a pixel centre, so all filters return the
    // source pixel itself (bicubic still clamps negatives, as in the reference).
    let (mx, my) = footprint.map_or((1.0, 1.0), |(hx, hy)| (hx.max(1.0), hy.max(1.0)));
    let int_tx = m[0][0] == 1.0 && m[1][1] == 1.0 && m[0][1] == 0.0 && m[1][0] == 0.0 && m[0][2].fract() == 0.0 && m[1][2].fract() == 0.0;
    // Bilinear samples skip the transparent parts of the source (most of a thin shape's or a
    // glow-padded layer's buffer) without reading them.
    let clear = (sampling == Sampling::Bilinear && !int_tx && footprint.is_none()).then(|| ClearTiles::new(img));
    let empty = [usize::MAX, usize::MAX, 0, 0];
    dst.data
        .par_chunks_mut(w)
        .enumerate()
        .skip(y0)
        .take(y1 - y0)
        .map(|(y, row)| {
            let mut changed = RowSpan::NONE;
            let fy = y as f64 + 0.5;
            let p0 = vec2(m[0][0] * (x0 as f64 + 0.5) + m[0][1] * fy + m[0][2], m[1][0] * (x0 as f64 + 0.5) + m[1][1] * fy + m[1][2]);
            let n = x1 - x0;
            let (ax, bx) = span(p0.x, step.x, -mx, iw + mx, n);
            let (ay, by) = span(p0.y, step.y, -my, ih + my, n);
            let (lo, hi) = (ax.max(ay), bx.min(by));
            if lo >= hi {
                return empty;
            }
            if int_tx {
                let sy = y as i64 + m[1][2] as i64;
                if sy < 0 || sy >= img.height as i64 {
                    return empty;
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
                    if let Some((l, inv)) = levels {
                        crate::quantize_px(&mut row[x], l, inv);
                    }
                    changed.add(x);
                }
            } else {
                for i in lo..hi {
                    let x = x0 + i;
                    let p = vec2(p0.x + step.x * i as f64, p0.y + step.y * i as f64);
                    if p.x < -mx || p.y < -my || p.x > iw + mx || p.y > ih + my {
                        continue;
                    }
                    let mut s = match (sampling, &clear) {
                        (Sampling::Bilinear, None) if footprint.is_some() => footprint.map_or([0.0; 4], |f| sample_box(img, p.x, p.y, f)),
                        (Sampling::Nearest, _) => img.get(p.x.floor() as i64, p.y.floor() as i64),
                        (Sampling::Bilinear, Some(clear)) => match sample_bilinear_fast(img, clear, p.x, p.y) {
                            Some(s) => s,
                            None => continue,
                        },
                        (Sampling::Bilinear, None) => img.sample_bilinear(p.x, p.y),
                        (Sampling::Bicubic, _) => img.sample_bicubic(p.x, p.y),
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
                    if let Some((l, inv)) = levels {
                        crate::quantize_px(&mut row[x], l, inv);
                    }
                    changed.add(x);
                }
            }
            if changed.a < changed.b { [changed.a, y, changed.b, y + 1] } else { empty }
        })
        .reduce(|| empty, union)
}

/// How [`composite_warp_impl`] draws.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Path {
    /// Row spans and the other fast paths of [`composite_warp`].
    Fast,
    /// One pixel at a time ([`composite_warp_reference`]).
    Reference,
    /// [`Path::Fast`], shrinking through the halving pyramid rather than a box under each pixel:
    /// [`resample`], whose results callers build on (half-size footage conversion matches it).
    Pyramid,
}

fn composite_warp_impl(
    dst: &mut Image,
    src: &Image,
    m: &Mat3,
    opts: &WarpOpts,
    path: Path,
    accumulate: Option<f32>,
    quantize: Option<f32>,
) -> Option<[u32; 4]> {
    const NOTHING: Option<[u32; 4]> = Some([0; 4]);
    if src.is_empty() || dst.is_empty() || opts.opacity <= 0.0 {
        return NOTHING;
    }
    let Some(inv) = m.inverse() else { return NOTHING };
    // Minification: pick a pre-filtered level so sampling doesn't alias.
    let scale = m.mean_scale();
    let mut level_img;
    let mut img = src;
    let mut inv = inv;
    let footprint = if m.is_affine() && opts.sampling == Sampling::Bilinear && path != Path::Pyramid { box_footprint(m) } else { None };
    if footprint.is_none() && scale < 0.5 && m.is_affine() {
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
        return NOTHING;
    }
    let (x0, x1) = (area.x0.max(0.0) as usize, area.x1.min(dst.width as f64) as usize);
    let (y0, y1) = (area.y0.max(0.0) as usize, area.y1.min(dst.height as f64) as usize);
    let w = dst.width as usize;
    let affine = inv.is_affine();
    let mode = opts.mode;
    let op = opts.opacity;
    let changed = |r: [usize; 4]| Some(r.map(|v| v.min(u32::MAX as usize) as u32));
    if let Some(k) = accumulate
        && affine
    {
        let r = affine_rows(
            dst,
            img,
            &inv,
            AffineRows { sampling: opts.sampling, footprint, opacity: 1.0, quantize: None, bounds: [x0, x1, y0, y1] },
            |d, s, _, _| [d[0] + s[0] * k, d[1] + s[1] * k, d[2] + s[2] * k, d[3] + s[3] * k],
        );
        return changed(r);
    }
    if path != Path::Reference && affine && !mode.is_stencil() {
        let r = match mode {
            BlendMode::Normal => {
                affine_rows(dst, img, &inv, AffineRows { sampling: opts.sampling, footprint, opacity: op, quantize, bounds: [x0, x1, y0, y1] }, |d, s, _, _| {
                    let k = 1.0 - s[3];
                    [s[0] + d[0] * k, s[1] + d[1] * k, s[2] + d[2] * k, s[3] + d[3] * k]
                })
            }
            BlendMode::Dissolve | BlendMode::DancingDissolve => {
                let seed = opts.seed;
                affine_rows(dst, img, &inv, AffineRows { sampling: opts.sampling, footprint, opacity: op, quantize, bounds: [x0, x1, y0, y1] }, |d, s, x, y| {
                    blend_pixel(mode, d, s, hash_noise(x, y, seed))
                })
            }
            _ => {
                affine_rows(dst, img, &inv, AffineRows { sampling: opts.sampling, footprint, opacity: op, quantize, bounds: [x0, x1, y0, y1] }, |d, s, _, _| {
                    blend_pixel(mode, d, s, 0.5)
                })
            }
        };
        return changed(r);
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
            let (mx, my) = footprint.map_or((1.0, 1.0), |(hx, hy)| (hx.max(1.0), hy.max(1.0)));
            if p.x < -mx || p.y < -my || p.x > img.width as f64 + mx || p.y > img.height as f64 + my {
                if mode.is_stencil() {
                    row[x] = blend_pixel(mode, row[x], [0.0; 4], 0.5);
                }
                continue;
            }
            let mut s = match opts.sampling {
                Sampling::Nearest => img.get(p.x.floor() as i64, p.y.floor() as i64),
                Sampling::Bilinear => footprint.map_or_else(|| img.sample_bilinear(p.x, p.y), |f| sample_box(img, p.x, p.y, f)),
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
    None
}

/// Resize with bilinear filtering (box pre-filter when shrinking).
pub fn resample(src: &Image, w: u32, h: u32) -> Image {
    let mut out = Image::new(w, h);
    if src.is_empty() || w == 0 || h == 0 {
        return out;
    }
    let m = Mat3::scale(vec2(w as f64 / src.width as f64, h as f64 / src.height as f64));
    composite_warp_impl(&mut out, src, &m, &WarpOpts::default(), Path::Pyramid, None, None);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows_image(rows: &[f32]) -> Image {
        let mut src = Image::new(3, rows.len() as u32);
        for (y, v) in rows.iter().enumerate() {
            for x in 0..3 {
                src.set(x, y as u32, [*v, *v, *v, 1.0]);
            }
        }
        src
    }

    fn test_rows() -> Vec<f32> {
        (0..40).map(|i| if i % 7 == 0 || i % 3 == 1 { 1.0 } else { 0.0 }).collect()
    }

    #[test]
    fn a_layer_squashed_on_one_axis_averages_the_area_under_each_pixel() {
        let rows = test_rows();
        for (ay, rows_per_pixel) in [(0.05, 20.0), (0.4, 2.5)] {
            let mut dst = Image::new(3, 8);
            composite_warp(&mut dst, &rows_image(&rows), &Mat3::scale(vec2(1.0, ay)), &WarpOpts::default());
            let want: f32 = (0..rows.len()).map(|i| ((rows_per_pixel as f32).min((i + 1) as f32) - (i as f32).max(0.0)).max(0.0) * rows[i]).sum::<f32>()
                / rows_per_pixel as f32;
            assert!((dst.get(1, 0)[0] - want).abs() < 1e-4, "{ay}: {:?} vs {want}", dst.get(1, 0));
        }
    }

    #[test]
    fn a_fractional_position_averages_around_the_destination_pixel() {
        // Scale 0.2 placed 0.5 down: destination row 1 covers source rows 2.5 to 7.5.
        let rows = test_rows();
        let mut dst = Image::new(3, 10);
        composite_warp(&mut dst, &rows_image(&rows), &(Mat3::translate(vec2(0.0, 0.5)) * Mat3::scale(vec2(1.0, 0.2))), &WarpOpts::default());
        let want = (rows[2] * 0.5 + rows[3] + rows[4] + rows[5] + rows[6] + rows[7] * 0.5) / 5.0;
        assert!((dst.get(1, 1)[0] - want).abs() < 1e-4, "{:?} vs {want}", dst.get(1, 1));
    }

    #[test]
    fn a_box_of_one_pixel_is_a_bilinear_read() {
        let mut img = Image::new(2, 2);
        img.set(0, 0, [1.0, 0.0, 0.0, 1.0]);
        img.set(1, 1, [0.0, 0.0, 1.0, 1.0]);
        for (x, y) in [(0.7, 0.9), (1.2, 1.4), (1.0, 1.0)] {
            let (a, b) = (sample_box(&img, x, y, (0.5, 0.5)), img.sample_bilinear(x, y));
            for (p, q) in a.iter().zip(b) {
                assert!((p - q).abs() < 1e-5, "({x}, {y}): {a:?} vs {b:?}");
            }
        }
    }

    #[test]
    fn a_shrunken_layer_fades_out_past_its_edge() {
        // Half the box beyond the image is transparent.
        let img = Image::filled(4, 4, [1.0, 1.0, 1.0, 1.0]);
        let edge = sample_box(&img, 0.0, 2.0, (2.0, 0.5));
        assert!((edge[3] - 0.5).abs() < 1e-5, "{edge:?}");
    }

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

    /// A mostly transparent source: a thin ring with a few stray pixels (one with negative zero alpha).
    fn sparse_image(w: u32, h: u32) -> Image {
        let mut img = Image::new(w, h);
        let (cx, cy, r) = (w as f32 / 2.0, h as f32 / 2.0, w.min(h) as f32 * 0.4);
        for y in 0..h {
            for x in 0..w {
                let d = ((x as f32 + 0.5 - cx).hypot(y as f32 + 0.5 - cy) - r).abs();
                if d < 1.5 {
                    let a = 1.0 - d / 1.5;
                    img.set(x, y, [0.2 * a, 0.6 * a, a, a]);
                }
            }
        }
        img.set(3, 2, [0.5, 0.5, 0.5, 0.5]);
        img.set(w - 1, h - 1, [1.0; 4]);
        img.set(w / 2, 1, [0.0, 0.0, 0.0, -0.0]);
        img
    }

    #[test]
    fn clear_tiles_hold_only_transparent_taps() {
        let img = sparse_image(150, 97);
        let clear = ClearTiles::new(&img);
        let mut skipped = 0;
        for y in 0..img.height as usize - 1 {
            for x in 0..img.width as usize - 1 {
                if clear.is_clear(x, y) {
                    skipped += 1;
                    for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                        assert_eq!(img.get(x as i64 + dx, y as i64 + dy)[3], 0.0, "tap ({}, {}) of a clear tile", x as i64 + dx, y as i64 + dy);
                    }
                }
            }
        }
        assert!(skipped > 100 * 70, "the ring's empty middle is skipped: {skipped}");
    }

    /// The changed rectangle holds every pixel a warp changed, and the skipped transparent
    /// samples leave the result as the reference path draws it.
    #[test]
    fn changed_rect_covers_the_changed_pixels() {
        let src = sparse_image(90, 70);
        let mats = [
            Mat3::translate(vec2(7.0, 3.0)),
            Mat3::translate(vec2(10.25, 4.5)),
            Mat3::translate(vec2(60.0, 50.0)) * Mat3::rotate_deg(33.0) * Mat3::scale(vec2(0.9, 1.2)) * Mat3::translate(vec2(-45.0, -35.0)),
            Mat3::translate(vec2(-30.0, -20.0)),
            Mat3::scale(vec2(0.3, 0.3)),
        ];
        for (mi, m) in mats.iter().enumerate() {
            for mode in [BlendMode::Normal, BlendMode::Add, BlendMode::Dissolve] {
                let opts = WarpOpts { mode, seed: 3, ..Default::default() };
                let before = test_image(120, 100, 4);
                let mut a = before.clone();
                let r = composite_warp_changed(&mut a, &src, m, &opts, None);
                let Some([x0, y0, x1, y1]) = r else { panic!("mat {mi}: the fast path tracks what it changed") };
                for (i, (p, q)) in a.data.iter().zip(&before.data).enumerate() {
                    let (x, y) = (i as u32 % a.width, i as u32 / a.width);
                    if p != q {
                        assert!(x >= x0 && x < x1 && y >= y0 && y < y1, "mat {mi} {mode:?}: ({x}, {y}) changed outside {r:?}");
                    }
                }
                let mut b = before.clone();
                composite_warp_reference(&mut b, &src, m, &opts);
                for (i, (p, q)) in a.data.iter().zip(&b.data).enumerate() {
                    for c in 0..4 {
                        assert!((p[c] - q[c]).abs() < 1e-4, "mat {mi} {mode:?} px {i}: {p:?} vs {q:?}");
                    }
                }
            }
        }
        let mut dst = Image::new(10, 10);
        assert_eq!(composite_warp_changed(&mut dst, &Image::new(4, 4), &Mat3::IDENTITY, &WarpOpts::default(), None), Some([u32::MAX, u32::MAX, 0, 0]));
        let stencil = WarpOpts { mode: BlendMode::StencilAlpha, ..Default::default() };
        assert_eq!(composite_warp_changed(&mut dst, &src, &Mat3::IDENTITY, &stencil, Some(255.0)), None, "stencils change the whole destination");
    }

    /// Quantising while blending equals blending, then quantising the changed rectangle of an
    /// already quantised destination (what an 8/16 bpc comp did after every layer), bit for bit.
    #[test]
    fn quantize_while_blending_matches_quantizing_after() {
        let src = sparse_image(90, 70);
        let mats = [Mat3::translate(vec2(7.0, 3.0)), Mat3::translate(vec2(60.0, 50.0)) * Mat3::rotate_deg(33.0) * Mat3::translate(vec2(-45.0, -35.0))];
        for m in &mats {
            for mode in [BlendMode::Normal, BlendMode::Screen, BlendMode::Dissolve] {
                for (levels, opacity) in [(255.0f32, 1.0), (32_768.0, 0.7)] {
                    let mut before = test_image(120, 100, 8);
                    for p in &mut before.data {
                        crate::quantize_px(p, levels, 1.0 / levels);
                    }
                    let opts = WarpOpts { mode, opacity, seed: 5, ..Default::default() };
                    let mut after = before.clone();
                    let Some([x0, y0, x1, y1]) = composite_warp_changed(&mut after, &src, m, &opts, None) else { panic!("fast path") };
                    for y in y0..y1 {
                        for x in x0..x1 {
                            let i = after.idx(x, y);
                            crate::quantize_px(&mut after.data[i], levels, 1.0 / levels);
                        }
                    }
                    let mut fused = before.clone();
                    let r = composite_warp_changed(&mut fused, &src, m, &opts, Some(levels));
                    assert_eq!(r, Some([x0, y0, x1, y1]));
                    for (i, (p, q)) in fused.data.iter().zip(&after.data).enumerate() {
                        assert_eq!(p.map(f32::to_bits), q.map(f32::to_bits), "{mode:?} {levels} px {i}");
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

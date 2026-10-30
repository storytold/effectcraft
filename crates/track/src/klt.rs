//! Sparse feature tracking (KLT), clean-room from the published methods:
//!
//! - **corners**: the Shi–Tomasi criterion (Shi & Tomasi, "Good Features to Track", CVPR 1994):
//!   the smaller eigenvalue of the gradient structure tensor summed over a small window, with
//!   grid-based non-maximum suppression and a minimum distance between features;
//! - **tracking**: iterative Lucas–Kanade translation estimation (Lucas & Kanade 1981) run coarse
//!   to fine over an image pyramid (the pyramidal scheme described in Bouguet's 2000 technical
//!   note "Pyramidal Implementation of the Lucas Kanade Feature Tracker"), with a
//!   **forward–backward** consistency check (Kalal et al., "Forward-Backward Error", ICPR 2010) to
//!   drop features that were lost or occluded.
//!
//! Frames are analysed in luma at a reduced resolution ([`GrayPyramid::from_image`] takes an
//! integer box-downsampling factor). Coordinates follow [`Plane`]: pixel `i` covers `[i, i + 1)`.

use effectcraft_raster::Image;
use rayon::prelude::*;

use crate::plane::Plane;

/// A luma pyramid with gradients per level.
#[derive(Clone, Debug, Default)]
pub struct GrayPyramid {
    pub levels: Vec<Plane>,
    pub grads: Vec<(Plane, Plane)>,
    /// Image pixels per level-0 pixel.
    pub factor: f64,
    /// Image pixel offset of the plane origin (layer pixel `p` is image pixel `p + offset`).
    pub offset: [f64; 2],
}

/// Luma of `img` averaged over `factor × factor` blocks (straight colour over premultiplied
/// alpha is ignored: tracking looks at the visible pixels).
pub fn luma_plane(img: &Image, factor: usize) -> Plane {
    let f = factor.max(1);
    let w = (img.width as usize).div_ceil(f).max(1);
    let h = (img.height as usize).div_ceil(f).max(1);
    let mut p = Plane::new(w, h, 1);
    if img.is_empty() {
        return p;
    }
    p.data.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for (x, o) in row.iter_mut().enumerate() {
            let mut s = 0.0f32;
            let mut n = 0.0f32;
            for yy in y * f..((y + 1) * f).min(img.height as usize) {
                for xx in x * f..((x + 1) * f).min(img.width as usize) {
                    let px = img.data[yy * img.width as usize + xx];
                    s += 0.2126 * px[0] + 0.7152 * px[1] + 0.0722 * px[2];
                    n += 1.0;
                }
            }
            *o = if n > 0.0 { s / n } else { 0.0 };
        }
    });
    p
}

impl GrayPyramid {
    /// A pyramid of `img` downsampled by `factor`, with `levels` extra half-resolution levels.
    pub fn from_image(img: &Image, offset: [f64; 2], factor: usize, levels: usize) -> GrayPyramid {
        let base = luma_plane(img, factor);
        Self::from_plane(base, offset, factor as f64, levels)
    }

    pub fn from_plane(base: Plane, offset: [f64; 2], factor: f64, levels: usize) -> GrayPyramid {
        let mut lv = vec![base];
        for _ in 0..levels {
            let Some(last) = lv.last() else { break };
            if last.w < 16 || last.h < 16 {
                break;
            }
            let d = last.downsample();
            lv.push(d);
        }
        let grads = lv.par_iter().map(Plane::gradients).collect();
        GrayPyramid { levels: lv, grads, factor, offset }
    }

    /// Layer pixels → level-0 plane coordinates.
    pub fn to_plane(&self, p: [f64; 2]) -> [f64; 2] {
        [(p[0] + self.offset[0]) / self.factor, (p[1] + self.offset[1]) / self.factor]
    }
    /// Level-0 plane coordinates → layer pixels.
    pub fn to_layer(&self, q: [f64; 2]) -> [f64; 2] {
        [q[0] * self.factor - self.offset[0], q[1] * self.factor - self.offset[1]]
    }
    pub fn width(&self) -> usize {
        self.levels.first().map(|p| p.w).unwrap_or(0)
    }
    pub fn height(&self) -> usize {
        self.levels.first().map(|p| p.h).unwrap_or(0)
    }
}

/// Feature detection options.
#[derive(Clone, Copy, Debug)]
pub struct CornerOpts {
    pub max_features: usize,
    /// Minimum distance between features (level-0 plane pixels).
    pub min_distance: f64,
    /// Keep corners whose score is at least this fraction of the strongest.
    pub quality: f64,
    /// Structure-tensor window radius.
    pub window: usize,
    /// Ignore this many pixels at the border.
    pub border: usize,
}

impl Default for CornerOpts {
    fn default() -> Self {
        CornerOpts { max_features: 300, min_distance: 8.0, quality: 0.01, window: 2, border: 6 }
    }
}

/// Shi–Tomasi corner score (smaller eigenvalue of the structure tensor) per pixel.
pub fn corner_scores(p: &Plane, gx: &Plane, gy: &Plane, r: usize) -> Vec<f32> {
    let (w, h) = (p.w, p.h);
    // Products, then box sums via summed-area tables.
    let n = w * h;
    let mut sxx = vec![0.0f64; (w + 1) * (h + 1)];
    let mut sxy = vec![0.0f64; (w + 1) * (h + 1)];
    let mut syy = vec![0.0f64; (w + 1) * (h + 1)];
    for y in 0..h {
        let (mut axx, mut axy, mut ayy) = (0.0, 0.0, 0.0);
        for x in 0..w {
            let i = y * w + x;
            let (dx, dy) = (gx.data[i] as f64, gy.data[i] as f64);
            axx += dx * dx;
            axy += dx * dy;
            ayy += dy * dy;
            let o = (y + 1) * (w + 1) + x + 1;
            let up = y * (w + 1) + x + 1;
            sxx[o] = sxx[up] + axx;
            sxy[o] = sxy[up] + axy;
            syy[o] = syy[up] + ayy;
        }
    }
    let mut out = vec![0.0f32; n];
    out.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let y0 = y.saturating_sub(r);
        let y1 = (y + r + 1).min(h);
        for (x, o) in row.iter_mut().enumerate() {
            let x0 = x.saturating_sub(r);
            let x1 = (x + r + 1).min(w);
            let bs = |s: &[f64]| s[y1 * (w + 1) + x1] - s[y0 * (w + 1) + x1] - s[y1 * (w + 1) + x0] + s[y0 * (w + 1) + x0];
            let (a, b, c) = (bs(&sxx), bs(&sxy), bs(&syy));
            let tr = 0.5 * (a + c);
            let d = (0.25 * (a - c) * (a - c) + b * b).sqrt();
            *o = (tr - d).max(0.0) as f32;
        }
    });
    out
}

/// Good features to track in `pyr` level 0, optionally restricted by `inside(plane point)`.
/// Returns level-0 plane coordinates (pixel centres), strongest first.
pub fn good_features(pyr: &GrayPyramid, opts: &CornerOpts, inside: Option<&(dyn Fn([f64; 2]) -> bool + Sync)>) -> Vec<[f64; 2]> {
    let Some(p) = pyr.levels.first() else { return vec![] };
    let (gx, gy) = &pyr.grads[0];
    let score = corner_scores(p, gx, gy, opts.window.max(1));
    let (w, h) = (p.w, p.h);
    let b = opts.border.min(w / 4).min(h / 4);
    // Local maxima (3 × 3) above the quality threshold.
    let mut cands: Vec<(f32, usize, usize)> = vec![];
    let mut best = 0.0f32;
    for y in b..h.saturating_sub(b) {
        for x in b..w.saturating_sub(b) {
            let v = score[y * w + x];
            if v <= 1e-9 {
                continue;
            }
            let mut is_max = true;
            'n: for dy in -1i64..=1 {
                for dx in -1i64..=1 {
                    if dx == 0 && dy == 0 {
                        continue;
                    }
                    let (xx, yy) = (x as i64 + dx, y as i64 + dy);
                    if xx < 0 || yy < 0 || xx >= w as i64 || yy >= h as i64 {
                        continue;
                    }
                    let o = score[yy as usize * w + xx as usize];
                    if o > v || (o == v && (dy < 0 || (dy == 0 && dx < 0))) {
                        is_max = false;
                        break 'n;
                    }
                }
            }
            if !is_max {
                continue;
            }
            let c = [x as f64 + 0.5, y as f64 + 0.5];
            if let Some(f) = inside
                && !f(c)
            {
                continue;
            }
            best = best.max(v);
            cands.push((v, x, y));
        }
    }
    let thr = best * opts.quality as f32;
    cands.retain(|c| c.0 >= thr);
    cands.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.2.cmp(&b.2)).then(a.1.cmp(&b.1)));
    // Minimum distance via a coarse occupancy grid.
    let md = opts.min_distance.max(1.0);
    let cell = md;
    let gw = (w as f64 / cell).ceil() as usize + 1;
    let gh = (h as f64 / cell).ceil() as usize + 1;
    let mut grid: Vec<Vec<[f64; 2]>> = vec![vec![]; gw * gh];
    let mut out = vec![];
    for (_, x, y) in cands {
        let c = [x as f64 + 0.5, y as f64 + 0.5];
        let (cx, cy) = ((c[0] / cell) as usize, (c[1] / cell) as usize);
        let mut ok = true;
        'g: for yy in cy.saturating_sub(1)..=(cy + 1).min(gh - 1) {
            for xx in cx.saturating_sub(1)..=(cx + 1).min(gw - 1) {
                for q in &grid[yy * gw + xx] {
                    if (q[0] - c[0]).powi(2) + (q[1] - c[1]).powi(2) < md * md {
                        ok = false;
                        break 'g;
                    }
                }
            }
        }
        if ok {
            grid[cy * gw + cx].push(c);
            out.push(c);
            if out.len() >= opts.max_features {
                break;
            }
        }
    }
    out
}

/// Lucas–Kanade options.
#[derive(Clone, Copy, Debug)]
pub struct LkOpts {
    /// Window radius (the window is `2r + 1` square).
    pub radius: i32,
    pub iterations: usize,
    /// Stop when the update is below this (pixels).
    pub epsilon: f64,
    /// Reject windows whose structure tensor's smaller eigenvalue (per pixel) is below this.
    pub min_eigen: f64,
    /// Forward–backward error limit (level-0 pixels); 0 = no check.
    pub fb_max: f64,
}

impl Default for LkOpts {
    fn default() -> Self {
        LkOpts { radius: 5, iterations: 20, epsilon: 0.01, min_eigen: 1e-5, fb_max: 1.0 }
    }
}

/// Bilinear taps shared by every pixel of a `(2r+1)²` window centred at `c`: the fractional
/// offset is the same for the whole window, and the column/row indices are clamped like
/// [`Plane::at`] (edges repeated). Row indices are pre-multiplied by the plane width.
#[derive(Default)]
struct Taps {
    xs: Vec<[usize; 2]>,
    ys: Vec<[usize; 2]>,
    t: [f32; 2],
}

impl Taps {
    fn set(&mut self, p: &Plane, c: [f64; 2], r: i32) {
        debug_assert_eq!(p.ch, 1);
        let (fx, fy) = (c[0] - 0.5, c[1] - 0.5);
        let (x0, y0) = (fx.floor(), fy.floor());
        self.t = [(fx - x0) as f32, (fy - y0) as f32];
        const FAR: f64 = (1u64 << 40) as f64;
        let (xi, yi) = (x0.clamp(-FAR, FAR) as isize, y0.clamp(-FAR, FAR) as isize);
        let (wm, hm) = (p.w as isize - 1, p.h as isize - 1);
        let r = r as isize;
        self.xs.clear();
        self.ys.clear();
        self.xs.extend((-r..=r).map(|d| [(xi + d).clamp(0, wm) as usize, (xi + d + 1).clamp(0, wm) as usize]));
        self.ys.extend((-r..=r).map(|d| [(yi + d).clamp(0, hm) as usize * p.w, (yi + d + 1).clamp(0, hm) as usize * p.w]));
    }
}

/// [`Plane::sample`]'s bilinear form on pre-resolved taps.
#[inline(always)]
fn bilerp(d: &[f32], x: [usize; 2], y: [usize; 2], t: [f32; 2]) -> f32 {
    let (a, b, c, e) = (d[y[0] + x[0]], d[y[0] + x[1]], d[y[1] + x[0]], d[y[1] + x[1]]);
    let top = a + (b - a) * t[0];
    let bot = c + (e - c) * t[0];
    top + (bot - top) * t[1]
}

fn lk_point(a: &GrayPyramid, b: &GrayPyramid, p: [f64; 2], guess: [f64; 2], o: &LkOpts) -> Option<[f64; 2]> {
    let top = a.levels.len().min(b.levels.len()).checked_sub(1)?;
    let r = o.radius;
    let n = ((2 * r + 1) * (2 * r + 1)) as f64;
    let s0 = (1u32 << top) as f64;
    let mut g = [guess[0] / s0, guess[1] / s0];
    let mut win = vec![0.0f32; ((2 * r + 1) * (2 * r + 1)) as usize];
    let mut wgx = win.clone();
    let mut wgy = win.clone();
    let mut taps = Taps::default();
    for l in (0..=top).rev() {
        let s = (1u32 << l) as f64;
        let (ia, ib) = (&a.levels[l], &b.levels[l]);
        let (gx, gy) = &a.grads[l];
        let c = [p[0] / s, p[1] / s];
        if c[0] < -1.0 || c[1] < -1.0 || c[0] > ia.w as f64 + 1.0 || c[1] > ia.h as f64 + 1.0 {
            return None;
        }
        let (mut gxx, mut gxy, mut gyy) = (0.0f64, 0.0f64, 0.0f64);
        let mut k = 0;
        taps.set(ia, c, r);
        for &y in &taps.ys {
            for &x in &taps.xs {
                win[k] = bilerp(&ia.data, x, y, taps.t);
                wgx[k] = bilerp(&gx.data, x, y, taps.t);
                wgy[k] = bilerp(&gy.data, x, y, taps.t);
                let (ix, iy) = (wgx[k] as f64, wgy[k] as f64);
                gxx += ix * ix;
                gxy += ix * iy;
                gyy += iy * iy;
                k += 1;
            }
        }
        let det = gxx * gyy - gxy * gxy;
        let tr = 0.5 * (gxx + gyy);
        let mineig = (tr - (0.25 * (gxx - gyy).powi(2) + gxy * gxy).sqrt()) / n;
        if mineig < o.min_eigen || det.abs() < 1e-18 {
            return None;
        }
        let mut v = [0.0f64; 2];
        for _ in 0..o.iterations {
            let (mut bx, mut by) = (0.0f64, 0.0f64);
            let mut k = 0;
            taps.set(ib, [c[0] + g[0] + v[0], c[1] + g[1] + v[1]], r);
            for &y in &taps.ys {
                for &x in &taps.xs {
                    let diff = (win[k] - bilerp(&ib.data, x, y, taps.t)) as f64;
                    bx += diff * wgx[k] as f64;
                    by += diff * wgy[k] as f64;
                    k += 1;
                }
            }
            let nx = (gyy * bx - gxy * by) / det;
            let ny = (gxx * by - gxy * bx) / det;
            v[0] += nx;
            v[1] += ny;
            if !(v[0].is_finite() && v[1].is_finite()) {
                return None;
            }
            if nx * nx + ny * ny < o.epsilon * o.epsilon {
                break;
            }
        }
        if l > 0 {
            g = [2.0 * (g[0] + v[0]), 2.0 * (g[1] + v[1])];
        } else {
            g = [g[0] + v[0], g[1] + v[1]];
        }
    }
    let q = [p[0] + g[0], p[1] + g[1]];
    let l0 = &b.levels[0];
    if q[0] < 0.0 || q[1] < 0.0 || q[0] > l0.w as f64 || q[1] > l0.h as f64 {
        return None;
    }
    Some(q)
}

/// Track level-0 points from pyramid `a` to pyramid `b` (with optional per-point initial
/// displacement guesses). `None` for lost points (untrackable window, left the frame, or failed
/// the forward–backward check).
pub fn track(a: &GrayPyramid, b: &GrayPyramid, pts: &[[f64; 2]], guesses: Option<&[[f64; 2]]>, o: &LkOpts) -> Vec<Option<[f64; 2]>> {
    pts.par_iter()
        .enumerate()
        .map(|(i, p)| {
            let g = guesses.and_then(|g| g.get(i).copied()).unwrap_or([0.0; 2]);
            let q = lk_point(a, b, *p, g, o)?;
            if o.fb_max > 0.0 {
                let back = lk_point(b, a, q, [p[0] - q[0], p[1] - q[1]], o)?;
                if (back[0] - p[0]).powi(2) + (back[1] - p[1]).powi(2) > o.fb_max * o.fb_max {
                    return None;
                }
            }
            Some(q)
        })
        .collect()
}

/// Integer downsampling factor so the longer side is at most `max_side` pixels.
pub fn analysis_factor(w: u32, h: u32, max_side: u32) -> usize {
    let long = w.max(h).max(1);
    long.div_ceil(max_side.max(16)).max(1) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Value noise: smooth random blobs at two scales.
    fn tex(x: f64, y: f64) -> f32 {
        fn h(i: i64, j: i64) -> f64 {
            let mut v = (i.wrapping_mul(73856093) ^ j.wrapping_mul(19349663)) as u64;
            v ^= v >> 13;
            v = v.wrapping_mul(0x5bd1e995);
            v ^= v >> 15;
            (v % 1000) as f64 / 1000.0
        }
        fn noise(x: f64, y: f64, cell: f64) -> f64 {
            let (fx, fy) = (x / cell, y / cell);
            let (i, j) = (fx.floor(), fy.floor());
            let (tx, ty) = (fx - i, fy - j);
            let (sx, sy) = (tx * tx * (3.0 - 2.0 * tx), ty * ty * (3.0 - 2.0 * ty));
            let (i, j) = (i as i64, j as i64);
            let a = h(i, j) + (h(i + 1, j) - h(i, j)) * sx;
            let b = h(i, j + 1) + (h(i + 1, j + 1) - h(i, j + 1)) * sx;
            a + (b - a) * sy
        }
        (0.2 + 0.4 * noise(x + 1000.0, y + 1000.0, 9.0) + 0.3 * noise(x + 1000.0, y + 1000.0, 4.0)) as f32
    }

    fn img(dx: f64, dy: f64) -> Image {
        let mut im = Image::new(160, 120);
        for y in 0..120 {
            for x in 0..160 {
                let v = tex(x as f64 + 0.5 - dx, y as f64 + 0.5 - dy);
                im.set(x, y, [v, v, v, 1.0]);
            }
        }
        im
    }

    /// The per-pixel `Plane::sample` implementation `lk_point` replaced, kept as the oracle.
    fn lk_point_ref(a: &GrayPyramid, b: &GrayPyramid, p: [f64; 2], guess: [f64; 2], o: &LkOpts) -> Option<[f64; 2]> {
        let top = a.levels.len().min(b.levels.len()).checked_sub(1)?;
        let r = o.radius;
        let n = ((2 * r + 1) * (2 * r + 1)) as f64;
        let s0 = (1u32 << top) as f64;
        let mut g = [guess[0] / s0, guess[1] / s0];
        let mut win = vec![0.0f32; ((2 * r + 1) * (2 * r + 1)) as usize];
        let mut wgx = win.clone();
        let mut wgy = win.clone();
        let mut v1 = [0.0f32];
        for l in (0..=top).rev() {
            let s = (1u32 << l) as f64;
            let (ia, ib) = (&a.levels[l], &b.levels[l]);
            let (gx, gy) = &a.grads[l];
            let c = [p[0] / s, p[1] / s];
            if c[0] < -1.0 || c[1] < -1.0 || c[0] > ia.w as f64 + 1.0 || c[1] > ia.h as f64 + 1.0 {
                return None;
            }
            let (mut gxx, mut gxy, mut gyy) = (0.0f64, 0.0f64, 0.0f64);
            let mut k = 0;
            for dy in -r..=r {
                for dx in -r..=r {
                    let (x, y) = (c[0] + dx as f64, c[1] + dy as f64);
                    ia.sample(x, y, &mut v1);
                    win[k] = v1[0];
                    gx.sample(x, y, &mut v1);
                    wgx[k] = v1[0];
                    gy.sample(x, y, &mut v1);
                    wgy[k] = v1[0];
                    let (ix, iy) = (wgx[k] as f64, wgy[k] as f64);
                    gxx += ix * ix;
                    gxy += ix * iy;
                    gyy += iy * iy;
                    k += 1;
                }
            }
            let det = gxx * gyy - gxy * gxy;
            let tr = 0.5 * (gxx + gyy);
            let mineig = (tr - (0.25 * (gxx - gyy).powi(2) + gxy * gxy).sqrt()) / n;
            if mineig < o.min_eigen || det.abs() < 1e-18 {
                return None;
            }
            let mut v = [0.0f64; 2];
            for _ in 0..o.iterations {
                let (mut bx, mut by) = (0.0f64, 0.0f64);
                let mut k = 0;
                for dy in -r..=r {
                    for dx in -r..=r {
                        ib.sample(c[0] + g[0] + v[0] + dx as f64, c[1] + g[1] + v[1] + dy as f64, &mut v1);
                        let diff = (win[k] - v1[0]) as f64;
                        bx += diff * wgx[k] as f64;
                        by += diff * wgy[k] as f64;
                        k += 1;
                    }
                }
                let nx = (gyy * bx - gxy * by) / det;
                let ny = (gxx * by - gxy * bx) / det;
                v[0] += nx;
                v[1] += ny;
                if !(v[0].is_finite() && v[1].is_finite()) {
                    return None;
                }
                if nx * nx + ny * ny < o.epsilon * o.epsilon {
                    break;
                }
            }
            if l > 0 {
                g = [2.0 * (g[0] + v[0]), 2.0 * (g[1] + v[1])];
            } else {
                g = [g[0] + v[0], g[1] + v[1]];
            }
        }
        let q = [p[0] + g[0], p[1] + g[1]];
        let l0 = &b.levels[0];
        if q[0] < 0.0 || q[1] < 0.0 || q[0] > l0.w as f64 || q[1] > l0.h as f64 {
            return None;
        }
        Some(q)
    }

    #[test]
    fn klt_recovers_a_shift() {
        let a = GrayPyramid::from_image(&img(0.0, 0.0), [0.0; 2], 1, 3);
        let b = GrayPyramid::from_image(&img(6.3, -4.2), [0.0; 2], 1, 3);
        let pts = good_features(&a, &CornerOpts { max_features: 60, ..Default::default() }, None);
        assert!(pts.len() >= 20, "{}", pts.len());
        let q = track(&a, &b, &pts, None, &LkOpts::default());
        let good: Vec<_> = pts.iter().zip(&q).filter_map(|(p, q)| q.map(|q| [q[0] - p[0], q[1] - p[1]])).collect();
        assert!(good.len() * 2 > pts.len(), "{} of {}", good.len(), pts.len());
        let mut errs: Vec<f64> = good.iter().map(|d| ((d[0] - 6.3).powi(2) + (d[1] + 4.2).powi(2)).sqrt()).collect();
        errs.sort_by(f64::total_cmp);
        assert!(errs[errs.len() / 2] < 0.1, "median error {}", errs[errs.len() / 2]);
    }

    #[test]
    fn window_taps_match_per_pixel_sampling_bit_for_bit() {
        let a = GrayPyramid::from_image(&img(0.0, 0.0), [0.0; 2], 1, 3);
        let b = GrayPyramid::from_image(&img(6.3, -4.2), [0.0; 2], 1, 3);
        let mut pts = vec![];
        for y in (-3..125).step_by(3) {
            for x in (-3..165).step_by(3) {
                pts.push([x as f64 + 0.37 * ((x * 7 + y) % 3) as f64, y as f64 + 0.5 * ((x + y * 5) % 2) as f64]);
            }
        }
        pts.extend([[0.0, 0.0], [160.0, 120.0], [80.0, 60.0], [0.5, 0.5], [159.5, 119.5]]);
        let (mut tracked, mut lost) = (0, 0);
        for (i, p) in pts.iter().enumerate() {
            for o in [LkOpts::default(), LkOpts { radius: 3, iterations: 5, ..Default::default() }] {
                let g = [(i % 5) as f64 - 2.0, (i % 3) as f64 - 1.0];
                let (new, old) = (lk_point(&a, &b, *p, g, &o), lk_point_ref(&a, &b, *p, g, &o));
                match (new, old) {
                    (Some(n), Some(o)) => {
                        assert_eq!([n[0].to_bits(), n[1].to_bits()], [o[0].to_bits(), o[1].to_bits()], "{p:?}");
                        tracked += 1;
                    }
                    (None, None) => lost += 1,
                    (n, o) => panic!("{p:?}: {n:?} vs {o:?}"),
                }
            }
        }
        assert!(tracked > 500 && lost > 0, "{tracked} tracked, {lost} lost");
    }

    #[test]
    fn features_respect_the_region_and_spacing() {
        let a = GrayPyramid::from_image(&img(0.0, 0.0), [0.0; 2], 1, 2);
        let inside = |p: [f64; 2]| p[0] < 80.0;
        let pts = good_features(&a, &CornerOpts { min_distance: 10.0, ..Default::default() }, Some(&inside));
        assert!(!pts.is_empty());
        assert!(pts.iter().all(|p| p[0] < 80.0));
        for (i, p) in pts.iter().enumerate() {
            for q in &pts[i + 1..] {
                assert!((p[0] - q[0]).hypot(p[1] - q[1]) >= 10.0 - 1e-9);
            }
        }
        assert_eq!(analysis_factor(1920, 1080, 640), 3);
    }
}

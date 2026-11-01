//! Optical flow by block matching, and motion-compensated frame interpolation (the Pixel Motion
//! mode of frame blending).
//!
//! The method is the textbook hierarchical block matcher used in video coding and frame-rate
//! conversion: build a luma pyramid of both frames; at the coarsest level, find for each block of
//! frame A the integer displacement into frame B with the smallest sum of absolute differences
//! (SAD) in a search window; at each finer level, start from the doubled coarse vectors (and the
//! neighbours' vectors) and refine in a small window; finish with a sub-pixel parabola fit of the
//! SAD around the best vector and a 3×3 vector median to remove outliers.
//!
//! Interpolation at a fraction `w` between the frames warps both towards the in-between time —
//! A forward by `w·F`, B back by `(1 − w)·F` — and cross-fades them, so moving detail stays sharp
//! instead of double-exposing as with a plain frame mix.

use rayon::prelude::*;

use crate::Image;

/// A block motion field: one vector per `block × block` tile of frame A (in pixels, A → B).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Flow {
    pub width: u32,
    pub height: u32,
    pub block: u32,
    /// Tiles across / down.
    pub cols: usize,
    pub rows: usize,
    pub v: Vec<[f32; 2]>,
}

impl Flow {
    /// The motion at pixel position (x, y), bilinearly interpolated between tile centres.
    pub fn at(&self, x: f64, y: f64) -> [f32; 2] {
        if self.v.is_empty() {
            return [0.0; 2];
        }
        let b = self.block as f64;
        let fx = (x / b - 0.5).clamp(0.0, (self.cols - 1) as f64);
        let fy = (y / b - 0.5).clamp(0.0, (self.rows - 1) as f64);
        let (x0, y0) = (fx.floor() as usize, fy.floor() as usize);
        let (x1, y1) = ((x0 + 1).min(self.cols - 1), (y0 + 1).min(self.rows - 1));
        let (tx, ty) = ((fx - x0 as f64) as f32, (fy - y0 as f64) as f32);
        let g = |c: usize, r: usize| self.v[r * self.cols + c];
        let mut o = [0.0; 2];
        for i in 0..2 {
            let top = g(x0, y0)[i] + (g(x1, y0)[i] - g(x0, y0)[i]) * tx;
            let bot = g(x0, y1)[i] + (g(x1, y1)[i] - g(x0, y1)[i]) * tx;
            o[i] = top + (bot - top) * ty;
        }
        o
    }
}

/// The `Plane::luma` of `a` and `b` packed into one image (`a` in the red channel, `b` in the
/// green; the rest zero). A device block matcher takes this as its input rather than computing
/// the luma itself: the weighted sum is not reproducible on a device, which may contract
/// `a·b + c` into a fused multiply-add, and the search's near-tie choices would then differ.
pub fn luma_pair(a: &Image, b: &Image) -> Image {
    let luma = |p: &[f32; 4]| 0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2] + 0.25 * p[3];
    let mut out = Image::new(a.width, a.height);
    out.data.par_iter_mut().enumerate().for_each(|(i, q)| {
        let la = a.data.get(i).map(luma).unwrap_or(0.0);
        let lb = b.data.get(i).map(luma).unwrap_or(0.0);
        *q = [la, lb, 0.0, 0.0];
    });
    out
}

/// A single-channel plane.
struct Plane {
    w: usize,
    h: usize,
    d: Vec<f32>,
}

impl Plane {
    fn luma(img: &Image) -> Plane {
        let d = img.data.par_iter().map(|p| 0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2] + 0.25 * p[3]).collect();
        Plane { w: img.width as usize, h: img.height as usize, d }
    }
    fn half(&self) -> Plane {
        let (w, h) = ((self.w / 2).max(1), (self.h / 2).max(1));
        let d = (0..w * h)
            .into_par_iter()
            .map(|i| {
                let (x, y) = (i % w * 2, i / w * 2);
                let g = |dx: usize, dy: usize| self.d[(y + dy).min(self.h - 1) * self.w + (x + dx).min(self.w - 1)];
                (g(0, 0) + g(1, 0) + g(0, 1) + g(1, 1)) * 0.25
            })
            .collect();
        Plane { w, h, d }
    }
    #[inline]
    fn get(&self, x: i64, y: i64) -> f32 {
        self.d[y.clamp(0, self.h as i64 - 1) as usize * self.w + x.clamp(0, self.w as i64 - 1) as usize]
    }
}

/// SAD of the `bs` block at (bx, by) in `a` against `b` displaced by (dx, dy).
fn sad(a: &Plane, b: &Plane, bx: i64, by: i64, bs: i64, dx: i64, dy: i64) -> f32 {
    let mut s = 0.0;
    for y in by..by + bs {
        for x in bx..bx + bs {
            s += (a.get(x, y) - b.get(x + dx, y + dy)).abs();
        }
    }
    s
}

/// Block-matching flow from `a` to `b` (same size). `block` is the tile size at full resolution;
/// `radius` the search range at the coarsest level (motion up to about `radius · 2^levels`).
pub fn block_flow(a: &Image, b: &Image, block: u32, radius: i32) -> Flow {
    block_flow_window(a, b, block, block, radius)
}

/// [`block_flow`] with separate vector spacing and matching window: one vector per
/// `tile × tile` tile, each matched with the `window × window` block centred on its tile
/// (Timewarp's Vector Detail and Block Size).
pub fn block_flow_window(a: &Image, b: &Image, tile: u32, window: u32, radius: i32) -> Flow {
    let bs = tile.max(2) as usize;
    let win = window.max(4) as i64;
    // Window origin relative to the tile origin.
    let wo = bs as i64 / 2 - win / 2;
    let (w, h) = (a.width as usize, a.height as usize);
    let cols = w.div_ceil(bs).max(1);
    let rows = h.div_ceil(bs).max(1);
    let mut flow = Flow { width: a.width, height: a.height, block: bs as u32, cols, rows, v: vec![[0.0; 2]; cols * rows] };
    if a.width != b.width || a.height != b.height || w < bs || h < bs || (w as i64) < win || (h as i64) < win {
        return flow;
    }
    // Pyramids (level 0 = full resolution); stop while blocks still fit.
    let mut pa = vec![Plane::luma(a)];
    let mut pb = vec![Plane::luma(b)];
    while pa.len() < 4 && pa.last().is_some_and(|p| p.w / 2 >= (bs as i64).max(win) as usize * 2 && p.h / 2 >= (bs as i64).max(win) as usize * 2) {
        let (na, nb) = (pa.last().map(Plane::half), pb.last().map(Plane::half));
        pa.extend(na);
        pb.extend(nb);
    }
    let top = pa.len() - 1;
    // Integer vectors at the current level, per full-resolution tile.
    let mut cur: Vec<[i64; 2]> = vec![[0, 0]; cols * rows];
    for level in (0..=top).rev() {
        let (la, lb) = (&pa[level], &pb[level]);
        let k = 1i64 << level;
        // The block covers the tile's footprint at this level (at least 4 pixels).
        let lbs = (win / k).max(4);
        let r = if level == top { radius as i64 } else { 1 };
        let prev = cur.clone();
        cur = (0..cols * rows)
            .into_par_iter()
            .map(|i| {
                let (c, rr) = ((i % cols) as i64, (i / cols) as i64);
                let bx = (c * bs as i64 + wo) / k;
                let by = (rr * bs as i64 + wo) / k;
                // Candidates: this tile's and its neighbours' vectors from the coarser level.
                let mut seeds: Vec<[i64; 2]> = vec![[0, 0]];
                if level != top {
                    for (dc, dr) in [(0, 0), (-1, 0), (1, 0), (0, -1), (0, 1)] {
                        let (nc, nr) = (c + dc, rr + dr);
                        if nc >= 0 && nr >= 0 && (nc as usize) < cols && (nr as usize) < rows {
                            let v = prev[nr as usize * cols + nc as usize];
                            seeds.push([v[0] * 2, v[1] * 2]);
                        }
                    }
                }
                seeds.sort_unstable();
                seeds.dedup();
                let mut best = ([0i64, 0i64], f32::INFINITY);
                for s in seeds {
                    for dy in -r..=r {
                        for dx in -r..=r {
                            let d = [s[0] + dx, s[1] + dy];
                            // A tiny bias towards short vectors breaks ties in flat areas.
                            let e = sad(la, lb, bx, by, lbs, d[0], d[1]) + 1e-4 * (d[0].abs() + d[1].abs()) as f32;
                            if e < best.1 {
                                best = (d, e);
                            }
                        }
                    }
                }
                best.0
            })
            .collect();
    }
    // Sub-pixel refinement (parabola through the SAD of the neighbours) at full resolution.
    let (la, lb) = (&pa[0], &pb[0]);
    let bsi = bs as i64;
    let wsi = win;
    let fine: Vec<[f32; 2]> = (0..cols * rows)
        .into_par_iter()
        .map(|i| {
            let (bx, by) = ((i % cols) as i64 * bsi + wo, (i / cols) as i64 * bsi + wo);
            let d = cur[i];
            let e0 = sad(la, lb, bx, by, wsi, d[0], d[1]);
            let fit = |em: f32, ep: f32| {
                let den = em - 2.0 * e0 + ep;
                if den > 1e-6 { (0.5 * (em - ep) / den).clamp(-0.5, 0.5) } else { 0.0 }
            };
            let sx = fit(sad(la, lb, bx, by, wsi, d[0] - 1, d[1]), sad(la, lb, bx, by, wsi, d[0] + 1, d[1]));
            let sy = fit(sad(la, lb, bx, by, wsi, d[0], d[1] - 1), sad(la, lb, bx, by, wsi, d[0], d[1] + 1));
            [d[0] as f32 + sx, d[1] as f32 + sy]
        })
        .collect();
    // 3×3 vector median (component-wise) against outliers.
    flow.v = (0..cols * rows)
        .into_par_iter()
        .map(|i| {
            let (c, r) = ((i % cols) as i64, (i / cols) as i64);
            let mut xs = Vec::with_capacity(9);
            let mut ys = Vec::with_capacity(9);
            for dr in -1..=1 {
                for dc in -1..=1 {
                    let (nc, nr) = (c + dc, r + dr);
                    if nc >= 0 && nr >= 0 && (nc as usize) < cols && (nr as usize) < rows {
                        let v = fine[nr as usize * cols + nc as usize];
                        xs.push(v[0]);
                        ys.push(v[1]);
                    }
                }
            }
            xs.sort_by(f32::total_cmp);
            ys.sort_by(f32::total_cmp);
            [xs[xs.len() / 2], ys[ys.len() / 2]]
        })
        .collect();
    flow
}

/// Cross-fade of two frames: `a·(1 − w) + b·w` (Frame Mix).
pub fn mix(a: &Image, b: &Image, w: f32) -> Image {
    if a.width != b.width || a.height != b.height {
        return if w < 0.5 { a.clone() } else { b.clone() };
    }
    let mut out = a.clone();
    out.data.par_iter_mut().zip(b.data.par_iter()).for_each(|(p, q)| {
        for c in 0..4 {
            p[c] += (q[c] - p[c]) * w;
        }
    });
    out
}

/// Motion-compensated frame at fraction `w` (0 = `a`, 1 = `b`) between two frames (Pixel Motion).
pub fn interpolate(a: &Image, b: &Image, w: f32) -> Image {
    if a.width != b.width || a.height != b.height {
        return mix(a, b, w);
    }
    let flow = block_flow(a, b, 8, 4);
    let mut out = Image::new(a.width, a.height);
    let wd = w as f64;
    out.rows_mut().for_each(|(y, row)| {
        let cy = y as f64 + 0.5;
        for (x, px) in row.iter_mut().enumerate() {
            let cx = x as f64 + 0.5;
            let f = flow.at(cx, cy);
            let (fx, fy) = (f[0] as f64, f[1] as f64);
            let pa = a.sample_bilinear_clamped(cx - wd * fx, cy - wd * fy);
            let pb = b.sample_bilinear_clamped(cx + (1.0 - wd) * fx, cy + (1.0 - wd) * fy);
            for c in 0..4 {
                px[c] = pa[c] + (pb[c] - pa[c]) * w;
            }
        }
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A textured image shifted right by `dx` pixels.
    fn texture(w: u32, h: u32, dx: f64) -> Image {
        let mut img = Image::new(w, h);
        img.rows_mut().for_each(|(y, row)| {
            for (x, p) in row.iter_mut().enumerate() {
                let (u, v) = (x as f64 - dx, y as f64);
                let s = 0.5 + 0.2 * (u * 0.21 + v * 0.07).sin() + 0.15 * (u * 0.05 - v * 0.19).cos() + 0.1 * (u * 0.013 * v * 0.011).sin();
                let s = s as f32;
                *p = [s, s * 0.8, 1.0 - s, 1.0];
            }
        });
        img
    }

    fn mae(a: &Image, b: &Image, margin: u32) -> f32 {
        let mut s = 0.0;
        let mut n = 0;
        for y in margin..a.height - margin {
            for x in margin..a.width - margin {
                let (p, q) = (a.get(x as i64, y as i64), b.get(x as i64, y as i64));
                s += (0..3).map(|c| (p[c] - q[c]).abs()).sum::<f32>();
                n += 3;
            }
        }
        s / n as f32
    }

    #[test]
    fn flow_finds_translation() {
        let a = texture(128, 96, 0.0);
        let b = texture(128, 96, 6.0);
        let f = block_flow(&a, &b, 8, 4);
        let m = f.at(64.0, 48.0);
        assert!((m[0] - 6.0).abs() < 0.3 && m[1].abs() < 0.3, "{m:?}");
    }

    #[test]
    fn pixel_motion_beats_frame_mix_on_translation() {
        let a = texture(128, 96, 0.0);
        let b = texture(128, 96, 6.0);
        let truth = texture(128, 96, 3.0);
        let pm = mae(&interpolate(&a, &b, 0.5), &truth, 12);
        let fm = mae(&mix(&a, &b, 0.5), &truth, 12);
        assert!(pm < fm * 0.25, "pixel motion {pm} vs frame mix {fm}");
    }
}

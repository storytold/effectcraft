//! Content-Aware Fill: filling the transparent area of a layer across a range of frames.
//!
//! Written from the published methods:
//!
//! - **Membrane (harmonic) fill**: the hole takes the smoothest values that match its boundary
//!   (Laplace's equation with Dirichlet boundary values), solved coarse to fine with
//!   known-weighted pyramids and Jacobi relaxation. Edge Blend uses it directly; it also
//!   initialises the other methods and completes flow fields.
//! - **PatchMatch** (Barnes, Shechtman, Finkelstein, Goldman 2009) nearest-neighbour fields
//!   inside the **space-time completion** EM loop of Wexler, Shechtman and Irani (2007), applied
//!   per frame: every patch overlapping the hole is matched to a fully known patch (random
//!   initialisation, propagation, random search); hole pixels become the weighted vote of the
//!   overlapping matches; repeated over a multi-scale pyramid, coarse to fine.
//! - **Flow-guided propagation** (the propagation stage of flow-guided video inpainting, e.g. Xu
//!   et al. 2019 / Huang et al. 2016): optical flow between neighbouring frames is computed on
//!   membrane-filled frames, *completed* inside the hole from the surrounding motion, and each
//!   missing pixel follows the flow forwards and backwards until it lands on a frame where that
//!   point of the scene is visible; the two candidates are blended by temporal distance.
//!   **Lighting Correction** adds the membrane interpolation of the mismatch along the hole's
//!   border (Poisson-style seamless blending), scaled by the chosen strength.
//!
//! Methods (After Effects' Content-Aware Fill panel):
//! - **Object**: propagation, then PatchMatch synthesis for what no frame shows; each frame's
//!   synthesis starts from the previous frame's result warped along the flow, so the fill stays
//!   temporally stable.
//! - **Surface**: propagation, then one synthesised fill carried through the range by the flow
//!   (flat, rigid surfaces).
//! - **Edge Blend**: the membrane fill of each frame from its own border (fast; textureless
//!   surfaces).
//!
//! Reference frames are complete images of given frames (for example a clean plate): they count as
//! frames where every pixel is visible, so propagation pulls their content into the hole.

use rayon::prelude::*;

use crate::Image;
use crate::flow::{Flow, block_flow};

/// Fill method.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FillMethod {
    #[default]
    Object,
    Surface,
    EdgeBlend,
}

impl FillMethod {
    pub fn from_name(s: &str) -> Option<FillMethod> {
        match s.to_ascii_lowercase().replace([' ', '_', '-'], "").as_str() {
            "object" => Some(FillMethod::Object),
            "surface" => Some(FillMethod::Surface),
            "edgeblend" | "edge" => Some(FillMethod::EdgeBlend),
            _ => None,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            FillMethod::Object => "object",
            FillMethod::Surface => "surface",
            FillMethod::EdgeBlend => "edgeBlend",
        }
    }
}

/// Fill options.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FillOpts {
    pub method: FillMethod,
    /// Lighting Correction strength 0 (None) … 1 (Strong).
    pub lighting: f32,
    /// Patch size (odd).
    pub patch: usize,
    /// Seed of the PatchMatch random search.
    pub seed: u32,
}

impl Default for FillOpts {
    fn default() -> Self {
        FillOpts { method: FillMethod::Object, lighting: 0.0, patch: 7, seed: 1 }
    }
}

// ------------------------------------------------------------------ helpers

#[inline]
fn rnd(seed: u32, a: u32, b: u32) -> u32 {
    let mut h = seed.wrapping_mul(0x9E37_79B9) ^ a.wrapping_mul(0x85EB_CA6B) ^ b.wrapping_mul(0xC2B2_AE35);
    h ^= h >> 16;
    h = h.wrapping_mul(0x7FEB_352D);
    h ^= h >> 15;
    h = h.wrapping_mul(0x846C_A68B);
    h ^ (h >> 16)
}

/// Grow a hole mask by `r` pixels (square structuring element, separable).
pub fn dilate(hole: &[bool], w: usize, h: usize, r: usize) -> Vec<bool> {
    if r == 0 {
        return hole.to_vec();
    }
    let mut tmp = vec![false; w * h];
    for y in 0..h {
        for x in 0..w {
            let lo = x.saturating_sub(r);
            let hi = (x + r).min(w - 1);
            tmp[y * w + x] = (lo..=hi).any(|k| hole[y * w + k]);
        }
    }
    let mut out = vec![false; w * h];
    for y in 0..h {
        for x in 0..w {
            let lo = y.saturating_sub(r);
            let hi = (y + r).min(h - 1);
            out[y * w + x] = (lo..=hi).any(|k| tmp[k * w + x]);
        }
    }
    out
}

/// Peak signal-to-noise ratio (dB) of the RGB of `a` against `b` over `region` (all pixels when
/// `None`), for values in 0…1.
pub fn psnr(a: &Image, b: &Image, region: Option<&[bool]>) -> f64 {
    let (mut se, mut n) = (0.0f64, 0.0f64);
    for i in 0..a.data.len().min(b.data.len()) {
        if region.is_some_and(|r| !r[i]) {
            continue;
        }
        for c in 0..3 {
            let d = (a.data[i][c] - b.data[i][c]) as f64;
            se += d * d;
        }
        n += 3.0;
    }
    if se <= 0.0 || n == 0.0 {
        return 99.0;
    }
    10.0 * (1.0 / (se / n)).log10()
}

// ------------------------------------------------------------------ membrane

/// Harmonic fill of `data` (`w × h`, 4 channels) where `known` is false.
fn membrane_core(data: &mut [[f32; 4]], known: &[bool], w: usize, h: usize) {
    if !known.iter().any(|k| !*k) {
        return;
    }
    if !known.iter().any(|k| *k) {
        return;
    }
    // Coarse level: known-weighted 2×2 averages.
    if w > 8 && h > 8 {
        let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
        let mut cd = vec![[0.0f32; 4]; cw * ch];
        let mut ck = vec![false; cw * ch];
        for y in 0..ch {
            for x in 0..cw {
                let mut s = [0.0f32; 4];
                let mut n = 0.0;
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let (fx, fy) = (2 * x + dx, 2 * y + dy);
                    if fx < w && fy < h && known[fy * w + fx] {
                        let p = data[fy * w + fx];
                        for c in 0..4 {
                            s[c] += p[c];
                        }
                        n += 1.0;
                    }
                }
                if n > 0.0 {
                    cd[y * cw + x] = s.map(|v| v / n);
                    ck[y * cw + x] = true;
                }
            }
        }
        membrane_core(&mut cd, &ck, cw, ch);
        for y in 0..h {
            for x in 0..w {
                if !known[y * w + x] {
                    data[y * w + x] = cd[(y / 2).min(ch - 1) * cw + (x / 2).min(cw - 1)];
                }
            }
        }
    }
    // Jacobi relaxation of the unknown pixels (in place, Gauss–Seidel order).
    let iters = if w * h <= 4096 { 200 } else { 40 };
    let unknown: Vec<usize> = (0..w * h).filter(|&i| !known[i]).collect();
    for _ in 0..iters {
        let mut delta = 0.0f32;
        for &i in &unknown {
            let (x, y) = (i % w, i / w);
            let mut s = [0.0f32; 4];
            let mut n = 0.0;
            for (nx, ny) in [(x.wrapping_sub(1), y), (x + 1, y), (x, y.wrapping_sub(1)), (x, y + 1)] {
                if nx < w && ny < h {
                    let p = data[ny * w + nx];
                    for c in 0..4 {
                        s[c] += p[c];
                    }
                    n += 1.0;
                }
            }
            let v = s.map(|v| v / n);
            delta = delta.max((v[0] - data[i][0]).abs());
            data[i] = v;
        }
        if delta < 1e-5 {
            break;
        }
    }
}

/// Membrane fill: `img` with the `hole` pixels replaced by the harmonic interpolation of their
/// border.
pub fn membrane_fill(img: &Image, hole: &[bool]) -> Image {
    let (w, h) = (img.width as usize, img.height as usize);
    let mut out = img.clone();
    let known: Vec<bool> = hole.iter().map(|h| !*h).collect();
    membrane_core(&mut out.data, &known, w, h);
    out
}

// ------------------------------------------------------------------ PatchMatch

struct Level {
    w: usize,
    h: usize,
    img: Vec<[f32; 4]>,
    hole: Vec<bool>,
}

fn downsample(l: &Level) -> Level {
    let (w, h) = (l.w.div_ceil(2), l.h.div_ceil(2));
    let mut img = vec![[0.0f32; 4]; w * h];
    let mut hole = vec![false; w * h];
    for y in 0..h {
        for x in 0..w {
            let mut s = [0.0f32; 4];
            let mut n = 0.0;
            let mut any_hole = false;
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let (fx, fy) = ((2 * x + dx).min(l.w - 1), (2 * y + dy).min(l.h - 1));
                let i = fy * l.w + fx;
                if l.hole[i] {
                    any_hole = true;
                } else {
                    for c in 0..4 {
                        s[c] += l.img[i][c];
                    }
                    n += 1.0;
                }
            }
            hole[y * w + x] = any_hole;
            if n > 0.0 {
                img[y * w + x] = s.map(|v| v / n);
            }
        }
    }
    Level { w, h, img, hole }
}

/// Summed-area table of hole pixels, for "is this patch fully known" queries.
struct HoleSat {
    w: usize,
    s: Vec<u32>,
}

impl HoleSat {
    fn new(hole: &[bool], w: usize, h: usize) -> HoleSat {
        let sw = w + 1;
        let mut s = vec![0u32; sw * (h + 1)];
        for y in 0..h {
            for x in 0..w {
                s[(y + 1) * sw + x + 1] = hole[y * w + x] as u32 + s[y * sw + x + 1] + s[(y + 1) * sw + x] - s[y * sw + x];
            }
        }
        HoleSat { w: sw, s }
    }
    /// Hole pixels in [x0, x1) × [y0, y1).
    fn count(&self, x0: usize, y0: usize, x1: usize, y1: usize) -> u32 {
        self.s[y1 * self.w + x1] + self.s[y0 * self.w + x0] - self.s[y0 * self.w + x1] - self.s[y1 * self.w + x0]
    }
}

struct Pm<'a> {
    w: usize,
    h: usize,
    r: usize,
    img: &'a [[f32; 4]],
    sat: HoleSat,
}

impl Pm<'_> {
    fn valid_source(&self, x: i64, y: i64) -> bool {
        let r = self.r as i64;
        x >= r
            && y >= r
            && x + r < self.w as i64
            && y + r < self.h as i64
            && self.sat.count((x - r) as usize, (y - r) as usize, (x + r + 1) as usize, (y + r + 1) as usize) == 0
    }
    /// SSD between the patch at target (tx, ty) (clipped to the image) and source (sx, sy).
    fn dist(&self, tx: i64, ty: i64, sx: i64, sy: i64, cap: f32) -> f32 {
        let r = self.r as i64;
        let mut d = 0.0f32;
        let mut n = 0.0f32;
        for dy in -r..=r {
            let (ty2, sy2) = (ty + dy, sy + dy);
            if ty2 < 0 || ty2 >= self.h as i64 {
                continue;
            }
            for dx in -r..=r {
                let tx2 = tx + dx;
                if tx2 < 0 || tx2 >= self.w as i64 {
                    continue;
                }
                let a = self.img[ty2 as usize * self.w + tx2 as usize];
                let b = self.img[sy2 as usize * self.w + (sx + dx) as usize];
                for c in 0..4 {
                    let e = a[c] - b[c];
                    d += e * e;
                }
                n += 1.0;
            }
            if d > cap * n.max(1.0) {
                return f32::INFINITY;
            }
        }
        d / n.max(1.0)
    }
}

/// One Wexler/PatchMatch level: refine `img` (hole pixels) with `em` EM rounds, starting from the
/// nearest-neighbour field `nnf` (per pixel, source centre; `None` = random).
fn pm_level(l: &mut Level, nnf: &mut Vec<Option<[i32; 2]>>, em: usize, pm_iters: usize, patch: usize, seed: u32) {
    let (w, h) = (l.w, l.h);
    let r = (patch / 2).min((w.min(h).saturating_sub(1)) / 2).max(1);
    // Target centres: pixels whose patch touches the hole.
    let near = dilate(&l.hole, w, h, r);
    let targets: Vec<usize> = (0..w * h).filter(|&i| near[i]).collect();
    if targets.is_empty() {
        return;
    }
    let sat = HoleSat::new(&l.hole, w, h);
    // Sources: centres of fully known patches.
    let sources: Vec<[i32; 2]> = {
        let pm = Pm { w, h, r, img: &l.img, sat: HoleSat::new(&l.hole, w, h) };
        (0..w * h).filter(|&i| pm.valid_source((i % w) as i64, (i / w) as i64)).map(|i| [(i % w) as i32, (i / w) as i32]).collect()
    };
    if sources.is_empty() {
        return;
    }
    nnf.resize(w * h, None);
    for e in 0..em {
        let pm = Pm { w, h, r, img: &l.img, sat: HoleSat { w: sat.w, s: sat.s.clone() } };
        // Initialise / validate.
        let mut field: Vec<([i32; 2], f32)> = vec![([0, 0], f32::INFINITY); w * h];
        targets
            .par_iter()
            .map(|&i| {
                let (x, y) = ((i % w) as i64, (i / w) as i64);
                let s = nnf[i]
                    .filter(|s| pm.valid_source(s[0] as i64, s[1] as i64))
                    .unwrap_or_else(|| sources[rnd(seed ^ e as u32, i as u32, 0) as usize % sources.len()]);
                (i, (s, pm.dist(x, y, s[0] as i64, s[1] as i64, f32::INFINITY)))
            })
            .collect::<Vec<_>>()
            .into_iter()
            .for_each(|(i, v)| field[i] = v);
        let is_target = &near;
        for it in 0..pm_iters {
            let fwd = it % 2 == 0;
            let snap = field.clone();
            let rows: Vec<Vec<(usize, ([i32; 2], f32))>> = (0..h)
                .into_par_iter()
                .map(|y| {
                    let mut row: Vec<([i32; 2], f32)> = snap[y * w..(y + 1) * w].to_vec();
                    let mut out = vec![];
                    let xs: Box<dyn Iterator<Item = usize>> = if fwd { Box::new(0..w) } else { Box::new((0..w).rev()) };
                    for x in xs {
                        let i = y * w + x;
                        if !is_target[i] {
                            continue;
                        }
                        let (mut best, mut bd) = row[x];
                        let (xi, yi) = (x as i64, y as i64);
                        let try_c = |c: [i32; 2], best: &mut [i32; 2], bd: &mut f32| {
                            if c != *best && pm.valid_source(c[0] as i64, c[1] as i64) {
                                let d = pm.dist(xi, yi, c[0] as i64, c[1] as i64, *bd);
                                if d < *bd {
                                    *bd = d;
                                    *best = c;
                                }
                            }
                        };
                        // Propagation from the scan-order neighbours.
                        let step: i64 = if fwd { -1 } else { 1 };
                        let nx = xi + step;
                        if nx >= 0 && nx < w as i64 && is_target[y * w + nx as usize] {
                            let s = row[nx as usize].0;
                            try_c([s[0] - step as i32, s[1]], &mut best, &mut bd);
                        }
                        let ny = yi + step;
                        if ny >= 0 && ny < h as i64 && is_target[ny as usize * w + x] {
                            let s = snap[ny as usize * w + x].0;
                            try_c([s[0], s[1] - step as i32], &mut best, &mut bd);
                        }
                        // Random search around the best match, halving the radius.
                        let mut rad = w.max(h) as i64;
                        let mut k = 0u32;
                        while rad >= 1 {
                            let a = rnd(seed.wrapping_add(it as u32 * 977 + e as u32 * 31), i as u32, k);
                            let ox = (a % (2 * rad as u32 + 1)) as i64 - rad;
                            let oy = ((a >> 16) % (2 * rad as u32 + 1)) as i64 - rad;
                            try_c([(best[0] as i64 + ox) as i32, (best[1] as i64 + oy) as i32], &mut best, &mut bd);
                            rad /= 2;
                            k += 1;
                        }
                        // And a random source anywhere (helps escape bad starts).
                        let any = sources[rnd(seed ^ 0xABCD, i as u32, it as u32 + e as u32 * 64) as usize % sources.len()];
                        try_c(any, &mut best, &mut bd);
                        row[x] = (best, bd);
                        out.push((i, (best, bd)));
                    }
                    out
                })
                .collect();
            for row in rows {
                for (i, v) in row {
                    field[i] = v;
                }
            }
        }
        // Vote.
        let mut ds: Vec<f32> = targets.iter().map(|&i| field[i].1).filter(|d| d.is_finite()).collect();
        ds.sort_by(f32::total_cmp);
        let sigma2 = ds.get(ds.len() * 3 / 4).copied().unwrap_or(1.0).max(1e-5);
        let ri = r as i64;
        let new: Vec<(usize, [f32; 4])> = (0..w * h)
            .into_par_iter()
            .filter(|&i| l.hole[i])
            .map(|i| {
                let (x, y) = ((i % w) as i64, (i / w) as i64);
                let mut acc = [0.0f32; 4];
                let mut wsum = 0.0f32;
                for dy in -ri..=ri {
                    for dx in -ri..=ri {
                        let (px, py) = (x + dx, y + dy);
                        if px < 0 || py < 0 || px >= w as i64 || py >= h as i64 {
                            continue;
                        }
                        let (s, d) = field[py as usize * w + px as usize];
                        if !d.is_finite() {
                            continue;
                        }
                        let (sx, sy) = (s[0] as i64 - dx, s[1] as i64 - dy);
                        let wt = (-d / (2.0 * sigma2)).exp();
                        let v = l.img[sy as usize * w + sx as usize];
                        for c in 0..4 {
                            acc[c] += v[c] * wt;
                        }
                        wsum += wt;
                    }
                }
                (i, if wsum > 0.0 { acc.map(|v| v / wsum) } else { l.img[i] })
            })
            .collect();
        for (i, v) in new {
            l.img[i] = v;
        }
        for &i in &targets {
            nnf[i] = Some(field[i].0);
        }
    }
}

/// Fill the `hole` of `img` with PatchMatch synthesis (multi-scale Wexler EM). `init`: a starting
/// guess for the hole (e.g. the previous frame's fill warped here); with it only the finest
/// scale is refined.
pub fn patch_fill(img: &Image, hole: &[bool], init: Option<&Image>, patch: usize, seed: u32) -> Image {
    let (w, h) = (img.width as usize, img.height as usize);
    if !hole.iter().any(|b| *b) || w == 0 || h == 0 {
        return img.clone();
    }
    let patch = patch.max(3) | 1;
    let fine = Level { w, h, img: img.data.clone(), hole: hole.to_vec() };
    if let Some(init) = init {
        let mut l = fine;
        for (i, v) in l.img.iter_mut().enumerate() {
            if l.hole[i] {
                *v = init.data[i];
            }
        }
        let mut nnf = vec![];
        pm_level(&mut l, &mut nnf, 2, 4, patch, seed);
        return Image { width: img.width, height: img.height, data: l.img };
    }
    // Pyramid: stop while the image is still a few patches across.
    let mut levels = vec![fine];
    while levels.len() < 6 {
        let Some(last) = levels.last() else { break };
        if last.w.min(last.h) / 2 < patch * 4 || !last.hole.iter().any(|b| *b) {
            break;
        }
        let d = downsample(last);
        // Keep at least one fully known patch.
        if !d.hole.iter().any(|b| !*b) {
            break;
        }
        levels.push(d);
    }
    // Coarsest: membrane start.
    let top = levels.len() - 1;
    {
        let l = &mut levels[top];
        let known: Vec<bool> = l.hole.iter().map(|b| !*b).collect();
        membrane_core(&mut l.img, &known, l.w, l.h);
    }
    let mut nnf: Vec<Option<[i32; 2]>> = vec![];
    for k in (0..=top).rev() {
        if k < top {
            // Upsample the coarser result into this level's hole and the NNF (×2).
            let (coarse_img, cw, ch, coarse_nnf) = {
                let c = &levels[k + 1];
                (c.img.clone(), c.w, c.h, nnf.clone())
            };
            let l = &mut levels[k];
            for y in 0..l.h {
                for x in 0..l.w {
                    let i = y * l.w + x;
                    if l.hole[i] {
                        // Bilinear sample of the coarse image.
                        let fx = ((x as f32 + 0.5) / 2.0 - 0.5).clamp(0.0, (cw - 1) as f32);
                        let fy = ((y as f32 + 0.5) / 2.0 - 0.5).clamp(0.0, (ch - 1) as f32);
                        let (x0, y0) = (fx.floor() as usize, fy.floor() as usize);
                        let (x1, y1) = ((x0 + 1).min(cw - 1), (y0 + 1).min(ch - 1));
                        let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
                        let g = |xx: usize, yy: usize| coarse_img[yy * cw + xx];
                        let mut v = [0.0f32; 4];
                        for c in 0..4 {
                            let top = g(x0, y0)[c] + (g(x1, y0)[c] - g(x0, y0)[c]) * tx;
                            let bot = g(x0, y1)[c] + (g(x1, y1)[c] - g(x0, y1)[c]) * tx;
                            v[c] = top + (bot - top) * ty;
                        }
                        l.img[i] = v;
                    }
                }
            }
            nnf = (0..l.w * l.h)
                .map(|i| {
                    let (x, y) = (i % l.w, i / l.w);
                    let ci = (y / 2).min(ch - 1) * cw + (x / 2).min(cw - 1);
                    coarse_nnf.get(ci).copied().flatten().map(|s| [s[0] * 2 + (x % 2) as i32, s[1] * 2 + (y % 2) as i32])
                })
                .collect();
        }
        let em = if k == top { 8 } else { 4 };
        pm_level(&mut levels[k], &mut nnf, em, 5, patch, seed.wrapping_add(k as u32 * 101));
    }
    let l = levels.swap_remove(0);
    Image { width: img.width, height: img.height, data: l.img }
}

// ------------------------------------------------------------------ flow-guided propagation

/// Block flow from `a` to `b` with the tiles that touch either hole replaced by the membrane
/// interpolation of the surrounding motion (flow completion).
fn completed_flow(a: &Image, b: &Image, hole_a: &[bool], hole_b: &[bool]) -> Flow {
    let mut f = block_flow(a, b, 8, 6);
    if f.v.is_empty() {
        return f;
    }
    let (w, bs) = (a.width as usize, f.block as usize);
    let mut known = vec![true; f.v.len()];
    for (i, k) in known.iter_mut().enumerate() {
        let (c, r) = (i % f.cols, i / f.cols);
        // A tile is unreliable when any pixel within it (and a block's margin) is a hole.
        let (x0, y0) = ((c * bs).saturating_sub(bs / 2), (r * bs).saturating_sub(bs / 2));
        let (x1, y1) = (((c + 1) * bs + bs / 2).min(w), ((r + 1) * bs + bs / 2).min(a.height as usize));
        'tile: for y in y0..y1 {
            for x in x0..x1 {
                if hole_a[y * w + x] || hole_b[y * w + x] {
                    *k = false;
                    break 'tile;
                }
            }
        }
    }
    if known.iter().all(|k| !*k) {
        f.v.iter_mut().for_each(|v| *v = [0.0; 2]);
        return f;
    }
    let mut d: Vec<[f32; 4]> = f.v.iter().map(|v| [v[0], v[1], 0.0, 0.0]).collect();
    membrane_core(&mut d, &known, f.cols, f.rows);
    for (v, x) in f.v.iter_mut().zip(d) {
        *v = [x[0], x[1]];
    }
    f
}

/// The input of a sequence fill.
pub struct FillInput {
    /// Frames (premultiplied RGBA, all the same size).
    pub frames: Vec<Image>,
    /// Per frame, `true` where the pixel is missing.
    pub holes: Vec<Vec<bool>>,
    /// Complete images for some frames: (frame index, image).
    pub references: Vec<(usize, Image)>,
}

/// Shared frame and flow data for tracing pixels through a sequence.
struct TraceContext<'a> {
    fwd: &'a [Flow],
    bwd: &'a [Flow],
    known: &'a (dyn Fn(usize, i64, i64) -> bool + Sync),
    frames: &'a [&'a Image],
    max: usize,
}

/// Starting frame, point, and direction for a flow trace.
struct TraceStart {
    frame: usize,
    x: f64,
    y: f64,
    step: i64,
}

/// Trace a point to the nearest visible frame: (sampled colour, frames travelled).
fn trace(context: &TraceContext<'_>, start: TraceStart) -> Option<([f32; 4], usize)> {
    let n = context.frames.len();
    let (w, h) = (context.frames[0].width as f64, context.frames[0].height as f64);
    let (mut px, mut py) = (start.x, start.y);
    let mut k = start.frame as i64;
    for d in 1..=context.max {
        let v = if start.step > 0 {
            if k as usize + 1 >= n {
                return None;
            }
            context.fwd[k as usize].at(px, py)
        } else {
            if k == 0 {
                return None;
            }
            context.bwd[k as usize - 1].at(px, py)
        };
        px += v[0] as f64;
        py += v[1] as f64;
        k += start.step;
        if px < 0.0 || py < 0.0 || px >= w || py >= h {
            return None;
        }
        // Visible when the bilinear support is known.
        let (x0, y0) = ((px - 0.5).floor() as i64, (py - 0.5).floor() as i64);
        let ok = [(x0, y0), (x0 + 1, y0), (x0, y0 + 1), (x0 + 1, y0 + 1)]
            .iter()
            .all(|&(xx, yy)| (context.known)(k as usize, xx.clamp(0, w as i64 - 1), yy.clamp(0, h as i64 - 1)));
        if ok {
            return Some((context.frames[k as usize].sample_bilinear_clamped(px, py), d));
        }
    }
    None
}

/// Fill a range of frames. `progress(done, total)` returns `false` to cancel (→ `None`).
pub fn fill_sequence(input: &FillInput, o: &FillOpts, progress: &mut dyn FnMut(usize, usize) -> bool) -> Option<Vec<Image>> {
    let n = input.frames.len();
    if n == 0 {
        return Some(vec![]);
    }
    let (w, h) = (input.frames[0].width as usize, input.frames[0].height as usize);
    // Steps: flows (1) + propagation (1) + per-frame synthesis (n).
    let total = n + 2;
    if o.method == FillMethod::EdgeBlend {
        let mut out = Vec::with_capacity(n);
        for (i, (f, hl)) in input.frames.iter().zip(&input.holes).enumerate() {
            out.push(opaque_fill(membrane_fill(f, hl), hl));
            if !progress(i + 1, n) {
                return None;
            }
        }
        return Some(out);
    }
    // Reference frames make their frame fully known.
    let mut frames: Vec<Image> = input.frames.clone();
    let mut holes: Vec<Vec<bool>> = input.holes.clone();
    let mut is_ref = vec![false; n];
    for (k, img) in &input.references {
        if *k < n && img.width as usize == w && img.height as usize == h {
            frames[*k] = img.clone();
            holes[*k] = vec![false; w * h];
            is_ref[*k] = true;
        }
    }
    // 1. Flows between membrane-filled neighbours, completed inside the holes.
    let pre: Vec<Image> = frames.par_iter().zip(&holes).map(|(f, hl)| membrane_fill(f, hl)).collect();
    let pairs: Vec<(Flow, Flow)> = (0..n.saturating_sub(1))
        .into_par_iter()
        .map(|i| (completed_flow(&pre[i], &pre[i + 1], &holes[i], &holes[i + 1]), completed_flow(&pre[i + 1], &pre[i], &holes[i + 1], &holes[i])))
        .collect();
    let (fwd, bwd): (Vec<Flow>, Vec<Flow>) = pairs.into_iter().unzip();
    if !progress(1, total) {
        return None;
    }
    // 2. Propagation (and the lighting-correction border ring).
    let refs: Vec<&Image> = frames.iter().collect();
    let known = |k: usize, x: i64, y: i64| !holes[k][y as usize * w + x as usize];
    let trace_context = TraceContext { fwd: &fwd, bwd: &bwd, known: &known, frames: &refs, max: n };
    let ring_r = 2usize;
    let propagated: Vec<(Image, Vec<bool>)> = (0..n)
        .into_par_iter()
        .map(|t| {
            let mut img = frames[t].clone();
            let mut left = holes[t].clone();
            if is_ref[t] || !holes[t].iter().any(|b| *b) {
                return (img, left);
            }
            let sample = |i: usize| -> Option<[f32; 4]> {
                let (x, y) = ((i % w) as f64 + 0.5, (i / w) as f64 + 0.5);
                let f = trace(&trace_context, TraceStart { frame: t, x, y, step: 1 });
                let b = trace(&trace_context, TraceStart { frame: t, x, y, step: -1 });
                match (f, b) {
                    (Some((a, da)), Some((c, dc))) => {
                        let (wa, wc) = (1.0 / da as f32, 1.0 / dc as f32);
                        Some(std::array::from_fn(|k| (a[k] * wa + c[k] * wc) / (wa + wc)))
                    }
                    (Some((a, _)), None) | (None, Some((a, _))) => Some(a),
                    _ => None,
                }
            };
            let filled: Vec<(usize, [f32; 4])> = (0..w * h).filter(|&i| holes[t][i]).filter_map(|i| sample(i).map(|v| (i, v))).collect();
            for (i, v) in &filled {
                img.data[*i] = *v;
                left[*i] = false;
            }
            // Lighting correction: membrane of the border mismatch, added to propagated pixels.
            if o.lighting > 0.0 && !filled.is_empty() {
                let grown = dilate(&holes[t], w, h, ring_r);
                let mut diff = vec![[0.0f32; 4]; w * h];
                let mut dk = vec![true; w * h];
                for i in 0..w * h {
                    if holes[t][i] {
                        dk[i] = false;
                    } else if grown[i]
                        && let Some(v) = sample(i)
                    {
                        let a = frames[t].data[i];
                        diff[i] = std::array::from_fn(|c| if c < 3 { a[c] - v[c] } else { 0.0 });
                    }
                }
                membrane_core(&mut diff, &dk, w, h);
                for (i, _) in &filled {
                    for c in 0..3 {
                        img.data[*i][c] += o.lighting * diff[*i][c];
                    }
                }
            }
            (img, left)
        })
        .collect();
    if !progress(2, total) {
        return None;
    }
    // 3. Synthesis of what no frame shows.
    let mut out: Vec<Option<Image>> = vec![None; n];
    let first = (0..n).find(|&t| propagated[t].1.iter().any(|b| *b));
    let Some(t0) = first else {
        return Some(propagated.into_iter().zip(&holes).map(|((img, _), hl)| opaque_fill(img, hl)).collect());
    };
    // Frames before t0 are complete already.
    for (t, slot) in out.iter_mut().enumerate().take(t0) {
        *slot = Some(propagated[t].0.clone());
    }
    let mut done = 2;
    out[t0] = Some(patch_fill(&propagated[t0].0, &propagated[t0].1, None, o.patch, o.seed));
    done += 1;
    if !progress(done, total) {
        return None;
    }
    for t in t0 + 1..n {
        let (img, left) = &propagated[t];
        if !left.iter().any(|b| *b) {
            out[t] = Some(img.clone());
        } else {
            // Warp the previous result here along the backward flow (t → t − 1).
            // Frames after `t0` are filled in order, so the previous one is set.
            let Some(prev) = out[t - 1].as_ref() else {
                out[t] = Some(patch_fill(img, left, None, o.patch, o.seed));
                continue;
            };
            let mut warped = img.clone();
            for i in 0..w * h {
                if left[i] {
                    let (x, y) = ((i % w) as f64 + 0.5, (i / w) as f64 + 0.5);
                    let v = bwd[t - 1].at(x, y);
                    warped.data[i] = prev.sample_bilinear_clamped(x + v[0] as f64, y + v[1] as f64);
                }
            }
            out[t] = Some(match o.method {
                FillMethod::Surface => warped,
                _ => patch_fill(img, left, Some(&warped), o.patch, o.seed),
            });
        }
        done += 1;
        if !progress(done, total) {
            return None;
        }
    }
    Some(out.into_iter().zip(&holes).map(|(img, hl)| opaque_fill(img.unwrap_or_default(), hl)).collect())
}

/// Filled pixels are opaque.
fn opaque_fill(mut img: Image, hole: &[bool]) -> Image {
    for (p, hl) in img.data.iter_mut().zip(hole) {
        if *hl && p[3] > 1e-6 {
            let a = p[3];
            *p = [p[0] / a, p[1] / a, p[2] / a, 1.0];
        }
    }
    img
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash_noise;

    /// A textured background (periodic stripes + blocky noise), shifted by `dx`.
    fn background(w: u32, h: u32, dx: f32) -> Image {
        let mut img = Image::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let fx = x as f32 + dx;
                let s = 0.5 + 0.3 * (fx * std::f32::consts::TAU / 10.0).sin();
                let n = hash_noise(((fx.floor() as i64).rem_euclid(20) / 4) as u32, (y % 20) / 4, 3);
                img.set(x, y, [s, 0.3 + 0.4 * n, 0.6 - 0.3 * s * n, 1.0]);
            }
        }
        img
    }

    fn cut(img: &Image, hole: &[bool]) -> Image {
        let mut o = img.clone();
        for (p, h) in o.data.iter_mut().zip(hole) {
            if *h {
                *p = [0.0; 4];
            }
        }
        o
    }

    fn rect_hole(w: u32, h: u32, x0: u32, y0: u32, s: u32) -> Vec<bool> {
        (0..w * h).map(|i| (x0..x0 + s).contains(&(i % w)) && (y0..y0 + s).contains(&(i / w))).collect()
    }

    #[test]
    fn membrane_matches_a_linear_ramp() {
        let (w, h) = (40u32, 30u32);
        let mut img = Image::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let v = x as f32 / w as f32;
                img.set(x, y, [v, v, v, 1.0]);
            }
        }
        let hole = rect_hole(w, h, 10, 8, 14);
        let f = membrane_fill(&cut(&img, &hole), &hole);
        assert!(psnr(&f, &img, Some(&hole)) > 40.0, "{}", psnr(&f, &img, Some(&hole)));
    }

    #[test]
    fn patchmatch_beats_the_membrane_on_texture() {
        let (w, h) = (96u32, 72u32);
        let gt = background(w, h, 0.0);
        let hole = rect_hole(w, h, 40, 28, 16);
        let src = cut(&gt, &hole);
        let mem = membrane_fill(&src, &hole);
        let pm = patch_fill(&src, &hole, None, 7, 1);
        let (pm_db, mem_db) = (psnr(&pm, &gt, Some(&hole)), psnr(&mem, &gt, Some(&hole)));
        assert!(pm_db > mem_db + 3.0 && pm_db > 20.0, "PatchMatch {pm_db:.1} dB vs membrane {mem_db:.1} dB");
        // Known pixels are untouched.
        for i in 0..hole.len() {
            if !hole[i] {
                assert_eq!(pm.data[i], gt.data[i]);
            }
        }
    }

    /// A moving object removed from a (static or panning) textured background.
    fn sequence(n: usize, pan: f32) -> (FillInput, Vec<Image>) {
        let (w, h) = (96u32, 64u32);
        let mut frames = vec![];
        let mut holes = vec![];
        let mut gts = vec![];
        for t in 0..n {
            let gt = background(w, h, t as f32 * pan);
            let hole = rect_hole(w, h, 10 + 8 * t as u32, 22, 18);
            frames.push(cut(&gt, &hole));
            holes.push(hole);
            gts.push(gt);
        }
        (FillInput { frames, holes, references: vec![] }, gts)
    }

    #[test]
    fn object_fill_recovers_background_and_is_stable() {
        for pan in [0.0, 1.0] {
            let (input, gts) = sequence(8, pan);
            let out = fill_sequence(&input, &FillOpts::default(), &mut |_, _| true).expect("fill");
            for t in 0..out.len() {
                let db = psnr(&out[t], &gts[t], Some(&input.holes[t]));
                assert!(db > 26.0, "frame {t} pan {pan}: {db:.1} dB");
                assert_eq!(out[t].data[0][3], 1.0);
            }
            if pan == 0.0 {
                // Temporal consistency: on a static background the filled pixels agree between
                // frames.
                for t in 1..out.len() {
                    let both: Vec<bool> = input.holes[t].iter().zip(&input.holes[t - 1]).map(|(a, b)| *a || *b).collect();
                    let db = psnr(&out[t], &out[t - 1], Some(&both));
                    assert!(db > 30.0, "frames {}–{t} differ: {db:.1} dB", t - 1);
                }
            }
        }
    }

    #[test]
    fn surface_and_edge_blend_and_references() {
        // A hole that never moves: nothing to propagate, so synthesis must carry it.
        let (w, h) = (80u32, 60u32);
        let gt = background(w, h, 0.0);
        let hole = rect_hole(w, h, 30, 20, 14);
        let input = FillInput { frames: vec![cut(&gt, &hole); 4], holes: vec![hole.clone(); 4], references: vec![] };
        let surf = fill_sequence(&input, &FillOpts { method: FillMethod::Surface, ..Default::default() }, &mut |_, _| true).expect("fill");
        for t in 1..4 {
            // Surface carries one fill through: identical frames.
            assert!(psnr(&surf[t], &surf[0], Some(&hole)) > 40.0);
        }
        let edge = fill_sequence(&input, &FillOpts { method: FillMethod::EdgeBlend, ..Default::default() }, &mut |_, _| true).expect("fill");
        assert!(psnr(&edge[0], &membrane_fill(&input.frames[0], &hole), Some(&hole)) > 60.0);
        // A clean reference frame recovers the background exactly.
        let mut with_ref = FillInput { frames: input.frames.clone(), holes: input.holes.clone(), references: vec![(2, gt.clone())] };
        let out = fill_sequence(&with_ref, &FillOpts::default(), &mut |_, _| true).expect("fill");
        for (t, f) in out.iter().enumerate() {
            assert!(psnr(f, &gt, Some(&hole)) > 35.0, "frame {t}: {:.1}", psnr(f, &gt, Some(&hole)));
        }
        // Cancel.
        with_ref.references.clear();
        assert!(fill_sequence(&with_ref, &FillOpts::default(), &mut |d, _| d < 2).is_none());
    }

    #[test]
    fn lighting_correction_absorbs_a_brightness_change() {
        // The background brightens over time; propagated pixels come from darker frames.
        let (mut input, mut gts) = sequence(6, 0.0);
        for t in 0..6 {
            let k = 1.0 + 0.06 * t as f32;
            for img in [&mut input.frames[t], &mut gts[t]] {
                for p in &mut img.data {
                    for c in 0..3 {
                        p[c] *= k;
                    }
                }
            }
        }
        let plain = fill_sequence(&input, &FillOpts::default(), &mut |_, _| true).expect("fill");
        let lit = fill_sequence(&input, &FillOpts { lighting: 1.0, ..Default::default() }, &mut |_, _| true).expect("fill");
        let score = |o: &[Image]| (0..6).map(|t| psnr(&o[t], &gts[t], Some(&input.holes[t]))).sum::<f64>();
        assert!(score(&lit) > score(&plain), "lit {} vs plain {}", score(&lit), score(&plain));
    }
}

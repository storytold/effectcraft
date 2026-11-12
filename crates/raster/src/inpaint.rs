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
use crate::Px;
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

/// Block flow `a` → `b` (tile 8, search radius 6) with the tiles that touch either hole replaced
/// by the membrane interpolation of the surrounding motion (flow completion).
fn completed_flow(a: &Image, b: &Image, hole_a: &[bool], hole_b: &[bool]) -> Flow {
    let f = block_flow(a, b, 8, 6);
    complete_flow(f, hole_a, hole_b, a.width as usize, a.height as usize)
}

/// The completion half of `completed_flow`: the tiles of `f` (a `block_flow` of `w × h`) that
/// touch either hole become the harmonic interpolation of the surrounding motion. Split out so a
/// hardware backend can compute the block matching itself and hand the raw field here; the
/// completion is cheap (the field is a `cols × rows` grid) and stays on the CPU.
pub fn complete_flow(mut f: Flow, hole_a: &[bool], hole_b: &[bool], w: usize, h: usize) -> Flow {
    if f.v.is_empty() {
        return f;
    }
    let bs = f.block as usize;
    let mut known = vec![true; f.v.len()];
    for (i, k) in known.iter_mut().enumerate() {
        let (c, r) = (i % f.cols, i / f.cols);
        // A tile is unreliable when any pixel within it (and a block's margin) is a hole.
        let (x0, y0) = ((c * bs).saturating_sub(bs / 2), (r * bs).saturating_sub(bs / 2));
        let (x1, y1) = (((c + 1) * bs + bs / 2).min(w), ((r + 1) * bs + bs / 2).min(h));
        'tile: for y in y0..y1 {
            for x in x0..x1 {
                if hole_a.get(y * w + x).copied().unwrap_or(true) || hole_b.get(y * w + x).copied().unwrap_or(true) {
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

/// A hole mask packed into 64-bit words (1 bit per pixel) to keep the per-frame cost
/// down across a whole range (#2570).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HoleMask {
    pub words: Vec<u64>,
    pub len: usize,
}

impl HoleMask {
    /// Pack one `bool` a pixel into 64-bit words.
    pub fn from_vec(hole: &[bool]) -> Self {
        let mut words = vec![0u64; hole.len().div_ceil(64)];
        for (i, &h) in hole.iter().enumerate() {
            if h {
                words[i / 64] |= 1u64 << (i % 64);
            }
        }
        HoleMask { words, len: hole.len() }
    }
    /// An all-known mask of `n` pixels.
    pub fn zeros(n: usize) -> Self {
        HoleMask { words: vec![0u64; n.div_ceil(64)], len: n }
    }
    /// Whether pixel `i` is in the hole. `words` and `len` are public, so a mask whose words are
    /// shorter than `len` reads the missing pixels as known instead of panicking.
    pub fn get(&self, i: usize) -> bool {
        i < self.len && self.words.get(i / 64).is_some_and(|w| (w >> (i % 64)) & 1 == 1)
    }
    /// Mark pixel `i` as missing (`true`) or known (`false`). A pixel outside the mask is ignored.
    pub fn set(&mut self, i: usize, v: bool) {
        if i >= self.len {
            return;
        }
        let Some(w) = self.words.get_mut(i / 64) else {
            return;
        };
        if v {
            *w |= 1u64 << (i % 64);
        } else {
            *w &= !(1u64 << (i % 64));
        }
    }
    /// Whether any pixel is in the hole.
    pub fn any(&self) -> bool {
        self.words.iter().any(|&w| w != 0)
    }
    /// Unpack to one `bool` a pixel.
    pub fn to_vec(&self) -> Vec<bool> {
        (0..self.len).map(|i| self.get(i)).collect()
    }
}

/// Where a fill's input frames live. `Memory` keeps them in RAM (tests, small ranges);
/// `Spilled` stores them on disk and reads them on demand, so a whole range costs ~0 RAM.
#[derive(Clone)]
pub enum Frames {
    Memory(Vec<Image>),
    Spilled(SpillHandle),
}

impl Frames {
    /// The number of frames in the range.
    pub fn len(&self) -> usize {
        match self {
            Frames::Memory(v) => v.len(),
            Frames::Spilled(h) => h.n,
        }
    }
    /// Whether the range has no frames.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// Frame `k`: a clone from RAM, or a decode from the spill.
    pub fn get(&self, k: usize) -> Image {
        match self {
            Frames::Memory(v) => v[k].clone(),
            Frames::Spilled(h) => h.get(k),
        }
    }
    /// Frame size without decoding a whole image.
    pub fn size(&self) -> Option<(usize, usize)> {
        match self {
            Frames::Memory(v) => v.first().map(|i| (i.width as usize, i.height as usize)),
            Frames::Spilled(h) => Some((h.w, h.h)),
        }
    }
    /// Bilinear sample of one pixel. A spilled frame reads only the 2x2 support from disk, so
    /// the hot trace path never decodes or clones a whole frame (#2570).
    pub fn sample(&self, k: usize, x: f64, y: f64) -> Px {
        match self {
            Frames::Memory(v) => v[k].sample_bilinear_clamped(x, y),
            Frames::Spilled(h) => h.sample(k, x, y),
        }
    }
    /// Directory of the spill file, so a secondary spill can live beside it.
    pub fn scratch_dir(&self) -> Option<std::path::PathBuf> {
        match self {
            Frames::Memory(_) => None,
            Frames::Spilled(h) => h.dir().map(std::path::Path::to_path_buf),
        }
    }
    /// Whether a read or write of a spilled frame failed.
    pub fn has_error(&self) -> bool {
        match self {
            Frames::Memory(_) => false,
            Frames::Spilled(h) => h.has_error(),
        }
    }
    /// Take the first spill failure, clearing it.
    pub fn take_error(&self) -> Option<String> {
        match self {
            Frames::Memory(_) => None,
            Frames::Spilled(h) => h.take_error(),
        }
    }
    /// The error slot shared by every spill in this fill, so a caller holding a clone of the
    /// input handle also sees failures in the propagated-frame spill.
    pub fn error_cell(&self) -> Option<std::sync::Arc<std::sync::Mutex<Option<String>>>> {
        match self {
            Frames::Memory(_) => None,
            Frames::Spilled(h) => Some(h.error_cell()),
        }
    }
}

/// A spill file holding `n` frames of `w x h` pixels at `bpc` bits per channel.
/// Frames are read back on demand. The file is shared (reference-counted), so it lives until the
/// last clone is dropped (#2570 review).
#[derive(Clone)]
pub struct SpillHandle {
    inner: std::sync::Arc<SpillInner>,
    pub w: usize,
    pub h: usize,
    pub n: usize,
    pub bpc: u8,
}

struct SpillInner {
    /// The folder the spill was created in (`None`: the system temporary folder), so the fill's
    /// other spills go beside it.
    dir: Option<std::path::PathBuf>,
    /// An anonymous temporary file: it has no name in `dir`, and the system reclaims it when the
    /// last handle closes, even when the app is killed or runs out of memory partway through a
    /// fill, so a multi-gigabyte scratch file is never left in the user's folder (#2570 review).
    /// Every read and write is positioned on this one handle: several trace threads read the same
    /// file at once, and a shared file offset would need a lock around every read (#2570).
    file: std::fs::File,
    /// Shared with any secondary spill so one probe reports every scratch failure.
    error: std::sync::Arc<std::sync::Mutex<Option<String>>>,
}

impl SpillHandle {
    /// The folder the spill lives in (`None`: the system temporary folder).
    pub fn dir(&self) -> Option<&std::path::Path> {
        self.inner.dir.as_deref()
    }

    /// Bytes a pixel in the spill: RGBA `u8`, or four `f32` above 8 bits a channel.
    fn bytes_per_px(&self) -> usize {
        if self.bpc <= 8 { 4 } else { 16 }
    }

    /// Record a scratch failure in the shared slot, keeping the first.
    pub fn record_error(&self, message: String) {
        if let Ok(mut e) = self.inner.error.lock()
            && e.is_none()
        {
            *e = Some(message);
        }
    }

    /// The error slot shared with any secondary spill.
    pub fn error_cell(&self) -> std::sync::Arc<std::sync::Mutex<Option<String>>> {
        std::sync::Arc::clone(&self.inner.error)
    }

    /// Whether a read or write failed (`true` when the slot is poisoned).
    pub fn has_error(&self) -> bool {
        self.inner.error.lock().map(|e| e.is_some()).unwrap_or(true)
    }

    /// Take the first failure, clearing it.
    pub fn take_error(&self) -> Option<String> {
        self.inner.error.lock().ok().and_then(|mut e| e.take())
    }

    /// Read one whole frame from the spill, reporting a short or failed read.
    fn load_frame(&self, k: usize) -> Image {
        let mut img = Image::new(self.w as u32, self.h as u32);
        let Some(px) = self.w.checked_mul(self.h) else {
            self.record_error("spill: pixel count overflow".into());
            return img;
        };
        let bytes_per_px = self.bytes_per_px();
        let Some(off) = k.checked_mul(px).and_then(|v| v.checked_mul(bytes_per_px)) else {
            self.record_error(format!("spill: offset overflow for frame {k}"));
            return img;
        };
        let Some(len) = px.checked_mul(bytes_per_px) else {
            self.record_error("spill: frame size overflow".into());
            return img;
        };
        let mut buf = vec![0u8; len];
        if let Err(e) = self.read_at(off as u64, &mut buf) {
            self.record_error(format!("spill: {e}"));
            return img;
        }
        if self.bpc <= 8 {
            for (i, p) in img.data.iter_mut().enumerate() {
                let j = i * 4;
                p[0] = buf[j] as f32 / 255.0;
                p[1] = buf[j + 1] as f32 / 255.0;
                p[2] = buf[j + 2] as f32 / 255.0;
                p[3] = buf[j + 3] as f32 / 255.0;
            }
        } else {
            for (i, p) in img.data.iter_mut().enumerate() {
                let base = i * 16;
                for c in 0..4 {
                    let mut b = [0u8; 4];
                    b.copy_from_slice(&buf[base + c * 4..base + c * 4 + 4]);
                    p[c] = f32::from_bits(u32::from_le_bytes(b));
                }
            }
        }
        img
    }

    /// Read a whole frame, decoding it.
    pub fn get(&self, k: usize) -> Image {
        self.load_frame(k)
    }

    /// A positioned read: `pread`/`seek_read` use the shared handle without touching a shared
    /// file offset, so tracing threads do not serialise on the file.
    fn read_at(&self, off: u64, buf: &mut [u8]) -> std::io::Result<()> {
        let file = &self.inner.file;
        #[cfg(unix)]
        {
            use std::os::unix::fs::FileExt;
            file.read_exact_at(buf, off)
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::FileExt;
            let mut done = 0usize;
            while done < buf.len() {
                match file.seek_read(&mut buf[done..], off.saturating_add(done as u64)) {
                    Ok(0) => return Err(std::io::ErrorKind::UnexpectedEof.into()),
                    Ok(n) => done += n,
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(e) => return Err(e),
                }
            }
            Ok(())
        }
        // Elsewhere, seek the shared handle. Spills are only made natively: wasm keeps the
        // propagated frames in RAM and has no file system to spill the input to.
        #[cfg(not(any(unix, windows)))]
        {
            use std::io::{Read, Seek, SeekFrom};
            let mut f = file;
            f.seek(SeekFrom::Start(off))?;
            f.read_exact(buf)
        }
    }

    /// A positioned write on the same handle as `read_at`, so the propagation workers can store
    /// their frames at once and the file never needs a name to be reopened by.
    fn write_at(&self, off: u64, buf: &[u8]) -> std::io::Result<()> {
        let file = &self.inner.file;
        #[cfg(unix)]
        {
            use std::os::unix::fs::FileExt;
            file.write_all_at(buf, off)
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::FileExt;
            let mut done = 0usize;
            while done < buf.len() {
                match file.seek_write(&buf[done..], off.saturating_add(done as u64)) {
                    Ok(0) => return Err(std::io::ErrorKind::WriteZero.into()),
                    Ok(n) => done += n,
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(e) => return Err(e),
                }
            }
            Ok(())
        }
        #[cfg(not(any(unix, windows)))]
        {
            use std::io::{Seek, SeekFrom, Write};
            let mut f = file;
            f.seek(SeekFrom::Start(off))?;
            f.write_all(buf)
        }
    }

    /// One pixel decoded from `buf` at index `i` (transparent if `buf` is short).
    fn px_at(&self, buf: &[u8], i: usize) -> Px {
        if self.bpc <= 8 {
            let j = i * 4;
            [
                buf.get(j).copied().unwrap_or(0) as f32 / 255.0,
                buf.get(j + 1).copied().unwrap_or(0) as f32 / 255.0,
                buf.get(j + 2).copied().unwrap_or(0) as f32 / 255.0,
                buf.get(j + 3).copied().unwrap_or(0) as f32 / 255.0,
            ]
        } else {
            let base = i * 16;
            let mut p = [0.0f32; 4];
            for (c, v) in p.iter_mut().enumerate() {
                let at = base + c * 4;
                let Some(b) = buf.get(at..at + 4).and_then(|s| <[u8; 4]>::try_from(s).ok()) else {
                    return [0.0; 4];
                };
                *v = f32::from_bits(u32::from_le_bytes(b));
            }
            p
        }
    }

    /// The two horizontally adjacent pixels (`x`, `x + 1`, both clamped) of row `y` (clamped),
    /// read straight from the spill file.
    fn edge(&self, k: usize, y: i64, x: i64) -> Option<(Px, Px)> {
        let (w, h) = (self.w, self.h);
        if w == 0 || h == 0 || k >= self.n {
            return None;
        }
        let (iw, ih) = (i64::try_from(w).ok()?, i64::try_from(h).ok()?);
        let y = y.clamp(0, ih - 1) as usize;
        let xa = x.clamp(0, iw - 1) as usize;
        let xb = x.saturating_add(1).clamp(0, iw - 1) as usize;
        let bpp = self.bytes_per_px();
        let off = k
            .checked_mul(w)
            .and_then(|v| v.checked_mul(h))
            .and_then(|v| v.checked_add(y.checked_mul(w)?))
            .and_then(|v| v.checked_add(xa))
            .and_then(|v| v.checked_mul(bpp));
        let Some(off) = off.and_then(|v| u64::try_from(v).ok()) else {
            self.record_error("spill: sample offset overflow".into());
            return None;
        };
        // One read covers both pixels unless the right edge clamps them together. A stack buffer
        // keeps the per-sample allocation out of the trace loop.
        let len = (xb - xa + 1) * bpp;
        let mut held = [0u8; 32];
        let buf = held.get_mut(..len)?;
        if let Err(e) = self.read_at(off, buf) {
            self.record_error(format!("spill: {e}"));
            return None;
        }
        let a = self.px_at(buf, 0);
        let b = if xb > xa { self.px_at(buf, 1) } else { a };
        Some((a, b))
    }

    /// Bilinear sample of one pixel, reading only its 2x2 support. Matches
    /// `Image::sample_bilinear_clamped` exactly, with the same edge clamping.
    pub fn sample(&self, k: usize, x: f64, y: f64) -> Px {
        let fx = x - 0.5;
        let fy = y - 0.5;
        let (x0, y0) = (fx.floor(), fy.floor());
        let (tx, ty) = ((fx - x0) as f32, (fy - y0) as f32);
        // `as i64` saturates NaN and out-of-range values, as in `sample_bilinear_clamped`.
        let (x0, y0) = (x0 as i64, y0 as i64);
        let (Some((a, b)), Some((c, d))) = (self.edge(k, y0, x0), self.edge(k, y0.saturating_add(1), x0)) else {
            return [0.0; 4];
        };
        let mut o = [0.0; 4];
        for i in 0..4 {
            let top = a[i] + (b[i] - a[i]) * tx;
            let bot = c[i] + (d[i] - c[i]) * tx;
            o[i] = top + (bot - top) * ty;
        }
        o
    }

    /// Write frame `k` into the spill, reporting a disk error.
    pub fn write_frame(&self, k: usize, img: &Image) -> std::result::Result<(), String> {
        if k >= self.n {
            return Err(format!("spill: frame {k} out of range (n = {})", self.n));
        }
        if img.width as usize != self.w || img.height as usize != self.h {
            return Err(format!("spill: frame {k} is {}x{}, expected {}x{}", img.width, img.height, self.w, self.h));
        }
        let bytes_per_px = self.bytes_per_px();
        let px = self.w.checked_mul(self.h).ok_or("overflow")?;
        let off = k.checked_mul(px).and_then(|v| v.checked_mul(bytes_per_px)).ok_or("overflow")?;
        let buf = if self.bpc <= 8 {
            let mut buf = vec![0u8; px * 4];
            for (i, p) in img.data.iter().enumerate() {
                buf[i * 4] = (p[0] * 255.0).round().clamp(0.0, 255.0) as u8;
                buf[i * 4 + 1] = (p[1] * 255.0).round().clamp(0.0, 255.0) as u8;
                buf[i * 4 + 2] = (p[2] * 255.0).round().clamp(0.0, 255.0) as u8;
                buf[i * 4 + 3] = (p[3] * 255.0).round().clamp(0.0, 255.0) as u8;
            }
            buf
        } else {
            let mut buf = vec![0u8; px * 16];
            for (i, p) in img.data.iter().enumerate() {
                let base = i * 16;
                for c in 0..4 {
                    buf[base + c * 4..base + c * 4 + 4].copy_from_slice(&p[c].to_le_bytes());
                }
            }
            buf
        };
        self.write_at(off as u64, &buf).map_err(|e| format!("spill: {e}"))
    }

    /// Create a spill file. `dir` lets the caller keep scratch next to the output (so a
    /// RAM-backed `/tmp` does not defeat the point of spilling); `None` uses the temp dir.
    pub fn create_in(dir: Option<&std::path::Path>, w: usize, h: usize, n: usize, bpc: u8) -> std::io::Result<Self> {
        Self::create_in_with_error(dir, w, h, n, bpc, None)
    }

    /// `error`: share an existing error slot, so a secondary spill reports through the same
    /// handle the caller already watches (#2570 review).
    pub fn create_in_with_error(
        dir: Option<&std::path::Path>,
        w: usize,
        h: usize,
        n: usize,
        bpc: u8,
        error: Option<std::sync::Arc<std::sync::Mutex<Option<String>>>>,
    ) -> std::io::Result<Self> {
        let bytes_per_px: usize = if bpc <= 8 { 4 } else { 16 };
        let total = n
            .checked_mul(w)
            .and_then(|v| v.checked_mul(h))
            .and_then(|v| v.checked_mul(bytes_per_px))
            .ok_or(std::io::Error::new(std::io::ErrorKind::InvalidInput, "overflow"))?;
        // Anonymous, so nothing is left behind if `set_len` fails below or the app never gets
        // to clean up.
        let file = match dir {
            Some(d) => {
                std::fs::create_dir_all(d)?;
                tempfile::tempfile_in(d)?
            }
            None => tempfile::tempfile()?,
        };
        file.set_len(total as u64)?;
        let error = error.unwrap_or_else(|| std::sync::Arc::new(std::sync::Mutex::new(None)));
        let inner = std::sync::Arc::new(SpillInner { dir: dir.map(std::path::Path::to_path_buf), file, error });
        Ok(SpillHandle { inner, w, h, n, bpc })
    }

    /// Create a spill in the system temporary directory.
    pub fn create(w: usize, h: usize, n: usize, bpc: u8) -> std::io::Result<Self> {
        Self::create_in(None, w, h, n, bpc)
    }
}

/// The input of a sequence fill.
#[derive(Clone)]
pub struct FillInput {
    /// Frames (premultiplied RGBA, all the same size), in RAM or spilled to disk.
    pub frames: Frames,
    /// Per frame, packed hole mask (`true` where the pixel is missing).
    pub holes: Vec<HoleMask>,
    /// Complete images for some frames: (frame index, image).
    pub references: Vec<(usize, Image)>,
}

/// Shared frame and flow data for tracing pixels through a sequence.
struct TraceContext<'a> {
    fwd: &'a [Flow],
    bwd: &'a [Flow],
    known: &'a (dyn Fn(usize, i64, i64) -> bool + Sync),
    frames: &'a Frames,
    ref_images: &'a [Option<Image>],
    w: usize,
    h: usize,
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
    let (w, h) = (context.w as f64, context.h as f64);
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
            let img = if let Some(r) = &context.ref_images[k as usize] {
                r.sample_bilinear_clamped(px, py)
            } else {
                // Samples a pixel; never clones a whole spilled frame (the trace hot path).
                context.frames.sample(k as usize, px, py)
            };
            return Some((img, d));
        }
    }
    None
}

/// A hardware backend for the fill's stages (the GPU compositor implements it; see
/// `effectcraft_render::Accelerator::inpaint`). Every method has the CPU implementation as its
/// default, so a backend that cannot take a stage returns the CPU's work and the fill stays
/// correct — only slower.
///
/// The orchestration ([`fill_sequence_with`]) stays on the CPU: the sequence's frame-to-frame
/// coupling is sequential, and only the per-frame and per-pair work is worth offloading.
///
/// **A backend must be safe to call concurrently.** [`fill_sequence_with`] runs the stages of
/// independent frames and frame pairs in parallel (`rayon`), so `membrane`, `flow` and `patch`
/// may all be entered from several threads at once — a backend that owns a device must serialise
/// its use of it (the GPU compositor's per-call encoder does; a backend whose device cannot be
/// used from several threads must lock).
pub trait FillBackend: Send + Sync {
    /// [`membrane_fill`]: the `hole` pixels take the harmonic interpolation of their border.
    fn membrane(&self, img: &Image, hole: &[bool]) -> Image {
        membrane_fill(img, hole)
    }
    /// `membrane_core`: harmonic interpolation of `data`'s unknown pixels in place (`known` is
    /// true where the pixel is known).
    fn membrane_known(&self, data: &mut [[f32; 4]], known: &[bool], w: usize, h: usize) {
        membrane_core(data, known, w, h)
    }
    /// `completed_flow`: block flow `a` → `b` with the tiles touching either hole replaced by
    /// the membrane interpolation of the surrounding motion.
    fn flow(&self, a: &Image, b: &Image, hole_a: &[bool], hole_b: &[bool]) -> Flow {
        completed_flow(a, b, hole_a, hole_b)
    }
    /// [`patch_fill`]: PatchMatch synthesis of `img`'s `hole` (`init`: a starting guess, e.g. the
    /// previous frame's fill warped here).
    fn patch(&self, img: &Image, hole: &[bool], init: Option<&Image>, patch: usize, seed: u32) -> Image {
        patch_fill(img, hole, init, patch, seed)
    }
}

/// The CPU backend: every stage is the reference implementation.
pub struct CpuFill;

impl FillBackend for CpuFill {}

/// Holds the propagated frames between the parallel propagation pass and the sequential
/// synthesis pass. Native builds spill them beside the input, so a whole range is never
/// resident at once (#2570); wasm keeps them in memory (it has no shared file system).
enum PropStore {
    #[cfg(not(target_arch = "wasm32"))]
    Spilled(SpillHandle),
    #[cfg(target_arch = "wasm32")]
    Memory(std::sync::Mutex<Vec<Option<Image>>>),
}

impl PropStore {
    /// Open the propagated-frame store: spilled natively, in RAM on wasm.
    fn new(
        dir: Option<&std::path::Path>,
        w: usize,
        h: usize,
        n: usize,
        error: Option<std::sync::Arc<std::sync::Mutex<Option<String>>>>,
    ) -> std::io::Result<Self> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            // 32-bit floats: propagating quantized input would lose quality (#2570).
            Ok(PropStore::Spilled(SpillHandle::create_in_with_error(dir, w, h, n, 32, error)?))
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (dir, w, h, error);
            Ok(PropStore::Memory(std::sync::Mutex::new((0..n).map(|_| None).collect())))
        }
    }

    /// Store frame `k`, reporting a spill write failure.
    fn put(&self, k: usize, img: Image) -> std::result::Result<(), String> {
        match self {
            #[cfg(not(target_arch = "wasm32"))]
            PropStore::Spilled(h) => h.write_frame(k, &img),
            #[cfg(target_arch = "wasm32")]
            PropStore::Memory(v) => {
                let mut v = v.lock().map_err(|_| "propagation store poisoned".to_string())?;
                if let Some(slot) = v.get_mut(k) {
                    *slot = Some(img);
                }
                Ok(())
            }
        }
    }

    /// Read a stored frame without consuming it (a frame can be shared by two flows).
    fn get(&self, k: usize) -> Image {
        match self {
            #[cfg(not(target_arch = "wasm32"))]
            PropStore::Spilled(h) => h.get(k),
            #[cfg(target_arch = "wasm32")]
            PropStore::Memory(v) => v.lock().ok().and_then(|v| v.get(k).cloned()).flatten().unwrap_or_default(),
        }
    }

    /// Read a propagated frame once, freeing it where the backing store allows it.
    fn take(&self, k: usize) -> Image {
        match self {
            #[cfg(not(target_arch = "wasm32"))]
            PropStore::Spilled(h) => h.get(k),
            #[cfg(target_arch = "wasm32")]
            PropStore::Memory(v) => v.lock().ok().and_then(|mut v| v.get_mut(k).and_then(Option::take)).unwrap_or_default(),
        }
    }

    /// Record a failure in the spill, so the caller reports it instead of a bare cancellation.
    fn record_error(&self, message: String) {
        match self {
            #[cfg(not(target_arch = "wasm32"))]
            PropStore::Spilled(h) => h.record_error(message),
            #[cfg(target_arch = "wasm32")]
            PropStore::Memory(_) => {
                let _ = message;
            }
        }
    }

    /// Whether the spill failed (always `false` on wasm, which keeps frames in RAM).
    fn has_error(&self) -> bool {
        match self {
            #[cfg(not(target_arch = "wasm32"))]
            PropStore::Spilled(h) => h.has_error(),
            #[cfg(target_arch = "wasm32")]
            PropStore::Memory(_) => false,
        }
    }
}

/// Publish a scratch failure through the input spill's shared error slot, so the caller reports
/// the disk error instead of a bare cancellation.
fn record_spill_error(cell: &Option<std::sync::Arc<std::sync::Mutex<Option<String>>>>, message: String) {
    if let Some(cell) = cell
        && let Ok(mut e) = cell.lock()
        && e.is_none()
    {
        *e = Some(message);
    }
}

/// Membrane-fill every frame and return the forward/backward flow for each neighbouring pair.
/// The filled frames are written to a scratch spill, so the flow pass never holds the range in
/// RAM (#2570).
#[allow(clippy::too_many_arguments)]
fn flow_pairs(
    n: usize,
    w: usize,
    h: usize,
    holes: &[HoleMask],
    is_ref: &[bool],
    ref_images: &[Option<Image>],
    frames: &Frames,
    prop_dir: Option<&std::path::Path>,
    error_cell: Option<std::sync::Arc<std::sync::Mutex<Option<String>>>>,
    backend: &dyn FillBackend,
) -> Option<Vec<(Flow, Flow)>> {
    let store = match PropStore::new(prop_dir, w, h, n, error_cell.clone()) {
        Ok(s) => s,
        Err(e) => {
            record_spill_error(&error_cell, format!("spill: {e}"));
            return None;
        }
    };
    let pre = (0..n).into_par_iter().try_for_each(|t| {
        let base = if is_ref[t] { ref_images[t].clone().unwrap_or_default() } else { frames.get(t) };
        let img = backend.membrane(&base, &holes[t].to_vec());
        store.put(t, img)
    });
    if let Err(e) = pre {
        record_spill_error(&error_cell, e);
        return None;
    }
    if frames.has_error() {
        return None;
    }
    let pairs: Vec<(Flow, Flow)> = (0..n.saturating_sub(1))
        .into_par_iter()
        .map(|i| {
            let a = store.get(i);
            let b = store.get(i + 1);
            let ha = holes[i].to_vec();
            let hb = holes[i + 1].to_vec();
            (backend.flow(&a, &b, &ha, &hb), backend.flow(&b, &a, &hb, &ha))
        })
        .collect();
    let store_failed = store.has_error();
    drop(store);
    if store_failed || frames.has_error() { None } else { Some(pairs) }
}

/// Fill a range of frames on the CPU. `progress(done, total)` returns `false` to cancel
/// (→ `None`).
pub fn fill_sequence(input: &FillInput, o: &FillOpts, progress: &mut dyn FnMut(usize, usize) -> bool) -> Option<Vec<Image>> {
    fill_sequence_with(input.clone(), o, &CpuFill, progress, None)
}

/// [`fill_sequence`] with a hardware backend for the stages it can take (see [`FillBackend`]).
pub fn fill_sequence_with(
    mut input: FillInput,
    o: &FillOpts,
    backend: &dyn FillBackend,
    progress: &mut dyn FnMut(usize, usize) -> bool,
    mut sink: Option<&mut dyn FnMut(usize, &Image) -> Result<(), String>>,
) -> Option<Vec<Image>> {
    let n = input.frames.len();
    if n == 0 {
        return Some(vec![]);
    }
    let (w, h) = input.frames.size().unwrap_or((0, 0));
    if w == 0 || h == 0 {
        return Some(vec![]);
    }
    // Steps: flows (1) + propagation (1) + per-frame synthesis (n).
    let total = n + 2;
    let streaming = sink.is_some();
    if o.method == FillMethod::EdgeBlend {
        let mut out = Vec::with_capacity(n);
        for i in 0..n {
            let f = input.frames.get(i);
            // A failed read returns a black frame, which must never reach the sink.
            if input.frames.has_error() {
                return None;
            }
            let hl = input.holes[i].to_vec();
            let img = opaque_fill(backend.membrane(&f, &hl), &hl);
            if let Some(cb) = sink.as_mut()
                && cb(i, &img).is_err()
            {
                return None;
            }
            if !streaming {
                out.push(img);
            }
            if !progress(i + 1, n) {
                return None;
            }
        }
        if input.frames.has_error() {
            return None;
        }
        if streaming {
            return Some(Vec::new());
        }
        return Some(out);
    }
    // Reference frames make their frame fully known.
    let mut holes: Vec<HoleMask> = std::mem::take(&mut input.holes);
    let references = std::mem::take(&mut input.references);
    let mut is_ref = vec![false; n];
    let mut ref_images: Vec<Option<Image>> = vec![None; n];
    for (k, img) in references {
        if k < n && img.width as usize == w && img.height as usize == h {
            ref_images[k] = Some(img);
            holes[k] = HoleMask::zeros(w * h);
            is_ref[k] = true;
        }
    }
    let prop_dir = input.frames.scratch_dir();
    let error_cell = input.frames.error_cell();
    // 1. Flows between membrane-filled neighbours, completed inside the holes. The
    // membrane-filled frames are spilled too, so the flow pass does not hold the range.
    let pairs = flow_pairs(n, w, h, &holes, &is_ref, &ref_images, &input.frames, prop_dir.as_deref(), error_cell.clone(), backend)?;
    let (fwd, bwd): (Vec<Flow>, Vec<Flow>) = pairs.into_iter().unzip();
    if !progress(1, total) {
        return None;
    }
    // 2. Propagation (and the lighting-correction border ring). The filled frames are
    // spilled as they are produced, so only the leftover masks stay in RAM.
    let store = match PropStore::new(prop_dir.as_deref(), w, h, n, error_cell.clone()) {
        Ok(s) => s,
        Err(e) => {
            record_spill_error(&error_cell, format!("spill: {e}"));
            return None;
        }
    };
    let known = |k: usize, x: i64, y: i64| !holes[k].get(y as usize * w + x as usize);
    let trace_context = TraceContext { fwd: &fwd, bwd: &bwd, known: &known, frames: &input.frames, ref_images: &ref_images, w, h, max: n };
    let ring_r = 2usize;
    let propagated_left: Vec<HoleMask> = (0..n)
        .into_par_iter()
        .map(|t| {
            // Once a frame failed to spill or read the fill is lost: stop tracing the rest (the
            // masks are dropped below).
            if store.has_error() || input.frames.has_error() {
                return HoleMask::default();
            }
            let mut img = if is_ref[t] { ref_images[t].clone().unwrap_or_default() } else { input.frames.get(t) };
            let mut left = holes[t].to_vec();
            if !is_ref[t] && holes[t].any() {
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
                let filled: Vec<(usize, [f32; 4])> = (0..w * h).filter(|&i| holes[t].get(i)).filter_map(|i| sample(i).map(|v| (i, v))).collect();
                for (i, v) in &filled {
                    img.data[*i] = *v;
                    left[*i] = false;
                }
                // Lighting correction: membrane of the border mismatch, added to propagated pixels.
                // Non-hole pixels are untouched above, so `img` is still the original there.
                if o.lighting > 0.0 && !filled.is_empty() {
                    let grown = dilate(&holes[t].to_vec(), w, h, ring_r);
                    let mut diff = vec![[0.0f32; 4]; w * h];
                    let mut dk = vec![true; w * h];
                    for i in 0..w * h {
                        if holes[t].get(i) {
                            dk[i] = false;
                        } else if grown[i]
                            && let Some(v) = sample(i)
                        {
                            let a = img.data[i];
                            diff[i] = std::array::from_fn(|c| if c < 3 { a[c] - v[c] } else { 0.0 });
                        }
                    }
                    backend.membrane_known(&mut diff, &dk, w, h);
                    for (i, _) in &filled {
                        for c in 0..3 {
                            img.data[*i][c] += o.lighting * diff[*i][c];
                        }
                    }
                }
            }
            if let Err(e) = store.put(t, img) {
                store.record_error(e);
            }
            HoleMask::from_vec(&left)
        })
        .collect();
    if input.frames.has_error() || store.has_error() {
        return None;
    }
    drop(input);
    if !progress(2, total) {
        return None;
    }
    // 3. Synthesis of what no frame shows.
    let mut out: Vec<Option<Image>> = vec![None; n];
    let first = (0..n).find(|&t| propagated_left[t].any());
    let Some(t0) = first else {
        if streaming {
            for t in 0..n {
                let img = opaque_fill(take_checked(&store, t)?, &holes[t].to_vec());
                if !stream_frame(&mut sink, t, Some(&img), &holes) {
                    return None;
                }
            }
            return Some(Vec::new());
        }
        let out: Vec<Image> = (0..n).map(|t| opaque_fill(store.take(t), &holes[t].to_vec())).collect();
        if store.has_error() {
            return None;
        }
        return Some(out);
    };
    // Frames before t0 are complete already.
    for t in 0..t0 {
        let img = take_checked(&store, t)?;
        if streaming {
            if !stream_frame(&mut sink, t, Some(&img), &holes) {
                return None;
            }
        } else {
            out[t] = Some(img);
        }
    }
    let mut done = 2;
    let left0 = propagated_left[t0].to_vec();
    out[t0] = Some(backend.patch(&take_checked(&store, t0)?, &left0, None, o.patch, o.seed));
    if !stream_frame(&mut sink, t0, out[t0].as_ref(), &holes) {
        return None;
    }
    done += 1;
    if !progress(done, total) {
        return None;
    }
    for t in t0 + 1..n {
        let complete = !propagated_left[t].any();
        if complete {
            out[t] = Some(take_checked(&store, t)?);
        } else {
            let img = take_checked(&store, t)?;
            let left = propagated_left[t].to_vec();
            // Warp the previous result here along the backward flow (t → t − 1).
            // Frames after `t0` are filled in order, so the previous one is set.
            let Some(prev) = out[t - 1].as_ref() else {
                out[t] = Some(backend.patch(&img, &left, None, o.patch, o.seed));
                if !stream_frame(&mut sink, t, out[t].as_ref(), &holes) {
                    return None;
                }
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
                _ => backend.patch(&img, &left, Some(&warped), o.patch, o.seed),
            });
        }
        // Only the streaming sink has consumed the frame; a non-streaming result returns it.
        if streaming && t >= 2 {
            out[t - 2] = None;
        }
        if !stream_frame(&mut sink, t, out[t].as_ref(), &holes) {
            return None;
        }
        done += 1;
        if !progress(done, total) {
            return None;
        }
    }
    if store.has_error() {
        return None;
    }
    if streaming {
        for slot in out.iter_mut() {
            *slot = None;
        }
        return Some(Vec::new());
    }
    Some(out.into_iter().enumerate().map(|(i, img)| opaque_fill(img.unwrap_or_default(), &holes[i].to_vec())).collect())
}

/// Take frame `k` from the store, or `None` when the spill read failed. A failed read yields a
/// black frame, which must never be emitted (#2570 review).
fn take_checked(store: &PropStore, k: usize) -> Option<Image> {
    let img = store.take(k);
    (!store.has_error()).then_some(img)
}

/// Hand frame `t` to the streaming sink with its filled pixels made opaque. `true` when there is
/// no sink or it accepted the frame.
fn stream_frame(sink: &mut Option<&mut dyn FnMut(usize, &Image) -> Result<(), String>>, t: usize, img: Option<&Image>, holes: &[HoleMask]) -> bool {
    let (Some(cb), Some(img)) = (sink.as_mut(), img) else {
        return true;
    };
    cb(t, &opaque_fill(img.clone(), &holes[t].to_vec())).is_ok()
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

    /// Packing and unpacking a hole mask round-trips every pixel.
    #[test]
    fn holemask_roundtrip() {
        let v: Vec<bool> = (0..1000).map(|i| i % 3 == 0).collect();
        let m = HoleMask::from_vec(&v);
        assert_eq!(m.to_vec(), v);
        for i in 0..v.len() {
            assert_eq!(m.get(i), v[i]);
        }
    }

    /// Packing round-trips where the last word is full, holds one bit or is one bit short, and
    /// `set` builds the same mask; a mask whose public `words` are shorter than `len` reads and
    /// writes without panicking (#2570 review).
    #[test]
    fn holemask_roundtrips_at_word_boundaries() {
        for len in [1usize, 63, 64, 65, 127, 128, 129, 191, 192] {
            let v: Vec<bool> = (0..len).map(|i| i % 3 == 0 || i == len - 1).collect();
            let m = HoleMask::from_vec(&v);
            assert_eq!(m.words.len(), len.div_ceil(64), "len {len}");
            assert_eq!(m.to_vec(), v, "len {len}");
            let mut s = HoleMask::zeros(len);
            for (i, &h) in v.iter().enumerate() {
                s.set(i, h);
            }
            assert_eq!(s, m, "len {len}: set builds the packed mask");
            for i in 0..len {
                s.set(i, false);
            }
            assert!(!s.any(), "len {len}: every pixel cleared");
            // A pixel past the end is ignored.
            s.set(len, true);
            assert!(!s.get(len), "len {len}");
            assert!(!s.any(), "len {len}");
        }
        let mut short = HoleMask { words: vec![], len: 100 };
        assert!(!short.get(70));
        short.set(70, true);
        assert!(!short.any());
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
            holes.push(HoleMask::from_vec(&hole));
            gts.push(gt);
        }
        (FillInput { frames: Frames::Memory(frames), holes, references: vec![] }, gts)
    }

    #[test]
    fn object_fill_recovers_background_and_is_stable() {
        for pan in [0.0, 1.0] {
            let (input, gts) = sequence(8, pan);
            let out = fill_sequence(&input, &FillOpts::default(), &mut |_, _| true).expect("fill");
            for t in 0..out.len() {
                let db = psnr(&out[t], &gts[t], Some(&input.holes[t].to_vec()));
                assert!(db > 26.0, "frame {t} pan {pan}: {db:.1} dB");
                assert_eq!(out[t].data[0][3], 1.0);
            }
            if pan == 0.0 {
                // Temporal consistency: on a static background the filled pixels agree between
                // frames.
                for t in 1..out.len() {
                    let both: Vec<bool> = input.holes[t].to_vec().iter().zip(input.holes[t - 1].to_vec().iter()).map(|(a, b)| *a || *b).collect();
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
        let input = FillInput { frames: Frames::Memory(vec![cut(&gt, &hole); 4]), holes: vec![HoleMask::from_vec(&hole); 4], references: vec![] };
        let surf = fill_sequence(&input, &FillOpts { method: FillMethod::Surface, ..Default::default() }, &mut |_, _| true).expect("fill");
        for t in 1..4 {
            // Surface carries one fill through: identical frames.
            assert!(psnr(&surf[t], &surf[0], Some(&hole)) > 40.0);
        }
        let edge = fill_sequence(&input, &FillOpts { method: FillMethod::EdgeBlend, ..Default::default() }, &mut |_, _| true).expect("fill");
        assert!(psnr(&edge[0], &membrane_fill(&input.frames.get(0), &hole), Some(&hole)) > 60.0);
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
        let mut frames = match std::mem::replace(&mut input.frames, Frames::Memory(vec![])) {
            Frames::Memory(v) => v,
            _ => vec![],
        };
        for t in 0..6 {
            let k = 1.0 + 0.06 * t as f32;
            for img in [&mut frames[t], &mut gts[t]] {
                for p in &mut img.data {
                    for c in 0..3 {
                        p[c] *= k;
                    }
                }
            }
        }
        input.frames = Frames::Memory(frames);
        let plain = fill_sequence(&input, &FillOpts::default(), &mut |_, _| true).expect("fill");
        let lit = fill_sequence(&input, &FillOpts { lighting: 1.0, ..Default::default() }, &mut |_, _| true).expect("fill");
        let score = |o: &[Image]| (0..6).map(|t| psnr(&o[t], &gts[t], Some(&input.holes[t].to_vec()))).sum::<f64>();
        assert!(score(&lit) > score(&plain), "lit {} vs plain {}", score(&lit), score(&plain));
    }

    /// `SpillHandle::sample` reads only the 2x2 support; it must still agree with sampling the
    /// decoded frame, including the edges and coordinates outside the image (#2570 review).
    #[test]
    fn spilled_samples_match_the_decoded_frame() {
        let (w, h, n) = (7u32, 5u32, 3usize);
        for bpc in [8u8, 32] {
            let mut frames = Vec::new();
            for k in 0..n {
                let mut img = Image::new(w, h);
                for (i, p) in img.data.iter_mut().enumerate() {
                    *p = [(i % 7) as f32 / 7.0, (k * 3 + i % 5) as f32 / 5.0, 0.25, 1.0];
                }
                frames.push(img);
            }
            let spill = SpillHandle::create(w as usize, h as usize, n, bpc).expect("spill");
            for (k, img) in frames.iter().enumerate() {
                spill.write_frame(k, img).expect("write");
            }
            let spilled = Frames::Spilled(spill.clone());
            assert!(!spilled.has_error());
            for k in 0..n {
                let decoded = spill.get(k);
                for &(x, y) in &[(0.0, 0.0), (0.5, 0.5), (3.4, 2.6), (6.5, 4.5), (7.9, 5.9), (-1.0, -1.0), (2.25, 3.75)] {
                    let a = spilled.sample(k, x, y);
                    let b = decoded.sample_bilinear_clamped(x, y);
                    for c in 0..4 {
                        assert!((a[c] - b[c]).abs() <= 1e-6, "bpc {bpc} frame {k} at ({x}, {y}) channel {c}: {} vs {}", a[c], b[c]);
                    }
                }
            }
            assert!(!spilled.has_error(), "sampling a valid spill records no error");
        }
    }

    /// The input's error slot is shared with the membrane and propagation spills, so one probe
    /// reports any scratch failure (#2570 review).
    #[test]
    fn a_secondary_spill_reports_through_the_input_error_slot() {
        let input = SpillHandle::create(4, 4, 2, 32).expect("input spill");
        let prop = SpillHandle::create_in_with_error(None, 4, 4, 2, 32, Some(input.error_cell())).expect("prop spill");
        prop.record_error("spill: No space left on device".into());
        assert!(input.has_error());
        assert_eq!(input.take_error().as_deref(), Some("spill: No space left on device"));
        assert!(!prop.has_error(), "take clears the shared slot");
    }

    /// A spill whose scratch folder cannot be created is an error, not a silent fallback.
    #[test]
    fn a_spill_that_cannot_create_its_folder_is_an_error() {
        let dir = tempfile::tempdir().expect("temp");
        let file = dir.path().join("not-a-folder");
        std::fs::write(&file, b"x").expect("write");
        assert!(SpillHandle::create_in(Some(&file), 4, 4, 1, 32).is_err());
    }

    /// The scratch file has no name in its folder, so a fill that is killed partway through
    /// leaves nothing behind, and every read and write still reaches the frames (#2570 review).
    #[test]
    fn a_spill_leaves_no_file_in_its_folder() {
        let dir = tempfile::tempdir().expect("temp");
        let spill = SpillHandle::create_in(Some(dir.path()), 4, 3, 2, 32).expect("spill");
        let mut img = Image::new(4, 3);
        for (i, p) in img.data.iter_mut().enumerate() {
            *p = [i as f32 / 12.0, 0.5, 0.25, 1.0];
        }
        spill.write_frame(1, &img).expect("write");
        assert_eq!(spill.get(1).data, img.data);
        assert!(!spill.has_error());
        assert_eq!(spill.dir(), Some(dir.path()));
        assert_eq!(Frames::Spilled(spill.clone()).scratch_dir().as_deref(), Some(dir.path()));
        // Elsewhere the file is unlinked as soon as it is made; Windows keeps a delete-on-close
        // file listed until its last handle closes.
        #[cfg(not(windows))]
        assert_eq!(std::fs::read_dir(dir.path()).expect("list").count(), 0, "the open spill has a name in the folder");
        drop(spill);
        assert_eq!(std::fs::read_dir(dir.path()).expect("list").count(), 0, "scratch left in the folder");
    }

    /// The same frames spilled to 32 bpc scratch.
    fn spill_frames(frames: &Frames) -> SpillHandle {
        let (w, h) = frames.size().expect("size");
        let spill = SpillHandle::create(w, h, frames.len(), 32).expect("spill");
        for k in 0..frames.len() {
            spill.write_frame(k, &frames.get(k)).expect("write");
        }
        spill
    }

    /// Every channel of every pixel, as bits.
    fn bits(img: &Image) -> Vec<u32> {
        img.data.iter().flatten().map(|v| v.to_bits()).collect()
    }

    /// A spilled range fills bit-identically to the same frames in RAM, streamed or returned: the
    /// spill keeps 32-bit floats and samples the same 2x2 support (#2570, "no quality loss").
    #[test]
    fn a_spilled_range_fills_exactly_like_one_in_memory() {
        let (input, _) = sequence(6, 1.0);
        for method in [FillMethod::Object, FillMethod::Surface, FillMethod::EdgeBlend] {
            let opts = FillOpts { method, lighting: 0.5, ..Default::default() };
            let memory = fill_sequence(&input, &opts, &mut |_, _| true).expect("memory fill");
            let spilled = FillInput { frames: Frames::Spilled(spill_frames(&input.frames)), holes: input.holes.clone(), references: vec![] };
            let returned = fill_sequence(&spilled, &opts, &mut |_, _| true).expect("spilled fill");
            let mut streamed = Vec::new();
            let mut sink = |_: usize, img: &Image| -> Result<(), String> {
                streamed.push(img.clone());
                Ok(())
            };
            let rest = fill_sequence_with(spilled, &opts, &CpuFill, &mut |_, _| true, Some(&mut sink));
            assert!(rest.is_some_and(|v| v.is_empty()), "{method:?}: the sink took every frame");
            assert_eq!((memory.len(), returned.len(), streamed.len()), (6, 6, 6), "{method:?}");
            for t in 0..6 {
                assert!(bits(&returned[t]) == bits(&memory[t]), "{method:?} frame {t}: spilled fill differs from RAM");
                assert!(bits(&streamed[t]) == bits(&memory[t]), "{method:?} frame {t}: streamed fill differs from RAM");
            }
        }
    }

    /// The sink sees every frame once and strictly in order, whichever stage finishes it: the
    /// complete frames before the first synthesised one, the synthesised run, and Edge Blend.
    #[test]
    fn the_sink_sees_frames_in_order() {
        let (input, gts) = sequence(8, 1.0);
        let mut frames: Vec<Image> = (0..8).map(|k| input.frames.get(k)).collect();
        let mut holes = input.holes.clone();
        // The first two frames are clean, so they stream ahead of the first synthesised frame.
        for k in 0..2 {
            frames[k] = gts[k].clone();
            holes[k] = HoleMask::zeros(frames[k].data.len());
        }
        let input = FillInput { frames: Frames::Spilled(spill_frames(&Frames::Memory(frames))), holes, references: vec![] };
        for method in [FillMethod::Object, FillMethod::Surface, FillMethod::EdgeBlend] {
            let mut seen = Vec::new();
            let mut sink = |t: usize, _: &Image| -> Result<(), String> {
                seen.push(t);
                Ok(())
            };
            let out = fill_sequence_with(input.clone(), &FillOpts { method, ..Default::default() }, &CpuFill, &mut |_, _| true, Some(&mut sink));
            assert!(out.is_some(), "{method:?}");
            assert_eq!(seen, (0..8).collect::<Vec<_>>(), "{method:?}");
        }
    }

    /// A spill that cannot be read fails the fill with the disk error in the slot the caller
    /// probes, and the black fallback frame never reaches the sink (#2570 review).
    #[test]
    fn a_spill_read_failure_is_an_error_and_never_streamed() {
        let (input, _) = sequence(4, 1.0);
        let spill = spill_frames(&input.frames);
        // Truncate the scratch under the fill: every read comes back short.
        spill.inner.file.set_len(0).expect("truncate");
        for method in [FillMethod::Object, FillMethod::Surface, FillMethod::EdgeBlend] {
            let mut seen = Vec::new();
            let mut sink = |t: usize, _: &Image| -> Result<(), String> {
                seen.push(t);
                Ok(())
            };
            let failed = FillInput { frames: Frames::Spilled(spill.clone()), holes: input.holes.clone(), references: vec![] };
            let out = fill_sequence_with(failed, &FillOpts { method, ..Default::default() }, &CpuFill, &mut |_, _| true, Some(&mut sink));
            assert!(out.is_none(), "{method:?}");
            assert!(seen.is_empty(), "{method:?}: streamed frames {seen:?} from an unreadable spill");
            let e = spill.take_error().expect("the read failure is recorded");
            assert!(e.starts_with("spill:"), "{method:?}: {e}");
        }
    }

    /// A scratch failure while the output streams stops the fill before the next frame reaches
    /// the sink, and leaves the failure for the caller to report.
    #[test]
    fn a_spill_failure_mid_stream_stops_before_the_next_frame() {
        let (input, _) = sequence(6, 1.0);
        let spill = spill_frames(&input.frames);
        let cell = spill.error_cell();
        let mut seen = Vec::new();
        let mut sink = |t: usize, _: &Image| -> Result<(), String> {
            seen.push(t);
            if t == 2 {
                *cell.lock().unwrap() = Some("spill: injected".into());
            }
            Ok(())
        };
        let streamed = FillInput { frames: Frames::Spilled(spill.clone()), holes: input.holes.clone(), references: vec![] };
        let out = fill_sequence_with(streamed, &FillOpts::default(), &CpuFill, &mut |_, _| true, Some(&mut sink));
        assert!(out.is_none());
        assert_eq!(seen, vec![0, 1, 2]);
        assert_eq!(spill.take_error().as_deref(), Some("spill: injected"));
    }

    /// Once a propagated frame fails to spill, the other propagation workers stop instead of
    /// tracing the rest of a range whose fill is already lost (#2570 review).
    #[test]
    fn propagation_stops_after_a_spill_failure() {
        /// Fails the spill from inside the propagation stage (Lighting Correction's membrane).
        struct FailInPropagation {
            cell: std::sync::Arc<std::sync::Mutex<Option<String>>>,
            calls: std::sync::atomic::AtomicUsize,
        }
        impl FillBackend for FailInPropagation {
            fn membrane_known(&self, data: &mut [[f32; 4]], known: &[bool], w: usize, h: usize) {
                self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                *self.cell.lock().unwrap() = Some("spill: injected".into());
                membrane_core(data, known, w, h);
            }
        }
        let (input, _) = sequence(6, 1.0);
        let spill = spill_frames(&input.frames);
        let backend = FailInPropagation { cell: spill.error_cell(), calls: std::sync::atomic::AtomicUsize::new(0) };
        let input = FillInput { frames: Frames::Spilled(spill.clone()), holes: input.holes, references: vec![] };
        let opts = FillOpts { lighting: 1.0, ..Default::default() };
        // One worker, so the frames run one after another and the count is exact.
        let pool = rayon::ThreadPoolBuilder::new().num_threads(1).build().unwrap();
        let out = pool.install(|| fill_sequence_with(input, &opts, &backend, &mut |_, _| true, None));
        assert!(out.is_none());
        assert_eq!(backend.calls.load(std::sync::atomic::Ordering::SeqCst), 1, "propagation kept going after the failure");
        assert_eq!(spill.take_error().as_deref(), Some("spill: injected"));
    }

    /// Peak RSS of one synthetic fill, quoted in the PR. Ignored so CI does not run it. Run it
    /// under `/usr/bin/time -v` and read `VmHWM` (`Maximum resident set size`):
    ///
    /// ```text
    /// cargo test -p effectcraft-raster --lib measure_peak_rss --no-run
    /// target/debug/deps/effectcraft_raster-* inpaint::tests::measure_peak_rss_of_a_fill \
    ///     --ignored --exact --nocapture --test-threads=1
    /// ```
    ///
    /// `RAYON_NUM_THREADS`, `MEASURE_LIGHTING=1` and `MEASURE_FRAMES` select the configuration.
    /// Measured on 18 workers (2026-11-04): 90 frames of 640×360 peak at 340–470 MB across runs
    /// (35 MB with one worker) and 600 frames at 582 MB, i.e. ~7 frames of scratch a worker, so
    /// the guard budgets 8 (9 with Lighting Correction) (#2570).
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    #[ignore = "memory measurement; run explicitly"]
    fn measure_peak_rss_of_a_fill() {
        let n: usize = std::env::var("MEASURE_FRAMES").ok().and_then(|v| v.parse().ok()).unwrap_or(90);
        let (w, h) = (640u32, 360u32);
        let spill = SpillHandle::create_in(None, w as usize, h as usize, n, 32).expect("spill");
        let hole = rect_hole(w, h, 240, 120, 160);
        let mut holes = Vec::with_capacity(n);
        for k in 0..n {
            let img = background(w, h, 2.0 * k as f32);
            spill.write_frame(k, &img).expect("write");
            holes.push(HoleMask::from_vec(&hole));
        }
        let input = FillInput { frames: Frames::Spilled(spill), holes, references: vec![] };
        // MEASURE_LIGHTING=1 exercises the propagation ring too (one process per reading: VmHWM
        // never goes down).
        let lighting = std::env::var("MEASURE_LIGHTING").is_ok_and(|v| v == "1");
        let opts = FillOpts { method: FillMethod::Surface, lighting: if lighting { 1.0 } else { 0.0 }, ..Default::default() };
        let before = peak_rss_kb();
        let started = std::time::Instant::now();
        let mut sink = |_: usize, _: &Image| -> Result<(), String> { Ok(()) };
        let out = fill_sequence_with(input, &opts, &CpuFill, &mut |_, _| true, Some(&mut sink));
        eprintln!(
            "measure_peak_rss: {n} frames of {w}x{h}, surface, lighting={}, {:.2}s, peak RSS {} KB -> {} KB",
            if lighting { "on" } else { "off" },
            started.elapsed().as_secs_f64(),
            before.unwrap_or(0),
            peak_rss_kb().unwrap_or(0)
        );
        assert!(out.is_some_and(|v| v.is_empty()), "the sink consumed every frame");
    }

    /// Peak resident set size of this process (`VmHWM`), in kilobytes.
    #[cfg(target_os = "linux")]
    fn peak_rss_kb() -> Option<u64> {
        std::fs::read_to_string("/proc/self/status").ok()?.lines().find_map(|l| {
            let v = l.strip_prefix("VmHWM:")?;
            v.split_whitespace().next()?.parse().ok()
        })
    }

    /// Peak resident set size is only measured on Linux.
    #[cfg(not(target_os = "linux"))]
    fn peak_rss_kb() -> Option<u64> {
        None
    }
}

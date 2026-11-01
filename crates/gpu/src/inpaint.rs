//! Content-Aware Fill's per-frame synthesis and flow on the GPU: PatchMatch's multi-scale EM
//! loop, propagation, random search and vote of `effectcraft_raster::inpaint`, and the block
//! matching of `effectcraft_raster::flow::block_flow`, as compute kernels following the CPU's
//! steps (`shaders/inpaint.wgsl`).
//!
//! The fill's input frames are composited on the CPU by design (analyses do not use the job
//! worker's GPU: `effectcraft_engine::background::offload::job_accel`), so this module is about the fill
//! itself — the part that runs for minutes on a large mask, and whose parallel search is what a
//! device wins on. The membrane (harmonic) solve, the flow's hole completion, the orchestration
//! ([`effectcraft_raster::inpaint::fill_sequence_with`]), the flow-guided propagation and the
//! per-frame flow control stay on the CPU: the membrane is bandwidth-bound (see
//! [`FillBackend::patch`]), and the propagation's coupling is sequential, needing every frame of
//! the range resident at once.
//!
//! Parity with the CPU reference is a quality contract, not bit-exactness: the CPU's
//! scan-order propagation becomes a two-pass checkerboard, and its in-place relaxation becomes
//! Jacobi reads. Both solve the same problem to the same quality (`tests/inpaint.rs`).

use effectcraft_raster::Image;
use effectcraft_raster::flow::{Flow, block_flow};
use effectcraft_raster::inpaint::{FillBackend, complete_flow, dilate};

use crate::Gpu;
use crate::context::{Enc, GpuContext, GpuImage, KernelImages, Params};

/// The kernels this module adds to the shared pipeline registry (`context.rs`).
pub(crate) const KERNELS: &[&str] = &[
    "imp_down",
    "imp_up",
    "imp_jacobi",
    "imp_pm_down",
    "imp_pm_up",
    "imp_nnf_up",
    "imp_pm_init",
    "imp_pm_prop",
    "imp_pm_rand",
    "imp_nnf_store",
    "imp_pm_vote",
    "imp_hist",
    "imp_fl_half",
    "imp_fl_search",
    "imp_fl_subpix",
    "imp_fl_median",
];

/// Distance histogram bins (`imp_hist`), over distances in 0…4.
const BINS: usize = 1024;

/// `u`/`f` blocks of a kernel's parameter buffer (see `inpaint.wgsl`'s header).
fn params(u0: [u32; 4], u1: [u32; 4], u2: [u32; 4], f0: [f32; 4]) -> Params {
    Params { u: [u0, u1, u2, [0; 4]], f: [f0, [0.0; 4], [0.0; 4], [0.0; 4], [0.0; 4], [0.0; 4], [0.0; 4], [0.0; 4], [0.0; 4], [0.0; 4], [0.0; 4], [0.0; 4]] }
}

/// `p` with this level's mask offset in `u[3].x` (`inpaint.wgsl`'s `data`).
fn with_off(p: Params, off: u32) -> Params {
    Params { u: [p.u[0], p.u[1], p.u[2], [off, 0, 0, 0]], f: p.f }
}

/// A relaxation sweep count of `membrane_core`. The CPU uses 200 for the small levels and 40 for
/// the large ones; this uses 64 and 40. The small levels are where the difference lies: Jacobi
/// needs O(n²) sweeps to converge on an n-wide grid, so at the levels the coarse-to-fine schedule
/// actually reaches (n ≤ 16, where n² ≤ 256) 64 sweeps is already converged, and finer levels
/// start from a good guess. Matching the CPU's 200 measured ≈ 6% slower on the whole fill with no
/// change in the result to two decimals (67.0 dB either way), so the schedule is deliberately
/// not the CPU's — Jacobi and Gauss–Seidel do not converge alike, and the parity contract is
/// quality, not bit-exactness.
fn sweep_count(w: usize, h: usize) -> usize {
    if w * h <= 4096 { 64 } else { 40 }
}

/// PatchMatch's patch radius at a level (`pm_level`).
fn radius(patch: usize, w: u32, h: u32) -> u32 {
    (patch / 2).min(w.min(h).saturating_sub(1) as usize / 2).max(1) as u32
}

/// The bounding box of `mask`'s set pixels, `[x0, y0, x1, y1)`, or `None` when it is empty.
fn bbox(mask: &[bool], w: usize, h: usize) -> Option<[u32; 4]> {
    let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0usize, 0usize);
    for y in 0..h {
        for x in 0..w {
            if mask.get(y * w + x).copied().unwrap_or(false) {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x + 1);
                y1 = y1.max(y + 1);
            }
        }
    }
    if x0 == usize::MAX { None } else { Some([x0 as u32, y0 as u32, x1 as u32, y1 as u32]) }
}

/// [`bbox`] grown by `n` pixels, clipped to the image.
fn grow(b: [u32; 4], n: u32, w: u32, h: u32) -> [u32; 4] {
    [b[0].saturating_sub(n), b[1].saturating_sub(n), b[2].saturating_add(n).min(w), b[3].saturating_add(n).min(h)]
}

/// The bounding box of the pixels a membrane level still has to solve (`known` false), or
/// `None` when every pixel is known.
fn unknown_bbox(known: &[f32], w: usize, h: usize) -> Option<[u32; 4]> {
    let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0usize, 0usize);
    for y in 0..h {
        for x in 0..w {
            if known.get(y * w + x).copied().unwrap_or(1.0) <= 0.5 {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x + 1);
                y1 = y1.max(y + 1);
            }
        }
    }
    if x0 == usize::MAX { None } else { Some([x0 as u32, y0 as u32, x1 as u32, y1 as u32]) }
}

/// Dispatch a kernel over `bb`. The box's origin is written into `P.u[1].xy` (the kernels place
/// their other `u[1]` fields in `.zw`): deriving it from the box the dispatch is sized from keeps
/// a dispatch's covered region and the origin its shader reads from drifting apart, which is the
/// subtle way an edge of the region goes unwritten.
#[allow(clippy::too_many_arguments)]
fn over(e: &mut Enc, entry: &str, p: &Params, src: &GpuImage, aux: Option<&GpuImage>, out: &GpuImage, data: Option<&wgpu::Buffer>, bb: [u32; 4]) {
    let p = Params { u: [p.u[0], [bb[0], bb[1], p.u[1][2], p.u[1][3]], p.u[2], p.u[3]], f: p.f };
    let groups = ((bb[2] - bb[0]).div_ceil(16), (bb[3] - bb[1]).div_ceil(16));
    e.dispatch(entry, &p, KernelImages { src, aux, out }, data, groups);
}

// ------------------------------------------------------------------ membrane

/// Relax `fine`'s unknown pixels — the harmonic interpolation of their border — with the CPU's
/// coarse-to-fine schedule (`membrane_core`). `spare` is a free texture of the same size; the
/// result is returned (it may be `fine`, `spare` or one of the coarse levels).
fn membrane_tex(e: &mut Enc, fine: &GpuImage, spare: &GpuImage, hole: &[bool], w: u32, h: u32) -> Option<GpuImage> {
    let (uw, uh) = (w as usize, h as usize);
    let n = uw * uh;
    if hole.len() < n || !hole.iter().take(n).any(|b| *b) || !hole.iter().take(n).any(|b| !*b) {
        return Some(fine.clone());
    }
    // The recursion's levels: halve while the level is wider than 8.
    let mut dims = vec![(uw, uh)];
    while let Some(&(cw, ch)) = dims.last() {
        if !(cw > 8 && ch > 8) || dims.len() >= 32 {
            break;
        }
        dims.push((cw.div_ceil(2), ch.div_ceil(2)));
    }
    // The known mask of every level: a coarse pixel is known where any of its 2×2 is.
    let mut knowns: Vec<Vec<f32>> = Vec::with_capacity(dims.len());
    knowns.push(hole.iter().take(n).map(|b| if *b { 0.0 } else { 1.0 }).collect());
    for k in 1..dims.len() {
        let (pw, ph) = dims[k - 1];
        let (cw, ch) = dims[k];
        let prev = &knowns[k - 1];
        let mut cur = vec![0.0f32; cw * ch];
        for y in 0..ch {
            for x in 0..cw {
                for (dx, dy) in [(0usize, 0usize), (1, 0), (0, 1), (1, 1)] {
                    let (fx, fy) = (2 * x + dx, 2 * y + dy);
                    if fx < pw && fy < ph && prev[fy * pw + fx] > 0.5 {
                        cur[y * cw + x] = 1.0;
                        break;
                    }
                }
            }
        }
        knowns.push(cur);
    }
    // One buffer for every level's mask (a buffer per level costs a device allocation each).
    let mut packed: Vec<f32> = Vec::with_capacity(knowns.iter().map(|k| k.len()).sum());
    let mut offs: Vec<u32> = Vec::with_capacity(knowns.len());
    for k in &knowns {
        offs.push(packed.len() as u32);
        packed.extend_from_slice(k);
    }
    let datas = e.data(&packed);
    let moff = |k: usize| offs.get(k).copied().unwrap_or(0);
    // The bounds of each level's unknown pixels: the relaxation only touches them (known pixels
    // never move), which keeps a fill's cost proportional to the hole, not the frame.
    let mut uns: Vec<Option<[u32; 4]>> = Vec::with_capacity(dims.len());
    for (k, &(cw, ch)) in dims.iter().enumerate() {
        uns.push(unknown_bbox(knowns.get(k)?, cw, ch));
    }
    // Two textures per coarse level, for the ping-pong sweeps.
    let mut lv: Vec<GpuImage> = Vec::with_capacity(2 * dims.len());
    for k in 1..dims.len() {
        let (cw, ch) = dims[k];
        lv.push(e.scratch(cw as u32, ch as u32));
        lv.push(e.scratch(cw as u32, ch as u32));
    }
    // Fine to coarse.
    for k in 1..dims.len() {
        let (pw, ph) = dims[k - 1];
        let (cw, ch) = dims[k];
        let p = with_off(params([pw as u32, ph as u32, cw as u32, ch as u32], [0; 4], [0; 4], [0.0; 4]), moff(k - 1));
        let src = if k == 1 { fine } else { lv.get(2 * (k - 2))? };
        e.pixels("imp_down", &p, src, None, lv.get(2 * (k - 1))?, Some(&datas));
    }
    // Relax, coarse to fine. `first[k]` says whether level k's image is the first of its pair
    // (level 0's pair is `fine`/`spare`).
    let top = dims.len() - 1;
    let mut first = vec![true; dims.len()];
    if top >= 1 {
        let (a, b) = (lv.get(2 * (top - 1))?, lv.get(2 * (top - 1) + 1)?);
        // Both textures must hold the level before the sweeps: they now write only the hole.
        e.copy_into(a, b, 0, 0);
        first[top] = sweeps(e, &datas, moff(top), a, b, dims[top].0 as u32, dims[top].1 as u32, uns.get(top).copied().flatten());
    }
    for k in (1..top).rev() {
        let (cw, ch) = dims[k];
        let (pw, ph) = dims[k + 1];
        let coarse = lv.get(2 * k + usize::from(!first[k + 1]))?;
        let (fa, fb) = (lv.get(2 * (k - 1))?, lv.get(2 * (k - 1) + 1)?);
        e.copy_into(fa, fb, 0, 0);
        if let Some(bb) = uns.get(k).copied().flatten() {
            over(
                e,
                "imp_up",
                &with_off(params([cw as u32, ch as u32, pw as u32, ph as u32], [0; 4], [0; 4], [0.0; 4]), moff(k)),
                coarse,
                None,
                fb,
                Some(&datas),
                bb,
            );
        }
        // `imp_up` wrote into `fb`, the pair's second.
        first[k] = !sweeps(e, &datas, moff(k), fb, fa, cw as u32, ch as u32, uns.get(k).copied().flatten());
    }
    if top >= 1 {
        let coarse = lv.get(usize::from(!first[1]))?;
        e.copy_into(fine, spare, 0, 0);
        let bb = uns.first().copied().flatten()?;
        over(
            e,
            "imp_up",
            &with_off(params([w, h, dims[1].0 as u32, dims[1].1 as u32], [0; 4], [0; 4], [0.0; 4]), moff(0)),
            coarse,
            None,
            spare,
            Some(&datas),
            bb,
        );
        return Some(if sweeps(e, &datas, moff(0), spare, fine, w, h, Some(bb)) { spare.clone() } else { fine.clone() });
    }
    let bb = uns.first().copied().flatten()?;
    e.copy_into(fine, spare, 0, 0);
    Some(if sweeps(e, &datas, moff(0), fine, spare, w, h, Some(bb)) { fine.clone() } else { spare.clone() })
}

/// Jacobi relaxation sweeps of a texture pair, over the unknown pixels' bounds `bb` (the state
/// starts in `a`); returns whether the result is in `a`.
#[allow(clippy::too_many_arguments)]
fn sweeps(e: &mut Enc, data: &wgpu::Buffer, off: u32, a: &GpuImage, b: &GpuImage, w: u32, h: u32, bb: Option<[u32; 4]>) -> bool {
    let Some(bb) = bb else { return true };
    let p = with_off(params([w, h, 0, 0], [0; 4], [0; 4], [0.0; 4]), off);
    let mut in_a = true;
    for _ in 0..sweep_count(w as usize, h as usize) {
        let (src, out) = if in_a { (a, b) } else { (b, a) };
        over(e, "imp_jacobi", &p, src, None, out, Some(data), bb);
        in_a = !in_a;
    }
    in_a
}

/// `img` with the pixels `mask` marks inside `bb` replaced by `rect` — the tightly packed RGBA f32
/// rows of the rectangle that was read back. Pixels outside the mask stay the input's, bit for bit.
fn patch_back(img: &Image, mask: &[bool], bb: [u32; 4], rect: &[u8]) -> Image {
    let (w, rw) = (img.width as usize, (bb[2] - bb[0]) as usize);
    let mut out = img.clone();
    for y in bb[1]..bb[3] {
        for x in bb[0]..bb[2] {
            let i = y as usize * w + x as usize;
            if !mask.get(i).copied().unwrap_or(false) {
                continue;
            }
            let off = ((y - bb[1]) as usize * rw + (x - bb[0]) as usize) * 16;
            let Some(b) = rect.get(off..off + 16) else { continue };
            let mut px = [0.0f32; 4];
            for (c, v) in px.iter_mut().enumerate() {
                *v = f32::from_le_bytes([b[c * 4], b[c * 4 + 1], b[c * 4 + 2], b[c * 4 + 3]]);
            }
            out.data[i] = px;
        }
    }
    out
}

impl FillBackend for Gpu {
    /// PatchMatch synthesis — the fill's dominant cost, and the stage a device wins on: two to
    /// three times faster than the CPU's 18-rayon-thread loop at 1920×1080 with a 300×200 hole
    /// on an Intel Arc iGPU, and far more on a discrete one.
    ///
    /// The membrane ([`FillBackend::membrane`]) deliberately stays on the CPU: a harmonic solve
    /// over a hole is only a few tens of milliseconds, and on the same measurement it is *slower*
    /// on the device (≈ 90 ms against ≈ 55 ms). Most of that is the fill's full-frame data
    /// movement — uploading the frame, copying it and building the known pyramid — which does not
    /// shrink with the hole, while the relaxation itself does. The CPU's in-place Gauss–Seidel
    /// over many cores is already competitive there, so the device only takes the stage where its
    /// parallel search actually pays. (`membrane_tex` still runs on the device *inside* this
    /// stage, where it is the coarsest level's start and never crosses back to the CPU.)
    fn patch(&self, img: &Image, hole: &[bool], init: Option<&Image>, patch: usize, seed: u32) -> Image {
        if self.ctx.can_wait()
            && self.ctx.check_health().is_ok()
            && let Some(out) = patch_image(&self.ctx, img, hole, init, patch, seed)
        {
            return out;
        }
        effectcraft_raster::inpaint::patch_fill(img, hole, init, patch, seed)
    }

    /// The block matching that the sequence's flow-guided propagation starts from, which is the
    /// flow's dominant cost. The field is a small `cols × rows` grid (8 px tiles, so 240 × 135 at
    /// 1080p), and the search is texture-load-bound: at 1920×1080 the CPU's `block_flow` is
    /// ≈ 65–70 ms against ≈ 55 ms here, most of it the coarsest level's window (`radius 6` over
    /// up to six seeds) and the finest level's 8×8 blocks. That is a smaller margin than
    /// [`patch`](Self::patch), but the flow runs twice per frame pair, so on a four-frame 1080p
    /// sequence it is worth ≈ 0.15× of the fill's total (1.42× against 1.27× with the CPU flow).
    ///
    /// The field is exhaustive, not heuristic: it matches the CPU's to a float rounding, so the
    /// propagation sees the same motion. Both A's block per candidate (hoisted into registers)
    /// and the CPU's `Plane::luma` (computed here by [`effectcraft_raster::flow::luma_pair`] and
    /// uploaded, because a device may contract `a·b + c` into a multiply-add and round the luma
    /// differently) are why it is that close.
    ///
    /// The completion — marking the tiles that touch either hole and interpolating the surrounding
    /// motion over them — stays on the CPU ([`complete_flow`]). It is a membrane solve over that
    /// same tiny grid whose cost is the mask, not the solve, so there is little for the device to
    /// take; keeping it here also keeps the completed field the CPU's to within the search's
    /// rounding.
    fn flow(&self, a: &Image, b: &Image, hole_a: &[bool], hole_b: &[bool]) -> Flow {
        if self.ctx.can_wait()
            && self.ctx.check_health().is_ok()
            && let Some(f) = flow_image(&self.ctx, a, b)
        {
            return complete_flow(f, hole_a, hole_b, a.width as usize, a.height as usize);
        }
        complete_flow(block_flow(a, b, 8, 6), hole_a, hole_b, a.width as usize, a.height as usize)
    }
}

// ------------------------------------------------------------------ block flow

/// `effectcraft_raster::flow::block_flow(a, b, 8, 6)` on the device: the two frames' luma
/// pyramids, the coarse-to-fine search, the sub-pixel fit and the 3×3 median, returning the raw
/// field ([`complete_flow`] does the hole completion). `None` where the device cannot run it and
/// the caller falls back to the CPU.
pub(crate) fn flow_image(g: &GpuContext, a: &Image, b: &Image) -> Option<Flow> {
    let (w, h) = (a.width, a.height);
    if w == 0 || h == 0 || w != b.width || h != b.height || !g.fits(w, h) {
        return None;
    }
    let (tile, win, radius) = (8u32, 8u32, 6u32);
    let cols = (w as usize).div_ceil(tile as usize).max(1);
    let rows = (h as usize).div_ceil(tile as usize).max(1);
    // block_flow_window's early out: a frame too small to hold one block and window.
    if w < tile || h < tile || w < win || h < win {
        return Some(Flow { width: w, height: h, block: tile, cols, rows, v: vec![[0.0; 2]; cols * rows] });
    }
    let mut e = Enc::new(g);
    // The luma pyramid: `Plane::luma` (packed .x/.y), halved while both sides stay worth matching
    // (`Plane::half`), at most four levels.
    let mut dims = vec![(w, h)];
    while dims.len() < 4 {
        let (cw, ch) = *dims.last()?;
        if cw / 2 < 16 || ch / 2 < 16 {
            break;
        }
        dims.push(((cw / 2).max(1), (ch / 2).max(1)));
    }
    // The luma is the CPU's (`luma_pair`), so the field matches `block_flow`'s bit for bit; the
    // halves below are exact additions and a multiply by a power of two.
    let pair = g.upload_image(&effectcraft_raster::flow::luma_pair(a, b))?;
    let mut luma: Vec<GpuImage> = Vec::with_capacity(dims.len());
    luma.push(pair);
    for k in 1..dims.len() {
        let (pw, ph) = dims[k - 1];
        let (cw, ch) = dims[k];
        let dst = e.scratch(cw, ch);
        e.pixels("imp_fl_half", &params([pw, ph, cw, ch], [0; 4], [0; 4], [0.0; 4]), luma.get(k - 1)?, None, &dst, None);
        luma.push(dst);
    }
    // The coarse-to-fine search: the coarsest level from the zero vector, each finer level from
    // the doubled vectors of its tile and its four neighbours.
    let top = dims.len() - 1;
    let groups = (cols.div_ceil(16) as u32, rows.div_ceil(16) as u32);
    let mut field: Vec<GpuImage> = Vec::with_capacity(dims.len());
    for _ in 0..dims.len() {
        field.push(e.scratch(cols as u32, rows as u32));
    }
    for k in (0..dims.len()).rev() {
        let (pw, ph) = dims[k];
        let coarser = (k != top).then(|| field.get(k + 1)).flatten();
        let p = params([pw, ph, cols as u32, rows as u32], [tile, win, 1u32 << k, if k == top { radius } else { 1 }], [u32::from(k != top), 0, 0, 0], [0.0; 4]);
        e.dispatch("imp_fl_search", &p, KernelImages { src: luma.get(k)?, aux: coarser, out: field.get(k)? }, None, groups);
    }
    // The full-resolution sub-pixel fit, then the vector median.
    let fine = e.scratch(cols as u32, rows as u32);
    e.dispatch(
        "imp_fl_subpix",
        &params([w, h, cols as u32, rows as u32], [tile, win, 0, 0], [0; 4], [0.0; 4]),
        KernelImages { src: luma.first()?, aux: Some(field.first()?), out: &fine },
        None,
        groups,
    );
    let out = e.scratch(cols as u32, rows as u32);
    e.dispatch(
        "imp_fl_median",
        &params([w, h, cols as u32, rows as u32], [0; 4], [0; 4], [0.0; 4]),
        KernelImages { src: &fine, aux: None, out: &out },
        None,
        groups,
    );
    if e.take_missing() {
        return None;
    }
    // Only the field crosses back to the CPU: `cols × rows` entries.
    let rect = e.read_rect(&out.texture, 0, 0, cols as u32, rows as u32, 16)?;
    let mut v = Vec::with_capacity(cols * rows);
    for i in 0..cols * rows {
        let b = rect.get(i * 16..i * 16 + 16)?;
        let f = |o: usize| b.get(o).copied().unwrap_or(0);
        v.push([f32::from_le_bytes([f(0), f(1), f(2), f(3)]), f32::from_le_bytes([f(4), f(5), f(6), f(7)])]);
    }
    Some(Flow { width: w, height: h, block: tile, cols, rows, v })
}

// ------------------------------------------------------------------ PatchMatch

/// One PatchMatch level's plan.
struct Lv {
    w: u32,
    h: u32,
    r: u32,
    /// The target centres: pixels whose patch overlaps the hole (`near`).
    near: [u32; 4],
    /// The hole's own bounding box (the vote's dispatch).
    holes: [u32; 4],
    /// Where the nearest-neighbour field must be defined: the vote reads it `r` pixels out.
    field: [u32; 4],
    flags: wgpu::Buffer,
}

/// The multi-scale PatchMatch synthesis of `patch_fill`: `init` is the starting guess (the
/// previous frame's fill warped onto this one), which refines the finest level only.
pub(crate) fn patch_image(g: &GpuContext, img: &Image, hole: &[bool], init: Option<&Image>, patch: usize, seed: u32) -> Option<Image> {
    let (w, h) = (img.width, img.height);
    if w == 0 || h == 0 || !g.fits(w, h) {
        return None;
    }
    let (uw, uh) = (w as usize, h as usize);
    let n = uw * uh;
    if hole.len() < n || !hole.iter().take(n).any(|b| *b) {
        return Some(img.clone());
    }
    let hole: Vec<bool> = hole.iter().take(n).copied().collect();
    let patch = patch.max(3) | 1;
    let mut e = Enc::new(g);

    // A refinement of one frame (`patch_fill`'s `init`): the previous frame's fill seeds the
    // hole and only the finest level runs, from 2 EM rounds of 4 iterations.
    if let Some(init) = init {
        if init.width != w || init.height != h {
            return None;
        }
        let mut start = img.clone();
        for (i, v) in start.data.iter_mut().enumerate() {
            if hole.get(i).copied().unwrap_or(false) {
                *v = *init.data.get(i)?;
            }
        }
        let lv = plan_level(&mut e, &hole, w, h, patch)?;
        let image = g.upload_image(&start)?;
        let spare = e.scratch(w, h);
        let nnf = e.image(w, h);
        let (out, _) = pm_level(&mut e, &lv, seed, 2, 4, &image, &spare, &nnf)?;
        if e.take_missing() {
            return None;
        }
        let bb = bbox(&hole, uw, uh)?;
        let rect = e.read_rect(&out.texture, bb[0], bb[1], bb[2] - bb[0], bb[3] - bb[1], 16)?;
        return Some(patch_back(img, &hole, bb, &rect));
    }

    // The pyramid: halve while the level is still a few patches across.
    let mut dims = vec![(w, h)];
    let mut holes = vec![hole.clone()];
    while dims.len() < 6 {
        let &(lw, lh) = dims.last()?;
        let hl = holes.last()?;
        if lw.min(lh) / 2 < patch as u32 * 4 || !hl.iter().any(|b| *b) {
            break;
        }
        let (cw, ch) = (lw.div_ceil(2) as usize, lh.div_ceil(2) as usize);
        let mut nh = vec![false; cw * ch];
        for y in 0..ch {
            for x in 0..cw {
                for (dx, dy) in [(0usize, 0usize), (1, 0), (0, 1), (1, 1)] {
                    let (fx, fy) = ((2 * x + dx).min(lw as usize - 1), (2 * y + dy).min(lh as usize - 1));
                    if hl[fy * lw as usize + fx] {
                        nh[y * cw + x] = true;
                        break;
                    }
                }
            }
        }
        if !nh.iter().any(|b| !*b) {
            break;
        }
        dims.push((cw as u32, ch as u32));
        holes.push(nh);
    }
    let levels = dims.len();
    let top = levels - 1;

    let mut plan = Vec::with_capacity(levels);
    for k in 0..levels {
        plan.push(plan_level(&mut e, holes.get(k)?, dims[k].0, dims[k].1, patch)?);
    }

    // One image per level (the coarse-to-fine steps and the vote ping-pong in fresh textures).
    let mut ima: Vec<GpuImage> = Vec::with_capacity(levels);
    ima.push(g.upload_image(img)?);
    for k in 1..levels {
        let (cw, ch) = dims[k];
        ima.push(e.scratch(cw, ch));
    }
    for k in 1..levels {
        let (pw, ph) = dims[k - 1];
        let (cw, ch) = dims[k];
        e.pixels("imp_pm_down", &params([pw, ph, cw, ch], [0; 4], [0; 4], [0.0; 4]), &ima[k - 1], None, &ima[k], Some(&plan[k - 1].flags));
    }
    // The coarsest level starts as its membrane fill.
    let (cw, ch) = dims[top];
    let spare = e.scratch(cw, ch);
    ima[top] = membrane_tex(&mut e, &ima[top], &spare, &holes[top], cw, ch)?;

    // Coarse to fine: upsample, then the EM rounds of propagation and voting.
    let mut nnf: Option<GpuImage> = None;
    for k in (0..levels).rev() {
        let lv = &plan[k];
        nnf = Some(match nnf {
            Some(coarse) => {
                let (cw, ch) = dims[k + 1];
                let fine = e.scratch(lv.w, lv.h);
                e.pixels("imp_nnf_up", &params([lv.w, lv.h, cw, ch], [0; 4], [0; 4], [0.0; 4]), &coarse, None, &fine, None);
                fine
            }
            None => e.image(lv.w, lv.h),
        });
        if k < top {
            let (cw, ch) = dims[k + 1];
            let dst = e.scratch(lv.w, lv.h);
            e.copy_into(&ima[k], &dst, 0, 0);
            e.pixels("imp_pm_up", &params([lv.w, lv.h, cw, ch], [0; 4], [0; 4], [0.0; 4]), &ima[k + 1], None, &dst, Some(&lv.flags));
            ima[k] = dst;
        }
        let (em, iters) = if k == top { (8, 5) } else { (4, 5) };
        let spare = e.scratch(lv.w, lv.h);
        let (out, out_nnf) = pm_level(&mut e, lv, seed.wrapping_add(k as u32 * 101), em, iters, &ima[k], &spare, nnf.as_ref()?)?;
        ima[k] = out;
        nnf = Some(out_nnf);
    }
    if e.take_missing() {
        return None;
    }
    // Only the hole changes, so only the hole's bounds cross back to the CPU.
    let bb = bbox(&hole, uw, uh)?;
    let rect = e.read_rect(&ima[0].texture, bb[0], bb[1], bb[2] - bb[0], bb[3] - bb[1], 16)?;
    Some(patch_back(img, &hole, bb, &rect))
}

/// The plan of one level: its flags (`bit 0` hole, `bit 1` a target centre) and the dispatch
/// bounds of its targets, holes and field.
fn plan_level(e: &mut Enc, hole: &[bool], w: u32, h: u32, patch: usize) -> Option<Lv> {
    let (iw, ih) = (w as usize, h as usize);
    let r = radius(patch, w, h);
    let near = dilate(hole, iw, ih, r as usize);
    let near_bb = bbox(&near, iw, ih)?;
    let flags: Vec<f32> = (0..iw * ih).map(|i| (u32::from(hole[i]) + 2 * u32::from(near[i])) as f32).collect();
    Some(Lv { w, h, r, near: near_bb, holes: bbox(hole, iw, ih)?, field: grow(near_bb, r + 1, w, h), flags: e.data(&flags) })
}

/// `pm_level`: `em` EM rounds of propagation and voting at one level. `img` holds the level's
/// image and `img_spare` is a free texture of the same size; the nearest-neighbour field starts
/// at `nnf`. Returns the level's filled image and its field.
#[allow(clippy::too_many_arguments)]
fn pm_level(e: &mut Enc, lv: &Lv, seed: u32, em: usize, iters: usize, img: &GpuImage, img_spare: &GpuImage, nnf: &GpuImage) -> Option<(GpuImage, GpuImage)> {
    let (r, w, h) = (lv.r, lv.w, lv.h);
    let field_a = e.scratch(w, h);
    let field_b = e.scratch(w, h);
    let hist = e.scratch(BINS as u32, 1);
    let (img_a, img_b) = (img.clone(), img_spare.clone());
    let (mut field_nnf, mut nnf_spare) = (nnf.clone(), e.scratch(w, h));
    // The propagation and random search write the targets and the one-pixel ring around them, so
    // every neighbour they read has a defined alpha (`grow(near, 1)`). The dispatch origin must
    // be the ring's, not the targets': `over` sizes the dispatch from the box it is given.
    let ring = grow(lv.near, 1, w, h);
    let mut in_a = true;
    for round in 0..em {
        // The round's initial field: the inherited source where it is still valid, else random.
        // Every pixel of the field's bounds is written, so the previous round's field is free.
        let img = if in_a { &img_a } else { &img_b };
        over(e, "imp_pm_init", &params([w, h, r, 0], [0; 4], [seed, 0, 0, round as u32], [0.0; 4]), &field_nnf, Some(img), &field_a, Some(&lv.flags), lv.field);
        let mut f_in_a = true;
        for it in 0..iters {
            for parity in 0..2u32 {
                let (s, o) = if f_in_a { (&field_a, &field_b) } else { (&field_b, &field_a) };
                over(
                    e,
                    "imp_pm_prop",
                    &params([w, h, r, parity], [0; 4], [seed, it as u32, 0, round as u32], [0.0; 4]),
                    s,
                    Some(img),
                    o,
                    Some(&lv.flags),
                    ring,
                );
                f_in_a = !f_in_a;
            }
            let (s, o) = if f_in_a { (&field_a, &field_b) } else { (&field_b, &field_a) };
            over(e, "imp_pm_rand", &params([w, h, r, 0], [0; 4], [seed, it as u32, 0, round as u32], [0.0; 4]), s, Some(img), o, Some(&lv.flags), ring);
            f_in_a = !f_in_a;
        }
        let field = if f_in_a { &field_a } else { &field_b };
        // The vote's sigma²: the 75th percentile of the targets' match distances (`pm_level`).
        let sigma2 = sigma2(e, field, lv, &hist)?;
        let next_img = if in_a { &img_b } else { &img_a };
        e.copy_into(img, next_img, 0, 0);
        over(e, "imp_pm_vote", &params([w, h, r, 0], [0; 4], [0; 4], [sigma2, 0.0, 0.0, 0.0]), field, Some(img), next_img, Some(&lv.flags), lv.holes);
        // Carry the round's sources into the nearest-neighbour field (targets only).
        e.pixels("imp_nnf_store", &params([w, h, 0, 0], [0; 4], [0; 4], [0.0; 4]), field, Some(&field_nnf), &nnf_spare, Some(&lv.flags));
        std::mem::swap(&mut field_nnf, &mut nnf_spare);
        in_a = !in_a;
    }
    Some((if in_a { img_a } else { img_b }, field_nnf))
}

/// The 75th percentile of a 1024-bin distance histogram (`imp_hist`'s readback), and the
/// histogram's total. The bin's centre is the estimate: within half a bin (4/1024) of the CPU's
/// exact percentile.
fn percentile(bytes: &[u8]) -> (f32, f64) {
    let bin = |i: usize| -> f64 {
        match bytes.get(i * 16..i * 16 + 4) {
            Some(b) => f32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f64,
            None => 0.0,
        }
    };
    let total: f64 = (0..BINS).map(bin).sum();
    if total < 1.0 {
        return (1.0, total);
    }
    let rank = total * 3.0 / 4.0;
    let mut cum = 0.0f64;
    let mut at = BINS - 1;
    for i in 0..BINS {
        cum += bin(i);
        if cum > rank {
            at = i;
            break;
        }
    }
    ((((at as f32) + 0.5) / (BINS as f32 / 4.0)).max(1e-5), total)
}

/// The vote's sigma² at a level (`pm_level`): the 75th percentile of the targets' distances. One
/// 16 KB readback per EM round (≈ 28 per 1080p patch): a few percent of the stage, and the
/// obvious next thing to remove if the patch has to be faster (a device-side percentile would
/// need a reduction instead).
fn sigma2(e: &mut Enc, field: &GpuImage, lv: &Lv, hist: &GpuImage) -> Option<f32> {
    let (bw, bh) = (lv.near[2] - lv.near[0], lv.near[3] - lv.near[1]);
    let p = params([lv.w, lv.h, 0, 0], [lv.near[0], lv.near[1], bw, bh], [0; 4], [0.0; 4]);
    e.dispatch("imp_hist", &p, KernelImages { src: field, aux: None, out: hist }, Some(&lv.flags), (1, 1));
    let bytes = e.read_texture(&hist.texture, BINS as u32, 1, 16)?;
    Some(percentile(&bytes).0)
}

/// [`imp_hist`] on its own: `field`'s match distance (`.b`, `.a` > 0.5 where set) over the
/// pixels `near` marks, returning the 75th percentile and the sample count. Used by the test that
/// checks the histogram counts the whole target population.
#[cfg(test)]
pub(crate) fn distance_histogram(g: &GpuContext, field: &Image, near: &[bool], w: u32, h: u32) -> Option<(f32, f64)> {
    let flags: Vec<f32> = near.iter().map(|b| if *b { 2.0 } else { 0.0 }).collect();
    let mut e = Enc::new(g);
    let data = e.data(&flags);
    let img = g.upload_image(field)?;
    let hist = e.scratch(BINS as u32, 1);
    let p = params([w, h, 0, 0], [0, 0, w, h], [0; 4], [0.0; 4]);
    e.dispatch("imp_hist", &p, KernelImages { src: &img, aux: None, out: &hist }, Some(&data), (1, 1));
    if e.take_missing() {
        return None;
    }
    let bytes = e.read_texture(&hist.texture, BINS as u32, 1, 16)?;
    Some(percentile(&bytes))
}

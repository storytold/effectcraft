//! Mode decision and reconstruction (not normative): partitions, intra modes, motion vectors,
//! transform coefficients. Every prediction and reconstruction step uses the normative
//! processes, so the reconstruction built here is exactly what a decoder produces; the syntax
//! is written afterwards by `writer.rs`.

use crate::pred::{INTRA_MODES_USED, InterBlock, IntraEdge, Plane, edges, half_planes, predict_inter, predict_intra};
use crate::tables::{
    AC_QLOOKUP, BLOCK_8X8, BLOCK_64X64, DC_PRED, DC_QLOOKUP, DCT_DCT, DEFAULT_SCAN_4X4, DEFAULT_SCAN_8X8, DEFAULT_SCAN_16X16, DEFAULT_SCAN_32X32, MODE_TO_TXFM,
    NEWMV, NUM_4X4_BLOCKS_WIDE, PARTITION_SPLIT, PARTITION_SUBSIZE, TX_4X4, TX_8X8, TX_16X16, TX_32X32, TX_64X64, TX_SET_DCTONLY, TX_SET_INTER_3,
    TX_SET_INTRA_2, TX_SIZE_SQR_UP, TX_TYPE_IN_SET_INTER, TX_TYPE_IN_SET_INTRA,
};
use crate::transform::{forward_2d, inverse_2d};

/// Decisions for one coded block.
#[derive(Clone)]
pub(crate) struct Leaf {
    pub bsize: usize,
    pub is_inter: bool,
    pub y_mode: usize,
    pub uv_mode: usize,
    pub tx_type_y: usize,
    /// Motion vector [row, col] in 1/8 luma samples (inter blocks).
    pub mv: [i32; 2],
    pub skip: bool,
    /// Quantised coefficients of the single transform block of each plane (raster order).
    pub coefs: [Vec<i32>; 3],
}

pub(crate) enum Node {
    Out,
    Leaf(Box<Leaf>),
    Split(Box<[Node; 4]>),
}

/// A reconstructed frame kept as a reference.
pub(crate) struct RefFrame {
    pub planes: [Plane; 3],
    /// Motion vectors chosen for the frame (per 4x4), used as search candidates.
    pub mvs: Vec<[i32; 2]>,
    /// The frame's loop filter level.
    pub lf_level: u32,
    /// The CDFs saved at the end of the frame (loaded by the next frame).
    pub cdfs: Box<crate::cdf::Cdfs>,
}

pub(crate) struct FrameGeom {
    pub width: usize,
    pub height: usize,
    pub mi_cols: usize,
    pub mi_rows: usize,
    pub sb_cols: usize,
    pub sb_rows: usize,
}

impl FrameGeom {
    pub fn new(width: usize, height: usize) -> FrameGeom {
        let mi_cols = 2 * width.div_ceil(8);
        let mi_rows = 2 * height.div_ceil(8);
        FrameGeom { width, height, mi_cols, mi_rows, sb_cols: mi_cols.div_ceil(16), sb_rows: mi_rows.div_ceil(16) }
    }
    /// Padded plane size (whole superblocks).
    pub fn padded(&self, plane: usize) -> (usize, usize) {
        let s = (plane > 0) as usize;
        ((self.sb_cols * 64) >> s, (self.sb_rows * 64) >> s)
    }
    /// Visible plane size.
    pub fn visible(&self, plane: usize) -> (usize, usize) {
        if plane == 0 { (self.width, self.height) } else { (self.width.div_ceil(2), self.height.div_ceil(2)) }
    }
}

pub(crate) fn tx_size_for(log2: u32) -> usize {
    match log2 {
        2 => TX_4X4,
        3 => TX_8X8,
        4 => TX_16X16,
        5 => TX_32X32,
        _ => TX_64X64,
    }
}

/// get_tx_set( ) for the square sizes used here with reduced_tx_set = 1.
pub(crate) fn tx_set(tx_sz: usize, is_inter: bool) -> usize {
    let sqr_up = TX_SIZE_SQR_UP[tx_sz] as usize;
    if sqr_up > TX_32X32 {
        return TX_SET_DCTONLY;
    }
    if is_inter {
        TX_SET_INTER_3
    } else if sqr_up == TX_32X32 {
        TX_SET_DCTONLY
    } else {
        TX_SET_INTRA_2
    }
}

pub(crate) fn tx_in_set(set: usize, is_inter: bool, tx_type: usize) -> bool {
    if is_inter { TX_TYPE_IN_SET_INTER[set][tx_type] == 1 } else { TX_TYPE_IN_SET_INTRA[set][tx_type] == 1 }
}

/// The chroma transform type of an intra block (compute_tx_type for plane > 0).
pub(crate) fn intra_uv_tx_type(uv_mode: usize, uv_tx: usize) -> usize {
    let t = MODE_TO_TXFM[uv_mode] as usize;
    if tx_in_set(tx_set(uv_tx, false), false, t) { t } else { DCT_DCT }
}

/// Geometry and coding mode of one square transform block.
#[derive(Clone, Copy)]
struct TransformBlock {
    plane: usize,
    x: usize,
    y: usize,
    log2: u32,
    tx_type: usize,
    intra: bool,
}

struct TxResult {
    levels: Vec<i32>,
    nonzero: bool,
    dist: f64,
    bits: f64,
}

/// The output and scratch buffers of one reconstruction, each `n * n` entries long.
struct ReconBuffers<'a> {
    deq: &'a mut [i32],
    resid: &'a mut [i32],
    recon: &'a mut [u16],
}

/// Outcome of trying one transform block; the levels and the reconstruction stay in `TxScratch`.
#[derive(Clone, Copy)]
struct TxEval {
    nonzero: bool,
    dist: f64,
    bits: f64,
}

/// Buffers of the transform path, sized for the largest coded transform block (32x32). Only the
/// first `n * n` entries are used; they are reused across candidates instead of being allocated.
struct TxScratch {
    res: [i32; 1024],
    coef: [f32; 1024],
    levels: [i32; 1024],
    deq: [i32; 1024],
    resid: [i32; 1024],
    recon: [u16; 1024],
}

impl Default for TxScratch {
    fn default() -> Self {
        TxScratch { res: [0; 1024], coef: [0.0; 1024], levels: [0; 1024], deq: [0; 1024], resid: [0; 1024], recon: [0; 1024] }
    }
}

/// Saved encoder state of a block area (reconstruction, BlockDecoded flags).
struct Snapshot {
    planes: [Vec<u16>; 3],
    decoded: Box<[[[bool; 35]; 35]; 3]>,
}

pub(crate) struct Decider<'a> {
    pub g: &'a FrameGeom,
    pub bd: u32,
    pub intra_only: bool,
    pub src: &'a [Plane; 3],
    pub rec: [Plane; 3],
    pub refr: Option<&'a RefFrame>,
    decoded: Box<[[[bool; 35]; 35]; 3]>,
    /// Decision motion vectors per 4x4 (search candidates).
    pub mvs: Vec<[i32; 2]>,
    dc_q: f32,
    ac_q: f32,
    dc_qi: i64,
    ac_qi: i64,
    /// Smallest magnitude that quantises to a non-zero level, `[dc, ac][intra]`.
    dead_zone: [[f32; 2]; 2],
    lambda: f64,
    lambda_sad: f64,
    /// Half-sample interpolations of the reference luma (visible area): (8, 0), (0, 8), (8, 8).
    half: [Vec<u16>; 3],
    /// Transform buffers, taken out while a block is being tried (see `code_tx`).
    scratch: Option<Box<TxScratch>>,
}

const MODE_BITS_INTRA: f64 = 3.0;
/// Dead-zone quantiser rounding offsets (below one half) for inter and intra blocks.
const INTER_RND: f32 = 0.30;
const INTRA_RND: f32 = 0.38;

impl<'a> Decider<'a> {
    pub fn new(g: &'a FrameGeom, bd: u32, qidx: u8, intra_only: bool, src: &'a [Plane; 3], refr: Option<&'a RefFrame>) -> Self {
        let bdi = ((bd - 8) >> 1) as usize;
        let dc_qi = DC_QLOOKUP[bdi][qidx as usize] as i64;
        let ac_qi = AC_QLOOKUP[bdi][qidx as usize] as i64;
        // Lagrangian multiplier in squared-sample units of the coded bit depth.
        let step = ac_qi as f64 / 8.0;
        let lambda = 0.25 * step * step;
        let rec = std::array::from_fn(|p| {
            let (w, h) = g.padded(p);
            Plane::new(w, h)
        });
        Decider {
            g,
            bd,
            intra_only,
            src,
            rec,
            refr,
            decoded: Box::new([[[false; 35]; 35]; 3]),
            mvs: vec![[0, 0]; g.mi_cols * g.mi_rows],
            dc_q: dc_qi as f32,
            ac_q: ac_qi as f32,
            dc_qi,
            ac_qi,
            dead_zone: [dc_qi, ac_qi].map(|q| [INTER_RND, INTRA_RND].map(|rnd| zero_threshold(q as f32, rnd))),
            lambda,
            lambda_sad: lambda.sqrt(),
            half: match refr {
                Some(rf) => half_planes(&rf.planes[0], g.width, g.height, bd),
                None => Default::default(),
            },
            scratch: None,
        }
    }

    /// Decides one 64x64 superblock (reconstruction is written to `rec`).
    pub fn superblock(&mut self, r: usize, c: usize) -> Node {
        // clear_block_decoded_flags( r, c, sbSize4 )
        for plane in 0..3 {
            let s = (plane > 0) as usize;
            let sb_w4 = (self.g.mi_cols as isize - c as isize) >> s;
            let sb_h4 = (self.g.mi_rows as isize - r as isize) >> s;
            let bd = &mut self.decoded[plane];
            for y in -1..=((16 >> s) as isize) {
                for x in -1..=((16 >> s) as isize) {
                    let v = if y < 0 && x < sb_w4 { true } else { x < 0 && y < sb_h4 };
                    bd[(y + 1) as usize][(x + 1) as usize] = v;
                }
            }
            bd[(16 >> s) + 1][0] = false;
        }
        self.partition(r, c, BLOCK_64X64, None).0
    }

    fn decoded_at(&self, plane: usize, y: isize, x: isize) -> bool {
        let (yy, xx) = (y + 1, x + 1);
        if yy < 0 || xx < 0 || yy > 34 || xx > 34 {
            return false;
        }
        self.decoded[plane][yy as usize][xx as usize]
    }

    /// Marks the transform block of `plane` covering the block at (r, c) as decoded.
    fn mark_decoded(&mut self, plane: usize, r: usize, c: usize, n4: usize) {
        let s = (plane > 0) as usize;
        let by = (r & 15) >> s;
        let bx = (c & 15) >> s;
        let step = (n4 >> s).max(1);
        for i in 0..step {
            for j in 0..step {
                if by + i + 1 < 35 && bx + j + 1 < 35 {
                    self.decoded[plane][by + i + 1][bx + j + 1] = true;
                }
            }
        }
    }

    fn snapshot(&self, r: usize, c: usize, n4: usize) -> Snapshot {
        let planes = std::array::from_fn(|p| {
            let s = (p > 0) as usize;
            let (x0, y0, n) = ((c * 4) >> s, (r * 4) >> s, (n4 * 4) >> s);
            let pl = &self.rec[p];
            let mut v = Vec::with_capacity(n * n);
            for y in y0..y0 + n {
                v.extend_from_slice(&pl.data[y * pl.stride + x0..y * pl.stride + x0 + n]);
            }
            v
        });
        Snapshot { planes, decoded: self.decoded.clone() }
    }

    fn restore(&mut self, snap: &Snapshot, r: usize, c: usize, n4: usize) {
        for p in 0..3 {
            let s = (p > 0) as usize;
            let (x0, y0, n) = ((c * 4) >> s, (r * 4) >> s, (n4 * 4) >> s);
            let pl = &mut self.rec[p];
            for (i, y) in (y0..y0 + n).enumerate() {
                pl.data[y * pl.stride + x0..y * pl.stride + x0 + n].copy_from_slice(&snap.planes[p][i * n..i * n + n]);
            }
        }
        self.decoded.clone_from(&snap.decoded);
    }

    fn set_mvs(&mut self, r: usize, c: usize, n4: usize, mv: [i32; 2]) {
        for y in r..(r + n4).min(self.g.mi_rows) {
            for x in c..(c + n4).min(self.g.mi_cols) {
                self.mvs[y * self.g.mi_cols + x] = mv;
            }
        }
    }

    fn partition(&mut self, r: usize, c: usize, bsize: usize, parent_mv: Option<[i32; 2]>) -> (Node, f64) {
        let g = self.g;
        if r >= g.mi_rows || c >= g.mi_cols {
            return (Node::Out, 0.0);
        }
        let n4 = NUM_4X4_BLOCKS_WIDE[bsize] as usize;
        let half = n4 >> 1;
        let has_rows = r + half < g.mi_rows;
        let has_cols = c + half < g.mi_cols;
        let can_none = has_rows && has_cols && (bsize != BLOCK_64X64 || !self.intra_only);
        let can_split = bsize > BLOCK_8X8;
        let sub = PARTITION_SUBSIZE[PARTITION_SPLIT][bsize] as usize;
        if !can_none {
            return self.split(r, c, sub, half, parent_mv);
        }
        let before = self.snapshot(r, c, n4);
        let (leaf, cost_none) = self.leaf(r, c, bsize, parent_mv);
        let cost_none = cost_none + self.lambda * 1.0;
        if !can_split {
            return (Node::Leaf(Box::new(leaf)), cost_none);
        }
        // a cheap skip block rarely gains from splitting
        if leaf.skip && cost_none < self.lambda * 8.0 {
            return (Node::Leaf(Box::new(leaf)), cost_none);
        }
        let after_none = self.snapshot(r, c, n4);
        self.restore(&before, r, c, n4);
        let pm = if leaf.is_inter { Some(leaf.mv) } else { parent_mv };
        let (node, cost_split) = self.split(r, c, sub, half, pm);
        let cost_split = cost_split + self.lambda * 2.0;
        if cost_split < cost_none {
            (node, cost_split)
        } else {
            self.restore(&after_none, r, c, n4);
            self.set_mvs(r, c, n4, if leaf.is_inter { leaf.mv } else { [0, 0] });
            (Node::Leaf(Box::new(leaf)), cost_none)
        }
    }

    fn split(&mut self, r: usize, c: usize, sub: usize, half: usize, pm: Option<[i32; 2]>) -> (Node, f64) {
        let (n0, c0) = self.partition(r, c, sub, pm);
        let (n1, c1) = self.partition(r, c + half, sub, pm);
        let (n2, c2) = self.partition(r + half, c, sub, pm);
        let (n3, c3) = self.partition(r + half, c + half, sub, pm);
        (Node::Split(Box::new([n0, n1, n2, n3])), c0 + c1 + c2 + c3)
    }

    /// Best coding of the block as a single (NONE partition) block.
    fn leaf(&mut self, r: usize, c: usize, bsize: usize, parent_mv: Option<[i32; 2]>) -> (Leaf, f64) {
        let n4 = NUM_4X4_BLOCKS_WIDE[bsize] as usize;
        if self.intra_only {
            let res = self.intra_leaf(r, c, bsize);
            self.set_mvs(r, c, n4, [0, 0]);
            return res;
        }
        let before = self.snapshot(r, c, n4);
        let (inter, cost_inter) = self.inter_leaf(r, c, bsize, parent_mv);
        if bsize == BLOCK_64X64 || inter.skip {
            self.set_mvs(r, c, n4, inter.mv);
            return (inter, cost_inter);
        }
        let after_inter = self.snapshot(r, c, n4);
        self.restore(&before, r, c, n4);
        let (intra, cost_intra) = self.intra_leaf(r, c, bsize);
        if cost_intra < cost_inter {
            self.set_mvs(r, c, n4, [0, 0]);
            (intra, cost_intra)
        } else {
            self.restore(&after_inter, r, c, n4);
            self.set_mvs(r, c, n4, inter.mv);
            (inter, cost_inter)
        }
    }

    fn intra_leaf(&mut self, r: usize, c: usize, bsize: usize) -> (Leaf, f64) {
        let n4 = NUM_4X4_BLOCKS_WIDE[bsize] as usize;
        let log2 = (n4 * 4).trailing_zeros();
        let (sbr, sbc) = ((r & 15) as isize, (c & 15) as isize);
        let bd = self.bd;
        let mut pred = [0u16; 32 * 32];
        let mut best_pred = [0u16; 32 * 32];
        let mut best_levels = [0i32; 32 * 32];
        let mut best_recon = [0u16; 32 * 32];
        let n = 1usize << log2;
        // luma
        let e = IntraEdge {
            x: c * 4,
            y: r * 4,
            have_left: c > 0,
            have_above: r > 0,
            have_above_right: self.decoded_at(0, sbr - 1, sbc + n4 as isize),
            have_below_left: self.decoded_at(0, sbr + n4 as isize, sbc - 1),
            log2,
            max_x: (self.g.mi_cols * 4) as i32 - 1,
            max_y: (self.g.mi_rows * 4) as i32 - 1,
            bit_depth: bd,
        };
        let ed = edges(&self.rec[0], &e);
        // modes ranked by SATD, then a rate-distortion choice of mode and transform type among
        // the best three
        let mut ranked: [(f64, usize); INTRA_MODES_USED.len()] = INTRA_MODES_USED.map(|mode| {
            predict_intra(&ed, &e, mode, &mut pred);
            let sad = self.satd(0, c * 4, r * 4, n, &pred[..n * n]) as f64;
            (sad + self.lambda_sad * if mode == DC_PRED { 1.0 } else { MODE_BITS_INTRA }, mode)
        });
        ranked.sort_by(|a, b| a.0.total_cmp(&b.0));
        let tx_y = tx_size_for(log2);
        let mut best: Option<(f64, usize, usize, TxEval, bool)> = None;
        let mut sc = self.scratch.take().unwrap_or_default();
        for &(_, mode) in ranked.iter().take(3) {
            predict_intra(&ed, &e, mode, &mut pred);
            let mut types = [DCT_DCT; 2];
            let mut n_types = 1;
            let alt = MODE_TO_TXFM[mode] as usize;
            if alt != DCT_DCT && tx_in_set(tx_set(tx_y, false), false, alt) {
                types[1] = alt;
                n_types = 2;
            }
            for &t in &types[..n_types] {
                let (res, has_recon) = self.try_tx(&mut sc, TransformBlock { plane: 0, x: c * 4, y: r * 4, log2, tx_type: t, intra: true }, &pred[..n * n]);
                let type_bits = if !res.nonzero || tx_set(tx_y, false) == TX_SET_DCTONLY {
                    0.0
                } else if t == DCT_DCT {
                    1.0
                } else {
                    2.0
                };
                let mode_bits = if mode == DC_PRED { 1.0 } else { MODE_BITS_INTRA };
                let cost = res.dist + self.lambda * (res.bits + type_bits + mode_bits);
                if best.as_ref().is_none_or(|b| cost < b.0) {
                    best_pred[..n * n].copy_from_slice(&pred[..n * n]);
                    best_levels[..n * n].copy_from_slice(&sc.levels[..n * n]);
                    if has_recon {
                        best_recon[..n * n].copy_from_slice(&sc.recon[..n * n]);
                    }
                    best = Some((cost, mode, t, res, has_recon));
                }
            }
        }
        self.scratch = Some(sc);
        // Every ranked mode tries DCT_DCT, so `best` is always set; without a candidate the block
        // can't be coded intra (an infinite cost keeps it from being chosen).
        let Some((_, y_mode, tx_type_y, ty, has_recon)) = best else {
            let leaf =
                Leaf { bsize, is_inter: false, y_mode: DC_PRED, uv_mode: DC_PRED, tx_type_y: DCT_DCT, mv: [0, 0], skip: true, coefs: Default::default() };
            return (leaf, f64::INFINITY);
        };
        self.put(0, c * 4, r * 4, n, if has_recon { &best_recon[..n * n] } else { &best_pred[..n * n] });
        let ty = TxResult { levels: best_levels[..n * n].to_vec(), nonzero: ty.nonzero, dist: ty.dist, bits: ty.bits };
        let tx_type_y = if ty.nonzero { tx_type_y } else { DCT_DCT };
        self.mark_decoded(0, r, c, n4);
        // chroma (one mode for both planes)
        let cn = n >> 1;
        let clog2 = log2 - 1;
        let (csbr, csbc) = (sbr >> 1, sbc >> 1);
        let ce: [IntraEdge; 2] = std::array::from_fn(|i| IntraEdge {
            x: c * 2,
            y: r * 2,
            have_left: c > 0,
            have_above: r > 0,
            have_above_right: self.decoded_at(1 + i, csbr - 1, csbc + (n4 >> 1) as isize),
            have_below_left: self.decoded_at(1 + i, csbr + (n4 >> 1) as isize, csbc - 1),
            log2: clog2,
            max_x: (self.g.mi_cols * 2) as i32 - 1,
            max_y: (self.g.mi_rows * 2) as i32 - 1,
            bit_depth: bd,
        });
        let ced = [edges(&self.rec[1], &ce[0]), edges(&self.rec[2], &ce[1])];
        let mut best_uv = (f64::MAX, DC_PRED);
        for &mode in INTRA_MODES_USED.iter() {
            let mut sad = 0u64;
            for i in 0..2 {
                predict_intra(&ced[i], &ce[i], mode, &mut pred);
                sad += self.satd(1 + i, c * 2, r * 2, cn, &pred[..cn * cn]);
            }
            let cost = sad as f64 + self.lambda_sad * if mode == DC_PRED { 1.0 } else { 2.5 };
            if cost < best_uv.0 {
                best_uv = (cost, mode);
            }
        }
        let uv_mode = best_uv.1;
        let uv_tx = tx_size_for(clog2);
        let uv_type = intra_uv_tx_type(uv_mode, uv_tx);
        let mut code_uv = |i: usize| {
            predict_intra(&ced[i], &ce[i], uv_mode, &mut pred);
            let t = self.code_tx(TransformBlock { plane: 1 + i, x: c * 2, y: r * 2, log2: clog2, tx_type: uv_type, intra: true }, &pred[..cn * cn]);
            self.mark_decoded(1 + i, r, c, n4);
            t
        };
        let u = code_uv(0);
        let v = code_uv(1);
        let skip = !ty.nonzero && !u.nonzero && !v.nonzero;
        let dist = ty.dist + u.dist + v.dist;
        let bits = if skip { 1.0 } else { ty.bits + u.bits + v.bits + 1.0 } + MODE_BITS_INTRA + 2.0;
        let leaf = Leaf { bsize, is_inter: false, y_mode, uv_mode, tx_type_y, mv: [0, 0], skip, coefs: [ty.levels, u.levels, v.levels] };
        (leaf, dist + self.lambda * bits)
    }

    fn inter_leaf(&mut self, r: usize, c: usize, bsize: usize, parent_mv: Option<[i32; 2]>) -> (Leaf, f64) {
        let n4 = NUM_4X4_BLOCKS_WIDE[bsize] as usize;
        let n = n4 * 4;
        let log2 = n.trailing_zeros();
        // Inter blocks are only decided with a reference frame; without one, code intra.
        let Some(refr) = self.refr else { return self.intra_leaf(r, c, bsize) };
        let (mv, mv_bits) = self.motion_search(r, c, n, parent_mv);
        let mut pred = vec![0u16; n * n];
        let mut tx = Vec::with_capacity(3);
        let mut dist = 0.0;
        for p in 0..3 {
            let s = (p > 0) as usize;
            let (vw, vh) = self.g.visible(p);
            let pn = n >> s;
            let (x, y) = ((c * 4) >> s, (r * 4) >> s);
            predict_inter(
                &refr.planes[p],
                &InterBlock { last_x: vw as i32 - 1, last_y: vh as i32 - 1, x, y, width: pn, height: pn, mv, subsampling: s, bit_depth: self.bd },
                &mut pred[..pn * pn],
            );
            if bsize == BLOCK_64X64 {
                // coded as a skip block: the prediction is the reconstruction
                dist += self.sse(p, x, y, pn, &pred[..pn * pn]);
                self.put(p, x, y, pn, &pred[..pn * pn]);
            } else {
                let t = self.code_tx(TransformBlock { plane: p, x, y, log2: log2 - s as u32, tx_type: DCT_DCT, intra: false }, &pred[..pn * pn]);
                dist += t.dist;
                tx.push(t);
            }
            self.mark_decoded(p, r, c, n4);
        }
        let mode_bits = if mv == [0, 0] { 1.5 } else { 2.0 + mv_bits };
        if bsize == BLOCK_64X64 {
            let leaf = Leaf { bsize, is_inter: true, y_mode: NEWMV, uv_mode: DC_PRED, tx_type_y: DCT_DCT, mv, skip: true, coefs: Default::default() };
            return (leaf, dist + self.lambda * (mode_bits + 1.0));
        }
        let skip = tx.iter().all(|t| !t.nonzero);
        let bits = mode_bits + if skip { 1.0 } else { 1.0 + tx.iter().map(|t| t.bits).sum::<f64>() };
        let [y, u, v]: [TxResult; 3] = match tx.try_into() {
            Ok(planes) => planes,
            // One transform block per plane was coded above.
            Err(_) => {
                return (
                    Leaf { bsize, is_inter: true, y_mode: NEWMV, uv_mode: DC_PRED, tx_type_y: DCT_DCT, mv, skip: true, coefs: Default::default() },
                    f64::INFINITY,
                );
            }
        };
        let leaf = Leaf { bsize, is_inter: true, y_mode: NEWMV, uv_mode: DC_PRED, tx_type_y: DCT_DCT, mv, skip, coefs: [y.levels, u.levels, v.levels] };
        (leaf, dist + self.lambda * bits)
    }

    /// Full-sample motion search: candidate vectors then a shrinking diamond search.
    /// Returns the vector and an estimate of its coding cost in bits.
    fn motion_search(&self, r: usize, c: usize, n: usize, parent_mv: Option<[i32; 2]>) -> ([i32; 2], f64) {
        let g = self.g;
        let Some(refr) = self.refr else { return ([0, 0], 0.0) };
        let mi = |rr: usize, cc: usize| self.mvs[rr * g.mi_cols + cc];
        let mut pred_mv = [0, 0];
        let mut cands: Vec<[i32; 2]> = vec![[0, 0]];
        if c > 0 {
            pred_mv = mi(r, c - 1);
            cands.push(pred_mv);
        }
        if r > 0 {
            cands.push(mi(r - 1, c));
            let cr = c + n / 4;
            if cr < g.mi_cols {
                cands.push(mi(r - 1, cr));
            }
        }
        if let Some(p) = parent_mv {
            cands.push(p);
        }
        cands.push(refr.mvs[r * g.mi_cols + c]);
        let (x0, y0) = ((c * 4) as i32, (r * 4) as i32);
        let (w, h) = (g.width as i32, g.height as i32);
        // keep the referenced block within 64 samples of the picture
        let lim_x = (-x0 - n as i32 - 64, w - x0 + 64);
        let lim_y = (-y0 - n as i32 - 64, h - y0 + 64);
        let valid = |m: [i32; 2]| {
            let (dy, dx) = (m[0] >> 3, m[1] >> 3);
            dx >= lim_x.0 && dx <= lim_x.1 && dy >= lim_y.0 && dy <= lim_y.1
        };
        let lg = log2_table();
        let mv_bits = |m: [i32; 2]| -> f64 {
            let comp = |d: i32| if d == 0 { 0.5 } else { 2.0 + 2.0 * log2_of(lg, (d.abs() / 8) as usize + 1) };
            comp(m[0] - pred_mv[0]) + comp(m[1] - pred_mv[1])
        };
        let cost =
            |m: [i32; 2]| -> f64 { self.sad_ref(&refr.planes[0], x0 + (m[1] >> 3), y0 + (m[0] >> 3), c * 4, r * 4, n) as f64 + self.lambda_sad * mv_bits(m) };
        let mut best = ([0, 0], f64::MAX);
        for &m in &cands {
            let m = [m[0] & !7, m[1] & !7];
            if !valid(m) {
                continue;
            }
            let cst = cost(m);
            if cst < best.1 {
                best = (m, cst);
            }
        }
        let mut step = 8 * if n >= 32 {
            16
        } else if n == 16 {
            8
        } else {
            4
        };
        while step >= 8 {
            let mut moved = true;
            let mut iters = 0;
            while moved && iters < 16 {
                moved = false;
                iters += 1;
                let center = best.0;
                for d in [[-step, 0], [step, 0], [0, -step], [0, step]] {
                    let m = [center[0] + d[0], center[1] + d[1]];
                    if !valid(m) {
                        continue;
                    }
                    let cst = cost(m);
                    if cst < best.1 {
                        best = (m, cst);
                        moved = true;
                    }
                }
            }
            step >>= 1;
        }
        // half- then quarter-sample refinement (plus the exact neighbour candidates)
        let mut buf = vec![0u16; n * n];
        let sub_cost = |m: [i32; 2], buf: &mut Vec<u16>| -> f64 {
            self.search_block(m, c * 4, r * 4, n, buf);
            self.sad(0, c * 4, r * 4, n, buf) as f64 + self.lambda_sad * mv_bits(m)
        };
        for m in cands {
            if m[0] & 7 != 0 || m[1] & 7 != 0 {
                let m = [m[0] & !1, m[1] & !1];
                if valid(m) {
                    let cst = sub_cost(m, &mut buf);
                    if cst < best.1 {
                        best = (m, cst);
                    }
                }
            }
        }
        for step in [4, 2] {
            let center = best.0;
            for d in [[-1, -1], [-1, 0], [-1, 1], [0, -1], [0, 1], [1, -1], [1, 0], [1, 1]] {
                let m = [center[0] + d[0] * step, center[1] + d[1] * step];
                if !valid(m) {
                    continue;
                }
                let cst = sub_cost(m, &mut buf);
                if cst < best.1 {
                    best = (m, cst);
                }
            }
        }
        (best.0, mv_bits(best.0))
    }

    /// Approximate luma prediction for motion search: exact at whole and half-sample
    /// positions (precomputed planes), averaged neighbours at quarter-sample positions.
    fn search_block(&self, mv: [i32; 2], x: usize, y: usize, n: usize, out: &mut [u16]) {
        let Some(refr) = self.refr else { return };
        let (w, h) = (self.g.width as i32, self.g.height as i32);
        // per axis: up to two (integer offset, half) taps
        let axis = |v: i32| -> ([(i32, bool); 2], usize) {
            let p = 2 * v; // 1/16 units
            let (i, f) = (p >> 4, p & 15);
            match f {
                0 => ([(i, false), (i, false)], 1),
                8 => ([(i, true), (i, true)], 1),
                4 => ([(i, false), (i, true)], 2),
                _ => ([(i, true), (i + 1, false)], 2),
            }
        };
        let (ax, nx) = axis(mv[1]);
        let (ay, ny) = axis(mv[0]);
        let taps = nx.max(ny);
        let fetch = |k: usize, xx: i32, yy: i32| -> u16 {
            let (ox, hx) = ax[k.min(nx - 1)];
            let (oy, hy) = ay[k.min(ny - 1)];
            let sx = (xx + ox).clamp(0, w - 1) as usize;
            let sy = (yy + oy).clamp(0, h - 1) as usize;
            match (hx, hy) {
                (false, false) => refr.planes[0].at(sx, sy),
                (true, false) => self.half[0][sy * w as usize + sx],
                (false, true) => self.half[1][sy * w as usize + sx],
                (true, true) => self.half[2][sy * w as usize + sx],
            }
        };
        let vw = n.min(self.g.width.saturating_sub(x));
        let vh = n.min(self.g.height.saturating_sub(y));
        // fast path: every tap inside the picture
        let (x0, y0) = (x as i32, y as i32);
        let inside = (0..taps).all(|k| {
            let (ox, _) = ax[k.min(nx - 1)];
            let (oy, _) = ay[k.min(ny - 1)];
            x0 + ox >= 0 && y0 + oy >= 0 && x0 + ox + vw as i32 <= w && y0 + oy + vh as i32 <= h
        });
        if inside {
            let src = |k: usize| -> (&[u16], usize, usize) {
                let (ox, hx) = ax[k.min(nx - 1)];
                let (oy, hy) = ay[k.min(ny - 1)];
                let (sx, sy) = ((x0 + ox) as usize, (y0 + oy) as usize);
                match (hx, hy) {
                    (false, false) => (&refr.planes[0].data, refr.planes[0].stride, sy * refr.planes[0].stride + sx),
                    (true, false) => (&self.half[0], w as usize, sy * w as usize + sx),
                    (false, true) => (&self.half[1], w as usize, sy * w as usize + sx),
                    (true, true) => (&self.half[2], w as usize, sy * w as usize + sx),
                }
            };
            let (a, sa, oa) = src(0);
            if taps == 1 {
                for i in 0..vh {
                    out[i * n..i * n + vw].copy_from_slice(&a[oa + i * sa..oa + i * sa + vw]);
                }
            } else {
                let (b, sb, ob) = src(1);
                for i in 0..vh {
                    let (ra, rb) = (&a[oa + i * sa..oa + i * sa + vw], &b[ob + i * sb..ob + i * sb + vw]);
                    for (o, (&p, &q)) in out[i * n..i * n + vw].iter_mut().zip(ra.iter().zip(rb)) {
                        *o = ((p as u32 + q as u32 + 1) >> 1) as u16;
                    }
                }
            }
            return;
        }
        for i in 0..vh {
            for j in 0..vw {
                let (xx, yy) = (x0 + j as i32, y0 + i as i32);
                out[i * n + j] = if taps == 1 { fetch(0, xx, yy) } else { ((fetch(0, xx, yy) as u32 + fetch(1, xx, yy) as u32 + 1) >> 1) as u16 };
            }
        }
    }

    /// SAD of the source block at (x, y) (luma) against the reference block at (rx, ry).
    fn sad_ref(&self, refp: &Plane, rx: i32, ry: i32, x: usize, y: usize, n: usize) -> u64 {
        let (vw, vh) = (self.g.width, self.g.height);
        let w = n.min(vw.saturating_sub(x));
        let h = n.min(vh.saturating_sub(y));
        let src = &self.src[0];
        let (lx, ly) = (vw as i32 - 1, vh as i32 - 1);
        let mut sad = 0u64;
        let inside = rx >= 0 && ry >= 0 && rx + w as i32 - 1 <= lx && ry + h as i32 - 1 <= ly;
        for i in 0..h {
            let s = &src.row(y + i)[x..x + w];
            let rr = refp.row((ry + i as i32).clamp(0, ly) as usize);
            if inside {
                let rrow = &rr[rx as usize..rx as usize + w];
                sad += s.iter().zip(rrow).map(|(&a, &b)| (a as i32 - b as i32).unsigned_abs()).sum::<u32>() as u64;
            } else {
                for j in 0..w {
                    let b = rr[(rx + j as i32).clamp(0, lx) as usize];
                    sad += (s[j] as i32 - b as i32).unsigned_abs() as u64;
                }
            }
        }
        sad
    }

    /// Sum of absolute Hadamard-transformed differences (4x4 or 8x8 sub-blocks) of a prediction
    /// against the source, scaled to about the size of the SAD.
    fn satd(&self, p: usize, x: usize, y: usize, n: usize, pred: &[u16]) -> u64 {
        let src = &self.src[p];
        let k = n.min(8);
        let mut total = 0u64;
        let mut d = [0i32; 64];
        for by in (0..n).step_by(k) {
            for bx in (0..n).step_by(k) {
                for i in 0..k {
                    let row = &src.row(y + by + i)[x + bx..x + bx + k];
                    for j in 0..k {
                        d[i * k + j] = row[j] as i32 - pred[(by + i) * n + bx + j] as i32;
                    }
                }
                for i in 0..k {
                    hadamard(&mut d[i * k..i * k + k]);
                }
                let mut col = [0i32; 8];
                let mut s = 0u64;
                for j in 0..k {
                    for i in 0..k {
                        col[i] = d[i * k + j];
                    }
                    hadamard(&mut col[..k]);
                    s += col[..k].iter().map(|v| v.unsigned_abs() as u64).sum::<u64>();
                }
                total += s / (k as u64 / 2);
            }
        }
        total
    }

    /// SAD of a prediction against the source over the visible part of the block.
    fn sad(&self, p: usize, x: usize, y: usize, n: usize, pred: &[u16]) -> u64 {
        let (vw, vh) = self.g.visible(p);
        let w = n.min(vw.saturating_sub(x));
        let h = n.min(vh.saturating_sub(y));
        let src = &self.src[p];
        let mut s = 0u64;
        for i in 0..h {
            let row = &src.row(y + i)[x..x + w];
            s += row.iter().zip(&pred[i * n..i * n + w]).map(|(&a, &b)| (a as i32 - b as i32).unsigned_abs()).sum::<u32>() as u64;
        }
        s
    }

    fn sse(&self, p: usize, x: usize, y: usize, n: usize, blk: &[u16]) -> f64 {
        let (vw, vh) = self.g.visible(p);
        let w = n.min(vw.saturating_sub(x));
        let h = n.min(vh.saturating_sub(y));
        let src = &self.src[p];
        let mut s = 0u64;
        for i in 0..h {
            let row = &src.row(y + i)[x..x + w];
            s += row
                .iter()
                .zip(&blk[i * n..i * n + w])
                .map(|(&a, &b)| {
                    let d = a as i64 - b as i64;
                    (d * d) as u64
                })
                .sum::<u64>();
        }
        s as f64
    }

    fn put(&mut self, p: usize, x: usize, y: usize, n: usize, blk: &[u16]) {
        let pl = &mut self.rec[p];
        for i in 0..n {
            let o = (y + i) * pl.stride + x;
            pl.data[o..o + n].copy_from_slice(&blk[i * n..i * n + n]);
        }
    }

    /// Transform, quantise and reconstruct one square transform block of `plane` at (x, y)
    /// over the prediction `pred`; the reconstruction is written to `rec`.
    fn code_tx(&mut self, block: TransformBlock, pred: &[u16]) -> TxResult {
        let TransformBlock { plane: p, x, y, log2, .. } = block;
        let mut sc = self.scratch.take().unwrap_or_default();
        let (res, has_recon) = self.try_tx(&mut sc, block, pred);
        let n = 1usize << log2;
        self.put(p, x, y, n, if has_recon { &sc.recon[..n * n] } else { pred });
        let levels = sc.levels[..n * n].to_vec();
        self.scratch = Some(sc);
        TxResult { levels, nonzero: res.nonzero, dist: res.dist, bits: res.bits }
    }

    /// Transform and quantise one block without writing it. The levels are left in `sc.levels`;
    /// the flag tells whether `sc.recon` holds the reconstruction (false: it is the prediction).
    fn try_tx(&self, sc: &mut TxScratch, block: TransformBlock, pred: &[u16]) -> (TxEval, bool) {
        let TransformBlock { plane: p, x, y, log2, tx_type, intra } = block;
        let n = 1usize << log2;
        let nn = n * n;
        let tx_sz = tx_size_for(log2);
        let bd = self.bd;
        let src = &self.src[p];
        let TxScratch { res, coef, levels, deq, resid, recon } = sc;
        let (res, coef, levels) = (&mut res[..nn], &mut coef[..nn], &mut levels[..nn]);
        for i in 0..n {
            let row = &src.row(y + i)[x..x + n];
            for j in 0..n {
                res[i * n + j] = row[j] as i32 - pred[i * n + j] as i32;
            }
        }
        forward_2d(res, coef, n, tx_type);
        // dead-zone quantiser: rounding offset below one half
        let rnd = if intra { INTRA_RND } else { INTER_RND };
        let [dc_thr, ac_thr] = [self.dead_zone[0][intra as usize], self.dead_zone[1][intra as usize]];
        levels.fill(0);
        let mut nonzero = false;
        for (k, (&cf, l)) in coef.iter().zip(levels.iter_mut()).enumerate() {
            let (q, thr) = if k == 0 { (self.dc_q, dc_thr) } else { (self.ac_q, ac_thr) };
            // dead zone: below the exact threshold the level is zero, no division needed
            if cf.abs() < thr {
                continue;
            }
            // the operand is >= 0, so truncation equals floor
            let a = (cf.abs() / q + rnd) as i32;
            if a != 0 {
                *l = if cf < 0.0 { -a.min(1 << 16) } else { a.min(1 << 16) };
                nonzero = true;
            }
        }
        let zero_dist = self.sse(p, x, y, n, pred);
        if !nonzero {
            return (TxEval { nonzero, dist: zero_dist, bits: 1.0 }, false);
        }
        let recon = &mut recon[..nn];
        self.reconstruct(levels, ReconBuffers { deq: &mut deq[..nn], resid: &mut resid[..nn], recon }, tx_sz, tx_type, pred, bd);
        let dist = self.sse(p, x, y, n, recon);
        let bits = estimate_bits(levels, tx_sz);
        if dist + self.lambda * bits >= zero_dist + self.lambda {
            levels.fill(0);
            return (TxEval { nonzero: false, dist: zero_dist, bits: 1.0 }, false);
        }
        (TxEval { nonzero: true, dist, bits }, true)
    }

    /// Dequantisation (7.12.3) and the inverse transform, added to the prediction; the result is
    /// written to `recon`. `deq` and `resid` are scratch buffers of `n * n` entries.
    fn reconstruct(&self, levels: &[i32], bufs: ReconBuffers, tx_sz: usize, tx_type: usize, pred: &[u16], bd: u32) {
        let ReconBuffers { deq, resid, recon } = bufs;
        let denom: i64 = if tx_sz == TX_32X32 { 2 } else { 1 };
        let lim = 1i64 << (7 + bd);
        deq.fill(0);
        for (k, (&l, d)) in levels.iter().zip(deq.iter_mut()).enumerate() {
            if l == 0 {
                continue;
            }
            let q = if k == 0 { self.dc_qi } else { self.ac_qi };
            let dq = l as i64 * q;
            let mag = (dq.abs() & 0xFF_FFFF) / denom;
            *d = (if dq < 0 { -mag } else { mag }).clamp(-lim, lim - 1) as i32;
        }
        inverse_2d(deq, resid, tx_sz, tx_type, bd);
        let max = (1i32 << bd) - 1;
        for ((o, &p), &r) in recon.iter_mut().zip(pred).zip(resid.iter()) {
            *o = (p as i32 + r).clamp(0, max) as u16;
        }
    }
}

/// Smallest `x >= 0` with `(x / q + rnd) as i32 >= 1` in f32 arithmetic. The expression is
/// monotone in `x`, so a binary search over the bit patterns finds the exact boundary.
fn zero_threshold(q: f32, rnd: f32) -> f32 {
    let (mut lo, mut hi) = (0u32, f32::MAX.to_bits());
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if (f32::from_bits(mid) / q + rnd) as i32 >= 1 {
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }
    f32::from_bits(lo)
}

const LOG2_TABLE_LEN: usize = 1025;

/// `log2(i)` for `i < LOG2_TABLE_LEN`, computed with the same `f64::log2` call as the direct form.
fn log2_table() -> &'static [f64; LOG2_TABLE_LEN] {
    static TABLE: std::sync::OnceLock<[f64; LOG2_TABLE_LEN]> = std::sync::OnceLock::new();
    TABLE.get_or_init(|| std::array::from_fn(|i| (i as f64).log2()))
}

fn log2_of(table: &[f64; LOG2_TABLE_LEN], v: usize) -> f64 {
    if v < LOG2_TABLE_LEN { table[v] } else { (v as f64).log2() }
}

/// Unnormalised Walsh-Hadamard transform of 4 or 8 values, in place.
fn hadamard(v: &mut [i32]) {
    let n = v.len();
    let mut h = 1;
    while h < n {
        for i in (0..n).step_by(2 * h) {
            for j in i..i + h {
                let (a, b) = (v[j], v[j + h]);
                v[j] = a + b;
                v[j + h] = a - b;
            }
        }
        h *= 2;
    }
}

/// Rough coefficient coding cost in bits.
fn estimate_bits(levels: &[i32], tx_sz: usize) -> f64 {
    let scan = scan_for(tx_sz);
    let mut eob = 0;
    for (i, &pos) in scan.iter().enumerate() {
        if levels[pos as usize] != 0 {
            eob = i + 1;
        }
    }
    if eob == 0 {
        return 1.0;
    }
    let lg = log2_table();
    let mut bits = 3.0 + log2_of(lg, eob);
    for &pos in &scan[..eob] {
        let a = levels[pos as usize].unsigned_abs();
        bits += match a {
            0 => 0.7,
            1 => 2.5,
            _ => 3.5 + 1.6 * log2_of(lg, a as usize),
        };
    }
    bits
}

pub(crate) fn scan_for(tx_sz: usize) -> &'static [u16] {
    match tx_sz {
        TX_4X4 => &DEFAULT_SCAN_4X4,
        TX_8X8 => &DEFAULT_SCAN_8X8,
        TX_16X16 => &DEFAULT_SCAN_16X16,
        _ => &DEFAULT_SCAN_32X32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tables::{AC_QLOOKUP, DC_QLOOKUP};

    /// The quantiser as it was before the dead-zone threshold and the truncation.
    fn quant_ref(cf: f32, q: f32, rnd: f32) -> i32 {
        (cf.abs() / q + rnd).floor() as i32
    }

    fn quant_new(cf: f32, q: f32, rnd: f32, thr: f32) -> i32 {
        if cf.abs() < thr {
            return 0;
        }
        (cf.abs() / q + rnd) as i32
    }

    #[test]
    fn dead_zone_threshold_is_exact() {
        let mut state = 0x1234_5678_9abc_def0u64;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        for table in AC_QLOOKUP.iter().chain(DC_QLOOKUP.iter()) {
            for &qi in table.iter() {
                let q = qi as f32;
                for rnd in [INTER_RND, INTRA_RND] {
                    let thr = zero_threshold(q, rnd);
                    // the boundary itself and its neighbours
                    let b = thr.to_bits();
                    for bits in b.saturating_sub(3)..=b + 3 {
                        let cf = f32::from_bits(bits);
                        assert_eq!(quant_ref(cf, q, rnd), quant_new(cf, q, rnd, thr), "q {q} rnd {rnd} cf {cf}");
                        assert_eq!(quant_ref(-cf, q, rnd), quant_new(-cf, q, rnd, thr));
                    }
                    for _ in 0..200 {
                        let cf = f32::from_bits((next() >> 32) as u32 & 0x7fff_ffff);
                        if cf.is_nan() {
                            continue;
                        }
                        assert_eq!(quant_ref(cf, q, rnd), quant_new(cf, q, rnd, thr));
                        let cf = (next() % 100_000) as f32 / 100.0;
                        assert_eq!(quant_ref(cf, q, rnd), quant_new(cf, q, rnd, thr));
                    }
                }
            }
        }
    }

    #[test]
    fn log2_table_matches_direct_log2() {
        let t = log2_table();
        for v in 1..3000usize {
            assert_eq!(log2_of(t, v).to_bits(), (v as f64).log2().to_bits(), "{v}");
        }
    }
}

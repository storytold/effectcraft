//! Picture coding: CTU loop, rate-distortion mode decisions (with a CABAC bit estimator) and the
//! slice_segment_data() syntax (7.3.8): coding quadtree, coding unit, prediction unit, transform tree
//! and residual coding. Reconstruction mirrors the decoding process exactly (no in-loop filters).

use crate::cabac::{BIT, CabacEncoder, Estimator, Sink};
use crate::inter::{self, PlaneRef};
use crate::intra::{self, Refs};
use crate::me::MeRef;
use crate::params::{LOG2_CTB, LOG2_MAX_TB, LOG2_MIN_CB, MAX_MERGE_CAND};
use crate::tables::{
    ABS_MVD_GT0, ABS_MVD_GT1, CBF_CHROMA, CBF_LUMA, CODED_SUB_BLOCK, CTX_IDX_MAP, CU_SKIP, GT1, GT2, INTRA_CHROMA, LAST_X_PREFIX, LAST_Y_PREFIX, MERGE_FLAG,
    MERGE_IDX, MVP_FLAG, PART_MODE, PRED_MODE, PREV_INTRA_LUMA, RQT_ROOT_CBF, SCAN_4, SIG_COEFF, SPLIT_CU, qpc_420, scan_order,
};
use crate::transform;

pub const F_INTRA: u8 = 1;
pub const F_SKIP: u8 = 2;
/// The luma transform block covering this 4x4 block has non-zero coefficients.
pub const F_CBF: u8 = 4;
/// The left / top edge of this 4x4 block is a transform (and prediction) block edge.
pub const E_EDGE_V: u8 = 8;
pub const E_EDGE_H: u8 = 16;

/// Per 4x4 luma block state of the current picture.
#[derive(Clone, Copy, Default)]
pub struct Blk {
    pub depth: u8,
    pub flags: u8,
    /// IntraPredModeY (DC for inter blocks).
    pub ipm: u8,
    /// L0 motion vector (quarter samples) of inter blocks.
    pub mv: [i16; 2],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CuMode {
    Intra { nxn: bool, modes: [u8; 4] },
    Skip { idx: u8 },
    Merge { idx: u8 },
    Amvp { mv: [i16; 2], mvp: u8 },
}

#[derive(Clone, Copy, Debug)]
struct CuDec {
    x: u32,
    y: u32,
    log2: u32,
    mode: CuMode,
}

/// One coded transform block of the current CU.
#[derive(Clone, Copy)]
struct Tb {
    c: u8,
    /// Luma-coordinate region the block belongs to (for cbf derivation).
    lx: u32,
    ly: u32,
    log2: u32,
    nz: bool,
    off: usize,
    scan: u8,
}

/// Saved samples and block state of a square region.
struct Snap {
    x: usize,
    y: usize,
    n: usize,
    planes: [Vec<u16>; 3],
    blk: Vec<Blk>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SliceKind {
    I,
    P,
}

/// Picture geometry and slice coding parameters shared by the CTU encoder.
pub struct FrameConfig {
    pub bit_depth: u32,
    pub width: usize,
    pub height: usize,
    pub kind: SliceKind,
    pub slice_qp: i32,
}

/// A square node in the coding or transform quadtree.
#[derive(Clone, Copy)]
struct TreeNode {
    x: usize,
    y: usize,
    log2: u32,
    depth: u32,
}

impl TreeNode {
    fn children(self) -> impl Iterator<Item = Self> {
        let h = 1usize << (self.log2 - 1);
        [(0, 0), (h, 0), (0, h), (h, h)].into_iter().map(move |(dx, dy)| Self { x: self.x + dx, y: self.y + dy, log2: self.log2 - 1, depth: self.depth + 1 })
    }
}

/// Transform syntax inherited from the parent node and the enclosing CU.
#[derive(Clone, Copy)]
struct TransformState {
    blk_idx: u32,
    parent: [bool; 2],
    intra: bool,
    nxn: bool,
}

pub struct FrameCoder<'a> {
    pub bd: u32,
    pub w: usize,
    pub h: usize,
    pub cw: usize,
    pub ch: usize,
    w4: usize,
    wctb: usize,
    hctb: usize,
    pub kind: SliceKind,
    /// QP' (with QpBdOffset) for luma and chroma.
    slice_qp: i32,
    qp_y: i32,
    qp_c: i32,
    lambda: f64,
    lambda_sad: f64,
    src: &'a [Vec<u16>; 3],
    pub rec: [Vec<u16>; 3],
    refp: Option<&'a [Vec<u16>; 3]>,
    me: Option<&'a MeRef>,
    blk: Vec<Blk>,
    tbs: Vec<Tb>,
    coefs: Vec<i32>,
    pred: Vec<i16>,
    scratch: [Vec<i32>; 2],
}

#[inline]
fn morton(x: u32, y: u32) -> u32 {
    let mut r = 0;
    for i in 0..5 {
        r |= ((x >> i) & 1) << (2 * i);
        r |= ((y >> i) & 1) << (2 * i + 1);
    }
    r
}

/// Intra modes tried in the coarse pass of the SATD search.
const COARSE_MODES: [u32; 11] = [0, 1, 2, 6, 10, 14, 18, 22, 26, 30, 34];

impl<'a> FrameCoder<'a> {
    pub fn new(config: FrameConfig, src: &'a [Vec<u16>; 3], refp: Option<&'a [Vec<u16>; 3]>, me: Option<&'a MeRef>) -> Self {
        let FrameConfig { bit_depth: bd, width: w, height: h, kind, slice_qp } = config;
        let off = 6 * (bd as i32 - 8);
        let qpi = slice_qp.clamp(-off, 57);
        let qp_c = qpc_420(qpi) + off;
        let base = if kind == SliceKind::I { 0.57 } else { 0.62 };
        let lambda = base * 2f64.powf((slice_qp as f64 - 12.0) / 3.0) * (1u64 << (2 * (bd - 8))) as f64;
        let ctb = 1usize << LOG2_CTB;
        FrameCoder {
            bd,
            w,
            h,
            cw: w / 2,
            ch: h / 2,
            w4: w / 4,
            wctb: w.div_ceil(ctb),
            hctb: h.div_ceil(ctb),
            kind,
            slice_qp,
            qp_y: slice_qp + off,
            qp_c,
            lambda,
            lambda_sad: lambda.sqrt(),
            src,
            rec: [vec![0; w * h], vec![0; w * h / 4], vec![0; w * h / 4]],
            refp,
            me,
            blk: vec![Blk::default(); (w / 4) * (h / 4)],
            tbs: Vec::with_capacity(16),
            coefs: Vec::with_capacity(64 * 64 * 2),
            pred: vec![0; 64 * 64],
            scratch: [vec![0; 32 * 32], vec![0; 32 * 32]],
        }
    }

    // ---------------------------------------------------------------------------------------
    // neighbourhood

    /// z-scan order availability (6.4.1) of (xn, yn) for the block at (xc, yc): single slice, no tiles.
    #[inline]
    fn zavail(&self, xc: i32, yc: i32, xn: i32, yn: i32) -> bool {
        if xn < 0 || yn < 0 || xn >= self.w as i32 || yn >= self.h as i32 {
            return false;
        }
        let s = LOG2_CTB;
        let ctb_n = (yn >> s) as usize * self.wctb + (xn >> s) as usize;
        let ctb_c = (yc >> s) as usize * self.wctb + (xc >> s) as usize;
        if ctb_n != ctb_c {
            ctb_n < ctb_c
        } else {
            let m = (1 << s) - 1;
            morton(((xn & m) >> 2) as u32, ((yn & m) >> 2) as u32) <= morton(((xc & m) >> 2) as u32, ((yc & m) >> 2) as u32)
        }
    }

    #[inline]
    fn blk_at(&self, x: i32, y: i32) -> &Blk {
        &self.blk[(y as usize >> 2) * self.w4 + (x as usize >> 2)]
    }

    fn fill(&mut self, x: usize, y: usize, n: usize, f: impl Fn(&mut Blk)) {
        for by in (y >> 2)..((y + n) >> 2) {
            for b in &mut self.blk[by * self.w4 + (x >> 2)..by * self.w4 + ((x + n) >> 2)] {
                f(b);
            }
        }
    }

    /// Mark the CU's left / top edges and, for CUs larger than the maximum transform size, the
    /// inner transform edges (deblocking).
    fn mark_edges(&mut self, x: usize, y: usize, log2: u32) {
        let n = 1usize << log2;
        let t = 1usize << log2.min(LOG2_MAX_TB);
        for ty in (y..y + n).step_by(t) {
            for tx in (x..x + n).step_by(t) {
                for by in ty / 4..(ty + t) / 4 {
                    self.blk[by * self.w4 + tx / 4].flags |= E_EDGE_V;
                }
                for b in &mut self.blk[(ty / 4) * self.w4 + tx / 4..(ty / 4) * self.w4 + (tx + t) / 4] {
                    b.flags |= E_EDGE_H;
                }
            }
        }
    }

    /// In-loop deblocking of the finished picture.
    pub fn deblock(&mut self) {
        crate::deblock::deblock(&mut self.rec, &self.blk, self.w, self.h, self.slice_qp, self.bd);
    }

    fn snap(&self, x: usize, y: usize, n: usize) -> Snap {
        let mut planes: [Vec<u16>; 3] = Default::default();
        for (c, p) in planes.iter_mut().enumerate() {
            let (px, py, pn, stride) = if c == 0 { (x, y, n, self.w) } else { (x / 2, y / 2, n / 2, self.cw) };
            p.reserve(pn * pn);
            for r in 0..pn {
                p.extend_from_slice(&self.rec[c][(py + r) * stride + px..][..pn]);
            }
        }
        let mut blk = Vec::with_capacity((n / 4) * (n / 4));
        for by in (y >> 2)..((y + n) >> 2) {
            blk.extend_from_slice(&self.blk[by * self.w4 + (x >> 2)..by * self.w4 + ((x + n) >> 2)]);
        }
        Snap { x, y, n, planes, blk }
    }

    fn restore(&mut self, s: &Snap) {
        let (x, y, n) = (s.x, s.y, s.n);
        for c in 0..3 {
            let (px, py, pn, stride) = if c == 0 { (x, y, n, self.w) } else { (x / 2, y / 2, n / 2, self.cw) };
            for r in 0..pn {
                self.rec[c][(py + r) * stride + px..][..pn].copy_from_slice(&s.planes[c][r * pn..r * pn + pn]);
            }
        }
        let bw = n / 4;
        for (i, by) in ((y >> 2)..((y + n) >> 2)).enumerate() {
            self.blk[by * self.w4 + (x >> 2)..by * self.w4 + (x >> 2) + bw].copy_from_slice(&s.blk[i * bw..i * bw + bw]);
        }
    }

    // ---------------------------------------------------------------------------------------
    // picture loop

    /// Code all CTUs of the picture into `enc` (one slice); the caller flushes the encoder.
    pub fn encode(&mut self, enc: &mut CabacEncoder) {
        let ctb = 1usize << LOG2_CTB;
        let total = self.wctb * self.hctb;
        for i in 0..total {
            let (x, y) = ((i % self.wctb) * ctb, (i / self.wctb) * ctb);
            let mut est = Estimator::new(enc.ctx);
            let mut decs = Vec::with_capacity(64);
            self.decide(x, y, LOG2_CTB, 0, &mut est, &mut decs);
            let mut idx = 0;
            self.write_cq(enc, TreeNode { x, y, log2: LOG2_CTB, depth: 0 }, &decs, &mut idx);
            debug_assert_eq!(idx, decs.len());
            debug_assert_eq!(est.ctx, enc.ctx);
            enc.terminate((i + 1 == total) as u32); // end_of_slice_segment_flag
        }
    }

    fn write_split<S: Sink>(&self, s: &mut S, x: usize, y: usize, depth: u32, split: bool) {
        let (x, y) = (x as i32, y as i32);
        let mut inc = 0;
        if self.zavail(x, y, x - 1, y) && self.blk_at(x - 1, y).depth as u32 > depth {
            inc += 1;
        }
        if self.zavail(x, y, x, y - 1) && self.blk_at(x, y - 1).depth as u32 > depth {
            inc += 1;
        }
        s.decision(SPLIT_CU + inc, split as u32);
    }

    /// Final pass: re-code the decided quadtree with the real arithmetic coder.
    fn write_cq(&mut self, enc: &mut CabacEncoder, node: TreeNode, decs: &[CuDec], idx: &mut usize) {
        let TreeNode { x, y, log2, depth } = node;
        if x >= self.w || y >= self.h {
            return;
        }
        let n = 1usize << log2;
        let inside = x + n <= self.w && y + n <= self.h;
        let d = decs[*idx];
        let leaf = d.x as usize == x && d.y as usize == y && d.log2 == log2;
        if inside && log2 > LOG2_MIN_CB {
            self.write_split(enc, x, y, depth, !leaf);
        } else {
            debug_assert_eq!(leaf, log2 == LOG2_MIN_CB);
        }
        if leaf {
            *idx += 1;
            let (eff, _) = self.encode_cu(enc, x, y, log2, depth, d.mode);
            debug_assert_eq!(eff, d.mode);
        } else {
            for child in node.children() {
                self.write_cq(enc, child, decs, idx);
            }
        }
    }

    #[inline]
    fn cost(&self, dist: u64, bits: u64) -> f64 {
        dist as f64 + self.lambda * bits as f64 / BIT as f64
    }

    /// Rate-distortion decision for the quadtree node at (x, y); leaves the chosen reconstruction
    /// and block state in place, advances `est` and appends the decided CUs. Returns the RD cost.
    fn decide(&mut self, x: usize, y: usize, log2: u32, depth: u32, est: &mut Estimator, decs: &mut Vec<CuDec>) -> f64 {
        if x >= self.w || y >= self.h {
            return 0.0;
        }
        let n = 1usize << log2;
        let inside = x + n <= self.w && y + n <= self.h;
        let can_split = log2 > LOG2_MIN_CB;
        let base = est.bits;
        let leaf_ok = inside && (self.kind == SliceKind::P || log2 <= LOG2_MAX_TB);
        let mut best: Option<(f64, Estimator, Snap, CuMode)> = None;
        if leaf_ok {
            let mut e0 = est.clone();
            if can_split {
                self.write_split(&mut e0, x, y, depth, false);
            }
            let try_mode = |me: &mut Self, mode: CuMode, best: &mut Option<(f64, Estimator, Snap, CuMode)>| {
                let mut e = e0.clone();
                let (eff, dist) = me.encode_cu(&mut e, x, y, log2, depth, mode);
                let cost = me.cost(dist, e.bits - base);
                if best.as_ref().is_none_or(|b| cost < b.0) {
                    *best = Some((cost, e, me.snap(x, y, n), eff));
                }
            };
            let mut inter_satd = u64::MAX;
            if self.kind == SliceKind::P {
                let merge = self.merge_list(x, y, n);
                let (midx, msatd) = self.best_merge(x, y, n, &merge);
                try_mode(self, CuMode::Skip { idx: midx as u8 }, &mut best);
                try_mode(self, CuMode::Merge { idx: midx as u8 }, &mut best);
                let amvp = self.amvp(x, y, n);
                let (mv, mvp, asatd) = self.motion_search(x, y, n, &amvp, &merge);
                if mv != merge[midx] {
                    try_mode(self, CuMode::Amvp { mv, mvp }, &mut best);
                }
                inter_satd = msatd.min(asatd);
            }
            if log2 <= LOG2_MAX_TB && !matches!(best, Some((_, _, _, CuMode::Skip { .. }))) {
                let (mode, satd) = self.search_intra_mode(x, y, log2);
                if satd < inter_satd {
                    try_mode(self, CuMode::Intra { nxn: false, modes: [mode as u8; 4] }, &mut best);
                    if log2 == LOG2_MIN_CB {
                        let modes = self.search_nxn_modes(x, y);
                        try_mode(self, CuMode::Intra { nxn: true, modes }, &mut best);
                    }
                }
            }
            if let Some(b) = &best {
                self.restore(&b.2);
            }
        }
        let early = matches!(best, Some((_, _, _, CuMode::Skip { .. })));
        if can_split && (!leaf_ok || !early) {
            let mut e = est.clone();
            if inside {
                self.write_split(&mut e, x, y, depth, true);
            }
            let limit = best.as_ref().map_or(f64::INFINITY, |b| b.0);
            let mut cost = self.cost(0, e.bits - base);
            let mut d2 = Vec::new();
            let h = n / 2;
            for (dx, dy) in [(0, 0), (h, 0), (0, h), (h, h)] {
                cost += self.decide(x + dx, y + dy, log2 - 1, depth + 1, &mut e, &mut d2);
                if cost >= limit {
                    break;
                }
            }
            if cost < limit {
                *est = e;
                decs.extend(d2);
                return cost;
            }
        }
        // A coding unit always has at least one candidate mode (intra, or skip/merge in P
        // slices); without one, report an infinite cost.
        let Some((cost, e, snap, mode)) = best else { return f64::INFINITY };
        self.restore(&snap);
        *est = e;
        decs.push(CuDec { x: x as u32, y: y as u32, log2, mode });
        cost
    }

    // ---------------------------------------------------------------------------------------
    // coding unit

    fn skip_ctx(&self, x: usize, y: usize) -> usize {
        let (x, y) = (x as i32, y as i32);
        let mut inc = 0;
        if self.zavail(x, y, x - 1, y) && self.blk_at(x - 1, y).flags & F_SKIP != 0 {
            inc += 1;
        }
        if self.zavail(x, y, x, y - 1) && self.blk_at(x, y - 1).flags & F_SKIP != 0 {
            inc += 1;
        }
        inc
    }

    /// Predict, transform, quantise and reconstruct one CU and write its syntax into `s`.
    /// Returns the effective mode (a residual-free merge becomes a skip) and the SSE.
    fn encode_cu<S: Sink>(&mut self, s: &mut S, x: usize, y: usize, log2: u32, depth: u32, mode: CuMode) -> (CuMode, u64) {
        let n = 1usize << log2;
        self.tbs.clear();
        self.coefs.clear();
        let p_slice = self.kind == SliceKind::P;
        let skip_ctx = if p_slice { self.skip_ctx(x, y) } else { 0 };
        let d8 = depth as u8;
        match mode {
            CuMode::Intra { nxn, modes } => {
                self.fill(x, y, n, |b| {
                    b.depth = d8;
                    b.flags = F_INTRA;
                    b.ipm = modes[0];
                });
                self.mark_edges(x, y, log2);
                if nxn {
                    let h = n / 2;
                    for k in 1..4 {
                        let m = modes[k];
                        self.fill(x + (k & 1) * h, y + (k >> 1) * h, h, |b| b.ipm = m);
                    }
                    for k in 0..4 {
                        let (px, py) = (x + (k & 1) * h, y + (k >> 1) * h);
                        self.code_tb(0, px, py, 2, Some(modes[k] as u32), (x, y));
                    }
                    for c in 1..3 {
                        self.code_tb(c, x / 2, y / 2, 2, Some(modes[0] as u32), (x, y));
                    }
                } else {
                    self.code_tus(x, y, log2, Some(modes[0] as u32));
                }
                if p_slice {
                    s.decision(CU_SKIP + skip_ctx, 0);
                    s.decision(PRED_MODE, 1);
                }
                if log2 == LOG2_MIN_CB {
                    s.decision(PART_MODE, (!nxn) as u32);
                }
                self.write_intra_modes(s, x, y, n, nxn, &modes);
                s.decision(INTRA_CHROMA, 0); // intra_chroma_pred_mode 4 (DM)
                let mut ti = 0;
                self.write_tt(s, TreeNode { x, y, log2, depth: 0 }, TransformState { blk_idx: 0, parent: [true, true], intra: true, nxn }, &mut ti);
                (mode, self.cu_dist(x, y, n))
            }
            _ => {
                let mv = match mode {
                    CuMode::Skip { idx } | CuMode::Merge { idx } => self.merge_list(x, y, n)[idx as usize],
                    CuMode::Amvp { mv, .. } => mv,
                    // Handled by the arm above.
                    CuMode::Intra { .. } => [0, 0],
                };
                let amvp = if let CuMode::Amvp { .. } = mode { self.amvp(x, y, n) } else { [[0; 2]; 2] };
                let skip = matches!(mode, CuMode::Skip { .. });
                self.fill(x, y, n, |b| {
                    b.depth = d8;
                    b.flags = if skip { F_SKIP } else { 0 };
                    b.ipm = 1;
                    b.mv = mv;
                });
                self.mark_edges(x, y, log2);
                self.predict_inter(x, y, n, mv);
                let mut eff = mode;
                let mut root = false;
                if !skip {
                    self.code_tus(x, y, log2, None);
                    root = self.tbs.iter().any(|t| t.nz);
                    if !root && let CuMode::Merge { idx } = mode {
                        // rqt_root_cbf would be inferred: code a skipped CU instead
                        eff = CuMode::Skip { idx };
                        self.fill(x, y, n, |b| b.flags |= F_SKIP);
                    }
                }
                match eff {
                    CuMode::Skip { idx } => {
                        s.decision(CU_SKIP + skip_ctx, 1);
                        write_merge_idx(s, idx as usize);
                    }
                    CuMode::Merge { idx } => {
                        s.decision(CU_SKIP + skip_ctx, 0);
                        s.decision(PRED_MODE, 0);
                        s.decision(PART_MODE, 1); // PART_2Nx2N
                        s.decision(MERGE_FLAG, 1);
                        write_merge_idx(s, idx as usize);
                        let mut ti = 0;
                        self.write_tt(
                            s,
                            TreeNode { x, y, log2, depth: 0 },
                            TransformState { blk_idx: 0, parent: [true, true], intra: false, nxn: false },
                            &mut ti,
                        );
                    }
                    CuMode::Amvp { mv, mvp } => {
                        s.decision(CU_SKIP + skip_ctx, 0);
                        s.decision(PRED_MODE, 0);
                        s.decision(PART_MODE, 1);
                        s.decision(MERGE_FLAG, 0);
                        let p = amvp[mvp as usize];
                        write_mvd(s, [mv[0] as i32 - p[0] as i32, mv[1] as i32 - p[1] as i32]);
                        s.decision(MVP_FLAG, mvp as u32);
                        s.decision(RQT_ROOT_CBF, root as u32);
                        if root {
                            let mut ti = 0;
                            self.write_tt(
                                s,
                                TreeNode { x, y, log2, depth: 0 },
                                TransformState { blk_idx: 0, parent: [true, true], intra: false, nxn: false },
                                &mut ti,
                            );
                        }
                    }
                    // Handled by the arm above.
                    CuMode::Intra { .. } => {}
                }
                (eff, self.cu_dist(x, y, n))
            }
        }
    }

    /// Transform units of a 2Nx2N CU (split to the maximum TB size), luma then Cb, Cr per TU.
    fn code_tus(&mut self, x: usize, y: usize, log2: u32, intra_mode: Option<u32>) {
        let tl = log2.min(LOG2_MAX_TB);
        let t = 1usize << tl;
        let k = 1usize << (log2 - tl);
        for i in 0..k * k {
            // z-order of at most 2x2 TUs
            let (tx, ty) = (x + (i & 1) * t, y + (i >> 1) * t);
            self.code_tb(0, tx, ty, tl, intra_mode, (tx, ty));
            self.code_tb(1, tx / 2, ty / 2, tl - 1, intra_mode, (tx, ty));
            self.code_tb(2, tx / 2, ty / 2, tl - 1, intra_mode, (tx, ty));
        }
    }

    fn cu_dist(&self, x: usize, y: usize, n: usize) -> u64 {
        let mut d = 0u64;
        for c in 0..3 {
            let (px, py, pn, stride) = if c == 0 { (x, y, n, self.w) } else { (x / 2, y / 2, n / 2, self.cw) };
            for r in 0..pn {
                let o = (py + r) * stride + px;
                let a = &self.src[c][o..o + pn];
                let b = &self.rec[c][o..o + pn];
                d += a.iter().zip(b).map(|(&p, &q)| (p as i64 - q as i64).pow(2) as u64).sum::<u64>();
            }
        }
        d
    }

    /// Code one transform block of component `c` at component position (x, y): optional intra
    /// prediction, residual, transform, quantisation and reconstruction.
    fn code_tb(&mut self, c: usize, x: usize, y: usize, log2: u32, intra_mode: Option<u32>, lpos: (usize, usize)) {
        let n = 1usize << log2;
        let stride = if c == 0 { self.w } else { self.cw };
        if let Some(m) = intra_mode {
            let refs = self.build_refs(c, x, y, log2);
            let bd = self.bd;
            predict_from_refs(&refs, c, m, n, bd, &mut self.rec[c][y * stride + x..], stride);
        }
        let mut scr = std::mem::take(&mut self.scratch);
        let [r, coef] = &mut scr;
        for yy in 0..n {
            let o = (y + yy) * stride + x;
            for xx in 0..n {
                r[yy * n + xx] = self.src[c][o + xx] as i32 - self.rec[c][o + xx] as i32;
            }
        }
        let intra = intra_mode.is_some();
        let dst = intra && c == 0 && n == 4;
        transform::forward(&r[..n * n], &mut coef[..n * n], n, dst, self.bd);
        let qp = if c == 0 { self.qp_y } else { self.qp_c };
        let off = self.coefs.len();
        self.coefs.resize(off + n * n, 0);
        let lv = &mut self.coefs[off..off + n * n];
        let scan = match intra_mode {
            Some(m) if log2 == 2 || (log2 == 3 && c == 0) => scan_idx_for_mode(m),
            _ => 0,
        };
        let nz = transform::quantize(&coef[..n * n], lv, n, qp, intra, self.bd) > 0;
        if nz {
            transform::sign_hiding(&coef[..n * n], lv, log2, scan as usize, qp, self.bd);
        }
        if nz {
            let d = &mut r[..n * n];
            transform::dequantize(lv, d, n, qp, self.bd);
            transform::inverse(d, n, dst, self.bd);
            let max = (1i32 << self.bd) - 1;
            for yy in 0..n {
                let o = (y + yy) * stride + x;
                for xx in 0..n {
                    let v = &mut self.rec[c][o + xx];
                    *v = (*v as i32 + d[yy * n + xx]).clamp(0, max) as u16;
                }
            }
        }
        self.scratch = scr;
        if nz && c == 0 {
            self.fill(x, y, n, |b| b.flags |= F_CBF);
        }
        self.tbs.push(Tb { c: c as u8, lx: lpos.0 as u32, ly: lpos.1 as u32, log2, nz, off, scan });
    }

    /// Reference samples (substituted, unfiltered) of an intra block, as in 8.4.4.2.1 / 8.4.4.2.2.
    fn build_refs(&self, c: usize, x0: usize, y0: usize, log2: u32) -> Refs {
        let n = 1usize << log2;
        let sc = if c == 0 { 1 } else { 2 };
        let (x0, y0) = (x0 as i32, y0 as i32);
        let (xl, yl) = (x0 * sc, y0 * sc);
        let unit = 4 / sc as usize;
        let stride = if c == 0 { self.w } else { self.cw };
        let mut al = [false; 129];
        let mut at = [false; 129];
        let plane = &self.rec[c];
        let mut r = Refs::default();
        if self.zavail(xl, yl, xl - 1, yl - 1) {
            al[0] = true;
            at[0] = true;
            let v = plane[(y0 - 1) as usize * stride + (x0 - 1) as usize];
            r.left[0] = v;
            r.top[0] = v;
        }
        let mut k = 0;
        while k < 2 * n {
            let yn = y0 + k as i32;
            if self.zavail(xl, yl, xl - 1, yn * sc) {
                for i in 0..unit {
                    al[1 + k + i] = true;
                    r.left[1 + k + i] = plane[(yn as usize + i) * stride + (x0 - 1) as usize];
                }
            }
            let xn = x0 + k as i32;
            if self.zavail(xl, yl, xn * sc, yl - 1) {
                let o = (y0 - 1) as usize * stride + xn as usize;
                for i in 0..unit {
                    at[1 + k + i] = true;
                    r.top[1 + k + i] = plane[o + i];
                }
            }
            k += unit;
        }
        intra::substitute(&mut r, &al, &at, n, self.bd);
        r
    }

    // ---------------------------------------------------------------------------------------
    // syntax writers

    /// Candidate modes candModeList (8.4.2) of the prediction block at (xp, yp).
    fn mpm(&self, xp: usize, yp: usize) -> [u32; 3] {
        let (xp, yp) = (xp as i32, yp as i32);
        let cand_a = if self.zavail(xp, yp, xp - 1, yp) {
            let b = self.blk_at(xp - 1, yp);
            if b.flags & F_INTRA != 0 { b.ipm as u32 } else { 1 }
        } else {
            1
        };
        let cand_b = if self.zavail(xp, yp, xp, yp - 1) && yp > ((yp >> LOG2_CTB) << LOG2_CTB) {
            let b = self.blk_at(xp, yp - 1);
            if b.flags & F_INTRA != 0 { b.ipm as u32 } else { 1 }
        } else {
            1
        };
        if cand_a == cand_b {
            if cand_a < 2 { [0, 1, 26] } else { [cand_a, 2 + ((cand_a + 29) % 32), 2 + ((cand_a - 2 + 1) % 32)] }
        } else {
            let c = if cand_a != 0 && cand_b != 0 {
                0
            } else if cand_a != 1 && cand_b != 1 {
                1
            } else {
                26
            };
            [cand_a, cand_b, c]
        }
    }

    fn write_intra_modes<S: Sink>(&self, s: &mut S, x: usize, y: usize, n: usize, nxn: bool, modes: &[u8; 4]) {
        let nb = if nxn { 2 } else { 1 };
        let pb = n / nb;
        let mut codes = [(false, 0u32); 4];
        for j in 0..nb {
            for i in 0..nb {
                let k = j * nb + i;
                let list = self.mpm(x + i * pb, y + j * pb);
                let m = modes[k] as u32;
                codes[k] = match list.iter().position(|&c| c == m) {
                    Some(idx) => (true, idx as u32),
                    None => {
                        let mut sorted = list;
                        sorted.sort_unstable();
                        let mut r = m;
                        for &c in sorted.iter().rev() {
                            if r > c {
                                r -= 1;
                            }
                        }
                        (false, r)
                    }
                };
            }
        }
        for code in codes.iter().take(nb * nb) {
            s.decision(PREV_INTRA_LUMA, code.0 as u32);
        }
        for &(prev, v) in codes.iter().take(nb * nb) {
            if prev {
                s.bypass((v > 0) as u32);
                if v > 0 {
                    s.bypass((v > 1) as u32);
                }
            } else {
                s.bypass_bits(v, 5);
            }
        }
    }

    fn chroma_cbf(&self, c: u8, x0: usize, y0: usize, n: usize) -> bool {
        self.tbs.iter().any(|t| t.c == c && t.nz && (t.lx as usize) >= x0 && (t.lx as usize) < x0 + n && (t.ly as usize) >= y0 && (t.ly as usize) < y0 + n)
    }

    /// transform_tree() (7.3.8.8) over the coded transform blocks of the CU; no split flag is ever
    /// coded (max_transform_hierarchy_depth_* = 0), splits are inferred.
    fn write_tt<S: Sink>(&self, s: &mut S, node: TreeNode, state: TransformState, ti: &mut usize) {
        let TreeNode { x: x0, y: y0, log2, depth } = node;
        let TransformState { blk_idx, parent, intra, nxn } = state;
        let n = 1usize << log2;
        let split = log2 > LOG2_MAX_TB || (nxn && depth == 0);
        let mut cbf_c = [false; 2];
        if log2 > 2 {
            for c in 0..2 {
                if depth == 0 || parent[c] {
                    cbf_c[c] = self.chroma_cbf(c as u8 + 1, x0, y0, n);
                    s.decision(CBF_CHROMA + depth as usize, cbf_c[c] as u32);
                }
            }
        } else if depth > 0 {
            cbf_c = parent;
        }
        if split {
            for (i, child) in node.children().enumerate() {
                self.write_tt(s, child, TransformState { blk_idx: i as u32, parent: cbf_c, ..state }, ti);
            }
            return;
        }
        let tb = self.tbs[*ti];
        *ti += 1;
        debug_assert_eq!(tb.c, 0);
        if intra || depth != 0 || cbf_c[0] || cbf_c[1] {
            s.decision(CBF_LUMA + (depth == 0) as usize, tb.nz as u32);
        } else {
            debug_assert!(tb.nz, "inferred cbf_luma needs coefficients");
        }
        if tb.nz {
            let n2 = 1usize << (2 * tb.log2);
            residual_coding(s, &self.coefs[tb.off..tb.off + n2], tb.log2, 0, tb.scan as usize);
        }
        if log2 > 2 || blk_idx == 3 {
            for c in 0..2 {
                let tb = self.tbs[*ti];
                *ti += 1;
                debug_assert_eq!(tb.c as usize, c + 1);
                debug_assert_eq!(tb.nz, cbf_c[c]);
                if cbf_c[c] {
                    let n2 = 1usize << (2 * tb.log2);
                    residual_coding(s, &self.coefs[tb.off..tb.off + n2], tb.log2, c + 1, tb.scan as usize);
                }
            }
        }
    }

    // ---------------------------------------------------------------------------------------
    // inter prediction

    /// Prediction block availability (6.4.2) for a 2Nx2N CU: available and not intra.
    #[inline]
    fn pb_avail(&self, xp: i32, yp: i32, xn: i32, yn: i32) -> bool {
        self.zavail(xp, yp, xn, yn) && self.blk_at(xn, yn).flags & F_INTRA == 0
    }

    /// Merge candidate list (8.5.3.2.2 - 8.5.3.2.5) of a 2Nx2N P-slice CU: spatial candidates and
    /// zero candidates (no temporal candidate, one reference picture).
    pub fn merge_list(&self, x: usize, y: usize, n: usize) -> [[i16; 2]; MAX_MERGE_CAND] {
        let (xp, yp, w, h) = (x as i32, y as i32, n as i32, n as i32);
        let mv = |p: (i32, i32)| self.blk_at(p.0, p.1).mv;
        let mut list = [[0i16; 2]; MAX_MERGE_CAND];
        let mut k = 0;
        let pa1 = (xp - 1, yp + h - 1);
        let av_a1 = self.pb_avail(xp, yp, pa1.0, pa1.1);
        if av_a1 {
            list[k] = mv(pa1);
            k += 1;
        }
        let pb1 = (xp + w - 1, yp - 1);
        let av_b1 = self.pb_avail(xp, yp, pb1.0, pb1.1);
        let fl_b1 = av_b1 && !(av_a1 && mv(pa1) == mv(pb1));
        if fl_b1 {
            list[k] = mv(pb1);
            k += 1;
        }
        let pb0 = (xp + w, yp - 1);
        let fl_b0 = self.pb_avail(xp, yp, pb0.0, pb0.1) && !(av_b1 && mv(pb1) == mv(pb0));
        if fl_b0 {
            list[k] = mv(pb0);
            k += 1;
        }
        let pa0 = (xp - 1, yp + h);
        let fl_a0 = self.pb_avail(xp, yp, pa0.0, pa0.1) && !(av_a1 && mv(pa1) == mv(pa0));
        if fl_a0 {
            list[k] = mv(pa0);
            k += 1;
        }
        let pb2 = (xp - 1, yp - 1);
        let fl_b2 = self.pb_avail(xp, yp, pb2.0, pb2.1)
            && !((av_a1 && mv(pa1) == mv(pb2)) || (av_b1 && mv(pb1) == mv(pb2)))
            && (fl_a0 as u32 + av_a1 as u32 + fl_b0 as u32 + fl_b1 as u32) != 4;
        if fl_b2 {
            list[k] = mv(pb2);
        }
        // remaining entries are zero candidates with refIdx 0
        list
    }

    /// AMVP candidates (8.5.3.2.6, 8.5.3.2.7) of a 2Nx2N CU with one reference picture.
    pub fn amvp(&self, x: usize, y: usize, n: usize) -> [[i16; 2]; 2] {
        let (xp, yp, w, h) = (x as i32, y as i32, n as i32, n as i32);
        let pa = [(xp - 1, yp + h), (xp - 1, yp + h - 1)];
        let av_a = [self.pb_avail(xp, yp, pa[0].0, pa[0].1), self.pb_avail(xp, yp, pa[1].0, pa[1].1)];
        let is_scaled = av_a[0] || av_a[1];
        let mut a = None;
        for k in 0..2 {
            if av_a[k] && a.is_none() {
                a = Some(self.blk_at(pa[k].0, pa[k].1).mv);
            }
        }
        let pb = [(xp + w, yp - 1), (xp + w - 1, yp - 1), (xp - 1, yp - 1)];
        let mut b = None;
        for p in pb {
            if b.is_none() && self.pb_avail(xp, yp, p.0, p.1) {
                b = Some(self.blk_at(p.0, p.1).mv);
            }
        }
        if !is_scaled && b.is_some() {
            a = b;
        }
        let mut list = [[0i16; 2]; 2];
        match (a, b) {
            (Some(a), Some(b)) => {
                list[0] = a;
                if b != a {
                    list[1] = b;
                }
            }
            (Some(a), None) => list[0] = a,
            (None, Some(b)) => list[0] = b,
            (None, None) => {}
        }
        list
    }

    fn predict_inter(&mut self, x: usize, y: usize, n: usize, mv: [i16; 2]) {
        // Inter modes are only tried in P slices, which have a reference picture.
        let Some(refp) = self.refp else { return };
        for c in 0..3 {
            let (px, py, pn, pw, ph) = if c == 0 { (x, y, n, self.w, self.h) } else { (x / 2, y / 2, n / 2, self.cw, self.ch) };
            let pr = PlaneRef { data: &refp[c], w: pw, h: ph };
            if c == 0 {
                inter::mc_luma(pr, self.bd, px as i32, py as i32, mv, (pn, pn), &mut self.pred);
            } else {
                inter::mc_chroma(pr, self.bd, px as i32, py as i32, mv, (pn, pn), &mut self.pred);
            }
            inter::put_uni(&self.pred, pn, pn, self.bd, &mut self.rec[c][py * pw + px..], pw);
        }
    }

    // ---------------------------------------------------------------------------------------
    // estimation helpers

    /// Best merge index by SATD of the (approximate) luma prediction.
    fn best_merge(&self, x: usize, y: usize, n: usize, list: &[[i16; 2]; MAX_MERGE_CAND]) -> (usize, u64) {
        let Some(me) = self.me else { return (0, u64::MAX / 4) };
        let mut buf = vec![0u16; n * n];
        let mut best = (0, u64::MAX);
        for (i, &mv) in list.iter().enumerate() {
            if list[..i].contains(&mv) {
                continue;
            }
            me.approx_pred(x as i32, y as i32, n, mv, &mut buf);
            let satd = satd(&self.src[0][y * self.w + x..], self.w, &buf, n, n) as u64;
            let cost = satd + (self.lambda_sad * (i + 1) as f64) as u64;
            if cost < best.1 {
                best = (i, cost);
            }
        }
        best
    }

    /// Integer then fractional motion search. Returns (mv, mvp index, SATD cost).
    fn motion_search(&self, x: usize, y: usize, n: usize, amvp: &[[i16; 2]; 2], merge: &[[i16; 2]; MAX_MERGE_CAND]) -> ([i16; 2], u8, u64) {
        let Some(me) = self.me else { return (amvp[0], 0, u64::MAX / 4) };
        let (xi, yi) = (x as i32, y as i32);
        let src = &self.src[0][y * self.w + x..];
        let ls = self.lambda_sad;
        let mv_bits = |mv: [i32; 2]| -> f64 { amvp.iter().map(|p| mvd_bits(mv[0] - p[0] as i32) + mvd_bits(mv[1] - p[1] as i32)).fold(f64::MAX, f64::min) };
        let (lo_x, hi_x, lo_y, hi_y) = me.int_range(xi, yi, n);
        let clamp_i = |v: [i32; 2]| [v[0].clamp(lo_x, hi_x), v[1].clamp(lo_y, hi_y)];
        let sad_cost = |v: [i32; 2]| -> f64 { me.sad(src, self.w, xi + v[0], yi + v[1], n) as f64 + ls * mv_bits([v[0] * 4, v[1] * 4]) };
        // starting points
        let mut best = [0i32; 2];
        let mut best_cost = sad_cost(best);
        for mv in amvp.iter().chain(merge.iter()) {
            let v = clamp_i([(mv[0] as i32 + 2) >> 2, (mv[1] as i32 + 2) >> 2]);
            let c = sad_cost(v);
            if c < best_cost {
                best_cost = c;
                best = v;
            }
        }
        // square pattern search with decreasing steps
        let mut step = 16;
        while step >= 1 {
            let mut moved = true;
            let mut iters = 0;
            while moved && iters < 4 {
                moved = false;
                iters += 1;
                let center = best;
                for (dx, dy) in [(-1, -1), (0, -1), (1, -1), (-1, 0), (1, 0), (-1, 1), (0, 1), (1, 1)] {
                    let v = clamp_i([center[0] + dx * step, center[1] + dy * step]);
                    if v == center {
                        continue;
                    }
                    let c = sad_cost(v);
                    if c < best_cost {
                        best_cost = c;
                        best = v;
                        moved = true;
                    }
                }
            }
            step /= 2;
        }
        // fractional refinement with SATD on the approximate interpolated planes
        let mut buf = vec![0u16; n * n];
        let mut satd_cost = |mv: [i32; 2]| -> f64 {
            me.approx_pred(xi, yi, n, [mv[0] as i16, mv[1] as i16], &mut buf);
            satd(src, self.w, &buf, n, n) as f64 + ls * mv_bits(mv)
        };
        let mut bq = [best[0] * 4, best[1] * 4];
        let mut bc = satd_cost(bq);
        for step in [2, 1] {
            let center = bq;
            for (dx, dy) in [(-1, -1), (0, -1), (1, -1), (-1, 0), (1, 0), (-1, 1), (0, 1), (1, 1)] {
                let v = [center[0] + dx * step, center[1] + dy * step];
                let c = satd_cost(v);
                if c < bc {
                    bc = c;
                    bq = v;
                }
            }
        }
        let mv = [bq[0] as i16, bq[1] as i16];
        let bits = |p: &[i16; 2]| mvd_bits(bq[0] - p[0] as i32) + mvd_bits(bq[1] - p[1] as i32);
        let mvp = if bits(&amvp[1]) < bits(&amvp[0]) { 1 } else { 0 };
        (mv, mvp, bc as u64)
    }

    /// Best 2Nx2N intra luma mode by SATD (coarse grid, then refinement around the best angle).
    fn search_intra_mode(&self, x: usize, y: usize, log2: u32) -> (u32, u64) {
        let n = 1usize << log2;
        let refs = self.build_refs(0, x, y, log2);
        let mpm = self.mpm(x, y);
        let mut buf = vec![0u16; n * n];
        let src = &self.src[0][y * self.w + x..];
        let mut eval = |m: u32| -> u64 {
            predict_from_refs(&refs, 0, m, n, self.bd, &mut buf, n);
            let bits = match mpm.iter().position(|&c| c == m) {
                Some(0) => 2.0,
                Some(_) => 3.0,
                None => 6.0,
            };
            satd(src, self.w, &buf, n, n) as u64 + (self.lambda_sad * bits) as u64
        };
        let mut best = (1u32, u64::MAX);
        let mut tried = [false; 35];
        for &m in COARSE_MODES.iter().chain(mpm.iter()) {
            if !tried[m as usize] {
                tried[m as usize] = true;
                let c = eval(m);
                if c < best.1 {
                    best = (m, c);
                }
            }
        }
        if best.0 >= 2 {
            for d in [2i32, 1] {
                let center = best.0 as i32;
                for m in [center - d, center + d] {
                    if (2..=34).contains(&m) && !tried[m as usize] {
                        tried[m as usize] = true;
                        let c = eval(m as u32);
                        if c < best.1 {
                            best = (m as u32, c);
                        }
                    }
                }
            }
        }
        best
    }

    /// Modes of the four 4x4 PUs of an NxN intra CU, chosen sequentially (each PU reconstructed
    /// before the next one is searched, as in the real coding order).
    fn search_nxn_modes(&mut self, x: usize, y: usize) -> [u8; 4] {
        let mut modes = [1u8; 4];
        self.fill(x, y, 8, |b| {
            b.flags = F_INTRA;
            b.ipm = 1;
        });
        for k in 0..4 {
            let (px, py) = (x + (k & 1) * 4, y + (k >> 1) * 4);
            let (m, _) = self.search_intra_mode(px, py, 2);
            modes[k] = m as u8;
            self.fill(px, py, 4, |b| b.ipm = m as u8);
            self.tbs.clear();
            self.coefs.clear();
            self.code_tb(0, px, py, 2, Some(m), (px, py));
        }
        modes
    }
}

/// Filter (luma) and predict an intra block from substituted references.
fn predict_from_refs(refs: &Refs, c: usize, mode: u32, n: usize, bd: u32, out: &mut [u16], os: usize) {
    if c == 0 {
        let mut r = refs.clone();
        intra::filter(&mut r, mode, n, true, true, bd);
        intra::predict(&r, mode, n, true, bd, out, os);
    } else {
        intra::predict(refs, mode, n, false, bd, out, os);
    }
}

/// scanIdx (7.4.9.11) for an intra prediction mode.
fn scan_idx_for_mode(m: u32) -> u8 {
    if (6..=14).contains(&m) {
        2
    } else if (22..=30).contains(&m) {
        1
    } else {
        0
    }
}

/// Approximate bits of one motion vector difference component.
fn mvd_bits(v: i32) -> f64 {
    if v == 0 { 1.0 } else { (2 * (32 - (v.unsigned_abs()).leading_zeros()) + 1) as f64 }
}

fn write_merge_idx<S: Sink>(s: &mut S, idx: usize) {
    if MAX_MERGE_CAND > 1 {
        s.decision(MERGE_IDX, (idx > 0) as u32);
        if idx > 0 {
            let mut i = 1;
            while i < MAX_MERGE_CAND - 1 {
                let b = idx > i;
                s.bypass(b as u32);
                if !b {
                    break;
                }
                i += 1;
            }
        }
    }
}

/// mvd_coding() (7.3.8.9).
fn write_mvd<S: Sink>(s: &mut S, d: [i32; 2]) {
    let g0 = [d[0] != 0, d[1] != 0];
    s.decision(ABS_MVD_GT0, g0[0] as u32);
    s.decision(ABS_MVD_GT0, g0[1] as u32);
    for k in 0..2 {
        if g0[k] {
            s.decision(ABS_MVD_GT1, (d[k].abs() > 1) as u32);
        }
    }
    for k in 0..2 {
        if g0[k] {
            let a = d[k].unsigned_abs();
            if a > 1 {
                // abs_mvd_minus2: EG1
                let mut v = a - 2;
                let mut kk = 1;
                while v >= (1 << kk) {
                    s.bypass(1);
                    v -= 1 << kk;
                    kk += 1;
                }
                s.bypass(0);
                s.bypass_bits(v, kk);
            }
            s.bypass((d[k] < 0) as u32);
        }
    }
}

/// coeff_abs_level_remaining binarisation (9.3.3.11): TR prefix up to 3 with Rice suffix, then
/// an escape with an Exp-Golomb-like suffix.
fn write_remaining<S: Sink>(s: &mut S, v: u32, rice: u32) {
    if v < (4 << rice) {
        let p = v >> rice;
        for _ in 0..p {
            s.bypass(1);
        }
        s.bypass(0);
        s.bypass_bits(v & ((1 << rice) - 1), rice);
    } else {
        let u = v - (2 << rice);
        let kr = 31 - u.leading_zeros(); // k + rice
        let k = kr - rice;
        for _ in 0..k + 3 {
            s.bypass(1);
        }
        s.bypass(0);
        s.bypass_bits(u - (1 << kr), kr);
    }
}

/// last_sig_coeff_{x,y}_prefix value and suffix for a position.
fn last_prefix(v: u32) -> (u32, u32, u32) {
    if v < 4 {
        return (v, 0, 0);
    }
    let mut p = 4;
    loop {
        let nb = (p >> 1) - 1;
        let min = (1 << nb) * (2 + (p & 1));
        let next_nb = ((p + 1) >> 1) - 1;
        let next = (1 << next_nb) * (2 + ((p + 1) & 1));
        if v < next {
            return (p, v - min, nb);
        }
        p += 1;
    }
}

/// Context increment for significant_coeff_flag (H.265 9.3.4.2).
fn significance_context(group: (usize, usize), pos: (usize, usize), log2: u32, prev_csbf: u8, c: usize, scan_idx: usize) -> usize {
    let (xs, ys) = group;
    let (xp, yp) = pos;
    let (xc, yc) = (xs * 4 + xp, ys * 4 + yp);
    if log2 == 2 {
        CTX_IDX_MAP[(yc << 2) + xc] as usize
    } else if xc + yc == 0 {
        0
    } else {
        let mut sc = match prev_csbf {
            0 => {
                if xp + yp == 0 {
                    2
                } else if xp + yp < 3 {
                    1
                } else {
                    0
                }
            }
            1 => {
                if yp == 0 {
                    2
                } else if yp == 1 {
                    1
                } else {
                    0
                }
            }
            2 => {
                if xp == 0 {
                    2
                } else if xp == 1 {
                    1
                } else {
                    0
                }
            }
            _ => 2,
        };
        if c == 0 {
            if xs + ys > 0 {
                sc += 3;
            }
            sc += if log2 == 3 { if scan_idx == 0 { 9 } else { 15 } } else { 21 };
        } else {
            sc += if log2 == 3 { 9 } else { 12 };
        }
        sc
    }
}

/// Absolute levels, signs and Rice remainders for one significant coefficient group.
fn write_coefficient_levels<S: Sink>(s: &mut S, sig: &[u8], at: impl Fn(usize) -> i32, i: usize, c: usize, prev_g1: &mut Option<u32>) {
    let nsig = sig.len();
    let abs_at = |k: usize| at(sig[k] as usize).unsigned_abs();
    let mut ctx_set = if i == 0 || c > 0 { 0 } else { 2 };
    if *prev_g1 == Some(0) {
        ctx_set += 1;
    }
    let mut g1ctx = 1u32;
    let mut gt1 = [0u8; 16];
    let mut first_g1: Option<usize> = None;
    let c_off_g1 = if c > 0 { 16 } else { 0 };
    for k in 0..nsig.min(8) {
        let f = abs_at(k) > 1;
        s.decision(GT1 + ctx_set * 4 + g1ctx.min(3) as usize + c_off_g1, f as u32);
        gt1[k] = f as u8;
        if f {
            g1ctx = 0;
            if first_g1.is_none() {
                first_g1 = Some(k);
            }
        } else if g1ctx > 0 {
            g1ctx += 1;
        }
    }
    *prev_g1 = Some(g1ctx);
    let mut gt2 = 0u32;
    if let Some(k) = first_g1 {
        gt2 = (abs_at(k) > 2) as u32;
        s.decision(GT2 + ctx_set + if c > 0 { 4 } else { 0 }, gt2);
    }
    // sign data hiding: the sign of the first coefficient in scan order (sig[nsig - 1]) is
    // inferred from the parity of the sum of absolute levels
    let hidden = sig[0] as i32 - sig[nsig - 1] as i32 > 3;
    let nsigns = if hidden { nsig - 1 } else { nsig };
    for k in 0..nsigns {
        s.bypass((at(sig[k] as usize) < 0) as u32);
    }
    if hidden {
        let sum: u32 = (0..nsig).map(abs_at).sum();
        debug_assert_eq!(at(sig[nsig - 1] as usize) < 0, sum & 1 == 1, "sign hiding parity");
    }
    let mut rice = 0u32;
    for k in 0..nsig {
        let mut base = 1 + gt1[k] as u32;
        if Some(k) == first_g1 {
            base += gt2;
        }
        let thresh = if k < 8 { if Some(k) == first_g1 { 3 } else { 2 } } else { 1 };
        if base == thresh {
            let level = abs_at(k);
            write_remaining(s, level - base, rice);
            if level > 3 * (1 << rice) {
                rice = (rice + 1).min(4);
            }
        }
    }
}

/// residual_coding() (7.3.8.11) of one transform block of levels (row-major, n x n). No transform
/// skip; signs use sign data hiding where the coefficient span permits it.
fn residual_coding<S: Sink>(s: &mut S, lv: &[i32], log2: u32, c: usize, scan_idx: usize) {
    let n = 1usize << log2;
    let log2sb = log2 - 2;
    let sb_w = 1usize << log2sb;
    let sb_scan = scan_order(log2sb, scan_idx);
    let pos_scan = &SCAN_4[scan_idx];
    let at = |xs: usize, ys: usize, p: usize| lv[(ys * 4 + pos_scan[p].1 as usize) * n + xs * 4 + pos_scan[p].0 as usize];
    // last significant coefficient in scan order
    let mut last = None;
    'outer: for i in (0..sb_scan.len()).rev() {
        let (xs, ys) = (sb_scan[i].0 as usize, sb_scan[i].1 as usize);
        for p in (0..16).rev() {
            if at(xs, ys, p) != 0 {
                last = Some((i, p));
                break 'outer;
            }
        }
    }
    // Callers only code blocks with a nonzero level (coded_block_flag = 1).
    let Some((last_sb, last_pos)) = last else { return };
    let lx = sb_scan[last_sb].0 as u32 * 4 + pos_scan[last_pos].0 as u32;
    let ly = sb_scan[last_sb].1 as u32 * 4 + pos_scan[last_pos].1 as u32;
    let (cx, cy) = if scan_idx == 2 { (ly, lx) } else { (lx, ly) };
    let cmax = (log2 << 1) - 1;
    let (off, shift) = if c == 0 { (3 * (log2 - 2) + ((log2 - 1) >> 2), (log2 + 1) >> 2) } else { (15, log2 - 2) };
    let (px, sx, nbx) = last_prefix(cx);
    let (py, sy, nby) = last_prefix(cy);
    for (p, base) in [(px, LAST_X_PREFIX), (py, LAST_Y_PREFIX)] {
        for i in 0..p {
            s.decision(base + (off + (i >> shift)) as usize, 1);
        }
        if p < cmax {
            s.decision(base + (off + (p >> shift)) as usize, 0);
        }
    }
    if px > 3 {
        s.bypass_bits(sx, nbx);
    }
    if py > 3 {
        s.bypass_bits(sy, nby);
    }
    let mut csbf = [0u8; 64];
    let mut prev_g1: Option<u32> = None;
    let c_off_sig = if c == 0 { 0 } else { 27 };
    for i in (0..=last_sb).rev() {
        let (xs, ys) = (sb_scan[i].0 as usize, sb_scan[i].1 as usize);
        let right = if xs + 1 < sb_w { csbf[ys * 8 + xs + 1] } else { 0 };
        let below = if ys + 1 < sb_w { csbf[(ys + 1) * 8 + xs] } else { 0 };
        let mut infer_dc = false;
        if i < last_sb && i > 0 {
            let has = (0..16).any(|p| at(xs, ys, p) != 0);
            let inc = (right | below).min(1) as usize + if c > 0 { 2 } else { 0 };
            s.decision(CODED_SUB_BLOCK + inc, has as u32);
            csbf[ys * 8 + xs] = has as u8;
            infer_dc = true;
        } else {
            csbf[ys * 8 + xs] = 1;
        }
        if csbf[ys * 8 + xs] == 0 {
            continue;
        }
        let mut sig = [0u8; 16];
        let mut nsig = 0;
        let start: i32 = if i == last_sb {
            sig[0] = last_pos as u8;
            nsig = 1;
            last_pos as i32 - 1
        } else {
            15
        };
        let prev_csbf = right + (below << 1);
        let mut np = start;
        while np >= 0 {
            let p = np as usize;
            let (xp, yp) = (pos_scan[p].0 as usize, pos_scan[p].1 as usize);
            let (xc, yc) = (xs * 4 + xp, ys * 4 + yp);
            let v = lv[yc * n + xc] != 0;
            if np > 0 || !infer_dc {
                let sig_ctx = significance_context((xs, ys), (xp, yp), log2, prev_csbf, c, scan_idx);
                s.decision(SIG_COEFF + c_off_sig + sig_ctx, v as u32);
                if v {
                    sig[nsig] = p as u8;
                    nsig += 1;
                    infer_dc = false;
                }
            } else {
                debug_assert!(v, "inferred DC coefficient must be non-zero");
                sig[nsig] = 0;
                nsig += 1;
            }
            np -= 1;
        }
        if nsig == 0 {
            continue;
        }
        write_coefficient_levels(s, &sig[..nsig], |p| at(xs, ys, p), i, c, &mut prev_g1);
    }
}

/// Sum of absolute Hadamard-transformed differences (8x8 blocks, 4x4 for 4-sample blocks).
pub fn satd(src: &[u16], ss: usize, pred: &[u16], ps: usize, n: usize) -> u32 {
    if n == 4 {
        return satd4(src, ss, pred, ps);
    }
    let mut total = 0;
    for by in (0..n).step_by(8) {
        for bx in (0..n).step_by(8) {
            total += satd8(&src[by * ss + bx..], ss, &pred[by * ps + bx..], ps);
        }
    }
    total
}

fn satd4(src: &[u16], ss: usize, pred: &[u16], ps: usize) -> u32 {
    let mut d = [0i32; 16];
    for y in 0..4 {
        for x in 0..4 {
            d[y * 4 + x] = src[y * ss + x] as i32 - pred[y * ps + x] as i32;
        }
    }
    let mut m = [0i32; 16];
    for y in 0..4 {
        let r = &d[y * 4..y * 4 + 4];
        let (a, b, c, e) = (r[0] + r[3], r[1] + r[2], r[1] - r[2], r[0] - r[3]);
        m[y * 4] = a + b;
        m[y * 4 + 1] = e + c;
        m[y * 4 + 2] = a - b;
        m[y * 4 + 3] = e - c;
    }
    let mut sum = 0;
    for x in 0..4 {
        let (r0, r1, r2, r3) = (m[x], m[4 + x], m[8 + x], m[12 + x]);
        let (a, b, c, e) = (r0 + r3, r1 + r2, r1 - r2, r0 - r3);
        sum += (a + b).abs() + (e + c).abs() + (a - b).abs() + (e - c).abs();
    }
    ((sum + 1) >> 1) as u32
}

fn hadamard8(v: &mut [i32; 8]) {
    let mut t = *v;
    let mut h = 1;
    while h < 8 {
        for i in (0..8).step_by(h * 2) {
            for j in i..i + h {
                let (a, b) = (t[j], t[j + h]);
                t[j] = a + b;
                t[j + h] = a - b;
            }
        }
        h *= 2;
    }
    *v = t;
}

fn satd8(src: &[u16], ss: usize, pred: &[u16], ps: usize) -> u32 {
    let mut m = [[0i32; 8]; 8];
    for y in 0..8 {
        for x in 0..8 {
            m[y][x] = src[y * ss + x] as i32 - pred[y * ps + x] as i32;
        }
        hadamard8(&mut m[y]);
    }
    let mut sum = 0;
    for x in 0..8 {
        let mut col = [0i32; 8];
        for y in 0..8 {
            col[y] = m[y][x];
        }
        hadamard8(&mut col);
        sum += col.iter().map(|v| v.abs()).sum::<i32>();
    }
    ((sum + 2) >> 2) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn last_prefix_groups() {
        assert_eq!(last_prefix(3), (3, 0, 0));
        assert_eq!(last_prefix(4), (4, 0, 1));
        assert_eq!(last_prefix(5), (4, 1, 1));
        assert_eq!(last_prefix(7), (5, 1, 1));
        assert_eq!(last_prefix(8), (6, 0, 2));
        assert_eq!(last_prefix(15), (7, 3, 2));
        assert_eq!(last_prefix(16), (8, 0, 3));
        assert_eq!(last_prefix(31), (9, 7, 3));
    }
}

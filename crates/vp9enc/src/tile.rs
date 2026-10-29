//! Tile coding: partitions, intra blocks (key frames) and inter blocks (motion search, mode
//! decision, motion vector prediction, residual), recorded as symbols (see [`crate::probs`]).
//!
//! Mode info, motion vector candidate lists and contexts follow the decoding process of the
//! VP9 specification (6.4, 6.4.x `find_mv_refs` / `find_best_ref_mvs`) for the subset of the
//! syntax this encoder emits: 8×8 and 16×16 blocks, single LAST references, no
//! `UsePrevFrameMvs` (inter frames are error resilient), no high-precision vectors.

use crate::mc::{Mv, PhasePlanes, RefPlane, clamp_for_plane, predict};
use crate::probs::{self, P, Writer, cost, tree_cost};
use crate::tables::{
    COEFBAND_4X4, COL_SCAN_4X4, COUNTER_TO_CONTEXT, DEFAULT_SCAN_4X4, ENERGY_CLASS, KF_PARTITION_PROBS, KF_UV_MODE_PROBS, KF_Y_MODE_PROBS, MODE_2_COUNTER,
    MV_REF_8X8, MV_REF_16X16, ROW_SCAN_4X4,
};
use crate::transform::{ADST_ADST, ADST_DCT, DCT_ADST, DCT_DCT, Quant};
use crate::{BLOCK_8X8, BLOCK_16X16, Mi, Plane};

pub const DC_PRED: u8 = 0;
pub const V_PRED: u8 = 1;
pub const H_PRED: u8 = 2;
pub const TM_PRED: u8 = 9;
const MODES: [u8; 4] = [DC_PRED, V_PRED, H_PRED, TM_PRED];
pub const NEARESTMV: u8 = 10;
pub const NEARMV: u8 = 11;
pub const ZEROMV: u8 = 12;
pub const NEWMV: u8 = 13;

/// Which reconstructed neighbours are available for intra prediction.
#[derive(Clone, Copy)]
struct IntraEdges {
    left: bool,
    above: bool,
}

const PARTITION_TREE: [i8; 6] = [0, 2, -1, 4, -2, -3];
const INTRA_MODE_TREE: [i8; 18] = [0, 2, -9, 4, -1, 6, 8, 12, -2, 10, -4, -5, -3, 14, -8, 16, -6, -7];
/// Leaves are `mode - NEARESTMV`.
const INTER_MODE_TREE: [i8; 6] = [-2, 2, 0, 4, -1, -3];
const MV_JOINT_TREE: [i8; 6] = [0, 2, -1, 4, -2, -3];
const MV_CLASS_TREE: [i8; 20] = [0, 2, -1, 4, 6, 8, -2, -3, 10, 12, -4, -5, -6, 14, 16, 18, -7, -8, -9, -10];
const MV_FR_TREE: [i8; 6] = [0, 2, -1, 4, -2, -3];

/// extra_bits[token] = (cat, numExtra, base).
const EXTRA_BITS: [(u8, u8, u16); 11] =
    [(0, 0, 0), (0, 0, 1), (0, 0, 2), (0, 0, 3), (0, 0, 4), (1, 1, 5), (2, 2, 7), (3, 3, 11), (4, 4, 19), (5, 5, 35), (6, 14, 67)];
const CAT_PROBS: [&[u8]; 7] = [
    &[],
    &[159],
    &[165, 145],
    &[173, 148, 140],
    &[176, 155, 140, 135],
    &[180, 157, 141, 134, 130],
    &[254, 254, 254, 252, 249, 243, 230, 196, 177, 153, 140, 133, 130, 129],
];

/// mode2txfm_map for the intra modes used here (inter blocks use DCT_DCT).
fn mode_tx_type(mode: u8) -> u8 {
    match mode {
        V_PRED => ADST_DCT,
        H_PRED => DCT_ADST,
        TM_PRED => ADST_ADST,
        _ => DCT_DCT,
    }
}

fn scan(tx_type: u8) -> &'static [u16; 16] {
    match tx_type {
        ADST_DCT => &ROW_SCAN_4X4,
        DCT_ADST => &COL_SCAN_4X4,
        _ => &DEFAULT_SCAN_4X4,
    }
}

/// Coefficient context neighbours (9.3.2) of each scan position.
fn neighbors(tx_type: u8) -> [(usize, usize); 16] {
    let sc = scan(tx_type);
    let mut nb = [(0, 0); 16];
    for (c, &pos) in sc.iter().enumerate().skip(1) {
        let pos = pos as usize;
        let (i, j) = (pos / 4, pos % 4);
        nb[c] = if i > 0 && j > 0 {
            let a = (i - 1) * 4 + j;
            let b = i * 4 + j - 1;
            match tx_type {
                DCT_ADST => (a, a),
                ADST_DCT => (b, b),
                _ => (a, b),
            }
        } else if i > 0 {
            ((i - 1) * 4 + j, (i - 1) * 4 + j)
        } else {
            (j - 1, j - 1)
        };
    }
    nb
}

fn eob_of(levels: &[i32; 16], tx_type: u8) -> usize {
    let sc = scan(tx_type);
    (0..16).rev().find(|&i| levels[sc[i] as usize] != 0).map_or(0, |i| i + 1)
}

/// Rough coefficient bit cost (1/256 bit) for rate-distortion decisions.
fn levels_cost(levels: &[i32; 16], eob: usize) -> u32 {
    if eob == 0 {
        return 0;
    }
    let mut bits = 256;
    for &l in levels {
        let a = l.unsigned_abs();
        bits += if a == 0 { 160 } else { 640 + 384 * (32 - a.leading_zeros()) };
    }
    bits
}

/// Intra prediction (8.5.1) of a square block for DC / V / H / TM.
fn predict_intra(p: &Plane, x: usize, y: usize, log2: usize, mode: u8, edges: IntraEdges) -> [i32; 64] {
    let IntraEdges { left: have_left, above: have_above } = edges;
    let size = 1usize << log2;
    let base = 128i32;
    let buf = &p.rec;
    let w = p.w;
    let mut above = [0i32; 8];
    let mut left = [0i32; 8];
    if have_above {
        for i in 0..size {
            above[i] = buf[(y - 1) * w + (x + i).min(p.max_x)] as i32;
        }
    } else {
        above[..size].fill(base - 1);
    }
    let top_left = if have_above && have_left {
        buf[(y - 1) * w + (x - 1).min(p.max_x)] as i32
    } else if have_above {
        base + 1
    } else {
        base - 1
    };
    if have_left {
        for i in 0..size {
            left[i] = buf[(y + i).min(p.max_y) * w + x - 1] as i32;
        }
    } else {
        left[..size].fill(base + 1);
    }
    let mut out = [0i32; 64];
    for i in 0..size {
        for j in 0..size {
            out[i * size + j] = match mode {
                V_PRED => above[j],
                H_PRED => left[i],
                TM_PRED => (above[j] + left[i] - top_left).clamp(0, 255),
                _ => 0,
            };
        }
    }
    if mode == DC_PRED {
        let v = match (have_left, have_above) {
            (true, true) => (left[..size].iter().sum::<i32>() + above[..size].iter().sum::<i32>() + size as i32) >> (log2 + 1),
            (true, false) => (left[..size].iter().sum::<i32>() + (1 << (log2 - 1))) >> log2,
            (false, true) => (above[..size].iter().sum::<i32>() + (1 << (log2 - 1))) >> log2,
            (false, false) => base,
        };
        out[..size * size].fill(v);
    }
    out
}

// ---------------------------------------------------------------- motion vector coding

/// `mv_class` of an offset (|component| - 1).
fn mv_class(offset: u32) -> u32 {
    if offset < 16 { 0 } else { (31 - offset.leading_zeros()) - 3 }
}

fn mv_component_cost(i: usize, v: i32) -> u32 {
    let d = &probs_defaults().0;
    let offset = v.unsigned_abs() - 1;
    let c = mv_class(offset);
    let mut bits = cost(v < 0, d[probs::MV_SIGN + i]);
    bits += tree_cost(&MV_CLASS_TREE, &d[probs::MV_CLASS + i * 10..], c as u8);
    let fr = ((offset >> 1) & 3) as u8;
    if c == 0 {
        let b = offset >> 3;
        bits += cost(b == 1, d[probs::MV_CLASS0_BIT + i]);
        bits += tree_cost(&MV_FR_TREE, &d[probs::MV_CLASS0_FR + i * 6 + b as usize * 3..], fr);
    } else {
        let dd = (offset - (1 << (c + 3))) >> 3;
        for b in 0..c {
            bits += cost((dd >> b) & 1 == 1, d[probs::MV_BITS + i * 10 + b as usize]);
        }
        bits += tree_cost(&MV_FR_TREE, &d[probs::MV_FR + i * 3..], fr);
    }
    bits
}

const MV_TABLE: i32 = 8192;

/// Component costs for |v| ≤ MV_TABLE (index v + MV_TABLE) and the four joint costs.
fn mv_tables() -> &'static ([Vec<u32>; 2], [u32; 4]) {
    use std::sync::OnceLock;
    static T: OnceLock<([Vec<u32>; 2], [u32; 4])> = OnceLock::new();
    T.get_or_init(|| {
        let comp = std::array::from_fn(|i| (-MV_TABLE..=MV_TABLE).map(|v| if v == 0 { 0 } else { mv_component_cost(i, v) }).collect());
        let d = &probs_defaults().0;
        let joints = std::array::from_fn(|j| tree_cost(&MV_JOINT_TREE, &d[probs::MV_JOINT..], j as u8));
        (comp, joints)
    })
}

/// Bits (1/256) of a NEWMV difference with the default probabilities.
pub fn mv_cost(diff: Mv) -> u32 {
    let (comp, joints) = mv_tables();
    let joint = (diff.row != 0) as usize * 2 + (diff.col != 0) as usize;
    let c = |i: usize, v: i32| if v.abs() <= MV_TABLE { comp[i][(v + MV_TABLE) as usize] } else { mv_component_cost(i, v) };
    joints[joint] + if diff.row != 0 { c(0, diff.row) } else { 0 } + if diff.col != 0 { c(1, diff.col) } else { 0 }
}

fn probs_defaults() -> &'static probs::Probs {
    use std::sync::OnceLock;
    static D: OnceLock<probs::Probs> = OnceLock::new();
    D.get_or_init(probs::Probs::defaults)
}

/// SAD of two N×N blocks: per-column 16-bit sums (at most 16 · 255), summed once.
#[inline(always)]
fn sad_n<const N: usize>(a: &[u8], sa: usize, b: &[u8], sb: usize) -> u32 {
    let mut acc = [0u16; N];
    for i in 0..N {
        let (Some(ra), Some(rb)) = (a.get(i * sa..).and_then(|r| r.first_chunk::<N>()), b.get(i * sb..).and_then(|r| r.first_chunk::<N>())) else {
            return u32::MAX / 4;
        };
        for j in 0..N {
            acc[j] += ra[j].abs_diff(rb[j]) as u16;
        }
    }
    acc.iter().map(|&v| v as u32).sum()
}

// ---------------------------------------------------------------- the tile

/// Per-frame coding parameters shared by the tiles.
pub struct FrameParams<'a> {
    pub key: bool,
    pub quant: Quant,
    pub mi_cols: usize,
    pub mi_rows: usize,
    /// Reference planes (inter frames).
    pub refs: Option<[RefPlane<'a>; 3]>,
    /// The reference luma at every quarter-sample phase (motion search).
    pub phases: Option<PhasePlanes>,
    /// Encoder-side motion hints from the previous frame (by MI position).
    pub prev_mvs: Option<&'a [Mv]>,
    /// Lagrangian multipliers: per bit, in SAD and in squared-error units.
    pub lambda_sad: f64,
    pub lambda_sse: f64,
}

pub struct Tile<'a> {
    pub f: &'a FrameParams<'a>,
    pub w: Writer,
    pub planes: &'a mut [Plane; 3],
    pub mi: &'a mut [Mi],
    pub col_start: usize,
    pub col_end: usize,
    above_nz: [Vec<u8>; 3],
    left_nz: [Vec<u8>; 3],
    above_part: Vec<u8>,
    left_part: Vec<u8>,
    /// Motion vectors of the four 8×8 blocks evaluated for the current 16×16 area.
    pending: [(usize, usize, Mv); 4],
}

impl<'a> Tile<'a> {
    pub fn new(f: &'a FrameParams<'a>, planes: &'a mut [Plane; 3], mi: &'a mut [Mi], col_start: usize, col_end: usize) -> Tile<'a> {
        let n = f.mi_cols * 2 + 16;
        Tile {
            f,
            w: Writer::default(),
            planes,
            mi,
            col_start,
            col_end,
            above_nz: [vec![0; n], vec![0; n], vec![0; n]],
            left_nz: [vec![0; 16], vec![0; 16], vec![0; 16]],
            above_part: vec![0; f.mi_cols + 8],
            left_part: vec![0; 8],
            pending: [(usize::MAX, 0, Mv::ZERO); 4],
        }
    }

    /// Code the tile's superblocks.
    pub fn code(&mut self) {
        for r in (0..self.f.mi_rows).step_by(8) {
            for p in 0..3 {
                self.left_nz[p].fill(0);
            }
            self.left_part.fill(0);
            for c in (self.col_start..self.col_end).step_by(8) {
                self.partition(r, c, 12);
            }
        }
    }

    fn mi_at(&self, r: usize, c: usize) -> Mi {
        self.mi[r * self.f.mi_cols + c]
    }

    fn set_mi(&mut self, r: usize, c: usize, n8: usize, m: Mi) {
        for dr in 0..n8 {
            for dc in 0..n8 {
                if r + dr < self.f.mi_rows && c + dc < self.f.mi_cols {
                    self.mi[(r + dr) * self.f.mi_cols + c + dc] = m;
                }
            }
        }
    }

    fn partition(&mut self, r: usize, c: usize, bsize: u8) {
        let (mi_rows, mi_cols) = (self.f.mi_rows, self.f.mi_cols);
        if r >= mi_rows || c >= mi_cols {
            return;
        }
        let num8 = [1usize, 1, 1, 1, 1, 2, 2, 2, 4, 4, 4, 8, 8][bsize as usize];
        let half = num8 >> 1;
        let has_rows = (r + half) < mi_rows;
        let has_cols = (c + half) < mi_cols;
        let bsl = [0usize, 0, 0, 0, 0, 1, 1, 1, 2, 2, 2, 3, 3][bsize as usize];
        let boffset = 3 - bsl;
        let mut above = 0u8;
        let mut left = 0u8;
        for i in 0..num8 {
            above |= self.above_part[c + i];
            left |= self.left_part[(r & 7) + i];
        }
        let ctx = bsl * 4 + (((left >> boffset) & 1) as usize) * 2 + ((above >> boffset) & 1) as usize;
        let key = self.f.key;
        let prob = |i: usize| if key { P::Fixed(KF_PARTITION_PROBS[ctx * 3 + i]) } else { P::Ctx((probs::PARTITION + ctx * 3 + i) as u16) };
        if bsize == BLOCK_8X8 {
            self.w.put(false, prob(0)); // PARTITION_NONE
            if key {
                self.intra_block(r, c);
            } else {
                let mv = self.pending.iter().find(|p| p.0 == r && p.1 == c).map(|p| p.2);
                let mv = mv.unwrap_or_else(|| self.search(r, c, BLOCK_8X8).0);
                self.inter_block(r, c, BLOCK_8X8, mv);
            }
            self.above_part[c] = 15 >> 1;
            self.left_part[r & 7] = 15 >> 1;
            return;
        }
        if !key && bsize == BLOCK_16X16 && has_rows && has_cols {
            if let Some(mv) = self.decide16(r, c) {
                self.w.put(false, prob(0)); // PARTITION_NONE
                self.inter_block(r, c, BLOCK_16X16, mv);
                for i in 0..2 {
                    self.above_part[c + i] = 15 >> 2;
                    self.left_part[(r & 7) + i] = 15 >> 2;
                }
                return;
            }
        } else if !key && bsize == BLOCK_16X16 {
            self.pending = [(usize::MAX, 0, Mv::ZERO); 4];
        }
        // PARTITION_SPLIT.
        if has_rows && has_cols {
            self.w.tree(&PARTITION_TREE, 3, prob);
        } else if has_cols {
            self.w.put(true, prob(1));
        } else if has_rows {
            self.w.put(true, prob(2));
        }
        let sub = bsize - 3;
        self.partition(r, c, sub);
        self.partition(r, c + half, sub);
        self.partition(r + half, c, sub);
        self.partition(r + half, c + half, sub);
    }

    // ------------------------------------------------------------ intra (key frames)

    fn intra_block(&mut self, r: usize, c: usize) {
        let avail_u = r > 0;
        let avail_l = c > self.col_start;
        let y_mode = self.choose_intra(0, c * 8, r * 8, 3, IntraEdges { left: avail_l, above: avail_u });
        let uv_mode = {
            let mut best = (i64::MAX, DC_PRED);
            for m in MODES {
                let cost: i64 = (1..3).map(|p| self.intra_cost(p, c * 4, r * 4, 2, m, IntraEdges { left: avail_l, above: avail_u })).sum();
                if cost < best.0 {
                    best = (cost, m);
                }
            }
            best.1
        };
        // skip = 0 (neighbours never skip in key frames: context 0); tx_size implied (4×4).
        self.w.ctx(false, probs::SKIP);
        let above_mode = if avail_u { self.mi_at(r - 1, c).mode } else { DC_PRED };
        let left_mode = if avail_l { self.mi_at(r, c - 1).mode } else { DC_PRED };
        let o = (above_mode as usize * 10 + left_mode as usize) * 9;
        self.w.tree(&INTRA_MODE_TREE, y_mode, |i| P::Fixed(KF_Y_MODE_PROBS[o + i]));
        let o = y_mode as usize * 9;
        self.w.tree(&INTRA_MODE_TREE, uv_mode, |i| P::Fixed(KF_UV_MODE_PROBS[o + i]));
        self.set_mi(r, c, 1, Mi { inter: false, mode: y_mode, mv: Mv::ZERO, skip: false, size: BLOCK_8X8 });
        // Residual: luma 2×2 transform blocks, then one 4×4 block per chroma plane.
        for (dy, dx) in [(0, 0), (0, 1), (1, 0), (1, 1)] {
            let tx = if self.f.quant.lossless { DCT_DCT } else { mode_tx_type(y_mode) };
            self.intra_tx(0, c * 8 + dx * 4, r * 8 + dy * 4, y_mode, tx, IntraEdges { left: avail_l || dx > 0, above: avail_u || dy > 0 });
        }
        for p in 1..3 {
            self.intra_tx(p, c * 4, r * 4, uv_mode, DCT_DCT, IntraEdges { left: avail_l, above: avail_u });
        }
    }

    fn intra_cost(&self, plane: usize, x: usize, y: usize, log2: usize, mode: u8, edges: IntraEdges) -> i64 {
        let p = &self.planes[plane];
        let pred = predict_intra(p, x, y, log2, mode, edges);
        let size = 1 << log2;
        let mut s = 0i64;
        for i in 0..size {
            for j in 0..size {
                s += (p.src[(y + i) * p.w + x + j] as i32 - pred[i * size + j]).abs() as i64;
            }
        }
        s
    }

    fn choose_intra(&self, plane: usize, x: usize, y: usize, log2: usize, edges: IntraEdges) -> u8 {
        let mut best = (i64::MAX, DC_PRED);
        for m in MODES {
            let cost = self.intra_cost(plane, x, y, log2, m, edges);
            if cost < best.0 {
                best = (cost, m);
            }
        }
        best.1
    }

    fn intra_tx(&mut self, plane: usize, x: usize, y: usize, mode: u8, tx_type: u8, edges: IntraEdges) {
        let ss = (plane > 0) as usize;
        let max_x = (self.f.mi_cols * 8) >> ss;
        let max_y = (self.f.mi_rows * 8) >> ss;
        let (x4, y4) = (x >> 2, y >> 2);
        let ly = y4 & (15 >> ss);
        if x >= max_x || y >= max_y {
            self.above_nz[plane][x4] = 0;
            self.left_nz[plane][ly] = 0;
            return;
        }
        let p = &self.planes[plane];
        let pred = predict_intra(p, x, y, 2, mode, edges);
        let mut res = [0i32; 16];
        for i in 0..4 {
            for j in 0..4 {
                res[i * 4 + j] = p.src[(y + i) * p.w + x + j] as i32 - pred[i * 4 + j];
            }
        }
        let q = Quant { round_dc: 0.5, round_ac: 0.38, ..self.f.quant };
        let levels = q.levels(&res, tx_type);
        let eob = eob_of(&levels, tx_type);
        let ctx0 = (self.above_nz[plane][x4] + self.left_nz[plane][ly]) as usize;
        self.tokens(plane, false, tx_type, &levels, eob, ctx0);
        let resid = q.residual(&levels, tx_type, eob);
        let p = &mut self.planes[plane];
        for i in 0..4 {
            for j in 0..4 {
                p.rec[(y + i) * p.w + x + j] = (pred[i * 4 + j] + resid[i * 4 + j]).clamp(0, 255) as u8;
            }
        }
        let nz = (eob > 0) as u8;
        self.above_nz[plane][x4] = nz;
        self.left_nz[plane][ly] = nz;
    }

    fn tokens(&mut self, plane: usize, inter: bool, tx_type: u8, levels: &[i32; 16], eob: usize, ctx0: usize) {
        let sc = scan(tx_type);
        let nb = neighbors(tx_type);
        let ptype = (plane > 0) as usize;
        let mut tc = [0u8; 16];
        let mut check_eob = true;
        let mut c = 0;
        while c < 16 {
            let pos = sc[c] as usize;
            let band = COEFBAND_4X4[c] as usize;
            let ctx = if c == 0 { ctx0 } else { ((1 + tc[nb[c].0] + tc[nb[c].1]) >> 1) as usize };
            let o = probs::coef_index(ptype, inter, band, ctx);
            if check_eob {
                let more = c < eob;
                self.w.ctx(more, o);
                if !more {
                    break;
                }
            }
            let v = levels[pos].unsigned_abs();
            if v == 0 {
                self.w.ctx(false, o + 1);
                tc[pos] = 0;
                check_eob = false;
                c += 1;
                continue;
            }
            self.w.ctx(true, o + 1);
            check_eob = true;
            let token = match v {
                1 => 1,
                2 => 2,
                3 => 3,
                4 => 4,
                5..=6 => 5,
                7..=10 => 6,
                11..=18 => 7,
                19..=34 => 8,
                35..=66 => 9,
                _ => 10,
            };
            if token == 1 {
                self.w.ctx(false, o + 2);
            } else {
                self.w.ctx(true, o + 2);
                let path: &[(u8, bool)] = match token {
                    2 => &[(0, false), (1, false)],
                    3 => &[(0, false), (1, true), (2, false)],
                    4 => &[(0, false), (1, true), (2, true)],
                    5 => &[(0, true), (3, false), (4, false)],
                    6 => &[(0, true), (3, false), (4, true)],
                    7 => &[(0, true), (3, true), (5, false), (6, false)],
                    8 => &[(0, true), (3, true), (5, false), (6, true)],
                    9 => &[(0, true), (3, true), (5, true), (7, false)],
                    _ => &[(0, true), (3, true), (5, true), (7, true)],
                };
                for &(n, b) in path {
                    self.w.put(b, P::Pareto((o + 2) as u16, n));
                }
                let (cat, num_extra, base_v) = EXTRA_BITS[token];
                let extra = (v - base_v as u32).min((1 << num_extra) - 1);
                let cp = CAT_PROBS[cat as usize];
                for e in 0..num_extra as usize {
                    self.w.fixed((extra >> (num_extra as usize - 1 - e)) & 1 != 0, cp[e]);
                }
            }
            tc[pos] = ENERGY_CLASS[token];
            self.w.fixed(levels[pos] < 0, 128);
            c += 1;
        }
    }

    // ------------------------------------------------------------ inter

    fn is_inside(&self, r: i32, c: i32) -> bool {
        r >= 0 && (r as usize) < self.f.mi_rows && c >= self.col_start as i32 && (c as usize) < self.col_end
    }

    /// `find_mv_refs` + `find_best_ref_mvs` for LAST_FRAME: (NearestMv, NearMv, mode context).
    fn mv_refs(&self, r: usize, c: usize, bsize: u8) -> (Mv, Mv, usize) {
        let search = if bsize == BLOCK_16X16 { &MV_REF_16X16 } else { &MV_REF_8X8 };
        let mut list = [Mv::ZERO; 2];
        let mut count = 0;
        let add = |mv: Mv, list: &mut [Mv; 2], count: &mut usize| {
            if *count >= 2 || (*count > 0 && mv == list[0]) {
                return;
            }
            list[*count] = mv;
            *count += 1;
        };
        let mut counter = 0usize;
        for (i, &(dr, dc)) in search.iter().enumerate() {
            let (cr, cc) = (r as i32 + dr, c as i32 + dc);
            if !self.is_inside(cr, cc) {
                continue;
            }
            let m = self.mi_at(cr as usize, cc as usize);
            if i < 2 {
                counter += MODE_2_COUNTER[m.mode as usize] as usize;
            }
            if m.inter {
                add(m.mv, &mut list, &mut count);
            }
        }
        let ctx = COUNTER_TO_CONTEXT[counter.min(18)] as usize;
        let n8 = if bsize == BLOCK_16X16 { 2 } else { 1 };
        let clamp = |mv: Mv, border: i32| -> Mv {
            let (r, c, b) = (r as i32, c as i32, n8);
            let to_top = -(r * 64);
            let to_bottom = (self.f.mi_rows as i32 - b - r) * 64;
            let to_left = -(c * 64);
            let to_right = (self.f.mi_cols as i32 - b - c) * 64;
            Mv::new(
                mv.row.clamp(to_top - border, (to_bottom + border).max(to_top - border)),
                mv.col.clamp(to_left - border, (to_right + border).max(to_left - border)),
            )
        };
        for mv in list.iter_mut() {
            *mv = clamp(*mv, 128);
            // No high precision: odd components move toward zero.
            let even = |v: i32| if v & 1 != 0 { v + if v > 0 { -1 } else { 1 } } else { v };
            *mv = clamp(Mv::new(even(mv.row), even(mv.col)), (160 - 4) << 3);
        }
        (list[0], list[1], ctx)
    }

    /// The allowed motion vector range of a block (no decoder clamping inside it).
    fn mv_range(&self, r: usize, c: usize, n: i32) -> (i32, i32, i32, i32) {
        let n8 = n / 8;
        let (r, c) = (r as i32, c as i32);
        let lo_c = -(c * 64) - (4 + n) * 8;
        let hi_c = (self.f.mi_cols as i32 - n8 - c) * 64 + (4 + n) * 8 - 8;
        let lo_r = -(r * 64) - (4 + n) * 8;
        let hi_r = (self.f.mi_rows as i32 - n8 - r) * 64 + (4 + n) * 8 - 8;
        let lim = 8 * 1000;
        (lo_r.max(-lim), hi_r.min(lim), lo_c.max(-lim), hi_c.min(lim))
    }

    /// SAD of the luma prediction at `mv` against the source.
    fn sad(&self, r: usize, c: usize, n: usize, mv: Mv) -> u32 {
        let p = &self.planes[0];
        let (x16, y16) = (((c * 8) << 4) as i32 + 2 * mv.col, ((r * 8) << 4) as i32 + 2 * mv.row);
        let mut s = 0u32;
        if let Some((pl, o, st)) = self.f.phases.as_ref().and_then(|ph| ph.block(x16, y16, n)) {
            let a = &p.src[(r * 8) * p.w + c * 8..];
            let b = &pl[o..];
            return match n {
                8 => sad_n::<8>(a, p.w, b, st),
                16 => sad_n::<16>(a, p.w, b, st),
                _ => {
                    for i in 0..n {
                        s += a[i * p.w..][..n].iter().zip(&b[i * st..][..n]).map(|(a, b)| a.abs_diff(*b) as u32).sum::<u32>();
                    }
                    s
                }
            };
        }
        // Inter search only runs on frames with a reference.
        let Some(refs) = self.f.refs.as_ref() else { return u32::MAX / 4 };
        let mut pred = [0u8; 256];
        predict(&refs[0], x16, y16, n, n, &mut pred);
        for i in 0..n {
            let a = &p.src[(r * 8 + i) * p.w + c * 8..][..n];
            s += a.iter().zip(&pred[i * n..i * n + n]).map(|(a, b)| (*a as i32 - *b as i32).unsigned_abs()).sum::<u32>();
        }
        s
    }

    /// Motion search for the block at (`r`, `c`): the best vector and its cost
    /// (SAD + λ·bits).
    fn search(&self, r: usize, c: usize, bsize: u8) -> (Mv, f64) {
        let n = if bsize == BLOCK_16X16 { 16 } else { 8 };
        let (nearest, near, mctx) = self.mv_refs(r, c, bsize);
        let (lo_r, hi_r, lo_c, hi_c) = self.mv_range(r, c, n as i32);
        let inside = |mv: Mv| mv.row >= lo_r && mv.row <= hi_r && mv.col >= lo_c && mv.col <= hi_c;
        let lambda = self.f.lambda_sad / 256.0;
        let d = &probs_defaults().0;
        let mb: [u32; 4] = std::array::from_fn(|m| tree_cost(&INTER_MODE_TREE, &d[probs::INTER_MODE + mctx * 3..], m as u8));
        let mode_bits = |m: u8| mb[(m - NEARESTMV) as usize];
        let cost_raw = |mv: Mv| -> f64 {
            let bits = if mv == nearest {
                mode_bits(NEARESTMV)
            } else if mv == Mv::ZERO {
                mode_bits(ZEROMV)
            } else if mv == near {
                mode_bits(NEARMV)
            } else {
                mode_bits(NEWMV) + mv_cost(mv.minus(nearest))
            };
            self.sad(r, c, n, mv) as f64 + lambda * bits as f64
        };
        // The cost is a pure function of the vector: points the search revisits (the centre it
        // came from, candidates on the diamond) are looked up instead of predicted again.
        let mut seen = [(Mv::ZERO, 0.0f64); 64];
        let mut nseen = 0usize;
        let mut cost = |mv: Mv| -> f64 {
            if let Some(&(_, j)) = seen[..nseen].iter().find(|e| e.0 == mv) {
                return j;
            }
            let j = cost_raw(mv);
            if nseen < seen.len() {
                seen[nseen] = (mv, j);
                nseen += 1;
            }
            j
        };
        let mut best = (Mv::ZERO, cost(Mv::ZERO));
        let prev = self.f.prev_mvs.map(|p| p[r * self.f.mi_cols + c]);
        for cand in [Some(nearest), Some(near), prev].into_iter().flatten() {
            if cand != Mv::ZERO && inside(cand) {
                let j = cost(cand);
                if j < best.1 {
                    best = (cand, j);
                }
            }
        }
        // A perfect match needs no search.
        let good_enough = lambda * 256.0 * 0.5;
        if best.1 > good_enough {
            // Whole-sample search from the best candidate: a coarse cross, then a small diamond.
            let round8 = |v: i32| (v + 4).div_euclid(8) * 8;
            let mut cur = Mv::new(round8(best.0.row), round8(best.0.col));
            let mut cur_j = if inside(cur) { cost(cur) } else { f64::MAX };
            if cur_j < best.1 {
                best = (cur, cur_j);
            } else {
                cur = best.0;
                cur_j = best.1;
            }
            for (step, max_iters) in [(16, 1), (8, 2), (4, 3), (2, 4), (1, 8)] {
                let s = step * 8;
                for _ in 0..max_iters {
                    let mut moved = false;
                    let center = cur;
                    for (dr, dc) in [(-s, 0), (s, 0), (0, -s), (0, s)] {
                        let cand = Mv::new(center.row + dr, center.col + dc);
                        if !inside(cand) {
                            continue;
                        }
                        let j = cost(cand);
                        if j < cur_j {
                            cur = cand;
                            cur_j = j;
                            moved = true;
                        }
                    }
                    if !moved {
                        break;
                    }
                }
            }
            if cur_j < best.1 {
                best = (cur, cur_j);
            }
        }
        // Sub-sample refinement: half then quarter samples (1/8 units, even: no high precision).
        for s in [4, 2] {
            let center = best.0;
            for (dr, dc) in [(-s, 0), (s, 0), (0, -s), (0, s), (-s, -s), (-s, s), (s, -s), (s, s)] {
                let cand = Mv::new(center.row + dr, center.col + dc);
                if !inside(cand) {
                    continue;
                }
                let j = cost(cand);
                if j < best.1 {
                    best = (cand, j);
                }
            }
        }
        best
    }

    /// Choose between one 16×16 block and four 8×8 blocks at (`r`, `c`): `Some(mv)` for 16×16.
    fn decide16(&mut self, r: usize, c: usize) -> Option<Mv> {
        let (mv16, j16) = self.search(r, c, BLOCK_16X16);
        let lambda = self.f.lambda_sad;
        // A near-perfect 16×16 prediction needs no split.
        if j16 < 2.0 * 256.0 / 256.0 * 16.0 {
            return Some(mv16);
        }
        let mut j8 = lambda * 4.0; // partition split + three more blocks of mode info (≈ 4 bits)
        for (k, (dr, dc)) in [(0, 0), (0, 1), (1, 0), (1, 1)].into_iter().enumerate() {
            let (rr, cc) = (r + dr, c + dc);
            let (mv, j) = self.search(rr, cc, BLOCK_8X8);
            j8 += j;
            self.pending[k] = (rr, cc, mv);
            // Tentative mode info so the next 8×8 blocks see this one as a candidate.
            self.set_mi(rr, cc, 1, Mi { inter: true, mode: NEWMV, mv, skip: false, size: BLOCK_8X8 });
        }
        if j16 <= j8 {
            self.pending = [(usize::MAX, 0, Mv::ZERO); 4];
            Some(mv16)
        } else {
            None
        }
    }

    /// Code an inter block with motion vector `mv`.
    fn inter_block(&mut self, r: usize, c: usize, bsize: u8, mv: Mv) {
        let (mi_rows, mi_cols) = (self.f.mi_rows, self.f.mi_cols);
        let n8 = if bsize == BLOCK_16X16 { 2 } else { 1 };
        let n = 8 * n8;
        let (nearest, near, mctx) = self.mv_refs(r, c, bsize);
        let d = &probs_defaults().0;
        let mode_bits = |m: u8| tree_cost(&INTER_MODE_TREE, &d[probs::INTER_MODE + mctx * 3..], m - NEARESTMV);
        let mut mode = (NEWMV, mode_bits(NEWMV) + mv_cost(mv.minus(nearest)));
        for (m, ok) in [(NEARESTMV, mv == nearest), (NEARMV, mv == near), (ZEROMV, mv == Mv::ZERO)] {
            if ok && mode_bits(m) < mode.1 {
                mode = (m, mode_bits(m));
            }
        }
        let mode = mode.0;
        // Prediction of every plane (written straight into the reconstruction).
        let Some(refs) = self.f.refs.as_ref() else { return };
        for plane in 0..3 {
            let ch = plane > 0;
            let (row16, col16) = clamp_for_plane(mv, r, c, n8, mi_rows, mi_cols, ch);
            let bn = if ch { n / 2 } else { n };
            let (bx, by) = if ch { (c * 4, r * 4) } else { (c * 8, r * 8) };
            let mut pred = [0u8; 256];
            predict(&refs[plane], ((bx as i32) << 4) + col16, ((by as i32) << 4) + row16, bn, bn, &mut pred);
            let p = &mut self.planes[plane];
            for i in 0..bn {
                p.rec[(by + i) * p.w + bx..][..bn].copy_from_slice(&pred[i * bn..i * bn + bn]);
            }
        }
        // Residual: quantise every 4×4, dropping blocks whose coefficients don't pay for themselves.
        let q = Quant { round_dc: 0.5, round_ac: 0.42, ..self.f.quant };
        // Weight of the rate term when dropping a transform block's coefficients (low: keep quality on par
        // with key frames at the same q index).
        let zl = 0.07;
        let tx = DCT_DCT;
        let mut blocks: Vec<(usize, usize, usize, [i32; 16], usize)> = Vec::with_capacity(24);
        let mut any = false;
        for plane in 0..3 {
            let ch = plane > 0;
            let bn = if ch { n / 2 } else { n };
            let (bx, by) = if ch { (c * 4, r * 4) } else { (c * 8, r * 8) };
            let p = &self.planes[plane];
            for ty in (0..bn).step_by(4) {
                for tx4 in (0..bn).step_by(4) {
                    let (x, y) = (bx + tx4, by + ty);
                    let mut res = [0i32; 16];
                    for i in 0..4 {
                        for j in 0..4 {
                            let k = (y + i) * p.w + x + j;
                            res[i * 4 + j] = p.src[k] as i32 - p.rec[k] as i32;
                        }
                    }
                    let mut levels = q.levels(&res, tx);
                    let mut eob = eob_of(&levels, tx);
                    if eob > 0 && !q.lossless {
                        let back = q.residual(&levels, tx, eob);
                        let d_skip: i64 = res.iter().map(|v| (*v as i64) * (*v as i64)).sum();
                        let d_code: i64 = res.iter().zip(&back).map(|(a, b)| ((a - b) as i64).pow(2)).sum();
                        if d_skip as f64 <= d_code as f64 + zl * self.f.lambda_sse * levels_cost(&levels, eob) as f64 / 256.0 {
                            levels = [0; 16];
                            eob = 0;
                        }
                    }
                    any |= eob > 0;
                    blocks.push((plane, x, y, levels, eob));
                }
            }
        }
        let skip = !any;
        // Mode info.
        let avail_u = r > 0;
        let avail_l = c > self.col_start;
        let above = avail_u.then(|| self.mi_at(r - 1, c));
        let left = avail_l.then(|| self.mi_at(r, c - 1));
        let skip_ctx = above.is_some_and(|m| m.skip) as usize + left.is_some_and(|m| m.skip) as usize;
        self.w.ctx(skip, probs::SKIP + skip_ctx);
        // is_inter context: neighbours are inter (no intra blocks in inter frames).
        let intra = |m: Option<Mi>| m.is_some_and(|m| !m.inter);
        let inter_ctx = match (above, left) {
            (Some(_), Some(_)) => {
                if intra(above) && intra(left) {
                    3
                } else {
                    (intra(above) || intra(left)) as usize
                }
            }
            (Some(_), None) => 2 * intra(above) as usize,
            (None, Some(_)) => 2 * intra(left) as usize,
            (None, None) => 0,
        };
        self.w.ctx(true, probs::IS_INTER + inter_ctx);
        // single_ref_p1 = 0 (LAST_FRAME); context for single-reference LAST neighbours.
        let ref_ctx = match (above, left) {
            (Some(a), Some(l)) => match (a.inter, l.inter) {
                (false, false) => 2,
                (false, true) => 4,
                (true, false) => 4,
                (true, true) => 4,
            },
            (Some(m), None) | (None, Some(m)) => {
                if m.inter {
                    4
                } else {
                    2
                }
            }
            (None, None) => 2,
        };
        self.w.ctx(false, probs::SINGLE_REF + ref_ctx * 2);
        self.w.tree(&INTER_MODE_TREE, mode - NEARESTMV, |i| P::Ctx((probs::INTER_MODE + mctx * 3 + i) as u16));
        if mode == NEWMV {
            self.write_mv(mv.minus(nearest));
        }
        self.set_mi(r, c, n8, Mi { inter: true, mode, mv, skip, size: bsize });
        // Tokens and reconstruction.
        for (plane, x, y, levels, eob) in blocks {
            let ss = (plane > 0) as usize;
            let (x4, ly) = (x >> 2, (y >> 2) & (15 >> ss));
            if !skip {
                let ctx0 = (self.above_nz[plane][x4] + self.left_nz[plane][ly]) as usize;
                self.tokens(plane, true, tx, &levels, eob, ctx0);
                if eob > 0 {
                    let resid = q.residual(&levels, tx, eob);
                    let p = &mut self.planes[plane];
                    for i in 0..4 {
                        for j in 0..4 {
                            let k = (y + i) * p.w + x + j;
                            p.rec[k] = (p.rec[k] as i32 + resid[i * 4 + j]).clamp(0, 255) as u8;
                        }
                    }
                }
            }
            let nz = (!skip && eob > 0) as u8;
            self.above_nz[plane][x4] = nz;
            self.left_nz[plane][ly] = nz;
        }
    }

    fn write_mv(&mut self, diff: Mv) {
        let joint = (diff.row != 0) as u8 * 2 + (diff.col != 0) as u8;
        self.w.tree(&MV_JOINT_TREE, joint, |i| P::Ctx((probs::MV_JOINT + i) as u16));
        if diff.row != 0 {
            self.write_mv_component(0, diff.row);
        }
        if diff.col != 0 {
            self.write_mv_component(1, diff.col);
        }
    }

    fn write_mv_component(&mut self, i: usize, v: i32) {
        debug_assert!(v != 0 && v & 1 == 0, "vectors are even without high precision");
        let offset = v.unsigned_abs() - 1;
        let c = mv_class(offset);
        self.w.ctx(v < 0, probs::MV_SIGN + i);
        self.w.tree(&MV_CLASS_TREE, c as u8, |k| P::Ctx((probs::MV_CLASS + i * 10 + k) as u16));
        let fr = ((offset >> 1) & 3) as u8;
        if c == 0 {
            let b = (offset >> 3) as usize;
            self.w.ctx(b == 1, probs::MV_CLASS0_BIT + i);
            self.w.tree(&MV_FR_TREE, fr, |k| P::Ctx((probs::MV_CLASS0_FR + i * 6 + b * 3 + k) as u16));
        } else {
            let dd = (offset - (1 << (c + 3))) >> 3;
            for b in 0..c {
                self.w.ctx((dd >> b) & 1 == 1, probs::MV_BITS + i * 10 + b as usize);
            }
            self.w.tree(&MV_FR_TREE, fr, |k| P::Ctx((probs::MV_FR + i * 3 + k) as u16));
        }
    }
}

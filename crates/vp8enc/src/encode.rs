//! Mode decision, quantization, reconstruction and the bitstream of a key frame (RFC 6386 §9–§14
//! and Annex A).
//!
//! Three passes over the macroblocks: the first chooses each one's modes and quantized
//! coefficients and reconstructs it as a decoder will (later macroblocks are predicted from that
//! reconstruction); the second counts the coefficient tokens, from which the probability updates
//! are chosen; the third writes the first partition (header and modes) and the token partition.

use crate::bool::{BoolWriter, cost, put_tree, tree_cost};
use crate::predict::{B_DC_PRED, B_HE_PRED, B_PRED, B_TM_PRED, B_VE_PRED, DC_PRED, Edges, H_PRED, TM_PRED, V_PRED, predict_block, predict_sub};
use crate::tables::{
    AC_QLOOKUP, CAT_BASE, COEFF_BANDS, COEFF_UPDATE_PROBS, DC_QLOOKUP, DEFAULT_COEFF_PROBS, KF_BMODE_PROB, KF_UV_MODE_PROB, KF_YMODE_PROB, PCAT,
};
use crate::transform::{fdct, fwht, idct, iwht};
use crate::{Vp8Error, Vp8Frame, Vp8Params, q_index};

/// One 8-bit plane.
#[derive(Clone, Debug, PartialEq)]
pub struct Plane {
    pub width: usize,
    pub height: usize,
    pub data: Vec<u8>,
}

impl Plane {
    /// A zeroed `width`×`height` plane.
    pub(crate) fn new(width: usize, height: usize) -> Plane {
        Plane { width, height, data: vec![0; width * height] }
    }
    /// The sample at (x, y); 0 outside the plane.
    pub fn get(&self, x: usize, y: usize) -> u8 {
        if x < self.width { self.data.get(y * self.width + x).copied().unwrap_or(0) } else { 0 }
    }
    /// Set the sample at (x, y); writes outside the plane are ignored.
    pub(crate) fn set(&mut self, x: usize, y: usize, v: u8) {
        if x < self.width
            && let Some(p) = self.data.get_mut(y * self.width + x)
        {
            *p = v;
        }
    }
}

/// The scan order of coefficients: position in the scan → raster index in the 4×4 block.
const ZIGZAG: [usize; 16] = [0, 1, 4, 8, 5, 2, 3, 6, 9, 12, 13, 10, 7, 11, 14, 15];

/// The mode trees of §11.2 and §11.4 in the RFC's array form.
const KF_YMODE_TREE: [i8; 8] = [-4, 2, 4, 6, 0, -1, -2, -3];
const UV_MODE_TREE: [i8; 6] = [0, 2, -1, 4, -2, -3];
const BMODE_TREE: [i8; 18] = [0, 2, -1, 4, -2, 6, 8, 12, -3, 10, -5, -6, -4, 14, -7, 16, -8, -9];

/// Coefficient block types, the outer index of the token probabilities (§13.3).
const TYPE_Y_AFTER_Y2: usize = 0;
const TYPE_Y2: usize = 1;
const TYPE_UV: usize = 2;
const TYPE_Y_WITH_DC: usize = 3;

type Probs = [[[[u8; 11]; 3]; 8]; 4];

/// Lagrange multiplier per squared AC step: the weight of a bit against squared error.
const LAMBDA_PER_STEP2: f64 = 0.02;

/// Dequantization factors, `[dc, ac]` for luma, Y2 and chroma, from one quantizer index.
#[derive(Clone, Copy)]
struct Quant {
    y1: [i32; 2],
    y2: [i32; 2],
    uv: [i32; 2],
}

impl Quant {
    /// The dequantization factors for quantizer index `q` (0–127, §14.1).
    fn new(q: u8) -> Quant {
        let q = usize::from(q.min(127));
        let (dc, ac) = (DC_QLOOKUP[q], AC_QLOOKUP[q]);
        Quant { y1: [dc, ac], y2: [dc * 2, (ac * 155 / 100).max(8)], uv: [dc.min(132), ac] }
    }
}

/// A coefficient quantized to a level a decoder can hold: the token alphabet stops at 2048 and the
/// dequantized value is a 16-bit integer.
fn quantize(c: f64, dq: i32, dc: bool) -> i16 {
    let bias = if dc { 0.45 } else { 0.38 };
    let max = (32767 / dq.max(1)).min(2048);
    let level = ((c.abs() / f64::from(dq.max(1))) + bias).floor().min(f64::from(max)) as i16;
    if c < 0.0 { -level } else { level }
}

/// A level times its dequantization factor, as a decoder computes it.
fn dequant(level: i16, dq: i32) -> i16 {
    (i32::from(level) * dq) as i16
}

/// Whether neighbouring blocks had non-zero coefficients, per plane: the first token's context.
#[derive(Clone, Copy, Default)]
struct Nz {
    y: [bool; 4],
    u: [bool; 2],
    v: [bool; 2],
    y2: bool,
}

/// What is decided for a macroblock. Levels are in raster order within each 4×4 block.
#[derive(Clone)]
struct Mb {
    ymode: u8,
    bmodes: [u8; 16],
    uvmode: u8,
    skip: bool,
    y2: [i16; 16],
    y: [[i16; 16]; 16],
    u: [[i16; 16]; 4],
    v: [[i16; 16]; 4],
}

/// The subblock mode a whole-block luma mode stands for in later blocks' contexts (§11.3).
fn implied_bmode(ymode: u8) -> u8 {
    match ymode {
        V_PRED => B_VE_PRED,
        H_PRED => B_HE_PRED,
        TM_PRED => B_TM_PRED,
        _ => B_DC_PRED,
    }
}

/// Receives the bools of coefficient tokens: to cost them, count them or write them.
trait TokenSink {
    /// `node` is the token tree node (for counting), `None` for extra bits and signs.
    fn bit(&mut self, ty: usize, band: usize, ctx: usize, node: Option<usize>, prob: u8, bit: bool);
}

struct CostSink(f32);
impl TokenSink for CostSink {
    /// Adds the bit's cost.
    fn bit(&mut self, _: usize, _: usize, _: usize, _: Option<usize>, prob: u8, bit: bool) {
        self.0 += cost(prob, bit);
    }
}

struct CountSink(Box<[[[[[u32; 2]; 11]; 3]; 8]; 4]>);
impl TokenSink for CountSink {
    /// Counts the bit against its tree node, for choosing probabilities.
    fn bit(&mut self, ty: usize, band: usize, ctx: usize, node: Option<usize>, _: u8, bit: bool) {
        if let Some(n) = node {
            self.0[ty][band][ctx][n][usize::from(bit)] += 1;
        }
    }
}

impl TokenSink for BoolWriter {
    /// Writes the bit.
    fn bit(&mut self, _: usize, _: usize, _: usize, _: Option<usize>, prob: u8, bit: bool) {
        self.put(prob, bit);
    }
}

/// Codes one token (§13.2): its tree path, extra bits and sign.
fn token<S: TokenSink>(s: &mut S, probs: &Probs, ty: usize, band: usize, ctx: usize, v: i16, skip_eob: bool) {
    let p = &probs[ty][band][ctx];
    let node = |s: &mut S, n: usize, bit: bool| s.bit(ty, band, ctx, Some(n), p[n], bit);
    if !skip_eob {
        node(s, 0, true);
    }
    let a = u32::from(v.unsigned_abs());
    if a == 0 {
        node(s, 1, false);
        return;
    }
    node(s, 1, true);
    if a == 1 {
        node(s, 2, false);
    } else {
        node(s, 2, true);
        if a <= 4 {
            node(s, 3, false);
            if a == 2 {
                node(s, 4, false);
            } else {
                node(s, 4, true);
                node(s, 5, a == 4);
            }
        } else {
            node(s, 3, true);
            let cat = match a {
                5..=6 => 0,
                7..=10 => 1,
                11..=18 => 2,
                19..=34 => 3,
                35..=66 => 4,
                _ => 5,
            };
            if cat <= 1 {
                node(s, 6, false);
                node(s, 7, cat == 1);
            } else {
                node(s, 6, true);
                if cat <= 3 {
                    node(s, 8, false);
                    node(s, 9, cat == 3);
                } else {
                    node(s, 8, true);
                    node(s, 10, cat == 5);
                }
            }
            let extra = a - CAT_BASE[cat];
            let bits = PCAT[cat];
            for (k, &pp) in bits.iter().enumerate() {
                s.bit(ty, band, ctx, None, pp, (extra >> (bits.len() - 1 - k)) & 1 == 1);
            }
        }
    }
    s.bit(ty, band, ctx, None, 128, v < 0);
}

/// Codes a block's coefficients from scan position `first` with neighbour context `ctx`;
/// returns whether any of them is non-zero.
fn code_block<S: TokenSink>(s: &mut S, probs: &Probs, levels: &[i16; 16], ty: usize, first: usize, ctx: usize) -> bool {
    let Some(last) = (first..16).rev().find(|&i| levels[ZIGZAG[i]] != 0) else {
        let band = COEFF_BANDS[first];
        s.bit(ty, band, ctx, Some(0), probs[ty][band][ctx][0], false);
        return false;
    };
    let mut c = ctx;
    let mut skip_eob = false;
    for i in first..=last {
        let v = levels[ZIGZAG[i]];
        token(s, probs, ty, COEFF_BANDS[i], c, v, skip_eob);
        let a = v.unsigned_abs();
        c = match a {
            0 => 0,
            1 => 1,
            _ => 2,
        };
        skip_eob = a == 0;
    }
    if last < 15 {
        let band = COEFF_BANDS[last + 1];
        s.bit(ty, band, c, Some(0), probs[ty][band][c][0], false);
    }
    true
}

/// The bits a block of levels costs to code, from its first coefficient.
fn block_cost(probs: &Probs, levels: &[i16; 16], ty: usize, first: usize, ctx: usize) -> f32 {
    let mut s = CostSink(0.0);
    code_block(&mut s, probs, levels, ty, first, ctx);
    s.0
}

/// Whether any coefficient from `first` on is non-zero.
fn any_coded(levels: &[i16; 16], first: usize) -> bool {
    (first..16).any(|i| levels[ZIGZAG[i]] != 0)
}

/// Runs every macroblock's coefficients through `sink` with the contexts a decoder keeps.
fn tokens<S: TokenSink>(s: &mut S, probs: &Probs, mbs: &[Mb], mbw: usize) {
    let mut above = vec![Nz::default(); mbw];
    let mut left = Nz::default();
    for (i, mb) in mbs.iter().enumerate() {
        let x = i % mbw;
        if x == 0 {
            left = Nz::default();
        }
        let Some(up) = above.get_mut(x) else { return };
        let has_y2 = mb.ymode != B_PRED;
        if mb.skip {
            // Nothing coded: every block is empty, and so is Y2 for a macroblock that has one.
            up.y = [false; 4];
            up.u = [false; 2];
            up.v = [false; 2];
            left.y = [false; 4];
            left.u = [false; 2];
            left.v = [false; 2];
            if has_y2 {
                up.y2 = false;
                left.y2 = false;
            }
            continue;
        }
        let (ty, first) = if has_y2 {
            let nz = code_block(s, probs, &mb.y2, TYPE_Y2, 0, usize::from(left.y2) + usize::from(up.y2));
            left.y2 = nz;
            up.y2 = nz;
            (TYPE_Y_AFTER_Y2, 1)
        } else {
            (TYPE_Y_WITH_DC, 0)
        };
        for (b, levels) in mb.y.iter().enumerate() {
            let (r, c) = (b / 4, b % 4);
            let nz = code_block(s, probs, levels, ty, first, usize::from(left.y[r]) + usize::from(up.y[c]));
            left.y[r] = nz;
            up.y[c] = nz;
        }
        for (b, levels) in mb.u.iter().enumerate() {
            let (r, c) = (b / 2, b % 2);
            let nz = code_block(s, probs, levels, TYPE_UV, 0, usize::from(left.u[r]) + usize::from(up.u[c]));
            left.u[r] = nz;
            up.u[c] = nz;
        }
        for (b, levels) in mb.v.iter().enumerate() {
            let (r, c) = (b / 2, b % 2);
            let nz = code_block(s, probs, levels, TYPE_UV, 0, usize::from(left.v[r]) + usize::from(up.v[c]));
            left.v[r] = nz;
            up.v[c] = nz;
        }
    }
}

/// The probabilities to code the frame's tokens with: each default replaced where the frame's
/// own counts save more than the update costs (§13.4).
fn updated_probs(counts: &CountSink) -> Probs {
    let mut probs = DEFAULT_COEFF_PROBS;
    for t in 0..4 {
        for b in 0..8 {
            for c in 0..3 {
                for n in 0..11 {
                    let [n0, n1] = counts.0[t][b][c][n];
                    let total = n0 + n1;
                    if total == 0 {
                        continue;
                    }
                    let old = DEFAULT_COEFF_PROBS[t][b][c][n];
                    let new = ((u64::from(n0) * 256 + u64::from(total) / 2) / u64::from(total)).clamp(1, 255) as u8;
                    let bits = |p: u8| n0 as f32 * cost(p, false) + n1 as f32 * cost(p, true);
                    let upd = COEFF_UPDATE_PROBS[t][b][c][n];
                    let keep = bits(old) + cost(upd, false);
                    let change = bits(new) + cost(upd, true) + 8.0;
                    if new != old && change < keep {
                        probs[t][b][c][n] = new;
                    }
                }
            }
        }
    }
    probs
}

struct Encoder<'a> {
    src_y: &'a Plane,
    src_u: &'a Plane,
    src_v: &'a Plane,
    rec_y: Plane,
    rec_u: Plane,
    rec_v: Plane,
    mbw: usize,
    q: Quant,
    lambda: f64,
    probs: Probs,
    above_nz: Vec<Nz>,
    left_nz: Nz,
    above_bmodes: Vec<[u8; 4]>,
    left_bmodes: [u8; 4],
}

/// A candidate luma coding of a macroblock.
struct Luma {
    mode: u8,
    bmodes: [u8; 16],
    y2: [i16; 16],
    y: [[i16; 16]; 16],
    recon: [u8; 256],
    sse: u64,
    bits: f32,
}

/// A candidate chroma coding.
struct Chroma {
    mode: u8,
    u: [[i16; 16]; 4],
    v: [[i16; 16]; 4],
    recon_u: [u8; 64],
    recon_v: [u8; 64],
    sse: u64,
    bits: f32,
}

/// `v` clamped to a sample.
fn clamp255(v: i32) -> u8 {
    v.clamp(0, 255) as u8
}

impl Encoder<'_> {
    /// The rate-distortion cost: the squared error plus λ times the bits.
    fn cost_of(&self, sse: u64, bits: f32) -> f64 {
        sse as f64 + self.lambda * f64::from(bits)
    }

    /// The 16×16 luma edges, and the four pixels above and to the right (§12.3).
    fn luma_edges(&self, mbx: usize, mby: usize) -> (Edges<16>, [u8; 4]) {
        let (x0, y0) = (mbx * 16, mby * 16);
        let rec = &self.rec_y;
        let above: [u8; 16] = std::array::from_fn(|i| if mby == 0 { 127 } else { rec.get(x0 + i, y0 - 1) });
        let left: [u8; 16] = std::array::from_fn(|i| if mbx == 0 { 129 } else { rec.get(x0 - 1, y0 + i) });
        let corner = if mby == 0 {
            127
        } else if mbx == 0 {
            129
        } else {
            rec.get(x0 - 1, y0 - 1)
        };
        let right: [u8; 4] = std::array::from_fn(|i| {
            if mby == 0 {
                127
            } else if mbx + 1 == self.mbw {
                rec.get(x0 + 15, y0 - 1)
            } else {
                rec.get(x0 + 16 + i, y0 - 1)
            }
        });
        (Edges { above, left, corner, have_above: mby > 0, have_left: mbx > 0 }, right)
    }

    /// The reconstructed edges an 8×8 chroma block predicts from, with the spec's 127 / 129 outside the frame.
    fn chroma_edges(rec: &Plane, mbx: usize, mby: usize) -> Edges<8> {
        let (x0, y0) = (mbx * 8, mby * 8);
        let above: [u8; 8] = std::array::from_fn(|i| if mby == 0 { 127 } else { rec.get(x0 + i, y0 - 1) });
        let left: [u8; 8] = std::array::from_fn(|i| if mbx == 0 { 129 } else { rec.get(x0 - 1, y0 + i) });
        let corner = if mby == 0 {
            127
        } else if mbx == 0 {
            129
        } else {
            rec.get(x0 - 1, y0 - 1)
        };
        Edges { above, left, corner, have_above: mby > 0, have_left: mbx > 0 }
    }

    /// A 4×4 block of a plane minus a prediction.
    fn residual(src: &Plane, x0: usize, y0: usize, pred: &[u8], stride: usize) -> [i32; 16] {
        std::array::from_fn(|i| {
            let (r, c) = (i / 4, i % 4);
            i32::from(src.get(x0 + c, y0 + r)) - i32::from(pred[r * stride + c])
        })
    }

    /// Code the macroblock's luma with one 16×16 prediction `mode`: its residual through Y2 and its cost.
    fn whole_luma(&self, mbx: usize, mby: usize, mode: u8, e: &Edges<16>) -> Luma {
        let (x0, y0) = (mbx * 16, mby * 16);
        let mut pred = [0u8; 256];
        predict_block(mode, e, &mut pred);
        let coefs: [[f64; 16]; 16] = std::array::from_fn(|b| {
            let (r, c) = (b / 4, b % 4);
            fdct(&Self::residual(self.src_y, x0 + c * 4, y0 + r * 4, &pred[r * 64 + c * 4..], 16))
        });
        let dc: [f64; 16] = std::array::from_fn(|b| coefs[b][0]);
        let y2c = fwht(&dc);
        let y2: [i16; 16] = std::array::from_fn(|i| quantize(y2c[i], self.q.y2[usize::from(i > 0)], i == 0));
        let y: [[i16; 16]; 16] = std::array::from_fn(|b| std::array::from_fn(|i| if i == 0 { 0 } else { quantize(coefs[b][i], self.q.y1[1], false) }));
        let y2dq: [i16; 16] = std::array::from_fn(|i| dequant(y2[i], self.q.y2[usize::from(i > 0)]));
        let dcs = iwht(&y2dq);
        let mut recon = [0u8; 256];
        let mut sse = 0u64;
        let (mut ly, mut ay) = (self.left_nz.y, self.above_nz.get(mbx).copied().unwrap_or_default().y);
        let up = self.above_nz.get(mbx).copied().unwrap_or_default();
        let mut bits =
            tree_cost(&KF_YMODE_TREE, &KF_YMODE_PROB, mode) + block_cost(&self.probs, &y2, TYPE_Y2, 0, usize::from(self.left_nz.y2) + usize::from(up.y2));
        for b in 0..16 {
            let (r, c) = (b / 4, b % 4);
            let mut d: [i16; 16] = std::array::from_fn(|i| dequant(y[b][i], self.q.y1[1]));
            d[0] = dcs[b];
            let res = idct(&d);
            for i in 0..16 {
                let (rr, cc) = (r * 4 + i / 4, c * 4 + i % 4);
                let v = clamp255(i32::from(pred[rr * 16 + cc]) + i32::from(res[i]));
                recon[rr * 16 + cc] = v;
                let diff = i64::from(v) - i64::from(self.src_y.get(x0 + cc, y0 + rr));
                sse += (diff * diff) as u64;
            }
            bits += block_cost(&self.probs, &y[b], TYPE_Y_AFTER_Y2, 1, usize::from(ly[r]) + usize::from(ay[c]));
            let nz = any_coded(&y[b], 1);
            ly[r] = nz;
            ay[c] = nz;
        }
        Luma { mode, bmodes: [implied_bmode(mode); 16], y2, y, recon, sse, bits }
    }

    /// B_PRED: each subblock with its best mode, in order, each predicted from the ones before.
    /// Gives up (None) once it costs more than `limit`.
    fn sub_luma(&self, mbx: usize, mby: usize, e: &Edges<16>, right: &[u8; 4], limit: f64) -> Option<Luma> {
        let (x0, y0) = (mbx * 16, mby * 16);
        let mut wr = [0u8; 256];
        let mut bm = [0u8; 16];
        let mut y = [[0i16; 16]; 16];
        let (mut ly, mut ay) = (self.left_nz.y, self.above_nz.get(mbx).copied().unwrap_or_default().y);
        let above_modes = self.above_bmodes.get(mbx).copied().unwrap_or([B_DC_PRED; 4]);
        let mut bits = tree_cost(&KF_YMODE_TREE, &KF_YMODE_PROB, B_PRED);
        let mut sse = 0u64;
        for s in 0..16 {
            let (r, c) = (s / 4, s % 4);
            let a: [u8; 8] = std::array::from_fn(|i| {
                if i < 4 {
                    if r == 0 { e.above[c * 4 + i] } else { wr[(r * 4 - 1) * 16 + c * 4 + i] }
                } else if c == 3 {
                    right[i - 4]
                } else if r == 0 {
                    e.above[c * 4 + i]
                } else {
                    wr[(r * 4 - 1) * 16 + c * 4 + i]
                }
            });
            let l: [u8; 4] = std::array::from_fn(|i| if c == 0 { e.left[r * 4 + i] } else { wr[(r * 4 + i) * 16 + c * 4 - 1] });
            let p = match (r, c) {
                (0, 0) => e.corner,
                (0, _) => e.above[c * 4 - 1],
                (_, 0) => e.left[r * 4 - 1],
                _ => wr[(r * 4 - 1) * 16 + c * 4 - 1],
            };
            let am = if r == 0 { above_modes[c] } else { bm[s - 4] };
            let lm = if c == 0 { self.left_bmodes[r] } else { bm[s - 1] };
            let mode_probs = &KF_BMODE_PROB[usize::from(am)][usize::from(lm)];
            let ctx = usize::from(ly[r]) + usize::from(ay[c]);
            let mut best: Option<(f64, u8, [i16; 16], [u8; 16], u64, f32)> = None;
            for m in 0..10u8 {
                let pred = predict_sub(m, &a, &l, p);
                let coefs = fdct(&Self::residual(self.src_y, x0 + c * 4, y0 + r * 4, &pred, 4));
                let lv: [i16; 16] = std::array::from_fn(|i| quantize(coefs[i], self.q.y1[usize::from(i > 0)], i == 0));
                let d: [i16; 16] = std::array::from_fn(|i| dequant(lv[i], self.q.y1[usize::from(i > 0)]));
                let res = idct(&d);
                let mut out = [0u8; 16];
                let mut e2 = 0u64;
                for i in 0..16 {
                    out[i] = clamp255(i32::from(pred[i]) + i32::from(res[i]));
                    let diff = i64::from(out[i]) - i64::from(self.src_y.get(x0 + c * 4 + i % 4, y0 + r * 4 + i / 4));
                    e2 += (diff * diff) as u64;
                }
                let b = tree_cost(&BMODE_TREE, mode_probs, m) + block_cost(&self.probs, &lv, TYPE_Y_WITH_DC, 0, ctx);
                let j = self.cost_of(e2, b);
                if best.as_ref().is_none_or(|bst| j < bst.0) {
                    best = Some((j, m, lv, out, e2, b));
                }
            }
            let (_, m, lv, out, e2, b) = best?;
            for i in 0..16 {
                wr[(r * 4 + i / 4) * 16 + c * 4 + i % 4] = out[i];
            }
            bm[s] = m;
            y[s] = lv;
            sse += e2;
            bits += b;
            let nz = any_coded(&lv, 0);
            ly[r] = nz;
            ay[c] = nz;
            if self.cost_of(sse, bits) > limit {
                return None;
            }
        }
        Some(Luma { mode: B_PRED, bmodes: bm, y2: [0; 16], y, recon: wr, sse, bits })
    }

    /// Code the macroblock's U and V with prediction `mode`: their levels, reconstruction and cost.
    fn chroma(&self, mbx: usize, mby: usize, mode: u8) -> Chroma {
        let (x0, y0) = (mbx * 8, mby * 8);
        let up = self.above_nz.get(mbx).copied().unwrap_or_default();
        let mut bits = tree_cost(&UV_MODE_TREE, &KF_UV_MODE_PROB, mode);
        let mut sse = 0u64;
        let mut plane = |src: &Plane, rec: &Plane, lnz: [bool; 2], anz: [bool; 2]| -> ([[i16; 16]; 4], [u8; 64]) {
            let e = Self::chroma_edges(rec, mbx, mby);
            let mut pred = [0u8; 64];
            predict_block(mode, &e, &mut pred);
            let mut levels = [[0i16; 16]; 4];
            let mut recon = [0u8; 64];
            let (mut l, mut a) = (lnz, anz);
            for b in 0..4 {
                let (r, c) = (b / 2, b % 2);
                let coefs = fdct(&Self::residual(src, x0 + c * 4, y0 + r * 4, &pred[r * 32 + c * 4..], 8));
                let lv: [i16; 16] = std::array::from_fn(|i| quantize(coefs[i], self.q.uv[usize::from(i > 0)], i == 0));
                let d: [i16; 16] = std::array::from_fn(|i| dequant(lv[i], self.q.uv[usize::from(i > 0)]));
                let res = idct(&d);
                for i in 0..16 {
                    let (rr, cc) = (r * 4 + i / 4, c * 4 + i % 4);
                    let v = clamp255(i32::from(pred[rr * 8 + cc]) + i32::from(res[i]));
                    recon[rr * 8 + cc] = v;
                    let diff = i64::from(v) - i64::from(src.get(x0 + cc, y0 + rr));
                    sse += (diff * diff) as u64;
                }
                bits += block_cost(&self.probs, &lv, TYPE_UV, 0, usize::from(l[r]) + usize::from(a[c]));
                let nz = any_coded(&lv, 0);
                l[r] = nz;
                a[c] = nz;
                levels[b] = lv;
            }
            (levels, recon)
        };
        let (u, recon_u) = plane(self.src_u, &self.rec_u, self.left_nz.u, up.u);
        let (v, recon_v) = plane(self.src_v, &self.rec_v, self.left_nz.v, up.v);
        Chroma { mode, u, v, recon_u, recon_v, sse, bits }
    }

    /// Chooses, codes and reconstructs macroblock (mbx, mby).
    fn macroblock(&mut self, mbx: usize, mby: usize) -> Mb {
        let (e, right) = self.luma_edges(mbx, mby);
        let mut luma: Option<Luma> = None;
        for mode in [DC_PRED, V_PRED, H_PRED, TM_PRED] {
            let cand = self.whole_luma(mbx, mby, mode, &e);
            if luma.as_ref().is_none_or(|l| self.cost_of(cand.sse, cand.bits) < self.cost_of(l.sse, l.bits)) {
                luma = Some(cand);
            }
        }
        let limit = luma.as_ref().map_or(f64::INFINITY, |l| self.cost_of(l.sse, l.bits));
        if let Some(sub) = self.sub_luma(mbx, mby, &e, &right, limit) {
            luma = Some(sub);
        }
        let mut chroma: Option<Chroma> = None;
        for mode in [DC_PRED, V_PRED, H_PRED, TM_PRED] {
            let cand = self.chroma(mbx, mby, mode);
            if chroma.as_ref().is_none_or(|c| self.cost_of(cand.sse, cand.bits) < self.cost_of(c.sse, c.bits)) {
                chroma = Some(cand);
            }
        }
        let luma = luma.unwrap_or_else(|| self.whole_luma(mbx, mby, DC_PRED, &e));
        let chroma = chroma.unwrap_or_else(|| self.chroma(mbx, mby, DC_PRED));

        // The reconstruction later macroblocks are predicted from.
        for i in 0..256 {
            self.rec_y.set(mbx * 16 + i % 16, mby * 16 + i / 16, luma.recon[i]);
        }
        for i in 0..64 {
            self.rec_u.set(mbx * 8 + i % 8, mby * 8 + i / 8, chroma.recon_u[i]);
            self.rec_v.set(mbx * 8 + i % 8, mby * 8 + i / 8, chroma.recon_v[i]);
        }

        // Contexts, as the token pass will keep them.
        let has_y2 = luma.mode != B_PRED;
        let first = usize::from(has_y2);
        let up = self.above_nz.get(mbx).copied().unwrap_or_default();
        let mut nz = Nz { y2: if has_y2 { any_coded(&luma.y2, 0) } else { up.y2 }, ..Nz::default() };
        let mut left = self.left_nz;
        if has_y2 {
            left.y2 = nz.y2;
        }
        for b in 0..16 {
            let coded = any_coded(&luma.y[b], first);
            left.y[b / 4] = coded;
            nz.y[b % 4] = coded;
        }
        for b in 0..4 {
            let cu = any_coded(&chroma.u[b], 0);
            let cv = any_coded(&chroma.v[b], 0);
            left.u[b / 2] = cu;
            nz.u[b % 2] = cu;
            left.v[b / 2] = cv;
            nz.v[b % 2] = cv;
        }
        if !has_y2 {
            left.y2 = self.left_nz.y2;
        }
        self.left_nz = left;
        if let Some(a) = self.above_nz.get_mut(mbx) {
            *a = nz;
        }
        if let Some(a) = self.above_bmodes.get_mut(mbx) {
            *a = [luma.bmodes[12], luma.bmodes[13], luma.bmodes[14], luma.bmodes[15]];
        }
        self.left_bmodes = [luma.bmodes[3], luma.bmodes[7], luma.bmodes[11], luma.bmodes[15]];

        let skip = (!has_y2 || !any_coded(&luma.y2, 0))
            && luma.y.iter().all(|b| !any_coded(b, first))
            && chroma.u.iter().chain(chroma.v.iter()).all(|b| !any_coded(b, 0));
        Mb { ymode: luma.mode, bmodes: luma.bmodes, uvmode: chroma.mode, skip, y2: luma.y2, y: luma.y, u: chroma.u, v: chroma.v }
    }
}

/// Encodes planes already at the coded size (whole macroblocks).
pub(crate) fn encode(y: &Plane, u: &Plane, v: &Plane, width: u32, height: u32, params: &Vp8Params) -> Result<Vp8Frame, Vp8Error> {
    let (mbw, mbh) = (y.width / 16, y.height / 16);
    let q = q_index(params.quality);
    let quant = Quant::new(q);
    let lf = params.loop_filter.unwrap_or_else(|| ((f64::from(q) * 0.4).round() as u8).min(63)).min(63);
    let mut enc = Encoder {
        src_y: y,
        src_u: u,
        src_v: v,
        rec_y: Plane::new(y.width, y.height),
        rec_u: Plane::new(u.width, u.height),
        rec_v: Plane::new(v.width, v.height),
        mbw,
        q: quant,
        lambda: LAMBDA_PER_STEP2 * f64::from(quant.y1[1]).powi(2),
        probs: DEFAULT_COEFF_PROBS,
        above_nz: vec![Nz::default(); mbw],
        left_nz: Nz::default(),
        above_bmodes: vec![[B_DC_PRED; 4]; mbw],
        left_bmodes: [B_DC_PRED; 4],
    };
    let mut mbs = Vec::with_capacity(mbw * mbh);
    for mby in 0..mbh {
        enc.left_nz = Nz::default();
        enc.left_bmodes = [B_DC_PRED; 4];
        for mbx in 0..mbw {
            mbs.push(enc.macroblock(mbx, mby));
        }
    }

    // Token counts → probability updates.
    let mut counts = CountSink(Box::new([[[[[0; 2]; 11]; 3]; 8]; 4]));
    tokens(&mut counts, &DEFAULT_COEFF_PROBS, &mbs, mbw);
    let probs = updated_probs(&counts);

    // First partition: the frame header (Annex A §19.2) and the macroblock headers (§19.3).
    let skipped = mbs.iter().filter(|m| m.skip).count();
    let use_skip = skipped > 0;
    let prob_skip = (((mbs.len() - skipped) * 256 + mbs.len() / 2) / mbs.len().max(1)).clamp(1, 255) as u8;
    let mut w = BoolWriter::new();
    w.literal(1, 0); // color_space: BT.601
    w.literal(1, 0); // clamping_type: the decoder clamps
    w.literal(1, 0); // segmentation_enabled
    w.literal(1, 0); // filter_type: normal
    w.literal(6, u32::from(lf));
    w.literal(3, 0); // sharpness_level
    w.literal(1, 0); // loop_filter_adj_enable
    w.literal(2, 0); // log2_nbr_of_dct_partitions: one
    w.literal(7, u32::from(q));
    for _ in 0..5 {
        w.literal(1, 0); // no quantizer index deltas
    }
    w.literal(1, 0); // refresh_entropy_probs
    for t in 0..4 {
        for b in 0..8 {
            for c in 0..3 {
                for n in 0..11 {
                    let new = probs[t][b][c][n];
                    let changed = new != DEFAULT_COEFF_PROBS[t][b][c][n];
                    w.put(COEFF_UPDATE_PROBS[t][b][c][n], changed);
                    if changed {
                        w.literal(8, u32::from(new));
                    }
                }
            }
        }
    }
    w.literal(1, u32::from(use_skip));
    if use_skip {
        w.literal(8, u32::from(prob_skip));
    }
    let mut above_modes = vec![[B_DC_PRED; 4]; mbw];
    let mut left_modes = [B_DC_PRED; 4];
    for (i, mb) in mbs.iter().enumerate() {
        let x = i % mbw;
        if x == 0 {
            left_modes = [B_DC_PRED; 4];
        }
        if use_skip {
            w.put(prob_skip, mb.skip);
        }
        put_tree(&mut w, &KF_YMODE_TREE, &KF_YMODE_PROB, mb.ymode);
        let up = above_modes.get(x).copied().unwrap_or([B_DC_PRED; 4]);
        if mb.ymode == B_PRED {
            for s in 0..16 {
                let (r, c) = (s / 4, s % 4);
                let am = if r == 0 { up[c] } else { mb.bmodes[s - 4] };
                let lm = if c == 0 { left_modes[r] } else { mb.bmodes[s - 1] };
                put_tree(&mut w, &BMODE_TREE, &KF_BMODE_PROB[usize::from(am)][usize::from(lm)], mb.bmodes[s]);
            }
        }
        put_tree(&mut w, &UV_MODE_TREE, &KF_UV_MODE_PROB, mb.uvmode);
        if let Some(a) = above_modes.get_mut(x) {
            *a = [mb.bmodes[12], mb.bmodes[13], mb.bmodes[14], mb.bmodes[15]];
        }
        left_modes = [mb.bmodes[3], mb.bmodes[7], mb.bmodes[11], mb.bmodes[15]];
    }
    let first = w.finish();
    if first.len() >= 1 << 19 {
        return Err(Vp8Error::PartitionTooLarge(first.len()));
    }

    let mut t = BoolWriter::new();
    tokens(&mut t, &probs, &mbs, mbw);
    let second = t.finish();

    // Frame tag (key frame, version 0, shown) and the key frame start code and size (§9.1).
    let tag = (1u32 << 4) | ((first.len() as u32) << 5);
    let mut data = Vec::with_capacity(10 + first.len() + second.len());
    data.extend_from_slice(&tag.to_le_bytes()[..3]);
    data.extend_from_slice(&[0x9d, 0x01, 0x2a]);
    data.extend_from_slice(&((width & 0x3fff) as u16).to_le_bytes());
    data.extend_from_slice(&((height & 0x3fff) as u16).to_le_bytes());
    data.extend_from_slice(&first);
    data.extend_from_slice(&second);
    Ok(Vp8Frame { data, y: enc.rec_y, u: enc.rec_u, v: enc.rec_v, q_index: q, loop_filter: lf })
}

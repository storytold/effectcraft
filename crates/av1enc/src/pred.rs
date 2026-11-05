//! Prediction processes, mirrored from the spec (7.11.2 intra prediction for the non-directional
//! modes plus V_PRED / H_PRED without edge filtering, 7.11.3 block inter prediction without
//! reference scaling) so that the encoder predicts exactly like a decoder.

use crate::tables::{
    DC_PRED, EIGHTTAP, H_PRED, PAETH_PRED, SM_WEIGHTS_TX_4X4, SM_WEIGHTS_TX_8X8, SM_WEIGHTS_TX_16X16, SM_WEIGHTS_TX_32X32, SMOOTH_H_PRED, SMOOTH_PRED,
    SMOOTH_V_PRED, SUBPEL_FILTERS, V_PRED,
};
use crate::transform::round2;

/// A plane buffer (padded to whole superblocks).
#[derive(Clone)]
pub(crate) struct Plane {
    pub data: Vec<u16>,
    pub stride: usize,
}

impl Plane {
    pub fn new(w: usize, h: usize) -> Plane {
        Plane { data: vec![0; w * h], stride: w }
    }
    #[inline(always)]
    pub fn at(&self, x: usize, y: usize) -> u16 {
        self.data[y * self.stride + x]
    }
    #[inline(always)]
    pub fn row(&self, y: usize) -> &[u16] {
        &self.data[y * self.stride..(y + 1) * self.stride]
    }
}

/// The intra modes this encoder uses (values are the spec's y / uv mode numbers).
pub(crate) const INTRA_MODES_USED: [usize; 7] = [DC_PRED, V_PRED, H_PRED, PAETH_PRED, SMOOTH_PRED, SMOOTH_V_PRED, SMOOTH_H_PRED];

/// Inputs of the intra prediction process.
pub(crate) struct IntraEdge {
    pub x: usize,
    pub y: usize,
    pub have_left: bool,
    pub have_above: bool,
    pub have_above_right: bool,
    pub have_below_left: bool,
    pub log2: u32,
    /// ((MiCols * MI_SIZE) >> subX) - 1 and the same for rows.
    pub max_x: i32,
    pub max_y: i32,
    pub bit_depth: u32,
}

/// Edge samples: `above[0..2n]`, `left[0..2n]` and the top-left corner.
pub(crate) struct Edges {
    above: [i32; 64],
    left: [i32; 64],
    corner: i32,
}

pub(crate) fn edges(pl: &Plane, e: &IntraEdge) -> Edges {
    let w = 1usize << e.log2;
    let h = w;
    let bd = e.bit_depth;
    let (x, y) = (e.x, e.y);
    let n = w + h;
    let mut above = [0i32; 64];
    let mut left = [0i32; 64];
    if !e.have_above && e.have_left {
        let v = pl.at(x - 1, y) as i32;
        above[..n].iter_mut().for_each(|a| *a = v);
    } else if !e.have_above && !e.have_left {
        let v = (1 << (bd - 1)) - 1;
        above[..n].iter_mut().for_each(|a| *a = v);
    } else {
        let above_limit = e.max_x.min(x as i32 + if e.have_above_right { 2 * w as i32 } else { w as i32 } - 1);
        let row = pl.row(y - 1);
        for (i, a) in above[..n].iter_mut().enumerate() {
            *a = row[above_limit.min(x as i32 + i as i32) as usize] as i32;
        }
    }
    if !e.have_left && e.have_above {
        let v = pl.at(x, y - 1) as i32;
        left[..n].iter_mut().for_each(|a| *a = v);
    } else if !e.have_left && !e.have_above {
        let v = (1 << (bd - 1)) + 1;
        left[..n].iter_mut().for_each(|a| *a = v);
    } else {
        let left_limit = e.max_y.min(y as i32 + if e.have_below_left { 2 * h as i32 } else { h as i32 } - 1);
        for (i, l) in left[..n].iter_mut().enumerate() {
            *l = pl.at(x - 1, left_limit.min(y as i32 + i as i32) as usize) as i32;
        }
    }
    let corner = if e.have_above && e.have_left {
        pl.at(x - 1, y - 1) as i32
    } else if e.have_above {
        pl.at(x, y - 1) as i32
    } else if e.have_left {
        pl.at(x - 1, y) as i32
    } else {
        1 << (bd - 1)
    };
    Edges { above, left, corner }
}

fn sm_weights(log2: u32) -> &'static [u8] {
    match log2 {
        2 => &SM_WEIGHTS_TX_4X4,
        3 => &SM_WEIGHTS_TX_8X8,
        4 => &SM_WEIGHTS_TX_16X16,
        _ => &SM_WEIGHTS_TX_32X32,
    }
}

/// Predicts a square `2^log2` block with `mode` into `pred` (row stride = block width).
pub(crate) fn predict_intra(ed: &Edges, e: &IntraEdge, mode: usize, pred: &mut [u16]) {
    let log2 = e.log2;
    let w = 1usize << log2;
    let h = w;
    let bd = e.bit_depth;
    let (above, left) = (&ed.above, &ed.left);
    match mode {
        DC_PRED => {
            let v = if e.have_left && e.have_above {
                let sum: i32 = left[..h].iter().sum::<i32>() + above[..w].iter().sum::<i32>() + ((w + h) >> 1) as i32;
                sum / (w + h) as i32
            } else if e.have_left {
                (left[..h].iter().sum::<i32>() + (h >> 1) as i32) >> log2
            } else if e.have_above {
                (above[..w].iter().sum::<i32>() + (w >> 1) as i32) >> log2
            } else {
                1 << (bd - 1)
            };
            let v = v.clamp(0, (1 << bd) - 1) as u16;
            pred[..w * h].iter_mut().for_each(|p| *p = v);
        }
        V_PRED => {
            for i in 0..h {
                for j in 0..w {
                    pred[i * w + j] = above[j] as u16;
                }
            }
        }
        H_PRED => {
            for i in 0..h {
                pred[i * w..i * w + w].iter_mut().for_each(|p| *p = left[i] as u16);
            }
        }
        PAETH_PRED => {
            let tl = ed.corner;
            for i in 0..h {
                for j in 0..w {
                    let a = above[j];
                    let l = left[i];
                    let base = a + l - tl;
                    let p_left = (base - l).abs();
                    let p_top = (base - a).abs();
                    let p_tl = (base - tl).abs();
                    pred[i * w + j] = if p_left <= p_top && p_left <= p_tl {
                        l as u16
                    } else if p_top <= p_tl {
                        a as u16
                    } else {
                        tl as u16
                    };
                }
            }
        }
        _ => {
            let wx = sm_weights(log2);
            let wy = wx;
            for i in 0..h {
                for j in 0..w {
                    let v = if mode == SMOOTH_PRED {
                        let s = wy[i] as i32 * above[j] + (256 - wy[i] as i32) * left[h - 1] + wx[j] as i32 * left[i] + (256 - wx[j] as i32) * above[w - 1];
                        round2(s, 9)
                    } else if mode == SMOOTH_V_PRED {
                        round2(wy[i] as i32 * above[j] + (256 - wy[i] as i32) * left[h - 1], 8)
                    } else {
                        round2(wx[j] as i32 * left[i] + (256 - wx[j] as i32) * above[w - 1], 8)
                    };
                    pred[i * w + j] = v as u16;
                }
            }
        }
    }
}

/// Block inter prediction (7.11.3.3 motion vector scaling without scaling, 7.11.3.4 block
/// inter prediction with the EIGHTTAP filter, single reference so InterRound1 = 11 and the
/// final rounding is the identity) of the `w x h` block at plane position (x, y) displaced by
/// `mv` (1/8 luma sample units, [row, col]). `last_x` / `last_y` are the largest sample
/// coordinates of the reference plane; the result goes to `pred` (row stride `w`).
pub(crate) struct InterBlock {
    pub last_x: i32,
    pub last_y: i32,
    pub x: usize,
    pub y: usize,
    pub width: usize,
    pub height: usize,
    pub mv: [i32; 2],
    pub subsampling: usize,
    pub bit_depth: u32,
}

pub(crate) fn predict_inter(refp: &Plane, block: &InterBlock, pred: &mut [u16]) {
    let InterBlock { last_x, last_y, x, y, width: w, height: h, mv, subsampling: ss, bit_depth } = *block;
    // positions in 1/16 sample units ((startX >> 6) & 15 is the filter phase)
    let px = ((x as i32) << 4) + ((2 * mv[1]) >> ss);
    let py = ((y as i32) << 4) + ((2 * mv[0]) >> ss);
    let (fx, fy) = ((px & 15) as usize, (py & 15) as usize);
    let (ix, iy) = (px >> 4, py >> 4);
    let max = (1i32 << bit_depth) - 1;
    if fx == 0 && fy == 0 {
        // the filters' phase-0 kernel is a unit impulse: a clamped copy
        for r in 0..h {
            let ry = (iy + r as i32).clamp(0, last_y) as usize;
            let row = refp.row(ry);
            let out = &mut pred[r * w..r * w + w];
            let x0 = ix;
            if x0 >= 0 && x0 + w as i32 - 1 <= last_x {
                out.copy_from_slice(&row[x0 as usize..x0 as usize + w]);
            } else {
                for (c, o) in out.iter_mut().enumerate() {
                    *o = row[(x0 + c as i32).clamp(0, last_x) as usize];
                }
            }
        }
        return;
    }
    let filt_h = if w <= 4 { 4 } else { EIGHTTAP };
    let filt_v = if h <= 4 { 4 } else { EIGHTTAP };
    let kh = &SUBPEL_FILTERS[filt_h][fx];
    let kv = &SUBPEL_FILTERS[filt_v][fy];
    let bx = ix - 3;
    let x_inside = bx >= 0 && bx + w as i32 + 7 <= last_x + 1;
    // horizontal pass of one reference row into `out` (Round2( sum, InterRound0 = 3 ))
    let hfilter = |ry: usize, out: &mut [i32]| {
        let row = refp.row(ry);
        if x_inside {
            let seg = &row[bx as usize..bx as usize + w + 7];
            for (c, o) in out.iter_mut().enumerate() {
                let mut s = 0i32;
                for t in 0..8 {
                    s += kh[t] as i32 * seg[c + t] as i32;
                }
                *o = round2(s, 3);
            }
        } else {
            for (c, o) in out.iter_mut().enumerate() {
                let mut s = 0i32;
                for t in 0..8 {
                    s += kh[t] as i32 * row[(bx + c as i32 + t as i32).clamp(0, last_x) as usize] as i32;
                }
                *o = round2(s, 3);
            }
        }
    };
    let mut inter = [0i32; (64 + 7) * 64];
    if fy == 0 {
        // the vertical kernel is 128 at the centre: Round2( 128 * v, 11 ) = Round2( v, 4 )
        for r in 0..h {
            let ry = (iy + r as i32).clamp(0, last_y) as usize;
            hfilter(ry, &mut inter[..w]);
            for (p, &v) in pred[r * w..r * w + w].iter_mut().zip(&inter[..w]) {
                *p = round2(v, 4).clamp(0, max) as u16;
            }
        }
        return;
    }
    if fx == 0 {
        // the horizontal pass is exactly 16 * sample: Round2( 16 * sum, 11 ) = Round2( sum, 7 )
        for r in 0..h {
            let rows: [&[u16]; 8] = std::array::from_fn(|t| refp.row((iy + r as i32 + t as i32 - 3).clamp(0, last_y) as usize));
            for c in 0..w {
                let xx = if x_inside { (ix + c as i32) as usize } else { (ix + c as i32).clamp(0, last_x) as usize };
                let mut s = 0i32;
                for t in 0..8 {
                    s += kv[t] as i32 * rows[t][xx] as i32;
                }
                pred[r * w + c] = round2(s, 7).clamp(0, max) as u16;
            }
        }
        return;
    }
    let inter_h = h + 7;
    for r in 0..inter_h {
        let ry = (iy + r as i32 - 3).clamp(0, last_y) as usize;
        hfilter(ry, &mut inter[r * w..r * w + w]);
    }
    for r in 0..h {
        for c in 0..w {
            let mut s = 0i32;
            for t in 0..8 {
                s += kv[t] as i32 * inter[(r + t) * w + c];
            }
            pred[r * w + c] = round2(s, 11).clamp(0, max) as u16;
        }
    }
}

/// The visible `w x h` area of a reference plane interpolated at the sub-sample phase
/// (`fx`, `fy`) (1/16 units) with the EIGHTTAP filter, as `predict_inter` computes it for
/// blocks wider and taller than 4. The reference for `half_planes`, which the encoder uses.
#[cfg(test)]
pub(crate) fn subpel_plane(refp: &Plane, w: usize, h: usize, fx: usize, fy: usize, bit_depth: u32) -> Vec<u16> {
    let kh = &SUBPEL_FILTERS[EIGHTTAP][fx];
    let kv = &SUBPEL_FILTERS[EIGHTTAP][fy];
    let max = (1i32 << bit_depth) - 1;
    let (lx, ly) = (w as i32 - 1, h as i32 - 1);
    let mut inter = vec![0i32; w * h];
    for y in 0..h {
        let row = refp.row(y);
        for x in 0..w {
            let mut s = 0i32;
            for t in 0..8 {
                s += kh[t] as i32 * row[(x as i32 + t as i32 - 3).clamp(0, lx) as usize] as i32;
            }
            inter[y * w + x] = round2(s, 3);
        }
    }
    let mut out = vec![0u16; w * h];
    for y in 0..h {
        for x in 0..w {
            let mut s = 0i32;
            for t in 0..8 {
                s += kv[t] as i32 * inter[(y as i32 + t as i32 - 3).clamp(0, ly) as usize * w + x];
            }
            out[y * w + x] = round2(s, 11).clamp(0, max) as u16;
        }
    }
    out
}

/// The three half-sample planes (8, 0), (0, 8), (8, 8) of `subpel_plane` in one pass: the
/// horizontal half-sample pass is shared by (8, 0) and (8, 8), and the phase-0 kernels reduce to
/// exact shifts (Round2( 128 * v, 11 ) = Round2( v, 4 ), Round2( 16 * s, 11 ) = Round2( s, 7 )),
/// so the result equals three `subpel_plane` calls bit for bit.
pub(crate) fn half_planes(refp: &Plane, w: usize, h: usize, bit_depth: u32) -> [Vec<u16>; 3] {
    let k = &SUBPEL_FILTERS[EIGHTTAP][8];
    let max = (1i32 << bit_depth) - 1;
    let (lx, ly) = (w as i32 - 1, h as i32 - 1);
    // horizontal pass at phase 8: Round2( sum, 3 )
    let mut hp = vec![0i32; w * h];
    for y in 0..h {
        let row = &refp.row(y)[..w];
        let out = &mut hp[y * w..y * w + w];
        let tap = |x: i32| -> i32 {
            let mut s = 0i32;
            for t in 0..8 {
                s += k[t] as i32 * row[(x + t as i32 - 3).clamp(0, lx) as usize] as i32;
            }
            round2(s, 3)
        };
        if w >= 8 {
            for x in 0..3 {
                out[x] = tap(x as i32);
            }
            for (x, o) in out.iter_mut().enumerate().take(w - 4).skip(3) {
                let seg = &row[x - 3..x + 5];
                let mut s = 0i32;
                for t in 0..8 {
                    s += k[t] as i32 * seg[t] as i32;
                }
                *o = round2(s, 3);
            }
            for x in w - 4..w {
                out[x] = tap(x as i32);
            }
        } else {
            for (x, o) in out.iter_mut().enumerate() {
                *o = tap(x as i32);
            }
        }
    }
    let p80: Vec<u16> = hp.iter().map(|&v| round2(v, 4).clamp(0, max) as u16).collect();
    let mut p08 = vec![0u16; w * h];
    let mut p88 = vec![0u16; w * h];
    for y in 0..h {
        let rows: [usize; 8] = std::array::from_fn(|t| (y as i32 + t as i32 - 3).clamp(0, ly) as usize);
        let (o08, o88) = (&mut p08[y * w..y * w + w], &mut p88[y * w..y * w + w]);
        for x in 0..w {
            let (mut a, mut b) = (0i32, 0i32);
            for t in 0..8 {
                a += k[t] as i32 * refp.data[rows[t] * refp.stride + x] as i32;
                b += k[t] as i32 * hp[rows[t] * w + x];
            }
            o08[x] = round2(a, 7).clamp(0, max) as u16;
            o88[x] = round2(b, 11).clamp(0, max) as u16;
        }
    }
    [p80, p08, p88]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Whole-plane interpolation equals block inter prediction at the same phase.
    #[test]
    fn subpel_plane_matches_block_prediction() {
        let (w, h) = (37, 29);
        let mut p = Plane::new(64, 64);
        for (i, v) in p.data.iter_mut().enumerate() {
            *v = ((i * 7919) % 256) as u16;
        }
        for (fx, fy) in [(8, 0), (0, 8), (8, 8), (4, 12)] {
            let plane = subpel_plane(&p, w, h, fx, fy, 8);
            for (bx, by) in [(0i32, 0i32), (8, 8), (24, 16), (-5, 20), (30, -3)] {
                let mut pred = vec![0u16; 64];
                let mv = [by * 8 + fy as i32 / 2, bx * 8 + fx as i32 / 2];
                predict_inter(
                    &p,
                    &InterBlock { last_x: w as i32 - 1, last_y: h as i32 - 1, x: 0, y: 0, width: 8, height: 8, mv, subsampling: 0, bit_depth: 8 },
                    &mut pred,
                );
                for r in 0..8 {
                    for c in 0..8 {
                        let (x, y) = ((bx + c).clamp(0, w as i32 - 1), (by + r).clamp(0, h as i32 - 1));
                        if bx + c == x && by + r == y {
                            assert_eq!(pred[(r * 8 + c) as usize], plane[y as usize * w + x as usize], "phase ({fx}, {fy}) at ({}, {})", bx + c, by + r);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn half_planes_match_subpel_plane() {
        for (w, h) in [(37usize, 29usize), (5, 3), (1, 1), (64, 64)] {
            let mut p = Plane::new(64, 64);
            for (i, v) in p.data.iter_mut().enumerate() {
                *v = ((i * 7919) % 1024) as u16;
            }
            let hp = half_planes(&p, w, h, 10);
            for (i, (fx, fy)) in [(8, 0), (0, 8), (8, 8)].into_iter().enumerate() {
                assert_eq!(hp[i], subpel_plane(&p, w, h, fx, fy, 10), "{w}x{h} phase ({fx}, {fy})");
            }
        }
    }

    #[test]
    fn phase_zero_filters_are_impulses() {
        for f in SUBPEL_FILTERS.iter() {
            assert_eq!(f[0], [0, 0, 0, 128, 0, 0, 0, 0]);
        }
    }
}

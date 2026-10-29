//! Deblocking filter (8.7.2) over a whole reconstructed picture, mirrored from the filmcraft-hevc
//! decoder: all vertical edges on the 8x8 grid first, then all horizontal edges. One slice, one QP,
//! uni-directional prediction from a single reference picture, offsets 0.

use crate::frame::{Blk, E_EDGE_H, E_EDGE_V, F_CBF, F_INTRA};
use crate::tables::{BETA_TABLE, TC_TABLE, qpc_420};

#[inline]
fn tc_beta(q: i32, bs: i32, bd: u32) -> (i32, i32) {
    let qb = q.clamp(0, 51);
    let qt = (q + 2 * (bs - 1)).clamp(0, 53);
    ((TC_TABLE[qt as usize] as i32) << (bd - 8), (BETA_TABLE[qb as usize] as i32) << (bd - 8))
}

/// Boundary strength (8.7.2.4) between 4x4 blocks p and q on a transform / prediction edge.
fn boundary_strength(p: &Blk, q: &Blk) -> i32 {
    if (p.flags | q.flags) & F_INTRA != 0 {
        return 2;
    }
    if (p.flags | q.flags) & F_CBF != 0 {
        return 1;
    }
    let far = (p.mv[0] as i32 - q.mv[0] as i32).abs() >= 4 || (p.mv[1] as i32 - q.mv[1] as i32).abs() >= 4;
    far as i32
}

/// Deblock the planes in place. `qp` is the slice QpY (without QpBdOffset).
pub fn deblock(planes: &mut [Vec<u16>; 3], blk: &[Blk], w: usize, h: usize, qp: i32, bd: u32) {
    let (w4, h4) = (w / 4, h / 4);
    let cw = w / 2;
    // chroma tc (bS 2 only; QpC from the averaged luma QP, no offsets)
    let qt = (qpc_420(qp) + 2).clamp(0, 53);
    let tc_c = (TC_TABLE[qt as usize] as i32) << (bd - 8);
    // vertical edges
    for by in 0..h4 {
        for bx in (2..w4).step_by(2) {
            let qi = by * w4 + bx;
            if blk[qi].flags & E_EDGE_V == 0 {
                continue;
            }
            let bs = boundary_strength(&blk[qi - 1], &blk[qi]);
            if bs == 0 {
                continue;
            }
            let (x, y) = (bx * 4, by * 4);
            let (tc, beta) = tc_beta(qp, bs, bd);
            filter_luma(&mut planes[0], y * w + x, 1, w, tc, beta, bd);
            // chroma: 8x8 chroma grid (16 luma samples), one 4-sample chroma segment per 8 luma rows
            if bs == 2 && bx % 4 == 0 && by % 2 == 0 {
                for c in 1..3 {
                    filter_chroma(&mut planes[c], (y / 2) * cw + x / 2, 1, cw, tc_c, bd);
                }
            }
        }
    }
    // horizontal edges
    for by in (2..h4).step_by(2) {
        for bx in 0..w4 {
            let qi = by * w4 + bx;
            if blk[qi].flags & E_EDGE_H == 0 {
                continue;
            }
            let bs = boundary_strength(&blk[qi - w4], &blk[qi]);
            if bs == 0 {
                continue;
            }
            let (x, y) = (bx * 4, by * 4);
            let (tc, beta) = tc_beta(qp, bs, bd);
            filter_luma(&mut planes[0], y * w + x, w, 1, tc, beta, bd);
            if bs == 2 && by % 4 == 0 && bx % 2 == 0 {
                for c in 1..3 {
                    filter_chroma(&mut planes[c], (y / 2) * cw + x / 2, cw, 1, tc_c, bd);
                }
            }
        }
    }
}

/// Filter one 4-sample luma edge segment. `o` = offset of q0 of the first line, `step` = distance
/// between samples across the edge, `along` = distance between lines.
fn filter_luma(s: &mut [u16], o: usize, step: usize, along: usize, tc: i32, beta: i32, bd: u32) {
    let px = |s: &[u16], k: usize, i: usize| s[o + k * along - (i + 1) * step] as i32;
    let qx = |s: &[u16], k: usize, i: usize| s[o + k * along + i * step] as i32;
    let dp0 = (px(s, 0, 2) - 2 * px(s, 0, 1) + px(s, 0, 0)).abs();
    let dp3 = (px(s, 3, 2) - 2 * px(s, 3, 1) + px(s, 3, 0)).abs();
    let dq0 = (qx(s, 0, 2) - 2 * qx(s, 0, 1) + qx(s, 0, 0)).abs();
    let dq3 = (qx(s, 3, 2) - 2 * qx(s, 3, 1) + qx(s, 3, 0)).abs();
    let (dpq0, dpq3) = (dp0 + dq0, dp3 + dq3);
    let (dp, dq) = (dp0 + dp3, dq0 + dq3);
    let d = dpq0 + dpq3;
    if d >= beta {
        return;
    }
    let dsam = |s: &[u16], k: usize, dpq: i32| -> bool {
        dpq < (beta >> 2)
            && (px(s, k, 3) - px(s, k, 0)).abs() + (qx(s, k, 0) - qx(s, k, 3)).abs() < (beta >> 3)
            && (px(s, k, 0) - qx(s, k, 0)).abs() < ((5 * tc + 1) >> 1)
    };
    let strong = dsam(s, 0, 2 * dpq0) && dsam(s, 3, 2 * dpq3);
    let dep = dp < ((beta + (beta >> 1)) >> 3);
    let deq = dq < ((beta + (beta >> 1)) >> 3);
    let max = (1i32 << bd) - 1;
    for k in 0..4 {
        let base = o + k * along;
        let p = |i: usize| s[base - (i + 1) * step] as i32;
        let q = |i: usize| s[base + i * step] as i32;
        let (p0, p1, p2, p3) = (p(0), p(1), p(2), p(3));
        let (q0, q1, q2, q3) = (q(0), q(1), q(2), q(3));
        if strong {
            let t2 = 2 * tc;
            s[base - step] = ((p2 + 2 * p1 + 2 * p0 + 2 * q0 + q1 + 4) >> 3).clamp(p0 - t2, p0 + t2) as u16;
            s[base - 2 * step] = ((p2 + p1 + p0 + q0 + 2) >> 2).clamp(p1 - t2, p1 + t2) as u16;
            s[base - 3 * step] = ((2 * p3 + 3 * p2 + p1 + p0 + q0 + 4) >> 3).clamp(p2 - t2, p2 + t2) as u16;
            s[base] = ((p1 + 2 * p0 + 2 * q0 + 2 * q1 + q2 + 4) >> 3).clamp(q0 - t2, q0 + t2) as u16;
            s[base + step] = ((p0 + q0 + q1 + q2 + 2) >> 2).clamp(q1 - t2, q1 + t2) as u16;
            s[base + 2 * step] = ((p0 + q0 + q1 + 3 * q2 + 2 * q3 + 4) >> 3).clamp(q2 - t2, q2 + t2) as u16;
        } else {
            let mut delta = (9 * (q0 - p0) - 3 * (q1 - p1) + 8) >> 4;
            if delta.abs() >= tc * 10 {
                continue;
            }
            delta = delta.clamp(-tc, tc);
            s[base - step] = (p0 + delta).clamp(0, max) as u16;
            s[base] = (q0 - delta).clamp(0, max) as u16;
            if dep {
                let dp = ((((p2 + p0 + 1) >> 1) - p1 + delta) >> 1).clamp(-(tc >> 1), tc >> 1);
                s[base - 2 * step] = (p1 + dp).clamp(0, max) as u16;
            }
            if deq {
                let dq = ((((q2 + q0 + 1) >> 1) - q1 - delta) >> 1).clamp(-(tc >> 1), tc >> 1);
                s[base + step] = (q1 + dq).clamp(0, max) as u16;
            }
        }
    }
}

/// Filter one 4-sample chroma edge segment.
fn filter_chroma(s: &mut [u16], o: usize, step: usize, along: usize, tc: i32, bd: u32) {
    let max = (1i32 << bd) - 1;
    for k in 0..4 {
        let base = o + k * along;
        let (p0, p1) = (s[base - step] as i32, s[base - 2 * step] as i32);
        let (q0, q1) = (s[base] as i32, s[base + step] as i32);
        let delta = ((((q0 - p0) << 2) + p1 - q1 + 4) >> 3).clamp(-tc, tc);
        s[base - step] = (p0 + delta).clamp(0, max) as u16;
        s[base] = (q0 - delta).clamp(0, max) as u16;
    }
}

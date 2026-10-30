//! Intra prediction, RFC 6386 §12: the whole-block modes for 16×16 luma and 8×8 chroma, and the
//! ten 4×4 subblock modes.

/// Whole-block modes, numbered as `intra_mbmode` (§11.2).
pub(crate) const DC_PRED: u8 = 0;
pub(crate) const V_PRED: u8 = 1;
pub(crate) const H_PRED: u8 = 2;
pub(crate) const TM_PRED: u8 = 3;
pub(crate) const B_PRED: u8 = 4;

/// Subblock modes, numbered as `intra_bmode` (§11.2).
pub(crate) const B_DC_PRED: u8 = 0;
pub(crate) const B_TM_PRED: u8 = 1;
pub(crate) const B_VE_PRED: u8 = 2;
pub(crate) const B_HE_PRED: u8 = 3;
pub(crate) const B_LD_PRED: u8 = 4;
pub(crate) const B_RD_PRED: u8 = 5;
pub(crate) const B_VR_PRED: u8 = 6;
pub(crate) const B_VL_PRED: u8 = 7;
pub(crate) const B_HD_PRED: u8 = 8;
pub(crate) const B_HU_PRED: u8 = 9;

/// The pixels a block is predicted from, with the frame-edge values of §12 already in place:
/// `above` (and, for 16×16 luma, the four above-right pixels after it), `left`, the corner above
/// and to the left, and whether the real neighbours exist (for DC prediction's averages).
pub(crate) struct Edges<const N: usize> {
    pub above: [u8; N],
    pub left: [u8; N],
    pub corner: u8,
    pub have_above: bool,
    pub have_left: bool,
}

/// Predicts an N×N whole block (N = 16 luma, 8 chroma) in `mode` (DC, V, H or TM).
pub(crate) fn predict_block<const N: usize>(mode: u8, e: &Edges<N>, out: &mut [u8]) {
    match mode {
        V_PRED => {
            for r in 0..N {
                out[r * N..r * N + N].copy_from_slice(&e.above);
            }
        }
        H_PRED => {
            for r in 0..N {
                out[r * N..r * N + N].fill(e.left[r]);
            }
        }
        TM_PRED => {
            for r in 0..N {
                for c in 0..N {
                    out[r * N + c] = clamp(i32::from(e.left[r]) + i32::from(e.above[c]) - i32::from(e.corner));
                }
            }
        }
        _ => {
            let shift = N.trailing_zeros();
            let sum_a: u32 = e.above.iter().map(|&v| u32::from(v)).sum();
            let sum_l: u32 = e.left.iter().map(|&v| u32::from(v)).sum();
            let dc = match (e.have_above, e.have_left) {
                (true, true) => (sum_a + sum_l + (1 << shift)) >> (shift + 1),
                (true, false) => (sum_a + (1 << (shift - 1))) >> shift,
                (false, true) => (sum_l + (1 << (shift - 1))) >> shift,
                (false, false) => 128,
            };
            out[..N * N].fill(dc as u8);
        }
    }
}

/// `v` clamped to a sample.
fn clamp(v: i32) -> u8 {
    v.clamp(0, 255) as u8
}

/// The weighted average of three neighbours, `(x + 2y + z + 2) / 4`.
fn avg3(x: u8, y: u8, z: u8) -> u8 {
    ((u32::from(x) + 2 * u32::from(y) + u32::from(z) + 2) >> 2) as u8
}

/// The average of two neighbours, rounded up.
fn avg2(x: u8, y: u8) -> u8 {
    ((u32::from(x) + u32::from(y) + 1) >> 1) as u8
}

/// Predicts a 4×4 subblock in `mode` from `a` (the 8 pixels above: 4 over the block, then 4 above
/// and to its right), `l` (the 4 to its left) and `p` (above-left), as §12.3's
/// `subblock_intra_predict`.
pub(crate) fn predict_sub(mode: u8, a: &[u8; 8], l: &[u8; 4], p: u8) -> [u8; 16] {
    let mut b = [0u8; 16];
    let mut set = |r: usize, c: usize, v: u8| b[r * 4 + c] = v;
    // E: left column bottom to top, the corner, then the row above.
    let e = [l[3], l[2], l[1], l[0], p, a[0], a[1], a[2], a[3]];
    // avg3p(A + j) for j = -1..=6 (A[-1] is the corner).
    let at = |j: i32| if j < 0 { p } else { a[j as usize] };
    let a3 = |j: i32| avg3(at(j - 1), at(j), at(j + 1));
    let e3 = |j: usize| avg3(e[j - 1], e[j], e[j + 1]);
    let e2 = |j: usize| avg2(e[j], e[j + 1]);
    match mode {
        B_TM_PRED => {
            for r in 0..4 {
                for c in 0..4 {
                    set(r, c, clamp(i32::from(l[r]) + i32::from(a[c]) - i32::from(p)));
                }
            }
        }
        B_VE_PRED => {
            for c in 0..4 {
                let v = a3(c as i32);
                for r in 0..4 {
                    set(r, c, v);
                }
            }
        }
        B_HE_PRED => {
            let rows = [avg3(p, l[0], l[1]), avg3(l[0], l[1], l[2]), avg3(l[1], l[2], l[3]), avg3(l[2], l[3], l[3])];
            for (r, v) in rows.into_iter().enumerate() {
                for c in 0..4 {
                    set(r, c, v);
                }
            }
        }
        B_LD_PRED => {
            for r in 0..4 {
                for c in 0..4 {
                    let d = r + c;
                    set(r, c, if d == 6 { avg3(a[6], a[7], a[7]) } else { a3(d as i32 + 1) });
                }
            }
        }
        B_RD_PRED => {
            for r in 0..4 {
                for c in 0..4 {
                    // (3,0) is E+1 ... (0,3) is E+7.
                    set(r, c, e3(4 + c - r));
                }
            }
        }
        B_VR_PRED => {
            set(3, 0, e3(2));
            set(2, 0, e3(3));
            let v = e3(4);
            set(3, 1, v);
            set(1, 0, v);
            let v = e2(4);
            set(2, 1, v);
            set(0, 0, v);
            let v = e3(5);
            set(3, 2, v);
            set(1, 1, v);
            let v = e2(5);
            set(2, 2, v);
            set(0, 1, v);
            let v = e3(6);
            set(3, 3, v);
            set(1, 2, v);
            let v = e2(6);
            set(2, 3, v);
            set(0, 2, v);
            set(1, 3, e3(7));
            set(0, 3, e2(7));
        }
        B_VL_PRED => {
            let a2 = |j: usize| avg2(a[j], a[j + 1]);
            set(0, 0, a2(0));
            set(1, 0, a3(1));
            let v = a2(1);
            set(2, 0, v);
            set(0, 1, v);
            let v = a3(2);
            set(1, 1, v);
            set(3, 0, v);
            let v = a2(2);
            set(2, 1, v);
            set(0, 2, v);
            let v = a3(3);
            set(3, 1, v);
            set(1, 2, v);
            let v = a2(3);
            set(2, 2, v);
            set(0, 3, v);
            let v = a3(4);
            set(3, 2, v);
            set(1, 3, v);
            set(2, 3, a3(5));
            set(3, 3, a3(6));
        }
        B_HD_PRED => {
            set(3, 0, e2(0));
            set(3, 1, e3(1));
            let v = e2(1);
            set(2, 0, v);
            set(3, 2, v);
            let v = e3(2);
            set(2, 1, v);
            set(3, 3, v);
            let v = e2(2);
            set(2, 2, v);
            set(1, 0, v);
            let v = e3(3);
            set(2, 3, v);
            set(1, 1, v);
            let v = e2(3);
            set(1, 2, v);
            set(0, 0, v);
            let v = e3(4);
            set(1, 3, v);
            set(0, 1, v);
            set(0, 2, e3(5));
            set(0, 3, e3(6));
        }
        B_HU_PRED => {
            set(0, 0, avg2(l[0], l[1]));
            set(0, 1, avg3(l[0], l[1], l[2]));
            let v = avg2(l[1], l[2]);
            set(0, 2, v);
            set(1, 0, v);
            let v = avg3(l[1], l[2], l[3]);
            set(0, 3, v);
            set(1, 1, v);
            let v = avg2(l[2], l[3]);
            set(1, 2, v);
            set(2, 0, v);
            let v = avg3(l[2], l[3], l[3]);
            set(1, 3, v);
            set(2, 1, v);
            for (r, c) in [(2, 2), (2, 3), (3, 0), (3, 1), (3, 2), (3, 3)] {
                set(r, c, l[3]);
            }
        }
        _ => {
            // B_DC_PRED
            let sum: u32 = a[..4].iter().chain(l.iter()).map(|&v| u32::from(v)).sum();
            b.fill(((sum + 4) >> 3) as u8);
        }
    }
    b
}

#[cfg(test)]
mod tests {
    use super::*;

    /// V, TM and DC predictions of a whole block, and DC with an edge missing.
    #[test]
    fn whole_block_modes() {
        let e = Edges::<8> { above: [10, 20, 30, 40, 50, 60, 70, 80], left: [5; 8], corner: 10, have_above: true, have_left: true };
        let mut out = [0u8; 64];
        predict_block(V_PRED, &e, &mut out);
        assert_eq!(&out[56..64], &e.above);
        predict_block(TM_PRED, &e, &mut out);
        assert_eq!(out[0], 5, "5 + 10 - 10");
        assert_eq!(out[7], 75);
        predict_block(DC_PRED, &e, &mut out);
        assert_eq!(out[0], ((360 + 40 + 8) >> 4) as u8);
        let top = Edges::<8> { have_above: false, ..e };
        predict_block(DC_PRED, &top, &mut out);
        assert_eq!(out[0], 5, "left only");
    }

    /// Each subblock mode follows the direction of its lines.
    #[test]
    fn subblock_modes_follow_their_lines() {
        let a = [10, 20, 30, 40, 50, 60, 70, 80];
        let l = [15, 25, 35, 45];
        let p = 5;
        let ld = predict_sub(B_LD_PRED, &a, &l, p);
        // Each antidiagonal is one value.
        assert_eq!(ld[1], ld[4]);
        assert_eq!(ld[3], ld[12]);
        assert_eq!(ld[15], avg3(70, 80, 80));
        let rd = predict_sub(B_RD_PRED, &a, &l, p);
        assert_eq!(rd[0], rd[5]);
        assert_eq!(rd[0], avg3(15, 5, 10));
        let hu = predict_sub(B_HU_PRED, &a, &l, p);
        assert_eq!(hu[15], 45);
        let dc = predict_sub(B_DC_PRED, &a, &l, p);
        assert_eq!(dc[0], ((100 + 120 + 4) >> 3) as u8);
    }
}

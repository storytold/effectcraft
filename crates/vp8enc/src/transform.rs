//! The 4×4 transforms: the decoder's inverse DCT and WHT exactly as RFC 6386 §14.3–14.4 gives
//! them, and forward transforms derived from them.
//!
//! The inverse DCT is, apart from its integer rounding, `R = T·X·Tᵀ / 8` with the 1-D basis
//! `T = [[1, C, 1, S], [1, S, −1, −C], [1, −S, −1, C], [1, −C, 1, −S]]` (C = √2·cos π/8,
//! S = √2·sin π/8). T's columns are orthogonal and of norm 2, so `TᵀT = 4I` and the exact inverse
//! is `X = Tᵀ·R·T / 2`. The inverse WHT is likewise `R = H·Y·Hᵀ / 8` with the 4×4 Hadamard
//! matrix H in the order of §14.3, so `Y = Hᵀ·R·H / 2`.

const COSPI8SQRT2MINUS1: i32 = 20091;
const SINPI8SQRT2: i32 = 35468;

/// Inverse DCT of dequantized coefficients (raster order) to residue, §14.4. Intermediate and
/// final values are 16-bit, as the RFC stores them.
pub(crate) fn idct(input: &[i16; 16]) -> [i16; 16] {
    let mut tmp = [0i16; 16];
    for i in 0..4 {
        let x = |r: usize| i32::from(input[r * 4 + i]);
        let a1 = x(0) + x(2);
        let b1 = x(0) - x(2);
        let c1 = ((x(1) * SINPI8SQRT2) >> 16) - (x(3) + ((x(3) * COSPI8SQRT2MINUS1) >> 16));
        let d1 = (x(1) + ((x(1) * COSPI8SQRT2MINUS1) >> 16)) + ((x(3) * SINPI8SQRT2) >> 16);
        tmp[i] = (a1 + d1) as i16;
        tmp[12 + i] = (a1 - d1) as i16;
        tmp[4 + i] = (b1 + c1) as i16;
        tmp[8 + i] = (b1 - c1) as i16;
    }
    let mut out = [0i16; 16];
    for r in 0..4 {
        let x = |c: usize| i32::from(tmp[r * 4 + c]);
        let a1 = x(0) + x(2);
        let b1 = x(0) - x(2);
        let c1 = ((x(1) * SINPI8SQRT2) >> 16) - (x(3) + ((x(3) * COSPI8SQRT2MINUS1) >> 16));
        let d1 = (x(1) + ((x(1) * COSPI8SQRT2MINUS1) >> 16)) + ((x(3) * SINPI8SQRT2) >> 16);
        out[r * 4] = ((a1 + d1 + 4) >> 3) as i16;
        out[r * 4 + 3] = ((a1 - d1 + 4) >> 3) as i16;
        out[r * 4 + 1] = ((b1 + c1 + 4) >> 3) as i16;
        out[r * 4 + 2] = ((b1 - c1 + 4) >> 3) as i16;
    }
    out
}

/// Inverse WHT of the dequantized Y2 block to the 16 luma DC coefficients, §14.3.
pub(crate) fn iwht(input: &[i16; 16]) -> [i16; 16] {
    let mut tmp = [0i16; 16];
    for i in 0..4 {
        let x = |r: usize| i32::from(input[r * 4 + i]);
        let a1 = x(0) + x(3);
        let b1 = x(1) + x(2);
        let c1 = x(1) - x(2);
        let d1 = x(0) - x(3);
        tmp[i] = (a1 + b1) as i16;
        tmp[4 + i] = (c1 + d1) as i16;
        tmp[8 + i] = (a1 - b1) as i16;
        tmp[12 + i] = (d1 - c1) as i16;
    }
    let mut out = [0i16; 16];
    for r in 0..4 {
        let x = |c: usize| i32::from(tmp[r * 4 + c]);
        let a1 = x(0) + x(3);
        let b1 = x(1) + x(2);
        let c1 = x(1) - x(2);
        let d1 = x(0) - x(3);
        out[r * 4] = ((a1 + b1 + 3) >> 3) as i16;
        out[r * 4 + 1] = ((c1 + d1 + 3) >> 3) as i16;
        out[r * 4 + 2] = ((a1 - b1 + 3) >> 3) as i16;
        out[r * 4 + 3] = ((d1 - c1 + 3) >> 3) as i16;
    }
    out
}

/// The forward DCT's basis, scaled to match the spec's inverse.
fn basis_dct() -> [[f64; 4]; 4] {
    let c = std::f64::consts::SQRT_2 * (std::f64::consts::PI / 8.0).cos();
    let s = std::f64::consts::SQRT_2 * (std::f64::consts::PI / 8.0).sin();
    [[1.0, c, 1.0, s], [1.0, s, -1.0, -c], [1.0, -s, -1.0, c], [1.0, -c, 1.0, -s]]
}

const HADAMARD: [[f64; 4]; 4] = [[1.0, 1.0, 1.0, 1.0], [1.0, 1.0, -1.0, -1.0], [1.0, -1.0, -1.0, 1.0], [1.0, -1.0, 1.0, -1.0]];

/// `Mᵀ·R·M / 2` for a 4×4 `R` in raster order.
fn forward(m: &[[f64; 4]; 4], r: &[f64; 16]) -> [f64; 16] {
    let mut tmp = [0f64; 16]; // Mᵀ·R
    for i in 0..4 {
        for j in 0..4 {
            tmp[i * 4 + j] = (0..4).map(|k| m[k][i] * r[k * 4 + j]).sum();
        }
    }
    let mut out = [0f64; 16];
    for i in 0..4 {
        for j in 0..4 {
            out[i * 4 + j] = (0..4).map(|k| tmp[i * 4 + k] * m[k][j]).sum::<f64>() / 2.0;
        }
    }
    out
}

/// Forward DCT of a 4×4 residue (raster order), in the scale [`idct`] inverts.
pub(crate) fn fdct(residue: &[i32; 16]) -> [f64; 16] {
    let r: [f64; 16] = std::array::from_fn(|i| f64::from(residue[i]));
    forward(&basis_dct(), &r)
}

/// Forward WHT of the 16 luma DC coefficients (raster order of their subblocks), in the scale
/// [`iwht`] inverts.
pub(crate) fn fwht(dc: &[f64; 16]) -> [f64; 16] {
    forward(&HADAMARD, dc)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The DCT and its inverse round-trip residuals within a level.
    #[test]
    fn dct_round_trips() {
        let mut x = 7u32;
        for _ in 0..500 {
            let r: [i32; 16] = std::array::from_fn(|_| {
                x = x.wrapping_mul(1664525).wrapping_add(1013904223);
                (x >> 16) as i32 % 511 - 255
            });
            let c = fdct(&r);
            let q: [i16; 16] = std::array::from_fn(|i| c[i].round() as i16);
            let back = idct(&q);
            for i in 0..16 {
                assert!((i32::from(back[i]) - r[i]).abs() <= 1, "{r:?} -> {back:?}");
            }
        }
    }

    /// The Walsh–Hadamard transform and its inverse round-trip within a level.
    #[test]
    fn wht_round_trips() {
        let mut x = 99u32;
        for _ in 0..500 {
            let d: [f64; 16] = std::array::from_fn(|_| {
                x = x.wrapping_mul(1664525).wrapping_add(1013904223);
                f64::from((x >> 16) as i32 % 4001 - 2000)
            });
            let y = fwht(&d);
            let q: [i16; 16] = std::array::from_fn(|i| y[i].round() as i16);
            let back = iwht(&q);
            for i in 0..16 {
                assert!((f64::from(back[i]) - d[i]).abs() <= 1.0, "{d:?} -> {back:?}");
            }
        }
    }

    /// A lone DC coefficient inverts to a flat block.
    #[test]
    fn a_lone_dc_is_flat() {
        let mut c = [0i16; 16];
        c[0] = 80;
        assert!(idct(&c).iter().all(|&v| v == (80 + 4) >> 3));
    }
}

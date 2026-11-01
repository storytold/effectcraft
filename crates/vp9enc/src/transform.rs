//! 4×4 transforms: the decoder's integer inverse DCT / ADST (spec 8.7.1) and Walsh–Hadamard
//! transform (8.7.1.10), and their forward counterparts (the exact inverse matrices of the
//! decoder's linear transforms, so `inverse4(forward(res)) ≈ res`; the WHT inverse is exact).

pub const DCT_DCT: u8 = 0;
pub const ADST_DCT: u8 = 1;
pub const DCT_ADST: u8 = 2;
pub const ADST_ADST: u8 = 3;

fn r14(v: i32) -> i32 {
    (v + (1 << 13)) >> 14
}

fn idct4(x: [i32; 4]) -> [i32; 4] {
    let v0 = r14(x[0] * 11585 - x[2] * 11585);
    let v1 = r14(x[0] * 11585 + x[2] * 11585);
    let v2 = r14(x[1] * 6270 - x[3] * 15137);
    let v3 = r14(x[1] * 15137 + x[3] * 6270);
    [v1 + v3, v0 + v2, v0 - v2, v1 - v3]
}

fn iadst4(x: [i32; 4]) -> [i32; 4] {
    let (s19, s29, s39, s49) = (5283i64, 9929i64, 13377i64, 15212i64);
    let x: [i64; 4] = x.map(|v| v as i64);
    let s0 = s19 * x[0];
    let s1 = s29 * x[0];
    let s2 = s39 * x[1];
    let s3 = s49 * x[2];
    let s4 = s19 * x[2];
    let s5 = s29 * x[3];
    let s6 = s49 * x[3];
    let s7 = s39 * (x[0] - x[2] + x[3]);
    let x0 = s0 + s3 + s5;
    let x1 = s1 - s4 - s6;
    let x2 = s7;
    let x3 = s2;
    let r = |v: i64| ((v + (1 << 13)) >> 14) as i32;
    [r(x0 + x3), r(x1 + x3), r(x2), r(x0 + x1 - x3)]
}

/// The decoder's 4×4 inverse transform of dequantised coefficients (row-major) to a residual.
pub fn inverse4(coefs: &[i32; 16], tx_type: u8) -> [i32; 16] {
    let row_adst = matches!(tx_type, DCT_ADST | ADST_ADST);
    let col_adst = matches!(tx_type, ADST_DCT | ADST_ADST);
    let mut t = [[0i32; 4]; 4];
    for i in 0..4 {
        let r = [coefs[i * 4], coefs[i * 4 + 1], coefs[i * 4 + 2], coefs[i * 4 + 3]];
        t[i] = if row_adst { iadst4(r) } else { idct4(r) };
    }
    let mut out = [0i32; 16];
    for j in 0..4 {
        let c = [t[0][j], t[1][j], t[2][j], t[3][j]];
        let c = if col_adst { iadst4(c) } else { idct4(c) };
        for i in 0..4 {
            out[i * 4 + j] = (c[i] + 8) >> 4;
        }
    }
    out
}

/// Inverse Walsh–Hadamard (8.7.1.10) on one line.
fn iwht(x: [i32; 4], shift: u32) -> [i32; 4] {
    let mut a = x[0] >> shift;
    let mut c = x[1] >> shift;
    let mut d = x[2] >> shift;
    let mut b = x[3] >> shift;
    a += c;
    d -= b;
    let e = (a - d) >> 1;
    b = e - b;
    c = e - c;
    a -= b;
    d += c;
    [a, b, c, d]
}

/// The exact inverse of [`iwht`] (with shift 0): inputs that produce outputs `y`.
fn fwht(y: [i32; 4]) -> [i32; 4] {
    let (aa, bb, cc, dd) = (y[0], y[1], y[2], y[3]);
    let d1 = dd - cc;
    let a1 = aa + bb;
    let e = (a1 - d1) >> 1;
    let b0 = e - bb;
    let c0 = e - cc;
    let a0 = a1 - c0;
    let d0 = d1 + b0;
    [a0, c0, d0, b0]
}

pub fn inverse_wht(coefs: &[i32; 16]) -> [i32; 16] {
    let mut t = [[0i32; 4]; 4];
    for i in 0..4 {
        t[i] = iwht([coefs[i * 4], coefs[i * 4 + 1], coefs[i * 4 + 2], coefs[i * 4 + 3]], 2);
    }
    let mut out = [0i32; 16];
    for j in 0..4 {
        let r = iwht([t[0][j], t[1][j], t[2][j], t[3][j]], 0);
        for i in 0..4 {
            out[i * 4 + j] = r[i];
        }
    }
    out
}

/// Lossless forward transform: levels (dequantised by 4) whose inverse is exactly `res`.
pub fn forward_wht(res: &[i32; 16]) -> [i32; 16] {
    let mut t = [[0i32; 4]; 4];
    for j in 0..4 {
        let col = fwht([res[j], res[4 + j], res[8 + j], res[12 + j]]);
        for i in 0..4 {
            t[i][j] = col[i];
        }
    }
    let mut lv = [0i32; 16];
    for i in 0..4 {
        let r = fwht(t[i]);
        lv[i * 4..i * 4 + 4].copy_from_slice(&r);
    }
    lv
}

/// Forward transform matrices: the inverses of the decoder's (linear part of the) inverse
/// transforms.
pub fn forward_matrices() -> &'static [[[f64; 16]; 16]; 4] {
    use std::sync::OnceLock;
    static M: OnceLock<[[[f64; 16]; 16]; 4]> = OnceLock::new();
    M.get_or_init(|| {
        let mut out = [[[0.0; 16]; 16]; 4];
        for (ty, o) in out.iter_mut().enumerate() {
            // Columns of the inverse: response to a scaled unit coefficient.
            let k = 1 << 16;
            let mut inv = [[0.0f64; 16]; 16];
            for c in 0..16 {
                let mut e = [0i32; 16];
                e[c] = k;
                let r = inverse4(&e, ty as u8);
                for p in 0..16 {
                    inv[p][c] = r[p] as f64 / k as f64;
                }
            }
            *o = invert16(inv);
        }
        out
    })
}

/// [`forward_matrices`] transposed: `[type][q][k]`.
fn forward_matrices_t() -> &'static [[[f64; 16]; 16]; 4] {
    use std::sync::OnceLock;
    static M: OnceLock<[[[f64; 16]; 16]; 4]> = OnceLock::new();
    M.get_or_init(|| {
        let f = forward_matrices();
        std::array::from_fn(|t| std::array::from_fn(|q| std::array::from_fn(|k| f[t][k][q])))
    })
}

fn invert16(m: [[f64; 16]; 16]) -> [[f64; 16]; 16] {
    let mut a = m;
    let mut inv = [[0.0; 16]; 16];
    for (i, r) in inv.iter_mut().enumerate() {
        r[i] = 1.0;
    }
    for col in 0..16 {
        let piv = (col..16).max_by(|&x, &y| a[x][col].abs().total_cmp(&a[y][col].abs())).unwrap_or(col);
        a.swap(col, piv);
        inv.swap(col, piv);
        let d = a[col][col];
        for j in 0..16 {
            a[col][j] /= d;
            inv[col][j] /= d;
        }
        for r in 0..16 {
            if r != col {
                let f = a[r][col];
                if f != 0.0 {
                    for j in 0..16 {
                        a[r][j] -= f * a[col][j];
                        inv[r][j] -= f * inv[col][j];
                    }
                }
            }
        }
    }
    inv
}

/// Quantisation settings of one block.
#[derive(Clone, Copy)]
pub struct Quant {
    pub dcq: i32,
    pub acq: i32,
    pub lossless: bool,
    /// Rounding offsets (fractions of a step) for DC and AC; smaller = wider dead zone.
    pub round_dc: f64,
    pub round_ac: f64,
}

impl Quant {
    /// Quantised levels (raster order) of a residual.
    pub fn levels(&self, res: &[i32; 16], tx_type: u8) -> [i32; 16] {
        if self.lossless {
            return forward_wht(res);
        }
        let ft = &forward_matrices_t()[tx_type as usize];
        // Every output accumulates its 16 products in the same order (q ascending) as a
        // per-output sum, but the 16 outputs advance together: 16 independent chains that
        // vectorize instead of one serial chain per output.
        let mut acc = [-0.0f64; 16];
        for (q, col) in ft.iter().enumerate() {
            let r = res[q] as f64;
            for k in 0..16 {
                acc[k] += col[k] * r;
            }
        }
        let mut lv = [0i32; 16];
        for k in 0..16 {
            let coef = acc[k];
            let (qs, rnd) = if k == 0 { (self.dcq, self.round_dc) } else { (self.acq, self.round_ac) };
            // Non-negative: truncation is floor.
            let l = (coef.abs() / qs as f64 + rnd) as i32;
            lv[k] = if coef < 0.0 { -l } else { l };
        }
        lv
    }

    /// The decoder's residual for `levels` (all zero when `eob` is 0).
    pub fn residual(&self, levels: &[i32; 16], tx_type: u8, eob: usize) -> [i32; 16] {
        if eob == 0 {
            return [0; 16];
        }
        let mut deq = [0i32; 16];
        for k in 0..16 {
            deq[k] = levels[k] * if k == 0 { self.dcq } else { self.acq };
        }
        if self.lossless { inverse_wht(&deq) } else { inverse4(&deq, tx_type) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wht_is_exactly_invertible() {
        let mut s = 7u32;
        for _ in 0..2000 {
            let mut r = [0i32; 16];
            for v in &mut r {
                s = s.wrapping_mul(1664525).wrapping_add(1013904223);
                *v = (s >> 16) as i32 % 511 - 255;
            }
            let lv = forward_wht(&r);
            let deq = lv.map(|l| l * 4);
            assert_eq!(inverse_wht(&deq), r);
        }
    }

    #[test]
    fn forward_transforms_invert_the_decoder() {
        for ty in 0..4u8 {
            let f = &forward_matrices()[ty as usize];
            let r: [i32; 16] = std::array::from_fn(|i| (i as i32 * 37 % 61) - 30);
            let c: [i32; 16] = std::array::from_fn(|k| (0..16).map(|q| f[k][q] * r[q] as f64).sum::<f64>().round() as i32);
            let back = inverse4(&c, ty);
            for i in 0..16 {
                assert!((back[i] - r[i]).abs() <= 1, "type {ty}: {back:?} vs {r:?}");
            }
        }
    }
}

//! Transforms. The inverse transforms are the normative ones of spec 7.13 (mirrored from
//! filmcraft-av1, so that the encoder reconstructs exactly like a decoder); the forward
//! transforms are floating-point matrices scaled to match them (not normative).

use std::sync::OnceLock;

use crate::tables::{ADST_ADST, ADST_DCT, COS128_LOOKUP, DCT_ADST, DCT_DCT, TRANSFORM_ROW_SHIFT, TX_WIDTH_LOG2};

#[inline(always)]
fn round2_64(x: i64, n: u32) -> i32 {
    if n == 0 { x as i32 } else { ((x + (1i64 << (n - 1))) >> n) as i32 }
}

#[inline(always)]
pub(crate) fn round2(x: i32, n: u32) -> i32 {
    if n == 0 { x } else { (x + (1 << (n - 1))) >> n }
}

#[inline(always)]
fn cos128(angle: i32) -> i64 {
    let a = (angle & 255) as usize;
    (match a {
        0..=64 => COS128_LOOKUP[a] as i32,
        65..=128 => -(COS128_LOOKUP[128 - a] as i32),
        129..=192 => -(COS128_LOOKUP[a - 128] as i32),
        _ => COS128_LOOKUP[256 - a] as i32,
    }) as i64
}

#[inline(always)]
fn sin128(angle: i32) -> i64 {
    cos128(angle - 64)
}

fn brev(num_bits: u32, x: usize) -> usize {
    let mut t = 0;
    for i in 0..num_bits {
        let bit = (x >> i) & 1;
        t += bit << (num_bits - 1 - i);
    }
    t
}

#[inline(always)]
fn bf(t: &mut [i32], a: usize, b: usize, angle: i32, flip: bool) {
    let x = t[a] as i64 * cos128(angle) - t[b] as i64 * sin128(angle);
    let y = t[a] as i64 * sin128(angle) + t[b] as i64 * cos128(angle);
    t[a] = round2_64(x, 12);
    t[b] = round2_64(y, 12);
    if flip {
        t.swap(a, b);
    }
}

#[inline(always)]
fn hd(t: &mut [i32], a: usize, b: usize, flip: bool, r: u32) {
    let (a, b) = if flip { (b, a) } else { (a, b) };
    let x = t[a];
    let y = t[b];
    let lo = -(1i32 << (r - 1));
    let hi = (1i32 << (r - 1)) - 1;
    t[a] = (x + y).clamp(lo, hi);
    t[b] = (x - y).clamp(lo, hi);
}

/// Inverse DCT of length 2^n, n = 2..=5 (7.13.2.3), in place.
fn inverse_dct(t: &mut [i32], n: u32, r: u32) {
    let len = 1usize << n;
    let mut copy = [0i32; 32];
    copy[..len].copy_from_slice(&t[..len]);
    for i in 0..len {
        t[i] = copy[brev(n, i)];
    }
    inverse_dct_initial_stages(t, n, r);
    inverse_dct_final_stages(t, n, r);
}

/// Rotate the high-frequency branches before the four-point core.
fn inverse_dct_initial_stages(t: &mut [i32], n: u32, r: u32) {
    if n >= 5 {
        for i in 0..8 {
            bf(t, 16 + i, 31 - i, 6 + ((brev(3, 7 - i) as i32) << 3), false);
        }
    }
    if n >= 4 {
        for i in 0..4 {
            bf(t, 8 + i, 15 - i, 12 + ((brev(2, 3 - i) as i32) << 4), false);
        }
    }
    if n >= 5 {
        for i in 0..8 {
            hd(t, 16 + 2 * i, 17 + 2 * i, i & 1 == 1, r);
        }
    }
    if n >= 3 {
        for i in 0..2 {
            bf(t, 4 + i, 7 - i, 56 - 32 * i as i32, false);
        }
    }
    if n >= 4 {
        for i in 0..4 {
            hd(t, 8 + 2 * i, 9 + 2 * i, i & 1 == 1, r);
        }
    }
    if n >= 5 {
        for i in 0..2 {
            for j in 0..2 {
                bf(t, 30 - 4 * i - j, 17 + 4 * i + j, 24 + ((j as i32) << 6) + (((1 - i) as i32) << 5), true);
            }
        }
    }
}

/// Evaluate the four-point core and merge progressively larger branches.
fn inverse_dct_final_stages(t: &mut [i32], n: u32, r: u32) {
    for i in 0..2 {
        bf(t, 2 * i, 2 * i + 1, 32 + 16 * i as i32, i == 0);
    }
    if n >= 3 {
        for i in 0..2 {
            hd(t, 4 + 2 * i, 5 + 2 * i, i == 1, r);
        }
    }
    if n >= 4 {
        for i in 0..2 {
            bf(t, 14 - i, 9 + i, 48 + 64 * i as i32, true);
        }
    }
    if n >= 5 {
        for i in 0..4 {
            for j in 0..2 {
                hd(t, 16 + 4 * i + j, 19 + 4 * i - j, i & 1 == 1, r);
            }
        }
    }
    for i in 0..2 {
        hd(t, i, 3 - i, false, r);
    }
    if n >= 3 {
        bf(t, 6, 5, 32, true);
    }
    if n >= 4 {
        for i in 0..2 {
            for j in 0..2 {
                hd(t, 8 + 4 * i + j, 11 + 4 * i - j, i == 1, r);
            }
        }
    }
    if n >= 5 {
        for i in 0..4 {
            bf(t, 29 - i, 18 + i, 48 + (i as i32 >> 1) * 64, true);
        }
    }
    if n >= 3 {
        for i in 0..4 {
            hd(t, i, 7 - i, false, r);
        }
    }
    if n >= 4 {
        for i in 0..2 {
            bf(t, 13 - i, 10 + i, 32, true);
        }
    }
    if n >= 5 {
        for i in 0..2 {
            for j in 0..4 {
                hd(t, 16 + i * 8 + j, 23 + i * 8 - j, i == 1, r);
            }
        }
    }
    if n >= 4 {
        for i in 0..8 {
            hd(t, i, 15 - i, false, r);
        }
    }
    if n >= 5 {
        for i in 0..4 {
            bf(t, 27 - i, 20 + i, 32, true);
        }
    }
    if n >= 5 {
        for i in 0..16 {
            hd(t, i, 31 - i, false, r);
        }
    }
}

const SINPI_1_9: i64 = 1321;
const SINPI_2_9: i64 = 2482;
const SINPI_3_9: i64 = 3344;
const SINPI_4_9: i64 = 3803;

fn inverse_adst4(t: &mut [i32]) {
    let (t0, t1, t2, t3) = (t[0] as i64, t[1] as i64, t[2] as i64, t[3] as i64);
    let mut s = [SINPI_1_9 * t0, SINPI_2_9 * t0, SINPI_3_9 * t1, SINPI_4_9 * t2, SINPI_1_9 * t2, SINPI_2_9 * t3, SINPI_4_9 * t3];
    let a7 = t0 - t2;
    let b7 = a7 + t3;
    s[0] += s[3];
    s[1] -= s[4];
    s[3] = s[2];
    s[2] = SINPI_3_9 * b7;
    s[0] += s[5];
    s[1] -= s[6];
    let x = [s[0] + s[3], s[1] + s[3], s[2], s[0] + s[1] - s[3]];
    for i in 0..4 {
        t[i] = round2_64(x[i], 12);
    }
}

fn adst_in_permute(t: &mut [i32], n: u32) {
    let n0 = 1usize << n;
    let mut copy = [0i32; 16];
    copy[..n0].copy_from_slice(&t[..n0]);
    for i in 0..n0 {
        let idx = if i & 1 == 1 { i - 1 } else { n0 - i - 1 };
        t[i] = copy[idx];
    }
}

fn adst_out_permute(t: &mut [i32], n: u32) {
    let n0 = 1usize << n;
    let mut copy = [0i32; 16];
    copy[..n0].copy_from_slice(&t[..n0]);
    for i in 0..n0 {
        let a = (i >> 3) & 1;
        let b = ((i >> 2) & 1) ^ ((i >> 3) & 1);
        let c = ((i >> 1) & 1) ^ ((i >> 2) & 1);
        let d = (i & 1) ^ ((i >> 1) & 1);
        let idx = ((d << 3) | (c << 2) | (b << 1) | a) >> (4 - n);
        t[i] = if i & 1 == 1 { -copy[idx] } else { copy[idx] };
    }
}

fn inverse_adst8(t: &mut [i32], r: u32) {
    adst_in_permute(t, 3);
    for i in 0..4 {
        bf(t, 2 * i, 2 * i + 1, 60 - 16 * i as i32, true);
    }
    for i in 0..4 {
        hd(t, i, 4 + i, false, r);
    }
    for i in 0..2 {
        bf(t, 4 + 3 * i, 5 + i, 48 - 32 * i as i32, true);
    }
    for i in 0..2 {
        for j in 0..2 {
            hd(t, 4 * j + i, 2 + 4 * j + i, false, r);
        }
    }
    for i in 0..2 {
        bf(t, 2 + 4 * i, 3 + 4 * i, 32, true);
    }
    adst_out_permute(t, 3);
}

fn inverse_adst16(t: &mut [i32], r: u32) {
    adst_in_permute(t, 4);
    for i in 0..8 {
        bf(t, 2 * i, 2 * i + 1, 62 - 8 * i as i32, true);
    }
    for i in 0..8 {
        hd(t, i, 8 + i, false, r);
    }
    for i in 0..2 {
        bf(t, 8 + 2 * i, 9 + 2 * i, 56 - 32 * i as i32, true);
        bf(t, 13 + 2 * i, 12 + 2 * i, 8 + 32 * i as i32, true);
    }
    for i in 0..4 {
        for j in 0..2 {
            hd(t, 8 * j + i, 4 + 8 * j + i, false, r);
        }
    }
    for i in 0..2 {
        for j in 0..2 {
            bf(t, 4 + 8 * j + 3 * i, 5 + 8 * j + i, 48 - 32 * i as i32, true);
        }
    }
    for i in 0..2 {
        for j in 0..4 {
            hd(t, 4 * j + i, 2 + 4 * j + i, false, r);
        }
    }
    for i in 0..4 {
        bf(t, 2 + 4 * i, 3 + 4 * i, 32, true);
    }
    adst_out_permute(t, 4);
}

/// 1-D transform kind.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Kind {
    Dct,
    Adst,
}

/// (vertical / column kind, horizontal / row kind) of a transform type.
pub(crate) fn kinds(tx_type: usize) -> (Kind, Kind) {
    match tx_type {
        DCT_DCT => (Kind::Dct, Kind::Dct),
        ADST_DCT => (Kind::Adst, Kind::Dct),
        DCT_ADST => (Kind::Dct, Kind::Adst),
        ADST_ADST => (Kind::Adst, Kind::Adst),
        // The encoder only chooses the four types above.
        _ => {
            debug_assert!(false, "unsupported transform type {tx_type}");
            (Kind::Dct, Kind::Dct)
        }
    }
}

fn inverse_1d(t: &mut [i32], kind: Kind, n: u32, r: u32) {
    match kind {
        Kind::Dct => inverse_dct(t, n, r),
        Kind::Adst => match n {
            2 => inverse_adst4(t),
            3 => inverse_adst8(t, r),
            _ => inverse_adst16(t, r),
        },
    }
}

/// 2-D inverse transform (7.13.3) of a square `n x n` block (n = 4..=32) of dequantised
/// coefficients (row stride `n`) into `residual` (row stride `n`).
pub(crate) fn inverse_2d(dequant: &[i32], residual: &mut [i32], tx_sz: usize, tx_type: usize, bit_depth: u32) {
    let log2 = TX_WIDTH_LOG2[tx_sz] as u32;
    let n = 1usize << log2;
    let row_shift = TRANSFORM_ROW_SHIFT[tx_sz] as u32;
    let row_clamp = bit_depth + 8;
    let col_clamp = (bit_depth + 6).max(16);
    let (ck, rk) = kinds(tx_type);
    let lo = -(1i32 << (col_clamp - 1));
    let hi = (1i32 << (col_clamp - 1)) - 1;
    let mut t = [0i32; 32];
    for i in 0..n {
        let row = &dequant[i * n..i * n + n];
        if row.iter().all(|&v| v == 0) {
            residual[i * n..i * n + n].iter_mut().for_each(|v| *v = 0);
            continue;
        }
        t[..n].copy_from_slice(row);
        inverse_1d(&mut t, rk, log2, row_clamp);
        for j in 0..n {
            residual[i * n + j] = round2(t[j], row_shift).clamp(lo, hi);
        }
    }
    for j in 0..n {
        for i in 0..n {
            t[i] = residual[i * n + j];
        }
        inverse_1d(&mut t, ck, log2, col_clamp);
        for i in 0..n {
            residual[i * n + j] = round2(t[i], 4);
        }
    }
}

/// Orthonormal forward basis matrices `[k][i]` (row k = basis function k), sizes 4..=32.
struct Bases {
    dct: [Vec<f32>; 4],
    adst: [Vec<f32>; 3],
    /// The same matrices transposed (`[i][k]`).
    dct_t: [Vec<f32>; 4],
    adst_t: [Vec<f32>; 3],
}

fn transpose(m: &[f32]) -> Vec<f32> {
    let n = (m.len() as f64).sqrt() as usize;
    let mut t = vec![0f32; n * n];
    for k in 0..n {
        for i in 0..n {
            t[i * n + k] = m[k * n + i];
        }
    }
    t
}

fn bases() -> &'static Bases {
    static B: OnceLock<Bases> = OnceLock::new();
    B.get_or_init(|| {
        let pi = std::f64::consts::PI;
        let dct = std::array::from_fn(|l| {
            let n = 4usize << l;
            let mut m = vec![0f32; n * n];
            for k in 0..n {
                let c = if k == 0 { std::f64::consts::FRAC_1_SQRT_2 } else { 1.0 };
                for i in 0..n {
                    m[k * n + i] = (c * (2.0 / n as f64).sqrt() * (pi * (2 * i + 1) as f64 * k as f64 / (2 * n) as f64).cos()) as f32;
                }
            }
            m
        });
        let adst = std::array::from_fn(|l| {
            let n = 4usize << l;
            let mut m = vec![0f32; n * n];
            for k in 0..n {
                for i in 0..n {
                    m[k * n + i] = if n == 4 {
                        // the AV1 4-point ADST (sin(pi (i + 1)(2k + 1) / 9) basis)
                        (2.0 / 3.0 * (pi * ((i + 1) * (2 * k + 1)) as f64 / 9.0).sin()) as f32
                    } else {
                        ((2.0 / n as f64).sqrt() * (pi * ((2 * i + 1) * (2 * k + 1)) as f64 / (4 * n) as f64).sin()) as f32
                    };
                }
            }
            m
        });
        let dct_t = std::array::from_fn(|l| transpose(&dct[l]));
        let adst_t = std::array::from_fn(|l| transpose(&adst[l]));
        Bases { dct, adst, dct_t, adst_t }
    })
}

fn basis_t(kind: Kind, n: usize) -> &'static [f32] {
    let l = n.trailing_zeros() as usize - 2;
    match kind {
        Kind::Dct => &bases().dct_t[l],
        Kind::Adst => &bases().adst_t[l],
    }
}

fn basis(kind: Kind, n: usize) -> &'static [f32] {
    let l = n.trailing_zeros() as usize - 2;
    match kind {
        Kind::Dct => &bases().dct[l],
        Kind::Adst => &bases().adst[l],
    }
}

/// Forward 2-D transform of the `n x n` residual (row stride `n`) into orthonormal-domain
/// coefficients scaled by 8 (so that `level = out / q` with the spec's dequantiser).
pub(crate) fn forward_2d(residual: &[i32], out: &mut [f32], n: usize, tx_type: usize) {
    let (ck, rk) = kinds(tx_type);
    let bc = basis(ck, n);
    let brt = basis_t(rk, n);
    // tmp[i][k] = sum_j residual[i][j] * br[k][j]  (horizontal; written as row updates so that
    // the inner loops vectorise)
    let mut tmp = [0f32; 32 * 32];
    for i in 0..n {
        let t = &mut tmp[i * n..i * n + n];
        for j in 0..n {
            let r = residual[i * n + j];
            if r == 0 {
                continue;
            }
            let r = r as f32;
            for (o, &b) in t.iter_mut().zip(&brt[j * n..j * n + n]) {
                *o += r * b;
            }
        }
    }
    // out[k][j] = 8 * sum_i bc[k][i] * tmp[i][j]  (vertical)
    out[..n * n].iter_mut().for_each(|v| *v = 0.0);
    for k in 0..n {
        let o = &mut out[k * n..k * n + n];
        for i in 0..n {
            let b = 8.0 * bc[k * n + i];
            for (v, &t) in o.iter_mut().zip(&tmp[i * n..i * n + n]) {
                *v += b * t;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tables::{TX_4X4, TX_8X8, TX_16X16, TX_32X32};

    /// forward (float) then the normative inverse reproduces the residual: the forward basis
    /// and scale match the decoder's inverse transforms for every supported size and type.
    #[test]
    fn forward_matches_inverse() {
        let mut seed = 99u32;
        let mut rnd = || {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
            ((seed >> 16) % 201) as i32 - 100
        };
        for (tx, n) in [(TX_4X4, 4), (TX_8X8, 8), (TX_16X16, 16), (TX_32X32, 32)] {
            let denom = if tx == TX_32X32 { 2.0 } else { 1.0 };
            for tx_type in [DCT_DCT, ADST_DCT, DCT_ADST, ADST_ADST] {
                if n == 32 && tx_type != DCT_DCT {
                    continue;
                }
                let res: Vec<i32> = (0..n * n).map(|_| rnd()).collect();
                let mut coef = vec![0f32; n * n];
                forward_2d(&res, &mut coef, n, tx_type);
                // q = 8 / denom keeps full precision: dequant = level * q / denom = coef
                let deq: Vec<i32> = coef.iter().map(|&c| (c / denom).round() as i32).collect();
                let mut out = vec![0i32; n * n];
                inverse_2d(&deq, &mut out, tx, tx_type, 8);
                let err: i32 = res.iter().zip(&out).map(|(a, b)| (a - b).abs()).max().unwrap();
                assert!(err <= 1, "tx {tx} type {tx_type}: max error {err}");
            }
        }
    }
}

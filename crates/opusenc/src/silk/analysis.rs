//! Encoder-side signal analysis for SILK (own design): windowed autocorrelation LPC, LPC → NLSF
//! conversion, open-loop pitch search and LTP codebook search.

use super::tables::{CB_NB_20, CB_WB_20, LTP_FILTER, LTP_PERIODICITY, LTP_TAPS_0, LTP_TAPS_1, LTP_TAPS_2};

/// Short-term prediction coefficients `a` (prediction `x[n] ≈ Σ a[j] x[n-1-j]`) from the
/// windowed segment `x`, with a Gaussian lag window and white-noise correction.
pub fn lpc(x: &[f32], order: usize, fs_khz: usize) -> Vec<f64> {
    let n = x.len();
    // Asymmetric window: sin² rise over the first half, flat, cos fall over the last 1/8.
    let rise = n / 2;
    let fall = n / 8;
    let w = |i: usize| -> f64 {
        if i < rise {
            let s = (std::f64::consts::FRAC_PI_2 * (i as f64 + 0.5) / rise as f64).sin();
            s * s
        } else if i >= n - fall {
            (std::f64::consts::FRAC_PI_2 * (i + fall - n) as f64 / fall as f64 + 0.0).cos()
        } else {
            1.0
        }
    };
    let xw: Vec<f64> = x.iter().enumerate().map(|(i, &v)| v as f64 * w(i)).collect();
    let mut r = vec![0f64; order + 1];
    for (k, rk) in r.iter_mut().enumerate() {
        *rk = xw[k..].iter().zip(&xw).map(|(a, b)| a * b).sum();
    }
    if r[0] <= 1e-6 {
        return vec![0.0; order];
    }
    // White-noise correction (-40 dB) and a 50 Hz Gaussian lag window.
    r[0] *= 1.0 + 1e-4;
    let f0 = 2.0 * std::f64::consts::PI * 50.0 / (fs_khz as f64 * 1000.0);
    for (k, rk) in r.iter_mut().enumerate().skip(1) {
        *rk *= (-0.5 * (f0 * k as f64).powi(2)).exp();
    }
    let mut a = levinson(&r, order);
    // Mild bandwidth expansion keeps the NLSFs apart.
    let mut g = 1.0;
    for v in a.iter_mut() {
        g *= 0.996;
        *v *= g;
    }
    a
}

/// Levinson-Durbin recursion on autocorrelation `r` (predictor convention as in [`lpc`]).
pub fn levinson(r: &[f64], order: usize) -> Vec<f64> {
    let mut a = vec![0f64; order];
    let mut err = r[0];
    let mut tmp = vec![0f64; order];
    for i in 0..order {
        let mut acc = r[i + 1];
        for j in 0..i {
            acc -= a[j] * r[i - j];
        }
        let k = (acc / err).clamp(-0.9999, 0.9999);
        tmp[..i].copy_from_slice(&a[..i]);
        for j in 0..i {
            a[j] = tmp[j] - k * tmp[i - 1 - j];
        }
        a[i] = k;
        err *= 1.0 - k * k;
        if err <= 0.0 {
            break;
        }
    }
    a
}

/// Evaluates the symmetric polynomial `p` (length `d + 1`, `d` even) as the cosine series
/// `e^{jωd/2} P(e^{jω})`.
fn cos_series(p: &[f64], w: f64) -> f64 {
    let h = (p.len() - 1) / 2;
    let mut s = p[h];
    for k in 0..h {
        s += 2.0 * p[k] * ((h - k) as f64 * w).cos();
    }
    s
}

fn roots(p: &[f64], out: &mut Vec<f64>) {
    const GRID: usize = 1024;
    let mut w0 = 0.0;
    let mut f0 = cos_series(p, w0);
    for g in 1..=GRID {
        let w1 = std::f64::consts::PI * g as f64 / GRID as f64;
        let f1 = cos_series(p, w1);
        if f0 == 0.0 {
            out.push(w0);
        } else if f0 * f1 < 0.0 {
            let (mut lo, mut hi, mut flo) = (w0, w1, f0);
            for _ in 0..40 {
                let mid = 0.5 * (lo + hi);
                let fm = cos_series(p, mid);
                if fm * flo <= 0.0 {
                    hi = mid;
                } else {
                    lo = mid;
                    flo = fm;
                }
            }
            out.push(0.5 * (lo + hi));
        }
        w0 = w1;
        f0 = f1;
    }
}

/// LPC → normalised line spectral frequencies in radians (ascending, `order` values), or `None`
/// when the root search fails (unstable or degenerate filter).
pub fn a2nlsf(a: &[f64]) -> Option<Vec<f64>> {
    let d = a.len();
    let mut c = vec![0f64; d + 2];
    c[0] = 1.0;
    for k in 0..d {
        c[k + 1] = -a[k];
    }
    let mut p = vec![0f64; d + 2];
    let mut q = vec![0f64; d + 2];
    for k in 0..d + 2 {
        p[k] = c[k] + c[d + 1 - k];
        q[k] = c[k] - c[d + 1 - k];
    }
    // Remove the trivial roots at z = -1 (P) and z = +1 (Q).
    let mut p1 = vec![0f64; d + 1];
    let mut q1 = vec![0f64; d + 1];
    p1[0] = p[0];
    q1[0] = q[0];
    for k in 1..=d {
        p1[k] = p[k] - p1[k - 1];
        q1[k] = q[k] + q1[k - 1];
    }
    let mut r = Vec::with_capacity(d);
    roots(&p1, &mut r);
    roots(&q1, &mut r);
    if r.len() != d {
        return None;
    }
    r.sort_by(f64::total_cmp);
    Some(r)
}

/// Normalised cross-correlation of `x[n]` and `x[n - lag]` over `n` in `range`.
fn ncorr(x: &[f32], range: std::ops::Range<usize>, lag: usize) -> f64 {
    let (mut xy, mut xx, mut yy) = (0f64, 0f64, 0f64);
    for n in range {
        let a = x[n] as f64;
        let b = x[n - lag] as f64;
        xy += a * b;
        xx += a * a;
        yy += b * b;
    }
    if xy <= 0.0 { 0.0 } else { xy / (xx * yy + 1e-9).sqrt() }
}

pub struct Pitch {
    pub lag_index: i32,
    pub contour: usize,
    pub lags: [i32; 4],
    pub score: f64,
}

/// Open-loop pitch search on `x` (history followed by the frame starting at `start`, `len`
/// samples, four subframes) at `fs_khz`.
pub fn pitch_search(x: &[f32], start: usize, len: usize, fs_khz: usize) -> Pitch {
    let fs = fs_khz as i32;
    let min_lag = 2 * fs;
    let max_lag = 18 * fs - 1;
    let sub = len / 4;
    let nl = (max_lag - min_lag + 1) as usize;
    // Whole-frame score per lag.
    let full: Vec<f64> = (0..nl).map(|i| ncorr(x, start..start + len, min_lag as usize + i)).collect();
    let mut best = 0usize;
    for i in 0..nl {
        // Slight preference for shorter lags.
        if full[i] * (1.0 - 0.0004 * i as f64) > full[best] * (1.0 - 0.0004 * best as f64) {
            best = i;
        }
    }
    let mut lag = min_lag + best as i32;
    // Prefer a sub-multiple when it explains the signal almost as well (octave errors).
    for div in [4, 3, 2] {
        let cand = (lag as f64 / div as f64).round() as i32;
        let mut cbest = (0.0, 0);
        for c in cand - 1..=cand + 1 {
            if c >= min_lag && c <= max_lag {
                let s = full[(c - min_lag) as usize];
                if s > cbest.0 {
                    cbest = (s, c);
                }
            }
        }
        if cbest.1 != 0 && cbest.0 > 0.9 * full[(lag - min_lag) as usize] {
            lag = cbest.1;
            break;
        }
    }
    let score = full[(lag - min_lag) as usize];
    // Contour: per-subframe lag offsets around the chosen lag.
    let cb: &[[i32; 4]] = if fs_khz == 8 { &CB_NB_20 } else { &CB_WB_20 };
    let sub_score = |k: usize, l: i32| -> f64 {
        let l = l.clamp(min_lag, 18 * fs);
        ncorr(x, start + k * sub..start + (k + 1) * sub, l as usize)
    };
    let mut best = (f64::MIN, lag, 0usize);
    for l0 in (lag - 2).max(min_lag)..=(lag + 2).min(max_lag) {
        for (ci, offs) in cb.iter().enumerate() {
            let s: f64 = (0..4).map(|k| sub_score(k, l0 + offs[k])).sum::<f64>() - 0.002 * ci as f64;
            if s > best.0 {
                best = (s, l0, ci);
            }
        }
    }
    let (_, l0, ci) = best;
    let mut lags = [0i32; 4];
    for k in 0..4 {
        lags[k] = (l0 + cb[ci][k]).clamp(min_lag, 18 * fs);
    }
    Pitch { lag_index: l0 - min_lag, contour: ci, lags, score }
}

/// Result of the LTP codebook search.
pub struct Ltp {
    pub per_index: usize,
    pub ltp_index: [usize; 4],
    /// Residual energy after LTP prediction / before, per subframe.
    pub err: [f64; 4],
    pub energy: [f64; 4],
}

fn icdf_bits(icdf: &[u8], s: usize) -> f64 {
    let hi = if s == 0 { 256 } else { icdf[s - 1] as u32 };
    let p = (hi - icdf[s] as u32).max(1) as f64 / 256.0;
    -p.log2()
}

/// Searches the three LTP codebooks (RFC 6716 Tables 39–42) for the taps that best predict the
/// residual `r` (history + frame starting at `start`) from its past at the per-subframe `lags`.
pub fn ltp_search(r: &[f64], start: usize, sub: usize, lags: &[i32; 4]) -> Ltp {
    let mut rr = [[0f64; 25]; 4];
    let mut cv = [[0f64; 5]; 4];
    let mut e = [0f64; 4];
    for k in 0..4 {
        let s0 = start + k * sub;
        let lag = lags[k] as usize;
        for n in s0..s0 + sub {
            let t = r[n];
            e[k] += t * t;
            let base = n + 2 - lag;
            for i in 0..5 {
                let vi = r[base - i];
                cv[k][i] += t * vi;
                for j in 0..5 {
                    rr[k][i * 5 + j] += vi * r[base - j];
                }
            }
        }
    }
    let books: [&[[i32; 5]]; 3] = [&LTP_TAPS_0, &LTP_TAPS_1, &LTP_TAPS_2];
    let mut best = (f64::MAX, 0usize, [0usize; 4], [0f64; 4]);
    for (p, book) in books.iter().enumerate() {
        let mut total = icdf_bits(&LTP_PERIODICITY, p) * 1e-3 * e.iter().sum::<f64>() / 4.0;
        let mut idx = [0usize; 4];
        let mut errs = [0f64; 4];
        for k in 0..4 {
            let mut bk = (f64::MAX, 0usize, 0f64);
            for (i, taps) in book.iter().enumerate() {
                let b: [f64; 5] = std::array::from_fn(|j| taps[j] as f64 / 128.0);
                let mut err = e[k];
                for a in 0..5 {
                    err -= 2.0 * b[a] * cv[k][a];
                    for c in 0..5 {
                        err += b[a] * b[c] * rr[k][a * 5 + c];
                    }
                }
                let cost = err + icdf_bits(LTP_FILTER[p], i) * 1e-3 * e[k];
                if cost < bk.0 {
                    bk = (cost, i, err.max(0.0));
                }
            }
            total += bk.0;
            idx[k] = bk.1;
            errs[k] = bk.2;
        }
        if total < best.0 {
            best = (total, p, idx, errs);
        }
    }
    Ltp { per_index: best.1, ltp_index: best.2, err: best.3, energy: e }
}

#[cfg(test)]
mod tests {
    use super::super::lpc::nlsf2a;
    use super::*;

    #[test]
    fn nlsf_roundtrip_through_decoder_conversion() {
        // A resonant 10th/16th-order predictor from a synthetic signal.
        for &(order, fs) in &[(10usize, 8usize), (16, 16)] {
            let mut st = [0f64; 4];
            let mut seed = 1u32;
            let x: Vec<f32> = (0..400)
                .map(|_| {
                    seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    let w = (seed >> 9) as f64 / (1u32 << 23) as f64 - 1.0;
                    let y = w + 1.6 * st[0] - 0.9 * st[1] + 0.3 * st[2] - 0.2 * st[3];
                    st = [y, st[0], st[1], st[2]];
                    (y * 1000.0) as f32
                })
                .collect();
            let a = lpc(&x, order, fs);
            let nlsf = a2nlsf(&a).expect("roots");
            let q15: Vec<i32> = nlsf.iter().map(|w| (w / std::f64::consts::PI * 32768.0).round() as i32).collect();
            let a_q12 = nlsf2a(&q15);
            for j in 0..order {
                assert!((a_q12[j] as f64 / 4096.0 - a[j]).abs() < 0.02, "order {order} coef {j}: {} vs {}", a_q12[j] as f64 / 4096.0, a[j]);
            }
        }
    }
}

//! Band shape coding: PVQ search and codeword indexing, spreading, recursive band splitting
//! (RFC 6716 §4.3.4, encoder direction).
//!
//! The encoder codes 20 ms long-block frames only (no transients, no TF changes), and stereo
//! as dual (independent L/R) stereo, so a band is always coded by `quant_partition` with a single
//! block. Every bit-budget computation mirrors the decoder so both sides take the same
//! split/pulse decisions.

use crate::range::{BITRES, RangeEncoder, ilog};
use crate::rate::{Mode, QTHETA_OFFSET, get_pulses};
use crate::tables::EBANDS;
use crate::{CeltFrame, rate::Allocation};

pub const SPREAD_NORMAL: usize = 2;

#[inline]
fn frac_mul16(a: i32, b: i32) -> i32 {
    (16384 + (a as i16 as i32) * (b as i16 as i32)) >> 15
}

fn bitexact_cos(x: i32) -> i32 {
    let tmp = (4096 + x * x) >> 13;
    let x2 = tmp;
    let x2 = (32767 - x2) + frac_mul16(x2, -7651 + frac_mul16(x2, 8277 + frac_mul16(-626, x2)));
    1 + x2
}

fn bitexact_log2tan(isin: i32, icos: i32) -> i32 {
    let lc = ilog(icos as u32);
    let ls = ilog(isin as u32);
    let icos = icos << (15 - lc);
    let isin = isin << (15 - ls);
    (ls - lc) * (1 << 11) + frac_mul16(isin, frac_mul16(isin, -2597) + 7932) - frac_mul16(icos, frac_mul16(icos, -2597) + 7932)
}

fn compute_qn(n: i32, b: i32, offset: i32, pulse_cap: i32, stereo: bool) -> i32 {
    const EXP2_TABLE8: [i32; 8] = [16384, 17866, 19483, 21247, 23170, 25267, 27554, 30048];
    let mut n2 = 2 * n - 1;
    if stereo && n == 2 {
        n2 -= 1;
    }
    let mut qb = (b + n2 * offset) / n2;
    qb = qb.min(b - pulse_cap - (4 << BITRES));
    qb = qb.min(8 << BITRES);
    if qb < (1 << BITRES >> 1) {
        1
    } else {
        let qn = EXP2_TABLE8[(qb & 0x7) as usize] >> (14 - (qb >> BITRES));
        (qn + 1) >> 1 << 1
    }
}

/// One elementary pass of the spreading rotation, inverted (the decoder applies the forward
/// pass; the encoder undoes it before the pulse search).
fn exp_rotation1_inv(x: &mut [f32], len: usize, stride: usize, c: f32, s: f32) {
    if len <= stride {
        return;
    }
    if len > 2 * stride {
        for i in 0..=len - 2 * stride - 1 {
            let x1 = x[i];
            let x2 = x[i + stride];
            x[i + stride] = c * x2 - s * x1;
            x[i] = c * x1 + s * x2;
        }
    }
    for i in (0..len - stride).rev() {
        let x1 = x[i];
        let x2 = x[i + stride];
        x[i + stride] = c * x2 - s * x1;
        x[i] = c * x1 + s * x2;
    }
}

/// Inverse of the decoder's spreading rotation (§4.3.4.3) for one block of `len` samples.
pub fn exp_rotation_inv(x: &mut [f32], len: usize, k: usize, spread: usize) {
    const SPREAD_FACTOR: [usize; 3] = [15, 10, 5];
    if 2 * k >= len || spread == 0 {
        return;
    }
    let factor = SPREAD_FACTOR[spread - 1];
    let gain = len as f32 / (len + factor * k) as f32;
    let theta = 0.5 * gain * gain;
    let c = (0.5 * std::f32::consts::PI * theta).cos();
    let s = (0.5 * std::f32::consts::PI * (1.0 - theta)).cos();
    let mut stride2 = 0;
    if len >= 8 {
        stride2 = 1;
        while stride2 * stride2 + stride2 < len {
            stride2 += 1;
        }
    }
    // The decoder applies (stride2: s, c) then (1: c, s); undo in reverse order.
    exp_rotation1_inv(x, len, 1, c, s);
    if stride2 != 0 {
        exp_rotation1_inv(x, len, stride2, s, c);
    }
}

/// Greedy PVQ search: the integer vector `y` with `sum |y| = k` closest in direction to `x`.
pub fn pvq_search(x: &[f32], k: usize, y: &mut [i32]) {
    let n = x.len();
    y[..n].fill(0);
    let abs: Vec<f32> = x.iter().map(|v| v.abs()).collect();
    let sum: f32 = abs.iter().sum();
    if !sum.is_finite() || sum <= 1e-15 {
        y[0] = k as i32;
        return;
    }
    let mut placed = 0usize;
    let mut xy = 0f32;
    let mut yy = 0f32;
    if k > n / 2 {
        // Pre-project onto the pyramid (rounding down) and place the rest greedily.
        let r = (k as f32 - 1.0) / sum;
        for j in 0..n {
            let p = (abs[j] * r).floor() as i32;
            y[j] = p;
            placed += p as usize;
            xy += abs[j] * p as f32;
            yy += (p * p) as f32;
        }
    }
    while placed < k {
        let mut best = 0usize;
        let mut best_num = -1f32;
        let mut best_den = 1f32;
        for j in 0..n {
            let num = xy + abs[j];
            let num = num * num;
            let den = yy + 2.0 * y[j] as f32 + 1.0;
            if num * best_den > best_num * den {
                best_num = num;
                best_den = den;
                best = j;
            }
        }
        xy += abs[best];
        yy += 2.0 * y[best] as f32 + 1.0;
        y[best] += 1;
        placed += 1;
    }
    for j in 0..n {
        if x[j] < 0.0 {
            y[j] = -y[j];
        }
    }
}

/// Encodes the PVQ codeword `y` (`sum |y| = k`) as the index the RFC 6716 §4.3.4.2 decoder maps
/// back to `y`.
pub fn encode_pulses(y: &[i32], k: usize, enc: &mut RangeEncoder, v: &mut Vec<u64>) {
    let n = y.len();
    let w = k + 1;
    v.clear();
    v.resize((n + 1) * w, 0);
    v[0] = 1;
    for nn in 1..=n {
        v[nn * w] = 1;
        for kk in 1..=k {
            let a = v[(nn - 1) * w + kk];
            let b = v[nn * w + kk - 1];
            let c = v[(nn - 1) * w + kk - 1];
            v[nn * w + kk] = a.saturating_add(b).saturating_add(c);
        }
    }
    let total = v[n * w + k];
    let vv = |nn: usize, kx: usize| v[nn * w + kx];
    let mut index = 0u64;
    let mut kk = k;
    for j in 0..n {
        if kk == 0 {
            break;
        }
        let rem = n - j;
        let t = y[j].unsigned_abs() as usize;
        // Codewords with |y_j| = t start at sum_{m < kk - t} V(rem-1, m) within their sign half.
        let mut base: u64 = (0..kk - t).map(|m| vv(rem - 1, m)).sum();
        if y[j] < 0 {
            base += (vv(rem - 1, kk) + vv(rem, kk)) / 2;
        }
        index += base;
        kk -= t;
    }
    debug_assert!(total <= u32::MAX as u64);
    if total >= 2 {
        enc.uint(index as u32, total as u32);
    }
}

pub struct BandEnc<'m, 'e> {
    pub m: &'m Mode,
    pub enc: &'e mut RangeEncoder,
    pub i: usize,
    pub spread: usize,
    pub remaining_bits: i32,
    pvq_v: Vec<u64>,
    iy: Vec<i32>,
    tmp: Vec<f32>,
}

impl BandEnc<'_, '_> {
    /// `compute_theta` for a mono split (single block): measures the energy split between the
    /// two halves and codes the quantised angle with the triangular pdf.
    fn compute_theta_split(&mut self, xa: &[f32], xb: &[f32], b: &mut i32, lm: i32) -> (i32, i32, i32) {
        let n = xa.len() as i32;
        let i = self.i;
        let pulse_cap = self.m.log_n[i] + lm * (1 << BITRES);
        let offset = (pulse_cap >> 1) - QTHETA_OFFSET;
        let qn = compute_qn(n, *b, offset, pulse_cap, false);
        let tell = self.enc.tell_frac();
        let mut itheta = 0i32;
        if qn != 1 {
            let mid = xa.iter().map(|v| v * v).sum::<f32>().sqrt();
            let side = xb.iter().map(|v| v * v).sum::<f32>().sqrt();
            let ang = (side as f64).atan2(mid as f64) * std::f64::consts::FRAC_2_PI;
            let it = ((ang * qn as f64 + 0.5).floor() as i32).clamp(0, qn) as u32;
            let q = qn as u32;
            let half = q >> 1;
            let ft = (half + 1) * (half + 1);
            let (fl, fs) = if it <= half { ((it * (it + 1)) >> 1, it + 1) } else { (ft - (((q + 1 - it) * (q + 2 - it)) >> 1), q + 1 - it) };
            self.enc.encode(fl, fl + fs, ft);
            itheta = ((it * 16384) / q) as i32;
        }
        let qalloc = self.enc.tell_frac() - tell;
        *b -= qalloc;
        let delta = if itheta == 0 {
            -16384
        } else if itheta == 16384 {
            16384
        } else {
            let imid = bitexact_cos(itheta);
            let iside = bitexact_cos(16384 - itheta);
            frac_mul16((n - 1) << 7, bitexact_log2tan(iside, imid))
        };
        (itheta, delta, qalloc)
    }

    /// `quant_partition` (single block, encoder side).
    pub fn quant_partition(&mut self, x: &[f32], mut b: i32, mut lm: i32) {
        let n = x.len();
        let i = self.i;
        let cache = self.m.cache(lm, i);
        let cache_max = cache[cache[0] as usize] as i32;
        if lm != -1 && b > cache_max + 12 && n > 2 {
            let half = n >> 1;
            let (xa, xb) = x.split_at(half);
            lm -= 1;
            let (itheta, delta, qalloc) = self.compute_theta_split(xa, xb, &mut b, lm);
            let mut mbits = 0.max(b.min((b - delta) / 2));
            let mut sbits = b - mbits;
            self.remaining_bits -= qalloc;
            let mut rebalance = self.remaining_bits;
            if mbits >= sbits {
                self.quant_partition(xa, mbits, lm);
                rebalance = mbits - (rebalance - self.remaining_bits);
                if rebalance > 3 << BITRES && itheta != 0 {
                    sbits += rebalance - (3 << BITRES);
                }
                self.quant_partition(xb, sbits, lm);
            } else {
                self.quant_partition(xb, sbits, lm);
                rebalance = sbits - (rebalance - self.remaining_bits);
                if rebalance > 3 << BITRES && itheta != 16384 {
                    mbits += rebalance - (3 << BITRES);
                }
                self.quant_partition(xa, mbits, lm);
            }
            return;
        }
        let m = self.m;
        let mut q = m.bits2pulses(i, lm, b);
        let mut curr_bits = m.pulses2bits(i, lm, q);
        self.remaining_bits -= curr_bits;
        while self.remaining_bits < 0 && q > 0 {
            self.remaining_bits += curr_bits;
            q -= 1;
            curr_bits = m.pulses2bits(i, lm, q);
            self.remaining_bits -= curr_bits;
        }
        if q != 0 {
            let k = get_pulses(q) as usize;
            let mut tmp = std::mem::take(&mut self.tmp);
            tmp.clear();
            tmp.extend_from_slice(x);
            exp_rotation_inv(&mut tmp, n, k, self.spread);
            let mut iy = std::mem::take(&mut self.iy);
            iy.clear();
            iy.resize(n, 0);
            pvq_search(&tmp, k, &mut iy);
            encode_pulses(&iy, k, self.enc, &mut self.pvq_v);
            self.iy = iy;
            self.tmp = tmp;
        }
    }
}

/// Shape-coding controls, independent of the selected band allocation.
pub struct BandCoding {
    pub spread: usize,
    pub total_bits: i32,
}

/// Codes all band shapes (`quant_all_bands`, encoder side, long blocks). `x` holds
/// `frame.channels` channels of `n_total` normalised MDCT coefficients each.
pub fn quant_all_bands(m: &Mode, frame: CeltFrame, x: &[f32], n_total: usize, allocation: &Allocation, coding: BandCoding, enc: &mut RangeEncoder) {
    let CeltFrame { start, end, channels: c, lm } = frame;
    let Allocation { pulses, dual_stereo, intensity, coded_bands, balance, .. } = allocation;
    let (dual_stereo, intensity, coded_bands, mut balance) = (*dual_stereo, *intensity, *coded_bands, *balance);
    let BandCoding { spread, total_bits } = coding;
    let mm = 1usize << lm;
    debug_assert!(c == 1 || dual_stereo || intensity <= start, "stereo frames are coded as dual stereo");
    let mut ctx = BandEnc { m, enc, i: 0, spread, remaining_bits: 0, pvq_v: Vec::new(), iy: Vec::new(), tmp: Vec::new() };
    for i in start..end {
        ctx.i = i;
        let xo = mm * EBANDS[i] as usize;
        let n = mm * EBANDS[i + 1] as usize - xo;
        let tell = ctx.enc.tell_frac();
        if i != start {
            balance -= tell;
        }
        let remaining_bits = total_bits - tell - 1;
        ctx.remaining_bits = remaining_bits;
        let b = if i < coded_bands {
            let curr_balance = balance / 3.min(coded_bands as i32 - i as i32);
            0.max(16383.min((remaining_bits + 1).min(pulses[i] + curr_balance)))
        } else {
            0
        };
        // Bands at and above `intensity` (= coded_bands) carry no bits, so they emit no symbols.
        if i < coded_bands || c == 1 {
            if c == 2 && dual_stereo && i < intensity {
                ctx.quant_partition(&x[xo..xo + n], b / 2, lm as i32);
                ctx.quant_partition(&x[n_total + xo..n_total + xo + n], b / 2, lm as i32);
            } else {
                ctx.quant_partition(&x[xo..xo + n], b, lm as i32);
            }
        }
        balance += pulses[i] + tell;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::range::dec::RangeDecoder;

    fn exp_rotation1(x: &mut [f32], len: usize, stride: usize, c: f32, s: f32) {
        let ms = -s;
        if len <= stride {
            return;
        }
        for i in 0..len - stride {
            let x1 = x[i];
            let x2 = x[i + stride];
            x[i + stride] = c * x2 + s * x1;
            x[i] = c * x1 + ms * x2;
        }
        if len > 2 * stride {
            for i in (0..=len - 2 * stride - 1).rev() {
                let x1 = x[i];
                let x2 = x[i + stride];
                x[i + stride] = c * x2 + s * x1;
                x[i] = c * x1 + ms * x2;
            }
        }
    }

    /// The decoder's spreading rotation (single block).
    fn exp_rotation(x: &mut [f32], len: usize, k: usize, spread: usize) {
        const SPREAD_FACTOR: [usize; 3] = [15, 10, 5];
        if 2 * k >= len || spread == 0 {
            return;
        }
        let factor = SPREAD_FACTOR[spread - 1];
        let gain = len as f32 / (len + factor * k) as f32;
        let theta = 0.5 * gain * gain;
        let c = (0.5 * std::f32::consts::PI * theta).cos();
        let s = (0.5 * std::f32::consts::PI * (1.0 - theta)).cos();
        let mut stride2 = 0;
        if len >= 8 {
            stride2 = 1;
            while stride2 * stride2 + stride2 < len {
                stride2 += 1;
            }
        }
        if stride2 != 0 {
            exp_rotation1(x, len, stride2, s, c);
        }
        exp_rotation1(x, len, 1, c, s);
    }

    #[test]
    fn spreading_inverse_is_exact() {
        for &len in &[8usize, 16, 24, 48, 96, 176] {
            for k in [1usize, 2, 3, 5] {
                let orig: Vec<f32> = (0..len).map(|i| ((i * 31 % 17) as f32 - 8.0) / 8.0).collect();
                let mut x = orig.clone();
                exp_rotation_inv(&mut x, len, k, SPREAD_NORMAL);
                exp_rotation(&mut x, len, k, SPREAD_NORMAL);
                for (a, b) in x.iter().zip(&orig) {
                    assert!((a - b).abs() < 1e-4, "len={len} k={k}");
                }
            }
        }
    }

    /// PVQ decoder of RFC 6716 §4.3.4.2 (as in FilmCraft's decoder).
    fn decode_pulses(n: usize, k: usize, dec: &mut RangeDecoder) -> Vec<i32> {
        let w = k + 1;
        let mut v = vec![0u64; (n + 1) * w];
        v[0] = 1;
        for nn in 1..=n {
            v[nn * w] = 1;
            for kk in 1..=k {
                v[nn * w + kk] = v[(nn - 1) * w + kk].saturating_add(v[nn * w + kk - 1]).saturating_add(v[(nn - 1) * w + kk - 1]);
            }
        }
        let total = v[n * w + k];
        let mut i = if total >= 2 { dec.uint(total as u32) as u64 } else { 0 };
        let mut y = vec![0i32; n];
        let mut kk = k;
        for j in 0..n {
            let rem = n - j;
            if kk == 0 {
                break;
            }
            let vv = |nn: usize, kx: usize| v[nn * w + kx];
            let mut p = (vv(rem - 1, kk) + vv(rem, kk)) / 2;
            let neg = i >= p;
            if neg {
                i -= p;
            }
            let k0 = kk;
            p -= vv(rem - 1, kk);
            while p > i && kk > 0 {
                kk -= 1;
                p -= vv(rem - 1, kk);
            }
            let mag = (k0 - kk) as i32;
            y[j] = if neg { -mag } else { mag };
            i -= p.min(i);
        }
        y
    }

    fn all_codewords(n: usize, k: usize) -> Vec<Vec<i32>> {
        if n == 0 {
            return if k == 0 { vec![vec![]] } else { vec![] };
        }
        let mut out = Vec::new();
        for t in -(k as i32)..=(k as i32) {
            for mut rest in all_codewords(n - 1, k - t.unsigned_abs() as usize) {
                rest.insert(0, t);
                out.push(rest);
            }
        }
        out
    }

    #[test]
    fn pvq_index_roundtrip_exhaustive() {
        let mut v = Vec::new();
        for n in 1..6usize {
            for k in 1..6usize {
                for y in all_codewords(n, k) {
                    let mut e = RangeEncoder::new();
                    encode_pulses(&y, k, &mut e, &mut v);
                    let buf = e.done(16);
                    let mut d = RangeDecoder::new(&buf);
                    assert_eq!(decode_pulses(n, k, &mut d), y, "n={n} k={k}");
                }
            }
        }
    }

    #[test]
    fn pvq_search_finds_good_direction() {
        let x: Vec<f32> = (0..32).map(|i| ((i as f32) * 0.7).sin()).collect();
        let mut y = vec![0i32; 32];
        for k in [1usize, 4, 16, 64] {
            pvq_search(&x, k, &mut y);
            assert_eq!(y.iter().map(|v| v.unsigned_abs() as usize).sum::<usize>(), k);
            let xy: f32 = x.iter().zip(&y).map(|(a, &b)| a * b as f32).sum();
            let yy: f32 = y.iter().map(|&b| (b * b) as f32).sum();
            let xx: f32 = x.iter().map(|a| a * a).sum();
            let corr = xy / (xx * yy).sqrt();
            assert!(corr > if k >= 16 { 0.9 } else { 0.2 }, "k={k} corr={corr}");
        }
    }
}

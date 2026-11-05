//! Band energy quantisation: Laplace-coded coarse energy with inter/intra prediction, fine
//! energy and the final refinement bits (RFC 6716 §4.3.2, encoder direction).
//!
//! The quantised energies are updated with exactly the decoder's arithmetic so the prediction
//! state never drifts between encoder and decoder.

use crate::CeltFrame;
use crate::range::RangeEncoder;
use crate::rate::{Allocation, MAX_FINE_BITS};
use crate::tables::{BETA_COEF, BETA_INTRA, E_PROB_MODEL, NB_EBANDS, PRED_COEF, SMALL_ENERGY_ICDF};

const LAPLACE_MINP: u32 = 1;
const LAPLACE_NMIN: u32 = 16;

fn laplace_get_freq1(fs0: u32, decay: u32) -> u32 {
    let ft = 32768 - LAPLACE_MINP * (2 * LAPLACE_NMIN) - fs0;
    (ft * (16384 - decay)) >> 15
}

/// Encodes a Laplace-distributed integer (`fs` = probability of 0 in Q15, `decay` in Q14).
/// Returns the value actually coded (very large magnitudes are clamped to the representable
/// range).
pub fn laplace_encode(enc: &mut RangeEncoder, value: i32, mut fs: u32, decay: u32) -> i32 {
    let mut fl = 0u32;
    let mut coded = value;
    if value != 0 {
        let s: i32 = if value < 0 { -1 } else { 0 };
        let val = value.abs();
        fl = fs;
        fs = laplace_get_freq1(fs, decay);
        let mut i = 1;
        while fs > 0 && i < val {
            fs *= 2;
            fl += fs + 2 * LAPLACE_MINP;
            fs = (fs * decay) >> 15;
            i += 1;
        }
        if fs == 0 {
            let ndi_max = (32768 - fl + LAPLACE_MINP - 1) as i32;
            let ndi_max = (ndi_max - s) >> 1;
            let di = (val - i).min(ndi_max - 1);
            fl = (fl as i32 + (2 * di + 1 + s) * LAPLACE_MINP as i32) as u32;
            fs = LAPLACE_MINP.min(32768 - fl);
            let mag = i + di;
            coded = if s < 0 { -mag } else { mag };
        } else {
            fs += LAPLACE_MINP;
            if s == 0 {
                fl += fs;
            }
        }
    }
    enc.encode_bin(fl, fl + fs, 15);
    coded
}

/// `quant_coarse_energy` for one prediction mode. `e` holds the target log2 energies (relative
/// to `E_MEANS`), `old_e` the quantised energies, updated in place. `budget` is the frame size in
/// bits.
pub fn quant_coarse_energy(frame: CeltFrame, e: &[f32; 2 * NB_EBANDS], old_e: &mut [f32; 2 * NB_EBANDS], intra: bool, enc: &mut RangeEncoder, budget: i32) {
    let CeltFrame { start, end, channels: c, lm } = frame;
    let prob = &E_PROB_MODEL[lm][intra as usize];
    let (coef, beta) = if intra { (0.0, BETA_INTRA) } else { (PRED_COEF[lm], BETA_COEF[lm]) };
    let mut prev = [0f32; 2];
    for i in start..end {
        for ch in 0..c {
            let idx = i + ch * NB_EBANDS;
            let oldc = old_e[idx].max(-9.0);
            let x = e[idx];
            let f = x - coef * oldc - prev[ch];
            let mut qi = (f + 0.5).floor().clamp(-64.0, 64.0) as i32;
            let tell = enc.tell();
            // Keep a few bits per remaining band in reserve when the budget runs out.
            let bits_left = budget - tell - 3 * c as i32 * (end - i) as i32;
            if i != start && bits_left < 30 {
                if bits_left < 24 {
                    qi = qi.min(1);
                }
                if bits_left < 16 {
                    qi = qi.max(-1);
                }
            }
            if budget - tell >= 15 {
                let pi = 2 * i.min(20);
                qi = laplace_encode(enc, qi, (prob[pi] as u32) << 7, (prob[pi + 1] as u32) << 6);
            } else if budget - tell >= 2 {
                qi = qi.clamp(-1, 1);
                enc.icdf(if qi < 0 { (-2 * qi - 1) as usize } else { (2 * qi) as usize }, &SMALL_ENERGY_ICDF, 2);
            } else if budget - tell >= 1 {
                qi = qi.clamp(-1, 0);
                enc.bit_logp(qi == -1, 1);
            } else {
                qi = -1;
            }
            let q = qi as f32;
            old_e[idx] = oldc;
            let tmp = coef * old_e[idx] + prev[ch] + q;
            old_e[idx] = tmp;
            prev[ch] = prev[ch] + q - beta * q;
        }
    }
}

/// `quant_fine_energy`: raw bits refining each band's quantised energy.
pub fn quant_fine_energy(frame: CeltFrame, e: &[f32; 2 * NB_EBANDS], old_e: &mut [f32; 2 * NB_EBANDS], fine_quant: &[i32; NB_EBANDS], enc: &mut RangeEncoder) {
    let CeltFrame { start, end, channels: c, .. } = frame;
    for i in start..end {
        let fq = fine_quant[i];
        if fq <= 0 {
            continue;
        }
        for ch in 0..c {
            let idx = i + ch * NB_EBANDS;
            let err = e[idx] - old_e[idx];
            let levels = 1i32 << fq;
            let q2 = (((err + 0.5) * levels as f32).floor() as i32).clamp(0, levels - 1);
            enc.bits(q2 as u32, fq as u32);
            let offset = (q2 as f32 + 0.5) * (1 << (14 - fq)) as f32 / 16384.0 - 0.5;
            old_e[idx] += offset;
        }
    }
}

/// `quant_energy_finalise`: spends the bits left at the end of the frame on one more bit of
/// energy resolution per band, in priority order.
pub fn quant_energy_finalise(
    frame: CeltFrame,
    e: &[f32; 2 * NB_EBANDS],
    old_e: &mut [f32; 2 * NB_EBANDS],
    allocation: &Allocation,
    mut bits_left: i32,
    enc: &mut RangeEncoder,
) {
    let CeltFrame { start, end, channels: c, .. } = frame;
    let Allocation { fine_quant, fine_priority, .. } = allocation;
    for prio in 0..2 {
        let mut i = start;
        while i < end && bits_left >= c as i32 {
            if fine_quant[i] >= MAX_FINE_BITS || fine_priority[i] != prio {
                i += 1;
                continue;
            }
            for ch in 0..c {
                let idx = i + ch * NB_EBANDS;
                let q2 = (e[idx] - old_e[idx] >= 0.0) as u32;
                enc.bits(q2, 1);
                let offset = (q2 as f32 - 0.5) * (1 << (14 - fine_quant[i] - 1)) as f32 / 16384.0;
                old_e[idx] += offset;
                bits_left -= 1;
            }
            i += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::range::dec::RangeDecoder;

    /// Laplace decoder of RFC 6716 §4.3.2.1 (as in FilmCraft's decoder).
    fn laplace_decode(dec: &mut RangeDecoder, mut fs: u32, decay: u32) -> i32 {
        let mut val = 0i32;
        let fm = dec.decode(32768);
        let mut fl = 0u32;
        if fm >= fs {
            val += 1;
            fl = fs;
            fs = laplace_get_freq1(fs, decay) + LAPLACE_MINP;
            while fs > LAPLACE_MINP && fm >= fl + 2 * fs {
                fs *= 2;
                fl += fs;
                fs = (((fs - 2 * LAPLACE_MINP) * decay) >> 15) + LAPLACE_MINP;
                val += 1;
            }
            if fs <= LAPLACE_MINP {
                let di = (fm - fl) >> 1;
                val += di as i32;
                fl += 2 * di * LAPLACE_MINP;
            }
            if fm < fl + fs {
                val = -val;
            } else {
                fl += fs;
            }
        }
        dec.update(fl, (fl + fs).min(32768), 32768);
        val
    }

    #[test]
    fn laplace_roundtrip_all_models() {
        for lm in 0..4 {
            for intra in 0..2 {
                for band in 0..21 {
                    let prob = &E_PROB_MODEL[lm][intra];
                    let (fs, decay) = ((prob[2 * band] as u32) << 7, (prob[2 * band + 1] as u32) << 6);
                    let vals: Vec<i32> = (-64..=64).collect();
                    let mut e = RangeEncoder::new();
                    let coded: Vec<i32> = vals.iter().map(|&v| laplace_encode(&mut e, v, fs, decay)).collect();
                    let size = (e.tell() as usize).div_ceil(8);
                    let buf = e.done(size);
                    let mut d = RangeDecoder::new(&buf);
                    for (&v, &c) in vals.iter().zip(&coded) {
                        assert!(c == v || (c.signum() == v.signum() && c.abs() < v.abs()), "clamp {v} -> {c}");
                        assert_eq!(laplace_decode(&mut d, fs, decay), c, "lm={lm} intra={intra} band={band} v={v}");
                    }
                }
            }
        }
    }
}

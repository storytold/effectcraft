//! NLSF quantisation (encoder side of RFC 6716 §4.2.7.5): stage-1 vector codebook preselection
//! and a backwards-predictive stage-2 scalar quantiser that reproduces the decoder's residual
//! dequantisation exactly. The search strategy is our own.

use super::lpc::{MAX_LPC_ORDER, nlsf_decode, smlawb, sqrt_approx};
use super::tables::{
    NLSF_CB_NBMB, NLSF_CB_WB, NLSF_EXT, NLSF_PRED_NBMB, NLSF_PRED_WB, NLSF_PSEL_NBMB, NLSF_PSEL_WB, NLSF_SEL_NBMB, NLSF_SEL_WB, NLSF_STAGE1, NLSF_STAGE2,
};

/// Number of stage-1 candidates refined with stage 2.
const SURVIVORS: usize = 8;
/// Rate weight (weighted squared Q15 error per bit).
const LAMBDA: f64 = 60.0;

fn icdf_bits(icdf: &[u8], s: usize) -> f64 {
    let hi = if s == 0 { 256 } else { icdf[s - 1] as u32 };
    let p = (hi - icdf[s] as u32).max(1) as f64 / 256.0;
    -p.log2()
}

/// Bits of a stage-2 index `q` (-10..=10) with stage-2 table `tab`.
fn stage2_bits(tab: &[u8], q: i32) -> f64 {
    let v = q + 4;
    if v <= 0 {
        icdf_bits(tab, 0) + icdf_bits(&NLSF_EXT, (-v) as usize)
    } else if v >= 8 {
        icdf_bits(tab, 8) + icdf_bits(&NLSF_EXT, (v - 8) as usize)
    } else {
        icdf_bits(tab, v as usize)
    }
}

/// Quantises target NLSFs (`target`, Q15 as f64, ascending) and returns the 1 + order indices
/// in the decoder's layout (`[stage1, stage2...]`) together with the reconstructed NLSFs.
pub fn quantize(target: &[f64], signal_type: usize, wb: bool) -> ([i32; 17], [i32; MAX_LPC_ORDER]) {
    let order = target.len();
    // Laroia-style weights.
    let w: Vec<f64> = (0..order)
        .map(|k| {
            let prev = if k == 0 { 0.0 } else { target[k - 1] };
            let next = if k + 1 == order { 32768.0 } else { target[k + 1] };
            1.0 / (target[k] - prev).max(50.0) + 1.0 / (next - target[k]).max(50.0)
        })
        .collect();
    let s1_tab = NLSF_STAGE1[(signal_type >> 1) + if wb { 2 } else { 0 }];
    let n_cb = 32;
    let mut cands: Vec<(f64, usize)> = (0..n_cb)
        .map(|i1| {
            let cb: &[i32] = if wb { &NLSF_CB_WB[i1] } else { &NLSF_CB_NBMB[i1] };
            let d: f64 = (0..order).map(|k| w[k] * ((cb[k] << 7) as f64 - target[k]).powi(2)).sum();
            (d + LAMBDA * icdf_bits(s1_tab, i1), i1)
        })
        .collect();
    cands.sort_by(|a, b| a.0.total_cmp(&b.0));
    let qstep = if wb { 9830 } else { 11796 };
    let mut best = (f64::MAX, [0i32; 17], [0i32; MAX_LPC_ORDER]);
    for &(_, i1) in cands.iter().take(SURVIVORS) {
        let cb1: &[i32] = if wb { &NLSF_CB_WB[i1] } else { &NLSF_CB_NBMB[i1] };
        let mut ix = [0i32; 17];
        ix[0] = i1 as i32;
        let mut bits = icdf_bits(s1_tab, i1);
        // Exact decoder weights per coefficient.
        let w_q9: Vec<i32> = (0..order)
            .map(|k| {
                let prev = if k == 0 { 0 } else { cb1[k - 1] };
                let next = if k + 1 == order { 256 } else { cb1[k + 1] };
                let ww = (1024 / (cb1[k] - prev).max(1) + 1024 / (next - cb1[k]).max(1)).min(32767);
                sqrt_approx(ww << 16).max(1)
            })
            .collect();
        let mut out_q10 = 0i32;
        for i in (0..order).rev() {
            let pred = if i + 1 < order {
                let sel = if wb { NLSF_PSEL_WB[i1][i] } else { NLSF_PSEL_NBMB[i1][i] } as usize;
                let wp = if wb { NLSF_PRED_WB[sel][i] } else { NLSF_PRED_NBMB[sel][i] };
                (out_q10 * wp) >> 8
            } else {
                0
            };
            let cb = if wb { 8 + NLSF_SEL_WB[i1][i] as usize } else { NLSF_SEL_NBMB[i1][i] as usize };
            let tab = NLSF_STAGE2[cb];
            let want_q10 = (target[i] - (cb1[i] << 7) as f64) * w_q9[i] as f64 / 16384.0;
            let step = qstep as f64 * 1024.0 / 65536.0;
            let q0 = ((want_q10 - pred as f64) / step).round() as i32;
            let mut bq = (f64::MAX, 0i32, 0i32);
            for q in (q0 - 1).max(-10)..=(q0 + 1).min(10) {
                let mut v = q << 10;
                if v > 0 {
                    v -= 102;
                } else if v < 0 {
                    v += 102;
                }
                let o = smlawb(pred, v, qstep);
                let nl = (cb1[i] << 7) as f64 + ((o << 14) / w_q9[i]) as f64;
                let cost = w[i] * (nl - target[i]).powi(2) + LAMBDA * stage2_bits(tab, q);
                if cost < bq.0 {
                    bq = (cost, q, o);
                }
            }
            ix[i + 1] = bq.1;
            out_q10 = bq.2;
            bits += stage2_bits(tab, bq.1);
        }
        let nlsf = nlsf_decode(&ix, wb);
        let d: f64 = (0..order).map(|k| w[k] * (nlsf[k] as f64 - target[k]).powi(2)).sum();
        let cost = d + LAMBDA * bits;
        if cost < best.0 {
            best = (cost, ix, nlsf);
        }
    }
    (best.1, best.2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantised_nlsfs_track_the_target() {
        for wb in [false, true] {
            let order = if wb { 16 } else { 10 };
            // A smooth, plausible NLSF vector.
            let target: Vec<f64> = (0..order).map(|k| 32768.0 * (k as f64 + 0.8 + 0.3 * (k as f64).sin()) / (order as f64 + 1.0)).collect();
            let (ix, nlsf) = quantize(&target, 1, wb);
            assert!(ix[1..=order].iter().all(|v| (-10..=10).contains(v)));
            let max_err = (0..order).map(|k| (nlsf[k] as f64 - target[k]).abs()).fold(0.0, f64::max);
            assert!(max_err < 1200.0, "wb={wb} max err {max_err}: {:?} vs {target:?}", &nlsf[..order]);
        }
    }
}

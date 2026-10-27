//! SILK layer encoder (RFC 6716 §4.2, encoder direction), 20 ms frames at 8, 12 or 16 kHz.
//!
//! Each frame is analysed (VAD, open-loop pitch, windowed-autocorrelation LPC, LTP codebook
//! search), its NLSFs are quantised, and the excitation is chosen sample by sample by running
//! the decoder's exact fixed-point reconstruction (LTP and LPC synthesis with the dequantised
//! Q12/Q14 coefficients, quantisation offsets and the pseudo-random sign sequence), so the
//! encoder always holds the decoder's state and there is no drift. A noise-feedback loop
//! shapes the coding noise with a bandwidth-expanded LPC envelope, and a per-frame search over
//! a gain offset hits the bit budget. Stereo input is coded as mid/side with the decoder's
//! stereo prediction (§4.2.8). No LBRR, no NLSF interpolation.

pub(crate) mod analysis;
pub(crate) mod lpc;
mod nlsf;
mod pulses;
#[allow(dead_code)]
pub(crate) mod tables;

use lpc::*;
use tables::*;

use crate::range::RangeEncoder;

const MAX_FRAME: usize = 320;
const LTP_ORDER: usize = 5;
/// Frame RMS (16-bit scale) below which a frame is coded as inactive (VAD off).
const VAD_RMS: f32 = 30.0;
/// Normalised pitch correlation above which a frame is a voicing candidate.
const VOICING_THRESHOLD: f64 = 0.45;
/// Minimum LTP prediction gain (energy ratio) for a frame to be coded as voiced.
const MIN_LTP_GAIN: f64 = 1.3;
/// Rate penalty (squared pulse units per unit magnitude) of the excitation quantiser.
const RD_LAMBDA: f64 = 0.15;
/// Extra gain offset (log2) for inactive frames.
const INACTIVE_OFFSET: f64 = 1.0;
/// Noise feedback: the coding noise is shaped by `1 / A(z/γ)` (A the short-term predictor).
/// γ = 0 leaves it white (closed-loop DPCM, best waveform SNR at high rates), γ = 1 shapes it
/// like the spectral envelope (open-loop residual coding, needed at low rates). γ grows with the
/// gain offset, i.e. as the quantisation gets coarser.
fn nf_gamma(offset: f64) -> f64 {
    (0.75 + 0.05 * offset).clamp(0.75, 0.9)
}

/// `silk_log2lin` (decoder arithmetic).
fn log2lin(in_log_q7: i32) -> i32 {
    if in_log_q7 < 0 {
        return 0;
    }
    if in_log_q7 >= 3967 {
        return i32::MAX;
    }
    let out = 1i32 << (in_log_q7 >> 7);
    let frac = in_log_q7 & 0x7F;
    if in_log_q7 < 2048 { out + ((out * smlawb(frac, frac * (128 - frac), -174)) >> 7) } else { out + (out >> 7) * smlawb(frac, frac * (128 - frac), -174) }
}

fn gain_q16(index: i32) -> i32 {
    log2lin((smulwb(0x1D1C71, index) + 2090).min(3967))
}

/// The decoder's LPC analysis filter on 16-bit samples.
fn lpc_analysis_filter(out: &mut [i16], input: &[i16], b: &[i32], d: usize) {
    let len = out.len();
    for ix in d..len {
        let mut acc: i32 = 0;
        for j in 0..d {
            acc = acc.wrapping_add((input[ix - 1 - j] as i32).wrapping_mul(b[j]));
        }
        let v = ((input[ix] as i32) << 12).wrapping_sub(acc);
        out[ix] = sat16(rshift_round(v, 12)) as i16;
    }
    for v in out.iter_mut().take(d.min(len)) {
        *v = 0;
    }
}

/// The part of the decoder's channel state that the reconstruction depends on.
#[derive(Clone)]
struct DecState {
    prev_gain_q16: i32,
    s_lpc_q14: [i32; MAX_LPC_ORDER],
    out_buf: [i16; MAX_FRAME],
    last_gain_index: i32,
}

impl DecState {
    fn new() -> Self {
        DecState { prev_gain_q16: 65536, s_lpc_q14: [0; MAX_LPC_ORDER], out_buf: [0; MAX_FRAME], last_gain_index: 10 }
    }

    /// The decoder's reset of the side channel when it is coded again after mid-only frames.
    fn reset_side(&mut self) {
        self.out_buf = [0; MAX_FRAME];
        self.s_lpc_q14 = [0; MAX_LPC_ORDER];
        self.last_gain_index = 10;
    }
}

/// Coded parameters of one frame (everything except gains and pulses).
#[derive(Clone, Default)]
struct Params {
    vad: bool,
    signal_type: usize,
    quant_offset_type: usize,
    nlsf_ix: [i32; 17],
    a_q12: [i32; MAX_LPC_ORDER],
    lag_index: i32,
    contour: usize,
    per_index: usize,
    ltp_index: [usize; 4],
    pitch_l: [i32; 4],
    /// RMS (16-bit scale) of the open-loop prediction residual per subframe.
    res_rms: [f32; 4],
}

/// Gains, seed and pulses chosen by the quantiser.
struct Coded {
    gain_ix: [i32; 4],
    gains_q16: [i32; 4],
    seed: usize,
    pulses: Vec<i32>,
}

#[derive(Clone)]
struct Channel {
    fs_khz: usize,
    len: usize,
    sub: usize,
    order: usize,
    dec: DecState,
    /// The last two frames of input (internal rate, 16-bit scale).
    hist: Vec<f32>,
    /// Recent output errors (decoded − input), most recent first, for noise feedback.
    nf: [f64; MAX_LPC_ORDER],
}

impl Channel {
    fn new(fs_khz: usize) -> Self {
        let len = 20 * fs_khz;
        Channel {
            fs_khz,
            len,
            sub: 5 * fs_khz,
            order: if fs_khz == 16 { 16 } else { 10 },
            dec: DecState::new(),
            hist: vec![0.0; 2 * len],
            nf: [0.0; MAX_LPC_ORDER],
        }
    }

    fn push_history(&mut self, x: &[f32]) {
        let l = self.len;
        self.hist.copy_within(l.., 0);
        self.hist[l..].copy_from_slice(x);
    }

    /// Frame analysis: VAD, signal type, pitch, LPC/NLSF quantisation and LTP taps.
    fn analyze(&self, x: &[f32]) -> Params {
        let l = self.len;
        let sub = self.sub;
        let order = self.order;
        let wb = order == 16;
        let mut buf = self.hist.clone();
        buf.extend_from_slice(x);
        let start = 2 * l;
        let rms = (x.iter().map(|v| v * v).sum::<f32>() / l as f32).sqrt();
        let vad = rms > VAD_RMS;
        let pitch = if vad { Some(analysis::pitch_search(&buf, start, l, self.fs_khz)) } else { None };
        let mut voiced = pitch.as_ref().is_some_and(|p| p.score > VOICING_THRESHOLD);

        let a = analysis::lpc(&buf[start - l / 4..], order, self.fs_khz);
        let target: Vec<f64> = match analysis::a2nlsf(&a) {
            Some(w) => w.iter().map(|v| v / std::f64::consts::PI * 32768.0).collect(),
            None => (1..=order).map(|k| k as f64 * 32768.0 / (order + 1) as f64).collect(),
        };
        let quant = |signal_type: usize| {
            let (ix, nlsf) = nlsf::quantize(&target, signal_type, wb);
            (ix, nlsf2a(&nlsf[..order]))
        };
        let mut p = Params { vad, ..Default::default() };
        let (mut ix, mut a_q12) = quant(if voiced { 2 } else { 1 });
        // Open-loop residual with the quantised predictor.
        let mut r = vec![0f64; buf.len()];
        for n in order..buf.len() {
            let mut acc = buf[n] as f64;
            for j in 0..order {
                acc -= a_q12[j] as f64 / 4096.0 * buf[n - 1 - j] as f64;
            }
            r[n] = acc;
        }
        if voiced && let Some(pitch) = &pitch {
            let ltp = analysis::ltp_search(&r, start, sub, &pitch.lags);
            let gain = ltp.energy.iter().sum::<f64>() / ltp.err.iter().sum::<f64>().max(1e-9);
            if gain >= MIN_LTP_GAIN {
                p.lag_index = pitch.lag_index;
                p.contour = pitch.contour;
                p.pitch_l = pitch.lags;
                p.per_index = ltp.per_index;
                p.ltp_index = ltp.ltp_index;
            } else {
                voiced = false;
                (ix, a_q12) = quant(1);
            }
        }
        // Gains follow the short-term residual level (the long-term predictor then only
        // reduces the number of pulses), which keeps the noise-to-signal ratio steady.
        for k in 0..4 {
            let e: f64 = r[start + k * sub..start + (k + 1) * sub].iter().map(|v| v * v).sum();
            p.res_rms[k] = (e / sub as f64).sqrt() as f32;
        }
        p.signal_type = if !vad {
            0
        } else if voiced {
            2
        } else {
            1
        };
        p.quant_offset_type = 0;
        p.nlsf_ix = ix;
        p.a_q12 = a_q12;
        p
    }

    /// Chooses gains and pulses for target `x` by running the decoder's reconstruction, and
    /// advances the mirrored decoder state. `offset` scales the gains (log2), `floor` gives
    /// minimum gains (Q16) per subframe. Returns the coded values and the decoded frame.
    fn quantize(&mut self, p: &Params, x: &[f32], offset: f64, floor: Option<&[i32; 4]>, seed: usize) -> (Coded, Vec<i16>) {
        let l = self.len;
        let sub = self.sub;
        let order = self.order;
        let st = &mut self.dec;
        let nf = &mut self.nf;
        let off = if p.signal_type == 0 { offset + INACTIVE_OFFSET } else { offset };
        let gamma = nf_gamma(off);
        let mut nf_c = [0f64; MAX_LPC_ORDER];
        let mut gpow = 1.0;
        for j in 0..order {
            gpow *= gamma;
            nf_c[j] = p.a_q12[j] as f64 / 4096.0 * gpow;
        }
        // Gains (independent coding of the first subframe, deltas for the rest).
        let mut gain_ix = [0i32; 4];
        let mut gains_q16 = [0i32; 4];
        let mut prev = st.last_gain_index;
        for k in 0..4 {
            let mut g = p.res_rms[k] as f64 * off.exp2();
            if let Some(f) = floor {
                g = g.max(f[k] as f64 / 65536.0);
            }
            let want = (((g * 65536.0).max(1.0).log2() * 128.0 - 2090.0) / (1_907_825.0 / 65536.0)).round().clamp(0.0, 63.0) as i32;
            if k == 0 {
                gain_ix[0] = want;
                prev = want.max(prev - 16).clamp(0, 63);
            } else {
                let mut best = (i32::MAX, 0i32, prev);
                for ind in 0..41 {
                    let tmp = ind - 4;
                    let thr = 2 * 36 - 64 + prev;
                    let d = if tmp > thr { prev + (tmp << 1) - thr } else { prev + tmp }.clamp(0, 63);
                    let e = (d - want).abs();
                    if e < best.0 {
                        best = (e, ind, d);
                    }
                }
                gain_ix[k] = best.1;
                prev = best.2;
            }
            gains_q16[k] = gain_q16(prev);
        }
        st.last_gain_index = prev;

        // Analysis-by-synthesis excitation quantisation through the decoder's exact synthesis.
        let voiced = p.signal_type == 2;
        let offset_q10 = [[100, 240], [32, 100]][p.signal_type >> 1][p.quant_offset_type];
        let off16 = offset_q10 << 4;
        let mut seed_v = seed as i32;
        let mut pulses = vec![0i32; l];
        let mut xq = vec![0i16; l];
        let mut s_lpc = [0i32; MAX_FRAME / 4 + MAX_LPC_ORDER];
        s_lpc[..MAX_LPC_ORDER].copy_from_slice(&st.s_lpc_q14);
        let mut s_ltp = [0i16; MAX_FRAME];
        let mut s_ltp_q15 = [0i32; 2 * MAX_FRAME];
        let ltp_mem = l;
        let mut ltp_buf_idx = ltp_mem;
        let a_q12 = &p.a_q12;
        let ltp_scale_q14 = 15565;
        for k in 0..4 {
            let mut b_q14 = [0i32; LTP_ORDER];
            if voiced {
                let ix = p.ltp_index[k];
                let taps = match p.per_index {
                    0 => LTP_TAPS_0[ix],
                    1 => LTP_TAPS_1[ix],
                    _ => LTP_TAPS_2[ix],
                };
                for i in 0..LTP_ORDER {
                    b_q14[i] = taps[i] << 7;
                }
            }
            let gain_q10 = gains_q16[k] >> 6;
            let mut inv_gain_q31 = inverse32_varq(gains_q16[k], 47);
            let gain_adj_q16 = if gains_q16[k] != st.prev_gain_q16 {
                let g = div32_varq(st.prev_gain_q16, gains_q16[k], 16);
                for v in s_lpc[..MAX_LPC_ORDER].iter_mut() {
                    *v = smulww(g, *v);
                }
                g
            } else {
                1 << 16
            };
            st.prev_gain_q16 = gains_q16[k];
            let lag = p.pitch_l[k];
            if voiced {
                let lag_u = lag as usize;
                if k == 0 {
                    let start_idx = (ltp_mem as isize - lag as isize - order as isize - (LTP_ORDER / 2) as isize).max(0) as usize;
                    lpc_analysis_filter(&mut s_ltp[start_idx..ltp_mem], &st.out_buf[start_idx..ltp_mem], a_q12, order);
                    inv_gain_q31 = smulwb(inv_gain_q31, ltp_scale_q14) << 2;
                    for i in 0..(lag_u + LTP_ORDER / 2).min(ltp_mem) {
                        s_ltp_q15[ltp_buf_idx - i - 1] = smulwb(inv_gain_q31, s_ltp[ltp_mem - i - 1] as i32);
                    }
                } else if gain_adj_q16 != 1 << 16 {
                    for i in 0..(lag_u + LTP_ORDER / 2).min(ltp_buf_idx) {
                        let q = ltp_buf_idx - i - 1;
                        s_ltp_q15[q] = smulww(gain_adj_q16, s_ltp_q15[q]);
                    }
                }
            }
            let inv_g = 16_777_216.0 / gain_q10.max(1) as f64;
            for i in 0..sub {
                let n = k * sub + i;
                seed_v = seed_v.wrapping_mul(196_314_165).wrapping_add(907_633_515);
                let ltp_q14 = if voiced {
                    let base = ltp_buf_idx as isize - lag as isize + (LTP_ORDER / 2) as isize;
                    let g = |o: isize| -> i32 { if base - o >= 0 { s_ltp_q15[(base - o) as usize] } else { 0 } };
                    let mut pred = 2i32;
                    for o in 0..LTP_ORDER {
                        pred = smlawb(pred, g(o as isize), b_q14[o]);
                    }
                    pred << 1
                } else {
                    0
                };
                let mut lpc_pred = (order >> 1) as i32;
                for j in 0..order {
                    lpc_pred = smlawb(lpc_pred, s_lpc[MAX_LPC_ORDER + i - 1 - j], a_q12[j]);
                }
                let lpc_q14 = lpc_pred.saturating_mul(16);
                // Target excitation (Q14) for this sample.
                let fb: f64 = (0..order).map(|j| nf_c[j] * nf[j]).sum();
                let tgt = (x[n] as f64 + fb).clamp(-32768.0, 32767.0) * inv_g - lpc_q14 as f64 - ltp_q14 as f64;
                let sgn = if seed_v < 0 { -1.0 } else { 1.0 };
                let u = sgn * tgt;
                let pf = (u - off16 as f64) / 16384.0;
                let p0 = if pf > 0.0 { (pf + 0.078_125).round() } else { (pf - 0.078_125).round() } as i32;
                let level = |q: i32| -> i32 {
                    let mut e = q << 14;
                    if e > 0 {
                        e -= 80 << 4;
                    } else if e < 0 {
                        e += 80 << 4;
                    }
                    e + off16
                };
                let mut best = (f64::MAX, 0i32);
                for q in [p0 - 1, p0, p0 + 1, 0] {
                    let q = q.clamp(-pulses::MAX_PULSE, pulses::MAX_PULSE);
                    let d = (level(q) as f64 - u) / 16384.0;
                    let cost = d * d + RD_LAMBDA * q.abs() as f64;
                    if cost < best.0 {
                        best = (cost, q);
                    }
                }
                let q = best.1;
                pulses[n] = q;
                let mut e = level(q);
                if seed_v < 0 {
                    e = -e;
                }
                seed_v = seed_v.wrapping_add(q);
                let res = e.wrapping_add(ltp_q14);
                if voiced {
                    s_ltp_q15[ltp_buf_idx] = res << 1;
                    ltp_buf_idx += 1;
                }
                let v = res.saturating_add(lpc_q14);
                s_lpc[MAX_LPC_ORDER + i] = v;
                xq[n] = sat16(rshift_round(smulww(v, gain_q10), 8)) as i16;
                nf.copy_within(0..MAX_LPC_ORDER - 1, 1);
                nf[0] = xq[n] as f64 - x[n] as f64;
            }
            s_lpc.copy_within(sub..sub + MAX_LPC_ORDER, 0);
        }
        st.s_lpc_q14.copy_from_slice(&s_lpc[..MAX_LPC_ORDER]);
        st.out_buf[..l].copy_from_slice(&xq);
        (Coded { gain_ix, gains_q16, seed, pulses }, xq)
    }

    /// Writes the frame's indices and pulses (`decode_indices` + `decode_pulses` order, coded
    /// independently).
    fn write(&self, enc: &mut RangeEncoder, p: &Params, c: &Coded) {
        let ix = p.signal_type * 2 + p.quant_offset_type;
        if p.vad {
            enc.icdf(ix - 2, &TYPE_ACTIVE, 8);
        } else {
            enc.icdf(ix, &TYPE_INACTIVE, 8);
        }
        enc.icdf((c.gain_ix[0] >> 3) as usize, GAIN_MSB[p.signal_type], 8);
        enc.icdf((c.gain_ix[0] & 7) as usize, &UNIFORM8, 8);
        for k in 1..4 {
            enc.icdf(c.gain_ix[k] as usize, &DELTA_GAIN, 8);
        }
        let wb = self.order == 16;
        let i1 = p.nlsf_ix[0] as usize;
        enc.icdf(i1, NLSF_STAGE1[(p.signal_type >> 1) + if wb { 2 } else { 0 }], 8);
        for i in 0..self.order {
            let cb = if wb { 8 + NLSF_SEL_WB[i1][i] as usize } else { NLSF_SEL_NBMB[i1][i] as usize };
            let v = p.nlsf_ix[i + 1] + 4;
            if v <= 0 {
                enc.icdf(0, NLSF_STAGE2[cb], 8);
                enc.icdf((-v) as usize, &NLSF_EXT, 8);
            } else if v >= 8 {
                enc.icdf(8, NLSF_STAGE2[cb], 8);
                enc.icdf((v - 8) as usize, &NLSF_EXT, 8);
            } else {
                enc.icdf(v as usize, NLSF_STAGE2[cb], 8);
            }
        }
        // No NLSF interpolation.
        enc.icdf(4, &NLSF_INTERP, 8);
        if p.signal_type == 2 {
            let half = (self.fs_khz >> 1) as i32;
            enc.icdf((p.lag_index / half) as usize, &PITCH_HIGH, 8);
            let low = match self.fs_khz {
                8 => PITCH_LOW[0],
                12 => PITCH_LOW[1],
                _ => PITCH_LOW[2],
            };
            enc.icdf((p.lag_index % half) as usize, low, 8);
            enc.icdf(p.contour, if self.fs_khz == 8 { PITCH_CONTOUR[1] } else { PITCH_CONTOUR[3] }, 8);
            enc.icdf(p.per_index, &LTP_PERIODICITY, 8);
            for k in 0..4 {
                enc.icdf(p.ltp_index[k], LTP_FILTER[p.per_index], 8);
            }
            // LTP scaling index 0 (15565 in Q14), matching `quantize`.
            enc.icdf(0, &LTP_SCALE, 8);
        }
        enc.icdf(c.seed, &UNIFORM4, 8);
        pulses::encode(enc, &c.pulses, p.signal_type, p.quant_offset_type);
    }
}

/// Quantises a stereo prediction weight (Q13) to the decoder's grid (§4.2.7.1): returns the
/// (UNIFORM3, UNIFORM5, joint) indices and the reconstructed value.
fn quant_pred(w: f64) -> ([usize; 3], i32) {
    let mut best = (f64::MAX, [0usize; 3], 0i32);
    for i in 0..15 {
        let low = STEREO_WEIGHTS_Q13[i];
        let step = smulwb(STEREO_WEIGHTS_Q13[i + 1] - low, 6554);
        for j in 0..5 {
            let v = low + step * (2 * j as i32 + 1);
            let e = (v as f64 - w).abs();
            if e < best.0 {
                best = (e, [i % 3, j, i / 3], v);
            }
        }
    }
    (best.1, best.2)
}

/// Per-frame preparation shared by every rate-loop trial.
struct Prep {
    x: [Vec<f32>; 2],
    params: [Params; 2],
    mid_only: bool,
    pred_ix: [[usize; 3]; 2],
    pred_q13: [i32; 2],
}

/// SILK encoder for one mono or stereo (mid/side) stream.
#[derive(Clone)]
pub struct SilkEncoder {
    fs_khz: usize,
    channels: usize,
    ch: [Channel; 2],
    pred_prev_q13: [i32; 2],
    prev_mid_only: bool,
    /// Last mid sample of the previous frame (for the stereo predictor's low-pass).
    prev_mid: f32,
    frame: usize,
    /// Gain offset chosen for the previous frame (search starting point).
    pub last_offset: f64,
}

impl SilkEncoder {
    pub fn new(fs_khz: usize, channels: usize) -> Self {
        SilkEncoder {
            fs_khz,
            channels,
            ch: [Channel::new(fs_khz), Channel::new(fs_khz)],
            pred_prev_q13: [0; 2],
            prev_mid_only: false,
            prev_mid: 0.0,
            frame: 0,
            last_offset: 1.0,
        }
    }

    /// Samples per channel of one 20 ms frame at the internal rate.
    pub fn frame_len(&self) -> usize {
        20 * self.fs_khz
    }

    fn prepare(&self, input: &[Vec<f32>]) -> Prep {
        let l = self.frame_len();
        if self.channels == 1 {
            let x = input[0].clone();
            let p = self.ch[0].analyze(&x);
            return Prep { x: [x, vec![0.0; l]], params: [p, Params::default()], mid_only: true, pred_ix: [[0; 3]; 2], pred_q13: [0; 2] };
        }
        let mid: Vec<f32> = (0..l).map(|i| 0.5 * (input[0][i] + input[1][i])).collect();
        let side: Vec<f32> = (0..l).map(|i| 0.5 * (input[0][i] - input[1][i])).collect();
        let lp = |j: usize| -> f64 {
            let a = if j == 0 { self.prev_mid } else { mid[j - 1] };
            let c = if j + 1 < l { mid[j + 1] } else { mid[l - 1] };
            (a as f64 + 2.0 * mid[j] as f64 + c as f64) / 4.0
        };
        // Least-squares prediction of the side from the low-passed mid and the mid.
        let (mut r00, mut r01, mut r11, mut c0, mut c1, mut ss, mut mm) = (0f64, 0f64, 0f64, 0f64, 0f64, 0f64, 0f64);
        for j in 0..l {
            let (a, b, s) = (lp(j), mid[j] as f64, side[j] as f64);
            r00 += a * a;
            r01 += a * b;
            r11 += b * b;
            c0 += a * s;
            c1 += b * s;
            ss += s * s;
            mm += b * b;
        }
        // Ridge regularisation: the low-passed mid and the mid are nearly collinear.
        let lam = 0.02 * 0.5 * (r00 + r11) + 1e-9;
        let (a00, a11) = (r00 + lam, r11 + lam);
        let det = a00 * a11 - r01 * r01;
        let (p0, p1) = ((c0 * a11 - c1 * r01) / det, (c1 * a00 - c0 * r01) / det);
        let lim = 13732.0;
        let quant = |p0: f64, p1: f64| {
            let (ix0, q0) = quant_pred(((p0 + p1) * 8192.0).clamp(-lim, lim));
            let (ix1, q1) = quant_pred((p1 * 8192.0).clamp(-lim, lim));
            ([ix0, ix1], [q0 - q1, q1])
        };
        // Side residual with the decoder's interpolated predictor.
        let fs = self.fs_khz as i32;
        let denom_q16 = (1 << 16) / (8 * fs);
        let interp = 8 * self.fs_khz;
        let residual = |pred_q13: [i32; 2]| -> Vec<f32> {
            let d0 = rshift_round((pred_q13[0] - self.pred_prev_q13[0]) as i16 as i32 * denom_q16 as i16 as i32, 16);
            let d1 = rshift_round((pred_q13[1] - self.pred_prev_q13[1]) as i16 as i32 * denom_q16 as i16 as i32, 16);
            let (mut pp0, mut pp1) = (self.pred_prev_q13[0] + d0, self.pred_prev_q13[1] + d1);
            (0..l)
                .map(|j| {
                    if j + 1 < interp {
                        pp0 += d0;
                        pp1 += d1;
                    } else {
                        pp0 = pred_q13[0];
                        pp1 = pred_q13[1];
                    }
                    (side[j] as f64 - (pp0 as f64 * lp(j) + pp1 as f64 * mid[j] as f64) / 8192.0) as f32
                })
                .collect()
        };
        let energy = |v: &[f32]| -> f64 { v.iter().map(|&x| (x as f64).powi(2)).sum() };
        let (mut pred_ix, mut pred_q13) = quant(p0, p1);
        let mut res = residual(pred_q13);
        let mut res_e = energy(&res);
        // Never predict worse than not predicting at all.
        if res_e > ss {
            (pred_ix, pred_q13) = quant(0.0, 0.0);
            res = residual(pred_q13);
            res_e = energy(&res);
        }
        let res_rms = (res_e / l as f64).sqrt();
        let mid_rms = (mm / l as f64).sqrt();
        let mid_only = res_rms < 0.01 * mid_rms + 1.0;
        let pm = self.ch[0].analyze(&mid);
        let ps = if mid_only { Params::default() } else { self.ch[1].analyze(&res) };
        Prep { x: [mid, res], params: [pm, ps], mid_only, pred_ix, pred_q13 }
    }

    /// Codes one frame into `enc` with gain offset `offset`.
    fn code(&mut self, prep: &Prep, offset: f64, enc: &mut RangeEncoder) {
        let stereo = self.channels == 2;
        let side_vad = stereo && !prep.mid_only && prep.params[1].vad;
        enc.bit_logp(prep.params[0].vad, 1);
        enc.bit_logp(false, 1);
        if stereo {
            enc.bit_logp(side_vad, 1);
            enc.bit_logp(false, 1);
            let [a, b] = prep.pred_ix;
            enc.icdf(5 * a[2] + b[2], &STEREO_PRED_JOINT, 8);
            enc.icdf(a[0], &UNIFORM3, 8);
            enc.icdf(a[1], &UNIFORM5, 8);
            enc.icdf(b[0], &UNIFORM3, 8);
            enc.icdf(b[1], &UNIFORM5, 8);
            if !side_vad {
                enc.icdf(prep.mid_only as usize, &MID_ONLY, 8);
            }
        }
        let seed = self.frame & 3;
        let (cm, _) = self.ch[0].quantize(&prep.params[0], &prep.x[0], offset, None, seed);
        self.ch[0].write(enc, &prep.params[0], &cm);
        if stereo {
            if !prep.mid_only {
                if self.prev_mid_only {
                    self.ch[1].dec.reset_side();
                }
                let (cs, _) = self.ch[1].quantize(&prep.params[1], &prep.x[1], offset, Some(&cm.gains_q16), seed);
                self.ch[1].write(enc, &prep.params[1], &cs);
            }
            self.pred_prev_q13 = prep.pred_q13;
            self.prev_mid_only = prep.mid_only;
        }
    }

    /// Encodes one 20 ms frame (`input`: one internal-rate, 16-bit-scale vector per channel)
    /// into `enc`, aiming at `target_bits` total and never exceeding `max_bits`.
    pub fn encode(&mut self, input: &[Vec<f32>], target_bits: f64, max_bits: i32, enc: &mut RangeEncoder) {
        let prep = self.prepare(input);
        let start_bits = enc.tell();
        let trial = |s: &Self, o: f64| -> i32 {
            let mut t = s.clone();
            let mut e = enc.clone();
            t.code(&prep, o, &mut e);
            e.tell() - start_bits
        };
        // Bisection on the gain offset: the smallest offset (finest quantisation) within budget.
        let target = target_bits.min(max_bits as f64);
        // Bits are not strictly monotonic in the offset (very coarse quantisation feeds large
        // errors back), so walk an integer grid from the previous frame's offset to find a
        // bracket [infeasible, feasible], then bisect inside it.
        const MIN_O: f64 = -3.0;
        const MAX_O: f64 = 6.0;
        let mut best_any = (i32::MAX, MAX_O);
        let mut probe = |o: f64| -> bool {
            let b = trial(self, o);
            if b < best_any.0 {
                best_any = (b, o);
            }
            b as f64 <= target
        };
        let g0 = self.last_offset.round().clamp(MIN_O, MAX_O);
        let mut bracket = None;
        if probe(g0) {
            let mut hi = g0;
            loop {
                if hi - 1.0 < MIN_O {
                    bracket = Some((None, hi));
                    break;
                }
                if probe(hi - 1.0) {
                    hi -= 1.0;
                } else {
                    bracket = Some((Some(hi - 1.0), hi));
                    break;
                }
            }
        } else {
            let mut lo = g0;
            while lo + 1.0 <= MAX_O {
                if probe(lo + 1.0) {
                    bracket = Some((Some(lo), lo + 1.0));
                    break;
                }
                lo += 1.0;
            }
        }
        let mut o = match bracket {
            Some((Some(mut lo), mut hi)) => {
                for _ in 0..4 {
                    let m = 0.5 * (lo + hi);
                    if probe(m) {
                        hi = m;
                    } else {
                        lo = m;
                    }
                }
                hi
            }
            Some((None, hi)) => hi,
            None => best_any.1,
        };
        // Hard limit (hybrid packets must leave room for CELT).
        while trial(self, o) > max_bits && o < 12.0 {
            o += 1.0;
        }
        self.last_offset = o;
        self.code(&prep, o, enc);
        for ch in 0..self.channels {
            self.ch[ch].push_history(&prep.x[ch]);
        }
        if self.channels == 2 {
            self.prev_mid = prep.x[0][self.frame_len() - 1];
        }
        self.frame += 1;
    }
}

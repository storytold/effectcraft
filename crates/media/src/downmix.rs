//! Stereo downmix of multichannel audio, shared by preview, waveform and export mixdown (they
//! all read footage audio through `MediaPool::audio_samples`).
//!
//! The matrix is the usual ITU-R BS.775 one: front L/R at unity, the centre and every surround
//! or back channel at −3 dB (1/√2) into both sides' own side (centre into both), the low
//! frequency effects channel left out (as ffmpeg's and swresample's default, `lfe_mix_level` 0;
//! not verified against Adobe's documentation, which does not state it). A back-centre channel
//! reaches both sides at −6 dB. The whole matrix is then scaled by one fixed gain, the reciprocal
//! of the largest row sum (ffmpeg's default normalisation), so that full-scale input can never
//! exceed full scale: 5.1 is scaled by 1/(1 + 0.707 + 0.707). Mono stays duplicated at unity and
//! stereo is untouched.
//!
//! Channel order follows the WAVE / SMPTE convention by channel count (what FilmCraft's AC-3, PCM,
//! FLAC and Opus readers deliver): 3 = L R C, 4 = quad L R Ls Rs, 5 = L R C Ls Rs,
//! 6 = L R C LFE Ls Rs, 7 = L R C LFE Cs Ls Rs, 8 = L R C LFE Lb Rb Ls Rs. FilmCraft's AAC-LC
//! decoder instead delivers AAC's own order (C L R first, LFE last), so AAC gets its own tables.
//! More than 8 channels carry no known layout: only the first two (front L/R) are used.

use std::f32::consts::FRAC_1_SQRT_2;

/// A source channel's contribution to the left and right outputs, before normalisation.
type Pair = [f32; 2];

const FL: Pair = [1.0, 0.0];
const FR: Pair = [0.0, 1.0];
const FC: Pair = [FRAC_1_SQRT_2, FRAC_1_SQRT_2];
const LFE: Pair = [0.0, 0.0];
const SL: Pair = [FRAC_1_SQRT_2, 0.0];
const SR: Pair = [0.0, FRAC_1_SQRT_2];
const CS: Pair = [0.5, 0.5];

/// The per-channel `[left, right]` gains for `count` channels in the given order, normalised.
/// `aac` selects AAC's native channel order. Empty for zero channels.
pub fn stereo_gains(count: usize, aac: bool) -> Vec<Pair> {
    let raw: &[Pair] = match (count, aac) {
        (0, _) => &[],
        (1, _) => &[[1.0, 1.0]],
        (2, _) => &[FL, FR],
        (3, false) => &[FL, FR, FC],
        (3, true) => &[FC, FL, FR],
        (4, false) => &[FL, FR, SL, SR],
        (4, true) => &[FC, FL, FR, CS],
        (5, false) => &[FL, FR, FC, SL, SR],
        (5, true) => &[FC, FL, FR, SL, SR],
        (6, false) => &[FL, FR, FC, LFE, SL, SR],
        (6, true) => &[FC, FL, FR, SL, SR, LFE],
        (7, false) => &[FL, FR, FC, LFE, CS, SL, SR],
        (7, true) => &[FC, FL, FR, SL, SR, CS, LFE],
        (8, false) => &[FL, FR, FC, LFE, SL, SR, SL, SR],
        (8, true) => &[FC, FL, FR, SL, SR, SL, SR, LFE],
        _ => &[FL, FR],
    };
    let mut out = raw.to_vec();
    // Channels beyond a known layout (more than 8) are silent.
    out.resize(count, [0.0, 0.0]);
    let peak = out.iter().map(|g| g[0]).sum::<f32>().max(out.iter().map(|g| g[1]).sum::<f32>());
    // Mono is deliberately left at unity on both sides; so is anything already within range.
    if count > 2 && peak > 1.0 {
        for g in &mut out {
            g[0] /= peak;
            g[1] /= peak;
        }
    }
    out
}

/// Downmix planar `channels` to interleaved stereo in `out` (`out.len() / 2` frames); frames a
/// channel is too short for count as silence, extra frames in `out` stay as they are.
pub fn downmix_into(channels: &[Vec<f32>], aac: bool, out: &mut [f32]) {
    let gains = stereo_gains(channels.len(), aac);
    for (ch, g) in channels.iter().zip(&gains) {
        if g[0] == 0.0 && g[1] == 0.0 {
            continue;
        }
        for (o, v) in out.as_chunks_mut::<2>().0.iter_mut().zip(ch) {
            o[0] += v * g[0];
            o[1] += v * g[1];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_channel_count_has_a_bounded_defined_matrix() {
        for aac in [false, true] {
            for n in 0..40usize {
                let g = stereo_gains(n, aac);
                assert_eq!(g.len(), n);
                let (l, r) = (g.iter().map(|p| p[0]).sum::<f32>(), g.iter().map(|p| p[1]).sum::<f32>());
                if n > 2 {
                    assert!(l <= 1.0001 && r <= 1.0001, "{n} channels (aac {aac}): {l} {r}");
                    assert!(l > 0.0 && r > 0.0);
                }
            }
        }
    }

    #[test]
    fn five_one_matches_the_bs775_matrix() {
        let k = 1.0 / (1.0 + 2.0 * FRAC_1_SQRT_2);
        let g = stereo_gains(6, false);
        assert!((g[0][0] - k).abs() < 1e-6 && g[0][1] == 0.0);
        assert!((g[2][0] - k * FRAC_1_SQRT_2).abs() < 1e-6 && (g[2][1] - k * FRAC_1_SQRT_2).abs() < 1e-6);
        assert_eq!(g[3], [0.0, 0.0], "LFE is left out");
        assert!((g[4][0] - k * FRAC_1_SQRT_2).abs() < 1e-6 && g[4][1] == 0.0);
        assert!((g[5][1] - k * FRAC_1_SQRT_2).abs() < 1e-6 && g[5][0] == 0.0);
        // AAC order: the centre is first, LFE last
        let a = stereo_gains(6, true);
        assert_eq!((a[0], a[5]), (g[2], g[3]));
        assert_eq!((a[1], a[2]), (g[0], g[1]));
    }

    #[test]
    fn mono_and_stereo_are_untouched_and_wide_layouts_use_the_front_pair() {
        assert_eq!(stereo_gains(1, false), vec![[1.0, 1.0]]);
        assert_eq!(stereo_gains(2, true), vec![FL, FR]);
        let g = stereo_gains(12, false);
        assert_eq!((g[0], g[1], g[2]), (FL, FR, [0.0, 0.0]));
    }

    #[test]
    fn ragged_and_empty_input_never_panics() {
        let mut out = vec![0.0; 8];
        downmix_into(&[], false, &mut out);
        downmix_into(&[vec![1.0; 100], vec![1.0; 2], vec![], vec![1.0; 3], vec![1.0; 1], vec![1.0]], true, &mut out);
        assert!(out.iter().all(|v| v.is_finite()));
        let mut none: [f32; 0] = [];
        downmix_into(&vec![vec![1.0; 3]; 7], false, &mut none);
    }
}

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
//! A decoded channel's gains come from where it plays. WAVE / SMPTE order by channel count is what
//! FilmCraft's AC-3, PCM, FLAC and Opus readers deliver: 3 = L R C, 4 = quad L R Ls Rs,
//! 5 = L R C Ls Rs, 6 = L R C LFE Ls Rs, 7 = L R C LFE Cs Ls Rs, 8 = L R C LFE Lb Rb Ls Rs. AAC
//! arrives in an order its configuration decides ([`aac_order`]). More than 8 channels in WAVE
//! order carry no known layout: only the first two (front L/R) are used.

use std::f32::consts::FRAC_1_SQRT_2;
use std::sync::Arc;

use filmcraft_aac::{AudioSpecificConfig, ProgramConfig};

/// A source channel's contribution to the left and right outputs, before normalisation.
pub type Pair = [f32; 2];

const FL: Pair = [1.0, 0.0];
const FR: Pair = [0.0, 1.0];
const FC: Pair = [FRAC_1_SQRT_2, FRAC_1_SQRT_2];
const LFE: Pair = [0.0, 0.0];
const SL: Pair = [FRAC_1_SQRT_2, 0.0];
const SR: Pair = [0.0, FRAC_1_SQRT_2];
const CS: Pair = [0.5, 0.5];

/// AAC channel configuration 7 as FilmCraft decodes it (element order, not reordered).
const CONFIG_7: [Pair; 8] = [FC, FL, FR, SL, SR, SL, SR, LFE];
/// AAC channel configuration 4 (C L R Cs), which FilmCraft reorders to L R C Cs.
const CONFIG_4: [Pair; 4] = [FL, FR, FC, CS];

/// `raw` scaled by the reciprocal of its larger row sum when that exceeds 1 (more than two
/// channels): full-scale input can never exceed full scale.
fn normalised(raw: &[Pair]) -> Vec<Pair> {
    let mut out = raw.to_vec();
    let peak = out.iter().map(|g| g[0]).sum::<f32>().max(out.iter().map(|g| g[1]).sum::<f32>());
    // Mono is deliberately left at unity on both sides; so is anything already within range.
    if out.len() > 2 && peak > 1.0 {
        for g in &mut out {
            g[0] /= peak;
            g[1] /= peak;
        }
    }
    out
}

/// The per-channel `[left, right]` gains for `count` channels in WAVE / SMPTE order, normalised.
/// Empty for zero channels.
pub fn stereo_gains(count: usize) -> Vec<Pair> {
    let raw: &[Pair] = match count {
        0 => &[],
        1 => &[[1.0, 1.0]],
        2 => &[FL, FR],
        3 => &[FL, FR, FC],
        4 => &[FL, FR, SL, SR],
        5 => &[FL, FR, FC, SL, SR],
        6 => &[FL, FR, FC, LFE, SL, SR],
        7 => &[FL, FR, FC, LFE, CS, SL, SR],
        8 => &[FL, FR, FC, LFE, SL, SR, SL, SR],
        _ => &[FL, FR],
    };
    let mut raw = raw.to_vec();
    // Channels beyond a known layout (more than 8) are silent.
    raw.resize(count, [0.0, 0.0]);
    normalised(&raw)
}

/// Where a source's decoded channels play.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Order {
    /// WAVE / SMPTE by channel count: PCM, FLAC, AC-3, Opus and the rest.
    #[default]
    Wav,
    /// AAC whose configuration wasn't read: by channel count as the configurations 1 to 7 decode
    /// (exact for ADTS, which can signal no other configuration FilmCraft decodes), 7 channels as
    /// configuration 11.
    Aac,
    /// Each decoded channel's gains, before normalisation.
    Speakers(Arc<[Pair]>),
}

/// How FilmCraft's decoder hands over AAC with the configuration `asc`: it reorders channel
/// configurations 3 to 6, 11 and 12 to WAVE / SMPTE and leaves 7 and PCE layouts in element order.
pub fn aac_order(asc: &AudioSpecificConfig) -> Order {
    match (asc.channel_config, &asc.pce) {
        (4, _) => Order::Speakers(CONFIG_4.into()),
        (1..=6 | 11 | 12, _) => Order::Wav,
        (7, _) => Order::Speakers(CONFIG_7.into()),
        (0, Some(pce)) => Order::Speakers(pce_speakers(pce).into()),
        _ => Order::Aac,
    }
}

/// ffmpeg's encoder fills a program config element's elements with its layout's channels in its
/// own order (L R C LFE …), and for these structures that order doesn't match the element
/// positions: an LFE in a side SCE, a back centre in a side pair. Front, side and back element
/// types (`true` for a pair) and the channels of the ffmpeg layout with that structure (the first
/// ffmpeg 8.1 lists, where several share it: 6.1 for 6.1(back) and 6.1(front), 7.1(wide) for
/// 7.1(wide-side)).
const FFMPEG_PCE: [(&[bool], &[bool], &[bool], &[Pair]); 6] = [
    (&[true, false], &[false], &[false], &[FL, FR, FC, LFE, CS]),
    (&[true, false], &[false], &[true], &[FL, FR, FC, LFE, SL, SR]),
    (&[true, false], &[true], &[false], &[FL, FR, FC, CS, SL, SR]),
    (&[true, false], &[false], &[true, false], &[FL, FR, FC, LFE, CS, SL, SR]),
    (&[true, false], &[false], &[true, true], &[FL, FR, FC, LFE, SL, SR, FL, FR]),
    (&[true, false], &[true], &[true, false], &[FL, FR, FC, SL, SR, CS, SL, SR]),
];

/// The channels of a PCE layout in decoded (element) order: ffmpeg's for its structures above
/// (it names itself in the PCE's comment), otherwise by position (ISO/IEC 14496-3's program config element):
/// front pairs left and right, a front single the centre, side and back pairs surrounds, a side
/// or back single a back centre.
fn pce_speakers(pce: &ProgramConfig) -> Vec<Pair> {
    let pairs = |els: &[(bool, u8)]| els.iter().map(|e| e.0).collect::<Vec<_>>();
    let (front, side, back) = (pairs(&pce.front), pairs(&pce.side), pairs(&pce.back));
    if pce.comment.starts_with(b"Lavc") && pce.lfe.is_empty() {
        let known = FFMPEG_PCE.iter().find(|(f, s, b, _)| *f == front.as_slice() && *s == side.as_slice() && *b == back.as_slice());
        if let Some((.., speakers)) = known {
            return speakers.to_vec();
        }
    }
    let mut out = Vec::new();
    for pair in front {
        out.extend_from_slice(if pair { &[FL, FR] } else { &[FC] });
    }
    for pair in side.into_iter().chain(back) {
        out.extend_from_slice(if pair { &[SL, SR] } else { &[CS] });
    }
    out.extend(pce.lfe.iter().map(|_| LFE));
    out
}

/// The gains for `count` channels arriving in `order`. FilmCraft decodes as many channels as the
/// container names, the first ones of the layout where that is fewer (MPEG-TS LATM names 7 of
/// configuration 7's 8, ffmpeg 4.2 MP4s 2 of a PCE's); more go by count.
pub fn decoded_gains(count: usize, order: &Order) -> Vec<Pair> {
    match order {
        Order::Speakers(s) if count <= s.len() => normalised(s.get(..count).unwrap_or_default()),
        Order::Aac if count == 4 => normalised(&CONFIG_4),
        Order::Aac if count == 8 => normalised(&CONFIG_7),
        _ => stereo_gains(count),
    }
}

/// Downmix planar `channels` arriving in `order` to interleaved stereo in `out` (`out.len() / 2`
/// frames); frames a channel is too short for count as silence, extra frames in `out` stay as
/// they are.
pub fn downmix_into(channels: &[Vec<f32>], order: &Order, out: &mut [f32]) {
    for (ch, g) in channels.iter().zip(&decoded_gains(channels.len(), order)) {
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
        let pce = Order::Speakers([FL, FR, FC, FC, SL, SR, SL, SR, CS, LFE].into());
        for order in [Order::Wav, Order::Aac, Order::Speakers(CONFIG_7.into()), pce] {
            for n in 0..40usize {
                let g = decoded_gains(n, &order);
                assert_eq!(g.len(), n);
                let (l, r) = (g.iter().map(|p| p[0]).sum::<f32>(), g.iter().map(|p| p[1]).sum::<f32>());
                if n > 2 {
                    assert!(l <= 1.0001 && r <= 1.0001, "{n} channels ({order:?}): {l} {r}");
                    assert!(l > 0.0 && r > 0.0);
                }
            }
        }
    }

    /// AudioSpecificConfigs as ffmpeg 8.1 and FilmCraft's encoder write them (48 kHz AAC-LC).
    const PCE_FFMPEG_61: &str = "118004c848002000c4400d4c61766336322e32382e31303256e500";
    const PCE_FILMCRAFT_61: &str = "118004c845000108840000";

    fn order(hex: &str) -> Order {
        let asc: Vec<u8> = (0..hex.len() / 2).filter_map(|i| hex.get(2 * i..2 * i + 2).and_then(|b| u8::from_str_radix(b, 16).ok())).collect();
        aac_order(&AudioSpecificConfig::parse(&asc).expect("an AudioSpecificConfig"))
    }

    fn speakers(order: &Order) -> Vec<Pair> {
        match order {
            Order::Speakers(s) => s.to_vec(),
            o => panic!("{o:?} names no speakers"),
        }
    }

    #[test]
    fn aac_order_follows_the_channel_configuration() {
        for n in [1, 2, 3, 5, 6, 11, 12] {
            assert_eq!(aac_order(&AudioSpecificConfig::lc(3, n)), Order::Wav, "configuration {n}");
        }
        assert_eq!(speakers(&aac_order(&AudioSpecificConfig::lc(3, 4))), [FL, FR, FC, CS]);
        assert_eq!(speakers(&aac_order(&AudioSpecificConfig::lc(3, 7))), [FC, FL, FR, SL, SR, SL, SR, LFE]);
        assert_eq!(speakers(&order(PCE_FFMPEG_61)), [FL, FR, FC, LFE, CS, SL, SR], "ffmpeg's 6.1 carries the LFE in a side SCE");
        assert_eq!(speakers(&order(PCE_FILMCRAFT_61)), [FC, FL, FR, SL, SR, CS, LFE], "a centre-first PCE by position");
    }

    #[test]
    fn only_ffmpeg_pce_structures_it_misplaces_take_its_channel_order() {
        let pce = |comment: &[u8], side: Vec<(bool, u8)>, lfe: Vec<u8>| ProgramConfig {
            front: vec![(true, 0), (false, 0)],
            side,
            back: vec![(true, 1)],
            lfe,
            comment: comment.to_vec(),
            ..Default::default()
        };
        // 5.1(side): ffmpeg's LFE sits in the side SCE
        assert_eq!(pce_speakers(&pce(b"Lavc58.54.100", vec![(false, 1)], vec![])), [FL, FR, FC, LFE, SL, SR]);
        // the same structure from another encoder: a back centre by position
        assert_eq!(pce_speakers(&pce(b"", vec![(false, 1)], vec![])), [FL, FR, FC, CS, SL, SR]);
        // 7.0: ffmpeg's order matches the positions
        assert_eq!(pce_speakers(&pce(b"Lavc62.28.102", vec![(true, 2)], vec![])), [FL, FR, FC, SL, SR, SL, SR]);
        // a real LFE element is never ffmpeg's misplaced one
        assert_eq!(pce_speakers(&pce(b"Lavc62.28.102", vec![(false, 1)], vec![0])), [FL, FR, FC, CS, SL, SR, LFE]);
    }

    #[test]
    fn a_decode_of_fewer_channels_takes_the_first_of_the_layout_and_more_go_by_count() {
        assert_eq!(decoded_gains(2, &order(PCE_FILMCRAFT_61)), [FC, FL]);
        assert_eq!(decoded_gains(7, &Order::Speakers(CONFIG_7.into())), normalised(&CONFIG_7[..7]));
        assert_eq!(decoded_gains(9, &Order::Speakers(CONFIG_7.into())), stereo_gains(9));
        assert_eq!(decoded_gains(4, &Order::Aac), normalised(&[FL, FR, FC, CS]));
        assert_eq!(decoded_gains(8, &Order::Aac), normalised(&CONFIG_7));
        for n in [3, 5, 6, 7] {
            assert_eq!(decoded_gains(n, &Order::Aac), stereo_gains(n), "{n} channels");
        }
    }

    #[test]
    fn five_one_matches_the_bs775_matrix() {
        let k = 1.0 / (1.0 + 2.0 * FRAC_1_SQRT_2);
        let g = stereo_gains(6);
        assert!((g[0][0] - k).abs() < 1e-6 && g[0][1] == 0.0);
        assert!((g[2][0] - k * FRAC_1_SQRT_2).abs() < 1e-6 && (g[2][1] - k * FRAC_1_SQRT_2).abs() < 1e-6);
        assert_eq!(g[3], [0.0, 0.0], "LFE is left out");
        assert!((g[4][0] - k * FRAC_1_SQRT_2).abs() < 1e-6 && g[4][1] == 0.0);
        assert!((g[5][1] - k * FRAC_1_SQRT_2).abs() < 1e-6 && g[5][0] == 0.0);
    }

    #[test]
    fn mono_and_stereo_are_untouched_and_wide_layouts_use_the_front_pair() {
        assert_eq!(stereo_gains(1), vec![[1.0, 1.0]]);
        assert_eq!(stereo_gains(2), vec![FL, FR]);
        let g = stereo_gains(12);
        assert_eq!((g[0], g[1], g[2]), (FL, FR, [0.0, 0.0]));
    }

    #[test]
    fn ragged_and_empty_input_never_panics() {
        let mut out = vec![0.0; 8];
        downmix_into(&[], &Order::Wav, &mut out);
        downmix_into(&[vec![1.0; 100], vec![1.0; 2], vec![], vec![1.0; 3], vec![1.0; 1], vec![1.0]], &Order::Aac, &mut out);
        assert!(out.iter().all(|v| v.is_finite()));
        let mut none: [f32; 0] = [];
        downmix_into(&vec![vec![1.0; 3]; 7], &Order::Speakers(CONFIG_7.into()), &mut none);
    }
}

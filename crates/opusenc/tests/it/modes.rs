//! SILK, hybrid and CELT round trips through FilmCraft's spec-derived Opus decoder: every
//! mode/bandwidth decodes, waveform quality on speech-like and music-like signals, bitrate
//! tracking, stereo, the application tables and delay alignment.

use crate::common;

use common::*;
use effectcraft_opusenc::{Application, Bandwidth, Mode, OpusEncoder};

struct Run {
    snr: f64,
    seg: f64,
    delay: isize,
    kbps: f64,
    packets: Vec<Vec<u8>>,
}

/// Encodes `planar`, decodes, and measures each channel against `reference` (searching a
/// residual delay of up to ±6 samples).
fn run(enc: OpusEncoder, planar: &[Vec<f32>], reference: &[Vec<f32>]) -> Vec<Run> {
    let (enc, packets) = encode(enc, planar);
    let dec = decode(&enc.opus_head(), &packets);
    let kbps = bitrate(&packets) / 1000.0;
    reference
        .iter()
        .zip(&dec)
        .map(|(r, d)| {
            assert!(d.len() >= r.len(), "decoded {} < {}", d.len(), r.len());
            assert!(d.iter().all(|v| v.is_finite()));
            let (snr, delay) = snr_search(r, d, 6);
            Run { snr, seg: seg_snr(r, d, delay), delay, kbps, packets: packets.clone() }
        })
        .collect()
}

fn mono(mode: Mode, bw: Bandwidth, rate: u32, x: &[f32], fc: f64) -> Run {
    let reference = if fc < 20_000.0 { lowpass(x, fc) } else { x.to_vec() };
    run(OpusEncoder::with_mode(1, rate, mode, bw), &[x.to_vec()], &[reference]).remove(0)
}

/// Upper edge of the band a SILK bandwidth keeps (for band-limited references).
fn band_edge(bw: Bandwidth) -> f64 {
    match bw {
        Bandwidth::Narrowband => 3_400.0,
        Bandwidth::Mediumband => 5_000.0,
        Bandwidth::Wideband => 7_000.0,
        _ => 24_000.0,
    }
}

const LEN: usize = 96_000;

#[test]
fn every_mode_and_bandwidth_decodes() {
    let sp = speech(LEN + 40_000, 11);
    let left = sp[..LEN].to_vec();
    let right: Vec<f32> = sp[40_000..].iter().map(|v| v * 0.6).collect();
    for (mode, bw, config, rate) in [
        (Mode::Silk, Bandwidth::Narrowband, 1u8, 8_000u32),
        (Mode::Silk, Bandwidth::Mediumband, 5, 11_000),
        (Mode::Silk, Bandwidth::Wideband, 9, 16_000),
        (Mode::Hybrid, Bandwidth::SuperWideband, 13, 24_000),
        (Mode::Hybrid, Bandwidth::Fullband, 15, 32_000),
        (Mode::Celt, Bandwidth::Fullband, 31, 64_000),
    ] {
        for c in [1usize, 2] {
            let planar = if c == 1 { vec![left.clone()] } else { vec![left.clone(), right.clone()] };
            let reference: Vec<Vec<f32>> = planar.iter().map(|x| lowpass(x, band_edge(bw))).collect();
            let enc = OpusEncoder::with_mode(c as u8, rate * c as u32, mode, bw);
            assert_eq!((enc.mode(), enc.bandwidth()), (mode, bw));
            let res = run(enc, &planar, &reference);
            for p in &res[0].packets {
                assert_eq!(p[0], (config << 3) | if c == 2 { 4 } else { 0 }, "TOC of {mode:?}/{bw:?}");
                assert!(p.len() >= 3 && p.len() <= 1276);
            }
            for (ch, r) in res.iter().enumerate() {
                eprintln!(
                    "{mode:?}/{bw:?} {c} ch @ {} kb/s: {:.1} kb/s, channel {ch} SNR {:.1} dB, segSNR {:.1} dB, residual delay {}",
                    rate * c as u32 / 1000,
                    r.kbps,
                    r.snr,
                    r.seg,
                    r.delay
                );
                assert!(r.snr > 8.0, "{mode:?}/{bw:?} {c} ch channel {ch}: SNR {:.1} dB", r.snr);
                assert!(r.delay.abs() <= 1, "{mode:?}/{bw:?}: residual delay {} (pre-skip mismatch)", r.delay);
            }
        }
    }
}

#[test]
fn silk_speech_quality() {
    let x = speech(LEN, 7);
    for (bw, rate, min_snr) in [
        (Bandwidth::Narrowband, 8_000u32, 10.0),
        (Bandwidth::Mediumband, 11_000, 14.0),
        (Bandwidth::Wideband, 12_000, 14.0),
        (Bandwidth::Wideband, 16_000, 18.0),
        (Bandwidth::Wideband, 24_000, 24.0),
    ] {
        let r = mono(Mode::Silk, bw, rate, &x, band_edge(bw));
        eprintln!("SILK {bw:?} speech @ {} kb/s: {:.1} kb/s, SNR {:.1} dB, segSNR {:.1} dB", rate / 1000, r.kbps, r.snr, r.seg);
        assert!(r.snr > min_snr && r.seg > min_snr - 3.0, "SILK {bw:?} @ {rate}: SNR {:.1} / seg {:.1}", r.snr, r.seg);
    }
}

#[test]
fn silk_beats_celt_on_speech_at_low_rate() {
    let x = speech(LEN, 5);
    // Both measured against the full-band input.
    let silk = mono(Mode::Silk, Bandwidth::Wideband, 12_000, &x, 24_000.0);
    let celt = mono(Mode::Celt, Bandwidth::Fullband, 12_000, &x, 24_000.0);
    eprintln!(
        "12 kb/s mono speech: SILK WB {:.1} dB (seg {:.1}) at {:.1} kb/s vs CELT {:.1} dB (seg {:.1}) at {:.1} kb/s",
        silk.snr, silk.seg, silk.kbps, celt.snr, celt.seg, celt.kbps
    );
    assert!(silk.snr > celt.snr + 3.0, "SILK {:.1} dB vs CELT {:.1} dB", silk.snr, celt.snr);
    assert!(silk.seg > celt.seg + 3.0, "SILK seg {:.1} dB vs CELT seg {:.1} dB", silk.seg, celt.seg);
}

#[test]
fn hybrid_quality() {
    let sp = speech(LEN, 9);
    let mu = music(LEN);
    for (name, x, bw, rate, min_snr) in [
        ("speech", &sp, Bandwidth::SuperWideband, 24_000u32, 18.0),
        ("speech", &sp, Bandwidth::Fullband, 32_000, 20.0),
        ("music", &mu, Bandwidth::SuperWideband, 24_000, 9.0),
        ("music", &mu, Bandwidth::Fullband, 32_000, 10.0),
    ] {
        let r = mono(Mode::Hybrid, bw, rate, x, 24_000.0);
        eprintln!("hybrid {bw:?} {name} @ {} kb/s: {:.1} kb/s, SNR {:.1} dB, segSNR {:.1} dB", rate / 1000, r.kbps, r.snr, r.seg);
        assert!(r.snr > min_snr, "hybrid {bw:?} {name}: SNR {:.1} dB", r.snr);
        // Constant packet size.
        assert!(r.packets.iter().all(|p| p.len() == rate as usize / 400));
    }
}

#[test]
fn silk_music_quality() {
    let x = music(LEN);
    for (rate, min_snr) in [(12_000u32, 5.0), (16_000, 8.0), (24_000, 13.0)] {
        let r = mono(Mode::Silk, Bandwidth::Wideband, rate, &x, 7_000.0);
        eprintln!("SILK WB music @ {} kb/s: {:.1} kb/s, SNR {:.1} dB", rate / 1000, r.kbps, r.snr);
        assert!(r.snr > min_snr, "SILK music @ {rate}: SNR {:.1} dB", r.snr);
    }
}

#[test]
fn bitrate_tracking() {
    let sp = speech(LEN, 13);
    let mu = music(LEN);
    for (name, x) in [("speech", &sp), ("music", &mu)] {
        for (mode, bw, rate) in [
            (Mode::Silk, Bandwidth::Narrowband, 6_000u32),
            (Mode::Silk, Bandwidth::Narrowband, 9_000),
            (Mode::Silk, Bandwidth::Mediumband, 11_000),
            (Mode::Silk, Bandwidth::Wideband, 12_000),
            (Mode::Silk, Bandwidth::Wideband, 20_000),
            (Mode::Silk, Bandwidth::Wideband, 32_000),
            (Mode::Hybrid, Bandwidth::SuperWideband, 20_000),
            (Mode::Hybrid, Bandwidth::Fullband, 36_000),
        ] {
            let r = mono(mode, bw, rate, x, band_edge(bw));
            let ratio = r.kbps * 1000.0 / rate as f64;
            eprintln!("{name} {mode:?}/{bw:?} target {} kb/s: actual {:.2} kb/s ({:+.0} %), SNR {:.1} dB", rate / 1000, r.kbps, (ratio - 1.0) * 100.0, r.snr);
            assert!((0.75..=1.25).contains(&ratio), "{name} {mode:?}/{bw:?} @ {rate}: {:.2} kb/s", r.kbps);
        }
    }
}

#[test]
fn stereo_silk_and_hybrid() {
    let sp = speech(LEN + 40_000, 21);
    let a = sp[..LEN].to_vec();
    let b: Vec<f32> = sp[40_000..].iter().map(|v| v * 0.7).collect();
    let mu = music(LEN);
    let cases: Vec<(&str, Vec<f32>, Vec<f32>)> = vec![
        ("dual mono", a.clone(), a.clone()),
        ("panned", a.iter().map(|v| v * 0.9).collect(), a.iter().map(|v| v * 0.35).collect()),
        ("independent", a.clone(), b),
        ("speech+music", a.iter().zip(&mu).map(|(x, y)| 0.8 * x + 0.3 * y).collect(), a.iter().zip(&mu).map(|(x, y)| 0.3 * x + 0.8 * y).collect()),
    ];
    for (name, l, r) in &cases {
        for (mode, bw, rate, min_snr) in [(Mode::Silk, Bandwidth::Wideband, 32_000u32, 9.0), (Mode::Hybrid, Bandwidth::Fullband, 64_000, 9.0)] {
            let fc = band_edge(bw);
            let res = run(OpusEncoder::with_mode(2, rate, mode, bw), &[l.clone(), r.clone()], &[lowpass(l, fc), lowpass(r, fc)]);
            eprintln!("stereo {name} {mode:?}/{bw:?} @ {} kb/s: {:.1} kb/s, L {:.1} dB, R {:.1} dB", rate / 1000, res[0].kbps, res[0].snr, res[1].snr);
            assert!(res[0].snr > min_snr && res[1].snr > min_snr, "stereo {name} {mode:?}: L {:.1} R {:.1}", res[0].snr, res[1].snr);
        }
    }
}

#[test]
fn application_tables_and_pre_skip() {
    let cases = [
        (Application::Voip, 1u8, 8_000u32, Mode::Silk, Bandwidth::Narrowband),
        (Application::Voip, 1, 11_000, Mode::Silk, Bandwidth::Mediumband),
        (Application::Voip, 1, 16_000, Mode::Silk, Bandwidth::Wideband),
        (Application::Voip, 1, 24_000, Mode::Hybrid, Bandwidth::SuperWideband),
        (Application::Voip, 1, 32_000, Mode::Hybrid, Bandwidth::Fullband),
        (Application::Voip, 1, 48_000, Mode::Celt, Bandwidth::Fullband),
        (Application::Voip, 2, 32_000, Mode::Silk, Bandwidth::Wideband),
        (Application::Audio, 1, 12_000, Mode::Silk, Bandwidth::Wideband),
        (Application::Audio, 1, 24_000, Mode::Hybrid, Bandwidth::Fullband),
        (Application::Audio, 2, 48_000, Mode::Hybrid, Bandwidth::Fullband),
        (Application::Audio, 1, 64_000, Mode::Celt, Bandwidth::Fullband),
        (Application::Audio, 2, 128_000, Mode::Celt, Bandwidth::Fullband),
    ];
    for (app, c, rate, mode, bw) in cases {
        let e = OpusEncoder::with_application(c, rate, app);
        assert_eq!((e.mode(), e.bandwidth(), e.application()), (mode, bw, app), "{app:?} {c} ch @ {rate}");
        assert_eq!(e.pre_skip(), 120, "every mode is aligned to a 120-sample delay");
    }
    assert_eq!(OpusEncoder::new(2, 128_000).mode(), Mode::Celt);
    assert_eq!(OpusEncoder::new(1, 12_000).application(), Application::Audio);
}

#[test]
#[should_panic(expected = "unsupported mode/bandwidth")]
fn invalid_mode_bandwidth_panics() {
    let _ = OpusEncoder::with_mode(1, 16_000, Mode::Silk, Bandwidth::Fullband);
}

#[test]
fn extreme_inputs_stay_decodable() {
    let len = 24_000;
    let mut s = 1u32;
    let noise: Vec<f32> = (0..len)
        .map(|_| {
            s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (s >> 8) as f32 / (1u32 << 23) as f32 - 1.0
        })
        .collect();
    let square: Vec<f32> = (0..len).map(|i| if (i / 60) % 2 == 0 { 0.999 } else { -0.999 }).collect();
    let clicks: Vec<f32> = (0..len).map(|i| if i % 4_801 == 0 { 1.0 } else { 0.0 }).collect();
    let dc = vec![0.7f32; len];
    let nan: Vec<f32> = (0..len).map(|i| if i % 1000 == 0 { f32::NAN } else { 0.1 * (i as f32 * 0.01).sin() }).collect();
    for (name, x) in [("noise", &noise), ("square", &square), ("clicks", &clicks), ("dc", &dc), ("nan", &nan)] {
        for (mode, bw, rate) in
            [(Mode::Silk, Bandwidth::Narrowband, 6_000u32), (Mode::Silk, Bandwidth::Wideband, 40_000), (Mode::Hybrid, Bandwidth::Fullband, 24_000)]
        {
            for c in [1usize, 2] {
                let planar: Vec<Vec<f32>> = if c == 1 { vec![x.clone()] } else { vec![x.clone(), x.iter().map(|v| -0.5 * v).collect()] };
                let (enc, packets) = encode(OpusEncoder::with_mode(c as u8, rate * c as u32, mode, bw), &planar);
                assert!(packets.iter().all(|p| p.len() >= 3 && p.len() <= 1276), "{name} {mode:?}: packet sizes");
                let dec = decode(&enc.opus_head(), &packets);
                let peak = dec.iter().flatten().fold(0f32, |m, v| m.max(v.abs()));
                let snr = if name == "nan" { f64::NAN } else { snr_search(&planar[0], &dec[0], 2).0 };
                eprintln!("{name} {mode:?}/{bw:?} {c} ch: peak {peak:.2}, SNR {snr:.1} dB, {:.1} kb/s", bitrate(&packets) / 1000.0);
                assert!(dec.iter().all(|ch| ch.iter().all(|v| v.is_finite() && v.abs() <= 4.0)), "{name} {mode:?}/{bw:?} {c} ch: bad output");
            }
        }
    }
}

#[test]
fn silk_and_hybrid_silence() {
    for (mode, bw, rate) in [(Mode::Silk, Bandwidth::Wideband, 16_000u32), (Mode::Hybrid, Bandwidth::Fullband, 32_000)] {
        let x = vec![0f32; 48_000];
        let (enc, packets) = encode(OpusEncoder::with_mode(1, rate, mode, bw), std::slice::from_ref(&x));
        let dec = decode(&enc.opus_head(), &packets);
        let peak = dec[0].iter().fold(0f32, |m, v| m.max(v.abs()));
        let kbps = bitrate(&packets) / 1000.0;
        eprintln!("{mode:?} silence: {kbps:.2} kb/s, decoded peak {peak:.2e}");
        assert!(peak < 1e-3, "{mode:?} silence decodes to peak {peak}");
        if mode == Mode::Silk {
            assert!(kbps < 4.0, "SILK silence costs {kbps:.2} kb/s");
        }
    }
}

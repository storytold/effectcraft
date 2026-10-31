//! Round trips through FilmCraft's spec-derived AV1 decoder: decoded frames must equal the
//! encoder's own reconstruction bit for bit.

use crate::common;

use common::*;
use effectcraft_av1enc::{EncoderConfig, RateControl};

fn cfg(w: u32, h: u32, bd: u8, q: u8, keyint: u32) -> EncoderConfig {
    let mut c = EncoderConfig::new(w, h, 30, 1);
    c.bit_depth = bd;
    c.rate = RateControl::ConstantQ(q);
    c.keyint = keyint;
    c
}

#[test]
fn intra_8bit_smooth_high_quality() {
    let pics = vec![synth(128, 96, 8, 0)];
    let run = encode_and_check(cfg(128, 96, 8, 40, 1), &pics);
    eprintln!("intra 8-bit q40: {} bytes, PSNR-Y {:.2} dB", run.packets[0].data.len(), run.psnr_y[0]);
    assert!(run.packets[0].keyframe);
    assert!(run.psnr_y[0] > 35.0, "PSNR {:.2}", run.psnr_y[0]);
}

#[test]
fn odd_sizes_all_intra() {
    for (w, h) in [(33, 17), (1, 1), (7, 9), (65, 63), (100, 30)] {
        let pics: Vec<Pic> = (0..2).map(|t| synth(w, h, 8, t)).collect();
        let run = encode_and_check(cfg(w as u32, h as u32, 8, 90, 1), &pics);
        assert!(run.packets.iter().all(|p| p.keyframe));
    }
}

#[test]
fn noise_low_q_exact() {
    let pics = vec![noise(72, 40, 8, 1)];
    let run = encode_and_check(cfg(72, 40, 8, 1, 1), &pics);
    assert!(run.psnr_y[0] > 40.0, "PSNR {:.2}", run.psnr_y[0]);
}

#[test]
fn inter_moving_pattern() {
    let pics: Vec<Pic> = (0..6).map(|t| synth(160, 112, 8, t)).collect();
    let run = encode_and_check(cfg(160, 112, 8, 80, 30), &pics);
    let sizes: Vec<usize> = run.packets.iter().map(|p| p.data.len()).collect();
    eprintln!("inter sizes {sizes:?} psnr {:?}", run.psnr_y);
    assert!(run.packets[0].keyframe && run.packets[1..].iter().all(|p| !p.keyframe));
    let inter_avg = sizes[1..].iter().sum::<usize>() as f64 / (sizes.len() - 1) as f64;
    assert!(inter_avg < sizes[0] as f64 * 0.5, "inter frames {inter_avg} vs key {}", sizes[0]);
    assert!(run.psnr_y.iter().all(|&p| p > 33.0));
}

#[test]
fn ten_bit_inter() {
    let pics: Vec<Pic> = (0..4).map(|t| synth(96, 72, 10, t)).collect();
    let run = encode_and_check(cfg(96, 72, 10, 60, 3), &pics);
    eprintln!("10-bit psnr {:?}", run.psnr_y);
    assert!(run.psnr_y.iter().all(|&p| p > 35.0));
}

#[test]
fn odd_size_inter() {
    let pics: Vec<Pic> = (0..5).map(|t| synth(57, 43, 8, t)).collect();
    encode_and_check(cfg(57, 43, 8, 120, 10), &pics);
    let pics: Vec<Pic> = (0..3).map(|t| synth(33, 17, 10, t)).collect();
    encode_and_check(cfg(33, 17, 10, 120, 10), &pics);
}

#[test]
fn bitrate_mode_tracks_target() {
    let n = 12;
    let pics: Vec<Pic> = (0..n).map(|t| synth(192, 128, 8, t)).collect();
    let mut c = cfg(192, 128, 8, 0, 12);
    c.rate = RateControl::Bitrate { kbps: 400 };
    let run = encode_and_check(c, &pics);
    let bits: usize = run.packets.iter().map(|p| p.data.len() * 8).sum();
    let kbps = bits as f64 / (n as f64 / 30.0) / 1000.0;
    eprintln!("bitrate mode: {kbps:.1} kbit/s for a 400 kbit/s target, psnr {:?}", run.psnr_y);
    assert!(kbps > 400.0 / 2.0 && kbps < 400.0 * 2.0, "{kbps} kbit/s");
}

/// Over several key frame intervals the average rate stays within 5 % of the target, for a
/// range of targets (the key frame budget is taken out of the inter frames of its group).
#[test]
fn bitrate_mode_is_accurate_over_many_groups() {
    let n = 180;
    for (kbps, keyint, bd) in [(40, 30, 8), (100, 30, 8), (200, 60, 8), (150, 20, 10)] {
        let pics: Vec<Pic> = (0..n).map(|t| synth(192, 128, bd, t)).collect();
        let mut c = cfg(192, 128, bd as u8, 0, keyint);
        c.rate = RateControl::Bitrate { kbps };
        let mut enc = effectcraft_av1enc::Encoder::new(c).unwrap();
        let bits: usize = pics.iter().map(|p| enc.encode(&p.frame()).data.len() * 8).sum();
        let got = bits as f64 / (n as f64 / 30.0) / 1000.0;
        eprintln!("{bd} bit, keyint {keyint}: {got:.1} kbit/s for {kbps}");
        assert!((got / kbps as f64 - 1.0).abs() < 0.05, "{got:.1} kbit/s for a {kbps} kbit/s target ({bd} bit, keyint {keyint})");
    }
}

#[test]
fn headers() {
    let enc = effectcraft_av1enc::Encoder::new(cfg(1920, 1080, 10, 100, 60)).unwrap();
    let c = enc.av1c();
    assert_eq!(c[0], 0x81);
    assert_eq!(c[1] & 0x1f, 8, "1080p30 is level 4.0");
    assert_eq!(c[2], 0b0100_1100);
    assert_eq!(&c[4..], &enc.sequence_header_obu()[..]);
    assert_eq!(enc.sequence_header_obu()[0], 0x0A);
}

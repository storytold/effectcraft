//! External oracle: streams decoded by ffmpeg's libdav1d must equal the encoder's reconstruction
//! bit for bit. Skipped when `ffmpeg` (with libdav1d) is not installed. ffmpeg's native `av1`
//! decoder only drives hardware decoders, so it is not used.

use crate::common;

use std::process::Command;

use common::*;
use effectcraft_av1enc::{Encoder, EncoderConfig, Packet, RateControl};

fn have_libdav1d() -> bool {
    let Ok(out) = Command::new("ffmpeg").args(["-hide_banner", "-decoders"]).output() else {
        eprintln!("ffmpeg not found: skipping the external oracle");
        return false;
    };
    let ok = out.status.success() && String::from_utf8_lossy(&out.stdout).contains("libdav1d");
    if !ok {
        eprintln!("ffmpeg without libdav1d: skipping the external oracle");
    }
    ok
}

/// IVF container with one temporal unit (temporal delimiter + packet) per frame.
fn ivf(w: usize, h: usize, packets: &[Packet]) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(b"DKIF");
    v.extend_from_slice(&0u16.to_le_bytes());
    v.extend_from_slice(&32u16.to_le_bytes());
    v.extend_from_slice(b"AV01");
    v.extend_from_slice(&(w as u16).to_le_bytes());
    v.extend_from_slice(&(h as u16).to_le_bytes());
    v.extend_from_slice(&30u32.to_le_bytes());
    v.extend_from_slice(&1u32.to_le_bytes());
    v.extend_from_slice(&(packets.len() as u32).to_le_bytes());
    v.extend_from_slice(&0u32.to_le_bytes());
    for (i, p) in packets.iter().enumerate() {
        v.extend_from_slice(&((p.data.len() + 2) as u32).to_le_bytes());
        v.extend_from_slice(&(i as u64).to_le_bytes());
        v.extend_from_slice(&[0x12, 0x00]); // OBU_TEMPORAL_DELIMITER
        v.extend_from_slice(&p.data);
    }
    v
}

/// Encodes `pics`, decodes the IVF stream with `ffmpeg -c:v libdav1d` and compares every
/// decoded frame with the encoder's reconstruction.
/// Returns the loop filter levels used.
fn check(name: &str, cfg: EncoderConfig, pics: &[Pic]) -> Vec<u32> {
    let (w, h, bd) = (cfg.width as usize, cfg.height as usize, cfg.bit_depth);
    let mut enc = Encoder::new(cfg).expect("config");
    let mut packets = Vec::new();
    let mut recons = Vec::new();
    let mut levels = Vec::new();
    for p in pics {
        packets.push(enc.encode(&p.frame()));
        recons.push(enc.last_reconstruction().expect("reconstruction"));
        levels.push(enc.last_loop_filter_level().expect("level"));
    }
    let dir = std::env::temp_dir().join(format!("effectcraft-av1enc-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let input = dir.join("in.ivf");
    let output = dir.join("out.yuv");
    std::fs::write(&input, ivf(w, h, &packets)).expect("write ivf");
    let pix_fmt = if bd == 8 { "yuv420p" } else { "yuv420p10le" };
    let res = Command::new("ffmpeg")
        .args(["-v", "error", "-y", "-c:v", "libdav1d", "-i"])
        .arg(&input)
        .args(["-f", "rawvideo", "-pix_fmt", pix_fmt])
        .arg(&output)
        .output()
        .expect("run ffmpeg");
    let stderr = String::from_utf8_lossy(&res.stderr);
    assert!(res.status.success() && stderr.trim().is_empty(), "{name}: ffmpeg -c:v libdav1d failed: {stderr}");
    let raw = std::fs::read(&output).expect("decoded yuv");
    let _ = std::fs::remove_dir_all(&dir);
    let samples: Vec<u16> =
        if bd == 8 { raw.iter().map(|&b| b as u16).collect() } else { raw.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes([c[0], c[1]])).collect() };
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let fsize = w * h + 2 * cw * ch;
    assert_eq!(samples.len(), fsize * pics.len(), "{name}: decoded frame count");
    for (i, rec) in recons.iter().enumerate() {
        let f = &samples[i * fsize..(i + 1) * fsize];
        let planes = [&f[..w * h], &f[w * h..w * h + cw * ch], &f[w * h + cw * ch..]];
        for p in 0..3 {
            if planes[p] != &rec[p][..] {
                let pw = if p == 0 { w } else { cw };
                let k = planes[p].iter().zip(&rec[p]).position(|(a, b)| a != b).unwrap_or(0);
                panic!("{name}: frame {i} plane {p} differs from the reconstruction at ({}, {})", k % pw, k / pw);
            }
        }
    }
    eprintln!(
        "{name}: libdav1d bit-exact over {} frames ({} bytes, loop filter levels {levels:?})",
        pics.len(),
        packets.iter().map(|p| p.data.len()).sum::<usize>()
    );
    levels
}

fn cfg(w: usize, h: usize, bd: u8, rate: RateControl, keyint: u32) -> EncoderConfig {
    let mut c = EncoderConfig::new(w as u32, h as u32, 30, 1);
    c.bit_depth = bd;
    c.rate = rate;
    c.keyint = keyint;
    c
}

#[test]
fn libdav1d_decodes_bit_exact() {
    if !have_libdav1d() {
        return;
    }
    // a moving pattern with inter frames
    let pics: Vec<Pic> = (0..8).map(|t| synth(176, 144, 8, t)).collect();
    check("inter8", cfg(176, 144, 8, RateControl::ConstantQ(90), 5), &pics);
    // all-intra at a fine quantizer (large coefficients, Golomb codes)
    let pics: Vec<Pic> = (0..2).map(|t| noise(64, 48, 8, t as u32 + 3)).collect();
    check("noise-intra", cfg(64, 48, 8, RateControl::ConstantQ(4), 1), &pics);
    // the finest quantizer: the largest coefficients, 8 and 10 bit, intra and inter
    let pics: Vec<Pic> = (0..3).map(|t| noise(72, 40, 8, t as u32 + 11)).collect();
    check("noise-q1", cfg(72, 40, 8, RateControl::ConstantQ(1), 2), &pics);
    let pics: Vec<Pic> = (0..3).map(|t| noise(40, 72, 10, t as u32 + 21)).collect();
    check("noise-q1-10", cfg(40, 72, 10, RateControl::ConstantQ(1), 2), &pics);
    // odd sizes, 8 and 10 bit
    let pics: Vec<Pic> = (0..4).map(|t| synth(33, 17, 8, t)).collect();
    check("odd8", cfg(33, 17, 8, RateControl::ConstantQ(60), 3), &pics);
    let pics: Vec<Pic> = (0..5).map(|t| synth(101, 75, 10, t)).collect();
    check("odd10", cfg(101, 75, 10, RateControl::ConstantQ(110), 4), &pics);
    // a noisy moving picture, bitrate mode
    let pics: Vec<Pic> = (0..6).map(|t| noisy(128, 80, t)).collect();
    let levels = check("bitrate", cfg(128, 80, 8, RateControl::Bitrate { kbps: 300 }, 10), &pics);
    assert!(levels.iter().any(|&l| l > 0), "the deblocking filter is exercised");
    // a coarse quantizer: strong deblocking, 10 bit
    let pics: Vec<Pic> = (0..4).map(|t| synth(120, 88, 10, t)).collect();
    let levels = check("deblock10", cfg(120, 88, 10, RateControl::ConstantQ(220), 2), &pics);
    assert!(levels.iter().any(|&l| l > 0), "the deblocking filter is exercised");
}

#[test]
fn libdav1d_decodes_1080p() {
    if !have_libdav1d() {
        return;
    }
    let pics: Vec<Pic> = (0..3).map(|t| noisy(1920, 1080, t)).collect();
    check("1080p", cfg(1920, 1080, 8, RateControl::ConstantQ(120), 30), &pics);
}

fn noisy(w: usize, h: usize, t: usize) -> Pic {
    let mut p = synth(w, h, 8, t);
    let n = noise(w, h, 8, 7 + t as u32);
    for (a, b) in p.y.iter_mut().zip(&n.y) {
        *a = (*a as i32 + (*b as i32 - 128) / 16).clamp(0, 255) as u16;
    }
    p
}

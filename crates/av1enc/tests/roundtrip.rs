//! Round trips through FilmCraft's spec-derived AV1 decoder: decoded frames must equal the
//! encoder's own reconstruction bit for bit.

mod common;

use common::*;
use effectcraft_av1enc::{Encoder, EncoderConfig, RateControl};

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

/// Several tile columns and rows, with a partial last column and row: tiles reset their
/// contexts and neighbours stop at tile borders, in key and inter frames, 8 and 10 bit.
#[test]
fn tiles_columns_and_rows() {
    // 5x4 superblocks: columns of 2, 2 and 1 superblocks, rows of 2 and 2
    let pics: Vec<Pic> = (0..4).map(|t| synth(320, 200, 8, t)).collect();
    let run = encode_and_check_with(Encoder::with_tiles(cfg(320, 200, 8, 90, 30), 2, 1).unwrap(), &pics);
    eprintln!("6 tiles: sizes {:?} psnr {:?}", run.packets.iter().map(|p| p.data.len()).collect::<Vec<_>>(), run.psnr_y);
    // one tile per superblock, odd edges
    let pics: Vec<Pic> = (0..3).map(|t| synth(250, 150, 10, t)).collect();
    encode_and_check_with(Encoder::with_tiles(cfg(250, 150, 10, 110, 30), 3, 2).unwrap(), &pics);
    // rows only, and columns only at a low quantizer (large tiles, two-byte tile sizes)
    let pics: Vec<Pic> = (0..2).map(|t| synth(136, 200, 8, t)).collect();
    encode_and_check_with(Encoder::with_tiles(cfg(136, 200, 8, 120, 30), 0, 2).unwrap(), &pics);
    let pics = vec![noise(200, 72, 8, 5)];
    let run = encode_and_check_with(Encoder::with_tiles(cfg(200, 72, 8, 10, 1), 2, 0).unwrap(), &pics);
    assert!(run.packets[0].data.len() > 4 * 512, "tile sizes over one byte");
}

/// Frames wider than MAX_TILE_WIDTH (4096) need several tile columns; this width also gets a
/// partial last column (66 superblocks: 17, 17, 17 and 15).
#[test]
fn wider_than_4096() {
    let pics: Vec<Pic> = (0..3).map(|t| synth(4200, 72, 8, t)).collect();
    let run = encode_and_check(cfg(4200, 72, 8, 100, 30), &pics);
    eprintln!("4200x72: sizes {:?} psnr {:?}", run.packets.iter().map(|p| p.data.len()).collect::<Vec<_>>(), run.psnr_y);
    assert!(run.psnr_y.iter().all(|&p| p > 33.0));
}

/// Over MAX_TILE_AREA in one column (30x77 superblocks): two tile rows.
#[test]
fn tile_rows_from_area() {
    let pics: Vec<Pic> = (0..2).map(|t| synth(1920, 4928, 8, t)).collect();
    encode_and_check(cfg(1920, 4928, 8, 160, 30), &pics);
}

/// The tile layout depends on the frame size only: the bytes are the same whatever the
/// number of threads.
#[test]
fn tiles_independent_of_threads() {
    let pics: Vec<Pic> = (0..3).map(|t| synth(2304, 96, 8, t)).collect();
    let enc = |threads: usize| {
        let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).build().unwrap();
        pool.install(|| {
            let mut e = Encoder::new(cfg(2304, 96, 8, 90, 30)).unwrap();
            pics.iter().map(|p| e.encode(&p.frame()).data).collect::<Vec<_>>()
        })
    };
    assert_eq!(enc(1), enc(4));
}

/// FNV-1a over every packet (length then bytes).
fn stream_hash(run: &common::Run) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for p in &run.packets {
        for b in (p.data.len() as u64).to_le_bytes().iter().chain(&p.data) {
            h = (h ^ *b as u64).wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    h
}

/// The encoder's output is pinned byte for byte: speed work (scratch buffers, parallel
/// superblock decisions) must not change a single bit. Clips with several superblock rows and
/// columns, odd edges, both bit depths, inter frames and the first-frame rate-control passes.
#[test]
fn bitstream_is_stable() {
    let mut got = Vec::new();
    let pics: Vec<Pic> = (0..2).map(|t| synth(250, 150, 8, t)).collect();
    got.push(("intra 250x150", stream_hash(&encode_and_check(cfg(250, 150, 8, 70, 1), &pics))));
    let pics: Vec<Pic> = (0..4).map(|t| synth(320, 200, 8, t)).collect();
    got.push(("inter 320x200", stream_hash(&encode_and_check(cfg(320, 200, 8, 90, 30), &pics))));
    let pics: Vec<Pic> = (0..3).map(|t| synth(200, 136, 10, t)).collect();
    got.push(("10-bit inter 200x136", stream_hash(&encode_and_check(cfg(200, 136, 10, 110, 30), &pics))));
    let pics: Vec<Pic> = (0..3).map(|t| synth(256, 192, 8, t)).collect();
    let mut c = cfg(256, 192, 8, 0, 30);
    c.rate = RateControl::Bitrate { kbps: 600 };
    got.push(("bitrate 256x192", stream_hash(&encode_and_check(c, &pics))));
    let pics = vec![noise(136, 72, 8, 3)];
    got.push(("noise 136x72", stream_hash(&encode_and_check(cfg(136, 72, 8, 20, 1), &pics))));
    let pics: Vec<Pic> = (0..3).map(|t| synth(320, 200, 8, t)).collect();
    got.push(("6 tiles 320x200", stream_hash(&encode_and_check_with(Encoder::with_tiles(cfg(320, 200, 8, 90, 30), 2, 1).unwrap(), &pics))));
    eprintln!("{got:#x?}");
    let want: [(&str, u64); 6] = [
        ("intra 250x150", 0x4448a919c11598ff),
        ("inter 320x200", 0x5f93a0b1f0a4f0d1),
        ("10-bit inter 200x136", 0xcf20671005e62b11),
        ("bitrate 256x192", 0x18911ac7b31d9906),
        ("noise 136x72", 0xa3634854a234f8ed),
        ("6 tiles 320x200", 0xdbb94bdf097c1752),
    ];
    assert_eq!(got, want);
}

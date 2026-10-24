//! Speed / efficiency measurements on ffmpeg-generated test sources (ignored by default):
//! `cargo test -p effectcraft-av1enc --test it bench:: -- --ignored --nocapture`.

use crate::common;

use std::process::Command;
use std::time::Instant;

use common::*;
use effectcraft_av1enc::{Encoder, EncoderConfig, RateControl};

/// Raw 8-bit 4:2:0 frames of an ffmpeg lavfi source, or None without ffmpeg.
fn lavfi(src: &str, w: usize, h: usize, frames: usize) -> Option<Vec<Pic>> {
    let out = std::env::temp_dir().join(format!("effectcraft-av1enc-bench-{}-{src}-{w}x{h}.yuv", std::process::id()));
    let st = Command::new("ffmpeg")
        .args(["-v", "error", "-y", "-f", "lavfi", "-i"])
        .arg(format!("{src}=size={w}x{h}:rate=30"))
        .args(["-frames:v", &frames.to_string(), "-pix_fmt", "yuv420p", "-f", "rawvideo"])
        .arg(&out)
        .status()
        .ok()?;
    if !st.success() {
        return None;
    }
    let raw = std::fs::read(&out).ok()?;
    let _ = std::fs::remove_file(&out);
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let fs = w * h + 2 * cw * ch;
    Some(
        raw.chunks_exact(fs)
            .map(|f| Pic {
                w,
                h,
                y: f[..w * h].iter().map(|&b| b as u16).collect(),
                u: f[w * h..w * h + cw * ch].iter().map(|&b| b as u16).collect(),
                v: f[w * h + cw * ch..].iter().map(|&b| b as u16).collect(),
            })
            .collect(),
    )
}

fn run(name: &str, pics: &[Pic], rate: RateControl, keyint: u32) {
    let (w, h) = (pics[0].w, pics[0].h);
    let mut cfg = EncoderConfig::new(w as u32, h as u32, 30, 1);
    cfg.rate = rate;
    cfg.keyint = keyint;
    let mut enc = Encoder::new(cfg).unwrap();
    let mut bytes = 0;
    let mut psnr_sum = 0.0;
    let mut t_key = 0.0;
    let mut t_inter = 0.0;
    let mut sizes = Vec::new();
    for p in pics {
        let t = Instant::now();
        let pkt = enc.encode(&p.frame());
        let dt = t.elapsed().as_secs_f64();
        if pkt.keyframe {
            t_key += dt;
        } else {
            t_inter += dt;
        }
        bytes += pkt.data.len();
        sizes.push(pkt.data.len());
        let rec = enc.last_reconstruction().unwrap();
        psnr_sum += psnr(&p.y, &rec[0], 8);
    }
    let n = pics.len() as f64;
    let n_key = pics.len().div_ceil(keyint as usize) as f64;
    eprintln!(
        "{name} {w}x{h} {rate:?}: {:.0} kbit/s, PSNR-Y {:.2} dB, key {:.2} s/frame, inter {:.2} s/frame, sizes {:?}",
        bytes as f64 * 8.0 * 30.0 / n / 1000.0,
        psnr_sum / n,
        t_key / n_key,
        if n > n_key { t_inter / (n - n_key) } else { 0.0 },
        &sizes[..sizes.len().min(6)]
    );
}

#[test]
#[ignore]
fn bench_lavfi() {
    if std::env::var("AV1ENC_BENCH_QUICK").is_err() {
        let Some(pics) = lavfi("testsrc2", 1920, 1080, 6) else {
            eprintln!("ffmpeg not available");
            return;
        };
        run("testsrc2", &pics, RateControl::ConstantQ(100), 30);
    }
    if let Some(pics) = lavfi("mandelbrot", 640, 360, 20) {
        for q in [40, 80, 120, 160] {
            run("mandelbrot", &pics, RateControl::ConstantQ(q), 30);
        }
        run("mandelbrot", &pics, RateControl::Bitrate { kbps: 1000 }, 30);
    }
}

#[test]
#[ignore]
fn bench_intra() {
    for src in ["mandelbrot", "sierpinski", "gradients"] {
        let Some(pics) = lavfi(src, 640, 360, 4) else {
            return;
        };
        for q in [40, 80, 120, 160] {
            run(&format!("{src}-intra"), &pics, RateControl::ConstantQ(q), 1);
        }
    }
}

#[test]
#[ignore]
fn bench_1080p_loop() {
    let Some(pics) = lavfi("mandelbrot", 1920, 1080, 4) else {
        return;
    };
    run("mandelbrot", &pics, RateControl::ConstantQ(120), 30);
    run("mandelbrot", &pics, RateControl::ConstantQ(120), 30);
}

#[test]
#[ignore]
fn bench_rate_control() {
    let Some(pics) = lavfi("mandelbrot", 480, 272, 60) else {
        eprintln!("ffmpeg not available");
        return;
    };
    for kbps in [200, 600, 2000] {
        run("mandelbrot", &pics, RateControl::Bitrate { kbps }, 30);
    }
    let Some(pics) = lavfi("testsrc2", 480, 272, 60) else {
        return;
    };
    for kbps in [300, 3000] {
        run("testsrc2", &pics, RateControl::Bitrate { kbps }, 60);
    }
}

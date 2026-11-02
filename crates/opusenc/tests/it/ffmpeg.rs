//! ffmpeg as a second, external oracle: every mode/bandwidth written as Ogg Opus must decode
//! without errors with both ffmpeg's native `opus` decoder and `libopus`, honour the pre-skip
//! and end granule, and reproduce the input. Skipped when ffmpeg is not installed.

use crate::common;

use std::process::Command;

use common::*;
use effectcraft_opusenc::{Bandwidth, Mode, OpusEncoder};

const FFMPEG: &str = "/opt/homebrew/bin/ffmpeg";

fn ffmpeg() -> Option<&'static str> {
    [FFMPEG, "ffmpeg"].into_iter().find(|exe| Command::new(exe).arg("-version").output().is_ok_and(|o| o.status.success()))
}

/// Decodes an Ogg Opus file with ffmpeg's decoder `codec`; returns interleaved f32 output.
fn ffmpeg_decode(exe: &str, codec: &str, file: &[u8], channels: usize, tag: &str) -> Option<Vec<f32>> {
    let has = Command::new(exe).args(["-hide_banner", "-decoders"]).output().ok()?;
    if !String::from_utf8_lossy(&has.stdout).lines().any(|l| l.split_whitespace().nth(1) == Some(codec)) {
        eprintln!("ffmpeg has no {codec} decoder; skipping it");
        return None;
    }
    let path = std::env::temp_dir().join(format!("effectcraft-opusenc-{}-{tag}.opus", std::process::id()));
    std::fs::write(&path, file).unwrap();
    let out = Command::new(exe)
        .args(["-v", "error", "-c:a", codec, "-i"])
        .arg(&path)
        .args(["-f", "f32le", "-ac", &channels.to_string(), "-ar", "48000", "-"])
        .output()
        .expect("run ffmpeg");
    let _ = std::fs::remove_file(&path);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "ffmpeg ({codec}) failed: {stderr}");
    // ffmpeg's native decoder also makes its raw-PCM muxer print a timestamp complaint for
    // narrowband SILK streams written by libopus itself; that line is about ffmpeg's own
    // timestamping, not the bitstream, so it is the one message tolerated.
    let errors: Vec<&str> = stderr.lines().filter(|l| !l.trim().is_empty() && !l.contains("non monotonically increasing dts to muxer")).collect();
    assert!(errors.is_empty(), "ffmpeg ({codec}) reported errors: {errors:?}");
    Some(out.stdout.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect())
}

#[test]
fn ffmpeg_decodes_every_mode() {
    let Some(exe) = ffmpeg() else {
        eprintln!("ffmpeg not installed; skipping the oracle test");
        return;
    };
    let len = 48_000;
    let sp = speech(len + 30_000, 3);
    let left = sp[..len].to_vec();
    let right: Vec<f32> = sp[30_000..].iter().map(|v| v * 0.7).collect();
    let cases = [
        (1usize, Mode::Silk, Bandwidth::Narrowband, 8_000u32, 3_400.0, 8.0),
        (1, Mode::Silk, Bandwidth::Mediumband, 11_000, 5_000.0, 10.0),
        (1, Mode::Silk, Bandwidth::Wideband, 16_000, 7_000.0, 12.0),
        (2, Mode::Silk, Bandwidth::Wideband, 32_000, 7_000.0, 12.0),
        (1, Mode::Hybrid, Bandwidth::SuperWideband, 24_000, 24_000.0, 10.0),
        (1, Mode::Hybrid, Bandwidth::Fullband, 32_000, 24_000.0, 10.0),
        (2, Mode::Hybrid, Bandwidth::Fullband, 64_000, 24_000.0, 10.0),
        (2, Mode::Celt, Bandwidth::Fullband, 96_000, 24_000.0, 10.0),
    ];
    for (c, mode, bw, rate, fc, min_snr) in cases {
        let planar: Vec<Vec<f32>> = if c == 1 { vec![left.clone()] } else { vec![left.clone(), right.clone()] };
        let (enc, packets) = encode(OpusEncoder::with_mode(c as u8, rate, mode, bw), &planar);
        let file = ogg_opus(&enc, &packets, len);
        for codec in ["opus", "libopus"] {
            let Some(dec) = ffmpeg_decode(exe, codec, &file, c, &format!("{mode:?}{bw:?}{c}{codec}")) else {
                continue;
            };
            assert_eq!(dec.len(), len * c, "{codec}: pre-skip and end granule must be honoured");
            for (ch, x) in planar.iter().enumerate() {
                let d: Vec<f32> = (0..len).map(|i| dec[i * c + ch]).collect();
                let (snr, delay) = snr_search(&lowpass(x, fc), &d, 6);
                eprintln!("ffmpeg {codec:7} {mode:?}/{bw:?} {c} ch @ {} kb/s, channel {ch}: SNR {snr:.1} dB (residual delay {delay})", rate / 1000);
                assert!(snr > min_snr, "{codec} {mode:?}/{bw:?}: SNR {snr:.1} dB");
            }
        }
    }
}

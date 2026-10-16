//! Opening a long movie reads its index, not the whole file: probing it and decoding a frame and
//! some audio must not grow peak memory by anything near the file's size.
//!
//! The large movie is a small ffmpeg-made MP4 followed by a 256 MiB `free` box (a sparse file, so
//! it costs no disk writes). Reading it whole, as before, grew peak RSS by its size; read in place,
//! the box is skipped by its header. The only test in this binary, so peak RSS is its own.
//! Skips (passes with a note) without ffmpeg or off Linux (peak RSS from `/proc/self/status`).

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use effectcraft_media::{MediaPool, probe};
use effectcraft_time::Tick;

const PADDING: u64 = 256 << 20;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/fixtures/media")
}

/// An H.264 encoder this ffmpeg build actually ships, or `None` when it has none (see the same
/// helper in `footage.rs`: distro "free" builds often omit `libx264` and ship an alternative).
fn h264_encoder() -> Option<&'static str> {
    static ENC: std::sync::OnceLock<Option<&'static str>> = std::sync::OnceLock::new();
    *ENC.get_or_init(|| {
        let out = Command::new("ffmpeg").args(["-hide_banner", "-encoders"]).output().ok()?;
        if !out.status.success() {
            return None;
        }
        let text = String::from_utf8_lossy(&out.stdout);
        ["libx264", "libopenh264", "h264_nvenc", "h264_vaapi", "h264_qsv", "h264_v4l2m2m", "h264_amf"]
            .into_iter()
            .find(|name| text.lines().any(|l| l.split_whitespace().any(|tok| tok == *name)))
    })
}

/// A 2 s 320×240 H.264 + AAC movie (written to a temp name and renamed, so concurrent test
/// processes never see a partial file). `None` without ffmpeg, or when this build cannot encode
/// H.264 at all — the test only measures how a movie is read, so it skips rather than asserting
/// on the packager's choice of ffmpeg.
fn small_mp4() -> Option<PathBuf> {
    let out = fixtures().join("in_place_small.mp4");
    if out.exists() {
        return Some(out);
    }
    if !Command::new("ffmpeg").arg("-version").output().is_ok_and(|o| o.status.success()) {
        return None;
    }
    let enc = h264_encoder()?;
    std::fs::create_dir_all(fixtures()).unwrap();
    let tmp = fixtures().join(format!("in_place_tmp_{}.mp4", std::process::id()));
    let st = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i", "testsrc2=size=320x240:rate=30:duration=2"])
        .args(["-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000:duration=2"])
        .args(["-c:v", enc, "-pix_fmt", "yuv420p", "-c:a", "aac", "-shortest"])
        .arg(&tmp)
        .status()
        .unwrap();
    if !st.success() {
        let _ = std::fs::remove_file(&tmp);
        eprintln!("this ffmpeg build cannot make in_place_small.mp4: skipping");
        return None;
    }
    std::fs::rename(&tmp, &out).unwrap();
    Some(out)
}

/// `/proc/self/status` field `key` in KiB.
fn status_kib(key: &str) -> Option<u64> {
    let s = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = s.lines().find(|l| l.starts_with(key))?;
    line.split_whitespace().nth(1)?.parse().ok()
}

/// Peak RSS so far in bytes, after resetting it to the current RSS where the kernel allows that.
fn reset_peak() -> Option<u64> {
    let _ = std::fs::write("/proc/self/clear_refs", "5");
    status_kib("VmHWM:").map(|k| k * 1024)
}

#[test]
fn a_long_movie_is_read_in_place_not_whole() {
    if !cfg!(target_os = "linux") {
        eprintln!("peak RSS is read from /proc: skipping");
        return;
    }
    let Some(small) = small_mp4() else {
        eprintln!("ffmpeg not found: skipping");
        return;
    };
    // The same movie with 256 MiB of padding after its boxes.
    let large = fixtures().join(format!("in_place_large_{}.mp4", std::process::id()));
    std::fs::copy(&small, &large).unwrap();
    let mut f = std::fs::OpenOptions::new().append(true).open(&large).unwrap();
    f.write_all(&(8 + PADDING as u32).to_be_bytes()).unwrap();
    f.write_all(b"free").unwrap();
    f.set_len(std::fs::metadata(&small).unwrap().len() + 8 + PADDING).unwrap();
    drop(f);

    // Warm up on the small file (code paths, allocator, decoder tables).
    let small_footage = probe(&small).unwrap();
    let pool = MediaPool::new();
    pool.set_read_ahead(false);
    let small_frame = pool.frame_at(&small_footage, Tick::ZERO).unwrap();
    let small_audio = pool.audio_samples(&small_footage, Tick::ZERO, 4800, 48_000);

    let before = reset_peak().unwrap();
    let footage = probe(&large).unwrap();
    let frame = pool.frame_at(&footage, Tick::ZERO).unwrap();
    let audio = pool.audio_samples(&footage, Tick::ZERO, 4800, 48_000);
    let after = status_kib("VmHWM:").unwrap() * 1024;
    drop(pool);
    let _ = std::fs::remove_file(&large);

    let growth = after.saturating_sub(before);
    eprintln!("peak RSS growth: {} KiB", growth >> 10);
    assert!(growth < 64 << 20, "probing and decoding a {} MiB movie grew peak RSS by {} MiB", PADDING >> 20, growth >> 20);
    // Same movie, same results.
    assert_eq!(footage, effectcraft_project::Footage { path: footage.path.clone(), ..small_footage });
    assert_eq!(frame, small_frame);
    assert_eq!(audio, small_audio);
    assert!(audio.iter().any(|s| s.abs() > 0.1), "the tone decodes");
}

//! Footage import against ffmpeg as an external oracle.
//!
//! Fixtures are generated with ffmpeg into `target/fixtures/media` (never committed); every test
//! skips (passes with a note) when ffmpeg is not installed. Decoded frames are compared with
//! frames ffmpeg extracts to PNG (PSNR, premultiplied RGBA).

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Instant;

use effectcraft_media::{MediaPool, probe, probe_bytes};
use effectcraft_project::{AlphaMode, Footage, FootageKind, ItemId};
use effectcraft_raster::Image;
use effectcraft_render::FootageSource;
use effectcraft_time::{FrameRate, TICKS_PER_SECOND, Tick};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/fixtures/media")
}

fn have_ffmpeg() -> bool {
    static HAVE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *HAVE.get_or_init(|| Command::new("ffmpeg").arg("-version").output().is_ok_and(|o| o.status.success()))
}

/// An H.264 encoder this ffmpeg build actually ships, or `None` when it has none. Distro "free"
/// builds often omit `libx264` (the one named historically here) but ship an alternative, so pick
/// the first present by name rather than hard-coding one that may be absent. These tests only
/// need *an* H.264 encoder to make an oracle fixture; the encoder identity is irrelevant.
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

/// Run ffmpeg to produce `name` (once; written to a temp name and renamed so concurrent test
/// processes never see a partial file). `None` when ffmpeg is unavailable.
fn ffmpeg_fixture(name: &str, args: &[&str]) -> Option<PathBuf> {
    ffmpeg_fixture_inner(name, args, true)
}

/// Like [`ffmpeg_fixture`] for a fixture this build of ffmpeg may fail to make: an encoder found
/// in the `-encoders` listing can still not run here (the hardware backends need their device,
/// Fedora's `libopenh264` is the `noopenh264` stub), and fixtures built from one, like
/// [`h264_mp4`], should skip with a note rather than fail on the packager's choice of ffmpeg —
/// the same way every test here skips when ffmpeg is missing altogether.
fn try_ffmpeg_fixture(name: &str, args: &[&str]) -> Option<PathBuf> {
    ffmpeg_fixture_inner(name, args, false)
}

fn ffmpeg_fixture_inner(name: &str, args: &[&str], must_succeed: bool) -> Option<PathBuf> {
    if !have_ffmpeg() {
        eprintln!("ffmpeg not found: skipping");
        return None;
    }
    let out = fixtures().join(name);
    if out.exists() {
        return Some(out);
    }
    std::fs::create_dir_all(out.parent().expect("parent")).expect("mkdir");
    let ext = out.extension().map(|e| e.to_string_lossy().to_string()).unwrap_or_default();
    let tmp = out.with_file_name(format!("tmp-{}-{:?}.{ext}", std::process::id(), std::thread::current().id()).replace(['(', ')'], ""));
    let st = Command::new("ffmpeg").args(["-hide_banner", "-loglevel", "error", "-y"]).args(args).arg(&tmp).status().expect("run ffmpeg");
    if !st.success() {
        let _ = std::fs::remove_file(&tmp);
        if must_succeed {
            panic!("ffmpeg failed for {name}");
        }
        eprintln!("this ffmpeg build cannot make {name}: skipping");
        return None;
    }
    if std::fs::rename(&tmp, &out).is_err() {
        // Another test made it first; on Windows the rename fails while that test has it open.
        let _ = std::fs::remove_file(&tmp);
        assert!(out.exists(), "rename {} to {name} failed", tmp.display());
    }
    Some(out)
}

/// Video options the chosen H.264 encoder actually accepts. `libx264` keeps its constant-quality
/// `-preset`/`-crf` pair; `libopenh264` rejects both (it takes a bitrate), and the hardware
/// backends take a bitrate too — without this, a build whose picked encoder is not libx264 failed
/// the fixture and silently skipped every check gated on it.
fn h264_video_options(enc: &str) -> Vec<&'static str> {
    if enc == "libx264" { vec!["-preset", "medium", "-crf", "18"] } else { vec!["-b:v", "2M"] }
}

fn h264_mp4() -> Option<PathBuf> {
    // Made with whatever encoder this build has listed: a listed encoder can still fail at run
    // time (the hardware ones without their device), so this may skip rather than fail.
    let enc = h264_encoder()?;
    let mut args: Vec<&'static str> = vec![
        "-f",
        "lavfi",
        "-i",
        "testsrc2=size=1920x1080:rate=30:duration=2",
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=440:sample_rate=48000:duration=2",
        "-c:v",
        enc,
        "-pix_fmt",
        "yuv420p",
    ];
    args.extend(h264_video_options(enc));
    args.extend(["-colorspace", "bt709", "-color_primaries", "bt709", "-color_trc", "bt709", "-c:a", "aac", "-ac", "2", "-shortest"]);
    try_ffmpeg_fixture("testsrc2_1080p.mp4", &args)
}

fn prores_mov() -> Option<PathBuf> {
    ffmpeg_fixture(
        "prores4444_alpha.mov",
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=640x360:rate=24:duration=1,format=rgba,geq=r='r(X,Y)':g='g(X,Y)':b='b(X,Y)':a='255*X/W'",
            "-c:v",
            "prores_ks",
            "-profile:v",
            "4444",
            "-pix_fmt",
            "yuva444p10le",
            "-colorspace",
            "bt709",
            "-color_primaries",
            "bt709",
            "-color_trc",
            "bt709",
        ],
    )
}

fn vp9_webm() -> Option<PathBuf> {
    ffmpeg_fixture(
        "testsrc2_vp9.webm",
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=640x360:rate=25:duration=1",
            "-c:v",
            "libvpx-vp9",
            "-pix_fmt",
            "yuv420p",
            "-crf",
            "20",
            "-b:v",
            "0",
            "-deadline",
            "good",
            "-cpu-used",
            "4",
            "-colorspace",
            "bt709",
            "-color_primaries",
            "bt709",
            "-color_trc",
            "bt709",
        ],
    )
}

fn png_sequence() -> Option<PathBuf> {
    // ffmpeg's image2 muxer writes seq_0001.png … seq_0010.png; generate into a temp dir and rename it.
    if !have_ffmpeg() {
        return None;
    }
    let dir = fixtures().join("seq");
    if !dir.join("seq_0010.png").exists() {
        let tmp = fixtures().join(format!("seq-tmp-{}-{:?}", std::process::id(), std::thread::current().id()).replace(['(', ')'], ""));
        std::fs::create_dir_all(&tmp).expect("mkdir");
        let st = Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i", "testsrc2=size=320x240:rate=30:duration=1", "-frames:v", "10"])
            .arg(tmp.join("seq_%04d.png"))
            .status()
            .expect("ffmpeg");
        assert!(st.success());
        if std::fs::rename(&tmp, &dir).is_err() {
            let _ = std::fs::remove_dir_all(&tmp); // another process won the race
        }
    }
    Some(dir.join("seq_0001.png"))
}

fn alpha_png() -> Option<PathBuf> {
    ffmpeg_fixture(
        "alpha.png",
        &["-f", "lavfi", "-i", "testsrc2=size=256x128:rate=1:duration=1,format=rgba,geq=r='r(X,Y)':g='g(X,Y)':b='b(X,Y)':a='255*Y/H'", "-frames:v", "1"],
    )
}

fn tone_wav() -> Option<PathBuf> {
    ffmpeg_fixture("tone.wav", &["-f", "lavfi", "-i", "sine=frequency=1000:sample_rate=48000:duration=1", "-ac", "2", "-c:a", "pcm_s16le"])
}

/// Frame `n` of `src` as RGBA PNG extracted by ffmpeg (Y'CbCr → RGB with the BT.709 matrix).
fn reference_frame(src: &Path, n: usize) -> Image {
    let stem = src.file_stem().expect("stem").to_string_lossy().to_string();
    let name = format!("ref_{stem}_{n}.png");
    let vf = format!("select='eq(n\\,{n})',scale=in_color_matrix=bt709:in_range=auto:out_range=pc:flags=bilinear+accurate_rnd+full_chroma_int,format=rgba");
    let p = ffmpeg_fixture(&name, &["-i", src.to_str().expect("utf8"), "-vf", &vf, "-fps_mode", "passthrough", "-frames:v", "1"]).expect("ffmpeg");
    load_png(&p)
}

/// A PNG as a premultiplied image (straight alpha in the file).
fn load_png(p: &Path) -> Image {
    let img = image::open(p).expect("png").to_rgba8();
    let (w, h) = img.dimensions();
    let data = img
        .pixels()
        .map(|p| {
            let a = p[3] as f32 / 255.0;
            [p[0] as f32 / 255.0 * a, p[1] as f32 / 255.0 * a, p[2] as f32 / 255.0 * a, a]
        })
        .collect();
    Image { width: w, height: h, data }
}

fn psnr(a: &Image, b: &Image) -> f64 {
    assert_eq!((a.width, a.height), (b.width, b.height), "size");
    let mut se = 0f64;
    for (p, q) in a.data.iter().zip(&b.data) {
        for k in 0..4 {
            let d = (p[k].clamp(0.0, 1.0) - q[k]) as f64;
            se += d * d;
        }
    }
    let mse = se / (a.data.len() * 4) as f64;
    if mse == 0.0 { f64::INFINITY } else { 10.0 * (1.0 / mse).log10() }
}

fn t_of(rate: FrameRate, n: i64) -> Tick {
    rate.tick_of(n)
}

#[test]
fn h264_mp4_probe_and_frames() {
    let Some(p) = h264_mp4() else { return };
    let f = probe(&p).expect("probe");
    assert_eq!(f.kind, FootageKind::Video);
    assert_eq!((f.width, f.height), (1920, 1080));
    assert_eq!(f.frame_rate, FrameRate::FPS_30);
    assert!((f.duration.seconds() - 2.0).abs() < 0.1, "duration {:?}", f.duration.seconds());
    assert!(f.has_video && f.has_audio);
    assert_eq!(f.alpha, AlphaMode::Ignore);
    assert!(!f.codec.is_empty());
    eprintln!("h264: codec {:?}", f.codec);
    let pool = MediaPool::new();
    // sequential, then a backwards seek and a forward jump into the GOP
    for n in [0usize, 1, 2, 3, 45, 10, 59] {
        let img = pool.frame(ItemId(1), &f, t_of(f.frame_rate, n as i64)).expect("frame");
        let db = psnr(&img, &reference_frame(&p, n));
        eprintln!("h264 frame {n}: {db:.2} dB");
        assert!(db > 35.0, "frame {n}: {db:.2} dB");
    }
    // clamped past the end: last frame
    let last = pool.frame_at(&f, Tick(10 * TICKS_PER_SECOND)).expect("frame");
    let db = psnr(&last, &reference_frame(&p, 59));
    assert!(db > 35.0, "clamp: {db:.2} dB");
    // cached
    let a = pool.frame_at(&f, t_of(f.frame_rate, 45)).expect("frame");
    let b = pool.frame_at(&f, t_of(f.frame_rate, 45)).expect("frame");
    assert!(Arc::ptr_eq(&a, &b));
}

/// A reduced-resolution render's movie frames are converted at its size: they match resampling
/// the full frames (testsrc2's saturated colours clip, where the two differ most), and are cached
/// per size.
#[test]
fn h264_frames_at_reduced_sizes() {
    let Some(p) = h264_mp4() else { return };
    let f = probe(&p).expect("probe");
    let pool = MediaPool::new();
    for (n, (w, h)) in [(0i64, (960, 540)), (1, (640, 360)), (2, (480, 270)), (30, (323, 182))] {
        let t = t_of(f.frame_rate, n);
        let sized = pool.frame_at_size(ItemId(1), &f, t, w, h).expect("frame");
        let full = pool.frame(ItemId(1), &f, t).expect("frame");
        assert_eq!((sized.width, sized.height), (w, h));
        let db = psnr(&sized, &effectcraft_raster::resample(&full, w, h));
        eprintln!("h264 frame {n} at {w}×{h}: {db:.2} dB against the resampled full frame");
        // Converting at half size averages before the YUV→RGB clip, so saturated pixels differ
        // slightly; how much depends on the encoder that made the fixture (50 dB with x264 from
        // ffmpeg 6.1, above 55 with newer ones), all well past visible.
        assert!(db > 45.0, "frame {n} at {w}×{h}: {db:.2} dB");
        assert!(Arc::ptr_eq(&sized, &pool.frame_at_size(ItemId(1), &f, t, w, h).expect("frame")), "cached");
    }
}

#[test]
fn h264_audio_samples() {
    let Some(p) = h264_mp4() else { return };
    let f = probe(&p).expect("probe");
    let pool = MediaPool::new();
    let s = pool.audio_samples(&f, Tick(TICKS_PER_SECOND / 2), 4800, 48_000);
    assert_eq!(s.len(), 9600);
    let rms = (s.iter().map(|v| v * v).sum::<f32>() / s.len() as f32).sqrt();
    // ffmpeg's sine source: −24 dBFS RMS
    assert!((rms - 0.0625).abs() < 0.01, "rms {rms}");
    // before the start: silence
    let pre = pool.audio_samples(&f, Tick(-TICKS_PER_SECOND), 100, 48_000);
    assert!(pre.iter().all(|v| *v == 0.0));
}

/// H.264 + AAC at 44.1 kHz in MP4 (ffmpeg writes an edit list that skips the AAC priming):
/// 440 Hz left, a 200 → 1000 Hz sweep right, 29.97 fps.
fn aac_44k_mp4() -> Option<PathBuf> {
    // Made with whatever encoder this build has listed: a listed encoder can still fail at run
    // time (the hardware ones without their device), so this may skip rather than fail.
    let enc = h264_encoder()?;
    try_ffmpeg_fixture(
        "aac_44k.mp4",
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=64x48:rate=30000/1001:duration=3",
            "-f",
            "lavfi",
            "-i",
            "aevalsrc=0.5*sin(2*PI*440*t)|0.5*sin(2*PI*(200+400*t/3)*t):s=44100:d=3",
            "-c:v",
            enc,
            "-pix_fmt",
            "yuv420p",
            "-c:a",
            "aac",
            "-shortest",
        ],
    )
}

/// `src`'s audio decoded by ffmpeg as interleaved stereo `f32` at `rate` Hz (external oracle).
fn ffmpeg_audio(src: &Path, rate: u32) -> Vec<f32> {
    let out = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-i"])
        .arg(src)
        .args(["-f", "f32le", "-ac", "2", "-ar", &rate.to_string(), "-"])
        .output()
        .expect("ffmpeg");
    assert!(out.status.success(), "ffmpeg failed to decode {}", src.display());
    out.stdout.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect()
}

/// Footage audio at a rate other than the file's, read in one-frame pieces as export and the
/// preview mix do, matches ffmpeg's decode: a resampled MP4 read ramped from a wrong position
/// at the start of every piece (peaks near 100 against a 0.5 tone), so AAC footage stuttered in
/// previews and exports (#274).
#[test]
fn resampled_audio_read_in_frame_pieces_matches_ffmpeg() {
    let (Some(a), Some(b)) = (aac_44k_mp4(), h264_mp4()) else { return };
    // 44.1 kHz read at 48 kHz, 48 kHz read at 44.1 kHz; NTSC frames are fractional in samples.
    for (p, rate) in [(a, 48_000u32), (b, 44_100)] {
        let f = probe(&p).expect("probe");
        let want = ffmpeg_audio(&p, rate);
        let fps = FrameRate::new(30000, 1001);
        let pool = MediaPool::new();
        let (mut got, mut cursor) = (Vec::new(), 0i64);
        for k in 1..=55 {
            let end = fps.tick_of(k).to_units_floor(rate as i64);
            got.extend(pool.audio_samples(&f, Tick::from_units(cursor, rate as i64), (end - cursor) as usize, rate));
            cursor = end;
        }
        let worst = got.iter().zip(&want).map(|(g, w)| (g - w).abs()).fold(0.0f32, f32::max);
        eprintln!("{} at {rate} Hz: max error {worst:.4}", p.display());
        assert!(want.len() >= got.len() && worst < 0.01, "{} at {rate} Hz: max error {worst}", p.display());
    }
}

/// The H.264 fixture with the rotation part of its video track header's matrix (`[a b; c d]`,
/// 16.16 fixed point) replaced: what a phone writes for a movie shot with the camera turned.
fn turned_mp4(p: &Path, abcd: [i32; 4]) -> Arc<[u8]> {
    let mut bytes = std::fs::read(p).expect("read");
    // (The video track comes first; a version 0 `tkhd` has its matrix 44 bytes after the type.)
    let at = bytes.windows(4).position(|w| w == b"tkhd").expect("tkhd");
    assert_eq!(bytes[at + 4], 0, "a version 0 tkhd");
    for (slot, v) in [0, 1, 3, 4].into_iter().zip(abcd) {
        let o = at + 44 + slot * 4;
        bytes[o..o + 4].copy_from_slice(&v.to_be_bytes());
    }
    bytes.into()
}

/// A movie whose track header says it is shown rotated (a phone held upright) imports the way it
/// is shown: the footage has the turned size and its frames are turned, at full and reduced size.
#[test]
fn rotated_mp4_imports_upright() {
    let Some(p) = h264_mp4() else { return };
    let plain_footage = probe(&p).expect("probe");
    let pool = MediaPool::new();
    let t = t_of(plain_footage.frame_rate, 3);
    let plain = pool.frame(ItemId(1), &plain_footage, t).expect("frame");
    let (w, h) = (plain.width as usize, plain.height as usize);
    let one = 0x1_0000;
    // The matrix, the turned size and the plain frame's pixel shown at (x, y).
    type From = fn(usize, usize, usize, usize) -> (usize, usize);
    let cases: [(&str, [i32; 4], (usize, usize), From); 3] = [
        ("90", [0, one, -one, 0], (h, w), |x, y, _, h| (y, h - 1 - x)),
        ("180", [-one, 0, 0, -one], (w, h), |x, y, w, h| (w - 1 - x, h - 1 - y)),
        ("270", [0, -one, one, 0], (h, w), |x, y, w, _| (w - 1 - y, x)),
    ];
    for (name, matrix, (tw, th), from) in cases {
        let path = format!("turned-{name}.mp4");
        let bytes = turned_mp4(&p, matrix);
        let f = probe_bytes(&path, bytes.clone()).expect("probe");
        assert_eq!((f.width as usize, f.height as usize), (tw, th), "{name}°: size");
        pool.add_bytes(&path, bytes);
        let img = pool.frame(ItemId(2), &f, t).expect("frame");
        assert_eq!((img.width as usize, img.height as usize), (tw, th), "{name}°: frame size");
        for (x, y) in [(0, 0), (tw - 1, 0), (0, th - 1), (tw - 1, th - 1), (tw / 3, th / 5), (tw / 2, th - 7)] {
            let (sx, sy) = from(x, y, w, h);
            assert_eq!(img.data[y * tw + x], plain.data[sy * w + sx], "{name}°: pixel ({x}, {y})");
        }
        // (A reduced-resolution render asks for the turned size.)
        let (hw, hh) = (f.width / 2, f.height / 2);
        let sized = pool.frame_at_size(ItemId(2), &f, t, hw, hh).expect("frame");
        assert_eq!((sized.width, sized.height), (hw, hh), "{name}°: reduced size");
        let db = psnr(&sized, &effectcraft_raster::resample(&img, hw, hh));
        assert!(db > 45.0, "{name}° at half size: {db:.2} dB");
    }
}

#[test]
fn prores_4444_with_alpha() {
    let Some(p) = prores_mov() else { return };
    let f = probe(&p).expect("probe");
    assert_eq!(f.kind, FootageKind::Video);
    assert_eq!((f.width, f.height), (640, 360));
    assert_eq!(f.frame_rate, FrameRate::FPS_24);
    assert!(!f.has_audio);
    assert_eq!(f.alpha, AlphaMode::Straight, "ProRes 4444 alpha detected");
    eprintln!("prores: codec {:?}", f.codec);
    let pool = MediaPool::new();
    for n in [0usize, 7, 23] {
        let img = pool.frame_at(&f, t_of(f.frame_rate, n as i64)).expect("frame");
        let db = psnr(&img, &reference_frame(&p, n));
        eprintln!("prores frame {n}: {db:.2} dB");
        assert!(db > 35.0, "frame {n}: {db:.2} dB");
    }
    // alpha ramps left to right
    let img = pool.frame_at(&f, Tick::ZERO).expect("frame");
    assert!(img.data[0][3] < 0.02 && img.data[639][3] > 0.97);
    // Ignore → opaque
    let mut g = f.clone();
    g.alpha = AlphaMode::Ignore;
    assert!(pool.frame_at(&g, Tick::ZERO).expect("frame").data.iter().all(|p| p[3] == 1.0));
}

#[test]
fn vp9_webm_frames() {
    let Some(p) = vp9_webm() else { return };
    let f = probe(&p).expect("probe");
    assert_eq!(f.kind, FootageKind::Video);
    assert_eq!((f.width, f.height), (640, 360));
    assert_eq!(f.frame_rate, FrameRate::FPS_25);
    eprintln!("vp9: codec {:?}", f.codec);
    let pool = MediaPool::new();
    for n in [0usize, 1, 12, 24, 5] {
        let img = pool.frame_at(&f, t_of(f.frame_rate, n as i64)).expect("frame");
        let db = psnr(&img, &reference_frame(&p, n));
        eprintln!("vp9 frame {n}: {db:.2} dB");
        assert!(db > 35.0, "frame {n}: {db:.2} dB");
    }
}

/// MXF OP1a as broadcast cameras write it: long-GOP MPEG-2 4:2:2 (XDCAM-like, B-frames) with
/// 24-bit PCM audio.
fn mpeg2_mxf() -> Option<PathBuf> {
    ffmpeg_fixture(
        "testsrc2_mpeg2_pcm.mxf",
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=1280x720:rate=25:duration=2",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:sample_rate=48000:duration=2",
            "-c:v",
            "mpeg2video",
            "-pix_fmt",
            "yuv422p",
            "-q:v",
            "2",
            "-g",
            "12",
            "-bf",
            "2",
            "-c:a",
            "pcm_s24le",
            "-ac",
            "2",
        ],
    )
}

/// MXF OP1a with intra-only DNxHD 36 (Avid's offline codec).
fn dnxhd_mxf() -> Option<PathBuf> {
    ffmpeg_fixture(
        "testsrc2_dnxhd.mxf",
        &["-f", "lavfi", "-i", "testsrc2=size=1920x1080:rate=25:duration=1", "-c:v", "dnxhd", "-b:v", "36M", "-pix_fmt", "yuv422p"],
    )
}

/// MXF is importable footage (After Effects imports it), whatever the extension's case.
#[test]
fn mxf_is_importable() {
    assert!(effectcraft_media::is_importable(Path::new("/media/A001C003.mxf")));
    assert!(effectcraft_media::is_importable(Path::new("/media/A001C003.MXF")));
    assert!(!effectcraft_media::is_importable(Path::new("/media/A001C003.avi")), "no AVI demuxer yet");
}

#[test]
fn mxf_mpeg2_with_pcm_audio() {
    let Some(p) = mpeg2_mxf() else { return };
    let f = probe(&p).expect("probe");
    assert_eq!(f.kind, FootageKind::Video);
    assert_eq!((f.width, f.height), (1280, 720));
    assert_eq!(f.frame_rate, FrameRate::FPS_25);
    assert!((f.duration.seconds() - 2.0).abs() < 0.1, "duration {:?}", f.duration.seconds());
    assert!(f.has_video && f.has_audio);
    eprintln!("mxf mpeg-2: codec {:?}", f.codec);
    let pool = MediaPool::new();
    // sequential, a backwards seek, a jump into a GOP (B-frames reordered), the last frame
    for n in [0usize, 1, 2, 3, 30, 7, 49] {
        let img = pool.frame_at(&f, t_of(f.frame_rate, n as i64)).expect("frame");
        let db = psnr(&img, &reference_frame(&p, n));
        eprintln!("mxf mpeg-2 frame {n}: {db:.2} dB");
        assert!(db > 35.0, "frame {n}: {db:.2} dB");
    }
    let s = pool.audio_samples(&f, Tick(TICKS_PER_SECOND / 2), 4800, 48_000);
    assert_eq!(s.len(), 9600);
    let rms = (s.iter().map(|v| v * v).sum::<f32>() / s.len() as f32).sqrt();
    // ffmpeg's sine source upmixed to stereo: −24 dBFS RMS
    assert!((rms - 0.0625).abs() < 0.01, "rms {rms}");
}

#[test]
fn mxf_dnxhd_frames() {
    let Some(p) = dnxhd_mxf() else { return };
    let f = probe(&p).expect("probe");
    assert_eq!(f.kind, FootageKind::Video);
    assert_eq!((f.width, f.height), (1920, 1080));
    assert_eq!(f.frame_rate, FrameRate::FPS_25);
    assert!(!f.has_audio);
    eprintln!("mxf dnxhd: codec {:?}", f.codec);
    let pool = MediaPool::new();
    for n in [0usize, 12, 24, 3] {
        let img = pool.frame_at(&f, t_of(f.frame_rate, n as i64)).expect("frame");
        let db = psnr(&img, &reference_frame(&p, n));
        eprintln!("mxf dnxhd frame {n}: {db:.2} dB");
        assert!(db > 35.0, "frame {n}: {db:.2} dB");
    }
}

#[test]
fn png_sequence_import() {
    let Some(first) = png_sequence() else { return };
    // One file probes as a still; the engine groups numbered files into the sequence.
    let mut f = probe(&first).expect("probe");
    assert_eq!(f.kind, FootageKind::Still);
    f.kind = FootageKind::Sequence;
    f.sequence = (1..=10).map(|n| first.with_file_name(format!("seq_{n:04}.png")).to_string_lossy().to_string()).collect();
    f.sync_sequence_duration();
    assert_eq!((f.width, f.height), (320, 240));
    assert_eq!(f.duration, FrameRate::FPS_30.tick_of(10));
    let pool = MediaPool::new();
    let img = pool.frame_at(&f, t_of(f.frame_rate, 3)).expect("frame");
    assert_eq!(*img, load_png(&first.with_file_name("seq_0004.png")));
    // past the end clamps to the last file; with 2 loops it wraps
    let img = pool.frame_at(&f, t_of(f.frame_rate, 13)).expect("frame");
    assert_eq!(*img, load_png(&first.with_file_name("seq_0010.png")));
    let mut looped = f.clone();
    looped.loop_count = 2;
    let img = pool.frame_at(&looped, t_of(f.frame_rate, 13)).expect("frame");
    assert_eq!(*img, load_png(&first.with_file_name("seq_0004.png")));
}

/// A gap in a sequence's numbering shows colour bars, as in After Effects; in Force
/// Alphabetical Order the files play back to back (#297).
#[test]
fn sequence_gaps_show_placeholders() {
    let dir = fixtures().join(format!("gap-seq-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("mkdir");
    // Frames 1, 2 and 4: frame 3 is missing. Each frame is a flat grey of its number.
    let grey = |n: u8| image::RgbaImage::from_pixel(8, 4, image::Rgba([n * 50, n * 50, n * 50, 255]));
    let mut files = vec![];
    for n in [1u8, 2, 4] {
        let p = dir.join(format!("gap_{n:03}.png"));
        grey(n).save(&p).expect("write png");
        files.push(p.to_string_lossy().to_string());
    }
    let mut f = Footage { kind: FootageKind::Sequence, sequence: files, ..probe(dir.join("gap_001.png")).expect("probe") };
    f.sync_sequence_duration();
    assert_eq!(f.duration, f.frame_rate.tick_of(4), "frames 1-4");
    let pool = MediaPool::new();
    let at = |f: &Footage, n: i64| pool.frame_at(f, t_of(f.frame_rate, n)).expect("frame");
    let level = |img: &Image| (img.get(0, 0)[0] * 255.0).round() as u32;
    assert_eq!(level(&at(&f, 1)), 100);
    // The placeholder: colour bars across the frame, starting with 75 % white.
    let bars = at(&f, 2);
    assert_eq!((bars.width, bars.height), (8, 4));
    assert_eq!(bars.get(0, 0), [0.75, 0.75, 0.75, 1.0]);
    assert_eq!(bars.get(7, 3), [0.0, 0.0, 0.75, 1.0], "blue on the right");
    assert_eq!(level(&at(&f, 3)), 200);
    f.alphabetical = true;
    f.sync_sequence_duration();
    assert_eq!(f.duration, f.frame_rate.tick_of(3));
    assert_eq!(level(&at(&f, 2)), 200, "frame 4 follows frame 2");
    let _ = std::fs::remove_dir_all(&dir);
}

/// #1812: a still asked for at a reduced size comes back as the renderer would resample it, and
/// is kept at that size: later frames at that size neither decode nor resample it again (a
/// parallax project's 21120-px plates were decoded and resampled for every frame at Quarter).
#[test]
fn stills_at_reduced_sizes_are_resampled_once() {
    let dir = std::env::temp_dir().join(format!("ec-still-size-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let p = dir.join("plate.png");
    let px = |x: u32, y: u32| {
        image::Rgba([(x * 7 % 256) as u8, (y * 13 % 256) as u8, ((x ^ y) % 256) as u8, if (x + y).is_multiple_of(5) { 0 } else { 255 - (x % 128) as u8 }])
    };
    image::RgbaImage::from_fn(613, 97, px).save(&p).expect("png");
    let f = probe(&p).expect("probe");
    let pool = MediaPool::new();
    let full = pool.frame(ItemId(1), &f, Tick::ZERO).expect("frame");
    // At its own size it is the unsized frame: no second decode, no second full-size copy.
    let misses = pool.stats().misses;
    let native = pool.frame_at_size(ItemId(1), &f, Tick::ZERO, f.width, f.height).expect("frame");
    assert!(Arc::ptr_eq(&full, &native), "the native size reuses the unsized frame");
    assert_eq!(pool.stats().misses, misses);
    for (w, h) in [(306, 48), (153, 24), (77, 12)] {
        let sized = pool.frame_at_size(ItemId(1), &f, Tick::ZERO, w, h).expect("frame");
        assert_eq!(*sized, effectcraft_raster::resample(&full, w, h), "{w}×{h}: the renderer's resample");
        let misses = pool.stats().misses;
        let again = pool.frame_at_size(ItemId(1), &f, Tick(3 * TICKS_PER_SECOND), w, h).expect("frame");
        assert!(Arc::ptr_eq(&sized, &again), "{w}×{h}: kept at that size");
        assert_eq!(pool.stats().misses, misses, "{w}×{h}: not decoded again");
    }
    // Pixels larger than the nominal size (a smart object's) are resampled as the renderer does;
    // smaller ones are left as they are (the renderer scales the layer up instead).
    // One pool, one file, one requested size: the two decisions don't share a cache entry.
    let larger = Footage { width: 306, height: 48, ..f.clone() };
    let smaller = Footage { width: 1226, height: 194, ..f.clone() };
    let sized = pool.frame_at_size(ItemId(2), &larger, Tick::ZERO, 153, 24).expect("frame");
    assert_eq!(*sized, effectcraft_raster::resample(&full, 153, 24));
    let kept = pool.frame_at_size(ItemId(3), &smaller, Tick::ZERO, 153, 24).expect("frame");
    assert_eq!(*kept, *full);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn png_with_alpha() {
    let Some(p) = alpha_png() else { return };
    let f = probe(&p).expect("probe");
    assert_eq!(f.kind, FootageKind::Still);
    assert_eq!((f.width, f.height), (256, 128));
    assert_eq!(f.alpha, AlphaMode::Straight);
    assert_eq!(f.codec, "PNG");
    let pool = MediaPool::new();
    let a = pool.frame_at(&f, Tick::ZERO).expect("frame");
    let b = pool.frame_at(&f, Tick(5 * TICKS_PER_SECOND)).expect("frame");
    assert!(Arc::ptr_eq(&a, &b), "stills are timeless");
    assert_eq!(*a, load_png(&p));
    assert!(a.data[0][3] == 0.0 && a.data[a.data.len() - 1][3] > 0.98);
    let th = pool.thumbnail(&f, 64).expect("thumb");
    assert_eq!((th.width, th.height), (64, 32));
}

/// File ▸ Import of an FBX scene: importable by extension and probed as a 3D model item.
#[test]
fn fbx_imports_as_a_model() {
    assert!(effectcraft_media::is_importable(Path::new("scene.fbx")));
    let fbx = "FBXHeaderExtension: {\n FBXVersion: 7400\n}\nObjects: {\n Geometry: 1, \"Geometry::Tri\", \"Mesh\" {\n  Vertices: *9 {\n   a: 0,0,0,1,0,0,0,1,0\n  }\n  PolygonVertexIndex: *3 {\n   a: 0,1,-3\n  }\n }\n Model: 2, \"Model::Tri\", \"Mesh\" {\n }\n}\nConnections: {\n C: \"OO\",2,0\n C: \"OO\",1,2\n}\n";
    let f = effectcraft_media::probe_bytes("scene.fbx", Arc::from(fbx.as_bytes())).expect("probe");
    assert_eq!(f.kind, FootageKind::Model);
    assert_eq!(f.codec, "FBX (1 meshes, 1 triangles)");
}

#[test]
fn wav_audio() {
    let Some(p) = tone_wav() else { return };
    let f = probe(&p).expect("probe");
    assert_eq!(f.kind, FootageKind::Audio);
    assert!(f.has_audio && !f.has_video);
    assert!((f.duration.seconds() - 1.0).abs() < 0.01);
    let pool = MediaPool::new();
    assert!(pool.frame(ItemId(1), &f, Tick::ZERO).is_none());
    let s = pool.audio_samples(&f, Tick::ZERO, 48_000, 48_000);
    let rms = (s.iter().map(|v| v * v).sum::<f32>() / s.len() as f32).sqrt();
    assert!((rms - 0.0625).abs() < 0.005, "rms {rms}");
    // resampled to 44.1 kHz: same loudness
    let s = pool.audio_samples(&f, Tick::ZERO, 44_100, 44_100);
    let rms = (s.iter().map(|v| v * v).sum::<f32>() / s.len() as f32).sqrt();
    assert!((rms - 0.0625).abs() < 0.01, "rms {rms}");
}

#[test]
fn cache_budget_evicts() {
    let Some(p) = vp9_webm() else { return };
    let f = probe(&p).expect("probe");
    let frame_bytes = 640 * 360 * 16;
    let pool = MediaPool::with_budget(3 * frame_bytes + 1024);
    for n in 0..10 {
        pool.frame_at(&f, t_of(f.frame_rate, n)).expect("frame");
    }
    let st = pool.stats();
    assert!(st.frames <= 3 && st.bytes <= 3 * frame_bytes + 1024, "{st:?}");
    pool.set_budget(0);
    assert_eq!(pool.stats().frames, 1);
}

#[test]
fn concurrent_requests() {
    let Some(p) = h264_mp4() else { return };
    let f = probe(&p).expect("probe");
    let pool = MediaPool::new();
    std::thread::scope(|s| {
        for k in 0..4i64 {
            let (pool, f) = (&pool, &f);
            s.spawn(move || {
                for n in 0..15 {
                    let img = pool.frame_at(f, t_of(f.frame_rate, k * 15 + n)).expect("frame");
                    assert_eq!((img.width, img.height), (1920, 1080));
                }
            });
        }
    });
}

#[test]
fn perf_sequential_1080p_h264() {
    let Some(p) = h264_mp4() else { return };
    let f = probe(&p).expect("probe");
    let n = 60;
    // FilmCraft alone (decode only, YUV frames)
    let bytes: Arc<[u8]> = std::fs::read(&p).expect("read").into();
    let src = filmcraft_codecs::open_bytes("a.mp4", bytes).expect("open");
    let t0 = Instant::now();
    for i in 0..n {
        let t = filmcraft_time::Tick(f.frame_rate.tick_of(i).0 + f.frame_rate.frame_duration().0 / 2);
        src.video_frame(filmcraft_media::FrameRequest::full(t)).expect("frame");
    }
    let raw = n as f64 / t0.elapsed().as_secs_f64();
    // Through the pool (decode + conversion to f32 RGBA), cold cache, without and with read-ahead
    let pool = MediaPool::new();
    pool.set_read_ahead(false);
    let t0 = Instant::now();
    for i in 0..n {
        pool.frame_at(&f, t_of(f.frame_rate, i)).expect("frame");
    }
    let plain = n as f64 / t0.elapsed().as_secs_f64();
    let pool = MediaPool::new();
    let t0 = Instant::now();
    for i in 0..n {
        pool.frame_at(&f, t_of(f.frame_rate, i)).expect("frame");
    }
    let ours = n as f64 / t0.elapsed().as_secs_f64();
    assert!(pool.stats().prefetched > 0);
    // Cached (the last 20 frames fit the default budget)
    let t0 = Instant::now();
    for i in n - 20..n {
        pool.frame_at(&f, t_of(f.frame_rate, i)).expect("frame");
    }
    let cached = 20.0 / t0.elapsed().as_secs_f64();
    assert!(pool.stats().hits >= 20);
    eprintln!(
        "1080p H.264 sequential: filmcraft decode {raw:.1} fps; MediaPool (decode + RGBA f32) {plain:.1} fps, with read-ahead {ours:.1} fps; cached {cached:.0} fps"
    );
}

#[test]
fn exr_still_is_linear_float() {
    // written with the image crate's OpenEXR encoder (no ffmpeg needed)
    let dir = fixtures();
    std::fs::create_dir_all(&dir).expect("mkdir");
    let p = dir.join(format!("float-{}.exr", std::process::id()));
    // linear 0.214 ≈ sRGB 0.5 (opaque); premultiplied 2.0 at alpha 0.5
    let px = vec![0.214f32, 0.214, 0.214, 1.0, 2.0, 1.0, 0.5, 0.5];
    image::DynamicImage::ImageRgba32F(image::ImageBuffer::from_raw(2, 1, px).expect("buf")).save(&p).expect("write exr");
    let f = probe(&p).expect("probe");
    assert_eq!(f.kind, FootageKind::Still);
    assert_eq!((f.width, f.height), (2, 1));
    assert_eq!(f.codec, "EXR");
    assert_eq!(f.alpha, AlphaMode::Premultiplied);
    let img = MediaPool::new().frame_at(&f, Tick::ZERO).expect("frame");
    std::fs::remove_file(&p).ok();
    let a = img.data[0];
    assert!((a[0] - 0.5).abs() < 2e-3 && a[3] == 1.0, "{a:?}");
    // straight 4.0 → encoded ≈ 1.83 (super-white kept) → re-premultiplied ≈ 0.92
    let b = img.data[1];
    assert!(b[3] == 0.5 && b[0] > 0.85 && b[0] < 1.0, "{b:?}");
}

/// #745: a Targa file has no signature, so its format comes from the extension; it imports as
/// a still with straight alpha, identical to ffmpeg's own decode of the source.
#[test]
fn targa_still_matches_ffmpeg() {
    let Some(png) = alpha_png() else { return };
    // ffmpeg's Targa encoder (run-length compressed)
    let Some(tga) = ffmpeg_fixture("alpha.tga", &["-i", png.to_str().expect("utf8")]) else { return };
    assert!(effectcraft_media::is_importable(&tga));
    let f = probe(&tga).expect("probe");
    assert_eq!(f.kind, FootageKind::Still);
    assert_eq!((f.width, f.height), (256, 128));
    assert_eq!(f.codec, "TGA");
    assert_eq!(f.alpha, AlphaMode::Straight);
    let img = MediaPool::new().frame_at(&f, Tick::ZERO).expect("frame");
    assert_eq!(*img, load_png(&png));
    // hostile input: a truncated file is refused or decodes partially, never panics
    let bytes = std::fs::read(&tga).expect("read");
    for n in [0, 5, 18, 40, bytes.len() / 2] {
        let cut: Arc<[u8]> = bytes.get(..n).expect("cut").into();
        if let Ok(f) = effectcraft_media::probe_bytes("cut.tga", cut.clone()) {
            let pool = MediaPool::new();
            pool.add_bytes("cut.tga", cut);
            let _ = pool.frame_at(&f, Tick::ZERO);
        }
    }
}

/// #745: a Radiance HDR file holds linear float light, sRGB-encoded like OpenEXR (values above 1
/// kept); ffmpeg's encoder is the oracle for the file layout.
#[test]
fn radiance_hdr_still_is_linear_float() {
    // written with the image crate's encoder: linear 0.214 ≈ sRGB 0.5, and super-white 4.0
    let dir = fixtures();
    std::fs::create_dir_all(&dir).expect("mkdir");
    let p = dir.join(format!("float-{}.hdr", std::process::id()));
    let px = vec![0.214f32, 0.214, 0.214, 4.0, 4.0, 4.0];
    image::DynamicImage::ImageRgb32F(image::ImageBuffer::from_raw(2, 1, px).expect("buf")).save(&p).expect("write hdr");
    assert!(effectcraft_media::is_importable(&p));
    let f = probe(&p).expect("probe");
    assert_eq!(f.kind, FootageKind::Still);
    assert_eq!((f.width, f.height), (2, 1));
    assert_eq!(f.codec, "HDR");
    assert_eq!(f.alpha, AlphaMode::Ignore);
    let img = MediaPool::new().frame_at(&f, Tick::ZERO).expect("frame");
    std::fs::remove_file(&p).ok();
    let a = img.data[0];
    assert!((a[0] - 0.5).abs() < 5e-3 && a[3] == 1.0, "{a:?}");
    assert!(img.data[1][0] > 1.5, "super-white kept: {:?}", img.data[1]);

    // ffmpeg's Radiance encoder (run-length compressed scanlines): a mid grey of 0.5 linear
    let Some(ff) = ffmpeg_fixture("grey.hdr", &["-f", "lavfi", "-i", "color=c=0x808080:size=64x16", "-frames:v", "1", "-pix_fmt", "gbrpf32le"]) else {
        return;
    };
    let f = probe(&ff).expect("probe ffmpeg hdr");
    assert_eq!((f.width, f.height), (64, 16));
    let img = MediaPool::new().frame_at(&f, Tick::ZERO).expect("frame");
    let want = 1.055 * (128.0f32 / 255.0).powf(1.0 / 2.4) - 0.055;
    for q in &img.data {
        assert!((q[0] - want).abs() < 0.01 && (q[1] - want).abs() < 0.01 && (q[2] - want).abs() < 0.01 && q[3] == 1.0, "{q:?} vs {want}");
    }
}

/// A FLAC of `secs` seconds at `rate` Hz with `channels` channels of a 440 Hz sine (first two:
/// −24 dBFS RMS, the rest quieter), samples of `fmt` (`s16`, or `s32` with 24 significant bits).
fn sine_flac(secs: u32, rate: u32, channels: u32, fmt: &str) -> Option<PathBuf> {
    let name = format!("sine_{secs}s_{rate}_{channels}ch_{fmt}.flac");
    let src = format!("sine=frequency=440:sample_rate={rate}:duration={secs}");
    let pan = format!("pan={channels}c|{}", (0..channels).map(|c| format!("c{c}={}*c0", if c < 2 { "1" } else { "0.5" })).collect::<Vec<_>>().join("|"));
    let mut args = vec!["-f", "lavfi", "-i", &src, "-af", &pan, "-sample_fmt", fmt];
    if fmt == "s32" {
        args.extend(["-bits_per_raw_sample", "24"]);
    }
    args.extend(["-c:a", "flac"]);
    ffmpeg_fixture(&name, &args)
}

/// #408 robustness: FLAC footage of any rate, depth and channel count reads correctly as the
/// preview feeder reads it, one device block at a time at the mix rate, through its end and past
/// it: two minutes of 24-bit, 6-channel, 96 kHz audio, and mono 8 kHz, 11.025 kHz, 192 kHz and
/// 8-channel files.
#[test]
fn flac_audio_of_any_shape_reads_in_device_blocks() {
    for (secs, rate, channels, fmt) in [(120, 96_000, 6, "s32"), (5, 8_000, 1, "s16"), (5, 11_025, 2, "s16"), (5, 192_000, 2, "s32"), (5, 48_000, 8, "s16")] {
        let Some(p) = sine_flac(secs, rate, channels, fmt) else { return };
        let f = probe(&p).expect("probe");
        assert!(f.has_audio && f.kind == FootageKind::Audio, "{}", p.display());
        assert!((f.duration.seconds() - f64::from(secs)).abs() < 0.01, "{}: {}", p.display(), f.duration.seconds());
        let pool = MediaPool::new();
        let mix = 48_000i64;
        let total = f.duration.to_units_floor(mix);
        let (mut at, mut sum, mut n, mut worst) = (0i64, 0.0f64, 0usize, std::time::Duration::ZERO);
        let t0 = Instant::now();
        while at < total + 4096 {
            let t = Instant::now();
            let s = pool.audio_samples(&f, Tick::from_units(at, mix), 1024, 48_000);
            if at > 0 {
                worst = worst.max(t.elapsed());
            }
            assert_eq!(s.len(), 2048);
            assert!(s.iter().all(|v| v.is_finite() && v.abs() <= 1.0), "{} at {at}", p.display());
            if at + 1024 <= total {
                sum += s.iter().map(|v| f64::from(*v) * f64::from(*v)).sum::<f64>();
                n += s.len();
            } else if at >= total {
                assert!(s.iter().all(|v| *v == 0.0), "{}: silence past the end", p.display());
            }
            at += 1024;
        }
        // ffmpeg's sine source has an amplitude of 1/8 (mono is duplicated). The fixture carries
        // channels 0 and 1 at full level and the others at half, so a multichannel file sounds as
        // its stereo downmix (tests/it/downmix.rs checks that matrix channel by channel).
        let gains = effectcraft_media::downmix::stereo_gains(channels as usize, false);
        let side = |s: usize| gains.iter().enumerate().map(|(c, g)| f64::from(g[s]) * if c < 2 { 1.0 } else { 0.5 }).sum::<f64>();
        let want = 0.125 * ((side(0).powi(2) + side(1).powi(2)) / 2.0).sqrt() / 2f64.sqrt();
        let rms = (sum / n.max(1) as f64).sqrt();
        assert!((rms - want).abs() < 0.003, "{}: rms {rms}, expected {want}", p.display());
        eprintln!("{}: read {secs} s in {:?}, slowest block after the first {worst:?}", p.display(), t0.elapsed());
        assert_eq!(pool.stats().opened, 1, "opened once, not per block");
    }
}

/// #408: the first reads of a FLAC from many threads at once (a preview's frame workers, audio
/// feeder, conform and waveform threads) decode it once: each decoded the whole file, so memory
/// and time grew with the number of threads.
#[test]
fn concurrent_first_reads_open_a_file_once() {
    let Some(p) = sine_flac(60, 44_100, 2, "s16") else { return };
    let f = probe(&p).expect("probe");
    let pool = MediaPool::new();
    let start = Arc::new(std::sync::Barrier::new(8));
    let reads: Vec<_> = (0..8)
        .map(|_| {
            let (pool, f, start) = (pool.clone(), f.clone(), start.clone());
            std::thread::spawn(move || {
                start.wait();
                pool.audio_samples(&f, Tick(TICKS_PER_SECOND), 1024, 48_000)
            })
        })
        .collect();
    let reads: Vec<Vec<f32>> = reads.into_iter().map(|t| t.join().expect("read")).collect();
    assert_eq!(pool.stats().opened, 1, "the file was opened once");
    assert!(reads.iter().all(|r| *r == reads[0]) && reads[0].iter().any(|v| *v != 0.0));
}

/// Settings ▸ Disk ▸ Conformed Audio Folder: a FLAC's conformed file, written a chunk at a time,
/// reads back exactly as the decoder reads.
#[test]
fn conformed_flac_audio_reads_back_like_the_decoder() {
    let Some(p) = sine_flac(5, 44_100, 2, "s16") else { return };
    let f = probe(&p).expect("probe");
    let folder = fixtures().join(format!("conformed-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&folder);
    let pool = MediaPool::new();
    let at = Tick::from_seconds_f64(2.5);
    let decoded = pool.audio_samples(&f, at, 4800, 48_000);
    pool.set_conform_folder(Some(folder.clone()));
    let file = pool.conformed_path(&p.to_string_lossy(), 48_000).expect("a conformed path");
    pool.audio_samples(&f, at, 4800, 48_000);
    let t0 = Instant::now();
    while !file.exists() {
        assert!(t0.elapsed().as_secs() < 60, "the conformed file was never written");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    // (Renamed into place once complete.) Raw interleaved stereo f32 at 48 kHz.
    let bytes = std::fs::read(&file).expect("conformed file");
    assert_eq!(bytes.len(), 5 * 48_000 * 8);
    let s0 = at.to_units_floor(48_000) as usize * 8;
    let conformed: Vec<f32> = bytes[s0..s0 + 4800 * 8].as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect();
    assert_eq!(conformed, decoded);
    assert_eq!(pool.audio_samples(&f, at, 4800, 48_000), decoded);
    let _ = std::fs::remove_dir_all(&folder);
}

/// File ▸ Import of an STL part: importable by extension and probed as a 3D model item.
#[test]
fn stl_imports_as_a_model() {
    assert!(effectcraft_media::is_importable(Path::new("part.stl")));
    let ascii = "solid part\nfacet normal 0 0 1\nouter loop\nvertex 0 0 0\nvertex 1 0 0\nvertex 0 1 0\nendloop\nendfacet\nendsolid part\n";
    let f = effectcraft_media::probe_bytes("part.stl", Arc::from(ascii.as_bytes())).expect("probe");
    assert_eq!(f.kind, FootageKind::Model);
    assert_eq!(f.codec, "STL (1 meshes, 1 triangles)");
}

/// `src` decoded by ffmpeg to 8-bit (`deep`: 16-bit) RGBA, premultiplied like the pool's frames.
fn ffmpeg_rgba(src: &Path, deep: bool) -> Vec<[f32; 4]> {
    let out = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-i"])
        .arg(src)
        .args(["-sws_flags", "accurate_rnd+full_chroma_int+bitexact", "-f", "rawvideo", "-pix_fmt", if deep { "rgba64le" } else { "rgba" }, "-"])
        .output()
        .expect("run ffmpeg");
    assert!(out.status.success(), "ffmpeg decode of {}", src.display());
    let px = |v: [f32; 4]| [v[0] * v[3], v[1] * v[3], v[2] * v[3], v[3]];
    if !deep {
        return out.stdout.as_chunks::<4>().0.iter().map(|p| px(p.map(|v| v as f32 / 255.0))).collect();
    }
    out.stdout.as_chunks::<8>().0.iter().map(|p| px(std::array::from_fn(|i| u16::from_le_bytes([p[2 * i], p[2 * i + 1]]) as f32 / 65535.0))).collect()
}

/// #756: DPX stills in every layout and depth ffmpeg's encoder writes (8-bit luma / RGB / RGBA /
/// ABGR, 10- and 12-bit filled RGB, 16-bit luma / RGB / RGBA, big- and little-endian, lines
/// padded to 32-bit words) import and decode as ffmpeg decodes them.
#[test]
fn dpx_stills_match_ffmpeg() {
    let Some(png) = alpha_png() else { return };
    let src = png.to_str().expect("utf8");
    let formats = [
        ("rgb24", "DPX 8-bit RGB", false),
        ("rgba", "DPX 8-bit RGBA", true),
        ("abgr", "DPX 8-bit ABGR", true),
        ("gray", "DPX 8-bit luma", false),
        ("gray16be", "DPX 16-bit luma", false),
        ("gbrp10be", "DPX 10-bit RGB", false),
        ("gbrp10le", "DPX 10-bit RGB", false),
        ("gbrp12be", "DPX 12-bit RGB", false),
        ("gbrp12le", "DPX 12-bit RGB", false),
        ("rgb48be", "DPX 16-bit RGB", false),
        ("rgba64le", "DPX 16-bit RGBA", true),
    ];
    for (pf, codec, alpha) in formats {
        // 250 px wide: 8-bit and 10-bit lines need padding to a 32-bit word.
        let Some(p) = ffmpeg_fixture(&format!("dpx-{pf}.dpx"), &["-i", src, "-vf", "scale=250:100", "-pix_fmt", pf]) else { return };
        assert!(effectcraft_media::is_importable(&p));
        let f = probe(&p).expect(pf);
        assert_eq!(f.kind, FootageKind::Still);
        assert_eq!((f.width, f.height), (250, 100), "{pf}");
        // Some ffmpeg builds state the transfer (", linear"), which the codec name then carries.
        assert!(f.codec == codec || f.codec.starts_with(&format!("{codec}, ")), "{pf}: {}", f.codec);
        assert_eq!(f.alpha, if alpha { AlphaMode::Straight } else { AlphaMode::Ignore }, "{pf}");
        let img = MediaPool::new().frame_at(&f, Tick::ZERO).expect(pf);
        let want = ffmpeg_rgba(&p, !codec.contains("8-bit"));
        assert_eq!(want.len(), img.data.len(), "{pf}");
        let worst = img.data.iter().zip(&want).flat_map(|(a, b)| (0..4).map(move |k| (a[k] - b[k]).abs())).fold(0.0f32, f32::max);
        // (within one 16-bit step: ffmpeg widens 10/12-bit samples by bit replication)
        assert!(worst < 2.0 / 65535.0, "{pf}: worst difference {worst}");
    }
}

/// #756: a numbered run of DPX files plays as an image sequence.
#[test]
fn dpx_sequence_plays() {
    let Some(first) = ffmpeg_fixture("dpxseq/plate.0001.dpx", &["-f", "lavfi", "-i", "color=c=0x202020:size=32x16", "-frames:v", "1", "-pix_fmt", "gbrp10le"])
    else {
        return;
    };
    let Some(second) = ffmpeg_fixture("dpxseq/plate.0002.dpx", &["-f", "lavfi", "-i", "color=c=0xe0e0e0:size=32x16", "-frames:v", "1", "-pix_fmt", "gbrp10le"])
    else {
        return;
    };
    let mut f = probe(&first).expect("probe");
    f.kind = FootageKind::Sequence;
    f.sequence = [&first, &second].iter().map(|p| p.to_string_lossy().to_string()).collect();
    f.sync_sequence_duration();
    let pool = MediaPool::new();
    let level = |n| pool.frame_at(&f, t_of(f.frame_rate, n)).expect("frame").data[0][0];
    assert!((level(0) - 32.0 / 255.0).abs() < 0.01 && (level(1) - 224.0 / 255.0).abs() < 0.01, "{} {}", level(0), level(1));
}

/// #756: a Kodak Cineon file (no ffmpeg encoder: written here from the spec, 10-bit RGB
/// left-justified in big-endian 32-bit words; checked against ImageMagick's reader when written)
/// shows its log code values as stored; a truncated one is refused, never a panic.
#[test]
fn cineon_still_shows_code_values() {
    let (w, h) = (5u32, 3u32);
    let code = |x: u32, y: u32| [(x * 37 + y * 101) % 1024, (x * 11 + y * 7 + 300) % 1024, (x * 3 + y * 251 + 700) % 1024];
    let mut b = vec![0u8; 1024];
    b[..4].copy_from_slice(&[0x80, 0x2a, 0x5f, 0xd7]);
    b[4..8].copy_from_slice(&1024u32.to_be_bytes());
    b[8..12].copy_from_slice(&1024u32.to_be_bytes());
    b[24..28].copy_from_slice(b"V4.5");
    b[193] = 3;
    for c in 0..3 {
        let o = 196 + 28 * c;
        b[o + 1] = c as u8 + 1;
        b[o + 2] = 10;
        b[o + 4..o + 8].copy_from_slice(&w.to_be_bytes());
        b[o + 8..o + 12].copy_from_slice(&h.to_be_bytes());
    }
    b[681] = 5;
    for y in 0..h {
        for x in 0..w {
            let [r, g, bl] = code(x, y);
            b.extend_from_slice(&((r << 22) | (g << 12) | (bl << 2)).to_be_bytes());
        }
    }
    let bytes: Arc<[u8]> = b.clone().into();
    let f = effectcraft_media::probe_bytes("scan.cin", bytes.clone()).expect("probe");
    assert_eq!((f.kind, f.width, f.height, f.codec.as_str(), f.alpha), (FootageKind::Still, 5, 3, "Cineon 10-bit RGB, log", AlphaMode::Ignore));
    assert!(effectcraft_media::is_importable(Path::new("scan.cin")));
    let pool = MediaPool::new();
    pool.add_bytes("scan.cin", bytes);
    let img = pool.frame_at(&f, Tick::ZERO).expect("frame");
    for y in 0..h {
        for x in 0..w {
            let want = code(x, y).map(|v| v as f32 / 1023.0);
            let got = img.get(i64::from(x), i64::from(y));
            assert!((0..3).all(|k| (got[k] - want[k]).abs() < 1e-4) && got[3] == 1.0, "{x},{y}: {got:?} vs {want:?}");
        }
    }
    for n in [3, 100, 700, 1024, b.len() - 1] {
        let cut: Arc<[u8]> = b.get(..n).expect("cut").into();
        assert!(effectcraft_media::probe_bytes("cut.cin", cut).is_err(), "{n}");
    }
}

/// #329: a frame read that is called off (a scrub has moved on) gives up before the decoder
/// seeks and decodes forward from the keyframe, so it no longer holds the decoder the newest
/// frame waits for; the frame decodes as usual when it is wanted again.
#[test]
fn a_called_off_read_does_not_decode() {
    use std::sync::atomic::AtomicBool;
    let Some(p) = h264_mp4() else { return };
    let f = probe(&p).expect("probe");
    let pool = MediaPool::new();
    let t = t_of(f.frame_rate, 45);
    let off = Arc::new(AtomicBool::new(true));
    let mut got = None;
    pool.cancellable(&off, &mut || got = Some(pool.frame(ItemId(1), &f, t)));
    assert!(got.expect("ran").is_none(), "a called-off read decoded its frame");
    assert_eq!(pool.stats().frames, 0, "nothing cached for it");
    let on = Arc::new(AtomicBool::new(false));
    let mut got = None;
    pool.cancellable(&on, &mut || got = pool.frame(ItemId(1), &f, t));
    let img = got.expect("decoded once wanted again");
    assert_eq!((img.width, img.height), (1920, 1080));
}

/// H.264 High 4:2:2 10-bit: the software decoder lacks it; the pool says why, once, and the
/// footage reports it (not a black frame without a word).
#[test]
fn an_unsupported_stream_reports_why_it_shows_no_picture() {
    let Some(p) = try_ffmpeg_fixture(
        "h264_high422_10bit.mp4",
        &["-f", "lavfi", "-i", "testsrc=size=320x180:rate=24:duration=1", "-c:v", "libx264", "-profile:v", "high422", "-pix_fmt", "yuv422p10le"],
    ) else {
        return;
    };
    let f = probe(&p).expect("probe");
    let pool = MediaPool::new();
    assert_eq!(pool.problem(&f), None, "nothing is wrong before a frame was asked for");
    if pool.frame(ItemId(1), &f, Tick::ZERO).is_some() {
        // a decoder that handles 4:2:2 (FilmCraft's, or a hardware one): no notice
        assert_eq!(pool.problem(&f), None);
        return;
    }
    let why = pool.problem(&f).expect("a failed frame leaves a reason");
    assert!(why.contains("4:2:2"), "names the stream: {why}");
    assert!(why.contains("isn't supported"), "says it is unsupported: {why}");
    assert!(!why.contains("decode error"), "no error wrappers: {why}");
    // it stays for the other frames, and is dropped when the file is forgotten
    assert!(pool.frame(ItemId(1), &f, Tick::from_seconds_f64(0.5)).is_none());
    assert_eq!(pool.problem(&f).as_deref(), Some(why.as_str()));
    pool.forget(&f.path);
    assert_eq!(pool.problem(&f), None);
}

#[test]
fn the_h264_fixture_options_fit_the_picked_encoder() {
    assert_eq!(h264_video_options("libx264"), ["-preset", "medium", "-crf", "18"]);
    for enc in ["libopenh264", "h264_nvenc", "h264_vaapi", "h264_qsv"] {
        let options = h264_video_options(enc);
        assert!(!options.iter().any(|o| *o == "-preset" || *o == "-crf"), "{enc} rejects preset/crf: {options:?}");
        assert!(options.contains(&"-b:v"), "{enc} gets a bitrate: {options:?}");
    }
}

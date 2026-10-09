//! Footage import against ffmpeg as an external oracle.
//!
//! Fixtures are generated with ffmpeg into `target/fixtures/media` (never committed); every test
//! skips (passes with a note) when ffmpeg is not installed. Decoded frames are compared with
//! frames ffmpeg extracts to PNG (PSNR, premultiplied RGBA).

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Instant;

use effectcraft_media::{MediaPool, probe};
use effectcraft_project::{AlphaMode, Footage, FootageKind, ItemId, MissingFrames};
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

/// Run ffmpeg to produce `name` (once; written to a temp name and renamed so concurrent test
/// processes never see a partial file). `None` when ffmpeg is unavailable.
fn ffmpeg_fixture(name: &str, args: &[&str]) -> Option<PathBuf> {
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
    assert!(st.success(), "ffmpeg failed for {name}");
    std::fs::rename(&tmp, &out).expect("rename");
    Some(out)
}

fn h264_mp4() -> Option<PathBuf> {
    ffmpeg_fixture(
        "testsrc2_1080p.mp4",
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=1920x1080:rate=30:duration=2",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:sample_rate=48000:duration=2",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            "-preset",
            "medium",
            "-crf",
            "18",
            "-colorspace",
            "bt709",
            "-color_primaries",
            "bt709",
            "-color_trc",
            "bt709",
            "-c:a",
            "aac",
            "-ac",
            "2",
            "-shortest",
        ],
    )
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
    ffmpeg_fixture(
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
            "libx264",
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

/// A gap in a sequence's numbering shows colour bars (Missing Frames ▸ Show Placeholder), the
/// frame before it (Hold) or nothing at all (Skip: the files play back to back) (#297).
#[test]
fn sequence_gaps_show_placeholders_hold_or_close_up() {
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
    let mut f =
        Footage { kind: FootageKind::Sequence, sequence: files, missing_frames: MissingFrames::Placeholder, ..probe(dir.join("gap_001.png")).expect("probe") };
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
    f.missing_frames = MissingFrames::Hold;
    assert_eq!(level(&at(&f, 2)), 100, "holds frame 2");
    f.missing_frames = MissingFrames::Skip;
    f.sync_sequence_duration();
    assert_eq!(f.duration, f.frame_rate.tick_of(3));
    assert_eq!(level(&at(&f, 2)), 200, "frame 4 follows frame 2");
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

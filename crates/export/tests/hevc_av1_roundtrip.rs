//! HEVC (MP4 `hvc1`) and AV1 (MP4 `av01`, WebM `V_AV1`) export, decoded back through
//! EffectCraft's own import path (FilmCraft's demuxers and HEVC / AV1 decoders) and — when
//! installed — ffprobe / ffmpeg as external oracles (skipped otherwise).
//!
//! The comp: 64×48 @ 10 fps, 1.2 s (12 frames), dark-blue background, a red solid of 32×48
//! centred, starting at frame 5 (inter frames code both static and changing blocks).

use std::path::{Path, PathBuf};
use std::process::Command;

use effectcraft_color::Label;
use effectcraft_export::render_queue::{
    AudioOutput, CodecProfile, OpusApplication, OutputFormat, OutputModule, RateControlMode, RenderSettings, TimeSpan, WebmVideoCodec,
};
use effectcraft_export::{Job, Progress, export};
use effectcraft_media::MediaPool;
use effectcraft_project::{Comp, ItemId, ItemKind, LayerSource, Project, Solid, build};
use effectcraft_raster::Image;
use effectcraft_render::{FootageSource, NoFootage};
use effectcraft_time::{FrameRate, Tick};

const W: u32 = 64;
const H: u32 = 48;
const BG: [f32; 3] = [0.1, 0.1, 0.4];
const FRAMES: u64 = 12;

fn out_dir(name: &str) -> PathBuf {
    let d = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-out/export-hevc-av1").join(name);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("mkdir");
    d
}

fn project(audio: Option<&Path>) -> (Project, ItemId) {
    project_size(W, H, audio)
}

fn project_size(w: u32, h: u32, audio: Option<&Path>) -> (Project, ItemId) {
    let mut p = Project::default();
    let mut comp = Comp::new(w, h, FrameRate::new(10, 1), Tick::from_seconds_f64(1.2));
    comp.background = BG;
    let sid = p.add_item("Red", Label::Red, None, ItemKind::Solid(Solid { color: [1.0, 0.0, 0.0], width: w / 2, height: h, pixel_aspect: 1.0 }));
    let mut l = build::layer(&mut p, &comp, "Red", LayerSource::Solid { item: sid }, (w / 2, h), None);
    l.in_point = comp.frame_rate.tick_of(5);
    comp.layers.push(l);
    if let Some(path) = audio {
        let f = effectcraft_media::probe(path).expect("probe wav");
        let fid = p.add_item("tone.wav", Label::SeaFoam, None, ItemKind::Footage(f));
        let l = build::layer(&mut p, &comp, "tone", LayerSource::Footage { item: fid }, (0, 0), None);
        comp.layers.push(l);
    }
    let cid = p.add_item("Comp 1", Label::Sandstone, None, ItemKind::Comp(comp.into()));
    (p, cid)
}

fn run(p: &Project, cid: ItemId, footage: &dyn FootageSource, om: &OutputModule, path: &Path) -> effectcraft_export::Report {
    let s = RenderSettings { time_span: TimeSpan::LengthOfComp, ..Default::default() };
    let path = path.to_string_lossy().to_string();
    let job = Job {
        project: p,
        footage,
        expr: None,
        accel: None,
        comp: cid,
        settings: &s,
        output: om,
        path: &path,
        sink: None,
        nested_switches: true,
        options: Default::default(),
    };
    let mut last = Progress::default();
    let r = export(&job, &mut |pr| {
        last = *pr;
        true
    })
    .expect("export");
    assert_eq!((last.done, r.frames), (FRAMES, FRAMES));
    r
}

fn px(img: &Image, x: u32, y: u32) -> [f32; 4] {
    img.data[img.idx(x, y)]
}

fn near(a: [f32; 3], b: [f32; 3], tol: f32) -> bool {
    a.iter().zip(b).all(|(x, y)| (x - y).abs() <= tol)
}

fn check_frame(img: &Image, k: u64, tol: f32) {
    let corner = px(img, 2, 2);
    assert!(near([corner[0], corner[1], corner[2]], BG, tol), "frame {k} corner {corner:?}");
    let c = px(img, img.width / 2, img.height / 2);
    let want = if k >= 5 { [1.0, 0.0, 0.0] } else { BG };
    assert!(near([c[0], c[1], c[2]], want, tol), "frame {k} centre {c:?} want {want:?}");
}

/// Import our file through the media layer and check every frame.
fn decode_movie(path: &Path, tol: f32) -> effectcraft_project::Footage {
    let f = effectcraft_media::probe(path).expect("probe our output");
    assert_eq!((f.width, f.height), (W, H));
    assert!(f.has_video);
    let n = f.frame_rate.frame_at(f.duration - Tick(1)) + 1;
    assert_eq!(n as u64, FRAMES, "frame count from duration {:?} @ {:?}", f.duration, f.frame_rate);
    let pool = MediaPool::new();
    for k in 0..FRAMES {
        let img = pool.frame_at(&f, f.frame_rate.tick_of(k as i64)).expect("decode");
        check_frame(&img, k, tol);
    }
    f
}

fn have(tool: &str) -> bool {
    Command::new(tool).arg("-version").output().map(|o| o.status.success()).unwrap_or(false)
}

/// `codec|w|h|pix_fmt|frames` of the video stream, or `None` without ffprobe.
fn ffprobe_video(path: &Path) -> Option<String> {
    if !have("ffprobe") {
        eprintln!("ffprobe not found: skipping the oracle");
        return None;
    }
    let out = Command::new("ffprobe")
        .args(["-v", "error", "-select_streams", "v:0", "-count_frames", "-show_entries", "stream=codec_name,width,height,pix_fmt,nb_read_frames"])
        .args(["-of", "compact=p=0:nk=1"])
        .arg(path)
        .output()
        .expect("ffprobe");
    assert!(out.status.success() && out.stderr.is_empty(), "ffprobe rejected {}: {}", path.display(), String::from_utf8_lossy(&out.stderr));
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Decode every frame with ffmpeg (no errors allowed) and check frames 0, 4, 5 and the last.
fn ffmpeg_check(path: &Path, decoder: Option<&str>, tol: f32) {
    if !have("ffmpeg") {
        eprintln!("ffmpeg not found: skipping the oracle decode");
        return;
    }
    let dir = path.with_extension("ffmpeg");
    let _ = std::fs::create_dir_all(&dir);
    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-hide_banner", "-loglevel", "error", "-xerror", "-y"]);
    if let Some(d) = decoder {
        cmd.args(["-c:v", d]);
    }
    let out = cmd.arg("-i").arg(path).args(["-pix_fmt", "rgba"]).arg(dir.join("f%03d.png")).output().expect("ffmpeg");
    assert!(out.status.success() && out.stderr.is_empty(), "ffmpeg ({decoder:?}) failed on {}: {}", path.display(), String::from_utf8_lossy(&out.stderr));
    for k in [0, 4, 5, FRAMES - 1] {
        let img = image::open(dir.join(format!("f{:03}.png", k + 1))).expect("ffmpeg frame").to_rgba8();
        check_frame(&Image::from_rgba8(img.width(), img.height(), img.as_raw()), k, tol);
    }
}

fn module(format: OutputFormat, keyint: u32) -> OutputModule {
    let mut om = OutputModule::for_format(format);
    om.keyframe_interval = keyint;
    om.codec.rate_control = RateControlMode::Quality;
    om.codec.quality = 80;
    om.audio = AudioOutput::Off;
    om
}

/// Exercise the EffectCraft adapter, not just FilmCraft's encoder, and compare every decoded
/// picture with the uncompressed composition. Machines without a hardware session skip.
#[cfg(target_os = "macos")]
#[test]
fn videotoolbox_h264_and_hevc_roundtrip_against_rendered_frames() {
    use effectcraft_export::render_queue::HardwareEncoding;
    use filmcraft_platform::videotoolbox_encode::{VtConfig, VtEncoder, VtProfile, VtRate};

    let (p, cid) = project(None);
    for (format, profile) in [(OutputFormat::H264, VtProfile::High), (OutputFormat::Hevc, VtProfile::HevcMain)] {
        let cfg = VtConfig {
            width: W,
            height: H,
            timescale: 10,
            frame_duration: 1,
            keyframe_interval: 4,
            profile,
            rate: VtRate::Vbr { target_kbps: 4_000, max_kbps: 6_000 },
        };
        let Ok(probe) = VtEncoder::new(cfg) else {
            eprintln!("SKIPPED: no VideoToolbox {format:?} session at {W}x{H}");
            continue;
        };
        if !probe.uses_hardware() {
            eprintln!("SKIPPED: VideoToolbox did not create a hardware {format:?} session");
            continue;
        }
        drop(probe);
        let mut om = OutputModule::for_format(format);
        om.keyframe_interval = 4;
        om.bitrate_kbps = 4_000;
        om.audio = AudioOutput::Off;
        let settings = RenderSettings { time_span: TimeSpan::LengthOfComp, hardware_encoding: HardwareEncoding::On, ..Default::default() };
        let dir = out_dir(&format!("vt-{format:?}"));
        let path = dir.join("clip.mp4");
        let path_string = path.to_string_lossy().to_string();
        let job = Job {
            project: &p,
            footage: &NoFootage,
            expr: None,
            accel: None,
            comp: cid,
            settings: &settings,
            output: &om,
            path: &path_string,
            sink: None,
            nested_switches: true,
            options: Default::default(),
        };
        let report = export(&job, &mut |_| true).expect("hardware export");
        assert_eq!((report.frames, report.width, report.height), (FRAMES, W, H));
        assert_eq!(report.video_encoder, Some("VideoToolbox"), "unexpected fallback: {:?}", report.hardware_fallback_reason);
        let f = effectcraft_media::probe(&path).expect("probe hardware output");
        assert_eq!((f.width, f.height), (W, H));
        assert_eq!((f.frame_rate.frame_at(f.duration - Tick(1)) + 1) as u64, FRAMES);
        let pool = MediaPool::new();
        let hw_before = filmcraft_codecs::hw::hw_stats();
        for k in 0..FRAMES {
            let t = f.frame_rate.tick_of(k as i64);
            let actual = pool.frame_at(&f, t).expect("decode hardware frame").to_rgba8();
            // RGB export composites transparent pixels over the comp background before encoding.
            let reference = effectcraft_render::render_frame(&p, cid, t, 1.0).to_rgba8_over(BG);
            let mse = actual
                .as_chunks::<4>()
                .0
                .iter()
                .zip(reference.as_chunks::<4>().0.iter())
                .flat_map(|(a, b)| (0..3).map(move |c| (f64::from(a[c]) - f64::from(b[c])).powi(2)))
                .sum::<f64>()
                / (W * H * 3) as f64;
            let psnr = if mse == 0.0 { f64::INFINITY } else { 10.0 * (255.0 * 255.0 / mse).log10() };
            eprintln!("{format:?} frame {k}: PSNR {psnr:.2} dB");
            // The small, saturated red rectangle creates high-contrast 4:2:0 chroma edges.
            assert!(psnr > 28.0, "{format:?} frame {k}: PSNR {psnr:.2} dB");
        }
        let hw_after = filmcraft_codecs::hw::hw_stats();
        eprintln!(
            "{format:?} hardware decode: {} frames, {} sessions, {} fallbacks",
            hw_after.frames - hw_before.frames,
            hw_after.sessions - hw_before.sessions,
            hw_after.fallbacks - hw_before.fallbacks
        );
        assert!(hw_after.frames > hw_before.frames, "VideoToolbox did not decode {format:?} frames");
    }
}

#[cfg(target_os = "linux")]
#[test]
fn linux_h264_and_hevc_hardware_decode_roundtrip() {
    eprintln!("Linux hardware decoder probe: {:?}", filmcraft_platform::vaapi::va::probe());
    let (p, cid) = project_size(640, 360, None);
    for format in [OutputFormat::H264, OutputFormat::Hevc] {
        let dir = out_dir(&format!("linux-decode-{format:?}"));
        let path = dir.join("clip.mp4");
        let mut om = module(format, 4);
        if format == OutputFormat::Hevc {
            // Exercise the Linux hardware path, which accepts target bitrate rather than CQ.
            om.codec.rate_control = RateControlMode::Bitrate;
            om.bitrate_kbps = 4_000;
        }
        let report = run(&p, cid, &NoFootage, &om, &path);
        eprintln!("{format:?} encoder: {:?}, fallback: {:?}", report.video_encoder, report.hardware_fallback_reason);
        if format == OutputFormat::Hevc && filmcraft_platform::nvenc::hevc_available() {
            assert_eq!(report.video_encoder, Some("NVENC"), "HEVC did not use NVENC on a capable NVIDIA host");
            assert_eq!(report.hardware_fallback_reason, None);
        }
        if std::env::var_os("EFFECTCRAFT_REQUIRE_VAAPI").is_some() {
            assert_eq!(report.video_encoder, Some("VA-API"), "{format:?} did not use VA-API on the requested Radeon test host");
            assert_eq!(report.hardware_fallback_reason, None);
        }
        let before = filmcraft_codecs::hw::hw_stats();
        let f = effectcraft_media::probe(&path).expect("probe Linux output");
        assert_eq!((f.width, f.height), (640, 360));
        assert_eq!((f.frame_rate.frame_at(f.duration - Tick(1)) + 1) as u64, FRAMES);
        let pool = MediaPool::new();
        for k in 0..FRAMES {
            let img = pool.frame_at(&f, f.frame_rate.tick_of(k as i64)).expect("decode Linux output");
            check_frame(&img, k, 0.15);
        }
        let after = filmcraft_codecs::hw::hw_stats();
        eprintln!(
            "{format:?} hardware decode {:?}: {} frames, {} sessions, {} declined, {} fallbacks",
            filmcraft_codecs::hw::hw_backend(),
            after.frames - before.frames,
            after.sessions - before.sessions,
            after.declined - before.declined,
            after.fallbacks - before.fallbacks
        );
        assert!(after.frames > before.frames, "hardware decoder did not decode {format:?} frames");
        assert_eq!(after.fallbacks, before.fallbacks, "hardware decoder fell back on {format:?}");
    }
}

/// Key-frame flags of the MP4 video samples (via ffprobe), or `None` without it.
fn ffprobe_keyframes(path: &Path) -> Option<Vec<bool>> {
    if !have("ffprobe") {
        return None;
    }
    let out = Command::new("ffprobe")
        .args(["-v", "error", "-select_streams", "v:0", "-show_entries", "packet=flags", "-of", "csv=p=0"])
        .arg(path)
        .output()
        .expect("ffprobe");
    Some(String::from_utf8_lossy(&out.stdout).lines().map(|l| l.starts_with('K')).collect())
}

#[test]
fn hevc_mp4_roundtrip() {
    let d = out_dir("hevc");
    let (p, cid) = project(None);
    let om = module(OutputFormat::Hevc, 4);
    let path = d.join("comp.mp4");
    let r = run(&p, cid, &NoFootage, &om, &path);
    assert!(r.bytes > 100 && !r.audio);
    #[cfg(target_os = "macos")]
    {
        assert_eq!(r.video_encoder, Some("software"));
        assert!(r.hardware_fallback_reason.as_deref().is_some_and(|reason| reason.contains("constant-quality")));
    }
    let bytes = std::fs::read(&path).unwrap();
    assert!(bytes.windows(4).any(|w| w == b"hvc1") && bytes.windows(4).any(|w| w == b"hvcC"));
    decode_movie(&path, 0.08);
    if let Some(s) = ffprobe_video(&path) {
        assert_eq!(s, "hevc|64|48|yuv420p|12");
    }
    if let Some(k) = ffprobe_keyframes(&path) {
        assert_eq!(k, (0..FRAMES).map(|i| i % 4 == 0).collect::<Vec<_>>(), "IDR every 4 frames");
    }
    ffmpeg_check(&path, None, 0.08);
}

#[test]
fn hevc_main10_bitrate_mode() {
    let d = out_dir("hevc10");
    let (p, cid) = project(None);
    let mut om = module(OutputFormat::Hevc, 0);
    om.codec.profile = CodecProfile::Main10;
    om.codec.rate_control = RateControlMode::Bitrate;
    om.bitrate_kbps = 2_000;
    om.codec.level = Some(31);
    let path = d.join("comp10.mp4");
    run(&p, cid, &NoFootage, &om, &path);
    decode_movie(&path, 0.08);
    if let Some(s) = ffprobe_video(&path) {
        assert_eq!(s, "hevc|64|48|yuv420p10le|12");
    }
    ffmpeg_check(&path, None, 0.08);
}

#[test]
fn av1_mp4_roundtrip() {
    let d = out_dir("av1");
    let (p, cid) = project(None);
    let om = module(OutputFormat::Av1, 6);
    let path = d.join("comp.mp4");
    run(&p, cid, &NoFootage, &om, &path);
    let bytes = std::fs::read(&path).unwrap();
    assert!(bytes.windows(4).any(|w| w == b"av01") && bytes.windows(4).any(|w| w == b"av1C"));
    decode_movie(&path, 0.08);
    if let Some(s) = ffprobe_video(&path) {
        assert_eq!(s, "av1|64|48|yuv420p|12");
    }
    if let Some(k) = ffprobe_keyframes(&path) {
        assert_eq!(k, (0..FRAMES).map(|i| i % 6 == 0).collect::<Vec<_>>(), "key frame every 6 frames");
    }
    ffmpeg_check(&path, Some("libdav1d"), 0.08);
}

#[test]
fn av1_webm_with_opus() {
    let d = out_dir("av1-webm");
    let wav = d.join("tone.wav");
    write_tone(&wav, 2.0);
    let (p, cid) = project(Some(&wav));
    let pool = MediaPool::new();
    let mut om = module(OutputFormat::WebM, 0);
    om.webm_codec = WebmVideoCodec::Av1;
    om.codec.profile = CodecProfile::Main10;
    om.audio = AudioOutput::Auto;
    om.opus_bitrate_kbps = 96;
    let path = d.join("comp.webm");
    let r = run(&p, cid, &pool, &om, &path);
    assert!(r.audio);
    let f = decode_movie(&path, 0.08);
    assert!(f.has_audio);
    let s = MediaPool::new().audio_samples(&f, Tick::from_seconds_f64(0.3), 24_000, 48_000);
    let l = rms(s.iter().step_by(2).copied());
    assert!((l - 0.3536).abs() < 0.06, "left RMS {l}");
    if let Some(s) = ffprobe_video(&path) {
        assert_eq!(s, "av1|64|48|yuv420p10le|12");
    }
    ffmpeg_check(&path, Some("libdav1d"), 0.08);
}

/// Low-bitrate WebM audio goes through Opus SILK (voice) or hybrid (audio) coding and still
/// decodes to the tone (FilmCraft's decoder, and ffmpeg when installed).
#[test]
fn webm_low_bitrate_opus_modes() {
    let d = out_dir("opus-modes");
    let wav = d.join("tone.wav");
    write_tone(&wav, 2.0);
    let (p, cid) = project(Some(&wav));
    let pool = MediaPool::new();
    for (kbps, app, name) in [(24u32, OpusApplication::Voip, "voice24.webm"), (40, OpusApplication::Audio, "audio40.webm")] {
        let mut om = OutputModule::for_format(OutputFormat::WebM);
        om.audio = AudioOutput::On;
        om.opus_bitrate_kbps = kbps;
        om.opus_application = app;
        let path = d.join(name);
        assert!(run(&p, cid, &pool, &om, &path).audio);
        let f = effectcraft_media::probe(&path).expect("probe");
        let s = MediaPool::new().audio_samples(&f, Tick::from_seconds_f64(0.3), 24_000, 48_000);
        let l = rms(s.iter().step_by(2).copied());
        assert!((l - 0.3536).abs() < 0.08, "{name}: left RMS {l}");
        if have("ffmpeg") {
            let out =
                Command::new("ffmpeg").args(["-v", "error", "-xerror", "-i"]).arg(&path).args(["-map", "0:a", "-f", "null", "-"]).output().expect("ffmpeg");
            assert!(out.status.success() && out.stderr.is_empty(), "{name}: {}", String::from_utf8_lossy(&out.stderr));
        }
    }
}

fn rms(v: impl Iterator<Item = f32>) -> f32 {
    let (s, n) = v.fold((0.0f64, 0usize), |(s, n), x| (s + (x as f64) * (x as f64), n + 1));
    (s / n.max(1) as f64).sqrt() as f32
}

/// 16-bit stereo WAV with a 440 Hz sine at amplitude 0.5 (left) and silence (right).
fn write_tone(path: &Path, secs: f64) {
    let sr = 48_000u32;
    let n = (secs * sr as f64) as usize;
    let mut data = Vec::with_capacity(n * 4);
    for i in 0..n {
        let s = (0.5 * (2.0 * std::f64::consts::PI * 440.0 * i as f64 / sr as f64).sin() * 32767.0).round() as i16;
        data.extend_from_slice(&s.to_le_bytes());
        data.extend_from_slice(&0i16.to_le_bytes());
    }
    let mut w = Vec::new();
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    w.extend_from_slice(b"WAVEfmt ");
    w.extend_from_slice(&16u32.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes());
    w.extend_from_slice(&2u16.to_le_bytes());
    w.extend_from_slice(&sr.to_le_bytes());
    w.extend_from_slice(&(sr * 4).to_le_bytes());
    w.extend_from_slice(&4u16.to_le_bytes());
    w.extend_from_slice(&16u16.to_le_bytes());
    w.extend_from_slice(b"data");
    w.extend_from_slice(&(data.len() as u32).to_le_bytes());
    w.extend_from_slice(&data);
    std::fs::write(path, w).expect("write wav");
}

//! Encode a short comp to every output format and decode it back: through EffectCraft's own
//! media layer (FilmCraft decoders) for MP4/MOV, the `image`/`gif` crates for stills and GIF,
//! and — when installed — ffprobe as an external oracle (skipped otherwise).
//!
//! The comp: 64×48 @ 10 fps, 1.2 s (12 frames), dark-blue background, a red solid of 32×48
//! centred, starting at frame 5. So frame < 5 is background everywhere; from frame 5 the centre
//! is red and the corners stay background.

use std::path::{Path, PathBuf};
use std::process::Command;

use effectcraft_color::Label;
use effectcraft_export::render_queue::{AudioOutput, Channels, OutputFormat, OutputModule, ProResProfile, RenderSettings, TimeSpan, sequence_path};
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
    let d = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-out/export").join(name);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("mkdir");
    d
}

fn project(audio: Option<&Path>) -> (Project, ItemId) {
    let mut p = Project::default();
    let mut comp = Comp::new(W, H, FrameRate::new(10, 1), Tick::from_seconds_f64(1.2));
    comp.background = BG;
    let sid = p.add_item("Red", Label::Red, None, ItemKind::Solid(Solid { color: [1.0, 0.0, 0.0], width: 32, height: H, pixel_aspect: 1.0 }));
    let mut l = build::layer(&mut p, &comp, "Red", LayerSource::Solid { item: sid }, (32, H), None);
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

fn settings() -> RenderSettings {
    RenderSettings { time_span: TimeSpan::LengthOfComp, ..Default::default() }
}

fn run(p: &Project, cid: ItemId, footage: &dyn FootageSource, om: &OutputModule, path: &Path) -> effectcraft_export::Report {
    run_frames(p, cid, footage, om, path, FRAMES)
}

/// [`run`] for a comp of `frames` frames.
fn run_frames(p: &Project, cid: ItemId, footage: &dyn FootageSource, om: &OutputModule, path: &Path, frames: u64) -> effectcraft_export::Report {
    let s = settings();
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
        options: Default::default(),
        nested_switches: true,
    };
    let mut last = Progress::default();
    let mut calls = 0;
    let r = export(&job, &mut |pr| {
        assert!(pr.done >= last.done, "progress is monotonic");
        last = *pr;
        calls += 1;
        true
    })
    .expect("export");
    assert_eq!(last.done, frames);
    assert_eq!(last.total, frames);
    assert!(calls >= 2);
    assert_eq!(r.frames, frames);
    r
}

fn px(img: &Image, x: u32, y: u32) -> [f32; 4] {
    img.data[img.idx(x, y)]
}

fn near(a: [f32; 3], b: [f32; 3], tol: f32) -> bool {
    a.iter().zip(b).all(|(x, y)| (x - y).abs() <= tol)
}

/// Check a decoded (premultiplied, opaque) frame `k`.
fn check_frame(img: &Image, k: u64, tol: f32) {
    let corner = px(img, 2, 2);
    assert!(near([corner[0], corner[1], corner[2]], BG, tol), "frame {k} corner {corner:?}");
    let c = px(img, img.width / 2, img.height / 2);
    let want = if k >= 5 { [1.0, 0.0, 0.0] } else { BG };
    assert!(near([c[0], c[1], c[2]], want, tol), "frame {k} centre {c:?} want {want:?}");
}

fn decode_movie(path: &Path, frames: u64, tol: f32) -> effectcraft_project::Footage {
    let f = effectcraft_media::probe(path).expect("probe our output");
    assert_eq!((f.width, f.height), (W, H));
    assert!(f.has_video);
    let n = f.frame_rate.frame_at(f.duration - Tick(1)) + 1;
    assert_eq!(n as u64, frames, "frame count from duration {:?} @ {:?}", f.duration, f.frame_rate);
    let pool = MediaPool::new();
    for k in 0..frames {
        let img = pool.frame_at(&f, f.frame_rate.tick_of(k as i64)).expect("decode");
        check_frame(&img, k, tol);
    }
    f
}

fn ffprobe(path: &Path) -> Option<String> {
    let out = Command::new("ffprobe")
        .args(["-v", "error", "-count_frames", "-show_entries", "stream=codec_name,width,height,nb_read_frames,pix_fmt", "-of", "compact=p=0:nk=1"])
        .arg(path)
        .output()
        .ok()?;
    if !out.status.success() {
        panic!("ffprobe rejected {}: {}", path.display(), String::from_utf8_lossy(&out.stderr));
    }
    Some(String::from_utf8_lossy(&out.stdout).to_string())
}

/// Frame `k` decoded by ffmpeg (external oracle) as RGBA, or `None` without ffmpeg.
fn ffmpeg_frame(path: &Path, k: u64) -> Option<Image> {
    let png = path.with_extension(format!("ffmpeg-{k}.png"));
    let st = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y", "-i"])
        .arg(path)
        .args(["-vf", &format!("select=eq(n\\,{k})"), "-frames:v", "1", "-pix_fmt", "rgba"])
        .arg(&png)
        .status()
        .ok()?;
    assert!(st.success(), "ffmpeg failed to decode {}", path.display());
    let img = image::open(&png).expect("ffmpeg png").to_rgba8();
    Some(Image::from_rgba8(img.width(), img.height(), img.as_raw()))
}

fn ffmpeg_check(path: &Path, tol: f32) {
    for k in [0, 4, 5, FRAMES - 1] {
        match ffmpeg_frame(path, k) {
            Some(img) => check_frame(&img, k, tol),
            None => {
                eprintln!("ffmpeg not found: skipping oracle decode");
                return;
            }
        }
    }
}

#[test]
fn h264_mp4_roundtrip() {
    let d = out_dir("h264");
    let (p, cid) = project(None);
    let om = OutputModule { bitrate_kbps: 4000, ..OutputModule::for_format(OutputFormat::H264) };
    let path = d.join("comp.mp4");
    let r = run(&p, cid, &NoFootage, &om, &path);
    assert!(r.bytes > 100 && !r.audio);
    decode_movie(&path, FRAMES, 0.08);
    ffmpeg_check(&path, 0.08);
    if let Some(s) = ffprobe(&path) {
        assert!(s.contains("h264|64|48|yuv420p"), "{s}");
        assert!(s.lines().next().unwrap_or("").ends_with("|12"), "{s}");
    }
}

#[test]
fn h264_odd_size_is_cropped_even() {
    let d = out_dir("h264-odd");
    let (p, cid) = project(None);
    let om = OutputModule::for_format(OutputFormat::H264);
    let s = RenderSettings { resolution: 31.0 / 64.0, ..settings() };
    let comp = p.comp(cid).expect("comp");
    assert_eq!(s.output_size(comp).0 % 2, 1);
    let path = d.join("odd.mp4").to_string_lossy().to_string();
    let job = Job {
        project: &p,
        footage: &NoFootage,
        expr: None,
        accel: None,
        comp: cid,
        settings: &s,
        output: &om,
        path: &path,
        sink: None,
        options: Default::default(),
        nested_switches: true,
    };
    let r = export(&job, &mut |_| true).expect("export");
    assert_eq!((r.width % 2, r.height % 2), (0, 0));
}

#[test]
fn prores_mov_roundtrip() {
    let d = out_dir("prores");
    let (p, cid) = project(None);
    let om = OutputModule::for_format(OutputFormat::ProRes);
    let path = d.join("comp.mov");
    run(&p, cid, &NoFootage, &om, &path);
    decode_movie(&path, FRAMES, 0.03);
    ffmpeg_check(&path, 0.03);
    if let Some(s) = ffprobe(&path) {
        assert!(s.starts_with("prores|64|48"), "{s}");
        assert!(s.lines().next().unwrap_or("").ends_with("|12"), "{s}");
    }
}

#[test]
fn prores_4444_with_alpha() {
    let d = out_dir("prores4444");
    let (p, cid) = project(None);
    let om = OutputModule { channels: Channels::Rgba, prores_profile: ProResProfile::Hq, ..OutputModule::for_format(OutputFormat::ProRes) };
    let path = d.join("alpha.mov");
    run(&p, cid, &NoFootage, &om, &path);
    let f = effectcraft_media::probe(&path).expect("probe");
    let img = MediaPool::new().frame_at(&f, f.frame_rate.tick_of(7)).expect("decode");
    assert!(px(&img, 2, 2)[3] < 0.02, "corner transparent: {:?}", px(&img, 2, 2));
    let c = px(&img, W / 2, H / 2);
    assert!(c[3] > 0.98 && c[0] > 0.95 && c[1] < 0.05, "centre opaque red: {c:?}");
    if let Some(s) = ffprobe(&path) {
        assert!(s.starts_with("prores|64|48|yuva444"), "{s}");
    }
}

/// Motion blur creates fractional alpha on moving edges. Both the app decoder and ffmpeg
/// must decode every frame of a ProRes 4444 output, including those edge samples (#293).
#[test]
fn prores_4444_motion_blur_preserves_fractional_alpha() {
    use effectcraft_keyframe::{Keyframe, Value};
    let d = out_dir("prores4444-motion-blur");
    let (mut p, cid) = project(None);
    let comp = p.comp_mut(cid).unwrap();
    comp.shutter_angle = 360.0;
    comp.shutter_phase = -180.0;
    let layer = &mut comp.layers[0];
    layer.in_point = Tick::ZERO;
    layer.switches.motion_blur = true;
    layer.props.prop_mut("transform/position").unwrap().keys = vec![
        Keyframe::new(Tick::ZERO, Value::Vec3([12.0, H as f64 / 2.0, 0.0])),
        Keyframe::new(Tick::from_seconds_f64(1.2), Value::Vec3([52.0, H as f64 / 2.0, 0.0])),
    ];
    let om = OutputModule { channels: Channels::Rgba, prores_profile: ProResProfile::P4444, ..OutputModule::for_format(OutputFormat::ProRes) };
    let path = d.join("blur.mov");
    run(&p, cid, &NoFootage, &om, &path);
    let footage = effectcraft_media::probe(&path).expect("probe blurred alpha ProRes");
    let pool = MediaPool::new();
    let mut fractional = 0;
    let mut unblurred_project = p.clone();
    unblurred_project.comp_mut(cid).unwrap().layers[0].switches.motion_blur = false;
    let mut blur_changes_alpha = false;
    for k in 0..FRAMES {
        let time = footage.frame_rate.tick_of(k as i64);
        let reference = effectcraft_render::render_frame(&p, cid, time, 1.0);
        let unblurred = effectcraft_render::render_frame(&unblurred_project, cid, time, 1.0);
        assert_eq!((reference.width, reference.height), (unblurred.width, unblurred.height));
        blur_changes_alpha |= reference.data.iter().zip(&unblurred.data).any(|(blurred, sharp)| (blurred[3] - sharp[3]).abs() > 0.015);
        let decoded = pool.frame_at(&footage, time).expect("decode blurred alpha ProRes");
        let oracle = ffmpeg_frame(&path, k);
        for (i, expected) in reference.data.iter().enumerate() {
            let alpha = expected[3];
            if alpha > 0.02 && alpha < 0.98 {
                fractional += 1;
            }
            assert!((decoded.data[i][3] - alpha).abs() < 0.015, "frame {k}, pixel {i}: alpha {} vs {alpha}", decoded.data[i][3]);
            if let Some(oracle) = &oracle {
                assert!((oracle.data[i][3] - alpha).abs() < 0.015, "ffmpeg frame {k}, pixel {i}: alpha {} vs {alpha}", oracle.data[i][3]);
            }
        }
    }
    assert!(fractional > 0, "the fixture has motion-blurred, partly transparent edges");
    assert!(blur_changes_alpha, "motion blur must change alpha compared with the same unblurred fixture");
}

fn check_still_dir(dir: &Path, ext: &str, tol: f32, alpha: bool) {
    for k in 0..FRAMES {
        let path = sequence_path(&dir.join(format!("f_[#####].{ext}")).to_string_lossy(), k as i64);
        let img = image::open(&path).unwrap_or_else(|e| panic!("{path}: {e}")).to_rgba8();
        assert_eq!(img.dimensions(), (W, H));
        let img = Image::from_rgba8(W, H, img.as_raw());
        if alpha {
            let corner = px(&img, 2, 2);
            assert!(corner[3] < 0.01, "{path}: corner {corner:?}");
            let c = px(&img, W / 2, H / 2);
            assert_eq!(c[3] > 0.99, k >= 5, "{path}: centre {c:?}");
        } else {
            check_frame(&img, k, tol);
        }
    }
    assert!(!dir.join(format!("f_{:05}.{ext}", FRAMES)).exists());
}

#[test]
fn image_sequences_roundtrip() {
    let (p, cid) = project(None);
    for (fmt, ext, tol, ch) in [
        (OutputFormat::PngSequence, "png", 0.01, Channels::Rgb),
        (OutputFormat::PngSequence, "png", 0.01, Channels::Rgba),
        (OutputFormat::JpegSequence, "jpg", 0.06, Channels::Rgb),
        (OutputFormat::TiffSequence, "tif", 0.01, Channels::Rgb),
        (OutputFormat::TiffSequence, "tif", 0.01, Channels::Rgba),
    ] {
        let d = out_dir(&format!("seq-{ext}-{ch:?}"));
        let om = OutputModule { channels: ch, ..OutputModule::for_format(fmt) };
        let r = run(&p, cid, &NoFootage, &om, &d.join(format!("f_[#####].{ext}")));
        assert!(r.path.ends_with(&format!("f_00000.{ext}")), "{}", r.path);
        check_still_dir(&d, ext, tol, ch == Channels::Rgba);
    }
}

#[test]
fn exr_sequence_is_linear() {
    let (p, cid) = project(None);
    let d = out_dir("seq-exr");
    let om = OutputModule::for_format(OutputFormat::ExrSequence);
    run(&p, cid, &NoFootage, &om, &d.join("f_[#####].exr"));
    let img = image::open(d.join("f_00000.exr")).expect("exr").to_rgba32f();
    assert_eq!(img.dimensions(), (W, H));
    let c = img.get_pixel(2, 2).0;
    // sRGB 0.4 → linear ≈ 0.133
    assert!((c[2] - 0.1329).abs() < 0.01 && (c[3] - 1.0).abs() < 1e-6, "{c:?}");
}

/// A 32 bpc project whose only layer is a full-frame opaque solid of the given colour.
fn bright_project(color: [f32; 3]) -> (Project, ItemId) {
    let mut p = Project::default();
    p.settings.bit_depth = effectcraft_project::BitDepth::Bpc32;
    let comp = Comp::new(W, H, FrameRate::new(10, 1), Tick::from_seconds_f64(0.1));
    let sid = p.add_item("Bright", Label::Red, None, ItemKind::Solid(Solid { color, width: W, height: H, pixel_aspect: 1.0 }));
    let mut comp = comp;
    let l = build::layer(&mut p, &comp, "Bright", LayerSource::Solid { item: sid }, (W, H), None);
    comp.layers.push(l);
    let cid = p.add_item("Comp 1", Label::Sandstone, None, ItemKind::Comp(comp.into()));
    (p, cid)
}

/// Export one frame of a 32 bpc solid to an EXR sequence and read its centre pixel.
fn exr_centre(color: [f32; 3], channels: Channels, name: &str) -> [f32; 4] {
    let (p, cid) = bright_project(color);
    let d = out_dir(name);
    let mut om = OutputModule::for_format(OutputFormat::ExrSequence);
    om.channels = channels;
    let s = RenderSettings { time_span: TimeSpan::LengthOfComp, ..Default::default() };
    let path = d.join("f_[#####].exr").to_string_lossy().to_string();
    let job = Job {
        project: &p,
        footage: &NoFootage,
        expr: None,
        accel: None,
        comp: cid,
        settings: &s,
        output: &om,
        path: &path,
        sink: None,
        options: Default::default(),
        nested_switches: true,
    };
    export(&job, &mut |_| true).expect("export");
    let img = image::open(d.join("f_00000.exr")).expect("exr").to_rgba32f();
    img.get_pixel(W / 2, H / 2).0
}

/// The sRGB decode of the frame the writer is handed (the solid is sRGB-encoded in the frame).
fn lin(v: f32) -> f32 {
    effectcraft_color::srgb_to_linear(v)
}

#[test]
fn exr_sequence_keeps_values_above_one() {
    // Issue #339: a 32 bpc project's over-range colour must reach the EXR, in scene-linear light.
    let src = [1.5, 1.25, 0.75];
    for channels in [Channels::Rgba, Channels::Rgb] {
        let c = exr_centre(src, channels, "seq-exr-over");
        for i in 0..3 {
            assert!((c[i] - lin(src[i])).abs() < 1e-3 * lin(src[i]).max(1.0), "{channels:?} channel {i}: {c:?}");
        }
        assert!(c[0] > 1.5 && c[1] > 1.25, "{channels:?} not clamped: {c:?}");
        assert!((c[3] - 1.0).abs() < 1e-6, "{c:?}");
    }
}

#[test]
fn exr_sequence_values_at_or_below_one_are_unchanged() {
    for channels in [Channels::Rgba, Channels::Rgb] {
        let c = exr_centre([1.0, 0.4, 0.0], channels, "seq-exr-in-range");
        assert!((c[0] - 1.0).abs() < 1e-4 && (c[1] - lin(0.4)).abs() < 1e-4 && c[2].abs() < 1e-6, "{channels:?}: {c:?}");
    }
}
#[test]
fn gif_roundtrip() {
    let (p, cid) = project(None);
    let d = out_dir("gif");
    let om = OutputModule::for_format(OutputFormat::Gif);
    let path = d.join("anim.gif");
    run(&p, cid, &NoFootage, &om, &path);
    let mut opts = gif::DecodeOptions::new();
    opts.set_color_output(gif::ColorOutput::RGBA);
    let mut dec = opts.read_info(std::fs::File::open(&path).expect("open")).expect("gif header");
    assert_eq!((dec.width() as u32, dec.height() as u32), (W, H));
    let mut k = 0;
    let mut total_delay = 0u32;
    while let Some(f) = dec.read_next_frame().expect("frame") {
        assert_eq!((f.width as u32, f.height as u32), (W, H));
        total_delay += f.delay as u32;
        let img = Image::from_rgba8(W, H, &f.buffer);
        check_frame(&img, k, 0.06);
        k += 1;
    }
    assert_eq!(k, FRAMES);
    assert_eq!(total_delay, 120, "1.2 s in centiseconds");
    if let Some(s) = ffprobe(&path) {
        assert!(s.starts_with("gif|64|48"), "{s}");
    }
}

#[test]
fn cancel_stops_early() {
    let (p, cid) = project(None);
    let d = out_dir("cancel");
    let om = OutputModule::for_format(OutputFormat::PngSequence);
    let s = settings();
    let path = d.join("c_[#####].png").to_string_lossy().to_string();
    let job = Job {
        project: &p,
        footage: &NoFootage,
        expr: None,
        accel: None,
        comp: cid,
        settings: &s,
        output: &om,
        path: &path,
        sink: None,
        options: Default::default(),
        nested_switches: true,
    };
    let r = export(&job, &mut |_| false);
    assert!(matches!(r, Err(effectcraft_export::ExportError::Cancelled)));
    let written = std::fs::read_dir(&d).map(|r| r.count()).unwrap_or(0);
    assert_eq!(written, 0, "cancelled before the first batch");
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

fn rms(v: impl Iterator<Item = f32>) -> f32 {
    let (s, n) = v.fold((0.0f64, 0usize), |(s, n), x| (s + (x as f64) * (x as f64), n + 1));
    (s / n.max(1) as f64).sqrt() as f32
}

#[test]
fn movies_carry_audio() {
    let d = out_dir("audio");
    let wav = d.join("tone.wav");
    write_tone(&wav, 2.0);
    let (p, cid) = project(Some(&wav));
    let pool = MediaPool::new();
    for (fmt, name) in [(OutputFormat::H264, "a.mp4"), (OutputFormat::ProRes, "a.mov")] {
        let om = OutputModule { audio: AudioOutput::Auto, ..OutputModule::for_format(fmt) };
        let path = d.join(name);
        let r = run(&p, cid, &pool, &om, &path);
        assert!(r.audio, "{name}: auto audio is on for a comp with a sound layer");
        let f = effectcraft_media::probe(&path).expect("probe");
        assert!(f.has_audio, "{name}");
        let dec = MediaPool::new();
        let s = dec.audio_samples(&f, Tick::from_seconds_f64(0.3), 24_000, 48_000);
        let (l, r) = (rms(s.iter().step_by(2).copied()), rms(s.iter().skip(1).step_by(2).copied()));
        // sine of amplitude 0.5 → RMS ≈ 0.354
        assert!((l - 0.3536).abs() < 0.03, "{name}: left RMS {l}");
        assert!(r < 0.01, "{name}: right RMS {r}");
        if Command::new("ffprobe").arg("-version").output().is_ok() {
            let out = Command::new("ffprobe")
                .args(["-v", "error", "-select_streams", "a:0", "-show_entries", "stream=codec_name,channels,sample_rate", "-of", "compact=p=0:nk=1"])
                .arg(&path)
                .output()
                .expect("ffprobe");
            let s = String::from_utf8_lossy(&out.stdout);
            let want = if fmt == OutputFormat::H264 { "aac|48000|2" } else { "pcm_s16le|48000|2" };
            assert!(s.trim() == want, "{name}: {s}");
        }
        // Audio off.
        let om = OutputModule { audio: AudioOutput::Off, ..om };
        let path = d.join(format!("silent-{name}"));
        assert!(!run(&p, cid, &pool, &om, &path).audio);
        assert!(!effectcraft_media::probe(&path).expect("probe").has_audio);
    }
}

/// `src`'s audio decoded by ffmpeg as interleaved stereo `f32` at 48 kHz (external oracle).
fn ffmpeg_audio(src: &Path) -> Vec<f32> {
    let out = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-i"])
        .arg(src)
        .args(["-f", "f32le", "-ac", "2", "-ar", "48000", "-"])
        .output()
        .expect("ffmpeg");
    assert!(out.status.success(), "ffmpeg failed to decode {}", src.display());
    out.stdout.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect()
}

/// Movies of a 29.97 fps comp whose footage has AAC audio at 44.1 kHz sound like the footage
/// (both decoded by ffmpeg): reading footage audio at another rate ramped from a wrong position
/// at the start of every batch of frames, so exports stuttered (#274).
#[test]
fn movies_carry_resampled_footage_audio() {
    let d = out_dir("audio-44k");
    let clip = d.join("clip.mp4");
    let made = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i", "testsrc2=size=64x48:rate=30000/1001:duration=3", "-f", "lavfi", "-i"])
        .args(["aevalsrc=0.5*sin(2*PI*440*t)|0.5*sin(2*PI*(200+400*t/3)*t):s=44100:d=3", "-c:v", "libx264", "-pix_fmt", "yuv420p", "-c:a", "aac", "-shortest"])
        .arg(&clip)
        .status()
        .is_ok_and(|s| s.success());
    if !made {
        eprintln!("ffmpeg not found: skipping");
        return;
    }
    let fps = FrameRate::new(30000, 1001);
    let mut p = Project::default();
    let mut comp = Comp::new(W, H, fps, fps.tick_of(60));
    let fid = p.add_item("clip.mp4", Label::SeaFoam, None, ItemKind::Footage(effectcraft_media::probe(&clip).expect("probe clip")));
    let l = build::layer(&mut p, &comp, "clip", LayerSource::Footage { item: fid }, (64, 48), None);
    comp.layers.push(l);
    let cid = p.add_item("Comp 1", Label::Sandstone, None, ItemKind::Comp(comp.into()));
    let want = ffmpeg_audio(&clip);
    let pool = MediaPool::new();
    // AAC is lossy; PCM differs from ffmpeg's resampler by little more than 16-bit rounding.
    for (fmt, name, tol) in [(OutputFormat::H264, "a.mp4", 0.06), (OutputFormat::ProRes, "a.mov", 0.01)] {
        let path = d.join(name);
        assert!(run_frames(&p, cid, &pool, &OutputModule::for_format(fmt), &path, 60).audio);
        let got = ffmpeg_audio(&path);
        // 60 frames at 29.97 fps: 96 096 samples at 48 kHz.
        assert!((got.len() as i64 / 2 - 96_096).abs() < 1024, "{name}: {} samples", got.len() / 2);
        // (Up to the last AAC frames, where the cut to silence smears back into the sound.)
        let worst = got.iter().zip(&want).take(94_000 * 2).map(|(g, w)| (g - w).abs()).fold(0.0f32, f32::max);
        eprintln!("{name}: max error {worst:.4}");
        assert!(worst < tol, "{name}: max error {worst} against the footage");
    }
}

#[test]
fn webm_vp9_opus_roundtrip() {
    let d = out_dir("webm");
    let wav = d.join("tone.wav");
    write_tone(&wav, 2.0);
    let (p, cid) = project(Some(&wav));
    let pool = MediaPool::new();
    let om = OutputModule { quality: 90, audio: AudioOutput::Auto, ..OutputModule::for_format(OutputFormat::WebM) };
    let path = d.join("comp.webm");
    let r = run(&p, cid, &pool, &om, &path);
    assert!(r.audio && r.bytes > 200, "{r:?}");
    // Decoded back through FilmCraft (Matroska demuxer, VP9 and Opus decoders).
    let f = decode_movie(&path, FRAMES, 0.08);
    assert!(f.has_audio);
    let s = MediaPool::new().audio_samples(&f, Tick::from_seconds_f64(0.3), 24_000, 48_000);
    let l = rms(s.iter().step_by(2).copied());
    assert!((l - 0.3536).abs() < 0.06, "left RMS {l}");
    ffmpeg_check(&path, 0.08);
    if let Some(s) = ffprobe(&path) {
        assert!(s.contains("vp9|64|48|yuv420p"), "{s}");
        assert!(s.contains("opus"), "{s}");
    }
}

#[test]
fn webm_with_alpha() {
    let d = out_dir("webm-alpha");
    let (p, cid) = project(None);
    // Transparent outside the red solid.
    let om = OutputModule { channels: Channels::Rgba, ..OutputModule::for_format(OutputFormat::WebM) };
    let path = d.join("alpha.webm");
    run(&p, cid, &NoFootage, &om, &path);
    let f = effectcraft_media::probe(&path).expect("probe our output");
    assert_eq!((f.width, f.height), (W, H));
    let bytes = std::fs::read(&path).unwrap();
    // AlphaMode element (0x53C0) and BlockAdditions (0x75A1).
    assert!(bytes.windows(3).any(|w| w == [0x53, 0xC0, 0x81]), "AlphaMode 1");
    assert!(bytes.windows(2).any(|w| w == [0x75, 0xA1]), "BlockAdditions");
    // ffmpeg's libvpx decoder reads the alpha channel.
    let png = path.with_extension("alpha.png");
    let ok = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y", "-c:v", "libvpx-vp9", "-i"])
        .arg(&path)
        .args(["-vf", "select=eq(n\\,8)", "-frames:v", "1", "-pix_fmt", "rgba"])
        .arg(&png)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if ok {
        let img = image::open(&png).unwrap().to_rgba8();
        assert!(img.get_pixel(2, 2)[3] < 20, "corner transparent");
        assert!(img.get_pixel(W / 2, H / 2)[3] > 235, "centre opaque");
    } else {
        eprintln!("ffmpeg with libvpx not available: skipping alpha oracle");
    }
}

#[test]
fn wav_and_aiff_audio_only() {
    let d = out_dir("audio-only");
    let wav = d.join("tone.wav");
    write_tone(&wav, 2.0);
    let (p, cid) = project(Some(&wav));
    let pool = MediaPool::new();
    for (fmt, name, magic) in [(OutputFormat::Wav, "out.wav", &b"RIFF"[..]), (OutputFormat::Aiff, "out.aif", &b"FORM"[..])] {
        let om = OutputModule::for_format(fmt);
        let path = d.join(name);
        let r = run(&p, cid, &pool, &om, &path);
        assert!(r.audio);
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(&bytes[..4], magic);
        // 1.2 s of 48 kHz 16-bit stereo.
        assert!((bytes.len() as i64 - 57_600 * 4).abs() < 200, "{name}: {}", bytes.len());
        let l = if fmt == OutputFormat::Wav {
            // Decoded by FilmCraft.
            let f = effectcraft_media::probe(&path).expect("probe audio");
            assert!(f.has_audio && !f.has_video, "{name}");
            let s = MediaPool::new().audio_samples(&f, Tick::from_seconds_f64(0.3), 24_000, 48_000);
            rms(s.iter().step_by(2).copied())
        } else {
            // AIFF: big-endian samples after the SSND chunk's offset/block size.
            let at = bytes.windows(4).position(|w| w == b"SSND").expect("SSND") + 16;
            let left = bytes[at..].as_chunks::<4>().0.iter().skip(14_400).take(24_000).map(|c| i16::from_be_bytes([c[0], c[1]]) as f32 / 32767.0);
            rms(left)
        };
        assert!((l - 0.3536).abs() < 0.01, "{name}: left RMS {l}");
        if let Some(s) = ffprobe(&path) {
            assert!(s.contains("pcm_s16"), "{s}");
        }
    }
}

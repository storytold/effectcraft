//! Uncompressed QuickTime (`raw `) output (#372): every frame stored is exactly the frame the
//! lossless PNG sequence of the same job writes, read back with FilmCraft's demuxer and — when
//! installed — decoded by ffmpeg as an external oracle. Audio rides along as PCM.
//!
//! The comp is 33×20 so 24-bit rows (99 bytes) need their padding byte, and a 16-px solid centred
//! in it sits on a half pixel, so its edges carry partial alpha.

use std::path::{Path, PathBuf};
use std::process::Command;

use effectcraft_color::Label;
use effectcraft_export::render_queue::{AudioFormat, AudioOutput, Channels, OutputFormat, OutputModule, RenderSettings, TimeSpan, sequence_path};
use effectcraft_export::{Job, export};
use effectcraft_media::MediaPool;
use effectcraft_project::{Comp, ItemId, ItemKind, LayerSource, Project, Solid, build};
use effectcraft_render::FootageSource;
use effectcraft_time::{FrameRate, Tick};
use filmcraft_isobmff::TrackKind;

const W: u32 = 33;
const H: u32 = 20;
const FRAMES: u64 = 6;

fn out_dir(name: &str) -> PathBuf {
    let d = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-out/export").join(name);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("mkdir");
    d
}

/// 16-bit stereo 48 kHz WAV: a 440 Hz sine at 0.5 on the left, silence on the right.
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

fn project(audio: Option<&Path>) -> (Project, ItemId) {
    let mut p = Project::default();
    let mut comp = Comp::new(W, H, FrameRate::new(10, 1), Tick::from_seconds_f64(FRAMES as f64 / 10.0));
    comp.background = [0.1, 0.3, 0.6];
    let sid = p.add_item("Red", Label::Red, None, ItemKind::Solid(Solid { color: [0.9, 0.2, 0.1], width: 16, height: 12, pixel_aspect: 1.0 }));
    let mut l = build::layer(&mut p, &comp, "Red", LayerSource::Solid { item: sid }, (16, 12), None);
    l.in_point = comp.frame_rate.tick_of(2);
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
        options: Default::default(),
        nested_switches: true,
    };
    export(&job, &mut |_| true).expect("export")
}

/// The job's frames as the PNG sequence writes them (lossless 8-bit), as RGBA.
fn reference_frames(p: &Project, cid: ItemId, footage: &dyn FootageSource, om: &OutputModule, dir: &Path) -> Vec<Vec<u8>> {
    let png = OutputModule { channels: om.channels, alpha_mode: om.alpha_mode, ..OutputModule::for_format(OutputFormat::PngSequence) };
    let pattern = dir.join("ref_[#####].png").to_string_lossy().to_string();
    run(p, cid, footage, &png, Path::new(&pattern));
    (0..FRAMES).map(|k| image::open(sequence_path(&pattern, k as i64)).expect("png").to_rgba8().into_raw()).collect()
}

/// The frames as stored in the movie, unpacked to RGBA (rows padded to even bytes).
fn stored_frames(path: &Path, alpha: bool) -> Vec<Vec<u8>> {
    let bytes = std::fs::read(path).expect("read movie");
    let mov = filmcraft_isobmff::open(bytes.as_slice()).expect("demux");
    assert!(mov.is_quicktime);
    let t = mov.tracks.iter().find(|t| t.kind == TrackKind::Video).expect("video track");
    let e = &t.entries[0];
    assert_eq!(&e.format.0, b"raw ");
    let v = e.video.as_ref().expect("visual entry");
    assert_eq!((v.width, v.height, v.depth), (W as u16, H as u16, if alpha { 32 } else { 24 }));
    assert_eq!(t.samples.len() as u64, FRAMES);
    let bpp = if alpha { 4 } else { 3 };
    let row = (W as usize * bpp).next_multiple_of(2);
    t.samples
        .iter()
        .map(|s| {
            assert_eq!(s.size as usize, row * H as usize);
            let data = &bytes[s.offset as usize..][..s.size as usize];
            let mut rgba = Vec::new();
            for r in data.chunks_exact(row) {
                for p in r[..W as usize * bpp].chunks_exact(bpp) {
                    rgba.extend_from_slice(&if alpha { [p[1], p[2], p[3], p[0]] } else { [p[0], p[1], p[2], 255] });
                }
            }
            rgba
        })
        .collect()
}

/// All frames decoded by ffmpeg (external oracle) as RGBA, or `None` without ffmpeg.
fn ffmpeg_frames(path: &Path) -> Option<Vec<Vec<u8>>> {
    let out =
        Command::new("ffmpeg").args(["-hide_banner", "-loglevel", "error", "-i"]).arg(path).args(["-f", "rawvideo", "-pix_fmt", "rgba", "-"]).output().ok()?;
    assert!(out.status.success(), "ffmpeg failed to decode {}: {}", path.display(), String::from_utf8_lossy(&out.stderr));
    Some(out.stdout.as_chunks::<{ (W * H * 4) as usize }>().0.iter().map(|f| f.to_vec()).collect())
}

fn ffprobe_streams(path: &Path) -> Option<String> {
    let out = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "stream=codec_type,codec_name,codec_tag_string,pix_fmt,width,height,sample_rate,channels",
            "-of",
            "compact=p=0:nk=1",
        ])
        .arg(path)
        .output()
        .ok()?;
    assert!(out.status.success(), "ffprobe rejected {}", path.display());
    Some(String::from_utf8_lossy(&out.stdout).to_string())
}

fn check(name: &str, channels: Channels) {
    let d = out_dir(name);
    let (p, cid) = project(None);
    let om = OutputModule { channels, ..OutputModule::for_format(OutputFormat::Uncompressed) };
    let path = d.join("comp.mov");
    let r = run(&p, cid, &effectcraft_render::NoFootage, &om, &path);
    assert_eq!((r.width, r.height, r.frames, r.audio), (W, H, FRAMES, false));
    let mut want_rgba = reference_frames(&p, cid, &effectcraft_render::NoFootage, &om, &d);
    let alpha = channels == Channels::Rgba;
    if !alpha {
        // The PNG is RGB (opaque); `to_rgba8` already filled alpha with 255.
        assert!(want_rgba.iter().all(|f| f.as_chunks::<4>().0.iter().all(|p| p[3] == 255)));
    } else {
        let a: Vec<u8> = want_rgba[3].as_chunks::<4>().0.iter().map(|p| p[3]).collect();
        assert!(a.contains(&0) && a.contains(&255) && a.iter().any(|v| *v > 0 && *v < 255), "partial alpha at the solid's edges");
    }
    assert_eq!(stored_frames(&path, alpha), want_rgba, "stored frames are the rendered frames");
    if let Some(got) = ffmpeg_frames(&path) {
        assert_eq!(got.len() as u64, FRAMES);
        want_rgba.truncate(got.len());
        assert!(got == want_rgba, "ffmpeg decodes the rendered frames exactly");
        let s = ffprobe_streams(&path).unwrap_or_default();
        let fmt = if alpha { "argb" } else { "rgb24" };
        assert_eq!(s.trim(), format!("rawvideo|video|raw |{W}|{H}|{fmt}"), "{s}");
    } else {
        eprintln!("ffmpeg not found: skipping oracle decode");
    }
}

#[test]
fn uncompressed_rgb_odd_width() {
    check("uncompressed-rgb", Channels::Rgb);
}

#[test]
fn uncompressed_rgba_straight() {
    check("uncompressed-rgba", Channels::Rgba);
}

#[test]
fn uncompressed_with_pcm_audio() {
    let d = out_dir("uncompressed-audio");
    let wav = d.join("tone.wav");
    write_tone(&wav, 1.0);
    let (p, cid) = project(Some(&wav));
    let pool = MediaPool::new();
    let om = OutputModule { audio: AudioOutput::Auto, audio_format: AudioFormat::S24, ..OutputModule::for_format(OutputFormat::Uncompressed) };
    let path = d.join("comp.mov");
    assert!(run(&p, cid, &pool, &om, &path).audio);
    let bytes = std::fs::read(&path).expect("read");
    let mov = filmcraft_isobmff::open(bytes.as_slice()).expect("demux");
    let a = mov.tracks.iter().find(|t| t.kind == TrackKind::Audio).expect("audio track");
    assert_eq!(a.timescale, 48_000);
    // 0.6 s of audio at 48 kHz.
    let frames: u64 = a.samples.iter().map(|s| s.duration as u64).sum();
    assert!((frames as i64 - 28_800).abs() <= 1, "{frames} audio frames");
    if let Some(s) = ffprobe_streams(&path) {
        assert!(s.lines().any(|l| l == "pcm_s24le|audio|lpcm|48000|2"), "{s}");
        let out = Command::new("ffmpeg").args(["-hide_banner", "-loglevel", "error", "-i"]).arg(&path).args(["-f", "f32le", "-"]).output().expect("ffmpeg");
        let pcm: Vec<f32> = out.stdout.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect();
        let rms = |v: Vec<f32>| (v.iter().map(|x| (*x as f64).powi(2)).sum::<f64>() / v.len().max(1) as f64).sqrt();
        let left = rms(pcm.iter().step_by(2).copied().collect());
        // A sine of amplitude 0.5 → RMS ≈ 0.354.
        assert!((left - 0.3536).abs() < 0.01, "left RMS {left}");
    }
}

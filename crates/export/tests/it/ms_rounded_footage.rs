//! Footage whose frame times are rounded to whole milliseconds (#1442), as phones and messaging
//! apps write MP4s: a 30 fps clip's frames last 33 or 34 ms, a 60 fps clip's 16 or 17 ms. It
//! imports at its true rate, a comp made from it (New Comp from Selection takes the footage's
//! rate) renders every source frame once, and the H.264 output has that rate and frame count.
//!
//! The fixtures are written here with FilmCraft's own encoder and muxer: 128×32 frames showing
//! their index in binary (eight bars, white for a set bit).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use effectcraft_color::Label;
use effectcraft_export::render_queue::{OutputFormat, OutputModule, RenderSettings, TimeSpan};
use effectcraft_export::{Job, export};
use effectcraft_media::MediaPool;
use effectcraft_project::{Comp, Footage, ItemKind, LayerSource, Project, build};
use effectcraft_raster::Image;
use effectcraft_time::{FrameRate, Tick};
use filmcraft_isobmff::{AvcConfig, Brand, Mp4Writer, SampleEntry, TrackConfig, WriteSample, WriterOptions};

const W: u32 = 128;
const H: u32 = 32;
const BARS: u32 = 8;

fn out_dir(name: &str) -> PathBuf {
    let d = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-out/export").join(name);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("mkdir");
    d
}

/// Durations of `n` frames at `rate` with their times rounded to whole milliseconds.
fn ms_rounded(rate: FrameRate, n: i64) -> Vec<u32> {
    let at = |k: i64| (2 * k * 1000 * rate.den + rate.num) / (2 * rate.num);
    (0..n).map(|k| u32::try_from(at(k + 1) - at(k)).expect("duration")).collect()
}

/// An H.264 MP4 (video track timescale 1000) with one frame per entry of `durations` (ms);
/// frame k shows k in binary.
fn write_movie(path: &Path, durations: &[u32]) {
    let mut cfg = filmcraft_h264enc::EncoderConfig::new(W, H, 30, 1);
    cfg.format = filmcraft_h264enc::PacketFormat::LengthPrefixed;
    cfg.aud = false;
    cfg.bframes = 0;
    cfg.keyint = 30;
    cfg.scenecut = false;
    let mut enc = filmcraft_h264enc::Encoder::new(cfg).expect("encoder");
    let (u, v) = (vec![128u8; (W * H / 4) as usize], vec![128u8; (W * H / 4) as usize]);
    let mut packets = Vec::new();
    for k in 0..durations.len() {
        let y: Vec<u8> = (0..W * H).map(|i| if (k >> (i % W * BARS / W)) & 1 == 1 { 235 } else { 16 }).collect();
        let frame = filmcraft_h264enc::YuvFrame { y: &y, u: &u, v: &v, y_stride: W as usize, uv_stride: W as usize / 2 };
        packets.extend(enc.try_encode(&frame, k as i64).expect("encode"));
    }
    packets.extend(enc.flush());
    assert_eq!(packets.len(), durations.len(), "one access unit per frame");
    let avcc = AvcConfig::parse(&enc.avcc()).expect("avcC");
    let file = std::fs::File::create(path).expect("create");
    let mut mux = Mp4Writer::new(file, WriterOptions::new(Brand::Mp4)).expect("mux");
    let t = mux.add_track(TrackConfig::new(SampleEntry::avc(avcc, W as u16, H as u16), 1000)).expect("track");
    for (p, &duration) in packets.iter().zip(durations) {
        mux.write_sample(t, WriteSample { data: &p.data, duration, composition_offset: 0, is_sync: p.keyframe }).expect("sample");
    }
    mux.finish().expect("finish");
}

/// The frame index a decoded frame shows.
fn shown(img: &Image) -> u32 {
    (0..BARS).filter(|b| img.data[img.idx(b * W / BARS + W / BARS / 2, H / 2)][1] > 0.5).map(|b| 1 << b).sum()
}

fn frame_count(f: &Footage) -> i64 {
    f.frame_rate.frame_at(f.duration - Tick(1)) + 1
}

/// Every frame of a movie in order, as indices.
fn frames_of(f: &Footage) -> Vec<u32> {
    let pool = MediaPool::new();
    (0..frame_count(f)).map(|k| shown(&pool.frame_at(f, f.frame_rate.tick_of(k)).expect("decode"))).collect()
}

/// Import `path`, make a comp from it as New Comp from Selection does (the footage's size, rate
/// and duration), render it to H.264 and return the output's footage.
fn comp_from_footage(path: &Path, out: &Path) -> Footage {
    let footage = effectcraft_media::probe(path).expect("probe");
    let mut p = Project::default();
    let fid = p.add_item("clip.mp4", Label::Aqua, None, ItemKind::Footage(footage.clone()));
    let rate = p.item(fid).and_then(|it| it.frame_rate()).expect("a movie has a rate");
    let mut comp = Comp::new(W, H, rate, footage.duration);
    comp.layers.push(build::layer(&mut p, &comp, "clip.mp4", LayerSource::Footage { item: fid }, (W, H), None));
    let cid = p.add_item("clip", Label::Sandstone, None, ItemKind::Comp(comp.into()));
    let pool = MediaPool::new();
    let settings = RenderSettings { time_span: TimeSpan::LengthOfComp, ..Default::default() };
    let om = OutputModule { bitrate_kbps: 4000, ..OutputModule::for_format(OutputFormat::H264) };
    let out_path = out.to_string_lossy().to_string();
    let job = Job {
        project: &p,
        footage: &pool,
        expr: None,
        accel: None,
        comp: cid,
        settings: &settings,
        output: &om,
        path: &out_path,
        sink: None,
        options: Default::default(),
        nested_switches: true,
    };
    let r = export(&job, &mut |_| true).expect("export");
    assert_eq!(r.frames as i64, frame_count(&footage), "the render has the footage's frames");
    effectcraft_media::probe(out).expect("probe the render")
}

#[test]
fn millisecond_rounded_footage_keeps_its_rate_through_comp_and_render() {
    let d = out_dir("ms_rounded");
    for (rate, frames) in [(FrameRate::FPS_30, 120), (FrameRate::FPS_60, 240)] {
        let path = d.join(format!("ms-{}.mp4", rate.num));
        write_movie(&path, &ms_rounded(rate, frames));
        let f = effectcraft_media::probe(&path).expect("probe");
        assert_eq!((f.frame_rate, frame_count(&f)), (rate, frames), "{rate:?} footage");
        let bytes: Arc<[u8]> = std::fs::read(&path).expect("read").into();
        let from_bytes = effectcraft_media::probe_bytes(&path.to_string_lossy(), bytes).expect("probe bytes");
        assert_eq!(from_bytes.frame_rate, rate, "{rate:?} footage read from memory");
        let all: Vec<u32> = (0..frames as u32).collect();
        assert_eq!(frames_of(&f), all, "{rate:?} footage: each source frame once");

        let out = comp_from_footage(&path, &d.join(format!("render-{}.mp4", rate.num)));
        assert_eq!((out.frame_rate, frame_count(&out)), (rate, frames), "{rate:?} render");
        assert_eq!(out.duration, Tick::from_seconds_f64(4.0), "{rate:?} render duration");
        assert_eq!(frames_of(&out), all, "{rate:?} render: no frame skipped or repeated");
    }
}

#[test]
fn ntsc_and_variable_rate_footage_is_not_pulled_to_a_round_rate() {
    let d = out_dir("ms_rounded_controls");
    for rate in [FrameRate::FPS_23_976, FrameRate::FPS_29_97, FrameRate::FPS_59_94] {
        let path = d.join(format!("ms-{}.mp4", rate.num));
        write_movie(&path, &ms_rounded(rate, 60));
        assert_eq!(effectcraft_media::probe(&path).expect("probe").frame_rate, rate);
    }
    // Frames of 40, 45 and 50 ms: the reader's median (45 ms, 22.2 fps) stands.
    let path = d.join("variable.mp4");
    write_movie(&path, &[40, 45, 50].repeat(20));
    let f = effectcraft_media::probe(&path).expect("probe");
    assert!(!FrameRate::COMMON.contains(&f.frame_rate) && (f.frame_rate.as_f64() - 1000.0 / 45.0).abs() < 0.01, "{:?}", f.frame_rate);
}

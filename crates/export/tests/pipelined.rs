//! Movie and WebM exports render the next batch of frames while the current one encodes (G5
//! B1). The output must be the bytes of the serial path (render a batch, encode it, render the
//! next), and an error or a stop on the encode side must end the export without hanging.
//!
//! The serial path: an export started on a rayon worker renders its batches one after another
//! (a worker must not block on frames rendered by its own pool). A pool of the global pool's
//! size gives the same batch size, so both runs cut the same clusters and audio blocks.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use effectcraft_color::Label;
use effectcraft_export::render_queue::{AudioOutput, Channels, OutputFormat, OutputModule, RenderSettings, TimeSpan, WebmVideoCodec};
use effectcraft_export::{ExportError, Job, Progress, Result, export};
use effectcraft_keyframe::{Keyframe, Value};
use effectcraft_media::MediaPool;
use effectcraft_project::{Comp, ItemId, ItemKind, LayerSource, Project, Solid, build};
use effectcraft_time::{FrameRate, Tick};

/// Frames per batch of the export path (`batch_size`).
fn batch() -> u64 {
    rayon::current_num_threads().clamp(2, 16) as u64
}

fn tone(path: &std::path::Path, secs: f64) {
    let sr = 48_000u32;
    let n = (secs * sr as f64) as usize;
    let mut data = Vec::with_capacity(n * 4);
    for i in 0..n {
        let s = (0.4 * (2.0 * std::f64::consts::PI * 330.0 * i as f64 / sr as f64).sin() * 32767.0).round() as i16;
        data.extend_from_slice(&s.to_le_bytes());
        data.extend_from_slice(&(s / 2).to_le_bytes());
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

/// 64×48 @ 10 fps over three and a bit batches: a half-transparent solid moving across, and a
/// tone (audio blocks are cut at batch ends).
fn project(name: &str) -> (Project, ItemId, u64) {
    let frames = batch() * 3 + 1;
    let d = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-out/export/pipelined").join(name);
    std::fs::create_dir_all(&d).expect("mkdir");
    let wav = d.join("tone.wav");
    tone(&wav, frames as f64 / 10.0 + 1.0);
    let mut p = Project::default();
    let mut comp = Comp::new(64, 48, FrameRate::new(10, 1), Tick::from_seconds_f64(frames as f64 / 10.0));
    comp.background = [0.1, 0.2, 0.4];
    let sid = p.add_item("Solid", Label::Red, None, ItemKind::Solid(Solid { color: [1.0, 0.5, 0.0], width: 24, height: 24, pixel_aspect: 1.0 }));
    let mut l = build::layer(&mut p, &comp, "Solid", LayerSource::Solid { item: sid }, (24, 24), None);
    let end = Tick::from_seconds_f64(frames as f64 / 10.0);
    l.props.prop_mut("transform/position").expect("position").keys =
        vec![Keyframe::new(Tick::ZERO, Value::Vec3([0.0, 10.0, 0.0])), Keyframe::new(end, Value::Vec3([64.0, 40.0, 0.0]))];
    l.props.prop_mut("transform/opacity").expect("opacity").keys =
        vec![Keyframe::new(Tick::ZERO, Value::Scalar(30.0)), Keyframe::new(end, Value::Scalar(100.0))];
    comp.layers.push(l);
    let f = effectcraft_media::probe(&wav).expect("probe wav");
    let fid = p.add_item("tone.wav", Label::SeaFoam, None, ItemKind::Footage(f));
    let l = build::layer(&mut p, &comp, "tone", LayerSource::Footage { item: fid }, (0, 0), None);
    comp.layers.push(l);
    let cid = p.add_item("Comp", Label::Sandstone, None, ItemKind::Comp(comp.into()));
    (p, cid, frames)
}

/// Export into memory; `stop_after` makes the progress callback stop once that many frames are done.
fn run(p: &Project, cid: ItemId, om: &OutputModule, stop_after: Option<u64>) -> (Result<effectcraft_export::Report>, Vec<u8>, Vec<Progress>) {
    let pool = MediaPool::new();
    let settings = RenderSettings { time_span: TimeSpan::LengthOfComp, ..Default::default() };
    let bytes = Arc::new(Mutex::new(Vec::new()));
    let out = bytes.clone();
    let sink = move |_: &str, data: Vec<u8>| *out.lock().unwrap() = data;
    let job = Job {
        project: p,
        footage: &pool,
        expr: None,
        accel: None,
        comp: cid,
        settings: &settings,
        output: om,
        path: "/mem/out",
        sink: Some(&sink),
        options: Default::default(),
        nested_switches: true,
    };
    let mut calls = vec![];
    let r = export(&job, &mut |pr| {
        calls.push(*pr);
        stop_after.is_none_or(|n| pr.done < n)
    });
    let data = std::mem::take(&mut *bytes.lock().unwrap());
    (r, data, calls)
}

fn modules() -> Vec<(&'static str, OutputModule)> {
    let with_audio = |f| OutputModule { audio: AudioOutput::On, ..OutputModule::for_format(f) };
    vec![
        ("h264", with_audio(OutputFormat::H264)),
        ("prores", with_audio(OutputFormat::ProRes)),
        ("webm-vp9-alpha", OutputModule { channels: Channels::Rgba, ..with_audio(OutputFormat::WebM) }),
        ("webm-av1", OutputModule { webm_codec: WebmVideoCodec::Av1, ..with_audio(OutputFormat::WebM) }),
    ]
}

#[test]
fn pipelined_exports_match_the_serial_path() {
    let (p, cid, frames) = project("same");
    let serial_pool = rayon::ThreadPoolBuilder::new().num_threads(rayon::current_num_threads()).build().unwrap();
    for (name, om) in modules() {
        let (r, piped, calls) = run(&p, cid, &om, None);
        let r = r.unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(r.frames, frames, "{name}");
        assert!(r.audio, "{name}");
        // Progress after every batch, in order, as before.
        let done: Vec<u64> = calls.iter().map(|c| c.done).collect();
        let want: Vec<u64> = std::iter::once(0).chain((1..=frames.div_ceil(batch())).map(|b| (b * batch()).min(frames))).collect();
        assert_eq!(done, want, "{name}: progress");
        let (r2, serial, _) = serial_pool.install(|| run(&p, cid, &om, None));
        r2.unwrap_or_else(|e| panic!("{name} serial: {e}"));
        let _ = std::fs::write(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-out/export/pipelined/same").join(name), &piped);
        assert!(piped.len() > 1000, "{name}: {} bytes", piped.len());
        assert!(piped == serial, "{name}: pipelined output differs from the serial path ({} vs {} bytes)", piped.len(), serial.len());
    }
}

/// Run `f` on its own thread; fail instead of hanging.
fn within<T: Send + 'static>(what: &str, f: impl FnOnce() -> T + Send + 'static) -> T {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(f());
    });
    rx.recv_timeout(Duration::from_secs(300)).unwrap_or_else(|_| panic!("{what}: export hung"))
}

#[test]
fn a_stop_on_the_encode_side_ends_the_export() {
    // The callback stops after the first batch: the encode side returns while the renderer has
    // the next batch ready (blocked on the hand-off) or in flight.
    for (name, om) in modules() {
        let calls = within(name, move || {
            let (p, cid, _) = project("stop");
            let (r, _, calls) = run(&p, cid, &om, Some(1));
            assert!(matches!(r, Err(ExportError::Cancelled)), "{name}: {r:?}");
            calls
        });
        assert_eq!(calls.last().map(|c| c.done), Some(batch()), "{name}: stopped after the first batch");
    }
}

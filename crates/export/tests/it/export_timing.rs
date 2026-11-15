//! The summary line of an export (#3713): where the time went and what ran the job.

use std::sync::{Arc, Mutex};

use effectcraft_color::Label;
use effectcraft_export::render_queue::{OutputFormat, OutputModule, RenderLog, RenderSettings, TimeSpan};
use effectcraft_export::{Job, JobOptions, Sink, export};
use effectcraft_project::{Comp, ItemId, ItemKind, LayerSource, Project, Solid, build};
use effectcraft_render::NoFootage;
use effectcraft_time::{FrameRate, Tick};

fn project() -> (Project, ItemId) {
    let mut p = Project::default();
    let comp = Comp::new(64, 48, FrameRate::new(30, 1), Tick::from_seconds_f64(1.0));
    let sid = p.add_item("Solid", Label::Red, None, ItemKind::Solid(Solid { color: [0.9, 0.5, 0.1], width: 64, height: 48, pixel_aspect: 1.0 }));
    let layer = build::layer(&mut p, &comp, "Solid", LayerSource::Solid { item: sid }, (64, 48), None);
    let mut comp = comp;
    comp.layers = vec![layer].into();
    let cid = p.add_item("Comp 1", Label::Sandstone, None, ItemKind::Comp(comp.into()));
    (p, cid)
}

#[test]
fn a_movie_export_reports_where_the_time_went() {
    let (p, cid) = project();
    let settings = RenderSettings { time_span: TimeSpan::LengthOfComp, ..Default::default() };
    let om = OutputModule::for_format(OutputFormat::H264);
    let files: Arc<Mutex<Vec<(String, Vec<u8>)>>> = Arc::default();
    let sink_files = files.clone();
    let sink: &Sink = &move |path: &str, data: Vec<u8>| sink_files.lock().unwrap().push((path.to_string(), data));
    let job = Job {
        project: &p,
        footage: &NoFootage,
        expr: None,
        accel: None,
        comp: cid,
        settings: &settings,
        output: &om,
        path: "/out/timing.mp4",
        sink: Some(sink),
        options: JobOptions { log: RenderLog::PlusSettings, label: "#1 Comp 1".into(), ..Default::default() },
        nested_switches: true,
    };
    let r = export(&job, &mut |_| true).expect("export");
    let t = &r.timing;
    assert_eq!(t.frames, 30);
    assert!(t.wall_seconds > 0.0);
    // The movie formats time encoding and writing apart from rendering.
    let (encode, write) = (t.encode_seconds.expect("encode time"), t.write_seconds.expect("write time"));
    for (name, secs) in [("render", t.render_seconds), ("encode", encode), ("write", write)] {
        assert!(secs >= 0.0 && secs <= t.wall_seconds, "{name} {secs} s of {} s wall", t.wall_seconds);
    }
    assert!(t.render_seconds > 0.0 && encode > 0.0, "{t:?}");
    assert_eq!(t.audio_seconds, None);
    assert!((t.frames_per_second - 30.0 / t.wall_seconds).abs() < 1e-9);
    assert!(t.renderer.starts_with("Software Rendering"), "{}", t.renderer);
    assert!(t.encoder.contains("H.264") && t.encoder.contains("software"), "{}", t.encoder);

    let line = t.summary();
    for part in
        ["export took", "30 frames", "frames/s", "overlap", "rendering frames", "encoder", "writing output", "Compositor: Software Rendering", "Encoder: H.264"]
    {
        assert!(line.contains(part), "{part:?} missing from {line}");
    }
    // The render log carries the same line.
    let files = files.lock().unwrap();
    let log = files.iter().find(|(path, _)| path.ends_with(".txt")).map(|(_, d)| String::from_utf8_lossy(d).into_owned()).expect("render log");
    assert!(log.contains(&format!("Timing: {line}")), "{log}");
}

fn job_of<'a>(p: &'a Project, cid: ItemId, settings: &'a RenderSettings, om: &'a OutputModule, path: &'a str, sink: &'a Sink) -> Job<'a> {
    Job {
        project: p,
        footage: &NoFootage,
        expr: None,
        accel: None,
        comp: cid,
        settings,
        output: om,
        path,
        sink: Some(sink),
        options: Default::default(),
        nested_switches: true,
    }
}

#[test]
fn an_image_sequence_says_the_stages_are_measured_together() {
    let (p, cid) = project();
    let settings = RenderSettings { time_span: TimeSpan::LengthOfComp, ..Default::default() };
    let om = OutputModule::for_format(OutputFormat::PngSequence);
    let sink: &Sink = &|_: &str, _: Vec<u8>| {};
    let r = export(&job_of(&p, cid, &settings, &om, "/out/timing_[#####].png", sink), &mut |_| true).expect("export");
    let t = &r.timing;
    assert_eq!((t.encode_seconds, t.write_seconds, t.encode_write_seconds), (None, None, None));
    assert!(t.render_seconds <= t.wall_seconds);
    assert!(t.summary().contains("rendering, encoding and writing measured together"), "{}", t.summary());
}

#[test]
fn gif_and_webm_time_the_stages_they_have() {
    let (p, cid) = project();
    let settings = RenderSettings { time_span: TimeSpan::LengthOfComp, ..Default::default() };
    let sink: &Sink = &|_: &str, _: Vec<u8>| {};
    let gif = OutputModule::for_format(OutputFormat::Gif);
    let t = export(&job_of(&p, cid, &settings, &gif, "/out/timing.gif", sink), &mut |_| true).expect("gif").timing;
    assert_eq!((t.encode_seconds, t.write_seconds), (None, None));
    let both = t.encode_write_seconds.expect("encode and write time");
    assert!(both >= 0.0 && both <= t.wall_seconds && t.render_seconds <= t.wall_seconds);
    assert!(t.summary().contains("encoding and writing output measured together"), "{}", t.summary());

    let webm = OutputModule::for_format(OutputFormat::WebM);
    let t = export(&job_of(&p, cid, &settings, &webm, "/out/timing.webm", sink), &mut |_| true).expect("webm").timing;
    let (e, w) = (t.encode_seconds.expect("encode"), t.write_seconds.expect("write"));
    assert!(e <= t.wall_seconds && w <= t.wall_seconds && t.render_seconds <= t.wall_seconds, "{t:?}");
}

#[test]
fn a_stopped_export_has_a_summary_without_a_frame_count() {
    let t = effectcraft_export::render_queue::RenderTiming {
        wall_seconds: 2.0,
        render_seconds: 1.5,
        renderer: "Software Rendering (x)".into(),
        encoder: "H.264".into(),
        ..Default::default()
    };
    let line = t.summary_of_unfinished("stopped");
    assert!(line.starts_with("export stopped after 2.00 s. Busy time per stage"), "{line}");
    assert!(!line.contains("frames/s"), "{line}");
}

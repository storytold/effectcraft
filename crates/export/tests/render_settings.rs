//! Render Settings and Output Module features end to end, through a [`Sink`] and native
//! temporary files: field rendering and 3:2 pulldown, crop / region of interest / resize, alpha-only
//! and premultiplied output, the solo and guide-layer overrides, the render log, storage
//! overflow, PCM audio formats and the compositor a job renders on.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use effectcraft_color::Label;
use effectcraft_effects::Buf;
use effectcraft_export::render_queue::*;
use effectcraft_export::{Job, JobOptions, Report, Sink, StorageQuota, export};
use effectcraft_keyframe::{Keyframe, Value};
use effectcraft_project::{Comp, ItemId, ItemKind, LayerSource, Project, Solid, build};
use effectcraft_render::{Accelerator, AutoPick, FxStep, Image, NoFootage, Renderer};
use effectcraft_time::{FrameRate, Tick};

const W: u32 = 64;
const H: u32 = 48;

fn disk_job<'a>(p: &'a Project, cid: ItemId, settings: &'a RenderSettings, output: &'a OutputModule, path: &'a str) -> Job<'a> {
    Job {
        project: p,
        footage: &NoFootage,
        expr: None,
        accel: None,
        comp: cid,
        settings,
        output,
        path,
        sink: None,
        nested_switches: true,
        options: JobOptions::default(),
    }
}

/// Tests intentionally overwriting an old export use the same review/acceptance API
/// as an interactive host.
fn export_reviewed(job: &Job<'_>, progress: &mut dyn FnMut(&effectcraft_export::Progress) -> bool) -> effectcraft_export::Result<Report> {
    let reviewed = effectcraft_export::preflight(job)?;
    effectcraft_export::export_authorized(job, &reviewed.authorization, progress)
}

/// A comp at `rate`, black, with a white bar 8 px wide sweeping left to right (64 px/s) and a
/// static red block at the bottom left; `extra` adds layers.
fn project(rate: FrameRate, extra: impl FnOnce(&mut Project, &mut Comp)) -> (Project, ItemId) {
    let mut p = Project::default();
    let mut comp = Comp::new(W, H, rate, Tick::from_seconds_f64(2.0));
    comp.background = [0.0, 0.0, 0.0];
    let bar = p.add_item("Bar", Label::Red, None, ItemKind::Solid(Solid { color: [1.0, 1.0, 1.0], width: 8, height: 32, pixel_aspect: 1.0 }));
    let mut l = build::layer(&mut p, &comp, "Bar", LayerSource::Solid { item: bar }, (8, 32), None);
    l.props.prop_mut("transform/position").unwrap().keys =
        vec![Keyframe::new(Tick::ZERO, Value::Vec3([0.0, 16.0, 0.0])), Keyframe::new(Tick::from_seconds_f64(1.0), Value::Vec3([64.0, 16.0, 0.0]))];
    comp.layers.push(l);
    let red = p.add_item("Block", Label::Red, None, ItemKind::Solid(Solid { color: [1.0, 0.0, 0.0], width: 16, height: 8, pixel_aspect: 1.0 }));
    let mut b = build::layer(&mut p, &comp, "Block", LayerSource::Solid { item: red }, (16, 8), None);
    b.props.prop_mut("transform/position").unwrap().value = Value::Vec3([8.0, 44.0, 0.0]);
    comp.layers.push(b);
    extra(&mut p, &mut comp);
    let cid = p.add_item("Comp", Label::Sandstone, None, ItemKind::Comp(comp.into()));
    (p, cid)
}

type Files = Arc<Mutex<Vec<(String, Vec<u8>)>>>;

fn custom(start: f64, end: f64) -> RenderSettings {
    RenderSettings { time_span: TimeSpan::Custom { start: Tick::from_seconds_f64(start), end: Tick::from_seconds_f64(end) }, ..Default::default() }
}

fn run_with(p: &Project, cid: ItemId, s: &RenderSettings, om: &OutputModule, path: &str, options: JobOptions, cancel: bool) -> (Result<Report, String>, Files) {
    run_on(p, cid, s, om, path, options, None, cancel)
}

/// [`run_with`] with an accelerator.
fn run_on(
    p: &Project,
    cid: ItemId,
    s: &RenderSettings,
    om: &OutputModule,
    path: &str,
    options: JobOptions,
    accel: Option<&dyn Accelerator>,
    cancel: bool,
) -> (Result<Report, String>, Files) {
    let files: Files = Default::default();
    let f = files.clone();
    let sink: Box<Sink> = Box::new(move |path: &str, data: Vec<u8>| f.lock().unwrap().push((path.to_string(), data)));
    let job = Job {
        project: p,
        footage: &NoFootage,
        expr: None,
        accel,
        comp: cid,
        settings: s,
        output: om,
        path,
        sink: Some(&*sink),
        options,
        nested_switches: true,
    };
    let r = export(&job, &mut |pr| !(cancel && pr.done > 0)).map_err(|e| e.to_string());
    (r, files)
}

fn run(p: &Project, cid: ItemId, s: &RenderSettings, om: &OutputModule, path: &str) -> Files {
    let (r, files) = run_with(p, cid, s, om, path, JobOptions::default(), false);
    r.expect("export");
    files
}

fn png(files: &Files, i: usize) -> image::RgbaImage {
    let f = files.lock().unwrap();
    let pngs: Vec<&(String, Vec<u8>)> = f.iter().filter(|(p, _)| p.ends_with(".png")).collect();
    let mut pngs = pngs;
    pngs.sort_by(|a, b| a.0.cmp(&b.0));
    image::load_from_memory(&pngs[i].1).expect("png").to_rgba8()
}

fn row(img: &image::RgbaImage, y: u32) -> Vec<u8> {
    (0..img.width()).flat_map(|x| img.get_pixel(x, y).0).collect()
}

#[test]
fn field_render_interleaves_two_times() {
    let (p, cid) = project(FrameRate::new(25, 1), |_, _| {});
    let om = OutputModule::for_format(OutputFormat::PngSequence);
    // Frame 10 at 25 fps with fields: 0.40 s and 0.42 s.
    for (fr, upper) in [(FieldRender::UpperFirst, true), (FieldRender::LowerFirst, false)] {
        let s = RenderSettings { field_render: fr, ..custom(0.4, 0.44) };
        let fields = png(&run(&p, cid, &s, &om, "/f/field_[#####].png"), 0);
        // Progressive references at 50 fps: frames 20 (0.40 s) and 21 (0.42 s).
        let r = RenderSettings { frame_rate: Some(FrameRate::new(50, 1)), ..custom(0.4, 0.44) };
        let refs = run(&p, cid, &r, &om, "/f/ref_[#####].png");
        let (a, b) = (png(&refs, 0), png(&refs, 1));
        assert_ne!(row(&a, 10), row(&b, 10), "the bar moved between the fields");
        for y in 0..H {
            let first = (y % 2 == 0) == upper;
            assert_eq!(row(&fields, y), row(if first { &a } else { &b }, y), "{fr:?}: line {y}");
        }
    }
}

#[test]
fn pulldown_follows_the_cadence() {
    let ntsc = FrameRate::new(30000, 1001);
    let (p, cid) = project(ntsc, |_, _| {});
    let om = OutputModule::for_format(OutputFormat::PngSequence);
    let five = 5.0 * 1001.0 / 30000.0;
    let s = RenderSettings { field_render: FieldRender::UpperFirst, pulldown: Pulldown::Wssww, ..custom(0.0, five) };
    let out = run(&p, cid, &s, &om, "/p/pd_[#####].png");
    assert_eq!(out.lock().unwrap().len(), 5);
    // Film frames at 23.976 fps.
    let r = RenderSettings { frame_rate: Some(FrameRate::new(24000, 1001)), ..custom(0.0, 0.2) };
    let film = run(&p, cid, &r, &om, "/p/film_[#####].png");
    let mut pattern = String::new();
    for k in 0..5 {
        let (a, b) = Pulldown::Wssww.fields(k as u64).unwrap();
        pattern.push(if a == b { 'W' } else { 'S' });
        let img = png(&out, k);
        let (fa, fb) = (png(&film, a as usize), png(&film, b as usize));
        for y in 0..H {
            assert_eq!(row(&img, y), row(if y % 2 == 0 { &fa } else { &fb }, y), "frame {k} line {y}");
        }
    }
    assert_eq!(pattern, "WSSWW");
}

#[test]
fn crop_roi_and_resize() {
    let (p, cid) = project(FrameRate::new(25, 1), |_, _| {});
    let s = custom(0.4, 0.44);
    let full = png(&run(&p, cid, &s, &OutputModule::for_format(OutputFormat::PngSequence), "/c/full_[#####].png"), 0);
    let mut om = OutputModule::for_format(OutputFormat::PngSequence);
    om.crop = Crop { enabled: true, top: 4, left: 10, bottom: 6, right: 2, ..Default::default() };
    let c = png(&run(&p, cid, &s, &om, "/c/crop_[#####].png"), 0);
    assert_eq!((c.width(), c.height()), (52, 38));
    for (x, y) in [(0, 0), (20, 11), (51, 37), (3, 30)] {
        assert_eq!(c.get_pixel(x, y), full.get_pixel(x + 10, y + 4), "({x}, {y})");
    }
    // Region of interest at half resolution.
    om.crop = Crop { enabled: true, use_roi: true, roi: Some([16.0, 8.0, 32.0, 32.0]), ..Default::default() };
    let half = RenderSettings { resolution: 0.5, ..s.clone() };
    let r = png(&run(&p, cid, &half, &om, "/c/roi_[#####].png"), 0);
    assert_eq!((r.width(), r.height()), (16, 16));
    // Resize (fixed size, low and high quality) after the crop.
    om.crop = Crop::default();
    for q in [ResizeQuality::Low, ResizeQuality::High] {
        om.resize = Resize { enabled: true, width: 32, height: 20, lock_aspect: false, quality: q };
        let z = png(&run(&p, cid, &s, &om, "/c/resize_[#####].png"), 0);
        assert_eq!((z.width(), z.height()), (32, 20));
        // The red block (bottom left) survives the resize.
        let px = z.get_pixel(4, 18).0;
        assert!(px[0] > 150 && px[1] < 80, "{q:?}: {px:?}");
    }
    om.resize.lock_aspect = true;
    let z = png(&run(&p, cid, &s, &om, "/c/aspect_[#####].png"), 0);
    assert_eq!((z.width(), z.height()), (32, 24));
}

#[test]
fn alpha_only_and_premultiplied() {
    let (p, cid) = project(FrameRate::new(25, 1), |p, comp| {
        // A half-transparent green layer over the right half.
        let g = p.add_item("Green", Label::Green, None, ItemKind::Solid(Solid { color: [0.0, 1.0, 0.0], width: 32, height: 48, pixel_aspect: 1.0 }));
        let mut l = build::layer(p, comp, "Green", LayerSource::Solid { item: g }, (32, 48), None);
        l.props.prop_mut("transform/position").unwrap().value = Value::Vec3([48.0, 24.0, 0.0]);
        l.props.prop_mut("transform/opacity").unwrap().value = Value::Scalar(50.0);
        comp.layers.insert(0, l);
    });
    let s = custom(0.0, 0.04);
    let mut om = OutputModule::for_format(OutputFormat::PngSequence);
    om.channels = Channels::Alpha;
    let a = png(&run(&p, cid, &s, &om, "/a/alpha_[#####].png"), 0);
    assert_eq!(a.get_pixel(60, 2).0[0], 128, "half-opaque green");
    assert_eq!(a.get_pixel(20, 2).0[0], 0, "transparent");
    assert_eq!(a.get_pixel(8, 44).0[0], 255, "opaque block");
    om.channels = Channels::Rgba;
    let straight = png(&run(&p, cid, &s, &om, "/a/s_[#####].png"), 0);
    om.alpha_mode = AlphaMode::Premultiplied;
    let premul = png(&run(&p, cid, &s, &om, "/a/p_[#####].png"), 0);
    assert_eq!(straight.get_pixel(60, 2).0, [0, 255, 0, 128]);
    assert_eq!(premul.get_pixel(60, 2).0, [0, 128, 0, 128]);
}

#[test]
fn solo_and_guide_layer_overrides() {
    let (p, cid) = project(FrameRate::new(25, 1), |p, comp| {
        let g = p.add_item("Guide", Label::Green, None, ItemKind::Solid(Solid { color: [0.0, 0.0, 1.0], width: 8, height: 8, pixel_aspect: 1.0 }));
        let mut l = build::layer(p, comp, "Guide", LayerSource::Solid { item: g }, (8, 8), None);
        l.props.prop_mut("transform/position").unwrap().value = Value::Vec3([56.0, 44.0, 0.0]);
        l.switches.guide = true;
        comp.layers.insert(0, l);
        // Solo the moving bar.
        comp.layers[1].switches.solo = true;
    });
    let om = OutputModule::for_format(OutputFormat::PngSequence);
    let s = custom(0.4, 0.44);
    let cur = png(&run(&p, cid, &s, &om, "/s/cur_[#####].png"), 0);
    assert_eq!(cur.get_pixel(8, 44).0[0], 0, "solo: the red block is hidden");
    assert_eq!(cur.get_pixel(56, 44).0[2], 0, "guide layers off by default");
    let off = RenderSettings { solo: CurrentOrOff::AllOff, guide_layers: CurrentOrOff::Current, ..s.clone() };
    let all = png(&run(&p, cid, &off, &om, "/s/off_[#####].png"), 0);
    assert_eq!(all.get_pixel(8, 44).0[0], 255, "Solo Switches: All Off");
    // (with solo off the guide renders too: Guide Layers: Current Settings)
    assert_eq!(all.get_pixel(56, 44).0[2], 255);
}

#[test]
fn render_logs() {
    let (p, cid) = project(FrameRate::new(25, 1), |_, _| {});
    let om = OutputModule::for_format(OutputFormat::PngSequence);
    let s = RenderSettings { field_render: FieldRender::UpperFirst, name: "DV-ish".into(), ..custom(0.0, 0.2) };
    let log_of = |files: &Files| {
        files.lock().unwrap().iter().find(|(p, _)| p.ends_with("_RenderLog.txt")).map(|(p, d)| (p.clone(), String::from_utf8(d.clone()).unwrap()))
    };
    // Errors Only + success: no log.
    let (r, files) = run_with(&p, cid, &s, &om, "/l/a_[#####].png", JobOptions::default(), false);
    assert!(r.unwrap().log.is_none());
    assert!(log_of(&files).is_none());
    // Plus Per Frame Info: settings and one line per frame.
    let opts = JobOptions { log: RenderLog::PlusPerFrameInfo, label: "#1 Comp".into(), ..Default::default() };
    let (r, files) = run_with(&p, cid, &s, &om, "/l/b_[#####].png", opts, false);
    let r = r.unwrap();
    let (path, text) = log_of(&files).expect("log");
    assert_eq!(r.log.as_deref(), Some(path.as_str()));
    assert_eq!(std::path::Path::new(&path), std::path::Path::new("/l/b_RenderLog.txt"));
    assert!(text.contains("Item: #1 Comp") && text.contains("Result: Done (5 frames"), "{text}");
    assert!(text.contains("Field Render: Upper Field First") && text.contains("Template: DV-ish") && text.contains("Format: PNG Sequence"), "{text}");
    assert_eq!(text.lines().filter(|l| l.contains("rendered in")).count(), 5, "{text}");
    assert!(text.contains("Frame 0 (0.000 s): rendered in") && text.contains("Frame 4 (0.160 s)"), "{text}");
    // Plus Settings: no per-frame lines.
    let opts = JobOptions { log: RenderLog::PlusSettings, ..Default::default() };
    let (_, files) = run_with(&p, cid, &s, &om, "/l/c.gif", opts, false);
    let (_, text) = log_of(&files).expect("log");
    assert!(text.contains("Render Settings:") && !text.contains("rendered in"), "{text}");
    // Errors Only + failure (the composition is gone): the error is logged.
    let gif = OutputModule::for_format(OutputFormat::Gif);
    let (r, files) = run_with(&p, ItemId(424_242), &s, &gif, "/l/d.gif", JobOptions::default(), false);
    assert!(r.is_err());
    let (path, text) = log_of(&files).expect("error log");
    assert_eq!(std::path::Path::new(&path), std::path::Path::new("/l/d_RenderLog.txt"));
    assert!(text.contains("Result: Failed") && text.contains("Error: composition not found") && !text.contains("Render Settings:"), "{text}");
    // A user stop is not an error: no log.
    let (r, files) = run_with(&p, cid, &s, &gif, "/l/e.gif", JobOptions::default(), true);
    assert!(r.is_err());
    assert!(log_of(&files).is_none());
}

/// The primary folder holds `limit` bytes; overflow folders are unlimited.
struct Quota {
    limit: u64,
    used: Mutex<u64>,
}

impl StorageQuota for Quota {
    fn has_room(&self, path: &str, bytes: u64) -> bool {
        if !std::path::Path::new(path).starts_with("/primary") {
            return true;
        }
        let mut u = self.used.lock().unwrap();
        if *u + bytes > self.limit {
            return false;
        }
        *u += bytes;
        true
    }
}

#[test]
fn storage_overflow_moves_files_to_the_overflow_folder() {
    let (p, cid) = project(FrameRate::new(25, 1), |_, _| {});
    let om = OutputModule::for_format(OutputFormat::PngSequence);
    let s = RenderSettings { skip_existing: false, ..custom(0.0, 0.4) };
    let q = Quota { limit: 1, used: Mutex::new(0) };
    let opts = JobOptions { storage: Some(&q), overflow: vec!["/overflow".into()], log: RenderLog::PlusSettings, ..Default::default() };
    let (r, files) = run_with(&p, cid, &s, &om, "/primary/seq_[#####].png", opts, false);
    let r = r.unwrap();
    assert_eq!(r.overflow.len(), 10, "{:?}", r.overflow);
    let f = files.lock().unwrap();
    assert_eq!(
        f.iter()
            .filter(|(p, _)| std::path::Path::new(p).parent() == Some(std::path::Path::new("/overflow"))
                && std::path::Path::new(p).file_name().is_some_and(|n| n.to_string_lossy().starts_with("seq_"))
                && p.ends_with(".png"))
            .count(),
        10
    );
    let log = f.iter().find(|(p, _)| p.ends_with("_RenderLog.txt")).map(|(_, d)| String::from_utf8_lossy(d).to_string()).unwrap();
    assert!(log.contains("Storage overflow: 10 file(s)"), "{log}");
    drop(f);
    // Use Storage Overflow off: everything stays in place.
    let off = RenderSettings { storage_overflow: false, ..s };
    let q = Quota { limit: 1, used: Mutex::new(0) };
    let opts = JobOptions { storage: Some(&q), overflow: vec!["/overflow".into()], ..Default::default() };
    let (r, files) = run_with(&p, cid, &off, &om, "/primary/seq_[#####].png", opts, false);
    assert!(r.unwrap().overflow.is_empty());
    assert!(files.lock().unwrap().iter().all(|(p, _)| std::path::Path::new(p).starts_with("/primary")));
}

#[test]
fn wav_and_aiff_channels_and_formats() {
    let (p, cid) = project(FrameRate::new(25, 1), |_, _| {});
    let s = custom(0.0, 0.5);
    let mut om = OutputModule::for_format(OutputFormat::Wav);
    om.audio_channels = 1;
    om.audio_format = AudioFormat::S24;
    om.audio_sample_rate = 44_100;
    let files = run(&p, cid, &s, &om, "/w/a.wav");
    let d = files.lock().unwrap()[0].1.clone();
    assert_eq!(&d[..4], b"RIFF");
    let u16_at = |o: usize| u16::from_le_bytes([d[o], d[o + 1]]);
    let u32_at = |o: usize| u32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]]);
    assert_eq!((u16_at(20), u16_at(22), u32_at(24), u16_at(32), u16_at(34)), (1, 1, 44_100, 3, 24));
    assert_eq!(u32_at(40), 22_050 * 3, "half a second of mono 24-bit");
    om.audio_channels = 2;
    om.audio_format = AudioFormat::F32;
    let files = run(&p, cid, &s, &om, "/w/b.wav");
    let d = files.lock().unwrap()[0].1.clone();
    let u16_at = |o: usize| u16::from_le_bytes([d[o], d[o + 1]]);
    assert_eq!((u16_at(20), u16_at(22), u16_at(32), u16_at(34)), (3, 2, 8, 32), "IEEE float stereo");
    // Plain AIFF has no float: 32-bit float is AIFF-C with `fl32` samples (#317).
    let mut aiff = OutputModule::for_format(OutputFormat::Aiff);
    aiff.audio_format = AudioFormat::F32;
    let files = run(&p, cid, &s, &aiff, "/w/c.aif");
    let d = files.lock().unwrap()[0].1.clone();
    let u32_at = |o: usize| u32::from_be_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]]);
    assert_eq!((&d[..4], u32_at(4) as usize + 8, &d[8..12]), (&b"FORM"[..], d.len(), &b"AIFC"[..]));
    assert_eq!((&d[12..16], u32_at(16), u32_at(20)), (&b"FVER"[..], 4, 0xA280_5140));
    assert_eq!(&d[24..28], b"COMM");
    let comm = 32;
    assert_eq!(u32_at(28) as usize, 18 + 4 + 22, "COMM: AIFF fields, compression type, name");
    assert_eq!(u16::from_be_bytes([d[comm], d[comm + 1]]), 2, "stereo");
    assert_eq!(u32_at(comm + 2), 24_000, "half a second at 48 kHz");
    assert_eq!(u16::from_be_bytes([d[comm + 6], d[comm + 7]]), 32, "sample size");
    assert_eq!(&d[comm + 18..comm + 22], b"fl32");
    assert_eq!(&d[comm + 22..comm + 44], b"\x1532-bit floating point");
    let ssnd = comm + 44;
    assert_eq!((&d[ssnd..ssnd + 4], u32_at(ssnd + 4) as usize), (&b"SSND"[..], 8 + 24_000 * 2 * 4));
    // 16- and 24-bit stay plain AIFF.
    aiff.audio_format = AudioFormat::S24;
    let files = run(&p, cid, &s, &aiff, "/w/d.aif");
    let d = files.lock().unwrap()[0].1.clone();
    assert_eq!(&d[8..12], b"AIFF");
    assert_eq!(u16::from_be_bytes([d[26], d[27]]), 24, "sample size");
}

#[test]
fn cancelled_export_removes_only_files_it_opened() {
    use effectcraft_export::ExportError;
    let dir = std::env::temp_dir().join(format!("ec-cancel-owned-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (p, cid) = project(FrameRate::new(25, 1), |_, _| {});
    struct Overflow;
    impl StorageQuota for Overflow {
        fn has_room(&self, path: &str, _: u64) -> bool {
            path.contains("overflow")
        }
    }
    for overflow in [false, true] {
        let output_dir = if overflow { dir.join("overflow") } else { dir.clone() };
        std::fs::create_dir_all(&output_dir).unwrap();
        let settings = RenderSettings { skip_existing: true, storage_overflow: overflow, ..Default::default() };
        for format in [OutputFormat::PngSequence, OutputFormat::H264] {
            let om = OutputModule::for_format(format);
            let path = dir.join(if format.is_sequence() { "clip_[#####].png" } else { "clip.mp4" });
            let path = path.to_str().unwrap();
            let decoys = ["clip.aep", "clip2.psd", "clip000.mp4", "clip_99999.png", "clip_00000.png", "clip.txt", "clip_RenderLog.txt"];
            for name in decoys {
                std::fs::write(output_dir.join(name), b"keep").unwrap();
            }
            // Skip-existing checks the primary destination even when the new frames overflow.
            std::fs::write(dir.join("clip_00000.png"), b"keep").unwrap();
            let result = export(
                &Job {
                    project: &p,
                    footage: &NoFootage,
                    expr: None,
                    accel: None,
                    comp: cid,
                    settings: &settings,
                    output: &om,
                    path,
                    sink: None,
                    nested_switches: true,
                    options: JobOptions {
                        log: RenderLog::PlusPerFrameInfo,
                        storage: Some(&Overflow),
                        overflow: vec![output_dir.to_string_lossy().into_owned()],
                        ..Default::default()
                    },
                },
                &mut |progress| progress.done == 0,
            );
            assert!(matches!(result, Err(ExportError::Cancelled)), "{result:?}");
            for name in decoys {
                assert_eq!(std::fs::read(output_dir.join(name)).unwrap(), b"keep", "{name}");
            }
            let remaining: Vec<_> = std::fs::read_dir(&output_dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
            assert_eq!(remaining.len(), decoys.len(), "partial files remain: {remaining:?}");
        }
    }
    std::fs::remove_dir_all(dir).unwrap();
}

/// A GPU compositor stand-in that renders every frame solid green (the CPU draws the bar and the
/// block on black) and keeps Auto's timing history.
#[derive(Default)]
struct GreenAccel {
    pick: AutoPick,
    frames: AtomicUsize,
}

impl Accelerator for GreenAccel {
    fn name(&self) -> String {
        "green".into()
    }
    fn comp_frame(&self, r: &Renderer, comp: ItemId, _t: Tick) -> Option<Image> {
        self.frames.fetch_add(1, Ordering::SeqCst);
        let c = r.project.comp(comp)?;
        Some(Image::filled(c.width, c.height, [0.0, 1.0, 0.0, 1.0]))
    }
    fn supports_effect(&self, _id: &str) -> bool {
        false
    }
    fn effects(&self, _chain: &[FxStep], _buf: &Buf, _levels: Option<f32>) -> Option<Buf> {
        None
    }
    fn auto_pick(&self) -> Option<&AutoPick> {
        Some(&self.pick)
    }
}

#[test]
fn a_job_renders_on_one_compositor() {
    // #414: Auto chose the CPU or the GPU for each frame from measured frame times, so GPU
    // renders of the same frames differed run to run. A job renders every frame on the
    // project's renderer.
    let (mut p, cid) = project(FrameRate::new(25, 1), |_, _| {});
    let om = OutputModule::for_format(OutputFormat::PngSequence);
    let s = custom(0.0, 0.4);
    let render = |p: &Project, accel: &GreenAccel, path: &str| {
        let (r, files) = run_on(p, cid, &s, &om, path, JobOptions::default(), Some(accel), false);
        r.expect("export");
        (0..10).map(|i| png(&files, i)).collect::<Vec<_>>()
    };
    // Mercury GPU Acceleration: every frame on the GPU, the same in every run.
    p.settings.gpu_acceleration = true;
    let gpu = GreenAccel::default();
    let a = render(&p, &gpu, "/g/a_[#####].png");
    assert_eq!(gpu.frames.load(Ordering::SeqCst), 10);
    for (i, f) in a.iter().enumerate() {
        assert!(f.pixels().all(|px| px.0 == [0, 255, 0, 255]), "frame {i} rendered on the CPU");
    }
    assert_eq!(a, render(&p, &gpu, "/g/b_[#####].png"));
    // Mercury Software Only: every frame on the CPU.
    p.settings.gpu_acceleration = false;
    let cpu = GreenAccel::default();
    let c = render(&p, &cpu, "/g/c_[#####].png");
    assert_eq!(cpu.frames.load(Ordering::SeqCst), 0);
    assert_eq!(c[0].get_pixel(10, 44).0, [255, 0, 0, 255], "the CPU drew the red block");
}

/// Cancellation after an encoder has opened its output must preserve the prior export.
#[test]
fn cancelled_replacement_preserves_existing_movie_and_sequence() {
    let dir = std::env::temp_dir().join(format!("ec-preserve-export-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (p, cid) = project(FrameRate::new(25, 1), |_, _| {});
    for format in [OutputFormat::H264, OutputFormat::PngSequence] {
        let om = OutputModule::for_format(format);
        let path = dir.join(if format.is_sequence() { "existing_[#####].png" } else { "existing.mp4" });
        let final_path = if format.is_sequence() { dir.join("existing_00000.png") } else { path.clone() };
        std::fs::write(&final_path, b"previous complete export").unwrap();
        let result = export_reviewed(
            &Job {
                project: &p,
                footage: &NoFootage,
                expr: None,
                accel: None,
                comp: cid,
                settings: &RenderSettings::default(),
                output: &om,
                path: path.to_str().unwrap(),
                sink: None,
                nested_switches: true,
                options: JobOptions::default(),
            },
            &mut |progress| progress.done == 0,
        );
        assert!(matches!(result, Err(effectcraft_export::ExportError::Cancelled)));
        assert_eq!(std::fs::read(&final_path).ok().as_deref(), Some(b"previous complete export".as_slice()));
        std::fs::remove_file(final_path).unwrap();
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
    }
    std::fs::remove_dir(dir).unwrap();
}

/// A complete two-second export matches the sink output byte-for-byte, both for a new
/// destination and a replacement. This exercises the real muxer and parallel sequence path.
#[test]
fn published_movie_and_sequence_match_complete_sink_outputs() {
    let dir = tempfile::tempdir().unwrap();
    let (p, cid) = project(FrameRate::new(25, 1), |_, _| {});
    let settings = RenderSettings::default();
    for format in [OutputFormat::Uncompressed, OutputFormat::PngSequence] {
        let om = OutputModule::for_format(format);
        let path = dir.path().join(if format.is_sequence() { "movie_[#####].png" } else { "movie.mov" });
        let path = path.to_str().unwrap();
        let expected = run(&p, cid, &settings, &om, path);
        for replace in [false, true] {
            if replace {
                for (name, _) in expected.lock().unwrap().iter() {
                    std::fs::write(name, b"previous export").unwrap();
                }
            }
            let result = export_reviewed(
                &Job {
                    project: &p,
                    footage: &NoFootage,
                    expr: None,
                    accel: None,
                    comp: cid,
                    settings: &settings,
                    output: &om,
                    path,
                    sink: None,
                    nested_switches: true,
                    options: JobOptions::default(),
                },
                &mut |_| true,
            )
            .unwrap();
            assert_eq!(result.frames, 50);
            for (name, bytes) in expected.lock().unwrap().iter() {
                assert_eq!(&std::fs::read(name).unwrap(), bytes, "{name}");
            }
        }
        for (name, _) in expected.lock().unwrap().iter() {
            std::fs::remove_file(name).unwrap();
        }
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }
}

/// An invalid overflow destination must preserve all earlier complete outputs.
#[test]
fn sequence_write_failure_preserves_existing_frames() {
    let dir = tempfile::tempdir().unwrap();
    let blocked = dir.path().join("blocked-overflow");
    std::fs::write(&blocked, b"ordinary file, not a directory").unwrap();
    let path = dir.path().join("movie_[#####].png");
    let path = path.to_str().unwrap();
    for frame in 0..50 {
        std::fs::write(sequence_path(path, frame), b"old complete frame").unwrap();
    }
    struct FailAfterFirst(AtomicUsize);
    impl StorageQuota for FailAfterFirst {
        fn has_room(&self, path: &str, _: u64) -> bool {
            path.contains("blocked-overflow") || self.0.fetch_add(1, Ordering::SeqCst) == 0
        }
    }
    let storage = FailAfterFirst(AtomicUsize::new(0));
    let (p, cid) = project(FrameRate::new(25, 1), |_, _| {});
    let result = export_reviewed(
        &Job {
            project: &p,
            footage: &NoFootage,
            expr: None,
            accel: None,
            comp: cid,
            settings: &RenderSettings { storage_overflow: true, ..Default::default() },
            output: &OutputModule::for_format(OutputFormat::PngSequence),
            path,
            sink: None,
            nested_switches: true,
            options: JobOptions { storage: Some(&storage), overflow: vec![blocked.to_string_lossy().into_owned()], ..Default::default() },
        },
        &mut |_| true,
    );
    assert!(matches!(result, Err(effectcraft_export::ExportError::Io(_))), "{result:?}");
    for frame in 0..50 {
        assert_eq!(std::fs::read(sequence_path(path, frame)).unwrap(), b"old complete frame");
    }
    assert_eq!(std::fs::read(blocked).unwrap(), b"ordinary file, not a directory");
    assert!(std::fs::read_dir(dir.path()).unwrap().all(|entry| !entry.unwrap().file_name().to_string_lossy().starts_with(".effectcraft-")));
}

/// Lose a completed staging file immediately before publication through the real export
/// entry point. Preparation must reject the whole set, including every surviving frame.
#[test]
fn sequence_preparation_failure_publishes_no_frames() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("movie_[#####].png");
    let (p, cid) = project(FrameRate::new(25, 1), |_, _| {});
    let mut injected = false;
    let result = export(
        &Job {
            project: &p,
            footage: &NoFootage,
            expr: None,
            accel: None,
            comp: cid,
            settings: &RenderSettings::default(),
            output: &OutputModule::for_format(OutputFormat::PngSequence),
            path: path.to_str().unwrap(),
            sink: None,
            nested_switches: true,
            options: JobOptions::default(),
        },
        &mut |progress| {
            if progress.done == progress.total && progress.total > 0 {
                let mut staging = std::fs::read_dir(dir.path()).unwrap().next().unwrap().unwrap().path();
                if staging.is_dir() {
                    staging = std::fs::read_dir(staging).unwrap().next().unwrap().unwrap().path();
                }
                std::fs::remove_file(staging).unwrap();
                injected = true;
            }
            true
        },
    );
    assert!(injected);
    assert!(matches!(result, Err(effectcraft_export::ExportError::Io(_))), "{result:?}");
    // Failure logs have their separate, existing delivery contract. No frame or staging
    // file may remain; the only entry is the useful error log.
    let log = std::path::PathBuf::from(log_path(path.to_str().unwrap()));
    assert!(std::fs::read_to_string(&log).unwrap().contains("before publication"));
    let entries: Vec<_> = std::fs::read_dir(dir.path()).unwrap().map(|entry| entry.unwrap().path()).collect();
    assert_eq!(entries, vec![log]);
}

#[test]
fn preflight_is_read_only_and_authorized_new_movie_matches_sink() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("missing").join("subfolder").join("movie.mov");
    let (p, cid) = project(FrameRate::new(25, 1), |_, _| {});
    let settings = custom(0.0, 0.08);
    let output = OutputModule::for_format(OutputFormat::Uncompressed);
    let job = disk_job(&p, cid, &settings, &output, path.to_str().unwrap());
    let reviewed = effectcraft_export::preflight(&job).unwrap();
    assert!(reviewed.collisions.is_empty());
    assert!(!path.parent().unwrap().exists(), "inspection must not create folders");
    let expected = run(&p, cid, &settings, &output, job.path);
    effectcraft_export::export_authorized(&job, &reviewed.authorization, &mut |_| true).unwrap();
    for (name, bytes) in expected.lock().unwrap().iter() {
        assert_eq!(&std::fs::read(name).unwrap(), bytes);
    }
}

#[test]
fn existing_output_requires_confirmation_and_stale_review_preserves_external_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("movie.mov");
    std::fs::write(&path, b"old complete movie").unwrap();
    let (p, cid) = project(FrameRate::new(25, 1), |_, _| {});
    let settings = custom(0.0, 0.08);
    let output = OutputModule::for_format(OutputFormat::Uncompressed);
    let job = disk_job(&p, cid, &settings, &output, path.to_str().unwrap());
    let reviewed = effectcraft_export::preflight(&job).unwrap();
    assert_eq!(reviewed.collisions, vec![job.path]);
    let mut progressed = false;
    let error = export(&job, &mut |_| {
        progressed = true;
        true
    })
    .unwrap_err()
    .to_string();
    assert!(error.contains("confirmation required"), "{error}");
    assert!(!progressed);
    assert_eq!(std::fs::read(&path).unwrap(), b"old complete movie");
    std::fs::write(&path, b"external writer changed this after review").unwrap();
    let error = effectcraft_export::export_authorized(&job, &reviewed.authorization, &mut |_| true).unwrap_err().to_string();
    assert!(error.contains("changed since preflight"), "{error}");
    assert_eq!(std::fs::read(&path).unwrap(), b"external writer changed this after review");
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[test]
fn file_appearing_after_job_validation_is_not_an_approved_overwrite() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("movie_[#####].png");
    let external = dir.path().join("movie_00000.png");
    let (p, cid) = project(FrameRate::new(25, 1), |_, _| {});
    let settings = custom(0.0, 0.08);
    let output = OutputModule::for_format(OutputFormat::PngSequence);
    let job = disk_job(&p, cid, &settings, &output, path.to_str().unwrap());
    let reviewed = effectcraft_export::preflight(&job).unwrap();
    let mut injected = false;
    let error = effectcraft_export::export_authorized(&job, &reviewed.authorization, &mut |progress| {
        if progress.done == 0 && !injected {
            // export_inner has checked every target; the first staging open has not run.
            std::fs::write(&external, b"external creator").unwrap();
            injected = true;
        }
        true
    })
    .unwrap_err()
    .to_string();
    assert!(injected);
    assert!(error.contains("changed since preflight"), "{error}");
    assert_eq!(std::fs::read(&external).unwrap(), b"external creator");
    assert!(!dir.path().join("movie_00001.png").exists());
    assert!(std::fs::read_dir(dir.path()).unwrap().all(|entry| !entry.unwrap().file_name().to_string_lossy().starts_with(".effectcraft-")));
}

#[test]
fn authorization_cannot_be_reused_for_an_unreviewed_path_or_replaced_by_a_boolean() {
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a.mov");
    let b = dir.path().join("unreviewed").join("b.mov");
    let (p, cid) = project(FrameRate::new(25, 1), |_, _| {});
    let settings = custom(0.0, 0.08);
    let output = OutputModule::for_format(OutputFormat::Uncompressed);
    let a_job = disk_job(&p, cid, &settings, &output, a.to_str().unwrap());
    let b_job = disk_job(&p, cid, &settings, &output, b.to_str().unwrap());
    let reviewed = effectcraft_export::preflight(&a_job).unwrap();
    let error = effectcraft_export::export_authorized(&b_job, &reviewed.authorization, &mut |_| true).unwrap_err().to_string();
    assert!(error.contains("was not reviewed"), "{error}");
    let error = effectcraft_export::export_authorized(&a_job, &serde_json::json!(true), &mut |_| true).unwrap_err().to_string();
    assert!(error.contains("invalid export authorization"), "{error}");
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[test]
fn skip_existing_does_not_hide_an_overflow_replacement() {
    struct Overflow;
    impl StorageQuota for Overflow {
        fn has_room(&self, path: &str, _: u64) -> bool {
            path.contains("overflow")
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let overflow = dir.path().join("overflow");
    std::fs::create_dir(&overflow).unwrap();
    let path = dir.path().join("movie_[#####].png");
    let existing = overflow.join("movie_00000.png");
    std::fs::write(&existing, b"old overflow frame").unwrap();
    let (p, cid) = project(FrameRate::new(25, 1), |_, _| {});
    let settings = RenderSettings { skip_existing: true, storage_overflow: true, ..custom(0.0, 0.04) };
    let output = OutputModule::for_format(OutputFormat::PngSequence);
    let mut job = disk_job(&p, cid, &settings, &output, path.to_str().unwrap());
    job.options.storage = Some(&Overflow);
    job.options.overflow = vec![overflow.to_str().unwrap().to_string()];
    let reviewed = effectcraft_export::preflight(&job).unwrap();
    assert_eq!(reviewed.collisions, vec![existing.to_str().unwrap()]);
    assert!(export(&job, &mut |_| true).unwrap_err().to_string().contains("confirmation required"));
    let base = dir.path().join("movie_00000.png");
    std::fs::write(&base, b"already rendered").unwrap();
    let permissions = std::fs::metadata(&base).unwrap().permissions();
    let mut readonly = permissions.clone();
    readonly.set_readonly(true);
    std::fs::set_permissions(&base, readonly).unwrap();
    let reviewed = effectcraft_export::preflight(&job).unwrap();
    assert!(reviewed.collisions.is_empty());
    assert_eq!(export(&job, &mut |_| true).unwrap().bytes, 0);
    std::fs::set_permissions(&base, permissions).unwrap();
    assert_eq!(std::fs::read(existing).unwrap(), b"old overflow frame");
}

#[test]
fn render_log_collision_preserves_existing_file_and_returns_unique_paths() {
    for hardlink in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("movie.mov");
        let wanted = std::path::PathBuf::from(log_path(path.to_str().unwrap()));
        let unrelated = dir.path().join("unrelated-project.ecproj");
        std::fs::write(&unrelated, b"unrelated project bytes").unwrap();
        if hardlink {
            std::fs::hard_link(&unrelated, &wanted).unwrap();
        } else {
            std::fs::write(&wanted, b"previous render log").unwrap();
        }
        let original = std::fs::read(&wanted).unwrap();
        let (p, cid) = project(FrameRate::new(25, 1), |_, _| {});
        let settings = custom(0.0, 0.08);
        let output = OutputModule::for_format(OutputFormat::Uncompressed);
        let mut job = disk_job(&p, cid, &settings, &output, path.to_str().unwrap());
        job.options.log = RenderLog::PlusSettings;
        let first = export(&job, &mut |_| true).unwrap().log.unwrap();
        assert_eq!(std::fs::read(&wanted).unwrap(), original);
        assert_eq!(std::fs::read(&unrelated).unwrap(), b"unrelated project bytes");
        assert_ne!(std::path::Path::new(&first), wanted);
        let first_bytes = std::fs::read(&first).unwrap();
        std::fs::remove_file(&path).unwrap(); // Only this fixture's newly exported movie.
        let second = export(&job, &mut |_| true).unwrap().log.unwrap();
        assert_ne!(first, second);
        assert_eq!(std::fs::read(first).unwrap(), first_bytes);
        assert_eq!(std::fs::read(wanted).unwrap(), original);
    }
}

#[cfg(unix)]
#[test]
fn render_log_symlink_does_not_follow_or_truncate_an_unrelated_target() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("movie.mov");
    let wanted = std::path::PathBuf::from(log_path(path.to_str().unwrap()));
    let unrelated = dir.path().join("unrelated-project.ecproj");
    std::fs::write(&unrelated, b"unrelated project bytes").unwrap();
    std::os::unix::fs::symlink(&unrelated, &wanted).unwrap();
    let (p, cid) = project(FrameRate::new(25, 1), |_, _| {});
    let settings = custom(0.0, 0.08);
    let output = OutputModule::for_format(OutputFormat::Uncompressed);
    let mut job = disk_job(&p, cid, &settings, &output, path.to_str().unwrap());
    job.options.log = RenderLog::PlusSettings;
    let actual = export(&job, &mut |_| true).unwrap().log.unwrap();
    assert_ne!(std::path::Path::new(&actual), wanted);
    assert_eq!(std::fs::read_link(&wanted).unwrap(), unrelated);
    assert_eq!(std::fs::read(&wanted).unwrap(), b"unrelated project bytes");
}

/// Review more than the former path limit without encoding 100001 frames or creating them.
#[test]
fn long_fresh_sequences_and_overflow_keep_exact_bounded_authorization() {
    struct Room;
    impl StorageQuota for Room {
        fn has_room(&self, _: &str, _: u64) -> bool {
            true
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("f_[#####].png");
    let overflow = dir.path().join("overflow");
    let (p, cid) = project(FrameRate::new(25, 1), |_, comp| {
        comp.duration = Tick::from_seconds_f64(100_001.0 / 25.0);
    });
    let output = OutputModule::for_format(OutputFormat::PngSequence);
    for alternatives in [false, true] {
        // 50001 frames with one overflow folder yield100002 candidates.
        let time_span =
            if alternatives { TimeSpan::Custom { start: Tick::ZERO, end: Tick::from_seconds_f64(50_001.0 / 25.0) } } else { TimeSpan::LengthOfComp };
        let settings = RenderSettings { time_span, storage_overflow: alternatives, ..Default::default() };
        let mut job = disk_job(&p, cid, &settings, &output, path.to_str().unwrap());
        job.options.storage = alternatives.then_some(&Room);
        job.options.overflow = if alternatives { vec![overflow.to_str().unwrap().into()] } else { vec![] };
        let reviewed = effectcraft_export::preflight(&job).unwrap();
        let expected = if alternatives { 100_002 } else { 100_001 };
        assert!(reviewed.collisions.is_empty());
        assert_eq!(reviewed.destinations.len(), expected);
        assert_eq!(reviewed.authorization["targets"].as_object().unwrap().len(), expected);
        assert!(serde_json::to_vec(&reviewed.authorization).unwrap().len() <= 64 * 1024 * 1024);
        let last = sequence_path(job.path, if alternatives { 50_000 } else { 100_000 });
        let key = std::path::absolute(&last).unwrap();
        let entry = &reviewed.authorization["targets"][key.to_str().unwrap()];
        assert!(entry["state"].is_null());
        assert!(entry["resolved"].is_string());
        assert!(entry["anchor"].is_string());
        assert!(entry.get("anchorPolicy").is_some());
        // This validates every reviewed path and a token larger than the former count
        // limit, then cancels before opening staging or encoding the long sequence.
        let error = effectcraft_export::export_authorized(&job, &reviewed.authorization, &mut |_| false).unwrap_err();
        assert!(matches!(error, effectcraft_export::ExportError::Cancelled), "{error}");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }
}

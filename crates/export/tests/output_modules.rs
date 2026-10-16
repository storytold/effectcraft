//! A Render Queue item with several output modules exports each module as a job of its own,
//! back to back. A module's job continues from the layer cache of the module before it when
//! both render the same frames (G5): the files must be the bytes of each module exported alone.
//! With `export_modules` such modules export at once from one render of each frame.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use effectcraft_color::Label;
use effectcraft_export::render_queue::{OutputFormat, OutputModule, RenderSettings, TimeSpan};
use effectcraft_export::{ExportError, Job, Progress, export, export_modules};
use effectcraft_keyframe::{Keyframe, Value};
use effectcraft_media::MediaPool;
use effectcraft_project::build::{self, Ids};
use effectcraft_project::{Comp, ItemId, ItemKind, Layer, LayerSource, Project, Solid};
use effectcraft_time::{FrameRate, Tick};

fn solid(p: &mut Project, comp: &Comp, color: [f32; 3], w: u32, h: u32) -> Layer {
    let sid = p.add_item("Solid", Label::Red, None, ItemKind::Solid(Solid { color, width: w, height: h, pixel_aspect: 1.0 }));
    build::layer(p, comp, "Solid", LayerSource::Solid { item: sid }, (w, h), None)
}

/// 96×64 @ 10 fps, 12 frames: a blurred static solid (its buffer is cached) under a smaller solid
/// moving across.
fn project() -> (Project, ItemId) {
    project_sized(96, 64, false)
}

/// [`project`] at `w`×`h`; `animate`: the blur animates too (every frame renders it again).
fn project_sized(w: u32, h: u32, animate: bool) -> (Project, ItemId) {
    let mut p = Project::default();
    let mut comp = Comp::new(w, h, FrameRate::new(10, 1), Tick::from_seconds_f64(1.2));
    comp.background = [0.1, 0.2, 0.4];
    let mut bg = solid(&mut p, &comp, [0.9, 0.6, 0.1], w * 2 / 3, h * 5 / 8);
    let spec = effectcraft_effects::find("ec.blur.gaussian").expect("gaussian blur");
    let mut next = p.next_id;
    let mut fx = effectcraft_effects::instantiate(spec, &mut Ids(&mut next), spec.name, [(w * 2 / 3) as f64, (h * 5 / 8) as f64]);
    p.next_id = next;
    fx.prop_mut("blurriness").expect("blurriness").value = Value::Scalar(12.0);
    if animate {
        fx.prop_mut("blurriness").expect("blurriness").keys =
            vec![Keyframe::new(Tick::ZERO, Value::Scalar(4.0)), Keyframe::new(Tick::from_seconds_f64(1.2), Value::Scalar(40.0))];
    }
    bg.props.sub_mut("effects").expect("effects").children.push(fx.into());
    let mut fg = solid(&mut p, &comp, [0.2, 0.9, 0.3], 16, 16);
    fg.props.prop_mut("transform/position").expect("position").keys =
        vec![Keyframe::new(Tick::ZERO, Value::Vec3([0.0, 20.0, 0.0])), Keyframe::new(Tick::from_seconds_f64(1.2), Value::Vec3([96.0, 44.0, 0.0]))];
    comp.layers.push(fg);
    comp.layers.push(bg);
    let cid = p.add_item("Comp", Label::Sandstone, None, ItemKind::Comp(comp.into()));
    (p, cid)
}

fn job<'a>(p: &'a Project, cid: ItemId, footage: &'a MediaPool, settings: &'a RenderSettings, om: &'a OutputModule, path: &'a str) -> Job<'a> {
    Job { project: p, footage, expr: None, accel: None, comp: cid, settings, output: om, path, sink: None, options: Default::default(), nested_switches: true }
}

fn settings() -> RenderSettings {
    RenderSettings { time_span: TimeSpan::LengthOfComp, ..Default::default() }
}

/// Export one module into memory with `footage`: the files it wrote, by path.
fn run(p: &Project, cid: ItemId, footage: &MediaPool, om: &OutputModule, path: &str) -> BTreeMap<String, Vec<u8>> {
    let settings = settings();
    let files = Arc::new(Mutex::new(BTreeMap::new()));
    let out = files.clone();
    let sink = move |path: &str, data: Vec<u8>| {
        out.lock().unwrap().insert(path.to_string(), data);
    };
    let mut job = job(p, cid, footage, &settings, om, path);
    job.sink = Some(&sink);
    let r = export(&job, &mut |_| true);
    assert!(r.is_ok(), "{path}: {r:?}");
    std::mem::take(&mut *files.lock().unwrap())
}

#[test]
fn every_output_module_writes_the_bytes_it_writes_alone() {
    let (p, cid) = project();
    let modules = [
        (OutputModule::for_format(OutputFormat::H264), "/mem/out.mp4"),
        (OutputModule::for_format(OutputFormat::PngSequence), "/mem/out_[#####].png"),
        (OutputModule::for_format(OutputFormat::ProRes), "/mem/out.mov"),
    ];
    // Alone: each with a footage pool of its own, so no job continues from another's cache.
    let pools: Vec<MediaPool> = modules.iter().map(|_| MediaPool::new()).collect();
    let alone: Vec<_> = modules.iter().zip(&pools).map(|((om, path), pool)| run(&p, cid, pool, om, path)).collect();
    assert_eq!(alone[1].len(), 12, "one PNG per frame");
    // As one item: the same pool, back to back (modules after the first find the cache warm).
    let pool = MediaPool::new();
    for ((om, path), alone) in modules.iter().zip(&alone) {
        let item = run(&p, cid, &pool, om, path);
        assert!(!item.is_empty());
        assert!(item == *alone, "{path} differs from the module exported alone");
    }
}

/// Every module of a list exports from one render: the files are still those of each alone.
#[test]
fn modules_exported_together_write_the_bytes_they_write_alone() {
    let (p, cid) = project();
    let mut alpha = OutputModule::for_format(OutputFormat::PngSequence);
    alpha.channels = effectcraft_export::render_queue::Channels::Rgba;
    let mut cropped = OutputModule::for_format(OutputFormat::H264);
    cropped.crop.enabled = true;
    cropped.crop.left = 8;
    let modules = [
        (OutputModule::for_format(OutputFormat::H264), "/mem/all.mp4"),
        (alpha, "/mem/all_[#####].png"),
        (OutputModule::for_format(OutputFormat::ProRes), "/mem/all.mov"),
        (OutputModule::for_format(OutputFormat::WebM), "/mem/all.webm"),
        (OutputModule::for_format(OutputFormat::Gif), "/mem/all.gif"),
        // Cropped: renders other frames, so it exports on its own.
        (cropped, "/mem/all_cropped.mp4"),
    ];
    let pools: Vec<MediaPool> = modules.iter().map(|_| MediaPool::new()).collect();
    let alone: Vec<_> = modules.iter().zip(&pools).map(|((om, path), pool)| run(&p, cid, pool, om, path)).collect();
    let settings = settings();
    let pool = MediaPool::new();
    let files = Arc::new(Mutex::new(BTreeMap::new()));
    let out = files.clone();
    let sink = move |path: &str, data: Vec<u8>| {
        out.lock().unwrap().insert(path.to_string(), data);
    };
    let jobs: Vec<Job> = modules
        .iter()
        .map(|(om, path)| {
            let mut j = job(&p, cid, &pool, &settings, om, path);
            j.sink = Some(&sink);
            j
        })
        .collect();
    let mut last = vec![0u64; jobs.len()];
    let rs = export_modules(&jobs, &mut |m, pr: &Progress| {
        assert!(pr.done >= last[m] && pr.total == 12, "module {m}: {pr:?}");
        last[m] = pr.done;
        true
    });
    assert_eq!(rs.len(), modules.len());
    assert!(rs.iter().all(|r| r.as_ref().is_ok_and(|r| r.frames == 12)), "{rs:?}");
    assert!(last.iter().all(|d| *d == 12), "every module reported all its frames: {last:?}");
    let files = files.lock().unwrap();
    for ((_, path), alone) in modules.iter().zip(&alone) {
        for (f, bytes) in alone {
            assert!(files.get(f) == Some(bytes), "{path}: {f} differs from the module exported alone");
        }
    }
    assert_eq!(files.len(), alone.iter().map(BTreeMap::len).sum::<usize>());
}

fn temp_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("effectcraft-modules-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A module that fails stops the item there, as when modules export one after another: the
/// modules before it finish, the ones after it leave no files.
#[test]
fn a_failing_module_stops_the_modules_after_it() {
    let (p, cid) = project();
    let dir = temp_dir("fail");
    std::fs::write(dir.join("file"), b"x").unwrap();
    let path = |n: &str| dir.join(n).to_string_lossy().to_string();
    let (a, b, c) = (path("a_[#####].png"), path("file/b.mp4"), path("c.mov"));
    let settings = settings();
    let pool = MediaPool::new();
    let oms = [OutputFormat::PngSequence, OutputFormat::H264, OutputFormat::ProRes].map(OutputModule::for_format);
    let jobs = [job(&p, cid, &pool, &settings, &oms[0], &a), job(&p, cid, &pool, &settings, &oms[1], &b), job(&p, cid, &pool, &settings, &oms[2], &c)];
    let rs = export_modules(&jobs, &mut |_, _| true);
    assert_eq!(rs.len(), 2, "{rs:?}");
    assert!(rs[0].is_ok() && matches!(rs[1], Err(ExportError::Io(_))), "{rs:?}");
    assert!(dir.join("a_00000.png").exists() && dir.join("a_00011.png").exists());
    assert!(!dir.join("c.mov").exists(), "the module after the failing one left its file");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A stop cancels the modules being written: their files are removed, and the item stops there.
#[test]
fn a_stop_cancels_the_modules_being_written() {
    let (p, cid) = project();
    let dir = temp_dir("stop");
    let path = |n: &str| dir.join(n).to_string_lossy().to_string();
    let (a, b, c) = (path("a.mp4"), path("b_[#####].png"), path("c.mov"));
    let settings = settings();
    let pool = MediaPool::new();
    let oms = [OutputFormat::H264, OutputFormat::PngSequence, OutputFormat::ProRes].map(OutputModule::for_format);
    let jobs = [job(&p, cid, &pool, &settings, &oms[0], &a), job(&p, cid, &pool, &settings, &oms[1], &b), job(&p, cid, &pool, &settings, &oms[2], &c)];
    // Module 1 is stopped after its first batch; the others would go on.
    let rs = export_modules(&jobs, &mut |m, pr| !(m == 1 && pr.done > 0));
    assert_eq!(rs.len(), 2, "{rs:?}");
    assert!(rs[0].is_ok() && matches!(rs[1], Err(ExportError::Cancelled)), "{rs:?}");
    assert!(dir.join("a.mp4").exists());
    let left: Vec<_> = std::fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()?.file_name().into_string().ok()).filter(|f| f != "a.mp4").collect();
    assert!(left.is_empty(), "cancelled modules left files: {left:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Seconds to export 2 and 3 modules of a 1280×720 comp (an animated blur: every frame renders) one after another (each continuing from
/// the previous module's layer cache) and together (`export_modules`). Run with `--ignored
/// --nocapture`.
#[test]
#[ignore]
fn time_modules_alone_and_together() {
    let (p, cid) = project_sized(1280, 720, true);
    let settings = settings();
    let fmts = [OutputFormat::H264, OutputFormat::ProRes, OutputFormat::PngSequence];
    let oms = fmts.map(OutputModule::for_format);
    let paths = ["/mem/t.mp4", "/mem/t.mov", "/mem/t_[#####].png"];
    let sink = |_: &str, _: Vec<u8>| {};
    for n in [2, 3] {
        for round in 0..2 {
            let pool = MediaPool::new();
            let jobs: Vec<Job> = (0..n)
                .map(|i| {
                    let mut j = job(&p, cid, &pool, &settings, &oms[i], paths[i]);
                    j.sink = Some(&sink);
                    j
                })
                .collect();
            let t = std::time::Instant::now();
            for j in &jobs {
                assert!(export(j, &mut |_| true).is_ok());
            }
            let alone = t.elapsed().as_secs_f64();
            let pool = MediaPool::new();
            let jobs: Vec<Job> = (0..n)
                .map(|i| {
                    let mut j = job(&p, cid, &pool, &settings, &oms[i], paths[i]);
                    j.sink = Some(&sink);
                    j
                })
                .collect();
            let t = std::time::Instant::now();
            assert!(export_modules(&jobs, &mut |_, _| true).iter().all(Result::is_ok));
            let together = t.elapsed().as_secs_f64();
            println!("{n} modules (round {round}): one after another {alone:.3} s, together {together:.3} s");
        }
    }
}

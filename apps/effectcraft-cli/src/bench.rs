//! `bench`: render timings, everyday-operation timings and the Roto Brush trackers' scores.

use serde_json::json;

use effectcraft_engine::Session;
use effectcraft_engine::project::ItemId;
use effectcraft_render::{LayerCache, LayerTiming, RenderOpts, Renderer};
use effectcraft_time::Tick;

use crate::args::BenchArgs;
use crate::commands::emit;
use crate::error::{BenchError, Result};
use crate::io::Io;

/// `bench`: time one frame N times without the layer cache (per-layer and per-effect breakdown),
/// and optionally `--play N` consecutive frames with and without the cache.
pub fn bench_cmd(a: &BenchArgs, json_out: bool, io: &mut Io) -> Result {
    if a.ops {
        return bench_ops(a, json_out, io);
    }
    if a.roto {
        return bench_roto(a, json_out, io);
    }
    let mut s = effectcraft_host::session();
    match &a.project_file {
        Some(p) => s.execute("file.open", json!({"path": p})),
        None => s.execute("file.openDemoProject", json!({})),
    }?;
    if let Some(c) = &a.comp {
        s.execute("comp.open", json!({"comp": c}))?;
    }
    if a.gpu {
        return bench_gpu(&s, a, io);
    }
    let cid = s.active_comp_id().ok_or(BenchError::NoComp)?;
    let t = Tick::from_seconds_f64(a.time.unwrap_or(3.0));
    let opts = RenderOpts { scale: a.scale.unwrap_or(1.0), ..Default::default() };
    bench(&s, cid, t, opts, a.n.unwrap_or(10).max(1), io);
    if let Some(n) = a.play {
        bench_play(&s, cid, t, opts, n.max(1), io);
    }
    Ok(())
}

/// The model a bench option asks for: the file named on the command line, where failing to read or
/// build it is an error saying why (not "not installed"); else the installed file, where a file that
/// cannot be used just leaves the model out.
fn bench_weights<T>(
    flag: &'static str,
    named: Option<&str>,
    installed: Option<std::path::PathBuf>,
    build: impl Fn(&[u8]) -> std::result::Result<T, String>,
) -> Result<Option<T>> {
    match named {
        Some(path) => {
            let bytes = std::fs::read(path).map_err(|e| BenchError::Weights { flag, path: path.to_owned(), message: e.to_string() })?;
            build(&bytes).map(Some).map_err(|message| BenchError::Weights { flag, path: path.to_owned(), message }.into())
        }
        None => Ok(installed.and_then(|p| std::fs::read(p).ok()).and_then(|bytes| build(&bytes).ok())),
    }
}

/// `bench --roto`: the Roto Brush's trackers on synthetic clips whose true mattes are known.
fn bench_roto(a: &BenchArgs, json_out: bool, io: &mut Io) -> Result {
    use effectcraft_segment::{Loaded, MOBILE_SAM, MaskModel};
    use effectcraft_track::roto::bench;
    let frames = a.frames.unwrap_or(24).clamp(1, 240);
    let session = effectcraft_host::session();
    // `--mobilesam FILE` uses the weights at FILE (a file that cannot be used is an error);
    // otherwise the installed ones.
    let mobilesam: Option<std::sync::Arc<dyn MaskModel>> = bench_weights("--mobilesam", a.mobilesam.as_deref(), session.model_path(&MOBILE_SAM), |bytes| {
        match effectcraft_segment::load(MOBILE_SAM.id, bytes)? {
            Loaded::Mask(m) => Ok(m),
            _ => Err("not a mask model".to_string()),
        }
    })?;
    if mobilesam.is_none() {
        note!(io, "bench --roto: MobileSAM is not installed; comparing the classical engine only");
    }
    // `--sam2 FILE` uses the weights at FILE (a file that cannot be used is an error); otherwise
    // the installed ones.
    let installed_sam2 = effectcraft_sam2::mask::SIZES.iter().find_map(|m| session.model_path(m));
    let sam2: Option<std::sync::Arc<dyn MaskModel>> =
        bench_weights("--sam2", a.sam2.as_deref(), installed_sam2, |bytes| effectcraft_sam2::mask::load(bytes).map(|m| m as std::sync::Arc<dyn MaskModel>))?;
    if sam2.is_none() {
        note!(io, "bench --roto: no SAM 2.1 size is installed; no SAM 2 column");
    }
    // `--fixes`: count correction rounds before every frame reaches IoU 0.9 (up to 6 rounds, each up to two strokes).
    let fixes = a.fixes;
    let measure = |sc: &bench::Scenario, run: &dyn Fn(&bench::Scenario) -> Option<bench::Run>| if fixes { bench::with_fixes(sc, 0.9, 6, run) } else { run(sc) };
    let mut runs = vec![];
    // User clips pair unique filename stems, allowing JPEG frames with PNG truth masks.
    let scenarios = if let (Some(dir), Some(truth)) = (&a.clip, &a.truth) {
        let name = std::path::Path::new(dir).file_name().map_or_else(|| "clip".to_string(), |n| n.to_string_lossy().chars().take(10).collect());
        vec![load_bench_clip(&name, dir, truth, frames)?]
    } else {
        bench::scenarios(frames)
    };
    for sc in scenarios {
        runs.extend(measure(&sc, &|s| Some(bench::run_flow(s, "classical", None))));
        if let Some(m) = &mobilesam {
            runs.extend(measure(&sc, &|s| Some(bench::run_flow(s, "mobilesam", Some(m.as_ref())))));
        }
        if let Some(m) = &sam2 {
            let label = m.info().id.replacen("sam2.1-hiera-", "sam2-", 1);
            runs.extend(measure(&sc, &|s| bench::run_tracker(s, &label, m.as_ref())));
        }
    }
    if json_out {
        say!(io, "{}", serde_json::to_string_pretty(&runs)?);
    } else {
        say!(io, "{}", bench::table(&runs));
    }
    Ok(())
}

/// The image files of `dir` (png, jpg, jpeg), ordered by stem then full file name.
fn image_files(dir: &str) -> std::result::Result<Vec<std::path::PathBuf>, BenchError> {
    let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(dir)
        .map_err(|e| BenchError::ReadDir(dir.to_owned(), e))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().and_then(|e| e.to_str()).is_some_and(|e| ["png", "jpg", "jpeg"].contains(&e.to_ascii_lowercase().as_str())))
        .collect();
    files.sort_by(|a, b| a.file_stem().cmp(&b.file_stem()).then_with(|| a.file_name().cmp(&b.file_name())));
    Ok(files)
}

/// A benchmark scenario from the user's frames and truth masks (at most `limit` frames, frames at most 480 px
/// wide; a mask pixel above 127 is the object). Nothing is copied or kept.
fn load_bench_clip(name: &str, frames_dir: &str, truth_dir: &str, limit: usize) -> std::result::Result<effectcraft_track::roto::bench::Scenario, BenchError> {
    let (frames, masks) = (image_files(frames_dir)?, image_files(truth_dir)?);
    if frames.is_empty() || frames.len() != masks.len() {
        return Err(BenchError::CountMismatch { frames: frames.len(), frames_dir: frames_dir.to_owned(), masks: masks.len(), truth_dir: truth_dir.to_owned() });
    }
    if frames.iter().map(|p| p.file_stem()).ne(masks.iter().map(|p| p.file_stem()))
        || frames.windows(2).chain(masks.windows(2)).any(|pair| pair.first().and_then(|p| p.file_stem()) == pair.get(1).and_then(|p| p.file_stem()))
    {
        return Err(BenchError::NameMismatch);
    }
    let n = frames.len().min(limit.clamp(1, 240));
    let (mut imgs, mut truth) = (vec![], vec![]);
    for (f, m) in frames.iter().zip(&masks).take(n) {
        let rgba = image::open(f).map_err(|e| BenchError::Image(f.clone(), e))?.to_rgba8();
        let scale = (480.0 / f64::from(rgba.width().max(1))).min(1.0);
        let (w, h) = (((f64::from(rgba.width()) * scale).round() as u32).max(1), ((f64::from(rgba.height()) * scale).round() as u32).max(1));
        let rgba = if (w, h) == rgba.dimensions() { rgba } else { image::imageops::resize(&rgba, w, h, image::imageops::FilterType::Triangle) };
        let mask = image::open(m).map_err(|e| BenchError::Image(m.clone(), e))?.to_luma8();
        let mask = image::imageops::resize(&mask, w, h, image::imageops::FilterType::Nearest);
        imgs.push(effectcraft_render::Image::from_rgba8(w, h, rgba.as_raw()));
        truth.push(mask.as_raw().iter().map(|v| u8::from(*v > 127)).collect());
    }
    effectcraft_track::roto::bench::scenario_from(name, imgs, truth).map_err(BenchError::Scenario)
}

/// `bench --ops`: everyday operations on a large generated project (200 comps, 5,000 layers in
/// the main comp, 300 footage items, 20-deep precomp nesting, expressions): startup to first
/// frame, open, save, auto-save, small and large edits with undo/redo, first frame, timeline
/// scrolling and the Project panel. `--small` for a quick run; `--layers N`, `--comps N`,
/// `--footage N` resize it. See docs/architecture.md ▸ Performance.
fn bench_ops(a: &BenchArgs, json_out: bool, io: &mut Io) -> Result {
    use effectcraft_engine::perf::{self, LargeSpec};
    let mut spec = if a.small { LargeSpec::small() } else { LargeSpec::default() };
    if let Some(n) = a.layers {
        spec.main_layers = n.max(1);
    }
    if let Some(n) = a.comps {
        spec.comps = n.max(1);
    }
    if let Some(n) = a.footage {
        spec.footage = n;
    }
    let dir = std::env::temp_dir().join(format!("effectcraft-bench-ops-{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| BenchError::Io(dir.clone(), e))?;
    let t0 = std::time::Instant::now();
    drop(effectcraft_host::session());
    let host_ms = t0.elapsed().as_secs_f64() * 1000.0;
    let (mut m, _) = perf::ops_bench(&spec, &dir, &effectcraft_host::session);
    m.insert(0, perf::Measure { name: "hostSession", ms: host_ms, note: "a fully wired session (media, expressions, scripting, export)".into() });
    let path = dir.join("bench-large.ecproj").to_string_lossy().to_string();
    let mut failures = vec![];
    m.extend(effectcraft_ui_egui::bench::ui_ops(|| {
        let mut s = effectcraft_host::session();
        if let Err(e) = s.execute("file.open", json!({"path": path})) {
            failures.push(e.to_string());
        }
        s
    }));
    for e in failures {
        note!(io, "bench --ops: {e}");
    }
    let _ = std::fs::remove_dir_all(&dir);
    if json_out {
        emit(io, &json!({"spec": format!("{spec:?}"), "measures": m.iter().map(perf::Measure::json).collect::<Vec<_>>()}), true);
    } else {
        say!(io, "bench --ops: {} comps, {} layers in Main, {} footage items", spec.comps, spec.main_layers, spec.footage);
        for x in &m {
            say!(io, "  {:<22} {:>10.2} ms  {}", x.name, x.ms, x.note);
        }
    }
    Ok(())
}

// ---------------------------------------------------------------- benchmark helpers

fn median(v: &mut [f64]) -> f64 {
    v.sort_by(f64::total_cmp);
    if v.is_empty() { 0.0 } else { v[v.len() / 2] }
}

fn min(v: &[f64]) -> f64 {
    v.iter().copied().fold(f64::INFINITY, f64::min)
}

/// CPU time of the whole process (all threads) in ms: a load-independent measure of work.
fn cpu_ms() -> f64 {
    cpu_time::ProcessTime::try_now().map(|t| t.as_duration().as_secs_f64() * 1e3).unwrap_or(0.0)
}

/// One frame: (wall ms, CPU ms, per-layer timings).
fn render_timed(s: &Session, cid: ItemId, t: Tick, opts: RenderOpts, cache: Option<&LayerCache>) -> (f64, f64, Vec<LayerTiming>) {
    let prof = std::sync::Mutex::new(Vec::<LayerTiming>::new());
    let mut r = Renderer::new(&s.project, s.footage.as_ref(), opts);
    r.expr = s.expr.as_deref();
    r.cache = cache;
    r.profile = Some(&prof);
    let c0 = cpu_ms();
    let t0 = std::time::Instant::now();
    let img = r.comp_frame(cid, t);
    let ms = t0.elapsed().as_secs_f64() * 1e3;
    let cpu = cpu_ms() - c0;
    std::hint::black_box(&img);
    (ms, cpu, prof.into_inner().unwrap_or_default())
}

type LayerRow = (usize, String, Vec<f64>, Vec<f64>, Vec<(String, Vec<f64>)>);

/// `frame --bench N`: render the same frame N times without the layer cache (the full cost of a
/// frame) and print min/median frame time plus min per-layer and per-effect times.
fn bench(s: &Session, cid: ItemId, t: Tick, opts: RenderOpts, n: usize, io: &mut Io) {
    let mut totals = Vec::with_capacity(n);
    let mut cpus = Vec::with_capacity(n);
    let mut per_layer: Vec<LayerRow> = Vec::new();
    for _ in 0..n {
        let (ms, cpu, prof) = render_timed(s, cid, t, opts, None);
        totals.push(ms);
        cpus.push(cpu);
        for (i, lt) in prof.into_iter().enumerate() {
            if per_layer.len() <= i {
                per_layer.push((lt.depth, lt.layer.clone(), vec![], vec![], vec![]));
            }
            let e = &mut per_layer[i];
            e.2.push(lt.process_ms);
            e.3.push(lt.composite_ms);
            for (j, (id, ms)) in lt.effects.into_iter().enumerate() {
                if e.4.len() <= j {
                    e.4.push((id, vec![]));
                }
                e.4[j].1.push(ms);
            }
        }
    }
    note!(
        io,
        "bench (no cache): {n} runs at scale {}: wall min {:.2} ms, median {:.2} ms; CPU (all threads) min {:.2} ms",
        opts.scale,
        min(&totals),
        median(&mut totals),
        min(&cpus)
    );
    for (d, name, p, c, fx) in per_layer {
        note!(io, "  {:indent$}{name:<28} process {:8.2} ms  composite {:8.2} ms  (min)", "", min(&p), min(&c), indent = d * 2);
        for (id, v) in fx {
            note!(io, "  {:indent$}  fx {id:<30} {:8.2} ms", "", min(&v), indent = d * 2);
        }
    }
}

/// `frame --bench-play N`: render N consecutive frames from `t` as playback does, without and
/// with the layer cache, and print per-frame times.
fn bench_play(s: &Session, cid: ItemId, t: Tick, opts: RenderOpts, n: usize, io: &mut Io) {
    let Some(comp) = s.project.comp(cid) else { return };
    let f0 = comp.frame_rate.frame_at(t);
    let cache = LayerCache::default();
    for (label, c) in [("no cache", None), ("layer cache", Some(&cache))] {
        let runs: Vec<(f64, f64)> = (0..n as i64)
            .map(|i| {
                let (ms, cpu, _) = render_timed(s, cid, comp.frame_rate.tick_of(f0 + i), opts, c);
                (ms, cpu)
            })
            .collect();
        let mut v: Vec<f64> = runs.iter().map(|r| r.0).collect();
        let mean = v.iter().sum::<f64>() / n as f64;
        let cpu = runs.iter().map(|r| r.1).sum::<f64>() / n as f64;
        note!(
            io,
            "bench-play ({label}): {n} frames at scale {}: wall mean {mean:.2} ms, median {:.2} ms, min {:.2} ms; CPU mean {cpu:.2} ms",
            opts.scale,
            median(&mut v),
            min(&v)
        );
    }
    let st = cache.stats();
    note!(io, "  layer cache: {} hits, {} misses, {} entries, {:.1} MB", st.hits, st.misses, st.entries, st.bytes as f64 / 1e6);
}

/// Median wall time (ms) of `n` runs of `f` after one warm-up run.
fn time_ms(n: usize, mut f: impl FnMut()) -> f64 {
    f();
    let mut v: Vec<f64> = (0..n)
        .map(|_| {
            let t0 = std::time::Instant::now();
            f();
            t0.elapsed().as_secs_f64() * 1e3
        })
        .collect();
    median(&mut v)
}

/// `bench --gpu`'s adjustment-layer comp: a copy of the first comp with a full-frame adjustment
/// layer on top running Gaussian Blur, Levels, Vibrance and Glow (all GPU effects).
fn adjustment_bench_comp(p: &mut effectcraft_engine::project::Project) -> Option<ItemId> {
    use effectcraft_engine::effects;
    use effectcraft_engine::keyframe::Value;
    use effectcraft_engine::project::build::{self, Ids};
    use effectcraft_engine::project::{ItemKind, LayerSource, Solid};
    let (_, base) = p.comps().next().map(|(id, c)| (*id, c.clone()))?;
    let mut comp = base.clone();
    let (w, h) = (comp.width, comp.height);
    let sid = p.add_item(
        "Adjustment (bench)",
        effectcraft_engine::color::Label::None,
        None,
        ItemKind::Solid(Solid { color: [1.0; 3], width: w, height: h, pixel_aspect: 1.0 }),
    );
    let mut l = build::layer(p, &comp, "Adjustment (bench)", LayerSource::Solid { item: sid }, (w, h), None);
    l.switches.adjustment = true;
    for (id, vals) in [
        ("ec.blur.gaussian", vec![("blurriness", Value::Scalar(12.0))]),
        ("ec.color.levels", vec![("gamma", Value::Scalar(1.3))]),
        ("ec.color.vibrance", vec![("vibrance", Value::Scalar(40.0))]),
        ("ec.stylize.glow", vec![("threshold", Value::Scalar(55.0))]),
    ] {
        let spec = effects::find(id)?;
        let mut next = p.next_id;
        let mut g = effects::instantiate(spec, &mut Ids(&mut next), spec.name, [w as f64, h as f64]);
        p.next_id = next;
        for (k, v) in vals {
            g.prop_mut(k)?.value = v;
        }
        l.props.sub_mut("effects")?.children.push(g.into());
    }
    comp.layers.insert(0, l);
    Some(p.add_item("Adjustment Layers (bench)", effectcraft_engine::color::Label::None, None, ItemKind::Comp(comp.into())))
}

/// `bench --gpu`'s Advanced 3D comp (1920×1080): a ground plane, PBR primitives, extruded
/// bevelled text, a shadow-casting spot light, a point light and an environment light, seen
/// through a depth-of-field camera (hexagonal iris, highlights) with a motion-blurred sphere.
fn adv3d_bench_comp(p: &mut effectcraft_engine::project::Project) -> Option<ItemId> {
    use effectcraft_engine::keyframe::{Keyframe, TextDoc, Value};
    use effectcraft_engine::project::build::{self, Ids};
    use effectcraft_engine::project::{Comp, ItemKind, Layer, LayerSource, LightKind, PrimitiveKind, Renderer as R3, Solid};
    let (w, h) = (1920u32, 1080u32);
    let mut comp = Comp::new(w, h, effectcraft_time::FrameRate::FPS_30, Tick::from_seconds_f64(10.0));
    comp.renderer = R3::Advanced3D;
    comp.enable_motion_blur = true;
    comp.motion_blur_samples = 8;
    let mut layers: Vec<Layer> = vec![];
    let mut add = |p: &mut effectcraft_engine::project::Project, comp: &Comp, src: LayerSource, edit: &dyn Fn(&mut Layer)| {
        let mut l = build::layer(p, comp, "L", src, (w, h), None);
        edit(&mut l);
        layers.insert(0, l);
    };
    fn set(l: &mut Layer, path: &str, v: Value) {
        if let Some(pr) = l.props.prop_mut(path) {
            pr.value = v;
        }
    }
    add(p, &comp, LayerSource::Primitive { kind: PrimitiveKind::Plane }, &|l| {
        set(l, "geometryOptions/width", Value::Scalar(6000.0));
        set(l, "geometryOptions/height", Value::Scalar(6000.0));
        set(l, "transform/position", Value::Vec3([960.0, 800.0, 0.0]));
        set(l, "transform/rotationX", Value::Scalar(90.0));
    });
    for (i, kind) in [PrimitiveKind::Sphere, PrimitiveKind::Torus, PrimitiveKind::Cube, PrimitiveKind::Sphere].into_iter().enumerate() {
        add(p, &comp, LayerSource::Primitive { kind }, &|l| {
            let x = 400.0 + 380.0 * i as f64;
            set(l, "geometryOptions/radius", Value::Scalar(140.0));
            set(l, "geometryOptions/tubeRadius", Value::Scalar(45.0));
            for k in ["width", "height", "depth"] {
                set(l, &format!("geometryOptions/{k}"), Value::Scalar(220.0));
            }
            set(l, "transform/position", Value::Vec3([x, 640.0, 300.0 * i as f64 - 300.0]));
            set(l, "transform/rotationX", Value::Scalar(35.0));
            set(l, "transform/rotationY", Value::Scalar(25.0));
            set(l, "materialOptions/metallic", Value::Scalar(if i % 2 == 1 { 100.0 } else { 0.0 }));
            set(l, "materialOptions/roughness", Value::Scalar(30.0));
            set(l, "materialOptions/castsShadows", Value::Enum(1));
            if i == 0 {
                l.switches.motion_blur = true;
                if let Some(pr) = l.props.prop_mut("transform/position") {
                    pr.keys = vec![
                        Keyframe::new(Tick::ZERO, Value::Vec3([x, 640.0, -300.0])),
                        Keyframe::new(Tick::from_seconds_f64(10.0), Value::Vec3([x + 300.0 * 60.0, 640.0, -300.0])),
                    ];
                }
            }
        });
    }
    add(p, &comp, LayerSource::Text, &|l| {
        l.switches.three_d = true;
        let doc = TextDoc { text: "EffectCraft".into(), size: 180.0, fill: [0.95, 0.75, 0.2, 1.0], apply_fill: true, ..Default::default() };
        set(l, "text/sourceText", Value::Text(Box::new(doc)));
        let mut next = 900_000u64;
        l.props.children.push(build::extrusion_geometry_options(&mut Ids(&mut next)).into());
        set(l, "geometryOptions/extrusionDepth", Value::Scalar(40.0));
        set(l, "geometryOptions/bevelStyle", Value::Enum(1));
        set(l, "transform/position", Value::Vec3([960.0, 260.0, 200.0]));
        set(l, "transform/rotationY", Value::Scalar(-15.0));
    });
    let env = p.add_item(
        "Environment (bench)",
        effectcraft_engine::color::Label::None,
        None,
        ItemKind::Solid(Solid { color: [0.55, 0.65, 0.9], width: 64, height: 32, pixel_aspect: 1.0 }),
    );
    add(p, &comp, LayerSource::Solid { item: env }, &|l| {
        l.environment = true;
        l.switches.three_d = true;
    });
    add(p, &comp, LayerSource::Light { kind: LightKind::Environment }, &|l| set(l, "lightOptions/intensity", Value::Scalar(40.0)));
    add(p, &comp, LayerSource::Light { kind: LightKind::Spot }, &|l| {
        set(l, "transform/position", Value::Vec3([500.0, -600.0, -900.0]));
        set(l, "transform/poi", Value::Vec3([960.0, 640.0, 0.0]));
        set(l, "lightOptions/castsShadows", Value::Bool(true));
        set(l, "lightOptions/coneAngle", Value::Scalar(100.0));
        set(l, "lightOptions/shadowDiffusion", Value::Scalar(8.0));
    });
    add(p, &comp, LayerSource::Light { kind: LightKind::Point }, &|l| {
        set(l, "transform/position", Value::Vec3([1500.0, 200.0, -500.0]));
        set(l, "lightOptions/intensity", Value::Scalar(50.0));
    });
    add(p, &comp, LayerSource::Camera, &|l| {
        let zoom = match l.props.prop_mut("cameraOptions/zoom").map(|pr| pr.value.clone()) {
            Some(Value::Scalar(z)) => z,
            _ => 2666.7,
        };
        set(l, "cameraOptions/dof", Value::Bool(true));
        set(l, "cameraOptions/focusDistance", Value::Scalar(zoom));
        set(l, "cameraOptions/aperture", Value::Scalar(120.0));
        set(l, "cameraOptions/irisShape", Value::Enum(4));
        set(l, "cameraOptions/highlightGain", Value::Scalar(20.0));
        set(l, "cameraOptions/highlightThreshold", Value::Scalar(200.0));
    });
    comp.layers = layers.into();
    Some(p.add_item("Advanced 3D (bench)", effectcraft_engine::color::Label::None, None, ItemKind::Comp(comp.into())))
}

/// `bench --gpu`: CPU vs GPU ms/frame for every comp (or `--comp`) at Full and Half. "cold"
/// renders everything (no layer cache); "warm" reuses the layer cache (playback / scrubbing of
/// unchanged layers: compositing cost); "viewer" is the GPU display path without readback;
/// "auto" is Backend::Auto (Mercury GPU Acceleration) choosing per comp from measured times,
/// warm; "up/dn MB" are the GPU's uploads / readbacks per warm GPU frame.
fn bench_gpu(s: &Session, a: &BenchArgs, io: &mut Io) -> Result {
    use effectcraft_render::Backend;
    let gpu = effectcraft_gpu::Gpu::try_headless().map_err(BenchError::NoGpu)?;
    let n = a.n.unwrap_or(10).max(1);
    // Every comp, plus an adjustment-layer comp built for the benchmark (the main comp under a
    // full-frame adjustment layer with a GPU effect stack) and an Advanced 3D comp (`--adv3d`:
    // that one only).
    let mut project = (*s.project).clone();
    project.settings.gpu_acceleration = true;
    let comps: Vec<ItemId> = match &a.comp {
        _ if a.adv3d => adv3d_bench_comp(&mut project).into_iter().collect(),
        Some(_) => s.active_comp_id().into_iter().collect(),
        None => {
            let mut v: Vec<ItemId> = s.project.comps().map(|(id, _)| *id).collect();
            v.extend(adjustment_bench_comp(&mut project));
            v.extend(adv3d_bench_comp(&mut project));
            v
        }
    };
    let project = &project;
    note!(io, "GPU: {} — median of {n} runs (ms/frame)", effectcraft_render::Accelerator::name(&gpu));
    note!(
        io,
        "{:<28} {:>5} {:>10} {:>9} {:>9} {:>9} {:>9} {:>9} {:>8} {:>9} {:>9} {:>11}",
        "comp",
        "res",
        "size",
        "cpu cold",
        "gpu cold",
        "cpu warm",
        "gpu warm",
        "gpu view",
        "speedup",
        "auto",
        "gpu≠cpu",
        "up/dn MB"
    );
    for cid in comps {
        let Some(comp) = project.comp(cid) else { continue };
        let name = project.item(cid).map(|i| i.name.clone()).unwrap_or_default();
        let t = Tick::from_seconds_f64(a.time.unwrap_or(3.0).min(comp.duration.seconds() * 0.5));
        for (label, scale) in [("Full", 1.0), ("Half", 0.5)] {
            let mk = |backend: Backend, cache: Option<&LayerCache>| {
                let mut r = Renderer::new(project, s.footage.as_ref(), RenderOpts { scale, backend, ..Default::default() });
                r.expr = s.expr.as_deref();
                r.cache = cache;
                r.accel = Some(&gpu);
                r.comp_frame(cid, t)
            };
            let cpu_cold = time_ms(n, || {
                std::hint::black_box(mk(Backend::Cpu, None));
            });
            let gpu_cold = time_ms(n, || {
                std::hint::black_box(mk(Backend::Gpu, None));
            });
            let cache = LayerCache::default();
            let cpu_warm = time_ms(n, || {
                std::hint::black_box(mk(Backend::Cpu, Some(&cache)));
            });
            let gcache = LayerCache::default();
            // Traffic in steady state: after the frame that fills the layer cache.
            std::hint::black_box(mk(Backend::Gpu, Some(&gcache)));
            let t0 = gpu.context().transfer_stats();
            let gpu_warm = time_ms(n, || {
                std::hint::black_box(mk(Backend::Gpu, Some(&gcache)));
            });
            let t1 = gpu.context().transfer_stats();
            let per = |b: u64| b as f64 / (n + 1) as f64 / (1u64 << 20) as f64;
            let traffic = format!("{:.1}/{:.1}", per(t1.upload_bytes - t0.upload_bytes), per(t1.readback_bytes - t0.readback_bytes));
            // Auto: warm-up frames on both sides, then the measured choice.
            for _ in 0..2 * effectcraft_render::AutoPick::WARMUP {
                std::hint::black_box(mk(Backend::Auto, Some(&gcache)));
            }
            let auto = time_ms(n, || {
                std::hint::black_box(mk(Backend::Auto, Some(&gcache)));
            });
            let view = time_ms(n, || {
                let mut r = Renderer::new(project, s.footage.as_ref(), RenderOpts { scale, backend: Backend::Gpu, ..Default::default() });
                r.expr = s.expr.as_deref();
                r.cache = Some(&gcache);
                r.accel = Some(&gpu);
                std::hint::black_box(gpu.render_display(&r, cid, t));
                gpu.wait();
            });
            let size = format!("{}x{}", (comp.width as f64 * scale).round(), (comp.height as f64 * scale).round());
            // Agreement: share of pixels where the GPU frame differs from the CPU reference by
            // more than 1/255 on any channel.
            let (a, b) = (mk(Backend::Cpu, Some(&cache)), mk(Backend::Gpu, Some(&gcache)));
            let off = a
                .data
                .iter()
                .zip(&b.data)
                .filter(|(p, q)| {
                    (0..4).any(|c| {
                        let d = (p[c] - q[c]).abs();
                        d.is_nan() || d > 1.0 / 255.0 + 1e-6
                    })
                })
                .count();
            let pct = 100.0 * off as f64 / a.data.len().max(1) as f64;
            let speedup = cpu_warm / gpu_warm.max(1e-9);
            note!(
                io,
                "{name:<28} {label:>5} {size:>10} {cpu_cold:>9.2} {gpu_cold:>9.2} {cpu_warm:>9.2} {gpu_warm:>9.2} {view:>9.2} {speedup:>7.2}x {auto:>9.2} {pct:>8.3}% {traffic:>11}"
            );
        }
    }
    Ok(())
}

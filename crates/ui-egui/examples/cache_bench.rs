//! Background caching benchmark: how fast the RAM preview fills a comp, and where the time goes.
//!
//! `cargo run --release -p effectcraft-ui-egui --example cache_bench -- [options]`
//!
//! Without `--project` it builds a comp like a typical edit: eight 1080p30 H.264 clips one after
//! another (encoded once into `target/cache-bench/media` with the Render Queue, no committed
//! media) under an adjustment layer with Gaussian Blur. It then fills the comp's RAM preview the
//! way idle caching does, through [`Frames`], and prints frames per second with the decoder,
//! footage-cache and disk-cache counters.
//!
//! Options: `--project P --comp C` (a saved project's comp instead), `--scale S` (render scale,
//! default 0.5: a viewer at half size), `--frames N` (frames to cache, default the whole comp),
//! `--slots N` (frames in flight, at least 1, default one per lane as the app does when idle),
//! `--lane-width W` (threads per lane; the app's default when absent; `--play` takes it too),
//! `--disk` (with a fresh disk cache in `target/cache-bench/disk`), `--no-adjustment`,
//! `--gpu` (composite on a GPU of its own, as the desktop app does with Mercury GPU Acceleration:
//! the GPU or the CPU, whichever Auto measures faster per comp, read back to the CPU; the viewer's
//! backend), `--backend cpu|gpu|auto` (with `--gpu`: force one compositor, default `auto`).
//!
//! `--play SECS` instead plays the comp in the app (headless, a viewer at 100 %, Resolution
//! Auto, nothing cached, Cache Frames When Idle off) for SECS seconds of wall time, as a first
//! playback, and prints the frames shown per second, the time stalled and the scale it played at.
//! With `--gpu` the app composites on the GPU (Mercury Auto), otherwise on the CPU; `--res N`
//! sets the comp's Resolution to every N-th pixel instead of Auto.

use std::sync::Arc;
use std::time::{Duration, Instant};

use effectcraft_engine::Session;
use effectcraft_engine::project::ItemId;
use effectcraft_engine::render::RenderOpts;
use effectcraft_ui_egui::frames::{FrameKey, Frames, RenderSource, opts_hash};
use serde_json::{Value, json};

fn arg(name: &str) -> Option<String> {
    let a: Vec<String> = std::env::args().collect();
    a.iter().position(|x| x == name).and_then(|i| a.get(i + 1).cloned())
}

/// `--lane-width W`: threads per frame lane (the app's default when absent).
fn lane_width() -> Option<usize> {
    arg("--lane-width").and_then(|v| v.parse().ok())
}

fn run(s: &mut Session, cmd: &str, p: Value) -> Value {
    s.execute(cmd, p).unwrap_or_else(|e| panic!("{cmd}: {e}"))
}

/// Eight copies of one generated clip (separate files: separate decoders, as with real clips).
fn clips(dir: &std::path::Path) -> Vec<String> {
    let media = dir.join("media");
    std::fs::create_dir_all(&media).expect("media folder");
    let first = media.join("clip0.mp4");
    if !first.exists() {
        eprintln!("encoding the source clip (once)…");
        let mut s = effectcraft_host::session();
        run(&mut s, "comp.new", json!({"name": "Src", "width": 1920, "height": 1080, "frameRate": 30, "duration": 4.25}));
        let l = run(&mut s, "layer.newSolid", json!({"color": "#808080"}))["layer"].clone();
        run(&mut s, "effect.apply", json!({"layers": [l], "effect": "Fractal Noise"}));
        run(&mut s, "prop.addKey", json!({"layer": l, "path": "effects/#1/evolution", "time": 0.0, "value": 0.0}));
        run(&mut s, "prop.addKey", json!({"layer": l, "path": "effects/#1/evolution", "time": 4.25, "value": 720.0}));
        run(&mut s, "renderQueue.add", json!({"comp": "Src", "format": "h264", "output": first.to_string_lossy()}));
        run(&mut s, "renderQueue.render", json!({"wait": true}));
    }
    (0..8)
        .map(|i| {
            let p = media.join(format!("clip{i}.mp4"));
            if !p.exists() {
                std::fs::copy(&first, &p).expect("copy clip");
            }
            p.to_string_lossy().to_string()
        })
        .collect()
}

fn main() {
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/cache-bench");
    let mut s = effectcraft_host::session();
    let pool = Arc::new(effectcraft_media::MediaPool::new());
    s.footage = pool.clone();
    if std::env::args().any(|a| a == "--disk") {
        let disk = dir.join("disk");
        if let Err(e) = std::fs::remove_dir_all(&disk)
            && e.kind() != std::io::ErrorKind::NotFound
        {
            eprintln!("can't clear the disk cache at {}: {e}", disk.display());
            std::process::exit(2);
        }
        s.prefs.disk.disk_cache_enabled = true;
        s.prefs.disk.disk_cache_folder = disk.to_string_lossy().to_string();
        s.configure_disk_cache();
    }
    let cid = match arg("--project") {
        Some(p) => {
            run(&mut s, "file.open", json!({"path": p}));
            let comp = match arg("--comp") {
                Some(c) => s.project.comps().find(|(id, _)| s.project.item(**id).is_some_and(|it| it.name == c) || id.0.to_string() == c).map(|(id, _)| *id),
                None => s.active_comp_id(),
            };
            comp.unwrap_or_else(|| {
                eprintln!("no such composition: {}", arg("--comp").unwrap_or_else(|| "(no active comp; pass --comp)".into()));
                std::process::exit(2)
            })
        }
        None => {
            let paths = clips(&dir);
            let cid = ItemId(
                run(&mut s, "comp.new", json!({"name": "Edit", "width": 1920, "height": 1080, "frameRate": 30, "duration": 34}))["comp"]
                    .as_u64()
                    .expect("comp"),
            );
            let items = run(&mut s, "file.import", json!({"paths": paths}))["items"].clone();
            for (i, item) in items.as_array().expect("items").iter().enumerate() {
                let l = run(&mut s, "layer.addItem", json!({"item": item}))["layer"].as_u64().expect("layer");
                let c = Arc::make_mut(&mut s.project).comp_mut(cid).expect("comp");
                let fr = c.frame_rate;
                let l = c.layer_mut(effectcraft_engine::project::LayerId(l)).expect("layer");
                let at = fr.tick_of(i as i64 * 1275 / 10);
                (l.start_time, l.in_point, l.out_point) = (at, at, fr.tick_of((i as i64 + 1) * 1275 / 10));
            }
            if !std::env::args().any(|a| a == "--no-adjustment") {
                let adj = run(&mut s, "layer.newAdjustment", json!({}))["layer"].clone();
                run(&mut s, "effect.apply", json!({"layers": [adj], "effect": "Gaussian Blur"}));
                run(&mut s, "prop.set", json!({"layer": adj, "path": "effects/#1/blurriness", "value": 4.0}));
            }
            cid
        }
    };
    if let Some(secs) = arg("--play").and_then(|v| v.parse().ok()) {
        play(s, cid, secs, std::env::args().any(|a| a == "--gpu"));
        return;
    }
    let comp = s.project.comp(cid).expect("comp").clone();
    let total = comp.frame_rate.frame_at(comp.duration);
    let n = arg("--frames").and_then(|v| v.parse().ok()).unwrap_or(total).min(total);
    let scale: f64 = arg("--scale").and_then(|v| v.parse().ok()).unwrap_or(0.5);

    let frames = lane_width().map_or_else(Frames::default, Frames::with_lane_width);
    // (as Cache Frames When Idle: one frame per lane, see `app/prefetch.rs`)
    let slots: usize = arg("--slots").and_then(|v| v.parse().ok()).map(|n: usize| n.max(1)).unwrap_or(frames.lanes());
    // Room for every frame: this measures speed, not eviction.
    frames.set_budget(64 << 30);
    use effectcraft_engine::render::Backend;
    let backend = match arg("--backend").as_deref() {
        Some("cpu") => Backend::Cpu,
        Some("gpu") => Backend::Gpu,
        _ => Backend::Auto,
    };
    let opts = RenderOpts { scale, motion_blur: true, backend, ..Default::default() };
    let gpu = std::env::args().any(|a| a == "--gpu").then(|| {
        effectcraft_gpu::Gpu::headless().unwrap_or_else(|| {
            eprintln!("--gpu: no usable GPU adapter");
            std::process::exit(2)
        })
    });
    let src = RenderSource {
        project: s.project.clone(),
        footage: s.footage.clone(),
        expr: s.expr.clone(),
        layer_cache: s.layer_cache.clone(),
        gpu,
        gpu_display: false,
        disk: s.disk_cache.clone(),
    };
    let series = FrameKey {
        revision: 1,
        content: frames.content_of(&src.project, 1, cid),
        comp: cid.0,
        frame: 0,
        scale: (scale * 1000.0).round() as u32,
        view: 0,
        opts: opts_hash(&opts),
    };

    eprintln!(
        "caching {n} frames of {}×{} at scale {scale} with {slots} in flight on {} lanes{}…",
        comp.width,
        comp.height,
        frames.lanes(),
        if src.gpu.is_some() { " on the GPU" } else { "" }
    );
    let (gop0, t0) = (effectcraft_media::gop_stats(), Instant::now());
    let mut next = 0;
    loop {
        while next < n && frames.inflight() < slots {
            frames.request_background(&src, FrameKey { frame: next, ..series }, cid, comp.frame_rate.tick_of(next), opts);
            next += 1;
        }
        if next >= n && frames.inflight() == 0 {
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    let secs = t0.elapsed().as_secs_f64();
    let gop = effectcraft_media::gop_stats();
    let cached = frames.cached_frames(&series).len();
    println!("frames cached     {cached}/{n} in {secs:.2} s: {:.1} frames/s, {:.1} ms each", cached as f64 / secs, secs * 1000.0 / cached.max(1) as f64);
    println!(
        "video decoding    {:.2} s in decoders, {} samples decoded, {} seeks, {} cache hits",
        (gop.decode_ns - gop0.decode_ns) as f64 / 1e9,
        gop.decoded - gop0.decoded,
        gop.seeks - gop0.seeks,
        gop.hits - gop0.hits
    );
    if let Some(g) = &src.gpu
        && let Some(p) = effectcraft_engine::render::Accelerator::auto_pick(g)
        && let Some(a) = p.stats(effectcraft_engine::render::AutoKey::new(cid, scale, false))
    {
        let ms = |v: Option<f64>| v.map_or("–".into(), |v| format!("{v:.1} ms"));
        println!("auto (Mercury)    GPU {} over {} frames, CPU {} over {} frames", ms(a.gpu_ms), a.gpu_frames, ms(a.cpu_ms), a.cpu_frames);
    }
    let p = pool.stats();
    println!("footage cache     {} hits, {} misses, {} frames ({} MB) kept", p.hits, p.misses, p.frames, p.bytes >> 20);
    if let Some(dc) = &s.disk_cache {
        // (writes finish on the disk cache's own thread)
        let t = Instant::now();
        dc.flush();
        let flushed = t.elapsed().as_secs_f64();
        let d = dc.stats();
        println!("disk cache        {} written, {} dropped, {} entries ({} MB), flushed in {flushed:.2} s", d.writes, d.dropped, d.entries, d.bytes >> 20);
    }
}

/// `--play`: a first playback of `cid` in the app, as someone pressing play in it would see.
fn play(session: effectcraft_engine::Session, cid: ItemId, secs: f64, gpu: bool) {
    use effectcraft_ui_egui::{ControlRequest, EffectcraftApp};
    let (tx, rx) = std::sync::mpsc::channel::<ControlRequest>();
    let builder = egui_kittest::Harness::builder().with_size(egui::vec2(1920.0, 1200.0)).with_pixels_per_point(1.0);
    let builder = if gpu { builder.wgpu() } else { builder };
    let mut h = builder.build_eframe(move |_| EffectcraftApp::new(session).with_control(rx));
    {
        let app = h.state_mut();
        if let Some(w) = lane_width() {
            app.frames = effectcraft_ui_egui::frames::Frames::with_lane_width(w);
        }
        let _ = app.session.execute("comp.open", json!({"comp": cid.0}));
        app.ui.viewer.zoom = Some(1.0);
        app.ui.cache_when_idle = false;
        if let (Some(n), Some(c)) = (arg("--res").and_then(|v| v.parse().ok()), Arc::make_mut(&mut app.session.project).comp_mut(cid)) {
            c.resolution = effectcraft_engine::project::Resolution::from_factor(n);
        }
    }
    // Let the UI settle (fonts, the GPU compositor's pipelines) before the clock starts.
    let ready = Instant::now() + Duration::from_secs(if gpu { 8 } else { 2 });
    while Instant::now() < ready {
        h.step();
        std::thread::sleep(Duration::from_millis(10));
    }
    h.state().frames.clear();
    let (reply, _r) = std::sync::mpsc::channel();
    let _ = tx.send(ControlRequest { method: "ui.playback".into(), params: json!({"action": "play"}), reply });
    let t0 = Instant::now();
    let (mut started, mut stalled, mut last, mut scales) = (None, 0.0, t0, std::collections::BTreeMap::<u32, u64>::new());
    // When each new frame went on screen (seconds since play was pressed): the cadence. The
    // current time, not `Playback::shown`: frames shown while the clock holds don't count there.
    let fr = h.state().session.project.comp(cid).map(|c| c.frame_rate);
    let (mut shown_at, mut last_frame) = (Vec::<f64>::new(), None);
    while t0.elapsed().as_secs_f64() < secs {
        h.step();
        let now = Instant::now();
        let app = h.state();
        if app.playback.playing {
            let t = *started.get_or_insert(now);
            if app.playback.waiting && now > t {
                stalled += (now - last).as_secs_f64();
            }
        }
        let frame = fr.map(|fr| fr.frame_at(app.session.time()));
        if app.playback.playing && frame != last_frame {
            last_frame = frame;
            shown_at.push((now - t0).as_secs_f64());
        }
        *scales.entry(app.shown_series(cid).scale).or_default() += 1;
        last = now;
        std::thread::sleep(Duration::from_millis(2));
    }
    let app = h.state();
    let played = started.map_or(0.0, |t| t.elapsed().as_secs_f64());
    let shown = shown_at.len();
    let frame = fr.map_or(0, |fr| fr.frame_at(app.session.time()));
    println!(
        "played            {shown} frames shown in {played:.2} s: {:.1} frames/s, stalled {stalled:.2} s, reached frame {frame}{}",
        shown as f64 / played.max(1e-9),
        if gpu { " (GPU)" } else { "" }
    );
    // Cadence: how evenly the frames went on screen (the intervals between consecutive shown
    // frames after the first), and how long the first two took.
    let mut gaps: Vec<f64> = shown_at.windows(2).map(|w| w[1] - w[0]).collect();
    gaps.sort_by(f64::total_cmp);
    let at = |q: f64| gaps.get(((gaps.len() as f64 - 1.0) * q).round() as usize).copied().unwrap_or(0.0) * 1000.0;
    let holds = gaps.iter().filter(|g| **g > 0.2).count();
    println!(
        "cadence           first frame {:.2} s, second {:.2} s; gaps median {:.0} ms, p95 {:.0} ms, max {:.0} ms, {holds} holds > 200 ms",
        shown_at.first().copied().unwrap_or(0.0),
        shown_at.get(1).copied().unwrap_or(0.0),
        at(0.5),
        at(0.95),
        at(1.0)
    );
    // The opening (the first 3 s, before an adaptive step down can have taken effect): how
    // evenly the uncached frames at the starting resolution went on screen.
    let mut early: Vec<f64> = shown_at.windows(2).filter(|w| w[1] <= 3.0).map(|w| w[1] - w[0]).collect();
    early.sort_by(f64::total_cmp);
    let at = |q: f64| early.get(((early.len() as f64 - 1.0) * q).round() as usize).copied().unwrap_or(0.0) * 1000.0;
    println!(
        "opening (3 s)     {} frames; gaps median {:.0} ms, p95 {:.0} ms, max {:.0} ms, {} holds > 200 ms",
        early.len() + usize::from(!early.is_empty()),
        at(0.5),
        at(0.95),
        at(1.0),
        early.iter().filter(|g| **g > 0.2).count()
    );
    let passes: u64 = scales.values().sum();
    let s: Vec<String> = scales.iter().map(|(k, v)| format!("{:.3}: {:.0} %", *k as f64 / 1000.0, 100.0 * *v as f64 / passes.max(1) as f64)).collect();
    println!("viewer scale      {}", s.join(", "));
}

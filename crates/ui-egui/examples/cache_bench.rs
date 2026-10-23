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
//! `--slots N` (frames in flight, default half the frame threads as the app does when idle),
//! `--disk` (with a fresh disk cache in `target/cache-bench/disk`), `--no-adjustment`.

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
        let _ = std::fs::remove_dir_all(&disk);
        s.prefs.disk.disk_cache_enabled = true;
        s.prefs.disk.disk_cache_folder = disk.to_string_lossy().to_string();
        s.configure_disk_cache();
    }
    let cid = match arg("--project") {
        Some(p) => {
            run(&mut s, "file.open", json!({"path": p}));
            let c = arg("--comp").unwrap_or_default();
            let found = s.project.items.iter().find(|(id, it)| it.name == c || id.0.to_string() == c).map(|(id, _)| *id);
            found.or_else(|| s.active_comp_id()).expect("a comp")
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
                (l.start_time, l.in_point, l.out_point) = (at, at, at + fr.tick_of(1275 / 10));
            }
            if !std::env::args().any(|a| a == "--no-adjustment") {
                let adj = run(&mut s, "layer.newAdjustment", json!({}))["layer"].clone();
                run(&mut s, "effect.apply", json!({"layers": [adj], "effect": "Gaussian Blur"}));
                run(&mut s, "prop.set", json!({"layer": adj, "path": "effects/#1/blurriness", "value": 4.0}));
            }
            cid
        }
    };
    let comp = s.project.comp(cid).expect("comp").clone();
    let total = comp.frame_rate.frame_at(comp.duration);
    let n = arg("--frames").and_then(|v| v.parse().ok()).unwrap_or(total).min(total);
    let scale: f64 = arg("--scale").and_then(|v| v.parse().ok()).unwrap_or(0.5);

    let frames = Frames::default();
    let slots: usize = arg("--slots").and_then(|v| v.parse().ok()).unwrap_or((frames.parallelism() / 2).max(1));
    // Room for every frame: this measures speed, not eviction.
    frames.set_budget(64 << 30);
    let opts = RenderOpts { scale, motion_blur: true, ..Default::default() };
    let src = RenderSource {
        project: s.project.clone(),
        footage: s.footage.clone(),
        expr: s.expr.clone(),
        layer_cache: s.layer_cache.clone(),
        gpu: None,
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

    eprintln!("caching {n} frames of {}×{} at scale {scale} with {slots} in flight…", comp.width, comp.height);
    let (gop0, t0) = (filmcraft_codecs::gop_stats(), Instant::now());
    let mut next = 0;
    loop {
        while next < n && frames.inflight() < slots {
            frames.request(&src, FrameKey { frame: next, ..series }, cid, comp.frame_rate.tick_of(next), opts);
            next += 1;
        }
        if next >= n && frames.inflight() == 0 {
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    let secs = t0.elapsed().as_secs_f64();
    let gop = filmcraft_codecs::gop_stats();
    let cached = frames.cached_frames(&series).len();
    println!("frames cached     {cached}/{n} in {secs:.2} s: {:.1} frames/s, {:.1} ms each", cached as f64 / secs, secs * 1000.0 / cached.max(1) as f64);
    println!(
        "video decoding    {:.2} s in decoders, {} samples decoded, {} seeks, {} cache hits",
        (gop.decode_ns - gop0.decode_ns) as f64 / 1e9,
        gop.decoded - gop0.decoded,
        gop.seeks - gop0.seeks,
        gop.hits - gop0.hits
    );
    let p = pool.stats();
    println!("footage cache     {} hits, {} misses, {} frames ({} MB) kept", p.hits, p.misses, p.frames, p.bytes >> 20);
    if let Some(dc) = &s.disk_cache {
        // (writes finish on the disk cache's own thread)
        std::thread::sleep(Duration::from_secs(2));
        let d = dc.stats();
        println!("disk cache        {} written, {} dropped, {} entries ({} MB)", d.writes, d.dropped, d.entries, d.bytes >> 20);
    }
}

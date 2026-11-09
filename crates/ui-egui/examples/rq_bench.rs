//! Render Queue benchmark (#3335): a 1080p 32 bpc job on the GPU, the CPU or the measured choice.
//!
//! `cargo run --release -p effectcraft-ui-egui --example rq_bench -- [options]`
//!
//! It builds a comp like a compositing shot: a 1920 × 1080 OpenEXR sequence (float RGBA with a
//! Cryptomatte ID channel and manifest, generated once into `target/rq-bench/media`, no
//! committed media) under Gaussian Blur, Curves and the Cryptomatte effect and a moving
//! translucent solid, at 32 bpc. It queues the comp twice (the second item shows a warm start) and
//! renders the queue as the app does, printing each item's milliseconds per frame and a hash of
//! the written pixels (equal hashes: identical output).
//!
//! Options: `--renderer gpu|cpu` (the project's renderer, default gpu; with gpu the Render
//! Queue measures both unless `EFFECTCRAFT_RQ_RENDERER` says otherwise), `--frames N` (default
//! 60), `--format png|exr|tiff` (default png), `--out DIR` (default `target/rq-bench/out`).

use std::sync::Arc;

use effectcraft_engine::Session;
use effectcraft_engine::raster::channels3d::{crypto_float, crypto_hash};
use serde_json::{Value, json};

const W: usize = 1920;
const H: usize = 1080;

fn arg(name: &str) -> Option<String> {
    let a: Vec<String> = std::env::args().collect();
    a.iter().position(|x| x == name).and_then(|i| a.get(i + 1).cloned())
}

fn run(s: &mut Session, cmd: &str, p: Value) -> Value {
    s.execute(cmd, p).unwrap_or_else(|e| panic!("{cmd}: {e}"))
}

/// The plate's object at pixel (x, y) in frame `f`: 24 bands that drift sideways.
fn object(x: usize, y: usize, f: usize) -> String {
    format!("obj{}", ((x + f * 7) / 80 + y / 270 * 24) % 96)
}

/// Frame `f` of the plate: float colour over 1.0 (32 bpc), alpha, and Cryptomatte IDs.
fn plate(path: &std::path::Path, f: usize) {
    use exr::prelude::*;
    let px = |c: usize| -> Vec<f32> {
        (0..W * H)
            .map(|i| {
                let (x, y) = (i % W, i / W);
                let v = ((x as f32 * 0.003 + f as f32 * 0.05 + c as f32).sin() * 0.5 + 0.5) * (1.0 + y as f32 / H as f32);
                if c == 3 { 1.0 } else { v }
            })
            .collect()
    };
    let ids: Vec<f32> = (0..W * H).map(|i| crypto_float(crypto_hash(&object(i % W, i / W, f)))).collect();
    let ch = |n: &str, v: Vec<f32>| AnyChannel::new(n, FlatSamples::F32(v));
    let channels = AnyChannels::sort(
        vec![ch("R", px(0)), ch("G", px(1)), ch("B", px(2)), ch("A", px(3)), ch("CryptoObject00.R", ids), ch("CryptoObject00.G", vec![1.0; W * H])].into(),
    );
    let mut layer = Layer::new((W, H), LayerAttributes::default(), Encoding::FAST_LOSSLESS, channels);
    let names: serde_json::Map<String, Value> = (0..96).map(|k| format!("obj{k}")).map(|n| (n.clone(), json!(format!("{:08x}", crypto_hash(&n))))).collect();
    for (key, v) in [("name", "CryptoObject".to_string()), ("manifest", Value::Object(names).to_string())] {
        layer.attributes.other.insert(Text::from(format!("cryptomatte/f834d0a/{key}").as_str()), AttributeValue::Text(Text::from(v.as_str())));
    }
    Image::from_layer(layer).write().to_file(path).expect("write plate");
}

/// FNV-1a over every written frame file, in name order (equal: identical output).
fn hash_dir(dir: &std::path::Path) -> (usize, u64) {
    let mut names: Vec<_> = std::fs::read_dir(dir).map(|d| d.filter_map(|e| e.ok()).map(|e| e.path()).collect()).unwrap_or_default();
    names.retain(|p| !p.to_string_lossy().ends_with(".txt"));
    names.sort();
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for p in &names {
        for b in std::fs::read(p).unwrap_or_default() {
            h = (h ^ b as u64).wrapping_mul(0x100_0000_01b3);
        }
    }
    (names.len(), h)
}

fn main() {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/rq-bench");
    let frames: usize = arg("--frames").and_then(|n| n.parse().ok()).unwrap_or(60);
    let format = arg("--format").unwrap_or_else(|| "png".into());
    let gpu = arg("--renderer").as_deref() != Some("cpu");
    let out = arg("--out").map(std::path::PathBuf::from).unwrap_or_else(|| root.join("out"));
    let media = root.join("media");
    std::fs::create_dir_all(&media).expect("media folder");
    let first = media.join("plate_0000.exr");
    for f in 0..frames {
        let p = media.join(format!("plate_{f:04}.exr"));
        if !p.exists() {
            if f == 0 {
                eprintln!("writing the {frames}-frame EXR plate (once)…");
            }
            plate(&p, f);
        }
    }

    let mut s = effectcraft_host::session();
    let accel = effectcraft_gpu::Gpu::try_headless().unwrap_or_else(|e| panic!("no GPU: {e}"));
    eprintln!("GPU: {}", effectcraft_engine::render::Accelerator::name(&accel));
    s.accel = Some(Arc::new(accel));
    let r = run(&mut s, "file.import", json!({"paths": [first.to_string_lossy()], "sequence": true}));
    let footage = r["items"][0].as_u64().expect("the plate");
    let secs = frames as f64 / 30.0;
    run(&mut s, "comp.new", json!({"name": "Shot", "width": W, "height": H, "frameRate": 30, "duration": secs}));
    let plate = run(&mut s, "layer.addItem", json!({"item": footage}))["layer"].clone();
    run(&mut s, "effect.apply", json!({"layers": [plate], "effect": "Gaussian Blur"}));
    run(&mut s, "prop.set", json!({"layer": plate, "path": "effects/#1/blurriness", "value": 12.0}));
    run(&mut s, "effect.apply", json!({"layers": [plate], "effect": "Curves"}));
    run(&mut s, "effect.apply", json!({"layers": [plate], "effect": "Cryptomatte"}));
    let solid = run(&mut s, "layer.newSolid", json!({"color": "#d04020", "width": 400, "height": 300}))["layer"].clone();
    run(&mut s, "prop.addKey", json!({"layer": solid, "path": "transform/position", "time": 0.0, "value": [200.0, 540.0]}));
    run(&mut s, "prop.addKey", json!({"layer": solid, "path": "transform/position", "time": secs, "value": [1720.0, 540.0]}));
    run(&mut s, "prop.set", json!({"layer": solid, "path": "transform/opacity", "value": 70.0}));
    let mut p = (*s.project).clone();
    p.settings.bit_depth = effectcraft_engine::project::BitDepth::Bpc32;
    p.settings.gpu_acceleration = gpu;
    s.project = Arc::new(p);

    let _ = std::fs::remove_dir_all(&out);
    let dirs = [out.join("a"), out.join("b")];
    for d in &dirs {
        std::fs::create_dir_all(d).expect("output folder");
        let path = d.join(format!("f_[####].{format}"));
        run(&mut s, "renderQueue.add", json!({"comp": "Shot", "format": format, "output": path.to_string_lossy(), "log": "plusSettings"}));
    }
    let t0 = std::time::Instant::now();
    run(&mut s, "renderQueue.render", json!({"wait": true}));
    let total = t0.elapsed().as_secs_f64();
    println!(
        "renderer {} (EFFECTCRAFT_RQ_RENDERER={:?}, EFFECTCRAFT_GPU_INFLIGHT={:?}), {frames} frames {W}x{H} 32 bpc, {format}",
        if gpu { "gpu" } else { "cpu" },
        std::env::var("EFFECTCRAFT_RQ_RENDERER").ok(),
        std::env::var("EFFECTCRAFT_GPU_INFLIGHT").ok()
    );
    for (i, (item, d)) in s.project.render_queue.iter().zip(&dirs).enumerate() {
        let secs = item.render_time.unwrap_or(f64::NAN);
        let (n, h) = hash_dir(d);
        println!("  item {}: {:?}, {:.1} ms/frame, {n} files, hash {h:016x}", i + 1, item.status, secs * 1000.0 / frames as f64);
        // The render log's account of the compositor and the GPU frames in flight.
        let logs =
            std::fs::read_dir(d).map(|r| r.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.to_string_lossy().ends_with(".txt")).collect::<Vec<_>>());
        for log in logs.unwrap_or_default() {
            for line in std::fs::read_to_string(log).unwrap_or_default().lines().filter(|l| l.starts_with("Renderer:") || l.starts_with("GPU frames")) {
                println!("    {line}");
            }
        }
    }
    println!("  queue: {total:.2} s");
}

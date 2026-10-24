//! Headless checks for M13.24 (web app depth: GPU frame workers, the browser's disk cache, the
//! storage manager), run natively with stand-ins for the browser:
//!
//! - a frame worker with its own GPU device and deferred readbacks (the browser worker's
//!   WebGPU path) renders frames with GPU effects in passes; the page's own GPU compositor
//!   keeps frames without effects, and frames with effects go to the worker;
//! - frames the worker rendered are stored under their disk-cache keys (an in-memory stand-in
//!   for the Origin Private File System) and served from there after a "reload" (a new frame
//!   cache), without rendering; the blue cache bar sees them;
//! - Settings ▸ Disk shows the browser storage manager when the session has a storage host,
//!   and its buttons run `storage.*` commands.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use effectcraft_engine::Session;
use effectcraft_engine::remote::{FrameMsg, FrameReply, FrameServer, Mirror};
use effectcraft_engine::render::RenderOpts;
use effectcraft_engine::render::disk_cache;
use effectcraft_engine::storage::StorageHost;
use effectcraft_gpu::Gpu;
use effectcraft_ui_egui::EffectcraftApp;
use effectcraft_ui_egui::control::{self, ControlRequest, Outcome};
use effectcraft_ui_egui::frames::{FrameImage, FrameKey, Frames, RemoteDone, RemoteFrame, RemoteFrames, RemoteJob, RenderSource};
use egui_kittest::Harness;
use serde_json::{Value, json};

fn rt<T: serde::Serialize + for<'de> serde::Deserialize<'de>>(v: &T) -> T {
    serde_json::from_str(&serde_json::to_string(v).unwrap()).unwrap()
}

/// A GPU frame worker in this process, with a "disk" shared across reloads.
struct GpuWorker {
    gpu: Gpu,
    state: Mutex<(Mirror, FrameServer, u64)>,
    disk: Arc<Mutex<HashMap<u128, Vec<u8>>>>,
    rendered: Mutex<Vec<(u32, bool)>>,
}

impl GpuWorker {
    fn new(s: &Session, gpu: Gpu, disk: Arc<Mutex<HashMap<u128, Vec<u8>>>>) -> GpuWorker {
        let mut server = FrameServer::new(s.footage.clone(), s.expr.clone());
        server.set_accel(Some(Arc::new(gpu.clone())));
        GpuWorker { gpu, state: Mutex::new((Mirror::default(), server, 0)), disk, rendered: Mutex::default() }
    }
}

impl RemoteFrames for GpuWorker {
    fn slots(&self) -> usize {
        1
    }
    fn gpu(&self) -> bool {
        true
    }
    fn disk(&self) -> bool {
        true
    }
    fn disk_contains(&self, key: u128) -> bool {
        self.disk.lock().unwrap().contains_key(&key)
    }
    fn start(&self, job: RemoteJob, done: RemoteDone) {
        // The page reads a hit from the disk cache instead of asking the worker.
        if let Some(k) = job.disk_key
            && let Some(f) = self.disk.lock().unwrap().get(&k).and_then(|e| disk_cache::read_frame_entry(e))
        {
            return done(Ok(RemoteFrame { width: f.width, height: f.height, rgba: f.rgba, ms: 0.0 }));
        }
        let mut st = self.state.lock().unwrap();
        let (mirror, server, next) = &mut *st;
        if let Some(m) = mirror.sync(job.revision, &job.project) {
            assert!(server.handle(rt(&m)).is_empty());
        }
        *next += 1;
        let disk = job.disk_key.map(|k| format!("{k:032x}"));
        let mut out = server.handle(rt(&FrameMsg::Render {
            id: *next,
            revision: job.revision,
            comp: job.comp,
            time: job.t,
            opts: Box::new(job.opts),
            disk: disk.clone(),
            layers: false,
            prefetch: vec![],
        }));
        // The worker awaits its device between passes (`Gpu::settled`); natively a poll.
        while server.waiting() {
            self.gpu.wait();
            pollster::block_on(self.gpu.settled());
            out = server.resume();
        }
        match out.into_iter().next() {
            Some((FrameReply::Frame { width, height, ms, passes, gpu, .. }, Some(rgba))) => {
                self.rendered.lock().unwrap().push((passes, gpu));
                if let Some(k) = job.disk_key {
                    self.disk.lock().unwrap().insert(k, disk_cache::frame_entry(width, height, &rgba));
                }
                done(Ok(RemoteFrame { width, height, rgba, ms }))
            }
            other => done(Err(format!("{:?}", other.map(|o| o.0)))),
        }
    }
}

fn source(s: &Session, gpu: Option<Gpu>) -> RenderSource {
    RenderSource {
        project: s.project.clone(),
        footage: s.footage.clone(),
        expr: s.expr.clone(),
        layer_cache: s.layer_cache.clone(),
        gpu,
        gpu_display: true,
        disk: None,
    }
}

fn cpu_pixels(f: &FrameImage) -> Vec<egui::Color32> {
    match f {
        FrameImage::Cpu(c) => c.pixels.clone(),
        FrameImage::Gpu(_) => panic!("expected a CPU frame"),
    }
}

#[test]
fn gpu_worker_renders_effect_frames_and_the_disk_cache_survives_a_reload() {
    let (Some(page_gpu), Some(worker_gpu)) = (Gpu::headless(), Gpu::headless_deferred()) else {
        eprintln!("no GPU adapter: skipped");
        return;
    };
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Fx", "width": 160, "height": 90, "frameRate": 30, "duration": 1})).unwrap();
    let plain = s.active_comp_id().unwrap();
    s.execute("layer.newSolid", json!({"color": "#3080ff", "width": 80, "height": 40})).unwrap();
    s.execute("comp.new", json!({"name": "Blurred", "width": 160, "height": 90, "frameRate": 30, "duration": 1})).unwrap();
    let fx = s.active_comp_id().unwrap();
    s.execute("layer.newSolid", json!({"color": "#ff8030", "width": 80, "height": 40})).unwrap();
    s.execute("effect.apply", json!({"layer": "#1", "effect": "Gaussian Blur"})).unwrap();
    s.execute("prop.set", json!({"layer": "#1", "path": "effects/#1/blurriness", "value": 12})).unwrap();
    let opts = RenderOpts { backend: effectcraft_engine::render::Backend::Gpu, ..Default::default() };
    let t = effectcraft_engine::time::Tick::ZERO;
    let key = |comp: effectcraft_engine::project::ItemId| FrameKey {
        revision: s.revision,
        content: effectcraft_ui_egui::frames::comp_content(&s.project, comp),
        comp: comp.0,
        frame: 0,
        scale: 1000,
        view: 0,
        opts: 0,
    };
    let disk = Arc::new(Mutex::new(HashMap::new()));
    let worker = Arc::new(GpuWorker::new(&s, worker_gpu.clone(), disk.clone()));
    let mut frames = Frames::default();
    frames.set_remote(Some(worker.clone()));
    // No effects: the page's GPU compositor keeps the frame on the GPU.
    frames.request_urgent(&source(&s, Some(page_gpu.clone())), key(plain), plain, t, opts);
    frames.dispatch_remote();
    assert!(matches!(frames.get(&key(plain)), Some(FrameImage::Gpu(_))));
    assert!(worker.rendered.lock().unwrap().is_empty());
    // GPU effects: the worker renders the frame on its GPU, in passes.
    frames.request_urgent(&source(&s, Some(page_gpu.clone())), key(fx), fx, t, opts);
    frames.dispatch_remote();
    let got = cpu_pixels(&frames.get(&key(fx)).expect("rendered by the worker"));
    let (passes, on_gpu) = worker.rendered.lock().unwrap()[0];
    assert!(on_gpu && passes >= 2, "passes {passes}");
    let want = effectcraft_ui_egui::frames::to_color_image(&s.render(fx, t, RenderOpts { backend: effectcraft_engine::render::Backend::Cpu, ..opts }));
    let worst = got.iter().zip(&want.pixels).map(|(a, b)| (0..4).map(|c| (a[c] as i32 - b[c] as i32).abs()).max().unwrap()).max().unwrap();
    assert!(worst <= 2, "GPU vs CPU: {worst}/255");
    assert_eq!(disk.lock().unwrap().len(), 1, "stored under its disk key");
    // "Reload": a new frame cache and worker; the frame comes from the disk cache.
    let worker2 = Arc::new(GpuWorker::new(&s, worker_gpu, disk.clone()));
    let mut frames2 = Frames::default();
    frames2.set_remote(Some(worker2.clone()));
    assert!(frames2.remote_disk());
    let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let bar = loop {
        let bar = frames2.disk_frames(&source(&s, None), &key(fx), 0..30, &opts);
        if !bar.is_empty() {
            break bar;
        }
        assert!(std::time::Instant::now() < until, "disk-cache indicator did not finish hashing");
        std::thread::sleep(std::time::Duration::from_millis(1));
    };
    assert_eq!(bar, vec![0], "the blue cache bar shows the stored frame");
    frames2.request_urgent(&source(&s, Some(page_gpu)), key(fx), fx, t, opts);
    frames2.dispatch_remote();
    assert_eq!(cpu_pixels(&frames2.get(&key(fx)).unwrap()), got);
    assert!(worker2.rendered.lock().unwrap().is_empty(), "a hit is not rendered again");
}

#[derive(Default)]
struct Storage {
    cleared: Mutex<Vec<String>>,
    persist_asked: Mutex<bool>,
}

impl StorageHost for Storage {
    fn info(&self) -> Value {
        json!({"backend": "opfs", "usage": 52_000_000u64, "quota": 2_000_000_000u64, "persisted": false,
               "files": {"projects": {"count": 2, "bytes": 40000}, "media": {"count": 3, "bytes": 50_000_000u64}, "autoSaves": {"count": 1, "bytes": 20000}},
               "diskCache": {"enabled": true, "entries": 12, "bytes": 1_500_000, "maxBytes": 1_000_000_000u64, "hits": 4, "misses": 12}})
    }
    fn request_persist(&self) -> Value {
        *self.persist_asked.lock().unwrap() = true;
        json!({"requested": true})
    }
    fn clear(&self, what: &str) -> Result<Value, String> {
        self.cleared.lock().unwrap().push(what.into());
        Ok(json!({"cleared": what, "entries": 1, "bytes": 1}))
    }
}

fn send(h: &mut Harness<'_, EffectcraftApp>, method: &str, params: Value) {
    let ctx = h.ctx.clone();
    let (req, _rx) = ControlRequest::new(method, params);
    if let Outcome::Done(v) = control::handle(h.state_mut(), &ctx, &req) {
        assert!(v["ok"] != false, "{method}: {v}");
    }
    // kittest doesn't call `raw_input_hook`: feed the synthetic click in.
    for e in h.state_mut().take_synthetic_input() {
        h.event(e);
    }
    h.run_steps(3);
}

#[test]
fn settings_disk_page_shows_the_browser_storage_manager() {
    let mut s = Session::default();
    let host = Arc::new(Storage::default());
    s.storage = Some(host.clone());
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_eframe(|_| EffectcraftApp::new(s));
    h.run_steps(2);
    effectcraft_ui_egui::panels::settings::open(h.state_mut(), "disk");
    h.run_steps(3);
    for id in [
        "settings.storage.backend",
        "settings.storage.usage",
        "settings.storage.diskCache",
        "settings.storage.persist",
        "settings.storage.clear.diskCache",
        "settings.storage.clear.media",
    ] {
        assert!(h.state().auto.find(id).is_some(), "{id} is registered");
    }
    let usage = h.state().auto.find("settings.storage.usage").unwrap().label.clone();
    assert!(usage.contains("52.0 MB of 2.0 GB"), "{usage}");
    send(&mut h, "ui.click", json!({"id": "settings.storage.clear.diskCache"}));
    send(&mut h, "ui.click", json!({"id": "settings.storage.persist"}));
    assert_eq!(*host.cleared.lock().unwrap(), vec!["diskCache".to_string()]);
    assert!(*host.persist_asked.lock().unwrap());
    // Without a host (the desktop) the section is not there.
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_eframe(|_| EffectcraftApp::new(Session::default()));
    h.run_steps(2);
    effectcraft_ui_egui::panels::settings::open(h.state_mut(), "disk");
    h.run_steps(3);
    assert!(h.state().auto.find("settings.storage.usage").is_none());
}

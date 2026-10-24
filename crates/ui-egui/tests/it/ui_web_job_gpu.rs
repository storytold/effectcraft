//! Headless checks for M13.30 (GPU in the browser's job workers), run natively: a job worker
//! session whose accelerator is a device with deferred readbacks (the worker's own WebGPU
//! device) runs the Render Queue and an analysis with their frames rendered on the device in
//! passes (`offload::run_request_async`, `render::passes`), and gets what a CPU worker gets.

use std::sync::{Arc, Mutex};

use effectcraft_engine::Session;
use effectcraft_engine::offload::{Inbox, Offload, Post, WorkerReply, WorkerRequest};
use effectcraft_gpu::Gpu;
use serde_json::json;

/// One device at a time in this test binary: wgpu's OpenGL backend (Mesa llvmpipe on FreeBSD,
/// Linux without Vulkan) panics or crashes when tests create and drive devices concurrently.
static GPU_LOCK: Mutex<()> = Mutex::new(());

/// Holds offloaded jobs until the test runs them.
#[derive(Default)]
struct Manual(Mutex<Vec<(WorkerRequest, Arc<Inbox>)>>);

impl Offload for Manual {
    fn start(&self, req: WorkerRequest, inbox: Arc<Inbox>) -> Result<(), String> {
        self.0.lock().unwrap().push((req, inbox));
        Ok(())
    }
    fn cancel(&self, _: u64) {}
}

type Files = Arc<Mutex<Vec<(String, Vec<u8>)>>>;

fn exporter(files: Files) -> Arc<effectcraft_host::FileExporter> {
    let sink: Arc<dyn Fn(&str, Vec<u8>) + Send + Sync> = Arc::new(move |p: &str, d: Vec<u8>| files.lock().unwrap().push((p.to_string(), d)));
    Arc::new(effectcraft_host::FileExporter { sink: Some(sink) })
}

/// Run `req` on a worker session (`gpu`: its accelerator); its replies and the files it wrote.
fn run(req: WorkerRequest, gpu: Option<&Gpu>) -> (Vec<WorkerReply>, Vec<(String, Vec<u8>)>) {
    let files: Files = Arc::default();
    let mut w = Session { exporter: Some(exporter(files.clone())), ..Default::default() };
    w.accel = gpu.map(|g| Arc::new(g.clone()) as Arc<dyn effectcraft_engine::render::Accelerator>);
    let replies = Arc::new(Mutex::new(vec![]));
    let r2 = replies.clone();
    let post: Post = std::rc::Rc::new(move |r| r2.lock().unwrap().push(r));
    effectcraft_engine::offload::run_request(&mut w, req, &post);
    let r = replies.lock().unwrap().clone();
    let f = files.lock().unwrap().clone();
    (r, f)
}

fn page() -> (Session, Arc<Manual>) {
    let off = Arc::new(Manual::default());
    let mut s = Session { offload: Some(off.clone()), exporter: Some(exporter(Arc::default())), ..Default::default() };
    s.execute("comp.new", json!({"name": "Job", "width": 96, "height": 64, "frameRate": 10, "duration": 0.6})).unwrap();
    let l = s.execute("layer.newSolid", json!({"color": "#3070c0"})).unwrap()["layer"].as_u64().unwrap();
    // Moving texture (Fractal Noise, offset animated), then a blur: GPU effects.
    s.execute("effect.apply", json!({"layers": [l], "effect": "ec.noise.fractal"})).unwrap();
    s.execute("prop.addKey", json!({"layer": l, "path": "effects/#1/transform/offset", "time": 0.0, "value": [0.5, 0.5]})).unwrap();
    s.execute("prop.addKey", json!({"layer": l, "path": "effects/#1/transform/offset", "time": 0.5, "value": [0.6, 0.56]})).unwrap();
    s.execute("effect.apply", json!({"layers": [l], "effect": "ec.blur.gaussian"})).unwrap();
    s.execute("prop.set", json!({"layer": l, "path": "effects/#2/blurriness", "value": 3.0})).unwrap();
    (s, off)
}

#[test]
fn render_queue_jobs_render_on_the_worker_gpu() {
    let _gpu = GPU_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(g) = Gpu::headless_deferred() else {
        eprintln!("no GPU adapter: skipped");
        return;
    };
    let (mut s, off) = page();
    s.execute("renderQueue.add", json!({"format": "png", "output": "/job/f_[#####].png"})).unwrap();
    s.execute("renderQueue.render", json!({"wait": false})).unwrap();
    let (req, _) = off.0.lock().unwrap().pop().expect("the render went to the worker");
    let (cpu_replies, mut cpu_files) = run(req.clone(), None);
    let before = g.deferred_stats().unwrap();
    let (gpu_replies, mut gpu_files) = run(req, Some(&g));
    // (the CPU worker writes a batch's files in parallel)
    cpu_files.sort();
    gpu_files.sort();
    let after = g.deferred_stats().unwrap();
    let done = |r: &[WorkerReply]| r.iter().any(|r| matches!(r, WorkerReply::Item { update } if format!("{update:?}").contains("Done")));
    assert!(done(&cpu_replies) && done(&gpu_replies), "{gpu_replies:?}");
    assert!(matches!(gpu_replies.last(), Some(WorkerReply::Done)));
    // Every frame renders on the project's renderer, the GPU: in passes on the device (their
    // chains, then the frame read back).
    let frames = gpu_files.len() as u64;
    assert_eq!(frames, 6);
    assert!(after.0 - before.0 >= 6, "passes {before:?} → {after:?}");
    assert!(after.1 - before.1 >= 6, "readbacks {before:?} → {after:?}");
    assert_eq!(after.2, 0, "nothing left in flight");
    // The same frames as the CPU worker's (within GPU rounding).
    assert_eq!(cpu_files.iter().map(|f| &f.0).collect::<Vec<_>>(), gpu_files.iter().map(|f| &f.0).collect::<Vec<_>>());
    for ((name, a), (_, b)) in cpu_files.iter().zip(&gpu_files) {
        let a = image::load_from_memory(a).unwrap().to_rgba8();
        let b = image::load_from_memory(b).unwrap().to_rgba8();
        assert_eq!(a.dimensions(), b.dimensions());
        let worst = a.as_raw().iter().zip(b.as_raw()).map(|(x, y)| x.abs_diff(*y)).max().unwrap();
        assert!(worst <= 3, "{name}: differs by {worst} levels");
    }
}

#[test]
fn analyses_render_their_input_on_the_worker_gpu() {
    let _gpu = GPU_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(g) = Gpu::headless_deferred() else {
        eprintln!("no GPU adapter: skipped");
        return;
    };
    let (mut s, off) = page();
    let l = s.active_comp().unwrap().layers[0].id;
    s.execute("layer.select", json!({"layers": [l.0]})).unwrap();
    s.execute("effect.apply", json!({"layers": [l.0], "effect": "Warp Stabilizer"})).unwrap();
    s.execute("warp.analyze", json!({"layer": l.0, "wait": false})).unwrap();
    let (req, _) = off.0.lock().unwrap().pop().expect("the analysis went to the worker");
    let (cpu, _) = run(req.clone(), None);
    let before = g.deferred_stats().unwrap();
    let (gpu, _) = run(req, Some(&g));
    let after = g.deferred_stats().unwrap();
    let group = |r: &[WorkerReply]| r.iter().find_map(|r| if let WorkerReply::Group { group, .. } = r { Some(group.clone()) } else { None });
    let failed = |r: &[WorkerReply]| r.iter().find_map(|r| if let WorkerReply::Failed { error } = r { Some(error.clone()) } else { None });
    assert_eq!(group(&cpu).is_some(), group(&gpu).is_some(), "cpu {:?} / gpu {:?}", failed(&cpu), failed(&gpu));
    assert!(group(&gpu).is_some(), "{:?}", failed(&gpu));
    // The input frames (the effects above Warp Stabilizer) rendered on the device in passes.
    assert!(after.1 - before.1 >= 6, "readbacks {before:?} → {after:?}");
    assert!(after.0 - before.0 >= 12, "passes {before:?} → {after:?}");
    let progress = gpu.iter().filter(|r| matches!(r, WorkerReply::Progress { .. })).count();
    assert!(progress >= 1);
}

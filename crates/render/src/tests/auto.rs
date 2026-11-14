//! Backend::Auto's per-comp CPU / GPU choice (`auto.rs`), with a mock accelerator.

use std::sync::atomic::{AtomicUsize, Ordering};

use effectcraft_color::Label;
use effectcraft_project::build;
use effectcraft_project::{Comp, ItemId, ItemKind, LayerSource, Project, Solid};
use effectcraft_time::{FrameRate, Tick};

use crate::{Accelerator, AutoKey, AutoPick, Backend, NoFootage, RenderOpts, Renderer};

/// An accelerator that returns a representative pixel (or declines) and keeps Auto history.
struct MockAccel {
    handle: bool,
    calls: AtomicUsize,
    pick: AutoPick,
    effect_calls: AtomicUsize,
}

impl MockAccel {
    /// Construct a renderer stand-in without making assumptions about scheduling or speed.
    fn new(handle: bool) -> MockAccel {
        MockAccel { handle, calls: Default::default(), pick: Default::default(), effect_calls: Default::default() }
    }
    /// Number of full-frame requests dispatched to this accelerator.
    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl Accelerator for MockAccel {
    fn name(&self) -> String {
        "timed".into()
    }
    /// Count dispatched frames and return the same centre pixel as the CPU fixture.
    fn comp_frame(&self, _r: &Renderer, _comp: ItemId, _t: Tick) -> Option<crate::Image> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        // The centre of the comp, as the CPU draws it (a red solid).
        self.handle.then(|| {
            let mut img = crate::Image::new(200, 100);
            img.set(100, 50, [1.0, 0.0, 0.0, 1.0]);
            img
        })
    }
    fn supports_effect(&self, _id: &str) -> bool {
        self.effect_calls.fetch_add(1, Ordering::SeqCst);
        false
    }
    fn effects(&self, _chain: &[crate::FxStep], _buf: &crate::Buf, _levels: Option<f32>) -> Option<crate::Buf> {
        None
    }
    fn auto_pick(&self) -> Option<&AutoPick> {
        Some(&self.pick)
    }
}

/// A small red solid whose centre pixel is shared by the CPU and mock GPU fixtures.
fn project() -> (Project, ItemId) {
    let mut p = Project::default();
    p.settings.gpu_acceleration = true;
    let mut comp = Comp::new(200, 100, FrameRate::FPS_30, Tick::from_seconds_f64(2.0));
    let sid = p.add_item("Solid", Label::Red, None, ItemKind::Solid(Solid { color: [1.0, 0.0, 0.0], width: 50, height: 50, pixel_aspect: 1.0 }));
    let l = build::layer(&mut p, &comp, "Solid", LayerSource::Solid { item: sid }, (50, 50), None);
    comp.layers.push(l);
    let cid = p.add_item("Comp", Label::Sandstone, None, ItemKind::Comp(comp.into()));
    (p, cid)
}

/// Render `n` frames with `backend`; how many the accelerator was asked for.
fn frames(p: &Project, cid: ItemId, a: &MockAccel, backend: Backend, n: usize) -> usize {
    let before = a.calls();
    for _ in 0..n {
        let mut r = Renderer::new(p, &NoFootage, RenderOpts { backend, ..Default::default() });
        r.accel = Some(a);
        let img = r.comp_frame(cid, Tick::ZERO);
        // Whichever compositor ran, the frame is the same.
        assert!(img.get(100, 50)[0] > 0.99 && img.get(100, 50)[3] > 0.99);
    }
    a.calls() - before
}

/// Drive the production selector with measured costs supplied by the test, not the OS clock.
fn measured_frames(pick: &AutoPick, key: AutoKey, cpu_ms: f64, gpu_ms: f64, n: usize) -> usize {
    (0..n)
        .filter(|_| {
            let gpu = pick.choose(key);
            pick.record(key, gpu, if gpu { gpu_ms } else { cpu_ms });
            gpu
        })
        .count()
}

/// A slower GPU is periodically re-probed; explicit GPU and Software Only ignore that history.
#[test]
fn auto_backend_moves_light_comps_to_the_cpu() {
    let (p, cid) = project();
    let slow = MockAccel::new(true);
    let key = AutoKey::new(cid, 1.0, false);
    let n = 6 + 2 * AutoPick::REPROBE as usize;
    let asked = measured_frames(&slow.pick, key, 1.0, 20.0, n);
    assert!((3..=5).contains(&asked), "asked the slow accelerator {asked} times in {n} frames");
    let s = slow.pick.stats(key).unwrap();
    assert!(s.gpu_ms.unwrap() > s.cpu_ms.unwrap(), "{s:?}");
    // Check renderer wiring with known history before a real measurement can change it.
    assert_eq!(frames(&p, cid, &slow, Backend::Auto, 1), 0);
    assert_eq!(frames(&p, cid, &slow, Backend::Gpu, 3), 3);
    let mut sw = p.clone();
    sw.settings.gpu_acceleration = false;
    assert_eq!(frames(&sw, cid, &slow, Backend::Auto, 3), 0);
}

/// Faster, tied and slightly noisier GPU costs stay on the GPU except during warm-up/probes.
#[test]
fn auto_backend_keeps_fast_accelerators() {
    let (p, cid) = project();
    for gpu_ms in [1.0, 5.0, 5.5] {
        let fast = MockAccel::new(true);
        let asked = measured_frames(&fast.pick, AutoKey::new(cid, 1.0, false), 5.0, gpu_ms, 40);
        assert!(asked >= 36, "asked {asked} of 40 for GPU cost {gpu_ms}");
        assert_eq!(frames(&p, cid, &fast, Backend::Auto, 1), 1);
    }
}

/// A declining accelerator is retried only periodically while the CPU supplies frames.
#[test]
fn auto_backend_falls_back_when_the_accelerator_declines() {
    let (p, cid) = project();
    let declining = MockAccel::new(false);
    let n = 2 * AutoPick::REPROBE as usize;
    let asked = frames(&p, cid, &declining, Backend::Auto, n);
    // Asked first, then only on the periodic re-probes.
    assert!((1..=4).contains(&asked), "asked {asked} times in {n} frames");
}

/// Periodic probes detect a formerly slow GPU becoming faster, without mixing cache keys.
#[test]
fn auto_pick_follows_changing_costs() {
    let pick = AutoPick::default();
    let key = AutoKey::new(ItemId(7), 0.5, true);
    let cpu = 5.0;
    let mut gpu = 10.0;
    let (mut early, mut late) = (0, 0);
    for frame in 0..400 {
        // The GPU gets faster halfway (say, its uploads became cached).
        if frame == 200 {
            gpu = 1.0;
        }
        let g = pick.choose(key);
        pick.record(key, g, if g { gpu } else { cpu });
        if (100..200).contains(&frame) && g {
            early += 1;
        }
        if frame >= 300 && g {
            late += 1;
        }
    }
    assert!(early <= 3, "{early} of 100 frames on the slower GPU");
    assert!(late >= 97, "{late} of 100 frames on the faster GPU");
    let s = pick.stats(key).unwrap();
    assert!(s.gpu_ms.unwrap() < 2.0 && s.cpu_ms.unwrap() > 4.0, "{s:?}");
    // Keys are separate per comp, scale and path.
    assert!(pick.stats(AutoKey::new(ItemId(7), 1.0, true)).is_none());
    assert!(pick.stats(AutoKey::new(ItemId(7), 0.5, false)).is_none());
}

/// Counts actual CPU source reads and rejects any CPU fallback inside a GPU error scope.
struct ScopedFootage {
    in_scope: std::sync::atomic::AtomicBool,
    reads: AtomicUsize,
}
impl crate::FootageSource for ScopedFootage {
    fn frame(&self, _: ItemId, f: &effectcraft_project::Footage, _: Tick) -> Option<std::sync::Arc<crate::Image>> {
        assert!(!self.in_scope.load(Ordering::SeqCst), "CPU fallback ran before GPU validation/cleanup");
        self.reads.fetch_add(1, Ordering::SeqCst);
        Some(std::sync::Arc::new(crate::Image::filled(f.width, f.height, [1.0, 0.0, 0.0, 1.0])))
    }
}

fn scoped_project() -> (Project, ItemId) {
    let (mut p, cid) = project();
    let comp = p.comp(cid).unwrap().clone();
    let footage = effectcraft_project::Footage {
        path: "synthetic.png".into(),
        kind: effectcraft_project::FootageKind::Still,
        width: 50,
        height: 50,
        has_video: true,
        duration: Tick::from_seconds_f64(2.0),
        ..Default::default()
    };
    let item = p.add_item("Source", Label::Red, None, ItemKind::Footage(footage));
    let mut layer = build::layer(&mut p, &comp, "Source", LayerSource::Footage { item }, (50, 50), None);
    super::add_effect_200(&mut p, &mut layer, "ec.blur.gaussian", &[]);
    p.comp_mut(cid).unwrap().layers = vec![layer].into();
    (p, cid)
}

/// #3357: a declined GPU frame must not allocate a CPU frame only to discard it when the
/// enclosing GPU scope reports a fault. The caller retries on a fresh CPU renderer once.
#[test]
fn failed_gpu_scope_returns_before_cpu_fallback() {
    let (p, cid) = scoped_project();
    for handled in [false, true] {
        let a = MockAccel::new(handled);
        let src = ScopedFootage { in_scope: false.into(), reads: 0.into() };
        let mut r = Renderer::new(&p, &src, RenderOpts { backend: Backend::Gpu, ..Default::default() });
        r.accel = Some(&a);
        let result = r.comp_frame_scoped(cid, Tick::ZERO, |a| {
            src.in_scope.store(true, Ordering::SeqCst);
            let frame = a.comp_frame(&r, cid, Tick::ZERO);
            drop(frame);
            src.in_scope.store(false, Ordering::SeqCst);
            Err::<Option<crate::Image>, _>("device retired")
        });
        assert_eq!(result.unwrap_err(), "device retired");
        assert_eq!(src.reads.load(Ordering::SeqCst), 0);
        let frame = r.with_accel(None).comp_frame(cid, Tick::ZERO);
        assert!(frame.get(100, 50)[3] > 0.99);
        assert_eq!(src.reads.load(Ordering::SeqCst), 1);
        assert_eq!(a.calls(), 1);
    }
}

/// A healthy decline closes its scope before CPU work, and still records Auto timings.
#[test]
fn healthy_gpu_decline_renders_cpu_once_outside_scope() {
    let (p, cid) = scoped_project();
    let a = MockAccel::new(false);
    let src = ScopedFootage { in_scope: false.into(), reads: 0.into() };
    let mut r = Renderer::new(&p, &src, RenderOpts { backend: Backend::Auto, ..Default::default() });
    r.accel = Some(&a);
    // Auto warms up CPU first: the first frame renders on the CPU without a GPU scope.
    r.comp_frame_scoped(cid, Tick::ZERO, |_| -> Result<Option<crate::Image>, ()> { panic!("Auto's CPU warm-up frame must not enter GPU error scopes") })
        .unwrap();
    assert_eq!(src.reads.load(Ordering::SeqCst), 1);
    assert_eq!(a.calls(), 0);
    // The next one is the GPU's warm-up frame, which the accelerator declines.
    let frame = r
        .comp_frame_scoped(cid, Tick::ZERO, |a| {
            src.in_scope.store(true, Ordering::SeqCst);
            let frame = a.comp_frame(&r, cid, Tick::ZERO);
            src.in_scope.store(false, Ordering::SeqCst);
            Ok::<_, ()>(frame)
        })
        .unwrap();
    assert!(frame.get(100, 50)[3] > 0.99);
    assert_eq!(src.reads.load(Ordering::SeqCst), 2);
    assert_eq!(a.calls(), 1);
    assert_eq!(a.pick.stats(AutoKey::new(cid, 1.0, false)).unwrap().cpu_frames, 2);
    // Auto's next frame stays on CPU, without entering a GPU scope at all.
    r.comp_frame_scoped(cid, Tick::ZERO, |_| -> Result<Option<crate::Image>, ()> { panic!("Auto CPU choice must not enter GPU error scopes") }).unwrap();
    assert_eq!(src.reads.load(Ordering::SeqCst), 3);
    assert_eq!(a.effect_calls.load(Ordering::SeqCst), 0, "CPU effects must not use the GPU outside its scope");
}

/// A preview starts with every lane asking at once, before any frame is measured: the first
/// frames (the ones on screen) render on the CPU; once one is measured, one lane takes a GPU
/// warm-up frame, and the next waits for its time. (Every lane took a GPU warm-up frame on an
/// integrated GPU 2–4× slower than the CPU, and each first playback stalled at its start.)
#[test]
fn auto_pick_warms_the_gpu_up_one_frame_at_a_time() {
    let pick = AutoPick::default();
    let key = AutoKey::new(ItemId(3), 1.0, false);
    let first: Vec<bool> = (0..12).map(|_| pick.choose(key)).collect();
    assert!(!first.iter().any(|g| *g), "{first:?}");
    pick.record(key, false, 10.0);
    let next: Vec<bool> = (0..12).map(|_| pick.choose(key)).collect();
    assert_eq!(next.iter().filter(|g| **g).count(), 1, "{next:?}");
    // The CPU frames finish first: still no second GPU frame while the first runs.
    for _ in 0..22 {
        pick.record(key, false, 10.0);
    }
    assert!(!(0..12).any(|_| pick.choose(key)), "a second GPU warm-up frame while one runs");
    // Measured: the next warm-up frame may go to the GPU.
    pick.record(key, true, 40.0);
    assert!(pick.choose(key));
    // A re-probe of the slow side runs one frame at a time too.
    let pick = AutoPick::default();
    let mut gpu_asked = 0;
    for _ in 0..(4 * AutoPick::REPROBE) {
        // (sequential warm-up, then GPU frames held: as lanes that never report back)
        if pick.choose(key) {
            gpu_asked += 1;
            if pick.stats(key).is_none_or(|s| s.gpu_frames < AutoPick::WARMUP) {
                pick.record(key, true, 40.0);
            }
        } else {
            pick.record(key, false, 10.0);
        }
    }
    assert!(gpu_asked <= AutoPick::WARMUP as usize + 1, "asked the GPU {gpu_asked} times");
}

/// Frames render side by side: twelve CPU frames of 210 ms each at once cost ~18 ms a frame,
/// so a lone GPU frame of 175 ms is the slower side (it measured faster and took the comp).
#[test]
fn auto_pick_counts_frames_running_side_by_side() {
    let pick = AutoPick::default();
    let key = AutoKey::new(ItemId(4), 0.5, false);
    // Twelve lanes: each starts its next frame as soon as one is done.
    let mut lanes: std::collections::VecDeque<bool> = (0..12).map(|_| pick.choose(key)).collect();
    for _ in 0..240 {
        let Some(gpu) = lanes.pop_front() else { break };
        pick.record(key, gpu, if gpu { 175.0 } else { 210.0 });
        lanes.push_back(pick.choose(key));
    }
    let s = pick.stats(key).unwrap();
    assert!(s.cpu_ms.unwrap() < 30.0 && s.gpu_ms.unwrap() > 100.0, "{s:?}");
    assert!(lanes.iter().filter(|g| **g).count() <= 1, "{lanes:?}");
}

/// A frame that ends without a time (a GPU failure, a remote frame replaced before it finished)
/// no longer counts as running: the frames after it are timed as one at a time. (It stayed
/// counted, and every later frame on its side was timed as half its cost.)
#[test]
fn auto_pick_abandoned_frames_stop_counting_as_running() {
    let pick = AutoPick::default();
    let key = AutoKey::new(ItemId(5), 1.0, false);
    let mut dropped = [false; 2];
    for _ in 0..16 {
        let gpu = pick.choose(key);
        if !std::mem::replace(&mut dropped[usize::from(gpu)], true) {
            pick.abandoned(key, gpu);
            continue;
        }
        pick.record(key, gpu, if gpu { 40.0 } else { 10.0 });
    }
    let s = pick.stats(key).unwrap();
    assert_eq!((s.cpu_ms, s.gpu_ms), (Some(10.0), Some(40.0)), "{s:?}");
}

/// A ticket that never reports (dropped on an error, a retry elsewhere, a cancelled export
/// future) ends its frame like [`AutoPick::abandoned`]; one the GPU declined reports from the
/// CPU. (Each exit that skipped `record` left its frame counted as running, and every later
/// frame on that side was timed as one of several side by side.)
#[test]
fn auto_tickets_end_their_frame_on_every_exit() {
    let pick = AutoPick::default();
    let key = AutoKey::new(ItemId(6), 1.0, false);
    let mut dropped = [false; 2];
    for _ in 0..16 {
        let t = pick.ticket(key);
        let gpu = t.gpu();
        if !std::mem::replace(&mut dropped[usize::from(gpu)], true) {
            drop(t);
            continue;
        }
        t.record(if gpu { 40.0 } else { 10.0 });
    }
    let s = pick.stats(key).unwrap();
    assert_eq!((s.cpu_ms, s.gpu_ms), (Some(10.0), Some(40.0)), "{s:?}");
    // Declined: the frame renders on the CPU and reports there, one frame on it.
    let key = AutoKey::new(ItemId(7), 1.0, false);
    pick.ticket(key).record(10.0);
    let mut t = pick.ticket(key);
    assert!(t.gpu(), "the GPU's warm-up frame");
    t.declined();
    assert!(!t.gpu());
    t.record(10.0);
    pick.ticket(key).record(10.0);
    assert_eq!(pick.stats(key).unwrap().cpu_ms, Some(10.0), "timed as one frame on the CPU");
}

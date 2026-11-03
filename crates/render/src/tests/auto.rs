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
}

impl MockAccel {
    /// Construct a renderer stand-in without making assumptions about scheduling or speed.
    fn new(handle: bool) -> MockAccel {
        MockAccel { handle, calls: Default::default(), pick: Default::default() }
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

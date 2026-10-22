//! Backend::Auto's per-comp CPU / GPU choice (`auto.rs`), with a mock accelerator.

use std::sync::atomic::{AtomicUsize, Ordering};

use effectcraft_color::Label;
use effectcraft_project::build;
use effectcraft_project::{Comp, ItemId, ItemKind, LayerSource, Project, Solid};
use effectcraft_time::{FrameRate, Tick};

use crate::{Accelerator, AutoKey, AutoPick, Backend, NoFootage, RenderOpts, Renderer};

/// An accelerator that takes `delay` per frame (or declines with `handle = false`) and keeps
/// Auto timing history.
struct TimedAccel {
    delay: std::time::Duration,
    handle: bool,
    calls: AtomicUsize,
    pick: AutoPick,
}

impl TimedAccel {
    fn new(ms: u64, handle: bool) -> TimedAccel {
        TimedAccel { delay: std::time::Duration::from_millis(ms), handle, calls: Default::default(), pick: Default::default() }
    }
    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl Accelerator for TimedAccel {
    fn name(&self) -> String {
        "timed".into()
    }
    /// Count a timed render attempt and return the synthetic red frame only when enabled.
    fn comp_frame(&self, _r: &Renderer, _comp: ItemId, _t: Tick) -> Option<crate::Image> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        // On Windows a zero-duration sleep yields; a zero-delay mock should do no work.
        if !self.delay.is_zero() {
            std::thread::sleep(self.delay);
        }
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
fn frames(p: &Project, cid: ItemId, a: &TimedAccel, backend: Backend, n: usize) -> usize {
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

#[test]
fn auto_backend_moves_light_comps_to_the_cpu() {
    // A light comp on a slow accelerator: after the warm-up (three frames on
    // each side) Auto renders on the CPU, re-measuring the accelerator now and then.
    let (p, cid) = project();
    // Several times the CPU's frame time (unoptimised test builds are slow).
    let t0 = std::time::Instant::now();
    for _ in 0..3 {
        Renderer::new(&p, &NoFootage, RenderOpts::default()).comp_frame(cid, Tick::ZERO);
    }
    let cpu_ms = t0.elapsed().as_millis() as u64 / 3;
    let slow = TimedAccel::new((cpu_ms * 5).max(20), true);
    let n = 6 + 2 * AutoPick::REPROBE as usize;
    let asked = frames(&p, cid, &slow, Backend::Auto, n);
    assert!((3..=5).contains(&asked), "asked the slow accelerator {asked} times in {n} frames");
    let s = slow.pick.stats(AutoKey::new(cid, 1.0, false)).unwrap();
    assert!(s.gpu_ms.unwrap() > s.cpu_ms.unwrap(), "{s:?}");
    // Backend::Gpu ignores the history; Software Only never asks.
    assert_eq!(frames(&p, cid, &slow, Backend::Gpu, 3), 3);
    let mut sw = p.clone();
    sw.settings.gpu_acceleration = false;
    assert_eq!(frames(&sw, cid, &slow, Backend::Auto, 3), 0);
}

#[test]
fn auto_backend_keeps_fast_accelerators() {
    // As fast as the CPU (ties stay on the accelerator): every frame but the CPU warm-up and
    // re-probes.
    let (p, cid) = project();
    let fast = TimedAccel::new(0, true);
    let asked = frames(&p, cid, &fast, Backend::Auto, 40);
    assert!(asked >= 36, "asked {asked} of 40");
}

#[test]
fn auto_backend_falls_back_when_the_accelerator_declines() {
    let (p, cid) = project();
    let declining = TimedAccel::new(0, false);
    let n = 2 * AutoPick::REPROBE as usize;
    let asked = frames(&p, cid, &declining, Backend::Auto, n);
    // Asked first, then only on the periodic re-probes.
    assert!((1..=4).contains(&asked), "asked {asked} times in {n} frames");
}

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

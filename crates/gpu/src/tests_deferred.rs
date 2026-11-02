//! Deferred readbacks (the browser worker's WebGPU path) vs the CPU: a frame renders in passes
//! until no readback is missing, and the result matches the CPU render. Natively the same
//! device works with readbacks deferred; a device poll delivers them, as the browser's event
//! loop does.

use std::sync::OnceLock;

use effectcraft_keyframe::Value;
use effectcraft_project::build;
use effectcraft_project::{BitDepth, Comp, ItemKind, LayerSource};
use effectcraft_render::{Accelerator, Backend, LayerCache, RenderOpts, Renderer};
use effectcraft_time::{FrameRate, Tick};

use crate::Gpu;
use crate::tests::{Pattern, Scene, diff, n, set, tolerance, v3};

/// The device, held exclusively: deferred state is per device and a frame renders at a time
/// (a frame worker renders one frame at a time too).
pub(crate) fn deferred_gpu() -> Option<(&'static Gpu, std::sync::MutexGuard<'static, ()>)> {
    static G: OnceLock<Result<Gpu, String>> = OnceLock::new();
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    // Also no other GPU test while it runs (see `hold_gpu_lock`).
    crate::tests::hold_gpu_lock();
    let g = G.get_or_init(|| {
        let g = pollster::block_on(Gpu::request_deferred());
        match &g {
            Ok(g) => eprintln!("deferred gpu tests: adapter {}", g.ctx.name),
            Err(e) => eprintln!("deferred gpu tests: no GPU ({e}), skipping"),
        }
        g
    });
    let g = crate::tests::require_gpu("deferred gpu tests", g.as_ref())?;
    Some((g, LOCK.lock().unwrap_or_else(|e| e.into_inner())))
}

/// Footage with GPU effects around a CPU-only one, a solid with Glow, and a precomp whose
/// layer is blurred inside and outside (a chain whose input is another chain's result).
pub(crate) fn scene() -> Scene {
    let cpu_only = "ec.generate.fractal";
    assert!(!effectcraft_effects::GPU_EFFECTS.contains(&cpu_only));
    let mut s = Scene::new(BitDepth::Bpc32);
    let mut bg = s.footage(97, 61);
    s.effect(&mut bg, "ec.blur.gaussian", &[("blurriness", n(5.0))]);
    s.effect(&mut bg, cpu_only, &[]);
    s.effect(&mut bg, "ec.color.levels", &[("gamma", n(1.3))]);
    s.push(bg);
    let mut solid = s.solid([0.9, 0.4, 0.1], 30, 20);
    s.effect(&mut solid, "ec.stylize.glow", &[("threshold", n(20.0)), ("radius", n(6.0))]);
    set(&mut solid, "transform/position", v3(30.0, 20.0));
    s.push(solid);
    // Precomp: a footage layer blurred inside; the precomp layer blurred again outside.
    let inner = Comp::new(60, 40, FrameRate::FPS_30, Tick::from_seconds_f64(2.0));
    let inner_id = s.p.add_item("Inner", effectcraft_color::Label::Blue, None, ItemKind::Comp(inner.clone().into()));
    {
        let mut ins = Scene { p: std::mem::take(&mut s.p), cid: inner_id, comp: inner };
        let mut l = ins.footage(50, 30);
        ins.effect(&mut l, "ec.blur.fastbox", &[("radius", n(3.0))]);
        ins.push(l);
        s.p = ins.p;
    }
    let mut pre = build::layer(&mut s.p, &s.comp, "Inner", LayerSource::Comp { item: inner_id }, (60, 40), None);
    s.effect(&mut pre, "ec.blur.directional", &[("length", n(6.0)), ("direction", Value::Scalar(30.0))]);
    set(&mut pre, "transform/position", v3(60.0, 35.0));
    s.push(pre);
    s
}

/// Render in passes with deferred readbacks; returns the frame and the number of passes.
pub(crate) fn render_passes(g: &Gpu, s: &Scene, opts: RenderOpts, cache: &LayerCache, t: Tick) -> (effectcraft_raster::Image, usize) {
    cache.set_gate(g.miss_gate());
    let mut r = Renderer::new(&s.p, &Pattern, opts);
    r.accel = Some(g);
    r.cache = Some(cache);
    g.frame_begin();
    for pass in 1..=12 {
        g.pass_begin();
        let img = r.comp_frame(s.cid, t);
        if !g.pass_missed() {
            assert_eq!(g.deferred().unwrap().in_flight(), 0);
            return (img, pass);
        }
        // The browser awaits `Gpu::settled`; natively a blocking poll delivers the readbacks.
        g.wait();
        pollster::block_on(g.settled());
    }
    panic!("the frame never completed");
}

#[test]
fn deferred_passes_match_the_cpu() {
    let Some((g, _lock)) = deferred_gpu() else { return };
    assert!(g.context().can_readback() && !g.context().can_wait());
    let s = scene();
    let t = Tick::from_seconds_f64(0.5);
    let cpu = Renderer::new(&s.p, &Pattern, RenderOpts { backend: Backend::Cpu, ..Default::default() }).comp_frame_cpu(s.cid, t);
    let tol = tolerance(BitDepth::Bpc32) * 2.0;
    // The whole frame on the GPU (GPU compositor + chains + the frame's readback).
    let cache = LayerCache::default();
    let (img, passes) = render_passes(g, &s, RenderOpts { backend: Backend::Gpu, ..Default::default() }, &cache, t);
    let d = diff(&cpu, &img, tol);
    assert!(d.over == 0, "whole-frame GPU: {d:?}");
    assert!(passes >= 3, "chains, the nested chain, then the frame readback: {passes} passes");
    // Again (a new frame: results are per frame; the layer cache holds the genuine buffers of
    // the cacheable layers): the same pixels.
    let (img2, passes2) = render_passes(g, &s, RenderOpts { backend: Backend::Gpu, ..Default::default() }, &cache, t);
    assert_eq!(img2.data, img.data);
    assert!(passes2 <= passes, "{passes2} <= {passes}");
    // CPU compositing with GPU chains (a region of interest keeps the frame off the GPU
    // compositor): every placeholder is gone by the last pass.
    let roi = Some([0.0, 0.0, 97.0, 61.0]);
    let cpu_roi = Renderer::new(&s.p, &Pattern, RenderOpts { backend: Backend::Cpu, roi, ..Default::default() }).comp_frame_cpu(s.cid, t);
    let (img3, passes3) = render_passes(g, &s, RenderOpts { backend: Backend::Gpu, roi, ..Default::default() }, &LayerCache::default(), t);
    let d = diff(&cpu_roi, &img3, tol);
    assert!(d.over == 0, "CPU composite, GPU chains: {d:?}");
    assert!(passes3 >= 2);
    let (p, rb, inflight) = g.deferred_stats().unwrap();
    assert!(p >= (passes + passes2 + passes3) as u64 && rb > 0 && inflight == 0);
}

#[test]
fn placeholders_never_reach_the_layer_cache() {
    let Some((g, _lock)) = deferred_gpu() else { return };
    let s = scene();
    let t = Tick::from_seconds_f64(1.0);
    let cache = LayerCache::default();
    cache.set_gate(g.miss_gate());
    let mut r = Renderer::new(&s.p, &Pattern, RenderOpts { backend: Backend::Gpu, roi: Some([0.0, 0.0, 97.0, 61.0]), ..Default::default() });
    r.accel = Some(g);
    r.cache = Some(&cache);
    g.frame_begin();
    g.pass_begin();
    let _ = r.comp_frame(s.cid, t);
    assert!(g.pass_missed());
    // Only buffers computed before the first miss were kept (the first layer's input up to its
    // first chain: nothing with an effect result).
    let first_pass_entries = cache.stats().entries;
    g.wait();
    pollster::block_on(g.settled());
    let (img, _) = render_passes(g, &s, RenderOpts { backend: Backend::Gpu, roi: Some([0.0, 0.0, 97.0, 61.0]), ..Default::default() }, &cache, t);
    assert!(cache.stats().entries > first_pass_entries, "{first_pass_entries} → {:?}", cache.stats());
    // A fresh CPU render through the same cache (served from it) agrees with the CPU.
    let mut cr = Renderer::new(&s.p, &Pattern, RenderOpts { backend: Backend::Cpu, roi: Some([0.0, 0.0, 97.0, 61.0]), ..Default::default() });
    cr.cache = Some(&cache);
    let cached = cr.comp_frame_cpu(s.cid, t);
    let fresh =
        Renderer::new(&s.p, &Pattern, RenderOpts { backend: Backend::Cpu, roi: Some([0.0, 0.0, 97.0, 61.0]), ..Default::default() }).comp_frame_cpu(s.cid, t);
    let tol = tolerance(BitDepth::Bpc32) * 2.0;
    assert_eq!(diff(&fresh, &cached, tol).over, 0);
    assert_eq!(diff(&fresh, &img, tol).over, 0);
}

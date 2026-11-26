//! Deferred readbacks for what job workers and frame workers render besides effect chains:
//! GPU particles and Advanced 3D read back under keys too, so they render on the device in
//! passes; and the job loops' pass driver (`effectcraft_render::passes`) settles natively.

use effectcraft_effects::EffectCtx;
use effectcraft_effects::psim::{EngineDesc, ParticleSim, ParticleSystem, SimRequest};
use effectcraft_project::BitDepth;
use effectcraft_render::three_d::adv;
use effectcraft_render::{Accelerator, Backend, LayerCache, NoFootage, RenderOpts, Renderer};
use effectcraft_time::Tick;

use crate::tests::adv3d::{off_share, run_of, scene, scene_project};
use crate::tests::deferred::{deferred_gpu, render_passes};
use crate::tests::particles::{Host, agree, params};
use crate::tests::{Pattern, diff, n, tolerance};

/// The device's readbacks have landed (the browser's event loop; natively a poll).
fn settle(g: &crate::Gpu) {
    g.wait();
    pollster::block_on(g.settled());
}

#[test]
fn particles_resolve_in_passes() {
    let Some((g, _lock)) = deferred_gpu() else { return };
    let host = Host(g);
    for (id, vals) in [
        ("ec.sim.ccparticleworld", vec![("birthRate", n(2.0)), ("longevity", n(1.2)), ("extras/randomSeed", n(7.0))]),
        ("ec.sim.particleplayground", vec![("grid/particlesAcross", n(0.0)), ("cannon/particlesPerSecond", n(90.0))]),
    ] {
        let p = params(id, &vals);
        let ctx = |env| EffectCtx { params: &p, time: 1.5, layer_size: [320.0, 240.0], seed: 5, adjustment: false, env };
        let run = |c: &EffectCtx| {
            if id == "ec.sim.particleplayground" { effectcraft_effects::playground_state(c) } else { effectcraft_effects::particle_state(id, c) }
        };
        let cpu = run(&ctx(Default::default())).unwrap();
        let env = || effectcraft_effects::EffectEnv { host: Some(&host), ..Default::default() };
        g.frame_begin();
        g.pass_begin();
        // First pass: the simulation runs, its readback starts; no particles yet, the pass missed.
        let first = run(&ctx(env())).unwrap();
        assert!(first.is_empty() && g.pass_missed(), "{id}");
        settle(g);
        g.pass_begin();
        let gpu = run(&ctx(env())).unwrap();
        assert!(!g.pass_missed(), "{id}: served from the readback");
        agree(&format!("deferred {id}"), &cpu, &gpu, 0.01, cpu.len() / 100 + 1);
    }
}

/// The deferred readback key includes the producer history: a request with a different history
/// starts its own readback instead of returning the previous history's particles.
#[test]
fn particle_producer_history_is_part_of_the_deferred_key() {
    let Some((g, _lock)) = deferred_gpu() else { return };
    let system = ParticleSystem::Engine(EngineDesc {
        life: 10.0,
        radius: [0.0; 3],
        anim: 0,
        speed: 0.0,
        gravity: [0.0; 3],
        axis: [0.0, -1.0, 0.0],
        extra: 0.0,
        cone: 0.0,
        damp: 1.0,
        seed: 51,
        flat: true,
    });
    let births = [0u32];
    let first_producers = [[10.0, 0.0, 0.0]];
    let first = SimRequest { key: 0x1955_0def, steps: 1, births: &births, producers: &first_producers, system };
    g.frame_begin();
    g.pass_begin();
    assert!(g.simulate(&first).unwrap().is_empty() && g.pass_missed(), "the first readback starts");
    settle(g);
    g.pass_begin();
    let ready = g.simulate(&first).unwrap();
    assert!(!ready.is_empty() && !g.pass_missed(), "the first readback is ready");
    assert!((ready[0].p[0] - 10.0).abs() < 1e-4, "{:?}", ready[0].p);

    let changed_producers = [[99.0, 0.0, 0.0]];
    let changed = SimRequest { key: first.key, steps: 1, births: &births, producers: &changed_producers, system };
    g.pass_begin();
    assert!(g.simulate(&changed).unwrap().is_empty() && g.pass_missed(), "the producer history was not part of the deferred key");
    settle(g);
    g.pass_begin();
    let changed = g.simulate(&changed).unwrap();
    assert!(!changed.is_empty() && (changed[0].p[0] - 99.0).abs() < 1e-4, "{:?}", changed[0].p);
}

#[test]
fn advanced_3d_resolves_in_passes() {
    let Some((g, _lock)) = deferred_gpu() else { return };
    let (p, cid) = scene_project();
    // One scene rasterised (the CPU compositor's per-scene path).
    let s = scene(&p, cid);
    let cpu = adv::raster::render(&s);
    g.frame_begin();
    g.pass_begin();
    let t = g.raster_3d(&s).expect("handled");
    assert!(g.pass_missed() && t.color.iter().all(|c| c[3] == 0.0), "a placeholder first");
    settle(g);
    g.pass_begin();
    let t = g.raster_3d(&s).expect("handled");
    assert!(!g.pass_missed());
    let (share, _) = off_share(&cpu.color, &t.color);
    assert!(share < 0.015, "raster: {:.3} % of pixels differ", share * 100.0);
    // A whole prepared run (resolve, encoding) read back under its key.
    let (ctx, run) = run_of(&p, cid);
    let out = (ctx.comp.width, ctx.comp.height);
    let r = Renderer::new(&p, &NoFootage, RenderOpts::default());
    let prep = adv::Prepared::new(&r, &ctx, &run, out);
    let (cimg, _) = adv::render_prepared(&prep).expect("cpu drew");
    g.frame_begin();
    g.pass_begin();
    assert!(matches!(g.render_3d(&prep), Some(None)) && g.pass_missed(), "nothing drawn yet: a placeholder");
    settle(g);
    g.pass_begin();
    let (gimg, depth) = g.render_3d(&prep).expect("handled").expect("drawn");
    assert!(!g.pass_missed());
    assert_eq!(depth.len(), (out.0 * out.1) as usize);
    let (share, _) = off_share(&cimg.data, &gimg.data);
    assert!(share <= 0.01, "run: {:.3} % of pixels differ", share * 100.0);
}

/// The job loops' driver: `passes::comp_frame` awaits the device between passes (natively
/// `Accelerator::settle` polls it) and gives the frame `render_passes` gives.
#[test]
fn job_frames_render_in_passes() {
    let Some((g, _lock)) = deferred_gpu() else { return };
    let s = crate::tests::deferred::scene();
    let t = Tick::from_seconds_f64(0.5);
    let cache = LayerCache::default();
    cache.set_gate(g.miss_gate());
    let opts = RenderOpts { backend: Backend::Gpu, ..Default::default() };
    let (want, _) = render_passes(g, &s, opts, &LayerCache::default(), t);
    let mut r = Renderer::new(&s.p, &Pattern, opts);
    r.accel = Some(g);
    r.cache = Some(&cache);
    let before = g.deferred_stats().unwrap();
    let img = effectcraft_render::passes::block_on(effectcraft_render::passes::comp_frame(&r, s.cid, t));
    let after = g.deferred_stats().unwrap();
    assert!(after.0 - before.0 >= 2 && after.1 > before.1, "rendered in passes with readbacks: {before:?} → {after:?}");
    assert_eq!(img.data, want.data);
    let cpu = Renderer::new(&s.p, &Pattern, RenderOpts { backend: Backend::Cpu, ..Default::default() }).comp_frame_cpu(s.cid, t);
    assert_eq!(diff(&cpu, &img, tolerance(BitDepth::Bpc32) * 2.0).over, 0);
    // `in_passes` without a deferred accelerator: one pass, at once.
    let one = effectcraft_render::passes::block_on(effectcraft_render::passes::in_passes(None, |_| 7));
    assert_eq!((one.value, one.passes, one.accelerated), (7, 1, false));
}

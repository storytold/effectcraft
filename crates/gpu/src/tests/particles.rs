//! GPU particles vs the CPU simulation (the oracle): CC Particle World, CC Particle Systems II
//! and Particle Playground's cannon at fixed seeds must give the same live particles (same
//! ids) at positions within tolerance, from scratch and when advancing / seeking from GPU
//! checkpoints.

use effectcraft_effects::psim::{ParticleSim, SimParticle};
use effectcraft_effects::{EffectCtx, EffectEnv, EffectHost, LayerPixels, Params};
use effectcraft_keyframe::Value;

use crate::Gpu;
use crate::tests::{gpu, n};

pub(crate) struct Host(pub(crate) &'static Gpu);

impl EffectHost for Host {
    fn layer(&self, _id: u64, _masks_and_effects: bool) -> Option<LayerPixels> {
        None
    }
    fn audio(&self, _id: u64, _start: f64, _frames: usize, _rate: u32) -> Option<Vec<f32>> {
        None
    }
    fn particles(&self) -> Option<&dyn ParticleSim> {
        Some(self.0)
    }
}

pub(crate) fn params(id: &str, vals: &[(&str, Value)]) -> Params {
    let spec = effectcraft_effects::find(id).unwrap();
    let size = [320.0, 240.0];
    let mut p = Params { values: spec.params.iter().map(|ps| (ps.id.to_string(), effectcraft_effects::default_value(ps, size))).collect() };
    for (k, v) in vals {
        assert!(p.values.contains_key(*k), "{id}: no {k}");
        p.values.insert(k.to_string(), v.clone());
    }
    p
}

/// (CPU, GPU) particles of effect `id` at layer time `t`.
fn states(id: &str, p: &Params, t: f64, host: &Host) -> (Vec<SimParticle>, Vec<SimParticle>) {
    let ctx = |env| EffectCtx { params: p, time: t, layer_size: [320.0, 240.0], seed: 5, adjustment: false, env };
    let run =
        |c: &EffectCtx| if id == "ec.sim.particleplayground" { effectcraft_effects::playground_state(c) } else { effectcraft_effects::particle_state(id, c) };
    let cpu = run(&ctx(EffectEnv::default())).expect("cpu state");
    let gpu = run(&ctx(EffectEnv { host: Some(host), ..Default::default() })).expect("gpu state");
    (cpu, gpu)
}

/// Same ids alive (up to `id_slack` particles dying a step apart), positions within `tol`
/// (world units) for 99 % of them and on average within `tol / 10`.
pub(crate) fn agree(label: &str, cpu: &[SimParticle], gpu: &[SimParticle], tol: f32, id_slack: usize) {
    assert!(cpu.len() > 20, "{label}: the scene has particles ({})", cpu.len());
    let gi: std::collections::HashMap<u32, &SimParticle> = gpu.iter().map(|q| (q.id, q)).collect();
    let mut missing = 0;
    let mut errs = vec![];
    for q in cpu {
        match gi.get(&q.id) {
            Some(g) => errs.push((0..3).map(|k| (q.p[k] - g.p[k]).abs()).fold(0.0f32, f32::max)),
            None => missing += 1,
        }
    }
    let extra = gpu.len() + missing - cpu.len();
    assert!(missing + extra <= id_slack, "{label}: {missing} CPU particles missing on the GPU, {extra} extra (of {})", cpu.len());
    errs.sort_by(f32::total_cmp);
    let mean = errs.iter().sum::<f32>() / errs.len() as f32;
    let p99 = errs[(errs.len() * 99 / 100).min(errs.len() - 1)];
    eprintln!("{label}: {} particles, mean err {mean:.2e}, p99 {p99:.2e}, max {:.2e}", cpu.len(), errs.last().unwrap());
    assert!(p99 <= tol && mean <= tol / 10.0, "{label}: positions differ (mean {mean}, p99 {p99})");
}

#[test]
fn particle_world_matches_the_cpu() {
    let Some(g) = gpu() else { return };
    let host = Host(g);
    for anim in [0u32, 1, 2, 3, 4, 5, 6, 7, 8, 9] {
        let p = params(
            "ec.sim.ccparticleworld",
            &[
                ("physics/animation", Value::Enum(anim)),
                ("birthRate", n(2.0)),
                ("longevity", n(1.2)),
                ("physics/extra", n(0.6)),
                ("physics/resistance", n(0.5)),
                ("extras/randomSeed", n(7.0)),
            ],
        );
        let (cpu, gpu) = states("ec.sim.ccparticleworld", &p, 1.5, &host);
        // Fractal / fire animations sample value noise along the path: rounding differences can
        // grow, so they get a looser bound.
        let tol = if matches!(anim, 6 | 8 | 9) { 1e-3 } else { 1e-4 };
        agree(&format!("particle world anim {anim}"), &cpu, &gpu, tol, cpu.len() / 100 + 1);
    }
}

#[test]
fn particle_systems_ii_matches_the_cpu() {
    let Some(g) = gpu() else { return };
    let host = Host(g);
    for anim in [0u32, 1, 2, 4, 5, 8] {
        let p = params(
            "ec.sim.ccparticlesystems2",
            &[("physics/animation", Value::Enum(anim)), ("birthRate", n(3.0)), ("longevity", n(1.5)), ("physics/gravity", n(0.8)), ("randomSeed", n(3.0))],
        );
        let (cpu, gpu) = states("ec.sim.ccparticlesystems2", &p, 2.0, &host);
        let tol = if anim == 8 { 1e-3 } else { 1e-4 };
        agree(&format!("particle systems ii anim {anim}"), &cpu, &gpu, tol, cpu.len() / 100 + 1);
    }
}

#[test]
fn playground_cannon_matches_the_cpu_and_checkpoints_reuse() {
    let Some(g) = gpu() else { return };
    let host = Host(g);
    for (radius, spread) in [(4.0, 0.0), (-10.0, 50.0)] {
        let p = params(
            "ec.sim.particleplayground",
            &[
                ("grid/particlesAcross", n(0.0)),
                ("cannon/barrelRadius", n(radius)),
                ("cannon/particlesPerSecond", n(90.0)),
                ("cannon/directionRandomSpread", n(30.0)),
                ("cannon/velocityRandomSpread", n(40.0)),
                ("gravity/gravityForceRandomSpread", n(spread)),
            ],
        );
        // Advance through checkpoints (0.5 → 1.0 → 2.0), seek back (1.5), compare each.
        for t in [0.5, 1.0, 2.0, 1.5] {
            let (cpu, gpu) = states("ec.sim.particleplayground", &p, t, &host);
            // Positions are layer pixels here.
            agree(&format!("cannon r {radius} spread {spread} t {t}"), &cpu, &gpu, 0.01, cpu.len() / 100 + 1);
        }
    }
}

/// Through the renderer: the GPU backend's effect host hands the simulation to the GPU, and
/// the composited frames agree with the CPU render.
#[test]
fn particle_effects_render_like_the_cpu() {
    use effectcraft_project::BitDepth;
    use effectcraft_time::Tick;

    use crate::tests::{Scene, check, compare_at, opts};
    for (id, vals) in [
        ("ec.sim.ccparticleworld", vec![("birthRate", n(1.0)), ("physics/animation", Value::Enum(4))]),
        ("ec.sim.ccparticlesystems2", vec![("birthRate", n(2.0))]),
        ("ec.sim.particleplayground", vec![("grid/particlesAcross", n(0.0)), ("cannon/particlesPerSecond", n(60.0))]),
    ] {
        for depth in [BitDepth::Bpc8, BitDepth::Bpc32] {
            let mut s = Scene::new(depth);
            let mut l = s.solid([0.1, 0.1, 0.15], 97, 61);
            s.effect(&mut l, id, &vals);
            s.push(l);
            check(&format!("{id} {depth:?}"), compare_at(&s, opts(), Tick::from_seconds_f64(1.0)), 0.002);
        }
    }
}

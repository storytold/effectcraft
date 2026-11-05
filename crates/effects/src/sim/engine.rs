use super::common::{h, hs};
use super::particle_fx::{MERCURY_CACHE, MERCURY_RATE, PS2_CACHE, PS2_RATE, PW_CACHE, PW_RATE, ps2_phys, pw_phys};
use super::sprites::{Shape, Sprite};
use super::{Buf, EffectCtx, Image, IntoParallelRefMutIterator, ParallelIterator, SimCache, params_key_except, value_noise};

// ------------------------------------------------------------------ particle engine

/// Steps per second of the stepped particle simulations.
pub(crate) const SPS: f64 = 60.0;

/// Number of whole steps reached at layer time `t`.
pub(crate) fn steps_at(t: f64) -> u64 {
    if t <= 0.0 { 0 } else { (t * SPS + 1e-6).floor() as u64 }
}

#[derive(Clone, Debug)]
pub(crate) struct Particle {
    pub p: [f32; 3],
    pub v: [f32; 3],
    pub age: f32,
    pub life: f32,
    /// Per-particle random in [0, 1) (size variation, colour jitter…).
    pub rnd: f32,
    pub id: u32,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct PState {
    pub parts: Vec<Particle>,
    pub carry: f64,
    pub next_id: u32,
    /// The birth rates of the steps so far (see [`rates_hash`]): a checkpoint is only resumed
    /// from with the same history.
    pub hist: u64,
}

/// `hist` after one more step at `rate` particles per second.
fn rates_hash(hist: u64, rate: f64) -> u64 {
    (hist ^ rate.to_bits()).wrapping_mul(0x0100_0000_01b3).rotate_left(5)
}

/// Physics configuration (world units; y down).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Phys {
    /// Particles per second.
    pub rate: f64,
    pub life: f32,
    pub producer: [f32; 3],
    pub radius: [f32; 3],
    pub anim: u32,
    pub speed: f32,
    pub gravity: [f32; 3],
    pub resistance: f32,
    pub axis: [f32; 3],
    pub extra: f32,
    pub extra_angle: f32,
    pub seed: u32,
    pub max: usize,
    /// 2D systems keep z = 0.
    pub flat: bool,
}

pub(crate) const PW_ANIMS: &[&str] =
    &["Explosive", "Direction Axis", "Cone Axis", "Viscouse", "Twirly", "Vortex", "Fire", "Jet Sideways", "Fractal Omni", "Fractal Uni"];

pub(super) fn norm3(v: [f32; 3]) -> [f32; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if l > 1e-9 { [v[0] / l, v[1] / l, v[2] / l] } else { [0.0, -1.0, 0.0] }
}

fn rand_dir(i: u32, k: u32, seed: u32, flat: bool) -> [f32; 3] {
    if flat {
        let a = h(i, k, seed) * std::f32::consts::TAU;
        return [a.cos(), a.sin(), 0.0];
    }
    let z = hs(i, k, seed);
    let a = h(i, k + 1, seed) * std::f32::consts::TAU;
    let r = (1.0 - z * z).max(0.0).sqrt();
    [r * a.cos(), r * a.sin(), z]
}

impl Phys {
    fn spawn(&self, id: u32) -> Particle {
        let seed = self.seed;
        let mut p = [0.0f32; 3];
        // Uniform in the producer ellipsoid.
        let u = rand_dir(id, 1, seed, self.flat);
        let rr = h(id, 3, seed).powf(if self.flat { 0.5 } else { 1.0 / 3.0 });
        for k in 0..3 {
            p[k] = self.producer[k] + u[k] * self.radius[k] * rr;
        }
        if self.flat {
            p[2] = 0.0;
        }
        let ax = norm3(self.axis);
        let jitter = rand_dir(id, 5, seed, self.flat);
        let dir = match self.anim {
            1 | 9 => norm3([ax[0] + jitter[0] * 0.08, ax[1] + jitter[1] * 0.08, ax[2] + jitter[2] * 0.08]),
            2 => {
                let k = (self.extra_angle.to_radians() * 0.5).tan().abs().min(20.0);
                norm3([ax[0] + jitter[0] * k * h(id, 8, seed), ax[1] + jitter[1] * k * h(id, 8, seed), ax[2] + jitter[2] * k * h(id, 8, seed)])
            }
            5 => [jitter[0], 0.0, if self.flat { 0.0 } else { jitter[2] }],
            6 => norm3([jitter[0] * 0.25, -1.0, jitter[2] * 0.25]),
            7 => norm3([1.0, jitter[1] * 0.05, jitter[2] * 0.05]),
            _ => jitter,
        };
        let sp = self.speed * (0.5 + 0.5 * h(id, 9, seed));
        let mut v = [dir[0] * sp, dir[1] * sp, dir[2] * sp];
        if self.flat {
            v[2] = 0.0;
        }
        let life = self.life * (0.8 + 0.4 * h(id, 10, seed));
        Particle { p, v, age: 0.0, life, rnd: h(id, 11, seed), id }
    }

    /// One step at the constant [`Phys::rate`] (the tests' reference).
    #[cfg(test)]
    pub(crate) fn step(&self, st: &mut PState, step: u64) {
        self.step_at_rate(st, step, self.rate);
    }

    /// One step with `rate` particles per second born during it (rather than [`Phys::rate`]):
    /// the rate only decides the births, particles already alive move and age as ever.
    pub(crate) fn step_at_rate(&self, st: &mut PState, step: u64, rate: f64) {
        let dt = (1.0 / SPS) as f32;
        let t = step as f32 * dt;
        let mut drag = self.resistance;
        if self.anim == 3 {
            drag += 3.0 * self.extra.max(0.1);
        }
        let damp = (-drag * dt).exp();
        let seed = self.seed;
        st.parts.par_iter_mut().for_each(|q| {
            let mut a = self.gravity;
            match self.anim {
                4 => {
                    // Twirly: swirl around the vertical axis through the producer.
                    let (dx, dz) = (q.p[0] - self.producer[0], q.p[2] - self.producer[2]);
                    let k = 4.0 * self.extra;
                    a[0] += -dz * k;
                    a[2] += dx * k;
                    if self.flat {
                        let dy = q.p[1] - self.producer[1];
                        a[0] += -dy * k;
                        a[1] += dx * k;
                    }
                }
                5 => {
                    // Vortex: strong tangential + inward pull.
                    let (dx, dy) = (q.p[0] - self.producer[0], q.p[1] - self.producer[1]);
                    let k = 6.0 * self.extra;
                    a[0] += -dy * k - dx * 2.0;
                    a[1] += dx * k - dy * 2.0;
                }
                6 => {
                    // Fire: buoyant, flickering turbulence.
                    let n = value_noise(q.p[0] * 8.0, q.p[1] * 8.0, t * 3.0, seed) - 0.5;
                    a[0] += n * 4.0 * self.extra;
                    a[1] -= self.gravity[1] * 1.5 + 0.3 * self.extra;
                }
                8 | 9 => {
                    let f = 6.0 * self.extra;
                    a[0] += (value_noise(q.p[0] * 5.0, q.p[1] * 5.0, q.p[2] * 5.0 + t, seed) - 0.5) * f;
                    a[1] += (value_noise(q.p[0] * 5.0 + 17.0, q.p[1] * 5.0, q.p[2] * 5.0 + t, seed) - 0.5) * f;
                    if !self.flat {
                        a[2] += (value_noise(q.p[0] * 5.0, q.p[1] * 5.0 + 31.0, q.p[2] * 5.0 + t, seed) - 0.5) * f;
                    }
                }
                _ => {}
            }
            for k in 0..3 {
                q.v[k] = (q.v[k] + a[k] * dt) * damp;
                q.p[k] += q.v[k] * dt;
            }
            if self.flat {
                q.p[2] = 0.0;
                q.v[2] = 0.0;
            }
            q.age += dt;
        });
        st.parts.retain(|q| q.age < q.life);
        st.hist = rates_hash(st.hist, rate);
        st.carry += rate / SPS;
        let n = st.carry.floor() as u32;
        st.carry -= n as f64;
        for _ in 0..n {
            if st.parts.len() >= self.max {
                break;
            }
            let id = st.next_id;
            st.next_id = st.next_id.wrapping_add(1);
            // Sub-step birth offset keeps continuous streams smooth.
            let mut q = self.spawn(id);
            let frac = h(id, 12, seed) * dt;
            for k in 0..3 {
                q.p[k] += q.v[k] * frac;
            }
            q.age = frac;
            st.parts.push(q);
        }
    }
}

impl Phys {
    /// The backend description (per-step and per-spawn constants evaluated here, in f32).
    pub(crate) fn desc(&self) -> crate::psim::EngineDesc {
        let mut drag = self.resistance;
        if self.anim == 3 {
            drag += 3.0 * self.extra.max(0.1);
        }
        let dt = (1.0 / SPS) as f32;
        crate::psim::EngineDesc {
            life: self.life,
            producer: self.producer,
            radius: self.radius,
            anim: self.anim,
            speed: self.speed,
            gravity: self.gravity,
            axis: norm3(self.axis),
            extra: self.extra,
            cone: (self.extra_angle.to_radians() * 0.5).tan().abs().min(20.0),
            damp: (-drag * dt).exp(),
            seed: self.seed,
            flat: self.flat,
        }
    }
}

/// The state at layer time `ctx.time` from the host's particle backend (GPU particles), or
/// `None` (simulate on the CPU). `rates`: the birth rate of each step, when it changes over time
/// (else [`Phys::rate`]).
pub(super) fn accelerated(ctx: &EffectCtx, key: u64, phys: &Phys, rates: Option<&[f64]>) -> Option<std::sync::Arc<PState>> {
    let backend = ctx.env.host?.particles()?;
    let steps = steps_at(ctx.time);
    let births = match rates {
        Some(r) => crate::psim::births_at(steps, phys.max, |i| r.get(i as usize).copied().unwrap_or(0.0))?,
        None => crate::psim::births(phys.rate, steps, phys.max)?,
    };
    let req = crate::psim::SimRequest { key, steps, births: &births, system: crate::psim::ParticleSystem::Engine(phys.desc()) };
    let parts = backend.simulate(&req)?;
    Some(std::sync::Arc::new(PState {
        parts: parts.into_iter().map(|q| Particle { p: q.p, v: q.v, age: q.age, life: q.life, rnd: q.rnd, id: q.id }).collect(),
        carry: 0.0,
        next_id: births.len() as u32,
        hist: 0,
    }))
}

/// The birth rate (particles per second) of each step up to layer time `ctx.time`: parameter
/// `id` × `k`, sampled where the step starts (a keyframed or expression-driven Birth Rate only
/// decides the births of its own time, #1373). Without a host that evaluates the effect at
/// other times, the current value for every step.
pub(super) fn rate_history(ctx: &EffectCtx, id: &str, k: f64) -> Vec<f64> {
    let now = ctx.params.f(id);
    // A rate without keyframes or an expression is the same at every step.
    let host = ctx.env.host.filter(|h| !h.param_is_static(ctx.env.effect_index, id));
    (0..steps_at(ctx.time))
        .map(|i| {
            let v = host.and_then(|h| h.param_at(ctx.env.effect_index, id, i as f64 / SPS)).map_or(now, |v| v.as_f64());
            if v.is_finite() { v.max(0.0) * k } else { 0.0 }
        })
        .collect()
}

/// Run a particle system through `rates` (one birth rate per step, see [`rate_history`]) with
/// `cache`, resuming only from checkpoints reached with the same rates.
pub(super) fn simulate_rates(cache: &SimCache<PState>, key: u64, phys: &Phys, rates: &[f64]) -> std::sync::Arc<PState> {
    // `hist` of the state after each number of steps.
    let mut prefix = Vec::with_capacity(rates.len() + 1);
    prefix.push(0u64);
    for r in rates {
        prefix.push(rates_hash(prefix.last().copied().unwrap_or(0), *r));
    }
    let rate = |i: u64| rates.get(i as usize).copied().unwrap_or(0.0);
    cache.run_where(key, rates.len() as u64, PState::default, |st, i| phys.step_at_rate(st, i, rate(i)), |s, st| prefix.get(s as usize) == Some(&st.hist))
}

/// A stepped particle system's particles at `ctx.time`, born at the Birth Rate of their own
/// time (`birthRate` × `k` particles per second, see [`rate_history`]). The cache key is every
/// parameter but Birth Rate (its history is checked through `PState::hist`) and those under
/// `skip`, with `salt`. `gpu`: the host's particle backend may simulate it.
pub(super) struct Stepped<'a> {
    pub(super) cache: &'a SimCache<PState>,
    pub(super) salt: u64,
    pub(super) skip: &'a [&'a str],
    pub(super) k: f64,
    pub(super) gpu: bool,
}

impl Stepped<'_> {
    pub(super) fn state(&self, ctx: &EffectCtx, phys: &Phys) -> std::sync::Arc<PState> {
        let rates = rate_history(ctx, "birthRate", self.k);
        let mut skip = self.skip.to_vec();
        skip.push("birthRate");
        let key = params_key_except(ctx, &Buf { img: Image::new(0, 0), offset: [0.0; 2], scale: 1.0 }, self.salt, &skip);
        let gpu = if self.gpu { accelerated(ctx, key, phys, Some(&rates)) } else { None };
        gpu.unwrap_or_else(|| simulate_rates(self.cache, key, phys, &rates))
    }
}

/// CC Particle World.
const PW: Stepped = Stepped { cache: &PW_CACHE, salt: 1, skip: &[PW_EFFECT_CAMERA], k: PW_RATE, gpu: true };
/// CC Particle Systems II.
pub(super) const PS2: Stepped = Stepped { cache: &PS2_CACHE, salt: 2, skip: &[], k: PS2_RATE, gpu: true };
/// CC Mr. Mercury (its blobs are simulated on the CPU).
pub(super) const MERCURY: Stepped = Stepped { cache: &MERCURY_CACHE, salt: 3, skip: &[], k: MERCURY_RATE, gpu: false };

/// CC Particle World's particles at `ctx.time`, born at the Birth Rate of their own time.
pub(super) fn pw_state(ctx: &EffectCtx, phys: &Phys) -> std::sync::Arc<PState> {
    PW.state(ctx, phys)
}

/// The live particles of a stepped particle effect (`ec.sim.ccparticleworld`,
/// `ec.sim.ccparticlesystems2`) at `ctx.time`: from the host's particle backend when it has
/// one (GPU particles), else from the CPU simulation. For comparing the two.
pub fn particle_state(id: &str, ctx: &EffectCtx) -> Option<Vec<crate::psim::SimParticle>> {
    let st = match id {
        "ec.sim.ccparticleworld" => pw_state(ctx, &pw_phys(ctx)),
        "ec.sim.ccparticlesystems2" => PS2.state(ctx, &ps2_phys(ctx)),
        _ => return None,
    };
    Some(st.parts.iter().map(|q| crate::psim::SimParticle { p: q.p, v: q.v, age: q.age, life: q.life, rnd: q.rnd, id: q.id }).collect())
}

/// The simulation cache key of a stepped particle effect at a constant Birth Rate (the tests'
/// reference): every parameter but CC Particle World's Effect Camera.
#[cfg(test)]
pub(super) fn sim_key(ctx: &EffectCtx, salt: u64) -> u64 {
    params_key_except(ctx, &Buf { img: Image::new(0, 0), offset: [0.0; 2], scale: 1.0 }, salt, &[PW_EFFECT_CAMERA])
}

/// Run a particle system at its constant [`Phys::rate`] to layer time `t` through `cache` (the
/// tests' reference).
#[cfg(test)]
pub(crate) fn simulate(cache: &SimCache<PState>, key: u64, t: f64, phys: &Phys) -> std::sync::Arc<PState> {
    cache.run(key, steps_at(t), PState::default, |st, i| phys.step(st, i))
}

pub(crate) const PARTICLE_TYPES: &[&str] =
    &["Line", "Shaded Sphere", "Faded Sphere", "Bubble", "Star", "Motion Polygon", "Tri-Polygon", "Cube", "Textured Square"];

/// Colour/size/opacity of a particle at its age.
pub(super) fn particle_look(q: &Particle, birth: [f32; 4], death: [f32; 4], bsize: f32, dsize: f32, svar: f32, max_op: f32) -> ([f32; 4], f32) {
    let u = (q.age / q.life.max(1e-4)).clamp(0.0, 1.0);
    let size = (bsize + (dsize - bsize) * u) * (1.0 + svar * (q.rnd * 2.0 - 1.0));
    let fade = (u * 20.0).min(1.0) * ((1.0 - u) * 5.0).min(1.0);
    let c = [birth[0] + (death[0] - birth[0]) * u, birth[1] + (death[1] - birth[1]) * u, birth[2] + (death[2] - birth[2]) * u, max_op * fade];
    (c, size.max(0.0))
}

pub(super) fn type_sprite(kind: u32, x: f32, y: f32, r: f32, c: [f32; 4], velocity: [f32; 2], rot: f32) -> Sprite {
    let [vx, vy] = velocity;
    let vl = (vx * vx + vy * vy).sqrt().max(1e-6);
    let shape = match kind {
        0 => Shape::Line { dx: -vx / vl * r * 2.0, dy: -vy / vl * r * 2.0 },
        1 => Shape::Sphere,
        2 => Shape::Faded,
        3 => Shape::Bubble,
        4 => Shape::Star,
        5 => Shape::Line { dx: -vx * 0.05, dy: -vy * 0.05 },
        6 => Shape::Tri,
        _ => Shape::Square,
    };
    let rr = match kind {
        0 => (r * 0.08).max(0.5),
        5 => r * 0.5,
        _ => r,
    };
    Sprite { x, y, r: rr, c, shape, rot }
}

/// CC Particle World's Extras ▸ Effect Camera parameters (id prefix).
pub(super) const PW_EFFECT_CAMERA: &str = "extras/effectCamera/";

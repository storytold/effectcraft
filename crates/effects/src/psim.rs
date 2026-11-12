//! Pluggable particle simulation (GPU particles, M12.7).
//!
//! The stepped particle systems (CC Particle World, CC Particle Systems II and Particle
//! Playground's Cannon without interacting forces) evolve every particle independently: births
//! come from a rate (the same `carry` arithmetic as the CPU step), and each particle integrates
//! its own motion with the same fixed step. An accelerator ([`ParticleSim`], reached through
//! [`crate::EffectHost::particles`]) can therefore simulate all of them in parallel and return
//! the state the CPU's `SimCache` would reach. The CPU stays the oracle: anything the backend
//! declines (interacting forces, caps that would drop births) simulates on the CPU.

/// CC Particle World / CC Particle Systems II physics (see `sim::Phys`), with the per-step and
/// per-spawn constants already evaluated on the CPU (bit-identical to the CPU's).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EngineDesc {
    pub life: f32,
    pub producer: [f32; 3],
    pub radius: [f32; 3],
    /// Animation (0 Explosive … 9 Fractal Uni).
    pub anim: u32,
    pub speed: f32,
    pub gravity: [f32; 3],
    /// Normalised direction axis.
    pub axis: [f32; 3],
    pub extra: f32,
    /// Cone Axis spread factor `|tan(extra angle / 2)|` (≤ 20).
    pub cone: f32,
    /// Velocity damping per step `exp(−drag · dt)`.
    pub damp: f32,
    pub seed: u32,
    /// 2D systems keep z = 0.
    pub flat: bool,
}

/// Particle Playground's Cannon with gravity only (see `sim3::Pg`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CannonDesc {
    pub pos: [f32; 2],
    pub barrel_angle: f32,
    pub barrel_radius: f32,
    pub dir_spread: f32,
    pub vel: f32,
    pub vel_spread: f32,
    pub gravity: [f32; 2],
    pub grav_spread: f32,
    pub seed: u32,
    /// Particles leaving `[x0, y0, x1, y1]` are removed.
    pub bounds: [f32; 4],
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ParticleSystem {
    Engine(EngineDesc),
    Cannon(CannonDesc),
}

/// One simulation request: the state after `steps` steps of system `key`.
#[derive(Clone, Debug)]
pub struct SimRequest<'a> {
    /// Identifies the system (parameters, seed); checkpoints are reused per key.
    pub key: u64,
    pub steps: u64,
    /// Birth step of every particle id born before `steps` (ascending).
    pub births: &'a [u32],
    pub system: ParticleSystem,
}

/// A live particle (ascending ids, as the CPU state keeps them).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SimParticle {
    pub p: [f32; 3],
    pub v: [f32; 3],
    pub age: f32,
    pub life: f32,
    pub rnd: f32,
    pub id: u32,
}

/// A particle simulation backend (the GPU).
pub trait ParticleSim: Send + Sync {
    /// The live particles after `req.steps` steps, or `None` (simulate on the CPU).
    fn simulate(&self, req: &SimRequest) -> Option<Vec<SimParticle>>;
}

/// Birth step of every particle born in the first `steps` steps at `rate` particles per second
/// (the CPU step's `carry` arithmetic, exactly). `None` when more than `cap` would be born (the
/// CPU drops births at its live-particle cap, which an independent simulation can't know).
pub fn births(rate: f64, steps: u64, cap: usize) -> Option<Vec<u32>> {
    if rate / SPS * steps as f64 > cap as f64 + 2.0 {
        return None;
    }
    births_at(steps, cap, |_| rate)
}

/// [`births`] with the rate of each step, `rate(i)` particles per second during step `i` (a
/// keyframed Birth Rate, #1373).
pub fn births_at(steps: u64, cap: usize, rate: impl Fn(u64) -> f64) -> Option<Vec<u32>> {
    let mut out = Vec::new();
    let mut carry = 0.0f64;
    for i in 0..steps {
        carry += rate(i) / SPS;
        let n = carry.floor() as u32;
        carry -= n as f64;
        if n as usize > cap.saturating_sub(out.len()) {
            return None;
        }
        out.extend(std::iter::repeat_n(i as u32, n as usize));
    }
    Some(out)
}

/// Steps per second of the stepped simulations.
pub const SPS: f64 = crate::sim::SPS;

//! GPU particles (`particles.wgsl`): the stepped particle systems of `effectcraft-effects`
//! ([`effectcraft_effects::psim`]) simulated with one invocation per particle. States stay on
//! the GPU as checkpoints (per system key, like the CPU's `SimCache`), so playback advances from
//! the previous frame's state instead of simulating from layer time 0.

use std::sync::{Mutex, OnceLock};

use effectcraft_effects::psim::{ParticleSystem, SimParticle, SimRequest};

use crate::context::GpuContext;

/// Bytes of one particle state (`PState` in the shader).
const STATE_BYTES: u64 = 48;
/// Checkpoints kept (all keys).
const CHECKPOINTS: usize = 24;

pub(crate) struct Pipes {
    bgl: wgpu::BindGroupLayout,
    pipe: wgpu::ComputePipeline,
}

struct Checkpoint {
    key: u64,
    steps: u64,
    n: u32,
    /// The birth steps of its particles (see [`born`]).
    born: u64,
    buf: wgpu::Buffer,
    used: u64,
}

/// The particles born in the first `steps` steps of `births` (ascending birth steps) and a hash
/// of their birth steps: a checkpoint is only resumed from when the request's births before it
/// are the same (a keyframed Birth Rate isn't part of the system key, #1373).
fn born(births: &[u32], steps: u64) -> (u32, u64) {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    let mut n = 0u32;
    for &b in births.iter().take_while(|&&b| u64::from(b) < steps) {
        h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
        n += 1;
    }
    (n, h)
}

#[derive(Default)]
pub(crate) struct Checkpoints {
    list: Vec<Checkpoint>,
    clock: u64,
}

impl Checkpoints {
    /// Forget every kept state (after a failed frame, whose buffers may be invalid).
    pub(crate) fn clear(&mut self) {
        self.list.clear();
    }
}

fn pipes(g: &GpuContext) -> &Pipes {
    g.particles.get_or_init(|| {
        let d = &g.device;
        let src = [include_str!("shaders/common.wgsl"), include_str!("shaders/kernels.wgsl"), include_str!("shaders/particles.wgsl")].concat();
        let module =
            d.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("effectcraft particles"), source: wgpu::ShaderSource::Wgsl(src.into()) });
        let buffer = |binding: u32, ty: wgpu::BufferBindingType| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer { ty, has_dynamic_offset: false, min_binding_size: None },
            count: None,
        };
        let bgl = d.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("effectcraft particles"),
            entries: &[
                buffer(0, wgpu::BufferBindingType::Uniform),
                buffer(1, wgpu::BufferBindingType::Storage { read_only: true }),
                buffer(2, wgpu::BufferBindingType::Storage { read_only: true }),
                buffer(3, wgpu::BufferBindingType::Storage { read_only: false }),
            ],
        });
        let layout = d.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("effectcraft particles"),
            bind_group_layouts: &[None, Some(&bgl)],
            immediate_size: 0,
        });
        let pipe = d.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("particles"),
            layout: Some(&layout),
            module: &module,
            entry_point: Some("particles"),
            compilation_options: Default::default(),
            cache: None,
        });
        Pipes { bgl, pipe }
    })
}

/// The shader's parameter block.
fn uniforms(req: &SimRequest, n: u32, s0: u64, prev_n: u32) -> Vec<u8> {
    let dt = (1.0 / effectcraft_effects::psim::SPS) as f32;
    let mut u = [n, s0 as u32, req.steps as u32, 0];
    let u2;
    let mut f = [[0.0f32; 4]; 6];
    match &req.system {
        ParticleSystem::Engine(e) => {
            u2 = [e.anim, e.flat as u32, e.seed, prev_n];
            f[0] = [e.life, e.speed, e.extra, e.cone];
            f[1] = [e.producer[0], e.producer[1], e.producer[2], 0.0];
            f[2] = [e.radius[0], e.radius[1], e.radius[2], 0.0];
            f[3] = [e.gravity[0], e.gravity[1], e.gravity[2], 0.0];
            f[4] = [e.axis[0], e.axis[1], e.axis[2], 0.0];
            f[5] = [e.damp, dt, 0.0, 0.0];
        }
        ParticleSystem::Cannon(c) => {
            u[3] = 1;
            u2 = [0, 1, c.seed, prev_n];
            f[0] = [c.pos[0], c.pos[1], c.barrel_angle, c.barrel_radius];
            f[1] = [c.dir_spread, c.vel, c.vel_spread, c.grav_spread];
            f[2] = [c.gravity[0], c.gravity[1], dt, 0.0];
            f[3] = c.bounds;
        }
    }
    let mut out = Vec::with_capacity(128);
    for x in u.iter().chain(&u2) {
        out.extend_from_slice(&x.to_le_bytes());
    }
    for x in f.iter().flatten() {
        out.extend_from_slice(&x.to_le_bytes());
    }
    out
}

/// Simulations dispatched on the GPU (tests check the GPU path really ran).
#[cfg(test)]
pub(crate) static DISPATCHES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Simulate `req` (see [`effectcraft_effects::psim::ParticleSim`]).
///
/// With deferred readbacks (a browser worker) the state's readback is keyed by the request
/// ([`crate::deferred`]): the first pass starts it and gets no particles (the pass misses and is
/// rendered again), later passes read the bytes.
pub(crate) fn simulate(g: &GpuContext, req: &SimRequest) -> Option<Vec<SimParticle>> {
    if !g.can_readback() || req.steps > u32::MAX as u64 {
        return None;
    }
    let n = req.births.len() as u32;
    if n == 0 {
        return Some(vec![]);
    }
    let deferred = g.deferred.clone();
    let key = deferred.as_ref().map(|_| sim_key(req));
    if let (Some(d), Some(k)) = (&deferred, key) {
        match d.lookup(k) {
            crate::deferred::Lookup::Ready(r) => return r.map(|r| parse(&r.bytes)),
            crate::deferred::Lookup::InFlight => {
                d.miss();
                return Some(vec![]);
            }
            crate::deferred::Lookup::Absent => {}
        }
    }
    let size = n as u64 * STATE_BYTES;
    if size > g.device.limits().max_storage_buffer_binding_size {
        return None;
    }
    let p = pipes(g);
    // The latest checkpoint at or before the target step.
    let (s0, prev, prev_n) = {
        let mut c = g.particle_states.lock().ok()?;
        c.clock += 1;
        let now = c.clock;
        match c.list.iter_mut().filter(|k| k.key == req.key && k.steps <= req.steps && born(req.births, k.steps) == (k.n, k.born)).max_by_key(|k| k.steps) {
            Some(k) => {
                k.used = now;
                (k.steps, Some(k.buf.clone()), k.n)
            }
            None => (0, None, 0),
        }
    };
    let d = &g.device;
    let births: Vec<u8> = req.births.iter().flat_map(|b| b.to_le_bytes()).collect();
    let births = g.buffer_init(Some("births"), &births, wgpu::BufferUsages::STORAGE);
    let prev = prev.unwrap_or_else(|| {
        d.create_buffer(&wgpu::BufferDescriptor { label: Some("particles"), size: STATE_BYTES, usage: wgpu::BufferUsages::STORAGE, mapped_at_creation: false })
    });
    let next = d.create_buffer(&wgpu::BufferDescriptor {
        label: Some("particles"),
        size,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let ub = g.buffer_init(Some("particles"), &uniforms(req, n, s0, prev_n), wgpu::BufferUsages::UNIFORM);
    let bg = d.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &p.bgl,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: ub.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: births.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 2, resource: prev.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 3, resource: next.as_entire_binding() },
        ],
    });
    let read = d.create_buffer(&wgpu::BufferDescriptor {
        label: Some("particles readback"),
        size,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut enc = d.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("particles") });
    {
        let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("particles"), timestamp_writes: None });
        pass.set_pipeline(&p.pipe);
        pass.set_bind_group(1, &bg, &[]);
        pass.dispatch_workgroups(n.div_ceil(64), 1, 1);
    }
    enc.copy_buffer_to_buffer(&next, 0, &read, 0, size);
    g.queue.submit([enc.finish()]);
    #[cfg(test)]
    DISPATCHES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    if let (Some(d), Some(k)) = (&deferred, key) {
        let row = usize::try_from(size).ok()?;
        let done = d.start(k, n, 1, [0.0; 2], 1.0);
        g.map_readback(&read, row, row, 1, move |r| done(r.map_err(|e| log::error!("gpu particle readback: {e}")).ok()));
        #[cfg(not(target_arch = "wasm32"))]
        let _ = g.poll_readbacks(wgpu::PollType::Poll);
        keep_checkpoint(g, req, n, next);
        d.miss();
        return Some(vec![]);
    }
    let row = usize::try_from(size).ok()?;
    let bytes = g.read_buffer(&read, row, row, 1).map_err(|e| log::error!("gpu particle readback: {e}")).ok()?;
    let parts = parse(&bytes);
    keep_checkpoint(g, req, n, next);
    Some(parts)
}

/// The deferred readback key of a request: everything that determines the state.
fn sim_key(req: &SimRequest) -> u128 {
    let mut k = crate::deferred::Key::new(3);
    k.u64(req.key);
    k.u64(req.steps);
    k.u64(req.births.len() as u64);
    k.bytes(&req.births.iter().flat_map(|b| b.to_le_bytes()).collect::<Vec<u8>>());
    k.debug(&req.system);
    k.finish()
}

/// Live particles of a state buffer's bytes.
fn parse(bytes: &[u8]) -> Vec<SimParticle> {
    let f: Vec<f32> = bytes.as_chunks::<4>().0.iter().map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
    f.as_chunks::<12>()
        .0
        .iter()
        .enumerate()
        .filter(|(_, s)| s[9] != 0.0)
        .map(|(id, s)| SimParticle { p: [s[0], s[1], s[2]], v: [s[4], s[5], s[6]], age: s[3], life: s[7], rnd: s[8], id: id as u32 })
        .collect()
}

/// Keep a simulated state on the GPU as a checkpoint (the oldest go beyond the limit).
fn keep_checkpoint(g: &GpuContext, req: &SimRequest, n: u32, next: wgpu::Buffer) {
    if let Ok(mut c) = g.particle_states.lock() {
        let now = c.clock;
        c.list.retain(|k| !(k.key == req.key && k.steps == req.steps));
        let (_, h) = born(req.births, req.steps);
        c.list.push(Checkpoint { key: req.key, steps: req.steps, n, born: h, buf: next, used: now });
        while c.list.len() > CHECKPOINTS {
            let Some(i) = c.list.iter().enumerate().min_by_key(|(_, k)| k.used).map(|(i, _)| i) else { break };
            c.list.remove(i);
        }
    }
}

pub(crate) type PipesCell = OnceLock<Pipes>;
pub(crate) type StatesCell = Mutex<Checkpoints>;

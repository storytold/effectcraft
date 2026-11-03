//! Simulation: Particle Playground and CC Hair.
//!
//! **Particle Playground** is a deterministic 2D particle system stepped from layer time 0 at
//! [`SPS`] steps per second through [`SimCache`] (seeking anywhere gives the same frame as
//! playing up to it). Producers: **Cannon** (a stream from a barrel with angle, radius, rate,
//! velocity and random spreads), **Grid** (particles placed on a grid at time 0) and **Layer
//! Exploder** (another layer broken into particles at time 0). Forces: **Gravity** (force,
//! random spread, direction), **Repel** (particles push each other apart within a radius),
//! **Wall** (a mask of the layer the particles bounce off) and the **Persistent Property
//! Mapper** (a map layer's red / green / blue drive particle properties at the particle's
//! position each step) and the **Ephemeral Property Mapper** (the same with an operator per
//! channel, holding for one step only). **Particle Exploder** bursts particles into smaller
//! ones. **Layer Map** draws each particle as a copy of another layer, at a time set by its Time
//! Offset Type. Every producer / force / mapper has an **Affects** group (particles from which
//! producer, a selection map, characters, age with feather). The Options' cannon / grid text
//! turns particles into characters (the built-in stroke font). Map layers are read at the frame
//! being rendered (their pixels are part of the cache key). Particles replace the layer's own
//! pixels, as in After Effects.
//!
//! **CC Hair** grows strands from the layer's opaque pixels (Density per 1 000 px²), each a
//! quadratic curve of Length that droops with Weight, optionally steered and scaled by a hairfall
//! map layer (luminance → length, horizontal/vertical gradient → lean), and shades them with a
//! Kajiya–Kay-style strand model (diffuse ∝ sin(tangent, light), specular from the half vector).

use effectcraft_keyframe::Value;
use effectcraft_project::ParamUi;
use effectcraft_raster::{Image, Px};
use rayon::prelude::*;

use crate::sim::{Acc, Post, SPS, Shape, Sprite, SpritePlan, Tint, combine, splat, steps_at};
use crate::util::{SimCache, hash1, params_key, point_in_poly, unpremul};
use crate::{Buf, EffectCtx, EffectSpec, col, num, p, popup, slider};

fn spec(id: &'static str, name: &'static str, params: Vec<crate::ParamSpec>, render: crate::RenderFn) -> EffectSpec {
    EffectSpec { id, name, category: "Simulation", params, render, gpu: false, float: true }
}

// ---------------------------------------------------------------- Particle Playground

/// Which producer made a particle (Affects ▸ Particles From).
pub const SRC_CANNON: u8 = 0;
pub const SRC_GRID: u8 = 1;
pub const SRC_LAYER_EXPLODER: u8 = 2;
pub const SRC_PARTICLE_EXPLODER: u8 = 3;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PgParticle {
    pub p: [f32; 2],
    pub v: [f32; 2],
    /// Layer position where the particle was born.
    pub origin: [f32; 2],
    pub c: [f32; 4],
    /// Radius, or font size for text particles.
    pub r: f32,
    pub mass: f32,
    /// Kinetic friction (velocity damping) and static friction (force needed to start moving).
    pub friction: f32,
    pub static_friction: f32,
    pub force: [f32; 2],
    /// Rotation (degrees), angular velocity (°/s) and torque (°/s²).
    pub angle: f32,
    pub ang_vel: f32,
    pub torque: f32,
    /// X / Y scale (1 = 100 %) and its growth per second.
    pub scale: [f32; 2],
    pub scale_speed: f32,
    /// Age and lifespan in seconds.
    pub age: f32,
    pub lifespan: f32,
    /// Character code of a text particle (0 = a dot).
    pub ch: u32,
    /// Layer Map time offset (seconds).
    pub toff: f32,
    /// Producer (`SRC_*`).
    pub src: u8,
    pub id: u32,
}

impl PgParticle {
    pub fn new(p: [f32; 2], v: [f32; 2], c: [f32; 4], r: f32, id: u32, src: u8) -> PgParticle {
        PgParticle {
            p,
            v,
            origin: p,
            c,
            r,
            mass: 1.0,
            friction: 0.0,
            static_friction: 0.0,
            force: [0.0; 2],
            angle: 0.0,
            ang_vel: 0.0,
            torque: 0.0,
            scale: [1.0; 2],
            scale_speed: 0.0,
            age: 0.0,
            lifespan: f32::INFINITY,
            ch: 0,
            toff: 0.0,
            src,
            id,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct PgState {
    pub parts: Vec<PgParticle>,
    carry: f64,
    next_id: u32,
}

/// A map layer sampled in layer coordinates.
struct MapImg {
    img: Image,
    /// Layer point → pixel: (x · k₀ + o₀, y · k₁ + o₁).
    k: [f64; 2],
    o: [f64; 2],
}

impl MapImg {
    fn from(ctx: &EffectCtx, lp: crate::LayerPixels) -> MapImg {
        // Fit the map layer to this layer (stretch), like AE's layer maps.
        let (ls, os) = (ctx.layer_size, lp.size);
        let sx = if ls[0] > 0.0 { os[0] / ls[0] } else { 1.0 };
        let sy = if ls[1] > 0.0 { os[1] / ls[1] } else { 1.0 };
        MapImg { img: lp.buf.img, k: [lp.buf.scale * sx, lp.buf.scale * sy], o: lp.buf.offset }
    }
    fn at(&self, x: f32, y: f32) -> Px {
        let (c, a) = unpremul(self.img.sample_bilinear_clamped(x as f64 * self.k[0] + self.o[0], y as f64 * self.k[1] + self.o[1]));
        [c[0], c[1], c[2], a]
    }
}

/// Particle properties a Property Mapper channel can drive, in After Effects' order.
pub const MAP_TARGETS: [&str; 27] = [
    "None",
    "Red",
    "Green",
    "Blue",
    "Kinetic Friction",
    "Static Friction",
    "Angle",
    "Angular Velocity",
    "Torque",
    "Scale",
    "X Scale",
    "Y Scale",
    "X",
    "Y",
    "Gradient Velocity",
    "X Speed",
    "Y Speed",
    "Gradient Force",
    "X Force",
    "Y Force",
    "Opacity",
    "Mass",
    "Lifespan",
    "Character",
    "Font Size",
    "Time Offset",
    "Scale Speed",
];

/// Ephemeral Property Mapper operators.
pub const MAP_OPERATORS: [&str; 7] = ["Set", "Add", "Difference", "Subtract", "Multiply", "Min", "Max"];

/// Affects ▸ Particles From options.
pub const PARTICLES_FROM: [&str; 8] =
    ["All", "Cannon", "Grid", "Layer Exploder", "Particle Exploder", "Grid & Layer Exploder", "Cannon & Grid", "Cannon & Particle Exploder"];

/// Layer Map ▸ Time Offset Type options.
pub const TIME_OFFSET_TYPES: [&str; 4] = ["Relative", "Absolute", "Relative Random", "Absolute Random"];

/// A property's current value (`target` in [`MAP_TARGETS`]).
fn prop_get(q: &PgParticle, target: u32) -> f32 {
    match target {
        1 => q.c[0],
        2 => q.c[1],
        3 => q.c[2],
        4 => q.friction,
        5 => q.static_friction,
        6 => q.angle,
        7 => q.ang_vel,
        8 => q.torque,
        9 => (q.scale[0] + q.scale[1]) * 0.5,
        10 => q.scale[0],
        11 => q.scale[1],
        12 => q.p[0],
        13 => q.p[1],
        15 => q.v[0],
        16 => q.v[1],
        18 => q.force[0],
        19 => q.force[1],
        20 => q.c[3],
        21 => q.mass,
        22 => q.lifespan,
        23 => q.ch as f32,
        24 => q.r,
        25 => q.toff,
        26 => q.scale_speed,
        _ => 0.0,
    }
}

fn prop_set(q: &mut PgParticle, target: u32, v: f32) {
    match target {
        1 => q.c[0] = v,
        2 => q.c[1] = v,
        3 => q.c[2] = v,
        4 => q.friction = v.clamp(0.0, 10.0),
        5 => q.static_friction = v.max(0.0),
        6 => q.angle = v,
        7 => q.ang_vel = v,
        8 => q.torque = v,
        9 => q.scale = [v.max(0.0); 2],
        10 => q.scale[0] = v.max(0.0),
        11 => q.scale[1] = v.max(0.0),
        12 => q.p[0] = v,
        13 => q.p[1] = v,
        15 => q.v[0] = v,
        16 => q.v[1] = v,
        18 => q.force[0] = v,
        19 => q.force[1] = v,
        20 => q.c[3] = v.clamp(0.0, 1.0),
        21 => q.mass = v.max(0.01),
        22 => q.lifespan = v.max(0.0),
        23 => q.ch = v.round().clamp(0.0, 0x10ffff as f32) as u32,
        24 => q.r = v.max(0.0),
        25 => q.toff = v,
        26 => q.scale_speed = v,
        _ => {}
    }
}

/// An Affects group: which particles a producer, force or mapper acts on, as a weight 0..1.
#[derive(Default)]
struct Affects {
    from: u32,
    selection: Option<MapImg>,
    chars: Vec<char>,
    /// Positive: older than; negative: younger than (seconds); 0 = any age.
    age: f32,
    feather: f32,
}

impl Affects {
    fn from(ctx: &EffectCtx, group: &str) -> Affects {
        let pr = ctx.params;
        let g = |id: &str| format!("{group}/affects/{id}");
        Affects {
            from: pr.e(&g("particlesFrom")),
            selection: ctx.layer_param(&g("selectionMap"), true).map(|lp| MapImg::from(ctx, lp)),
            chars: pr.s(&g("characters")).chars().collect(),
            age: pr.f(&g("olderYoungerThan")) as f32,
            feather: pr.f(&g("ageFeather")).max(0.0) as f32,
        }
    }
    fn weight(&self, q: &PgParticle) -> f32 {
        let src_ok = match self.from {
            1 => q.src == SRC_CANNON,
            2 => q.src == SRC_GRID,
            3 => q.src == SRC_LAYER_EXPLODER,
            4 => q.src == SRC_PARTICLE_EXPLODER,
            5 => q.src == SRC_GRID || q.src == SRC_LAYER_EXPLODER,
            6 => q.src == SRC_CANNON || q.src == SRC_GRID,
            7 => q.src == SRC_CANNON || q.src == SRC_PARTICLE_EXPLODER,
            _ => true,
        };
        if !src_ok {
            return 0.0;
        }
        if !self.chars.is_empty() && !char::from_u32(q.ch).is_some_and(|c| self.chars.contains(&c)) {
            return 0.0;
        }
        let mut w = 1.0;
        if self.age != 0.0 {
            // Signed distance past the age threshold, feathered across Age Feather.
            let d = if self.age > 0.0 { q.age - self.age } else { -self.age - q.age };
            w *= if self.feather > 0.0 {
                (d / self.feather + 0.5).clamp(0.0, 1.0)
            } else if d >= 0.0 {
                1.0
            } else {
                0.0
            };
        }
        if let Some(m) = &self.selection {
            let c = m.at(q.p[0], q.p[1]);
            w *= effectcraft_color::luminance(c[0], c[1], c[2]).clamp(0.0, 1.0) * c[3];
        }
        w
    }
}

/// A Persistent or Ephemeral Property Mapper.
struct Mapper {
    map: MapImg,
    /// Per channel (R, G, B): target, operator, min, max.
    chans: [(u32, u32, f32, f32); 3],
    affects: Affects,
}

impl Mapper {
    fn apply(&self, q: &mut PgParticle) {
        let w = self.affects.weight(q);
        if w <= 0.0 {
            return;
        }
        let px = self.map.at(q.p[0], q.p[1]);
        for (ch, &(target, op, lo, hi)) in self.chans.iter().enumerate() {
            if target == 0 {
                continue;
            }
            let v = lo + (hi - lo) * px[ch];
            if target == 14 || target == 17 {
                // Gradient Velocity / Force: along the map channel's gradient.
                let gx = self.map.at(q.p[0] + 1.0, q.p[1])[ch] - self.map.at(q.p[0] - 1.0, q.p[1])[ch];
                let gy = self.map.at(q.p[0], q.p[1] + 1.0)[ch] - self.map.at(q.p[0], q.p[1] - 1.0)[ch];
                let (gx, gy) = (gx * 0.5 * (hi - lo) * w, gy * 0.5 * (hi - lo) * w);
                if target == 14 {
                    q.v = [q.v[0] + gx, q.v[1] + gy];
                } else {
                    q.force = [q.force[0] + gx, q.force[1] + gy];
                }
                continue;
            }
            let cur = prop_get(q, target);
            let new = match op {
                1 => cur + v,
                2 => (cur - v).abs(),
                3 => cur - v,
                4 => cur * v,
                5 => cur.min(v),
                6 => cur.max(v),
                _ => v,
            };
            // (An unlimited lifespan has no in-between.)
            let v = if w >= 1.0 || !cur.is_finite() { new } else { cur + (new - cur) * w };
            prop_set(q, target, v);
        }
    }
}

struct Pg {
    // Cannon
    cannon: bool,
    pos: [f32; 2],
    barrel_angle: f32,
    barrel_radius: f32,
    rate: f64,
    dir_spread: f32,
    vel: f32,
    vel_spread: f32,
    color: [f32; 4],
    radius: f32,
    /// Edit Cannon Text: characters fired in order (empty = dots).
    cannon_text: Vec<char>,
    // Particle Exploder
    pex_radius: f32,
    pex_dispersion: f32,
    pex_affects: Affects,
    // Gravity
    gravity: [f32; 2],
    grav_spread: f32,
    grav_affects: Affects,
    // Repel
    repel: f32,
    repel_radius: f32,
    repel_affects: Affects,
    // Wall
    wall: Option<Vec<[f64; 2]>>,
    wall_affects: Affects,
    // Property mappers
    persistent: Option<Mapper>,
    ephemeral: Option<Mapper>,
    seed: u32,
    bounds: [f32; 4],
}

impl Pg {
    fn spawn(&self, id: u32) -> PgParticle {
        let s = self.seed;
        let h = |k: u32| hash1(id, k, s) * 2.0 - 1.0;
        // AE angles: 0° = up, clockwise.
        let a = (self.barrel_angle + h(1) * self.dir_spread * 0.5).to_radians();
        let d = [a.sin(), -a.cos()];
        let speed = (self.vel + h(3) * self.vel_spread).max(0.0);
        // Barrel: positive radii are a square around the cannon position, negative ones a disc.
        let r = self.barrel_radius;
        let off = if r >= 0.0 {
            [h(2) * r, h(4) * r]
        } else {
            let (a, rr) = (hash1(id, 5, s) * std::f32::consts::TAU, hash1(id, 6, s).sqrt() * -r);
            [a.cos() * rr, a.sin() * rr]
        };
        let p = [self.pos[0] + off[0], self.pos[1] + off[1]];
        let mut q = PgParticle::new(p, [d[0] * speed, d[1] * speed], self.color, self.radius, id, SRC_CANNON);
        if !self.cannon_text.is_empty() {
            q.ch = self.cannon_text[id as usize % self.cannon_text.len()] as u32;
        }
        q
    }

    /// The particle as the ephemeral mapper sees it this step (unchanged without one).
    fn ephemeral(&self, q: &PgParticle) -> PgParticle {
        let mut e = *q;
        if let Some(m) = &self.ephemeral {
            m.apply(&mut e);
        }
        e
    }

    fn step(&self, st: &mut PgState) {
        let dt = (1.0 / SPS) as f32;
        let s = self.seed;
        // Persistent Property Mapper: the mapped values stay with the particle.
        if let Some(m) = &self.persistent {
            st.parts.par_iter_mut().for_each(|q| m.apply(q));
        }
        // Ephemeral Property Mapper: the mapped values hold for this step only.
        let eph: Vec<PgParticle> = st.parts.par_iter().map(|q| self.ephemeral(q)).collect();
        // Repel: spatial hash, symmetric pairwise push within the radius.
        let mut push = vec![[0.0f32; 2]; st.parts.len()];
        if self.repel != 0.0 && self.repel_radius > 0.0 && eph.len() > 1 {
            let cell = self.repel_radius;
            let mut grid: std::collections::HashMap<(i32, i32), Vec<usize>> = std::collections::HashMap::new();
            for (i, q) in eph.iter().enumerate() {
                grid.entry(((q.p[0] / cell).floor() as i32, (q.p[1] / cell).floor() as i32)).or_default().push(i);
            }
            let parts = &eph;
            push.par_iter_mut().enumerate().for_each(|(i, f)| {
                let q = parts[i];
                let w = self.repel_affects.weight(&q);
                if w <= 0.0 {
                    return;
                }
                let (cx, cy) = ((q.p[0] / cell).floor() as i32, (q.p[1] / cell).floor() as i32);
                for gy in cy - 1..=cy + 1 {
                    for gx in cx - 1..=cx + 1 {
                        let Some(list) = grid.get(&(gx, gy)) else { continue };
                        for &j in list {
                            if j == i {
                                continue;
                            }
                            let o = parts[j];
                            let (dx, dy) = (q.p[0] - o.p[0], q.p[1] - o.p[1]);
                            let d = (dx * dx + dy * dy).sqrt();
                            if d < self.repel_radius && d > 1e-4 {
                                let k = w * self.repel * (1.0 - d / self.repel_radius) / d;
                                f[0] += dx * k;
                                f[1] += dy * k;
                            }
                        }
                    }
                }
            });
        }
        let wall = self.wall.as_deref();
        st.parts.par_iter_mut().zip(eph.par_iter()).zip(push.par_iter()).for_each(|((q, e), f)| {
            let gw = self.grav_affects.weight(e);
            let g = if self.grav_spread > 0.0 { 1.0 + (hash1(q.id, 7, s) * 2.0 - 1.0) * self.grav_spread } else { 1.0 };
            let ax = self.gravity[0] * g * gw + (e.force[0] + f[0]) / e.mass;
            let ay = self.gravity[1] * g * gw + (e.force[1] + f[1]) / e.mass;
            // Static friction: a resting particle stays until the push beats it.
            let resting = e.v[0].abs() < 1e-3 && e.v[1].abs() < 1e-3;
            let (ax, ay) = if resting && (ax * ax + ay * ay).sqrt() <= e.static_friction { (0.0, 0.0) } else { (ax, ay) };
            let damp = (1.0 - e.friction * 0.1).max(0.0);
            let nv = [(e.v[0] + ax * dt) * damp, (e.v[1] + ay * dt) * damp];
            // Keep only the persistent part of the velocity.
            q.v = [nv[0] - (e.v[0] - q.v[0]), nv[1] - (e.v[1] - q.v[1])];
            q.ang_vel += e.torque * dt;
            q.angle += e.ang_vel * dt;
            q.scale = [(q.scale[0] + e.scale_speed * dt).max(0.0), (q.scale[1] + e.scale_speed * dt).max(0.0)];
            q.age += dt;
            let np = [q.p[0] + nv[0] * dt, q.p[1] + nv[1] * dt];
            if let Some(w) = wall
                && self.wall_affects.weight(e) > 0.5
            {
                let was = point_in_poly(w, q.p[0] as f64, q.p[1] as f64);
                let now = point_in_poly(w, np[0] as f64, np[1] as f64);
                if was != now {
                    // Bounce: reverse the velocity and stay on the same side.
                    q.v = [-q.v[0] * 0.9, -q.v[1] * 0.9];
                    return;
                }
            }
            q.p = np;
        });
        let b = self.bounds;
        let eph_life: Vec<f32> = st.parts.iter().map(|q| self.ephemeral(q).lifespan).collect();
        let mut k = 0;
        st.parts.retain(|q| {
            let life = eph_life[k];
            k += 1;
            q.p[0] > b[0] && q.p[0] < b[2] && q.p[1] > b[1] && q.p[1] < b[3] && q.age <= life
        });
        // Particle Exploder: affected particles larger than the new radius burst into smaller
        // ones that fly apart (each new particle bursts no further).
        if self.pex_radius > 0.0 {
            let mut out = Vec::with_capacity(st.parts.len());
            for q in std::mem::take(&mut st.parts) {
                if q.src == SRC_PARTICLE_EXPLODER || q.ch != 0 || q.r <= self.pex_radius * 1.01 || self.pex_affects.weight(&q) <= 0.5 || out.len() > 60_000 {
                    out.push(q);
                    continue;
                }
                let n = ((q.r / self.pex_radius).powi(2).round() as u32).clamp(2, 16);
                for k in 0..n {
                    let id = st.next_id;
                    st.next_id = st.next_id.wrapping_add(1);
                    let a = (k as f32 + hash1(id, 31, s)) / n as f32 * std::f32::consts::TAU;
                    let (sa, ca) = a.sin_cos();
                    let rr = q.r - self.pex_radius;
                    let sp = self.pex_dispersion * (0.5 + hash1(id, 32, s));
                    let mut c = q;
                    c.p = [q.p[0] + ca * rr, q.p[1] + sa * rr];
                    c.v = [q.v[0] + ca * sp, q.v[1] + sa * sp];
                    c.r = self.pex_radius;
                    c.src = SRC_PARTICLE_EXPLODER;
                    c.id = id;
                    out.push(c);
                }
            }
            st.parts = out;
        }
        if self.cannon {
            st.carry += self.rate / SPS;
            let n = st.carry.floor() as u32;
            st.carry -= n as f64;
            for _ in 0..n {
                if st.parts.len() >= 60_000 {
                    break;
                }
                let id = st.next_id;
                st.next_id = st.next_id.wrapping_add(1);
                let mut q = self.spawn(id);
                // Sub-step birth offset keeps streams smooth.
                let frac = hash1(id, 9, s) * dt;
                q.p = [q.p[0] + q.v[0] * frac, q.p[1] + q.v[1] * frac];
                st.parts.push(q);
            }
        }
    }
}

static PG_CACHE: SimCache<PgState> = SimCache::new(6);

fn image_key(img: &Image) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    img.width.hash(&mut h);
    img.height.hash(&mut h);
    for p in img.data.iter().step_by(7) {
        for c in p {
            c.to_bits().hash(&mut h);
        }
    }
    h.finish()
}

fn mapper_from(ctx: &EffectCtx, group: &str, ephemeral: bool) -> Option<Mapper> {
    let pr = ctx.params;
    let map = ctx.layer_param(&format!("{group}/useLayerAsMap"), true).map(|lp| MapImg::from(ctx, lp))?;
    let ch = |c: &str| {
        (
            pr.e(&format!("{group}/map{c}To")),
            if ephemeral { pr.e(&format!("{group}/{}Operator", c.to_lowercase())) } else { 0 },
            pr.f(&format!("{group}/{}Min", c.to_lowercase())) as f32,
            pr.f(&format!("{group}/{}Max", c.to_lowercase())) as f32,
        )
    };
    Some(Mapper { map, chans: [ch("Red"), ch("Green"), ch("Blue")], affects: Affects::from(ctx, group) })
}

/// Draw a map layer frame as a particle: centred on (x, y) buffer px, rotated `angle` degrees
/// and scaled, over `out`.
fn blit_layer(out: &mut Image, lp: &crate::LayerPixels, position: [f64; 2], angle: f64, scale: [f64; 2], buf_scale: f64, alpha: f32) {
    let [x, y] = position;
    let (w, h) = (lp.size[0] * scale[0].abs() * buf_scale, lp.size[1] * scale[1].abs() * buf_scale);
    let r = (w * w + h * h).sqrt() * 0.5 + 1.0;
    let (x0, x1) = ((x - r).floor().max(0.0) as i64, (x + r).ceil().min(out.width as f64) as i64);
    let (y0, y1) = ((y - r).floor().max(0.0) as i64, (y + r).ceil().min(out.height as f64) as i64);
    if x0 >= x1 || y0 >= y1 || scale[0] == 0.0 || scale[1] == 0.0 {
        return;
    }
    let (sa, ca) = angle.to_radians().sin_cos();
    for py in y0..y1 {
        for px in x0..x1 {
            let (dx, dy) = ((px as f64 + 0.5 - x) / buf_scale, (py as f64 + 0.5 - y) / buf_scale);
            // Undo rotation, then scale; into map layer pixels around its centre.
            let (u, v) = ((dx * ca + dy * sa) / scale[0] + lp.size[0] * 0.5, (-dx * sa + dy * ca) / scale[1] + lp.size[1] * 0.5);
            if u < 0.0 || v < 0.0 || u >= lp.size[0] || v >= lp.size[1] {
                continue;
            }
            let s = lp.buf.img.sample_bilinear(u * lp.buf.scale + lp.buf.offset[0], v * lp.buf.scale + lp.buf.offset[1]);
            if s[3] <= 0.0 {
                continue;
            }
            let d = out.get(px, py);
            let k = 1.0 - s[3] * alpha;
            out.set(px as u32, py as u32, [s[0] * alpha + d[0] * k, s[1] * alpha + d[1] * k, s[2] * alpha + d[2] * k, s[3] * alpha + d[3] * k]);
        }
    }
}

/// Particle Playground's producers and forces.
fn pg_of(ctx: &EffectCtx) -> Pg {
    let pr = ctx.params;
    let (lw, lh) = (ctx.layer_size[0] as f32, ctx.layer_size[1] as f32);
    let grav_dir = (pr.f("gravity/gravityDirection") as f32).to_radians();
    let gf = pr.f("gravity/gravityForce") as f32;
    let wall = (pr.f("wall/wallBoundary").round() as usize).checked_sub(1).and_then(|i| ctx.env.masks.get(i)).map(|m| m.points.clone());
    let persistent = if pr.b("mapperEnabled") { mapper_from(ctx, "persistentPropertyMapper", false) } else { None };
    let ephemeral = mapper_from(ctx, "ephemeralPropertyMapper", true);
    Pg {
        cannon: pr.b("cannonEnabled"),
        pos: { pr.v2("cannon/cannonPosition").map(|v| v as f32) },
        barrel_angle: pr.f("cannon/barrelAngle") as f32,
        barrel_radius: pr.f("cannon/barrelRadius") as f32,
        rate: pr.f("cannon/particlesPerSecond").max(0.0),
        dir_spread: pr.f("cannon/directionRandomSpread") as f32,
        vel: pr.f("cannon/velocity") as f32,
        vel_spread: pr.f("cannon/velocityRandomSpread") as f32,
        color: pr.color("cannon/cannonColor"),
        radius: pr.f("cannon/cannonParticleRadius").max(0.0) as f32,
        cannon_text: pr.s("options/cannonText").chars().collect(),
        pex_radius: pr.f("particleExploder/radiusOfNewParticles").max(0.0) as f32,
        pex_dispersion: pr.f("particleExploder/velocityDispersion") as f32,
        pex_affects: Affects::from(ctx, "particleExploder"),
        gravity: [grav_dir.sin() * gf, -grav_dir.cos() * gf],
        grav_spread: (pr.f("gravity/gravityForceRandomSpread") / 100.0).max(0.0) as f32,
        grav_affects: Affects::from(ctx, "gravity"),
        repel: pr.f("repel/repelForce") as f32 * 100.0,
        repel_radius: pr.f("repel/repelForceRadius").max(0.0) as f32,
        repel_affects: Affects::from(ctx, "repel"),
        wall,
        wall_affects: Affects::from(ctx, "wall"),
        persistent,
        ephemeral,
        seed: (pr.f("randomSeed") as u32).wrapping_mul(0x9e37_79b9) ^ ctx.seed,
        bounds: [-lw * 2.0, -lh * 2.0, lw * 3.0, lh * 3.0],
    }
}

impl Pg {
    /// Only a cannon with gravity: every particle moves on its own (what the host's particle
    /// backend can simulate).
    fn cannon_only(&self) -> bool {
        self.cannon
            && !(self.repel != 0.0 && self.repel_radius > 0.0)
            && self.wall.is_none()
            && self.persistent.is_none()
            && self.ephemeral.is_none()
            && self.pex_radius <= 0.0
            && self.grav_affects.is_all()
    }
}

impl Affects {
    /// Affects everything (the defaults).
    fn is_all(&self) -> bool {
        self.from == 0 && self.selection.is_none() && self.chars.is_empty() && self.age == 0.0
    }
}

/// Particle Playground cannons with gravity only, simulated by the host's particle backend.
fn pg_accelerated(ctx: &EffectCtx, key: u64, pg: &Pg) -> Option<std::sync::Arc<PgState>> {
    let backend = ctx.env.host?.particles()?;
    let steps = steps_at(ctx.time);
    let births = crate::psim::births(pg.rate, steps, 60_000)?;
    let desc = crate::psim::CannonDesc {
        pos: pg.pos,
        barrel_angle: pg.barrel_angle,
        barrel_radius: pg.barrel_radius,
        dir_spread: pg.dir_spread,
        vel: pg.vel,
        vel_spread: pg.vel_spread,
        gravity: pg.gravity,
        grav_spread: pg.grav_spread,
        seed: pg.seed,
        bounds: pg.bounds,
    };
    let req = crate::psim::SimRequest { key, steps, births: &births, system: crate::psim::ParticleSystem::Cannon(desc) };
    let parts = backend.simulate(&req)?;
    Some(std::sync::Arc::new(PgState {
        parts: parts
            .into_iter()
            .map(|q| {
                let born = pg.spawn(q.id);
                PgParticle { p: [q.p[0], q.p[1]], v: [q.v[0], q.v[1]], ..born }
            })
            .collect(),
        carry: 0.0,
        next_id: births.len() as u32,
    }))
}

/// The live particles of a Particle Playground whose cannon is the only producer and force
/// besides gravity, at `ctx.time`: from the host's particle backend when it has one (GPU
/// particles), else from the CPU simulation (`None` for other set-ups). For comparing the two.
pub fn playground_state(ctx: &EffectCtx) -> Option<Vec<crate::psim::SimParticle>> {
    let pg = pg_of(ctx);
    let pr = ctx.params;
    let grid = pr.b("gridEnabled") && pr.f("grid/particlesAcross").round() > 0.0 && pr.f("grid/particlesDown").round() > 0.0;
    let exploder = pr.b("exploderEnabled") && pr.get("layerExploder/explodeLayer").and_then(Value::as_layer).is_some();
    if !pg.cannon_only() || grid || exploder {
        return None;
    }
    let key = params_key(ctx, &Buf { img: Image::new(0, 0), offset: [0.0; 2], scale: 1.0 }, 0x7067);
    let st = pg_accelerated(ctx, key, &pg).unwrap_or_else(|| PG_CACHE.run(key, steps_at(ctx.time), PgState::default, |st, _| pg.step(st)));
    Some(
        st.parts
            .iter()
            .map(|q| crate::psim::SimParticle { p: [q.p[0], q.p[1], 0.0], v: [q.v[0], q.v[1], 0.0], age: 0.0, life: 0.0, rnd: 0.0, id: q.id })
            .collect(),
    )
}

fn particle_playground(ctx: &EffectCtx, b: Buf) -> Buf {
    playground_plan(ctx, &b).finish(b)
}

/// Particle Playground's [`PgPlan`] for buffer geometry `b` (its pixels are not read).
pub fn playground_plan(ctx: &EffectCtx, b: &Buf) -> PgPlan {
    let pr = ctx.params;
    let (lw, lh) = (ctx.layer_size[0] as f32, ctx.layer_size[1] as f32);
    let pg = pg_of(ctx);
    // Initial particles: Grid and Layer Exploder.
    let grid_on = pr.b("gridEnabled");
    let across = pr.f("grid/particlesAcross").max(0.0).round() as u32;
    let down = pr.f("grid/particlesDown").max(0.0).round() as u32;
    let gpos = pr.v2("grid/gridPosition");
    let (gw, gh) = (pr.f("grid/gridWidth"), pr.f("grid/gridHeight"));
    let gcol = pr.color("grid/gridColor");
    let grad = pr.f("grid/gridParticleRadius").max(0.0) as f32;
    let grid_text: Vec<char> = pr.s("options/gridText").chars().collect();
    let exploder = if pr.b("exploderEnabled") {
        let host = ctx.env.host;
        let lid = pr.get("layerExploder/explodeLayer").and_then(Value::as_layer);
        match (host, lid) {
            (Some(h), Some(id)) => h.layer_at(id, ctx.env.comp_time - ctx.time, true).or_else(|| h.layer(id, true)),
            _ => None,
        }
    } else {
        None
    };
    let ex_r = pr.f("layerExploder/radiusOfNewParticles").max(0.5) as f32;
    let ex_disp = pr.f("layerExploder/velocityDispersion") as f32;

    let mut key = params_key(ctx, &Buf { img: Image::new(0, 0), offset: [0.0; 2], scale: 1.0 }, 0x7067);
    for m in [&pg.persistent, &pg.ephemeral].into_iter().flatten() {
        key ^= image_key(&m.map.img).rotate_left(7);
    }
    for a in [&pg.pex_affects, &pg.grav_affects, &pg.repel_affects, &pg.wall_affects] {
        if let Some(s) = &a.selection {
            key ^= image_key(&s.img).rotate_left(11);
        }
    }
    if let Some(e) = &exploder {
        key ^= image_key(&e.buf.img).rotate_left(13);
    }
    if let Some(w) = &pg.wall {
        key ^= w.iter().fold(0u64, |a, p| a.rotate_left(5) ^ p[0].to_bits() ^ p[1].to_bits().rotate_left(17));
    }
    let seed = pg.seed;
    let init = || {
        let mut st = PgState::default();
        if grid_on && across > 0 && down > 0 {
            let mut k = 0usize;
            for j in 0..down {
                for i in 0..across {
                    // Edit Grid Text: one character per grid point in reading order; spaces
                    // leave the point empty.
                    let ch = if grid_text.is_empty() {
                        0
                    } else {
                        let c = grid_text[k % grid_text.len()];
                        k += 1;
                        if c == ' ' {
                            continue;
                        }
                        c as u32
                    };
                    let fx = if across > 1 { i as f64 / (across - 1) as f64 - 0.5 } else { 0.0 };
                    let fy = if down > 1 { j as f64 / (down - 1) as f64 - 0.5 } else { 0.0 };
                    let p = [(gpos[0] + fx * gw) as f32, (gpos[1] + fy * gh) as f32];
                    let id = st.next_id;
                    st.next_id += 1;
                    let mut q = PgParticle::new(p, [0.0; 2], gcol, grad, id, SRC_GRID);
                    q.ch = ch;
                    st.parts.push(q);
                }
            }
        }
        if let Some(e) = &exploder {
            let m = MapImg::from(ctx, e.clone());
            let step = (ex_r * 2.0).max(1.0);
            let mut y = step * 0.5;
            while y < lh {
                let mut x = step * 0.5;
                while x < lw {
                    let c = m.at(x, y);
                    if c[3] > 0.05 {
                        let id = st.next_id;
                        st.next_id += 1;
                        let h = |k: u32| hash1(id, k, seed) * 2.0 - 1.0;
                        let (dx, dy) = (x - lw * 0.5, y - lh * 0.5);
                        let l = (dx * dx + dy * dy).sqrt().max(1.0);
                        let v = [dx / l * ex_disp + h(1) * ex_disp, dy / l * ex_disp + h(2) * ex_disp];
                        st.parts.push(PgParticle::new([x, y], v, c, ex_r, id, SRC_LAYER_EXPLODER));
                    }
                    x += step;
                }
                y += step;
            }
        }
        st
    };
    // A cannon with gravity only: every particle moves on its own (GPU particles).
    let independent = pg.cannon_only() && !(grid_on && across > 0 && down > 0) && exploder.is_none();
    let gpu = if independent { pg_accelerated(ctx, key, &pg) } else { None };
    let st = gpu.unwrap_or_else(|| PG_CACHE.run(key, steps_at(ctx.time), init, |st, _| pg.step(st)));
    // Render: what the ephemeral mapper changes shows for this frame only.
    let parts: Vec<PgParticle> = st.parts.iter().map(|q| pg.ephemeral(q)).collect();
    let auto_orient = pr.b("options/autoOrientRotation");
    let s = b.scale as f32;
    // Layer Map: affected particles show the map layer (at a time set by the offset type).
    let map_id = if pr.b("layerMapEnabled") { pr.get("layerMap/layerMapLayer").and_then(Value::as_layer) } else { None };
    let map_affects = Affects::from(ctx, "layerMap");
    let offset_type = pr.e("layerMap/timeOffsetType");
    let offset = pr.f("layerMap/timeOffset");
    let fps = ctx.fps();
    let mut sprites: Vec<Sprite> = vec![];
    let mut blits: Vec<(PgParticle, i64)> = vec![];
    for q in &parts {
        let (x, y) = b.to_px([q.p[0] as f64, q.p[1] as f64]);
        let angle = if auto_orient && (q.v[0] != 0.0 || q.v[1] != 0.0) { q.v[1].atan2(q.v[0]).to_degrees() + 90.0 } else { q.angle };
        if let (Some(_), true) = (map_id, map_affects.weight(q) > 0.5) {
            let rnd = hash1(q.id, 41, seed) as f64;
            let t = match offset_type {
                1 => offset,
                2 => ctx.env.comp_time + offset * rnd,
                3 => offset * rnd,
                _ => ctx.env.comp_time + offset,
            } + q.toff as f64;
            let frame = (t * fps).round() as i64;
            blits.push((PgParticle { angle, ..*q }, frame));
            continue;
        }
        let sc = (q.scale[0] + q.scale[1]) * 0.5;
        match char::from_u32(q.ch).filter(|c| q.ch != 0 && *c != ' ') {
            Some(c) => {
                // A text particle: the character's strokes, Font Size tall, rotated.
                let size = (q.r * sc * s) as f64;
                let u = size * 0.7 / 6.0;
                let wgt = (size * 0.045).max(0.35) as f32;
                let (sa, ca) = (angle as f64).to_radians().sin_cos();
                let tr = |g: [f64; 2]| {
                    let (gx, gy) = ((g[0] - 2.0) * u * q.scale[0] as f64 / sc.max(1e-6) as f64, (g[1] - 3.0) * u * q.scale[1] as f64 / sc.max(1e-6) as f64);
                    [x + gx * ca - gy * sa, y + gx * sa + gy * ca]
                };
                for stroke in crate::textfx::glyph_strokes(c) {
                    if stroke.len() == 1 {
                        let p0 = tr(stroke[0]);
                        sprites.push(Sprite::new(p0[0] as f32, p0[1] as f32, wgt, q.c, Shape::Disc));
                    }
                    for w2 in stroke.windows(2) {
                        let (a0, a1) = (tr(w2[0]), tr(w2[1]));
                        sprites.push(Sprite::new(a0[0] as f32, a0[1] as f32, wgt, q.c, Shape::Line { dx: (a1[0] - a0[0]) as f32, dy: (a1[1] - a0[1]) as f32 }));
                    }
                }
            }
            None => sprites.push(Sprite::new(x as f32, y as f32, q.r * sc * s, q.c, Shape::Disc)),
        }
    }
    let mut plan = PgPlan { sprites: SpritePlan { sprites, tints: vec![], acc: Acc::Over, post: Post::Combine(5) }, blits: vec![], frames: vec![] };
    if let (Some(id), Some(host)) = (map_id, ctx.env.host) {
        let me = pr.get(&crate::layer_source_id("layerMap/layerMapLayer")).is_none_or(|v| v.as_enum() != 0);
        // Fetched frames by frame number: their index in `plan.frames` (`None` = unavailable).
        let mut frames: std::collections::HashMap<i64, Option<usize>> = std::collections::HashMap::new();
        for (q, frame) in &blits {
            let at = if !frames.contains_key(frame) && frames.len() >= 32 {
                // Too many distinct times: reuse the nearest fetched frame.
                match frames.keys().min_by_key(|k| (*k - frame).abs()).copied() {
                    Some(k) => frames.get(&k).copied().flatten(),
                    None => None,
                }
            } else {
                *frames.entry(*frame).or_insert_with(|| {
                    let lp = host.layer_at(id, *frame as f64 / fps, me)?;
                    plan.frames.push(lp);
                    Some(plan.frames.len() - 1)
                })
            };
            if let Some(i) = at {
                let (x, y) = b.to_px([q.p[0] as f64, q.p[1] as f64]);
                plan.blits.push(PgBlit { frame: i, x, y, angle: q.angle as f64, scale: [q.scale[0] as f64, q.scale[1] as f64], alpha: q.c[3] });
            }
        }
    }
    plan
}

/// A Layer Map particle: frame `frame` of [`PgPlan::frames`] centred on (`x`, `y`) buffer px,
/// rotated `angle` degrees, scaled and faded to `alpha`.
#[derive(Clone, Copy, Debug)]
pub struct PgBlit {
    pub frame: usize,
    pub x: f64,
    pub y: f64,
    pub angle: f64,
    pub scale: [f64; 2],
    pub alpha: f32,
}

/// What Particle Playground draws this frame: the dot and text particles as sprites, then the
/// Layer Map particles over them in order; the result replaces the layer's own pixels.
pub struct PgPlan {
    pub sprites: SpritePlan,
    pub blits: Vec<PgBlit>,
    pub frames: Vec<crate::LayerPixels>,
}

impl PgPlan {
    pub fn finish(&self, mut b: Buf) -> Buf {
        let mut fx = splat(b.img.width, b.img.height, &self.sprites.sprites, self.sprites.acc);
        for q in &self.blits {
            blit_layer(&mut fx, &self.frames[q.frame], [q.x, q.y], q.angle, q.scale, b.scale, q.alpha);
        }
        // Particles replace the layer's own pixels.
        b.img = combine(&b.img, &fx, 5);
        b
    }
}

// ---------------------------------------------------------------- CC Hair

fn cc_hair(ctx: &EffectCtx, b: Buf) -> Buf {
    let Some(mut plan) = hair_plan(ctx, &b) else { return b };
    plan.resolve(&b.img);
    plan.finish(b)
}

/// CC Hair's strands as line sprites (each coloured from its root, see [`Tint::Root`]);
/// `None` = no hair (the layer passes through).
pub(crate) fn hair_plan(ctx: &EffectCtx, b: &Buf) -> Option<SpritePlan> {
    let pr = ctx.params;
    let len = pr.f("length").max(0.0) as f32;
    let density = pr.f("density").max(0.0) as f32 / 100.0;
    if len <= 0.0 || density <= 0.0 {
        return None;
    }
    let thick = pr.f("thickness").max(0.05) as f32;
    let weight = pr.f("weight") as f32;
    let const_mass = pr.b("constantMass");
    let map = ctx.layer_param("hairfallMap/mapLayer", true).map(|lp| MapImg::from(ctx, lp));
    let map_strength = (pr.f("hairfallMap/mapStrength") / 100.0) as f32;
    let map_soft = pr.f("hairfallMap/mapSoftness").max(0.0) as f32;
    let noise = (pr.f("hairfallMap/addNoise") / 100.0) as f32;
    let hc = pr.color("hairColor/hairColor");
    let bright = (pr.f("hairColor/brightness") / 100.0) as f32;
    let opacity = (pr.f("hairColor/opacity") / 100.0) as f32;
    let inherit = (pr.f("hairColor/colorInheritance") / 100.0) as f32;
    let light_i = (pr.f("light/lightIntensity") / 100.0) as f32;
    let ld = (pr.f("light/lightDirection") as f32).to_radians();
    let light = [ld.sin(), -ld.cos()];
    let (amb, dif, spe) = ((pr.f("shading/ambient") / 100.0) as f32, (pr.f("shading/diffuse") / 100.0) as f32, (pr.f("shading/specular") / 100.0) as f32);
    let rough = (pr.f("shading/roughness") / 100.0).clamp(0.01, 1.0) as f32;
    let seed = ctx.seed ^ (pr.f("randomSeed") as u32).wrapping_mul(0x85eb_ca6b);
    let (lw, lh) = (ctx.layer_size[0] as f32, ctx.layer_size[1] as f32);
    // Roots: a jittered grid over the layer, density per 1 000 px².
    let count = ((lw * lh / 1000.0) * density * 10.0).min(200_000.0) as u32;
    let segs = 6u32;
    let strands: Vec<Vec<(Sprite, Tint)>> = (0..count)
        .into_par_iter()
        .map(|i| {
            let rx = hash1(i, 1, seed) * lw;
            let ry = hash1(i, 2, seed) * lh;
            let (bx, by) = b.to_px([rx as f64, ry as f64]);
            let mut l = len * (0.75 + 0.5 * hash1(i, 3, seed));
            // Lean: random, plus the map's local gradient (hairfall map).
            let mut lean = [(hash1(i, 4, seed) * 2.0 - 1.0) * 0.6, -1.0 + hash1(i, 5, seed) * 0.4];
            if let Some(m) = &map {
                let lum = |x: f32, y: f32| {
                    let c = m.at(x, y);
                    (0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]) * c[3]
                };
                let d = map_soft.max(1.0);
                let (gx, gy) = (lum(rx + d, ry) - lum(rx - d, ry), lum(rx, ry + d) - lum(rx, ry - d));
                l *= 1.0 - map_strength + map_strength * lum(rx, ry);
                lean = [lean[0] + gx * map_strength * 4.0, lean[1] + gy * map_strength * 4.0];
                if noise > 0.0 {
                    lean[0] += (hash1(i, 6, seed) * 2.0 - 1.0) * noise;
                }
            }
            let ll = (lean[0] * lean[0] + lean[1] * lean[1]).sqrt().max(1e-4);
            let dir = [lean[0] / ll, lean[1] / ll];
            // Droop: heavier (or thinner with Constant Mass off) hair bends more.
            let droop = weight * if const_mass { 1.0 } else { 1.0 / thick.max(0.2) } * l;
            let pt = |s: f32| [rx + dir[0] * l * s, ry + dir[1] * l * s + droop * s * s];
            let mut out = Vec::with_capacity(segs as usize);
            for k in 0..segs {
                let (s0, s1) = (k as f32 / segs as f32, (k + 1) as f32 / segs as f32);
                let (a, c) = (pt(s0), pt(s1));
                let t = {
                    let (tx, ty) = (c[0] - a[0], c[1] - a[1]);
                    let tl = (tx * tx + ty * ty).sqrt().max(1e-6);
                    [tx / tl, ty / tl]
                };
                // Strand shading (Kajiya–Kay): diffuse ∝ sin(T, L), specular from T·L.
                let tl = t[0] * light[0] + t[1] * light[1];
                let diff = (1.0 - tl * tl).max(0.0).sqrt();
                let spec = (1.0 - tl.abs()).powf(1.0 / rough) * spe;
                let shade = (amb + dif * diff * light_i) * bright;
                let tip = 1.0 - 0.6 * s1;
                let (pa, pc) = (b.to_px([a[0] as f64, a[1] as f64]), b.to_px([c[0] as f64, c[1] as f64]));
                let sp = Sprite {
                    x: pa.0 as f32,
                    y: pa.1 as f32,
                    r: thick * 0.5 * b.scale as f32 * tip,
                    c: [0.0, 0.0, 0.0, opacity],
                    shape: Shape::Line { dx: (pc.0 - pa.0) as f32, dy: (pc.1 - pa.1) as f32 },
                    rot: 0.0,
                };
                // The root's colour mixed with the hair colour (no strand where it is clear).
                out.push((sp, Tint::Root { x: bx, y: by, hair: [hc[0], hc[1], hc[2]], inherit, shade, spec: spec * light_i }));
            }
            out
        })
        .collect();
    let (sprites, tints) = strands.into_iter().flatten().unzip();
    Some(SpritePlan { sprites, tints, acc: Acc::Over, post: Post::Combine(0) })
}

pub fn specs() -> Vec<EffectSpec> {
    let pt = |x: f64, y: f64| Value::Vec2([x, y]);
    let px = |d: f64, max: f64| (num(d), slider(0.0, max * 10.0, 0.0, max, 1));
    let sp = |id: &'static str, name: &'static str, v: (Value, ParamUi)| p(id, name, v.0, v.1);
    let leak = |s: String| -> &'static str { Box::leak(s.into_boxed_str()) };
    // An Affects twirl-down inside `group`.
    let affects = |group: &str| -> Vec<crate::ParamSpec> {
        let id = |s: &str| leak(format!("{group}/affects/{s}"));
        vec![
            p(id("particlesFrom"), "Particles From", Value::Enum(0), popup(&PARTICLES_FROM)),
            p(id("selectionMap"), "Selection Map", Value::Layer(None), ParamUi::Layer),
            p(id("characters"), "Characters", Value::Str(String::new()), ParamUi::Text),
            p(id("olderYoungerThan"), "Older/Younger than", num(0.0), slider(-1000.0, 1000.0, -10.0, 10.0, 2)),
            p(id("ageFeather"), "Age Feather", num(0.0), slider(0.0, 1000.0, 0.0, 10.0, 2)),
        ]
    };
    let mut pg = vec![
        // Options (the Options dialog): text fired by the cannon / placed on the grid.
        p("options/cannonText", "Cannon Text", Value::Str(String::new()), ParamUi::Text),
        p("options/gridText", "Grid Text", Value::Str(String::new()), ParamUi::Text),
        p("options/autoOrientRotation", "Auto-Orient Rotation", Value::Bool(false), ParamUi::Checkbox),
        // Generator switches (kept for saved projects; After Effects turns a generator off with
        // its rate / counts instead).
        p("cannonEnabled", "Cannon Enabled", Value::Bool(true), ParamUi::Hidden),
        p("gridEnabled", "Grid Enabled", Value::Bool(true), ParamUi::Hidden),
        p("exploderEnabled", "Layer Exploder Enabled", Value::Bool(true), ParamUi::Hidden),
        p("layerMapEnabled", "Layer Map Enabled", Value::Bool(true), ParamUi::Hidden),
        p("mapperEnabled", "Persistent Property Mapper Enabled", Value::Bool(true), ParamUi::Hidden),
        // Cannon
        p("cannon/cannonPosition", "Position", pt(0.5, 0.9), ParamUi::Point),
        p("cannon/barrelRadius", "Barrel Radius", num(0.0), slider(-1000.0, 1000.0, -100.0, 100.0, 1)),
        sp("cannon/particlesPerSecond", "Particles Per Second", px(60.0, 500.0)),
        p("cannon/barrelAngle", "Direction", num(0.0), ParamUi::Angle),
        sp("cannon/directionRandomSpread", "Direction Random Spread", px(20.0, 360.0)),
        sp("cannon/velocity", "Velocity", px(130.0, 1000.0)),
        sp("cannon/velocityRandomSpread", "Velocity Random Spread", px(20.0, 500.0)),
        p("cannon/cannonColor", "Color", col(1.0, 0.0, 0.0), ParamUi::Color),
        sp("cannon/cannonParticleRadius", "Particle Radius", px(2.0, 50.0)),
        // Grid
        p("grid/gridPosition", "Position", pt(0.5, 0.5), ParamUi::Point),
        sp("grid/gridWidth", "Width", px(100.0, 2000.0)),
        sp("grid/gridHeight", "Height", px(100.0, 2000.0)),
        sp("grid/particlesAcross", "Particles Across", px(0.0, 100.0)),
        sp("grid/particlesDown", "Particles Down", px(0.0, 100.0)),
        p("grid/gridColor", "Color", col(1.0, 1.0, 1.0), ParamUi::Color),
        sp("grid/gridParticleRadius", "Particle Radius", px(2.0, 50.0)),
        // Layer Exploder
        p("layerExploder/explodeLayer", "Explode Layer", Value::Layer(None), ParamUi::Layer),
        sp("layerExploder/radiusOfNewParticles", "Radius of New Particles", px(2.0, 50.0)),
        sp("layerExploder/velocityDispersion", "Velocity Dispersion", px(20.0, 500.0)),
    ];
    pg.extend([
        sp("particleExploder/radiusOfNewParticles", "Radius of New Particles", px(0.0, 50.0)),
        sp("particleExploder/velocityDispersion", "Velocity Dispersion", px(20.0, 500.0)),
    ]);
    pg.extend(affects("particleExploder"));
    pg.extend([
        p("layerMap/layerMapLayer", "Use Layer", Value::Layer(None), ParamUi::Layer),
        p("layerMap/timeOffsetType", "Time Offset Type", Value::Enum(0), popup(&TIME_OFFSET_TYPES)),
        p("layerMap/timeOffset", "Time Offset", num(0.0), slider(-1000.0, 1000.0, -10.0, 10.0, 2)),
    ]);
    pg.extend(affects("layerMap"));
    pg.extend([
        // Gravity
        sp("gravity/gravityForce", "Force", px(108.0, 1000.0)),
        sp("gravity/gravityForceRandomSpread", "Force Random Spread", px(0.0, 100.0)),
        p("gravity/gravityDirection", "Direction", num(180.0), ParamUi::Angle),
    ]);
    pg.extend(affects("gravity"));
    pg.extend([
        // Repel
        p("repel/repelForce", "Force", num(0.0), slider(-100.0, 100.0, -10.0, 10.0, 2)),
        sp("repel/repelForceRadius", "Force Radius", px(0.0, 100.0)),
    ]);
    pg.extend(affects("repel"));
    pg.extend([
        // Wall
        p("wall/wallBoundary", "Boundary", num(0.0), ParamUi::Mask),
    ]);
    pg.extend(affects("wall"));
    pg.extend([
        // Persistent Property Mapper
        p("persistentPropertyMapper/useLayerAsMap", "Use Layer As Map", Value::Layer(None), ParamUi::Layer),
    ]);
    for (to, mn, mx, n_to) in [
        ("persistentPropertyMapper/mapRedTo", "persistentPropertyMapper/redMin", "persistentPropertyMapper/redMax", "Map Red To"),
        ("persistentPropertyMapper/mapGreenTo", "persistentPropertyMapper/greenMin", "persistentPropertyMapper/greenMax", "Map Green To"),
        ("persistentPropertyMapper/mapBlueTo", "persistentPropertyMapper/blueMin", "persistentPropertyMapper/blueMax", "Map Blue To"),
    ] {
        pg.push(p(to, n_to, Value::Enum(0), popup(&MAP_TARGETS)));
        pg.push(p(mn, "Min", num(0.0), slider(-10000.0, 10000.0, -100.0, 100.0, 2)));
        pg.push(p(mx, "Max", num(1.0), slider(-10000.0, 10000.0, -100.0, 100.0, 2)));
    }
    pg.extend(affects("persistentPropertyMapper"));
    // Ephemeral Property Mapper: like the persistent one, with an operator per channel; the
    // values hold for one frame.
    pg.push(p("ephemeralPropertyMapper/useLayerAsMap", "Use Layer As Map", Value::Layer(None), ParamUi::Layer));
    for c in ["Red", "Green", "Blue"] {
        let l = c.to_lowercase();
        pg.push(p(leak(format!("ephemeralPropertyMapper/map{c}To")), leak(format!("Map {c} To")), Value::Enum(0), popup(&MAP_TARGETS)));
        pg.push(p(leak(format!("ephemeralPropertyMapper/{l}Operator")), "Operator", Value::Enum(0), popup(&MAP_OPERATORS)));
        pg.push(p(leak(format!("ephemeralPropertyMapper/{l}Min")), "Min", num(0.0), slider(-10000.0, 10000.0, -100.0, 100.0, 2)));
        pg.push(p(leak(format!("ephemeralPropertyMapper/{l}Max")), "Max", num(1.0), slider(-10000.0, 10000.0, -100.0, 100.0, 2)));
    }
    pg.extend(affects("ephemeralPropertyMapper"));
    pg.push(p("randomSeed", "Random Seed", num(0.0), slider(0.0, 10000.0, 0.0, 1000.0, 0)));
    let pc = |d: f64| (num(d), slider(0.0, 100.0, 0.0, 100.0, 1));
    vec![
        spec("ec.sim.particleplayground", "Particle Playground", pg, particle_playground),
        spec(
            "ec.sim.cchair",
            "CC Hair",
            vec![
                sp("length", "Length", px(30.0, 200.0)),
                p("thickness", "Thickness", num(1.0), slider(0.05, 20.0, 0.1, 5.0, 2)),
                p("weight", "Weight", num(0.2), slider(-10.0, 10.0, -2.0, 2.0, 2)),
                p("constantMass", "Constant Mass", Value::Bool(false), ParamUi::Checkbox),
                sp("density", "Density", px(100.0, 1000.0)),
                // Hairfall Map
                sp("hairfallMap/mapStrength", "Map Strength", pc(0.0)),
                p("hairfallMap/mapLayer", "Map Layer", Value::Layer(None), ParamUi::Layer),
                sp("hairfallMap/mapSoftness", "Map Softness", px(0.0, 100.0)),
                sp("hairfallMap/addNoise", "Add Noise", pc(0.0)),
                // Hair Color
                p("hairColor/hairColor", "Color", col(0.35, 0.25, 0.15), ParamUi::Color),
                sp("hairColor/brightness", "Brightness", px(100.0, 400.0)),
                sp("hairColor/opacity", "Opacity", pc(100.0)),
                sp("hairColor/colorInheritance", "Color Inheritance", pc(0.0)),
                // Light
                sp("light/lightIntensity", "Light Intensity", px(100.0, 400.0)),
                p("light/lightDirection", "Light Direction", num(-45.0), ParamUi::Angle),
                // Shading
                sp("shading/ambient", "Ambient", pc(40.0)),
                sp("shading/diffuse", "Diffuse", pc(60.0)),
                sp("shading/specular", "Specular", pc(30.0)),
                sp("shading/roughness", "Roughness", pc(20.0)),
                p("randomSeed", "Random Seed", num(0.0), slider(0.0, 10000.0, 0.0, 1000.0, 0)),
            ],
            cc_hair,
        ),
    ]
}

#[cfg(test)]
mod tests;

use super::engine::{MERCURY, PS2, PState, PW_EFFECT_CAMERA, Phys, norm3, particle_look, pw_state, type_sprite};
use super::sprites::{Acc, Post, Sprite, SpritePlan, aa, raster, splat, sprite_bbox};
use super::{Buf, EffectCtx, Image, IntoParallelRefIterator, ParallelIterator, SimCache, Value, unpremul};

// ------------------------------------------------------------------ CC Particle World

pub(super) static PW_CACHE: SimCache<PState> = SimCache::new(6);

/// Particles per second per unit of CC Particle World's Birth Rate.
pub(super) const PW_RATE: f64 = 120.0;

/// CC Particle World's physics.
pub(super) fn pw_phys(ctx: &EffectCtx) -> Phys {
    let pr = ctx.params;
    Phys {
        rate: pr.f("birthRate").max(0.0) * PW_RATE,
        life: pr.f("longevity").max(0.0) as f32,
        producer: [pr.f("producer/producerX") as f32, pr.f("producer/producerY") as f32, pr.f("producer/producerZ") as f32],
        radius: [pr.f("producer/radiusX").max(0.0) as f32, pr.f("producer/radiusY").max(0.0) as f32, pr.f("producer/radiusZ").max(0.0) as f32],
        anim: pr.e("physics/animation"),
        speed: pr.f("physics/velocity") as f32 * 0.5,
        gravity: {
            let g = pr.f("physics/gravity") as f32;
            let gv =
                [pr.f("physics/gravityVector/gravityX") as f32, pr.f("physics/gravityVector/gravityY") as f32, pr.f("physics/gravityVector/gravityZ") as f32];
            [gv[0] * g, gv[1] * g, gv[2] * g]
        },
        resistance: pr.f("physics/resistance").max(0.0) as f32,
        axis: [pr.f("physics/directionAxis/axisX") as f32, pr.f("physics/directionAxis/axisY") as f32, pr.f("physics/directionAxis/axisZ") as f32],
        extra: pr.f("physics/extra") as f32,
        extra_angle: pr.f("physics/extraAngle") as f32,
        seed: (pr.f("extras/randomSeed") as u32).wrapping_mul(0x9e3779b9) ^ ctx.seed,
        max: 40_000,
        flat: false,
    }
}

/// CC Particle World's Particle Type ▸ Textured Square.
const PW_TEXTURED_SQUARE: u32 = 8;
/// CC Particle World's Particle ▸ Texture ▸ Texture Layer.
pub(super) const PW_TEXTURE_LAYER: &str = "particle/texture/textureLayer";

/// Whether CC Particle World draws its Texture Layer on the particles (Textured Square with a
/// layer chosen). Only the CPU does: `catalog::gpu_supported` keeps this case off the GPU.
pub(crate) fn particle_world_textured(ctx: &EffectCtx) -> bool {
    ctx.params.e("particle/particleType") == PW_TEXTURED_SQUARE && ctx.params.get(PW_TEXTURE_LAYER).and_then(Value::as_layer).is_some()
}

pub(super) fn particle_world(ctx: &EffectCtx, b: Buf) -> Buf {
    // The texture at the current comp time, shared by every shutter sample. A layer the host
    // can't give (the effect's own layer with its effects, a reference cycle) leaves the
    // squares in the particle colour. Authored buffers (text, shapes) span only their content:
    // covering the whole layer keeps the clamped sampling from stretching their edge pixels.
    let tex = particle_world_textured(ctx).then(|| ctx.layer_param(PW_TEXTURE_LAYER, true)).flatten().map(|mut t| {
        t.buf.cover(t.size);
        t
    });
    let (w, h) = (b.img.width, b.img.height);
    let draw = |plan: &SpritePlan| match &tex {
        Some(t) => splat_textured(w, h, &plan.sprites, plan.acc, t),
        None => splat(w, h, &plan.sprites, plan.acc),
    };
    // #1195: with the comp's motion-blur shutter open, particles draw as motion trails: the
    // sprite fields of samples across the shutter are averaged, as Transform averages its
    // matrix samples. (The GPU compositor runs this case on the CPU: catalog::gpu_supported.)
    if let Some((angle, phase, n)) = ctx.env.shutter.filter(|(a, _, _)| *a > 0.0) {
        let n = (n.clamp(2, 64) as usize).max(2);
        let fd = 1.0 / ctx.fps().max(0.001);
        let mut acc = Image::new(w, h);
        let mut post: Option<Post> = None;
        for k in 0..n {
            let t = ctx.time + (phase + angle * k as f64 / (n - 1) as f64) / 360.0 * fd;
            let sub = EffectCtx { params: ctx.params, time: t, layer_size: ctx.layer_size, seed: ctx.seed, adjustment: ctx.adjustment, env: ctx.env };
            if let Some(plan) = particle_world_plan(&sub, &b) {
                let fx = draw(&plan);
                acc.data.iter_mut().zip(&fx.data).for_each(|(a, o)| (0..4).for_each(|c| a[c] += o[c]));
                post.get_or_insert(plan.post);
            }
        }
        acc.data.iter_mut().for_each(|p| p.iter_mut().for_each(|v| *v /= n as f32));
        return match post {
            Some(p) => p.apply(b, acc),
            None => b,
        };
    }
    match particle_world_plan(ctx, &b) {
        Some(plan) => {
            let fx = draw(&plan);
            plan.finish_with(b, fx)
        }
        None => b,
    }
}

/// Draw square sprites into a new transparent image of `w`×`h`, each showing the layer `tex`
/// stretched over the square (turned with it) at the sprite's opacity.
fn splat_textured(w: u32, hgt: u32, sprites: &[Sprite], acc: Acc, tex: &crate::LayerPixels) -> Image {
    let mut img = Image::new(w, hgt);
    let (tw, th) = (tex.size[0], tex.size[1]);
    let shade = |s: &Sprite, x: f32, y: f32| {
        // As [`sprite_shade`]'s Square, sub-pixel sprites included.
        let (r, fade) = if s.r < 0.5 { (0.5, s.r / 0.5) } else { (s.r, 1.0) };
        let (sn, cs) = s.rot.sin_cos();
        let (dx, dy) = (x - s.x, y - s.y);
        let (ux, uy) = (dx * cs + dy * sn, -dx * sn + dy * cs);
        let e = r * 0.75;
        let a = aa(e - ux.abs()) * aa(e - uy.abs()) * s.c[3] * fade;
        if a <= 1e-6 {
            return None;
        }
        let (u, v) = (((ux / e + 1.0) * 0.5).clamp(0.0, 1.0) as f64, ((uy / e + 1.0) * 0.5).clamp(0.0, 1.0) as f64);
        let (px, py) = tex.buf.to_px([u * tw, v * th]);
        Some(tex.buf.img.sample_bilinear_clamped(px, py).map(|c| c * a))
    };
    raster(&mut img, sprites, sprite_bbox, shade, acc);
    img
}

/// Effect Camera ▸ FOV's default (degrees).
pub(super) const PW_FOV: f64 = 50.0;

/// What CC Particle World sees its particles through.
enum PwCamera {
    /// Extras ▸ Effect Camera, in world units (layer widths): `dist` from the world origin (the
    /// layer centre) along −z, the world turned by `rot` (`None` = not turned) about the origin,
    /// `focal` the scale at unit depth; particles nearer than `near` aren't drawn.
    Effect { rot: Option<[[f32; 3]; 3]>, dist: f32, focal: f32, near: f32 },
    /// The comp's active camera layer, on buffer-world points (After Effects then ignores the
    /// Effect Camera).
    Comp(crate::card3d::Proj),
}

/// A particle as the camera sees it: depth (larger is farther), buffer pixel, size scale `k`
/// (buffer pixels per world unit = `k · layer width · buffer scale`) and velocity in world units
/// on screen (× the same factor for buffer pixels).
struct PwSeen {
    depth: f32,
    x: f32,
    y: f32,
    k: f32,
    v: [f32; 2],
}

impl PwCamera {
    fn new(ctx: &EffectCtx, b: &Buf) -> PwCamera {
        let [lw, lh] = ctx.layer_size;
        if let Some(cam) = ctx.env.host.and_then(|h| h.comp_scene()).filter(|sc| sc.camera_layer).and_then(|sc| sc.camera) {
            let c = [b.offset[0] + lw * 0.5 * b.scale, b.offset[1] + lh * 0.5 * b.scale];
            return PwCamera::Comp(crate::card3d::comp_proj(&cam, c, b));
        }
        let f = |id: &str, d: f64| ctx.params.get(&format!("{PW_EFFECT_CAMERA}{id}")).map(Value::as_f64).filter(|v| v.is_finite()).unwrap_or(d);
        // The camera keeps the origin's plane at the scale Distance gives it as FOV changes
        // (moving in for a wider view), so FOV sets how strong the perspective is. Distance 2
        // shows the origin's plane at the layer's own scale.
        let half = |fov: f64| (fov.clamp(1.0, 179.0).to_radians() * 0.5).tan();
        let zoom = half(PW_FOV) / half(f("fov", PW_FOV));
        let dist = f("distance", 2.0).max(0.01) * zoom;
        let a = [f("rotationX", 0.0), f("rotationY", 0.0), f("rotationZ", 0.0)];
        let rot = (a != [0.0; 3]).then(|| {
            let r = crate::card3d::rot_ordered(a[0].to_radians(), a[1].to_radians(), a[2].to_radians(), 0);
            r.map(|row| row.map(|v| v as f32))
        });
        PwCamera::Effect { rot, dist: dist as f32, focal: (2.0 * zoom) as f32, near: (0.025 * dist) as f32 }
    }

    /// How the camera sees world point `p` moving at `v`; `None` behind or too close to it.
    fn see(&self, p: [f32; 3], v: [f32; 3], lw: f32, lh: f32, b: &Buf) -> Option<PwSeen> {
        match self {
            PwCamera::Effect { rot, dist, focal, near } => {
                let turn = |r: &[[f32; 3]; 3], q: [f32; 3]| r.map(|row| row[0] * q[0] + row[1] * q[1] + row[2] * q[2]);
                let (p, v) = match rot {
                    Some(r) => (turn(r, p), turn(r, v)),
                    None => (p, v),
                };
                let zc = dist + p[2];
                if zc.is_nan() || zc <= *near {
                    return None;
                }
                let k = focal / zc;
                let (x, y) = b.to_px([(lw * 0.5 + p[0] * lw * k) as f64, (lh * 0.5 + p[1] * lw * k) as f64]);
                Some(PwSeen { depth: zc, x: x as f32, y: y as f32, k, v: [v[0], v[1]] })
            }
            PwCamera::Comp(proj) => {
                let (s, lw, lh) = (b.scale, lw as f64, lh as f64);
                let u = lw * s;
                let at = [b.offset[0] + lw * 0.5 * s + p[0] as f64 * u, b.offset[1] + lh * 0.5 * s + p[1] as f64 * u, p[2] as f64 * u];
                let seen = proj.view(at, [v[0] as f64, v[1] as f64, v[2] as f64])?;
                let k = seen.scale;
                Some(PwSeen {
                    depth: seen.depth as f32,
                    x: seen.at[0] as f32,
                    y: seen.at[1] as f32,
                    k: k as f32,
                    v: [(seen.vel[0] / k) as f32, (seen.vel[1] / k) as f32],
                })
            }
        }
    }
}

/// CC Particle World's sprites (far to near), `None` before anything is born.
pub(super) fn particle_world_plan(ctx: &EffectCtx, b: &Buf) -> Option<SpritePlan> {
    let pr = ctx.params;
    let (lw, lh) = (ctx.layer_size[0] as f32, ctx.layer_size[1] as f32);
    let phys = pw_phys(ctx);
    if phys.rate <= 0.0 && ctx.time <= 0.0 {
        return None;
    }
    let st = pw_state(ctx, &phys);
    let kind = pr.e("particle/particleType");
    let birth = pr.color("particle/birthColor");
    let death = pr.color("particle/deathColor");
    let (bs, ds) = (pr.f("particle/birthSize") as f32, pr.f("particle/deathSize") as f32);
    let svar = pr.f("particle/sizeVariation") as f32 / 100.0;
    let max_op = pr.f("particle/maxOpacity") as f32 / 100.0;
    let s = b.scale as f32;
    let cam = PwCamera::new(ctx, b);
    let mut list: Vec<(f32, Sprite)> = st
        .parts
        .par_iter()
        .filter_map(|q| {
            let PwSeen { depth, x, y, k, v } = cam.see(q.p, q.v, lw, lh, b)?;
            let (c, size) = particle_look(q, birth, death, bs, ds, svar, max_op);
            let r = size * 0.5 * lw * k * s * 0.25;
            let sp = type_sprite(kind, x, y, r, c, [v[0] * lw * k * s, v[1] * lw * k * s], (q.id % 628) as f32 * 0.01 + q.age * 2.0);
            Some((depth, sp))
        })
        .collect();
    list.sort_by(|a, b| b.0.total_cmp(&a.0));
    let sprites: Vec<Sprite> = list.into_iter().map(|(_, s)| s).collect();
    Some(particle_sprites(sprites, pr.e("particle/transferMode")))
}

/// The CC particle systems' sprites with their Transfer Mode (Composite, Screen, Add, Black
/// Background).
fn particle_sprites(sprites: Vec<Sprite>, mode: u32) -> SpritePlan {
    // The particles replace the layer (After Effects shows no trace of the source in any
    // Transfer Mode, which sets how the particles meet each other).
    SpritePlan { sprites, tints: vec![], acc: if mode == 2 { Acc::Add } else { Acc::Over }, post: Post::Replace }
}

// ------------------------------------------------------------------ CC Particle Systems II

pub(super) static PS2_CACHE: SimCache<PState> = SimCache::new(6);

/// Particles per second per unit of CC Particle Systems II's Birth Rate.
pub(super) const PS2_RATE: f64 = 60.0;

/// CC Particle Systems II's physics.
pub(super) fn ps2_phys(ctx: &EffectCtx) -> Phys {
    let pr = ctx.params;
    let lh = ctx.layer_size[1] as f32;
    let unit = lh.max(1.0);
    let pos = pr.v2("producer/position");
    let dir = (pr.f("physics/direction") as f32).to_radians();
    Phys {
        rate: pr.f("birthRate").max(0.0) * PS2_RATE,
        life: pr.f("longevity").max(0.0) as f32,
        producer: [pos[0] as f32 / unit, pos[1] as f32 / unit, 0.0],
        radius: [pr.f("producer/radiusX").max(0.0) as f32 / unit, pr.f("producer/radiusY").max(0.0) as f32 / unit, 0.0],
        anim: [0u32, 1, 2, 3, 4, 5, 6, 7, 8, 9][pr.e("physics/animation").min(9) as usize],
        speed: pr.f("physics/velocity") as f32 * 0.25,
        gravity: [0.0, pr.f("physics/gravity") as f32 * 0.5, 0.0],
        resistance: pr.f("physics/resistance").max(0.0) as f32,
        // AE angles: 0° = up, clockwise.
        axis: [dir.sin(), -dir.cos(), 0.0],
        extra: pr.f("physics/extra") as f32,
        extra_angle: 45.0 * pr.f("physics/extra") as f32,
        seed: (pr.f("randomSeed") as u32).wrapping_mul(0x85ebca6b) ^ ctx.seed,
        max: 40_000,
        flat: true,
    }
}

pub(super) fn particle_systems2(ctx: &EffectCtx, b: Buf) -> Buf {
    particle_systems2_plan(ctx, &b).finish(b)
}

/// CC Particle Systems II's sprites.
pub(super) fn particle_systems2_plan(ctx: &EffectCtx, b: &Buf) -> SpritePlan {
    let pr = ctx.params;
    let lh = ctx.layer_size[1] as f32;
    let unit = lh.max(1.0);
    let st = PS2.state(ctx, &ps2_phys(ctx));
    let kind = pr.e("particle/particleType");
    let birth = pr.color("particle/birthColor");
    let death = pr.color("particle/deathColor");
    let (bs, ds) = (pr.f("particle/birthSize") as f32, pr.f("particle/deathSize") as f32);
    let svar = pr.f("particle/sizeVariation") as f32 / 100.0;
    let max_op = pr.f("particle/maxOpacity") as f32 / 100.0;
    let s = b.scale as f32;
    let sprites: Vec<Sprite> = st
        .parts
        .iter()
        .map(|q| {
            let (bx, by) = b.to_px([(q.p[0] * unit) as f64, (q.p[1] * unit) as f64]);
            let (c, size) = particle_look(q, birth, death, bs, ds, svar, max_op);
            let r = size * unit * 0.02 * s;
            type_sprite(kind, bx as f32, by as f32, r, c, [q.v[0] * unit * s, q.v[1] * unit * s], (q.id % 628) as f32 * 0.01 + q.age * 2.0)
        })
        .collect();
    particle_sprites(sprites, pr.e("particle/transferMode"))
}

// ------------------------------------------------------------------ CC Mr. Mercury

pub(super) static MERCURY_CACHE: SimCache<PState> = SimCache::new(6);

/// Blobs per second per unit of CC Mr. Mercury's Birth Rate.
pub(super) const MERCURY_RATE: f64 = 10.0;

/// A CC Mr. Mercury blob (buffer px).
#[derive(Clone, Copy, Debug)]
pub struct Blob {
    pub x: f32,
    pub y: f32,
    pub r: f32,
}

/// CC Mr. Mercury's physics.
pub(super) fn mercury_phys(ctx: &EffectCtx) -> Phys {
    let pr = ctx.params;
    let unit = (ctx.layer_size[1] as f32).max(1.0);
    let pos = pr.v2("producer");
    let dir = (pr.f("direction") as f32).to_radians();
    Phys {
        rate: pr.f("birthRate").max(0.0) * MERCURY_RATE,
        life: pr.f("longevity").max(0.0) as f32,
        producer: [pos[0] as f32 / unit, pos[1] as f32 / unit, 0.0],
        radius: [pr.f("radiusX").max(0.0) as f32 / unit, pr.f("radiusY").max(0.0) as f32 / unit, 0.0],
        anim: [0u32, 1, 2, 3, 4, 5, 6, 7, 8, 9][pr.e("animation").min(9) as usize],
        speed: pr.f("velocity") as f32 * 0.3,
        gravity: [0.0, pr.f("gravity") as f32 * 0.5, 0.0],
        resistance: pr.f("resistance").max(0.0) as f32,
        axis: [dir.sin(), -dir.cos(), 0.0],
        extra: pr.f("extra") as f32,
        extra_angle: 45.0 * pr.f("extra") as f32,
        seed: ctx.seed.wrapping_mul(0xc2b2ae35),
        max: 4_000,
        flat: true,
    }
}

/// CC Mr. Mercury's blobs this frame, from its particle simulation (for buffer geometry `b`).
pub fn mercury_blobs(ctx: &EffectCtx, b: &Buf) -> Vec<Blob> {
    let pr = ctx.params;
    let unit = (ctx.layer_size[1] as f32).max(1.0);
    let phys = mercury_phys(ctx);
    let st = MERCURY.state(ctx, &phys);
    let (bs, ds) = (pr.f("blobBirthSize") as f32, pr.f("blobDeathSize") as f32);
    let s = b.scale as f32;
    st.parts
        .iter()
        .map(|q| {
            let u = (q.age / q.life.max(1e-4)).clamp(0.0, 1.0);
            let size = bs + (ds - bs) * u;
            let (bx, by) = b.to_px([(q.p[0] * unit) as f64, (q.p[1] * unit) as f64]);
            Blob { x: bx as f32, y: by as f32, r: (size * unit * 0.08 * s).max(0.0) }
        })
        .filter(|q| q.r > 0.1)
        .collect()
}

pub(super) fn mr_mercury(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let pr = ctx.params;
    let blobs = mercury_blobs(ctx, &b);
    let influence = pr.f("blobInfluence") as f32 / 100.0;
    let s = b.scale as f32;
    let (w, hh) = (b.img.width, b.img.height);
    // Field and gradient accumulate additively: [f, gx, gy, _].
    let mut field = Image::new(w, hh);
    let infl = 1.0 + influence;
    raster(
        &mut field,
        &blobs,
        |q| {
            let e = q.r * 2.0 * infl;
            Some([q.x - e, q.y - e, q.x + e, q.y + e])
        },
        |q, x, y| {
            let (dx, dy) = (x - q.x, y - q.y);
            let rr = q.r * 2.0 * infl;
            let d2 = (dx * dx + dy * dy) / (rr * rr);
            if d2 >= 1.0 {
                return None;
            }
            let k = 1.0 - d2;
            let f = k * k * k;
            let g = -6.0 * k * k / (rr * rr);
            Some([f, g * dx, g * dy, 0.0])
        },
        Acc::Add,
    );
    let thr = 0.35f32;
    let ambient = pr.f("shading/ambient") as f32 / 100.0;
    let diffuse = pr.f("shading/diffuse") as f32 / 100.0;
    let specular = pr.f("shading/specular") as f32 / 100.0;
    let rough = pr.f("shading/roughness").max(0.001) as f32;
    let metal = pr.f("shading/metal") as f32 / 100.0;
    let li = pr.f("light/lightIntensity") as f32 / 100.0;
    let lc = pr.color("light/lightColor");
    let ldir = (pr.f("light/lightDirection") as f32).to_radians();
    let lheight = pr.f("light/lightHeight") as f32 / 100.0;
    let l = norm3([ldir.sin(), -ldir.cos(), lheight.max(0.05)]);
    let src = b.img.clone();
    let mut out = Image::new(w, hh);
    out.rows_mut().for_each(|(y, row)| {
        for (x, px) in row.iter_mut().enumerate() {
            let fv = field.data[y * w as usize + x];
            let f = fv[0];
            if f <= thr * 0.8 {
                continue;
            }
            let cov = ((f - thr * 0.8) / (thr * 0.4)).clamp(0.0, 1.0);
            let n = norm3([fv[1] * 3.0, fv[2] * 3.0, 1.0 / (1.0 + f)]);
            // Refract the layer through the blob.
            let sp = src.sample_bilinear_clamped(x as f64 + 0.5 - n[0] as f64 * 12.0 * s as f64, y as f64 + 0.5 - n[1] as f64 * 12.0 * s as f64);
            let (c, sa) = unpremul(sp);
            let base = if sa > 0.0 { c } else { [0.7, 0.7, 0.75] };
            let nl = (n[0] * l[0] + n[1] * l[1] + n[2] * l[2]).max(0.0);
            let hv = norm3([l[0], l[1], l[2] + 1.0]);
            let spec = (n[0] * hv[0] + n[1] * hv[1] + n[2] * hv[2]).max(0.0).powf(1.0 / rough) * specular * li;
            let shade = ambient + diffuse * nl * li;
            let cc: [f32; 3] = std::array::from_fn(|k| base[k] * shade + spec * (lc[k] * (1.0 - metal) + base[k] * metal));
            *px = [cc[0] * cov, cc[1] * cov, cc[2] * cov, cov];
        }
    });
    b.img = out;
    b
}

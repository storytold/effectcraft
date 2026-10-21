//! Simulation effects (particles, physics, liquids): CC Rainfall, CC Snowfall, CC Bubbles,
//! CC Drizzle, CC Star Burst, CC Particle World, CC Particle Systems II and CC Mr. Mercury.
//!
//! Written from the public descriptions of the effects' behaviour. Weather/bubble/star effects
//! are closed-form functions of time (every frame independent); particle systems are stepped
//! from layer time 0 at a fixed rate through [`SimCache`], so any frame can be rendered
//! directly and equals the frame reached by scrubbing.
//!
//! Shared here (also used by `sim2`): a band-bucketed rasteriser for many small items
//! ([`raster`]), particle sprites ([`Sprite`]) and transfer-mode compositing ([`combine`]).

use effectcraft_keyframe::Value;
use effectcraft_project::ParamUi;
use effectcraft_raster::{Image, Px};
use rayon::prelude::*;

use crate::generate::value_noise;
use crate::util::{SimCache, hash1, params_key, params_key_except, unpremul};
use crate::{Buf, EffectCtx, EffectSpec, col, num, p, popup, slider};

pub(crate) fn spec(id: &'static str, name: &'static str, params: Vec<crate::ParamSpec>, render: crate::RenderFn) -> EffectSpec {
    EffectSpec { id, name, category: "Simulation", params, render, gpu: false, float: true }
}

pub(crate) fn pt(x: f64, y: f64) -> Value {
    Value::Vec2([x, y])
}

pub(crate) fn pct() -> ParamUi {
    slider(0.0, 100.0, 0.0, 100.0, 1)
}

/// Deterministic hash → [0, 1).
#[inline]
pub(crate) fn h(i: u32, k: u32, seed: u32) -> f32 {
    hash1(i, k, seed)
}

/// Signed hash → [-1, 1).
#[inline]
pub(crate) fn hs(i: u32, k: u32, seed: u32) -> f32 {
    hash1(i, k, seed) * 2.0 - 1.0
}

// ------------------------------------------------------------------ band rasteriser

const BAND: usize = 16;

/// How rasterised items combine with what is already in the target.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Acc {
    /// Premultiplied "over" (later items on top).
    Over,
    /// Additive (glows, light).
    Add,
}

/// Rasterise many small items into `img` (buffer pixels). `bbox` gives an item's pixel bounds
/// `[x0, y0, x1, y1]` (or `None` to skip), `shade` its premultiplied colour at a pixel centre.
/// Items are bucketed into horizontal bands rendered in parallel; within a band they are drawn
/// in slice order.
pub(crate) fn raster<T: Sync>(
    img: &mut Image,
    items: &[T],
    bbox: impl Fn(&T) -> Option<[f32; 4]> + Sync,
    shade: impl Fn(&T, f32, f32) -> Option<Px> + Sync,
    acc: Acc,
) {
    let (w, hgt) = (img.width as usize, img.height as usize);
    if w == 0 || hgt == 0 || items.is_empty() {
        return;
    }
    let nb = hgt.div_ceil(BAND);
    let boxes: Vec<Option<[i32; 4]>> = items
        .par_iter()
        .map(|it| {
            let b = bbox(it)?;
            if !(b[0].is_finite() && b[1].is_finite() && b[2].is_finite() && b[3].is_finite()) {
                return None;
            }
            let x0 = b[0].floor().max(0.0) as i32;
            let y0 = b[1].floor().max(0.0) as i32;
            let x1 = (b[2].ceil() as i32).min(w as i32 - 1);
            let y1 = (b[3].ceil() as i32).min(hgt as i32 - 1);
            if x1 < x0 || y1 < y0 || b[2] < 0.0 || b[3] < 0.0 {
                return None;
            }
            Some([x0, y0, x1, y1])
        })
        .collect();
    let mut buckets: Vec<Vec<u32>> = vec![Vec::new(); nb];
    for (i, b) in boxes.iter().enumerate() {
        if let Some(b) = b {
            for band in (b[1] as usize / BAND)..=(b[3] as usize / BAND).min(nb - 1) {
                buckets[band].push(i as u32);
            }
        }
    }
    img.data.par_chunks_mut(w * BAND).enumerate().for_each(|(band, chunk)| {
        let by0 = band * BAND;
        let rows = chunk.len() / w;
        for &i in &buckets[band] {
            let Some(b) = boxes[i as usize] else { continue };
            let it = &items[i as usize];
            let ya = (b[1] as usize).max(by0);
            let yb = (b[3] as usize).min(by0 + rows - 1);
            for y in ya..=yb {
                let row = &mut chunk[(y - by0) * w..(y - by0 + 1) * w];
                for x in b[0] as usize..=b[2] as usize {
                    if let Some(c) = shade(it, x as f32 + 0.5, y as f32 + 0.5) {
                        let d = &mut row[x];
                        match acc {
                            Acc::Over => {
                                let k = 1.0 - c[3];
                                *d = [c[0] + d[0] * k, c[1] + d[1] * k, c[2] + d[2] * k, c[3] + d[3] * k];
                            }
                            Acc::Add => {
                                *d = [d[0] + c[0], d[1] + c[1], d[2] + c[2], (d[3] + c[3]).min(1.0)];
                            }
                        }
                    }
                }
            }
        }
    });
}

// ------------------------------------------------------------------ sprites

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Shape {
    /// Soft round dot (Gaussian-like falloff).
    Soft,
    /// Hard anti-aliased disc.
    Disc,
    /// Lit sphere (upper-left light, specular).
    Sphere,
    /// Disc fading linearly to its rim.
    Faded,
    /// Thin ring with a highlight.
    Bubble,
    /// Four-point star.
    Star,
    /// Rotated square (angle in `rot`).
    Square,
    /// Triangle (angle in `rot`).
    Tri,
    /// Segment from (x, y) by (dx, dy), half-width `r`.
    Line { dx: f32, dy: f32 },
}

/// A particle sprite in buffer pixels; `c` is straight colour + opacity.
#[derive(Clone, Copy, Debug)]
pub struct Sprite {
    pub x: f32,
    pub y: f32,
    pub r: f32,
    pub c: [f32; 4],
    pub shape: Shape,
    pub rot: f32,
}

impl Sprite {
    pub(crate) fn new(x: f32, y: f32, r: f32, c: [f32; 4], shape: Shape) -> Sprite {
        Sprite { x, y, r, c, shape, rot: 0.0 }
    }
}

/// Where a sprite's colour comes from when it depends on the layer's pixels (resolved by
/// [`SpritePlan::resolve`] on the CPU, per pixel by the GPU compositor). Positions are buffer
/// pixels.
#[derive(Clone, Copy, Debug)]
pub enum Tint {
    /// The sprite's own colour.
    Fixed,
    /// CC Rainfall's refracting drops: where the layer shows (alpha > 0), colour × (layer
    /// colour × 0.6 + 0.5) sampled at (x, y).
    Refract { x: f64, y: f64 },
    /// CC Star Burst: the layer's colour at (x, y), opacity × its alpha (none where clear).
    Source { x: f64, y: f64 },
    /// CC Hair: the root's colour at (x, y) mixed with the hair colour (`inherit` = 1 keeps the
    /// root's), × `shade`, + `spec`; no strand where the root's alpha is below ½.
    Root { x: f64, y: f64, hair: [f32; 3], inherit: f32, shade: f32, spec: f32 },
}

/// How a sprite layer meets the original layer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Post {
    /// [`combine`] with this mode (0 Composite … 5 sprites only).
    Combine(u32),
    /// The sprites alone.
    Replace,
    /// The sprites, mixed toward the original by this fraction.
    Lerp(f32),
    /// The sprites over opaque black.
    OverBlack,
}

/// What a sprite-drawing effect draws this frame: built on the CPU (closed-form or from the
/// simulation state), rasterised by [`SpritePlan::finish`] or the GPU compositor.
#[derive(Clone, Debug)]
pub struct SpritePlan {
    pub sprites: Vec<Sprite>,
    /// Per sprite (empty = all [`Tint::Fixed`]).
    pub tints: Vec<Tint>,
    pub acc: Acc,
    pub post: Post,
}

impl SpritePlan {
    /// Resolve layer-dependent colours against the layer `src`.
    pub fn resolve(&mut self, src: &Image) {
        let tints = std::mem::take(&mut self.tints);
        self.sprites.par_iter_mut().zip(tints.par_iter()).for_each(|(sp, t)| match *t {
            Tint::Fixed => {}
            Tint::Refract { x, y } => {
                let (sc, sa) = unpremul(src.sample_bilinear(x, y));
                if sa > 0.0 {
                    let c = sp.c;
                    sp.c = [(sc[0] * 0.6 + 0.5) * c[0], (sc[1] * 0.6 + 0.5) * c[1], (sc[2] * 0.6 + 0.5) * c[2], c[3]];
                }
            }
            Tint::Source { x, y } => {
                let (c, a) = unpremul(src.sample_bilinear(x, y));
                sp.c = if a <= 0.0 { [0.0; 4] } else { [c[0], c[1], c[2], a * sp.c[3]] };
            }
            Tint::Root { x, y, hair, inherit, shade, spec } => {
                let root = src.sample_bilinear(x, y);
                if root[3] < 0.5 {
                    sp.c = [0.0; 4];
                    return;
                }
                let (rc, _) = unpremul(root);
                let base: [f32; 3] = std::array::from_fn(|k| rc[k] + (hair[k] - rc[k]) * (1.0 - inherit));
                sp.c = [base[0] * shade + spec, base[1] * shade + spec, base[2] * shade + spec, sp.c[3]];
            }
        });
    }

    /// Rasterise (after [`SpritePlan::resolve`]) and meet the original layer `b`.
    pub fn finish(&self, b: Buf) -> Buf {
        let fx = splat(b.img.width, b.img.height, &self.sprites, self.acc);
        self.finish_with(b, fx)
    }

    /// [`finish`](Self::finish) with a prepared sprite image (the motion-blurred average of
    /// several frames, #1195).
    pub fn finish_with(&self, b: Buf, fx: Image) -> Buf {
        self.post.apply(b, fx)
    }
}

impl Post {
    /// Meet the sprite image `fx` with the original layer `b` (see [`SpritePlan::finish`]).
    fn apply(self, mut b: Buf, fx: Image) -> Buf {
        b.img = match self {
            Post::Combine(m) => combine(&b.img, &fx, m),
            Post::Replace => fx,
            Post::Lerp(t) => {
                let mut o = fx;
                o.data.par_iter_mut().zip(b.img.data.par_iter()).for_each(|(o, s0)| *o = crate::util::lerp4(*o, *s0, t));
                o
            }
            Post::OverBlack => combine(&Image::filled(b.img.width, b.img.height, [0.0, 0.0, 0.0, 1.0]), &fx, 0),
        };
        b
    }
}

/// The sprite plan of a sprite-drawing simulation effect for buffer geometry `b` (its pixels
/// are not read), `None` for other effects or settings drawn otherwise (the GPU compositor's
/// render pass).
pub fn sprite_plan(id: &str, ctx: &EffectCtx, b: &Buf) -> Option<SpritePlan> {
    match id {
        "ec.sim.ccrainfall" => Some(rainfall_plan(ctx, b)),
        "ec.sim.ccsnowfall" => Some(snowfall_plan(ctx, b)),
        "ec.sim.ccstarburst" => Some(star_burst_plan(ctx, b)),
        "ec.sim.cchair" => crate::sim3::hair_plan(ctx, b),
        "ec.sim.waveworld" => crate::sim2::wave_world_plan(ctx, b).and_then(|p| match p {
            crate::sim2::WavePlan::Wire(sp) => Some(sp),
            crate::sim2::WavePlan::Height { .. } => None,
        }),
        "ec.sim.foam" => crate::sim2::foam_plan(ctx, b),
        "ec.sim.ccparticleworld" => particle_world_plan(ctx, b),
        "ec.sim.ccparticlesystems2" => Some(particle_systems2_plan(ctx, b)),
        _ => None,
    }
}

fn sprite_bbox(s: &Sprite) -> Option<[f32; 4]> {
    if s.c[3] <= 1e-5 || s.r <= 0.0 {
        return None;
    }
    let r = s.r.max(0.5) + 1.0;
    Some(match s.shape {
        Shape::Line { dx, dy } => [s.x.min(s.x + dx) - r, s.y.min(s.y + dy) - r, s.x.max(s.x + dx) + r, s.y.max(s.y + dy) + r],
        Shape::Star => [s.x - r * 2.0, s.y - r * 2.0, s.x + r * 2.0, s.y + r * 2.0],
        _ => [s.x - r, s.y - r, s.x + r, s.y + r],
    })
}

#[inline]
fn aa(edge: f32) -> f32 {
    (edge + 0.5).clamp(0.0, 1.0)
}

fn sprite_shade(s: &Sprite, x: f32, y: f32) -> Option<Px> {
    // Sub-pixel sprites keep their energy: draw at radius 0.5 with reduced opacity.
    let (r, fade) = if s.r < 0.5 { (0.5, s.r / 0.5) } else { (s.r, 1.0) };
    let (dx, dy) = (x - s.x, y - s.y);
    let mut rgb = [s.c[0], s.c[1], s.c[2]];
    let cov = match s.shape {
        Shape::Soft => {
            let d2 = (dx * dx + dy * dy) / (r * r);
            if d2 >= 1.0 {
                return None;
            }
            let q = 1.0 - d2;
            q * q
        }
        Shape::Disc => aa(r - (dx * dx + dy * dy).sqrt()),
        Shape::Faded => {
            let d = (dx * dx + dy * dy).sqrt() / r;
            (1.0 - d).max(0.0).powf(1.5)
        }
        Shape::Sphere => {
            let d = (dx * dx + dy * dy).sqrt();
            let cv = aa(r - d);
            if cv <= 0.0 {
                return None;
            }
            let (nx, ny) = (dx / r, dy / r);
            let nz = (1.0 - nx * nx - ny * ny).max(0.0).sqrt();
            let (lx, ly, lz) = (-0.48, -0.58, 0.66);
            let diff = (nx * lx + ny * ly + nz * lz).max(0.0);
            let rz = 2.0 * diff * nz - lz;
            let spec = rz.max(0.0).powi(20) * 0.6;
            let k = 0.2 + 0.8 * diff;
            rgb = [rgb[0] * k + spec, rgb[1] * k + spec, rgb[2] * k + spec];
            cv
        }
        Shape::Bubble => {
            let d = (dx * dx + dy * dy).sqrt() / r;
            if d > 1.0 + 1.0 / r {
                return None;
            }
            let ring = (1.0 - ((d - 0.9) / 0.12).powi(2)).max(0.0);
            let hl = (1.0 - (((dx / r) + 0.4).powi(2) + ((dy / r) + 0.4).powi(2)) / 0.04).max(0.0);
            (0.12 * (d <= 1.0) as u8 as f32 + ring * 0.8 + hl).min(1.0)
        }
        Shape::Star => {
            let (sn, cs) = s.rot.sin_cos();
            let (ux, uy) = ((dx * cs + dy * sn) / r, (-dx * sn + dy * cs) / r);
            let d = (ux * ux + uy * uy).sqrt();
            let core = (1.0 - d).max(0.0).powi(2);
            let sx = (1.0 - ux.abs() / 2.0).max(0.0) * (1.0 - (uy.abs() * 6.0)).max(0.0);
            let sy = (1.0 - uy.abs() / 2.0).max(0.0) * (1.0 - (ux.abs() * 6.0)).max(0.0);
            (core + sx + sy).min(1.0)
        }
        Shape::Square => {
            let (sn, cs) = s.rot.sin_cos();
            let (ux, uy) = (dx * cs + dy * sn, -dx * sn + dy * cs);
            let e = r * 0.75;
            aa(e - ux.abs()) * aa(e - uy.abs())
        }
        Shape::Tri => {
            let (sn, cs) = s.rot.sin_cos();
            let (ux, uy) = (dx * cs + dy * sn, -dx * sn + dy * cs);
            // Equilateral triangle with circumradius r: three half-planes.
            let e = r * 0.5;
            let mut m = f32::INFINITY;
            for k in 0..3 {
                let a = std::f32::consts::FRAC_PI_2 + k as f32 * std::f32::consts::TAU / 3.0;
                let (ns, nc) = a.sin_cos();
                m = m.min(e - (ux * nc + uy * ns));
            }
            aa(m)
        }
        Shape::Line { dx: lx, dy: ly } => {
            let l2 = lx * lx + ly * ly;
            let t = if l2 > 0.0 { ((dx * lx + dy * ly) / l2).clamp(0.0, 1.0) } else { 0.0 };
            let (px, py) = (dx - lx * t, dy - ly * t);
            aa(r - (px * px + py * py).sqrt())
        }
    };
    let a = cov * s.c[3] * fade;
    if a <= 1e-6 {
        return None;
    }
    Some([rgb[0] * a, rgb[1] * a, rgb[2] * a, a])
}

/// Draw sprites into a new transparent image of `w`×`h`.
pub(crate) fn splat(w: u32, hgt: u32, sprites: &[Sprite], acc: Acc) -> Image {
    let mut img = Image::new(w, hgt);
    raster(&mut img, sprites, sprite_bbox, sprite_shade, acc);
    img
}

/// Combine an effect layer `fx` with the original `src` (both premultiplied, same size):
/// 0 Composite (fx over src), 1 Screen, 2 Add, 3 Lighten, 4 Darken, 5 fx only.
pub(crate) fn combine(src: &Image, fx: &Image, mode: u32) -> Image {
    let mut out = fx.clone();
    out.data.par_iter_mut().zip(src.data.par_iter()).for_each(|(o, s)| {
        let f = *o;
        *o = match mode {
            0 => {
                let k = 1.0 - f[3];
                [f[0] + s[0] * k, f[1] + s[1] * k, f[2] + s[2] * k, f[3] + s[3] * k]
            }
            1 => [s[0] + f[0] - s[0] * f[0], s[1] + f[1] - s[1] * f[1], s[2] + f[2] - s[2] * f[2], s[3] + f[3] - s[3] * f[3]],
            2 => [s[0] + f[0], s[1] + f[1], s[2] + f[2], (s[3] + f[3]).min(1.0)],
            3 => [s[0].max(f[0]), s[1].max(f[1]), s[2].max(f[2]), s[3] + f[3] - s[3] * f[3]],
            4 => {
                let (fc, fa) = unpremul(f);
                let m = |c: f32, k: usize| c - (c - c.min(fc[k] * s[3])) * fa;
                [m(s[0], 0), m(s[1], 1), m(s[2], 2), s[3]]
            }
            _ => f,
        };
    });
    out
}

// ------------------------------------------------------------------ CC Rainfall / CC Snowfall

/// Shared weather layout: per-item depth factor `k` in (0, 1] (1 = nearest) from Scene Depth.
#[inline]
fn depth_k(z: f32, scene_depth: f32) -> f32 {
    1.0 / (1.0 + z * scene_depth / 2500.0)
}

/// Wrap `v` into `[lo, lo + span)`.
#[inline]
fn wrap(v: f32, lo: f32, span: f32) -> f32 {
    if span <= 0.0 { v } else { lo + (v - lo).rem_euclid(span) }
}

fn rainfall(ctx: &EffectCtx, b: Buf) -> Buf {
    let mut plan = rainfall_plan(ctx, &b);
    plan.resolve(&b.img);
    plan.finish(b)
}

fn rainfall_plan(ctx: &EffectCtx, b: &Buf) -> SpritePlan {
    let pr = ctx.params;
    let n = pr.f("drops").clamp(0.0, 100_000.0) as u32;
    let size = pr.f("size").max(0.0) as f32;
    let depth = pr.f("sceneDepth").max(0.0) as f32;
    let speed = pr.f("speed") as f32;
    let wind = pr.f("wind") as f32;
    let wvar = pr.f("windVariation") as f32 / 100.0;
    let spread = pr.f("spread") as f32;
    let color = pr.color("color");
    let opacity = pr.f("opacity") as f32 / 100.0;
    let mode = pr.e("transferMode");
    let with_orig = pr.b("compositeWithOriginal");
    let refract = pr.e("extras/appearance") == 0;
    let off = pr.v2("extras/offset");
    let seed = pr.f("extras/randomSeed") as u32 ^ ctx.seed.wrapping_mul(0x9e37);
    let (lw, lh) = (ctx.layer_size[0] as f32, ctx.layer_size[1] as f32);
    let s = b.scale as f32;
    let t = ctx.time as f32;
    let (ox, oy) = (off[0] as f32 - lw * 0.5, off[1] as f32 - lh * 0.5);
    let (sprites, tints): (Vec<Sprite>, Vec<Tint>) = (0..n)
        .into_par_iter()
        .map(|i| {
            let z = h(i, 1, seed);
            let k = depth_k(z, depth);
            let vy = speed / 4000.0 * lh * 2.2 * k;
            let w = wind / 1000.0 * lh * (1.0 + wvar * hs(i, 2, seed)) * k + spread * hs(i, 7, seed) * k;
            let len = (vy * 0.04 * size.max(0.2)).max(1.0);
            let lenx = w * 0.04 * size.max(0.2);
            let margin = len + lenx.abs() + 4.0;
            let y = wrap(h(i, 3, seed) * (lh + 2.0 * margin) + vy * t + oy, -margin, lh + 2.0 * margin);
            let x = wrap(h(i, 4, seed) * (lw + 2.0 * margin) + w * t + ox, -margin, lw + 2.0 * margin);
            let (bx, by) = b.to_px([x as f64, y as f64]);
            let c = [color[0], color[1], color[2], opacity * (0.35 + 0.65 * k)];
            // Refracting drops show the scene behind them, brightened and offset.
            let tint = if refract { Tint::Refract { x: bx + 2.0 * s as f64, y: by } } else { Tint::Fixed };
            let r = (size * 0.6 * k * s).max(0.25);
            (Sprite::new(bx as f32, by as f32, r, c, Shape::Line { dx: -lenx * s, dy: -len * s }), tint)
        })
        .unzip();
    SpritePlan { sprites, tints, acc: Acc::Over, post: weather_post(mode, with_orig) }
}

fn weather_post(mode: u32, with_orig: bool) -> Post {
    if with_orig { Post::Combine(if mode == 1 { 3 } else { 0 }) } else { Post::Replace }
}

fn snowfall(ctx: &EffectCtx, b: Buf) -> Buf {
    snowfall_plan(ctx, &b).finish(b)
}

fn snowfall_plan(ctx: &EffectCtx, b: &Buf) -> SpritePlan {
    let pr = ctx.params;
    let n = pr.f("flakes").clamp(0.0, 200_000.0) as u32;
    let size = pr.f("size").max(0.0) as f32;
    let svar = pr.f("sizeVariation") as f32 / 100.0;
    let depth = pr.f("sceneDepth").max(0.0) as f32;
    let speed = pr.f("speed") as f32;
    let spvar = pr.f("speedVariation") as f32 / 100.0;
    let wind = pr.f("wind") as f32;
    let wvar = pr.f("windVariation") as f32 / 100.0;
    let spread = pr.f("spread") as f32;
    let wig = pr.f("wiggle/wiggleAmount") as f32;
    let wigvar = pr.f("wiggle/wiggleAmountVariation") as f32 / 100.0;
    let wfreq = pr.f("wiggle/wiggleFrequency") as f32;
    let wfvar = pr.f("wiggle/wiggleFrequencyVariation") as f32 / 100.0;
    let color = pr.color("color");
    let opacity = pr.f("opacity") as f32 / 100.0;
    let mode = pr.e("transferMode");
    let with_orig = pr.b("compositeWithOriginal");
    let off = pr.v2("extras/offset");
    let seed = pr.f("extras/randomSeed") as u32 ^ ctx.seed.wrapping_mul(0x85eb);
    let (lw, lh) = (ctx.layer_size[0] as f32, ctx.layer_size[1] as f32);
    let s = b.scale as f32;
    let t = ctx.time as f32;
    let (ox, oy) = (off[0] as f32 - lw * 0.5, off[1] as f32 - lh * 0.5);
    let sprites: Vec<Sprite> = (0..n)
        .into_par_iter()
        .map(|i| {
            let z = h(i, 1, seed);
            let k = depth_k(z, depth);
            let r = (size * 0.5 * k * (1.0 + svar * hs(i, 2, seed))).max(0.05);
            let vy = speed * (lh / 600.0) * k * (1.0 + spvar * hs(i, 3, seed));
            let vx = wind * (lh / 1200.0) * k * (1.0 + wvar * hs(i, 4, seed)) + spread * hs(i, 8, seed) * k;
            let amp = wig * 2.0 * k * (1.0 + wigvar * hs(i, 5, seed));
            let fr = wfreq * (1.0 + wfvar * hs(i, 6, seed));
            let ph = h(i, 9, seed) * std::f32::consts::TAU;
            let margin = r + amp.abs() + 2.0;
            let y = wrap(h(i, 10, seed) * (lh + 2.0 * margin) + vy * t + oy, -margin, lh + 2.0 * margin);
            let x = wrap(h(i, 11, seed) * (lw + 2.0 * margin) + vx * t + ox, -margin, lw + 2.0 * margin) + amp * (fr * t * std::f32::consts::TAU + ph).sin();
            let (bx, by) = b.to_px([x as f64, y as f64]);
            Sprite::new(bx as f32, by as f32, r * s, [color[0], color[1], color[2], opacity * (0.4 + 0.6 * k)], Shape::Soft)
        })
        .collect();
    SpritePlan { sprites, tints: vec![], acc: Acc::Over, post: weather_post(mode, with_orig) }
}

// ------------------------------------------------------------------ CC Bubbles

/// A CC Bubbles bubble in buffer pixels.
#[derive(Clone, Copy, Debug)]
pub struct Bubble {
    pub x: f32,
    pub y: f32,
    pub r: f32,
    /// Source sample centre (buffer px).
    pub sx: f32,
    pub sy: f32,
}

/// CC Bubbles' bubbles this frame (closed-form in time) for buffer geometry `b`.
pub fn bubble_list(ctx: &EffectCtx, b: &Buf) -> Vec<Bubble> {
    let pr = ctx.params;
    let n = pr.f("bubbleAmount").clamp(0.0, 10_000.0) as u32;
    let speed = pr.f("bubbleSpeed") as f32;
    let wamp = pr.f("wobbleAmplitude") as f32;
    let wfreq = pr.f("wobbleFrequency") as f32;
    let bsize = pr.f("bubbleSize").max(0.0) as f32;
    let seed = ctx.seed.wrapping_mul(0x27d4);
    let (lw, lh) = (ctx.layer_size[0] as f32, ctx.layer_size[1] as f32);
    let s = b.scale as f32;
    let t = ctx.time as f32;
    let base = lw.min(lh) * 0.035 * bsize;
    (0..n)
        .map(|i| {
            let r = base * (0.5 + h(i, 1, seed));
            let travel = lh + 2.0 * r;
            let v = speed * lh * 0.3 * (0.6 + 0.8 * h(i, 2, seed));
            let y = lh + r - (h(i, 3, seed) * travel + v * t).rem_euclid(travel);
            let x0 = h(i, 4, seed) * lw;
            let x = x0 + wamp * (r / base.max(1e-3)) * (t * wfreq * std::f32::consts::TAU + h(i, 5, seed) * std::f32::consts::TAU).sin();
            let (bx, by) = b.to_px([x as f64, y as f64]);
            let (sx, sy) = b.to_px([x0 as f64, y as f64]);
            Bubble { x: bx as f32, y: by as f32, r: r * s, sx: sx as f32, sy: sy as f32 }
        })
        .collect()
}

fn bubbles(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let pr = ctx.params;
    let metal = pr.e("reflectionType") == 1;
    let shading = pr.e("shadingType");
    let list = bubble_list(ctx, &b);
    let src = b.img.clone();
    let mut out = Image::new(src.width, src.height);
    raster(
        &mut out,
        &list,
        |q| Some([q.x - q.r - 1.0, q.y - q.r - 1.0, q.x + q.r + 1.0, q.y + q.r + 1.0]),
        |q, x, y| {
            let (dx, dy) = ((x - q.x) / q.r.max(0.5), (y - q.y) / q.r.max(0.5));
            let d2 = dx * dx + dy * dy;
            let d = d2.sqrt();
            let cov = aa((1.0 - d) * q.r);
            if cov <= 0.0 {
                return None;
            }
            // Liquid refracts (inverted, magnified), metal reflects (wide-angle mirror).
            let k = if metal { 1.6 } else { -0.7 };
            let nz = (1.0 - d2).max(0.0).sqrt();
            let p = src.sample_bilinear((q.sx + dx * q.r * k) as f64, (q.sy + dy * q.r * k) as f64);
            let (c, sa) = unpremul(p);
            let mut c = if sa <= 0.0 { [0.6, 0.7, 0.8] } else { c };
            let mut a = cov;
            match shading {
                1 => c = c.map(|v| v * 0.7 + 0.3 * (1.0 - nz)),
                2 => c = c.map(|v| v * (0.5 + 0.5 * nz)),
                3 => a *= nz.powf(0.5),
                4 => a *= 1.0 - nz * 0.8,
                _ => {}
            }
            let hl = (1.0 - ((dx + 0.35).powi(2) + (dy + 0.35).powi(2)) / 0.05).max(0.0) * 0.8;
            let c = c.map(|v| v + hl);
            Some([c[0] * a, c[1] * a, c[2] * a, a])
        },
        Acc::Over,
    );
    b.img = out;
    b
}

// ------------------------------------------------------------------ CC Drizzle

/// CC Drizzle's live drops this frame (buffer px: centre x, y, ring radius, amplitude);
/// `None` = no drops yet (the layer passes through).
pub fn drizzle_drops(ctx: &EffectCtx, b: &Buf) -> Option<Vec<[f32; 4]>> {
    let pr = ctx.params;
    let rate = pr.f("dripRate").max(0.0);
    let life = pr.f("longevity").max(0.01);
    let spreading = pr.f("spreading") as f32;
    let height = pr.f("rippleHeight") as f32 / 100.0;
    let seed = ctx.seed.wrapping_mul(0x165667b1);
    let t = ctx.time;
    let (lw, lh) = (ctx.layer_size[0] as f32, ctx.layer_size[1] as f32);
    let s = b.scale as f32;
    if rate <= 0.0 || t < 0.0 {
        return None;
    }
    // Drops born at k / rate with hashed positions; alive while younger than the longevity.
    let k0 = ((t - life) * rate).floor().max(0.0) as u32;
    let k1 = (t * rate).floor() as u32;
    let mut drops = Vec::new();
    for k in k0..=k1 {
        let born = k as f64 / rate;
        let age = (t - born) as f32;
        if age < 0.0 || age as f64 >= life {
            continue;
        }
        let (x, y) = b.to_px([(h(k, 1, seed) * lw) as f64, (h(k, 2, seed) * lh) as f64]);
        let radius = spreading * age * lh / 400.0 * s;
        let amp = height * (1.0 - age / life as f32) * (0.6 + 0.4 * h(k, 3, seed));
        drops.push([x as f32, y as f32, radius, amp]);
    }
    Some(drops)
}

fn drizzle(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let pr = ctx.params;
    let Some(drops) = drizzle_drops(ctx, &b) else { return b };
    let rippling = (pr.f("rippling") / 360.0).max(0.05) as f32;
    let disp = pr.f("displacement") as f32;
    let li = pr.f("light/lightIntensity") as f32 / 100.0;
    let lc = pr.color("light/lightColor");
    let lheight = pr.f("light/lightHeight") as f32 / 100.0;
    let ldir = (pr.f("light/lightDirection") as f32).to_radians();
    let ambient = pr.f("shading/ambient") as f32 / 100.0;
    let diffuse = pr.f("shading/diffuse") as f32 / 100.0;
    let specular = pr.f("shading/specular") as f32 / 100.0;
    let rough = pr.f("shading/roughness").max(0.001) as f32;
    let lh = ctx.layer_size[1] as f32;
    let s = b.scale as f32;
    let wavelen = (lh * 0.04 * s).max(2.0) / rippling.clamp(0.25, 4.0);
    let width = wavelen * (1.0 + rippling * 2.0);
    let field = |x: f32, y: f32| -> f32 {
        let mut v = 0.0;
        for &[dx, dy, r, a] in &drops {
            let d = ((x - dx).powi(2) + (y - dy).powi(2)).sqrt();
            let u = (d - r) / width;
            if u.abs() < 1.0 {
                let env = 0.5 + 0.5 * (u * std::f32::consts::PI).cos();
                v += a * env * ((d - r) / wavelen * std::f32::consts::TAU).cos();
            }
        }
        v
    };
    let src = b.img.clone();
    let (ls, lcs) = ldir.sin_cos();
    let l = {
        let v = [lcs, ls, lheight.max(0.05)];
        let n = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        [v[0] / n, v[1] / n, v[2] / n]
    };
    b.img.rows_mut().for_each(|(y, row)| {
        for (x, px) in row.iter_mut().enumerate() {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            let gx = field(fx + 1.0, fy) - field(fx - 1.0, fy);
            let gy = field(fx, fy + 1.0) - field(fx, fy - 1.0);
            if gx == 0.0 && gy == 0.0 {
                continue;
            }
            let p = src.sample_bilinear_clamped((fx - gx * disp * s * 4.0) as f64, (fy - gy * disp * s * 4.0) as f64);
            let n = {
                let v = [-gx * 6.0, -gy * 6.0, 1.0];
                let m = (v[0] * v[0] + v[1] * v[1] + 1.0).sqrt();
                [v[0] / m, v[1] / m, v[2] / m]
            };
            let nl = (n[0] * l[0] + n[1] * l[1] + n[2] * l[2]).max(0.0);
            let hz = (l[2] + 1.0) * 0.5;
            let hv = {
                let v = [l[0] * 0.5, l[1] * 0.5, hz];
                let m = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
                [v[0] / m, v[1] / m, v[2] / m]
            };
            let sp = (n[0] * hv[0] + n[1] * hv[1] + n[2] * hv[2]).max(0.0).powf(1.0 / rough) * specular * li;
            let shade = 1.0 + (ambient + diffuse * nl - diffuse * l[2]) * li;
            let a = p[3];
            *px = [p[0] * shade + sp * lc[0] * a, p[1] * shade + sp * lc[1] * a, p[2] * shade + sp * lc[2] * a, a];
        }
    });
    b
}

// ------------------------------------------------------------------ CC Star Burst

fn star_burst(ctx: &EffectCtx, b: Buf) -> Buf {
    let mut plan = star_burst_plan(ctx, &b);
    plan.resolve(&b.img);
    plan.finish(b)
}

fn star_burst_plan(ctx: &EffectCtx, b: &Buf) -> SpritePlan {
    let pr = ctx.params;
    let scatter = pr.f("scatter") as f32;
    let speed = pr.f("speed") as f32;
    let phase = pr.f("phase") as f32 / 360.0;
    let spacing = pr.f("gridSpacing").max(1.0) as f32;
    let size = pr.f("size") as f32 / 100.0;
    let blend = pr.f("blendWithOriginal") as f32 / 100.0;
    let seed = ctx.seed.wrapping_mul(0x3c6e);
    let (lw, lh) = (ctx.layer_size[0] as f32, ctx.layer_size[1] as f32);
    let s = b.scale as f32;
    let t = ctx.time as f32;
    let nx = (lw / spacing).ceil().max(1.0) as u32;
    let ny = (lh / spacing).ceil().max(1.0) as u32;
    let (cx, cy) = (lw * 0.5, lh * 0.5);
    let mut list: Vec<(f32, Sprite, Tint)> = (0..nx * ny)
        .into_par_iter()
        .map(|i| {
            let (gx, gy) = ((i % nx) as f32 * spacing + spacing * 0.5, (i / nx) as f32 * spacing + spacing * 0.5);
            let (sx, sy) = b.to_px([gx as f64, gy as f64]);
            // Depth cycles towards the viewer: z in [0, 1), perspective 1 / (1 - 0.9 z).
            let z = (h(i, 1, seed) + t * speed * 0.25 + phase).rem_euclid(1.0);
            let persp = 1.0 / (1.0 - 0.9 * z);
            let jx = hs(i, 2, seed) * scatter * 0.01 * lw * 0.5;
            let jy = hs(i, 3, seed) * scatter * 0.01 * lh * 0.5;
            let x = cx + (gx - cx + jx) * persp;
            let y = cy + (gy - cy + jy) * persp;
            let (bx, by) = b.to_px([x as f64, y as f64]);
            let r = spacing * 0.5 * size * persp * s;
            let fade = (z * 8.0).min(1.0) * ((1.0 - z) * 6.0).min(1.0);
            // The layer's colour at the grid point (none where it is clear).
            (z, Sprite::new(bx as f32, by as f32, r, [0.0, 0.0, 0.0, fade], Shape::Disc), Tint::Source { x: sx, y: sy })
        })
        .collect();
    list.sort_by(|a, b| a.0.total_cmp(&b.0));
    let (sprites, tints) = list.into_iter().map(|(_, s, t)| (s, t)).unzip();
    SpritePlan { sprites, tints, acc: Acc::Over, post: if blend > 0.0 { Post::Lerp(blend) } else { Post::Replace } }
}

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

fn norm3(v: [f32; 3]) -> [f32; 3] {
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

    pub(crate) fn step(&self, st: &mut PState, step: u64) {
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
        st.carry += self.rate / SPS;
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
/// `None` (simulate on the CPU).
fn accelerated(ctx: &EffectCtx, key: u64, phys: &Phys) -> Option<std::sync::Arc<PState>> {
    let backend = ctx.env.host?.particles()?;
    let steps = steps_at(ctx.time);
    let births = crate::psim::births(phys.rate, steps, phys.max)?;
    let req = crate::psim::SimRequest { key, steps, births: &births, system: crate::psim::ParticleSystem::Engine(phys.desc()) };
    let parts = backend.simulate(&req)?;
    Some(std::sync::Arc::new(PState {
        parts: parts.into_iter().map(|q| Particle { p: q.p, v: q.v, age: q.age, life: q.life, rnd: q.rnd, id: q.id }).collect(),
        carry: 0.0,
        next_id: births.len() as u32,
    }))
}

/// The live particles of a stepped particle effect (`ec.sim.ccparticleworld`,
/// `ec.sim.ccparticlesystems2`) at `ctx.time`: from the host's particle backend when it has
/// one (GPU particles), else from the CPU simulation. For comparing the two.
pub fn particle_state(id: &str, ctx: &EffectCtx) -> Option<Vec<crate::psim::SimParticle>> {
    let (phys, cache, salt) = match id {
        "ec.sim.ccparticleworld" => (pw_phys(ctx), &PW_CACHE, 1),
        "ec.sim.ccparticlesystems2" => (ps2_phys(ctx), &PS2_CACHE, 2),
        _ => return None,
    };
    let key = sim_key(ctx, salt);
    let st = accelerated(ctx, key, &phys).unwrap_or_else(|| simulate(cache, key, ctx.time, &phys));
    Some(st.parts.iter().map(|q| crate::psim::SimParticle { p: q.p, v: q.v, age: q.age, life: q.life, rnd: q.rnd, id: q.id }).collect())
}

/// The simulation cache key of a stepped particle effect: every parameter but CC Particle World's
/// Effect Camera, which only changes how the particles are seen (animating it doesn't restart
/// the simulation).
fn sim_key(ctx: &EffectCtx, salt: u64) -> u64 {
    params_key_except(ctx, &Buf { img: Image::new(0, 0), offset: [0.0; 2], scale: 1.0 }, salt, &[PW_EFFECT_CAMERA])
}

/// Run a particle system to layer time `t` through `cache`.
pub(crate) fn simulate(cache: &SimCache<PState>, key: u64, t: f64, phys: &Phys) -> std::sync::Arc<PState> {
    cache.run(key, steps_at(t), PState::default, |st, i| phys.step(st, i))
}

pub(crate) const PARTICLE_TYPES: &[&str] =
    &["Line", "Shaded Sphere", "Faded Sphere", "Bubble", "Star", "Motion Polygon", "Tri-Polygon", "Cube", "Textured Square"];

/// Colour/size/opacity of a particle at its age.
fn particle_look(q: &Particle, birth: [f32; 4], death: [f32; 4], bsize: f32, dsize: f32, svar: f32, max_op: f32) -> ([f32; 4], f32) {
    let u = (q.age / q.life.max(1e-4)).clamp(0.0, 1.0);
    let size = (bsize + (dsize - bsize) * u) * (1.0 + svar * (q.rnd * 2.0 - 1.0));
    let fade = (u * 20.0).min(1.0) * ((1.0 - u) * 5.0).min(1.0);
    let c = [birth[0] + (death[0] - birth[0]) * u, birth[1] + (death[1] - birth[1]) * u, birth[2] + (death[2] - birth[2]) * u, max_op * fade];
    (c, size.max(0.0))
}

fn type_sprite(kind: u32, x: f32, y: f32, r: f32, c: [f32; 4], vx: f32, vy: f32, rot: f32) -> Sprite {
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

// ------------------------------------------------------------------ CC Particle World

static PW_CACHE: SimCache<PState> = SimCache::new(6);

/// CC Particle World's physics.
fn pw_phys(ctx: &EffectCtx) -> Phys {
    let pr = ctx.params;
    Phys {
        rate: pr.f("birthRate").max(0.0) * 120.0,
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

fn particle_world(ctx: &EffectCtx, b: Buf) -> Buf {
    // #1195: with the comp's motion-blur shutter open, particles draw as motion trails: the
    // sprite fields of samples across the shutter are averaged, as Transform averages its
    // matrix samples. (The GPU compositor runs this case on the CPU: catalog::gpu_supported.)
    if let Some((angle, phase, n)) = ctx.env.shutter.filter(|(a, _, _)| *a > 0.0) {
        let n = (n.clamp(2, 64) as usize).max(2);
        let fd = 1.0 / ctx.fps().max(0.001);
        let mut acc = Image::new(b.img.width, b.img.height);
        let mut post: Option<Post> = None;
        for k in 0..n {
            let t = ctx.time + (phase + angle * k as f64 / (n - 1) as f64) / 360.0 * fd;
            let sub = EffectCtx { params: ctx.params, time: t, layer_size: ctx.layer_size, seed: ctx.seed, adjustment: ctx.adjustment, env: ctx.env };
            if let Some(plan) = particle_world_plan(&sub, &b) {
                let fx = splat(b.img.width, b.img.height, &plan.sprites, plan.acc);
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
        Some(plan) => plan.finish(b),
        None => b,
    }
}

/// CC Particle World's Extras ▸ Effect Camera parameters (id prefix).
const PW_EFFECT_CAMERA: &str = "extras/effectCamera/";
/// Effect Camera ▸ FOV's default (degrees).
const PW_FOV: f64 = 50.0;

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
fn particle_world_plan(ctx: &EffectCtx, b: &Buf) -> Option<SpritePlan> {
    let pr = ctx.params;
    let (lw, lh) = (ctx.layer_size[0] as f32, ctx.layer_size[1] as f32);
    let phys = pw_phys(ctx);
    if phys.rate <= 0.0 && ctx.time <= 0.0 {
        return None;
    }
    let key = sim_key(ctx, 1);
    let st = accelerated(ctx, key, &phys).unwrap_or_else(|| simulate(&PW_CACHE, key, ctx.time, &phys));
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
            let sp = type_sprite(kind, x, y, r, c, v[0] * lw * k * s, v[1] * lw * k * s, (q.id % 628) as f32 * 0.01 + q.age * 2.0);
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

static PS2_CACHE: SimCache<PState> = SimCache::new(6);

/// CC Particle Systems II's physics.
fn ps2_phys(ctx: &EffectCtx) -> Phys {
    let pr = ctx.params;
    let lh = ctx.layer_size[1] as f32;
    let unit = lh.max(1.0);
    let pos = pr.v2("producer/position");
    let dir = (pr.f("physics/direction") as f32).to_radians();
    Phys {
        rate: pr.f("birthRate").max(0.0) * 60.0,
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

fn particle_systems2(ctx: &EffectCtx, b: Buf) -> Buf {
    particle_systems2_plan(ctx, &b).finish(b)
}

/// CC Particle Systems II's sprites.
fn particle_systems2_plan(ctx: &EffectCtx, b: &Buf) -> SpritePlan {
    let pr = ctx.params;
    let lh = ctx.layer_size[1] as f32;
    let unit = lh.max(1.0);
    let phys = ps2_phys(ctx);
    let key = sim_key(ctx, 2);
    let st = accelerated(ctx, key, &phys).unwrap_or_else(|| simulate(&PS2_CACHE, key, ctx.time, &phys));
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
            type_sprite(kind, bx as f32, by as f32, r, c, q.v[0] * unit * s, q.v[1] * unit * s, (q.id % 628) as f32 * 0.01 + q.age * 2.0)
        })
        .collect();
    particle_sprites(sprites, pr.e("particle/transferMode"))
}

// ------------------------------------------------------------------ CC Mr. Mercury

static MERCURY_CACHE: SimCache<PState> = SimCache::new(6);

/// A CC Mr. Mercury blob (buffer px).
#[derive(Clone, Copy, Debug)]
pub struct Blob {
    pub x: f32,
    pub y: f32,
    pub r: f32,
}

/// CC Mr. Mercury's blobs this frame, from its particle simulation (for buffer geometry `b`).
pub fn mercury_blobs(ctx: &EffectCtx, b: &Buf) -> Vec<Blob> {
    let pr = ctx.params;
    let lh = ctx.layer_size[1] as f32;
    let unit = lh.max(1.0);
    let pos = pr.v2("producer");
    let dir = (pr.f("direction") as f32).to_radians();
    let phys = Phys {
        rate: pr.f("birthRate").max(0.0) * 10.0,
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
    };
    let st = simulate(&MERCURY_CACHE, params_key(ctx, &Buf { img: Image::new(0, 0), offset: [0.0; 2], scale: 1.0 }, 3), ctx.time, &phys);
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

fn mr_mercury(ctx: &EffectCtx, mut b: Buf) -> Buf {
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

// ------------------------------------------------------------------ specs

pub fn specs() -> Vec<EffectSpec> {
    let transfer = || popup(&["Composite", "Lighten"]);
    vec![
        spec(
            "ec.sim.ccrainfall",
            "CC Rainfall",
            vec![
                p("drops", "Drops", num(5000.0), slider(0.0, 100_000.0, 0.0, 20_000.0, 0)),
                p("size", "Size", num(1.0), slider(0.0, 50.0, 0.0, 10.0, 2)),
                p("sceneDepth", "Scene Depth", num(5000.0), slider(0.0, 20_000.0, 0.0, 10_000.0, 0)),
                p("speed", "Speed", num(4000.0), slider(0.0, 20_000.0, 0.0, 10_000.0, 0)),
                p("wind", "Wind", num(0.0), slider(-10_000.0, 10_000.0, -2000.0, 2000.0, 1)),
                p("windVariation", "Variation % (Wind)", num(0.0), pct()),
                p("spread", "Spread", num(0.0), slider(0.0, 1000.0, 0.0, 100.0, 1)),
                p("color", "Color", col(0.8, 0.8, 0.8), ParamUi::Color),
                p("opacity", "Opacity", num(25.0), pct()),
                p("transferMode", "Transfer Mode", Value::Enum(1), transfer()),
                p("compositeWithOriginal", "Composite With Original", Value::Bool(true), ParamUi::Checkbox),
                p("extras/appearance", "Appearance", Value::Enum(0), popup(&["Refracting", "Soft Solid"])),
                p("extras/offset", "Offset", pt(0.5, 0.5), ParamUi::Point),
                p("extras/randomSeed", "Random Seed", num(0.0), slider(0.0, 10_000.0, 0.0, 1000.0, 0)),
            ],
            rainfall,
        ),
        spec(
            "ec.sim.ccsnowfall",
            "CC Snowfall",
            vec![
                p("flakes", "Flakes", num(10_000.0), slider(0.0, 200_000.0, 0.0, 50_000.0, 0)),
                p("size", "Size", num(3.0), slider(0.0, 50.0, 0.0, 20.0, 2)),
                p("sizeVariation", "Variation % (Size)", num(50.0), pct()),
                p("sceneDepth", "Scene Depth", num(5000.0), slider(0.0, 20_000.0, 0.0, 10_000.0, 0)),
                p("speed", "Speed", num(100.0), slider(0.0, 5000.0, 0.0, 1000.0, 1)),
                p("speedVariation", "Variation % (Speed)", num(50.0), pct()),
                p("wind", "Wind", num(25.0), slider(-5000.0, 5000.0, -500.0, 500.0, 1)),
                p("windVariation", "Variation % (Wind)", num(50.0), pct()),
                p("spread", "Spread", num(0.0), slider(0.0, 1000.0, 0.0, 100.0, 1)),
                p("wiggle/wiggleAmount", "Amount", num(2.0), slider(0.0, 100.0, 0.0, 20.0, 2)),
                p("wiggle/wiggleAmountVariation", "Variation % (Amount)", num(50.0), pct()),
                p("wiggle/wiggleFrequency", "Frequency", num(1.0), slider(0.0, 20.0, 0.0, 5.0, 2)),
                p("wiggle/wiggleFrequencyVariation", "Variation % (Frequency)", num(50.0), pct()),
                p("color", "Color", col(1.0, 1.0, 1.0), ParamUi::Color),
                p("opacity", "Opacity", num(100.0), pct()),
                p("transferMode", "Transfer Mode", Value::Enum(0), transfer()),
                p("compositeWithOriginal", "Composite With Original", Value::Bool(true), ParamUi::Checkbox),
                p("extras/offset", "Offset", pt(0.5, 0.5), ParamUi::Point),
                p("extras/randomSeed", "Random Seed", num(0.0), slider(0.0, 10_000.0, 0.0, 1000.0, 0)),
            ],
            snowfall,
        ),
        spec(
            "ec.sim.ccbubbles",
            "CC Bubbles",
            vec![
                p("bubbleAmount", "Bubble Amount", num(100.0), slider(0.0, 10_000.0, 0.0, 1000.0, 0)),
                p("bubbleSpeed", "Bubble Speed", num(0.5), slider(-10.0, 10.0, -2.0, 2.0, 2)),
                p("wobbleAmplitude", "Wobble Amplitude", num(5.0), slider(0.0, 500.0, 0.0, 100.0, 1)),
                p("wobbleFrequency", "Wobble Frequency", num(1.0), slider(0.0, 50.0, 0.0, 10.0, 2)),
                p("bubbleSize", "Bubble Size", num(1.0), slider(0.0, 20.0, 0.0, 5.0, 2)),
                p("reflectionType", "Reflection Type", Value::Enum(0), popup(&["Liquid", "Metal"])),
                p("shadingType", "Shading Type", Value::Enum(0), popup(&["None", "Lighten", "Darken", "Fade Inwards", "Fade Outwards"])),
            ],
            bubbles,
        ),
        spec(
            "ec.sim.ccdrizzle",
            "CC Drizzle",
            vec![
                p("dripRate", "Drip Rate", num(10.0), slider(0.0, 200.0, 0.0, 50.0, 2)),
                p("longevity", "Longevity (sec)", num(1.0), slider(0.0, 30.0, 0.0, 5.0, 2)),
                p("rippling", "Rippling", num(360.0), ParamUi::Angle),
                p("displacement", "Displacement", num(5.0), slider(0.0, 100.0, 0.0, 30.0, 2)),
                p("rippleHeight", "Ripple Height", num(30.0), slider(0.0, 200.0, 0.0, 100.0, 1)),
                p("spreading", "Spreading", num(100.0), slider(0.0, 1000.0, 0.0, 300.0, 1)),
                p("light/lightIntensity", "Light Intensity", num(100.0), slider(0.0, 400.0, 0.0, 200.0, 1)),
                p("light/lightColor", "Light Color", col(1.0, 1.0, 1.0), ParamUi::Color),
                p("light/lightHeight", "Light Height", num(25.0), slider(-100.0, 100.0, -100.0, 100.0, 1)),
                p("light/lightDirection", "Light Direction", num(-45.0), ParamUi::Angle),
                p("shading/ambient", "Ambient", num(0.0), slider(-200.0, 200.0, -100.0, 100.0, 1)),
                p("shading/diffuse", "Diffuse", num(0.0), slider(0.0, 200.0, 0.0, 100.0, 1)),
                p("shading/specular", "Specular", num(20.0), slider(0.0, 200.0, 0.0, 100.0, 1)),
                p("shading/roughness", "Roughness", num(0.05), slider(0.001, 1.0, 0.001, 0.5, 3)),
            ],
            drizzle,
        ),
        spec(
            "ec.sim.ccstarburst",
            "CC Star Burst",
            vec![
                p("scatter", "Scatter", num(100.0), slider(0.0, 1000.0, 0.0, 300.0, 1)),
                p("speed", "Speed", num(1.0), slider(-10.0, 10.0, -4.0, 4.0, 2)),
                p("phase", "Phase", num(0.0), ParamUi::Angle),
                p("gridSpacing", "Grid Spacing", num(2.0), slider(1.0, 100.0, 1.0, 20.0, 0)),
                p("size", "Size", num(100.0), slider(0.0, 1000.0, 0.0, 300.0, 1)),
                p("blendWithOriginal", "Blend w. Original", num(0.0), pct()),
            ],
            star_burst,
        ),
        spec(
            "ec.sim.ccparticleworld",
            "CC Particle World",
            vec![
                p("birthRate", "Birth Rate", num(0.5), slider(0.0, 100.0, 0.0, 10.0, 2)),
                p("longevity", "Longevity (sec)", num(1.0), slider(0.0, 100.0, 0.0, 10.0, 2)),
                p("producer/producerX", "Position X", num(0.0), slider(-10.0, 10.0, -1.0, 1.0, 2)),
                p("producer/producerY", "Position Y", num(0.0), slider(-10.0, 10.0, -1.0, 1.0, 2)),
                p("producer/producerZ", "Position Z", num(0.0), slider(-10.0, 10.0, -1.0, 1.0, 2)),
                p("producer/radiusX", "Radius X", num(0.025), slider(0.0, 10.0, 0.0, 1.0, 3)),
                p("producer/radiusY", "Radius Y", num(0.025), slider(0.0, 10.0, 0.0, 1.0, 3)),
                p("producer/radiusZ", "Radius Z", num(0.025), slider(0.0, 10.0, 0.0, 1.0, 3)),
                p("physics/animation", "Animation", Value::Enum(0), popup(PW_ANIMS)),
                p("physics/velocity", "Velocity", num(1.0), slider(-10.0, 10.0, 0.0, 5.0, 2)),
                p("physics/inheritVelocity", "Inherit Velocity %", num(0.0), slider(-100.0, 100.0, -100.0, 100.0, 1)),
                p("physics/gravity", "Gravity", num(0.5), slider(-10.0, 10.0, -2.0, 2.0, 3)),
                p("physics/resistance", "Resistance", num(0.0), slider(0.0, 20.0, 0.0, 5.0, 2)),
                p("physics/extra", "Extra", num(1.0), slider(0.0, 10.0, 0.0, 2.0, 2)),
                p("physics/extraAngle", "Extra Angle", num(360.0), ParamUi::Angle),
                p("physics/directionAxis/axisX", "Axis X", num(0.0), slider(-1.0, 1.0, -1.0, 1.0, 2)),
                p("physics/directionAxis/axisY", "Axis Y", num(-1.0), slider(-1.0, 1.0, -1.0, 1.0, 2)),
                p("physics/directionAxis/axisZ", "Axis Z", num(0.0), slider(-1.0, 1.0, -1.0, 1.0, 2)),
                p("physics/gravityVector/gravityX", "Gravity X", num(0.0), slider(-1.0, 1.0, -1.0, 1.0, 2)),
                p("physics/gravityVector/gravityY", "Gravity Y", num(1.0), slider(-1.0, 1.0, -1.0, 1.0, 2)),
                p("physics/gravityVector/gravityZ", "Gravity Z", num(0.0), slider(-1.0, 1.0, -1.0, 1.0, 2)),
                p("particle/particleType", "Particle Type", Value::Enum(0), popup(PARTICLE_TYPES)),
                p("particle/birthSize", "Birth Size", num(0.2), slider(0.0, 10.0, 0.0, 1.0, 3)),
                p("particle/deathSize", "Death Size", num(0.2), slider(0.0, 10.0, 0.0, 1.0, 3)),
                p("particle/sizeVariation", "Size Variation", num(0.0), pct()),
                p("particle/maxOpacity", "Max Opacity", num(75.0), pct()),
                p("particle/birthColor", "Birth Color", col(1.0, 1.0, 0.0), ParamUi::Color),
                p("particle/deathColor", "Death Color", col(0.6, 0.0, 0.0), ParamUi::Color),
                p("particle/transferMode", "Transfer Mode", Value::Enum(0), popup(&["Composite", "Screen", "Add", "Black Matte"])),
                p("extras/effectCamera/distance", "Distance", num(2.0), slider(0.0, 100.0, 0.0, 10.0, 2)),
                p("extras/effectCamera/rotationX", "Rotation X", num(0.0), ParamUi::Angle),
                p("extras/effectCamera/rotationY", "Rotation Y", num(0.0), ParamUi::Angle),
                p("extras/effectCamera/rotationZ", "Rotation Z", num(0.0), ParamUi::Angle),
                p("extras/effectCamera/fov", "FOV", num(PW_FOV), slider(1.0, 179.0, 10.0, 120.0, 1)),
                p("extras/randomSeed", "Random Seed", num(0.0), slider(0.0, 10_000.0, 0.0, 1000.0, 0)),
            ],
            particle_world,
        ),
        spec(
            "ec.sim.ccparticlesystems2",
            "CC Particle Systems II",
            vec![
                p("birthRate", "Birth Rate", num(2.0), slider(0.0, 100.0, 0.0, 20.0, 2)),
                p("longevity", "Longevity (sec)", num(1.0), slider(0.0, 100.0, 0.0, 10.0, 2)),
                p("producer/position", "Position", pt(0.5, 0.5), ParamUi::Point),
                p("producer/radiusX", "Radius X", num(0.0), slider(0.0, 2000.0, 0.0, 200.0, 1)),
                p("producer/radiusY", "Radius Y", num(0.0), slider(0.0, 2000.0, 0.0, 200.0, 1)),
                p("physics/animation", "Animation", Value::Enum(0), popup(PW_ANIMS)),
                p("physics/velocity", "Velocity", num(2.0), slider(-20.0, 20.0, 0.0, 10.0, 2)),
                p("physics/inheritVelocity", "Inherit Velocity %", num(0.0), slider(-100.0, 100.0, -100.0, 100.0, 1)),
                p("physics/gravity", "Gravity", num(1.0), slider(-20.0, 20.0, -5.0, 5.0, 2)),
                p("physics/resistance", "Resistance", num(0.0), slider(0.0, 20.0, 0.0, 5.0, 2)),
                p("physics/direction", "Direction", num(0.0), ParamUi::Angle),
                p("physics/extra", "Extra", num(1.0), slider(0.0, 10.0, 0.0, 2.0, 2)),
                p("particle/particleType", "Particle Type", Value::Enum(0), popup(PARTICLE_TYPES)),
                p("particle/birthSize", "Birth Size", num(0.5), slider(0.0, 20.0, 0.0, 5.0, 2)),
                p("particle/deathSize", "Death Size", num(1.5), slider(0.0, 20.0, 0.0, 5.0, 2)),
                p("particle/sizeVariation", "Size Variation", num(0.0), pct()),
                p("particle/maxOpacity", "Max Opacity", num(75.0), pct()),
                p("particle/birthColor", "Birth Color", col(1.0, 1.0, 0.0), ParamUi::Color),
                p("particle/deathColor", "Death Color", col(0.6, 0.0, 0.0), ParamUi::Color),
                p("particle/transferMode", "Transfer Mode", Value::Enum(0), popup(&["Composite", "Screen", "Add", "Black Matte"])),
                p("randomSeed", "Random Seed", num(0.0), slider(0.0, 10_000.0, 0.0, 1000.0, 0)),
            ],
            particle_systems2,
        ),
        spec(
            "ec.sim.ccmrmercury",
            "CC Mr. Mercury",
            vec![
                p("radiusX", "Radius X", num(50.0), slider(0.0, 2000.0, 0.0, 300.0, 1)),
                p("radiusY", "Radius Y", num(50.0), slider(0.0, 2000.0, 0.0, 300.0, 1)),
                p("producer", "Producer", pt(0.5, 0.5), ParamUi::Point),
                p("direction", "Direction", num(0.0), ParamUi::Angle),
                p("velocity", "Velocity", num(1.0), slider(-20.0, 20.0, 0.0, 5.0, 2)),
                p("birthRate", "Birth Rate", num(1.0), slider(0.0, 20.0, 0.0, 5.0, 2)),
                p("longevity", "Longevity (sec)", num(2.0), slider(0.0, 100.0, 0.0, 10.0, 2)),
                p("gravity", "Gravity", num(0.5), slider(-20.0, 20.0, -5.0, 5.0, 2)),
                p("resistance", "Resistance", num(0.0), slider(0.0, 20.0, 0.0, 5.0, 2)),
                p("extra", "Extra", num(1.0), slider(0.0, 10.0, 0.0, 2.0, 2)),
                p("animation", "Animation", Value::Enum(0), popup(PW_ANIMS)),
                p("blobInfluence", "Blob Influence", num(100.0), slider(0.0, 400.0, 0.0, 200.0, 1)),
                p("blobBirthSize", "Blob Birth Size", num(0.5), slider(0.0, 10.0, 0.0, 2.0, 2)),
                p("blobDeathSize", "Blob Death Size", num(0.25), slider(0.0, 10.0, 0.0, 2.0, 2)),
                p("light/lightIntensity", "Light Intensity", num(100.0), slider(0.0, 400.0, 0.0, 200.0, 1)),
                p("light/lightColor", "Light Color", col(1.0, 1.0, 1.0), ParamUi::Color),
                p("light/lightHeight", "Light Height", num(50.0), slider(-100.0, 100.0, -100.0, 100.0, 1)),
                p("light/lightDirection", "Light Direction", num(-45.0), ParamUi::Angle),
                p("shading/ambient", "Ambient", num(0.0), slider(-200.0, 200.0, -100.0, 100.0, 1)),
                p("shading/diffuse", "Diffuse", num(75.0), slider(0.0, 200.0, 0.0, 100.0, 1)),
                p("shading/specular", "Specular", num(50.0), slider(0.0, 200.0, 0.0, 100.0, 1)),
                p("shading/roughness", "Roughness", num(0.05), slider(0.001, 1.0, 0.001, 0.5, 3)),
                p("shading/metal", "Metal", num(100.0), pct()),
            ],
            mr_mercury,
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EffectEnv, run_fx};

    fn grey(w: u32, hh: u32) -> Image {
        Image::filled(w, hh, [0.2, 0.2, 0.2, 1.0])
    }

    fn ramp(w: u32, hh: u32) -> Image {
        let mut img = Image::new(w, hh);
        for y in 0..hh {
            for x in 0..w {
                img.set(x, y, [x as f32 / w as f32, y as f32 / hh as f32, 0.4, 1.0]);
            }
        }
        img
    }

    fn run(id: &str, vals: &[(&str, Value)], img: Image, t: f64) -> Image {
        run_fx(id, vals, img, t, EffectEnv::default()).img
    }

    fn diff(a: &Image, b: &Image) -> f32 {
        a.data.iter().zip(&b.data).map(|(p, q)| (0..4).map(|k| (p[k] - q[k]).abs()).sum::<f32>()).sum()
    }

    /// Centre of mass (luminance-weighted difference from `base`), `[-1, -1]` when equal.
    fn mass(img: &Image, base: &Image) -> [f32; 2] {
        let (mut m, mut sx, mut sy) = (0.0, 0.0, 0.0);
        for y in 0..img.height {
            for x in 0..img.width {
                let i = img.idx(x, y);
                let d: f32 = (0..3).map(|k| (img.data[i][k] - base.data[i][k]).abs()).sum();
                m += d;
                sx += d * x as f32;
                sy += d * y as f32;
            }
        }
        if m > 0.0 { [sx / m, sy / m] } else { [-1.0; 2] }
    }

    /// [`mass`] in y.
    fn mass_y(img: &Image, base: &Image) -> f32 {
        mass(img, base)[1]
    }

    #[test]
    fn rasteriser_over_and_add() {
        let s = [Sprite::new(5.0, 5.0, 2.0, [1.0, 0.0, 0.0, 1.0], Shape::Disc)];
        let img = splat(10, 40, &s, Acc::Over);
        assert!(img.get(5, 5)[0] > 0.99 && img.get(9, 30)[3] == 0.0);
        let two = [s[0], s[0]];
        let add = splat(10, 40, &two, Acc::Add);
        assert!(add.get(5, 5)[0] > 1.9);
        // Items spanning band boundaries are drawn in every band.
        let tall = [Sprite::new(5.0, 16.0, 6.0, [0.0, 1.0, 0.0, 1.0], Shape::Square)];
        let t = splat(10, 40, &tall, Acc::Over);
        assert!(t.get(5, 13)[1] > 0.9 && t.get(5, 18)[1] > 0.9);
    }

    #[test]
    fn rainfall_deterministic_and_falls() {
        let vals = [("drops", num(200.0)), ("opacity", num(100.0)), ("extras/appearance", Value::Enum(1)), ("color", col(1.0, 1.0, 1.0))];
        let a = run("ec.sim.ccrainfall", &vals, grey(40, 40), 0.5);
        let b = run("ec.sim.ccrainfall", &vals, grey(40, 40), 0.5);
        assert_eq!(a, b);
        assert!(diff(&a, &grey(40, 40)) > 1.0, "drops visible");
        let none = run("ec.sim.ccrainfall", &[("drops", num(0.0))], grey(40, 40), 0.5);
        assert_eq!(none, grey(40, 40));
        let later = run("ec.sim.ccrainfall", &vals, grey(40, 40), 0.52);
        assert!(diff(&a, &later) > 0.1, "rain moves");
    }

    #[test]
    fn snowfall_deterministic_and_moves() {
        let vals = [("flakes", num(300.0))];
        let a = run("ec.sim.ccsnowfall", &vals, grey(40, 40), 1.0);
        assert_eq!(a, run("ec.sim.ccsnowfall", &vals, grey(40, 40), 1.0));
        assert!(diff(&a, &grey(40, 40)) > 1.0);
        assert!(diff(&a, &run("ec.sim.ccsnowfall", &vals, grey(40, 40), 1.2)) > 0.1);
        let off = run("ec.sim.ccsnowfall", &[("flakes", num(300.0)), ("compositeWithOriginal", Value::Bool(false))], grey(40, 40), 1.0);
        assert!(off.data.iter().any(|p| p[3] == 0.0), "without original the background is transparent");
    }

    #[test]
    fn bubbles_rise() {
        let vals = [("bubbleAmount", num(30.0)), ("bubbleSize", num(3.0))];
        let a = run("ec.sim.ccbubbles", &vals, ramp(40, 40), 0.3);
        assert_eq!(a, run("ec.sim.ccbubbles", &vals, ramp(40, 40), 0.3));
        assert!(a.data.iter().any(|p| p[3] > 0.5));
        let none = run("ec.sim.ccbubbles", &[("bubbleAmount", num(0.0))], ramp(40, 40), 0.3);
        assert!(none.data.iter().all(|p| p[3] == 0.0), "bubbles replace the layer");
        // A single slow bubble moves up over a short time.
        let one = [("bubbleAmount", num(1.0)), ("bubbleSize", num(4.0)), ("wobbleAmplitude", num(0.0)), ("bubbleSpeed", num(0.2))];
        let blank = Image::new(40, 40);
        let y0 = mass_y(&run("ec.sim.ccbubbles", &one, ramp(40, 40), 0.0), &blank);
        let y1 = mass_y(&run("ec.sim.ccbubbles", &one, ramp(40, 40), 0.1), &blank);
        assert!(y0 >= 0.0 && y1 >= 0.0 && (y1 < y0 || y0 < 8.0), "{y0} -> {y1}");
    }

    #[test]
    fn drizzle_ripples() {
        let a = run("ec.sim.ccdrizzle", &[], ramp(48, 48), 0.6);
        assert_eq!(a, run("ec.sim.ccdrizzle", &[], ramp(48, 48), 0.6));
        assert!(diff(&a, &ramp(48, 48)) > 0.1);
        let none = run("ec.sim.ccdrizzle", &[("dripRate", num(0.0))], ramp(48, 48), 0.6);
        assert_eq!(none, ramp(48, 48));
    }

    #[test]
    fn star_burst_moves_stars() {
        let a = run("ec.sim.ccstarburst", &[("gridSpacing", num(4.0))], ramp(40, 40), 0.5);
        assert_eq!(a, run("ec.sim.ccstarburst", &[("gridSpacing", num(4.0))], ramp(40, 40), 0.5));
        assert!(a.data.iter().any(|p| p[3] > 0.0));
        let b = run("ec.sim.ccstarburst", &[("gridSpacing", num(4.0))], ramp(40, 40), 0.9);
        assert!(diff(&a, &b) > 0.5);
        let full = run("ec.sim.ccstarburst", &[("blendWithOriginal", num(100.0))], ramp(40, 40), 0.5);
        assert!(diff(&full, &ramp(40, 40)) < 1e-3);
    }

    fn pw_vals(seed: f64) -> Vec<(&'static str, Value)> {
        vec![
            ("birthRate", num(1.0)),
            ("extras/randomSeed", num(seed)),
            ("particle/particleType", Value::Enum(2)),
            ("particle/birthSize", num(0.3)),
            ("particle/deathSize", num(0.3)),
        ]
    }

    /// #1195: with the comp's shutter open, the particles smear across it: the average of the
    /// sampled sprite fields differs from the sharp frame, touches more pixels and keeps the
    /// alpha energy.
    #[test]
    fn particle_world_motion_blur_trails() {
        let v = pw_vals(21.0);
        let blank = Image::new(40, 40);
        let sharp = run("ec.sim.ccparticleworld", &v, blank.clone(), 2.0);
        let blurred = crate::run_fx(
            "ec.sim.ccparticleworld",
            &v,
            blank.clone(),
            2.0,
            EffectEnv { frame_rate: 10.0, shutter: Some((360.0, -180.0, 8)), ..Default::default() },
        )
        .img;
        assert!(sharp.data.iter().any(|p| p[3] > 0.0), "sharp frame has particles");
        assert_ne!(sharp.data, blurred.data, "the shutter smears the particles");
        let lit = |img: &Image| img.data.iter().filter(|p| p[3] > 0.005).count();
        let sum = |img: &Image| img.data.iter().map(|p| p[3]).sum::<f32>();
        assert!(lit(&blurred) > lit(&sharp), "trails light more pixels: {} vs {}", lit(&blurred), lit(&sharp));
        let (sa, ba) = (sum(&sharp), sum(&blurred));
        assert!((ba - sa).abs() < sa * 0.6 + 1.0, "alpha energy roughly kept: {ba} vs {sa}");
    }

    #[test]
    fn particle_world_seek_consistent() {
        let v = pw_vals(11.0);
        let direct = run("ec.sim.ccparticleworld", &v, Image::new(40, 40), 2.0);
        let v2 = pw_vals(12.0);
        let _ = run("ec.sim.ccparticleworld", &v2, Image::new(40, 40), 1.0);
        let stepped = run("ec.sim.ccparticleworld", &v2, Image::new(40, 40), 2.0);
        let direct2 = run("ec.sim.ccparticleworld", &v2, Image::new(40, 40), 2.0);
        assert_eq!(stepped, direct2);
        assert!(direct.data.iter().any(|p| p[3] > 0.0), "particles alive");
        // Before birth nothing is drawn.
        let t0 = run("ec.sim.ccparticleworld", &v, Image::new(40, 40), 0.0);
        assert!(t0.data.iter().all(|p| p[3] == 0.0));
    }

    #[test]
    fn engine_resume_equals_from_scratch() {
        for anim in 0..PW_ANIMS.len() as u32 {
            let phys = Phys {
                rate: 90.0,
                life: 1.5,
                producer: [0.0; 3],
                radius: [0.05; 3],
                anim,
                speed: 0.5,
                gravity: [0.0, 0.5, 0.0],
                resistance: 0.2,
                axis: [0.0, -1.0, 0.0],
                extra: 1.0,
                extra_angle: 60.0,
                seed: 9,
                max: 10_000,
                flat: anim % 2 == 0,
            };
            let a: SimCache<PState> = SimCache::new(2);
            let b: SimCache<PState> = SimCache::new(2);
            let direct = simulate(&a, 1, 2.0, &phys);
            let _ = simulate(&b, 1, 1.0, &phys);
            let _ = simulate(&b, 1, 0.37, &phys);
            let resumed = simulate(&b, 1, 2.0, &phys);
            assert_eq!(direct.parts.len(), resumed.parts.len(), "anim {anim}");
            assert!(direct.parts.iter().zip(&resumed.parts).all(|(p, q)| p.p == q.p && p.v == q.v && p.id == q.id), "anim {anim}");
            assert!(!direct.parts.is_empty());
        }
    }

    #[test]
    fn particle_world_gravity_pulls_down() {
        let base = |g: f64| {
            vec![
                ("birthRate", num(2.0)),
                ("physics/velocity", num(0.2)),
                ("physics/gravity", num(g)),
                ("particle/particleType", Value::Enum(2)),
                ("particle/birthSize", num(0.2)),
                ("particle/deathSize", num(0.2)),
                ("longevity", num(3.0)),
            ]
        };
        let blank = Image::new(48, 48);
        let up = mass_y(&run("ec.sim.ccparticleworld", &base(0.0), blank.clone(), 1.0), &blank);
        let down = mass_y(&run("ec.sim.ccparticleworld", &base(2.0), blank.clone(), 1.0), &blank);
        assert!(down > up + 2.0, "{up} {down}");
    }

    /// A comp whose camera is `camera` ([`crate::CompScene::camera`]); `layer`: a camera layer
    /// (else the comp's default view).
    struct Comp {
        camera: [[f64; 4]; 3],
        layer: bool,
    }
    impl crate::EffectHost for Comp {
        fn layer(&self, _: u64, _: bool) -> Option<crate::LayerPixels> {
            None
        }
        fn audio(&self, _: u64, _: f64, _: usize, _: u32) -> Option<Vec<f32>> {
            None
        }
        fn comp_scene(&self) -> Option<crate::CompScene> {
            Some(crate::CompScene { camera: Some(self.camera), camera_layer: self.layer, light: None })
        }
    }

    /// A camera `d` px in front of layer point `c` looking at it, moved right by `dx` px and
    /// zoomed by `zoom` (1 = identity on the layer plane): `c + zoom·d·(p − c − dx) / (z + d)`.
    fn cam(c: [f64; 2], d: f64, dx: f64, zoom: f64) -> [[f64; 4]; 3] {
        let k = zoom * d;
        [[k, 0.0, c[0], d * c[0] - k * (c[0] + dx)], [0.0, k, c[1], d * c[1] - k * c[1]], [0.0, 0.0, 1.0, d]]
    }

    fn pw_in(comp: &Comp, vals: &[(&str, Value)]) -> Image {
        run_fx("ec.sim.ccparticleworld", vals, Image::new(48, 48), 1.0, EffectEnv { host: Some(comp), ..Default::default() }).img
    }

    fn finite(img: &Image) -> bool {
        img.data.iter().all(|p| p.iter().all(|c| c.is_finite()))
    }

    /// #397: with a camera layer in the comp, the particles are seen through it.
    #[test]
    fn particle_world_follows_the_comp_camera() {
        let v = pw_vals(3.0);
        let blank = Image::new(48, 48);
        let here = Comp { camera: cam([24.0, 24.0], 96.0, 0.0, 1.0), layer: true };
        let base = pw_in(&here, &v);
        assert!(base.data.iter().any(|p| p[3] > 0.0), "particles alive");
        // Moving the camera right moves the particles left.
        let moved = pw_in(&Comp { camera: cam([24.0, 24.0], 96.0, 8.0, 1.0), ..here }, &v);
        let (a, b) = (mass(&base, &blank), mass(&moved, &blank));
        assert!(b[0] < a[0] - 4.0 && (b[1] - a[1]).abs() < 2.0, "{a:?} -> {b:?}");
        // Zooming in spreads them out.
        let zoomed = pw_in(&Comp { camera: cam([24.0, 24.0], 96.0, 0.0, 2.0), ..here }, &v);
        let cover = |i: &Image| i.data.iter().filter(|p| p[3] > 0.01).count();
        assert!(cover(&zoomed) > cover(&base), "{} {}", cover(&zoomed), cover(&base));
        // The Effect Camera is ignored then.
        let mut turned = v.clone();
        turned.push(("extras/effectCamera/rotationY", num(60.0)));
        assert_eq!(pw_in(&here, &turned), base);
        // The comp's default view (no camera layer) leaves it to the Effect Camera.
        let default_view = Comp { layer: false, ..here };
        assert_eq!(pw_in(&default_view, &v), run("ec.sim.ccparticleworld", &v, blank.clone(), 1.0));
        // A camera facing away, or a degenerate one, draws nothing and never NaN.
        for camera in [[[96.0, 0.0, 24.0, 0.0], [0.0, 96.0, 24.0, 0.0], [0.0, 0.0, -1.0, -96.0]], [[0.0; 4]; 3], [[f64::NAN; 4]; 3]] {
            assert_eq!(pw_in(&Comp { camera, layer: true }, &v), blank);
        }
        // A camera among the particles draws the ones in front of it.
        let inside = pw_in(&Comp { camera: cam([24.0, 24.0], 1.0, 0.0, 1.0), ..here }, &v);
        assert!(finite(&inside));
    }

    /// Extras ▸ Effect Camera: the defaults are the fixed camera Particle World always had.
    #[test]
    fn particle_world_effect_camera() {
        let v = pw_vals(5.0);
        let base = run("ec.sim.ccparticleworld", &v, Image::new(48, 48), 1.0);
        let with = |extra: &[(&'static str, Value)]| {
            let mut w = v.clone();
            w.extend_from_slice(extra);
            run("ec.sim.ccparticleworld", &w, Image::new(48, 48), 1.0)
        };
        // Missing parameters (projects saved before) read as the defaults.
        let spec = crate::find("ec.sim.ccparticleworld").unwrap();
        let mut params = crate::Params { values: spec.params.iter().map(|p| (p.id.to_string(), p.default.clone())).collect() };
        params.values.retain(|k, _| !k.starts_with(PW_EFFECT_CAMERA));
        for (k, val) in &v {
            params.values.insert(k.to_string(), val.clone());
        }
        let ctx = EffectCtx { params: &params, time: 1.0, layer_size: [48.0, 48.0], seed: 1, adjustment: false, env: EffectEnv::default() };
        assert_eq!(crate::apply(spec, &ctx, Buf { img: Image::new(48, 48), offset: [0.0; 2], scale: 1.0 }).img, base);
        assert_eq!(with(&[("extras/effectCamera/fov", num(f64::NAN))]), base);
        // Turning or pulling back the camera changes the view.
        let turned = with(&[("extras/effectCamera/rotationY", num(60.0))]);
        assert!(diff(&turned, &base) > 1.0);
        let cover = |i: &Image| i.data.iter().filter(|p| p[3] > 0.01).count();
        let far = with(&[("extras/effectCamera/distance", num(4.0))]);
        assert!(cover(&far) < cover(&base), "{} {}", cover(&far), cover(&base));
        assert!(diff(&with(&[("extras/effectCamera/fov", num(120.0))]), &base) > 0.1);
        // Extremes never panic or produce NaN.
        for extra in [
            ("extras/effectCamera/distance", num(0.0)),
            ("extras/effectCamera/distance", num(-5.0)),
            ("extras/effectCamera/fov", num(0.0)),
            ("extras/effectCamera/fov", num(180.0)),
            ("extras/effectCamera/rotationX", num(1e12)),
        ] {
            assert!(finite(&with(std::slice::from_ref(&extra))), "{extra:?}");
        }
        // Moving the camera doesn't restart the simulation.
        let key = |pr: &crate::Params| {
            sim_key(&EffectCtx { params: pr, time: 1.0, layer_size: [48.0, 48.0], seed: 1, adjustment: false, env: EffectEnv::default() }, 1)
        };
        let mut moved = params.clone();
        moved.values.insert("extras/effectCamera/rotationZ".into(), num(30.0));
        assert_eq!(key(&params), key(&moved));
        moved.values.insert("extras/randomSeed".into(), num(9.0));
        assert_ne!(key(&params), key(&moved));
    }

    #[test]
    fn particle_systems2_seek_and_direction() {
        let v = vec![
            ("producer/position", pt(24.0, 24.0)),
            ("birthRate", num(3.0)),
            ("physics/gravity", num(0.0)),
            ("physics/velocity", num(1.0)),
            ("physics/animation", Value::Enum(1)),
        ];
        let _ = run("ec.sim.ccparticlesystems2", &v, Image::new(48, 48), 0.5);
        let a = run("ec.sim.ccparticlesystems2", &v, Image::new(48, 48), 1.0);
        let mut v2 = v.clone();
        v2.push(("randomSeed", num(0.0)));
        assert_eq!(a, run("ec.sim.ccparticlesystems2", &v2, Image::new(48, 48), 1.0));
        // Direction 0° (up): particles above the producer.
        let blank = Image::new(48, 48);
        let y = mass_y(&a, &blank);
        assert!((0.0..24.0).contains(&y), "{y}");
        // The particles replace the layer: with none born it is empty.
        let none = run("ec.sim.ccparticlesystems2", &[("birthRate", num(0.0))], grey(20, 20), 1.0);
        assert_eq!(none, Image::new(20, 20));
    }

    /// CC Particle World and CC Particle Systems II replace their layer with the particles, in
    /// every Transfer Mode (which only sets how particles meet each other). SceneYard frames from
    /// After Effects 2026 show no trace of the source solid: "Task Preparation UI" (black solids
    /// under fireworks hid a white page), "Burnout matchstick" (a red rectangle around the
    /// flame), "Documentary Man Falling" (a black block over the light).
    #[test]
    fn cc_particles_replace_their_layer() {
        let red = || {
            let mut img = Image::new(48, 48);
            img.data.iter_mut().for_each(|p| *p = [0.7, 0.1, 0.1, 1.0]);
            img
        };
        for id in ["ec.sim.ccparticlesystems2", "ec.sim.ccparticleworld"] {
            for mode in 0..4 {
                let v = vec![("birthRate", num(3.0)), ("particle/transferMode", Value::Enum(mode))];
                let _ = run(id, &v, red(), 0.5);
                let out = run(id, &v, red(), 1.0);
                // No source left: the corners, far from the producer, are clear.
                for (x, y) in [(0, 0), (47, 0), (0, 47), (47, 47)] {
                    assert!(out.get(x, y)[3] < 1e-6, "{id} mode {mode} ({x},{y}): {:?}", out.get(x, y));
                }
                assert!(out.data.iter().any(|p| p[3] > 0.05), "{id} mode {mode}: particles drawn");
            }
        }
    }

    #[test]
    fn mercury_blobs_and_seek() {
        let v = vec![("producer", pt(24.0, 16.0)), ("radiusX", num(5.0)), ("radiusY", num(5.0)), ("birthRate", num(3.0))];
        let _ = run("ec.sim.ccmrmercury", &v, ramp(48, 48), 0.7);
        let a = run("ec.sim.ccmrmercury", &v, ramp(48, 48), 1.4);
        let mut v2 = v.clone();
        v2.push(("shading/metal", num(100.0)));
        let b = run("ec.sim.ccmrmercury", &v2, ramp(48, 48), 1.4);
        assert_eq!(a, b);
        assert!(a.data.iter().any(|p| p[3] > 0.5), "blobs visible");
        let none = run("ec.sim.ccmrmercury", &[("birthRate", num(0.0))], ramp(48, 48), 1.4);
        assert!(none.data.iter().all(|p| p[3] == 0.0));
    }

    #[test]
    fn half_resolution_scales_sprites() {
        // Downsampled buffers render the same layout at half size.
        let s = crate::find("ec.sim.ccsnowfall").unwrap();
        let mut params = crate::Params { values: s.params.iter().map(|p| (p.id.to_string(), p.default.clone())).collect() };
        params.values.insert("flakes".into(), num(50.0));
        params.values.insert("offset".into(), pt(20.0, 20.0));
        let ctx = EffectCtx { params: &params, time: 0.5, layer_size: [40.0, 40.0], seed: 1, adjustment: false, env: EffectEnv::default() };
        let full = crate::apply(s, &ctx, Buf { img: grey(40, 40), offset: [0.0; 2], scale: 1.0 });
        let half = crate::apply(s, &ctx, Buf { img: grey(20, 20), offset: [0.0; 2], scale: 0.5 });
        let sum = |i: &Image| i.data.iter().map(|p| p[0]).sum::<f32>() / i.data.len() as f32;
        assert!((sum(&full.img) - sum(&half.img)).abs() < 0.05, "{} {}", sum(&full.img), sum(&half.img));
    }
}

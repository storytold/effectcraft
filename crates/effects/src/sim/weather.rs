use super::common::{h, hs};
use super::sprites::{Acc, Post, Shape, Sprite, SpritePlan, Tint, aa, raster};
use super::{Buf, EffectCtx, Image, IntoParallelIterator, ParallelIterator, unpremul};

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

pub(super) fn rainfall(ctx: &EffectCtx, b: Buf) -> Buf {
    let mut plan = rainfall_plan(ctx, &b);
    plan.resolve(&b.img);
    plan.finish(b)
}

pub(super) fn rainfall_plan(ctx: &EffectCtx, b: &Buf) -> SpritePlan {
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

pub(super) fn snowfall(ctx: &EffectCtx, b: Buf) -> Buf {
    snowfall_plan(ctx, &b).finish(b)
}

pub(super) fn snowfall_plan(ctx: &EffectCtx, b: &Buf) -> SpritePlan {
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

pub(super) fn bubbles(ctx: &EffectCtx, mut b: Buf) -> Buf {
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
            let p = src.sample_bilinear_clamped((q.sx + dx * q.r * k) as f64, (q.sy + dy * q.r * k) as f64);
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

pub(super) fn drizzle(ctx: &EffectCtx, mut b: Buf) -> Buf {
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

pub(super) fn star_burst(ctx: &EffectCtx, b: Buf) -> Buf {
    let mut plan = star_burst_plan(ctx, &b);
    plan.resolve(&b.img);
    plan.finish(b)
}

pub(super) fn star_burst_plan(ctx: &EffectCtx, b: &Buf) -> SpritePlan {
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

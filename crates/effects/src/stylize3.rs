//! Stylize effects, batch 3: CC Block Load, CC Burn Film, CC Glass, CC HexTile, CC Mr. Smoothie
//! and CC Plastic.
//!
//! Written from the public descriptions of what these effects do (progressive block loading,
//! burning film, bump-mapped glass / plastic surfaces lit by an effect light, hexagonal tiling,
//! a colour map sampled from the image itself); the maths is our own.

use effectcraft_color::rgb_to_hsl;
use effectcraft_keyframe::Value;
use effectcraft_project::ParamUi;
use effectcraft_raster::{Image, Px};
use rayon::prelude::*;

use crate::noise::fbm;
use crate::util::{Plane, gauss_plane, layer_or_self, lerp4, premul, smoothstep, unpremul};
use crate::{Buf, EffectCtx, EffectSpec, col, num, p, popup, slider};

fn spec(id: &'static str, name: &'static str, params: Vec<crate::ParamSpec>, render: crate::RenderFn) -> EffectSpec {
    EffectSpec { id, name, category: "Stylize", params, render, gpu: false, float: true }
}

fn pct() -> ParamUi {
    slider(0.0, 100.0, 0.0, 100.0, 1)
}

fn pt(x: f64, y: f64) -> Value {
    Value::Vec2([x, y])
}

// ------------------------------------------------------------------ CC Block Load

/// Mean colour of every `s`×`s` block (grid of `ceil(w/s)`×`ceil(h/s)`).
fn block_means(img: &Image, s: usize) -> (usize, usize, Vec<Px>) {
    let (w, h) = (img.width as usize, img.height as usize);
    let gw = w.div_ceil(s).max(1);
    let gh = h.div_ceil(s).max(1);
    let mut out = vec![[0.0f32; 4]; gw * gh];
    out.par_chunks_mut(gw).enumerate().for_each(|(by, row)| {
        for (bx, o) in row.iter_mut().enumerate() {
            let mut acc = [0.0f32; 4];
            let mut n = 0.0f32;
            for y in by * s..((by + 1) * s).min(h) {
                for x in bx * s..((bx + 1) * s).min(w) {
                    let p = img.data[y * w + x];
                    for c in 0..4 {
                        acc[c] += p[c];
                    }
                    n += 1.0;
                }
            }
            if n > 0.0 {
                *o = acc.map(|v| v / n);
            }
        }
    });
    (gw, gh, out)
}

/// Value of pixel (x, y) at block size `s` (nearest block, or bilinear between block centres).
fn block_at(g: &(usize, usize, Vec<Px>), s: usize, x: usize, y: usize, bilinear: bool) -> Px {
    let (gw, gh, d) = g;
    if !bilinear || s == 1 {
        return d[(y / s).min(gh - 1) * gw + (x / s).min(gw - 1)];
    }
    let fx = (x as f32 + 0.5) / s as f32 - 0.5;
    let fy = (y as f32 + 0.5) / s as f32 - 0.5;
    let x0 = fx.floor();
    let y0 = fy.floor();
    let (tx, ty) = (fx - x0, fy - y0);
    let gx = |v: f32| (v as i64).clamp(0, *gw as i64 - 1) as usize;
    let gy = |v: f32| (v as i64).clamp(0, *gh as i64 - 1) as usize;
    let a = d[gy(y0) * gw + gx(x0)];
    let b = d[gy(y0) * gw + gx(x0 + 1.0)];
    let c = d[gy(y0 + 1.0) * gw + gx(x0)];
    let e = d[gy(y0 + 1.0) * gw + gx(x0 + 1.0)];
    lerp4(lerp4(a, b, tx), lerp4(c, e, tx), ty)
}

fn block_load(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let c = (ctx.params.f("completion") / 100.0).clamp(0.0, 1.0);
    if c >= 1.0 {
        return b;
    }
    let scan = ctx.params.b("scanlines");
    let smooth = ctx.params.b("smoothing");
    let cleared = ctx.params.b("startCleared");
    let bilinear = ctx.params.b("bilinear");
    let (w, h) = (b.img.width as usize, b.img.height as usize);
    let nlev = (w.max(h).max(2) as f64).log2().ceil() as u32;
    // Levels 0..=nlev with block sizes 2^(nlev-k); level k is "loading" while p is in [k, k+1).
    let p = c * (nlev + 1) as f64;
    let done = p.floor() as u32;
    let frac = (p - done as f64) as f32;
    let size = |k: u32| 1usize << (nlev - k.min(nlev));
    let s_new = size(done);
    let new = block_means(&b.img, s_new);
    let old = if done > 0 { Some((size(done - 1), block_means(&b.img, size(done - 1)))) } else { None };
    let rows_new = h.div_ceil(s_new) as f32;
    let soft = if smooth { 1.5f32 } else { 0.0 };
    let mut out = Image::new(w as u32, h as u32);
    out.rows_mut().for_each(|(y, row)| {
        // Fraction of the new level present on this row.
        let t = if scan {
            let front = frac * (rows_new + soft) - soft * 0.5;
            let r = (y / s_new) as f32 + 0.5;
            if soft > 0.0 {
                smoothstep(r - soft * 0.5, r + soft * 0.5, front)
            } else if r < front {
                1.0
            } else {
                0.0
            }
        } else if smooth {
            frac
        } else {
            0.0
        };
        for (x, o) in row.iter_mut().enumerate() {
            let n = if t > 0.0 { block_at(&new, s_new, x, y, bilinear) } else { [0.0; 4] };
            let prev = match &old {
                Some((s, g)) => block_at(g, *s, x, y, bilinear),
                None if cleared => [0.0; 4],
                None => block_at(&new, s_new, x, y, bilinear),
            };
            *o = if t >= 1.0 {
                n
            } else if t <= 0.0 {
                prev
            } else {
                lerp4(prev, n, t)
            };
        }
    });
    b.img = out;
    b
}

// ------------------------------------------------------------------ CC Burn Film

fn burn_film(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let burn = (ctx.params.f("burn") / 100.0).clamp(0.0, 1.0) as f32;
    if burn <= 0.0 {
        return b;
    }
    let (cx, cy) = b.to_px(ctx.params.v2("center"));
    let seed = ctx.params.f("randomSeed").max(0.0) as u32;
    let s = b.scale;
    let diag = (ctx.layer_size[0].hypot(ctx.layer_size[1]) * s).max(1.0);
    let feat = (60.0 * s).max(1e-3);
    let th = 1.0 - burn * 1.25;
    let band = 0.12 * (burn * 10.0).min(1.0);
    b.img.rows_mut().for_each(|(y, row)| {
        for (x, px) in row.iter_mut().enumerate() {
            let (fx, fy) = (x as f64 + 0.5, y as f64 + 0.5);
            let d = ((fx - cx).hypot(fy - cy) / diag) as f32;
            let n = fbm((fx / feat) as f32, (fy / feat) as f32, 0.0, seed.wrapping_add(17), 4.0);
            // High near the centre and where the noise is high; burning eats from the top down.
            let f = n * 0.65 + (1.0 - d.min(1.0)) * 0.35;
            if f > th + band {
                *px = [0.0; 4];
                continue;
            }
            if f > th - band * 2.0 {
                let (c, a) = unpremul(*px);
                // Charred rim (scorched dark) with a hot orange edge right next to the hole.
                let k = smoothstep(th - band * 2.0, th, f);
                let hot = smoothstep(th, th + band, f);
                let char = 1.0 - k * 0.85;
                let mut o = [c[0] * char, c[1] * char, c[2] * char];
                o = [o[0] + (1.0 - o[0]) * hot * 0.9, o[1] + (0.45 - o[1]) * hot * 0.9, o[2] * (1.0 - hot)];
                let na = a * (1.0 - hot * 0.6);
                *px = premul(o, na);
            }
        }
    });
    b
}

// ------------------------------------------------------------------ bump-map surfaces (Glass, Plastic)

const BUMP_PROPS: [&str; 6] = ["Red", "Green", "Blue", "Alpha", "Luminance", "Lightness"];

/// The bump-map layer parameter: CC Glass's Bump Map, CC Plastic's Bump Layer.
pub fn bump_layer_id(ctx: &EffectCtx) -> &'static str {
    if ctx.params.get("bumpLayer").is_some() { "bumpLayer" } else { "bumpMap" }
}

/// Height field (0..1) from the bump-map layer (or the layer itself), softened.
fn height_field(ctx: &EffectCtx, b: &Buf) -> Plane {
    let src = layer_or_self(ctx, b, bump_layer_id(ctx), true, false);
    let prop = ctx.params.e("property");
    let mut pl = Plane::from_image(&src, |p| {
        let (c, a) = unpremul(p);
        let v = match prop {
            0 => c[0],
            1 => c[1],
            2 => c[2],
            3 => return a,
            4 => effectcraft_color::luminance(c[0], c[1], c[2]),
            _ => rgb_to_hsl(c[0], c[1], c[2]).2,
        };
        v * a
    });
    let soft = ctx.params.f("softness") * b.scale * 0.5;
    if soft > 0.05 {
        pl = gauss_plane(&pl, soft, soft);
    }
    pl
}

/// The effect light and surface of CC Glass / CC Plastic (shared with the GPU compositor).
pub struct BumpLight {
    pub intensity: f32,
    pub color: [f32; 3],
    pub point: bool,
    pub pos: (f64, f64),
    /// Distant light direction (towards the light).
    pub dir: [f32; 3],
    pub height: f32,
    pub ambient: f32,
    pub diffuse: f32,
    pub specular: f32,
    pub shininess: f32,
    pub metal: f32,
}

impl BumpLight {
    pub fn from(ctx: &EffectCtx, b: &Buf) -> BumpLight {
        let pr = &ctx.params;
        let elev = (pr.f("lightHeight").clamp(-100.0, 100.0) / 100.0 * 90.0).to_radians();
        let az = pr.f("lightDirection").to_radians();
        let dir = [(az.sin() * elev.cos()) as f32, (-az.cos() * elev.cos()) as f32, elev.sin() as f32];
        let c = pr.color("lightColor");
        let rough = pr.f("roughness").clamp(0.001, 1.0) as f32;
        BumpLight {
            intensity: (pr.f("lightIntensity") / 100.0) as f32,
            color: [c[0], c[1], c[2]],
            point: pr.e("lightType") == 1,
            pos: b.to_px(pr.v2("lightPosition")),
            dir,
            height: (pr.f("lightHeight") / 100.0 * ctx.layer_size[0].max(ctx.layer_size[1]) * b.scale * 0.5) as f32,
            ambient: (pr.f("ambient") / 100.0) as f32,
            diffuse: (pr.f("diffuse") / 100.0) as f32,
            specular: (pr.f("specular") / 100.0) as f32,
            shininess: 1.0 / rough,
            metal: (pr.f("metal") / 100.0) as f32,
        }
    }

    fn to_light(&self, x: f64, y: f64) -> [f32; 3] {
        if self.point {
            let v = [(self.pos.0 - x) as f32, (self.pos.1 - y) as f32, self.height.max(1.0)];
            let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-6);
            [v[0] / l, v[1] / l, v[2] / l]
        } else {
            self.dir
        }
    }

    /// Lit straight colour for surface normal `n`. Diffuse is normalised so that a flat surface
    /// keeps its colour; highlights add on top.
    fn shade(&self, c: [f32; 3], n: [f32; 3], l: [f32; 3]) -> [f32; 3] {
        let ndl = (n[0] * l[0] + n[1] * l[1] + n[2] * l[2]).max(0.0);
        let flat = l[2].max(0.0);
        let base = self.ambient + self.diffuse * flat;
        let lit = if base > 1e-4 { (self.ambient + self.diffuse * ndl) / base } else { 1.0 };
        let lit = 1.0 + (lit - 1.0) * self.intensity;
        // Blinn half vector with the viewer straight above.
        let hv = [l[0], l[1], l[2] + 1.0];
        let hl = (hv[0] * hv[0] + hv[1] * hv[1] + hv[2] * hv[2]).sqrt().max(1e-6);
        let ndh = ((n[0] * hv[0] + n[1] * hv[1] + n[2] * hv[2]) / hl).max(0.0);
        let spec = self.specular * self.intensity * ndh.powf(self.shininess);
        let mut o = [0.0f32; 3];
        for i in 0..3 {
            let hl_col = self.color[i] * (1.0 - self.metal) + c[i] * self.color[i] * self.metal;
            o[i] = c[i] * lit * (1.0 + (self.color[i] - 1.0) * self.intensity.min(1.0)) + spec * hl_col;
        }
        o
    }
}

/// Surface normal from a height field at pixel (x, y) with height scale `k`.
#[inline]
fn normal(h: &Plane, x: usize, y: usize, k: f32) -> [f32; 3] {
    let (xi, yi) = (x as i64, y as i64);
    let gx = (h.get_clamped(xi + 1, yi) - h.get_clamped(xi - 1, yi)) * 0.5 * k;
    let gy = (h.get_clamped(xi, yi + 1) - h.get_clamped(xi, yi - 1)) * 0.5 * k;
    let l = (gx * gx + gy * gy + 1.0).sqrt();
    [-gx / l, -gy / l, 1.0 / l]
}

fn glass(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let hf = height_field(ctx, &b);
    let height = ctx.params.f("height") as f32 / 100.0;
    let k = height * 25.0;
    let disp = ctx.params.f("displacement") * b.scale * 0.25;
    let light = BumpLight::from(ctx, &b);
    let src = b.img.clone();
    b.img.rows_mut().for_each(|(y, row)| {
        for (x, px) in row.iter_mut().enumerate() {
            let n = normal(&hf, x, y, k);
            let (fx, fy) = (x as f64 + 0.5, y as f64 + 0.5);
            let s = if disp != 0.0 && height != 0.0 {
                src.sample_bilinear_clamped(fx + n[0] as f64 * disp, fy + n[1] as f64 * disp)
            } else {
                src.data[y * src.width as usize + x]
            };
            let a = src.data[y * src.width as usize + x][3];
            if a <= 0.0 {
                *px = [0.0; 4];
                continue;
            }
            let (c, sa) = unpremul(s);
            let c = if sa > 0.0 { c } else { unpremul(src.data[y * src.width as usize + x]).0 };
            let o = light.shade(c, n, light.to_light(fx, fy));
            *px = premul(o, a);
        }
    });
    b
}

fn plastic(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let mut hf = height_field(ctx, &b);
    let lo = (ctx.params.f("cutMin") / 100.0) as f32;
    let hi = (ctx.params.f("cutMax") / 100.0) as f32;
    if lo > 0.0 || hi < 1.0 {
        hf = hf.map(|v| v.clamp(lo.min(hi), hi.max(lo)));
    }
    let k = ctx.params.f("height") as f32 / 100.0 * 25.0;
    let light = BumpLight::from(ctx, &b);
    b.img.rows_mut().for_each(|(y, row)| {
        for (x, px) in row.iter_mut().enumerate() {
            if px[3] <= 0.0 {
                continue;
            }
            let (c, a) = unpremul(*px);
            let n = normal(&hf, x, y, k);
            let o = light.shade(c, n, light.to_light(x as f64 + 0.5, y as f64 + 0.5));
            *px = premul(o, a);
        }
    });
    b
}

// ------------------------------------------------------------------ CC HexTile

/// Axial hex cell (pointy-top) containing (x, y) for circumradius `r`; returns the cell centre
/// and integer cell coordinates.
fn hex_cell(x: f64, y: f64, r: f64) -> ((f64, f64), (i64, i64)) {
    let q = (3f64.sqrt() / 3.0 * x - y / 3.0) / r;
    let rr = (2.0 / 3.0 * y) / r;
    // Cube rounding.
    let (cx, cz) = (q, rr);
    let cy = (-3f64.sqrt() / 3.0 * x - y / 3.0) / r;
    let (mut rx, ry, mut rz) = (cx.round(), cy.round(), cz.round());
    let (dx, dy, dz) = ((rx - cx).abs(), (ry - cy).abs(), (rz - cz).abs());
    if dx > dy && dx > dz {
        rx = -ry - rz;
    } else if dy <= dz {
        rz = -rx - ry;
    }
    let px = r * 3f64.sqrt() * (rx + rz / 2.0);
    let py = r * 1.5 * rz;
    ((px, py), (rx as i64, rz as i64))
}

fn hextile(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let r = (ctx.params.f("radius") * b.scale).max(0.5);
    let (cx, cy) = b.to_px(ctx.params.v2("center"));
    let rot = ctx.params.f("rotate").to_radians();
    let (sn, cs) = rot.sin_cos();
    let smear = (ctx.params.f("smearing") / 100.0).clamp(0.0, 1.0);
    let mode = ctx.params.e("render");
    let src = b.img.clone();
    b.img.rows_mut().for_each(|(y, row)| {
        for (x, px) in row.iter_mut().enumerate() {
            let (dx, dy) = (x as f64 + 0.5 - cx, y as f64 + 0.5 - cy);
            // Into the rotated tiling frame.
            let (u, v) = (dx * cs + dy * sn, -dx * sn + dy * cs);
            let ((hx, hy), (qi, ri)) = hex_cell(u, v, r);
            let (mut lu, lv) = (u - hx, v - hy);
            let odd = (qi + ri).rem_euclid(2) == 1;
            match mode {
                1 if odd => lu = -lu,
                2 if odd => lu = -lu,
                _ => {}
            }
            let lv = if mode == 2 && odd { -lv } else { lv };
            // Every tile shows the hexagon around the centre point.
            let (ox, oy) = (lu * cs - lv * sn, lu * sn + lv * cs);
            let (tx, ty) = (cx + ox, cy + oy);
            let (sx, sy) = (tx + (x as f64 + 0.5 - tx) * smear, ty + (y as f64 + 0.5 - ty) * smear);
            *px = src.sample_bilinear_clamped(sx, sy);
        }
    });
    b
}

// ------------------------------------------------------------------ CC Mr. Smoothie

const SMOOTHIE_PROPS: [&str; 8] = ["Red", "Green", "Blue", "Alpha", "Luminance", "Lightness", "Hue", "Saturation"];

fn smoothie(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let flow = layer_or_self(ctx, &b, "flowLayer", true, false);
    let prop = ctx.params.e("property");
    let mut pl = Plane::from_image(&flow, |p| {
        let (c, a) = unpremul(p);
        match prop {
            0 => c[0],
            1 => c[1],
            2 => c[2],
            3 => a,
            4 => effectcraft_color::luminance(c[0], c[1], c[2]),
            5 => rgb_to_hsl(c[0], c[1], c[2]).2,
            6 => rgb_to_hsl(c[0], c[1], c[2]).0,
            _ => rgb_to_hsl(c[0], c[1], c[2]).1,
        }
    });
    let sm = ctx.params.f("smoothness") * b.scale * 0.5;
    if sm > 0.05 {
        pl = gauss_plane(&pl, sm, sm);
    }
    let (ax, ay) = b.to_px(ctx.params.v2("sampleA"));
    let (bx, by) = b.to_px(ctx.params.v2("sampleB"));
    // Colour palette sampled from the image along A→B.
    const N: usize = 256;
    let pal: Vec<Px> = (0..N)
        .map(|i| {
            let t = i as f64 / (N - 1) as f64;
            let p = b.img.sample_bilinear_clamped(ax + (bx - ax) * t, ay + (by - ay) * t);
            let (c, _) = unpremul(p);
            [c[0], c[1], c[2], 1.0]
        })
        .collect();
    let phase = (ctx.params.f("phase") / 360.0) as f32;
    let loops = (ctx.params.e("colorLoop") + 1) as f32;
    b.img.data.par_iter_mut().zip(pl.data.par_iter()).for_each(|(px, &v)| {
        if px[3] <= 0.0 {
            return;
        }
        let t = (v * loops + phase).rem_euclid(1.0);
        // Ping-pong so the palette loops without seams.
        let t = if t < 0.5 { t * 2.0 } else { 2.0 - t * 2.0 };
        let f = t * (N - 1) as f32;
        let i = (f as usize).min(N - 2);
        let c = lerp4(pal[i], pal[i + 1], f - i as f32);
        *px = premul([c[0], c[1], c[2]], px[3]);
    });
    b
}

// ------------------------------------------------------------------ specs

fn light_params() -> Vec<crate::ParamSpec> {
    vec![
        p("using", "Using", Value::Enum(0), popup(&["Effect Light", "AE Lights"])),
        p("lightIntensity", "Light Intensity", num(100.0), slider(0.0, 400.0, 0.0, 200.0, 1)),
        p("lightColor", "Light Color", col(1.0, 1.0, 1.0), ParamUi::Color),
        p("lightType", "Light Type", Value::Enum(0), popup(&["Distant Light", "Point Light"])),
        p("lightHeight", "Light Height", num(50.0), slider(-100.0, 100.0, -100.0, 100.0, 1)),
        p("lightPosition", "Light Position", pt(0.25, 0.25), ParamUi::Point),
        p("lightDirection", "Light Direction", num(-45.0), ParamUi::Angle),
    ]
}

fn shading_params(ambient: f64, diffuse: f64, specular: f64, roughness: f64, metal: f64) -> Vec<crate::ParamSpec> {
    vec![
        p("ambient", "Ambient", num(ambient), slider(0.0, 200.0, 0.0, 100.0, 1)),
        p("diffuse", "Diffuse", num(diffuse), slider(0.0, 200.0, 0.0, 100.0, 1)),
        p("specular", "Specular", num(specular), slider(0.0, 200.0, 0.0, 100.0, 1)),
        p("roughness", "Roughness", num(roughness), slider(0.001, 0.5, 0.001, 0.5, 3)),
        p("metal", "Metal", num(metal), pct()),
    ]
}

pub fn specs() -> Vec<EffectSpec> {
    let mut glass_p = vec![
        p("bumpMap", "Bump Map", Value::Layer(None), ParamUi::Layer),
        p("property", "Property", Value::Enum(3), popup(&BUMP_PROPS)),
        p("softness", "Softness", num(10.0), slider(0.0, 200.0, 0.0, 100.0, 1)),
        p("height", "Height", num(50.0), slider(-100.0, 100.0, -100.0, 100.0, 1)),
        p("displacement", "Displacement", num(100.0), slider(-500.0, 500.0, -200.0, 200.0, 1)),
    ];
    glass_p.extend(light_params());
    glass_p.extend(shading_params(75.0, 32.0, 26.0, 0.05, 100.0));
    let mut plastic_p = vec![
        p("bumpLayer", "Bump Layer", Value::Layer(None), ParamUi::Layer),
        p("property", "Property", Value::Enum(3), popup(&BUMP_PROPS)),
        p("softness", "Softness", num(10.0), slider(0.0, 200.0, 0.0, 100.0, 1)),
        p("height", "Height", num(25.0), slider(-100.0, 100.0, -100.0, 100.0, 1)),
        p("cutMin", "Cut Min", num(0.0), pct()),
        p("cutMax", "Cut Max", num(100.0), pct()),
    ];
    plastic_p.extend(light_params());
    plastic_p.extend(shading_params(100.0, 70.0, 50.0, 0.05, 50.0));
    vec![
        spec(
            "ec.stylize.ccblockload",
            "CC Block Load",
            vec![
                p("completion", "Completion", num(0.0), pct()),
                p("scanlines", "Scanlines", Value::Bool(true), ParamUi::Checkbox),
                p("smoothing", "Smoothing", Value::Bool(true), ParamUi::Checkbox),
                p("startCleared", "Start Cleared", Value::Bool(true), ParamUi::Checkbox),
                p("bilinear", "Bilinear", Value::Bool(false), ParamUi::Checkbox),
            ],
            block_load,
        ),
        spec(
            "ec.stylize.ccburnfilm",
            "CC Burn Film",
            vec![
                p("burn", "Burn", num(0.0), pct()),
                p("center", "Center", pt(0.5, 0.5), ParamUi::Point),
                p("randomSeed", "Random Seed", num(0.0), slider(0.0, 10000.0, 0.0, 100.0, 0)),
            ],
            burn_film,
        ),
        spec("ec.stylize.ccglass", "CC Glass", glass_p, glass),
        spec(
            "ec.stylize.cchextile",
            "CC HexTile",
            vec![
                p("render", "Render", Value::Enum(0), popup(&["Normal", "Mirrored", "Rotated"])),
                p("radius", "Radius", num(50.0), slider(1.0, 4000.0, 2.0, 500.0, 1)),
                p("center", "Center", pt(0.5, 0.5), ParamUi::Point),
                p("rotate", "Rotate", num(0.0), ParamUi::Angle),
                p("smearing", "Smearing", num(0.0), pct()),
            ],
            hextile,
        ),
        spec(
            "ec.stylize.ccmrsmoothie",
            "CC Mr. Smoothie",
            vec![
                p("flowLayer", "Flow Layer", Value::Layer(None), ParamUi::Layer),
                p("property", "Property", Value::Enum(4), popup(&SMOOTHIE_PROPS)),
                p("smoothness", "Smoothness", num(20.0), slider(0.0, 200.0, 0.0, 100.0, 1)),
                p("sampleA", "Sample A", pt(0.25, 0.5), ParamUi::Point),
                p("sampleB", "Sample B", pt(0.75, 0.5), ParamUi::Point),
                p("phase", "Phase", num(0.0), ParamUi::Angle),
                p("colorLoop", "Color Loop", Value::Enum(0), popup(&["1x", "2x", "3x", "4x"])),
            ],
            smoothie,
        ),
        spec("ec.stylize.ccplastic", "CC Plastic", plastic_p, plastic),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EffectEnv, run_fx};

    fn ramp(w: u32, h: u32) -> Image {
        let mut img = Image::new(w, h);
        for y in 0..h {
            for x in 0..w {
                img.set(x, y, [x as f32 / w as f32, y as f32 / h as f32, 0.3, 1.0]);
            }
        }
        img
    }

    fn bumpy(w: u32, h: u32) -> Image {
        let mut img = ramp(w, h);
        for y in 0..h {
            for x in 0..w {
                let a = if (x as i32 - 20).pow(2) + (y as i32 - 16).pow(2) < 64 { 1.0 } else { 0.5 };
                let p = img.data[(y * w + x) as usize];
                img.set(x, y, [p[0] * a, p[1] * a, p[2] * a, a]);
            }
        }
        img
    }

    fn run(id: &str, vals: &[(&str, Value)], img: Image) -> Buf {
        run_fx(id, vals, img, 0.0, EffectEnv::default())
    }

    fn max_diff(a: &Image, b: &Image) -> f32 {
        a.data.iter().zip(&b.data).flat_map(|(p, q)| (0..4).map(move |c| (p[c] - q[c]).abs())).fold(0.0, f32::max)
    }

    const ALL: [&str; 6] =
        ["ec.stylize.ccblockload", "ec.stylize.ccburnfilm", "ec.stylize.ccglass", "ec.stylize.cchextile", "ec.stylize.ccmrsmoothie", "ec.stylize.ccplastic"];

    #[test]
    fn all_deterministic() {
        for id in ALL {
            let vals: &[(&str, Value)] = match id {
                "ec.stylize.ccblockload" => &[("completion", num(40.0))],
                "ec.stylize.ccburnfilm" => &[("burn", num(50.0))],
                _ => &[],
            };
            let a = run(id, vals, bumpy(40, 32));
            let b = run(id, vals, bumpy(40, 32));
            assert_eq!(a.img.data, b.img.data, "{id}");
        }
    }

    #[test]
    fn block_load_complete_is_identity_and_start_cleared() {
        let img = ramp(40, 30);
        assert_eq!(run("ec.stylize.ccblockload", &[("completion", num(100.0))], img.clone()).img.data, img.data);
        let o = run("ec.stylize.ccblockload", &[("completion", num(0.0))], img.clone());
        assert!(o.img.data.iter().all(|p| p[3] == 0.0));
        // Midway: blocky (fewer distinct colours than the source).
        let o = run("ec.stylize.ccblockload", &[("completion", num(50.0)), ("smoothing", Value::Bool(false))], img.clone());
        let mut cols: Vec<u32> = o.img.data.iter().map(|p| (p[0] * 1000.0) as u32).collect();
        cols.sort();
        cols.dedup();
        assert!(cols.len() < 40, "{}", cols.len());
    }

    #[test]
    fn burn_film_zero_identity_full_burns() {
        let img = ramp(40, 30);
        assert_eq!(run("ec.stylize.ccburnfilm", &[], img.clone()).img.data, img.data);
        let o = run("ec.stylize.ccburnfilm", &[("burn", num(100.0))], img);
        assert!(o.img.data.iter().all(|p| p[3] == 0.0));
    }

    #[test]
    fn glass_flat_without_highlight_is_identity() {
        let img = bumpy(40, 32);
        let o = run("ec.stylize.ccglass", &[("height", num(0.0)), ("specular", num(0.0))], img.clone());
        assert!(max_diff(&o.img, &img) < 1e-4, "{}", max_diff(&o.img, &img));
        let o = run("ec.stylize.ccglass", &[], img.clone());
        assert!(max_diff(&o.img, &img) > 0.01);
    }

    #[test]
    fn plastic_shades_bumps() {
        let img = bumpy(40, 32);
        let o = run("ec.stylize.ccplastic", &[("height", num(0.0)), ("specular", num(0.0))], img.clone());
        assert!(max_diff(&o.img, &img) < 1e-4);
        let o = run("ec.stylize.ccplastic", &[("height", num(80.0))], img.clone());
        assert!(max_diff(&o.img, &img) > 0.01);
        // Alpha is untouched.
        assert!(o.img.data.iter().zip(&img.data).all(|(a, b)| (a[3] - b[3]).abs() < 1e-6));
    }

    #[test]
    fn hextile_repeats_centre() {
        let img = ramp(64, 64);
        let o = run("ec.stylize.cchextile", &[("radius", num(8.0)), ("center", pt(32.0, 32.0))], img.clone());
        assert!(max_diff(&o.img, &img) > 0.1);
        // Pixels at the centre are unchanged (inside the central tile).
        let c = (32 * 64 + 32) as usize;
        assert!((0..4).all(|i| (o.img.data[c][i] - img.data[c][i]).abs() < 1e-4));
        // Full smearing restores the source.
        let o = run("ec.stylize.cchextile", &[("radius", num(8.0)), ("smearing", num(100.0))], img.clone());
        assert!(max_diff(&o.img, &img) < 1e-4);
    }

    #[test]
    fn smoothie_maps_to_sampled_palette() {
        let img = ramp(40, 30);
        let o = run("ec.stylize.ccmrsmoothie", &[("smoothness", num(0.0)), ("sampleA", pt(10.0, 15.0)), ("sampleB", pt(30.0, 15.0))], img.clone());
        // Every output colour lies on the palette line between A and B (blue stays 0.3, green ≈ 0.5).
        assert!(o.img.data.iter().all(|p| (p[2] - 0.3).abs() < 1e-3 && (p[1] - 0.5).abs() < 0.03));
        assert!(max_diff(&o.img, &img) > 0.1);
    }
}

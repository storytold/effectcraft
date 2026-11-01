//! Physically based shading for the Advanced 3D renderer (CPU reference; `crates/gpu`'s
//! `advanced3d.wgsl` repeats it step for step).
//!
//! - Direct light: Cook–Torrance microfacet specular with the GGX/Trowbridge–Reitz normal
//!   distribution, the Smith–Schlick-GGX geometry term (`k = (r + 1)² / 8`) and Schlick's
//!   Fresnel (`F0` = 0.04 for dielectrics, the base colour for metals), plus a Lambertian
//!   diffuse lobe weighted by `(1 − F)(1 − metallic)` (Karis 2013, "Real Shading in Unreal
//!   Engine 4"; the glTF 2.0 metallic-roughness model). Light colours are the irradiance on a
//!   surface facing the light (a white 100% light shows a white diffuse surface at its base
//!   colour), i.e. π is folded into the light intensity.
//! - Image-based light: a cosine-convolved irradiance map for diffuse, a box-filtered mip
//!   chain of the equirectangular environment sampled by roughness for specular, and the
//!   analytic split-sum environment BRDF fit (Karis 2014, "Physically Based Shading on
//!   Mobile").
//! - Shadows: depth maps per light, percentage-closer filtered.

use super::scene::{EnvInfo, Light, Material, Scene, ShadowMap, TexInfo};

pub const PI: f32 = std::f32::consts::PI;

#[inline]
pub fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
#[inline]
pub fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
#[inline]
pub fn norm(a: [f32; 3]) -> [f32; 3] {
    let l = dot(a, a).sqrt();
    if l > 1e-20 { [a[0] / l, a[1] / l, a[2] / l] } else { [0.0, 0.0, -1.0] }
}
#[inline]
fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
#[inline]
fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
#[inline]
fn scale(a: [f32; 3], k: f32) -> [f32; 3] {
    [a[0] * k, a[1] * k, a[2] * k]
}
#[inline]
fn mix(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// sRGB-encoded → linear light.
#[inline]
pub fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}
/// Linear light → sRGB-encoded (over-range values continue the curve).
#[inline]
pub fn linear_to_srgb(c: f32) -> f32 {
    if c <= 0.0031308 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 }
}

/// GGX / Trowbridge–Reitz normal distribution, `a = roughness²`.
#[inline]
pub fn d_ggx(n_h: f32, a: f32) -> f32 {
    let a2 = a * a;
    let d = n_h * n_h * (a2 - 1.0) + 1.0;
    a2 / (PI * d * d).max(1e-12)
}

/// Smith–Schlick-GGX geometry term for direct light (`k = (r + 1)² / 8`).
#[inline]
pub fn g_smith(n_v: f32, n_l: f32, rough: f32) -> f32 {
    let k = (rough + 1.0) * (rough + 1.0) / 8.0;
    let g1 = |x: f32| x / (x * (1.0 - k) + k);
    g1(n_v) * g1(n_l)
}

/// Schlick's Fresnel approximation.
#[inline]
pub fn f_schlick(f0: [f32; 3], v_h: f32) -> [f32; 3] {
    let k = (1.0 - v_h).clamp(0.0, 1.0).powi(5);
    [f0[0] + (1.0 - f0[0]) * k, f0[1] + (1.0 - f0[1]) * k, f0[2] + (1.0 - f0[2]) * k]
}

/// Surface parameters used by the direct-light BRDF after texture sampling.
#[derive(Clone, Copy)]
pub struct BrdfSurface {
    pub base: [f32; 3],
    pub metallic: f32,
    pub roughness: f32,
    pub specular: f32,
    pub diffuse: f32,
}

/// Cook–Torrance BRDF × N·L × π for one light: (diffuse colour factor, specular colour).
/// Returned as the reflected fraction of the light's irradiance.
pub fn brdf(surface: BrdfSurface, n: [f32; 3], v: [f32; 3], l: [f32; 3]) -> [f32; 3] {
    let BrdfSurface { base, metallic, roughness: rough, specular: spec_k, diffuse: diff_k } = surface;
    let n_l = dot(n, l);
    if n_l <= 0.0 {
        return [0.0; 3];
    }
    let n_v = dot(n, v).max(1e-4);
    let h = norm(add(v, l));
    let n_h = dot(n, h).max(0.0);
    let v_h = dot(v, h).max(0.0);
    let r = rough.clamp(0.03, 1.0);
    let f0 = [mix(0.04 * spec_k, base[0], metallic), mix(0.04 * spec_k, base[1], metallic), mix(0.04 * spec_k, base[2], metallic)];
    let f = f_schlick(f0, v_h);
    let d = d_ggx(n_h, r * r);
    let g = g_smith(n_v, n_l, r);
    let spec = d * g / (4.0 * n_v * n_l).max(1e-4);
    let mut out = [0.0; 3];
    for c in 0..3 {
        let kd = (1.0 - f[c]) * (1.0 - metallic);
        out[c] = (kd * base[c] / PI * diff_k + f[c] * spec) * n_l * PI;
    }
    out
}

/// Split-sum environment BRDF (scale, bias) for `F0·A + B` (analytic fit).
#[inline]
pub fn env_brdf(n_v: f32, rough: f32) -> (f32, f32) {
    let c0 = [-1.0, -0.0275, -0.572, 0.022];
    let c1 = [1.0, 0.0425, 1.04, -0.04];
    let r = [rough * c0[0] + c1[0], rough * c0[1] + c1[1], rough * c0[2] + c1[2], rough * c0[3] + c1[3]];
    let a004 = (r[0] * r[0]).min((-9.28 * n_v).exp2()) * r[0] + r[1];
    (-1.04 * a004 + r[2], 1.04 * a004 + r[3])
}

/// Wrap a texel index.
#[inline]
fn wrap(i: i64, n: i64, mode: u32) -> i64 {
    match mode {
        1 => i.clamp(0, n - 1),
        2 => {
            let m = i.rem_euclid(2 * n);
            if m >= n { 2 * n - 1 - m } else { m }
        }
        _ => i.rem_euclid(n),
    }
}

/// Bilinear texture sample at (u, v) (0..1 across the texture, texel centres at +0.5).
pub fn sample(t: &TexInfo, texels: &[[f32; 4]], u: f32, v: f32) -> [f32; 4] {
    let (w, h) = (t.width as i64, t.height as i64);
    if w == 0 || h == 0 {
        return [1.0; 4];
    }
    let x = u * w as f32 - 0.5;
    let y = v * h as f32 - 0.5;
    let (x0, y0) = (x.floor(), y.floor());
    let (fx, fy) = (x - x0, y - y0);
    let (x0, y0) = (x0 as i64, y0 as i64);
    let get = |xi: i64, yi: i64| -> [f32; 4] {
        let xi = wrap(xi, w, t.wrap_u);
        let yi = wrap(yi, h, t.wrap_v);
        texels.get(t.offset as usize + (yi * w + xi) as usize).copied().unwrap_or([0.0; 4])
    };
    let (a, b, c, d) = (get(x0, y0), get(x0 + 1, y0), get(x0, y0 + 1), get(x0 + 1, y0 + 1));
    let mut o = [0.0; 4];
    for k in 0..4 {
        let top = a[k] + (b[k] - a[k]) * fx;
        let bot = c[k] + (d[k] - c[k]) * fx;
        o[k] = top + (bot - top) * fy;
    }
    o
}

/// Equirectangular coordinates of a world direction (AE world: +Y down; the environment's up
/// is −Y), rotated about the vertical axis by `rot` radians.
#[inline]
pub fn equirect_uv(d: [f32; 3], rot: f32) -> (f32, f32) {
    let u = 0.5 + (d[0].atan2(d[2]) + rot) / (2.0 * PI);
    let v = (-d[1]).clamp(-1.0, 1.0).acos() / PI;
    (u - u.floor(), v)
}

/// Inverse of [`equirect_uv`] (texel centre → direction).
pub fn equirect_dir(u: f32, v: f32, rot: f32) -> [f32; 3] {
    let phi = (u - 0.5) * 2.0 * PI - rot;
    let th = v * PI;
    [th.sin() * phi.sin(), -th.cos(), th.sin() * phi.cos()]
}

/// Diffuse (irradiance) and roughness-filtered specular radiance of the environment.
pub fn env_light(s: &Scene, e: &EnvInfo, n: [f32; 3], r: [f32; 3], rough: f32) -> ([f32; 3], [f32; 3]) {
    let (u, v) = equirect_uv(n, e.rotation);
    let irr = s.textures.get(e.irradiance as usize).map_or([0.0; 4], |t| sample(t, &s.texels, u, v));
    let (u, v) = equirect_uv(r, e.rotation);
    let lv = rough.clamp(0.0, 1.0) * (e.mips.saturating_sub(1)) as f32;
    let l0 = lv.floor() as u32;
    let l1 = (l0 + 1).min(e.mips.saturating_sub(1));
    let f = lv - l0 as f32;
    let a = s.textures.get((e.radiance + l0) as usize).map_or([0.0; 4], |t| sample(t, &s.texels, u, v));
    let b = s.textures.get((e.radiance + l1) as usize).map_or([0.0; 4], |t| sample(t, &s.texels, u, v));
    let spec = [mix(a[0], b[0], f), mix(a[1], b[1], f), mix(a[2], b[2], f)];
    (scale([irr[0], irr[1], irr[2]], e.intensity), scale(spec, e.intensity))
}

/// Light-space position (u, v in shadow-map pixels, depth) of a world point.
#[inline]
pub fn shadow_coord(m: &ShadowMap, p: [f32; 3]) -> Option<(f32, f32, f32)> {
    let v = &m.view;
    let q = [
        v[0][0] * p[0] + v[0][1] * p[1] + v[0][2] * p[2] + v[0][3],
        v[1][0] * p[0] + v[1][1] * p[1] + v[1][2] * p[2] + v[1][3],
        v[2][0] * p[0] + v[2][1] * p[1] + v[2][2] * p[2] + v[2][3],
    ];
    let c = m.size as f32 * 0.5;
    if m.ortho {
        Some((q[0] * m.focal + c, q[1] * m.focal + c, q[2]))
    } else if q[2] > 1e-3 {
        Some((q[0] * m.focal / q[2] + c, q[1] * m.focal / q[2] + c, q[2]))
    } else {
        None
    }
}

/// Integer tap jitter hash: pure u32 bit math so the WGSL backend computes
/// bit-identical jitter from the same inputs (a float hash would diverge between
/// backends and break CPU/GPU parity).
fn tap_hash(x: i32, y: i32) -> u32 {
    let mut h = (x as u32).wrapping_mul(0x9E37_79B9) ^ (y as u32).wrapping_mul(0x85EB_CA6B);
    h ^= h >> 16;
    h = h.wrapping_mul(0x2545_F491);
    h ^= h >> 15;
    h
}

/// The jitter of tap (`i`, `j`), each axis in [−0.5, 0.5): [`tap_hash`] values at the four
/// corners of the receiver's 1/8-texel lattice cell (`cx`, `cy`), blended bilinearly by the
/// receiver's position in the cell (`wx`, `wy`). The jitter is continuous in the receiver's
/// map position, so a position that differs between backends in its last bits moves the
/// taps by an amount proportional to that difference and does not switch them to another
/// cell's jitter.
fn tap_jitter(cx: i32, cy: i32, wx: f32, wy: f32, i: i32, j: i32) -> (f32, f32) {
    let corner = |dx: i32, dy: i32| -> (f32, f32) {
        let h = tap_hash(cx.wrapping_add(dx).wrapping_add(i.wrapping_mul(7919)), cy.wrapping_add(dy).wrapping_add(j.wrapping_mul(7919)));
        (((h & 0xFFFF) as f32) * (1.0 / 65536.0) - 0.5, (((h >> 16) & 0xFFFF) as f32) * (1.0 / 65536.0) - 0.5)
    };
    let (c00, c10, c01, c11) = (corner(0, 0), corner(1, 0), corner(0, 1), corner(1, 1));
    let ax = c00.0 + (c10.0 - c00.0) * wx;
    let bx = c01.0 + (c11.0 - c01.0) * wx;
    let ay = c00.1 + (c10.1 - c00.1) * wx;
    let by = c01.1 + (c11.1 - c01.1) * wx;
    (ax + (bx - ax) * wy, ay + (by - ay) * wy)
}

/// Upper bound of the PCF grid's taps per axis. The grid grows with the reach so taps
/// stay about one texel apart for moderate penumbrae; the cap bounds the per-fragment
/// cost of very wide ones (bilinear taps still overlap and the jitter dithers the rest).
const PCF_MAX_TAPS: i32 = 10;

/// Steepest receiver-plane slope the PCF extrapolates, in texel footprints of depth per
/// texel (about 89°). Steeper planes are edge-on to the light, where their depth runs
/// away; they are clamped to this slope.
const PLANE_SLOPE_MAX: f32 = 64.0;

/// Fraction of light reaching `p` through shadow map `m` (1 = lit). `ng` is the
/// receiver's geometric (not normal-mapped) normal facing the viewer.
///
/// PCF over an N×N tap grid (5 ≤ N ≤ `PCF_MAX_TAPS`) spread over a reach derived
/// from `radius` (the wanted penumbra in output pixels) divided by the receiver's
/// texel footprint, each tap jittered by [`tap_jitter`] (an integer hash of the
/// receiver's sub-texel lattice and the tap index, blended across the receiver's
/// lattice cell), and each tap comparing against the four
/// nearest texel centres with bilinear weights. The jitter detaches the tap grid
/// from the texel axes (axis-aligned binary taps are what stair-step a straight
/// shadow edge), and keying it below texel size keeps the dither finer than an
/// output pixel even where one texel covers several. The bilinear compare makes
/// each tap's contribution vary smoothly inside a texel instead of snapping.
///
/// Every texel is compared against the depth the receiver's own plane has at that
/// texel's centre, not the receiver point's depth: a wide kernel on a surface tilted
/// towards the light would otherwise see its own nearer depth and shadow itself.
/// With that, the bias stays at texel scale; a diffusion-scaled bias lifts shadows
/// off their casters.
pub fn shadow_factor(m: &ShadowMap, texels: &[f32], p: [f32; 3], ng: [f32; 3], n_l: f32) -> f32 {
    let Some((u, v, z)) = shadow_coord(m, p) else { return 1.0 };
    let size = m.size as i64;
    // Slope-scaled bias at texel scale (world units are pixels), steeper for
    // grazing light: it absorbs depth-raster error and the gap between the shading
    // normal's plane and the faceted surface the map rasterised.
    let texel = if m.ortho { 1.0 / m.focal } else { z.abs() / m.focal };
    let bias = m.bias * texel * (1.0 + 3.0 * (1.0 - n_l.clamp(0.0, 1.0)));
    let at = |xi: i64, yi: i64| -> f32 {
        if xi < 0 || yi < 0 || xi >= size || yi >= size {
            return f32::INFINITY; // outside the map: the light was never blocked there (WGSL mirrors this with a large finite depth)
        }
        texels.get(m.offset as usize + (yi * size + xi) as usize).copied().unwrap_or(f32::INFINITY)
    };
    // The receiver plane in light space as the slope of its depth across the map, in
    // texel footprints per texel (`tx`, `ty`; depth grows along −t), clamped where the
    // plane grazes the light. `den0` is the plane normal against the light ray through
    // the receiver (the depth axis for an orthographic map), negative while it faces it.
    let w = &m.view;
    let nv = [0, 1, 2].map(|r| w[r][0] * ng[0] + w[r][1] * ng[1] + w[r][2] * ng[2]);
    let c = m.size as f32 * 0.5;
    let den0 = if m.ortho { nv[2] } else { nv[0] * ((u - c) / m.focal) + nv[1] * ((v - c) / m.focal) + nv[2] };
    let (tx, ty) = if den0 < -1e-6 {
        ((nv[0] / den0).clamp(-PLANE_SLOPE_MAX, PLANE_SLOPE_MAX), (nv[1] / den0).clamp(-PLANE_SLOPE_MAX, PLANE_SLOPE_MAX))
    } else {
        (0.0, 0.0)
    };
    // 1 where texel (xi, yi) does not occlude the receiver plane at the texel's centre.
    // Perspective depth is not linear across the map but its inverse is, so that
    // compare runs on 1/depth (inv · (texel + bias) ≥ 1) without a division; a plane
    // whose inverse depth reaches 0 has left the light's side and cannot be occluded.
    let (iz, iq) = (1.0 / z, 1.0 / (m.focal * z));
    let open = |xi: i64, yi: i64| -> f32 {
        let s = at(xi, yi) + bias;
        let g = tx * (xi as f32 + 0.5 - u) + ty * (yi as f32 + 0.5 - v);
        let lit = if m.ortho {
            z - g * texel <= s
        } else {
            let inv = iz + g * iq;
            inv <= 0.0 || inv * s >= 1.0
        };
        if lit { 1.0 } else { 0.0 }
    };
    // `radius` is the wanted penumbra in output pixels; converting it through the
    // receiver's texel footprint keeps the penumbra width in output pixels at any
    // map resolution (and lets perspective penumbra grow with distance from the light).
    // At least two texels: a hard edge then ramps across two texels, which hides the
    // texel steps of the binary depth compare where one texel spans several output
    // pixels (receivers grazing the light); a one-texel ramp leaves a sawtooth.
    let mut reach = (m.radius / texel).clamp(2.0, 256.0);
    if !reach.is_finite() {
        reach = 1.0;
    }
    let n = (reach.ceil() as i32 + 1).clamp(5, PCF_MAX_TAPS);
    let step = reach / (n - 1) as f32;
    let half = (n - 1) as f32 * 0.5;
    let (gu, gv) = (u * 8.0, v * 8.0);
    let (cx, cy) = (gu.floor() as i32, gv.floor() as i32);
    let (wx, wy) = (gu - gu.floor(), gv - gv.floor());
    let mut lit = 0.0;
    for j in 0..n {
        for i in 0..n {
            let (jx, jy) = tap_jitter(cx, cy, wx, wy, i, j);
            let (su, sv) = (u + ((i as f32 - half) + jx) * step, v + ((j as f32 - half) + jy) * step);
            // Texel (x, y) holds the depth at its centre (x + 0.5, y + 0.5); each of the
            // four is compared against the receiver plane at that centre.
            let (tu, tv) = (su - 0.5, sv - 0.5);
            let (fx, fy) = (tu - tu.floor(), tv - tv.floor());
            let (x0, y0) = (tu.floor() as i64, tv.floor() as i64);
            let (h00, h10, h01, h11) = (open(x0, y0), open(x0 + 1, y0), open(x0, y0 + 1), open(x0 + 1, y0 + 1));
            let a = h00 + (h10 - h00) * fx;
            let b = h01 + (h11 - h01) * fx;
            lit += a + (b - a) * fy;
        }
    }
    lit / (n * n) as f32
}

/// The visibility of light `li` at `p` (1 = unshadowed), darkened by Shadow Darkness.
pub fn light_shadow(s: &Scene, l: &Light, p: [f32; 3], ng: [f32; 3], n_l: f32) -> f32 {
    if l.shadow < 0 {
        return 1.0;
    }
    let first = l.shadow as usize;
    let m = if l.kind == 2 {
        // Point light: the cube face of the major axis of the light → point direction.
        let d = sub(p, l.pos);
        let (ax, ay, az) = (d[0].abs(), d[1].abs(), d[2].abs());
        let face = if ax >= ay && ax >= az {
            if d[0] > 0.0 { 0 } else { 1 }
        } else if ay >= az {
            if d[1] > 0.0 { 2 } else { 3 }
        } else if d[2] > 0.0 {
            4
        } else {
            5
        };
        s.shadows.get(first + face)
    } else {
        s.shadows.get(first)
    };
    let Some(m) = m else { return 1.0 };
    let f = shadow_factor(m, &s.shadow_texels, p, ng, n_l);
    1.0 - l.shadow_darkness * (1.0 - f)
}

/// The visibility of the Environment light at `p` (1 = unshadowed), darkened by its Shadow
/// Darkness — but only by the image's key fraction: the shadow map occludes the dominant
/// (key) lobe, while the environment's fill keeps shining on the shadowed surface, so an
/// Environment light's shadow stays partial the way After Effects renders an area-light
/// shadow (a broad environment like the default studio shadows softly, a lone bright spot
/// on black almost fully). A directionless image (or a non-casting light) has no map
/// and shades unshadowed.
pub fn env_shadow(s: &Scene, e: &EnvInfo, p: [f32; 3], ng: [f32; 3], n: [f32; 3]) -> f32 {
    if e.shadow < 0 || e.sun == [0.0; 3] {
        return 1.0;
    }
    let Some(m) = s.shadows.get(e.shadow as usize) else { return 1.0 };
    let n_l = dot(n, e.sun).clamp(0.0, 1.0);
    let f = shadow_factor(m, &s.shadow_texels, p, ng, n_l);
    1.0 - e.shadow_darkness * e.key * (1.0 - f)
}

/// Unit direction from `p` to light `l`, and the distance (infinite for a parallel light).
fn to_light(l: &Light, p: [f32; 3]) -> ([f32; 3], f32) {
    if l.kind == 0 {
        return (scale(l.dir, -1.0), f32::INFINITY);
    }
    let d = sub(l.pos, p);
    let len = dot(d, d).sqrt();
    if len < 1e-6 { ([0.0, 0.0, -1.0], 0.0) } else { (scale(d, 1.0 / len), len) }
}

/// Distance attenuation × spot cone of a light at `p` (AE falloff modes).
pub fn attenuation(l: &Light, p: [f32; 3], to_l: [f32; 3], dist: f32) -> f32 {
    let mut k = match l.falloff {
        1 if l.kind != 0 => {
            if dist <= l.radius {
                1.0
            } else if l.falloff_distance <= 0.0 {
                0.0
            } else {
                let t = ((dist - l.radius) / l.falloff_distance).clamp(0.0, 1.0);
                1.0 - t * t * (3.0 - 2.0 * t)
            }
        }
        2 if l.kind != 0 => {
            if dist <= l.radius || (l.radius <= 0.0 && dist <= 1.0) {
                1.0
            } else {
                (l.radius.max(1.0) / dist).powi(2)
            }
        }
        _ => 1.0,
    };
    let _ = p;
    if l.kind == 1 {
        let cos_a = dot(scale(to_l, -1.0), l.dir).clamp(-1.0, 1.0);
        k *= if cos_a >= l.cos_inner {
            1.0
        } else if cos_a <= l.cos_outer {
            0.0
        } else {
            // Smoothstep across the feather in angle.
            let a = cos_a.acos();
            let (ai, ao) = (l.cos_inner.acos(), l.cos_outer.acos());
            let t = ((a - ai) / (ao - ai).max(1e-6)).clamp(0.0, 1.0);
            1.0 - t * t * (3.0 - 2.0 * t)
        };
    }
    k
}

/// Inputs of one fragment.
#[derive(Clone, Copy, Debug)]
pub struct Frag {
    pub pos: [f32; 3],
    /// Interpolated (unnormalised) vertex normal.
    pub normal: [f32; 3],
    pub uv: [f32; 2],
    pub tangent: [f32; 4],
    pub material: u32,
    pub front: bool,
}

/// Shade a fragment: premultiplied linear RGBA, or `None` when discarded (alpha test, back
/// face of a single-sided material).
pub fn shade(s: &Scene, f: &Frag) -> Option<[f32; 4]> {
    let m: &Material = s.materials.get(f.material as usize)?;
    if !f.front && !m.double_sided {
        return None;
    }
    let tex = |i: i32| -> Option<[f32; 4]> { (i >= 0).then(|| s.textures.get(i as usize).map(|t| sample(t, &s.texels, f.uv[0], f.uv[1]))).flatten() };
    let mut base = m.base;
    if let Some(t) = tex(m.tex_base) {
        for k in 0..4 {
            base[k] *= t[k];
        }
    }
    let alpha = base[3] * m.opacity;
    if m.alpha_mode == 1 && alpha < m.alpha_cutoff {
        return None;
    }
    if alpha <= 1e-5 && m.alpha_mode != 0 {
        return None;
    }
    let alpha = if m.alpha_mode == 0 { m.opacity } else { alpha };
    let rgb = [base[0], base[1], base[2]];
    let mut emissive = m.emissive;
    if let Some(t) = tex(m.tex_emissive) {
        emissive = [emissive[0] * t[0], emissive[1] * t[1], emissive[2] * t[2]];
    }
    if m.shadows_only {
        return Some(shadow_only(s, m, f, alpha));
    }
    if m.unlit || !m.accepts_lights || (s.lights.is_empty() && s.env.is_none() && s.ambient == [0.0; 3]) {
        let c = add(rgb, emissive);
        return Some([c[0] * alpha, c[1] * alpha, c[2] * alpha, alpha]);
    }
    let (mut metallic, mut rough) = (m.metallic, m.roughness);
    if let Some(t) = tex(m.tex_mr) {
        rough *= t[1];
        metallic *= t[2];
    }
    let mut n = norm(f.normal);
    if !f.front {
        n = scale(n, -1.0);
    }
    // Shadow lookups follow the geometric surface, not the normal map.
    let ng = n;
    if let Some(t) = tex(m.tex_normal) {
        let tn = [(t[0] * 2.0 - 1.0) * m.normal_scale, (t[1] * 2.0 - 1.0) * m.normal_scale, t[2] * 2.0 - 1.0];
        let tg = [f.tangent[0], f.tangent[1], f.tangent[2]];
        // Gram-Schmidt against the shading normal.
        let tg = norm(sub(tg, scale(n, dot(n, tg))));
        let sign = if f.front { f.tangent[3] } else { -f.tangent[3] };
        let bt = scale(cross(n, tg), sign);
        n = norm(add(add(scale(tg, tn[0]), scale(bt, tn[1])), scale(n, tn[2])));
    }
    let ao = match tex(m.tex_occlusion) {
        Some(t) => 1.0 + m.occlusion_strength * (t[0] - 1.0),
        None => 1.0,
    };
    let v = if s.ortho { scale(s.cam_fwd, -1.0) } else { norm(sub(s.eye, f.pos)) };
    let mut out = [0.0f32; 3];
    for l in &s.lights {
        let (to_l, dist) = to_light(l, f.pos);
        let k = attenuation(l, f.pos, to_l, dist);
        if k <= 0.0 {
            continue;
        }
        let n_l = dot(n, to_l);
        if n_l <= 0.0 {
            continue;
        }
        let sh = if m.receives_shadows { light_shadow(s, l, f.pos, ng, n_l) } else { 1.0 };
        let b = brdf(BrdfSurface { base: rgb, metallic, roughness: rough, specular: m.specular_k, diffuse: m.diffuse_k }, n, v, to_l);
        for c in 0..3 {
            out[c] += b[c] * l.color[c] * k * sh;
        }
    }
    for c in 0..3 {
        out[c] += s.ambient[c] * rgb[c] * m.ambient_k * ao;
    }
    if let Some(e) = &s.env {
        let n_v = dot(n, v).clamp(1e-4, 1.0);
        let r = sub(scale(n, 2.0 * dot(n, v)), v);
        let (irr, spec) = env_light(s, e, n, r, rough);
        let f0 = [mix(0.04 * m.specular_k, rgb[0], metallic), mix(0.04 * m.specular_k, rgb[1], metallic), mix(0.04 * m.specular_k, rgb[2], metallic)];
        let (a, b) = env_brdf(n_v, rough);
        // The Environment light's shadow darkens its own diffuse and specular.
        let sh = if m.receives_shadows { env_shadow(s, e, f.pos, ng, n) } else { 1.0 };
        for c in 0..3 {
            let fs = f0[c] * a + b;
            out[c] += (irr[c] * rgb[c] * (1.0 - fs) * (1.0 - metallic) * m.diffuse_k + spec[c] * fs) * ao * sh;
        }
    }
    let c = add(out, emissive);
    Some([c[0] * alpha, c[1] * alpha, c[2] * alpha, alpha])
}

/// "Accepts Shadows: Only" (classic `compose::shade_texel`): black at the strongest shadow any
/// light casts on the fragment, nothing where it is unshadowed or takes no lights.
fn shadow_only(s: &Scene, m: &Material, f: &Frag, alpha: f32) -> [f32; 4] {
    if m.unlit || !m.accepts_lights {
        return [0.0; 4];
    }
    let n = if f.front { norm(f.normal) } else { scale(norm(f.normal), -1.0) };
    let mut amount = 0.0f32;
    for l in &s.lights {
        let (to_l, dist) = to_light(l, f.pos);
        if attenuation(l, f.pos, to_l, dist) <= 0.0 {
            continue;
        }
        let n_l = dot(n, to_l);
        if n_l <= 0.0 {
            continue;
        }
        amount = amount.max(1.0 - light_shadow(s, l, f.pos, n, n_l));
    }
    // The Environment light's shadow counts too (it reaches every surface: no falloff).
    if let Some(e) = &s.env {
        amount = amount.max(1.0 - env_shadow(s, e, f.pos, n, n));
    }
    [0.0, 0.0, 0.0, alpha * amount]
}

//! Shading math of the Advanced 3D renderer that the scene builder uses: vector helpers, the
//! sRGB-to-linear curve, the BRDF terms, texture sampling, equirectangular mapping and
//! shadow-map filtering. Fragments are shaded by `craft_raster3d`, and `crates/gpu`'s
//! `advanced3d.wgsl` repeats that shading step for step.
//!
//! - Direct light: Cook–Torrance microfacet specular with the GGX/Trowbridge–Reitz normal
//!   distribution, the Smith–Schlick-GGX geometry term (`k = (r + 1)² / 8`) and Schlick's
//!   Fresnel (`F0` = 0.04 for dielectrics, the base colour for metals), plus a Lambertian
//!   diffuse lobe weighted by `(1 − F)(1 − metallic)` (Karis 2013, "Real Shading in Unreal
//!   Engine 4"; the glTF 2.0 metallic-roughness model). Light colours are the irradiance on a
//!   surface facing the light (a white 100% light shows a white diffuse surface at its base
//!   colour), i.e. π is folded into the light intensity.
//! - Image-based light: the analytic split-sum environment BRDF fit (Karis 2014, "Physically
//!   Based Shading on Mobile").
//! - Shadows: depth maps per light, percentage-closer filtered.

use super::scene::{ShadowMap, TexInfo};

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
fn mix(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// sRGB-encoded → linear light.
#[inline]
pub fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
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

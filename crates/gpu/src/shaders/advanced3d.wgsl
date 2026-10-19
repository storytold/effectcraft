// Advanced 3D: rasterised meshes with physically based shading. A step-for-step port of
// `effectcraft_render::three_d::adv::shade` (Cook–Torrance GGX + Lambert, image-based light
// from an equirectangular environment, PCF shadow maps); the CPU rasteriser is the reference.

struct Globals {
    clip: mat4x4<f32>,
    // Camera-space depth row (x, y, z, w).
    view_z: vec4<f32>,
    // eye.xyz, ortho (0/1)
    eye: vec4<f32>,
    // cam_fwd.xyz, light count
    fwd: vec4<f32>,
    // ambient.rgb, environment present (0/1)
    ambient: vec4<f32>,
    // radiance first texture, mip count, irradiance texture, intensity
    env: vec4<f32>,
    // rotation, any lighting (0/1), -, -
    env2: vec4<f32>,
};

@group(0) @binding(0) var<uniform> g: Globals;
@group(0) @binding(1) var<storage, read> materials: array<vec4<f32>>;
@group(0) @binding(2) var<storage, read> texinfo: array<vec4<u32>>;
@group(0) @binding(3) var<storage, read> texels: array<vec4<f32>>;
@group(0) @binding(4) var<storage, read> lights: array<vec4<f32>>;
@group(0) @binding(5) var<storage, read> shadows: array<vec4<f32>>;
@group(0) @binding(6) var<storage, read> shadow_texels: array<f32>;

const PI: f32 = 3.14159265358979;
const MAT_STRIDE: u32 = 6u;
const LIGHT_STRIDE: u32 = 5u;
const SHADOW_STRIDE: u32 = 5u;

struct VIn {
    @location(0) pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) tangent: vec4<f32>,
    @location(4) material: u32,
};

struct VOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) tangent: vec4<f32>,
    @location(4) @interpolate(flat) material: u32,
};

@vertex
fn vs(v: VIn) -> VOut {
    var o: VOut;
    o.clip = g.clip * vec4<f32>(v.pos, 1.0);
    o.pos = v.pos;
    o.normal = v.normal;
    o.uv = v.uv;
    o.tangent = v.tangent;
    o.material = v.material;
    return o;
}

fn norm3(a: vec3<f32>) -> vec3<f32> {
    let l = sqrt(dot(a, a));
    if (l > 1e-20) {
        return a / l;
    }
    return vec3<f32>(0.0, 0.0, -1.0);
}

fn srgb_to_linear(c: f32) -> f32 {
    if (c <= 0.04045) {
        return c / 12.92;
    }
    return pow((c + 0.055) / 1.055, 2.4);
}

// `a` mod `n` in [0, n) for n > 0 (floored). Never `%` on a negative operand: WGSL's `%` truncates,
// but naga's GLSL output is plain `%`, undefined for negative operands (wrong on Mesa's GL).
fn imod(a: i32, n: i32) -> i32 {
    if (a >= 0) {
        return a % n;
    }
    return n - 1 - (-(a + 1)) % n;
}

fn wrap_i(i: i32, n: i32, mode: u32) -> i32 {
    if (mode == 1u) {
        return clamp(i, 0, n - 1);
    }
    if (mode == 2u) {
        let p = 2 * n;
        let m = imod(i, p);
        if (m >= n) {
            return p - 1 - m;
        }
        return m;
    }
    return imod(i, n);
}

fn texel(t: vec4<u32>, x: i32, y: i32) -> vec4<f32> {
    let w = i32(t.y);
    let h = i32(t.z);
    let xi = wrap_i(x, w, t.w & 255u);
    let yi = wrap_i(y, h, (t.w >> 8u) & 255u);
    return texels[t.x + u32(yi * w + xi)];
}

fn sample_tex(i: i32, uv: vec2<f32>) -> vec4<f32> {
    let t = texinfo[u32(i)];
    if (t.y == 0u || t.z == 0u) {
        return vec4<f32>(1.0);
    }
    let x = uv.x * f32(t.y) - 0.5;
    let y = uv.y * f32(t.z) - 0.5;
    let x0 = floor(x);
    let y0 = floor(y);
    let fx = x - x0;
    let fy = y - y0;
    let xi = i32(x0);
    let yi = i32(y0);
    let a = texel(t, xi, yi);
    let b = texel(t, xi + 1, yi);
    let c = texel(t, xi, yi + 1);
    let d = texel(t, xi + 1, yi + 1);
    let top = a + (b - a) * fx;
    let bot = c + (d - c) * fx;
    return top + (bot - top) * fy;
}

fn d_ggx(n_h: f32, a: f32) -> f32 {
    let a2 = a * a;
    let d = n_h * n_h * (a2 - 1.0) + 1.0;
    return a2 / max(PI * d * d, 1e-12);
}

fn g1(x: f32, k: f32) -> f32 {
    return x / (x * (1.0 - k) + k);
}

fn brdf(base: vec3<f32>, metallic: f32, rough_in: f32, spec_k: f32, diff_k: f32, n: vec3<f32>, v: vec3<f32>, l: vec3<f32>) -> vec3<f32> {
    let n_l = dot(n, l);
    if (n_l <= 0.0) {
        return vec3<f32>(0.0);
    }
    let n_v = max(dot(n, v), 1e-4);
    let h = norm3(v + l);
    let n_h = max(dot(n, h), 0.0);
    let v_h = max(dot(v, h), 0.0);
    let r = clamp(rough_in, 0.03, 1.0);
    let f0 = mix(vec3<f32>(0.04 * spec_k), base, metallic);
    let fk = pow(clamp(1.0 - v_h, 0.0, 1.0), 5.0);
    let f = f0 + (vec3<f32>(1.0) - f0) * fk;
    let d = d_ggx(n_h, r * r);
    let k = (r + 1.0) * (r + 1.0) / 8.0;
    let gg = g1(n_v, k) * g1(n_l, k);
    let spec = d * gg / max(4.0 * n_v * n_l, 1e-4);
    let kd = (vec3<f32>(1.0) - f) * (1.0 - metallic);
    return (kd * base / PI * diff_k + f * spec) * n_l * PI;
}

fn env_brdf(n_v: f32, rough: f32) -> vec2<f32> {
    let c0 = vec4<f32>(-1.0, -0.0275, -0.572, 0.022);
    let c1 = vec4<f32>(1.0, 0.0425, 1.04, -0.04);
    let r = rough * c0 + c1;
    let a004 = min(r.x * r.x, exp2(-9.28 * n_v)) * r.x + r.y;
    return vec2<f32>(-1.04 * a004 + r.z, 1.04 * a004 + r.w);
}

fn equirect_uv(d: vec3<f32>, rot: f32) -> vec2<f32> {
    // atan2(0, 0) is 0 on the CPU but may be NaN on GPUs (straight up / down normals).
    var a = 0.0;
    if (d.x != 0.0 || d.z != 0.0) {
        a = atan2(d.x, d.z);
    }
    let u = 0.5 + (a + rot) / (2.0 * PI);
    let v = acos(clamp(-d.y, -1.0, 1.0)) / PI;
    return vec2<f32>(u - floor(u), v);
}

fn shadow_factor(m: u32, p: vec3<f32>, n_l: f32) -> f32 {
    let r0 = shadows[m];
    let r1 = shadows[m + 1u];
    let r2 = shadows[m + 2u];
    let info = shadows[m + 3u];
    let extra = shadows[m + 4u];
    let q = vec3<f32>(dot(r0.xyz, p) + r0.w, dot(r1.xyz, p) + r1.w, dot(r2.xyz, p) + r2.w);
    let focal = info.x;
    let ortho = info.y > 0.5;
    let size = bitcast<u32>(info.z);
    let offset = bitcast<u32>(info.w);
    let c = f32(size) * 0.5;
    var u: f32;
    var v: f32;
    if (ortho) {
        u = q.x * focal + c;
        v = q.y * focal + c;
    } else {
        if (q.z <= 1e-3) {
            return 1.0;
        }
        u = q.x * focal / q.z + c;
        v = q.y * focal / q.z + c;
    }
    let z = q.z;
    let texel = select(abs(z) / focal, 1.0 / focal, ortho);
    let bias = extra.x * texel * (1.0 + extra.y) * (1.0 + 3.0 * (1.0 - clamp(n_l, 0.0, 1.0)));
    let step = extra.y / 2.0;
    let sz = i32(size);
    var lit = 0.0;
    for (var j = -2; j <= 2; j = j + 1) {
        for (var i = -2; i <= 2; i = i + 1) {
            let x = i32(floor(u + f32(i) * step));
            let y = i32(floor(v + f32(j) * step));
            if (x < 0 || y < 0 || x >= sz || y >= sz) {
                lit = lit + 1.0;
                continue;
            }
            let d = shadow_texels[offset + u32(y * sz + x)];
            if (z - bias <= d) {
                lit = lit + 1.0;
            }
        }
    }
    return lit / 25.0;
}

fn light_shadow(li: u32, kind: u32, lpos: vec3<f32>, shadow: i32, darkness: f32, p: vec3<f32>, n_l: f32) -> f32 {
    if (shadow < 0) {
        return 1.0;
    }
    var face = 0u;
    if (kind == 2u) {
        let d = p - lpos;
        let a = abs(d);
        if (a.x >= a.y && a.x >= a.z) {
            face = select(1u, 0u, d.x > 0.0);
        } else if (a.y >= a.z) {
            face = select(3u, 2u, d.y > 0.0);
        } else {
            face = select(5u, 4u, d.z > 0.0);
        }
    }
    let f = shadow_factor((u32(shadow) + face) * SHADOW_STRIDE, p, n_l);
    return 1.0 - darkness * (1.0 - f);
}

fn attenuation(kind: u32, falloff: u32, radius: f32, falloff_distance: f32, cos_inner: f32, cos_outer: f32, dir: vec3<f32>, to_l: vec3<f32>, dist: f32) -> f32 {
    var k = 1.0;
    if (falloff == 1u && kind != 0u) {
        if (dist <= radius) {
            k = 1.0;
        } else if (falloff_distance <= 0.0) {
            k = 0.0;
        } else {
            let t = clamp((dist - radius) / falloff_distance, 0.0, 1.0);
            k = 1.0 - t * t * (3.0 - 2.0 * t);
        }
    } else if (falloff == 2u && kind != 0u) {
        if (dist <= radius || (radius <= 0.0 && dist <= 1.0)) {
            k = 1.0;
        } else {
            let q = max(radius, 1.0) / dist;
            k = q * q;
        }
    }
    if (kind == 1u) {
        let cos_a = clamp(dot(-to_l, dir), -1.0, 1.0);
        if (cos_a >= cos_inner) {
            k = k * 1.0;
        } else if (cos_a <= cos_outer) {
            k = 0.0;
        } else {
            let a = acos(cos_a);
            let ai = acos(cos_inner);
            let ao = acos(cos_outer);
            let t = clamp((a - ai) / max(ao - ai, 1e-6), 0.0, 1.0);
            k = k * (1.0 - t * t * (3.0 - 2.0 * t));
        }
    }
    return k;
}

struct FOut {
    @location(0) color: vec4<f32>,
    // Camera depth's f32 bits (an `R32Uint` target, see `adv3d.rs`).
    @location(1) depth: u32,
};

// shade::to_light: unit direction to the light (xyz) and its distance (w).
fn to_light(kind: u32, lpos: vec3<f32>, ldir: vec3<f32>, pos: vec3<f32>) -> vec4<f32> {
    if (kind == 0u) {
        return vec4<f32>(-ldir, 1e30);
    }
    let d = lpos - pos;
    let len = sqrt(dot(d, d));
    if (len < 1e-6) {
        return vec4<f32>(0.0, 0.0, -1.0, 0.0);
    }
    return vec4<f32>(d / len, len);
}

// shade::shadow_only: the strongest shadow any light casts on the fragment.
fn shadow_only_amount(unlit: bool, normal: vec3<f32>, front: bool, pos: vec3<f32>) -> f32 {
    var amount = 0.0;
    if (unlit) {
        return amount;
    }
    var n = norm3(normal);
    if (!front) {
        n = -n;
    }
    let nl_count = u32(g.fwd.w);
    for (var li = 0u; li < nl_count; li = li + 1u) {
        let lb = li * LIGHT_STRIDE;
        let l0 = lights[lb];
        let l1 = lights[lb + 1u];
        let l2 = lights[lb + 2u];
        let l3 = lights[lb + 3u];
        let l4 = lights[lb + 4u];
        let kind = u32(l0.w);
        let tl = to_light(kind, l0.xyz, l1.xyz, pos);
        let to_l = tl.xyz;
        let dist = tl.w;
        if (attenuation(kind, u32(l3.x), l3.y, l3.z, l2.w, l1.w, l1.xyz, to_l, dist) <= 0.0) {
            continue;
        }
        let n_l = dot(n, to_l);
        if (n_l <= 0.0) {
            continue;
        }
        amount = max(amount, 1.0 - light_shadow(li, kind, l0.xyz, i32(l3.w), l4.x, pos, n_l));
    }
    return amount;
}

@fragment
fn fs(i: VOut, @builtin(front_facing) front: bool) -> FOut {
    let mb = i.material * MAT_STRIDE;
    let m0 = materials[mb];
    let m1 = materials[mb + 1u];
    let m2 = materials[mb + 2u];
    let m3 = materials[mb + 3u];
    let m4 = materials[mb + 4u];
    let m5 = materials[mb + 5u];
    let flags = u32(m4.y);
    let double_sided = (flags & 1u) != 0u;
    let unlit = (flags & 2u) != 0u;
    let accepts_lights = (flags & 4u) != 0u;
    let receives_shadows = (flags & 8u) != 0u;
    let shadows_only = (flags & 16u) != 0u;
    let alpha_mode = (flags >> 8u) & 3u;
    if (!front && !double_sided) {
        discard;
    }
    var base = m0;
    let tex_base = i32(m3.x);
    if (tex_base >= 0) {
        base = base * sample_tex(tex_base, i.uv);
    }
    let opacity = m5.y;
    var alpha = base.a * opacity;
    if (alpha_mode == 1u && alpha < m2.w) {
        discard;
    }
    if (alpha <= 1e-5 && alpha_mode != 0u) {
        discard;
    }
    if (alpha_mode == 0u) {
        alpha = opacity;
    }
    let rgb = base.rgb;
    var emissive = m1.xyz;
    let tex_em = i32(m4.x);
    if (tex_em >= 0) {
        emissive = emissive * sample_tex(tex_em, i.uv).rgb;
    }
    var out: FOut;
    out.depth = bitcast<u32>(dot(g.view_z.xyz, i.pos) + g.view_z.w);
    if (shadows_only) {
        out.color = vec4<f32>(0.0, 0.0, 0.0, alpha * shadow_only_amount(unlit || !accepts_lights, i.normal, front, i.pos));
        return out;
    }
    if (unlit || !accepts_lights || g.env2.y < 0.5) {
        let c = rgb + emissive;
        out.color = vec4<f32>(c * alpha, alpha);
        return out;
    }
    var metallic = m1.w;
    var rough = m2.x;
    let tex_mr = i32(m3.y);
    if (tex_mr >= 0) {
        let t = sample_tex(tex_mr, i.uv);
        rough = rough * t.y;
        metallic = metallic * t.z;
    }
    var n = norm3(i.normal);
    if (!front) {
        n = -n;
    }
    let tex_n = i32(m3.z);
    if (tex_n >= 0) {
        let t = sample_tex(tex_n, i.uv);
        let ns = m2.y;
        let tn = vec3<f32>((t.x * 2.0 - 1.0) * ns, (t.y * 2.0 - 1.0) * ns, t.z * 2.0 - 1.0);
        let tg = norm3(i.tangent.xyz - n * dot(n, i.tangent.xyz));
        let sign = select(-i.tangent.w, i.tangent.w, front);
        let bt = cross(n, tg) * sign;
        n = norm3(tg * tn.x + bt * tn.y + n * tn.z);
    }
    var ao = 1.0;
    let tex_ao = i32(m3.w);
    if (tex_ao >= 0) {
        ao = 1.0 + m2.z * (sample_tex(tex_ao, i.uv).x - 1.0);
    }
    var v: vec3<f32>;
    if (g.eye.w > 0.5) {
        v = -g.fwd.xyz;
    } else {
        v = norm3(g.eye.xyz - i.pos);
    }
    let diff_k = m4.z;
    let spec_k = m4.w;
    let amb_k = m5.x;
    var acc = vec3<f32>(0.0);
    let nl_count = u32(g.fwd.w);
    for (var li = 0u; li < nl_count; li = li + 1u) {
        let lb = li * LIGHT_STRIDE;
        let l0 = lights[lb];
        let l1 = lights[lb + 1u];
        let l2 = lights[lb + 2u];
        let l3 = lights[lb + 3u];
        let l4 = lights[lb + 4u];
        let kind = u32(l0.w);
        let tl = to_light(kind, l0.xyz, l1.xyz, i.pos);
        let to_l = tl.xyz;
        let dist = tl.w;
        let k = attenuation(kind, u32(l3.x), l3.y, l3.z, l2.w, l1.w, l1.xyz, to_l, dist);
        if (k <= 0.0) {
            continue;
        }
        let n_l = dot(n, to_l);
        if (n_l <= 0.0) {
            continue;
        }
        var sh = 1.0;
        if (receives_shadows) {
            sh = light_shadow(li, kind, l0.xyz, i32(l3.w), l4.x, i.pos, n_l);
        }
        let b = brdf(rgb, metallic, rough, spec_k, diff_k, n, v, to_l);
        acc = acc + b * l2.xyz * k * sh;
    }
    acc = acc + g.ambient.xyz * rgb * amb_k * ao;
    if (g.ambient.w > 0.5) {
        let n_v = clamp(dot(n, v), 1e-4, 1.0);
        let r = n * (2.0 * dot(n, v)) - v;
        let rot = g.env2.x;
        let irr = sample_tex(i32(g.env.z), equirect_uv(n, rot)).rgb * g.env.w;
        let mips = u32(g.env.y);
        let lv = clamp(rough, 0.0, 1.0) * f32(max(mips, 1u) - 1u);
        let lo = u32(floor(lv));
        let hi = min(lo + 1u, max(mips, 1u) - 1u);
        let f = lv - f32(lo);
        let ruv = equirect_uv(r, rot);
        let sa = sample_tex(i32(g.env.x) + i32(lo), ruv).rgb;
        let sb = sample_tex(i32(g.env.x) + i32(hi), ruv).rgb;
        let spec = (sa + (sb - sa) * f) * g.env.w;
        let f0 = mix(vec3<f32>(0.04 * spec_k), rgb, metallic);
        let ab = env_brdf(n_v, rough);
        let fs = f0 * ab.x + vec3<f32>(ab.y);
        acc = acc + (irr * rgb * (vec3<f32>(1.0) - fs) * (1.0 - metallic) * diff_k + spec * fs) * ao;
    }
    let c = acc + emissive;
    out.color = vec4<f32>(c * alpha, alpha);
    return out;
}

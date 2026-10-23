// Classic 3D on the GPU: the per-pixel compositor of `render::three_d::compose` (draw_run).
//
// Every plane covering an output pixel is inverse-mapped through its homography (one per
// motion-blur sub-sample), sampled from the plane atlas (`src`), shaded by the comp's lights
// (Blinn-Phong, ray-cast shadows against the caster planes) and multiplied by its track matte;
// the pixel's fragments are insertion-sorted far → near (coplanar fragments in stack order) and
// blended over the canvas (`aux`) with each layer's mode and opacity.
//
// Bindings: group 0 as every kernel (P, src = atlas, aux = canvas, out, data = track mattes);
// group 1 = planes, geometries, lights, caster indices.
// P.u[0] = (planes, lights, casters, _)

struct Plane {
    // Atlas rectangle (x, y, w, h) in texels.
    atlas: vec4<f32>,
    // Buffer offset (x, y), scale, bicubic (0 / 1).
    tex: vec4<f32>,
    // Output pixel bounds [x0, y0, x1, y1).
    bbox: vec4<f32>,
    // Layer-space bounds of the buffer.
    bounds: vec4<f32>,
    // World → layer (affine rows).
    winv0: vec4<f32>,
    winv1: vec4<f32>,
    winv2: vec4<f32>,
    // Ambient, diffuse, specular, Blinn-Phong exponent.
    mat0: vec4<f32>,
    // Metal, light transmission, opacity.
    mat1: vec4<f32>,
    // First geometry, geometry count, flags (1 draw, 2 preserve transparency, 4 accepts
    // lights), blend mode.
    ints: vec4<u32>,
    // Casts shadows, accepts shadows (0 off, 1 on, 2 only), layer id, stack order.
    ints2: vec4<u32>,
    // Track matte (index + 1; 0 = none), dissolve seed.
    ints3: vec4<u32>,
}

struct Geo {
    // Output pixel → homogeneous layer coordinates (rows).
    h0: vec4<f32>,
    h1: vec4<f32>,
    h2: vec4<f32>,
    // Camera depth = d.x·u + d.y·v + d.z.
    depth: vec4<f32>,
    // Layer → world (affine rows).
    w0: vec4<f32>,
    w1: vec4<f32>,
    w2: vec4<f32>,
    // Unit world normal.
    normal: vec4<f32>,
    // Camera eye (xyz) and orthographic flag (w).
    eye: vec4<f32>,
    // Camera forward (xyz).
    fwd: vec4<f32>,
}

struct Light {
    // Kind (0 ambient, 1 parallel, 2 point, 3 spot), falloff, casts shadows.
    ints: vec4<u32>,
    pos: vec4<f32>,
    dir: vec4<f32>,
    color: vec4<f32>,
    // Cone half-angle, feather, radius, falloff distance.
    cone: vec4<f32>,
    // Shadow darkness, shadow diffusion.
    shadow: vec4<f32>,
}

@group(1) @binding(0) var<storage, read> planes: array<Plane>;
@group(1) @binding(1) var<storage, read> geos: array<Geo>;
@group(1) @binding(2) var<storage, read> lights: array<Light>;
@group(1) @binding(3) var<storage, read> casters: array<u32>;

const NEAR3: f32 = 1.0;
const MAX_FRAGS: u32 = 32u;

fn atlas_get(i: u32, x: i32, y: i32) -> vec4<f32> {
    let r = planes[i].atlas;
    if (x < 0 || y < 0 || x >= i32(r.z) || y >= i32(r.w)) {
        return vec4<f32>(0.0);
    }
    return textureLoad(src, vec2<i32>(i32(r.x) + x, i32(r.y) + y), 0);
}

// Image::sample_bilinear / sample_bicubic on a plane's buffer at layer coordinates (u, v).
fn texel(i: u32, u: f32, v: f32) -> vec4<f32> {
    let t = planes[i].tex;
    let x = u * t.z + t.x;
    let y = v * t.z + t.y;
    let fx = x - 0.5;
    let fy = y - 0.5;
    let x0 = floor(fx);
    let y0 = floor(fy);
    let xi = i32(x0);
    let yi = i32(y0);
    if (t.w > 0.5) {
        let wx = cubic_w(fx - x0);
        let wy = cubic_w(fy - y0);
        var o = vec4<f32>(0.0);
        for (var j = 0; j < 4; j++) {
            for (var k = 0; k < 4; k++) {
                o += atlas_get(i, xi - 1 + k, yi - 1 + j) * (wx[k] * wy[j]);
            }
        }
        return max(o, vec4<f32>(0.0));
    }
    let tx = fx - x0;
    let ty = fy - y0;
    let a = atlas_get(i, xi, yi);
    let b = atlas_get(i, xi + 1, yi);
    let c = atlas_get(i, xi, yi + 1);
    let d = atlas_get(i, xi + 1, yi + 1);
    let top = a + (b - a) * tx;
    let bot = c + (d - c) * tx;
    return top + (bot - top) * ty;
}

fn affine(r0: vec4<f32>, r1: vec4<f32>, r2: vec4<f32>, p: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(dot(r0.xyz, p) + r0.w, dot(r1.xyz, p) + r1.w, dot(r2.xyz, p) + r2.w);
}

fn linear3(r0: vec4<f32>, r1: vec4<f32>, r2: vec4<f32>, v: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(dot(r0.xyz, v), dot(r1.xyz, v), dot(r2.xyz, v));
}

// compose::unproject: (u, v, depth, ok).
fn unproject(g: u32, sx: f32, sy: f32) -> vec4<f32> {
    let gm = geos[g];
    let ortho = gm.eye.w > 0.5;
    let qz = gm.h2.x * sx + gm.h2.y * sy + gm.h2.z;
    if ((!ortho && qz <= 0.0) || qz == 0.0) {
        return vec4<f32>(0.0);
    }
    let u = (gm.h0.x * sx + gm.h0.y * sy + gm.h0.z) / qz;
    let v = (gm.h1.x * sx + gm.h1.y * sy + gm.h1.z) / qz;
    let z = gm.depth.x * u + gm.depth.y * v + gm.depth.z;
    if (!ortho && z < NEAR3) {
        return vec4<f32>(0.0);
    }
    return vec4<f32>(u, v, z, 1.0);
}

// LightState::to_light: unit vector towards the light (xyz) and the distance (w; -1 = ∞).
fn to_light(l: u32, p: vec3<f32>) -> vec4<f32> {
    let li = lights[l];
    if (li.ints.x == 1u) {
        return vec4<f32>(-li.dir.xyz, -1.0);
    }
    let d = li.pos.xyz - p;
    let len = length(d);
    if (len < 1e-9) {
        return vec4<f32>(0.0, 0.0, -1.0, 0.0);
    }
    return vec4<f32>(d / len, len);
}

fn smoothstep01(t: f32) -> f32 {
    return 1.0 - t * t * (3.0 - 2.0 * t);
}

// LightState::attenuation.
fn attenuation(l: u32, p: vec3<f32>) -> f32 {
    let li = lights[l];
    let tl = to_light(l, p);
    let dist = tl.w;
    let finite = dist >= 0.0;
    let radius = li.cone.z;
    let fd = li.cone.w;
    var k = 1.0;
    if (li.ints.y == 1u && finite) {
        if (dist <= radius) {
            k = 1.0;
        } else if (fd <= 0.0) {
            k = 0.0;
        } else {
            k = smoothstep01(clamp((dist - radius) / fd, 0.0, 1.0));
        }
    } else if (li.ints.y == 2u && finite) {
        if (dist <= radius || (radius <= 0.0 && dist <= 1.0)) {
            k = 1.0;
        } else {
            let q = max(radius, 1.0) / dist;
            k = q * q;
        }
    }
    if (li.ints.x == 3u) {
        let cos_a = clamp(dot(-tl.xyz, li.dir.xyz), -1.0, 1.0);
        let a = acos_p(cos_a);
        let outer = li.cone.x;
        let inner = outer * (1.0 - li.cone.y);
        if (a <= inner) {
        } else if (a >= outer) {
            k = 0.0;
        } else {
            k *= smoothstep01((a - inner) / max(outer - inner, 1e-9));
        }
    }
    return k;
}

// compose::occlusion: RGB transmittance of light `l` reaching `p` past caster `c`.
fn occlusion(c: u32, p: vec3<f32>, l: u32) -> vec3<f32> {
    let pl = planes[c];
    let li = lights[l];
    let a = affine(pl.winv0, pl.winv1, pl.winv2, p);
    var hit: vec3<f32>;
    var d_pc: f32;
    var d_cl: f32;
    if (li.ints.x == 1u) {
        // Towards the light, 1e6 away (in direction form: exact in f32).
        let dl = linear3(pl.winv0, pl.winv1, pl.winv2, -li.dir.xyz);
        if (dl.z == 0.0) {
            return vec3<f32>(1.0);
        }
        let s = -a.z / dl.z;
        if (s <= 1.0 || s >= 1.0e6) {
            return vec3<f32>(1.0);
        }
        hit = a + dl * s;
        d_pc = length(li.dir.xyz) * s;
        d_cl = 1000.0;
    } else {
        let b = affine(pl.winv0, pl.winv1, pl.winv2, li.pos.xyz);
        if (a.z == b.z || (a.z < 0.0) == (b.z < 0.0)) {
            return vec3<f32>(1.0);
        }
        let t = a.z / (a.z - b.z);
        if (t <= 1e-6 || t >= 1.0) {
            return vec3<f32>(1.0);
        }
        hit = a + (b - a) * t;
        let seg = length(li.pos.xyz - p);
        d_pc = seg * t;
        d_cl = max(seg * (1.0 - t), 1.0);
    }
    let r = li.shadow.y * min(d_pc / d_cl, 4.0);
    let bd = pl.bounds;
    if (hit.x < bd.x - r || hit.y < bd.y - r || hit.x > bd.z + r || hit.y > bd.w + r) {
        return vec3<f32>(1.0);
    }
    var px = vec4<f32>(0.0);
    if (r * pl.tex.z < 0.5) {
        px = texel(c, hit.x, hit.y);
    } else {
        for (var j = -1; j <= 1; j++) {
            for (var i = -1; i <= 1; i++) {
                px += texel(c, hit.x + f32(i) * r * 0.66, hit.y + f32(j) * r * 0.66) / 9.0;
            }
        }
    }
    let alpha = clamp(px.w * pl.mat1.z, 0.0, 1.0);
    if (alpha <= 0.0) {
        return vec3<f32>(1.0);
    }
    let tr = pl.mat1.y;
    let dark = li.shadow.x;
    // Per channel, as whole vectors: FXC (DirectX 12) can't write a vector component through a
    // loop index without unrolling every loop around the call, and fails (#613).
    var st = vec3<f32>(0.0);
    if (px.w > 0.0) {
        st = clamp(px.xyz / px.w, vec3<f32>(0.0), vec3<f32>(1.0));
    }
    let through = (1.0 - alpha) + alpha * tr * st;
    return 1.0 - dark * (1.0 - through);
}

fn shadow_of(it: u32, l: u32, p: vec3<f32>) -> vec3<f32> {
    var tr = vec3<f32>(1.0);
    let n = P.u[0].z;
    let lid = planes[it].ints2.z;
    for (var k = 0u; k < n; k++) {
        let c = casters[k];
        if (c == it || planes[c].ints2.z == lid) {
            continue;
        }
        tr *= occlusion(c, p, l);
    }
    return tr;
}

// compose::shade_texel (light::shade).
fn shade_texel(it: u32, g: u32, u: f32, v: f32, t: vec4<f32>) -> vec4<f32> {
    let pl = planes[it];
    let n_lights = P.u[0].y;
    let accepts_shadows = pl.ints2.y;
    if (n_lights == 0u || (pl.ints.z & 4u) == 0u) {
        if (accepts_shadows == 2u) {
            return vec4<f32>(0.0);
        }
        return t;
    }
    let gm = geos[g];
    let a = t.w;
    let rgb = t.xyz / a;
    let p = affine(gm.w0, gm.w1, gm.w2, vec3<f32>(u, v, 0.0));
    var to_viewer: vec3<f32>;
    if (gm.eye.w > 0.5) {
        to_viewer = -gm.fwd.xyz;
    } else {
        to_viewer = normalize(gm.eye.xyz - p);
    }
    var n = gm.normal.xyz;
    if (dot(n, to_viewer) < 0.0) {
        n = -n;
    }
    var amb = vec3<f32>(0.0);
    var diff = vec3<f32>(0.0);
    var spec = vec3<f32>(0.0);
    var shadow_amt = 0.0;
    let ex = pl.mat0.w;
    let metal = pl.mat1.x;
    let transmission = pl.mat1.y;
    let specular = pl.mat0.z;
    for (var i = 0u; i < n_lights; i++) {
        let li = lights[i];
        if (li.ints.x == 0u) {
            amb += li.color.xyz;
            continue;
        }
        let k = attenuation(i, p);
        if (k <= 0.0) {
            continue;
        }
        let lv = to_light(i, p).xyz;
        let ndl = dot(n, lv);
        let front = ndl > 0.0;
        var lambert = ndl;
        if (!front) {
            lambert = -ndl * transmission;
        }
        if (lambert <= 0.0) {
            continue;
        }
        var sh = vec3<f32>(1.0);
        if (li.ints.z != 0u && accepts_shadows != 0u) {
            sh = shadow_of(it, i, p);
        }
        shadow_amt = max(shadow_amt, 1.0 - (sh.x + sh.y + sh.z) / 3.0);
        diff += li.color.xyz * (lambert * k) * sh;
        if (front && specular > 0.0) {
            let h = normalize(lv + to_viewer);
            let s = powz(max(dot(n, h), 0.0), ex) * k;
            let tint = vec3<f32>(1.0) + (rgb - vec3<f32>(1.0)) * metal;
            spec += li.color.xyz * s * sh * tint;
        }
    }
    let lit = rgb * (pl.mat0.x * amb + pl.mat0.y * diff) + specular * spec;
    if (accepts_shadows == 2u) {
        return vec4<f32>(0.0, 0.0, 0.0, a * shadow_amt);
    }
    return vec4<f32>(lit * a, a);
}

// compose::before: a draws before b (farther; coplanar → lower stack order first).
fn before(za: f32, oa: u32, zb: f32, ob: u32) -> bool {
    let tol = 1e-5 * max(max(abs(za), abs(zb)), 1.0);
    if (abs(za - zb) <= tol) {
        return oa < ob;
    }
    return za > zb;
}

@compute @workgroup_size(16, 16)
fn classic3d(@builtin(global_invocation_id) gid: vec3<u32>) {
    let dims = out_dims();
    let x = i32(gid.x);
    let y = i32(gid.y);
    if (x >= dims.x || y >= dims.y) {
        return;
    }
    let pix = vec2<i32>(x, y);
    var d = textureLoad(aux, pix, 0);
    let fx = f32(x);
    let fy = f32(y);
    let sx = fx + 0.5;
    let sy = fy + 0.5;
    var fz: array<f32, MAX_FRAGS>;
    var fo: array<u32, MAX_FRAGS>;
    var fi: array<u32, MAX_FRAGS>;
    var fp: array<vec4<f32>, MAX_FRAGS>;
    var nf = 0u;
    let n_planes = P.u[0].x;
    for (var i = 0u; i < n_planes; i++) {
        let pl = planes[i];
        if ((pl.ints.z & 1u) == 0u) {
            continue;
        }
        if (fx < pl.bbox.x || fx >= pl.bbox.z || fy < pl.bbox.y || fy >= pl.bbox.w) {
            continue;
        }
        // compose::fragment: average of the motion-blur sub-samples.
        let g0 = pl.ints.x;
        let ng = pl.ints.y;
        var acc = vec4<f32>(0.0);
        var depth = 0.0;
        var hit = false;
        for (var k = 0u; k < ng; k++) {
            let q = unproject(g0 + k, sx, sy);
            if (q.w == 0.0) {
                continue;
            }
            let t = texel(i, q.x, q.y);
            if (t.w <= 1e-6) {
                continue;
            }
            if (!hit) {
                depth = q.z;
                hit = true;
            }
            acc += shade_texel(i, g0 + k, q.x, q.y, t);
        }
        if (!hit) {
            continue;
        }
        if (ng > 1u) {
            acc *= 1.0 / f32(ng);
        }
        let m = pl.ints3.x;
        if (m != 0u) {
            let f = data[(m - 1u) * u32(dims.x) * u32(dims.y) + u32(y) * u32(dims.x) + u32(x)];
            acc *= f;
        }
        if (!(acc.w > 0.0 || acc.x != 0.0 || acc.y != 0.0 || acc.z != 0.0) || nf >= MAX_FRAGS) {
            continue;
        }
        // Insertion: far → near, coplanar in stack order (stable, as the CPU's insertion sort).
        let order = pl.ints2.w;
        var j = nf;
        while (j > 0u && before(depth, order, fz[j - 1u], fo[j - 1u])) {
            fz[j] = fz[j - 1u];
            fo[j] = fo[j - 1u];
            fi[j] = fi[j - 1u];
            fp[j] = fp[j - 1u];
            j--;
        }
        fz[j] = depth;
        fo[j] = order;
        fi[j] = i;
        fp[j] = acc;
        nf++;
    }
    for (var k = 0u; k < nf; k++) {
        let pl = planes[fi[k]];
        var op = pl.mat1.z;
        if ((pl.ints.z & 2u) != 0u) {
            op *= d.w;
        }
        let s = fp[k] * op;
        let mode = pl.ints.w;
        var n = 0.5;
        if (mode == 1u || mode == 2u) {
            n = hash_noise(u32(x), u32(y), pl.ints3.y);
        }
        d = blend_pixel(mode, d, s, n);
    }
    textureStore(out, pix, d);
}

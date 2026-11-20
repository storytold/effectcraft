// GPU effects, the CC light family: see src/fx_light.rs. CC Light Rays, CC Light Burst 2.5,
// CC Light Sweep and CC Light Wipe, each mirroring the CPU effect operation for operation.
// Coordinates are pixel centres relative to the effect's centre, so f32 keeps its precision.

fn flt_inside(p: vec2<i32>) -> bool {
    let d = out_dims();
    return p.x < d.x && p.y < d.y;
}

fn flt_rel(p: vec2<i32>, c: vec2<f32>) -> vec2<f32> {
    return vec2<f32>(f32(p.x) + 0.5, f32(p.y) + 0.5) - c;
}

// util::smoothstep.
fn flt_smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    if (abs(e1 - e0) < 1e-9) {
        return select(1.0, 0.0, x < e0);
    }
    let t = clamp((x - e0) / (e1 - e0), 0.0, 1.0);
    return t * t * (3.0 - 2.0 * t);
}

// util::unpremul's colour.
fn flt_unpremul(p: vec4<f32>) -> vec3<f32> {
    if (p.w > 1e-6) {
        return p.xyz / p.w;
    }
    return vec3<f32>(0.0);
}

// ---------------------------------------------------------------- CC Light Rays (generate2::light_rays)
// u[0] = (square, colour from source, transfer mode); f[0] = (cx, cy, k, gain);
// f[1] = (radius, outer radius + 1e-3); f[2] = colour.

fn flt_ray_mask(q: vec2<f32>) -> f32 {
    let d = abs(q);
    var r = length(d);
    if (P.u[0].x != 0u) {
        r = max(d.x, d.y);
    }
    return 1.0 - flt_smoothstep(P.f[1].x, P.f[1].y, r);
}

@compute @workgroup_size(16, 16)
fn flt_rays(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!flt_inside(p)) {
        return;
    }
    let c = P.f[0].xy;
    let rel = flt_rel(p, c);
    var acc = vec4<f32>(0.0);
    for (var i = 0u; i < 32u; i++) {
        let s = 1.0 - P.f[0].z * f32(i) / 32.0;
        let q = rel * s;
        let m = flt_ray_mask(q);
        if (m <= 0.0) {
            continue;
        }
        acc += sample_bilinear(src, c.x + q.x, c.y + q.y) * m;
    }
    var ray = acc / 32.0 * P.f[0].w;
    if (P.u[0].y == 0u) {
        let l = luminance(flt_unpremul(ray)) * ray.w;
        ray = vec4<f32>(P.f[2].xyz * l, ray.w);
    }
    let la = min(ray.w, 1.0);
    let px = textureLoad(src, p, 0);
    var o: vec4<f32>;
    switch P.u[0].z {
        case 1u: {
            o = vec4<f32>(max(px.xyz, ray.xyz), max(px.w, la));
        }
        case 2u: {
            let r4 = vec4<f32>(ray.xyz, la);
            o = px + r4 - px * r4;
        }
        case 3u: {
            o = vec4<f32>(ray.xyz, la);
        }
        default: {
            let a = clamp(la, 0.0, 1.0);
            o = vec4<f32>(px.xyz + ray.xyz, clamp(px.w + a * (1.0 - px.w), 0.0, 1.0));
        }
    }
    textureStore(out, p, o);
}

// ---------------------------------------------------------------- CC Light Burst 2.5 (generate2::light_burst)
// u[0] = (burst, set colour); f[0] = (cx, cy, ray length k, intensity); f[1] = colour.

@compute @workgroup_size(16, 16)
fn flt_burst(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!flt_inside(p)) {
        return;
    }
    let c = P.f[0].xy;
    let rel = flt_rel(p, c);
    let mode = P.u[0].x;
    var acc = vec4<f32>(0.0);
    var wsum = 0.0;
    var best = vec4<f32>(0.0);
    var best_l = -1.0;
    for (var i = 0u; i < 32u; i++) {
        let f = f32(i) / 32.0;
        let s = 1.0 - P.f[0].z * f;
        let q = rel * s;
        let v = sample_bilinear(src, c.x + q.x, c.y + q.y);
        if (mode == 0u) {
            let l = luminance(v.xyz) + v.w * 1e-3;
            if (l > best_l) {
                best_l = l;
                best = v;
            }
        } else if (mode == 1u) {
            let w = 1.0 - f;
            acc += v * w;
            wsum += w;
        } else {
            acc += v;
            wsum += 1.0;
        }
    }
    var o = best;
    if (mode != 0u) {
        o = acc / max(wsum, 1e-6);
    }
    if (P.u[0].y != 0u) {
        let a = max(o.w, 0.0);
        let l = luminance(flt_unpremul(o)) * a;
        o = vec4<f32>(P.f[1].xyz * l, a);
    }
    let k = P.f[0].w;
    textureStore(out, p, vec4<f32>(o.xyz * k, clamp(o.w * k, 0.0, 1.0)));
}

// ---------------------------------------------------------------- CC Light Sweep (generate2::light_sweep)
// aux = the (softened) alpha plane in x. u[0] = (shape, reception mode); f[0] = (cx, cy, nx, ny);
// f[1] = (half width, sweep, edge intensity); f[2] = light colour.

@compute @workgroup_size(16, 16)
fn flt_sweep(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!flt_inside(p)) {
        return;
    }
    let px = textureLoad(src, p, 0);
    let rel = flt_rel(p, P.f[0].xy);
    let d = abs(rel.x * P.f[0].z + rel.y * P.f[0].w);
    let half = P.f[1].x;
    let mode = P.u[0].y;
    var band: f32;
    switch P.u[0].x {
        case 0u: {
            band = max(1.0 - d / half, 0.0);
        }
        case 2u: {
            band = clamp(half - d + 0.5, 0.0, 1.0);
        }
        default: {
            band = flt_smoothstep(half, 0.0, d);
        }
    }
    if (band <= 0.0) {
        textureStore(out, p, select(px, vec4<f32>(0.0), mode == 2u));
        return;
    }
    let gx = tex_get_clamped(aux, p.x + 1, p.y).x - tex_get_clamped(aux, p.x - 1, p.y).x;
    let gy = tex_get_clamped(aux, p.x, p.y + 1).x - tex_get_clamped(aux, p.x, p.y - 1).x;
    // Directional edge light, as the CPU effect: only the side the sweep reaches first glows
    // (#3295).
    let edge = clamp(2.0 * (gx * P.f[0].z + gy * P.f[0].w), 0.0, 1.0);
    let light = band * P.f[1].y + band * edge * P.f[1].z;
    let lc = P.f[2].xyz;
    let a = px.w;
    var o: vec4<f32>;
    if (mode == 1u) {
        let la = min(band * P.f[1].y, 1.0);
        o = vec4<f32>(px.xyz + lc * light, min(a + la * (1.0 - a), 1.0));
    } else if (mode == 2u) {
        let la = min(light * a, 1.0);
        o = vec4<f32>(lc * la, la);
    } else {
        o = vec4<f32>(px.xyz + lc * light * a, a);
    }
    textureStore(out, p, o);
}

// ---------------------------------------------------------------- CC Light Wipe (transition::light_wipe)
// u[0] = (shape, colour from source, reverse); f[0] = (cx, cy, sin, cos);
// f[1] = (edge, band, intensity × ramp); f[2] = colour.

@compute @workgroup_size(16, 16)
fn flt_wipe(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!flt_inside(p)) {
        return;
    }
    var px = textureLoad(src, p, 0);
    let d = flt_rel(p, P.f[0].xy);
    let s = P.f[0].z;
    let co = P.f[0].w;
    let q = vec2<f32>(d.x * co + d.y * s, -d.x * s + d.y * co);
    var m: f32;
    switch P.u[0].x {
        case 0u: {
            m = abs(q.x);
        }
        case 2u: {
            m = max(abs(q.x), abs(q.y));
        }
        default: {
            m = length(q);
        }
    }
    let edge = P.f[1].x;
    let visible = select(m > edge, m < edge, P.u[0].z != 0u);
    if (!visible) {
        textureStore(out, p, vec4<f32>(0.0));
        return;
    }
    let z = (m - edge) / P.f[1].y;
    let g = exp(-(z * z)) * P.f[1].z;
    if (g > 0.0) {
        var lc = P.f[2].xyz;
        if (P.u[0].y != 0u) {
            lc = flt_unpremul(px);
        }
        px = vec4<f32>(px.xyz + lc * g * px.w, px.w);
    }
    textureStore(out, p, px);
}

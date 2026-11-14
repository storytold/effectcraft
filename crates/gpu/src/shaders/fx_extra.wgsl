// GPU effects (shapes and bevels): see src/fx_extra.rs. Circle, Ellipse, Iris Wipe, Bevel Alpha
// and Bevel Edges, each mirroring the CPU effect operation for operation. Coordinates are pixel
// centres relative to the shape's centre (or the layer's corner), so f32 keeps its precision.

fn fxe_inside(p: vec2<i32>) -> bool {
    let d = out_dims();
    return p.x < d.x && p.y < d.y;
}

fn fxe_rel(p: vec2<i32>, c: vec2<f32>) -> vec2<f32> {
    return vec2<f32>(f32(p.x) + 0.5, f32(p.y) + 0.5) - c;
}

// ---------------------------------------------------------------- Circle (generate::circle)
// u[0] = (gen mode, has inner, invert); f[0] = (cx, cy, outer radius, inner radius);
// f[1] = (outer feather, inner feather, opacity); f[2] = colour.

@compute @workgroup_size(16, 16)
fn fxe_circle(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!fxe_inside(p)) {
        return;
    }
    let d = length(fxe_rel(p, P.f[0].xy));
    var cov = clamp((P.f[0].z - d) / P.f[1].x + 0.5, 0.0, 1.0);
    if (P.u[0].y != 0u) {
        cov *= clamp((d - P.f[0].w) / P.f[1].y + 0.5, 0.0, 1.0);
    }
    if (P.u[0].z != 0u) {
        cov = 1.0 - cov;
    }
    let o = textureLoad(src, p, 0);
    let a = gen_alpha(cov * P.f[1].z, o.w, P.u[0].x);
    textureStore(out, p, gen_blend(o, vec4<f32>(P.f[2].xyz * a, a), P.u[0].x));
}

// ---------------------------------------------------------------- Ellipse (generate2::ellipse)
// u[0].y = composite on original; f[0] = (cx, cy, rx, ry); f[1] = (half thickness, softness);
// f[2] = inside colour; f[3] = outside colour.

fn fxe_smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    if (abs(e1 - e0) < 1e-9) {
        return select(1.0, 0.0, x < e0);
    }
    let t = clamp((x - e0) / (e1 - e0), 0.0, 1.0);
    return t * t * (3.0 - 2.0 * t);
}

@compute @workgroup_size(16, 16)
fn fxe_ellipse(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!fxe_inside(p)) {
        return;
    }
    let d = fxe_rel(p, P.f[0].xy);
    let r = P.f[0].zw;
    let q = length(d / r);
    var dist = min(r.x, r.y);
    if (q >= 1e-9) {
        let g = d / (r * r) / q;
        dist = abs((q - 1.0) / max(length(g), 1e-9));
    }
    let half = P.f[1].x;
    let soft = P.f[1].y;
    let t = dist / half;
    var a = clamp(half - dist + 0.5, 0.0, 1.0);
    if (soft > 1e-3) {
        a = 1.0 - fxe_smoothstep(1.0 - soft, 1.0, t);
    }
    let k = fxe_smoothstep(0.0, 1.0, t);
    let cc = P.f[2].xyz + (P.f[3].xyz - P.f[2].xyz) * k;
    let g4 = vec4<f32>(cc * a, a);
    if (P.u[0].y != 0u) {
        let o = textureLoad(src, p, 0);
        textureStore(out, p, g4 + o * (1.0 - g4.w));
    } else {
        textureStore(out, p, g4);
    }
}

// ---------------------------------------------------------------- Iris Wipe (transition::iris_wipe)
// data = polygon vertices relative to the centre; u[0] = (_, vertex count);
// f[0] = (cx, cy, rotation, angle step); f[1] = (feather, radius at the centre, outer radius).

fn fxe_cross(a: vec2<f32>, b: vec2<f32>) -> f32 {
    return a.x * b.y - a.y * b.x;
}

@compute @workgroup_size(16, 16)
fn fxe_iris(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!fxe_inside(p)) {
        return;
    }
    let d = fxe_rel(p, P.f[0].xy);
    let r = length(d);
    let m = P.u[0].y;
    let rot = P.f[0].z;
    let step = P.f[0].w;
    var rb = P.f[1].y;
    if (r >= 1e-9) {
        let tau = 6.2831855;
        let a0 = atan2(d.x, -d.y) - rot;
        let a = a0 - floor(a0 / tau) * tau;
        let i = min(u32(floor(a / step)), m - 1u);
        let vi = vec2<f32>(data[2u * i], data[2u * i + 1u]);
        let j = (i + 1u) % m;
        let vj = vec2<f32>(data[2u * j], data[2u * j + 1u]);
        let e = vj - vi;
        let den = fxe_cross(d / r, e);
        rb = select(fxe_cross(vi, e) / den, P.f[1].z, abs(den) < 1e-12);
    }
    let feather = P.f[1].x;
    var k = select(0.0, 1.0, r >= rb);
    if (feather > 0.0) {
        k = clamp((r - rb) / feather + 0.5, 0.0, 1.0);
    }
    textureStore(out, p, textureLoad(src, p, 0) * k);
}

// ---------------------------------------------------------------- Bevels (perspective.rs)

// perspective::apply_shade.
fn fxe_shade(px: vec4<f32>, s: f32, light: vec3<f32>) -> vec4<f32> {
    if (s > 0.0) {
        return vec4<f32>(px.xyz + light * s * px.w, px.w);
    }
    if (s < 0.0) {
        return vec4<f32>(px.xyz * max(1.0 + s, 0.0), px.w);
    }
    return px;
}

// Plane::alpha (in x).
@compute @workgroup_size(16, 16)
fn fxe_alpha(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!fxe_inside(p)) {
        return;
    }
    textureStore(out, p, vec4<f32>(textureLoad(src, p, 0).w, 0.0, 0.0, 0.0));
}

// Bevel Alpha: src = the layer, aux = its blurred alpha (x). f[0] = (light direction, normal
// scale k); f[1] = (light colour, intensity).
@compute @workgroup_size(16, 16)
fn fxe_bevel_alpha(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!fxe_inside(p)) {
        return;
    }
    let px = textureLoad(src, p, 0);
    if (px.w <= 0.0) {
        textureStore(out, p, px);
        return;
    }
    let dx = (tex_get_clamped(aux, p.x + 1, p.y).x - tex_get_clamped(aux, p.x - 1, p.y).x) * 0.5;
    let dy = (tex_get_clamped(aux, p.x, p.y + 1).x - tex_get_clamped(aux, p.x, p.y - 1).x) * 0.5;
    let k = P.f[0].w;
    let n = normalize(vec3<f32>(-dx * k, -dy * k, 1.0));
    let l = P.f[0].xyz;
    let s = (dot(n, l) - l.z) * P.f[1].w * 2.0;
    textureStore(out, p, fxe_shade(px, s, P.f[1].xyz));
}

// Bevel Edges: f[0] = (light direction, thickness); f[1] = (light colour, intensity);
// f[2] = layer rectangle (x0, y0, w, h).
@compute @workgroup_size(16, 16)
fn fxe_bevel_edges(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!fxe_inside(p)) {
        return;
    }
    let px = textureLoad(src, p, 0);
    let rc = P.f[2];
    let q = vec2<f32>(f32(p.x) + 0.5, f32(p.y) + 0.5);
    let d = vec4<f32>(q.x - rc.x, rc.x + rc.z - q.x, q.y - rc.y, rc.y + rc.w - q.y);
    if (d.x < 0.0 || d.y < 0.0 || d.z < 0.0 || d.w < 0.0) {
        textureStore(out, p, px);
        return;
    }
    var mi = 0;
    var mv = d.x;
    for (var i = 1; i < 4; i++) {
        if (d[i] < mv) {
            mv = d[i];
            mi = i;
        }
    }
    if (mv >= P.f[0].w) {
        textureStore(out, p, px);
        return;
    }
    var n2 = vec2<f32>(-1.0, 0.0);
    if (mi == 1) {
        n2 = vec2<f32>(1.0, 0.0);
    } else if (mi == 2) {
        n2 = vec2<f32>(0.0, -1.0);
    } else if (mi == 3) {
        n2 = vec2<f32>(0.0, 1.0);
    }
    let tilt = 0.70710677;
    let n = vec3<f32>(n2 * tilt, tilt);
    let l = P.f[0].xyz;
    let s = (dot(n, l) - l.z) * P.f[1].w * 2.0;
    textureStore(out, p, fxe_shade(px, s, P.f[1].xyz));
}

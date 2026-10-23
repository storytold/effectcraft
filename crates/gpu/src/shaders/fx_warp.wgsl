// GPU effects (warp family): see src/fx_warp.rs. Warp, CC Bend It, CC Page Turn, Smear, Reshape,
// Color Emboss and Cartoon, each mirroring the CPU effect in effectcraft-effects operation for
// operation. Coordinates are pixel centres (x + 0.5, y + 0.5) in buffer pixels, as in
// util::remap; scalar set-up (radii, angles, outlines, correspondence) is computed on the CPU in
// f64 and passed in.

const FXW_PI: f32 = 3.1415927;

fn fxw_centre(p: vec2<i32>) -> vec2<f32> {
    return vec2<f32>(f32(p.x) + 0.5, f32(p.y) + 0.5);
}

fn fxw_inside(p: vec2<i32>) -> bool {
    let d = out_dims();
    return p.x < d.x && p.y < d.y;
}

// util::unpremul / premul.
fn fxw_unpremul(p: vec4<f32>) -> vec3<f32> {
    if (p.w > 1e-6) {
        return p.xyz / p.w;
    }
    return vec3<f32>(0.0);
}

fn fxw_smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    if (abs(e1 - e0) < 1e-9) {
        return select(1.0, 0.0, x < e0);
    }
    let t = clamp((x - e0) / (e1 - e0), 0.0, 1.0);
    return t * t * (3.0 - 2.0 * t);
}

// ---------------------------------------------------------------- Warp (distort3::warp)
// u[0] = (_, style, vertical); f[0] = (cx, cy, hx, hy); f[1] = (bend, horizontal, vertical
// distortion). Positions are relative to the centre.

fn fxw_style(style: u32, k: f32, u: f32, v: f32) -> vec2<f32> {
    let bell = 1.0 - u * u;
    switch style {
        case 0u: {
            return vec2<f32>(u * (1.0 - 0.3 * k * v), v - k * bell);
        }
        case 1u: {
            return vec2<f32>(u, v - k * bell * (v + 1.0) * 0.5);
        }
        case 2u: {
            return vec2<f32>(u, v - k * bell * (1.0 - v) * 0.5);
        }
        case 3u: {
            return vec2<f32>(u, v - k * bell);
        }
        case 4u: {
            return vec2<f32>(u, v * (1.0 + k * bell));
        }
        case 5u: {
            return vec2<f32>(u, v + k * bell * bell * (v + 1.0) * 0.5);
        }
        case 6u: {
            return vec2<f32>(u, v - k * bell * bell * (1.0 - v) * 0.5);
        }
        case 7u: {
            return vec2<f32>(u, v - 0.5 * k * sin(FXW_PI * u));
        }
        case 8u: {
            return vec2<f32>(u, v - 0.5 * k * sin(FXW_PI * u + 0.5 * FXW_PI * v));
        }
        case 9u: {
            return vec2<f32>(u, v * (1.0 + 0.5 * k * sin(FXW_PI * u)));
        }
        case 10u: {
            return vec2<f32>(u, v - 0.5 * k * sin(0.5 * FXW_PI * u));
        }
        case 11u: {
            let r2 = u * u + v * v;
            if (r2 >= 1.0) {
                return vec2<f32>(u, v);
            }
            let f = 1.0 + k * (1.0 - r2);
            return vec2<f32>(u * f, v * f);
        }
        case 12u: {
            return vec2<f32>(u * (1.0 + 0.5 * k * (1.0 - v * v)), v * (1.0 + 0.5 * k * bell));
        }
        case 13u: {
            return vec2<f32>(u * (1.0 - 0.5 * k * (1.0 - v * v)), v * (1.0 + 0.5 * k * bell));
        }
        default: {}
    }
    // Twist (the default style), after the switch: FXC (DirectX 12) rejects a function whose
    // every return sits inside a switch ("not all control paths return a value", #613).
    let r = sqrt(u * u + v * v);
    if (r >= 1.0) {
        return vec2<f32>(u, v);
    }
    let a = k * FXW_PI * (1.0 - r);
    let s = sin(a);
    let c = cos(a);
    return vec2<f32>(u * c - v * s, u * s + v * c);
}

fn fxw_warp_fwd(q: vec2<f32>) -> vec2<f32> {
    let h = P.f[0].zw;
    let k = P.f[1].x;
    let hd = P.f[1].y;
    let vd = P.f[1].z;
    let vertical = P.u[0].z != 0u;
    let n = q / h;
    var uv = n;
    if (vertical) {
        uv = n.yx;
    }
    let w = fxw_style(P.u[0].y, k, uv.x, uv.y);
    var o = w;
    if (vertical) {
        o = w.yx;
    }
    o.y *= 1.0 + 0.5 * hd * o.x;
    o.x *= 1.0 + 0.5 * vd * o.y;
    return o * h;
}

@compute @workgroup_size(16, 16)
fn fxw_warp(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!fxw_inside(p)) {
        return;
    }
    let c = P.f[0].xy;
    let t = fxw_centre(p) - c;
    var s = t;
    for (var i = 0; i < 12; i++) {
        let f = fxw_warp_fwd(s);
        let r = f - t;
        // The CPU stops at 1e-7 px in f64; f32 cannot get there, and iterating on past
        // convergence can hop across a fold (Twist, Fisheye edges), so stop at 1e-4 px.
        if (abs(r.x) + abs(r.y) < 1e-4) {
            break;
        }
        let e = 0.01;
        let a = fxw_warp_fwd(s + vec2<f32>(e, 0.0));
        let b = fxw_warp_fwd(s + vec2<f32>(0.0, e));
        let j00 = (a.x - f.x) / e;
        let j10 = (a.y - f.y) / e;
        let j01 = (b.x - f.x) / e;
        let j11 = (b.y - f.y) / e;
        let det = j00 * j11 - j01 * j10;
        if (abs(det) < 1e-9) {
            textureStore(out, p, vec4<f32>(0.0));
            return;
        }
        s.x -= (j11 * r.x - j01 * r.y) / det;
        s.y -= (-j10 * r.x + j00 * r.y) / det;
    }
    let f = fxw_warp_fwd(s);
    if (abs(f.x - t.x) + abs(f.y - t.y) > 0.5) {
        textureStore(out, p, vec4<f32>(0.0));
        return;
    }
    textureStore(out, p, sample_bilinear(src, s.x + c.x, s.y + c.y));
}

// ---------------------------------------------------------------- CC Bend It (distort2::bend_it)
// u[0].y = static prestart; f[0] = (start x, y, axis a x, y); f[1] = (normal n x, y, rho, beta);
// f[2] = (end of arc pe x, y, length, _); f[3] = (t x, y, nn x, y).

@compute @workgroup_size(16, 16)
fn fxw_bendit(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!fxw_inside(p)) {
        return;
    }
    let s = P.f[0].xy;
    let a = P.f[0].zw;
    let n = P.f[1].xy;
    let rho = P.f[1].z;
    let beta = P.f[1].w;
    let pe = P.f[2].xy;
    let l = P.f[2].z;
    let t = P.f[3].xy;
    let nn = P.f[3].zw;
    let v = fxw_centre(p) - s;
    let lx = dot(v, a);
    let ly = dot(v, n);
    // Arc: rho - |(lx, ly - rho)| without cancellation for large radii.
    let dy = ly - rho;
    let th = atan2_p(lx, -dy);
    let dd = lx * lx + ly * ly - 2.0 * ly * rho;
    let w_arc = -dd / (rho + sqrt(max(rho * rho + dd, 0.0)));
    var cand: array<vec3<f32>, 3>;
    cand[0] = vec3<f32>(rho * th, w_arc, select(0.0, 1.0, th >= 0.0 && th <= beta));
    let e = vec2<f32>(lx, ly) - pe;
    let u2 = dot(e, t);
    cand[1] = vec3<f32>(l + u2, dot(e, nn), select(0.0, 1.0, u2 >= 0.0));
    cand[2] = vec3<f32>(lx, ly, select(0.0, 1.0, lx < 0.0 && P.u[0].y != 0u));
    for (var i = 0; i < 3; i++) {
        let c = cand[i];
        if (c.z == 0.0) {
            continue;
        }
        let q = s + a * c.x + n * c.y;
        let px = sample_bilinear(src, q.x, q.y);
        if (px.w > 1e-4) {
            textureStore(out, p, px);
            return;
        }
    }
    textureStore(out, p, vec4<f32>(0.0));
}

// ---------------------------------------------------------------- CC Page Turn (distort2::page_turn)
// u[0] = (_, show front, show back); f[0] = (fold x, y, n x, y); f[1] = (radius, light dot,
// back opacity, _); f[2] = paper colour.

@compute @workgroup_size(16, 16)
fn fxw_pageturn(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!fxw_inside(p)) {
        return;
    }
    let q = fxw_centre(p);
    let f = P.f[0].xy;
    let n = P.f[0].zw;
    let r = P.f[1].x;
    let ldot = P.f[1].y;
    let bo = P.f[1].z;
    let paper = P.f[2].xyz;
    let show_front = P.u[0].y != 0u;
    let show_back = P.u[0].z != 0u;
    let d = dot(q - f, n);
    if (d > r) {
        textureStore(out, p, vec4<f32>(0.0));
        return;
    }
    // (u, back side, shade, valid) from the top layer down.
    var cand: array<vec4<f32>, 4>;
    if (d <= 0.0) {
        cand[0] = vec4<f32>(FXW_PI * r - d, 1.0, 0.9, 1.0);
        cand[3] = vec4<f32>(d, 0.0, 1.0, 1.0);
    } else {
        let a = asin_p(clamp(d / r, -1.0, 1.0));
        cand[1] = vec4<f32>(FXW_PI * r - r * a, 1.0, 0.6 + 0.4 * cos(a), 1.0);
        let psi = a / (0.5 * FXW_PI);
        cand[2] = vec4<f32>(r * a, 0.0, 1.0 - 0.4 * psi * (0.5 + 0.5 * ldot), 1.0);
    }
    for (var i = 0; i < 4; i++) {
        let c = cand[i];
        if (c.w == 0.0) {
            continue;
        }
        let back = c.y != 0.0;
        if ((back && !show_back) || (!back && !show_front)) {
            continue;
        }
        let k = c.x - d;
        let v = sample_bilinear(src, q.x + k * n.x, q.y + k * n.y);
        if (v.w <= 1e-4) {
            continue;
        }
        var col = fxw_unpremul(v);
        if (back) {
            col = paper * (1.0 - bo) + col * bo;
        }
        textureStore(out, p, vec4<f32>(col * c.z * v.w, v.w));
        return;
    }
    textureStore(out, p, vec4<f32>(0.0));
}

// ---------------------------------------------------------------- polygons (util)
// A polygon of `n` points at data[base..base + 2n].

fn fxw_pt(base: u32, i: u32) -> vec2<f32> {
    return vec2<f32>(data[base + 2u * i], data[base + 2u * i + 1u]);
}

fn fxw_in_poly(base: u32, n: u32, x: f32, y: f32) -> bool {
    if (n < 3u) {
        return false;
    }
    var inside = false;
    var j = n - 1u;
    for (var i = 0u; i < n; i++) {
        let a = fxw_pt(base, i);
        let b = fxw_pt(base, j);
        if ((a.y > y) != (b.y > y) && x < (b.x - a.x) * (y - a.y) / (b.y - a.y) + a.x) {
            inside = !inside;
        }
        j = i;
    }
    return inside;
}

// Distance to a closed polyline.
fn fxw_dist_poly(base: u32, n: u32, q: vec2<f32>) -> f32 {
    if (n == 0u) {
        return 3.0e38;
    }
    if (n == 1u) {
        return length(fxw_pt(base, 0u) - q);
    }
    var best = 3.0e38;
    for (var i = 0u; i < n; i++) {
        let a = fxw_pt(base, i);
        let b = fxw_pt(base, (i + 1u) % n);
        let d = b - a;
        let l2 = dot(d, d);
        var t = 0.0;
        if (l2 > 0.0) {
            t = clamp(dot(q - a, d) / l2, 0.0, 1.0);
        }
        let e = a + d * t - q;
        best = min(best, dot(e, e));
    }
    return sqrt(best);
}

// ---------------------------------------------------------------- Smear (distort3::smear)
// data = boundary outline, moved source outline; u[0] = (_, boundary points, moved points);
// f[0] = (centroid x, y, cos, sin); f[1] = (scale, offset x, y, percent); f[2].x = 1 / elasticity.

@compute @workgroup_size(16, 16)
fn fxw_smear(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!fxw_inside(p)) {
        return;
    }
    let q = fxw_centre(p);
    let nb = P.u[0].y;
    let nm = P.u[0].z;
    if (!fxw_in_poly(0u, nb, q.x, q.y)) {
        textureStore(out, p, textureLoad(src, p, 0));
        return;
    }
    var w = 1.0;
    if (!fxw_in_poly(2u * nb, nm, q.x, q.y)) {
        let dm = fxw_dist_poly(2u * nb, nm, q);
        let db = fxw_dist_poly(0u, nb, q);
        w = pow(db / max(db + dm, 1e-9), P.f[2].x);
    }
    let c = P.f[0].xy;
    let cs = P.f[0].z;
    let sn = P.f[0].w;
    let scl = P.f[1].x;
    let o = P.f[1].yz;
    let d = q - o - c;
    let iv = vec2<f32>(c.x + (d.x * cs + d.y * sn) / scl, c.y + (-d.x * sn + d.y * cs) / scl);
    let k = clamp(w * P.f[1].w, 0.0, 1.0);
    let s = q + (iv - q) * k;
    textureStore(out, p, sample_bilinear(src, s.x, s.y));
}

// ---------------------------------------------------------------- Reshape (distort3::reshape)
// data = 48 destination points, their 48 displacements, then the boundary outline;
// u[0] = (_, boundary points (0 = none), smooth); f[0] = (elasticity / 2, percent).

@compute @workgroup_size(16, 16)
fn fxw_reshape(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!fxw_inside(p)) {
        return;
    }
    let q = fxw_centre(p);
    let nb = P.u[0].y;
    let bbase = 4u * 48u;
    if (nb > 0u && !fxw_in_poly(bbase, nb, q.x, q.y)) {
        textureStore(out, p, textureLoad(src, p, 0));
        return;
    }
    var wv = vec2<f32>(0.0);
    var ws = 0.0;
    var dmin = 3.0e38;
    for (var i = 0u; i < 48u; i++) {
        let e = q - fxw_pt(0u, i);
        let d2 = dot(e, e);
        dmin = min(dmin, d2);
        let w = 1.0 / pow(d2 + 1e-6, P.f[0].x);
        wv += w * fxw_pt(96u, i);
        ws += w;
    }
    var fade = 1.0;
    if (nb > 0u) {
        let db = fxw_dist_poly(bbase, nb, q);
        fade = db / max(db + sqrt(dmin), 1e-9);
        if (P.u[0].z != 0u) {
            fade = fade * fade * (3.0 - 2.0 * fade);
        }
    }
    let k = fade * P.f[0].y / ws;
    let s = q + wv * k;
    textureStore(out, p, sample_bilinear(src, s.x, s.y));
}

// ---------------------------------------------------------------- Color Emboss (stylize2::color_emboss)
// f[0] = (dx, dy, contrast, blend).

@compute @workgroup_size(16, 16)
fn fxw_coloremboss(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!fxw_inside(p)) {
        return;
    }
    let px = textureLoad(src, p, 0);
    if (px.w <= 0.0) {
        textureStore(out, p, px);
        return;
    }
    let q = fxw_centre(p);
    let d = P.f[0].xy;
    let c = fxw_unpremul(px);
    let pa = fxw_unpremul(sample_bilinear_clamped(src, q.x + d.x, q.y + d.y));
    let pb = fxw_unpremul(sample_bilinear_clamped(src, q.x - d.x, q.y - d.y));
    let e = max(c + (pa - pb) * P.f[0].z, vec3<f32>(0.0));
    let o = e + (c - e) * P.f[0].w;
    textureStore(out, p, vec4<f32>(o * px.w, px.w));
}

// ---------------------------------------------------------------- Cartoon (stylize2::cartoon)

// Plane::luma in every channel (the guided filter's guide).
@compute @workgroup_size(16, 16)
fn fxw_luma(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!fxw_inside(p)) {
        return;
    }
    let l = luminance(fxw_unpremul(textureLoad(src, p, 0)));
    textureStore(out, p, vec4<f32>(l));
}

// Colour from aux (the filtered channels), alpha from src.
@compute @workgroup_size(16, 16)
fn fxw_keep_alpha(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!fxw_inside(p)) {
        return;
    }
    textureStore(out, p, vec4<f32>(textureLoad(aux, p, 0).xyz, textureLoad(src, p, 0).w));
}

// Edge Enhancement > 0: one shock-filter step. src = the smoothed picture, aux = its luma (x);
// f[0].x = step length.
@compute @workgroup_size(16, 16)
fn fxw_shock(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!fxw_inside(p)) {
        return;
    }
    let g0 = tex_get_clamped(aux, p.x, p.y).x;
    let gr = tex_get_clamped(aux, p.x + 1, p.y).x;
    let gl = tex_get_clamped(aux, p.x - 1, p.y).x;
    let gd = tex_get_clamped(aux, p.x, p.y + 1).x;
    let gu = tex_get_clamped(aux, p.x, p.y - 1).x;
    let gx = (gr - gl) * 0.5;
    let gy = (gd - gu) * 0.5;
    let lap = gr + gl + gd + gu - 4.0 * g0;
    let m = sqrt(gx * gx + gy * gy);
    if (m < 1e-4 || abs(lap) < 1e-5) {
        textureStore(out, p, textureLoad(src, p, 0));
        return;
    }
    let k = -sign(lap) * P.f[0].x / m;
    let q = fxw_centre(p);
    textureStore(out, p, sample_bilinear_clamped(src, q.x + gx * k, q.y + gy * k));
}

// Sobel magnitude of the luma (src.x) through the edge threshold: f[0] = (t0, t1).
@compute @workgroup_size(16, 16)
fn fxw_edges(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!fxw_inside(p)) {
        return;
    }
    let a = tex_get_clamped(src, p.x - 1, p.y - 1).x;
    let b = tex_get_clamped(src, p.x, p.y - 1).x;
    let c = tex_get_clamped(src, p.x + 1, p.y - 1).x;
    let d = tex_get_clamped(src, p.x - 1, p.y).x;
    let f = tex_get_clamped(src, p.x + 1, p.y).x;
    let g = tex_get_clamped(src, p.x - 1, p.y + 1).x;
    let h = tex_get_clamped(src, p.x, p.y + 1).x;
    let i = tex_get_clamped(src, p.x + 1, p.y + 1).x;
    let gx = c + 2.0 * f + i - a - 2.0 * d - g;
    let gy = g + 2.0 * h + i - a - 2.0 * b - c;
    let m = sqrt(gx * gx + gy * gy);
    textureStore(out, p, vec4<f32>(fxw_smoothstep(P.f[0].x, P.f[0].y, m), 0.0, 0.0, 0.0));
}

// The cartoon fill and edges. src = the layer, aux = the smoothed picture, data = the edge
// plane's rows (x); u[0] = (_, render, edge row stride, has edges); f[0] = (steps, smoothness,
// edge opacity × width factor, edge black level); f[1].x = edge contrast.
@compute @workgroup_size(16, 16)
fn fxw_cartoon(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!fxw_inside(p)) {
        return;
    }
    let px = textureLoad(src, p, 0);
    let a = px.w;
    if (a <= 0.0) {
        textureStore(out, p, px);
        return;
    }
    let render = P.u[0].y;
    let c = fxw_unpremul(textureLoad(aux, p, 0));
    let steps = P.f[0].x;
    let smoothness = P.f[0].y;
    let l = luminance(c);
    let t = max(l, 0.0) * steps;
    let fr = t - floor(t);
    var f2 = 0.0;
    if (smoothness <= 0.0) {
        f2 = select(0.0, 1.0, fr >= 0.5);
    } else {
        f2 = fxw_smoothstep(0.5 - smoothness * 0.5, 0.5 + smoothness * 0.5, fr);
    }
    let lq = (floor(t) + f2) / steps;
    var fill = vec3<f32>(lq);
    if (l > 1e-4) {
        fill = c * lq / l;
    }
    var e = 0.0;
    if (P.u[0].w != 0u) {
        e = data[(u32(p.y) * P.u[0].z + u32(p.x)) * 4u] * P.f[0].z;
    }
    let g = clamp((abs((1.0 - e) - P.f[0].w) - 0.5) * P.f[1].x + 0.5, 0.0, 1.0);
    var o = fill;
    if (render == 1u) {
        o = vec3<f32>(g);
    } else if (render > 1u) {
        o = fill * g;
    }
    textureStore(out, p, vec4<f32>(o * a, a));
}

// Warp along an inverse map solved on the CPU (aux: source x, y, valid), util::remap without
// edge clamping.
@compute @workgroup_size(16, 16)
fn fxw_remap(@builtin(global_invocation_id) gid: vec3<u32>) {
    let dims = out_dims();
    let p = vec2<i32>(gid.xy);
    if (p.x >= dims.x || p.y >= dims.y) {
        return;
    }
    let m = textureLoad(aux, p, 0);
    var o = vec4<f32>(0.0);
    if (m.z > 0.0) {
        o = sample_bilinear(src, m.x, m.y);
    }
    textureStore(out, p, o);
}

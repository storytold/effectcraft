// GPU effects, generators of part C: see src/fx_gen2.rs. Every entry point is prefixed `fg2_`.
// Geometry computed on the CPU (bolts, strokes, glyphs, audio marks, waves) arrives as item
// tables in `data` — tile offsets, item indices, items (as fx_sim's tiled raster) — and is
// rasterised here with the CPU's coverage functions; closed-form generators run per pixel.

fn fg2_inside(p: vec2<i32>) -> bool {
    let d = out_dims();
    return p.x < d.x && p.y < d.y;
}

// util::unpremul (colour 0 below alpha 1e-6).
fn fg2_unpremul(p: vec4<f32>) -> vec3<f32> {
    if (p.w > 1e-6) {
        return p.xyz / p.w;
    }
    return vec3<f32>(0.0);
}

// util::smoothstep.
fn fg2_smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    if (abs(e1 - e0) < 1e-9) {
        return select(1.0, 0.0, x < e0);
    }
    let t = clamp((x - e0) / (e1 - e0), 0.0, 1.0);
    return t * t * (3.0 - 2.0 * t);
}

// Premultiplied "over": `s` on top of `d`.
fn fg2_over(d: vec4<f32>, s: vec4<f32>) -> vec4<f32> {
    return s + d * (1.0 - s.w);
}

// The tile's item range: (first index slot, end slot).
fn fg2_tile(p: vec2<i32>, tiles_x: u32) -> vec2<u32> {
    let tile = u32(p.y / 16) * tiles_x + u32(p.x / 16);
    return vec2<u32>(u32(data[tile]), u32(data[tile + 1u]));
}

// Whether pixel `p` lies in the integer bounds stored at the end of item `at` (stride `stride`).
fn fg2_in_bounds(p: vec2<i32>, at: u32, stride: u32) -> bool {
    let bb = at + stride - 4u;
    return !(f32(p.x) < data[bb] || f32(p.y) < data[bb + 1u] || f32(p.x) > data[bb + 2u] || f32(p.y) > data[bb + 3u]);
}

// ---------------------------------------------------------------- segments (generate3::raster_segs)
// Items: (ax, ay, bx, by, radius, value, bounds). Up to four coverages per pixel (x, y, z, w),
// variant k with radius max(r · f[k].x + f[k].y, f[k].z) and hardness f[k].w, max-combined;
// bit k of u[1].x selects Advanced Lightning's linear core (clamp(r − d + 0.5, 0, 1)) instead.
// u[0] = (items at, stride, tiles x, variants).

@compute @workgroup_size(16, 16)
fn fg2_segs(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!fg2_inside(p)) {
        return;
    }
    let stride = P.u[0].y;
    let nv = P.u[0].w;
    let rg = fg2_tile(p, P.u[0].z);
    let px = f32(p.x) + 0.5;
    let py = f32(p.y) + 0.5;
    var o = vec4<f32>(0.0);
    for (var k = rg.x; k < rg.y; k++) {
        let at = P.u[0].x + u32(data[k]) * stride;
        if (!fg2_in_bounds(p, at, stride)) {
            continue;
        }
        let ax = data[at];
        let ay = data[at + 1u];
        let dx = data[at + 2u] - ax;
        let dy = data[at + 3u] - ay;
        let r = data[at + 4u];
        let v = data[at + 5u];
        let l2 = dx * dx + dy * dy;
        var t = 0.0;
        if (l2 > 1e-12) {
            t = clamp(((px - ax) * dx + (py - ay) * dy) / l2, 0.0, 1.0);
        }
        let ex = px - ax - dx * t;
        let ey = py - ay - dy * t;
        let d = sqrt(ex * ex + ey * ey);
        for (var j = 0u; j < nv; j++) {
            let vf = P.f[j];
            let rk = max(r * vf.x + vf.y, vf.z);
            var c = 0.0;
            if (((P.u[1].x >> j) & 1u) != 0u) {
                c = clamp(rk - d + 0.5, 0.0, 1.0) * v;
            } else {
                let inner = rk * clamp(vf.w, 0.0, 1.0) - 0.5;
                let outer = rk + 0.5;
                if (d >= outer) {
                    continue;
                }
                if (d <= inner) {
                    c = v;
                } else {
                    c = (1.0 - fg2_smoothstep(inner, outer, d)) * v;
                }
            }
            if (c > o[j]) {
                o = vset4f(o, u32(j), c);
            }
        }
    }
    textureStore(out, p, o);
}

// ---------------------------------------------------------------- convex polygons (generate3::raster_convex)
// Items: (vertex count, 16 vertex pairs, bounds); coverage = union, 4 × 4 samples per pixel.
// u[0] = (items at, stride, tiles x).

fn fg2_in_poly(at: u32, x: f32, y: f32) -> bool {
    let n = u32(data[at]);
    var sgn = 0.0;
    for (var i = 0u; i < n; i++) {
        let j = (i + 1u) % n;
        let ax = data[at + 1u + 2u * i];
        let ay = data[at + 2u + 2u * i];
        let bx = data[at + 1u + 2u * j];
        let by = data[at + 2u + 2u * j];
        let c = (bx - ax) * (y - ay) - (by - ay) * (x - ax);
        if (abs(c) < 1e-12) {
            continue;
        }
        if (sgn == 0.0) {
            sgn = sign(c);
        } else if (sign(c) != sgn) {
            return false;
        }
    }
    return true;
}

@compute @workgroup_size(16, 16)
fn fg2_convex(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!fg2_inside(p)) {
        return;
    }
    let stride = P.u[0].y;
    let rg = fg2_tile(p, P.u[0].z);
    var o = 0.0;
    for (var k = rg.x; k < rg.y; k++) {
        let at = P.u[0].x + u32(data[k]) * stride;
        if (!fg2_in_bounds(p, at, stride)) {
            continue;
        }
        var hit = 0;
        for (var sy = 0; sy < 4; sy++) {
            for (var sx = 0; sx < 4; sx++) {
                if (fg2_in_poly(at, f32(p.x) + (f32(sx) + 0.5) / 4.0, f32(p.y) + (f32(sy) + 0.5) / 4.0)) {
                    hit += 1;
                }
            }
        }
        o = max(o, f32(hit) / 16.0);
    }
    textureStore(out, p, vec4<f32>(o, 0.0, 0.0, 0.0));
}

// ---------------------------------------------------------------- painting
// generate3::paint (style 0 on the layer, 1 on transparency, 2 reveal) and Vegas' Composite
// Under (3). aux = coverage (x); u[0].x = style; f[0] = colour; f[1].x = opacity.

@compute @workgroup_size(16, 16)
fn fg2_paint(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!fg2_inside(p)) {
        return;
    }
    let px = textureLoad(src, p, 0);
    let c = textureLoad(aux, p, 0).x;
    let col = P.f[0];
    var o: vec4<f32>;
    if (P.u[0].x == 3u) {
        let a = c * col.w;
        o = fg2_over(vec4<f32>(col.xyz * a, a), px);
    } else {
        let a = clamp(c * P.f[1].x * col.w, 0.0, 1.0);
        let s = vec4<f32>(col.xyz * a, a);
        switch P.u[0].x {
            case 1u: {
                o = s;
            }
            case 2u: {
                o = px * a;
            }
            default: {
                o = fg2_over(px, s);
            }
        }
    }
    textureStore(out, p, o);
}

// generate3::draw_marks: aux = (core, halo); u[0].x = composite; f[0] = inside, f[1] = outside.
@compute @workgroup_size(16, 16)
fn fg2_marks(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!fg2_inside(p)) {
        return;
    }
    let cv = textureLoad(aux, p, 0);
    let c = cv.x;
    let a = max(c, cv.y);
    var base = vec4<f32>(0.0);
    if (P.u[0].x != 0u) {
        base = textureLoad(src, p, 0);
    }
    if (a <= 0.0) {
        textureStore(out, p, base);
        return;
    }
    let t = c / a;
    let col = P.f[1].xyz + (P.f[0].xyz - P.f[1].xyz) * t;
    textureStore(out, p, fg2_over(base, vec4<f32>(col * a, a)));
}

// (src.x, aux.x) into (x, y), src.y into z: a coverage next to its blurred copy.
@compute @workgroup_size(16, 16)
fn fg2_pair(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!fg2_inside(p)) {
        return;
    }
    let s = textureLoad(src, p, 0);
    textureStore(out, p, vec4<f32>(s.x, textureLoad(aux, p, 0).x, s.y, 0.0));
}

// textfx::lightning's composite: aux = (outer, glow, core); u[0].x = blending mode;
// f[0] = outside colour, f[1] = inside colour.
@compute @workgroup_size(16, 16)
fn fg2_bolt(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!fg2_inside(p)) {
        return;
    }
    let px = textureLoad(src, p, 0);
    let m = textureLoad(aux, p, 0);
    let o = max(m.x, m.y * 0.8);
    let c = m.z;
    let a = min(max(o, c), 1.0);
    if (a <= 0.0) {
        textureStore(out, p, px);
        return;
    }
    let t = c / a;
    let oc = P.f[0].xyz;
    let colr = oc + (P.f[1].xyz - oc) * t;
    var r: vec4<f32>;
    switch P.u[0].x {
        case 1u: {
            r = vec4<f32>(px.xyz + colr * a, min(px.w + a * (1.0 - px.w), 1.0));
        }
        case 2u: {
            let s = vec4<f32>(colr * a, a);
            r = px + s - px * s;
        }
        default: {
            r = fg2_over(px, vec4<f32>(colr * a, a));
        }
    }
    textureStore(out, p, r);
}

// generate2::advanced_lightning's composite: aux = (core, blurred core); u[0] = (composite, glow);
// f[0] = (core opacity, glow opacity); f[1] = core colour, f[2] = glow colour.
@compute @workgroup_size(16, 16)
fn fg2_adv_bolt(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!fg2_inside(p)) {
        return;
    }
    let px = textureLoad(src, p, 0);
    let m = textureLoad(aux, p, 0);
    let cv = m.x * P.f[0].x;
    var g = 0.0;
    if (P.u[0].y != 0u) {
        g = min(m.y * 3.0, 1.0);
    }
    let gv = g * P.f[0].y;
    let l = P.f[1].xyz * cv + P.f[2].xyz * gv;
    let la = min(max(cv, gv), 1.0);
    if (P.u[0].x != 0u) {
        let k = clamp(la, 0.0, 1.0);
        textureStore(out, p, vec4<f32>(px.xyz + l, clamp(px.w + k * (1.0 - px.w), 0.0, 1.0)));
    } else {
        textureStore(out, p, vec4<f32>(l, la));
    }
}

// textfx::text_px over the previous pass: aux = (fill, outer, inner) coverages.
// u[0] = (display, keep the layer, stroked, inner ring); f[0] = fill, f[1] = stroke colour;
// f[2].x = opacity.
fn fg2_tint(c: vec4<f32>, a0: f32) -> vec4<f32> {
    let a = clamp(a0 * c.w, 0.0, 1.0);
    return vec4<f32>(c.xyz * a, a);
}

@compute @workgroup_size(16, 16)
fn fg2_text(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!fg2_inside(p)) {
        return;
    }
    let cov = textureLoad(aux, p, 0);
    let op = P.f[2].x;
    let f = cov.x * op;
    var ring = 0.0;
    if (P.u[0].z != 0u) {
        var inner = 0.0;
        if (P.u[0].w != 0u) {
            inner = cov.z;
        }
        ring = max(cov.y - inner, 0.0);
    }
    let s = ring * op;
    var o = vec4<f32>(0.0);
    if (P.u[0].y != 0u) {
        o = textureLoad(src, p, 0);
    }
    let fill = fg2_tint(P.f[0], f);
    let stroke = fg2_tint(P.f[1], s);
    switch P.u[0].x {
        case 1u: {
            o = fg2_over(o, stroke);
        }
        case 2u: {
            o = fg2_over(fg2_over(o, stroke), fill);
        }
        case 3u: {
            o = fg2_over(fg2_over(o, fill), stroke);
        }
        default: {
            o = fg2_over(o, fill);
        }
    }
    textureStore(out, p, o);
}

// ---------------------------------------------------------------- Radio Waves (generate2::radio_waves)
// data: waves (stride 13: cx, cy, radius, half width, rotation, fade, colour rgba, opacity,
// outline offset, outline vertices), then outline vertices (x, y relative to the centre).
// aux = the contour's signed distance (x) when u[0].y. u[0] = (waves, contour, profile);
// f[0] = (anti-aliasing, anchor x, anchor y).

fn fg2_rw_profile(kind: u32, u0: f32) -> f32 {
    let u = clamp(u0, -1.0, 1.0);
    switch kind {
        case 1u: {
            return (1.0 - u) * 0.5;
        }
        case 2u: {
            return (1.0 + u) * 0.5;
        }
        case 3u: {
            return 1.0 - abs(u);
        }
        case 4u: {
            return cos(u * 1.5707963267948966);
        }
        case 5u: {
            if (u < 0.0) {
                return 1.0 + u;
            }
            return 1.0;
        }
        case 6u: {
            if (u > 0.0) {
                return 1.0 - u;
            }
            return 1.0;
        }
        default: {
            return 1.0;
        }
    }
}

// generate2::outline_dist.
fn fg2_outline_dist(off: u32, n: u32, dx: f32, dy: f32) -> f32 {
    let r = sqrt(dx * dx + dy * dy);
    if (r < 1e-9) {
        var m = 3.0e38;
        for (var k = 0u; k < n; k++) {
            let x = data[off + 2u * k];
            let y = data[off + 2u * k + 1u];
            m = min(m, sqrt(x * x + y * y));
        }
        return m;
    }
    let dir = vec2<f32>(dx / r, dy / r);
    var best = 3.0e38;
    for (var k = 0u; k < n; k++) {
        let j = (k + 1u) % n;
        let p0 = vec2<f32>(data[off + 2u * k], data[off + 2u * k + 1u]);
        let p1 = vec2<f32>(data[off + 2u * j], data[off + 2u * j + 1u]);
        let e = p1 - p0;
        let den = dir.x * e.y - dir.y * e.x;
        if (abs(den) < 1e-12) {
            continue;
        }
        let t = (p0.x * e.y - p0.y * e.x) / den;
        let q = vec2<f32>(dir.x * t - p0.x, dir.y * t - p0.y);
        let el2 = max(e.x * e.x + e.y * e.y, 1e-12);
        let s = (q.x * e.x + q.y * e.y) / el2;
        if (t <= 0.0 || s < -1e-5 || s > 1.0 + 1e-5) {
            continue;
        }
        best = min(best, abs(r - t) * abs(den) / sqrt(el2));
    }
    return best;
}

// util::point_in_poly (even-odd).
fn fg2_point_in_poly(off: u32, n: u32, x: f32, y: f32) -> bool {
    if (n < 3u) {
        return false;
    }
    var inside = false;
    var j = n - 1u;
    for (var i = 0u; i < n; i++) {
        let ax = data[off + 2u * i];
        let ay = data[off + 2u * i + 1u];
        let bx = data[off + 2u * j];
        let by = data[off + 2u * j + 1u];
        if ((ay > y) != (by > y) && x < (bx - ax) * (y - ay) / (by - ay) + ax) {
            inside = !inside;
        }
        j = i;
    }
    return inside;
}

@compute @workgroup_size(16, 16)
fn fg2_radio(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!fg2_inside(p)) {
        return;
    }
    var o = textureLoad(src, p, 0);
    let aa = P.f[0].x;
    for (var w = 0u; w < P.u[0].x; w++) {
        let at = w * 13u;
        let dx = f32(p.x) + 0.5 - data[at];
        let dy = f32(p.y) + 0.5 - data[at + 1u];
        let radius = data[at + 2u];
        let hw = data[at + 3u];
        let n = u32(data[at + 12u]);
        var d: f32;
        if (P.u[0].y != 0u) {
            let rot = data[at + 4u];
            let s = sin(-rot);
            let c = cos(-rot);
            let rx = dx * c - dy * s;
            let ry = dx * s + dy * c;
            d = sample_bilinear_clamped(aux, P.f[0].y + rx, P.f[0].z + ry).x - radius;
        } else if (n > 0u) {
            let off = u32(data[at + 11u]);
            let ud = fg2_outline_dist(off, n, dx, dy);
            d = select(ud, -ud, fg2_point_in_poly(off, n, dx, dy));
        } else {
            d = sqrt(dx * dx + dy * dy) - radius;
        }
        let ad = abs(d);
        let cov = clamp((hw - ad) / aa + 0.5, 0.0, 1.0);
        if (cov <= 0.0) {
            continue;
        }
        let u = clamp(d / hw, -1.0, 1.0);
        let col = vec4<f32>(data[at + 6u], data[at + 7u], data[at + 8u], data[at + 9u]);
        let a = cov * fg2_rw_profile(P.u[0].z, u) * data[at + 5u] * data[at + 10u] * col.w;
        o = fg2_over(o, vec4<f32>(col.xyz * a, a));
    }
    textureStore(out, p, o);
}

// ---------------------------------------------------------------- Beam (generate2::beam)
// u[0] = (3D perspective, composite, length > 0); f[0] = (sx, sy, dx, dy); f[1] = (t0, t1,
// start thickness, end thickness); f[2] = (softness, |d|², ...); f[3] = inside, f[4] = outside.

@compute @workgroup_size(16, 16)
fn fg2_beam(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!fg2_inside(p)) {
        return;
    }
    let px = textureLoad(src, p, 0);
    let s = P.f[0].xy;
    let d = P.f[0].zw;
    let t0 = P.f[1].x;
    let t1 = P.f[1].y;
    let soft = P.f[2].x;
    let vx = f32(p.x) + 0.5 - s.x;
    let vy = f32(p.y) + 0.5 - s.y;
    let tc = clamp((vx * d.x + vy * d.y) / P.f[2].y, t0, t1);
    let ex = vx - d.x * tc;
    let ey = vy - d.y * tc;
    let dist = sqrt(ex * ex + ey * ey);
    var tk = tc;
    if (!(P.u[0].x != 0u || t1 - t0 < 1e-9)) {
        tk = (tc - t0) / (t1 - t0);
    }
    let hw = max((P.f[1].z + (P.f[1].w - P.f[1].z) * tk) * 0.5, 0.25);
    let tt = dist / hw;
    var a = 0.0;
    if (P.u[0].z == 0u) {
        a = 0.0;
    } else if (soft > 1e-3) {
        a = 1.0 - fg2_smoothstep(1.0 - soft, 1.0, tt);
    } else {
        a = clamp(hw - dist + 0.5, 0.0, 1.0);
    }
    let cc = P.f[3].xyz + (P.f[4].xyz - P.f[3].xyz) * fg2_smoothstep(0.0, 1.0, tt);
    let g = vec4<f32>(cc * a, a);
    if (P.u[0].y != 0u) {
        textureStore(out, p, fg2_over(px, g));
    } else {
        textureStore(out, p, g);
    }
}

// ---------------------------------------------------------------- Lens Flare (generate2::lens_flare)
// data: ghosts (t, r, colour rgb, k, ring); u[0] = ghosts; f[0] = (fx, fy, cx, cy);
// f[1] = (diagonal, core k, rays, halo r); f[2] = (brightness, blend).

@compute @workgroup_size(16, 16)
fn fg2_flare(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!fg2_inside(p)) {
        return;
    }
    let px = textureLoad(src, p, 0);
    let f = P.f[0].xy;
    let c = P.f[0].zw;
    let diag = P.f[1].x;
    let dx = f32(p.x) + 0.5 - f.x;
    let dy = f32(p.y) + 0.5 - f.y;
    let r = sqrt(dx * dx + dy * dy) / diag;
    let th = atan2(dy, dx);
    let ck = r / P.f[1].y;
    let core = 1.5 * exp(-(ck * ck)) + 0.25 * exp(-r / 0.12);
    let streak = powz(abs(cos(P.f[1].z * 0.5 * th)), 40.0) * exp(-r / 0.3) * 0.35;
    let hk = (r - P.f[1].w) / 0.012;
    let halo = exp(-(hk * hk)) * 0.12;
    let base = core + streak;
    var l = vec3<f32>(base, base * 0.95, base * 0.85);
    l += vec3<f32>(halo * 0.6, halo * 0.8, halo);
    for (var i = 0u; i < P.u[0].x; i++) {
        let at = i * 7u;
        let gt = data[at];
        let gr = data[at + 1u];
        let gx = f.x + (c.x - f.x) * gt;
        let gy = f.y + (c.y - f.y) * gt;
        let ex = f32(p.x) + 0.5 - gx;
        let ey = f32(p.y) + 0.5 - gy;
        let d = sqrt(ex * ex + ey * ey) / diag;
        var v: f32;
        if (data[at + 6u] != 0.0) {
            let q = (d - gr) / (gr * 0.12);
            v = exp(-(q * q));
        } else {
            v = fg2_smoothstep(gr, gr * 0.8, d);
        }
        l += vec3<f32>(data[at + 2u], data[at + 3u], data[at + 4u]) * data[at + 5u] * v;
    }
    l = l * P.f[2].x;
    let la = min(max(max(l.x, l.y), l.z), 1.0);
    let k = clamp(la, 0.0, 1.0);
    let o = vec4<f32>(px.xyz + l, clamp(px.w + k * (1.0 - px.w), 0.0, 1.0));
    textureStore(out, p, o + (px - o) * P.f[2].y);
}

// ---------------------------------------------------------------- CC Glue Gun (generate3::glue_gun)
// data: beads (x, y, radius); u[0] = (beads, glue only); f[0] = (light x, y, z, strength);
// f[1].x = strength weight (clamp(strength, 0, 1) max 0.3).

@compute @workgroup_size(16, 16)
fn fg2_glue(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!fg2_inside(p)) {
        return;
    }
    let px = textureLoad(src, p, 0);
    let glue_only = P.u[0].y != 0u;
    let fx = f32(p.x) + 0.5;
    let fy = f32(p.y) + 0.5;
    var best_h = 0.0;
    var best_d = vec2<f32>(0.0);
    var best_r = 1.0;
    for (var i = 0u; i < P.u[0].x; i++) {
        let r = data[i * 3u + 2u];
        let dx = (fx - data[i * 3u]) / r;
        let dy = (fy - data[i * 3u + 1u]) / r;
        let d2 = dx * dx + dy * dy;
        if (d2 < 1.0) {
            let hgt = sqrt(1.0 - d2);
            if (hgt > best_h) {
                best_h = hgt;
                best_d = vec2<f32>(dx, dy);
                best_r = r;
            }
        }
    }
    if (best_h <= 0.0) {
        textureStore(out, p, select(px, vec4<f32>(0.0), glue_only));
        return;
    }
    let nrm = vec3<f32>(best_d, best_h);
    let ndl = max(dot(nrm, P.f[0].xyz), 0.0);
    let spec = powz(ndl, 24.0);
    let strength = P.f[0].w;
    let s = sample_bilinear_clamped(src, fx - nrm.x * best_r * 0.5 * strength, fy - nrm.y * best_r * 0.5 * strength);
    let edge = fg2_smoothstep(0.0, 0.15, best_h);
    let shade = 0.65 + 0.35 * ndl;
    let g = vec4<f32>(s.xyz * shade + vec3<f32>(spec), max(s.w, 0.6));
    let gp = vec4<f32>(min(g.xyz, vec3<f32>(g.w + spec)), min(g.w, 1.0));
    var base = px;
    if (glue_only) {
        base = vec4<f32>(0.0);
    }
    var o = base + (gp - base) * (edge * P.f[1].x);
    o.w = clamp(o.w, 0.0, 1.0);
    textureStore(out, p, o);
}

// ---------------------------------------------------------------- CC Threads (generate3::threads)
// u[0] = (overlaps); f[0] = (cx, cy, sin, cos); f[1] = (thread width, height, coverage,
// shadowing); f[2].x = texture.

@compute @workgroup_size(16, 16)
fn fg2_threads(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!fg2_inside(p)) {
        return;
    }
    let c = P.f[0].xy;
    let sn = P.f[0].z;
    let cs = P.f[0].w;
    let tw = P.f[1].x;
    let th = P.f[1].y;
    let coverage = P.f[1].z;
    let shadow = P.f[1].w;
    let tex_amt = P.f[2].x;
    let overlaps = i32(P.u[0].x);
    let dx = f32(p.x) + 0.5 - c.x;
    let dy = f32(p.y) + 0.5 - c.y;
    let u = dx * cs + dy * sn;
    let v = -dx * sn + dy * cs;
    let ci = i32(floor(u / tw));
    let cj = i32(floor(v / th));
    let fu = u / tw - f32(ci);
    let fv = v / th - f32(cj);
    let in_h = abs(fv - 0.5) < coverage * 0.5;
    let in_v = abs(fu - 0.5) < coverage * 0.5;
    let h_top = imod(ci + cj, 2 * overlaps) < overlaps;
    var horizontal: bool;
    if (in_h && in_v) {
        horizontal = h_top;
    } else if (in_h) {
        horizontal = true;
    } else if (in_v) {
        horizontal = false;
    } else {
        textureStore(out, p, vec4<f32>(0.0));
        return;
    }
    var su = u;
    var sv = (f32(cj) + 0.5) * th;
    if (!horizontal) {
        su = (f32(ci) + 0.5) * tw;
        sv = v;
    }
    let s = sample_bilinear_clamped(src, c.x + su * cs - sv * sn, c.y + su * sn + sv * cs);
    let half_cov = max(coverage * 0.5, 1e-6);
    var across = (fu - 0.5) / half_cov;
    var along = fv;
    if (horizontal) {
        across = (fv - 0.5) / half_cov;
        along = fu;
    }
    let profile = sqrt(max(1.0 - across * across, 0.0));
    let dive = 1.0 - abs(along - 0.5) * 2.0;
    let under = select(1.0, 0.0, horizontal == h_top);
    let shade = 1.0 - shadow * (1.0 - profile) * 0.8 - shadow * 0.25 * (1.0 - dive) * under;
    var fib = 1.0;
    if (tex_amt > 0.0) {
        let seed = bitcast<u32>(ci) ^ (bitcast<u32>(cj) << 8u);
        fib = 1.0 - tex_amt * 0.3 * hash_noise(u32(max(along * 64.0, 0.0)), u32(max(across * 8.0 + 8.0, 0.0)), seed);
    }
    let k = max(shade * fib, 0.0);
    textureStore(out, p, vec4<f32>(s.xyz * k, s.w));
}

// ---------------------------------------------------------------- Paint Bucket / Eyedropper Fill (generate3)

// Paint Bucket's fill: aux = coverage (x); u[0] = (mode); f[0] = colour; f[1].x = opacity.
@compute @workgroup_size(16, 16)
fn fg2_bucket(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!fg2_inside(p)) {
        return;
    }
    let px = textureLoad(src, p, 0);
    let c = textureLoad(aux, p, 0).x;
    let col = P.f[0];
    let mode = P.u[0].x;
    let k = clamp(c * P.f[1].x * col.w, 0.0, 1.0);
    if (k <= 0.0) {
        textureStore(out, p, select(px, vec4<f32>(0.0), mode == 9u));
        return;
    }
    let dc = fg2_unpremul(px);
    let da = select(max(px.w, 0.0), px.w, px.w > 1e-6);
    let fc = col.xyz;
    var mixed = fc;
    switch mode {
        case 1u: {
            mixed = dc + fc;
        }
        case 2u: {
            mixed = dc * fc;
        }
        case 3u: {
            mixed = vec3<f32>(1.0) - (vec3<f32>(1.0) - dc) * (vec3<f32>(1.0) - fc);
        }
        case 4u, 5u, 6u, 7u: {
            let d = rgb_to_hsl(dc);
            let f = rgb_to_hsl(fc);
            switch mode {
                case 4u: {
                    mixed = hsl_to_rgb(f.x, d.y, d.z);
                }
                case 5u: {
                    mixed = hsl_to_rgb(d.x, f.y, d.z);
                }
                case 6u: {
                    mixed = hsl_to_rgb(f.x, f.y, d.z);
                }
                default: {
                    mixed = hsl_to_rgb(d.x, d.y, f.z);
                }
            }
        }
        default: {}
    }
    var o: vec4<f32>;
    switch mode {
        case 8u: {
            o = fg2_over(vec4<f32>(fc * k, k), px);
        }
        case 9u: {
            o = vec4<f32>(fc * k, k);
        }
        case 0u: {
            o = fg2_over(px, vec4<f32>(fc * k, k));
        }
        default: {
            if (da <= 1e-6) {
                o = px;
            } else {
                let c3 = dc + (mixed - dc) * k;
                o = vec4<f32>(c3 * da, da);
            }
        }
    }
    textureStore(out, p, o);
}

// Paint Bucket's coverage from the region (x): u[0].x = 0 antialias (aux = the blurred region),
// 1 = view threshold, 2 = stroke ring (aux = the eroded region, src = the dilated one).
@compute @workgroup_size(16, 16)
fn fg2_bucket_cov(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!fg2_inside(p)) {
        return;
    }
    let a = textureLoad(src, p, 0).x;
    let b = textureLoad(aux, p, 0).x;
    switch P.u[0].x {
        case 0u: {
            // src = the region, aux = its blur.
            textureStore(out, p, vec4<f32>(select(b, max(b, 0.5), a > 0.0), 0.0, 0.0, 0.0));
        }
        case 1u: {
            textureStore(out, p, vec4<f32>(a, a, a, 1.0));
        }
        default: {
            textureStore(out, p, vec4<f32>(max(a - b, 0.0), 0.0, 0.0, 0.0));
        }
    }
}

// Eyedropper Fill: u[0].x = keep alpha; f[0] = fill (straight, alpha); f[1].x = blend.
@compute @workgroup_size(16, 16)
fn fg2_eyedropper(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!fg2_inside(p)) {
        return;
    }
    let px = textureLoad(src, p, 0);
    let a = select(P.f[0].w, px.w, P.u[0].x != 0u);
    let f = vec4<f32>(P.f[0].xyz * a, a);
    textureStore(out, p, f + (px - f) * P.f[1].x);
}

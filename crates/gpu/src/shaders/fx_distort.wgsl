// GPU effects (distort family): see src/fx_distort.rs. Each entry point mirrors the CPU effect in
// effectcraft-effects operation for operation; parameter layouts are documented per entry.
// Coordinates are pixel centres (x + 0.5, y + 0.5) in buffer pixels, as in util::remap.

const DST_TAU: f32 = 6.2831855;
const DST_HALF_PI: f32 = 1.5707964;

fn dst_centre(p: vec2<i32>) -> vec2<f32> {
    return vec2<f32>(f32(p.x) + 0.5, f32(p.y) + 0.5);
}

fn dst_inside(p: vec2<i32>) -> bool {
    let d = out_dims();
    return p.x < d.x && p.y < d.y;
}

// ---------------------------------------------------------------- Twirl (distort::twirl)
// f[0] = (cx, cy, radius, angle in radians)
@compute @workgroup_size(16, 16)
fn dst_twirl(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!dst_inside(p)) {
        return;
    }
    let q = dst_centre(p);
    let c = P.f[0].xy;
    let r = P.f[0].z;
    let dx = q.x - c.x;
    let dy = q.y - c.y;
    let d = sqrt(dx * dx + dy * dy);
    var s = q;
    if (d < r) {
        let t = 1.0 - d / r;
        let a = -P.f[0].w * t * t;
        let sn = sin(a);
        let co = cos(a);
        s = vec2<f32>(c.x + dx * co - dy * sn, c.y + dx * sn + dy * co);
    }
    textureStore(out, p, sample_bilinear(src, s.x, s.y));
}

// ---------------------------------------------------------------- Bulge (distort::bulge)
// f[0] = (cx, cy, rx, ry); f[1] = (height, taper, pin, edge); f[2] = layer rect (x, y, w, h)
@compute @workgroup_size(16, 16)
fn dst_bulge(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!dst_inside(p)) {
        return;
    }
    let q = dst_centre(p);
    let c = P.f[0].xy;
    let rx = P.f[0].z;
    let ry = P.f[0].w;
    let dx = (q.x - c.x) / rx;
    let dy = (q.y - c.y) / ry;
    let d2 = dx * dx + dy * dy;
    var s = q;
    if (d2 < 1.0) {
        var amt = P.f[1].x * pow(1.0 - d2, P.f[1].y) * 0.5;
        if (P.f[1].z > 0.5) {
            let l = P.f[2];
            let edge = P.f[1].w;
            let ex = clamp(min(q.x - l.x - 0.5, l.x + l.z - 0.5 - q.x) / edge, 0.0, 1.0);
            let ey = clamp(min(q.y - l.y - 0.5, l.y + l.w - 0.5 - q.y) / edge, 0.0, 1.0);
            amt *= min(ex, ey);
        }
        let k = 1.0 - amt;
        s = vec2<f32>(c.x + dx * rx * k, c.y + dy * ry * k);
    }
    textureStore(out, p, sample_bilinear(src, s.x, s.y));
}

// ---------------------------------------------------------------- Wave Warp (distort::wave_warp)

fn dst_half_circle(u: f32) -> f32 {
    let v = 2.0 * u - 1.0;
    return sqrt(max(1.0 - v * v, 0.0));
}

fn dst_wave_noise(i: f32, seed: u32) -> f32 {
    return hash_noise(bitcast<u32>(i32(i)), 0x5eedu, seed) * 2.0 - 1.0;
}

// distort::wave_shape at the phase `cf + ci` in cycles (t / 2π), split into a fraction cf in
// [0, 2) and whole cycles ci so the fraction keeps full precision.
fn dst_wave_shape(kind: u32, cf: f32, ci: f32, seed: u32) -> f32 {
    let ph = cf - floor(cf);
    switch kind {
        case 1u: {
            return select(-1.0, 1.0, ph < 0.5);
        }
        case 2u: {
            return 1.0 - 4.0 * abs(ph - 0.5);
        }
        case 3u: {
            return 2.0 * ph - 1.0;
        }
        case 4u: {
            if (ph < 0.5) {
                return dst_half_circle(ph * 2.0);
            }
            return -dst_half_circle(ph * 2.0 - 1.0);
        }
        case 5u: {
            return 2.0 * dst_half_circle(fract(ph * 2.0)) - 1.0;
        }
        case 6u: {
            return 1.0 - 2.0 * dst_half_circle(fract(ph * 2.0));
        }
        case 7u: {
            return dst_wave_noise(floor(cf * 2.0) + 2.0 * ci, seed);
        }
        case 8u: {
            let u = cf * 2.0;
            let i = floor(u);
            let f = u - i;
            let s = f * f * (3.0 - 2.0 * f);
            let n = i + 2.0 * ci;
            return dst_wave_noise(n, seed) * (1.0 - s) + dst_wave_noise(n + 1.0, seed) * s;
        }
        default: {
            return sin(ph * DST_TAU);
        }
    }
}

// distort::wave_pin.
fn dst_wave_pin(pin: u32, x: f32, y: f32, r: vec4<f32>) -> f32 {
    let u = clamp((x - r.x - 0.5) / max(r.z - 1.0, 1.0), 0.0, 1.0);
    let v = clamp((y - r.y - 0.5) / max(r.w - 1.0, 1.0), 0.0, 1.0);
    let horiz = clamp(min(v, 1.0 - v) * 2.0, 0.0, 1.0);
    let vert = clamp(min(u, 1.0 - u) * 2.0, 0.0, 1.0);
    switch pin {
        case 1u: { return min(horiz, vert); }
        case 2u: { return min(sqrt((u - 0.5) * (u - 0.5) + (v - 0.5) * (v - 0.5)) * 2.0, 1.0); }
        case 3u: { return u; }
        case 4u: { return v; }
        case 5u: { return 1.0 - u; }
        case 6u: { return 1.0 - v; }
        case 7u: { return horiz; }
        case 8u: { return vert; }
        default: { return 1.0; }
    }
}

// u[0] = (wave type, pinning, seed, buffer width); f[0] = (dx, dy, height); f[2] = layer rect
// (x, y, w, h). data: the phase in cycles (along / width + phase / 2π) as per-column and per-row
// (fraction, whole cycles) pairs: fx (w), ix (w), fy (h), iy (h).
@compute @workgroup_size(16, 16)
fn dst_wavewarp(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!dst_inside(p)) {
        return;
    }
    let q = dst_centre(p);
    let dx = P.f[0].x;
    let dy = P.f[0].y;
    let w = P.u[0].w;
    let x = u32(p.x);
    let y = u32(p.y);
    let cf = data[x] + data[2u * w + y];
    let ci = data[w + x] + data[2u * w + u32(out_dims().y) + y];
    let disp = dst_wave_shape(P.u[0].x, cf, ci, P.u[0].z) * P.f[0].z * dst_wave_pin(P.u[0].y, q.x, q.y, P.f[2]);
    textureStore(out, p, sample_bilinear(src, q.x + dy * disp, q.y - dx * disp));
}

// ---------------------------------------------------------------- Ripple (distort::ripple)
// u[0].x = asymmetric; f[0] = (cx, cy, radius, wave width); f[1] = (wave height, phase mod 2π)
@compute @workgroup_size(16, 16)
fn dst_ripple(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!dst_inside(p)) {
        return;
    }
    let q = dst_centre(p);
    let c = P.f[0].xy;
    let r = P.f[0].z;
    let dx = q.x - c.x;
    let dy = q.y - c.y;
    let d = sqrt(dx * dx + dy * dy);
    var s = q;
    if (d < r && d != 0.0) {
        let h = P.f[1].x;
        let fall = 1.0 - d / r;
        let arg = d / P.f[0].w * DST_TAU + P.f[1].y;
        let o = sin(arg) * h * fall;
        var t = 0.0;
        if (P.u[0].x != 0u) {
            t = cos(arg) * h * fall * 0.5;
        }
        let ux = dx / d;
        let uy = dy / d;
        s = vec2<f32>(q.x + ux * o - uy * t, q.y + uy * o + ux * t);
    }
    textureStore(out, p, sample_bilinear(src, s.x, s.y));
}

// ---------------------------------------------------------------- CC Lens (distort2::cc_lens)
// f[0] = (cx, cy, radius, convergence exponent)
@compute @workgroup_size(16, 16)
fn dst_cclens(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!dst_inside(p)) {
        return;
    }
    let q = dst_centre(p);
    let c = P.f[0].xy;
    let r = P.f[0].z;
    let dx = q.x - c.x;
    let dy = q.y - c.y;
    let d = sqrt(dx * dx + dy * dy);
    if (d >= r) {
        textureStore(out, p, vec4<f32>(0.0));
        return;
    }
    var s = q;
    if (d >= 1e-9) {
        let g = powz(asin_p(min(d / r, 1.0)) / DST_HALF_PI, P.f[0].w);
        let k = r * g / d;
        s = vec2<f32>(c.x + dx * k, c.y + dy * k);
    }
    textureStore(out, p, sample_bilinear(src, s.x, s.y));
}

// ---------------------------------------------------------------- Turbulent Displace (distort2)

// distort2::fbm. u[0].w = octave count; f[1].w = fraction of the last octave; f[3].y = falloff.
fn dst_fbm(u: f32, v: f32, z: f32, seed: u32) -> f32 {
    let n = P.u[0].w;
    let frac = P.f[1].w;
    let falloff = P.f[3].y;
    var sum = 0.0;
    var norm = 0.0;
    var amp = 1.0;
    var f = 1.0;
    for (var o = 0u; o < n; o++) {
        var w = 1.0;
        if (o + 1u == n && frac > 0.0) {
            w = frac;
        }
        sum += value_noise_linear_z(u * f, v * f, z + f32(o) * 5.17, seed + o * 101u) * amp * w;
        norm += amp * w;
        amp *= falloff;
        f *= 2.0;
    }
    return sum / max(norm, 1e-6);
}

// The centred noise `n(u, v, s)` (cross-faded over the evolution cycle when u[1].x is set).
// f[1] = (z, z - cycle, cycle position t).
fn dst_turb_n(u: f32, v: f32, s: u32) -> f32 {
    let seed = P.u[0].z + s;
    if (P.u[1].x != 0u) {
        let t = P.f[1].z;
        return dst_fbm(u, v, P.f[1].x, seed) * (1.0 - t) + dst_fbm(u, v, P.f[1].y, seed) * t - 0.5;
    }
    return dst_fbm(u, v, P.f[1].x, seed) - 0.5;
}

// u[0] = (displacement kind, pinning, seed, octaves); u[1].x = cycle evolution;
// f[0] = (offset x, offset y, size, amount); f[1] = (z0, z1, t, last-octave fraction);
// f[2] = layer rect (x, y, w, h); f[3] = (edge, falloff)
@compute @workgroup_size(16, 16)
fn dst_turbulent(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!dst_inside(p)) {
        return;
    }
    let q = dst_centre(p);
    let kind = P.u[0].x;
    let amount = P.f[0].w;
    let u = (q.x - P.f[0].x) / P.f[0].z;
    let v = (q.y - P.f[0].y) / P.f[0].z;
    var d = vec2<f32>(0.0);
    switch kind {
        case 1u, 2u, 4u, 5u: {
            let e = 0.05;
            let gx = (dst_turb_n(u + e, v, 0u) - dst_turb_n(u - e, v, 0u)) / (2.0 * e);
            let gy = (dst_turb_n(u, v + e, 0u) - dst_turb_n(u, v - e, 0u)) / (2.0 * e);
            let k = amount * 0.25;
            if (kind == 1u || kind == 4u) {
                d = vec2<f32>(gx * k, gy * k);
            } else {
                d = vec2<f32>(-gy * k, gx * k);
            }
        }
        case 6u: {
            d = vec2<f32>(0.0, dst_turb_n(u, v, 17u) * amount);
        }
        case 7u: {
            d = vec2<f32>(dst_turb_n(u, v, 0u) * amount, 0.0);
        }
        case 8u: {
            let a = dst_turb_n(u, v, 0u) * amount;
            d = vec2<f32>(a, a);
        }
        default: {
            d = vec2<f32>(dst_turb_n(u, v, 0u) * amount, dst_turb_n(u, v, 17u) * amount);
        }
    }
    let r = P.f[2];
    let edge = P.f[3].x;
    let l = clamp((q.x - r.x - 0.5) / edge, 0.0, 1.0);
    let rr = clamp((r.x + r.z - 0.5 - q.x) / edge, 0.0, 1.0);
    let t = clamp((q.y - r.y - 0.5) / edge, 0.0, 1.0);
    let bt = clamp((r.y + r.w - 0.5 - q.y) / edge, 0.0, 1.0);
    var k = 1.0;
    switch P.u[0].y {
        case 1u: { k = min(min(l, rr), min(t, bt)); }
        case 2u: { k = min(t, bt); }
        case 3u: { k = min(l, rr); }
        case 4u: { k = l; }
        case 5u: { k = t; }
        case 6u: { k = rr; }
        case 7u: { k = bt; }
        default: {}
    }
    textureStore(out, p, sample_bilinear(src, q.x + d.x * k, q.y + d.y * k));
}

// ---------------------------------------------------------------- Displacement Map (distort2)

// util::pick for a straight colour (source index in util::SRC_ORDER).
fn dst_pick(s: u32, c: vec3<f32>, a: f32) -> f32 {
    switch s {
        case 0u: { return c.x; }
        case 1u: { return c.y; }
        case 2u: { return c.z; }
        case 3u: { return a; }
        case 4u: { return luminance(c); }
        case 5u: { return rgb_to_hsl(c).x; }
        case 6u: { return rgb_to_hsl(c).z; }
        case 7u: { return rgb_to_hsl(c).y; }
        case 8u: { return 1.0; }
        case 9u: { return 0.5; }
        default: { return 0.0; }
    }
}

// u[0] = (horizontal source, vertical source, map mode: 0 self, 1 fitted layer (transparent
// outside), 2 tiled layer (edges repeat), flags: 1 wrap, 2 Center Map bounds);
// u[1] = (buffer width, buffer height); f[0] = (max horizontal, max vertical).
// data: per-column map x, per-row map y (the map layer's buffer pixels), per-column and
// per-row "inside the centred map" flags. aux = the map layer.
@compute @workgroup_size(16, 16)
fn dst_displace(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!dst_inside(p)) {
        return;
    }
    let w = P.u[1].x;
    let h = P.u[1].y;
    let x = u32(p.x);
    let y = u32(p.y);
    let px = textureLoad(src, p, 0);
    let flags = P.u[0].w;
    if ((flags & 2u) != 0u && (data[w + h + x] < 0.5 || data[2u * w + h + y] < 0.5)) {
        textureStore(out, p, px);
        return;
    }
    var m = px;
    let mode = P.u[0].z;
    if (mode == 1u) {
        m = sample_bilinear(aux, data[x], data[w + y]);
    } else if (mode == 2u) {
        m = sample_bilinear_clamped(aux, data[x], data[w + y]);
    }
    var c = vec3<f32>(0.0);
    let a = m.w;
    if (a > 1e-6) {
        c = m.xyz / a;
    }
    let al = max(a, 0.0);
    var dx = 0.0;
    var dy = 0.0;
    if (P.u[0].x != 10u) {
        dx = (dst_pick(P.u[0].x, c, al) - 0.5) * 2.0 * P.f[0].x;
    }
    if (P.u[0].y != 10u) {
        dy = (dst_pick(P.u[0].y, c, al) - 0.5) * 2.0 * P.f[0].y;
    }
    if (dx == 0.0 && dy == 0.0) {
        textureStore(out, p, px);
        return;
    }
    var sx = f32(p.x) + 0.5 + dx;
    var sy = f32(p.y) + 0.5 + dy;
    if ((flags & 1u) != 0u) {
        let fw = f32(w);
        let fh = f32(h);
        sx = sx - floor(sx / fw) * fw;
        sy = sy - floor(sy / fh) * fh;
        textureStore(out, p, sample_bilinear_clamped(src, sx, sy));
    } else {
        textureStore(out, p, sample_bilinear(src, sx, sy));
    }
}

// ---------------------------------------------------------------- Mesh Warp (distort2::grid_warp)
// u[0] = (row starts offset, list offset). data: 12 floats per triangle (the barycentric
// coefficients k11 k12 k21 k22 relative to vertex c, c, then the source points a, b, c), then
// per output row the start of its triangle list (height + 1 entries), then the lists (triangle
// indices in the CPU's bucket order).
@compute @workgroup_size(16, 16)
fn dst_mesh(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!dst_inside(p)) {
        return;
    }
    let q = dst_centre(p);
    let rs = P.u[0].x;
    let lo = P.u[0].y;
    let i0 = u32(data[rs + u32(p.y)]);
    let i1 = u32(data[rs + u32(p.y) + 1u]);
    var cur = vec4<f32>(0.0);
    for (var i = i0; i < i1; i++) {
        if (cur.w > 0.0) {
            break;
        }
        let b = u32(data[lo + i]) * 12u;
        let ex = q.x - data[b + 4u];
        let ey = q.y - data[b + 5u];
        let l1 = data[b] * ex + data[b + 1u] * ey;
        let l2 = data[b + 2u] * ex + data[b + 3u] * ey;
        let l3 = 1.0 - l1 - l2;
        // The CPU tests against -1e-9 in f64; f32 rounding needs a wider margin so pixels on
        // shared edges are not dropped by both triangles.
        let e = -1e-5;
        if (l1 < e || l2 < e || l3 < e) {
            continue;
        }
        let sx = data[b + 6u] * l1 + data[b + 8u] * l2 + data[b + 10u] * l3;
        let sy = data[b + 7u] * l1 + data[b + 9u] * l2 + data[b + 11u] * l3;
        cur = sample_bilinear(src, sx, sy);
    }
    textureStore(out, p, cur);
}

// ---------------------------------------------------------------- Mosaic (misc::mosaic)
// data: column tile index (w), row tile index (h), tile column bounds (nx + 1), tile row bounds
// (ny + 1); u[0] = (w, h, nx, ny).

// Row sums of each tile's columns: out is nx × h.
@compute @workgroup_size(16, 16)
fn dst_mosaic_rows(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!dst_inside(p)) {
        return;
    }
    let w = P.u[0].x;
    let h = P.u[0].y;
    let xb = w + h + u32(p.x);
    var s = vec4<f32>(0.0);
    for (var x = u32(data[xb]); x < u32(data[xb + 1u]); x++) {
        s += textureLoad(src, vec2<i32>(i32(x), p.y), 0);
    }
    textureStore(out, p, s);
}

// Tile averages: out is nx × ny (src = the row sums).
@compute @workgroup_size(16, 16)
fn dst_mosaic_tiles(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!dst_inside(p)) {
        return;
    }
    let w = P.u[0].x;
    let h = P.u[0].y;
    let xb = w + h + u32(p.x);
    let yb = w + h + P.u[0].z + 1u + u32(p.y);
    let y0 = u32(data[yb]);
    let y1 = u32(data[yb + 1u]);
    var s = vec4<f32>(0.0);
    for (var y = y0; y < y1; y++) {
        s += textureLoad(src, vec2<i32>(p.x, i32(y)), 0);
    }
    let n = f32((u32(data[xb + 1u]) - u32(data[xb])) * (y1 - y0));
    textureStore(out, p, s / max(n, 1.0));
}

// Each pixel takes its tile's average (src = the tile averages).
@compute @workgroup_size(16, 16)
fn dst_mosaic_out(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!dst_inside(p)) {
        return;
    }
    let tx = i32(data[u32(p.x)]);
    let ty = i32(data[P.u[0].x + u32(p.y)]);
    textureStore(out, p, textureLoad(src, vec2<i32>(tx, ty), 0));
}

// Sharp Colors: data = per-column / per-row source pixel; u[0].x = w.
@compute @workgroup_size(16, 16)
fn dst_mosaic_sharp(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!dst_inside(p)) {
        return;
    }
    let sx = i32(data[u32(p.x)]);
    let sy = i32(data[P.u[0].x + u32(p.y)]);
    textureStore(out, p, tex_get_clamped(src, sx, sy));
}

// ---------------------------------------------------------------- Find Edges / Emboss (misc)

fn dst_blend_with(orig: vec4<f32>, fx: vec4<f32>, k: f32) -> vec4<f32> {
    return fx + (orig - fx) * k;
}

fn dst_lum_at(x: i32, y: i32) -> f32 {
    return luminance(tex_get_clamped(src, x, y).xyz);
}

// u[0].x = invert; f[0].x = blend with original.
@compute @workgroup_size(16, 16)
fn dst_findedges(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!dst_inside(p)) {
        return;
    }
    let x = p.x;
    let y = p.y;
    let gx = dst_lum_at(x + 1, y - 1) + 2.0 * dst_lum_at(x + 1, y) + dst_lum_at(x + 1, y + 1) - dst_lum_at(x - 1, y - 1) - 2.0 * dst_lum_at(x - 1, y)
        - dst_lum_at(x - 1, y + 1);
    let gy = dst_lum_at(x - 1, y + 1) + 2.0 * dst_lum_at(x, y + 1) + dst_lum_at(x + 1, y + 1) - dst_lum_at(x - 1, y - 1) - 2.0 * dst_lum_at(x, y - 1)
        - dst_lum_at(x + 1, y - 1);
    let e = min(sqrt(gx * gx + gy * gy), 1.0);
    var v = 1.0 - e;
    if (P.u[0].x != 0u) {
        v = e;
    }
    let px = textureLoad(src, p, 0);
    let a = px.w;
    textureStore(out, p, dst_blend_with(px, vec4<f32>(v * a, v * a, v * a, a), P.f[0].x));
}

// f[0] = (dx, dy, contrast, blend with original).
@compute @workgroup_size(16, 16)
fn dst_emboss(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!dst_inside(p)) {
        return;
    }
    let q = dst_centre(p);
    let d = P.f[0].xy;
    let a = sample_bilinear_clamped(src, q.x + d.x, q.y + d.y);
    let c = sample_bilinear_clamped(src, q.x - d.x, q.y - d.y);
    let l = luminance(a.xyz) - luminance(c.xyz);
    let v = clamp(0.5 + l * P.f[0].z, 0.0, 1.0);
    let px = textureLoad(src, p, 0);
    let al = px.w;
    textureStore(out, p, dst_blend_with(px, vec4<f32>(v * al, v * al, v * al, al), P.f[0].w));
}

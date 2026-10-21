// GPU effects (noise, blur and time family): see src/fx_noise.rs. Each entry point mirrors the CPU effect in
// effectcraft-effects operation for operation. Every name here is prefixed `fxn_` (the
// family files share one module).

// f32::MAX exactly (a decimal that rounds above it is rejected by browsers' WGSL compilers).
const FXN_MAX: f32 = 3.4028234663852886e38;

fn fxn_pixel(gid: vec3<u32>) -> vec2<i32> {
    let dims = out_dims();
    let p = vec2<i32>(gid.xy);
    if (p.x >= dims.x || p.y >= dims.y) {
        return vec2<i32>(-1);
    }
    return p;
}

// Refine approximate GPU division with two fma residual corrections before restoring
// the median's straight colour. Driver float optimisations can still affect rounding;
// the fixed median levels instead come from a CPU-rounded lookup table.
fn fxn_div4(a: vec4<f32>, b: vec4<f32>) -> vec4<f32> {
    let q = a / b;
    let r = fma(-q, b, a);
    let corrected = fma(r, 1.0 / b, q);
    let residual = fma(-corrected, b, a);
    return fma(residual, 1.0 / b, corrected);
}

// util::unpremul: (straight colour, alpha).
fn fxn_unpremul(p: vec4<f32>) -> vec4<f32> {
    if (p.w > 1e-6) {
        return vec4<f32>(p.xyz / p.w, p.w);
    }
    return vec4<f32>(0.0, 0.0, 0.0, max(p.w, 0.0));
}

// ---------------------------------------------------------------- Median family (noise::median_image)

// Quantise to the median's 512 levels (0..511). u[0].x = 1: straight colour with alpha 1
// (Median (Legacy) colour only).
@compute @workgroup_size(16, 16)
fn fxn_quant(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = fxn_pixel(gid);
    if (p.x < 0) {
        return;
    }
    var v = textureLoad(src, p, 0);
    if (P.u[0].x == 1u) {
        v = vec4<f32>(fxn_unpremul(v).xyz, 1.0);
    }
    let q = clamp(v, vec4<f32>(0.0), vec4<f32>(1.0)) * 511.0;
    textureStore(out, p, vec4<f32>(round_away(q.x), round_away(q.y), round_away(q.z), round_away(q.w)));
}

// Per-channel median of the quantised levels over a (2r+1)² window (edges repeated): the
// smallest level m with more than half the window at or below m. u[0].x = r.
// data = the 512 CPU-rounded level / 511 values.
@compute @workgroup_size(16, 16)
fn fxn_median(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = fxn_pixel(gid);
    if (p.x < 0) {
        return;
    }
    let r = i32(P.u[0].x);
    let n = (2 * r + 1) * (2 * r + 1);
    let half = f32(n / 2);
    var lo = vec4<f32>(0.0);
    var hi = vec4<f32>(511.0);
    for (var it = 0; it < 9; it++) {
        let mid = floor((lo + hi) * 0.5);
        var cnt = vec4<f32>(0.0);
        for (var dy = -r; dy <= r; dy++) {
            for (var dx = -r; dx <= r; dx++) {
                let q = tex_get_clamped(src, p.x + dx, p.y + dy);
                cnt += select(vec4<f32>(0.0), vec4<f32>(1.0), q <= mid);
            }
        }
        let above = cnt > vec4<f32>(half);
        hi = select(hi, mid, above);
        lo = select(mid + 1.0, lo, above);
    }
    let level = vec4<u32>(lo);
    textureStore(out, p, vec4<f32>(data[level.x], data[level.y], data[level.z], data[level.w]));
}

// The same median for large radii, as the CPU computes it: a sliding 512-bin histogram per
// channel (Huang). One invocation per row segment of u[0].y pixels: it fills the histogram for
// the segment's first window, then slides right one column at a time (remove the column
// leaving, add the one entering) and moves each channel's median by the counts below it.
// u[0] = (r, segment length); dispatched over (rows, segments) with 64 × 1 workgroups.
// data = the 512 CPU-rounded level / 511 values.
@compute @workgroup_size(64, 1)
fn fxn_median_huang(@builtin(global_invocation_id) gid: vec3<u32>) {
    let dims = out_dims();
    let y = i32(gid.x);
    let seg = i32(P.u[0].y);
    let x0 = i32(gid.y) * seg;
    if (y >= dims.y || x0 >= dims.x) {
        return;
    }
    let x1 = min(x0 + seg, dims.x);
    let r = i32(P.u[0].x);
    let half = u32((2 * r + 1) * (2 * r + 1) / 2);
    var hist: array<u32, 2048>;
    for (var dy = -r; dy <= r; dy++) {
        for (var dx = -r; dx <= r; dx++) {
            let v = vec4<u32>(tex_get_clamped(src, x0 + dx, y + dy));
            hist[v.x] += 1u;
            hist[512u + v.y] += 1u;
            hist[1024u + v.z] += 1u;
            hist[1536u + v.w] += 1u;
        }
    }
    // Per channel: the median level and the count of values below it.
    var med = vec4<u32>(0u);
    var lt = vec4<u32>(0u);
    for (var x = x0; x < x1; x++) {
        if (x > x0) {
            for (var dy = -r; dy <= r; dy++) {
                let o = vec4<u32>(tex_get_clamped(src, x - 1 - r, y + dy));
                let n = vec4<u32>(tex_get_clamped(src, x + r, y + dy));
                for (var c = 0u; c < 4u; c++) {
                    hist[c * 512u + o[c]] -= 1u;
                    if (o[c] < med[c]) {
                        lt[c] -= 1u;
                    }
                    hist[c * 512u + n[c]] += 1u;
                    if (n[c] < med[c]) {
                        lt[c] += 1u;
                    }
                }
            }
        }
        for (var c = 0u; c < 4u; c++) {
            let base = c * 512u;
            while (lt[c] > half) {
                med[c] -= 1u;
                lt[c] -= hist[base + med[c]];
            }
            while (lt[c] + hist[base + med[c]] <= half) {
                lt[c] += hist[base + med[c]];
                med[c] += 1u;
            }
        }
        textureStore(out, vec2<i32>(x, y), vec4<f32>(data[med.x], data[med.y], data[med.z], data[med.w]));
    }
}

// noise::median_px.
fn fxn_median_px(o: vec4<f32>, m: vec4<f32>, on_alpha: bool) -> vec4<f32> {
    if (on_alpha) {
        let a = clamp(m.w, 0.0, 1.0);
        return vec4<f32>(min(m.xyz, vec3<f32>(a)), a);
    }
    if (m.w <= 1e-4) {
        return o;
    }
    return vec4<f32>(min(fxn_div4(m, vec4<f32>(m.w)).xyz, vec3<f32>(1.0)) * o.w, o.w);
}

// src = the layer, aux = the median. u[0] = (mode, operate on alpha): 0 Median, 1 Dust &
// Scratches (f[0].x = threshold), 2 Median (Legacy) with alpha, 3 Median (Legacy) colour only.
@compute @workgroup_size(16, 16)
fn fxn_median_out(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = fxn_pixel(gid);
    if (p.x < 0) {
        return;
    }
    let o = textureLoad(src, p, 0);
    let m = textureLoad(aux, p, 0);
    let on_alpha = P.u[0].y != 0u;
    var r = o;
    switch P.u[0].x {
        case 0u: {
            r = fxn_median_px(o, m, on_alpha);
        }
        case 1u: {
            let cand = fxn_median_px(o, m, on_alpha);
            let d = abs(cand - o);
            if (max(max(d.x, d.y), max(d.z, d.w)) > P.f[0].x) {
                r = cand;
            }
        }
        case 2u: {
            r = m;
        }
        default: {
            r = vec4<f32>(m.xyz * o.w, o.w);
        }
    }
    textureStore(out, p, r);
}

// ---------------------------------------------------------------- Bilateral / Smart Blur (blur2.rs)

// blur2::edge_preserving. u[0] = (ceil r, stride, hard, largest d²); u[1].x = grey (Bilateral
// with Colorize off); f[0] = (2σs², 2σr², range).
@compute @workgroup_size(16, 16)
fn fxn_edge(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = fxn_pixel(gid);
    if (p.x < 0) {
        return;
    }
    let ri = i32(P.u[0].x);
    let stride = i32(P.u[0].y);
    let hard = P.u[0].z != 0u;
    let d2max = i32(P.u[0].w);
    let sig_s2 = P.f[0].x;
    let sig_r2 = P.f[0].y;
    let range = P.f[0].z;
    let c = textureLoad(src, p, 0);
    let cl = luminance(c.xyz);
    var acc = vec4<f32>(0.0);
    var wsum = 0.0;
    for (var dy = -ri; dy <= ri; dy += stride) {
        for (var dx = -ri; dx <= ri; dx += stride) {
            let d2 = dx * dx + dy * dy;
            if (d2 > d2max) {
                continue;
            }
            let q = tex_get_clamped(src, p.x + dx, p.y + dy);
            var w = 0.0;
            if (hard) {
                w = select(0.0, 1.0, abs(luminance(q.xyz) - cl) <= range);
            } else {
                let d = q - c;
                let dc = d.x * d.x + d.y * d.y + d.z * d.z + d.w * d.w;
                w = exp(-f32(d2) / sig_s2 - dc / sig_r2);
            }
            if (w > 0.0) {
                acc += q * w;
                wsum += w;
            }
        }
    }
    var o = c;
    if (wsum > 0.0) {
        o = acc / wsum;
    }
    if (P.u[1].x != 0u && o.w > 0.0) {
        let l = luminance(o.xyz / o.w);
        o = vec4<f32>(vec3<f32>(l * o.w), o.w);
    }
    textureStore(out, p, o);
}

// Smart Blur's Edge Only / Overlay Edge (blur2::sobel on the blurred layer's premultiplied
// luminance). u[0].x = mode; f[0].x = edge threshold.
@compute @workgroup_size(16, 16)
fn fxn_smart_edges(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = fxn_pixel(gid);
    if (p.x < 0) {
        return;
    }
    var l: array<f32, 9>;
    for (var j = 0; j < 3; j++) {
        for (var i = 0; i < 3; i++) {
            l[j * 3 + i] = luminance(tex_get_clamped(src, p.x + i - 1, p.y + j - 1).xyz);
        }
    }
    let gx = l[2] + 2.0 * l[5] + l[8] - l[0] - 2.0 * l[3] - l[6];
    let gy = l[6] + 2.0 * l[7] + l[8] - l[0] - 2.0 * l[1] - l[2];
    let e = sqrt(gx * gx + gy * gy) * 0.25;
    let px = textureLoad(src, p, 0);
    let on = e > P.f[0].x;
    var o = px;
    if (P.u[0].x == 1u) {
        o = vec4<f32>(vec3<f32>(select(0.0, 1.0, on) * px.w), px.w);
    } else if (on) {
        o = vec4<f32>(vec3<f32>(px.w), px.w);
    }
    textureStore(out, p, o);
}

// ---------------------------------------------------------------- Sharpen / Unsharp Mask (misc::unsharp_core)

// src = the layer, aux = its blur. f[0] = (amount, threshold in 0..255).
@compute @workgroup_size(16, 16)
fn fxn_unsharp(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = fxn_pixel(gid);
    if (p.x < 0) {
        return;
    }
    var o = textureLoad(src, p, 0);
    let bl = textureLoad(aux, p, 0);
    for (var c = 0; c < 3; c++) {
        let d = o[c] - bl[c];
        if (abs(d) * 255.0 >= P.f[0].y) {
            o[c] = max(o[c] + d * P.f[0].x, 0.0);
        }
    }
    textureStore(out, p, o);
}

// ---------------------------------------------------------------- planes

// Channel select: channel c of `src` becomes aux[c] (u[0].x = 0) or aux's alpha (1) where
// u[1][c] != 0.
@compute @workgroup_size(16, 16)
fn fxn_pick(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = fxn_pixel(gid);
    if (p.x < 0) {
        return;
    }
    var o = textureLoad(src, p, 0);
    let a = textureLoad(aux, p, 0);
    for (var c = 0; c < 4; c++) {
        if (P.u[1][c] != 0u) {
            o[c] = select(a[c], a.w, P.u[0].x == 1u);
        }
    }
    textureStore(out, p, o);
}

// Channel Blur's result: src = blurred premultiplied channels, aux = the alpha blurred like
// each channel (alpha = the Alpha Blurriness result).
@compute @workgroup_size(16, 16)
fn fxn_cblur_out(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = fxn_pixel(gid);
    if (p.x < 0) {
        return;
    }
    let v = textureLoad(src, p, 0);
    let ba = textureLoad(aux, p, 0);
    var s = vec3<f32>(0.0);
    for (var c = 0; c < 3; c++) {
        if (ba[c] > 1e-6) {
            s[c] = v[c] / ba[c];
        }
    }
    let a = clamp(ba.w, 0.0, 1.0);
    textureStore(out, p, vec4<f32>(s * a, a));
}

// Straight colour planes with the raw alpha (Minimax).
@compute @workgroup_size(16, 16)
fn fxn_straight(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = fxn_pixel(gid);
    if (p.x < 0) {
        return;
    }
    let v = textureLoad(src, p, 0);
    textureStore(out, p, vec4<f32>(fxn_unpremul(v).xyz, v.w));
}

// Premultiply straight planes by the clamped alpha.
@compute @workgroup_size(16, 16)
fn fxn_premul(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = fxn_pixel(gid);
    if (p.x < 0) {
        return;
    }
    let v = textureLoad(src, p, 0);
    let a = clamp(v.w, 0.0, 1.0);
    textureStore(out, p, vec4<f32>(v.xyz * a, a));
}

// util::morph_plane along one axis (edges repeated). u[0] = (r, vertical); u[1] = per-channel
// operation (0 min, 1 max, 2 keep).
@compute @workgroup_size(16, 16)
fn fxn_morph(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = fxn_pixel(gid);
    if (p.x < 0) {
        return;
    }
    let r = i32(P.u[0].x);
    let vertical = P.u[0].y != 0u;
    let centre = textureLoad(src, p, 0);
    var lo = centre;
    var hi = centre;
    for (var k = -r; k <= r; k++) {
        var v: vec4<f32>;
        if (vertical) {
            v = tex_get_clamped(src, p.x, p.y + k);
        } else {
            v = tex_get_clamped(src, p.x + k, p.y);
        }
        lo = min(lo, v);
        hi = max(hi, v);
    }
    var o = centre;
    for (var c = 0; c < 4; c++) {
        if (P.u[1][c] == 0u) {
            o[c] = lo[c];
        } else if (P.u[1][c] == 1u) {
            o[c] = hi[c];
        }
    }
    textureStore(out, p, o);
}

// Crop: out(p) = src(p + u[0].xy).
@compute @workgroup_size(16, 16)
fn fxn_crop(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = fxn_pixel(gid);
    if (p.x < 0) {
        return;
    }
    textureStore(out, p, textureLoad(src, p + vec2<i32>(P.u[0].xy), 0));
}

// ---------------------------------------------------------------- Compound Blur (blur2::compound_blur)

// The blur map (straight luminance × alpha, inverted with u[0].y) in every channel. u[0].x = 1:
// `src` is the Blur Layer sampled at the per-column / per-row coordinates in `data` (u[1].x =
// width); 0: `src` is the layer itself.
@compute @workgroup_size(16, 16)
fn fxn_cb_map(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = fxn_pixel(gid);
    if (p.x < 0) {
        return;
    }
    var v: vec4<f32>;
    if (P.u[0].x == 1u) {
        let w = P.u[1].x;
        v = sample_bilinear(src, data[u32(p.x)], data[w + u32(p.y)]);
    } else {
        v = textureLoad(src, p, 0);
    }
    let s = fxn_unpremul(v);
    var l = luminance(s.xyz) * s.w;
    if (P.u[0].y != 0u) {
        l = 1.0 - l;
    }
    textureStore(out, p, vec4<f32>(l));
}

// Writes the pixels whose blur level falls between levels k (src) and k + 1 (aux). The map is
// `data` (RGBA f32 rows of u[0].y pixels); u[0].x = k.
@compute @workgroup_size(16, 16)
fn fxn_cb_level(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = fxn_pixel(gid);
    if (p.x < 0) {
        return;
    }
    let m = data[(u32(p.y) * P.u[0].y + u32(p.x)) * 4u];
    let t = clamp(m, 0.0, 1.0) * 4.0;
    let k = min(u32(floor(t)), 3u);
    if (k != P.u[0].x) {
        return;
    }
    let f = t - f32(k);
    let a = textureLoad(src, p, 0);
    let c = textureLoad(aux, p, 0);
    textureStore(out, p, a + (c - a) * f);
}

// ---------------------------------------------------------------- Echo (time_fx::echo)

// acc (src) ⊕ frame (aux) × f[0].x. u[0] = (operator, first frame).
@compute @workgroup_size(16, 16)
fn fxn_echo(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = fxn_pixel(gid);
    if (p.x < 0) {
        return;
    }
    let op = P.u[0].x;
    var acc = textureLoad(src, p, 0);
    if (P.u[0].y != 0u) {
        acc = select(vec4<f32>(0.0), vec4<f32>(FXN_MAX), op == 2u);
    }
    let k = P.f[0].x;
    let q = textureLoad(aux, p, 0) * k;
    switch op {
        case 1u: {
            acc = max(acc, q);
        }
        case 2u: {
            acc = min(acc, q);
        }
        case 3u: {
            acc = acc + q - acc * q;
        }
        case 4u: {
            acc += q * (1.0 - acc.w);
        }
        case 5u: {
            acc = q + acc * (1.0 - q.w);
        }
        default: {
            acc += q;
        }
    }
    textureStore(out, p, acc);
}

// Echo's finish. u[0].x = operator; f[0].x = number of frames.
@compute @workgroup_size(16, 16)
fn fxn_echo_fin(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = fxn_pixel(gid);
    if (p.x < 0) {
        return;
    }
    let op = P.u[0].x;
    var acc = textureLoad(src, p, 0);
    if (op == 6u) {
        acc = acc / P.f[0].x;
    }
    if (op == 2u && acc.x == FXN_MAX) {
        acc = vec4<f32>(0.0);
    }
    textureStore(out, p, vec4<f32>(max(acc.xyz, vec3<f32>(0.0)), clamp(acc.w, 0.0, 1.0)));
}

// ---------------------------------------------------------------- Fractal / Turbulent Noise (noise3.rs)

fn fxn_catmull(p0: f32, p1: f32, p2: f32, p3: f32, t: f32) -> f32 {
    return 0.5 * (2.0 * p1 + (p2 - p0) * t + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t * t + (3.0 * p1 - p0 - 3.0 * p2 + p3) * t * t * t);
}

fn fxn_slice(ix: i32, iy: i32, iz: i32, tx: f32, ty: f32, seed: u32, nt: u32) -> f32 {
    if (nt == 0u) {
        return lattice(ix, iy, iz, seed);
    }
    if (nt == 1u) {
        let a = lattice(ix, iy, iz, seed);
        let b = lattice(ix + 1, iy, iz, seed);
        let c = lattice(ix, iy + 1, iz, seed);
        let d = lattice(ix + 1, iy + 1, iz, seed);
        let top = a + (b - a) * tx;
        let bot = c + (d - c) * tx;
        return top + (bot - top) * ty;
    }
    var rows: array<f32, 4>;
    for (var j = 0; j < 4; j++) {
        let y = iy + j - 1;
        rows[j] = fxn_catmull(lattice(ix - 1, y, iz, seed), lattice(ix, y, iz, seed), lattice(ix + 1, y, iz, seed), lattice(ix + 2, y, iz, seed), tx);
    }
    return fxn_catmull(rows[0], rows[1], rows[2], rows[3], ty);
}

// noise3::typed_noise.
fn fxn_typed_noise(x: f32, y: f32, z: f32, seed: u32, nt: u32) -> f32 {
    if (nt == 2u) {
        return soft_linear_noise(x, y, z, seed);
    }
    let x0 = floor(x);
    let y0 = floor(y);
    let z0 = floor(z);
    let tx = x - x0;
    let ty = y - y0;
    let fz = fade(z - z0);
    let a = fxn_slice(i32(x0), i32(y0), i32(z0), tx, ty, seed, nt);
    let b = fxn_slice(i32(x0), i32(y0), i32(z0) + 1, tx, ty, seed, nt);
    return a + (b - a) * fz;
}

// noise3::shape.
fn fxn_shape(kind: u32, n: f32) -> f32 {
    let s = n * 2.0 - 1.0;
    switch kind {
        case 1u: {
            return 1.0 - s * s;
        }
        case 2u: {
            return 1.0 - abs(s);
        }
        case 3u: {
            let t = 1.0 - abs(s);
            return t * t;
        }
        case 8u: {
            let t = clamp(abs(s) / 0.12, 0.0, 1.0);
            return 1.0 - t * t;
        }
        default: {
            return n;
        }
    }
}

// Fractal::layer. `o` = octave data base index.
fn fxn_layer(u: f32, v: f32, o: u32, z: f32, seed: u32) -> f32 {
    let nt = P.u[0].y;
    if (P.u[1].y != 0u) {
        let t = P.f[2].x;
        let c = P.f[2].y;
        let w = P.f[2].z;
        let a = fxn_typed_noise(u, v, t + z, seed, nt);
        let b = fxn_typed_noise(u, v, t - c + z, seed, nt);
        return a * (1.0 - w) + b * w;
    }
    return fxn_typed_noise(u, v, data[o + 7u] + z, seed, nt);
}

// u[0] = (kind, noise type, octaves, seed); u[1] = (sub rotation, cycle, invert, overflow);
// u[2].x = blend mode (BlendMode::ALL index, 255 = None); f[0] = (sin, cos, size x, size y);
// f[1] = (contrast, brightness, opacity, blend with original); f[2] = (cycle t, cycle, t / cycle,
// norm); data = 10 floats per octave (noise3::FractalGpu).
@compute @workgroup_size(16, 16)
fn fxn_fractal(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = fxn_pixel(gid);
    if (p.x < 0) {
        return;
    }
    let kind = P.u[0].x;
    let n_oct = P.u[0].z;
    let sr = P.f[0].x;
    let cr = P.f[0].y;
    let x = f32(p.x) + 0.5;
    let y = f32(p.y) + 0.5;
    var sum = 0.0;
    var best = 0.0;
    for (var oi = 0u; oi < n_oct; oi++) {
        let o = oi * 10u;
        let dx = x - data[o];
        let dy = y - data[o + 1u];
        var u = (dx * cr + dy * sr) / P.f[0].z;
        var v = (-dx * sr + dy * cr) / P.f[0].w;
        if (P.u[1].x != 0u) {
            let s = data[o + 2u];
            let c = data[o + 3u];
            let u2 = u * c + v * s;
            v = -u * s + v * c;
            u = u2;
        }
        let f = data[o + 4u];
        u = u * f;
        v = v * f;
        let z = data[o + 8u];
        let seed = P.u[0].w + oi;
        if (kind >= 4u && kind <= 6u) {
            let k = data[o + 9u];
            let a = fxn_layer(u * 0.5, v * 0.5, o, z + 3.1, seed ^ 0x55u) - 0.5;
            let bb = fxn_layer(u * 0.5, v * 0.5, o, z + 5.7, seed ^ 0xaau) - 0.5;
            if (kind == 5u) {
                let s = sin(a * 3.0);
                let c = cos(a * 3.0);
                let u2 = u * c - v * s;
                v = u * s + v * c;
                u = u2;
            } else {
                u += a * 1.5 * k;
                v += bb * 1.5 * k;
            }
        }
        let n = fxn_shape(kind, fxn_layer(u, v, o, z, seed));
        let amp = data[o + 5u];
        if (kind == 7u) {
            best = max(best, n * amp);
        }
        sum += n * amp * data[o + 6u];
    }
    var raw = sum / max(P.f[2].w, 1e-6);
    if (kind == 7u) {
        raw = best;
    }
    var val = (raw - 0.5) * P.f[1].x + 0.5 + P.f[1].y;
    let ov = P.u[1].w;
    if (ov == 0u) {
        val = clamp(val, 0.0, 1.0);
    } else if (ov == 1u) {
        val = clamp(0.5 + 0.5 * tanh(2.0 * (val - 0.5)) / tanh(1.0), 0.0, 1.0);
    } else if (ov == 2u) {
        let t = val - 2.0 * floor(val / 2.0);
        val = select(t, 2.0 - t, t > 1.0);
    }
    if (P.u[1].z != 0u) {
        val = 1.0 - val;
    }
    let op = P.f[1].z;
    let s = vec4<f32>(vec3<f32>(val * op), op);
    let orig = textureLoad(src, p, 0);
    var r = s;
    if (P.u[2].x != 255u) {
        r = blend_pixel(P.u[2].x, orig, s, 0.5);
    }
    let bo = P.f[1].w;
    textureStore(out, p, orig * bo + r * orig.a * (1.0 - bo));
}

// ---------------------------------------------------------------- Noise Alpha / Noise HLS (noise.rs)

// noise::phased_cycle with the phase's two seeds and smoothed fraction resolved on the CPU.
fn fxn_phased(x: u32, y: u32, sa: u32, sb: u32, t: f32) -> f32 {
    let a = hash_noise(x, y, sa);
    let b = hash_noise(x, y, sb);
    return a + (b - a) * t;
}

// u[0] = (squared, original alpha mode, overflow, seed a); u[1].x = seed b; f[0] = (amount, t).
@compute @workgroup_size(16, 16)
fn fxn_noise_alpha(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = fxn_pixel(gid);
    if (p.x < 0) {
        return;
    }
    let k = P.f[0].x;
    var n = fxn_phased(u32(p.x), u32(p.y), P.u[0].w, P.u[1].x, P.f[0].y);
    if (P.u[0].x != 0u) {
        n = n * n;
    }
    let s = fxn_unpremul(textureLoad(src, p, 0));
    let a = s.w;
    var na: f32;
    switch P.u[0].y {
        case 0u: {
            na = a + n * k;
        }
        case 1u: {
            na = select(0.0, a + n * k, a > 0.0);
        }
        case 2u: {
            na = a * (1.0 + (2.0 * n - 1.0) * k);
        }
        default: {
            na = a + (2.0 * n - 1.0) * k * 4.0 * a * (1.0 - a);
        }
    }
    if (P.u[0].z == 1u) {
        let t = na - 2.0 * floor(na / 2.0);
        na = select(t, 2.0 - t, t > 1.0);
    } else if (P.u[0].z == 2u && !(na >= 0.0 && na <= 1.0)) {
        na = na - floor(na);
    }
    na = clamp(na, 0.0, 1.0);
    textureStore(out, p, vec4<f32>(s.xyz * na, na));
}

// Noise HLS: u[0] = (noise kind, seed); u[1], u[2].xy = per channel (hue, lightness,
// saturation) the phase's two seeds; f[0] = (hue, lightness, saturation, grain size); f[1].x =
// phase (Grain), f[1].y = smoothed fraction.
fn fxn_hls_seed(i: u32) -> u32 {
    if (i < 4u) {
        return P.u[1][i];
    }
    return P.u[2][i - 4u];
}

fn fxn_hls_n(x: u32, y: u32, k: u32) -> f32 {
    let s = P.u[0].y + k * 7919u;
    switch P.u[0].x {
        case 2u: {
            let gs = P.f[0].w;
            return clamp((value_noise(f32(x) / gs, f32(y) / gs, P.f[1].x, s) - 0.5) * 3.0, -1.0, 1.0);
        }
        case 1u: {
            let v = fxn_phased(x, y, fxn_hls_seed(2u * k), fxn_hls_seed(2u * k + 1u), P.f[1].y) * 2.0 - 1.0;
            return v * abs(v);
        }
        default: {
            return fxn_phased(x, y, fxn_hls_seed(2u * k), fxn_hls_seed(2u * k + 1u), P.f[1].y) * 2.0 - 1.0;
        }
    }
}

@compute @workgroup_size(16, 16)
fn fxn_noise_hls(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = fxn_pixel(gid);
    if (p.x < 0) {
        return;
    }
    let px = textureLoad(src, p, 0);
    let s0 = fxn_unpremul(px);
    let a = s0.w;
    if (a <= 0.0) {
        textureStore(out, p, px);
        return;
    }
    let x = u32(p.x);
    let y = u32(p.y);
    var hsl = rgb_to_hsl(s0.xyz);
    if (P.f[0].x != 0.0) {
        hsl.x = fract_euclid(hsl.x + fxn_hls_n(x, y, 0u) * P.f[0].x * 0.5);
    }
    if (P.f[0].y != 0.0) {
        hsl.z = clamp(hsl.z + fxn_hls_n(x, y, 1u) * P.f[0].y, 0.0, 1.0);
    }
    if (P.f[0].z != 0.0) {
        hsl.y = clamp(hsl.y + fxn_hls_n(x, y, 2u) * P.f[0].z, 0.0, 1.0);
    }
    let c = hsl_to_rgb(hsl.x, hsl.y, hsl.z);
    textureStore(out, p, vec4<f32>(c * a, a));
}

// ---------------------------------------------------------------- Remove Grain (noise::remove_grain)

// The guide (util::guided_filter's g): the luminance of the premultiplied channels in every
// channel (Multichannel), or the channels themselves (u[0].x, Single Channel).
@compute @workgroup_size(16, 16)
fn fxn_gf_guide(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = fxn_pixel(gid);
    if (p.x < 0) {
        return;
    }
    let v = textureLoad(src, p, 0);
    if (P.u[0].x != 0u) {
        textureStore(out, p, v);
    } else {
        textureStore(out, p, vec4<f32>(luminance(v.xyz)));
    }
}

// Guided filter arithmetic, S = src, A = aux, D = `data` (RGBA f32 rows of u[0].y pixels):
// u[0].x = 0: S − A·D, 1: S − A·A, 2: S / (A + f[0].x), 3: A − S·D, 4: S·D + A, 5: S·A, 6: S·S.
@compute @workgroup_size(16, 16)
fn fxn_gf(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = fxn_pixel(gid);
    if (p.x < 0) {
        return;
    }
    let s = textureLoad(src, p, 0);
    let a = textureLoad(aux, p, 0);
    var d = vec4<f32>(0.0);
    let mode = P.u[0].x;
    if (mode == 0u || mode == 3u || mode == 4u) {
        let i = (u32(p.y) * P.u[0].y + u32(p.x)) * 4u;
        d = vec4<f32>(data[i], data[i + 1u], data[i + 2u], data[i + 3u]);
    }
    var o: vec4<f32>;
    switch mode {
        case 0u: {
            o = s - a * d;
        }
        case 1u: {
            o = s - a * a;
        }
        case 2u: {
            o = s / (a + P.f[0].x);
        }
        case 3u: {
            o = a - s * d;
        }
        case 4u: {
            o = s * d + a;
        }
        case 5u: {
            o = s * a;
        }
        default: {
            o = s * s;
        }
    }
    textureStore(out, p, o);
}

// Texture: f + (o − f) × f[0].x on colour (src = filtered, aux = original).
@compute @workgroup_size(16, 16)
fn fxn_rg_texture(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = fxn_pixel(gid);
    if (p.x < 0) {
        return;
    }
    let f = textureLoad(src, p, 0);
    let o = textureLoad(aux, p, 0);
    textureStore(out, p, vec4<f32>(f.xyz + (o.xyz - f.xyz) * P.f[0].x, f.w));
}

// Unsharp Mask on the planes: src = channels, aux = their blur; f[0] = (amount, threshold).
@compute @workgroup_size(16, 16)
fn fxn_rg_unsharp(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = fxn_pixel(gid);
    if (p.x < 0) {
        return;
    }
    var v = textureLoad(src, p, 0);
    let l = textureLoad(aux, p, 0);
    for (var c = 0; c < 3; c++) {
        if (abs(v[c] - l[c]) > P.f[0].y) {
            v[c] = v[c] + (v[c] - l[c]) * P.f[0].x;
        }
    }
    textureStore(out, p, v);
}

// Remove Grain's result: colour clamped to 0..alpha (alpha from aux = the original); with
// u[0].x the Preview viewing mode (noise::preview_compose): the original (aux) outside the
// region f[0] = (x0, x1, y0, y1), the box outline in f[1] where u[0].y.
@compute @workgroup_size(16, 16)
fn fxn_rg_out(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = fxn_pixel(gid);
    if (p.x < 0) {
        return;
    }
    let o = textureLoad(aux, p, 0);
    let a = o.w;
    var v = vec4<f32>(clamp(textureLoad(src, p, 0).xyz, vec3<f32>(0.0), vec3<f32>(max(a, 0.0))), a);
    if (P.u[0].x != 0u) {
        let x = f32(p.x) + 0.5;
        let y = f32(p.y) + 0.5;
        let r = P.f[0];
        let inside = x >= r.x && x < r.y && y >= r.z && y < r.w;
        if (!inside) {
            v = o;
        }
        if (P.u[0].y != 0u) {
            let nx = abs(x - r.x) < 0.75 || abs(x - r.y) < 0.75;
            let ny = abs(y - r.z) < 0.75 || abs(y - r.w) < 0.75;
            if ((nx && y >= r.z - 0.75 && y < r.w + 0.75) || (ny && x >= r.x - 0.75 && x < r.y + 0.75)) {
                v = vec4<f32>(P.f[1].xyz, 1.0);
            }
        }
    }
    textureStore(out, p, v);
}

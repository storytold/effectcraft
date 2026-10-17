// Compositing and effect kernels (rgba32float output). Parameter layouts are documented next to
// each entry point and mirrored by the Rust dispatch code (src/ops.rs, src/effects.rs).

@group(0) @binding(3) var out: texture_storage_2d<rgba32float, write>;

fn out_dims() -> vec2<i32> {
    return vec2<i32>(textureDimensions(out));
}

// ---------------------------------------------------------------- compositing

// Warp `src` by the inverse matrix (output pixel → source pixel) and blend over `aux`
// (raster::composite_warp / accumulate_warp).
// Double-f32 (hi + lo) arithmetic: the inverse transform in about f64 precision, so a pixel maps
// to the same source position (and bilinear weights) as raster::warp's f64 math.
// GPU compilers may simplify (a + b) - a to b, losing the rounding error two_sum recovers; the
// sum is scaled by f[7].x (always 1.0, set by the host) so the compiler can't see through it.
fn two_sum(a: f32, b: f32) -> vec2<f32> {
    let s = (a + b) * P.f[7].x;
    let bb = s - a;
    return vec2<f32>(s, (a - (s - bb)) + (b - bb));
}

// (hi, lo) * x + (hi, lo) with x exact in f32.
fn df_mul_add(c: vec2<f32>, x: f32, acc: vec2<f32>) -> vec2<f32> {
    let p = (c.x * x) * P.f[7].x;
    let pe = fma(c.x, x, -p) + c.y * x;
    let s = two_sum(acc.x, p);
    return two_sum(s.x, s.y + acc.y + pe);
}

// Row `k` of the inverse at the pixel centre (fx, fy): hi from f[k], lo from f[5 + k].
fn df_map(k: u32, fx: f32, fy: f32) -> vec2<f32> {
    let hi = P.f[k];
    let lo = P.f[5u + k];
    var acc = two_sum(hi.z, lo.z);
    acc = df_mul_add(vec2<f32>(hi.x, lo.x), fx, acc);
    acc = df_mul_add(vec2<f32>(hi.y, lo.y), fy, acc);
    return acc;
}

// Image::sample_bilinear / nearest at a (hi, lo) position: the integer pixel and the fraction
// (rounded to f32 once) as the CPU takes them from its f64 coordinate.
fn sample_df(t: texture_2d<f32>, sampling: u32, px: vec2<f32>, py: vec2<f32>) -> vec4<f32> {
    let fx = two_sum(px.x - 0.5, px.y);
    let fy = two_sum(py.x - 0.5, py.y);
    var x0 = floor(fx.x);
    if (fx.x == x0 && fx.y < 0.0) {
        x0 -= 1.0;
    }
    var y0 = floor(fy.x);
    if (fy.x == y0 && fy.y < 0.0) {
        y0 -= 1.0;
    }
    let xi = i32(x0);
    let yi = i32(y0);
    let tx = (fx.x - x0) + fx.y;
    let ty = (fy.x - y0) + fy.y;
    if (sampling == 0u) {
        // floor(p) rather than floor(p - 0.5): p - 0.5 + 0.5.
        return tex_get(t, i32(floor(px.x + px.y)), i32(floor(py.x + py.y)));
    }
    let a = tex_get(t, xi, yi);
    let b = tex_get(t, xi + 1, yi);
    let c = tex_get(t, xi, yi + 1);
    let d = tex_get(t, xi + 1, yi + 1);
    let top = a + (b - a) * tx;
    let bot = c + (d - c) * tx;
    return top + (bot - top) * ty;
}

// u[0] = (mode, sampling, seed, flags: 1 accumulate, 2 clear outside bounds)
// f[0..3] = inverse rows; f[3] = (opacity, weight); f[4] = bounds (x0, y0, x1, y1);
// f[5], f[6] = what f[0], f[1] rounded off from the f64 inverse
@compute @workgroup_size(16, 16)
fn warp_blend(@builtin(global_invocation_id) gid: vec3<u32>) {
    let dims = out_dims();
    let x = i32(gid.x);
    let y = i32(gid.y);
    if (x >= dims.x || y >= dims.y) {
        return;
    }
    let mode = P.u[0].x;
    let sampling = P.u[0].y;
    let seed = P.u[0].z;
    let flags = P.u[0].w;
    let b = P.f[4];
    var d = textureLoad(aux, vec2<i32>(x, y), 0);
    let fx = f32(x) + 0.5;
    let fy = f32(y) + 0.5;
    if (fx < b.x || fy < b.y || fx > b.z || fy > b.w) {
        if ((flags & 2u) != 0u) {
            d = vec4<f32>(0.0);
        }
        store_q(vec2<i32>(x, y), d, P.f[3].z, P.f[3].w);
        return;
    }
    let r0 = P.f[0];
    let r1 = P.f[1];
    let r2 = P.f[2];
    var px = r0.x * fx + r0.y * fy + r0.z;
    var py = r1.x * fx + r1.y * fy + r1.z;
    let w = r2.x * fx + r2.y * fy + r2.z;
    if (w != 1.0 && w != 0.0) {
        px = px / w;
        py = py / w;
    }
    let sz = vec2<f32>(textureDimensions(src));
    let stencil = is_stencil(mode);
    if (px < -1.0 || py < -1.0 || px > sz.x + 1.0 || py > sz.y + 1.0) {
        if (stencil && (flags & 1u) == 0u) {
            d = blend_pixel(mode, d, vec4<f32>(0.0), 0.5);
        }
        store_q(vec2<i32>(x, y), d, P.f[3].z, P.f[3].w);
        return;
    }
    var s: vec4<f32>;
    let affine = r2.x == 0.0 && r2.y == 0.0 && r2.z == 1.0;
    if (affine && sampling != 2u) {
        s = sample_df(src, sampling, df_map(0u, fx, fy), df_map(1u, fx, fy));
    } else {
        s = sample(src, sampling, px, py);
    }
    if ((flags & 1u) != 0u) {
        if (s.w > 0.0) {
            d = d + s * P.f[3].y;
        }
        store_q(vec2<i32>(x, y), d, P.f[3].z, P.f[3].w);
        return;
    }
    if (s.w <= 0.0 && !stencil) {
        store_q(vec2<i32>(x, y), d, P.f[3].z, P.f[3].w);
        return;
    }
    let op = P.f[3].x;
    if (op < 1.0) {
        s = s * op;
    }
    var n = 0.5;
    if (mode == 1u || mode == 2u) {
        n = hash_noise(u32(x), u32(y), seed);
    }
    store_q(vec2<i32>(x, y), blend_pixel(mode, d, s, n), P.f[3].z, P.f[3].w);
}

// Blend `src` (same size) onto `aux` (Image::blend_from).
// u[0] = (mode, _, seed); f[0].x = opacity
@compute @workgroup_size(16, 16)
fn blend_full(@builtin(global_invocation_id) gid: vec3<u32>) {
    let dims = out_dims();
    let p = vec2<i32>(gid.xy);
    if (p.x >= dims.x || p.y >= dims.y) {
        return;
    }
    let mode = P.u[0].x;
    let d = textureLoad(aux, p, 0);
    var s = textureLoad(src, p, 0);
    if (s.w <= 0.0 && !is_stencil(mode)) {
        store_q(p, d, P.f[0].y, P.f[0].z);
        return;
    }
    s = s * P.f[0].x;
    var n = 0.5;
    if (mode == 1u || mode == 2u) {
        n = hash_noise(gid.x, gid.y, P.u[0].z);
    }
    store_q(p, blend_pixel(mode, d, s, n), P.f[0].y, P.f[0].z);
}

// Track matte: `src` × matte factor of `aux`. u[0].x = 0 alpha, 1 alpha inverted, 2 luma,
// 3 luma inverted.
@compute @workgroup_size(16, 16)
fn matte(@builtin(global_invocation_id) gid: vec3<u32>) {
    let dims = out_dims();
    let p = vec2<i32>(gid.xy);
    if (p.x >= dims.x || p.y >= dims.y) {
        return;
    }
    let q = textureLoad(aux, p, 0);
    var k: f32;
    switch P.u[0].x {
        case 0u: { k = q.w; }
        case 1u: { k = 1.0 - q.w; }
        case 2u: { k = luminance(q.xyz); }
        default: { k = 1.0 - luminance(q.xyz); }
    }
    k = clamp(k, 0.0, 1.0);
    textureStore(out, p, textureLoad(src, p, 0) * k);
}

// Preserve Transparency: `src` × alpha of `aux`.
@compute @workgroup_size(16, 16)
fn preserve(@builtin(global_invocation_id) gid: vec3<u32>) {
    let dims = out_dims();
    let p = vec2<i32>(gid.xy);
    if (p.x >= dims.x || p.y >= dims.y) {
        return;
    }
    textureStore(out, p, textureLoad(src, p, 0) * textureLoad(aux, p, 0).w);
}

// Knockout: `src` × (1 − alpha of `aux`).
@compute @workgroup_size(16, 16)
fn knockout(@builtin(global_invocation_id) gid: vec3<u32>) {
    let dims = out_dims();
    let p = vec2<i32>(gid.xy);
    if (p.x >= dims.x || p.y >= dims.y) {
        return;
    }
    let keep = 1.0 - clamp(textureLoad(aux, p, 0).w, 0.0, 1.0);
    textureStore(out, p, textureLoad(src, p, 0) * keep);
}

// Styled layer finish: canvas `src` moves toward `aux` by the opacity, per enabled channel.
// u[0].xyz = R/G/B switches; f[0].x = opacity.
@compute @workgroup_size(16, 16)
fn channel_mix(@builtin(global_invocation_id) gid: vec3<u32>) {
    let dims = out_dims();
    let p = vec2<i32>(gid.xy);
    if (p.x >= dims.x || p.y >= dims.y) {
        return;
    }
    let c = textureLoad(src, p, 0);
    let t = textureLoad(aux, p, 0);
    let op = P.f[0].x;
    var v = t;
    if (P.u[0].x == 0u) {
        v.x = c.x;
    }
    if (P.u[0].y == 0u) {
        v.y = c.y;
    }
    if (P.u[0].z == 0u) {
        v.z = c.z;
    }
    textureStore(out, p, c + (v - c) * op);
}

// 8/16 bpc: clamp premultiplied pixels to 0..1 (colour ≤ alpha) and round. f[0].x = levels.
@compute @workgroup_size(16, 16)
fn quantize(@builtin(global_invocation_id) gid: vec3<u32>) {
    let dims = out_dims();
    let p = vec2<i32>(gid.xy);
    if (p.x >= dims.x || p.y >= dims.y) {
        return;
    }
    textureStore(out, p, quantize_px(textureLoad(src, p, 0), P.f[0].x, P.f[0].y));
}

fn quantize_px(s: vec4<f32>, levels: f32, inv: f32) -> vec4<f32> {
    // Use the CPU-rounded reciprocal to avoid backend-dependent division.
    let a = floor(clamp(s.w, 0.0, 1.0) * levels + 0.5) * inv;
    var o = vec4<f32>(0.0, 0.0, 0.0, a);
    o.x = floor(clamp(s.x, 0.0, a) * levels + 0.5) * inv;
    o.y = floor(clamp(s.y, 0.0, a) * levels + 0.5) * inv;
    o.z = floor(clamp(s.z, 0.0, a) * levels + 0.5) * inv;
    return o;
}

// Store a compositing result, clamped and quantised to `levels` when > 0 (8/16 bpc: the comp
// after a layer, fused into the layer's composite).
fn store_q(p: vec2<i32>, v: vec4<f32>, levels: f32, inv: f32) {
    if (levels > 0.0) {
        textureStore(out, p, quantize_px(v, levels, inv));
    } else {
        textureStore(out, p, v);
    }
}

// Colour conversion on straight colour (render::color::convert).
// u[0] = (decode curve, has matrix, encode curve); f[0..3] = matrix rows.
@compute @workgroup_size(16, 16)
fn convert(@builtin(global_invocation_id) gid: vec3<u32>) {
    let dims = out_dims();
    let p = vec2<i32>(gid.xy);
    if (p.x >= dims.x || p.y >= dims.y) {
        return;
    }
    let s = textureLoad(src, p, 0);
    let a = s.w;
    if (a <= 0.0) {
        textureStore(out, p, s);
        return;
    }
    var c = s.xyz / a;
    let dc = P.u[0].x;
    if (dc != 0u) {
        c = vec3<f32>(curve_decode(dc, c.x), curve_decode(dc, c.y), curve_decode(dc, c.z));
    }
    if (P.u[0].y != 0u) {
        let m0 = P.f[0].xyz;
        let m1 = P.f[1].xyz;
        let m2 = P.f[2].xyz;
        c = vec3<f32>(m0.x * c.x + m0.y * c.y + m0.z * c.z, m1.x * c.x + m1.y * c.y + m1.z * c.z, m2.x * c.x + m2.y * c.y + m2.z * c.z);
    }
    let ec = P.u[0].z;
    if (ec != 0u) {
        c = vec3<f32>(curve_encode(ec, c.x), curve_encode(ec, c.y), curve_encode(ec, c.z));
    }
    textureStore(out, p, vec4<f32>(c * a, a));
}

// Box-filter halving for minification (raster::warp::half).
@compute @workgroup_size(16, 16)
fn half(@builtin(global_invocation_id) gid: vec3<u32>) {
    let dims = out_dims();
    let p = vec2<i32>(gid.xy);
    if (p.x >= dims.x || p.y >= dims.y) {
        return;
    }
    let sx = 2 * p.x;
    let sy = 2 * p.y;
    var acc = vec4<f32>(0.0);
    acc += tex_get_clamped(src, sx, sy);
    acc += tex_get_clamped(src, sx + 1, sy);
    acc += tex_get_clamped(src, sx, sy + 1);
    acc += tex_get_clamped(src, sx + 1, sy + 1);
    textureStore(out, p, acc * 0.25);
}

// ---------------------------------------------------------------- blurs (raster::blur)

fn line_get(horizontal: bool, line: i32, i: i32, n: i32, repeat: bool) -> vec4<f32> {
    var j = i;
    if (i < 0 || i >= n) {
        if (!repeat) {
            return vec4<f32>(0.0);
        }
        j = clamp(i, 0, n - 1);
    }
    if (horizontal) {
        return textureLoad(src, vec2<i32>(j, line), 0);
    }
    return textureLoad(src, vec2<i32>(line, j), 0);
}

// One box pass of radius r along rows (horizontal) or columns, as running sums over blocks of
// `block` pixels (each block restarts its sum, as the CPU's vertical pass does; rows are one block).
fn box_pass(gid: vec3<u32>, horizontal: bool) {
    let dims = out_dims();
    let r = i32(P.u[0].x);
    let repeat = P.u[0].y != 0u;
    let block = i32(P.u[0].z);
    var lines = dims.x;
    var n = dims.y;
    if (horizontal) {
        lines = dims.y;
        n = dims.x;
    }
    let line = i32(gid.x);
    let b0 = i32(gid.y) * block;
    if (line >= lines || b0 >= n) {
        return;
    }
    let norm = 1.0 / f32(2 * r + 1);
    var acc = vec4<f32>(0.0);
    for (var i = b0 - r; i <= b0 + r; i++) {
        acc += line_get(horizontal, line, i, n, repeat);
    }
    let b1 = min(b0 + block, n);
    for (var i = b0; i < b1; i++) {
        if (horizontal) {
            textureStore(out, vec2<i32>(i, line), acc * norm);
        } else {
            textureStore(out, vec2<i32>(line, i), acc * norm);
        }
        let add = line_get(horizontal, line, i + r + 1, n, repeat);
        let sub = line_get(horizontal, line, i - r, n, repeat);
        acc += add - sub;
    }
}

// u[0] = (radius, repeat edges, block); dispatched over (line, block).
@compute @workgroup_size(64, 1)
fn box_h(@builtin(global_invocation_id) gid: vec3<u32>) {
    box_pass(gid, true);
}

@compute @workgroup_size(64, 1)
fn box_v(@builtin(global_invocation_id) gid: vec3<u32>) {
    box_pass(gid, false);
}

// Directional blur (raster::directional_blur). f[0] = (dx, dy, length, samples).
@compute @workgroup_size(16, 16)
fn directional(@builtin(global_invocation_id) gid: vec3<u32>) {
    let dims = out_dims();
    let p = vec2<i32>(gid.xy);
    if (p.x >= dims.x || p.y >= dims.y) {
        return;
    }
    let dx = P.f[0].x;
    let dy = P.f[0].y;
    let len = P.f[0].z;
    let n = i32(P.f[0].w);
    var acc = vec4<f32>(0.0);
    for (var i = 0; i < n; i++) {
        let t = (f32(i) / f32(n - 1) - 0.5) * len;
        acc += sample_bilinear(src, f32(p.x) + 0.5 + dx * t, f32(p.y) + 0.5 + dy * t);
    }
    textureStore(out, p, acc / f32(n));
}

// ---------------------------------------------------------------- Glow / Drop Shadow

// Glow's A & B gradient position (effects::misc::glow_ab_t). f[0].yzw = loops, phase, midpoint;
// u[0].z = colour looping.
fn glow_ab_t(l: f32) -> f32 {
    let u = clamp(l, 0.0, 1.0) * max(P.f[0].y, 0.0) + P.f[0].z;
    var t = u - floor(u);
    if (u > 0.0 && t <= 0.0) {
        t = 1.0;
    }
    let tri = 1.0 - abs(2.0 * t - 1.0);
    let lp = P.u[0].z;
    if (lp == 1u) {
        t = 1.0 - t;
    } else if (lp == 3u) {
        t = 1.0 - tri;
    } else if (lp != 0u) {
        t = tri;
    }
    let m = clamp(P.f[0].w, 0.01, 0.99);
    return pow(max(t, 0.0), log(0.5) / log(m));
}

// Glow bright pass. u[0].x = A & B colours; u[0].y = based on alpha; f[0].x = threshold;
// f[1] = colour A; f[2] = colour B.
@compute @workgroup_size(16, 16)
fn glow_bright(@builtin(global_invocation_id) gid: vec3<u32>) {
    let dims = out_dims();
    let p = vec2<i32>(gid.xy);
    if (p.x >= dims.x || p.y >= dims.y) {
        return;
    }
    let px = textureLoad(src, p, 0);
    let a = px.w;
    if (a <= 0.0) {
        textureStore(out, p, vec4<f32>(0.0));
        return;
    }
    let thr = P.f[0].x;
    var l = luminance(px.xyz / a);
    if (P.u[0].y != 0u) {
        l = a;
    }
    let k = clamp((l - thr) / max(1.0 - thr, 1e-3), 0.0, 1.0);
    if (P.u[0].x != 0u) {
        let t = glow_ab_t(l);
        let ca = P.f[1].xyz;
        let cb = P.f[2].xyz;
        let c = ca + (cb - ca) * t;
        textureStore(out, p, vec4<f32>(c * k * a, k * a));
    } else {
        textureStore(out, p, px * k);
    }
}

// Glow composite: original `src` with the blurred glow `aux`. u[0].x = 0 on top, 1 behind,
// 2 none (glow only); f[0].x = intensity.
@compute @workgroup_size(16, 16)
fn glow_combine(@builtin(global_invocation_id) gid: vec3<u32>) {
    let dims = out_dims();
    let p = vec2<i32>(gid.xy);
    if (p.x >= dims.x || p.y >= dims.y) {
        return;
    }
    var o = textureLoad(src, p, 0);
    let g = textureLoad(aux, p, 0) * P.f[0].x;
    if (P.u[0].x == 2u) {
        o = g;
    } else if (P.u[0].x == 1u) {
        let k = 1.0 - o.w;
        o = o + g * k;
    } else {
        o = vec4<f32>(o.xyz + g.xyz, min(o.w + g.w * (1.0 - o.w), 1.0));
    }
    o.w = clamp(o.w, 0.0, 1.0);
    textureStore(out, p, o);
}

// Drop shadow silhouette. f[0] = shadow colour; f[1] = (opacity, dx, dy).
@compute @workgroup_size(16, 16)
fn shadow_make(@builtin(global_invocation_id) gid: vec3<u32>) {
    let dims = out_dims();
    let p = vec2<i32>(gid.xy);
    if (p.x >= dims.x || p.y >= dims.y) {
        return;
    }
    let c = P.f[0];
    let a = sample_bilinear(src, f32(p.x) + 0.5 - P.f[1].y, f32(p.y) + 0.5 - P.f[1].z).w * P.f[1].x * c.w;
    textureStore(out, p, vec4<f32>(c.xyz * a, a));
}

// The layer over its shadow: `aux` (original) over `src` (shadow).
@compute @workgroup_size(16, 16)
fn shadow_combine(@builtin(global_invocation_id) gid: vec3<u32>) {
    let dims = out_dims();
    let p = vec2<i32>(gid.xy);
    if (p.x >= dims.x || p.y >= dims.y) {
        return;
    }
    let s = textureLoad(src, p, 0);
    let o = textureLoad(aux, p, 0);
    textureStore(out, p, o + s * (1.0 - o.w));
}

// ---------------------------------------------------------------- per-pixel effects

fn mix3(a: vec3<f32>, b: vec3<f32>, t: f32) -> vec3<f32> {
    return a + (b - a) * t;
}

// Curves: data = [n, xs.., ys.., ms.., lut (1025)] per channel at offsets f[c].
fn curve_exact(base: u32, x: f32) -> f32 {
    let n = u32(data[base]);
    let xs = base + 1u;
    let ys = xs + n;
    let ms = ys + n;
    if (x <= data[xs]) {
        return data[ys] + (x - data[xs]) * data[ms];
    }
    if (x >= data[xs + n - 1u]) {
        return data[ys + n - 1u] + (x - data[xs + n - 1u]) * data[ms + n - 1u];
    }
    // partition_point(|v| v <= x) - 1, clamped to n - 2.
    var k = 0u;
    for (var i = 0u; i < n; i++) {
        if (data[xs + i] <= x) {
            k = i + 1u;
        }
    }
    k = min(max(k, 1u) - 1u, n - 2u);
    let h = data[xs + k + 1u] - data[xs + k];
    let t = (x - data[xs + k]) / h;
    let t2 = t * t;
    let t3 = t * t * t;
    let h00 = 2.0 * t3 - 3.0 * t2 + 1.0;
    let h10 = t3 - 2.0 * t2 + t;
    let h01 = -2.0 * t3 + 3.0 * t2;
    let h11 = t3 - t2;
    return h00 * data[ys + k] + h10 * h * data[ms + k] + h01 * data[ys + k + 1u] + h11 * h * data[ms + k + 1u];
}

fn curve_eval(base_f: f32, x: f32) -> f32 {
    if (base_f < 0.0) {
        return x;
    }
    let base = u32(base_f);
    if (!(x >= 0.0 && x <= 1.0)) {
        return curve_exact(base, x);
    }
    let n = u32(data[base]);
    let lut = base + 1u + 3u * n;
    let f = x * 1024.0;
    let i = min(u32(f), 1023u);
    let t = f - f32(i);
    return data[lut + i] + (data[lut + i + 1u] - data[lut + i]) * t;
}

// Fractal Noise lattice (effects::generate::lattice / value_noise).
fn lattice(ix: i32, iy: i32, iz: i32, seed: u32) -> f32 {
    var h = (bitcast<u32>(ix) * 0x27d4eb2du) ^ (bitcast<u32>(iy) * 0x165667b1u) ^ (bitcast<u32>(iz) * 0x9e3779b9u) ^ (seed * 0x85ebca6bu);
    h = h ^ (h >> 15u);
    h = h * 0x2c1b3c6du;
    h = h ^ (h >> 12u);
    h = h * 0x297a2d39u;
    h = h ^ (h >> 15u);
    return f32(h & 0xffffu) / 65535.0;
}

fn fade(t: f32) -> f32 {
    return t * t * t * (t * (t * 6.0 - 15.0) + 10.0);
}

fn value_noise(x: f32, y: f32, z: f32, seed: u32) -> f32 {
    let x0 = floor(x);
    let y0 = floor(y);
    let z0 = floor(z);
    let fx = fade(x - x0);
    let fy = fade(y - y0);
    let fz = fade(z - z0);
    let ix = i32(x0);
    let iy = i32(y0);
    let iz = i32(z0);
    let c000 = lattice(ix, iy, iz, seed);
    let c100 = lattice(ix + 1, iy, iz, seed);
    let c010 = lattice(ix, iy + 1, iz, seed);
    let c110 = lattice(ix + 1, iy + 1, iz, seed);
    let c001 = lattice(ix, iy, iz + 1, seed);
    let c101 = lattice(ix + 1, iy, iz + 1, seed);
    let c011 = lattice(ix, iy + 1, iz + 1, seed);
    let c111 = lattice(ix + 1, iy + 1, iz + 1, seed);
    let l00 = c000 + (c100 - c000) * fx;
    let l01 = c010 + (c110 - c010) * fx;
    let a = l00 + (l01 - l00) * fy;
    let l10 = c001 + (c101 - c001) * fx;
    let l11 = c011 + (c111 - c011) * fx;
    let b = l10 + (l11 - l10) * fy;
    return a + (b - a) * fz;
}

// Fractal Noise's Soft Linear (effects::noise3::typed_noise): the quadratic B-spline over the
// 3 × 3 lattice values around the nearest one, faded along evolution, and SOFT_LINEAR_GAIN.
const SOFT_LINEAR_GAIN: f32 = (181.0 / 231.0) / 0.55;

fn bspline(t: f32) -> vec3<f32> {
    let s = 1.0 - t;
    return vec3<f32>(0.5 * s * s, 0.5 + t * s, 0.5 * t * t);
}

fn soft_linear_slice(ix: i32, iy: i32, iz: i32, wx: vec3<f32>, wy: vec3<f32>, seed: u32) -> f32 {
    var sum = 0.0;
    for (var j = 0; j < 3; j++) {
        let y = iy + j - 1;
        sum += wy[j] * (wx.x * lattice(ix - 1, y, iz, seed) + wx.y * lattice(ix, y, iz, seed) + wx.z * lattice(ix + 1, y, iz, seed));
    }
    return sum;
}

fn soft_linear_noise(x: f32, y: f32, z: f32, seed: u32) -> f32 {
    let x0 = floor(x + 0.5);
    let y0 = floor(y + 0.5);
    let z0 = floor(z);
    let wx = bspline(x + 0.5 - x0);
    let wy = bspline(y + 0.5 - y0);
    let a = soft_linear_slice(i32(x0), i32(y0), i32(z0), wx, wy, seed);
    let b = soft_linear_slice(i32(x0), i32(y0), i32(z0) + 1, wx, wy, seed);
    let n = a + (b - a) * fade(z - z0);
    return 0.5 + (n - 0.5) * SOFT_LINEAR_GAIN;
}

// generate::put: blend a generated straight colour (alpha ga) over the pixel.
fn put(px: vec4<f32>, g: vec3<f32>, ga: f32, blend_orig: f32) -> vec4<f32> {
    let g2 = vec4<f32>(g * ga, ga);
    let k = 1.0 - blend_orig;
    return px * blend_orig + g2 * k;
}

// Straight-colour map helper: skips transparent pixels (Image::map_straight).
fn straight(px: vec4<f32>) -> vec3<f32> {
    return px.xyz / px.w;
}

// u[0].x = op:
//  1 Tint            f[0] = black, f[1] = white, f[2].x = amount
//  2 Bright/Contrast f[0] = (brightness, k)
//  3 Hue/Saturation  u[0].y = colorize; f[0] = (hue, sat, light); f[1] = (ch, cs, cl)
//  4 Levels          f[0] = (ib, iw, g, ob); f[1] = (ow); u[0].y = clip
//  5 Exposure        f[0] = (e, offset, gamma)
//  6 Invert          u[0].y = channel; f[0].x = blend
//  7 Fill            f[0] = colour; f[1] = (opacity); u[0].y = invert
//  8 Curves          f[0] = data offsets (rgb, red, green, blue), f[1].x = alpha (−1 = none)
//  9 Gradient Ramp   f[0] = (sx, sy, ex, ey); f[1] = start colour; f[2] = end colour;
//                    f[3] = (len2, scatter, blend); u[0] = (_, radial, seed)
// 10 Fractal Noise   f[0] = (contrast, brightness, scale x, frac); f[1] = (ox, oy, sr, cr);
//                    f[2] = (evo, blend, opacity, scale y); f[3] = (sub influence, 1 / sub scaling);
//                    u[0] = (_, kind, n_oct, seed); u[1] = (invert, overflow, normal blend)
@compute @workgroup_size(16, 16)
fn pointwise(@builtin(global_invocation_id) gid: vec3<u32>) {
    let dims = out_dims();
    let p = vec2<i32>(gid.xy);
    if (p.x >= dims.x || p.y >= dims.y) {
        return;
    }
    var px = textureLoad(src, p, 0);
    let a = px.w;
    let op = P.u[0].x;
    switch op {
        case 1u: {
            if (a > 0.0) {
                let c = straight(px);
                let l = clamp(luminance(c), 0.0, 1.0);
                let blk = P.f[0].xyz;
                let wht = P.f[1].xyz;
                let t = blk + (wht - blk) * l;
                px = vec4<f32>(mix3(c, t, P.f[2].x) * a, a);
            }
        }
        case 2u: {
            if (a > 0.0) {
                let c = straight(px);
                let br = P.f[0].x;
                let k = P.f[0].y;
                let ct = P.f[0].z;
                if (P.u[0].y == 1u) {
                    px = vec4<f32>(max((c + br - 0.5) * k + 0.5, vec3<f32>(0.0)) * a, a);
                } else if (P.u[0].y == 2u) {
                    // Brightness & Contrast tone curves (effects crate bc_modern, AE-fitted #681).
                    var o = c;
                    let g = max(1.0 + 0.53 * ct, 0.05);
                    for (var i = 0; i < 3; i++) {
                        if (c[i] < 1.0) {
                            var v = max(c[i], 0.0);
                            if (br > 0.0) {
                                v = 1.0 - powz(1.0 - v, exp2(1.24 * br));
                            } else if (br < 0.0) {
                                v = powz(v, exp2(-1.32 * br));
                            }
                            if (ct != 0.0) {
                                if (v < 0.5) {
                                    v = 0.5 * powz(2.0 * v, g);
                                } else {
                                    v = 1.0 - 0.5 * powz(max(2.0 - 2.0 * v, 0.0), g);
                                }
                            }
                            o[i] = v;
                        }
                    }
                    px = vec4<f32>(o * a, a);
                }
            }
        }
        case 3u: {
            if (a > 0.0) {
                let c = straight(px);
                let hsl = rgb_to_hsl(c);
                var h = hsl.x;
                var s = hsl.y;
                var l = hsl.z;
                if (P.u[0].y != 0u) {
                    let cl = P.f[1].z;
                    var k = l;
                    if (cl > 0.0) {
                        k = 1.0 - l;
                    }
                    h = P.f[1].x;
                    s = P.f[1].y;
                    l = clamp(l + cl * k, 0.0, 1.0);
                } else {
                    let hue = P.f[0].x;
                    let sat = P.f[0].y;
                    let light = P.f[0].z;
                    var s2: f32;
                    if (sat >= 0.0) {
                        s2 = s + (1.0 - s) * sat * min(s, 1.0);
                    } else {
                        s2 = s * (1.0 + sat);
                    }
                    var l2: f32;
                    if (light >= 0.0) {
                        l2 = l + (1.0 - l) * light;
                    } else {
                        l2 = l * (1.0 + light);
                    }
                    h = fract_euclid(h + hue);
                    s = clamp(s2, 0.0, 1.0);
                    l = l2;
                }
                px = vec4<f32>(hsl_to_rgb(h, s, l) * a, a);
            }
        }
        case 4u: {
            if (a > 0.0) {
                let c = straight(px);
                let ib = P.f[0].x;
                let iw = P.f[0].y;
                let g = P.f[0].z;
                let ob = P.f[0].w;
                let ow = P.f[1].x;
                var o = vec3<f32>(0.0);
                for (var i = 0; i < 3; i++) {
                    var t = (c[i] - ib) / max(iw - ib, 1e-6);
                    if (P.u[0].y != 0u) {
                        t = max(t, 0.0);
                    }
                    if (P.u[0].z != 0u) {
                        t = min(t, 1.0);
                    }
                    t = powz(max(t, 0.0), 1.0 / g);
                    o[i] = ob + (ow - ob) * t;
                }
                px = vec4<f32>(o * a, a);
            }
        }
        case 5u: {
            if (a > 0.0) {
                let c = straight(px);
                var o = vec3<f32>(0.0);
                for (var i = 0; i < 3; i++) {
                    // Per-channel gain, offset, gamma; negative values are mirrored.
                    if (P.u[0].y != 0u) {
                        let x = c[i] * P.f[0][i] + P.f[1][i];
                        o[i] = sign(x) * powz(abs(x), 1.0 / P.f[2][i]);
                    } else {
                        let lin = srgb_to_linear(max(c[i], 0.0));
                        let x = lin * P.f[0][i] + P.f[1][i];
                        o[i] = sign(x) * linear_to_srgb(powz(abs(x), 1.0 / P.f[2][i]));
                    }
                }
                px = vec4<f32>(o * a, a);
            }
        }
        case 6u: {
            let ch = P.u[0].y;
            let blend = P.f[0].x;
            if (ch == 4u) {
                let na = 1.0 - a;
                var k = 0.0;
                if (a > 0.0) {
                    k = na / a;
                }
                let inv = vec4<f32>(px.xyz * k, na);
                px = px + (inv - px) * blend;
            } else if (a > 0.0) {
                let s = straight(px);
                var o = s;
                switch ch {
                    case 1u: { o.x = 1.0 - s.x; }
                    case 2u: { o.y = 1.0 - s.y; }
                    case 3u: { o.z = 1.0 - s.z; }
                    default: { o = 1.0 - s; }
                }
                px = vec4<f32>((s + (o - s) * blend) * a, a);
            }
        }
        case 7u: {
            let c = P.f[0];
            let invert = P.u[0].y != 0u;
            let opacity = P.f[1].x;
            var fa = px.w;
            if (invert) {
                fa = 1.0 - px.w;
            }
            let f = c.xyz * fa;
            let rgb = px.xyz + (f - px.xyz) * opacity;
            var alpha = px.w;
            if (invert) {
                alpha = px.w + (fa - px.w) * opacity;
            }
            px = vec4<f32>(rgb, alpha);
        }
        case 8u: {
            // effects::color2::map_ca: transparent pixels only change through the alpha curve.
            if (a <= 0.0 && px.x == 0.0 && px.y == 0.0 && px.z == 0.0) {
                let na = curve_eval(P.f[1].x, 0.0);
                if (na > 0.0) {
                    px = vec4<f32>(0.0, 0.0, 0.0, clamp(na, 0.0, 1.0));
                }
            } else {
                var c = vec3<f32>(0.0);
                var aa = max(a, 0.0);
                if (a > 1e-6) {
                    c = px.xyz / a;
                    aa = a;
                }
                var o = vec3<f32>(0.0);
                for (var i = 0; i < 3; i++) {
                    o[i] = curve_eval(P.f[0][i + 1], curve_eval(P.f[0].x, c[i]));
                }
                let na = clamp(curve_eval(P.f[1].x, aa), 0.0, 1.0);
                px = vec4<f32>(o * na, na);
            }
        }
        case 9u: {
            let s = P.f[0].xy;
            let e = P.f[0].zw;
            let d = e - s;
            let v = vec2<f32>(f32(p.x) + 0.5, f32(p.y) + 0.5) - s;
            let len2 = P.f[3].x;
            var t: f32;
            if (P.u[0].y != 0u) {
                t = sqrt((v.x * v.x + v.y * v.y) / len2);
            } else {
                t = (v.x * d.x + v.y * d.y) / len2;
            }
            let scatter = P.f[3].y;
            if (scatter > 0.0) {
                t += (hash_noise(gid.x, gid.y, P.u[0].z) - 0.5) * scatter;
            }
            t = clamp(t, 0.0, 1.0);
            let c0 = P.f[1].xyz;
            let c1 = P.f[2].xyz;
            // Keep the source alpha, as the CPU path does (#1063): the ramp fills only the shape.
            px = put(px, c0 + (c1 - c0) * t, px.w, P.f[3].z);
        }
        case 10u: {
            let contrast = P.f[0].x;
            let brightness = P.f[0].y;
            let scale = P.f[0].z;
            let scale_y = P.f[2].w;
            let infl = P.f[3].x;
            let fmul = P.f[3].y;
            let frac = P.f[0].w;
            let ox = P.f[1].x;
            let oy = P.f[1].y;
            let sr = P.f[1].z;
            let cr = P.f[1].w;
            let evo = P.f[2].x;
            let kind = P.u[0].y;
            let n_oct = P.u[0].z;
            let seed = P.u[0].w;
            let dx = f32(p.x) + 0.5 - ox;
            let dy = f32(p.y) + 0.5 - oy;
            let u = (dx * cr + dy * sr) / scale;
            let v = (-dx * sr + dy * cr) / scale_y;
            var sum = 0.0;
            var amp = 1.0;
            var norm = 0.0;
            var f = 1.0;
            for (var o = 0u; o < n_oct; o++) {
                var w = 1.0;
                if (o + 1u == n_oct && frac > 0.0) {
                    w = frac;
                }
                var n = soft_linear_noise(u * f, v * f, evo + f32(o) * 7.31, seed + o);
                let s = n * 2.0 - 1.0;
                if (kind == 1u) {
                    n = 1.0 - s * s;
                } else if (kind == 2u) {
                    n = 1.0 - abs(s);
                }
                sum += n * amp * w;
                norm += amp * w;
                amp *= infl;
                f *= fmul;
            }
            var val = sum / max(norm, 1e-6);
            val = (val - 0.5) * contrast + 0.5 + brightness;
            // Overflow: Clip, Soft Clamp, Wrap Back, Allow HDR Results.
            let ov = P.u[1].y;
            if (ov == 0u) {
                val = clamp(val, 0.0, 1.0);
            } else if (ov == 1u) {
                val = clamp(0.5 + 0.5 * tanh(2.0 * (val - 0.5)) / tanh(1.0), 0.0, 1.0);
            } else if (ov == 2u) {
                let t = val - 2.0 * floor(val / 2.0);
                val = select(t, 2.0 - t, t > 1.0);
            }
            if (P.u[1].x != 0u) {
                val = 1.0 - val;
            }
            let op = P.f[2].z;
            var fx = vec4<f32>(vec3<f32>(val) * op, op);
            if (P.u[1].z == 1u) {
                // Normal: the noise over the layer at Opacity.
                fx = fx + px * (1.0 - op);
            }
            // Inside the layer's alpha (its masks), then Blend With Original.
            px = px * P.f[2].y + fx * px.w * (1.0 - P.f[2].y);
        }
        default: {}
    }
    textureStore(out, p, px);
}

// Adjustment layer finish (Renderer::draw_adjustment): canvas `src` moves toward the adjusted
// comp `aux` by the footprint's alpha × opacity. The footprint is `data` (RGBA f32 rows of
// u[0].x pixels); f[0].x = opacity.
@compute @workgroup_size(16, 16)
fn adjust_mix(@builtin(global_invocation_id) gid: vec3<u32>) {
    let dims = out_dims();
    let p = vec2<i32>(gid.xy);
    if (p.x >= dims.x || p.y >= dims.y) {
        return;
    }
    let c = textureLoad(src, p, 0);
    let a = textureLoad(aux, p, 0);
    let k = data[(gid.y * P.u[0].x + gid.x) * 4u + 3u] * P.f[0].x;
    textureStore(out, p, c + (a - c) * k);
}

// ---------------------------------------------------------------- Classic 3D bokeh depth of field
// (render::three_d::bokeh: boost_highlights, row prefix sums, span gathers per blur level
// blended by each pixel's radius.)

// Highlight boost: f[0] = (gain, threshold, saturation).
@compute @workgroup_size(16, 16)
fn bokeh_boost(@builtin(global_invocation_id) gid: vec3<u32>) {
    let dims = out_dims();
    let p = vec2<i32>(gid.xy);
    if (p.x >= dims.x || p.y >= dims.y) {
        return;
    }
    var c = textureLoad(src, p, 0);
    let gain = P.f[0].x;
    let thr = clamp(P.f[0].y, 0.0, 1.0);
    let satk = P.f[0].z;
    if (c.w > 0.0 && gain > 0.0) {
        let a = c.w;
        let rgb = c.xyz / a;
        let l = 0.2126 * rgb.x + 0.7152 * rgb.y + 0.0722 * rgb.z;
        if (!(l < thr || (thr >= 1.0 && l < 1.0))) {
            var over = 1.0;
            if (thr < 1.0) {
                over = clamp((l - thr) / (1.0 - thr), 0.0, 1.0);
            }
            let k = 1.0 + gain * 4.0 * max(over, 0.25);
            let sat = 1.0 + satk;
            let o = max(vec3<f32>(l) + (rgb - vec3<f32>(l)) * sat, vec3<f32>(0.0)) * k;
            c = vec4<f32>(o * a, a);
        }
    }
    textureStore(out, p, c);
}

// Row prefix sums: out is (w + 1) × h, out[x] = sum of src[0..x]. One invocation per row.
@compute @workgroup_size(64, 1)
fn bokeh_prefix(@builtin(global_invocation_id) gid: vec3<u32>) {
    let dims = vec2<i32>(textureDimensions(src));
    let y = i32(gid.x);
    if (y >= dims.y) {
        return;
    }
    var acc = vec4<f32>(0.0);
    textureStore(out, vec2<i32>(0, y), acc);
    for (var x = 0; x < dims.x; x = x + 1) {
        acc = acc + textureLoad(src, vec2<i32>(x, y), 0);
        textureStore(out, vec2<i32>(x + 1, y), acc);
    }
}

fn bokeh_prefix_at(y: i32, t: f32) -> vec4<f32> {
    let n = i32(textureDimensions(src).x) - 1;
    if (t <= 0.0) {
        return vec4<f32>(0.0);
    }
    if (t >= f32(n)) {
        return textureLoad(src, vec2<i32>(n, y), 0);
    }
    let i = i32(floor(t));
    let f = t - f32(i);
    let a = textureLoad(src, vec2<i32>(i, y), 0);
    let b = textureLoad(src, vec2<i32>(i + 1, y), 0);
    return a + (b - a) * f;
}

// One blur level added into the accumulator `aux`: src = prefix sums; data = the radius map
// (w·h) then spans (dy, x0, x1, weight). u[0] = (span offset, span count, level, levels),
// u[1].x = 1 for the first level (no accumulator yet); f[0] = (1 / norm, lo, hi).
@compute @workgroup_size(16, 16)
fn bokeh_gather(@builtin(global_invocation_id) gid: vec3<u32>) {
    let dims = out_dims();
    let p = vec2<i32>(gid.xy);
    if (p.x >= dims.x || p.y >= dims.y) {
        return;
    }
    var acc = vec4<f32>(0.0);
    if (P.u[1].x == 0u) {
        acc = textureLoad(aux, p, 0);
    }
    let r = data[u32(p.y) * u32(dims.x) + u32(p.x)];
    let k = P.u[0].z;
    let n = P.u[0].w;
    var wgt = 1.0;
    if (n > 1u) {
        let lo = P.f[0].y;
        let hi = P.f[0].z;
        let f = clamp((r - lo) / (hi - lo) * f32(n - 1u), 0.0, f32(n - 1u));
        let kk = min(u32(floor(f)), n - 2u);
        let t = f - f32(kk);
        if (k == kk) {
            wgt = 1.0 - t;
        } else if (k == kk + 1u) {
            wgt = t;
        } else {
            wgt = 0.0;
        }
    }
    if (wgt > 0.0) {
        var g = vec4<f32>(0.0);
        let off = P.u[0].x;
        for (var i = 0u; i < P.u[0].y; i = i + 1u) {
            let s = off + i * 4u;
            let sy = p.y + i32(data[s]);
            if (sy < 0 || sy >= dims.y) {
                continue;
            }
            let hi = bokeh_prefix_at(sy, f32(p.x) + data[s + 2u] + 0.5);
            let lo = bokeh_prefix_at(sy, f32(p.x) + data[s + 1u] + 0.5);
            g = g + (hi - lo) * data[s + 3u];
        }
        acc = acc + g * P.f[0].x * wgt;
    }
    textureStore(out, p, acc);
}

// ---------------------------------------------------------------- pool

// Set every pixel to f[0] (clearing reused working textures; NaN poison under test).
@compute @workgroup_size(16, 16)
fn fill(@builtin(global_invocation_id) gid: vec3<u32>) {
    let dims = out_dims();
    let p = vec2<i32>(gid.xy);
    if (p.x >= dims.x || p.y >= dims.y) {
        return;
    }
    textureStore(out, p, P.f[0]);
}

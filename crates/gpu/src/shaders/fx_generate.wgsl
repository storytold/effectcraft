// GPU effects (generate family): see src/fx_generate.rs. Each entry point mirrors the CPU effect in
// effectcraft-effects operation for operation; parameter layouts are documented per entry.
//
// Hard edges (wipes) take per-column and per-row terms computed in f64 on the CPU and uploaded
// as (hi, lo) f32 pairs in `data`: near an edge the hi parts cancel exactly, so the edge sits
// where the CPU's f64 arithmetic puts it.

const GEN_DEG: f32 = 57.29577951308232;

// util::unpremul: (straight colour, alpha).
fn gen_unpremul(p: vec4<f32>) -> vec4<f32> {
    if (p.w > 1e-6) {
        return vec4<f32>(p.xyz / p.w, p.w);
    }
    return vec4<f32>(0.0, 0.0, 0.0, max(p.w, 0.0));
}

// util::smoothstep.
fn gen_smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    if (abs(e1 - e0) < 1e-9) {
        return select(1.0, 0.0, x < e0);
    }
    let t = clamp((x - e0) / (e1 - e0), 0.0, 1.0);
    return t * t * (3.0 - 2.0 * t);
}

// generate::gen_blend: mode = index into BlendMode::ALL, 255 = None (replace).
fn gen_blend(o: vec4<f32>, g: vec4<f32>, mode: u32) -> vec4<f32> {
    if (mode == 255u) {
        return g;
    }
    return blend_pixel(mode, o, g, 0.5);
}

// A (hi, lo) pair from `data`.
fn gen_hl(i: u32) -> vec2<f32> {
    return vec2<f32>(data[i], data[i + 1u]);
}

// (a + b) for (hi, lo) pairs whose hi parts cancel near zero.
fn gen_sum(a: vec2<f32>, b: vec2<f32>) -> f32 {
    return (a.x + b.x) + (a.y + b.y);
}

// ---------------------------------------------------------------- Radial Blur (raster::radial_blur)

// u[0] = (zoom, samples); f[0] = (cx, cy); data = per sample (sin, cos) (spin) or k (zoom).
@compute @workgroup_size(16, 16)
fn gen_radial_blur(@builtin(global_invocation_id) gid: vec3<u32>) {
    let dims = out_dims();
    let p = vec2<i32>(gid.xy);
    if (p.x >= dims.x || p.y >= dims.y) {
        return;
    }
    let zoom = P.u[0].x != 0u;
    let n = P.u[0].y;
    let c = P.f[0].xy;
    let v = vec2<f32>(f32(p.x) + 0.5 - c.x, f32(p.y) + 0.5 - c.y);
    var acc = vec4<f32>(0.0);
    for (var i = 0u; i < n; i++) {
        var s: vec2<f32>;
        if (zoom) {
            let k = data[i];
            s = vec2<f32>(c.x + v.x * k, c.y + v.y * k);
        } else {
            let sn = data[2u * i];
            let cs = data[2u * i + 1u];
            s = vec2<f32>(c.x + v.x * cs - v.y * sn, c.y + v.x * sn + v.y * cs);
        }
        acc += sample_bilinear(src, s.x, s.y);
    }
    textureStore(out, p, acc / f32(n));
}

// ---------------------------------------------------------------- CC Radial Fast Blur

// u[0].x = zoom mode (0 standard, 1 brightest, 2 darkest); f[0] = (cx, cy, s).
@compute @workgroup_size(16, 16)
fn gen_ccradialfast(@builtin(global_invocation_id) gid: vec3<u32>) {
    let dims = out_dims();
    let p = vec2<i32>(gid.xy);
    if (p.x >= dims.x || p.y >= dims.y) {
        return;
    }
    let mode = P.u[0].x;
    let c = P.f[0].xy;
    let s = P.f[0].z;
    let d = vec2<f32>(f32(p.x) + 0.5 - c.x, f32(p.y) + 0.5 - c.y);
    let len = sqrt(d.x * d.x + d.y * d.y) * s;
    let n = clamp(u32(ceil(len)), 2u, 64u);
    var acc = vec4<f32>(0.0);
    if (mode == 1u) {
        acc = vec4<f32>(-3.0e38);
    } else if (mode == 2u) {
        acc = vec4<f32>(3.0e38);
    }
    for (var i = 0u; i < n; i++) {
        let k = 1.0 - s * f32(i) / f32(n - 1u);
        let q = sample_bilinear(src, c.x + d.x * k, c.y + d.y * k);
        if (mode == 1u) {
            acc = max(acc, q);
        } else if (mode == 2u) {
            acc = min(acc, q);
        } else {
            acc += q / f32(n);
        }
    }
    let a = acc.w;
    textureStore(out, p, vec4<f32>(max(min(acc.xyz, vec3<f32>(a)), vec3<f32>(0.0)), clamp(a, 0.0, 1.0)));
}

// ---------------------------------------------------------------- Camera Lens Blur

// Linear working space and highlight boost before the blur.
// u[0] = (linear, boost); f[0] = (threshold, boost, saturation).
@compute @workgroup_size(16, 16)
fn gen_lens_pre(@builtin(global_invocation_id) gid: vec3<u32>) {
    let dims = out_dims();
    let p = vec2<i32>(gid.xy);
    if (p.x >= dims.x || p.y >= dims.y) {
        return;
    }
    var px = textureLoad(src, p, 0);
    if (P.u[0].x != 0u) {
        let u = gen_unpremul(px);
        px = vec4<f32>(srgb_to_linear(u.x) * u.w, srgb_to_linear(u.y) * u.w, srgb_to_linear(u.z) * u.w, u.w);
    }
    if (P.u[0].y != 0u) {
        let u = gen_unpremul(px);
        let c = u.xyz;
        let a = u.w;
        if (a > 0.0 && luminance(c) >= P.f[0].x) {
            let boost = P.f[0].y;
            let sat = P.f[0].z;
            let m = max(max(c.x, c.y), c.z);
            let tinted = c + (m - c) * (1.0 - sat);
            px = vec4<f32>((c + tinted * (boost - 1.0)) * a, px.w);
        }
    }
    textureStore(out, p, px);
}

// Per-row prefix sums: out(x + 1, y) = sum of src(0..=x, y), out(0, y) = 0. One thread per row.
@compute @workgroup_size(64, 1)
fn gen_row_prefix(@builtin(global_invocation_id) gid: vec3<u32>) {
    let dims = vec2<i32>(textureDimensions(src));
    let y = i32(gid.x);
    if (y >= dims.y) {
        return;
    }
    var acc = vec4<f32>(0.0);
    textureStore(out, vec2<i32>(0, y), acc);
    for (var x = 0; x < dims.x; x++) {
        acc += textureLoad(src, vec2<i32>(x, y), 0);
        textureStore(out, vec2<i32>(x + 1, y), acc);
    }
}

// The iris sum. data = spans (dy, left, right); aux = row prefix sums (when u[0].w).
// u[0] = (spans, repeat, linear, prefix); f[0].x = 1 / pixel count.
@compute @workgroup_size(16, 16)
fn gen_lens_blur(@builtin(global_invocation_id) gid: vec3<u32>) {
    let dims = out_dims();
    let p = vec2<i32>(gid.xy);
    if (p.x >= dims.x || p.y >= dims.y) {
        return;
    }
    let n = P.u[0].x;
    let repeat = P.u[0].y != 0u;
    let prefix = P.u[0].w != 0u;
    let w = dims.x;
    let h = dims.y;
    var acc = vec4<f32>(0.0);
    for (var s = 0u; s < n; s++) {
        var yy = p.y + i32(data[3u * s]);
        if (yy < 0 || yy >= h) {
            if (!repeat) {
                continue;
            }
            yy = clamp(yy, 0, h - 1);
        }
        var a = p.x + i32(data[3u * s + 1u]);
        var bnd = p.x + i32(data[3u * s + 2u]);
        if (!prefix) {
            for (var i = a; i <= bnd; i++) {
                if (repeat) {
                    acc += textureLoad(src, vec2<i32>(clamp(i, 0, w - 1), yy), 0);
                } else if (i >= 0 && i < w) {
                    acc += textureLoad(src, vec2<i32>(i, yy), 0);
                }
            }
            continue;
        }
        if (repeat) {
            if (a < 0) {
                let k = min(-a, bnd - a + 1);
                acc += textureLoad(src, vec2<i32>(0, yy), 0) * f32(k);
                a = 0;
            }
            if (bnd >= w) {
                let k = min(bnd - (w - 1), bnd - a + 1);
                acc += textureLoad(src, vec2<i32>(w - 1, yy), 0) * f32(k);
                bnd = w - 1;
            }
        } else {
            a = max(a, 0);
            bnd = min(bnd, w - 1);
        }
        if (a <= bnd) {
            acc += textureLoad(aux, vec2<i32>(bnd + 1, yy), 0) - textureLoad(aux, vec2<i32>(a, yy), 0);
        }
    }
    var px = acc * P.f[0].x;
    px.w = clamp(px.w, 0.0, 1.0);
    if (P.u[0].z != 0u) {
        let u = gen_unpremul(px);
        px = vec4<f32>(linear_to_srgb(max(u.x, 0.0)) * u.w, linear_to_srgb(max(u.y, 0.0)) * u.w, linear_to_srgb(max(u.z, 0.0)) * u.w, u.w);
    }
    textureStore(out, p, px);
}

// ---------------------------------------------------------------- wipes

// Linear Wipe. data = per column ((x + 0.5)·dx − edge) (hi, lo), then per row ((y + 0.5)·dy)
// (hi, lo); u[0].x = row table offset; f[0].x = feather.
@compute @workgroup_size(16, 16)
fn gen_linear_wipe(@builtin(global_invocation_id) gid: vec3<u32>) {
    let dims = out_dims();
    let p = vec2<i32>(gid.xy);
    if (p.x >= dims.x || p.y >= dims.y) {
        return;
    }
    let d = gen_sum(gen_hl(2u * u32(p.x)), gen_hl(P.u[0].x + 2u * u32(p.y)));
    let k = clamp(d / P.f[0].x + 0.5, 0.0, 1.0);
    textureStore(out, p, textureLoad(src, p, 0) * k);
}

// Venetian Blinds. Per column (6 floats): (cx − done·w), (cx − w − done·w), (cx − w) as
// (hi, lo), with cx = ((x + 0.5)·dx) mod w; per row (2 floats): ((y + 0.5)·dy) mod w.
// u[0].x = row table offset; f[0].x = feather.
@compute @workgroup_size(16, 16)
fn gen_venetian(@builtin(global_invocation_id) gid: vec3<u32>) {
    let dims = out_dims();
    let p = vec2<i32>(gid.xy);
    if (p.x >= dims.x || p.y >= dims.y) {
        return;
    }
    let col = 6u * u32(p.x);
    let row = gen_hl(P.u[0].x + 2u * u32(p.y));
    // (d mod w) wraps once the column and row terms reach w.
    let wrapped = gen_sum(gen_hl(col + 4u), row) >= 0.0;
    var t: f32;
    if (wrapped) {
        t = gen_sum(gen_hl(col + 2u), row);
    } else {
        t = gen_sum(gen_hl(col), row);
    }
    let k = clamp(t / P.f[0].x + 0.5, 0.0, 1.0);
    textureStore(out, p, textureLoad(src, p, 0) * k);
}

// Angle (degrees, clockwise from up) of the pixel vector rotated by rotation `r` (0 = start,
// 1 = start + edge, 2 = start − edge), from the per-column / per-row tables of Radial Wipe.
fn gen_psi(r: u32, x: u32, y: u32) -> f32 {
    let c = 9u * x + 3u * r;
    let w = P.u[0].x + 9u * y + 3u * r;
    // x' = ux·cos R − uy·sin R (cancels at the edge), y' = uy·cos R + ux·sin R.
    let xr = (data[c] - data[w]) + (data[c + 1u] - data[w + 1u]);
    let yr = data[w + 2u] + data[c + 2u];
    return atan2(xr, yr) * GEN_DEG;
}

// Radial Wipe. data: per column 9 floats (for each rotation: ux·cos R (hi, lo), ux·sin R),
// per row 9 floats (uy·sin R (hi, lo), uy·cos R); u[0] = (row offset, wipe);
// f[0] = (edge, feather).
@compute @workgroup_size(16, 16)
fn gen_radial_wipe(@builtin(global_invocation_id) gid: vec3<u32>) {
    let dims = out_dims();
    let p = vec2<i32>(gid.xy);
    if (p.x >= dims.x || p.y >= dims.y) {
        return;
    }
    let x = u32(p.x);
    let y = u32(p.y);
    let dir = P.u[0].y;
    let edge = P.f[0].x;
    var a = gen_psi(0u, x, y);
    if (a < 0.0) {
        a += 360.0;
    }
    var base: f32;
    if (dir == 0u || (dir == 2u && a <= 180.0)) {
        base = a - edge;
        if (abs(base) < 90.0) {
            base = gen_psi(1u, x, y);
        }
    } else {
        base = (360.0 - a) - edge;
        if (abs(base) < 90.0) {
            base = -gen_psi(2u, x, y);
        }
    }
    let k = clamp(base / P.f[0].y + 0.5, 0.0, 1.0);
    textureStore(out, p, textureLoad(src, p, 0) * k);
}

// Gradient Wipe: aux = the placed gradient layer. u[0].x = invert; f[0] = (done, softness).
@compute @workgroup_size(16, 16)
fn gen_gradient_wipe(@builtin(global_invocation_id) gid: vec3<u32>) {
    let dims = out_dims();
    let p = vec2<i32>(gid.xy);
    if (p.x >= dims.x || p.y >= dims.y) {
        return;
    }
    let done = P.f[0].x;
    let soft = P.f[0].y;
    let g = gen_unpremul(textureLoad(aux, p, 0));
    var t = clamp(luminance(g.xyz), 0.0, 1.0);
    if (P.u[0].x != 0u) {
        t = 1.0 - t;
    }
    var k = 0.0;
    if (done >= 1.0) {
        k = 0.0;
    } else if (soft > 0.0) {
        k = clamp((t - (done * (1.0 + soft) - soft)) / soft, 0.0, 1.0);
    } else if (t >= done) {
        k = 1.0;
    }
    textureStore(out, p, textureLoad(src, p, 0) * k);
}

// ---------------------------------------------------------------- Cell Pattern (generate2)

// Worley distances for (u, v) in cell units: (F1, F2, hash of the nearest cell).
fn gen_worley(u: f32, v: f32, disperse: f32, evo: f32, seed: u32) -> vec3<f32> {
    let tile = P.u[1].x != 0u;
    let nx = i32(P.u[1].y);
    let ny = i32(P.u[1].z);
    let cx = i32(floor(u));
    let cy = i32(floor(v));
    var f1 = 3.0e38;
    var f2 = 3.0e38;
    var id = 0.0;
    for (var j = -1; j <= 1; j++) {
        for (var i = -1; i <= 1; i++) {
            let gx = cx + i;
            let gy = cy + j;
            var tx = gx;
            var ty = gy;
            if (tile) {
                tx = imod(gx, nx);
                ty = imod(gy, ny);
            }
            let hx = bitcast<u32>(tx);
            let hy = bitcast<u32>(ty);
            let h1 = hash_noise(hx, hy, seed);
            let h2 = hash_noise(hx, hy, seed + 1u);
            let h3 = hash_noise(hx, hy, seed + 2u);
            let ang = 6.283185307179586 * (h3 + evo);
            let px = f32(gx) + 0.5 + (h1 - 0.5) * disperse + 0.15 * disperse * cos(ang);
            let py = f32(gy) + 0.5 + (h2 - 0.5) * disperse + 0.15 * disperse * sin(ang);
            let d = sqrt((px - u) * (px - u) + (py - v) * (py - v));
            if (d < f1) {
                f2 = f1;
                f1 = d;
                id = hash_noise(hx, hy, seed + 3u);
            } else if (d < f2) {
                f2 = d;
            }
        }
    }
    return vec3<f32>(f1, f2, id);
}

fn gen_cell_value(pattern: u32, f1: f32, f2: f32, id: f32) -> f32 {
    let e = f2 - f1;
    switch pattern {
        case 0u: { return clamp(1.0 - f1 * 1.3, 0.0, 1.0); }
        case 1u: { return id * 0.8 + 0.2 * clamp(1.0 - f1, 0.0, 1.0); }
        case 2u: { return clamp(e * 2.0, 0.0, 1.0); }
        case 3u: { return id * gen_smoothstep(0.0, 0.06, e); }
        case 4u: { return id; }
        case 5u: { return clamp(1.0 - f1 * f1 * 2.0, 0.0, 1.0); }
        case 6u: { return id * clamp(e * 3.0, 0.0, 1.0); }
        default: { return 1.0 - clamp(abs(e - 0.15) * 5.0, 0.0, 1.0); }
    }
}

fn gen_overflow(v: f32, mode: u32) -> f32 {
    if (mode == 1u) {
        return 0.5 + 0.5 * tanh((v - 0.5) * 2.0);
    }
    if (mode == 2u) {
        let t = v - 2.0 * floor(v * 0.5);
        return select(t, 2.0 - t, t > 1.0);
    }
    return clamp(v, 0.0, 1.0);
}

// u[0] = (pattern kind, overflow, invert, seed); u[1] = (tile, columns, rows);
// f[0] = (contrast, disperse, size, evolution mod 1); f[1] = (0.5 − offset x, 0.5 − offset y).
@compute @workgroup_size(16, 16)
fn gen_cell_pattern(@builtin(global_invocation_id) gid: vec3<u32>) {
    let dims = out_dims();
    let p = vec2<i32>(gid.xy);
    if (p.x >= dims.x || p.y >= dims.y) {
        return;
    }
    let size = P.f[0].z;
    let u = (f32(p.x) + P.f[1].x) / size;
    let v = (f32(p.y) + P.f[1].y) / size;
    let w = gen_worley(u, v, P.f[0].y, P.f[0].w, P.u[0].w);
    var val = gen_cell_value(P.u[0].x, w.x, w.y, w.z);
    val = gen_overflow((val - 0.5) * P.f[0].x + 0.5, P.u[0].y);
    if (P.u[0].z != 0u) {
        val = 1.0 - val;
    }
    textureStore(out, p, vec4<f32>(val, val, val, 1.0));
}

// ---------------------------------------------------------------- Checkerboard / Grid / 4-Color

// Signed distance (in feather units) to the nearest cell edge, positive in even cells.
fn gen_axis(q: f32, size: f32, f: f32) -> f32 {
    let u = q / size;
    let fl = floor(u);
    let fr = u - fl;
    let d = min(fr, 1.0 - fr) * size;
    let n = i32(fl);
    let s = select(-1.0, 1.0, imod(n, 2) == 0);
    return s * min(d / (f * 0.5), 1.0);
}

// u[0].x = mode; f[0] = (0.5 − anchor x, 0.5 − anchor y, cell w, cell h);
// f[1] = (feather w, feather h, opacity); f[2] = colour.
@compute @workgroup_size(16, 16)
fn gen_checker(@builtin(global_invocation_id) gid: vec3<u32>) {
    let dims = out_dims();
    let p = vec2<i32>(gid.xy);
    if (p.x >= dims.x || p.y >= dims.y) {
        return;
    }
    let sx = gen_axis(f32(p.x) + P.f[0].x, P.f[0].z, P.f[1].x);
    let sy = gen_axis(f32(p.y) + P.f[0].y, P.f[0].w, P.f[1].y);
    let cov = clamp(0.5 + 0.5 * sx * sy, 0.0, 1.0) * P.f[1].z;
    let c = P.f[2].xyz;
    textureStore(out, p, gen_blend(textureLoad(src, p, 0), vec4<f32>(c * cov, cov), P.u[0].x));
}

// f32::rem_euclid.
fn gen_rem(x: f32, m: f32) -> f32 {
    let r = x - m * trunc(x / m);
    return select(r, r + m, r < 0.0);
}

// u[0] = (mode, invert); f[0] = (0.5 − anchor x, 0.5 − anchor y, cell w, cell h);
// f[1] = (feather w, feather h, opacity, border); f[2] = colour.
@compute @workgroup_size(16, 16)
fn gen_grid(@builtin(global_invocation_id) gid: vec3<u32>) {
    let dims = out_dims();
    let p = vec2<i32>(gid.xy);
    if (p.x >= dims.x || p.y >= dims.y) {
        return;
    }
    let w = P.f[0].z;
    let h = P.f[0].w;
    let border = P.f[1].w;
    let gx = gen_rem(f32(p.x) + P.f[0].x, w);
    let gy = gen_rem(f32(p.y) + P.f[0].y, h);
    let dx = min(gx, w - gx);
    let dy = min(gy, h - gy);
    let cx = clamp((border / 2.0 - dx) / P.f[1].x + 0.5, 0.0, 1.0);
    let cy = clamp((border / 2.0 - dy) / P.f[1].y + 0.5, 0.0, 1.0);
    var cov = 0.0;
    if (border > 0.0) {
        cov = max(cx, cy);
    }
    if (P.u[0].y != 0u) {
        cov = 1.0 - cov;
    }
    let a = cov * P.f[1].z;
    textureStore(out, p, gen_blend(textureLoad(src, p, 0), vec4<f32>(P.f[2].xyz * a, a), P.u[0].x));
}

// u[0] = (mode, seed); f[0..4] = points (x, y); f[4..8] = colours;
// f[8] = (blend · 100, jitter, opacity).
@compute @workgroup_size(16, 16)
fn gen_fourcolor(@builtin(global_invocation_id) gid: vec3<u32>) {
    let dims = out_dims();
    let p = vec2<i32>(gid.xy);
    if (p.x >= dims.x || p.y >= dims.y) {
        return;
    }
    let bl = P.f[8].x;
    var acc = vec3<f32>(0.0);
    var wsum = 0.0;
    for (var i = 0; i < 4; i++) {
        let q = P.f[i].xy;
        let dx = f32(p.x) + 0.5 - q.x;
        let dy = f32(p.y) + 0.5 - q.y;
        let d2 = dx * dx + dy * dy;
        let w = 1.0 / pow(d2 / bl + 1e-6, 1.5);
        acc += P.f[4 + i].xyz * w;
        wsum += w;
    }
    var c = acc / wsum;
    let jitter = P.f[8].y;
    if (jitter > 0.0) {
        let n = (hash_noise(u32(p.x), u32(p.y), P.u[0].y) - 0.5) * jitter * (8.0 / 255.0);
        c += vec3<f32>(n);
    }
    let o = textureLoad(src, p, 0);
    // None keeps the layer's alpha, as the CPU path does (#1381).
    let op = select(P.f[8].z, P.f[8].z * o.w, P.u[0].x == 255u);
    textureStore(out, p, gen_blend(o, vec4<f32>(c * op, op), P.u[0].x));
}

// ---------------------------------------------------------------- Noise / Add Grain

// Noise (misc::noise). u[0] = (frame seed, colour, clip, width); f[0].x = amount.
@compute @workgroup_size(16, 16)
fn gen_noise(@builtin(global_invocation_id) gid: vec3<u32>) {
    let dims = out_dims();
    let p = vec2<i32>(gid.xy);
    if (p.x >= dims.x || p.y >= dims.y) {
        return;
    }
    var px = textureLoad(src, p, 0);
    let a = px.w;
    if (a <= 0.0) {
        textureStore(out, p, px);
        return;
    }
    let seed = P.u[0].x;
    let w = P.u[0].w;
    let amt = P.f[0].x;
    let x = u32(p.x);
    let y = u32(p.y);
    var n = vec3<f32>((hash_noise(x, y, seed) - 0.5) * amt);
    if (P.u[0].y != 0u) {
        n.y = (hash_noise(x + w, y, seed) - 0.5) * amt;
        n.z = (hash_noise(x + 2u * w, y, seed) - 0.5) * amt;
    }
    if (P.u[0].z != 0u) {
        px = vec4<f32>(clamp(px.xyz + n * a, vec3<f32>(0.0), vec3<f32>(a)), a);
    } else {
        // Unclipped: values past black / white wrap around.
        px = vec4<f32>(fract_euclid(px.x / a + n.x) * a, fract_euclid(px.y / a + n.y) * a, fract_euclid(px.z / a + n.z) * a, a);
    }
    textureStore(out, p, px);
}

// Add Grain's noise planes (GrainLook::planes before Softness): rgb = per-channel zero-mean
// value noise. u[0] = (mono, seed, salt); f[0].xyz = x divisors (size · aspect), f[0].w = frame;
// f[1].xyz = y divisors (size).
@compute @workgroup_size(16, 16)
fn gen_grain_planes(@builtin(global_invocation_id) gid: vec3<u32>) {
    let dims = out_dims();
    let p = vec2<i32>(gid.xy);
    if (p.x >= dims.x || p.y >= dims.y) {
        return;
    }
    let x = f32(p.x);
    let y = f32(p.y);
    let frame = P.f[0].w;
    let seed = P.u[0].y;
    let salt = P.u[0].z;
    let n0 = value_noise(x / P.f[0].x, y / P.f[1].x, frame, seed) - 0.5;
    var g = vec3<f32>(n0);
    if (P.u[0].x == 0u) {
        g.y = value_noise(x / P.f[0].y, y / P.f[1].y, frame, seed + salt) - 0.5;
        g.z = value_noise(x / P.f[0].z, y / P.f[1].z, frame, seed + 2u * salt) - 0.5;
    }
    textureStore(out, p, vec4<f32>(g, 0.0));
}

// GrainLook::blend.
fn gen_grain_blend(mode: u32, c: f32, n: f32) -> f32 {
    var v: f32;
    switch mode {
        case 1u: { v = c * (1.0 + n); }
        case 2u: { v = c + n; }
        case 3u: { v = 1.0 - (1.0 - c) * (1.0 - abs(n)); }
        case 4u: { v = c + n * 4.0 * c * (1.0 - c); }
        default: { v = c + n * (1.0 - 0.5 * c); }
    }
    return max(v, 0.0);
}

// Add Grain applied: aux = noise planes. u[0] = (mono, mode, tint); f[0] = (amp, saturation,
// midpoint); f[1].xyz = tint factors; f[2].xyz = channel intensities; f[3].xyz = tone weights.
@compute @workgroup_size(16, 16)
fn gen_grain_apply(@builtin(global_invocation_id) gid: vec3<u32>) {
    let dims = out_dims();
    let p = vec2<i32>(gid.xy);
    if (p.x >= dims.x || p.y >= dims.y) {
        return;
    }
    let px = textureLoad(src, p, 0);
    let u = gen_unpremul(px);
    let a = u.w;
    if (a <= 0.0) {
        textureStore(out, p, px);
        return;
    }
    let c = u.xyz;
    var g = textureLoad(aux, p, 0).xyz;
    if (P.u[0].x != 0u) {
        g = vec3<f32>(g.x);
    } else {
        let gm = (g.x + g.y + g.z) / 3.0;
        g = gm + (g - gm) * P.f[0].y;
    }
    if (P.u[0].z != 0u) {
        g = g * P.f[1].xyz;
    }
    let mid = P.f[0].z;
    let l = clamp(luminance(c), 0.0, 1.0);
    let s = 1.0 - gen_smoothstep(0.0, mid, l);
    let hi = gen_smoothstep(mid, 1.0, l);
    let tones = P.f[3].xyz;
    let weight = tones.x * s + tones.y * (1.0 - s - hi) + tones.z * hi;
    let amp = P.f[0].x;
    let ci = P.f[2].xyz;
    let mode = P.u[0].y;
    let o = vec3<f32>(
        gen_grain_blend(mode, c.x, g.x * amp * ci.x * weight),
        gen_grain_blend(mode, c.y, g.y * amp * ci.y * weight),
        gen_grain_blend(mode, c.z, g.z * amp * ci.z * weight),
    );
    textureStore(out, p, vec4<f32>(o * a, a));
}

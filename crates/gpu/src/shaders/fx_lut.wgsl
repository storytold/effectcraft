// ---------------------------------------------------------------- colour management (fx_lut.rs)
//
// A colour program in `data`: opcodes with their arguments, ending in 0, then LUT tables
// (layout in fx_lut.rs::push_lut). Every step mirrors effects::ocio / effects::utility.

fn fxl_d3(i: u32) -> vec3<f32> {
    return vec3<f32>(data[i], data[i + 1u], data[i + 2u]);
}

// ---- LUTs (utility::Lut)

fn fxl_lut_norm(o: u32, c: vec3<f32>) -> vec3<f32> {
    let dmin = fxl_d3(o + 2u);
    let dmax = fxl_d3(o + 5u);
    return clamp((c - dmin) / max(dmax - dmin, vec3<f32>(1e-9)), vec3<f32>(0.0), vec3<f32>(1.0));
}

fn fxl_lut_3d(base: u32, n: u32, r: u32, g: u32, b: u32) -> vec3<f32> {
    return fxl_d3(base + (r + g * n + b * n * n) * 3u);
}

// Lut::apply_interp: 0 nearest, 1 trilinear, otherwise tetrahedral (1D tables linear unless 0).
fn fxl_lut_apply(o: u32, c0: vec3<f32>, interp: u32) -> vec3<f32> {
    let n1 = u32(data[o]);
    let n3 = u32(data[o + 1u]);
    let b1 = o + 8u;
    let b3 = b1 + n1 * 3u;
    var c = c0;
    if (n1 > 1u) {
        let t = fxl_lut_norm(o, c);
        for (var i = 0u; i < 3u; i++) {
            let x = t[i] * f32(n1 - 1u);
            if (interp == 0u) {
                c = vset3f(c, u32(i), data[b1 + min(u32(round_away(x)), n1 - 1u) * 3u + i]);
            } else {
                let i0 = min(u32(floor(x)), n1 - 2u);
                let f = x - f32(i0);
                let a = data[b1 + i0 * 3u + i];
                let b = data[b1 + (i0 + 1u) * 3u + i];
                c = vset3f(c, u32(i), a + (b - a) * f);
            }
        }
    }
    if (n3 > 1u) {
        let t = fxl_lut_norm(o, c);
        let s = f32(n3 - 1u);
        if (interp == 0u) {
            let ir = min(u32(round_away(t.x * s)), n3 - 1u);
            let ig = min(u32(round_away(t.y * s)), n3 - 1u);
            let ib = min(u32(round_away(t.z * s)), n3 - 1u);
            return fxl_lut_3d(b3, n3, ir, ig, ib);
        }
        let v = t * s;
        let r = min(u32(floor(v.x)), n3 - 2u);
        let g = min(u32(floor(v.y)), n3 - 2u);
        let b = min(u32(floor(v.z)), n3 - 2u);
        let fr = v.x - f32(r);
        let fg = v.y - f32(g);
        let fb = v.z - f32(b);
        if (interp == 1u) {
            let c00 = mix_exact(fxl_lut_3d(b3, n3, r, g, b), fxl_lut_3d(b3, n3, r + 1u, g, b), fr);
            let c10 = mix_exact(fxl_lut_3d(b3, n3, r, g + 1u, b), fxl_lut_3d(b3, n3, r + 1u, g + 1u, b), fr);
            let c01 = mix_exact(fxl_lut_3d(b3, n3, r, g, b + 1u), fxl_lut_3d(b3, n3, r + 1u, g, b + 1u), fr);
            let c11 = mix_exact(fxl_lut_3d(b3, n3, r, g + 1u, b + 1u), fxl_lut_3d(b3, n3, r + 1u, g + 1u, b + 1u), fr);
            return mix_exact(mix_exact(c00, c10, fg), mix_exact(c01, c11, fg), fb);
        }
        let c000 = fxl_lut_3d(b3, n3, r, g, b);
        let c111 = fxl_lut_3d(b3, n3, r + 1u, g + 1u, b + 1u);
        var w1: f32;
        var w2: f32;
        var w3: f32;
        var p1: vec3<f32>;
        var p2: vec3<f32>;
        if (fr > fg) {
            if (fg > fb) {
                w1 = fr;
                w2 = fg;
                w3 = fb;
                p1 = fxl_lut_3d(b3, n3, r + 1u, g, b);
                p2 = fxl_lut_3d(b3, n3, r + 1u, g + 1u, b);
            } else if (fr > fb) {
                w1 = fr;
                w2 = fb;
                w3 = fg;
                p1 = fxl_lut_3d(b3, n3, r + 1u, g, b);
                p2 = fxl_lut_3d(b3, n3, r + 1u, g, b + 1u);
            } else {
                w1 = fb;
                w2 = fr;
                w3 = fg;
                p1 = fxl_lut_3d(b3, n3, r, g, b + 1u);
                p2 = fxl_lut_3d(b3, n3, r + 1u, g, b + 1u);
            }
        } else if (fb > fg) {
            w1 = fb;
            w2 = fg;
            w3 = fr;
            p1 = fxl_lut_3d(b3, n3, r, g, b + 1u);
            p2 = fxl_lut_3d(b3, n3, r, g + 1u, b + 1u);
        } else if (fb > fr) {
            w1 = fg;
            w2 = fb;
            w3 = fr;
            p1 = fxl_lut_3d(b3, n3, r, g + 1u, b);
            p2 = fxl_lut_3d(b3, n3, r, g + 1u, b + 1u);
        } else {
            w1 = fg;
            w2 = fr;
            w3 = fb;
            p1 = fxl_lut_3d(b3, n3, r, g + 1u, b);
            p2 = fxl_lut_3d(b3, n3, r + 1u, g + 1u, b);
        }
        return c000 + w1 * (p1 - c000) + w2 * (p2 - p1) + w3 * (c111 - p2);
    }
    return c;
}

// a + (b - a) t, per component (the CPU's lerp, not WGSL's mix).
fn mix_exact(a: vec3<f32>, b: vec3<f32>, t: f32) -> vec3<f32> {
    return a + (b - a) * t;
}

// ocio::shape over `n` (x, y) points at `at` (`flip`: the curve with x and y swapped).
fn fxl_shape(at: u32, n: u32, x: f32, flip: bool) -> f32 {
    if (n < 2u) {
        return x;
    }
    let kx = select(0u, 1u, flip);
    let ky = 1u - kx;
    // partition_point(|p| p.x < x).
    var lo = 0u;
    var hi = n;
    while (lo < hi) {
        let mid = (lo + hi) / 2u;
        if (data[at + mid * 2u + kx] < x) {
            lo = mid + 1u;
        } else {
            hi = mid;
        }
    }
    let i = clamp(lo, 1u, n - 1u);
    let ax = data[at + (i - 1u) * 2u + kx];
    let ay = data[at + (i - 1u) * 2u + ky];
    let bx = data[at + i * 2u + kx];
    let by = data[at + i * 2u + ky];
    let t = clamp((x - ax) / max(bx - ax, 1e-12), 0.0, 1.0);
    return ay + (by - ay) * t;
}

// Shaper channel k's points (three counts, then the channels' (x, y) pairs).
fn fxl_shaper_at(s: u32, k: u32) -> vec2<u32> {
    var at = s + 3u;
    for (var j = 0u; j < k; j++) {
        at += u32(data[s + j]) * 2u;
    }
    return vec2<u32>(at, u32(data[s + k]));
}

fn fxl_file_fwd(o: u32, sh: i32, c0: vec3<f32>, interp: u32) -> vec3<f32> {
    var c = c0;
    if (sh >= 0) {
        for (var k = 0u; k < 3u; k++) {
            let a = fxl_shaper_at(u32(sh), k);
            c = vset3f(c, u32(k), fxl_shape(a.x, a.y, c[k], false));
        }
    }
    return fxl_lut_apply(o, c, interp);
}

// The inverse of a 1D table channel (ocio::FileXform::apply: points (table, domain)).
fn fxl_inv_1d(o: u32, i: u32, y: f32) -> f32 {
    let n = u32(data[o]);
    let dmin = data[o + 2u + i];
    let dmax = data[o + 5u + i];
    let b1 = o + 8u;
    // partition_point over the table's values.
    var lo = 0u;
    var hi = n;
    while (lo < hi) {
        let mid = (lo + hi) / 2u;
        if (data[b1 + mid * 3u + i] < y) {
            lo = mid + 1u;
        } else {
            hi = mid;
        }
    }
    let k = clamp(lo, 1u, n - 1u);
    let ax = data[b1 + (k - 1u) * 3u + i];
    let bx = data[b1 + k * 3u + i];
    let ay = dmin + (dmax - dmin) * f32(k - 1u) / f32(n - 1u);
    let by = dmin + (dmax - dmin) * f32(k) / f32(n - 1u);
    let t = clamp((y - ax) / max(bx - ax, 1e-12), 0.0, 1.0);
    return ay + (by - ay) * t;
}

fn fxl_file(o: u32, sh: i32, c: vec3<f32>, interp: u32, inverse: bool) -> vec3<f32> {
    if (!inverse) {
        return fxl_file_fwd(o, sh, c, interp);
    }
    let n1 = u32(data[o]);
    let n3 = u32(data[o + 1u]);
    if (n3 == 0u) {
        var x = c;
        if (n1 > 1u) {
            x = vec3<f32>(fxl_inv_1d(o, 0u, c.x), fxl_inv_1d(o, 1u, c.y), fxl_inv_1d(o, 2u, c.z));
        }
        if (sh >= 0) {
            for (var k = 0u; k < 3u; k++) {
                let a = fxl_shaper_at(u32(sh), k);
                x = vset3f(x, u32(k), fxl_shape(a.x, a.y, x[k], true));
            }
        }
        return x;
    }
    // Damped fixed point from the target, as the CPU.
    var x = c;
    for (var it = 0; it < 40; it++) {
        let y = fxl_file_fwd(o, sh, x, interp);
        let e = c - y;
        if (all(abs(e) < vec3<f32>(1e-6))) {
            break;
        }
        x = x + e;
    }
    return x;
}

// ---- transfer functions (ocio::Tf)

const FXL_PQ_M1: f32 = 0.1593017578125;
const FXL_PQ_M2: f32 = 78.84375;
const FXL_PQ_C1: f32 = 0.8359375;
const FXL_PQ_C2: f32 = 18.8515625;
const FXL_PQ_C3: f32 = 18.6875;

fn fxl_copysign(m: f32, v: f32) -> f32 {
    return select(abs(m), -abs(m), v < 0.0 || (v == 0.0 && bitcast<u32>(v) != 0u));
}

fn fxl_decode(tf: u32, g: f32, v: f32) -> f32 {
    switch (tf) {
        case 1u: {
            let a = abs(v);
            var l: f32;
            if (a <= 0.04045) {
                l = a / 12.92;
            } else {
                l = pow((a + 0.055) / 1.055, 2.4);
            }
            return fxl_copysign(l, v);
        }
        case 2u: {
            return fxl_copysign(powz(abs(v), g), v);
        }
        case 3u: {
            if (v <= 0.155251141552511) {
                return (v - 0.0729055341958355) / 10.5402377416545;
            }
            if (v < (log2(65504.0) + 9.72) / 17.52) {
                return exp2(v * 17.52 - 9.72);
            }
            return 65504.0;
        }
        case 4u: {
            let e = powz(max(v, 0.0), 1.0 / FXL_PQ_M2);
            let l = powz(max(e - FXL_PQ_C1, 0.0) / (FXL_PQ_C2 - FXL_PQ_C3 * e), 1.0 / FXL_PQ_M1);
            return l * 100.0;
        }
        case 5u: {
            // ACEScc.
            if (v < (-15.0 + 9.72) / 17.52) {
                return (exp2(v * 17.52 - 9.72) - exp2(-16.0)) * 2.0;
            }
            if (v < (log2(65504.0) + 9.72) / 17.52) {
                return exp2(v * 17.52 - 9.72);
            }
            return 65504.0;
        }
        default: {
            return v;
        }
    }
}

fn fxl_encode(tf: u32, g: f32, v: f32) -> f32 {
    switch (tf) {
        case 1u: {
            let a = abs(v);
            var e: f32;
            if (a <= 0.0031308) {
                e = a * 12.92;
            } else {
                e = 1.055 * pow(a, 1.0 / 2.4) - 0.055;
            }
            return fxl_copysign(e, v);
        }
        case 2u: {
            return fxl_copysign(powz(abs(v), 1.0 / g), v);
        }
        case 3u: {
            if (v <= 0.0078125) {
                return 10.5402377416545 * v + 0.0729055341958355;
            }
            return (log2(v) + 9.72) / 17.52;
        }
        case 4u: {
            let y = powz(max(v / 100.0, 0.0), FXL_PQ_M1);
            return powz((FXL_PQ_C1 + FXL_PQ_C2 * y) / (1.0 + FXL_PQ_C3 * y), FXL_PQ_M2);
        }
        case 5u: {
            // ACEScc.
            if (v <= 0.0) {
                return (-16.0 + 9.72) / 17.52;
            }
            if (v < exp2(-15.0)) {
                return (log2(exp2(-16.0) + v * 0.5) + 9.72) / 17.52;
            }
            return (log2(v) + 9.72) / 17.52;
        }
        default: {
            return v;
        }
    }
}

// ---- ASC CDL (ocio::Cdl), args at `a`: slope (3), offset (3), power (3), sat, clamp, inverse.

const FXL_LUMA: vec3<f32> = vec3<f32>(0.2126, 0.7152, 0.0722);

fn fxl_cdl(a: u32, c0: vec3<f32>) -> vec3<f32> {
    let slope = fxl_d3(a);
    let offset = fxl_d3(a + 3u);
    let power = fxl_d3(a + 6u);
    let sat = data[a + 9u];
    let clampo = data[a + 10u] != 0.0;
    let zero = vec3<f32>(0.0);
    let one = vec3<f32>(1.0);
    if (data[a + 11u] == 0.0) {
        var o = vec3<f32>(0.0);
        for (var i = 0u; i < 3u; i++) {
            let v = c0[i] * slope[i] + offset[i];
            if (clampo) {
                o = vset3f(o, u32(i), powz(clamp(v, 0.0, 1.0), power[i]));
            } else if (v >= 0.0) {
                o = vset3f(o, u32(i), powz(v, power[i]));
            } else {
                o = vset3f(o, u32(i), v);
            }
        }
        let l = o.x * FXL_LUMA.x + o.y * FXL_LUMA.y + o.z * FXL_LUMA.z;
        o = l + sat * (o - l);
        if (clampo) {
            o = clamp(o, zero, one);
        }
        return o;
    }
    var c = c0;
    if (clampo) {
        c = clamp(c, zero, one);
    }
    let l = c.x * FXL_LUMA.x + c.y * FXL_LUMA.y + c.z * FXL_LUMA.z;
    var s = sat;
    if (abs(s) < 1e-9) {
        s = 1e-9;
    }
    let o = l + (c - l) / s;
    var r = vec3<f32>(0.0);
    for (var i = 0u; i < 3u; i++) {
        var v = o[i];
        if (clampo) {
            v = clamp(v, 0.0, 1.0);
        }
        let pw = max(power[i], 1e-9);
        var u = v;
        if (v >= 0.0) {
            u = powz(v, 1.0 / pw);
        }
        var sl = slope[i];
        if (abs(sl) < 1e-9) {
            sl = 1e-9;
        }
        r = vset3f(r, u32(i), (u - offset[i]) / sl);
    }
    if (clampo) {
        r = clamp(r, zero, one);
    }
    return r;
}

fn fxl_tonemap(v: f32, inverse: bool) -> f32 {
    if (!inverse) {
        if (v <= 0.0) {
            return v;
        }
        return v * (1.0 + v / 256.0) / (1.0 + v);
    }
    if (v <= 0.0) {
        return v;
    }
    let a = 1.0 / 256.0;
    let b = 1.0 - v;
    let c = -v;
    return (-b + sqrt(b * b - 4.0 * a * c)) / (2.0 * a);
}

// ExponentWithLinearTransform (ocio_config::moncurve): forward decodes; a gamma of at most 1 or
// a non-positive offset is a plain power curve.
fn fxl_moncurve(v0: f32, g: f32, o: f32, inv: bool, mirror: bool) -> f32 {
    var v = v0;
    var sign = 1.0;
    if (mirror && v < 0.0) {
        v = -v;
        sign = -1.0;
    }
    if (g <= 1.0 || o <= 0.0) {
        var p = g;
        if (inv) {
            p = 1.0 / max(g, 1e-9);
        }
        return sign * powz(max(v, 0.0), p);
    }
    let xb = o / (g - 1.0);
    let yb = powz((xb + o) / (1.0 + o), g);
    let slope = yb / xb;
    if (!inv) {
        if (v >= xb) {
            return sign * powz((v + o) / (1.0 + o), g);
        }
        return sign * v * slope;
    }
    if (v >= yb) {
        return sign * ((1.0 + o) * powz(v, 1.0 / g) - o);
    }
    return sign * v / slope;
}

// Run the colour program at data[0..] on a straight colour.
fn fxl_run(c0: vec3<f32>) -> vec3<f32> {
    var c = c0;
    var pc = 0u;
    for (var guard = 0; guard < 256; guard++) {
        let op = u32(data[pc]);
        switch (op) {
            case 1u, 2u: {
                let tf = u32(data[pc + 1u]);
                let g = data[pc + 2u];
                for (var i = 0u; i < 3u; i++) {
                    if (op == 1u) {
                        c = vset3f(c, u32(i), fxl_decode(tf, g, c[i]));
                    } else {
                        c = vset3f(c, u32(i), fxl_encode(tf, g, c[i]));
                    }
                }
                pc += 3u;
            }
            case 3u: {
                let r0 = fxl_d3(pc + 1u);
                let r1 = fxl_d3(pc + 4u);
                let r2 = fxl_d3(pc + 7u);
                c = vec3<f32>(r0.x * c.x + r0.y * c.y + r0.z * c.z, r1.x * c.x + r1.y * c.y + r1.z * c.z, r2.x * c.x + r2.y * c.y + r2.z * c.z);
                pc += 10u;
            }
            case 4u: {
                c = fxl_cdl(pc + 1u, c);
                pc += 13u;
            }
            case 5u: {
                let inv = data[pc + 1u] != 0.0;
                c = vec3<f32>(fxl_tonemap(c.x, inv), fxl_tonemap(c.y, inv), fxl_tonemap(c.z, inv));
                pc += 2u;
            }
            case 6u: {
                let luma = fxl_d3(pc + 1u);
                let l = c.x * luma.x + c.y * luma.y + c.z * luma.z;
                if (l <= 0.0) {
                    c = max(c, vec3<f32>(0.0));
                } else {
                    let mn = min(min(c.x, c.y), c.z);
                    if (mn < 0.0) {
                        let t = clamp(l / (l - mn), 0.0, 1.0);
                        c = l + (c - l) * t;
                    }
                }
                pc += 4u;
            }
            case 7u: {
                c = max(c, vec3<f32>(0.0));
                pc += 1u;
            }
            case 8u: {
                let o = u32(data[pc + 1u]);
                let interp = u32(data[pc + 2u]);
                let inv = data[pc + 3u] != 0.0;
                let sh = i32(data[pc + 4u]);
                c = fxl_file(o, sh, c, interp, inv);
                pc += 5u;
            }
            case 9u: {
                // Affine: matrix × (c + pre) + post (a config's MatrixTransform, RangeTransform).
                let r0 = fxl_d3(pc + 1u);
                let r1 = fxl_d3(pc + 4u);
                let r2 = fxl_d3(pc + 7u);
                let d = c + fxl_d3(pc + 10u);
                c = vec3<f32>(r0.x * d.x + r0.y * d.y + r0.z * d.z, r1.x * d.x + r1.y * d.y + r1.z * d.z, r2.x * d.x + r2.y * d.y + r2.z * d.z)
                    + fxl_d3(pc + 13u);
                pc += 16u;
            }
            case 10u: {
                // ExponentTransform: max(c, 0) ^ p.
                let e = fxl_d3(pc + 1u);
                c = vec3<f32>(powz(max(c.x, 0.0), e.x), powz(max(c.y, 0.0), e.y), powz(max(c.z, 0.0), e.z));
                pc += 4u;
            }
            case 11u: {
                // Log / LogAffineTransform: base, log slope, log offset, lin slope, lin offset,
                // inverse, ln of f64's smallest normal (the CPU's floor).
                let base = data[pc + 1u];
                let ls = fxl_d3(pc + 2u);
                let lo = fxl_d3(pc + 5u);
                let lis = fxl_d3(pc + 8u);
                let lio = fxl_d3(pc + 11u);
                let lnb = log(base);
                if (data[pc + 14u] != 0.0) {
                    for (var i = 0u; i < 3u; i++) {
                        c = vset3f(c, u32(i), (exp((c[i] - lo[i]) / ls[i] * lnb) - lio[i]) / lis[i]);
                    }
                } else {
                    for (var i = 0u; i < 3u; i++) {
                        let v = lis[i] * c[i] + lio[i];
                        var ln = data[pc + 15u];
                        if (v >= 1.1754944e-38) {
                            ln = log(v);
                        }
                        c = vset3f(c, u32(i), ls[i] * (ln / lnb) + lo[i]);
                    }
                }
                pc += 16u;
            }
            case 12u: {
                // ExponentWithLinearTransform: gamma (3), offset (3), inverse, mirror.
                let g = fxl_d3(pc + 1u);
                let o = fxl_d3(pc + 4u);
                let inv = data[pc + 7u] != 0.0;
                let mirror = data[pc + 8u] != 0.0;
                for (var i = 0u; i < 3u; i++) {
                    c = vset3f(c, u32(i), fxl_moncurve(c[i], g[i], o[i], inv, mirror));
                }
                pc += 9u;
            }
            case 13u: {
                // Clamp (a clamping RangeTransform): low (3), high (3).
                c = min(max(c, fxl_d3(pc + 1u)), fxl_d3(pc + 4u));
                pc += 7u;
            }
            default: {
                return c;
            }
        }
    }
    return c;
}

// u[0].x = 0: OCIO pixels (unpremul); 1: colour ÷ alpha (Image::map_straight).
@compute @workgroup_size(16, 16)
fn fxl_point(@builtin(global_invocation_id) gid: vec3<u32>) {
    let dims = out_dims();
    let p = vec2<i32>(gid.xy);
    if (p.x >= dims.x || p.y >= dims.y) {
        return;
    }
    let px = textureLoad(src, p, 0);
    let a = px.w;
    if (a <= 0.0) {
        textureStore(out, p, px);
        return;
    }
    var c = px.xyz / a;
    if (P.u[0].x == 0u && a <= 1e-6) {
        c = vec3<f32>(0.0);
    }
    textureStore(out, p, vec4<f32>(fxl_run(c) * a, a));
}

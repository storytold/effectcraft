// GPU effects (keying, matte and channel family): see src/fx_key.rs. Each entry point mirrors
// the CPU effect in effectcraft-effects operation for operation. Every name here is prefixed
// `fxk_` (the family files share one module).

// Correct the approximate quotient before bitwise/threshold operations.
fn fxk_div3(a: vec3<f32>, b: f32) -> vec3<f32> {
    let q = a / b;
    let r = fma(-q, vec3<f32>(b), a);
    return fma(r, vec3<f32>(1.0 / b), q);
}

// util::unpremul: (straight colour, alpha); colour 0 when alpha ≤ 1e-6.
fn fxk_straight(px: vec4<f32>) -> vec3<f32> {
    if (px.w > 1e-6) {
        return fxk_div3(px.xyz, px.w);
    }
    return vec3<f32>(0.0);
}

fn fxk_premul(c: vec3<f32>, a: f32) -> vec4<f32> {
    return vec4<f32>(c * a, a);
}

// Pixel `p` of the third image, copied into `data` as RGBA f32 rows of P.u[3].x pixels.
fn fxk_rows(p: vec2<i32>) -> vec4<f32> {
    let i = (u32(p.y) * P.u[3].x + u32(p.x)) * 4u;
    return vec4<f32>(data[i], data[i + 1u], data[i + 2u], data[i + 3u]);
}

// keying::to_lab.
fn fxk_cbrt(t: f32) -> f32 {
    return sign(t) * pow(abs(t), 1.0 / 3.0);
}

fn fxk_lab_f(t: f32) -> f32 {
    if (t > 0.008856) {
        return fxk_cbrt(t);
    }
    return 7.787 * t + 16.0 / 116.0;
}

// keying::range_components.
fn fxk_range_components(space: u32, c: vec3<f32>) -> vec3<f32> {
    if (space == 1u) {
        let y = 0.299 * c.x + 0.587 * c.y + 0.114 * c.z;
        let u = -0.14713 * c.x - 0.28886 * c.y + 0.436 * c.z;
        let v = 0.615 * c.x - 0.51499 * c.y - 0.10001 * c.z;
        return vec3<f32>(y * 255.0, u * 255.0 + 128.0, v * 255.0 + 128.0);
    }
    if (space == 2u) {
        return c * 255.0;
    }
    let cl = clamp(c, vec3<f32>(0.0), vec3<f32>(1.0));
    let l = vec3<f32>(srgb_to_linear(cl.x), srgb_to_linear(cl.y), srgb_to_linear(cl.z));
    let x = (0.4124 * l.x + 0.3576 * l.y + 0.1805 * l.z) / 0.95047;
    let yy = 0.2126 * l.x + 0.7152 * l.y + 0.0722 * l.z;
    let z = (0.0193 * l.x + 0.1192 * l.y + 0.9505 * l.z) / 1.08883;
    let fx = fxk_lab_f(x);
    let fy = fxk_lab_f(yy);
    let fz = fxk_lab_f(z);
    return vec3<f32>((116.0 * fy - 16.0) * 2.55, 500.0 * (fx - fy) + 128.0, 200.0 * (fy - fz) + 128.0);
}

// keying::matte_levels.
// Luma Key / Extract luminance: Rec. 601, effects::LUMA_601 (checked by the test
// shader_constants_match_the_cpu).
fn fxk_key_luma(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.299, 0.587, 0.114));
}

fn fxk_matte_levels(v: f32, in_b: f32, in_w: f32, gamma: f32, out_b: f32, out_w: f32) -> f32 {
    var t = clamp((v - in_b / 255.0) / max((in_w - in_b) / 255.0, 1e-4), 0.0, 1.0);
    t = powz(t, 1.0 / max(gamma, 0.01));
    return clamp(out_b / 255.0 + t * (out_w - out_b) / 255.0, 0.0, 1.0);
}

// keying::secondary.
fn fxk_secondary(c: vec3<f32>, o1: u32, o2: u32, bal: f32) -> f32 {
    var hi = c[o2];
    var lo = c[o1];
    if (c[o1] > c[o2]) {
        hi = c[o1];
        lo = c[o2];
    }
    return bal * hi + (1.0 - bal) * lo;
}

// channel::arithmetic's 8-bit quantisation.
fn fxk_q(v: f32) -> u32 {
    return u32(round_away(clamp(v, 0.0, 1.0) * 255.0));
}

// channel2::extract.
fn fxk_extract(px: vec4<f32>, ch: u32, inv: bool) -> vec4<f32> {
    let c = fxk_straight(px);
    let a = max(px.w, 0.0);
    var g: f32;
    switch ch {
        case 0u: {
            if (inv) {
                return fxk_premul(1.0 - c, a);
            }
            return px;
        }
        case 1u: { g = luminance(c) * a; }
        case 2u: { g = c.x * a; }
        case 3u: { g = c.y * a; }
        case 4u: { g = c.z * a; }
        default: { g = a; }
    }
    if (inv) {
        g = 1.0 - g;
    }
    return vec4<f32>(g, g, g, 1.0);
}

// channel2::arith.
fn fxk_arith(op: u32, a: f32, b: f32) -> f32 {
    switch op {
        case 0u: { return b; }
        case 1u: { return a + b; }
        case 2u: { return a - b; }
        case 3u: { return a * b; }
        case 4u: { return abs(a - b); }
        case 5u: { return f32(fxk_q(a) & fxk_q(b)) / 255.0; }
        case 6u: { return f32(fxk_q(a) | fxk_q(b)) / 255.0; }
        case 7u: { return f32(fxk_q(a) ^ fxk_q(b)) / 255.0; }
        case 8u, 11u: { return max(a, b); }
        case 9u, 10u: { return min(a, b); }
        case 12u: { return a + b - a * b; }
        case 13u: {
            if (a < 0.5) {
                return 2.0 * a * b;
            }
            return 1.0 - 2.0 * (1.0 - a) * (1.0 - b);
        }
        default: {
            if (b < 0.5) {
                return 2.0 * a * b;
            }
            return 1.0 - 2.0 * (1.0 - a) * (1.0 - b);
        }
    }
}

// channel2::overflow.
fn fxk_overflow(op: u32, v: f32, how: u32) -> f32 {
    if (how == 0u) {
        return clamp(v, 0.0, 1.0);
    }
    if (how == 1u) {
        let m = 1.0001;
        let r = v - m * floor(v / m);
        return min(select(r, 0.0, r >= m || r < 0.0), 1.0);
    }
    if (op == 1u) {
        return v * 0.5;
    }
    if (op == 2u) {
        return (v + 1.0) * 0.5;
    }
    return clamp(v, 0.0, 1.0);
}

// Per-pixel keying / matte / channel operations. u[0].x = op (see src/fx_key.rs for each
// effect's parameter layout):
//  1 Color Key matte      2 Luma Key matte      3 Color Range matte   4 Extract matte
//  5 apply a matte (aux.x; u[0].y = 0 final, 2 matte view, 3 Difference Matte's matte view,
//    4 Difference Matte's output)
//  6 Difference Matte matte (src and aux blurred)
//  7 Spill Suppressor     8 Advanced Spill Suppressor (aux = 1×1 excess sums when u[1].w)
//  9 Color Difference Key 10 Unmult             11 route channels (u[1] = sources)
// 12 Remove Color Matting 13 Arithmetic         14 Solid Composite    15 Set Matte
// 16 Channel Combiner     17 Blend              18 Calculations       19 Compound Arithmetic
// 20 Set Channels: channel u[0].y of the value image from aux; 21 Set Channels: premultiply
// 22 Screen Key matte     23 Screen Key output  24 alpha plane        25 Key Cleaner output
// 26 Matte Choker ramp    27 util::set_alpha (aux.x = alpha, data = fill)
// 28 matte view (alpha as opaque grey)
// Refine Soft / Hard Matte: 29 guide pack (luminance, alpha, products); 30 guided-filter
// coefficients from the box means (f[0].x = eps); 31 refined alpha (src = boxed coefficients,
// aux = pack, data = edge band); 32 edge band (src.y − aux.y > 1e-3); 33 choke / contrast /
// invert; 34 View Edge Region; 35 decontamination (aux.x = refined alpha, data = colour estimate)
@compute @workgroup_size(16, 16)
fn fxk_point(@builtin(global_invocation_id) gid: vec3<u32>) {
    let dims = out_dims();
    let p = vec2<i32>(gid.xy);
    if (p.x >= dims.x || p.y >= dims.y) {
        return;
    }
    let px = textureLoad(src, p, 0);
    let c = fxk_straight(px);
    let a = max(px.w, 0.0);
    let u0 = P.u[0];
    let u1 = P.u[1];
    let f0 = P.f[0];
    var o = px;
    switch u0.x {
        case 1u: {
            let k = f0.xyz;
            let d = max(max(abs(c.x - k.x), abs(c.y - k.y)), abs(c.z - k.z));
            o = vec4<f32>(select(1.0, 0.0, d <= f0.w));
        }
        case 2u: {
            let l = fxk_key_luma(c);
            let thr = f0.x;
            let tol = f0.y;
            var m: f32;
            // As effects::keying::luma_key: Brighter / Darker cut hard at the threshold
            // (Tolerance only shapes Similar / Dissimilar), as After Effects.
            switch u0.y {
                case 0u: { m = select(1.0, 0.0, l > thr); }
                case 2u: { m = select(1.0, 0.0, abs(l - thr) <= tol); }
                case 3u: { m = select(0.0, 1.0, abs(l - thr) <= tol); }
                default: { m = select(1.0, 0.0, l < thr); }
            }
            o = vec4<f32>(m);
        }
        case 3u: {
            let v = fxk_range_components(u0.y, c);
            var out_d = 0.0;
            for (var i = 0u; i < 3u; i++) {
                out_d = max(max(out_d, P.f[0][i] - v[i]), v[i] - P.f[1][i]);
            }
            let fuzz = P.f[2].x;
            var m = 1.0;
            if (out_d <= 0.0) {
                m = 0.0;
            } else if (fuzz > 0.0) {
                m = clamp(out_d / fuzz, 0.0, 1.0);
            }
            o = vec4<f32>(m);
        }
        case 4u: {
            var v: f32;
            switch u0.y {
                case 1u: { v = c.x; }
                case 2u: { v = c.y; }
                case 3u: { v = c.z; }
                case 4u: { v = a; }
                default: { v = fxk_key_luma(c); }
            }
            v = 255.0 * v;
            let bp = f0.x;
            let wp = f0.y;
            let bs = f0.z;
            let ws = f0.w;
            var k = 1.0;
            if (v < bp) {
                k = select(0.0, clamp(1.0 - (bp - v) / bs, 0.0, 1.0), bs > 0.0);
            } else if (v > wp) {
                k = select(0.0, clamp(1.0 - (v - wp) / ws, 0.0, 1.0), ws > 0.0);
            }
            if (u0.z != 0u) {
                k = 1.0 - k;
            }
            o = vec4<f32>(k);
        }
        case 5u: {
            let m = textureLoad(aux, p, 0).x;
            switch u0.y {
                case 2u: {
                    let k = clamp(m, 0.0, 1.0);
                    let v = px.w * k;
                    o = vec4<f32>(v, v, v, 1.0);
                }
                case 3u: {
                    let v = m * px.w;
                    o = vec4<f32>(v, v, v, px.w);
                }
                case 4u: { o = px * m; }
                default: { o = px * clamp(m, 0.0, 1.0); }
            }
        }
        case 6u: {
            let q = textureLoad(aux, p, 0);
            let oc = fxk_straight(q);
            let oa = max(q.w, 0.0);
            let d = max(max(max(abs(c.x - oc.x), abs(c.y - oc.y)), abs(c.z - oc.z)), abs(a - oa));
            var m: f32;
            if (d <= f0.x) {
                m = 0.0;
            } else if (f0.y <= 1e-6) {
                m = 1.0;
            } else {
                m = clamp((d - f0.x) / f0.y, 0.0, 1.0);
            }
            o = vec4<f32>(m);
        }
        case 7u: {
            if (px.w <= 0.0) {
                textureStore(out, p, px);
                return;
            }
            var s = px.xyz / px.w;
            if (u0.y == 1u) {
                let g = (s.x + s.y + s.z) / 3.0;
                let dir = f0.xyz;
                let proj = (s.x - g) * dir.x + (s.y - g) * dir.y + (s.z - g) * dir.z;
                if (proj > 0.0) {
                    s = s - proj * dir * f0.w;
                }
            } else {
                let spill = max(s[u1.x] - max(s[u1.y], s[u1.z]), 0.0);
                s[u1.x] = s[u1.x] - spill * f0.w;
            }
            o = vec4<f32>(s * px.w, px.w);
        }
        case 8u: {
            if (px.w <= 0.0) {
                textureStore(out, p, px);
                return;
            }
            var pi = u1.x;
            var o1 = u1.y;
            var o2 = u1.z;
            if (u1.w != 0u) {
                let ex = textureLoad(aux, vec2<i32>(0, 0), 0);
                if (ex.y >= ex.x && ex.y >= ex.z) {
                    pi = 1u;
                    o1 = 0u;
                    o2 = 2u;
                } else if (ex.z >= ex.x) {
                    pi = 2u;
                    o1 = 0u;
                    o2 = 1u;
                } else {
                    pi = 0u;
                    o1 = 1u;
                    o2 = 2u;
                }
            }
            var s = px.xyz / px.w;
            let amt = f0.x;
            let spill_range = f0.y;
            let luma = f0.z;
            let tol = f0.w;
            let desat = P.f[1].x;
            let neutral = P.f[1].y;
            let key_hue = P.f[1].z;
            var w = 1.0;
            if (tol < 1.0) {
                let h = rgb_to_hsl(s).x;
                let d0 = abs(h - key_hue);
                let d = min(d0, 1.0 - d0);
                w = clamp(1.0 - d / max(tol * 0.5, 1e-4), 0.0, 1.0);
            }
            let mx = max(s[o1], s[o2]);
            let avg = 0.5 * (s[o1] + s[o2]);
            let lim = mx + (avg - mx) * spill_range;
            let spill = max(s[pi] - lim, 0.0) * amt * w;
            s[pi] = s[pi] - spill;
            let lw = vec3<f32>(0.2126, 0.7152, 0.0722);
            let back = spill * lw[pi] * luma + spill * neutral / 3.0;
            s = s + back;
            if (desat > 0.0 && spill > 0.0) {
                let l = luminance(s);
                let k = min(desat * min(spill * 4.0, 1.0), 1.0);
                s = s + (vec3<f32>(l) - s) * k;
            }
            o = vec4<f32>(s * px.w, px.w);
        }
        case 9u: {
            var cc = c;
            if (u0.z != 0u) {
                let cl = clamp(cc, vec3<f32>(0.0), vec3<f32>(1.0));
                cc = vec3<f32>(srgb_to_linear(cl.x), srgb_to_linear(cl.y), srgb_to_linear(cl.z));
            }
            let pa = P.f[1];
            let pb = P.f[3];
            let ua = 1.0 - clamp((cc[u1.x] - cc[u1.y]) / f0.x, 0.0, 1.0);
            let ub = 1.0 - clamp((cc[u1.x] - cc[u1.z]) / f0.y, 0.0, 1.0);
            let ca = fxk_matte_levels(ua, pa.x, pa.y, pa.z, pa.w, P.f[2].x);
            let cb = fxk_matte_levels(ub, pb.x, pb.y, pb.z, pb.w, P.f[2].y);
            let um = max(ua, ub);
            let cm = fxk_matte_levels(max(ca, cb), f0.z, f0.w, P.f[2].z, 0.0, 255.0);
            var view = u0.y;
            if (view == 8u) {
                let right = p.x * 2 >= dims.x;
                let bottom = p.y * 2 >= dims.y;
                if (!right && !bottom) {
                    view = 2u;
                } else if (right && !bottom) {
                    view = 4u;
                } else if (!right && bottom) {
                    view = 6u;
                } else {
                    view = 7u;
                }
            }
            var g = -1.0;
            switch view {
                case 1u: { g = ua; }
                case 2u: { g = ca; }
                case 3u: { g = ub; }
                case 4u: { g = cb; }
                case 5u: { g = um; }
                case 6u: { g = cm; }
                default: {}
            }
            if (g >= 0.0) {
                let m = g * a;
                o = vec4<f32>(m, m, m, 1.0);
            } else {
                o = px * cm;
            }
        }
        case 10u: {
            let white = u0.y != 0u;
            var d: f32;
            var start: f32;
            if (white) {
                d = 1.0 - min(min(c.x, c.y), c.z);
                start = 1.0 - f0.x;
            } else {
                d = max(max(c.x, c.y), c.z);
                start = f0.x;
            }
            d = clamp(d, 0.0, 1.0);
            let width = f0.y * (1.0 - start);
            var m: f32;
            if (width > 1e-6) {
                m = clamp((d - start) / width, 0.0, 1.0);
            } else {
                m = select(0.0, 1.0, d > start);
            }
            if (m <= 1e-6) {
                o = vec4<f32>(0.0);
            } else {
                let bg = select(0.0, 1.0, white);
                var oc = c;
                if (u0.z != 0u) {
                    oc = (c - bg * (1.0 - m)) / m;
                }
                if (u0.w != 0u) {
                    oc = clamp(oc, vec3<f32>(0.0), vec3<f32>(1.0));
                }
                o = fxk_premul(oc, a * m);
            }
        }
        case 11u: {
            let v = vec4<f32>(dst_pick(u1.x, c, a), dst_pick(u1.y, c, a), dst_pick(u1.z, c, a), dst_pick(u1.w, c, a));
            o = fxk_premul(v.xyz, clamp(v.w, 0.0, 1.0));
        }
        case 12u: {
            if (px.w <= 1e-6) {
                o = vec4<f32>(0.0);
            } else {
                for (var i = 0; i < 3; i++) {
                    let s = px[i] / px.w;
                    var v = max(s - f0[i] * (1.0 - px.w), 0.0);
                    if (u0.y != 0u) {
                        v = min(v, px.w);
                    }
                    o[i] = v;
                }
            }
        }
        case 13u: {
            if (px.w <= 0.0) {
                textureStore(out, p, px);
                return;
            }
            let s = fxk_div3(px.xyz, px.w);
            var r = vec3<f32>(0.0);
            for (var i = 0u; i < 3u; i++) {
                let v = s[i];
                let kv = f0[i];
                let iv = fxk_q(v);
                let ik = u32(round_away(kv * 255.0));
                var x: f32;
                switch u0.y {
                    case 0u: { x = f32(iv & ik) / 255.0; }
                    case 1u: { x = f32(iv | ik) / 255.0; }
                    case 2u: { x = f32(iv ^ ik) / 255.0; }
                    case 3u: { x = v + kv; }
                    case 4u: { x = v - kv; }
                    case 5u: { x = abs(v - kv); }
                    case 6u: { x = min(v, kv); }
                    case 7u: { x = max(v, kv); }
                    case 8u: { x = select(v, 0.0, v > kv); }
                    case 9u: { x = select(v, 0.0, v < kv); }
                    case 10u: { x = select(0.0, 1.0, v >= kv); }
                    case 11u: { x = v * kv; }
                    default: { x = 1.0 - (1.0 - v) * (1.0 - kv); }
                }
                if (u0.z != 0u) {
                    r[i] = clamp(x, 0.0, 1.0);
                } else {
                    r[i] = max(x, 0.0);
                }
            }
            o = vec4<f32>(r * px.w, px.w);
        }
        case 14u: {
            var b = blend_pixel(u0.y, f0, px * P.f[1].x, 0.5);
            b.w = clamp(b.w, 0.0, 1.0);
            o = vec4<f32>(max(b.xyz, vec3<f32>(0.0)), b.w);
        }
        case 15u: {
            var mc = c;
            var ma = a;
            if ((u0.z & 8u) == 0u) {
                let q = textureLoad(aux, p, 0);
                mc = fxk_straight(q);
                ma = max(q.w, 0.0);
            }
            var m = dst_pick(u0.y, mc, ma);
            if ((u0.z & 4u) != 0u) {
                m = m * ma;
            }
            m = clamp(m, 0.0, 1.0);
            let inv = (u0.z & 1u) != 0u;
            if (inv) {
                m = 1.0 - m;
            }
            let own_m = (u0.z & 8u) != 0u;
            let is_alpha = (u0.z & 16u) != 0u;
            var na = m;
            if ((u0.z & 2u) != 0u && (!own_m || !is_alpha || inv)) {
                na = a * m;
            }
            o = fxk_premul(c, clamp(na, 0.0, 1.0));
        }
        case 16u: {
            let ch_from = u0.y;
            let ch_to = u0.z;
            let inv = (u0.w & 1u) != 0u;
            let solid = (u0.w & 2u) != 0u;
            let second = (u0.w & 4u) != 0u;
            var sc = c;
            var sa = a;
            if (second) {
                let q = textureLoad(aux, p, 0);
                sc = fxk_straight(q);
                sa = max(q.w, 0.0);
            }
            if (sa <= 0.0 && ch_from != 7u && !second) {
                if (solid) {
                    o = fxk_premul(c, 1.0);
                }
                textureStore(out, p, o);
                return;
            }
            var na = a;
            var r: vec3<f32>;
            if (ch_from <= 3u) {
                switch ch_from {
                    case 0u: {
                        let hsl = rgb_to_hsl(sc);
                        r = vec3<f32>(hsl.x, hsl.z, hsl.y);
                    }
                    case 1u: { r = hsl_to_rgb(sc.x, sc.z, sc.y); }
                    case 2u: {
                        let y = 0.299 * sc.x + 0.587 * sc.y + 0.114 * sc.z;
                        r = vec3<f32>(y, 0.492 * (sc.z - y) + 0.5, 0.877 * (sc.x - y) + 0.5);
                    }
                    default: {
                        let y = sc.x;
                        let uu = sc.y - 0.5;
                        let vv = sc.z - 0.5;
                        let rr = y + vv / 0.877;
                        let bb = y + uu / 0.492;
                        let gg = (y - 0.299 * rr - 0.114 * bb) / 0.587;
                        r = vec3<f32>(rr, gg, bb);
                    }
                }
                if (inv) {
                    r = 1.0 - r;
                }
            } else {
                var v: f32;
                switch ch_from {
                    case 4u: { v = sc.x; }
                    case 5u: { v = sc.y; }
                    case 6u: { v = sc.z; }
                    case 7u: { v = sa; }
                    case 8u: { v = rgb_to_hsl(sc).z; }
                    case 9u: { v = rgb_to_hsl(sc).x; }
                    case 10u: { v = rgb_to_hsl(sc).y; }
                    case 11u: { v = luminance(sc); }
                    case 12u: { v = max(max(sc.x, sc.y), sc.z); }
                    default: { v = min(min(sc.x, sc.y), sc.z); }
                }
                if (inv) {
                    v = 1.0 - v;
                }
                switch ch_to {
                    case 0u: { r = vec3<f32>(v, 0.0, 0.0); }
                    case 1u: { r = vec3<f32>(0.0, v, 0.0); }
                    case 2u: { r = vec3<f32>(0.0, 0.0, v); }
                    case 3u: {
                        na = clamp(v, 0.0, 1.0);
                        r = vec3<f32>(1.0);
                    }
                    case 5u: { r = hsl_to_rgb(v, 1.0, 0.5); }
                    case 6u: { r = hsl_to_rgb(0.0, clamp(v, 0.0, 1.0), 0.5); }
                    case 8u: {
                        let hsl = rgb_to_hsl(c);
                        r = hsl_to_rgb(hsl.x, clamp(hsl.y * v, 0.0, 1.0), hsl.z);
                    }
                    default: { r = vec3<f32>(v); }
                }
            }
            if (solid) {
                na = 1.0;
            }
            o = fxk_premul(max(r, vec3<f32>(0.0)), na);
        }
        case 17u: {
            let q = textureLoad(aux, p, 0);
            var bl: vec4<f32>;
            switch u0.y {
                case 0u: { bl = q; }
                case 1u, 2u: {
                    let oa = q.w;
                    if (px.w <= 0.0 || oa <= 0.0) {
                        bl = px;
                    } else {
                        let hsl = rgb_to_hsl(c);
                        let ohsl = rgb_to_hsl(fxk_straight(q));
                        var h2 = ohsl.x;
                        var s2 = ohsl.y;
                        if (u0.y == 2u) {
                            if (hsl.y > 1e-4) {
                                s2 = hsl.y;
                            } else {
                                h2 = 0.0;
                                s2 = 0.0;
                            }
                        }
                        let rgb = hsl_to_rgb(h2, s2, hsl.z);
                        bl = px + (fxk_premul(rgb, a) - px) * oa;
                    }
                }
                case 3u: { bl = vec4<f32>(min(px.xyz, q.xyz), px.w); }
                default: { bl = max(px, q); }
            }
            o = bl + (px - bl) * f0.x;
        }
        case 18u: {
            let flags = u1.x;
            var r = fxk_extract(px, u0.y, (flags & 1u) != 0u);
            if ((flags & 8u) != 0u) {
                let s = fxk_extract(textureLoad(aux, p, 0), u0.z, (flags & 2u) != 0u) * f0.x;
                r = blend_pixel(u0.w, r, s, 0.5);
            }
            if ((flags & 4u) != 0u && r.w > 1e-6) {
                r = fxk_premul(fxk_straight(r), px.w);
            }
            r.w = clamp(r.w, 0.0, 1.0);
            o = r;
        }
        case 19u: {
            let q = textureLoad(aux, p, 0);
            let oc = fxk_straight(q);
            let oa = max(q.w, 0.0);
            var rc = c;
            var ra = a;
            if (u0.z != 2u) {
                for (var i = 0u; i < 3u; i++) {
                    rc[i] = fxk_overflow(u0.y, fxk_arith(u0.y, c[i], oc[i]), u0.w);
                }
            }
            if (u0.z != 0u) {
                ra = fxk_overflow(u0.y, fxk_arith(u0.y, a, oa), u0.w);
            }
            let res = fxk_premul(rc, ra);
            o = res + (px - res) * f0.x;
        }
        case 20u: {
            let q = textureLoad(aux, p, 0);
            o = px;
            o[u0.y] = dst_pick(u0.z, fxk_straight(q), max(q.w, 0.0));
        }
        case 21u: {
            o = fxk_premul(px.xyz, clamp(px.w, 0.0, 1.0));
        }
        case 22u: {
            let d = c[u1.x] - fxk_secondary(c, u1.y, u1.z, f0.z);
            let t = clamp(d / f0.x * f0.y, 0.0, 1.0);
            o = vec4<f32>(clamp((1.0 - t - f0.w) / max(P.f[1].x - f0.w, 1e-3), 0.0, 1.0));
        }
        case 23u: {
            let k = clamp(textureLoad(aux, p, 0).x, 0.0, 1.0);
            let alpha = a * k;
            if (u0.y == 1u) {
                o = vec4<f32>(alpha, alpha, alpha, 1.0);
            } else if (u0.y == 2u) {
                var v = 0.5;
                if (alpha <= 1e-3) {
                    v = 0.0;
                } else if (alpha >= 0.999) {
                    v = 1.0;
                }
                o = vec4<f32>(v, v, v, 1.0);
            } else {
                let screen = f0.xyz;
                var fg = c;
                if (!(u0.z != 0u || k <= 1e-4)) {
                    let s = 1.0 - k;
                    for (var i = 0; i < 3; i++) {
                        fg[i] = min(max(c[i] - s * screen[i], 0.0) / k, max(c[i], 1.0));
                    }
                }
                let spill = max(fg[u1.x] - fxk_secondary(fg, u1.y, u1.z, f0.w), 0.0);
                fg[u1.x] = fg[u1.x] - spill * P.f[1].x;
                o = fxk_premul(fg, alpha);
            }
        }
        case 24u: {
            o = vec4<f32>(px.w);
        }
        case 25u: {
            var na = textureLoad(aux, p, 0).x;
            if (u0.y != 0u) {
                na = clamp((na - 0.5) * 1.5 + 0.5, 0.0, 1.0);
            }
            var cc = c;
            let a0 = a;
            if (P.u[3].y != 0u && a0 > 1e-4 && a0 < 0.999) {
                let q = fxk_rows(p);
                if (q.w > 1e-6) {
                    let ea = q.w;
                    if (ea > 1e-4) {
                        let t = f0.y * (1.0 - a0);
                        cc = cc + (q.xyz / ea - cc) * t;
                    }
                }
            }
            na = clamp((na - 0.5) * f0.x + 0.5, 0.0, 1.0);
            if (a0 <= 1e-6) {
                na = 0.0;
            }
            o = fxk_premul(cc, na);
        }
        case 26u: {
            o = vec4<f32>(clamp((px.x - f0.x) / f0.y + 0.5, 0.0, 1.0));
        }
        case 27u: {
            let na = clamp(textureLoad(aux, p, 0).x, 0.0, 1.0);
            var cc = c;
            if (px.w <= 1e-4) {
                cc = fxk_straight(fxk_rows(p));
            }
            o = fxk_premul(cc, na);
        }
        case 29u: {
            let g = luminance(px.xyz);
            o = vec4<f32>(g, px.w, g * px.w, g * g);
        }
        case 30u: {
            let var_g = px.w - px.x * px.x;
            let cov = px.z - px.x * px.y;
            let ca = cov / (var_g + f0.x);
            o = vec4<f32>(ca, px.y - ca * px.x, 0.0, 0.0);
        }
        case 31u: {
            let q = textureLoad(aux, p, 0);
            if (fxk_rows(p).x != 0.0) {
                o = vec4<f32>(clamp(px.x * q.x + px.y, 0.0, 1.0));
            } else {
                o = vec4<f32>(q.y);
            }
        }
        case 32u: {
            o = vec4<f32>(select(0.0, 1.0, px.y - textureLoad(aux, p, 0).y > 1e-3));
        }
        case 33u: {
            var v = px.x;
            let choke = f0.x;
            if (choke > 0.0) {
                v = (v - choke) / (1.0 - choke);
            } else if (choke < 0.0) {
                v = v / (1.0 + choke);
            }
            if (abs(f0.y - 1.0) > 1e-6) {
                v = (v - 0.5) * f0.y + 0.5;
            }
            v = clamp(v, 0.0, 1.0);
            if (u0.y != 0u) {
                v = 1.0 - v;
            }
            o = vec4<f32>(v);
        }
        case 34u: {
            let v = px.x;
            if (textureLoad(aux, p, 0).x != 0.0) {
                o = vec4<f32>(0.5 + 0.5 * v, 0.5 * v, 0.5 * v, 1.0);
            } else {
                o = vec4<f32>(v, v, v, 1.0);
            }
        }
        case 35u: {
            let n = textureLoad(aux, p, 0).x;
            let a0 = px.w;
            var t = 0.0;
            if (a0 > 1e-4 && a0 < 0.999) {
                t = 1.0 - a0;
            } else if (u0.w != 0u && n > 1e-4 && n < 0.999) {
                t = 1.0 - n;
            }
            let strength = f0.x * t;
            if (u0.y != 0u) {
                let v = select(0.0, strength, u0.z != 0u);
                o = vec4<f32>(v, v, v, 1.0);
            } else if (P.u[3].y != 0u && strength > 0.0 && a0 > 1e-4) {
                let q = fxk_rows(p);
                if (q.w > 1e-4) {
                    let cc = c + (q.xyz / q.w - c) * strength;
                    o = fxk_premul(cc, a0);
                }
            }
        }
        case 28u: {
            let v = clamp(px.w, 0.0, 1.0);
            o = vec4<f32>(v, v, v, 1.0);
        }
        default: {}
    }
    textureStore(out, p, o);
}

// util::fit_layer: bilinear sample of the other layer (src) at the buffer coordinates in data
// (one per column, then one per row).
@compute @workgroup_size(16, 16)
fn fxk_fit(@builtin(global_invocation_id) gid: vec3<u32>) {
    let dims = out_dims();
    let p = vec2<i32>(gid.xy);
    if (p.x >= dims.x || p.y >= dims.y) {
        return;
    }
    let x = data[u32(p.x)];
    let y = data[u32(dims.x) + u32(p.y)];
    textureStore(out, p, sample_bilinear(src, x, y));
}

// Sum of 16×16 blocks; u[0].x = first pass: sum Advanced Spill Suppressor's per-channel excess
// of each primary over the other two (straight colour × alpha), else sum the values.
@compute @workgroup_size(16, 16)
fn fxk_reduce(@builtin(global_invocation_id) gid: vec3<u32>) {
    let dims = out_dims();
    let p = vec2<i32>(gid.xy);
    if (p.x >= dims.x || p.y >= dims.y) {
        return;
    }
    let sd = vec2<i32>(textureDimensions(src));
    var acc = vec4<f32>(0.0);
    for (var j = 0; j < 16; j++) {
        for (var i = 0; i < 16; i++) {
            let q = vec2<i32>(p.x * 16 + i, p.y * 16 + j);
            if (q.x >= sd.x || q.y >= sd.y) {
                continue;
            }
            let v = textureLoad(src, q, 0);
            if (P.u[0].x != 0u) {
                let c = fxk_straight(v);
                let a = max(v.w, 0.0);
                acc += vec4<f32>(
                    max(c.x - max(c.y, c.z), 0.0) * a,
                    max(c.y - max(c.x, c.z), 0.0) * a,
                    max(c.z - max(c.x, c.y), 0.0) * a,
                    0.0,
                );
            } else {
                acc += v;
            }
        }
    }
    textureStore(out, p, acc);
}

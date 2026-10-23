// GPU effects (tonal and colour correction family): see src/fx_tone.rs. Each operation mirrors
// the CPU effect in effectcraft-effects operation for operation. Every name here is prefixed
// `fxt_` (the family files share one module).

fn fxt_straight(px: vec4<f32>) -> vec3<f32> {
    if (px.w > 1e-6) {
        return px.xyz / px.w;
    }
    return vec3<f32>(0.0);
}

fn fxt_mix(a: vec3<f32>, b: vec3<f32>, t: f32) -> vec3<f32> {
    return a + (b - a) * t;
}

// f32::rem_euclid(360.0).
fn fxt_rem360(x: f32) -> f32 {
    let r = x - 360.0 * floor(x / 360.0);
    return select(r, 0.0, r >= 360.0);
}

// Levels (Individual Controls) channel `i` (data: 5 × (ib, iw, g, ob, ow, identity)).
fn fxt_lv(i: u32, v: f32, clip_b: bool, clip_w: bool) -> f32 {
    let o = i * 6u;
    if (data[o + 5u] != 0.0) {
        return v;
    }
    var t = (v - data[o]) / max(data[o + 1u] - data[o], 1e-6);
    if (clip_b) {
        t = max(t, 0.0);
    }
    if (clip_w) {
        t = min(t, 1.0);
    }
    return data[o + 3u] + (data[o + 4u] - data[o + 3u]) * powz(max(t, 0.0), 1.0 / max(data[o + 2u], 0.01));
}

// color_fx::level (data offset o: ib, iw, g, ob, ow, clip black, clip white).
fn fxt_level(o: u32, v: f32) -> f32 {
    var t = (v - data[o]) / max(data[o + 1u] - data[o], 1e-6);
    if (data[o + 5u] != 0.0) {
        t = max(t, 0.0);
    }
    if (data[o + 6u] != 0.0) {
        t = min(t, 1.0);
    }
    t = powz(max(t, 0.0), 1.0 / data[o + 2u]);
    return data[o + 3u] + (data[o + 4u] - data[o + 3u]) * t;
}

fn fxt_level_ident(o: u32) -> bool {
    return data[o] == 0.0 && data[o + 1u] == 1.0 && data[o + 2u] == 1.0 && data[o + 3u] == 0.0 && data[o + 4u] == 1.0;
}

fn fxt_adjust_sat(s: f32, sat: f32) -> f32 {
    if (sat >= 0.0) {
        return s + (1.0 - s) * sat * min(s, 1.0);
    }
    return s * (1.0 + sat);
}

fn fxt_adjust_light(l: f32, light: f32) -> f32 {
    if (light >= 0.0) {
        return l + (1.0 - l) * light;
    }
    return l * (1.0 + light);
}

// HueRange::weight for range `r` (data: start, end, fall start, fall end, hue, sat, light).
fn fxt_range_weight(r: u32, h: f32) -> f32 {
    let o = r * 7u;
    let width = fxt_rem360(data[o + 1u] - data[o]);
    let d = fxt_rem360(h - data[o]);
    if (d <= width) {
        return 1.0;
    }
    let after = d - width;
    let before = 360.0 - d;
    var wa = 0.0;
    if (data[o + 3u] > 0.0) {
        wa = 1.0 - after / data[o + 3u];
    }
    var wb = 0.0;
    if (data[o + 2u] > 0.0) {
        wb = 1.0 - before / data[o + 2u];
    }
    return clamp(max(wa, wb), 0.0, 1.0);
}

// color3::colour_match.
fn fxt_colour_match(c: vec3<f32>, key: vec3<f32>, key_hue: f32, mode: u32, tol: f32, soft: f32) -> f32 {
    var d: f32;
    if (mode == 0u) {
        d = sqrt(dot(c - key, c - key)) / sqrt(3.0);
    } else if (mode == 1u) {
        let hs = rgb_to_hsl(c);
        let dh0 = abs(hs.x - key_hue);
        let dh = min(dh0, 1.0 - dh0) * 2.0;
        d = select(dh, 1.0, hs.y < 0.02);
    } else {
        let sa = max(c.x + c.y + c.z, 1e-6);
        let sk = max(key.x + key.y + key.z, 1e-6);
        let a = vec2<f32>(c.x / sa, c.y / sa);
        let k = vec2<f32>(key.x / sk, key.y / sk);
        d = sqrt(dot(a - k, a - k)) * 1.5;
    }
    if (d <= tol) {
        return 1.0;
    }
    if (soft <= 1e-6) {
        return 0.0;
    }
    return clamp(1.0 - (d - tol) / soft, 0.0, 1.0);
}

// color3::knee.
fn fxt_knee(v: f32, start: f32, limit: f32) -> f32 {
    if (v <= start || limit <= start) {
        return min(v, limit);
    }
    let r = limit - start;
    return start + r * (1.0 - exp(-(v - start) / r));
}

// PS Arbitrary Map lookup (map in data, n entries; f[0].x = phase offset in entries).
fn fxt_lookup(v: f32, n: u32) -> f32 {
    let nf = f32(n);
    var x = clamp(v, 0.0, 1.0) * (nf - 1.0) + P.f[0].x;
    x = x - nf * floor(x / nf);
    let fl = floor(x);
    let i0 = u32(fl) % n;
    let i1 = select(i0, i0 + 1u, i0 + 1u < n);
    let t = x - fl;
    return data[i0] + (data[i1] - data[i0]) * t;
}

fn fxt_eq_look(table: u32, n: u32, v: f32) -> f32 {
    let i = min(u32(round_away(clamp(v, 0.0, 1.0) * f32(n - 1u))), n - 1u);
    return data[table * n + i];
}

// Per-pixel colour corrections. u[0].x = op (layouts in src/fx_tone.rs):
//  1 Levels (Individual Controls)  2 Gamma/Pedestal/Gain  3 Photo Filter  4 Change to Color
//  5 Leave Color  6 Change Color  7 Broadcast Colors  8 Color Balance (HLS)  9 CC Toner
// 10 CC Color Offset  11 CC Kernel  12 PS Arbitrary Map  13 Video Limiter
// 14 Hue/Saturation with colour ranges  15 Levels with channel controls
// 16 Auto Levels / Contrast / Color apply  17 Equalize apply  18 Invert (HLS / YIQ channels)
// 19 luminance plane  20 Shadow/Highlight (aux = (shadow base, highlight base))
// 21 pack src.x, aux.x  22 Shadow/Highlight clip remap  23 CC Color Neutralizer
// 24 Color Stabilizer (data: per channel n, then 5 × (x, y))
@compute @workgroup_size(16, 16)
fn fxt_point(@builtin(global_invocation_id) gid: vec3<u32>) {
    let dims = out_dims();
    let p = vec2<i32>(gid.xy);
    if (p.x >= dims.x || p.y >= dims.y) {
        return;
    }
    let px = textureLoad(src, p, 0);
    let a = px.w;
    let u0 = P.u[0];
    let f0 = P.f[0];
    // Image::map_straight: transparent pixels stay as they are.
    let straight_skip = a <= 0.0;
    let c = fxt_straight(px);
    var o = px;
    switch u0.x {
        case 1u: {
            let cb = u0.y != 0u;
            let cw = u0.z != 0u;
            // color2::map_ca.
            if (a <= 0.0 && px.x == 0.0 && px.y == 0.0 && px.z == 0.0) {
                let na = clamp(fxt_lv(4u, 0.0, cb, cw), 0.0, 1.0);
                if (na > 0.0) {
                    o = vec4<f32>(0.0, 0.0, 0.0, clamp(na, 0.0, 1.0));
                }
            } else {
                let aa = max(a, 0.0);
                var nc: vec3<f32>;
                for (var i = 0u; i < 3u; i++) {
                    nc = vset3f(nc, u32(i), fxt_lv(i + 1u, fxt_lv(0u, c[i], cb, cw), cb, cw));
                }
                let na = clamp(clamp(fxt_lv(4u, aa, cb, cw), 0.0, 1.0), 0.0, 1.0);
                o = vec4<f32>(nc * na, na);
            }
        }
        case 2u: {
            if (straight_skip) {
                break;
            }
            let stretch = P.f[3].x;
            var r: vec3<f32>;
            for (var i = 0; i < 3; i++) {
                var v = c[i];
                if (stretch > 0.0 && v > 0.0 && v < 1.0) {
                    let q = 1.0 - v;
                    v = v + stretch * 0.25 * v * (q * q * q * q);
                }
                let g = P.f[i];
                r = vset3f(r, u32(i), max(g.y + (g.z - g.y) * powz(max(v, 0.0), g.x), 0.0));
            }
            o = vec4<f32>(r * a, a);
        }
        case 3u: {
            if (straight_skip) {
                break;
            }
            var r = fxt_mix(c, c * f0.xyz, f0.w);
            if (u0.y != 0u) {
                let l0 = luminance(c);
                let l1 = max(luminance(r), 1e-6);
                r = r * l0 / l1;
            }
            o = vec4<f32>(r * a, a);
        }
        case 4u: {
            if (straight_skip) {
                break;
            }
            let fr = f0.xyz;
            let tc = P.f[1].xyz;
            let tol = P.f[2].xyz;
            let soft = f0.w;
            let hsl = rgb_to_hsl(c);
            let d = vec3<f32>(abs(fract_euclid(hsl.x - fr.x + 0.5) - 0.5) * 2.0, abs(hsl.z - fr.z), abs(hsl.y - fr.y));
            var k = 1.0;
            for (var i = 0; i < 3; i++) {
                let over = d[i] - tol[i];
                if (over > 0.0) {
                    k = min(k, select(clamp(1.0 - over / soft, 0.0, 1.0), 0.0, soft <= 1e-6));
                }
            }
            let ch_l = (u0.y & 1u) != 0u;
            let ch_s = (u0.y & 2u) != 0u;
            var r = c;
            if ((u0.y & 8u) != 0u) {
                r = vec3<f32>(k);
            } else if (k > 0.0) {
                var nh: f32;
                var ns = hsl.y;
                var nl = hsl.z;
                if ((u0.y & 4u) != 0u) {
                    let dh = (fract_euclid(tc.x - fr.x + 0.5) - 0.5) * k;
                    nh = fract_euclid(hsl.x + dh);
                    if (ch_s) {
                        ns = hsl.y + (tc.y - fr.y) * k;
                    }
                    if (ch_l) {
                        nl = hsl.z + (tc.z - fr.z) * k;
                    }
                } else {
                    let dh = (fract_euclid(tc.x - hsl.x + 0.5) - 0.5) * k;
                    nh = fract_euclid(hsl.x + dh);
                    if (ch_s) {
                        ns = hsl.y + (tc.y - hsl.y) * k;
                    }
                    if (ch_l) {
                        nl = hsl.z + (tc.z - hsl.z) * k;
                    }
                }
                r = hsl_to_rgb(nh, clamp(ns, 0.0, 1.0), clamp(nl, 0.0, 1.0));
            }
            o = vec4<f32>(r * a, a);
        }
        case 5u: {
            if (straight_skip) {
                break;
            }
            let keep = f0.xyz;
            let amt = P.f[1].x;
            let tol = P.f[1].y;
            let soft = P.f[1].z;
            var d: f32;
            if (u0.y != 0u) {
                let hsl = rgb_to_hsl(c);
                d = select(abs(fract_euclid(hsl.x - f0.w + 0.5) - 0.5) * 2.0, 1.0, hsl.y < 0.02);
            } else {
                d = sqrt(dot(c - keep, c - keep)) / sqrt(3.0);
            }
            var keepk: f32;
            if (tol >= 1.0 || d <= tol) {
                keepk = 1.0;
            } else if (soft <= 1e-6) {
                keepk = 0.0;
            } else {
                keepk = clamp(1.0 - (d - tol) / soft, 0.0, 1.0);
            }
            let g = luminance(c);
            let r = fxt_mix(c, vec3<f32>(g), amt * (1.0 - keepk));
            o = vec4<f32>(r * a, a);
        }
        case 6u: {
            if (a <= 0.0) {
                break;
            }
            var m = fxt_colour_match(c, f0.xyz, f0.w, u0.y, P.f[2].x, P.f[2].y);
            if ((u0.z & 2u) != 0u) {
                m = 1.0 - m;
            }
            var r = c;
            if ((u0.z & 1u) != 0u) {
                r = vec3<f32>(m);
            } else if (m > 0.0) {
                let hsl = rgb_to_hsl(c);
                let dh = P.f[1].x;
                let dl = P.f[1].y;
                let ds = P.f[1].z;
                let h2 = fract_euclid(hsl.x + dh);
                let s2 = clamp(hsl.y + ds * select(hsl.y, 1.0 - hsl.y, ds > 0.0), 0.0, 1.0);
                let l2 = clamp(hsl.z + dl * select(hsl.z, 1.0 - hsl.z, dl > 0.0), 0.0, 1.0);
                r = fxt_mix(c, hsl_to_rgb(h2, s2, l2), m);
            }
            o = vec4<f32>(r * a, a);
        }
        case 7u: {
            if (a <= 0.0) {
                break;
            }
            let pal = u0.y != 0u;
            let y = 0.299 * c.x + 0.587 * c.y + 0.114 * c.z;
            var ch: f32;
            if (pal) {
                let uu = 0.492 * (c.z - y);
                let vv = 0.877 * (c.x - y);
                ch = sqrt(uu * uu + vv * vv);
            } else {
                let ii = 0.596 * c.x - 0.274 * c.y - 0.322 * c.z;
                let qq = 0.211 * c.x - 0.523 * c.y + 0.312 * c.z;
                ch = sqrt(ii * ii + qq * qq);
            }
            let max_norm = f0.x;
            let unsafe_px = y + ch > max_norm + 1e-5;
            switch u0.z {
                case 2u: {
                    if (unsafe_px) {
                        o = vec4<f32>(0.0);
                    }
                }
                case 3u: {
                    if (!unsafe_px) {
                        o = vec4<f32>(0.0);
                    }
                }
                case 1u: {
                    if (unsafe_px) {
                        let target_v = max(max_norm - y, 0.0);
                        let s = select(1.0, target_v / ch, ch > 1e-6);
                        var r = y + (c - y) * s;
                        if (y > max_norm) {
                            r = r * max_norm / y;
                        }
                        o = vec4<f32>(r * a, a);
                    }
                }
                default: {
                    if (unsafe_px) {
                        let k = max_norm / (y + ch);
                        o = vec4<f32>(c * k * a, a);
                    }
                }
            }
        }
        case 8u: {
            if (straight_skip) {
                break;
            }
            let hsl = rgb_to_hsl(c);
            let dh = f0.x;
            let dl = f0.y;
            let ds = f0.z;
            var l = hsl.z;
            if (dl > 0.0) {
                l = l + (1.0 - l) * dl;
            } else {
                l = l + l * dl;
            }
            var s = hsl.y;
            if (ds > 0.0) {
                s = s + (1.0 - s) * ds;
            } else {
                s = s + s * ds;
            }
            let r = hsl_to_rgb(hsl.x + dh, clamp(s, 0.0, 1.0), clamp(l, 0.0, 1.0));
            o = vec4<f32>(r * a, a);
        }
        case 9u: {
            if (straight_skip) {
                break;
            }
            let n = u0.y;
            let t = clamp(luminance(c), 0.0, 1.0) * f32(n);
            let k = min(u32(floor(t)), n - 1u);
            let tone = fxt_mix(P.f[1u + k].xyz, P.f[2u + k].xyz, t - f32(k));
            let r = fxt_mix(tone, c, f0.x);
            o = vec4<f32>(r * a, a);
        }
        case 10u: {
            if (straight_skip) {
                break;
            }
            var r = c;
            for (var k = 0; k < 3; k++) {
                if (f0[k] == 0.0) {
                    continue;
                }
                let x = c[k] + f0[k];
                if (x <= 1.0) {
                    r = vset3f(r, u32(k), x);
                } else if (u0.y == 1u) {
                    r = vset3f(r, u32(k), max(2.0 - x, 0.0));
                } else if (u0.y == 2u) {
                    r = vset3f(r, u32(k), select(1.0, 0.0, x - 1.0 < 0.5));
                } else {
                    r = vset3f(r, u32(k), fract_euclid(x));
                }
            }
            o = vec4<f32>(r * a, a);
        }
        case 11u: {
            if (a <= 0.0) {
                break;
            }
            var acc = vec3<f32>(0.0);
            for (var j = 0; j < 3; j++) {
                for (var i = 0; i < 3; i++) {
                    let idx = j * 3 + i;
                    let w = P.f[idx / 4][idx % 4];
                    if (w == 0.0) {
                        continue;
                    }
                    let q = tex_get_clamped(src, p.x + i - 1, p.y + j - 1);
                    acc += fxt_straight(q) * w;
                }
            }
            var v = acc * P.f[2].y + P.f[2].z;
            if (u0.y != 0u) {
                v = abs(v);
            }
            o = vec4<f32>(max(v, vec3<f32>(0.0)) * a, a);
        }
        case 12u: {
            let n = u0.z;
            let aa = max(a, 0.0);
            let r = vec3<f32>(fxt_lookup(c.x, n), fxt_lookup(c.y, n), fxt_lookup(c.z, n));
            var na = aa;
            if (u0.y != 0u) {
                na = fxt_lookup(aa, n);
            }
            o = vec4<f32>(r * na, na);
        }
        case 13u: {
            if (a <= 0.0) {
                break;
            }
            let level = f0.x;
            let start = f0.y;
            let y = luminance(c);
            var r = c;
            if (u0.y != 1u) {
                let y2 = fxt_knee(max(y, 0.0), start, level);
                r = r - y + y2;
            }
            if (u0.y != 0u) {
                let yl = luminance(r);
                var t = 1.0;
                for (var i = 0; i < 3; i++) {
                    let v = r[i];
                    if (v > level && v - yl > 1e-6) {
                        t = min(t, (level - yl) / (v - yl));
                    }
                    if (v < 0.0 && yl - v > 1e-6) {
                        t = min(t, yl / (yl - v));
                    }
                }
                t = clamp(t, 0.0, 1.0);
                r = yl + (r - yl) * t;
            }
            r = clamp(r, vec3<f32>(0.0), vec3<f32>(level));
            if (u0.z != 0u && any(abs(c - r) > vec3<f32>(1e-4))) {
                r = P.f[1].xyz;
            }
            o = vec4<f32>(r * a, a);
        }
        case 14u: {
            if (straight_skip) {
                break;
            }
            let hsl = rgb_to_hsl(c);
            var h = hsl.x;
            var s = hsl.y;
            var l = hsl.z;
            if (u0.y != 0u) {
                let cl = P.f[1].z;
                h = P.f[1].x;
                s = P.f[1].y;
                l = clamp(l + cl * select(l, 1.0 - l, cl > 0.0), 0.0, 1.0);
            } else {
                var s2 = fxt_adjust_sat(s, f0.y);
                var l2 = fxt_adjust_light(l, f0.z);
                var h2 = h + f0.x;
                for (var r = 0u; r < u0.z; r++) {
                    let w = fxt_range_weight(r, hsl.x * 360.0) * min(hsl.y * 20.0, 1.0);
                    if (w > 0.0) {
                        let ro = r * 7u;
                        h2 += data[ro + 4u] * w;
                        s2 = fxt_adjust_sat(clamp(s2, 0.0, 1.0), data[ro + 5u] * w);
                        l2 = fxt_adjust_light(l2, data[ro + 6u] * w);
                    }
                }
                h = fract_euclid(h2);
                s = clamp(s2, 0.0, 1.0);
                l = l2;
            }
            o = vec4<f32>(hsl_to_rgb(h, s, l) * a, a);
        }
        case 15u: {
            var r = c;
            if (!straight_skip) {
                for (var i = 0u; i < 3u; i++) {
                    r = vset3f(r, u32(i), fxt_level(0u, c[i]));
                }
            }
            // The master pass writes premultiplied values; the channel pass reads them back.
            var q = px;
            if (!straight_skip) {
                q = vec4<f32>(r * a, a);
            }
            let qc = fxt_straight(q);
            let qa = max(q.w, 0.0);
            var any_ch = false;
            for (var k = 0u; k < 4u; k++) {
                if (!fxt_level_ident(7u + 7u * k)) {
                    any_ch = true;
                }
            }
            if (!any_ch) {
                o = q;
                break;
            }
            var oc = qc;
            for (var i = 0u; i < 3u; i++) {
                let off = 7u + 7u * i;
                if (!fxt_level_ident(off)) {
                    oc = vset3f(oc, u32(i), fxt_level(off, qc[i]));
                }
            }
            var na = qa;
            if (!fxt_level_ident(28u)) {
                na = clamp(fxt_level(28u, qa), 0.0, 1.0);
            }
            o = vec4<f32>(oc * na, na);
        }
        case 16u: {
            if (a <= 0.0 && px.x == 0.0 && px.y == 0.0 && px.z == 0.0) {
                break;
            }
            let lo = f0.xyz;
            let hi = P.f[1].xyz;
            let g = P.f[2];
            var r: vec3<f32>;
            for (var k = 0; k < 3; k++) {
                let st = clamp((c[k] - lo[k]) / (hi[k] - lo[k]), 0.0, 1.0);
                r = vset3f(r, u32(k), powz(st, g[k]));
            }
            r = fxt_mix(r, c, g.w);
            let na = clamp(max(a, 0.0), 0.0, 1.0);
            o = vec4<f32>(r * na, na);
        }
        case 17u: {
            if (a <= 0.0 && px.x == 0.0 && px.y == 0.0 && px.z == 0.0) {
                break;
            }
            let n = u0.z;
            let amt = f0.x;
            var r: vec3<f32>;
            if (u0.y == 1u) {
                let l = luminance(c);
                let nl = fxt_eq_look(0u, n, l);
                if (l > 1e-5) {
                    r = c * nl / l;
                } else {
                    r = vec3<f32>(nl);
                }
            } else if (u0.y == 2u) {
                r = vec3<f32>(fxt_eq_look(0u, n, c.x), fxt_eq_look(0u, n, c.y), fxt_eq_look(0u, n, c.z));
            } else {
                r = vec3<f32>(fxt_eq_look(0u, n, c.x), fxt_eq_look(1u, n, c.y), fxt_eq_look(2u, n, c.z));
            }
            r = fxt_mix(c, r, amt);
            let na = clamp(max(a, 0.0), 0.0, 1.0);
            o = vec4<f32>(r * na, na);
        }
        case 18u: {
            if (a <= 0.0) {
                break;
            }
            let ch = u0.y;
            var r: vec3<f32>;
            if (ch <= 7u) {
                var hsl = rgb_to_hsl(c);
                if (ch == 4u || ch == 5u) {
                    hsl.x = 1.0 - hsl.x;
                }
                if (ch == 4u || ch == 6u) {
                    hsl.z = 1.0 - hsl.z;
                }
                if (ch == 4u || ch == 7u) {
                    hsl.y = 1.0 - hsl.y;
                }
                r = hsl_to_rgb(hsl.x, hsl.y, hsl.z);
            } else {
                var q = vec3<f32>(
                    0.299 * c.x + 0.587 * c.y + 0.114 * c.z,
                    0.596 * c.x - 0.274 * c.y - 0.322 * c.z,
                    0.211 * c.x - 0.523 * c.y + 0.312 * c.z,
                );
                if (ch == 8u || ch == 9u) {
                    q.x = 1.0 - q.x;
                }
                if (ch == 8u || ch == 10u) {
                    q.y = -q.y;
                }
                if (ch == 8u || ch == 11u) {
                    q.z = -q.z;
                }
                r = max(
                    vec3<f32>(q.x + 0.956 * q.y + 0.621 * q.z, q.x - 0.272 * q.y - 0.647 * q.z, q.x - 1.106 * q.y + 1.703 * q.z),
                    vec3<f32>(0.0),
                );
            }
            o = vec4<f32>((c + (r - c) * f0.x) * a, a);
        }
        case 19u: {
            o = vec4<f32>(luminance(c));
        }
        case 20u: {
            if (a <= 0.0) {
                break;
            }
            let bases = textureLoad(aux, p, 0);
            let s_amt = f0.x;
            let h_amt = f0.y;
            let ws0 = clamp(1.0 - bases.x / f0.z, 0.0, 1.0);
            let wh0 = clamp((bases.y - (1.0 - f0.w)) / f0.w, 0.0, 1.0);
            let ws = ws0 * ws0;
            let wh = wh0 * wh0;
            let gain = (1.0 + s_amt * ws * 2.0) * (1.0 - h_amt * wh * 0.5);
            let l = luminance(c);
            let nl = l * gain;
            let sat = pow(max(gain, 1e-3), P.f[1].x);
            var r = nl + (c - l) * sat;
            let mc = P.f[1].y;
            if (mc != 0.0) {
                let shift = (0.5 + (nl - 0.5) * (1.0 + mc)) - nl;
                r = r + shift;
            }
            r = fxt_mix(max(r, vec3<f32>(0.0)), c, P.f[1].z);
            o = vec4<f32>(r * a, a);
        }
        case 21u: {
            o = vec4<f32>(px.x, textureLoad(aux, p, 0).x, 0.0, 0.0);
        }
        case 22u: {
            if (a <= 0.0 && px.x == 0.0 && px.y == 0.0 && px.z == 0.0) {
                break;
            }
            let r = max(f0.x + (c - f0.y) * f0.z, vec3<f32>(0.0));
            let na = clamp(max(a, 0.0), 0.0, 1.0);
            o = vec4<f32>(r * na, na);
        }
        case 23u: {
            if (a <= 0.0) {
                break;
            }
            let l = clamp(luminance(c), 0.0, 1.0);
            let ws = 1.0 - fxt_smoothstep(0.0, 0.5, l);
            let wh = fxt_smoothstep(0.5, 1.0, l);
            let wm = 1.0 - ws - wh;
            let pin = P.f[3].x;
            let keep = 1.0 - pin * clamp(1.0 - 4.0 * l * (1.0 - l), 0.0, 1.0);
            let contrast = P.f[3].z;
            let darks = P.f[4].x;
            let brights = P.f[4].y;
            var r: vec3<f32>;
            for (var k = 0; k < 3; k++) {
                let d = P.f[0][k] * ws + P.f[1][k] * wm + P.f[2][k] * wh;
                var v = c[k] + d * keep;
                v = 0.5 + (v - 0.5) * (1.0 + contrast);
                v += darks * 0.25 * (1.0 - l) * (1.0 - l) + brights * 0.25 * l * l;
                r = vset3f(r, u32(k), max(v, 0.0));
            }
            r = fxt_mix(r, c, P.f[3].y);
            o = vec4<f32>(r * a, a);
        }
        case 24u: {
            if (a <= 0.0) {
                break;
            }
            var r: vec3<f32>;
            for (var k = 0u; k < 3u; k++) {
                let base = k * 11u;
                let n = u32(data[base]);
                let v = c[k];
                // slice::partition_point(x < v), clamped to 1..n-1.
                var i = 0u;
                while (i < n && data[base + 1u + 2u * i] < v) {
                    i++;
                }
                i = clamp(i, 1u, n - 1u);
                let ax = data[base + 1u + 2u * (i - 1u)];
                let ay = data[base + 2u + 2u * (i - 1u)];
                let cx = data[base + 1u + 2u * i];
                let cy = data[base + 2u + 2u * i];
                r = vset3f(r, u32(k), ay + (cy - ay) * (v - ax) / max(cx - ax, 1e-9));
            }
            o = vec4<f32>(r * a, a);
        }
        default: {}
    }
    textureStore(out, p, o);
}

// util::smoothstep.
fn fxt_smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    if (abs(e1 - e0) < 1e-9) {
        return select(1.0, 0.0, x < e0);
    }
    let t = clamp((x - e0) / (e1 - e0), 0.0, 1.0);
    return t * t * (3.0 - 2.0 * t);
}

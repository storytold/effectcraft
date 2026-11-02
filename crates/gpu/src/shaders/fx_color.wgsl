// GPU effects (color family): see src/fx_color.rs. Each entry point mirrors the CPU effect in
// effectcraft-effects operation for operation; parameter layouts are documented per entry.
// Every name here is prefixed `fxc_` (the family files share one module).

// ---------------------------------------------------------------- helpers

// util::unpremul: straight colour (0 when alpha ≤ 1e-6).
fn fxc_unpremul(px: vec4<f32>) -> vec3<f32> {
    if (px.w > 1e-6) {
        return px.xyz / px.w;
    }
    return vec3<f32>(0.0);
}

// util::smoothstep.
fn fxc_smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    if (abs(e1 - e0) < 1e-9) {
        return select(1.0, 0.0, x < e0);
    }
    let t = clamp((x - e0) / (e1 - e0), 0.0, 1.0);
    return t * t * (3.0 - 2.0 * t);
}

fn fxc_mix(a: vec3<f32>, b: vec3<f32>, t: f32) -> vec3<f32> {
    return a + (b - a) * t;
}

// color::hsv_to_rgb.
fn fxc_hsv_to_rgb(h: f32, s: f32, v: f32) -> vec3<f32> {
    let h6 = fract_euclid(h) * 6.0;
    let i = floor(h6);
    let f = h6 - i;
    let p = v * (1.0 - s);
    let q = v * (1.0 - s * f);
    let t = v * (1.0 - s * (1.0 - f));
    switch i32(i) {
        case 0: { return vec3<f32>(v, t, p); }
        case 1: { return vec3<f32>(q, v, p); }
        case 2: { return vec3<f32>(p, v, t); }
        case 3: { return vec3<f32>(p, q, v); }
        case 4: { return vec3<f32>(t, p, v); }
        default: { return vec3<f32>(v, p, q); }
    }
}

fn fxc_pixel(gid: vec3<u32>) -> vec2<i32> {
    let dims = out_dims();
    let p = vec2<i32>(gid.xy);
    if (p.x >= dims.x || p.y >= dims.y) {
        return vec2<i32>(-1);
    }
    return p;
}

// ---------------------------------------------------------------- per-pixel colour effects

// Colorama's input phase (color_fx::colorama_phase).
fn fxc_colorama_phase(c: vec3<f32>, a: f32, mode: u32) -> f32 {
    var v: f32;
    switch mode {
        case 1u: { v = c.x; }
        case 2u: { v = c.y; }
        case 3u: { v = c.z; }
        case 4u: { v = rgb_to_hsl(c).x; }
        case 5u: { v = rgb_to_hsl(c).z; }
        case 6u: { v = rgb_to_hsl(c).y; }
        case 7u: { v = max(max(c.x, c.y), c.z); }
        case 8u: { v = a; }
        case 9u: { v = 0.0; }
        default: { v = luminance(c); }
    }
    return clamp(v, 0.0, 1.0);
}

// Selective Color range weights (color2::sc_weights).
fn fxc_sc_weights(c0: vec3<f32>) -> array<f32, 9> {
    let c = clamp(c0, vec3<f32>(0.0), vec3<f32>(1.0));
    // Stable descending sort of the channel indices.
    var idx = vec3<u32>(0u, 1u, 2u);
    for (var i = 1u; i < 3u; i++) {
        var j = i;
        while (j > 0u && c[idx[j]] > c[idx[j - 1u]]) {
            let t = idx[j];
            idx[j] = idx[j - 1u];
            idx[j - 1u] = t;
            j--;
        }
    }
    let mx = c[idx.x];
    let md = c[idx.y];
    let mn = c[idx.z];
    var w = array<f32, 9>(0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
    var prim = array<u32, 3>(0u, 2u, 4u);
    var sec = array<u32, 3>(3u, 5u, 1u);
    w[prim[idx.x]] = mx - md;
    w[sec[idx.z]] = md - mn;
    w[6] = max((mn - 0.5) * 2.0, 0.0);
    w[8] = max((0.5 - mx) * 2.0, 0.0);
    w[7] = clamp(1.0 - (abs(mx - 0.5) + abs(mn - 0.5)), 0.0, 1.0);
    return w;
}

// u[0].x = op:
//  1 Color Balance   f[0] = shadows, f[1] = midtones, f[2] = highlights; u[0].y = preserve luminosity
//  2 Vibrance        f[0] = (vibrance, saturation)
//  3 Black & White   f[0] = (reds, yellows, greens, cyans), f[1] = (blues, magentas);
//                    u[0].y = tint; f[2] = (tint hue, tint saturation)
//  4 Tritone         f[0] = highlights, f[1] = midtones, f[2] = shadows; f[3].x = 1 − blend
//  5 Colorama        u[0].y = phase mode, u[0].z = interpolate; f[0] = (cycles, shift, 1 − blend)
//  6 Channel Mixer   f[0..3] = output rows (r, g, b, const); u[0].y = monochrome
//  7 Selective Color data = 9 × (cyan, magenta, yellow, black); u[0].y = relative
//  8 Linear Color Key f[0] = (key rgb, tolerance), f[1] = (softness, key chroma xy, key hue);
//                    u[0] = (_, match mode, keep colours, view)
@compute @workgroup_size(16, 16)
fn fxc_point(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = fxc_pixel(gid);
    if (p.x < 0) {
        return;
    }
    var px = textureLoad(src, p, 0);
    let a = px.w;
    switch P.u[0].x {
        case 1u: {
            if (a > 0.0) {
                let c = px.xyz / a;
                let l = clamp(luminance(c), 0.0, 1.0);
                let ws = clamp(1.0 - l * 2.0, 0.0, 1.0);
                let wh = clamp(l * 2.0 - 1.0, 0.0, 1.0);
                let wm = 1.0 - ws - wh;
                var o = max(c + (P.f[0].xyz * ws + P.f[1].xyz * wm + P.f[2].xyz * wh) * 0.5, vec3<f32>(0.0));
                if (P.u[0].y != 0u) {
                    let d = luminance(c) - luminance(o);
                    o = max(o + d, vec3<f32>(0.0));
                }
                px = vec4<f32>(o * a, a);
            }
        }
        case 2u: {
            if (a > 0.0) {
                let c = px.xyz / a;
                let hsl = rgb_to_hsl(c);
                let vib = P.f[0].x;
                let sat = P.f[0].y;
                let s = clamp(hsl.y * (1.0 + sat) + vib * (1.0 - hsl.y) * min(hsl.y, 0.5), 0.0, 1.0);
                px = vec4<f32>(hsl_to_rgb(hsl.x, s, hsl.z) * a, a);
            }
        }
        case 3u: {
            if (a > 0.0) {
                let c = px.xyz / a;
                var w = array<f32, 6>(P.f[0].x, P.f[0].y, P.f[0].z, P.f[0].w, P.f[1].x, P.f[1].y);
                let hsl = rgb_to_hsl(c);
                let hp = hsl.x * 6.0;
                let i = u32(max(floor(hp), 0.0)) % 6u;
                let f = hp - floor(hp);
                let wt = w[i] + (w[(i + 1u) % 6u] - w[i]) * f;
                let mx = max(max(c.x, c.y), c.z);
                let mn = min(min(c.x, c.y), c.z);
                let gray = max(mn + (mx - mn) * wt, 0.0);
                var o = vec3<f32>(gray);
                if (P.u[0].y != 0u) {
                    o = hsl_to_rgb(P.f[2].x, P.f[2].y, min(gray, 1.0));
                }
                px = vec4<f32>(o * a, a);
            }
        }
        case 4u: {
            if (a > 0.0) {
                let c = px.xyz / a;
                let l = clamp(luminance(c), 0.0, 1.0);
                var t: vec3<f32>;
                if (l < 0.5) {
                    t = fxc_mix(P.f[2].xyz, P.f[1].xyz, l * 2.0);
                } else {
                    t = fxc_mix(P.f[1].xyz, P.f[0].xyz, (l - 0.5) * 2.0);
                }
                px = vec4<f32>(fxc_mix(c, t, P.f[3].x) * a, a);
            }
        }
        case 5u: {
            if (a > 0.0) {
                let c = px.xyz / a;
                var ph = fract_euclid(fxc_colorama_phase(c, a, P.u[0].y) * P.f[0].x + P.f[0].y);
                if (P.u[0].z == 0u) {
                    ph = floor(ph * 6.0) / 6.0;
                }
                let o = fxc_mix(c, fxc_hsv_to_rgb(ph, 1.0, 1.0), P.f[0].z);
                px = vec4<f32>(o * a, a);
            }
        }
        case 6u: {
            if (a > 0.0) {
                let c = px.xyz / a;
                var o: vec3<f32>;
                for (var i = 0; i < 3; i++) {
                    let r = P.f[i];
                    o[i] = max(c.x * r.x + c.y * r.y + c.z * r.z + r.w, 0.0);
                }
                if (P.u[0].y != 0u) {
                    o = vec3<f32>(o.x);
                }
                px = vec4<f32>(o * a, a);
            }
        }
        case 7u: {
            // color2::map_ca: fully transparent black stays.
            if (a <= 0.0 && px.x == 0.0 && px.y == 0.0 && px.z == 0.0) {
                textureStore(out, p, px);
                return;
            }
            let c = fxc_unpremul(px);
            var w = fxc_sc_weights(c);
            var o = c;
            let relative = P.u[0].y != 0u;
            for (var r = 0u; r < 9u; r++) {
                let wr = w[r];
                if (wr <= 0.0) {
                    continue;
                }
                let k = data[r * 4u + 3u];
                for (var i = 0u; i < 3u; i++) {
                    let ink = 1.0 - c[i];
                    var delta = data[r * 4u + i] + k;
                    if (relative) {
                        delta = delta * max(ink, 0.0);
                    }
                    o[i] -= wr * delta;
                }
            }
            let na = clamp(max(a, 0.0), 0.0, 1.0);
            px = vec4<f32>(max(o, vec3<f32>(0.0)) * na, na);
        }
        case 8u: {
            let c = fxc_unpremul(px);
            let key = P.f[0].xyz;
            let tol = P.f[0].w;
            let soft = P.f[1].x;
            var d: f32;
            switch P.u[0].y {
                case 1u: {
                    let hsl = rgb_to_hsl(c);
                    if (hsl.y < 0.02) {
                        d = 1.0;
                    } else {
                        let dh = abs(hsl.x - P.f[1].w);
                        d = min(dh, 1.0 - dh) * 2.0;
                    }
                }
                case 2u: {
                    let s = c.x + c.y + c.z;
                    var pc = vec2<f32>(1.0 / 3.0, 1.0 / 3.0);
                    if (s > 1e-6) {
                        pc = vec2<f32>(c.x / s, c.y / s);
                    }
                    let dx = pc.x - P.f[1].y;
                    let dy = pc.y - P.f[1].z;
                    d = sqrt(dx * dx + dy * dy) / 1.4142135;
                }
                default: {
                    let e = c - key;
                    d = sqrt(e.x * e.x + e.y * e.y + e.z * e.z) / 1.7320508;
                }
            }
            var v: f32;
            if (soft > 1e-6) {
                v = clamp((d - tol) / soft, 0.0, 1.0);
            } else if (d > tol) {
                v = 1.0;
            } else {
                v = 0.0;
            }
            if (P.u[0].z != 0u) {
                v = 1.0 - v;
            }
            let k = clamp(v, 0.0, 1.0);
            if (P.u[0].w == 2u) {
                let m = px.w * k;
                px = vec4<f32>(m, m, m, 1.0);
            } else {
                px = px * k;
            }
        }
        default: {}
    }
    textureStore(out, p, px);
}

// ---------------------------------------------------------------- Lumetri Color (color3)

fn fxc_tone_curve(x: f32, s: f32, m: f32, h: f32) -> f32 {
    if (s == 0.0 && m == 0.0 && h == 0.0) {
        return x;
    }
    let t = clamp(x, 0.0, 1.0);
    let d0 = (t - 0.25) / 0.25;
    let d1 = (t - 0.5) / 0.25;
    let d2 = (t - 0.75) / 0.25;
    let b0 = max(1.0 - d0 * d0, 0.0);
    let b1 = max(1.0 - d1 * d1, 0.0);
    let b2 = max(1.0 - d2 * d2, 0.0);
    return x + 0.25 * (s * (b0 * b0) + m * (b1 * b1) + h * (b2 * b2));
}

fn fxc_d3(i: u32) -> vec3<f32> {
    return vec3<f32>(data[i], data[i + 1u], data[i + 2u]);
}

// data: 0 temperature, 1 tint, 2 exposure gain, 3 contrast, 4 highlights, 5 shadows, 6 whites,
// 7 blacks, 8 saturation, 9 look intensity, 10 faded film, 11 vibrance, 12 creative saturation,
// 13 tint balance pivot, 14..17 shadow tint, 17..20 highlight tint, 20..32 curves (master, red,
// green, blue × shadows / midtones / highlights), 32..41 wheels (shadows, midtones, highlights),
// 41 vignette amount, 42 vignette start, 43 roundness, 44 feather, 45 cx, 46 cy, 47 lw, 48 lh,
// 49 aspect.
// color3::builtin_look.
fn fxc_builtin_look(look: u32, c: vec3<f32>) -> vec3<f32> {
    let l = luminance(c);
    switch (look) {
        case 2u: {
            return vec3<f32>(c.x * 1.06 + 0.015, c.y * 1.01 + 0.01, c.z * 0.88);
        }
        case 3u: {
            return vec3<f32>(c.x * 0.82, c.y * 0.92, c.z * 1.08 + 0.02) * 0.92;
        }
        case 4u: {
            let d = l + (c - l) * 0.45;
            var o: vec3<f32>;
            if (l < 0.5) {
                o = 2.0 * d * l;
            } else {
                o = 1.0 - 2.0 * (1.0 - d) * (1.0 - l);
            }
            return d * 0.5 + o * 0.5;
        }
        case 5u: {
            let t = clamp(l - 0.5, -0.5, 0.5);
            return vec3<f32>(c.x + 0.18 * t, c.y + 0.03 * t, c.z - 0.16 * t);
        }
        case 6u: {
            let v = 0.08 + c * 0.84;
            return l + (v - l) * 0.8;
        }
        case 7u: {
            return vec3<f32>(l);
        }
        default: {
            return c;
        }
    }
}

@compute @workgroup_size(16, 16)
fn fxc_lumetri(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = fxc_pixel(gid);
    if (p.x < 0) {
        return;
    }
    var px = textureLoad(src, p, 0);
    let a = px.w;
    if (a <= 0.0) {
        textureStore(out, p, px);
        return;
    }
    var c = fxc_unpremul(px);
    if (data[51] >= 0.0) {
        c = fxl_lut_apply(u32(data[51]), c, 2u);
    }
    let temp = data[0];
    let tint = data[1];
    if (temp != 0.0 || tint != 0.0) {
        c = vec3<f32>(c.x * (1.0 + 0.25 * temp), c.y * (1.0 - 0.25 * tint), c.z * (1.0 - 0.25 * temp));
    }
    let expo = data[2];
    if (expo != 1.0) {
        c = c * expo;
    }
    let contrast = data[3];
    if (contrast != 0.0) {
        c = 0.5 + (c - 0.5) * (1.0 + contrast);
    }
    let hi = data[4];
    let sh = data[5];
    let wh = data[6];
    let bl = data[7];
    if (hi != 0.0 || sh != 0.0 || wh != 0.0 || bl != 0.0) {
        let l = max(luminance(c), 0.0);
        let lt = min(l, 1.0);
        let il = 1.0 - lt;
        let d = 0.25 * hi * fxc_smoothstep(0.5, 1.0, lt) + 0.25 * sh * (1.0 - fxc_smoothstep(0.0, 0.5, lt)) + 0.2 * wh * lt * lt + 0.2 * bl * (il * il);
        c = c + d;
    }
    let l0 = luminance(c);
    let sat = data[8];
    if (sat != 1.0) {
        c = l0 + (c - l0) * sat;
    }
    let intensity = data[9];
    let faded = data[10];
    let vib = data[11];
    let csat = data[12];
    let st = fxc_d3(14u);
    let ht = fxc_d3(17u);
    let zero = vec3<f32>(0.0);
    let look = u32(data[50]);
    let has_look = look >= 2u || data[52] >= 0.0;
    if (intensity != 0.0 && (has_look || faded != 0.0 || vib != 0.0 || csat != 1.0 || any(st != zero) || any(ht != zero))) {
        var d = c;
        if (data[52] >= 0.0) {
            d = fxl_lut_apply(u32(data[52]), c, 2u);
        } else {
            d = fxc_builtin_look(look, c);
        }
        if (faded != 0.0) {
            d = d * (1.0 - 0.25 * faded) + 0.12 * faded;
        }
        let l = luminance(d);
        if (vib != 0.0) {
            let mx = max(max(d.x, d.y), d.z);
            let mn = min(min(d.x, d.y), d.z);
            var s = 0.0;
            if (mx > 1e-6) {
                s = (mx - mn) / mx;
            }
            let k = 1.0 + vib * (1.0 - clamp(s, 0.0, 1.0));
            d = l + (d - l) * k;
        }
        if (csat != 1.0) {
            d = l + (d - l) * csat;
        }
        let pivot = data[13];
        let lt = clamp(l, 0.0, 1.0);
        let wsh = 1.0 - fxc_smoothstep(0.0, pivot, lt);
        let whi = fxc_smoothstep(pivot, 1.0, lt);
        d = d + 0.5 * (st * wsh + ht * whi);
        c = c + (d - c) * intensity;
    }
    for (var i = 0u; i < 3u; i++) {
        c[i] = fxc_tone_curve(c[i], data[20], data[21], data[22]);
    }
    for (var i = 0u; i < 3u; i++) {
        let b = 23u + i * 3u;
        c[i] = fxc_tone_curve(c[i], data[b], data[b + 1u], data[b + 2u]);
    }
    let ws = fxc_d3(32u);
    let wm = fxc_d3(35u);
    let wh3 = fxc_d3(38u);
    if (any(ws != zero) || any(wm != zero) || any(wh3 != zero)) {
        let lt = clamp(luminance(c), 0.0, 1.0);
        let ka = 1.0 - fxc_smoothstep(0.0, 0.5, lt);
        let kz = fxc_smoothstep(0.5, 1.0, lt);
        let km = 1.0 - ka - kz;
        c = c + 0.5 * (ws * ka + wm * km + wh3 * kz);
    }
    let vig = data[41];
    if (vig != 0.0) {
        let x = f32(p.x) + 0.5;
        let y = f32(p.y) + 0.5;
        let lw = data[47];
        let lh = data[48];
        var nx = (x - data[45]) / max(lw * 0.5, 1e-6);
        var ny = (y - data[46]) / max(lh * 0.5, 1e-6);
        let vround = data[43];
        let aspect = data[49];
        if (vround > 0.0) {
            nx = nx * (1.0 + (max(aspect, 1.0) - 1.0) * vround);
            ny = ny * (1.0 + (max(1.0 / aspect, 1.0) - 1.0) * vround);
        }
        let pw = 2.0 + max(-vround, 0.0) * 6.0;
        let r = powz(powz(abs(nx), pw) + powz(abs(ny), pw), 1.0 / pw) / 1.4142135;
        let start = data[42];
        let f = fxc_smoothstep(start, start + 0.05 + data[44] * (1.05 - start), r);
        if (vig > 0.0) {
            c = c + (1.0 - c) * min(vig * 0.2 * f, 1.0);
        } else {
            let k = 1.0 + vig * 0.2 * f;
            c = c * max(k, 0.0);
        }
    }
    c = max(c, zero);
    textureStore(out, p, vec4<f32>(c * a, a));
}

// Straight-colour luminance plane (util::Plane::luma) in x.
@compute @workgroup_size(16, 16)
fn fxc_luma(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = fxc_pixel(gid);
    if (p.x < 0) {
        return;
    }
    let px = textureLoad(src, p, 0);
    textureStore(out, p, vec4<f32>(luminance(fxc_unpremul(px)), 0.0, 0.0, 0.0));
}

// Lumetri Sharpen: `src` + (luma − blurred luma `aux`.x) × amount. f[0].x = sharpen × 2.
@compute @workgroup_size(16, 16)
fn fxc_sharpen(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = fxc_pixel(gid);
    if (p.x < 0) {
        return;
    }
    let px = textureLoad(src, p, 0);
    let a = px.w;
    if (a <= 0.0) {
        textureStore(out, p, px);
        return;
    }
    let c = fxc_unpremul(px);
    let d = (luminance(c) - textureLoad(aux, p, 0).x) * P.f[0].x;
    textureStore(out, p, vec4<f32>(max(c + d, vec3<f32>(0.0)) * a, a));
}

// ---------------------------------------------------------------- planes (util)

// One morphology pass (util::minmax_row, edges repeated) of radius u[0].x along rows (u[0].y = 0)
// or columns; u[1] = per-channel operation: 0 min, 1 max, 2 copy.
@compute @workgroup_size(16, 16)
fn fxc_morph(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = fxc_pixel(gid);
    if (p.x < 0) {
        return;
    }
    let dims = out_dims();
    let r = i32(P.u[0].x);
    let vertical = P.u[0].y != 0u;
    let modes = P.u[1];
    let centre = textureLoad(src, p, 0);
    var lo = centre;
    var hi = centre;
    for (var k = -r; k <= r; k++) {
        var q = p;
        if (vertical) {
            q.y = clamp(p.y + k, 0, dims.y - 1);
        } else {
            q.x = clamp(p.x + k, 0, dims.x - 1);
        }
        let v = textureLoad(src, q, 0);
        lo = min(lo, v);
        hi = max(hi, v);
    }
    var o = centre;
    for (var i = 0; i < 4; i++) {
        if (modes[i] == 0u) {
            o[i] = lo[i];
        } else if (modes[i] == 1u) {
            o[i] = hi[i];
        }
    }
    textureStore(out, p, o);
}

// `src` + (`aux` − `src`) × f[0].x (Plane::zip_map lerp).
@compute @workgroup_size(16, 16)
fn fxc_lerp(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = fxc_pixel(gid);
    if (p.x < 0) {
        return;
    }
    let x = textureLoad(src, p, 0);
    let y = textureLoad(aux, p, 0);
    textureStore(out, p, x + (y - x) * P.f[0].x);
}

// ---------------------------------------------------------------- Key Light (keylight.rs)

fn fxc_neutralise(c: vec3<f32>, bias: vec3<f32>) -> vec3<f32> {
    let l = max(luminance(bias), 1e-4);
    return vec3<f32>(c.x * l / max(bias.x, 1e-4), c.y * l / max(bias.y, 1e-4), c.z * l / max(bias.z, 1e-4));
}

fn fxc_screen_diff(c: vec3<f32>, ix: vec3<u32>, bal: f32) -> f32 {
    return c[ix.x] - (bal * c[ix.y] + (1.0 - bal) * c[ix.z]);
}

fn fxc_correct(c0: vec3<f32>, sat: f32, contrast: f32, bright: f32) -> vec3<f32> {
    let l = luminance(c0);
    let c = l + (c0 - l) * sat;
    return ((c - 0.5) * contrast + 0.5) * bright;
}

// Raw screen matte of `src` (the key source): out = (clipped, clipped, raw, 0).
// u[0].xyz = primary, other channels; f[0] = (alpha bias, screen difference);
// f[1] = (gain, balance, clip black, clip white).
@compute @workgroup_size(16, 16)
fn fxc_kl_raw(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = fxc_pixel(gid);
    if (p.x < 0) {
        return;
    }
    let c = fxc_unpremul(textureLoad(src, p, 0));
    let d = fxc_screen_diff(fxc_neutralise(c, P.f[0].xyz), P.u[0].xyz, P.f[1].y);
    let raw = clamp(1.0 - P.f[1].x * d / P.f[0].w, 0.0, 1.0);
    let cb = P.f[1].z;
    let m = clamp((raw - cb) / max(P.f[1].w - cb, 1e-4), 0.0, 1.0);
    textureStore(out, p, vec4<f32>(m, m, raw, 0.0));
}

// Clip Rollback: `src` = (shrunk, grown) clipped matte, `aux` = raw planes (clipped, _, raw).
@compute @workgroup_size(16, 16)
fn fxc_kl_rollback(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = fxc_pixel(gid);
    if (p.x < 0) {
        return;
    }
    let sg = textureLoad(src, p, 0);
    let r = textureLoad(aux, p, 0);
    var m = r.x;
    if (sg.y > sg.x + 1e-4) {
        m = min(max(m, r.z), sg.y);
    }
    textureStore(out, p, vec4<f32>(m, m, r.z, 0.0));
}

// Planes for the final pass: (screen matte from `src`.x, raw from `aux`.z, edge 0, 0).
@compute @workgroup_size(16, 16)
fn fxc_kl_pack(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = fxc_pixel(gid);
    if (p.x < 0) {
        return;
    }
    textureStore(out, p, vec4<f32>(textureLoad(src, p, 0).x, textureLoad(aux, p, 0).z, 0.0, 0.0));
}

// Combined matte from the screen matte and the source alpha (no inside / outside masks).
fn fxc_kl_combined(sm: f32, src_a: f32, mode: u32) -> f32 {
    var a = sm;
    let sa = clamp(src_a, 0.0, 1.0);
    var ins = 0.0;
    if (mode == 1u) {
        ins = max(ins, sa);
    }
    a = max(a, ins);
    if (mode == 2u) {
        a = a * sa;
    }
    return clamp(a, 0.0, 1.0);
}

// Edge band of the combined matte: `src` = source, `aux` = planes; u[0].x = source alpha mode.
@compute @workgroup_size(16, 16)
fn fxc_kl_band(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = fxc_pixel(gid);
    if (p.x < 0) {
        return;
    }
    let a = fxc_kl_combined(textureLoad(aux, p, 0).x, textureLoad(src, p, 0).w, P.u[0].x);
    textureStore(out, p, vec4<f32>(select(0.0, 1.0, a > 1e-3 && a < 0.999), 0.0, 0.0, 0.0));
}

// Planes `src` with the edge weight from the processed band `aux`.x; f[0].x = hardness.
@compute @workgroup_size(16, 16)
fn fxc_kl_edge(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = fxc_pixel(gid);
    if (p.x < 0) {
        return;
    }
    let pl = textureLoad(src, p, 0);
    let e = min(textureLoad(aux, p, 0).x * (1.0 + P.f[0].x * 4.0), 1.0);
    textureStore(out, p, vec4<f32>(pl.x, pl.y, e, 0.0));
}

fn fxc_kl_replace(fg: vec3<f32>, c: vec3<f32>, method: u32, rc: vec3<f32>, amt: f32) -> vec3<f32> {
    if (amt <= 0.0) {
        return fg;
    }
    switch method {
        case 1u: { return fg + (c - fg) * amt; }
        case 2u: { return rc; }
        case 3u: {
            let l = luminance(c) / max(luminance(rc), 1e-4);
            return fg + (rc * l - fg) * amt;
        }
        default: { return fg; }
    }
}

// Final pass: `src` = source, `aux` = planes (screen matte, raw, edge).
// u[0] = (primary, others, view); u[1] = (replace method, foreground cc, unpremultiply,
// source alpha mode); f[0] = (screen, balance); f[1] = (despill bias, l0); f[2] = replace colour;
// f[3] = fg cc (sat, contrast, bright); f[4] = edge cc; f[5] = crops (l, r, t, b);
// f[6] = (offset, scale, _); f[7] = (layer w, layer h).
@compute @workgroup_size(16, 16)
fn fxc_kl_final(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = fxc_pixel(gid);
    if (p.x < 0) {
        return;
    }
    let s = textureLoad(src, p, 0);
    let pl = textureLoad(aux, p, 0);
    let ix = P.u[0].xyz;
    let view = P.u[0].w;
    let c = fxc_unpremul(s);
    let sm = pl.x;
    let raw = pl.y;
    let edge = pl.z;
    let a = fxc_kl_combined(sm, s.w, P.u[1].w);
    let screen = P.f[0].xyz;
    let bal = P.f[0].w;
    let dbias = P.f[1].xyz;
    let fg_cc = P.u[1].y != 0u;
    let ar = max(raw, 1e-4);
    var fg = max((c - (1.0 - ar) * screen) / ar, vec3<f32>(0.0));
    let spill = max(fxc_screen_diff(fxc_neutralise(fg, dbias), ix, bal), 0.0);
    fg[ix.x] -= spill / P.f[1].w;
    let intermediate = fg;
    fg = fxc_kl_replace(fg, c, P.u[1].x, P.f[2].xyz, max(sm - raw, 0.0));
    if (fg_cc) {
        fg = fxc_correct(fg, P.f[3].x, P.f[3].y, P.f[3].z);
    }
    if (edge > 0.0) {
        let ce = fxc_correct(fg, P.f[4].x, P.f[4].y, P.f[4].z);
        fg = fg + (ce - fg) * edge;
    }
    var o: vec4<f32>;
    switch view {
        case 1u: { o = vec4<f32>(s.w, s.w, s.w, 1.0); }
        case 2u: {
            var cs = fxc_neutralise(c, dbias);
            let s2 = max(fxc_screen_diff(cs, ix, bal), 0.0);
            cs[ix.x] -= s2;
            if (fg_cc) {
                cs = fxc_correct(cs, P.f[3].x, P.f[3].y, P.f[3].z);
            }
            o = vec4<f32>(cs, 1.0);
        }
        case 3u: { o = vec4<f32>(edge, edge, edge, 1.0); }
        case 4u: { o = vec4<f32>(sm, sm, sm, 1.0); }
        case 5u, 6u: { o = vec4<f32>(0.0, 0.0, 0.0, 1.0); }
        case 7u: { o = vec4<f32>(a, a, a, 1.0); }
        case 8u: {
            var v = 0.5;
            if (a <= 1e-3) {
                v = 0.0;
            } else if (a >= 0.999) {
                v = 1.0;
            }
            o = vec4<f32>(v, v, v, 1.0);
        }
        case 9u: { o = vec4<f32>(intermediate * sm, sm); }
        default: {
            let x = f32(p.x) + 0.5;
            let y = f32(p.y) + 0.5;
            let lx = (x - P.f[6].x) / P.f[6].z / P.f[7].x;
            let ly = (y - P.f[6].y) / P.f[6].z / P.f[7].y;
            let cr = P.f[5];
            let cropped = lx < cr.x || lx > 1.0 - cr.y || ly < cr.z || ly > 1.0 - cr.w;
            if (cropped) {
                o = vec4<f32>(0.0);
            } else if (P.u[1].z != 0u) {
                o = vec4<f32>(fg * a, a);
            } else {
                o = vec4<f32>(fg * a * a, a);
            }
        }
    }
    textureStore(out, p, o);
}

// GPU effects, pixel ports (part C): see src/fx_pixel2.rs. Every entry point is prefixed
// `fp2_` and mirrors the CPU effect operation for operation; positions are pixel centres
// relative to the effect's centre or anchor so f32 keeps its precision.

fn fp2_inside(p: vec2<i32>) -> bool {
    let d = out_dims();
    return p.x < d.x && p.y < d.y;
}

// util::unpremul (colour 0 below alpha 1e-6).
fn fp2_unpremul(p: vec4<f32>) -> vec3<f32> {
    if (p.w > 1e-6) {
        return p.xyz / p.w;
    }
    return vec3<f32>(0.0);
}

// util::smoothstep.
fn fp2_smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    if (abs(e1 - e0) < 1e-9) {
        return select(1.0, 0.0, x < e0);
    }
    let t = clamp((x - e0) / (e1 - e0), 0.0, 1.0);
    return t * t * (3.0 - 2.0 * t);
}

// ---------------------------------------------------------------- CC Cross Blur (blur2::cross_blur)
// src = horizontal blur, aux = vertical blur; u[0].x = transfer mode.

@compute @workgroup_size(16, 16)
fn fp2_cross(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!fp2_inside(p)) {
        return;
    }
    let a = textureLoad(src, p, 0);
    let v = textureLoad(aux, p, 0);
    var o: vec4<f32>;
    switch P.u[0].x {
        case 1u: {
            o = a + v;
        }
        case 2u: {
            o = a + v - a * v;
        }
        case 3u: {
            o = max(a, v);
        }
        default: {
            o = (a + v) * 0.5;
        }
    }
    o.w = clamp(o.w, 0.0, 1.0);
    textureStore(out, p, o);
}

// ---------------------------------------------------------------- CC Radial Blur (blur3::cc_radial_blur)
// u[0].x = type; f[0] = (cx, cy, zoom, arc); f[1].x = quality.

@compute @workgroup_size(16, 16)
fn fp2_radial(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!fp2_inside(p)) {
        return;
    }
    let kind = P.u[0].x;
    let c = P.f[0].xy;
    let zoom = P.f[0].z;
    let arc = P.f[0].w;
    let dx = f32(p.x) + 0.5 - c.x;
    let dy = f32(p.y) + 0.5 - c.y;
    let r = sqrt(dx * dx + dy * dy);
    var span = r * abs(zoom);
    if (kind >= 3u) {
        span = r * abs(arc);
    }
    let n = u32(clamp(ceil(span * P.f[1].x / 50.0), 1.0, 256.0));
    if (n <= 1u) {
        textureStore(out, p, textureLoad(src, p, 0));
        return;
    }
    var acc = vec4<f32>(0.0);
    var wsum = 0.0;
    for (var i = 0u; i < n; i++) {
        let t = f32(i) / f32(n - 1u);
        var s = vec2<f32>(0.0);
        var w = 1.0;
        switch kind {
            case 0u: {
                let k = 1.0 - zoom * t;
                s = vec2<f32>(c.x + dx * k, c.y + dy * k);
            }
            case 1u: {
                let k = 1.0 - zoom * t;
                s = vec2<f32>(c.x + dx * k, c.y + dy * k);
                w = (1.0 - t) + 0.05;
            }
            case 2u: {
                let k = 1.0 + zoom * (t - 0.5);
                s = vec2<f32>(c.x + dx * k, c.y + dy * k);
            }
            default: {
                let a = arc * (t - 0.5);
                let sn = sin(a);
                let co = cos(a);
                if (kind == 4u) {
                    w = (1.0 - abs(2.0 * t - 1.0)) + 0.05;
                }
                s = vec2<f32>(c.x + dx * co - dy * sn, c.y + dx * sn + dy * co);
            }
        }
        acc += sample_bilinear(src, s.x, s.y) * w;
        wsum += w;
    }
    textureStore(out, p, acc / wsum);
}

// ---------------------------------------------------------------- util::gauss_plane
// One box pass (edges repeated) summing each window directly: the CPU's planes accumulate
// in f64, so a running f32 sum would lose the small values (normalised blurs divide them).
// u[0] = (radius, vertical).

@compute @workgroup_size(16, 16)
fn fp2_box(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!fp2_inside(p)) {
        return;
    }
    let r = i32(P.u[0].x);
    var acc = vec4<f32>(0.0);
    for (var i = -r; i <= r; i++) {
        if (P.u[0].y != 0u) {
            acc += tex_get_clamped(src, p.x, p.y + i);
        } else {
            acc += tex_get_clamped(src, p.x + i, p.y);
        }
    }
    textureStore(out, p, acc * (1.0 / f32(2 * r + 1)));
}

// ---------------------------------------------------------------- planes
// One value per pixel into x. u[0] = (kind, property): kind 0 = luminance of the premultiplied
// pixel (blur2::plum); 1 = CC Glass / Plastic height (stylize3::height_field: the property of
// the straight colour times alpha, alpha itself); 2 = CC Mr. Smoothie's flow property (straight);
// 3 = x clamped to [f[0].x, f[0].y] (CC Plastic's cut).

@compute @workgroup_size(16, 16)
fn fp2_plane(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!fp2_inside(p)) {
        return;
    }
    let px = textureLoad(src, p, 0);
    let prop = P.u[0].y;
    var v = 0.0;
    if (P.u[0].x == 0u) {
        v = luminance(px.xyz);
    } else if (P.u[0].x == 3u) {
        v = clamp(px.x, P.f[0].x, P.f[0].y);
    } else {
        let c = fp2_unpremul(px);
        let a = px.w;
        switch prop {
            case 0u: {
                v = c.x;
            }
            case 1u: {
                v = c.y;
            }
            case 2u: {
                v = c.z;
            }
            case 3u: {
                v = a;
            }
            case 4u: {
                v = luminance(c);
            }
            case 5u: {
                v = rgb_to_hsl(c).z;
            }
            case 6u: {
                v = rgb_to_hsl(c).x;
            }
            default: {
                v = rgb_to_hsl(c).y;
            }
        }
        if (P.u[0].x == 1u && prop != 3u) {
            v = v * a;
        }
    }
    textureStore(out, p, vec4<f32>(v, 0.0, 0.0, 0.0));
}

// ---------------------------------------------------------------- CC Vector Blur (blur2::vector_blur)
// aux = softened luminance map (x). u[0].x = type; f[0] = (amount, angle offset).

fn fp2_map(x: i32, y: i32) -> f32 {
    return tex_get_clamped(aux, x, y).x;
}

@compute @workgroup_size(16, 16)
fn fp2_vector(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!fp2_inside(p)) {
        return;
    }
    let d = vec2<i32>(textureDimensions(aux));
    let x = p.x;
    let y = p.y;
    let gx = (fp2_map(min(x + 1, d.x - 1), y) - fp2_map(max(x - 1, 0), y)) * 0.5;
    let gy = (fp2_map(x, min(y + 1, d.y - 1)) - fp2_map(x, max(y - 1, 0))) * 0.5;
    let mag = sqrt(gx * gx + gy * gy);
    let m = fp2_map(x, y);
    let kind = P.u[0].x;
    let amount = P.f[0].x;
    let off = P.f[0].y;
    var ang = 0.0;
    var len = amount;
    if (kind <= 2u) {
        if (mag < 1e-6) {
            textureStore(out, p, textureLoad(src, p, 0));
            return;
        }
        let g = atan2(gy, gx);
        var base = g + 1.5707963267948966;
        if (kind == 2u) {
            base = g;
        }
        if (kind != 1u) {
            len = amount * min(mag * 10.0, 1.0);
        }
        ang = base + off;
    } else {
        ang = m * 6.283185307179586 + off;
        if (kind != 3u) {
            len = amount * m;
        }
    }
    if (len < 0.5) {
        textureStore(out, p, textureLoad(src, p, 0));
        return;
    }
    let dx = cos(ang) * len;
    let dy = sin(ang) * len;
    let n = u32(clamp(ceil(len * 2.0), 2.0, 48.0));
    var acc = vec4<f32>(0.0);
    for (var i = 0u; i < n; i++) {
        let t = f32(i) / f32(n - 1u) - 0.5;
        acc += sample_bilinear_clamped(src, f32(x) + 0.5 + dx * t, f32(y) + 0.5 + dy * t) / f32(n);
    }
    textureStore(out, p, acc);
}

// ---------------------------------------------------------------- per-pixel colour effects
// u[0].x = effect: 0 Cineon Converter, 1 HDR Compander, 2 HDR Highlight Compression,
// 3 CC Overbrights, 4 Color Link (alpha), 5 Color Link (blend), 6 CC Composite.

// utility::Cineon::log_to_lin; c = (offset, knee, white, gamma), r = (internal black, white).
fn fp2_log_to_lin(v: f32, c: vec4<f32>, r: vec2<f32>) -> f32 {
    let code = v * 1023.0;
    var lin = (pow(10.0, (code - c.z) * 0.002 / 0.6) - c.x) / max(1.0 - c.x, 1e-6);
    lin = powz(max(lin, 0.0), 1.7 / max(c.w, 0.01));
    let k = c.y;
    if (k < 1.0 && lin > k) {
        let w = 1.0 - k;
        lin = k + w * (1.0 - exp(-(lin - k) / w));
    }
    return r.x + (r.y - r.x) * lin;
}

// utility::Cineon::lin_to_log.
fn fp2_lin_to_log(v: f32, c: vec4<f32>, r: vec2<f32>) -> f32 {
    let d = r.y - r.x;
    var lin = (v - r.x) / select(d, 1e-6, abs(d) < 1e-6);
    let k = c.y;
    if (k < 1.0 && lin > k) {
        let w = 1.0 - k;
        lin = k - w * log(max(1.0 - (lin - k) / w, 1e-6));
    }
    lin = powz(max(lin, 0.0), max(c.w, 0.01) / 1.7);
    let code = c.z + log(max(lin * (1.0 - c.x) + c.x, 1e-9)) * 0.4342944819032518 * 0.6 / 0.002;
    return code / 1023.0;
}

// utility2::spow.
fn fp2_spow(v: f32, e: f32) -> f32 {
    return sign(v) * powz(abs(v), e);
}

fn fp2_cineon(v: f32) -> f32 {
    let c = P.f[0];
    let r = P.f[1].xy;
    switch P.u[0].y {
        case 0u: {
            return fp2_lin_to_log(v, c, r);
        }
        case 1u: {
            return fp2_log_to_lin(v, c, r);
        }
        default: {
            return fp2_lin_to_log(fp2_log_to_lin(v, c, r), P.f[2], vec2<f32>(0.0, 1.0));
        }
    }
}

@compute @workgroup_size(16, 16)
fn fp2_point(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!fp2_inside(p)) {
        return;
    }
    let px = textureLoad(src, p, 0);
    var o = px;
    switch P.u[0].x {
        // f[0], f[1], f[2]: see fp2_cineon; u[0].y = conversion type.
        case 0u: {
            if (px.w > 0.0) {
                let c = px.xyz / px.w;
                o = vec4<f32>(vec3<f32>(fp2_cineon(c.x), fp2_cineon(c.y), fp2_cineon(c.z)) * px.w, px.w);
            }
        }
        // u[0].y = expand; f[0] = (gain, gamma).
        case 1u: {
            if (px.w > 0.0) {
                let c = fp2_unpremul(px);
                let g = P.f[0].x;
                let e = P.f[0].y;
                var r = vec3<f32>(0.0);
                if (P.u[0].y != 0u) {
                    r = vec3<f32>(fp2_spow(c.x, e), fp2_spow(c.y, e), fp2_spow(c.z, e)) * g;
                } else {
                    r = vec3<f32>(fp2_spow(c.x / g, 1.0 / e), fp2_spow(c.y / g, 1.0 / e), fp2_spow(c.z / g, 1.0 / e));
                }
                o = vec4<f32>(r * px.w, px.w);
            }
        }
        // f[0].x = knee.
        case 2u: {
            if (px.w > 0.0) {
                let c = px.xyz / px.w;
                let m = max(max(c.x, c.y), c.z);
                let knee = P.f[0].x;
                if (m > knee) {
                    let w = 1.0 - knee;
                    let k = (knee + w * (m - knee) / ((m - knee) + w)) / m;
                    o = vec4<f32>(c * k * px.w, px.w);
                }
            }
        }
        // u[0].y = channel; f[0] = highlight colour.
        case 3u: {
            if (px.w > 0.0) {
                let c = fp2_unpremul(px);
                var v = max(max(c.x, c.y), c.z);
                switch P.u[0].y {
                    case 1u: {
                        v = c.x;
                    }
                    case 2u: {
                        v = c.y;
                    }
                    case 3u: {
                        v = c.z;
                    }
                    case 4u: {
                        v = px.w;
                    }
                    case 5u: {
                        v = luminance(c);
                    }
                    default: {}
                }
                var r = vec3<f32>(v * 0.35);
                if (v > 1.0) {
                    r = P.f[0].xyz;
                } else if (v < 0.0) {
                    r = vec3<f32>(1.0) - P.f[0].xyz;
                }
                o = vec4<f32>(r * px.w, px.w);
            }
        }
        // f[0] = (sampled alpha, opacity); u[0].y = stencil.
        case 4u: {
            let c = fp2_unpremul(px);
            let a0 = select(max(px.w, 0.0), px.w, px.w > 1e-6);
            var na = a0 + (P.f[0].x - a0) * P.f[0].y;
            if (P.u[0].y != 0u) {
                na = min(na, a0);
            }
            let a = clamp(na, 0.0, 1.0);
            o = vec4<f32>(c * a, a);
        }
        // f[0] = premultiplied sampled colour; u[0] = (stencil, mode).
        case 5u: {
            o = blend_pixel(P.u[0].z, px, P.f[0], 0.5);
            o.w = clamp(o.w, 0.0, 1.0);
            if (P.u[0].y != 0u) {
                o = vec4<f32>(fp2_unpremul(o) * px.w, px.w);
            }
        }
        // u[0] = (_, composite original, mode, rgb only); f[0].x = opacity.
        default: {
            var source = px;
            if (P.u[1].x != 0u) { source = textureLoad(aux, p, 0); }
            let orig = source * P.f[0].x;
            if (P.u[0].y == 1u) {
                o = blend_pixel(P.u[0].z, orig, px, 0.5);
            } else {
                o = blend_pixel(P.u[0].z, px, orig, 0.5);
            }
            o.w = clamp(o.w, 0.0, 1.0);
            if (P.u[0].w != 0u) {
                o = vec4<f32>(fp2_unpremul(o) * px.w, px.w);
            }
        }
    }
    textureStore(out, p, o);
}

// ---------------------------------------------------------------- CC Block Load (stylize3::block_load)

// Mean colour of every s×s block: one invocation per block (the CPU's summation order).
// u[0].x = s.
@compute @workgroup_size(16, 16)
fn fp2_block_means(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!fp2_inside(p)) {
        return;
    }
    let s = i32(P.u[0].x);
    let d = vec2<i32>(textureDimensions(src));
    var acc = vec4<f32>(0.0);
    var n = 0.0;
    for (var y = p.y * s; y < min((p.y + 1) * s, d.y); y++) {
        for (var x = p.x * s; x < min((p.x + 1) * s, d.x); x++) {
            acc += textureLoad(src, vec2<i32>(x, y), 0);
            n += 1.0;
        }
    }
    if (n > 0.0) {
        acc = acc / n;
    }
    textureStore(out, p, acc);
}

// stylize3::block_at on grid `t` (block size `s`).
fn fp2_block_at(t: texture_2d<f32>, s: u32, x: i32, y: i32, bilinear: bool) -> vec4<f32> {
    let g = vec2<i32>(textureDimensions(t));
    if (!bilinear || s == 1u) {
        return textureLoad(t, vec2<i32>(min(x / i32(s), g.x - 1), min(y / i32(s), g.y - 1)), 0);
    }
    let fx = (f32(x) + 0.5) / f32(s) - 0.5;
    let fy = (f32(y) + 0.5) / f32(s) - 0.5;
    let x0 = floor(fx);
    let y0 = floor(fy);
    let tx = fx - x0;
    let ty = fy - y0;
    let xa = clamp(i32(x0), 0, g.x - 1);
    let xb = clamp(i32(x0 + 1.0), 0, g.x - 1);
    let ya = clamp(i32(y0), 0, g.y - 1);
    let yb = clamp(i32(y0 + 1.0), 0, g.y - 1);
    let a = textureLoad(t, vec2<i32>(xa, ya), 0);
    let b = textureLoad(t, vec2<i32>(xb, ya), 0);
    let c = textureLoad(t, vec2<i32>(xa, yb), 0);
    let e = textureLoad(t, vec2<i32>(xb, yb), 0);
    let top = a + (b - a) * tx;
    let bot = c + (e - c) * tx;
    return top + (bot - top) * ty;
}

// src = the new level's grid, aux = the previous level's grid (or the new one).
// u[0] = (new block size, old block size, flags: 1 scanlines, 2 smoothing, 4 bilinear,
// 8 previous level exists, 16 start cleared); f[0] = (frac, rows of the new level).
@compute @workgroup_size(16, 16)
fn fp2_block_load(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!fp2_inside(p)) {
        return;
    }
    let s_new = P.u[0].x;
    let flags = P.u[0].z;
    let scan = (flags & 1u) != 0u;
    let smoothing = (flags & 2u) != 0u;
    let bilinear = (flags & 4u) != 0u;
    let frac = P.f[0].x;
    let soft = select(0.0, 1.5, smoothing);
    var t = 0.0;
    if (scan) {
        let front = frac * (P.f[0].y + soft) - soft * 0.5;
        let r = f32(p.y / i32(s_new)) + 0.5;
        if (soft > 0.0) {
            t = fp2_smoothstep(r - soft * 0.5, r + soft * 0.5, front);
        } else if (r < front) {
            t = 1.0;
        }
    } else if (smoothing) {
        t = frac;
    }
    var n = vec4<f32>(0.0);
    if (t > 0.0) {
        n = fp2_block_at(src, s_new, p.x, p.y, bilinear);
    }
    var prev = vec4<f32>(0.0);
    if ((flags & 8u) != 0u) {
        prev = fp2_block_at(aux, P.u[0].y, p.x, p.y, bilinear);
    } else if ((flags & 16u) == 0u) {
        prev = fp2_block_at(src, s_new, p.x, p.y, bilinear);
    }
    var o = prev;
    if (t >= 1.0) {
        o = n;
    } else if (t > 0.0) {
        o = prev + (n - prev) * t;
    }
    textureStore(out, p, o);
}

// ---------------------------------------------------------------- CC Burn Film (stylize3::burn_film)
// u[0].x = seed + 17; f[0] = (cx, cy, diagonal, feature size); f[1] = (threshold, band).

@compute @workgroup_size(16, 16)
fn fp2_burn(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!fp2_inside(p)) {
        return;
    }
    let px = textureLoad(src, p, 0);
    let fx = f32(p.x) + 0.5;
    let fy = f32(p.y) + 0.5;
    let dx = fx - P.f[0].x;
    let dy = fy - P.f[0].y;
    let d = sqrt(dx * dx + dy * dy) / P.f[0].z;
    let n = fxs_fbm(fx / P.f[0].w, fy / P.f[0].w, 0.0, P.u[0].x, 4.0);
    let f = n * 0.65 + (1.0 - min(d, 1.0)) * 0.35;
    let th = P.f[1].x;
    let band = P.f[1].y;
    var o = px;
    if (f > th + band) {
        o = vec4<f32>(0.0);
    } else if (f > th - band * 2.0) {
        let c = fp2_unpremul(px);
        let k = fp2_smoothstep(th - band * 2.0, th, f);
        let hot = fp2_smoothstep(th, th + band, f);
        let chr = 1.0 - k * 0.85;
        var r = c * chr;
        r = vec3<f32>(r.x + (1.0 - r.x) * hot * 0.9, r.y + (0.45 - r.y) * hot * 0.9, r.z * (1.0 - hot));
        let na = px.w * (1.0 - hot * 0.6);
        o = vec4<f32>(r * na, na);
    }
    textureStore(out, p, o);
}

// ---------------------------------------------------------------- CC Glass / CC Plastic (stylize3)
// aux = height field (x). u[0] = (glass, point light); f[0] = (height scale k, displacement,
// height != 0, light height); f[1] = (intensity, ambient, diffuse, specular);
// f[2] = (shininess, metal, light x, light y); f[3] = light colour; f[4] = distant direction.

fn fp2_h(x: i32, y: i32) -> f32 {
    return tex_get_clamped(aux, x, y).x;
}

// stylize3::BumpLight::shade.
fn fp2_shade(c: vec3<f32>, n: vec3<f32>, l: vec3<f32>) -> vec3<f32> {
    let intensity = P.f[1].x;
    let ambient = P.f[1].y;
    let diffuse = P.f[1].z;
    let specular = P.f[1].w;
    let ndl = max(dot(n, l), 0.0);
    let flat_z = max(l.z, 0.0);
    let base = ambient + diffuse * flat_z;
    var lit = 1.0;
    if (base > 1e-4) {
        lit = (ambient + diffuse * ndl) / base;
    }
    lit = 1.0 + (lit - 1.0) * intensity;
    let hv = vec3<f32>(l.x, l.y, l.z + 1.0);
    let hl = max(sqrt(hv.x * hv.x + hv.y * hv.y + hv.z * hv.z), 1e-6);
    let ndh = max((n.x * hv.x + n.y * hv.y + n.z * hv.z) / hl, 0.0);
    let spec = specular * intensity * powz(ndh, P.f[2].x);
    let lc = P.f[3].xyz;
    let metal = P.f[2].y;
    let hl_col = lc * (1.0 - metal) + c * lc * metal;
    return c * lit * (vec3<f32>(1.0) + (lc - vec3<f32>(1.0)) * min(intensity, 1.0)) + spec * hl_col;
}

@compute @workgroup_size(16, 16)
fn fp2_bump(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!fp2_inside(p)) {
        return;
    }
    let px = textureLoad(src, p, 0);
    let glass = P.u[0].x != 0u;
    if (px.w <= 0.0) {
        textureStore(out, p, select(px, vec4<f32>(0.0), glass));
        return;
    }
    let k = P.f[0].x;
    let gx = (fp2_h(p.x + 1, p.y) - fp2_h(p.x - 1, p.y)) * 0.5 * k;
    let gy = (fp2_h(p.x, p.y + 1) - fp2_h(p.x, p.y - 1)) * 0.5 * k;
    let ln = sqrt(gx * gx + gy * gy + 1.0);
    let n = vec3<f32>(-gx / ln, -gy / ln, 1.0 / ln);
    let fx = f32(p.x) + 0.5;
    let fy = f32(p.y) + 0.5;
    var l = P.f[4].xyz;
    if (P.u[0].y != 0u) {
        let v = vec3<f32>(P.f[2].z - fx, P.f[2].w - fy, max(P.f[0].w, 1.0));
        let vl = max(sqrt(v.x * v.x + v.y * v.y + v.z * v.z), 1e-6);
        l = v / vl;
    }
    var c = fp2_unpremul(px);
    if (glass) {
        let disp = P.f[0].y;
        if (disp != 0.0 && P.f[0].z != 0.0) {
            let s = sample_bilinear_clamped(src, fx + n.x * disp, fy + n.y * disp);
            if (s.w > 0.0) {
                c = fp2_unpremul(s);
            }
        }
    }
    let o = fp2_shade(c, n, l);
    textureStore(out, p, vec4<f32>(o * px.w, px.w));
}

// ---------------------------------------------------------------- CC HexTile (stylize3::hextile)
// u[0].x = render mode; f[0] = (cx, cy, radius, smearing); f[1] = (sin, cos).

@compute @workgroup_size(16, 16)
fn fp2_hextile(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!fp2_inside(p)) {
        return;
    }
    let cx = P.f[0].x;
    let cy = P.f[0].y;
    let r = P.f[0].z;
    let smear = P.f[0].w;
    let sn = P.f[1].x;
    let cs = P.f[1].y;
    let mode = P.u[0].x;
    let dx = f32(p.x) + 0.5 - cx;
    let dy = f32(p.y) + 0.5 - cy;
    let u = dx * cs + dy * sn;
    let v = -dx * sn + dy * cs;
    // stylize3::hex_cell (cube rounding).
    let q = (0.5773502691896257 * u - v / 3.0) / r;
    let rr = (2.0 / 3.0 * v) / r;
    // Evaluate the third cube coordinate symmetrically with q so the centerline
    // tie is not broken by subtracting separately rounded coordinates.
    let ccy = (-0.5773502691896257 * u - v / 3.0) / r;
    var rx = round_away(q);
    let ry = round_away(ccy);
    var rz = round_away(rr);
    let ddx = abs(rx - q);
    let ddy = abs(ry - ccy);
    let ddz = abs(rz - rr);
    if (ddx > ddy && ddx > ddz) {
        rx = -ry - rz;
    } else if (ddy <= ddz) {
        rz = -rx - ry;
    }
    let hx = r * 1.7320508075688772 * (rx + rz / 2.0);
    let hy = r * 1.5 * rz;
    var lu = u - hx;
    var lv = v - hy;
    let odd = imod(i32(rx) + i32(rz), 2) == 1;
    if ((mode == 1u || mode == 2u) && odd) {
        lu = -lu;
    }
    if (mode == 2u && odd) {
        lv = -lv;
    }
    let ox = lu * cs - lv * sn;
    let oy = lu * sn + lv * cs;
    let tx = cx + ox;
    let ty = cy + oy;
    let sx = tx + (f32(p.x) + 0.5 - tx) * smear;
    let sy = ty + (f32(p.y) + 0.5 - ty) * smear;
    textureStore(out, p, sample_bilinear_clamped(src, sx, sy));
}

// ---------------------------------------------------------------- CC Mr. Smoothie (stylize3::smoothie)
// aux = flow plane (x). f[0] = (ax, ay, bx, by); f[1] = (phase, loops).

fn fp2_pal(i: f32) -> vec4<f32> {
    let t = i / 255.0;
    let a = P.f[0].xy;
    let b = P.f[0].zw;
    let s = sample_bilinear_clamped(src, a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t);
    return vec4<f32>(fp2_unpremul(s), 1.0);
}

@compute @workgroup_size(16, 16)
fn fp2_smoothie(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!fp2_inside(p)) {
        return;
    }
    let px = textureLoad(src, p, 0);
    if (px.w <= 0.0) {
        textureStore(out, p, px);
        return;
    }
    let v = textureLoad(aux, p, 0).x;
    var t = fract_euclid(v * P.f[1].y + P.f[1].x);
    if (t < 0.5) {
        t = t * 2.0;
    } else {
        t = 2.0 - t * 2.0;
    }
    let f = t * 255.0;
    let i = min(floor(f), 254.0);
    let a = fp2_pal(i);
    let b = fp2_pal(i + 1.0);
    let c = a + (b - a) * (f - i);
    textureStore(out, p, vec4<f32>(c.xyz * px.w, px.w));
}

// ---------------------------------------------------------------- Inner/Outer Key (keying2)

// T = (straight colour, trimap) from src = the layer and aux = the trimap (x).
@compute @workgroup_size(16, 16)
fn fp2_io_prep(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!fp2_inside(p)) {
        return;
    }
    textureStore(out, p, vec4<f32>(fp2_unpremul(textureLoad(src, p, 0)), textureLoad(aux, p, 0).x));
}

// A known region (u[0].x: 0 foreground, 1 background) of T: (colour × known, known).
@compute @workgroup_size(16, 16)
fn fp2_io_known(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!fp2_inside(p)) {
        return;
    }
    let t = textureLoad(src, p, 0);
    var m = select(0.0, 1.0, t.w > 0.75);
    if (P.u[0].x == 1u) {
        m = select(0.0, 1.0, t.w < 0.25);
    }
    textureStore(out, p, vec4<f32>(t.xyz * m, m));
}

// src = blurred foreground, aux = blurred background, data = T's rows (row length u[0].x):
// the estimated matte with the decontaminated colours (colour, alpha).
@compute @workgroup_size(16, 16)
fn fp2_io_est(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!fp2_inside(p)) {
        return;
    }
    let at = (u32(p.y) * P.u[0].x + u32(p.x)) * 4u;
    let c = vec3<f32>(data[at], data[at + 1u], data[at + 2u]);
    let tri = data[at + 3u];
    var a = tri;
    if (!(tri > 0.25 && tri < 0.75)) {
        textureStore(out, p, vec4<f32>(c, a));
        return;
    }
    let fs = textureLoad(src, p, 0);
    let bs = textureLoad(aux, p, 0);
    let fc = fs.xyz / max(fs.w, 1e-6);
    let bc = bs.xyz / max(bs.w, 1e-6);
    let d = fc - bc;
    let dd = d.x * d.x + d.y * d.y + d.z * d.z;
    if (dd < 1e-6) {
        a = 0.5;
    } else {
        a = clamp(((c.x - bc.x) * d.x + (c.y - bc.y) * d.y + (c.z - bc.z) * d.z) / dd, 0.0, 1.0);
    }
    var col = c;
    if (a > 0.02) {
        col = clamp((c - (1.0 - a) * bc) / a, vec3<f32>(0.0), vec3<f32>(1.0));
    }
    textureStore(out, p, vec4<f32>(col, a));
}

// The matte (src.x, or src.w with u[0].y) after the Cleanup strokes: data = targets (u[0].x of
// them) then each stroke's coverage plane (negative = untouched).
@compute @workgroup_size(16, 16)
fn fp2_io_strokes(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!fp2_inside(p)) {
        return;
    }
    let s = textureLoad(src, p, 0);
    var a = select(s.x, s.w, P.u[0].y != 0u);
    let n = P.u[0].x;
    let d = out_dims();
    let plane = u32(d.x * d.y);
    let i = u32(p.y * d.x + p.x);
    for (var k = 0u; k < n; k++) {
        let c = data[n + k * plane + i];
        if (c >= 0.0) {
            a += (data[k] - a) * c;
        }
    }
    textureStore(out, p, vec4<f32>(a, 0.0, 0.0, 0.0));
}

// src = the layer, aux = the final matte (x), data = estimated colours' rows (row length u[0].x,
// colours used when u[0].y). u[0].z = invert; f[0] = (threshold, blend with original).
@compute @workgroup_size(16, 16)
fn fp2_io_final(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!fp2_inside(p)) {
        return;
    }
    let px = textureLoad(src, p, 0);
    var m = textureLoad(aux, p, 0).x;
    let thr = P.f[0].x;
    if (thr > 0.0 && m < thr) {
        m = 0.0;
    }
    if (P.u[0].z != 0u) {
        m = 1.0 - m;
    }
    var c = fp2_unpremul(px);
    if (P.u[0].y != 0u) {
        let at = (u32(p.y) * P.u[0].x + u32(p.x)) * 4u;
        c = vec3<f32>(data[at], data[at + 1u], data[at + 2u]);
    }
    let a = max(px.w, 0.0);
    let keyed = vec4<f32>(c * (a * m), a * m);
    textureStore(out, p, keyed + (px - keyed) * P.f[0].y);
}

// ---------------------------------------------------------------- CC Simple Wire Removal (keying2::wire_removal)
// u[0].x = style; f[0] = (ax, ay, nx, ny); f[1] = (hw thickness, soft width, mirror, segment length).

fn fp2_wire_pos(base: vec2<f32>, nrm: vec2<f32>, s: f32) -> vec4<f32> {
    return sample_bilinear_clamped(src, base.x + nrm.x * s, base.y + nrm.y * s);
}

@compute @workgroup_size(16, 16)
fn fp2_wire(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!fp2_inside(p)) {
        return;
    }
    let px = textureLoad(src, p, 0);
    let a = P.f[0].xy;
    let nrm = P.f[0].zw;
    let hw = P.f[1].x;
    let soft_w = P.f[1].y;
    let mirror = P.f[1].z;
    let fx = f32(p.x) + 0.5;
    let fy = f32(p.y) + 0.5;
    let q = vec2<f32>(fx - a.x, fy - a.y);
    let d = q.x * nrm.x + q.y * nrm.y;
    let ad = abs(d);
    // The wire is the segment A to B: the tangent is the normal turned back by 90 degrees.
    let along = q.x * nrm.y - q.y * nrm.x;
    if (along < 0.0 || along > P.f[1].w || ad > hw + soft_w) {
        textureStore(out, p, px);
        return;
    }
    let base = vec2<f32>(fx - d * nrm.x, fy - d * nrm.y);
    let edge = hw + 1.0;
    var repl: vec4<f32>;
    switch P.u[0].x {
        case 0u: {
            let t = clamp((d + edge) / (2.0 * edge), 0.0, 1.0);
            let l = fp2_wire_pos(base, nrm, -edge);
            repl = l + (fp2_wire_pos(base, nrm, edge) - l) * t;
        }
        case 1u, 2u: {
            var s = d - 2.0 * (hw - ad) - 1.0;
            if (d >= 0.0) {
                s = d + 2.0 * (hw - ad) + 1.0;
            }
            let sh = fp2_wire_pos(base, nrm, s);
            repl = sh;
            if (mirror > 0.0) {
                repl = sh + (fp2_wire_pos(base, nrm, -s) - sh) * (mirror * 0.5);
            }
        }
        default: {
            var sx = 0.0;
            if (abs(nrm.x) > 1e-6) {
                sx = (hw - ad + 1.0) / abs(nrm.x);
            }
            let dir = select(-1.0, 1.0, d * nrm.x >= 0.0);
            let sh = sample_bilinear_clamped(src, fx + dir * sx, fy);
            let mi = sample_bilinear_clamped(src, fx - dir * sx, fy);
            repl = sh;
            if (mirror > 0.0) {
                repl = sh + (mi - sh) * (mirror * 0.5);
            }
        }
    }
    var k = 1.0;
    if (ad > hw) {
        k = 1.0 - fp2_smoothstep(hw, hw + soft_w, ad);
    }
    textureStore(out, p, px + (repl - px) * k);
}

// ---------------------------------------------------------------- Basic 3D (obsolete::basic_3d)
// u[0] = (specular, wireframe); f[0] = (cx, cy, focal, distance); f[1] = ux; f[2] = uy;
// f[3] = normal; f[4] = light; f[5] = (hw width, hw height).

@compute @workgroup_size(16, 16)
fn fp2_basic3d(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!fp2_inside(p)) {
        return;
    }
    let cx = P.f[0].x;
    let cy = P.f[0].y;
    let f = P.f[0].z;
    let dist = P.f[0].w;
    let ux = P.f[1].xyz;
    let uy = P.f[2].xyz;
    let n = P.f[3].xyz;
    let d = vec3<f32>(f32(p.x) + 0.5 - cx, f32(p.y) + 0.5 - cy, f);
    let denom = dot(d, n);
    if (abs(denom) < 1e-9) {
        textureStore(out, p, vec4<f32>(0.0));
        return;
    }
    // Eye at (0, 0, -f), plane through (0, 0, dist).
    let t = (dist + f) * n.z / denom;
    if (t <= 0.0) {
        textureStore(out, p, vec4<f32>(0.0));
        return;
    }
    let hit = vec3<f32>(d.x * t, d.y * t, -f + d.z * t - dist);
    let u = dot(hit, ux);
    let v = dot(hit, uy);
    var c = sample_bilinear(src, cx + u, cy + v);
    if (P.u[0].x != 0u && c.w > 0.0) {
        let dv = d / sqrt(dot(d, d));
        let k = 2.0 * dot(dv, n);
        let r = dv - k * n;
        let s = powz(max(-dot(r, P.f[4].xyz), 0.0), 40.0);
        let a = c.w;
        c = vec4<f32>(c.xyz + s * a * max(vec3<f32>(1.0) - c.xyz / max(a, 1e-6), vec3<f32>(0.0)), a);
    }
    if (P.u[0].y != 0u) {
        let hw = P.f[5].x;
        let hh = P.f[5].y;
        let e = min(abs(abs(u) - hw), abs(abs(v) - hh));
        if (e < 0.75 && abs(u) <= hw + 0.75 && abs(v) <= hh + 0.75) {
            c = vec4<f32>(1.0);
        }
    }
    textureStore(out, p, c);
}

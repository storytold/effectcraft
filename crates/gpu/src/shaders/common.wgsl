// EffectCraft GPU compositor: shared bindings and helpers.
//
// Every kernel uses the same bind group: a uniform block of parameters, two input textures
// (premultiplied RGBA f32, read with textureLoad), one output storage texture and a read-only
// storage buffer of extra data (curve tables). Each helper mirrors a CPU function in
// effectcraft-raster / effectcraft-color operation for operation, so results match the CPU
// reference to float rounding.

struct Params {
    u: array<vec4<u32>, 4>,
    f: array<vec4<f32>, 12>,
}

@group(0) @binding(0) var<uniform> P: Params;
@group(0) @binding(1) var src: texture_2d<f32>;
@group(0) @binding(2) var aux: texture_2d<f32>;
@group(0) @binding(4) var<storage, read> data: array<f32>;

// `a` mod `n` in [0, n) for n > 0 (floored). Never `%` on a negative operand: WGSL's `%` truncates,
// but naga's GLSL output is plain `%`, undefined for negative operands (wrong on Mesa's GL).
fn imod(a: i32, n: i32) -> i32 {
    if (a >= 0) {
        return a % n;
    }
    return n - 1 - (-(a + 1)) % n;
}

// asin / acos to f32 precision on every backend (Cephes' asinf polynomial). WGSL leaves their
// precision to the implementation, and Mesa's GL drivers use a coarse approximation (errors near
// 1e-4 rad), which shows in every effect that maps through a sphere or a lens.
fn asin_p(x: f32) -> f32 {
    let a = abs(x);
    let big = a > 0.5;
    let z = select(a * a, 0.5 * (1.0 - a), big);
    let s = select(a, sqrt(z), big);
    var p = ((((4.2163199048e-2 * z + 2.4181311049e-2) * z + 4.5470025998e-2) * z + 7.4953002686e-2) * z + 1.6666752422e-1) * z * s + s;
    if (big) {
        p = 1.5707963267948966 - 2.0 * p;
    }
    return select(p, -p, x < 0.0);
}

fn acos_p(x: f32) -> f32 {
    if (x > 0.5) {
        return 2.0 * asin_p(sqrt(0.5 * (1.0 - x)));
    }
    if (x < -0.5) {
        return 3.141592653589793 - 2.0 * asin_p(sqrt(0.5 * (1.0 + x)));
    }
    return 1.5707963267948966 - asin_p(x);
}

// Some hardware's native trig instructions saturate cos near a full turn and
// have errors large enough to move particles and sphere samples visibly. Reduce
// to [-pi/4, pi/4] with a split pi/2, then evaluate the Taylor polynomials with
// explicit fused operations. The effect angles use this bounded range; retain
// the builtins for non-finite or very large arguments rather than cast them.
fn sincos_p(x: f32) -> vec2<f32> {
    if (!(abs(x) <= 8192.0)) {
        return vec2<f32>(sin(x), cos(x));
    }
    let k = round(x * 0.6366197723675814);
    // Keep the coarse reduction in integers. Some drivers reassociate even
    // explicit fma chains into x - k*pi/2, losing low bits for large angles.
    // 3216/2048 is exact; the bounded scaled input and product fit in i32.
    let scaled = x * 2048.0;
    let whole = floor(scaled);
    var r = f32(i32(whole) - i32(k) * 3216) / 2048.0;
    r += (scaled - whole) / 2048.0;
    r = fma(-k, 0.0004838267893064022, r);
    r = fma(-k, 5.564431554416771e-12, r);
    let z = r * r;
    var sp = fma(z, -1.0 / 39916800.0, 1.0 / 362880.0);
    sp = fma(z, sp, -1.0 / 5040.0);
    sp = fma(z, sp, 1.0 / 120.0);
    sp = fma(z, sp, -1.0 / 6.0);
    let sn = fma(r * z, sp, r);
    var cp = fma(z, -1.0 / 3628800.0, 1.0 / 40320.0);
    cp = fma(z, cp, -1.0 / 720.0);
    cp = fma(z, cp, 1.0 / 24.0);
    cp = fma(z, cp, -0.5);
    let cs = fma(z, cp, 1.0);
    switch imod(i32(k), 4) {
        case 1: { return vec2<f32>(cs, -sn); }
        case 2: { return vec2<f32>(-sn, -cs); }
        case 3: { return vec2<f32>(-cs, sn); }
        default: { return vec2<f32>(sn, cs); }
    }
}

fn sin_p(x: f32) -> f32 { return sincos_p(x).x; }
fn cos_p(x: f32) -> f32 { return sincos_p(x).y; }

// Correctly-rounded positive x/a, clamped to [0,1]. Native division and fma
// need not have IEEE precision on every shader backend. Discontinuous colour
// effects need the exact CPU f32 value, not just an approximation within an ulp.
// Long division uses only bounded 24-bit significands (at most 24 iterations).
fn unit_div(x: f32, a: f32) -> f32 {
    if (x <= 0.0 || a <= 0.0) { return 0.0; }
    if (x >= a) { return 1.0; }
    if (a == 1.0) { return x; }
    let xb = bitcast<u32>(x);
    let ab = bitcast<u32>(a);
    var nx = xb & 0x7fffffu;
    var na = ab & 0x7fffffu;
    var ex = i32((xb >> 23u) & 255u) - 127;
    var ea = i32((ab >> 23u) & 255u) - 127;
    if (ex == -127) {
        let shift = 23u - firstLeadingBit(nx);
        nx <<= shift;
        ex = -126 - i32(shift);
    } else { nx |= 0x800000u; }
    if (ea == -127) {
        let shift = 23u - firstLeadingBit(na);
        na <<= shift;
        ea = -126 - i32(shift);
    } else { na |= 0x800000u; }
    var exponent = ex - ea;
    if (nx < na) { nx <<= 1u; exponent -= 1; }
    var rem = nx - na;
    if (exponent < -150) { return 0.0; }
    if (exponent == -150) { return bitcast<f32>(select(0u, 1u, rem != 0u)); }
    let fraction = u32(min(23, exponent + 149));
    var q = 1u;
    for (var i = 0u; i <= fraction; i++) {
        rem <<= 1u;
        q <<= 1u;
        if (rem >= na) { rem -= na; q |= 1u; }
    }
    let guard = q & 1u;
    q >>= 1u;
    if (guard != 0u && (rem != 0u || (q & 1u) != 0u)) { q += 1u; }
    if (exponent < -126) { return bitcast<f32>(q); }
    if (q == 0x1000000u) { q >>= 1u; exponent += 1; }
    return bitcast<f32>((u32(exponent + 127) << 23u) | (q & 0x7fffffu));
}

// ---------------------------------------------------------------- sampling (raster::Image)

fn tex_get(t: texture_2d<f32>, x: i32, y: i32) -> vec4<f32> {
    let d = vec2<i32>(textureDimensions(t));
    if (x < 0 || y < 0 || x >= d.x || y >= d.y) {
        return vec4<f32>(0.0);
    }
    return textureLoad(t, vec2<i32>(x, y), 0);
}

fn tex_get_clamped(t: texture_2d<f32>, x: i32, y: i32) -> vec4<f32> {
    let d = vec2<i32>(textureDimensions(t));
    return textureLoad(t, vec2<i32>(clamp(x, 0, d.x - 1), clamp(y, 0, d.y - 1)), 0);
}

// Image::sample_bilinear (pixel centres at +0.5; transparent outside).
fn sample_bilinear(t: texture_2d<f32>, x: f32, y: f32) -> vec4<f32> {
    let fx = x - 0.5;
    let fy = y - 0.5;
    let x0 = floor(fx);
    let y0 = floor(fy);
    let tx = fx - x0;
    let ty = fy - y0;
    let xi = i32(x0);
    let yi = i32(y0);
    let a = tex_get(t, xi, yi);
    let b = tex_get(t, xi + 1, yi);
    let c = tex_get(t, xi, yi + 1);
    let d = tex_get(t, xi + 1, yi + 1);
    let top = a + (b - a) * tx;
    let bot = c + (d - c) * tx;
    return top + (bot - top) * ty;
}

// Image::sample_bilinear_clamped (edge pixels repeat).
fn sample_bilinear_clamped(t: texture_2d<f32>, x: f32, y: f32) -> vec4<f32> {
    let fx = x - 0.5;
    let fy = y - 0.5;
    let x0 = floor(fx);
    let y0 = floor(fy);
    let tx = fx - x0;
    let ty = fy - y0;
    let xi = i32(x0);
    let yi = i32(y0);
    let a = tex_get_clamped(t, xi, yi);
    let b = tex_get_clamped(t, xi + 1, yi);
    let c = tex_get_clamped(t, xi, yi + 1);
    let d = tex_get_clamped(t, xi + 1, yi + 1);
    let top = a + (b - a) * tx;
    let bot = c + (d - c) * tx;
    return top + (bot - top) * ty;
}

fn cubic_w(t: f32) -> vec4<f32> {
    let t2 = t * t;
    let t3 = t2 * t;
    return vec4<f32>(-0.5 * t3 + t2 - 0.5 * t, 1.5 * t3 - 2.5 * t2 + 1.0, -1.5 * t3 + 2.0 * t2 + 0.5 * t, 0.5 * t3 - 0.5 * t2);
}

// Image::sample_bicubic (Catmull-Rom; negatives clamped after ringing).
fn sample_bicubic(t: texture_2d<f32>, x: f32, y: f32) -> vec4<f32> {
    let fx = x - 0.5;
    let fy = y - 0.5;
    let x0 = floor(fx);
    let y0 = floor(fy);
    let wx = cubic_w(fx - x0);
    let wy = cubic_w(fy - y0);
    let xi = i32(x0);
    let yi = i32(y0);
    var o = vec4<f32>(0.0);
    for (var j = 0; j < 4; j++) {
        for (var i = 0; i < 4; i++) {
            let p = tex_get(t, xi - 1 + i, yi - 1 + j);
            o += p * (wx[i] * wy[j]);
        }
    }
    return max(o, vec4<f32>(0.0));
}

// 0 nearest, 1 bilinear, 2 bicubic (raster::Sampling).
fn sample(t: texture_2d<f32>, sampling: u32, x: f32, y: f32) -> vec4<f32> {
    if (sampling == 0u) {
        return tex_get(t, i32(floor(x)), i32(floor(y)));
    }
    if (sampling == 2u) {
        return sample_bicubic(t, x, y);
    }
    return sample_bilinear(t, x, y);
}

// raster::hash_noise.
fn hash_noise(x: u32, y: u32, seed: u32) -> f32 {
    var h = (x * 0x8da6b343u) ^ (y * 0xd8163841u) ^ (seed * 0xcb1ab31fu);
    h = h ^ (h >> 13u);
    h = h * 0x5bd1e995u;
    h = h ^ (h >> 15u);
    return f32(h & 0x00ffffffu) / 16777216.0;
}

// powf for x >= 0 (0^y = 0 for y > 0; WGSL leaves pow(0, y) to the implementation).
fn powz(x: f32, y: f32) -> f32 {
    if (x <= 0.0) {
        return select(0.0, 1.0, y == 0.0);
    }
    return pow(x, y);
}

// Rust's f32::round (half away from zero; WGSL's round() is half to even).
fn round_away(x: f32) -> f32 {
    let t = trunc(x);
    if (abs(x - t) >= 0.5) {
        return t + sign(x);
    }
    return t;
}

// f32::rem_euclid(1.0).
fn fract_euclid(x: f32) -> f32 {
    return x - floor(x);
}

fn luminance(c: vec3<f32>) -> f32 {
    return 0.2126 * c.x + 0.7152 * c.y + 0.0722 * c.z;
}

// ---------------------------------------------------------------- blend modes (color::blend)
// Mode numbers are indices into BlendMode::ALL.

fn burn(cb: f32, cs: f32) -> f32 {
    if (cb >= 1.0) {
        return 1.0;
    }
    if (cs <= 0.0) {
        return 0.0;
    }
    return 1.0 - min((1.0 - cb) / cs, 1.0);
}

fn dodge(cb: f32, cs: f32) -> f32 {
    if (cb <= 0.0) {
        return 0.0;
    }
    if (cs >= 1.0) {
        return 1.0;
    }
    return min(cb / (1.0 - cs), 1.0);
}

fn hard_light(cb: f32, cs: f32) -> f32 {
    if (cs <= 0.5) {
        return cb * 2.0 * cs;
    }
    let s = 2.0 * cs - 1.0;
    return cb + s - cb * s;
}

fn soft_light(cb: f32, cs: f32) -> f32 {
    if (cs <= 0.5) {
        return cb - (1.0 - 2.0 * cs) * cb * (1.0 - cb);
    }
    var d: f32;
    if (cb <= 0.25) {
        d = ((16.0 * cb - 12.0) * cb + 4.0) * cb;
    } else {
        d = sqrt(max(cb, 0.0));
    }
    return cb + (2.0 * cs - 1.0) * (d - cb);
}

fn vivid(cb: f32, cs: f32) -> f32 {
    if (cs <= 0.5) {
        return burn(cb, 2.0 * cs);
    }
    return dodge(cb, 2.0 * (cs - 0.5));
}

fn separable(mode: u32, cb: f32, cs: f32) -> f32 {
    switch mode {
        case 3u: { return min(cb, cs); }
        case 4u: { return cb * cs; }
        case 5u: { return burn(cb, cs); }
        case 6u: {
            if (cs <= 0.0) {
                return 0.0;
            }
            return 1.0 - (1.0 - cb) / cs;
        }
        case 7u: { return max(cb + cs - 1.0, 0.0); }
        case 9u, 14u: { return cb + cs; }
        case 10u: { return max(cb, cs); }
        case 11u: { return cb + cs - cb * cs; }
        case 12u: { return dodge(cb, cs); }
        case 13u: {
            if (cs >= 1.0) {
                return 1.0;
            }
            return min(cb / (1.0 - cs), 1.0);
        }
        case 16u: { return hard_light(cs, cb); }
        case 17u: { return soft_light(cb, cs); }
        case 18u: { return hard_light(cb, cs); }
        case 19u: { return clamp(cb + 2.0 * cs - 1.0, 0.0, 1.0); }
        case 20u: { return vivid(cb, cs); }
        case 21u: {
            if (cs <= 0.5) {
                return min(cb, 2.0 * cs);
            }
            return max(cb, 2.0 * cs - 1.0);
        }
        case 22u: {
            if (cb + cs >= 1.0) {
                return 1.0;
            }
            return 0.0;
        }
        case 23u, 24u: { return abs(cb - cs); }
        case 25u: { return cb + cs - 2.0 * cb * cs; }
        case 26u: { return max(cb - cs, 0.0); }
        case 27u: {
            if (cs <= 0.0) {
                if (cb > 0.0) {
                    return 1.0;
                }
                return 0.0;
            }
            return cb / cs;
        }
        default: { return cs; }
    }
}

fn blum(c: vec3<f32>) -> f32 {
    return 0.3 * c.x + 0.59 * c.y + 0.11 * c.z;
}

fn clip_color(c: vec3<f32>) -> vec3<f32> {
    let l = blum(c);
    let n = min(min(c.x, c.y), c.z);
    let x = max(max(c.x, c.y), c.z);
    var o = c;
    if (n < 0.0) {
        o = l + (o - l) * l / max(l - n, 1e-9);
    }
    if (x > 1.0) {
        o = l + (o - l) * (1.0 - l) / max(x - l, 1e-9);
    }
    return o;
}

fn set_lum(c: vec3<f32>, l: f32) -> vec3<f32> {
    let d = l - blum(c);
    return clip_color(c + d);
}

fn bsat(c: vec3<f32>) -> f32 {
    return max(max(c.x, c.y), c.z) - min(min(c.x, c.y), c.z);
}

fn set_sat(c: vec3<f32>, s: f32) -> vec3<f32> {
    let mx = max(max(c.x, c.y), c.z);
    let mn = min(min(c.x, c.y), c.z);
    if (mx - mn <= 1e-9) {
        return vec3<f32>(0.0);
    }
    return (c - mn) * s / (mx - mn);
}

fn non_separable(mode: u32, cb: vec3<f32>, cs: vec3<f32>) -> vec3<f32> {
    switch mode {
        case 28u: { return set_lum(set_sat(cs, bsat(cb)), blum(cb)); }
        case 29u: { return set_lum(set_sat(cb, bsat(cs)), blum(cb)); }
        case 30u: { return set_lum(cs, blum(cb)); }
        case 31u: { return set_lum(cb, blum(cs)); }
        case 8u: {
            if (luminance(cs) < luminance(cb)) {
                return cs;
            }
            return cb;
        }
        case 15u: {
            if (luminance(cs) > luminance(cb)) {
                return cs;
            }
            return cb;
        }
        default: {
            return vec3<f32>(separable(mode, cb.x, cs.x), separable(mode, cb.y, cs.y), separable(mode, cb.z, cs.z));
        }
    }
}

fn supports_hdr(mode: u32) -> bool {
    switch mode {
        case 0u, 1u, 2u, 3u, 4u, 5u, 6u, 7u, 8u, 9u, 10u, 11u, 14u, 15u, 16u, 23u, 24u, 25u, 26u, 27u, 31u, 32u, 33u, 34u, 35u, 36u, 37u: { return true; }
        default: { return false; }
    }
}

fn is_stencil(mode: u32) -> bool {
    return mode >= 32u && mode <= 35u;
}

// color::blend_pixel.
fn blend_pixel(mode: u32, dst: vec4<f32>, src: vec4<f32>, noise: f32) -> vec4<f32> {
    let sa = src.w;
    let da = dst.w;
    switch mode {
        case 0u: {
            let k = 1.0 - sa;
            return vec4<f32>(src.xyz + dst.xyz * k, sa + da * k);
        }
        case 1u, 2u: {
            if (sa <= 0.0 || noise >= sa) {
                return dst;
            }
            let inv = 1.0 / sa;
            return vec4<f32>(src.xyz * inv, 1.0);
        }
        case 32u: { return dst * sa; }
        case 34u: { return dst * (1.0 - sa); }
        case 33u: { return dst * luminance(src.xyz); }
        case 35u: { return dst * (1.0 - luminance(src.xyz)); }
        case 36u: { return vec4<f32>(src.xyz + dst.xyz * (1.0 - sa), min(sa + da, 1.0)); }
        case 37u: { return vec4<f32>(src.xyz + dst.xyz * (1.0 - sa), sa + da - sa * da); }
        default: {}
    }
    if (sa <= 0.0) {
        return dst;
    }
    if (da <= 0.0) {
        return src;
    }
    var cs = src.xyz / sa;
    var cb = dst.xyz / da;
    if (!supports_hdr(mode)) {
        cs = clamp(cs, vec3<f32>(0.0), vec3<f32>(1.0));
        cb = clamp(cb, vec3<f32>(0.0), vec3<f32>(1.0));
    }
    let ao = sa + da - sa * da;
    // Classic Color Dodge / Difference (neutral 0) and Classic Color Burn (neutral 1): the
    // source fades toward the neutral colour by its coverage, then the formula applies.
    if (mode == 13u || mode == 24u || mode == 6u) {
        var neutral = 0.0;
        if (mode == 6u) {
            neutral = 1.0;
        }
        let bc = non_separable(mode, cb, neutral + sa * (cs - neutral));
        return vec4<f32>((1.0 - da) * src.xyz + da * bc, ao);
    }
    let b = non_separable(mode, cb, cs);
    return vec4<f32>((1.0 - da) * src.xyz + (1.0 - sa) * dst.xyz + sa * da * b, ao);
}

// ---------------------------------------------------------------- colour (color crate)

fn srgb_to_linear(v: f32) -> f32 {
    if (v <= 0.04045) {
        return v / 12.92;
    }
    return pow((v + 0.055) / 1.055, 2.4);
}

fn linear_to_srgb(v: f32) -> f32 {
    if (v <= 0.0031308) {
        return v * 12.92;
    }
    return 1.055 * pow(max(v, 0.0), 1.0 / 2.4) - 0.055;
}

fn rgb_to_hsl(c: vec3<f32>) -> vec3<f32> {
    let mx = max(max(c.x, c.y), c.z);
    let mn = min(min(c.x, c.y), c.z);
    let l = (mx + mn) * 0.5;
    if (abs(mx - mn) < 1e-7) {
        return vec3<f32>(0.0, 0.0, l);
    }
    let d = mx - mn;
    var s: f32;
    if (l > 0.5) {
        s = d / (2.0 - mx - mn);
    } else {
        s = d / (mx + mn);
    }
    var h: f32;
    if (mx == c.x) {
        h = ((c.y - c.z) / d + select(0.0, 6.0, c.y < c.z)) / 6.0;
    } else if (mx == c.y) {
        h = ((c.z - c.x) / d + 2.0) / 6.0;
    } else {
        h = ((c.x - c.y) / d + 4.0) / 6.0;
    }
    return vec3<f32>(h, s, l);
}

fn hsl_channel(t0: f32, p: f32, q: f32) -> f32 {
    let t = fract_euclid(t0);
    if (t < 1.0 / 6.0) {
        return p + (q - p) * 6.0 * t;
    }
    if (t < 0.5) {
        return q;
    }
    if (t < 2.0 / 3.0) {
        return p + (q - p) * (2.0 / 3.0 - t) * 6.0;
    }
    return p;
}

fn hsl_to_rgb(h: f32, s: f32, l: f32) -> vec3<f32> {
    if (s <= 0.0) {
        return vec3<f32>(l);
    }
    var q: f32;
    if (l < 0.5) {
        q = l * (1.0 + s);
    } else {
        q = l + s - l * s;
    }
    let p = 2.0 * l - q;
    return vec3<f32>(hsl_channel(h + 1.0 / 3.0, p, q), hsl_channel(h, p, q), hsl_channel(h - 1.0 / 3.0, p, q));
}

// ColorSpace transfer curves: 1 = sRGB (sRGB, Display P3), 2 = 2.4 power (Rec. 709 / 2020),
// 3 = SMPTE ST 2084 PQ and 4 = BT.2100 HLG (linear 1.0 = the BT.2408 reference white).
const PQ_M1: f32 = 0.1593017578125;
const PQ_M2: f32 = 78.84375;
const PQ_C1: f32 = 0.8359375;
const PQ_C2: f32 = 18.8515625;
const PQ_C3: f32 = 18.6875;
const PQ_WHITE: f32 = 0.0203;
const HLG_A: f32 = 0.17883277;
const HLG_B: f32 = 0.28466892;
const HLG_C: f32 = 0.55991073;
const HLG_REF: f32 = 0.26496256;

fn pq_decode(e: f32) -> f32 {
    let p = pow(clamp(e, 0.0, 1.0), 1.0 / PQ_M2);
    return pow(max(p - PQ_C1, 0.0) / (PQ_C2 - PQ_C3 * p), 1.0 / PQ_M1) / PQ_WHITE;
}

fn pq_encode(l: f32) -> f32 {
    let y = pow(max(l * PQ_WHITE, 0.0), PQ_M1);
    return pow((PQ_C1 + PQ_C2 * y) / (1.0 + PQ_C3 * y), PQ_M2);
}

fn hlg_decode(v: f32) -> f32 {
    if (v <= 0.5) {
        return v * v / 3.0 / HLG_REF;
    }
    return (exp((v - HLG_C) / HLG_A) + HLG_B) / 12.0 / HLG_REF;
}

fn hlg_encode(l: f32) -> f32 {
    let e = max(l * HLG_REF, 0.0);
    if (e <= 1.0 / 12.0) {
        return sqrt(3.0 * e);
    }
    return HLG_A * log(12.0 * e - HLG_B) + HLG_C;
}

fn curve_decode(curve: u32, v: f32) -> f32 {
    let a = abs(v);
    var l = a;
    if (curve == 1u) {
        l = srgb_to_linear(a);
    } else if (curve == 2u) {
        l = powz(a, 2.4);
    } else if (curve == 3u) {
        l = pq_decode(a);
    } else if (curve == 4u) {
        l = hlg_decode(a);
    }
    return select(l, -l, v < 0.0);
}

fn curve_encode(curve: u32, v: f32) -> f32 {
    let a = abs(v);
    var e = a;
    if (curve == 1u) {
        e = linear_to_srgb(a);
    } else if (curve == 2u) {
        e = powz(a, 1.0 / 2.4);
    } else if (curve == 3u) {
        e = pq_encode(a);
    } else if (curve == 4u) {
        e = hlg_encode(a);
    }
    return select(e, -e, v < 0.0);
}

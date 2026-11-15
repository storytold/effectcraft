// The viewer's display pass (#2797): a rendered frame (premultiplied, encoded in the output
// space; Rgba16Float for 32 bpc frames, Rgba8Unorm otherwise) → the premultiplied RGBA8 texture
// the viewer draws. Line by line the same operations as `effectcraft_engine::viewer::DisplayPass`
// (its `pixel` is this shader's CPU reference and fallback):
// unpremultiply → decode, × 2^stops, Compand / Tone Map, encode → clamp → display colour →
// Show Channel, premultiply → floor(v · 255 + 0.5).

struct Params {
    // channel (0 RGB, 1 R, 2 G, 3 B, 4 A, 5 RGB Straight), colorized, frame curve, HDR step
    // (0 none, 1 Compand, 2 Tone Map)
    ids0: vec4<u32>,
    // display colour (0 none, 1 conversions, 2 LUT), 8-bit device numbers, a second conversion,
    // LUT size
    ids1: vec4<u32>,
    // curves: first conversion's decode and encode, second conversion's decode and encode
    ids2: vec4<u32>,
    // gain (2^stops), frame curve's gamma, first / second conversion has a matrix (1.0)
    vals0: vec4<f32>,
    // gammas of the ids2 curves (encoding: 1 / gamma)
    vals1: vec4<f32>,
    // 1 / the frame curve's gamma, 1 / 2.4, 1 / PQ m2, 1 / PQ m1: divided on the CPU, as the
    // reference does, so the exponents are the same bits
    vals2: vec4<f32>,
    m1: array<vec4<f32>, 3>,
    m2: array<vec4<f32>, 3>,
};

@group(0) @binding(0) var<uniform> P: Params;
@group(0) @binding(1) var frame: texture_2d<f32>;
@group(0) @binding(2) var lut: texture_3d<f32>;

@vertex
fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    // One triangle covering the target.
    let x = f32((i << 1u) & 2u);
    let y = f32(i & 2u);
    return vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
}

// The curves take the gamma to decode with, or 1 / gamma to encode with.
// SMPTE ST 2084 and BT.2100 HLG (effectcraft_color::space), linear 1.0 = reference white.
const PQ_M1: f32 = 0.1593017578125;
const PQ_M2: f32 = 78.84375;
const PQ_C1: f32 = 0.8359375;
const PQ_C2: f32 = 18.8515625;
const PQ_C3: f32 = 18.6875;
const PQ_WHITE: f32 = 0.0203;
const HLG_A: f32 = 0.17883277;
const HLG_B: f32 = 0.28466892;
const HLG_C: f32 = 0.55991073;
const HLG_REFERENCE: f32 = 0.26496256;

fn power(a: f32, g: f32) -> f32 {
    if (a <= 0.0) {
        return 0.0;
    }
    return pow(a, g);
}

fn copysign(m: f32, v: f32) -> f32 {
    if (v < 0.0) {
        return -m;
    }
    return m;
}

fn decode(curve: u32, g: f32, v: f32) -> f32 {
    let a = abs(v);
    var l = a;
    switch curve {
        case 1u: {
            if (a <= 0.04045) {
                l = a / 12.92;
            } else {
                l = power((a + 0.055) / 1.055, 2.4);
            }
        }
        case 2u: {
            l = power(a, g);
        }
        case 3u: {
            let p = power(min(a, 1.0), P.vals2.z);
            l = power(max(p - PQ_C1, 0.0) / (PQ_C2 - PQ_C3 * p), P.vals2.w) / PQ_WHITE;
        }
        case 4u: {
            var e: f32;
            if (a <= 0.5) {
                e = a * a / 3.0;
            } else {
                e = (exp((a - HLG_C) / HLG_A) + HLG_B) / 12.0;
            }
            l = e / HLG_REFERENCE;
        }
        default: {}
    }
    return copysign(l, v);
}

fn encode(curve: u32, g: f32, v: f32) -> f32 {
    let a = abs(v);
    var e = a;
    switch curve {
        case 1u: {
            if (a <= 0.0031308) {
                e = a * 12.92;
            } else {
                e = 1.055 * power(a, P.vals2.y) - 0.055;
            }
        }
        case 2u: {
            e = power(a, g);
        }
        case 3u: {
            let y = power(a * PQ_WHITE, PQ_M1);
            e = power((PQ_C1 + PQ_C2 * y) / (1.0 + PQ_C3 * y), PQ_M2);
        }
        case 4u: {
            let s = a * HLG_REFERENCE;
            if (s <= 1.0 / 12.0) {
                e = sqrt(3.0 * s);
            } else {
                e = HLG_A * log(12.0 * s - HLG_B) + HLG_C;
            }
        }
        default: {}
    }
    return copysign(e, v);
}

fn decode3(curve: u32, g: f32, c: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(decode(curve, g, c.x), decode(curve, g, c.y), decode(curve, g, c.z));
}

fn encode3(curve: u32, g: f32, c: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(encode(curve, g, c.x), encode(curve, g, c.y), encode(curve, g, c.z));
}

fn mat_mul(m: array<vec4<f32>, 3>, l: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(
        m[0].x * l.x + m[0].y * l.y + m[0].z * l.z,
        m[1].x * l.x + m[1].y * l.y + m[1].z * l.z,
        m[2].x * l.x + m[2].y * l.y + m[2].z * l.z,
    );
}

// effectcraft_render::color::compand
fn compand(v: f32) -> f32 {
    let k = 0.8;
    if (v <= k) {
        return v;
    }
    return k + (1.0 - k) * (1.0 - exp(-(v - k) / (1.0 - k)));
}

// effectcraft_render::color::tone_map (extended Reinhard of Rec. 709 luminance, white 4.0)
fn tone_map(c: vec3<f32>) -> vec3<f32> {
    let w = 4.0;
    let l = 0.2126 * c.x + 0.7152 * c.y + 0.0722 * c.z;
    if (l <= 0.0) {
        return c;
    }
    let lm = l * (1.0 + l / (w * w)) / (1.0 + l);
    let k = lm / l;
    return c * k;
}

fn lut_at(n: u32, r: u32, g: u32, b: u32) -> vec3<f32> {
    return textureLoad(lut, vec3<u32>(b, g, r), 0).xyz;
}

// effectcraft_engine::viewer::lut_eval: trilinear in the same order.
fn lut_eval(c: vec3<f32>) -> vec3<f32> {
    let n = P.ids1.w;
    let x = clamp(c, vec3<f32>(0.0), vec3<f32>(1.0)) * f32(n - 1u);
    let k = min(vec3<u32>(floor(x)), vec3<u32>(n - 2u));
    let f = x - vec3<f32>(k);
    var out = vec3<f32>(0.0);
    for (var corner = 0u; corner < 8u; corner++) {
        let d = vec3<u32>((corner >> 2u) & 1u, (corner >> 1u) & 1u, corner & 1u);
        var wr = 1.0 - f.x;
        if (d.x == 1u) {
            wr = f.x;
        }
        var wg = 1.0 - f.y;
        if (d.y == 1u) {
            wg = f.y;
        }
        var wb = 1.0 - f.z;
        if (d.z == 1u) {
            wb = f.z;
        }
        let w = wr * wg * wb;
        out = out + w * lut_at(n, k.x + d.x, k.y + d.y, k.z + d.z);
    }
    return out;
}

fn display_colour(c: vec3<f32>) -> vec3<f32> {
    var o = c;
    if (P.ids1.x == 2u) {
        o = lut_eval(c);
    } else if (P.ids1.x == 1u) {
        o = decode3(P.ids2.x, P.vals1.x, o);
        if (P.vals0.z == 1.0) {
            o = mat_mul(P.m1, o);
        }
        o = encode3(P.ids2.y, P.vals1.y, o);
        if (P.ids1.y == 1u) {
            o = floor(clamp(o, vec3<f32>(0.0), vec3<f32>(1.0)) * 255.0 + 0.5) / 255.0;
        }
        if (P.ids1.z == 1u) {
            o = decode3(P.ids2.z, P.vals1.z, o);
            if (P.vals0.w == 1.0) {
                o = mat_mul(P.m2, o);
            }
            o = encode3(P.ids2.w, P.vals1.w, o);
        }
    }
    return clamp(o, vec3<f32>(0.0), vec3<f32>(1.0));
}

fn q(v: f32) -> f32 {
    return floor(clamp(v, 0.0, 1.0) * 255.0 + 0.5) / 255.0;
}

@fragment
fn fs(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let s = textureLoad(frame, vec2<i32>(floor(pos.xy)), 0);
    // Frames are never NaN here: `frames::F16Frame` and 8-bit frames hold finite values only
    // (the NaN test is the reference's, kept for safety; WGSL may drop it). Alpha is clamped to
    // 0..1 first, as `display_pixel` and `DisplayPass::pixel` do (the compositor's alpha never
    // exceeds 1).
    var a = s.w;
    if (!(a == a)) {
        a = 0.0;
    }
    a = clamp(a, 0.0, 1.0);
    var c = vec3<f32>(0.0);
    if (a > 0.0) {
        c = s.xyz / a;
    }
    let channel = P.ids0.x;
    let curve = P.ids0.z;
    let g = P.vals0.y;
    let gain = P.vals0.x;
    let light = gain != 1.0 || P.ids0.w != 0u;
    // Premultiplied channels without exposure or HDR step: straight from the frame.
    var pm = clamp(s.xyz, vec3<f32>(0.0), vec3<f32>(a));
    if (light) {
        var l = decode3(curve, g, c) * gain;
        if (P.ids0.w == 1u) {
            l = vec3<f32>(compand(l.x), compand(l.y), compand(l.z));
        } else if (P.ids0.w == 2u) {
            l = tone_map(l);
        }
        c = encode3(curve, P.vals2.x, l);
    }
    c = clamp(c, vec3<f32>(0.0), vec3<f32>(1.0));
    if (light) {
        pm = c * a;
    }
    if (channel == 0u || channel == 5u) {
        if (P.ids1.x != 0u) {
            c = display_colour(c);
            pm = c * a;
        }
        if (channel == 5u) {
            return vec4<f32>(q(c.x), q(c.y), q(c.z), 1.0);
        }
        return vec4<f32>(q(pm.x), q(pm.y), q(pm.z), q(a));
    }
    if (channel == 4u) {
        var v = a;
        if (gain != 1.0) {
            v = encode(curve, P.vals2.x, decode(curve, g, a) * gain);
        }
        let e = q(v);
        return vec4<f32>(e, e, e, 1.0);
    }
    let v = q(pm[channel - 1u]);
    if (P.ids0.y == 1u) {
        var o = vec4<f32>(0.0, 0.0, 0.0, 1.0);
        o[channel - 1u] = v;
        return o;
    }
    return vec4<f32>(v, v, v, 1.0);
}

// The next mip level of the displayed texture (`frame` = the level above): each texel the mean
// of the premultiplied texels it covers, 2 × 2, and at the right / bottom edge of a level above
// of odd size also its last column / row (3 wide or high), so no texel of the frame is dropped
// (the level sizes are the GPU's, rounded down). Rounded as the pass rounds.
@fragment
fn mip(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let above = vec2<i32>(textureDimensions(frame));
    let size = max(above / 2, vec2<i32>(1));
    let p = vec2<i32>(floor(pos.xy));
    let lo = min(p * 2, above - vec2<i32>(1));
    var hi = min(p * 2 + vec2<i32>(2), above);
    if (p.x == size.x - 1) {
        hi.x = above.x;
    }
    if (p.y == size.y - 1) {
        hi.y = above.y;
    }
    var s = vec4<f32>(0.0);
    for (var y = lo.y; y < hi.y; y++) {
        for (var x = lo.x; x < hi.x; x++) {
            s = s + textureLoad(frame, vec2<i32>(x, y), 0);
        }
    }
    let n = f32((hi.x - lo.x) * (hi.y - lo.y));
    return floor(s / n * 255.0 + 0.5) / 255.0;
}

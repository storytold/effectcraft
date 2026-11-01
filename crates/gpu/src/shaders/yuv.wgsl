// Finished frame → 8-bit BT.709 limited-range 4:2:0 (Y, U, V planes packed 4 bytes per word),
// composited over the comp background: the GPU twin of `Image::to_rgba8_over` followed by
// export's `rgba_to_yuv420`, operation for operation (same order of float operations, the same
// rounding: `as u8` truncation of `x * 255 + 0.5`, `f32::round` half away from zero).

struct YP {
    // width, height, chroma width, Y words
    size: vec4<u32>,
    bg: vec4<f32>,
}

@group(0) @binding(0) var<uniform> Y: YP;
@group(0) @binding(1) var frame: texture_2d<f32>;
@group(0) @binding(2) var<storage, read_write> outw: array<u32>;

// NaN by its bits: drivers may assume arithmetic never sees NaN.
fn is_nan(x: f32) -> bool {
    return (bitcast<u32>(x) & 0x7fffffffu) > 0x7f800000u;
}

// `(c.clamp(0, 1) * 255 + 0.5) as u8`; the CPU's `as u8` turns NaN into 0.
fn q8(c: f32, nan: bool) -> f32 {
    if (nan) {
        return 0.0;
    }
    return floor(clamp(c, 0.0, 1.0) * 255.0 + 0.5);
}

// f32::round for x >= 0 (half away from zero).
fn round_pos(x: f32) -> f32 {
    let f = floor(x);
    return select(f, f + 1.0, x - f >= 0.5);
}

fn px(x: u32, y: u32) -> vec3<f32> {
    let p = textureLoad(frame, vec2<i32>(i32(x), i32(y)), 0);
    // A NaN alpha makes every channel NaN on the CPU (k is NaN).
    let na = is_nan(p.w);
    let k = select(1.0 - clamp(p.w, 0.0, 1.0), 0.0, na);
    let c = vec3<f32>(q8(p.x + Y.bg.x * k, na || is_nan(p.x)), q8(p.y + Y.bg.y * k, na || is_nan(p.y)), q8(p.z + Y.bg.z * k, na || is_nan(p.z)));
    return c / 255.0;
}

fn luma(c: vec3<f32>) -> f32 {
    return 0.2126 * c.x + 0.7152 * c.y + 0.0722 * c.z;
}

fn y8(l: f32) -> u32 {
    return u32(clamp(round_pos(16.0 + 219.0 * l), 1.0, 254.0));
}

// One invocation: 4 chroma samples = 8 × 2 pixels.
@compute @workgroup_size(8, 8)
fn yuv420(@builtin(global_invocation_id) gid: vec3<u32>) {
    let w = Y.size.x;
    let h = Y.size.y;
    let cw = Y.size.z;
    if (gid.x * 4u >= cw || gid.y * 2u >= h) {
        return;
    }
    let x0 = gid.x * 8u;
    let cy = gid.y;
    var uw = 0u;
    var vw = 0u;
    var yw = array<u32, 4>(0u, 0u, 0u, 0u);
    for (var q = 0u; q < 4u; q++) {
        // Accumulate in the CPU's order: row 0 left, right; row 1 left, right.
        var a0 = 0.0;
        var a1 = 0.0;
        for (var dy = 0u; dy < 2u; dy++) {
            for (var dx = 0u; dx < 2u; dx++) {
                let x = x0 + q * 2u + dx;
                let c = px(x, cy * 2u + dy);
                let l = luma(c);
                let byte = q * 2u + dx;
                yw[dy * 2u + byte / 4u] |= y8(l) << (8u * (byte % 4u));
                a0 += (c.z - l) / 1.8556;
                a1 += (c.x - l) / 1.5748;
            }
        }
        let u = u32(clamp(round_pos(128.0 + 224.0 * a0 / 4.0), 1.0, 254.0));
        let v = u32(clamp(round_pos(128.0 + 224.0 * a1 / 4.0), 1.0, 254.0));
        uw |= u << (8u * q);
        vw |= v << (8u * q);
    }
    let r0 = (cy * 2u * w + x0) / 4u;
    let r1 = ((cy * 2u + 1u) * w + x0) / 4u;
    outw[r0] = yw[0];
    outw[r0 + 1u] = yw[1];
    outw[r1] = yw[2];
    outw[r1 + 1u] = yw[3];
    let cwords = cw / 4u * (h / 2u);
    let ci = (cy * cw) / 4u + gid.x;
    outw[Y.size.w + ci] = uw;
    outw[Y.size.w + cwords + ci] = vw;
}

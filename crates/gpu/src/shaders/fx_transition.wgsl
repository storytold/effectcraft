// GPU effects, transitions and perspective: see src/fx_transition.rs. Block Dissolve, the CC
// transitions (Glass Wipe, Grid Wipe, Image Wipe, Jaws, Line Sweep, Radial ScaleWipe, Scale
// Wipe, Twister, WarpoMatic), Radial Shadow, CC Bender, CC Blobbylize, CC Cylinder, CC Sphere,
// CC Spotlight, CC Environment and 3D Glasses, each mirroring the CPU effect operation for
// operation. Coordinates are pixel centres relative to the effect's centre where it has one.

fn ftr_inside(p: vec2<i32>) -> bool {
    let d = out_dims();
    return p.x < d.x && p.y < d.y;
}

fn ftr_centre(p: vec2<i32>) -> vec2<f32> {
    return vec2<f32>(f32(p.x) + 0.5, f32(p.y) + 0.5);
}

// util::smoothstep.
fn ftr_smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    if (abs(e1 - e0) < 1e-9) {
        return select(1.0, 0.0, x < e0);
    }
    let t = clamp((x - e0) / (e1 - e0), 0.0, 1.0);
    return t * t * (3.0 - 2.0 * t);
}

// util::unpremul's colour.
fn ftr_unpremul(p: vec4<f32>) -> vec3<f32> {
    if (p.w > 1e-6) {
        return p.xyz / p.w;
    }
    return vec3<f32>(0.0);
}

// The third image, copied into `data` as RGBA f32 rows: P.u[3] = (row stride, width, height).
fn ftr_row(x: i32, y: i32) -> vec4<f32> {
    if (x < 0 || y < 0 || x >= i32(P.u[3].y) || y >= i32(P.u[3].z)) {
        return vec4<f32>(0.0);
    }
    let i = (u32(y) * P.u[3].x + u32(x)) * 4u;
    return vec4<f32>(data[i], data[i + 1u], data[i + 2u], data[i + 3u]);
}

fn ftr_row_clamped(x: i32, y: i32) -> vec4<f32> {
    return ftr_row(clamp(x, 0, i32(P.u[3].y) - 1), clamp(y, 0, i32(P.u[3].z) - 1));
}

// Image::sample_bilinear on the third image.
fn ftr_row_bilinear(x: f32, y: f32) -> vec4<f32> {
    let fx = x - 0.5;
    let fy = y - 0.5;
    let x0 = floor(fx);
    let y0 = floor(fy);
    let tx = fx - x0;
    let ty = fy - y0;
    let xi = i32(x0);
    let yi = i32(y0);
    let a = ftr_row(xi, yi);
    let b = ftr_row(xi + 1, yi);
    let c = ftr_row(xi, yi + 1);
    let d = ftr_row(xi + 1, yi + 1);
    let top = a + (b - a) * tx;
    let bot = c + (d - c) * tx;
    return top + (bot - top) * ty;
}

// Image::sample_bilinear_clamped on the third image.
fn ftr_row_bilinear_clamped(x: f32, y: f32) -> vec4<f32> {
    let fx = x - 0.5;
    let fy = y - 0.5;
    let x0 = floor(fx);
    let y0 = floor(fy);
    let tx = fx - x0;
    let ty = fy - y0;
    let xi = i32(x0);
    let yi = i32(y0);
    let a = ftr_row_clamped(xi, yi);
    let b = ftr_row_clamped(xi + 1, yi);
    let c = ftr_row_clamped(xi, yi + 1);
    let d = ftr_row_clamped(xi + 1, yi + 1);
    let top = a + (b - a) * tx;
    let bot = c + (d - c) * tx;
    return top + (bot - top) * ty;
}

// Central differences of the aux plane (x), edges clamped (transition2::grad).
fn ftr_grad(p: vec2<i32>) -> vec2<f32> {
    let gx = (tex_get_clamped(aux, p.x + 1, p.y).x - tex_get_clamped(aux, p.x - 1, p.y).x) * 0.5;
    let gy = (tex_get_clamped(aux, p.x, p.y + 1).x - tex_get_clamped(aux, p.x, p.y - 1).x) * 0.5;
    return vec2<f32>(gx, gy);
}

// transition2::wipe_t.
fn ftr_wipe_t(g: f32, c: f32, w0: f32) -> f32 {
    let w = max(w0, 1e-3);
    let edge = c * (1.0 + 2.0 * w) - w;
    return 1.0 - ftr_smoothstep(edge - w, edge + w, g);
}

// ---------------------------------------------------------------- planes

// A plane (x) from the source: u[0].x = util::Src index (Red, Green, Blue, Alpha, Luminance,
// Hue, Lightness, Saturation, Full, Half, Off) times alpha except for Alpha itself
// (transition2::prop_plane, CC Blobbylize); 11 = CC Environment's relief.
@compute @workgroup_size(16, 16)
fn ftr_plane(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!ftr_inside(p)) {
        return;
    }
    let px = textureLoad(src, p, 0);
    let c = ftr_unpremul(px);
    let a = max(px.w, 0.0);
    var v: f32;
    switch P.u[0].x {
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
            v = rgb_to_hsl(c).x;
        }
        case 6u: {
            v = rgb_to_hsl(c).z;
        }
        case 7u: {
            v = rgb_to_hsl(c).y;
        }
        case 8u: {
            v = 1.0;
        }
        case 9u: {
            v = 0.5;
        }
        case 11u: {
            v = luminance(c) * 0.5 + 0.5;
        }
        default: {
            v = 0.0;
        }
    }
    if (P.u[0].x != 3u) {
        v *= a;
    }
    textureStore(out, p, vec4<f32>(v, 0.0, 0.0, 0.0));
}

// src × aux.x (Image::mul_mask).
@compute @workgroup_size(16, 16)
fn ftr_mul(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!ftr_inside(p)) {
        return;
    }
    textureStore(out, p, textureLoad(src, p, 0) * textureLoad(aux, p, 0).x);
}

// aux + src × (1 − aux.a): src composited under aux.
@compute @workgroup_size(16, 16)
fn ftr_under(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!ftr_inside(p)) {
        return;
    }
    let o = textureLoad(aux, p, 0);
    textureStore(out, p, o + textureLoad(src, p, 0) * (1.0 - o.w));
}

// ---------------------------------------------------------------- Block Dissolve (transition::block_dissolve)
// The kept-block mask (x). u[0] = (sub-samples per axis, seed, column table length);
// f[0].x = completion; data = block columns (width × n, u32) then block rows (height × n).

@compute @workgroup_size(16, 16)
fn ftr_blocks(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!ftr_inside(p)) {
        return;
    }
    let n = P.u[0].x;
    var hits = 0u;
    for (var j = 0u; j < n; j++) {
        let by = bitcast<u32>(data[P.u[0].z + u32(p.y) * n + j]);
        for (var i = 0u; i < n; i++) {
            let bx = bitcast<u32>(data[u32(p.x) * n + i]);
            if (hash_noise(bx, by, P.u[0].y) >= P.f[0].x) {
                hits += 1u;
            }
        }
    }
    textureStore(out, p, vec4<f32>(f32(hits) / f32(n * n), 0.0, 0.0, 0.0));
}

// ---------------------------------------------------------------- CC Grid Wipe (transition::grid_wipe)
// u[0] = (shape, reverse); f[0] = (cx, cy, sin, cos); f[1] = (cell size, max distance,
// completion, border).

fn ftr_metric(shape: u32, d: vec2<f32>) -> f32 {
    switch shape {
        case 0u: {
            return abs(d.x);
        }
        case 2u: {
            return max(abs(d.x), abs(d.y));
        }
        default: {
            return length(d);
        }
    }
}

@compute @workgroup_size(16, 16)
fn ftr_grid(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!ftr_inside(p)) {
        return;
    }
    let px = textureLoad(src, p, 0);
    let d = ftr_centre(p) - P.f[0].xy;
    let s = P.f[0].z;
    let co = P.f[0].w;
    let q = vec2<f32>(d.x * co + d.y * s, -d.x * s + d.y * co);
    let cs = P.f[1].x;
    let cc = (floor(q / cs) + 0.5) * cs;
    var o = clamp(ftr_metric(P.u[0].x, cc) / P.f[1].y, 0.0, 1.0);
    if (P.u[0].y != 0u) {
        o = 1.0 - o;
    }
    let spread = 0.25;
    let pr = clamp((P.f[1].z * (1.0 + spread) - o) / spread, 0.0, 1.0);
    if (pr <= 0.0) {
        textureStore(out, p, px);
        return;
    }
    let half = cs * 0.5 * (1.0 - pr);
    let m = max(abs(q.x - cc.x), abs(q.y - cc.y));
    var k = select(0.0, 1.0, m < half);
    if (P.f[1].w > 0.0) {
        k = clamp((half - m) / P.f[1].w + 0.5, 0.0, 1.0);
    }
    textureStore(out, p, px * k);
}

// ---------------------------------------------------------------- CC Radial ScaleWipe / CC Scale Wipe (transition.rs)
// Radial: u[0].x = reverse; f[0] = (cx, cy, max distance, completion).

@compute @workgroup_size(16, 16)
fn ftr_radial_scale(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!ftr_inside(p)) {
        return;
    }
    let c = P.f[0].xy;
    let maxd = P.f[0].z;
    let done = P.f[0].w;
    let d = ftr_centre(p) - c;
    let r = length(d);
    var o = vec4<f32>(0.0);
    if (P.u[0].x != 0u) {
        let lim = (1.0 - done) * maxd;
        if (!(r >= lim || lim <= 1e-9)) {
            let k = maxd / lim;
            o = sample_bilinear(src, c.x + d.x * k, c.y + d.y * k);
        }
    } else {
        let hole = done * maxd;
        if (!(r <= hole || maxd - hole <= 1e-9)) {
            let rs = (r - hole) * maxd / (maxd - hole);
            o = sample_bilinear(src, c.x + d.x / r * rs, c.y + d.y / r * rs);
        }
    }
    textureStore(out, p, o);
}

// Scale Wipe: f[0] = (cx, cy, direction x, direction y); f[1].x = stretch.
@compute @workgroup_size(16, 16)
fn ftr_scale(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!ftr_inside(p)) {
        return;
    }
    let f = ftr_centre(p);
    let dir = P.f[0].zw;
    let u = dot(f - P.f[0].xy, dir);
    var q = f;
    if (u > 0.0) {
        let back = u - u / (1.0 + P.f[1].x);
        q = f - dir * back;
    }
    textureStore(out, p, sample_bilinear_clamped(src, q.x, q.y));
}

// ---------------------------------------------------------------- CC Glass Wipe (transition2::glass_wipe)
// aux = the gradient plane (x); data = the layer to reveal (rows). u[0].x = has reveal;
// f[0] = (completion, band half-width, displacement).

@compute @workgroup_size(16, 16)
fn ftr_glass(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!ftr_inside(p)) {
        return;
    }
    let t = ftr_wipe_t(textureLoad(aux, p, 0).x, P.f[0].x, P.f[0].y);
    let band = 4.0 * t * (1.0 - t);
    let g = ftr_grad(p);
    let f = ftr_centre(p);
    let o = g * P.f[0].z * band * 10.0;
    var a = textureLoad(src, p, 0);
    var r = vec4<f32>(0.0);
    if (band > 1e-4) {
        a = sample_bilinear(src, f.x + o.x, f.y + o.y);
        if (P.u[0].x != 0u) {
            r = ftr_row_bilinear(f.x - o.x, f.y - o.y);
        }
    } else if (P.u[0].x != 0u) {
        r = ftr_row(p.x, p.y);
    }
    textureStore(out, p, a + (r - a) * t);
}

// ---------------------------------------------------------------- CC Image Wipe (transition2::image_wipe)
// aux = the gradient plane (x). u[0].x = inverse; f[0] = (completion, band half-width).

@compute @workgroup_size(16, 16)
fn ftr_image_wipe(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!ftr_inside(p)) {
        return;
    }
    var g = textureLoad(aux, p, 0).x;
    if (P.u[0].x != 0u) {
        g = 1.0 - g;
    }
    let k = 1.0 - ftr_wipe_t(g, P.f[0].x, P.f[0].y);
    textureStore(out, p, textureLoad(src, p, 0) * k);
}

// ---------------------------------------------------------------- CC Jaws (transition2::jaws)
// u[0].x = shape; f[0] = (cx, cy, sin, cos); f[1] = (tooth width, amplitude, travel).

@compute @workgroup_size(16, 16)
fn ftr_jaws(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!ftr_inside(p)) {
        return;
    }
    let c = P.f[0].xy;
    let sn = P.f[0].z;
    let cs = P.f[0].w;
    let width = P.f[1].x;
    let amp = P.f[1].y;
    let dd = P.f[1].z;
    let d = ftr_centre(p) - c;
    let u = d.x * cs + d.y * sn;
    let v = -d.x * sn + d.y * cs;
    let ph = fract_euclid(u / width);
    var f: f32;
    switch P.u[0].x {
        case 0u: {
            f = amp * (1.0 - 4.0 * abs(ph - 0.5));
        }
        case 1u: {
            f = amp * clamp((1.0 - 4.0 * abs(ph - 0.5)) * 2.0, -1.0, 1.0);
        }
        case 2u: {
            f = select(-amp, amp, ph < 0.5);
        }
        default: {
            f = amp * sin(ph * 6.283185307179586);
        }
    }
    var vs = 0.0;
    var hit = true;
    if (v + dd < f) {
        vs = v + dd;
    } else if (v - dd >= f) {
        vs = v - dd;
    } else {
        hit = false;
    }
    var o = vec4<f32>(0.0);
    if (hit) {
        o = sample_bilinear(src, u * cs - vs * sn + c.x, u * sn + vs * cs + c.y);
    }
    textureStore(out, p, o);
}

// ---------------------------------------------------------------- CC Line Sweep (transition2::line_sweep)
// u[0] = (flip, slant ≥ 0); f[0] = (sin, cos, v0, u0); f[1] = (thickness, |slant|, strips,
// total travel); f[2].x = completion.

@compute @workgroup_size(16, 16)
fn ftr_line_sweep(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!ftr_inside(p)) {
        return;
    }
    let f = ftr_centre(p);
    let sn = P.f[0].x;
    let cs = P.f[0].y;
    let u = f.x * cs + f.y * sn;
    let v = -f.x * sn + f.y * cs;
    let nstr = P.f[1].z;
    var k = floor((v - P.f[0].z) / P.f[1].x);
    if (P.u[0].x != 0u) {
        k = nstr - 1.0 - k;
    }
    let stagger = select(nstr - 1.0 - k, k, P.u[0].y != 0u);
    let front = P.f[0].w + P.f[2].x * P.f[1].w - P.f[1].y * stagger;
    textureStore(out, p, select(textureLoad(src, p, 0), vec4<f32>(0.0), u < front));
}

// ---------------------------------------------------------------- CC Twister (transition2::twister)
// aux = the backside layer (u[0].x = has one); u[0].y = shading; f[0] = (cx, cy, sin, cos);
// f[1] = (half diagonal, completion).

@compute @workgroup_size(16, 16)
fn ftr_twister(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!ftr_inside(p)) {
        return;
    }
    let c = P.f[0].xy;
    let sn = P.f[0].z;
    let cs = P.f[0].w;
    let d = ftr_centre(p) - c;
    let u = d.x * cs + d.y * sn;
    let v = -d.x * sn + d.y * cs;
    let s01 = clamp((u / P.f[1].x) * 0.5 + 0.5, 0.0, 1.0);
    let th = 3.141592653589793 * clamp(P.f[1].y * 2.0 - s01, 0.0, 1.0);
    let k = cos(th);
    if (abs(k) < 1e-4) {
        textureStore(out, p, vec4<f32>(0.0));
        return;
    }
    let vs = v / k;
    var o: vec4<f32>;
    if (k > 0.0 || P.u[0].x == 0u) {
        o = sample_bilinear(src, u * cs - vs * sn + c.x, u * sn + vs * cs + c.y);
    } else {
        o = sample_bilinear(aux, u * cs + vs * sn + c.x, u * sn - vs * cs + c.y);
    }
    if (P.u[0].y != 0u) {
        let l = 0.4 + 0.6 * abs(k);
        o = vec4<f32>(o.xyz * l, o.w);
    }
    textureStore(out, p, o);
}

// ---------------------------------------------------------------- CC WarpoMatic (transition2::warpomatic)

// Reactor "Contrast Differences": src = the plane, aux = its wide mean.
@compute @workgroup_size(16, 16)
fn ftr_react_contrast(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!ftr_inside(p)) {
        return;
    }
    let a = textureLoad(src, p, 0).x;
    let b = textureLoad(aux, p, 0).x;
    textureStore(out, p, vec4<f32>(min(abs(a - b) * 4.0, 1.0), 0.0, 0.0, 0.0));
}

// Reactor "Local Differences": aux = the plane; its gradient magnitude.
@compute @workgroup_size(16, 16)
fn ftr_react_local(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!ftr_inside(p)) {
        return;
    }
    let g = ftr_grad(p);
    textureStore(out, p, vec4<f32>(min(sqrt(g.x * g.x + g.y * g.y) * 8.0, 1.0), 0.0, 0.0, 0.0));
}

// aux = the reactor plane (x); data = the layer to reveal (rows). u[0] = (warp direction,
// has reveal); f[0] = (completion, blend span, warp amount).
@compute @workgroup_size(16, 16)
fn ftr_warpo(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!ftr_inside(p)) {
        return;
    }
    let q = ftr_wipe_t(textureLoad(aux, p, 0).x, P.f[0].x, P.f[0].y);
    let g = ftr_grad(p);
    let f = ftr_centre(p);
    let amt = P.f[0].z;
    let o = g * amt * q;
    var i = g * amt * (1.0 - q);
    if (P.u[0].x == 1u) {
        i = -i;
    } else if (P.u[0].x == 2u) {
        i = vec2<f32>(-i.y, i.x);
    }
    var a = vec4<f32>(0.0);
    if (q < 1.0) {
        a = sample_bilinear(src, f.x + o.x, f.y + o.y);
    }
    var inc = vec4<f32>(0.0);
    if (P.u[0].y != 0u && q > 0.0) {
        inc = ftr_row_bilinear(f.x + i.x, f.y + i.y);
    }
    textureStore(out, p, a + (inc - a) * q);
}

// ---------------------------------------------------------------- Radial Shadow (perspective::radial_shadow)
// The shadow before its blur. u[0].x = glass edge; f[0] = (light x, y, projection k,
// opacity × colour alpha); f[1] = (colour, colour influence).

@compute @workgroup_size(16, 16)
fn ftr_rshadow(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!ftr_inside(p)) {
        return;
    }
    let l = P.f[0].xy;
    let q = ftr_centre(p);
    let s = sample_bilinear(src, l.x + (q.x - l.x) / P.f[0].z, l.y + (q.y - l.y) / P.f[0].z);
    let sc = ftr_unpremul(s);
    let sa = max(s.w, 0.0);
    var a = sa;
    if (P.u[0].x != 0u) {
        a = clamp(sa * (1.0 - sa) * 4.0, 0.0, 1.0);
    }
    a *= P.f[0].w;
    let col = P.f[1].xyz;
    let c = col + (sc - col) * P.f[1].w;
    textureStore(out, p, vec4<f32>(c * a, a));
}

// ---------------------------------------------------------------- CC Bender (distort3::cc_bender)
// u[0].x = style; f[0] = (base x, y, axis x, y); f[1] = (axis length, normal x, y, amount ×
// reference).

@compute @workgroup_size(16, 16)
fn ftr_bender(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!ftr_inside(p)) {
        return;
    }
    let f = ftr_centre(p);
    let r = f - P.f[0].xy;
    let t = clamp(dot(r, P.f[0].zw) / P.f[1].x, 0.0, 1.0);
    let pi = 3.141592653589793;
    var b: f32;
    switch P.u[0].x {
        case 1u: {
            b = 0.5 * sin(pi * t) * t;
        }
        case 2u: {
            b = 0.5 * t;
        }
        case 3u: {
            b = 0.25 * (1.0 - cos(pi * t)) - 0.1 * sin(2.0 * pi * t);
        }
        default: {
            b = 0.5 * t * t;
        }
    }
    let d = P.f[1].w * b;
    textureStore(out, p, sample_bilinear(src, f.x - P.f[1].y * d, f.y - P.f[1].z * d));
}

// ---------------------------------------------------------------- CC Blobbylize (distort3::cc_blobbylize)
// aux = the Softness-blurred blob plane (x). f[0] = (cut away, intensity, ambient,
// matte slope); f[1] = (diffuse, specular, shininess, metal); f[2] = (light x, y,
// point light height, point); f[3] = distant light direction; f[4] = light colour;
// f[5] = (matte depth, RGB depth, light slope, matte antialias width);
// f[6].x = displacement clamp.

@compute @workgroup_size(16, 16)
fn ftr_blob(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!ftr_inside(p)) {
        return;
    }
    let cut = P.f[0].x;
    if (cut >= 1.0) {
        // Endpoint guard: an opaque source can reach h = 1.0, where the gradient antialias
        // would otherwise leave a half-covered pixel at Cut Away 100.
        textureStore(out, p, vec4<f32>(0.0));
        return;
    }
    let hv = textureLoad(aux, p, 0).x;
    let g = ftr_grad(p);
    let grad = length(g);
    var m = 0.0;
    if (grad > 1e-9) {
        let aa = max(P.f[5].w * grad, 5e-4);
        m = clamp(0.5 + (hv - cut) / aa, 0.0, 1.0);
    } else if (hv > cut) {
        m = 1.0;
    }
    if (m <= 0.0) {
        textureStore(out, p, vec4<f32>(0.0));
        return;
    }
    let f = ftr_centre(p);
    // Softness-scaled bevel warp of the source matte (normal z = 1).
    let na = normalize(vec3<f32>(-g.x * P.f[0].w, -g.y * P.f[0].w, 1.0));
    let sa = sample_bilinear(src, f.x + na.x * P.f[5].x, f.y + na.y * P.f[5].x);
    let alpha = max(sa.w, 0.0) * m;
    if (alpha <= 0.0) {
        textureStore(out, p, vec4<f32>(0.0));
        return;
    }
    // Height-field refraction of the colour (normal z = height), independent of Softness.
    let hz = max(hv, 1e-3);
    let d = -g * P.f[5].y / hz;
    let lim = P.f[6].x;
    let disp = vec2<f32>(clamp(d.x, -lim, lim), clamp(d.y, -lim, lim));
    let srgb = sample_bilinear(src, f.x + disp.x, f.y + disp.y);
    let c = ftr_unpremul(srgb);
    let n = normalize(vec3<f32>(-g.x * P.f[5].z, -g.y * P.f[5].z, 1.0));
    var l = P.f[3].xyz;
    if (P.f[2].w != 0.0) {
        l = normalize(vec3<f32>(P.f[2].x - f.x, P.f[2].y - f.y, P.f[2].z));
    }
    let intensity = P.f[0].y;
    let ndl = max(dot(n, l), 0.0);
    let h3 = normalize(vec3<f32>(l.x, l.y, l.z + 1.0));
    let sp = powz(max(dot(n, h3), 0.0), P.f[1].z) * P.f[1].y;
    let lc = P.f[4].xyz;
    let metal = P.f[1].w;
    let lit = (P.f[0].z + P.f[1].x * ndl * intensity) * lc;
    let spec_col = (1.0 - metal) * lc + metal * c * lc;
    let o = c * lit + (sp * intensity) * spec_col;
    textureStore(out, p, vec4<f32>(o * alpha, alpha));
}

// 0.5 + atan2(x, z) / 2π, kept on the side of the ±π seam that the sign of `x` puts it on (a
// GPU's approximate atan2 can land just across it, which wraps to the opposite edge of the
// texture). x = ±0 takes the CPU's −π (its zero is negative there).
fn ftr_lon_u(x: f32, z: f32) -> f32 {
    let u = 0.5 + atan2(x, z) / 6.283185307179586;
    if (x <= 0.0) {
        return clamp(u, 0.0, 0.5);
    }
    return clamp(u, 0.5, 0.99999994);
}

// ---------------------------------------------------------------- CC Cylinder / CC Sphere (perspective.rs)
// Shading (perspective::Shading): f[3] = (light direction, intensity); f[4] = (light colour,
// ambient); f[5] = (diffuse, specular, shininess); f[6] = half vector. The layer rectangle in
// f[1] = (x0, y0, width, height).

fn ftr_shade(px: vec4<f32>, n: vec3<f32>) -> vec4<f32> {
    let c = ftr_unpremul(px);
    let a = max(px.w, 0.0);
    if (a <= 0.0) {
        return vec4<f32>(0.0);
    }
    let nl = max(dot(n, P.f[3].xyz), 0.0);
    var spec = 0.0;
    if (nl > 0.0) {
        spec = powz(max(dot(n, P.f[6].xyz), 0.0), P.f[5].z) * P.f[5].y;
    }
    let lc = P.f[4].xyz * P.f[3].w;
    let o = c * (P.f[4].w + P.f[5].x * nl * lc) + spec * lc;
    return vec4<f32>(o * a, a);
}

// perspective::texel: the layer as a texture at (u, v), wrapping horizontally.
fn ftr_texel(u0: f32, v: f32) -> vec4<f32> {
    let u = fract_euclid(u0);
    if (v < 0.0 || v > 1.0) {
        return vec4<f32>(0.0);
    }
    let r = P.f[1];
    return sample_bilinear_clamped(src, r.x + u * r.z, r.y + v * r.w);
}

fn ftr_over(top: vec4<f32>, bottom: vec4<f32>) -> vec4<f32> {
    return top + bottom * (1.0 - top.w);
}

fn ftr_faces(mode: u32, front: vec4<f32>, back: vec4<f32>) -> vec4<f32> {
    if (mode == 1u) {
        return front;
    }
    if (mode == 2u) {
        return back;
    }
    return ftr_over(front, back);
}

// Cylinder: u[0].x = render, u[0].y = axis rotation enabled;
// f[0] = (cx, cy, radius, layer height); f[2].x = legacy texture spin;
// f[7..10] = rows of the local-to-view rotation matrix.
fn ftr_cylinder_face(d: vec2<f32>, z: f32, back: bool) -> vec4<f32> {
    let m0 = P.f[7].xyz;
    let m1 = P.f[8].xyz;
    let m2 = P.f[9].xyz;
    let q = vec3<f32>(d, z);
    let local = vec3<f32>(m0.x * q.x + m1.x * q.y + m2.x * q.z,
                          m0.y * q.x + m1.y * q.y + m2.y * q.z,
                          m0.z * q.x + m1.z * q.y + m2.z * q.z);
    let u = (atan2(local.x, local.z) + P.f[2].x) / 6.283185307179586 + 0.5;
    let v = local.y / P.f[0].w + 0.5;
    let flip = select(1.0, -1.0, back);
    let n = vec3<f32>(local.x / P.f[0].z * flip, 0.0, local.z / P.f[0].z * flip);
    return ftr_shade(ftr_texel(u, v), vec3<f32>(dot(m0, n), dot(m1, n), dot(m2, n)));
}

@compute @workgroup_size(16, 16)
fn ftr_cylinder(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!ftr_inside(p)) {
        return;
    }
    let d = ftr_centre(p) - P.f[0].xy;
    if (P.u[0].y == 0u) {
        // Original path is unchanged for old projects and default parameters.
        let v = d.y / P.f[0].w + 0.5;
        let s = d.x / P.f[0].z;
        if (abs(s) >= 1.0) {
            textureStore(out, p, vec4<f32>(0.0));
            return;
        }
        let tau = 6.283185307179586;
        let th = asin_p(s);
        let cz = cos(th);
        let spin = P.f[2].x;
        let front = ftr_shade(ftr_texel((th + spin) / tau + 0.5, v), vec3<f32>(s, 0.0, cz));
        let back = ftr_shade(ftr_texel((3.141592653589793 - th + spin) / tau + 0.5, v), vec3<f32>(-s, 0.0, cz));
        textureStore(out, p, ftr_faces(P.u[0].x, front, back));
        return;
    }
    let axis = vec3<f32>(P.f[7].y, P.f[8].y, P.f[9].y);
    let a = 1.0 - axis.z * axis.z;
    let da = dot(d, axis.xy);
    let bq = -da * axis.z;
    let disc = bq * bq - a * (dot(d, d) - da * da - P.f[0].z * P.f[0].z);
    if (a <= 1e-10 || !(disc > 0.0)) {
        textureStore(out, p, vec4<f32>(0.0));
        return;
    }
    let root = sqrt(disc);
    let front = ftr_cylinder_face(d, (-bq + root) / a, false);
    let back = ftr_cylinder_face(d, (-bq - root) / a, true);
    textureStore(out, p, ftr_faces(P.u[0].x, front, back));
}

// Sphere: u[0].x = render; f[0] = (cx, cy, radius); f[7..10] = the rotation matrix rows.
fn ftr_sphere_tex(q: vec3<f32>) -> vec4<f32> {
    let m0 = P.f[7].xyz;
    let m1 = P.f[8].xyz;
    let m2 = P.f[9].xyz;
    let t = m0 * q.x + m1 * q.y + m2 * q.z;
    let lat = asin_p(clamp(t.y, -1.0, 1.0));
    return ftr_texel(ftr_lon_u(t.x, t.z), lat / 3.141592653589793 + 0.5);
}

@compute @workgroup_size(16, 16)
fn ftr_sphere(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!ftr_inside(p)) {
        return;
    }
    let sxy = (ftr_centre(p) - P.f[0].xy) / P.f[0].z;
    let d2 = sxy.x * sxy.x + sxy.y * sxy.y;
    if (d2 >= 1.0) {
        textureStore(out, p, vec4<f32>(0.0));
        return;
    }
    let z = sqrt(1.0 - d2);
    let front = ftr_shade(ftr_sphere_tex(vec3<f32>(sxy, z)), vec3<f32>(sxy, z));
    let back = ftr_shade(ftr_sphere_tex(vec3<f32>(sxy, -z)), vec3<f32>(-sxy, z));
    textureStore(out, p, ftr_faces(P.u[0].x, front, back));
}

// ---------------------------------------------------------------- CC Spotlight (perspective::spotlight)
// u[0].x = light only; f[0] = (from x, y, height, intensity); f[1] = (axis, inner angle);
// f[2] = (colour, cone angle).

@compute @workgroup_size(16, 16)
fn ftr_spot(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!ftr_inside(p)) {
        return;
    }
    let d = ftr_centre(p) - P.f[0].xy;
    let v = normalize(vec3<f32>(d, -P.f[0].z));
    let axis = P.f[1].xyz;
    // acos(v · axis), well conditioned near the axis.
    let ang = atan2(length(cross(v, axis)), dot(v, axis));
    let m = (1.0 - ftr_smoothstep(P.f[1].w, P.f[2].w, ang)) * P.f[0].w;
    let px = textureLoad(src, p, 0);
    if (P.u[0].x != 0u) {
        let a = clamp(m, 0.0, 1.0);
        textureStore(out, p, vec4<f32>(P.f[2].xyz * a, a));
    } else {
        textureStore(out, p, vec4<f32>(px.xyz * (m * P.f[2].xyz), px.w));
    }
}

// ---------------------------------------------------------------- CC Environment (perspective2::cc_environment)
// aux = the relief height map (x); data = the environment image (rows). u[0].x = mapping;
// f[0] = (1 / buffer scale, height, environment scale); f[1] = (environment offset, size).

fn ftr_env_uv(mapping: u32, r: vec3<f32>) -> vec2<f32> {
    if (mapping == 1u) {
        let m = 2.0 * max(sqrt(r.x * r.x + r.y * r.y + (r.z + 1.0) * (r.z + 1.0)), 1e-9);
        return vec2<f32>(0.5 + r.x / m, 0.5 + r.y / m);
    }
    if (mapping == 2u) {
        let ax = abs(r.x);
        let ay = abs(r.y);
        let az = abs(r.z);
        var cr: vec4<f32>;
        if (ax >= ay && ax >= az) {
            if (r.x > 0.0) {
                cr = vec4<f32>(2.0, 1.0, -r.z / ax, r.y / ax);
            } else {
                cr = vec4<f32>(0.0, 1.0, r.z / ax, r.y / ax);
            }
        } else if (ay >= az) {
            if (r.y < 0.0) {
                cr = vec4<f32>(1.0, 0.0, r.x / ay, -r.z / ay);
            } else {
                cr = vec4<f32>(1.0, 2.0, r.x / ay, r.z / ay);
            }
        } else if (r.z > 0.0) {
            cr = vec4<f32>(1.0, 1.0, r.x / az, r.y / az);
        } else {
            cr = vec4<f32>(1.0, 3.0, r.x / az, -r.y / az);
        }
        return vec2<f32>((cr.x + (cr.z + 1.0) * 0.5) / 3.0, (cr.y + (cr.w + 1.0) * 0.5) / 4.0);
    }
    let lat = asin_p(clamp(r.y, -1.0, 1.0));
    return vec2<f32>(ftr_lon_u(r.x, r.z), 0.5 + lat / 3.141592653589793);
}

@compute @workgroup_size(16, 16)
fn ftr_env(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!ftr_inside(p)) {
        return;
    }
    let a = textureLoad(src, p, 0).w;
    if (a <= 0.0) {
        textureStore(out, p, vec4<f32>(0.0));
        return;
    }
    let g = ftr_grad(p) * P.f[0].x;
    let height = P.f[0].y;
    let n = normalize(vec3<f32>(-g.x * height, -g.y * height, 1.0));
    let d = -n.z;
    let r = vec3<f32>(-2.0 * d * n.x, -2.0 * d * n.y, -(-1.0 - 2.0 * d * n.z));
    let uv = ftr_env_uv(P.u[0].x, r);
    let es = P.f[0].z;
    let ex = fract_euclid(uv.x) * P.f[1].z * es + P.f[1].x;
    let ey = clamp(uv.y, 0.0, 1.0) * P.f[1].w * es + P.f[1].y;
    let env = ftr_row_bilinear_clamped(ex, ey);
    let ec = env.xyz * (1.0 / max(env.w, 1e-3));
    textureStore(out, p, vec4<f32>(ec * a, a));
}

// ---------------------------------------------------------------- 3D Glasses (perspective2::glasses_3d)

// Translate by f[0].xy (perspective2::shift).
@compute @workgroup_size(16, 16)
fn ftr_shift(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!ftr_inside(p)) {
        return;
    }
    let f = ftr_centre(p) - P.f[0].xy;
    textureStore(out, p, sample_bilinear(src, f.x, f.y));
}

// src = left view, aux = right view. u[0].x = 3D view; f[0] = (width, height, balance).
@compute @workgroup_size(16, 16)
fn ftr_glasses(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!ftr_inside(p)) {
        return;
    }
    let f = ftr_centre(p);
    let w = P.f[0].x;
    let h = P.f[0].y;
    let k = P.f[0].z;
    let view = P.u[0].x;
    var o: vec4<f32>;
    if (view == 0u) {
        if (f.x < w * 0.5) {
            o = sample_bilinear(src, f.x * 2.0, f.y);
        } else {
            o = sample_bilinear(aux, (f.x - w * 0.5) * 2.0, f.y);
        }
    } else if (view == 1u) {
        if (f.y < h * 0.5) {
            o = sample_bilinear(src, f.x, f.y * 2.0);
        } else {
            o = sample_bilinear(aux, f.x, (f.y - h * 0.5) * 2.0);
        }
    } else if (view == 2u) {
        if (p.y % 2 == 0) {
            o = textureLoad(src, p, 0);
        } else {
            o = textureLoad(aux, p, 0);
        }
    } else {
        let lp = textureLoad(src, p, 0);
        let rp = textureLoad(aux, p, 0);
        let lc = ftr_unpremul(lp);
        let rc = ftr_unpremul(rp);
        let a = max(max(lp.w, 0.0), max(rp.w, 0.0));
        let ll = luminance(lc);
        let rl = luminance(rc);
        var c: vec3<f32>;
        switch view {
            case 3u: {
                c = vec3<f32>(ll, rl, 0.0);
            }
            case 4u: {
                c = vec3<f32>(ll, 0.0, rl);
            }
            case 5u: {
                c = vec3<f32>(lc.x + (ll - lc.x) * k, rc.y, rc.z);
            }
            case 6u: {
                c = vec3<f32>(ll * (1.0 - 0.5 * k) + lc.x * 0.5 * k, rl, 0.0);
            }
            case 7u: {
                c = vec3<f32>(ll * (1.0 - 0.5 * k) + lc.x * 0.5 * k, 0.0, rl);
            }
            default: {
                c = abs(lc - rc);
            }
        }
        o = vec4<f32>(c * a, a);
    }
    textureStore(out, p, o);
}

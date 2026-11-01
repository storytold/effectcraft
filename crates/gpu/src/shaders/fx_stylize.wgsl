// GPU effects (stylize and distort family): see src/fx_stylize.rs. Each entry point mirrors the CPU effect in
// effectcraft-effects operation for operation. Every name here is prefixed `fxs_` (the
// family files share one module). Coordinates are pixel centres (x + 0.5, y + 0.5) in buffer
// pixels, as in util::remap. Per-column / per-row tables in `data` hold coordinates the CPU
// computes in f64 (anything with a floor or a wrap), so tile and wrap boundaries match exactly.

const FXS_TAU: f32 = 6.2831855;
const FXS_PI: f32 = 3.1415927;
const FXS_HALF_PI: f32 = 1.5707964;

fn fxs_inside(p: vec2<i32>) -> bool {
    let d = out_dims();
    return p.x < d.x && p.y < d.y;
}

fn fxs_centre(p: vec2<i32>) -> vec2<f32> {
    return vec2<f32>(f32(p.x) + 0.5, f32(p.y) + 0.5);
}

// util::unpremul: straight colour (zero below alpha 1e-6) and alpha (≥ 0).
fn fxs_unpremul(px: vec4<f32>) -> vec4<f32> {
    let a = px.w;
    if (a > 1e-6) {
        return vec4<f32>(fxs_qdiv(px.xyz, a), a);
    }
    return vec4<f32>(0.0, 0.0, 0.0, max(a, 0.0));
}

// util::smoothstep.
fn fxs_smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    if (abs(e1 - e0) < 1e-9) {
        return select(1.0, 0.0, x < e0);
    }
    let t = clamp((x - e0) / (e1 - e0), 0.0, 1.0);
    return t * t * (3.0 - 2.0 * t);
}

// x / a rounded like the CPU's IEEE division (GPU division may be off by an ulp, which flips
// thresholds on 8 / 16 bpc colours that sit exactly on them): one residual correction.
fn fxs_qdiv(x: vec3<f32>, a: f32) -> vec3<f32> {
    let q = x / a;
    let r = fma(-q, vec3<f32>(a), x);
    // `q + r / a`, written as an fma so drivers (NVIDIA Vulkan) can't contract it another way
    // and round differently from the CPU.
    return fma(r / a, vec3<f32>(1.0), q);
}

fn fxs_rem2(i: i32) -> i32 {
    return imod(i, 2);
}

// ---------------------------------------------------------------- per-pixel colour effects
// u[0].x = mode:
//  1 Posterize (misc::posterize): f[0].x = levels − 1
//  2 Threshold (misc::threshold): f[0].x = level
//  3 CC Threshold: u[0].yz = (channel, invert); f[0].xy = (threshold, blend)
//  4 CC Threshold RGB: u[0].yzw = invert R, G, B; f[0] = (thresholds, blend)
//  5 Strobe Light on colour: u[0].y = operator; f[0] = (strobe colour, blend)
//  6 Strobe Light transparent (Image::scale_alpha): f[0].x = factor
//  7 CC Vignette: f[0] = (cx, cy, distance, amount); f[1].x = pin highlights

fn fxs_bin(v: f32, t: f32, inv: bool) -> f32 {
    return select(0.0, 1.0, (v >= t) != inv);
}

fn fxs_strobe_op(op: u32, v: f32, k: f32) -> f32 {
    switch op {
        case 1u: { return v + k; }
        case 2u: { return max(v - k, 0.0); }
        case 3u: { return v * k; }
        case 4u: { return abs(v - k); }
        case 5u: { return 1.0 - (1.0 - v) * (1.0 - k); }
        case 6u: { return max(v, k); }
        case 7u: { return min(v, k); }
        default: { return k; }
    }
}

@compute @workgroup_size(16, 16)
fn fxs_point(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!fxs_inside(p)) {
        return;
    }
    let px = textureLoad(src, p, 0);
    let mode = P.u[0].x;
    let a = px.w;
    if (mode == 6u) {
        textureStore(out, p, px * P.f[0].x);
        return;
    }
    if (a <= 0.0) {
        textureStore(out, p, px);
        return;
    }
    switch mode {
        case 1u: {
            let lv = P.f[0].x;
            let c = clamp(fxs_qdiv(px.xyz, a), vec3<f32>(0.0), vec3<f32>(1.0)) * lv;
            let o = vec3<f32>(round_away(c.x), round_away(c.y), round_away(c.z)) / lv;
            textureStore(out, p, vec4<f32>(o * a, a));
        }
        case 2u: {
            let v = select(0.0, 1.0, luminance(fxs_qdiv(px.xyz, a)) >= P.f[0].x);
            textureStore(out, p, vec4<f32>(vec3<f32>(v) * a, a));
        }
        case 3u: {
            let u = fxs_unpremul(px);
            let c = u.xyz;
            let t = P.f[0].x;
            let blend = P.f[0].y;
            let inv = P.u[0].z != 0u;
            var o = c;
            var na = u.w;
            switch P.u[0].y {
                case 1u: {
                    o = vec3<f32>(fxs_bin(c.x, t, inv), fxs_bin(c.y, t, inv), fxs_bin(c.z, t, inv));
                }
                case 2u: {
                    o = vec3<f32>(fxs_bin(rgb_to_hsl(c).y, t, inv));
                }
                case 3u: {
                    na = fxs_bin(u.w, t, inv);
                }
                default: {
                    o = vec3<f32>(fxs_bin(luminance(c), t, inv));
                }
            }
            o = o + (c - o) * blend;
            na = na + (u.w - na) * blend;
            textureStore(out, p, vec4<f32>(o * na, na));
        }
        case 4u: {
            let c = fxs_qdiv(px.xyz, a);
            let t = P.f[0].xyz;
            let o = vec3<f32>(fxs_bin(c.x, t.x, P.u[0].y != 0u), fxs_bin(c.y, t.y, P.u[0].z != 0u), fxs_bin(c.z, t.z, P.u[0].w != 0u));
            let r = o + (c - o) * P.f[0].w;
            textureStore(out, p, vec4<f32>(r * a, a));
        }
        case 5u: {
            let c = px.xyz / a;
            let k = P.f[0].xyz;
            let op = P.u[0].y;
            let o = vec3<f32>(fxs_strobe_op(op, c.x, k.x), fxs_strobe_op(op, c.y, k.y), fxs_strobe_op(op, c.z, k.z));
            let r = o + (c - o) * P.f[0].w;
            textureStore(out, p, vec4<f32>(r * a, a));
        }
        case 7u: {
            let u = fxs_unpremul(px);
            let q = fxs_centre(p);
            let dx = q.x - P.f[0].x;
            let dy = q.y - P.f[0].y;
            let r = sqrt(dx * dx + dy * dy) / P.f[0].z;
            let f0 = 1.0 / (1.0 + r * r);
            let f = f0 * f0;
            var k = 1.0 - P.f[0].w * (1.0 - f);
            let pin = P.f[1].x;
            if (pin > 0.0) {
                let l = clamp(luminance(u.xyz), 0.0, 1.0);
                k += (1.0 - k) * pin * l;
            }
            textureStore(out, p, vec4<f32>(max(u.xyz * k, vec3<f32>(0.0)) * u.w, u.w));
        }
        default: {
            textureStore(out, p, px);
        }
    }
}

// ---------------------------------------------------------------- inverse-mapped warps
// u[0].x = mode (all sample bilinear, transparent outside, unless noted):
//  1 Mirror (distort::mirror): f[0] = (cx, cy, nx, ny)
//  2 Spherize (distort::spherize): f[0] = (cx, cy, radius)
//  3 Polar Coordinates (distort::polar, edges repeated): u[0].y = to polar;
//    f[0] = (w, h, cx, cy); f[1] = (rmax, interpolation)
//  4 CC Slant (distort3::cc_slant): f[0] = (floor x, floor y, 1 / height, slant); f[1].x = x scale
//  5 CC Smear (distort3::cc_smear): f[0] = (from x, from y, dx, dy); f[1] = (len², reach, radius)
//  6 CC Split (distort3::split_impl): f[0] = (ax, ay, ux, uy); f[1] = (len, split 1, split 2, reach)
//  7 Optics Compensation (distort2::optics_compensation): u[0].y = reverse;
//    f[0] = (cx, cy, fit, f); f[1] = (reference radius, half FOV)
//  8 CC Kaleida (stylize2::kaleida, edges repeated): u[0].y = mirror;
//    f[0] = (cx, cy, size, rotation); f[1].x = segment angle
//  9 Liquify (distort4::liquify): u[0].xy = (_, mesh nx, mesh ny); f[0] = (cell, mesh offset x, y,
//    percentage); f[1] = (scale, buffer offset x, y); data = mesh offsets (x, y) per node
// 10 Twirl (Legacy) (distort3::twirl_legacy): f[0] = (cx, cy, radius, angle)
// 11 CC Ripple Pulse (distort3::cc_ripple_pulse): u[0].y = render bump; f[0] = (cx, cy, front,
//    speed); f[1] = (time span, wavelength, height)
// 12 CC Power Pin, full perspective (distort3::cc_power_pin): f[2..5] = inverse of the
//    square → quad matrix (rows); f[5] = source rect in layer pixels (x0, y0, w, h);
//    f[6] = (scale, buffer offset x, y)

fn fxs_mesh_d(i: u32, j: u32) -> vec2<f32> {
    let k = (j * P.u[0].y + i) * 2u;
    return vec2<f32>(data[k], data[k + 1u]);
}

// distort3::cc_ripple_pulse's wave.
fn fxs_pulse(r: f32) -> f32 {
    let front = P.f[0].z;
    let speed = P.f[0].w;
    let span = P.f[1].x;
    let age = (front - r) / max(speed, 1e-9);
    if (age < 0.0 || age > span) {
        return 0.0;
    }
    let fade = 1.0 - age / span;
    return sin(FXS_TAU * (r - front) / P.f[1].y) * fade;
}

// distort4::Mesh::at.
fn fxs_mesh_at(x: f32, y: f32) -> vec2<f32> {
    let cell = P.f[0].x;
    let nx = P.u[0].y;
    let ny = P.u[0].z;
    let fx = x / cell;
    let fy = y / cell;
    if (fx < 0.0 || fy < 0.0 || fx > f32(nx - 1u) || fy > f32(ny - 1u)) {
        return vec2<f32>(0.0);
    }
    let x0 = min(u32(floor(fx)), nx - 2u);
    let y0 = min(u32(floor(fy)), ny - 2u);
    let x1 = min(x0 + 1u, nx - 1u);
    let y1 = min(y0 + 1u, ny - 1u);
    let tx = fx - f32(x0);
    let ty = fy - f32(y0);
    let a = fxs_mesh_d(x0, y0);
    let b = fxs_mesh_d(x1, y0);
    let c = fxs_mesh_d(x0, y1);
    let d = fxs_mesh_d(x1, y1);
    let top = a + (b - a) * tx;
    let bot = c + (d - c) * tx;
    return top + (bot - top) * ty;
}

@compute @workgroup_size(16, 16)
fn fxs_warp(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!fxs_inside(p)) {
        return;
    }
    let q = fxs_centre(p);
    let x = q.x;
    let y = q.y;
    var s = q;
    var clamped = false;
    switch P.u[0].x {
        case 1u: {
            let n = P.f[0].zw;
            let d = (x - P.f[0].x) * n.x + (y - P.f[0].y) * n.y;
            if (d > 0.0) {
                s = vec2<f32>(x - 2.0 * d * n.x, y - 2.0 * d * n.y);
            }
        }
        case 2u: {
            let c = P.f[0].xy;
            let r = P.f[0].z;
            let dx = x - c.x;
            let dy = y - c.y;
            let d = sqrt(dx * dx + dy * dy);
            if (d < r && d != 0.0) {
                let nd = d / r;
                let k = (asin_p(nd) / FXS_HALF_PI) / nd;
                s = vec2<f32>(c.x + dx * k, c.y + dy * k);
            }
        }
        case 3u: {
            clamped = true;
            let w = P.f[0].x;
            let h = P.f[0].y;
            let cx = P.f[0].z;
            let cy = P.f[0].w;
            let rmax = P.f[1].x;
            var pp: vec2<f32>;
            if (P.u[0].y != 0u) {
                let dx = x - cx;
                let dy = y - cy;
                let a0 = atan2(dx, -dy);
                let a = a0 - floor(a0 / FXS_TAU) * FXS_TAU;
                let r = sqrt(dx * dx + dy * dy) / rmax;
                pp = vec2<f32>(a / FXS_TAU * w, h - r * h);
            } else {
                let a = x / w * FXS_TAU;
                let r = (h - y) / h * rmax;
                pp = vec2<f32>(cx + r * sin(a), cy - r * cos(a));
            }
            let amt = P.f[1].y;
            s = vec2<f32>(x + (pp.x - x) * amt, y + (pp.y - y) * amt);
        }
        case 4u: {
            let fl = P.f[0].xy;
            s = vec2<f32>(fl.x + (x - fl.x - (fl.y - y) * P.f[0].w) / P.f[1].x, fl.y + (y - fl.y) * P.f[0].z);
        }
        case 5u: {
            let fr = P.f[0].xy;
            let dx = P.f[0].z;
            let dy = P.f[0].w;
            let t = clamp(((x - fr.x) * dx + (y - fr.y) * dy) / P.f[1].x, 0.0, 1.0);
            let px = fr.x + dx * t;
            let py = fr.y + dy * t;
            let d = sqrt((x - px) * (x - px) + (y - py) * (y - py));
            let w0 = max(1.0 - d / P.f[1].z, 0.0);
            let w = w0 * w0 * (3.0 - 2.0 * w0);
            let k = P.f[1].y * w * t;
            s = vec2<f32>(x - dx * k, y - dy * k);
        }
        case 6u: {
            let a = P.f[0].xy;
            let ux = P.f[0].z;
            let uy = P.f[0].w;
            let nx = -uy;
            let ny = ux;
            let len = P.f[1].x;
            let reach = P.f[1].w;
            let rx = x - a.x;
            let ry = y - a.y;
            let t = (rx * ux + ry * uy) / len;
            let sd = rx * nx + ry * ny;
            if (t >= 0.0 && t <= 1.0) {
                let amt = select(P.f[1].z, P.f[1].y, sd >= 0.0);
                let g = amt * reach * sin(FXS_PI * t);
                let ad = abs(sd);
                if (ad < g) {
                    textureStore(out, p, vec4<f32>(0.0));
                    return;
                }
                let push = g * max(1.0 - (ad - g) / reach, 0.0);
                let ns = select(-1.0, 1.0, sd >= 0.0) * (ad - push);
                s = vec2<f32>(x + nx * (ns - sd), y + ny * (ns - sd));
            }
        }
        case 7u: {
            let c = P.f[0].xy;
            let fit = P.f[0].z;
            let f = P.f[0].w;
            let r_ref = P.f[1].x;
            let half = P.f[1].y;
            let dx = x - c.x;
            let dy = y - c.y;
            let r = sqrt(dx * dx + dy * dy) * fit;
            if (r >= 1e-9) {
                var rs: f32;
                if (P.u[0].y != 0u) {
                    rs = atan(r / f) / half * r_ref;
                } else {
                    let th = r / r_ref * half;
                    if (th >= FXS_HALF_PI - 1e-4) {
                        textureStore(out, p, vec4<f32>(0.0));
                        return;
                    }
                    rs = f * tan(th);
                }
                let k = rs / r * fit;
                s = vec2<f32>(c.x + dx * k, c.y + dy * k);
            }
        }
        case 8u: {
            clamped = true;
            let c = P.f[0].xy;
            let rot = P.f[0].w;
            let seg = P.f[1].x;
            let dx = x - c.x;
            let dy = y - c.y;
            let r = sqrt(dx * dx + dy * dy) / P.f[0].z;
            let a0 = atan2(dy, dx) - rot;
            var a = a0 - floor(a0 / seg) * seg;
            if (P.u[0].y != 0u && a > seg * 0.5) {
                a = seg - a;
            }
            s = vec2<f32>(c.x + r * cos(a + rot), c.y + r * sin(a + rot));
        }
        case 9u: {
            let scale = P.f[1].x;
            let o = P.f[1].yz;
            let lx = (x - o.x) / scale;
            let ly = (y - o.y) / scale;
            let d = fxs_mesh_at(lx - P.f[0].y, ly - P.f[0].z);
            if (d.x == 0.0 && d.y == 0.0) {
                textureStore(out, p, textureLoad(src, p, 0));
                return;
            }
            let pct = P.f[0].w;
            s = vec2<f32>((lx + d.x * pct) * scale + o.x, (ly + d.y * pct) * scale + o.y);
        }
        case 10u: {
            let c = P.f[0].xy;
            let r = P.f[0].z;
            let dx = x - c.x;
            let dy = y - c.y;
            let d = sqrt(dx * dx + dy * dy);
            if (d < r) {
                let a = -P.f[0].w * (1.0 - d / r);
                let sn = sin(a);
                let co = cos(a);
                s = vec2<f32>(c.x + dx * co - dy * sn, c.y + dx * sn + dy * co);
            }
        }
        case 11u: {
            let c = P.f[0].xy;
            let dx = x - c.x;
            let dy = y - c.y;
            let r = sqrt(dx * dx + dy * dy);
            let height = P.f[1].z;
            let d = fxs_pulse(r) * height;
            var u = vec2<f32>(0.0);
            if (r > 1e-9) {
                u = vec2<f32>(dx / r, dy / r);
            }
            var o = sample_bilinear(src, x - u.x * d, y - u.y * d);
            if (P.u[0].y != 0u && d != 0.0) {
                let slope = (fxs_pulse(r + 0.5) - fxs_pulse(r - 0.5)) * height;
                let k = clamp(1.0 - slope * 0.5, 0.0, 2.0);
                o = vec4<f32>(o.xyz * k, o.w);
            }
            textureStore(out, p, o);
            return;
        }
        case 12u: {
            let r0 = P.f[2];
            let r1 = P.f[3];
            let r2 = P.f[4];
            var u = r0.x * x + r0.y * y + r0.z;
            var v = r1.x * x + r1.y * y + r1.z;
            let w = r2.x * x + r2.y * y + r2.z;
            if (w != 1.0 && w != 0.0) {
                u = u / w;
                v = v / w;
            }
            // Perspective below 100 %: blend toward the bilinear (non-perspective) inverse, found
            // by Newton iterations from the projective guess as on the CPU; positions relative to
            // the first corner keep f32's precision. f[7] = (corner 0, perspective, on);
            // f[8] = (corner 1, corner 2) and f[9].xy = corner 3, relative to corner 0.
            if (P.f[7].w != 0.0) {
                let q1 = P.f[8].xy;
                let q2 = P.f[8].zw;
                let q3 = P.f[9].xy;
                let k = q2 - q3 - q1;
                let t = vec2<f32>(x, y) - P.f[7].xy;
                var bu = u;
                var bv = v;
                for (var i = 0; i < 8; i++) {
                    let f = q1 * bu + q3 * bv + k * (bu * bv) - t;
                    if (abs(f.x) + abs(f.y) < 1e-4) {
                        break;
                    }
                    let ju = q1 + k * bv;
                    let jv = q3 + k * bu;
                    let det = ju.x * jv.y - jv.x * ju.y;
                    if (abs(det) < 1e-12) {
                        break;
                    }
                    bu -= (jv.y * f.x - jv.x * f.y) / det;
                    bv -= (-ju.y * f.x + ju.x * f.y) / det;
                }
                let persp = P.f[7].z;
                u = bu + (u - bu) * persp;
                v = bv + (v - bv) * persp;
            }
            // The CPU's 1e-9 edge allowance disappears in f32 at 1.0. Allow eight
            // f32 ULPs for the inverse solve so exact quad-edge pixels remain covered.
            let edge = 0.00000095367431640625;
            if (u < -edge || u > 1.0 + edge || v < -edge || v > 1.0 + edge) {
                textureStore(out, p, vec4<f32>(0.0));
                return;
            }
            u = clamp(u, 0.0, 1.0);
            v = clamp(v, 0.0, 1.0);
            let rc = P.f[5];
            let lx = rc.x + u * rc.z;
            let ly = rc.y + v * rc.w;
            let o = P.f[6];
            s = vec2<f32>(lx * o.x + o.y, ly * o.x + o.z);
        }
        default: {}
    }
    if (clamped) {
        textureStore(out, p, sample_bilinear_clamped(src, s.x, s.y));
    } else {
        textureStore(out, p, sample_bilinear(src, s.x, s.y));
    }
}

// ---------------------------------------------------------------- CC Flo Motion (distort4::flo_motion)

// One knot's pull at layer point q: f[k] = (knot x, knot y, amount, _).
fn fxs_flo_knot(q: vec2<f32>, k: vec4<f32>) -> vec2<f32> {
    if (k.z == 0.0) {
        return q;
    }
    let d = q - k.xy;
    let r = sqrt(d.x * d.x + d.y * d.y) / P.f[2].y;
    let w = powz(1.0 - min(r, 1.0), P.f[2].x);
    let s = max(1.0 - clamp(k.z, -4.0, 0.95) * w, 0.02);
    return k.xy + d * s;
}

// u[0] = (supersampling, tile edges); f[0], f[1] = knots; f[2] = (falloff, radius, layer w,
// layer h); f[3] = (scale, buffer offset x, y)
@compute @workgroup_size(16, 16)
fn fxs_flomotion(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!fxs_inside(p)) {
        return;
    }
    let ss = P.u[0].x;
    let scale = P.f[3].x;
    let off = P.f[3].yz;
    let lw = P.f[2].z;
    let lh = P.f[2].w;
    var acc = vec4<f32>(0.0);
    for (var sy = 0u; sy < ss; sy++) {
        for (var sx = 0u; sx < ss; sx++) {
            let px = f32(p.x) + (f32(sx) + 0.5) / f32(ss);
            let py = f32(p.y) + (f32(sy) + 0.5) / f32(ss);
            var q = vec2<f32>((px - off.x) / scale, (py - off.y) / scale);
            q = fxs_flo_knot(q, P.f[0]);
            q = fxs_flo_knot(q, P.f[1]);
            if (P.u[0].y != 0u) {
                q = vec2<f32>(q.x - floor(q.x / lw) * lw, q.y - floor(q.y / lh) * lh);
            }
            acc += sample_bilinear(src, q.x * scale + off.x, q.y * scale + off.y);
        }
    }
    textureStore(out, p, acc / f32(ss * ss));
}

// ---------------------------------------------------------------- separable table warps
// The source point is (data[x], data[w + y]) (computed in f64 per column and per row), then
// mixed with the original: out = sample + (orig − sample) · blend (Offset, CC Tiler).
// u[0] = (buffer width, repeat edges); f[0].x = blend
@compute @workgroup_size(16, 16)
fn fxs_table(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!fxs_inside(p)) {
        return;
    }
    let w = P.u[0].x;
    let sx = data[u32(p.x)];
    let sy = data[w + u32(p.y)];
    var s: vec4<f32>;
    if (P.u[0].y != 0u) {
        s = sample_bilinear_clamped(src, sx, sy);
    } else {
        s = sample_bilinear(src, sx, sy);
    }
    let o = textureLoad(src, p, 0);
    textureStore(out, p, s + (o - s) * P.f[0].x);
}

// CC Griddler (distort2::cc_griddler). data: per column (tile centre x, local x), per row
// (tile centre y, local y). u[0] = (buffer width, cut tiles); f[0] = (cos, sin, h scale, v scale);
// f[1].x = cut half size (0.45 · tile)
@compute @workgroup_size(16, 16)
fn fxs_griddler(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!fxs_inside(p)) {
        return;
    }
    let w = P.u[0].x;
    let x = u32(p.x);
    let y = u32(p.y);
    let tcx = data[2u * x];
    let lx = data[2u * x + 1u];
    let tcy = data[2u * w + 2u * y];
    let ly = data[2u * w + 2u * y + 1u];
    if (P.u[0].y != 0u && max(abs(lx), abs(ly)) > P.f[1].x) {
        textureStore(out, p, vec4<f32>(0.0));
        return;
    }
    let cr = P.f[0].x;
    let sr = P.f[0].y;
    let rx = lx * cr + ly * sr;
    let ry = -lx * sr + ly * cr;
    textureStore(out, p, sample_bilinear(src, tcx + rx / P.f[0].z, tcy + ry / P.f[0].w));
}

// Motion Tile (stylize2::motion_tile) into a new output size. data: per output column (tile
// index, fraction) of tx, then per row of ty. u[0] = (output width, horizontal phase shift,
// mirror edges); f[0] = (phase, layer w · scale, layer h · scale); f[1].xy = source buffer offset
@compute @workgroup_size(16, 16)
fn fxs_motiontile(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!fxs_inside(p)) {
        return;
    }
    let w = P.u[0].x;
    let x = u32(p.x);
    let y = u32(p.y);
    let ixc = data[2u * x];
    let fxc = data[2u * x + 1u];
    let iyr = data[2u * w + 2u * y];
    let fyr = data[2u * w + 2u * y + 1u];
    let phase = P.f[0].x;
    var i = ixc;
    var fx = fxc;
    var j = iyr;
    var fy = fyr;
    if (P.u[0].y != 0u) {
        let t = fxc + phase * iyr;
        i = ixc + floor(t);
        fx = t - floor(t);
    } else {
        let t = fyr + phase * ixc;
        j = iyr + floor(t);
        fy = t - floor(t);
    }
    if (P.u[0].z != 0u) {
        if (fxs_rem2(i32(i)) == 1) {
            fx = 1.0 - fx;
        }
        if (fxs_rem2(i32(j)) == 1) {
            fy = 1.0 - fy;
        }
    }
    textureStore(out, p, sample_bilinear(src, fx * P.f[0].y + P.f[1].x, fy * P.f[0].z + P.f[1].y));
}

// CC RepeTile (stylize2::repetile) into a new output size. data: per output column (tile index,
// position in the tile), then per row. u[0] = (output width, tiling); f[0] = layer rect (x0, y0, w, h)
@compute @workgroup_size(16, 16)
fn fxs_repetile(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!fxs_inside(p)) {
        return;
    }
    let w = P.u[0].x;
    let x = u32(p.x);
    let y = u32(p.y);
    let ix = i32(data[2u * x]);
    var fx = data[2u * x + 1u];
    let iy = i32(data[2u * w + 2u * y]);
    var fy = data[2u * w + 2u * y + 1u];
    let odd = fxs_rem2(ix + iy) == 1;
    var flip_x = false;
    var flip_y = false;
    switch P.u[0].y {
        case 1u: {
            flip_x = fxs_rem2(ix) == 1;
            flip_y = fxs_rem2(iy) == 1;
        }
        case 2u: {
            flip_x = odd;
        }
        case 3u: {
            flip_y = odd;
        }
        case 4u: {
            flip_x = odd;
            flip_y = odd;
        }
        default: {}
    }
    let r = P.f[0];
    if (flip_x) {
        fx = r.z - fx;
    }
    if (flip_y) {
        fy = r.w - fy;
    }
    textureStore(out, p, sample_bilinear(src, r.x + fx, r.y + fy));
}

// ---------------------------------------------------------------- Magnify (distort2::magnify)
// data: per column (dx, sx, floor(sx) + 0.5), then per row (dy, sy, floor(sy) + 0.5).
// u[0] = (buffer width, square, scaling, blend: 0 none, 1 normal, 2 blend mode);
// u[1] = (blend mode id, seed); f[0] = (size, feather, opacity, magnification); f[1].x = size − feather
@compute @workgroup_size(16, 16)
fn fxs_magnify(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!fxs_inside(p)) {
        return;
    }
    let bw = P.u[0].x;
    let x = u32(p.x);
    let y = u32(p.y);
    let dx = data[3u * x];
    let sx = data[3u * x + 1u];
    let nx = data[3u * x + 2u];
    let dy = data[3u * bw + 3u * y];
    let sy = data[3u * bw + 3u * y + 1u];
    let ny = data[3u * bw + 3u * y + 2u];
    var d: f32;
    if (P.u[0].y != 0u) {
        d = max(abs(dx), abs(dy));
    } else {
        d = sqrt(dx * dx + dy * dy);
    }
    let size = P.f[0].x;
    let feather = P.f[0].y;
    var w: f32;
    if (feather > 0.01) {
        w = 1.0 - fxs_smoothstep(P.f[1].x, size, d);
    } else {
        w = clamp(size - d + 0.5, 0.0, 1.0);
    }
    w = w * P.f[0].z;
    let px = textureLoad(src, p, 0);
    let blend = P.u[0].w;
    if (w <= 0.0) {
        if (blend == 0u) {
            textureStore(out, p, vec4<f32>(0.0));
        } else {
            textureStore(out, p, px);
        }
        return;
    }
    var m: vec4<f32>;
    switch P.u[0].z {
        case 1u: {
            m = sample_bicubic(src, sx, sy);
        }
        case 2u: {
            let seed = P.u[1].y;
            let k = max(1.0 - 1.0 / P.f[0].w, 0.0);
            let j1 = (hash_noise(x, y ^ (1u << 20u), seed) - 0.5) * k;
            let j2 = (hash_noise(x, y ^ (2u << 20u), seed) - 0.5) * k;
            m = sample_bilinear(src, nx + j1, ny + j2);
        }
        default: {
            m = sample_bilinear(src, nx, ny);
        }
    }
    if (blend == 0u) {
        textureStore(out, p, m * w);
    } else if (blend == 1u) {
        textureStore(out, p, px + (m - px) * w);
    } else {
        textureStore(out, p, blend_pixel(P.u[1].x, px, m * w, 0.5));
    }
}

// ---------------------------------------------------------------- Scatter (stylize2::scatter)
// u[0] = (grain, seed); f[0].x = amount
@compute @workgroup_size(16, 16)
fn fxs_scatter(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!fxs_inside(p)) {
        return;
    }
    let grain = P.u[0].x;
    let seed = P.u[0].y;
    let amt = P.f[0].x;
    let x = u32(p.x);
    let y = u32(p.y);
    var dx = 0.0;
    var dy = 0.0;
    if (grain != 2u) {
        dx = (hash_noise(x, y, seed) - 0.5) * 2.0 * amt;
    }
    if (grain != 1u) {
        dy = (hash_noise(x, y, seed ^ 0x5bd1u) - 0.5) * 2.0 * amt;
    }
    textureStore(out, p, tex_get(src, i32(round_away(f32(p.x) + dx)), i32(round_away(f32(p.y) + dy))));
}

// ---------------------------------------------------------------- Brush Strokes (stylize2::brush_strokes)
// data: cell column per x (w), cell row per y (h), then per cell (dx, dy, length, jitter, samples,
// painted). u[0] = (buffer width, cell columns, paint surface, buffer height); f[0].x = blend
@compute @workgroup_size(16, 16)
fn fxs_brush(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!fxs_inside(p)) {
        return;
    }
    let w = P.u[0].x;
    let h = P.u[0].w;
    let cx = u32(data[u32(p.x)]);
    let cy = u32(data[w + u32(p.y)]);
    let base = w + h + (cy * P.u[0].y + cx) * 6u;
    let dx = data[base];
    let dy = data[base + 1u];
    let l = data[base + 2u];
    let jit = data[base + 3u];
    let n = u32(data[base + 4u]);
    let m = data[base + 5u];
    let q = fxs_centre(p);
    let bx = q.x - dy * jit;
    let by = q.y + dx * jit;
    var acc = vec4<f32>(0.0);
    for (var i = 0u; i < n; i++) {
        var t = 0.0;
        if (n != 1u) {
            t = (f32(i) / f32(n - 1u) - 0.5) * l;
        }
        acc += sample_bilinear_clamped(src, bx + dx * t, by + dy * t);
    }
    let stroke = acc / f32(n);
    let orig = textureLoad(src, p, 0);
    let surface = P.u[0].z;
    var base_px = orig;
    if (surface == 1u) {
        base_px = vec4<f32>(0.0);
    } else if (surface == 2u) {
        base_px = vec4<f32>(1.0);
    } else if (surface == 3u) {
        base_px = vec4<f32>(0.0, 0.0, 0.0, 1.0);
    }
    var painted: vec4<f32>;
    if (surface == 2u || surface == 3u) {
        let k = 1.0 - stroke.w * m;
        painted = stroke * m + base_px * k;
    } else {
        painted = base_px + (stroke - base_px) * m;
    }
    textureStore(out, p, painted + (orig - painted) * P.f[0].x);
}

// ---------------------------------------------------------------- Roughen Edges (stylize2::roughen_edges)

// noise::fbm.
fn fxs_fbm(u: f32, v: f32, z: f32, seed: u32, octaves0: f32) -> f32 {
    let octaves = clamp(octaves0, 1.0, 20.0);
    let n = u32(ceil(octaves));
    let frac = octaves - floor(octaves);
    var sum = 0.0;
    var norm = 0.0;
    var amp = 1.0;
    var f = 1.0;
    for (var o = 0u; o < n; o++) {
        var w = 1.0;
        if (o + 1u == n && frac > 0.0) {
            w = frac;
        }
        sum += value_noise(u * f, v * f, z + f32(o) * 7.31, seed + o) * amp * w;
        norm += amp * w;
        amp *= 0.5;
        f *= 2.0;
    }
    return sum / max(norm, 1e-6);
}

// aux = the (blurred) alpha in .w. u[0] = (seed, cycle evolution, coloured);
// f[0] = (offset x, offset y, scale · stretch x, scale · stretch y); f[1] = (frequency, evolution,
// cycle, cycle weight); f[2] = (octaves, influence, influence multiplier, sharpness);
// f[3] = (edge colour, sharpness multiplier)
@compute @workgroup_size(16, 16)
fn fxs_roughen(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!fxs_inside(p)) {
        return;
    }
    let px = textureLoad(src, p, 0);
    if (px.w <= 0.0) {
        textureStore(out, p, px);
        return;
    }
    let dv = textureLoad(aux, p, 0).w;
    let fm = P.f[1].x;
    let nx = (f32(p.x) + 0.5 - P.f[0].x) / P.f[0].z * fm;
    let ny = (f32(p.y) + 0.5 - P.f[0].y) / P.f[0].w * fm;
    let seed = P.u[0].x;
    let evo = P.f[1].y;
    let oct = P.f[2].x;
    var n = fxs_fbm(nx, ny, evo, seed, oct);
    if (P.u[0].y != 0u) {
        n += (fxs_fbm(nx, ny, evo - P.f[1].z, seed, oct) - n) * P.f[1].w;
    }
    let infl = P.f[2].y;
    let s = dv - 0.5 + (n - 0.5) * infl * P.f[2].z * 0.5;
    let m = clamp(s * P.f[2].w * P.f[3].w * 4.0 + 0.5, 0.0, 1.0);
    let u = fxs_unpremul(px);
    var c = u.xyz;
    if (P.u[0].z != 0u) {
        let band = 1.0 - fxs_smoothstep(0.5, 0.9, dv + (n - 0.5) * infl * 0.5);
        c = c + (P.f[3].xyz - c) * band;
    }
    let a = u.w * m;
    textureStore(out, p, vec4<f32>(c * a, a));
}

// ---------------------------------------------------------------- Texturize (stylize2::texturize)

// The texture layer placed into the buffer (transition::place_layer) as luminance
// (Plane::luma). aux = the texture layer's buffer; data: per column x, then per row y in its
// pixels. u[0].x = buffer width
@compute @workgroup_size(16, 16)
fn fxs_tex_luma(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!fxs_inside(p)) {
        return;
    }
    let w = P.u[0].x;
    let t = sample_bilinear(aux, data[u32(p.x)], data[w + u32(p.y)]);
    let l = luminance(fxs_unpremul(t).xyz);
    textureStore(out, p, vec4<f32>(l));
}

// Relief shading. aux = the blurred texture luminance; f[0] = (light x, light y, contrast)
@compute @workgroup_size(16, 16)
fn fxs_texturize(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!fxs_inside(p)) {
        return;
    }
    let gx = (tex_get_clamped(aux, p.x + 1, p.y).x - tex_get_clamped(aux, p.x - 1, p.y).x) * 0.5;
    let gy = (tex_get_clamped(aux, p.x, p.y + 1).x - tex_get_clamped(aux, p.x, p.y - 1).x) * 0.5;
    let shade = max(1.0 - P.f[0].z * 4.0 * (gx * P.f[0].x + gy * P.f[0].y), 0.0);
    let px = textureLoad(src, p, 0);
    textureStore(out, p, vec4<f32>(px.xyz * shade, px.w));
}

// ---------------------------------------------------------------- Glow (misc::glow) extras

// Bright pass with the Arbitrary Map: as `glow_bright` (same f[0] / u[0] layout, see
// kernels.wgsl) with the red, green and blue curves at data offsets f[1].xyz (−1 = identity).
@compute @workgroup_size(16, 16)
fn fxs_glow_map(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!fxs_inside(p)) {
        return;
    }
    let px = textureLoad(src, p, 0);
    let a = px.w;
    if (a <= 0.0) {
        textureStore(out, p, vec4<f32>(0.0));
        return;
    }
    let thr = P.f[0].x;
    var l = luminance(px.xyz / a);
    if (P.u[0].y != 0u) {
        l = a;
    }
    let k = clamp((l - thr) / max(1.0 - thr, 1e-3), 0.0, 1.0);
    let t = glow_ab_t(l);
    let c = vec3<f32>(curve_eval(P.f[1].x, t), curve_eval(P.f[1].y, t), curve_eval(P.f[1].z, t));
    // Based on the colour channels the glow covers the layer's whole alpha (effects::misc::glow).
    var ga = a;
    if (P.u[0].y != 0u) {
        ga = k * a;
    }
    textureStore(out, p, vec4<f32>(c * k * a, ga));
}

// On Top with another Glow Operation: blend_pixel(mode, original, glow · intensity, 0).
// aux = the blurred glow; u[0].x = blend mode id; u[0].y = based on alpha (intensity also
// scales the glow's alpha); f[0].x = intensity
@compute @workgroup_size(16, 16)
fn fxs_glow_op(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!fxs_inside(p)) {
        return;
    }
    let gs = textureLoad(aux, p, 0);
    var g = vec4<f32>(gs.xyz * P.f[0].x, gs.w);
    if (P.u[0].y != 0u) {
        g.w = gs.w * P.f[0].x;
    }
    var o = blend_pixel(P.u[0].x, textureLoad(src, p, 0), g, 0.0);
    o.w = clamp(o.w, 0.0, 1.0);
    textureStore(out, p, o);
}

// ---------------------------------------------------------------- accumulation (Transform motion blur)

// out = src + aux
@compute @workgroup_size(16, 16)
fn fxs_add(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!fxs_inside(p)) {
        return;
    }
    textureStore(out, p, textureLoad(src, p, 0) + textureLoad(aux, p, 0));
}

// out = src / f[0].x
@compute @workgroup_size(16, 16)
fn fxs_div(@builtin(global_invocation_id) gid: vec3<u32>) {
    let p = vec2<i32>(gid.xy);
    if (!fxs_inside(p)) {
        return;
    }
    textureStore(out, p, textureLoad(src, p, 0) / P.f[0].x);
}

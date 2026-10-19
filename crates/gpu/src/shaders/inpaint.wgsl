// GPU Content-Aware Fill: the membrane (harmonic) fill and PatchMatch synthesis of
// effectcraft_raster::inpaint, operation for operation (the CPU reference is the oracle; see
// crates/gpu/tests_inpaint.rs).
//
// Bindings are the shared ones (common.wgsl): `src` / `aux` are the input textures (an image, a
// coarser level, or the nearest-neighbour field), `out` is the storage texture and `data` is a
// per-pixel float table — the level's flags:
//   * membrane kernels: data[i] = 1 where the pixel is known (the hole's complement);
//   * PatchMatch kernels: data[i] = bit 0 hole, bit 1 near (a pixel whose patch touches the hole).
//
// Parameter block (P.u = u32, P.f = f32):
//   membrane kernels:  u[0] = (w, h, coarseW, coarseH), u[1] = (x0, y0, 0, 0) (dispatch
//                      origin), u[3].x = this level's offset into `data`
//   down kernels:      u[0] = (srcW, srcH, dstW, dstH)
//   PatchMatch kernels: u[0] = (w, h, r, parity), u[1] = (x0, y0, 0, 0) (dispatch origin),
//                       u[2] = (seed, iteration, 0, EM round), f[0] = (sigma², 0, 0, 0)
// The nearest-neighbour field texture holds one entry per pixel: r, g = source centre ("px"),
// b = SSD distance (inf where unset), a = 1 where the entry is set.

const IMP_INF: f32 = 1e30;

fn imp_flags(i: i32) -> u32 {
    return u32(max(data[i], 0.0));
}

fn imp_hole(i: i32) -> bool {
    return (imp_flags(i) & 1u) != 0u;
}

fn imp_near(i: i32) -> bool {
    return (imp_flags(i) & 2u) != 0u;
}

// A source centre whose patch is fully known and inside the image (Pm::valid_source).
fn imp_valid_source(x: i32, y: i32, w: i32, h: i32, r: i32) -> bool {
    if (x < r || y < r || x + r >= w || y + r >= h) {
        return false;
    }
    return !imp_near(y * w + x);
}

// PatchMatch's hashing rnd (raster::inpaint::rnd), bit for bit: the CPU's random stream is a
// contract of the algorithm, not an implementation detail.
fn imp_rnd(seed: u32, a: u32, b: u32) -> u32 {
    var h = seed * 0x9E3779B9u ^ a * 0x85EBCA6Bu ^ b * 0xC2B2AE35u;
    h = h ^ (h >> 16u);
    h = h * 0x7FEB352Du;
    h = h ^ (h >> 15u);
    h = h * 0x846CA68Bu;
    return h ^ (h >> 16u);
}

// SSD of the patch at target (tx, ty) (clipped to the image) against source (sx, sy)
// (Pm::dist). `cap` short-circuits a match that is already worse.
fn imp_dist(tx: i32, ty: i32, sx: i32, sy: i32, w: i32, h: i32, r: i32, cap: f32) -> f32 {
    var d = 0.0;
    var n = 0.0;
    for (var dy = -r; dy <= r; dy = dy + 1) {
        let ty2 = ty + dy;
        if (ty2 < 0 || ty2 >= h) {
            continue;
        }
        let sy2 = sy + dy;
        for (var dx = -r; dx <= r; dx = dx + 1) {
            let tx2 = tx + dx;
            if (tx2 < 0 || tx2 >= w) {
                continue;
            }
            let a = textureLoad(aux, vec2<i32>(tx2, ty2), 0);
            let b = textureLoad(aux, vec2<i32>(sx + dx, sy2), 0);
            let e = a - b;
            d = d + e.x * e.x + e.y * e.y + e.z * e.z + e.w * e.w;
            n = n + 1.0;
        }
        if (d > cap * max(n, 1.0)) {
            return IMP_INF;
        }
    }
    return d / max(n, 1.0);
}

// ---------------------------------------------------------------- membrane

// membrane_core's coarse level: the known-weighted 2×2 average.
@compute @workgroup_size(16, 16)
fn imp_down(@builtin(global_invocation_id) gid: vec3<u32>) {
    let w = i32(P.u[0].x);
    let h = i32(P.u[0].y);
    let cw = i32(P.u[0].z);
    let ch = i32(P.u[0].w);
    let cx = i32(gid.x);
    let cy = i32(gid.y);
    if (cx >= cw || cy >= ch) {
        return;
    }
    var s = vec4<f32>(0.0);
    var n = 0.0;
    for (var dy = 0; dy < 2; dy = dy + 1) {
        for (var dx = 0; dx < 2; dx = dx + 1) {
            let fx = 2 * cx + dx;
            let fy = 2 * cy + dy;
            if (fx < w && fy < h && data[i32(P.u[3].x) + fy * w + fx] > 0.5) {
                s = s + textureLoad(src, vec2<i32>(fx, fy), 0);
                n = n + 1.0;
            }
        }
    }
    var v = vec4<f32>(0.0);
    if (n > 0.0) {
        v = s / n;
    }
    textureStore(out, vec2<i32>(cx, cy), v);
}

// membrane_core's coarse-to-fine step: unknown pixels take the nearest coarse value. The fine
// image is copied into `out` first, so writing only the unknown pixels keeps the known ones.
// Dispatched over the unknown pixels' bounds (`P.u[1]`); the coarse level is valid everywhere.
@compute @workgroup_size(16, 16)
fn imp_up(@builtin(global_invocation_id) gid: vec3<u32>) {
    let w = i32(P.u[0].x);
    let h = i32(P.u[0].y);
    let cw = i32(P.u[0].z);
    let ch = i32(P.u[0].w);
    let x = i32(P.u[1].x) + i32(gid.x);
    let y = i32(P.u[1].y) + i32(gid.y);
    if (x >= w || y >= h || data[i32(P.u[3].x) + y * w + x] > 0.5) {
        return;
    }
    let cx = min(x / 2, cw - 1);
    let cy = min(y / 2, ch - 1);
    textureStore(out, vec2<i32>(x, y), textureLoad(src, vec2<i32>(cx, cy), 0));
}

// One relaxation of the unknown pixels (membrane_core's Jacobi sweep). `src` is the current
// state and `aux` is unused. Dispatched over the unknown pixels' bounds (`P.u[1]`): known
// pixels never move, so nothing outside them needs writing. Both `src` and `out` are valid
// across the image (the fine level's, a copy of it, or the full downsample).
@compute @workgroup_size(16, 16)
fn imp_jacobi(@builtin(global_invocation_id) gid: vec3<u32>) {
    let w = i32(P.u[0].x);
    let h = i32(P.u[0].y);
    let x = i32(P.u[1].x) + i32(gid.x);
    let y = i32(P.u[1].y) + i32(gid.y);
    if (x >= w || y >= h) {
        return;
    }
    var v = textureLoad(src, vec2<i32>(x, y), 0);
    if (data[i32(P.u[3].x) + y * w + x] <= 0.5) {
        var s = vec4<f32>(0.0);
        var n = 0.0;
        if (x > 0) {
            s = s + textureLoad(src, vec2<i32>(x - 1, y), 0);
            n = n + 1.0;
        }
        if (x + 1 < w) {
            s = s + textureLoad(src, vec2<i32>(x + 1, y), 0);
            n = n + 1.0;
        }
        if (y > 0) {
            s = s + textureLoad(src, vec2<i32>(x, y - 1), 0);
            n = n + 1.0;
        }
        if (y + 1 < h) {
            s = s + textureLoad(src, vec2<i32>(x, y + 1), 0);
            n = n + 1.0;
        }
        if (n > 0.0) {
            v = s / n;
        }
    }
    textureStore(out, vec2<i32>(x, y), v);
}

// ---------------------------------------------------------------- PatchMatch

// PatchMatch's pyramid downsample (downsample): the level's image, hole pixels excluded from the
// average, over `out`'s size.
@compute @workgroup_size(16, 16)
fn imp_pm_down(@builtin(global_invocation_id) gid: vec3<u32>) {
    let w = i32(P.u[0].x);
    let h = i32(P.u[0].y);
    let cw = i32(P.u[0].z);
    let ch = i32(P.u[0].w);
    let cx = i32(gid.x);
    let cy = i32(gid.y);
    if (cx >= cw || cy >= ch) {
        return;
    }
    var s = vec4<f32>(0.0);
    var n = 0.0;
    for (var dy = 0; dy < 2; dy = dy + 1) {
        for (var dx = 0; dx < 2; dx = dx + 1) {
            let fx = min(2 * cx + dx, w - 1);
            let fy = min(2 * cy + dy, h - 1);
            if (!imp_hole(fy * w + fx)) {
                s = s + textureLoad(src, vec2<i32>(fx, fy), 0);
                n = n + 1.0;
            }
        }
    }
    var v = vec4<f32>(0.0);
    if (n > 0.0) {
        v = s / n;
    }
    textureStore(out, vec2<i32>(cx, cy), v);
}

// PatchMatch's coarse-to-fine step: bilinear upsample of the coarse image into this level's hole
// (patch_fill's upsample loop). The fine image is copied into `out` first.
@compute @workgroup_size(16, 16)
fn imp_pm_up(@builtin(global_invocation_id) gid: vec3<u32>) {
    let w = i32(P.u[0].x);
    let h = i32(P.u[0].y);
    let cw = i32(P.u[0].z);
    let ch = i32(P.u[0].w);
    let x = i32(P.u[1].x) + i32(gid.x);
    let y = i32(P.u[1].y) + i32(gid.y);
    if (x >= w || y >= h || !imp_hole(y * w + x)) {
        return;
    }
    let fx = clamp((f32(x) + 0.5) / 2.0 - 0.5, 0.0, f32(cw - 1));
    let fy = clamp((f32(y) + 0.5) / 2.0 - 0.5, 0.0, f32(ch - 1));
    let x0 = i32(floor(fx));
    let y0 = i32(floor(fy));
    let x1 = min(x0 + 1, cw - 1);
    let y1 = min(y0 + 1, ch - 1);
    let tx = fx - f32(x0);
    let ty = fy - f32(y0);
    let a = textureLoad(src, vec2<i32>(x0, y0), 0);
    let b = textureLoad(src, vec2<i32>(x1, y0), 0);
    let c = textureLoad(src, vec2<i32>(x0, y1), 0);
    let d = textureLoad(src, vec2<i32>(x1, y1), 0);
    let top = a + (b - a) * tx;
    let bot = c + (d - c) * tx;
    textureStore(out, vec2<i32>(x, y), top + (bot - top) * ty);
}

// The NNF's coarse-to-fine step: ×2 (patch_fill's nnf upsample).
@compute @workgroup_size(16, 16)
fn imp_nnf_up(@builtin(global_invocation_id) gid: vec3<u32>) {
    let w = i32(P.u[0].x);
    let h = i32(P.u[0].y);
    let cw = i32(P.u[0].z);
    let ch = i32(P.u[0].w);
    let x = i32(gid.x);
    let y = i32(gid.y);
    if (x >= w || y >= h) {
        return;
    }
    let cx = min(x / 2, cw - 1);
    let cy = min(y / 2, ch - 1);
    let c = textureLoad(src, vec2<i32>(cx, cy), 0);
    var o = vec4<f32>(0.0);
    if (c.a > 0.5) {
        o = vec4<f32>(c.r * 2.0 + f32(x % 2), c.g * 2.0 + f32(y % 2), 0.0, 1.0);
    }
    textureStore(out, vec2<i32>(x, y), o);
}

// A random valid source centre, by rejection (the CPU draws from its compacted source list; the
// distribution over the valid centres is the same, the stream differs from the list's order).
fn imp_any_source(seed: u32, i: u32, k: u32, w: i32, h: i32, r: i32) -> vec2<i32> {
    for (var n = 0u; n < 32u; n = n + 1u) {
        let a = imp_rnd(seed, i, k + n);
        let x = i32(a % u32(w));
        let y = i32((a >> 16u) % u32(h));
        if (imp_valid_source(x, y, w, h, r)) {
            return vec2<i32>(x, y);
        }
    }
    return vec2<i32>(-1, -1);
}

// A round's initial field: the inherited source where it is still valid, a random one otherwise
// (pm_level's initialise/validate step).
@compute @workgroup_size(16, 16)
fn imp_pm_init(@builtin(global_invocation_id) gid: vec3<u32>) {
    let w = i32(P.u[0].x);
    let h = i32(P.u[0].y);
    let r = i32(P.u[0].z);
    let x = i32(P.u[1].x) + i32(gid.x);
    let y = i32(P.u[1].y) + i32(gid.y);
    if (x >= w || y >= h) {
        return;
    }
    let i = y * w + x;
    var o = vec4<f32>(0.0, 0.0, IMP_INF, 0.0);
    if (imp_near(i)) {
        let prev = textureLoad(src, vec2<i32>(x, y), 0);
        var s = vec2<i32>(i32(prev.r), i32(prev.g));
        if (prev.a <= 0.5 || !imp_valid_source(s.x, s.y, w, h, r)) {
            // rnd(seed ^ e, i, 0), the round's random source (see imp_any_source).
            s = imp_any_source(P.u[2].x ^ P.u[2].w, u32(i), 0u, w, h, r);
        }
        if (s.x >= 0) {
            let d = imp_dist(x, y, s.x, s.y, w, h, r, IMP_INF);
            o = vec4<f32>(f32(s.x), f32(s.y), d, 1.0);
        }
    }
    textureStore(out, vec2<i32>(x, y), o);
}

// One propagation pass: every target pixel tries its four neighbours' sources shifted by the
// step that would move that neighbour's match onto this pixel. The checkerboard parity in P.u[0].w
// makes the pass race-free (a pixel of one parity only reads the other's).
@compute @workgroup_size(16, 16)
fn imp_pm_prop(@builtin(global_invocation_id) gid: vec3<u32>) {
    let w = i32(P.u[0].x);
    let h = i32(P.u[0].y);
    let r = i32(P.u[0].z);
    let x = i32(P.u[1].x) + i32(gid.x);
    let y = i32(P.u[1].y) + i32(gid.y);
    if (x >= w || y >= h) {
        return;
    }
    let cur = textureLoad(src, vec2<i32>(x, y), 0);
    let i = y * w + x;
    if (!imp_near(i) || ((x + y) & 1) != i32(P.u[0].w)) {
        textureStore(out, vec2<i32>(x, y), cur);
        return;
    }
    var best = vec2<i32>(i32(cur.r), i32(cur.g));
    var bd = cur.b;
    if (cur.a <= 0.5) {
        bd = IMP_INF;
    }
    for (var k = 0; k < 4; k = k + 1) {
        var nx = x;
        var ny = y;
        var sx = 0;
        var sy = 0;
        if (k == 0) {
            nx = x - 1;
            sx = 1;
        } else if (k == 1) {
            nx = x + 1;
            sx = -1;
        } else if (k == 2) {
            ny = y - 1;
            sy = 1;
        } else {
            ny = y + 1;
            sy = -1;
        }
        if (nx < 0 || ny < 0 || nx >= w || ny >= h) {
            continue;
        }
        let n = textureLoad(src, vec2<i32>(nx, ny), 0);
        if (n.a <= 0.5) {
            continue;
        }
        let c = vec2<i32>(i32(n.r) + sx, i32(n.g) + sy);
        if (c.x == best.x && c.y == best.y) {
            continue;
        }
        if (!imp_valid_source(c.x, c.y, w, h, r)) {
            continue;
        }
        let d = imp_dist(x, y, c.x, c.y, w, h, r, bd);
        if (d < bd) {
            bd = d;
            best = c;
        }
    }
    textureStore(out, vec2<i32>(x, y), vec4<f32>(f32(best.x), f32(best.y), bd, 1.0));
}

// The random search (halving radius) and one random source anywhere, after propagation.
@compute @workgroup_size(16, 16)
fn imp_pm_rand(@builtin(global_invocation_id) gid: vec3<u32>) {
    let w = i32(P.u[0].x);
    let h = i32(P.u[0].y);
    let r = i32(P.u[0].z);
    let x = i32(P.u[1].x) + i32(gid.x);
    let y = i32(P.u[1].y) + i32(gid.y);
    if (x >= w || y >= h) {
        return;
    }
    let cur = textureLoad(src, vec2<i32>(x, y), 0);
    let i = y * w + x;
    if (!imp_near(i) || cur.a <= 0.5) {
        textureStore(out, vec2<i32>(x, y), cur);
        return;
    }
    var best = vec2<i32>(i32(cur.r), i32(cur.g));
    var bd = cur.b;
    let seed = P.u[2].x;
    let it = P.u[2].y;
    let em = P.u[2].w;
    let iu = u32(i);
    var rad = max(w, h);
    var k = 0u;
    while (rad >= 1) {
        let a = imp_rnd(seed + it * 977u + em * 31u, iu, k);
        let ox = i32(a % u32(2 * rad + 1)) - rad;
        let oy = i32((a >> 16u) % u32(2 * rad + 1)) - rad;
        let c = vec2<i32>(best.x + ox, best.y + oy);
        if (imp_valid_source(c.x, c.y, w, h, r)) {
            let d = imp_dist(x, y, c.x, c.y, w, h, r, bd);
            if (d < bd) {
                bd = d;
                best = c;
            }
        }
        rad = rad / 2;
        k = k + 1u;
    }
    let any = imp_any_source(seed ^ 0xABCDu, iu, it + em * 64u, w, h, r);
    if (any.x >= 0 && (any.x != best.x || any.y != best.y)) {
        let d = imp_dist(x, y, any.x, any.y, w, h, r, bd);
        if (d < bd) {
            bd = d;
            best = any;
        }
    }
    textureStore(out, vec2<i32>(x, y), vec4<f32>(f32(best.x), f32(best.y), bd, 1.0));
}

// Carry a round's field back into the nearest-neighbour field (only targets, as on the CPU).
@compute @workgroup_size(16, 16)
fn imp_nnf_store(@builtin(global_invocation_id) gid: vec3<u32>) {
    let w = i32(P.u[0].x);
    let h = i32(P.u[0].y);
    let x = i32(gid.x);
    let y = i32(gid.y);
    if (x >= w || y >= h) {
        return;
    }
    let i = y * w + x;
    var o = textureLoad(aux, vec2<i32>(x, y), 0);
    if (imp_near(i)) {
        let f = textureLoad(src, vec2<i32>(x, y), 0);
        o = vec4<f32>(f.r, f.g, 0.0, 1.0);
    }
    textureStore(out, vec2<i32>(x, y), o);
}

// The vote: every hole pixel becomes the distance-weighted mean of the samples its overlapping
// patches carry (pm_level's vote step). `aux` is the current image; the image is copied into
// `out` first, so only hole pixels are written.
@compute @workgroup_size(16, 16)
fn imp_pm_vote(@builtin(global_invocation_id) gid: vec3<u32>) {
    let w = i32(P.u[0].x);
    let h = i32(P.u[0].y);
    let r = i32(P.u[0].z);
    let x = i32(P.u[1].x) + i32(gid.x);
    let y = i32(P.u[1].y) + i32(gid.y);
    if (x >= w || y >= h) {
        return;
    }
    let i = y * w + x;
    if (!imp_hole(i)) {
        return;
    }
    let sigma2 = P.f[0].x;
    var acc = vec4<f32>(0.0);
    var wsum = 0.0;
    for (var dy = -r; dy <= r; dy = dy + 1) {
        let py = y + dy;
        if (py < 0 || py >= h) {
            continue;
        }
        for (var dx = -r; dx <= r; dx = dx + 1) {
            let px = x + dx;
            if (px < 0 || px >= w) {
                continue;
            }
            let f = textureLoad(src, vec2<i32>(px, py), 0);
            if (f.a <= 0.5 || f.b >= IMP_INF) {
                continue;
            }
            let s = vec2<i32>(i32(f.r) - dx, i32(f.g) - dy);
            let wt = exp(-f.b / (2.0 * sigma2));
            acc = acc + textureLoad(aux, vec2<i32>(s.x, s.y), 0) * wt;
            wsum = wsum + wt;
        }
    }
    if (wsum > 0.0) {
        textureStore(out, vec2<i32>(x, y), acc / wsum);
    } else {
        textureStore(out, vec2<i32>(x, y), textureLoad(aux, vec2<i32>(x, y), 0));
    }
}

// One workgroup's histogram of the targets' match distances, for the vote's sigma² (the CPU takes
// the 75th percentile of the finite distances). 256 threads own 1024 bins, 4 each, so no atomics.
@compute @workgroup_size(256, 1, 1)
fn imp_hist(@builtin(global_invocation_id) gid: vec3<u32>) {
    let w = i32(P.u[0].x);
    let h = i32(P.u[0].y);
    let x0 = i32(P.u[1].x);
    let y0 = i32(P.u[1].y);
    let bw = i32(P.u[1].z);
    let bh = i32(P.u[1].w);
    let t = gid.x;
    var mine = array<u32, 4>(0u, 0u, 0u, 0u);
    let total = u32(max(bw, 0) * max(bh, 0));
    for (var idx = t; idx < total; idx = idx + 256u) {
        let x = x0 + i32(idx) % bw;
        let y = y0 + i32(idx) / bw;
        let i = y * w + x;
        if (!imp_near(i)) {
            continue;
        }
        let f = textureLoad(src, vec2<i32>(x, y), 0);
        if (f.a <= 0.5 || f.b >= IMP_INF) {
            continue;
        }
        let b = clamp(i32(f.b * 256.0), 0, 1023);
        if (b >= i32(t) * 4 && b < i32(t) * 4 + 4) {
            mine[b - i32(t) * 4] = mine[b - i32(t) * 4] + 1u;
        }
    }
    for (var j = 0; j < 4; j = j + 1) {
        textureStore(out, vec2<i32>(i32(t) * 4 + j, 0), vec4<f32>(f32(mine[j]), 0.0, 0.0, 0.0));
    }
}

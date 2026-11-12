
// ---------------------------------------------------------------- helpers
// A 1-D index over n items spread on a 2-D grid of 256-thread groups (a dimension holds 65535 groups).
fn gid1(wg: vec3<u32>, nw: vec3<u32>, li: vec3<u32>) -> u32 { return (wg.y * nw.x + wg.x) * 256u + li.x; }

var<workgroup> red: array<f32, 64>;

// Sum of `red` over the workgroup (64 threads), for a caller that has filled it; all threads get the result.
fn reduce_sum(li: u32) -> f32 {
    workgroupBarrier();
    for (var st = 32u; st > 0u; st = st / 2u) {
        if (li < st) { red[li] += red[li + st]; }
        workgroupBarrier();
    }
    let r = red[0];
    workgroupBarrier();
    return r;
}

fn reduce_max(li: u32) -> f32 {
    workgroupBarrier();
    for (var st = 32u; st > 0u; st = st / 2u) {
        if (li < st) { red[li] = max(red[li], red[li + st]); }
        workgroupBarrier();
    }
    let r = red[0];
    workgroupBarrier();
    return r;
}

// ---------------------------------------------------------------- layer norm over the last dimension
//  words: 0 rows, 1 cols, 2 eps (f32), 3 gelu (1: apply GELU to the result). in0 x, in1 weight, in2 bias, out0 y. One workgroup per row.
@compute @workgroup_size(64)
fn layernorm(@builtin(workgroup_id) wg: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>, @builtin(local_invocation_id) li: vec3<u32>) {
    let row = wg.y * nw.x + wg.x;
    let rows = u(0u); let cols = u(1u);
    if (row >= rows) { return; }
    let base = row * cols;
    var s = 0.0;
    for (var c = li.x; c < cols; c += 64u) { s += in0[base + c]; }
    red[li.x] = s;
    let mean = reduce_sum(li.x) / f32(cols);
    var q = 0.0;
    for (var c = li.x; c < cols; c += 64u) { let d = in0[base + c] - mean; q += d * d; }
    red[li.x] = q;
    let inv = inverseSqrt(reduce_sum(li.x) / f32(cols) + f(2u));
    for (var c = li.x; c < cols; c += 64u) {
        var y = (in0[base + c] - mean) * inv * in1[c] + in2[c];
        if (u(3u) == 1u) { y = gelu_(y); }
        out0[base + c] = y;
    }
}

// ---------------------------------------------------------------- convolution over channels-last [H, W, Ci]
//  words: 0 n (output elements), 1 H, 2 W, 3 Ci, 4 Co, 5 Ho, 6 Wo, 7 k, 8 stride, 9 pad, 10 groups.
//  in0 input, in1 weight [Co, Ci / groups, k, k], in2 bias [Co]; output [Ho, Wo, Co].
@compute @workgroup_size(256)
fn conv2d(@builtin(workgroup_id) wg: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>, @builtin(local_invocation_id) li: vec3<u32>) {
    let i = gid1(wg, nw, li);
    if (i >= u(0u)) { return; }
    let H = u(1u); let W = u(2u); let Ci = u(3u); let Co = u(4u); let Wo = u(6u);
    let k = u(7u); let stride = u(8u); let pad = i32(u(9u)); let groups = u(10u);
    let co = i % Co; let t = i / Co;
    let ox = t % Wo; let oy = t / Wo;
    let cig = Ci / groups;
    let g0 = co / (Co / groups);
    var acc = in2[co];
    for (var c = 0u; c < cig; c++) {
        let ci = g0 * cig + c;
        for (var ky = 0u; ky < k; ky++) {
            let iy = i32(oy * stride + ky) - pad;
            if (iy < 0 || iy >= i32(H)) { continue; }
            for (var kx = 0u; kx < k; kx++) {
                let ix = i32(ox * stride + kx) - pad;
                if (ix < 0 || ix >= i32(W)) { continue; }
                acc += in0[(u32(iy) * W + u32(ix)) * Ci + ci] * in1[((co * cig + c) * k + ky) * k + kx];
            }
        }
    }
    out0[i] = acc;
}

// ---------------------------------------------------------------- softmax over rows, in place on out0
//  words: 0 rows, 1 cols. One workgroup per row.
@compute @workgroup_size(64)
fn softmax(@builtin(workgroup_id) wg: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>, @builtin(local_invocation_id) li: vec3<u32>) {
    let row = wg.y * nw.x + wg.x;
    let rows = u(0u); let cols = u(1u);
    if (row >= rows) { return; }
    let base = row * cols;
    var mx = -3.0e38;
    for (var c = li.x; c < cols; c += 64u) { mx = max(mx, out0[base + c]); }
    red[li.x] = mx;
    let m = reduce_max(li.x);
    var s = 0.0;
    for (var c = li.x; c < cols; c += 64u) { let e = exp(out0[base + c] - m); out0[base + c] = e; s += e; }
    red[li.x] = s;
    let inv = 1.0 / reduce_sum(li.x);
    for (var c = li.x; c < cols; c += 64u) { out0[base + c] = out0[base + c] * inv; }
}

// ---------------------------------------------------------------- elementwise add: out0 = in0 + in1 (n items)
@compute @workgroup_size(256)
fn add(@builtin(workgroup_id) wg: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>, @builtin(local_invocation_id) li: vec3<u32>) {
    let i = gid1(wg, nw, li);
    if (i < u(0u)) { out0[i] = in0[i] + in1[i]; }
}

// ---------------------------------------------------------------- windows
//  words: 0 n (output elements), 1 H, 2 W, 3 C, 4 ws, 5 windows across (nwx)
//  window_partition: [H, W, C] -> [nwin, ws, ws, C] (zero padded); window_unpartition: the inverse.
@compute @workgroup_size(256)
fn window_partition(@builtin(workgroup_id) wg: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>, @builtin(local_invocation_id) li: vec3<u32>) {
    let i = gid1(wg, nw, li);
    if (i >= u(0u)) { return; }
    let H = u(1u); let W = u(2u); let C = u(3u); let ws = u(4u); let nwx = u(5u);
    let c = i % C; var t = i / C;
    let x = t % ws; t = t / ws;
    let y = t % ws; let win = t / ws;
    let gy = (win / nwx) * ws + y;
    let gx = (win % nwx) * ws + x;
    var v = 0.0;
    if (gy < H && gx < W) { v = in0[(gy * W + gx) * C + c]; }
    out0[i] = v;
}

@compute @workgroup_size(256)
fn window_unpartition(@builtin(workgroup_id) wg: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>, @builtin(local_invocation_id) li: vec3<u32>) {
    let i = gid1(wg, nw, li);
    if (i >= u(0u)) { return; }
    let W = u(2u); let C = u(3u); let ws = u(4u); let nwx = u(5u);
    let c = i % C; let t = i / C;
    let gx = t % W; let gy = t / W;
    let win = (gy / ws) * nwx + gx / ws;
    out0[i] = in0[((win * ws + gy % ws) * ws + gx % ws) * C + c];
}

// ---------------------------------------------------------------- 2 x 2 max pool over [nb, h, w, c] tokens
//  words: 0 n (output elements), 1 nb, 2 h, 3 w, 4 c, 5 src image stride, 6 src row stride, 7 src token stride
//  (so it can read the q third of a qkv buffer in place). Output [nb, h/2, w/2, c] contiguous.
@compute @workgroup_size(256)
fn maxpool2(@builtin(workgroup_id) wg: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>, @builtin(local_invocation_id) li: vec3<u32>) {
    let i = gid1(wg, nw, li);
    if (i >= u(0u)) { return; }
    let h = u(2u); let w = u(3u); let c = u(4u);
    let oh = h / 2u; let ow = w / 2u;
    let ch = i % c; var t = i / c;
    let ox = t % ow; t = t / ow;
    let oy = t % oh; let img = t / oh;
    let base = img * u(5u) + (oy * 2u) * u(6u) + (ox * 2u) * u(7u) + ch;
    out0[i] = max(max(in0[base], in0[base + u(7u)]), max(in0[base + u(6u)], in0[base + u(6u) + u(7u)]));
}

// ---------------------------------------------------------------- patch embedding: 7 x 7 stride 4 pad 3 convolution
//  words: 0 n (output elements), 1 H, 2 W, 3 Co, 4 Ho, 5 Wo. in0 pixels [3, H, W], in1 weight [Co, 3, 7, 7],
//  in2 bias [Co]. Output [Ho, Wo, Co].
@compute @workgroup_size(256)
fn patch_embed(@builtin(workgroup_id) wg: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>, @builtin(local_invocation_id) li: vec3<u32>) {
    let i = gid1(wg, nw, li);
    if (i >= u(0u)) { return; }
    let H = u(1u); let W = u(2u); let Co = u(3u); let Wo = u(5u);
    let co = i % Co; let t = i / Co;
    let ox = t % Wo; let oy = t / Wo;
    var acc = in2[co];
    for (var ci = 0u; ci < 3u; ci++) {
        for (var ky = 0u; ky < 7u; ky++) {
            let iy = i32(oy * 4u + ky) - 3;
            if (iy < 0 || iy >= i32(H)) { continue; }
            for (var kx = 0u; kx < 7u; kx++) {
                let ix = i32(ox * 4u + kx) - 3;
                if (ix < 0 || ix >= i32(W)) { continue; }
                acc += in0[ci * H * W + u32(iy) * W + u32(ix)] * in1[((co * 3u + ci) * 7u + ky) * 7u + kx];
            }
        }
    }
    out0[i] = acc;
}

// ---------------------------------------------------------------- rotary position encoding on [rows, cols]
//  words: 0 n (= rows * cols), 1 cols, 2 table rows, 3 rows rotated (the rest is copied). in0 x, in1 cos, in2 sin
//  (tables [table rows, cols]); pairs (x0, x1) -> (x0 c - x1 s, x1 c + x0 s).
@compute @workgroup_size(256)
fn rope(@builtin(workgroup_id) wg: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>, @builtin(local_invocation_id) li: vec3<u32>) {
    let i = gid1(wg, nw, li);
    if (i >= u(0u)) { return; }
    let cols = u(1u);
    let row = i / cols; let col = i % cols;
    if (row >= u(3u)) { out0[i] = in0[i]; return; }
    let t = (row % u(2u)) * cols + col;
    let x = in0[i];
    var pair = 0.0;
    if ((col & 1u) == 1u) { pair = in0[i - 1u]; } else { pair = in0[i + 1u]; }
    if ((col & 1u) == 0u) { out0[i] = x * in1[t] - pair * in2[t]; } else { out0[i] = x * in1[t] + pair * in2[t]; }
}

// ---------------------------------------------------------------- neck: lateral + nearest 2x upsample of prev
//  words: 0 n (output elements), 1 h, 2 w, 3 c. in0 lateral [h, w, c], in1 prev [h/2, w/2, c].
@compute @workgroup_size(256)
fn upsample_add(@builtin(workgroup_id) wg: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>, @builtin(local_invocation_id) li: vec3<u32>) {
    let i = gid1(wg, nw, li);
    if (i >= u(0u)) { return; }
    let w = u(2u); let c = u(3u);
    let ch = i % c; let t = i / c;
    let x = t % w; let y = t / w;
    out0[i] = in0[i] + in1[(((y / 2u) * (w / 2u)) + x / 2u) * c + ch];
}

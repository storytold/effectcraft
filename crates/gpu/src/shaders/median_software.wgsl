// The same median for large radii, as the CPU computes it: a sliding 512-bin histogram per
// channel (Huang). One invocation per row segment of u[0].y pixels: it fills the histogram for
// the segment's first window, then slides right one column at a time (remove the column
// leaving, add the one entering) and moves each channel's median by the counts below it.
// Each invocation owns one 8 KiB slice of the workgroup's zero-initialized histogram.
// Two slices fit the guaranteed 16 KiB workgroup storage limit. No bins are shared.
var<workgroup> median_hist: array<array<u32, 2048>, 2>;
// u[0] = (r, segment length); dispatched over (rows, segments) with 2 × 1 workgroups.
// data = the 512 CPU-rounded level / 511 values.
@compute @workgroup_size(2, 1)
fn fxn_median_huang(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(local_invocation_index) lane: u32) {
    let dims = out_dims();
    let y = i32(gid.x);
    let seg = i32(P.u[0].y);
    let x0 = i32(gid.y) * seg;
    if (y >= dims.y || x0 >= dims.x) {
        return;
    }
    let x1 = min(x0 + seg, dims.x);
    let r = i32(P.u[0].x);
    let half = u32((2 * r + 1) * (2 * r + 1) / 2);
    for (var dy = -r; dy <= r; dy++) {
        for (var dx = -r; dx <= r; dx++) {
            let v = vec4<u32>(tex_get_clamped(src, x0 + dx, y + dy));
            median_hist[lane][v.x] += 1u;
            median_hist[lane][512u + v.y] += 1u;
            median_hist[lane][1024u + v.z] += 1u;
            median_hist[lane][1536u + v.w] += 1u;
        }
    }
    // Per channel: the median level and the count of values below it.
    var med = vec4<u32>(0u);
    var lt = vec4<u32>(0u);
    for (var x = x0; x < x1; x++) {
        if (x > x0) {
            for (var dy = -r; dy <= r; dy++) {
                let o = vec4<u32>(tex_get_clamped(src, x - 1 - r, y + dy));
                let n = vec4<u32>(tex_get_clamped(src, x + r, y + dy));
                for (var c = 0u; c < 4u; c++) {
                    median_hist[lane][c * 512u + o[c]] -= 1u;
                    if (o[c] < med[c]) {
                        lt[c] -= 1u;
                    }
                    median_hist[lane][c * 512u + n[c]] += 1u;
                    if (n[c] < med[c]) {
                        lt[c] += 1u;
                    }
                }
            }
        }
        for (var c = 0u; c < 4u; c++) {
            let base = c * 512u;
            while (lt[c] > half) {
                med[c] -= 1u;
                lt[c] -= median_hist[lane][base + med[c]];
            }
            while (lt[c] + median_hist[lane][base + med[c]] <= half) {
                lt[c] += median_hist[lane][base + med[c]];
                med[c] += 1u;
            }
        }
        textureStore(out, vec2<i32>(x, y), vec4<f32>(data[med.x], data[med.y], data[med.z], data[med.w]));
    }
}


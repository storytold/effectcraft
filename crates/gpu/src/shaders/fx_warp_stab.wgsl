// fxs_warpstab — GPU inverse-map warp stabilizer
//
// f[0] = (crop_x0, crop_y0, crop_x1, crop_y1)  // −1 sentinel = no crop
// f[1] = H row0 [h00, h01, h02, size_w]
// f[2] = H row1 [h10, h11, h12, size_h]
// f[3] = H row2 [h20, h21, h22, scale]
// f[4] = (mesh_cols, mesh_rows, _, has_mesh: 0.0 or 1.0)
//
// For the homography path, H = warp.inverse() (framing already folded into pl.warps[k]).
// For the mesh path, H = pl.framing.inverse() (finv): output → stabilised-frame-local, then
//   the mesh is sampled (stabilised → source) from the uploaded vertex buffer.
//
// data buffer: mesh vertex (x, y) f32 pairs if has_mesh == 1.0
//   Buffer layout: (cols+1)*(rows+1) vertices, row-major (matches Mesh::verts).
//   Vertex (ci, ri) is at data[(ri*(cols+1)+ci)*2], data[(ri*(cols+1)+ci)*2+1].

// Apply a row-major 3×3 homography H to point p (homogeneous coords).
fn h_apply(h: mat3x3<f32>, p: vec2<f32>) -> vec2<f32> {
    let r = h * vec3<f32>(p.x, p.y, 1.0);
    return r.xy / r.z;
}

@compute @workgroup_size(16, 16)
fn fxs_warpstab(@builtin(global_invocation_id) gid: vec3<u32>) {
    let x = gid.x;
    let y = gid.y;
    let sw = u32(textureDimensions(src).x);
    let sh = u32(textureDimensions(src).y);
    if (x >= sw || y >= sh) {
        return;
    }

    let scale = P.f[3].w;        // b.scale
    let size_w = P.f[1].w;
    let size_h = P.f[2].w;

    // Output pixel → normalised frame coords (pixel centres at +0.5)
    let px = (f32(x) + 0.5) / scale;
    let py = (f32(y) + 0.5) / scale;

    // ── Crop early-out ────────────────────────────────────────────────────────
    let crop = P.f[0];
    if (crop.x >= 0.0) {
        if (px < crop.x || px > crop.z || py < crop.y || py > crop.w) {
            textureStore(out, vec2<i32>(i32(x), i32(y)), vec4<f32>(0.0, 0.0, 0.0, 0.0));
            return;
        }
    }

    // ── Build the 3×3 homography ───────────────────────────────────────────────
    let H = mat3x3<f32>(
        vec3<f32>(P.f[1].x, P.f[2].x, P.f[3].x),
        vec3<f32>(P.f[1].y, P.f[2].y, P.f[3].y),
        vec3<f32>(P.f[1].z, P.f[2].z, P.f[3].z),
    );

    let has_mesh = P.f[4].w;

    var sx: f32;
    var sy: f32;

    if (has_mesh > 0.5) {
        // ── Subspace Warp ────────────────────────────────────────────────────
        // pf = finv.apply(p) → stabilised-frame-local coords
        let pf = h_apply(H, vec2<f32>(px, py));

        let cols = i32(P.f[4].x);
        let rows = i32(P.f[4].y);
        let stride = u32(cols + 1);   // (cols+1) vertices per row

        let cell_w = size_w / f32(cols);
        let cell_h = size_h / f32(rows);

        // Locate the cell: clamp to [0, cols-1] × [0, rows-1] (matches Mesh::locate)
        let ci = clamp(i32(pf.x / cell_w), 0, max(cols - 1, 0));
        let ri = clamp(i32(pf.y / cell_h), 0, max(rows - 1, 0));

        // Vertex layout matches Mesh::verts: row-major, idx = ri*(cols+1) + ci
        let base00 = (u32(ri) * stride + u32(ci)) * 2u;
        let base10 = base00 + 2u;                          // (ci+1, ri)
        let base01 = base00 + stride * 2u;                // (ci, ri+1)
        let base11 = base01 + 2u;                         // (ci+1, ri+1)

        let v00 = vec2<f32>(data[base00],     data[base00 + 1u]);
        let v10 = vec2<f32>(data[base10],     data[base10 + 1u]);
        let v01 = vec2<f32>(data[base01],     data[base01 + 1u]);
        let v11 = vec2<f32>(data[base11],     data[base11 + 1u]);

        // Bilinear weights (identical to Mesh::sample)
        let u = (pf.x - f32(ci) * cell_w) / cell_w;
        let v = (pf.y - f32(ri) * cell_h) / cell_h;

        let top = v00 * (1.0 - u) + v10 * u;
        let bot = v01 * (1.0 - u) + v11 * u;
        let q = top * (1.0 - v) + bot * v;

        sx = q.x;
        sy = q.y;
    } else {
        // ── Homography path ──────────────────────────────────────────────────
        // H = warp.inverse(): output → source directly (framing already folded in)
        let q = h_apply(H, vec2<f32>(px, py));
        sx = q.x;
        sy = q.y;
    }

    // ── Source-bounds transparency check (mirrors CPU `sample()` guard) ────────
    if (sx < 0.0 || sy < 0.0 || sx > size_w || sy > size_h) {
        textureStore(out, vec2<i32>(i32(x), i32(y)), vec4<f32>(0.0, 0.0, 0.0, 0.0));
        return;
    }

    // ── Clamped bilinear sample (edge pixels repeat) ───────────────────────────
    // CPU: b.img.sample_bilinear_clamped(q[0] * b.scale + b.offset[0], q[1] * b.scale + b.offset[1])
    // b.offset is [0,0] for the warp output.
    let col = sample_bilinear_clamped(src, sx * scale, sy * scale);
    textureStore(out, vec2<i32>(i32(x), i32(y)), col);
}

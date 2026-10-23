//! GPU path for ec.distort.warpstabilizer.
//!
//! CPU-precomputes the fused correction transform per frame (the inverse warp with framing
//! already folded in) and uploads it as a 3 x 3 f32 homography matrix. The kernel performs
//! per-pixel inverse mapping with bilinear clamped sampling - identical to the CPU path.
//!
//! Synthesize Edges and Show Track Points fall back to the CPU (gated by CPU_ONLY_CONTROLS).

use crate::context::{Enc, Params};
use crate::effects::GBuf;
use effectcraft_effects::EffectCtx;
use effectcraft_track::Homography;

pub(crate) const KERNELS: &[&str] = &["fxs_warpstab"];
pub(crate) const IDS: &[&str] = &["ec.distort.warpstabilizer"];

/// Run ec.distort.warpstabilizer on the GPU; `None` = fall back to CPU.
pub(crate) fn apply(e: &mut Enc, id: &str, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    debug_assert!(IDS.contains(&id));
    let mut p = Params::default();

    // -- 1. Resolve the plan ---------------------------------------------------
    let Some((k, analysis, pl)) = effectcraft_effects::warp_stab::plan_at(ctx.params, ctx.time) else {
        // No analysis (or invalid analysis): the CPU path passes the buffer through unchanged.
        return Some(b);
    };

    // Match the CPU passthrough: analysis size must agree with layer size, and adjustment
    // layers pass the input through unchanged.
    let size = [analysis.size[0], analysis.size[1]];
    if (size[0] - ctx.layer_size[0]).abs() > 0.5 || (size[1] - ctx.layer_size[1]).abs() > 0.5 || ctx.adjustment {
        return Some(b);
    }

    // -- 2. Early-out crop: if pixel is outside the crop rect, make it transparent.
    //       We pass the crop rect (or -1 sentinel if no crop) via f[0].
    if let Some(c) = pl.crop {
        p.f[0] = [c[0] as f32, c[1] as f32, c[2] as f32, c[3] as f32];
    } else {
        p.f[0] = [-1.0, -1.0, -1.0, -1.0];
    }

    // -- 3. Build the per-frame transform (CPU, f64, exact) --------------------
    let sc = if b.scale > 0.0 { b.scale } else { 1.0 };

    let (h0, h1, h2, h_meta, data): ([f32; 4], [f32; 4], [f32; 4], [f32; 4], Vec<f32>) = if let Some(mesh) = pl.meshes.get(k) {
        // -- Subspace Warp: upload mesh vertices + framing inverse -------------
        // finv = pl.framing.inverse(): output -> stabilised-frame-local.
        // The kernel applies finv to p, then samples the mesh (stabilised -> source).
        let finv = pl.framing.inverse().unwrap_or(Homography::IDENTITY);
        let fh = finv.0;
        // Upload mesh SOURCE positions (Mesh::verts: deformed source coords, row-major).
        let mut buf = Vec::with_capacity(mesh.verts.len() * 2);
        for v in &mesh.verts {
            buf.push(v[0] as f32);
            buf.push(v[1] as f32);
        }
        let (cw, rh) = (mesh.cols as f32, mesh.rows as f32);
        let meta = [cw, rh, 0.0, 1.0]; // has_mesh = 1.0
        (
            [fh[0][0] as f32, fh[0][1] as f32, fh[0][2] as f32, size[0] as f32],
            [fh[1][0] as f32, fh[1][1] as f32, fh[1][2] as f32, size[1] as f32],
            [fh[2][0] as f32, fh[2][1] as f32, fh[2][2] as f32, sc as f32],
            meta,
            buf,
        )
    } else {
        // -- Homography path (Position / Similarity / Perspective) -----------
        // pl.warps[k] already has framing folded in (confirmed: plan() does
        // framing.then_after(&w.homography()) for all frames).
        let warp = pl.warps.get(k)?;
        let warp_inv = warp.inverse()?;
        let h = warp_inv.0;
        let meta = [0.0, 0.0, 0.0, 0.0]; // has_mesh = 0.0
        (
            [h[0][0] as f32, h[0][1] as f32, h[0][2] as f32, size[0] as f32],
            [h[1][0] as f32, h[1][1] as f32, h[1][2] as f32, size[1] as f32],
            [h[2][0] as f32, h[2][1] as f32, h[2][2] as f32, sc as f32],
            meta,
            vec![],
        )
    };

    p.f[1] = h0;
    p.f[2] = h1;
    p.f[3] = h2;
    p.f[4] = h_meta;

    // -- 4. Upload mesh data buffer (empty vec for homography path) --------------
    let data_buf = if data.is_empty() { None } else { Some(e.data(&data)) };

    // -- 5. Output size ---------------------------------------------------------
    let ow = ((size[0] * sc).ceil().max(1.0) as u32).max(1);
    let oh = ((size[1] * sc).ceil().max(1.0) as u32).max(1);
    let out = e.scratch(ow, oh);

    // -- 6. Run kernel ----------------------------------------------------------
    if let Some(buf) = &data_buf {
        e.pixels("fxs_warpstab", &p, &b.img, None, &out, Some(buf));
    } else {
        e.pixels("fxs_warpstab", &p, &b.img, None, &out, None);
    }

    // Offset: the stabilisation output has no additional offset (buf.offset already accounts
    // for any upstream transform; the warp produces a centred, stabilised result).
    Some(GBuf { img: out, offset: [0.0, 0.0], scale: sc })
}

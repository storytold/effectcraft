//! Classic 3D depth of field on the GPU (`bokeh_*` kernels): the plane buffer padded, its
//! highlights boosted, row prefix sums, then one span gather per blur level
//! ([`effectcraft_render::three_d::bokeh::kernel_spans`]) accumulated with each pixel's level
//! weight — the CPU's `progressive_blur` step for step.

use effectcraft_render::three_d::PlaneDof;
use effectcraft_render::three_d::bokeh::kernel_spans;

use crate::context::{Enc, GpuImage, Params};

/// Blur `src` (an unpadded plane buffer) as `d` says. Returns the padded, blurred image.
pub(crate) fn apply(e: &mut Enc, src: &GpuImage, d: &PlaneDof) -> Option<GpuImage> {
    // Saturating, so an oversized padding is refused by `fits` below instead of wrapping around.
    let (w, h) = (src.width.saturating_add(d.pad.saturating_mul(2)), src.height.saturating_add(d.pad.saturating_mul(2)));
    if !e.g.fits(w.saturating_add(1), h) || d.radius.len() != w as usize * h as usize || d.levels.is_empty() {
        return None;
    }
    let padded = e.image(w, h);
    e.copy_into(src, &padded, d.pad, d.pad);
    let boosted = if d.highlight.gain > 0.0 {
        let out = e.scratch(w, h);
        let mut p = Params::default();
        p.f[0] = [d.highlight.gain as f32, d.highlight.threshold as f32, d.highlight.saturation as f32, 0.0];
        e.pixels("bokeh_boost", &p, &padded, None, &out, None);
        out
    } else {
        padded
    };
    let prefix = e.image(w + 1, h);
    e.dispatch("bokeh_prefix", &Params::default(), crate::context::KernelImages { src: &boosted, aux: None, out: &prefix }, None, (h.div_ceil(64), 1));
    // Radius map, then every level's spans.
    let mut data: Vec<f32> = d.radius.as_ref().clone();
    let mut levels = vec![];
    for &r in &d.levels {
        let (kernels, norm) = kernel_spans(&d.iris, r as f64).unwrap_or_else(|| (vec![(1.0, vec![(0, -0.5, 0.5)])], 1.0));
        let off = data.len() as u32;
        let mut count = 0u32;
        for (wt, spans) in &kernels {
            for &(dy, x0, x1) in spans {
                data.extend_from_slice(&[dy as f32, x0 as f32, x1 as f32, *wt]);
                count += 1;
            }
        }
        levels.push((off, count, norm));
    }
    let buf = e.data(&data);
    let n = d.levels.len() as u32;
    let (lo, hi) = (d.levels[0], d.levels[d.levels.len() - 1]);
    let mut acc = e.image(w, h);
    for (k, (off, count, norm)) in levels.into_iter().enumerate() {
        let out = e.scratch(w, h);
        let mut p = Params::default();
        p.u[0] = [off, count, k as u32, n];
        p.u[1] = [u32::from(k == 0), 0, 0, 0];
        p.f[0] = [1.0 / norm, lo, hi, 0.0];
        e.pixels("bokeh_gather", &p, &prefix, Some(&acc), &out, Some(&buf));
        acc = out;
    }
    Some(acc)
}

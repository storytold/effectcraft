//! GPU effects, colour management (kernels in `shaders/fx_lut.wgsl`, every entry point
//! prefixed `fxl_`): Apply Color LUT, OCIO CDL / Color Space / Display / File / Look Transform
//! and Color Profile Converter; Lumetri Color's Input LUT and Look also use the LUT helpers.
//!
//! Each effect compiles to a colour program ([`effectcraft_effects::color_program`]: transfer
//! functions, matrices, ASC CDLs, tone mapping, gamut compression, LUTs) built next to the CPU
//! effect, which `fxl_point` interprets per pixel. LUTs (1D tables and 3D lattices, with a
//! cineSpace shaper) travel in the kernel's storage buffer and are interpolated in the kernel
//! exactly as the CPU does (nearest, trilinear or tetrahedral; inverse 1D tables and the
//! iterative 3D inverse): hardware 3D-texture filtering would round the interpolation weights
//! and cannot do tetrahedral interpolation. The CPU evaluates transfer functions, CDLs and
//! Color Profile Converter's matrix in f64; the kernel in f32 (within the 1e-3 tolerance).
//! Custom `.ocio` configurations compile their conversions (matrix and offset, exponent,
//! exponent with linear segment, log / log-affine, allocation, range with its clamp, CDL, file
//! LUTs and matrices, built-in curves, with colour space / look / display-view transforms
//! resolved) to the same program; one longer than the kernel's 255 ops renders on the CPU.

use effectcraft_effects::{ColorOp, EffectCtx, Lut, Straight, Tf};

use crate::context::{Enc, Params};
use crate::effects::GBuf;

/// Compute entry points in `fx_lut.wgsl`.
pub(crate) const KERNELS: &[&str] = &["fxl_point"];

/// Effect ids implemented here.
pub(crate) const IDS: &[&str] = &[
    "ec.utility.applylut",
    "ec.color.ociocdl",
    "ec.color.ociocolorspace",
    "ec.color.ociodisplay",
    "ec.color.ociofile",
    "ec.color.ociolook",
    "ec.utility.colorprofileconverter",
];

/// Largest LUT the kernels take (floats in the storage buffer); bigger ones render on the CPU.
const MAX_FLOATS: usize = 1 << 24;

/// Most halvings the kernel's partition searches run (`fxl_search_steps` caps the count there).
const SHADER_MAX_STEPS: u32 = 32;

/// Halvings a partition search over `n` entries runs: `n`'s bit length, which every search
/// finishes within, plus one to spare.
const fn search_steps(n: usize) -> u32 {
    usize::BITS - n.leading_zeros() + 1
}

// A program's tables fit in MAX_FLOATS floats, so none needs more halvings than the kernel runs.
const _: () = assert!(search_steps(MAX_FLOATS) <= SHADER_MAX_STEPS);

/// Run effect `id` (one of [`IDS`]); `None` = this parameter combination runs on the CPU.
pub(crate) fn apply(e: &mut Enc, id: &str, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let (ops, straight) = effectcraft_effects::color_program(id, ctx)?;
    if ops.is_empty() {
        return Some(b);
    }
    let (data, steps) = encode(&ops)?;
    let buf = e.data(&data);
    let mut p = Params::default();
    p.u[0][0] = (straight == Straight::Divide) as u32;
    p.u[0][1] = steps;
    let out = e.scratch(b.img.width, b.img.height);
    e.pixels("fxl_point", &p, &b.img, None, &out, Some(&buf));
    Some(GBuf { img: out, ..b })
}

fn tf_code(t: Tf) -> [f32; 2] {
    match t {
        Tf::Linear => [0.0, 0.0],
        Tf::Srgb => [1.0, 0.0],
        Tf::Gamma(g) => [2.0, g as f32],
        Tf::AcesCct => [3.0, 0.0],
        Tf::Pq => [4.0, 0.0],
        Tf::AcesCc => [5.0, 0.0],
    }
}

/// Most ops `fxl_run` interprets (its loop guard); longer programs render on the CPU.
const MAX_OPS: usize = 255;

/// A LUT in the layout `fxl_lut_*` read at the returned offset: n1, n3, domain min (3), domain
/// max (3), the 1D table (n1 × RGB), then the lattice (n3³ × RGB, red fastest).
pub(crate) fn push_lut(d: &mut Vec<f32>, lut: &Lut) -> f32 {
    let at = d.len() as f32;
    d.extend([lut.n1 as f32, lut.n3 as f32]);
    d.extend(lut.dmin);
    d.extend(lut.dmax);
    for v in lut.d1.iter().chain(&lut.d3) {
        d.extend(v);
    }
    at
}

/// Program layout: a stream of (opcode, arguments…) ending in 0, followed by LUT data; and the
/// halvings the kernel's partition searches need for this program's tables.
fn encode(ops: &[ColorOp]) -> Option<(Vec<f32>, u32)> {
    if ops.len() > MAX_OPS {
        return None;
    }
    let mut steps = 0;
    let mut prog: Vec<f32> = vec![];
    let mut tables: Vec<Vec<f32>> = vec![];
    // LUT offsets are patched once the program's length is known.
    let mut patches: Vec<(usize, usize)> = vec![];
    for op in ops {
        match op {
            ColorOp::Decode(Tf::Linear) | ColorOp::Encode(Tf::Linear) => {}
            ColorOp::Decode(t) => {
                prog.push(1.0);
                prog.extend(tf_code(*t));
            }
            ColorOp::Encode(t) => {
                prog.push(2.0);
                prog.extend(tf_code(*t));
            }
            ColorOp::Matrix(m) => {
                prog.push(3.0);
                for r in m {
                    prog.extend(r);
                }
            }
            ColorOp::Cdl { cdl, clamp, inverse } => {
                prog.push(4.0);
                for v in cdl.slope.iter().chain(&cdl.offset).chain(&cdl.power) {
                    prog.push(*v as f32);
                }
                prog.extend([cdl.sat as f32, *clamp as u32 as f32, *inverse as u32 as f32]);
            }
            ColorOp::Tonemap { inverse } => prog.extend([5.0, *inverse as u32 as f32]),
            ColorOp::Gamut(l) => prog.extend([6.0, l[0] as f32, l[1] as f32, l[2] as f32]),
            ColorOp::Max0 => prog.push(7.0),
            ColorOp::Affine { m, pre, post } => {
                prog.push(9.0);
                for r in m {
                    prog.extend(r);
                }
                prog.extend(pre);
                prog.extend(post);
            }
            ColorOp::Pow(e) => {
                prog.push(10.0);
                prog.extend(e);
            }
            ColorOp::LogAffine { base, log_slope, log_offset, lin_slope, lin_offset, inverse } => {
                prog.extend([11.0, *base]);
                prog.extend(log_slope.iter().chain(log_offset).chain(lin_slope).chain(lin_offset));
                prog.extend([*inverse as u32 as f32, f64::MIN_POSITIVE.ln() as f32]);
            }
            ColorOp::MonCurve { gamma, offset, inverse, mirror } => {
                prog.push(12.0);
                prog.extend(gamma.iter().chain(offset));
                prog.extend([*inverse as u32 as f32, *mirror as u32 as f32]);
            }
            ColorOp::Clamp { lo, hi } => {
                prog.push(13.0);
                prog.extend(lo.iter().chain(hi));
            }
            ColorOp::Lut { lut, shaper, interp, inverse } => {
                // The kernel searches an inverted 1D table and every shaper channel.
                if *inverse && lut.n3 == 0 {
                    steps = steps.max(search_steps(lut.n1));
                }
                for channel in shaper.iter().flat_map(|s| s.iter()) {
                    steps = steps.max(search_steps(channel.len()));
                }
                prog.extend([8.0, 0.0, *interp as f32, *inverse as u32 as f32, -1.0]);
                let mut t = vec![];
                push_lut(&mut t, lut);
                let lut_at = prog.len() - 4;
                patches.push((lut_at, tables.len()));
                tables.push(t);
                if let Some(s) = shaper {
                    // Shaper: three point counts, then each channel's (x, y) points.
                    let mut t: Vec<f32> = s.iter().map(|c| c.len() as f32).collect();
                    for c in s.iter() {
                        for (x, y) in c {
                            t.extend([*x, *y]);
                        }
                    }
                    patches.push((prog.len() - 1, tables.len()));
                    tables.push(t);
                }
            }
        }
    }
    prog.push(0.0);
    let mut starts = vec![];
    let mut at = prog.len();
    for t in &tables {
        starts.push(at);
        at += t.len();
    }
    if at > MAX_FLOATS {
        return None;
    }
    // Offsets travel as f32: exact below 2^24.
    for (slot, table) in patches {
        prog[slot] = starts[table] as f32;
    }
    for t in tables {
        prog.extend(t);
    }
    Some((prog, steps))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The kernel's partition search (fxl_shape, fxl_inv_1d) run for `steps` halvings over the
    /// sorted table 0, 1, …, n - 1: the index of the first entry not below `x`.
    fn search(n: usize, steps: u32, x: f64) -> usize {
        let (mut lo, mut hi) = (0, n);
        for _ in 0..steps {
            if lo < hi {
                let mid = lo + (hi - lo) / 2;
                if (mid as f64) < x {
                    lo = mid + 1;
                } else {
                    hi = mid;
                }
            }
        }
        lo
    }

    #[test]
    fn search_steps_finish_every_partition_search() {
        for n in [1, 2, 3, 4, 5, 255, 256, 257, 1 << 16, (1 << 16) + 1, MAX_FLOATS / 3, MAX_FLOATS] {
            let steps = search_steps(n);
            assert!(steps <= SHADER_MAX_STEPS, "{n} entries need more halvings than the kernel runs");
            for x in [-1.0, 0.0, 0.5, n as f64 / 3.0 + 0.5, n as f64 - 1.5, n as f64 - 0.5, n as f64 + 1.0] {
                let want = (x.ceil().max(0.0) as usize).min(n);
                assert_eq!(search(n, steps, x), want, "{n} entries, target {x}");
            }
        }
    }
}

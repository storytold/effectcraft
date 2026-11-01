//! GPU Content-Aware Fill vs the CPU reference: the CPU's fill is the oracle.
//!
//! Parity here is a quality contract, not bit-exactness (see `inpaint.rs`): PatchMatch's
//! scan-order propagation becomes a two-pass checkerboard. So the tests compare both
//! syntheses against the ground truth they are reconstructing, and check the GPU is not the
//! worse of the two.

use effectcraft_raster::Image;
use effectcraft_raster::inpaint::{FillBackend, FillInput, FillOpts, fill_sequence_with, membrane_fill, patch_fill, psnr};

use crate::tests::gpu;

/// A textured background (periodic stripes + blocky noise), shifted by `dx` (the CPU tests'
/// scene, so the numbers here are comparable with theirs).
fn background(w: u32, h: u32, dx: f32) -> Image {
    let mut img = Image::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let fx = x as f32 + dx;
            let s = 0.5 + 0.3 * (fx * std::f32::consts::TAU / 10.0).sin();
            let n = effectcraft_raster::hash_noise(((fx.floor() as i64).rem_euclid(20) / 4) as u32, (y % 20) / 4, 3);
            img.set(x, y, [s, 0.3 + 0.4 * n, 0.6 - 0.3 * s * n, 1.0]);
        }
    }
    img
}

fn cut(img: &Image, hole: &[bool]) -> Image {
    let mut o = img.clone();
    for (p, h) in o.data.iter_mut().zip(hole) {
        if *h {
            *p = [0.0; 4];
        }
    }
    o
}

fn rect_hole(w: u32, h: u32, x0: u32, y0: u32, s: u32) -> Vec<bool> {
    (0..w * h).map(|i| (x0..x0 + s).contains(&(i % w)) && (y0..y0 + s).contains(&(i / w))).collect()
}

/// The kernels must exist: a shader that fails to build falls back to the CPU silently, which
/// would make every quality test below pass while testing nothing.
#[test]
fn the_inpaint_kernels_are_available() {
    let Some(g) = gpu() else { return };
    let bad: Vec<&str> = g.ctx.unavailable_kernels().iter().filter(|(e, _)| crate::inpaint::KERNELS.contains(e)).map(|(e, _)| *e).collect();
    assert!(bad.is_empty(), "inpaint kernels did not build on {}: {bad:?}", g.ctx.name);
}

/// The device path (not the CPU fallback) is what the quality tests measure.
#[test]
fn the_gpu_runs_the_fill_itself() {
    let Some(g) = gpu() else { return };
    let (w, h) = (96u32, 72u32);
    let gt = background(w, h, 0.0);
    let hole = rect_hole(w, h, 40, 28, 16);
    let src = cut(&gt, &hole);
    assert!(crate::inpaint::patch_image(&g.ctx, &src, &hole, None, 7, 1).is_some(), "PatchMatch fell back to the CPU");
}

#[test]
fn patchmatch_beats_the_membrane_on_texture() {
    let Some(g) = gpu() else { return };
    let (w, h) = (96u32, 72u32);
    let gt = background(w, h, 0.0);
    let hole = rect_hole(w, h, 40, 28, 16);
    let src = cut(&gt, &hole);
    let mem = membrane_fill(&src, &hole);
    let pm = g.patch(&src, &hole, None, 7, 1);
    let (pm_db, mem_db) = (psnr(&pm, &gt, Some(&hole)), psnr(&mem, &gt, Some(&hole)));
    assert!(pm_db > mem_db + 3.0 && pm_db > 20.0, "PatchMatch {pm_db:.1} dB vs membrane {mem_db:.1} dB");
    for i in 0..hole.len() {
        if !hole[i] {
            assert_eq!(pm.data[i], gt.data[i]);
        }
    }
}

#[test]
fn patchmatch_agrees_with_the_cpu() {
    let Some(g) = gpu() else { return };
    let (w, h) = (96u32, 72u32);
    let gt = background(w, h, 0.0);
    let hole = rect_hole(w, h, 40, 28, 16);
    let src = cut(&gt, &hole);
    let (cpu, gpu_pm) = (patch_fill(&src, &hole, None, 7, 1), g.patch(&src, &hole, None, 7, 1));
    // Both are syntheses of the same hole: neither is the other's ground truth, so they are
    // compared as two reconstructions of the same thing. On this periodic background both are
    // near-exact, so a relative dB check at machine precision says nothing; the device must be a
    // good reconstruction *and* agree with the CPU (a wrong fill that still looks plausible would
    // pass the first and fail the second).
    let gpu_db = psnr(&gpu_pm, &gt, Some(&hole));
    assert!(gpu_db > 25.0, "GPU {gpu_db:.1} dB");
    assert!(psnr(&gpu_pm, &cpu, Some(&hole)) > 30.0, "the two syntheses disagree");
}

/// The `init` path (a frame refined from the previous frame's warped fill) is the finest level
/// only, from two rounds of four iterations (`patch_fill`).
#[test]
fn patchmatch_refines_an_init_frame() {
    let Some(g) = gpu() else { return };
    let (w, h) = (96u32, 72u32);
    let gt = background(w, h, 0.0);
    let hole = rect_hole(w, h, 40, 28, 16);
    let src = cut(&gt, &hole);
    let init = gt.clone();
    let (cpu, gpu_pm) = (patch_fill(&src, &hole, Some(&init), 7, 1), g.patch(&src, &hole, Some(&init), 7, 1));
    assert!(psnr(&gpu_pm, &gt, Some(&hole)) > 25.0, "{:.1} dB", psnr(&gpu_pm, &gt, Some(&hole)));
    assert!(psnr(&gpu_pm, &cpu, Some(&hole)) > 20.0, "the two syntheses disagree");
}

/// A hole across the whole multi-scale pyramid (the coarse levels' membrane start included):
/// every level must come back finite and the pyramid must beat a single-level solve.
#[test]
fn patchmatch_survives_a_large_hole_across_levels() {
    let Some(g) = gpu() else { return };
    let (w, h) = (256u32, 192u32);
    let gt = background(w, h, 0.0);
    let hole = rect_hole(w, h, 90, 70, 80);
    let src = cut(&gt, &hole);
    let pm = g.patch(&src, &hole, None, 7, 1);
    assert!(pm.data.iter().flatten().all(|v| v.is_finite()), "the synthesis left a non-finite value");
    let db = psnr(&pm, &gt, Some(&hole));
    assert!(db > 20.0, "{db:.1} dB");
    for i in 0..hole.len() {
        if !hole[i] {
            assert_eq!(pm.data[i], gt.data[i], "known pixel {i}");
        }
    }
}

/// A moving object removed from a (static or panning) textured background, filled frame by
/// frame through the device backend the sequence driver uses.
#[test]
fn object_fill_recovers_background_and_is_stable() {
    let Some(g) = gpu() else { return };
    let (w, h) = (96u32, 64u32);
    for pan in [0.0f32, 1.0] {
        let mut frames = vec![];
        let mut holes = vec![];
        let mut gts = vec![];
        for t in 0..8u32 {
            let gt = background(w, h, t as f32 * pan);
            let hole = rect_hole(w, h, 10 + 8 * t, 22, 18);
            frames.push(cut(&gt, &hole));
            holes.push(hole);
            gts.push(gt);
        }
        let input = FillInput { frames, holes, references: vec![] };
        let out = fill_sequence_with(&input, &FillOpts::default(), g, &mut |_, _| true).expect("fill");
        for t in 0..out.len() {
            let db = psnr(&out[t], &gts[t], Some(&input.holes[t]));
            assert!(db > 26.0, "frame {t} pan {pan}: {db:.1} dB");
            assert_eq!(out[t].data[0][3], 1.0);
        }
        if pan == 0.0 {
            for t in 1..out.len() {
                let both: Vec<bool> = input.holes[t].iter().zip(&input.holes[t - 1]).map(|(a, b)| *a || *b).collect();
                let db = psnr(&out[t], &out[t - 1], Some(&both));
                assert!(db > 30.0, "frames {}–{t} differ: {db:.1} dB", t - 1);
            }
        }
    }
}

/// The membrane stays on the CPU (see [`crate::inpaint`]): Edge Blend must still get the CPU's
/// result through the backend, not a device one.
#[test]
fn edge_blend_takes_the_cpu_membrane() {
    let Some(g) = gpu() else { return };
    let (w, h) = (80u32, 60u32);
    let gt = background(w, h, 0.0);
    let hole = rect_hole(w, h, 30, 20, 14);
    let input = FillInput { frames: vec![cut(&gt, &hole); 4], holes: vec![hole.clone(); 4], references: vec![] };
    let opts = FillOpts { method: effectcraft_raster::inpaint::FillMethod::EdgeBlend, ..Default::default() };
    let edge = fill_sequence_with(&input, &opts, g, &mut |_, _| true).expect("fill");
    for t in 1..4 {
        assert!(psnr(&edge[t], &edge[0], Some(&hole)) > 40.0, "frame {t}");
    }
    assert!(psnr(&edge[0], &membrane_fill(&input.frames[0], &hole), Some(&hole)) > 60.0);
}

/// The dilation the level plans are built from (kept honest: the flags' target bit is `near`).
#[test]
fn the_target_band_is_the_dilated_hole() {
    let (w, h) = (24u32, 18u32);
    let hole = rect_hole(w, h, 8, 6, 4);
    let near = effectcraft_raster::inpaint::dilate(&hole, w as usize, h as usize, 3);
    assert!(near.iter().filter(|b| **b).count() > hole.iter().filter(|b| **b).count());
    assert!(near.iter().zip(&hole).all(|(n, h)| !*h || *n));
}

/// The flow's block matching runs on the device and finds the same motion the CPU does: the two
/// frames are the membrane-filled neighbours the sequence driver builds, with a hole in each.
#[test]
fn flow_agrees_with_the_cpu() {
    let Some(g) = gpu() else { return };
    let (w, h) = (192u32, 128u32);
    let a_gt = background(w, h, 0.0);
    let b_gt = background(w, h, 5.0);
    let hole_a = rect_hole(w, h, 30, 40, 40);
    let mut hole_b = vec![false; (w * h) as usize];
    for y in 50..90 {
        for x in 120..160 {
            hole_b[(y * w + x) as usize] = true;
        }
    }
    let a = membrane_fill(&cut(&a_gt, &hole_a), &hole_a);
    let b = membrane_fill(&cut(&b_gt, &hole_b), &hole_b);
    assert!(crate::inpaint::flow_image(&g.ctx, &a, &b).is_some(), "the flow fell back to the CPU");
    let cpu = effectcraft_raster::inpaint::CpuFill.flow(&a, &b, &hole_a, &hole_b);
    let dev = g.flow(&a, &b, &hole_a, &hole_b);
    assert_eq!((dev.cols, dev.rows, dev.block), (cpu.cols, cpu.rows, cpu.block));
    // The field is the CPU's to a float rounding: the luma is the CPU's (`luma_pair`), the
    // pyramid is exact additions, and the search enumerates candidates in the CPU's order.
    let diff = cpu.v.iter().zip(&dev.v).map(|(c, d)| (c[0] - d[0]).abs().max((c[1] - d[1]).abs())).fold(0.0f32, f32::max);
    assert!(diff <= 1e-6, "the flow fields differ by {diff}");
    // The motion is real: the panning background's 5 px shift survives the completion. `b`'s
    // content sits 5 px left of `a`'s (`background` adds `dx` inside the sample), so a → b is −5.
    let m = dev.at(96.0, 64.0);
    assert!((m[0] + 5.0).abs() < 0.5 && m[1].abs() < 0.5, "flow at centre: {m:?}");
}

/// The vote's σ² comes from a histogram that must count *every* target: the old per-thread
/// strided pass kept only the pixels whose bin selected that thread (a biased ~1/256 subsample),
/// which left small holes with a `total < 1` fallback and near-uniform vote weights.
#[test]
fn the_distance_histogram_counts_every_target() {
    let Some(g) = gpu() else { return };
    let (w, h) = (48u32, 32u32);
    let (mut field, near) = (Image::new(w, h), vec![true; (w * h) as usize]);
    for px in field.data.iter_mut() {
        // Every target has the same distance (0.5 → bin 128): the histogram is one spike.
        *px = [0.0, 0.0, 0.5, 1.0];
    }
    let (sigma2, total) = crate::inpaint::distance_histogram(&g.ctx, &field, &near, w, h).expect("histogram");
    assert_eq!(total as usize, (w * h) as usize, "the histogram kept {total} of {} samples", w * h);
    assert!((sigma2 - 128.5 / 256.0).abs() < 1e-3, "sigma2 {sigma2}");
}

/// A frame too small for one block is the CPU's zero field, on the device path too.
#[test]
fn small_frames_get_a_zero_flow() {
    let Some(g) = gpu() else { return };
    let (w, h) = (6u32, 6u32);
    let a = background(w, h, 0.0);
    let b = background(w, h, 2.0);
    let none = vec![false; (w * h) as usize];
    let f = g.flow(&a, &b, &none, &none);
    assert_eq!((f.cols, f.rows), (1, 1));
    assert_eq!(f.v, vec![[0.0, 0.0]]);
}

/// The flow the sequence driver uses (through the `Gpu` backend) keeps the fill stable: the same
/// frames filled with the CPU backend and the device backend agree, so the propagation sees the
/// same motion.
#[test]
fn sequence_fill_with_the_device_flow_matches_the_cpu() {
    let Some(g) = gpu() else { return };
    let (w, h) = (96u32, 64u32);
    let cpu = effectcraft_raster::inpaint::CpuFill;
    for pan in [0.0f32, 2.0] {
        let mut frames = vec![];
        let mut holes = vec![];
        for t in 0..6u32 {
            let gt = background(w, h, t as f32 * pan);
            let hole = rect_hole(w, h, 12 + 6 * t, 22, 18);
            frames.push(cut(&gt, &hole));
            holes.push(hole);
        }
        let input = FillInput { frames, holes, references: vec![] };
        let dev = fill_sequence_with(&input, &FillOpts::default(), g, &mut |_, _| true).expect("gpu fill");
        let ref_cpu = fill_sequence_with(&input, &FillOpts::default(), &cpu, &mut |_, _| true).expect("cpu fill");
        for t in 0..dev.len() {
            let db = psnr(&dev[t], &ref_cpu[t], Some(&input.holes[t]));
            assert!(db > 40.0, "frame {t} pan {pan}: device vs CPU {db:.1} dB");
        }
    }
}

// Modified work: the antialiased bilinear resize of PyTorch `F.interpolate(mode="bilinear",
// antialias=True, align_corners=False)`, as used by the Hugging Face `transformers` SAM 2 mask path
// (Apache-2.0 for the model code; the algorithm follows PyTorch's documented behaviour). See NOTICE.
//! Resizing masks to and from the model's resolution.

use candle_core::{Device, Tensor};

use crate::Result;
use crate::mask::validate_frame_size;
use crate::prompt::INPUT_SIZE;

/// For each output index: the first input index and the normalised triangle-filter weights.
fn axis_weights(n_in: usize, n_out: usize) -> Vec<(usize, Vec<f32>)> {
    let scale = n_in as f64 / n_out.max(1) as f64;
    let support = if scale >= 1.0 { scale } else { 1.0 };
    let invscale = if scale >= 1.0 { 1.0 / scale } else { 1.0 };
    (0..n_out)
        .map(|i| {
            let center = scale * (i as f64 + 0.5);
            let xmin = ((center - support + 0.5) as i64).max(0) as usize;
            let xmax = (((center + support + 0.5) as i64).max(0) as usize).min(n_in);
            let mut w: Vec<f32> = (xmin..xmax.max(xmin))
                .map(|x| {
                    let d = ((x as f64 - center + 0.5) * invscale).abs();
                    if d < 1.0 { (1.0 - d) as f32 } else { 0.0 }
                })
                .collect();
            let total: f32 = w.iter().sum();
            if total > 0.0 {
                w.iter_mut().for_each(|v| *v /= total);
            }
            (xmin, w)
        })
        .collect()
}

/// Antialiased bilinear resize of a row-major `h x w` image to `oh x ow`.
pub fn antialias_bilinear(src: &[f32], (h, w): (usize, usize), (oh, ow): (usize, usize)) -> Result<Vec<f32>> {
    validate_frame_size(w, h, src.len())?;
    let out_len = validate_frame_size(ow, oh, oh.saturating_mul(ow))?;
    let mid_len = validate_frame_size(ow, h, h.saturating_mul(ow))?;
    let xs = axis_weights(w, ow);
    let ys = axis_weights(h, oh);
    // horizontal pass: h x ow
    let mut mid = vec![0f32; mid_len];
    for y in 0..h {
        let row = src.get(y * w..(y + 1) * w).unwrap_or(&[]);
        for (x, (x0, wt)) in xs.iter().enumerate() {
            let acc: f32 = wt.iter().enumerate().map(|(k, a)| a * row.get(x0 + k).copied().unwrap_or(0.0)).sum();
            if let Some(o) = mid.get_mut(y * ow + x) {
                *o = acc;
            }
        }
    }
    // vertical pass: oh x ow
    let mut out = vec![0f32; out_len];
    for (y, (y0, wt)) in ys.iter().enumerate() {
        for x in 0..ow {
            let acc: f32 = wt.iter().enumerate().map(|(k, a)| a * mid.get((y0 + k) * ow + x).copied().unwrap_or(0.0)).sum();
            if let Some(o) = out.get_mut(y * ow + x) {
                *o = acc;
            }
        }
    }
    Ok(out)
}

/// A binary mask of any size as the model's mask prompt: `[1, 1, 1024, 1024]` of 0 and 1 (resized
/// like the reference: antialiased bilinear, then thresholded at one half).
pub fn mask_to_model(mask: &[u8], w: usize, h: usize) -> Result<Tensor> {
    validate_frame_size(w, h, mask.len())?;
    let side = INPUT_SIZE;
    let f: Vec<f32> = mask.iter().map(|v| f32::from(*v != 0)).collect();
    let data = if (w, h) == (side, side) { f } else { antialias_bilinear(&f, (h, w), (side, side))?.into_iter().map(|v| f32::from(v >= 0.5)).collect() };
    Ok(Tensor::from_vec(data, (1, 1, side, side), &Device::Cpu)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resize_rejects_overflowing_oversized_and_mismatched_grids() {
        for (input, output) in [((usize::MAX, 2), (1, 1)), ((1, 1), (usize::MAX, 2)), ((1, 1), (16_385, 1)), ((0, 1), (1, 1))] {
            assert!(antialias_bilinear(&[1.0], input, output).is_err(), "{input:?} to {output:?}");
        }
        assert!(mask_to_model(&[1], usize::MAX, 2).is_err());
        assert!(mask_to_model(&[1], INPUT_SIZE, INPUT_SIZE).is_err());
    }

    #[test]
    fn a_constant_stays_constant_and_weights_sum_to_one() {
        let out = antialias_bilinear(&[3.0; 64 * 64], (64, 64), (16, 16)).unwrap();
        assert!(out.iter().all(|v| (v - 3.0).abs() < 1e-5));
        let up = antialias_bilinear(&[2.0; 16], (4, 4), (9, 7)).unwrap();
        assert!(up.iter().all(|v| (v - 2.0).abs() < 1e-5));
    }

    #[test]
    fn a_half_plane_blurs_over_the_filter_width() {
        // 8 x 1: left half 0, right half 1, down to 2 x 1. The triangle filter is four pixels wide on
        // each side, so the first output sees the ones at weights 0.375 and 0.125 of a total 3.5.
        let src = [0., 0., 0., 0., 1., 1., 1., 1.];
        let out = antialias_bilinear(&src, (1, 8), (1, 2)).unwrap();
        assert!((out[0] - 0.5 / 3.5).abs() < 1e-6 && (out[1] - 3.0 / 3.5).abs() < 1e-6, "{out:?}");
    }

    #[test]
    fn hostile_sizes_are_errors() {
        assert!(antialias_bilinear(&[1.0; 4], (2, 2), (0, 3)).is_err());
        assert!(antialias_bilinear(&[1.0; 5], (2, 2), (1, 1)).is_err());
        assert!(mask_to_model(&[1; 3], 2, 2).is_err());
        assert!(mask_to_model(&[], 0, 0).is_err());
    }

    #[test]
    fn a_small_mask_becomes_a_model_sized_binary_mask() {
        let m = mask_to_model(&[0, 1, 1, 0], 2, 2).unwrap();
        assert_eq!(m.dims(), [1, 1, INPUT_SIZE, INPUT_SIZE]);
        let v = m.flatten_all().unwrap().to_vec1::<f32>().unwrap();
        assert!(v.iter().all(|x| *x == 0.0 || *x == 1.0));
        assert!(v.iter().sum::<f32>() > 0.0);
    }
}

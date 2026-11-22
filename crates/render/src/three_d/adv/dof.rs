//! Advanced 3D depth of field with the camera's iris and highlight options (the same Camera
//! Options as Classic 3D: Iris Shape from Fast Rectangle to Decagon, Iris Rotation, Roundness,
//! Aspect Ratio, Diffraction Fringe, Highlight Gain / Threshold / Saturation).
//!
//! Every pixel's blur radius comes from its camera depth (the circle of confusion of the
//! resolved scene, in output pixels). The image is blurred with the Classic 3D bokeh kernel
//! ([`crate::three_d::bokeh`]) at a few radii spanning the range and each pixel blends the two
//! levels around its own radius, so an out-of-focus point spreads into exactly the iris shape
//! a Classic 3D layer shows, and in-focus pixels stay sharp. Highlights are boosted before the
//! blur (only where the image is blurred) so bright points bloom into bokeh.

use rayon::prelude::*;

use crate::Image;
use crate::three_d::bokeh::{boost_highlights, progressive_blur};
use crate::three_d::camera::Dof;

/// Largest blur radius (output pixels).
const MAX_RADIUS: f32 = 48.0;

/// Depth of field of a resolved Advanced 3D image (premultiplied) with camera depth per pixel
/// (∞ where nothing was drawn: blurred as far away). A blurred pixel's alpha is at most 1.
pub fn depth_of_field(img: &Image, depth: &[f32], dof: &Dof, scale: f64) -> Image {
    let radius: Vec<f32> = depth
        .iter()
        .map(|&z| {
            let z = if z.is_finite() { z as f64 } else { 1.0e9 };
            ((dof.coc(z) * scale * 0.5) as f32).min(MAX_RADIUS)
        })
        .collect();
    let rmax = radius.iter().fold(0.0f32, |a, &b| a.max(b));
    if rmax < 0.5 {
        return img.clone();
    }
    let mut src = img.clone();
    if dof.highlight.gain > 0.0 {
        let mut boosted = img.clone();
        boost_highlights(&mut boosted, &dof.highlight);
        src.data.par_iter_mut().zip(boosted.data.par_iter()).zip(radius.par_iter()).for_each(|((d, b), &r)| {
            let k = (r - 0.5).clamp(0.0, 1.0);
            for c in 0..4 {
                d[c] += (b[c] - d[c]) * k;
            }
        });
    }
    let mut out = progressive_blur(&src, &dof.iris, &radius);
    // The gather's f32 prefix sums round, so a blurred opaque pixel on a wide row can come out
    // with alpha a little over 1 (1.00017 on an 8192 px row at a radius of 0.7 px), which the
    // encode rejects. Such a pixel is scaled to alpha 1, and its color by the same factor.
    out.data.par_iter_mut().for_each(|p| {
        if p[3] > 1.0 {
            let k = 1.0 / p[3];
            *p = [p[0] * k, p[1] * k, p[2] * k, 1.0];
        }
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::three_d::bokeh::{Highlight, Iris, bokeh_blur};

    fn dot(size: u32) -> Image {
        let mut img = Image::new(size, size);
        let c = (size / 2) as usize;
        img.data[c * size as usize + c] = [1.0, 1.0, 1.0, 1.0];
        img
    }

    /// Out of focus everywhere with a blur radius of `r` output pixels.
    fn dof(r: f64, iris: Iris) -> (Dof, Vec<f32>) {
        // coc = aperture · (z − focus) / z; radius = coc / 2.
        let (focus, z) = (500.0, 2000.0);
        let aperture = 2.0 * r * z / (z - focus);
        (Dof { focus, aperture, blur_level: 1.0, iris, highlight: Highlight::default() }, vec![z as f32; 81 * 81])
    }

    fn lit(img: &Image) -> Vec<bool> {
        let m = img.data.iter().fold(0.0f32, |a, p| a.max(p[3]));
        img.data.iter().map(|p| p[3] > m * 0.2).collect()
    }

    #[test]
    fn iris_shapes_match_the_classic_kernel() {
        for (sides, rotation, roundness, aspect) in
            [(3u32, 0.0, 0.0, 1.0), (5, 20.0, 0.0, 1.0), (6, 0.0, 0.0, 1.0), (10, 0.0, 0.5, 1.0), (4, 45.0, 0.0, 2.0), (0, 0.0, 0.0, 1.0)]
        {
            let iris = Iris { sides, rotation, roundness, aspect, fringe: 0.0 };
            let (d, depth) = dof(16.0, iris);
            let ours = lit(&depth_of_field(&dot(81), &depth, &d, 1.0));
            let classic = lit(&bokeh_blur(&dot(81), &iris, 16.0));
            let (mut same, mut any) = (0, 0);
            for (a, b) in ours.iter().zip(&classic) {
                if *a || *b {
                    any += 1;
                    if a == b {
                        same += 1;
                    }
                }
            }
            let iou = same as f64 / any as f64;
            assert!(iou > 0.8, "{sides} sides, rotation {rotation}: overlap {iou:.2}");
        }
    }

    #[test]
    fn triangle_is_not_a_disc() {
        let iris = Iris { sides: 3, ..Default::default() };
        let (d, depth) = dof(16.0, iris);
        let out = lit(&depth_of_field(&dot(81), &depth, &d, 1.0));
        // One side flat (half the radius from the centre), the opposite a vertex.
        let at = |x: usize| out[40 * 81 + x];
        assert_ne!(at(40 - 12), at(40 + 12));
    }

    #[test]
    fn fringe_brightens_the_rim_and_highlights_bloom() {
        let mut iris = Iris { sides: 8, ..Default::default() };
        let (mut d, depth) = dof(16.0, iris);
        let plain = depth_of_field(&dot(81), &depth, &d, 1.0);
        iris.fringe = 1.0;
        d.iris = iris;
        let rimmed = depth_of_field(&dot(81), &depth, &d, 1.0);
        let ratio = |im: &Image| im.data[40 * 81 + 40 + 14][3] / im.data[40 * 81 + 40 + 2][3];
        assert!(ratio(&rimmed) > ratio(&plain) * 1.3, "{} vs {}", ratio(&rimmed), ratio(&plain));
        // Highlight gain above the threshold brightens the bloom.
        d.iris.fringe = 0.0;
        d.highlight = Highlight { gain: 0.5, threshold: 0.5, saturation: 0.0 };
        let bright = depth_of_field(&dot(81), &depth, &d, 1.0);
        assert!(bright.data[40 * 81 + 40][0] > plain.data[40 * 81 + 40][0] * 1.5);
    }

    #[test]
    fn a_wide_opaque_image_stays_within_alpha_one_and_encodes() {
        // The gather's f32 prefix sums round along a row wider than 4096 px.
        let (w, h) = (8192u32, 3u32);
        let mut img = Image::new(w, h);
        img.data.fill([1.0; 4]);
        let (d, _) = dof(0.7, Iris { sides: 6, ..Default::default() });
        let depth = vec![2000.0f32; (w * h) as usize];
        let out = depth_of_field(&img, &depth, &d, 1.0);
        let encoded = super::super::encode(&mut out.clone());
        assert!(encoded.is_ok(), "{encoded:?}");
        let a = out.data.iter().fold(0.0f32, |m, p| m.max(p[3]));
        assert!(a <= 1.0, "alpha {a}");
        let row = &out.data[w as usize + 2..2 * w as usize - 2];
        assert!(row.iter().all(|p| p.iter().all(|&c| (c - 1.0).abs() < 1e-3)), "the middle row stays opaque white");
    }
}

//! Images that are transparent outside one rectangle of a larger frame.
//!
//! Glows of small shapes on large (comp-sized, padded) layers blur and resample mostly
//! transparent pixels. [`Sparse`] keeps only the rectangle that can be non-zero, and
//! [`gaussian_blur_sparse`] / [`resample_sparse`] return what [`gaussian_blur`] (transparent
//! edges) and [`resample`](crate::resample) return on the whole frame, restricted to the
//! rectangle that can be non-zero, so the work follows the content instead of the frame.

use effectcraft_geom::{Mat3, vec2};
use rayon::prelude::*;

use crate::blur::gaussian_support;
use crate::warp::half;
use crate::{Image, Px, gaussian_blur};

/// A `width × height` frame that is transparent outside `[x, x + img.width) × [y, y +
/// img.height)`, whose pixels `img` holds.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Sparse {
    pub img: Image,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl Sparse {
    /// A fully transparent frame.
    pub fn empty(width: u32, height: u32) -> Sparse {
        Sparse { img: Image::new(0, 0), x: 0, y: 0, width, height }
    }

    /// The whole of `img` as a frame of its size.
    pub fn full(img: Image) -> Sparse {
        let (width, height) = (img.width, img.height);
        Sparse { img, x: 0, y: 0, width, height }
    }

    /// Pixel at frame coordinates (transparent outside the rectangle).
    #[inline]
    pub fn get(&self, x: u32, y: u32) -> Px {
        self.img.get(i64::from(x) - i64::from(self.x), i64::from(y) - i64::from(self.y))
    }

    /// End of the rectangle (exclusive) along x and y.
    pub fn end(&self) -> (u32, u32) {
        (self.x.saturating_add(self.img.width).min(self.width), self.y.saturating_add(self.img.height).min(self.height))
    }

    /// The whole frame as an image.
    pub fn to_image(&self) -> Image {
        let mut out = Image::new(self.width, self.height);
        out.rows_mut().for_each(|(y, row)| {
            for (x, px) in row.iter_mut().enumerate() {
                *px = self.get(x as u32, y as u32);
            }
        });
        out
    }

    /// The same frame with its rectangle grown to `[x0, x1) × [y0, y1)` (clamped to the frame;
    /// the current rectangle is always kept).
    pub fn grown(&self, x0: u32, y0: u32, x1: u32, y1: u32) -> Sparse {
        let (ex, ey) = self.end();
        let (x0, y0) = (x0.min(self.x), y0.min(self.y));
        let (x1, y1) = (x1.min(self.width).max(ex), y1.min(self.height).max(ey));
        if self.img.is_empty() || (x0, y0, x1, y1) == (self.x, self.y, ex, ey) {
            return Sparse { img: if self.img.is_empty() { Image::new(0, 0) } else { self.img.clone() }, ..*self };
        }
        let mut img = Image::new(x1 - x0, y1 - y0);
        let (dx, dy) = ((self.x - x0) as usize, (self.y - y0) as usize);
        let (sw, ow) = (self.img.width as usize, img.width as usize);
        img.data.par_chunks_mut(ow).enumerate().skip(dy).take(self.img.height as usize).for_each(|(y, row)| {
            let s = (y - dy) * sw;
            row[dx..dx + sw].copy_from_slice(&self.img.data[s..s + sw]);
        });
        Sparse { img, x: x0, y: y0, width: self.width, height: self.height }
    }
}

/// [`gaussian_blur`] with transparent edges of the whole frame, computed on the rectangle
/// grown by the blur's reach ([`gaussian_support`]); outside it the frame blur is zero.
pub fn gaussian_blur_sparse(src: &Sparse, sigma_x: f64, sigma_y: f64) -> Sparse {
    if src.img.is_empty() {
        return Sparse::empty(src.width, src.height);
    }
    let rx = u32::try_from(gaussian_support(sigma_x)).unwrap_or(u32::MAX);
    let ry = u32::try_from(gaussian_support(sigma_y)).unwrap_or(u32::MAX);
    let (ex, ey) = src.end();
    let grown = src.grown(src.x.saturating_sub(rx), src.y.saturating_sub(ry), ex.saturating_add(rx), ey.saturating_add(ry));
    let img = gaussian_blur(&grown.img, sigma_x, sigma_y, false);
    Sparse { img, ..grown }
}

/// [`resample`](crate::resample) of the whole frame to `w × h`, computed only where it can be
/// non-zero: the same sample positions, pre-filter levels and blending, so it matches the
/// frame resample to float rounding.
pub fn resample_sparse(src: &Sparse, w: u32, h: u32) -> Sparse {
    let mut out = Sparse::empty(w, h);
    if src.img.is_empty() || src.width == 0 || src.height == 0 || w == 0 || h == 0 {
        return out;
    }
    let m = Mat3::scale(vec2(w as f64 / src.width as f64, h as f64 / src.height as f64));
    let Some(mut inv) = m.inverse() else { return out };
    // Minification levels, as in `composite_warp`: box halvings of the whole frame.
    let (mut lw, mut lh) = (src.width, src.height);
    let mut levels = 0u32;
    if m.mean_scale() < 0.5 && m.is_affine() {
        let mut s = m.mean_scale();
        let mut f = 1.0;
        loop {
            lw = lw.div_ceil(2).max(1);
            lh = lh.div_ceil(2).max(1);
            levels += 1;
            f *= 0.5;
            s *= 2.0;
            if !(s < 0.5 && lw > 1 && lh > 1) {
                break;
            }
        }
        inv = Mat3::scale(vec2(f, f)) * inv;
    }
    // Halve the rectangle on the frame's 2^levels grid: aligned at the start and with
    // transparent pixels past its end, every halving equals the frame's on that rectangle.
    let level_src;
    let (level, lx, ly) = if levels > 0 {
        let unit = 1u32.checked_shl(levels).unwrap_or(u32::MAX);
        let (ex, ey) = src.end();
        let pad = unit.saturating_mul(2);
        let grown = src.grown(src.x - src.x % unit, src.y - src.y % unit, ex.saturating_add(pad), ey.saturating_add(pad));
        let mut img = half(&grown.img);
        for _ in 1..levels {
            img = half(&img);
        }
        level_src = img;
        (&level_src, grown.x >> levels, grown.y >> levels)
    } else {
        (&src.img, src.x, src.y)
    };
    let mi = inv.0;
    let (iw, ih) = (lw as f64, lh as f64);
    // Output pixels whose bilinear taps can reach the rectangle (two pixels of slack).
    let span = |origin: u32, len: u32, step: f64, n: u32| -> (u32, u32) {
        if step <= 0.0 || !step.is_finite() {
            return (0, n);
        }
        let lo = ((f64::from(origin) - 0.5) / step - 0.5).floor() - 2.0;
        let hi = ((f64::from(origin) + f64::from(len) + 0.5) / step).ceil() + 2.0;
        (lo.clamp(0.0, f64::from(n)) as u32, hi.clamp(0.0, f64::from(n)) as u32)
    };
    let (x0, x1) = span(lx, level.width, mi[0][0], w);
    let (y0, y1) = span(ly, level.height, mi[1][1], h);
    if x0 >= x1 || y0 >= y1 {
        return out;
    }
    let (ox, oy) = (f64::from(lx), f64::from(ly));
    let mut img = Image::new(x1 - x0, y1 - y0);
    img.rows_mut().for_each(|(row_y, row)| {
        let fy = (y0 as usize + row_y) as f64 + 0.5;
        // Row start and step exactly as `composite_warp`'s affine rows (area starting at x 0).
        let p0 = (mi[0][0] * 0.5 + mi[0][1] * fy + mi[0][2], mi[1][0] * 0.5 + mi[1][1] * fy + mi[1][2]);
        for (i, px) in row.iter_mut().enumerate() {
            let x = (x0 as usize + i) as f64;
            let p = (p0.0 + mi[0][0] * x, p0.1 + mi[1][0] * x);
            if p.0 < -1.0 || p.1 < -1.0 || p.0 > iw + 1.0 || p.1 > ih + 1.0 {
                continue;
            }
            // Integer offsets keep the sample's fractional position exact.
            let s = level.sample_bilinear(p.0 - ox, p.1 - oy);
            // Normal over a transparent pixel: the sample itself (skipped where transparent).
            if s[3] > 0.0 {
                *px = s;
            }
        }
    });
    out.img = img;
    out.x = x0;
    out.y = y0;
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{hash_noise, resample};

    /// A noisy blob in `[bx0, bx1) × [by0, by1)` of a `w × h` frame.
    fn blob(w: u32, h: u32, bx0: u32, by0: u32, bx1: u32, by1: u32) -> Image {
        let mut img = Image::new(w, h);
        for y in by0..by1 {
            for x in bx0..bx1 {
                let a = hash_noise(x, y, 4);
                img.set(x, y, [a * hash_noise(x, y, 5), a * 0.5, a * hash_noise(x, y, 6), a]);
            }
        }
        img
    }

    fn sparse_of(img: &Image, bx0: u32, by0: u32, bx1: u32, by1: u32) -> Sparse {
        let crop = img.crop(i64::from(bx0), i64::from(by0), bx1 - bx0, by1 - by0);
        Sparse { img: crop, x: bx0, y: by0, width: img.width, height: img.height }
    }

    fn max_diff(a: &Image, b: &Image) -> f32 {
        assert_eq!((a.width, a.height), (b.width, b.height));
        a.data.iter().zip(&b.data).flat_map(|(p, q)| (0..4).map(move |c| (p[c] - q[c]).abs())).fold(0.0, f32::max)
    }

    const BLOBS: [(u32, u32, u32, u32); 4] = [(40, 30, 70, 52), (0, 0, 20, 9), (150, 90, 181, 120), (3, 100, 181, 107)];

    #[test]
    fn sparse_blur_matches_frame_blur() {
        let (w, h) = (181, 120);
        for &(bx0, by0, bx1, by1) in &BLOBS {
            let img = blob(w, h, bx0, by0, bx1, by1);
            let s = sparse_of(&img, bx0, by0, bx1, by1);
            for (sx, sy) in [(0.45, 0.45), (4.5, 4.5), (18.0, 7.0), (0.0, 30.0), (60.0, 0.02)] {
                let full = gaussian_blur(&img, sx, sy, false);
                let fast = gaussian_blur_sparse(&s, sx, sy).to_image();
                let d = max_diff(&full, &fast);
                assert!(d < 1e-5, "blob {bx0},{by0} sigma {sx},{sy}: {d}");
            }
        }
    }

    #[test]
    fn sparse_resample_matches_frame_resample() {
        let (w, h) = (181, 120);
        for &(bx0, by0, bx1, by1) in &BLOBS {
            let img = blob(w, h, bx0, by0, bx1, by1);
            let s = sparse_of(&img, bx0, by0, bx1, by1);
            for (ow, oh) in [(145, 96), (181, 120), (300, 200), (90, 60), (40, 26), (18, 12), (7, 5), (1, 1), (2000, 33)] {
                let full = resample(&img, ow, oh);
                let small = resample_sparse(&s, ow, oh);
                let d = max_diff(&full, &small.to_image());
                assert_eq!(d, 0.0, "blob {bx0},{by0} to {ow}x{oh}");
                // And back up again from the sparse result.
                let back = resample(&full, w, h);
                let d = max_diff(&back, &resample_sparse(&small, w, h).to_image());
                assert_eq!(d, 0.0, "blob {bx0},{by0} back from {ow}x{oh}");
            }
        }
    }

    #[test]
    fn empty_sparse_stays_empty() {
        let e = Sparse::empty(30, 20);
        assert!(gaussian_blur_sparse(&e, 5.0, 5.0).img.is_empty());
        assert!(resample_sparse(&e, 10, 10).img.is_empty());
        assert_eq!(e.to_image(), Image::new(30, 20));
    }
}

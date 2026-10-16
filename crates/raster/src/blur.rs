//! Blurs: separable box passes (exact running sums) approximating Gaussians, plus directional
//! and radial (spin/zoom) blurs.
//!
//! Box passes run straight on row-major pixels: horizontal passes per row, vertical passes over
//! blocks of rows with a per-column running sum (no transposes), ping-ponging between two
//! buffers so a Gaussian allocates two images regardless of the number of passes.

use rayon::prelude::*;

use crate::{Image, Px};

/// One horizontal box pass of radius `r` (window 2r+1) over each row of `src` (width `w`) into
/// `out`; `repeat` = edge pixels extend, otherwise transparent.
fn box_h_into(src: &[Px], out: &mut [Px], w: usize, r: usize, repeat: bool) {
    if r == 0 {
        out.copy_from_slice(src);
        return;
    }
    let norm = 1.0 / (2 * r + 1) as f32;
    out.par_chunks_mut(w).zip(src.par_chunks(w)).for_each(|(o, row)| {
        let get = |i: isize| -> Px {
            if i < 0 {
                if repeat { row[0] } else { [0.0; 4] }
            } else if i as usize >= w {
                if repeat { row[w - 1] } else { [0.0; 4] }
            } else {
                row[i as usize]
            }
        };
        let mut acc = [0.0f32; 4];
        for i in -(r as isize)..=(r as isize) {
            let p = get(i);
            for c in 0..4 {
                acc[c] += p[c];
            }
        }
        for x in 0..w {
            o[x] = acc.map(|v| v * norm);
            let add = get(x as isize + r as isize + 1);
            let sub = get(x as isize - r as isize);
            for c in 0..4 {
                acc[c] += add[c] - sub[c];
            }
        }
    });
}

/// One vertical box pass (same semantics as [`box_h_into`] along columns).
fn box_v_into(src: &[Px], out: &mut [Px], w: usize, h: usize, r: usize, repeat: bool) {
    if r == 0 {
        out.copy_from_slice(src);
        return;
    }
    let norm = 1.0 / (2 * r + 1) as f32;
    let block = (4 * r).clamp(32, 256);
    let row = |i: isize| -> Option<&[Px]> {
        if i < 0 {
            repeat.then(|| &src[0..w])
        } else if i as usize >= h {
            repeat.then(|| &src[(h - 1) * w..h * w])
        } else {
            Some(&src[i as usize * w..(i as usize + 1) * w])
        }
    };
    out.par_chunks_mut(w * block).enumerate().for_each(|(bi, chunk)| {
        let ya = (bi * block) as isize;
        let mut acc = vec![[0.0f32; 4]; w];
        for i in ya - r as isize..=ya + r as isize {
            if let Some(rw) = row(i) {
                for (a, p) in acc.iter_mut().zip(rw) {
                    for c in 0..4 {
                        a[c] += p[c];
                    }
                }
            }
        }
        for (yy, o) in chunk.chunks_mut(w).enumerate() {
            let y = ya + yy as isize;
            for (o, a) in o.iter_mut().zip(&acc) {
                *o = a.map(|v| v * norm);
            }
            match (row(y + r as isize + 1), row(y - r as isize)) {
                (Some(add), Some(sub)) => {
                    for ((a, p), q) in acc.iter_mut().zip(add).zip(sub) {
                        for c in 0..4 {
                            a[c] += p[c] - q[c];
                        }
                    }
                }
                (Some(add), None) => {
                    for (a, p) in acc.iter_mut().zip(add) {
                        for c in 0..4 {
                            a[c] += p[c];
                        }
                    }
                }
                (None, Some(sub)) => {
                    for (a, q) in acc.iter_mut().zip(sub) {
                        for c in 0..4 {
                            a[c] -= q[c];
                        }
                    }
                }
                (None, None) => {}
            }
        }
    });
}

/// Run horizontal passes with radii `rx` then vertical passes with radii `ry`.
fn box_passes(img: &Image, rx: &[usize], ry: &[usize], repeat: bool) -> Image {
    let (w, h) = (img.width as usize, img.height as usize);
    let passes: Vec<(bool, usize)> = rx.iter().map(|&r| (false, r)).chain(ry.iter().map(|&r| (true, r))).filter(|p| p.1 > 0).collect();
    if passes.is_empty() || w == 0 || h == 0 {
        return img.clone();
    }
    let mut a = Image::new(img.width, img.height);
    let mut b = Image::new(img.width, img.height);
    for (i, &(vertical, r)) in passes.iter().enumerate() {
        let src: &[Px] = if i == 0 { &img.data } else { &a.data };
        if vertical {
            box_v_into(src, &mut b.data, w, h, r, repeat);
        } else {
            box_h_into(src, &mut b.data, w, r, repeat);
        }
        std::mem::swap(&mut a, &mut b);
    }
    a
}

/// Box radii for `n` passes approximating a Gaussian of `sigma` (Kovesi / "boxes for Gauss").
fn box_radii(sigma: f64, n: usize) -> Vec<usize> {
    if sigma <= 0.0 {
        return vec![0; n];
    }
    let w_ideal = (12.0 * sigma * sigma / n as f64 + 1.0).sqrt();
    let mut wl = w_ideal.floor() as i64;
    if wl % 2 == 0 {
        wl -= 1;
    }
    let wu = wl + 2;
    let m_ideal = (12.0 * sigma * sigma - (n as i64 * wl * wl) as f64 - 4.0 * n as f64 * wl as f64 - 3.0 * n as f64) / (-4.0 * wl as f64 - 4.0);
    let m = m_ideal.round() as i64;
    (0..n as i64).map(|i| (((if i < m { wl } else { wu }) - 1) / 2).max(0) as usize).collect()
}

/// Separable box blur with given radii, `iterations` passes.
pub fn box_blur(img: &Image, rx: usize, ry: usize, iterations: usize, repeat: bool) -> Image {
    let n = iterations.max(1);
    box_passes(img, &vec![rx; n], &vec![ry; n], repeat)
}

/// Gaussian blur with standard deviations in pixels (3 box passes per axis).
pub fn gaussian_blur(img: &Image, sigma_x: f64, sigma_y: f64, repeat: bool) -> Image {
    let rx = if sigma_x > 0.05 { box_radii(sigma_x, 3) } else { vec![] };
    let ry = if sigma_y > 0.05 { box_radii(sigma_y, 3) } else { vec![] };
    if rx.iter().chain(&ry).all(|&r| r == 0) {
        return img.clone();
    }
    let f = coarse_factor(sigma_x, sigma_y);
    let reach = |r: &[usize]| r.iter().fold(0usize, |a, &b| a.saturating_add(b)).saturating_add(2 * f).min(u32::MAX as usize) as u32;
    // Box passes without edge repeat cut what spills past the buffer's edge after each pass; the
    // coarse grid can't, so it only takes content whose blur stays inside the buffer (as it does
    // for a padded layer).
    let blur = |part: &Image, inside: bool| {
        if f > 1 && (repeat || inside) { coarse_gaussian(part, sigma_x, sigma_y, f, repeat) } else { box_passes(part, &rx, &ry, repeat) }
    };
    within_reach(img, reach(&rx), reach(&ry), blur)
}

/// `blur` run over just the part of `img` that can end up non-zero: its content bounds grown by
/// the blur's reach `(gx, gy)`. Outside that the result is zero either way (zero pixels stay
/// zero, and with edge repeat the crop's edge pixels are zero wherever the crop stops short of
/// the image edge), so this is the same blur, much cheaper on a large buffer that draws little.
/// `blur` is told whether the content's reach lies inside the image.
fn within_reach(img: &Image, gx: u32, gy: u32, blur: impl Fn(&Image, bool) -> Image) -> Image {
    let (w, h) = (img.width as u64, img.height as u64);
    if w == 0 || h == 0 {
        return img.clone();
    }
    let Some((x0, y0, x1, y1)) = img.content_bounds() else {
        return Image::new(img.width, img.height);
    };
    let inside = x0 >= gx && y0 >= gy && x1.saturating_add(gx) <= img.width && y1.saturating_add(gy) <= img.height;
    let (cx0, cy0) = (x0.saturating_sub(gx), y0.saturating_sub(gy));
    let (cx1, cy1) = (x1.saturating_add(gx).min(img.width), y1.saturating_add(gy).min(img.height));
    let area = (cx1 - cx0) as u64 * (cy1 - cy0) as u64;
    if area * 4 >= w * h * 3 {
        return blur(img, inside);
    }
    let part = blur(&img.crop_rect(cx0, cy0, cx1, cy1), inside);
    let mut out = Image::new(img.width, img.height);
    out.paste(&part, cx0, cy0);
    out
}

/// Gaussians at least this wide (sigma, pixels, on both axes) are blurred at a reduced
/// resolution: the result is so smooth that a coarser grid holds it to well under one 8-bit level,
/// at a fraction of the cost (a 190 px vignette blur: 1/256 of the pixels).
const COARSE_SIGMA: f64 = 16.0;
/// The coarse grid keeps at least this many coarse pixels per sigma.
const COARSE_STEPS: f64 = 8.0;

/// The reduction for a Gaussian of these sigmas: a power of two up to 16, 1 = full resolution.
fn coarse_factor(sigma_x: f64, sigma_y: f64) -> usize {
    let s = sigma_x.min(sigma_y);
    if !sigma_x.is_finite() || !sigma_y.is_finite() || s < COARSE_SIGMA {
        return 1;
    }
    let mut f = 1usize;
    while f < 16 && s / (2 * f) as f64 >= COARSE_STEPS {
        f *= 2;
    }
    f
}

/// [`gaussian_blur`] on a grid `f` times coarser: average `f`×`f` blocks, blur with the sigma
/// that leaves (the block average and the bilinear return add about f²/4 of variance), and
/// interpolate back. Outside the image is transparent, or with `repeat` the edge extends.
fn coarse_gaussian(img: &Image, sigma_x: f64, sigma_y: f64, f: usize, repeat: bool) -> Image {
    let (w, h) = (img.width as i64, img.height as i64);
    // The coarse grid starts `m` pixels before the image and ends past it, so the image's own
    // edge falls between coarse samples and is interpolated like any other place.
    let m = 2 * f as i64;
    let fi = f as i64;
    let (lw, lh) = ((w + 2 * m + fi - 1) / fi, (h + 2 * m + fi - 1) / fi);
    let mut low = Image::new(lw as u32, lh as u32);
    let read = |x: i64, y: i64| if repeat { img.get_clamped(x, y) } else { img.get(x, y) };
    let norm = 1.0 / (f * f) as f32;
    low.rows_mut().for_each(|(ly, row)| {
        let y0 = ly as i64 * fi - m;
        for (lx, px) in row.iter_mut().enumerate() {
            let x0 = lx as i64 * fi - m;
            let mut acc = [0.0f32; 4];
            for y in y0..y0 + fi {
                for x in x0..x0 + fi {
                    let p = read(x, y);
                    for c in 0..4 {
                        acc[c] += p[c];
                    }
                }
            }
            *px = acc.map(|v| v * norm);
        }
    });
    let extra = (f * f) as f64 / 4.0;
    let coarse = |s: f64| ((s * s - extra).max(0.25)).sqrt() / f as f64;
    let blurred = box_passes(&low, &box_radii(coarse(sigma_x), 3), &box_radii(coarse(sigma_y), 3), repeat);
    let mut out = Image::new(img.width, img.height);
    let k = 1.0 / f as f64;
    out.rows_mut().for_each(|(y, row)| {
        let sy = (y as i64 + m) as f64 * k + 0.5 * k;
        for (x, px) in row.iter_mut().enumerate() {
            let sx = (x as i64 + m) as f64 * k + 0.5 * k;
            *px = blurred.sample_bilinear_clamped(sx, sy);
        }
    });
    out
}

/// Motion blur along `angle_deg` (0 = vertical in AE's Directional Blur, measured clockwise from
/// up) over `length` pixels.
pub fn directional_blur(img: &Image, angle_deg: f64, length: f64) -> Image {
    if length < 0.5 {
        return img.clone();
    }
    let a = angle_deg.to_radians();
    let (dx, dy) = (a.sin(), -a.cos());
    let n = (length.ceil() as usize * 2 + 1).clamp(3, 257);
    let mut out = Image::new(img.width, img.height);
    out.rows_mut().for_each(|(y, row)| {
        for (x, px) in row.iter_mut().enumerate() {
            let mut acc = [0.0f32; 4];
            for i in 0..n {
                let t = (i as f64 / (n - 1) as f64 - 0.5) * length;
                let p = img.sample_bilinear(x as f64 + 0.5 + dx * t, y as f64 + 0.5 + dy * t);
                for c in 0..4 {
                    acc[c] += p[c];
                }
            }
            *px = acc.map(|v| v / n as f32);
        }
    });
    out
}

/// Radial blur around `center`: `spin` (degrees) or zoom (`amount` as a fraction of distance).
pub fn radial_blur(img: &Image, center: (f64, f64), amount: f64, zoom: bool) -> Image {
    if amount.abs() < 1e-6 {
        return img.clone();
    }
    let n = 32usize;
    let mut out = Image::new(img.width, img.height);
    out.rows_mut().for_each(|(y, row)| {
        for (x, px) in row.iter_mut().enumerate() {
            let (vx, vy) = (x as f64 + 0.5 - center.0, y as f64 + 0.5 - center.1);
            let mut acc = [0.0f32; 4];
            for i in 0..n {
                let t = i as f64 / (n - 1) as f64 - 0.5;
                let (sx, sy) = if zoom {
                    let k = 1.0 + t * amount;
                    (center.0 + vx * k, center.1 + vy * k)
                } else {
                    let (s, c) = (t * amount).to_radians().sin_cos();
                    (center.0 + vx * c - vy * s, center.1 + vx * s + vy * c)
                };
                let p = img.sample_bilinear(sx, sy);
                for ch in 0..4 {
                    acc[ch] += p[ch];
                }
            }
            *px = acc.map(|v| v / n as f32);
        }
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gaussian_preserves_mass() {
        let mut img = Image::new(64, 64);
        img.set(32, 32, [1.0; 4]);
        let b = gaussian_blur(&img, 4.0, 4.0, false);
        let sum: f32 = b.data.iter().map(|p| p[3]).sum();
        assert!((sum - 1.0).abs() < 1e-3, "{sum}");
        assert!(b.get(32, 32)[3] < 0.05);
    }

    #[test]
    fn flat_image_stays_flat_with_repeat() {
        let img = Image::filled(20, 10, [0.5, 0.5, 0.5, 1.0]);
        let b = gaussian_blur(&img, 3.0, 3.0, true);
        assert!(b.data.iter().all(|p| (p[0] - 0.5).abs() < 1e-5));
    }

    /// Direct (windowed sum) box filter along x or y: the definition the passes must match.
    fn box_ref(img: &Image, r: usize, vertical: bool, repeat: bool) -> Image {
        let mut out = Image::new(img.width, img.height);
        let (w, h) = (img.width as i64, img.height as i64);
        for y in 0..h {
            for x in 0..w {
                let mut acc = [0.0f64; 4];
                for k in -(r as i64)..=(r as i64) {
                    let (sx, sy) = if vertical { (x, y + k) } else { (x + k, y) };
                    let p = if repeat { img.get_clamped(sx, sy) } else { img.get(sx, sy) };
                    for c in 0..4 {
                        acc[c] += p[c] as f64;
                    }
                }
                out.set(x as u32, y as u32, acc.map(|v| (v / (2 * r + 1) as f64) as f32));
            }
        }
        out
    }

    #[test]
    fn passes_match_direct_box_filter() {
        let mut img = Image::new(53, 301);
        for y in 0..301 {
            for x in 0..53 {
                let v = crate::hash_noise(x, y, 3);
                img.set(x, y, [v * 0.5, v * 0.25, v, v]);
            }
        }
        for repeat in [false, true] {
            for (rx, ry) in [(0, 3), (2, 0), (5, 9), (40, 70), (1, 200)] {
                let fast = box_passes(&img, &[rx, rx + 1], &[ry, ry / 2], repeat);
                let mut slow = img.clone();
                for r in [rx, rx + 1] {
                    slow = box_ref(&slow, r, false, repeat);
                }
                for r in [ry, ry / 2] {
                    slow = box_ref(&slow, r, true, repeat);
                }
                for (i, (p, q)) in fast.data.iter().zip(&slow.data).enumerate() {
                    for c in 0..4 {
                        assert!((p[c] - q[c]).abs() < 1e-4, "r {rx},{ry} repeat {repeat} px {i}: {p:?} vs {q:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn radii_sane() {
        let r = box_radii(10.0, 3);
        assert_eq!(r.len(), 3);
        assert!(r.iter().all(|&v| (7..=10).contains(&v)), "{r:?}");
    }

    /// The blur restricted to the content's reach is the full blur (to float rounding), whether
    /// the content sits mid-buffer, touches an edge or is absent.
    #[test]
    fn roi_blur_matches_full_blur() {
        let blobs: [&[(u32, u32)]; 4] = [&[(150, 90)], &[(0, 3), (5, 0)], &[(299, 179), (150, 90)], &[]];
        for blob in blobs {
            let mut img = Image::new(300, 180);
            for &(cx, cy) in blob {
                for y in cy.saturating_sub(6)..(cy + 6).min(180) {
                    for x in cx.saturating_sub(9)..(cx + 9).min(300) {
                        let v = crate::hash_noise(x, y, 7);
                        img.set(x, y, [v * 0.3, v * 0.6, v, v]);
                    }
                }
            }
            for repeat in [false, true] {
                for (sx, sy) in [(4.0, 4.0), (0.0, 12.0), (20.0, 0.0), (2.5, 31.0)] {
                    let rx = if sx > 0.05 { box_radii(sx, 3) } else { vec![] };
                    let ry = if sy > 0.05 { box_radii(sy, 3) } else { vec![] };
                    let full = box_passes(&img, &rx, &ry, repeat);
                    let roi = gaussian_blur(&img, sx, sy, repeat);
                    assert_eq!((roi.width, roi.height), (full.width, full.height));
                    for (i, (p, q)) in roi.data.iter().zip(&full.data).enumerate() {
                        for c in 0..4 {
                            assert!((p[c] - q[c]).abs() < 1e-5, "{blob:?} repeat {repeat} sigma ({sx}, {sy}) px {i}: {p:?} vs {q:?}");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn content_bounds_crop_and_paste() {
        let mut img = Image::new(10, 8);
        assert_eq!(img.content_bounds(), None);
        img.set(3, 2, [0.0, 0.0, 0.0, 0.5]);
        img.set(7, 5, [0.1, 0.0, 0.0, 0.0]);
        assert_eq!(img.content_bounds(), Some((3, 2, 8, 6)));
        img.set(0, 7, [f32::NAN, 0.0, 0.0, 0.0]);
        assert_eq!(img.content_bounds(), Some((0, 2, 8, 8)));
        let c = img.crop_rect(3, 2, 8, 6);
        assert_eq!((c.width, c.height), (5, 4));
        assert_eq!(c.get(0, 0), [0.0, 0.0, 0.0, 0.5]);
        let mut out = Image::new(10, 8);
        out.paste(&c, 3, 2);
        assert_eq!(out.get(7, 5), [0.1, 0.0, 0.0, 0.0]);
        // clipped and out-of-range rectangles never panic
        assert_eq!(img.crop_rect(8, 6, 100, 100).width, 2);
        assert!(img.crop_rect(20, 20, 5, 5).is_empty());
        out.paste(&c, 9, 7);
        out.paste(&c, 50, 50);
        Image::new(0, 0).paste(&c, 0, 0);
    }

    /// The coarse-grid Gaussian stays within a small fraction of an 8-bit level of the
    /// full-resolution one, for hard edges and for content at the buffer's edge.
    #[test]
    fn coarse_blur_matches_full_blur() {
        // non-repeat blurs see padded content (a margin of three sigmas, as the effects pad)
        let mut img = Image::new(420 + 600, 260 + 600);
        for y in 300..560u32 {
            for x in 300..720u32 {
                // a hard-edged frame with a hole (a vignette), plus a noisy corner block
                let hole = ((x as f64 - 510.0) / 170.0).powi(2) + ((y as f64 - 430.0) / 100.0).powi(2) < 1.0;
                let mut p = if hole { [0.0; 4] } else { [0.1, 0.05, 0.0, 0.8] };
                if x < 360 && y < 340 {
                    let v = crate::hash_noise(x, y, 5);
                    p = [v, v * 0.5, 0.2 * v, v];
                }
                img.set(x, y, p);
            }
        }
        for repeat in [false, true] {
            for (sx, sy) in [(16.0, 16.0), (24.0, 40.0), (95.0, 95.0)] {
                let f = coarse_factor(sx, sy);
                assert!(f > 1, "{sx}");
                let exact = box_passes(&img, &box_radii(sx, 3), &box_radii(sy, 3), repeat);
                let fast = gaussian_blur(&img, sx, sy, repeat);
                let worst = exact.data.iter().zip(&fast.data).flat_map(|(p, q)| (0..4).map(move |c| (p[c] - q[c]).abs())).fold(0.0f32, f32::max);
                assert!(worst < 1.5 / 255.0, "repeat {repeat} sigma ({sx}, {sy}) factor {f}: off by {}", worst * 255.0);
            }
        }
        // content touching the edge without repeat takes the exact passes
        let edge = Image::filled(300, 200, [0.0, 0.0, 0.0, 1.0]);
        assert_eq!(gaussian_blur(&edge, 40.0, 40.0, false), box_passes(&edge, &box_radii(40.0, 3), &box_radii(40.0, 3), false));
        assert_eq!(coarse_factor(15.0, 200.0), 1);
        assert_eq!(coarse_factor(f64::NAN, 200.0), 1);
        assert_eq!(coarse_factor(f64::INFINITY, f64::INFINITY), 1);
        assert_eq!(coarse_factor(190.0, 190.0), 16);
        assert_eq!(coarse_factor(24.0, 30.0), 2);
    }
}

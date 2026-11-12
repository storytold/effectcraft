//! Blurs: separable box passes (exact running sums) approximating Gaussians, a sampled Gaussian
//! kernel for small sigmas, plus directional and radial (spin/zoom) blurs.
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
        // The starting window: the pixels inside the row once each, and the repeated edge pixels
        // counted rather than visited (a radius far past the row would otherwise loop over it).
        let mut acc = [0.0f32; 4];
        let inside = r.min(w - 1);
        for p in &row[..=inside] {
            for c in 0..4 {
                acc[c] += p[c];
            }
        }
        if repeat {
            let (left, right) = (r as f32, (r - inside) as f32);
            for c in 0..4 {
                acc[c] += row[0][c] * left;
                // A zero count adds nothing (0 * NaN would be NaN).
                if right > 0.0 {
                    acc[c] += row[w - 1][c] * right;
                }
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
        // The starting window: rows inside the image once each, the repeated edge rows counted.
        let (lo, hi) = (ya - r as isize, ya + r as isize);
        let add = |acc: &mut [Px], rw: &[Px], k: f32| {
            // A zero count adds nothing (0 * NaN would be NaN).
            if k == 0.0 {
                return;
            }
            for (a, p) in acc.iter_mut().zip(rw) {
                for c in 0..4 {
                    a[c] += p[c] * k;
                }
            }
        };
        for i in lo.max(0)..=hi.min(h as isize - 1) {
            add(&mut acc, &src[i as usize * w..(i as usize + 1) * w], 1.0);
        }
        if repeat {
            let above = (-lo).max(0) as f32;
            let below = (hi - (h as isize - 1)).max(0) as f32;
            add(&mut acc, &src[0..w], above);
            add(&mut acc, &src[(h - 1) * w..h * w], below);
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
    box_passes(img, &rx, &ry, repeat)
}

/// Below this sigma (pixels) three integer-radius box passes are too coarse (sigma under ~0.6 is
/// a no-op and the result steps between a few shapes), so [`gaussian_blur_exact`] is used instead.
const EXACT_SIGMA_BELOW: f64 = 3.0;

/// True when a Gaussian of these per-axis sigmas should use [`gaussian_blur_exact`].
pub fn gaussian_is_exact(sigma_x: f64, sigma_y: f64) -> bool {
    sigma_x.max(sigma_y) < EXACT_SIGMA_BELOW
}

/// Pixels per tile of [`gauss_pass`]'s vertical pass.
const TILE: usize = 128;

/// One separable pass of a sampled Gaussian (radius 3 sigma) along x or y.
fn gauss_pass(img: &Image, sigma: f64, vertical: bool, repeat: bool) -> Image {
    // Below 1e-3 the kernel is the identity in f64; tiny sigmas would also make 2σ² underflow to 0 (NaN taps).
    if sigma.is_nan() || sigma <= 1e-3 || img.is_empty() {
        return img.clone();
    }
    let r = (sigma * 3.0).ceil() as i64;
    let mut k: Vec<f64> = (-r..=r).map(|i| (-((i * i) as f64) / (2.0 * sigma * sigma)).exp()).collect();
    let total: f64 = k.iter().sum();
    k.iter_mut().for_each(|v| *v /= total);
    let (w, h) = (img.width as usize, img.height as usize);
    let mut out = Image::new(img.width, img.height);
    // Every output pixel adds its taps in kernel order, in f64, as a per-pixel loop would: the
    // result is the same to the bit. Taps outside a transparent edge are skipped (adding `+0.0`
    // leaves a sum that started at `+0.0` unchanged).
    if vertical {
        // Row by row: each output row adds whole input rows (contiguous, vectorises), instead of
        // walking a column per pixel (a strided read per tap).
        out.data.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
            // 128 pixels at a time: the f64 sums (4 KB) stay in L1 across the taps.
            for (t, out_tile) in row.chunks_mut(TILE).enumerate() {
                let x0 = t * TILE;
                let mut acc = [[0.0f64; 4]; TILE];
                for (i, &wt) in k.iter().enumerate() {
                    let sy = y as i64 + i as i64 - r;
                    let sy = if repeat {
                        sy.clamp(0, h as i64 - 1)
                    } else if sy < 0 || sy >= h as i64 {
                        continue;
                    } else {
                        sy
                    };
                    let start = sy as usize * w + x0;
                    let Some(src) = img.data.get(start..start + out_tile.len()) else { continue };
                    for (a, p) in acc.iter_mut().zip(src) {
                        for c in 0..4 {
                            a[c] += p[c] as f64 * wt;
                        }
                    }
                }
                for (o, a) in out_tile.iter_mut().zip(acc.iter()) {
                    *o = a.map(|v| v as f32);
                }
            }
        });
        return out;
    }
    let n = k.len();
    out.data.par_chunks_mut(w).zip(img.data.par_chunks(w)).for_each(|(row, src)| {
        for (x, o) in row.iter_mut().enumerate() {
            let mut acc = [0.0f64; 4];
            // Inside the row: the taps are one slice (no per-tap edge checks).
            if let Some(taps) = x.checked_sub(r as usize).and_then(|x0| src.get(x0..x0 + n)) {
                for (p, wt) in taps.iter().zip(&k) {
                    for c in 0..4 {
                        acc[c] += p[c] as f64 * wt;
                    }
                }
            } else {
                for (i, wt) in k.iter().enumerate() {
                    let sx = x as i64 + i as i64 - r;
                    let at = if repeat { Some(sx.clamp(0, w as i64 - 1)) } else { Some(sx) };
                    let Some(p) = at.and_then(|i| usize::try_from(i).ok()).and_then(|i| src.get(i)) else { continue };
                    for c in 0..4 {
                        acc[c] += p[c] as f64 * wt;
                    }
                }
            }
            *o = acc.map(|v| v as f32);
        }
    });
    out
}

/// Gaussian blur as a true sampled kernel, continuous in sigma (for small sigmas, see
/// [`gaussian_is_exact`]).
pub fn gaussian_blur_exact(img: &Image, sigma_x: f64, sigma_y: f64, repeat: bool) -> Image {
    gauss_pass(&gauss_pass(img, sigma_x, false, repeat), sigma_y, true, repeat)
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
                // Samples outside the image repeat its edge: After Effects' Radial Blur keeps a
                // layer's edges (cases 713, 829) instead of fading them toward transparency.
                let p = img.sample_bilinear(sx.clamp(0.5, (img.width as f64 - 0.5).max(0.5)), sy.clamp(0.5, (img.height as f64 - 0.5).max(0.5)));
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

    /// The sampled Gaussian pass as it was (one pixel at a time, bounds-checked taps): the
    /// row-wise passes must match it to the bit.
    fn gauss_pass_per_pixel(img: &Image, sigma: f64, vertical: bool, repeat: bool) -> Image {
        let r = (sigma * 3.0).ceil() as i64;
        let mut k: Vec<f64> = (-r..=r).map(|i| (-((i * i) as f64) / (2.0 * sigma * sigma)).exp()).collect();
        let total: f64 = k.iter().sum();
        k.iter_mut().for_each(|v| *v /= total);
        let mut out = Image::new(img.width, img.height);
        for y in 0..img.height as i64 {
            for x in 0..img.width as i64 {
                let mut acc = [0.0f64; 4];
                for (i, wt) in k.iter().enumerate() {
                    let d = i as i64 - r;
                    let (sx, sy) = if vertical { (x, y + d) } else { (x + d, y) };
                    let p = if repeat { img.get_clamped(sx, sy) } else { img.get(sx, sy) };
                    for c in 0..4 {
                        acc[c] += p[c] as f64 * wt;
                    }
                }
                out.set(x as u32, y as u32, acc.map(|v| v as f32));
            }
        }
        out
    }

    #[test]
    fn gauss_pass_is_bit_identical_to_the_per_pixel_kernel() {
        // Sizes narrower and shorter than the kernel too (every tap near an edge), one wider
        // than three tiles (tiles past the first, and a partial last one), with negative, zero
        // and HDR values.
        for (w, h) in [(37, 23), (3, 2), (1, 9), (64, 1), (300, 5)] {
            let mut img = Image::new(w, h);
            for (i, p) in img.data.iter_mut().enumerate() {
                let v = ((i * 7919) % 101) as f32 / 37.0 - 0.6;
                *p = [v, -v * 0.5, if i % 5 == 0 { 0.0 } else { v * 3.0 }, (v + 0.6).min(1.0)];
            }
            for sigma in [0.4, 1.3, 2.9] {
                for vertical in [false, true] {
                    for repeat in [false, true] {
                        let (a, b) = (gauss_pass(&img, sigma, vertical, repeat), gauss_pass_per_pixel(&img, sigma, vertical, repeat));
                        let bits = |i: &Image| i.data.iter().flat_map(|p| p.map(f32::to_bits)).collect::<Vec<_>>();
                        assert!(bits(&a) == bits(&b), "{w}×{h} sigma {sigma} vertical {vertical} repeat {repeat}");
                    }
                }
            }
        }
    }

    #[test]
    fn a_non_finite_edge_pixel_outside_every_window_is_not_counted() {
        // 9 wide, radius 1: the last pixel is in no window of the first pixels; likewise the last row.
        let mut src = vec![[1.0f32; 4]; 81];
        src[80] = [f32::NAN; 4];
        let mut out = vec![[0.0f32; 4]; 81];
        box_h_into(&src, &mut out, 9, 1, true);
        assert!(out[0].iter().all(|v| v.is_finite()), "{:?}", out[0]);
        box_v_into(&src, &mut out, 9, 9, 1, true);
        assert!(out[0].iter().all(|v| v.is_finite()), "{:?}", out[0]);
    }

    #[test]
    fn box_windows_count_repeated_edges_and_take_any_radius() {
        // Against a window summed pixel by pixel, for radii inside and far past the image.
        let (w, h) = (7usize, 5usize);
        let src: Vec<Px> = (0..w * h).map(|i| [i as f32, (i % 3) as f32, 1.0, 0.5]).collect();
        let at = |x: isize, y: isize, repeat: bool| -> Px {
            if !repeat && (x < 0 || y < 0 || x >= w as isize || y >= h as isize) {
                return [0.0; 4];
            }
            src[y.clamp(0, h as isize - 1) as usize * w + x.clamp(0, w as isize - 1) as usize]
        };
        for repeat in [true, false] {
            for r in [1usize, 3, 6, 9, 40] {
                let (mut hx, mut vx) = (vec![[0.0; 4]; w * h], vec![[0.0; 4]; w * h]);
                box_h_into(&src, &mut hx, w, r, repeat);
                box_v_into(&src, &mut vx, w, h, r, repeat);
                for y in 0..h as isize {
                    for x in 0..w as isize {
                        let (mut eh, mut ev) = ([0.0f32; 4], [0.0f32; 4]);
                        for d in -(r as isize)..=r as isize {
                            for c in 0..4 {
                                eh[c] += at(x + d, y, repeat)[c];
                                ev[c] += at(x, y + d, repeat)[c];
                            }
                        }
                        let n = (2 * r + 1) as f32;
                        let i = y as usize * w + x as usize;
                        for c in 0..4 {
                            assert!((hx[i][c] - eh[c] / n).abs() < 1e-3, "h r={r} repeat={repeat}");
                            assert!((vx[i][c] - ev[c] / n).abs() < 1e-3, "v r={r} repeat={repeat}");
                        }
                    }
                }
            }
        }
        // A radius of 2^40 finishes at once.
        let mut out = vec![[0.0; 4]; w * h];
        box_h_into(&src, &mut out, w, 1 << 40, true);
        box_v_into(&src, &mut out.clone(), w, h, 1 << 40, true);
    }

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
    fn vanishing_or_nan_sigma_leaves_the_image_unchanged() {
        let mut img = Image::new(5, 5);
        img.set(2, 2, [1.0, 0.5, 0.25, 1.0]);
        for s in [1e-200, f64::MIN_POSITIVE, f64::NAN] {
            let b = gaussian_blur_exact(&img, s, s, false);
            assert!(b.data.iter().all(|p| p.iter().all(|v| v.is_finite())), "sigma {s}");
            assert_eq!(b, img, "sigma {s}");
        }
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
}

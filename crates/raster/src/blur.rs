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
///
/// The running column sums restart every `block` rows; `row_phase` places those restarts as if
/// the buffer had `row_phase` more transparent rows on top (blocks start where
/// `(y + row_phase) % block == 0`), so a buffer cropped on top by that many rows rounds exactly
/// like the taller one. Ignored with `repeat` (the edge rows would differ).
fn box_v_into(src: &[Px], out: &mut [Px], w: usize, h: usize, r: usize, repeat: bool, row_phase: i64) {
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
    let ph = if repeat { 0 } else { row_phase.rem_euclid(block as i64) as usize };
    let head = if ph == 0 { 0 } else { (block - ph).min(h) };
    let (first, rest) = out.split_at_mut(head * w);
    // (first row of the block, possibly above the buffer; its rows inside the buffer)
    let mut parts: Vec<(isize, &mut [Px])> = Vec::with_capacity(h / block + 2);
    if head > 0 {
        parts.push((-(ph as isize), first));
    }
    parts.extend(rest.chunks_mut(w * block).enumerate().map(|(bi, c)| ((head + bi * block) as isize, c)));
    parts.into_par_iter().for_each(|(ya, chunk)| {
        // Rows of the block above the buffer: the sums advance over them, nothing is written.
        let skip = (-ya).max(0) as usize;
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
        let rows = chunk.len() / w;
        for yy in 0..skip + rows {
            let y = ya + yy as isize;
            if yy >= skip {
                let o = &mut chunk[(yy - skip) * w..(yy - skip + 1) * w];
                for (o, a) in o.iter_mut().zip(&acc) {
                    *o = a.map(|v| v * norm);
                }
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
fn box_passes(img: &Image, rx: &[usize], ry: &[usize], repeat: bool, row_phase: i64) -> Image {
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
            box_v_into(src, &mut b.data, w, h, r, repeat, row_phase);
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
    box_blur_phased(img, rx, ry, iterations, repeat, 0)
}

/// [`box_blur`] with the vertical passes' block phase (see `box_v_into`).
pub fn box_blur_phased(img: &Image, rx: usize, ry: usize, iterations: usize, repeat: bool, row_phase: i64) -> Image {
    let n = iterations.max(1);
    box_passes(img, &vec![rx; n], &vec![ry; n], repeat, row_phase)
}

/// Gaussian blur with standard deviations in pixels (3 box passes per axis).
pub fn gaussian_blur(img: &Image, sigma_x: f64, sigma_y: f64, repeat: bool) -> Image {
    gaussian_blur_phased(img, sigma_x, sigma_y, repeat, 0)
}

/// [`gaussian_blur`] with the vertical passes' block phase (see `box_v_into`).
pub fn gaussian_blur_phased(img: &Image, sigma_x: f64, sigma_y: f64, repeat: bool, row_phase: i64) -> Image {
    let rx = if sigma_x > 0.05 { box_radii(sigma_x, 3) } else { vec![] };
    let ry = if sigma_y > 0.05 { box_radii(sigma_y, 3) } else { vec![] };
    box_passes(img, &rx, &ry, repeat, row_phase)
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

    /// A buffer missing `crop` transparent rows on top, blurred with that row phase, gives the
    /// taller buffer's pixels bit for bit (tight cover vs the blanket cover).
    #[test]
    fn row_phase_matches_the_taller_buffer() {
        let (w, h, crop) = (40usize, 400usize, 70usize);
        let mut tall = Image::new(w as u32, h as u32);
        for y in 150..250 {
            for x in 10..30 {
                let v = ((x * 7 + y * 13) % 17) as f32 / 17.0;
                tall.set(x as u32, y as u32, [v * 0.3, v * 0.7, v, 1.0]);
            }
        }
        let mut short = Image::new(w as u32, (h - crop) as u32);
        short.data.copy_from_slice(&tall.data[crop * w..]);
        for sigma in [3.0, 11.0, 20.0] {
            let a = gaussian_blur(&tall, sigma, sigma, false);
            let b = gaussian_blur_phased(&short, sigma, sigma, false, crop as i64);
            assert!(a.data[crop * w..].iter().zip(&b.data).all(|(p, q)| p.map(f32::to_bits) == q.map(f32::to_bits)), "sigma {sigma}");
            let c = box_blur_phased(&short, 9, 9, 3, false, crop as i64);
            let d = box_blur(&tall, 9, 9, 3, false);
            assert!(d.data[crop * w..].iter().zip(&c.data).all(|(p, q)| p.map(f32::to_bits) == q.map(f32::to_bits)));
        }
    }

    /// A transparent-edge blur on a crop of a larger buffer gives the larger buffer's pixels
    /// within the blur's reach of the content when the crop keeps a second reach of margin on
    /// the right and bottom (the running sums read ahead into their residue there).
    #[test]
    fn crop_with_read_ahead_margin_matches_the_larger_buffer() {
        let (w, h) = (700usize, 760usize);
        let mut big = Image::new(w as u32, h as u32);
        let (x0, x1, y0, y1) = (250usize, 330usize, 300usize, 380usize);
        for y in y0..y1 {
            for x in x0..x1 {
                let v = ((x * 7 + y * 13) % 17) as f32 / 17.0;
                big.set(x as u32, y as u32, [v * 0.3, v * 0.7, v, 1.0]);
            }
        }
        let (r, it) = (40usize, 3usize);
        let reach = r * it + 1;
        let (cx, cy) = (x0 - reach, y0 - reach);
        let (sw, sh) = (x1 + 2 * reach - cx, y1 + 2 * reach - cy);
        let mut crop = Image::new(sw as u32, sh as u32);
        for y in 0..sh {
            crop.data[y * sw..(y + 1) * sw].copy_from_slice(&big.data[(y + cy) * w + cx..(y + cy) * w + cx + sw]);
        }
        let a = box_blur(&big, r, r, it, true);
        let b = box_blur_phased(&crop, r, r, it, false, cy as i64);
        for y in 0..sh - reach {
            for x in 0..sw - reach {
                assert_eq!(a.data[(y + cy) * w + x + cx].map(f32::to_bits), b.data[y * sw + x].map(f32::to_bits), "{x},{y}");
            }
        }
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
                let fast = box_passes(&img, &[rx, rx + 1], &[ry, ry / 2], repeat, 0);
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

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

/// [`gaussian_blur`] with transparent edges, as if the image went on with transparency on
/// every side: what a box pass spreads past the edge still feeds the passes after it, where
/// [`gaussian_blur`] drops it. For blurs that reach past the buffer holding them (an effect's
/// padding cut to its pixel budget), which [`gaussian_blur`] leaves far too faint once the
/// boxes are wider than the buffer. A line costs O(length) whatever the sigma.
pub fn gaussian_blur_unbounded(img: &Image, sigma_x: f64, sigma_y: f64) -> Image {
    gaussian_blur_unbounded_at(img, sigma_x, sigma_y, [0, 0], img.width, img.height)
}

/// [`gaussian_blur_unbounded`] of `img` moved by `shift` pixels, seen through a `width` ×
/// `height` window at the origin: output `(x, y)` is the blurred plane at
/// `(x − shift[0], y − shift[1])`. Nothing is cut before the blur, so the tails of content
/// that lands outside the window still reach into it.
pub fn gaussian_blur_unbounded_at(img: &Image, sigma_x: f64, sigma_y: f64, shift: [i64; 2], width: u32, height: u32) -> Image {
    let (w, h, ow, oh) = (img.width as usize, img.height as usize, width as usize, height as usize);
    if w == 0 || h == 0 || ow == 0 || oh == 0 {
        return Image::new(width, height);
    }
    let (rx, ry) = (unbounded_radii(sigma_x).unwrap_or_default(), unbounded_radii(sigma_y).unwrap_or_default());
    // Horizontal: each source row into a row of the window's width.
    let mut rows = vec![[0.0f32; 4]; ow.saturating_mul(h)];
    rows.par_chunks_mut(ow).zip(img.data.par_chunks(w)).for_each(|(o, src)| boxes_unbounded(src, &rx, shift[0], o));
    // Vertical: each of those columns into a column of the window's height.
    let cols: Vec<Vec<Px>> = (0..ow)
        .into_par_iter()
        .map(|x| {
            let col: Vec<Px> = rows.iter().skip(x).step_by(ow).copied().collect();
            let mut o = vec![[0.0f32; 4]; oh];
            boxes_unbounded(&col, &ry, shift[1], &mut o);
            o
        })
        .collect();
    // At most three buffers at once: the rows go before the output comes.
    drop(rows);
    let mut out = Image::new(width, height);
    out.data.par_chunks_mut(ow).enumerate().for_each(|(y, row)| {
        for (o, col) in row.iter_mut().zip(&cols) {
            if let Some(v) = col.get(y) {
                *o = *v;
            }
        }
    });
    out
}

/// Above this sigma (pixels) [`gaussian_blur_unbounded`] blurs as if it were this: far wider
/// than any buffer ([`crate::MAX_PADDED_SIDE`]), and small enough for [`box_radii`]'s `i64`
/// arithmetic (which overflows near 9e8).
const UNBOUNDED_SIGMA_MAX: f64 = 1e7;

/// [`gaussian_blur`]'s box radii for `sigma` without the zero ones (each an identity), or
/// `None` where it doesn't blur (and for a sigma that isn't a number).
fn unbounded_radii(sigma: f64) -> Option<Vec<i64>> {
    if sigma.is_nan() || sigma <= 0.05 {
        return None;
    }
    let radii: Vec<i64> = box_radii(sigma.min(UNBOUNDED_SIGMA_MAX), 3).into_iter().filter_map(|r| i64::try_from(r).ok()).filter(|&r| r > 0).collect();
    (!radii.is_empty()).then_some(radii)
}

/// Transparent-edged box passes of radii `r` (up to three, none zero) over `line`, as if it
/// went on with zeros, moved by `shift`: `out[x]` is the blurred line at `x − shift`.
///
/// One box is a difference of prefix sums, `(S(x + r) − S(x − r − 1)) / (2r + 1)`, so `k` boxes
/// are `2^k` signed terms of the `k`-th prefix sum `Sk` (`S0` is the line). Before the line
/// every `Sk` is 0; past it, `j = m − n + 1` steps on with `T` the line's total, they are
/// polynomials: `S0(m) = 0`, `S1(m) = T`, `S2(m) = S2(n − 1) + j·T`,
/// `S3(m) = S3(n − 1) + j·S2(n − 1) + T·j(j + 1)/2`. The sums are `f64`. `Sk` grows like `n^k`,
/// so the cancelling terms leave a rounding error near `ε·n^k / (2r + 1)^k` of the values:
/// under 1e-8 for any line a blur too wide for its buffer runs on.
fn boxes_unbounded(line: &[Px], r: &[i64], shift: i64, out: &mut [Px]) {
    let (n, k) = (line.len(), r.len());
    let Ok(n_i) = i64::try_from(n) else { return };
    if n == 0 || k > 3 {
        return;
    }
    // sums[o] = the (o + 1)-th prefix sum so far.
    let mut sums = [[0.0f64; 4]; 3];
    let mut sk = Vec::with_capacity(n);
    for p in line {
        let v = p.map(f64::from);
        for c in 0..4 {
            sums[0][c] += v[c];
            sums[1][c] += sums[0][c];
            sums[2][c] += sums[1][c];
        }
        sk.push(match k {
            0 => v,
            _ => sums[k - 1],
        });
    }
    let at = |m: i64| -> [f64; 4] {
        if m < 0 {
            return [0.0; 4];
        }
        if let Some(v) = usize::try_from(m).ok().and_then(|i| sk.get(i)) {
            return *v;
        }
        let j = (m - n_i + 1) as f64;
        std::array::from_fn(|c| match k {
            0 => 0.0,
            1 => sums[0][c],
            2 => sums[1][c] + j * sums[0][c],
            _ => sums[2][c] + j * sums[1][c] + sums[0][c] * j * (j + 1.0) / 2.0,
        })
    };
    // The offsets and signs: each box adds `S(x + r)` and subtracts `S(x − r − 1)`.
    let terms: Vec<(i64, f64)> = (0..1usize << k)
        .map(|bits| {
            r.iter().enumerate().fold((0i64.saturating_sub(shift), 1.0f64), |(at, sign), (b, &ri)| {
                if bits >> b & 1 == 0 { (at.saturating_add(ri), sign) } else { (at.saturating_sub(ri.saturating_add(1)), -sign) }
            })
        })
        .collect();
    let norm: f64 = r.iter().map(|&ri| 2.0 * ri as f64 + 1.0).product();
    // The boxes read the line within `reach` of `x − shift`. Outside that the result is 0, and
    // the polynomials far past the line would only cancel to their rounding error.
    let reach = r.iter().fold(0i64, |s, &ri| s.saturating_add(ri));
    for (x, o) in (0i64..).zip(out.iter_mut()) {
        let c = x.saturating_sub(shift);
        if c.saturating_add(reach) < 0 || c.saturating_sub(reach) >= n_i {
            *o = [0.0; 4];
            continue;
        }
        let mut v = [0.0f64; 4];
        for &(offset, sign) in &terms {
            let s = at(x.saturating_add(offset));
            for c in 0..4 {
                v[c] += sign * s[c];
            }
        }
        *o = v.map(|v| (v / norm) as f32);
    }
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

/// Samples [`radial_blur`] averages per pixel at every amount, so the work per pixel stays the
/// same up to Radial Blur's largest Amount (1000). The GPU path takes the same count.
pub const RADIAL_BLUR_SAMPLES: usize = 32;

/// Where sample `i` of [`RADIAL_BLUR_SAMPLES`] lies in the blur's spread, from −0.5 to 0.5.
fn radial_blur_t(i: usize) -> f64 {
    i as f64 / (RADIAL_BLUR_SAMPLES - 1) as f64 - 0.5
}

/// Spin sample `i`'s rotation in degrees for a spin of `amount` degrees.
pub fn radial_blur_spin_degrees(i: usize, amount: f64) -> f64 {
    radial_blur_t(i) * amount
}

/// Zoom sample `i`'s scale about the centre for a zoom of `amount` (a fraction of the distance
/// to the centre): evenly over `1 ± amount / 2`. Past a spread of 2 (Radial Blur's Amount 200)
/// that would reach below 0 and sample the point mirrored through the centre, so the samples
/// spread evenly from the centre (0) to `1 + |amount| / 2` instead (#3467). That wider shape is
/// EffectCraft's; After Effects has not been measured past Amount 200.
pub fn radial_blur_zoom_scale(i: usize, amount: f64) -> f64 {
    let t = radial_blur_t(i);
    if amount.abs() <= 2.0 {
        return 1.0 + t * amount;
    }
    (t + 0.5) * (1.0 + amount.abs() / 2.0)
}

/// Radial blur around `center`: `spin` (degrees) or zoom (`amount` as a fraction of distance).
pub fn radial_blur(img: &Image, center: (f64, f64), amount: f64, zoom: bool) -> Image {
    if amount.abs() < 1e-6 {
        return img.clone();
    }
    let n = RADIAL_BLUR_SAMPLES;
    let mut out = Image::new(img.width, img.height);
    out.rows_mut().for_each(|(y, row)| {
        for (x, px) in row.iter_mut().enumerate() {
            let (vx, vy) = (x as f64 + 0.5 - center.0, y as f64 + 0.5 - center.1);
            let mut acc = [0.0f32; 4];
            for i in 0..n {
                let (sx, sy) = if zoom {
                    let k = radial_blur_zoom_scale(i, amount);
                    (center.0 + vx * k, center.1 + vy * k)
                } else {
                    let (s, c) = radial_blur_spin_degrees(i, amount).to_radians().sin_cos();
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

    /// Zoom spreads up to 2 keep `1 ± spread / 2` exactly as before (#3467); wider ones run
    /// from the centre (scale 0) out to `1 + spread / 2`, evenly and never below 0.
    #[test]
    fn radial_blur_zoom_scales_never_cross_the_centre() {
        let n = RADIAL_BLUR_SAMPLES;
        for amount in [0.2, 1.0, 2.0, -1.5] {
            for i in 0..n {
                let t = i as f64 / (n - 1) as f64 - 0.5;
                assert_eq!(radial_blur_zoom_scale(i, amount), 1.0 + t * amount, "amount {amount} sample {i}");
            }
        }
        for amount in [2.5, 10.0, -10.0] {
            let k: Vec<f64> = (0..n).map(|i| radial_blur_zoom_scale(i, amount)).collect();
            assert_eq!(k[0], 0.0, "amount {amount}");
            assert!((k[n - 1] - (1.0 + amount.abs() / 2.0)).abs() < 1e-12, "amount {amount}: {k:?}");
            let step = k[n - 1] / (n - 1) as f64;
            assert!(k.windows(2).all(|w| (w[1] - w[0] - step).abs() < 1e-12), "amount {amount}: {k:?}");
        }
    }

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

    /// #3816: a sigma past what `i64` holds (from an expression or a file value) gives the
    /// radii of the largest sigma that blurs, not an overflow; NaN doesn't blur.
    #[test]
    fn huge_sigma_radii_are_capped() {
        let cap = box_radii(1e8, 3);
        for sigma in [1e9, 1e300, f64::MAX, f64::INFINITY] {
            assert_eq!(box_radii(sigma, 3), cap, "sigma {sigma}");
        }
        assert_eq!(box_radii(f64::NAN, 3), vec![0; 3]);
        let mut img = Image::new(5, 5);
        img.set(2, 2, [1.0; 4]);
        for sigma in [1e9, f64::INFINITY] {
            for repeat in [false, true] {
                let b = gaussian_blur(&img, sigma, sigma, repeat);
                assert!(b.data.iter().flatten().all(|v| v.is_finite()), "sigma {sigma} repeat {repeat}");
            }
        }
    }

    /// #3464: moving the image before [`gaussian_blur_unbounded`] and blurring it moved with
    /// [`gaussian_blur_unbounded_at`] agree, also when the move puts all of it outside the window
    /// (its tails still reach in) and along an axis that isn't blurred.
    #[test]
    fn unbounded_gaussian_at_a_shift_matches_blurring_the_moved_image() {
        let mut img = Image::new(6, 5);
        for (i, p) in img.data.iter_mut().enumerate() {
            let v = ((i * 37) % 11) as f32 / 11.0;
            *p = [v, 0.5 * v, 1.0 - v, 1.0];
        }
        let (ow, oh) = (14u32, 9u32);
        for (sx, sy, shift) in [(3.0, 2.0, [4i64, 2i64]), (5.0, 0.0, [-3, 1]), (12.0, 12.0, [-20, 16]), (8.0, 8.0, [25, -12])] {
            let got = gaussian_blur_unbounded_at(&img, sx, sy, shift, ow, oh);
            // The moved image in a canvas large enough to hold it and the window, blurred.
            let off = [40i64, 40i64];
            let mut canvas = Image::new(100, 100);
            for y in 0..img.height {
                for x in 0..img.width {
                    let (cx, cy) = (i64::from(x) + shift[0] + off[0], i64::from(y) + shift[1] + off[1]);
                    canvas.set(cx as u32, cy as u32, img.get(i64::from(x), i64::from(y)));
                }
            }
            let want = gaussian_blur_unbounded(&canvas, sx, sy).crop(off[0], off[1], ow, oh);
            assert!(got.data.iter().flatten().any(|v| *v > 1e-4), "{shift:?}: something reaches the window");
            for (a, b) in got.data.iter().flatten().zip(want.data.iter().flatten()) {
                assert!((a - b).abs() <= 1e-5, "σ ({sx}, {sy}) {shift:?}: {a} vs {b}");
            }
        }
    }

    /// #3464: a window the blur doesn't reach stays exactly transparent, however far the move
    /// (far past the line, the cancelling polynomials left a rounding residue of about 5).
    #[test]
    fn unbounded_gaussian_at_a_far_shift_is_exactly_transparent() {
        let img = Image::filled(1, 1, [1.0, 1.0, 1.0, 1.0]);
        for shift in [[-1_000_000_030i64, 0i64], [0, 1_000_000_030], [i64::MIN, i64::MAX]] {
            let out = gaussian_blur_unbounded_at(&img, 1.5, 1.5, shift, 4, 3);
            assert!(out.data.iter().flatten().all(|v| *v == 0.0), "{shift:?}");
        }
        // Next to the window the tail still reaches in.
        assert!(gaussian_blur_unbounded_at(&img, 1.5, 1.5, [-1, 0], 4, 3).get(0, 0)[3] > 0.0);
    }

    /// #3464: a sigma past any buffer, or infinite, blurs as the widest one does: finite, no
    /// overflow in the radii. A NaN sigma leaves the axis alone.
    #[test]
    fn unbounded_gaussian_takes_any_sigma() {
        let mut img = Image::new(9, 7);
        img.set(4, 3, [1.0, 0.5, 0.25, 1.0]);
        let wide = gaussian_blur_unbounded(&img, 1e7, 1e7);
        for (sx, sy) in [(f64::INFINITY, f64::INFINITY), (1e300, 1e12)] {
            assert_eq!(gaussian_blur_unbounded(&img, sx, sy).data, wide.data, "σ ({sx}, {sy})");
        }
        assert!(wide.data.iter().flatten().all(|v| v.is_finite()));
        assert_eq!(gaussian_blur_unbounded(&img, f64::NAN, f64::NAN).data, img.data);
    }

    /// #3464: [`gaussian_blur_unbounded`] is [`gaussian_blur`] on the image padded with enough
    /// transparency to hold every pass, cropped back: what a pass spreads past the edge comes
    /// back in the passes after it. Sigmas far wider than the image too, along each axis.
    #[test]
    fn unbounded_gaussian_matches_a_buffer_padded_past_its_reach() {
        let pattern = |w: u32, h: u32| {
            let mut img = Image::new(w, h);
            for (i, p) in img.data.iter_mut().enumerate() {
                let v = ((i * 7919) % 101) as f32 / 101.0;
                *p = [v, v * 0.5, 1.0 - v, (v + 0.3).min(1.0)];
            }
            img
        };
        let cases = [
            (pattern(23, 17), 4.0f64, 0.0f64, 1e-4),
            (pattern(23, 17), 0.0, 6.0, 1e-4),
            (pattern(23, 17), 7.5, 3.2, 1e-4),
            (pattern(23, 17), 60.0, 45.0, 1e-4),
            // Zero radii (an identity) and one non-zero box among them.
            (pattern(23, 17), 0.5, 0.0, 1e-6),
            (pattern(23, 17), 0.0, 1.1, 1e-5),
            // The reference's f32 running sums drift over its 90 000-pixel line (by about 1.5e-4
            // of the value).
            (pattern(40, 1), 15000.0, 0.0, 1e-3),
            (pattern(1, 40), 0.0, 15000.0, 1e-3),
        ];
        for (img, sx, sy, tol) in cases {
            // Three boxes of radius about σ each reach at most 3σ + 2. Padded by hand, along
            // the blurred axes only (`Image::padded` pads both and caps a side at 30000).
            let reach = |s: f64| if s > 0.0 { (s * 3.0 + 4.0).ceil() as u32 } else { 0 };
            let (px, py) = (reach(sx), reach(sy));
            let mut big = Image::new(img.width + 2 * px, img.height + 2 * py);
            for y in 0..img.height {
                for x in 0..img.width {
                    big.set(x + px, y + py, img.get(x as i64, y as i64));
                }
            }
            let want = gaussian_blur(&big, sx, sy, false).crop(px as i64, py as i64, img.width, img.height);
            let got = gaussian_blur_unbounded(&img, sx, sy);
            let peak = want.data.iter().flatten().fold(0.0f32, |m, v| m.max(v.abs()));
            for (a, b) in got.data.iter().flatten().zip(want.data.iter().flatten()) {
                assert!((a - b).abs() <= tol * peak.max(1e-6), "σ ({sx}, {sy}) on {}×{}: {a} vs {b} (peak {peak})", img.width, img.height);
            }
        }
    }
}

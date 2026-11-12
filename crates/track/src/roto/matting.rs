//! Matte utilities: exact Euclidean distance transforms (Felzenszwalb & Huttenlocher,
//! "Distance Transforms of Sampled Functions", Theory of Computing 8, 2012), box and Gaussian
//! filters, the colour **guided filter** (K. He, J. Sun and X. Tang, "Guided Image Filtering",
//! ECCV 2010 / IEEE PAMI 2013) used for edge matting, foreground colour estimation
//! (decontamination, from the compositing equation I = αF + (1 − α)B with B and F estimated from
//! nearby pure background / foreground pixels), and the Refine Edge matte adjustments.

use rayon::prelude::*;

const BIG: f32 = 1e12;

/// 1-D squared distance transform of `f` (lower envelope of parabolas).
fn dt1(f: &[f32], d: &mut [f32], v: &mut [usize], z: &mut [f64]) {
    let n = f.len();
    if n == 0 {
        return;
    }
    let ff = |q: usize| f[q] as f64 + (q * q) as f64;
    let mut k = 0usize;
    v[0] = 0;
    z[0] = f64::NEG_INFINITY;
    z[1] = f64::INFINITY;
    for q in 1..n {
        let mut s = (ff(q) - ff(v[k])) / (2.0 * (q as f64 - v[k] as f64));
        while s <= z[k] {
            if k == 0 {
                break;
            }
            k -= 1;
            s = (ff(q) - ff(v[k])) / (2.0 * (q as f64 - v[k] as f64));
        }
        k += 1;
        v[k] = q;
        z[k] = s;
        z[k + 1] = f64::INFINITY;
    }
    k = 0;
    for (q, dq) in d.iter_mut().enumerate().take(n) {
        while z[k + 1] < q as f64 {
            k += 1;
        }
        let p = v[k];
        let dd = q as f64 - p as f64;
        *dq = (dd * dd + f[p] as f64) as f32;
    }
}

/// Scratch vectors (`f`, `v`, `z`) for `dt1` over a line of `n` samples; sized per pass so a
/// 1 x N image does not allocate N-sized buffers for each of its N one-pixel rows.
fn line_scratch(n: usize) -> (Vec<f32>, Vec<usize>, Vec<f64>) {
    (vec![0.0; n], vec![0; n], vec![0.0; n + 1])
}

/// Euclidean distance from every pixel to the nearest pixel where `seed` is true (BIG.sqrt()
/// when there is none).
pub fn distance(seed: &[bool], w: usize, h: usize) -> Vec<f32> {
    if w == 0 || h == 0 {
        return vec![];
    }
    // dt1 needs its input intact while it writes the output, so each line goes through scratch
    // (`f`, `v`, `z`) that rayon creates once per work split, not once per line. The column pass
    // runs on a transposed copy so that every column is a contiguous chunk.
    // Columns: `t[x * h + y]` is pixel (x, y).
    let mut t = vec![0.0f32; w * h];
    t.par_chunks_mut(h).enumerate().for_each_init(
        || line_scratch(h),
        |(f, v, z), (x, col)| {
            for (y, fv) in f.iter_mut().enumerate() {
                *fv = if seed.get(y * w + x).copied().unwrap_or(false) { 0.0 } else { BIG };
            }
            dt1(f, col, v, z);
        },
    );
    // Rows, then the square root.
    let mut g = vec![0.0f32; w * h];
    g.par_chunks_mut(w).enumerate().for_each_init(
        || line_scratch(w),
        |(f, v, z), (y, row)| {
            for (x, fv) in f.iter_mut().enumerate() {
                *fv = t[x * h + y];
            }
            dt1(f, row, v, z);
            for d in row.iter_mut() {
                *d = d.sqrt();
            }
        },
    );
    g
}

/// Signed distance to a binary matte's edge: positive inside (distance to the nearest
/// background pixel centre, minus ½), negative outside.
pub fn signed_distance(m: &[u8], w: usize, h: usize) -> Vec<f32> {
    let fg: Vec<bool> = m.iter().map(|v| *v != 0).collect();
    let bg: Vec<bool> = m.iter().map(|v| *v == 0).collect();
    let din = distance(&bg, w, h);
    let dout = distance(&fg, w, h);
    m.iter().enumerate().map(|(i, v)| if *v != 0 { din[i] - 0.5 } else { 0.5 - dout[i] }).collect()
}

/// Mean over the (2r+1)² window (clamped at the edges: averages only the pixels inside).
pub fn box_mean(src: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    if w == 0 || h == 0 {
        return vec![];
    }
    // Horizontal window sums per row.
    let mut tmp = vec![0.0f64; w * h];
    tmp.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let s = &src[y * w..y * w + w];
        let mut acc = 0.0f64;
        for x in 0..r.min(w) {
            acc += s[x] as f64;
        }
        for (x, o) in row.iter_mut().enumerate() {
            if x + r < w {
                acc += s[x + r] as f64;
            }
            if x > r {
                acc -= s[x - r - 1] as f64;
            }
            *o = acc;
        }
    });
    // Vertical prefix sums (row by row, cache friendly), then window differences.
    let mut pre = vec![0.0f64; w * (h + 1)];
    for y in 0..h {
        let (done, rest) = pre.split_at_mut((y + 1) * w);
        let prev = &done[y * w..];
        let cur = &mut rest[..w];
        let t = &tmp[y * w..y * w + w];
        for x in 0..w {
            cur[x] = prev[x] + t[x];
        }
    }
    let mut out = vec![0.0f32; w * h];
    out.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let a = y.saturating_sub(r);
        let b = (y + r + 1).min(h);
        let cy = (b - a) as f64;
        for (x, o) in row.iter_mut().enumerate() {
            let cx = ((x + r + 1).min(w) - x.saturating_sub(r)) as f64;
            *o = ((pre[b * w + x] - pre[a * w + x]) / (cx * cy)) as f32;
        }
    });
    out
}

/// Box radius of [`gauss`]: three passes of it match the variance, 3·((2r+1)² − 1)/12 = σ².
/// `None` when the blur is a copy.
fn gauss_box(sigma: f64) -> Option<usize> {
    (sigma >= 0.3).then(|| (((4.0 * sigma * sigma + 1.0).sqrt() - 1.0) / 2.0).round().max(1.0) as usize)
}

/// How far (pixels, per axis) [`gauss`] reaches: an output pixel depends only on the input
/// within this distance.
pub fn gauss_reach(sigma: f64) -> usize {
    gauss_box(sigma).map_or(0, |r| 3 * r)
}

/// Approximate Gaussian blur (three box passes) with standard deviation `sigma` pixels.
pub fn gauss(src: &[f32], w: usize, h: usize, sigma: f64) -> Vec<f32> {
    let Some(r) = gauss_box(sigma) else { return src.to_vec() };
    let a = box_mean(src, w, h, r);
    let b = box_mean(&a, w, h, r);
    box_mean(&b, w, h, r)
}

fn inv3(m: [[f32; 3]; 3]) -> [[f32; 3]; 3] {
    let d = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1]) - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
    let d = if d.abs() < 1e-20 { 1e-20 } else { d };
    [
        [(m[1][1] * m[2][2] - m[1][2] * m[2][1]) / d, (m[0][2] * m[2][1] - m[0][1] * m[2][2]) / d, (m[0][1] * m[1][2] - m[0][2] * m[1][1]) / d],
        [(m[1][2] * m[2][0] - m[1][0] * m[2][2]) / d, (m[0][0] * m[2][2] - m[0][2] * m[2][0]) / d, (m[0][2] * m[1][0] - m[0][0] * m[1][2]) / d],
        [(m[1][0] * m[2][1] - m[1][1] * m[2][0]) / d, (m[0][1] * m[2][0] - m[0][0] * m[2][1]) / d, (m[0][0] * m[1][1] - m[0][1] * m[1][0]) / d],
    ]
}

/// The colour guided filter of `p` with guide `img` (RGB, w × h), window radius `r`,
/// regularisation `eps` (He et al. 2010, section 3.5).
pub fn guided_filter(img: &[[f32; 3]], p: &[f32], w: usize, h: usize, r: usize, eps: f32) -> Vec<f32> {
    let n = w * h;
    let ch = |c: usize| -> Vec<f32> { img.iter().map(|v| v[c]).collect() };
    let (ir, ig, ib) = (ch(0), ch(1), ch(2));
    let mul = |a: &[f32], b: &[f32]| -> Vec<f32> { a.iter().zip(b).map(|(x, y)| x * y).collect() };
    let bm = |v: &[f32]| box_mean(v, w, h, r);
    let (mr, mg, mb, mp) = (bm(&ir), bm(&ig), bm(&ib), bm(p));
    let (mrp, mgp, mbp) = (bm(&mul(&ir, p)), bm(&mul(&ig, p)), bm(&mul(&ib, p)));
    let (vrr, vrg, vrb) = (bm(&mul(&ir, &ir)), bm(&mul(&ir, &ig)), bm(&mul(&ir, &ib)));
    let (vgg, vgb, vbb) = (bm(&mul(&ig, &ig)), bm(&mul(&ig, &ib)), bm(&mul(&ib, &ib)));
    let mut a = vec![[0.0f32; 3]; n];
    let mut b = vec![0.0f32; n];
    a.par_iter_mut().zip(b.par_iter_mut()).enumerate().for_each(|(i, (ai, bi))| {
        let cov = [mrp[i] - mr[i] * mp[i], mgp[i] - mg[i] * mp[i], mbp[i] - mb[i] * mp[i]];
        let s = [
            [vrr[i] - mr[i] * mr[i] + eps, vrg[i] - mr[i] * mg[i], vrb[i] - mr[i] * mb[i]],
            [vrg[i] - mr[i] * mg[i], vgg[i] - mg[i] * mg[i] + eps, vgb[i] - mg[i] * mb[i]],
            [vrb[i] - mr[i] * mb[i], vgb[i] - mg[i] * mb[i], vbb[i] - mb[i] * mb[i] + eps],
        ];
        let inv = inv3(s);
        let av = [
            cov[0] * inv[0][0] + cov[1] * inv[1][0] + cov[2] * inv[2][0],
            cov[0] * inv[0][1] + cov[1] * inv[1][1] + cov[2] * inv[2][1],
            cov[0] * inv[0][2] + cov[1] * inv[1][2] + cov[2] * inv[2][2],
        ];
        *ai = av;
        *bi = mp[i] - av[0] * mr[i] - av[1] * mg[i] - av[2] * mb[i];
    });
    let (a0, a1, a2): (Vec<f32>, Vec<f32>, Vec<f32>) = (a.iter().map(|v| v[0]).collect(), a.iter().map(|v| v[1]).collect(), a.iter().map(|v| v[2]).collect());
    let (ma0, ma1, ma2, mbb) = (bm(&a0), bm(&a1), bm(&a2), bm(&b));
    (0..n).into_par_iter().map(|i| ma0[i] * ir[i] + ma1[i] * ig[i] + ma2[i] * ib[i] + mbb[i]).collect()
}

/// Closed-form matting (A. Levin, D. Lischinski and Y. Weiss, "A Closed-Form Solution to Natural
/// Image Matting", IEEE PAMI 30(2), 2008): the alpha that minimises αᵀLα with the matting
/// Laplacian `L` of the colour image, with `known` pixels fixed to `init`. Solved by conjugate
/// gradients over the unknown pixels, starting from `init`; `L·x` is evaluated with box filters
/// (K. He, J. Sun and X. Tang, "Fast Matting Using Large Kernel Matting Laplacian Matrices",
/// CVPR 2010): `(L·x)ᵢ = Σ_{k∋i} (xᵢ − a_kᵀIᵢ − b_k)` with `(a_k, b_k)` the ridge-regression
/// fit of `x` to the colours in window k.
pub fn closed_form(img: &[[f32; 3]], init: &[f32], known: &[bool], size: (usize, usize), r: usize, eps: f32, iters: usize) -> Vec<f32> {
    let (w, h) = size;
    let n = w * h;
    if n == 0 || known.iter().all(|k| *k) {
        return init.to_vec();
    }
    let ch = |c: usize| -> Vec<f32> { img.iter().map(|v| v[c]).collect() };
    let ic = [ch(0), ch(1), ch(2)];
    let bm = |v: &[f32]| box_mean(v, w, h, r);
    let mu = [bm(&ic[0]), bm(&ic[1]), bm(&ic[2])];
    let prod = |a: usize, b: usize| -> Vec<f32> { ic[a].iter().zip(&ic[b]).map(|(x, y)| x * y).collect() };
    let mut cov = [[vec![], vec![], vec![]], [vec![], vec![], vec![]], [vec![], vec![], vec![]]];
    for a in 0..3 {
        for b in a..3 {
            let m = bm(&prod(a, b));
            cov[a][b] = m.clone();
            cov[b][a] = m;
        }
    }
    let inv: Vec<[[f32; 3]; 3]> = (0..n)
        .into_par_iter()
        .map(|i| {
            let mut s = [[0.0f32; 3]; 3];
            for a in 0..3 {
                for b in 0..3 {
                    s[a][b] = cov[a][b][i] - mu[a][i] * mu[b][i] + if a == b { eps } else { 0.0 };
                }
            }
            inv3(s)
        })
        .collect();
    // Pixels per window (= windows per pixel).
    let cnt: Vec<f32> = {
        let (rr, mut out) = (r as i64, vec![0.0f32; n]);
        for (i, o) in out.iter_mut().enumerate() {
            let (x, y) = ((i % w) as i64, (i / w) as i64);
            let cx = (x + rr).min(w as i64 - 1) - (x - rr).max(0) + 1;
            let cy = (y + rr).min(h as i64 - 1) - (y - rr).max(0) + 1;
            *o = (cx * cy) as f32;
        }
        out
    };
    let lap = |x: &[f32]| -> Vec<f32> {
        let pm = bm(x);
        let ipm: Vec<Vec<f32>> = (0..3).map(|c| bm(&ic[c].iter().zip(x).map(|(a, b)| a * b).collect::<Vec<_>>())).collect();
        let ab: Vec<[f32; 4]> = (0..n)
            .into_par_iter()
            .map(|i| {
                let cv = [ipm[0][i] - mu[0][i] * pm[i], ipm[1][i] - mu[1][i] * pm[i], ipm[2][i] - mu[2][i] * pm[i]];
                let m = &inv[i];
                let a = [
                    m[0][0] * cv[0] + m[0][1] * cv[1] + m[0][2] * cv[2],
                    m[1][0] * cv[0] + m[1][1] * cv[1] + m[1][2] * cv[2],
                    m[2][0] * cv[0] + m[2][1] * cv[1] + m[2][2] * cv[2],
                ];
                [a[0], a[1], a[2], pm[i] - a[0] * mu[0][i] - a[1] * mu[1][i] - a[2] * mu[2][i]]
            })
            .collect();
        let sa: Vec<Vec<f32>> = (0..3).map(|c| bm(&ab.iter().map(|v| v[c]).collect::<Vec<_>>())).collect();
        let sb = bm(&ab.iter().map(|v| v[3]).collect::<Vec<_>>());
        (0..n).into_par_iter().map(|i| cnt[i] * (x[i] - sa[0][i] * ic[0][i] - sa[1][i] * ic[1][i] - sa[2][i] * ic[2][i] - sb[i])).collect()
    };
    // L_UU x = −(L α_K)_U.
    let a0: Vec<f32> = (0..n).map(|i| if known[i] { init[i] } else { 0.0 }).collect();
    let la0 = lap(&a0);
    let mut x: Vec<f32> = (0..n).map(|i| if known[i] { 0.0 } else { init[i] }).collect();
    let lx = lap(&x);
    let mut res: Vec<f64> = (0..n).map(|i| if known[i] { 0.0 } else { (-la0[i] - lx[i]) as f64 }).collect();
    let mut p = res.clone();
    let mut rs: f64 = res.iter().map(|v| v * v).sum();
    let rs0 = rs.max(1e-30);
    for _ in 0..iters {
        if rs <= rs0 * 1e-8 {
            break;
        }
        let pf: Vec<f32> = p.iter().map(|v| *v as f32).collect();
        let ap: Vec<f64> = lap(&pf).iter().enumerate().map(|(i, v)| if known[i] { 0.0 } else { *v as f64 }).collect();
        let pap: f64 = p.iter().zip(&ap).map(|(a, b)| a * b).sum();
        if pap <= 1e-30 {
            break;
        }
        let alpha = rs / pap;
        for i in 0..n {
            x[i] += (alpha * p[i]) as f32;
            res[i] -= alpha * ap[i];
        }
        let rs_new: f64 = res.iter().map(|v| v * v).sum();
        let beta = rs_new / rs;
        rs = rs_new;
        for i in 0..n {
            p[i] = res[i] + beta * p[i];
        }
    }
    (0..n).map(|i| if known[i] { init[i] } else { x[i].clamp(0.0, 1.0) }).collect()
}

/// Contrast (0–100 %) around ½ on a soft alpha.
pub fn contrast(a: &mut [f32], pc: f64) {
    if pc <= 0.0 {
        return;
    }
    let k = 1.0 / (1.0 - 0.99 * (pc / 100.0).clamp(0.0, 1.0)) as f32;
    a.par_iter_mut().for_each(|v| *v = (0.5 + (*v - 0.5) * k).clamp(0.0, 1.0));
}

/// Shift Edge (−100–100 %) on a soft alpha: positive values grow the matte (a gamma curve on α).
pub fn shift_soft(a: &mut [f32], pc: f64) {
    if pc.abs() < 1e-9 {
        return;
    }
    let g = 2f32.powf((-pc / 50.0) as f32);
    a.par_iter_mut().for_each(|v| *v = v.clamp(0.0, 1.0).powf(g));
}

/// Estimate the pure foreground colour of every pixel listed in `region` (alpha strictly
/// between 0 and 1) and mix it in by `amount` (0–1). `radius`: neighbourhood (pixels) that pure
/// background / foreground estimates are taken from. Returns the decontamination map (how much
/// each pixel's colour changed, 0–1).
pub fn decontaminate(img: &mut [[f32; 3]], alpha: &[f32], region: &[bool], w: usize, h: usize, radius: usize, amount: f32) -> Vec<f32> {
    let n = w * h;
    let mut map = vec![0.0f32; n];
    if !region.iter().any(|r| *r) || amount <= 0.0 {
        return map;
    }
    let lo: Vec<f32> = alpha.iter().map(|a| if *a < 0.02 { 1.0 } else { 0.0 }).collect();
    let hi: Vec<f32> = alpha.iter().map(|a| if *a > 0.98 { 1.0 } else { 0.0 }).collect();
    // Windowed means of pure background / foreground colours, widening where none is near.
    let est = |m: &[f32]| -> Vec<Option<[f32; 3]>> {
        let mut out: Vec<Option<[f32; 3]>> = vec![None; n];
        let mut r = radius.max(2);
        for _ in 0..4 {
            let cnt = box_mean(m, w, h, r);
            let sums: Vec<Vec<f32>> = (0..3).map(|c| box_mean(&img.iter().zip(m).map(|(p, k)| p[c] * k).collect::<Vec<_>>(), w, h, r)).collect();
            let mut missing = false;
            for i in 0..n {
                if region[i] && out[i].is_none() {
                    if cnt[i] > 1e-4 {
                        out[i] = Some([sums[0][i] / cnt[i], sums[1][i] / cnt[i], sums[2][i] / cnt[i]]);
                    } else {
                        missing = true;
                    }
                }
            }
            if !missing {
                break;
            }
            r *= 2;
        }
        out
    };
    let bgc = est(&lo);
    let fgc = est(&hi);
    for i in 0..n {
        if !region[i] {
            continue;
        }
        let a = alpha[i].clamp(0.0, 1.0);
        if a <= 0.0 || a >= 1.0 {
            continue;
        }
        let c = img[i];
        let f = match (bgc[i], fgc[i]) {
            (Some(b), fbar) => {
                let mut f = [0.0f32; 3];
                for k in 0..3 {
                    f[k] = ((c[k] - (1.0 - a) * b[k]) / a).clamp(0.0, 1.0);
                }
                // Low alphas amplify noise: lean on the nearby foreground colour there.
                if let Some(fb) = fbar {
                    let t = ((0.25 - a) / 0.25).clamp(0.0, 1.0);
                    for k in 0..3 {
                        f[k] += (fb[k] - f[k]) * t;
                    }
                }
                f
            }
            (None, Some(fb)) => fb,
            (None, None) => c,
        };
        let mut d = 0.0f32;
        for k in 0..3 {
            let v = c[k] + (f[k] - c[k]) * amount;
            d = d.max((v - c[k]).abs());
            img[i][k] = v;
        }
        map[i] = d.min(1.0);
    }
    map
}

/// Sample a float plane bilinearly at continuous coordinates (pixel centres at +0.5), clamped.
pub fn sample(p: &[f32], w: usize, h: usize, x: f64, y: f64) -> f32 {
    if w == 0 || h == 0 {
        return 0.0;
    }
    let fx = (x - 0.5).clamp(0.0, (w - 1) as f64);
    let fy = (y - 0.5).clamp(0.0, (h - 1) as f64);
    let (x0, y0) = (fx.floor() as usize, fy.floor() as usize);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let (tx, ty) = ((fx - x0 as f64) as f32, (fy - y0 as f64) as f32);
    let g = |x: usize, y: usize| p[y * w + x];
    let top = g(x0, y0) + (g(x1, y0) - g(x0, y0)) * tx;
    let bot = g(x0, y1) + (g(x1, y1) - g(x0, y1)) * tx;
    top + (bot - top) * ty
}

/// Average of `a` sampled along the segment `[-v/2, v/2]` with `samples` taps (motion blur of a
/// matte moving by `v` pixels during the shutter).
pub fn motion_blur(a: &[f32], w: usize, h: usize, v: [f64; 2], samples: usize) -> Vec<f32> {
    let n = samples.max(2);
    if v[0].hypot(v[1]) < 0.25 {
        return a.to_vec();
    }
    (0..w * h)
        .into_par_iter()
        .map(|i| {
            let (x, y) = ((i % w) as f64 + 0.5, (i / w) as f64 + 0.5);
            let mut s = 0.0;
            for k in 0..n {
                let t = k as f64 / (n - 1) as f64 - 0.5;
                s += sample(a, w, h, x - v[0] * t, y - v[1] * t);
            }
            s / n as f32
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_scratch_is_sized_to_the_line_not_the_image() {
        let (f, v, z) = line_scratch(1);
        assert_eq!((f.len(), v.len(), z.len()), (1, 1, 2));
        // A 1 x N image: every row job needs one sample, every column job N.
        let (w, h) = (1usize, 50_000usize);
        let seed: Vec<bool> = (0..w * h).map(|i| i == 777).collect();
        let d = distance(&seed, w, h);
        let r = distance_reference(&seed, w, h);
        assert!(d.iter().zip(&r).all(|(a, b)| a.to_bits() == b.to_bits()));
    }

    /// The distance transform as it was before the scratch reuse (one set of vectors per line).
    fn distance_reference(seed: &[bool], w: usize, h: usize) -> Vec<f32> {
        if w == 0 || h == 0 {
            return vec![];
        }
        let mut g: Vec<f32> = seed.iter().map(|s| if *s { 0.0 } else { BIG }).collect();
        let cols: Vec<Vec<f32>> = (0..w)
            .map(|x| {
                let f: Vec<f32> = (0..h).map(|y| g[y * w + x]).collect();
                let mut d = vec![0.0; h];
                let (mut v, mut z) = (vec![0usize; h], vec![0.0f64; h + 1]);
                dt1(&f, &mut d, &mut v, &mut z);
                d
            })
            .collect();
        for (x, c) in cols.iter().enumerate() {
            for y in 0..h {
                g[y * w + x] = c[y];
            }
        }
        for row in g.chunks_mut(w) {
            let f = row.to_vec();
            let (mut v, mut z) = (vec![0usize; w], vec![0.0f64; w + 1]);
            dt1(&f, row, &mut v, &mut z);
        }
        g.iter().map(|v| v.sqrt()).collect()
    }

    #[test]
    fn distance_is_bit_identical_to_the_per_line_version() {
        let disk =
            |w: usize, h: usize, cx: f64, cy: f64, r: f64| -> Vec<bool> { (0..w * h).map(|i| ((i % w) as f64 - cx).hypot((i / w) as f64 - cy) <= r).collect() };
        let cases: Vec<(usize, usize, Vec<bool>)> = vec![
            (64, 48, disk(64, 48, 30.0, 20.0, 11.0)),
            (64, 48, disk(64, 48, 30.0, 20.0, 11.0).iter().map(|b| !b).collect()),
            (97, 1, (0..97).map(|i| i % 13 == 0).collect()),
            (1, 97, (0..97).map(|i| i % 17 == 5).collect()),
            (40, 30, vec![true; 1200]),
            (40, 30, vec![false; 1200]),
            (40, 30, (0..1200).map(|i| i % 40 == 39 || i / 40 == 0).collect()),
            (333, 211, (0..333 * 211).map(|i| (i * 7919) % 1031 == 0).collect()),
        ];
        for (w, h, seed) in cases {
            let a = distance(&seed, w, h);
            let b = distance_reference(&seed, w, h);
            assert_eq!(a.len(), b.len());
            for (i, (x, y)) in a.iter().zip(&b).enumerate() {
                assert_eq!(x.to_bits(), y.to_bits(), "{w}x{h} pixel {i}: {x} vs {y}");
            }
        }
        assert!(distance(&[], 0, 5).is_empty());
    }

    /// `cargo test --release -p effectcraft-track distance_bench -- --ignored --nocapture`
    /// (set `RAYON_NUM_THREADS` to compare thread counts).
    #[test]
    #[ignore]
    fn distance_bench() {
        let (w, h) = (1920, 1080);
        let seed: Vec<bool> = (0..w * h).map(|i| ((i % w) as f64 - 960.0).hypot((i / w) as f64 - 540.0) <= 300.0).collect();
        let mut times = vec![];
        for _ in 0..15 {
            let t = std::time::Instant::now();
            std::hint::black_box(distance(&seed, w, h));
            times.push(t.elapsed().as_secs_f64() * 1000.0);
        }
        times.sort_by(f64::total_cmp);
        eprintln!("distance 1080p, {} threads: median {:.2} ms", rayon::current_num_threads(), times[times.len() / 2]);
    }

    #[test]
    fn distance_matches_brute_force() {
        let (w, h) = (23, 17);
        let seed: Vec<bool> = (0..w * h).map(|i| (i * 7919) % 31 == 0).collect();
        let d = distance(&seed, w, h);
        for y in 0..h {
            for x in 0..w {
                let mut b = f32::INFINITY;
                for j in 0..h {
                    for i in 0..w {
                        if seed[j * w + i] {
                            b = b.min(((x as f32 - i as f32).powi(2) + (y as f32 - j as f32).powi(2)).sqrt());
                        }
                    }
                }
                assert!((d[y * w + x] - b).abs() < 1e-3, "{x},{y}: {} vs {b}", d[y * w + x]);
            }
        }
    }

    #[test]
    fn guided_filter_recovers_a_linear_blend() {
        // I = αF + (1 − α)B with constant F, B: the filtered binary matte is close to α.
        let (w, h) = (64, 8);
        let alpha: Vec<f32> = (0..w * h).map(|i| (((i % w) as f32 - 26.0) / 12.0).clamp(0.0, 1.0)).collect();
        let (f, b) = ([0.9f32, 0.2, 0.1], [0.1f32, 0.3, 0.8]);
        let img: Vec<[f32; 3]> = alpha.iter().map(|a| [a * f[0] + (1.0 - a) * b[0], a * f[1] + (1.0 - a) * b[1], a * f[2] + (1.0 - a) * b[2]]).collect();
        let p: Vec<f32> = alpha.iter().map(|a| if *a >= 0.5 { 1.0 } else { 0.0 }).collect();
        let q = guided_filter(&img, &p, w, h, 8, 1e-5);
        let err: f32 = q.iter().zip(&alpha).map(|(a, b)| (a.clamp(0.0, 1.0) - b).abs()).sum::<f32>() / (w * h) as f32;
        assert!(err < 0.03, "mean error {err}");
    }
}

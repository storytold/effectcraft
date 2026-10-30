use super::raster::over;
use super::{
    Buf, EffectCtx, IndexedParallelIterator, IntoParallelRefIterator, IntoParallelRefMutIterator, PI, ParallelIterator, Plane, Px, gauss_plane, hash1,
    hsl_to_rgb, morph_frac, premul, rgb_to_hsl, smoothstep, unpremul,
};

pub(super) fn glue_gun(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let pr = ctx.params;
    let q = b.to_px(pr.v2("brushPosition"));
    let rad = (pr.f("strokeWidth") * b.scale * 0.5).max(0.5);
    let density = pr.f("density").max(0.0);
    let refl = pr.f("reflection").to_radians();
    let strength = (pr.f("strength") / 100.0) as f32;
    let glue_only = pr.e("style") == 1;
    let src = b.img.clone();
    let light = [refl.cos() * 0.6, -refl.sin() * 0.6, 0.53];
    let beads = (density.round() as usize).clamp(1, 32);
    let seed = ctx.seed;
    let centers: Vec<([f64; 2], f64)> = (0..beads)
        .map(|i| {
            let a = hash1(i as u32, 1, seed) as f64 * 2.0 * PI;
            let d = if i == 0 { 0.0 } else { hash1(i as u32, 2, seed) as f64 * rad * 0.35 };
            ([q.0 + a.cos() * d, q.1 + a.sin() * d], rad * (0.75 + 0.25 * hash1(i as u32, 3, seed) as f64))
        })
        .collect();
    b.img.rows_mut().for_each(|(y, row)| {
        for (x, px) in row.iter_mut().enumerate() {
            let (fx, fy) = (x as f64 + 0.5, y as f64 + 0.5);
            // Height field: union of hemispheres.
            let mut best = (0.0f64, [0.0f64; 2], 1.0f64);
            for &(c, r) in &centers {
                let (dx, dy) = ((fx - c[0]) / r, (fy - c[1]) / r);
                let d2 = dx * dx + dy * dy;
                if d2 < 1.0 {
                    let hgt = (1.0 - d2).sqrt();
                    if hgt > best.0 {
                        best = (hgt, [dx, dy], r);
                    }
                }
            }
            if best.0 <= 0.0 {
                if glue_only {
                    *px = [0.0; 4];
                }
                continue;
            }
            let nrm = [best.1[0], best.1[1], best.0];
            let ndl = (nrm[0] * light[0] + nrm[1] * light[1] + nrm[2] * light[2]).max(0.0);
            let spec = ndl.powi(24) as f32;
            // Refraction: look through the bead.
            let s = src.sample_bilinear_clamped(fx - nrm[0] * best.2 * 0.5 * strength as f64, fy - nrm[1] * best.2 * 0.5 * strength as f64);
            let edge = smoothstep(0.0, 0.15, best.0 as f32);
            let shade = 0.65 + 0.35 * ndl as f32;
            let g = [s[0] * shade + spec, s[1] * shade + spec, s[2] * shade + spec, s[3].max(0.6)];
            let gp = [g[0].min(g[3] + spec), g[1].min(g[3] + spec), g[2].min(g[3] + spec), g[3].min(1.0)];
            let base = if glue_only { [0.0; 4] } else { *px };
            *px = crate::util::lerp4(base, gp, edge * strength.clamp(0.0, 1.0).max(0.3));
            px[3] = px[3].clamp(0.0, 1.0);
        }
    });
    b
}

// ---------------------------------------------------------------- Paint Bucket

/// Paint Bucket's filled region (1 inside) on `b`: the flood fill from the Fill Point over the
/// layer's pixels (shared with the GPU compositor, which reads the layer back for it).
pub fn paint_bucket_region(ctx: &EffectCtx, b: &Buf) -> Plane {
    let pr = ctx.params;
    let (w, h) = (b.img.width as usize, b.img.height as usize);
    let fp = b.to_px(pr.v2("fillPoint"));
    let (sx, sy) = (fp.0.floor() as i64, fp.1.floor() as i64);
    let sel = pr.e("fillSelector");
    let tol = (pr.f("tolerance") / 255.0) as f32;
    let key = |p: Px| -> [f32; 4] {
        let (c, a) = unpremul(p);
        match sel {
            1 => [c[0], c[1], c[2], 0.0],
            2 => [0.0, 0.0, 0.0, if a <= 1e-3 { 0.0 } else { 1.0 }],
            3 | 4 => [0.0, 0.0, 0.0, a],
            _ => p,
        }
    };
    let mut region = Plane::new(w, h);
    if sx >= 0 && sy >= 0 && (sx as usize) < w && (sy as usize) < h {
        let seed = key(b.img.data[sy as usize * w + sx as usize]);
        let within = |i: usize| {
            let k = key(b.img.data[i]);
            if sel == 2 {
                return k[3] == seed[3];
            }
            (0..4).all(|j| (k[j] - seed[j]).abs() <= tol + 1e-6)
        };
        let mut seen = vec![false; w * h];
        let mut stack = vec![(sx as usize, sy as usize)];
        while let Some((x, y)) = stack.pop() {
            let row = y * w;
            if seen[row + x] || !within(row + x) {
                continue;
            }
            let mut l = x;
            while l > 0 && !seen[row + l - 1] && within(row + l - 1) {
                l -= 1;
            }
            let mut r = x;
            while r + 1 < w && !seen[row + r + 1] && within(row + r + 1) {
                r += 1;
            }
            for i in l..=r {
                seen[row + i] = true;
                region.data[row + i] = 1.0;
                if y > 0 && !seen[row - w + i] {
                    stack.push((i, y - 1));
                }
                if y + 1 < h && !seen[row + w + i] {
                    stack.push((i, y + 1));
                }
            }
        }
    }
    region
}

pub(super) fn paint_bucket(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let pr = ctx.params;
    let (w, h) = (b.img.width as usize, b.img.height as usize);
    if w == 0 || h == 0 {
        return b;
    }
    let region = paint_bucket_region(ctx, &b);
    if pr.b("viewThreshold") {
        b.img.data.par_iter_mut().zip(region.data.par_iter()).for_each(|(px, &v)| *px = [v, v, v, 1.0]);
        return b;
    }
    let sc = b.scale;
    let mut cov = match pr.e("stroke") {
        1 => {
            let s = pr.f("featherSoftness") * sc * 0.5;
            if s > 0.05 { gauss_plane(&region, s, s) } else { region.clone() }
        }
        2 => morph_frac(&region, pr.f("spreadRadius") * sc, true),
        3 => morph_frac(&region, pr.f("spreadRadius") * sc, false),
        4 => {
            let r = pr.f("strokeWidth") * sc;
            let out = morph_frac(&region, r * 0.5, true);
            let inn = morph_frac(&region, r * 0.5, false);
            out.zip_map(&inn, |a, b| (a - b).max(0.0))
        }
        _ => gauss_plane(&region, 0.5, 0.5),
    };
    if pr.e("stroke") == 0 {
        // Antialias: keep the hard interior, soften only the boundary.
        cov = cov.zip_map(&region, |a, r| if r > 0.0 { a.max(0.5) } else { a });
    }
    let color = pr.color("color");
    let op = (pr.f("opacity") / 100.0) as f32;
    let mode = pr.e("blendingMode");
    b.img.data.par_iter_mut().zip(cov.data.par_iter()).for_each(|(px, &c)| {
        let k = (c * op * color[3]).clamp(0.0, 1.0);
        if k <= 0.0 {
            if mode == 9 {
                *px = [0.0; 4];
            }
            return;
        }
        let (dc, da) = unpremul(*px);
        let fc = [color[0], color[1], color[2]];
        let mixed = match mode {
            1 => [dc[0] + fc[0], dc[1] + fc[1], dc[2] + fc[2]],
            2 => [dc[0] * fc[0], dc[1] * fc[1], dc[2] * fc[2]],
            3 => [1.0 - (1.0 - dc[0]) * (1.0 - fc[0]), 1.0 - (1.0 - dc[1]) * (1.0 - fc[1]), 1.0 - (1.0 - dc[2]) * (1.0 - fc[2])],
            4..=7 => {
                let (dh, ds, dl) = rgb_to_hsl(dc[0], dc[1], dc[2]);
                let (fh, fs, fl) = rgb_to_hsl(fc[0], fc[1], fc[2]);
                let (r, g, bl) = match mode {
                    4 => hsl_to_rgb(fh, ds, dl),
                    5 => hsl_to_rgb(dh, fs, dl),
                    6 => hsl_to_rgb(fh, fs, dl),
                    _ => hsl_to_rgb(dh, ds, fl),
                };
                [r, g, bl]
            }
            _ => fc,
        };
        *px = match mode {
            8 => over(premul(fc, k), *px),
            9 => premul(fc, k),
            0 => over(*px, premul(fc, k)),
            _ => {
                if da <= 1e-6 {
                    return;
                }
                let c3 = crate::util::lerp3(dc, mixed, k);
                premul(c3, da)
            }
        };
    });
    b
}

// ---------------------------------------------------------------- Eyedropper Fill

pub(super) fn eyedropper_fill(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let pr = ctx.params;
    let fill = eyedropper_color(ctx, &b);
    let keep_alpha = pr.b("maintainOriginalAlpha");
    let blend = (pr.f("blendWithOriginal") / 100.0) as f32;
    b.img.data.par_iter_mut().for_each(|px| {
        let a = if keep_alpha { px[3] } else { fill[3] };
        let f = premul([fill[0], fill[1], fill[2]], a);
        *px = crate::util::lerp4(f, *px, blend);
    });
    b
}

/// Eyedropper Fill's sampled colour (straight RGB, alpha) from the layer's pixels (shared with
/// the GPU compositor, which reads the layer back for it).
pub fn eyedropper_color(ctx: &EffectCtx, b: &Buf) -> Px {
    let pr = ctx.params;
    let (w, h) = (b.img.width as i64, b.img.height as i64);
    let c = b.to_px(pr.v2("samplePoint"));
    let r = (pr.f("sampleRadius") * b.scale).max(0.0);
    let mode = pr.e("averagePixelColors");
    let (cx, cy) = (c.0.floor() as i64, c.1.floor() as i64);
    let ri = r.ceil() as i64;
    let mut acc = [0.0f64; 4];
    let mut n = 0.0f64;
    for y in (cy - ri).max(0)..=(cy + ri).min(h - 1) {
        for x in (cx - ri).max(0)..=(cx + ri).min(w - 1) {
            if ((x - cx).pow(2) + (y - cy).pow(2)) as f64 > r * r + 0.5 {
                continue;
            }
            let px = b.img.data[(y * w + x) as usize];
            match mode {
                0 => {
                    if px[3] > 1e-4 {
                        let (s, _) = unpremul(px);
                        for k in 0..3 {
                            acc[k] += s[k] as f64;
                        }
                        acc[3] += 1.0;
                        n += 1.0;
                    }
                }
                1 => {
                    let (s, _) = unpremul(px);
                    for k in 0..3 {
                        acc[k] += s[k] as f64;
                    }
                    acc[3] += 1.0;
                    n += 1.0;
                }
                2 => {
                    for k in 0..3 {
                        acc[k] += px[k] as f64;
                    }
                    acc[3] += 1.0;
                    n += 1.0;
                }
                _ => {
                    for k in 0..4 {
                        acc[k] += px[k] as f64;
                    }
                    n += 1.0;
                }
            }
        }
    }
    if n > 0.0 {
        if mode == 3 {
            let a = (acc[3] / n) as f32;
            let s = if a > 1e-6 { [(acc[0] / n) as f32 / a, (acc[1] / n) as f32 / a, (acc[2] / n) as f32 / a] } else { [0.0; 3] };
            [s[0], s[1], s[2], a]
        } else {
            [(acc[0] / n) as f32, (acc[1] / n) as f32, (acc[2] / n) as f32, 1.0]
        }
    } else {
        [0.0, 0.0, 0.0, 0.0]
    }
}

// ---------------------------------------------------------------- CC Threads

pub(super) fn threads(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let pr = ctx.params;
    let sc = b.scale;
    let tw = (pr.f("width") * sc).max(1.0);
    let th = (pr.f("height") * sc).max(1.0);
    let overlaps = pr.f("overlaps").round().clamp(1.0, 20.0) as i64;
    let ang = pr.f("direction").to_radians();
    let (sn, cs) = ang.sin_cos();
    let c = b.to_px(pr.v2("center"));
    let coverage = (pr.f("coverage") / 100.0).clamp(0.0, 1.0);
    let shadow = (pr.f("shadowing") / 100.0) as f32;
    let texture = (pr.f("texture") / 100.0) as f32;
    let src = b.img.clone();
    let to_px = |u: f64, v: f64| (c.0 + u * cs - v * sn, c.1 + u * sn + v * cs);
    b.img.rows_mut().for_each(|(y, row)| {
        for (x, px) in row.iter_mut().enumerate() {
            let (dx, dy) = (x as f64 + 0.5 - c.0, y as f64 + 0.5 - c.1);
            let u = dx * cs + dy * sn;
            let v = -dx * sn + dy * cs;
            let (ci, cj) = ((u / tw).floor() as i64, (v / th).floor() as i64);
            let (fu, fv) = (u / tw - ci as f64, v / th - cj as f64);
            // Horizontal thread j occupies the band |fv-0.5| < coverage/2; vertical thread i the
            // band |fu-0.5| < coverage/2.
            let in_h = (fv - 0.5).abs() < coverage * 0.5;
            let in_v = (fu - 0.5).abs() < coverage * 0.5;
            // Twill: horizontal is on top when (i + j) mod (2·overlaps) < overlaps.
            let h_top = (ci + cj).rem_euclid(2 * overlaps) < overlaps;
            let which = match (in_h, in_v) {
                (true, true) => Some(h_top),
                (true, false) => Some(true),
                (false, true) => Some(false),
                _ => None,
            };
            let Some(horizontal) = which else {
                *px = [0.0; 4];
                continue;
            };
            // Thread colour: the source sampled along the thread centre line.
            let (sx, sy) = if horizontal { to_px(u, (cj as f64 + 0.5) * th) } else { to_px((ci as f64 + 0.5) * tw, v) };
            let s = src.sample_bilinear_clamped(sx, sy);
            // Round profile across the thread + darkening where it dives under its neighbour.
            let across = if horizontal { (fv - 0.5) / (coverage * 0.5).max(1e-6) } else { (fu - 0.5) / (coverage * 0.5).max(1e-6) };
            let along = if horizontal { fu } else { fv };
            let profile = (1.0 - across * across).max(0.0).sqrt() as f32;
            let dive = 1.0 - (along - 0.5).abs() as f32 * 2.0;
            let shade = 1.0 - shadow * (1.0 - profile) * 0.8 - shadow * 0.25 * (1.0 - dive) * if horizontal == h_top { 0.0 } else { 1.0 };
            let fib =
                if texture > 0.0 { 1.0 - texture * 0.3 * hash1((along * 64.0) as u32, (across * 8.0 + 8.0) as u32, ci as u32 ^ (cj as u32) << 8) } else { 1.0 };
            let k = (shade * fib).max(0.0);
            *px = [s[0] * k, s[1] * k, s[2] * k, s[3]];
        }
    });
    b
}

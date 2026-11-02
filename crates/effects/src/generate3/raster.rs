use super::{Buf, EffectCtx, IndexedParallelIterator, ParallelIterator, ParallelSliceMut, Plane, Px, smoothstep};

// ---------------------------------------------------------------- shared rasterisation

/// A round-capped line segment in buffer pixels with radius `r` and intensity `v`.
#[derive(Clone, Copy, Debug)]
pub struct Seg {
    pub a: [f64; 2],
    pub b: [f64; 2],
    pub r: f64,
    pub v: f32,
}

/// Rasterise segments into a coverage plane (max-combined). `hardness` (0..1) is the fraction
/// of the radius that is fully opaque; the rest falls off smoothly.
pub(crate) fn raster_segs(w: usize, h: usize, segs: &[Seg], hardness: f64) -> Plane {
    let mut out = Plane::new(w, h);
    if w == 0 || h == 0 || segs.is_empty() {
        return out;
    }
    let mut buckets: Vec<Vec<u32>> = vec![Vec::new(); h];
    for (i, s) in segs.iter().enumerate() {
        let pad = s.r + 1.0;
        let y0 = (s.a[1].min(s.b[1]) - pad).floor().max(0.0) as usize;
        let y1 = (s.a[1].max(s.b[1]) + pad).ceil().min(h as f64);
        if y1 <= 0.0 || y0 >= h {
            continue;
        }
        for row in buckets.iter_mut().take(y1 as usize).skip(y0) {
            row.push(i as u32);
        }
    }
    let hard = hardness.clamp(0.0, 1.0);
    out.data.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let py = y as f64 + 0.5;
        for &i in &buckets[y] {
            let s = segs[i as usize];
            let pad = s.r + 1.0;
            let x0 = (s.a[0].min(s.b[0]) - pad).floor().max(0.0) as usize;
            let x1 = ((s.a[0].max(s.b[0]) + pad).ceil().max(0.0) as usize).min(w);
            let (dx, dy) = (s.b[0] - s.a[0], s.b[1] - s.a[1]);
            let l2 = dx * dx + dy * dy;
            let inner = s.r * hard - 0.5;
            let outer = s.r + 0.5;
            for (x, o) in row.iter_mut().enumerate().take(x1).skip(x0) {
                let px = x as f64 + 0.5;
                let t = if l2 > 1e-12 { (((px - s.a[0]) * dx + (py - s.a[1]) * dy) / l2).clamp(0.0, 1.0) } else { 0.0 };
                let d = ((px - s.a[0] - dx * t).powi(2) + (py - s.a[1] - dy * t).powi(2)).sqrt();
                if d >= outer {
                    continue;
                }
                let c = if d <= inner { 1.0 } else { 1.0 - smoothstep(inner as f32, outer as f32, d as f32) } * s.v;
                if c > *o {
                    *o = c;
                }
            }
        }
    });
    out
}

/// Segments along a polyline (buffer px).
pub fn poly_segs(pts: &[[f64; 2]], closed: bool, r: f64, v: f32, out: &mut Vec<Seg>) {
    let n = pts.len();
    if n == 1 {
        out.push(Seg { a: pts[0], b: pts[0], r, v });
    }
    if n < 2 {
        return;
    }
    let segs = if closed { n } else { n - 1 };
    for i in 0..segs {
        out.push(Seg { a: pts[i], b: pts[(i + 1) % n], r, v });
    }
}

/// Portion `[s0, s1]` (arc length) of a polyline.
pub(crate) fn trim_poly(pts: &[[f64; 2]], closed: bool, s0: f64, s1: f64) -> Vec<[f64; 2]> {
    let n = pts.len();
    let mut out = Vec::new();
    if n < 2 || s1 <= s0 {
        return out;
    }
    let segs = if closed { n } else { n - 1 };
    let mut acc = 0.0;
    for i in 0..segs {
        let (a, b) = (pts[i], pts[(i + 1) % n]);
        let l = ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt();
        let (e0, e1) = (acc, acc + l);
        acc = e1;
        if e1 < s0 || e0 > s1 || l <= 0.0 {
            continue;
        }
        let at = |s: f64| {
            let t = ((s - e0) / l).clamp(0.0, 1.0);
            [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]
        };
        let p0 = at(s0.max(e0));
        if out.last() != Some(&p0) {
            out.push(p0);
        }
        out.push(at(s1.min(e1)));
    }
    out
}

#[inline]
pub(super) fn over(dst: Px, src: Px) -> Px {
    let k = 1.0 - src[3];
    [src[0] + dst[0] * k, src[1] + dst[1] * k, src[2] + dst[2] * k, src[3] + dst[3] * k]
}

/// Mask `idx` (1-based; 0 = none) mapped to buffer pixels.
pub(crate) fn mask_px(ctx: &EffectCtx, b: &Buf, idx: usize) -> Option<(Vec<[f64; 2]>, bool, bool)> {
    let m = ctx.env.masks.get(idx.checked_sub(1)?)?;
    if m.points.is_empty() {
        return None;
    }
    Some((m.points.iter().map(|&q| [q[0] * b.scale + b.offset[0], q[1] * b.scale + b.offset[1]]).collect(), m.closed, m.inverted))
}

/// All masks mapped to buffer pixels.
pub(super) fn all_masks_px(ctx: &EffectCtx, b: &Buf) -> Vec<(Vec<[f64; 2]>, bool, bool)> {
    (1..=ctx.env.masks.len()).filter_map(|i| mask_px(ctx, b, i)).collect()
}

/// Chaikin corner cutting (open polyline, endpoints kept).
pub(super) fn chaikin(pts: &[[f64; 2]], iters: usize) -> Vec<[f64; 2]> {
    let mut cur = pts.to_vec();
    for _ in 0..iters {
        if cur.len() < 3 {
            return cur;
        }
        let mut nx = Vec::with_capacity(cur.len() * 2);
        nx.push(cur[0]);
        for w in cur.windows(2) {
            let (a, b) = (w[0], w[1]);
            nx.push([a[0] * 0.75 + b[0] * 0.25, a[1] * 0.75 + b[1] * 0.25]);
            nx.push([a[0] * 0.25 + b[0] * 0.75, a[1] * 0.25 + b[1] * 0.75]);
        }
        nx.push(*cur.last().unwrap_or(&[0.0; 2]));
        cur = nx;
    }
    cur
}

// ---------------------------------------------------------------- Scribble

/// Scan runs (start, end along direction `d`) of the fill region on the line `n·p = s`.
pub(super) fn scan_runs(polys: &[(Vec<[f64; 2]>, bool, bool)], fill: u32, ew: f64, d: [f64; 2], n: [f64; 2], s: f64, out: &mut Vec<(f64, f64)>) {
    out.clear();
    for (pts, _closed, inverted) in polys {
        let m = pts.len();
        if m < 2 {
            continue;
        }
        // Crossings: (t along d, |sin| of crossing angle).
        let mut xs: Vec<(f64, f64)> = Vec::new();
        for i in 0..m {
            let (a, b) = (pts[i], pts[(i + 1) % m]);
            let (na, nb) = (a[0] * n[0] + a[1] * n[1] - s, b[0] * n[0] + b[1] * n[1] - s);
            if (na > 0.0) == (nb > 0.0) {
                continue;
            }
            let t = na / (na - nb);
            let q = [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t];
            let (ex, ey) = (b[0] - a[0], b[1] - a[1]);
            let el = (ex * ex + ey * ey).sqrt().max(1e-9);
            let sin = ((ex * n[0] + ey * n[1]) / el).abs().max(0.2);
            xs.push((q[0] * d[0] + q[1] * d[1], sin));
        }
        xs.sort_by(|a, b| a.0.total_cmp(&b.0));
        // Signed area decides which side is "left".
        let area: f64 = (0..m).map(|i| pts[i][0] * pts[(i + 1) % m][1] - pts[(i + 1) % m][0] * pts[i][1]).sum();
        let left_is_inside = area < 0.0;
        let mode = match fill {
            4 => {
                if left_is_inside {
                    2
                } else {
                    3
                }
            }
            5 => {
                if left_is_inside {
                    3
                } else {
                    2
                }
            }
            f => f,
        };
        let mode = if *inverted && mode == 0 { 6 } else { mode };
        for (k, &(t, sin)) in xs.iter().enumerate() {
            let entering = k % 2 == 0;
            let l = ew / sin;
            match mode {
                0 if entering && k + 1 < xs.len() => {
                    out.push((t, xs[k + 1].0));
                }
                1 => out.push((t - l * 0.5, t + l * 0.5)),
                2 => out.push(if entering { (t, t + l) } else { (t - l, t) }),
                3 => out.push(if entering { (t - l, t) } else { (t, t + l) }),
                _ => {}
            }
        }
        if mode == 6 {
            // Inverted Inside: outside of the polygon within the bounds.
            let lo = pts.iter().map(|q| q[0] * d[0] + q[1] * d[1]).fold(f64::INFINITY, f64::min) - ew;
            let hi = pts.iter().map(|q| q[0] * d[0] + q[1] * d[1]).fold(f64::NEG_INFINITY, f64::max) + ew;
            let mut cur = lo;
            for pair in xs.chunks(2) {
                out.push((cur, pair[0].0));
                cur = pair.get(1).map(|v| v.0).unwrap_or(hi);
            }
            out.push((cur, hi));
        }
    }
    out.sort_by(|a, b| a.0.total_cmp(&b.0));
    // Merge overlaps.
    let mut merged: Vec<(f64, f64)> = Vec::with_capacity(out.len());
    for &(a, b) in out.iter() {
        if let Some(last) = merged.last_mut()
            && a <= last.1
        {
            last.1 = last.1.max(b);
            continue;
        }
        merged.push((a, b));
    }
    *out = merged;
}

/// End Cap and Join options (Scribble's Edge Options).
pub(super) const END_CAPS: [&str; 3] = ["Round", "Butt", "Projecting"];

pub(super) const JOINS: [&str; 3] = ["Round", "Miter", "Bevel"];

/// Convex polygons whose union is the stroke of the polylines `lines` with half width `r`:
/// one quad per segment, plus the joins (Round discs, Miter wedges up to `miter_limit` × the
/// half width, Bevel triangles) and the caps (Round discs, Butt none, Projecting square ends).
pub(crate) fn stroke_outline(lines: &[Vec<[f64; 2]>], r: f64, cap: u32, join: u32, miter_limit: f64) -> Vec<Vec<[f64; 2]>> {
    let disc = |c: [f64; 2]| -> Vec<[f64; 2]> {
        (0..16).map(|k| (k as f64 / 16.0 * std::f64::consts::TAU).sin_cos()).map(|(s, co)| [c[0] + co * r, c[1] + s * r]).collect()
    };
    let mut out = Vec::new();
    for l in lines {
        let pts: Vec<[f64; 2]> = l.iter().copied().fold(Vec::new(), |mut v, p| {
            if v.last().is_none_or(|q: &[f64; 2]| (q[0] - p[0]).hypot(q[1] - p[1]) > 1e-9) {
                v.push(p);
            }
            v
        });
        if pts.len() < 2 {
            if let (Some(p), 0) = (pts.first(), cap) {
                out.push(disc(*p));
            }
            continue;
        }
        let dir = |a: [f64; 2], b: [f64; 2]| {
            let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
            let l = dx.hypot(dy).max(1e-12);
            [dx / l, dy / l]
        };
        let n = pts.len();
        for k in 0..n - 1 {
            let (mut a, mut b) = (pts[k], pts[k + 1]);
            let d = dir(a, b);
            // Projecting caps extend the end segments by the half width.
            if cap == 2 && k == 0 {
                a = [a[0] - d[0] * r, a[1] - d[1] * r];
            }
            if cap == 2 && k == n - 2 {
                b = [b[0] + d[0] * r, b[1] + d[1] * r];
            }
            let nn = [-d[1] * r, d[0] * r];
            out.push(vec![[a[0] + nn[0], a[1] + nn[1]], [b[0] + nn[0], b[1] + nn[1]], [b[0] - nn[0], b[1] - nn[1]], [a[0] - nn[0], a[1] - nn[1]]]);
        }
        // Joins.
        for k in 1..n - 1 {
            let (p, d0, d1) = (pts[k], dir(pts[k - 1], pts[k]), dir(pts[k], pts[k + 1]));
            let cross = d0[0] * d1[1] - d0[1] * d1[0];
            if cross.abs() < 1e-9 && d0[0] * d1[0] + d0[1] * d1[1] > 0.0 {
                continue;
            }
            if join == 0 {
                out.push(disc(p));
                continue;
            }
            // The outer side of the turn.
            let s = if cross > 0.0 { -1.0 } else { 1.0 };
            let o0 = [p[0] - d0[1] * r * s, p[1] + d0[0] * r * s];
            let o1 = [p[0] - d1[1] * r * s, p[1] + d1[0] * r * s];
            let bevel = vec![p, o0, o1];
            if join == 1 {
                // Miter point: where the two offset edges meet.
                let bis = [d0[0] - d1[0], d0[1] - d1[1]];
                let bl = bis[0].hypot(bis[1]);
                let cos_half = ((1.0 + d0[0] * d1[0] + d0[1] * d1[1]) * 0.5).max(0.0).sqrt();
                let len = if cos_half > 1e-6 { r / cos_half } else { f64::INFINITY };
                if bl > 1e-9 && len <= miter_limit * r {
                    let m = [p[0] + (o0[0] + o1[0] - 2.0 * p[0]) * 0.5, p[1] + (o0[1] + o1[1] - 2.0 * p[1]) * 0.5];
                    let ml = (m[0] - p[0]).hypot(m[1] - p[1]).max(1e-12);
                    let tip = [p[0] + (m[0] - p[0]) / ml * len, p[1] + (m[1] - p[1]) / ml * len];
                    out.push(vec![p, o0, tip, o1]);
                    continue;
                }
            }
            out.push(bevel);
        }
        if cap == 0 {
            out.push(disc(pts[0]));
            out.push(disc(pts[n - 1]));
        }
    }
    out
}

/// Coverage (union, 4×4 samples per pixel) of convex polygons on a `w`×`h` grid.
pub(crate) fn raster_convex(w: usize, h: usize, polys: &[Vec<[f64; 2]>]) -> Plane {
    let mut out = Plane::new(w, h);
    let inside = |poly: &[[f64; 2]], x: f64, y: f64| {
        let n = poly.len();
        let mut sign = 0.0;
        for i in 0..n {
            let (a, b) = (poly[i], poly[(i + 1) % n]);
            let c = (b[0] - a[0]) * (y - a[1]) - (b[1] - a[1]) * (x - a[0]);
            if c.abs() < 1e-12 {
                continue;
            }
            if sign == 0.0 {
                sign = c.signum();
            } else if c.signum() != sign {
                return false;
            }
        }
        true
    };
    for poly in polys {
        if poly.len() < 3 {
            continue;
        }
        let (mut x0, mut y0, mut x1, mut y1) = (f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
        for p in poly {
            x0 = x0.min(p[0]);
            y0 = y0.min(p[1]);
            x1 = x1.max(p[0]);
            y1 = y1.max(p[1]);
        }
        let (xa, xb) = (x0.floor().max(0.0) as usize, (x1.ceil().max(0.0) as usize).min(w));
        let (ya, yb) = (y0.floor().max(0.0) as usize, (y1.ceil().max(0.0) as usize).min(h));
        for y in ya..yb {
            for x in xa..xb {
                let mut hit = 0;
                for sy in 0..4 {
                    for sx in 0..4 {
                        if inside(poly, x as f64 + (sx as f64 + 0.5) / 4.0, y as f64 + (sy as f64 + 0.5) / 4.0) {
                            hit += 1;
                        }
                    }
                }
                let v = &mut out.data[y * w + x];
                *v = v.max(hit as f32 / 16.0);
            }
        }
    }
    out
}

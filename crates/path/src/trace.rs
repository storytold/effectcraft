//! Auto-trace: outlines of a scalar field (a layer's alpha, a colour channel or luminance) as
//! Bezier paths (Layer ▸ Auto-trace).
//!
//! 1. **Marching squares** over the samples at pixel centres, with linear interpolation of the
//!    threshold crossing on each cell edge (saddles resolved by the cell's mean), on a field padded
//!    with "outside" so every contour closes.
//! 2. **Douglas–Peucker** simplification of each closed loop to the Tolerance (pixels).
//! 3. **Bezier fitting**: each simplified span is fitted with one cubic (least squares with fixed
//!    end tangents, Newton reparameterisation; `crate::fit`). The tangent at each vertex is the
//!    chord direction at Corner Roundness 0 (straight segments, sharp corners) and the smooth
//!    (Catmull–Rom) direction at 100 %, blended between.
//!
//! Loops smaller than Minimum Area are dropped. Each loop gets its nesting depth (0 = outermost),
//! so callers can add outer outlines and subtract holes.

use std::collections::HashMap;

use effectcraft_keyframe::ShapePath;
use kurbo::{Point, Vec2};

use crate::fit::{fit_single, unit};

/// Auto-trace options.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TraceOpts {
    /// Inside where the field is at least this (0…1).
    pub threshold: f32,
    /// Douglas–Peucker tolerance in pixels.
    pub tolerance: f64,
    /// Loops with a smaller area (square pixels) are dropped.
    pub min_area: f64,
    /// 0 = polygons, 1 = fully smooth.
    pub corner_roundness: f64,
}

impl Default for TraceOpts {
    fn default() -> Self {
        TraceOpts { threshold: 0.5, tolerance: 1.0, min_area: 10.0, corner_roundness: 0.5 }
    }
}

/// A traced outline.
#[derive(Clone, Debug, PartialEq)]
pub struct Traced {
    pub path: ShapePath,
    /// Number of other outlines that contain this one (even = filled, odd = hole).
    pub depth: usize,
    /// Signed area of the simplified polygon (pixels²).
    pub area: f64,
}

/// Trace `field` (`w × h`, row-major, sample (x, y) at pixel centre (x + 0.5, y + 0.5)).
pub fn trace(field: &[f32], w: usize, h: usize, o: &TraceOpts) -> Vec<Traced> {
    if w == 0 || h == 0 || field.len() < w * h {
        return vec![];
    }
    let loops = contours(field, w, h, o.threshold);
    // (raw loop, kept raw indices, simplified loop, area)
    let mut polys: Vec<(Vec<[f64; 2]>, Vec<usize>, Vec<[f64; 2]>, f64)> = loops
        .into_iter()
        .filter_map(|raw| {
            let a = signed_area(&raw);
            if a.abs() < o.min_area.max(0.0) || raw.len() < 3 {
                return None;
            }
            let idx = simplify_idx(&raw, o.tolerance.max(0.01));
            if idx.len() < 3 {
                return None;
            }
            let simp = idx.iter().map(|&i| raw[i]).collect();
            Some((raw, idx, simp, a))
        })
        .collect();
    let depths = nesting_depths(&polys.iter().map(|p| (p.2.as_slice(), p.3.abs())).collect::<Vec<_>>(), w, h);
    let mut out: Vec<Traced> = polys
        .drain(..)
        .zip(depths)
        .map(|((raw, idx, simp, a), depth)| Traced { path: fit_loop(&raw, &idx, &simp, o.corner_roundness.clamp(0.0, 1.0)), depth, area: a })
        .collect();
    // Outer outlines first, then holes, islands…; larger first within a depth.
    out.sort_by(|a, b| a.depth.cmp(&b.depth).then(b.area.abs().total_cmp(&a.area.abs())));
    out
}

/// Grid cell (pixels) of the index [`nesting_depths`] finds candidate containers with.
const DEPTH_CELL: f64 = 32.0;

/// Most grid cells one loop's box is entered into; a box covering more is kept in a short list
/// every point is tested against, so nested full-image contours can't fill the grid.
const MAX_BOX_CELLS: usize = 64;

/// Per loop (simplified points, |area|): how many other, larger loops contain its first point.
/// Only loops whose bounding box holds that point can contain it, so each loop's box is entered
/// into a coarse grid over the `w × h` field and a point is tested against the loops of its
/// cell alone: many separate regions no longer test every pair.
fn nesting_depths(polys: &[(&[[f64; 2]], f64)], w: usize, h: usize) -> Vec<usize> {
    // Loops lie within the field padded by half a pixel each side.
    let (gw, gh) = (((w as f64 + 1.0) / DEPTH_CELL).ceil() as usize + 1, ((h as f64 + 1.0) / DEPTH_CELL).ceil() as usize + 1);
    let cell = |v: f64, n: usize| (((v + 0.5) / DEPTH_CELL).floor().max(0.0) as usize).min(n - 1);
    let boxes: Vec<[f64; 4]> = polys
        .iter()
        .map(|(p, _)| {
            p.iter().fold([f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY], |b, q| {
                [b[0].min(q[0]), b[1].min(q[1]), b[2].max(q[0]), b[3].max(q[1])]
            })
        })
        .collect();
    let mut grid: Vec<Vec<usize>> = vec![Vec::new(); gw * gh];
    let mut wide: Vec<usize> = Vec::new();
    for (j, b) in boxes.iter().enumerate() {
        let (x0, x1, y0, y1) = (cell(b[0], gw), cell(b[2], gw), cell(b[1], gh), cell(b[3], gh));
        if (x1 - x0 + 1).saturating_mul(y1 - y0 + 1) > MAX_BOX_CELLS {
            wide.push(j);
            continue;
        }
        for cy in y0..=y1 {
            for cx in x0..=x1 {
                grid[cy * gw + cx].push(j);
            }
        }
    }
    (0..polys.len())
        .map(|i| {
            let p = polys[i].0[0];
            grid[cell(p[1], gh) * gw + cell(p[0], gw)]
                .iter()
                .chain(&wide)
                .filter(|&&j| {
                    let b = boxes[j];
                    j != i && polys[j].1 > polys[i].1 && p[0] >= b[0] && p[0] <= b[2] && p[1] >= b[1] && p[1] <= b[3] && point_in_poly(p, polys[j].0)
                })
                .count()
        })
        .collect()
}

/// Marching squares: closed polylines (pixel coordinates).
fn contours(field: &[f32], w: usize, h: usize, thr: f32) -> Vec<Vec<[f64; 2]>> {
    // Padded grid: sample (i, j) of the padded grid is pixel (i - 1, j - 1), outside = -inf.
    let (gw, gh) = (w + 2, h + 2);
    let val = |i: usize, j: usize| -> f32 { if i == 0 || j == 0 || i > w || j > h { f32::NEG_INFINITY } else { field[(j - 1) * w + (i - 1)] } };
    let inside = |i: usize, j: usize| val(i, j) >= thr;
    // Edge ids: horizontal edge (i, j)-(i+1, j) = 2 * (j * gw + i); vertical (i, j)-(i, j+1) = +1.
    let pos = |i: usize, j: usize| [i as f64 - 0.5, j as f64 - 0.5];
    let cross = |a: (usize, usize), b: (usize, usize)| -> [f64; 2] {
        let (va, vb) = (val(a.0, a.1), val(b.0, b.1));
        let t = if va.is_finite() && vb.is_finite() && (vb - va).abs() > 1e-12 {
            ((thr - va) / (vb - va)).clamp(0.0, 1.0) as f64
        } else if va.is_finite() {
            // b is padding: the crossing sits half a pixel out.
            if va >= thr { 0.5 } else { 0.0 }
        } else if vb >= thr {
            0.5
        } else {
            1.0
        };
        let (pa, pb) = (pos(a.0, a.1), pos(b.0, b.1));
        [pa[0] + (pb[0] - pa[0]) * t, pa[1] + (pb[1] - pa[1]) * t]
    };
    let hid = |i: usize, j: usize| 2 * (j * gw + i);
    let vid = |i: usize, j: usize| 2 * (j * gw + i) + 1;
    // Directed segments (from edge, to edge) keeping the inside on the left.
    let mut next: HashMap<usize, usize> = HashMap::new();
    let mut point: HashMap<usize, [f64; 2]> = HashMap::new();
    for j in 0..gh - 1 {
        for i in 0..gw - 1 {
            // Corners: tl (i, j), tr (i+1, j), br (i+1, j+1), bl (i, j+1).
            let (tl, tr, br, bl) = (inside(i, j), inside(i + 1, j), inside(i + 1, j + 1), inside(i, j + 1));
            let code = (tl as u8) << 3 | (tr as u8) << 2 | (br as u8) << 1 | bl as u8;
            if code == 0 || code == 15 {
                continue;
            }
            let top = hid(i, j);
            let bottom = hid(i, j + 1);
            let left = vid(i, j);
            let right = vid(i + 1, j);
            let mut seg = |a: usize, b: usize| {
                next.insert(a, b);
            };
            // Walk with the inside on the right in image coordinates (y down), which is
            // counter-clockwise on screen for outer loops… orientation is fixed afterwards.
            match code {
                1 => seg(left, bottom),
                2 => seg(bottom, right),
                3 => seg(left, right),
                4 => seg(right, top),
                5 => {
                    // tr + bl inside (saddle).
                    let centre = (val(i, j) + val(i + 1, j) + val(i + 1, j + 1) + val(i, j + 1)) / 4.0;
                    if centre >= thr {
                        seg(left, top);
                        seg(right, bottom);
                    } else {
                        seg(left, bottom);
                        seg(right, top);
                    }
                }
                6 => seg(bottom, top),
                7 => seg(left, top),
                8 => seg(top, left),
                9 => seg(top, bottom),
                10 => {
                    // tl + br inside (saddle).
                    let centre = (val(i, j) + val(i + 1, j) + val(i + 1, j + 1) + val(i, j + 1)) / 4.0;
                    if centre >= thr {
                        seg(top, right);
                        seg(bottom, left);
                    } else {
                        seg(top, left);
                        seg(bottom, right);
                    }
                }
                11 => seg(top, right),
                12 => seg(right, left),
                13 => seg(right, bottom),
                14 => seg(bottom, left),
                _ => {}
            }
            for (e, a, b) in [(top, (i, j), (i + 1, j)), (bottom, (i, j + 1), (i + 1, j + 1)), (left, (i, j), (i, j + 1)), (right, (i + 1, j), (i + 1, j + 1))]
            {
                if inside(a.0, a.1) != inside(b.0, b.1) {
                    point.entry(e).or_insert_with(|| cross(a, b));
                }
            }
        }
    }
    let mut out = vec![];
    let mut keys: Vec<usize> = next.keys().copied().collect();
    keys.sort_unstable();
    for start in keys {
        if !next.contains_key(&start) {
            continue;
        }
        let mut poly = vec![];
        let mut e = start;
        while let Some(n) = next.remove(&e) {
            if let Some(p) = point.get(&e) {
                poly.push(*p);
            }
            e = n;
            if e == start {
                break;
            }
        }
        if poly.len() >= 3 {
            out.push(poly);
        }
    }
    out
}

/// Shoelace area (positive = clockwise on screen with y down).
pub fn signed_area(p: &[[f64; 2]]) -> f64 {
    let n = p.len();
    let mut a = 0.0;
    for i in 0..n {
        let (q, r) = (p[i], p[(i + 1) % n]);
        a += q[0] * r[1] - r[0] * q[1];
    }
    a * 0.5
}

fn point_in_poly(p: [f64; 2], poly: &[[f64; 2]]) -> bool {
    let mut inside = false;
    let n = poly.len();
    let mut j = n - 1;
    for i in 0..n {
        let (a, b) = (poly[i], poly[j]);
        if (a[1] > p[1]) != (b[1] > p[1]) && p[0] < (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) + a[0] {
            inside = !inside;
        }
        j = i;
    }
    inside
}

fn seg_dist(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let l2 = dx * dx + dy * dy;
    let t = if l2 > 0.0 { (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / l2).clamp(0.0, 1.0) } else { 0.0 };
    ((p[0] - a[0] - t * dx).powi(2) + (p[1] - a[1] - t * dy).powi(2)).sqrt()
}

/// Distance evaluations Douglas–Peucker may spend per point of a loop. Balanced splits need
/// about log2(n); splits that keep landing next to an end would need up to n.
const DP_WORK_PER_POINT: usize = 64;

/// Douglas–Peucker on an open polyline: kept indices (unordered). Spans wait on an explicit
/// stack (no recursion depth to overflow), and once `budget` distance evaluations are spent the
/// spans still waiting keep all their points: the outline stays exact, just less simplified.
fn dp(p: &[[f64; 2]], lo: usize, hi: usize, tol: f64, budget: &mut usize, keep: &mut Vec<usize>) {
    let mut spans = vec![(lo, hi)];
    while let Some((lo, hi)) = spans.pop() {
        if hi <= lo + 1 {
            continue;
        }
        if *budget < hi - lo - 1 {
            keep.extend(lo + 1..hi);
            continue;
        }
        *budget -= hi - lo - 1;
        let (mut best, mut bi) = (0.0, lo);
        for (k, q) in p.iter().enumerate().take(hi).skip(lo + 1) {
            let d = seg_dist(*q, p[lo], p[hi]);
            if d > best {
                best = d;
                bi = k;
            }
        }
        if best > tol {
            keep.push(bi);
            spans.push((lo, bi));
            spans.push((bi, hi));
        }
    }
}

/// Douglas–Peucker on a closed loop: indices into `p` of the kept vertices, in order.
fn simplify_idx(p: &[[f64; 2]], tol: f64) -> Vec<usize> {
    let n = p.len();
    // Split at the two mutually farthest-ish points (first point and the point farthest from it).
    let far = (0..n).max_by(|&a, &b| {
        let d = |i: usize| (p[i][0] - p[0][0]).powi(2) + (p[i][1] - p[0][1]).powi(2);
        d(a).total_cmp(&d(b))
    });
    let far = far.unwrap_or(0);
    let mut ext: Vec<[f64; 2]> = p.to_vec();
    ext.push(p[0]);
    let mut keep = vec![0];
    let mut budget = n.saturating_mul(DP_WORK_PER_POINT);
    dp(&ext, 0, far, tol, &mut budget, &mut keep);
    keep.push(far);
    dp(&ext, far, n, tol, &mut budget, &mut keep);
    keep.sort_unstable();
    keep.dedup();
    keep.retain(|&i| i < n);
    keep
}

/// Fit cubics through the simplified vertices of a loop (`raw` holds the full contour, `idx`
/// the raw index of each simplified vertex, as [`simplify_idx`] kept them).
fn fit_loop(raw: &[[f64; 2]], idx: &[usize], simp: &[[f64; 2]], round: f64) -> ShapePath {
    let n = simp.len();
    if round <= 1e-6 {
        return ShapePath::polygon(simp, true);
    }
    let pt = |a: [f64; 2]| Point::new(a[0], a[1]);
    let v = |a: [f64; 2], b: [f64; 2]| Vec2::new(b[0] - a[0], b[1] - a[1]);
    // Per vertex: (direction leaving, direction arriving), blended between chord and smooth.
    let dirs: Vec<(Vec2, Vec2)> = (0..n)
        .map(|i| {
            let (prev, cur, nxt) = (simp[(i + n - 1) % n], simp[i], simp[(i + 1) % n]);
            let smooth = unit(v(prev, nxt)).unwrap_or(Vec2::new(1.0, 0.0));
            let out_chord = unit(v(cur, nxt)).unwrap_or(smooth);
            let in_chord = unit(v(prev, cur)).unwrap_or(smooth);
            let leave = unit(out_chord * (1.0 - round) + smooth * round).unwrap_or(out_chord);
            let arrive = unit(in_chord * (1.0 - round) + smooth * round).unwrap_or(in_chord);
            (leave, arrive)
        })
        .collect();
    let mut path = ShapePath::polygon(simp, true);
    for i in 0..n {
        let j = (i + 1) % n;
        let (a, b) = (idx[i], idx[j]);
        let mut pts: Vec<Point> =
            if b > a { raw[a..=b].iter().map(|q| pt(*q)).collect() } else { raw[a..].iter().chain(raw[..=b].iter()).map(|q| pt(*q)).collect() };
        if pts.len() < 2 {
            pts = vec![pt(simp[i]), pt(simp[j])];
        }
        let chord = pts[0].distance(pts[pts.len() - 1]);
        let (t0, t1) = (dirs[i].0, dirs[j].1);
        let (c, _, _) = if pts.len() >= 3 { fit_single(&pts, t0, t1) } else { (kurbo::CubicBez::new(pts[0], pts[0], pts[1], pts[1]), 0.0, 0) };
        // Keep handles pointing along the tangents and within the chord.
        let max = chord * 0.5 * round.max(0.34);
        let l0 = (c.p1 - c.p0).dot(t0).clamp(0.0, max);
        let l1 = (c.p3 - c.p2).dot(t1).clamp(0.0, max);
        let (l0, l1) = if pts.len() < 3 { (chord / 3.0 * round, chord / 3.0 * round) } else { (l0, l1) };
        path.out_tangents[i] = [t0.x * l0, t0.y * l0];
        path.in_tangents[j] = [-t1.x * l1, -t1.y * l1];
    }
    path
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FillRule, fill_coverage, to_kurbo};
    use effectcraft_geom::Mat3;

    fn iou(a: &[bool], b: &[f32]) -> f64 {
        let (mut i, mut u) = (0.0, 0.0);
        for (x, y) in a.iter().zip(b) {
            let y = *y >= 0.5;
            if *x && y {
                i += 1.0;
            }
            if *x || y {
                u += 1.0;
            }
        }
        i / u
    }

    /// Rasterise traced outlines with even-odd fill (as the Add / Subtract mask stack does).
    fn raster(t: &[Traced], w: usize, h: usize) -> Vec<f32> {
        let paths: Vec<_> = t.iter().map(|t| to_kurbo(&t.path)).collect();
        fill_coverage(&paths, &Mat3::IDENTITY, w as u32, h as u32, FillRule::EvenOdd).data
    }

    #[test]
    fn nesting_depths_match_every_pair() {
        // Rings within rings beside many separate dots: the grid lookup gives the same depths
        // as testing every pair.
        let (w, h) = (200, 120);
        let mut field = vec![0.0f32; w * h];
        for y in 0..h {
            for x in 0..w {
                let (fx, fy) = (x as f64 + 0.5 - 60.0, y as f64 + 0.5 - 60.0);
                let r = (fx * fx + fy * fy).sqrt();
                let ring = r < 56.0 && ((r / 8.0) as usize).is_multiple_of(2);
                let dot = x >= 130 && x % 6 < 3 && y % 6 < 3;
                field[y * w + x] = if ring || dot { 1.0 } else { 0.0 };
            }
        }
        let opts = TraceOpts { min_area: 1.0, ..Default::default() };
        let t = trace(&field, w, h, &opts);
        assert!(t.len() > 200, "{}", t.len());
        let polys: Vec<(Vec<[f64; 2]>, f64)> = t.iter().map(|t| (t.path.vertices.clone(), t.area.abs())).collect();
        let refs: Vec<(&[[f64; 2]], f64)> = polys.iter().map(|(p, a)| (p.as_slice(), *a)).collect();
        let fast = nesting_depths(&refs, w, h);
        let pairs: Vec<usize> =
            (0..refs.len()).map(|i| (0..refs.len()).filter(|&j| j != i && refs[j].1 > refs[i].1 && point_in_poly(refs[i].0[0], refs[j].0)).count()).collect();
        assert_eq!(fast, pairs);
        assert_eq!(fast, t.iter().map(|t| t.depth).collect::<Vec<_>>());
        assert!(fast.iter().max() >= Some(&5));
    }

    #[test]
    fn nesting_depths_count_wide_boxes_kept_out_of_the_grid() {
        // Concentric squares whose boxes each cover far more than MAX_BOX_CELLS cells.
        let (w, h) = (4000, 4000);
        let squares: Vec<Vec<[f64; 2]>> = (0..40)
            .map(|k| {
                let (a, b) = (10.0 + 40.0 * k as f64, 3990.0 - 40.0 * k as f64);
                vec![[a, a], [b, a], [b, b], [a, b]]
            })
            .collect();
        let refs: Vec<(&[[f64; 2]], f64)> = squares.iter().map(|p| (p.as_slice(), (p[1][0] - p[0][0]).powi(2))).collect();
        assert_eq!(nesting_depths(&refs, w, h), (0..40).collect::<Vec<_>>());
    }

    #[test]
    fn simplification_work_is_bounded_and_keeps_the_outline() {
        // Out of budget, the spans still waiting keep every point: nothing is lost, the outline
        // is just less simplified.
        let p: Vec<[f64; 2]> = (0..12).map(|i| [i as f64, if i % 2 == 0 { 0.0 } else { 5.0 }]).collect();
        let (mut budget, mut keep) = (0, vec![]);
        dp(&p, 0, 11, 0.01, &mut budget, &mut keep);
        keep.sort_unstable();
        assert_eq!(keep, (1..11).collect::<Vec<_>>());
        // With a budget, the result matches the plain recursion's on an ordinary loop.
        let (mut budget, mut keep) = (usize::MAX, vec![]);
        dp(&p, 0, 11, 0.01, &mut budget, &mut keep);
        keep.sort_unstable();
        assert_eq!(keep, (1..11).collect::<Vec<_>>());
        // A long, finely zig-zagging loop: simplified within a budget linear in its length.
        let n = 200_000;
        let ring: Vec<[f64; 2]> = (0..n)
            .map(|i| {
                let a = i as f64 / n as f64 * std::f64::consts::TAU;
                let r = 1000.0 + if i % 2 == 0 { 0.0 } else { 3.0 };
                [r * a.cos(), r * a.sin()]
            })
            .collect();
        let idx = simplify_idx(&ring, 0.5);
        assert!(idx.windows(2).all(|w| w[0] < w[1]) && idx.len() >= 3 && *idx.last().unwrap() < n);
    }

    #[test]
    fn fit_uses_the_kept_indices() {
        // A contour that passes the same point twice: the fit follows the simplifier's index,
        // not the first raw point with those coordinates.
        let raw = vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 0.0], [-10.0, 10.0], [-10.0, 0.0]];
        let idx = vec![0, 2, 3, 4];
        let simp: Vec<[f64; 2]> = idx.iter().map(|&i| raw[i]).collect();
        let path = fit_loop(&raw, &idx, &simp, 0.5);
        assert_eq!(path.vertices, simp);
    }

    #[test]
    fn traces_a_disc_and_a_square_with_a_hole() {
        let (w, h) = (120, 80);
        let mut field = vec![0.0f32; w * h];
        let mut truth = vec![false; w * h];
        for y in 0..h {
            for x in 0..w {
                let (fx, fy) = (x as f64 + 0.5, y as f64 + 0.5);
                let disc = (fx - 30.0).powi(2) + (fy - 40.0).powi(2) <= 20.0f64.powi(2);
                let square = (60.0..110.0).contains(&fx) && (15.0..65.0).contains(&fy);
                let hole = (75.0..95.0).contains(&fx) && (30.0..50.0).contains(&fy);
                let on = disc || (square && !hole);
                truth[y * w + x] = on;
                field[y * w + x] = if on { 1.0 } else { 0.0 };
            }
        }
        for round in [0.0, 0.5, 1.0] {
            let t = trace(&field, w, h, &TraceOpts { corner_roundness: round, ..Default::default() });
            assert_eq!(t.len(), 3, "disc, square, hole");
            assert_eq!(t.iter().filter(|t| t.depth == 1).count(), 1, "one hole");
            let r = raster(&t, w, h);
            let score = iou(&truth, &r);
            // Full roundness rounds the square's corners off as well.
            let min = if round >= 1.0 { 0.88 } else { 0.96 };
            assert!(score > min, "IoU {score} at roundness {round}");
        }
        // A disc stays a disc when smooth.
        let disc: Vec<f32> =
            (0..w * h).map(|i| if ((i % w) as f64 + 0.5 - 30.0).powi(2) + ((i / w) as f64 + 0.5 - 40.0).powi(2) <= 400.0 { 1.0 } else { 0.0 }).collect();
        let t = trace(&disc, w, h, &TraceOpts { corner_roundness: 1.0, tolerance: 1.5, ..Default::default() });
        let truth_disc: Vec<bool> = disc.iter().map(|v| *v > 0.5).collect();
        let score = iou(&truth_disc, &raster(&t, w, h));
        assert!(score > 0.97, "disc IoU {score}");
        assert!(t[0].path.vertices.len() < 40, "simplified to {}", t[0].path.vertices.len());
        // The square traces to 4 corners at zero roundness.
        let t = trace(&field, w, h, &TraceOpts { corner_roundness: 0.0, tolerance: 1.0, ..Default::default() });
        let sq = t.iter().find(|t| t.depth == 0 && t.path.vertices.iter().all(|v| v[0] > 55.0)).expect("square");
        assert!(sq.path.vertices.len() <= 8, "{:?}", sq.path.vertices);
    }

    #[test]
    fn min_area_and_threshold() {
        let (w, h) = (40, 40);
        let mut field = vec![0.0f32; w * h];
        // A 2×2 speck and a 20×20 block at 0.6.
        for y in 2..4 {
            for x in 2..4 {
                field[y * w + x] = 1.0;
            }
        }
        for y in 10..30 {
            for x in 10..30 {
                field[y * w + x] = 0.6;
            }
        }
        assert_eq!(trace(&field, w, h, &TraceOpts { min_area: 10.0, ..Default::default() }).len(), 1);
        assert_eq!(trace(&field, w, h, &TraceOpts { min_area: 1.0, ..Default::default() }).len(), 2);
        // Above 0.6 only the speck remains (and it is too small).
        assert!(trace(&field, w, h, &TraceOpts { threshold: 0.7, ..Default::default() }).is_empty());
        // Edge-touching shapes close along the border.
        let full = vec![1.0f32; 100];
        let t = trace(&full, 10, 10, &TraceOpts::default());
        assert_eq!(t.len(), 1);
        assert!((t[0].area.abs() - 100.0).abs() < 2.0, "{}", t[0].area);
    }
}

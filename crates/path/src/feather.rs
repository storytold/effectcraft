//! Variable-width mask feather (After Effects' Mask Feather tool).
//!
//! Feather points sit on the mask path with a signed radius (outer > 0, inner < 0) and a
//! tension. Between points the outer and inner radii are interpolated along the path's arc
//! length (smoothly at tension 0, linearly at tension 1; closed paths wrap around). Every pixel
//! near the path takes the feather width at its nearest path position: outside the path the
//! opacity falls from 1 at the edge to 0 at the outer radius, inside it rises from 0 at the
//! edge to 1 at the inner radius. Where no radius applies the edge stays hard (anti-aliased).

use effectcraft_geom::{Mat3, vec2};
use effectcraft_keyframe::{FeatherPoint, ShapePath};
use effectcraft_raster::Mask;

use crate::{FillRule, fill_coverage, to_kurbo};

/// A flattened path in buffer space: points, their arc lengths, and per segment the
/// `(parameter, arc length)` table used to place feather points.
struct Flat {
    pts: Vec<[f64; 2]>,
    s: Vec<f64>,
    seg_tables: Vec<Vec<(f64, f64)>>,
    len: f64,
}

fn cubic(p0: [f64; 2], p1: [f64; 2], p2: [f64; 2], p3: [f64; 2], t: f64) -> [f64; 2] {
    let u = 1.0 - t;
    let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
    [a * p0[0] + b * p1[0] + c * p2[0] + d * p3[0], a * p0[1] + b * p1[1] + c * p2[1] + d * p3[1]]
}

fn flatten(sp: &ShapePath, m: &Mat3) -> Flat {
    let n = sp.vertices.len();
    let tr = |p: [f64; 2]| {
        let r = m.apply(vec2(p[0], p[1]));
        [r.x, r.y]
    };
    let mut pts: Vec<[f64; 2]> = Vec::new();
    let mut s: Vec<f64> = Vec::new();
    let mut seg_tables = Vec::new();
    let segs = sp.segment_count();
    let mut acc = 0.0;
    for i in 0..segs {
        let j = (i + 1) % n;
        let v0 = sp.vertices[i];
        let v1 = sp.vertices[j];
        let o = sp.out_tangents.get(i).copied().unwrap_or([0.0; 2]);
        let it = sp.in_tangents.get(j).copied().unwrap_or([0.0; 2]);
        let (p0, p1, p2, p3) = (tr(v0), tr([v0[0] + o[0], v0[1] + o[1]]), tr([v1[0] + it[0], v1[1] + it[1]]), tr(v1));
        let est = ((p1[0] - p0[0]).hypot(p1[1] - p0[1]) + (p2[0] - p1[0]).hypot(p2[1] - p1[1]) + (p3[0] - p2[0]).hypot(p3[1] - p2[1])).max(1.0);
        let k = ((est / 1.5).ceil() as usize).clamp(4, 1024);
        let mut table = Vec::with_capacity(k + 1);
        if pts.is_empty() {
            pts.push(p0);
            s.push(0.0);
        }
        table.push((0.0, acc));
        let mut prev = p0;
        for q in 1..=k {
            let t = q as f64 / k as f64;
            let p = cubic(p0, p1, p2, p3, t);
            acc += (p[0] - prev[0]).hypot(p[1] - prev[1]);
            pts.push(p);
            s.push(acc);
            table.push((t, acc));
            prev = p;
        }
        seg_tables.push(table);
    }
    Flat { pts, s, seg_tables, len: acc }
}

impl Flat {
    /// Keep every other point (and the last), with their arc lengths.
    fn thin(&mut self) {
        let n = self.pts.len();
        let keep = |i: usize| i.is_multiple_of(2) || i + 1 == n;
        let mut i = 0;
        self.pts.retain(|_| {
            i += 1;
            keep(i - 1)
        });
        let mut i = 0;
        self.s.retain(|_| {
            i += 1;
            keep(i - 1)
        });
    }

    /// Arc length of a feather point.
    fn arc_of(&self, f: &FeatherPoint) -> Option<f64> {
        let table = self.seg_tables.get(f.segment)?;
        let t = f.t.clamp(0.0, 1.0);
        let i = table.partition_point(|e| e.0 <= t).clamp(1, table.len() - 1);
        let (a, b) = (table[i - 1], table[i]);
        let u = if b.0 > a.0 { (t - a.0) / (b.0 - a.0) } else { 0.0 };
        Some(a.1 + (b.1 - a.1) * u)
    }
}

/// Outer / inner feather radius (buffer pixels) along the path, from the feather points.
struct Profile {
    /// (arc length, outer radius, inner radius, tension), sorted by arc length.
    pts: Vec<(f64, f64, f64, f64)>,
    len: f64,
    closed: bool,
}

impl Profile {
    fn at(&self, u: f64) -> (f64, f64) {
        let p = &self.pts;
        if p.is_empty() {
            return (0.0, 0.0);
        }
        if p.len() == 1 {
            return (p[0].1, p[0].2);
        }
        let blend = |a: &(f64, f64, f64, f64), b: &(f64, f64, f64, f64), x: f64| {
            let x = x.clamp(0.0, 1.0);
            let smooth = x * x * (3.0 - 2.0 * x);
            let tension = ((a.3 + b.3) * 0.5).clamp(0.0, 1.0);
            let w = smooth + (x - smooth) * tension;
            (a.1 + (b.1 - a.1) * w, a.2 + (b.2 - a.2) * w)
        };
        let first = p[0];
        let last = p[p.len() - 1];
        if u < first.0 || u >= last.0 {
            if !self.closed {
                return if u < first.0 { (first.1, first.2) } else { (last.1, last.2) };
            }
            // Wrap from the last point to the first through the path's start.
            let span = (self.len - last.0) + first.0;
            let x = if u >= last.0 { u - last.0 } else { u + self.len - last.0 };
            return blend(&last, &first, if span > 1e-9 { x / span } else { 0.0 });
        }
        let i = p.partition_point(|e| e.0 <= u).clamp(1, p.len() - 1);
        let (a, b) = (&p[i - 1], &p[i]);
        let span = b.0 - a.0;
        blend(a, b, if span > 1e-9 { (u - a.0) / span } else { 0.0 })
    }
}

/// Most pixel visits (summed over the flattened path's segments) a variable feather's distance
/// pass may take: a few seconds of work, far beyond a feathered mask at 4K with a radius of a
/// few hundred pixels. Past it the flattened path is thinned (see [`variable_feather_coverage`]).
pub const FEATHER_WORK_BUDGET: u64 = 1 << 31;

/// Pixel visits of the distance pass over the polyline `pts`: each segment's box grown by
/// `reach`, clipped to the `w × h` mask.
fn feather_work(pts: &[[f64; 2]], reach: f64, w: u32, h: u32) -> u64 {
    let span = |lo: f64, hi: f64, n: u32| -> u64 {
        let a = (lo - reach).floor().max(0.0);
        let b = (hi + reach).ceil().min(f64::from(n) - 1.0);
        if b >= a { (b - a + 1.0) as u64 } else { 0 }
    };
    pts.windows(2)
        .map(|p| {
            let (a, b) = (p[0], p[1]);
            span(a[0].min(b[0]), a[0].max(b[0]), w).saturating_mul(span(a[1].min(b[1]), a[1].max(b[1]), h))
        })
        .fold(0u64, u64::saturating_add)
}

/// Coverage of a mask path with feather points, transformed by `m` (translate + uniform scale)
/// into a `w × h` mask. `expansion` (layer pixels) grows (>0) or shrinks the mask first;
/// `linear` selects the Linear feather falloff instead of Smooth.
pub fn variable_feather_coverage(sp: &ShapePath, m: &Mat3, w: u32, h: u32, expansion: f64, linear: bool) -> Mask {
    coverage_within(sp, m, (w, h), expansion, linear, FEATHER_WORK_BUDGET)
}

fn coverage_within(sp: &ShapePath, m: &Mat3, (w, h): (u32, u32), expansion: f64, linear: bool, budget: u64) -> Mask {
    let scale = m.mean_scale();
    let path = to_kurbo(sp);
    let hard = fill_coverage(std::slice::from_ref(&path), m, w, h, FillRule::NonZero);
    let mut flat = flatten(sp, m);
    if flat.pts.len() < 2 {
        return hard;
    }
    let mut pts: Vec<(f64, f64, f64, f64)> = sp
        .feather
        .iter()
        .filter_map(|f| {
            let u = flat.arc_of(f)?;
            let r = f.radius * scale;
            Some((u, r.max(0.0), (-r).max(0.0), f.tension))
        })
        .collect();
    pts.sort_by(|a, b| a.0.total_cmp(&b.0));
    let prof = Profile { pts, len: flat.len, closed: sp.closed };
    let rmax = prof.pts.iter().map(|p| p.1.max(p.2)).fold(0.0, f64::max);
    let exp = expansion * scale;
    let reach = rmax + exp.abs() + 2.0;
    // Each flattened segment visits its box grown by the reach, so many segments with a reach
    // spanning the mask would revisit every pixel once per segment. Past the budget the
    // polyline is thinned (every other point, keeping their arc lengths) until it fits: the
    // distances move by a fraction of the chords, small next to a feather that wide.
    while feather_work(&flat.pts, reach, w, h) > budget {
        if flat.pts.len() <= 8 {
            return hard;
        }
        flat.thin();
    }
    let (wu, hu) = (w as usize, h as usize);
    let mut dist = vec![f32::INFINITY; wu * hu];
    let mut arc = vec![0.0f32; wu * hu];
    for k in 0..flat.pts.len() - 1 {
        let (a, b) = (flat.pts[k], flat.pts[k + 1]);
        let (sa, sb) = (flat.s[k], flat.s[k + 1]);
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let l2 = dx * dx + dy * dy;
        let x0 = ((a[0].min(b[0]) - reach).floor().max(0.0)) as usize;
        let x1 = ((a[0].max(b[0]) + reach).ceil().min(w as f64 - 1.0)).max(-1.0) as isize;
        let y0 = ((a[1].min(b[1]) - reach).floor().max(0.0)) as usize;
        let y1 = ((a[1].max(b[1]) + reach).ceil().min(h as f64 - 1.0)).max(-1.0) as isize;
        if x1 < 0 || y1 < 0 {
            continue;
        }
        for y in y0..=y1 as usize {
            for x in x0..=x1 as usize {
                let (px, py) = (x as f64 + 0.5, y as f64 + 0.5);
                let t = if l2 > 0.0 { (((px - a[0]) * dx + (py - a[1]) * dy) / l2).clamp(0.0, 1.0) } else { 0.0 };
                let (qx, qy) = (a[0] + dx * t - px, a[1] + dy * t - py);
                let d = (qx * qx + qy * qy).sqrt() as f32;
                let i = y * wu + x;
                if d < dist[i] {
                    dist[i] = d;
                    arc[i] = (sa + (sb - sa) * t) as f32;
                }
            }
        }
    }
    let ramp = |x: f64| {
        let x = x.clamp(0.0, 1.0);
        if linear { x } else { x * x * (3.0 - 2.0 * x) }
    };
    let mut out = Mask::new(w, h, 0.0);
    for (i, o) in out.data.iter_mut().enumerate() {
        let inside = hard.data[i] >= 0.5;
        let d = dist[i] as f64;
        if !d.is_finite() || d > reach {
            // Farther from the path than any feather or expansion reaches.
            *o = if inside { 1.0 } else { 0.0 };
            continue;
        }
        // Signed distance to the expanded edge (negative inside).
        let sd = if inside { -d } else { d } - exp;
        let (ro, ri) = prof.at(arc[i] as f64);
        let a = if sd > 0.0 {
            if ro > 0.5 { ramp(1.0 - sd / ro) } else { (0.5 - sd).clamp(0.0, 1.0) }
        } else if ri > 0.5 {
            ramp(-sd / ri)
        } else {
            (0.5 - sd).clamp(0.0, 1.0)
        };
        *o = a as f32;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square() -> ShapePath {
        // 100×100 square from (50,50) to (150,150), clockwise: top, right, bottom, left segments.
        ShapePath::rect([100.0, 100.0], 100.0, 100.0)
    }

    fn at(m: &Mask, x: u32, y: u32) -> f32 {
        m.data[(y * m.width + x) as usize]
    }

    #[test]
    fn feather_work_is_bounded_by_thinning_the_path() {
        // Thousands of vertices with a feather reaching across the mask: the polyline is thinned
        // until the distance pass fits the budget, and the result is still a feathered edge.
        let n = 2000;
        let verts: Vec<[f64; 2]> = (0..n)
            .map(|i| {
                let a = i as f64 / n as f64 * std::f64::consts::TAU;
                [100.0 + 50.0 * a.cos(), 100.0 + 50.0 * a.sin()]
            })
            .collect();
        let mut sp = ShapePath::polygon(&verts, true);
        sp.feather = vec![FeatherPoint { segment: 0, t: 0.5, radius: 40.0, tension: 0.0 }];
        let budget = 1 << 22;
        let full = flatten(&sp, &Mat3::IDENTITY);
        assert!(feather_work(&full.pts, 42.0, 200, 200) > budget);
        let m = coverage_within(&sp, &Mat3::IDENTITY, (200, 200), 0.0, true, budget);
        let unbounded = coverage_within(&sp, &Mat3::IDENTITY, (200, 200), 0.0, true, u64::MAX);
        // Outside the edge: 1 at the circle falling linearly to 0 at 40 px, close to the
        // unthinned result.
        assert!(at(&m, 100, 100) > 0.99);
        let (thin, exact) = (at(&m, 100, 30), at(&unbounded, 100, 30));
        assert!(thin > 0.4 && thin < 0.6 && (thin - exact).abs() < 0.05, "{thin} {exact}");
        assert!(at(&m, 100, 5) < 0.01);
    }

    #[test]
    fn no_points_is_the_hard_mask() {
        let m = variable_feather_coverage(&square(), &Mat3::IDENTITY, 200, 200, 0.0, false);
        assert!(at(&m, 100, 100) > 0.99 && at(&m, 40, 100) < 0.01);
    }

    #[test]
    fn single_outer_point_feathers_all_around() {
        let mut sp = square();
        sp.feather = vec![FeatherPoint { segment: 0, t: 0.5, radius: 20.0, tension: 0.0 }];
        let m = variable_feather_coverage(&sp, &Mat3::IDENTITY, 200, 200, 0.0, true);
        // Above the top edge: 1 at the edge falling linearly to 0 at 20 px.
        assert!((at(&m, 100, 39) - 0.475).abs() < 0.06, "{}", at(&m, 100, 39));
        assert!(at(&m, 100, 28) < 0.01);
        assert!(at(&m, 100, 60) > 0.99, "inside stays opaque");
        // One point applies to the whole closed path.
        assert!((at(&m, 161, 100) - 0.475).abs() < 0.06, "{}", at(&m, 161, 100));
    }

    #[test]
    fn outer_and_inner_points_vary_along_the_path() {
        let mut sp = square();
        // Outer 20 px in the middle of the top edge, inner 20 px in the middle of the bottom edge.
        sp.feather = vec![FeatherPoint { segment: 0, t: 0.5, radius: 20.0, tension: 1.0 }, FeatherPoint { segment: 2, t: 0.5, radius: -20.0, tension: 1.0 }];
        let m = variable_feather_coverage(&sp, &Mat3::IDENTITY, 200, 200, 0.0, true);
        // Top: soft outside, hard inside.
        assert!(at(&m, 100, 45) > 0.6 && at(&m, 100, 45) < 0.8, "{}", at(&m, 100, 45));
        assert!(at(&m, 100, 52) > 0.99);
        // Bottom: hard outside, soft inside (0 at the edge, 1 at 20 px in).
        assert!(at(&m, 100, 152) < 0.01);
        assert!((at(&m, 100, 140) - 0.475).abs() < 0.06, "{}", at(&m, 100, 140));
        // Half way round (middle of the right edge): both radii are half way, 10 px.
        assert!((at(&m, 154, 100) - 0.55).abs() < 0.05, "{}", at(&m, 154, 100));
        assert!((at(&m, 145, 100) - 0.45).abs() < 0.05, "{}", at(&m, 145, 100));
    }

    #[test]
    fn scale_and_expansion() {
        let mut sp = square();
        sp.feather = vec![FeatherPoint { segment: 0, t: 0.5, radius: 10.0, tension: 0.0 }];
        // Half resolution: the 10 px feather is 5 buffer pixels.
        let m = variable_feather_coverage(&sp, &Mat3::scale(vec2(0.5, 0.5)), 100, 100, 0.0, true);
        assert!(at(&m, 50, 22) > 0.4 && at(&m, 50, 22) < 0.6, "{}", at(&m, 50, 22));
        let e = variable_feather_coverage(&sp, &Mat3::IDENTITY, 200, 200, 10.0, true);
        assert!(at(&e, 100, 41) > 0.99, "expanded by 10 px: {}", at(&e, 100, 41));
        assert!((at(&e, 100, 34) - 0.45).abs() < 0.05, "{}", at(&e, 100, 34));
    }
}

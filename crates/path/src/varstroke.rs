//! Variable-width strokes: the shape Stroke's **Taper** (start/end length, width and ease) and
//! **Wave** (amount, wavelength or cycles, phase), with dashes cut along the same arc length.
//!
//! The stroke is built as a union of positively-wound primitives: one trapezoid per flattened
//! segment (its two ends at the local widths), a disc at every bend (round joins) and the caps.
//! Filled with the non-zero rule the union is exact, whatever the width profile does.

use kurbo::{BezPath, PathEl, Point, Shape, Vec2};

use crate::{Cap, StrokeStyle};

/// Stroke Taper. Lengths are pixels, or fractions of each path's length when `percent`; widths and
/// eases are fractions (0–1). A width of 0 tapers to a point.
#[derive(Clone, Debug, PartialEq)]
pub struct Taper {
    pub percent: bool,
    pub start_len: f64,
    pub end_len: f64,
    pub start_width: f64,
    pub end_width: f64,
    pub start_ease: f64,
    pub end_ease: f64,
}

impl Taper {
    pub fn is_active(&self) -> bool {
        (self.start_len > 0.0 && self.start_width < 1.0) || (self.end_len > 0.0 && self.end_width < 1.0)
    }
}

/// Stroke Wave: the width oscillates between `1 - amount` and 1 of the stroke width.
#[derive(Clone, Debug, PartialEq)]
pub struct Wave {
    /// 0–1.
    pub amount: f64,
    /// Units: `Some(cycles)` = cycles along each path, else `wavelength` in pixels.
    pub cycles: Option<f64>,
    pub wavelength: f64,
    /// Degrees.
    pub phase: f64,
}

impl Wave {
    pub fn is_active(&self) -> bool {
        self.amount > 0.0 && self.cycles.map_or(self.wavelength > 0.0, |c| c > 0.0)
    }
}

/// Ease profile from the tip (`u = 0`) to full width (`u = 1`): linear at ease 0, a quarter
/// circle (bulging outwards) at ease 1.
fn ease(u: f64, e: f64) -> f64 {
    let u = u.clamp(0.0, 1.0);
    let round = (1.0 - (1.0 - u) * (1.0 - u)).max(0.0).sqrt();
    u + (round - u) * e.clamp(0.0, 1.0)
}

/// Width factor (0–1) at arc length `s` of a path `len` long.
pub fn width_factor(taper: Option<&Taper>, wave: Option<&Wave>, s: f64, len: f64) -> f64 {
    let mut k = 1.0;
    if let Some(t) = taper.filter(|t| t.is_active()) {
        let (mut ls, mut le) = if t.percent { (t.start_len * len, t.end_len * len) } else { (t.start_len, t.end_len) };
        ls = ls.max(0.0);
        le = le.max(0.0);
        // Overlapping tapers share the path in proportion.
        if ls + le > len && ls + le > 0.0 {
            let f = len / (ls + le);
            ls *= f;
            le *= f;
        }
        let sw = t.start_width.clamp(0.0, 1.0);
        let ew = t.end_width.clamp(0.0, 1.0);
        if ls > 0.0 && s < ls {
            k = f64::min(k, sw + (1.0 - sw) * ease(s / ls, t.start_ease));
        }
        if le > 0.0 && s > len - le {
            k = f64::min(k, ew + (1.0 - ew) * ease((len - s) / le, t.end_ease));
        }
    }
    if let Some(w) = wave.filter(|w| w.is_active()) {
        let lambda = match w.cycles {
            Some(c) => len / c,
            None => w.wavelength,
        };
        if lambda > 0.0 {
            let ph = std::f64::consts::TAU * s / lambda + w.phase.to_radians();
            k *= 1.0 - w.amount.clamp(0.0, 1.0) * (0.5 - 0.5 * ph.cos());
        }
    }
    k
}

/// A flattened subpath: points and their cumulative arc lengths.
struct Poly {
    pts: Vec<Point>,
    s: Vec<f64>,
    closed: bool,
}

impl Poly {
    fn len(&self) -> f64 {
        self.s.last().copied().unwrap_or(0.0)
    }
    /// Point at arc length `t` (clamped), and the index of the segment it lies on.
    fn at(&self, t: f64) -> (Point, usize) {
        let i = self.s.partition_point(|&x| x <= t).clamp(1, self.pts.len() - 1);
        let (a, b) = (self.s[i - 1], self.s[i]);
        let u = if b > a { ((t - a) / (b - a)).clamp(0.0, 1.0) } else { 0.0 };
        (self.pts[i - 1].lerp(self.pts[i], u), i)
    }
}

/// Flatten each subpath, then subdivide so no segment is longer than `step`.
fn flatten(path: &BezPath, tol: f64, step: f64) -> Vec<Poly> {
    let mut out: Vec<Poly> = Vec::new();
    let mut cur: Vec<Point> = Vec::new();
    let mut closed = false;
    let flush = |cur: &mut Vec<Point>, closed: bool, out: &mut Vec<Poly>| {
        let mut pts: Vec<Point> = Vec::with_capacity(cur.len());
        for &p in cur.iter() {
            if pts.last().is_none_or(|q: &Point| q.distance(p) > 1e-9) {
                pts.push(p);
            }
        }
        if closed && pts.len() > 2 && pts[0].distance(pts[pts.len() - 1]) > 1e-9 {
            pts.push(pts[0]);
        }
        cur.clear();
        if pts.len() < 2 {
            return;
        }
        // Subdivide.
        let mut fine = vec![pts[0]];
        for w in pts.windows(2) {
            let d = w[0].distance(w[1]);
            let n = (d / step).ceil().max(1.0) as usize;
            for k in 1..=n {
                fine.push(w[0].lerp(w[1], k as f64 / n as f64));
            }
        }
        let mut s = Vec::with_capacity(fine.len());
        let mut acc = 0.0;
        s.push(0.0);
        for w in fine.windows(2) {
            acc += w[0].distance(w[1]);
            s.push(acc);
        }
        out.push(Poly { pts: fine, s, closed });
    };
    kurbo::flatten(path.elements().iter().copied(), tol, |el| match el {
        PathEl::MoveTo(p) => {
            flush(&mut cur, closed, &mut out);
            closed = false;
            cur.push(p);
        }
        PathEl::LineTo(p) => cur.push(p),
        PathEl::ClosePath => {
            closed = true;
            flush(&mut cur, closed, &mut out);
            closed = false;
        }
        _ => {}
    });
    flush(&mut cur, closed, &mut out);
    out
}

/// Arc-length intervals that are "on" for a dash pattern on a path `len` long.
pub fn dash_intervals(pattern: &[f64], offset: f64, len: f64) -> Vec<(f64, f64)> {
    let pat: Vec<f64> = pattern.iter().map(|v| v.max(0.0)).collect();
    let pat = if pat.len() % 2 == 1 { [pat.clone(), pat].concat() } else { pat };
    let period: f64 = pat.iter().sum();
    if pat.len() < 2 || period <= 1e-9 || len <= 0.0 {
        return vec![(0.0, len)];
    }
    // Pattern position at s = 0.
    let mut pos = offset.rem_euclid(period);
    let mut idx = 0;
    while pos >= pat[idx] {
        pos -= pat[idx];
        idx = (idx + 1) % pat.len();
    }
    let mut s = 0.0;
    let mut out = Vec::new();
    let mut guard = 0usize;
    while s < len && guard < 1_000_000 {
        guard += 1;
        let seg = pat[idx] - pos;
        let e = (s + seg).min(len);
        if idx % 2 == 0 && e >= s {
            out.push((s, e));
        }
        s += seg;
        pos = 0.0;
        idx = (idx + 1) % pat.len();
    }
    out
}

/// Append `p` to `out`, wound positively.
fn push_pos(out: &mut BezPath, p: BezPath) {
    let p = if p.area() < 0.0 { p.reverse_subpaths() } else { p };
    out.extend(p.elements().iter().copied());
}

fn quad(out: &mut BezPath, pts: [Point; 4]) {
    let mut p = BezPath::new();
    p.move_to(pts[0]);
    p.line_to(pts[1]);
    p.line_to(pts[2]);
    p.line_to(pts[3]);
    p.close_path();
    if p.area().abs() > 1e-12 {
        push_pos(out, p);
    }
}

fn disc(out: &mut BezPath, c: Point, r: f64) {
    if r > 1e-6 {
        push_pos(out, kurbo::Circle::new(c, r).to_path(0.05));
    }
}

/// `path` with each curve whose extent is over 10⁷ × `tol` replaced by lines flattened at 10⁻⁷ of
/// that extent. `None` when a coordinate is not finite.
fn coarsen_huge(path: &BezPath, tol: f64) -> Option<BezPath> {
    use kurbo::Shape as _;
    let mut out = BezPath::new();
    let (mut cur, mut start) = (Point::ZERO, Point::ZERO);
    for el in path.elements() {
        let seg = match *el {
            PathEl::MoveTo(p) => {
                (cur, start) = (p, p);
                out.push(*el);
                continue;
            }
            PathEl::LineTo(p) => {
                cur = p;
                out.push(*el);
                continue;
            }
            PathEl::ClosePath => {
                cur = start;
                out.push(*el);
                continue;
            }
            PathEl::QuadTo(_, p) | PathEl::CurveTo(_, _, p) => {
                let mut one = BezPath::new();
                one.move_to(cur);
                one.push(*el);
                (one, p)
            }
        };
        let (one, end) = seg;
        let bb = one.bounding_box();
        let extent = bb.width().max(bb.height());
        if !extent.is_finite() {
            return None;
        }
        if extent * 1e-7 > tol {
            kurbo::flatten(one.elements().iter().copied(), extent * 1e-7, |e| {
                if let PathEl::LineTo(p) = e {
                    out.line_to(p);
                }
            });
        } else {
            out.push(*el);
        }
        cur = end;
    }
    Some(out)
}

/// Outline of a tapered / waved stroke (local space; `res_scale` = device/local scale).
pub fn outline(paths: &[BezPath], style: &StrokeStyle, res_scale: f64) -> Option<BezPath> {
    if style.width <= 0.0 {
        return None;
    }
    let rs = res_scale.max(0.01);
    let mut out = BezPath::new();
    let half = style.width / 2.0;
    for path in paths {
        // Fine enough for the width profile: ½ device pixel, at most ~20 000 samples per path.
        // Curves with huge extents are first cut into lines at a tolerance of 10⁻⁷ of their own
        // extent, so none subdivides into billions of points before the step below applies;
        // every other segment (small curves in a large path included) keeps the ½-pixel tolerance.
        let Some(path) = coarsen_huge(path, 0.1 / rs) else { continue };
        let rough = flatten(&path, 0.1 / rs, f64::INFINITY);
        for poly0 in rough {
            let len0 = poly0.len();
            if len0 <= 0.0 {
                continue;
            }
            let step = (0.5 / rs).max(len0 / 20_000.0);
            let mut bp = BezPath::new();
            bp.move_to(poly0.pts[0]);
            for p in &poly0.pts[1..] {
                bp.line_to(*p);
            }
            let poly = flatten(&bp, 0.1 / rs, step).pop()?;
            let poly = Poly { closed: poly0.closed, ..poly };
            let len = poly.len();
            let wf = |s: f64| half * width_factor(style.taper.as_ref(), style.wave.as_ref(), s, len);
            let intervals = match &style.dash {
                Some((d, off)) if d.iter().any(|v| *v > 0.0) => dash_intervals(d, *off, len),
                _ => vec![(0.0, len)],
            };
            let whole = intervals.len() == 1 && intervals[0].0 <= 0.0 && intervals[0].1 >= len;
            for (a, b) in intervals {
                // The piece's points: interpolated ends plus every vertex strictly inside.
                let (pa, ia) = poly.at(a);
                let (pb, ib) = poly.at(b);
                let mut pts = vec![(pa, a)];
                for i in ia..ib {
                    pts.push((poly.pts[i], poly.s[i]));
                }
                pts.push((pb, b));
                pts.dedup_by(|x, y| x.0.distance(y.0) < 1e-9);
                if pts.len() < 2 {
                    // A zero-length dash: only its caps show.
                    if matches!(style.cap, Cap::Round) {
                        disc(&mut out, pa, wf(a));
                    }
                    continue;
                }
                let closed = whole && poly.closed;
                let dir = |i: usize| (pts[i + 1].0 - pts[i].0).normalize();
                for i in 0..pts.len() - 1 {
                    let d = dir(i);
                    let n = Vec2::new(-d.y, d.x);
                    let (p0, s0) = pts[i];
                    let (p1, s1) = pts[i + 1];
                    let (w0, w1) = (wf(s0), wf(s1));
                    quad(&mut out, [p0 + n * w0, p1 + n * w1, p1 - n * w1, p0 - n * w0]);
                    // Joins: a disc where the direction turns.
                    if i > 0 && dir(i - 1).dot(d) < 0.9999 {
                        disc(&mut out, p0, w0);
                    }
                }
                if closed {
                    disc(&mut out, pts[0].0, wf(0.0));
                    continue;
                }
                // Caps.
                let last = pts.len() - 1;
                let ends = [(pts[0].0, wf(pts[0].1), -dir(0)), (pts[last].0, wf(pts[last].1), dir(last - 1))];
                for (p, w, outward) in ends {
                    match style.cap {
                        Cap::Butt => {}
                        Cap::Round => disc(&mut out, p, w),
                        Cap::Square => {
                            let n = Vec2::new(-outward.y, outward.x);
                            let q = p + outward * w;
                            quad(&mut out, [p + n * w, q + n * w, q - n * w, p - n * w]);
                        }
                    }
                }
            }
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FillRule, fill_coverage};
    use effectcraft_geom::Mat3;

    fn line(len: f64) -> BezPath {
        let mut p = BezPath::new();
        p.move_to((10.0, 50.0));
        p.line_to((10.0 + len, 50.0));
        p
    }

    fn column(m: &effectcraft_raster::Mask, x: usize) -> f32 {
        (0..m.height as usize).map(|y| m.data[y * m.width as usize + x]).sum()
    }

    #[test]
    fn taper_width_profile() {
        let t = Taper { percent: false, start_len: 50.0, end_len: 0.0, start_width: 0.0, end_width: 1.0, start_ease: 0.0, end_ease: 0.0 };
        assert!(width_factor(Some(&t), None, 0.0, 200.0).abs() < 1e-9);
        assert!((width_factor(Some(&t), None, 25.0, 200.0) - 0.5).abs() < 1e-9);
        assert!((width_factor(Some(&t), None, 100.0, 200.0) - 1.0).abs() < 1e-9);
        // Ease bulges the profile outwards.
        let e = Taper { start_ease: 1.0, ..t.clone() };
        assert!(width_factor(Some(&e), None, 25.0, 200.0) > 0.8);
        // Percent units and overlapping tapers.
        let p = Taper { percent: true, start_len: 0.75, end_len: 0.75, start_width: 0.0, end_width: 0.0, ..t };
        assert!((width_factor(Some(&p), None, 100.0, 200.0) - 1.0).abs() < 1e-9, "meet in the middle at full width");
        assert!(width_factor(Some(&p), None, 50.0, 200.0) < 0.6);
    }

    #[test]
    fn wave_width_profile() {
        let w = Wave { amount: 1.0, cycles: None, wavelength: 40.0, phase: 0.0 };
        assert!((width_factor(None, Some(&w), 0.0, 200.0) - 1.0).abs() < 1e-9);
        assert!(width_factor(None, Some(&w), 20.0, 200.0).abs() < 1e-9);
        let c = Wave { cycles: Some(5.0), ..w.clone() };
        assert!(width_factor(None, Some(&c), 20.0, 200.0).abs() < 1e-9, "5 cycles over 200 px = 40 px");
        let ph = Wave { phase: 180.0, ..w };
        assert!(width_factor(None, Some(&ph), 0.0, 200.0).abs() < 1e-9);
    }

    #[test]
    fn tapered_line_coverage_grows_along_the_path() {
        let t = Taper { percent: false, start_len: 100.0, end_len: 0.0, start_width: 0.0, end_width: 1.0, start_ease: 0.0, end_ease: 0.0 };
        let st = StrokeStyle { width: 20.0, taper: Some(t), ..Default::default() };
        let o = outline(&[line(150.0)], &st, 1.0).unwrap();
        let m = fill_coverage(&[o], &Mat3::IDENTITY, 200, 100, FillRule::NonZero);
        let (c1, c2, c3) = (column(&m, 30), column(&m, 70), column(&m, 140));
        assert!((c1 - 4.0).abs() < 1.0, "20% along the taper → 4 px: {c1}");
        assert!((c2 - 12.0).abs() < 1.0, "{c2}");
        assert!((c3 - 20.0).abs() < 0.6, "{c3}");
        let area: f32 = m.data.iter().sum();
        assert!((area - (100.0 * 10.0 + 50.0 * 20.0)).abs() < 40.0, "{area}");
    }

    #[test]
    fn dash_intervals_multi_segment() {
        let iv = dash_intervals(&[10.0, 5.0, 2.0, 3.0], 0.0, 45.0);
        assert_eq!(iv, vec![(0.0, 10.0), (15.0, 17.0), (20.0, 30.0), (35.0, 37.0), (40.0, 45.0)]);
        let off = dash_intervals(&[10.0, 10.0], 5.0, 30.0);
        assert_eq!(off, vec![(0.0, 5.0), (15.0, 25.0)]);
    }

    #[test]
    fn waved_dashed_stroke_has_gaps() {
        let st = StrokeStyle {
            width: 10.0,
            dash: Some((vec![20.0, 10.0], 0.0)),
            wave: Some(Wave { amount: 0.5, cycles: None, wavelength: 30.0, phase: 0.0 }),
            ..Default::default()
        };
        let o = outline(&[line(150.0)], &st, 1.0).unwrap();
        let m = fill_coverage(&[o], &Mat3::IDENTITY, 200, 100, FillRule::NonZero);
        assert!(column(&m, 41) > 9.0, "dash at full width (wave crest): {}", column(&m, 41));
        assert!(column(&m, 35) < 0.01, "gap");
        assert!(column(&m, 55) > 4.0 && column(&m, 55) < 6.5, "wave trough ≈ half width: {}", column(&m, 55));
    }

    #[test]
    fn closed_tapered_path_is_continuous() {
        let r = crate::rect([60.0, 60.0], [50.0, 50.0], 0.0);
        let st = StrokeStyle { width: 6.0, wave: Some(Wave { amount: 0.2, cycles: Some(4.0), wavelength: 0.0, phase: 0.0 }), ..Default::default() };
        let o = outline(&[r], &st, 1.0).unwrap();
        let m = fill_coverage(&[o], &Mat3::IDENTITY, 100, 100, FillRule::NonZero);
        assert!(m.data.iter().all(|v| *v <= 1.0001));
        let area: f32 = m.data.iter().sum();
        assert!(area > 240.0 * 6.0 * 0.8 && area < 240.0 * 6.0 * 1.05, "{area}");
    }

    /// A curve with enormous coordinates is approximated within a bounded number of points.
    #[test]
    fn huge_coordinates_do_not_explode_the_first_approximation() {
        let mut p = BezPath::new();
        p.move_to((0.0, 0.0));
        p.curve_to((1e13, 0.0), (-1e13, 1e13), (1e13, 1e13));
        let st = StrokeStyle {
            width: 4.0,
            taper: Some(Taper { percent: true, start_len: 50.0, end_len: 50.0, start_width: 0.0, end_width: 0.0, start_ease: 0.0, end_ease: 0.0 }),
            ..Default::default()
        };
        let t = std::time::Instant::now();
        let o = outline(&[p], &st, 1.0).unwrap();
        assert!(o.elements().len() < 200_000, "{}", o.elements().len());
        assert!(t.elapsed().as_secs() < 20);
    }

    /// Small curves keep their shape in a path that also has a huge segment.
    #[test]
    fn small_curves_survive_a_huge_segment() {
        let mut p = BezPath::new();
        p.move_to((0.0, 0.0));
        p.curve_to((100.0, 0.0), (0.0, 100.0), (0.0, 0.0));
        p.line_to((1e10, 0.0));
        let c = coarsen_huge(&p, 0.1).unwrap();
        assert_eq!(c.elements().len(), p.elements().len(), "nothing here is huge enough to cut");
        let mut q = BezPath::new();
        q.move_to((0.0, 0.0));
        q.curve_to((1e13, 0.0), (-1e13, 1e13), (1e13, 1e13));
        let c = coarsen_huge(&q, 0.1).unwrap();
        assert!(c.elements().iter().skip(1).all(|e| matches!(e, PathEl::LineTo(_))) && c.elements().len() < 10_000);
    }
}

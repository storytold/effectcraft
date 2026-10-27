//! Curve-preserving boolean operations on filled [`BezPath`]s, built on `linesweeper`'s robust
//! sweep-line topology (MIT OR Apache-2.0).
//!
//! Adapted from VectorCraft's `vectorcraft-pathops` (`crates/pathops/src/boolean.rs`; our own code,
//! MIT OR Apache-2.0), reworked to take and return kurbo paths directly.
//!
//! Inputs are closed implicitly (a filled open path is painted as if closed). The sweep splits
//! curves at intersections and y-extrema; `tidy_segments` re-joins pieces that came from one
//! input curve (exactly) or one smooth run (by least-squares refit), so results carry roughly as
//! few vertices as the inputs. Output contours are simple and consistently oriented (holes wind
//! opposite to their outer contour), so they fill the same under either fill rule.

use kurbo::{BezPath, CubicBez, PathEl, PathSeg, Point, Shape as _};
use linesweeper::topology::{Contours, Topology, WindingNumber};

use crate::FillRule;
use crate::fit::{end_tangent, fit_single, is_straight, sample, start_tangent};

/// Refit tolerance (pixels) used to merge split curve pieces back together.
pub const DEFAULT_PRECISION: f64 = 0.01;

pub(crate) fn inside(rule: FillRule, w: i32) -> bool {
    match rule {
        FillRule::NonZero => w != 0,
        FillRule::EvenOdd => w % 2 != 0,
    }
}

/// Winding numbers for an arbitrary number of input paths (tag = input index).
#[derive(Clone, Debug, Default)]
pub(crate) struct Multi(Vec<i32>);

impl Multi {
    fn get(&self, i: usize) -> i32 {
        self.0.get(i).copied().unwrap_or(0)
    }
}

impl PartialEq for Multi {
    fn eq(&self, other: &Self) -> bool {
        let n = self.0.len().max(other.0.len());
        (0..n).all(|i| self.get(i) == other.get(i))
    }
}
impl Eq for Multi {}

impl std::ops::Add for Multi {
    type Output = Multi;
    fn add(mut self, rhs: Self) -> Multi {
        self += rhs;
        self
    }
}

impl std::ops::AddAssign for Multi {
    fn add_assign(&mut self, rhs: Self) {
        if self.0.len() < rhs.0.len() {
            self.0.resize(rhs.0.len(), 0);
        }
        for (a, b) in self.0.iter_mut().zip(rhs.0) {
            *a += b;
        }
    }
}

impl WindingNumber for Multi {
    type Tag = usize;
    fn single(tag: usize, positive: bool) -> Self {
        let mut v = vec![0; tag + 1];
        v[tag] = if positive { 1 } else { -1 };
        Multi(v)
    }
    fn of_tag(&self, tag: usize) -> Self {
        let mut v = vec![0; tag + 1];
        v[tag] = self.get(tag);
        Multi(v)
    }
}

/// Close every subpath with at least two points; drop single points and non-finite input.
pub fn close_subpaths(p: &BezPath) -> BezPath {
    let mut out = BezPath::new();
    for sp in p.subpaths() {
        let drawing = sp.iter().filter(|e| matches!(e, PathEl::LineTo(_) | PathEl::QuadTo(..) | PathEl::CurveTo(..))).count();
        if drawing == 0 {
            continue;
        }
        for el in sp {
            if !matches!(el, PathEl::ClosePath) {
                out.push(*el);
            }
        }
        out.close_path();
    }
    out
}

fn finite(p: &BezPath) -> bool {
    p.elements().iter().all(|el| match *el {
        PathEl::MoveTo(a) | PathEl::LineTo(a) => a.is_finite(),
        PathEl::QuadTo(a, b) => a.is_finite() && b.is_finite(),
        PathEl::CurveTo(a, b, c) => a.is_finite() && b.is_finite() && c.is_finite(),
        PathEl::ClosePath => true,
    })
}

/// Sweep tolerance for a set of paths (scaled with the coordinate magnitude).
fn eps_for<'a>(paths: impl IntoIterator<Item = &'a BezPath>) -> f64 {
    let mut m: f64 = 0.0;
    for p in paths {
        let b = p.bounding_box();
        m = m.max(b.x0.abs()).max(b.x1.abs()).max(b.y0.abs()).max(b.y1.abs());
    }
    (m * f64::EPSILON * 64.0).max(1e-6)
}

/// Output cleanup settings: refit `precision`, anchor positions that must survive (the inputs' own
/// vertices — only joints the sweep introduced are merged away) and the input curves, used to
/// rebuild split pieces exactly.
#[derive(Clone, Debug, Default)]
pub(crate) struct Tidy {
    pub(crate) precision: f64,
    keep: Vec<Point>,
    curves: Vec<(kurbo::Rect, CubicBez)>,
}

impl Tidy {
    /// Merge any smooth joint.
    pub(crate) fn free(precision: f64) -> Self {
        Self { precision, keep: Vec::new(), curves: Vec::new() }
    }
    /// Keep every on-curve point of `paths`.
    pub(crate) fn keeping<'a>(precision: f64, paths: impl IntoIterator<Item = &'a BezPath>) -> Self {
        let mut keep = Vec::new();
        let mut curves = Vec::new();
        for p in paths {
            for s in p.segments() {
                let seg = to_seg(s);
                keep.push(seg.c.p0);
                if !seg.line {
                    curves.push((seg.c.bounding_box().inflate(1e-6, 1e-6), seg.c));
                }
            }
        }
        keep.sort_by(|a, b| a.x.total_cmp(&b.x));
        Self { precision, keep, curves }
    }
    fn is_kept(&self, p: Point) -> bool {
        let tol = 1e-6 * (1.0 + p.x.abs().max(p.y.abs()));
        let lo = self.keep.partition_point(|q| q.x < p.x - tol);
        self.keep[lo..].iter().take_while(|q| q.x <= p.x + tol).any(|q| (q.y - p.y).abs() <= tol)
    }
}

/// An arrangement (planar subdivision) of N filled paths.
pub(crate) struct Arrangement {
    top: Topology<Multi>,
    rules: Vec<FillRule>,
    pub(crate) tidy: Tidy,
}

impl Arrangement {
    pub(crate) fn new(paths: &[(BezPath, FillRule)]) -> Option<Self> {
        let closed: Vec<BezPath> = paths.iter().map(|(p, _)| close_subpaths(p)).collect();
        if !closed.iter().all(finite) {
            return None;
        }
        let eps = eps_for(&closed);
        let top = Topology::<Multi>::from_paths(closed.iter().enumerate().map(|(i, p)| (p, i)), eps).ok()?;
        let tidy = Tidy::keeping(DEFAULT_PRECISION, &closed);
        Some(Self { top, rules: paths.iter().map(|p| p.1).collect(), tidy })
    }

    fn mask(&self, w: &Multi) -> Vec<bool> {
        self.rules.iter().enumerate().map(|(i, &r)| inside(r, w.get(i))).collect()
    }

    pub(crate) fn contours(&self, pred: impl Fn(&[bool]) -> bool) -> Contours {
        self.top.contours(|w| pred(&self.mask(w)))
    }
}

/// Contours of a single path under its fill rule (self-overlaps resolved).
pub(crate) fn normalize_contours(p: &BezPath, rule: FillRule) -> Option<Contours> {
    let bp = close_subpaths(p);
    if !finite(&bp) {
        return None;
    }
    let top = Topology::<i32>::from_path(&bp, eps_for([&bp])).ok()?;
    Some(top.contours(|w| inside(rule, *w)))
}

/// Contours whose mean width (2·area / perimeter) is below `precision` are numerical debris.
fn is_sliver(bp: &BezPath, precision: f64) -> bool {
    let a = bp.area().abs();
    let per = bp.perimeter(1e-6);
    per <= 0.0 || 2.0 * a / per < precision
}

/// Convert selected contours into one tidy compound path.
pub(crate) fn contours_to_path<'a>(contours: impl IntoIterator<Item = &'a BezPath>, tidy: &Tidy) -> BezPath {
    let mut out = BezPath::new();
    for c in contours {
        if is_sliver(c, tidy.precision) {
            continue;
        }
        let segs: Vec<Seg> = c.segments().map(to_seg).collect();
        let segs = tidy_segments(segs, tidy);
        if segs.len() < 2 && segs.iter().all(|s| s.line) {
            continue;
        }
        out.move_to(segs[0].c.p0);
        for s in &segs {
            if s.line {
                out.line_to(s.c.p3);
            } else {
                out.curve_to(s.c.p1, s.c.p2, s.c.p3);
            }
        }
        out.close_path();
    }
    out
}

pub(crate) fn all_contours(c: &Contours, tidy: &Tidy) -> BezPath {
    contours_to_path(c.contours().map(|k| &k.path), tidy)
}

/// One segment of a path: a cubic plus whether it is a straight line.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Seg {
    pub(crate) c: CubicBez,
    pub(crate) line: bool,
}

impl Seg {
    fn line(a: Point, b: Point) -> Self {
        Self { c: CubicBez::new(a, a, b, b), line: true }
    }
}

pub(crate) fn to_seg(s: PathSeg) -> Seg {
    match s {
        PathSeg::Line(l) => Seg::line(l.p0, l.p1),
        PathSeg::Quad(q) => {
            let c = q.raise();
            Seg { c, line: is_straight(&c, 1e-9) }
        }
        PathSeg::Cubic(c) => Seg { c, line: is_straight(&c, 1e-9 * (1.0 + c.p0.distance(c.p3))) },
    }
}

/// Merge segments of a closed loop that were split from one curve (or collinear lines) back
/// together, dropping degenerate pieces.
fn tidy_segments(segs: Vec<Seg>, tidy: &Tidy) -> Vec<Seg> {
    let scale = segs.iter().map(|s| s.c.p0.distance(s.c.p3)).fold(0.0, f64::max);
    let tiny = (scale * 1e-9).max(1e-9);
    let mut segs: Vec<Seg> = segs
        .into_iter()
        .filter(|s| {
            let r = kurbo::Rect::from_points(s.c.p0, s.c.p3).union_pt(s.c.p1).union_pt(s.c.p2);
            r.width() + r.height() > tiny
        })
        .collect();
    let n = segs.len();
    if n < 2 {
        return segs;
    }
    // Joint i sits at the start of segment i.
    let mergeable = |a: &Seg, b: &Seg| -> bool {
        if a.line != b.line || tidy.is_kept(b.c.p0) {
            return false;
        }
        let ta = end_tangent(&a.c);
        let tb = start_tangent(&b.c);
        ta.dot(tb) > 0.9998 && ta.cross(tb).abs() < 0.02
    };
    // Rotate so we start at a real corner (if there is one).
    if let Some(k) = (0..n).find(|&i| !mergeable(&segs[(i + n - 1) % n], &segs[i])) {
        segs.rotate_left(k);
    }
    let mut out: Vec<Seg> = Vec::with_capacity(n);
    let mut i = 0;
    while i < n {
        let mut cur = segs[i];
        let mut j = i + 1;
        while j < n && mergeable(&segs[j - 1], &segs[j]) {
            match try_merge(&segs[i..=j], tidy) {
                Some(m) => {
                    cur = m;
                    j += 1;
                }
                None => break,
            }
        }
        out.push(cur);
        i = j;
    }
    out
}

fn try_merge(run: &[Seg], tidy: &Tidy) -> Option<Seg> {
    let precision = tidy.precision;
    let first = run[0].c;
    let last = run[run.len() - 1].c;
    if run[0].line {
        let chord = last.p3 - first.p0;
        let len = chord.hypot();
        if len < 1e-12 {
            return None;
        }
        let ok = run.iter().all(|s| ((s.c.p3 - first.p0).cross(chord) / len).abs() <= precision * 0.5);
        return ok.then(|| Seg::line(first.p0, last.p3));
    }
    if let Some(c) = exact_merge(run, tidy) {
        return Some(Seg { c, line: false });
    }
    let mut pts = Vec::with_capacity(run.len() * 12 + 1);
    for (k, s) in run.iter().enumerate() {
        sample(&s.c, 12, &mut pts, k == 0);
    }
    let (c, err, _) = fit_single(&pts, start_tangent(&first), end_tangent(&last));
    if err > precision {
        return None;
    }
    Some(Seg { c, line: false })
}

/// If every piece of `run` lies on one input cubic, return the exact sub-curve spanning the run.
fn exact_merge(run: &[Seg], tidy: &Tidy) -> Option<CubicBez> {
    use kurbo::{ParamCurve, ParamCurveNearest};
    let a = run[0].c.p0;
    let b = run[run.len() - 1].c.p3;
    let scale = 1.0 + a.x.abs().max(a.y.abs()).max(b.x.abs()).max(b.y.abs());
    let tol = 1e-6 * scale;
    for (r, c) in &tidy.curves {
        if !r.contains(a) || !r.contains(b) {
            continue;
        }
        let na = c.nearest(a, 1e-12);
        let nb = c.nearest(b, 1e-12);
        if na.distance_sq.sqrt() > tol || nb.distance_sq.sqrt() > tol {
            continue;
        }
        let on_curve = run.iter().all(|s| c.nearest(s.c.eval(0.5), 1e-12).distance_sq.sqrt() <= tol * 10.0);
        if !on_curve || (na.t - nb.t).abs() < 1e-12 {
            continue;
        }
        let sub = if na.t < nb.t {
            c.subsegment(na.t..nb.t)
        } else {
            let r = c.subsegment(nb.t..na.t);
            CubicBez::new(r.p3, r.p2, r.p1, r.p0)
        };
        return Some(CubicBez::new(a, sub.p1, sub.p2, b));
    }
    None
}

/// N-ary boolean: the region where `pred(inside_flags)` holds, `inside_flags[i]` telling whether a
/// point is inside `paths[i]` under its fill rule. Returns one compound path.
pub fn boolean_n(paths: &[(BezPath, FillRule)], pred: impl Fn(&[bool]) -> bool) -> BezPath {
    match Arrangement::new(paths) {
        Some(arr) => all_contours(&arr.contours(pred), &arr.tidy),
        None => BezPath::new(),
    }
}

/// Resolve self-intersections / overlaps of one filled path into simple, consistently oriented
/// contours.
pub fn normalize(path: &BezPath, rule: FillRule) -> BezPath {
    match normalize_contours(path, rule) {
        Some(c) => all_contours(&c, &Tidy::keeping(DEFAULT_PRECISION, [path])),
        None => BezPath::new(),
    }
}

/// Unsigned filled area of a path under a fill rule (normalised first, so it is exact for
/// self-overlapping input).
pub fn filled_area(path: &BezPath, rule: FillRule) -> f64 {
    match normalize_contours(path, rule) {
        Some(c) => c.contours().map(|ct| ct.path.area()).sum::<f64>().abs(),
        None => 0.0,
    }
}

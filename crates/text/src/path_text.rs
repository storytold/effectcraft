//! Text on a path: arc-length parameterisation of a (mask) path and character placement along it
//! (Path Options: Reverse Path, Perpendicular To Path, Force Alignment, First / Last Margin).

use kurbo::{BezPath, PathEl, Point, Vec2};

/// A flattened path measured by arc length.
#[derive(Clone, Debug, Default)]
pub struct PathMeasure {
    pts: Vec<Point>,
    /// Cumulative length at each point.
    cum: Vec<f64>,
    pub closed: bool,
}

impl PathMeasure {
    /// Measure the first subpath of `path` (reversed when `reverse`).
    pub fn new(path: &BezPath, reverse: bool) -> PathMeasure {
        let mut pts: Vec<Point> = Vec::new();
        let mut closed = false;
        let mut started = false;
        let mut done = false;
        kurbo::flatten(path.iter(), 0.05, |el| {
            if done {
                return;
            }
            match el {
                PathEl::MoveTo(p) => {
                    if started {
                        done = true;
                    } else {
                        started = true;
                        pts.push(p);
                    }
                }
                PathEl::LineTo(p) if pts.last().is_none_or(|q| q.distance(p) > 1e-9) => {
                    pts.push(p);
                }
                PathEl::ClosePath => {
                    closed = true;
                    if let (Some(&a), Some(&b)) = (pts.first(), pts.last())
                        && a.distance(b) > 1e-9
                    {
                        pts.push(a);
                    }
                    done = true;
                }
                _ => {}
            }
        });
        if reverse {
            pts.reverse();
        }
        let mut cum = Vec::with_capacity(pts.len());
        let mut acc = 0.0;
        for (i, p) in pts.iter().enumerate() {
            if i > 0 {
                acc += pts[i - 1].distance(*p);
            }
            cum.push(acc);
        }
        PathMeasure { pts, cum, closed }
    }

    pub fn length(&self) -> f64 {
        self.cum.last().copied().unwrap_or(0.0)
    }

    pub fn is_empty(&self) -> bool {
        self.pts.len() < 2 || self.length() <= 1e-9
    }

    /// Point and unit tangent at arc length `s`. Closed paths wrap; open paths continue straight
    /// along the end tangents beyond either end.
    pub fn at(&self, s: f64) -> (Point, Vec2) {
        if self.is_empty() {
            return (self.pts.first().copied().unwrap_or(Point::ZERO), Vec2::new(1.0, 0.0));
        }
        let len = self.length();
        let s = if self.closed { s.rem_euclid(len) } else { s };
        let k = match self.cum.binary_search_by(|c| c.total_cmp(&s)) {
            Ok(i) => i.min(self.pts.len() - 2),
            Err(i) => i.clamp(1, self.pts.len() - 1) - 1,
        };
        let (a, b) = (self.pts[k], self.pts[k + 1]);
        let seg = self.cum[k + 1] - self.cum[k];
        let d = b - a;
        let tan = if seg > 0.0 { d / seg } else { Vec2::new(1.0, 0.0) };
        (a + tan * (s - self.cum[k]), tan)
    }
}

/// Where a character pivot lands on the path: the point and the tangent angle in degrees.
pub fn place(pm: &PathMeasure, s: f64, baseline_offset: f64, perpendicular: bool) -> (Point, f64) {
    let (p, t) = pm.at(s);
    // y down: the normal pointing "below" the baseline is the tangent turned clockwise.
    let n = Vec2::new(-t.y, t.x);
    let angle = if perpendicular { t.y.atan2(t.x).to_degrees() } else { 0.0 };
    let off = if perpendicular { n * baseline_offset } else { Vec2::new(0.0, baseline_offset) };
    (p + off, angle)
}

/// Arc-length positions for character pivots of one line, given each pivot's x along the
/// line, the line's [x0, x1] extent and alignment (0 = left, 1 = centre, 2 = right).
pub fn arc_positions(xs: &[f64], line: [f64; 2], align: u8, length: f64, first_margin: f64, last_margin: f64, force: bool) -> Vec<f64> {
    if xs.is_empty() {
        return vec![];
    }
    if force {
        let (a, b) = (first_margin, length - last_margin);
        let (x0, x1) = (xs[0], xs[xs.len() - 1]);
        if (x1 - x0).abs() < 1e-9 {
            return vec![a; xs.len()];
        }
        return xs.iter().map(|x| a + (b - a) * (x - x0) / (x1 - x0)).collect();
    }
    match align {
        1 => {
            let mid = (first_margin + length - last_margin) * 0.5;
            let c = (line[0] + line[1]) * 0.5;
            xs.iter().map(|x| mid + x - c).collect()
        }
        2 => xs.iter().map(|x| length - last_margin - (line[1] - x)).collect(),
        _ => xs.iter().map(|x| first_margin + x - line[0]).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measures_a_line_and_a_circle() {
        let mut p = BezPath::new();
        p.move_to((0.0, 0.0));
        p.line_to((100.0, 0.0));
        p.line_to((100.0, 50.0));
        let m = PathMeasure::new(&p, false);
        assert!((m.length() - 150.0).abs() < 1e-9);
        let (q, t) = m.at(120.0);
        assert!((q.x - 100.0).abs() < 1e-9 && (q.y - 20.0).abs() < 1e-9);
        assert!((t.y - 1.0).abs() < 1e-9);
        // Past the end: straight along the end tangent.
        let (q, _) = m.at(160.0);
        assert!((q.y - 60.0).abs() < 1e-9);
        let r = PathMeasure::new(&p, true);
        assert!((r.at(0.0).0.y - 50.0).abs() < 1e-9);

        let c = kurbo::Circle::new((0.0, 0.0), 100.0);
        let path = kurbo::Shape::to_path(&c, 0.01);
        let m = PathMeasure::new(&path, false);
        assert!(m.closed);
        assert!((m.length() - 2.0 * std::f64::consts::PI * 100.0).abs() < 0.5, "{}", m.length());
        // Every point is on the circle; wraps.
        for k in 0..10 {
            let (q, _) = m.at(k as f64 * 97.0);
            assert!((q.to_vec2().hypot() - 100.0).abs() < 0.05);
        }
    }

    #[test]
    fn placement_perpendicular_and_margins() {
        let mut p = BezPath::new();
        p.move_to((0.0, 100.0));
        p.line_to((0.0, 0.0)); // straight up
        let m = PathMeasure::new(&p, false);
        let (q, a) = place(&m, 30.0, 0.0, true);
        assert!((q.y - 70.0).abs() < 1e-9 && (a + 90.0).abs() < 1e-9);
        // Baseline offset below the path (y down) turns with the path: to the right of travel... up = -y → normal = (1, 0).
        let (q, _) = place(&m, 30.0, 10.0, true);
        assert!((q.x - 10.0).abs() < 1e-9, "{q:?}");
        let (q, a) = place(&m, 30.0, 10.0, false);
        assert!(a == 0.0 && (q.y - 80.0).abs() < 1e-9);
        let xs = [0.0, 10.0, 20.0];
        assert_eq!(arc_positions(&xs, [0.0, 20.0], 0, 100.0, 5.0, 0.0, false), vec![5.0, 15.0, 25.0]);
        assert_eq!(arc_positions(&xs, [0.0, 20.0], 2, 100.0, 0.0, 10.0, false), vec![70.0, 80.0, 90.0]);
        assert_eq!(arc_positions(&xs, [0.0, 20.0], 1, 100.0, 0.0, 0.0, false), vec![40.0, 50.0, 60.0]);
        assert_eq!(arc_positions(&xs, [0.0, 20.0], 0, 100.0, 10.0, 10.0, true), vec![10.0, 50.0, 90.0]);
    }
}

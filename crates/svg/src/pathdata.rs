//! SVG path data (SVG 1.1 §8.3 / SVG 2 §9.3): every command, implicit repeats, relative
//! coordinates, smooth curves and elliptical arcs (converted to cubics through kurbo's `SvgArc`).
//! Parsing stops at the first error, keeping what was read (as the specification asks).

use kurbo::{BezPath, PathEl, Point, SvgArc, Vec2};

/// Maximum path elements retained from one SVG path or point list.
pub(crate) const MAX_PATH_ELEMENTS: usize = 100_000;

/// Tokenizer over numbers and command letters.
struct Lex<'a> {
    b: &'a [u8],
    i: usize,
}

impl Lex<'_> {
    fn skip_sep(&mut self) {
        while self.i < self.b.len() && (self.b[self.i].is_ascii_whitespace() || self.b[self.i] == b',') {
            self.i += 1;
        }
    }
    fn command(&mut self) -> Option<u8> {
        self.skip_sep();
        let c = *self.b.get(self.i)?;
        if c.is_ascii_alphabetic() && c != b'e' && c != b'E' {
            self.i += 1;
            Some(c)
        } else {
            None
        }
    }
    fn at_number(&mut self) -> bool {
        self.skip_sep();
        self.b.get(self.i).is_some_and(|c| c.is_ascii_digit() || matches!(c, b'-' | b'+' | b'.'))
    }
    fn number(&mut self) -> Option<f64> {
        self.skip_sep();
        let start = self.i;
        let b = self.b;
        let mut i = self.i;
        if i < b.len() && matches!(b[i], b'-' | b'+') {
            i += 1;
        }
        let mut digits = false;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
            digits = true;
        }
        if i < b.len() && b[i] == b'.' {
            i += 1;
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
                digits = true;
            }
        }
        if !digits {
            return None;
        }
        if i < b.len() && matches!(b[i], b'e' | b'E') {
            let mut j = i + 1;
            if j < b.len() && matches!(b[j], b'-' | b'+') {
                j += 1;
            }
            if j < b.len() && b[j].is_ascii_digit() {
                while j < b.len() && b[j].is_ascii_digit() {
                    j += 1;
                }
                i = j;
            }
        }
        self.i = i;
        let number = std::str::from_utf8(&b[start..i]).ok()?.parse::<f64>().ok()?;
        number.is_finite().then_some(number)
    }
    /// Arc flags may be written without separators ("a1 1 0 00 1 1").
    fn flag(&mut self) -> Option<bool> {
        self.skip_sep();
        let c = *self.b.get(self.i)?;
        self.i += 1;
        match c {
            b'0' => Some(false),
            b'1' => Some(true),
            _ => None,
        }
    }
}

/// Parse no more than `max_values`; the flag reports whether another valid value was present.
pub(crate) fn numbers_bounded(s: &str, max_values: usize) -> (Vec<f64>, bool) {
    let mut l = Lex { b: s.as_bytes(), i: 0 };
    let mut v = vec![];
    while l.at_number() {
        if v.len() >= max_values {
            return (v, true);
        }
        match l.number() {
            Some(x) => v.push(x),
            None => break,
        }
    }
    (v, false)
}

/// Parse path data into a kurbo path, retaining at most [`MAX_PATH_ELEMENTS`] elements.
/// Malformed or overlong input returns the prefix parsed before the problem.
pub fn parse_path_data(d: &str) -> BezPath {
    parse_path_data_bounded(d, MAX_PATH_ELEMENTS).0
}

/// Pen position and reflection state while parsing path commands.
struct PathState {
    /// Current point.
    current: Point,
    /// Start of the current subpath.
    start: Point,
    /// Last cubic control point, for smooth curves.
    last_cubic: Option<Point>,
    /// Last quadratic control point, for smooth curves.
    last_quad: Option<Point>,
    /// Whether a subpath is open.
    open: bool,
}

/// Cap on path elements and whether it has been hit.
struct PathLimit {
    /// Maximum number of path elements.
    max_elements: usize,
    /// Set when the cap or a non-finite coordinate stopped output.
    exceeded: bool,
}

/// Parse at most `max_elements`, reporting when more path geometry was present.
pub(crate) fn parse_path_data_bounded(d: &str, max_elements: usize) -> (BezPath, bool) {
    let mut l = Lex { b: d.as_bytes(), i: 0 };
    let mut p = BezPath::new();
    let mut state = PathState { current: Point::ZERO, start: Point::ZERO, last_cubic: None, last_quad: None, open: false };
    let mut limit = PathLimit { max_elements, exceeded: false };
    let mut cmd: Option<u8> = None;
    loop {
        if p.elements().len() >= limit.max_elements {
            l.skip_sep();
            limit.exceeded = l.i < l.b.len();
            break;
        }
        let c = match l.command() {
            Some(c) => c,
            None => match cmd {
                // Implicit repeat (a moveto repeats as lineto). Z takes no numbers, so a number
                // after it is an error: repeating Z would loop forever without consuming input.
                Some(c) if l.at_number() && !c.eq_ignore_ascii_case(&b'z') => match c {
                    b'M' => b'L',
                    b'm' => b'l',
                    other => other,
                },
                _ => break,
            },
        };
        let rel = c.is_ascii_lowercase();
        let base = if rel { state.current.to_vec2() } else { Vec2::ZERO };
        let pt = |l: &mut Lex| -> Option<Point> { Some(Point::new(l.number()?, l.number()?) + base) };
        let ok = (|| -> Option<()> {
            match c.to_ascii_uppercase() {
                b'M' => {
                    state.current = pt(&mut l)?;
                    if !push_element(&mut p, PathEl::MoveTo(state.current), &mut limit) {
                        return None;
                    }
                    state.start = state.current;
                    state.open = true;
                    state.last_cubic = None;
                    state.last_quad = None;
                }
                b'Z' => {
                    if state.open && !push_element(&mut p, PathEl::ClosePath, &mut limit) {
                        return None;
                    }
                    state.current = state.start;
                    state.last_cubic = None;
                    state.last_quad = None;
                }
                _ if !state.open => {
                    // A segment without a preceding moveto starts at the current point.
                    if !push_element(&mut p, PathEl::MoveTo(state.current), &mut limit) {
                        return None;
                    }
                    state.open = true;
                    return segment(&mut l, c, base, &mut p, &mut state, &mut limit);
                }
                _ => return segment(&mut l, c, base, &mut p, &mut state, &mut limit),
            }
            Some(())
        })();
        if ok.is_none() {
            break;
        }
        if limit.exceeded {
            break;
        }
        cmd = Some(c);
        if c.eq_ignore_ascii_case(&b'z') {
            // After Z a new segment starts from the subpath start (an unused move is dropped
            // below).
            if !push_element(&mut p, PathEl::MoveTo(state.start), &mut limit) {
                break;
            }
            state.open = true;
        }
    }
    (strip_trailing_moves(p), limit.exceeded)
}

/// Push an element if finite and under the cap; otherwise mark the limit exceeded and return false.
fn push_element(p: &mut BezPath, element: PathEl, limit: &mut PathLimit) -> bool {
    let finite = match element {
        PathEl::MoveTo(point) | PathEl::LineTo(point) => point.x.is_finite() && point.y.is_finite(),
        PathEl::QuadTo(control, point) => [control.x, control.y, point.x, point.y].iter().all(|value| value.is_finite()),
        PathEl::CurveTo(first, second, point) => [first.x, first.y, second.x, second.y, point.x, point.y].iter().all(|value| value.is_finite()),
        PathEl::ClosePath => true,
    };
    if p.elements().len() >= limit.max_elements || !finite {
        limit.exceeded = true;
        return false;
    }
    p.push(element);
    true
}

/// Parse one path command's arguments into `p`, returning `None` on malformed or over-limit input.
fn segment(l: &mut Lex, c: u8, base: Vec2, p: &mut BezPath, state: &mut PathState, limit: &mut PathLimit) -> Option<()> {
    let num = |l: &mut Lex| l.number();
    let pt = |l: &mut Lex| -> Option<Point> { Some(Point::new(l.number()?, l.number()?) + base) };
    match c.to_ascii_uppercase() {
        b'L' => {
            let current = pt(l)?;
            if !push_element(p, PathEl::LineTo(current), limit) {
                return None;
            }
            state.current = current;
            state.last_cubic = None;
            state.last_quad = None;
        }
        b'H' => {
            let x = num(l)? + base.x;
            let current = Point::new(x, state.current.y);
            if !push_element(p, PathEl::LineTo(current), limit) {
                return None;
            }
            state.current = current;
            state.last_cubic = None;
            state.last_quad = None;
        }
        b'V' => {
            let y = num(l)? + base.y;
            let current = Point::new(state.current.x, y);
            if !push_element(p, PathEl::LineTo(current), limit) {
                return None;
            }
            state.current = current;
            state.last_cubic = None;
            state.last_quad = None;
        }
        b'C' => {
            let (c1, c2, e) = (pt(l)?, pt(l)?, pt(l)?);
            if !push_element(p, PathEl::CurveTo(c1, c2, e), limit) {
                return None;
            }
            state.last_cubic = Some(c2);
            state.last_quad = None;
            state.current = e;
        }
        b'S' => {
            let c1 = state.last_cubic.map_or(state.current, |c| state.current + (state.current - c));
            let (c2, e) = (pt(l)?, pt(l)?);
            if !push_element(p, PathEl::CurveTo(c1, c2, e), limit) {
                return None;
            }
            state.last_cubic = Some(c2);
            state.last_quad = None;
            state.current = e;
        }
        b'Q' => {
            let (q, e) = (pt(l)?, pt(l)?);
            if !push_element(p, PathEl::QuadTo(q, e), limit) {
                return None;
            }
            state.last_quad = Some(q);
            state.last_cubic = None;
            state.current = e;
        }
        b'T' => {
            let q = state.last_quad.map_or(state.current, |c| state.current + (state.current - c));
            let e = pt(l)?;
            if !push_element(p, PathEl::QuadTo(q, e), limit) {
                return None;
            }
            state.last_quad = Some(q);
            state.last_cubic = None;
            state.current = e;
        }
        b'A' => {
            let rx = num(l)?.abs();
            let ry = num(l)?.abs();
            let rot = num(l)?;
            let large = l.flag()?;
            let sweep = l.flag()?;
            let e = pt(l)?;
            let arc = SvgArc { from: state.current, to: e, radii: Vec2::new(rx, ry), x_rotation: rot.to_radians(), large_arc: large, sweep };
            let mut last_end = None;
            if let Some(a) = kurbo::Arc::from_svg_arc(&arc) {
                // Scale tolerance with the radius so the segment count stays bounded.
                let tol = 0.1_f64.max(a.radii.x.max(a.radii.y) * 1e-4);
                a.to_cubic_beziers(tol, |c1, c2, end| {
                    if limit.exceeded {
                        return;
                    }
                    if push_element(p, PathEl::CurveTo(c1, c2, end), limit) {
                        last_end = Some(end);
                    }
                });
            }
            if limit.exceeded {
                return None;
            }
            // Degenerate arcs (e.g. huge radii) may emit nothing or stop short of the end point.
            if last_end.is_none_or(|q| (q - e).hypot() > 1e-3 * (1.0 + e.to_vec2().hypot())) && !push_element(p, PathEl::LineTo(e), limit) {
                return None;
            }
            state.current = e;
            state.last_cubic = None;
            state.last_quad = None;
        }
        _ => return None,
    }
    Some(())
}

/// Drop move-tos that start no segment (from closepath bookkeeping).
fn strip_trailing_moves(p: BezPath) -> BezPath {
    use kurbo::PathEl;
    let els = p.elements();
    let mut out = BezPath::new();
    for (i, e) in els.iter().enumerate() {
        if let PathEl::MoveTo(_) = e {
            let next_is_seg = els.get(i + 1).is_some_and(|n| !matches!(n, PathEl::MoveTo(_)));
            if !next_is_seg {
                continue;
            }
        }
        out.push(*e);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use kurbo::PathEl;

    #[test]
    /// Huge arc radii stay bounded.
    fn huge_arc_radius_stays_bounded() {
        let p = parse_path_data("M0 0A1e36 1e36 0 1 1 1e30 0");
        assert!(p.elements().len() < 1000, "got {} elements", p.elements().len());
    }

    #[test]
    fn commands() {
        let p = parse_path_data("M10,10 L20,10 h10 v10 H10 z m5 5 l1-1.5.5.5e1 C 0 0 1 1 2 2 s3 3 4 4 Q1 1 2 2 t3 3 A5 5 0 0 1 20 20 a5 5 0 1010 0");
        let els = p.elements();
        assert_eq!(els[0], PathEl::MoveTo(Point::new(10.0, 10.0)));
        assert_eq!(els[2], PathEl::LineTo(Point::new(30.0, 10.0)));
        assert_eq!(els[3], PathEl::LineTo(Point::new(30.0, 20.0)));
        assert!(els.iter().any(|e| matches!(e, PathEl::ClosePath)));
        // Relative move after Z is relative to the subpath start (10,10).
        assert!(els.contains(&PathEl::MoveTo(Point::new(15.0, 15.0))));
        // "l1-1.5.5.5e1": two implicit linetos (1,-1.5) then (.5,5).
        assert!(els.contains(&PathEl::LineTo(Point::new(16.0, 13.5))));
        assert!(els.contains(&PathEl::LineTo(Point::new(16.5, 18.5))));
        assert!(els.iter().filter(|e| matches!(e, PathEl::CurveTo(..))).count() >= 4);
    }

    #[test]
    fn errors_keep_prefix() {
        let p = parse_path_data("M0 0 L10 0 L10 x10 L0 10");
        assert_eq!(p.elements().len(), 2);
        assert!(parse_path_data("garbage").elements().is_empty());
        assert!(parse_path_data("").elements().is_empty());
    }

    #[test]
    fn number_after_close_stops_instead_of_looping() {
        // "Z9": Z was repeated implicitly without consuming the number, appending close-paths
        // until memory ran out.
        let p = parse_path_data("M0 0 L10 0 L10 10 Z9 C1 2 3 4 5 6");
        assert_eq!(p.elements().len(), 4);
        assert!(parse_path_data("M0 0 L10 0 z 9 9").elements().len() <= 4);
    }

    #[test]
    /// Huge-radius arcs still reach their end point.
    fn huge_radius_arc_still_reaches_end_point() {
        for d in ["M0 0 A1e20 1e20 0 0 1 10 0 L10 10", "M0 0 A1e20 1e20 0 1 1 10 0 L10 10", "M0 0 a1e20 1e20 0 0 1 10 0 L10 10"] {
            let p = parse_path_data(d);
            let reaches = p.elements().iter().any(|e| match e {
                PathEl::LineTo(q) | PathEl::CurveTo(_, _, q) => (q.x - 10.0).abs() < 1e-6 && q.y.abs() < 1e-6,
                _ => false,
            });
            assert!(reaches, "{d}: {:?}", p.elements());
        }
    }

    #[test]
    fn number_lists() {
        assert_eq!(numbers_bounded("0 0,100 50", 8).0, vec![0.0, 0.0, 100.0, 50.0]);
        assert_eq!(numbers_bounded("-1-2.5.5", 8).0, vec![-1.0, -2.5, 0.5]);
        assert_eq!(numbers_bounded("1e2,2E-1", 8).0, vec![100.0, 0.2]);
    }

    #[test]
    /// The element cap stops expansion and reports truncation.
    fn path_element_limit_stops_expansion_and_reports_truncation() {
        let data = format!("M0 0{}", "h1".repeat(MAX_PATH_ELEMENTS + 1));
        let (path, exceeded) = parse_path_data_bounded(&data, 32);
        assert!(exceeded);
        assert!(path.elements().len() <= 32);
    }

    #[test]
    /// The number list cap stops retaining values.
    fn number_list_limit_stops_retaining_values() {
        let (values, exceeded) = numbers_bounded("1 2 3 4", 3);
        assert_eq!(values, [1.0, 2.0, 3.0]);
        assert!(exceeded);
    }
}

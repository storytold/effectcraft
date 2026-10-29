//! SVG path data (SVG 1.1 §8.3 / SVG 2 §9.3): every command, implicit repeats, relative
//! coordinates, smooth curves and elliptical arcs (converted to cubics through kurbo's `SvgArc`).
//! Parsing stops at the first error, keeping what was read (as the specification asks).

use kurbo::{BezPath, Point, SvgArc, Vec2};

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
        std::str::from_utf8(&b[start..i]).ok()?.parse().ok()
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

/// All numbers in a list (`points`, `viewBox`, transform arguments).
pub fn numbers(s: &str) -> Vec<f64> {
    let mut l = Lex { b: s.as_bytes(), i: 0 };
    let mut v = vec![];
    while l.at_number() {
        match l.number() {
            Some(x) => v.push(x),
            None => break,
        }
    }
    v
}

/// Parse path data into a kurbo path.
pub fn parse_path_data(d: &str) -> BezPath {
    let mut l = Lex { b: d.as_bytes(), i: 0 };
    let mut p = BezPath::new();
    let mut cur = Point::ZERO;
    let mut start = Point::ZERO;
    // Last control point of the previous C/S (cubic) or Q/T (quad) command.
    let mut last_cubic: Option<Point> = None;
    let mut last_quad: Option<Point> = None;
    let mut cmd: Option<u8> = None;
    let mut open = false;
    loop {
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
        let base = if rel { cur.to_vec2() } else { Vec2::ZERO };
        let pt = |l: &mut Lex| -> Option<Point> { Some(Point::new(l.number()?, l.number()?) + base) };
        let ok = (|| -> Option<()> {
            match c.to_ascii_uppercase() {
                b'M' => {
                    cur = pt(&mut l)?;
                    p.move_to(cur);
                    start = cur;
                    open = true;
                    last_cubic = None;
                    last_quad = None;
                }
                b'Z' => {
                    if open {
                        p.close_path();
                    }
                    cur = start;
                    last_cubic = None;
                    last_quad = None;
                }
                _ if !open => {
                    // A segment without a preceding moveto starts at the current point.
                    p.move_to(cur);
                    open = true;
                    return segment(&mut l, c, base, &mut p, &mut cur, &mut last_cubic, &mut last_quad);
                }
                _ => return segment(&mut l, c, base, &mut p, &mut cur, &mut last_cubic, &mut last_quad),
            }
            Some(())
        })();
        if ok.is_none() {
            break;
        }
        cmd = Some(c);
        if c.eq_ignore_ascii_case(&b'z') {
            // After Z a new segment starts from the subpath start (an unused move is dropped
            // below).
            p.move_to(start);
            open = true;
        }
    }
    strip_trailing_moves(p)
}

fn segment(l: &mut Lex, c: u8, base: Vec2, p: &mut BezPath, cur: &mut Point, last_cubic: &mut Option<Point>, last_quad: &mut Option<Point>) -> Option<()> {
    let num = |l: &mut Lex| l.number();
    let pt = |l: &mut Lex| -> Option<Point> { Some(Point::new(l.number()?, l.number()?) + base) };
    match c.to_ascii_uppercase() {
        b'L' => {
            *cur = pt(l)?;
            p.line_to(*cur);
            *last_cubic = None;
            *last_quad = None;
        }
        b'H' => {
            let x = num(l)? + base.x;
            *cur = Point::new(x, cur.y);
            p.line_to(*cur);
            *last_cubic = None;
            *last_quad = None;
        }
        b'V' => {
            let y = num(l)? + base.y;
            *cur = Point::new(cur.x, y);
            p.line_to(*cur);
            *last_cubic = None;
            *last_quad = None;
        }
        b'C' => {
            let (c1, c2, e) = (pt(l)?, pt(l)?, pt(l)?);
            p.curve_to(c1, c2, e);
            *last_cubic = Some(c2);
            *last_quad = None;
            *cur = e;
        }
        b'S' => {
            let c1 = last_cubic.map_or(*cur, |c| *cur + (*cur - c));
            let (c2, e) = (pt(l)?, pt(l)?);
            p.curve_to(c1, c2, e);
            *last_cubic = Some(c2);
            *last_quad = None;
            *cur = e;
        }
        b'Q' => {
            let (q, e) = (pt(l)?, pt(l)?);
            p.quad_to(q, e);
            *last_quad = Some(q);
            *last_cubic = None;
            *cur = e;
        }
        b'T' => {
            let q = last_quad.map_or(*cur, |c| *cur + (*cur - c));
            let e = pt(l)?;
            p.quad_to(q, e);
            *last_quad = Some(q);
            *last_cubic = None;
            *cur = e;
        }
        b'A' => {
            let rx = num(l)?.abs();
            let ry = num(l)?.abs();
            let rot = num(l)?;
            let large = l.flag()?;
            let sweep = l.flag()?;
            let e = pt(l)?;
            let arc = SvgArc { from: *cur, to: e, radii: Vec2::new(rx, ry), x_rotation: rot.to_radians(), large_arc: large, sweep };
            let mut last_end = None;
            if let Some(a) = kurbo::Arc::from_svg_arc(&arc) {
                // Scale tolerance with the radius so the segment count stays bounded.
                let tol = 0.1_f64.max(a.radii.x.max(a.radii.y) * 1e-4);
                a.to_cubic_beziers(tol, |c1, c2, end| {
                    p.curve_to(c1, c2, end);
                    last_end = Some(end);
                });
            }
            // Degenerate arcs (e.g. huge radii) may emit nothing or stop short of the end point.
            if last_end.is_none_or(|q| (q - e).hypot() > 1e-3 * (1.0 + e.to_vec2().hypot())) {
                p.line_to(e);
            }
            *cur = e;
            *last_cubic = None;
            *last_quad = None;
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
        assert_eq!(numbers("0 0,100 50"), vec![0.0, 0.0, 100.0, 50.0]);
        assert_eq!(numbers("-1-2.5.5"), vec![-1.0, -2.5, 0.5]);
        assert_eq!(numbers("1e2,2E-1"), vec![100.0, 0.2]);
    }
}

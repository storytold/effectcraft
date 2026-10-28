use super::value::{Flow, V};
use super::{Affine, BezPath, FillRule, Interp, MAX_PATH, Paint, PathEl, Point, Stroke};

/// The angle from `a` counterclockwise to `b` (degrees, 0 to 360), worked out from what is left
/// of each after whole turns.
fn ccw_turn(a: f64, b: f64) -> f64 {
    (b % 360.0 - a % 360.0).rem_euclid(360.0)
}

/// The most turns one `arc` or `arcn` draws.
const MAX_ARC_TURNS: f64 = 1024.0;

impl Interp {
    fn device_scale(&self) -> f64 {
        self.gs.ctm.determinant().abs().sqrt()
    }

    pub(super) fn user_point(&mut self) -> Point {
        let y = self.popn();
        let x = self.popn();
        Point::new(x, y)
    }

    /// Stop the program when a path of `len` elements would be longer than [`MAX_PATH`].
    fn path_fits(&mut self, len: usize) -> Result<(), Flow> {
        if len <= MAX_PATH {
            return Ok(());
        }
        self.b.skip("PostScript (path limit)");
        Err(Flow::Quit)
    }

    /// [`Self::path_fits`] for the current path with `n` more elements.
    pub(super) fn room(&mut self, n: usize) -> Result<(), Flow> {
        self.path_fits(self.gs.path.elements().len().saturating_add(n))
    }

    pub(super) fn move_to(&mut self, p: Point) -> Result<(), Flow> {
        // A moveto right after a moveto replaces it (PostScript Language Reference §8.2, `moveto`).
        if matches!(self.gs.path.elements().last(), Some(PathEl::MoveTo(_))) {
            self.gs.path.pop();
        }
        self.room(1)?;
        let q = self.gs.ctm * p;
        self.gs.path.move_to(q);
        self.gs.cur = Some(q);
        self.gs.start = Some(q);
        Ok(())
    }

    pub(super) fn line_to(&mut self, p: Point) -> Result<(), Flow> {
        self.room(1 + usize::from(self.gs.cur.is_none()))?;
        let q = self.gs.ctm * p;
        if self.gs.cur.is_none() {
            self.gs.path.move_to(q);
            self.gs.start = Some(q);
        }
        self.gs.path.line_to(q);
        self.gs.cur = Some(q);
        Ok(())
    }

    pub(super) fn curve_to(&mut self, a: Point, b: Point, c: Point) -> Result<(), Flow> {
        self.room(1 + usize::from(self.gs.cur.is_none()))?;
        let m = self.gs.ctm;
        if self.gs.cur.is_none() {
            self.gs.path.move_to(m * a);
        }
        self.gs.path.curve_to(m * a, m * b, m * c);
        self.gs.cur = Some(m * c);
        Ok(())
    }

    pub(super) fn current_user(&self) -> Point {
        self.gs.ctm.inverse() * self.gs.cur.unwrap_or_default()
    }

    pub(super) fn arc(&mut self, neg: bool) -> Result<(), Flow> {
        let a2 = self.popn();
        let a1 = self.popn();
        let r = self.popn();
        let c = self.user_point();
        // `arc` raises `a2` by multiples of 360 until it is at least `a1`, and `arcn` lowers it
        // until it is at most `a1` (PostScript Language Reference §8.2, `arc` and `arcn`). Those
        // sweeps, and the first endpoint, are worked out from what is left of each angle after
        // whole turns.
        let sweep = if !neg && a2 < a1 {
            ccw_turn(a1, a2)
        } else if neg && a2 > a1 {
            -ccw_turn(a2, a1)
        } else {
            a2 - a1
        };
        // A sweep of more than `MAX_ARC_TURNS` turns is shortened by whole pairs of turns to
        // between `MAX_ARC_TURNS - 2` and `MAX_ARC_TURNS` turns, with a skip note. The arc still
        // ends at `a2`, and an even-odd fill or clip of it covers the same area as the full sweep.
        let max = MAX_ARC_TURNS * 360.0;
        let sweep = if sweep.abs() > max {
            self.b.skip("arc (too many turns)");
            let s = sweep.signum();
            s * (max - 720.0 + (s * (a2 % 720.0 - a1 % 720.0)).rem_euclid(720.0))
        } else {
            sweep
        };
        let (a1, sweep) = ((a1 % 360.0).to_radians(), sweep.to_radians());
        let start = Point::new(c.x + r * a1.cos(), c.y + r * a1.sin());
        if self.gs.cur.is_some() {
            self.line_to(start)?;
        } else {
            self.move_to(start)?;
        }
        let arc = kurbo::Arc::new(c, (r, r), a1, sweep, 0.0);
        let m = self.gs.ctm;
        // kurbo divides an arc into more curves the larger the radius (about 3.7 × 10^49 for a
        // quarter turn at a radius of 10^300, more than a usize can count), so each one counts
        // against the budget and the path limit.
        let mut curves = arc.append_iter(0.1);
        while let Some(PathEl::CurveTo(p1, p2, p3)) = curves.next() {
            self.room(1)?;
            self.spend(1)?;
            self.gs.path.curve_to(m * p1, m * p2, m * p3);
            self.gs.cur = Some(m * p3);
        }
        Ok(())
    }

    pub(super) fn paint(&mut self, fill: Option<FillRule>, stroke: bool) {
        let path = std::mem::take(&mut self.gs.path);
        self.gs.cur = None;
        self.gs.start = None;
        let s = self.device_scale();
        let st = stroke.then(|| Stroke {
            paint: Paint::Color(self.gs.color),
            opacity: 1.0,
            width: if self.gs.lw <= 0.0 { 1.0 } else { self.gs.lw * s },
            cap: self.gs.cap,
            join: self.gs.join,
            miter: self.gs.miter.max(1.0),
            dash: self.gs.dash.as_ref().map(|(a, o)| (a.iter().map(|v| v * s).collect(), o * s)),
        });
        self.b.fill_stroke(path, Affine::IDENTITY, fill.map(|r| (Paint::Color(self.gs.color), 1.0, r)), st);
    }

    pub(super) fn rect_path(&mut self) -> Result<Vec<BezPath>, Flow> {
        // x y w h, or an array / string of numbers.
        let top = self.pop();
        let nums: Vec<f64> = match &top {
            V::Array(_) | V::Proc(_) => {
                let items = top.items().unwrap_or_default();
                // An array's rectangles count against the budget, five path elements each.
                self.spend(items.len() / 4 * 5)?;
                items.iter().filter_map(V::num).collect()
            }
            V::Num(h) => {
                let w = self.popn();
                let y = self.popn();
                let x = self.popn();
                vec![x, y, w, *h]
            }
            _ => vec![],
        };
        self.path_fits(nums.len() / 4 * 5)?;
        let m = self.gs.ctm;
        Ok(nums
            .as_chunks::<4>()
            .0
            .iter()
            .map(|r| {
                let mut p = BezPath::new();
                p.move_to(m * Point::new(r[0], r[1]));
                p.line_to(m * Point::new(r[0] + r[2], r[1]));
                p.line_to(m * Point::new(r[0] + r[2], r[1] + r[3]));
                p.line_to(m * Point::new(r[0], r[1] + r[3]));
                p.close_path();
                p
            })
            .collect())
    }

    pub(super) fn grestore(&mut self) {
        if let Some(g) = self.saved.pop() {
            let d = g.depth;
            self.gs = g;
            self.b.close_to(d);
        }
    }
}

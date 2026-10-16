//! Encapsulated PostScript: a small PostScript interpreter (PostScript Language Reference, 3rd
//! edition, and the EPSF 3.0 specification) covering what exported vector EPS files use: the
//! operand and dictionary stacks, procedures and `def`, arithmetic, control flow, arrays and
//! dictionaries, the graphics state, path construction (including arcs) and painting, clipping
//! and device colours, and text (`findfont` / `selectfont`, `show` and its variants, `charpath`,
//! `stringwidth`, re-encoded fonts) with embedded Type 1 and CFF programs or the bundled
//! stand-in fonts. Images are skipped (their inline data is stepped over), as is
//! Level 3 smooth shading (`shfill`).

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use effectcraft_svg::{Affine, BezPath, Cap, FillRule, Join, Paint, Stroke};
use kurbo::{PathEl, Point};

use crate::build::Builder;
use crate::font::Font;
use crate::object::{ascii85, is_white, parse_num};

type DictRef = Rc<RefCell<HashMap<String, V>>>;

#[derive(Clone)]
enum V {
    Num(f64),
    Bool(bool),
    /// An executable name.
    Name(Rc<str>),
    /// A literal name (`/name`).
    Lit(Rc<str>),
    Str(Rc<RefCell<Vec<u8>>>),
    Proc(Rc<Vec<V>>),
    Array(Rc<RefCell<Vec<V>>>),
    Dict(DictRef),
    Mark,
    Null,
    /// `currentfile`.
    File,
}

impl V {
    fn num(&self) -> Option<f64> {
        match self {
            V::Num(n) => Some(*n),
            _ => None,
        }
    }
    fn key(&self) -> String {
        match self {
            V::Name(n) | V::Lit(n) => n.to_string(),
            V::Str(s) => String::from_utf8_lossy(&s.borrow()).into_owned(),
            V::Num(n) => format!("{n}"),
            V::Bool(b) => format!("{b}"),
            _ => String::new(),
        }
    }
    fn items(&self) -> Option<Vec<V>> {
        match self {
            V::Array(a) => Some(a.borrow().clone()),
            V::Proc(p) => Some(p.as_ref().clone()),
            _ => None,
        }
    }
}

enum Flow {
    Exit,
    Stop,
    Quit,
}

#[derive(Clone)]
struct GState {
    ctm: Affine,
    color: [f64; 3],
    lw: f64,
    cap: Cap,
    join: Join,
    miter: f64,
    dash: Option<(Vec<f64>, f64)>,
    path: BezPath,
    cur: Option<Point>,
    start: Option<Point>,
    depth: usize,
    /// The current font dictionary.
    font: Option<DictRef>,
}

/// Source tokenizer (the data source for `currentfile`).
struct Src<'a> {
    d: &'a [u8],
    pos: usize,
}

impl Src<'_> {
    fn skip_ws(&mut self) {
        while self.pos < self.d.len() {
            let c = self.d[self.pos];
            if is_white(c) {
                self.pos += 1;
            } else if c == b'%' {
                let s = self.pos;
                while self.pos < self.d.len() && !matches!(self.d[self.pos], b'\n' | b'\r') {
                    self.pos += 1;
                }
                // DSC: skip embedded binary / data sections.
                let line = &self.d[s..self.pos];
                if line.starts_with(b"%%BeginData:") || line.starts_with(b"%%BeginBinary:") || line.starts_with(b"%%BeginPreview") {
                    let end: &[u8] = if line.starts_with(b"%%BeginData:") {
                        b"%%EndData"
                    } else if line.starts_with(b"%%BeginBinary:") {
                        b"%%EndBinary"
                    } else {
                        b"%%EndPreview"
                    };
                    match self.d[self.pos..].windows(end.len()).position(|w| w == end) {
                        Some(p) => self.pos += p,
                        None => self.pos = self.d.len(),
                    }
                }
            } else {
                break;
            }
        }
    }

    fn word(&mut self) -> &[u8] {
        let s = self.pos;
        while self.pos < self.d.len() {
            let c = self.d[self.pos];
            if is_white(c) || matches!(c, b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%') {
                break;
            }
            self.pos += 1;
        }
        &self.d[s..self.pos]
    }

    /// The next token; `{ … }` comes back as a whole procedure.
    fn token(&mut self, depth: usize) -> Option<V> {
        self.skip_ws();
        let c = *self.d.get(self.pos)?;
        Some(match c {
            b'{' => {
                self.pos += 1;
                let mut body = vec![];
                loop {
                    self.skip_ws();
                    match self.d.get(self.pos) {
                        None => break,
                        Some(b'}') => {
                            self.pos += 1;
                            break;
                        }
                        _ => {
                            if depth > 100 {
                                return None;
                            }
                            match self.token(depth + 1) {
                                Some(t) => body.push(t),
                                None => break,
                            }
                        }
                    }
                }
                V::Proc(Rc::new(body))
            }
            b'}' => {
                self.pos += 1;
                V::Null
            }
            b'/' => {
                self.pos += 1;
                let imm = self.d.get(self.pos) == Some(&b'/');
                if imm {
                    self.pos += 1;
                }
                let w = String::from_utf8_lossy(self.word()).into_owned();
                // `//name` (immediately evaluated) is treated like a plain name.
                if imm { V::Name(w.into()) } else { V::Lit(w.into()) }
            }
            b'(' => {
                self.pos += 1;
                let mut out = vec![];
                let mut depth = 1;
                while self.pos < self.d.len() {
                    let c = self.d[self.pos];
                    self.pos += 1;
                    match c {
                        b'\\' => {
                            let e = self.d.get(self.pos).copied().unwrap_or(b'\\');
                            self.pos += 1;
                            match e {
                                b'n' => out.push(b'\n'),
                                b'r' => out.push(b'\r'),
                                b't' => out.push(b'\t'),
                                b'0'..=b'7' => {
                                    let mut v = (e - b'0') as u32;
                                    for _ in 0..2 {
                                        match self.d.get(self.pos) {
                                            Some(d @ b'0'..=b'7') => {
                                                v = v * 8 + (d - b'0') as u32;
                                                self.pos += 1;
                                            }
                                            _ => break,
                                        }
                                    }
                                    out.push(v as u8);
                                }
                                b'\n' | b'\r' => {}
                                o => out.push(o),
                            }
                        }
                        b'(' => {
                            depth += 1;
                            out.push(c);
                        }
                        b')' => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                            out.push(c);
                        }
                        _ => out.push(c),
                    }
                }
                V::Str(Rc::new(RefCell::new(out)))
            }
            b'<' if self.d.get(self.pos + 1) == Some(&b'<') => {
                self.pos += 2;
                V::Name("<<".into())
            }
            b'>' if self.d.get(self.pos + 1) == Some(&b'>') => {
                self.pos += 2;
                V::Name(">>".into())
            }
            b'<' if self.d.get(self.pos + 1) == Some(&b'~') => {
                let s = self.pos;
                while self.pos + 1 < self.d.len() && !(self.d[self.pos] == b'~' && self.d[self.pos + 1] == b'>') {
                    self.pos += 1;
                }
                self.pos = (self.pos + 2).min(self.d.len());
                V::Str(Rc::new(RefCell::new(ascii85(&self.d[s..self.pos]))))
            }
            b'<' => {
                self.pos += 1;
                let mut hex = vec![];
                while self.pos < self.d.len() && self.d[self.pos] != b'>' {
                    if self.d[self.pos].is_ascii_hexdigit() {
                        hex.push(self.d[self.pos]);
                    }
                    self.pos += 1;
                }
                self.pos += 1;
                if hex.len() % 2 == 1 {
                    hex.push(b'0');
                }
                let h = |c: u8| (c as char).to_digit(16).unwrap_or(0) as u8;
                V::Str(Rc::new(RefCell::new(hex.chunks(2).map(|p| (h(p[0]) << 4) | h(p[1])).collect())))
            }
            b'[' | b']' => {
                self.pos += 1;
                V::Name(if c == b'[' { "[" } else { "]" }.into())
            }
            b')' | b'>' => {
                self.pos += 1;
                V::Null
            }
            _ => {
                let w = self.word();
                if w.is_empty() {
                    self.pos += 1;
                    return Some(V::Null);
                }
                match parse_num(w) {
                    Some(n) => V::Num(n),
                    None => {
                        // Radix numbers (16#FF).
                        let s = String::from_utf8_lossy(w).into_owned();
                        match s.split_once('#').and_then(|(r, v)| i64::from_str_radix(v, r.parse::<u32>().ok().filter(|r| (2..=36).contains(r))?).ok()) {
                            Some(n) => V::Num(n as f64),
                            None => V::Name(s.into()),
                        }
                    }
                }
            }
        })
    }

    /// Step over inline image data that follows an `image` operator reading `currentfile`.
    fn skip_image_data(&mut self) {
        self.skip_ws();
        if self.d[self.pos..].starts_with(b"<~") || self.d.get(self.pos).is_some_and(|c| (b'!'..=b'u').contains(c) && !c.is_ascii_hexdigit()) {
            match self.d[self.pos..].windows(2).position(|w| w == b"~>") {
                Some(p) => self.pos += p + 2,
                None => self.pos = self.d.len(),
            }
            return;
        }
        while self.pos < self.d.len() && (self.d[self.pos].is_ascii_hexdigit() || is_white(self.d[self.pos])) {
            self.pos += 1;
        }
        if self.d.get(self.pos) == Some(&b'>') {
            self.pos += 1;
        }
    }
}

pub(crate) struct Interp {
    pub b: Builder,
    st: Vec<V>,
    dicts: Vec<DictRef>,
    gs: GState,
    saved: Vec<GState>,
    budget: usize,
    /// Nested procedure calls (recursion is cut off at [`MAX_CALL_DEPTH`]).
    calls: usize,
    /// Fonts by name: `definefont`d dictionaries.
    font_dir: HashMap<String, DictRef>,
    /// Embedded font programs found in the file (name → (program, CFF)).
    embedded: HashMap<String, (Rc<Vec<u8>>, bool)>,
    /// Fonts built for drawing, by base font and encoding.
    font_cache: HashMap<String, Rc<Font>>,
    page: [f64; 4],
    pub unknown: Vec<String>,
}

fn new_dict() -> DictRef {
    Rc::new(RefCell::new(HashMap::new()))
}

fn array(v: Vec<V>) -> V {
    V::Array(Rc::new(RefCell::new(v)))
}

fn matrix_of(v: &V) -> Option<Affine> {
    let a: Vec<f64> = v.items()?.iter().filter_map(V::num).collect();
    (a.len() == 6).then(|| Affine::new([a[0], a[1], a[2], a[3], a[4], a[5]]))
}

fn matrix_val(m: Affine) -> V {
    array(m.as_coeffs().iter().map(|c| V::Num(*c)).collect())
}

/// The angle from `a` counterclockwise to `b` (degrees, 0 to 360), worked out from what is left
/// of each after whole turns.
fn ccw_turn(a: f64, b: f64) -> f64 {
    (b % 360.0 - a % 360.0).rem_euclid(360.0)
}

/// The most turns one `arc` or `arcn` draws.
const MAX_ARC_TURNS: f64 = 1024.0;

/// The deepest procedure nesting run (deeper recursion stops the program).
const MAX_CALL_DEPTH: usize = 256;

impl Interp {
    pub fn new(page: [f64; 4]) -> Interp {
        Interp {
            b: Builder::new(),
            st: vec![],
            dicts: vec![new_dict()],
            gs: GState {
                ctm: Affine::IDENTITY,
                color: [0.0; 3],
                lw: 1.0,
                cap: Cap::Butt,
                join: Join::Miter,
                miter: 10.0,
                dash: None,
                path: BezPath::new(),
                cur: None,
                start: None,
                depth: 1,
                font: None,
            },
            saved: vec![],
            budget: 8_000_000,
            calls: 0,
            font_dir: HashMap::new(),
            embedded: HashMap::new(),
            font_cache: HashMap::new(),
            page,
            unknown: vec![],
        }
    }

    pub fn run(&mut self, data: &[u8]) {
        self.embedded = embedded_fonts(data);
        let mut src = Src { d: data, pos: 0 };
        while let Some(t) = src.token(0) {
            if let Err(Flow::Quit) = self.exec_token(t, &mut src) {
                break;
            }
        }
    }

    fn pop(&mut self) -> V {
        self.st.pop().unwrap_or(V::Null)
    }
    fn popn(&mut self) -> f64 {
        self.pop().num().unwrap_or(0.0)
    }
    fn push(&mut self, v: V) {
        if self.st.len() < 100_000 {
            self.st.push(v);
        }
    }

    fn lookup(&self, k: &str) -> Option<V> {
        self.dicts.iter().rev().find_map(|d| d.borrow().get(k).cloned())
    }

    /// A token read from the file: procedures are pushed, names executed.
    fn exec_token(&mut self, t: V, src: &mut Src) -> Result<(), Flow> {
        match t {
            V::Name(n) => self.exec_name(&n, src),
            other => {
                self.push(other);
                Ok(())
            }
        }
    }

    /// Execute a value as an object (procedures run).
    fn exec_obj(&mut self, v: V, src: &mut Src) -> Result<(), Flow> {
        match v {
            V::Proc(p) => {
                if self.calls >= MAX_CALL_DEPTH {
                    self.b.skip("PostScript (recursion limit)");
                    return Err(Flow::Quit);
                }
                self.calls += 1;
                let mut r = Ok(());
                for x in p.iter() {
                    match x {
                        V::Name(n) => {
                            r = self.exec_name(n, src);
                            if r.is_err() {
                                break;
                            }
                        }
                        other => self.push(other.clone()),
                    }
                }
                self.calls -= 1;
                r
            }
            V::Name(n) => self.exec_name(&n, src),
            other => {
                self.push(other);
                Ok(())
            }
        }
    }

    fn exec_name(&mut self, n: &str, src: &mut Src) -> Result<(), Flow> {
        if self.budget == 0 {
            self.b.skip("PostScript (operation limit)");
            return Err(Flow::Quit);
        }
        self.budget -= 1;
        if let Some(v) = self.lookup(n) {
            return match v {
                V::Proc(_) => self.exec_obj(v, src),
                other => {
                    self.push(other);
                    Ok(())
                }
            };
        }
        self.builtin(n, src)
    }

    fn device_scale(&self) -> f64 {
        self.gs.ctm.determinant().abs().sqrt()
    }

    fn user_point(&mut self) -> Point {
        let y = self.popn();
        let x = self.popn();
        Point::new(x, y)
    }

    fn move_to(&mut self, p: Point) {
        let q = self.gs.ctm * p;
        self.gs.path.move_to(q);
        self.gs.cur = Some(q);
        self.gs.start = Some(q);
    }
    fn line_to(&mut self, p: Point) {
        let q = self.gs.ctm * p;
        if self.gs.cur.is_none() {
            self.gs.path.move_to(q);
            self.gs.start = Some(q);
        }
        self.gs.path.line_to(q);
        self.gs.cur = Some(q);
    }
    fn curve_to(&mut self, a: Point, b: Point, c: Point) {
        let m = self.gs.ctm;
        if self.gs.cur.is_none() {
            self.gs.path.move_to(m * a);
        }
        self.gs.path.curve_to(m * a, m * b, m * c);
        self.gs.cur = Some(m * c);
    }
    fn current_user(&self) -> Point {
        self.gs.ctm.inverse() * self.gs.cur.unwrap_or_default()
    }

    fn arc(&mut self, neg: bool) {
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
            self.line_to(start);
        } else {
            self.move_to(start);
        }
        let arc = kurbo::Arc::new(c, (r, r), a1, sweep, 0.0);
        let m = self.gs.ctm;
        arc.to_cubic_beziers(0.1, |p1, p2, p3| {
            self.gs.path.curve_to(m * p1, m * p2, m * p3);
            self.gs.cur = Some(m * p3);
        });
    }

    fn paint(&mut self, fill: Option<FillRule>, stroke: bool) {
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

    fn rect_path(&mut self) -> Vec<BezPath> {
        // x y w h, or an array / string of numbers.
        let top = self.pop();
        let nums: Vec<f64> = match &top {
            V::Array(_) | V::Proc(_) => top.items().unwrap_or_default().iter().filter_map(V::num).collect(),
            V::Num(h) => {
                let w = self.popn();
                let y = self.popn();
                let x = self.popn();
                vec![x, y, w, *h]
            }
            _ => vec![],
        };
        let m = self.gs.ctm;
        nums.as_chunks::<4>()
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
            .collect()
    }

    fn grestore(&mut self) {
        if let Some(g) = self.saved.pop() {
            let d = g.depth;
            self.gs = g;
            self.b.close_to(d);
        }
    }

    fn builtin(&mut self, n: &str, src: &mut Src) -> Result<(), Flow> {
        match n {
            // Stack.
            "pop" => {
                self.pop();
            }
            "exch" => {
                let b = self.pop();
                let a = self.pop();
                self.push(b);
                self.push(a);
            }
            "dup" => {
                let a = self.st.last().cloned().unwrap_or(V::Null);
                self.push(a);
            }
            "copy" => match self.pop() {
                V::Num(k) => {
                    let k = (k.max(0.0) as usize).min(self.st.len());
                    let tail: Vec<V> = self.st[self.st.len() - k..].to_vec();
                    self.st.extend(tail);
                }
                V::Array(dst) => {
                    let s = self.pop().items().unwrap_or_default();
                    let mut d = dst.borrow_mut();
                    for (i, v) in s.into_iter().enumerate() {
                        if i < d.len() {
                            d[i] = v;
                        }
                    }
                    drop(d);
                    self.push(V::Array(dst));
                }
                other => self.push(other),
            },
            "index" => {
                let k = self.popn().max(0.0) as usize;
                let v = if k < self.st.len() { self.st[self.st.len() - 1 - k].clone() } else { V::Null };
                self.push(v);
            }
            "roll" => {
                let j = self.popn() as i64;
                let k = (self.popn().max(0.0) as usize).min(self.st.len());
                if k > 0 {
                    let s = self.st.len() - k;
                    let r = j.rem_euclid(k as i64) as usize;
                    self.st[s..].rotate_right(r);
                }
            }
            "clear" => self.st.clear(),
            "count" => self.push(V::Num(self.st.len() as f64)),
            "mark" | "[" | "<<" => self.push(V::Mark),
            "cleartomark" => {
                while let Some(v) = self.st.pop() {
                    if matches!(v, V::Mark) {
                        break;
                    }
                }
            }
            "counttomark" => {
                let k = self.st.iter().rev().position(|v| matches!(v, V::Mark)).unwrap_or(self.st.len());
                self.push(V::Num(k as f64));
            }
            "]" => {
                let k = self.st.iter().rev().position(|v| matches!(v, V::Mark)).unwrap_or(self.st.len());
                let items = self.st.split_off(self.st.len() - k);
                self.st.pop();
                self.push(array(items));
            }
            ">>" => {
                let k = self.st.iter().rev().position(|v| matches!(v, V::Mark)).unwrap_or(self.st.len());
                let items = self.st.split_off(self.st.len() - k);
                self.st.pop();
                let d = new_dict();
                for kv in items.as_chunks::<2>().0 {
                    d.borrow_mut().insert(kv[0].key(), kv[1].clone());
                }
                self.push(V::Dict(d));
            }
            // Arithmetic.
            "add" | "sub" | "mul" | "div" | "idiv" | "mod" | "atan" | "exp" | "max" | "min" => {
                let b = self.popn();
                let a = self.popn();
                let v = match n {
                    "add" => a + b,
                    "sub" => a - b,
                    "mul" => a * b,
                    "div" => {
                        if b == 0.0 {
                            0.0
                        } else {
                            a / b
                        }
                    }
                    // `checked_*` give 0 both for a zero divisor and for the one overflowing
                    // case, `i64::MIN / -1` (`/` and `%` panic on it in every build).
                    "idiv" => (a as i64).checked_div(b as i64).unwrap_or(0) as f64,
                    "mod" => (a as i64).checked_rem(b as i64).unwrap_or(0) as f64,
                    "atan" => a.atan2(b).to_degrees().rem_euclid(360.0),
                    "exp" => a.powf(b),
                    "max" => a.max(b),
                    _ => a.min(b),
                };
                self.push(V::Num(if v.is_finite() { v } else { 0.0 }));
            }
            "neg" | "abs" | "sqrt" | "sin" | "cos" | "ln" | "log" | "cvi" | "cvr" | "round" | "floor" | "ceiling" | "truncate" => {
                let a = match self.pop() {
                    V::Str(s) => parse_num(&s.borrow()).unwrap_or(0.0),
                    v => v.num().unwrap_or(0.0),
                };
                let v = match n {
                    "neg" => -a,
                    "abs" => a.abs(),
                    "sqrt" => a.max(0.0).sqrt(),
                    "sin" => a.to_radians().sin(),
                    "cos" => a.to_radians().cos(),
                    "ln" => a.ln(),
                    "log" => a.log10(),
                    "cvi" | "truncate" => a.trunc(),
                    "round" => (a + 0.5).floor(),
                    "floor" => a.floor(),
                    "ceiling" => a.ceil(),
                    _ => a,
                };
                self.push(V::Num(if v.is_finite() { v } else { 0.0 }));
            }
            // Comparison and logic.
            "eq" | "ne" => {
                let b = self.pop();
                let a = self.pop();
                let eq = match (&a, &b) {
                    (V::Num(x), V::Num(y)) => x == y,
                    (V::Bool(x), V::Bool(y)) => x == y,
                    (V::Null, V::Null) => true,
                    (V::Array(x), V::Array(y)) => Rc::ptr_eq(x, y),
                    (V::Dict(x), V::Dict(y)) => Rc::ptr_eq(x, y),
                    (x, y) => !x.key().is_empty() && x.key() == y.key(),
                };
                self.push(V::Bool(eq == (n == "eq")));
            }
            "gt" | "ge" | "lt" | "le" => {
                let b = self.popn();
                let a = self.popn();
                self.push(V::Bool(match n {
                    "gt" => a > b,
                    "ge" => a >= b,
                    "lt" => a < b,
                    _ => a <= b,
                }));
            }
            "not" => match self.pop() {
                V::Bool(b) => self.push(V::Bool(!b)),
                v => self.push(V::Num(!(v.num().unwrap_or(0.0) as i64) as f64)),
            },
            "and" | "or" | "xor" => {
                let b = self.pop();
                let a = self.pop();
                match (a, b) {
                    (V::Bool(x), V::Bool(y)) => self.push(V::Bool(match n {
                        "and" => x && y,
                        "or" => x || y,
                        _ => x ^ y,
                    })),
                    (x, y) => {
                        let (x, y) = (x.num().unwrap_or(0.0) as i64, y.num().unwrap_or(0.0) as i64);
                        self.push(V::Num(match n {
                            "and" => x & y,
                            "or" => x | y,
                            _ => x ^ y,
                        } as f64));
                    }
                }
            }
            "true" => self.push(V::Bool(true)),
            "false" => self.push(V::Bool(false)),
            "null" => self.push(V::Null),
            // Control.
            "exec" => {
                let v = self.pop();
                self.exec_obj(v, src)?;
            }
            "if" => {
                let p = self.pop();
                if let V::Bool(true) = self.pop() {
                    self.exec_obj(p, src)?;
                }
            }
            "ifelse" => {
                let f = self.pop();
                let t = self.pop();
                let c = matches!(self.pop(), V::Bool(true));
                self.exec_obj(if c { t } else { f }, src)?;
            }
            "repeat" => {
                let p = self.pop();
                let k = self.popn().clamp(0.0, 1e6) as usize;
                for _ in 0..k {
                    match self.exec_obj(p.clone(), src) {
                        Err(Flow::Exit) => break,
                        r => r?,
                    }
                }
            }
            "for" => {
                let p = self.pop();
                let lim = self.popn();
                let inc = self.popn();
                let mut i = self.popn();
                let mut guard = 0;
                while inc != 0.0 && ((inc > 0.0 && i <= lim) || (inc < 0.0 && i >= lim)) && guard < 1_000_000 {
                    self.push(V::Num(i));
                    match self.exec_obj(p.clone(), src) {
                        Err(Flow::Exit) => break,
                        r => r?,
                    }
                    i += inc;
                    guard += 1;
                }
            }
            "loop" => {
                let p = self.pop();
                for _ in 0..1_000_000 {
                    match self.exec_obj(p.clone(), src) {
                        Err(Flow::Exit) => break,
                        r => r?,
                    }
                }
            }
            "forall" => {
                let p = self.pop();
                match self.pop() {
                    V::Dict(d) => {
                        let items: Vec<(String, V)> = d.borrow().iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                        for (k, v) in items {
                            self.push(V::Lit(k.into()));
                            self.push(v);
                            match self.exec_obj(p.clone(), src) {
                                Err(Flow::Exit) => break,
                                r => r?,
                            }
                        }
                    }
                    V::Str(s) => {
                        let bytes = s.borrow().clone();
                        for c in bytes {
                            self.push(V::Num(c as f64));
                            match self.exec_obj(p.clone(), src) {
                                Err(Flow::Exit) => break,
                                r => r?,
                            }
                        }
                    }
                    a => {
                        for v in a.items().unwrap_or_default() {
                            self.push(v);
                            match self.exec_obj(p.clone(), src) {
                                Err(Flow::Exit) => break,
                                r => r?,
                            }
                        }
                    }
                }
            }
            "exit" => return Err(Flow::Exit),
            "stop" => return Err(Flow::Stop),
            "stopped" => {
                let p = self.pop();
                let stopped = match self.exec_obj(p, src) {
                    Err(Flow::Quit) => return Err(Flow::Quit),
                    Err(_) => true,
                    Ok(()) => false,
                };
                self.push(V::Bool(stopped));
            }
            "quit" => return Err(Flow::Quit),
            "showpage" | "copypage" => {}
            // Dictionaries.
            "def" => {
                let v = self.pop();
                let k = self.pop().key();
                if let Some(d) = self.dicts.last() {
                    d.borrow_mut().insert(k, v);
                }
            }
            "store" => {
                let v = self.pop();
                let k = self.pop().key();
                let d = self.dicts.iter().rev().find(|d| d.borrow().contains_key(&k)).or(self.dicts.last()).cloned();
                if let Some(d) = d {
                    d.borrow_mut().insert(k, v);
                }
            }
            "load" => {
                let k = self.pop().key();
                let v = self.lookup(&k).unwrap_or(V::Name(k.into()));
                self.push(v);
            }
            "undef" => {
                let k = self.pop().key();
                if let V::Dict(d) = self.pop() {
                    d.borrow_mut().remove(&k);
                }
            }
            "dict" => {
                self.pop();
                self.push(V::Dict(new_dict()));
            }
            "begin" => {
                if let V::Dict(d) = self.pop()
                    && self.dicts.len() < 64
                {
                    self.dicts.push(d);
                }
            }
            "end" => {
                if self.dicts.len() > 1 {
                    self.dicts.pop();
                }
            }
            "currentdict" => {
                let d = self.dicts.last().cloned().unwrap_or_else(new_dict);
                self.push(V::Dict(d));
            }
            "userdict" | "globaldict" | "systemdict" | "statusdict" | "errordict" | "$error" => {
                let d = self.dicts[0].clone();
                self.push(V::Dict(d));
            }
            "where" => {
                let k = self.pop().key();
                match self.dicts.iter().rev().find(|d| d.borrow().contains_key(&k)).cloned() {
                    Some(d) => {
                        self.push(V::Dict(d));
                        self.push(V::Bool(true));
                    }
                    None => self.push(V::Bool(false)),
                }
            }
            "known" => {
                let k = self.pop().key();
                let v = match self.pop() {
                    V::Dict(d) => d.borrow().contains_key(&k),
                    _ => false,
                };
                self.push(V::Bool(v));
            }
            "countdictstack" => self.push(V::Num(self.dicts.len() as f64)),
            "cleardictstack" => self.dicts.truncate(1),
            "findresource" | "findfont" | "findencoding" => {
                let category = if n == "findresource" { self.pop().key() } else { String::new() };
                let key = self.pop().key();
                match (n, category.as_str()) {
                    ("findfont", _) | (_, "Font") => {
                        let f = self.find_font(&key);
                        self.push(V::Dict(f));
                    }
                    ("findencoding", _) | (_, "Encoding") => self.push(encoding_array(&key)),
                    _ => self.push(V::Dict(new_dict())),
                }
            }
            "defineresource" => {
                let category = self.pop().key();
                let v = self.pop();
                let key = self.pop().key();
                if let (V::Dict(d), "Font") = (&v, category.as_str()) {
                    self.font_dir.insert(key, d.clone());
                }
                self.push(v);
            }
            "resourcestatus" => {
                self.pop();
                self.pop();
                self.push(V::Bool(false));
            }
            // Arrays and strings.
            "array" => {
                let k = self.popn().clamp(0.0, 1e6) as usize;
                self.push(array(vec![V::Null; k]));
            }
            "string" => {
                let k = self.popn().clamp(0.0, 1e7) as usize;
                self.push(V::Str(Rc::new(RefCell::new(vec![0; k]))));
            }
            "length" => {
                let l = match self.pop() {
                    V::Str(s) => s.borrow().len(),
                    V::Dict(d) => d.borrow().len(),
                    V::Name(s) | V::Lit(s) => s.len(),
                    v => v.items().map_or(0, |i| i.len()),
                };
                self.push(V::Num(l as f64));
            }
            "get" => {
                let k = self.pop();
                let v = match self.pop() {
                    V::Dict(d) => d.borrow().get(&k.key()).cloned().unwrap_or(V::Null),
                    V::Str(s) => V::Num(s.borrow().get(k.num().unwrap_or(0.0) as usize).copied().unwrap_or(0) as f64),
                    a => a.items().and_then(|i| i.get(k.num().unwrap_or(0.0) as usize).cloned()).unwrap_or(V::Null),
                };
                self.push(v);
            }
            "put" => {
                let v = self.pop();
                let k = self.pop();
                match self.pop() {
                    V::Dict(d) => {
                        d.borrow_mut().insert(k.key(), v);
                    }
                    V::Array(a) => {
                        let i = k.num().unwrap_or(0.0) as usize;
                        if let Some(slot) = a.borrow_mut().get_mut(i) {
                            *slot = v;
                        }
                    }
                    V::Str(s) => {
                        let i = k.num().unwrap_or(0.0) as usize;
                        if let Some(slot) = s.borrow_mut().get_mut(i) {
                            *slot = v.num().unwrap_or(0.0) as u8;
                        }
                    }
                    _ => {}
                }
            }
            "getinterval" => {
                let c = self.popn().max(0.0) as usize;
                let i = self.popn().max(0.0) as usize;
                match self.pop() {
                    V::Str(s) => {
                        let b = s.borrow();
                        let part = b.get(i.min(b.len())..(i + c).min(b.len())).unwrap_or(&[]).to_vec();
                        drop(b);
                        self.push(V::Str(Rc::new(RefCell::new(part))));
                    }
                    a => {
                        let items = a.items().unwrap_or_default();
                        let part = items.get(i.min(items.len())..(i + c).min(items.len())).unwrap_or(&[]).to_vec();
                        self.push(array(part));
                    }
                }
            }
            "putinterval" => {
                self.pop();
                self.pop();
                self.pop();
            }
            "aload" => {
                let a = self.pop();
                for v in a.items().unwrap_or_default() {
                    self.push(v);
                }
                self.push(a);
            }
            "astore" => {
                let a = self.pop();
                if let V::Array(arr) = &a {
                    let k = arr.borrow().len().min(self.st.len());
                    let items = self.st.split_off(self.st.len() - k);
                    *arr.borrow_mut() = items;
                }
                self.push(a);
            }
            "cvx" => match self.pop() {
                V::Array(a) => self.push(V::Proc(Rc::new(a.borrow().clone()))),
                V::Lit(s) => self.push(V::Name(s)),
                V::Str(s) => {
                    // Executable string: tokenize it as a procedure.
                    let bytes = s.borrow().clone();
                    let mut sub = Src { d: &bytes, pos: 0 };
                    let mut body = vec![];
                    while let Some(t) = sub.token(0) {
                        body.push(t);
                    }
                    self.push(V::Proc(Rc::new(body)));
                }
                v => self.push(v),
            },
            "cvlit" => match self.pop() {
                V::Proc(p) => self.push(array(p.as_ref().clone())),
                V::Name(s) => self.push(V::Lit(s)),
                v => self.push(v),
            },
            "cvn" => {
                let k = self.pop().key();
                self.push(V::Lit(k.into()));
            }
            "cvs" => {
                self.pop();
                let k = self.pop().key();
                self.push(V::Str(Rc::new(RefCell::new(k.into_bytes()))));
            }
            "xcheck" => {
                let v = self.pop();
                self.push(V::Bool(matches!(v, V::Proc(_) | V::Name(_))));
            }
            "type" => {
                let t = match self.pop() {
                    V::Num(_) => "realtype",
                    V::Bool(_) => "booleantype",
                    V::Name(_) | V::Lit(_) => "nametype",
                    V::Str(_) => "stringtype",
                    V::Proc(_) | V::Array(_) => "arraytype",
                    V::Dict(_) => "dicttype",
                    V::Mark => "marktype",
                    V::Null => "nulltype",
                    V::File => "filetype",
                };
                self.push(V::Name(t.into()));
            }
            "bind" | "readonly" | "executeonly" | "noaccess" => {}
            "languagelevel" => self.push(V::Num(3.0)),
            "version" | "product" => self.push(V::Str(Rc::new(RefCell::new(b"3010".to_vec())))),
            "save" => {
                self.saved.push(self.gs.clone());
                self.gs.depth = self.b.depth();
                self.push(V::Null);
            }
            "restore" => {
                self.pop();
                self.grestore();
            }
            "currentfile" => self.push(V::File),
            "readstring" | "readhexstring" | "readline" => {
                self.pop();
                self.pop();
                self.push(V::Str(Rc::new(RefCell::new(vec![]))));
                self.push(V::Bool(false));
            }
            "flushfile" | "closefile" | "print" | "==" | "=" | "pstack" | "setpagedevice" | "setglobal" | "setpacking" | "setshared" | "setobjectformat"
            | "setsmoothness" => {
                if !matches!(n, "pstack") {
                    self.pop();
                }
            }
            "currentglobal" | "currentpacking" | "currentshared" => self.push(V::Bool(false)),
            "vmstatus" => {
                for _ in 0..3 {
                    self.push(V::Num(0.0));
                }
            }
            // Graphics state.
            "gsave" => {
                let mut g = self.gs.clone();
                g.depth = self.b.depth();
                self.saved.push(g);
            }
            "grestore" => self.grestore(),
            "grestoreall" => {
                while !self.saved.is_empty() {
                    self.grestore();
                }
            }
            "initgraphics" => {
                self.gs.ctm = Affine::IDENTITY;
                self.gs.lw = 1.0;
                self.gs.color = [0.0; 3];
            }
            "setlinewidth" => self.gs.lw = self.popn(),
            "currentlinewidth" => self.push(V::Num(self.gs.lw)),
            "setlinecap" => self.gs.cap = [Cap::Butt, Cap::Round, Cap::Square][(self.popn().max(0.0) as usize).min(2)],
            "setlinejoin" => self.gs.join = [Join::Miter, Join::Round, Join::Bevel][(self.popn().max(0.0) as usize).min(2)],
            "setmiterlimit" => self.gs.miter = self.popn(),
            "setdash" => {
                let off = self.popn();
                let a: Vec<f64> = self.pop().items().unwrap_or_default().iter().filter_map(V::num).collect();
                self.gs.dash = (!a.is_empty() && a.iter().any(|v| *v > 0.0)).then_some((a, off));
            }
            "setflat" | "setstrokeadjust" | "setoverprint" | "setrenderingintent" | "setcolorrendering" | "sethalftone" | "setscreen" | "settransfer" => {
                self.pop();
            }
            "setgray" => {
                let g = self.popn().clamp(0.0, 1.0);
                self.gs.color = [g; 3];
            }
            "setrgbcolor" => {
                let b = self.popn();
                let g = self.popn();
                let r = self.popn();
                self.gs.color = [r, g, b].map(|v| v.clamp(0.0, 1.0));
            }
            "sethsbcolor" => {
                let v = self.popn().clamp(0.0, 1.0);
                let s = self.popn().clamp(0.0, 1.0);
                let h = self.popn().rem_euclid(1.0) * 6.0;
                let (i, f) = (h.floor() as i32, h.fract());
                let (p, q, t) = (v * (1.0 - s), v * (1.0 - s * f), v * (1.0 - s * (1.0 - f)));
                self.gs.color = match i {
                    0 => [v, t, p],
                    1 => [q, v, p],
                    2 => [p, v, t],
                    3 => [p, q, v],
                    4 => [t, p, v],
                    _ => [v, p, q],
                };
            }
            "setcmykcolor" => {
                let k = self.popn().clamp(0.0, 1.0);
                let y = self.popn().clamp(0.0, 1.0);
                let m = self.popn().clamp(0.0, 1.0);
                let c = self.popn().clamp(0.0, 1.0);
                self.gs.color = [(1.0 - c) * (1.0 - k), (1.0 - m) * (1.0 - k), (1.0 - y) * (1.0 - k)];
            }
            "setcolorspace" => {
                self.pop();
            }
            "setcolor" => {
                // Device colour by operand count (patterns are not drawn).
                let k = self.st.iter().rev().take_while(|v| matches!(v, V::Num(_))).count().min(4);
                let c: Vec<f64> = self.st.split_off(self.st.len() - k).iter().filter_map(V::num).collect();
                self.gs.color = match c.len() {
                    1 => [c[0]; 3],
                    3 => [c[0], c[1], c[2]],
                    4 => [(1.0 - c[0]) * (1.0 - c[3]), (1.0 - c[1]) * (1.0 - c[3]), (1.0 - c[2]) * (1.0 - c[3])],
                    _ => self.gs.color,
                };
            }
            "currentgray" => self.push(V::Num((self.gs.color[0] + self.gs.color[1] + self.gs.color[2]) / 3.0)),
            "currentrgbcolor" => {
                for c in self.gs.color {
                    self.push(V::Num(c));
                }
            }
            // Coordinates.
            "matrix" => self.push(matrix_val(Affine::IDENTITY)),
            "identmatrix" | "defaultmatrix" => {
                self.pop();
                self.push(matrix_val(Affine::IDENTITY));
            }
            "currentmatrix" => {
                self.pop();
                self.push(matrix_val(self.gs.ctm));
            }
            "setmatrix" => {
                let m = self.pop();
                self.gs.ctm = matrix_of(&m).unwrap_or(Affine::IDENTITY);
            }
            "concat" => {
                let m = self.pop();
                if let Some(m) = matrix_of(&m) {
                    self.gs.ctm *= m;
                }
            }
            "concatmatrix" => {
                self.pop();
                let b = self.pop();
                let a = self.pop();
                let (a, b) = (matrix_of(&a).unwrap_or(Affine::IDENTITY), matrix_of(&b).unwrap_or(Affine::IDENTITY));
                self.push(matrix_val(b * a));
            }
            "invertmatrix" => {
                self.pop();
                let a = self.pop();
                self.push(matrix_val(matrix_of(&a).unwrap_or(Affine::IDENTITY).inverse()));
            }
            "translate" | "scale" => {
                let top = self.pop();
                let (m_out, y) = match top {
                    V::Array(_) => (Some(top), self.popn()),
                    v => (None, v.num().unwrap_or(0.0)),
                };
                let x = self.popn();
                let t = if n == "translate" { Affine::translate((x, y)) } else { Affine::scale_non_uniform(x, y) };
                match m_out {
                    Some(_) => self.push(matrix_val(t)),
                    None => self.gs.ctm *= t,
                }
            }
            "rotate" => {
                let top = self.pop();
                match top {
                    V::Array(_) => {
                        let a = self.popn();
                        self.push(matrix_val(Affine::rotate(a.to_radians())));
                    }
                    v => self.gs.ctm *= Affine::rotate(v.num().unwrap_or(0.0).to_radians()),
                }
            }
            "transform" | "itransform" | "dtransform" | "idtransform" => {
                let top = self.pop();
                let (m, y) = match top {
                    V::Array(_) | V::Proc(_) => (matrix_of(&top).unwrap_or(Affine::IDENTITY), self.popn()),
                    v => (self.gs.ctm, v.num().unwrap_or(0.0)),
                };
                let x = self.popn();
                let m = if n.starts_with('i') { m.inverse() } else { m };
                let p = if n.contains('d') {
                    let c = m.as_coeffs();
                    Point::new(c[0] * x + c[2] * y, c[1] * x + c[3] * y)
                } else {
                    m * Point::new(x, y)
                };
                self.push(V::Num(p.x));
                self.push(V::Num(p.y));
            }
            // Paths.
            "newpath" => {
                self.gs.path = BezPath::new();
                self.gs.cur = None;
                self.gs.start = None;
            }
            "moveto" => {
                let p = self.user_point();
                self.move_to(p);
            }
            "rmoveto" => {
                let d = self.user_point();
                let c = self.current_user();
                self.move_to(Point::new(c.x + d.x, c.y + d.y));
            }
            "lineto" => {
                let p = self.user_point();
                self.line_to(p);
            }
            "rlineto" => {
                let d = self.user_point();
                let c = self.current_user();
                self.line_to(Point::new(c.x + d.x, c.y + d.y));
            }
            "curveto" | "rcurveto" => {
                let p3 = self.user_point();
                let p2 = self.user_point();
                let p1 = self.user_point();
                if n == "rcurveto" {
                    let c = self.current_user();
                    let o = |p: Point| Point::new(c.x + p.x, c.y + p.y);
                    self.curve_to(o(p1), o(p2), o(p3));
                } else {
                    self.curve_to(p1, p2, p3);
                }
            }
            "closepath" => {
                if self.gs.cur.is_some() {
                    self.gs.path.close_path();
                    self.gs.cur = self.gs.start;
                }
            }
            "arc" => self.arc(false),
            "arcn" => self.arc(true),
            "currentpoint" => {
                let c = self.current_user();
                self.push(V::Num(c.x));
                self.push(V::Num(c.y));
            }
            "pathbbox" => {
                let r = kurbo::Shape::bounding_box(&self.gs.path);
                let inv = self.gs.ctm.inverse();
                let (a, b) = (inv * Point::new(r.x0, r.y0), inv * Point::new(r.x1, r.y1));
                for v in [a.x.min(b.x), a.y.min(b.y), a.x.max(b.x), a.y.max(b.y)] {
                    self.push(V::Num(v));
                }
            }
            "flattenpath" | "reversepath" | "strokepath" | "initclip" => {}
            "clippath" => {
                let p = self.page;
                self.gs.path = kurbo::Shape::to_path(&kurbo::Rect::new(p[0], p[1], p[2], p[3]), 0.1);
            }
            // Painting.
            "fill" => self.paint(Some(FillRule::NonZero), false),
            "eofill" => self.paint(Some(FillRule::EvenOdd), false),
            "stroke" => self.paint(None, true),
            "clip" | "eoclip" => {
                let rule = if n == "clip" { FillRule::NonZero } else { FillRule::EvenOdd };
                let p = self.gs.path.clone();
                if !p.elements().iter().all(|e| matches!(e, PathEl::MoveTo(_))) {
                    self.b.clip(p, rule);
                }
            }
            "rectfill" | "rectstroke" | "rectclip" => {
                let paths = self.rect_path();
                if n == "rectstroke" && matches!(self.st.last(), Some(V::Array(_))) {
                    self.pop();
                }
                let mut all = BezPath::new();
                for p in paths {
                    all.extend(p);
                }
                match n {
                    "rectclip" => self.b.clip(all, FillRule::NonZero),
                    _ => {
                        let saved = std::mem::replace(&mut self.gs.path, all);
                        self.paint(if n == "rectfill" { Some(FillRule::NonZero) } else { None }, n == "rectstroke");
                        self.gs.path = saved;
                    }
                }
            }
            "shfill" => {
                self.pop();
                self.b.skip("smooth shading (shfill)");
            }
            // Text (see `text_op`).
            "scalefont" | "makefont" | "selectfont" | "setfont" | "show" | "charpath" | "glyphshow" | "rootfont" | "currentfont" | "setcachedevice"
            | "setcharwidth" | "ashow" | "widthshow" | "awidthshow" | "xshow" | "yshow" | "xyshow" | "kshow" | "cshow" | "stringwidth" | "definefont"
            | "StandardEncoding" | "ISOLatin1Encoding" | "eexec" | "StartData" | "undefinefont" => self.text_op(n, src)?,
            // Images: operands popped, inline data skipped.
            "image" | "imagemask" | "colorimage" => {
                let mut reads_file = false;
                let check = |v: &V, rf: &mut bool| {
                    if matches!(v, V::File) || matches!(v, V::Proc(p) if p.iter().any(|x| matches!(x, V::Name(n) if &**n == "currentfile"))) {
                        *rf = true;
                    }
                };
                if n == "colorimage" {
                    let ncomp = self.popn().max(1.0) as usize;
                    let multi = matches!(self.pop(), V::Bool(true));
                    for _ in 0..if multi { ncomp.min(4) } else { 1 } {
                        let v = self.pop();
                        check(&v, &mut reads_file);
                    }
                    for _ in 0..4 {
                        self.pop();
                    }
                } else {
                    match self.pop() {
                        V::Dict(d) => {
                            if let Some(ds) = d.borrow().get("DataSource") {
                                check(ds, &mut reads_file);
                            }
                        }
                        top => {
                            check(&top, &mut reads_file);
                            for _ in 0..4 {
                                self.pop();
                            }
                        }
                    }
                }
                if reads_file {
                    src.skip_image_data();
                }
                self.b.skip("image");
            }
            _ => {
                if self.unknown.len() < 32 && !self.unknown.iter().any(|u| u == n) {
                    self.unknown.push(n.to_string());
                }
            }
        }
        Ok(())
    }
}

/// The PostScript section of an EPS file (DOS EPS binary header handled) and its bounding box
/// (`%%HiResBoundingBox` preferred).
pub fn eps_parts(bytes: &[u8]) -> Option<(&[u8], [f64; 4])> {
    let ps = if bytes.starts_with(&[0xC5, 0xD0, 0xD3, 0xC6]) && bytes.len() >= 12 {
        let off = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]) as usize;
        let len = u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as usize;
        bytes.get(off..off.checked_add(len)?.min(bytes.len()))?
    } else {
        bytes
    };
    let find_box = |key: &[u8]| -> Option<[f64; 4]> {
        let mut from = 0;
        while let Some(p) = ps[from..].windows(key.len()).position(|w| w == key) {
            let s = from + p + key.len();
            let e = ps[s..].iter().position(|c| matches!(c, b'\n' | b'\r')).map_or(ps.len(), |q| s + q);
            let v: Vec<f64> = String::from_utf8_lossy(&ps[s..e]).split_whitespace().filter_map(|t| t.parse().ok()).collect();
            if v.len() == 4 && v[2] > v[0] && v[3] > v[1] {
                return Some([v[0], v[1], v[2], v[3]]);
            }
            from = s;
        }
        None
    };
    let bbox = find_box(b"%%HiResBoundingBox:").or_else(|| find_box(b"%%BoundingBox:"))?;
    Some((ps, bbox))
}

// ------------------------------------------------------------------ text

/// An encoding array of glyph names (`StandardEncoding`; `ISOLatin1Encoding` and others are
/// approximated by WinAnsi, which agrees with ISO Latin-1 on the printable codes).
fn encoding_array(name: &str) -> V {
    let base = if name == "StandardEncoding" { crate::encoding::Base::Standard } else { crate::encoding::Base::WinAnsi };
    array((0..256).map(|c| V::Lit(base.name(c as u8).unwrap_or_else(|| ".notdef".into()).into())).collect())
}

fn find_from(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    hay.get(from..)?.windows(needle.len()).position(|w| w == needle).map(|p| p + from)
}

fn rfind_before(hay: &[u8], needle: &[u8], to: usize) -> Option<usize> {
    hay.get(..to)?.windows(needle.len()).rposition(|w| w == needle)
}

/// The name after `key` (`/FontName /Name`).
fn name_after(b: &[u8], key: &[u8]) -> Option<String> {
    let p = find_from(b, key, 0)? + key.len();
    let rest = &b[p..];
    let s = rest.iter().position(|c| *c == b'/')? + 1;
    let e = rest[s..].iter().position(|c| is_white(*c) || matches!(c, b'/' | b'[' | b'(' | b'{')).map_or(rest.len(), |q| s + q);
    (e > s).then(|| String::from_utf8_lossy(&rest[s..e]).into_owned())
}

/// Embedded font programs of an EPS file: Type 1 fonts (`… /FontName /X … currentfile eexec
/// … cleartomark`, hexadecimal or binary) and CFF font sets (`… <length> StartData <bytes>`).
fn embedded_fonts(ps: &[u8]) -> HashMap<String, (Rc<Vec<u8>>, bool)> {
    let mut out = HashMap::new();
    let mut from = 0;
    while let Some(ee) = find_from(ps, b"eexec", from) {
        from = ee + 5;
        let start = [b"%!PS-AdobeFont".as_slice(), b"%!FontType1"]
            .iter()
            .filter_map(|h| rfind_before(ps, h, ee))
            .max()
            .or_else(|| rfind_before(ps, b"/FontName", ee).map(|p| p.saturating_sub(512)));
        let Some(start) = start else { continue };
        let end = find_from(ps, b"cleartomark", ee).map_or(ps.len(), |p| p + 11);
        if let Some(name) = name_after(&ps[start..ee], b"/FontName") {
            out.insert(name, (Rc::new(ps[start..end].to_vec()), false));
        }
        from = end;
    }
    let mut from = 0;
    while let Some(sd) = find_from(ps, b"StartData", from) {
        from = sd + 9;
        let head = String::from_utf8_lossy(&ps[sd.saturating_sub(64)..sd]).into_owned();
        let Some(len) = head.split_whitespace().last().and_then(|t| t.parse::<usize>().ok()) else { continue };
        let s = sd + 10;
        let Some(data) = ps.get(s..s.saturating_add(len)) else { continue };
        if let Some(name) = crate::cff::font_name(data) {
            out.insert(name, (Rc::new(data.to_vec()), true));
        }
        from = s + len;
    }
    out
}

fn dict_matrix(d: &DictRef, key: &str) -> Affine {
    d.borrow().get(key).and_then(matrix_of).unwrap_or(Affine::IDENTITY)
}

/// A copy of a font dictionary with `m` applied after its matrix (`scalefont`, `makefont`).
fn transformed(font: &DictRef, m: Affine) -> DictRef {
    let copy: HashMap<String, V> = font.borrow().clone();
    let d = Rc::new(RefCell::new(copy));
    let old = dict_matrix(&d, "__m");
    d.borrow_mut().insert("__m".into(), matrix_val(m * old));
    d
}

impl Interp {
    /// A font dictionary by name: one `definefont` made, else a new dictionary for an
    /// embedded program or a bundled stand-in.
    fn find_font(&mut self, key: &str) -> DictRef {
        if let Some(d) = self.font_dir.get(key) {
            return d.clone();
        }
        let d = new_dict();
        {
            let mut m = d.borrow_mut();
            m.insert("FontName".into(), V::Lit(key.into()));
            m.insert("FontType".into(), V::Num(1.0));
            m.insert("FontMatrix".into(), matrix_val(Affine::scale(0.001)));
            m.insert("FID".into(), V::Null);
            m.insert("__base".into(), V::Lit(key.into()));
            m.insert("__m".into(), matrix_val(Affine::IDENTITY));
        }
        self.font_dir.insert(key.to_string(), d.clone());
        d
    }

    /// The drawable font of a font dictionary (its base program and encoding) and the matrix
    /// from text space (font size 1) to user space.
    fn font_of(&mut self, d: &DictRef) -> Option<(Rc<Font>, Affine)> {
        if matches!(d.borrow().get("FontType"), Some(V::Num(t)) if *t == 3.0) {
            // Type 3 fonts (glyph procedures) are not run.
            self.b.skip("EPS Type 3 font");
            return None;
        }
        let (base, names) = {
            let m = d.borrow();
            let base = m.get("__base").or_else(|| m.get("FontName")).map(V::key)?;
            let names: Option<Vec<Option<String>>> = m.get("Encoding").and_then(V::items).map(|a| {
                a.iter()
                    .map(|v| match v {
                        V::Lit(n) | V::Name(n) if &**n != ".notdef" => Some(n.to_string()),
                        _ => None,
                    })
                    .collect()
            });
            (base, names)
        };
        let key = match &names {
            Some(n) => format!("{base}\u{0}{}", n.iter().map(|x| x.as_deref().unwrap_or("")).collect::<Vec<_>>().join(" ")),
            None => base.clone(),
        };
        let font = match self.font_cache.get(&key) {
            Some(f) => f.clone(),
            None => {
                // Not embedded: a bundled stand-in, as for PDF.
                let prog = self.embedded.get(&base).cloned();
                let f = Rc::new(Font::standalone(&base, prog.as_ref().map(|(d, cff)| (d.as_slice(), *cff)), names));
                self.font_cache.insert(key, f.clone());
                f
            }
        };
        Some((font, dict_matrix(d, "__m")))
    }

    /// A string's glyph outlines (device space) from the current point, with an extra
    /// displacement `extra(index, code)` (user space) after each glyph; moves the current
    /// point.
    fn text_path(&mut self, bytes: &[u8], mut extra: impl FnMut(usize, u32) -> (f64, f64)) -> Option<(BezPath, String)> {
        let d = self.gs.font.clone()?;
        let (font, m) = self.font_of(&d)?;
        let mut p = self.current_user();
        let mut path = BezPath::new();
        let mut text = String::new();
        let ctm = self.gs.ctm;
        for (i, &code) in bytes.iter().enumerate() {
            let code = code as u32;
            if let Some(g) = font.glyph(code) {
                path.extend(ctm * Affine::translate(p.to_vec2()) * m * (*g).clone());
            }
            text.push_str(&font.unicode(code));
            let adv = m * Point::new(font.width(code), 0.0) - m * Point::ZERO;
            let (ex, ey) = extra(i, code);
            p += adv + kurbo::Vec2::new(ex, ey);
        }
        self.move_to(p);
        Some((path, text))
    }

    fn fill_text(&mut self, path: BezPath, text: &str) {
        let label: String = text.chars().filter(|c| !c.is_control()).take(40).collect();
        let name = if label.trim().is_empty() { "Text".to_string() } else { format!("Text: {}", label.trim()) };
        self.b.fill_stroke_named(&name, path, Affine::IDENTITY, Some((Paint::Color(self.gs.color), 1.0, FillRule::NonZero)), None);
    }

    fn pop_string(&mut self) -> Vec<u8> {
        match self.pop() {
            V::Str(s) => s.borrow().clone(),
            _ => vec![],
        }
    }

    fn show_with(&mut self, s: &[u8], extra: impl FnMut(usize, u32) -> (f64, f64)) {
        match self.text_path(s, extra) {
            Some((path, text)) => self.fill_text(path, &text),
            None => self.b.skip("text (no font)"),
        }
    }

    /// Text and font operators (PostScript Language Reference §5.1, §8.2).
    fn text_op(&mut self, n: &str, src: &mut Src) -> Result<(), Flow> {
        match n {
            "scalefont" | "makefont" => {
                let m = if n == "scalefont" {
                    Affine::scale(self.popn())
                } else {
                    let v = self.pop();
                    matrix_of(&v).unwrap_or(Affine::IDENTITY)
                };
                let font = match self.pop() {
                    V::Dict(d) => d,
                    _ => self.find_font("Helvetica"),
                };
                self.push(V::Dict(transformed(&font, m)));
            }
            "selectfont" => {
                let size = self.pop();
                let key = self.pop().key();
                let font = self.find_font(&key);
                let m = match &size {
                    V::Num(s) => Affine::scale(*s),
                    other => matrix_of(other).unwrap_or(Affine::IDENTITY),
                };
                self.gs.font = Some(transformed(&font, m));
            }
            "setfont" => {
                if let V::Dict(d) = self.pop() {
                    self.gs.font = Some(d);
                }
            }
            "currentfont" | "rootfont" => {
                let d = self.gs.font.clone().unwrap_or_else(new_dict);
                self.push(V::Dict(d));
            }
            "definefont" => {
                let v = self.pop();
                let key = self.pop().key();
                if let V::Dict(d) = &v {
                    if !d.borrow().contains_key("__base") {
                        // A font the file defines itself: its embedded program, by name.
                        d.borrow_mut().insert("__base".into(), V::Lit(key.as_str().into()));
                        d.borrow_mut().insert("__m".into(), matrix_val(Affine::IDENTITY));
                    }
                    self.font_dir.insert(key, d.clone());
                }
                self.push(v);
            }
            "undefinefont" => {
                let key = self.pop().key();
                self.font_dir.remove(&key);
            }
            "StandardEncoding" | "ISOLatin1Encoding" => self.push(encoding_array(n)),
            "show" => {
                let s = self.pop_string();
                self.show_with(&s, |_, _| (0.0, 0.0));
            }
            "ashow" => {
                let s = self.pop_string();
                let ay = self.popn();
                let ax = self.popn();
                self.show_with(&s, |_, _| (ax, ay));
            }
            "widthshow" => {
                let s = self.pop_string();
                let ch = self.popn() as u32;
                let cy = self.popn();
                let cx = self.popn();
                self.show_with(&s, |_, c| if c == ch { (cx, cy) } else { (0.0, 0.0) });
            }
            "awidthshow" => {
                let s = self.pop_string();
                let ay = self.popn();
                let ax = self.popn();
                let ch = self.popn() as u32;
                let cy = self.popn();
                let cx = self.popn();
                self.show_with(&s, |_, c| if c == ch { (cx + ax, cy + ay) } else { (ax, ay) });
            }
            "kshow" | "cshow" => {
                // The procedure between characters is not run.
                let s = self.pop_string();
                self.pop();
                self.show_with(&s, |_, _| (0.0, 0.0));
            }
            "xshow" | "yshow" | "xyshow" => {
                let disp: Vec<f64> = self.pop().items().unwrap_or_default().iter().filter_map(V::num).collect();
                let s = self.pop_string();
                self.explicit_show(&s, &disp, n);
            }
            "glyphshow" => {
                let name = self.pop().key();
                let Some(d) = self.gs.font.clone() else { return Ok(()) };
                // Drawn through a one-glyph encoding.
                let mut e: Vec<V> = (0..256).map(|_| V::Lit(".notdef".into())).collect();
                e[65] = V::Lit(name.as_str().into());
                let tmp = transformed(&d, Affine::IDENTITY);
                tmp.borrow_mut().insert("Encoding".into(), array(e));
                self.gs.font = Some(tmp);
                self.show_with(b"A", |_, _| (0.0, 0.0));
                self.gs.font = Some(d);
            }
            "charpath" => {
                self.pop();
                let s = self.pop_string();
                let saved = std::mem::take(&mut self.gs.path);
                let start = self.gs.start;
                match self.text_path(&s, |_, _| (0.0, 0.0)) {
                    Some((glyphs, _)) => {
                        let mut p = saved;
                        p.extend(glyphs);
                        if let Some(c) = self.gs.cur {
                            p.move_to(c);
                        }
                        self.gs.path = p;
                        self.gs.start = start;
                    }
                    None => self.gs.path = saved,
                }
            }
            "stringwidth" => {
                let s = self.pop_string();
                let (mut w, mut h) = (0.0, 0.0);
                if let Some(d) = self.gs.font.clone()
                    && let Some((font, m)) = self.font_of(&d)
                {
                    for &c in &s {
                        let v = m * Point::new(font.width(c as u32), 0.0) - m * Point::ZERO;
                        w += v.x;
                        h += v.y;
                    }
                }
                self.push(V::Num(w));
                self.push(V::Num(h));
            }
            "setcachedevice" => {
                for _ in 0..6 {
                    self.pop();
                }
            }
            "setcharwidth" => {
                self.pop();
                self.pop();
            }
            // An embedded Type 1 program's encrypted part (read by `embedded_fonts`).
            "eexec" => {
                if matches!(self.pop(), V::File) {
                    let rest = &src.d[src.pos.min(src.d.len())..];
                    src.pos += rest.windows(11).position(|w| w == b"cleartomark").map_or(rest.len(), |p| p + 11);
                }
            }
            // A CFF font set's binary data (read by `embedded_fonts`).
            "StartData" => {
                let len = self.popn().max(0.0) as usize;
                self.pop();
                src.pos = src.pos.saturating_add(1).saturating_add(len).min(src.d.len());
            }
            _ => {}
        }
        Ok(())
    }

    /// `xshow` / `yshow` / `xyshow`: glyph displacements from a number array instead of the
    /// advances.
    fn explicit_show(&mut self, s: &[u8], disp: &[f64], n: &str) {
        let step = if n == "xyshow" { 2 } else { 1 };
        let d: Vec<(f64, f64)> = (0..s.len())
            .map(|k| {
                let i = k * step;
                let v = |j: usize| disp.get(j).copied().unwrap_or(0.0);
                match n {
                    "xshow" => (v(i), 0.0),
                    "yshow" => (0.0, v(i)),
                    _ => (v(i), v(i + 1)),
                }
            })
            .collect();
        let Some(fd) = self.gs.font.clone() else {
            self.b.skip("text (no font)");
            return;
        };
        let Some((font, m)) = self.font_of(&fd) else { return };
        self.show_with(s, move |k, c| {
            let adv = m * Point::new(font.width(c), 0.0) - m * Point::ZERO;
            let (dx, dy) = d.get(k).copied().unwrap_or((0.0, 0.0));
            (dx - adv.x, dy - adv.y)
        });
    }
}

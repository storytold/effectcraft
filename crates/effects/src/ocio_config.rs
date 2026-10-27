//! Custom OpenColorIO configuration files (`.ocio`) for the OCIO effects, read from the
//! published configuration format (a YAML document) and the OpenColorIO v2 documentation of its
//! transforms, not from OpenColorIO's source.
//!
//! Supported subset:
//!
//! * `search_path` (a string with `:`-separated entries or a list), resolved against the
//!   config's folder, for `FileTransform` sources;
//! * `roles` (any role name can stand for its colour space);
//! * `colorspaces` (scene-referred) and `display_colorspaces` (display-referred): `name`,
//!   `aliases`, `isdata`, and the transforms to / from their reference space —
//!   `to_reference` / `from_reference` (OCIO v1), `to_scene_reference` / `from_scene_reference`
//!   and `to_display_reference` / `from_display_reference` (v2); a space with only one direction
//!   is inverted for the other;
//! * the two reference spaces of v2: a conversion between a scene-referred and a
//!   display-referred space goes through the default view transform (`default_view_transform`,
//!   else the first scene-referred one in `view_transforms`), as OCIO documents;
//! * `displays`: display → views, either `!<View> {name, colorspace, looks}` or
//!   `!<View> {name, view_transform, display_colorspace, looks}` (`<USE_DISPLAY_NAME>` too);
//! * `looks` (`name`, `process_space`, `transform`, `inverse_transform`) and look strings
//!   (`+A, -B`, `A | B` = the first one the config has);
//! * transforms: `MatrixTransform` (`matrix` 4×4 row-major, `offset`), `FileTransform` (the LUT,
//!   CDL and matrix files `super::ocio::parse_file` reads; `interpolation`, `ccc_id`),
//!   `ExponentTransform` (`value`), `ExponentWithLinearTransform` (`gamma`, `offset`, `style:
//!   mirror`; the power curve with a linear toe that the Academy CLF specification calls
//!   "moncurve"), `LogTransform` (`base`), `LogAffineTransform` (`base`, `logSideSlope`,
//!   `logSideOffset`, `linSideSlope`, `linSideOffset`), `CDLTransform` (`slope`, `offset`,
//!   `power`, `sat`), `RangeTransform` (any of `min_in_value`, `max_in_value`,
//!   `min_out_value`, `max_out_value`; clamping unless `style: noClamp`), `AllocationTransform`
//!   (`allocation: uniform | lg2`, `vars`), `ColorSpaceTransform` (`src`, `dst`, `data_bypass`),
//!   `LookTransform` (`src`, `dst`, `looks`), `DisplayViewTransform` (`src`, `display`, `view`,
//!   `looks_bypass`, `data_bypass`), `BuiltinTransform` (the styles [`builtin`] lists) and
//!   `GroupTransform` (`children`), each with `direction: inverse`.
//!
//! Other transforms (`GradingPrimaryTransform`, `GradingToneTransform`, the ACES output
//! transforms…) pass colours through; [`Config::unsupported`] lists them for a config and
//! [`Xf::unsupported_list`] for one conversion, and the effects show that as a warning.
//!
//! The YAML reader covers what configs use: block mappings and sequences, flow `{}` / `[]`
//! collections (also spanning lines), `!<Tag>` tags, quoted scalars, `|` / `>` block scalars
//! and comments. Nesting deeper than [`MAX_YAML_DEPTH`] is cut off, and a conversion stops
//! resolving named transforms [`MAX_NAMED_DEPTH`] levels down or past [`MAX_OPS`] steps
//! (a config that refers to itself in a loop), so a hostile config can't exhaust the stack.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use super::ocio::{ACES_CG, ACES2065_1, Cdl, D65, FileXform, P3, REC709, REC2020, Space, Tf, load_file, matrix, sp};

/// Deepest YAML nesting read (configs use about ten levels).
pub const MAX_YAML_DEPTH: usize = 64;
/// Deepest chain of named transforms (a ColorSpaceTransform naming a space whose transform
/// names another…) one conversion resolves.
pub const MAX_NAMED_DEPTH: usize = 16;
/// Most steps one conversion has.
pub const MAX_OPS: usize = 4096;

// ---------------------------------------------------------------- YAML subset

/// A parsed YAML node.
#[derive(Clone, Debug, PartialEq)]
pub enum Yaml {
    Str(String),
    Seq(Vec<Yaml>),
    Map(Vec<(String, Yaml)>),
    /// A `!<Tag>` node.
    Tagged(String, Box<Yaml>),
    Null,
}

impl Yaml {
    pub fn get(&self, k: &str) -> Option<&Yaml> {
        match self {
            Yaml::Map(m) => m.iter().find(|(key, _)| key == k).map(|(_, v)| v),
            Yaml::Tagged(_, v) => v.get(k),
            _ => None,
        }
    }
    pub fn str(&self) -> Option<&str> {
        match self {
            Yaml::Str(s) => Some(s),
            Yaml::Tagged(_, v) => v.str(),
            _ => None,
        }
    }
    pub fn seq(&self) -> &[Yaml] {
        match self {
            Yaml::Seq(v) => v,
            Yaml::Tagged(_, v) => v.seq(),
            _ => &[],
        }
    }
    pub fn tag(&self) -> Option<&str> {
        match self {
            Yaml::Tagged(t, _) => Some(t),
            _ => None,
        }
    }
    fn f(&self) -> Option<f64> {
        self.str()?.trim().parse().ok()
    }
    fn nums(&self) -> Vec<f64> {
        match self {
            Yaml::Seq(v) => v.iter().filter_map(Yaml::f).collect(),
            Yaml::Str(_) => self.f().into_iter().collect(),
            Yaml::Tagged(_, v) => v.nums(),
            _ => vec![],
        }
    }
    /// A string value of key `k` (`""` when missing).
    fn text(&self, k: &str) -> &str {
        self.get(k).and_then(Yaml::str).unwrap_or("")
    }
    /// A boolean value of key `k` (`true` / `false`), `default` when missing.
    fn flag(&self, k: &str, default: bool) -> bool {
        match self.text(k).trim().to_ascii_lowercase().as_str() {
            "true" | "yes" => true,
            "false" | "no" => false,
            _ => default,
        }
    }
}

/// Strip a trailing comment (a `#` outside quotes, at the start or after whitespace).
fn strip_comment(line: &str) -> &str {
    let (mut sq, mut dq) = (false, false);
    let b = line.as_bytes();
    for (i, &c) in b.iter().enumerate() {
        match c {
            b'\'' if !dq => sq = !sq,
            b'"' if !sq => dq = !dq,
            b'#' if !sq && !dq && (i == 0 || b[i - 1].is_ascii_whitespace()) => return &line[..i],
            _ => {}
        }
    }
    line
}

fn unquote(s: &str) -> String {
    let t = s.trim();
    if t.len() >= 2 && ((t.starts_with('"') && t.ends_with('"')) || (t.starts_with('\'') && t.ends_with('\''))) {
        return t[1..t.len() - 1].replace("\\\"", "\"").replace("''", "'");
    }
    t.to_string()
}

/// Split `key: value` at the first `:` followed by a space or the end (outside quotes/brackets).
fn split_key(s: &str) -> Option<(String, &str)> {
    let b = s.as_bytes();
    let (mut sq, mut dq, mut depth) = (false, false, 0i32);
    for (i, &c) in b.iter().enumerate() {
        match c {
            b'\'' if !dq => sq = !sq,
            b'"' if !sq => dq = !dq,
            b'{' | b'[' if !sq && !dq => depth += 1,
            b'}' | b']' if !sq && !dq => depth -= 1,
            b':' if !sq && !dq && depth == 0 && (i + 1 == b.len() || b[i + 1] == b' ' || b[i + 1] == b'\t') => {
                let k = s[..i].trim();
                if k.is_empty() || k.starts_with('!') || k.starts_with('{') || k.starts_with('[') {
                    return None;
                }
                return Some((unquote(k), s[i + 1..].trim()));
            }
            _ => {}
        }
    }
    None
}

/// Bracket balance of a flow fragment (outside quotes).
fn balance(s: &str) -> i32 {
    let (mut sq, mut dq, mut d) = (false, false, 0i32);
    for c in s.chars() {
        match c {
            '\'' if !dq => sq = !sq,
            '"' if !sq => dq = !dq,
            '{' | '[' if !sq && !dq => d = d.saturating_add(1),
            '}' | ']' if !sq && !dq => d = d.saturating_sub(1),
            _ => {}
        }
    }
    d
}

struct Flow<'a> {
    s: &'a [u8],
    i: usize,
    depth: usize,
}

impl Flow<'_> {
    fn ws(&mut self) {
        while self.i < self.s.len() && (self.s[self.i] as char).is_whitespace() {
            self.i += 1;
        }
    }
    fn peek(&self) -> Option<u8> {
        self.s.get(self.i).copied()
    }
    /// A nested value (cut off past [`MAX_YAML_DEPTH`]: the rest of the text is skipped).
    fn nested(&mut self) -> Yaml {
        if self.depth >= MAX_YAML_DEPTH {
            self.i = self.s.len();
            return Yaml::Null;
        }
        self.depth += 1;
        let v = self.value();
        self.depth -= 1;
        v
    }
    fn value(&mut self) -> Yaml {
        self.ws();
        match self.peek() {
            Some(b'{') => {
                self.i += 1;
                let mut m = vec![];
                loop {
                    self.ws();
                    match self.peek() {
                        None => break,
                        Some(b'}') => {
                            self.i += 1;
                            break;
                        }
                        Some(b',') => {
                            self.i += 1;
                            continue;
                        }
                        _ => {}
                    }
                    let k = self.scalar(true);
                    self.ws();
                    if self.peek() == Some(b':') {
                        self.i += 1;
                    }
                    let v = self.nested();
                    m.push((unquote(&k), v));
                }
                Yaml::Map(m)
            }
            Some(b'[') => {
                self.i += 1;
                let mut v = vec![];
                loop {
                    self.ws();
                    match self.peek() {
                        None => break,
                        Some(b']') => {
                            self.i += 1;
                            break;
                        }
                        Some(b',') => {
                            self.i += 1;
                            continue;
                        }
                        _ => v.push(self.nested()),
                    }
                }
                Yaml::Seq(v)
            }
            Some(b'!') => {
                let start = self.i;
                while self.i < self.s.len() && !(self.s[self.i] as char).is_whitespace() && self.s[self.i] != b'{' && self.s[self.i] != b'[' {
                    self.i += 1;
                }
                let tag = tag_name(std::str::from_utf8(&self.s[start..self.i]).unwrap_or(""));
                self.ws();
                let inner = match self.peek() {
                    Some(b'{') | Some(b'[') => self.nested(),
                    _ => Yaml::Null,
                };
                Yaml::Tagged(tag, Box::new(inner))
            }
            _ => {
                let s = self.scalar(false);
                if s.is_empty() { Yaml::Null } else { Yaml::Str(unquote(&s)) }
            }
        }
    }
    /// A plain or quoted scalar, ending at `,` `}` `]` (and `:` + space for keys).
    fn scalar(&mut self, key: bool) -> String {
        self.ws();
        let start = self.i;
        if let Some(q @ (b'"' | b'\'')) = self.peek() {
            self.i += 1;
            while self.i < self.s.len() && self.s[self.i] != q {
                self.i += 1;
            }
            self.i = (self.i + 1).min(self.s.len());
            return String::from_utf8_lossy(&self.s[start..self.i]).into_owned();
        }
        while self.i < self.s.len() {
            let c = self.s[self.i];
            if c == b',' || c == b'}' || c == b']' {
                break;
            }
            if key && c == b':' {
                break;
            }
            self.i += 1;
        }
        String::from_utf8_lossy(&self.s[start..self.i]).trim().to_string()
    }
}

fn tag_name(t: &str) -> String {
    t.trim().trim_start_matches('!').trim_start_matches('<').trim_end_matches('>').to_string()
}

fn flow(s: &str, depth: usize) -> Yaml {
    Flow { s: s.as_bytes(), i: 0, depth }.value()
}

struct Lines {
    /// (indent, content without comment), blank lines removed.
    v: Vec<(usize, String)>,
}

impl Lines {
    fn new(text: &str) -> Lines {
        let v = text
            .lines()
            .filter(|l| !l.trim_start().starts_with('%') && l.trim() != "---" && l.trim() != "...")
            .map(|l| {
                let c = strip_comment(l).trim_end();
                (c.len() - c.trim_start().len(), c.trim_start().to_string())
            })
            .filter(|(_, c)| !c.is_empty())
            .collect();
        Lines { v }
    }

    /// The value of a `key:` / `- ` remainder `rest` whose children are indented more than
    /// `indent` (from line `*i`), `depth` levels down.
    fn value(&self, rest: &str, indent: usize, i: &mut usize, depth: usize) -> Yaml {
        let rest = rest.trim();
        if rest.is_empty() {
            return match self.v.get(*i) {
                // A sequence may sit at the key's own indent.
                Some((ind, c)) if *ind > indent || (*ind == indent && (c == "-" || c.starts_with("- "))) => self.block(*ind, i, depth + 1),
                _ => Yaml::Null,
            };
        }
        if rest == "|" || rest == ">" || rest.starts_with("|-") || rest.starts_with(">-") || rest.starts_with("|+") || rest.starts_with(">+") {
            let mut out = vec![];
            while let Some((ind, c)) = self.v.get(*i) {
                if *ind <= indent {
                    break;
                }
                out.push(c.clone());
                *i += 1;
            }
            return Yaml::Str(out.join(if rest.starts_with('>') { " " } else { "\n" }));
        }
        if rest.starts_with('!') && !rest.contains('{') && !rest.contains('[') {
            let tag = tag_name(rest);
            let inner = match self.v.get(*i) {
                Some((ind, _)) if *ind > indent => self.block(*ind, i, depth + 1),
                _ => Yaml::Null,
            };
            return Yaml::Tagged(tag, Box::new(inner));
        }
        if rest.starts_with('{') || rest.starts_with('[') || (rest.starts_with('!') && (rest.contains('{') || rest.contains('['))) {
            // Flow collections may continue on the next lines.
            let mut text = rest.to_string();
            while balance(&text) > 0
                && let Some((_, next)) = self.v.get(*i)
            {
                text.push(' ');
                text.push_str(next);
                *i += 1;
            }
            return flow(&text, depth);
        }
        Yaml::Str(unquote(rest))
    }

    /// A block node whose lines start at `indent`, `depth` levels down (cut off past
    /// [`MAX_YAML_DEPTH`]: the rest of the document is skipped).
    fn block(&self, indent: usize, i: &mut usize, depth: usize) -> Yaml {
        if depth >= MAX_YAML_DEPTH {
            *i = self.v.len();
            return Yaml::Null;
        }
        let Some((_, first)) = self.v.get(*i) else { return Yaml::Null };
        if first == "-" || first.starts_with("- ") {
            let mut items = vec![];
            while let Some((ind, c)) = self.v.get(*i) {
                if *ind != indent || !(c == "-" || c.starts_with("- ")) {
                    break;
                }
                *i += 1;
                let rest = c[1..].trim_start().to_string();
                // `- key: value` opens a mapping whose keys are indented to the key.
                if split_key(&rest).is_some() && !rest.starts_with('{') && !rest.starts_with('!') {
                    let inner = indent + 1 + (c.len() - 1 - c[1..].trim_start().len());
                    items.push(self.map_from(Some(rest), inner.max(indent + 2), i, depth + 1));
                } else {
                    items.push(self.value(&rest, indent, i, depth));
                }
            }
            return Yaml::Seq(items);
        }
        if split_key(first).is_some() {
            return self.map_from(None, indent, i, depth);
        }
        *i += 1;
        Yaml::Str(unquote(first))
    }

    /// A block mapping at `indent`; `first` is an entry already taken from a `- ` line.
    fn map_from(&self, first: Option<String>, indent: usize, i: &mut usize, depth: usize) -> Yaml {
        let mut m = vec![];
        if let Some(f) = first
            && let Some((k, rest)) = split_key(&f)
        {
            let v = self.value(rest, indent, i, depth);
            m.push((k, v));
        }
        while let Some((ind, c)) = self.v.get(*i) {
            if *ind != indent {
                if *ind < indent {
                    break;
                }
                // Over-indented stray line: skip.
                *i += 1;
                continue;
            }
            let Some((k, rest)) = split_key(c) else { break };
            *i += 1;
            let v = self.value(rest, indent, i, depth);
            m.push((k, v));
        }
        Yaml::Map(m)
    }
}

/// Parse a YAML document (the subset described in the module docs).
pub fn parse_yaml(text: &str) -> Yaml {
    let l = Lines::new(text);
    let mut i = 0;
    let indent = l.v.first().map_or(0, |(ind, _)| *ind);
    l.block(indent, &mut i, 0)
}

// ---------------------------------------------------------------- transforms

/// A colour transform of a config. As in OCIO, the forward direction of ExponentTransform,
/// ExponentWithLinearTransform and [`Xf::Curve`] decodes (encoded → linear).
#[derive(Clone, Debug)]
pub enum Xf {
    /// 4×4 row-major matrix (RGB of the first three rows/columns) plus offset.
    Matrix {
        m: [f64; 16],
        offset: [f64; 4],
    },
    File {
        src: String,
        xf: Option<Arc<FileXform>>,
        interp: u32,
        ccc: String,
    },
    Exponent([f64; 3]),
    /// ExponentWithLinearTransform ([`moncurve`]); `mirror` = `style: mirror` (else negative
    /// values continue the linear segment).
    ExponentLinear {
        gamma: [f64; 3],
        offset: [f64; 3],
        mirror: bool,
    },
    Log {
        base: f64,
    },
    LogAffine {
        base: f64,
        log_slope: [f64; 3],
        log_offset: [f64; 3],
        lin_slope: [f64; 3],
        lin_offset: [f64; 3],
    },
    Cdl(Cdl),
    Range(RangeXf),
    /// A transfer function of the built-in configuration (BuiltinTransform), forward = decode.
    Curve(Tf),
    Group(Vec<Xf>),
    Inverse(Box<Xf>),
    /// A transform that names other parts of the config, replaced by their transforms when a
    /// conversion is built ([`Config::path`]).
    Named(Box<Named>),
    /// An unsupported transform (by tag): identity.
    Unsupported(String),
}

/// A transform naming parts of the config (see [`Xf::Named`]).
#[derive(Clone, Debug)]
pub enum Named {
    /// ColorSpaceTransform: `src` → `dst`.
    Space { src: String, dst: String, data_bypass: bool },
    /// LookTransform: `src` → the looks → `dst`.
    Look { src: String, dst: String, looks: String },
    /// DisplayViewTransform: `src` through a display's view.
    DisplayView { src: String, display: String, view: String, looks_bypass: bool, data_bypass: bool },
}

/// RangeTransform: each bound pair is optional (`min_in` with `min_out`, `max_in` with
/// `max_out`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RangeXf {
    pub min_in: Option<f64>,
    pub max_in: Option<f64>,
    pub min_out: Option<f64>,
    pub max_out: Option<f64>,
    /// Clamp to the output range (`style: Clamp`, the default).
    pub clamp: bool,
}

impl RangeXf {
    /// (scale, offset, low, high) of the forward (or inverse) range: `v · scale + offset`
    /// clamped to `low..=high` (±∞ = unbounded). Both pairs map the input range onto the
    /// output range; one pair only shifts (and clamps on that side).
    pub fn parts(&self, inverse: bool) -> (f64, f64, f64, f64) {
        let (lo_in, hi_in, lo_out, hi_out) =
            if inverse { (self.min_out, self.max_out, self.min_in, self.max_in) } else { (self.min_in, self.max_in, self.min_out, self.max_out) };
        let lo = lo_in.and(lo_out);
        let hi = hi_in.and(hi_out);
        let (k, off) = match (lo_in, hi_in, lo_out, hi_out) {
            (Some(a0), Some(a1), Some(b0), Some(b1)) if (a1 - a0).abs() > 1e-12 => {
                let k = (b1 - b0) / (a1 - a0);
                (k, b0 - a0 * k)
            }
            (Some(a0), _, Some(b0), _) => (1.0, b0 - a0),
            (_, Some(a1), _, Some(b1)) => (1.0, b1 - a1),
            _ => (1.0, 0.0),
        };
        let bound = |v: Option<f64>, unbounded: f64| v.filter(|_| self.clamp).filter(|v| v.is_finite()).unwrap_or(unbounded);
        (k, off, bound(lo, f64::NEG_INFINITY), bound(hi, f64::INFINITY))
    }

    fn apply(&self, c: [f64; 3], inverse: bool) -> [f64; 3] {
        let (k, off, lo, hi) = self.parts(inverse);
        // Comparisons, not `clamp`: never panics on a reversed range and keeps NaN.
        c.map(|v| {
            let v = v * k + off;
            if v < lo {
                lo
            } else if v > hi {
                hi
            } else {
                v
            }
        })
    }
}

fn vec3_of(y: Option<&Yaml>, d: f64) -> [f64; 3] {
    let v = y.map(Yaml::nums).unwrap_or_default();
    match v.as_slice() {
        [] => [d; 3],
        [a] | [a, _] => [*a; 3],
        [a, b, c, ..] => [*a, *b, *c],
    }
}

/// A matrix-and-offset transform (MatrixTransform, `.spimtx`) applied forward or inverse.
pub fn matrix_apply(m: &[f64; 16], off: &[f64; 4], c: [f64; 3], inverse: bool) -> [f64; 3] {
    if !inverse {
        return [0, 1, 2].map(|r| m[r * 4] * c[0] + m[r * 4 + 1] * c[1] + m[r * 4 + 2] * c[2] + off[r]);
    }
    match mat3_inverse(m) {
        Some(inv) => effectcraft_color::space::mul_vec(&inv, [c[0] - off[0], c[1] - off[1], c[2] - off[2]]),
        None => c,
    }
}

pub(crate) fn mat3_inverse(m: &[f64; 16]) -> Option<[[f64; 3]; 3]> {
    let a = [[m[0], m[1], m[2]], [m[4], m[5], m[6]], [m[8], m[9], m[10]]];
    let det = a[0][0] * (a[1][1] * a[2][2] - a[1][2] * a[2][1]) - a[0][1] * (a[1][0] * a[2][2] - a[1][2] * a[2][0])
        + a[0][2] * (a[1][0] * a[2][1] - a[1][1] * a[2][0]);
    (det.abs() > 1e-12).then(|| effectcraft_color::space::invert(&a))
}

fn log_base(v: f64, base: f64) -> f64 {
    v.max(f64::MIN_POSITIVE).ln() / base.ln()
}

/// ExponentWithLinearTransform (the Academy CLF "moncurve"): forward decodes, `((v + offset) /
/// (1 + offset))^gamma` above the break point `offset / (gamma − 1)` and a straight line through
/// 0 that meets it below; inverse encodes. `mirror` mirrors negative values instead of
/// continuing the line. A gamma of at most 1 or a non-positive offset is a plain power curve.
pub fn moncurve(v: f64, gamma: f64, offset: f64, inverse: bool, mirror: bool) -> f64 {
    if mirror && v < 0.0 {
        return -moncurve(-v, gamma, offset, inverse, false);
    }
    if gamma <= 1.0 || offset <= 0.0 {
        let p = if inverse { 1.0 / gamma.max(1e-9) } else { gamma };
        return v.max(0.0).powf(p);
    }
    let x_break = offset / (gamma - 1.0);
    let y_break = ((x_break + offset) / (1.0 + offset)).powf(gamma);
    // Slope of the linear segment (y = x · slope).
    let slope = y_break / x_break;
    if !inverse {
        if v >= x_break { ((v + offset) / (1.0 + offset)).powf(gamma) } else { v * slope }
    } else if v >= y_break {
        (1.0 + offset) * v.powf(1.0 / gamma) - offset
    } else {
        v / slope
    }
}

/// A 3×3 matrix as a MatrixTransform.
fn m3(m: [[f64; 3]; 3]) -> Xf {
    Xf::Matrix { m: [m[0][0], m[0][1], m[0][2], 0.0, m[1][0], m[1][1], m[1][2], 0.0, m[2][0], m[2][1], m[2][2], 0.0, 0.0, 0.0, 0.0, 1.0], offset: [0.0; 4] }
}

/// The sRGB curve as OCIO defines it: ExponentWithLinearTransform with gamma 2.4, offset 0.055.
const SRGB_CURVE: Xf = Xf::ExponentLinear { gamma: [2.4; 3], offset: [0.055; 3], mirror: false };

/// A BuiltinTransform by its documented `style` (case-insensitive), built from the published
/// primaries and curves (Bradford adaptation where the name says `BFD`; PQ with 1.0 = 100 nits).
/// `None` for the styles not implemented (the ACES output transforms and gamut compression,
/// camera log curves, HLG).
pub fn builtin(style: &str) -> Option<Xf> {
    let xyz = Space { name: "XYZ", prims: None, tf: Tf::Linear, raw: false };
    let lin = |prims| sp("linear", prims, D65, Tf::Linear);
    // CIE XYZ D65 → a display's linear RGB, then its encoding (inverse = encode).
    let display = |prims, curve: Xf| Some(Xf::Group(vec![m3(matrix(&xyz, &lin(prims))), Xf::Inverse(Box::new(curve))]));
    let gamma = |g: f64| Xf::Exponent([g; 3]);
    let s = style.trim().to_ascii_uppercase();
    match s.as_str() {
        "IDENTITY" => Some(Xf::Group(vec![])),
        "UTILITY - ACES-AP0_TO_CIE-XYZ-D65_BFD" => Some(m3(ACES2065_1.to_ref(true))),
        "UTILITY - ACES-AP1_TO_CIE-XYZ-D65_BFD" => Some(m3(ACES_CG.to_ref(true))),
        "UTILITY - ACES-AP1_TO_LINEAR-REC709_BFD" => Some(m3(matrix(&ACES_CG, &lin(REC709)))),
        "ACESCCT_TO_ACES2065-1" => Some(Xf::Group(vec![Xf::Curve(Tf::AcesCct), m3(matrix(&ACES_CG, &ACES2065_1))])),
        "ACESCC_TO_ACES2065-1" => Some(Xf::Group(vec![Xf::Curve(Tf::AcesCc), m3(matrix(&ACES_CG, &ACES2065_1))])),
        "DISPLAY - CIE-XYZ-D65_TO_SRGB" => display(REC709, SRGB_CURVE),
        "DISPLAY - CIE-XYZ-D65_TO_DISPLAYP3" => display(P3, SRGB_CURVE),
        "DISPLAY - CIE-XYZ-D65_TO_REC.1886-REC.709" => display(REC709, gamma(2.4)),
        "DISPLAY - CIE-XYZ-D65_TO_REC.1886-REC.2020" => display(REC2020, gamma(2.4)),
        "DISPLAY - CIE-XYZ-D65_TO_G2.2-REC.709" => display(REC709, gamma(2.2)),
        "DISPLAY - CIE-XYZ-D65_TO_G2.6-P3-D65" => display(P3, gamma(2.6)),
        "DISPLAY - CIE-XYZ-D65_TO_REC.2100-PQ" => display(REC2020, Xf::Curve(Tf::Pq)),
        "DISPLAY - CIE-XYZ-D65_TO_ST2084-P3-D65" => display(P3, Xf::Curve(Tf::Pq)),
        _ => None,
    }
}

impl Xf {
    fn from_yaml(y: &Yaml, dir: &Path, search: &[String]) -> Xf {
        let tag = y.tag().unwrap_or("");
        let body = match y {
            Yaml::Tagged(_, b) => b.as_ref(),
            other => other,
        };
        let inverse = body.text("direction").eq_ignore_ascii_case("inverse");
        let xf = match tag {
            "MatrixTransform" => {
                let v = body.get("matrix").map(Yaml::nums).unwrap_or_default();
                let mut m = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0];
                if v.len() == 16 {
                    m.copy_from_slice(&v);
                } else if v.len() == 9 {
                    for r in 0..3 {
                        for c in 0..3 {
                            m[r * 4 + c] = v[r * 3 + c];
                        }
                    }
                }
                let o = body.get("offset").map(Yaml::nums).unwrap_or_default();
                let mut offset = [0.0; 4];
                for (k, v) in o.iter().take(4).enumerate() {
                    offset[k] = *v;
                }
                Xf::Matrix { m, offset }
            }
            "FileTransform" => {
                let src = body.text("src").to_string();
                let interp = match body.get("interpolation").and_then(Yaml::str).unwrap_or("linear").to_ascii_lowercase().as_str() {
                    "nearest" => 0,
                    "tetrahedral" | "best" => 2,
                    _ => 1,
                };
                let ccc = body.get("cccid").or_else(|| body.get("ccc_id")).and_then(Yaml::str).unwrap_or("").to_string();
                let xf = resolve(&src, dir, search).and_then(|p| load_file(&p.to_string_lossy()));
                Xf::File { src, xf, interp, ccc }
            }
            "ExponentTransform" => Xf::Exponent(vec3_of(body.get("value"), 1.0)),
            "ExponentWithLinearTransform" => Xf::ExponentLinear {
                gamma: vec3_of(body.get("gamma"), 1.0),
                offset: vec3_of(body.get("offset"), 0.0),
                mirror: body.text("style").eq_ignore_ascii_case("mirror"),
            },
            "LogTransform" => Xf::Log { base: body.get("base").and_then(Yaml::f).unwrap_or(2.0) },
            "LogAffineTransform" => Xf::LogAffine {
                base: body.get("base").and_then(Yaml::f).unwrap_or(2.0),
                log_slope: vec3_of(body.get("logSideSlope"), 1.0),
                log_offset: vec3_of(body.get("logSideOffset"), 0.0),
                lin_slope: vec3_of(body.get("linSideSlope"), 1.0),
                lin_offset: vec3_of(body.get("linSideOffset"), 0.0),
            },
            "CDLTransform" => Xf::Cdl(Cdl {
                slope: vec3_of(body.get("slope"), 1.0),
                offset: vec3_of(body.get("offset"), 0.0),
                power: vec3_of(body.get("power"), 1.0),
                sat: body.get("sat").and_then(Yaml::f).unwrap_or(1.0),
            }),
            "RangeTransform" => Xf::Range(RangeXf {
                min_in: body.get("min_in_value").and_then(Yaml::f),
                max_in: body.get("max_in_value").and_then(Yaml::f),
                min_out: body.get("min_out_value").and_then(Yaml::f),
                max_out: body.get("max_out_value").and_then(Yaml::f),
                clamp: !body.text("style").eq_ignore_ascii_case("noclamp"),
            }),
            // Uniform: the `vars` range onto 0..1; lg2: its log₂ (after adding the optional
            // third var) onto 0..1.
            "AllocationTransform" => {
                let lg2 = body.text("allocation").eq_ignore_ascii_case("lg2");
                let vars = body.get("vars").map(Yaml::nums).unwrap_or_default();
                let (lo, hi) = match vars.as_slice() {
                    [lo, hi, ..] => (*lo, *hi),
                    _ if lg2 => (-15.0, 6.0),
                    _ => (0.0, 1.0),
                };
                let span = hi - lo;
                if !span.is_finite() || span.abs() < 1e-12 {
                    Xf::Unsupported("AllocationTransform (empty range)".into())
                } else if lg2 {
                    let off = vars.get(2).copied().unwrap_or(0.0);
                    Xf::LogAffine { base: 2.0, log_slope: [1.0 / span; 3], log_offset: [-lo / span; 3], lin_slope: [1.0; 3], lin_offset: [off; 3] }
                } else {
                    Xf::Range(RangeXf { min_in: Some(lo), max_in: Some(hi), min_out: Some(0.0), max_out: Some(1.0), clamp: false })
                }
            }
            "BuiltinTransform" => {
                let style = body.text("style");
                builtin(style).unwrap_or_else(|| Xf::Unsupported(format!("BuiltinTransform `{style}`")))
            }
            "ColorSpaceTransform" => Xf::Named(Box::new(Named::Space {
                src: body.text("src").to_string(),
                dst: body.text("dst").to_string(),
                data_bypass: body.flag("data_bypass", true),
            })),
            "LookTransform" => {
                Xf::Named(Box::new(Named::Look { src: body.text("src").to_string(), dst: body.text("dst").to_string(), looks: body.text("looks").to_string() }))
            }
            "DisplayViewTransform" => Xf::Named(Box::new(Named::DisplayView {
                src: body.text("src").to_string(),
                display: body.text("display").to_string(),
                view: body.text("view").to_string(),
                looks_bypass: body.flag("looks_bypass", false),
                data_bypass: body.flag("data_bypass", true),
            })),
            "GroupTransform" => Xf::Group(body.get("children").map(|c| c.seq().iter().map(|t| Xf::from_yaml(t, dir, search)).collect()).unwrap_or_default()),
            other => Xf::Unsupported(other.to_string()),
        };
        if inverse { Xf::Inverse(Box::new(xf)) } else { xf }
    }

    /// Apply forward (or inverse) to linear-or-encoded RGB. Named transforms pass through:
    /// apply the conversions [`Config::path`] builds, where they are resolved.
    pub fn apply(&self, c: [f64; 3], inverse: bool) -> [f64; 3] {
        match self {
            Xf::Inverse(x) => x.apply(c, !inverse),
            Xf::Matrix { m, offset } => matrix_apply(m, offset, c, inverse),
            Xf::File { xf, interp, ccc, .. } => match xf {
                Some(x) => x.apply(c.map(|v| v as f32), *interp, inverse, ccc).map(|v| v as f64),
                None => c,
            },
            Xf::Exponent(e) => [0, 1, 2].map(|k| {
                let p = if inverse { 1.0 / e[k].max(1e-9) } else { e[k] };
                c[k].max(0.0).powf(p)
            }),
            Xf::ExponentLinear { gamma, offset, mirror } => [0, 1, 2].map(|k| moncurve(c[k], gamma[k], offset[k], inverse, *mirror)),
            Xf::Log { base } => {
                if inverse {
                    c.map(|v| base.powf(v))
                } else {
                    c.map(|v| log_base(v, *base))
                }
            }
            Xf::LogAffine { base, log_slope, log_offset, lin_slope, lin_offset } => [0, 1, 2].map(|k| {
                if inverse {
                    (base.powf((c[k] - log_offset[k]) / log_slope[k]) - lin_offset[k]) / lin_slope[k]
                } else {
                    log_slope[k] * log_base(lin_slope[k] * c[k] + lin_offset[k], *base) + log_offset[k]
                }
            }),
            Xf::Cdl(cdl) => {
                if inverse {
                    cdl.invert(c, true)
                } else {
                    cdl.apply(c, true)
                }
            }
            Xf::Range(r) => r.apply(c, inverse),
            Xf::Curve(tf) => {
                if inverse {
                    c.map(|v| tf.encode(v))
                } else {
                    c.map(|v| tf.decode(v))
                }
            }
            Xf::Group(list) => {
                let mut v = c;
                if inverse {
                    for x in list.iter().rev() {
                        v = x.apply(v, true);
                    }
                } else {
                    for x in list {
                        v = x.apply(v, false);
                    }
                }
                v
            }
            Xf::Named(_) | Xf::Unsupported(_) => c,
        }
    }

    fn unsupported(&self, out: &mut Vec<String>) {
        match self {
            Xf::Unsupported(t) => out.push(t.clone()),
            Xf::Inverse(x) => x.unsupported(out),
            Xf::Group(v) => v.iter().for_each(|x| x.unsupported(out)),
            Xf::File { src, xf: None, .. } => out.push(format!("FileTransform (missing `{src}`)")),
            _ => {}
        }
    }

    /// The transforms of this one that can't be applied (and pass colours through), sorted and
    /// without repeats: for warnings.
    pub fn unsupported_list(&self) -> Vec<String> {
        let mut out = vec![];
        self.unsupported(&mut out);
        out.sort();
        out.dedup();
        out
    }
}

/// Find a FileTransform source on the search path (relative to the config's folder).
fn resolve(src: &str, dir: &Path, search: &[String]) -> Option<PathBuf> {
    if src.trim().is_empty() {
        return None;
    }
    let p = Path::new(src);
    if p.is_absolute() {
        return p.exists().then(|| p.to_path_buf());
    }
    let mut dirs: Vec<PathBuf> = search.iter().map(|s| dir.join(s)).collect();
    dirs.push(dir.to_path_buf());
    dirs.into_iter().map(|d| d.join(src)).find(|c| c.exists())
}

// ---------------------------------------------------------------- the config

/// A colour space of a config.
#[derive(Clone, Debug)]
pub struct ConfigSpace {
    pub name: String,
    pub aliases: Vec<String>,
    pub family: String,
    pub is_data: bool,
    /// Display-referred (`display_colorspaces`): its reference is the display reference space.
    pub display: bool,
    pub to_ref: Option<Xf>,
    pub from_ref: Option<Xf>,
}

/// A view of a display: a colour space (OCIO v1 style), or a view transform and a display
/// colour space (v2); either with looks.
#[derive(Clone, Debug, Default)]
pub struct View {
    pub name: String,
    pub colorspace: String,
    pub view_transform: String,
    pub display_colorspace: String,
    pub looks: String,
}

/// A view transform: scene reference → display reference (or display → display).
#[derive(Clone, Debug)]
pub struct ViewTransform {
    pub name: String,
    pub from_scene: Option<Xf>,
    pub to_scene: Option<Xf>,
    pub from_display: Option<Xf>,
    pub to_display: Option<Xf>,
}

impl ViewTransform {
    fn scene(&self) -> bool {
        self.from_scene.is_some() || self.to_scene.is_some()
    }
}

/// A look: a transform applied in its process space.
#[derive(Clone, Debug)]
pub struct Look {
    pub name: String,
    pub process_space: String,
    pub transform: Option<Xf>,
    pub inverse_transform: Option<Xf>,
}

/// A parsed `.ocio` configuration.
#[derive(Clone, Debug, Default)]
pub struct Config {
    pub name: String,
    pub search_path: Vec<String>,
    pub roles: Vec<(String, String)>,
    pub spaces: Vec<ConfigSpace>,
    /// Display → views.
    pub displays: Vec<(String, Vec<View>)>,
    pub view_transforms: Vec<ViewTransform>,
    pub default_view_transform: String,
    pub looks: Vec<Look>,
}

impl Config {
    /// Parse config text; FileTransform sources resolve against `dir`.
    pub fn parse(text: &str, dir: &Path) -> Result<Config, String> {
        let y = parse_yaml(text);
        if !matches!(y, Yaml::Map(_)) {
            return Err("not an OCIO config (expected a YAML mapping)".into());
        }
        let search_path: Vec<String> = match y.get("search_path") {
            Some(Yaml::Seq(v)) => v.iter().filter_map(|s| s.str().map(str::to_string)).collect(),
            Some(s) => s.str().map(|s| s.split(':').map(|p| p.trim().to_string()).filter(|p| !p.is_empty()).collect()).unwrap_or_default(),
            None => vec![],
        };
        let roles = match y.get("roles") {
            Some(Yaml::Map(m)) => m.iter().filter_map(|(k, v)| v.str().map(|s| (k.clone(), s.to_string()))).collect(),
            _ => vec![],
        };
        // The first of `keys` an entry has, as a transform.
        let xf = |entry: &Yaml, keys: &[&str]| {
            keys.iter().find_map(|k| entry.get(k)).filter(|v| !matches!(v, Yaml::Null)).map(|v| Xf::from_yaml(v, dir, &search_path))
        };
        let mut spaces = vec![];
        for (list, display) in [("colorspaces", false), ("display_colorspaces", true)] {
            for cs in y.get(list).map(Yaml::seq).unwrap_or(&[]) {
                let Some(name) = cs.get("name").and_then(Yaml::str) else { continue };
                spaces.push(ConfigSpace {
                    name: name.to_string(),
                    aliases: cs.get("aliases").map(|a| a.seq().iter().filter_map(|s| s.str().map(str::to_string)).collect()).unwrap_or_default(),
                    family: cs.text("family").to_string(),
                    is_data: cs.flag("isdata", false),
                    display,
                    to_ref: xf(cs, &["to_reference", "to_scene_reference", "to_display_reference"]),
                    from_ref: xf(cs, &["from_reference", "from_scene_reference", "from_display_reference"]),
                });
            }
        }
        if spaces.is_empty() {
            return Err("the config has no colorspaces".into());
        }
        let displays = match y.get("displays") {
            Some(Yaml::Map(m)) => m
                .iter()
                .map(|(d, views)| {
                    let v = views
                        .seq()
                        .iter()
                        .filter(|v| !v.text("name").is_empty())
                        .map(|v| View {
                            name: v.text("name").to_string(),
                            colorspace: v.text("colorspace").to_string(),
                            view_transform: v.text("view_transform").to_string(),
                            display_colorspace: v.text("display_colorspace").to_string(),
                            looks: v.text("looks").to_string(),
                        })
                        .collect();
                    (d.clone(), v)
                })
                .collect(),
            _ => vec![],
        };
        let view_transforms = y
            .get("view_transforms")
            .map(Yaml::seq)
            .unwrap_or(&[])
            .iter()
            .filter(|v| !v.text("name").is_empty())
            .map(|v| ViewTransform {
                name: v.text("name").to_string(),
                from_scene: xf(v, &["from_scene_reference", "from_reference"]),
                to_scene: xf(v, &["to_scene_reference", "to_reference"]),
                from_display: xf(v, &["from_display_reference"]),
                to_display: xf(v, &["to_display_reference"]),
            })
            .collect();
        let looks = y
            .get("looks")
            .map(Yaml::seq)
            .unwrap_or(&[])
            .iter()
            .filter(|l| !l.text("name").is_empty())
            .map(|l| Look {
                name: l.text("name").to_string(),
                process_space: l.text("process_space").to_string(),
                transform: xf(l, &["transform"]),
                inverse_transform: xf(l, &["inverse_transform"]),
            })
            .collect();
        Ok(Config {
            name: y.text("name").to_string(),
            search_path,
            roles,
            spaces,
            displays,
            view_transforms,
            default_view_transform: y.text("default_view_transform").to_string(),
            looks,
        })
    }

    pub fn space_names(&self) -> Vec<&str> {
        self.spaces.iter().map(|s| s.name.as_str()).collect()
    }

    /// A colour space by name, alias or role (case-insensitive).
    pub fn space(&self, name: &str) -> Option<&ConfigSpace> {
        let n = name.trim();
        let by = |n: &str| self.spaces.iter().find(|s| s.name.eq_ignore_ascii_case(n) || s.aliases.iter().any(|a| a.eq_ignore_ascii_case(n)));
        by(n).or_else(|| self.roles.iter().find(|(r, _)| r.eq_ignore_ascii_case(n)).and_then(|(_, s)| by(s)))
    }

    /// A display's view (case-insensitive): the display named (else the first) and its view
    /// named (else its first).
    pub fn view(&self, display: &str, view: &str) -> Option<(&str, &View)> {
        let (d, views) = self.displays.iter().find(|(d, _)| d.eq_ignore_ascii_case(display.trim())).or(self.displays.first())?;
        let v = views.iter().find(|v| v.name.eq_ignore_ascii_case(view.trim())).or(views.first())?;
        Some((d.as_str(), v))
    }

    fn look(&self, name: &str) -> Option<&Look> {
        self.looks.iter().find(|l| l.name.eq_ignore_ascii_case(name.trim()))
    }

    fn view_transform(&self, name: &str) -> Option<&ViewTransform> {
        self.view_transforms.iter().find(|v| v.name.eq_ignore_ascii_case(name.trim()))
    }

    /// The view transform between the scene and display references: `default_view_transform`,
    /// else the first scene-referred one.
    fn default_view_transform(&self) -> Option<&ViewTransform> {
        self.view_transform(&self.default_view_transform).filter(|v| v.scene()).or_else(|| self.view_transforms.iter().find(|v| v.scene()))
    }

    /// The conversion from space `a` to space `b` as one flat group of steps (named transforms
    /// resolved; data spaces pass through).
    pub fn path(&self, a: &ConfigSpace, b: &ConfigSpace) -> Xf {
        let mut p = Build::new(self);
        p.path(a, b, 0);
        p.done()
    }

    /// Space `input` shown on `display` through `view` ([`Config::view`]): its looks, then the
    /// view's colour space, or its view transform and display colour space.
    pub fn display_path<'a>(&'a self, input: &'a ConfigSpace, display: &str, view: &str) -> Xf {
        let mut p = Build::new(self);
        p.display_view(input, display, view, false, true, 0);
        p.done()
    }

    /// Convert one colour from space `a` to space `b` (through the reference space; data
    /// spaces pass through). For many colours, build the [`Config::path`] once.
    pub fn convert(&self, a: &ConfigSpace, b: &ConfigSpace, c: [f64; 3]) -> [f64; 3] {
        self.path(a, b).apply(c, false)
    }

    /// Transforms this module can't apply anywhere in the config (by tag), for warnings.
    pub fn unsupported(&self) -> Vec<String> {
        let mut out = vec![];
        let spaces = self.spaces.iter().flat_map(|s| [&s.to_ref, &s.from_ref]);
        let views = self.view_transforms.iter().flat_map(|v| [&v.from_scene, &v.to_scene, &v.from_display, &v.to_display]);
        let looks = self.looks.iter().flat_map(|l| [&l.transform, &l.inverse_transform]);
        for x in spaces.chain(views).chain(looks).flatten() {
            x.unsupported(&mut out);
        }
        out.sort();
        out.dedup();
        out
    }
}

/// Builds one conversion of a config as a flat list of steps, resolving named transforms
/// (limited: [`MAX_NAMED_DEPTH`] levels, [`MAX_OPS`] steps and as many resolutions).
struct Build<'c> {
    cfg: &'c Config,
    out: Vec<Xf>,
    /// Named transforms still allowed to resolve.
    budget: usize,
    /// Set when a limit cut the conversion short.
    full: bool,
}

impl<'c> Build<'c> {
    fn new(cfg: &'c Config) -> Self {
        Build { cfg, out: vec![], budget: MAX_OPS, full: false }
    }

    fn done(mut self) -> Xf {
        if self.full {
            self.out.push(Xf::Unsupported(format!("the rest of a conversion of more than {MAX_OPS} steps")));
        }
        Xf::Group(self.out)
    }

    fn push(&mut self, x: Xf) {
        if self.out.len() < MAX_OPS {
            self.out.push(x);
        } else {
            self.full = true;
        }
    }

    /// Transform `x` forward, or inverse (its steps reversed and each inverted).
    fn xf(&mut self, x: &Xf, inverse: bool, depth: usize) {
        if self.full {
            return;
        }
        let start = self.out.len();
        match x {
            Xf::Group(list) => list.iter().for_each(|x| self.xf(x, false, depth)),
            Xf::Inverse(x) => self.xf(x, true, depth),
            Xf::Named(n) => self.named(n, depth),
            leaf => self.push(leaf.clone()),
        }
        if inverse && let Some(steps) = self.out.get_mut(start..) {
            steps.reverse();
            for s in steps.iter_mut() {
                *s = match std::mem::replace(s, Xf::Group(vec![])) {
                    Xf::Inverse(x) => *x,
                    x @ Xf::Unsupported(_) => x,
                    x => Xf::Inverse(Box::new(x)),
                };
            }
        }
    }

    fn space(&mut self, name: &str, what: &str) -> Option<&'c ConfigSpace> {
        let s = self.cfg.space(name);
        if s.is_none() {
            self.push(Xf::Unsupported(format!("{what} (no color space `{name}`)")));
        }
        s
    }

    fn named(&mut self, n: &Named, depth: usize) {
        if depth >= MAX_NAMED_DEPTH {
            self.push(Xf::Unsupported(format!("transforms nested more than {MAX_NAMED_DEPTH} deep")));
            return;
        }
        let Some(budget) = self.budget.checked_sub(1) else {
            self.full = true;
            return;
        };
        self.budget = budget;
        let depth = depth + 1;
        match n {
            Named::Space { src, dst, data_bypass } => {
                let (Some(a), Some(b)) = (self.space(src, "ColorSpaceTransform"), self.space(dst, "ColorSpaceTransform")) else { return };
                if *data_bypass {
                    self.path(a, b, depth);
                } else {
                    self.convert(a, b, depth);
                }
            }
            Named::Look { src, dst, looks } => {
                let (Some(a), Some(b)) = (self.space(src, "LookTransform"), self.space(dst, "LookTransform")) else { return };
                let cur = self.looks(a, looks, depth);
                self.path(cur, b, depth);
            }
            Named::DisplayView { src, display, view, looks_bypass, data_bypass } => {
                let Some(a) = self.space(src, "DisplayViewTransform") else { return };
                self.display_view(a, display, view, *looks_bypass, *data_bypass, depth);
            }
        }
    }

    /// Space `s` → its reference space.
    fn space_to_ref(&mut self, s: &ConfigSpace, depth: usize) {
        match (&s.to_ref, &s.from_ref) {
            (Some(x), _) => self.xf(x, false, depth),
            (None, Some(x)) => self.xf(x, true, depth),
            _ => {}
        }
    }

    /// Its reference space → space `s`.
    fn ref_to_space(&mut self, s: &ConfigSpace, depth: usize) {
        match (&s.from_ref, &s.to_ref) {
            (Some(x), _) => self.xf(x, false, depth),
            (None, Some(x)) => self.xf(x, true, depth),
            _ => {}
        }
    }

    /// Between the reference spaces: scene → display through the default view transform, or
    /// back (nothing when they are the same).
    fn bridge(&mut self, from_display: bool, to_display: bool, depth: usize) {
        if from_display == to_display {
            return;
        }
        let Some(vt) = self.cfg.default_view_transform() else {
            self.push(Xf::Unsupported("the scene ↔ display reference conversion (the config has no view transform)".into()));
            return;
        };
        let (fwd, back) = (&vt.from_scene, &vt.to_scene);
        match (to_display, fwd, back) {
            (true, Some(x), _) | (false, _, Some(x)) => self.xf(x, false, depth),
            (true, None, Some(x)) | (false, Some(x), None) => self.xf(x, true, depth),
            _ => {}
        }
    }

    /// Space `a` → space `b`, whatever `isdata` says.
    fn convert(&mut self, a: &ConfigSpace, b: &ConfigSpace, depth: usize) {
        if a.name == b.name {
            return;
        }
        self.space_to_ref(a, depth);
        self.bridge(a.display, b.display, depth);
        self.ref_to_space(b, depth);
    }

    /// Space `a` → space `b`; data spaces pass through.
    fn path(&mut self, a: &ConfigSpace, b: &ConfigSpace, depth: usize) {
        if !(a.is_data || b.is_data) {
            self.convert(a, b, depth);
        }
    }

    /// Apply a look string from space `cur`, each look in its process space; the space the
    /// colours end in.
    fn looks(&mut self, mut cur: &'c ConfigSpace, spec: &str, depth: usize) -> &'c ConfigSpace {
        for item in spec.split([',', ':']).map(str::trim).filter(|t| !t.is_empty()) {
            let (inverse, names) = match item.strip_prefix('-') {
                Some(n) => (true, n),
                None => (false, item.trim_start_matches('+')),
            };
            // `A | B`: the first look the config has.
            let Some(look) = names.split('|').find_map(|n| self.cfg.look(n)) else {
                self.push(Xf::Unsupported(format!("Look `{}` (not in the config)", names.trim())));
                continue;
            };
            if let Some(ps) = self.cfg.space(&look.process_space) {
                self.path(cur, ps, depth);
                cur = ps;
            }
            match (inverse, &look.transform, &look.inverse_transform) {
                (false, Some(x), _) | (true, _, Some(x)) => self.xf(x, false, depth),
                (false, None, Some(x)) | (true, Some(x), None) => self.xf(x, true, depth),
                _ => {}
            }
        }
        cur
    }

    fn display_view(&mut self, input: &'c ConfigSpace, display: &str, view: &str, looks_bypass: bool, data_bypass: bool, depth: usize) {
        if input.is_data && data_bypass {
            return;
        }
        let cfg = self.cfg;
        let Some((dname, v)) = cfg.view(display, view) else {
            self.push(Xf::Unsupported(format!("display `{display}` (the config has no displays)")));
            return;
        };
        let cur = if looks_bypass { input } else { self.looks(input, &v.looks, depth) };
        if v.view_transform.is_empty() {
            if let Some(cs) = self.space(&v.colorspace, "View") {
                self.path(cur, cs, depth);
            }
            return;
        }
        let Some(vt) = cfg.view_transform(&v.view_transform) else {
            self.push(Xf::Unsupported(format!("View (no view transform `{}`)", v.view_transform)));
            return;
        };
        let dcs_name = if v.display_colorspace == "<USE_DISPLAY_NAME>" { dname } else { v.display_colorspace.as_str() };
        let Some(dcs) = self.space(dcs_name, "View") else { return };
        // To the view transform's input reference, through it to the display reference, then
        // to the display colour space.
        let scene = vt.scene();
        self.space_to_ref(cur, depth);
        self.bridge(cur.display, !scene, depth);
        let (fwd, back) = if scene { (&vt.from_scene, &vt.to_scene) } else { (&vt.from_display, &vt.to_display) };
        match (fwd, back) {
            (Some(x), _) => self.xf(x, false, depth),
            (None, Some(x)) => self.xf(x, true, depth),
            _ => {}
        }
        if !dcs.is_data {
            self.bridge(true, dcs.display, depth);
            self.ref_to_space(dcs, depth);
        }
    }
}

type ConfigCache = Mutex<HashMap<String, Result<Arc<Config>, String>>>;

/// Load (and cache) a config from a `.ocio` path or the config text itself; why not, when it
/// can't be read.
pub fn load_config(src: &str) -> Result<Arc<Config>, String> {
    let t = src.trim();
    if t.is_empty() {
        return Err("no config file chosen".into());
    }
    static CACHE: OnceLock<ConfigCache> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(m) = cache.lock()
        && let Some(v) = m.get(src)
    {
        return v.clone();
    }
    let parsed = if src.contains('\n') {
        Config::parse(src, Path::new("."))
    } else {
        let p = Path::new(t);
        std::fs::read_to_string(p).map_err(|e| format!("{t}: {e}")).and_then(|text| Config::parse(&text, p.parent().unwrap_or(Path::new("."))))
    }
    .map(Arc::new);
    if let Ok(mut m) = cache.lock() {
        m.insert(src.to_string(), parsed.clone());
    }
    parsed
}

#[cfg(test)]
mod tests;

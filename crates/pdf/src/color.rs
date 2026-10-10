//! Colour spaces (ISO 32000-1 §8.6) reduced to sRGB, and functions (§7.10: sampled,
//! exponential, stitching and PostScript calculator functions).

use std::collections::HashMap;
use std::sync::Arc;

use crate::object::{Dict, File, Obj, decode_stream};

#[derive(Clone, Debug, PartialEq)]
pub enum Cs {
    Gray,
    Rgb,
    Cmyk,
    /// CIE L*a*b* (approximated).
    Lab,
    Indexed {
        base: Box<Cs>,
        hival: usize,
        lookup: Vec<u8>,
    },
    /// Separation / DeviceN: components → alternate space through a tint transform.
    Tint {
        n: usize,
        alt: Box<Cs>,
        func: Func,
    },
    Pattern,
}

impl Cs {
    pub fn components(&self) -> usize {
        match self {
            Cs::Gray | Cs::Indexed { .. } => 1,
            Cs::Rgb | Cs::Lab => 3,
            Cs::Cmyk => 4,
            Cs::Tint { n, .. } => *n,
            Cs::Pattern => 0,
        }
    }

    /// The initial colour of the space (black, or tint 1).
    pub fn initial(&self) -> Vec<f64> {
        match self {
            Cs::Cmyk => vec![0.0, 0.0, 0.0, 1.0],
            Cs::Tint { n, .. } => vec![1.0; *n],
            Cs::Lab => vec![0.0, 0.0, 0.0],
            c => vec![0.0; c.components()],
        }
    }

    pub fn to_rgb(&self, c: &[f64]) -> [f64; 3] {
        let g = |i: usize| c.get(i).copied().unwrap_or(0.0).clamp(0.0, 1.0);
        match self {
            Cs::Gray => [g(0); 3],
            Cs::Rgb | Cs::Pattern => [g(0), g(1), g(2)],
            Cs::Cmyk => {
                let k = g(3);
                [(1.0 - g(0)) * (1.0 - k), (1.0 - g(1)) * (1.0 - k), (1.0 - g(2)) * (1.0 - k)]
            }
            Cs::Lab => lab_to_rgb(c.first().copied().unwrap_or(0.0), c.get(1).copied().unwrap_or(0.0), c.get(2).copied().unwrap_or(0.0)),
            Cs::Indexed { base, hival, lookup } => {
                let i = (c.first().copied().unwrap_or(0.0).round().max(0.0) as usize).min(*hival);
                let n = base.components();
                let v: Vec<f64> = (0..n).map(|k| lookup.get(i * n + k).copied().unwrap_or(0) as f64 / 255.0).collect();
                base.to_rgb(&v)
            }
            Cs::Tint { alt, func, .. } => alt.to_rgb(&func.eval(c)),
        }
    }
}

fn lab_to_rgb(l: f64, a: f64, b: f64) -> [f64; 3] {
    let fy = (l + 16.0) / 116.0;
    let fx = fy + a / 500.0;
    let fz = fy - b / 200.0;
    let f = |t: f64| if t > 6.0 / 29.0 { t * t * t } else { 3.0 * (6.0f64 / 29.0).powi(2) * (t - 4.0 / 29.0) };
    let (x, y, z) = (0.9505 * f(fx), f(fy), 1.089 * f(fz));
    let lin = [3.2406 * x - 1.5372 * y - 0.4986 * z, -0.9689 * x + 1.8758 * y + 0.0415 * z, 0.0557 * x - 0.204 * y + 1.057 * z];
    lin.map(|v| {
        let v = v.clamp(0.0, 1.0);
        if v <= 0.003_130_8 { 12.92 * v } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 }
    })
}

/// How deep colour spaces nest (an Indexed base, the alternate of an ICCBased, Separation or
/// DeviceN space) and, counted separately, functions (in arrays and stitching functions).
/// References can make either a cycle, while real files nest two or three levels; anything deeper
/// reads as unreadable (DeviceGray, an unsupported function).
const MAX_DEPTH: usize = 8;

/// Read a colour space (a name, or an array such as `[/ICCBased 5 0 R]`); named resources are
/// looked up in `res` (`/ColorSpace`).
pub fn color_space(file: &File, o: &Obj, res: Option<&Dict>) -> Cs {
    color_space_at(file, o, res, 0)
}

/// [`color_space`] nested `depth` levels deep: DeviceGray past [`MAX_DEPTH`].
fn color_space_at(file: &File, o: &Obj, res: Option<&Dict>, depth: usize) -> Cs {
    if depth >= MAX_DEPTH {
        return Cs::Gray;
    }
    let o = file.resolve(o);
    match o {
        Obj::Name(n) => match n.as_str() {
            "DeviceGray" | "G" | "CalGray" => Cs::Gray,
            "DeviceRGB" | "RGB" | "CalRGB" => Cs::Rgb,
            "DeviceCMYK" | "CMYK" => Cs::Cmyk,
            "Pattern" => Cs::Pattern,
            other => match res.and_then(|r| file.get_dict(r, "ColorSpace")).and_then(|cs| file.get(cs, other)) {
                Some(x) if !matches!(x, Obj::Name(m) if m == other) => color_space_at(file, x, None, depth + 1),
                _ => Cs::Gray,
            },
        },
        Obj::Array(a) => {
            let head = a.first().map(|h| file.resolve(h)).and_then(Obj::name).unwrap_or("");
            match head {
                "ICCBased" => {
                    let d = a.get(1).map(|s| file.resolve(s)).and_then(Obj::dict);
                    // The profile's alternate space when it names one (profiles are not
                    // evaluated), else the device space with the same number of components.
                    if let Some(alt) = d.and_then(|d| file.get(d, "Alternate")) {
                        return color_space_at(file, alt, res, depth + 1);
                    }
                    let n = d.and_then(|d| file.get_num(d, "N")).unwrap_or(3.0) as usize;
                    match n {
                        1 => Cs::Gray,
                        4 => Cs::Cmyk,
                        _ => Cs::Rgb,
                    }
                }
                "CalRGB" => Cs::Rgb,
                "CalGray" => Cs::Gray,
                "Lab" => Cs::Lab,
                "Pattern" => Cs::Pattern,
                "Indexed" | "I" => {
                    let base = a.get(1).map(|b| color_space_at(file, b, res, depth + 1)).unwrap_or(Cs::Rgb);
                    // An indexed palette holds at most 256 entries, so hival (its largest index) is
                    // 0 to 255; an image builds its palette from every entry up to hival.
                    let hival = (a.get(2).and_then(|h| file.resolve(h).num()).unwrap_or(0.0) as usize).min(255);
                    let lookup = match a.get(3).map(|l| file.resolve(l)) {
                        Some(Obj::Str(s)) => s.clone(),
                        Some(Obj::Stream(d, raw)) => decode_stream(file, d, raw).unwrap_or_default(),
                        _ => vec![],
                    };
                    Cs::Indexed { base: Box::new(base), hival, lookup }
                }
                "Separation" | "DeviceN" => {
                    let n = if head == "Separation" { 1 } else { a.get(1).map(|x| file.resolve(x)).and_then(Obj::array).map_or(1, <[Obj]>::len) };
                    let alt = a.get(2).map(|b| color_space_at(file, b, res, depth + 1)).unwrap_or(Cs::Gray);
                    let func = a.get(3).map(|f| Func::read(file, f)).unwrap_or(Func::Unsupported);
                    // An unreadable tint transform: tint 1 = black, 0 = white.
                    if func == Func::Unsupported {
                        return Cs::Tint { n, alt: Box::new(Cs::Gray), func: Func::Exp { domain: [0.0, 1.0], c0: vec![1.0], c1: vec![0.0], n: 1.0 } };
                    }
                    Cs::Tint { n, alt: Box::new(alt), func }
                }
                _ => Cs::Rgb,
            }
        }
        _ => Cs::Gray,
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Func {
    /// Type 2.
    Exp {
        domain: [f64; 2],
        c0: Vec<f64>,
        c1: Vec<f64>,
        n: f64,
    },
    /// Type 3.
    Stitch {
        domain: [f64; 2],
        funcs: Vec<Func>,
        bounds: Vec<f64>,
        encode: Vec<f64>,
    },
    /// Type 0 with one input. Copies share the samples (a graphics state is copied at every `q`).
    Sampled {
        domain: [f64; 2],
        size: usize,
        outputs: usize,
        encode: [f64; 2],
        decode: Vec<f64>,
        samples: Arc<[f64]>,
    },
    /// Type 0 with several inputs (function-based shadings): multilinear interpolation, the
    /// first input varying fastest in the sample table.
    SampledN {
        domain: Vec<f64>,
        size: Vec<usize>,
        outputs: usize,
        encode: Vec<f64>,
        decode: Vec<f64>,
        samples: Arc<[f64]>,
    },
    /// An array of one-output functions.
    Array(Vec<Func>),
    /// Type 4: a PostScript calculator program (§7.10.5).
    Calc {
        domain: Vec<f64>,
        range: Vec<f64>,
        code: Vec<PsOp>,
    },
    Unsupported,
}

/// A PostScript calculator token; procedures nest for `if` / `ifelse`.
#[derive(Clone, Debug, PartialEq)]
pub enum PsOp {
    Num(f64),
    Op(String),
    Proc(Vec<PsOp>),
}

fn ps_parse(lx: &mut crate::object::Lexer, depth: usize) -> Vec<PsOp> {
    let mut out = vec![];
    while let Some(o) = lx.next() {
        match o {
            Obj::Num(n) => out.push(PsOp::Num(n)),
            Obj::Bool(b) => out.push(PsOp::Num(if b { 1.0 } else { 0.0 })),
            Obj::Op(op) if op == "{" && depth < 32 => out.push(PsOp::Proc(ps_parse(lx, depth + 1))),
            // Nested too deep: skip the procedure's opening brace (never push it as an operator).
            Obj::Op(op) if op == "{" => {}
            Obj::Op(op) if op == "}" => return out,
            Obj::Op(op) => out.push(PsOp::Op(op)),
            _ => {}
        }
    }
    out
}

/// A calculator function's operand stack holds at most 100 values (the PDF specification's
/// implementation limits); a real program uses a handful.
const MAX_STACK: usize = 100;

/// Push onto a calculator's operand stack; a full stack takes nothing more.
fn ps_push(st: &mut Vec<f64>, v: f64) {
    if st.len() < MAX_STACK {
        st.push(v);
    }
}

fn ps_run(code: &[PsOp], st: &mut Vec<f64>, depth: usize) {
    if depth > 32 {
        return;
    }
    let mut procs: Vec<&[PsOp]> = vec![];
    for op in code {
        match op {
            PsOp::Num(n) => ps_push(st, *n),
            PsOp::Proc(p) => procs.push(p),
            PsOp::Op(o) => {
                let pop = |st: &mut Vec<f64>| st.pop().unwrap_or(0.0);
                let b = |v: bool| if v { 1.0 } else { 0.0 };
                match o.as_str() {
                    "if" => {
                        let p = procs.pop();
                        if pop(st) != 0.0
                            && let Some(p) = p
                        {
                            ps_run(p, st, depth + 1);
                        }
                    }
                    "ifelse" => {
                        let (f, t) = (procs.pop(), procs.pop());
                        let c = pop(st) != 0.0;
                        if let Some(p) = if c { t } else { f } {
                            ps_run(p, st, depth + 1);
                        }
                    }
                    "true" => ps_push(st, 1.0),
                    "false" => ps_push(st, 0.0),
                    "pop" => {
                        st.pop();
                    }
                    "dup" => {
                        let v = st.last().copied().unwrap_or(0.0);
                        ps_push(st, v);
                    }
                    "exch" => {
                        let n = st.len();
                        if n >= 2 {
                            st.swap(n - 1, n - 2);
                        }
                    }
                    "copy" => {
                        let k = pop(st).max(0.0) as usize;
                        let n = st.len();
                        // A copy that would pass the cap copies nothing.
                        if k <= n && n + k <= MAX_STACK {
                            let tail = st.get(n - k..).unwrap_or_default().to_vec();
                            st.extend(tail);
                        }
                    }
                    "index" => {
                        let k = pop(st).max(0.0) as usize;
                        let v = k.checked_add(1).and_then(|k| st.len().checked_sub(k)).and_then(|i| st.get(i)).copied().unwrap_or(0.0);
                        ps_push(st, v);
                    }
                    "roll" => {
                        let j = pop(st) as i64;
                        let n = pop(st).max(0.0) as usize;
                        let len = st.len();
                        if n > 0 && n <= len {
                            let s = &mut st[len - n..];
                            let r = j.rem_euclid(n as i64) as usize;
                            s.rotate_right(r);
                        }
                    }
                    _ => {
                        // Unary and binary operators.
                        let unary: Option<fn(f64) -> f64> = match o.as_str() {
                            "abs" => Some(f64::abs),
                            "neg" => Some(|x| -x),
                            "ceiling" => Some(f64::ceil),
                            "floor" => Some(f64::floor),
                            "round" => Some(|x: f64| (x + 0.5).floor()),
                            "truncate" | "cvi" => Some(f64::trunc),
                            "cvr" => Some(|x| x),
                            "sqrt" => Some(|x: f64| x.max(0.0).sqrt()),
                            "sin" => Some(|x: f64| x.to_radians().sin()),
                            "cos" => Some(|x: f64| x.to_radians().cos()),
                            "ln" => Some(|x: f64| x.max(1e-300).ln()),
                            "log" => Some(|x: f64| x.max(1e-300).log10()),
                            "not" => Some(|x: f64| {
                                if x == 0.0 {
                                    1.0
                                } else if x == 1.0 {
                                    0.0
                                } else {
                                    !(x as i64) as f64
                                }
                            }),
                            _ => None,
                        };
                        if let Some(f) = unary {
                            let x = pop(st);
                            ps_push(st, f(x));
                            continue;
                        }
                        let y = pop(st);
                        let x = pop(st);
                        let v = match o.as_str() {
                            "add" => x + y,
                            "sub" => x - y,
                            "mul" => x * y,
                            "div" => {
                                if y != 0.0 {
                                    x / y
                                } else {
                                    0.0
                                }
                            }
                            // `checked_*` give 0 both for a zero divisor and for the one overflowing
                            // case, `i64::MIN / -1` (`/` and `%` panic on it in every build).
                            "idiv" => (x as i64).checked_div(y as i64).unwrap_or(0) as f64,
                            "mod" => (x as i64).checked_rem(y as i64).unwrap_or(0) as f64,
                            "exp" => x.powf(y),
                            "atan" => {
                                let a = x.atan2(y).to_degrees();
                                if a < 0.0 { a + 360.0 } else { a }
                            }
                            "eq" => b(x == y),
                            "ne" => b(x != y),
                            "gt" => b(x > y),
                            "ge" => b(x >= y),
                            "lt" => b(x < y),
                            "le" => b(x <= y),
                            "and" => ((x as i64) & (y as i64)) as f64,
                            "or" => ((x as i64) | (y as i64)) as f64,
                            "xor" => ((x as i64) ^ (y as i64)) as f64,
                            "bitshift" => {
                                let (v, s) = (x as i64, y as i64);
                                (if s >= 0 { v << s.min(62) } else { v >> (-s).min(62) }) as f64
                            }
                            _ => {
                                ps_push(st, x);
                                y
                            }
                        };
                        ps_push(st, v);
                    }
                }
            }
        }
    }
}

/// The most `f64`s one sampled table holds: 16 Mi (128 MiB), room for a 33 × 33 × 33 × 33 grid
/// with four outputs (4_743_684 samples). One table this large is allowed; what stops a file from
/// building many of them is [`Budget`], which caps the samples decoded across a whole
/// [`Func::read`] at the same figure.
const MAX_SAMPLES: usize = 1 << 24;

/// The most inputs or outputs of a function: no colour space has more than 32 components (the most
/// a DeviceN space may have), so a sampled function needs no more. Also the most one-output
/// functions a [`Func::Array`] holds (one per output). It is NOT a limit on the pieces of a
/// stitching function — a repeating gradient lists one piece once per repeat, often many more.
const MAX_IO: usize = 32;

/// The most `Func` nodes one top-level [`Func::read`] builds, across the whole structure of
/// references. A real function is a handful of nodes; a cyclic or fan-out reference graph would
/// build far more, so past this the whole function is unreadable. It is a soft limit, and the only
/// one on the pieces of a stitching function: past it a colour space takes its fallback (a
/// Separation tint reads as grey), nothing unsafe.
const MAX_NODES: usize = 10_000;

/// Samples in a table of `total` entries × `outputs`; `None` when that overflows or passes
/// [`MAX_SAMPLES`].
fn sample_count(total: usize, outputs: usize) -> Option<usize> {
    total.checked_mul(outputs).filter(|n| *n <= MAX_SAMPLES)
}

/// What one top-level [`Func::read`] may build before the whole function is treated as unreadable:
/// a running cap on `Func` nodes and on sampled-table samples across the entire structure, so a
/// reference graph that is cyclic or fans out (one object listed many times at each of several
/// levels) cannot build an unbounded tree or decode unbounded memory. Samples of a table shared by
/// repeat references (one `Arc`) are charged once; its nodes are charged per copy.
struct Budget {
    nodes: usize,
    samples: usize,
    /// Set once either cap is passed; the top-level read then yields [`Func::Unsupported`].
    overrun: bool,
}

impl Budget {
    fn new() -> Self {
        Budget { nodes: MAX_NODES, samples: MAX_SAMPLES, overrun: false }
    }

    /// Charge `n` nodes; `false` (and the read is abandoned) once the cap is passed.
    fn take_nodes(&mut self, n: usize) -> bool {
        match self.nodes.checked_sub(n) {
            Some(left) => {
                self.nodes = left;
                true
            }
            None => {
                self.overrun = true;
                false
            }
        }
    }

    /// Charge `n` samples; `false` once the cap is passed.
    fn take_samples(&mut self, n: usize) -> bool {
        match self.samples.checked_sub(n) {
            Some(left) => {
                self.samples = left;
                true
            }
            None => {
                self.overrun = true;
                false
            }
        }
    }
}

/// `v` limited to the range between `a` and `b`, in either order. Unlike [`f64::clamp`] this never
/// panics, whether a file gives the bounds reversed (`/Domain [1 0]`) or a bound is NaN.
fn clamp(v: f64, a: f64, b: f64) -> f64 {
    let (lo, hi) = (a.min(b), a.max(b));
    if v < lo {
        lo
    } else if v > hi {
        hi
    } else {
        v
    }
}

impl Func {
    pub fn read(file: &File, o: &Obj) -> Func {
        let mut budget = Budget::new();
        let f = Func::read_at(file, o, 0, &mut budget);
        // Either cap passed anywhere in the structure: the whole function is unreadable.
        if budget.overrun { Func::Unsupported } else { f }
    }

    /// [`Func::read`] nested `depth` levels deep, charging `budget`: unsupported past [`MAX_DEPTH`]
    /// or once the budget is spent.
    fn read_at(file: &File, o: &Obj, depth: usize, budget: &mut Budget) -> Func {
        if depth >= MAX_DEPTH || !budget.take_nodes(1) {
            return Func::Unsupported;
        }
        let o = file.resolve(o);
        if let Obj::Array(a) = o {
            // An array of one-output functions: one per output, so at most `MAX_IO`.
            if a.len() > MAX_IO {
                return Func::Unsupported;
            }
            return Func::read_all(file, a, depth + 1, budget).map_or(Func::Unsupported, Func::Array);
        }
        let Some(d) = o.dict() else { return Func::Unsupported };
        let dom = file.get(d, "Domain").map(|x| file.nums(x)).unwrap_or_default();
        let domain = [dom.first().copied().unwrap_or(0.0), dom.get(1).copied().unwrap_or(1.0)];
        match file.get_num(d, "FunctionType").unwrap_or(-1.0) as i32 {
            2 => Func::Exp {
                domain,
                c0: file.get(d, "C0").map(|x| file.nums(x)).unwrap_or_else(|| vec![0.0]),
                c1: file.get(d, "C1").map(|x| file.nums(x)).unwrap_or_else(|| vec![1.0]),
                n: file.get_num(d, "N").unwrap_or(1.0),
            },
            3 => {
                // A stitching function's pieces: no element cap (a repeating gradient lists one
                // piece once per repeat); the budget bounds the total work.
                let funcs = match file.get(d, "Functions").and_then(Obj::array) {
                    Some(a) => Func::read_all(file, a, depth + 1, budget),
                    None => Some(vec![]),
                };
                let Some(funcs) = funcs else { return Func::Unsupported };
                Func::Stitch {
                    domain,
                    funcs,
                    bounds: file.get(d, "Bounds").map(|x| file.nums(x)).unwrap_or_default(),
                    encode: file.get(d, "Encode").map(|x| file.nums(x)).unwrap_or_default(),
                }
            }
            0 => {
                let Obj::Stream(sd, raw) = o else { return Func::Unsupported };
                let Some(data) = decode_stream(file, sd, raw) else { return Func::Unsupported };
                let sizes: Vec<usize> = file.get(d, "Size").map(|x| file.nums(x)).unwrap_or_default().iter().map(|v| v.max(0.0) as usize).collect();
                let size = sizes.first().copied().unwrap_or(0);
                let bps = file.get_num(d, "BitsPerSample").unwrap_or(8.0) as usize;
                let range = file.get(d, "Range").map(|x| file.nums(x)).unwrap_or_default();
                let outputs = range.len() / 2;
                if size == 0 || outputs == 0 || outputs > MAX_IO || sizes.len() > MAX_IO || !matches!(bps, 1 | 2 | 4 | 8 | 12 | 16 | 24 | 32) {
                    return Func::Unsupported;
                }
                let total = sizes.iter().try_fold(1usize, |a, b| a.checked_mul(*b)).filter(|t| *t > 0 && *t <= 1 << 22);
                let Some(total) = total else { return Func::Unsupported };
                let Some(count) = sample_count(total, outputs) else { return Func::Unsupported };
                if !budget.take_samples(count) {
                    return Func::Unsupported;
                }
                let enc = file.get(d, "Encode").map(|x| file.nums(x)).unwrap_or_default();
                let encode = [enc.first().copied().unwrap_or(0.0), enc.get(1).copied().unwrap_or(size as f64 - 1.0)];
                let decode = file.get(d, "Decode").map(|x| file.nums(x)).filter(|v| v.len() >= outputs * 2).unwrap_or_else(|| range.clone());
                let max = ((1u64 << bps) - 1) as f64;
                let mut bit = 0usize;
                let samples: Arc<[f64]> = (0..count)
                    .map(|_| {
                        let mut v = 0u64;
                        for _ in 0..bps {
                            let byte = data.get(bit / 8).copied().unwrap_or(0);
                            v = (v << 1) | ((byte >> (7 - bit % 8)) & 1) as u64;
                            bit += 1;
                        }
                        v as f64 / max
                    })
                    .collect();
                if sizes.len() > 1 {
                    let m = sizes.len();
                    let encode =
                        (0..m).flat_map(|i| [enc.get(2 * i).copied().unwrap_or(0.0), enc.get(2 * i + 1).copied().unwrap_or(sizes[i] as f64 - 1.0)]).collect();
                    let domain = (0..m).flat_map(|i| [dom.get(2 * i).copied().unwrap_or(0.0), dom.get(2 * i + 1).copied().unwrap_or(1.0)]).collect();
                    return Func::SampledN { domain, size: sizes, outputs, encode, decode, samples };
                }
                Func::Sampled { domain, size, outputs, encode, decode, samples }
            }
            4 => {
                let Obj::Stream(sd, raw) = o else { return Func::Unsupported };
                let Some(data) = decode_stream(file, sd, raw) else { return Func::Unsupported };
                let mut lx = crate::object::Lexer::new(&data, 0);
                // The program is one procedure: `{ … }`.
                let code = match ps_parse(&mut lx, 0).into_iter().next() {
                    Some(PsOp::Proc(p)) => p,
                    _ => return Func::Unsupported,
                };
                Func::Calc { domain: dom, range: file.get(d, "Range").map(|x| file.nums(x)).unwrap_or_default(), code }
            }
            _ => Func::Unsupported,
        }
    }

    /// The functions of an array (of one-output functions, or a stitching function's
    /// `/Functions`) read at `depth`, charging `budget`; `None` once the budget is spent. An object
    /// the array refers to more than once is read once and copied: the copies share its samples
    /// (one `Arc`, charged once), but each copy is charged its nodes again — so a reference listed
    /// many times at each of several levels spends the node budget quickly rather than expanding.
    fn read_all(file: &File, a: &[Obj], depth: usize, budget: &mut Budget) -> Option<Vec<Func>> {
        let mut cache: HashMap<(u32, u16), (Func, usize)> = HashMap::new();
        let mut funcs = Vec::with_capacity(a.len());
        for o in a {
            if budget.overrun {
                return None;
            }
            let f = match o {
                Obj::Ref(n, g) => {
                    if let Some((cached, cost)) = cache.get(&(*n, *g)) {
                        let (cached, cost) = (cached.clone(), *cost);
                        if !budget.take_nodes(cost) {
                            return None;
                        }
                        cached
                    } else {
                        let before = budget.nodes;
                        let f = Func::read_at(file, o, depth, budget);
                        let cost = before.saturating_sub(budget.nodes);
                        cache.insert((*n, *g), (f.clone(), cost));
                        f
                    }
                }
                _ => Func::read_at(file, o, depth, budget),
            };
            funcs.push(f);
        }
        Some(funcs)
    }

    /// Evaluate at the first input (the functions used by shadings and tints take one input
    /// in practice; extra inputs of DeviceN tints are averaged in).
    pub fn eval(&self, input: &[f64]) -> Vec<f64> {
        if let Func::Calc { domain, range, code } = self {
            // The inputs start the operand stack, which holds at most `MAX_STACK` values.
            let mut st: Vec<f64> = input
                .iter()
                .take(MAX_STACK)
                .enumerate()
                .map(|(i, v)| match (domain.get(2 * i), domain.get(2 * i + 1)) {
                    (Some(a), Some(b)) => clamp(*v, *a, *b),
                    _ => *v,
                })
                .collect();
            ps_run(code, &mut st, 0);
            let n = range.len() / 2;
            let k = st.len().saturating_sub(n);
            return st.get(k..).unwrap_or_default().iter().zip(range.as_chunks::<2>().0).map(|(v, [a, b])| clamp(*v, *a, *b)).collect();
        }
        match self {
            Func::SampledN { domain, size, outputs, encode, decode, samples } => {
                let m = size.len();
                // Each input → a sample-table coordinate, then the 2^m surrounding samples.
                let mut base = vec![0usize; m];
                let mut frac = vec![0.0f64; m];
                for i in 0..m {
                    let x = input.get(i).copied().unwrap_or(0.0);
                    let (d0, d1) = (domain[2 * i], domain[2 * i + 1]);
                    let x = clamp(x, d0, d1);
                    let e = if d1 != d0 { encode[2 * i] + (x - d0) / (d1 - d0) * (encode[2 * i + 1] - encode[2 * i]) } else { encode[2 * i] };
                    let e = if e.is_finite() { clamp(e, 0.0, size[i] as f64 - 1.0) } else { 0.0 };
                    base[i] = e.floor() as usize;
                    frac[i] = e - e.floor();
                }
                let corners = if m <= 6 { 1usize << m } else { 1 };
                let mut out = vec![0.0; *outputs];
                for c in 0..corners {
                    let mut w = 1.0;
                    let mut idx = 0usize;
                    let mut stride = 1usize;
                    for i in 0..m {
                        let up = m <= 6 && (c >> i) & 1 == 1;
                        w *= if up { frac[i] } else { 1.0 - frac[i] };
                        let k = if up { (base[i] + 1).min(size[i] - 1) } else { base[i] };
                        idx += k * stride;
                        stride *= size[i];
                    }
                    if w == 0.0 {
                        continue;
                    }
                    for (o, v) in out.iter_mut().enumerate() {
                        *v += w * samples.get(idx * outputs + o).copied().unwrap_or(0.0);
                    }
                }
                for (o, v) in out.iter_mut().enumerate() {
                    let (d0, d1) = (decode.get(2 * o).copied().unwrap_or(0.0), decode.get(2 * o + 1).copied().unwrap_or(1.0));
                    *v = d0 + *v * (d1 - d0);
                }
                out
            }
            Func::Array(fs) if input.len() > 1 => fs.iter().map(|f| f.eval(input).first().copied().unwrap_or(0.0)).collect(),
            _ => self.eval1(input.first().copied().unwrap_or(0.0)),
        }
    }

    pub fn eval1(&self, t: f64) -> Vec<f64> {
        match self {
            Func::Exp { domain, c0, c1, n } => {
                let t = clamp(t, domain[0], domain[1]);
                let x = if *n == 1.0 { t } else { t.max(0.0).powf(*n) };
                (0..c0.len().max(c1.len()))
                    .map(|i| c0.get(i).copied().unwrap_or(0.0) + x * (c1.get(i).copied().unwrap_or(1.0) - c0.get(i).copied().unwrap_or(0.0)))
                    .collect()
            }
            Func::Stitch { domain, funcs, bounds, encode } => {
                if funcs.is_empty() {
                    return vec![0.0];
                }
                let t = clamp(t, domain[0], domain[1]);
                let k = bounds.iter().take_while(|b| t >= **b).count().min(funcs.len() - 1);
                let lo = if k == 0 { domain[0] } else { bounds[k - 1] };
                let hi = bounds.get(k).copied().unwrap_or(domain[1]);
                let (e0, e1) = (encode.get(2 * k).copied().unwrap_or(0.0), encode.get(2 * k + 1).copied().unwrap_or(1.0));
                let u = if hi > lo { e0 + (t - lo) / (hi - lo) * (e1 - e0) } else { e0 };
                funcs[k].eval1(u)
            }
            Func::Sampled { domain, size, outputs, encode, decode, samples } => {
                let t = clamp(t, domain[0], domain[1]);
                let e = if domain[1] > domain[0] { encode[0] + (t - domain[0]) / (domain[1] - domain[0]) * (encode[1] - encode[0]) } else { encode[0] };
                let e = clamp(e, 0.0, *size as f64 - 1.0);
                let (i0, f) = (e.floor() as usize, e.fract());
                let i1 = (i0 + 1).min(size - 1);
                (0..*outputs)
                    .map(|o| {
                        let s = samples[i0 * outputs + o] * (1.0 - f) + samples[i1 * outputs + o] * f;
                        let (d0, d1) = (decode.get(2 * o).copied().unwrap_or(0.0), decode.get(2 * o + 1).copied().unwrap_or(1.0));
                        d0 + s * (d1 - d0)
                    })
                    .collect()
            }
            Func::Array(fs) => fs.iter().map(|f| f.eval1(t).first().copied().unwrap_or(0.0)).collect(),
            Func::Calc { .. } | Func::SampledN { .. } => self.eval(&[t]),
            Func::Unsupported => vec![0.5],
        }
    }

    /// Input values worth sampling exactly (stitching bounds).
    pub fn breakpoints(&self) -> Vec<f64> {
        match self {
            Func::Stitch { bounds, .. } => bounds.clone(),
            _ => vec![],
        }
    }
}

/// PDF text string → Rust string (UTF-16BE with a byte-order mark, else PDFDocEncoding ≈ Latin-1).
pub fn text_string(b: &[u8]) -> String {
    if b.starts_with(&[0xFE, 0xFF]) {
        let u: Vec<u16> = b[2..].as_chunks::<2>().0.iter().map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
        return String::from_utf16_lossy(&u);
    }
    if let Some(rest) = b.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8_lossy(rest).into_owned();
    }
    b.iter().map(|&c| c as char).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sample_count_at_and_over_the_cap() {
        // 4 Mi entries × 4 outputs fill the cap exactly; a fifth output passes it.
        assert_eq!(sample_count(1 << 22, 4), Some(MAX_SAMPLES));
        assert_eq!(sample_count(1 << 22, 5), None);
        // A 33 × 33 × 33 × 33 grid with four outputs fits (4_743_684 samples, under the cap, so
        // `Some`).
        assert_eq!(sample_count(33 * 33 * 33 * 33, 4), Some(4_743_684));
        // A product that overflows is refused (on 64-bit targets that takes a `/Range` of at least
        // 2^41 numbers, too big to build in a test).
        assert_eq!(sample_count(1 << 22, usize::MAX / (1 << 22) + 1), None);
    }

    #[test]
    fn calculator_functions() {
        // Two inputs → three outputs: { 2 copy mul 3 1 roll add 2 div  dup 0.5 gt { pop 1 } if  0 }
        let code = b"{ 2 copy mul 3 1 roll add 2 div dup 0.5 gt { pop 1 } { 0.25 mul } ifelse 0 }";
        let mut lx = crate::object::Lexer::new(code, 0);
        let Some(PsOp::Proc(code)) = ps_parse(&mut lx, 0).into_iter().next() else { panic!() };
        let f = Func::Calc { domain: vec![0.0, 1.0, 0.0, 1.0], range: vec![0.0, 1.0, 0.0, 1.0, 0.0, 1.0], code };
        assert_eq!(f.eval(&[0.5, 0.4]), vec![0.2, 0.1125, 0.0]);
        assert_eq!(f.eval(&[1.0, 0.8]), vec![0.8, 1.0, 0.0]);
    }

    /// Run the calculator program `{ src }` on the stack `st`.
    fn run(src: &str, mut st: Vec<f64>) -> Vec<f64> {
        let src = format!("{{ {src} }}");
        let mut lx = crate::object::Lexer::new(src.as_bytes(), 0);
        let Some(PsOp::Proc(code)) = ps_parse(&mut lx, 0).into_iter().next() else { panic!() };
        ps_run(&code, &mut st, 0);
        st
    }

    #[test]
    fn calculator_stack_holds_at_most_100_values() {
        // `1 copy 2 copy 4 copy …`: 40 doublings asked for 2^40 values (8 TiB). The copies stop at
        // 64 values, since the next would pass 100.
        let doublings: String = (0..40).map(|i| format!("{} copy ", 1u64 << i)).collect();
        assert_eq!(run(&doublings, vec![0.0]).len(), 64);
        // A copy up to exactly 100 values is made; after it nothing grows the stack.
        let fifty: Vec<f64> = (0..50).map(f64::from).collect();
        let full = run("50 copy", fifty.clone());
        assert_eq!(full.len(), 100);
        assert_eq!(full[50..], fifty[..]);
        for op in ["7", "true", "false", "dup", "0 index", "1 copy", "neg", "add", "nonsense"] {
            assert!(run(op, full.clone()).len() <= 100, "{op}");
        }
        // Nor does a long program without loops: one push per operator, up to the cap.
        for op in ["1 ", "true ", "dup ", "0 index "] {
            assert_eq!(run(&op.repeat(1000), vec![1.0]).len(), 100, "{op}");
        }
    }

    #[test]
    fn calculator_index_past_the_stack_pushes_zero() {
        // 10^20 is past usize::MAX: `k + 1` overflowed (a panic), or wrapped to read past the stack.
        assert_eq!(run("99999999999999999999 index", vec![1.0, 2.0]), vec![1.0, 2.0, 0.0]);
        assert_eq!(run("2 index", vec![1.0, 2.0]), vec![1.0, 2.0, 0.0]);
        assert_eq!(run("1 index", vec![1.0, 2.0]), vec![1.0, 2.0, 1.0]);
    }

    #[test]
    fn calculator_copy_index_and_roll_below_the_cap() {
        assert_eq!(run("3 copy 1 index 4 1 roll", vec![1.0, 2.0, 3.0]), vec![1.0, 2.0, 3.0, 2.0, 1.0, 2.0, 3.0]);
        assert_eq!(run("3 -1 roll dup", vec![1.0, 2.0, 3.0]), vec![2.0, 3.0, 1.0, 1.0]);
    }

    #[test]
    fn calculator_idiv_and_mod_of_i64_min_by_minus_one() {
        // `i64::MIN / -1` and `i64::MIN % -1` overflow and panic (in debug and release); `idiv` and
        // `mod` must fall back like a zero divisor does. -2^63 is below any f64 the program sees,
        // but the cast saturates to i64::MIN, which is enough to trigger it.
        let min = -(2f64.powi(63));
        assert_eq!(run("idiv", vec![min, -1.0]), vec![0.0]);
        assert_eq!(run("mod", vec![min, -1.0]), vec![0.0]);
        // Divide by zero still gives 0, ordinary division still works.
        assert_eq!(run("idiv", vec![7.0, 0.0]), vec![0.0]);
        assert_eq!(run("mod", vec![7.0, 0.0]), vec![0.0]);
        assert_eq!(run("idiv", vec![7.0, 2.0]), vec![3.0]);
        assert_eq!(run("mod", vec![7.0, 2.0]), vec![1.0]);
    }
}

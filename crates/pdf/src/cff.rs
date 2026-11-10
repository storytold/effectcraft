//! Compact Font Format programs (`FontFile3` `/Type1C` and `/CIDFontType0C`, and the `CFF `
//! table of OpenType fonts), from Adobe Technical Note #5176 (The Compact Font Format
//! Specification) and #5177 (The Type 2 Charstring Format): INDEX and DICT structures,
//! charsets, encodings, CID-keyed FDArray / FDSelect, and a Type 2 charstring interpreter that
//! produces glyph outlines in glyph space.

use kurbo::BezPath;

/// The standard strings (SIDs 0–228: the ISOAdobe character set); SIDs 229–390 (the expert
/// set) are not named here and match no glyph name.
const STD: &[&str] = &[
    ".notdef",
    "space",
    "exclam",
    "quotedbl",
    "numbersign",
    "dollar",
    "percent",
    "ampersand",
    "quoteright",
    "parenleft",
    "parenright",
    "asterisk",
    "plus",
    "comma",
    "hyphen",
    "period",
    "slash",
    "zero",
    "one",
    "two",
    "three",
    "four",
    "five",
    "six",
    "seven",
    "eight",
    "nine",
    "colon",
    "semicolon",
    "less",
    "equal",
    "greater",
    "question",
    "at",
    "A",
    "B",
    "C",
    "D",
    "E",
    "F",
    "G",
    "H",
    "I",
    "J",
    "K",
    "L",
    "M",
    "N",
    "O",
    "P",
    "Q",
    "R",
    "S",
    "T",
    "U",
    "V",
    "W",
    "X",
    "Y",
    "Z",
    "bracketleft",
    "backslash",
    "bracketright",
    "asciicircum",
    "underscore",
    "quoteleft",
    "a",
    "b",
    "c",
    "d",
    "e",
    "f",
    "g",
    "h",
    "i",
    "j",
    "k",
    "l",
    "m",
    "n",
    "o",
    "p",
    "q",
    "r",
    "s",
    "t",
    "u",
    "v",
    "w",
    "x",
    "y",
    "z",
    "braceleft",
    "bar",
    "braceright",
    "asciitilde",
    "exclamdown",
    "cent",
    "sterling",
    "fraction",
    "yen",
    "florin",
    "section",
    "currency",
    "quotesingle",
    "quotedblleft",
    "guillemotleft",
    "guilsinglleft",
    "guilsinglright",
    "fi",
    "fl",
    "endash",
    "dagger",
    "daggerdbl",
    "periodcentered",
    "paragraph",
    "bullet",
    "quotesinglbase",
    "quotedblbase",
    "quotedblright",
    "guillemotright",
    "ellipsis",
    "perthousand",
    "questiondown",
    "grave",
    "acute",
    "circumflex",
    "tilde",
    "macron",
    "breve",
    "dotaccent",
    "dieresis",
    "ring",
    "cedilla",
    "hungarumlaut",
    "ogonek",
    "caron",
    "emdash",
    "AE",
    "ordfeminine",
    "Lslash",
    "Oslash",
    "OE",
    "ordmasculine",
    "ae",
    "dotlessi",
    "lslash",
    "oslash",
    "oe",
    "germandbls",
    "onesuperior",
    "logicalnot",
    "mu",
    "trademark",
    "Eth",
    "onehalf",
    "plusminus",
    "Thorn",
    "onequarter",
    "divide",
    "brokenbar",
    "degree",
    "thorn",
    "threequarters",
    "twosuperior",
    "registered",
    "minus",
    "eth",
    "multiply",
    "threesuperior",
    "copyright",
    "Aacute",
    "Acircumflex",
    "Adieresis",
    "Agrave",
    "Aring",
    "Atilde",
    "Ccedilla",
    "Eacute",
    "Ecircumflex",
    "Edieresis",
    "Egrave",
    "Iacute",
    "Icircumflex",
    "Idieresis",
    "Igrave",
    "Ntilde",
    "Oacute",
    "Ocircumflex",
    "Odieresis",
    "Ograve",
    "Otilde",
    "Scaron",
    "Uacute",
    "Ucircumflex",
    "Udieresis",
    "Ugrave",
    "Yacute",
    "Ydieresis",
    "Zcaron",
    "aacute",
    "acircumflex",
    "adieresis",
    "agrave",
    "aring",
    "atilde",
    "ccedilla",
    "eacute",
    "ecircumflex",
    "edieresis",
    "egrave",
    "iacute",
    "icircumflex",
    "idieresis",
    "igrave",
    "ntilde",
    "oacute",
    "ocircumflex",
    "odieresis",
    "ograve",
    "otilde",
    "scaron",
    "uacute",
    "ucircumflex",
    "udieresis",
    "ugrave",
    "yacute",
    "ydieresis",
    "zcaron",
];

const N_STD: usize = 391;

/// An INDEX: the byte ranges of its objects.
fn index(d: &[u8], pos: usize) -> Option<(Vec<std::ops::Range<usize>>, usize)> {
    let count = u16::from_be_bytes([*d.get(pos)?, *d.get(pos + 1)?]) as usize;
    if count == 0 {
        return Some((vec![], pos + 2));
    }
    let os = *d.get(pos + 2)? as usize;
    if !(1..=4).contains(&os) {
        return None;
    }
    let off = |i: usize| -> Option<usize> {
        let p = pos + 3 + i * os;
        let mut v = 0usize;
        for k in 0..os {
            v = (v << 8) | *d.get(p + k)? as usize;
        }
        Some(v)
    };
    let base = pos + 3 + (count + 1) * os - 1;
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let (a, b) = (off(i)?, off(i + 1)?);
        if b < a || base + b > d.len() {
            return None;
        }
        out.push(base + a..base + b);
    }
    let end = base + off(count)?;
    Some((out, end))
}

/// A DICT: (operator, operands) pairs; two-byte operators are `1200 + b1`.
fn dict(d: &[u8]) -> Vec<(u16, Vec<f64>)> {
    let mut out = vec![];
    let mut ops: Vec<f64> = vec![];
    let mut i = 0;
    while i < d.len() {
        let b0 = d[i];
        match b0 {
            0..=21 => {
                let op = if b0 == 12 {
                    i += 1;
                    1200 + *d.get(i).unwrap_or(&0) as u16
                } else {
                    b0 as u16
                };
                out.push((op, std::mem::take(&mut ops)));
                i += 1;
            }
            28 => {
                ops.push(i16::from_be_bytes([*d.get(i + 1).unwrap_or(&0), *d.get(i + 2).unwrap_or(&0)]) as f64);
                i += 3;
            }
            29 => {
                let b = |k: usize| *d.get(i + k).unwrap_or(&0);
                ops.push(i32::from_be_bytes([b(1), b(2), b(3), b(4)]) as f64);
                i += 5;
            }
            30 => {
                // Real number: BCD nibbles.
                let mut s = String::new();
                i += 1;
                'n: while i < d.len() {
                    for nib in [d[i] >> 4, d[i] & 15] {
                        match nib {
                            0..=9 => s.push((b'0' + nib) as char),
                            0xA => s.push('.'),
                            0xB => s.push('E'),
                            0xC => s.push_str("E-"),
                            0xE => s.push('-'),
                            0xF => {
                                i += 1;
                                break 'n;
                            }
                            _ => {}
                        }
                    }
                    i += 1;
                }
                ops.push(s.parse().unwrap_or(0.0));
            }
            32..=246 => {
                ops.push(b0 as f64 - 139.0);
                i += 1;
            }
            247..=250 => {
                ops.push((b0 as f64 - 247.0) * 256.0 + *d.get(i + 1).unwrap_or(&0) as f64 + 108.0);
                i += 2;
            }
            251..=254 => {
                ops.push(-(b0 as f64 - 251.0) * 256.0 - *d.get(i + 1).unwrap_or(&0) as f64 - 108.0);
                i += 2;
            }
            _ => i += 1,
        }
    }
    out
}

fn get(d: &[(u16, Vec<f64>)], op: u16) -> Option<&[f64]> {
    d.iter().find(|(o, _)| *o == op).map(|(_, v)| v.as_slice())
}

/// One font of a CFF program.
#[derive(Clone, Debug)]
pub struct Cff {
    data: Vec<u8>,
    charstrings: Vec<std::ops::Range<usize>>,
    gsubrs: Vec<std::ops::Range<usize>>,
    /// Local subroutines of each Font DICT (one for name-keyed fonts).
    lsubrs: Vec<Vec<std::ops::Range<usize>>>,
    /// Nominal / default widths per Font DICT.
    widths: Vec<(f64, f64)>,
    /// Glyph → Font DICT (CID-keyed fonts).
    fdselect: Vec<u8>,
    /// Glyph → SID (name-keyed) or CID.
    charset: Vec<u16>,
    strings: Vec<std::ops::Range<usize>>,
    pub cid: bool,
    pub font_matrix: [f64; 6],
    /// The built-in encoding: code → glyph.
    pub encoding: [u16; 256],
}

/// The first font name in a CFF program's Name INDEX.
pub fn font_name(data: &[u8]) -> Option<String> {
    let hdr = *data.get(2)? as usize;
    let (names, _) = index(data, hdr)?;
    let r = names.first()?.clone();
    Some(String::from_utf8_lossy(data.get(r)?).into_owned())
}

impl Cff {
    pub fn parse(data: &[u8]) -> Option<Cff> {
        let d = data;
        let hdr = *d.get(2)? as usize;
        let (_names, p) = index(d, hdr)?;
        let (tops, p) = index(d, p)?;
        let (strings, p) = index(d, p)?;
        let (gsubrs, _) = index(d, p)?;
        let top = dict(&d[tops.first()?.clone()]);
        let cs_off = get(&top, 17)?.first().copied()? as usize;
        let (charstrings, _) = index(d, cs_off)?;
        let n = charstrings.len();
        let cid = get(&top, 1230).is_some();
        let font_matrix = match get(&top, 1207) {
            Some(m) if m.len() == 6 => [m[0], m[1], m[2], m[3], m[4], m[5]],
            _ => [0.001, 0.0, 0.0, 0.001, 0.0, 0.0],
        };
        let private = |pd: &[(u16, Vec<f64>)]| -> (Vec<std::ops::Range<usize>>, (f64, f64)) {
            let Some(pv) = get(pd, 18).filter(|v| v.len() == 2) else { return (vec![], (0.0, 0.0)) };
            let (size, off) = (pv[0] as usize, pv[1] as usize);
            let Some(bytes) = off.checked_add(size).and_then(|end| d.get(off..end)) else { return (vec![], (0.0, 0.0)) };
            let pr = dict(bytes);
            let nominal = get(&pr, 21).and_then(|v| v.first().copied()).unwrap_or(0.0);
            let default = get(&pr, 20).and_then(|v| v.first().copied()).unwrap_or(0.0);
            let subrs = get(&pr, 19)
                .and_then(|v| v.first().copied())
                .and_then(|s| off.checked_add(s as usize))
                .and_then(|p| index(d, p))
                .map(|x| x.0)
                .unwrap_or_default();
            (subrs, (default, nominal))
        };
        let mut lsubrs = vec![];
        let mut widths = vec![];
        let mut fdselect = vec![];
        if cid {
            if let Some(fa) = get(&top, 1236).and_then(|v| v.first().copied())
                && let Some((fds, _)) = index(d, fa as usize)
            {
                for r in fds {
                    let (s, w) = private(&dict(&d[r]));
                    lsubrs.push(s);
                    widths.push(w);
                }
            }
            if let Some(fs) = get(&top, 1237).and_then(|v| v.first().copied()) {
                fdselect = read_fdselect(d, fs as usize, n).unwrap_or_default();
            }
        } else {
            let (s, w) = private(&top);
            lsubrs.push(s);
            widths.push(w);
        }
        if lsubrs.is_empty() {
            lsubrs.push(vec![]);
            widths.push((0.0, 0.0));
        }
        let charset = match get(&top, 15).and_then(|v| v.first().copied()).unwrap_or(0.0) as usize {
            0..=2 => (0..n as u16).collect(),
            off => read_charset(d, off, n).unwrap_or_else(|| (0..n as u16).collect()),
        };
        let mut cff = Cff { data: data.to_vec(), charstrings, gsubrs, lsubrs, widths, fdselect, charset, strings, cid, font_matrix, encoding: [0; 256] };
        let enc_off = get(&top, 16).and_then(|v| v.first().copied()).unwrap_or(0.0) as usize;
        cff.encoding = cff.read_encoding(enc_off);
        Some(cff)
    }

    fn read_encoding(&self, off: usize) -> [u16; 256] {
        let mut enc = [0u16; 256];
        if self.cid {
            return enc;
        }
        if off <= 1 {
            // Standard encoding by glyph name.
            for code in 0..=255u8 {
                if let Some(name) = crate::encoding::Base::Standard.name(code)
                    && let Some(g) = self.gid_by_name(&name)
                {
                    enc[code as usize] = g;
                }
            }
            return enc;
        }
        let d = &self.data;
        let Some(&fmt) = d.get(off) else { return enc };
        let mut p = off + 1;
        match fmt & 0x7F {
            0 => {
                let n = *d.get(p).unwrap_or(&0) as usize;
                p += 1;
                for i in 0..n {
                    if let Some(&c) = d.get(p + i) {
                        enc[c as usize] = (i + 1) as u16;
                    }
                }
                p += n;
            }
            1 => {
                let n = *d.get(p).unwrap_or(&0) as usize;
                p += 1;
                let mut g = 1u16;
                for _ in 0..n {
                    let (Some(&first), Some(&left)) = (d.get(p), d.get(p + 1)) else { break };
                    for k in 0..=left as usize {
                        if let Some(e) = enc.get_mut(first as usize + k) {
                            *e = g;
                        }
                        g += 1;
                    }
                    p += 2;
                }
            }
            _ => {}
        }
        if fmt & 0x80 != 0 {
            let n = *d.get(p).unwrap_or(&0) as usize;
            p += 1;
            for _ in 0..n {
                let (Some(&c), Some(&a), Some(&b)) = (d.get(p), d.get(p + 1), d.get(p + 2)) else { break };
                let sid = u16::from_be_bytes([a, b]);
                if let Some(g) = self.charset.iter().position(|s| *s == sid) {
                    enc[c as usize] = g as u16;
                }
                p += 3;
            }
        }
        enc
    }

    #[cfg(test)]
    pub fn glyph_count(&self) -> usize {
        self.charstrings.len()
    }

    fn sid_name(&self, sid: u16) -> Option<&str> {
        let sid = sid as usize;
        if sid < STD.len() {
            return Some(STD[sid]);
        }
        if sid < N_STD {
            return None;
        }
        let r = self.strings.get(sid - N_STD)?.clone();
        std::str::from_utf8(&self.data[r]).ok()
    }

    /// A glyph by name (name-keyed fonts).
    pub fn gid_by_name(&self, name: &str) -> Option<u16> {
        if self.cid {
            return None;
        }
        self.charset.iter().position(|&sid| self.sid_name(sid) == Some(name)).map(|g| g as u16)
    }

    /// A glyph by CID (CID-keyed fonts; name-keyed fonts take the CID as the glyph index).
    pub fn gid_by_cid(&self, cid: u16) -> Option<u16> {
        if !self.cid {
            return ((cid as usize) < self.charstrings.len()).then_some(cid);
        }
        self.charset.iter().position(|&c| c == cid).map(|g| g as u16)
    }

    /// A glyph's outline in glyph space (`font_matrix` maps it to text space) and its advance.
    pub fn outline(&self, gid: u16) -> Option<(BezPath, f64)> {
        self.outline_depth(gid, 0)
    }

    fn outline_depth(&self, gid: u16, depth: usize) -> Option<(BezPath, f64)> {
        let r = self.charstrings.get(gid as usize)?.clone();
        let fd = self.fdselect.get(gid as usize).copied().unwrap_or(0) as usize;
        let fd = fd.min(self.lsubrs.len() - 1);
        let mut st =
            T2 { cff: self, fd, path: BezPath::new(), x: 0.0, y: 0.0, stack: vec![], stems: 0, width: None, open: false, done: false, depth: 0, seac: None };
        st.run(&self.data[r]);
        if st.open {
            st.path.close_path();
        }
        let width = match st.width {
            Some(w) if !w.is_nan() => w + self.widths[fd].1,
            _ => self.widths[fd].0,
        };
        let mut path = st.path;
        // endchar seac: base and accent glyphs by standard code. The nesting is bounded like the
        // Type 1 interpreter's: a font whose accented glyphs seac into each other (or into
        // themselves) must not recurse until the stack overflows.
        if let Some((adx, ady, b, a)) = st.seac
            && depth < 2
        {
            let find = |code: u8| crate::encoding::Base::Standard.name(code).and_then(|n| self.gid_by_name(&n));
            if let Some((bp, _)) = find(b).and_then(|g| self.outline_depth(g, depth + 1)) {
                path.extend(bp);
            }
            if let Some((ap, _)) = find(a).and_then(|g| self.outline_depth(g, depth + 1)) {
                path.extend(kurbo::Affine::translate((adx, ady)) * ap);
            }
        }
        Some((path, width))
    }
}

fn read_charset(d: &[u8], off: usize, n: usize) -> Option<Vec<u16>> {
    let fmt = *d.get(off)?;
    let mut out = vec![0u16];
    let mut p = off + 1;
    let u16at = |p: usize| -> Option<u16> { Some(u16::from_be_bytes([*d.get(p)?, *d.get(p + 1)?])) };
    while out.len() < n {
        match fmt {
            0 => {
                out.push(u16at(p)?);
                p += 2;
            }
            1 | 2 => {
                let first = u16at(p)?;
                let left = if fmt == 1 { *d.get(p + 2)? as u16 } else { u16at(p + 2)? };
                p += if fmt == 1 { 3 } else { 4 };
                for k in 0..=left {
                    if out.len() >= n {
                        break;
                    }
                    out.push(first.wrapping_add(k));
                }
            }
            _ => return None,
        }
    }
    Some(out)
}

fn read_fdselect(d: &[u8], off: usize, n: usize) -> Option<Vec<u8>> {
    match *d.get(off)? {
        0 => d.get(off + 1..off + 1 + n).map(<[u8]>::to_vec),
        3 => {
            let nr = u16::from_be_bytes([*d.get(off + 1)?, *d.get(off + 2)?]) as usize;
            let mut out = vec![0u8; n];
            for i in 0..nr {
                let p = off + 3 + i * 3;
                let first = u16::from_be_bytes([*d.get(p)?, *d.get(p + 1)?]) as usize;
                let fd = *d.get(p + 2)?;
                let next = u16::from_be_bytes([*d.get(p + 3)?, *d.get(p + 4)?]) as usize;
                for g in first..next.min(n) {
                    out[g] = fd;
                }
            }
            Some(out)
        }
        _ => None,
    }
}

fn bias(n: usize) -> i64 {
    if n < 1240 {
        107
    } else if n < 33900 {
        1131
    } else {
        32768
    }
}

/// Type 2 charstring interpreter state.
struct T2<'a> {
    cff: &'a Cff,
    fd: usize,
    path: BezPath,
    x: f64,
    y: f64,
    stack: Vec<f64>,
    stems: usize,
    width: Option<f64>,
    open: bool,
    done: bool,
    depth: usize,
    seac: Option<(f64, f64, u8, u8)>,
}

impl T2<'_> {
    fn move_to(&mut self, dx: f64, dy: f64) {
        if self.open {
            self.path.close_path();
        }
        self.x += dx;
        self.y += dy;
        self.path.move_to((self.x, self.y));
        self.open = true;
    }
    /// A segment before the first moveto starts at the current point.
    fn ensure_open(&mut self) {
        if !self.open {
            self.path.move_to((self.x, self.y));
            self.open = true;
        }
    }
    fn line_to(&mut self, dx: f64, dy: f64) {
        self.ensure_open();
        self.x += dx;
        self.y += dy;
        self.path.line_to((self.x, self.y));
    }
    fn curve(&mut self, d: [f64; 6]) {
        self.ensure_open();
        let (x1, y1) = (self.x + d[0], self.y + d[1]);
        let (x2, y2) = (x1 + d[2], y1 + d[3]);
        self.x = x2 + d[4];
        self.y = y2 + d[5];
        self.path.curve_to((x1, y1), (x2, y2), (self.x, self.y));
    }
    /// The width argument before the first stack-clearing operator.
    fn take_width(&mut self, expected_even: bool) {
        if self.width.is_none() {
            let odd = self.stack.len() % 2 == 1;
            if expected_even == odd && !self.stack.is_empty() {
                self.width = Some(self.stack.remove(0));
            } else {
                self.width = Some(f64::NAN);
            }
        }
    }

    fn run(&mut self, cs: &[u8]) {
        if self.depth > 10 {
            return;
        }
        let mut i = 0;
        while i < cs.len() && !self.done {
            let b0 = cs[i];
            i += 1;
            match b0 {
                32..=246 => self.stack.push(b0 as f64 - 139.0),
                247..=250 => {
                    self.stack.push((b0 as f64 - 247.0) * 256.0 + *cs.get(i).unwrap_or(&0) as f64 + 108.0);
                    i += 1;
                }
                251..=254 => {
                    self.stack.push(-(b0 as f64 - 251.0) * 256.0 - *cs.get(i).unwrap_or(&0) as f64 - 108.0);
                    i += 1;
                }
                28 => {
                    self.stack.push(i16::from_be_bytes([*cs.get(i).unwrap_or(&0), *cs.get(i + 1).unwrap_or(&0)]) as f64);
                    i += 2;
                }
                255 => {
                    let b = |k: usize| *cs.get(i + k).unwrap_or(&0);
                    self.stack.push(i32::from_be_bytes([b(0), b(1), b(2), b(3)]) as f64 / 65536.0);
                    i += 4;
                }
                1 | 3 | 18 | 23 => {
                    self.take_width(true);
                    self.stems += self.stack.len() / 2;
                    self.stack.clear();
                }
                19 | 20 => {
                    self.take_width(true);
                    self.stems += self.stack.len() / 2;
                    self.stack.clear();
                    i += self.stems.div_ceil(8);
                }
                21 => {
                    self.take_width(true);
                    let s = self.args();
                    let n = s.len();
                    if n >= 2 {
                        self.move_to(s[n - 2], s[n - 1]);
                    }
                }
                22 | 4 => {
                    self.take_width(false);
                    let s = self.args();
                    let v = s.last().copied().unwrap_or(0.0);
                    if b0 == 22 { self.move_to(v, 0.0) } else { self.move_to(0.0, v) }
                }
                5 => {
                    let s = self.args();
                    for c in s.as_chunks::<2>().0 {
                        self.line_to(c[0], c[1]);
                    }
                }
                6 | 7 => {
                    let s = self.args();
                    let mut horiz = b0 == 6;
                    for v in s {
                        if horiz {
                            self.line_to(v, 0.0)
                        } else {
                            self.line_to(0.0, v)
                        }
                        horiz = !horiz;
                    }
                }
                8 => {
                    let s = self.args();
                    for c in s.as_chunks::<6>().0 {
                        self.curve([c[0], c[1], c[2], c[3], c[4], c[5]]);
                    }
                }
                24 => {
                    // rcurveline
                    let s = self.args();
                    let nc = s.len().saturating_sub(2) / 6;
                    for c in s[..nc * 6].as_chunks::<6>().0 {
                        self.curve([c[0], c[1], c[2], c[3], c[4], c[5]]);
                    }
                    if s.len() >= nc * 6 + 2 {
                        self.line_to(s[nc * 6], s[nc * 6 + 1]);
                    }
                }
                25 => {
                    // rlinecurve
                    let s = self.args();
                    if s.len() >= 6 {
                        let nl = (s.len() - 6) / 2;
                        for c in s[..nl * 2].as_chunks::<2>().0 {
                            self.line_to(c[0], c[1]);
                        }
                        let c = &s[nl * 2..];
                        self.curve([c[0], c[1], c[2], c[3], c[4], c[5]]);
                    }
                }
                26 => {
                    // vvcurveto
                    let mut s = self.args();
                    let mut dx1 = 0.0;
                    if s.len() % 4 == 1 {
                        dx1 = s.remove(0);
                    }
                    for c in s.as_chunks::<4>().0 {
                        self.curve([dx1, c[0], c[1], c[2], 0.0, c[3]]);
                        dx1 = 0.0;
                    }
                }
                27 => {
                    // hhcurveto
                    let mut s = self.args();
                    let mut dy1 = 0.0;
                    if s.len() % 4 == 1 {
                        dy1 = s.remove(0);
                    }
                    for c in s.as_chunks::<4>().0 {
                        self.curve([c[0], dy1, c[1], c[2], c[3], 0.0]);
                        dy1 = 0.0;
                    }
                }
                30 | 31 => {
                    // vhcurveto / hvcurveto: alternating tangents, optional final delta.
                    let s = self.args();
                    let mut horiz = b0 == 31;
                    let mut k = 0;
                    while k + 4 <= s.len() {
                        let last = k + 4 == s.len() - 1;
                        let extra = if last { s[k + 4] } else { 0.0 };
                        if horiz {
                            self.curve([s[k], 0.0, s[k + 1], s[k + 2], extra, s[k + 3]]);
                        } else {
                            self.curve([0.0, s[k], s[k + 1], s[k + 2], s[k + 3], extra]);
                        }
                        horiz = !horiz;
                        k += 4;
                        if last {
                            break;
                        }
                    }
                }
                10 | 29 => {
                    let Some(idx) = self.stack.pop() else { continue };
                    let subrs = if b0 == 10 { &self.cff.lsubrs[self.fd] } else { &self.cff.gsubrs };
                    let k = idx as i64 + bias(subrs.len());
                    if let Some(r) = usize::try_from(k).ok().and_then(|k| subrs.get(k)).cloned() {
                        self.depth += 1;
                        let data = &self.cff.data[r];
                        self.run(data);
                        self.depth -= 1;
                    }
                }
                11 => return,
                14 => {
                    // A width precedes the operands when the count is odd (5 with seac, 1 alone);
                    // the no-width counts (0 and 4) are even, like the stems' and rmoveto's.
                    self.take_width(true);
                    if self.stack.len() >= 4 {
                        let n = self.stack.len();
                        let s = &self.stack[n - 4..];
                        self.seac = Some((s[0], s[1], s[2] as u8, s[3] as u8));
                    }
                    self.stack.clear();
                    self.done = true;
                }
                12 => {
                    let b1 = *cs.get(i).unwrap_or(&0);
                    i += 1;
                    self.escape(b1);
                }
                _ => self.stack.clear(),
            }
        }
    }

    fn args(&mut self) -> Vec<f64> {
        std::mem::take(&mut self.stack)
    }

    fn escape(&mut self, b1: u8) {
        let pop = |s: &mut Vec<f64>| s.pop().unwrap_or(0.0);
        match b1 {
            35 => {
                // flex: two curves.
                let s = self.args();
                if s.len() >= 12 {
                    self.curve([s[0], s[1], s[2], s[3], s[4], s[5]]);
                    self.curve([s[6], s[7], s[8], s[9], s[10], s[11]]);
                }
            }
            34 => {
                // hflex: dx1 dx2 dy2 dx3 dx4 dx5 dx6.
                let s = self.args();
                if s.len() >= 7 {
                    self.curve([s[0], 0.0, s[1], s[2], s[3], 0.0]);
                    self.curve([s[4], 0.0, s[5], -s[2], s[6], 0.0]);
                }
            }
            36 => {
                // hflex1: dx1 dy1 dx2 dy2 dx3 dx4 dx5 dy5 dx6.
                let s = self.args();
                if s.len() >= 9 {
                    self.curve([s[0], s[1], s[2], s[3], s[4], 0.0]);
                    self.curve([s[5], 0.0, s[6], s[7], s[8], -(s[1] + s[3] + s[7])]);
                }
            }
            37 => {
                // flex1
                let s = self.args();
                if s.len() >= 11 {
                    let (x0, y0) = (self.x, self.y);
                    let dx: f64 = s[0] + s[2] + s[4] + s[6] + s[8];
                    let dy: f64 = s[1] + s[3] + s[5] + s[7] + s[9];
                    self.curve([s[0], s[1], s[2], s[3], s[4], s[5]]);
                    let (lx, ly) = if dx.abs() > dy.abs() { (s[10], y0 - (self.y + s[7] + s[9])) } else { (x0 - (self.x + s[6] + s[8]), s[10]) };
                    self.curve([s[6], s[7], s[8], s[9], lx, ly]);
                }
            }
            9 => {
                let a = pop(&mut self.stack);
                self.stack.push(a.abs());
            }
            10 => {
                let (b, a) = (pop(&mut self.stack), pop(&mut self.stack));
                self.stack.push(a + b);
            }
            11 => {
                let (b, a) = (pop(&mut self.stack), pop(&mut self.stack));
                self.stack.push(a - b);
            }
            12 => {
                let (b, a) = (pop(&mut self.stack), pop(&mut self.stack));
                self.stack.push(if b != 0.0 { a / b } else { 0.0 });
            }
            14 => {
                let a = pop(&mut self.stack);
                self.stack.push(-a);
            }
            18 => {
                self.stack.pop();
            }
            24 => {
                let (b, a) = (pop(&mut self.stack), pop(&mut self.stack));
                self.stack.push(a * b);
            }
            27 => {
                let a = pop(&mut self.stack);
                self.stack.push(a);
                self.stack.push(a);
            }
            28 => {
                let (b, a) = (pop(&mut self.stack), pop(&mut self.stack));
                self.stack.push(b);
                self.stack.push(a);
            }
            _ => self.stack.clear(),
        }
    }
}

/// A minimal CFF writer for tests: one name-keyed font whose glyphs are Type 2 charstrings,
/// named by custom strings.
#[cfg(test)]
pub(crate) fn write_test_cff(glyphs: &[(&str, Vec<u8>)]) -> Vec<u8> {
    fn idx(items: &[Vec<u8>]) -> Vec<u8> {
        let mut out = (items.len() as u16).to_be_bytes().to_vec();
        if items.is_empty() {
            return out;
        }
        out.push(4);
        let mut off = 1u32;
        out.extend_from_slice(&off.to_be_bytes());
        for it in items {
            off += it.len() as u32;
            out.extend_from_slice(&off.to_be_bytes());
        }
        for it in items {
            out.extend_from_slice(it);
        }
        out
    }
    let int = |v: i32| {
        let mut o = vec![29];
        o.extend_from_slice(&v.to_be_bytes());
        o
    };
    let names = idx(&[b"Test".to_vec()]);
    let strings: Vec<Vec<u8>> = glyphs.iter().skip(1).map(|(n, _)| n.as_bytes().to_vec()).collect();
    let strings_idx = idx(&strings);
    let gsubrs = idx(&[]);
    let cs = idx(&glyphs.iter().map(|(_, c)| c.clone()).collect::<Vec<_>>());
    // charset format 0: SIDs of glyphs 1.. (custom strings from 391).
    let mut charset = vec![0u8];
    for i in 0..glyphs.len() - 1 {
        charset.extend_from_slice(&((N_STD + i) as u16).to_be_bytes());
    }
    // Top DICT with fixed-size operands so offsets can be computed first.
    let top_len = 4 * 5 + 3;
    let header = [1u8, 0, 4, 4];
    let top_index_len = 2 + 1 + 8 + top_len;
    let base = header.len() + names.len() + top_index_len + strings_idx.len() + gsubrs.len();
    let charset_off = base;
    let cs_off = charset_off + charset.len();
    let priv_off = cs_off + cs.len();
    let mut top = vec![];
    top.extend(int(charset_off as i32));
    top.push(15);
    top.extend(int(cs_off as i32));
    top.push(17);
    top.extend(int(0));
    top.extend(int(priv_off as i32));
    top.push(18);
    assert_eq!(top.len(), top_len);
    let mut out = header.to_vec();
    out.extend(names);
    out.extend(idx(&[top]));
    out.extend(strings_idx);
    out.extend(gsubrs);
    out.extend(charset);
    out.extend(cs);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use kurbo::Shape;

    #[test]
    fn standard_strings_cover_isoadobe() {
        assert_eq!(STD.len(), 229);
        assert_eq!(STD[34], "A");
        assert_eq!(STD[66], "a");
        assert_eq!(STD[149], "germandbls");
        assert_eq!(STD[228], "zcaron");
    }

    #[test]
    fn type2_charstrings_draw_outlines() {
        // A 500-unit square with a width: `w dx dy rmoveto dx hlineto dy vlineto dx hlineto endchar`.
        let enc = |v: i32| -> Vec<u8> {
            if (-107..=107).contains(&v) {
                vec![(v + 139) as u8]
            } else {
                let mut o = vec![28];
                o.extend_from_slice(&(v as i16).to_be_bytes());
                o
            }
        };
        let mut square = vec![];
        for v in [600, 50, 0] {
            square.extend(enc(v));
        }
        square.push(21);
        for v in [500, 500, -500] {
            square.extend(enc(v));
        }
        square.push(6);
        square.push(14);
        let font = write_test_cff(&[(".notdef", vec![14]), ("square", square)]);
        let cff = Cff::parse(&font).unwrap();
        assert_eq!(cff.glyph_count(), 2);
        let g = cff.gid_by_name("square").unwrap();
        assert_eq!(g, 1);
        let (p, w) = cff.outline(g).unwrap();
        assert_eq!(w, 600.0);
        assert!((p.area().abs() - 250_000.0).abs() < 1e-6, "{}", p.area());
        assert_eq!(p.bounding_box(), kurbo::Rect::new(50.0, 0.0, 550.0, 500.0));
    }

    /// An `endchar` seac whose base and accent name its own glyph used to recurse until the
    /// stack overflowed and aborted the app; the depth limit cuts it off like the Type 1
    /// interpreter's.
    #[test]
    fn seac_cycles_are_cut_off_not_a_stack_overflow() {
        let enc = |v: i32| -> Vec<u8> {
            if (-107..=107).contains(&v) {
                vec![(v + 139) as u8]
            } else {
                let mut o = vec![28];
                o.extend_from_slice(&(v as i16).to_be_bytes());
                o
            }
        };
        // `w adx ady bchar achar endchar`: both components are 'A' (standard code 65), this glyph.
        let mut cs = vec![];
        for v in [500, 0, 0, 65, 65] {
            cs.extend(enc(v));
        }
        cs.push(14);
        let font = write_test_cff(&[(".notdef", vec![14]), ("A", cs)]);
        let cff = Cff::parse(&font).unwrap();
        let (p, _) = cff.outline(cff.gid_by_name("A").unwrap()).unwrap();
        assert!(p.elements().is_empty());
    }

    /// A CFF whose top DICT points its Private DICT at offsets near `usize::MAX`: `size` and `off`
    /// come from the file, and reading the Private DICT must not overflow `off + size`.
    #[test]
    fn huge_private_dict_offsets_do_not_overflow() {
        fn idx(items: &[Vec<u8>]) -> Vec<u8> {
            let mut out = (items.len() as u16).to_be_bytes().to_vec();
            if items.is_empty() {
                return out;
            }
            out.push(4);
            let mut off = 1u32;
            out.extend_from_slice(&off.to_be_bytes());
            for it in items {
                off += it.len() as u32;
                out.extend_from_slice(&off.to_be_bytes());
            }
            for it in items {
                out.extend_from_slice(it);
            }
            out
        }
        let int = |v: i32| {
            let mut o = vec![29];
            o.extend_from_slice(&v.to_be_bytes());
            o
        };
        // The BCD real "1E300": a size and an offset that both saturate at `usize::MAX`.
        let huge = vec![0x1E, 0x1B, 0x30, 0x0F];
        let header = [1u8, 0, 4, 4];
        let names = idx(&[b"Test".to_vec()]);
        let strings = idx(&[]);
        let gsubrs = idx(&[]);
        let charset = vec![0u8];
        let cs = idx(&[vec![14]]); // one `.notdef` charstring: endchar
        let top_len = 5 + 1 + 5 + 1 + huge.len() * 2 + 1;
        let top_index_len = 2 + 1 + 8 + top_len;
        let base = header.len() + names.len() + top_index_len + strings.len() + gsubrs.len();
        let cs_off = base + charset.len();
        let mut top = vec![];
        top.extend(int(0));
        top.push(15); // charset: ISOAdobe
        top.extend(int(cs_off as i32));
        top.push(17); // CharStrings
        top.extend(&huge);
        top.extend(&huge);
        top.push(18); // Private: [size, offset]
        assert_eq!(top.len(), top_len);
        let mut out = header.to_vec();
        out.extend(names);
        out.extend(idx(&[top]));
        out.extend(strings);
        out.extend(gsubrs);
        out.extend(charset);
        out.extend(cs);
        let cff = Cff::parse(&out).expect("the damaged Private DICT is skipped, the font parses");
        assert_eq!(cff.glyph_count(), 1);
    }

    /// A four-operand `endchar` (`adx ady bchar achar`, no width) is a seac like the Type 1
    /// interpreter's: `endchar` must not eat `adx` as a width, and a five-operand one reads its
    /// width first.
    #[test]
    fn endchar_reads_its_width_by_operand_count() {
        let enc = |v: i32| -> Vec<u8> {
            if (-107..=107).contains(&v) {
                vec![(v + 139) as u8]
            } else {
                let mut o = vec![28];
                o.extend_from_slice(&(v as i16).to_be_bytes());
                o
            }
        };
        // The base glyph: a square drawn without a width (`500 500 -500 hlineto endchar`).
        let mut square = vec![];
        for v in [500, 500, -500] {
            square.extend(enc(v));
        }
        square.extend([6, 14]);
        // Four operands: `adx ady bchar achar endchar`, no width to take.
        let mut seac = vec![];
        for v in [10, 20, 65, 65] {
            seac.extend(enc(v));
        }
        seac.push(14);
        let font = write_test_cff(&[(".notdef", vec![14]), ("A", square.clone()), ("Aacute", seac)]);
        let cff = Cff::parse(&font).unwrap();
        let (p, _) = cff.outline(cff.gid_by_name("Aacute").unwrap()).unwrap();
        assert!(!p.elements().is_empty(), "the base glyph of the seac is part of the outline");
        // Five operands: the first is the width, the last four the seac.
        let mut wide = vec![];
        for v in [600, 0, 0, 65, 65] {
            wide.extend(enc(v));
        }
        wide.push(14);
        let font = write_test_cff(&[(".notdef", vec![14]), ("A", square), ("Awide", wide)]);
        let cff = Cff::parse(&font).unwrap();
        let (_, w) = cff.outline(cff.gid_by_name("Awide").unwrap()).unwrap();
        assert_eq!(w, 600.0);
    }
}

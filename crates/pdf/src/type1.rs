//! Type 1 font programs (`FontFile`), from Adobe's published *Adobe Type 1 Font Format*:
//! the cleartext part (`/FontMatrix`, the built-in `/Encoding`), `eexec` decryption, `/Subrs`
//! and `/CharStrings`, and a Type 1 charstring interpreter (including flex and hint
//! replacement through `callothersubr`, and `seac` accented characters).

use std::collections::HashMap;

use kurbo::BezPath;

const EEXEC_R: u16 = 55665;
const CHARSTRING_R: u16 = 4330;

fn decrypt(data: &[u8], mut r: u16, skip: usize) -> Vec<u8> {
    const C1: u16 = 52845;
    const C2: u16 = 22719;
    let mut out = Vec::with_capacity(data.len());
    for &c in data {
        out.push(c ^ (r >> 8) as u8);
        r = (c as u16).wrapping_add(r).wrapping_mul(C1).wrapping_add(C2);
    }
    out.drain(..skip.min(out.len()));
    out
}

/// Encrypt (for test fonts): the inverse of [`decrypt`] with `skip` leading zero bytes.
#[cfg(test)]
pub(crate) fn encrypt(plain: &[u8], mut r: u16, skip: usize) -> Vec<u8> {
    const C1: u16 = 52845;
    const C2: u16 = 22719;
    let mut out = vec![];
    for &p in std::iter::repeat_n(&0u8, skip).chain(plain.iter()) {
        let c = p ^ (r >> 8) as u8;
        r = (c as u16).wrapping_add(r).wrapping_mul(C1).wrapping_add(C2);
        out.push(c);
    }
    out
}

/// A parsed Type 1 font.
#[derive(Clone, Debug, Default)]
pub struct Type1 {
    pub font_matrix: Option<[f64; 6]>,
    /// The built-in encoding (code → glyph name), when the font has one.
    pub encoding: HashMap<u8, String>,
    subrs: Vec<Vec<u8>>,
    charstrings: HashMap<String, Vec<u8>>,
}

fn find(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if from >= hay.len() {
        return None;
    }
    hay[from..].windows(needle.len()).position(|w| w == needle).map(|p| p + from)
}

/// Strip PFB segment headers (`0x80 type len32le`).
fn unpfb(data: &[u8]) -> Vec<u8> {
    if data.first() != Some(&0x80) {
        return data.to_vec();
    }
    let mut out = vec![];
    let mut p = 0;
    while p + 6 <= data.len() && data[p] == 0x80 && data[p + 1] != 3 {
        let n = u32::from_le_bytes([data[p + 2], data[p + 3], data[p + 4], data[p + 5]]) as usize;
        let end = (p + 6 + n).min(data.len());
        out.extend_from_slice(&data[p + 6..end]);
        p = end;
    }
    out
}

fn tokens(s: &[u8]) -> impl Iterator<Item = &[u8]> {
    s.split(|c| c.is_ascii_whitespace() || matches!(c, b'[' | b']' | b'{' | b'}')).filter(|t| !t.is_empty())
}

impl Type1 {
    pub fn parse(data: &[u8]) -> Option<Type1> {
        let data = unpfb(data);
        let mut f = Type1::default();
        let ee = find(&data, b"eexec", 0)?;
        let clear = &data[..ee];
        // /FontMatrix [a b c d e f]
        if let Some(p) = find(clear, b"/FontMatrix", 0) {
            let end = find(clear, b"]", p).unwrap_or(clear.len());
            let v: Vec<f64> = tokens(&clear[p + 11..end]).filter_map(|t| std::str::from_utf8(t).ok()?.parse().ok()).collect();
            if v.len() == 6 {
                f.font_matrix = Some([v[0], v[1], v[2], v[3], v[4], v[5]]);
            }
        }
        // /Encoding: `dup code /name put` entries (StandardEncoding otherwise).
        if let Some(p) = find(clear, b"/Encoding", 0) {
            let mut toks = tokens(&clear[p + 9..]).peekable();
            let mut last: Vec<&[u8]> = vec![];
            for t in toks.by_ref() {
                if t == b"readonly" || t == b"def" {
                    break;
                }
                if t == b"put" && last.len() >= 3 && last[last.len() - 3] == b"dup" {
                    let code = std::str::from_utf8(last[last.len() - 2]).ok().and_then(|c| c.parse::<u8>().ok());
                    let name = last[last.len() - 1].strip_prefix(b"/");
                    if let (Some(c), Some(n)) = (code, name) {
                        f.encoding.insert(c, String::from_utf8_lossy(n).into_owned());
                    }
                }
                last.push(t);
                if last.len() > 4 {
                    last.remove(0);
                }
            }
        }
        // The encrypted part: binary or hexadecimal.
        let mut body = &data[ee + 5..];
        while body.first().is_some_and(|c| c.is_ascii_whitespace()) {
            body = &body[1..];
        }
        let hex = body.iter().take(4).all(u8::is_ascii_hexdigit);
        let raw: Vec<u8> = if hex {
            let digits: Vec<u8> = body.iter().copied().filter(u8::is_ascii_hexdigit).collect();
            digits.as_chunks::<2>().0.iter().map(|p| (hexv(p[0]) << 4) | hexv(p[1])).collect()
        } else {
            body.to_vec()
        };
        let priv_ = decrypt(&raw, EEXEC_R, 4);
        let len_iv =
            find(&priv_, b"/lenIV", 0).and_then(|p| tokens(&priv_[p + 6..]).next()).and_then(|t| std::str::from_utf8(t).ok()?.parse::<i64>().ok()).unwrap_or(4);
        let cs = |bytes: &[u8]| if len_iv < 0 { bytes.to_vec() } else { decrypt(bytes, CHARSTRING_R, len_iv as usize) };
        // Binary items: `<n> RD <n bytes>` (RD may be spelt `-|`).
        let read_binary = |p: usize| -> Option<(Vec<u8>, usize)> {
            // p points at the length token.
            let mut q = p;
            while q < priv_.len() && priv_[q].is_ascii_whitespace() {
                q += 1;
            }
            let s = q;
            while q < priv_.len() && priv_[q].is_ascii_digit() {
                q += 1;
            }
            let n: usize = std::str::from_utf8(&priv_[s..q]).ok()?.parse().ok()?;
            while q < priv_.len() && priv_[q].is_ascii_whitespace() {
                q += 1;
            }
            while q < priv_.len() && !priv_[q].is_ascii_whitespace() {
                q += 1;
            }
            q += 1;
            let bytes = priv_.get(q..q.checked_add(n)?)?;
            Some((cs(bytes), q + n))
        };
        if let Some(p) = find(&priv_, b"/Subrs", 0) {
            let mut q = p + 6;
            while let Some(d) = find(&priv_, b"dup", q) {
                let lim = find(&priv_, b"/CharStrings", q).unwrap_or(priv_.len());
                if d > lim {
                    break;
                }
                let mut r = d + 3;
                while r < priv_.len() && priv_[r].is_ascii_whitespace() {
                    r += 1;
                }
                let s = r;
                while r < priv_.len() && priv_[r].is_ascii_digit() {
                    r += 1;
                }
                let Some(i) = std::str::from_utf8(&priv_[s..r]).ok().and_then(|x| x.parse::<usize>().ok()) else { break };
                let Some((bytes, end)) = read_binary(r) else { break };
                if i < 65536 {
                    if f.subrs.len() <= i {
                        f.subrs.resize(i + 1, vec![]);
                    }
                    f.subrs[i] = bytes;
                }
                q = end;
            }
        }
        let cs_at = find(&priv_, b"/CharStrings", 0)?;
        let mut q = cs_at + 12;
        // Skip `n dict dup begin`.
        q = find(&priv_, b"begin", q).map(|b| b + 5).unwrap_or(q);
        while let Some(slash) = priv_[q.min(priv_.len())..].iter().position(|&c| c == b'/').map(|x| x + q) {
            let mut e = slash + 1;
            while e < priv_.len() && !priv_[e].is_ascii_whitespace() && !matches!(priv_[e], b'/' | b'[' | b'{') {
                e += 1;
            }
            let name = String::from_utf8_lossy(&priv_[slash + 1..e]).into_owned();
            let Some((bytes, end)) = read_binary(e) else { break };
            f.charstrings.insert(name, bytes);
            q = end;
            // Stop at the end of the CharStrings dictionary.
            let mut r = q;
            while r < priv_.len() && !priv_[r].is_ascii_whitespace() {
                r += 1;
            }
            let rest = &priv_[r.min(priv_.len())..];
            let next_tok = tokens(rest).next().unwrap_or(b"");
            if next_tok == b"end" {
                break;
            }
        }
        (!f.charstrings.is_empty()).then_some(f)
    }

    pub fn has_glyph(&self, name: &str) -> bool {
        self.charstrings.contains_key(name)
    }

    /// A glyph's outline in glyph space and its advance width.
    pub fn outline(&self, name: &str) -> Option<(BezPath, f64)> {
        self.outline_depth(name, 0)
    }

    fn outline_depth(&self, name: &str, depth: usize) -> Option<(BezPath, f64)> {
        let cs = self.charstrings.get(name)?;
        let mut st = T1 {
            font: self,
            path: BezPath::new(),
            x: 0.0,
            y: 0.0,
            stack: vec![],
            ps: vec![],
            width: 0.0,
            sbx: 0.0,
            open: false,
            flex: None,
            done: false,
            depth: 0,
            seac: None,
        };
        st.run(cs);
        if st.open {
            st.path.close_path();
        }
        let mut path = st.path;
        if let Some((asb, adx, ady, b, a)) = st.seac
            && depth < 2
        {
            let name_of = |c: u8| crate::encoding::Base::Standard.name(c);
            if let Some((bp, _)) = name_of(b).and_then(|n| self.outline_depth(&n, depth + 1)) {
                path.extend(bp);
            }
            if let Some((ap, _)) = name_of(a).and_then(|n| self.outline_depth(&n, depth + 1)) {
                // The accent's origin: its own sidebearing is replaced by `asb`.
                path.extend(kurbo::Affine::translate((adx - asb + st.sbx, ady)) * ap);
            }
        }
        Some((path, st.width))
    }
}

fn hexv(c: u8) -> u8 {
    match c {
        b'0'..=b'9' => c - b'0',
        b'a'..=b'f' => c - b'a' + 10,
        b'A'..=b'F' => c - b'A' + 10,
        _ => 0,
    }
}

struct T1<'a> {
    font: &'a Type1,
    path: BezPath,
    x: f64,
    y: f64,
    stack: Vec<f64>,
    /// The PostScript operand stack shared with `callothersubr` / `pop`.
    ps: Vec<f64>,
    width: f64,
    sbx: f64,
    open: bool,
    /// Flex points being collected (othersubr 1 … 0).
    flex: Option<Vec<(f64, f64)>>,
    done: bool,
    depth: usize,
    seac: Option<(f64, f64, f64, u8, u8)>,
}

impl T1<'_> {
    fn rmove(&mut self, dx: f64, dy: f64) {
        self.x += dx;
        self.y += dy;
        if let Some(f) = &mut self.flex {
            f.push((self.x, self.y));
            return;
        }
        if self.open {
            self.path.close_path();
        }
        self.path.move_to((self.x, self.y));
        self.open = true;
    }
    fn rline(&mut self, dx: f64, dy: f64) {
        if !self.open {
            self.path.move_to((self.x, self.y));
            self.open = true;
        }
        self.x += dx;
        self.y += dy;
        self.path.line_to((self.x, self.y));
    }
    fn rcurve(&mut self, d: [f64; 6]) {
        if !self.open {
            self.path.move_to((self.x, self.y));
            self.open = true;
        }
        let (x1, y1) = (self.x + d[0], self.y + d[1]);
        let (x2, y2) = (x1 + d[2], y1 + d[3]);
        self.x = x2 + d[4];
        self.y = y2 + d[5];
        self.path.curve_to((x1, y1), (x2, y2), (self.x, self.y));
    }

    fn run(&mut self, cs: &[u8]) {
        if self.depth > 10 {
            return;
        }
        let mut i = 0;
        while i < cs.len() && !self.done {
            let b0 = cs[i];
            i += 1;
            let s = |st: &Self, k: usize| st.stack.get(k).copied().unwrap_or(0.0);
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
                255 => {
                    let b = |k: usize| *cs.get(i + k).unwrap_or(&0);
                    self.stack.push(i32::from_be_bytes([b(0), b(1), b(2), b(3)]) as f64);
                    i += 4;
                }
                13 => {
                    // hsbw: sbx wx
                    self.sbx = s(self, 0);
                    self.x = self.sbx;
                    self.y = 0.0;
                    self.width = s(self, 1);
                    self.stack.clear();
                }
                21 => {
                    let (a, b) = (s(self, 0), s(self, 1));
                    self.rmove(a, b);
                    self.stack.clear();
                }
                22 => {
                    let a = s(self, 0);
                    self.rmove(a, 0.0);
                    self.stack.clear();
                }
                4 => {
                    let a = s(self, 0);
                    self.rmove(0.0, a);
                    self.stack.clear();
                }
                5 => {
                    let (a, b) = (s(self, 0), s(self, 1));
                    self.rline(a, b);
                    self.stack.clear();
                }
                6 => {
                    let a = s(self, 0);
                    self.rline(a, 0.0);
                    self.stack.clear();
                }
                7 => {
                    let a = s(self, 0);
                    self.rline(0.0, a);
                    self.stack.clear();
                }
                8 => {
                    let d = [s(self, 0), s(self, 1), s(self, 2), s(self, 3), s(self, 4), s(self, 5)];
                    self.rcurve(d);
                    self.stack.clear();
                }
                30 => {
                    // vhcurveto: dy1 dx2 dy2 dx3
                    let d = [0.0, s(self, 0), s(self, 1), s(self, 2), s(self, 3), 0.0];
                    self.rcurve(d);
                    self.stack.clear();
                }
                31 => {
                    // hvcurveto: dx1 dx2 dy2 dy3
                    let d = [s(self, 0), 0.0, s(self, 1), s(self, 2), 0.0, s(self, 3)];
                    self.rcurve(d);
                    self.stack.clear();
                }
                9 => {
                    if self.open {
                        self.path.close_path();
                        self.open = false;
                    }
                    self.stack.clear();
                }
                10 => {
                    let Some(n) = self.stack.pop() else { continue };
                    if let Some(sub) = self.font.subrs.get(n.max(0.0) as usize) {
                        self.depth += 1;
                        let sub = sub.clone();
                        self.run(&sub);
                        self.depth -= 1;
                    }
                }
                11 => return,
                14 => {
                    self.done = true;
                    self.stack.clear();
                }
                1 | 3 => self.stack.clear(),
                12 => {
                    let b1 = *cs.get(i).unwrap_or(&0);
                    i += 1;
                    self.escape(b1);
                }
                _ => self.stack.clear(),
            }
        }
    }

    fn escape(&mut self, b1: u8) {
        match b1 {
            6 => {
                // seac: asb adx ady bchar achar
                let v = |k: usize| self.stack.get(k).copied().unwrap_or(0.0);
                self.seac = Some((v(0), v(1), v(2), v(3) as u8, v(4) as u8));
                self.done = true;
                self.stack.clear();
            }
            7 => {
                // sbw: sbx sby wx wy
                let v = |k: usize| self.stack.get(k).copied().unwrap_or(0.0);
                self.sbx = v(0);
                self.x = v(0);
                self.y = v(1);
                self.width = v(2);
                self.stack.clear();
            }
            12 => {
                let b = self.stack.pop().unwrap_or(1.0);
                let a = self.stack.pop().unwrap_or(0.0);
                self.stack.push(if b != 0.0 { a / b } else { 0.0 });
            }
            16 => {
                // callothersubr: args… n othersubr#
                let other = self.stack.pop().unwrap_or(0.0) as i32;
                let n = self.stack.pop().unwrap_or(0.0).max(0.0) as usize;
                let k = self.stack.len().saturating_sub(n);
                let args: Vec<f64> = self.stack.drain(k..).collect();
                match other {
                    1 => self.flex = Some(vec![]),
                    2 => {}
                    0 => {
                        // End of flex: seven points (a reference point, then two curves).
                        let pts = self.flex.take().unwrap_or_default();
                        if pts.len() >= 7 {
                            if !self.open {
                                self.path.move_to((self.x, self.y));
                                self.open = true;
                            }
                            self.path.curve_to(pts[1], pts[2], pts[3]);
                            self.path.curve_to(pts[4], pts[5], pts[6]);
                            self.x = pts[6].0;
                            self.y = pts[6].1;
                        }
                        // `pop pop setcurrentpoint` follows.
                        self.ps = vec![self.y, self.x];
                    }
                    _ => {
                        // Hint replacement and others: hand the arguments back to `pop`.
                        self.ps = args.into_iter().rev().collect();
                    }
                }
            }
            17 => {
                let v = self.ps.pop().unwrap_or(0.0);
                self.stack.push(v);
            }
            33 => {
                // setcurrentpoint
                let v = |k: usize| self.stack.get(k).copied().unwrap_or(0.0);
                self.x = v(0);
                self.y = v(1);
                self.stack.clear();
            }
            _ => self.stack.clear(),
        }
    }
}

/// A minimal Type 1 font for tests: `glyphs` are (name, plain charstring bytes).
#[cfg(test)]
pub(crate) fn write_test_type1(glyphs: &[(&str, Vec<u8>)], encoding: &[(u8, &str)]) -> Vec<u8> {
    let mut clear = b"%!PS-AdobeFont-1.0: Test 001\n/FontMatrix [0.001 0 0 0.001 0 0] readonly def\n/Encoding 256 array\n".to_vec();
    for (c, n) in encoding {
        clear.extend_from_slice(format!("dup {c} /{n} put\n").as_bytes());
    }
    clear.extend_from_slice(b"readonly def\ncurrentfile eexec\n");
    let mut priv_ =
        b"dup /Private 8 dict dup begin /RD{string currentfile exch readstring pop}executeonly def\n/lenIV 4 def\n/Subrs 0 array\nND\n2 index /CharStrings "
            .to_vec();
    priv_.extend_from_slice(format!("{} dict dup begin\n", glyphs.len()).as_bytes());
    for (n, cs) in glyphs {
        let enc = encrypt(cs, CHARSTRING_R, 4);
        priv_.extend_from_slice(format!("/{n} {} RD ", enc.len()).as_bytes());
        priv_.extend_from_slice(&enc);
        priv_.extend_from_slice(b" ND\n");
    }
    priv_.extend_from_slice(b"end\nend\nmark currentfile closefile\n");
    let mut out = clear;
    out.extend(encrypt(&priv_, EEXEC_R, 4));
    out.extend_from_slice(b"\n0000000000000000\ncleartomark\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use kurbo::Shape;

    #[test]
    fn type1_decrypts_and_draws() {
        // hsbw 50 600; rmoveto 0 0; 500 hlineto 500 vlineto -500 hlineto closepath endchar
        let cs = vec![50 + 139, 248, 236, 13, 139, 139, 21, 248, 136, 6, 248, 136, 7, 252, 136, 6, 9, 14];
        let font = write_test_type1(&[(".notdef", vec![139, 139, 13, 14]), ("box", cs)], &[(65, "box")]);
        let t = Type1::parse(&font).unwrap();
        assert_eq!(t.font_matrix, Some([0.001, 0.0, 0.0, 0.001, 0.0, 0.0]));
        assert_eq!(t.encoding.get(&65).map(String::as_str), Some("box"));
        let (p, w) = t.outline("box").unwrap();
        assert_eq!(w, 600.0);
        assert!((p.area().abs() - 250_000.0).abs() < 1e-6);
        assert_eq!(p.bounding_box(), kurbo::Rect::new(50.0, 0.0, 550.0, 500.0));
    }

    /// A `/Subrs` entry whose `RD` length is `usize::MAX`: the bytes are not in the file, and
    /// `q + n` must not overflow.
    #[test]
    fn huge_rd_lengths_do_not_overflow() {
        let mut out = b"%!PS-AdobeFont-1.0: Test 001\n/FontMatrix [0.001 0 0 0.001 0 0] readonly def\ncurrentfile eexec\n".to_vec();
        let mut priv_ = b"dup /Private 8 dict dup begin /lenIV -1 def\n/Subrs 1 array\ndup 0 18446744073709551615 RD\n".to_vec();
        priv_.extend_from_slice(b"end\n/CharStrings 1 dict dup begin\n/.notdef 4 RD \x8b\x8b\r\x0e ND\nend\nend\nmark currentfile closefile\n");
        out.extend(encrypt(&priv_, EEXEC_R, 4));
        out.extend_from_slice(b"\n0000000000000000\ncleartomark\n");
        let t = Type1::parse(&out).expect("the damaged Subrs entry is skipped");
        assert!(t.has_glyph(".notdef"));
    }
}

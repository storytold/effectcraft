use super::value::V;
use super::{Rc, RefCell, ascii85, is_white, parse_num};

/// Source tokenizer (the data source for `currentfile`).
pub(super) struct Src<'a> {
    pub(super) d: &'a [u8],
    pub(crate) pos: usize,
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
    pub(super) fn token(&mut self, depth: usize) -> Option<V> {
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
    pub(super) fn skip_image_data(&mut self) {
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

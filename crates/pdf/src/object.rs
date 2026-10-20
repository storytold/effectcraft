//! Local object model, content-stream lexer and stream filters. Document objects are read
//! through the shared parser in `read`; the local lexer remains for content operators and
//! PostScript-related data.

use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq)]
pub enum Obj {
    Null,
    Bool(bool),
    Num(f64),
    Name(String),
    Str(Vec<u8>),
    Array(Vec<Obj>),
    Dict(Dict),
    Ref(u32, u16),
    /// A stream: its dictionary and raw (still encoded) data.
    Stream(Dict, Vec<u8>),
    /// A content-stream operator (or any bare keyword).
    Op(String),
}

pub type Dict = HashMap<String, Obj>;

impl Obj {
    pub fn num(&self) -> Option<f64> {
        match self {
            Obj::Num(n) => Some(*n),
            _ => None,
        }
    }
    pub fn name(&self) -> Option<&str> {
        match self {
            Obj::Name(n) => Some(n),
            _ => None,
        }
    }
    pub fn array(&self) -> Option<&[Obj]> {
        match self {
            Obj::Array(a) => Some(a),
            _ => None,
        }
    }
    pub fn dict(&self) -> Option<&Dict> {
        match self {
            Obj::Dict(d) | Obj::Stream(d, _) => Some(d),
            _ => None,
        }
    }
}

pub fn is_white(c: u8) -> bool {
    matches!(c, 0 | 9 | 10 | 12 | 13 | 32)
}
pub fn is_delim(c: u8) -> bool {
    matches!(c, b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%')
}

/// A byte lexer producing PDF objects (arrays and dictionaries nested).
pub struct Lexer<'a> {
    pub data: &'a [u8],
    pub pos: usize,
}

impl<'a> Lexer<'a> {
    pub fn new(data: &'a [u8], pos: usize) -> Self {
        Lexer { data, pos }
    }

    pub fn skip_ws(&mut self) {
        while self.pos < self.data.len() {
            let c = self.data[self.pos];
            if is_white(c) {
                self.pos += 1;
            } else if c == b'%' {
                while self.pos < self.data.len() && !matches!(self.data[self.pos], b'\n' | b'\r') {
                    self.pos += 1;
                }
            } else {
                break;
            }
        }
    }

    fn word(&mut self) -> &'a [u8] {
        let s = self.pos;
        while self.pos < self.data.len() && !is_white(self.data[self.pos]) && !is_delim(self.data[self.pos]) {
            self.pos += 1;
        }
        &self.data[s..self.pos]
    }

    /// The next object, `None` at the end. `]` and `>>` come back as `Op`s.
    pub fn next(&mut self) -> Option<Obj> {
        self.next_depth(0)
    }

    fn next_depth(&mut self, depth: usize) -> Option<Obj> {
        self.skip_ws();
        let c = *self.data.get(self.pos)?;
        match c {
            b'/' => {
                self.pos += 1;
                let w = self.word();
                Some(Obj::Name(decode_name(w)))
            }
            b'(' => Some(Obj::Str(self.literal_string())),
            b'<' if self.data.get(self.pos + 1) == Some(&b'<') => {
                self.pos += 2;
                let mut d = Dict::new();
                loop {
                    if depth > 64 {
                        return Some(Obj::Null);
                    }
                    let k = self.next_depth(depth + 1);
                    match k {
                        None => break,
                        Some(Obj::Op(o)) if o == ">>" => break,
                        Some(Obj::Name(k)) => {
                            let v = self.next_depth(depth + 1).unwrap_or(Obj::Null);
                            let v = self.maybe_ref(v);
                            d.insert(k, v);
                        }
                        Some(_) => {}
                    }
                }
                Some(Obj::Dict(d))
            }
            b'<' => {
                self.pos += 1;
                let mut hex = vec![];
                while self.pos < self.data.len() && self.data[self.pos] != b'>' {
                    let h = self.data[self.pos];
                    if h.is_ascii_hexdigit() {
                        hex.push(h);
                    }
                    self.pos += 1;
                }
                self.pos += 1;
                if hex.len() % 2 == 1 {
                    hex.push(b'0');
                }
                Some(Obj::Str(hex.chunks(2).map(|p| (hexv(p[0]) << 4) | hexv(p[1])).collect()))
            }
            b'>' if self.data.get(self.pos + 1) == Some(&b'>') => {
                self.pos += 2;
                Some(Obj::Op(">>".into()))
            }
            b'[' => {
                self.pos += 1;
                let mut a = vec![];
                loop {
                    if depth > 64 {
                        return Some(Obj::Null);
                    }
                    match self.next_depth(depth + 1) {
                        None => break,
                        Some(Obj::Op(o)) if o == "]" => break,
                        Some(v) => {
                            let v = self.maybe_ref(v);
                            a.push(v);
                        }
                    }
                }
                Some(Obj::Array(a))
            }
            b']' | b'{' | b'}' | b')' | b'>' => {
                self.pos += 1;
                Some(Obj::Op((c as char).to_string()))
            }
            _ => {
                let w = self.word();
                if w.is_empty() {
                    self.pos += 1;
                    return Some(Obj::Op(String::new()));
                }
                Some(match w {
                    b"true" => Obj::Bool(true),
                    b"false" => Obj::Bool(false),
                    b"null" => Obj::Null,
                    _ => match parse_num(w) {
                        Some(n) => Obj::Num(n),
                        None => Obj::Op(String::from_utf8_lossy(w).into_owned()),
                    },
                })
            }
        }
    }

    /// `n g R` after a number inside arrays and dictionaries.
    pub(super) fn maybe_ref(&mut self, v: Obj) -> Obj {
        let Obj::Num(n) = v else { return v };
        if n.fract() != 0.0 || n < 0.0 {
            return v;
        }
        let save = self.pos;
        self.skip_ws();
        let gp = self.pos;
        let g = self.word();
        if let Some(gn) = parse_num(g).filter(|g| g.fract() == 0.0 && *g >= 0.0) {
            self.skip_ws();
            if self.data.get(self.pos) == Some(&b'R') && self.data.get(self.pos + 1).is_none_or(|c| is_white(*c) || is_delim(*c)) {
                self.pos += 1;
                return Obj::Ref(n as u32, gn as u16);
            }
        }
        let _ = gp;
        self.pos = save;
        v
    }

    fn literal_string(&mut self) -> Vec<u8> {
        self.pos += 1;
        let mut out = vec![];
        let mut depth = 1;
        while self.pos < self.data.len() {
            let c = self.data[self.pos];
            self.pos += 1;
            match c {
                b'\\' => {
                    let Some(&e) = self.data.get(self.pos) else { break };
                    self.pos += 1;
                    match e {
                        b'n' => out.push(b'\n'),
                        b'r' => out.push(b'\r'),
                        b't' => out.push(b'\t'),
                        b'b' => out.push(8),
                        b'f' => out.push(12),
                        b'\r' => {
                            if self.data.get(self.pos) == Some(&b'\n') {
                                self.pos += 1;
                            }
                        }
                        b'\n' => {}
                        b'0'..=b'7' => {
                            let mut v = (e - b'0') as u32;
                            for _ in 0..2 {
                                match self.data.get(self.pos) {
                                    Some(d @ b'0'..=b'7') => {
                                        v = v * 8 + (d - b'0') as u32;
                                        self.pos += 1;
                                    }
                                    _ => break,
                                }
                            }
                            out.push(v as u8);
                        }
                        other => out.push(other),
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
        out
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

fn decode_name(w: &[u8]) -> String {
    let mut out = vec![];
    let mut i = 0;
    while i < w.len() {
        if w[i] == b'#' && w.get(i + 1).is_some_and(u8::is_ascii_hexdigit) && w.get(i + 2).is_some_and(u8::is_ascii_hexdigit) {
            out.push((hexv(w[i + 1]) << 4) | hexv(w[i + 2]));
            i += 3;
        } else {
            out.push(w[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn parse_num(w: &[u8]) -> Option<f64> {
    if w.is_empty() || !w.iter().all(|c| c.is_ascii_digit() || matches!(c, b'+' | b'-' | b'.')) || !w.iter().any(u8::is_ascii_digit) {
        return None;
    }
    let s = std::str::from_utf8(w).ok()?;
    // "--5" and similar malformed numbers: take what parses.
    s.parse::<f64>().ok().or_else(|| s.trim_start_matches(['+', '-']).parse::<f64>().ok().map(|v| if s.starts_with('-') { -v } else { v }))
}

// ------------------------------------------------------------------ filters

/// The most bytes one stream filter may decode to. Deflate expands up to about 1000:1, so
/// unbounded, a few MB of PDF ask for gigabytes and a failed allocation aborts the process. The
/// largest image `image::decode` accepts (64 Mi pixels) of four 16-bit components is 512 MiB;
/// 1 GiB leaves room for more components and is PdfCraft's ceiling for one stream. 32-bit
/// targets (the web build) get a quarter: their whole address space is 4 GiB, and the inflater's
/// growing buffer can reserve about twice the limit.
const MAX_INFLATED: usize = if cfg!(target_pointer_width = "32") { 256 << 20 } else { 1 << 30 };

/// The bound for ordinary object/xref streams and malformed object-stream recovery.
/// The shared parser receives this immutable limit when the document is opened;
/// 32-bit targets retain the app's quarter-sized limit.
pub(crate) const MAX_OBJECT_STREAM: usize = if cfg!(target_pointer_width = "32") { 64 << 20 } else { 256 << 20 };

/// Decode a stream's data through its `/Filter` chain. Unknown filters (images: DCT, JPX, CCITT,
/// JBIG2) give `None`.
pub fn decode_stream(file: &File, d: &Dict, raw: &[u8]) -> Option<Vec<u8>> {
    decode_stream_within(file, d, raw, MAX_INFLATED)
}

/// [`decode_stream`] with every filter's output bounded by `max` bytes. Exceeding the
/// bound gives `None`, never a truncated prefix; predictors share the same bound.
pub fn decode_stream_within(file: &File, d: &Dict, raw: &[u8], max: usize) -> Option<Vec<u8>> {
    let filters: Vec<String> = match d.get("Filter").map(|f| file.resolve(f)) {
        None => vec![],
        Some(Obj::Name(n)) => vec![n.clone()],
        Some(Obj::Array(a)) => a.iter().filter_map(|f| file.resolve(f).name().map(str::to_string)).collect(),
        _ => vec![],
    };
    let parms: Vec<Option<Dict>> = match d.get("DecodeParms").or_else(|| d.get("DP")).map(|p| file.resolve(p)) {
        Some(Obj::Dict(p)) => vec![Some(p.clone())],
        Some(Obj::Array(a)) => a.iter().map(|p| file.resolve(p).dict().cloned()).collect(),
        _ => vec![],
    };
    let mut data = raw.to_vec();
    for (i, f) in filters.iter().enumerate() {
        data = match f.as_str() {
            "FlateDecode" | "Fl" => inflate(&data, max)?,
            "ASCIIHexDecode" | "AHx" => ascii_hex(&data, max)?,
            "ASCII85Decode" | "A85" => ascii85_within(&data, max)?,
            "LZWDecode" | "LZW" => {
                lzw(&data, parms.get(i).and_then(|p| p.as_ref()).and_then(|p| p.get("EarlyChange")).and_then(Obj::num).unwrap_or(1.0) != 0.0, max)?
            }
            "RunLengthDecode" | "RL" => run_length(&data, max)?,
            "CCITTFaxDecode" | "CCF" => crate::ccitt::decode(&data, &ccitt_params(file, parms.get(i).and_then(|p| p.as_ref())), max)?,
            _ => return None,
        };
        if data.len() > max {
            return None;
        }
        if let Some(Some(p)) = parms.get(i)
            && matches!(f.as_str(), "FlateDecode" | "Fl" | "LZWDecode" | "LZW")
        {
            data = predict(&data, p, max)?;
        }
    }
    Some(data)
}

/// `/DecodeParms` of a `CCITTFaxDecode` filter.
fn ccitt_params(file: &File, d: Option<&Dict>) -> crate::ccitt::Params {
    let mut p = crate::ccitt::Params::default();
    let Some(d) = d else { return p };
    let flag = |k: &str, default: bool| match file.get(d, k) {
        Some(Obj::Bool(b)) => *b,
        _ => default,
    };
    p.k = file.get_num(d, "K").unwrap_or(0.0) as i64;
    p.columns = file.get_num(d, "Columns").map(|c| c.max(1.0) as usize).unwrap_or(1728);
    p.rows = file.get_num(d, "Rows").map(|r| r.max(0.0) as usize).unwrap_or(0);
    p.end_of_line = flag("EndOfLine", false);
    p.byte_align = flag("EncodedByteAlign", false);
    p.end_of_block = flag("EndOfBlock", true);
    p.black_is_1 = flag("BlackIs1", false);
    p
}

/// Inflate zlib data (or raw deflate) to at most `max` bytes. A stream that inflates to more is
/// undecodable (`None`), not salvaged as a truncated one.
pub fn inflate(data: &[u8], max: usize) -> Option<Vec<u8>> {
    use miniz_oxide::inflate::{TINFLStatus, decompress_to_vec_with_limit, decompress_to_vec_zlib_with_limit};
    for attempt in [decompress_to_vec_zlib_with_limit, decompress_to_vec_with_limit] {
        match attempt(data, max) {
            Ok(v) => return Some(v),
            Err(e) if e.status == TINFLStatus::HasMoreOutput => return None,
            Err(_) => {}
        }
    }
    // Truncated streams: keep what inflates.
    let mut d = miniz_oxide::inflate::stream::InflateState::new_boxed(miniz_oxide::DataFormat::Zlib);
    let mut out = vec![0u8; salvage_len(data.len(), max)];
    let r = miniz_oxide::inflate::stream::inflate(&mut d, data, &mut out, miniz_oxide::MZFlush::Finish);
    (r.bytes_written > 0).then(|| out[..r.bytes_written].to_vec())
}

/// The buffer [`inflate`] salvages a truncated stream of `data_len` bytes into: eight times that
/// and 1 KiB, at most `max`. The zlib attempt already stopped within `max`, so the bound loses
/// nothing, while without it a Flate filter after another could ask for eight times `max`.
pub fn salvage_len(data_len: usize, max: usize) -> usize {
    data_len.saturating_mul(8).saturating_add(1024).min(max)
}

fn ascii_hex(data: &[u8], max: usize) -> Option<Vec<u8>> {
    let mut out = vec![];
    let mut high = None;
    for c in data.iter().copied().take_while(|c| *c != b'>').filter(u8::is_ascii_hexdigit) {
        if let Some(h) = high.take() {
            if out.len() >= max {
                return None;
            }
            out.push((h << 4) | hexv(c));
        } else {
            high = Some(hexv(c));
        }
    }
    if let Some(h) = high {
        if out.len() >= max {
            return None;
        }
        out.push(h << 4);
    }
    Some(out)
}

pub fn ascii85(data: &[u8]) -> Vec<u8> {
    // The PostScript reader uses this unbounded entry point. No Vec can hold more
    // than usize::MAX bytes, so its output cannot exceed this bound.
    ascii85_within(data, usize::MAX).unwrap_or_default()
}

fn ascii85_within(data: &[u8], max: usize) -> Option<Vec<u8>> {
    let mut out = vec![];
    let mut group = [0u32; 5];
    let mut n = 0;
    let mut i = 0;
    if data.starts_with(b"<~") {
        i = 2;
    }
    while i < data.len() {
        let c = data[i];
        i += 1;
        match c {
            b'~' => break,
            b'z' if n == 0 => {
                if out.len().checked_add(4)? > max {
                    return None;
                }
                out.extend_from_slice(&[0; 4]);
            }
            b'!'..=b'u' => {
                group[n] = (c - b'!') as u32;
                n += 1;
                if n == 5 {
                    if out.len().checked_add(4)? > max {
                        return None;
                    }
                    let v = group.iter().fold(0u32, |a, &d| a.wrapping_mul(85).wrapping_add(d));
                    out.extend_from_slice(&v.to_be_bytes());
                    n = 0;
                }
            }
            _ => {}
        }
    }
    if n > 1 {
        if out.len().checked_add(n - 1)? > max {
            return None;
        }
        for g in group.iter_mut().skip(n) {
            *g = 84;
        }
        let v = group.iter().fold(0u32, |a, &d| a.wrapping_mul(85).wrapping_add(d));
        out.extend_from_slice(&v.to_be_bytes()[..n - 1]);
    }
    Some(out)
}

fn lzw(data: &[u8], early: bool, max: usize) -> Option<Vec<u8>> {
    let mut out = vec![];
    let mut table: Vec<Vec<u8>> = (0..=255u16).map(|b| vec![b as u8]).collect();
    table.push(vec![]);
    table.push(vec![]);
    let (mut bits, mut acc, mut nbits) = (9u32, 0u32, 0u32);
    let mut prev: Option<Vec<u8>> = None;
    for &b in data {
        acc = (acc << 8) | b as u32;
        nbits += 8;
        while nbits >= bits {
            let code = ((acc >> (nbits - bits)) & ((1 << bits) - 1)) as usize;
            nbits -= bits;
            match code {
                256 => {
                    table.truncate(258);
                    bits = 9;
                    prev = None;
                    continue;
                }
                257 => return Some(out),
                _ => {}
            }
            let entry = if code < table.len() {
                table[code].clone()
            } else if let Some(p) = &prev {
                let mut e = p.clone();
                e.push(p[0]);
                e
            } else {
                return Some(out);
            };
            if out.len().checked_add(entry.len())? > max {
                return None;
            }
            out.extend_from_slice(&entry);
            // PDF codes have at most 12 bits. Freeze a full table until a clear code.
            if let Some(p) = prev
                && table.len() < 4096
            {
                let mut e = p;
                e.push(entry[0]);
                table.push(e);
            }
            prev = Some(entry);
            let limit = table.len() + usize::from(early);
            if limit >= (1 << bits) && bits < 12 {
                bits += 1;
            }
        }
    }
    Some(out)
}

fn run_length(data: &[u8], max: usize) -> Option<Vec<u8>> {
    let mut out = vec![];
    let mut i = 0;
    while i < data.len() {
        let l = data[i] as usize;
        i += 1;
        if l < 128 {
            let end = i.saturating_add(l + 1).min(data.len());
            let literal = data.get(i..end)?;
            if out.len().checked_add(literal.len())? > max {
                return None;
            }
            out.extend_from_slice(literal);
            i = end;
        } else if l > 128 {
            if let Some(&b) = data.get(i) {
                if out.len().checked_add(257 - l)? > max {
                    return None;
                }
                out.extend(std::iter::repeat_n(b, 257 - l));
            }
            i = i.saturating_add(1);
        } else {
            break;
        }
    }
    Some(out)
}

/// PNG / TIFF predictors (`/Predictor`, `/Colors`, `/BitsPerComponent`, `/Columns`).
fn predict(data: &[u8], p: &Dict, max: usize) -> Option<Vec<u8>> {
    if data.len() > max {
        return None;
    }
    let g = |k: &str, d: f64| p.get(k).and_then(Obj::num).unwrap_or(d) as usize;
    let pred = g("Predictor", 1.0);
    if pred < 10 {
        return Some(data.to_vec());
    }
    // A row that is zero bytes wide, overflows, or is not shorter than the data leaves no
    // complete row: nothing comes out, as it always did for the first and last, and the row
    // buffer (sized from `/Columns`) is not allocated.
    let Some(pixel_bits) = g("Colors", 1.0).checked_mul(g("BitsPerComponent", 8.0)) else { return Some(vec![]) };
    let Some(bpp) = pixel_bits.checked_add(7).map(|n| (n / 8).max(1)) else { return Some(vec![]) };
    let Some(row) = g("Columns", 1.0).checked_mul(pixel_bits).and_then(|n| n.checked_add(7)).map(|n| n / 8).filter(|&n| n > 0 && n < data.len()) else {
        return Some(vec![]);
    };
    let stride = row.checked_add(1)?;
    let mut out = Vec::with_capacity(data.len());
    let mut prev = vec![0u8; row];
    for chunk in data.chunks(stride) {
        if chunk.len() < stride {
            break;
        }
        let ft = chunk[0];
        let mut cur = chunk[1..].to_vec();
        for i in 0..row {
            let a = if i >= bpp { cur[i - bpp] as i32 } else { 0 };
            let b = prev[i] as i32;
            let c = if i >= bpp { prev[i - bpp] as i32 } else { 0 };
            let add = match ft {
                1 => a,
                2 => b,
                3 => (a + b) / 2,
                4 => {
                    let pp = a + b - c;
                    let (pa, pb, pc) = ((pp - a).abs(), (pp - b).abs(), (pp - c).abs());
                    if pa <= pb && pa <= pc {
                        a
                    } else if pb <= pc {
                        b
                    } else {
                        c
                    }
                }
                _ => 0,
            };
            cur[i] = (cur[i] as i32 + add) as u8;
        }
        out.extend_from_slice(&cur);
        prev = cur;
    }
    Some(out)
}

// ------------------------------------------------------------------ the file

/// Every indirect object of a PDF file, plus the trailer dictionaries found.
pub struct File {
    pub objects: HashMap<(u32, u16), Obj>,
    pub trailers: Vec<Dict>,
    pub repairs: Vec<String>,
}

impl File {
    pub fn parse(data: &[u8]) -> Result<File, crate::Error> {
        crate::read::open(data)
    }

    /// Follow references (bounded).
    pub fn resolve<'a>(&'a self, o: &'a Obj) -> &'a Obj {
        let mut o = o;
        for _ in 0..32 {
            match o {
                Obj::Ref(n, g) => match self.objects.get(&(*n, *g)) {
                    Some(x) => o = x,
                    None => return &Obj::Null,
                },
                _ => return o,
            }
        }
        &Obj::Null
    }

    pub fn get<'a>(&'a self, d: &'a Dict, key: &str) -> Option<&'a Obj> {
        d.get(key).map(|o| self.resolve(o)).filter(|o| !matches!(o, Obj::Null))
    }

    pub fn get_dict<'a>(&'a self, d: &'a Dict, key: &str) -> Option<&'a Dict> {
        self.get(d, key)?.dict()
    }

    pub fn get_num(&self, d: &Dict, key: &str) -> Option<f64> {
        self.get(d, key)?.num()
    }

    /// Numbers of an array (references resolved).
    pub fn nums(&self, o: &Obj) -> Vec<f64> {
        self.resolve(o).array().map(|a| a.iter().filter_map(|x| self.resolve(x).num()).collect()).unwrap_or_default()
    }

    /// The document catalog.
    pub fn catalog(&self) -> Option<&Dict> {
        for t in self.trailers.iter().rev() {
            if let Some(Obj::Dict(d)) = t.get("Root").map(|r| self.resolve(r)) {
                return Some(d);
            }
        }
        let mut cats: Vec<(&(u32, u16), &Obj)> =
            self.objects.iter().filter(|(_, o)| matches!(o, Obj::Dict(d) if d.get("Type").and_then(Obj::name) == Some("Catalog"))).collect();
        cats.sort_by_key(|(n, _)| **n);
        cats.last().and_then(|(_, o)| o.dict())
    }
}

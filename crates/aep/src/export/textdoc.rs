//! The text document dictionary (`LIST btdk`, `docs/aep-format.md` 3.10) as an editable tree:
//! `/key value` pairs, `<< >>` dictionaries, `[ ]` arrays, and atoms (numbers, names, booleans,
//! strings) kept as their raw bytes. Writing it back uses After Effects' own spacing (one space
//! between tokens, ` /key value` at the top level), so an unedited tree writes the bytes it was
//! read from.

/// Deepest nesting read (After Effects' documents nest about 16 deep).
const MAX_DEPTH: usize = 64;

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum TNode {
    Dict(Vec<(String, TNode)>),
    Array(Vec<TNode>),
    /// A number, name, boolean or string, as stored.
    Atom(Vec<u8>),
}

impl TNode {
    pub(crate) fn get(&self, key: &str) -> Option<&TNode> {
        match self {
            TNode::Dict(kv) => kv.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub(crate) fn get_mut(&mut self, key: &str) -> Option<&mut TNode> {
        match self {
            TNode::Dict(kv) => kv.iter_mut().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub(crate) fn at_mut(&mut self, i: usize) -> Option<&mut TNode> {
        match self {
            TNode::Array(v) => v.get_mut(i),
            _ => None,
        }
    }

    /// The node at a `/`-separated path of keys and `[n]` indices (`1/1[0]/0`).
    pub(crate) fn path_mut(&mut self, path: &str) -> Option<&mut TNode> {
        let mut at = self;
        for step in path.split('/').filter(|s| !s.is_empty()) {
            let (key, idx) = match step.split_once('[') {
                Some((k, rest)) => (k, rest.strip_suffix(']').and_then(|n| n.parse::<usize>().ok())),
                None => (step, None),
            };
            at = at.get_mut(key)?;
            if let Some(i) = idx {
                at = at.at_mut(i)?;
            }
        }
        Some(at)
    }

    pub(crate) fn path(&self, path: &str) -> Option<&TNode> {
        let mut at = self;
        for step in path.split('/').filter(|s| !s.is_empty()) {
            let (key, idx) = match step.split_once('[') {
                Some((k, rest)) => (k, rest.strip_suffix(']').and_then(|n| n.parse::<usize>().ok())),
                None => (step, None),
            };
            at = at.get(key)?;
            if let Some(i) = idx {
                at = match at {
                    TNode::Array(v) => v.get(i)?,
                    _ => return None,
                };
            }
        }
        Some(at)
    }

    /// Sets (or adds) `key` in a dictionary.
    pub(crate) fn set(&mut self, key: &str, value: TNode) {
        if let TNode::Dict(kv) = self {
            match kv.iter_mut().find(|(k, _)| k == key) {
                Some((_, v)) => *v = value,
                None => kv.push((key.to_string(), value)),
            }
        }
    }

    pub(crate) fn remove(&mut self, key: &str) {
        if let TNode::Dict(kv) = self {
            kv.retain(|(k, _)| k != key);
        }
    }
}

fn is_ws(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\r' | b'\n' | 0)
}

fn is_delim(c: u8) -> bool {
    is_ws(c) || matches!(c, b'/' | b'[' | b']' | b'(' | b')' | b'<' | b'>')
}

struct P<'a> {
    b: &'a [u8],
    i: usize,
}

impl P<'_> {
    fn ws(&mut self) {
        while self.b.get(self.i).copied().is_some_and(is_ws) {
            self.i += 1;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.b.get(self.i).copied()
    }

    fn starts(&self, s: &[u8]) -> bool {
        self.b.get(self.i..).is_some_and(|r| r.starts_with(s))
    }

    fn name(&mut self) -> Option<String> {
        if self.peek() != Some(b'/') {
            return None;
        }
        self.i += 1;
        let start = self.i;
        while self.peek().is_some_and(|c| !is_delim(c)) {
            self.i += 1;
        }
        Some(String::from_utf8_lossy(self.b.get(start..self.i)?).to_string())
    }

    fn value(&mut self, depth: usize) -> Option<TNode> {
        if depth > MAX_DEPTH {
            return None;
        }
        self.ws();
        if self.starts(b"<<") {
            self.i += 2;
            let mut kv = Vec::new();
            loop {
                self.ws();
                if self.starts(b">>") {
                    self.i += 2;
                    return Some(TNode::Dict(kv));
                }
                let k = self.name()?;
                let v = self.value(depth + 1)?;
                kv.push((k, v));
            }
        }
        match self.peek()? {
            b'[' => {
                self.i += 1;
                let mut v = Vec::new();
                loop {
                    self.ws();
                    if self.peek()? == b']' {
                        self.i += 1;
                        return Some(TNode::Array(v));
                    }
                    v.push(self.value(depth + 1)?);
                }
            }
            b'(' => {
                let start = self.i;
                self.i += 1;
                loop {
                    match self.peek()? {
                        b'\\' => self.i += 2,
                        b')' => {
                            self.i += 1;
                            break;
                        }
                        _ => self.i += 1,
                    }
                }
                Some(TNode::Atom(self.b.get(start..self.i)?.to_vec()))
            }
            b'/' => {
                // A name used as a value (`/CoolTypeFont`, `/nil`).
                let start = self.i;
                self.name()?;
                Some(TNode::Atom(self.b.get(start..self.i)?.to_vec()))
            }
            _ => {
                let start = self.i;
                while self.peek().is_some_and(|c| !is_delim(c)) {
                    self.i += 1;
                }
                if self.i == start {
                    return None;
                }
                Some(TNode::Atom(self.b.get(start..self.i)?.to_vec()))
            }
        }
    }
}

/// Parses a whole document (a top-level sequence of `/key value` pairs); `None` when it is not
/// well formed.
pub(crate) fn parse(b: &[u8]) -> Option<TNode> {
    let mut p = P { b, i: 0 };
    let mut kv = Vec::new();
    loop {
        p.ws();
        if p.i >= b.len() {
            return Some(TNode::Dict(kv));
        }
        let k = p.name()?;
        let v = p.value(0)?;
        kv.push((k, v));
    }
}

fn write_value(n: &TNode, out: &mut Vec<u8>, depth: usize) {
    if depth > MAX_DEPTH {
        out.extend_from_slice(b"<< >>");
        return;
    }
    match n {
        TNode::Atom(a) => out.extend_from_slice(a),
        TNode::Array(v) => {
            out.push(b'[');
            for x in v {
                out.push(b' ');
                write_value(x, out, depth + 1);
            }
            out.extend_from_slice(b" ]");
        }
        TNode::Dict(kv) => {
            out.extend_from_slice(b"<<");
            for (k, v) in kv {
                out.extend_from_slice(b" /");
                out.extend_from_slice(k.as_bytes());
                out.push(b' ');
                write_value(v, out, depth + 1);
            }
            out.extend_from_slice(b" >>");
        }
    }
}

/// The document's bytes.
pub(crate) fn write(doc: &TNode) -> Vec<u8> {
    let mut out = Vec::new();
    if let TNode::Dict(kv) = doc {
        for (k, v) in kv {
            out.extend_from_slice(b" /");
            out.extend_from_slice(k.as_bytes());
            out.push(b' ');
            write_value(v, &mut out, 0);
        }
    }
    out
}

/// A string atom: `(` + `fe ff` + UTF-16BE + `)`, with `(`, `)` and `\` escaped.
pub(crate) fn string(s: &str) -> TNode {
    let mut out = vec![b'('];
    let mut push = |b: u8| {
        if matches!(b, b'(' | b')' | b'\\') {
            out.push(b'\\');
        }
        out.push(b);
    };
    push(0xfe);
    push(0xff);
    for u in s.encode_utf16() {
        let [hi, lo] = u.to_be_bytes();
        push(hi);
        push(lo);
    }
    out.push(b')');
    TNode::Atom(out)
}

/// A number as After Effects writes it: integers without a point, others with one (`60.0`,
/// `.5`, `-49.05029`).
pub(crate) fn number(v: f64) -> TNode {
    let v = if v.is_finite() { v } else { 0.0 };
    let mut s = if v.fract() == 0.0 && v.abs() < 1e15 { format!("{v:.1}") } else { format!("{}", (v * 1e5).round() / 1e5) };
    // `.5` and `-.25`, but `0.0` (113: `[ 1.0 1.0 .5 0.0 ]`).
    if v != 0.0 {
        if let Some(rest) = s.strip_prefix("0.") {
            s = format!(".{rest}");
        } else if let Some(rest) = s.strip_prefix("-0.") {
            s = format!("-.{rest}");
        }
    }
    TNode::Atom(s.into_bytes())
}

pub(crate) fn int(v: i64) -> TNode {
    TNode::Atom(v.to_string().into_bytes())
}

pub(crate) fn boolean(v: bool) -> TNode {
    TNode::Atom(if v { b"true".to_vec() } else { b"false".to_vec() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_writes_back() {
        let src = b" /98 << /0 13 >> /0 << /1 [ 1.0 .5 (\xfe\xff\x00A\\)) /nil << >> [ ] ] >>";
        let t = parse(src).expect("parse");
        assert_eq!(write(&t), src.to_vec());
        assert_eq!(t.path("0/1[1]"), Some(&TNode::Atom(b".5".to_vec())));
    }

    #[test]
    fn numbers_and_strings_as_after_effects_writes_them() {
        assert_eq!(number(60.0), TNode::Atom(b"60.0".to_vec()));
        assert_eq!(number(0.5), TNode::Atom(b".5".to_vec()));
        assert_eq!(number(0.0), TNode::Atom(b"0.0".to_vec()));
        assert_eq!(number(-0.25), TNode::Atom(b"-.25".to_vec()));
        assert_eq!(number(98.123456), TNode::Atom(b"98.12346".to_vec()));
        assert_eq!(string("A)"), TNode::Atom(b"(\xfe\xff\x00A\x00\\))".to_vec()));
    }

    #[test]
    fn damaged_documents_are_none_not_a_panic() {
        for bad in [&b"/a << /b"[..], b"/a [ 1 2", b"/a (unterminated", b"<<", b"/", b"/a >>", b"/a ]"] {
            let _ = parse(bad);
        }
        let deep: Vec<u8> = std::iter::once(b"/a ".to_vec()).chain(std::iter::repeat_n(b"[ ".to_vec(), 500)).flatten().collect();
        assert_eq!(parse(&deep), None);
    }
}

//! The text engine data of type layers (`EngineData` in the `TySh` descriptor): a PostScript-like
//! tree of dictionaries (`<< /Key value >>`), arrays (`[ ]`), numbers, names, booleans and
//! strings (`(þÿ` + UTF-16BE with backslash escapes `)`). Read best-effort.

/// Bytes that end a bare token or a `/Name` (besides whitespace).
const DELIMITERS: &[u8] = b"[]<>/(";

#[derive(Clone, Debug, PartialEq)]
pub enum EValue {
    Dict(Vec<(String, EValue)>),
    Array(Vec<EValue>),
    Num(f64),
    Bool(bool),
    Name(String),
    Str(String),
}

impl EValue {
    pub fn get(&self, key: &str) -> Option<&EValue> {
        match self {
            EValue::Dict(v) => v.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
    pub fn idx(&self, i: usize) -> Option<&EValue> {
        match self {
            EValue::Array(v) => v.get(i),
            _ => None,
        }
    }
    /// Follow a `/`-separated path; numeric segments index arrays.
    pub fn path(&self, p: &str) -> Option<&EValue> {
        let mut cur = self;
        for seg in p.split('/').filter(|s| !s.is_empty()) {
            cur = match seg.parse::<usize>() {
                Ok(i) => cur.idx(i)?,
                Err(_) => cur.get(seg)?,
            };
        }
        Some(cur)
    }
    pub fn num(&self) -> Option<f64> {
        match self {
            EValue::Num(n) => Some(*n),
            _ => None,
        }
    }
    pub fn str(&self) -> Option<&str> {
        match self {
            EValue::Str(s) | EValue::Name(s) => Some(s),
            _ => None,
        }
    }
}

struct P<'a> {
    b: &'a [u8],
    i: usize,
}

impl P<'_> {
    fn ws(&mut self) {
        while self.i < self.b.len() && (self.b[self.i].is_ascii_whitespace() || self.b[self.i] == 0) {
            self.i += 1;
        }
    }
    fn peek(&self, s: &[u8]) -> bool {
        self.b[self.i..].starts_with(s)
    }
    fn value(&mut self, depth: usize) -> Option<EValue> {
        if depth > 128 {
            return None;
        }
        self.ws();
        if self.i >= self.b.len() {
            return None;
        }
        if self.peek(b"<<") {
            self.i += 2;
            let mut v = vec![];
            loop {
                self.ws();
                if self.i >= self.b.len() {
                    return Some(EValue::Dict(v));
                }
                if self.peek(b">>") {
                    self.i += 2;
                    return Some(EValue::Dict(v));
                }
                if self.b[self.i] != b'/' {
                    // Unexpected token: skip it.
                    self.i += 1;
                    continue;
                }
                let key = self.name();
                let val = self.value(depth + 1)?;
                v.push((key, val));
            }
        }
        match self.b[self.i] {
            b'[' => {
                self.i += 1;
                let mut v = vec![];
                loop {
                    self.ws();
                    if self.i >= self.b.len() {
                        return Some(EValue::Array(v));
                    }
                    if self.b[self.i] == b']' {
                        self.i += 1;
                        return Some(EValue::Array(v));
                    }
                    let start = self.i;
                    match self.value(depth + 1) {
                        Some(x) => v.push(x),
                        None => return Some(EValue::Array(v)),
                    }
                    if self.i == start {
                        self.i += 1;
                    }
                }
            }
            b'/' => Some(EValue::Name(self.name())),
            b'(' => Some(EValue::Str(self.string())),
            _ => {
                let start = self.i;
                while self.i < self.b.len() && !self.b[self.i].is_ascii_whitespace() && !DELIMITERS.contains(&self.b[self.i]) {
                    self.i += 1;
                }
                let tok = std::str::from_utf8(&self.b[start..self.i]).unwrap_or("");
                if self.i == start {
                    self.i += 1;
                    return Some(EValue::Name(String::new()));
                }
                Some(match tok {
                    "true" => EValue::Bool(true),
                    "false" => EValue::Bool(false),
                    // "nan", "inf" and overflowing literals parse as f64 but are not numbers here.
                    t => match t.parse::<f64>() {
                        Ok(n) if n.is_finite() => EValue::Num(n),
                        _ => EValue::Name(t.to_string()),
                    },
                })
            }
        }
    }
    fn name(&mut self) -> String {
        self.i += 1; // '/'
        let start = self.i;
        while self.i < self.b.len() && !self.b[self.i].is_ascii_whitespace() && !DELIMITERS.contains(&self.b[self.i]) {
            self.i += 1;
        }
        String::from_utf8_lossy(&self.b[start..self.i]).into_owned()
    }
    fn string(&mut self) -> String {
        self.i += 1; // '('
        let mut raw = vec![];
        while self.i < self.b.len() {
            let c = self.b[self.i];
            self.i += 1;
            match c {
                b'\\' if self.i < self.b.len() => {
                    let e = self.b[self.i];
                    self.i += 1;
                    raw.push(match e {
                        b'n' => b'\n',
                        b'r' => b'\r',
                        b't' => b'\t',
                        other => other,
                    });
                }
                b')' => break,
                _ => raw.push(c),
            }
        }
        if raw.starts_with(&[0xFE, 0xFF]) {
            let u: Vec<u16> = raw[2..].as_chunks::<2>().0.iter().map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
            String::from_utf16_lossy(&u).trim_end_matches('\0').to_string()
        } else {
            String::from_utf8_lossy(&raw).into_owned()
        }
    }
}

/// Parse engine data (returns an empty dictionary on garbage).
pub fn parse(b: &[u8]) -> EValue {
    let mut p = P { b, i: 0 };
    p.value(0).unwrap_or(EValue::Dict(vec![]))
}

/// Encode a string the way engine data stores it (UTF-16BE with a BOM, escaped).
pub fn encode_string(s: &str) -> Vec<u8> {
    let mut out = vec![b'(', 0xFE, 0xFF];
    for u in s.encode_utf16() {
        for b in u.to_be_bytes() {
            if matches!(b, b'(' | b')' | b'\\') {
                out.push(b'\\');
            }
            out.push(b);
        }
    }
    out.push(b')');
    out
}

/// The first style run's character attributes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TextStyle {
    pub font: Option<String>,
    pub size: Option<f64>,
    /// RGB 0..1.
    pub color: Option<[f64; 3]>,
    /// 0 left, 1 right, 2 centre, 3+ justified variants.
    pub justification: Option<u8>,
    pub tracking: Option<f64>,
    pub leading: Option<f64>,
    pub faux_bold: bool,
    pub faux_italic: bool,
    /// Paragraph (box) text rather than point text.
    pub box_text: bool,
}

/// Sane upper bound for a font size in points (Photoshop's own limit is 1296 pt).
const MAX_FONT_SIZE: f64 = 10_000.0;
/// Sane bound for tracking (1/1000 em) and leading (points).
const MAX_SPACING: f64 = 100_000.0;

pub fn text_style(ed: &EValue) -> TextStyle {
    let engine = ed.get("EngineDict").unwrap_or(ed);
    let resources = ed.get("ResourceDict").or_else(|| ed.get("DocumentResources"));
    let run = engine.path("StyleRun/RunArray/0/StyleSheet/StyleSheetData");
    let normal = resources.and_then(|r| r.path("StyleSheetSet/0/StyleSheetData"));
    let attr = |k: &str| run.and_then(|r| r.get(k)).or_else(|| normal.and_then(|n| n.get(k)));
    let font =
        attr("Font").and_then(EValue::num).and_then(|i| resources?.path(&format!("FontSet/{}/Name", i as usize))).and_then(EValue::str).map(str::to_string);
    let color = attr("FillColor").and_then(|c| c.get("Values")).and_then(|v| match v {
        EValue::Array(a) if a.len() >= 4 => Some([a[1].num()?, a[2].num()?, a[3].num()?]),
        _ => None,
    });
    let para = engine.path("ParagraphRun/RunArray/0/ParagraphSheet/Properties");
    let para_normal = resources.and_then(|r| r.path("ParagraphSheetSet/0/Properties"));
    let justification =
        para.and_then(|p| p.get("Justification")).or_else(|| para_normal.and_then(|p| p.get("Justification"))).and_then(EValue::num).map(|n| n as u8);
    let auto_leading = attr("AutoLeading").is_none_or(|v| *v != EValue::Bool(false));
    let box_text = engine.path("Rendered/Shapes/Children/0/ShapeType").and_then(EValue::num) == Some(1.0);
    TextStyle {
        font,
        size: attr("FontSize").and_then(EValue::num).filter(|s| *s > 0.0).map(|s| s.min(MAX_FONT_SIZE)),
        color,
        justification,
        tracking: attr("Tracking").and_then(EValue::num).map(|t| t.clamp(-MAX_SPACING, MAX_SPACING)),
        leading: if auto_leading { None } else { attr("Leading").and_then(EValue::num).map(|l| l.clamp(-MAX_SPACING, MAX_SPACING)) },
        faux_bold: attr("FauxBold") == Some(&EValue::Bool(true)),
        faux_italic: attr("FauxItalic") == Some(&EValue::Bool(true)),
        box_text,
    }
}

/// Minimal engine data for a type layer with one style run (fixtures, and what a writer needs).
pub fn minimal(text: &str, font: &str, size: f64, color: [f64; 3], justification: u8) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"\n\n<<\n\t/EngineDict\n\t<<\n\t\t/Editor << /Text ");
    out.extend_from_slice(&encode_string(&format!("{text}\r")));
    out.extend_from_slice(b" >>\n\t\t/ParagraphRun << /RunArray [ << /ParagraphSheet << /Properties << /Justification ");
    out.extend_from_slice(justification.to_string().as_bytes());
    out.extend_from_slice(b" >> >> >> ] >>\n\t\t/StyleRun << /RunArray [ << /StyleSheet << /StyleSheetData << /Font 0 /FontSize ");
    out.extend_from_slice(format!("{size:.1}").as_bytes());
    out.extend_from_slice(
        format!(" /FillColor << /Type 1 /Values [ 1.0 {:.5} {:.5} {:.5} ] >> /AutoLeading true >> >> >> ] >>\n\t>>\n", color[0], color[1], color[2]).as_bytes(),
    );
    out.extend_from_slice(b"\t/ResourceDict << /FontSet [ << /Name ");
    out.extend_from_slice(&encode_string(font));
    out.extend_from_slice(b" /Type 0 >> ] >>\n>>");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal() {
        let b = minimal("Hi (there)", "Arial-BoldMT", 36.0, [1.0, 0.5, 0.0], 2);
        let v = parse(&b);
        assert_eq!(v.path("EngineDict/Editor/Text").and_then(EValue::str), Some("Hi (there)\r"));
        let st = text_style(&v);
        assert_eq!(st.font.as_deref(), Some("Arial-BoldMT"));
        assert_eq!(st.size, Some(36.0));
        assert_eq!(st.color, Some([1.0, 0.5, 0.0]));
        assert_eq!(st.justification, Some(2));
        assert!(!st.box_text);
    }

    #[test]
    fn garbage_is_harmless() {
        let _ = parse(b"<< /A [ 1 2 << /B (unterminated");
        let _ = parse(b"]]]>>>> /x");
        let _ = parse(&[0xFF; 64]);
    }

    #[test]
    fn non_finite_and_out_of_range_numbers_are_not_numbers() {
        let b = b"<< /FontSize nan /Tracking inf /Leading -inf /Other 1e999 >>";
        let v = parse(b);
        assert_eq!(v.get("FontSize").and_then(EValue::num), None);
        assert_eq!(v.get("Other").and_then(EValue::num), None);
        let ed = parse(b"<< /EngineDict << /StyleRun << /RunArray [ << /StyleSheet << /StyleSheetData << /FontSize nan /Tracking inf /Leading -inf /AutoLeading false >> >> >> ] >> >> >>");
        let st = text_style(&ed);
        assert_eq!((st.size, st.tracking, st.leading), (None, None, None));
        let ed = parse(b"<< /EngineDict << /StyleRun << /RunArray [ << /StyleSheet << /StyleSheetData << /FontSize 1e300 /Tracking -1e300 /AutoLeading false /Leading 1e300 >> >> >> ] >> >> >>");
        let st = text_style(&ed);
        assert_eq!(st.size, Some(MAX_FONT_SIZE));
        assert_eq!(st.tracking, Some(-MAX_SPACING));
        assert_eq!(st.leading, Some(MAX_SPACING));
    }
}

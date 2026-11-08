//! Text documents (`LIST btdk`) and layer / composition markers.
//!
//! The body of a `btdk` list is not a chunk sequence but a PostScript-like dictionary: `/key
//! value` pairs, `<< >>` dictionaries, `[ ]` arrays, numbers, `true` / `false`, `/names`, and strings
//! as `(` + BOM `fe ff` + UTF-16BE + `)` with `\` escapes. [`parse_text_dict`] reads it (depth
//! limited, never panics) and [`Layer::text_document`] pulls out the verified fields.

use std::borrow::Cow;
use std::sync::LazyLock;

use crate::container::bytes::{be_u32, text};
use crate::container::index::Node;
use crate::container::typed::{Comp, Layer, Property};

/// How deep dictionaries and arrays may nest before the parser gives up.
const MAX_DEPTH: usize = 64;

/// A value of the text dictionary.
#[derive(Debug, Clone, PartialEq)]
pub enum TextValue {
    /// `<< ... >>`: keys in file order (a key is the name, string or number written before the value).
    Dict(Vec<(String, TextValue)>),
    /// `[ ... ]`
    Array(Vec<TextValue>),
    /// A string that starts with the UTF-16BE byte order mark, decoded.
    Str(String),
    /// A string without the byte order mark.
    Bytes(Vec<u8>),
    /// `/name`
    Name(String),
    Int(i64),
    Float(f64),
    Bool(bool),
    /// Any other bare word.
    Token(String),
}

impl TextValue {
    /// The value of a dictionary key (the last one when a key repeats).
    pub fn get(&self, key: &str) -> Option<&TextValue> {
        match self {
            Self::Dict(pairs) => pairs.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    /// An array element.
    pub fn at(&self, index: usize) -> Option<&TextValue> {
        match self {
            Self::Array(items) => items.get(index),
            _ => None,
        }
    }

    /// The text of a string value.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::Str(s) => Some(s),
            _ => None,
        }
    }

    /// An integer or float as `f64`.
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Self::Int(i) => Some(*i as f64),
            Self::Float(f) => Some(*f),
            _ => None,
        }
    }

    /// An integer value.
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Self::Int(i) => Some(*i),
            _ => None,
        }
    }
}

/// The text of a key the way the dictionary stores it: a `/name` as written, any other value in
/// its canonical form (`06` and `+6` are `6`, `1.0` is `1`, a string its decoded text).
fn owned_key(value: &V<'_>) -> String {
    match value {
        V::Name(s) | V::Token(s) => text(s),
        V::Str(utf16) => decode_utf16be(utf16),
        V::Int(i) => i.to_string(),
        V::Float(f) => f.to_string(),
        V::Bool(b) => b.to_string(),
        V::Bytes(b) => text(b),
        V::Dict(_) | V::Array(_) => String::new(),
    }
}

/// A value of the text dictionary as [`TextDocument::parse`] reads it: it borrows names, bare
/// words and strings from the body, so a document is read without a `String` per token. A key is
/// borrowed when it is a `/name` (every key After Effects writes); its text is the same as the
/// [`TextValue`] key's. [`V::into_owned`] gives the public [`TextValue`].
#[derive(Debug, Clone)]
enum V<'b> {
    Dict(Vec<(Cow<'b, str>, V<'b>)>),
    Array(Vec<V<'b>>),
    /// A string that started with the byte order mark: the UTF-16BE bytes after it, unescaped.
    Str(Cow<'b, [u8]>),
    Bytes(Cow<'b, [u8]>),
    Name(&'b [u8]),
    Int(i64),
    Float(f64),
    Bool(bool),
    Token(&'b [u8]),
}

impl<'b> V<'b> {
    /// The value of a dictionary key (the last one when a key repeats).
    fn get(&self, key: &str) -> Option<&V<'b>> {
        match self {
            Self::Dict(pairs) => pairs.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    fn at(&self, index: usize) -> Option<&V<'b>> {
        match self {
            Self::Array(items) => items.get(index),
            _ => None,
        }
    }

    /// The decoded text of a string value.
    fn string(&self) -> Option<String> {
        match self {
            Self::Str(utf16) => Some(decode_utf16be(utf16)),
            _ => None,
        }
    }

    fn as_f64(&self) -> Option<f64> {
        match self {
            Self::Int(i) => Some(*i as f64),
            Self::Float(f) => Some(*f),
            _ => None,
        }
    }

    fn as_i64(&self) -> Option<i64> {
        match self {
            Self::Int(i) => Some(*i),
            _ => None,
        }
    }

    /// The key a value makes when it is written in a key position.
    fn into_key(self) -> Cow<'b, str> {
        match self {
            Self::Name(s) => String::from_utf8_lossy(s),
            other => Cow::Owned(owned_key(&other)),
        }
    }

    /// The public form. Recursion is bounded: a parsed value nests at most [`MAX_DEPTH`] + 1 deep.
    fn into_owned(self) -> TextValue {
        match self {
            Self::Dict(pairs) => TextValue::Dict(pairs.into_iter().map(|(k, v)| (k.into_owned(), v.into_owned())).collect()),
            Self::Array(items) => TextValue::Array(items.into_iter().map(V::into_owned).collect()),
            Self::Str(utf16) => TextValue::Str(decode_utf16be(&utf16)),
            Self::Bytes(b) => TextValue::Bytes(b.into_owned()),
            Self::Name(s) => TextValue::Name(text(s)),
            Self::Int(i) => TextValue::Int(i),
            Self::Float(f) => TextValue::Float(f),
            Self::Bool(b) => TextValue::Bool(b),
            Self::Token(s) => TextValue::Token(text(s)),
        }
    }
}

struct Parser<'b> {
    b: &'b [u8],
    i: usize,
    /// The pairs of the dictionaries being read, innermost last: a dictionary's pairs are moved
    /// into a vector of their exact size when it closes (one allocation per dictionary instead of
    /// one per doubling).
    pairs: Vec<(Cow<'b, str>, V<'b>)>,
    /// The same for the items of arrays.
    items: Vec<V<'b>>,
}

/// Whitespace between tokens.
fn is_ws(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\r' | b'\n')
}

/// A byte that ends a bare word.
fn is_delimiter(c: u8) -> bool {
    is_ws(c) || matches!(c, b'/' | b'[' | b']' | b'<' | b'>' | b'(' | b')')
}

impl<'b> Parser<'b> {
    /// The next byte, `None` at the end.
    fn peek(&self) -> Option<u8> {
        self.b.get(self.i).copied()
    }

    /// Moves past whitespace.
    fn skip_ws(&mut self) {
        while self.peek().is_some_and(is_ws) {
            self.i = self.i.saturating_add(1);
        }
    }

    /// True when the unread bytes start with `s`.
    fn starts_with(&self, s: &[u8]) -> bool {
        self.b.get(self.i..).is_some_and(|rest| rest.starts_with(s))
    }

    /// The bare word at the position (up to the next delimiter), borrowed from the body.
    fn word(&mut self) -> &'b [u8] {
        let start = self.i;
        while self.peek().is_some_and(|c| !is_delimiter(c)) {
            self.i = self.i.saturating_add(1);
        }
        self.b.get(start..self.i).unwrap_or(&[])
    }

    /// The body of a `( ... )` string after the `(`, with `\` escapes resolved: borrowed when it
    /// has none. `None` when the string or an escape is not closed.
    fn string_body(&mut self) -> Option<Cow<'b, [u8]>> {
        let rest = self.b.get(self.i..)?;
        let stop = rest.iter().position(|c| matches!(c, b')' | b'\\'))?;
        let head = rest.get(..stop)?;
        self.i = self.i.saturating_add(stop);
        if self.peek()? == b')' {
            self.i = self.i.saturating_add(1);
            return Some(Cow::Borrowed(head));
        }
        let mut out = head.to_vec();
        loop {
            let c = self.peek()?;
            if c == b'\\' {
                out.push(*self.b.get(self.i.saturating_add(1))?);
                self.i = self.i.saturating_add(2);
            } else if c == b')' {
                self.i = self.i.saturating_add(1);
                return Some(Cow::Owned(out));
            } else {
                out.push(c);
                self.i = self.i.saturating_add(1);
            }
        }
    }

    /// The value at the position, `None` when it is malformed or nested deeper than the limit.
    fn parse(&mut self, depth: usize) -> Option<V<'b>> {
        self.skip_ws();
        if depth > MAX_DEPTH {
            return None;
        }
        if self.starts_with(b"<<") {
            self.i = self.i.saturating_add(2);
            // On an error the pairs read so far stay on the stack: the error ends the whole parse.
            let mark = self.pairs.len();
            loop {
                self.skip_ws();
                if self.starts_with(b">>") {
                    self.i = self.i.saturating_add(2);
                    return Some(V::Dict(self.pairs.drain(mark..).collect()));
                }
                let key = self.parse(depth.saturating_add(1))?;
                let value = self.parse(depth.saturating_add(1))?;
                self.pairs.push((key.into_key(), value));
            }
        }
        match self.peek()? {
            b'[' => {
                self.i = self.i.saturating_add(1);
                let mark = self.items.len();
                loop {
                    self.skip_ws();
                    if self.peek()? == b']' {
                        self.i = self.i.saturating_add(1);
                        return Some(V::Array(self.items.drain(mark..).collect()));
                    }
                    let item = self.parse(depth.saturating_add(1))?;
                    self.items.push(item);
                }
            }
            b'(' => {
                self.i = self.i.saturating_add(1);
                let body = self.string_body()?;
                Some(if body.starts_with(&[0xfe, 0xff]) {
                    V::Str(match body {
                        Cow::Borrowed(b) => Cow::Borrowed(b.get(2..).unwrap_or(&[])),
                        Cow::Owned(mut v) => {
                            let n = v.len().min(2);
                            v.drain(..n);
                            Cow::Owned(v)
                        }
                    })
                } else {
                    V::Bytes(body)
                })
            }
            b'/' => {
                self.i = self.i.saturating_add(1);
                Some(V::Name(self.word()))
            }
            _ => {
                let word = self.word();
                if word.is_empty() {
                    return None;
                }
                // A word that is not UTF-8 reads as text with replacement characters, which is
                // neither a boolean nor a number: a token.
                let Ok(w) = std::str::from_utf8(word) else { return Some(V::Token(word)) };
                Some(match w {
                    "true" => V::Bool(true),
                    "false" => V::Bool(false),
                    _ => match w.parse::<i64>() {
                        Ok(i) => V::Int(i),
                        // `parse` also takes `nan`, `inf` and overflowing exponents; those are not
                        // numbers a text document holds, so they stay tokens and read as missing.
                        Err(_) => match w.parse::<f64>() {
                            Ok(f) if f.is_finite() => V::Float(f),
                            _ => V::Token(word),
                        },
                    },
                })
            }
        }
    }
}

/// A big-endian UTF-16 string as text (unpaired surrogates become U+FFFD).
fn decode_utf16be(bytes: &[u8]) -> String {
    let (pairs, rest) = bytes.as_chunks::<2>();
    let units = pairs.iter().map(|p| u16::from_be_bytes(*p));
    let mut s: String = char::decode_utf16(units).map(|r| r.unwrap_or(char::REPLACEMENT_CHARACTER)).collect();
    if !rest.is_empty() {
        s.push(char::REPLACEMENT_CHARACTER);
    }
    s
}

/// The top-level `key value` pairs of a `btdk` body, borrowed (see [`parse_text_dict`]).
fn parse_dict(body: &[u8]) -> V<'_> {
    let mut parser = Parser { b: body, i: 0, pairs: Vec::new(), items: Vec::new() };
    let mut pairs = Vec::new();
    loop {
        parser.skip_ws();
        if parser.i >= body.len() {
            break;
        }
        let Some(key) = parser.parse(0) else { break };
        let Some(value) = parser.parse(0) else { break };
        pairs.push((key.into_key(), value));
    }
    V::Dict(pairs)
}

/// Parses the body of a `btdk` list into its top-level `key value` pairs (as a [`TextValue::Dict`]).
/// Parsing stops silently at the first syntax error or at nesting deeper than 64 levels; what was
/// read before is returned.
pub fn parse_text_dict(body: &[u8]) -> TextValue {
    parse_dict(body).into_owned()
}

/// The run style slots (`RUN/6/0[n]/0/0/6/<slot>`) that are decoded; every other slot is compared
/// with its default (`STYLE_DEFAULTS`) and listed in
/// [`TextDocument::undecoded_style_fields`] when it differs.
pub const DECODED_STYLE_SLOTS: &[&str] =
    &["0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "11", "12", "13", "18", "35", "36", "52", "53", "54", "56", "57", "58", "62", "63", "70"];

/// The paragraph slots (`RUN/5/0[n]/0/0/5/<slot>`) that are decoded (see [`ParagraphRun`]).
pub const DECODED_PARAGRAPH_SLOTS: &[&str] = &["0", "1", "2", "3", "4", "5", "8", "9", "21", "29", "33"];

/// A number slot.
fn val(v: f64) -> V<'static> {
    V::Float(v)
}

/// A dictionary value with these keys, for the built-in defaults.
fn dict(pairs: Vec<(&'static str, V<'static>)>) -> V<'static> {
    V::Dict(pairs.into_iter().map(|(k, v)| (Cow::Borrowed(k), v)).collect())
}

/// An array of numbers, for the built-in defaults.
fn numbers(v: &[f64]) -> V<'static> {
    V::Array(v.iter().map(|x| val(*x)).collect())
}

/// The default of every run style slot that is not decoded and not neutral (zero, `false`,
/// empty, `/nil`): the value an untouched point text holds in every sample (110, 243-245). A slot
/// with a neutral default is left out; [`undecoded_fields`] treats a missing default as neutral.
/// Built once.
static STYLE_DEFAULTS: LazyLock<V<'static>> = LazyLock::new(|| {
    let paint = |a: f64| dict(vec![("99", V::Name(b"SimplePaint")), ("0", dict(vec![("0", val(1.0)), ("1", numbers(&[1.0, 1.0, 1.0, a]))]))]);
    let class = |name: &'static [u8]| dict(vec![("99", V::Name(name))]);
    dict(vec![
        ("37", val(2.0)),
        ("41", val(2.0)),
        ("43", dict(vec![("0", val(0.5))])),
        ("44", val(2.0)),
        ("45", val(2.0)),
        ("46", val(7.0)),
        ("49", val(-1.0)),
        ("50", val(-1.0)),
        ("55", class(b"SimpleBlender")),
        ("64", val(4.0)),
        ("71", val(4.0)),
        ("77", V::Bool(true)),
        ("78", V::Bool(true)),
        ("79", paint(0.0)),
        ("82", val(3.0)),
        ("83", val(3.0)),
        ("86", class(b"SimpleCustomFeature")),
        ("87", val(100.0)),
        ("88", V::Bool(true)),
        ("95", dict(vec![("0", val(0.0)), ("1", val(0.0))])),
        ("96", V::Bool(true)),
    ])
});

/// The same for the paragraph slots (110, 243-245). Built once.
static PARAGRAPH_DEFAULTS: LazyLock<V<'static>> = LazyLock::new(|| {
    dict(vec![
        ("6", val(1.0)),
        ("7", val(1.2)),
        ("10", val(6.0)),
        ("11", val(2.0)),
        ("12", val(2.0)),
        ("14", val(36.0)),
        ("15", V::Bool(true)),
        ("16", val(0.5)),
        ("17", numbers(&[0.8, 1.0, 1.33])),
        ("18", numbers(&[0.0, 0.0, 0.0])),
        ("19", numbers(&[1.0, 1.0, 1.0])),
        ("20", val(6.0)),
        ("22", val(1.0)),
        ("23", V::Bool(true)),
        ("24", val(1.0)),
        ("26", V::Bool(true)),
        ("31", val(36.0)),
        ("35", val(1.0)),
        ("40", val(2.0)),
    ])
});

/// Equal values; numbers compare by value (`0` equals `0.0`), names and words by their text,
/// strings by their decoded text.
fn same(a: &V<'_>, b: &V<'_>) -> bool {
    match (a, b) {
        (V::Dict(x), V::Dict(y)) => x.len() == y.len() && x.iter().all(|(k, v)| y.iter().any(|(k2, v2)| k == k2 && same(v, v2))),
        (V::Array(x), V::Array(y)) => x.len() == y.len() && x.iter().zip(y).all(|(p, q)| same(p, q)),
        (V::Str(x), V::Str(y)) => x == y || decode_utf16be(x) == decode_utf16be(y),
        (V::Bytes(x), V::Bytes(y)) => x == y,
        (V::Name(x), V::Name(y)) | (V::Token(x), V::Token(y)) => x == y || text(x) == text(y),
        (V::Bool(x), V::Bool(y)) => x == y,
        _ => match (a.as_f64(), b.as_f64()) {
            (Some(p), Some(q)) => p == q,
            _ => false,
        },
    }
}

/// A zero, `false`, empty or `/nil` value: a slot that the sampled documents do not have is only
/// listed when it holds something else (a neutral value carries nothing that could be lost).
/// `/nil` is how an untouched document stores a slot without a value (244: paragraph `/27`, `/28`,
/// `/36`).
fn is_neutral(v: &V<'_>) -> bool {
    match v {
        V::Name(n) => *n == b"nil",
        V::Bool(b) => !b,
        V::Array(a) => a.is_empty(),
        V::Dict(d) => d.is_empty(),
        other => other.as_f64() == Some(0.0),
    }
}

/// Paragraph slots whose meaning is not known but whose listed values lay text out alike, so they
/// carry nothing to lose: `/34` is 0 in a scripted text and 7 after Reset Paragraph (727 changes
/// only this slot), and a justified 600 px box text composes the same lines, baselines and bounds
/// with both (S18). `/24` and `/35` are 1 in a scripted text; the wrapping box text 517 with `/24`
/// or `/35` edited to 0 (732, 733) opens without a dialog, keeps the value on a re-save and lays
/// out with the same four lines, baselines and bounds (S21).
pub const NEUTRAL_PARAGRAPH_VALUES: &[(&str, &[i64])] = &[("24", &[0, 1]), ("34", &[0, 7]), ("35", &[0, 1])];

/// Style slots of unknown meaning whose listed values lay text out alike: `/68` is 0 in a scripted
/// text; 517 with `/68` edited to 1 (734) keeps it on a re-save and lays out and renders the same
/// (S21).
pub const NEUTRAL_STYLE_VALUES: &[(&str, &[i64])] = &[("68", &[0, 1])];

/// The most undecoded slots recorded per text document (style and paragraph each). They are listed
/// in one report line; a crafted dictionary with many thousands of distinct keys must neither make
/// the duplicate check quadratic nor the line unbounded.
pub const MAX_UNDECODED_FIELDS: usize = 64;

/// Adds to `out` the slots of `actual` that are not decoded and differ from `sampled`, each once
/// and at most [`MAX_UNDECODED_FIELDS`] in all.
fn undecoded_fields(actual: &V<'_>, sampled: Option<&V<'_>>, decoded: &[&str], neutral: &[(&str, &[i64])], out: &mut Vec<String>) {
    let V::Dict(pairs) = actual else { return };
    for (key, value) in pairs {
        if out.len() >= MAX_UNDECODED_FIELDS {
            return;
        }
        if decoded.contains(&key.as_ref()) || out.iter().any(|o| o == key) {
            continue;
        }
        if neutral.iter().any(|(slot, values)| slot == key && value.as_i64().is_some_and(|v| values.contains(&v))) {
            continue;
        }
        let differs = match sampled.and_then(|s| s.get(key)) {
            Some(default) => !same(value, default),
            None => !is_neutral(value),
        };
        if differs {
            out.push(key.to_string());
        }
    }
}

/// The verified fields of a layer's text document.
#[derive(Debug, Clone, PartialEq)]
pub struct TextDocument {
    /// The text without the trailing paragraph terminators (`\r`).
    pub text: String,
    /// The text as stored (a trailing `\r` ends the paragraph).
    pub raw_text: String,
    /// The font size of the first style run (100 is the default).
    pub font_size: f64,
    /// The fill colour of the first style run as `[A, R, G, B]` floats 0..1.
    pub fill_argb: [f64; 4],
    /// The index of the first style run's font in the document's font list.
    pub font_index: usize,
    /// The font (PostScript name) of the first style run.
    pub font: Option<String>,
    /// The document's font list, in stored order (an entry whose name is unreadable is empty, so
    /// the indices of the style runs stay valid). It always holds entries no run uses
    /// (`Myriad-Roman`, `MyriadPro-Regular`, `AdobeInvisFont` in every sample).
    pub fonts: Vec<String>,
    /// The paragraph runs: one per paragraph, with its length in characters (including the
    /// terminating `\r`) and its justification.
    pub paragraphs: Vec<ParagraphRun>,
    /// The style runs: consecutive characters with one style (a range style splits `Hello` into
    /// runs of 1, 2 and 3 characters).
    pub style_runs: Vec<StyleRun>,
    /// The text box of paragraph (box) text; `None` for point text.
    pub text_box: Option<TextBox>,
    /// Style and paragraph runs that were damaged: a run without a readable length is left out, a
    /// run with a missing size, fill or font index takes them from the run before it (the first
    /// run from the sampled defaults).
    pub incomplete_runs: usize,
    /// Style slots that are not decoded and differ from the sampled default (`"68"`, ...), each
    /// once and at most [`MAX_UNDECODED_FIELDS`]; empty for every synthetic sample.
    pub undecoded_style_fields: Vec<String>,
    /// Paragraph slots that are not decoded and differ from the sampled default.
    pub undecoded_paragraph_fields: Vec<String>,
    /// Manual kerning (`RUN/8/0`, 554): runs of characters with their kerning amount in 1/1000 em;
    /// a run without an amount has none. Empty when no character is kerned by hand.
    pub kerning: Vec<KernRun>,
    /// The line orientation (`/0/8/0[0]/0/2/1`): absent for horizontal text (538), 2 for vertical
    /// right to left (575).
    pub line_orientation: Option<i64>,
}

/// Characters with one manual kerning amount.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct KernRun {
    /// The number of characters.
    pub length: usize,
    /// The amount in 1/1000 em, `None` when the run is not kerned.
    pub amount: Option<f64>,
}

/// The manual kerning list of a text run dictionary (`RUN/8/0`): entries `<< /0 << /0 amount >> /1
/// length >>` (554 kerns each of `HHHH` by 50; the closing `\r` has an entry without an amount).
fn kern_runs(run: &V<'_>) -> Vec<KernRun> {
    let Some(V::Array(items)) = run.get("8").and_then(|v| v.get("0")) else { return Vec::new() };
    let runs: Vec<KernRun> = items
        .iter()
        .filter_map(|e| {
            let length = e.get("1").and_then(usize_of)?;
            let amount = e.get("0").and_then(|v| v.get("0")).and_then(V::as_f64).filter(|v| v.is_finite());
            Some(KernRun { length, amount })
        })
        .collect();
    if runs.iter().any(|r| r.amount.is_some_and(|a| a != 0.0)) { runs } else { Vec::new() }
}

/// The paragraph justification (`RUN/5/0[n]/0/0/5/0`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Justification {
    /// `0`
    Left,
    /// `1` (334; the scripting enum value 7414 is not what is stored)
    Right,
    /// `2`
    Center,
    /// `3`: justified, last line left (565)
    FullLeft,
    /// `4`: justified, last line right (567)
    FullRight,
    /// `5`: justified, last line centred (566)
    FullCenter,
    /// `6`: justified, every line (568)
    FullAll,
    /// Any other value.
    Other(i64),
}

/// A paragraph run. Besides the justification it carries the paragraph settings found with one
/// change each against 538 (`RUN/5/0[n]/0/0/5/<slot>`); a missing slot gives the value of 538.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ParagraphRun {
    /// The number of characters including the `\r` that ends the paragraph.
    pub length: usize,
    pub justification: Justification,
    /// First-line indent in pixels (`/1`, 560: 20).
    pub first_line_indent: f64,
    /// Start (left) indent in pixels (`/2`, 561: 30).
    pub start_indent: f64,
    /// End (right) indent in pixels (`/3`, 562: 40).
    pub end_indent: f64,
    /// Space before in pixels (`/4`, 563: 10).
    pub space_before: f64,
    /// Space after in pixels (`/5`, 564: 12).
    pub space_after: f64,
    /// The leading model (`/8`): 0 Roman, 1 Japanese (572).
    pub leading_type: i64,
    /// Automatic hyphenation (`/9`, on by default; 556 switches it off).
    pub auto_hyphenate: bool,
    /// Roman hanging punctuation (`/21`, 558).
    pub hanging_roman: bool,
    /// Every-line composer (`/29`: off by default, the single-line composer; 559 switches it on).
    pub every_line_composer: bool,
    /// The paragraph direction (`/33`): 0 left to right, 1 right to left (569).
    pub direction: i64,
}

/// The capitalisation of a style run (`RUN/6/0[n]/0/0/6/12`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Caps {
    /// `0`
    Normal,
    /// `1`
    Small,
    /// `2`
    All,
    /// Any other value.
    Other(i64),
}

/// Superscript / subscript of a style run (`RUN/6/0[n]/0/0/6/13`; 526-528: 1 superscript, 0
/// normal, 2 subscript). After Effects draws superscript and subscript at 58.3 % of the size, 33.3 %
/// of the size above or below the baseline (`text.measure` and the layer bounds of 526-528).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FontBaseline {
    /// `0`
    Normal,
    /// `1`
    Superscript,
    /// `2`
    Subscript,
    /// Any other value.
    Other(i64),
}

/// Automatic kerning of a style run (`RUN/6/0[n]/0/0/6/11`; 542, 543). Manual kerning also stores
/// `0` and keeps the amounts in [`TextDocument::kerning`] (554).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoKern {
    /// `0`
    None,
    /// `1`, the default
    Metrics,
    /// `2`
    Optical,
    /// Any other value.
    Other(i64),
}

/// A style run.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StyleRun {
    /// The number of characters in the run.
    pub length: usize,
    pub font_size: f64,
    /// The fill colour as `[A, R, G, B]` floats 0..1.
    pub fill_argb: [f64; 4],
    /// The stroke colour as `[A, R, G, B]` (slot `/6/54`, same layout as the fill; white in every
    /// sample). Whether the stroke is drawn and its width are not decoded.
    pub stroke_argb: Option<[f64; 4]>,
    /// The tracking (`RUN/6/0[n]/0/0/6/8`, 0 by default; 100 in the sample).
    pub tracking: f64,
    /// Faux bold (slot `/6/2`). Setting it also prepends a font-list entry and moves the style's
    /// font index to 1 (335).
    pub faux_bold: bool,
    /// Faux italic (slot `/6/3`, 336).
    pub faux_italic: bool,
    /// All caps / small caps (slot `/6/12`; 337, 338).
    pub caps: Caps,
    /// Automatic leading (slot `/6/4`, 400): when it is set, lines advance by 1.2 x the size and
    /// the stored [`StyleRun::leading`] is not used (400 stores 0.01 there).
    pub auto_leading: bool,
    /// The leading as stored (slot `/6/5`): the baseline-to-baseline distance in pixels when
    /// [`StyleRun::auto_leading`] is off (243: the second line sits 98 px below the first; 339:
    /// 60). `None` when the slot is missing.
    pub leading: Option<f64>,
    /// The index of the run's font in the document's font list (slot `/6/0`).
    pub font_index: usize,
    /// Superscript / subscript (slot `/6/13`); normal when the slot is missing.
    pub baseline: FontBaseline,
    /// Horizontal scale as a factor (slot `/6/6`, 1.0 by default; 539: 0.9).
    pub horizontal_scale: f64,
    /// Vertical scale as a factor (slot `/6/7`; 540: 1.5).
    pub vertical_scale: f64,
    /// Baseline shift in pixels (slot `/6/9`; 541: 41).
    pub baseline_shift: f64,
    /// Automatic kerning (slot `/6/11`).
    pub auto_kern: AutoKern,
    /// Standard ligatures (slot `/6/18`, off by default; 555).
    pub ligatures: bool,
    /// The baseline direction of vertical text (slot `/6/35`): 1 with the stream (default), 2
    /// rotated (573), 3 across the stream, tate-chu-yoko (576).
    pub baseline_direction: i64,
    /// Tsume as a fraction (slot `/6/36`; 553: 0.5).
    pub tsume: f64,
    /// No break (slot `/6/52`; 552).
    pub no_break: bool,
    /// Whether the fill is drawn (slot `/6/56`, on by default; 546).
    pub apply_fill: bool,
    /// Whether the stroke is drawn (slot `/6/57`, off by default; 544).
    pub apply_stroke: bool,
    /// Stroke over fill (slot `/6/58`, on by default; 550 switches it off).
    pub stroke_over_fill: bool,
    /// The stroke line join (slot `/6/62`): 0 miter (default), 1 round (548).
    pub line_join: i64,
    /// The stroke width in pixels (slot `/6/63`, 1.0 by default; 545: 5).
    pub stroke_width: f64,
    /// The digit set (slot `/6/70`): 0 default, 1 Arabic (571), 2 Hindi (578).
    pub digit_set: i64,
}

impl StyleRun {
    /// The style of the sampled untouched document (244), used for a damaged first run.
    pub(crate) const SAMPLED: StyleRun = StyleRun {
        length: 0,
        font_size: 100.0,
        fill_argb: [1.0; 4],
        stroke_argb: None,
        tracking: 0.0,
        faux_bold: false,
        faux_italic: false,
        caps: Caps::Normal,
        auto_leading: false,
        leading: Some(98.0),
        font_index: 0,
        // 244 itself is superscript (the character state the corpus was made in, 526); a damaged
        // run falls back to the normal baseline.
        baseline: FontBaseline::Normal,
        horizontal_scale: 1.0,
        vertical_scale: 1.0,
        baseline_shift: 0.0,
        auto_kern: AutoKern::Metrics,
        ligatures: false,
        baseline_direction: 1,
        tsume: 0.0,
        no_break: false,
        apply_fill: true,
        apply_stroke: false,
        stroke_over_fill: true,
        line_join: 0,
        stroke_width: 1.0,
        digit_set: 0,
    };
}

/// The frame of paragraph (box) text: the box in layer space, as traced by the 16 points of the
/// frame path.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextBox {
    /// The top-left corner.
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// The RGBA colour in a style slot (`slot/0/1`), `None` when it is not four numbers.
fn colour_of(style: &V<'_>, slot: &str) -> Option<[f64; 4]> {
    let c = style.get(slot)?.get("0")?.get("1")?;
    Some([c.at(0)?.as_f64()?, c.at(1)?.as_f64()?, c.at(2)?.as_f64()?, c.at(3)?.as_f64()?])
}

/// A non-negative integer value as `usize`.
fn usize_of(v: &V<'_>) -> Option<usize> {
    usize::try_from(v.as_i64()?).ok()
}

/// A finite number slot of a style, `default` when it is missing or not a finite number.
fn num(style: &V<'_>, slot: &str, default: f64) -> f64 {
    style.get(slot).and_then(V::as_f64).filter(|v| v.is_finite()).unwrap_or(default)
}

/// A boolean slot of a style, `default` when it is missing or not a boolean.
fn flag(style: &V<'_>, slot: &str, default: bool) -> bool {
    match style.get(slot) {
        Some(V::Bool(b)) => *b,
        _ => default,
    }
}

/// What the run lists of one document collect besides the runs.
#[derive(Default)]
struct RunNotes {
    incomplete: usize,
    style_fields: Vec<String>,
    paragraph_fields: Vec<String>,
}

/// The paragraph runs of a text run dictionary (`RUN/5/0`).
fn paragraph_runs(run: &V<'_>, notes: &mut RunNotes) -> Vec<ParagraphRun> {
    let Some(V::Array(items)) = run.get("5").and_then(|v| v.get("0")) else { return Vec::new() };
    let sampled = Some(&*PARAGRAPH_DEFAULTS);
    let mut out: Vec<ParagraphRun> = Vec::with_capacity(items.len());
    for e in items {
        let Some(length) = e.get("1").and_then(usize_of) else {
            notes.incomplete = notes.incomplete.saturating_add(1);
            continue;
        };
        let para = e.get("0").and_then(|v| v.get("0")).and_then(|v| v.get("5"));
        let justification = match para.and_then(|p| p.get("0")).and_then(V::as_i64) {
            Some(0) => Justification::Left,
            Some(1) => Justification::Right,
            Some(2) => Justification::Center,
            Some(3) => Justification::FullLeft,
            Some(4) => Justification::FullRight,
            Some(5) => Justification::FullCenter,
            Some(6) => Justification::FullAll,
            Some(other) => Justification::Other(other),
            None => {
                notes.incomplete = notes.incomplete.saturating_add(1);
                out.last().map_or(Justification::Left, |p| p.justification)
            }
        };
        if let Some(p) = para {
            undecoded_fields(p, sampled, DECODED_PARAGRAPH_SLOTS, NEUTRAL_PARAGRAPH_VALUES, &mut notes.paragraph_fields);
        }
        let num = |slot: &str| para.and_then(|p| p.get(slot)).and_then(V::as_f64).filter(|v| v.is_finite()).unwrap_or(0.0);
        let int = |slot: &str, default: i64| para.and_then(|p| p.get(slot)).and_then(V::as_i64).unwrap_or(default);
        let flag = |slot: &str, default: bool| match para.and_then(|p| p.get(slot)) {
            Some(V::Bool(b)) => *b,
            _ => default,
        };
        out.push(ParagraphRun {
            length,
            justification,
            first_line_indent: num("1"),
            start_indent: num("2"),
            end_indent: num("3"),
            space_before: num("4"),
            space_after: num("5"),
            leading_type: int("8", 0),
            auto_hyphenate: flag("9", true),
            hanging_roman: flag("21", false),
            every_line_composer: flag("29", false),
            direction: int("33", 0),
        });
    }
    out
}

/// The style runs of a text run dictionary (`RUN/6/0`).
fn style_runs(run: &V<'_>, notes: &mut RunNotes) -> Vec<StyleRun> {
    let Some(V::Array(items)) = run.get("6").and_then(|v| v.get("0")) else { return Vec::new() };
    let sampled = Some(&*STYLE_DEFAULTS);
    let mut out: Vec<StyleRun> = Vec::with_capacity(items.len());
    for e in items {
        let Some(length) = e.get("1").and_then(usize_of) else {
            notes.incomplete = notes.incomplete.saturating_add(1);
            continue;
        };
        let prev = out.last().copied().unwrap_or(StyleRun::SAMPLED);
        let Some(style) = e.get("0").and_then(|v| v.get("0")).and_then(|v| v.get("6")) else {
            notes.incomplete = notes.incomplete.saturating_add(1);
            out.push(StyleRun { length, ..prev });
            continue;
        };
        let font_size = style.get("1").and_then(V::as_f64);
        let fill_argb = colour_of(style, "53");
        let font_index = style.get("0").and_then(usize_of);
        if font_size.is_none() || fill_argb.is_none() || font_index.is_none() {
            notes.incomplete = notes.incomplete.saturating_add(1);
        }
        undecoded_fields(style, sampled, DECODED_STYLE_SLOTS, NEUTRAL_STYLE_VALUES, &mut notes.style_fields);
        out.push(StyleRun {
            length,
            font_size: font_size.unwrap_or(prev.font_size),
            fill_argb: fill_argb.unwrap_or(prev.fill_argb),
            stroke_argb: colour_of(style, "54"),
            tracking: style.get("8").and_then(V::as_f64).unwrap_or(0.0),
            faux_bold: matches!(style.get("2"), Some(V::Bool(true))),
            faux_italic: matches!(style.get("3"), Some(V::Bool(true))),
            caps: match style.get("12").and_then(V::as_i64) {
                None | Some(0) => Caps::Normal,
                Some(1) => Caps::Small,
                Some(2) => Caps::All,
                Some(other) => Caps::Other(other),
            },
            auto_leading: matches!(style.get("4"), Some(V::Bool(true))),
            leading: style.get("5").and_then(V::as_f64),
            font_index: font_index.unwrap_or(prev.font_index),
            baseline: match style.get("13").and_then(V::as_i64) {
                None | Some(0) => FontBaseline::Normal,
                Some(1) => FontBaseline::Superscript,
                Some(2) => FontBaseline::Subscript,
                Some(other) => FontBaseline::Other(other),
            },
            horizontal_scale: num(style, "6", 1.0),
            vertical_scale: num(style, "7", 1.0),
            baseline_shift: num(style, "9", 0.0),
            auto_kern: match style.get("11").and_then(V::as_i64) {
                Some(0) => AutoKern::None,
                None | Some(1) => AutoKern::Metrics,
                Some(2) => AutoKern::Optical,
                Some(other) => AutoKern::Other(other),
            },
            ligatures: flag(style, "18", false),
            baseline_direction: style.get("35").and_then(V::as_i64).unwrap_or(1),
            tsume: num(style, "36", 0.0),
            no_break: flag(style, "52", false),
            apply_fill: flag(style, "56", true),
            apply_stroke: flag(style, "57", false),
            stroke_over_fill: flag(style, "58", true),
            line_join: style.get("62").and_then(V::as_i64).unwrap_or(0),
            stroke_width: num(style, "63", 1.0),
            digit_set: style.get("70").and_then(V::as_i64).unwrap_or(0),
        });
    }
    out
}

/// The box of paragraph text from the frame dictionary `/0/8/0[0]/0`: key `/1/0` holds 16 x, y pairs
/// that trace the rectangle, and `/2/0` is 1; both keys are absent for point text.
fn text_box(top: &V<'_>) -> Option<TextBox> {
    let frame = top.get("0")?.get("8")?.get("0")?.at(0)?.get("0")?;
    if frame.get("2")?.get("0")?.as_i64()? != 1 {
        return None;
    }
    let V::Array(coords) = frame.get("1")?.get("0")? else { return None };
    let numbers: Vec<f64> = coords.iter().map(V::as_f64).collect::<Option<_>>()?;
    let (pairs, _) = numbers.as_chunks::<2>();
    if pairs.is_empty() {
        return None;
    }
    let min = |f: fn(&[f64; 2]) -> f64| pairs.iter().map(f).fold(f64::INFINITY, f64::min);
    let max = |f: fn(&[f64; 2]) -> f64| pairs.iter().map(f).fold(f64::NEG_INFINITY, f64::max);
    let (x, y) = (min(|p| p[0]), min(|p| p[1]));
    Some(TextBox { x, y, width: max(|p| p[0]) - x, height: max(|p| p[1]) - y })
}

impl TextDocument {
    /// Decodes the body of a `btdk` list.
    pub fn parse(body: &[u8]) -> Option<Self> {
        let top = parse_dict(body);
        let run = top.get("1")?.get("1")?.at(0)?.get("0")?;
        let raw_text = run.get("0")?.string()?;
        let style = run.get("6")?.get("0")?.at(0)?.get("0")?.get("0")?.get("6")?;
        let fill_argb = colour_of(style, "53")?;
        let font_index = usize::try_from(style.get("0")?.as_i64()?).ok()?;
        let mut fonts = Vec::new();
        if let Some(V::Array(list)) = top.get("0").and_then(|v| v.get("1")).and_then(|v| v.get("0")) {
            fonts = list.iter().map(|f| f.get("0").and_then(|v| v.get("0")).and_then(|v| v.get("0")).and_then(V::string).unwrap_or_default()).collect();
        }
        let mut notes = RunNotes::default();
        let paragraphs = paragraph_runs(run, &mut notes);
        let style_runs = style_runs(run, &mut notes);
        Some(Self {
            text: raw_text.trim_end_matches('\r').to_string(),
            raw_text,
            font_size: style.get("1")?.as_f64()?,
            fill_argb,
            font_index,
            font: fonts.get(font_index).filter(|f| !f.is_empty()).cloned(),
            fonts,
            paragraphs,
            style_runs,
            text_box: text_box(&top),
            incomplete_runs: notes.incomplete,
            undecoded_style_fields: notes.style_fields,
            undecoded_paragraph_fields: notes.paragraph_fields,
            kerning: kern_runs(run),
            line_orientation: top
                .get("0")
                .and_then(|v| v.get("8"))
                .and_then(|v| v.get("0"))
                .and_then(|v| v.at(0))
                .and_then(|v| v.get("0"))
                .and_then(|v| v.get("2"))
                .and_then(|v| v.get("1"))
                .and_then(V::as_i64),
        })
    }

    /// The paragraphs: the stored text split at `\r`, each without its terminator. `A\rB` is stored
    /// as `A\rB\r` and gives `["A", "B"]`.
    pub fn paragraph_texts(&self) -> Vec<&str> {
        self.raw_text.strip_suffix('\r').unwrap_or(&self.raw_text).split('\r').collect()
    }

    /// The fill colour as `[R, G, B]`.
    pub fn fill_rgb(&self) -> [f64; 3] {
        let [_, r, g, b] = self.fill_argb;
        [r, g, b]
    }

    /// The font (PostScript name) of a style run; `None` when its index is outside the font list or
    /// the entry's name is unreadable.
    pub fn font_of(&self, run: &StyleRun) -> Option<&str> {
        self.fonts.get(run.font_index).map(String::as_str).filter(|f| !f.is_empty())
    }
}

impl<'f, 'a> Layer<'f, 'a> {
    /// The Source Text record `LIST btds` (`ADBE Text Document`). It is a direct child of the
    /// `ADBE Text Properties` group, which is a direct child of the layer's root `tdgp`
    /// (`Layr/tdgp/tdgp/btds` in every sample, and where the exporter writes it), so only that
    /// level is searched rather than the whole layer.
    fn source_text_record(&self) -> Option<Node<'f, 'a>> {
        let root = self.chunk.child_list(b"tdgp")?;
        root.children().filter(|g| g.is_list(b"tdgp")).find_map(|g| g.child_list(b"btds"))
    }

    /// The text document of a text layer: the first `LIST btdk` in the Source Text record `btds`
    /// (the same documents [`Layer::text_document_count`] counts).
    pub fn text_document(&self) -> Option<TextDocument> {
        TextDocument::parse(self.source_text_record()?.find_list(b"btdk")?.bytes()?)
    }

    /// The Source Text property record: the `tdbs` in `btds[ADBE Text Document]` (a placeholder
    /// in every sample: no keys, no expression). Source Text keyframes and expressions were not
    /// sampled; a `list` (keys) or an expression in it marks them.
    pub fn source_text_property(&self) -> Option<Property<'f, 'a>> {
        Property::from_chunk(self.source_text_record()?.child_list(b"tdbs")?)
    }

    /// The number of text documents (`LIST btdk`) in the Source Text record: 1 in every sample;
    /// more would be one per Source Text keyframe (not sampled).
    pub fn text_document_count(&self) -> usize {
        self.source_text_record().map_or(0, |btds| btds.descendants().filter(|(_, c)| c.is_list(b"btdk")).count())
    }
}

/// A marker with its comment.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Marker {
    /// The time in the property's time units (layer time for layer markers).
    pub time_units: i32,
    /// The time in seconds.
    pub seconds: Option<f64>,
    /// The comment (the first of the marker's `Utf8` strings).
    pub comment: String,
    /// The duration in seconds (`NmHd`: the `u32` at byte 8 counts 1/600 s, the `u32` at byte 12
    /// is that 600); 0 for a marker without duration.
    pub duration_seconds: Option<f64>,
    /// The chapter (second string).
    pub chapter: String,
    /// The URL (third string).
    pub url: String,
    /// The frame target (fourth string).
    pub frame_target: String,
    /// The cue point name (fifth string).
    pub cue_point_name: String,
    /// The label colour index (`NmHd` byte 16; 3 in 347, 0 by default). It is not in the keyframe
    /// flags.
    pub label: Option<u8>,
    /// The cue type (`NmHd` `u32` at byte 0).
    pub cue_type: Option<CueType>,
    /// The cue point parameters: key and value strings that follow the five fixed strings (the
    /// count is the `u32` at byte 4 of `NmHd`).
    pub cue_params: Vec<(String, String)>,
}

/// The cue point type of a marker (`NmHd` `u32` at byte 0).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CueType {
    /// `4`: an event cue (also for markers without a cue point name).
    Event,
    /// `5`: a navigation cue.
    Navigation,
    /// Any other value.
    Other(u32),
}

impl<'f, 'a> Node<'f, 'a> {
    /// The marker list `LIST mrst` of a marker holder: a layer (`LIST Layr`) or a composition's
    /// `SecL` list. A layer's `mrst` is a direct child of its root `tdgp` (`ADBE Marker`, where
    /// After Effects and the exporter write it), so only that place is looked at; most layers have
    /// none, and searching their whole property tree for it was a large part of an import. Any
    /// other holder is searched depth-first.
    pub(crate) fn marker_list(self) -> Option<Node<'f, 'a>> {
        if self.is_list(b"Layr") {
            return self.child_list(b"tdgp")?.child_list(b"mrst");
        }
        self.find_list(b"mrst")
    }
}

/// Reads the markers below `holder` (a layer or the composition's `SecL` list): the `mrst` list has a
/// property whose 16-byte records give the times and one `Nmrd` list per marker with the strings.
fn markers_in(holder: Node<'_, '_>) -> Vec<Marker> {
    let Some(mrst) = holder.marker_list() else { return Vec::new() };
    let Some(prop) = mrst.child_list(b"tdbs").and_then(Property::from_chunk) else { return Vec::new() };
    let Some(keys) = prop.keys() else { return Vec::new() };
    let scale = prop.time_scale().unwrap_or(0);
    let details = mrst.descendants().map(|(_, c)| c).filter(|c| c.is_list(b"Nmrd"));
    keys.keyframes()
        .zip(details)
        .filter_map(|(k, nmrd)| {
            let time_units = k.time_units()?;
            // The five strings, in the order comment, chapter, URL, frame target, cue point name.
            let mut strings = nmrd.children().filter(|c| c.is(b"Utf8")).map(|c| c.bytes().map(text).unwrap_or_default());
            let mut next = || strings.next().unwrap_or_default();
            let (comment, chapter, url, frame_target, cue_point_name) = (next(), next(), next(), next(), next());
            let extras: Vec<String> = strings.collect();
            let header = nmrd.child(b"NmHd").and_then(Node::bytes);
            let duration_seconds = header.and_then(|h| {
                let (duration, per_second) = (be_u32(h, 8)?, be_u32(h, 12)?);
                (per_second != 0).then(|| f64::from(duration) / f64::from(per_second))
            });
            let pairs = header.and_then(|h| be_u32(h, 4)).and_then(|n| usize::try_from(n).ok()).unwrap_or(0);
            let cue_params = extras.as_chunks::<2>().0.iter().take(pairs).map(|[k, v]| (k.clone(), v.clone())).collect();
            let cue_type = header.and_then(|h| be_u32(h, 0)).map(|t| match t {
                4 => CueType::Event,
                5 => CueType::Navigation,
                other => CueType::Other(other),
            });
            Some(Marker {
                time_units,
                seconds: k.seconds(scale),
                comment,
                duration_seconds,
                chapter,
                url,
                frame_target,
                cue_point_name,
                label: header.and_then(|h| h.get(16).copied()),
                cue_type,
                cue_params,
            })
        })
        .collect()
}

impl<'f, 'a> Layer<'f, 'a> {
    /// The layer's markers (`mrst`), in time order.
    pub fn markers(&self) -> Vec<Marker> {
        markers_in(self.chunk)
    }
}

impl<'f, 'a> Comp<'f, 'a> {
    /// The composition's markers: they live in the `SecL` list named "Markers".
    pub fn markers(&self) -> Vec<Marker> {
        self.item.child_list(b"SecL").map(markers_in).unwrap_or_default()
    }
}

/// The dictionary parser and the defaults as they were before they borrowed from the body (#3276),
/// kept as the reference the differential tests compare with.
#[cfg(test)]
mod oracle {
    use super::{MAX_DEPTH, MAX_UNDECODED_FIELDS, TextValue};
    use crate::container::bytes::text;

    pub(super) fn same(a: &TextValue, b: &TextValue) -> bool {
        match (a, b) {
            (TextValue::Dict(x), TextValue::Dict(y)) => x.len() == y.len() && x.iter().all(|(k, v)| y.iter().any(|(k2, v2)| k == k2 && same(v, v2))),
            (TextValue::Array(x), TextValue::Array(y)) => x.len() == y.len() && x.iter().zip(y).all(|(p, q)| same(p, q)),
            _ => match (a.as_f64(), b.as_f64()) {
                (Some(p), Some(q)) => p == q,
                _ => a == b,
            },
        }
    }

    pub(super) fn is_neutral(v: &TextValue) -> bool {
        match v {
            TextValue::Name(n) => n == "nil",
            TextValue::Bool(b) => !b,
            TextValue::Array(a) => a.is_empty(),
            TextValue::Dict(d) => d.is_empty(),
            other => other.as_f64() == Some(0.0),
        }
    }

    pub(super) fn undecoded_fields(actual: &TextValue, sampled: Option<&TextValue>, decoded: &[&str], neutral: &[(&str, &[i64])], out: &mut Vec<String>) {
        let TextValue::Dict(pairs) = actual else { return };
        for (key, value) in pairs {
            if out.len() >= MAX_UNDECODED_FIELDS {
                return;
            }
            if decoded.contains(&key.as_str()) || out.contains(key) {
                continue;
            }
            if neutral.iter().any(|(slot, values)| slot == key && value.as_i64().is_some_and(|v| values.contains(&v))) {
                continue;
            }
            let differs = match sampled.and_then(|s| s.get(key)) {
                Some(default) => !same(value, default),
                None => !is_neutral(value),
            };
            if differs {
                out.push(key.clone());
            }
        }
    }

    impl TextValue {
        /// The canonical text of a key, as the owning parser built it.
        fn key_text(&self) -> String {
            match self {
                Self::Name(s) | Self::Str(s) | Self::Token(s) => s.clone(),
                Self::Int(i) => i.to_string(),
                Self::Float(f) => f.to_string(),
                Self::Bool(b) => b.to_string(),
                Self::Bytes(b) => text(b),
                Self::Dict(_) | Self::Array(_) => String::new(),
            }
        }
    }

    struct Parser<'b> {
        b: &'b [u8],
        i: usize,
    }

    /// Whitespace between tokens.
    fn is_ws(c: u8) -> bool {
        matches!(c, b' ' | b'\t' | b'\r' | b'\n')
    }

    /// A byte that ends a bare word.
    fn is_delimiter(c: u8) -> bool {
        is_ws(c) || matches!(c, b'/' | b'[' | b']' | b'<' | b'>' | b'(' | b')')
    }

    impl Parser<'_> {
        /// The next byte, `None` at the end.
        fn peek(&self) -> Option<u8> {
            self.b.get(self.i).copied()
        }

        /// Moves past whitespace.
        fn skip_ws(&mut self) {
            while self.peek().is_some_and(is_ws) {
                self.i = self.i.saturating_add(1);
            }
        }

        /// True when the unread bytes start with `s`.
        fn starts_with(&self, s: &[u8]) -> bool {
            self.b.get(self.i..).is_some_and(|rest| rest.starts_with(s))
        }

        /// The bare word at the position (up to the next delimiter), borrowed from the body.
        fn word(&mut self) -> &[u8] {
            let start = self.i;
            while self.peek().is_some_and(|c| !is_delimiter(c)) {
                self.i = self.i.saturating_add(1);
            }
            self.b.get(start..self.i).unwrap_or(&[])
        }

        /// The value at the position, `None` when it is malformed or nested deeper than the limit.
        fn parse(&mut self, depth: usize) -> Option<TextValue> {
            self.skip_ws();
            if depth > MAX_DEPTH {
                return None;
            }
            if self.starts_with(b"<<") {
                self.i = self.i.saturating_add(2);
                let mut pairs = Vec::new();
                loop {
                    self.skip_ws();
                    if self.starts_with(b">>") {
                        self.i = self.i.saturating_add(2);
                        return Some(TextValue::Dict(pairs));
                    }
                    let key = self.parse(depth.saturating_add(1))?;
                    let value = self.parse(depth.saturating_add(1))?;
                    pairs.push((key.key_text(), value));
                }
            }
            match self.peek()? {
                b'[' => {
                    self.i = self.i.saturating_add(1);
                    let mut items = Vec::new();
                    loop {
                        self.skip_ws();
                        if self.peek()? == b']' {
                            self.i = self.i.saturating_add(1);
                            return Some(TextValue::Array(items));
                        }
                        items.push(self.parse(depth.saturating_add(1))?);
                    }
                }
                b'(' => {
                    self.i = self.i.saturating_add(1);
                    let mut out = Vec::new();
                    loop {
                        let c = self.peek()?;
                        if c == b'\\' {
                            out.push(*self.b.get(self.i.saturating_add(1))?);
                            self.i = self.i.saturating_add(2);
                        } else if c == b')' {
                            self.i = self.i.saturating_add(1);
                            break;
                        } else {
                            out.push(c);
                            self.i = self.i.saturating_add(1);
                        }
                    }
                    Some(match out.strip_prefix(&[0xfe, 0xff]) {
                        Some(utf16) => TextValue::Str(decode_utf16be(utf16)),
                        None => TextValue::Bytes(out),
                    })
                }
                b'/' => {
                    self.i = self.i.saturating_add(1);
                    Some(TextValue::Name(text(self.word())))
                }
                _ => {
                    let word = self.word();
                    if word.is_empty() {
                        return None;
                    }
                    let w = text(word);
                    Some(match w.as_str() {
                        "true" => TextValue::Bool(true),
                        "false" => TextValue::Bool(false),
                        _ => match w.parse::<i64>() {
                            Ok(i) => TextValue::Int(i),
                            // `parse` also takes `nan`, `inf` and overflowing exponents; those are not
                            // numbers a text document holds, so they stay tokens and read as missing.
                            Err(_) => match w.parse::<f64>() {
                                Ok(f) if f.is_finite() => TextValue::Float(f),
                                _ => TextValue::Token(w),
                            },
                        },
                    })
                }
            }
        }
    }

    /// A big-endian UTF-16 string as text (unpaired surrogates become U+FFFD).
    fn decode_utf16be(bytes: &[u8]) -> String {
        let (pairs, rest) = bytes.as_chunks::<2>();
        let units = pairs.iter().map(|p| u16::from_be_bytes(*p));
        let mut s: String = char::decode_utf16(units).map(|r| r.unwrap_or(char::REPLACEMENT_CHARACTER)).collect();
        if !rest.is_empty() {
            s.push(char::REPLACEMENT_CHARACTER);
        }
        s
    }

    /// Parses the body of a `btdk` list into its top-level `key value` pairs (as a [`TextValue::Dict`]).
    /// Parsing stops silently at the first syntax error or at nesting deeper than 64 levels; what was
    /// read before is returned.
    pub(super) fn parse_text_dict(body: &[u8]) -> TextValue {
        let mut parser = Parser { b: body, i: 0 };
        let mut pairs = Vec::new();
        loop {
            parser.skip_ws();
            if parser.i >= body.len() {
                break;
            }
            let Some(key) = parser.parse(0) else { break };
            let Some(value) = parser.parse(0) else { break };
            pairs.push((key.key_text(), value));
        }
        TextValue::Dict(pairs)
    }

    fn val(v: f64) -> TextValue {
        TextValue::Float(v)
    }

    /// A dictionary value with these keys, for the built-in defaults.
    fn dict(pairs: Vec<(&str, TextValue)>) -> TextValue {
        TextValue::Dict(pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
    }

    /// An array of numbers, for the built-in defaults.
    fn numbers(v: &[f64]) -> TextValue {
        TextValue::Array(v.iter().map(|x| val(*x)).collect())
    }

    /// The default of every run style slot that is not decoded and not neutral (zero, `false`,
    /// empty, `/nil`): the value an untouched point text holds in every sample (110, 243-245). A slot
    /// with a neutral default is left out; [`undecoded_fields`] treats a missing default as neutral.
    pub(super) fn style_defaults() -> TextValue {
        let paint = |a: f64| dict(vec![("99", TextValue::Name("SimplePaint".into())), ("0", dict(vec![("0", val(1.0)), ("1", numbers(&[1.0, 1.0, 1.0, a]))]))]);
        let class = |name: &str| dict(vec![("99", TextValue::Name(name.into()))]);
        dict(vec![
            ("37", val(2.0)),
            ("41", val(2.0)),
            ("43", dict(vec![("0", val(0.5))])),
            ("44", val(2.0)),
            ("45", val(2.0)),
            ("46", val(7.0)),
            ("49", val(-1.0)),
            ("50", val(-1.0)),
            ("55", class("SimpleBlender")),
            ("64", val(4.0)),
            ("71", val(4.0)),
            ("77", TextValue::Bool(true)),
            ("78", TextValue::Bool(true)),
            ("79", paint(0.0)),
            ("82", val(3.0)),
            ("83", val(3.0)),
            ("86", class("SimpleCustomFeature")),
            ("87", val(100.0)),
            ("88", TextValue::Bool(true)),
            ("95", dict(vec![("0", val(0.0)), ("1", val(0.0))])),
            ("96", TextValue::Bool(true)),
        ])
    }

    /// The same for the paragraph slots (110, 243-245).
    pub(super) fn paragraph_defaults() -> TextValue {
        dict(vec![
            ("6", val(1.0)),
            ("7", val(1.2)),
            ("10", val(6.0)),
            ("11", val(2.0)),
            ("12", val(2.0)),
            ("14", val(36.0)),
            ("15", TextValue::Bool(true)),
            ("16", val(0.5)),
            ("17", numbers(&[0.8, 1.0, 1.33])),
            ("18", numbers(&[0.0, 0.0, 0.0])),
            ("19", numbers(&[1.0, 1.0, 1.0])),
            ("20", val(6.0)),
            ("22", val(1.0)),
            ("23", TextValue::Bool(true)),
            ("24", val(1.0)),
            ("26", TextValue::Bool(true)),
            ("31", val(36.0)),
            ("35", val(1.0)),
            ("40", val(2.0)),
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::container::Chunk;

    /// A `LIST` of this type holding `kids`.
    fn list(t: &[u8; 4], kids: Vec<Chunk<'static>>) -> Chunk<'static> {
        Chunk::list(*t, kids)
    }

    /// A document with every kind of value, close to what After Effects writes: names, numbers,
    /// a UTF-16 string with escapes, a plain string, nested dictionaries and arrays, an undecoded
    /// style slot, a font list and a box frame.
    const DOC: &[u8] = b"/0 << /1 << /0 [ << /0 << /0 << /0 (\xfe\xff\0M\0y\0r) >> /1 1 >> >> << /0 << /0 << /0 (\xfe\xff\0A\0\\)\0b) >> >> >> ] >> \
        /8 << /0 [ << /0 << /1 << /0 [ 0 0 10 0 10 5 0 5 ] >> /2 << /0 1 /1 2 >> >> >> ] >> >> \
        /1 << /1 [ << /0 << /0 (\xfe\xff\0H\0i\0\r) /5 << /0 [ << /0 << /0 << /5 << /0 2 /9 false /24 0 /34 7 /17 [ .8 1.0 1.33 ] >> >> >> /1 3 >> ] >> \
        /6 << /0 [ << /0 << /0 << /6 << /0 1 /1 36.5 /53 << /0 << /0 1 /1 [ 1 0.5 0 1 ] >> >> /79 << /99 /SimplePaint /0 << /0 1 /1 [ 1 1 1 0 ] >> >> /68 1 /90 /x /91 (plain) /92 word >> >> >> /1 3 >> ] >> \
        /8 << /0 [ << /0 << /0 50 >> /1 2 >> << /1 1 >> ] >> >> >> ] >>";

    /// A deterministic xorshift generator: the tests need varied input, not a dependency.
    struct Rng(u64);

    impl Rng {
        /// The next pseudo-random number (xorshift).
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }

        /// A number below `n` (0 for `n == 0`).
        fn below(&mut self, n: usize) -> usize {
            usize::try_from(self.next() % (n as u64)).unwrap_or(0)
        }
    }

    /// The borrowing parser reads what the owning one read.
    #[test]
    fn the_borrowing_parser_reads_what_the_owning_one_read() {
        // Every key form the owned keys canonicalise (`06`, `+6`, `-0`, `1.0`, `1e2`, strings,
        // booleans, containers), names that are not UTF-8, escapes, unclosed strings and escapes,
        // non-finite numbers, and bare words.
        let parts: &[&[u8]] = &[
            b"<<",
            b">>",
            b"[",
            b"]",
            b"/0",
            b"/1",
            b"/nil",
            b"/SimplePaint",
            b"/\xff\xfe",
            b"/\xc3\xa9",
            b"/",
            b"06",
            b"+6",
            b"-0",
            b"1.0",
            b"1.50",
            b"1e2",
            b"-2.5",
            b".8",
            b"inf",
            b"nan",
            b"1e999",
            b"9223372036854775808",
            b"true",
            b"false",
            b"(abc)",
            b"(\xfe\xff\0A\0B)",
            b"(\xfe\xff\0A\0)",
            b"(a\\)b)",
            b"(\\",
            b"(\xfe\xff\\)\0)",
            b"(\\\xfe\xff\0C)",
            b"()",
            b"(\xfe\xff)",
            b"(unclosed",
            b"word",
            b"\xc3\x28",
            b"\xff",
            b" ",
            b"\n",
            b"\t\r",
            b"<",
            b">",
            b")",
        ];
        // A value: mostly well formed, with a random part in its place now and then.
        /// Appends one generated value to `out`: mostly well formed, now and then a random part.
        fn value(rng: &mut Rng, parts: &[&[u8]], depth: usize, out: &mut Vec<u8>) {
            let pick = |rng: &mut Rng| parts.get(rng.below(parts.len())).copied().unwrap_or_default();
            match rng.below(if depth > 6 { 5 } else { 10 }) {
                0 => out.extend_from_slice(pick(rng)),
                1..=4 => out.extend_from_slice([&b"/0"[..], b"1", b"(\xfe\xff\0z)", b"0.5", b"/nil"].get(rng.below(5)).copied().unwrap_or_default()),
                5..=7 => {
                    out.extend_from_slice(b"<< ");
                    for _ in 0..rng.below(5) {
                        let key = if rng.below(4) == 0 { pick(rng) } else { [&b"/0"[..], b"/1", b"/6", b"/53"].get(rng.below(4)).copied().unwrap_or_default() };
                        out.extend_from_slice(key);
                        out.push(b' ');
                        value(rng, parts, depth + 1, out);
                        out.push(b' ');
                    }
                    if rng.below(20) != 0 {
                        out.extend_from_slice(b">>");
                    }
                }
                _ => {
                    out.extend_from_slice(b"[ ");
                    for _ in 0..rng.below(5) {
                        value(rng, parts, depth + 1, out);
                        out.push(b' ');
                    }
                    if rng.below(20) != 0 {
                        out.push(b']');
                    }
                }
            }
        }
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
        let (mut read, mut nested) = (0, 0);
        for case in 0..40_000 {
            let mut body = Vec::new();
            if case % 2 == 0 {
                for _ in 0..rng.below(60) {
                    body.extend_from_slice(parts.get(rng.below(parts.len())).copied().unwrap_or_default());
                    if rng.below(3) != 0 {
                        body.push(b' ');
                    }
                }
            } else {
                for _ in 0..1 + rng.below(4) {
                    body.extend_from_slice(b"/0 ");
                    value(&mut rng, parts, 0, &mut body);
                    body.push(b'\n');
                }
            }
            let parsed = parse_text_dict(&body);
            assert_eq!(parsed, oracle::parse_text_dict(&body), "case {case}: {}", String::from_utf8_lossy(&body));
            let _ = TextDocument::parse(&body);
            if let TextValue::Dict(pairs) = &parsed {
                read += usize::from(!pairs.is_empty());
                nested += usize::from(pairs.iter().any(|(_, v)| matches!(v, TextValue::Dict(d) if !d.is_empty())));
            }
        }
        // The input is not all rejected at the first token.
        assert!(read > 25_000 && nested > 3_000, "{read} cases read something, {nested} a dictionary");
        // Every cut of a realistic document, and nesting past the limit.
        for end in 0..=DOC.len() {
            let body = DOC.get(..end).unwrap_or_default();
            assert_eq!(parse_text_dict(body), oracle::parse_text_dict(body), "cut at {end}");
            let _ = TextDocument::parse(body);
        }
        for depth in [63, 64, 65, 66, 200] {
            let body = [b"/k ".to_vec(), b"<< /a ".repeat(depth), b"1".to_vec(), b" >>".repeat(depth), b" /z 2".to_vec()].concat();
            assert_eq!(parse_text_dict(&body), oracle::parse_text_dict(&body), "depth {depth}");
            let body = [b"/k ".to_vec(), b"[ ".repeat(depth), b"]".repeat(depth)].concat();
            assert_eq!(parse_text_dict(&body), oracle::parse_text_dict(&body), "array depth {depth}");
        }
        // Dictionaries and arrays mixed (`<< /a [ << /a [ ...`), closed, and left open exactly
        // at the limit.
        for depth in 60..=68 {
            let open: Vec<u8> = (0..depth).flat_map(|i| if i % 2 == 0 { &b"<< /a "[..] } else { &b"[ "[..] }).copied().collect();
            let close: Vec<u8> = (0..depth).rev().flat_map(|i| if i % 2 == 0 { &b" >>"[..] } else { &b" ]"[..] }).copied().collect();
            for body in [[b"/k ".to_vec(), open.clone(), b"1".to_vec(), close, b" /z 2".to_vec()].concat(), [b"/k ".to_vec(), open, b"1".to_vec()].concat()] {
                assert_eq!(parse_text_dict(&body), oracle::parse_text_dict(&body), "mixed depth {depth}");
                let _ = TextDocument::parse(&body);
            }
        }
    }

    /// A realistic document is read in full.
    #[test]
    fn a_realistic_document_is_read_in_full() {
        let doc = TextDocument::parse(DOC).expect("a document");
        assert_eq!((doc.text.as_str(), doc.font_size, doc.fill_argb, doc.font.as_deref()), ("Hi", 36.5, [1.0, 0.5, 0.0, 1.0], Some("A)b")));
        assert_eq!(doc.fonts, ["Myr", "A)b"]);
        assert_eq!(doc.paragraphs.first().map(|p| (p.length, p.justification, p.auto_hyphenate)), Some((3, Justification::Center, false)));
        // `/68 1` and `/34 7` are neutral; `/90 /x`, `/91 (plain)` and `/92 word` differ from the
        // (missing, neutral) default; `/79` equals its default.
        assert_eq!(doc.undecoded_style_fields, ["90", "91", "92"]);
        assert!(doc.undecoded_paragraph_fields.is_empty());
        assert_eq!(doc.kerning, [KernRun { length: 2, amount: Some(50.0) }, KernRun { length: 1, amount: None }]);
        assert_eq!(doc.text_box, Some(TextBox { x: 0.0, y: 0.0, width: 10.0, height: 5.0 }));
        assert_eq!(doc.line_orientation, Some(2));
    }

    /// The defaults are the sampled ones and built once.
    #[test]
    fn the_defaults_are_the_sampled_ones_and_built_once() {
        assert_eq!(STYLE_DEFAULTS.clone().into_owned(), oracle::style_defaults());
        assert_eq!(PARAGRAPH_DEFAULTS.clone().into_owned(), oracle::paragraph_defaults());
        assert!(std::ptr::eq(&*STYLE_DEFAULTS, &*STYLE_DEFAULTS));
    }

    /// Names keys and strings are borrowed from the body.
    #[test]
    fn names_keys_and_strings_are_borrowed_from_the_body() {
        let V::Dict(pairs) = parse_dict(b"/a /b /c (\xfe\xff\0x) /d (plain) /e word /f 1.5 /g (\\(esc) 07 1") else { panic!("a dict") };
        // Every `/name` key, name, string without escapes and bare word points into the body.
        assert!(pairs.iter().take(6).all(|(k, _)| matches!(k, Cow::Borrowed(_))));
        assert!(matches!(pairs.first().map(|p| &p.1), Some(V::Name(b"b"))));
        assert!(matches!(pairs.get(1).map(|p| &p.1), Some(V::Str(Cow::Borrowed(b"\0x")))));
        assert!(matches!(pairs.get(2).map(|p| &p.1), Some(V::Bytes(Cow::Borrowed(b"plain")))));
        assert!(matches!(pairs.get(3).map(|p| &p.1), Some(V::Token(b"word"))));
        // An escaped string is copied; a number key is written in its canonical form.
        assert!(matches!(pairs.get(5).map(|p| &p.1), Some(V::Bytes(Cow::Owned(b))) if b == b"(esc"));
        assert_eq!(pairs.get(6).map(|p| p.0.as_ref()), Some("7"));
        // Containers hold what was read.
        let V::Dict(outer) = parse_dict(b"/0 << /a 1 /b 2 /c 3 /d 4 /e 5 >> /1 [ 1 2 3 4 5 6 7 8 9 ]") else { panic!("a dict") };
        assert!(matches!(outer.first().map(|p| &p.1), Some(V::Dict(d)) if d.len() == 5 && d.capacity() >= d.len()));
        assert!(matches!(outer.get(1).map(|p| &p.1), Some(V::Array(a)) if a.len() == 9 && a.capacity() >= a.len()));
    }

    /// Slot checks agree with the owned ones.
    #[test]
    fn slot_checks_agree_with_the_owned_ones() {
        // Slots: defaults, decoded, neutral-listed and unknown ones. Values: each default as
        // stored (also as `Int` where it is `2.0`), near misses, neutral values and every kind.
        let slots = ["0", "6", "7", "17", "24", "34", "35", "37", "43", "55", "68", "77", "79", "86", "95", "96", "200", "x"];
        let values: &[&[u8]] = &[
            b"2",
            b"2.0",
            b"7",
            b"1",
            b"0",
            b"-1",
            b"0.5",
            b"1.2",
            b"36",
            b"true",
            b"false",
            b"/nil",
            b"/x",
            b"/SimpleBlender",
            b"[ ]",
            b"<< >>",
            b"[ .8 1 1.33 ]",
            b"[ 0.8 1.0 ]",
            b"[ 1 1 1 ]",
            b"<< /0 .5 >>",
            b"<< /0 0 /1 0 >>",
            b"<< /1 0 /0 0 >>",
            b"<< /99 /SimpleBlender >>",
            b"<< /99 /SimplePaint /0 << /0 1 /1 [ 1 1 1 0 ] >> >>",
            b"<< /99 /SimplePaint /0 << /0 1 /1 [ 1 1 1 1 ] >> >>",
            b"(plain)",
            b"(\xfe\xff\0a)",
            b"word",
            b"/\xff",
        ];
        let mut rng = Rng(0x2545_f491_4f6c_dd1d);
        let mut listed = 0;
        for case in 0..20_000 {
            let mut body = b"/s << ".to_vec();
            for _ in 0..rng.below(12) {
                body.push(b'/');
                body.extend_from_slice(slots.get(rng.below(slots.len())).copied().unwrap_or_default().as_bytes());
                body.push(b' ');
                body.extend_from_slice(values.get(rng.below(values.len())).copied().unwrap_or_default());
                body.push(b' ');
            }
            body.extend_from_slice(b">>");
            let (new, old) = (parse_dict(&body), oracle::parse_text_dict(&body));
            let (Some(new), Some(old)) = (new.get("s"), old.get("s")) else { panic!("case {case}: a dict") };
            for (sampled, old_sampled, decoded, neutral) in [
                (&*STYLE_DEFAULTS, oracle::style_defaults(), DECODED_STYLE_SLOTS, NEUTRAL_STYLE_VALUES),
                (&*PARAGRAPH_DEFAULTS, oracle::paragraph_defaults(), DECODED_PARAGRAPH_SLOTS, NEUTRAL_PARAGRAPH_VALUES),
            ] {
                let (mut a, mut b) = (Vec::new(), Vec::new());
                undecoded_fields(new, Some(sampled), decoded, neutral, &mut a);
                oracle::undecoded_fields(old, Some(&old_sampled), decoded, neutral, &mut b);
                assert_eq!(a, b, "case {case}: {}", String::from_utf8_lossy(&body));
                listed += a.len();
            }
            // Every pair of values compares and reads as neutral alike.
            if let (V::Dict(x), TextValue::Dict(y)) = (new, old) {
                for ((_, p), (_, q)) in x.iter().zip(y) {
                    assert_eq!(is_neutral(p), oracle::is_neutral(q), "case {case}");
                    for ((_, r), (_, t)) in x.iter().zip(y) {
                        assert_eq!(same(p, r), oracle::same(q, t), "case {case}");
                    }
                }
            }
        }
        assert!(listed > 10_000, "{listed} slots listed");
    }

    /// Values compare by their text.
    #[test]
    fn values_compare_by_their_text() {
        // Two names that are not UTF-8 read as the same text with replacement characters, as the
        // owned values did.
        assert!(same(&V::Name(b"\xff"), &V::Name(b"\xfe")));
        assert!(!same(&V::Name(b"a"), &V::Name(b"b")));
        assert!(same(&V::Str(Cow::Borrowed(b"\xd8\x00")), &V::Str(Cow::Owned(b"\xd8\x01".to_vec()))));
        assert!(same(&V::Int(0), &V::Float(0.0)) && !same(&V::Int(0), &V::Bool(false)) && !same(&V::Name(b"x"), &V::Token(b"x")));
        assert!(is_neutral(&V::Name(b"nil")) && !is_neutral(&V::Token(b"nil")) && is_neutral(&V::Float(0.0)) && !is_neutral(&V::Str(Cow::Borrowed(b""))));
    }

    /// A layers marker list is the one in its root group.
    #[test]
    fn a_layers_marker_list_is_the_one_in_its_root_group() {
        let marked = |tag: u8| list(b"mrst", vec![Chunk::data(*b"test", vec![tag])]);
        // The stored place: `Layr/tdgp/mrst`.
        let layer = list(b"Layr", vec![list(b"tdgp", vec![list(b"tdgp", vec![]), marked(1)])]);
        assert_eq!(Node::from(&layer).marker_list().and_then(|m| m.child(b"test")).and_then(Node::bytes), Some(&[1u8][..]));
        // A list deeper in the layer is not the layer's marker list (the old depth-first search
        // found it); a layer without a marker list in its root group has no markers.
        let deep = list(b"Layr", vec![list(b"tdgp", vec![list(b"tdgp", vec![marked(2)])])]);
        assert!(Node::from(&deep).marker_list().is_none());
        assert!(Layer { chunk: Node::from(&deep) }.markers().is_empty());
        // Any other holder (a composition's `SecL`) is still searched depth-first.
        let secl = list(b"SecL", vec![list(b"tdgp", vec![list(b"tdgp", vec![marked(3)])])]);
        assert_eq!(Node::from(&secl).marker_list().and_then(|m| m.child(b"test")).and_then(Node::bytes), Some(&[3u8][..]));
    }

    /// The source text record is found in the text group.
    #[test]
    fn the_source_text_record_is_found_in_the_text_group() {
        let doc = b"/1 << /1 [ << /0 << /0 (\xfe\xff\0H\0\r) /6 << /0 [ << /0 << /0 << /6 << /0 0 /1 50 /53 << /0 << /1 [ 1 1 0 0 ] >> >> >> >> >> /1 2 >> ] >> >> >> ] >>";
        // `btdk` is an opaque list (its body is not a chunk sequence), as the reader makes it.
        let btdk = || Chunk { id: *b"LIST", body: crate::container::Body::OpaqueList { list_type: *b"btdk", data: doc.to_vec().into() }, pad: None };
        let btds_real = |n: usize| list(b"btds", std::iter::once(list(b"tdbs", vec![])).chain((0..n).map(|_| btdk())).collect());
        let layer = list(b"Layr", vec![list(b"tdgp", vec![list(b"tdgp", vec![]), list(b"tdgp", vec![btds_real(2)])])]);
        let view = Layer { chunk: Node::from(&layer) };
        assert_eq!(view.text_document().map(|d| d.text), Some("H".to_string()));
        assert!(view.source_text_property().is_some());
        assert_eq!(view.text_document_count(), 2);
        // A record anywhere else in the layer is not its Source Text.
        let elsewhere = list(b"Layr", vec![list(b"tdgp", vec![list(b"tdgp", vec![list(b"tdgp", vec![btds_real(1)])])])]);
        let view = Layer { chunk: Node::from(&elsewhere) };
        assert!(view.text_document().is_none() && view.source_text_property().is_none());
        assert_eq!(view.text_document_count(), 0);
    }

    #[test]
    fn non_finite_numbers_are_not_numbers() {
        let TextValue::Dict(pairs) = parse_text_dict(b"/1 inf /2 nan /3 1e999 /4 -inf /5 2.5 /6 7") else { panic!("a dict") };
        let numbers: Vec<Option<f64>> = pairs.iter().map(|(_, v)| v.as_f64()).collect();
        assert_eq!(numbers, [None, None, None, None, Some(2.5), Some(7.0)]);
    }

    #[test]
    fn undecoded_fields_are_capped_and_linear() {
        // 300 000 distinct undecoded slots that differ from a sampled default: before, each was
        // checked against every slot recorded so far (4.5e10 compares) and all were listed.
        let actual = V::Dict((0..300_000).map(|i| (Cow::Owned(format!("{}", 9000 + i)), V::Int(1))).collect());
        let mut out = Vec::new();
        undecoded_fields(&actual, None, &["0"], &[], &mut out);
        assert_eq!(out.len(), MAX_UNDECODED_FIELDS);
        assert_eq!(out.first().map(String::as_str), Some("9000"));
        // A slot listed once is not listed again.
        let again = V::Dict(vec![(Cow::Borrowed("9000"), V::Int(1))]);
        let mut short = vec!["9000".to_string()];
        undecoded_fields(&again, None, &[], &[], &mut short);
        assert_eq!(short, ["9000"]);
    }
}

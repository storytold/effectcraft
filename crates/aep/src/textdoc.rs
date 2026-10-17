//! Text documents (`LIST btdk`) and layer / composition markers.
//!
//! The body of a `btdk` list is not a chunk sequence but a PostScript-like dictionary: `/key
//! value` pairs, `<< >>` dictionaries, `[ ]` arrays, numbers, `true` / `false`, `/names`, and strings
//! as `(` + BOM `fe ff` + UTF-16BE + `)` with `\` escapes. [`parse_text_dict`] reads it (depth
//! limited, never panics) and [`Layer::text_document`] pulls out the verified fields.

use crate::Chunk;
use crate::bytes::{be_u32, text};
use crate::typed::{Comp, Layer, Property};

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

fn is_ws(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\r' | b'\n')
}

fn is_delimiter(c: u8) -> bool {
    is_ws(c) || matches!(c, b'/' | b'[' | b']' | b'<' | b'>' | b'(' | b')')
}

impl Parser<'_> {
    fn peek(&self) -> Option<u8> {
        self.b.get(self.i).copied()
    }

    fn skip_ws(&mut self) {
        while self.peek().is_some_and(is_ws) {
            self.i = self.i.saturating_add(1);
        }
    }

    fn starts_with(&self, s: &[u8]) -> bool {
        self.b.get(self.i..).is_some_and(|rest| rest.starts_with(s))
    }

    fn word(&mut self) -> &[u8] {
        let start = self.i;
        while self.peek().is_some_and(|c| !is_delimiter(c)) {
            self.i = self.i.saturating_add(1);
        }
        self.b.get(start..self.i).unwrap_or(&[])
    }

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
                        Err(_) => match w.parse::<f64>() {
                            Ok(f) => TextValue::Float(f),
                            Err(_) => TextValue::Token(w),
                        },
                    },
                })
            }
        }
    }
}

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
pub fn parse_text_dict(body: &[u8]) -> TextValue {
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
    /// The document's font list.
    pub fonts: Vec<String>,
    /// The paragraph runs: one per paragraph, with its length in characters (including the
    /// terminating `\r`) and its justification.
    pub paragraphs: Vec<ParagraphRun>,
    /// The style runs: consecutive characters with one style (a range style splits `Hello` into
    /// runs of 1, 2 and 3 characters).
    pub style_runs: Vec<StyleRun>,
    /// The text box of paragraph (box) text; `None` for point text.
    pub text_box: Option<TextBox>,
}

/// The paragraph justification (`RUN/5/0[n]/0/0/5/0`). The justified variants were not sampled and
/// are kept as their raw value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Justification {
    /// `0`
    Left,
    /// `1` (334; the scripting enum value 7414 is not what is stored)
    Right,
    /// `2`
    Center,
    /// Any other value.
    Other(i64),
}

/// A paragraph run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParagraphRun {
    /// The number of characters including the `\r` that ends the paragraph.
    pub length: usize,
    pub justification: Justification,
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

/// A style run.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StyleRun {
    /// The number of characters in the run.
    pub length: usize,
    pub font_size: f64,
    /// The fill colour as `[A, R, G, B]` floats 0..1.
    pub fill_argb: [f64; 4],
    /// The tracking (`RUN/6/0[n]/0/0/6/8`, 0 by default; 100 in the sample).
    pub tracking: f64,
    /// Faux bold (slot `/6/2`). Setting it also prepends a font-list entry and moves the style's
    /// font index to 1 (335).
    pub faux_bold: bool,
    /// Faux italic (slot `/6/3`, 336).
    pub faux_italic: bool,
    /// All caps / small caps (slot `/6/12`; 337, 338).
    pub caps: Caps,
    /// The leading as stored (slot `/6/5`): automatic leading is stored as 98.0 for 100 px Times
    /// New Roman; an explicit leading 60 is 60.0 (339). `None` when the slot is missing.
    pub leading: Option<f64>,
    /// The index of the run's font in the document's font list (slot `/6/0`).
    pub font_index: usize,
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

fn fill_of(style: &TextValue) -> Option<[f64; 4]> {
    let fill = style.get("53")?.get("0")?.get("1")?;
    Some([fill.at(0)?.as_f64()?, fill.at(1)?.as_f64()?, fill.at(2)?.as_f64()?, fill.at(3)?.as_f64()?])
}

fn usize_of(v: &TextValue) -> Option<usize> {
    usize::try_from(v.as_i64()?).ok()
}

/// The paragraph runs of a text run dictionary (`RUN/5/0`).
fn paragraph_runs(run: &TextValue) -> Vec<ParagraphRun> {
    let Some(TextValue::Array(items)) = run.get("5").and_then(|v| v.get("0")) else { return Vec::new() };
    items
        .iter()
        .filter_map(|e| {
            let justification = e.get("0")?.get("0")?.get("5")?.get("0")?.as_i64()?;
            Some(ParagraphRun {
                length: usize_of(e.get("1")?)?,
                justification: match justification {
                    0 => Justification::Left,
                    1 => Justification::Right,
                    2 => Justification::Center,
                    other => Justification::Other(other),
                },
            })
        })
        .collect()
}

/// The style runs of a text run dictionary (`RUN/6/0`).
fn style_runs(run: &TextValue) -> Vec<StyleRun> {
    let Some(TextValue::Array(items)) = run.get("6").and_then(|v| v.get("0")) else { return Vec::new() };
    items
        .iter()
        .filter_map(|e| {
            let style = e.get("0")?.get("0")?.get("6")?;
            Some(StyleRun {
                length: usize_of(e.get("1")?)?,
                font_size: style.get("1")?.as_f64()?,
                fill_argb: fill_of(style)?,
                tracking: style.get("8").and_then(TextValue::as_f64).unwrap_or(0.0),
                faux_bold: matches!(style.get("2"), Some(TextValue::Bool(true))),
                faux_italic: matches!(style.get("3"), Some(TextValue::Bool(true))),
                caps: match style.get("12").and_then(TextValue::as_i64) {
                    None | Some(0) => Caps::Normal,
                    Some(1) => Caps::Small,
                    Some(2) => Caps::All,
                    Some(other) => Caps::Other(other),
                },
                leading: style.get("5").and_then(TextValue::as_f64),
                font_index: style.get("0").and_then(usize_of).unwrap_or(0),
            })
        })
        .collect()
}

/// The box of paragraph text from the frame dictionary `/0/8/0[0]/0`: key `/1/0` holds 16 x, y pairs
/// that trace the rectangle, and `/2/0` is 1; both keys are absent for point text.
fn text_box(top: &TextValue) -> Option<TextBox> {
    let frame = top.get("0")?.get("8")?.get("0")?.at(0)?.get("0")?;
    if frame.get("2")?.get("0")?.as_i64()? != 1 {
        return None;
    }
    let TextValue::Array(coords) = frame.get("1")?.get("0")? else { return None };
    let numbers: Vec<f64> = coords.iter().map(TextValue::as_f64).collect::<Option<_>>()?;
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
        let top = parse_text_dict(body);
        let run = top.get("1")?.get("1")?.at(0)?.get("0")?;
        let raw_text = run.get("0")?.as_str()?.to_string();
        let style = run.get("6")?.get("0")?.at(0)?.get("0")?.get("0")?.get("6")?;
        let fill_argb = fill_of(style)?;
        let font_index = usize::try_from(style.get("0")?.as_i64()?).ok()?;
        let mut fonts = Vec::new();
        if let Some(TextValue::Array(list)) = top.get("0").and_then(|v| v.get("1")).and_then(|v| v.get("0")) {
            fonts = list.iter().filter_map(|f| Some(f.get("0")?.get("0")?.get("0")?.as_str()?.to_string())).collect();
        }
        Some(Self {
            text: raw_text.trim_end_matches('\r').to_string(),
            raw_text,
            font_size: style.get("1")?.as_f64()?,
            fill_argb,
            font_index,
            font: fonts.get(font_index).cloned(),
            fonts,
            paragraphs: paragraph_runs(run),
            style_runs: style_runs(run),
            text_box: text_box(&top),
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
}

impl<'f, 'a> Layer<'f, 'a> {
    /// The text document of a text layer (`LIST btdk` below `ADBE Text Properties`).
    pub fn text_document(&self) -> Option<TextDocument> {
        TextDocument::parse(self.chunk.find_list(b"btdk")?.bytes()?)
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

/// Reads the markers below `holder` (a layer or the composition's `SecL` list): the `mrst` list has a
/// property whose 16-byte records give the times and one `Nmrd` list per marker with the strings.
fn markers_in(holder: &Chunk<'_>) -> Vec<Marker> {
    let Some(mrst) = holder.find_list(b"mrst") else { return Vec::new() };
    let Some(prop) = mrst.child_list(b"tdbs").and_then(Property::from_chunk) else { return Vec::new() };
    let Some(keys) = prop.keys() else { return Vec::new() };
    let scale = prop.time_scale().unwrap_or(0);
    let details = mrst.descendants().map(|(_, c)| c).filter(|c| c.is_list(b"Nmrd"));
    keys.keyframes()
        .zip(details)
        .filter_map(|(k, nmrd)| {
            let time_units = k.time_units()?;
            // The five strings, in the order comment, chapter, URL, frame target, cue point name.
            let mut strings = nmrd.children().iter().filter(|c| c.is(b"Utf8")).map(|c| c.bytes().map(text).unwrap_or_default());
            let mut next = || strings.next().unwrap_or_default();
            let (comment, chapter, url, frame_target, cue_point_name) = (next(), next(), next(), next(), next());
            let extras: Vec<String> = strings.collect();
            let header = nmrd.child(b"NmHd").and_then(Chunk::bytes);
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

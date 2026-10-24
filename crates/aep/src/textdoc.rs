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

/// The run style slots (`RUN/6/0[n]/0/0/6/<slot>`) that are decoded; every other slot is compared
/// with the sampled default ([`SAMPLED_STYLE`]) and listed in
/// [`TextDocument::undecoded_style_fields`] when it differs.
pub const DECODED_STYLE_SLOTS: &[&str] =
    &["0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "11", "12", "13", "18", "35", "36", "52", "53", "54", "56", "57", "58", "62", "63", "70"];

/// The paragraph slots (`RUN/5/0[n]/0/0/5/<slot>`) that are decoded (see [`ParagraphRun`]).
pub const DECODED_PARAGRAPH_SLOTS: &[&str] = &["0", "1", "2", "3", "4", "5", "8", "9", "21", "29", "33"];

/// The run style of an untouched point text document as After Effects stores it (244: `Hello`,
/// Times New Roman, 100 px, white fill; 110, 243, 245 store the same slots).
const SAMPLED_STYLE: &[u8] = b"<< /0 0 /1 100.0 /2 false /3 false /4 false /5 98.0 /6 1.0 /7 1.0 /8 0 /9 0.0 /10 0.0 /11 1 /12 0 /13 1 /14 0 /15 0 \
/16 0 /17 0.0 /18 false /19 false /20 false /21 false /22 false /23 false /24 false /25 false /26 false /27 false /28 false /29 false /30 0 \
/31 false /32 false /33 false /34 false /35 1 /36 0.0 /37 2 /38 0 /39 0 /40 false /41 2 /42 0 /43 << /0 .5 >> /44 2 /45 2 /46 7 /47 0 /48 0 \
/49 -1.0 /50 -1.0 /51 0 /52 false /53 << /99 /SimplePaint /0 << /0 1 /1 [ 1.0 1.0 1.0 1.0 ] >> >> /54 << /99 /SimplePaint /0 << /0 1 /1 \
[ 1.0 1.0 1.0 1.0 ] >> >> /55 << /99 /SimpleBlender >> /56 true /57 false /58 true /59 false /60 false /61 0 /62 0 /63 1.0 /64 4.0 /65 0.0 \
/66 [ ] /67 [ ] /68 0 /69 0 /70 0 /71 4 /72 0.0 /73 0.0 /74 false /75 false /76 false /77 true /78 true /79 << /99 /SimplePaint /0 << /0 1 \
/1 [ 1.0 1.0 1.0 0.0 ] >> >> /80 false /81 0 /82 3.0 /83 3.0 /84 false /85 0 /86 << /99 /SimpleCustomFeature >> /87 100.0 /88 true /92 0.0 \
/95 << /0 0.0 /1 0 >> /96 true >>";

/// The paragraph settings of the same document (244).
const SAMPLED_PARAGRAPH: &[u8] = b"<< /0 0 /1 0.0 /2 0.0 /3 0.0 /4 0.0 /5 0.0 /6 1 /7 1.2 /8 0 /9 true /10 6 /11 2 /12 2 /13 0 /14 36.0 /15 true \
/16 .5 /17 [ .8 1.0 1.33 ] /18 [ 0.0 0.0 0.0 ] /19 [ 1.0 1.0 1.0 ] /20 6 /21 false /22 1 /23 true /24 1 /25 0 /27 /nil /26 true /28 /nil \
/29 false /30 << >> /31 36.0 /32 << >> /33 0 /34 0 /35 1 /36 /nil /37 0 /38 false /39 0 /40 2 /41 0 >>";

/// Parses one value (a dictionary) of the text dictionary syntax.
fn parse_value(body: &[u8]) -> Option<TextValue> {
    Parser { b: body, i: 0 }.parse(0)
}

/// Equal values; numbers compare by value (`0` equals `0.0`).
fn same(a: &TextValue, b: &TextValue) -> bool {
    match (a, b) {
        (TextValue::Dict(x), TextValue::Dict(y)) => x.len() == y.len() && x.iter().all(|(k, v)| y.iter().any(|(k2, v2)| k == k2 && same(v, v2))),
        (TextValue::Array(x), TextValue::Array(y)) => x.len() == y.len() && x.iter().zip(y).all(|(p, q)| same(p, q)),
        _ => match (a.as_f64(), b.as_f64()) {
            (Some(p), Some(q)) => p == q,
            _ => a == b,
        },
    }
}

/// A zero, `false`, empty or `/nil` value: a slot that the sampled documents do not have is only
/// listed when it holds something else (a neutral value carries nothing that could be lost).
/// `/nil` is how an untouched document stores a slot without a value (244: paragraph `/27`, `/28`,
/// `/36`).
fn is_neutral(v: &TextValue) -> bool {
    match v {
        TextValue::Name(n) => n == "nil",
        TextValue::Bool(b) => !b,
        TextValue::Array(a) => a.is_empty(),
        TextValue::Dict(d) => d.is_empty(),
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

/// Adds to `out` the slots of `actual` that are not decoded and differ from `sampled`.
fn undecoded_fields(actual: &TextValue, sampled: Option<&TextValue>, decoded: &[&str], neutral: &[(&str, &[i64])], out: &mut Vec<String>) {
    let TextValue::Dict(pairs) = actual else { return };
    for (key, value) in pairs {
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
    /// once; empty for every synthetic sample.
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
fn kern_runs(run: &TextValue) -> Vec<KernRun> {
    let Some(TextValue::Array(items)) = run.get("8").and_then(|v| v.get("0")) else { return Vec::new() };
    let runs: Vec<KernRun> = items
        .iter()
        .filter_map(|e| {
            let length = e.get("1").and_then(usize_of)?;
            let amount = e.get("0").and_then(|v| v.get("0")).and_then(TextValue::as_f64).filter(|v| v.is_finite());
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
    const SAMPLED: StyleRun = StyleRun {
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

fn colour_of(style: &TextValue, slot: &str) -> Option<[f64; 4]> {
    let c = style.get(slot)?.get("0")?.get("1")?;
    Some([c.at(0)?.as_f64()?, c.at(1)?.as_f64()?, c.at(2)?.as_f64()?, c.at(3)?.as_f64()?])
}

fn usize_of(v: &TextValue) -> Option<usize> {
    usize::try_from(v.as_i64()?).ok()
}

/// A finite number slot of a style, `default` when it is missing or not a finite number.
fn num(style: &TextValue, slot: &str, default: f64) -> f64 {
    style.get(slot).and_then(TextValue::as_f64).filter(|v| v.is_finite()).unwrap_or(default)
}

/// A boolean slot of a style, `default` when it is missing or not a boolean.
fn flag(style: &TextValue, slot: &str, default: bool) -> bool {
    match style.get(slot) {
        Some(TextValue::Bool(b)) => *b,
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
fn paragraph_runs(run: &TextValue, notes: &mut RunNotes) -> Vec<ParagraphRun> {
    let Some(TextValue::Array(items)) = run.get("5").and_then(|v| v.get("0")) else { return Vec::new() };
    let sampled = parse_value(SAMPLED_PARAGRAPH);
    let mut out: Vec<ParagraphRun> = Vec::with_capacity(items.len());
    for e in items {
        let Some(length) = e.get("1").and_then(usize_of) else {
            notes.incomplete = notes.incomplete.saturating_add(1);
            continue;
        };
        let para = e.get("0").and_then(|v| v.get("0")).and_then(|v| v.get("5"));
        let justification = match para.and_then(|p| p.get("0")).and_then(TextValue::as_i64) {
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
            undecoded_fields(p, sampled.as_ref(), DECODED_PARAGRAPH_SLOTS, NEUTRAL_PARAGRAPH_VALUES, &mut notes.paragraph_fields);
        }
        let num = |slot: &str| para.and_then(|p| p.get(slot)).and_then(TextValue::as_f64).filter(|v| v.is_finite()).unwrap_or(0.0);
        let int = |slot: &str, default: i64| para.and_then(|p| p.get(slot)).and_then(TextValue::as_i64).unwrap_or(default);
        let flag = |slot: &str, default: bool| match para.and_then(|p| p.get(slot)) {
            Some(TextValue::Bool(b)) => *b,
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
fn style_runs(run: &TextValue, notes: &mut RunNotes) -> Vec<StyleRun> {
    let Some(TextValue::Array(items)) = run.get("6").and_then(|v| v.get("0")) else { return Vec::new() };
    let sampled = parse_value(SAMPLED_STYLE);
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
        let font_size = style.get("1").and_then(TextValue::as_f64);
        let fill_argb = colour_of(style, "53");
        let font_index = style.get("0").and_then(usize_of);
        if font_size.is_none() || fill_argb.is_none() || font_index.is_none() {
            notes.incomplete = notes.incomplete.saturating_add(1);
        }
        undecoded_fields(style, sampled.as_ref(), DECODED_STYLE_SLOTS, NEUTRAL_STYLE_VALUES, &mut notes.style_fields);
        out.push(StyleRun {
            length,
            font_size: font_size.unwrap_or(prev.font_size),
            fill_argb: fill_argb.unwrap_or(prev.fill_argb),
            stroke_argb: colour_of(style, "54"),
            tracking: style.get("8").and_then(TextValue::as_f64).unwrap_or(0.0),
            faux_bold: matches!(style.get("2"), Some(TextValue::Bool(true))),
            faux_italic: matches!(style.get("3"), Some(TextValue::Bool(true))),
            caps: match style.get("12").and_then(TextValue::as_i64) {
                None | Some(0) => Caps::Normal,
                Some(1) => Caps::Small,
                Some(2) => Caps::All,
                Some(other) => Caps::Other(other),
            },
            auto_leading: matches!(style.get("4"), Some(TextValue::Bool(true))),
            leading: style.get("5").and_then(TextValue::as_f64),
            font_index: font_index.unwrap_or(prev.font_index),
            baseline: match style.get("13").and_then(TextValue::as_i64) {
                None | Some(0) => FontBaseline::Normal,
                Some(1) => FontBaseline::Superscript,
                Some(2) => FontBaseline::Subscript,
                Some(other) => FontBaseline::Other(other),
            },
            horizontal_scale: num(style, "6", 1.0),
            vertical_scale: num(style, "7", 1.0),
            baseline_shift: num(style, "9", 0.0),
            auto_kern: match style.get("11").and_then(TextValue::as_i64) {
                Some(0) => AutoKern::None,
                None | Some(1) => AutoKern::Metrics,
                Some(2) => AutoKern::Optical,
                Some(other) => AutoKern::Other(other),
            },
            ligatures: flag(style, "18", false),
            baseline_direction: style.get("35").and_then(TextValue::as_i64).unwrap_or(1),
            tsume: num(style, "36", 0.0),
            no_break: flag(style, "52", false),
            apply_fill: flag(style, "56", true),
            apply_stroke: flag(style, "57", false),
            stroke_over_fill: flag(style, "58", true),
            line_join: style.get("62").and_then(TextValue::as_i64).unwrap_or(0),
            stroke_width: num(style, "63", 1.0),
            digit_set: style.get("70").and_then(TextValue::as_i64).unwrap_or(0),
        });
    }
    out
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
        let fill_argb = colour_of(style, "53")?;
        let font_index = usize::try_from(style.get("0")?.as_i64()?).ok()?;
        let mut fonts = Vec::new();
        if let Some(TextValue::Array(list)) = top.get("0").and_then(|v| v.get("1")).and_then(|v| v.get("0")) {
            fonts = list
                .iter()
                .map(|f| f.get("0").and_then(|v| v.get("0")).and_then(|v| v.get("0")).and_then(TextValue::as_str).unwrap_or_default().to_string())
                .collect();
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
                .and_then(TextValue::as_i64),
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
    /// The text document of a text layer (`LIST btdk` below `ADBE Text Properties`).
    pub fn text_document(&self) -> Option<TextDocument> {
        TextDocument::parse(self.chunk.find_list(b"btdk")?.bytes()?)
    }

    /// The Source Text property record: the `tdbs` in `btds[ADBE Text Document]` (a placeholder
    /// in every sample: no keys, no expression). Source Text keyframes and expressions were not
    /// sampled; a `list` (keys) or an expression in it marks them.
    pub fn source_text_property(&self) -> Option<Property<'f, 'a>> {
        let btds = self.chunk.descendants().map(|(_, c)| c).find(|c| c.is_list(b"btds"))?;
        Property::from_chunk(btds.child_list(b"tdbs")?)
    }

    /// The number of text documents (`LIST btdk`) of the layer: 1 in every sample; more would be
    /// one per Source Text keyframe (not sampled).
    pub fn text_document_count(&self) -> usize {
        self.chunk.descendants().filter(|(_, c)| c.is_list(b"btdk")).count()
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

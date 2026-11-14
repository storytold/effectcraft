//! Source Text documents: text with character style runs and per-paragraph settings.
//!
//! A [`TextDoc`] keeps its *base* character style and paragraph settings in flat fields (the
//! style of the first character and the settings of the first paragraph), which is all a
//! single-style document needs and what older projects contain. Mixed formatting is stored in
//! [`TextDoc::runs`] (character style runs covering the text, by character count) and
//! [`TextDoc::paragraphs`] (one entry per CR, LF or CRLF-separated paragraph). Both lists are
//! empty while the document is uniform, so single-style documents serialize exactly as before.
//!
//! The base document of an Essential Properties override ([`TextDoc::merge_master`]) also keeps
//! [`TextDoc::follow_runs`] and [`TextDoc::follow_paras`]: the base each run and paragraph of the
//! copy follows, so ownership of mixed instance formatting is per run, not per document. Both are
//! empty (and not written) everywhere else, and in projects saved before they existed.
//!
//! Positions are **character** indices (Unicode scalar values), as in After Effects' expression
//! and scripting APIs.

use std::ops::Range;

use serde::{Deserialize, Serialize};
use serde_json::Value as J;

/// A paragraph's original byte and character ranges, excluding its line break.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParagraphRange {
    pub bytes: Range<usize>,
    pub chars: Range<usize>,
}

/// Paragraphs separated by CR, LF or CRLF (one break), including empty/trailing paragraphs.
/// The source is never normalized, so formatting and caret indices keep their original offsets.
pub fn paragraph_ranges(text: &str) -> impl Iterator<Item = ParagraphRange> + '_ {
    let mut chars = text.char_indices().enumerate().peekable();
    let (mut byte_start, mut char_start, mut char_end) = (0, 0, 0);
    let mut done = false;
    std::iter::from_fn(move || {
        if done {
            return None;
        }
        while let Some((ci, (bi, ch))) = chars.next() {
            char_end = ci.saturating_add(1);
            if !matches!(ch, '\r' | '\n') {
                continue;
            }
            let range = ParagraphRange { bytes: byte_start..bi, chars: char_start..ci };
            byte_start = bi.saturating_add(1);
            char_start = char_end;
            if ch == '\r'
                && matches!(chars.peek(), Some((_, (_, '\n'))))
                && let Some((ci, (bi, _))) = chars.next()
            {
                byte_start = bi.saturating_add(1);
                char_end = ci.saturating_add(1);
                char_start = char_end;
            }
            return Some(range);
        }
        done = true;
        Some(ParagraphRange { bytes: byte_start..text.len(), chars: char_start..char_end })
    })
}

/// Horizontal paragraph alignment (AE's seven Paragraph panel buttons).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Justify {
    #[default]
    Left,
    Center,
    Right,
    JustifyLastLeft,
    JustifyLastCenter,
    JustifyLastRight,
    JustifyAll,
}

impl Justify {
    pub const ALL: [Justify; 7] =
        [Justify::Left, Justify::Center, Justify::Right, Justify::JustifyLastLeft, Justify::JustifyLastCenter, Justify::JustifyLastRight, Justify::JustifyAll];
    /// The `layer.setText` / expression name.
    pub fn key(self) -> &'static str {
        match self {
            Justify::Left => "left",
            Justify::Center => "center",
            Justify::Right => "right",
            Justify::JustifyLastLeft => "justifyLeft",
            Justify::JustifyLastCenter => "justifyCenter",
            Justify::JustifyLastRight => "justifyRight",
            Justify::JustifyAll => "justifyAll",
        }
    }
    pub fn parse(s: &str) -> Option<Justify> {
        Some(match s.to_ascii_lowercase().replace(['_', ' ', '-'], "").as_str() {
            "left" | "leftjustify" | "alignleft" => Justify::Left,
            "center" | "centre" | "centerjustify" | "aligncenter" => Justify::Center,
            "right" | "rightjustify" | "alignright" => Justify::Right,
            "justify" | "justifyleft" | "justifylastleft" | "fulljustifylastlineleft" => Justify::JustifyLastLeft,
            "justifycenter" | "justifylastcenter" | "fulljustifylastlinecenter" => Justify::JustifyLastCenter,
            "justifyright" | "justifylastright" | "fulljustifylastlineright" => Justify::JustifyLastRight,
            "justifyall" | "justifyfull" | "fulljustifylastlinefull" | "fulljustify" => Justify::JustifyAll,
            _ => return None,
        })
    }
}

/// Kerning between a character and the one before it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum Kerning {
    /// The font's kerning pairs (`kern` / GPOS).
    #[default]
    Metrics,
    /// Spacing from the glyph outlines.
    Optical,
    /// A manual value in 1/1000 em before this character (replaces metrics for the pair).
    Manual(f64),
}

impl Kerning {
    pub fn parse(v: &J) -> Option<Kerning> {
        match v {
            J::Number(n) => n.as_f64().map(Kerning::Manual),
            J::String(s) => match s.to_ascii_lowercase().as_str() {
                "metrics" | "metric" | "auto" => Some(Kerning::Metrics),
                "optical" => Some(Kerning::Optical),
                "none" | "0" | "manual" => Some(Kerning::Manual(0.0)),
                s => s.parse::<f64>().ok().map(Kerning::Manual),
            },
            _ => None,
        }
    }
    pub fn to_json(self) -> J {
        match self {
            Kerning::Metrics => J::from("metrics"),
            Kerning::Optical => J::from("optical"),
            Kerning::Manual(v) => J::from(v),
        }
    }
}

/// Superscript / subscript.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BaselineOption {
    #[default]
    Normal,
    Superscript,
    Subscript,
}

impl BaselineOption {
    pub fn key(self) -> &'static str {
        match self {
            BaselineOption::Normal => "normal",
            BaselineOption::Superscript => "superscript",
            BaselineOption::Subscript => "subscript",
        }
    }
    pub fn parse(s: &str) -> Option<BaselineOption> {
        Some(match s.to_ascii_lowercase().as_str() {
            "normal" | "none" => BaselineOption::Normal,
            "superscript" | "super" => BaselineOption::Superscript,
            "subscript" | "sub" => BaselineOption::Subscript,
            _ => return None,
        })
    }
}

/// Paragraph direction.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Direction {
    #[default]
    Ltr,
    Rtl,
}

/// Line-breaking composer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Composer {
    /// Balances the breaks of the whole paragraph (AE's default for new text).
    #[default]
    EveryLine,
    /// Breaks each line as soon as it is full.
    SingleLine,
}

fn single_line() -> Composer {
    Composer::SingleLine
}

/// How the corners of a text stroke are drawn (Character panel Line Join).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum LineJoin {
    /// Sharp corners (AE's default for new text).
    #[default]
    Miter,
    Round,
    Bevel,
}

impl LineJoin {
    pub fn parse(s: &str) -> Option<LineJoin> {
        Some(match s.to_ascii_lowercase().as_str() {
            "miter" => LineJoin::Miter,
            "round" => LineJoin::Round,
            "bevel" => LineJoin::Bevel,
            _ => return None,
        })
    }
}

fn round_join() -> LineJoin {
    LineJoin::Round
}
fn yes() -> bool {
    true
}

/// Figure style (OpenType `lnum` / `onum`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FigureStyle {
    /// The font's default figures.
    #[default]
    Default,
    Lining,
    OldStyle,
}

/// Figure spacing (OpenType `pnum` / `tnum`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FigureWidth {
    #[default]
    Default,
    Proportional,
    Tabular,
}

/// OpenType layout features of a run of characters (Character panel ▸ OpenType). Kerning,
/// standard ligatures, small caps and superscript / subscript have their own [`CharStyle`]
/// fields (they use the font's `smcp` / `sups` / `subs` glyphs when it has them and synthesize
/// them otherwise).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default)]
pub struct OpenType {
    /// `dlig`.
    pub discretionary_ligatures: bool,
    /// `calt` (on by default, as in the font).
    pub contextual_alternates: bool,
    /// `salt`.
    pub stylistic_alternates: bool,
    /// Stylistic sets: bit `n - 1` turns on `ssNN` (n = 1…20).
    pub stylistic_sets: u32,
    /// `swsh`.
    pub swash: bool,
    /// `titl`.
    pub titling: bool,
    /// `ordn`.
    pub ordinals: bool,
    /// `frac`.
    pub fractions: bool,
    /// All Small Caps: capitals become small capitals too (`c2sc` with `smcp`).
    pub all_small_caps: bool,
    pub figure_style: FigureStyle,
    pub figure_width: FigureWidth,
}

impl Default for OpenType {
    fn default() -> Self {
        OpenType {
            discretionary_ligatures: false,
            contextual_alternates: true,
            stylistic_alternates: false,
            stylistic_sets: 0,
            swash: false,
            titling: false,
            ordinals: false,
            fractions: false,
            all_small_caps: false,
            figure_style: FigureStyle::Default,
            figure_width: FigureWidth::Default,
        }
    }
}

impl OpenType {
    pub fn is_default(&self) -> bool {
        *self == OpenType::default()
    }
    /// Whether stylistic set `n` (1…20) is on.
    pub fn stylistic_set(&self, n: u32) -> bool {
        (1..=20).contains(&n) && self.stylistic_sets & (1 << (n - 1)) != 0
    }
    pub fn set_stylistic_set(&mut self, n: u32, on: bool) {
        if (1..=20).contains(&n) {
            if on {
                self.stylistic_sets |= 1 << (n - 1);
            } else {
                self.stylistic_sets &= !(1 << (n - 1));
            }
        }
    }
    /// The OpenType feature settings these options ask for (tag, value), beyond the font's
    /// defaults: e.g. `[("dlig", 1), ("ss02", 1), ("calt", 0)]`.
    pub fn feature_settings(&self) -> Vec<([u8; 4], u32)> {
        let mut v = Vec::new();
        for (tag, f) in [
            (b"dlig", self.discretionary_ligatures),
            (b"salt", self.stylistic_alternates),
            (b"swsh", self.swash),
            (b"titl", self.titling),
            (b"ordn", self.ordinals),
            (b"frac", self.fractions),
            (b"c2sc", self.all_small_caps),
            (b"smcp", self.all_small_caps),
            (b"lnum", self.figure_style == FigureStyle::Lining),
            (b"onum", self.figure_style == FigureStyle::OldStyle),
            (b"pnum", self.figure_width == FigureWidth::Proportional),
            (b"tnum", self.figure_width == FigureWidth::Tabular),
        ] {
            if f {
                v.push((*tag, 1));
            }
        }
        if !self.contextual_alternates {
            v.push((*b"calt", 0));
        }
        for n in 1..=20u32 {
            if self.stylistic_set(n) {
                let d = format!("ss{n:02}");
                let b = d.as_bytes();
                v.push(([b[0], b[1], b[2], b[3]], 1));
            }
        }
        v
    }
}

/// The formatting of a run of characters (Character panel).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CharStyle {
    pub font: String,
    pub style: String,
    pub size: f64,
    pub fill: [f32; 4],
    pub stroke: [f32; 4],
    pub stroke_width: f64,
    pub apply_fill: bool,
    pub apply_stroke: bool,
    /// 1/1000 em after each character.
    pub tracking: f64,
    /// Baseline-to-baseline distance; None = auto (120% of the size).
    pub leading: Option<f64>,
    /// Pixels, positive = up.
    pub baseline_shift: f64,
    pub h_scale: f64,
    pub v_scale: f64,
    /// Tsume: percent (0–100) of the space around each character removed.
    pub tsume: f64,
    pub faux_bold: bool,
    pub faux_italic: bool,
    pub all_caps: bool,
    pub small_caps: bool,
    pub baseline: BaselineOption,
    pub kerning: Kerning,
    pub ligatures: bool,
    /// Vertical type: set these characters horizontally within the column (Tate-Chu-Yoko).
    pub tate_chu_yoko: bool,
    /// Vertical type: keep Roman (half-width) characters upright instead of turning them on
    /// their side (Character panel menu ▸ Standard Vertical Roman Alignment).
    pub vertical_roman_upright: bool,
    /// OpenType features (stylistic sets, figures, fractions…).
    #[serde(skip_serializing_if = "OpenType::is_default")]
    pub opentype: OpenType,
    /// Variable font axis values (tag, user units; axes not listed stay at their defaults):
    /// the Character panel's Variable Font Axes.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub variations: Vec<(String, f32)>,
}

impl Default for CharStyle {
    fn default() -> Self {
        TextDoc::default().base_style()
    }
}

impl CharStyle {
    /// The line advance this style asks for (Auto Leading = 120% of the size).
    pub fn line_advance(&self) -> f64 {
        self.leading.unwrap_or(self.size * 1.2)
    }
}

/// A run of `len` characters sharing a style.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StyleRun {
    pub len: usize,
    pub style: CharStyle,
}

/// Paragraph panel settings of one paragraph.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ParaStyle {
    pub justify: Justify,
    /// Start (left in LTR) and end margins, and the first line's extra indent, in pixels.
    pub indent_left: f64,
    pub indent_right: f64,
    pub indent_first: f64,
    pub space_before: f64,
    pub space_after: f64,
    pub direction: Direction,
    pub composer: Composer,
    /// Roman hanging punctuation: punctuation at the edges of paragraph text hangs outside the box.
    pub hanging_punctuation: bool,
}

impl Default for ParaStyle {
    fn default() -> Self {
        TextDoc::default().base_para()
    }
}

/// The value of a text layer's Source Text (hold-interpolated).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TextDoc {
    pub text: String,
    pub font: String,
    pub style: String,
    pub size: f64,
    pub fill: [f32; 4],
    pub stroke: [f32; 4],
    pub stroke_width: f64,
    pub apply_fill: bool,
    pub apply_stroke: bool,
    pub stroke_over_fill: bool,
    /// Documents saved before line joins existed drew round joins.
    #[serde(default = "round_join")]
    pub line_join: LineJoin,
    pub tracking: f64,
    /// None = auto (120% of size).
    pub leading: Option<f64>,
    pub justify: Justify,
    pub baseline_shift: f64,
    pub h_scale: f64,
    pub v_scale: f64,
    pub faux_bold: bool,
    pub faux_italic: bool,
    pub all_caps: bool,
    pub small_caps: bool,
    /// Paragraph text box (width, height) with top-left at `box_pos`; None = point text.
    pub box_size: Option<[f64; 2]>,
    pub box_pos: [f64; 2],
    pub tsume: f64,
    pub baseline: BaselineOption,
    pub kerning: Kerning,
    #[serde(default = "yes")]
    pub ligatures: bool,
    /// Base Tate-Chu-Yoko and Standard Vertical Roman Alignment (see [`CharStyle`]).
    pub tate_chu_yoko: bool,
    pub vertical_roman_upright: bool,
    /// Base OpenType features (see [`CharStyle::opentype`]).
    #[serde(skip_serializing_if = "OpenType::is_default")]
    pub opentype: OpenType,
    /// Base variable font axis values (see [`CharStyle::variations`]).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub variations: Vec<(String, f32)>,
    pub indent_left: f64,
    pub indent_right: f64,
    pub indent_first: f64,
    pub space_before: f64,
    pub space_after: f64,
    pub direction: Direction,
    /// Documents saved before composers existed broke lines greedily.
    #[serde(default = "single_line")]
    pub composer: Composer,
    pub hanging_punctuation: bool,
    /// Vertical type (columns right to left, characters upright).
    pub vertical: bool,
    /// Character style runs covering the text (empty = the whole text uses the base style).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub runs: Vec<StyleRun>,
    /// One entry per paragraph (empty = every paragraph uses the base settings).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub paragraphs: Vec<ParaStyle>,
    /// Only in the base document of an Essential Properties override ([`TextDoc::merge_master`]):
    /// the style each character of the copy follows, as runs over the copy's text (empty = the
    /// base's own style for every character). Kept per run so that a run that did not follow a
    /// restyle keeps the base it still has, whatever the others did.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub follow_runs: Vec<StyleRun>,
    /// The same per paragraph ([`TextDoc::follow_runs`]): the settings each paragraph follows.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub follow_paras: Vec<ParaStyle>,
}

impl Default for TextDoc {
    fn default() -> Self {
        TextDoc {
            text: String::new(),
            font: "Inter".into(),
            style: "Regular".into(),
            size: 72.0,
            fill: [1.0, 1.0, 1.0, 1.0],
            stroke: [0.0, 0.0, 0.0, 1.0],
            stroke_width: 0.0,
            apply_fill: true,
            apply_stroke: false,
            stroke_over_fill: true,
            line_join: LineJoin::Miter,
            tracking: 0.0,
            leading: None,
            justify: Justify::Left,
            baseline_shift: 0.0,
            h_scale: 100.0,
            v_scale: 100.0,
            faux_bold: false,
            faux_italic: false,
            all_caps: false,
            small_caps: false,
            box_size: None,
            box_pos: [0.0, 0.0],
            tsume: 0.0,
            baseline: BaselineOption::Normal,
            kerning: Kerning::Metrics,
            ligatures: true,
            tate_chu_yoko: false,
            vertical_roman_upright: false,
            opentype: OpenType::default(),
            variations: vec![],
            indent_left: 0.0,
            indent_right: 0.0,
            indent_first: 0.0,
            space_before: 0.0,
            space_after: 0.0,
            direction: Direction::Ltr,
            composer: Composer::EveryLine,
            hanging_punctuation: false,
            vertical: false,
            runs: Vec::new(),
            paragraphs: Vec::new(),
            follow_runs: Vec::new(),
            follow_paras: Vec::new(),
        }
    }
}

/// Split `runs` so a run starts at character `at`; returns that run's index.
fn split_at(runs: &mut Vec<StyleRun>, at: usize) -> usize {
    let mut pos = 0;
    for i in 0..runs.len() {
        if pos == at {
            return i;
        }
        let len = runs[i].len;
        if at < pos + len {
            let mut tail = runs[i].clone();
            tail.len = pos + len - at;
            runs[i].len = at - pos;
            runs.insert(i + 1, tail);
            return i + 1;
        }
        pos += len;
    }
    runs.len()
}

impl TextDoc {
    /// A document with `text` in the default style.
    pub fn plain(text: &str) -> TextDoc {
        TextDoc { text: text.into(), ..Default::default() }
    }

    pub fn char_len(&self) -> usize {
        self.text.chars().count()
    }

    /// Byte offset of character `ci` (clamped to the end).
    pub fn byte_of(&self, ci: usize) -> usize {
        self.text.char_indices().nth(ci).map_or(self.text.len(), |(b, _)| b)
    }

    /// Character index of byte offset `b`.
    pub fn char_of_byte(&self, b: usize) -> usize {
        self.text.char_indices().take_while(|(i, _)| *i < b).count()
    }

    /// The text of characters `r`.
    pub fn substring(&self, r: Range<usize>) -> String {
        self.text.chars().skip(r.start).take(r.end.saturating_sub(r.start)).collect()
    }

    /// The base (first character's) style.
    pub fn base_style(&self) -> CharStyle {
        CharStyle {
            font: self.font.clone(),
            style: self.style.clone(),
            size: self.size,
            fill: self.fill,
            stroke: self.stroke,
            stroke_width: self.stroke_width,
            apply_fill: self.apply_fill,
            apply_stroke: self.apply_stroke,
            tracking: self.tracking,
            leading: self.leading,
            baseline_shift: self.baseline_shift,
            h_scale: self.h_scale,
            v_scale: self.v_scale,
            tsume: self.tsume,
            faux_bold: self.faux_bold,
            faux_italic: self.faux_italic,
            all_caps: self.all_caps,
            small_caps: self.small_caps,
            baseline: self.baseline,
            kerning: self.kerning,
            ligatures: self.ligatures,
            tate_chu_yoko: self.tate_chu_yoko,
            vertical_roman_upright: self.vertical_roman_upright,
            opentype: self.opentype,
            variations: self.variations.clone(),
        }
    }

    fn write_base(&mut self, s: &CharStyle) {
        self.font = s.font.clone();
        self.style = s.style.clone();
        self.size = s.size;
        self.fill = s.fill;
        self.stroke = s.stroke;
        self.stroke_width = s.stroke_width;
        self.apply_fill = s.apply_fill;
        self.apply_stroke = s.apply_stroke;
        self.tracking = s.tracking;
        self.leading = s.leading;
        self.baseline_shift = s.baseline_shift;
        self.h_scale = s.h_scale;
        self.v_scale = s.v_scale;
        self.tsume = s.tsume;
        self.faux_bold = s.faux_bold;
        self.faux_italic = s.faux_italic;
        self.all_caps = s.all_caps;
        self.small_caps = s.small_caps;
        self.baseline = s.baseline;
        self.kerning = s.kerning;
        self.ligatures = s.ligatures;
        self.tate_chu_yoko = s.tate_chu_yoko;
        self.vertical_roman_upright = s.vertical_roman_upright;
        self.opentype = s.opentype;
        self.variations = s.variations.clone();
    }

    /// The base (first paragraph's) settings.
    pub fn base_para(&self) -> ParaStyle {
        ParaStyle {
            justify: self.justify,
            indent_left: self.indent_left,
            indent_right: self.indent_right,
            indent_first: self.indent_first,
            space_before: self.space_before,
            space_after: self.space_after,
            direction: self.direction,
            composer: self.composer,
            hanging_punctuation: self.hanging_punctuation,
        }
    }

    fn write_base_para(&mut self, p: &ParaStyle) {
        self.justify = p.justify;
        self.indent_left = p.indent_left;
        self.indent_right = p.indent_right;
        self.indent_first = p.indent_first;
        self.space_before = p.space_before;
        self.space_after = p.space_after;
        self.direction = p.direction;
        self.composer = p.composer;
        self.hanging_punctuation = p.hanging_punctuation;
    }

    /// Whether every character shares one style.
    pub fn is_uniform(&self) -> bool {
        self.runs.is_empty()
    }

    /// Style runs covering the whole text (at least one run, of length 0 for empty text).
    pub fn runs(&self) -> Vec<StyleRun> {
        let n = self.char_len();
        if self.runs.is_empty() {
            return vec![StyleRun { len: n, style: self.base_style() }];
        }
        let mut out = Vec::with_capacity(self.runs.len());
        let mut pos = 0;
        for r in &self.runs {
            if pos >= n {
                break;
            }
            let len = r.len.min(n - pos);
            if len > 0 {
                out.push(StyleRun { len, style: r.style.clone() });
            }
            pos += len;
        }
        match out.last_mut() {
            Some(last) if pos < n => last.len += n - pos,
            None => out.push(StyleRun { len: n, style: self.runs[0].style.clone() }),
            _ => {}
        }
        out
    }

    /// Replace the style runs: zero-length runs are dropped, equal neighbours merged, the base
    /// style becomes the first run's, and a single run collapses to the uniform form.
    pub fn set_runs(&mut self, runs: Vec<StyleRun>) {
        let first = runs.first().map(|r| r.style.clone());
        let mut out: Vec<StyleRun> = Vec::with_capacity(runs.len());
        for r in runs.into_iter().filter(|r| r.len > 0) {
            match out.last_mut() {
                Some(last) if last.style == r.style => last.len += r.len,
                _ => out.push(r),
            }
        }
        if let Some(s) = out.first().map(|r| r.style.clone()).or(first) {
            self.write_base(&s);
        }
        if out.len() <= 1 {
            out.clear();
        }
        self.runs = out;
    }

    /// The style of character `ci` (the last character's past the end).
    pub fn style_at(&self, ci: usize) -> CharStyle {
        if self.runs.is_empty() {
            return self.base_style();
        }
        let mut pos = 0;
        let runs = self.runs();
        for r in &runs {
            if ci < pos + r.len {
                return r.style.clone();
            }
            pos += r.len;
        }
        runs.last().map(|r| r.style.clone()).unwrap_or_else(|| self.base_style())
    }

    /// The style text typed at caret `at` takes: the character before it, or the first one.
    pub fn insertion_style(&self, at: usize) -> CharStyle {
        self.style_at(at.saturating_sub(1))
    }

    /// Change the style of characters `r` (clamped).
    pub fn apply_style(&mut self, r: Range<usize>, mut f: impl FnMut(&mut CharStyle)) {
        let n = self.char_len();
        let (a, b) = (r.start.min(n), r.end.min(n));
        if n == 0 {
            // Empty text: the base style is what the next typed character gets.
            let mut s = self.base_style();
            f(&mut s);
            self.write_base(&s);
            return;
        }
        if a >= b {
            return;
        }
        let mut runs = self.runs();
        let i = split_at(&mut runs, a);
        let j = split_at(&mut runs, b);
        for run in &mut runs[i..j] {
            f(&mut run.style);
        }
        self.set_runs(runs);
    }

    /// Apply a style to every character.
    pub fn apply_style_all(&mut self, f: impl FnMut(&mut CharStyle)) {
        let n = self.char_len();
        self.apply_style(0..n.max(1), f);
    }

    /// Number of paragraphs (CR, LF or CRLF-separated).
    pub fn para_count(&self) -> usize {
        paragraph_ranges(&self.text).count()
    }

    /// Character ranges of the paragraphs (without their newline).
    pub fn para_ranges(&self) -> Vec<Range<usize>> {
        paragraph_ranges(&self.text).map(|p| p.chars).collect()
    }

    /// Paragraph index of character / caret position `ci`.
    pub fn para_of(&self, ci: usize) -> usize {
        let (mut para, mut cr) = (0usize, false);
        for ch in self.text.chars().take(ci) {
            if ch == '\r' || (ch == '\n' && !cr) {
                para = para.saturating_add(1);
            }
            cr = ch == '\r';
        }
        para
    }

    /// Paragraph settings, one per paragraph.
    pub fn paras(&self) -> Vec<ParaStyle> {
        let n = self.para_count();
        let base = self.base_para();
        if self.paragraphs.is_empty() {
            return vec![base; n];
        }
        let mut v: Vec<ParaStyle> = self.paragraphs.iter().take(n).cloned().collect();
        let last = v.last().cloned().unwrap_or(base);
        v.resize(n, last);
        v
    }

    /// Settings of paragraph `i`.
    pub fn para(&self, i: usize) -> ParaStyle {
        if self.paragraphs.is_empty() {
            return self.base_para();
        }
        let v = self.paras();
        v[i.min(v.len() - 1)].clone()
    }

    /// Replace the paragraph settings (collapsed when they're all equal).
    pub fn set_paras(&mut self, mut v: Vec<ParaStyle>) {
        if let Some(first) = v.first().cloned() {
            self.write_base_para(&first);
        }
        if v.iter().all(|p| Some(p) == v.first()) {
            v.clear();
        }
        self.paragraphs = v;
    }

    /// Change the paragraphs touched by characters `r` (an empty range: the caret's paragraph).
    pub fn apply_para(&mut self, r: Range<usize>, mut f: impl FnMut(&mut ParaStyle)) {
        let pa = self.para_of(r.start);
        let pb = if r.end > r.start { self.para_of(r.end - 1) } else { pa };
        let mut v = self.paras();
        for p in v.iter_mut().take(pb + 1).skip(pa) {
            f(p);
        }
        self.set_paras(v);
    }

    /// Replace characters `r` with `s`. The new text takes `style`, or the style of the first
    /// replaced character, or (inserting) the style of the character before the caret.
    pub fn replace_range(&mut self, r: Range<usize>, s: &str, style: Option<&CharStyle>) {
        let n = self.char_len();
        let (a, b) = (r.start.min(n), r.end.min(n).max(r.start.min(n)));
        let st = match style {
            Some(s) => s.clone(),
            None if b > a => self.style_at(a),
            None => self.insertion_style(a),
        };
        let mut runs = self.runs();
        let i = split_at(&mut runs, a);
        let j = split_at(&mut runs, b);
        let ins = s.chars().count();
        runs.splice(i..j, std::iter::once(StyleRun { len: ins, style: st }));
        // Map paragraph starts back to the old text; inserted paragraphs inherit the edited
        // paragraph. Counting breaks in `s` alone loses styles when CR/LF join or split at an
        // edit boundary (including deleting just one half of a CRLF).
        let old_paras = self.paras();
        let starts: Vec<usize> =
            std::iter::once(0).chain(self.para_ranges().into_iter().take(old_paras.len().saturating_sub(1)).map(|r| r.end.saturating_add(1))).collect();
        let base = self.base_para();
        let old_para = |ci| {
            let i = starts.partition_point(|start| *start <= ci).saturating_sub(1);
            old_paras.get(i).cloned().unwrap_or_else(|| base.clone())
        };
        let keep = old_para(a);
        let (ba, bb) = (self.byte_of(a), self.byte_of(b));
        self.text.replace_range(ba..bb, s);
        let end = a.saturating_add(ins);
        let ends_with_break = s.ends_with('\r') || s.ends_with('\n');
        let paras = self
            .para_ranges()
            .into_iter()
            .map(|r| {
                if r.start <= a {
                    old_para(r.start)
                } else if r.start < end || (r.start == end && ends_with_break) {
                    keep.clone()
                } else {
                    old_para(r.start.saturating_sub(ins).saturating_add(b.saturating_sub(a)))
                }
            })
            .collect();
        self.set_runs(runs);
        self.set_paras(paras);
    }

    /// Replace the whole text, keeping the first character's style and the paragraph settings.
    pub fn set_text(&mut self, s: &str) {
        if self.runs.is_empty() && self.paragraphs.is_empty() {
            self.text = s.to_string();
            return;
        }
        let n = self.char_len();
        let st = self.style_at(0);
        self.replace_range(0..n, s, Some(&st));
    }

    /// Characters `r` as their own document (formatting kept).
    pub fn slice(&self, r: Range<usize>) -> TextDoc {
        let n = self.char_len();
        let (a, b) = (r.start.min(n), r.end.min(n).max(r.start.min(n)));
        let mut runs = self.runs();
        let i = split_at(&mut runs, a);
        let j = split_at(&mut runs, b);
        let sub: Vec<StyleRun> = runs[i..j].to_vec();
        let paras = self.paras();
        let (pa, pb) = (self.para_of(a), self.para_of(b));
        let mut d = self.clone();
        d.text = self.substring(a..b);
        d.runs.clear();
        d.paragraphs.clear();
        if sub.is_empty() {
            d.write_base(&self.style_at(a));
        }
        d.set_runs(sub);
        d.set_paras(paras[pa..=pb.min(paras.len() - 1)].to_vec());
        d
    }

    /// Replace characters `r` with another document's text and character formatting.
    pub fn insert_doc(&mut self, r: Range<usize>, frag: &TextDoc) {
        let a = r.start.min(self.char_len());
        self.replace_range(r, &frag.text, None);
        let mut pos = a;
        for run in frag.runs() {
            let st = run.style.clone();
            self.apply_style(pos..pos + run.len, |s| *s = st.clone());
            pos += run.len;
        }
    }

    /// Re-establish the invariants after direct field edits (runs clamped, lists collapsed).
    pub fn normalize(&mut self) {
        let runs = self.runs();
        if !self.runs.is_empty() {
            self.set_runs(runs);
        }
        if !self.paragraphs.is_empty() {
            let p = self.paras();
            self.set_paras(p);
        }
    }

    /// Set a document-level, character or paragraph attribute by its `layer.setText` key over
    /// characters `r` (None = the whole text). Returns false for unknown keys.
    pub fn set_attr(&mut self, key: &str, v: &J, r: Option<Range<usize>>) -> Result<bool, String> {
        match key {
            "strokeOverFill" => {
                self.stroke_over_fill = v.as_bool().ok_or("strokeOverFill: expected a boolean")?;
                return Ok(true);
            }
            "lineJoin" => {
                self.line_join = v.as_str().and_then(LineJoin::parse).ok_or("lineJoin: expected miter|round|bevel")?;
                return Ok(true);
            }
            "vertical" => {
                self.vertical = v.as_bool().ok_or("vertical: expected a boolean")?;
                return Ok(true);
            }
            "box" => {
                match v {
                    J::Null => self.box_size = None,
                    J::Array(a) if a.len() >= 4 => {
                        let g = |i: usize| a[i].as_f64().unwrap_or(0.0);
                        self.box_pos = [g(0), g(1)];
                        self.box_size = Some([g(2).max(1.0), g(3).max(1.0)]);
                    }
                    _ => return Err("box: expected [x, y, width, height] or null".into()),
                }
                return Ok(true);
            }
            _ => {}
        }
        let n = self.char_len();
        let r = r.unwrap_or(0..n);
        let mut probe = ParaStyle::default();
        if apply_para_attr(&mut probe, key, v)? {
            let mut err = None;
            self.apply_para(r, |p| {
                if let Err(e) = apply_para_attr(p, key, v) {
                    err = Some(e);
                }
            });
            return err.map_or(Ok(true), Err);
        }
        let mut probe = CharStyle::default();
        if apply_char_attr(&mut probe, key, v)? {
            if r.start >= r.end && n > 0 {
                return Ok(true);
            }
            // Empty text: apply_style ignores the bounds, so a fixed range avoids `start + 1` overflow.
            let range = if n == 0 { 0..1 } else { r.start..r.end };
            // Check every affected run first, so a value one run refuses changes none of them.
            let mut err = None;
            self.apply_style(range.clone(), |s| {
                if let Err(e) = apply_char_attr(&mut s.clone(), key, v) {
                    err.get_or_insert(e);
                }
            });
            if let Some(e) = err {
                return Err(e);
            }
            self.apply_style(range, |s| {
                let _ = apply_char_attr(s, key, v);
            });
            return Ok(true);
        }
        Ok(false)
    }

    /// Three-way merge of a master document into this copy of it (an Essential Properties
    /// override of a Source Text, #3186). `base_in` is the master the copy is merged with (as it
    /// was when the copy was made, with what the copy followed since brought up to date); `master`
    /// is the master now. The text is always this document's own. For every character, paragraph
    /// and document attribute: where this document still has `base_in`'s value, it takes the
    /// master's, and so does `base_out`; where it has another value, that value is its own and
    /// stays (and `base_out` is left alone, so it stays the copy's own whatever the master does
    /// later). `base_out` starts as a copy of `base_in`. Each document has a base of its own (an
    /// override's keyframes each have theirs), so what one followed never makes another's own
    /// value count as following.
    ///
    /// Character and paragraph attributes are merged only where they are uniform in `master`
    /// (every style run, or every paragraph, the same): a master whose mixed formatting changed
    /// with its text (its bold first word deleted, say) is not taken for a restyle. Where the
    /// attribute is uniform in `base_in`, each run or paragraph is merged against **its own
    /// base**, kept in [`TextDoc::follow_runs`] and [`TextDoc::follow_paras`] of the base (they
    /// are aligned to the copy's text and carried through its edits by [`TextDoc::rebase`]): a
    /// run that followed takes the master's value as its base, a run with its own value keeps the
    /// base it had, so an own value the master later passes through is never taken for
    /// following (blue base, first run red by the instance: master red, then green, leaves the
    /// first run red). Where the attribute is mixed in `base_in` (a master with mixed formatting),
    /// the copy follows only when it still has that formatting as it is (the same values over the
    /// same characters), else it is the copy's own. OpenType settings merge one by one; All Caps,
    /// Small Caps and All Small Caps merge as one choice. Merging again with the updated base
    /// changes nothing.
    pub fn merge_master(&mut self, base_in: &TextDoc, base_out: &mut TextDoc, master: &TextDoc) {
        if same_uniform_formatting(base_in, master) && base_in.follow_runs.is_empty() && base_in.follow_paras.is_empty() {
            return;
        }
        let mut ir = self.runs();
        let (mut bi, mut bo, mr) = (base_in.runs(), base_out.runs(), master.runs());
        let mut fr = follow_or_base(&base_in.follow_runs, &ir, StyleRun { len: 0, style: base_in.base_style() });
        let followed = char_attrs(&mut Work { op: Op::Merge, a: &mut ir, b: &mut bi, c: &mut bo, f: &mut fr, m: &mr, b0: &[] });
        if followed {
            self.set_runs(ir);
            base_out.set_runs(bo);
        }
        base_out.follow_runs = tidy_runs(fr, &base_out.base_style());
        let mut ip = self.paras();
        let (mut bi, mut bo, mp) = (base_in.paras(), base_out.paras(), master.paras());
        let mut fp = follow_or_base(&base_in.follow_paras, &ip, base_in.base_para());
        let followed = para_attrs(&mut Work { op: Op::Merge, a: &mut ip, b: &mut bi, c: &mut bo, f: &mut fp, m: &mp, b0: &[] });
        if followed {
            self.set_paras(ip);
            base_out.set_paras(bo);
        }
        base_out.follow_paras = tidy_paras(fp, &base_out.base_para());
        merge_doc_attrs(self, base_in, base_out, master);
    }

    /// After the copy's own edit (`before` → `after`, an Essential Properties Set of an overridden
    /// Source Text): every attribute the edit changed is set in `base` to the master's value, so
    /// [`TextDoc::merge_master`] keeps the new value as the copy's own (or lets it follow, where
    /// it equals the master's). Without this, a value set back to the base's would count as
    /// following and take the master's at once. Character and paragraph attributes are rebased
    /// **per run and paragraph** (where the master's is uniform): the base's per-run records
    /// ([`TextDoc::follow_runs`], [`TextDoc::follow_paras`]) are carried through the edit by
    /// comparing the texts (what both share at the start and at the end keeps its records, typed
    /// text takes the record of the character before it, deleted text drops its), and only the
    /// runs whose value the edit changed take the master's value. Splitting a run, merging runs
    /// and typing therefore keep every other run's ownership; typing alone is no restyle.
    pub fn rebase(base: &mut TextDoc, before: &TextDoc, after: &TextDoc, master: &TextDoc) {
        let b0 = before.runs();
        let segs = edit_segs(&before.text, &after.text);
        let mut ar = after.runs();
        let mut br = remap_runs(&b0, &segs);
        let mut fr = remap_runs(&follow_or_base(&base.follow_runs, &b0, StyleRun { len: 0, style: base.base_style() }), &segs);
        let (mut xr, mr) = (base.runs(), master.runs());
        if char_attrs(&mut Work { op: Op::Rebase, a: &mut ar, b: &mut br, c: &mut xr, f: &mut fr, m: &mr, b0: &b0 }) {
            base.set_runs(xr);
        }
        base.follow_runs = tidy_runs(fr, &base.base_style());
        let p0 = before.paras();
        let map = para_map(before, after);
        let mut ap = after.paras();
        let mut bp = remap_paras(&p0, &map);
        let mut fp = remap_paras(&follow_or_base(&base.follow_paras, &p0, base.base_para()), &map);
        let (mut xp, mp) = (base.paras(), master.paras());
        if para_attrs(&mut Work { op: Op::Rebase, a: &mut ap, b: &mut bp, c: &mut xp, f: &mut fp, m: &mp, b0: &p0 }) {
            base.set_paras(xp);
        }
        base.follow_paras = tidy_paras(fp, &base.base_para());
        if before.stroke_over_fill != after.stroke_over_fill {
            base.stroke_over_fill = master.stroke_over_fill;
        }
        if before.line_join != after.line_join {
            base.line_join = master.line_join;
        }
        if before.box_size != after.box_size {
            base.box_size = master.box_size;
        }
        if before.box_pos != after.box_pos {
            base.box_pos = master.box_pos;
        }
        if before.vertical != after.vertical {
            base.vertical = master.vertical;
        }
    }
}

/// Whether two documents are uniform (no style runs or per-paragraph settings) and formatted
/// alike, their text aside: then [`TextDoc::merge_master`] has nothing to take.
fn same_uniform_formatting(a: &TextDoc, b: &TextDoc) -> bool {
    a.runs.is_empty()
        && b.runs.is_empty()
        && a.paragraphs.is_empty()
        && b.paragraphs.is_empty()
        && a.base_style() == b.base_style()
        && a.base_para() == b.base_para()
        && a.stroke_over_fill == b.stroke_over_fill
        && a.line_join == b.line_join
        && a.box_size == b.box_size
        && a.box_pos == b.box_pos
        && a.vertical == b.vertical
}

/// [`TextDoc::merge_master`] for the document attributes (the others are merged per style run
/// and paragraph). The destructuring lists every field of [`TextDoc`], so a new one has to be
/// placed here to compile.
fn merge_doc_attrs(v: &mut TextDoc, base_in: &TextDoc, base_out: &mut TextDoc, master: &TextDoc) {
    let TextDoc {
        text: _,
        font: _,
        style: _,
        size: _,
        fill: _,
        stroke: _,
        stroke_width: _,
        apply_fill: _,
        apply_stroke: _,
        stroke_over_fill,
        line_join,
        tracking: _,
        leading: _,
        justify: _,
        baseline_shift: _,
        h_scale: _,
        v_scale: _,
        faux_bold: _,
        faux_italic: _,
        all_caps: _,
        small_caps: _,
        box_size,
        box_pos,
        tsume: _,
        baseline: _,
        kerning: _,
        ligatures: _,
        tate_chu_yoko: _,
        vertical_roman_upright: _,
        opentype: _,
        variations: _,
        indent_left: _,
        indent_right: _,
        indent_first: _,
        space_before: _,
        space_after: _,
        direction: _,
        composer: _,
        hanging_punctuation: _,
        vertical,
        runs: _,
        paragraphs: _,
        follow_runs: _,
        follow_paras: _,
    } = v;
    merge_one(stroke_over_fill, &base_in.stroke_over_fill, &mut base_out.stroke_over_fill, &master.stroke_over_fill);
    merge_one(line_join, &base_in.line_join, &mut base_out.line_join, &master.line_join);
    merge_one(box_size, &base_in.box_size, &mut base_out.box_size, &master.box_size);
    merge_one(box_pos, &base_in.box_pos, &mut base_out.box_pos, &master.box_pos);
    merge_one(vertical, &base_in.vertical, &mut base_out.vertical, &master.vertical);
}

/// [`TextDoc::merge_master`] for one value: `v` (and `base_out`) take `master` when `v` still
/// equals `base_in`.
fn merge_one<T: PartialEq + Clone>(v: &mut T, base_in: &T, base_out: &mut T, master: &T) {
    if *v == *base_in && *base_in != *master {
        *v = master.clone();
        *base_out = master.clone();
    }
}

/// The value every item has for one attribute, `None` when they differ (or there are none).
fn uniform<S, T: PartialEq>(items: &[S], get: fn(&S) -> T) -> Option<T> {
    let mut it = items.iter();
    let first = get(it.next()?);
    for s in it {
        if get(s) != first {
            return None;
        }
    }
    Some(first)
}

/// Style runs and paragraphs, as [`attr_op`] merges them: how many characters (or paragraphs)
/// an item covers.
trait Span {
    fn span(&self) -> usize;
    /// The same item covering `n` instead.
    fn with_span(&self, n: usize) -> Self;
}

impl Span for StyleRun {
    fn span(&self) -> usize {
        self.len
    }
    fn with_span(&self, n: usize) -> Self {
        StyleRun { len: n, style: self.style.clone() }
    }
}

impl Span for ParaStyle {
    fn span(&self) -> usize {
        1
    }
    fn with_span(&self, _: usize) -> Self {
        self.clone()
    }
}

/// One attribute of an item, read and written as one value (a field, or several fields that
/// make one choice, such as the capitalization).
struct Attr<S, T> {
    get: fn(&S) -> T,
    set: fn(&mut S, T),
}

/// [`Attr`] for one field of `$ty`.
macro_rules! field {
    ($ty:ty, $($f:ident).+) => {
        Attr { get: |r: &$ty| r.$($f).+.clone(), set: |r: &mut $ty, v| r.$($f).+ = v }
    };
}

/// One attribute as the document holds it: its values in order, equal neighbours joined, each
/// with how many characters (or paragraphs) it covers.
fn profile<S: Span, T: PartialEq>(items: &[S], get: fn(&S) -> T) -> Vec<(usize, T)> {
    let mut out: Vec<(usize, T)> = Vec::with_capacity(items.len());
    for s in items {
        let v = get(s);
        match out.last_mut() {
            Some(last) if last.1 == v => last.0 = last.0.saturating_add(s.span()),
            _ => out.push((s.span(), v)),
        }
    }
    out
}

/// Whether an edit (`a` → `b`) changed one attribute. With the same length the whole profile
/// counts (a run set to another value inside mixed formatting is a change); when the text's
/// length changed, only the values in order do, so typing alone is no restyle.
fn attr_changed<S: Span, T: PartialEq>(a: &[S], b: &[S], get: fn(&S) -> T) -> bool {
    let (pa, pb) = (profile(a, get), profile(b, get));
    let total = |p: &[(usize, T)]| p.iter().fold(0usize, |n, x| n.saturating_add(x.0));
    if total(&pa) == total(&pb) { pa != pb } else { !pa.iter().map(|x| &x.1).eq(pb.iter().map(|x| &x.1)) }
}

/// What [`char_attrs`] and [`para_attrs`] do with each attribute.
#[derive(Clone, Copy)]
enum Op {
    /// [`TextDoc::merge_master`]: `a` is the copy, `b` the base in, `c` the base out.
    Merge,
    /// [`TextDoc::rebase`]: `a` is the copy after its edit, `b` the copy before it (both over
    /// the text after), `c` the base.
    Rebase,
}

/// The lists [`attr_op`] works on (runs or paragraphs).
struct Work<'a, S> {
    op: Op,
    /// The copy (merge) or the copy after its edit (rebase).
    a: &'a mut Vec<S>,
    /// The base in as the document holds it (merge) or the copy before its edit, carried to the
    /// text after it (rebase).
    b: &'a mut Vec<S>,
    /// The base out (merge) or the base (rebase), as the document holds it.
    c: &'a mut [S],
    /// The base of every run or paragraph of the copy: [`TextDoc::follow_runs`] or
    /// [`TextDoc::follow_paras`], aligned to `a`. Authoritative for an attribute that is uniform
    /// in the document base `c`.
    f: &'a mut Vec<S>,
    /// The master.
    m: &'a [S],
    /// Rebase: the copy before its edit, as it was (`b` carried over the edit).
    b0: &'a [S],
}

fn set_all<S, T: Clone>(items: &mut [S], at: &Attr<S, T>, v: &T) {
    for s in items.iter_mut() {
        (at.set)(s, v.clone());
    }
}

/// One attribute of style runs or paragraphs, only where it is uniform in the master `w.m`.
///
/// Where the base is uniform in the attribute, every run or paragraph has a base of its own
/// (`w.f`). Merge: an item of the copy that has its base's value takes the master's, and so does
/// its base; an item with another value is its own and keeps its base. Rebase: an item whose
/// value the edit changed takes the master's value as its base (the others keep theirs). Then the
/// document base `w.c` takes the value every base has now (the master's where they differ, for
/// a document that lost its records). Where the base is mixed (a master with mixed formatting),
/// the copy follows only when it has the base's formatting as it is (the same [`profile`]), and
/// the whole base takes the master's value; rebase does the same when the edit changed the
/// attribute ([`attr_changed`]). Returns whether the document base `w.c` or the copy changed.
fn attr_op<S: Span + Clone, T: PartialEq + Clone>(w: &mut Work<S>, at: Attr<S, T>) -> bool {
    let Some(m) = uniform(w.m, at.get) else { return false };
    match w.op {
        Op::Merge => {
            if uniform(w.b, at.get).is_none() {
                // Mixed in the base: followed only where the copy left it as it was.
                if profile(w.a, at.get) != profile(w.b, at.get) {
                    return false;
                }
                set_all(w.a, &at, &m);
                set_all(w.c, &at, &m);
                set_all(w.f, &at, &m);
                return true;
            }
            if !align(&mut [&mut *w.a, &mut *w.f]) {
                return false;
            }
            let mut followed = false;
            for (s, base) in w.a.iter_mut().zip(w.f.iter_mut()) {
                let held = (at.get)(base);
                if held != m && (at.get)(s) == held {
                    (at.set)(s, m.clone());
                    (at.set)(base, m.clone());
                    followed = true;
                }
            }
            if followed {
                let v = uniform(w.f, at.get).unwrap_or(m);
                set_all(w.c, &at, &v);
            }
            followed
        }
        Op::Rebase => {
            if uniform(w.c, at.get).is_none() {
                if !attr_changed(w.b0, w.a, at.get) {
                    return false;
                }
                set_all(w.c, &at, &m);
                set_all(w.f, &at, &m);
                return true;
            }
            if !align(&mut [&mut *w.a, &mut *w.b, &mut *w.f]) {
                return false;
            }
            let mut changed = false;
            for ((s, was), base) in w.a.iter().zip(w.b.iter()).zip(w.f.iter_mut()) {
                if (at.get)(s) != (at.get)(was) {
                    (at.set)(base, m.clone());
                    changed = true;
                }
            }
            if changed {
                let v = uniform(w.f, at.get).unwrap_or(m);
                set_all(w.c, &at, &v);
            }
            changed
        }
    }
}

/// Split every list at every boundary of the others, so that they all hold the same spans item
/// by item. False (nothing split) when the lists do not cover the same length.
fn align<S: Span + Clone>(lists: &mut [&mut Vec<S>]) -> bool {
    let total = |l: &[S]| l.iter().fold(0usize, |n, s| n.saturating_add(s.span()));
    let Some(first) = lists.first() else { return true };
    let t = total(first);
    if lists.iter().any(|l| total(l) != t) {
        return false;
    }
    let mut cuts: Vec<usize> = vec![];
    for l in lists.iter() {
        let mut pos = 0usize;
        for s in l.iter() {
            pos = pos.saturating_add(s.span());
            cuts.push(pos);
        }
    }
    cuts.sort_unstable();
    cuts.dedup();
    for l in lists.iter_mut() {
        let mut out = Vec::with_capacity(cuts.len());
        let mut pos = 0usize;
        for s in l.iter() {
            let end = pos.saturating_add(s.span());
            let lo = cuts.partition_point(|&c| c <= pos);
            let hi = cuts.partition_point(|&c| c < end);
            let mut at = pos;
            for &c in cuts.get(lo..hi).unwrap_or(&[]) {
                out.push(s.with_span(c.saturating_sub(at)));
                at = c;
            }
            out.push(s.with_span(end.saturating_sub(at)));
            pos = end;
        }
        **l = out;
    }
    let n = lists.first().map_or(0, |l| l.len());
    lists.iter().all(|l| l.len() == n)
}

/// The per-run (or per-paragraph) base `follow` when it covers the copy `items` exactly, else
/// one base style for every item (the document base's): old projects, or a copy that was
/// edited by something that did not rebase.
fn follow_or_base<S: Span + Clone>(follow: &[S], items: &[S], base: S) -> Vec<S> {
    let total = |l: &[S]| l.iter().fold(0usize, |n, s| n.saturating_add(s.span()));
    if !follow.is_empty() && total(follow) == total(items) {
        return follow.to_vec();
    }
    items.iter().map(|s| base.with_span(s.span())).collect()
}

/// The per-run bases to keep: neighbours with the same base joined, none when every run has the
/// document base's style `base` (it says it all).
fn tidy_runs(runs: Vec<StyleRun>, base: &CharStyle) -> Vec<StyleRun> {
    let mut out: Vec<StyleRun> = Vec::with_capacity(runs.len());
    for r in runs.into_iter().filter(|r| r.len > 0) {
        match out.last_mut() {
            Some(last) if last.style == r.style => last.len = last.len.saturating_add(r.len),
            _ => out.push(r),
        }
    }
    if out.iter().all(|r| r.style == *base) { vec![] } else { out }
}

/// [`tidy_runs`] for paragraphs.
fn tidy_paras(paras: Vec<ParaStyle>, base: &ParaStyle) -> Vec<ParaStyle> {
    if paras.iter().all(|p| p == base) { vec![] } else { paras }
}

/// How the characters after an edit map to those before it: `len` of them, starting at `old`
/// before the edit and going on one by one (`step`), or all like the one character at `old`
/// (typed text).
struct Seg {
    len: usize,
    old: usize,
    step: bool,
}

/// The common start and end of two texts, in characters (the end never overlaps the start), and
/// the two lengths.
fn common_ends(old: &str, new: &str) -> (usize, usize, usize, usize) {
    let (ol, nl) = (old.chars().count(), new.chars().count());
    let p = old.chars().zip(new.chars()).take_while(|(a, b)| a == b).count();
    let room = ol.min(nl).saturating_sub(p);
    let s = old.chars().rev().zip(new.chars().rev()).take_while(|(a, b)| a == b).count().min(room);
    (p, s, ol, nl)
}

/// The edit from `old` to `new` as the texts' common start and end say: the start and the end
/// map to themselves, what lies between is new (it takes what the character before it has).
fn edit_segs(old: &str, new: &str) -> Vec<Seg> {
    let (p, s, ol, nl) = common_ends(old, new);
    let typed = nl.saturating_sub(p).saturating_sub(s);
    let mut v = vec![
        Seg { len: p, old: 0, step: true },
        Seg { len: typed, old: p.saturating_sub(1), step: false },
        Seg { len: s, old: ol.saturating_sub(s), step: true },
    ];
    v.retain(|g| g.len > 0);
    v
}

/// `items` (covering the text before an edit) as they lie over the text after it.
fn remap_runs(items: &[StyleRun], segs: &[Seg]) -> Vec<StyleRun> {
    let mut out = vec![];
    for g in segs {
        if g.step {
            let end = g.old.saturating_add(g.len);
            let mut pos = 0usize;
            for r in items {
                let (rs, re) = (pos, pos.saturating_add(r.len));
                pos = re;
                let (from, to) = (rs.max(g.old), re.min(end));
                if from < to {
                    out.push(StyleRun { len: to - from, style: r.style.clone() });
                }
            }
        } else {
            // Typed text: the run of the character before it (the nearest one at the ends).
            let mut pos = 0usize;
            let mut found = None;
            for r in items {
                pos = pos.saturating_add(r.len);
                found = Some(r);
                if g.old < pos {
                    break;
                }
            }
            if let Some(r) = found {
                out.push(StyleRun { len: g.len, style: r.style.clone() });
            }
        }
    }
    if out.is_empty()
        && let Some(r) = items.first()
    {
        out.push(StyleRun { len: 0, style: r.style.clone() });
    }
    out
}

/// For every paragraph after an edit, the paragraph before it that it continues: the ones at the
/// start and the end of the text keep theirs, a paragraph made inside the edit (Enter pressed)
/// is the paragraph the edit started in.
fn para_map(before: &TextDoc, after: &TextDoc) -> Vec<usize> {
    let (p, s, ol, nl) = common_ends(&before.text, &after.text);
    let last = before.para_count().saturating_sub(1);
    after
        .para_ranges()
        .iter()
        .enumerate()
        .map(|(j, r)| {
            let old = if r.start >= nl.saturating_sub(s) {
                before.para_of(r.start.saturating_add(ol).saturating_sub(nl))
            } else if r.start <= p {
                j
            } else {
                before.para_of(p)
            };
            old.min(last)
        })
        .collect()
}

/// [`remap_runs`] for paragraphs.
fn remap_paras(items: &[ParaStyle], map: &[usize]) -> Vec<ParaStyle> {
    map.iter().filter_map(|&i| items.get(i).cloned()).collect()
}

/// [`attr_op`] for every character attribute; returns whether any did something. The destructuring lists every field
/// of [`CharStyle`], so a new attribute has to be added here to compile.
fn char_attrs(w: &mut Work<StyleRun>) -> bool {
    let Some(StyleRun {
        len: _,
        style:
            CharStyle {
                font: _,
                style: _,
                size: _,
                fill: _,
                stroke: _,
                stroke_width: _,
                apply_fill: _,
                apply_stroke: _,
                tracking: _,
                leading: _,
                baseline_shift: _,
                h_scale: _,
                v_scale: _,
                tsume: _,
                faux_bold: _,
                faux_italic: _,
                all_caps: _,
                small_caps: _,
                baseline: _,
                kerning: _,
                ligatures: _,
                tate_chu_yoko: _,
                vertical_roman_upright: _,
                opentype:
                    OpenType {
                        discretionary_ligatures: _,
                        contextual_alternates: _,
                        stylistic_alternates: _,
                        stylistic_sets: _,
                        swash: _,
                        titling: _,
                        ordinals: _,
                        fractions: _,
                        all_small_caps: _,
                        figure_style: _,
                        figure_width: _,
                    },
                variations: _,
            },
    }) = w.m.first()
    else {
        return false;
    };
    let mut followed = false;
    followed |= attr_op(w, field!(StyleRun, style.font));
    followed |= attr_op(w, field!(StyleRun, style.style));
    followed |= attr_op(w, field!(StyleRun, style.size));
    followed |= attr_op(w, field!(StyleRun, style.fill));
    followed |= attr_op(w, field!(StyleRun, style.stroke));
    followed |= attr_op(w, field!(StyleRun, style.stroke_width));
    followed |= attr_op(w, field!(StyleRun, style.apply_fill));
    followed |= attr_op(w, field!(StyleRun, style.apply_stroke));
    followed |= attr_op(w, field!(StyleRun, style.tracking));
    followed |= attr_op(w, field!(StyleRun, style.leading));
    followed |= attr_op(w, field!(StyleRun, style.baseline_shift));
    followed |= attr_op(w, field!(StyleRun, style.h_scale));
    followed |= attr_op(w, field!(StyleRun, style.v_scale));
    followed |= attr_op(w, field!(StyleRun, style.tsume));
    followed |= attr_op(w, field!(StyleRun, style.faux_bold));
    followed |= attr_op(w, field!(StyleRun, style.faux_italic));
    // All Caps, Small Caps and All Small Caps are one choice (setting one turns the others off,
    // see `apply_char_attr`): merged as one value, so an instance's own choice stays whole and
    // the master's never joins it.
    followed |= attr_op(
        w,
        Attr {
            get: |r: &StyleRun| (r.style.all_caps, r.style.small_caps, r.style.opentype.all_small_caps),
            set: |r: &mut StyleRun, (all, small, all_small)| {
                r.style.all_caps = all;
                r.style.small_caps = small;
                r.style.opentype.all_small_caps = all_small;
            },
        },
    );
    followed |= attr_op(w, field!(StyleRun, style.baseline));
    followed |= attr_op(w, field!(StyleRun, style.kerning));
    followed |= attr_op(w, field!(StyleRun, style.ligatures));
    followed |= attr_op(w, field!(StyleRun, style.tate_chu_yoko));
    followed |= attr_op(w, field!(StyleRun, style.vertical_roman_upright));
    // OpenType settings one by one (All Small Caps is with the capitalization above).
    followed |= attr_op(w, field!(StyleRun, style.opentype.discretionary_ligatures));
    followed |= attr_op(w, field!(StyleRun, style.opentype.contextual_alternates));
    followed |= attr_op(w, field!(StyleRun, style.opentype.stylistic_alternates));
    followed |= attr_op(w, field!(StyleRun, style.opentype.stylistic_sets));
    followed |= attr_op(w, field!(StyleRun, style.opentype.swash));
    followed |= attr_op(w, field!(StyleRun, style.opentype.titling));
    followed |= attr_op(w, field!(StyleRun, style.opentype.ordinals));
    followed |= attr_op(w, field!(StyleRun, style.opentype.fractions));
    followed |= attr_op(w, field!(StyleRun, style.opentype.figure_style));
    followed |= attr_op(w, field!(StyleRun, style.opentype.figure_width));
    followed |= attr_op(w, field!(StyleRun, style.variations));
    followed
}

/// [`attr_op`] for every paragraph attribute (every field of [`ParaStyle`] listed, as above).
fn para_attrs(w: &mut Work<ParaStyle>) -> bool {
    let Some(ParaStyle {
        justify: _,
        indent_left: _,
        indent_right: _,
        indent_first: _,
        space_before: _,
        space_after: _,
        direction: _,
        composer: _,
        hanging_punctuation: _,
    }) = w.m.first()
    else {
        return false;
    };
    let mut followed = false;
    followed |= attr_op(w, field!(ParaStyle, justify));
    followed |= attr_op(w, field!(ParaStyle, indent_left));
    followed |= attr_op(w, field!(ParaStyle, indent_right));
    followed |= attr_op(w, field!(ParaStyle, indent_first));
    followed |= attr_op(w, field!(ParaStyle, space_before));
    followed |= attr_op(w, field!(ParaStyle, space_after));
    followed |= attr_op(w, field!(ParaStyle, direction));
    followed |= attr_op(w, field!(ParaStyle, composer));
    followed |= attr_op(w, field!(ParaStyle, hanging_punctuation));
    followed
}

fn color_json(v: &J) -> Option<[f32; 4]> {
    match v {
        J::String(s) => {
            let s = s.trim_start_matches('#');
            let h = |i: usize| u8::from_str_radix(s.get(i..i + 2)?, 16).ok().map(|x| x as f32 / 255.0);
            Some([h(0)?, h(2)?, h(4)?, if s.len() >= 8 { h(6)? } else { 1.0 }])
        }
        J::Array(a) if a.len() >= 3 => {
            let g = |i: usize, d: f64| a.get(i).and_then(J::as_f64).unwrap_or(d) as f32;
            Some([g(0, 0.0), g(1, 0.0), g(2, 0.0), g(3, 1.0)])
        }
        _ => None,
    }
}

fn num(key: &str, v: &J) -> Result<f64, String> {
    v.as_f64().filter(|x| x.is_finite()).ok_or_else(|| format!("{key}: expected a number"))
}
fn norm(v: &J) -> Option<String> {
    v.as_str().map(|s| s.to_ascii_lowercase().replace(['-', ' ', '_'], ""))
}
fn flag(key: &str, v: &J) -> Result<bool, String> {
    v.as_bool().or_else(|| v.as_f64().map(|x| x != 0.0)).ok_or_else(|| format!("{key}: expected a boolean"))
}

/// Character attribute keys of `layer.setText` (also used by the expression style API).
pub const CHAR_ATTRS: &[&str] = &[
    "font",
    "style",
    "size",
    "fill",
    "stroke",
    "strokeWidth",
    "applyFill",
    "applyStroke",
    "tracking",
    "leading",
    "baselineShift",
    "hScale",
    "vScale",
    "tsume",
    "fauxBold",
    "fauxItalic",
    "allCaps",
    "smallCaps",
    "baseline",
    "superscript",
    "subscript",
    "kerning",
    "ligatures",
    "tateChuYoko",
    "verticalRomanUpright",
    "discretionaryLigatures",
    "contextualAlternates",
    "stylisticAlternates",
    "stylisticSets",
    "swash",
    "titling",
    "ordinals",
    "fractions",
    "allSmallCaps",
    "figureStyle",
    "figureWidth",
    "figures",
    "variations",
];

/// Paragraph attribute keys of `layer.setText`.
pub const PARA_ATTRS: &[&str] =
    &["justify", "indentLeft", "indentRight", "indentFirst", "spaceBefore", "spaceAfter", "direction", "composer", "hangingPunctuation"];

/// Most variable font axes one style can set. Real fonts have a handful (OpenType registers
/// five; custom axes rarely bring the total past twenty); a `variations` object with more is
/// rejected before any of it is applied.
pub const MAX_VARIATION_AXES: usize = 64;

/// Set one character attribute; Ok(false) for keys that aren't character attributes.
pub fn apply_char_attr(s: &mut CharStyle, key: &str, v: &J) -> Result<bool, String> {
    match key {
        "font" => s.font = v.as_str().ok_or("font: expected a string")?.to_string(),
        "style" => s.style = v.as_str().ok_or("style: expected a string")?.to_string(),
        "size" => s.size = num(key, v)?.clamp(0.1, 10_000.0),
        "fill" => s.fill = color_json(v).ok_or("fill: expected #rrggbb or [r, g, b]")?,
        "stroke" => {
            s.stroke = color_json(v).ok_or("stroke: expected #rrggbb or [r, g, b]")?;
            s.apply_stroke = true;
        }
        "strokeWidth" => {
            s.stroke_width = num(key, v)?.max(0.0);
            s.apply_stroke = s.stroke_width > 0.0;
        }
        "applyFill" => s.apply_fill = flag(key, v)?,
        "applyStroke" => s.apply_stroke = flag(key, v)?,
        "tracking" => s.tracking = num(key, v)?,
        "leading" => {
            s.leading = match v {
                J::Number(n) => n.as_f64().map(|x| x.max(0.0)),
                J::String(_) | J::Null => None,
                J::Bool(true) => None,
                _ => return Err("leading: expected a number or \"auto\"".into()),
            }
        }
        "baselineShift" => s.baseline_shift = num(key, v)?,
        "hScale" => s.h_scale = num(key, v)?.clamp(1.0, 1000.0),
        "vScale" => s.v_scale = num(key, v)?.clamp(1.0, 1000.0),
        "tsume" => s.tsume = num(key, v)?.clamp(0.0, 100.0),
        "fauxBold" => s.faux_bold = flag(key, v)?,
        "fauxItalic" => s.faux_italic = flag(key, v)?,
        "allCaps" => {
            s.all_caps = flag(key, v)?;
            if s.all_caps {
                s.small_caps = false;
                s.opentype.all_small_caps = false;
            }
        }
        "smallCaps" => {
            s.small_caps = flag(key, v)?;
            if s.small_caps {
                s.all_caps = false;
            }
        }
        "baseline" => s.baseline = v.as_str().and_then(BaselineOption::parse).ok_or("baseline: expected normal|superscript|subscript")?,
        "superscript" => {
            s.baseline = if flag(key, v)? { BaselineOption::Superscript } else { BaselineOption::Normal };
        }
        "subscript" => {
            s.baseline = if flag(key, v)? { BaselineOption::Subscript } else { BaselineOption::Normal };
        }
        "kerning" => s.kerning = Kerning::parse(v).ok_or("kerning: expected metrics|optical|number")?,
        "ligatures" => s.ligatures = flag(key, v)?,
        "tateChuYoko" => s.tate_chu_yoko = flag(key, v)?,
        "verticalRomanUpright" => s.vertical_roman_upright = flag(key, v)?,
        "discretionaryLigatures" => s.opentype.discretionary_ligatures = flag(key, v)?,
        "contextualAlternates" => s.opentype.contextual_alternates = flag(key, v)?,
        "stylisticAlternates" => s.opentype.stylistic_alternates = flag(key, v)?,
        "swash" => s.opentype.swash = flag(key, v)?,
        "titling" => s.opentype.titling = flag(key, v)?,
        "ordinals" => s.opentype.ordinals = flag(key, v)?,
        "fractions" => s.opentype.fractions = flag(key, v)?,
        "allSmallCaps" => {
            s.opentype.all_small_caps = flag(key, v)?;
            if s.opentype.all_small_caps {
                s.all_caps = false;
            }
        }
        "stylisticSets" => {
            s.opentype.stylistic_sets = match v {
                J::Array(a) => {
                    let mut m = 0u32;
                    for n in a {
                        let n = n.as_u64().filter(|n| (1..=20).contains(n)).ok_or("stylisticSets: expected set numbers 1–20")?;
                        m |= 1 << (n - 1);
                    }
                    m
                }
                J::Number(_) => (num(key, v)? as u64 & 0xF_FFFF) as u32,
                _ => return Err("stylisticSets: expected [1, 2, …] or a bit mask".into()),
            }
        }
        "figureStyle" => {
            s.opentype.figure_style = match norm(v).as_deref() {
                Some("default") => FigureStyle::Default,
                Some("lining") => FigureStyle::Lining,
                Some("oldstyle") => FigureStyle::OldStyle,
                _ => return Err("figureStyle: expected default|lining|oldStyle".into()),
            }
        }
        "figureWidth" => {
            s.opentype.figure_width = match norm(v).as_deref() {
                Some("default") => FigureWidth::Default,
                Some("proportional") => FigureWidth::Proportional,
                Some("tabular") => FigureWidth::Tabular,
                _ => return Err("figureWidth: expected default|proportional|tabular".into()),
            }
        }
        "figures" => {
            let (st, w) = match norm(v).as_deref() {
                Some("default") => (FigureStyle::Default, FigureWidth::Default),
                Some("tabularlining") => (FigureStyle::Lining, FigureWidth::Tabular),
                Some("proportionallining") => (FigureStyle::Lining, FigureWidth::Proportional),
                Some("tabularoldstyle") => (FigureStyle::OldStyle, FigureWidth::Tabular),
                Some("proportionaloldstyle") => (FigureStyle::OldStyle, FigureWidth::Proportional),
                _ => return Err("figures: expected default|tabularLining|proportionalLining|tabularOldstyle|proportionalOldstyle".into()),
            };
            s.opentype.figure_style = st;
            s.opentype.figure_width = w;
        }
        // Variable font axes: `{wght: 650, wdth: 90}` (null removes an axis), or `{}` / null to
        // reset every axis to its default.
        "variations" => match v {
            J::Null => s.variations.clear(),
            J::Object(m) => {
                if m.len() > MAX_VARIATION_AXES {
                    return Err(format!("variations: at most {MAX_VARIATION_AXES} axes"));
                }
                // One keyed merge (the result sorted by tag), applied only when every entry is valid.
                let mut axes: std::collections::BTreeMap<String, f32> = if m.is_empty() { Default::default() } else { s.variations.iter().cloned().collect() };
                for (tag, val) in m {
                    if tag.is_empty() || tag.len() > 4 {
                        return Err(format!("variations: `{tag}` is not an axis tag"));
                    }
                    match val {
                        J::Null => {
                            axes.remove(tag);
                        }
                        _ => {
                            axes.insert(tag.clone(), val.as_f64().ok_or("variations: expected {tag: number}")? as f32);
                        }
                    }
                }
                if axes.len() > MAX_VARIATION_AXES {
                    return Err(format!("variations: at most {MAX_VARIATION_AXES} axes"));
                }
                s.variations = axes.into_iter().collect();
            }
            _ => return Err("variations: expected {tag: number, …}".into()),
        },
        k if k.len() == 4 && k.starts_with("ss") && k[2..].parse::<u32>().is_ok_and(|n| (1..=20).contains(&n)) => {
            let n = k[2..].parse::<u32>().unwrap_or(0);
            s.opentype.set_stylistic_set(n, flag(key, v)?);
        }
        _ => return Ok(false),
    }
    Ok(true)
}

/// Set one paragraph attribute; Ok(false) for keys that aren't paragraph attributes.
pub fn apply_para_attr(p: &mut ParaStyle, key: &str, v: &J) -> Result<bool, String> {
    match key {
        "justify" => {
            p.justify = v.as_str().and_then(Justify::parse).ok_or("justify: expected left|center|right|justifyLeft|justifyCenter|justifyRight|justifyAll")?
        }
        "indentLeft" => p.indent_left = num(key, v)?,
        "indentRight" => p.indent_right = num(key, v)?,
        "indentFirst" => p.indent_first = num(key, v)?,
        "spaceBefore" => p.space_before = num(key, v)?,
        "spaceAfter" => p.space_after = num(key, v)?,
        "direction" => {
            p.direction = match v.as_str().map(str::to_ascii_lowercase).as_deref() {
                Some("ltr" | "lefttoright" | "left-to-right") => Direction::Ltr,
                Some("rtl" | "righttoleft" | "right-to-left") => Direction::Rtl,
                _ => return Err("direction: expected ltr|rtl".into()),
            }
        }
        "composer" => {
            p.composer = match v.as_str().map(|s| s.to_ascii_lowercase().replace(['-', ' ', '_'], "")).as_deref() {
                Some("everyline" | "every") => Composer::EveryLine,
                Some("singleline" | "single") => Composer::SingleLine,
                _ => return Err("composer: expected everyLine|singleLine".into()),
            }
        }
        "hangingPunctuation" => p.hanging_punctuation = flag(key, v)?,
        _ => return Ok(false),
    }
    Ok(true)
}

/// A character style as `layer.setText` keys.
pub fn char_style_json(s: &CharStyle) -> J {
    let c = |c: [f32; 4]| J::from(vec![c[0] as f64, c[1] as f64, c[2] as f64, c[3] as f64]);
    serde_json::json!({
        "font": s.font, "style": s.style, "size": s.size, "fill": c(s.fill), "stroke": c(s.stroke),
        "strokeWidth": s.stroke_width, "applyFill": s.apply_fill, "applyStroke": s.apply_stroke,
        "tracking": s.tracking, "leading": s.leading.map_or(J::from("auto"), J::from), "baselineShift": s.baseline_shift,
        "hScale": s.h_scale, "vScale": s.v_scale, "tsume": s.tsume, "fauxBold": s.faux_bold, "fauxItalic": s.faux_italic,
        "allCaps": s.all_caps, "smallCaps": s.small_caps, "baseline": s.baseline.key(), "kerning": s.kerning.to_json(),
        "ligatures": s.ligatures, "tateChuYoko": s.tate_chu_yoko, "verticalRomanUpright": s.vertical_roman_upright,
        "discretionaryLigatures": s.opentype.discretionary_ligatures, "contextualAlternates": s.opentype.contextual_alternates,
        "stylisticAlternates": s.opentype.stylistic_alternates,
        "stylisticSets": (1..=20u32).filter(|n| s.opentype.stylistic_set(*n)).collect::<Vec<_>>(),
        "swash": s.opentype.swash, "titling": s.opentype.titling, "ordinals": s.opentype.ordinals, "fractions": s.opentype.fractions,
        "allSmallCaps": s.opentype.all_small_caps,
        "variations": s.variations.iter().map(|(t, v)| (t.clone(), J::from(*v as f64))).collect::<serde_json::Map<String, J>>(),
        "figureStyle": match s.opentype.figure_style { FigureStyle::Default => "default", FigureStyle::Lining => "lining", FigureStyle::OldStyle => "oldStyle" },
        "figureWidth": match s.opentype.figure_width { FigureWidth::Default => "default", FigureWidth::Proportional => "proportional", FigureWidth::Tabular => "tabular" },
    })
}

/// Paragraph settings as `layer.setText` keys.
pub fn para_style_json(p: &ParaStyle) -> J {
    serde_json::json!({
        "justify": p.justify.key(), "indentLeft": p.indent_left, "indentRight": p.indent_right, "indentFirst": p.indent_first,
        "spaceBefore": p.space_before, "spaceAfter": p.space_after,
        "direction": if p.direction == Direction::Rtl { "rtl" } else { "ltr" },
        "composer": if p.composer == Composer::EveryLine { "everyLine" } else { "singleLine" },
        "hangingPunctuation": p.hanging_punctuation,
    })
}

/// Word boundaries for caret movement: the start of the word left of `ci`.
pub fn word_left(text: &str, ci: usize) -> usize {
    let chars: Vec<char> = text.chars().collect();
    let mut i = ci.min(chars.len());
    while i > 0 && !chars[i - 1].is_alphanumeric() {
        i -= 1;
    }
    while i > 0 && chars[i - 1].is_alphanumeric() {
        i -= 1;
    }
    i
}

/// The end of the word right of `ci`.
pub fn word_right(text: &str, ci: usize) -> usize {
    let chars: Vec<char> = text.chars().collect();
    let mut i = ci.min(chars.len());
    while i < chars.len() && !chars[i].is_alphanumeric() {
        i += 1;
    }
    while i < chars.len() && chars[i].is_alphanumeric() {
        i += 1;
    }
    i
}

/// The word (or run of spaces / punctuation) containing character `ci`, for double-click.
pub fn word_at(text: &str, ci: usize) -> Range<usize> {
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return 0..0;
    }
    let i = ci.min(chars.len() - 1);
    let class = |c: char| {
        if c.is_alphanumeric() {
            0
        } else if matches!(c, '\r' | '\n') {
            2
        } else if c.is_whitespace() {
            1
        } else {
            3
        }
    };
    let k = class(chars[i]);
    let (mut a, mut b) = (i, i + 1);
    while a > 0 && class(chars[a - 1]) == k && k != 2 {
        a -= 1;
    }
    while b < chars.len() && class(chars[b]) == k && k != 2 {
        b += 1;
    }
    a..b
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bubble() -> TextDoc {
        TextDoc {
            text: "Original".into(),
            font: "Inter".into(),
            style: "SemiBold".into(),
            size: 54.0,
            fill: [0.141, 0.443, 0.639, 1.0],
            leading: Some(68.0),
            justify: Justify::Left,
            box_size: Some([820.0, 150.0]),
            box_pos: [-410.0, -75.0],
            ..Default::default()
        }
    }

    /// Merge one document, its base updated in place.
    fn merge(d: &mut TextDoc, base: &mut TextDoc, master: &TextDoc) {
        let base_in = base.clone();
        d.merge_master(&base_in, base, master);
    }

    /// #3186: own red, master moved to green (the base still blue), then the copy is set to blue:
    /// after the rebase blue stays the copy's own instead of counting as following.
    #[test]
    fn rebase_keeps_a_value_set_back_to_the_base() {
        let (blue, red, green) = ([0.0, 0.0, 1.0, 1.0], [1.0, 0.0, 0.0, 1.0], [0.0, 1.0, 0.0, 1.0]);
        let mut base = TextDoc { fill: blue, ..bubble() };
        let master = TextDoc { fill: green, size: 64.0, ..bubble() };
        let was = TextDoc { text: "Mine".into(), fill: red, ..base.clone() };
        let mut now = TextDoc { fill: blue, ..was.clone() };
        TextDoc::rebase(&mut base, &was, &now, &master);
        assert_eq!((base.fill, base.size), (green, 54.0), "only the fill was set");
        merge(&mut now, &mut base, &master);
        assert_eq!((now.fill, now.size, now.text.as_str()), (blue, 64.0, "Mine"));
    }

    /// #3186: two documents merged against the same base into one (the API allows it; overrides
    /// give each keyframe its own base): an attribute one followed moves the base on, while one
    /// that owns it keeps its value.
    #[test]
    fn merge_master_shares_one_base_between_documents() {
        let old = bubble();
        let new = TextDoc { fill: [0.0, 1.0, 0.0, 1.0], size: 64.0, ..old.clone() };
        let mut own = TextDoc { text: "First".into(), fill: [1.0, 0.0, 0.0, 1.0], ..old.clone() };
        let mut follows = TextDoc { text: "Second".into(), ..old.clone() };
        let mut base = old.clone();
        own.merge_master(&old, &mut base, &new);
        follows.merge_master(&old, &mut base, &new);
        assert_eq!((own.fill, own.size), ([1.0, 0.0, 0.0, 1.0], 64.0));
        assert_eq!((follows.fill, follows.size), (new.fill, 64.0));
        assert_eq!((base.fill, base.size), (new.fill, 64.0), "the base takes what any key followed");
        // Again with the new base: nothing changes, the first key's red stays its own.
        let (own_was, follows_was) = (own.clone(), follows.clone());
        let base_in = base.clone();
        own.merge_master(&base_in, &mut base, &new);
        follows.merge_master(&base_in, &mut base, &new);
        assert_eq!((own, follows), (own_was, follows_was));
    }

    /// #3186: an instance's copy of a document takes a restyle of the master in every attribute
    /// it did not change itself, and keeps its text and its own attributes.
    #[test]
    fn merge_master_takes_the_restyle_and_keeps_own_text_and_attributes() {
        let old = bubble();
        let mut new = old.clone();
        new.fill = [0.004, 0.122, 0.149, 1.0];
        new.size = 64.0;
        new.justify = Justify::Right;
        new.tracking = 20.0;
        new.box_size = Some([900.0, 150.0]);

        let mut base = old.clone();
        let mut inst = old.clone();
        inst.set_text("Own sentence\nsecond line");
        inst.tracking = 50.0;
        merge(&mut inst, &mut base, &new);
        assert_eq!(inst.text, "Own sentence\nsecond line");
        assert_eq!((inst.fill, inst.size, inst.justify), (new.fill, 64.0, Justify::Right));
        assert_eq!((inst.font.as_str(), inst.style.as_str(), inst.leading), ("Inter", "SemiBold", Some(68.0)));
        assert_eq!(inst.box_size, Some([900.0, 150.0]));
        assert_eq!(inst.tracking, 50.0, "the instance's own tracking stays");
        assert!(inst.runs.is_empty() && inst.paragraphs.is_empty());
        assert_eq!(inst.paras().iter().map(|p| p.justify).collect::<Vec<_>>(), vec![Justify::Right; 2]);
        // The base took what was followed, and kept the old value of what is the instance's own.
        assert_eq!((base.fill, base.size, base.tracking), (new.fill, 64.0, 0.0));

        // Merging again with the updated base changes nothing.
        let (inst_was, base_was) = (inst.clone(), base.clone());
        merge(&mut inst, &mut base, &new);
        assert_eq!((&inst, &base), (&inst_was, &base_was));

        // Mixed formatting in the instance: every run takes the new fill, the bold run stays bold.
        let mut base = old.clone();
        let mut mixed = old.clone();
        mixed.set_text("Bold rest");
        mixed.apply_style(0..4, |s| s.faux_bold = true);
        merge(&mut mixed, &mut base, &new);
        assert!(mixed.style_at(0).faux_bold && !mixed.style_at(6).faux_bold);
        assert_eq!((mixed.style_at(0).fill, mixed.style_at(6).fill), (new.fill, new.fill));
        assert_eq!(mixed.style_at(6).size, 64.0);
    }

    /// #3186: an attribute the instance changed stays its own even when the master passes
    /// through the instance's value (blue → red → green keeps the instance red).
    #[test]
    fn merge_master_keeps_own_value_when_the_master_passes_through_it() {
        let blue = [0.0, 0.0, 1.0, 1.0];
        let red = [1.0, 0.0, 0.0, 1.0];
        let green = [0.0, 1.0, 0.0, 1.0];
        let master = TextDoc { fill: blue, ..bubble() };
        let mut base = master.clone();
        let mut inst = TextDoc { text: "Mine".into(), fill: red, ..master.clone() };
        for fill in [red, green] {
            let master = TextDoc { fill, size: 60.0, ..master.clone() };
            merge(&mut inst, &mut base, &master);
            assert_eq!(inst.fill, red, "the instance's own fill stays");
            assert_eq!(inst.size, 60.0);
        }
        assert_eq!(inst.text, "Mine");
    }

    /// #3186: deleting the master's bold first word changes its first character's style, but is
    /// no restyle: the instance's bold word stays bold.
    #[test]
    fn merge_master_ignores_mixed_formatting_that_changed_with_the_text() {
        let mut master = bubble();
        master.set_text("Bold rest");
        master.apply_style(0..4, |s| s.faux_bold = true);
        let mut base = master.clone();
        let mut inst = master.clone();
        inst.set_text("Wow, more words");
        inst.apply_style(0..4, |s| s.faux_bold = true);
        inst.apply_style(4..15, |s| s.faux_bold = false);
        let inst_was = inst.clone();
        // The master's bold word is deleted: what is left is plain.
        master.replace_range(0..5, "", None);
        assert!(!master.style_at(0).faux_bold && master.runs.is_empty());
        merge(&mut inst, &mut base, &master);
        assert_eq!(inst, inst_was);
        assert!(inst.style_at(0).faux_bold);
    }

    /// #3186: OpenType settings merge one by one: the instance's own swash stays while the
    /// master's change to contextual alternates reaches it.
    #[test]
    fn merge_master_takes_each_opentype_setting_on_its_own() {
        let master = bubble();
        let mut base = master.clone();
        let mut inst = TextDoc { text: "Mine".into(), ..master.clone() };
        inst.opentype.swash = true;
        let mut new = master.clone();
        new.opentype.contextual_alternates = false;
        merge(&mut inst, &mut base, &new);
        assert!(inst.opentype.swash, "the instance's own swash stays");
        assert!(!inst.opentype.contextual_alternates, "the master's contextual alternates reach it");
        // The next master change to swash does not reach it; one to stylistic sets does.
        let mut newer = new.clone();
        newer.opentype.swash = false;
        newer.opentype.stylistic_sets = 0b101;
        merge(&mut inst, &mut base, &newer);
        assert!(inst.opentype.swash);
        assert_eq!(inst.opentype.stylistic_sets, 0b101);
    }

    /// #3186: All Caps, Small Caps and All Small Caps are one choice: an instance that chose Small
    /// Caps keeps it when the master turns on All Caps, and never ends up with both.
    #[test]
    fn merge_master_keeps_the_instances_capitalization_choice() {
        let master = bubble();
        let mut base = master.clone();
        let mut inst = TextDoc { text: "Mine".into(), small_caps: true, ..master.clone() };
        let new = TextDoc { all_caps: true, size: 64.0, ..master.clone() };
        merge(&mut inst, &mut base, &new);
        assert_eq!((inst.all_caps, inst.small_caps, inst.opentype.all_small_caps), (false, true, false));
        assert_eq!(inst.size, 64.0);
        // All Small Caps chosen by the instance: the master's All Caps is not added to it.
        let mut base = master.clone();
        let mut inst = TextDoc { text: "Mine".into(), ..master.clone() };
        inst.opentype.all_small_caps = true;
        merge(&mut inst, &mut base, &new);
        assert_eq!((inst.all_caps, inst.small_caps, inst.opentype.all_small_caps), (false, false, true));
        // An instance that left capitalization alone takes the master's.
        let mut base = master.clone();
        let mut inst = TextDoc { text: "Mine".into(), ..master.clone() };
        merge(&mut inst, &mut base, &new);
        assert_eq!((inst.all_caps, inst.small_caps, inst.opentype.all_small_caps), (true, false, false));
    }

    /// #3186: an edit inside mixed formatting is a change: base blue, the instance's runs red and
    /// yellow, the master green; the red run set to blue stays blue (it used to count as
    /// following the blue base and turn green).
    #[test]
    fn rebase_sees_a_change_inside_mixed_formatting() {
        let (blue, red, yellow, green) = ([0.0, 0.0, 1.0, 1.0], [1.0, 0.0, 0.0, 1.0], [1.0, 1.0, 0.0, 1.0], [0.0, 1.0, 0.0, 1.0]);
        let mut base = TextDoc { fill: blue, ..bubble() };
        let master = TextDoc { fill: green, ..bubble() };
        let mut was = TextDoc { fill: red, ..bubble() };
        was.set_text("Red yellow");
        was.apply_style(4..10, |s| s.fill = yellow);
        let mut now = was.clone();
        now.apply_style(0..3, |s| s.fill = blue);
        TextDoc::rebase(&mut base, &was, &now, &master);
        assert_eq!(base.fill, green);
        merge(&mut now, &mut base, &master);
        assert_eq!((now.style_at(0).fill, now.style_at(5).fill), (blue, yellow), "the blue just set stays");
        // A text edit alone inside mixed formatting is no restyle: the base stays.
        let mut base = TextDoc { fill: blue, ..bubble() };
        let mut typed = was.clone();
        typed.replace_range(10..10, "!", None);
        TextDoc::rebase(&mut base, &was, &typed, &master);
        assert_eq!(base.fill, blue);
    }

    /// #3186: an attribute mixed in the base that the instance left as it was follows the
    /// master's new uniform value; the attribute the instance changed stays its own.
    #[test]
    fn merge_master_takes_a_uniform_value_over_mixed_formatting_the_instance_left_alone() {
        let (red, blue, green) = ([1.0, 0.0, 0.0, 1.0], [0.0, 0.0, 1.0, 1.0], [0.0, 1.0, 0.0, 1.0]);
        let mut master = TextDoc { fill: red, ..bubble() };
        master.set_text("Red blue");
        master.apply_style(4..8, |s| s.fill = blue);
        let mut base = master.clone();
        let mut inst = master.clone();
        inst.apply_style_all(|s| s.tracking = 50.0);
        let mut new = master.clone();
        new.apply_style_all(|s| s.fill = green);
        merge(&mut inst, &mut base, &new);
        assert_eq!((inst.style_at(0).fill, inst.style_at(6).fill), (green, green));
        assert_eq!((inst.style_at(0).tracking, inst.style_at(6).tracking), (50.0, 50.0), "the instance's own tracking stays");
        assert_eq!(base.style_at(0).fill, green);
        // Mixed formatting the instance changed stays.
        let mut base = master.clone();
        let mut inst = master.clone();
        inst.apply_style(0..3, |s| s.fill = blue);
        let inst_was = inst.clone();
        merge(&mut inst, &mut base, &new);
        assert_eq!(inst, inst_was);
    }

    const BLUE: [f32; 4] = [0.0, 0.0, 1.0, 1.0];
    const RED: [f32; 4] = [1.0, 0.0, 0.0, 1.0];
    const GREEN: [f32; 4] = [0.0, 1.0, 0.0, 1.0];

    /// The master with one fill (and the same text).
    fn master_with(fill: [f32; 4]) -> TextDoc {
        TextDoc { fill, ..bubble() }
    }

    /// An instance over a blue base: "Red blue" with its first word explicitly red.
    fn red_first_word() -> TextDoc {
        let mut d = TextDoc { fill: BLUE, ..bubble() };
        d.set_text("Red blue");
        d.apply_style(0..3, |s| s.fill = RED);
        d
    }

    /// The instance's own edit, the way an Essential Properties Set does it: the edit, then the
    /// base rebased over it.
    fn edit(d: &mut TextDoc, base: &mut TextDoc, master: &TextDoc, f: impl FnOnce(&mut TextDoc)) {
        let before = d.clone();
        f(d);
        TextDoc::rebase(base, &before, d, master);
    }

    fn fills(d: &TextDoc) -> Vec<(usize, [f32; 4])> {
        let mut out: Vec<(usize, [f32; 4])> = vec![];
        for r in d.runs() {
            match out.last_mut() {
                Some(last) if last.1 == r.style.fill => last.0 += r.len,
                _ => out.push((r.len, r.style.fill)),
            }
        }
        out
    }

    /// #3186 (review): the instance's explicitly red first run stays red through two master
    /// changes (red, then green) while the unchanged second run follows both. With one base for
    /// the whole copy the first sync advanced it to red, and the second sync took the red first
    /// run for following and turned it green.
    #[test]
    fn an_own_run_keeps_its_value_through_two_master_changes() {
        let mut base = TextDoc { fill: BLUE, ..bubble() };
        let mut inst = red_first_word();
        merge(&mut inst, &mut base, &master_with(RED));
        assert_eq!(fills(&inst), vec![(8, RED)], "the blue run follows the master to red");
        merge(&mut inst, &mut base, &master_with(GREEN));
        assert_eq!(fills(&inst), vec![(3, RED), (5, GREEN)], "the own red run stays, the other follows");
        // And back to blue (the old base): the own run is still its own.
        merge(&mut inst, &mut base, &master_with(BLUE));
        assert_eq!(fills(&inst), vec![(3, RED), (5, BLUE)]);
        // Merging again changes nothing.
        let (inst_was, base_was) = (inst.clone(), base.clone());
        merge(&mut inst, &mut base, &master_with(BLUE));
        assert_eq!((&inst, &base), (&inst_was, &base_was));
    }

    /// #3186: typing, splitting and deleting keep every other run's ownership.
    #[test]
    fn edits_that_split_move_and_delete_runs_keep_ownership() {
        // Typing at the end of the following run: the new characters follow too.
        let mut base = TextDoc { fill: BLUE, ..bubble() };
        let mut inst = red_first_word();
        merge(&mut inst, &mut base, &master_with(RED));
        let mut typed = (inst.clone(), base.clone());
        edit(&mut typed.0, &mut typed.1, &master_with(RED), |d| d.replace_range(8..8, "!", None));
        merge(&mut typed.0, &mut typed.1, &master_with(GREEN));
        assert_eq!(typed.0.text, "Red blue!");
        assert_eq!(fills(&typed.0), vec![(3, RED), (6, GREEN)]);

        // Typing inside the own run: the new characters are the instance's own too.
        let mut typed = (inst.clone(), base.clone());
        edit(&mut typed.0, &mut typed.1, &master_with(RED), |d| d.replace_range(1..1, "xx", None));
        merge(&mut typed.0, &mut typed.1, &master_with(GREEN));
        assert_eq!(typed.0.text, "Rxxed blue");
        assert_eq!(fills(&typed.0), vec![(5, RED), (5, GREEN)]);

        // Splitting a following run by styling a part of it: both parts keep following, the
        // style the instance set stays its own.
        let mut split = (inst.clone(), base.clone());
        edit(&mut split.0, &mut split.1, &master_with(RED), |d| d.apply_style(5..7, |s| s.faux_bold = true));
        merge(&mut split.0, &mut split.1, &master_with(GREEN));
        assert_eq!(fills(&split.0), vec![(3, RED), (5, GREEN)]);
        assert!(split.0.style_at(5).faux_bold && !split.0.style_at(4).faux_bold && !split.0.style_at(7).faux_bold);

        // Splitting the own run: both parts stay the instance's own.
        let mut split = (inst.clone(), base.clone());
        edit(&mut split.0, &mut split.1, &master_with(RED), |d| d.apply_style(1..2, |s| s.faux_italic = true));
        merge(&mut split.0, &mut split.1, &master_with(GREEN));
        assert_eq!(fills(&split.0), vec![(3, RED), (5, GREEN)]);

        // Deleting the own run: the rest keeps following.
        let mut cut = (inst.clone(), base.clone());
        edit(&mut cut.0, &mut cut.1, &master_with(RED), |d| d.replace_range(0..4, "", None));
        merge(&mut cut.0, &mut cut.1, &master_with(GREEN));
        assert_eq!((cut.0.text.as_str(), fills(&cut.0)), ("blue", vec![(4, GREEN)]));

        // Deleting the following run: the own run stays own.
        let mut cut = (inst.clone(), base.clone());
        edit(&mut cut.0, &mut cut.1, &master_with(RED), |d| d.replace_range(3..8, "", None));
        merge(&mut cut.0, &mut cut.1, &master_with(GREEN));
        assert_eq!((cut.0.text.as_str(), fills(&cut.0)), ("Red", vec![(3, RED)]));

        // Pasting text with another look in front of everything: it is the instance's own, the
        // runs that were there keep their ownership.
        let mut paste = (inst.clone(), base.clone());
        edit(&mut paste.0, &mut paste.1, &master_with(RED), |d| {
            let yellow = CharStyle { fill: [1.0, 1.0, 0.0, 1.0], ..d.style_at(0) };
            d.replace_range(0..0, "Hi ", Some(&yellow));
        });
        merge(&mut paste.0, &mut paste.1, &master_with(GREEN));
        assert_eq!(fills(&paste.0), vec![(3, [1.0, 1.0, 0.0, 1.0]), (3, RED), (5, GREEN)]);

        // Setting the following run to the master's value by hand makes it the instance's own;
        // merging the two runs into one (everything red) keeps all of it.
        let mut own = (inst.clone(), base.clone());
        edit(&mut own.0, &mut own.1, &master_with(RED), |d| d.apply_style(3..8, |s| s.fill = BLUE));
        merge(&mut own.0, &mut own.1, &master_with(GREEN));
        assert_eq!(fills(&own.0), vec![(3, RED), (5, BLUE)], "blue set by hand stays");
        let mut one = (inst.clone(), base.clone());
        edit(&mut one.0, &mut one.1, &master_with(RED), |d| d.apply_style_all(|s| s.fill = BLUE));
        merge(&mut one.0, &mut one.1, &master_with(GREEN));
        assert_eq!(fills(&one.0), vec![(8, BLUE)], "everything set by hand in one go is the instance's own");
    }

    /// #3186: paragraphs get the same per-paragraph ownership, also through Enter and deletes.
    #[test]
    fn paragraphs_keep_ownership_through_two_master_changes_and_edits() {
        let master = |j: Justify| TextDoc { justify: j, ..bubble() };
        let mut inst = TextDoc { text: "one\ntwo".into(), ..master(Justify::Left) };
        inst.apply_para(0..1, |p| p.justify = Justify::Center);
        let mut base = master(Justify::Left);
        let justs = |d: &TextDoc| d.paras().iter().map(|p| p.justify).collect::<Vec<_>>();
        merge(&mut inst, &mut base, &master(Justify::Center));
        assert_eq!(justs(&inst), vec![Justify::Center; 2], "the left paragraph follows");
        merge(&mut inst, &mut base, &master(Justify::Right));
        assert_eq!(justs(&inst), vec![Justify::Center, Justify::Right], "the own centered one stays");

        // Enter at the end of the last paragraph: the new paragraph follows like it.
        let (mut d, mut b) = (inst.clone(), base.clone());
        edit(&mut d, &mut b, &master(Justify::Right), |d| d.replace_range(7..7, "\nthree", None));
        merge(&mut d, &mut b, &master(Justify::Left));
        assert_eq!(justs(&d), vec![Justify::Center, Justify::Left, Justify::Left]);
        // Enter inside the own paragraph: both halves are the instance's own.
        let (mut d, mut b) = (inst.clone(), base.clone());
        edit(&mut d, &mut b, &master(Justify::Right), |d| d.replace_range(1..1, "\n", None));
        merge(&mut d, &mut b, &master(Justify::Left));
        assert_eq!(justs(&d), vec![Justify::Center, Justify::Center, Justify::Left]);
        // Deleting the own paragraph with its line break: the text left takes the deleted
        // paragraph's settings (the edit's own doing), which stay the instance's own.
        let (mut d, mut b) = (inst.clone(), base.clone());
        edit(&mut d, &mut b, &master(Justify::Right), |d| d.replace_range(0..4, "", None));
        merge(&mut d, &mut b, &master(Justify::Left));
        assert_eq!((d.text.as_str(), justs(&d)), ("two", vec![Justify::Center]));
        // Deleting the following paragraph: the own one stays own, and a third that followed
        // still follows.
        let (mut d, mut b) = (inst.clone(), base.clone());
        edit(&mut d, &mut b, &master(Justify::Right), |d| {
            d.replace_range(
                7..7,
                "
three",
                None,
            )
        });
        edit(&mut d, &mut b, &master(Justify::Right), |d| d.replace_range(3..7, "", None));
        merge(&mut d, &mut b, &master(Justify::Left));
        assert_eq!(
            (d.text.as_str(), justs(&d)),
            (
                "one
three",
                vec![Justify::Center, Justify::Left]
            )
        );
    }

    /// #3186: a base saved before per-run records existed has none (they are not written while
    /// empty), and a copy edited without a rebase falls back to the base's own value.
    #[test]
    fn follow_records_are_optional_and_survive_a_round_trip() {
        let mut base = TextDoc { fill: BLUE, ..bubble() };
        let mut inst = red_first_word();
        merge(&mut inst, &mut base, &master_with(RED));
        assert!(!base.follow_runs.is_empty(), "the own run's base is kept");
        let json = serde_json::to_value(&base).unwrap();
        let mut old = json.clone();
        if let Some(o) = old.as_object_mut() {
            o.remove("follow_runs");
            o.remove("follow_paras");
        }
        let restored: TextDoc = serde_json::from_value(json).unwrap();
        assert_eq!(restored, base);
        let legacy: TextDoc = serde_json::from_value(old).unwrap();
        assert!(legacy.follow_runs.is_empty() && legacy.follow_paras.is_empty());
        // Plain documents never write the fields.
        let plain = serde_json::to_value(bubble()).unwrap();
        assert!(plain.get("follow_runs").is_none() && plain.get("follow_paras").is_none());
        // A copy whose text changed without a rebase: no panic, the base's value is used.
        let mut stray = inst.clone();
        stray.set_text("Something else entirely");
        let mut b = base.clone();
        let b_in = b.clone();
        stray.merge_master(&b_in, &mut b, &master_with(GREEN));
        assert_eq!(stray.text, "Something else entirely");
    }

    #[test]
    fn variations_merge_by_tag_and_reject_too_many_axes() {
        let mut doc = TextDoc::plain("ab");
        assert!(doc.set_attr("variations", &serde_json::json!({"wght": 650, "wdth": 90}), None).unwrap());
        assert!(doc.set_attr("variations", &serde_json::json!({"wght": 700, "wdth": null, "opsz": 12}), None).unwrap());
        assert_eq!(doc.style_at(0).variations, vec![("opsz".to_string(), 12.0), ("wght".to_string(), 700.0)]);
        // More axes than any font has: refused up front, the style untouched.
        let many: serde_json::Map<String, J> = (0..MAX_VARIATION_AXES + 1).map(|i| (format!("a{i:03}"), J::from(1.0))).collect();
        assert!(doc.set_attr("variations", &J::Object(many), None).is_err());
        assert_eq!(doc.style_at(0).variations.len(), 2);
        // A merge that would grow past the limit is refused too.
        let fill: serde_json::Map<String, J> = (0..MAX_VARIATION_AXES - 2).map(|i| (format!("b{i:03}"), J::from(1.0))).collect();
        assert!(doc.set_attr("variations", &J::Object(fill), None).unwrap());
        assert_eq!(doc.style_at(0).variations.len(), MAX_VARIATION_AXES);
        // (The probe on an empty style passes; the styles already at the limit keep their axes.)
        let _ = doc.set_attr("variations", &serde_json::json!({"zzzz": 1}), None);
        assert_eq!(doc.style_at(0).variations.len(), MAX_VARIATION_AXES);
        assert!(!doc.style_at(0).variations.iter().any(|(t, _)| t == "zzzz"));
    }

    #[test]
    fn refused_variation_leaves_every_run_unchanged() {
        // "ab" is at the axis limit; "cd" has none. Three new axes over both runs must be refused
        // for both, not applied to "cd" only.
        let mut doc = TextDoc::plain("abcd");
        let fill: serde_json::Map<String, J> = (0..MAX_VARIATION_AXES).map(|i| (format!("b{i:03}"), J::from(1.0))).collect();
        assert!(doc.set_attr("variations", &J::Object(fill), Some(0..2)).unwrap());
        assert!(doc.set_attr("variations", &serde_json::json!({"x1": 1, "x2": 1, "x3": 1}), None).is_err());
        assert_eq!(doc.style_at(3).variations.len(), 0);
        assert_eq!(doc.style_at(0).variations.len(), MAX_VARIATION_AXES);
    }

    #[test]
    fn empty_document_accepts_an_extreme_range_without_overflow() {
        let mut doc = TextDoc::plain("");
        assert!(doc.set_attr("size", &J::from(20), Some(usize::MAX..usize::MAX)).unwrap());
    }

    #[test]
    fn mixed_line_breaks_keep_original_byte_and_character_ranges() {
        let text = "é\r\nβ\rx\n\n";
        let spans: Vec<_> = paragraph_ranges(text).collect();
        assert_eq!(spans.iter().map(|p| p.bytes.clone()).collect::<Vec<_>>(), vec![0..2, 4..6, 7..8, 9..9, 10..10]);
        assert_eq!(spans.iter().map(|p| &text[p.bytes.clone()]).collect::<Vec<_>>(), vec!["é", "β", "x", "", ""]);
        let doc = TextDoc::plain(text);
        assert_eq!(doc.para_count(), 5);
        assert_eq!(doc.para_ranges(), vec![0..1, 3..4, 5..6, 7..7, 8..8]);
        assert_eq!((0..=8).map(|ci| doc.para_of(ci)).collect::<Vec<_>>(), vec![0, 0, 1, 1, 1, 2, 2, 3, 4]);
        let restored: TextDoc = serde_json::from_value(serde_json::to_value(&doc).unwrap()).unwrap();
        assert_eq!(restored.text, text, "line endings survive a project round trip");
        assert_eq!(paragraph_ranges("").count(), 1);
        assert_eq!(paragraph_ranges("\r\n").count(), 2);
    }

    #[test]
    fn paragraph_formatting_follows_cr_lf_and_crlf_edits() {
        for separator in ["\r", "\n", "\r\n"] {
            let mut doc = TextDoc::plain(&format!("one{separator}two{separator}three"));
            let start = doc.para_ranges()[1].start;
            doc.apply_para(start..start + 1, |p| p.indent_left = 30.0);
            doc.replace_range(start + 1..start + 1, separator, None);
            assert_eq!(doc.text, format!("one{separator}t{separator}wo{separator}three"));
            assert_eq!(doc.paras().iter().map(|p| p.indent_left).collect::<Vec<_>>(), vec![0.0, 30.0, 30.0, 0.0]);
            let ranges = doc.para_ranges();
            doc.replace_range(ranges[0].end..ranges[1].start, "", None);
            assert_eq!(doc.text, format!("onet{separator}wo{separator}three"));
            assert_eq!(doc.paras().iter().map(|p| p.indent_left).collect::<Vec<_>>(), vec![0.0, 30.0, 0.0]);
        }
    }

    #[test]
    fn editing_crlf_boundaries_preserves_untouched_paragraph_styles() {
        for (text, range, inserted, expected, indents) in [
            ("a\r\nb", 1..2, "", "a\nb", vec![1.0, 2.0]),
            ("a\r\nb", 2..3, "", "a\rb", vec![1.0, 2.0]),
            ("a\rb", 2..2, "\n", "a\r\nb", vec![1.0, 2.0]),
            ("a\nb", 1..1, "\r", "a\r\nb", vec![1.0, 2.0]),
            ("a\rX\nb", 2..3, "", "a\r\nb", vec![1.0, 3.0]),
            ("a\r\nb", 2..2, "X", "a\rX\nb", vec![1.0, 2.0, 2.0]),
        ] {
            let mut doc = TextDoc::plain(text);
            for (i, p) in doc.para_ranges().into_iter().enumerate() {
                doc.apply_para(p, |p| p.indent_left = (i + 1) as f64);
            }
            doc.replace_range(range, inserted, None);
            assert_eq!(doc.text, expected);
            assert_eq!(doc.paras().iter().map(|p| p.indent_left).collect::<Vec<_>>(), indents, "{text:?} → {expected:?}");
        }
    }

    fn sizes(d: &TextDoc) -> Vec<(usize, f64)> {
        d.runs().iter().map(|r| (r.len, r.style.size)).collect()
    }

    #[test]
    fn style_runs_split_and_merge() {
        let mut d = TextDoc::plain("Hello world");
        assert!(d.is_uniform());
        d.apply_style(6..11, |s| s.size = 40.0);
        assert_eq!(sizes(&d), vec![(6, 72.0), (5, 40.0)]);
        d.apply_style(2..8, |s| s.fill = [1.0, 0.0, 0.0, 1.0]);
        assert_eq!(d.runs().len(), 4);
        assert_eq!(d.style_at(7).fill, [1.0, 0.0, 0.0, 1.0]);
        assert_eq!(d.style_at(7).size, 40.0);
        // Undo the colour: neighbours merge again.
        d.apply_style(0..11, |s| s.fill = [1.0; 4]);
        assert_eq!(sizes(&d), vec![(6, 72.0), (5, 40.0)]);
        d.apply_style(0..11, |s| s.size = 10.0);
        assert!(d.is_uniform());
        assert_eq!(d.size, 10.0);
    }

    #[test]
    fn edits_keep_runs_aligned() {
        let mut d = TextDoc::plain("abcdef");
        d.apply_style(2..4, |s| s.size = 20.0);
        // Typing after "cd" continues its style.
        d.replace_range(4..4, "XY", None);
        assert_eq!(d.text, "abcdXYef");
        assert_eq!(sizes(&d), vec![(2, 72.0), (4, 20.0), (2, 72.0)]);
        // Typing at the very start takes the first character's style.
        d.replace_range(0..0, "_", None);
        assert_eq!(sizes(&d), vec![(3, 72.0), (4, 20.0), (2, 72.0)]);
        // Deleting the whole styled span merges the outer runs.
        d.replace_range(3..7, "", None);
        assert_eq!(d.text, "_abef");
        assert!(d.is_uniform());
        // Replacing a selection uses the first selected character's style.
        d.apply_style(1..2, |s| s.size = 5.0);
        d.replace_range(1..3, "Q", None);
        assert_eq!(d.text, "_Qef");
        assert_eq!(sizes(&d), vec![(1, 72.0), (1, 5.0), (2, 72.0)]);
        // Deleting everything keeps the style of what was there.
        d.replace_range(0..4, "", None);
        assert_eq!(d.text, "");
        assert!(d.is_uniform());
        assert_eq!(d.size, 72.0);
    }

    #[test]
    fn paragraphs_follow_edits() {
        let mut d = TextDoc::plain("one\ntwo\nthree");
        assert_eq!(d.para_count(), 3);
        d.apply_para(4..5, |p| p.indent_left = 30.0);
        assert_eq!(d.paras().iter().map(|p| p.indent_left).collect::<Vec<_>>(), vec![0.0, 30.0, 0.0]);
        // Enter inside "two" splits it: both halves keep the indent.
        d.replace_range(5..5, "\n", None);
        assert_eq!(d.text, "one\nt\nwo\nthree");
        assert_eq!(d.paras().iter().map(|p| p.indent_left).collect::<Vec<_>>(), vec![0.0, 30.0, 30.0, 0.0]);
        // Joining the first two paragraphs keeps the first's settings.
        d.replace_range(3..4, "", None);
        assert_eq!(d.paras().iter().map(|p| p.indent_left).collect::<Vec<_>>(), vec![0.0, 30.0, 0.0]);
        assert_eq!(d.para_ranges(), vec![0..4, 5..7, 8..13]);
        // Space before only on paragraph 3 via a range ending in it.
        d.apply_para(9..10, |p| p.space_before = 12.0);
        assert_eq!(d.para(2).space_before, 12.0);
        assert_eq!(d.para(1).space_before, 0.0);
    }

    #[test]
    fn single_style_serde_is_unchanged_and_old_docs_load() {
        let d = TextDoc::plain("Hi");
        let j = serde_json::to_value(&d).unwrap();
        assert!(j.get("runs").is_none() && j.get("paragraphs").is_none());
        // A document saved before style runs existed (no new fields).
        let old = r#"{"text":"Old","font":"Inter","style":"Bold","size":50.0,"fill":[1,0,0,1],"stroke":[0,0,0,1],
            "stroke_width":0.0,"apply_fill":true,"apply_stroke":false,"stroke_over_fill":true,"tracking":10.0,
            "leading":null,"justify":"Center","baseline_shift":0.0,"h_scale":100.0,"v_scale":100.0,"faux_bold":false,
            "faux_italic":false,"all_caps":false,"small_caps":false,"box_size":[300.0,200.0],"box_pos":[0.0,0.0]}"#;
        let d: TextDoc = serde_json::from_str(old).unwrap();
        assert!(d.is_uniform());
        assert_eq!(d.style_at(2).size, 50.0);
        assert_eq!(d.composer, Composer::SingleLine, "old box text keeps greedy breaking");
        assert_eq!(d.line_join, LineJoin::Round, "old strokes keep their round corners");
        assert_eq!(TextDoc::default().line_join, LineJoin::Miter);
        assert!(d.ligatures);
        assert_eq!(d.kerning, Kerning::Metrics);
        // Round trip with runs.
        let mut m = d.clone();
        m.apply_style(0..1, |s| s.kerning = Kerning::Manual(-40.0));
        let back: TextDoc = serde_json::from_value(serde_json::to_value(&m).unwrap()).unwrap();
        assert_eq!(back, m);
        assert_eq!(back.style_at(0).kerning, Kerning::Manual(-40.0));
    }

    #[test]
    fn slice_and_insert_doc_carry_formatting() {
        let mut d = TextDoc::plain("red blue");
        d.apply_style(0..3, |s| s.fill = [1.0, 0.0, 0.0, 1.0]);
        let frag = d.slice(2..6);
        assert_eq!(frag.text, "d bl");
        assert_eq!(frag.runs().len(), 2);
        let mut e = TextDoc::plain("xx");
        e.insert_doc(1..1, &frag);
        assert_eq!(e.text, "xd blx");
        assert_eq!(e.style_at(1).fill, [1.0, 0.0, 0.0, 1.0]);
        assert_eq!(e.style_at(2).fill, [1.0; 4]);
    }

    #[test]
    fn attrs_by_key() {
        let mut d = TextDoc::plain("abc\ndef");
        assert!(d.set_attr("size", &J::from(20), Some(0..2)).unwrap());
        assert!(d.set_attr("kerning", &J::from("optical"), None).unwrap());
        assert!(d.set_attr("justify", &J::from("justifyAll"), Some(5..5)).unwrap());
        assert!(d.set_attr("superscript", &J::from(true), Some(2..3)).unwrap());
        assert!(!d.set_attr("nope", &J::from(1), None).unwrap());
        assert!(d.set_attr("size", &J::from("big"), None).is_err());
        assert_eq!(d.style_at(1).size, 20.0);
        assert_eq!(d.style_at(2).baseline, BaselineOption::Superscript);
        assert_eq!(d.style_at(6).kerning, Kerning::Optical);
        assert_eq!(d.para(1).justify, Justify::JustifyAll);
        assert_eq!(d.para(0).justify, Justify::Left);
        let j = char_style_json(&d.style_at(0));
        let mut s = CharStyle::default();
        for (k, v) in j.as_object().unwrap() {
            assert!(apply_char_attr(&mut s, k, v).unwrap(), "{k}");
        }
        assert_eq!(s, d.style_at(0));
    }

    #[test]
    fn word_boundaries() {
        let t = "hello, big world";
        assert_eq!(word_right(t, 0), 5);
        assert_eq!(word_right(t, 5), 10);
        assert_eq!(word_left(t, 16), 11);
        assert_eq!(word_left(t, 11), 7);
        assert_eq!(word_at(t, 8), 7..10);
        assert_eq!(word_at(t, 6), 6..7);
    }

    #[test]
    fn opentype_attributes_per_range_and_serde() {
        let mut d = TextDoc::plain("Office 1/2 1st");
        // Untouched documents serialize without an OpenType block.
        assert!(!serde_json::to_string(&d).unwrap().contains("opentype"));
        d.set_attr("ss02", &J::Bool(true), Some(0..6)).unwrap();
        d.set_attr("fractions", &J::Bool(true), Some(7..10)).unwrap();
        d.set_attr("figures", &J::from("tabularOldstyle"), None).unwrap();
        assert!(d.style_at(0).opentype.stylistic_set(2));
        assert!(!d.style_at(8).opentype.stylistic_set(2));
        assert!(d.style_at(8).opentype.fractions);
        assert_eq!(d.style_at(12).opentype.figure_width, FigureWidth::Tabular);
        assert!(d.set_attr("stylisticSets", &serde_json::json!([1, 20]), Some(0..1)).unwrap());
        assert_eq!(d.style_at(0).opentype.stylistic_sets, 1 | (1 << 19));
        assert!(d.set_attr("stylisticSets", &serde_json::json!([21]), None).is_err());
        let f = d.style_at(8).opentype.feature_settings();
        assert!(f.contains(&(*b"frac", 1)) && f.contains(&(*b"onum", 1)) && f.contains(&(*b"tnum", 1)));
        let mut s = d.style_at(0);
        s.opentype.contextual_alternates = false;
        assert!(s.opentype.feature_settings().contains(&(*b"calt", 0)));
        // Round trip, and the JSON form names the attributes as layer.setText does.
        let back: TextDoc = serde_json::from_str(&serde_json::to_string(&d).unwrap()).unwrap();
        assert_eq!(back, d);
        let j = char_style_json(&d.style_at(0));
        assert_eq!(j["stylisticSets"], serde_json::json!([1, 20]));
        assert_eq!(j["figureStyle"], "oldStyle");
        // All Small Caps and All Caps exclude each other.
        let mut s = CharStyle::default();
        apply_char_attr(&mut s, "allCaps", &J::Bool(true)).unwrap();
        apply_char_attr(&mut s, "allSmallCaps", &J::Bool(true)).unwrap();
        assert!(!s.all_caps && s.opentype.all_small_caps);
    }
}

//! Source Text documents: text with character style runs and per-paragraph settings.
//!
//! A [`TextDoc`] keeps its *base* character style and paragraph settings in flat fields (the
//! style of the first character and the settings of the first paragraph), which is all a
//! single-style document needs and what older projects contain. Mixed formatting is stored in
//! [`TextDoc::runs`] (character style runs covering the text, by character count) and
//! [`TextDoc::paragraphs`] (one entry per CR, LF or CRLF-separated paragraph). Both lists are
//! empty while the document is uniform, so single-style documents serialize exactly as before.
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
            "left" | "leftjustify" => Justify::Left,
            "center" | "centre" | "centerjustify" => Justify::Center,
            "right" | "rightjustify" => Justify::Right,
            "justify" | "justifyleft" | "justifylastleft" | "fulljustifylastlineleft" => Justify::JustifyLastLeft,
            "justifycenter" | "justifylastcenter" | "fulljustifylastlinecenter" => Justify::JustifyLastCenter,
            "justifyright" | "justifylastright" | "fulljustifylastlineright" => Justify::JustifyLastRight,
            "justifyall" | "fulljustifylastlinefull" | "fulljustify" => Justify::JustifyAll,
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
            self.apply_style(r.start..r.end.max(r.start + usize::from(n == 0)), |s| {
                let _ = apply_char_attr(s, key, v);
            });
            return Ok(true);
        }
        Ok(false)
    }
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
                if m.is_empty() {
                    s.variations.clear();
                }
                for (tag, val) in m {
                    if tag.is_empty() || tag.len() > 4 {
                        return Err(format!("variations: `{tag}` is not an axis tag"));
                    }
                    s.variations.retain(|(t, _)| t != tag);
                    match val {
                        J::Null => {}
                        _ => s.variations.push((tag.clone(), val.as_f64().ok_or("variations: expected {tag: number}")? as f32)),
                    }
                }
                s.variations.sort_by(|a, b| a.0.cmp(&b.0));
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

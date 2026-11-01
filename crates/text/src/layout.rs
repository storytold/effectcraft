//! Paragraph layout: font fallback, bidi (UAX #9), shaping (OpenType via harfrust: kerning,
//! ligatures, mark positioning, complex scripts), line breaking (UAX #14, greedy or every-line),
//! alignment and justification, indents, paragraph spacing, leading, tracking, tsume, manual and
//! optical kerning, baseline shift, superscript / subscript, horizontal / vertical scale, all caps
//! / small caps, hanging punctuation, underline. Text may mix character styles (runs) and
//! paragraph settings. Results are cached.
//!
//! Coordinates are pixels, y down. For **point text** (`ParagraphStyle::width == None`) the
//! origin is the alignment point on the first baseline: x = 0 is the left edge (left / justify),
//! the centre (centre) or the right edge (right). For **area text** (`width == Some(w)`) the
//! origin is the top-left of the text box and lines wrap at `w`.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::sync::{Arc, Mutex, OnceLock};

use harfrust::{Direction, Feature, Tag, UnicodeBuffer};
use unicode_bidi::{BidiInfo, Level};

use effectcraft_keyframe::{OpenType, text_doc::paragraph_ranges};

use crate::fonts::{self, FaceId, Resolved, VMetrics};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Caps {
    #[default]
    Normal,
    All,
    Small,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Align {
    #[default]
    Left,
    Center,
    Right,
    Justify,
}

/// Superscript / subscript position.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Script {
    #[default]
    Normal,
    Super,
    Sub,
}

/// Superscript / subscript size and baseline offset (fractions of the font size).
pub const SCRIPT_SIZE: f32 = 0.583;
pub const SCRIPT_OFFSET: f32 = 0.333;

/// Character formatting.
#[derive(Clone, Debug, PartialEq)]
pub struct TextStyle {
    pub family: String,
    pub style: String,
    /// Font size in pixels.
    pub size: f32,
    /// Tracking in 1/1000 em (added after every cluster).
    pub tracking: f32,
    /// Metric kerning (`kern`).
    pub kerning: bool,
    /// Optical kerning (spacing from the outlines; replaces metric kerning).
    pub optical: bool,
    /// Manual kerning before every character of the run, in 1/1000 em (replaces metrics).
    pub manual_kern: Option<f32>,
    /// Standard ligatures (`liga`, `clig`).
    pub ligatures: bool,
    /// Baseline shift in pixels (positive = up).
    pub baseline_shift: f32,
    pub faux_bold: bool,
    pub faux_italic: bool,
    pub caps: Caps,
    pub underline: bool,
    /// Horizontal / vertical scale (1 = 100%).
    pub h_scale: f32,
    pub v_scale: f32,
    /// Tsume: fraction (0–1) of the space around each glyph removed.
    pub tsume: f32,
    pub script: Script,
    /// Baseline-to-baseline distance in pixels; None = auto (120% of the size).
    pub leading: Option<f32>,
    /// OpenType features (stylistic sets, figures, fractions, all small caps…).
    pub opentype: OpenType,
    /// Variable font axis values (tag, user units): shaping (advances through HVAR / gvar
    /// phantom points) and outlines use this design-space position.
    pub variations: Vec<(String, f32)>,
    /// Use vertical alternates for upright CJK glyphs (runtime layout setting only).
    pub vertical: bool,
}

impl Default for TextStyle {
    fn default() -> Self {
        Self {
            family: fonts::DEFAULT_FAMILY.into(),
            style: "Regular".into(),
            size: 100.0,
            tracking: 0.0,
            kerning: true,
            optical: false,
            manual_kern: None,
            ligatures: true,
            baseline_shift: 0.0,
            faux_bold: false,
            faux_italic: false,
            caps: Caps::Normal,
            underline: false,
            h_scale: 1.0,
            v_scale: 1.0,
            tsume: 0.0,
            script: Script::Normal,
            leading: None,
            opentype: OpenType::default(),
            variations: Vec::new(),
            vertical: false,
        }
    }
}

impl TextStyle {
    /// The size glyphs are drawn at (superscript / subscript are smaller).
    pub fn glyph_size(&self) -> f32 {
        match self.script {
            Script::Normal => self.size,
            _ => self.size * SCRIPT_SIZE,
        }
    }
    /// Upward baseline offset: baseline shift plus the superscript / subscript offset.
    pub fn rise(&self) -> f32 {
        self.baseline_shift
            + match self.script {
                Script::Normal => 0.0,
                Script::Super => self.size * SCRIPT_OFFSET,
                Script::Sub => -self.size * SCRIPT_OFFSET,
            }
    }
    /// The line advance this style asks for.
    pub fn line_advance(&self) -> f32 {
        self.leading.unwrap_or(self.size * 1.2)
    }
}

/// Paragraph formatting.
#[derive(Clone, Debug, PartialEq)]
pub struct ParagraphStyle {
    pub align: Align,
    /// How the last line of a justified paragraph is aligned (`Justify` = justify it too).
    pub justify_last: Align,
    /// Extra line spacing in pixels added to every line advance.
    pub leading: f32,
    /// Wrap width (area text); None = point text. Taken from the first paragraph.
    pub width: Option<f32>,
    /// Base direction: None = from the first strong character.
    pub rtl: Option<bool>,
    /// Start / end margins (left / right in left-to-right paragraphs) and the first line's indent.
    pub indent_start: f32,
    pub indent_end: f32,
    pub indent_first: f32,
    pub space_before: f32,
    pub space_after: f32,
    /// Every-line composer (balanced breaks) instead of single-line (greedy).
    pub every_line: bool,
    /// Roman hanging punctuation.
    pub hanging: bool,
}

impl Default for ParagraphStyle {
    fn default() -> Self {
        Self {
            align: Align::Left,
            justify_last: Align::Left,
            leading: 0.0,
            width: None,
            rtl: None,
            indent_start: 0.0,
            indent_end: 0.0,
            indent_first: 0.0,
            space_before: 0.0,
            space_after: 0.0,
            every_line: false,
            hanging: false,
        }
    }
}

fn hf(h: &mut impl Hasher, v: f32) {
    v.to_bits().hash(h);
}

impl Hash for TextStyle {
    fn hash<H: Hasher>(&self, h: &mut H) {
        self.family.hash(h);
        self.style.hash(h);
        for v in [self.size, self.tracking, self.baseline_shift, self.h_scale, self.v_scale, self.tsume] {
            hf(h, v);
        }
        self.manual_kern.map(f32::to_bits).hash(h);
        self.leading.map(f32::to_bits).hash(h);
        (self.kerning, self.optical, self.ligatures, self.faux_bold, self.faux_italic, self.caps, self.underline, self.script).hash(h);
        self.opentype.hash(h);
        self.vertical.hash(h);
        for (t, v) in &self.variations {
            t.hash(h);
            hf(h, *v);
        }
    }
}

impl Hash for ParagraphStyle {
    fn hash<H: Hasher>(&self, h: &mut H) {
        (self.align, self.justify_last).hash(h);
        for v in [self.leading, self.indent_start, self.indent_end, self.indent_first, self.space_before, self.space_after] {
            hf(h, v);
        }
        self.width.map(f32::to_bits).hash(h);
        (self.rtl, self.every_line, self.hanging).hash(h);
    }
}

/// A positioned glyph. `(x, y)` is the glyph origin on its baseline.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Glyph {
    pub face: FaceId,
    pub id: u32,
    pub x: f32,
    pub y: f32,
    pub size: f32,
    /// Byte offset of the cluster in the source text.
    pub cluster: usize,
    pub synth_bold: bool,
    pub synth_italic: bool,
    /// Horizontal / vertical scale of the outline (1 = 100%).
    pub h_scale: f32,
    pub v_scale: f32,
    /// Index of the style run the glyph belongs to.
    pub run: usize,
    /// The run's variable font axis values, interned ([`crate::variable::coords`]; 0 = the
    /// default instance): outlines are drawn there.
    pub variations: u32,
}

/// One laid-out line.
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    /// Byte range in the source text (without the terminating newline).
    pub range: Range<usize>,
    pub baseline: f32,
    /// Left edge and width of the line's content (trailing spaces excluded).
    pub x: f32,
    pub width: f32,
    pub ascent: f32,
    pub descent: f32,
    pub glyphs: Range<usize>,
    /// Caret stops `(byte offset, x)` for every character boundary in the line, by byte offset.
    pub carets: Vec<(usize, f32)>,
    /// Paragraph index.
    pub para: usize,
    /// The line ends its paragraph.
    pub para_end: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Layout {
    pub glyphs: Vec<Glyph>,
    pub lines: Vec<Line>,
    /// Underline rectangles `[x0, y0, x1, y1]`.
    pub underlines: Vec<[f32; 4]>,
    /// Logical bounds `[x0, y0, x1, y1]` (line boxes; area text spans the box width).
    pub bounds: [f32; 4],
    /// The face requested was missing (substituted).
    pub missing_font: bool,
    pub text_len: usize,
}

impl Layout {
    /// Line index for a caret at byte `pos`.
    pub fn line_of(&self, pos: usize) -> usize {
        let mut li = 0;
        for (i, l) in self.lines.iter().enumerate() {
            if l.range.start <= pos {
                li = i;
            }
        }
        li
    }
    /// Caret position `(x, baseline, line)` for byte offset `pos`.
    pub fn caret(&self, pos: usize) -> (f32, f32, usize) {
        let li = self.line_of(pos);
        let Some(l) = self.lines.get(li) else { return (0.0, 0.0, 0) };
        let x = l.carets.iter().min_by_key(|(b, _)| b.abs_diff(pos)).map_or(l.x, |c| c.1);
        (x, l.baseline, li)
    }
    /// Nearest caret byte offset to point `(x, y)`.
    pub fn hit(&self, x: f32, y: f32) -> usize {
        let Some(l) = self.lines.iter().min_by(|a, b| line_dist(a, y).total_cmp(&line_dist(b, y))) else {
            return 0;
        };
        self.hit_line(l, x)
    }
    /// Nearest caret byte offset to `x` on line `l`. A soft-wrapped line's end is the caret
    /// before its trailing space, so clicking past it doesn't land on the next line.
    pub fn hit_line(&self, l: &Line, x: f32) -> usize {
        let end = self.line_end(l);
        l.carets.iter().filter(|c| c.0 <= end).min_by(|a, b| (a.1 - x).abs().total_cmp(&(b.1 - x).abs())).map_or(l.range.start, |c| c.0)
    }
    /// The last caret stop of a line (before a wrapped line's trailing space).
    pub fn line_end(&self, l: &Line) -> usize {
        if l.para_end {
            return l.range.end;
        }
        l.carets.iter().rev().nth(1).map(|c| c.0).filter(|b| *b >= l.range.start).unwrap_or(l.range.end)
    }
    /// Selection highlight rectangles `[x0, y0, x1, y1]` for the byte range `a..b`.
    pub fn selection_rects(&self, a: usize, b: usize) -> Vec<[f32; 4]> {
        let (a, b) = (a.min(b), a.max(b));
        let mut out = Vec::new();
        for l in &self.lines {
            let s = a.max(l.range.start);
            let e = b.min(l.range.end);
            if s > e || (s == e && !(a < l.range.start && b > l.range.end)) {
                continue;
            }
            let xs: Vec<f32> = l.carets.iter().filter(|(p, _)| *p >= s && *p <= e).map(|c| c.1).collect();
            if xs.len() < 2 && b <= l.range.end {
                continue;
            }
            let x0 = xs.iter().copied().fold(f32::MAX, f32::min);
            let mut x1 = xs.iter().copied().fold(f32::MIN, f32::max);
            if x0 > x1 {
                x1 = l.x;
            }
            if b > l.range.end && l.para_end {
                x1 += l.ascent * 0.25; // selected newline
            }
            out.push([x0.min(x1), l.baseline - l.ascent, x1, l.baseline + l.descent]);
        }
        out
    }
}

fn line_dist(l: &Line, y: f32) -> f32 {
    let (t, b) = (l.baseline - l.ascent, l.baseline + l.descent);
    if y < t {
        t - y
    } else if y > b {
        y - b
    } else {
        0.0
    }
}

struct ShapedGlyph {
    face: FaceId,
    id: u32,
    cluster: usize,
    adv: f32,
    /// Space added before the glyph (manual / optical kerning).
    pre: f32,
    dx: f32,
    dy: f32,
    size: f32,
    run: usize,
    /// Upward baseline offset (baseline shift, plus a synthesized superscript / subscript's).
    rise: f32,
}

struct Item {
    range: Range<usize>,
    rtl: bool,
    glyphs: Vec<ShapedGlyph>,
}

fn upper_single(c: char) -> char {
    let mut u = c.to_uppercase();
    match (u.next(), u.next()) {
        (Some(x), None) => x,
        _ => c,
    }
}

const SMALL_CAPS_SCALE: f32 = 0.78;

/// How a character is set: as itself, as a synthesized small capital (a scaled-down capital),
/// with the font's small-capital glyph (`smcp` / `c2sc`), or with the font's superior / inferior
/// glyph (`sups` / `subs`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Form {
    Normal,
    FauxSmall,
    TrueSmall,
    TrueScript,
}

/// Whether OpenType feature `tag` gives character `c` its own glyph in `face` (cached): the
/// true small caps / superscripts / subscripts test, with synthesized forms as the fallback.
pub fn feature_substitutes(face: FaceId, tag: &[u8; 4], c: char) -> bool {
    static C: OnceLock<Mutex<HashMap<(FaceId, [u8; 4], char), bool>>> = OnceLock::new();
    let cache = C.get_or_init(Default::default);
    if let Some(v) = cache.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get(&(face, *tag, c)) {
        return *v;
    }
    let f = fonts::face(face);
    let v = f.has_feature(tag)
        && match (f.font(), f.shaper_data()) {
            (Some(font), Some(data)) => {
                let shaper = data.shaper(&font).build();
                let shape = |feats: &[Feature]| -> Vec<u32> {
                    let mut buf = UnicodeBuffer::new();
                    buf.add(c, 0);
                    buf.guess_segment_properties();
                    shaper.shape(buf, harfrust::ShapeOptions::new().features(feats)).glyph_infos().iter().map(|i| i.glyph_id).collect()
                };
                let plain = shape(&[]);
                let with = shape(&[Feature::new(Tag::new(tag), 1, ..)]);
                plain != with && !with.contains(&0)
            }
            _ => false,
        };
    let mut m = cache.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if m.len() > 100_000 {
        m.clear();
    }
    m.insert((face, *tag, c), v);
    v
}

/// Horizontal ink extent `(x0, x1)` of a glyph in font units.
fn ink_x(face: FaceId, gid: u32) -> Option<(f32, f32)> {
    let p = crate::outline_units(face, gid)?;
    if p.elements().is_empty() {
        return None;
    }
    let r = kurbo::Shape::bounding_box(&*p);
    Some((r.x0 as f32, r.x1 as f32))
}

fn shape_item(chars: &[(usize, char, char)], rtl: bool, face: FaceId, size: f32, style: &TextStyle, run: usize, form: Form) -> Vec<ShapedGlyph> {
    let f = fonts::face(face);
    let (Some(font), Some(data)) = (f.font(), f.shaper_data()) else { return Vec::new() };
    let instance = (!style.variations.is_empty()).then(|| {
        harfrust::ShaperInstance::from_variations(
            &font,
            style.variations.iter().map(|(t, v)| harfrust::Variation { tag: crate::variable::tag_of(t), value: *v }),
        )
    });
    let shaper = data.shaper(&font).instance(instance.as_ref()).build();
    let mut buf = UnicodeBuffer::new();
    for &(b, _, sc) in chars {
        buf.add(if sc == '\t' { ' ' } else { sc }, b as u32);
    }
    buf.set_direction(if rtl { Direction::RightToLeft } else { Direction::LeftToRight });
    buf.guess_segment_properties();
    let mut feats = Vec::new();
    if style.vertical && chars.first().is_some_and(|(_, c, _)| crate::is_cjk(*c)) {
        feats.push(Feature::new(Tag::new(b"vert"), 1, ..));
        feats.push(Feature::new(Tag::new(b"vrt2"), 1, ..));
    }
    if !style.kerning || style.optical || style.manual_kern.is_some() {
        feats.push(Feature::new(Tag::new(b"kern"), 0, ..));
    }
    if !style.ligatures {
        feats.push(Feature::new(Tag::new(b"liga"), 0, ..));
        feats.push(Feature::new(Tag::new(b"clig"), 0, ..));
    }
    // OpenType options; small caps go per character (true where the font has them).
    for (tag, v) in style.opentype.feature_settings() {
        if &tag != b"smcp" && &tag != b"c2sc" {
            feats.push(Feature::new(Tag::new(&tag), v, ..));
        }
    }
    if style.caps == Caps::All {
        // Case-sensitive forms (punctuation raised to sit with capitals).
        feats.push(Feature::new(Tag::new(b"case"), 1, ..));
    }
    let rise = match form {
        Form::TrueScript => style.baseline_shift,
        _ => style.rise(),
    };
    match form {
        Form::TrueSmall => {
            feats.push(Feature::new(Tag::new(b"smcp"), 1, ..));
            if style.opentype.all_small_caps {
                feats.push(Feature::new(Tag::new(b"c2sc"), 1, ..));
            }
        }
        Form::TrueScript => feats.push(Feature::new(Tag::new(if style.script == Script::Sub { b"subs" } else { b"sups" }), 1, ..)),
        _ => {}
    }
    let out = shaper.shape(buf, harfrust::ShapeOptions::new().features(&feats));
    let k = size / f.units_per_em();
    let hs = style.h_scale;
    out.glyph_infos()
        .iter()
        .zip(out.glyph_positions())
        .map(|(i, p)| ShapedGlyph {
            face,
            id: i.glyph_id,
            cluster: i.cluster as usize,
            adv: p.x_advance as f32 * k * hs,
            pre: 0.0,
            dx: p.x_offset as f32 * k * hs,
            dy: p.y_offset as f32 * k,
            size,
            run,
            rise,
        })
        .collect()
}

/// Tsume, tracking, manual and optical kerning on one shaped item (logical order).
fn space_item(glyphs: &mut [ShapedGlyph], style: &TextStyle, text: &str, para_start: bool) {
    let side = |g: &ShapedGlyph| -> Option<(f32, f32)> {
        let (x0, x1) = ink_x(g.face, g.id)?;
        let k = g.size / fonts::face(g.face).units_per_em() * style.h_scale;
        Some((x0 * k, g.adv - x1 * k))
    };
    let is_space = |g: &ShapedGlyph| text[g.cluster..].chars().next().is_none_or(char::is_whitespace);
    if style.tsume > 0.0 {
        let t = style.tsume.clamp(0.0, 1.0);
        for g in glyphs.iter_mut() {
            if is_space(g) {
                continue;
            }
            if let Some((lsb, rsb)) = side(g) {
                g.dx -= t * lsb.max(0.0);
                g.adv -= t * (lsb.max(0.0) + rsb.max(0.0));
            }
        }
    }
    if style.optical && glyphs.len() > 1 {
        // Pull every pair's ink gap halfway toward the item's median gap.
        let sides: Vec<Option<(f32, f32)>> = glyphs.iter().map(|g| if is_space(g) { None } else { side(g) }).collect();
        let gaps: Vec<Option<f32>> = (1..glyphs.len()).map(|i| Some(sides[i - 1]?.1 + sides[i]?.0)).collect();
        let mut sorted: Vec<f32> = gaps.iter().flatten().copied().collect();
        sorted.sort_by(f32::total_cmp);
        if let Some(&median) = sorted.get(sorted.len() / 2) {
            for (i, gap) in gaps.iter().enumerate() {
                if let Some(gap) = gap
                    && glyphs[i + 1].cluster != glyphs[i].cluster
                {
                    glyphs[i + 1].pre += (median - gap) * 0.5;
                }
            }
        }
    }
    if let Some(m) = style.manual_kern {
        let k = m * style.size / 1000.0;
        for i in 0..glyphs.len() {
            let first_of_cluster = i == 0 || glyphs[i - 1].cluster != glyphs[i].cluster;
            if first_of_cluster && !(para_start && glyphs[i].cluster == 0) {
                glyphs[i].pre += k;
            }
        }
    }
    if style.tracking != 0.0 {
        let t = style.tracking * style.size / 1000.0;
        for k in 0..glyphs.len() {
            if k + 1 == glyphs.len() || glyphs[k + 1].cluster != glyphs[k].cluster {
                glyphs[k].adv += t;
            }
        }
    }
}

/// After Effects' first-baseline offset of paragraph (box) text for a face at nominal size
/// `px`: the height of its lowercase `d`, in whole thousandths of an em. Measured in After
/// Effects 2026 (case 818): the first baseline sits 69.50 px below the box top for Times New
/// Roman at 100 px (its `d` is 1422/2048 em, 694.3 thousandths, rounded 695) and 71.60 px for
/// Arial (1466/2048, 715.8, rounded 716), at 50 px half of that; superscript text keeps the
/// line where normal text puts it and only raises its glyphs. `None` without a `d` outline.
fn first_ascent(face: FaceId, px: f32) -> Option<f32> {
    let f = fonts::face(face);
    let gid = f.glyph('d')?;
    let units = crate::outline_units(face, gid)?;
    let top = kurbo::Shape::bounding_box(&*units).y1 as f32;
    let em = (top / f.units_per_em() * 1000.0).round() / 1000.0;
    plausible_first(em, px, f.metrics(px).ascent)
}

/// A `d` at least this fraction of the font ascent is a Latin lowercase ascender; a shorter one
/// (a symbol or icon font's `d`) would put the first baseline almost at the box top, so the font
/// ascent is used instead (the rule before the `d` height).
const MIN_D_OF_ASCENT: f32 = 0.3;

/// The first baseline `em * px` below the box top, if `em` (the `d` height in em) is plausible:
/// finite, positive, under 4 em and at least [`MIN_D_OF_ASCENT`] of the font `ascent` (px).
fn plausible_first(em: f32, px: f32, ascent: f32) -> Option<f32> {
    let h = em * px;
    let tall_enough = !(ascent.is_finite() && ascent > 0.0) || h >= MIN_D_OF_ASCENT * ascent;
    (h.is_finite() && h > 0.0 && em < 4.0 && tall_enough).then_some(h)
}

struct ParaLine {
    range: Range<usize>,
    glyphs: Vec<Glyph>,
    carets: Vec<(usize, f32)>,
    content: f32,
    left_trim: f32,
    /// Hanging punctuation widths at the left / right edge.
    hang: (f32, f32),
    ascent: f32,
    descent: f32,
    /// Where a box's first line puts its baseline below the box top ([`first_ascent`] of the
    /// line's styles at their nominal size; the font ascent where a face has no `d`).
    first: f32,
    /// Line advance (largest leading on the line).
    advance: f32,
    align: Align,
    avail: Option<f32>,
    margins: (f32, f32),
    underline: Option<(f32, f32)>,
}

fn is_hanging_close(c: char) -> bool {
    matches!(c, '.' | ',' | ';' | ':' | '!' | '?' | '-' | '\u{2013}' | '\u{2014}' | '\'' | '"' | '\u{2019}' | '\u{201d}' | ')' | '\u{2026}')
}
fn is_hanging_open(c: char) -> bool {
    matches!(c, '"' | '\'' | '\u{2018}' | '\u{201c}' | '(' | '\u{ab}')
}

/// The style run index covering global byte `b` (the run before it at a boundary, for empty
/// paragraphs at the end).
fn run_at(runs: &[(Range<usize>, TextStyle)], b: usize) -> usize {
    runs.iter().position(|(r, _)| b >= r.start && b < r.end).unwrap_or_else(|| runs.iter().rposition(|(r, _)| r.start <= b).unwrap_or(0))
}

/// Lay out one paragraph (no newlines) whose text starts at byte `base` of the whole string.
/// Margins (left, right) of a paragraph's first or later lines.
/// `rtl` is the resolved paragraph direction (an unset direction follows the text).
fn margins_for(para: &ParagraphStyle, rtl: bool, first: bool) -> (f32, f32) {
    let fi = if first { para.indent_first } else { 0.0 };
    if rtl { (para.indent_end, para.indent_start + fi) } else { (para.indent_start + fi, para.indent_end) }
}

/// The width a line may fill (`None` for point text).
fn avail_for(para: &ParagraphStyle, rtl: bool, width: Option<f32>, first: bool) -> Option<f32> {
    let (l, r) = margins_for(para, rtl, first);
    width.map(|w| (w - l - r).max(1.0))
}

/// The alignment of a line, justified lines that cannot stretch falling back to `justify_last`.
fn align_for(para: &ParagraphStyle, rtl: bool, width: Option<f32>, last: bool) -> Align {
    if rtl && width.is_none() && para.align == Align::Justify {
        // Right-to-left point text with a justified paragraph sits flush at its start, the
        // right (After Effects 2026, every justify code; left-to-right not measured).
        Align::Right
    } else if (last || width.is_none()) && para.align == Align::Justify {
        if width.is_none() && para.justify_last == Align::Justify { Align::Left } else { para.justify_last }
    } else {
        para.align
    }
}

fn style_metrics(primaries: &[Resolved], runs: &[(Range<usize>, TextStyle)], si: usize) -> VMetrics {
    fonts::face(primaries[si].face).metrics(runs[si].1.glyph_size())
}

/// The baseline of a box's first line for style run `si`: the `d` height, else the font ascent.
fn style_first(primaries: &[Resolved], runs: &[(Range<usize>, TextStyle)], si: usize) -> f32 {
    let st = &runs[si].1;
    first_ascent(primaries[si].face, st.size).unwrap_or_else(|| fonts::face(primaries[si].face).metrics(st.size).ascent)
}

/// A char's shaping inputs: (byte, char, shaped char, face, rtl, form, run).
type CharInfo = (usize, char, char, FaceId, bool, Form, usize);

/// The glyphs of one line in visual order, where its pen ended, the extent of each cluster by
/// byte, and the underline span.
struct Placed {
    glyphs: Vec<Glyph>,
    pen: f32,
    extents: HashMap<usize, (f32, f32, bool)>,
    underline: Option<(f32, f32)>,
}

/// A paragraph shaped and measured, ready to be broken into lines.
struct Para<'a> {
    text: &'a str,
    base: usize,
    runs: &'a [(Range<usize>, TextStyle)],
    primaries: &'a [Resolved],
    variation_ids: &'a [u32],
    para: &'a ParagraphStyle,
    width: Option<f32>,
    bidi: BidiInfo<'a>,
    base_rtl: bool,
    chars: Vec<CharInfo>,
    items: Vec<Item>,
    /// Advance of the glyphs at each byte, and prefix sums over it.
    char_adv: Vec<f32>,
    prefix_adv: Vec<f64>,
    previous_non_ws: Vec<usize>,
}

impl<'a> Para<'a> {
    fn new(
        text: &'a str,
        base: usize,
        runs: &'a [(Range<usize>, TextStyle)],
        primaries: &'a [Resolved],
        variation_ids: &'a [u32],
        para: &'a ParagraphStyle,
        width: Option<f32>,
    ) -> Self {
        let default_level = para.rtl.map(|r| if r { Level::rtl() } else { Level::ltr() });
        let bidi = BidiInfo::new(text, default_level);
        let base_rtl = bidi.paragraphs[0].level.is_rtl();
        let chars = char_forms(text, base, runs, primaries, &bidi);
        let items = build_items(text, &chars, runs);
        // Prefix widths and the preceding non-whitespace boundary make candidate line widths O(1).
        // Every-line composition evaluates O(n²) candidate breaks; summing each candidate's bytes
        // or rescanning its trailing whitespace would otherwise add another factor of text length.
        let mut char_adv = vec![0.0f32; text.len() + 1];
        for it in &items {
            for g in &it.glyphs {
                char_adv[g.cluster.min(text.len())] += g.adv + g.pre;
            }
        }
        let mut prefix_adv = vec![0.0f64; text.len() + 1];
        for i in 0..text.len() {
            prefix_adv[i + 1] = prefix_adv[i] + f64::from(char_adv[i]);
        }
        let mut previous_non_ws = vec![0usize; text.len() + 1];
        let mut non_ws_end = 0usize;
        for (b, c) in text.char_indices() {
            previous_non_ws[b] = non_ws_end;
            if !c.is_whitespace() {
                non_ws_end = b + c.len_utf8();
            }
            previous_non_ws[b + c.len_utf8()] = non_ws_end;
        }
        Para { text, base, runs, primaries, variation_ids, para, width, bidi, base_rtl, chars, items, char_adv, prefix_adv, previous_non_ws }
    }

    fn range_w(&self, r: Range<usize>) -> f32 {
        (self.prefix_adv[r.end] - self.prefix_adv[r.start]) as f32
    }

    fn trailing_ws(&self, r: &Range<usize>) -> (usize, f32) {
        let end = self.previous_non_ws[r.end].max(r.start).min(r.end);
        (end, (self.prefix_adv[r.end] - self.prefix_adv[end]) as f32)
    }

    /// Hanging punctuation widths (left, right) of a line's content.
    fn hang_of(&self, r: &Range<usize>, ce: usize) -> (f32, f32) {
        if !self.para.hanging || self.width.is_none() || r.start >= ce {
            return (0.0, 0.0);
        }
        let first = self.text[r.start..ce].chars().next().filter(|c| is_hanging_open(*c)).map_or(0.0, |_| self.char_adv[r.start]);
        let last = self.text[r.start..ce].char_indices().next_back().filter(|(_, c)| is_hanging_close(*c)).map_or(0.0, |(b, _)| self.char_adv[r.start + b]);
        (first, last)
    }

    fn fits_w(&self, r: &Range<usize>) -> f32 {
        let (ce, _) = self.trailing_ws(r);
        let (hl, hr) = self.hang_of(r, ce);
        self.range_w(r.start..ce) - hl - hr
    }

    fn line_avail(&self, line: usize) -> f32 {
        avail_for(self.para, self.base_rtl, self.width, line == 0).unwrap_or(f32::MAX)
    }

    /// The byte ranges of the lines, for the wrap width.
    fn break_ranges(&self) -> Vec<Range<usize>> {
        if self.width.is_none() {
            return std::iter::once(0..self.text.len()).collect();
        }
        let opps: Vec<usize> = unicode_linebreak::linebreaks(self.text).map(|(p, _)| p).collect();
        let mut ranges = if self.para.every_line && opps.len() > 1 && opps.len() <= 1500 { self.optimal_breaks(&opps) } else { self.greedy_breaks(opps) };
        ranges.retain(|r| !r.is_empty());
        if ranges.is_empty() {
            ranges.push(0..self.text.len());
        }
        ranges
    }

    /// Minimises the summed squared slack of every line but the last; with Justify All the last
    /// line is stretched too, so its slack counts (After Effects then balances the lines: 4 / 4 / 4
    /// words where the other modes break 6 / 6 / 1).
    fn optimal_breaks(&self, opps: &[usize]) -> Vec<Range<usize>> {
        let last_counts = self.para.align == Align::Justify && self.para.justify_last == Align::Justify;
        let nodes: Vec<usize> = std::iter::once(0).chain(opps.iter().copied()).collect();
        let m = nodes.len();
        let mut best = vec![(f32::INFINITY, 0usize); m];
        best[0] = (0.0, 0);
        for j in 1..m {
            for i in (0..j).rev() {
                if !best[i].0.is_finite() {
                    continue;
                }
                let r = nodes[i]..nodes[j];
                let w = self.fits_w(&r);
                let maxw = self.line_avail(usize::from(i != 0));
                let over = w > maxw;
                if over && i + 1 != j {
                    break;
                }
                let slack = maxw - w;
                let cost = if over {
                    1e12 + w
                } else if j == m - 1 && !last_counts {
                    0.0
                } else {
                    slack * slack
                };
                let total = best[i].0 + cost;
                if total < best[j].0 {
                    best[j] = (total, i);
                }
            }
        }
        let mut ranges = Vec::new();
        let mut j = m - 1;
        while j > 0 {
            let i = best[j].1;
            ranges.push(nodes[i]..nodes[j]);
            j = i;
        }
        ranges.reverse();
        ranges
    }

    /// Breaks at each opportunity that still fits, else at the last one that did.
    fn greedy_breaks(&self, opps: Vec<usize>) -> Vec<Range<usize>> {
        let mut ranges = Vec::new();
        let mut start = 0usize;
        let mut last_ok: Option<usize> = None;
        for p in opps {
            loop {
                let w = self.fits_w(&(start..p));
                if w <= self.line_avail(ranges.len()) || last_ok.is_none() {
                    last_ok = Some(p);
                    break;
                }
                let Some(b) = last_ok.take() else { break };
                ranges.push(start..b);
                start = b;
            }
        }
        ranges.push(start..self.text.len());
        ranges
    }

    /// The line over byte range `r`: the `li`-th of the paragraph, `last` if it is the final one.
    fn place_line(&self, li: usize, r: Range<usize>, last: bool) -> ParaLine {
        let align = align_for(self.para, self.base_rtl, self.width, last);
        let avail = avail_for(self.para, self.base_rtl, self.width, li == 0);
        let (ce, tw) = self.trailing_ws(&r);
        let hang = self.hang_of(&r, ce);
        let content = self.range_w(r.start..ce);
        // justification
        let mut space_extra = 0.0;
        if align == Align::Justify
            && let Some(maxw) = avail
        {
            let spaces = self.text[r.start..ce].chars().filter(|c| *c == ' ').count();
            if spaces > 0 {
                space_extra = ((maxw - (content - hang.0 - hang.1)) / spaces as f32).max(0.0);
            }
        }
        let placed = self.place_glyphs(&r, space_extra, ce);
        let carets = self.carets(&r, &placed.extents, placed.pen);
        let (asc, desc, adv, first) = self.line_metrics(&r, &placed.glyphs);
        let content_w = if space_extra > 0.0 { avail.map_or(content, |a| a + hang.0 + hang.1) } else { content };
        ParaLine {
            range: self.base + r.start..self.base + r.end,
            glyphs: placed.glyphs,
            carets,
            content: content_w,
            left_trim: if self.base_rtl { tw } else { 0.0 },
            hang,
            ascent: asc,
            descent: desc,
            first,
            advance: adv,
            align,
            avail,
            margins: margins_for(self.para, self.base_rtl, li == 0),
            underline: placed.underline,
        }
    }

    fn place_glyphs(&self, r: &Range<usize>, space_extra: f32, ce: usize) -> Placed {
        let (levels, vruns) = self.bidi.visual_runs(&self.bidi.paragraphs[0], r.clone());
        let mut pen = 0.0f32;
        let mut glyphs = Vec::new();
        let mut extents: HashMap<usize, (f32, f32, bool)> = HashMap::new();
        let mut underline: Option<(f32, f32)> = None;
        for run in vruns {
            let rtl = levels[run.start].is_rtl();
            // Items are ordered and disjoint: binary-search both ends of the overlapping slice.
            let run_limit = self.items.partition_point(|item| item.range.start < run.end);
            let run_first = self.items[..run_limit].partition_point(|item| item.range.end <= run.start);
            let mut idx: Vec<usize> = (run_first..run_limit).collect();
            if rtl {
                idx.reverse();
            }
            for k in idx {
                for g in self.items[k].glyphs.iter().filter(|g| run.contains(&g.cluster)) {
                    let st = &self.runs[g.run].1;
                    let x0 = pen;
                    pen += g.pre;
                    glyphs.push(Glyph {
                        face: g.face,
                        id: g.id,
                        x: pen + g.dx,
                        y: -g.dy - g.rise,
                        size: g.size,
                        cluster: self.base + g.cluster,
                        synth_bold: self.primaries[g.run].synth_bold || st.faux_bold,
                        synth_italic: self.primaries[g.run].synth_italic || st.faux_italic,
                        h_scale: st.h_scale,
                        v_scale: st.v_scale,
                        run: g.run,
                        variations: self.variation_ids[g.run],
                    });
                    pen += g.adv;
                    if space_extra > 0.0 && self.text[g.cluster..].starts_with(' ') && g.cluster < ce {
                        pen += space_extra;
                    }
                    if st.underline && g.cluster < ce {
                        let u = underline.get_or_insert((x0, pen));
                        u.0 = u.0.min(x0);
                        u.1 = u.1.max(pen);
                    }
                    let e = extents.entry(g.cluster).or_insert((x0, pen, self.items[k].rtl));
                    e.0 = e.0.min(x0);
                    e.1 = e.1.max(pen);
                }
            }
        }
        Placed { glyphs, pen, extents, underline }
    }

    /// Caret stops at each char boundary of the line, and at its end.
    fn carets(&self, r: &Range<usize>, extents: &HashMap<usize, (f32, f32, bool)>, pen: f32) -> Vec<(usize, f32)> {
        let mut carets = Vec::new();
        let mut cluster_starts: Vec<usize> = extents.keys().copied().collect();
        cluster_starts.sort_unstable();
        let char_bytes: Vec<usize> = self.text[r.clone()].char_indices().map(|(b, _)| r.start + b).collect();
        let mut char_index = 0usize;
        for (ci, &cs) in cluster_starts.iter().enumerate() {
            let (x0, x1, rtl) = extents[&cs];
            let next = cluster_starts.get(ci + 1).copied().unwrap_or(r.end);
            while char_index < char_bytes.len() && char_bytes[char_index] < cs {
                char_index += 1;
            }
            let begin = char_index;
            while char_index < char_bytes.len() && char_bytes[char_index] < next {
                char_index += 1;
            }
            let count = (char_index - begin).max(1) as f32;
            for (mi, b) in char_bytes[begin..char_index].iter().enumerate() {
                let f = mi as f32 / count;
                let x = if rtl { x1 - (x1 - x0) * f } else { x0 + (x1 - x0) * f };
                carets.push((self.base + b, x));
            }
        }
        // end of line caret
        let end_x = match self.chars.iter().rev().find(|c| c.0 < r.end && c.0 >= r.start) {
            Some(c) => {
                let cs = cluster_starts.iter().rev().find(|s| **s <= c.0).copied();
                match cs.and_then(|s| extents.get(&s)) {
                    Some(&(x0, x1, rtl)) => {
                        if rtl {
                            x0
                        } else {
                            x1
                        }
                    }
                    None => pen,
                }
            }
            None => 0.0,
        };
        carets.push((self.base + r.end, end_x));
        carets.sort_by_key(|c| c.0);
        carets.dedup_by_key(|c| c.0);
        carets
    }

    /// Ascent, descent and advance of the line: the tallest face and the largest leading on it.
    fn line_metrics(&self, r: &Range<usize>, glyphs: &[Glyph]) -> (f32, f32, f32, f32) {
        let (mut asc, mut desc, mut adv, mut first) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
        let char_start = self.chars.partition_point(|c| c.0 < r.start);
        let char_end = self.chars.partition_point(|c| c.0 < r.end);
        let mut seen: Vec<usize> = self.chars[char_start..char_end].iter().map(|c| c.6).collect();
        seen.sort_unstable();
        seen.dedup();
        if seen.is_empty() {
            seen.push(self.chars.first().map_or(0, |c| c.6));
        }
        for &si in &seen {
            let m = style_metrics(self.primaries, self.runs, si);
            let rise = self.runs[si].1.rise();
            asc = asc.max(m.ascent + rise.max(0.0));
            desc = desc.max(m.descent - rise.min(0.0));
            adv = adv.max(self.runs[si].1.line_advance());
            first = first.max(style_first(self.primaries, self.runs, si));
        }
        for g in glyphs {
            if g.face != self.primaries[g.run].face {
                let m = fonts::face(g.face).metrics(g.size);
                asc = asc.max(m.ascent);
                desc = desc.max(m.descent);
            }
        }
        (asc, desc, adv, first)
    }
}

/// Each char with its shaped form, face, direction and run.
fn char_forms(text: &str, base: usize, runs: &[(Range<usize>, TextStyle)], primaries: &[Resolved], bidi: &BidiInfo) -> Vec<CharInfo> {
    text.char_indices()
        .map(|(b, c)| {
            let si = run_at(runs, base + b);
            let style = &runs[si].1;
            let primary = primaries[si].face;
            let face_of = |ch: char| if ch.is_whitespace() { primary } else { fonts::fallback_for(ch, primary) };
            let all_small = style.opentype.all_small_caps && style.caps != Caps::All;
            let small = (style.caps == Caps::Small || all_small) && c.is_lowercase();
            let small_cap = all_small && c.is_uppercase();
            let (sc, mut form) = match style.caps {
                Caps::All => (upper_single(c), Form::Normal),
                _ if small && feature_substitutes(face_of(c), b"smcp", c) => (c, Form::TrueSmall),
                _ if small_cap && feature_substitutes(face_of(c), b"c2sc", c) => (c, Form::TrueSmall),
                _ if small => (upper_single(c), Form::FauxSmall),
                _ if small_cap => (c, Form::FauxSmall),
                _ => (c, Form::Normal),
            };
            let face = face_of(sc);
            if form == Form::Normal && style.script != Script::Normal && !sc.is_whitespace() {
                let tag = if style.script == Script::Sub { b"subs" } else { b"sups" };
                if feature_substitutes(face, tag, sc) {
                    form = Form::TrueScript;
                }
            }
            (b, c, sc, face, bidi.levels[b].is_rtl(), form, si)
        })
        .collect()
}

/// Runs of chars with the same face, direction, form, run and CJK vertical flag, shaped as one.
fn build_items(text: &str, chars: &[CharInfo], runs: &[(Range<usize>, TextStyle)]) -> Vec<Item> {
    let key = |c: &CharInfo| (c.3, c.4, c.5, c.6, runs.get(c.6).is_some_and(|run| run.1.vertical) && crate::is_cjk(c.1));
    let mut items: Vec<Item> = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let k0 = key(&chars[i]);
        let mut j = i + 1;
        while j < chars.len() && key(&chars[j]) == k0 {
            j += 1;
        }
        let (face, rtl, form, si, _) = k0;
        let style = &runs[si].1;
        let sub: Vec<(usize, char, char)> = chars[i..j].iter().map(|c| (c.0, c.1, c.2)).collect();
        let size = match form {
            Form::FauxSmall => style.glyph_size() * SMALL_CAPS_SCALE,
            Form::TrueScript => style.size,
            _ => style.glyph_size(),
        };
        let mut glyphs = shape_item(&sub, rtl, face, size, style, si, form);
        space_item(&mut glyphs, style, text, true);
        let end = if j < chars.len() { chars[j].0 } else { text.len() };
        items.push(Item { range: chars[i].0..end, rtl, glyphs });
        i = j;
    }
    items
}

fn paragraph(
    text: &str,
    base: usize,
    runs: &[(Range<usize>, TextStyle)],
    primaries: &[Resolved],
    variation_ids: &[u32],
    para: &ParagraphStyle,
    width: Option<f32>,
) -> Vec<ParaLine> {
    if text.is_empty() {
        let si = run_at(runs, base);
        let pm = style_metrics(primaries, runs, si);
        // No text to resolve a direction from: the explicit one.
        let rtl = para.rtl == Some(true);
        return vec![ParaLine {
            range: base..base,
            glyphs: vec![],
            carets: vec![(base, 0.0)],
            content: 0.0,
            left_trim: 0.0,
            hang: (0.0, 0.0),
            ascent: pm.ascent,
            descent: pm.descent,
            advance: runs[si].1.line_advance(),
            first: style_first(primaries, runs, si),
            align: align_for(para, rtl, width, true),
            avail: avail_for(para, rtl, width, true),
            margins: margins_for(para, rtl, true),
            underline: None,
        }];
    }
    let p = Para::new(text, base, runs, primaries, variation_ids, para, width);
    let ranges = p.break_ranges();
    let nlines = ranges.len();
    ranges.into_iter().enumerate().map(|(li, r)| p.place_line(li, r, li + 1 == nlines)).collect()
}

pub fn layout_uncached(text: &str, style: &TextStyle, para: &ParagraphStyle) -> Layout {
    layout_rich_uncached(text, &[(0..text.len(), style.clone())], std::slice::from_ref(para))
}

/// Maximum input size processed by one layout request. Above this, return an empty layout
/// retaining the source length, so untrusted text cannot fan out into unbounded shaped data.
const MAX_LAYOUT_BYTES: usize = 1 << 20;

/// Lay out `text` with character style runs (byte ranges covering the text, in order; at least
/// one) and paragraph settings (one per CR/LF/CRLF-separated paragraph; the last repeats). The wrap
/// width is the first paragraph's.
pub fn layout_rich_uncached(text: &str, runs: &[(Range<usize>, TextStyle)], paras: &[ParagraphStyle]) -> Layout {
    if text.len() > MAX_LAYOUT_BYTES || runs.len() > MAX_LAYOUT_BYTES || paras.len() > MAX_LAYOUT_BYTES {
        return Layout { text_len: text.len(), ..Default::default() };
    }
    let fallback = [(0..text.len(), TextStyle::default())];
    let runs = if runs.is_empty() { &fallback[..] } else { runs };
    let default_para = ParagraphStyle::default();
    let para_at = |i: usize| paras.get(i).or(paras.last()).unwrap_or(&default_para);
    let width = para_at(0).width;
    let primaries: Vec<Resolved> = runs.iter().map(|(_, s)| fonts::resolve(&s.family, &s.style)).collect();
    // A named instance of a variable font (`Figtree` `Black`, or `Figtree-Black`) sets the base
    // axis values; axes set explicitly in the style keep theirs.
    let instanced = primaries.iter().any(|p| p.instance.is_some()).then(|| {
        runs.iter()
            .zip(&primaries)
            .map(|((r, s), p)| {
                let base = fonts::instance_variations(p);
                let mut s = s.clone();
                if !base.is_empty() {
                    s.variations = fonts::merge_variations(&base, &s.variations);
                }
                (r.clone(), s)
            })
            .collect::<Vec<_>>()
    });
    let runs = instanced.as_deref().unwrap_or(runs);
    // A location that cannot be interned would outline at the default; shape there too.
    let cleared = runs.iter().any(|(_, s)| !s.variations.is_empty() && crate::variable::intern(&s.variations) == 0).then(|| {
        runs.iter()
            .map(|(r, s)| {
                let mut s = s.clone();
                if crate::variable::intern(&s.variations) == 0 {
                    s.variations.clear();
                }
                (r.clone(), s)
            })
            .collect::<Vec<_>>()
    });
    let runs = cleared.as_deref().unwrap_or(runs);
    let variation_ids: Vec<u32> = runs.iter().map(|(_, s)| crate::variable::intern(&s.variations)).collect();
    let mut lay = Layout { missing_font: primaries.iter().any(|p| p.missing), text_len: text.len(), ..Default::default() };
    let mut plines: Vec<(usize, ParaLine)> = Vec::new();
    for (pi, span) in paragraph_ranges(text).enumerate() {
        let p = text.get(span.bytes.clone()).unwrap_or_default();
        plines.extend(paragraph(p, span.bytes.start, runs, &primaries, &variation_ids, para_at(pi), width).into_iter().map(|l| (pi, l)));
    }
    let mut baseline = 0.0f32;
    for (i, (pi, pl)) in plines.into_iter().enumerate() {
        let para = para_at(pi);
        if i == 0 {
            // Vertical type keeps its first column at the font ascent (not measured in AE).
            let vertical = runs.get(run_at(runs, pl.range.start)).is_some_and(|r| r.1.vertical)
                || runs.iter().any(|r| r.1.vertical && r.0.start < pl.range.end && r.0.end > pl.range.start);
            baseline = if width.is_some() { if vertical { pl.ascent } else { pl.first } } else { 0.0 };
        } else {
            let prev_para = lay.lines.last().map_or(pi, |l| l.para);
            let mut adv = pl.advance + para.leading;
            if prev_para != pi {
                adv += para_at(prev_para).space_after + para.space_before;
            }
            baseline += adv;
        }
        let shift = line_shift(&pl);
        let g0 = lay.glyphs.len();
        lay.glyphs.extend(pl.glyphs.into_iter().map(|mut g| {
            g.x += shift;
            g.y += baseline;
            g
        }));
        let x = shift + pl.left_trim;
        if let Some((u0, u1)) = pl.underline {
            let si = run_at(runs, pl.range.start);
            let pm = fonts::face(primaries[si].face).metrics(runs[si].1.size);
            lay.underlines.push([u0 + shift, baseline + pm.underline_pos, u1 + shift, baseline + pm.underline_pos + pm.underline_thickness]);
        }
        lay.lines.push(Line {
            range: pl.range,
            baseline,
            x,
            width: pl.content,
            ascent: pl.ascent,
            descent: pl.descent,
            glyphs: g0..lay.glyphs.len(),
            carets: pl.carets.into_iter().map(|(b, cx)| (b, cx + shift)).collect(),
            para: pi,
            para_end: false,
        });
    }
    for i in 0..lay.lines.len() {
        lay.lines[i].para_end = lay.lines.get(i + 1).is_none_or(|n| n.para != lay.lines[i].para);
    }
    set_bounds(&mut lay, width);
    lay
}

/// Horizontal shift of a line from its paragraph's alignment, margins and hanging punctuation.
fn line_shift(pl: &ParaLine) -> f32 {
    let shown = pl.content - pl.hang.0 - pl.hang.1;
    let (ml, mr) = pl.margins;
    let shift = match (pl.avail, pl.align) {
        (None, Align::Left | Align::Justify) => ml,
        (None, Align::Center) => -shown / 2.0 + (ml - mr) / 2.0,
        (None, Align::Right) => -shown - mr,
        (Some(_), Align::Left | Align::Justify) => ml,
        (Some(a), Align::Center) => ml + (a - shown) / 2.0,
        (Some(a), Align::Right) => ml + a - shown,
    };
    shift - pl.left_trim - pl.hang.0
}

/// The layout's bounds: the lines' horizontal extent (and the wrap width), and their vertical extent.
fn set_bounds(lay: &mut Layout, width: Option<f32>) {
    let (mut x0, mut x1) = (f32::MAX, f32::MIN);
    for l in &lay.lines {
        x0 = x0.min(l.x);
        x1 = x1.max(l.x + l.width);
    }
    if let Some(w) = width {
        x0 = x0.min(0.0);
        x1 = x1.max(w);
    }
    if let (Some(first), Some(last)) = (lay.lines.first(), lay.lines.last()) {
        lay.bounds = [x0, first.baseline - first.ascent, x1.max(x0), last.baseline + last.descent];
    }
}

fn cache() -> &'static Mutex<HashMap<u64, (u64, Arc<Layout>)>> {
    static C: OnceLock<Mutex<HashMap<u64, (u64, Arc<Layout>)>>> = OnceLock::new();
    C.get_or_init(Default::default)
}

/// Lay out `text` in one style (cached by text and styles).
pub fn layout(text: &str, style: &TextStyle, para: &ParagraphStyle) -> Arc<Layout> {
    layout_rich(text, &[(0..text.len(), style.clone())], std::slice::from_ref(para))
}

/// [`layout_rich_uncached`], cached by text, runs and paragraph settings.
pub fn layout_rich(text: &str, runs: &[(Range<usize>, TextStyle)], paras: &[ParagraphStyle]) -> Arc<Layout> {
    if text.len() > MAX_LAYOUT_BYTES || runs.len() > MAX_LAYOUT_BYTES || paras.len() > MAX_LAYOUT_BYTES {
        return Arc::new(layout_rich_uncached(text, runs, paras));
    }
    let mut h = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut h);
    runs.hash(&mut h);
    paras.hash(&mut h);
    let key = h.finish();
    static CLOCK: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let now = CLOCK.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    if let Some(e) = cache().lock().unwrap_or_else(std::sync::PoisonError::into_inner).get_mut(&key) {
        e.0 = now;
        return e.1.clone();
    }
    let l = Arc::new(layout_rich_uncached(text, runs, paras));
    let mut c = cache().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if c.len() >= 512 {
        // evict the oldest quarter
        let mut ages: Vec<u64> = c.values().map(|v| v.0).collect();
        ages.sort_unstable();
        let cut = ages[ages.len() / 4];
        c.retain(|_, v| v.0 > cut);
    }
    c.insert(key, (now, l.clone()));
    l
}

/// Width of a single line of `text` (no wrapping).
pub fn measure(text: &str, style: &TextStyle) -> f32 {
    let l = layout(text, style, &ParagraphStyle::default());
    l.lines.iter().map(|l| l.width).fold(0.0, f32::max)
}

#[cfg(test)]
mod tests;

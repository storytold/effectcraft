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

use crate::fonts::{self, FaceId, Resolved};

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
fn paragraph(
    text: &str,
    base: usize,
    runs: &[(Range<usize>, TextStyle)],
    primaries: &[Resolved],
    variation_ids: &[u32],
    para: &ParagraphStyle,
    width: Option<f32>,
) -> Vec<ParaLine> {
    let rtl_para = para.rtl == Some(true);
    let margins_for = |first: bool| -> (f32, f32) {
        let fi = if first { para.indent_first } else { 0.0 };
        if rtl_para { (para.indent_end, para.indent_start + fi) } else { (para.indent_start + fi, para.indent_end) }
    };
    let avail_for = |first: bool| -> Option<f32> {
        let (l, r) = margins_for(first);
        width.map(|w| (w - l - r).max(1.0))
    };
    let style_metrics = |si: usize| {
        let st = &runs[si].1;
        fonts::face(primaries[si].face).metrics(st.glyph_size())
    };
    let eff_align = |last: bool| -> Align {
        if (last || width.is_none()) && para.align == Align::Justify {
            if width.is_none() && para.justify_last == Align::Justify { Align::Left } else { para.justify_last }
        } else {
            para.align
        }
    };
    if text.is_empty() {
        let si = run_at(runs, base);
        let pm = style_metrics(si);
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
            align: eff_align(true),
            avail: avail_for(true),
            margins: margins_for(true),
            underline: None,
        }];
    }
    let default_level = para.rtl.map(|r| if r { Level::rtl() } else { Level::ltr() });
    let bidi = BidiInfo::new(text, default_level);
    let pinfo = &bidi.paragraphs[0];
    let base_rtl = pinfo.level.is_rtl();
    // per char: (byte, char, shaped char, face, rtl, form, run)
    let chars: Vec<(usize, char, char, FaceId, bool, Form, usize)> = text
        .char_indices()
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
        .collect();
    // items: runs of equal (face, rtl, small, style)
    let mut items: Vec<Item> = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let key =
            |c: &(usize, char, char, FaceId, bool, Form, usize)| (c.3, c.4, c.5, c.6, runs.get(c.6).is_some_and(|run| run.1.vertical) && crate::is_cjk(c.1));
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
    // advance per char byte (cluster start)
    let mut char_adv = vec![0.0f32; text.len() + 1];
    for it in &items {
        for g in &it.glyphs {
            char_adv[g.cluster.min(text.len())] += g.adv + g.pre;
        }
    }
    // Prefix widths and the preceding non-whitespace boundary make candidate line widths O(1).
    // Every-line composition evaluates O(n²) candidate breaks; summing each candidate's bytes
    // or rescanning its trailing whitespace would otherwise add another factor of text length.
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
    let range_w = |r: Range<usize>| -> f32 { (prefix_adv[r.end] - prefix_adv[r.start]) as f32 };
    let trailing_ws = |r: &Range<usize>| -> (usize, f32) {
        let end = previous_non_ws[r.end].max(r.start).min(r.end);
        (end, (prefix_adv[r.end] - prefix_adv[end]) as f32)
    };
    // Hanging punctuation widths (left, right) of a line's content.
    let hang_of = |r: &Range<usize>, ce: usize| -> (f32, f32) {
        if !para.hanging || width.is_none() || r.start >= ce {
            return (0.0, 0.0);
        }
        let first = text[r.start..ce].chars().next().filter(|c| is_hanging_open(*c)).map_or(0.0, |_| char_adv[r.start]);
        let last = text[r.start..ce].char_indices().next_back().filter(|(_, c)| is_hanging_close(*c)).map_or(0.0, |(b, _)| char_adv[r.start + b]);
        (first, last)
    };
    let fits_w = |r: &Range<usize>| -> f32 {
        let (ce, _) = trailing_ws(r);
        let (hl, hr) = hang_of(r, ce);
        range_w(r.start..ce) - hl - hr
    };
    // line breaking
    let mut ranges: Vec<Range<usize>> = Vec::new();
    match width {
        None => ranges.push(0..text.len()),
        Some(_) => {
            let opps: Vec<usize> = unicode_linebreak::linebreaks(text).map(|(p, _)| p).collect();
            let avail = |line: usize| avail_for(line == 0).unwrap_or(f32::MAX);
            if para.every_line && opps.len() > 1 && opps.len() <= 1500 {
                // Minimise the summed squared slack of every line but the last.
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
                        let w = fits_w(&r);
                        let maxw = avail(usize::from(i != 0));
                        let over = w > maxw;
                        if over && i + 1 != j {
                            break;
                        }
                        let slack = maxw - w;
                        let cost = if over {
                            1e12 + w
                        } else if j == m - 1 {
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
                let mut j = m - 1;
                while j > 0 {
                    let i = best[j].1;
                    ranges.push(nodes[i]..nodes[j]);
                    j = i;
                }
                ranges.reverse();
            } else {
                let mut start = 0usize;
                let mut last_ok: Option<usize> = None;
                for p in opps {
                    loop {
                        let w = fits_w(&(start..p));
                        if w <= avail(ranges.len()) || last_ok.is_none() {
                            last_ok = Some(p);
                            break;
                        }
                        let Some(b) = last_ok.take() else { break };
                        ranges.push(start..b);
                        start = b;
                    }
                }
                ranges.push(start..text.len());
            }
            ranges.retain(|r| !r.is_empty());
            if ranges.is_empty() {
                ranges.push(0..text.len());
            }
        }
    }
    let nlines = ranges.len();
    let mut out = Vec::new();
    for (li, r) in ranges.into_iter().enumerate() {
        let last = li + 1 == nlines;
        let align = eff_align(last);
        let avail = avail_for(li == 0);
        let (ce, tw) = trailing_ws(&r);
        let hang = hang_of(&r, ce);
        let content = range_w(r.start..ce);
        // justification
        let mut space_extra = 0.0;
        if align == Align::Justify
            && let Some(maxw) = avail
        {
            let spaces = text[r.start..ce].chars().filter(|c| *c == ' ').count();
            if spaces > 0 {
                space_extra = ((maxw - (content - hang.0 - hang.1)) / spaces as f32).max(0.0);
            }
        }
        let (levels, vruns) = bidi.visual_runs(pinfo, r.clone());
        let mut pen = 0.0f32;
        let mut glyphs = Vec::new();
        let mut extents: HashMap<usize, (f32, f32, bool)> = HashMap::new();
        let mut underline: Option<(f32, f32)> = None;
        for run in vruns {
            let rtl = levels[run.start].is_rtl();
            // Items are ordered and disjoint: binary-search both ends of the overlapping slice.
            let run_limit = items.partition_point(|item| item.range.start < run.end);
            let run_first = items[..run_limit].partition_point(|item| item.range.end <= run.start);
            let mut idx: Vec<usize> = (run_first..run_limit).collect();
            if rtl {
                idx.reverse();
            }
            for k in idx {
                for g in items[k].glyphs.iter().filter(|g| run.contains(&g.cluster)) {
                    let st = &runs[g.run].1;
                    let x0 = pen;
                    pen += g.pre;
                    glyphs.push(Glyph {
                        face: g.face,
                        id: g.id,
                        x: pen + g.dx,
                        y: -g.dy - g.rise,
                        size: g.size,
                        cluster: base + g.cluster,
                        synth_bold: primaries[g.run].synth_bold || st.faux_bold,
                        synth_italic: primaries[g.run].synth_italic || st.faux_italic,
                        h_scale: st.h_scale,
                        v_scale: st.v_scale,
                        run: g.run,
                        variations: variation_ids[g.run],
                    });
                    pen += g.adv;
                    if space_extra > 0.0 && text[g.cluster..].starts_with(' ') && g.cluster < ce {
                        pen += space_extra;
                    }
                    if st.underline && g.cluster < ce {
                        let u = underline.get_or_insert((x0, pen));
                        u.0 = u.0.min(x0);
                        u.1 = u.1.max(pen);
                    }
                    let e = extents.entry(g.cluster).or_insert((x0, pen, items[k].rtl));
                    e.0 = e.0.min(x0);
                    e.1 = e.1.max(pen);
                }
            }
        }
        // Caret stops for each char boundary in the line.
        let mut carets = Vec::new();
        let mut cluster_starts: Vec<usize> = extents.keys().copied().collect();
        cluster_starts.sort_unstable();
        let char_bytes: Vec<usize> = text[r.clone()].char_indices().map(|(b, _)| r.start + b).collect();
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
                carets.push((base + b, x));
            }
        }
        // end of line caret
        let end_x = match chars.iter().rev().find(|c| c.0 < r.end && c.0 >= r.start) {
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
        carets.push((base + r.end, end_x));
        carets.sort_by_key(|c| c.0);
        carets.dedup_by_key(|c| c.0);
        // Line metrics: the tallest face and the largest leading on the line.
        let (mut asc, mut desc, mut adv) = (0.0f32, 0.0f32, 0.0f32);
        let char_start = chars.partition_point(|c| c.0 < r.start);
        let char_end = chars.partition_point(|c| c.0 < r.end);
        let mut seen: Vec<usize> = chars[char_start..char_end].iter().map(|c| c.6).collect();
        seen.sort_unstable();
        seen.dedup();
        if seen.is_empty() {
            seen.push(chars.first().map_or(0, |c| c.6));
        }
        for &si in &seen {
            let m = style_metrics(si);
            let rise = runs[si].1.rise();
            asc = asc.max(m.ascent + rise.max(0.0));
            desc = desc.max(m.descent - rise.min(0.0));
            adv = adv.max(runs[si].1.line_advance());
        }
        for g in &glyphs {
            if g.face != primaries[g.run].face {
                let m = fonts::face(g.face).metrics(g.size);
                asc = asc.max(m.ascent);
                desc = desc.max(m.descent);
            }
        }
        let content_w = if space_extra > 0.0 { avail.map_or(content, |a| a + hang.0 + hang.1) } else { content };
        out.push(ParaLine {
            range: base + r.start..base + r.end,
            glyphs,
            carets,
            content: content_w,
            left_trim: if base_rtl { tw } else { 0.0 },
            hang,
            ascent: asc,
            descent: desc,
            advance: adv,
            align,
            avail,
            margins: margins_for(li == 0),
            underline,
        });
    }
    out
}

/// Lay out `text` (paragraphs separated by CR, LF or CRLF) in one style.
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
    let instanced: Vec<(Range<usize>, TextStyle)>;
    let runs = if primaries.iter().any(|p| p.instance.is_some()) {
        instanced = runs
            .iter()
            .zip(&primaries)
            .map(|((r, s), p)| {
                let base = fonts::instance_variations(p);
                let mut s = s.clone();
                if !base.is_empty() {
                    s.variations = fonts::merge_variations(&base, &s.variations);
                }
                (r.clone(), s)
            })
            .collect();
        &instanced[..]
    } else {
        runs
    };
    // A location that cannot be interned would outline at the default; shape there too.
    let cleared: Vec<(Range<usize>, TextStyle)>;
    let runs = if runs.iter().any(|(_, s)| !s.variations.is_empty() && crate::variable::intern(&s.variations) == 0) {
        cleared = runs
            .iter()
            .map(|(r, s)| {
                let mut s = s.clone();
                if crate::variable::intern(&s.variations) == 0 {
                    s.variations.clear();
                }
                (r.clone(), s)
            })
            .collect();
        &cleared[..]
    } else {
        runs
    };
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
            baseline = if width.is_some() { pl.ascent } else { 0.0 };
        } else {
            let prev_para = lay.lines.last().map_or(pi, |l| l.para);
            let mut adv = pl.advance + para.leading;
            if prev_para != pi {
                adv += para_at(prev_para).space_after + para.space_before;
            }
            baseline += adv;
        }
        let shown = pl.content - pl.hang.0 - pl.hang.1;
        let (ml, mr) = pl.margins;
        let shift = match (pl.avail, pl.align) {
            (None, Align::Left | Align::Justify) => ml,
            (None, Align::Center) => -shown / 2.0 + (ml - mr) / 2.0,
            (None, Align::Right) => -shown - mr,
            (Some(_), Align::Left | Align::Justify) => ml,
            (Some(a), Align::Center) => ml + (a - shown) / 2.0,
            (Some(a), Align::Right) => ml + a - shown,
        } - pl.left_trim
            - pl.hang.0;
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
    lay
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
mod tests {
    use super::*;

    fn st(size: f32) -> TextStyle {
        TextStyle { size, ..Default::default() }
    }

    fn ids(text: &str, s: &TextStyle) -> Vec<u32> {
        layout_uncached(text, s, &ParagraphStyle::default()).glyphs.iter().map(|g| g.id).collect()
    }

    fn ot(f: impl FnOnce(&mut OpenType)) -> TextStyle {
        let mut s = st(100.0);
        f(&mut s.opentype);
        s
    }

    fn serif() -> TextStyle {
        TextStyle { family: "Noto Serif".into(), ..st(100.0) }
    }

    #[test]
    fn bundled_faces_list_their_features() {
        let inter = fonts::face(fonts::resolve("Inter", "Regular").face).features();
        for t in ["ss01", "dlig", "frac", "tnum", "sups", "ordn"] {
            assert!(inter.iter().any(|f| f == t), "{t} in {inter:?}");
        }
        let serif = fonts::face(fonts::resolve("Noto Serif", "Regular").face).features();
        for t in ["smcp", "c2sc", "onum", "lnum"] {
            assert!(serif.iter().any(|f| f == t), "{t} in {serif:?}");
        }
    }

    #[test]
    fn opentype_features_substitute_glyphs() {
        let plain = st(100.0);
        // Stylistic sets, fractions, ordinals, figure widths and discretionary ligatures.
        assert_ne!(ids("1234567890", &plain), ids("1234567890", &ot(|o| o.set_stylistic_set(1, true))), "ss01 alternate digits");
        assert_eq!(ids("1234567890", &plain), ids("1234567890", &ot(|o| o.set_stylistic_set(20, true))), "Inter has no ss20");
        assert_ne!(ids("1/2", &plain), ids("1/2", &ot(|o| o.fractions = true)), "frac");
        assert_ne!(ids("1a 2o No", &plain), ids("1a 2o No", &ot(|o| o.ordinals = true)), "ordn");
        let tab = measure("1111", &ot(|o| o.figure_width = effectcraft_keyframe::FigureWidth::Tabular));
        let prop = measure("1111", &ot(|o| o.figure_width = effectcraft_keyframe::FigureWidth::Proportional));
        assert!((tab - prop).abs() > 1.0, "tabular {tab} vs proportional {prop}");
        // Old-style figures (Noto Serif).
        let old = TextStyle { opentype: OpenType { figure_style: effectcraft_keyframe::FigureStyle::OldStyle, ..Default::default() }, ..serif() };
        assert_ne!(ids("123", &serif()), ids("123", &old), "onum");
        // Contextual alternates are on by default; turning them off can only keep or change glyphs.
        let no_calt = ot(|o| o.contextual_alternates = false);
        assert_eq!(ids("Hello", &plain).len(), ids("Hello", &no_calt).len());
    }

    #[test]
    fn true_small_caps_and_superscripts_with_faux_fallback() {
        // Noto Serif has smcp: lowercase letters take their own small-capital glyphs at full size.
        let sc = TextStyle { caps: Caps::Small, ..serif() };
        let l = layout_uncached("Ab", &sc, &ParagraphStyle::default());
        let upper_b = ids("B", &serif())[0];
        assert_ne!(l.glyphs[1].id, upper_b, "true small cap glyph, not a scaled capital");
        assert_eq!(l.glyphs[1].size, 100.0);
        // Inter has no smcp: a capital scaled down.
        let faux = layout_uncached("Ab", &TextStyle { caps: Caps::Small, ..st(100.0) }, &ParagraphStyle::default());
        assert_eq!(faux.glyphs[1].id, ids("B", &st(100.0))[0]);
        assert!((faux.glyphs[1].size - 78.0).abs() < 0.01);
        // All Small Caps: capitals become small capitals too (c2sc).
        let all = TextStyle { opentype: OpenType { all_small_caps: true, ..Default::default() }, ..serif() };
        let a = layout_uncached("AB", &all, &ParagraphStyle::default());
        assert_ne!(a.glyphs[0].id, ids("A", &serif())[0]);
        // Superscript: Inter's sups figures at full size on the baseline; letters it lacks are
        // synthesized (smaller and raised).
        let sup = TextStyle { script: Script::Super, ..st(100.0) };
        let s2 = layout_uncached("2", &sup, &ParagraphStyle::default());
        assert_ne!(s2.glyphs[0].id, ids("2", &st(100.0))[0]);
        assert_eq!(s2.glyphs[0].size, 100.0);
        assert_eq!(s2.glyphs[0].y, 0.0);
        let sq = layout_uncached("q", &sup, &ParagraphStyle::default());
        if !feature_substitutes(sq.glyphs[0].face, b"sups", 'q') {
            assert!(sq.glyphs[0].size < 60.0 && sq.glyphs[0].y < -20.0, "{:?}", sq.glyphs[0]);
        }
        let sub = layout_uncached("2", &TextStyle { script: Script::Sub, ..st(100.0) }, &ParagraphStyle::default());
        assert_ne!(sub.glyphs[0].id, s2.glyphs[0].id);
    }

    #[test]
    fn kerning_and_ligatures_change_advances() {
        let kern = measure("AVAVAV", &st(100.0));
        let nokern = measure("AVAVAV", &TextStyle { kerning: false, ..st(100.0) });
        assert!(kern < nokern - 5.0, "kerning tightens AV: {kern} vs {nokern}");
        // tracking adds 1/1000 em per cluster
        let tracked = measure("AVAVAV", &TextStyle { tracking: 100.0, ..st(100.0) });
        assert!((tracked - kern - 6.0 * 10.0).abs() < 0.5, "{tracked} {kern}");
    }

    #[test]
    fn point_text_alignment() {
        let l = layout("Hello", &st(50.0), &ParagraphStyle { align: Align::Center, ..Default::default() });
        let line = &l.lines[0];
        assert!((line.x + line.width / 2.0).abs() < 0.01);
        let r = layout("Hello", &st(50.0), &ParagraphStyle { align: Align::Right, ..Default::default() });
        assert!((r.lines[0].x + r.lines[0].width).abs() < 0.01);
        assert_eq!(l.glyphs.len(), 5);
        assert!(l.bounds[1] < -30.0 && l.bounds[3] > 5.0, "{:?}", l.bounds);
    }

    #[test]
    fn wraps_area_text_and_justifies() {
        let p = ParagraphStyle { width: Some(300.0), align: Align::Justify, ..Default::default() };
        let l = layout("the quick brown fox jumps over the lazy dog again and again", &st(40.0), &p);
        assert!(l.lines.len() >= 3, "{}", l.lines.len());
        for line in &l.lines[..l.lines.len() - 1] {
            assert!((line.width - 300.0).abs() < 0.5, "justified to the box: {}", line.width);
        }
        // the lines tile the text
        assert_eq!(l.lines[0].range.start, 0);
        for w in l.lines.windows(2) {
            assert_eq!(w[0].range.end, w[1].range.start);
            assert!(w[1].baseline > w[0].baseline);
        }
        let left = layout("the quick brown fox jumps over the lazy dog", &st(40.0), &ParagraphStyle { width: Some(300.0), ..Default::default() });
        assert!(left.lines.iter().all(|l| l.width <= 300.0 + 0.01));
    }

    #[test]
    fn newlines_make_paragraphs_and_leading_adds() {
        let l = layout("one\ntwo\n\nfour", &st(30.0), &ParagraphStyle::default());
        assert_eq!(l.lines.len(), 4);
        assert_eq!(l.lines[1].range, 4..7);
        assert_eq!(l.lines[2].range, 8..8);
        let gap = l.lines[1].baseline - l.lines[0].baseline;
        let l2 = layout("one\ntwo", &st(30.0), &ParagraphStyle { leading: 10.0, ..Default::default() });
        assert!((l2.lines[1].baseline - l2.lines[0].baseline - gap - 10.0).abs() < 1e-3);
    }

    #[test]
    fn carets_and_hit_testing() {
        let l = layout("abc\nde", &st(40.0), &ParagraphStyle::default());
        let (x0, _, li0) = l.caret(0);
        let (x1, _, _) = l.caret(1);
        let (x3, _, _) = l.caret(3);
        assert!(x0.abs() < 1e-3 && x1 > x0 && x3 > x1);
        assert_eq!(li0, 0);
        let (_, b4, li4) = l.caret(4);
        assert_eq!(li4, 1);
        assert!(b4 > 0.0);
        assert_eq!(l.hit(x1 + 1.0, 0.0), 1);
        assert_eq!(l.hit(1000.0, b4), 6);
        let rects = l.selection_rects(1, 5);
        assert_eq!(rects.len(), 2);
    }

    #[test]
    fn bidi_reorders_rtl_runs() {
        // Hebrew letters are laid out right-to-left: the first logical letter is rightmost.
        let l = layout("ab \u{5d0}\u{5d1}\u{5d2} cd", &st(40.0), &ParagraphStyle::default());
        let x_of = |byte: usize| l.glyphs.iter().find(|g| g.cluster == byte).map(|g| g.x).unwrap();
        let alef = "ab ".len();
        let gimel = alef + 4;
        assert!(x_of(alef) > x_of(gimel), "alef right of gimel");
        assert!(x_of(0) < x_of(gimel) && x_of(alef) < x_of("ab \u{5d0}\u{5d1}\u{5d2} ".len()));
    }

    #[test]
    fn caps_and_small_caps() {
        let s = st(50.0);
        let up = measure("HELLO", &s);
        let all = measure("hello", &TextStyle { caps: Caps::All, ..s.clone() });
        assert!((up - all).abs() < 0.01);
        let small = measure("hello", &TextStyle { caps: Caps::Small, ..s.clone() });
        assert!(small < up * 0.9 && small > up * 0.6, "{small} {up}");
        let l = layout("Hi", &TextStyle { baseline_shift: 10.0, underline: true, ..s }, &ParagraphStyle::default());
        assert!((l.glyphs[0].y + 10.0).abs() < 1e-3);
        assert_eq!(l.underlines.len(), 1);
    }

    #[test]
    fn ligature_caret_interpolates() {
        // Inter has an "fi"-like ligature only via calt in some versions; use "ffi" which most
        // fonts ligate. Whatever shaping does, every char boundary has a caret.
        let l = layout("office", &st(40.0), &ParagraphStyle::default());
        let stops: Vec<usize> = l.lines[0].carets.iter().map(|c| c.0).collect();
        assert_eq!(stops, vec![0, 1, 2, 3, 4, 5, 6]);
        let xs: Vec<f32> = l.lines[0].carets.iter().map(|c| c.1).collect();
        assert!(xs.windows(2).all(|w| w[1] >= w[0]), "{xs:?}");
    }

    #[test]
    fn fallback_font_for_missing_glyphs() {
        // Inter lacks Devanagari/Hebrew; Noto Serif lacks Hebrew too → glyphs still produced (notdef)
        let l = layout("A\u{3b1}", &TextStyle { family: "Noto Serif".into(), ..st(40.0) }, &ParagraphStyle::default());
        assert_eq!(l.glyphs.len(), 2);
        assert!(l.glyphs.iter().all(|g| g.id != 0));
    }

    #[test]
    fn empty_text_has_a_caret() {
        let l = layout("", &st(40.0), &ParagraphStyle::default());
        assert_eq!(l.lines.len(), 1);
        assert_eq!(l.caret(0).0, 0.0);
        assert!(l.bounds[3] > l.bounds[1]);
    }

    fn rich(text: &str, runs: &[(Range<usize>, TextStyle)], para: ParagraphStyle) -> Layout {
        layout_rich_uncached(text, runs, &[para])
    }

    #[test]
    fn mixed_sizes_share_a_baseline_and_take_the_largest_leading() {
        let text = "small BIG\nnext";
        let runs = [(0..6, st(20.0)), (6..9, st(80.0)), (9..text.len(), st(20.0))];
        let l = rich(text, &runs, ParagraphStyle::default());
        assert_eq!(l.lines.len(), 2);
        let y0 = l.glyphs[0].y;
        assert!(l.glyphs[..9].iter().all(|g| (g.y - y0).abs() < 1e-3), "one baseline per line");
        assert!(l.glyphs[7].size > l.glyphs[0].size * 3.0);
        // Line 2's advance is its own leading (20 px text: 24 px); line 1 is as tall as the big run.
        assert!((l.lines[1].baseline - l.lines[0].baseline - 24.0).abs() < 1e-3, "{}", l.lines[1].baseline);
        assert!(l.lines[0].ascent > 60.0);
        // A big run on the second line pushes it down by its 120% leading.
        let text2 = "aa\nBB";
        let l2 = rich(text2, &[(0..3, st(20.0)), (3..5, st(80.0))], ParagraphStyle::default());
        assert!((l2.lines[1].baseline - 96.0).abs() < 1e-3);
        // Explicit leading wins over auto.
        let mut fixed = st(20.0);
        fixed.leading = Some(50.0);
        let l3 = rich("a\nb", &[(0..3, fixed)], ParagraphStyle::default());
        assert!((l3.lines[1].baseline - 50.0).abs() < 1e-3);
    }

    #[test]
    fn super_subscript_tsume_and_kerning_metrics() {
        let base = st(100.0);
        let sup = TextStyle { script: Script::Super, ..base.clone() };
        let sub = TextStyle { script: Script::Sub, ..base.clone() };
        // A letter the font has no superior / inferior glyph for: synthesized.
        let inter = fonts::resolve("Inter", "Regular").face;
        let c = "qwkzvjxyQWK".chars().find(|&c| !feature_substitutes(inter, b"sups", c) && !feature_substitutes(inter, b"subs", c)).unwrap();
        let text: String = ['x', c, 'y', c].iter().collect();
        let l = rich(&text, &[(0..1, base.clone()), (1..2, sup), (2..3, base.clone()), (3..4, sub)], ParagraphStyle::default());
        let g = &l.glyphs;
        assert!((g[1].size - 58.3).abs() < 0.1);
        assert!((g[1].y - (g[0].y - 33.3)).abs() < 0.1, "superscript raised by a third of the size");
        assert!((g[3].y - (g[2].y + 33.3)).abs() < 0.1, "subscript lowered");
        // Tsume removes side bearings; full tsume is narrower than none.
        let w0 = measure("onion", &base);
        let w1 = measure("onion", &TextStyle { tsume: 1.0, ..base.clone() });
        assert!(w1 < w0 - 5.0, "{w1} vs {w0}");
        // Manual kerning: +100/1000 em before each of the 5 later characters of AVAVAV, metrics off.
        let nokern = measure("AVAVAV", &TextStyle { kerning: false, ..base.clone() });
        let manual = measure("AVAVAV", &TextStyle { manual_kern: Some(100.0), ..base.clone() });
        assert!((manual - nokern - 50.0).abs() < 0.5, "{manual} {nokern}");
        let optical = measure("AVAVAV", &TextStyle { optical: true, ..base.clone() });
        assert!(optical > 0.0 && (optical - nokern).abs() < 100.0);
        // Horizontal scale stretches advances.
        let wide = measure("onion", &TextStyle { h_scale: 2.0, ..base.clone() });
        assert!((wide - 2.0 * w0).abs() < 0.5);
        let l = layout("o", &TextStyle { h_scale: 2.0, v_scale: 0.5, ..base }, &ParagraphStyle::default());
        assert_eq!((l.glyphs[0].h_scale, l.glyphs[0].v_scale), (2.0, 0.5));
    }

    #[test]
    fn indents_and_paragraph_spacing() {
        let s = st(20.0);
        let text = "first paragraph here\nsecond one";
        let p0 = ParagraphStyle { width: Some(400.0), indent_start: 30.0, indent_first: 15.0, space_after: 10.0, ..Default::default() };
        let p1 = ParagraphStyle { width: Some(400.0), indent_end: 50.0, align: Align::Right, space_before: 7.0, ..Default::default() };
        let l = layout_rich_uncached(text, &[(0..text.len(), s.clone())], &[p0.clone(), p1]);
        assert!((l.lines[0].x - 45.0).abs() < 1e-3, "left + first-line indent: {}", l.lines[0].x);
        assert!((l.lines[1].x + l.lines[1].width - 350.0).abs() < 0.5, "right indent: {}", l.lines[1].x + l.lines[1].width);
        assert!((l.lines[1].baseline - l.lines[0].baseline - (24.0 + 10.0 + 7.0)).abs() < 1e-3);
        assert_eq!((l.lines[0].para, l.lines[1].para), (0, 1));
        assert!(l.lines[0].para_end && l.lines[1].para_end);
        // Wrapped lines after the first don't get the first-line indent.
        let long = "word ".repeat(30);
        let l = layout_rich_uncached(&long, &[(0..long.len(), s.clone())], &[p0]);
        assert!(l.lines.len() > 2);
        assert!((l.lines[1].x - 30.0).abs() < 1e-3);
        assert!(l.lines.iter().all(|ln| ln.x + ln.width <= 400.0 + 0.5));
        assert!(!l.lines[0].para_end);
        // Point text: a right-aligned paragraph's end indent moves it left of the origin.
        let l = layout_rich_uncached("abc", &[(0..3, s)], &[ParagraphStyle { align: Align::Right, indent_end: 12.0, ..Default::default() }]);
        assert!((l.lines[0].x + l.lines[0].width + 12.0).abs() < 1e-3);
    }

    #[test]
    fn justify_last_line_modes_and_every_line_composer() {
        let text = "the quick brown fox jumps over the lazy dog and keeps running far";
        let s = st(30.0);
        let p = |last: Align| ParagraphStyle { width: Some(320.0), align: Align::Justify, justify_last: last, ..Default::default() };
        let left = layout_rich_uncached(text, &[(0..text.len(), s.clone())], &[p(Align::Left)]);
        let n = left.lines.len();
        assert!(n >= 3);
        assert!((left.lines[0].width - 320.0).abs() < 0.5);
        assert!(left.lines[n - 1].width < 320.0 && left.lines[n - 1].x.abs() < 1e-3);
        let center = layout_rich_uncached(text, &[(0..text.len(), s.clone())], &[p(Align::Center)]);
        let lc = &center.lines[n - 1];
        assert!((lc.x - (320.0 - lc.width) / 2.0).abs() < 0.5);
        let right = layout_rich_uncached(text, &[(0..text.len(), s.clone())], &[p(Align::Right)]);
        let lr = &right.lines[n - 1];
        assert!((lr.x + lr.width - 320.0).abs() < 0.5);
        let all = layout_rich_uncached(text, &[(0..text.len(), s.clone())], &[p(Align::Justify)]);
        assert!((all.lines[n - 1].width - 320.0).abs() < 0.5, "justify all stretches the last line");
        // Every-line composer: never worse (summed squared slack) than greedy breaking.
        let slack = |l: &Layout| -> f32 { l.lines[..l.lines.len() - 1].iter().map(|ln| (320.0 - ln.width).powi(2)).sum() };
        let greedy = layout_rich_uncached(text, &[(0..text.len(), s.clone())], &[ParagraphStyle { width: Some(320.0), ..Default::default() }]);
        let every = layout_rich_uncached(text, &[(0..text.len(), s)], &[ParagraphStyle { width: Some(320.0), every_line: true, ..Default::default() }]);
        assert!(slack(&every) <= slack(&greedy) + 1e-3, "{} vs {}", slack(&every), slack(&greedy));
        assert!(every.lines.iter().all(|ln| ln.width <= 320.0 + 0.5));
    }

    #[test]
    fn hanging_punctuation_overhangs_the_box() {
        let s = st(40.0);
        let text = "\"Hi.\"";
        let p = ParagraphStyle { width: Some(400.0), hanging: true, ..Default::default() };
        let l = layout_rich_uncached(text, &[(0..text.len(), s.clone())], &[p]);
        assert!(l.lines[0].x < -1.0, "opening quote hangs left: {}", l.lines[0].x);
        let r = layout_rich_uncached(
            text,
            &[(0..text.len(), s)],
            &[ParagraphStyle { width: Some(400.0), hanging: true, align: Align::Right, ..Default::default() }],
        );
        assert!(r.lines[0].x + r.lines[0].width > 401.0, "closing quote hangs right");
    }

    #[test]
    fn oversized_layout_requests_are_bounded_without_caching() {
        let text = "a".repeat(MAX_LAYOUT_BYTES + 1);
        let layout = layout(&text, &TextStyle::default(), &ParagraphStyle::default());
        assert_eq!(layout.text_len, text.len());
        assert!(layout.lines.is_empty() && layout.glyphs.is_empty());
        assert!(layout_rich_uncached("ok", &[], &[]).lines.len() == 1);
    }

    #[test]
    fn rtl_paragraph_carets_run_right_to_left() {
        let text = "\u{5d0}\u{5d1}\u{5d2}";
        let l = layout_rich_uncached(text, &[(0..text.len(), st(40.0))], &[ParagraphStyle { rtl: Some(true), ..Default::default() }]);
        let xs: Vec<f32> = l.lines[0].carets.iter().map(|c| c.1).collect();
        assert_eq!(xs.len(), 4);
        assert!(xs.windows(2).all(|w| w[1] < w[0]), "{xs:?}");
        // Hit testing returns the logical position nearest the point.
        assert_eq!(l.hit(xs[1] + 0.5, 0.0), 2);
        assert_eq!(l.hit(xs[0] + 50.0, 0.0), 0);
    }

    /// #1563: text set in a named instance (`Black`) is laid out at the instance's axis values,
    /// as if they had been set as variations; axes set on the text itself keep their values.
    #[test]
    fn named_instance_sets_the_base_variations() {
        const FAMILY: &str = "EffectCraft Layout Instance Test";
        fonts::add_font_data(crate::sfnt::test_font::variable(FAMILY, &[("Light", 300.0, None), ("Black", 900.0, None)], true));
        let st = |style: &str, variations: Vec<(String, f32)>| TextStyle {
            family: FAMILY.into(),
            style: style.into(),
            size: 50.0,
            variations,
            ..Default::default()
        };
        let coords = |l: &Layout| l.glyphs.iter().map(|g| crate::variable::coords(g.variations).to_vec()).collect::<Vec<_>>();
        let black = layout_uncached("Hi", &st("Black", vec![]), &ParagraphStyle::default());
        assert!(!black.glyphs.is_empty());
        assert!(coords(&black).iter().all(|c| c == &[("wght".to_string(), 900.0)]), "{:?}", coords(&black));
        let set = layout_uncached("Hi", &st("Black", vec![("wght".into(), 500.0)]), &ParagraphStyle::default());
        assert!(coords(&set).iter().all(|c| c == &[("wght".to_string(), 500.0)]), "{:?}", coords(&set));
        // A static family keeps no variations.
        let inter = layout_uncached("Hi", &TextStyle::default(), &ParagraphStyle::default());
        assert!(inter.glyphs.iter().all(|g| g.variations == 0));
    }
}

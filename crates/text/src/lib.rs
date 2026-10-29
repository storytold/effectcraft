//! EffectCraft text engine.
//!
//! - [`fonts`], [`sfnt`], [`layout`]: font database, shaping (harfrust), bidi, line breaking and
//!   paragraph layout (shared design with FilmCraft's text engine).
//! - This module turns a layer's [`TextDoc`] into per-character glyph outlines (Bezier paths) with
//!   character / word / line indices, which is what text animators and selectors work on.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

#[cfg(all(unix, not(target_os = "macos"), not(target_arch = "wasm32")))]
mod font_dirs;
pub mod fonts;
pub mod layout;
pub mod path_text;
pub mod selectors;
pub mod sfnt;
pub mod variable;

pub use kurbo;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use effectcraft_keyframe::{BaselineOption, CharStyle, Composer, Direction, Justify, Kerning, ParaStyle, TextDoc};
pub use fonts::{FaceId, Resolved, families, resolve};
use kurbo::{Affine, BezPath, Point};
pub use layout::{Align, Caps, Glyph, Layout, Line, ParagraphStyle, Script, TextStyle, layout as layout_text, layout_rich, measure};
use selectors::BasedOn;
use skrifa::instance::{LocationRef, Size};
use skrifa::outline::{DrawSettings, OutlinePen};
use skrifa::{GlyphId, MetadataProvider};

pub(crate) struct Pen(pub(crate) BezPath);

impl OutlinePen for Pen {
    fn move_to(&mut self, x: f32, y: f32) {
        self.0.move_to((x as f64, y as f64));
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.0.line_to((x as f64, y as f64));
    }
    fn quad_to(&mut self, cx0: f32, cy0: f32, x: f32, y: f32) {
        self.0.quad_to((cx0 as f64, cy0 as f64), (x as f64, y as f64));
    }
    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        self.0.curve_to((cx0 as f64, cy0 as f64), (cx1 as f64, cy1 as f64), (x as f64, y as f64));
    }
    fn close(&mut self) {
        self.0.close_path();
    }
}

/// Glyph outline in font units (y up), cached.
pub(crate) fn outline_units(face: FaceId, gid: u32) -> Option<Arc<BezPath>> {
    static C: OnceLock<Mutex<HashMap<(FaceId, u32), Option<Arc<BezPath>>>>> = OnceLock::new();
    let c = C.get_or_init(Default::default);
    if let Some(v) = c.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get(&(face, gid)) {
        return v.clone();
    }
    let f = fonts::face(face);
    let v = f.font().and_then(|font| {
        let g = font.outline_glyphs().get(GlyphId::new(gid))?;
        let mut pen = Pen(BezPath::new());
        g.draw(DrawSettings::unhinted(Size::unscaled(), LocationRef::default()), &mut pen).ok()?;
        Some(Arc::new(pen.0))
    });
    let mut m = c.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if m.len() > 50_000 {
        m.clear();
    }
    m.insert((face, gid), v.clone());
    v
}

/// The outline of a laid-out glyph in pixels, origin on its baseline (y down), with its
/// horizontal / vertical scale and faux italic.
pub fn glyph_outline(g: &Glyph) -> BezPath {
    let units = if g.variations == 0 { outline_units(g.face, g.id) } else { variable::outline_units_at(g.face, g.id, &variable::coords(g.variations)) };
    let Some(units) = units else { return BezPath::new() };
    glyph_affine(g) * (*units).clone()
}

/// Font units → the glyph's outline in pixels (size, horizontal / vertical scale, faux italic).
pub fn glyph_affine(g: &Glyph) -> Affine {
    let k = g.size as f64 / fonts::face(g.face).units_per_em() as f64;
    let slant = if g.synth_italic { 0.21 } else { 0.0 };
    let (hs, vs) = (g.h_scale as f64, g.v_scale as f64);
    Affine::new([k * hs, 0.0, slant * k * vs * hs, -k * vs, 0.0, 0.0])
}

/// One character of laid-out text.
#[derive(Clone, Debug)]
pub struct CharGlyph {
    /// Outline relative to `origin`.
    pub path: BezPath,
    /// Baseline origin in layer space.
    pub origin: Point,
    pub advance: f64,
    /// Indices for selectors (characters, characters excluding spaces, words, lines).
    pub char_index: usize,
    pub char_index_no_space: usize,
    pub word_index: usize,
    pub line_index: usize,
    pub is_space: bool,
    /// The source character (after All Caps).
    pub ch: char,
    pub synth_bold: bool,
    /// The glyph's drawn size (superscript / small caps are smaller than the run's size).
    pub size: f64,
    /// Index into [`TextLayout::styles`].
    pub run: usize,
    /// Face and glyph id, and font units → `path` (for variable-font redraws).
    pub face: FaceId,
    pub gid: u32,
    pub outline_xf: Affine,
    /// The character's variable font axis values (its style's Variable Font Axes).
    pub variations: Vec<(String, f32)>,
}

#[derive(Clone, Debug, Default)]
pub struct TextLayout {
    pub glyphs: Vec<CharGlyph>,
    pub chars: usize,
    pub chars_no_space: usize,
    pub words: usize,
    pub lines: usize,
    /// Bounds in layer space (x0, y0, x1, y1).
    pub bounds: [f64; 4],
    /// Line origins (layer space) `[x, baseline, width, height]`.
    pub line_boxes: Vec<[f64; 4]>,
    /// The character style of each run (glyphs refer to them by index).
    pub styles: Vec<CharStyle>,
    /// The paragraph layout (layout space) and its map into layer space.
    pub layout: Arc<Layout>,
    pub to_layer: Affine,
    /// Byte offset of every character, plus the text length.
    pub byte_of_char: Vec<usize>,
    /// Whether each character is white space (for the selectors' Based On).
    pub space_of_char: Vec<bool>,
    pub vertical: bool,
}

/// A character style → [`TextStyle`].
pub fn text_style(s: &CharStyle) -> TextStyle {
    TextStyle {
        family: s.font.clone(),
        style: s.style.clone(),
        size: s.size as f32,
        tracking: s.tracking as f32,
        kerning: true,
        optical: s.kerning == Kerning::Optical,
        manual_kern: if let Kerning::Manual(v) = s.kerning { Some(v as f32) } else { None },
        ligatures: s.ligatures,
        baseline_shift: s.baseline_shift as f32,
        faux_bold: s.faux_bold,
        faux_italic: s.faux_italic,
        caps: if s.all_caps {
            Caps::All
        } else if s.small_caps {
            Caps::Small
        } else {
            Caps::Normal
        },
        underline: false,
        h_scale: (s.h_scale / 100.0) as f32,
        v_scale: (s.v_scale / 100.0) as f32,
        tsume: (s.tsume / 100.0) as f32,
        script: match s.baseline {
            BaselineOption::Normal => Script::Normal,
            BaselineOption::Superscript => Script::Super,
            BaselineOption::Subscript => Script::Sub,
        },
        leading: s.leading.map(|l| l as f32),
        opentype: s.opentype,
        variations: s.variations.clone(),
        vertical: false,
    }
}

/// Paragraph settings → [`ParagraphStyle`] (wrap `width` for paragraph text).
pub fn paragraph_style(p: &ParaStyle, width: Option<f32>) -> ParagraphStyle {
    let (align, justify_last) = match p.justify {
        Justify::Left => (Align::Left, Align::Left),
        Justify::Center => (Align::Center, Align::Center),
        Justify::Right => (Align::Right, Align::Right),
        Justify::JustifyLastLeft => (Align::Justify, Align::Left),
        Justify::JustifyLastCenter => (Align::Justify, Align::Center),
        Justify::JustifyLastRight => (Align::Justify, Align::Right),
        Justify::JustifyAll => (Align::Justify, Align::Justify),
    };
    ParagraphStyle {
        align,
        justify_last,
        leading: 0.0,
        width,
        // The chosen direction is the paragraph's base direction (a left-to-right paragraph
        // starting with Arabic or Hebrew still runs left to right, as in After Effects).
        rtl: Some(p.direction == Direction::Rtl),
        indent_start: p.indent_left as f32,
        indent_end: p.indent_right as f32,
        indent_first: p.indent_first as f32,
        space_before: p.space_before as f32,
        space_after: p.space_after as f32,
        every_line: p.composer == Composer::EveryLine,
        hanging: p.hanging_punctuation,
    }
}

/// Characters set upright in vertical type: CJK ideographs, kana, hangul, full-width forms and
/// CJK punctuation. Everything else (Roman) turns on its side unless asked to stay upright.
pub fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x1100..=0x11FF | 0x2E80..=0x303F | 0x3040..=0x30FF | 0x3100..=0x31FF | 0x3200..=0x4DBF | 0x4E00..=0x9FFF
        | 0xA960..=0xA97F | 0xAC00..=0xD7FF | 0xF900..=0xFAFF | 0xFE10..=0xFE1F | 0xFE30..=0xFE4F | 0xFF00..=0xFFEF
        | 0x20000..=0x3FFFF)
}

/// Vertical type: the horizontal layout turned a quarter clockwise (lines become columns
/// running right to left), characters kept upright.
const ROT90: Affine = Affine::new([0.0, 1.0, -1.0, 0.0, 0.0, 0.0]);

/// Lay out a Source Text value in layer space. Point text: the origin is the start of the first
/// baseline (left), its centre (centre) or end (right). Paragraph text fills `box_size` from
/// `box_pos`. Vertical text runs down from the origin (point) or the box's top-right corner.
pub fn layout_doc(doc: &TextDoc) -> TextLayout {
    let text = doc.text.as_str();
    let byte_of_char: Vec<usize> = text.char_indices().map(|(b, _)| b).chain(std::iter::once(text.len())).collect();
    let nchars = byte_of_char.len() - 1;
    let mut runs = Vec::new();
    let mut styles = Vec::new();
    let mut ci = 0;
    for r in doc.runs() {
        let a = byte_of_char[ci.min(nchars)];
        ci += r.len;
        let b = byte_of_char[ci.min(nchars)];
        let mut runtime_style = text_style(&r.style);
        runtime_style.vertical = doc.vertical && !r.style.tate_chu_yoko;
        runs.push((a..b, runtime_style));
        styles.push(r.style);
    }
    let width = doc.box_size.map(|b| if doc.vertical { b[1] } else { b[0] } as f32);
    let paras: Vec<ParagraphStyle> = doc.paras().iter().map(|p| paragraph_style(p, width)).collect();
    let lay = layout::layout_rich(text, &runs, &paras);
    let to_layer = match (doc.vertical, doc.box_size) {
        (false, Some(_)) => Affine::translate((doc.box_pos[0], doc.box_pos[1])),
        (false, None) => Affine::IDENTITY,
        (true, Some(b)) => Affine::translate((doc.box_pos[0] + b[0], doc.box_pos[1])) * ROT90,
        (true, None) => ROT90,
    };
    let mut out = TextLayout { lines: lay.lines.len(), to_layer, vertical: doc.vertical, ..Default::default() };
    // Map byte clusters → char/word indices.
    let mut char_of_byte = vec![0usize; text.len() + 1];
    let mut word_of_char = Vec::new();
    let mut nospace_of_char = Vec::new();
    let mut words = 0usize;
    let mut in_word = false;
    let mut nospace = 0usize;
    for (ci, (b, ch)) in text.char_indices().enumerate() {
        for k in b..b + ch.len_utf8() {
            char_of_byte[k] = ci;
        }
        let space = ch.is_whitespace();
        if !space && !in_word {
            words += 1;
        }
        in_word = !space;
        word_of_char.push(words.saturating_sub(1));
        out.space_of_char.push(space);
        nospace_of_char.push(nospace);
        if !space {
            nospace += 1;
        }
    }
    char_of_byte[text.len()] = nchars;
    out.chars = nchars;
    out.chars_no_space = nospace;
    out.words = words;
    let mut b = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
    for (li, line) in lay.lines.iter().enumerate() {
        let o = to_layer * Point::new(line.x as f64, line.baseline as f64);
        out.line_boxes.push([o.x, o.y, line.width as f64, (line.ascent + line.descent) as f64]);
        // Vertical type: Tate-Chu-Yoko groups (consecutive glyphs of tcy runs) take one em of
        // the column; everything after them moves up by the rest of their width.
        let tcy = |gi: usize| doc.vertical && styles.get(lay.glyphs[gi].run).is_some_and(|s| s.tate_chu_yoko);
        let mut shift = 0.0f64;
        let mut group: Option<(f64, f64, f64)> = None; // (start x, width, em) in layout space
        let line_glyphs: Vec<usize> = line.glyphs.clone().collect();
        for (k, gi) in line_glyphs.iter().copied().enumerate() {
            let g = &lay.glyphs[gi];
            let ci = char_of_byte.get(g.cluster).copied().unwrap_or(0);
            let src = text[g.cluster..].chars().next().unwrap_or(' ');
            let st = styles.get(g.run);
            let ch = if st.is_some_and(|s| s.all_caps) { src.to_uppercase().next().unwrap_or(src) } else { src };
            let mut path = glyph_outline(g);
            let mut outline_xf = glyph_affine(g);
            let next_x = lay.glyphs.get(gi + 1).filter(|_| line.glyphs.contains(&(gi + 1))).map(|n| n.x).unwrap_or(line.x + line.width);
            let advance = (next_x - g.x) as f64;
            let mut origin = to_layer * Point::new(g.x as f64 - shift, g.y as f64);
            if doc.vertical && tcy(gi) {
                // Start of a group: measure it.
                if group.is_none() {
                    let mut w = 0.0;
                    for &gj in line_glyphs[k..].iter().take_while(|&&gj| tcy(gj)) {
                        let nx = lay.glyphs.get(gj + 1).filter(|_| line.glyphs.contains(&(gj + 1))).map(|n| n.x).unwrap_or(line.x + line.width);
                        w += (nx - lay.glyphs[gj].x) as f64;
                    }
                    group = Some((g.x as f64, w, g.size as f64));
                }
                let (x0, w, em) = group.unwrap_or((g.x as f64, advance, g.size as f64));
                // Upright and horizontal, centred across the column in an em-high cell.
                let centre = to_layer * Point::new(x0 - shift + em / 2.0, g.y as f64 - 0.35 * em);
                origin = Point::new(centre.x - w / 2.0 + (g.x as f64 - x0), centre.y + 0.35 * em);
                if line_glyphs.get(k + 1).is_none_or(|&n| !tcy(n)) {
                    shift += w - em;
                    group = None;
                }
            } else if doc.vertical {
                let upright = is_cjk(src) || st.is_some_and(|s| s.vertical_roman_upright);
                if upright {
                    // Anchor vertical forms using their own vertical origin and horizontal
                    // advance, not the next glyph's pen (which includes tracking/justification).
                    let font = fonts::face(g.face);
                    let metrics = is_cjk(src).then(|| font.vertical_origin(g.id, g.size)).flatten();
                    let t = if let Some((width, top)) = metrics {
                        Affine::translate((0.35 * g.size as f64 - width * g.h_scale as f64 / 2.0, top * g.v_scale as f64))
                    } else {
                        let c = Point::new(advance / 2.0, -0.35 * g.size as f64);
                        let rc = ROT90 * c;
                        Affine::translate((rc.x - c.x, rc.y - c.y))
                    };
                    path = t * path;
                    outline_xf = t * outline_xf;
                } else {
                    // Roman characters turn on their side with the column.
                    path = ROT90 * path;
                    outline_xf = ROT90 * outline_xf;
                }
            }
            if let Some(r) = (!path.elements().is_empty()).then(|| kurbo::Shape::bounding_box(&path)) {
                b[0] = b[0].min(origin.x + r.x0);
                b[1] = b[1].min(origin.y + r.y0);
                b[2] = b[2].max(origin.x + r.x1);
                b[3] = b[3].max(origin.y + r.y1);
            }
            out.glyphs.push(CharGlyph {
                path,
                origin,
                advance,
                char_index: ci,
                char_index_no_space: nospace_of_char.get(ci).copied().unwrap_or(0),
                word_index: word_of_char.get(ci).copied().unwrap_or(0),
                line_index: li,
                is_space: src.is_whitespace(),
                ch,
                synth_bold: g.synth_bold,
                size: g.size as f64,
                run: g.run,
                face: g.face,
                gid: g.id,
                outline_xf,
                variations: variable::coords(g.variations).to_vec(),
            });
        }
    }
    if b[0].is_finite() {
        out.bounds = b;
    }
    out.styles = styles;
    out.byte_of_char = byte_of_char;
    out.layout = lay;
    out
}

impl TextLayout {
    /// Character index of byte offset `b`.
    pub fn char_of_byte(&self, b: usize) -> usize {
        self.byte_of_char.partition_point(|x| *x < b).min(self.chars)
    }
    fn byte(&self, ci: usize) -> usize {
        self.byte_of_char.get(ci.min(self.chars)).copied().unwrap_or(0)
    }
    /// Layout line holding caret `ci`.
    pub fn line_of_char(&self, ci: usize) -> usize {
        self.layout.line_of(self.byte(ci))
    }
    /// The caret at character boundary `ci` as a segment (top, bottom) in layer space.
    pub fn caret(&self, ci: usize) -> (Point, Point) {
        let (x, base, li) = self.layout.caret(self.byte(ci));
        let (asc, desc) = self.layout.lines.get(li).map_or((10.0, 3.0), |l| (l.ascent, l.descent));
        (self.to_layer * Point::new(x as f64, (base - asc) as f64), self.to_layer * Point::new(x as f64, (base + desc) as f64))
    }
    /// The caret's position along its line (layout space), for up / down movement.
    pub fn caret_x(&self, ci: usize) -> f64 {
        self.layout.caret(self.byte(ci)).0 as f64
    }
    /// The character boundary nearest a layer-space point.
    pub fn hit(&self, p: Point) -> usize {
        let q = self.to_layer.inverse() * p;
        self.char_of_byte(self.layout.hit(q.x as f32, q.y as f32))
    }
    /// The caret one line up (`dir` < 0) or down from `ci`, keeping the line position `goal`.
    /// Past the first / last line it goes to the start / end of the text.
    pub fn move_vertical(&self, ci: usize, dir: i32, goal: Option<f64>) -> (usize, f64) {
        let goal = goal.unwrap_or_else(|| self.caret_x(ci));
        let li = self.line_of_char(ci) as i64 + dir as i64;
        if li < 0 {
            return (0, goal);
        }
        match self.layout.lines.get(li as usize) {
            Some(l) => (self.char_of_byte(self.layout.hit_line(l, goal as f32)), goal),
            None => (self.chars, goal),
        }
    }
    /// Start and end carets of the visual line holding `ci`.
    pub fn line_span(&self, ci: usize) -> (usize, usize) {
        let li = self.line_of_char(ci);
        match self.layout.lines.get(li) {
            Some(l) => (self.char_of_byte(l.range.start), self.char_of_byte(self.layout.line_end(l))),
            None => (0, self.chars),
        }
    }
    /// The number of selector units for Based On `based`.
    pub fn units(&self, based: BasedOn) -> usize {
        match based {
            BasedOn::Characters => self.chars,
            BasedOn::CharactersExcludingSpaces => self.chars_no_space,
            BasedOn::Words => self.words,
            BasedOn::Lines => self.lines,
        }
    }
    /// Characters `a..b` of every selector unit, in order; words and Characters Excluding Spaces
    /// leave the spaces out.
    fn unit_spans(&self, based: BasedOn) -> Vec<(usize, usize)> {
        let sp = |ci: usize| self.space_of_char.get(ci).copied().unwrap_or(false);
        match based {
            BasedOn::Characters => (0..self.chars).map(|ci| (ci, ci + 1)).collect(),
            BasedOn::CharactersExcludingSpaces => (0..self.chars).filter(|&ci| !sp(ci)).map(|ci| (ci, ci + 1)).collect(),
            BasedOn::Words => {
                let (mut out, mut start) = (vec![], None);
                for ci in 0..self.chars {
                    match (sp(ci), start) {
                        (false, None) => start = Some(ci),
                        (true, Some(a)) => {
                            out.push((a, ci));
                            start = None;
                        }
                        _ => {}
                    }
                }
                out.extend(start.map(|a| (a, self.chars)));
                out
            }
            BasedOn::Lines => self.layout.lines.iter().map(|l| (self.char_of_byte(l.range.start), self.char_of_byte(self.layout.line_end(l)))).collect(),
        }
    }
    /// The carets at the leading and trailing edges of the unit made of characters `a..b`: before
    /// its first character, and after its last one when that is on the same line, else the line's
    /// end. Carets rather than glyphs, so the edges also fall inside ligatures and follow
    /// right-to-left runs.
    fn unit_edges(&self, (a, b): (usize, usize)) -> Option<(usize, usize)> {
        let li = self.line_of_char(a);
        let trail = if self.line_of_char(b) == li { b } else { self.char_of_byte(self.layout.line_end(self.layout.lines.get(li)?)) };
        Some((a, trail))
    }
    /// The caret at `ci` as (top, baseline, bottom) in layer space.
    fn caret_points(&self, ci: usize) -> [Point; 3] {
        let (top, bottom) = self.caret(ci);
        let (x, base, _) = self.layout.caret(self.byte(ci));
        [top, self.to_layer * Point::new(x as f64, base as f64), bottom]
    }
    /// A Range Selector bar at fraction `f` of the units (#1011): a caret-like segment as (top,
    /// baseline, bottom) in layer space, between unit boundaries in proportion. None for empty or
    /// vertical text.
    pub fn range_bar(&self, based: BasedOn, f: f64) -> Option<[Point; 3]> {
        let n = self.units(based);
        if self.vertical || n == 0 || !f.is_finite() {
            return None;
        }
        let x = f.clamp(0.0, 1.0) * n as f64;
        let u = (x.floor() as usize).min(n - 1);
        let t = x - u as f64;
        let (a, z) = self.unit_edges(*self.unit_spans(based).get(u)?)?;
        let (lead, trail) = (self.caret_points(a), self.caret_points(z));
        Some([0, 1, 2].map(|k| lead[k].lerp(trail[k], t)))
    }
    /// The fraction of the units under a layer-space point, the inverse of [`Self::range_bar`]:
    /// on the line whose baseline is nearest, the nearest place between unit edges.
    pub fn range_fraction_at(&self, based: BasedOn, p: Point) -> Option<f64> {
        let n = self.units(based);
        if self.vertical || n == 0 {
            return None;
        }
        let q = self.to_layer.inverse() * p;
        let line = self.layout.lines.iter().enumerate().min_by(|a, b| (a.1.baseline as f64 - q.y).abs().total_cmp(&(b.1.baseline as f64 - q.y).abs()))?.0;
        let spans = self.unit_spans(based);
        let nearest = |on_line: bool| {
            let mut best: Option<(f64, f64)> = None;
            for (u, span) in spans.iter().enumerate() {
                if on_line && self.line_of_char(span.0) != line {
                    continue;
                }
                let Some((a, z)) = self.unit_edges(*span) else { continue };
                let (x0, x1) = (self.caret(a).1.x, self.caret(z).1.x);
                let t = if (x1 - x0).abs() > 1e-9 { ((p.x - x0) / (x1 - x0)).clamp(0.0, 1.0) } else { 0.0 };
                let d = (x0 + (x1 - x0) * t - p.x).abs();
                if best.is_none_or(|b| d < b.0) {
                    best = Some((d, (u as f64 + t) / n as f64));
                }
            }
            best.map(|b| b.1)
        };
        // (a line without units, such as an empty one, takes the nearest unit anywhere)
        nearest(true).or_else(|| nearest(false))
    }
    /// Selection highlight quads (layer space) for characters `a..b`.
    pub fn selection_quads(&self, a: usize, b: usize) -> Vec<[Point; 4]> {
        self.layout
            .selection_rects(self.byte(a.min(b)), self.byte(a.max(b)))
            .into_iter()
            .map(|r| {
                let m = |x: f32, y: f32| self.to_layer * Point::new(x as f64, y as f64);
                [m(r[0], r[1]), m(r[2], r[1]), m(r[2], r[3]), m(r[0], r[3])]
            })
            .collect()
    }
}

/// Outline (origin on the baseline) and advance of a single character in a character style:
/// used by Character Offset / Character Value substitutions.
pub fn char_glyph_style(s: &CharStyle, ch: char) -> (BezPath, f64) {
    let style = text_style(s);
    let lay = layout_text(&ch.to_string(), &style, &ParagraphStyle::default());
    let Some(g) = lay.glyphs.first() else { return (BezPath::new(), 0.0) };
    let adv = lay.lines.first().map(|l| l.width as f64).unwrap_or(0.0);
    (Affine::translate((-(g.x as f64), 0.0)) * glyph_outline(g), adv)
}

/// [`char_glyph_style`] in a Source Text's base style.
pub fn char_glyph(doc: &TextDoc, ch: char) -> (BezPath, f64) {
    char_glyph_style(&doc.base_style(), ch)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #1063 (item 8): After Effects scripts use CR for Source Text line breaks.
    #[test]
    fn carriage_returns_create_lines_without_rewriting_source_text() {
        let lf = TextDoc::plain("Line 1\nLine 2");
        let cr = TextDoc::plain("Line 1\rLine 2");
        let expected = layout_doc(&lf);
        let actual = layout_doc(&cr);
        assert_eq!(actual.lines, 2);
        assert_eq!(actual.bounds, expected.bounds);
        assert_eq!(actual.line_boxes, expected.line_boxes);
        assert_eq!(cr.text, "Line 1\rLine 2");
        assert_eq!(actual.chars, cr.char_len());
    }

    #[test]
    fn line_break_variants_match_in_point_box_and_vertical_text() {
        for vertical in [false, true] {
            for boxed in [false, true] {
                let template = TextDoc { size: 30.0, vertical, box_size: boxed.then_some([120.0, 120.0]), ..Default::default() };
                let expected = layout_doc(&TextDoc { text: "one two three\n\nshort\n".into(), ..template.clone() });
                for separator in ["\r", "\r\n", "\n"] {
                    let text = format!("one two three{separator}{separator}short{separator}");
                    let doc = TextDoc { text: text.clone(), ..template.clone() };
                    let actual = layout_doc(&doc);
                    assert_eq!(actual.lines, expected.lines, "{separator:?}, boxed={boxed}, vertical={vertical}");
                    assert_eq!(actual.bounds, expected.bounds);
                    assert_eq!(actual.line_boxes, expected.line_boxes);
                    assert_eq!(doc.text, text);
                    assert_eq!(actual.chars, doc.char_len());
                }
            }
        }
    }

    #[test]
    fn crlf_preserves_unicode_style_indices_and_paragraph_formatting() {
        for separator in ["\r", "\n", "\r\n"] {
            let text = format!("é{separator}Beta");
            let mut doc = TextDoc::plain(&text);
            let start = 1 + separator.chars().count();
            doc.apply_style(start..doc.char_len(), |s| {
                s.size = 80.0;
                s.fill = [0.0, 1.0, 0.0, 1.0];
            });
            doc.apply_para(start..start + 1, |p| p.indent_left = 30.0);
            let layout = layout_doc(&doc);
            assert_eq!(layout.lines, 2);
            let glyph = layout.glyphs.iter().find(|g| g.ch == 'B').unwrap();
            assert_eq!(glyph.char_index, start);
            assert_eq!(glyph.line_index, 1);
            assert_eq!(glyph.size, 80.0);
            assert_eq!(layout.styles[glyph.run].fill, [0.0, 1.0, 0.0, 1.0]);
            assert_eq!(layout.line_span(start), (start, doc.char_len()));
            assert_eq!(layout.char_of_byte(doc.byte_of(start)), start);
            assert_eq!(doc.paras().iter().map(|p| p.indent_left).collect::<Vec<_>>(), vec![0.0, 30.0]);
            assert_eq!(doc.text, text);
        }
    }

    #[test]
    fn layout_counts_and_bounds() {
        let doc = TextDoc { text: "Hello big world".into(), size: 100.0, ..Default::default() };
        let l = layout_doc(&doc);
        assert_eq!(l.chars, 15);
        assert_eq!(l.words, 3);
        assert_eq!(l.chars_no_space, 13);
        assert!(l.bounds[2] > 500.0, "{:?}", l.bounds);
        // Baseline at y = 0: caps rise above it.
        assert!(l.bounds[1] < -60.0 && l.bounds[3] < 30.0, "{:?}", l.bounds);
        assert!(l.glyphs.iter().filter(|g| !g.is_space).all(|g| !g.path.elements().is_empty()));
    }

    #[test]
    fn styled_runs_carets_and_hit_testing() {
        let mut doc = TextDoc { text: "Hello big\nworld".into(), size: 40.0, ..Default::default() };
        doc.apply_style(6..9, |s| {
            s.size = 80.0;
            s.fill = [1.0, 0.0, 0.0, 1.0];
        });
        let l = layout_doc(&doc);
        let g = l.glyphs.iter().find(|g| g.char_index == 7).unwrap();
        assert_eq!(l.styles[g.run].size, 80.0);
        assert_eq!(l.styles[g.run].fill, [1.0, 0.0, 0.0, 1.0]);
        assert_eq!(g.size, 80.0);
        let small = l.glyphs.iter().find(|g| g.char_index == 1).unwrap();
        assert_eq!(small.origin.y, g.origin.y, "one baseline for mixed sizes");
        // Line 2 sits one 40 px auto leading (48 px) below line 1.
        let w = l.glyphs.iter().find(|g| g.char_index == 10).unwrap();
        assert!((w.origin.y - g.origin.y - 48.0).abs() < 1e-6);
        // Every caret hit-tests back to itself; carets advance along each line.
        for ci in 0..=doc.char_len() {
            let (a, b) = l.caret(ci);
            assert!(b.y > a.y);
            assert_eq!(l.hit(a.midpoint(b)), ci, "caret {ci}");
        }
        assert!(l.caret(8).0.x > l.caret(7).0.x);
        // Up / down keep the line position; line spans and selections.
        let (down, goal) = l.move_vertical(2, 1, None);
        assert_eq!(l.line_of_char(down), 1);
        assert!((goal - l.caret_x(2)).abs() < 1e-6);
        assert_eq!(l.move_vertical(down, -1, Some(goal)).0, 2);
        assert_eq!(l.move_vertical(2, -1, None).0, 0);
        assert_eq!(l.move_vertical(12, 1, None).0, 15);
        assert_eq!(l.line_span(3), (0, 9));
        assert_eq!(l.line_span(12), (10, 15));
        let q = l.selection_quads(3, 12);
        assert_eq!(q.len(), 2);
        assert_eq!(l.char_of_byte(doc.byte_of(12)), 12);
    }

    #[test]
    fn paragraph_box_offset_and_vertical_type() {
        let doc = TextDoc { text: "ab cd".into(), size: 20.0, box_size: Some([200.0, 100.0]), box_pos: [-100.0, -50.0], ..Default::default() };
        let l = layout_doc(&doc);
        let (a, _) = l.caret(0);
        // The first baseline sits the `d` height below the box top (After Effects), so the
        // caret, which spans the font's full ascent, starts a little above the box.
        assert!((a.x + 100.0).abs() < 1e-6 && a.y + 50.0 < 0.0 && a.y + 50.0 > -0.35 * 20.0, "{a:?}");
        // Vertical: characters run down a column, carets are horizontal.
        let v = layout_doc(&TextDoc { text: "abc\nde".into(), size: 20.0, vertical: true, ..Default::default() });
        let ys: Vec<f64> = v.glyphs.iter().filter(|g| g.line_index == 0).map(|g| g.origin.y).collect();
        assert!(ys.windows(2).all(|w| w[1] > w[0]), "{ys:?}");
        let col2 = v.glyphs.iter().find(|g| g.line_index == 1).unwrap();
        assert!(col2.origin.x < v.glyphs[0].origin.x - 10.0, "next column to the left");
        let (p0, p1) = v.caret(1);
        assert!((p0.y - p1.y).abs() < 1e-6 && (p0.x - p1.x).abs() > 5.0);
        assert_eq!(v.hit(p0.midpoint(p1)), 1);
    }

    #[test]
    fn vertical_roman_rotates_and_tate_chu_yoko_sets_upright_groups() {
        let wide = |g: &CharGlyph| {
            let r = kurbo::Shape::bounding_box(&g.path);
            (r.width(), r.height())
        };
        // "H" sideways in a vertical column: wider than tall (it is taller than wide upright).
        let doc = TextDoc { text: "HI".into(), size: 40.0, vertical: true, ..Default::default() };
        let v = layout_doc(&doc);
        let (w, h) = wide(&v.glyphs[0]);
        assert!(w > h, "rotated: {w}×{h}");
        // Standard Vertical Roman Alignment keeps it upright.
        let mut up = doc.clone();
        up.apply_style_all(|s| s.vertical_roman_upright = true);
        let (w, h) = wide(&layout_doc(&up).glyphs[0]);
        assert!(h > w, "upright: {w}×{h}");
        // CJK stays upright.
        let cjk = layout_doc(&TextDoc { text: "日本".into(), size: 40.0, vertical: true, ..Default::default() });
        assert!(cjk.glyphs.iter().all(|g| g.origin.x.is_finite()));
        // Tate-Chu-Yoko on "12" in "A12B": the digits sit side by side on one row of the column
        // (same y, increasing x) and the group takes one em of the column.
        let mut t = TextDoc { text: "A12B".into(), size: 40.0, vertical: true, ..Default::default() };
        let plain = layout_doc(&t);
        t.apply_style(1..3, |s| s.tate_chu_yoko = true);
        let tcy = layout_doc(&t);
        let (one, two) = (&tcy.glyphs[1], &tcy.glyphs[2]);
        assert!((one.origin.y - two.origin.y).abs() < 1e-6, "{:?} {:?}", one.origin, two.origin);
        assert!(two.origin.x > one.origin.x + 5.0);
        let (w, h) = wide(one);
        assert!(h > w, "digits upright");
        let w = one.advance + two.advance;
        let moved = tcy.glyphs[3].origin.y - plain.glyphs[3].origin.y;
        // (Within kerning, which no longer applies across the run boundaries.)
        assert!((moved - (40.0 - w)).abs() < 2.5, "the group takes one em: moved {moved}, width {w}");
        // The group is centred across the column (x of the column centre line).
        let mid = (one.origin.x + two.origin.x + two.advance) / 2.0;
        assert!((mid - plain.glyphs[0].origin.x - 0.35 * 40.0).abs() < 3.0, "{mid}");
    }

    #[test]
    fn ltr_paragraph_direction_is_forced() {
        // Arabic first: Left-to-Right keeps a left-to-right base (the Latin word stays left).
        let text = "\u{0645}\u{0631}\u{062D}\u{0628}\u{0627} abc";
        let ltr = layout_doc(&TextDoc { text: text.into(), size: 30.0, ..Default::default() });
        let mut r = TextDoc { text: text.into(), size: 30.0, ..Default::default() };
        r.direction = effectcraft_keyframe::text_doc::Direction::Rtl;
        let rtl = layout_doc(&r);
        let x_of = |l: &TextLayout, c: char| l.glyphs.iter().find(|g| g.ch == c).map(|g| g.origin.x).unwrap();
        let first_ar = '\u{0645}';
        assert!(x_of(&ltr, 'a') > x_of(&ltr, first_ar), "LTR: Arabic run first, then abc");
        assert!(x_of(&rtl, 'a') < x_of(&rtl, first_ar), "RTL: abc on the left");
    }

    #[test]
    fn centered_point_text_straddles_origin() {
        let doc = TextDoc { text: "CENTER".into(), size: 80.0, justify: Justify::Center, ..Default::default() };
        let l = layout_doc(&doc);
        assert!(l.bounds[0] < -100.0 && l.bounds[2] > 100.0, "{:?}", l.bounds);
        assert!((l.bounds[0] + l.bounds[2]).abs() < 20.0);
    }

    // #1011: a Range Selector's bars in the Composition viewer.
    fn bar_x(l: &TextLayout, based: selectors::BasedOn, f: f64) -> f64 {
        l.range_bar(based, f).unwrap()[2].x
    }

    fn doc(text: &str) -> TextLayout {
        layout_doc(&TextDoc { text: text.into(), size: 40.0, ..Default::default() })
    }

    #[test]
    fn range_bars_sit_on_unit_boundaries() {
        use selectors::BasedOn::*;
        let l = doc("abcd");
        for (f, ci) in [(0.0, 0), (0.5, 2), (1.0, 4)] {
            assert!((bar_x(&l, Characters, f) - l.caret(ci).1.x).abs() < 1e-6, "{f} at caret {ci}");
        }
        let mid = (l.caret(0).1.x + l.caret(1).1.x) / 2.0;
        assert!((bar_x(&l, Characters, 0.125) - mid).abs() < 1e-6, "half way through the first character");
        // Words and Characters Excluding Spaces skip the space.
        let l = doc("ab cd");
        assert!((bar_x(&l, Words, 0.5) - l.caret(3).1.x).abs() < 1e-6, "the second word starts at c");
        assert!((bar_x(&l, Words, 0.25) - (l.caret(0).1.x + l.caret(2).1.x) / 2.0).abs() < 1e-6, "half of ab, no space");
        assert!((bar_x(&l, CharactersExcludingSpaces, 0.5) - l.caret(3).1.x).abs() < 1e-6, "c is the third of four");
        assert!(doc("").range_bar(Characters, 0.0).is_none());
        let v = layout_doc(&TextDoc { text: "ab".into(), vertical: true, ..Default::default() });
        assert!(v.range_bar(Characters, 0.0).is_none(), "vertical text has no bars");
    }

    #[test]
    fn range_bars_on_the_right_line_ligatures_and_rtl() {
        use selectors::BasedOn::*;
        let l = doc("ab\rcd");
        // 5 characters (the CR counts): 80% is the start of d, on the second line.
        let [top, base, bottom] = l.range_bar(Characters, 0.8).unwrap();
        assert_eq!((top, bottom), l.caret(4));
        assert!(top.y < base.y && base.y < bottom.y && (base.y - bottom.y + l.layout.lines[1].descent as f64).abs() < 1e-6, "the baseline");
        assert!(bottom.y > l.caret(0).1.y + 10.0, "below the first line");
        let [top, _, bottom] = l.range_bar(Lines, 0.5).unwrap();
        assert_eq!((top, bottom), l.caret(3), "the second line starts at c");
        // Inside a ligature ("ffl") the bars still move character by character.
        let l = doc("Offline");
        let xs: Vec<f64> = [1.0, 2.0, 3.0, 4.0].iter().map(|k| bar_x(&l, Characters, k / 7.0)).collect();
        assert!(xs.windows(2).all(|w| w[1] > w[0]), "{xs:?}");
        // Right to left: Start is on the right of the first character.
        let mut r = TextDoc { text: "\u{0645}\u{0631}\u{062D}\u{0628}\u{0627}".into(), size: 40.0, ..Default::default() };
        r.direction = effectcraft_keyframe::text_doc::Direction::Rtl;
        let r = layout_doc(&r);
        assert!(bar_x(&r, Characters, 0.0) > bar_x(&r, Characters, 0.2) && bar_x(&r, Characters, 0.2) > bar_x(&r, Characters, 1.0));
    }

    #[test]
    fn range_fraction_at_inverts_range_bar() {
        use selectors::BasedOn::*;
        // (Fractions off the CR, a unit with no width whose bar is the line's end.)
        for (text, based) in [("ab\rcd", Characters), ("one two\rthree", Words), ("Offline", Characters)] {
            let l = doc(text);
            for f in [0.1, 0.3, 0.9] {
                let [top, _, bottom] = l.range_bar(based, f).unwrap();
                let p = Point::new(bottom.x, (top.y + bottom.y) / 2.0);
                let back = l.range_fraction_at(based, p).unwrap();
                assert!((back - f).abs() < 1e-6, "{text}: {f} → {back}");
            }
        }
        // The nearest line by its baseline: far left of the second line is the start of c.
        let l = doc("ab\rcd");
        let base = l.caret(3).1.y - l.layout.lines[1].descent as f64;
        assert!((l.range_fraction_at(Characters, Point::new(-500.0, base)).unwrap() - 0.6).abs() < 1e-6);
    }
}

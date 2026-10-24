//! Text layers: the text document (`btdk`) as a Source Text value, and everything of a text layer
//! that is not imported, reported.
//!
//! Mapped: the text (paragraphs separated by `\r`; U+0003, which After Effects lays out exactly
//! like `\r`, becomes `\r`), per style run the font (its PostScript name,
//! which EffectCraft resolves like a family name), size, fill and stroke colour, whether fill and
//! stroke are drawn, the stroke width, tracking, leading (automatic or explicit), faux bold /
//! italic, all caps / small caps, superscript / subscript, horizontal and vertical scale, baseline
//! shift, automatic kerning (metrics, optical, none), ligatures, tsume; per paragraph the
//! justification (also the justified variants), indents, space before / after, direction,
//! composer and hanging punctuation; stroke over fill (one switch for the whole text in
//! EffectCraft); box text as a box; vertical right-to-left text. A missing font keeps its name (the
//! renderer falls back as for any missing font) and is listed in the report.

use effectcraft_aep::{AutoKern, Caps, FontBaseline, Justification, Layer as AepLayer, ParagraphRun, PropGroup, PropNode, StyleRun as AepRun, TextDocument};
use effectcraft_keyframe::text_doc::{BaselineOption, CharStyle, Composer, Direction, Justify, Kerning, ParaStyle, StyleRun, TextDoc};

use crate::report::{Collector, LossCategory};

/// How deep the walk over the text property groups goes.
const MAX_DEPTH: usize = 32;

/// The longest text kept (characters); longer text is cut and reported.
pub(crate) const MAX_TEXT_CHARS: usize = 1 << 20;

/// U+0003 (END OF TEXT): After Effects' other paragraph break besides `\r`.
const ETX: char = '\u{3}';

/// The first paragraph of a stored text, at most 64 characters: the name After Effects shows for
/// a text layer without a name of its own (U+0003 ends a paragraph like `\r`).
pub(crate) fn first_paragraph(raw: &str) -> String {
    raw.split(['\r', ETX]).next().unwrap_or_default().chars().take(64).collect()
}

/// The Source Text value of `doc`, with what cannot be mapped reported for the layer `what`.
pub(crate) fn text_doc(doc: &TextDocument, what: &str, has_font: &dyn Fn(&str) -> bool, col: &mut Collector) -> TextDoc {
    // The stored text ends with the `\r` of the last paragraph; EffectCraft's text does not.
    let stored = doc.raw_text.strip_suffix('\r').unwrap_or(&doc.raw_text);
    // U+0003 is a line break that starts a new paragraph in After Effects (386 against 387: the
    // same two lines and two paragraphs, and the same paragraph runs as a CR). One character for
    // one character of the same byte length, so the runs and the length check below stay aligned.
    let text: String = stored.chars().take(MAX_TEXT_CHARS).map(|c| if c == ETX { '\r' } else { c }).collect();
    if text.len() < stored.len() {
        col.lose(LossCategory::DamagedData, format!("{what}: the text is longer than {MAX_TEXT_CHARS} characters and was cut"));
    }
    report_control_characters(&text, what, col);
    let chars = text.chars().count();

    let mut out = TextDoc { text, ..TextDoc::default() };
    // Character runs, cut to the text (the last stored run also covers the final `\r`).
    let mut runs = Vec::with_capacity(doc.style_runs.len());
    let mut at = 0usize;
    for run in &doc.style_runs {
        if at >= chars {
            break;
        }
        let len = run.length.min(chars - at);
        let style = char_style(doc, run, what, has_font, col);
        runs.push(StyleRun { len, style });
        at = at.saturating_add(len);
    }
    if runs.is_empty()
        && let Some(first) = doc.style_runs.first()
    {
        // Empty text: the first run's style is the style text typed into it gets.
        runs.push(StyleRun { len: 0, style: char_style(doc, first, what, has_font, col) });
    }
    if let Some(last) = runs.last_mut()
        && at < chars
    {
        last.len = last.len.saturating_add(chars - at);
    }
    if runs.is_empty() {
        col.lose(LossCategory::DamagedData, format!("{what}: the text has no readable character style, the default style is used"));
    } else {
        out.set_runs(runs);
    }

    // Paragraph settings, one per stored paragraph.
    let paras: Vec<ParaStyle> = doc.paragraphs.iter().map(|p| para_style(p, what, col)).collect();
    if !paras.is_empty() {
        out.set_paras(paras);
    }
    // Stroke over fill is one switch for the whole text in EffectCraft (per run in After Effects).
    if let Some(first) = doc.style_runs.first() {
        out.stroke_over_fill = first.stroke_over_fill;
        if doc.style_runs.iter().any(|r| r.stroke_over_fill != first.stroke_over_fill && r.apply_stroke) {
            col.lose(LossCategory::TextLayers, format!("{what}: stroke over fill differs between characters, the first character's order is used"));
        }
    }
    if !doc.kerning.is_empty() {
        col.lose(LossCategory::TextLayers, format!("{what}: manual kerning is not imported"));
    }
    match doc.line_orientation {
        None => {}
        // Vertical, right to left (575).
        Some(2) => out.vertical = true,
        Some(code) => col.lose(LossCategory::TextLayers, format!("{what}: line orientation {code} is not decoded, imported as horizontal")),
    }
    report_character_extras(doc, what, col);

    if let Some(b) = doc.text_box {
        let finite = [b.x, b.y, b.width, b.height].iter().all(|v| v.is_finite() && v.abs() <= crate::props::LIMIT);
        if finite {
            out.box_pos = [b.x, b.y];
            out.box_size = Some([b.width.max(1.0), b.height.max(1.0)]);
        } else {
            col.lose(LossCategory::DamagedData, format!("{what}: the text box is not valid, imported as point text"));
        }
    }

    if doc.incomplete_runs > 0 {
        col.lose(
            LossCategory::DamagedData,
            format!("{what}: {} damaged text style or paragraph runs (their missing values were taken from the run before)", doc.incomplete_runs),
        );
    }
    if !doc.undecoded_style_fields.is_empty() {
        col.lose(
            LossCategory::TextLayers,
            format!("{what}: character style fields not imported (they differ from the default): /{}", doc.undecoded_style_fields.join(" /")),
        );
    }
    if !doc.undecoded_paragraph_fields.is_empty() {
        col.lose(
            LossCategory::TextLayers,
            format!("{what}: paragraph fields not imported (they differ from the default): /{}", doc.undecoded_paragraph_fields.join(" /")),
        );
    }
    out
}

/// Control characters other than the paragraph break (`\r`, and U+0003, mapped to it before) are
/// kept but reported: their meaning is not sampled.
fn report_control_characters(text: &str, what: &str, col: &mut Collector) {
    let mut seen: Vec<char> = Vec::new();
    for c in text.chars().filter(|c| c.is_control() && !matches!(c, '\r' | '\n' | '\t')) {
        if !seen.contains(&c) && seen.len() < 8 {
            seen.push(c);
        }
    }
    if !seen.is_empty() {
        let list: Vec<String> = seen.iter().map(|c| format!("U+{:04X}", u32::from(*c))).collect();
        col.lose(
            LossCategory::TextLayers,
            format!("{what}: the text contains control characters whose meaning is not decoded ({}); they are kept as they are", list.join(", ")),
        );
    }
}

fn finite(v: f64, lo: f64, hi: f64) -> Option<f64> {
    v.is_finite().then(|| v.clamp(lo, hi))
}

fn rgba(argb: [f64; 4]) -> Option<[f32; 4]> {
    let [a, r, g, b] = argb;
    let c = |v: f64| v.is_finite().then(|| v.clamp(0.0, 1.0) as f32);
    Some([c(r)?, c(g)?, c(b)?, c(a)?])
}

fn char_style(doc: &TextDocument, run: &AepRun, what: &str, has_font: &dyn Fn(&str) -> bool, col: &mut Collector) -> CharStyle {
    let mut s = CharStyle::default();
    match doc.font_of(run) {
        Some(font) => {
            s.font = font.to_string();
            if !col.missing_fonts.iter().any(|f| f == font) && !has_font(font) {
                col.missing_fonts.push(font.to_string());
            }
        }
        None => col.lose(LossCategory::DamagedData, format!("{what}: a style run names no readable font, the default font is used")),
    }
    s.style = "Regular".to_string();
    match finite(run.font_size, 0.1, 10_000.0) {
        Some(size) => s.size = size,
        None => col.lose(LossCategory::DamagedData, format!("{what}: a font size is not a number, the default is used")),
    }
    match rgba(run.fill_argb) {
        Some(c) => s.fill = c,
        None => col.lose(LossCategory::DamagedData, format!("{what}: a fill colour is not valid, the default is used")),
    }
    if let Some(c) = run.stroke_argb.and_then(rgba) {
        s.stroke = c;
    }
    // Whether fill and stroke are drawn and the stroke width (544-546).
    s.apply_fill = run.apply_fill;
    s.apply_stroke = run.apply_stroke;
    s.stroke_width = finite(run.stroke_width, 0.0, 10_000.0).unwrap_or(0.0);
    s.tracking = finite(run.tracking, -100_000.0, 100_000.0).unwrap_or(0.0);
    // Scales are factors in the file (539, 540) and percent in EffectCraft; tsume is a fraction
    // (553) and a percentage.
    s.h_scale = finite(run.horizontal_scale * 100.0, 1.0, 10_000.0).unwrap_or(100.0);
    s.v_scale = finite(run.vertical_scale * 100.0, 1.0, 10_000.0).unwrap_or(100.0);
    s.baseline_shift = finite(run.baseline_shift, -100_000.0, 100_000.0).unwrap_or(0.0);
    s.tsume = finite(run.tsume * 100.0, 0.0, 100.0).unwrap_or(0.0);
    s.ligatures = run.ligatures;
    s.kerning = match run.auto_kern {
        AutoKern::Metrics => Kerning::Metrics,
        AutoKern::Optical => Kerning::Optical,
        // No automatic kerning: a manual amount of 0 replaces the metrics (542).
        AutoKern::None => Kerning::Manual(0.0),
        AutoKern::Other(code) => {
            col.lose(LossCategory::TextLayers, format!("{what}: kerning code {code} is not decoded, metrics kerning used"));
            Kerning::Metrics
        }
    };
    s.leading = if run.auto_leading { None } else { run.leading.and_then(|l| finite(l, 0.0, 100_000.0)) };
    s.faux_bold = run.faux_bold;
    s.faux_italic = run.faux_italic;
    match run.caps {
        Caps::Normal => {}
        Caps::All => s.all_caps = true,
        Caps::Small => s.small_caps = true,
        Caps::Other(code) => col.lose(LossCategory::TextLayers, format!("{what}: capitalisation code {code} is not decoded, imported as normal")),
    }
    // Superscript / subscript (526-528): After Effects' 58.3 % size and 33.3 % shift are
    // EffectCraft's too.
    s.baseline = match run.baseline {
        FontBaseline::Normal => BaselineOption::Normal,
        FontBaseline::Superscript => BaselineOption::Superscript,
        FontBaseline::Subscript => BaselineOption::Subscript,
        FontBaseline::Other(code) => {
            col.lose(LossCategory::TextLayers, format!("{what}: superscript / subscript code {code} is not decoded, imported as normal"));
            BaselineOption::Normal
        }
    };
    s
}

/// The paragraph settings of one stored paragraph.
fn para_style(p: &ParagraphRun, what: &str, col: &mut Collector) -> ParaStyle {
    let px = |v: f64| finite(v, -100_000.0, 100_000.0).unwrap_or(0.0);
    if p.leading_type != 0 {
        col.lose(LossCategory::TextLayers, format!("{what}: the Japanese leading model is not imported, Roman leading used"));
    }
    // Automatic hyphenation (`/9`) is not reported: EffectCraft never hyphenates, so a paragraph
    // with hyphenation off (556) is laid out as After Effects does.
    let rtl = p.direction == 1;
    ParaStyle {
        justify: justify(p.justification, rtl, what, col),
        indent_left: px(p.start_indent),
        indent_right: px(p.end_indent),
        indent_first: px(p.first_line_indent),
        space_before: px(p.space_before),
        space_after: px(p.space_after),
        direction: match p.direction {
            0 => Direction::Ltr,
            1 => Direction::Rtl,
            code => {
                col.lose(LossCategory::TextLayers, format!("{what}: paragraph direction {code} is not decoded, left to right used"));
                Direction::Ltr
            }
        },
        // A new After Effects text uses the single-line composer (`/29` false, 557, 559).
        composer: if p.every_line_composer { Composer::EveryLine } else { Composer::SingleLine },
        hanging_punctuation: p.hanging_roman,
    }
}

/// Character settings EffectCraft has no field for, reported once per text.
fn report_character_extras(doc: &TextDocument, what: &str, col: &mut Collector) {
    let runs = &doc.style_runs;
    if runs.iter().any(|r| r.no_break) {
        col.lose(LossCategory::TextLayers, format!("{what}: No Break is not imported"));
    }
    if runs.iter().any(|r| r.apply_stroke && r.line_join != 0) {
        col.lose(LossCategory::TextLayers, format!("{what}: the stroke line join is not imported, miter used"));
    }
    // Digit set 1 (Arabic) lays out Latin digits as the default does (571: same bounds of `1234`).
    if runs.iter().any(|r| r.digit_set > 1 || r.digit_set < 0) {
        col.lose(LossCategory::TextLayers, format!("{what}: the digit set is not imported"));
    }
    // The baseline direction only matters for vertical text.
    if doc.line_orientation.is_some() {
        let tcy = runs.iter().any(|r| r.baseline_direction == 3);
        if tcy || runs.iter().any(|r| r.baseline_direction != 1 && r.baseline_direction != 3) {
            col.lose(LossCategory::TextLayers, format!("{what}: the baseline direction of vertical text is not imported"));
        }
    }
}

/// The stored justification is relative to the paragraph direction: in a right-to-left paragraph
/// After Effects lays "left" (0) and "justify, last line left" (3) out flush right and "right"
/// (1, 4) flush left (export test t7: point and box text with all seven codes; 2, 5 and 6 are
/// the same in both directions). EffectCraft's justification is visual, so the two pairs swap.
fn justify(j: Justification, rtl: bool, what: &str, col: &mut Collector) -> Justify {
    match j {
        Justification::Left if rtl => Justify::Right,
        Justification::Right if rtl => Justify::Left,
        Justification::FullLeft if rtl => Justify::JustifyLastRight,
        Justification::FullRight if rtl => Justify::JustifyLastLeft,
        Justification::Left => Justify::Left,
        Justification::Right => Justify::Right,
        Justification::Center => Justify::Center,
        Justification::FullLeft => Justify::JustifyLastLeft,
        Justification::FullRight => Justify::JustifyLastRight,
        Justification::FullCenter => Justify::JustifyLastCenter,
        Justification::FullAll => Justify::JustifyAll,
        Justification::Other(code) => {
            col.lose(LossCategory::TextLayers, format!("{what}: justification code {code} is not decoded, imported as left"));
            Justify::Left
        }
    }
}

/// Whether a group holds any stored value (a set, keyed or expression property).
fn has_stored(g: &PropGroup<'_, '_>, depth: usize) -> bool {
    if depth > MAX_DEPTH {
        return true;
    }
    g.entries().iter().any(|e| match e.node {
        PropNode::Group(sub) => has_stored(&sub, depth.saturating_add(1)),
        PropNode::Property(p) => p.flags() == Some(1) || p.is_animated() || p.expression().is_some(),
        PropNode::Other(_) => false,
    })
}

/// The parts of a text layer besides its first text document that are not imported.
pub(crate) fn report_text_extras(layer: &AepLayer<'_, '_>, what: &str, col: &mut Collector) {
    if let Some(st) = layer.source_text_property() {
        if st.is_animated() || layer.text_document_count() > 1 {
            col.lose(LossCategory::TextLayers, format!("{what}: Source Text keyframes are not imported (the first text document is)"));
        }
        if st.expression().is_some() {
            col.lose(LossCategory::TextLayers, format!("{what}: the Source Text expression is not imported"));
        }
    }
    let Some(PropNode::Group(text)) = layer.properties().and_then(|p| p.get("ADBE Text Properties")) else { return };
    for e in text.entries() {
        let PropNode::Group(g) = e.node else { continue };
        match e.match_name.as_str() {
            // Imported by `text_anim::import_animators`.
            "ADBE Text Animators" => {}
            "ADBE Text Path Options" if has_stored(&g, 0) => col.lose(LossCategory::TextLayers, format!("{what}: Path Options are not imported")),
            "ADBE Text More Options" if has_stored(&g, 0) => col.lose(LossCategory::TextLayers, format!("{what}: More Options are not imported")),
            _ => {}
        }
    }
}

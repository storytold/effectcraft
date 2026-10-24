//! Text layers: the layer block of a point (110) or box (245) text layer of our corpus
//! (`template/text.aep`, `examples/mktxprotos.rs`) with its text document rewritten from the
//! layer's Source Text (`docs/aep-format.md` 3.10, `docs/aep-export.md` R32-R34):
//!
//! * the text (paragraphs end with `\r`, the last one too), the paragraph runs with their
//!   justification, the style runs with font, size, faux bold / italic, automatic or explicit
//!   leading, tracking, caps, fill and stroke colour;
//! * fonts other than the prototype's are appended to the font list (its own entries, which the
//!   rest of the document refers to by index, keep their places);
//! * a box text's frame (16 points tracing the rectangle, 245);
//! * the composed layout (`/1/1[0]/1/2`) is left empty: After Effects lays the text out again
//!   when it opens the project (t1-t3).

use effectcraft_keyframe::Value;
use effectcraft_keyframe::text_doc::{BaselineOption, Composer, Direction, Justify, Kerning, TextDoc};
use effectcraft_project::{Layer, Node, PropGroup};

use crate::report::{Collector, LossCategory};
use crate::textdoc::{TNode, boolean, int, number, string};

/// The prototype's own font (index 0 of its font list).
const PROTO_FONT: &str = "TimesNewRomanPSMT";
/// The longest text written (characters).
const MAX_CHARS: usize = 1 << 20;

/// One style run in the file's terms.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct StyleOut {
    pub font: String,
    pub size: f64,
    pub fill: [f64; 3],
    pub stroke: [f64; 3],
    pub tracking: f64,
    /// `None` = automatic leading.
    pub leading: Option<f64>,
    pub faux_bold: bool,
    pub faux_italic: bool,
    /// 0 normal, 1 small caps, 2 all caps.
    pub caps: i64,
    /// Style slot `/13`: 0 normal, 1 superscript, 2 subscript (526-528).
    pub script: i64,
    /// Slots `/6` and `/7`: horizontal and vertical scale as factors (539, 540).
    pub h_scale: f64,
    pub v_scale: f64,
    /// Slot `/9`: baseline shift in pixels.
    pub baseline_shift: f64,
    /// Slot `/11`: automatic kerning, 0 none, 1 metrics, 2 optical (541-543).
    pub auto_kern: i64,
    /// Slot `/18`: standard ligatures (555).
    pub ligatures: bool,
    /// Slot `/36`: tsume as a fraction (553).
    pub tsume: f64,
    /// Slots `/56`, `/57`: whether fill and stroke are drawn (544-546).
    pub apply_fill: bool,
    pub apply_stroke: bool,
    /// Slot `/63`: stroke width in pixels; `None` keeps the prototype's value.
    pub stroke_width: Option<f64>,
    /// Slot `/58`: stroke over fill (one switch for the whole text in EffectCraft; 550).
    pub stroke_over_fill: bool,
}

/// One paragraph in the file's terms (paragraph dictionary `RUN/5/0[n]/0/0/5`).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ParaOut {
    /// `/0`: 0 left, 1 right, 2 centre, 3-6 justified with the last line left / right / centre /
    /// justified.
    pub justify: i64,
    /// `/1` first-line indent, `/2` start indent, `/3` end indent, `/4` space before, `/5` space
    /// after, in pixels.
    pub indents: [f64; 5],
    /// `/21`: Roman hanging punctuation (558).
    pub hanging: bool,
    /// `/29`: the every-line composer (557, 559).
    pub every_line: bool,
    /// `/33`: 0 left to right, 1 right to left (569).
    pub direction: i64,
}

/// A text document in the file's terms.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TextPlan {
    /// The stored text: paragraphs separated and ended by `\r`.
    pub text: String,
    /// (characters, style), covering the text including the final `\r`.
    pub runs: Vec<(usize, StyleOut)>,
    /// (characters including the `\r`, paragraph settings).
    pub paras: Vec<(usize, ParaOut)>,
    /// Box text: left, top, width, height in layer pixels.
    pub frame: Option<[f64; 4]>,
    /// The text animators (planned with the layer's time base, `textanim.rs`).
    pub animators: Vec<crate::textanim::AnimPlan>,
}

fn rgb(c: [f32; 4]) -> [f64; 3] {
    let f = |v: f32| if v.is_finite() { f64::from(v.clamp(0.0, 1.0)) } else { 0.0 };
    [f(c[0]), f(c[1]), f(c[2])]
}

fn sane(v: f64, lo: f64, hi: f64, default: f64) -> f64 {
    if v.is_finite() { v.clamp(lo, hi) } else { default }
}

/// Plans the text of a text layer and reports what the document cannot hold.
pub(crate) fn plan_text(layer: &Layer, comp_name: &str, col: &mut Collector) -> Option<TextPlan> {
    let what = format!("{comp_name}: '{}'", layer.name);
    let Some(text) = layer.props.sub("text") else {
        col.lose(LossCategory::TextLayers, format!("{what}: a text layer without text properties"));
        return None;
    };
    let Some(st) = text.prop("sourceText") else {
        col.lose(LossCategory::TextLayers, format!("{what}: a text layer without Source Text"));
        return None;
    };
    if !st.keys.is_empty() {
        col.lose(LossCategory::TextLayers, format!("{what}: Source Text keyframes are not written (the first key's text is)"));
    }
    if st.has_expression() {
        col.lose(LossCategory::TextLayers, format!("{what}: the Source Text expression is not written"));
    }
    let value = st.keys.first().map(|k| &k.value).unwrap_or(&st.value);
    let Value::Text(doc) = value else {
        col.lose(LossCategory::TextLayers, format!("{what}: Source Text holds no text document"));
        return None;
    };
    report_extras(text, &what, col);
    Some(plan_doc(doc, &what, col))
}

/// Path / More Options that hold anything other than a new text layer's defaults are not
/// written (animators are, `textanim.rs`).
fn report_extras(text: &PropGroup, what: &str, col: &mut Collector) {
    let mut next = 1u64;
    let reference = effectcraft_project::build::text(&mut effectcraft_project::build::Ids(&mut next), TextDoc::default());
    for node in &text.children {
        let Node::Group(g) = node else { continue };
        match g.match_id.as_str() {
            "pathOptions" | "moreOptions" => {
                let def = reference.children.iter().find_map(|n| match n {
                    Node::Group(r) if r.match_id == g.match_id => Some(r),
                    _ => None,
                });
                if differs(g, def, 0) {
                    col.lose(LossCategory::TextLayers, format!("{what}: {} are not written", g.name));
                }
            }
            _ => {}
        }
    }
}

/// Whether `g` holds keys, expressions or values other than those of `def` (the same group of a
/// new text layer).
fn differs(g: &PropGroup, def: Option<&PropGroup>, depth: usize) -> bool {
    if depth > 32 {
        return true;
    }
    g.children.iter().any(|n| match n {
        Node::Group(s) => {
            let d = def.and_then(|d| {
                d.children.iter().find_map(|x| match x {
                    Node::Group(r) if r.match_id == s.match_id => Some(r),
                    _ => None,
                })
            });
            differs(s, d, depth + 1)
        }
        Node::Prop(p) => {
            let d = def.and_then(|d| d.prop(&p.match_id));
            !p.keys.is_empty() || p.has_expression() || d.is_none_or(|d| d.value != p.value)
        }
    })
}

fn plan_doc(doc: &TextDoc, what: &str, col: &mut Collector) -> TextPlan {
    let base = doc.base_style();
    let runs = doc.runs();
    // Characters in EffectCraft's text, each with its run, CRLF and LF made After Effects' CR.
    let chars: Vec<char> = doc.text.chars().collect();
    let mut style_of: Vec<usize> = Vec::with_capacity(chars.len());
    for (ri, r) in runs.iter().enumerate() {
        style_of.extend(std::iter::repeat_n(ri, r.len));
    }
    let mut text = String::with_capacity(doc.text.len() + 1);
    let mut out_style: Vec<usize> = Vec::new();
    let mut i = 0;
    while i < chars.len() && out_style.len() < MAX_CHARS {
        let c = chars.get(i).copied().unwrap_or(' ');
        let ri = style_of.get(i).copied().unwrap_or(runs.len().saturating_sub(1));
        if c == '\r' && chars.get(i + 1) == Some(&'\n') {
            i += 1;
        }
        text.push(if c == '\n' { '\r' } else { c });
        out_style.push(ri);
        i += 1;
    }
    if i < chars.len() {
        col.lose(LossCategory::TextLayers, format!("{what}: the text is longer than {MAX_CHARS} characters and was cut"));
    }
    // The final paragraph end belongs to the last run.
    text.push('\r');
    out_style.push(out_style.last().copied().unwrap_or(0));

    let mut styles: Vec<StyleOut> = Vec::with_capacity(runs.len().max(1));
    for r in &runs {
        styles.push(style_out(&r.style, what, col));
    }
    if styles.is_empty() {
        styles.push(style_out(&base, what, col));
    }
    for st in &mut styles {
        st.stroke_over_fill = doc.stroke_over_fill;
    }
    let mut out_runs: Vec<(usize, StyleOut)> = Vec::new();
    for ri in out_style {
        let s = styles.get(ri).or(styles.last()).cloned();
        let Some(s) = s else { continue };
        match out_runs.last_mut() {
            Some((n, last)) if *last == s => *n += 1,
            _ => out_runs.push((1, s)),
        }
    }

    let paras_in = doc.paras();
    let mut paras = Vec::new();
    let mut at = 0usize;
    for (pi, part) in text.split_inclusive('\r').enumerate() {
        let len = part.chars().count();
        // A paragraph without its own settings takes the document's (every-line composer by
        // default, as a new text in After Effects' Paragraph panel).
        let base = doc.base_para();
        let p = paras_in.get(pi).or(paras_in.last()).unwrap_or(&base);
        // The stored code is relative to the direction: right-to-left swaps left / right and
        // the two justified variants with a left / right last line (t7, as the importer).
        let rtl = p.direction == Direction::Rtl;
        let j = match p.justify {
            Justify::Left if rtl => 1,
            Justify::Right if rtl => 0,
            Justify::JustifyLastLeft if rtl => 4,
            Justify::JustifyLastRight if rtl => 3,
            Justify::Left => 0,
            Justify::Right => 1,
            Justify::Center => 2,
            // The justified variants, as the importer reads them (paragraph `/0`, 3-6).
            Justify::JustifyLastLeft => 3,
            Justify::JustifyLastRight => 4,
            Justify::JustifyLastCenter => 5,
            Justify::JustifyAll => 6,
        };
        let px = |v: f64| sane(v, -100_000.0, 100_000.0, 0.0);
        let po = ParaOut {
            justify: j,
            indents: [px(p.indent_first), px(p.indent_left), px(p.indent_right), px(p.space_before), px(p.space_after)],
            hanging: p.hanging_punctuation,
            every_line: p.composer == Composer::EveryLine,
            direction: i64::from(rtl),
        };
        paras.push((len, po));
        at += len;
    }
    let _ = at;
    if doc.vertical {
        col.lose(LossCategory::TextLayers, format!("{what}: vertical text is written horizontally"));
    }
    let frame =
        doc.box_size.map(|[w, h]| [sane(doc.box_pos[0], -1e6, 1e6, 0.0), sane(doc.box_pos[1], -1e6, 1e6, 0.0), sane(w, 1.0, 1e6, 1.0), sane(h, 1.0, 1e6, 1.0)]);
    TextPlan { text, runs: out_runs, paras, frame, animators: Vec::new() }
}

fn style_out(s: &effectcraft_keyframe::text_doc::CharStyle, what: &str, col: &mut Collector) -> StyleOut {
    let def = effectcraft_keyframe::text_doc::CharStyle::default();
    let auto_kern = match s.kerning {
        Kerning::Metrics => 1,
        Kerning::Optical => 2,
        // No automatic kerning is a manual amount of 0 (542).
        Kerning::Manual(k) => {
            if k != 0.0 {
                col.lose(LossCategory::TextLayers, format!("{what}: a manual kerning amount is not written (written without automatic kerning)"));
            }
            0
        }
    };
    if !s.opentype.is_default() || !s.variations.is_empty() {
        col.lose(LossCategory::TextLayers, format!("{what}: OpenType features and variable font axes are not written"));
    }
    let _ = def;
    if s.style != "Regular" && !s.style.is_empty() {
        col.lose(LossCategory::TextLayers, format!("{what}: the font style '{}' is not written (the font is named by its PostScript name)", s.style));
    }
    StyleOut {
        font: if s.font.is_empty() { PROTO_FONT.to_string() } else { s.font.clone() },
        size: sane(s.size, 0.1, 10_000.0, 100.0),
        fill: rgb(s.fill),
        stroke: rgb(s.stroke),
        tracking: sane(s.tracking, -100_000.0, 100_000.0, 0.0),
        leading: s.leading.map(|l| sane(l, 0.0, 100_000.0, 0.0)),
        faux_bold: s.faux_bold,
        faux_italic: s.faux_italic,
        caps: if s.all_caps {
            2
        } else if s.small_caps {
            1
        } else {
            0
        },
        script: match s.baseline {
            BaselineOption::Normal => 0,
            BaselineOption::Superscript => 1,
            BaselineOption::Subscript => 2,
        },
        h_scale: sane(s.h_scale, 1.0, 10_000.0, 100.0) / 100.0,
        v_scale: sane(s.v_scale, 1.0, 10_000.0, 100.0) / 100.0,
        baseline_shift: sane(s.baseline_shift, -100_000.0, 100_000.0, 0.0),
        auto_kern,
        ligatures: s.ligatures,
        tsume: sane(s.tsume, 0.0, 100.0, 0.0) / 100.0,
        apply_fill: s.apply_fill,
        apply_stroke: s.apply_stroke,
        stroke_width: (s.apply_stroke || s.stroke_width > 0.0).then(|| sane(s.stroke_width, 0.0, 10_000.0, 1.0)),
        stroke_over_fill: true,
    }
}

// ---------------------------------------------------------------------------------------------
// The document.

/// The number an atom holds (`1`, `1.0`, `.5`).
fn atom_f64(n: Option<&TNode>) -> Option<f64> {
    match n? {
        TNode::Atom(a) => std::str::from_utf8(a).ok()?.trim().parse::<f64>().ok(),
        _ => None,
    }
}

/// Sets a number slot unless it already holds `v` (`absent` when the slot is missing).
fn set_num_if(d: &mut TNode, key: &str, v: f64, absent: f64) {
    let cur = match d.get(key) {
        None => Some(absent),
        n => atom_f64(n),
    };
    if cur.is_none_or(|c| (c - v).abs() > 1e-9) {
        d.set(key, number(v));
    }
}

/// Sets a boolean slot unless it already holds `v` (`absent` when the slot is missing).
fn set_bool_if(d: &mut TNode, key: &str, v: bool, absent: bool) {
    let cur = match d.get(key) {
        None => Some(absent),
        Some(TNode::Atom(a)) if a.as_slice() == b"true" => Some(true),
        Some(TNode::Atom(a)) if a.as_slice() == b"false" => Some(false),
        Some(_) => None,
    };
    if cur != Some(v) {
        d.set(key, boolean(v));
    }
}

fn num_or_int(v: f64) -> TNode {
    if v.fract() == 0.0 && v.abs() < 1e12 { int(v as i64) } else { number(v) }
}

fn paint(rgb: [f64; 3]) -> TNode {
    TNode::Array(vec![number(1.0), number(rgb[0]), number(rgb[1]), number(rgb[2])])
}

/// The rectangle as After Effects traces it: each corner twice at the ends of the list and four
/// times in between (245: top-left, top-right, bottom-right, bottom-left).
fn frame_points(f: [f64; 4]) -> TNode {
    let [x, y, w, h] = f;
    let (l, t, r, b) = (x, y, x + w, y + h);
    let corners = [(l, t), (l, t), (r, t), (r, t), (r, t), (r, t), (r, b), (r, b), (r, b), (r, b), (l, b), (l, b), (l, b), (l, b), (l, t), (l, t)];
    TNode::Array(corners.iter().flat_map(|(px, py)| [number(*px), number(*py)]).collect())
}

/// The prototype's document (110 or 245) rewritten for `tp`; `None` when the prototype does not
/// have the expected shape.
pub(crate) fn build_doc(proto: &TNode, tp: &TextPlan) -> Option<TNode> {
    let mut doc = proto.clone();
    // Fonts: the prototype's own stay where the document refers to them; others are appended.
    let mut font_index: Vec<(String, usize)> = Vec::new();
    {
        let list = doc.path_mut("0/1/0")?;
        let TNode::Array(entries) = list else { return None };
        let template = entries.first()?.clone();
        let name_of = |e: &TNode| -> Option<String> {
            match e.path("0/0/0")? {
                TNode::Atom(a) => Some(String::from_utf8_lossy(a).to_string()),
                _ => None,
            }
        };
        for (_, s) in &tp.runs {
            if font_index.iter().any(|(f, _)| f == &s.font) {
                continue;
            }
            let atom = match string(&s.font) {
                TNode::Atom(a) => String::from_utf8_lossy(&a).to_string(),
                _ => continue,
            };
            let at = match entries.iter().position(|e| name_of(e).as_deref() == Some(atom.as_str())) {
                Some(i) => i,
                None => {
                    let mut e = template.clone();
                    let inner = e.path_mut("0/0")?;
                    inner.set("0", string(&s.font));
                    inner.set("2", int(0));
                    inner.remove("5");
                    entries.push(e);
                    entries.len() - 1
                }
            };
            font_index.push((s.font.clone(), at));
        }
    }
    {
        let run = doc.path_mut("1/1[0]/0")?;
        run.set("0", string(&tp.text));
        let p0 = run.path("5/0[0]")?.clone();
        let paras: Vec<TNode> = tp
            .paras
            .iter()
            .map(|(len, po)| {
                let mut p = p0.clone();
                p.set("1", int(i64::try_from(*len).unwrap_or(i64::MAX)));
                if let Some(d) = p.path_mut("0/0/5") {
                    d.set("0", int(po.justify));
                    // The paragraph settings decoded by the importer (560-569), written only
                    // where they differ from the prototype.
                    for (k, v) in ["1", "2", "3", "4", "5"].iter().zip(po.indents) {
                        set_num_if(d, k, v, 0.0);
                    }
                    set_bool_if(d, "21", po.hanging, false);
                    set_bool_if(d, "29", po.every_line, false);
                    if atom_f64(d.get("33")).unwrap_or(0.0) != po.direction as f64 {
                        d.set("33", int(po.direction));
                    }
                }
                p
            })
            .collect();
        *run.path_mut("5/0")? = TNode::Array(paras);
        let s0 = run.path("6/0[0]")?.clone();
        let mut styles = Vec::with_capacity(tp.runs.len());
        for (len, s) in &tp.runs {
            let mut r = s0.clone();
            r.set("1", int(i64::try_from(*len).unwrap_or(i64::MAX)));
            let d = r.path_mut("0/0/6")?;
            let fi = font_index.iter().find(|(f, _)| f == &s.font).map_or(0, |(_, i)| *i);
            d.set("0", int(i64::try_from(fi).unwrap_or(0)));
            d.set("1", number(s.size));
            d.set("2", boolean(s.faux_bold));
            d.set("3", boolean(s.faux_italic));
            // Automatic leading: /4 true and /5 0.01 (400); explicit: /4 false, /5 the leading.
            d.set("4", boolean(s.leading.is_none()));
            d.set("5", number(s.leading.unwrap_or(0.01)));
            d.set("8", num_or_int(s.tracking));
            d.set("12", int(s.caps));
            // Normal text must store 0: the prototypes (and every text After Effects creates by
            // script) hold 1, superscript, which After Effects draws at 58.3 % size (526-528).
            d.set("13", int(s.script));
            // The character settings decoded by the importer (539-555): written only where they
            // differ from what the slot holds, so a document in the default state keeps the
            // prototype's bytes.
            set_num_if(d, "6", s.h_scale, 1.0);
            set_num_if(d, "7", s.v_scale, 1.0);
            set_num_if(d, "9", s.baseline_shift, 0.0);
            if atom_f64(d.get("11")).unwrap_or(1.0) != s.auto_kern as f64 {
                d.set("11", int(s.auto_kern));
            }
            set_bool_if(d, "18", s.ligatures, false);
            set_num_if(d, "36", s.tsume, 0.0);
            set_bool_if(d, "56", s.apply_fill, true);
            set_bool_if(d, "57", s.apply_stroke, false);
            set_bool_if(d, "58", s.stroke_over_fill, true);
            if let Some(w) = s.stroke_width {
                set_num_if(d, "63", w, 1.0);
            }
            if let Some(fill) = d.path_mut("53/0") {
                fill.set("1", paint(s.fill));
            }
            if let Some(stroke) = d.path_mut("54/0") {
                stroke.set("1", paint(s.stroke));
            }
            styles.push(r);
        }
        *run.path_mut("6/0")? = TNode::Array(styles);
    }
    if let Some(f) = tp.frame {
        doc.path_mut("0/8/0[0]/0/1")?.set("0", frame_points(f));
    }
    // The composed layout is dropped: After Effects lays the text out again (t3).
    doc.path_mut("1/1[0]/1")?.set("2", TNode::Array(Vec::new()));
    Some(doc)
}

/// Rewrites the text document of a text layer block's `Layr` for `tp` and gives the document a
/// fresh `btgu/pgui`.
pub(crate) fn put_doc(layr: &mut effectcraft_aep::Chunk<'static>, tp: &TextPlan, guid: [u8; 16]) -> crate::tree::R<()> {
    use crate::tree::tpl;
    use effectcraft_aep::Body;
    fn find<'c>(c: &'c mut effectcraft_aep::Chunk<'static>, depth: usize) -> Option<&'c mut effectcraft_aep::Chunk<'static>> {
        if depth > 32 {
            return None;
        }
        if matches!(&c.body, Body::OpaqueList { list_type, .. } if list_type == b"btdk") {
            return Some(c);
        }
        match &mut c.body {
            Body::List { children, .. } | Body::Chunks(children) => children.iter_mut().find_map(|k| find(k, depth + 1)),
            _ => None,
        }
    }
    fn find_pgui<'c>(c: &'c mut effectcraft_aep::Chunk<'static>, depth: usize) -> Option<&'c mut effectcraft_aep::Chunk<'static>> {
        if depth > 32 {
            return None;
        }
        if c.is_list(b"btgu") {
            return match &mut c.body {
                Body::List { children, .. } => children.iter_mut().find(|k| k.is(b"pgui")),
                _ => None,
            };
        }
        match &mut c.body {
            Body::List { children, .. } | Body::Chunks(children) => children.iter_mut().find_map(|k| find_pgui(k, depth + 1)),
            _ => None,
        }
    }
    let btdk = find(layr, 0).ok_or_else(|| tpl("text prototype without a document"))?;
    let Body::OpaqueList { data, .. } = &mut btdk.body else { return Err(tpl("text prototype without a document")) };
    let proto = crate::textdoc::parse(data.as_ref()).ok_or_else(|| tpl("the text prototype's document does not parse"))?;
    let doc = build_doc(&proto, tp).ok_or_else(|| tpl("the text prototype's document has another shape"))?;
    *data = std::borrow::Cow::Owned(crate::textdoc::write(&doc));
    if let Some(p) = find_pgui(layr, 0) {
        *p = effectcraft_aep::Chunk::data(*b"pgui", guid.to_vec());
    }
    Ok(())
}

//! Text layers ↔ Lottie text data (`t`: document keys `d`, animators `a`, more options `m`),
//! and text converted to glyph shapes.

use craft_lottie::model::{
    Font, Map, Path, Shape, ShapeKind, TextAlign, TextData, TextDocKey, TextDocs, TextDocument, TextFollowPath, TextRange, TextSelector, TextStyle,
};
use craft_lottie::{Animated, Json};
use effectcraft_keyframe::{Justify, TextDoc, Value};
use effectcraft_project::build::{self, Ids};
use effectcraft_project::{Layer, Node, PropGroup};
use effectcraft_time::Tick;
use serde_json::json;

use crate::anim::{self, Ex, Im, export_prop, import_prop};
use crate::shapes::{self, fixed};

const JUSTIFY: [Justify; 7] =
    [Justify::Left, Justify::Right, Justify::Center, Justify::JustifyLastLeft, Justify::JustifyLastRight, Justify::JustifyLastCenter, Justify::JustifyAll];

/// Font key (`fName`) of a family/style.
pub(crate) fn font_name(family: &str, style: &str) -> String {
    format!("{}-{}", family.replace(' ', ""), style.replace(' ', ""))
}

fn rgb(c: [f32; 4]) -> Vec<f64> {
    vec![f64::from(c[0]), f64::from(c[1]), f64::from(c[2])]
}

fn doc_json(d: &TextDoc) -> TextDocument {
    let stroke = d.apply_stroke && d.stroke_width > 0.0;
    TextDocument {
        text: Some(d.text.replace("\r\n", "\r").replace('\n', "\r")),
        font: Some(font_name(&d.font, &d.style)),
        size: Some(d.size),
        justify: Some(JUSTIFY.iter().position(|j| *j == d.justify).unwrap_or(0) as i64),
        tracking: Some(d.tracking),
        line_height: Some(d.leading.unwrap_or(d.size * 1.2)),
        baseline_shift: Some(d.baseline_shift),
        stroke_over_fill: Some(d.stroke_over_fill),
        fill: d.apply_fill.then(|| rgb(d.fill)),
        stroke: stroke.then(|| rgb(d.stroke)),
        stroke_width: stroke.then_some(d.stroke_width),
        caps: if d.all_caps {
            Some(1)
        } else if d.small_caps {
            Some(2)
        } else {
            None
        },
        box_size: d.box_size.map(|b| b.to_vec()),
        box_position: d.box_size.map(|_| d.box_pos.to_vec()),
        ..Default::default()
    }
}

/// A colour of a text document (`fc`, `sc`), 0..=1 or 0..=255.
fn doc_color(c: Option<&Vec<f64>>, extra: &Map, key: &str) -> Option<[f32; 4]> {
    let j = c.filter(|c| !c.is_empty()).map(|c| json!(c)).or_else(|| extra.get(key).cloned())?;
    anim::to_color(&j).map(|v| v.as_color())
}

fn doc_from(j: &TextDocument, fonts: &[(String, String, String)]) -> TextDoc {
    let mut d = TextDoc::default();
    if let Some(t) = &j.text {
        d.text = t.replace(['\r', '\u{3}'], "\n");
    }
    if let Some(name) = &j.font {
        match fonts.iter().find(|(n, _, _)| n == name) {
            Some((_, fam, st)) => {
                d.font = fam.clone();
                d.style = st.clone();
            }
            None => {
                let (fam, st) = name.split_once('-').unwrap_or((name, "Regular"));
                d.font = fam.to_string();
                d.style = st.to_string();
            }
        }
    }
    if let Some(s) = j.size {
        d.size = s;
    }
    match doc_color(j.fill.as_ref(), &j.extra, "fc") {
        Some(c) => d.fill = c,
        None => d.apply_fill = false,
    }
    if let Some(c) = doc_color(j.stroke.as_ref(), &j.extra, "sc") {
        d.stroke = c;
        d.apply_stroke = true;
        d.stroke_width = j.stroke_width.unwrap_or(1.0);
    }
    if let Some(of) = j.stroke_over_fill {
        d.stroke_over_fill = of;
    }
    if let Some(i) = j.justify.and_then(|i| usize::try_from(i).ok()) {
        d.justify = JUSTIFY.get(i).copied().unwrap_or_default();
    }
    d.tracking = j.tracking.unwrap_or(0.0);
    if let Some(lh) = j.line_height
        && (lh - d.size * 1.2).abs() > 1e-6
    {
        d.leading = Some(lh);
    }
    d.baseline_shift = j.baseline_shift.unwrap_or(0.0);
    match j.caps {
        Some(1) => d.all_caps = true,
        Some(2) => d.small_caps = true,
        _ => {}
    }
    if let Some(sz) = j.box_size.as_deref().filter(|v| v.len() >= 2) {
        d.box_size = Some([sz[0], sz[1]]);
        if let Some(ps) = j.box_position.as_deref().filter(|v| v.len() >= 2) {
            d.box_pos = [ps[0], ps[1]];
        }
    }
    d
}

/// Lottie text animator property key → (animator kind, match id).
const ANIM_PROPS: &[(&str, &str, &str)] = &[
    ("a", "anchor", "anchor"),
    ("p", "position", "position"),
    ("s", "scale", "scale"),
    ("sk", "skew", "skew"),
    ("sa", "skew", "skewAxis"),
    ("r", "rotation", "rotation"),
    ("o", "opacity", "opacity"),
    ("fc", "fillColor", "fillColor"),
    ("sc", "strokeColor", "strokeColor"),
    ("sw", "strokeWidth", "strokeWidth"),
    ("fh", "fillHue", "fillHue"),
    ("fs", "fillSaturation", "fillSaturation"),
    ("fb", "fillBrightness", "fillBrightness"),
    ("t", "tracking", "tracking"),
];

fn conv_for(v: &Value) -> fn(&Value) -> Json {
    match v {
        Value::Vec2(_) | Value::Vec3(_) | Value::Color(_) => anim::arr,
        _ => anim::num,
    }
}

/// The field of a range animator's style for a Lottie key.
fn style_field<'a>(st: &'a mut TextStyle, key: &str) -> Option<&'a mut Option<Animated>> {
    Some(match key {
        "p" => &mut st.position,
        "a" => &mut st.anchor,
        "s" => &mut st.scale,
        "r" => &mut st.rotation,
        "rx" => &mut st.rotation_x,
        "ry" => &mut st.rotation_y,
        "sk" => &mut st.skew,
        "sa" => &mut st.skew_axis,
        "o" => &mut st.opacity,
        "fc" => &mut st.fill,
        "fh" => &mut st.fill_hue,
        "fs" => &mut st.fill_saturation,
        "fb" => &mut st.fill_brightness,
        "fo" => &mut st.fill_opacity,
        "sc" => &mut st.stroke,
        "sw" => &mut st.stroke_width,
        "so" => &mut st.stroke_opacity,
        "t" => &mut st.tracking,
        "ls" => &mut st.line_spacing,
        "bl" => &mut st.blur,
        _ => return None,
    })
}

fn style_set(st: &mut TextStyle, key: &str, v: Animated) {
    match style_field(st, key) {
        Some(f) => *f = Some(v),
        None => {
            st.extra.insert(key.into(), v.0);
        }
    }
}

fn style_get(st: &TextStyle, key: &str) -> Option<Json> {
    let named = match key {
        "p" => &st.position,
        "a" => &st.anchor,
        "s" => &st.scale,
        "r" => &st.rotation,
        "sk" => &st.skew,
        "sa" => &st.skew_axis,
        "o" => &st.opacity,
        "fc" => &st.fill,
        "fh" => &st.fill_hue,
        "fs" => &st.fill_saturation,
        "fb" => &st.fill_brightness,
        "sc" => &st.stroke,
        "sw" => &st.stroke_width,
        "t" => &st.tracking,
        _ => &None,
    };
    named.as_ref().map(|a| a.0.clone()).or_else(|| st.extra.get(key).cloned())
}

/// Export a text layer's `t` object. Fonts used are added to `fonts`.
pub(crate) fn export_text(ex: &mut Ex, layer: &Layer, fonts: &mut Vec<Font>) -> TextData {
    let Some(tg) = layer.props.sub("text") else { return TextData::default() };
    let label = &layer.name;
    let mut docs = vec![];
    let mut add_font = |d: &TextDoc| {
        let n = font_name(&d.font, &d.style);
        if !fonts.iter().any(|f| f.name.as_deref() == Some(n.as_str())) {
            fonts.push(Font { name: Some(n), family: Some(d.font.clone()), style: Some(d.style.clone()), ascent: Some(75.0), ..Default::default() });
        }
    };
    let key = |t: f64, d: &TextDoc| TextDocKey { time: Some(t), document: Some(doc_json(d)), ..Default::default() };
    if let Some(st) = tg.get("sourceText") {
        if st.keys.is_empty() {
            if let Value::Text(d) = &st.value {
                add_font(d);
                docs.push(key(0.0, d));
            }
        } else {
            for k in &st.keys {
                if let Value::Text(d) = &k.value {
                    add_font(d);
                    docs.push(key(ex.tb.frame(k.time), d));
                }
            }
        }
        if st.has_expression() {
            ex.warn(format!("{label} ▸ Source Text: expressions on Source Text are not exported"));
        }
        if let Value::Text(d) = &st.value
            && ((d.h_scale - 100.0).abs() > 1e-9 || (d.v_scale - 100.0).abs() > 1e-9 || d.faux_bold || d.faux_italic)
        {
            ex.warn(format!("{label}: horizontal/vertical scale and faux styles are not part of Lottie text"));
        }
    }
    let mut animators = vec![];
    // Per-character styles: Lottie text documents have one style, so every run that differs
    // from the base becomes a text animator whose range selector covers exactly its characters
    // (index units). Fonts and faux styles per run can't be expressed.
    if let Some(st) = tg.get("sourceText")
        && let Value::Text(d) = &st.value
    {
        animators.extend(style_run_animators(ex, d, label));
    }
    if let Some(list) = tg.sub("animators") {
        for a in list.groups() {
            animators.push(export_animator(ex, a, &format!("{label} ▸ {}", a.name)));
        }
    }
    let mut m = TextAlign { grouping: Some(1), anchor: Some(fixed(json!([0, 0]))), ..Default::default() };
    if let Some(more) = tg.sub("moreOptions") {
        m.grouping = Some(i64::from(more.get("anchorGrouping").map(|p| p.value.as_enum()).unwrap_or(0)) + 1);
        if let Some(p) = more.get("groupingAlignment") {
            m.anchor = Some(export_prop(ex, p, &anim::arr2, &format!("{label} ▸ Grouping Alignment")));
        }
    }
    if tg.sub("pathOptions").and_then(|p| p.get("path")).is_some_and(|p| p.value.as_enum() > 0) {
        ex.warn(format!("{label}: text on a path is not exported"));
    }
    TextData {
        documents: Some(TextDocs { keys: Some(docs), ..Default::default() }),
        ranges: Some(animators),
        alignment: Some(m),
        follow_path: Some(TextFollowPath::default()),
        ..Default::default()
    }
}

/// Text animators reproducing the style runs of `d` (fill, stroke, stroke width, size,
/// tracking, baseline shift) over their character ranges.
fn style_run_animators(ex: &mut Ex, d: &TextDoc, label: &str) -> Vec<TextRange> {
    let base = d.base_style();
    let mut out = vec![];
    let mut start = 0usize;
    let mut warned = false;
    for run in d.runs() {
        let (a, b) = (start, start + run.len);
        start = b;
        let s = &run.style;
        if *s == base || run.len == 0 {
            continue;
        }
        let k = fixed;
        let mut props = TextStyle::default();
        if s.apply_fill && s.fill != base.fill {
            props.fill = Some(k(json!([s.fill[0] as f64, s.fill[1] as f64, s.fill[2] as f64, 1.0])));
        }
        if s.apply_stroke && s.stroke != base.stroke {
            props.stroke = Some(k(json!([s.stroke[0] as f64, s.stroke[1] as f64, s.stroke[2] as f64, 1.0])));
        }
        if (s.stroke_width - base.stroke_width).abs() > 1e-9 {
            props.stroke_width = Some(k(json!(s.stroke_width - base.stroke_width)));
        }
        if (s.size - base.size).abs() > 1e-9 && base.size > 0.0 {
            let p = s.size / base.size * 100.0;
            props.scale = Some(k(json!([p, p, 100.0])));
        }
        if (s.tracking - base.tracking).abs() > 1e-9 {
            // Lottie tracking is in 1/1000 em like the Character panel's.
            props.tracking = Some(k(json!(s.tracking - base.tracking)));
        }
        if (s.baseline_shift - base.baseline_shift).abs() > 1e-9 {
            props.position = Some(k(json!([0.0, -(s.baseline_shift - base.baseline_shift), 0.0])));
        }
        let lost = s.font != base.font || s.style != base.style || s.faux_bold != base.faux_bold || s.faux_italic != base.faux_italic;
        if lost && !warned {
            ex.warn(format!("{label}: per-character fonts and faux styles are not part of Lottie text (base font used)"));
            warned = true;
        }
        if props == TextStyle::default() {
            continue;
        }
        let mut sel = TextSelector {
            kind: Some(0),
            ease_high: Some(k(json!(0))),
            ease_low: Some(k(json!(0))),
            amount: Some(k(json!(100))),
            based_on: Some(1),
            randomize: Some(0),
            shape: Some(1),
            units: Some(2),
            start: Some(k(json!(a))),
            end: Some(k(json!(b))),
            offset: Some(k(json!(0))),
            ..Default::default()
        };
        sel.extra.insert("m".into(), k(json!(1)).0);
        out.push(TextRange { name: Some(format!("Style {a}–{b}")), selector: Some(sel), style: Some(props), ..Default::default() });
    }
    out
}

fn export_animator(ex: &mut Ex, a: &PropGroup, label: &str) -> TextRange {
    let mut props = TextStyle::default();
    if let Some(pg) = a.sub("properties") {
        for p in pg.props() {
            match ANIM_PROPS.iter().find(|(_, _, m)| *m == p.match_id) {
                Some((key, _, _)) => {
                    let v = export_prop(ex, p, &conv_for(&p.value), &format!("{label} ▸ {}", p.name));
                    style_set(&mut props, key, v);
                }
                None if matches!(p.match_id.as_str(), "trackingType") => {}
                None => ex.warn(format!("{label} ▸ {}: text animator property not supported by Lottie", p.name)),
            }
        }
    }
    let mut sel = TextSelector { kind: Some(0), ..Default::default() };
    let sels: Vec<&PropGroup> = a.sub("selectors").map(|s| s.groups().collect()).unwrap_or_default();
    if sels.len() > 1 {
        ex.warn(format!("{label}: only the first selector is exported (Lottie animators have one)"));
    }
    if let Some(s) = sels.first() {
        if s.match_id != "rangeSelector" {
            ex.warn(format!("{label}: {} is not supported by Lottie", s.name));
        } else {
            let mut prop = |m: &str| s.get(m).map(|p| export_prop(ex, p, &anim::num, &format!("{label} ▸ {}", p.name)));
            (sel.start, sel.end, sel.offset) = (prop("start"), prop("end"), prop("offset"));
            if let Some(adv) = s.sub("advanced") {
                let e = |m: &str| i64::from(adv.get(m).map(|p| p.value.as_enum()).unwrap_or(0)) + 1;
                sel.units = Some(e("units"));
                sel.based_on = Some(e("basedOn"));
                sel.extra.insert("m".into(), json!({"a": 0, "k": e("mode")}));
                sel.shape = Some(e("shape"));
                sel.randomize = Some(i64::from(adv.get("randomize").is_some_and(|p| p.value.as_bool())));
                let mut prop = |m: &str| adv.get(m).map(|p| export_prop(ex, p, &anim::num, &format!("{label} ▸ {}", p.name)));
                (sel.amount, sel.smoothness, sel.ease_high, sel.ease_low) = (prop("amount"), prop("smoothness"), prop("easeHigh"), prop("easeLow"));
            }
        }
    }
    TextRange { name: Some(a.name.clone()), selector: Some(sel), style: Some(props), ..Default::default() }
}

/// Import Lottie text data into a text layer's `text` group.
pub(crate) fn import_text(im: &mut Im, ids: &mut Ids, t: &TextData, tg: &mut PropGroup, fonts: &[(String, String, String)]) {
    if let Some(list) = t.documents.as_ref().and_then(|d| d.keys.as_deref())
        && let Some(st) = tg.get_mut("sourceText")
    {
        let docs: Vec<(f64, TextDoc)> = list.iter().filter_map(|k| Some((k.time.unwrap_or(0.0), doc_from(k.document.as_ref()?, fonts)))).collect();
        if let Some((_, first)) = docs.first() {
            st.value = Value::Text(Box::new(first.clone()));
        }
        if docs.len() > 1 {
            // Moved into the keys, not cloned: the documents are not kept twice.
            st.keys = docs.into_iter().map(|(f, d)| effectcraft_keyframe::Keyframe::new(im.tb.tick(f), Value::Text(Box::new(d))).hold()).collect();
        }
    }
    if let Some(m) = &t.alignment
        && let Some(more) = tg.sub_mut("moreOptions")
    {
        let grouping = m.grouping.or_else(|| m.extra.get("g").and_then(Json::as_i64)).and_then(|g| u64::try_from(g).ok());
        if let (Some(g), Some(p)) = (grouping, more.get_mut("anchorGrouping")) {
            p.value = Value::Enum((g.max(1) - 1).min(3) as u32);
        }
        if let (Some(a), Some(p)) = (&m.anchor, more.get_mut("groupingAlignment")) {
            import_prop(im, &a.0, p, &anim::to_vec2);
        }
    }
    let Some(list) = &t.ranges else { return };
    let mut animators = vec![];
    for (i, a) in list.iter().enumerate() {
        let name = a.name.clone().unwrap_or(format!("Animator {}", i + 1));
        let style = a.style.clone().unwrap_or_default();
        let mut kinds: Vec<&str> = vec![];
        for (key, kind, _) in ANIM_PROPS {
            if style_get(&style, key).is_some() && !kinds.contains(kind) {
                kinds.push(kind);
            }
        }
        let props: Vec<_> = kinds.iter().flat_map(|k| build::text_anim_props(ids, k, false)).collect();
        let mut g = build::text_animator(ids, &name, props);
        if let Some(pg) = g.sub_mut("properties") {
            for (key, _, m) in ANIM_PROPS {
                let (Some(j), Some(p)) = (style_get(&style, key), pg.get_mut(m)) else { continue };
                let conv: Box<dyn Fn(&Json) -> Option<Value>> = match &p.value {
                    Value::Vec3(d) => {
                        let z = d[2];
                        Box::new(move |v| anim::to_vec3(v, z))
                    }
                    Value::Vec2(_) => Box::new(anim::to_vec2),
                    Value::Color(_) => Box::new(anim::to_color),
                    _ => Box::new(anim::to_scalar),
                };
                import_prop(im, &j, p, &*conv);
            }
        }
        if let Some(s) = &a.selector
            && let Some(sel) = g.sub_mut("selectors").and_then(|s| s.groups_mut_first())
        {
            for (m, j) in [("start", &s.start), ("end", &s.end), ("offset", &s.offset)] {
                if let (Some(j), Some(p)) = (j, sel.get_mut(m)) {
                    import_prop(im, &j.0, p, &anim::to_scalar);
                }
            }
            if let Some(adv) = sel.sub_mut("advanced") {
                let mut en = |m: &str, field: Option<i64>, key: &str, max: u32| {
                    let v = field.map(|f| f as f64).or_else(|| {
                        let v = s.extra.get(key)?;
                        let v = if v.is_object() { v.get("k")? } else { v };
                        anim::nums(v).and_then(|n| n.first().copied())
                    });
                    if let (Some(v), Some(p)) = (v, adv.get_mut(m)) {
                        p.value = Value::Enum((((v.round() as i64).saturating_sub(1)).max(0) as u32).min(max));
                    }
                };
                en("units", s.units, "r", 1);
                en("basedOn", s.based_on, "b", 3);
                en("mode", None, "m", 5);
                en("shape", s.shape, "sh", 5);
                if let (Some(rn), Some(p)) = (s.randomize.and_then(|r| u64::try_from(r).ok()), adv.get_mut("randomize")) {
                    p.value = Value::Bool(rn != 0);
                }
                for (m, j) in [("amount", &s.amount), ("smoothness", &s.smoothness), ("easeHigh", &s.ease_high), ("easeLow", &s.ease_low)] {
                    if let (Some(j), Some(p)) = (j, adv.get_mut(m)) {
                        import_prop(im, &j.0, p, &anim::to_scalar);
                    }
                }
            }
        }
        animators.push(g);
    }
    if let Some(list) = tg.sub_mut("animators") {
        list.children.extend(animators.into_iter().map(Node::Group));
    }
}

trait FirstGroup {
    fn groups_mut_first(&mut self) -> Option<&mut PropGroup>;
}

impl FirstGroup for PropGroup {
    fn groups_mut_first(&mut self) -> Option<&mut PropGroup> {
        self.children.iter_mut().find_map(Node::as_group_mut)
    }
}

/// Text converted to glyph shapes: one group with every glyph outline (at layer time 0's
/// Source Text), a fill and a stroke, as Lottie shape items.
pub(crate) fn glyph_shapes(ex: &mut Ex, layer: &Layer) -> Vec<Shape> {
    let Some(tg) = layer.props.sub("text") else { return vec![] };
    let Some(st) = tg.get("sourceText") else { return vec![] };
    let Value::Text(doc) = st.value_at(Tick::ZERO) else { return vec![] };
    if st.keys.len() > 1 || tg.sub("animators").is_some_and(|a| !a.children.is_empty()) {
        ex.warn(format!("{}: text exported as static glyph shapes (Source Text keys and animators are baked at time 0)", layer.name));
    }
    let lay = effectcraft_text::layout_doc(&doc);
    let mut items = vec![];
    for g in &lay.glyphs {
        let p = kurbo::Affine::translate((g.origin.x, g.origin.y)) * g.path.clone();
        for sp in effectcraft_path::from_kurbo(&p) {
            let path = Path { direction: Some(1), path: Some(fixed(shapes::shape_json(&sp))), closed: None };
            items.push(Shape { name: Some(g.ch.to_string()), kind: ShapeKind::Path(path), ..Default::default() });
        }
    }
    let col = |c: [f32; 4]| fixed(json!([c[0] as f64, c[1] as f64, c[2] as f64, 1.0]));
    let stroke = (doc.apply_stroke && doc.stroke_width > 0.0).then(|| Shape {
        name: Some("Stroke".into()),
        kind: ShapeKind::Stroke(craft_lottie::model::Stroke {
            color: Some(col(doc.stroke)),
            opacity: Some(fixed(json!(100))),
            width: Some(fixed(json!(doc.stroke_width))),
            cap: Some(2),
            join: Some(2),
            miter_limit: Some(4.0),
            ..Default::default()
        }),
        ..Default::default()
    });
    let fill = doc.apply_fill.then(|| Shape {
        name: Some("Fill".into()),
        kind: ShapeKind::Fill(craft_lottie::model::Fill { color: Some(col(doc.fill)), opacity: Some(fixed(json!(100))), rule: Some(1) }),
        ..Default::default()
    });
    // Items above draw on top: stroke over fill puts the stroke first.
    if doc.stroke_over_fill {
        items.extend(stroke);
        items.extend(fill);
    } else {
        items.extend(fill);
        items.extend(stroke);
    }
    let n = items.len() + 1;
    let tr = craft_lottie::model::Transform {
        anchor: Some(fixed(json!([0, 0]))),
        position: Some(fixed(json!([0, 0]))),
        scale: Some(fixed(json!([100, 100]))),
        rotation: Some(fixed(json!(0))),
        opacity: Some(fixed(json!(100))),
        ..Default::default()
    };
    items.push(Shape { kind: ShapeKind::Transform(Box::new(tr)), ..Default::default() });
    let group = craft_lottie::model::Group { count: Some(n as f64), items: Some(items) };
    vec![Shape { name: Some(layer.name.clone()), kind: ShapeKind::Group(group), ..Default::default() }]
}

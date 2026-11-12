//! Text layers ↔ Lottie text data (`t`: document keys `d`, animators `a`, more options `m`),
//! and text converted to glyph shapes.

use effectcraft_keyframe::{Justify, TextDoc, Value};
use effectcraft_project::build::{self, Ids};
use effectcraft_project::{Layer, Node, PropGroup};
use effectcraft_time::Tick;
use serde_json::{Map, Value as Json, json};

use crate::anim::{self, Ex, Im, export_prop, import_prop};
use crate::shapes;

const JUSTIFY: [Justify; 7] =
    [Justify::Left, Justify::Right, Justify::Center, Justify::JustifyLastLeft, Justify::JustifyLastRight, Justify::JustifyLastCenter, Justify::JustifyAll];

/// Font key (`fName`) of a family/style.
pub(crate) fn font_name(family: &str, style: &str) -> String {
    format!("{}-{}", family.replace(' ', ""), style.replace(' ', ""))
}

fn rgb(c: [f32; 4]) -> Json {
    json!([c[0] as f64, c[1] as f64, c[2] as f64])
}

fn doc_json(d: &TextDoc) -> Json {
    let mut o = json!({
        "t": d.text.replace("\r\n", "\r").replace('\n', "\r"),
        "f": font_name(&d.font, &d.style),
        "s": d.size,
        "j": JUSTIFY.iter().position(|j| *j == d.justify).unwrap_or(0),
        "tr": d.tracking,
        "lh": d.leading.unwrap_or(d.size * 1.2),
        "ls": d.baseline_shift,
        "of": d.stroke_over_fill,
    });
    if d.apply_fill {
        o["fc"] = rgb(d.fill);
    }
    if d.apply_stroke && d.stroke_width > 0.0 {
        o["sc"] = rgb(d.stroke);
        o["sw"] = json!(d.stroke_width);
    }
    if d.all_caps {
        o["ca"] = json!(1);
    } else if d.small_caps {
        o["ca"] = json!(2);
    }
    if let Some(b) = d.box_size {
        o["sz"] = json!(b);
        o["ps"] = json!(d.box_pos);
    }
    o
}

fn doc_from(j: &Json, fonts: &[(String, String, String)]) -> TextDoc {
    let mut d = TextDoc::default();
    let f = |k: &str| j.get(k).and_then(Json::as_f64);
    if let Some(t) = j.get("t").and_then(Json::as_str) {
        d.text = t.replace(['\r', '\u{3}'], "\n");
    }
    if let Some(name) = j.get("f").and_then(Json::as_str) {
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
    if let Some(s) = f("s") {
        d.size = s;
    }
    let col = |k: &str| j.get(k).and_then(anim::to_color).map(|v| v.as_color());
    match col("fc") {
        Some(c) => d.fill = c,
        None => d.apply_fill = false,
    }
    if let Some(c) = col("sc") {
        d.stroke = c;
        d.apply_stroke = true;
        d.stroke_width = f("sw").unwrap_or(1.0);
    }
    if let Some(of) = j.get("of").and_then(Json::as_bool) {
        d.stroke_over_fill = of;
    }
    if let Some(i) = j.get("j").and_then(Json::as_u64) {
        d.justify = JUSTIFY.get(i as usize).copied().unwrap_or_default();
    }
    d.tracking = f("tr").unwrap_or(0.0);
    if let Some(lh) = f("lh")
        && (lh - d.size * 1.2).abs() > 1e-6
    {
        d.leading = Some(lh);
    }
    d.baseline_shift = f("ls").unwrap_or(0.0);
    match j.get("ca").and_then(Json::as_u64) {
        Some(1) => d.all_caps = true,
        Some(2) => d.small_caps = true,
        _ => {}
    }
    if let Some(sz) = j.get("sz").and_then(anim::nums).filter(|v| v.len() >= 2) {
        d.box_size = Some([sz[0], sz[1]]);
        if let Some(ps) = j.get("ps").and_then(anim::nums).filter(|v| v.len() >= 2) {
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

/// Export a text layer's `t` object. Fonts used are added to `fonts`.
pub(crate) fn export_text(ex: &mut Ex, layer: &Layer, fonts: &mut Vec<Json>) -> Json {
    let Some(tg) = layer.props.sub("text") else { return json!({}) };
    let label = &layer.name;
    let mut docs = vec![];
    let mut add_font = |d: &TextDoc| {
        let n = font_name(&d.font, &d.style);
        if !fonts.iter().any(|f| f["fName"] == json!(n)) {
            fonts.push(json!({"fName": n, "fFamily": d.font, "fStyle": d.style, "ascent": 75}));
        }
    };
    if let Some(st) = tg.get("sourceText") {
        if st.keys.is_empty() {
            if let Value::Text(d) = &st.value {
                add_font(d);
                docs.push(json!({"t": 0, "s": doc_json(d)}));
            }
        } else {
            for k in &st.keys {
                if let Value::Text(d) = &k.value {
                    add_font(d);
                    docs.push(json!({"t": ex.tb.frame(k.time), "s": doc_json(d)}));
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
    let mut m = json!({"g": 1, "a": {"a": 0, "k": [0, 0]}});
    if let Some(more) = tg.sub("moreOptions") {
        m["g"] = json!(more.get("anchorGrouping").map(|p| p.value.as_enum()).unwrap_or(0) + 1);
        if let Some(p) = more.get("groupingAlignment") {
            m["a"] = export_prop(ex, p, &anim::arr2, &format!("{label} ▸ Grouping Alignment"));
        }
    }
    if tg.sub("pathOptions").and_then(|p| p.get("path")).is_some_and(|p| p.value.as_enum() > 0) {
        ex.warn(format!("{label}: text on a path is not exported"));
    }
    json!({"d": {"k": docs}, "a": animators, "m": m, "p": {}})
}

/// Text animators reproducing the style runs of `d` (fill, stroke, stroke width, size,
/// tracking, baseline shift) over their character ranges.
fn style_run_animators(ex: &mut Ex, d: &TextDoc, label: &str) -> Vec<Json> {
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
        let k = |v: Json| json!({"a": 0, "k": v});
        let mut props = Map::new();
        if s.apply_fill && s.fill != base.fill {
            props.insert("fc".into(), k(json!([s.fill[0] as f64, s.fill[1] as f64, s.fill[2] as f64, 1.0])));
        }
        if s.apply_stroke && s.stroke != base.stroke {
            props.insert("sc".into(), k(json!([s.stroke[0] as f64, s.stroke[1] as f64, s.stroke[2] as f64, 1.0])));
        }
        if (s.stroke_width - base.stroke_width).abs() > 1e-9 {
            props.insert("sw".into(), k(json!(s.stroke_width - base.stroke_width)));
        }
        if (s.size - base.size).abs() > 1e-9 && base.size > 0.0 {
            let p = s.size / base.size * 100.0;
            props.insert("s".into(), k(json!([p, p, 100.0])));
        }
        if (s.tracking - base.tracking).abs() > 1e-9 {
            // Lottie tracking is in 1/1000 em like the Character panel's.
            props.insert("t".into(), k(json!(s.tracking - base.tracking)));
        }
        if (s.baseline_shift - base.baseline_shift).abs() > 1e-9 {
            props.insert("p".into(), k(json!([0.0, -(s.baseline_shift - base.baseline_shift), 0.0])));
        }
        let lost = s.font != base.font || s.style != base.style || s.faux_bold != base.faux_bold || s.faux_italic != base.faux_italic;
        if lost && !warned {
            ex.warn(format!("{label}: per-character fonts and faux styles are not part of Lottie text (base font used)"));
            warned = true;
        }
        if props.is_empty() {
            continue;
        }
        out.push(json!({
            "nm": format!("Style {a}–{b}"),
            "s": {
                "t": 0, "xe": k(json!(0)), "ne": k(json!(0)), "a": k(json!(100)), "b": 1, "rn": 0, "sh": 1,
                "r": 2, "m": k(json!(1)), "s": k(json!(a)), "e": k(json!(b)), "o": k(json!(0)),
            },
            "a": Json::Object(props),
        }));
    }
    out
}

fn export_animator(ex: &mut Ex, a: &PropGroup, label: &str) -> Json {
    let mut props = Map::new();
    if let Some(pg) = a.sub("properties") {
        for p in pg.props() {
            match ANIM_PROPS.iter().find(|(_, _, m)| *m == p.match_id) {
                Some((key, _, _)) => {
                    let v = export_prop(ex, p, &conv_for(&p.value), &format!("{label} ▸ {}", p.name));
                    props.insert((*key).into(), v);
                }
                None if matches!(p.match_id.as_str(), "trackingType") => {}
                None => ex.warn(format!("{label} ▸ {}: text animator property not supported by Lottie", p.name)),
            }
        }
    }
    let mut sel = json!({"t": 0});
    let sels: Vec<&PropGroup> = a.sub("selectors").map(|s| s.groups().collect()).unwrap_or_default();
    if sels.len() > 1 {
        ex.warn(format!("{label}: only the first selector is exported (Lottie animators have one)"));
    }
    if let Some(s) = sels.first() {
        if s.match_id != "rangeSelector" {
            ex.warn(format!("{label}: {} is not supported by Lottie", s.name));
        } else {
            for (k, m) in [("s", "start"), ("e", "end"), ("o", "offset")] {
                if let Some(p) = s.get(m) {
                    sel[k] = export_prop(ex, p, &anim::num, &format!("{label} ▸ {}", p.name));
                }
            }
            if let Some(adv) = s.sub("advanced") {
                let e = |m: &str| adv.get(m).map(|p| p.value.as_enum()).unwrap_or(0) + 1;
                sel["r"] = json!(e("units"));
                sel["b"] = json!(e("basedOn"));
                sel["m"] = json!({"a": 0, "k": e("mode")});
                sel["sh"] = json!(e("shape"));
                sel["rn"] = json!(adv.get("randomize").is_some_and(|p| p.value.as_bool()) as u8);
                for (k, m) in [("a", "amount"), ("sm", "smoothness"), ("xe", "easeHigh"), ("ne", "easeLow")] {
                    if let Some(p) = adv.get(m) {
                        sel[k] = export_prop(ex, p, &anim::num, &format!("{label} ▸ {}", p.name));
                    }
                }
            }
        }
    }
    json!({"nm": a.name, "s": sel, "a": Json::Object(props)})
}

/// Import Lottie text data into a text layer's `text` group.
pub(crate) fn import_text(im: &mut Im, ids: &mut Ids, t: &Json, tg: &mut PropGroup, fonts: &[(String, String, String)]) {
    if let Some(list) = t.get("d").and_then(|d| d.get("k")).and_then(Json::as_array)
        && let Some(st) = tg.get_mut("sourceText")
    {
        let docs: Vec<(f64, TextDoc)> =
            list.iter().filter_map(|k| Some((k.get("t").and_then(Json::as_f64).unwrap_or(0.0), doc_from(k.get("s")?, fonts)))).collect();
        if let Some((_, first)) = docs.first() {
            st.value = Value::Text(Box::new(first.clone()));
        }
        if docs.len() > 1 {
            // Moved into the keys, not cloned: the documents are not kept twice.
            st.keys = docs.into_iter().map(|(f, d)| effectcraft_keyframe::Keyframe::new(im.tb.tick(f), Value::Text(Box::new(d))).hold()).collect();
        }
    }
    if let Some(m) = t.get("m")
        && let Some(more) = tg.sub_mut("moreOptions")
    {
        if let (Some(g), Some(p)) = (m.get("g").and_then(Json::as_u64), more.get_mut("anchorGrouping")) {
            p.value = Value::Enum((g.max(1) - 1).min(3) as u32);
        }
        if let (Some(a), Some(p)) = (m.get("a"), more.get_mut("groupingAlignment")) {
            import_prop(im, a, p, &anim::to_vec2);
        }
    }
    let Some(list) = t.get("a").and_then(Json::as_array) else { return };
    let mut animators = vec![];
    for (i, a) in list.iter().enumerate() {
        let name = a.get("nm").and_then(Json::as_str).map(str::to_string).unwrap_or(format!("Animator {}", i + 1));
        let props_j = a.get("a").cloned().unwrap_or(json!({}));
        let mut kinds: Vec<&str> = vec![];
        for (key, kind, _) in ANIM_PROPS {
            if props_j.get(*key).is_some() && !kinds.contains(kind) {
                kinds.push(kind);
            }
        }
        let props: Vec<_> = kinds.iter().flat_map(|k| build::text_anim_props(ids, k, false)).collect();
        let mut g = build::text_animator(ids, &name, props);
        if let Some(pg) = g.sub_mut("properties") {
            for (key, _, m) in ANIM_PROPS {
                let (Some(j), Some(p)) = (props_j.get(*key), pg.get_mut(m)) else { continue };
                let conv: Box<dyn Fn(&Json) -> Option<Value>> = match &p.value {
                    Value::Vec3(d) => {
                        let z = d[2];
                        Box::new(move |v| anim::to_vec3(v, z))
                    }
                    Value::Vec2(_) => Box::new(anim::to_vec2),
                    Value::Color(_) => Box::new(anim::to_color),
                    _ => Box::new(anim::to_scalar),
                };
                import_prop(im, j, p, &*conv);
            }
        }
        if let Some(s) = a.get("s")
            && let Some(sel) = g.sub_mut("selectors").and_then(|s| s.groups_mut_first())
        {
            for (k, m) in [("s", "start"), ("e", "end"), ("o", "offset")] {
                if let (Some(j), Some(p)) = (s.get(k), sel.get_mut(m)) {
                    import_prop(im, j, p, &anim::to_scalar);
                }
            }
            if let Some(adv) = sel.sub_mut("advanced") {
                let mut en = |m: &str, key: &str, max: u32| {
                    let v = s.get(key).and_then(|v| if v.is_object() { v.get("k") } else { Some(v) }).and_then(anim::nums).and_then(|n| n.first().copied());
                    if let (Some(v), Some(p)) = (v, adv.get_mut(m)) {
                        p.value = Value::Enum((((v.round() as i64).saturating_sub(1)).max(0) as u32).min(max));
                    }
                };
                en("units", "r", 1);
                en("basedOn", "b", 3);
                en("mode", "m", 5);
                en("shape", "sh", 5);
                if let (Some(rn), Some(p)) = (s.get("rn").and_then(Json::as_u64), adv.get_mut("randomize")) {
                    p.value = Value::Bool(rn != 0);
                }
                for (k, m) in [("a", "amount"), ("sm", "smoothness"), ("xe", "easeHigh"), ("ne", "easeLow")] {
                    if let (Some(j), Some(p)) = (s.get(k), adv.get_mut(m)) {
                        import_prop(im, j, p, &anim::to_scalar);
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
pub(crate) fn glyph_shapes(ex: &mut Ex, layer: &Layer) -> Vec<Json> {
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
            items.push(json!({"ty": "sh", "nm": g.ch.to_string(), "d": 1, "ks": {"a": 0, "k": shapes::shape_json(&sp)}}));
        }
    }
    let col = |c: [f32; 4]| json!({"a": 0, "k": [c[0] as f64, c[1] as f64, c[2] as f64, 1.0]});
    let stroke = (doc.apply_stroke && doc.stroke_width > 0.0).then(
        || json!({"ty": "st", "nm": "Stroke", "c": col(doc.stroke), "o": {"a": 0, "k": 100}, "w": {"a": 0, "k": doc.stroke_width}, "lc": 2, "lj": 2, "ml": 4}),
    );
    let fill = doc.apply_fill.then(|| json!({"ty": "fl", "nm": "Fill", "c": col(doc.fill), "o": {"a": 0, "k": 100}, "r": 1}));
    // Items above draw on top: stroke over fill puts the stroke first.
    if doc.stroke_over_fill {
        items.extend(stroke);
        items.extend(fill);
    } else {
        items.extend(fill);
        items.extend(stroke);
    }
    let n = items.len() + 1;
    items.push(json!({"ty": "tr", "a": {"a": 0, "k": [0, 0]}, "p": {"a": 0, "k": [0, 0]}, "s": {"a": 0, "k": [100, 100]}, "r": {"a": 0, "k": 0}, "o": {"a": 0, "k": 100}}));
    vec![json!({"ty": "gr", "nm": layer.name, "np": n, "it": items})]
}

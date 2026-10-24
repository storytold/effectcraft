//! Shape layer contents ↔ Lottie shape items (`gr`, `rc`, `el`, `sr`, `sh`, `fl`, `st`, `gf`,
//! `gs`, `tm`, `rp`, `rd`, `op`, `pb`, `tw`, `zz`, `mm`, `tr`).

use effectcraft_color::BlendMode;
use effectcraft_keyframe::{Gradient, ShapePath, Value};
use effectcraft_project::build::{self, EXTRA_DASHES, Ids};
use effectcraft_project::{Node, ParamUi, PropGroup};
use serde_json::{Map, Value as Json, json};

use crate::anim::{self, Ex, Im, export_prop, import_prop};

// ---------------------------------------------------------------- shared value converters

pub(crate) fn shape_json(p: &ShapePath) -> Json {
    let pts = |v: &[[f64; 2]]| v.iter().map(|q| json!([r(q[0]), r(q[1])])).collect::<Vec<_>>();
    json!({"c": p.closed, "v": pts(&p.vertices), "i": pts(&p.in_tangents), "o": pts(&p.out_tangents)})
}

fn r(v: f64) -> f64 {
    (v * 1e6).round() / 1e6
}

pub(crate) fn path_value(v: &Value) -> Json {
    match v {
        Value::Path(p) => shape_json(p),
        _ => shape_json(&ShapePath::default()),
    }
}

pub(crate) fn to_path(j: &Json) -> Option<Value> {
    let o = match j {
        Json::Array(a) => a.first()?,
        o => o,
    };
    let pts = |k: &str| -> Vec<[f64; 2]> {
        o.get(k)
            .and_then(Json::as_array)
            .map(|a| a.iter().map(|p| anim::nums(p).map(|v| [*v.first().unwrap_or(&0.0), *v.get(1).unwrap_or(&0.0)]).unwrap_or([0.0; 2])).collect())
            .unwrap_or_default()
    };
    let vertices = pts("v");
    let n = vertices.len();
    let fit = |mut v: Vec<[f64; 2]>| {
        v.resize(n, [0.0; 2]);
        v
    };
    Some(Value::Path(ShapePath {
        in_tangents: fit(pts("i")),
        out_tangents: fit(pts("o")),
        vertices,
        closed: o.get("c").and_then(Json::as_bool).unwrap_or(false),
        feather: Vec::new(),
    }))
}

/// Gradient as Lottie's flat stop list: `[pos, r, g, b]…` then `[pos, alpha]…`.
pub(crate) fn gradient_flat(v: &Value) -> Json {
    let g = match v {
        Value::Gradient(g) => g.clone(),
        _ => Gradient::default(),
    };
    let mut out = vec![];
    for (p, c) in &g.colors {
        out.extend([r(*p), r(c[0] as f64), r(c[1] as f64), r(c[2] as f64)]);
    }
    let plain = g.opacities.iter().all(|(_, a)| (*a - 1.0).abs() < 1e-6);
    if !plain {
        for (p, a) in &g.opacities {
            out.extend([r(*p), r(*a as f64)]);
        }
    }
    json!(out)
}

pub(crate) fn to_gradient(j: &Json, ncolors: usize) -> Option<Value> {
    let v = anim::nums(j)?;
    let n = ncolors.min(v.len() / 4);
    let colors: Vec<(f64, [f32; 4])> = (0..n).map(|i| (v[i * 4], [v[i * 4 + 1] as f32, v[i * 4 + 2] as f32, v[i * 4 + 3] as f32, 1.0])).collect();
    let rest = &v[n * 4..];
    let mut opacities: Vec<(f64, f32)> = rest.chunks(2).filter(|c| c.len() == 2).map(|c| (c[0], c[1] as f32)).collect();
    if opacities.is_empty() {
        opacities = vec![(0.0, 1.0), (1.0, 1.0)];
    }
    Some(Value::Gradient(Gradient { colors, opacities }))
}

fn blend_index(m: BlendMode) -> u32 {
    BlendMode::ALL.iter().position(|b| *b == m).unwrap_or(0) as u32
}

// ---------------------------------------------------------------- export

struct Out<'a, 'b> {
    ex: &'a mut Ex,
    g: &'b PropGroup,
    o: Map<String, Json>,
    label: String,
}

impl Out<'_, '_> {
    fn put(&mut self, key: &str, m: &str, conv: &dyn Fn(&Value) -> Json) {
        if let Some(p) = self.g.get(m) {
            let v = export_prop(self.ex, p, conv, &format!("{} ▸ {}", self.label, p.name));
            self.o.insert(key.into(), v);
        }
    }
    fn raw(&mut self, key: &str, v: Json) {
        self.o.insert(key.into(), v);
    }
    fn e(&self, m: &str) -> u32 {
        self.g.get(m).map(|p| p.value.as_enum()).unwrap_or(0)
    }
    fn f(&self, m: &str) -> f64 {
        self.g.get(m).map(|p| p.value.as_f64()).unwrap_or(0.0)
    }
    fn animated(&self, m: &str) -> bool {
        self.g.get(m).is_some_and(|p| p.is_animated() || p.has_expression())
    }
    fn blend(&mut self) {
        let i = self.e("blend");
        let m = BlendMode::ALL.get(i as usize).copied().unwrap_or_default();
        match crate::blend_to_lottie(m) {
            Some(bm) => self.raw("bm", json!(bm)),
            None => {
                self.ex.warn(format!("{}: blend mode {m:?} has no Lottie equivalent (Normal used)", self.label));
                self.raw("bm", json!(0));
            }
        }
    }
    fn done(self) -> Json {
        Json::Object(self.o)
    }
}

/// Stroke fields shared by `st` and `gs`.
fn stroke_fields(o: &mut Out) {
    o.put("w", "width", &anim::num);
    o.raw("lc", json!(o.e("cap") + 1));
    o.raw("lj", json!(o.e("join") + 1));
    o.raw("ml", json!(o.f("miter")));
    let used = |g: Option<&PropGroup>, m: &str| g.and_then(|g| g.get(m)).is_some_and(|p| p.is_animated() || p.value.as_f64().abs() > 1e-9);
    let (taper, wave) = (o.g.sub("taper"), o.g.sub("wave"));
    if used(taper, "startLength") || used(taper, "endLength") || used(wave, "amount") {
        let msg = format!("{}: stroke Taper and Wave have no Lottie equivalent (uniform width exported)", o.label);
        o.ex.warn(msg);
    }
    if let Some(d) = o.g.sub("dashes") {
        let dash = d.get("dash").map(|p| p.value.as_f64()).unwrap_or(0.0);
        if dash > 0.0 || d.any_animated() {
            let mut list = vec![];
            for (n, m) in [("d", "dash"), ("g", "gap"), ("d", "dash2"), ("g", "gap2"), ("d", "dash3"), ("g", "gap3"), ("o", "offset")] {
                if let Some(p) = d.get(m) {
                    let v = export_prop(o.ex, p, &anim::num, &format!("{} ▸ Dashes ▸ {}", o.label, p.name));
                    list.push(json!({"n": n, "nm": p.name, "v": v}));
                }
            }
            o.raw("d", json!(list));
        }
    }
}

fn shape_transform(ex: &mut Ex, tr: &PropGroup, label: &str) -> Json {
    let mut o = Out { ex, g: tr, o: Map::new(), label: format!("{label} ▸ Transform") };
    o.raw("ty", json!("tr"));
    o.put("a", "anchor", &anim::arr2);
    o.put("p", "position", &anim::arr2);
    o.put("s", "scale", &anim::arr2);
    o.put("r", "rotation", &anim::num);
    o.put("o", "opacity", &anim::num);
    o.put("sk", "skew", &anim::num);
    o.put("sa", "skewAxis", &anim::num);
    o.put("so", "startOpacity", &anim::num);
    o.put("eo", "endOpacity", &anim::num);
    o.done()
}

/// Export a contents group's items (first item = top, as in the timeline).
pub(crate) fn export_items(ex: &mut Ex, contents: &PropGroup, label: &str) -> Vec<Json> {
    let mut out = vec![];
    for node in &contents.children {
        let Node::Group(g) = node else { continue };
        let lbl = format!("{label} ▸ {}", g.name);
        let ty = match g.match_id.as_str() {
            "group" => "gr",
            "rect" => "rc",
            "ellipse" => "el",
            "star" => "sr",
            "path" => "sh",
            "fill" => "fl",
            "stroke" => "st",
            "gfill" => "gf",
            "gstroke" => "gs",
            "trim" => "tm",
            "repeater" => "rp",
            "round" => "rd",
            "offset" => "op",
            "pucker" => "pb",
            "twist" => "tw",
            "zigzag" => "zz",
            "merge" => "mm",
            other => {
                ex.warn(format!("{lbl}: shape item `{other}` has no Lottie equivalent (skipped)"));
                continue;
            }
        };
        let mut o = Out { ex, g, o: Map::new(), label: lbl.clone() };
        o.raw("ty", json!(ty));
        o.raw("nm", json!(g.name));
        if !g.enabled {
            o.raw("hd", json!(true));
        }
        let dir = |o: &mut Out| {
            let d = if o.e("direction") == 1 { 3 } else { 1 };
            o.raw("d", json!(d));
        };
        match ty {
            "gr" => {
                let items = g.sub("contents").map(|c| export_items(o.ex, c, &lbl)).unwrap_or_default();
                let mut items = items;
                let tr = match g.sub("transform") {
                    Some(t) => shape_transform(o.ex, t, &lbl),
                    None => {
                        json!({"ty": "tr", "a": {"a": 0, "k": [0, 0]}, "p": {"a": 0, "k": [0, 0]}, "s": {"a": 0, "k": [100, 100]}, "r": {"a": 0, "k": 0}, "o": {"a": 0, "k": 100}})
                    }
                };
                items.push(tr);
                o.raw("np", json!(items.len()));
                o.raw("it", json!(items));
                o.blend();
            }
            "rc" => {
                dir(&mut o);
                o.put("s", "size", &anim::arr2);
                o.put("p", "position", &anim::arr2);
                o.put("r", "roundness", &anim::num);
            }
            "el" => {
                dir(&mut o);
                o.put("s", "size", &anim::arr2);
                o.put("p", "position", &anim::arr2);
            }
            "sr" => {
                dir(&mut o);
                o.raw("sy", json!(o.e("type") + 1));
                o.put("pt", "points", &anim::num);
                o.put("p", "position", &anim::arr2);
                o.put("r", "rotation", &anim::num);
                o.put("ir", "innerRadius", &anim::num);
                o.put("or", "outerRadius", &anim::num);
                o.put("is", "innerRoundness", &anim::num);
                o.put("os", "outerRoundness", &anim::num);
            }
            "sh" => {
                dir(&mut o);
                o.put("ks", "path", &path_value);
            }
            "fl" => {
                o.put("c", "color", &anim::arr);
                o.put("o", "opacity", &anim::num);
                o.raw("r", json!(o.e("rule") + 1));
                o.blend();
            }
            "st" => {
                o.put("c", "color", &anim::arr);
                o.put("o", "opacity", &anim::num);
                stroke_fields(&mut o);
                o.blend();
            }
            "gf" | "gs" => {
                o.put("o", "opacity", &anim::num);
                o.put("s", "start", &anim::arr2);
                o.put("e", "end", &anim::arr2);
                o.raw("t", json!(o.e("type") + 1));
                o.put("h", "highlightLength", &anim::num);
                o.put("a", "highlightAngle", &anim::num);
                let ncolors = match g.get("colors").map(|p| &p.value) {
                    Some(Value::Gradient(gr)) => gr.colors.len(),
                    _ => 2,
                };
                if let Some(p) = g.get("colors") {
                    let k = export_prop(o.ex, p, &gradient_flat, &format!("{lbl} ▸ Colors"));
                    o.raw("g", json!({"p": ncolors, "k": k}));
                }
                if ty == "gf" {
                    o.raw("r", json!(o.e("rule") + 1));
                } else {
                    stroke_fields(&mut o);
                }
                o.blend();
            }
            "tm" => {
                o.put("s", "start", &anim::num);
                o.put("e", "end", &anim::num);
                o.put("o", "offset", &anim::num);
                o.raw("m", json!(o.e("mode") + 1));
            }
            "rp" => {
                o.put("c", "copies", &anim::num);
                o.put("o", "offset", &anim::num);
                // Lottie: 1 = Above, 2 = Below; EffectCraft: 0 = Below, 1 = Above.
                o.raw("m", json!(if o.e("composite") == 1 { 1 } else { 2 }));
                if let Some(t) = g.sub("transform") {
                    let tr = shape_transform(o.ex, t, &lbl);
                    o.raw("tr", tr);
                }
            }
            "rd" => o.put("r", "radius", &anim::num),
            "op" => {
                o.put("a", "amount", &anim::num);
                o.raw("lj", json!(o.e("join") + 1));
                o.put("ml", "miter", &anim::num);
                if (o.f("copies") - 1.0).abs() > 1e-9 || o.animated("copies") || o.f("copyOffset") != 0.0 {
                    o.ex.warn(format!("{lbl}: Offset Paths copies are not part of Lottie (one copy exported)"));
                }
            }
            "pb" => o.put("a", "amount", &anim::num),
            "tw" => {
                o.put("a", "angle", &anim::num);
                o.put("c", "center", &anim::arr2);
            }
            "zz" => {
                o.put("s", "size", &anim::num);
                o.put("r", "ridges", &anim::num);
                o.raw("pt", json!({"a": 0, "k": o.e("points") + 1}));
            }
            "mm" => o.raw("mm", json!(o.e("mode") + 1)),
            _ => {}
        }
        out.push(o.done());
    }
    out
}

// ---------------------------------------------------------------- import

fn set(im: &Im, g: &mut PropGroup, m: &str, j: Option<&Json>, conv: &dyn Fn(&Json) -> Option<Value>) {
    if let (Some(j), Some(p)) = (j, g.get_mut(m)) {
        import_prop(im, j, p, conv);
    }
}

fn set_enum(g: &mut PropGroup, m: &str, v: u32) {
    if let Some(p) = g.get_mut(m) {
        p.value = Value::Enum(v);
    }
}

fn int(j: &Json, k: &str) -> Option<i64> {
    let v = j.get(k)?;
    v.as_i64().or_else(|| v.as_f64().map(|f| f.round() as i64)).or_else(|| v.get("k").and_then(anim::nums).and_then(|n| n.first().map(|f| f.round() as i64)))
}

fn import_blend(j: &Json, g: &mut PropGroup) {
    if let Some(bm) = j.get("bm").and_then(Json::as_u64) {
        set_enum(g, "blend", blend_index(crate::blend_from_lottie(bm)));
    }
}

fn import_stroke(im: &Im, ids: &mut Ids, j: &Json, g: &mut PropGroup) {
    set(im, g, "width", j.get("w"), &anim::to_scalar);
    if let Some(lc) = int(j, "lc") {
        set_enum(g, "cap", lc.saturating_sub(1).clamp(0, 2) as u32);
    }
    if let Some(lj) = int(j, "lj") {
        set_enum(g, "join", lj.saturating_sub(1).clamp(0, 2) as u32);
    }
    if let Some(p) = g.get_mut("miter") {
        if let Some(ml) = j.get("ml").and_then(Json::as_f64) {
            p.value = Value::Scalar(ml);
        } else if let Some(ml2) = j.get("ml2") {
            import_prop(im, ml2, p, &anim::to_scalar);
        }
    }
    if let Some(list) = j.get("d").and_then(Json::as_array)
        && let Some(d) = g.sub_mut("dashes")
    {
        // The n-th `d` and `g` are the n-th Dash/Gap pair; the extra pairs are created as the Dashes "+" does.
        let (mut dashes, mut gaps) = (0, 0);
        for e in list {
            let (n, pair) = match e.get("n").and_then(Json::as_str) {
                Some("d") => (&mut dashes, 0),
                Some("g") => (&mut gaps, 1),
                Some("o") => {
                    set(im, d, "offset", e.get("v"), &anim::to_scalar);
                    continue;
                }
                _ => continue,
            };
            let k = *n;
            *n += 1;
            let m = match k {
                0 => ["dash", "gap"][pair],
                _ => {
                    let Some(&(dm, gm)) = EXTRA_DASHES.get(k - 1) else { continue };
                    if d.get(dm).is_none() {
                        let at = d.children.iter().position(|c| c.match_id() == "offset").unwrap_or(d.children.len());
                        d.children.insert(at, ids.prop(gm, &format!("Gap {}", k + 1), Value::Scalar(0.0)).with_ui(ParamUi::Pixels).into());
                        d.children.insert(at, ids.prop(dm, &format!("Dash {}", k + 1), Value::Scalar(0.0)).with_ui(ParamUi::Pixels).into());
                    }
                    [dm, gm][pair]
                }
            };
            set(im, d, m, e.get("v"), &anim::to_scalar);
        }
    }
}

fn import_shape_transform(im: &Im, j: &Json, tr: &mut PropGroup) {
    set(im, tr, "anchor", j.get("a"), &anim::to_vec2);
    set(im, tr, "position", j.get("p"), &anim::to_vec2);
    set(im, tr, "scale", j.get("s"), &anim::to_vec2);
    set(im, tr, "rotation", j.get("r"), &anim::to_scalar);
    set(im, tr, "opacity", j.get("o"), &anim::to_scalar);
    set(im, tr, "skew", j.get("sk"), &anim::to_scalar);
    set(im, tr, "skewAxis", j.get("sa"), &anim::to_scalar);
    set(im, tr, "startOpacity", j.get("so"), &anim::to_scalar);
    set(im, tr, "endOpacity", j.get("eo"), &anim::to_scalar);
}

/// Import Lottie shape items into shape contents groups.
pub(crate) fn import_items(im: &mut Im, ids: &mut Ids, items: &[Json]) -> Vec<PropGroup> {
    let mut out = vec![];
    for j in items {
        let ty = j.get("ty").and_then(Json::as_str).unwrap_or("");
        let dir = |g: &mut PropGroup| {
            if int(j, "d") == Some(3) {
                set_enum(g, "direction", 1);
            }
        };
        let mut g = match ty {
            "gr" => {
                let list = j.get("it").and_then(Json::as_array).cloned().unwrap_or_default();
                let inner: Vec<Json> = list.iter().filter(|i| i.get("ty").and_then(Json::as_str) != Some("tr")).cloned().collect();
                let children = import_items(im, ids, &inner);
                let mut g = build::shape_group(ids, "Group 1", children);
                if let Some(tr) = list.iter().find(|i| i.get("ty").and_then(Json::as_str) == Some("tr"))
                    && let Some(t) = g.sub_mut("transform")
                {
                    import_shape_transform(im, tr, t);
                }
                import_blend(j, &mut g);
                g
            }
            "rc" => {
                let mut g = build::shape_rect(ids, [100.0, 100.0], [0.0, 0.0], 0.0);
                dir(&mut g);
                set(im, &mut g, "size", j.get("s"), &anim::to_vec2);
                set(im, &mut g, "position", j.get("p"), &anim::to_vec2);
                set(im, &mut g, "roundness", j.get("r"), &anim::to_scalar);
                g
            }
            "el" => {
                let mut g = build::shape_ellipse(ids, [100.0, 100.0], [0.0, 0.0]);
                dir(&mut g);
                set(im, &mut g, "size", j.get("s"), &anim::to_vec2);
                set(im, &mut g, "position", j.get("p"), &anim::to_vec2);
                g
            }
            "sr" => {
                let star = int(j, "sy").unwrap_or(1) != 2;
                let mut g = build::shape_star(ids, star, 5.0, [0.0, 0.0], 100.0, 50.0);
                dir(&mut g);
                set(im, &mut g, "points", j.get("pt"), &anim::to_scalar);
                set(im, &mut g, "position", j.get("p"), &anim::to_vec2);
                set(im, &mut g, "rotation", j.get("r"), &anim::to_scalar);
                set(im, &mut g, "innerRadius", j.get("ir"), &anim::to_scalar);
                set(im, &mut g, "outerRadius", j.get("or"), &anim::to_scalar);
                set(im, &mut g, "innerRoundness", j.get("is"), &anim::to_scalar);
                set(im, &mut g, "outerRoundness", j.get("os"), &anim::to_scalar);
                g
            }
            "sh" => {
                let mut g = build::shape_path(ids, ShapePath::default());
                dir(&mut g);
                set(im, &mut g, "path", j.get("ks"), &to_path);
                g
            }
            "fl" => {
                let mut g = build::shape_fill(ids, [1.0, 1.0, 1.0, 1.0]);
                set(im, &mut g, "color", j.get("c"), &anim::to_color);
                set(im, &mut g, "opacity", j.get("o"), &anim::to_scalar);
                if int(j, "r") == Some(2) {
                    set_enum(&mut g, "rule", 1);
                }
                import_blend(j, &mut g);
                g
            }
            "st" => {
                let mut g = build::shape_stroke(ids, [1.0, 1.0, 1.0, 1.0], 2.0);
                set(im, &mut g, "color", j.get("c"), &anim::to_color);
                set(im, &mut g, "opacity", j.get("o"), &anim::to_scalar);
                import_stroke(im, ids, j, &mut g);
                import_blend(j, &mut g);
                g
            }
            "gf" | "gs" => {
                let radial = int(j, "t") == Some(2);
                let mut g = if ty == "gf" {
                    build::shape_gradient_fill(ids, radial, [0.0, 0.0], [100.0, 0.0], Gradient::default())
                } else {
                    build::shape_gradient_stroke(ids, radial, [0.0, 0.0], [100.0, 0.0], Gradient::default(), 2.0)
                };
                set(im, &mut g, "opacity", j.get("o"), &anim::to_scalar);
                set(im, &mut g, "start", j.get("s"), &anim::to_vec2);
                set(im, &mut g, "end", j.get("e"), &anim::to_vec2);
                set(im, &mut g, "highlightLength", j.get("h"), &anim::to_scalar);
                set(im, &mut g, "highlightAngle", j.get("a"), &anim::to_scalar);
                if let Some(gj) = j.get("g") {
                    let n = gj.get("p").and_then(Json::as_u64).unwrap_or(2) as usize;
                    if let Some(k) = gj.get("k") {
                        set(im, &mut g, "colors", Some(k), &move |v| to_gradient(v, n));
                    }
                }
                if ty == "gf" {
                    if int(j, "r") == Some(2) {
                        set_enum(&mut g, "rule", 1);
                    }
                } else {
                    import_stroke(im, ids, j, &mut g);
                }
                import_blend(j, &mut g);
                g
            }
            "tm" => {
                let mut g = build::shape_trim(ids, 0.0, 100.0, 0.0);
                set(im, &mut g, "start", j.get("s"), &anim::to_scalar);
                set(im, &mut g, "end", j.get("e"), &anim::to_scalar);
                set(im, &mut g, "offset", j.get("o"), &anim::to_scalar);
                if int(j, "m") == Some(2) {
                    set_enum(&mut g, "mode", 1);
                }
                g
            }
            "rp" => {
                let mut g = build::shape_repeater(ids, 3.0, [100.0, 0.0]);
                set(im, &mut g, "copies", j.get("c"), &anim::to_scalar);
                set(im, &mut g, "offset", j.get("o"), &anim::to_scalar);
                set_enum(&mut g, "composite", if int(j, "m") == Some(1) { 1 } else { 0 });
                if let (Some(tr), Some(t)) = (j.get("tr"), g.sub_mut("transform")) {
                    import_shape_transform(im, tr, t);
                }
                g
            }
            "rd" | "op" | "pb" | "tw" | "zz" | "mm" => {
                let kind = match ty {
                    "rd" => "round",
                    "op" => "offset",
                    "pb" => "pucker",
                    "tw" => "twist",
                    "zz" => "zigzag",
                    _ => "merge",
                };
                let Some(mut g) = build::shape_simple_op(ids, kind) else { continue };
                match ty {
                    "rd" => set(im, &mut g, "radius", j.get("r"), &anim::to_scalar),
                    "op" => {
                        set(im, &mut g, "amount", j.get("a"), &anim::to_scalar);
                        if let Some(lj) = int(j, "lj") {
                            set_enum(&mut g, "join", lj.saturating_sub(1).clamp(0, 2) as u32);
                        }
                        set(im, &mut g, "miter", j.get("ml"), &anim::to_scalar);
                    }
                    "pb" => set(im, &mut g, "amount", j.get("a"), &anim::to_scalar),
                    "tw" => {
                        set(im, &mut g, "angle", j.get("a"), &anim::to_scalar);
                        set(im, &mut g, "center", j.get("c"), &anim::to_vec2);
                    }
                    "zz" => {
                        set(im, &mut g, "size", j.get("s"), &anim::to_scalar);
                        set(im, &mut g, "ridges", j.get("r"), &anim::to_scalar);
                        if int(j, "pt") == Some(2) {
                            set_enum(&mut g, "points", 1);
                        }
                    }
                    _ => {
                        if let Some(m) = int(j, "mm") {
                            set_enum(&mut g, "mode", m.saturating_sub(1).clamp(0, 4) as u32);
                        }
                    }
                }
                g
            }
            "tr" => continue,
            other => {
                im.warn(format!("shape item `{other}` is not supported (skipped)"));
                continue;
            }
        };
        if let Some(nm) = j.get("nm").and_then(Json::as_str) {
            g.name = nm.to_string();
        }
        if j.get("hd").and_then(Json::as_bool) == Some(true) {
            g.enabled = false;
        }
        out.push(g);
    }
    out
}

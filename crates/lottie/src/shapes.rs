//! Shape layer contents ↔ Lottie shape items (`gr`, `rc`, `el`, `sr`, `sh`, `fl`, `st`, `gf`,
//! `gs`, `tm`, `rp`, `rd`, `op`, `pb`, `tw`, `zz`, `mm`, `tr`), on `craft-lottie`'s model.

use craft_lottie::model::{
    Dash, Ellipse, Fill, GradientColors, GradientFill, GradientStroke, Group, Map, Merge, OffsetPath, Path, PolyStar, PuckerBloat, Rect, Repeater,
    RepeaterTransform, RoundCorners, Shape, ShapeKind, Stroke, Transform, Trim, Twist, ZigZag,
};
use craft_lottie::{Animated, Bezier, Json};
use effectcraft_color::BlendMode;
use effectcraft_keyframe::{Gradient, ShapePath, Value};
use effectcraft_project::build::{self, EXTRA_DASHES, Ids};
use effectcraft_project::{Node, ParamUi, PropGroup};
use serde_json::json;

use crate::anim::{self, Ex, Im, export_prop, import_prop};

// ---------------------------------------------------------------- shared value converters

pub(crate) fn bezier(p: &ShapePath) -> Bezier {
    Bezier { closed: p.closed, vertices: p.vertices.clone(), in_tangents: p.in_tangents.clone(), out_tangents: p.out_tangents.clone() }
}

pub(crate) fn shape_json(p: &ShapePath) -> Json {
    bezier(p).to_json()
}

pub(crate) fn path_value(v: &Value) -> Json {
    match v {
        Value::Path(p) => shape_json(p),
        _ => shape_json(&ShapePath::default()),
    }
}

/// A Lottie path. `closed` is used when the path has no `c`: Bodymovin 3.x and early 4.x keep the
/// flag outside the path, as `cl` on a mask and `closed` on an `sh` item.
pub(crate) fn to_path(j: &Json, closed: bool) -> Option<Value> {
    let b = Bezier::from_json(j, closed)?;
    Some(Value::Path(ShapePath { vertices: b.vertices, in_tangents: b.in_tangents, out_tangents: b.out_tangents, closed: b.closed, feather: Vec::new() }))
}

/// Gradient as Lottie's flat stop list: `[pos, r, g, b]…` then `[pos, alpha]…`.
pub(crate) fn gradient_flat(v: &Value) -> Json {
    let g = match v {
        Value::Gradient(g) => g.clone(),
        _ => Gradient::default(),
    };
    craft_lottie::Gradient {
        colors: g.colors.iter().map(|(p, c)| (*p, [f64::from(c[0]), f64::from(c[1]), f64::from(c[2])])).collect(),
        opacities: g.opacities.iter().map(|(p, a)| (*p, f64::from(*a))).collect(),
    }
    .to_flat()
}

pub(crate) fn to_gradient(j: &Json, ncolors: usize) -> Option<Value> {
    let g = craft_lottie::Gradient::from_flat(j, ncolors)?;
    Some(Value::Gradient(Gradient {
        colors: g.colors.iter().map(|(p, [r, gr, b])| (*p, [*r as f32, *gr as f32, *b as f32, 1.0])).collect(),
        opacities: g.opacities.iter().map(|(p, a)| (*p, *a as f32)).collect(),
    }))
}

fn blend_index(m: BlendMode) -> u32 {
    BlendMode::ALL.iter().position(|b| *b == m).unwrap_or(0) as u32
}

// ---------------------------------------------------------------- export

/// A group's properties, exported with the warnings they need.
struct Out<'a, 'b> {
    ex: &'a mut Ex,
    g: &'b PropGroup,
    label: String,
}

impl Out<'_, '_> {
    /// The property `m` of the group, when it has one.
    fn put(&mut self, m: &str, conv: &dyn Fn(&Value) -> Json) -> Option<Animated> {
        let p = self.g.get(m)?;
        Some(export_prop(self.ex, p, conv, &format!("{} ▸ {}", self.label, p.name)))
    }
    fn e(&self, m: &str) -> u32 {
        self.g.get(m).map(|p| p.value.as_enum()).unwrap_or(0)
    }
    fn f(&self, m: &str) -> f64 {
        self.g.get(m).map(|p| p.value.as_f64()).unwrap_or(0.0)
    }
    fn flag(&self, m: &str) -> bool {
        self.g.get(m).map(|p| p.value.as_bool()).unwrap_or(false)
    }
    fn animated(&self, m: &str) -> bool {
        self.g.get(m).is_some_and(|p| p.is_animated() || p.has_expression())
    }
    fn blend(&mut self) -> i64 {
        let i = self.e("blend");
        let m = BlendMode::ALL.get(i as usize).copied().unwrap_or_default();
        match crate::blend_to_lottie(m) {
            Some(bm) => i64::from(bm),
            None => {
                self.ex.warn(format!("{}: blend mode {m:?} has no Lottie equivalent (Normal used)", self.label));
                0
            }
        }
    }
    fn direction(&self) -> i64 {
        if self.e("direction") == 1 { 3 } else { 1 }
    }
}

/// A static property (`{a: 0, k}`).
pub(crate) fn fixed(k: Json) -> Animated {
    Animated(json!({"a": 0, "k": k}))
}

/// Stroke fields shared by `st` and `gs`: width, cap, join, miter limit, dashes.
struct StrokeOut {
    width: Option<Animated>,
    cap: i64,
    join: i64,
    miter: f64,
    dashes: Option<Vec<Dash>>,
}

fn stroke_fields(o: &mut Out) -> StrokeOut {
    let width = o.put("width", &anim::num);
    let (cap, join, miter) = (i64::from(o.e("cap")) + 1, i64::from(o.e("join")) + 1, o.f("miter"));
    let used = |g: Option<&PropGroup>, m: &str| g.and_then(|g| g.get(m)).is_some_and(|p| p.is_animated() || p.value.as_f64().abs() > 1e-9);
    let (taper, wave) = (o.g.sub("taper"), o.g.sub("wave"));
    if used(taper, "startLength") || used(taper, "endLength") || used(wave, "amount") {
        let msg = format!("{}: stroke Taper and Wave have no Lottie equivalent (uniform width exported)", o.label);
        o.ex.warn(msg);
    }
    let mut dashes = None;
    if let Some(d) = o.g.sub("dashes") {
        let dash = d.get("dash").map(|p| p.value.as_f64()).unwrap_or(0.0);
        if dash > 0.0 || d.any_animated() {
            let mut list = vec![];
            for (n, m) in [("d", "dash"), ("g", "gap"), ("d", "dash2"), ("g", "gap2"), ("d", "dash3"), ("g", "gap3"), ("o", "offset")] {
                if let Some(p) = d.get(m) {
                    let v = export_prop(o.ex, p, &anim::num, &format!("{} ▸ Dashes ▸ {}", o.label, p.name));
                    list.push(Dash { kind: Some(n.into()), name: Some(p.name.clone()), length: Some(v), ..Default::default() });
                }
            }
            dashes = Some(list);
        }
    }
    StrokeOut { width, cap, join, miter, dashes }
}

/// The `tr` of a group or a repeater: anchor, position, scale, rotation, opacity, skew, and the
/// repeater's start and end opacity, by their Lottie keys.
fn shape_transform(ex: &mut Ex, tr: &PropGroup, label: &str) -> Vec<(&'static str, Animated)> {
    let mut o = Out { ex, g: tr, label: format!("{label} ▸ Transform") };
    let mut out = vec![];
    for (k, m, conv) in [
        ("a", "anchor", anim::arr2 as fn(&Value) -> Json),
        ("p", "position", anim::arr2),
        ("s", "scale", anim::arr2),
        ("r", "rotation", anim::num),
        ("o", "opacity", anim::num),
        ("sk", "skew", anim::num),
        ("sa", "skewAxis", anim::num),
        ("so", "startOpacity", anim::num),
        ("eo", "endOpacity", anim::num),
    ] {
        if let Some(v) = o.put(m, &conv) {
            out.push((k, v));
        }
    }
    out
}

/// The `tr` item that ends a group's items.
fn group_transform(keys: Vec<(&'static str, Animated)>) -> Shape {
    let (mut t, mut extra) = (Transform::default(), Map::new());
    for (k, v) in keys {
        match k {
            "a" => t.anchor = Some(v),
            "p" => t.position = Some(v),
            "s" => t.scale = Some(v),
            "r" => t.rotation = Some(v),
            "o" => t.opacity = Some(v),
            "sk" => t.skew = Some(v),
            "sa" => t.skew_axis = Some(v),
            _ => {
                extra.insert(k.into(), v.0);
            }
        }
    }
    Shape { kind: ShapeKind::Transform(Box::new(t)), extra, ..Default::default() }
}

/// A repeater's `tr`, which players read with `ty: "tr"` like a group's.
fn repeater_transform(keys: Vec<(&'static str, Animated)>) -> RepeaterTransform {
    let mut t = RepeaterTransform::default();
    t.extra.insert("ty".into(), json!("tr"));
    for (k, v) in keys {
        match k {
            "a" => t.anchor = Some(v),
            "p" => t.position = Some(v),
            "s" => t.scale = Some(v),
            "r" => t.rotation = Some(v),
            "so" => t.start_opacity = Some(v),
            "eo" => t.end_opacity = Some(v),
            "sk" => t.skew = Some(v),
            "sa" => t.skew_axis = Some(v),
            _ => {
                t.extra.insert(k.into(), v.0);
            }
        }
    }
    t
}

/// Export a contents group's items (first item = top, as in the timeline).
pub(crate) fn export_items(ex: &mut Ex, contents: &PropGroup, label: &str) -> Vec<Shape> {
    let mut out = vec![];
    for node in &contents.children {
        let Node::Group(g) = node else { continue };
        let lbl = format!("{label} ▸ {}", g.name);
        let mut o = Out { ex, g, label: lbl.clone() };
        let mut blend = None;
        let kind = match g.match_id.as_str() {
            "group" => {
                let mut items = g.sub("contents").map(|c| export_items(o.ex, c, &lbl)).unwrap_or_default();
                items.push(match g.sub("transform") {
                    Some(t) => group_transform(shape_transform(o.ex, t, &lbl)),
                    None => group_transform(vec![
                        ("a", fixed(json!([0, 0]))),
                        ("p", fixed(json!([0, 0]))),
                        ("s", fixed(json!([100, 100]))),
                        ("r", fixed(json!(0))),
                        ("o", fixed(json!(100))),
                    ]),
                });
                blend = Some(o.blend());
                ShapeKind::Group(Group { count: Some(items.len() as f64), items: Some(items) })
            }
            "rect" => {
                let direction = Some(o.direction());
                let size = o.put("size", &anim::arr2);
                let position = o.put("position", &anim::arr2);
                let roundness = if o.flag("perCorner") {
                    let names = ["topLeftRoundness", "topRightRoundness", "bottomRightRoundness", "bottomLeftRoundness"];
                    let radii = names.map(|m| o.f(m));
                    // Lottie `rc` has one `r`: four equal static radii export as that value.
                    // `r` is a property (`{a, k}`) like the uniform branch writes, not a bare number.
                    if radii.iter().all(|v| (v - radii[0]).abs() < 1e-6) && !names.iter().any(|m| o.animated(m)) && radii[0] >= 0.0 {
                        Some(fixed(json!(craft_lottie::round6(radii[0]))))
                    } else {
                        o.ex.warn(format!("{}: per-corner roundness has no Lottie rectangle equivalent (roundness 0 exported)", o.label));
                        Some(fixed(json!(0)))
                    }
                } else {
                    o.put("roundness", &anim::num)
                };
                ShapeKind::Rect(Rect { direction, position, size, roundness })
            }
            "ellipse" => {
                let direction = Some(o.direction());
                let size = o.put("size", &anim::arr2);
                ShapeKind::Ellipse(Ellipse { direction, position: o.put("position", &anim::arr2), size })
            }
            "star" => ShapeKind::PolyStar(PolyStar {
                direction: Some(o.direction()),
                star_type: Some(i64::from(o.e("type")) + 1),
                points: o.put("points", &anim::num),
                position: o.put("position", &anim::arr2),
                rotation: o.put("rotation", &anim::num),
                inner_radius: o.put("innerRadius", &anim::num),
                outer_radius: o.put("outerRadius", &anim::num),
                inner_roundness: o.put("innerRoundness", &anim::num),
                outer_roundness: o.put("outerRoundness", &anim::num),
            }),
            "path" => ShapeKind::Path(Path { direction: Some(o.direction()), path: o.put("path", &path_value), closed: None }),
            "fill" => {
                let f = Fill { color: o.put("color", &anim::arr), opacity: o.put("opacity", &anim::num), rule: Some(i64::from(o.e("rule")) + 1) };
                blend = Some(o.blend());
                ShapeKind::Fill(f)
            }
            "stroke" => {
                let (color, opacity) = (o.put("color", &anim::arr), o.put("opacity", &anim::num));
                let st = stroke_fields(&mut o);
                blend = Some(o.blend());
                ShapeKind::Stroke(Stroke {
                    color,
                    opacity,
                    width: st.width,
                    cap: Some(st.cap),
                    join: Some(st.join),
                    miter_limit: Some(st.miter),
                    miter_limit_animated: None,
                    dashes: st.dashes,
                })
            }
            m @ ("gfill" | "gstroke") => {
                let opacity = o.put("opacity", &anim::num);
                let (start, end) = (o.put("start", &anim::arr2), o.put("end", &anim::arr2));
                let gradient_type = Some(i64::from(o.e("type")) + 1);
                let (highlight_length, highlight_angle) = (o.put("highlightLength", &anim::num), o.put("highlightAngle", &anim::num));
                let ncolors = match g.get("colors").map(|p| &p.value) {
                    Some(Value::Gradient(gr)) => gr.colors.len(),
                    _ => 2,
                };
                let colors = g.get("colors").map(|p| GradientColors {
                    count: Some(ncolors as i64),
                    stops: Some(export_prop(o.ex, p, &gradient_flat, &format!("{lbl} ▸ Colors"))),
                    ..Default::default()
                });
                let kind = if m == "gfill" {
                    ShapeKind::GradientFill(GradientFill {
                        colors,
                        start,
                        end,
                        gradient_type,
                        highlight_length,
                        highlight_angle,
                        opacity,
                        rule: Some(i64::from(o.e("rule")) + 1),
                    })
                } else {
                    let st = stroke_fields(&mut o);
                    ShapeKind::GradientStroke(GradientStroke {
                        colors,
                        start,
                        end,
                        gradient_type,
                        highlight_length,
                        highlight_angle,
                        opacity,
                        width: st.width,
                        cap: Some(st.cap),
                        join: Some(st.join),
                        miter_limit: Some(st.miter),
                        miter_limit_animated: None,
                        dashes: st.dashes,
                    })
                };
                blend = Some(o.blend());
                kind
            }
            "trim" => ShapeKind::Trim(Trim {
                start: o.put("start", &anim::num),
                end: o.put("end", &anim::num),
                offset: o.put("offset", &anim::num),
                multiple: Some(i64::from(o.e("mode")) + 1),
            }),
            "repeater" => {
                let (copies, offset) = (o.put("copies", &anim::num), o.put("offset", &anim::num));
                // Lottie: 1 = Above, 2 = Below; EffectCraft: 0 = Below, 1 = Above.
                let composite = Some(if o.e("composite") == 1 { 1 } else { 2 });
                let transform = g.sub("transform").map(|t| repeater_transform(shape_transform(o.ex, t, &lbl)));
                ShapeKind::Repeater(Repeater { copies, offset, composite, transform })
            }
            "round" => ShapeKind::RoundCorners(RoundCorners { radius: o.put("radius", &anim::num) }),
            "offset" => {
                let amount = o.put("amount", &anim::num);
                let join = Some(i64::from(o.e("join")) + 1);
                let miter_limit = o.put("miter", &anim::num);
                if (o.f("copies") - 1.0).abs() > 1e-9 || o.animated("copies") || o.f("copyOffset") != 0.0 {
                    o.ex.warn(format!("{lbl}: Offset Paths copies are not part of Lottie (one copy exported)"));
                }
                ShapeKind::OffsetPath(OffsetPath { amount, join, miter_limit })
            }
            "pucker" => ShapeKind::PuckerBloat(PuckerBloat { amount: o.put("amount", &anim::num) }),
            "twist" => ShapeKind::Twist(Twist { angle: o.put("angle", &anim::num), center: o.put("center", &anim::arr2) }),
            "zigzag" => ShapeKind::ZigZag(ZigZag {
                frequency: o.put("size", &anim::num),
                amount: o.put("ridges", &anim::num),
                point_type: Some(fixed(json!(o.e("points") + 1))),
            }),
            "merge" => ShapeKind::Merge(Merge { mode: Some(i64::from(o.e("mode")) + 1) }),
            other => {
                o.ex.warn(format!("{lbl}: shape item `{other}` has no Lottie equivalent (skipped)"));
                continue;
            }
        };
        out.push(Shape { name: Some(g.name.clone()), hidden: (!g.enabled).then_some(true), blend_mode: blend, kind, ..Default::default() });
    }
    out
}

// ---------------------------------------------------------------- import

fn set(im: &Im, g: &mut PropGroup, m: &str, j: Option<&Animated>, conv: &dyn Fn(&Json) -> Option<Value>) {
    if let (Some(j), Some(p)) = (j, g.get_mut(m)) {
        import_prop(im, &j.0, p, conv);
    }
}

fn set_json(im: &Im, g: &mut PropGroup, m: &str, j: Option<&Json>, conv: &dyn Fn(&Json) -> Option<Value>) {
    if let (Some(j), Some(p)) = (j, g.get_mut(m)) {
        import_prop(im, j, p, conv);
    }
}

fn set_enum(g: &mut PropGroup, m: &str, v: u32) {
    if let Some(p) = g.get_mut(m) {
        p.value = Value::Enum(v);
    }
}

/// A whole number however a file writes it: `2`, `2.0`, `2.4` (rounded) or a property `{k: 2}`.
fn int_json(v: Option<&Json>) -> Option<i64> {
    let v = v?;
    v.as_i64().or_else(|| v.as_f64().map(|f| f.round() as i64)).or_else(|| v.get("k").and_then(anim::nums).and_then(|n| n.first().map(|f| f.round() as i64)))
}

/// An integer key of the model, or the value the model kept in `extra` when it was written
/// another way.
fn int(field: Option<i64>, extra: &Map, key: &str) -> Option<i64> {
    field.or_else(|| int_json(extra.get(key)))
}

fn import_blend(s: &Shape, g: &mut PropGroup) {
    if let Some(bm) = int(s.blend_mode, &s.extra, "bm").and_then(|b| u64::try_from(b).ok()) {
        set_enum(g, "blend", blend_index(crate::blend_from_lottie(bm)));
    }
}

/// The stroke keys of `st` and `gs`.
struct StrokeIn<'a> {
    width: Option<&'a Animated>,
    cap: Option<i64>,
    join: Option<i64>,
    miter: Option<f64>,
    miter_animated: Option<&'a Animated>,
    dashes: Option<&'a [Dash]>,
}

fn import_stroke(im: &Im, ids: &mut Ids, st: StrokeIn<'_>, g: &mut PropGroup) {
    set(im, g, "width", st.width, &anim::to_scalar);
    if let Some(lc) = st.cap {
        set_enum(g, "cap", lc.saturating_sub(1).clamp(0, 2) as u32);
    }
    if let Some(lj) = st.join {
        set_enum(g, "join", lj.saturating_sub(1).clamp(0, 2) as u32);
    }
    if let Some(p) = g.get_mut("miter") {
        if let Some(ml) = st.miter {
            p.value = Value::Scalar(ml);
        } else if let Some(ml2) = st.miter_animated {
            import_prop(im, &ml2.0, p, &anim::to_scalar);
        }
    }
    if let Some(list) = st.dashes
        && let Some(d) = g.sub_mut("dashes")
    {
        // The n-th `d` and `g` are the n-th Dash/Gap pair; the extra pairs are created as the Dashes "+" does.
        let (mut dashes, mut gaps) = (0, 0);
        for e in list {
            let (n, pair) = match e.kind.as_deref() {
                Some("d") => (&mut dashes, 0),
                Some("g") => (&mut gaps, 1),
                Some("o") => {
                    set(im, d, "offset", e.length.as_ref(), &anim::to_scalar);
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
            set(im, d, m, e.length.as_ref(), &anim::to_scalar);
        }
    }
}

/// A `tr` (of a group or a repeater), looked up by its Lottie keys.
fn import_shape_transform(im: &Im, get: &dyn Fn(&str) -> Option<Json>, tr: &mut PropGroup) {
    for (k, m, conv) in [
        ("a", "anchor", anim::to_vec2 as fn(&Json) -> Option<Value>),
        ("p", "position", anim::to_vec2),
        ("s", "scale", anim::to_vec2),
        ("r", "rotation", anim::to_scalar),
        ("o", "opacity", anim::to_scalar),
        ("sk", "skew", anim::to_scalar),
        ("sa", "skewAxis", anim::to_scalar),
        ("so", "startOpacity", anim::to_scalar),
        ("eo", "endOpacity", anim::to_scalar),
    ] {
        set_json(im, tr, m, get(k).as_ref(), &conv);
    }
}

/// A named key of the model, or the same key kept in `extra`.
fn key(a: Option<&Animated>, extra: &Map, k: &str) -> Option<Json> {
    a.map(|a| a.0.clone()).or_else(|| extra.get(k).cloned())
}

fn group_tr_get<'a>(t: &'a Transform, item: &'a Map) -> impl Fn(&str) -> Option<Json> + 'a {
    move |k| {
        let named = match k {
            "a" => t.anchor.as_ref(),
            "p" => t.position.as_ref(),
            "s" => t.scale.as_ref(),
            "r" => t.rotation.as_ref(),
            "o" => t.opacity.as_ref(),
            "sk" => t.skew.as_ref(),
            "sa" => t.skew_axis.as_ref(),
            _ => None,
        };
        key(named, &t.extra, k).or_else(|| item.get(k).cloned())
    }
}

fn repeater_tr_get(t: &RepeaterTransform) -> impl Fn(&str) -> Option<Json> + '_ {
    move |k| {
        let named = match k {
            "a" => t.anchor.as_ref(),
            "p" => t.position.as_ref(),
            "s" => t.scale.as_ref(),
            "r" => t.rotation.as_ref(),
            "so" => t.start_opacity.as_ref(),
            "eo" => t.end_opacity.as_ref(),
            "sk" => t.skew.as_ref(),
            "sa" => t.skew_axis.as_ref(),
            _ => None,
        };
        key(named, &t.extra, k)
    }
}

/// Import Lottie shape items into shape contents groups.
pub(crate) fn import_items(im: &mut Im, ids: &mut Ids, items: &mut dyn Iterator<Item = &Shape>) -> Vec<PropGroup> {
    let mut out = vec![];
    for s in items {
        let x = &s.extra;
        let dir = |g: &mut PropGroup, d: Option<i64>| {
            if int(d, x, "d") == Some(3) {
                set_enum(g, "direction", 1);
            }
        };
        let mut g = match &s.kind {
            ShapeKind::Group(gr) => {
                let list = gr.items.as_deref().unwrap_or_default();
                // Borrowed, not cloned: each nesting level copying its subtree costs memory
                // quadratic in the depth.
                let children = import_items(im, ids, &mut list.iter().filter(|i| !matches!(i.kind, ShapeKind::Transform(_))));
                let mut g = build::shape_group(ids, "Group 1", children);
                if let Some(tr) = list.iter().find(|i| matches!(i.kind, ShapeKind::Transform(_)))
                    && let ShapeKind::Transform(t) = &tr.kind
                    && let Some(tg) = g.sub_mut("transform")
                {
                    import_shape_transform(im, &group_tr_get(t, &tr.extra), tg);
                }
                import_blend(s, &mut g);
                g
            }
            ShapeKind::Rect(r) => {
                let mut g = build::shape_rect(ids, [100.0, 100.0], [0.0, 0.0], 0.0);
                dir(&mut g, r.direction);
                set(im, &mut g, "size", r.size.as_ref(), &anim::to_vec2);
                set(im, &mut g, "position", r.position.as_ref(), &anim::to_vec2);
                set(im, &mut g, "roundness", r.roundness.as_ref(), &anim::to_scalar);
                // Lottie has one roundness: keep the per-corner values in step so ticking the box
                // on an imported rectangle keeps the shape.
                for m in ["topLeftRoundness", "topRightRoundness", "bottomRightRoundness", "bottomLeftRoundness"] {
                    set(im, &mut g, m, r.roundness.as_ref(), &anim::to_scalar);
                }
                g
            }
            ShapeKind::Ellipse(e) => {
                let mut g = build::shape_ellipse(ids, [100.0, 100.0], [0.0, 0.0]);
                dir(&mut g, e.direction);
                set(im, &mut g, "size", e.size.as_ref(), &anim::to_vec2);
                set(im, &mut g, "position", e.position.as_ref(), &anim::to_vec2);
                g
            }
            ShapeKind::PolyStar(p) => {
                let star = int(p.star_type, x, "sy").unwrap_or(1) != 2;
                let mut g = build::shape_star(ids, star, 5.0, [0.0, 0.0], 100.0, 50.0);
                dir(&mut g, p.direction);
                set(im, &mut g, "points", p.points.as_ref(), &anim::to_scalar);
                set(im, &mut g, "position", p.position.as_ref(), &anim::to_vec2);
                set(im, &mut g, "rotation", p.rotation.as_ref(), &anim::to_scalar);
                set(im, &mut g, "innerRadius", p.inner_radius.as_ref(), &anim::to_scalar);
                set(im, &mut g, "outerRadius", p.outer_radius.as_ref(), &anim::to_scalar);
                set(im, &mut g, "innerRoundness", p.inner_roundness.as_ref(), &anim::to_scalar);
                set(im, &mut g, "outerRoundness", p.outer_roundness.as_ref(), &anim::to_scalar);
                g
            }
            ShapeKind::Path(p) => {
                let mut g = build::shape_path(ids, ShapePath::default());
                dir(&mut g, p.direction);
                let closed = p.closed.unwrap_or(false);
                set(im, &mut g, "path", p.path.as_ref(), &|v| to_path(v, closed));
                g
            }
            ShapeKind::Fill(f) => {
                let mut g = build::shape_fill(ids, [1.0, 1.0, 1.0, 1.0]);
                set(im, &mut g, "color", f.color.as_ref(), &anim::to_color);
                set(im, &mut g, "opacity", f.opacity.as_ref(), &anim::to_scalar);
                if int(f.rule, x, "r") == Some(2) {
                    set_enum(&mut g, "rule", 1);
                }
                import_blend(s, &mut g);
                g
            }
            ShapeKind::Stroke(st) => {
                let mut g = build::shape_stroke(ids, [1.0, 1.0, 1.0, 1.0], 2.0);
                set(im, &mut g, "color", st.color.as_ref(), &anim::to_color);
                set(im, &mut g, "opacity", st.opacity.as_ref(), &anim::to_scalar);
                let fields = StrokeIn {
                    width: st.width.as_ref(),
                    cap: int(st.cap, x, "lc"),
                    join: int(st.join, x, "lj"),
                    miter: st.miter_limit,
                    miter_animated: st.miter_limit_animated.as_ref(),
                    dashes: st.dashes.as_deref(),
                };
                import_stroke(im, ids, fields, &mut g);
                import_blend(s, &mut g);
                g
            }
            ShapeKind::GradientFill(gf) => {
                let radial = int(gf.gradient_type, x, "t") == Some(2);
                let mut g = build::shape_gradient_fill(ids, radial, [0.0, 0.0], [100.0, 0.0], Gradient::default());
                gradient_common(
                    im,
                    &mut g,
                    gf.opacity.as_ref(),
                    gf.start.as_ref(),
                    gf.end.as_ref(),
                    gf.highlight_length.as_ref(),
                    gf.highlight_angle.as_ref(),
                    gf.colors.as_ref(),
                );
                if int(gf.rule, x, "r") == Some(2) {
                    set_enum(&mut g, "rule", 1);
                }
                import_blend(s, &mut g);
                g
            }
            ShapeKind::GradientStroke(gs) => {
                let radial = int(gs.gradient_type, x, "t") == Some(2);
                let mut g = build::shape_gradient_stroke(ids, radial, [0.0, 0.0], [100.0, 0.0], Gradient::default(), 2.0);
                gradient_common(
                    im,
                    &mut g,
                    gs.opacity.as_ref(),
                    gs.start.as_ref(),
                    gs.end.as_ref(),
                    gs.highlight_length.as_ref(),
                    gs.highlight_angle.as_ref(),
                    gs.colors.as_ref(),
                );
                let fields = StrokeIn {
                    width: gs.width.as_ref(),
                    cap: int(gs.cap, x, "lc"),
                    join: int(gs.join, x, "lj"),
                    miter: gs.miter_limit,
                    miter_animated: gs.miter_limit_animated.as_ref(),
                    dashes: gs.dashes.as_deref(),
                };
                import_stroke(im, ids, fields, &mut g);
                import_blend(s, &mut g);
                g
            }
            ShapeKind::Trim(t) => {
                let mut g = build::shape_trim(ids, 0.0, 100.0, 0.0);
                set(im, &mut g, "start", t.start.as_ref(), &anim::to_scalar);
                set(im, &mut g, "end", t.end.as_ref(), &anim::to_scalar);
                set(im, &mut g, "offset", t.offset.as_ref(), &anim::to_scalar);
                if int(t.multiple, x, "m") == Some(2) {
                    set_enum(&mut g, "mode", 1);
                }
                g
            }
            ShapeKind::Repeater(r) => {
                let mut g = build::shape_repeater(ids, 3.0, [100.0, 0.0]);
                set(im, &mut g, "copies", r.copies.as_ref(), &anim::to_scalar);
                set(im, &mut g, "offset", r.offset.as_ref(), &anim::to_scalar);
                set_enum(&mut g, "composite", if int(r.composite, x, "m") == Some(1) { 1 } else { 0 });
                if let (Some(tr), Some(t)) = (r.transform.as_ref(), g.sub_mut("transform")) {
                    import_shape_transform(im, &repeater_tr_get(tr), t);
                }
                g
            }
            ShapeKind::RoundCorners(r) => {
                let Some(mut g) = build::shape_simple_op(ids, "round") else { continue };
                set(im, &mut g, "radius", r.radius.as_ref(), &anim::to_scalar);
                g
            }
            ShapeKind::OffsetPath(op) => {
                let Some(mut g) = build::shape_simple_op(ids, "offset") else { continue };
                set(im, &mut g, "amount", op.amount.as_ref(), &anim::to_scalar);
                if let Some(lj) = int(op.join, x, "lj") {
                    set_enum(&mut g, "join", lj.saturating_sub(1).clamp(0, 2) as u32);
                }
                set(im, &mut g, "miter", op.miter_limit.as_ref(), &anim::to_scalar);
                g
            }
            ShapeKind::PuckerBloat(pb) => {
                let Some(mut g) = build::shape_simple_op(ids, "pucker") else { continue };
                set(im, &mut g, "amount", pb.amount.as_ref(), &anim::to_scalar);
                g
            }
            ShapeKind::Twist(tw) => {
                let Some(mut g) = build::shape_simple_op(ids, "twist") else { continue };
                set(im, &mut g, "angle", tw.angle.as_ref(), &anim::to_scalar);
                set(im, &mut g, "center", tw.center.as_ref(), &anim::to_vec2);
                g
            }
            ShapeKind::ZigZag(zz) => {
                let Some(mut g) = build::shape_simple_op(ids, "zigzag") else { continue };
                set(im, &mut g, "size", zz.frequency.as_ref(), &anim::to_scalar);
                set(im, &mut g, "ridges", zz.amount.as_ref(), &anim::to_scalar);
                if int_json(zz.point_type.as_ref().map(|a| &a.0)) == Some(2) {
                    set_enum(&mut g, "points", 1);
                }
                g
            }
            ShapeKind::Merge(mm) => {
                let Some(mut g) = build::shape_simple_op(ids, "merge") else { continue };
                if let Some(m) = int(mm.mode, x, "mm") {
                    set_enum(&mut g, "mode", m.saturating_sub(1).clamp(0, 4) as u32);
                }
                g
            }
            ShapeKind::Transform(_) => continue,
            ShapeKind::Other(ty) => {
                im.warn(format!("shape item `{}` is not supported (skipped)", ty.as_str().unwrap_or("")));
                continue;
            }
        };
        if let Some(nm) = &s.name {
            g.name = nm.clone();
        }
        if s.hidden == Some(true) {
            g.enabled = false;
        }
        out.push(g);
    }
    out
}

/// The keys `gf` and `gs` share.
#[allow(clippy::too_many_arguments)]
fn gradient_common(
    im: &Im,
    g: &mut PropGroup,
    opacity: Option<&Animated>,
    start: Option<&Animated>,
    end: Option<&Animated>,
    highlight_length: Option<&Animated>,
    highlight_angle: Option<&Animated>,
    colors: Option<&GradientColors>,
) {
    set(im, g, "opacity", opacity, &anim::to_scalar);
    set(im, g, "start", start, &anim::to_vec2);
    set(im, g, "end", end, &anim::to_vec2);
    set(im, g, "highlightLength", highlight_length, &anim::to_scalar);
    set(im, g, "highlightAngle", highlight_angle, &anim::to_scalar);
    if let Some(c) = colors {
        let n = c.count.and_then(|n| usize::try_from(n).ok()).unwrap_or(2);
        set(im, g, "colors", c.stops.as_ref(), &move |v| to_gradient(v, n));
    }
}

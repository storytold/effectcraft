//! Animated properties: EffectCraft keyframes ↔ Lottie keyframes, through `craft-lottie`'s
//! property encoding ([`craft_lottie::read_prop`], [`craft_lottie::write_prop`]) and its
//! conversion between a segment's normalised Bezier ease and After Effects' speed and influence
//! ([`Tangents`]). Spatial properties ease along the motion path (Δv = arc length, one
//! dimension) and carry `to`/`ti` spatial tangents.

pub(crate) use craft_lottie::nums;
use craft_lottie::{Animated, Json, KeyOut, PropValue, Tangents, read_prop, round6};
use effectcraft_keyframe::{self as kf, Ease, Interp, Keyframe, Value};
use effectcraft_project::{Expression, Property};
use serde_json::json;

use crate::Timebase;

/// Most distinct warnings an import or export reports.
const MAX_WARNINGS: usize = 200;

/// Export state shared by the whole document.
pub(crate) struct Ex {
    pub tb: Timebase,
    pub include_expressions: bool,
    pub warnings: Vec<String>,
}

impl Ex {
    pub fn warn(&mut self, w: impl Into<String>) {
        let w = w.into();
        // Each warning is looked for among the earlier ones: past a screenful, more add nothing.
        if self.warnings.len() < MAX_WARNINGS && !self.warnings.contains(&w) {
            self.warnings.push(w);
        }
    }
}

/// Import state.
pub(crate) struct Im {
    pub tb: Timebase,
    pub warnings: Vec<String>,
}

impl Im {
    pub fn warn(&mut self, w: impl Into<String>) {
        let w = w.into();
        // Each warning is looked for among the earlier ones: past a screenful, more add nothing.
        if self.warnings.len() < MAX_WARNINGS && !self.warnings.contains(&w) {
            self.warnings.push(w);
        }
    }
}

fn secs(t: effectcraft_time::Tick) -> f64 {
    t.seconds()
}

fn r6(v: f64) -> f64 {
    round6(v)
}

fn lottie_ease(e: Ease) -> craft_lottie::Ease {
    craft_lottie::Ease { speed: e.speed, influence: e.influence }
}

fn is_spatial(p: &Property) -> bool {
    p.spatial && matches!(p.value, Value::Vec2(_) | Value::Vec3(_))
}

/// Export a property as a Lottie animatable property (`{a, k}` plus `x` for expressions).
/// `label` names the property in warnings. Muted keys are omitted: Lottie has no mute state, and
/// the remaining keys carry the animation the renderer shows.
pub(crate) fn export_prop(ex: &mut Ex, p: &Property, conv: &dyn Fn(&Value) -> Json, label: &str) -> Animated {
    let muted = p.keys.iter().any(|k| k.muted);
    if muted {
        ex.warn(format!("{label}: muted keyframes are not exported"));
    }
    let active: Vec<Keyframe>;
    let keys: &[Keyframe] = if muted {
        active = p.keys.iter().filter(|k| !k.muted).cloned().collect();
        &active
    } else {
        &p.keys
    };
    let mut expression = None;
    if let Some(e) = p.expr.as_ref().filter(|e| e.enabled && !e.text.trim().is_empty()) {
        if ex.include_expressions {
            expression = Some(e.text.as_str());
        } else {
            ex.warn(format!("{label}: expression not exported (enable includeExpressions to export it as `x`)"));
        }
    }
    if keys.is_empty() {
        return Animated(craft_lottie::write_prop(conv(&p.value), &[], expression));
    }
    Animated::keyed(&export_keys(ex, keys, is_spatial(p), p.hold_only, conv), expression)
}

fn export_keys(ex: &Ex, keys: &[Keyframe], spatial: bool, hold_only: bool, conv: &dyn Fn(&Value) -> Json) -> Vec<KeyOut> {
    let mut out = Vec::with_capacity(keys.len());
    for (i, k) in keys.iter().enumerate() {
        let mut o = KeyOut { t: ex.tb.frame(k.time), s: conv(&k.value), hold: false, ease: vec![], spatial: None };
        let last = i + 1 == keys.len();
        if k.out_interp == Interp::Hold || hold_only || (!last && !k.value.interpolates()) {
            o.hold = true;
        } else if !last {
            o.ease = segment_ease(keys, i, spatial);
            if spatial {
                let (_, to) = kf::spatial_tangents(keys, i);
                let (ti, _) = kf::spatial_tangents(keys, i + 1);
                let n = if matches!(k.value, Value::Vec2(_)) { 2 } else { conv(&k.value).as_array().map(Vec::len).unwrap_or(3).min(3) };
                o.spatial = Some((to.get(..n).unwrap_or_default().to_vec(), ti.get(..n).unwrap_or_default().to_vec()));
            }
        }
        out.push(o);
    }
    out
}

/// Normalised ease of segment `i → i+1`, one entry per dimension.
fn segment_ease(keys: &[Keyframe], i: usize, spatial: bool) -> Vec<Tangents> {
    let (Some(a), Some(b)) = (keys.get(i), keys.get(i + 1)) else { return vec![] };
    let dur = (secs(b.time) - secs(a.time)).max(1e-12);
    let ca = a.value.components();
    let cb = b.value.components();
    let numeric = !ca.is_empty() && ca.len() == cb.len();
    let dims = if spatial || !numeric { 1 } else { ca.len() };
    (0..dims)
        .map(|d| {
            let (dv, eo, ei) = if spatial {
                let len = kf::spatial_segment_length(keys, i);
                (len, kf::side_ease(keys, i, 0, true, true), kf::side_ease(keys, i + 1, 0, true, false))
            } else if numeric {
                (cb[d] - ca[d], kf::side_ease(keys, i, d, false, true), kf::side_ease(keys, i + 1, d, false, false))
            } else {
                // Paths, gradients: overall progress 0 → 1.
                let side = |k: &Keyframe, out: bool| {
                    let interp = if out { k.out_interp } else { k.in_interp };
                    let list = if out { &k.out_ease } else { &k.in_ease };
                    if interp == Interp::Bezier {
                        list.first().copied().unwrap_or_default()
                    } else {
                        Ease { speed: 1.0 / dur, influence: kf::LINEAR_SIDE_INFLUENCE }
                    }
                };
                (1.0, Some(side(a, true)), Some(side(b, false)))
            };
            let lin = Ease { speed: dv / dur, influence: kf::LINEAR_SIDE_INFLUENCE };
            // Crossing time handles keep the independently specified influences.
            Tangents::from_speed(lottie_ease(eo.unwrap_or(lin)), lottie_ease(ei.unwrap_or(lin)), dv, dur)
        })
        .collect()
}

// ---------------------------------------------------------------- import

/// Import a Lottie animatable property into `p` (value, keys, expression). `conv` turns a Lottie
/// value (as found in `k` or a key's `s`, possibly wrapped in a one-element array) into a value of
/// the property's type.
pub(crate) fn import_prop(im: &Im, j: &Json, p: &mut Property, conv: &dyn Fn(&Json) -> Option<Value>) {
    let prop = read_prop(j);
    if let Some(x) = prop.expression {
        p.expr = Some(Expression { text: x.to_string(), enabled: true });
    }
    let list = match prop.value {
        PropValue::None => return,
        PropValue::Static(v) => {
            if let Some(v) = conv(v) {
                p.value = v;
            }
            return;
        }
        PropValue::Keys(list) => list,
    };
    let spatial = is_spatial(p);
    let mut keys: Vec<Keyframe> = Vec::with_capacity(list.len());
    let mut kept = Vec::with_capacity(list.len());
    for kj in list {
        let Some(v) = kj.s.and_then(conv) else { continue };
        let mut key = Keyframe::new(im.tb.tick(kj.t), v);
        key.spatial_auto = false;
        if p.hold_only || !key.value.interpolates() {
            key = key.hold();
        }
        keys.push(key);
        kept.push(kj);
    }
    if keys.is_empty() {
        return;
    }
    let n = keys.len();
    for i in 0..n.saturating_sub(1) {
        let Some(kj) = kept.get(i) else { break };
        if kj.hold || p.hold_only {
            keys[i].out_interp = Interp::Hold;
            keys[i + 1].in_interp = Interp::Hold;
            continue;
        }
        if spatial {
            keys[i].spatial_out = kj.to;
            keys[i + 1].spatial_in = kj.ti;
        }
    }
    // Eases need the spatial tangents (arc length), so they come second.
    for i in 0..n.saturating_sub(1) {
        let Some(kj) = kept.get(i) else { break };
        if keys[i].out_interp == Interp::Hold {
            continue;
        }
        let (Some(o), Some(ii)) = (kj.o, kj.i) else {
            keys[i].out_interp = Interp::Linear;
            keys[i + 1].in_interp = Interp::Linear;
            continue;
        };
        let dur = (secs(keys[i + 1].time) - secs(keys[i].time)).max(1e-12);
        let ca = keys[i].value.components();
        let cb = keys[i + 1].value.components();
        let numeric = !ca.is_empty() && ca.len() == cb.len();
        let dims = if spatial || !numeric { 1 } else { ca.len() };
        let mut linear = true;
        let mut outs = vec![];
        let mut ins = vec![];
        for d in 0..dims {
            let t = Tangents::read(o, ii, d);
            linear &= t.is_linear();
            let dv = if spatial {
                kf::spatial_segment_length(&keys, i)
            } else if numeric {
                cb[d] - ca[d]
            } else {
                1.0
            };
            let (out, into) = t.to_speed(dv, dur);
            outs.push(Ease { speed: out.speed, influence: out.influence });
            ins.push(Ease { speed: into.speed, influence: into.influence });
        }
        if linear {
            keys[i].out_interp = Interp::Linear;
            keys[i + 1].in_interp = Interp::Linear;
        } else {
            keys[i].out_interp = Interp::Bezier;
            keys[i].out_ease = outs;
            keys[i + 1].in_interp = Interp::Bezier;
            keys[i + 1].in_ease = ins;
        }
    }
    p.value = keys[0].value.clone();
    p.keys = keys;
}

/// [`import_prop`] for a property of the model ([`Animated`]).
pub(crate) fn import_animated(im: &Im, a: Option<&Animated>, p: &mut Property, conv: &dyn Fn(&Json) -> Option<Value>) {
    if let Some(a) = a {
        import_prop(im, &a.0, p, conv);
    }
}

// ---------------------------------------------------------------- value converters

pub(crate) fn num(v: &Value) -> Json {
    json!(r6(v.as_f64()))
}

pub(crate) fn arr(v: &Value) -> Json {
    match v {
        Value::Scalar(x) => json!([r6(*x)]),
        _ => json!(v.components().iter().map(|x| r6(*x)).collect::<Vec<_>>()),
    }
}

/// Vec2 (first two components of any vector).
pub(crate) fn arr2(v: &Value) -> Json {
    let c = v.components();
    json!([r6(*c.first().unwrap_or(&0.0)), r6(*c.get(1).unwrap_or(&0.0))])
}

/// Vec3 (padded with `z`).
pub(crate) fn arr3(v: &Value) -> Json {
    let c = v.components();
    json!([r6(*c.first().unwrap_or(&0.0)), r6(*c.get(1).unwrap_or(&0.0)), r6(*c.get(2).unwrap_or(&0.0))])
}

pub(crate) fn to_scalar(j: &Json) -> Option<Value> {
    nums(j).and_then(|v| v.first().copied()).map(Value::Scalar)
}

pub(crate) fn to_vec2(j: &Json) -> Option<Value> {
    let v = nums(j)?;
    Some(Value::Vec2([*v.first()?, *v.get(1).unwrap_or(&0.0)]))
}

/// Vec3 with a default third component.
pub(crate) fn to_vec3(j: &Json, z: f64) -> Option<Value> {
    let v = nums(j)?;
    Some(Value::Vec3([*v.first()?, *v.get(1).unwrap_or(&0.0), *v.get(2).unwrap_or(&z)]))
}

pub(crate) fn to_color(j: &Json) -> Option<Value> {
    // Some files use 0–255: `craft_lottie::color` divides those.
    craft_lottie::color(j).map(Value::Color)
}

pub(crate) fn to_bool(j: &Json) -> Option<Value> {
    match j {
        Json::Bool(b) => Some(Value::Bool(*b)),
        _ => to_scalar(j).map(|v| Value::Bool(v.as_f64() != 0.0)),
    }
}

/// Lottie 1-based enum → 0-based popup index.
pub(crate) fn to_enum1(j: &Json) -> Option<Value> {
    to_scalar(j).map(|v| Value::Enum((v.as_f64().round() as i64).saturating_sub(1).max(0) as u32))
}

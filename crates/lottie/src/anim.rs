//! Animated properties: EffectCraft keyframes ↔ Lottie keyframes.
//!
//! A Lottie keyframe holds the start value `s` of a segment and the segment's normalised cubic
//! Bezier ease: `o` (out tangent of this key) and `i` (in tangent of the next key), each with `x`
//! (time fraction) and `y` (value progress fraction), per dimension. After Effects' temporal ease
//! is speed (units/s) and influence (fraction of the segment): for a segment of duration `d` and
//! value change `Δv`,
//!
//! ```text
//! o.x = influence_out          o.y = speed_out · influence_out · d / Δv
//! i.x = 1 − influence_in       i.y = 1 − speed_in · influence_in · d / Δv
//! ```
//!
//! which is exactly the Bezier the keyframe evaluator uses. Spatial properties ease along the
//! motion path (Δv = arc length, one dimension) and carry `to`/`ti` spatial tangents. Hold keys
//! set `h: 1`.

use effectcraft_keyframe::{self as kf, Ease, Interp, Keyframe, Value};
use effectcraft_project::{Expression, Property};
use serde_json::{Value as Json, json};

use crate::Timebase;

/// Export state shared by the whole document.
pub(crate) struct Ex {
    pub tb: Timebase,
    pub include_expressions: bool,
    pub warnings: Vec<String>,
}

impl Ex {
    pub fn warn(&mut self, w: impl Into<String>) {
        let w = w.into();
        if !self.warnings.contains(&w) {
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
        if !self.warnings.contains(&w) {
            self.warnings.push(w);
        }
    }
}

fn secs(t: effectcraft_time::Tick) -> f64 {
    t.seconds()
}

fn r6(v: f64) -> f64 {
    if v.is_finite() { (v * 1e6).round() / 1e6 } else { 0.0 }
}

/// A key value as Lottie's `s` (numbers wrapped in an array, objects in a one-element array).
fn wrap(j: Json) -> Json {
    match j {
        Json::Array(_) => j,
        other => json!([other]),
    }
}

fn is_spatial(p: &Property) -> bool {
    p.spatial && matches!(p.value, Value::Vec2(_) | Value::Vec3(_))
}

/// Export a property as a Lottie animatable property (`{a, k}` plus `x` for expressions).
/// `label` names the property in warnings. Muted keys are omitted: Lottie has no mute state, and
/// the remaining keys carry the animation the renderer shows.
pub(crate) fn export_prop(ex: &mut Ex, p: &Property, conv: &dyn Fn(&Value) -> Json, label: &str) -> Json {
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
    let mut out =
        if keys.is_empty() { json!({"a": 0, "k": conv(&p.value)}) } else { json!({"a": 1, "k": export_keys(ex, keys, is_spatial(p), p.hold_only, conv)}) };
    if let Some(e) = p.expr.as_ref().filter(|e| e.enabled && !e.text.trim().is_empty()) {
        if ex.include_expressions {
            out["x"] = json!(e.text);
        } else {
            ex.warn(format!("{label}: expression not exported (enable includeExpressions to export it as `x`)"));
        }
    }
    out
}

fn export_keys(ex: &Ex, keys: &[Keyframe], spatial: bool, hold_only: bool, conv: &dyn Fn(&Value) -> Json) -> Vec<Json> {
    let mut out = Vec::with_capacity(keys.len());
    for (i, k) in keys.iter().enumerate() {
        let mut o = json!({"t": r6(ex.tb.frame(k.time)), "s": wrap(conv(&k.value))});
        if i + 1 == keys.len() {
            if k.out_interp == Interp::Hold || hold_only {
                o["h"] = json!(1);
            }
            out.push(o);
            continue;
        }
        if k.out_interp == Interp::Hold || hold_only || !k.value.interpolates() {
            o["h"] = json!(1);
            out.push(o);
            continue;
        }
        let (ox, oy, ix, iy) = segment_ease(keys, i, spatial);
        o["o"] = json!({"x": ox, "y": oy});
        o["i"] = json!({"x": ix, "y": iy});
        if spatial {
            let (_, to) = kf::spatial_tangents(keys, i);
            let (ti, _) = kf::spatial_tangents(keys, i + 1);
            let n = if matches!(k.value, Value::Vec2(_)) { 2 } else { conv(&k.value).as_array().map(Vec::len).unwrap_or(3).min(3) };
            o["to"] = json!(to[..n].iter().map(|v| r6(*v)).collect::<Vec<_>>());
            o["ti"] = json!(ti[..n].iter().map(|v| r6(*v)).collect::<Vec<_>>());
        }
        out.push(o);
    }
    out
}

/// Normalised ease of segment `i → i+1`: (o.x, o.y, i.x, i.y) per dimension.
fn segment_ease(keys: &[Keyframe], i: usize, spatial: bool) -> (Vec<f64>, Vec<f64>, Vec<f64>, Vec<f64>) {
    let (a, b) = (&keys[i], &keys[i + 1]);
    let dur = (secs(b.time) - secs(a.time)).max(1e-12);
    let ca = a.value.components();
    let cb = b.value.components();
    let numeric = !ca.is_empty() && ca.len() == cb.len();
    let dims = if spatial || !numeric { 1 } else { ca.len() };
    let (mut ox, mut oy, mut ix, mut iy) = (vec![], vec![], vec![], vec![]);
    for d in 0..dims {
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
                if interp == Interp::Bezier { list.first().copied().unwrap_or_default() } else { Ease { speed: 1.0 / dur, influence: 1.0 / 3.0 } }
            };
            (1.0, Some(side(a, true)), Some(side(b, false)))
        };
        let lin = Ease { speed: dv / dur, influence: 1.0 / 3.0 };
        let (eo, ei) = (eo.unwrap_or(lin), ei.unwrap_or(lin));
        // Crossing time handles must retain the independently specified influences.
        let (i0, i1) = (eo.influence.clamp(0.0, 1.0), ei.influence.clamp(0.0, 1.0));
        let (y0, y1) = if dv.abs() > 1e-12 { (eo.speed * i0 * dur / dv, 1.0 - ei.speed * i1 * dur / dv) } else { (0.0, 1.0) };
        ox.push(r6(i0));
        oy.push(r6(y0));
        ix.push(r6(1.0 - i1));
        iy.push(r6(y1));
    }
    (ox, oy, ix, iy)
}

// ---------------------------------------------------------------- import

/// Whether a Lottie property value is keyframed.
pub(crate) fn is_animated(j: &Json) -> bool {
    if j.get("a").and_then(Json::as_i64) == Some(1) {
        return true;
    }
    j.get("k").and_then(Json::as_array).and_then(|a| a.first()).is_some_and(|f| f.get("t").is_some())
}

/// Component `d` of an ease tangent field (`x` / `y` may be a number or an array).
fn comp(j: &Json, field: &str, d: usize) -> Option<f64> {
    match j.get(field)? {
        Json::Number(n) => n.as_f64(),
        Json::Array(a) => a.get(d).or(a.first()).and_then(Json::as_f64),
        _ => None,
    }
}

fn vec3_of(j: Option<&Json>) -> [f64; 3] {
    let a = j.and_then(Json::as_array);
    let g = |i: usize| a.and_then(|a| a.get(i)).and_then(Json::as_f64).unwrap_or(0.0);
    [g(0), g(1), g(2)]
}

/// Import a Lottie animatable property into `p` (value, keys, expression). `conv` turns a Lottie
/// value (as found in `k` or a key's `s`, possibly wrapped in a one-element array) into a value of
/// the property's type.
pub(crate) fn import_prop(im: &Im, j: &Json, p: &mut Property, conv: &dyn Fn(&Json) -> Option<Value>) {
    if !j.is_object() {
        if let Some(v) = conv(j) {
            p.value = v;
        }
        return;
    }
    if let Some(x) = j.get("x").and_then(Json::as_str).filter(|x| !x.trim().is_empty()) {
        p.expr = Some(Expression { text: x.to_string(), enabled: true });
    }
    let Some(k) = j.get("k") else { return };
    if !is_animated(j) {
        if let Some(v) = conv(k) {
            p.value = v;
        }
        return;
    }
    let Some(list) = k.as_array() else { return };
    let spatial = is_spatial(p);
    let mut keys: Vec<Keyframe> = Vec::with_capacity(list.len());
    let mut kept: Vec<&Json> = Vec::with_capacity(list.len());
    let mut prev_end: Option<&Json> = None;
    for kj in list {
        let t = kj.get("t").and_then(Json::as_f64).unwrap_or(0.0);
        let sv = kj.get("s").or(prev_end);
        prev_end = kj.get("e");
        let Some(v) = sv.and_then(conv) else { continue };
        let mut key = Keyframe::new(im.tb.tick(t), v);
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
        if kj.get("h").and_then(Json::as_i64) == Some(1) || p.hold_only {
            keys[i].out_interp = Interp::Hold;
            keys[i + 1].in_interp = Interp::Hold;
            continue;
        }
        if spatial {
            let to = vec3_of(kj.get("to"));
            let ti = vec3_of(kj.get("ti"));
            keys[i].spatial_out = to;
            keys[i + 1].spatial_in = ti;
        }
    }
    // Eases need the spatial tangents (arc length), so they come second.
    for i in 0..n.saturating_sub(1) {
        let Some(kj) = kept.get(i) else { break };
        if keys[i].out_interp == Interp::Hold {
            continue;
        }
        let (Some(o), Some(ii)) = (kj.get("o"), kj.get("i")) else {
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
            let ox = comp(o, "x", d).unwrap_or(1.0 / 3.0).clamp(0.0, 1.0);
            let oy = comp(o, "y", d).unwrap_or(ox);
            let ix = comp(ii, "x", d).unwrap_or(2.0 / 3.0).clamp(0.0, 1.0);
            let iy = comp(ii, "y", d).unwrap_or(ix);
            if (ox - oy).abs() > 1e-4 || (ix - iy).abs() > 1e-4 {
                linear = false;
            }
            let dv = if spatial {
                kf::spatial_segment_length(&keys, i)
            } else if numeric {
                cb[d] - ca[d]
            } else {
                1.0
            };
            let inf_o = ox.max(0.001);
            let inf_i = (1.0 - ix).max(0.001);
            outs.push(Ease { speed: oy * dv / (inf_o * dur), influence: inf_o });
            ins.push(Ease { speed: (1.0 - iy) * dv / (inf_i * dur), influence: inf_i });
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

/// Numbers of a Lottie value (`5`, `[5]`, `[1, 2, 3]`).
pub(crate) fn nums(j: &Json) -> Option<Vec<f64>> {
    match j {
        Json::Number(n) => Some(vec![n.as_f64()?]),
        Json::Array(a) => {
            // `[[x, y]]`-style wrapping.
            if a.len() == 1
                && let Some(inner @ Json::Array(_)) = a.first()
            {
                return nums(inner);
            }
            a.iter().map(Json::as_f64).collect()
        }
        _ => None,
    }
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
    let v = nums(j)?;
    let mut c = [*v.first()?, *v.get(1).unwrap_or(&0.0), *v.get(2).unwrap_or(&0.0), *v.get(3).unwrap_or(&1.0)];
    // Some files use 0–255.
    if c[..3].iter().any(|x| *x > 1.0) {
        for x in &mut c[..3] {
            *x /= 255.0;
        }
    }
    Some(Value::Color(c))
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

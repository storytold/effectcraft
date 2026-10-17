//! The Transform properties of a layer: static values and keyframes in EffectCraft's units, and
//! expressions as source text.
//!
//! Units: Anchor Point is stored as a fraction of the source size (pixels here), Scale and Opacity
//! as fractions (percent here, ease speeds scaled likewise), Position in pixels, Rotation in
//! degrees. Keyframe times are layer time and become ticks through [`TimeMap`].

use effectcraft_aep::{Interpolation, Keyframe as AepKey, KeyframeKind, Layer as AepLayer, PropGroup, PropNode, Property as AepProp, ValueSource};
use effectcraft_keyframe::{Ease, Interp, Keyframe, Value};
use effectcraft_project::{Expression, PropGroup as EcGroup, Property as EcProp};

use crate::report::{Collector, LossCategory};
use crate::time::TimeMap;

/// Which of the five properties is being converted.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    Position,
    /// Anchor Point of a source this big (pixels).
    Anchor(f64, f64),
    Scale,
    Rotation,
    Opacity,
}

impl Kind {
    /// Stored fraction to the unit shown (percent for Scale and Opacity).
    fn factor(self) -> f64 {
        match self {
            Kind::Scale | Kind::Opacity => 100.0,
            _ => 1.0,
        }
    }

    /// The value as the project stores it; `None` when a needed component is missing or not a
    /// finite number.
    fn value(self, c: &[f64]) -> Option<Value> {
        let g = |i: usize| c.get(i).copied().filter(|v| v.is_finite());
        let value = match self {
            Kind::Rotation | Kind::Opacity => Value::Scalar(g(0)? * self.factor()),
            Kind::Position => Value::Vec3([g(0)?, g(1)?, g(2).unwrap_or(0.0)]),
            Kind::Scale => Value::Vec3([g(0)? * 100.0, g(1)? * 100.0, g(2).map_or(100.0, |z| z * 100.0)]),
            Kind::Anchor(w, h) => Value::Vec3([g(0)? * w, g(1)? * h, g(2).unwrap_or(0.0)]),
        };
        sane(&value).then_some(value)
    }

    /// A tangent offset, in the same units as the value.
    fn tangent(self, c: &[f64]) -> [f64; 3] {
        let g = |i: usize| c.get(i).copied().filter(|v| v.is_finite()).unwrap_or(0.0);
        let t = match self {
            Kind::Anchor(w, h) => [g(0) * w, g(1) * h, g(2)],
            _ => [g(0), g(1), g(2)],
        };
        if t.iter().all(|v| v.abs() <= LIMIT) { t } else { [0.0; 3] }
    }
}

/// No coordinate, angle or percentage of a real project comes near this; larger numbers (and
/// NaN or infinity) mean damaged data.
const LIMIT: f64 = 1e9;

fn sane(v: &Value) -> bool {
    v.components().iter().all(|c| c.is_finite() && c.abs() <= LIMIT)
}

/// The five properties: stored match name, project property id.
const SPECS: [(&str, &str); 5] =
    [("ADBE Anchor Point", "anchor"), ("ADBE Position", "position"), ("ADBE Scale", "scale"), ("ADBE Rotate Z", "rotation"), ("ADBE Opacity", "opacity")];

/// Match names of the transform group that are always present and carry nothing in their default
/// state (the separated position dimensions and the reflection switch).
const IGNORED: [&str; 4] = ["ADBE Position_0", "ADBE Position_1", "ADBE Position_2", "ADBE Envir Appear in Reflect"];

/// Imports the transform of `layer` into `target` (the layer's `transform` group).
///
/// `comp` is the composition size in pixels and `source` the size of the layer's source, which
/// turns the stored Anchor Point fraction into pixels.
pub(crate) fn import_transform(
    layer: &AepLayer<'_, '_>,
    layer_name: &str,
    comp: (u32, u32),
    source: (u32, u32),
    tm: &TimeMap,
    target: &mut EcGroup,
    col: &mut Collector,
) {
    let Some(tr) = layer.transform(comp.0, comp.1) else { return };
    let Some(PropNode::Group(group)) = layer.properties().and_then(|p| p.get("ADBE Transform Group")) else { return };
    let (sw, sh) = (f64::from(source.0), f64::from(source.1));
    for (match_name, id) in SPECS {
        let (kind, base, source_of) = match id {
            "anchor" => (Kind::Anchor(sw, sh), Value::Vec3(tr.anchor_px(sw, sh)), tr.anchor_fraction.source),
            "position" => (Kind::Position, Value::Vec3(tr.position.value), tr.position.source),
            "scale" => (Kind::Scale, Value::Vec3(tr.scale_percent.value), tr.scale_percent.source),
            "rotation" => (Kind::Rotation, Value::Scalar(tr.rotation_degrees.value), tr.rotation_degrees.source),
            _ => (Kind::Opacity, Value::Scalar(tr.opacity_percent.value), tr.opacity_percent.source),
        };
        let Some(prop) = target.get_mut(id) else { continue };
        if sane(&base) {
            prop.value = base;
        } else {
            col.lose(LossCategory::DamagedData, format!("{layer_name}: {match_name} holds a number that is not valid, default used"));
        }
        let Some(PropNode::Property(node)) = group.get(match_name) else { continue };
        if source_of == ValueSource::Keyframed {
            import_keys(&node, kind, layer_name, match_name, tm, prop, col);
        }
        import_expression(&node, prop, col);
    }
    report_other_properties(&group, layer_name, col);
}

/// Stored transform properties that are not imported are reported.
fn report_other_properties(group: &PropGroup<'_, '_>, layer_name: &str, col: &mut Collector) {
    for entry in group.entries() {
        let name = entry.match_name.as_str();
        if SPECS.iter().any(|(m, _)| *m == name) || name == "ADBE Group End" {
            continue;
        }
        let PropNode::Property(p) = entry.node else { continue };
        // A property in its default state (`tdsb` 3) holds nothing to lose.
        let stored = p.is_animated() || p.flags() == Some(1) || p.expression().is_some();
        if stored && !(IGNORED.contains(&name) && p.flags() != Some(1) && !p.is_animated()) {
            col.lose(LossCategory::TransformProperties, format!("{layer_name}: {name}"));
        }
    }
}

/// Attaches the expression of `node` (source text and enable switch) to `prop`.
fn import_expression(node: &AepProp<'_, '_>, prop: &mut EcProp, col: &mut Collector) {
    let Some(enabled) = node.expression_enabled() else { return };
    let Some(text) = node.expression() else { return };
    prop.expr = Some(Expression { text, enabled });
    col.imported.expressions += 1;
}

fn interp(i: Option<Interpolation>, col: &mut Collector, what: &str) -> Interp {
    match i {
        Some(Interpolation::Linear) | None => Interp::Linear,
        Some(Interpolation::Bezier) => Interp::Bezier,
        Some(Interpolation::Hold) => Interp::Hold,
        Some(Interpolation::Other(code)) => {
            col.lose(LossCategory::ApproximatedKeyframes, format!("{what}: unknown interpolation {code} read as linear"));
            Interp::Linear
        }
    }
}

/// An ease that can be evaluated: finite speed, influence within 0.1 % and 100 %.
fn ease(speed: f64, influence: f64, factor: f64) -> Option<Ease> {
    let speed = speed * factor;
    (speed.is_finite() && speed.abs() <= LIMIT * 100.0 && influence.is_finite()).then(|| Ease { speed, influence: influence.clamp(0.001, 1.0) })
}

/// The temporal ease of both sides of a key: `(in, out)` lists with one entry per dimension, or
/// `None` where the data cannot be used.
fn key_eases(key: &AepKey<'_>, kind: Kind) -> Option<(Vec<Ease>, Vec<Ease>)> {
    let f = kind.factor();
    match key.kind() {
        KeyframeKind::Spatial { .. } | KeyframeKind::Plain { components: 1 } => {
            let e = key.ease()?;
            Some((vec![ease(e.in_speed, e.in_influence, f)?], vec![ease(e.out_speed, e.out_influence, f)?]))
        }
        KeyframeKind::Plain { components } => {
            // One ease per dimension (the stored slots are type-major, see
            // `Keyframe::ease_component`), so Scale keys with a different ease in each
            // dimension stay bezier.
            let all = key.ease_components()?;
            if all.len() != components {
                return None;
            }
            let mut in_e = Vec::with_capacity(components);
            let mut out_e = Vec::with_capacity(components);
            for e in &all {
                in_e.push(ease(e.in_speed, e.in_influence, f)?);
                out_e.push(ease(e.out_speed, e.out_influence, f)?);
            }
            Some((in_e, out_e))
        }
        _ => None,
    }
}

/// Keyframes of `node` into `prop`. Leaves `prop` untouched when no key is readable.
fn import_keys(node: &AepProp<'_, '_>, kind: Kind, layer_name: &str, name: &str, tm: &TimeMap, prop: &mut EcProp, col: &mut Collector) {
    let what = format!("{layer_name}: {name}");
    let Some(list) = node.keys() else { return };
    let scale = node.time_scale().filter(|s| *s != 0).unwrap_or(tm.timescale);
    let mut keys: Vec<Keyframe> = Vec::new();
    for rec in list.keyframes() {
        let (Some(units), Some(raw)) = (rec.time_units(), rec.value()) else {
            col.lose(LossCategory::DamagedData, format!("{what}: unreadable keyframe"));
            continue;
        };
        let Some(value) = kind.value(&raw) else {
            col.lose(LossCategory::DamagedData, format!("{what}: keyframe value is missing or not a number"));
            continue;
        };
        let mut key = Keyframe::new(tm.units(i64::from(units), scale), value);
        key.in_interp = interp(rec.interpolation_in(), col, &what);
        key.out_interp = interp(rec.interpolation_out(), col, &what);
        if let Some(spatial) = rec.spatial() {
            // The switch is 1 for automatic spatial tangents (EffectCraft computes them) and 0
            // once the tangents were set by hand.
            key.spatial_auto = spatial.auto_bezier_flag != 0;
            key.spatial_in = kind.tangent(&spatial.in_tangent);
            key.spatial_out = kind.tangent(&spatial.out_tangent);
        }
        key.roving = rec.is_roving().unwrap_or(false);
        key.label = rec.label().filter(|l| *l <= 16).unwrap_or(0);
        if key.in_interp == Interp::Bezier || key.out_interp == Interp::Bezier {
            let anchor = matches!(kind, Kind::Anchor(..));
            match key_eases(&rec, kind).filter(|_| !anchor) {
                Some((in_e, out_e)) => {
                    if key.in_interp == Interp::Bezier {
                        key.in_ease = in_e;
                    }
                    if key.out_interp == Interp::Bezier {
                        key.out_ease = out_e;
                    }
                }
                None => {
                    // The ease cannot be used, so the bezier sides become linear.
                    let why = if anchor { "anchor point ease is not decoded" } else { "ease data not decodable" };
                    col.lose(LossCategory::ApproximatedKeyframes, format!("{what}: bezier key read as linear ({why})"));
                    if key.in_interp == Interp::Bezier {
                        key.in_interp = Interp::Linear;
                    }
                    if key.out_interp == Interp::Bezier {
                        key.out_interp = Interp::Linear;
                    }
                }
            }
        }
        keys.push(key);
    }
    if keys.is_empty() {
        return;
    }
    keys.sort_by_key(|k| k.time);
    let before = keys.len();
    keys.dedup_by_key(|k| k.time);
    if keys.len() != before {
        col.lose(LossCategory::DamagedData, format!("{what}: keyframes at the same time"));
    }
    // The first key's incoming and the last key's outgoing ease mean nothing.
    if let Some(first) = keys.first_mut() {
        first.in_ease.clear();
    }
    if let Some(last) = keys.last_mut() {
        last.out_ease.clear();
    }
    col.imported.keyframes += keys.len();
    if let Some(first) = keys.first() {
        prop.value = first.value.clone();
    }
    prop.keys = keys;
}

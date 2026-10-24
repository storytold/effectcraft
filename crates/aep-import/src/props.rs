//! The Transform properties of a layer: static values and keyframes in EffectCraft's units, and
//! expressions as source text.
//!
//! Units: Anchor Point is stored as a fraction of the source size (pixels here; on text layers,
//! which have no source, it is stored in pixels and absent means the text origin), Scale and Opacity
//! as fractions (percent here, ease speeds scaled likewise), Position in pixels, Rotation in
//! degrees. Keyframe times are layer time and become ticks through [`TimeMap`].

use effectcraft_aep::{Interpolation, Keyframe as AepKey, KeyframeKind, Layer as AepLayer, PropGroup, PropNode, Property as AepProp, ValueSource};
use effectcraft_keyframe::{Ease, Interp, Keyframe, Value};
use effectcraft_project::{Expression, PropGroup as EcGroup, Property as EcProp};

use crate::report::{Collector, LossCategory};
use crate::time::TimeMap;

/// Which property is being converted: one of the five Transform properties, a mask property, or
/// an effect parameter (see `effects.rs`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Kind {
    Position,
    /// Anchor Point of a source this big (pixels).
    Anchor(f64, f64),
    Scale,
    Rotation,
    Opacity,
    /// Mask Feather: pixels `[x, y]`, never negative.
    MaskFeather,
    /// Mask Opacity: stored as a fraction 0..1, percent here.
    MaskOpacity,
    /// Mask Expansion: pixels.
    MaskExpansion,
    /// An effect slider or angle: the stored number times this factor.
    EffectNumber(f64),
    /// An effect point on a layer this big (pixels): stored as fractions of the layer size.
    EffectPoint(f64, f64),
    /// An effect colour: stored as ARGB 0..255, imported as straight RGBA 0..1 with alpha 1.
    EffectColor,
    /// An effect popup with this many items: stored 1-based, imported 0-based.
    EffectPopup(u32),
    /// An effect checkbox: stored 1.0 when on.
    EffectBool,
    /// Time Remapping: source seconds, as stored (319, 354).
    TimeRemap,
    /// A one-component value used as stored (shape sizes in pixels, percentages, degrees).
    Number,
    /// A two-component value used as stored (shape sizes and positions in pixels, group scale in
    /// percent).
    Vec2,
    /// A three-component percentage used as stored (text animator Scale 3D, 592); a missing third
    /// component is 100.
    Vec3Percent,
}

impl Kind {
    /// Stored value to the unit shown (percent for Scale and Opacity), also applied to ease
    /// speeds.
    fn factor(self) -> f64 {
        match self {
            Kind::Scale | Kind::Opacity | Kind::MaskOpacity => 100.0,
            Kind::EffectNumber(f) => f,
            // A fractional point's spatial speed is pixels per second divided by the layer height
            // (597, 598, 730).
            Kind::Anchor(_, h) | Kind::EffectPoint(_, h) => h,
            _ => 1.0,
        }
    }

    /// Whether a bezier ease of this kind can be imported. The ease of effect colours is only
    /// sampled as speed 0 / influence 0 (598, handled in [`import_keys`]); popups and checkboxes
    /// have no ease.
    fn ease_decoded(self) -> bool {
        !matches!(self, Kind::EffectColor | Kind::EffectPopup(_) | Kind::EffectBool)
    }

    /// The value as the project stores it; `None` when a needed component is missing or not a
    /// finite number.
    pub(crate) fn value(self, c: &[f64]) -> Option<Value> {
        let g = |i: usize| c.get(i).copied().filter(|v| v.is_finite());
        let value = match self {
            Kind::Rotation | Kind::Opacity => Value::Scalar(g(0)? * self.factor()),
            Kind::Position => Value::Vec3([g(0)?, g(1)?, g(2).unwrap_or(0.0)]),
            Kind::Scale => Value::Vec3([g(0)? * 100.0, g(1)? * 100.0, g(2).map_or(100.0, |z| z * 100.0)]),
            Kind::Anchor(w, h) => Value::Vec3([g(0)? * w, g(1)? * h, g(2).unwrap_or(0.0)]),
            Kind::MaskFeather => Value::Vec2([g(0).filter(|v| *v >= 0.0)?, g(1).filter(|v| *v >= 0.0)?]),
            Kind::MaskOpacity => Value::Scalar(g(0).filter(|v| (0.0..=1.0).contains(v))? * 100.0),
            Kind::MaskExpansion => Value::Scalar(g(0)?),
            Kind::EffectNumber(f) => Value::Scalar(g(0)? * f),
            Kind::EffectPoint(w, h) => Value::Vec2([g(0)? * w, g(1)? * h]),
            // ARGB: alpha first. After Effects colour parameters carry no editable alpha (Tint's
            // defaults even store alpha 0); EffectCraft's colour parameters are opaque.
            Kind::EffectColor => Value::Color([g(1)? / 255.0, g(2)? / 255.0, g(3)? / 255.0, 1.0]),
            Kind::EffectPopup(items) => {
                let i = g(0)?.round();
                if i < 1.0 || i > f64::from(items) {
                    return None;
                }
                // In range 1..=items, so the conversion is exact.
                Value::Enum(i as u32 - 1)
            }
            Kind::EffectBool => Value::Bool(g(0)? != 0.0),
            Kind::TimeRemap | Kind::Number => Value::Scalar(g(0)?),
            Kind::Vec2 => Value::Vec2([g(0)?, g(1)?]),
            Kind::Vec3Percent => Value::Vec3([g(0)?, g(1)?, g(2).unwrap_or(100.0)]),
        };
        sane(&value).then_some(value)
    }

    /// A tangent offset, in the same units as the value.
    fn tangent(self, c: &[f64]) -> [f64; 3] {
        let g = |i: usize| c.get(i).copied().filter(|v| v.is_finite()).unwrap_or(0.0);
        let t = match self {
            Kind::Anchor(w, h) => [g(0) * w, g(1) * h, g(2)],
            Kind::EffectPoint(w, h) => [g(0) * w, g(1) * h, 0.0],
            _ => [g(0), g(1), g(2)],
        };
        if t.iter().all(|v| v.abs() <= LIMIT) { t } else { [0.0; 3] }
    }
}

/// No sensible coordinate, angle or percentage comes near this; larger numbers (and
/// NaN or infinity) mean damaged data.
pub(crate) const LIMIT: f64 = 1e9;

pub(crate) fn sane(v: &Value) -> bool {
    v.components().iter().all(|c| c.is_finite() && c.abs() <= LIMIT)
}

/// The five properties: stored match name, project property id.
const SPECS: [(&str, &str); 5] =
    [("ADBE Anchor Point", "anchor"), ("ADBE Position", "position"), ("ADBE Scale", "scale"), ("ADBE Rotate Z", "rotation"), ("ADBE Opacity", "opacity")];

/// Match names of the transform group that are always present and carry nothing in their default
/// state (the separated position dimensions and the reflection switch).
const IGNORED: [&str; 4] = ["ADBE Position_0", "ADBE Position_1", "ADBE Position_2", "ADBE Envir Appear in Reflect"];

/// How the stored Anchor Point becomes pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AnchorUnit {
    /// A fraction of a source this big (pixels): AV layers.
    Source(u32, u32),
    /// Pixels as stored, the origin when absent: text (385) and shape (462) layers, which have no
    /// source.
    Pixels,
    /// The point of interest of a camera or light: pixels as stored (251, 253, 256), imported into
    /// `poi`; when absent the new layer's comp centre stays (255, 256 read back `[960, 540, 0]`).
    PointOfInterest,
}

/// The 3D rotations: stored match name, project property id (486, 487: plain degrees).
const ROTATIONS: [(&str, &str); 2] = [("ADBE Rotate X", "rotationX"), ("ADBE Rotate Y", "rotationY")];

/// Imports the transform of `layer` into `target` (the layer's `transform` group).
///
/// `comp` is the composition size in pixels; `anchor` says how the stored Anchor Point becomes
/// pixels.
pub(crate) fn import_transform(
    layer: &AepLayer<'_, '_>,
    layer_name: &str,
    comp: (u32, u32),
    anchor: AnchorUnit,
    tm: &TimeMap,
    target: &mut EcGroup,
    col: &mut Collector,
) {
    let Some(tr) = layer.transform(comp.0, comp.1) else { return };
    let Some(PropNode::Group(group)) = layer.properties().and_then(|p| p.get("ADBE Transform Group")) else { return };
    let (anchor_kind, anchor_px) = match anchor {
        AnchorUnit::Source(w, h) => {
            let (sw, sh) = (f64::from(w), f64::from(h));
            (Kind::Anchor(sw, sh), tr.anchor_px(sw, sh))
        }
        AnchorUnit::Pixels | AnchorUnit::PointOfInterest => (Kind::Anchor(1.0, 1.0), tr.text_anchor_px()),
    };
    let poi = anchor == AnchorUnit::PointOfInterest;
    for (match_name, id) in SPECS {
        let (kind, base, source_of) = match id {
            "anchor" => (anchor_kind, Value::Vec3(anchor_px), tr.anchor_fraction.source),
            "position" => (Kind::Position, Value::Vec3(tr.position.value), tr.position.source),
            "scale" => (Kind::Scale, Value::Vec3(tr.scale_percent.value), tr.scale_percent.source),
            "rotation" => (Kind::Rotation, Value::Scalar(tr.rotation_degrees.value), tr.rotation_degrees.source),
            _ => (Kind::Opacity, Value::Scalar(tr.opacity_percent.value), tr.opacity_percent.source),
        };
        let target_id = if poi && id == "anchor" { "poi" } else { id };
        let Some(prop) = target.get_mut(target_id) else { continue };
        if poi && id == "anchor" && source_of == ValueSource::Default {
            // No record: the default point of interest of the new layer stays.
            continue;
        }
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
    for (match_name, id) in ROTATIONS {
        let Some(PropNode::Property(node)) = group.get(match_name) else { continue };
        let Some(prop) = target.get_mut(id) else {
            col.lose(LossCategory::TransformProperties, format!("{layer_name}: {match_name}"));
            continue;
        };
        if node.is_animated() {
            import_keys(&node, Kind::Rotation, layer_name, match_name, tm, prop, col);
        } else if let Some(v) = node.static_value().and_then(|v| v.first().copied()) {
            let value = Value::Scalar(v);
            if sane(&value) {
                prop.value = value;
            } else {
                col.lose(LossCategory::DamagedData, format!("{layer_name}: {match_name} holds a number that is not valid, default used"));
            }
        }
        import_expression(&node, prop, col);
    }
    if let Some(PropNode::Other(_)) = group.get("ADBE Orientation") {
        match (layer.orientation(), target.get_mut("orientation")) {
            (Some(o), Some(prop)) if !o.animated && !o.has_expression && sane(&Value::Vec3(o.degrees)) => prop.value = Value::Vec3(o.degrees),
            // A layer without an orientation (an ambient light) loses nothing with the default.
            (Some(o), None) if !o.animated && !o.has_expression && o.degrees == [0.0; 3] => {}
            (Some(o), Some(prop)) if o.animated && !o.has_expression => import_orientation_keys(layer, layer_name, tm, prop, col),
            (Some(o), Some(_)) if o.has_expression => {
                col.lose(LossCategory::TransformProperties, format!("{layer_name}: ADBE Orientation expression (not decoded)"));
            }
            _ => col.lose(LossCategory::TransformProperties, format!("{layer_name}: ADBE Orientation")),
        }
    }
    report_other_properties(&group, layer_name, col);
}

/// The keys of a keyed Orientation (703). Linear and hold sides arrive as they are; the ease of a
/// bezier side is not sampled, so it becomes linear and is reported. EffectCraft interpolates the
/// three angles one by one, After Effects by a slerp of the rotations (a renderer difference).
fn import_orientation_keys(layer: &AepLayer<'_, '_>, layer_name: &str, tm: &TimeMap, prop: &mut EcProp, col: &mut Collector) {
    let what = format!("{layer_name}: ADBE Orientation");
    let stored = layer.orientation_keys();
    if stored.is_empty() {
        col.lose(LossCategory::TransformProperties, format!("{what} keys unreadable, the static value is used"));
        return;
    }
    let mut keys: Vec<Keyframe> = Vec::with_capacity(stored.len());
    let mut approximated = false;
    for k in &stored {
        let value = Value::Vec3(k.degrees);
        if !sane(&value) {
            col.lose(LossCategory::DamagedData, format!("{what}: keyframe value is not a number"));
            continue;
        }
        let mut key = Keyframe::new(tm.units(i64::from(k.time_units), tm.timescale), value);
        for (side, i) in [(&mut key.in_interp, k.interpolation_in), (&mut key.out_interp, k.interpolation_out)] {
            *side = match interp(Some(i), col, &what) {
                Interp::Bezier => {
                    approximated = true;
                    Interp::Linear
                }
                other => other,
            };
        }
        keys.push(key);
    }
    if approximated {
        col.lose(LossCategory::ApproximatedKeyframes, format!("{what}: bezier key read as linear (the Orientation ease is not sampled)"));
    }
    keys.sort_by_key(|k| k.time);
    keys.dedup_by_key(|k| k.time);
    col.imported.keyframes = col.imported.keyframes.saturating_add(keys.len());
    if let Some(first) = keys.first() {
        prop.value = first.value.clone();
    }
    prop.keys = keys;
}

/// Stored transform properties that are not imported are reported.
fn report_other_properties(group: &PropGroup<'_, '_>, layer_name: &str, col: &mut Collector) {
    for entry in group.entries() {
        let name = entry.match_name.as_str();
        if SPECS.iter().chain(ROTATIONS.iter()).any(|(m, _)| *m == name) || name == "ADBE Group End" {
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
pub(crate) fn import_expression(node: &AepProp<'_, '_>, prop: &mut EcProp, col: &mut Collector) {
    let Some(enabled) = node.expression_enabled() else { return };
    let Some(text) = node.expression() else { return };
    prop.expr = Some(Expression { text, enabled });
    col.imported.expressions += 1;
}

pub(crate) fn interp(i: Option<Interpolation>, col: &mut Collector, what: &str) -> Interp {
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
pub(crate) fn import_keys(node: &AepProp<'_, '_>, kind: Kind, layer_name: &str, name: &str, tm: &TimeMap, prop: &mut EcProp, col: &mut Collector) {
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
        // Colour keys switched to bezier store speed 0 and influence 0 on both sides, which After
        // Effects interpolates linearly (598).
        if kind == Kind::EffectColor
            && (key.in_interp == Interp::Bezier || key.out_interp == Interp::Bezier)
            && rec.ease().is_some_and(|e| [e.in_speed, e.in_influence, e.out_speed, e.out_influence] == [0.0; 4])
        {
            if key.in_interp == Interp::Bezier {
                key.in_interp = Interp::Linear;
            }
            if key.out_interp == Interp::Bezier {
                key.out_interp = Interp::Linear;
            }
        }
        if key.in_interp == Interp::Bezier || key.out_interp == Interp::Bezier {
            let undecoded = !kind.ease_decoded();
            match key_eases(&rec, kind).filter(|_| !undecoded) {
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
                    let why = if undecoded { "the ease of this effect parameter is not decoded" } else { "ease data not decodable" };
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

//! The Transform properties of a layer: static values and keyframes in EffectCraft's units, and
//! expressions as source text.
//!
//! Units: Anchor Point is stored as a fraction of the source size (pixels here; on text layers,
//! which have no source, it is stored in pixels and absent means the text origin), Scale and Opacity
//! as fractions (percent here, ease speeds scaled likewise), Position in pixels, Rotation in
//! degrees. Keyframe times are layer time and become ticks through [`TimeMap`].

use crate::{Interpolation, Keyframe as AepKey, KeyframeKind, Layer as AepLayer, PropGroup, PropNode, Property as AepProp, ValueSource};
use effectcraft_keyframe::{Ease, Interp, Keyframe, Value};
use effectcraft_project::build::Ids;
use effectcraft_project::{Expression, Node, ParamUi, PropGroup as EcGroup, Property as EcProp};

use crate::import::report::{Collector, LossCategory};
use crate::import::time::TimeMap;

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

/// The separated position dimensions: stored match name, project property id and name (the ids
/// and names of `prop.separateDimensions`).
const DIMENSIONS: [(&str, &str, &str); 3] =
    [("ADBE Position_0", "positionX", "X Position"), ("ADBE Position_1", "positionY", "Y Position"), ("ADBE Position_2", "positionZ", "Z Position")];

/// Bit of the `tdsb` flags of `ADBE Position` set while its dimensions are separated. In a
/// production project every transform group whose `ADBE Position_0` was written (`tdsb` 1) has it
/// (`tdsb` 0x803), and none of the thousands of groups with a plain Position (`tdsb` 1) does. The
/// combined Position then keeps stale keys and the dimensions hold the value.
const SEPARATED: u32 = 0x800;

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
/// pixels. Separated position dimensions are left to [`import_separated_position`].
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
    let separated = position_separated(&group);
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
        if separated && id == "position" {
            // The dimensions hold the value (see [`import_separated_position`]).
            continue;
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
    report_other_properties(&group, separated, layer_name, col);
}

/// True when the Position of this transform group has separated dimensions (see [`SEPARATED`]).
fn position_separated(group: &PropGroup<'_, '_>) -> bool {
    matches!(group.get("ADBE Position"), Some(PropNode::Property(p)) if p.flags().is_some_and(|f| f & SEPARATED != 0))
}

/// Imports the separated position dimensions of `layer`, if its Position has them, into `target`
/// (the layer's `transform` group, after [`import_transform`]); `ids` gives their uids.
pub(crate) fn import_separated_position(
    layer: &AepLayer<'_, '_>,
    layer_name: &str,
    tm: &TimeMap,
    target: &mut EcGroup,
    ids: &mut Ids<'_>,
    col: &mut Collector,
) {
    let Some(PropNode::Group(group)) = layer.properties().and_then(|p| p.get("ADBE Transform Group")) else { return };
    if position_separated(&group) {
        separated_position(&group, layer_name, tm, target, ids, col);
    }
}

/// Imports `ADBE Position_0/1/2` as X/Y/Z Position, laid out as `prop.separateDimensions` does
/// (right after Position, Z only on 3D layers). Each dimension is a one-component property with
/// its own keys (plain records: temporal ease in pixels per second, no spatial tangents) and
/// expression. The combined Position keeps no keys and holds the dimensions' first values. A
/// missing dimension (Z on a 2D layer) is 0 for Z and the combined Position's value for X and Y.
fn separated_position(group: &PropGroup<'_, '_>, layer_name: &str, tm: &TimeMap, target: &mut EcGroup, ids: &mut Ids<'_>, col: &mut Collector) {
    let Some(at) = target.children.iter().position(|c| c.match_id() == "position") else { return };
    let combined = target.get("position").map(|p| p.value.components()).unwrap_or_default();
    let mut first = [0.0; 3];
    let mut dims: Vec<EcProp> = Vec::with_capacity(3);
    for (d, (match_name, id, name)) in DIMENSIONS.iter().enumerate() {
        let fallback = if d == 2 { 0.0 } else { combined.get(d).copied().unwrap_or(0.0) };
        let mut prop = ids.prop(id, name, Value::Scalar(fallback)).with_ui(ParamUi::Number);
        prop.three_d_only = d == 2;
        if let Some(PropNode::Property(node)) = group.get(match_name) {
            if node.is_animated() {
                import_keys(&node, Kind::Number, layer_name, match_name, tm, &mut prop, col);
                if prop.keys.is_empty() {
                    col.lose(LossCategory::DamagedData, format!("{layer_name}: {match_name} keys unreadable, default used"));
                }
            } else {
                match node.static_value().and_then(|v| Kind::Number.value(&v)) {
                    Some(v) => prop.value = v,
                    None => col.lose(LossCategory::DamagedData, format!("{layer_name}: {match_name} holds a number that is not valid, default used")),
                }
            }
            import_expression(&node, &mut prop, col);
        }
        first[d] = prop.value.as_f64();
        dims.push(prop);
    }
    if let Some(position) = target.get_mut("position") {
        position.value = Value::Vec3(first);
        position.keys.clear();
        position.expr = None;
    }
    for (i, prop) in dims.into_iter().enumerate() {
        target.children.insert(at + 1 + i, Node::Prop(prop));
    }
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
    // Key times count in the property's time scale (a stretched layer's is scaled), as in
    // `import_keys`.
    let scale = layer.orientation_time_scale().filter(|s| *s != 0).unwrap_or(tm.timescale);
    let mut keys: Vec<Keyframe> = Vec::with_capacity(stored.len());
    let mut approximated = false;
    for k in &stored {
        let value = Value::Vec3(k.degrees);
        if !sane(&value) {
            col.lose(LossCategory::DamagedData, format!("{what}: keyframe value is not a number"));
            continue;
        }
        let mut key = Keyframe::new(tm.units(i64::from(k.time_units), scale), value);
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
fn report_other_properties(group: &PropGroup<'_, '_>, separated: bool, layer_name: &str, col: &mut Collector) {
    for entry in group.entries() {
        let name = entry.match_name.as_str();
        if SPECS.iter().chain(ROTATIONS.iter()).any(|(m, _)| *m == name) || name == "ADBE Group End" {
            continue;
        }
        if separated && DIMENSIONS.iter().any(|(m, _, _)| *m == name) {
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

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use effectcraft_project::build;
    use effectcraft_time::{TICKS_PER_SECOND, Tick};

    use super::*;
    use crate::import::time::frame_rate;
    use crate::{Body, Chunk, LIST};

    const SCALE: u32 = 30720;

    fn data(id: &[u8; 4], bytes: Vec<u8>) -> Chunk<'static> {
        Chunk { id: *id, body: Body::Data(Cow::Owned(bytes)), pad: None }
    }

    fn list(kind: &[u8; 4], children: Vec<Chunk<'static>>) -> Chunk<'static> {
        Chunk { id: LIST, body: Body::List { list_type: *kind, children }, pad: None }
    }

    fn name(n: &str) -> Chunk<'static> {
        let mut b = n.as_bytes().to_vec();
        b.resize(40, 0);
        data(b"tdmn", b)
    }

    fn tdb4(dims: u8, spatial: bool) -> Chunk<'static> {
        let mut h = vec![0u8; 124];
        h[3] = dims;
        h[7] = u8::from(spatial);
        h[12..16].copy_from_slice(&SCALE.to_be_bytes());
        data(b"tdb4", h)
    }

    fn doubles(values: &[f64], len: usize) -> Vec<u8> {
        let mut b: Vec<u8> = values.iter().flat_map(|v| v.to_be_bytes()).collect();
        b.resize(len, 0);
        b
    }

    /// A static property with `values` and these `tdsb` flags.
    fn static_prop(flags: u32, values: &[f64]) -> Chunk<'static> {
        let n = u8::try_from(values.len()).unwrap_or(0);
        list(b"tdbs", vec![data(b"tdsb", flags.to_be_bytes().to_vec()), tdb4(n, n == 3), data(b"cdat", doubles(values, 8 * values.len().max(5)))])
    }

    /// One plain one-component key: seconds, interpolation in/out (1 linear, 2 bezier, 3 hold),
    /// value, ease `[in speed, in influence, out speed, out influence]`.
    type Key = (f64, u8, u8, f64, [f64; 4]);

    /// A keyed one-component property (48-byte plain records).
    fn keyed_scalar(keys: &[Key]) -> Chunk<'static> {
        let mut ldat = Vec::new();
        for (t, i, o, v, e) in keys {
            let units = (t * f64::from(SCALE)).round() as i32;
            ldat.extend(units.to_be_bytes());
            ldat.extend([*i, *o, 0, 0]);
            ldat.extend(doubles(&[*v, e[0], e[1], e[2], e[3]], 40));
        }
        let mut lhd3 = vec![0u8; 52];
        lhd3[0..4].copy_from_slice(&0x00d0_0beeu32.to_be_bytes());
        lhd3[8..12].copy_from_slice(&u32::try_from(keys.len()).unwrap_or(0).to_be_bytes());
        lhd3[16..20].copy_from_slice(&48u32.to_be_bytes());
        list(b"tdbs", vec![data(b"tdsb", 1u32.to_be_bytes().to_vec()), tdb4(1, false), list(b"list", vec![data(b"lhd3", lhd3), data(b"ldat", ldat)])])
    }

    fn group(entries: Vec<(&str, Chunk<'static>)>) -> Chunk<'static> {
        let mut children = vec![data(b"tdsb", 1u32.to_be_bytes().to_vec())];
        for (n, c) in entries {
            children.push(name(n));
            children.push(c);
        }
        children.push(name("ADBE Group End"));
        list(b"tdgp", children)
    }

    fn tm() -> TimeMap {
        TimeMap { timescale: SCALE, frame_duration: 512, rate: frame_rate(SCALE, 512).unwrap_or_default() }
    }

    /// Runs the separated-position import on a transform whose combined Position is `[960, 540, 0]`.
    fn run(chunk: &Chunk<'static>) -> (bool, EcGroup, crate::import::ImportReport) {
        let g = PropGroup::from_chunk(chunk).unwrap_or_else(|| panic!("not a group"));
        let mut next = 1u64;
        let mut ids = Ids(&mut next);
        let mut target = build::transform(&mut ids, [0.0, 0.0], [960.0, 540.0]);
        let mut col = Collector::default();
        let separated = position_separated(&g);
        if separated {
            separated_position(&g, "L", &tm(), &mut target, &mut ids, &mut col);
        }
        report_other_properties(&g, separated, "L", &mut col);
        (separated, target, col.finish())
    }

    fn ids_in_order(g: &EcGroup) -> Vec<&str> {
        g.children.iter().map(|c| c.match_id()).collect()
    }

    fn at(p: &EcProp, seconds: f64) -> f64 {
        p.value_at(Tick::from_seconds_f64(seconds)).as_f64()
    }

    #[test]
    fn static_separated_3d_position() {
        let c = group(vec![
            ("ADBE Position", static_prop(0x803, &[1.0, 2.0, 3.0])),
            ("ADBE Position_0", static_prop(1, &[100.0])),
            ("ADBE Position_1", static_prop(1, &[200.0])),
            ("ADBE Position_2", static_prop(1, &[-300.0])),
        ]);
        let (separated, tr, report) = run(&c);
        assert!(separated);
        assert_eq!(&ids_in_order(&tr)[..5], ["anchor", "position", "positionX", "positionY", "positionZ"]);
        let v = |m: &str| tr.get(m).map(|p| p.value.as_f64()).unwrap_or(f64::NAN);
        assert_eq!((v("positionX"), v("positionY"), v("positionZ")), (100.0, 200.0, -300.0));
        assert_eq!(tr.get("position").map(|p| p.value.clone()), Some(Value::Vec3([100.0, 200.0, -300.0])));
        assert!(tr.get("positionZ").is_some_and(|p| p.three_d_only) && tr.get("positionX").is_some_and(|p| !p.three_d_only));
        assert!(report.losses.is_empty(), "{:?}", report.losses);
    }

    #[test]
    fn keyed_dimension_keeps_ease_and_hold() {
        // X eases from 0 to 1000 over 2 s (out speed 0 / influence 75 %, in speed 0 / 50 %), then
        // holds; Y is static; Z is keyed linearly.
        let c = group(vec![
            ("ADBE Position", static_prop(0x803, &[0.0, 0.0, 0.0])),
            (
                "ADBE Position_0",
                keyed_scalar(&[(0.0, 2, 2, 0.0, [0.0, 0.0, 0.0, 0.75]), (2.0, 2, 3, 1000.0, [0.0, 0.5, 0.0, 0.0]), (3.0, 3, 3, -50.0, [0.0; 4])]),
            ),
            ("ADBE Position_1", static_prop(1, &[540.0])),
            ("ADBE Position_2", keyed_scalar(&[(0.0, 1, 1, 0.0, [0.0; 4]), (1.0, 1, 1, 100.0, [0.0; 4])])),
        ]);
        let (_, tr, report) = run(&c);
        assert!(report.losses.is_empty(), "{:?}", report.losses);
        let x = tr.get("positionX").unwrap_or_else(|| panic!("no X"));
        assert_eq!(x.keys.len(), 3);
        assert_eq!(x.keys[1].time, Tick(2 * TICKS_PER_SECOND));
        assert_eq!((x.keys[0].out_interp, x.keys[1].in_interp, x.keys[1].out_interp), (Interp::Bezier, Interp::Bezier, Interp::Hold));
        assert_eq!(x.keys[0].out_ease, vec![Ease { speed: 0.0, influence: 0.75 }]);
        assert_eq!(x.keys[1].in_ease, vec![Ease { speed: 0.0, influence: 0.5 }]);
        // Eased: slow at both ends, so the midpoint value is not the linear 500 but still between.
        let mid = at(x, 1.0);
        assert!(mid > 0.0 && mid < 1000.0 && (mid - 500.0).abs() > 1.0, "{mid}");
        // Speed 0 out of the first key: slower than the linear 50 px after 0.1 s.
        assert!(at(x, 0.1) < 50.0);
        assert_eq!(at(x, 2.5), 1000.0);
        assert_eq!(at(x, 3.5), -50.0);
        let z = tr.get("positionZ").unwrap_or_else(|| panic!("no Z"));
        assert!((at(z, 0.5) - 50.0).abs() < 1e-9);
        assert_eq!(tr.get("positionY").map(|p| p.value.as_f64()), Some(540.0));
        // The combined Position holds the first values and no keys.
        let p = tr.get("position").unwrap_or_else(|| panic!("no Position"));
        assert!(p.keys.is_empty());
        assert_eq!(p.value, Value::Vec3([0.0, 540.0, 0.0]));
    }

    #[test]
    fn two_d_layer_without_z_dimension() {
        let c = group(vec![
            ("ADBE Position", static_prop(0x803, &[960.0, 540.0, 0.0])),
            ("ADBE Position_0", static_prop(1, &[10.0])),
            ("ADBE Position_1", keyed_scalar(&[(0.0, 1, 1, 20.0, [0.0; 4]), (1.0, 1, 1, 40.0, [0.0; 4])])),
        ]);
        let (_, tr, report) = run(&c);
        assert!(report.losses.is_empty(), "{:?}", report.losses);
        assert_eq!(tr.get("positionZ").map(|p| (p.value.as_f64(), p.keys.len())), Some((0.0, 0)));
        assert!((at(tr.get("positionY").unwrap_or_else(|| panic!("no Y")), 0.5) - 30.0).abs() < 1e-9);
    }

    #[test]
    fn default_dimensions_under_a_plain_position_are_not_imported() {
        // Every transform group carries the dimensions; under a plain Position they are inert.
        let c = group(vec![
            ("ADBE Position", static_prop(1, &[960.0, 540.0, 0.0])),
            ("ADBE Position_0", static_prop(3, &[0.0])),
            ("ADBE Position_1", static_prop(3, &[0.0])),
        ]);
        let (separated, tr, report) = run(&c);
        assert!(!separated);
        assert!(tr.get("positionX").is_none());
        assert!(report.losses.is_empty(), "{:?}", report.losses);
    }

    #[test]
    fn stale_combined_keys_are_dropped() {
        // A separated Position keeps the keys it had before the split.
        let mut keyed_combined = static_prop(0x803, &[0.0, 0.0, 0.0]);
        if let Body::List { children, .. } = &mut keyed_combined.body {
            children.retain(|c| !c.is(b"cdat"));
        }
        let c = group(vec![
            ("ADBE Position", keyed_combined),
            ("ADBE Position_0", static_prop(1, &[5.0])),
            ("ADBE Position_1", static_prop(1, &[6.0])),
            ("ADBE Position_2", static_prop(3, &[0.0])),
        ]);
        let (_, tr, _) = run(&c);
        let p = tr.get("position").unwrap_or_else(|| panic!("no Position"));
        assert!(p.keys.is_empty() && p.expr.is_none());
        assert_eq!(p.value, Value::Vec3([5.0, 6.0, 0.0]));
    }
}

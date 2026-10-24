//! Static transform values with their units, and the "absent = default" rule.
//!
//! After Effects stores a property only when it differs from the default it computes for the layer
//! and composition (sparse storage), so a reader has to supply the defaults: Position defaults to
//! the composition centre `[width / 2, height / 2, 0]` (verified: after the composition is resized
//! without moving the layer, the old centre appears as a stored value), Anchor Point to the layer
//! centre (`0.5, 0.5` of the layer **source** size), Scale to 100 %, Rotation to 0 and Opacity to
//! 100 %. These defaults are verified for AV layers (solids); cameras and lights use other
//! conventions (see the `camera` module).

use crate::typed::{Layer, Project, PropGroup, PropNode};

/// Where a [`Tracked`] value came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueSource {
    /// The property is absent from the file: the value is the computed default.
    Default,
    /// The property stores a static value.
    Stored,
    /// The property has keyframes: the value is that of the first keyframe.
    Keyframed,
    /// The property is present but its value could not be read: the value is the default.
    Unreadable,
}

/// A value and where it came from.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tracked<T> {
    pub value: T,
    pub source: ValueSource,
}

/// The five basic transform properties of an AV layer, in the units After Effects shows.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transform {
    /// Pixels `[x, y, z]`.
    pub position: Tracked<[f64; 3]>,
    /// Stored as a fraction of the layer's **source** size for x and y (`10, 20` px on a 100x100
    /// solid in a 1920x1080 composition is `0.1, 0.2`; on a 400x300 solid `100, 50` is `0.25, 1/6`)
    /// and in pixels for z; see [`Transform::anchor_px`] and [`Project::layer_anchor_px`].
    pub anchor_fraction: Tracked<[f64; 3]>,
    /// Percent `[x, y, z]` (stored as fractions: 50 % is `0.5`).
    pub scale_percent: Tracked<[f64; 3]>,
    /// Degrees (Rotation, the Z rotation).
    pub rotation_degrees: Tracked<f64>,
    /// Percent (stored as a fraction: 25 % is `0.25`).
    pub opacity_percent: Tracked<f64>,
}

impl Transform {
    /// The anchor point in pixels for a layer whose source has this size.
    pub fn anchor_px(&self, width: f64, height: f64) -> [f64; 3] {
        let [x, y, z] = self.anchor_fraction.value;
        [x * width, y * height, z]
    }

    /// The anchor point of a **text** layer in layer pixels. A text layer has no source, so the
    /// stored value is not a fraction: it is pixels as stored (385: Anchor Point `10, 20` is stored
    /// `10, 20`), and an
    /// absent anchor is the text origin `[0, 0, 0]` (After Effects' default for new text, absent in
    /// every other text sample 110-114, 243-248, 334-340, 386).
    pub fn text_anchor_px(&self) -> [f64; 3] {
        match self.anchor_fraction.source {
            ValueSource::Stored | ValueSource::Keyframed => self.anchor_fraction.value,
            ValueSource::Default | ValueSource::Unreadable => [0.0; 3],
        }
    }
}

/// Reads `N` components from the property `name` of `group` and multiplies them by `scale` (stored
/// fractions to percent); `default` is in the output units and used when the property is absent.
pub(crate) fn read<const N: usize>(group: &PropGroup<'_, '_>, name: &str, default: [f64; N], scale: f64) -> Tracked<[f64; N]> {
    let unreadable = Tracked { value: default, source: ValueSource::Unreadable };
    let property = match group.get(name) {
        None => return Tracked { value: default, source: ValueSource::Default },
        Some(PropNode::Property(p)) => p,
        Some(PropNode::Group(_) | PropNode::Other(_)) => return unreadable,
    };
    let (values, source) =
        if property.is_animated() { (property.first_key_value(), ValueSource::Keyframed) } else { (property.static_value(), ValueSource::Stored) };
    let Some(values) = values else { return unreadable };
    if values.len() < N {
        return unreadable;
    }
    let mut out = default;
    for (slot, v) in out.iter_mut().zip(&values) {
        *slot = *v * scale;
    }
    Tracked { value: out, source }
}

/// Like [`read`] but `None` when the property is absent or unreadable (for properties whose default
/// is not known), keeping the value's source.
pub(crate) fn read_opt<const N: usize>(group: &PropGroup<'_, '_>, name: &str, scale: f64) -> Option<Tracked<[f64; N]>> {
    let tracked = read(group, name, [0.0; N], scale);
    matches!(tracked.source, ValueSource::Stored | ValueSource::Keyframed).then_some(tracked)
}

impl<'f, 'a> Project<'f, 'a> {
    /// The anchor point of an AV layer in pixels: the stored fraction times the size of the layer's
    /// source (a solid or footage item, or a precomp's composition), z unchanged. `None` when the
    /// layer has no source (shape, text, camera, light) or no Transform group.
    pub fn layer_anchor_px(&self, layer: &Layer<'f, 'a>) -> Option<Tracked<[f64; 3]>> {
        let (width, height) = self.layer_source_size(layer)?;
        let Some(PropNode::Group(group)) = layer.properties()?.get("ADBE Transform Group") else { return None };
        let fraction = read(&group, "ADBE Anchor Point", [0.5, 0.5, 0.0], 1.0);
        let [x, y, z] = fraction.value;
        Some(Tracked { value: [x * f64::from(width), y * f64::from(height), z], source: fraction.source })
    }
}

impl<'f, 'a> Layer<'f, 'a> {
    /// The transform of an AV layer in a composition of this size (pixels), with absent
    /// properties replaced by their defaults. `None` when the layer has no Transform group.
    pub fn transform(&self, comp_width: u32, comp_height: u32) -> Option<Transform> {
        let Some(PropNode::Group(group)) = self.properties()?.get("ADBE Transform Group") else { return None };
        let centre = [f64::from(comp_width) / 2.0, f64::from(comp_height) / 2.0, 0.0];
        let rotation = read(&group, "ADBE Rotate Z", [0.0], 1.0);
        let opacity = read(&group, "ADBE Opacity", [100.0], 100.0);
        Some(Transform {
            position: read(&group, "ADBE Position", centre, 1.0),
            anchor_fraction: read(&group, "ADBE Anchor Point", [0.5, 0.5, 0.0], 1.0),
            scale_percent: read(&group, "ADBE Scale", [100.0, 100.0, 100.0], 100.0),
            rotation_degrees: Tracked { value: rotation.value[0], source: rotation.source },
            opacity_percent: Tracked { value: opacity.value[0], source: opacity.source },
        })
    }
}

/// One key of a layer's Time Remapping property: the layer time of the key and the source time it
/// maps to, both in seconds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TimeRemapKey {
    /// The key time in the property's time units.
    pub time_units: i32,
    /// The key time in seconds (layer time).
    pub time_seconds: Option<f64>,
    /// The value in seconds (the plain record's value at byte 8).
    pub value_seconds: f64,
}

/// The state of a layer's `ADBE Time Remapping` property. Every layer has the property; while the
/// remap is off it is an inert record (`tdsb` 3, static value 0, no keys). Enabling the remap turns
/// it into a keyed property (`tdsb` 1, `tdb4` byte 68 = 1, no `cdat`) with two keys: the layer's in
/// and out time mapped to the source time (319: 0 s and 5 s; 354: 0 s and 0.1 s).
#[derive(Debug, Clone, PartialEq)]
pub struct TimeRemap {
    /// True when the property has keys.
    pub enabled: bool,
    pub keys: Vec<TimeRemapKey>,
}

impl<'f, 'a> Layer<'f, 'a> {
    /// The Time Remapping state; `None` when the layer has no such property.
    pub fn time_remap(&self) -> Option<TimeRemap> {
        let Some(PropNode::Property(p)) = self.properties()?.get("ADBE Time Remapping") else { return None };
        let scale = p.time_scale().unwrap_or(0);
        let keys = match p.keys() {
            Some(list) => list
                .keyframes()
                .filter_map(|k| Some(TimeRemapKey { time_units: k.time_units()?, time_seconds: k.seconds(scale), value_seconds: k.scalar_value()? }))
                .collect(),
            None => Vec::new(),
        };
        Some(TimeRemap { enabled: p.is_animated(), keys })
    }
}

/// The Orientation of a 3D layer (`ADBE Orientation`, 488): a `LIST otst` holding a `tdbs` and a
/// `LIST otky` whose `otda` is three big-endian `f64` degrees `[x, y, z]`; absent at `[0, 0, 0]`
/// (485-487).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Orientation {
    /// Degrees `[x, y, z]`: the static value (for a keyed Orientation the first `otda`, whose
    /// meaning is not sampled).
    pub degrees: [f64; 3],
    /// The `tdbs` of the `otst` has keys (see [`Layer::orientation_keys`]).
    pub animated: bool,
    /// The `tdbs` of the `otst` has an expression.
    pub has_expression: bool,
}

impl<'f, 'a> Layer<'f, 'a> {
    /// The Orientation; `None` when it is absent (the default `[0, 0, 0]`) or unreadable.
    pub fn orientation(&self) -> Option<Orientation> {
        let Some(PropNode::Group(group)) = self.properties()?.get("ADBE Transform Group") else { return None };
        let Some(PropNode::Other(otst)) = group.get("ADBE Orientation") else { return None };
        if !otst.is_list(b"otst") {
            return None;
        }
        let property = otst.child_list(b"tdbs").and_then(crate::typed::Property::from_chunk);
        let otda = otst.child_list(b"otky")?.child(b"otda")?.bytes()?;
        let mut degrees = [0.0; 3];
        for (i, slot) in degrees.iter_mut().enumerate() {
            let b = otda.get(i * 8..i * 8 + 8)?;
            *slot = f64::from_be_bytes(b.try_into().ok()?);
        }
        Some(Orientation {
            degrees,
            animated: property.as_ref().is_some_and(|p| p.is_animated()),
            has_expression: property.as_ref().is_some_and(|p| p.expression().is_some()),
        })
    }

    /// The keyframes of a keyed Orientation (703): the `tdbs` of the `otst` holds one 80-byte
    /// record per key (time, interpolation sides, flags; the rest is not decoded) and `otky` one
    /// `otda` (three `f64` degrees) per key, in the same order. Empty when the Orientation is not
    /// keyed or the counts differ. After Effects interpolates between the keys by a quaternion slerp
    /// of the rotations `Rx Ry Rz`, not per component (703's values at 0.5 / 1 / 1.5 s).
    pub fn orientation_keys(&self) -> Vec<OrientationKey> {
        let Some(PropNode::Group(group)) = self.properties().and_then(|p| p.get("ADBE Transform Group")) else { return Vec::new() };
        let Some(PropNode::Other(otst)) = group.get("ADBE Orientation") else { return Vec::new() };
        let Some(list) = otst.child_list(b"tdbs").and_then(crate::typed::Property::from_chunk).and_then(|p| p.keys()) else { return Vec::new() };
        let values: Vec<Option<[f64; 3]>> = otst
            .child_list(b"otky")
            .map(|k| {
                k.children()
                    .iter()
                    .filter(|c| c.is(b"otda"))
                    .map(|c| {
                        let b = c.bytes()?;
                        let at = |i: usize| b.get(i * 8..i * 8 + 8).and_then(|s| s.try_into().ok()).map(f64::from_be_bytes);
                        Some([at(0)?, at(1)?, at(2)?])
                    })
                    .collect()
            })
            .unwrap_or_default();
        let keys: Vec<_> = list.keyframes().collect();
        if keys.len() != values.len() {
            return Vec::new();
        }
        keys.iter()
            .zip(values)
            .filter_map(|(k, v)| {
                Some(OrientationKey {
                    time_units: k.time_units()?,
                    interpolation_in: k.interpolation_in()?,
                    interpolation_out: k.interpolation_out()?,
                    degrees: v?,
                })
            })
            .collect()
    }
}

/// A keyframe of a keyed Orientation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OrientationKey {
    /// The time in the property's time units (layer time).
    pub time_units: i32,
    pub interpolation_in: crate::Interpolation,
    pub interpolation_out: crate::Interpolation,
    /// Degrees `[x, y, z]`.
    pub degrees: [f64; 3],
}

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

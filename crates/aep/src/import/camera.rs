//! Camera and light options (docs/aep-format.md 3.12; cases 251-256, 341-345, 535-537).
//!
//! The transform of a camera or light goes through [`crate::import::props::import_transform`] (the point
//! of interest is the Anchor Point record, in pixels); this module imports the Camera Options and
//! Light Options groups. A property is imported from its record (static value or keys, and an
//! expression); a stored property that is not mapped is reported.

use crate::{Layer as AepLayer, PropGroup as AepGroup, PropNode};
use effectcraft_keyframe::Value;
use effectcraft_project::PropGroup;

use crate::import::props::{Kind, import_expression, import_keys, sane};
use crate::import::report::{Collector, LossCategory};
use crate::import::time::TimeMap;

/// The default aperture of After Effects' 50 mm camera in a 1920 px wide composition (256 read
/// back 25.309 336 332 958 4); scaled with the composition width like the zoom (not sampled for
/// other widths).
const APERTURE_1920: f64 = 25.309_336_332_958_4;

/// Camera Options: match name, project property id, kind (341, 342). Blur Level is stored in
/// percent like the project's `blurLevel` (100 % is stored as 100.0; production cameras store
/// other values, such as 150.0, that change the depth of field).
const CAMERA: [(&str, &str, Kind); 5] = [
    ("ADBE Camera Zoom", "zoom", Kind::Number),
    ("ADBE Camera Depth of Field", "dof", Kind::EffectBool),
    ("ADBE Camera Focus Distance", "focusDistance", Kind::Number),
    ("ADBE Camera Aperture", "aperture", Kind::Number),
    ("ADBE Camera Blur Level", "blurLevel", Kind::Number),
];

/// Light Options of a spot or point light (254, 343-345). Every light stores Casts Shadows as a
/// set record holding 0 (off, 252, 254, 255).
const LIGHT: [(&str, &str, Kind); 5] = [
    ("ADBE Casts Shadows", "castsShadows", Kind::EffectBool),
    ("ADBE Light Intensity", "intensity", Kind::Number),
    ("ADBE Light Color", "color", Kind::EffectColor),
    ("ADBE Light Cone Angle", "coneAngle", Kind::Number),
    ("ADBE Light Cone Feather 2", "coneFeather", Kind::Number),
];

/// Imports the Camera Options of `layer` into `options` (the layer's `cameraOptions` group).
/// `comp_width` scales the default aperture.
pub(crate) fn import_camera_options(layer: &AepLayer<'_, '_>, what: &str, comp_width: u32, tm: &TimeMap, options: &mut PropGroup, col: &mut Collector) {
    // Absent Zoom: the auto Position stores `[0, 0, -zoom]` (251, 256, 341); otherwise the new
    // layer's default (the 50 mm preset for this width) stays.
    let auto_z = layer.camera_position().filter(|p| p.auto_state).map(|p| -p.stored[2]).filter(|z| z.is_finite() && *z > 0.0);
    if let (Some(z), Some(p)) = (auto_z, options.prop_mut("zoom")) {
        p.value = Value::Scalar(z);
    }
    let zoom = match options.prop("zoom").map(|p| &p.value) {
        Some(Value::Scalar(z)) => Some(*z),
        _ => None,
    };
    // Absent Focus Distance: the zoom (256 reads back 2666.67 for both); absent Aperture: After
    // Effects' 50 mm value (256).
    if let (Some(z), Some(p)) = (zoom, options.prop_mut("focusDistance")) {
        p.value = Value::Scalar(z);
    }
    if let Some(p) = options.prop_mut("aperture") {
        p.value = Value::Scalar(APERTURE_1920 * f64::from(comp_width) / 1920.0);
    }
    let Some(PropNode::Group(group)) = layer.properties().and_then(|p| p.get("ADBE Camera Options Group")) else { return };
    import_group(&group, &CAMERA, what, tm, options, col);
}

/// Imports the Light Options of `layer` into `options` (the layer's `lightOptions` group).
pub(crate) fn import_light_options(layer: &AepLayer<'_, '_>, what: &str, tm: &TimeMap, options: &mut PropGroup, col: &mut Collector) {
    let Some(PropNode::Group(group)) = layer.properties().and_then(|p| p.get("ADBE Light Options Group")) else { return };
    import_group(&group, &LIGHT, what, tm, options, col);
}

/// A record that holds something: keys, a set value (`tdsb` 1) or an expression. Point lights
/// store their cone as `tdsb` 3 records with the defaults (252), which hold nothing.
fn is_stored(p: &crate::Property<'_, '_>) -> bool {
    p.is_animated() || p.flags() == Some(1) || p.expression().is_some()
}

fn import_group(group: &AepGroup<'_, '_>, table: &[(&str, &str, Kind)], what: &str, tm: &TimeMap, options: &mut PropGroup, col: &mut Collector) {
    for entry in group.entries() {
        let name = entry.match_name.as_str();
        if name == "ADBE Group End" {
            continue;
        }
        let PropNode::Property(node) = entry.node else { continue };
        if !is_stored(&node) {
            continue;
        }
        let mapped = table.iter().find(|(m, _, _)| *m == name).and_then(|(_, id, kind)| options.prop_mut(id).map(|p| (p, *kind)));
        let Some((prop, kind)) = mapped else {
            col.lose(LossCategory::TransformProperties, format!("{what}: {name} not imported"));
            continue;
        };
        if node.is_animated() {
            if kind == Kind::EffectBool {
                col.lose(LossCategory::TransformProperties, format!("{what}: the keyframes of {name} are not imported (not sampled)"));
            } else {
                import_keys(&node, kind, what, name, tm, prop, col);
            }
        } else if let Some(raw) = node.static_value() {
            match kind.value(&raw).filter(sane) {
                Some(v) => prop.value = v,
                None => col.lose(LossCategory::DamagedData, format!("{what}: {name} holds a value that is not valid, default used")),
            }
        }
        import_expression(&node, prop, col);
    }
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use effectcraft_project::build::{self, Ids};

    use super::*;
    use crate::import::time::frame_rate;
    use crate::{Body, Chunk, LIST};

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

    /// A static one-component property holding `v` (`tdsb` 1, as cameras store it).
    fn scalar(v: f64) -> Chunk<'static> {
        let mut h = vec![0u8; 124];
        h[3] = 1;
        h[12..16].copy_from_slice(&30720u32.to_be_bytes());
        let mut c = v.to_be_bytes().to_vec();
        c.resize(40, 0);
        list(b"tdbs", vec![data(b"tdsb", 1u32.to_be_bytes().to_vec()), data(b"tdb4", h), data(b"cdat", c)])
    }

    #[test]
    fn blur_level_is_imported_in_percent() {
        let mut children = vec![data(b"tdsb", 1u32.to_be_bytes().to_vec())];
        for (n, v) in
            [("ADBE Camera Depth of Field", 1.0), ("ADBE Camera Focus Distance", 3000.0), ("ADBE Camera Aperture", 120.0), ("ADBE Camera Blur Level", 150.0)]
        {
            children.push(name(n));
            children.push(scalar(v));
        }
        children.push(name("ADBE Group End"));
        let chunk = list(b"tdgp", children);
        let g = AepGroup::from_chunk(&chunk).unwrap_or_else(|| panic!("not a group"));
        let mut next = 1u64;
        let mut options = build::camera_options_for(&mut Ids(&mut next), 2000.0, 1920.0);
        let tm = TimeMap { timescale: 30720, frame_duration: 512, rate: frame_rate(30720, 512).unwrap_or_default() };
        let mut col = Collector::default();
        import_group(&g, &CAMERA, "Camera 1", &tm, &mut options, &mut col);
        let v = |id: &str| options.prop(id).map(|p| p.value.clone());
        assert_eq!(v("blurLevel"), Some(Value::Scalar(150.0)));
        assert_eq!(v("aperture"), Some(Value::Scalar(120.0)));
        assert_eq!(v("focusDistance"), Some(Value::Scalar(3000.0)));
        let report = col.finish();
        assert!(report.losses.is_empty(), "{:?}", report.losses);
    }
}

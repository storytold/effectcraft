//! Camera and light options (docs/aep-format.md 3.12; cases 251-256, 341-345, 535-537).
//!
//! The transform of a camera or light goes through [`crate::props::import_transform`] (the point
//! of interest is the Anchor Point record, in pixels); this module imports the Camera Options and
//! Light Options groups. A property is imported from its record (static value or keys, and an
//! expression); a stored property that is not mapped is reported.

use effectcraft_aep::{Layer as AepLayer, PropGroup as AepGroup, PropNode};
use effectcraft_keyframe::Value;
use effectcraft_project::PropGroup;

use crate::props::{Kind, import_expression, import_keys, sane};
use crate::report::{Collector, LossCategory};
use crate::time::TimeMap;

/// The default aperture of After Effects' 50 mm camera in a 1920 px wide composition (256 read
/// back 25.309 336 332 958 4); scaled with the composition width like the zoom (not sampled for
/// other widths).
const APERTURE_1920: f64 = 25.309_336_332_958_4;

/// Camera Options: match name, project property id, kind (341, 342).
const CAMERA: [(&str, &str, Kind); 4] = [
    ("ADBE Camera Zoom", "zoom", Kind::Number),
    ("ADBE Camera Depth of Field", "dof", Kind::EffectBool),
    ("ADBE Camera Focus Distance", "focusDistance", Kind::Number),
    ("ADBE Camera Aperture", "aperture", Kind::Number),
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
fn is_stored(p: &effectcraft_aep::Property<'_, '_>) -> bool {
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
        // Blur Level at 100 (the default) is neutral: the exporter writes it explicitly and After
        // Effects reads it back as 100 % (export read-back o1); other values are not sampled.
        if name == "ADBE Camera Blur Level"
            && !node.is_animated()
            && node.expression().is_none()
            && node.static_value().and_then(|raw| Kind::Number.value(&raw)) == Some(Value::Scalar(100.0))
        {
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

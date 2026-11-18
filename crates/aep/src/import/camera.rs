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

/// Imports the Camera Options of the camera `layer` into its property tree `props`, then derives
/// a Position the file leaves out ([`derive_camera_position`]). `default_position` is the new
/// layer's Position before the transform was imported.
pub(crate) fn import_camera(
    layer: &AepLayer<'_, '_>,
    what: &str,
    comp_width: u32,
    tm: &TimeMap,
    default_position: Option<Value>,
    props: &mut PropGroup,
    col: &mut Collector,
) {
    if let Some(options) = props.sub_mut("cameraOptions") {
        import_camera_options(layer, what, comp_width, tm, options, col);
    }
    derive_camera_position(layer, what, default_position, props, col);
}

/// A camera without a Position record. After Effects 2026 leaves the record out when the
/// Position is the point of interest moved back by the zoom (lab file `lab_campos`: cameras set to
/// `[200, 100, -1000]` with the point of interest `[200, 100, 0]` and Zoom 1000 store no record and
/// read back that Position; one set to `[200, 100, -1000]` with Zoom 800 stores it; only sampled
/// with the point of interest at z = 0). The importer therefore sets `[x, y, z - zoom]` from the
/// imported point of interest and Zoom. When that cannot be done it puts back the new layer's
/// Position (`default_position`) and reports why ([`underivable`]; a Zoom that is not positive;
/// a result that is not a valid number). Runs after the transform and the Camera Options are
/// imported; `props` is the layer's property tree.
fn derive_camera_position(layer: &AepLayer<'_, '_>, what: &str, default_position: Option<Value>, props: &mut PropGroup, col: &mut Collector) {
    let root = layer.properties();
    let Some(PropNode::Group(transform)) = root.as_ref().and_then(|p| p.get("ADBE Transform Group")) else {
        col.lose(LossCategory::TransformProperties, format!("{what}: no transform group, so no Position can be derived; default Position used"));
        return;
    };
    if transform.get("ADBE Position").is_some() {
        return;
    }
    let zoom = match root.as_ref().and_then(|p| p.get("ADBE Camera Options Group")) {
        Some(PropNode::Group(options)) => options.get("ADBE Camera Zoom"),
        _ => None,
    };
    let fixed = |path: &str| props.prop(path).filter(|p| p.keys.is_empty() && p.expr.is_none()).map(|p| p.value.clone());
    let derived = underivable(&transform, zoom).map_or_else(
        || match (fixed("transform/poi"), fixed("cameraOptions/zoom")) {
            (Some(Value::Vec3(poi)), Some(Value::Scalar(zoom))) if zoom > 0.0 => {
                let position = Value::Vec3([poi[0], poi[1], poi[2] - zoom]);
                if sane(&position) {
                    Ok(position)
                } else {
                    Err((LossCategory::DamagedData, "the Position derived from the point of interest and Zoom is not valid"))
                }
            }
            _ => Err((LossCategory::TransformProperties, "no Position record, and it cannot be derived from the point of interest and Zoom")),
        },
        Err,
    );
    let Some(target) = props.prop_mut("transform/position") else { return };
    match derived {
        Ok(position) => target.value = position,
        Err((category, why)) => {
            if let Some(d) = default_position {
                target.value = d;
            }
            col.lose(category, format!("{what}: {why}; default Position used"));
        }
    }
}

/// Why the records of a camera without a Position record give no Position, if they don't: X, Y
/// and Z stored as separate records (they hold something, not the zero defaults After Effects
/// writes next to the 3D record), or a point of interest or Zoom record that animates (keys,
/// even ones that could not be imported, or an expression) or whose value cannot be read. An
/// absent record is fine: the default it stands for is what was imported.
fn underivable<'f, 'a>(transform: &AepGroup<'f, 'a>, zoom: Option<PropNode<'f, 'a>>) -> Option<(LossCategory, &'static str)> {
    fn record<'f, 'a>(node: Option<PropNode<'f, 'a>>) -> Option<crate::Property<'f, 'a>> {
        match node {
            Some(PropNode::Property(p)) => Some(p),
            _ => None,
        }
    }
    let separated = ["ADBE Position_0", "ADBE Position_1", "ADBE Position_2"].into_iter().filter_map(|n| record(transform.get(n))).any(|p| is_stored(&p));
    if separated {
        return Some((LossCategory::TransformProperties, "the Position is stored as separate X, Y and Z records, which are not imported"));
    }
    for p in [record(transform.get("ADBE Anchor Point")), record(zoom)].into_iter().flatten() {
        if p.is_animated() || p.expression().is_some() {
            return Some((LossCategory::TransformProperties, "no Position record, and the point of interest or Zoom it is derived from is animated"));
        }
        if p.static_value().is_none() {
            return Some((LossCategory::DamagedData, "no Position record, and the point of interest or Zoom it is derived from is unreadable"));
        }
    }
    None
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

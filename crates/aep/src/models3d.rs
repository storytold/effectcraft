//! AE 3D model layers remain editable model references, including offline footage.

use super::*;
use effectcraft_project::Layer;

pub(super) fn layer_source(ir: &Json, project: &Project, item_ids: &BTreeMap<i64, ItemId>, comp_id: i64, losses: &mut Vec<String>) -> Option<LayerSource> {
    let source_id = integer(ir, &["source_id"]);
    let item = source_id.and_then(|id| item_ids.get(&id)).copied();
    match item.and_then(|id| project.item(id)) {
        Some(source) if matches!(&source.kind, ItemKind::Footage(_)) => Some(LayerSource::Model { item: source.id }),
        _ => {
            losses.push(format!("comp {comp_id}: model layer {} has missing model source {:?}", string(ir, &["name"]).unwrap_or("?"), source_id));
            None
        }
    }
}

/// The common importer already handles 3D transforms, timing, keyframes, and AE-only
/// properties. Normalize AE's model material flags onto the native render controls.
/// `model_scale` is the footage's object scale from AE's model settings (1 = as authored).
pub(super) fn import(_project: &mut Project, layer: &mut Layer, ir: &Json, model_scale: Option<f64>, losses: &mut Vec<String>, context: &str) {
    let layer_time = LayerTime::of(layer);
    if let Some(scale) = layer.props.prop_mut("geometryOptions/unitScale") {
        scale.value = Value::Scalar(512.0);
    } else {
        losses.push(format!("{context}: native model scale missing"));
    }
    if let Some(k) = model_scale.filter(|k| k.is_finite() && *k > 0.0 && (*k - 1.0).abs() > 1e-12) {
        if let Some(scale) = layer.props.prop_mut("geometryOptions/objectScale") {
            scale.value = Value::Scalar(k * 100.0);
        } else {
            losses.push(format!("{context}: native model object scale missing"));
        }
    }
    if let Some(axes) = layer.props.prop_mut("geometryOptions/aeSourceSpace") {
        axes.value = Value::Bool(true);
    } else {
        losses.push(format!("{context}: native AE model axes control missing"));
    }
    let Some(material) = array(ir, "properties").iter().find(|p| string(p, &["match_name"]) == Some("ADBE Material Options Group")) else {
        return;
    };
    for source in array(material, "children") {
        let target = match string(source, &["match_name"]) {
            Some("ADBE Casts Shadows") => "castsShadows",
            Some("ADBE Accepts Shadows") => "acceptsShadows",
            Some("ADBE Accepts Lights") => "acceptsLights",
            Some("ADBE Appears in Reflections") => "appearsInReflections",
            _ => continue,
        };
        let Some(prop) = layer.props.prop_mut(&format!("materialOptions/{target}")) else {
            losses.push(format!("{context}: native model material {target} missing"));
            continue;
        };
        // AE model tri-state values are 0=Off, 1=On, 2=Only. The common
        // property adapter expects AE's usual 1-based enum, so offset here.
        let remap: Option<&dyn Fn(f64) -> f64> = if matches!(target, "castsShadows" | "acceptsShadows") { Some(&|v| v + 1.0) } else { None };
        let raw = remap_json(source.get("value").unwrap_or(&Json::Null), remap);
        let value_type = string(source, &["value_type"]).unwrap_or("");
        if let Some(value) = convert_value(&raw, value_type) {
            fill_property_mapped(prop, source, value, value_type, 1.0, remap, layer_time, losses, context);
        }
    }
}

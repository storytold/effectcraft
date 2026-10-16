//! AE Layer Styles → EffectCraft's editable nine-style property tree.

use effectcraft_keyframe::Value;
use effectcraft_project::{Layer, Node, Project, PropGroup, build, styles};
use serde_json::Value as Json;

use super::{LayerTime, array, boolean, convert_value, fill_property, number, string};

fn style_id(raw: &str) -> Option<&'static str> {
    Some(match raw {
        "dropShadow/enabled" => "dropShadow",
        "innerShadow/enabled" => "innerShadow",
        "outerGlow/enabled" => "outerGlow",
        "innerGlow/enabled" => "innerGlow",
        "bevelEmboss/enabled" => "bevelEmboss",
        "chromeFX/enabled" => "satin",
        "solidFill/enabled" => "colorOverlay",
        "gradientFill/enabled" => "gradientOverlay",
        "frameFX/enabled" => "stroke",
        _ => return None,
    })
}

fn param_id(style: &str, raw: &str) -> Option<&'static str> {
    if style == "blendingOptions" {
        return Some(match raw {
            "ADBE Global Angle2" => "globalLightAngle",
            "ADBE Global Altitude2" => "globalLightAltitude",
            "ADBE Layer Fill Opacity2" => "fillOpacity",
            "ADBE R Channel Blend" => "red",
            "ADBE G Channel Blend" => "green",
            "ADBE B Channel Blend" => "blue",
            "ADBE Blend Interior" => "blendInteriorAsGroup",
            "ADBE Blend Ranges" => "useBlendRanges",
            _ => return None,
        });
    }
    let (prefix, suffix) = raw.split_once('/')?;
    let expected = match style {
        "satin" => "chromeFX",
        "colorOverlay" => "solidFill",
        "gradientOverlay" => "gradientFill",
        "stroke" => "frameFX",
        other => other,
    };
    if prefix != expected {
        return None;
    }
    Some(match suffix {
        "mode2" => "blendMode",
        "color" => "color",
        "opacity" => "opacity",
        "useGlobalAngle" => "useGlobalLight",
        "localLightingAngle" => "angle",
        "localLightingAltitude" => "altitude",
        "distance" => "distance",
        "chokeMatte" => {
            if matches!(style, "innerShadow" | "innerGlow") {
                "choke"
            } else {
                "spread"
            }
        }
        "blur" => "size",
        "noise" => "noise",
        "layerConceals" => "knocksOut",
        "AEColorChoice" => "colorType",
        "gradient" => "colors",
        "gradientSmoothness" => "gradientSmoothness",
        "glowTechnique" => "technique",
        "innerGlowSource" => "source",
        "inputRange" => "range",
        "shadingNoise" => "jitter",
        "bevelStyle" => "style",
        "bevelTechnique" => "technique",
        "strengthRatio" => "depth",
        "bevelDirection" => "direction",
        "softness" => "soften",
        "highlightMode" => "highlightMode",
        "highlightColor" => "highlightColor",
        "highlightOpacity" => "highlightOpacity",
        "shadowMode" => "shadowMode",
        "shadowColor" => "shadowColor",
        "shadowOpacity" => "shadowOpacity",
        "invert" => "invert",
        "angle" => "angle",
        "type" => "style",
        "reverse" => "reverse",
        "align" => "alignWithLayer",
        "scale" => "scale",
        "offset" => "offset",
        "size" => "size",
        "style" => "position",
        _ => return None,
    })
}

// AE's style Blend Mode popup has gaps between families; native indices are contiguous.
fn mode_index(raw: u32) -> Option<u32> {
    Some(match raw {
        1 => 0,
        5 => 3,
        11 => 8,
        12 => 9,
        16 => 12,
        17 => 13,
        18 => 14,
        _ => return None,
    })
}

fn map_mode(v: &mut Value, losses: &mut Vec<String>, context: &str, raw: &str) {
    if let Value::Enum(index) = v {
        let ae = index.saturating_add(1);
        if let Some(native) = mode_index(ae) {
            *index = native;
        } else {
            losses.push(format!("{context}: unsupported layer style blend mode {ae} on {raw}"));
            *index = 0;
        }
    }
}

fn map_inner_glow_source(v: &mut Value, losses: &mut Vec<String>, context: &str) {
    if let Value::Enum(index) = v {
        *index = match *index {
            0 => 1, // AE 1 = Edge; native 1 = Edge.
            1 => 0, // AE 2 = Center; native 0 = Center.
            raw => {
                losses.push(format!("{context}: unsupported Inner Glow source {}", raw + 1));
                1
            }
        };
    }
}

fn fill_group(group: &mut PropGroup, ir: &Json, style: &str, layer_time: LayerTime, losses: &mut Vec<String>, context: &str) {
    for child in array(ir, "children") {
        if string(child, &["kind"]) == Some("group") {
            fill_group(group, child, style, layer_time, losses, context);
            continue;
        }
        let Some(raw) = string(child, &["match_name"]) else { continue };
        let Some(id) = param_id(style, raw) else {
            if child.get("value").is_some_and(Json::is_null) && array(child, "keyframes").is_empty() {
                continue;
            }
            losses.push(format!("{context}: unsupported layer style parameter {raw}"));
            continue;
        };
        let gradient_selected = id == "colors"
            && (style == "gradientOverlay"
                || group.get("colorType").is_some_and(|p| p.value == Value::Enum(1) || !p.keys.is_empty())
                || !array(child, "keyframes").is_empty());
        let Some(prop) = (if style == "blendingOptions" && !matches!(id, "globalLightAngle" | "globalLightAltitude") {
            group.sub_mut("advancedBlending").and_then(|g| g.get_mut(id))
        } else {
            group.get_mut(id)
        }) else {
            losses.push(format!("{context}: missing native layer style parameter {style}/{id}"));
            continue;
        };
        let value_type = string(child, &["value_type"]).unwrap_or("");
        let Some(value) = child.get("value").and_then(|v| convert_value(v, value_type)) else {
            if id == "colors" {
                if gradient_selected {
                    losses.push(format!("{context}: unsupported layer style gradient {raw}"));
                }
            } else {
                losses.push(format!("{context}: unsupported layer style value {raw}"));
            }
            continue;
        };
        fill_property(prop, child, value, value_type, 1.0, layer_time, losses, context);
        if style == "innerGlow" && id == "source" {
            if number(child, &["value"]).is_some_and(|raw| raw != 1.0 && raw != 2.0) {
                losses.push(format!("{context}: unsupported Inner Glow source value"));
            }
            if array(child, "keyframes").iter().any(|key| number(key, &["value"]).is_some_and(|raw| raw != 1.0 && raw != 2.0)) {
                losses.push(format!("{context}: unsupported keyed Inner Glow source value"));
            }
            map_inner_glow_source(&mut prop.value, losses, context);
            for key in &mut prop.keys {
                map_inner_glow_source(&mut key.value, losses, context);
            }
        }
        if matches!(id, "blendMode" | "highlightMode" | "shadowMode") {
            map_mode(&mut prop.value, losses, context, raw);
            for key in &mut prop.keys {
                map_mode(&mut key.value, losses, context, raw);
            }
        }
    }
}

/// Import only applied styles: AE exports disabled default placeholders for every layer.
pub(super) fn import(project: &mut Project, layer: &mut Layer, ir: &Json, losses: &mut Vec<String>, context: &str) {
    let layer_time = LayerTime::of(layer);
    let blend_ir = array(ir, "children").iter().find(|g| string(g, &["match_name"]) == Some("ADBE Blend Options Group"));
    let active: Vec<_> = array(ir, "children")
        .iter()
        .filter(|g| boolean(g, &["enabled"]) == Some(true) && style_id(string(g, &["match_name"]).unwrap_or("")).is_some())
        .collect();
    if active.is_empty() && boolean(ir, &["enabled"]) != Some(true) {
        return;
    }
    let mut group = styles::layer_styles(&mut build::Ids(&mut project.next_id), &styles::GlobalLight::default());
    group.enabled = boolean(ir, &["enabled"]).unwrap_or(true);
    if let Some(bo) = group.sub_mut(styles::BLENDING) {
        if let Some(blend_ir) = blend_ir {
            bo.enabled = boolean(blend_ir, &["enabled"]).unwrap_or(true);
            fill_group(bo, blend_ir, "blendingOptions", layer_time, losses, context);
        } else {
            losses.push(format!("{context}: Layer Styles lacks Blending Options"));
        }
    }
    for source in active {
        let raw = string(source, &["match_name"]).unwrap_or("");
        let Some(id) = style_id(raw) else { continue };
        let Some(mut native) = styles::style(&mut build::Ids(&mut project.next_id), id) else { continue };
        fill_group(&mut native, source, id, layer_time, losses, context);
        group.children.push(Node::Group(native));
    }
    for source in array(ir, "children") {
        let raw = string(source, &["match_name"]).unwrap_or("");
        if boolean(source, &["enabled"]) == Some(true) && raw != "ADBE Blend Options Group" && style_id(raw).is_none() {
            losses.push(format!("{context}: unsupported layer style {raw}"));
        }
    }
    let at = layer.props.children.iter().position(|node| node.match_id() == "transform").map(|i| i + 1).unwrap_or(layer.props.children.len());
    layer.props.children.insert(at, Node::Group(group));
}

#[cfg(test)]
mod tests {
    use super::*;
    use effectcraft_project::{Comp, LayerSource};

    #[test]
    fn ae_inner_glow_source_one_is_edge_two_is_center() {
        let mut project = Project::default();
        let comp = Comp::new(100, 100, effectcraft_time::FrameRate::new(30, 1), effectcraft_time::Tick::ZERO);
        let mut layer = build::layer(&mut project, &comp, "solid", LayerSource::Shape, (100, 100), None);
        let ir = serde_json::json!({"enabled":true,"children":[
            {"match_name":"ADBE Blend Options Group","enabled":true,"children":[]},
            {"match_name":"innerGlow/enabled","enabled":true,"children":[
                {"match_name":"innerGlow/innerGlowSource","kind":"property","value":1.0,"keyframes":[{"time":1.0,"value":2.0}]}
            ]}
        ]});
        let mut losses = Vec::new();
        import(&mut project, &mut layer, &ir, &mut losses, "test");
        let source = layer.layer_styles().unwrap().sub("innerGlow").unwrap().get("source").unwrap();
        assert_eq!(source.value, Value::Enum(1));
        assert_eq!(source.keys[0].value, Value::Enum(0));
        assert!(losses.is_empty(), "{losses:?}");
    }

    #[test]
    fn imports_editable_style_values_and_keys_in_source_order() {
        let mut project = Project::default();
        let comp = Comp::new(100, 100, effectcraft_time::FrameRate::new(30, 1), effectcraft_time::Tick::ZERO);
        let mut layer = build::layer(&mut project, &comp, "solid", LayerSource::Shape, (100, 100), None);
        let ir = serde_json::json!({"enabled":true,"children":[
            {"match_name":"ADBE Blend Options Group","enabled":true,"children":[]},
            {"match_name":"solidFill/enabled","enabled":true,"children":[
                {"match_name":"solidFill/color","kind":"property","value_type":"color","value":[0.2,0.4,0.6,1.0],"keyframes":[]}]},
            {"match_name":"dropShadow/enabled","enabled":true,"children":[
                {"match_name":"dropShadow/mode2","kind":"property","value":5.0,"keyframes":[]},
                {"match_name":"dropShadow/opacity","kind":"property","value":40.0,"keyframes":[{"time":1.0,"value":65.0}]}]},
            {"match_name":"gradientFill/enabled","enabled":true,"children":[
                {"match_name":"gradientFill/gradient","kind":"property","value":{"type":"Gradient","data":{
                    "color_stops":[{"data":{"offset":0.0,"midpoint":0.5,"color":[1.0,0.0,0.0]}},{"data":{"offset":1.0,"midpoint":0.5,"color":[0.0,0.0,1.0]}}],
                    "alpha_stops":[{"data":{"offset":0.0,"midpoint":0.5,"alpha":1.0}},{"data":{"offset":1.0,"midpoint":0.5,"alpha":0.5}}]
                }},"keyframes":[]}]}
        ]});
        let mut losses = vec![];
        import(&mut project, &mut layer, &ir, &mut losses, "test");
        let group = layer.layer_styles().unwrap();
        assert_eq!(group.children[1].match_id(), "colorOverlay");
        assert_eq!(group.children[2].match_id(), "dropShadow");
        assert_eq!(group.children[3].match_id(), "gradientOverlay");
        assert_eq!(group.sub("dropShadow").unwrap().get("blendMode").unwrap().value, Value::Enum(3));
        assert_eq!(group.sub("dropShadow").unwrap().get("opacity").unwrap().keys[0].value, Value::Scalar(65.0));
        assert_eq!(group.sub("colorOverlay").unwrap().get("color").unwrap().value, Value::Color([0.2, 0.4, 0.6, 1.0]));
        let Value::Gradient(gradient) = &group.sub("gradientOverlay").unwrap().get("colors").unwrap().value else { panic!("gradient lost") };
        assert_eq!(gradient.colors.len(), 2);
        assert_eq!(gradient.opacities[1], (1.0, 0.5));
        assert!(losses.is_empty(), "{losses:?}");
    }
}

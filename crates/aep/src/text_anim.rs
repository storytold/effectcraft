//! AE text animator IDs and the small subset whose rendering is verified natively.
//! py-aep exposes latent property slots as well as authored ones. Only authored
//! animator properties may become native operations; the rest remain editable
//! under their original match IDs.

use effectcraft_keyframe::Value;
use serde_json::Value as Json;

pub(crate) fn native_match(parent: &str, ae: &str, authored: Option<bool>) -> Option<&'static str> {
    Some(match (parent, ae) {
        ("text", "ADBE Text Animators") => "animators",
        ("text", "ADBE Text Path Options") => "pathOptions",
        ("pathOptions", "ADBE Text Path") => "path",
        ("pathOptions", "ADBE Text Reverse Path") => "reversePath",
        ("pathOptions", "ADBE Text Perpendicular To Path") => "perpendicular",
        ("pathOptions", "ADBE Text Force Align Path") => "forceAlignment",
        ("pathOptions", "ADBE Text First Margin") => "firstMargin",
        ("pathOptions", "ADBE Text Last Margin") => "lastMargin",
        ("animators", "ADBE Text Animator") => "animator",
        ("animator", "ADBE Text Selectors") => "selectors",
        ("animator", "ADBE Text Animator Properties") => "properties",
        ("selectors", "ADBE Text Selector") => "rangeSelector",
        ("rangeSelector", "ADBE Text Range Advanced") => "advanced",
        ("rangeSelector", "ADBE Text Percent Start") => "start",
        ("rangeSelector", "ADBE Text Percent End") => "end",
        ("rangeSelector", "ADBE Text Percent Offset") => "offset",
        ("advanced", "ADBE Text Range Units") => "units",
        ("advanced", "ADBE Text Range Type2") => "basedOn",
        ("advanced", "ADBE Text Selector Mode") => "mode",
        ("advanced", "ADBE Text Selector Max Amount") => "amount",
        ("advanced", "ADBE Text Range Shape") => "shape",
        ("advanced", "ADBE Text Selector Smoothness") => "smoothness",
        ("advanced", "ADBE Text Levels Max Ease") => "easeHigh",
        ("advanced", "ADBE Text Levels Min Ease") => "easeLow",
        ("advanced", "ADBE Text Randomize Order") => "randomize",
        ("advanced", "ADBE Text Random Seed") => "randomSeed",
        ("properties", "ADBE Text Scale 3D") if authored == Some(true) => "scale",
        ("properties", "ADBE Text Blur") if authored == Some(true) => "blur",
        ("properties", "ADBE Text Position 3D") if authored == Some(true) => "position",
        ("properties", "ADBE Text Opacity") if authored == Some(true) => "opacity",
        _ => return None,
    })
}

pub(crate) fn template(parent: &str, id: &str) -> Option<Value> {
    match (parent, id) {
        ("advanced", "units" | "basedOn" | "mode" | "shape") => Some(Value::Enum(0)),
        ("advanced", "randomize") => Some(Value::Bool(false)),
        _ => None,
    }
}

/// Clear the fidelity warning only when every enabled animator uses the native
/// Range Selector with authored Scale and/or Blur and all live selector controls
/// have a semantic mapping. Old IR without authored markers remains a loss.
pub(crate) fn fully_supported(ir: &Json) -> bool {
    let Some(anims) = ir.get("children").and_then(Json::as_array) else { return false };
    let mut active = false;
    for anim in anims.iter().filter(|a| a.get("enabled").and_then(Json::as_bool).unwrap_or(true)) {
        if anim.get("match_name").and_then(Json::as_str) != Some("ADBE Text Animator") {
            return false;
        }
        active = true;
        let Some(children) = anim.get("children").and_then(Json::as_array) else { return false };
        let mut has_selector = false;
        let mut has_operation = false;
        for group in children {
            let Some(id) = group.get("match_name").and_then(Json::as_str) else { return false };
            let Some(leaves) = group.get("children").and_then(Json::as_array) else { return false };
            match id {
                "ADBE Text Selectors" => {
                    for selector in leaves {
                        if selector.get("match_name").and_then(Json::as_str) != Some("ADBE Text Selector") {
                            return false;
                        }
                        has_selector = true;
                        if !selector_supported(selector, "selectors") {
                            return false;
                        }
                    }
                }
                "ADBE Text Animator Properties" => {
                    for prop in leaves {
                        if prop.get("authored").and_then(Json::as_bool).is_none() {
                            return false;
                        }
                        if prop.get("authored").and_then(Json::as_bool) != Some(true) {
                            continue;
                        }
                        let Some(ae) = prop.get("match_name").and_then(Json::as_str) else { return false };
                        if native_match("properties", ae, Some(true)).is_none() || has_expression(prop) {
                            return false;
                        }
                        has_operation = true;
                    }
                }
                _ => return false,
            }
        }
        if !has_selector || !has_operation {
            return false;
        }
    }
    active
}

fn selector_supported(group: &Json, parent: &str) -> bool {
    let Some(children) = group.get("children").and_then(Json::as_array) else { return false };
    children.iter().all(|child| {
        if child.get("match_name").and_then(Json::as_str) == Some("ADBE Text Range Advanced") {
            return selector_supported(child, "rangeSelector");
        }
        if child.get("authored").and_then(Json::as_bool).is_none() {
            return false;
        }
        if child.get("authored").and_then(Json::as_bool) != Some(true) {
            return true;
        }
        let Some(ae) = child.get("match_name").and_then(Json::as_str) else { return false };
        let mapped_parent = if parent == "rangeSelector" { "advanced" } else { "rangeSelector" };
        native_match(mapped_parent, ae, Some(true)).is_some() && !has_expression(child)
    })
}

fn has_expression(prop: &Json) -> bool {
    prop.get("expression_enabled").and_then(Json::as_bool) == Some(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn authored_scale_blur_and_word_range_are_native() {
        assert_eq!(native_match("properties", "ADBE Text Scale 3D", Some(true)), Some("scale"));
        assert_eq!(native_match("properties", "ADBE Text Scale 3D", Some(false)), None);
        assert_eq!(native_match("properties", "ADBE Text Fill Color", Some(true)), None);
        assert_eq!(native_match("advanced", "ADBE Text Range Type2", Some(true)), Some("basedOn"));
        assert_eq!(template("advanced", "basedOn"), Some(Value::Enum(0)));

        let ir = json!({"children":[{"match_name":"ADBE Text Animator","enabled":true,"children":[
            {"match_name":"ADBE Text Selectors","children":[{"match_name":"ADBE Text Selector","children":[
                {"match_name":"ADBE Text Percent Start","authored":true,"keyframes":[{"time":6.13,"value":0},{"time":7.46,"value":100}]},
                {"match_name":"ADBE Text Range Advanced","children":[{"match_name":"ADBE Text Range Type2","authored":true,"value":3}]}
            ]}]},
            {"match_name":"ADBE Text Animator Properties","children":[
                {"match_name":"ADBE Text Scale 3D","authored":true,"value":[0,0,0]},
                {"match_name":"ADBE Text Blur","authored":true,"value":[40,40]},
                {"match_name":"ADBE Text Fill Color","authored":false,"value":[1,0,0,1]}
            ]}
        ]}]});
        assert!(fully_supported(&ir));
        let mut unsupported = ir.clone();
        unsupported["children"][0]["children"][1]["children"][2]["authored"] = json!(true);
        assert!(!fully_supported(&unsupported));
        let mut old_ir = ir;
        old_ir["children"][0]["children"][1]["children"][0].as_object_mut().unwrap().remove("authored");
        assert!(!fully_supported(&old_ir));
    }
}

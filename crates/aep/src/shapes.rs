use effectcraft_keyframe::{FeatherPoint, ShapePath, Value};
use serde_json::Value as Json;

/// Canonical shape-tree IDs used by the native renderer. Property IDs that repeat in different
/// groups (such as `position`) rely on their parent group for meaning.
pub(crate) fn native_match(ae: &str) -> Option<&'static str> {
    Some(match ae {
        "ADBE Vectors Group" => "contents",
        "ADBE Vector Group" => "group",
        "ADBE Vector Transform Group" | "ADBE Vector Repeater Transform" => "transform",
        "ADBE Vector Shape - Group" => "path",
        "ADBE Vector Shape - Rect" => "rect",
        "ADBE Vector Shape - Ellipse" => "ellipse",
        "ADBE Vector Shape - Star" => "star",
        "ADBE Vector Graphic - Fill" => "fill",
        "ADBE Vector Graphic - Stroke" => "stroke",
        "ADBE Vector Graphic - G-Fill" => "gfill",
        "ADBE Vector Graphic - G-Stroke" => "gstroke",
        "ADBE Vector Filter - Merge" => "merge",
        "ADBE Vector Filter - Offset" => "offset",
        "ADBE Vector Filter - PB" => "pucker",
        "ADBE Vector Filter - Repeater" => "repeater",
        "ADBE Vector Filter - RC" => "round",
        "ADBE Vector Filter - Trim" => "trim",
        "ADBE Vector Filter - Twist" => "twist",
        "ADBE Vector Filter - Roughen" => "wiggle",
        "ADBE Vector Filter - Zigzag" => "zigzag",
        "ADBE Vector Shape" => "path",
        "ADBE Vector Shape Direction" => "direction",
        "ADBE Vector Blend Mode" => "blend",
        "ADBE Vector Composite Order" | "ADBE Vector Repeater Order" => "composite",
        "ADBE Vector Anchor" | "ADBE Vector Repeater Anchor" => "anchor",
        "ADBE Vector Position"
        | "ADBE Vector Rect Position"
        | "ADBE Vector Ellipse Position"
        | "ADBE Vector Star Position"
        | "ADBE Vector Repeater Position" => "position",
        "ADBE Vector Scale" | "ADBE Vector Repeater Scale" => "scale",
        "ADBE Vector Skew" => "skew",
        "ADBE Vector Skew Axis" => "skewAxis",
        "ADBE Vector Rotation" | "ADBE Vector Star Rotation" | "ADBE Vector Repeater Rotation" => "rotation",
        "ADBE Vector Group Opacity" | "ADBE Vector Fill Opacity" | "ADBE Vector Stroke Opacity" => "opacity",
        "ADBE Vector Rect Size" | "ADBE Vector Ellipse Size" => "size",
        "ADBE Vector Rect Roundness" => "roundness",
        "ADBE Vector Star Type" => "type",
        "ADBE Vector Star Points" => "points",
        "ADBE Vector Star Inner Radius" => "innerRadius",
        "ADBE Vector Star Outer Radius" => "outerRadius",
        "ADBE Vector Star Inner Roundess" => "innerRoundness",
        "ADBE Vector Star Outer Roundess" => "outerRoundness",
        "ADBE Vector Fill Rule" => "rule",
        "ADBE Vector Fill Color" | "ADBE Vector Stroke Color" => "color",
        "ADBE Vector Grad Type" => "type",
        "ADBE Vector Grad Start Pt" => "start",
        "ADBE Vector Grad End Pt" => "end",
        "ADBE Vector Grad HiLite Length" => "highlightLength",
        "ADBE Vector Grad HiLite Angle" => "highlightAngle",
        "ADBE Vector Grad Colors" => "colors",
        "ADBE Vector Stroke Width" => "width",
        "ADBE Vector Stroke Line Cap" => "cap",
        "ADBE Vector Stroke Line Join" => "join",
        "ADBE Vector Stroke Miter Limit" => "miter",
        "ADBE Vector Stroke Dashes" => "dashes",
        "ADBE Vector Stroke Dash 1" => "dash",
        "ADBE Vector Stroke Gap 1" => "gap",
        "ADBE Vector Stroke Dash 2" => "dash2",
        "ADBE Vector Stroke Gap 2" => "gap2",
        "ADBE Vector Stroke Dash 3" => "dash3",
        "ADBE Vector Stroke Gap 3" => "gap3",
        "ADBE Vector Stroke Offset" => "offset",
        "ADBE Vector Stroke Taper" => "taper",
        "ADBE Vector Stroke Wave" => "wave",
        "ADBE Vector Trim Start" => "start",
        "ADBE Vector Trim End" => "end",
        "ADBE Vector Trim Offset" | "ADBE Vector Repeater Offset" => "offset",
        "ADBE Vector Trim Type" | "ADBE Vector Merge Type" => "mode",
        "ADBE Vector Repeater Copies" => "copies",
        "ADBE Vector Repeater Opacity 1" => "startOpacity",
        "ADBE Vector Repeater Opacity 2" => "endOpacity",
        "ADBE Vector RoundCorner Radius" => "radius",
        "ADBE Vector PuckerBloat Amount" => "amount",
        "ADBE Vector Twist Angle" => "angle",
        "ADBE Vector Twist Center" => "center",
        _ => return None,
    })
}

/// AE shape popup values are one-based; native shape menus are zero-based.
pub(crate) fn is_popup(parent: &str, property: &str) -> bool {
    match parent {
        "path" | "rect" | "ellipse" | "star" => matches!(property, "direction" | "type"),
        "fill" | "stroke" | "gfill" | "gstroke" => {
            matches!(property, "blend" | "composite" | "rule" | "cap" | "join" | "type")
        }
        "trim" | "merge" | "repeater" => matches!(property, "mode" | "composite"),
        "offset" => property == "join",
        "zigzag" => property == "points",
        "taper" | "wave" => property == "units",
        _ => false,
    }
}

/// Convert exported AE Shape data to an editable path. Invalid geometry is rejected so the
/// importer can report a loss instead of silently changing the outline.
pub(crate) fn value(j: &Json) -> Option<Value> {
    const MAX_POINTS: usize = 100_000;
    fn points(j: &Json, key: &str, max: usize) -> Option<Vec<[f64; 2]>> {
        let a = j.get(key)?.as_array()?;
        if a.len() > max {
            return None;
        }
        a.iter()
            .map(|p| {
                let p = p.as_array()?;
                if p.len() != 2 {
                    return None;
                }
                let x = p.first()?.as_f64().filter(|n| n.is_finite())?;
                let y = p.get(1)?.as_f64().filter(|n| n.is_finite())?;
                Some([x, y])
            })
            .collect()
    }

    let vertices = points(j, "vertices", MAX_POINTS)?;
    if vertices.is_empty() {
        return None;
    }
    let in_tangents = points(j, "in_tangents", vertices.len())?;
    let out_tangents = points(j, "out_tangents", vertices.len())?;
    if in_tangents.len() != vertices.len() || out_tangents.len() != vertices.len() {
        return None;
    }
    let closed = j.get("closed")?.as_bool()?;
    let segment_count = if closed { vertices.len() } else { vertices.len().saturating_sub(1) };
    let feather = match j.get("feather_points") {
        None => Vec::new(),
        Some(f) => {
            let a = f.as_array()?;
            if a.len() > MAX_POINTS {
                return None;
            }
            a.iter()
                .map(|f| {
                    let segment = usize::try_from(f.get("segment")?.as_u64()?).ok()?;
                    if segment >= segment_count {
                        return None;
                    }
                    let finite = |key: &str| f.get(key)?.as_f64().filter(|n| n.is_finite());
                    let t = finite("t")?;
                    if !(0.0..=1.0).contains(&t) {
                        return None;
                    }
                    let radius = finite("radius")?;
                    let tension = if f.get("tension").is_some() { finite("tension")? } else { 0.0 };
                    if !(0.0..=1.0).contains(&tension) {
                        return None;
                    }
                    Some(FeatherPoint { segment, t, radius, tension })
                })
                .collect::<Option<Vec<_>>>()?
        }
    };
    Some(Value::Path(ShapePath { vertices, in_tangents, out_tangents, closed, feather }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn converts_shape_and_rejects_malformed_geometry() {
        let source = json!({
            "vertices": [[0, 0], [10, 20]],
            "in_tangents": [[0, 0], [-2, 3]],
            "out_tangents": [[2, -3], [0, 0]],
            "closed": false,
            "feather_points": [{"segment": 0, "t": 0.5, "radius": 4.0}]
        });
        let Some(Value::Path(path)) = value(&source) else { panic!("expected editable path") };
        assert_eq!(path.vertices, vec![[0.0, 0.0], [10.0, 20.0]]);
        assert_eq!(path.in_tangents[1], [-2.0, 3.0]);
        assert_eq!(path.out_tangents[0], [2.0, -3.0]);
        assert!(!path.closed);
        assert_eq!(path.feather[0].radius, 4.0);

        assert!(value(&json!({"vertices": [[0, 0]], "in_tangents": [], "out_tangents": [[0, 0]], "closed": true})).is_none());
        assert!(value(&json!({"vertices": [[0, "bad"]], "in_tangents": [[0, 0]], "out_tangents": [[0, 0]], "closed": true})).is_none());
    }

    #[test]
    fn maps_shape_groups_and_leaves() {
        assert_eq!(native_match("ADBE Vector Group"), Some("group"));
        assert_eq!(native_match("ADBE Vector Shape - Group"), Some("path"));
        assert_eq!(native_match("ADBE Vector Shape"), Some("path"));
        assert_eq!(native_match("ADBE Vector Graphic - Fill"), Some("fill"));
        assert_eq!(native_match("ADBE Vector Graphic - Stroke"), Some("stroke"));
        assert_eq!(native_match("ADBE Vector Filter - Trim"), Some("trim"));
        assert_eq!(native_match("ADBE Vector Trim Start"), Some("start"));
        assert_eq!(native_match("ADBE Some Other Group"), None);
    }

    #[test]
    fn maps_ae_multi_dash_stroke_to_editable_native_pattern() {
        let expected = [
            ("ADBE Vector Stroke Dash 1", "dash"),
            ("ADBE Vector Stroke Gap 1", "gap"),
            ("ADBE Vector Stroke Dash 2", "dash2"),
            ("ADBE Vector Stroke Gap 2", "gap2"),
            ("ADBE Vector Stroke Dash 3", "dash3"),
            ("ADBE Vector Stroke Gap 3", "gap3"),
            ("ADBE Vector Stroke Offset", "offset"),
        ];
        for (ae, native) in expected {
            assert_eq!(native_match(ae), Some(native));
        }
    }
}

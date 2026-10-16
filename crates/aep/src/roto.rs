//! AE Samurai Roto Brush strokes into editable native RotoData.

use super::*;
use effectcraft_effects::roto::{self, Stroke, StrokeKind};

fn child<'a>(group: &'a Json, match_name: &str) -> Option<&'a Json> {
    array(group, "children").iter().find(|c| string(c, &["match_name"]) == Some(match_name))
}

fn pair(value: &Json) -> Option<[f64; 2]> {
    let a = value.as_array()?;
    Some([a.first()?.as_f64().filter(|n| n.is_finite())?, a.get(1)?.as_f64().filter(|n| n.is_finite())?])
}

fn kind(name: &str) -> Option<StrokeKind> {
    let n = name.to_ascii_lowercase();
    if n.starts_with("foreground") {
        Some(StrokeKind::Fg)
    } else if n.starts_with("background") {
        Some(StrokeKind::Bg)
    } else if n.starts_with("refine edge erase") || n.starts_with("refine erase") {
        Some(StrokeKind::RefineErase)
    } else if n.starts_with("refine edge") || n.starts_with("refine") {
        Some(StrokeKind::Refine)
    } else {
        None
    }
}

fn stroke(ir: &Json, size: [f64; 2], fps: f64, losses: &mut Vec<String>, context: &str) -> Option<Stroke> {
    let name = string(ir, &["name"]).unwrap_or("?");
    let Some(kind) = kind(name) else {
        losses.push(format!("{context}: unsupported Roto stroke kind {name}"));
        return None;
    };
    if boolean(ir, &["enabled"]) == Some(false) {
        losses.push(format!("{context}: disabled Roto stroke {name} omitted"));
        return None;
    }
    let duration = child(ir, "ADBE Paint Duration").and_then(|v| v.get("value")).and_then(pair);
    let Some([start_ms, length_ms]) = duration else {
        losses.push(format!("{context}: Roto stroke {name} missing duration"));
        return None;
    };
    let frame = (start_ms * fps / 1000.0).round() as i64;
    if length_ms > 1500.0 / fps {
        losses.push(format!("{context}: Roto stroke {name} duration exceeds one native frame"));
    }
    let Some(shape) = child(ir, "ADBE Paint Shape") else {
        losses.push(format!("{context}: Roto stroke {name} missing shape"));
        return None;
    };
    let shape_data = shape.get("value").and_then(|v| v.get("data"));
    let Some(vertices) = shape_data.and_then(|v| v.get("vertices")).and_then(Json::as_array) else {
        losses.push(format!("{context}: Roto stroke {name} has unsupported shape"));
        return None;
    };
    let position = child(ir, "ADBE Paint Transform").and_then(|g| child(g, "ADBE Paint Position")).and_then(|v| v.get("value")).and_then(pair);
    let Some(position) = position else {
        losses.push(format!("{context}: Roto stroke {name} missing position"));
        return None;
    };
    let mut points: Vec<[f64; 2]> = vertices.iter().filter_map(pair).map(|v| [position[0] + v[0] * size[0], position[1] + v[1] * size[1]]).collect();
    if points.is_empty() || points.len() != vertices.len() {
        losses.push(format!("{context}: Roto stroke {name} has invalid path points"));
        return None;
    }
    if shape_data.and_then(|v| boolean(v, &["closed"])) == Some(true) && points.len() > 1 {
        points.push(points[0]);
    }
    for tangent in ["in_tangents", "out_tangents"] {
        if shape_data.and_then(|v| v.get(tangent)).and_then(Json::as_array).is_some_and(|a| a.iter().filter_map(pair).any(|v| v != [0.0, 0.0])) {
            losses.push(format!("{context}: Roto stroke {name} Bezier tangents simplified to sampled vertices"));
            break;
        }
    }
    let props = child(ir, "ADBE Paint Properties");
    let diameter = props.and_then(|g| child(g, "ADBE Paint Diameter")).and_then(|v| number(v, &["value"]));
    if diameter.is_none() {
        losses.push(format!("{context}: Roto stroke {name} diameter missing; using 5 px"));
    }
    let radius = diameter.map(|d| d / 2.0).unwrap_or(2.5);
    if radius <= 0.0 {
        losses.push(format!("{context}: Roto stroke {name} has invalid brush diameter"));
        return None;
    }
    for field in array(ir, "children") {
        let m = string(field, &["match_name"]).unwrap_or("?");
        if !matches!(m, "ADBE Paint Duration" | "ADBE Paint Shape" | "ADBE Paint Properties" | "ADBE Paint Transform") {
            losses.push(format!("{context}: unsupported Roto stroke field {name}/{m}"));
        }
    }
    if let Some(p) = props {
        for field in array(p, "children") {
            let m = string(field, &["match_name"]).unwrap_or("?");
            if !matches!(m, "ADBE Paint Color" | "ADBE Paint Diameter" | "ADBE Paint Hardness" | "ADBE Paint Target Channels" | "ADBE Paint Clone Layer") {
                losses.push(format!("{context}: unsupported Roto brush field {name}/{m}"));
            }
            if !array(field, "keyframes").is_empty() {
                losses.push(format!("{context}: animated Roto brush field {name}/{m} has no native stroke track"));
            }
        }
    }
    if let Some(t) = child(ir, "ADBE Paint Transform") {
        for field in array(t, "children") {
            let m = string(field, &["match_name"]).unwrap_or("?");
            if m != "ADBE Paint Position" || !array(field, "keyframes").is_empty() {
                losses.push(format!("{context}: unsupported Roto stroke transform {name}/{m}"));
            }
        }
    }
    for field in [child(ir, "ADBE Paint Duration"), Some(shape)].into_iter().flatten() {
        if !array(field, "keyframes").is_empty() {
            losses.push(format!("{context}: animated Roto stroke {} has no native stroke track", string(field, &["match_name"]).unwrap_or("?")));
        }
    }
    // Hardness, color and channels are tool metadata; the native matte uses kind and radius.
    if props.and_then(|p| child(p, "ADBE Paint Hardness")).and_then(|v| number(v, &["value"])).is_some_and(|v| v != 100.0) {
        losses.push(format!("{context}: Roto stroke {name} brush hardness is unsupported"));
    }
    for (field, default) in [("ADBE Paint Target Channels", 1.0), ("ADBE Paint Clone Layer", 0.0)] {
        if props.and_then(|p| child(p, field)).and_then(|v| number(v, &["value"])).is_some_and(|v| v != default) {
            losses.push(format!("{context}: Roto stroke {name} unsupported {field}"));
        }
    }
    Some(Stroke { kind, frame, radius, points })
}

pub(super) fn import(
    project: &mut Project,
    fx: &mut PropGroup,
    ir: &Json,
    size: [f64; 2],
    fps: f64,
    layer_time: LayerTime,
    losses: &mut Vec<String>,
    context: &str,
) {
    let mut plain = serde_json::Map::new();
    plain.insert(
        "children".into(),
        Json::Array(array(ir, "children").iter().filter(|c| string(c, &["match_name"]) != Some("ADBE Samurai Strokes Group")).cloned().collect()),
    );
    let plain = Json::Object(plain);
    import_native_effect_params(project, fx, &plain, &plain, roto::ID, layer_time, losses, context, 0);
    let Some(group) = child(ir, "ADBE Samurai Strokes Group") else { return };
    let mut strokes = Vec::new();
    let mut base = None;
    let mut span = [0, 0];
    // AE lists newest strokes first. Restore drawing order so corrections retain their order.
    for raw in array(group, "children").iter().rev() {
        if string(raw, &["match_name"]) != Some("ADBE Paint Atom") {
            losses.push(format!("{context}: unsupported Roto stroke group {}", string(raw, &["match_name"]).unwrap_or("?")));
            continue;
        }
        if let Some(stroke) = stroke(raw, size, fps, losses, context) {
            if base.is_none() && stroke.kind.is_segmentation() {
                base = Some(stroke.frame);
                span = [stroke.frame - roto::DEFAULT_SPAN, stroke.frame + roto::DEFAULT_SPAN];
            } else if base.is_some() {
                span = [span[0].min(stroke.frame), span[1].max(stroke.frame)];
            }
            strokes.push(stroke);
        }
    }
    let has_strokes = !strokes.is_empty();
    if let Some(prop) = fx.get_mut(roto::STROKES) {
        prop.value = Value::Str(serde_json::json!({"base":base,"span":span,"strokes":strokes}).to_string());
    } else {
        losses.push(format!("{context}: native Roto stroke store missing"));
    }
    if has_strokes {
        losses.push(format!("{context}: Roto base/span inferred from strokes; AE frozen segmentation cannot be imported"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imports_layer_space_roto_strokes() {
        let ir = serde_json::json!({"children":[
            {"match_name":"/winsize","value":30},
            {"match_name":"ADBE Samurai Strokes Group","children":[
                {"match_name":"ADBE Paint Atom","name":"Background 1","children":[
                    {"match_name":"ADBE Paint Duration","value":[1200,40]},
                    {"match_name":"ADBE Paint Shape","value":{"type":"Shape","data":{"vertices":[[0,0],[0.1,0.2]]}}},
                    {"match_name":"ADBE Paint Properties","children":[{"match_name":"ADBE Paint Diameter","value":8}]},
                    {"match_name":"ADBE Paint Transform","children":[{"match_name":"ADBE Paint Position","value":[100,200]}]}
                ]},
                {"match_name":"ADBE Paint Atom","name":"Foreground 1","children":[
                    {"match_name":"ADBE Paint Duration","value":[1000,40]},
                    {"match_name":"ADBE Paint Shape","value":{"type":"Shape","data":{"vertices":[[0,0],[0.1,0.2]]}}},
                    {"match_name":"ADBE Paint Properties","children":[{"match_name":"ADBE Paint Diameter","value":8}]},
                    {"match_name":"ADBE Paint Transform","children":[{"match_name":"ADBE Paint Position","value":[100,200]}]}
                ]}
            ]}
        ]});
        let mut project = Project::default();
        let mut fx =
            effectcraft_effects::instantiate(effectcraft_effects::find(roto::ID).unwrap(), &mut build::Ids(&mut project.next_id), "Roto", [1000.0, 500.0]);
        let mut losses = vec![];
        import(&mut project, &mut fx, &ir, [1000.0, 500.0], 25.0, LayerTime { start: tick(0.0), stretch: 100.0 }, &mut losses, "test");
        let data: Json = serde_json::from_str(match &fx.get(roto::STROKES).unwrap().value {
            Value::Str(s) => s,
            _ => "",
        })
        .unwrap();
        assert_eq!(data["base"], 25);
        assert_eq!(data["strokes"].as_array().unwrap().len(), 2);
        assert_eq!(data["strokes"][0]["points"], serde_json::json!([[100.0, 200.0], [200.0, 300.0]]));
        assert_eq!(data["strokes"][1]["kind"], "bg");
        assert_eq!(data["strokes"][1]["frame"], 30);
        assert_eq!(data["strokes"][0]["radius"], 4.0);
        assert_eq!(fx.prop("rotoBrushPropagation/searchRadius").unwrap().value.as_f64(), 30.0);
        assert!(losses.iter().any(|loss| loss.contains("AE frozen segmentation cannot be imported")));
        assert_eq!(fx.prop(roto::FROZEN).unwrap().value, Value::Str(String::new()));
    }

    #[test]
    fn imports_reviewed_matte_controls_as_editable_native_properties() {
        let mut project = Project::default();
        let mut fx =
            effectcraft_effects::instantiate(effectcraft_effects::find(roto::ID).unwrap(), &mut build::Ids(&mut project.next_id), "Roto", [100.0, 100.0]);
        let controls = [
            ("/spatialsmoothing2", "rotoBrushMatte/feather", 5.0),
            ("/spatialsoftness2", "rotoBrushMatte/contrast", 80.0),
            ("/rmshiftedges", "rotoBrushMatte/shiftEdge", -12.0),
            ("/rmreducechatter", "rotoBrushMatte/reduceChatter", 14.0),
            ("/HBspatialsoftness2", "refineEdgeMatte/feather", 2.0),
            ("/HBspatialcontrast", "refineEdgeMatte/contrast", 25.0),
            ("/HBspatialchoke", "refineEdgeMatte/shiftEdge", -3.0),
            ("/rm2reducechatter", "refineEdgeMatte/reduceChatter", 50.0),
            ("/rmsamplesperframe", "refineEdgeMatte/motionBlur/motionBlurSamples", 8.0),
            ("/rmcleaningamount", "refineEdgeMatte/decontamination/decontaminationAmount", 75.0),
            ("/rmadditionalcleaningradius", "refineEdgeMatte/decontamination/increaseDecontaminationRadius", 4.0),
        ];
        let mut children: Vec<Json> = controls.iter().map(|(raw, _, value)| serde_json::json!({"match_name":raw,"value":value})).collect();
        children.push(serde_json::json!({"match_name":"/rmviewedgecleaningmap","value":1}));
        children[0]["keyframes"] = serde_json::json!([
            {"time":0.0,"value":5.0}, {"time":1.0,"value":9.0}
        ]);
        let mut losses = Vec::new();
        import(
            &mut project,
            &mut fx,
            &serde_json::json!({"children":children}),
            [100.0, 100.0],
            25.0,
            LayerTime { start: Tick::ZERO, stretch: 100.0 },
            &mut losses,
            "test",
        );
        assert!(losses.is_empty(), "{losses:?}");
        for (_, path, value) in controls {
            assert_eq!(fx.prop(path).unwrap().value.as_f64(), value, "{path}");
        }
        assert_eq!(fx.prop("rotoBrushMatte/feather").unwrap().keys.len(), 2);
        assert_eq!(fx.prop("rotoBrushMatte/feather").unwrap().keys[1].value.as_f64(), 9.0);
        assert_eq!(fx.prop("refineEdgeMatte/decontamination/viewDecontaminationMap").unwrap().value, Value::Bool(true));

        let saved = serde_json::to_string(&fx).unwrap();
        let mut reopened: PropGroup = serde_json::from_str(&saved).unwrap();
        assert_eq!(reopened.prop("rotoBrushMatte/contrast").unwrap().value.as_f64(), 80.0);
        reopened.prop_mut("rotoBrushMatte/contrast").unwrap().value = Value::Scalar(40.0);
        assert_eq!(reopened.prop("rotoBrushMatte/contrast").unwrap().value.as_f64(), 40.0);
        assert_eq!(fx.prop("rotoBrushMatte/contrast").unwrap().value.as_f64(), 80.0);
    }

    #[test]
    fn unsupported_samurai_controls_remain_explicit_losses() {
        let mut project = Project::default();
        let mut fx =
            effectcraft_effects::instantiate(effectcraft_effects::find(roto::ID).unwrap(), &mut build::Ids(&mut project.next_id), "Roto", [100.0, 100.0]);
        let unsupported = [
            "/use_adobecut_too",
            "/edgedetection",
            "/enfromulate",
            "/invertforegroundbackground",
            "/rmrefinematteenable",
            "/renderHB",
            "/baserefineedgeradius",
            "/rmrefinematte2enable",
            "/chatter_reduction2",
        ];
        let children: Vec<Json> = unsupported.iter().map(|raw| serde_json::json!({"match_name":raw,"value":1})).collect();
        let mut losses = Vec::new();
        import(
            &mut project,
            &mut fx,
            &serde_json::json!({"children":children}),
            [100.0, 100.0],
            25.0,
            LayerTime { start: Tick::ZERO, stretch: 100.0 },
            &mut losses,
            "test",
        );
        assert_eq!(losses.len(), unsupported.len(), "{losses:?}");
        for raw in unsupported {
            assert!(losses.iter().any(|loss| loss.contains(raw)), "{raw}");
            assert!(fx.get(raw).is_some(), "opaque value remains available: {raw}");
        }
    }
}

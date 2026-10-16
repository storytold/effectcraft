//! Marker values and Essential Properties from py-aep's layer property IR.

use effectcraft_color::Label;
use effectcraft_keyframe::Value;
use effectcraft_project::{
    CuePoint, GroupKind, Layer, LayerSource, Marker, Node, ParamUi, Project, PropGroup, Property,
    essential::{self, EgControl, EgKind, EssentialGraphics},
};
use serde_json::Value as Json;

use super::{IMPLICIT_CAMERA_COMMENT, LayerTime, array, boolean, convert_value, fill_property, integer, native_match, number, string, tick};

pub(super) fn markers(out: &mut Vec<Marker>, ir: &Json, fps: f64, layer_time: LayerTime, losses: &mut Vec<String>, context: &str) {
    let keys = array(ir, "keyframes");
    if keys.is_empty() {
        if let Some(value) = ir.get("value").filter(|v| !v.is_null()) {
            match marker(value, 0.0, fps, layer_time) {
                Some(m) => out.push(m),
                None => losses.push(format!("{context}: unsupported static MarkerValue")),
            }
        }
    } else {
        for key in keys {
            let Some(time) = number(key, &["time"]) else {
                losses.push(format!("{context}: marker has invalid time"));
                continue;
            };
            match key.get("value").and_then(|v| marker(v, time, fps, layer_time)) {
                Some(m) => out.push(m),
                None => losses.push(format!("{context}: unsupported MarkerValue at {time}s")),
            }
        }
    }
    out.sort_by_key(|m| m.time);
}

fn marker(j: &Json, time: f64, fps: f64, layer_time: LayerTime) -> Option<Marker> {
    if string(j, &["type"]) != Some("MarkerValue") {
        return None;
    }
    let data = j.get("data")?;
    let duration = number(data, &["duration"]).unwrap_or(0.0);
    let frames = number(data, &["frame_duration"]).unwrap_or(0.0);
    let label = integer(data, &["label"]).and_then(|n| usize::try_from(n).ok()).and_then(|n| Label::ALL.get(n).copied()).unwrap_or(Label::None);
    let name = string(data, &["cue_point_name"]).unwrap_or("");
    let navigation = boolean(data, &["navigation"]).unwrap_or(false);
    let params: Vec<(String, String)> = match data.get("parameters") {
        Some(Json::Object(v)) => v.iter().map(|(k, v)| (k.clone(), v.as_str().map(str::to_owned).unwrap_or_else(|| v.to_string()))).collect(),
        Some(Json::Array(v)) => v
            .iter()
            .filter_map(|pair| {
                let pair = pair.as_array()?;
                Some((pair.first()?.as_str()?.to_owned(), pair.get(1)?.as_str()?.to_owned()))
            })
            .collect(),
        _ => Vec::new(),
    };
    Some(Marker {
        time: layer_time.at(time),
        duration: tick(
            (if duration > 0.0 {
                duration
            } else if fps > 0.0 {
                frames / fps
            } else {
                0.0
            }) * 100.0
                / layer_time.stretch.abs(),
        ),
        comment: string(data, &["comment"]).unwrap_or("").to_owned(),
        label,
        chapter: string(data, &["chapter"]).unwrap_or("").to_owned(),
        url: string(data, &["url"]).unwrap_or("").to_owned(),
        protected: boolean(data, &["protected_region"]).unwrap_or(false),
        frame_target: string(data, &["frame_target"]).unwrap_or("").to_owned(),
        cue_point: (!name.is_empty() || navigation || !params.is_empty()).then(|| CuePoint { name: name.to_owned(), navigation, params }),
    })
}

/// Imports values into the native Essential Properties group. Call `bind` after all comps exist.
pub(super) fn overrides(project: &mut Project, layer: &mut Layer, ir: &Json, losses: &mut Vec<String>, context: &str) {
    let layer_time = LayerTime::of(layer);
    let mut group = PropGroup::new(project.alloc(), essential::GROUP, "Essential Properties");
    group.kind = GroupKind::Essential { overridden: Vec::new() };
    for child in array(ir, "children") {
        let raw = string(child, &["match_name"]).unwrap_or("");
        let name = string(child, &["name"]).unwrap_or(raw);
        let keys = array(child, "keyframes");
        let value_json = child.get("value").unwrap_or(&Json::Null);
        // AE emits exposed controls and media slots without an instance value.
        if value_json.is_null() && keys.is_empty() {
            continue;
        }
        let value_type = string(child, &["value_type"]).unwrap_or("");
        let value = convert_value(value_json, value_type).or_else(|| keys.first().and_then(|k| k.get("value")).and_then(|v| convert_value(v, value_type)));
        let Some(value) = value else {
            losses.push(format!("{context}: Essential Property {name} ({raw}) has unsupported value"));
            continue;
        };
        let mut prop = Property::new(project.alloc(), raw, name, value.clone());
        if matches!(value, Value::Text(_) | Value::Str(_)) {
            prop.ui = ParamUi::Text;
        }
        fill_property(&mut prop, child, value, value_type, 1.0, layer_time, losses, context);
        if boolean(child, &["enabled"]) == Some(false) {
            losses.push(format!("{context}: disabled state of Essential Property {name} is not represented"));
        }
        group.children.push(Node::Prop(prop));
    }
    if !group.children.is_empty() {
        layer.props.children.retain(|n| n.match_id() != essential::GROUP);
        layer.props.children.push(Node::Group(group));
    }
}

/// Bind imported instance properties to unambiguous properties of their nested comp.
/// An ambiguous or missing source remains editable and receives an explicit loss.
pub(super) fn bind(project: &mut Project, losses: &mut Vec<String>) {
    let comp_ids: Vec<_> = project.comps().map(|(id, _)| *id).collect();
    for outer_id in comp_ids {
        let Some(outer) = project.comp(outer_id) else { continue };
        let instances: Vec<_> = outer
            .layers
            .iter()
            .filter_map(|layer| {
                let LayerSource::Comp { item } = &layer.source else { return None };
                let group = layer.props.sub(essential::GROUP)?;
                Some((layer.id, layer.name.clone(), *item, group.clone()))
            })
            .collect();
        for (instance_id, instance_name, source_id, group) in instances {
            let Some(source) = project.comp(source_id).cloned() else { continue };
            let mut eg = source.essential.clone().unwrap_or_else(|| EssentialGraphics { name: String::new(), controls: Vec::new() });
            let mut assignments = Vec::new();
            for child in group.children.iter().filter_map(Node::as_prop) {
                let wanted = if child.match_id == "ADBE EP Text Document" { "sourceText" } else { native_match(&child.match_id) };
                let mut candidates = Vec::new();
                for source_layer in &source.layers {
                    if source_layer.comment.starts_with(IMPLICIT_CAMERA_COMMENT) {
                        continue;
                    }
                    source_layer.props.walk("", &mut |_, p| {
                        if p.match_id == wanted {
                            candidates.push((source_layer.id, p.uid, p.value.clone(), p.keys.clone()));
                        }
                    });
                }
                if candidates.len() != 1 {
                    losses.push(format!(
                        "comp {} layer {}: Essential Property {} has {} source matches; value kept editable but unbound",
                        outer_id.0,
                        instance_name,
                        child.name,
                        candidates.len()
                    ));
                    continue;
                }
                let (source_layer, source_prop, source_value, source_keys) = candidates.remove(0);
                let control_id = eg
                    .controls
                    .iter()
                    .find_map(|c| match &c.kind {
                        EgKind::Property { layer, prop, .. } if *layer == source_layer && *prop == source_prop => Some(c.id),
                        _ => None,
                    })
                    .unwrap_or_else(|| {
                        let id = project.alloc();
                        eg.controls.push(EgControl {
                            id,
                            name: child.name.clone(),
                            kind: EgKind::Property { layer: source_layer, prop: source_prop, links: Vec::new() },
                            as_type: None,
                        });
                        id
                    });
                assignments.push((child.uid, control_id, child.value != source_value || child.keys != source_keys || child.expr.is_some()));
            }
            if let Some(c) = project.comp_mut(source_id) {
                c.essential = Some(eg);
            }
            if let Some(layer) = project.comp_mut(outer_id).and_then(|c| c.layer_mut(instance_id))
                && let Some(group) = layer.props.sub_mut(essential::GROUP)
            {
                let mut overridden = Vec::new();
                for node in &mut group.children {
                    let Node::Prop(p) = node else { continue };
                    if let Some((_, id, differs)) = assignments.iter().find(|(uid, _, _)| *uid == p.uid) {
                        p.match_id = essential::match_id(*id);
                        if *differs {
                            overridden.push(p.uid);
                        }
                    }
                }
                group.kind = GroupKind::Essential { overridden };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn marker_key_preserves_time_and_metadata() {
        let ir = json!({"keyframes":[{"time":-0.25,"value":{"type":"MarkerValue","data":{"comment":"cue","duration":0.5,"cue_point_name":"start","navigation":true,"parameters":{"a":"b"}}}}]});
        let mut out = Vec::new();
        markers(&mut out, &ir, 30.0, LayerTime { start: tick(0.0), stretch: 100.0 }, &mut Vec::new(), "test");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].time, tick(-0.25));
        assert_eq!(out[0].duration, tick(0.5));
        assert_eq!(out[0].cue_point.as_ref().map(|c| c.params.as_slice()), Some([("a".into(), "b".into())].as_slice()));
    }

    #[test]
    fn marker_key_uses_layer_time_and_stretch() {
        let ir = json!({"keyframes":[{"time":4.0,"value":{"type":"MarkerValue","data":{"duration":0.5}}}]});
        let clock = LayerTime { start: tick(1.0), stretch: 50.0 };
        let mut out = Vec::new();
        markers(&mut out, &ir, 30.0, clock, &mut Vec::new(), "test");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].time, tick(6.0));
        assert_eq!(out[0].duration, tick(1.0));
    }

    #[test]
    fn essential_override_binds_to_nested_comp() {
        let ir = json!({
            "schema_version": 1,
            "items": [
                {"id": 1, "kind": "comp", "name": "Source", "width": 100, "height": 100, "duration": 2,
                 "layers": [{"id": 10, "kind": "text", "name": "Text", "properties": []}]},
                {"id": 2, "kind": "comp", "name": "Outer", "width": 100, "height": 100, "duration": 2,
                 "layers": [{"id": 20, "kind": "precomp", "source_id": 1, "name": "Instance", "properties": [
                    {"kind": "group", "match_name": "ADBE Layer Overrides", "children": [
                        {"kind": "property", "name": "Text Position", "match_name": "ADBE Position", "value": [25, 30], "keyframes": []}
                    ]}
                 ]}]}
            ]
        });
        let mut project = Project::default();
        let result = super::super::import_ir(&mut project, ir.to_string().as_bytes(), "", &mut |_| None).unwrap();
        let source = project.comp(result.item_ids[&1]).unwrap();
        let control = &source.essential.as_ref().unwrap().controls[0];
        let outer = project.comp(result.item_ids[&2]).unwrap();
        let layer = outer.layer(result.layer_ids[&(2, 20)]).unwrap();
        let group = layer.props.sub(essential::GROUP).unwrap();
        assert!(group.get(&essential::match_id(control.id)).is_some());
        assert_eq!(essential::overridden(layer).len(), 1);
    }
}

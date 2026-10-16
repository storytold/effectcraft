//! AE FreePin3's nested meshes and pins into EffectCraft's editable Puppet tree.

use super::*;
use effectcraft_effects::puppet::{self, MeshGeometry, MeshOpts, PinKind};

fn raw_state(project: &mut Project, group: &mut PropGroup, name: &str, ir: &Json, losses: &mut Vec<String>, context: &str) {
    if let Some(state) = string(ir, &[name]) {
        if state.len() > 12_000_000 {
            losses.push(format!("{context}: oversized Puppet {name} state omitted"));
            return;
        }
        let mut p = Property::new(project.alloc(), name, name, Value::Str(state.to_owned()));
        p.ui = ParamUi::Hidden;
        p.static_only = true;
        group.children.push(p.into());
    }
}

fn child<'a>(group: &'a Json, match_name: &str) -> Option<&'a Json> {
    array(group, "children").iter().find(|c| string(c, &["match_name"]) == Some(match_name))
}

fn initial_position(ir: &Json) -> Option<[f64; 2]> {
    let first = array(ir, "keyframes").iter().filter_map(|k| Some((number(k, &["time"])?, k.get("value")?))).min_by(|a, b| a.0.total_cmp(&b.0)).map(|(_, v)| v);
    let value = first.or_else(|| ir.get("value"))?;
    let a = value.as_array()?;
    Some([a.first()?.as_f64().filter(|n| n.is_finite())?, a.get(1)?.as_f64().filter(|n| n.is_finite())?])
}

fn differs(ir: Option<&Json>, default: f64) -> bool {
    ir.is_some_and(|v| {
        number(v, &["value"]).is_some_and(|n| n != default)
            || array(v, "keyframes").iter().any(|k| number(k, &["value"]).is_some_and(|n| n != default))
            || (boolean(v, &["expression_enabled"]) == Some(true) && string(v, &["expression"]).is_some_and(|s| !s.is_empty()))
    })
}

fn import_builtin_params(ir: &Json, losses: &mut Vec<String>, context: &str) {
    if boolean(ir, &["enabled"]) == Some(false) {
        losses.push(format!("{context}: disabled Puppet compositing options are unsupported"));
    }
    for field in array(ir, "children") {
        let raw = string(field, &["match_name"]).unwrap_or("?");
        let inert = match raw {
            "ADBE Effect Mask Parade" => array(field, "children").is_empty(),
            "ADBE Effect Mask Opacity" => number(field, &["value"]) == Some(100.0),
            "ADBE Force CPU GPU" => number(field, &["value"]) == Some(1.0),
            _ => false,
        } && boolean(field, &["enabled"]) != Some(false)
            && array(field, "keyframes").is_empty()
            && !boolean(field, &["expression_enabled"]).unwrap_or(false);
        if !inert {
            losses.push(format!("{context}: unsupported Puppet compositing option {raw}"));
        }
    }
}

fn set(group: &mut PropGroup, name: &str, ir: &Json, layer_time: LayerTime, losses: &mut Vec<String>, context: &str) {
    let Some(prop) = group.get_mut(name) else {
        losses.push(format!("{context}: native Puppet property {name} missing"));
        return;
    };
    let ty = string(ir, &["value_type"]).unwrap_or("");
    let Some(value) = ir.get("value").and_then(|v| convert_value(v, ty)) else {
        losses.push(format!("{context}: unsupported Puppet value {}", string(ir, &["match_name"]).unwrap_or("?")));
        return;
    };
    fill_property(prop, ir, value, ty, 1.0, layer_time, losses, context);
}

fn import_pin(project: &mut Project, deform: &mut PropGroup, ir: &Json, layer_time: LayerTime, losses: &mut Vec<String>, context: &str) {
    const POS_FIELDS: &[(&str, &str)] =
        &[("ADBE FreePin3 PosPin Position", "position"), ("ADBE FreePin3 PosPin Scale", "scale"), ("ADBE FreePin3 PosPin Rotation", "rotation")];
    let raw = string(ir, &["match_name"]).unwrap_or("");
    let (kind, position, fields): (PinKind, &str, &[(&str, &str)]) = match raw {
        "ADBE FreePin3 PosPin Atom" => {
            let p = "ADBE FreePin3 PosPin Position";
            let pin_type = child(ir, "ADBE FreePin3 PosPin Type").and_then(|v| number(v, &["value"]));
            if pin_type.is_some_and(|n| n != 1.0 && n != 2.0) {
                losses.push(format!("{context}: unsupported Puppet pin type {}", pin_type.unwrap_or_default()));
            }
            let advanced =
                pin_type == Some(2.0) || differs(child(ir, "ADBE FreePin3 PosPin Scale"), 100.0) || differs(child(ir, "ADBE FreePin3 PosPin Rotation"), 0.0);
            (if advanced { PinKind::Advanced } else { PinKind::Position }, p, POS_FIELDS)
        }
        _ => {
            losses.push(format!("{context}: unsupported Puppet pin group {raw}"));
            return;
        }
    };
    let Some(pos) = child(ir, position) else {
        losses.push(format!("{context}: Puppet pin missing {position}"));
        return;
    };
    let Some(rest) = initial_position(pos) else {
        losses.push(format!("{context}: Puppet pin has no usable position"));
        return;
    };
    let name = string(ir, &["name"]).unwrap_or("Puppet Pin");
    let mut pin = puppet::pin_group(&mut build::Ids(&mut project.next_id), name, kind, rest);
    pin.enabled = boolean(ir, &["enabled"]).unwrap_or(true);
    for (raw, native) in fields {
        if let Some(field) = child(ir, raw) {
            // Basic Position pins expose inert Scale/Rotation fields in AE. Advanced pins use them.
            if *native == "position" || kind == PinKind::Advanced {
                set(&mut pin, native, field, layer_time, losses, context);
            } else if !array(field, "keyframes").is_empty() {
                let ty = string(field, &["value_type"]).unwrap_or("");
                if let Some(value) = field.get("value").and_then(|v| convert_value(v, ty)) {
                    let mut prop = Property::new(project.alloc(), raw, string(field, &["name"]).unwrap_or(raw), value.clone());
                    prop.ui = ParamUi::Hidden;
                    fill_property(&mut prop, field, value, ty, 1.0, layer_time, losses, context);
                    pin.children.push(prop.into());
                } else {
                    losses.push(format!("{context}: unsupported Basic Puppet pin value {raw}"));
                }
            }
        }
    }
    if let Some(vtx) = child(ir, "ADBE FreePin3 PosPin Vtx Index")
        && let Some(index) = number(vtx, &["value"])
    {
        let mut p = Property::new(project.alloc(), "ae_vertex_index", "AE Vertex Index", Value::Scalar(index));
        p.ui = ParamUi::Hidden;
        p.static_only = true;
        pin.children.push(p.into());
    }
    for field in array(ir, "children") {
        let m = string(field, &["match_name"]).unwrap_or("?");
        if !fields.iter().any(|(raw, _)| *raw == m) && !matches!(m, "ADBE FreePin3 PosPin Vtx Index" | "ADBE FreePin3 PosPin Type") {
            losses.push(format!("{context}: unsupported Puppet pin field {m}"));
        }
    }
    deform.children.push(pin.into());
}

fn import_mesh(project: &mut Project, fx: &mut PropGroup, ir: &Json, layer_time: LayerTime, losses: &mut Vec<String>, context: &str) {
    let pin_group = child(ir, "ADBE FreePin3 PosPins");
    let pins = pin_group.map(|g| array(g, "children")).unwrap_or(&[]);
    let seed = pins.iter().find_map(|p| child(p, "ADBE FreePin3 PosPin Position").and_then(initial_position)).unwrap_or([0.0, 0.0]);
    let mut opts = MeshOpts::default();
    if let Some(n) = child(ir, "ADBE FreePin3 Mesh Tri Count").and_then(|v| number(v, &["value"])) {
        opts.triangles = n;
    }
    let mut mesh = puppet::mesh_group(&mut build::Ids(&mut project.next_id), string(ir, &["name"]).unwrap_or("Mesh"), seed, &opts);
    mesh.enabled = boolean(ir, &["enabled"]).unwrap_or(true);
    raw_state(project, &mut mesh, "pedt_b64", ir, losses, context);
    raw_state(project, &mut mesh, "fpms_b64", ir, losses, context);
    let imported = ir.get("mesh_geometry").and_then(|v| serde_json::from_value::<MeshGeometry>(v.clone()).ok()).and_then(|g| g.clone().native().map(|_| g));
    if let Some(geometry) = imported {
        if let Ok(serialized) = serde_json::to_string(&geometry) {
            let mut p = Property::new(project.alloc(), "imported_geometry", "Imported Geometry", Value::Str(serialized));
            p.ui = ParamUi::Hidden;
            p.static_only = true;
            mesh.children.push(p.into());
        }
        losses.push(format!("{context}: Puppet imported mesh uses native solver; AE frame parity unverified"));
    } else {
        losses.push(format!("{context}: Puppet mesh geometry regenerated from layer alpha; AE frame parity unverified"));
    }
    if let Some(deform) = mesh.sub_mut("deform") {
        for pin in pins {
            import_pin(project, deform, pin, layer_time, losses, context);
        }
        if let Some(starch) = child(ir, "ADBE FreePin3 StarchPins") {
            for pin in array(starch, "children") {
                losses.push(format!("{context}: unsupported Puppet starch pin {}", string(pin, &["name"]).unwrap_or("?")));
            }
        }
    }
    for field in array(ir, "children") {
        let m = string(field, &["match_name"]).unwrap_or("?");
        if !matches!(m, "ADBE FreePin3 Mesh" | "ADBE FreePin3 Mesh Tri Count" | "ADBE FreePin3 PosPins" | "ADBE FreePin3 StarchPins") {
            losses.push(format!("{context}: unsupported Puppet mesh field {m}"));
        }
    }
    fx.children.push(mesh.into());
}

pub(super) fn import(project: &mut Project, fx: &mut PropGroup, ir: &Json, layer_time: LayerTime, losses: &mut Vec<String>, context: &str) {
    if let Some(refinement) = child(ir, "ADBE FreePin3 Auto Rotate Pins") {
        set(fx, "refinement", refinement, layer_time, losses, context);
    }
    if let Some(engine) = child(ir, "ADBE FreePin3 Puppet Engine")
        && number(engine, &["value"]) != Some(2.0)
    {
        losses.push(format!("{context}: unsupported Puppet Engine setting"));
    }
    if let Some(transparent) = child(ir, "ADBE FreePin3 On Transparent")
        && number(transparent, &["value"]) != Some(0.0)
    {
        losses.push(format!("{context}: unsupported Puppet On Transparent setting"));
    }
    if let Some(arap) = child(ir, "ADBE FreePin3 ARAP Group") {
        raw_state(project, fx, "fpol_b64", arap, losses, context);
        if let Some(meshes) = child(arap, "ADBE FreePin3 Mesh Group") {
            for mesh in array(meshes, "children") {
                if string(mesh, &["match_name"]) == Some("ADBE FreePin3 Mesh Atom") {
                    import_mesh(project, fx, mesh, layer_time, losses, context);
                } else {
                    losses.push(format!("{context}: unsupported Puppet mesh group {}", string(mesh, &["match_name"]).unwrap_or("?")));
                }
            }
        }
        for field in array(arap, "children") {
            let m = string(field, &["match_name"]).unwrap_or("?");
            if !matches!(m, "ADBE FreePin3 Outlines" | "ADBE FreePin3 Mesh Group") {
                losses.push(format!("{context}: unsupported Puppet ARAP field {m}"));
            }
        }
    }
    for field in array(ir, "children") {
        let m = string(field, &["match_name"]).unwrap_or("?");
        if m == "ADBE Effect Built In Params" {
            import_builtin_params(field, losses, context);
        } else if !matches!(m, "ADBE FreePin3 Puppet Engine" | "ADBE FreePin3 Auto Rotate Pins" | "ADBE FreePin3 On Transparent" | "ADBE FreePin3 ARAP Group") {
            losses.push(format!("{context}: unsupported Puppet field {m}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inert_builtin_compositing_options_are_not_losses() {
        let mut ir = serde_json::json!({"children": [
            {"match_name": "ADBE Effect Mask Parade", "enabled": true, "children": []},
            {"match_name": "ADBE Effect Mask Opacity", "enabled": true, "value": 100.0, "keyframes": [], "expression_enabled": false},
            {"match_name": "ADBE Force CPU GPU", "enabled": true, "value": 1.0, "keyframes": [], "expression_enabled": false}
        ]});
        let mut losses = Vec::new();
        import_builtin_params(&ir, &mut losses, "test");
        assert!(losses.is_empty());
        ir["children"][1]["value"] = serde_json::json!(50.0);
        import_builtin_params(&ir, &mut losses, "test");
        assert_eq!(losses.len(), 1);
        assert_eq!(losses[0], "test: unsupported Puppet compositing option ADBE Effect Mask Opacity");
    }

    #[test]
    fn imports_editable_mesh_pins_and_keyframes() {
        let ir = serde_json::json!({"children":[
            {"match_name":"ADBE FreePin3 Auto Rotate Pins","value":35},
            {"match_name":"ADBE FreePin3 ARAP Group","children":[
                {"match_name":"ADBE FreePin3 Mesh Group","children":[
                    {"match_name":"ADBE FreePin3 Mesh Atom","name":"Mesh 7","children":[
                        {"match_name":"ADBE FreePin3 Mesh Tri Count","value":80},
                        {"match_name":"ADBE FreePin3 PosPins","children":[
                            {"match_name":"ADBE FreePin3 PosPin Atom","name":"Puppet Pin 5","children":[
                                {"match_name":"ADBE FreePin3 PosPin Vtx Index","value":51},
                                {"match_name":"ADBE FreePin3 PosPin Position","value":[30,40],"keyframes":[{"time":0,"value":[10,20]},{"time":1,"value":[30,40]}]},
                                {"match_name":"ADBE FreePin3 PosPin Scale","value":120,"keyframes":[{"time":0,"value":100},{"time":1,"value":120}]},
                                {"match_name":"ADBE FreePin3 PosPin Rotation","value":0}
                            ]}
                        ]}
                    ]}
                ]}
            ]}
        ]});
        let mut project = Project::default();
        let mut fx =
            effectcraft_effects::instantiate(effectcraft_effects::find(puppet::ID).unwrap(), &mut build::Ids(&mut project.next_id), "Puppet", [100.0, 100.0]);
        let mut losses = vec![];
        import(&mut project, &mut fx, &ir, LayerTime { start: tick(0.0), stretch: 100.0 }, &mut losses, "test");
        let mesh = puppet::meshes(&fx).next().unwrap();
        let pin = puppet::pins(mesh).next().unwrap();
        assert_eq!(fx.get("refinement").unwrap().value.as_f64(), 35.0);
        assert_eq!(mesh.name, "Mesh 7");
        assert_eq!(mesh.get("triangles").unwrap().value.as_f64(), 80.0);
        assert_eq!(pin.name, "Puppet Pin 5");
        assert_eq!(pin.get("rest").unwrap().value.as_vec2(), [10.0, 20.0]);
        assert_eq!(pin.get("position").unwrap().keys.len(), 2);
        assert_eq!(pin.get("kind").unwrap().value.as_enum(), PinKind::Advanced.index());
        assert_eq!(pin.get("scale").unwrap().keys.len(), 2);
        assert_eq!(pin.get("ae_vertex_index").unwrap().value.as_f64(), 51.0);
        assert!(losses.iter().any(|x| x.contains("geometry regenerated")));
    }

    #[test]
    fn keeps_keyed_basic_pin_scale_and_rotation_without_enabling_them() {
        let ir = serde_json::json!({"children":[{"match_name":"ADBE FreePin3 ARAP Group","children":[
            {"match_name":"ADBE FreePin3 Mesh Group","children":[
                {"match_name":"ADBE FreePin3 Mesh Atom","children":[
                    {"match_name":"ADBE FreePin3 PosPins","children":[
                        {"match_name":"ADBE FreePin3 PosPin Atom","name":"Puppet Pin 1","children":[
                            {"match_name":"ADBE FreePin3 PosPin Position","value":[10,20]},
                            {"match_name":"ADBE FreePin3 PosPin Scale","value":100,"keyframes":[{"time":0,"value":100},{"time":1,"value":100}]},
                            {"match_name":"ADBE FreePin3 PosPin Rotation","value":0,"keyframes":[{"time":0,"value":0},{"time":1,"value":0}]}
                        ]}
                    ]}
                ]}
            ]}
        ]}]});
        let mut project = Project::default();
        let mut fx =
            effectcraft_effects::instantiate(effectcraft_effects::find(puppet::ID).unwrap(), &mut build::Ids(&mut project.next_id), "Puppet", [100.0, 100.0]);
        import(&mut project, &mut fx, &ir, LayerTime { start: tick(0.0), stretch: 100.0 }, &mut vec![], "test");
        let pin = puppet::meshes(&fx).flat_map(puppet::pins).next().unwrap();
        assert_eq!(pin.get("kind").unwrap().value.as_enum(), PinKind::Position.index());
        assert!(pin.get("scale").is_none());
        for raw in ["ADBE FreePin3 PosPin Scale", "ADBE FreePin3 PosPin Rotation"] {
            let prop = pin.get(raw).unwrap();
            assert_eq!(prop.keys.len(), 2);
            assert_eq!(prop.ui, ParamUi::Hidden);
        }
    }

    #[test]
    fn imports_rest_topology_and_preserves_opaque_states() {
        let ir = serde_json::json!({"children":[{"match_name":"ADBE FreePin3 ARAP Group","fpol_b64":"outline","children":[
            {"match_name":"ADBE FreePin3 Mesh Group","children":[{"match_name":"ADBE FreePin3 Mesh Atom",
                "pedt_b64":"mesh","fpms_b64":"cache",
                "mesh_geometry":{"vertices":[[0.0,0.0],[10.0,0.0],[0.0,10.0]],"triangles":[[0,1,2]]},
                "children":[]}]}
        ]}]});
        let mut project = Project::default();
        let mut fx =
            effectcraft_effects::instantiate(effectcraft_effects::find(puppet::ID).unwrap(), &mut build::Ids(&mut project.next_id), "Puppet", [10.0, 10.0]);
        let mut losses = vec![];
        import(&mut project, &mut fx, &ir, LayerTime { start: tick(0.0), stretch: 100.0 }, &mut losses, "test");
        let mesh = puppet::meshes(&fx).next().unwrap();
        assert!(mesh.get("imported_geometry").is_some());
        assert_eq!(mesh.get("pedt_b64").unwrap().value, Value::Str("mesh".into()));
        assert_eq!(mesh.get("fpms_b64").unwrap().value, Value::Str("cache".into()));
        assert_eq!(fx.get("fpol_b64").unwrap().value, Value::Str("outline".into()));
        assert!(losses.iter().any(|loss| loss.contains("AE frame parity unverified")));
    }
}

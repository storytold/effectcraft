//! Advanced 3D: model layers (File ▸ Import of glTF/OBJ/STL/FBX, Layer ▸ New ▸ 3D primitives), the
//! comp renderer switch, Layer ▸ Environment Layer, Geometry Options for extruded text and
//! shapes, and Layer ▸ Material commands.

use effectcraft_keyframe::Value as KV;
use effectcraft_project::build::{self, Ids};
use effectcraft_project::{FootageKind, ItemId, ItemKind, Layer, LayerSource, Node, PrimitiveKind, PropGroup, Renderer};
use effectcraft_time::Tick;
use serde_json::{Value, json};

use super::layer::{color_p, index_p, insert_layer, insert_layer_at, place, position_p};
use super::{CommandSpec, b_p, bad, comp_id, f_p, has_comp, has_layers, layer_mut, layer_p, layers_p, str_p};
use crate::{EngineError, Result, Session, cmd};

// ---------------------------------------------------------------- geometry options

/// Add Geometry Options (Bevel Style, Bevel Depth, Hole Bevel Depth, Extrusion Depth) to a text
/// or shape layer that has none.
pub(crate) fn add_geometry_options(next_id: &mut u64, l: &mut Layer) {
    if !matches!(l.source, LayerSource::Text | LayerSource::Shape) || l.props.sub("geometryOptions").is_some() {
        return;
    }
    let g = build::extrusion_geometry_options(&mut Ids(next_id));
    // After Transform, like After Effects.
    let at = l.props.children.iter().position(|c| c.match_id() == "transform").map_or(l.props.children.len(), |i| i + 1);
    l.props.children.insert(at, g.into());
}

/// Advanced 3D comps: give every text and shape layer Geometry Options.
pub(crate) fn sync_geometry_options(proj: &mut effectcraft_project::Project, cid: ItemId) {
    if proj.comp(cid).is_none_or(|c| c.renderer != Renderer::Advanced3D) {
        return;
    }
    let mut next = proj.next_id;
    if let Some(c) = proj.comp_mut(cid) {
        for l in &mut c.layers {
            add_geometry_options(&mut next, l);
        }
    }
    proj.next_id = next;
}

fn renderer_p(p: &Value, cmd: &str) -> Result<Renderer> {
    let v = str_p(p, "renderer").or_else(|| str_p(p, "value")).ok_or_else(|| bad(cmd, "missing `renderer` (advanced3d|classic3d)"))?;
    match v.to_ascii_lowercase().replace([' ', '_', '-'], "").as_str() {
        "advanced3d" | "advanced" => Ok(Renderer::Advanced3D),
        "classic3d" | "classic" => Ok(Renderer::Classic3D),
        _ => Err(bad(cmd, format!("renderer: advanced3d|classic3d, not `{v}`"))),
    }
}

fn comp_renderer(s: &mut Session, p: &Value) -> Result<Value> {
    let cid = comp_id(s, p)?;
    if p.get("renderer").is_none() && p.get("value").is_none() {
        let r = s.project.comp(cid).ok_or(EngineError::NoComp)?.renderer;
        return Ok(json!({"renderer": renderer_id(r)}));
    }
    let r = renderer_p(p, "comp.renderer")?;
    s.edit("Change Renderer", None, |proj, _| {
        proj.comp_mut(cid).ok_or(EngineError::NoComp)?.renderer = r;
        sync_geometry_options(proj, cid);
        Ok(())
    })?;
    if r == Renderer::Classic3D && s.project.comp(cid).is_some_and(|c| c.layers.iter().any(|l| l.source.is_model())) {
        s.toast("Classic 3D doesn't draw 3D model layers: they show with the Advanced 3D renderer");
    }
    if r == Renderer::Classic3D {
        let env = |l: &Layer| matches!(l.source, LayerSource::Light { kind: effectcraft_project::LightKind::Environment });
        if s.project.comp(cid).is_some_and(|c| c.layers.iter().any(env)) {
            s.toast("Classic 3D doesn't light Environment light layers: they light with the Advanced 3D renderer");
        }
    }
    Ok(json!({"renderer": renderer_id(r)}))
}

fn renderer_id(r: Renderer) -> &'static str {
    match r {
        Renderer::Advanced3D => "advanced3d",
        Renderer::Classic3D => "classic3d",
    }
}

/// Only the Advanced 3D renderer draws model and primitive layers, so adding one switches a
/// Classic 3D comp to it, like After Effects (inside the adding edit: one undo step). Whether
/// the renderer changed.
pub(crate) fn use_advanced_3d(proj: &mut effectcraft_project::Project, cid: ItemId) -> Result<bool> {
    let c = proj.comp_mut(cid).ok_or(EngineError::NoComp)?;
    let switched = c.renderer != Renderer::Advanced3D;
    c.renderer = Renderer::Advanced3D;
    sync_geometry_options(proj, cid);
    Ok(switched)
}

/// Tell the user when [`use_advanced_3d`] changed the comp's renderer.
pub(crate) fn report_switch(s: &mut Session, cid: ItemId, switched: bool, why: &str) {
    if switched {
        let name = s.project.item(cid).map_or("the composition", |i| i.name.as_str()).to_string();
        s.toast(format!("\"{name}\" now uses the Advanced 3D renderer, {why}"));
    }
}

// ---------------------------------------------------------------- model layers

/// Set a property's value at comp time `t` (adds a key when it is animated).
fn set_at(l: &mut Layer, path: &str, t: Tick, v: KV) -> bool {
    let lt = l.layer_time(t);
    match l.props.prop_mut(path) {
        Some(pr) => {
            pr.set_value_at(lt, v);
            true
        }
        None => false,
    }
}

/// New 3D model layer from a model footage item (`item`) or a file (`path`, imported first).
pub(crate) fn new_model(s: &mut Session, p: &Value) -> Result<Value> {
    let cid = comp_id(s, p)?;
    let item = match (p.get("item"), str_p(p, "path")) {
        (Some(Value::Number(n)), _) => ItemId(n.as_u64().unwrap_or(0)),
        (Some(Value::String(name)), _) => s.project.find_by_name(name).map(|i| i.id).ok_or_else(|| bad("layer.newModel", format!("no item `{name}`")))?,
        (_, Some(path)) => {
            let r = s.execute("file.import", json!({"paths": [path]}))?;
            let id = r.get("items").and_then(|a| a.get(0)).and_then(Value::as_u64);
            let err = r.get("errors").and_then(|e| e.get(0)).and_then(Value::as_str).unwrap_or("import failed").to_string();
            ItemId(id.ok_or_else(|| bad("layer.newModel", err))?)
        }
        _ => *s.state.project_selection.first().ok_or_else(|| bad("layer.newModel", "missing `item` or `path`"))?,
    };
    let it = s.project.item(item).ok_or_else(|| bad("layer.newModel", "no such item"))?.clone();
    let ItemKind::Footage(f) = &it.kind else { return Err(bad("layer.newModel", format!("`{}` is not a 3D model", it.name))) };
    if f.kind != FootageKind::Model {
        return Err(bad("layer.newModel", format!("`{}` is not a 3D model", it.name)));
    }
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?.clone();
    // Fit the model to half the comp height, and list its animation clips.
    let model = s.footage.model(item, f);
    let (scale, clips) = match &model {
        Some(m) => {
            let ext = m.bounds().map_or(1.0, |(lo, hi)| (0..3).map(|k| hi[k] - lo[k]).fold(0.0f64, f64::max));
            let scale = if ext > 1e-9 { comp.height as f64 * 0.5 / ext } else { 100.0 };
            let clips = m.animations.iter().enumerate().map(|(i, a)| if a.name.is_empty() { format!("Animation {}", i + 1) } else { a.name.clone() }).collect();
            (scale, clips)
        }
        None => (100.0, vec![]),
    };
    let name = str_p(p, "name").unwrap_or(&it.name).to_string();
    let fr = comp.frame_rate;
    let start = fr.snap_nearest(f_p(p, "time").map(Tick::from_seconds_f64).unwrap_or(Tick::ZERO));
    let index = index_p(p, "layer.newModel")?;
    let position = position_p(p, "layer.newModel")?;
    let (id, switched) = s.edit("New 3D Model Layer", None, |proj, st| {
        let mut l = build::layer(proj, &comp, &name, LayerSource::Model { item }, (comp.width, comp.height), None);
        let g = build::model_geometry_options(&mut Ids(&mut proj.next_id), scale, &clips);
        if let Some(slot) = l.props.children.iter_mut().find(|c| c.match_id() == "geometryOptions") {
            *slot = g.into();
        }
        l.start_time = start;
        l.in_point = start;
        l.out_point = comp.duration.max(start + fr.frame_duration());
        place(&mut l, position);
        let id = insert_layer_at(proj, st, cid, l, index)?;
        Ok((id, use_advanced_3d(proj, cid)?))
    })?;
    report_switch(s, cid, switched, "which draws 3D models");
    Ok(json!({"layer": id.0, "item": item.0, "advanced3d": true}))
}

fn kind_p(p: &Value, cmd: &str) -> Result<PrimitiveKind> {
    let k = str_p(p, "kind").or_else(|| str_p(p, "type")).ok_or_else(|| bad(cmd, "missing `kind` (cube|sphere|plane|torus|cone|cylinder)"))?;
    PrimitiveKind::parse(k).ok_or_else(|| bad(cmd, format!("kind: cube|sphere|plane|torus|cone|cylinder, not `{k}`")))
}

/// Parse Material Options parameters onto a layer at comp time `t`.
fn apply_material(l: &mut Layer, p: &Value, t: Tick, cmd: &str) -> Result<usize> {
    let mut n = 0;
    let tri = |v: &Value| -> Option<u32> {
        match v {
            Value::Bool(b) => Some(*b as u32),
            Value::Number(x) => x.as_u64().map(|x| x.min(2) as u32),
            Value::String(s) => match s.to_ascii_lowercase().as_str() {
                "off" | "false" => Some(0),
                "on" | "true" => Some(1),
                "only" => Some(2),
                _ => None,
            },
            _ => None,
        }
    };
    for k in ["castsShadows", "acceptsShadows"] {
        if let Some(v) = p.get(k) {
            let e = tri(v).ok_or_else(|| bad(cmd, format!("{k}: off|on|only")))?;
            n += set_at(l, &format!("materialOptions/{k}"), t, KV::Enum(e)) as usize;
        }
    }
    for k in ["acceptsLights", "appearsInReflections"] {
        if let Some(v) = b_p(p, k) {
            n += set_at(l, &format!("materialOptions/{k}"), t, KV::Bool(v)) as usize;
        }
    }
    for k in ["lightTransmission", "ambient", "diffuse", "specularIntensity", "specularShininess", "metal", "metallic", "roughness"] {
        if let Some(v) = f_p(p, k) {
            n += set_at(l, &format!("materialOptions/{k}"), t, KV::Scalar(v)) as usize;
        }
    }
    for k in ["baseColor", "emissive"] {
        if let Some(c) = color_p(p, k) {
            n += set_at(l, &format!("materialOptions/{k}"), t, KV::Color([c[0] as f64, c[1] as f64, c[2] as f64, 1.0])) as usize;
        }
    }
    Ok(n)
}

fn new_primitive(s: &mut Session, p: &Value) -> Result<Value> {
    let cid = comp_id(s, p)?;
    let kind = kind_p(p, "layer.newPrimitive")?;
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?.clone();
    let name = str_p(p, "name").map(str::to_string).unwrap_or_else(|| kind.label().to_string());
    let t = super::time_p(s, p, Some(&comp));
    let (id, switched) = s.edit(&format!("New {}", kind.label()), None, |proj, st| {
        let mut l = build::layer(proj, &comp, &name, LayerSource::Primitive { kind }, (comp.width, comp.height), None);
        for k in ["width", "height", "depth", "radius", "tubeRadius", "segments", "rings"] {
            if let Some(v) = f_p(p, k) {
                set_at(&mut l, &format!("geometryOptions/{k}"), t, KV::Scalar(v.max(0.0)));
            }
        }
        if let Some(pos) = p.get("position").and_then(Value::as_array) {
            let g = |i: usize| pos.get(i).and_then(Value::as_f64).unwrap_or(0.0);
            set_at(&mut l, "transform/position", t, KV::Vec3([g(0), g(1), g(2)]));
        }
        apply_material(&mut l, p, t, "layer.newPrimitive")?;
        let id = insert_layer(proj, st, cid, l)?;
        Ok((id, use_advanced_3d(proj, cid)?))
    })?;
    report_switch(s, cid, switched, "which draws 3D models");
    Ok(json!({"layer": id.0, "kind": kind.label(), "advanced3d": true}))
}

// ---------------------------------------------------------------- environment layer

fn environment(s: &mut Session, p: &Value) -> Result<Value> {
    let (cid, ids) = layers_p(s, p)?;
    if ids.is_empty() {
        return Err(bad("layer.environment", "select a footage or composition layer"));
    }
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    for id in &ids {
        let l = comp.layer(*id).ok_or(EngineError::NoComp)?;
        if !l.source.is_av() || l.source.is_model() {
            return Err(bad("layer.environment", format!("`{}` can't be an environment layer (use a footage, comp or solid layer)", l.name)));
        }
    }
    let on = b_p(p, "on").unwrap_or_else(|| !comp.layer(ids[0]).is_some_and(|l| l.environment));
    s.edit("Environment Layer", None, |proj, _| {
        for id in &ids {
            let l = layer_mut(proj, cid, *id)?;
            l.environment = on;
            if on {
                l.switches.three_d = true;
            }
        }
        Ok(())
    })?;
    Ok(json!({"environment": on, "layers": ids.iter().map(|l| l.0).collect::<Vec<_>>()}))
}

// ---------------------------------------------------------------- materials

fn material_set(s: &mut Session, p: &Value) -> Result<Value> {
    let (cid, ids) = layers_p(s, p)?;
    if ids.is_empty() {
        return Err(bad("material.set", "no `layers` given and no layer selected"));
    }
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?.clone();
    let t = super::time_p(s, p, Some(&comp));
    let n = s.edit("Material Options", None, |proj, _| {
        let mut n = 0;
        for id in &ids {
            n += apply_material(layer_mut(proj, cid, *id)?, p, t, "material.set")?;
        }
        Ok(n)
    })?;
    if n == 0 {
        return Err(bad(
            "material.set",
            "no Material Options property matched (castsShadows, acceptsShadows, acceptsLights, baseColor, metallic, roughness, …)",
        ));
    }
    Ok(json!({"set": n}))
}

/// Fresh Material Options for a layer.
fn default_material(next: &mut u64, l: &Layer) -> Option<PropGroup> {
    let ids = &mut Ids(next);
    match l.source {
        LayerSource::Primitive { .. } => Some(build::model_material_options(ids, true)),
        LayerSource::Model { .. } => Some(build::model_material_options(ids, false)),
        _ if l.source.is_av() => Some(build::material_options(ids)),
        _ => None,
    }
}

fn replace_material(l: &mut Layer, g: PropGroup) {
    match l.props.children.iter_mut().find(|c| c.match_id() == "materialOptions") {
        Some(slot) => *slot = Node::Group(g),
        None => l.props.children.push(g.into()),
    }
}

fn material_reset(s: &mut Session, p: &Value) -> Result<Value> {
    let (cid, ids) = layers_p(s, p)?;
    if ids.is_empty() {
        return Err(bad("material.reset", "no `layers` given and no layer selected"));
    }
    s.edit("Reset Material", None, |proj, _| {
        let mut next = proj.next_id;
        for id in &ids {
            let l = layer_mut(proj, cid, *id)?;
            if let Some(g) = default_material(&mut next, l) {
                replace_material(l, g);
            }
        }
        proj.next_id = next;
        Ok(())
    })?;
    Ok(json!({"layers": ids.iter().map(|l| l.0).collect::<Vec<_>>()}))
}

/// Duplicate and Assign Material: the first layer's Material Options are copied onto the other
/// selected layers of the same kind.
fn material_duplicate(s: &mut Session, p: &Value) -> Result<Value> {
    let (cid, ids) = layers_p(s, p)?;
    if ids.len() < 2 {
        return Err(bad("material.duplicateAssign", "select the source layer first, then the layers to assign its material to"));
    }
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    let src = comp
        .layer(ids[0])
        .and_then(|l| l.props.sub("materialOptions"))
        .cloned()
        .ok_or_else(|| bad("material.duplicateAssign", "the first layer has no material"))?;
    let n = s.edit("Duplicate and Assign Material", None, |proj, _| {
        let mut n = 0;
        for id in &ids[1..] {
            let l = layer_mut(proj, cid, *id)?;
            let Some(dst) = l.props.sub("materialOptions") else { continue };
            // Property sets differ (models, primitives, cards): copy the values they share.
            let mut g = dst.clone();
            for c in g.children.iter_mut() {
                if let Node::Prop(d) = c
                    && let Some(sv) = src.get(&d.match_id)
                {
                    d.value = sv.value.clone();
                    d.keys = sv.keys.clone();
                    d.expr = sv.expr.clone();
                }
            }
            replace_material(l, g);
            n += 1;
        }
        Ok(n)
    })?;
    Ok(json!({"assigned": n}))
}

fn material_reveal(s: &mut Session, p: &Value) -> Result<Value> {
    let (cid, lid) = layer_p(s, p, "material.revealSource")?;
    let l = s.project.comp(cid).and_then(|c| c.layer(lid)).ok_or(EngineError::NoComp)?;
    let LayerSource::Model { item } = l.source else { return Err(bad("material.revealSource", "the layer's material is its own (not from a model file)")) };
    s.state.project_selection = vec![item];
    Ok(json!({"item": item.0}))
}

fn has_model_or_layers(s: &Session) -> std::result::Result<(), String> {
    has_layers(s)
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "layer.newModel",
            "3D Model Layer",
            [],
            None,
            "{item?: id or name of a 3D model footage item, path?: a .gltf, .glb, .obj, .stl or .fbx file to import, name?, time? (s), index? (1-based stack position), position? ([x, y] comp px)}; switches a Classic 3D comp to Advanced 3D",
            has_comp,
            new_model
        ),
        cmd!(
            "layer.new3dPrimitive",
            "3D Primitive",
            [],
            None,
            "{kind: cube|sphere|plane|torus|cone|cylinder, name?, width?, height?, depth?, radius?, tubeRadius?, segments?, rings?, position? [x,y,z], baseColor?, metallic?, roughness?, emissive?, castsShadows?, acceptsShadows?, acceptsLights?}; switches a Classic 3D comp to Advanced 3D",
            has_comp,
            new_primitive
        ),
        cmd!(
            "layer.newPrimitive",
            "New 3D Primitive",
            [],
            None,
            "{kind: cube|sphere|plane|torus|cone|cylinder, name?, width?, height?, depth?, radius?, tubeRadius?, segments?, rings?, position? [x,y,z], baseColor?, metallic?, roughness?, emissive?, castsShadows?, acceptsShadows?, acceptsLights?}; switches a Classic 3D comp to Advanced 3D",
            has_comp,
            new_primitive
        ),
        cmd!("comp.renderer", "Renderer", [], None, "{renderer?: advanced3d|classic3d (omit to read), value?}", has_comp, comp_renderer),
        cmd!("layer.environment", "Environment Layer", ["Layer"], None, "{layers?, on?}", has_layers, environment),
        cmd!(
            "material.set",
            "Material Options",
            [],
            None,
            "{layers?, castsShadows?: off|on|only, acceptsShadows?: off|on|only, acceptsLights?, appearsInReflections?, lightTransmission?, ambient?, diffuse?, specularIntensity?, specularShininess?, metal?, baseColor?, metallic?, roughness?, emissive?, time?}",
            has_layers,
            material_set
        ),
        cmd!("material.revealSource", "Reveal Material Source in Project", ["Layer", "Material"], None, "{layer?}", has_layers, material_reveal),
        cmd!("material.reset", "Reset Material", ["Layer", "Material"], None, "{layers?}", has_model_or_layers, material_reset),
        cmd!(
            "material.duplicateAssign",
            "Duplicate and Assign Material",
            ["Layer", "Material"],
            None,
            "{layers? (first = source)}",
            has_layers,
            material_duplicate
        ),
    ]
}

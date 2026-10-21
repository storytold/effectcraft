//! Animation menu: animation presets (EffectCraft's own JSON format), Add Keyframe, Keyframe
//! Assistant (Exponential Scale), text animators/selectors, and the
//! Reveal Properties commands.

use std::collections::BTreeMap;

use effectcraft_keyframe::{Interp, Keyframe, Value as KV};
use effectcraft_project::build::{self, Ids};
use effectcraft_project::{GroupKind, Layer, LayerId, LayerSource, Node, Project, PropGroup, Property, Uid};
use effectcraft_time::Tick;
use serde_json::{Value, json};

use super::{CommandSpec, bad, frontend, has_comp, has_keys, has_layers, layer_mut, layers_p, match_path_of, resolve_effect_path, selected_leaf_props, str_p};
use crate::{EngineError, KeyRef, Result, Session, cmd};

/// Preset file format version tag (`*.ecpreset`, JSON).
pub const PRESET_TAG: &str = "effectcraftAnimationPreset";

fn has_selected_props(s: &Session) -> std::result::Result<(), String> {
    has_comp(s)?;
    if selected_leaf_props(s).is_empty() && selected_effects(s).is_empty() { Err("select properties or effects first".into()) } else { Ok(()) }
}

fn has_text_layer(s: &Session) -> std::result::Result<(), String> {
    has_layers(s)?;
    let c = s.active_comp().ok_or("no composition")?;
    if s.state.selected_layers.iter().filter_map(|l| c.layer(*l)).any(|l| l.props.group("text/animators").is_some()) {
        Ok(())
    } else {
        Err("select a text layer".into())
    }
}

/// Selected effect groups (layer, effect group).
fn selected_effects(s: &Session) -> Vec<(LayerId, PropGroup)> {
    let Some(c) = s.active_comp() else { return vec![] };
    s.state
        .selected_props
        .iter()
        .filter_map(|(l, u)| {
            let layer = c.layer(*l)?;
            let fx = layer.effects()?;
            fx.groups().find(|g| g.uid == *u).map(|g| (*l, g.clone()))
        })
        .collect()
}

// ---------------------------------------------------------------- presets

fn save_preset(s: &mut Session, p: &Value) -> Result<Value> {
    let path = str_p(p, "path").ok_or_else(|| bad("anim.savePreset", "missing `path`"))?.to_string();
    let comp = s.active_comp().ok_or(EngineError::NoComp)?;
    let t = s.time();
    let effects: Vec<(LayerId, PropGroup)> = selected_effects(s);
    let effect_uids: Vec<Uid> = effects.iter().map(|(_, g)| g.uid).collect();
    let mut props = vec![];
    for (lid, uid) in selected_leaf_props(s) {
        let Some(l) = comp.layer(lid) else { continue };
        // Properties inside a saved effect travel with the effect.
        if l.effects().is_some_and(|fx| fx.groups().any(|g| effect_uids.contains(&g.uid) && g.find(uid).is_some())) {
            continue;
        }
        let (Some(pr), Some(mp)) = (l.props.find(uid), match_path_of(&l.props, uid)) else { continue };
        props.push((l.layer_time(t), mp, pr.clone()));
    }
    if props.is_empty() && effects.is_empty() {
        return Err(bad("anim.savePreset", "select properties or effects to save"));
    }
    // Key times are stored relative to the earliest key so the preset applies at the CTI.
    let first = props.iter().flat_map(|(_, _, pr)| pr.keys.iter().map(|k| k.time)).min().unwrap_or(Tick::ZERO);
    let props_json: Vec<Value> = props
        .iter()
        .map(|(_, mp, pr)| {
            let mut pr = pr.clone();
            for k in &mut pr.keys {
                k.time -= first;
            }
            json!({"path": mp, "value": pr.value, "keys": pr.keys, "expression": pr.expr})
        })
        .collect();
    let fx_json: Vec<Value> = effects.iter().map(|(_, g)| serde_json::to_value(g).unwrap_or_default()).collect();
    let name = str_p(p, "name")
        .map(str::to_string)
        .unwrap_or_else(|| std::path::Path::new(&path).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default());
    let doc = json!({PRESET_TAG: 1, "name": name, "properties": props_json, "effects": fx_json});
    let text = serde_json::to_string_pretty(&doc).unwrap_or_default();
    s.services.write_file(&path, text.as_bytes()).map_err(|e| EngineError::Other(format!("cannot write {path}: {e}")))?;
    Ok(json!({"path": path, "properties": props.len(), "effects": effects.len()}))
}

fn apply_preset(s: &mut Session, p: &Value) -> Result<Value> {
    let doc: Value = match (p.get("preset"), str_p(p, "path")) {
        (Some(v), _) => v.clone(),
        (None, Some(path)) => {
            let bytes = s.services.read_file(path).map_err(|e| EngineError::Other(format!("cannot read {path}: {e}")))?;
            serde_json::from_slice(&bytes).map_err(|e| bad("anim.applyPreset", format!("not a preset: {e}")))?
        }
        _ => return Err(bad("anim.applyPreset", "missing `path` (or inline `preset`)")),
    };
    if doc.get(PRESET_TAG).is_none() {
        return Err(bad("anim.applyPreset", "not an EffectCraft animation preset"));
    }
    let (cid, ids) = layers_p(s, p)?;
    if ids.is_empty() {
        return Err(bad("anim.applyPreset", "select a layer"));
    }
    // Animation ▸ Recent Animation Presets.
    if let Some(path) = str_p(p, "path") {
        s.prefs.push_recent_preset(path);
        s.save_prefs();
        s.prefs_revision += 1;
    }
    let effects: Vec<PropGroup> =
        doc["effects"].as_array().map(|a| a.iter().filter_map(|v| serde_json::from_value(v.clone()).ok()).collect()).unwrap_or_default();
    let props: Vec<(String, Property)> = doc["properties"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| {
                    let path = v["path"].as_str()?.to_string();
                    let value: KV = serde_json::from_value(v["value"].clone()).ok()?;
                    let keys: Vec<Keyframe> = serde_json::from_value(v["keys"].clone()).unwrap_or_default();
                    let expr = serde_json::from_value(v["expression"].clone()).ok().flatten();
                    let mut pr = Property::new(0, "", "", value);
                    pr.keys = keys;
                    pr.expr = expr;
                    Some((path, pr))
                })
                .collect()
        })
        .unwrap_or_default();
    let t = s.time();
    let mut applied = 0;
    let mut skipped = vec![];
    s.edit("Apply Animation Preset", None, |proj, _| {
        let mut next = proj.next_id;
        for lid in &ids {
            let l = layer_mut(proj, cid, *lid)?;
            let lt = l.layer_time(t);
            if let Some(fx) = l.props.sub_mut("effects") {
                for g in &effects {
                    let mut g = g.clone();
                    g.reassign_uids(&mut next);
                    next += 1;
                    fx.children.push(Node::Group(g));
                    applied += 1;
                }
            }
            for (path, src) in &props {
                let resolved = resolve_effect_path(l, path);
                match l.props.prop_mut(&resolved) {
                    Some(pr) if std::mem::discriminant(&pr.value) == std::mem::discriminant(&src.value) => {
                        pr.value = src.value.clone();
                        pr.keys = src
                            .keys
                            .iter()
                            .cloned()
                            .map(|mut k| {
                                k.time += lt;
                                k
                            })
                            .collect();
                        if src.expr.is_some() {
                            pr.expr = src.expr.clone();
                        }
                        applied += 1;
                    }
                    _ => skipped.push(path.clone()),
                }
            }
        }
        proj.next_id = next;
        Ok(())
    })?;
    Ok(json!({"applied": applied, "skipped": skipped}))
}

// ---------------------------------------------------------------- keyframes

fn add_keyframe(s: &mut Session, _: &Value) -> Result<Value> {
    let targets = selected_leaf_props(s);
    if targets.is_empty() {
        return Err(bad("anim.addKeyframe", "select properties first"));
    }
    let cid = s.active_comp_id().ok_or(EngineError::NoComp)?;
    let t = s.time();
    let n = s.edit("Add Keyframe", None, |proj, _| {
        let mut n = 0;
        for (lid, uid) in &targets {
            let l = layer_mut(proj, cid, *lid)?;
            let lt = l.layer_time(t);
            let Some(pr) = l.props.find_mut(*uid) else { continue };
            if pr.static_only {
                continue;
            }
            if pr.keys.is_empty() {
                pr.set_animated(true, lt);
            } else {
                let v = pr.value_at(lt);
                pr.set_value_at(lt, v);
            }
            n += 1;
        }
        Ok(n)
    })?;
    Ok(json!({"keys": n}))
}

/// Selected keys grouped by (layer, property).
fn keys_by_prop(s: &Session) -> BTreeMap<(LayerId, Uid), Vec<Tick>> {
    let mut m: BTreeMap<(LayerId, Uid), Vec<Tick>> = BTreeMap::new();
    for k in &s.state.selected_keys {
        m.entry((k.layer, k.prop)).or_default().push(k.time);
    }
    m
}

fn exponential_scale(s: &mut Session, _: &Value) -> Result<Value> {
    let cid = s.active_comp_id().ok_or(EngineError::NoComp)?;
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    let fd = comp.frame_duration();
    let groups = keys_by_prop(s);
    let mut done = 0;
    let mut work = vec![];
    for ((lid, uid), mut times) in groups {
        let Some(l) = comp.layer(lid) else { continue };
        if l.transform().and_then(|tr| tr.get("scale")).map(|p| p.uid) != Some(uid) {
            continue;
        }
        times.sort();
        if times.len() >= 2 {
            work.push((lid, uid, times[0], *times.last().unwrap_or(&times[0])));
        }
    }
    if work.is_empty() {
        return Err(bad("keys.exponentialScale", "select two Scale keyframes"));
    }
    s.edit("Exponential Scale", None, |proj, st| {
        for (lid, uid, a, b) in &work {
            let l = layer_mut(proj, cid, *lid)?;
            let Some(pr) = l.props.find_mut(*uid) else { continue };
            let (va, vb) = (pr.value_at(*a).as_vec3(), pr.value_at(*b).as_vec3());
            pr.keys.retain(|k| k.time <= *a || k.time >= *b);
            let n = ((*b - *a).0 / fd.0.max(1)).max(1);
            for i in 0..=n {
                let u = i as f64 / n as f64;
                let t = if i == n { *b } else { *a + Tick(fd.0 * i) };
                let f = |x: f64, y: f64| if x.abs() < 1e-9 || y.abs() < 1e-9 || x.signum() != y.signum() { x + (y - x) * u } else { x * (y / x).powf(u) };
                let v = [f(va[0], vb[0]), f(va[1], vb[1]), f(va[2], vb[2])];
                let mut k = Keyframe::new(t, KV::Vec3(v));
                k.in_interp = Interp::Linear;
                k.out_interp = Interp::Linear;
                effectcraft_keyframe::set_key(&mut pr.keys, k);
                st.selected_keys.push(KeyRef { layer: *lid, prop: *uid, time: t });
            }
            done += 1;
        }
        st.selected_keys.sort_by_key(|k| (k.layer.0, k.prop, k.time));
        st.selected_keys.dedup();
        Ok(())
    })?;
    Ok(json!({"properties": done}))
}

// ---------------------------------------------------------------- text animators

/// Animation ▸ Add Text Selector ▸ Range / Wiggly / Expression (see `layer.addTextSelector`).
fn add_selector(s: &mut Session, p: &Value) -> Result<Value> {
    super::text_anim::add_selector(s, p)
}

fn remove_all_animators(s: &mut Session, p: &Value) -> Result<Value> {
    let (cid, ids) = layers_p(s, p)?;
    s.edit("Remove All Text Animators", None, |proj, _| {
        for lid in &ids {
            if let Some(a) = layer_mut(proj, cid, *lid)?.props.group_mut("text/animators") {
                a.children.clear();
            }
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

// ---------------------------------------------------------------- reveal

/// Whether a static property differs from the value a fresh layer would have. Transform
/// defaults depend on the source size and comp size.
fn modified(pr: &Property, group: &str, default_anchor: [f64; 2], comp_center: [f64; 2]) -> bool {
    if pr.is_animated() || pr.expr.is_some() {
        return true;
    }
    let v = &pr.value;
    let near = |a: &[f64], b: &[f64]| a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-6);
    match (group, pr.match_id.as_str()) {
        ("transform", "anchor") => !near(&v.as_vec2(), &default_anchor),
        ("transform", "position") => !near(&v.as_vec2(), &comp_center) || v.as_vec3()[2].abs() > 1e-6,
        // Separate Dimensions: X and Y Position default to the comp centre.
        ("transform", "positionX") => (v.as_f64() - comp_center[0]).abs() > 1e-6,
        ("transform", "positionY") => (v.as_f64() - comp_center[1]).abs() > 1e-6,
        ("transform", "scale") => !near(&v.as_vec3(), &[100.0; 3]),
        ("transform", "opacity") => (v.as_f64() - 100.0).abs() > 1e-6,
        ("transform", "orientation") => !near(&v.as_vec3(), &[0.0; 3]),
        ("transform", _) => v.as_f64().abs() > 1e-6,
        _ => false,
    }
}

/// The group a fresh layer of `l`'s kind (`fresh`) has in place of `top`, for the groups whose
/// defaults are fixed: Audio, Material / Geometry / Camera / Light Options, a text layer's Path
/// and More Options, and a camera's or light's Transform (whose defaults aren't a 2D layer's).
fn default_group(top: &PropGroup, l: &Layer, fresh: &Layer) -> Option<PropGroup> {
    let mut next = 0;
    let mut ids = Ids(&mut next);
    let camera_or_light = matches!(l.source, LayerSource::Camera | LayerSource::Light { .. });
    let mut g = match top.match_id.as_str() {
        "transform" if !camera_or_light => return None,
        // Added to a layer after it is made (a source with audio, an Advanced 3D comp).
        "audio" => build::audio(&mut ids),
        "geometryOptions" if matches!(l.source, LayerSource::Text | LayerSource::Shape) => build::extrusion_geometry_options(&mut ids),
        "transform" | "cameraOptions" | "lightOptions" | "materialOptions" | "geometryOptions" | "text" => fresh.props.sub(&top.match_id)?.clone(),
        _ => return None,
    };
    match top.match_id.as_str() {
        // Source Text and the animators are the layer's content, not options with a default.
        "text" => g.children.retain(|c| matches!(c.match_id(), "pathOptions" | "moreOptions")),
        // A model's scale and clip come from its file.
        "geometryOptions" if matches!(l.source, LayerSource::Model { .. }) => g.children.retain(|c| !matches!(c.match_id(), "unitScale" | "animation")),
        _ => {}
    }
    Some(g)
}

/// Static properties of `g` whose value differs from the same property of `default` (matched by
/// match id; properties `default` lacks have no known default and are left out).
fn changed_from(g: &PropGroup, default: &PropGroup, out: &mut Vec<Uid>) {
    for c in &g.children {
        match c {
            Node::Prop(pr) => {
                if default.get(&pr.match_id).is_some_and(|d| d.value != pr.value) && !out.contains(&pr.uid) {
                    out.push(pr.uid);
                }
            }
            Node::Group(sg) => {
                if let Some(d) = default.sub(&sg.match_id) {
                    changed_from(sg, d, out);
                }
            }
        }
    }
}

/// What of effect group `g` differs from `new`, a new instance of the effect: parameters with
/// another value, and in full the groups a new instance lacks (puppet meshes and pins, paint
/// strokes: what the user made).
fn effect_changes(g: &PropGroup, new: &PropGroup, out: &mut Vec<Uid>) {
    for c in &g.children {
        match c {
            Node::Prop(pr) => {
                if new.get(&pr.match_id).is_some_and(|d| d.value != pr.value) && !out.contains(&pr.uid) {
                    out.push(pr.uid);
                }
            }
            Node::Group(sg) => match new.sub(&sg.match_id) {
                Some(d) => effect_changes(sg, d, out),
                None => out.push(sg.uid),
            },
        }
    }
}

fn reveal(s: &mut Session, p: &Value) -> Result<Value> {
    let kind = str_p(p, "kind").unwrap_or("keyframes").to_string();
    let cid = super::comp_id(s, p)?;
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    let layers: Vec<LayerId> = if s.state.selected_layers.is_empty() { comp.layers.iter().map(|l| l.id).collect() } else { s.state.selected_layers.clone() };
    let mut out = vec![];
    for lid in &layers {
        let Some(l) = comp.layer(*lid) else { continue };
        let (w, h) = effectcraft_render::source_size(&s.project, l);
        let anchor = [w as f64 / 2.0, h as f64 / 2.0];
        let center = [comp.width as f64 / 2.0, comp.height as f64 / 2.0];
        // Properties at the top of the layer's tree (Time Remap): enabled time remapping is a
        // modification in itself, so it reveals even with no keys left on it (#990).
        for pr in l.props.props() {
            let hit = match kind.as_str() {
                "keyframes" => pr.is_animated() || pr.match_id == "timeRemap",
                "animation" => pr.is_animated() || pr.has_expression() || pr.match_id == "timeRemap",
                _ => pr.match_id == "timeRemap" || modified(pr, "", anchor, center),
            };
            if hit {
                out.push(json!({"layer": lid.0, "prop": pr.uid}));
            }
        }
        // Modified: a fresh layer of the same kind gives the defaults to compare against.
        let fresh = (kind == "modified").then(|| build::layer(&mut Project::default(), comp, "", l.source.clone(), (w, h), None));
        let camera_or_light = matches!(l.source, LayerSource::Camera | LayerSource::Light { .. });
        for top in l.props.groups() {
            let hit = |pr: &Property| match kind.as_str() {
                "keyframes" => pr.is_animated(),
                "animation" => pr.is_animated() || pr.has_expression(),
                // A camera's or light's Transform is compared with a fresh one's below.
                _ if camera_or_light && top.match_id == "transform" => pr.is_animated() || pr.expr.is_some(),
                _ => modified(pr, &top.match_id, anchor, center),
            };
            let mut found = vec![];
            top.walk("", &mut |_, pr| {
                if hit(pr) {
                    found.push(pr.uid);
                }
            });
            // Modified: masks count as modifications in full (AE reveals them).
            if kind == "modified" && top.match_id == "masks" {
                for g in top.groups() {
                    found.push(g.uid);
                }
            }
            // Modified: an effect's parameters that differ from a new instance's.
            if kind == "modified" && top.match_id == "effects" {
                let size = effectcraft_render::effect_bounds(&s.project, comp, l).0;
                for g in top.groups() {
                    let spec = match &g.kind {
                        GroupKind::Effect { effect } => effectcraft_effects::find(effect),
                        _ => None,
                    };
                    // A missing plug-in's defaults aren't known: the effect shows in full.
                    let Some(spec) = spec else {
                        found.push(g.uid);
                        continue;
                    };
                    let mut next = 0;
                    let new = effectcraft_effects::instantiate(spec, &mut Ids(&mut next), "", size);
                    effect_changes(g, &new, &mut found);
                }
            }
            if kind == "modified" && top.match_id == "text" {
                for g in top.group("animators").into_iter().flat_map(|a| a.groups()) {
                    found.push(g.uid);
                }
            }
            if let Some(fresh) = &fresh {
                if let Some(default) = default_group(top, l, fresh) {
                    changed_from(top, &default, &mut found);
                }
                // Every layer style counts in full, like an effect; so does each overridden
                // Essential Property.
                if top.match_id == effectcraft_project::styles::GROUP {
                    found.extend(top.groups().filter(|g| g.match_id != effectcraft_project::styles::BLENDING).map(|g| g.uid));
                }
                if let GroupKind::Essential { overridden } = &top.kind {
                    for u in overridden {
                        if !found.contains(u) && (top.find(*u).is_some() || top.find_group(*u).is_some()) {
                            found.push(*u);
                        }
                    }
                }
            }
            out.extend(found.into_iter().map(|u| json!({"layer": lid.0, "prop": u})));
        }
    }
    let layers: Vec<u64> = layers.iter().map(|l| l.0).collect();
    frontend(s, "timeline.revealProps", &json!({"props": out, "kind": kind, "layers": layers}))?;
    Ok(json!({"props": out}))
}

/// Is `g` an effect/mask group kind (used by tests).
#[allow(dead_code)]
pub(crate) fn is_indexed(g: &PropGroup) -> bool {
    matches!(g.kind, GroupKind::Indexed | GroupKind::Mask { .. } | GroupKind::Effect { .. })
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "anim.savePreset",
            "Save Animation Preset...",
            ["Animation"],
            None,
            "{path (.ecpreset), name?} — saves the selected properties/effects",
            has_selected_props,
            save_preset
        ),
        cmd!("anim.applyPreset", "Apply Animation Preset...", ["Animation"], None, "{path | preset, layers?}", has_layers, apply_preset),
        cmd!("anim.addKeyframe", "Add Keyframe", ["Animation"], None, "{} — keys the selected properties at the CTI", has_selected_props, add_keyframe),
        cmd!(
            "keys.exponentialScale",
            "Exponential Scale",
            ["Animation", "Keyframe Assistant"],
            None,
            "{} — two selected Scale keys",
            has_keys,
            exponential_scale
        ),
        cmd!("text.addSelector", "Add Text Selector", [], None, "{layer?, animator?: uid|index, kind: range|wiggly|expression}", has_text_layer, add_selector),
        cmd!("text.removeAllAnimators", "Remove All Text Animators", ["Animation"], None, "{layers?}", has_text_layer, remove_all_animators),
        cmd!("anim.reveal", "Reveal Properties", [], None, "{kind: keyframes|animation|modified}", has_comp, reveal),
    ]
}

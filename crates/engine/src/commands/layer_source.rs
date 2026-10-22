//! Source replacement: pixel-space geometry belongs to the source's dimensions.

use effectcraft_keyframe::Value as KV;
use effectcraft_project::{ItemId, ItemKind, Layer, LayerId, LayerSource, Node, Project, Property, build};
use serde_json::{Value, json};

use super::{bad, layers_p};
use crate::{EngineError, Result, Session};

fn replaceable(layer: &Layer) -> bool {
    matches!(layer.source, LayerSource::Footage { .. } | LayerSource::Comp { .. } | LayerSource::Solid { .. })
}

fn replacement_source(project: &Project, cid: ItemId, item: ItemId) -> Result<LayerSource> {
    let c = "layer.replaceSource";
    let source = match project.item(item).map(|i| &i.kind) {
        Some(ItemKind::Comp(_)) => LayerSource::Comp { item },
        Some(ItemKind::Footage(f)) if !matches!(f.kind, effectcraft_project::FootageKind::Data | effectcraft_project::FootageKind::Model) => {
            LayerSource::Footage { item }
        }
        Some(ItemKind::Solid(_)) => LayerSource::Solid { item },
        _ => return Err(bad(c, "the item cannot be used as an AV layer source")),
    };
    let mut todo = vec![item];
    let mut seen = std::collections::BTreeSet::new();
    while let Some(id) = todo.pop() {
        if id == cid {
            return Err(bad(c, "replacement would create a composition cycle"));
        }
        if !seen.insert(id) {
            continue;
        }
        if seen.len() > 4096 {
            return Err(bad(c, "source composition graph exceeds the safety limit"));
        }
        if let Some(comp) = project.comp(id) {
            for layer in &comp.layers {
                if let LayerSource::Comp { item } = layer.source
                    && !seen.contains(&item)
                    && !todo.contains(&item)
                {
                    if todo.len().saturating_add(seen.len()) >= 4096 {
                        return Err(bad(c, "source composition graph exceeds the safety limit"));
                    }
                    todo.push(item);
                }
            }
        }
    }
    Ok(source)
}

/// Shared read-only eligibility check for Timeline drag feedback and shortcut enablement.
/// Locked layers are ignored, matching the command; other invalid targets reject the batch.
pub fn can_replace_source(project: &Project, cid: ItemId, item: ItemId, ids: &[LayerId]) -> bool {
    let Some(comp) = project.comp(cid) else { return false };
    let mut any = false;
    for id in ids {
        let Some(layer) = comp.layer(*id) else { return false };
        if layer.switches.locked {
            continue;
        }
        if !replaceable(layer) {
            return false;
        }
        any = true;
    }
    any && replacement_source(project, cid, item).is_ok()
}

pub(super) fn enabled(s: &Session) -> std::result::Result<(), String> {
    let cid = s.active_comp_id().ok_or("no composition")?;
    let item = *s.state.project_selection.first().ok_or("select a source item in Project")?;
    if can_replace_source(&s.project, cid, item, &s.state.selected_layers) {
        Ok(())
    } else {
        Err("select an available AV layer and a valid replacement source".into())
    }
}

/// Replace footage while keeping source-relative anchor and mask geometry.
pub(super) fn replace_source(s: &mut Session, p: &Value) -> Result<Value> {
    let c = "layer.replaceSource";
    let (cid, ids) = layers_p(s, p)?;
    let ids = super::unlocked(s, cid, ids, c)?;
    if p.get("fixExpressions").and_then(Value::as_bool) == Some(true) {
        return Err(bad(c, "fixExpressions=true is not supported; pass false to preserve expression text"));
    }
    let item = match p.get("item") {
        Some(Value::Number(n)) => ItemId(n.as_u64().ok_or_else(|| bad(c, "item: an unsigned item ID or name"))?),
        Some(Value::String(name)) => s.project.find_by_name(name).map(|i| i.id).ok_or_else(|| bad(c, format!("no item `{name}`")))?,
        Some(_) => return Err(bad(c, "item: an unsigned item ID or name")),
        None => *s.state.project_selection.first().ok_or_else(|| bad(c, "select a source item in Project"))?,
    };
    let source = replacement_source(&s.project, cid, item)?;
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    for id in &ids {
        let layer = comp.layer(*id).ok_or_else(|| bad(c, "no such layer"))?;
        if !replaceable(layer) {
            return Err(bad(c, format!("“{}” has no AV source to replace", layer.name)));
        }
    }
    let dimensions = s.project.item(item).and_then(|i| i.dimensions()).unwrap_or((0, 0));
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    let factors: std::collections::BTreeMap<_, _> = comp
        .layers
        .iter()
        .filter(|l| ids.contains(&l.id))
        .map(|layer| {
            let old = effectcraft_render::source_size(&s.project, layer);
            // Audio-only items have no pixel coordinate space to remap.
            let ratio = |new: u32, old: u32| if old == 0 || new == 0 { 1.0 } else { f64::from(new) / f64::from(old) };
            (layer.id, [ratio(dimensions.0, old.0), ratio(dimensions.1, old.1)])
        })
        .collect();
    let needs_audio = build::source_has_audio_controls(&s.project, &source);
    let count = s.edit("Replace Layer Source", None, |project, _| {
        let mut next_id = project.next_id;
        let comp = project.comp_mut(cid).ok_or(EngineError::NoComp)?;
        let mut count = 0;
        for layer in comp.layers.iter_mut().filter(|l| ids.contains(&l.id)) {
            if matches!(layer.source, LayerSource::Footage { .. } | LayerSource::Comp { .. } | LayerSource::Solid { .. }) {
                if let Some(factors) = factors.get(&layer.id) {
                    remap_geometry(layer, *factors)?;
                }
                if needs_audio && layer.props.sub("audio").is_none() {
                    // The standard group allocates two IDs (group + Levels). Check before
                    // using the shared builder; any failure rolls back all target layers.
                    next_id.checked_add(2).ok_or_else(|| bad(c, "not enough property IDs for Audio controls"))?;
                    layer.props.children.push(build::audio(&mut build::Ids(&mut next_id)).into());
                }
                // Keep existing Audio controls even for a silent replacement: their UIDs,
                // animation, expressions and the layer's mute switch belong to the layer.
                layer.source = source.clone();
                count += 1;
            }
        }
        if count == 0 {
            return Err(bad(c, "select a footage, solid or composition layer"));
        }
        project.next_id = next_id;
        Ok(count)
    })?;
    Ok(json!({"layers":count,"item":item.0}))
}

fn multiply(value: &mut f64, factor: f64) -> Result<()> {
    let scaled = *value * factor;
    if !scaled.is_finite() {
        return Err(bad("layer.replaceSource", "source-relative geometry exceeds the finite coordinate range"));
    }
    *value = scaled;
    Ok(())
}

fn xy<const N: usize>(point: &mut [f64; N], factors: [f64; 2]) -> Result<()> {
    for (coordinate, factor) in point.iter_mut().zip(factors) {
        multiply(coordinate, factor)?;
    }
    Ok(())
}

fn anchor_value(value: &mut KV, factors: [f64; 2]) -> Result<()> {
    match value {
        KV::Vec2(v) => xy(v, factors),
        KV::Vec3(v) => xy(v, factors),
        _ => Ok(()),
    }
}

fn anchor(property: &mut Property, factors: [f64; 2]) -> Result<()> {
    anchor_value(&mut property.value, factors)?;
    for key in &mut property.keys {
        anchor_value(&mut key.value, factors)?;
        xy(&mut key.spatial_in, factors)?;
        xy(&mut key.spatial_out, factors)?;
        // AE 24.0.2 replacement scales spatial Anchor Point temporal speed by the
        // height ratio, including nonuniform replacements. Influences stay unchanged.
        for eases in [&mut key.in_ease, &mut key.out_ease] {
            for (dimension, ease) in eases.iter_mut().enumerate() {
                let factor = if property.spatial { factors[1] } else { factors.get(dimension).copied().unwrap_or(1.0) };
                multiply(&mut ease.speed, factor)?;
            }
        }
    }
    Ok(())
}

fn mask_value(value: &mut KV, factors: [f64; 2]) -> Result<()> {
    if let KV::Path(path) = value {
        for point in path.vertices.iter_mut().chain(&mut path.in_tangents).chain(&mut path.out_tangents) {
            xy(point, factors)?;
        }
    }
    Ok(())
}

fn remap_geometry(layer: &mut Layer, factors: [f64; 2]) -> Result<()> {
    if factors == [1.0; 2] {
        return Ok(());
    }
    if let Some(property) = layer.props.prop_mut("transform/anchor") {
        anchor(property, factors)?;
    }
    if let Some(masks) = layer.props.sub_mut("masks") {
        for node in &mut masks.children {
            if let Node::Group(mask) = node
                && let Some(path) = mask.get_mut("path")
            {
                mask_value(&mut path.value, factors)?;
                for key in &mut path.keys {
                    mask_value(&mut key.value, factors)?;
                }
            }
        }
    }
    // Position, Scale, effects, feather and expansion retain their existing values and keys.
    Ok(())
}

use super::*;

/// A layer whose source is an item (footage, precomposition or solid), as in After Effects'
/// Replace Source. Text, shape, null, camera, light and 3D model layers have no AVItem source to
/// replace.
fn replaceable_layer(l: &Layer) -> bool {
    matches!(l.source, LayerSource::Footage { .. } | LayerSource::Comp { .. } | LayerSource::Solid { .. })
}

/// The source a Project item gives a layer, or why it can't replace one.
fn replacement_source(item: ItemId, it: &effectcraft_project::Item) -> Result<LayerSource> {
    const CMD: &str = "layer.replaceSource";
    match &it.kind {
        ItemKind::Comp(_) => Ok(LayerSource::Comp { item }),
        ItemKind::Footage(f) if f.kind == FootageKind::Data => {
            Err(bad(CMD, "data files can't be layer sources; read them in expressions with footage(\"name\").sourceData"))
        }
        ItemKind::Footage(f) if f.kind == FootageKind::Model => Err(bad(CMD, "3D model items can't replace a 2D layer source; add them as 3D layers")),
        ItemKind::Footage(_) => Ok(LayerSource::Footage { item }),
        ItemKind::Solid(_) => Ok(LayerSource::Solid { item }),
        ItemKind::Folder => Err(bad(CMD, "folders can't be layer sources")),
    }
}

/// Whether a source brings audio properties (precompositions always can; footage when it has an
/// audio stream).
fn source_has_audio(project: &Project, source: &LayerSource) -> bool {
    match source {
        LayerSource::Comp { .. } => true,
        LayerSource::Footage { item } => project.item(*item).is_some_and(|i| matches!(&i.kind, ItemKind::Footage(f) if f.has_audio)),
        _ => false,
    }
}

pub(super) fn has_replace_source(s: &Session) -> std::result::Result<(), String> {
    has_comp(s)?;
    let c = s.active_comp().ok_or("no composition")?;
    if !s.state.selected_layers.iter().filter_map(|l| c.layer(*l)).any(replaceable_layer) {
        return Err("select a footage, composition or solid layer".into());
    }
    if s.state.project_selection.is_empty() {
        return Err("select a footage item, a composition or a solid in the Project panel".into());
    }
    Ok(())
}

/// Replace the source of the selected (or `layers`) AV layers with a Project item, as After
/// Effects' Alt-drag and Ctrl+Alt+/ do. The layer keeps every property: name, timing, transforms,
/// keyframes, expressions, effects, masks, switches, parenting and track matte. Only its source
/// changes; precompositions bring their Essential Properties group through `sync_project`, and a
/// source with audio adds an Audio group when the layer has none.
pub(super) fn replace_source(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "layer.replaceSource";
    let (cid, ids) = layers_p(s, p)?;
    let ids = unlocked(s, cid, ids, CMD)?;
    if ids.is_empty() {
        return Err(bad(CMD, "select a layer first"));
    }
    let item = match p.get("item") {
        Some(Value::Number(n)) => ItemId(n.as_u64().ok_or_else(|| bad(CMD, "item: a project item id"))?),
        Some(Value::String(name)) => s.project.find_by_name(name).map(|i| i.id).ok_or_else(|| bad(CMD, format!("no item `{name}`")))?,
        _ => *s.state.project_selection.first().ok_or_else(|| bad(CMD, "select a footage item, a composition or a solid in the Project panel"))?,
    };
    let it = s.project.item(item).ok_or_else(|| bad(CMD, format!("no item {}", item.0)))?;
    let source = replacement_source(item, it)?;
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    for lid in &ids {
        let l = comp.layer(*lid).ok_or_else(|| bad(CMD, "no such layer"))?;
        if !replaceable_layer(l) {
            return Err(bad(CMD, format!("“{}” has no source item to replace", l.name)));
        }
    }
    if let LayerSource::Comp { item: new } = &source
        && s.project.comp_contains(*new, cid)
    {
        return Err(bad(CMD, "a composition can't contain itself"));
    }
    let has_audio = source_has_audio(&s.project, &source);
    let source_for_edit = source.clone();
    s.edit("Replace Layer Source", None, |proj, _| {
        for lid in &ids {
            let needs_audio = has_audio && proj.comp(cid).and_then(|c| c.layer(*lid)).is_some_and(|l| l.props.sub("audio").is_none());
            let audio = if needs_audio {
                let mut next = Ids(&mut proj.next_id);
                Some(build::audio(&mut next))
            } else {
                None
            };
            let l = layer_mut(proj, cid, *lid)?;
            l.source = source_for_edit.clone();
            if let Some(audio) = audio {
                l.props.children.push(audio.into());
            }
        }
        Ok(())
    })?;
    Ok(json!({"layers": ids.iter().map(|l| l.0).collect::<Vec<_>>(), "item": item.0}))
}

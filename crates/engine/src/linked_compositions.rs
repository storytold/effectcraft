//! Sandbox linked compositions: isolated saved projects exposed as flat footage sources.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use effectcraft_project::{Footage, FootageKind, ItemId, ItemKind, LinkedComposition, Project};
use serde_json::{Value, json};

use crate::{EngineError, Result, Session};

const MAX_BYTES: u64 = 16 * 1024 * 1024;
const MAX_PROJECTS: usize = 8;
const MAX_ITEMS: usize = 4096;
const MAX_LAYERS: usize = 8192;

fn identity() -> Result<String> {
    Ok(effectcraft_project::try_new_identity()?)
}

/// Fill missing legacy identities and renew duplicated identities before authoring or saving.
pub(crate) fn identify(project: &mut Project) -> Result<()> {
    if project.project_id.is_empty() {
        project.project_id = identity()?;
    }
    let mut seen = BTreeSet::new();
    for item in project.items.values_mut() {
        if item.stable_id.is_empty() || !seen.insert(item.stable_id.clone()) {
            item.stable_id = identity()?;
            if !seen.insert(item.stable_id.clone()) {
                return Err(EngineError::Other("Cannot allocate a unique project item identity".into()));
            }
        }
    }
    Ok(())
}

pub(crate) fn has_links(project: &Project) -> bool {
    project.items.values().any(|i| matches!(&i.kind, ItemKind::Footage(f) if f.kind == FootageKind::LinkedComposition))
}

fn read_source(s: &Session, path: &str) -> std::result::Result<Arc<Project>, String> {
    let bytes = s.services.read_file_bounded(path, MAX_BYTES).map_err(|e| format!("Cannot read linked source {path}: {e}"))?;
    let text = String::from_utf8(bytes).map_err(|_| "Linked source is not a UTF-8 project".to_string())?;
    let text = if crate::xml_project::is_xml(&text) { crate::xml_project::from_xml(&text)? } else { text };
    let mut source = Project::from_json(&text).map_err(|e| e.to_string())?;
    if source.project_id.is_empty() || source.comps().any(|(id, _)| source.item(*id).is_none_or(|i| i.stable_id.is_empty())) {
        return Err("Save the source project with this build before linking its compositions".into());
    }
    let mut item_ids = BTreeSet::new();
    for item in source.items.values() {
        if item.stable_id.is_empty() || !item_ids.insert(item.stable_id.clone()) {
            return Err("Linked source contains missing or duplicate item identities; save it with this build before linking".into());
        }
    }
    if source.items.len() > MAX_ITEMS || source.comps().map(|(_, c)| c.layers.len()).sum::<usize>() > MAX_LAYERS {
        return Err("Linked source exceeds the sandbox item/layer budget".into());
    }
    if has_links(&source) {
        return Err(
            "The project you selected contains linked compositions from another project. Nested linked compositions are not supported at this time.".into()
        );
    }
    if source.project_id == s.project.project_id && !s.project.project_id.is_empty() {
        return Err("A project cannot link to itself".into());
    }
    let refused = crate::doc_path::resolve_footage(&mut source, Some(path));
    if let Some((_, path, why)) = refused.first() {
        return Err(crate::doc_path::refusal(path, *why));
    }
    for item in source.items.values() {
        let original = if let ItemKind::Footage(f) = &item.kind { Some(f) } else { None };
        for f in original.into_iter().chain(item.proxy.as_ref().filter(|p| p.enabled).map(|p| &p.footage)) {
            if f.kind == FootageKind::Data && f.data.is_some() {
                continue;
            }
            if std::iter::once(&f.path).chain(&f.sequence).any(|p| !p.is_empty() && !s.services.exists(p)) {
                return Err(format!("Missing source footage: {}", item.name));
            }
        }
    }
    let _ = crate::upgrade_effects(&mut source);
    Ok(Arc::new(source))
}

fn resolved_path(s: &Session, stored: &str) -> std::result::Result<String, String> {
    crate::doc_path::resolve(stored, s.path.as_deref()).map(|p| p.to_string_lossy().into_owned()).map_err(|e| e.to_string())
}

/// Group the lexical paths the host is allowed to resolve, without bypassing its filesystem
/// policy with a canonicalization read. Windows links differing only in slash style or case
/// refer to the same source; runtime paths retain their original spelling for display and I/O.
fn source_key(s: &Session, stored: &str) -> String {
    let path = resolved_path(s, stored).unwrap_or_else(|_| stored.to_string());
    #[cfg(windows)]
    {
        path.replace('\\', "/").to_lowercase()
    }
    #[cfg(not(windows))]
    {
        path
    }
}

fn describe(id: ItemId, f: &Footage) -> Value {
    let Some(link) = &f.linked_comp else { return json!({"item": id.0, "error": "Missing link record"}) };
    json!({"item": id.0, "path": f.path, "sourceProjectId": link.source_project_id,
        "compositionId": link.composition_id, "name": link.last_known_name,
        "sourceColor": link.source_color, "effectiveColor": f.linked_color_space(), "colorProfileOverride": f.color_profile,
        "preserveRgb": f.preserve_rgb,
        "generation": link.generation, "ready": link.resolved.is_some() && link.error.is_none(), "error": link.error})
}

pub(crate) fn list(s: &Session) -> Value {
    json!(
        s.project
            .items
            .values()
            .filter_map(|i| match &i.kind {
                ItemKind::Footage(f) if f.kind == FootageKind::LinkedComposition => Some(describe(i.id, f)),
                _ => None,
            })
            .collect::<Vec<_>>()
    )
}

fn source_path(s: &Session, path: &str) -> Result<String> {
    let full = s.services.full_path(path).ok_or_else(|| EngineError::Other("Cannot resolve source path".into()))?;
    crate::doc_path::resolve(&full, s.path.as_deref()).map(|path| path.to_string_lossy().into_owned()).map_err(|e| EngineError::Other(e.to_string()))
}

fn linked_items(s: &Session, source_project: &str, composition: &str) -> Vec<ItemId> {
    s.project
        .items
        .values()
        .filter_map(|item| match &item.kind {
            ItemKind::Footage(f) if f.kind == FootageKind::LinkedComposition => {
                f.linked_comp.as_ref().filter(|link| link.source_project_id == source_project && link.composition_id == composition).map(|_| item.id)
            }
            _ => None,
        })
        .collect()
}

/// Inspect the saved source without changing the destination or resolving its existing links.
/// Names are display metadata; the picker carries stable identities back to the link command.
pub(crate) fn linkable_compositions(s: &Session, path: &str) -> Result<Value> {
    let path = source_path(s, path)?;
    let source = read_source(s, &path).map_err(EngineError::Other)?;
    let compositions: Vec<_> = source
        .items
        .values()
        .filter_map(|item| {
            let comp = item.as_comp()?;
            let linked = linked_items(s, &source.project_id, &item.stable_id);
            Some(json!({
                "id": item.stable_id, "name": item.name, "width": comp.width, "height": comp.height,
                "frameRate": comp.frame_rate.as_f64(), "duration": comp.duration.seconds(),
                "alreadyLinked": !linked.is_empty(), "linkedItems": linked.iter().map(|id| id.0).collect::<Vec<_>>()
            }))
        })
        .collect();
    Ok(json!({"path": path, "sourceProjectId": source.project_id, "compositions": compositions}))
}

/// Reload selected linked sources once per project, including unselected sibling links.
pub(crate) fn reload_items(s: &mut Session, items: &[ItemId]) -> Result<usize> {
    let mut sources = BTreeMap::new();
    for id in items {
        if let Some(ItemKind::Footage(f)) = s.project.item(*id).map(|i| &i.kind)
            && f.kind == FootageKind::LinkedComposition
        {
            sources.entry(source_key(s, &f.path)).or_insert(*id);
        }
    }
    let affected: Vec<_> = s
        .project
        .items
        .values()
        .filter_map(|i| match &i.kind {
            ItemKind::Footage(f) if f.kind == FootageKind::LinkedComposition && sources.contains_key(&source_key(s, &f.path)) => Some(i.id),
            _ => None,
        })
        .collect();
    for id in sources.into_values() {
        refresh(s, Some(id))?;
    }
    for id in &affected {
        if let Some(item) = s.project.item(*id)
            && let Some(why) = crate::render::missing_item_problem(item)
        {
            return Err(EngineError::Other(format!("Could not reload linked composition {}: {why}", item.name)));
        }
    }
    Ok(affected.len())
}

/// Refresh one source project (every linked comp using it), or all sources. External disk
/// state does not create an undo step or change destination layer properties.
pub(crate) fn refresh(s: &mut Session, item: Option<ItemId>) -> Result<Value> {
    refresh_with_source(s, item, None)
}

fn refresh_with_source(s: &mut Session, item: Option<ItemId>, loaded_source: Option<(&str, Arc<Project>)>) -> Result<Value> {
    let selected_path = match item {
        Some(id) => match s.project.item(id).map(|i| &i.kind) {
            Some(ItemKind::Footage(f)) if f.kind == FootageKind::LinkedComposition => Some(source_key(s, &f.path)),
            _ => return Err(EngineError::Other("Select a linked composition".into())),
        },
        None => None,
    };
    let links: Vec<_> = s
        .project
        .items
        .values()
        .filter_map(|i| match &i.kind {
            ItemKind::Footage(f) if f.kind == FootageKind::LinkedComposition && selected_path.as_ref().is_none_or(|p| *p == source_key(s, &f.path)) => {
                Some((i.id, f.clone()))
            }
            _ => None,
        })
        .collect();
    if links.len() > 64 {
        return Err(EngineError::Other("The sandbox supports at most 64 linked compositions".into()));
    }
    let mut sources: BTreeMap<String, std::result::Result<Arc<Project>, String>> = BTreeMap::new();
    if let Some((path, source)) = loaded_source {
        sources.insert(source_key(s, path), Ok(source));
    }
    let mut updates = Vec::new();
    let mut forget = BTreeSet::new();
    let mut generations: BTreeMap<String, u64> = BTreeMap::new();
    for (_, f) in &links {
        let next = f.linked_comp.as_ref().map_or(1, |l| l.generation.saturating_add(1));
        let generation = generations.entry(source_key(s, &f.path)).or_insert(next);
        *generation = (*generation).max(next);
    }
    for (id, mut f) in links {
        if let Some(p) = f.linked_comp.as_ref().and_then(|l| l.resolved.as_ref()) {
            collect_paths(p, &mut forget);
        }
        let loaded = resolved_path(s, &f.path).and_then(|path| {
            let key = source_key(s, &path);
            if !sources.contains_key(&key) {
                if sources.len() >= MAX_PROJECTS {
                    return Err("The sandbox supports at most eight source projects".into());
                }
                sources.insert(key.clone(), read_source(s, &path));
            }
            f.path = path;
            sources.get(&key).cloned().ok_or_else(|| "Source resolution failed".to_string())?
        });
        let Some(link) = &mut f.linked_comp else {
            f.missing = true;
            updates.push((id, f));
            continue;
        };
        let loaded = loaded.and_then(|project| {
            if project.project_id != link.source_project_id {
                return Err("The file is a different source project".into());
            }
            let matches: Vec<_> = project.items.values().filter(|i| i.stable_id == link.composition_id && i.as_comp().is_some()).map(|i| i.id).collect();
            let [comp] = matches.as_slice() else { return Err(format!("Source composition is missing or has ambiguous identity: {}", link.last_known_name)) };
            validate_comp(&project, *comp)?;
            Ok((project, *comp))
        });
        match loaded {
            Ok((project, comp)) => {
                if let Some(c) = project.comp(comp) {
                    f.width = c.width;
                    f.height = c.height;
                    f.duration = c.duration;
                    f.frame_rate = c.frame_rate;
                    f.pixel_aspect = c.pixel_aspect;
                }
                link.last_known_name = project.item(comp).map(|i| i.name.clone()).unwrap_or_default();
                link.source_color = Some(project.settings.linked_color_space());
                collect_paths(&project, &mut forget);
                link.resolved = Some(project);
                link.last_known_source = None;
                link.resolved_comp = Some(comp);
                link.error = None;
                link.generation = generations.get(&source_key(s, &f.path)).copied().unwrap_or_else(|| link.generation.saturating_add(1));
                f.missing = false;
            }
            Err(error) => {
                if let Some(source) = link.resolved.take() {
                    link.last_known_source = Some(source);
                }
                link.resolved_comp = None;
                link.error = Some(error);
                f.missing = true;
            }
        }
        updates.push((id, f));
    }
    if !updates.is_empty() {
        let dirty = s.is_dirty();
        let project = Arc::make_mut(&mut s.project);
        for (id, f) in updates {
            if let Some(i) = project.items.get_mut(&id) {
                if let Some(link) = &f.linked_comp {
                    i.name.clone_from(&link.last_known_name);
                }
                i.kind = ItemKind::Footage(f);
            }
        }
        for source in sources.values().filter_map(|r| r.as_ref().ok()) {
            s.stored_paths.add(source, None);
        }
        crate::footage_reload::forget_media(s, &forget);
        if let Some(dc) = &s.disk_cache {
            dc.drop_files(forget.iter().map(String::as_str));
        } else {
            s.pending_disk_drops.extend(forget);
        }
        s.bump();
        if !dirty {
            s.mark_saved();
        }
    }
    let result = list(s);
    let errors: Vec<_> = result.as_array().into_iter().flatten().filter(|v| v["ready"] != true).cloned().collect();
    if !errors.is_empty() {
        s.toast(format!("{} linked composition(s) are offline; inspect project.linkedCompositions", errors.len()));
    }
    Ok(json!({"links": result, "errors": errors}))
}

fn collect_paths(p: &Project, paths: &mut BTreeSet<String>) {
    for i in p.items.values() {
        let f = if let ItemKind::Footage(f) = &i.kind { Some(f) } else { None };
        for f in f.into_iter().chain(i.proxy.as_ref().map(|p| &p.footage)) {
            if !f.path.is_empty() {
                paths.insert(f.path.clone());
            }
            paths.extend(f.sequence.iter().cloned());
        }
    }
}

pub(crate) fn validate_comp(p: &Project, root: ItemId) -> std::result::Result<(), String> {
    // Cache the height of each finished subtree, not merely whether it was visited. A shared
    // tail that fitted a shallow route may exceed the limit when reached by a deeper route.
    fn height(
        p: &Project,
        id: ItemId,
        depth: usize,
        active: &mut BTreeSet<ItemId>,
        heights: &mut BTreeMap<ItemId, usize>,
    ) -> std::result::Result<usize, String> {
        const LIMIT: usize = 12;
        if active.contains(&id) {
            return Err("Composition cycle in linked source".into());
        }
        if depth > LIMIT {
            return Err("Linked source exceeds the sandbox composition-depth limit".into());
        }
        if let Some(height) = heights.get(&id) {
            if *height > LIMIT - depth + 1 {
                return Err("Linked source exceeds the sandbox composition-depth limit".into());
            }
            return Ok(*height);
        }
        let c = p.comp(id).ok_or_else(|| "Missing composition dependency in linked source".to_string())?;
        if u64::from(c.width) * u64::from(c.height) > 16_777_216 {
            return Err("Linked source exceeds the sandbox 16-megapixel composition limit".into());
        }
        active.insert(id);
        let mut result = 1;
        for layer in &c.layers {
            if let effectcraft_project::LayerSource::Comp { item } = layer.source {
                result = result.max(1 + height(p, item, depth + 1, active, heights)?);
            }
        }
        active.remove(&id);
        heights.insert(id, result);
        Ok(result)
    }

    height(p, root, 1, &mut BTreeSet::new(), &mut BTreeMap::new()).map(|_| ())
}

pub(crate) fn link(s: &mut Session, path: &str, comps: &[Value], expected_project: Option<&str>, reuse_existing: bool) -> Result<Value> {
    if comps.is_empty() || comps.len() > 64 {
        return Err(EngineError::Other("Select between one and 64 source compositions".into()));
    }
    let path = source_path(s, path)?;
    let source = read_source(s, &path).map_err(EngineError::Other)?;
    if expected_project.is_some_and(|expected| expected != source.project_id) {
        return Err(EngineError::Other("The file is a different source project; choose the source project again".into()));
    }
    let mut selected = Vec::new();
    let mut seen = BTreeSet::new();
    for comp in comps {
        let item = source
            .items
            .values()
            .find(|item| item.as_comp().is_some() && (comp.as_str() == Some(item.stable_id.as_str()) || (!reuse_existing && comp.as_u64() == Some(item.id.0))))
            .or_else(|| {
                // The picker carries exact stable IDs. A deleted ID must never resolve to a
                // different composition that happens to have that ID as its displayed name.
                if reuse_existing { None } else { source.items.values().find(|item| item.as_comp().is_some() && comp.as_str() == Some(item.name.as_str())) }
            })
            .ok_or_else(|| {
                EngineError::Other(
                    if reuse_existing {
                        "A selected source composition is missing; choose the source project again"
                    } else {
                        "Source composition not found; pass its name, item ID, or stable ID"
                    }
                    .into(),
                )
            })?;
        if seen.insert(item.id) {
            validate_comp(&source, item.id).map_err(EngineError::Other)?;
            let existing = reuse_existing.then(|| linked_items(s, &source.project_id, &item.stable_id)).and_then(|items| items.first().copied());
            selected.push((item, existing));
        }
    }
    let additions = selected.iter().filter(|(_, existing)| existing.is_none()).count();
    let existing: Vec<_> = s
        .project
        .items
        .values()
        .filter_map(|i| match &i.kind {
            ItemKind::Footage(f) if f.kind == FootageKind::LinkedComposition => Some(f),
            _ => None,
        })
        .collect();
    if existing.len().saturating_add(additions) > 64 {
        return Err(EngineError::Other("The sandbox supports at most 64 linked compositions".into()));
    }
    let paths: BTreeSet<_> = existing.iter().map(|f| source_key(s, &f.path)).collect();
    if additions > 0 && !paths.contains(&source_key(s, &path)) && paths.len() >= MAX_PROJECTS {
        return Err(EngineError::Other("The sandbox supports at most eight source projects".into()));
    }
    let make_footage = |selected: &effectcraft_project::Item| -> Result<Footage> {
        let c = selected.as_comp().ok_or(EngineError::NoComp)?;
        Ok(Footage {
            path: path.clone(),
            kind: FootageKind::LinkedComposition,
            width: c.width,
            height: c.height,
            pixel_aspect: c.pixel_aspect,
            frame_rate: c.frame_rate,
            duration: c.duration,
            has_video: true,
            has_audio: true,
            linked_comp: Some(LinkedComposition {
                source_project_id: source.project_id.clone(),
                composition_id: selected.stable_id.clone(),
                last_known_name: selected.name.clone(),
                source_color: Some(source.settings.linked_color_space()),
                resolved: Some(source.clone()),
                last_known_source: None,
                resolved_comp: Some(selected.id),
                error: None,
                generation: 1,
            }),
            ..Default::default()
        })
    };
    // Build every source item before starting the single undoable edit. A stale selection,
    // changed project identity or resource-limit error cannot leave a partly linked project.
    let prepared = selected.iter().map(|(item, existing)| Ok((item.name.clone(), *existing, make_footage(item)?))).collect::<Result<Vec<_>>>()?;
    let label = if selected.len() == 1 { "Link Composition" } else { "Link Compositions" };
    let (items, created, reused) = s.edit(label, None, |p, st| {
        let mut items = Vec::new();
        let mut created = Vec::new();
        let mut reused = Vec::new();
        for (name, existing, footage) in prepared {
            let id = if let Some(id) = existing {
                reused.push(id);
                id
            } else {
                let id = p.add_item(&name, effectcraft_color::Label::Brown, None, ItemKind::Footage(footage));
                created.push(id);
                id
            };
            items.push(id);
        }
        st.project_selection.clone_from(&items);
        Ok((items, created, reused))
    })?;
    if let Some(id) = created.first() {
        // Reuse the exact validated snapshot, rather than reading a potentially replaced file
        // again after mutation. Existing sibling links get the same saved source generation.
        refresh_with_source(s, Some(*id), Some((&path, source.clone())))?;
    }
    Ok(json!({
        "item": if items.len() == 1 { items.first().map(|id| id.0) } else { None },
        "items": items.iter().map(|id| id.0).collect::<Vec<_>>(),
        "createdItems": created.iter().map(|id| id.0).collect::<Vec<_>>(),
        "existingItems": reused.iter().map(|id| id.0).collect::<Vec<_>>(), "links": list(s)
    }))
}

/// Error before a fallible render/export, rather than successful transparent output.
pub(crate) fn render_problem(p: &Project, comp: ItemId) -> Option<String> {
    let reach = p.pixel_reach(comp, &p.heard_layers());
    // Pixel reach omits audio-only layers; include their sources for export preflight.
    let mut sources = BTreeSet::new();
    let mut stack = vec![comp];
    while let Some(id) = stack.pop() {
        if !sources.insert(id) {
            continue;
        }
        if let Some(c) = p.comp(id) {
            stack.extend(c.layers.iter().filter_map(|l| l.source.item()));
        }
    }
    for i in p.items.values() {
        if let ItemKind::Footage(f) = &i.kind
            && f.kind == FootageKind::LinkedComposition
            && (sources.contains(&i.id) || reach.items.as_ref().is_none_or(|items| items.contains(&i.id)))
        {
            let Some(l) = &f.linked_comp else { return Some(format!("{}: missing link record", i.name)) };
            if let Some(e) = &l.error {
                return Some(format!("{}: {e}", i.name));
            }
            if l.resolved.is_none() || l.resolved_comp.is_none() || f.missing {
                return Some(format!("{}: linked composition is offline; refresh it", i.name));
            }
        }
    }
    None
}

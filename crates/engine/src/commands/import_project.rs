//! File ▸ Import ▸ EffectCraft Project: merge another `.ecproj` (or its XML copy) into the
//! current project, as After Effects imports an `.aep` into the open project. The imported
//! items land in a folder named after the file; every id is shifted past the current project's
//! so nothing collides, and the whole import is one undo step.

use effectcraft_color::Label;
use effectcraft_project::{ItemId, ItemKind, LayerSource, Project, essential};
use serde_json::{Value, json};

use super::{CommandSpec, always, bad, str_p};
use crate::{EngineError, Result, Session, cmd};

const C: &str = "file.importProject";

/// Whether `path` names a project EffectCraft imports as a project (not as footage).
pub(crate) fn is_project_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.ends_with(".ecproj") || lower.ends_with(".ecprojx")
}

/// Read a project file (JSON or its XML copy), as File ▸ Open does.
fn read_project(s: &Session, path: &str) -> Result<(Project, Option<String>)> {
    let bytes = s.services.read_file(path).map_err(|e| EngineError::Other(format!("cannot read {path}: {e}")))?;
    let text = String::from_utf8(bytes).map_err(|_| bad(C, format!("{path} is not a text project file")))?;
    let text = if crate::xml_project::is_xml(&text) { crate::xml_project::from_xml(&text).map_err(EngineError::Other)? } else { text };
    let project = Project::from_json(&text).map_err(|e| bad(C, format!("{path} is not a project EffectCraft can open: {e}")))?;
    Ok((project, effectcraft_project::saved_by(&text)))
}

/// Footage paths relative to the imported project's folder become absolute when the file is
/// there; returns the footage paths that are missing.
fn resolve_footage(s: &Session, project: &mut Project, dir: &std::path::Path) -> Vec<String> {
    let services = s.services.clone();
    let resolve = |raw: &mut String| -> bool {
        if raw.is_empty() || services.exists(raw) {
            return true;
        }
        if std::path::Path::new(raw.as_str()).is_absolute() {
            return false;
        }
        let joined = dir.join(raw.as_str()).to_string_lossy().to_string();
        if services.exists(&joined) {
            *raw = joined;
            true
        } else {
            false
        }
    };
    let mut missing = vec![];
    for it in project.items.values_mut() {
        if let ItemKind::Footage(f) = &mut it.kind {
            if f.sequence.is_empty() {
                if !resolve(&mut f.path) {
                    missing.push(f.path.clone());
                }
            } else {
                let mut all = true;
                for frame in &mut f.sequence {
                    all &= resolve(frame);
                }
                if !all {
                    missing.push(f.path.clone());
                }
            }
        }
    }
    missing
}

/// The comps no other comp of `project` nests: the ones to open or place.
fn root_comps(project: &Project) -> Vec<ItemId> {
    let nested: std::collections::HashSet<ItemId> =
        project.comps().flat_map(|(_, c)| c.layers.iter().filter_map(|l| if let LayerSource::Comp { item } = l.source { Some(item) } else { None })).collect();
    project.comps().map(|(id, _)| *id).filter(|id| !nested.contains(id)).collect()
}

fn import_project(s: &mut Session, p: &Value) -> Result<Value> {
    let path = str_p(p, "path").ok_or_else(|| bad(C, "missing `path` (.ecproj)"))?.to_string();
    let (mut sub, saved_by) = read_project(s, &path)?;
    crate::upgrade_effects(&mut sub);
    let file = std::path::Path::new(&path);
    let dir = file.parent().map(|d| d.to_path_buf()).unwrap_or_default();
    let missing = resolve_footage(s, &mut sub, &dir);
    // The imported project's own render queue stays behind (as for templates).
    let base = s.project.next_id;
    if sub.next_id.checked_add(base).is_none() {
        return Err(bad(C, format!("{path} has ids out of range")));
    }
    essential::offset_ids(&mut sub, base);
    let roots: Vec<ItemId> = root_comps(&sub);
    let folder_name = file.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "Imported Project".into());
    let (folder, items) = s.edit("Import Project", None, |proj, st| {
        let folder = proj.add_item(&folder_name, Label::Yellow, None, ItemKind::Folder);
        let mut items = vec![];
        for (id, mut it) in std::mem::take(&mut sub.items) {
            if it.parent.is_none() {
                it.parent = Some(folder);
            }
            items.push(id.0);
            proj.items.insert(id, it);
        }
        proj.next_id = proj.next_id.max(sub.next_id);
        proj.fix_next_id();
        st.project_selection = vec![folder];
        Ok((folder, items))
    })?;
    let comps: Vec<Value> = s
        .project
        .comps()
        .filter(|(id, _)| items.contains(&id.0))
        .map(|(id, _)| json!({"id": id.0, "name": s.project.item(*id).map(|i| i.name.clone()).unwrap_or_default()}))
        .collect();
    let primary = if let [one] = roots.as_slice() { Some(one.0) } else { None };
    let mut warnings = vec![];
    if let Some(v) = saved_by.as_deref().filter(|v| effectcraft_project::is_newer_version(v, effectcraft_project::APP_VERSION)) {
        warnings.push(format!("saved by EffectCraft {v}, newer than this version ({})", effectcraft_project::APP_VERSION));
    }
    if !missing.is_empty() {
        warnings.push(format!("{} footage file{} missing", missing.len(), if missing.len() == 1 { " is" } else { "s are" }));
    }
    s.toast(if warnings.is_empty() { format!("Imported {folder_name}") } else { format!("Imported {folder_name}: {}", warnings.join("; ")) });
    Ok(json!({
        "folder": folder.0,
        "items": items,
        "comps": comps,
        "rootComps": roots.iter().map(|id| id.0).collect::<Vec<_>>(),
        "primaryComp": primary,
        "missingMedia": missing,
        "warnings": warnings,
    }))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![cmd!(
        "file.importProject",
        "EffectCraft Project...",
        ["File", "Import"],
        None,
        "{path (.ecproj or .ecprojx)} — merges the project into a folder named after the file; returns {folder, items, comps, rootComps, primaryComp (the only root comp, else null), missingMedia, warnings}",
        always,
        import_project
    )]
}

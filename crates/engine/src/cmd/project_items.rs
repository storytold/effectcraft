//! Project panel item edits: select, rename, move into / out of folders, label and comment.
//! Every edit is undoable.

use effectcraft_project::{ItemId, Project};
use serde_json::{Value, json};

use super::{CommandSpec, always, b_p, bad, str_p};
use crate::{EngineError, Result, Session, cmd, query};

/// `item` (id or name) → id.
fn item_ref(s: &Session, v: &Value, cmd: &str) -> Result<ItemId> {
    match v {
        Value::Number(n) => n.as_u64().map(ItemId).filter(|i| s.project.item(*i).is_some()),
        Value::String(name) => s.project.find_by_name(name).map(|i| i.id),
        _ => None,
    }
    .ok_or_else(|| bad(cmd, format!("no project item {v}")))
}

/// `items` (array) / `item`, else the Project panel selection.
fn items_p(s: &Session, p: &Value, cmd: &str) -> Result<Vec<ItemId>> {
    if let Some(Value::Array(a)) = p.get("items") {
        return a.iter().map(|v| item_ref(s, v, cmd)).collect();
    }
    if let Some(v) = p.get("item") {
        return Ok(vec![item_ref(s, v, cmd)?]);
    }
    if s.state.project_selection.is_empty() {
        return Err(bad(cmd, "no `items` given and nothing selected in the Project panel"));
    }
    Ok(s.state.project_selection.clone())
}

/// Whether `folder` is `item` or inside it.
pub fn is_within(project: &Project, folder: ItemId, item: ItemId) -> bool {
    let mut cur = Some(folder);
    let mut guard = 0;
    while let Some(c) = cur {
        if c == item {
            return true;
        }
        cur = project.item(c).and_then(|i| i.parent);
        guard += 1;
        if guard > 256 {
            return true;
        }
    }
    false
}

fn select(s: &mut Session, p: &Value) -> Result<Value> {
    let ids = match p.get("items") {
        Some(Value::Array(a)) => a.iter().map(|v| item_ref(s, v, "project.select")).collect::<Result<Vec<_>>>()?,
        _ => vec![],
    };
    if b_p(p, "add").unwrap_or(false) {
        for i in ids {
            if !s.state.project_selection.contains(&i) {
                s.state.project_selection.push(i);
            }
        }
    } else {
        s.state.project_selection = ids;
    }
    Ok(json!(s.state.project_selection.iter().map(|i| i.0).collect::<Vec<_>>()))
}

fn rename(s: &mut Session, p: &Value) -> Result<Value> {
    let c = "project.rename";
    let id =
        item_ref(s, p.get("item").unwrap_or(&Value::Null), c).or_else(|_| s.state.project_selection.first().copied().ok_or_else(|| bad(c, "no `item`")))?;
    let name = str_p(p, "name").map(str::trim).filter(|n| !n.is_empty()).ok_or_else(|| bad(c, "missing `name`"))?.to_string();
    s.edit("Rename", None, |proj, _| {
        proj.item_mut(id).ok_or_else(|| bad(c, "item vanished"))?.name = name;
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Move items into `folder` (id/name; `null` = the project root).
fn move_items(s: &mut Session, p: &Value) -> Result<Value> {
    let c = "project.move";
    let ids = items_p(s, p, c)?;
    let folder = match p.get("folder") {
        None | Some(Value::Null) => None,
        Some(v) => {
            let f = item_ref(s, v, c)?;
            if !s.project.item(f).is_some_and(|i| i.is_folder()) {
                return Err(bad(c, format!("{v} is not a folder")));
            }
            Some(f)
        }
    };
    if let Some(f) = folder
        && let Some(bad_id) = ids.iter().find(|i| is_within(&s.project, f, **i))
    {
        return Err(bad(c, format!("can't move folder {} into itself", bad_id.0)));
    }
    s.edit("Move Items", None, |proj, _| {
        for i in &ids {
            if let Some(it) = proj.item_mut(*i) {
                it.parent = folder;
            }
        }
        Ok(())
    })?;
    Ok(json!({"moved": ids.len()}))
}

fn set_label(s: &mut Session, p: &Value) -> Result<Value> {
    let c = "project.setLabel";
    let ids = items_p(s, p, c)?;
    let lab = match p.get("label") {
        Some(Value::Number(n)) => effectcraft_color::Label::ALL.get(n.as_u64().unwrap_or(0) as usize).copied(),
        Some(Value::String(name)) => s.prefs.label_from_name(name),
        _ => None,
    }
    .ok_or_else(|| bad(c, "missing or unknown `label` (name or index 0-16)"))?;
    s.edit("Label", None, |proj, _| {
        for i in &ids {
            if let Some(it) = proj.item_mut(*i) {
                it.label = lab;
            }
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

fn set_comment(s: &mut Session, p: &Value) -> Result<Value> {
    let c = "project.setComment";
    let ids = items_p(s, p, c)?;
    let text = str_p(p, "comment").ok_or_else(|| bad(c, "missing `comment`"))?.to_string();
    s.edit("Comment", None, |proj, _| {
        for i in &ids {
            proj.item_mut(*i).ok_or(EngineError::Other("item vanished".into()))?.comment = text.clone();
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

/// The items deleting `ids` removes: them and, for folders, everything inside.
fn doomed_items(project: &Project, ids: &[ItemId]) -> Vec<ItemId> {
    project.items.keys().copied().filter(|id| ids.iter().any(|d| is_within(project, *id, *d))).collect()
}

/// What deleting `items` takes with it: (layers using them, in how many comps), counting only
/// the comps that stay.
fn usage_of(project: &Project, items: &[ItemId]) -> (usize, usize) {
    let mut layers = 0;
    let mut comps = 0;
    for (_, comp) in project.comps().filter(|(cid, _)| !items.contains(cid)) {
        let n = comp.layers.iter().filter(|l| l.source.item().is_some_and(|i| items.contains(&i))).count();
        layers += n;
        comps += usize::from(n > 0);
    }
    (layers, comps)
}

/// What deleting the items (default: selected) would delete: the items (with folder contents)
/// and the layers in other compositions that use them. The Project panel asks before deleting
/// items in use, as After Effects does.
fn usage(s: &mut Session, p: &Value) -> Result<Value> {
    let ids = items_p(s, p, "project.usage")?;
    let doomed = doomed_items(&s.project, &ids);
    let (layers, comps) = usage_of(&s.project, &doomed);
    Ok(json!({"items": doomed.len(), "layers": layers, "comps": comps}))
}

/// How many times an item (default: the first selected) is used: the layers, in every
/// composition, whose source it is, as the Project panel's "used N times", and the compositions
/// that use it (its ▾ menu opens one), ordered by name.
fn used_in(s: &mut Session, p: &Value) -> Result<Value> {
    let c = "project.usedIn";
    let id = match p.get("item") {
        Some(v) => item_ref(s, v, c)?,
        None => *s.state.project_selection.first().ok_or_else(|| bad(c, "no `item` given and nothing selected in the Project panel"))?,
    };
    let users = s.project.item_users(id);
    let count = users.iter().fold(0usize, |n, (_, k)| n.saturating_add(*k));
    let comps: Vec<Value> =
        users.iter().map(|(cid, n)| json!({"id": cid.0, "name": s.project.item(*cid).map(|i| i.name.as_str()).unwrap_or_default(), "layers": n})).collect();
    Ok(json!({"item": id.0, "count": count, "comps": comps}))
}

/// Delete project items (folders with their contents). Layers that use a deleted item and Render
/// Queue items of deleted comps go too, like After Effects' Edit ▸ Clear in the Project panel.
fn delete(s: &mut Session, p: &Value) -> Result<Value> {
    let c = "project.delete";
    let ids = items_p(s, p, c)?;
    let doomed = doomed_items(&s.project, &ids);
    let n = doomed.len();
    s.edit("Delete Items", None, |proj, st| {
        for id in &doomed {
            proj.items.remove(id);
        }
        let comp_ids: Vec<ItemId> = proj.comps().map(|(id, _)| *id).collect();
        for cid in comp_ids {
            let uses = proj.comp(cid).is_some_and(|c| c.layers.iter().any(|l| l.source.item().is_some_and(|i| doomed.contains(&i))));
            if !uses {
                continue;
            }
            if let Some(comp) = proj.comp_mut(cid) {
                let gone: Vec<effectcraft_project::LayerId> =
                    comp.layers.iter().filter(|l| l.source.item().is_some_and(|i| doomed.contains(&i))).map(|l| l.id).collect();
                comp.layers.retain(|l| !gone.contains(&l.id));
                for l in &mut comp.layers {
                    if l.parent.is_some_and(|x| gone.contains(&x)) {
                        l.parent = None;
                    }
                    if l.track_matte.is_some_and(|m| gone.contains(&m.layer)) {
                        l.track_matte = None;
                    }
                }
            }
        }
        proj.render_queue.retain(|r| !doomed.contains(&r.comp));
        st.project_selection.retain(|i| !doomed.contains(i));
        Ok(())
    })?;
    s.sanitize_state();
    Ok(json!(n))
}

/// Duplicate project items (Edit ▸ Duplicate in the Project panel): comps get fresh layer ids and
/// a numbered name; the copies are selected.
fn duplicate(s: &mut Session, p: &Value) -> Result<Value> {
    let c = "project.duplicate";
    let ids = items_p(s, p, c)?;
    let mut created = vec![];
    s.edit("Duplicate", None, |proj, st| {
        for id in &ids {
            let it = proj.item(*id).ok_or(EngineError::Other("item vanished".into()))?.clone();
            if it.is_folder() {
                return Err(bad(c, "folders can't be duplicated"));
            }
            let name = proj.numbered_item_name(&it.name, |_| false);
            let mut kind = it.kind.clone();
            if let effectcraft_project::ItemKind::Comp(comp) = &mut kind {
                let comp = std::sync::Arc::make_mut(comp);
                let mut map = std::collections::HashMap::new();
                for l in &mut comp.layers {
                    let new = effectcraft_project::LayerId(proj.alloc());
                    map.insert(l.id, new);
                    l.id = new;
                    let mut next = proj.next_id;
                    l.props.reassign_uids(&mut next);
                    proj.next_id = next + 1;
                }
                for l in &mut comp.layers {
                    l.parent = l.parent.and_then(|p| map.get(&p).copied());
                    if let Some(m) = &mut l.track_matte {
                        m.layer = map.get(&m.layer).copied().unwrap_or(m.layer);
                    }
                }
            }
            let nid = proj.add_item(&name, it.label, it.parent, kind);
            created.push(nid);
        }
        st.project_selection = created.clone();
        Ok(())
    })?;
    Ok(json!({"items": created.iter().map(|i| i.0).collect::<Vec<_>>()}))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!("project.select", "Select Project Items", [], None, "{items: [id|name], add?}", always, select),
        cmd!("project.rename", "Rename Item", [], None, "{item?: id|name (default: selected), name}", always, rename),
        cmd!("project.move", "Move to Folder", [], None, "{items?: [id|name] (default: selected), folder: id|name|null (root)}", always, move_items),
        cmd!("project.setLabel", "Item Label", [], None, "{items?, label: name|index}", always, set_label),
        cmd!("project.setComment", "Item Comment", [], None, "{items?, comment}", always, set_comment),
        cmd!(
            "project.delete",
            "Delete Project Items",
            [],
            None,
            "{items?: [id|name] (default: selected)} (folders with their contents; layers using the items go too)",
            always,
            delete
        ),
        query!("project.usage", "Project Item Usage", "{items?: [id|name] (default: selected)} → {items, layers, comps} that deleting them removes", usage),
        query!("project.usedIn", "Project Item Used In", "{item?: id|name (default: selected)} → {item, count, comps: [{id, name, layers}]}", used_in),
        cmd!("project.duplicate", "Duplicate Project Items", [], None, "{items?: [id|name] (default: selected)} → {items}", always, duplicate),
    ]
}

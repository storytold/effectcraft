//! Manual sandbox link commands; all frontends use the engine registry.
use super::{CommandSpec, always, bad, str_p};
use crate::{Result, Session};
use serde_json::{Value, json};

fn link(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "file.linkComposition";
    let path = str_p(p, "path").ok_or_else(|| bad(cmd, "missing path"))?;
    let project = match p.get("sourceProjectId") {
        Some(Value::String(id)) => Some(id.as_str()),
        Some(_) => return Err(bad(cmd, "sourceProjectId must be a stable project ID")),
        None => None,
    };
    match (p.get("comp"), p.get("comps")) {
        (Some(comp), None) => crate::linked_compositions::link(s, path, std::slice::from_ref(comp), project, false),
        (None, Some(Value::Array(comps))) => crate::linked_compositions::link(s, path, comps, project, true),
        _ => Err(bad(cmd, "pass comp or a nonempty comps array")),
    }
}

fn source_compositions(s: &mut Session, p: &Value) -> Result<Value> {
    let path = str_p(p, "path").ok_or_else(|| bad("file.linkableCompositions", "missing path"))?;
    crate::linked_compositions::linkable_compositions(s, path)
}

fn refresh(s: &mut Session, p: &Value) -> Result<Value> {
    let item = p.get("item").map(|v| super::project_items::item_ref(s, v, "project.refreshLinkedCompositions")).transpose()?;
    crate::linked_compositions::refresh(s, item)
}

fn list(s: &mut Session, _: &Value) -> Result<Value> {
    Ok(json!({"links": crate::linked_compositions::list(s)}))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "file.linkComposition",
            "Linked EffectCraft Composition...",
            ["File", "Import"],
            None,
            "{path, comp?: id|name|stableId, comps?: [stableId], sourceProjectId?: stableId} — link saved source compositions as read-only footage; comps reuses existing links and makes one undo step",
            always,
            link
        ),
        query!(
            "file.linkableCompositions",
            "Linkable Source Compositions",
            "{path} — inspect saved source compositions and existing destination links by stable identity",
            source_compositions
        ),
        cmd!(
            "project.refreshLinkedCompositions",
            "Refresh Linked Compositions (Sandbox)",
            [],
            None,
            "{item?: id|name} — refresh all links, or every link to the selected item's source project; returns {links, errors}",
            always,
            refresh
        ),
        query!("project.linkedCompositions", "Linked Compositions", "{}", list),
    ]
}

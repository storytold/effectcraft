//! Generic edits of property-tree groups by uid: rename, enable, remove, duplicate and reorder an
//! effect, mask, shape item or text animator (what scripting's `PropertyGroup.name`, `enabled`,
//! `remove()`, `duplicate()` and `moveTo()` do, and what agents need without a UI selection).

use effectcraft_project::{ItemId, LayerId, LayerSource, Node, PropGroup, Uid};
use serde_json::{Value, json};

use super::{CommandSpec, b_p, bad, has_layers, layer_mut, layer_p, layers_p, str_p};
use crate::{EngineError, Result, Session, cmd};

/// Groups that can be renamed, removed, duplicated and reordered: instances in indexed lists.
fn is_instance(g: &PropGroup) -> bool {
    g.is_instance()
}

/// `{layer, prop: uid}` → (comp, layer, group uid), checking that the group is an instance.
fn group_ref(s: &Session, p: &Value, cmd: &str, need_instance: bool) -> Result<(ItemId, LayerId, Uid)> {
    let (cid, lid) = layer_p(s, p, cmd)?;
    let uid = p.get("prop").and_then(Value::as_u64).ok_or_else(|| bad(cmd, "missing `prop` (group uid)"))?;
    let l = s.project.comp(cid).and_then(|c| c.layer(lid)).ok_or(EngineError::NoComp)?;
    let g = l.props.find_group(uid).ok_or_else(|| bad(cmd, format!("no property group @{uid}")))?;
    if need_instance && !is_instance(g) {
        return Err(bad(cmd, format!("`{}` is a fixed group (only effects, masks, shape items and animators)", g.name)));
    }
    Ok((cid, lid, uid))
}

fn rename(s: &mut Session, p: &Value) -> Result<Value> {
    let c = "prop.renameGroup";
    let (cid, lid, uid) = group_ref(s, p, c, true)?;
    let name = str_p(p, "name").ok_or_else(|| bad(c, "missing `name`"))?.to_string();
    if name.trim().is_empty() {
        return Err(bad(c, "the name can't be empty"));
    }
    s.edit("Rename", None, |proj, _| {
        let g = layer_mut(proj, cid, lid)?.props.find_group_mut(uid).ok_or_else(|| bad(c, "group vanished"))?;
        g.name = name.clone();
        Ok(json!(name))
    })
}

fn set_enabled(s: &mut Session, p: &Value) -> Result<Value> {
    let c = "prop.setGroupEnabled";
    let (cid, lid, uid) = group_ref(s, p, c, true)?;
    let v = b_p(p, "value");
    s.edit("Toggle", None, |proj, _| {
        let g = layer_mut(proj, cid, lid)?.props.find_group_mut(uid).ok_or_else(|| bad(c, "group vanished"))?;
        g.enabled = v.unwrap_or(!g.enabled);
        Ok(json!(g.enabled))
    })
}

fn remove(s: &mut Session, p: &Value) -> Result<Value> {
    let (cid, lid, uid) = group_ref(s, p, "prop.removeGroup", true)?;
    remove_groups(s, cid, &[(lid, uid)], "Delete")?;
    Ok(Value::Null)
}

/// Remove groups of comp `cid` in one undo step.
fn remove_groups(s: &mut Session, cid: ItemId, groups: &[(LayerId, Uid)], label: &str) -> Result<()> {
    s.edit(label, None, |proj, st| {
        for (lid, uid) in groups {
            // A group inside one removed before it went with it.
            if let Some(parent) = layer_mut(proj, cid, *lid)?.props.parent_of_mut(*uid) {
                parent.children.retain(|n| n.uid() != *uid);
            }
        }
        st.selected_props.retain(|(_, u)| !groups.iter().any(|(_, g)| g == u));
        Ok(())
    })
}

/// Edit ▸ Clear with masks, shape items, text animators or trackers selected in the active comp:
/// removes those groups (not their layers) in one undo step. None when none are selected.
pub(crate) fn remove_selected(s: &mut Session) -> Result<Option<Value>> {
    let (Some(cid), Some(c)) = (s.active_comp_id(), s.active_comp()) else { return Ok(None) };
    let groups: Vec<(LayerId, Uid)> =
        s.state.selected_props.iter().filter(|(lid, uid)| c.layer(*lid).and_then(|l| l.props.find_group(*uid)).is_some_and(is_instance)).copied().collect();
    if groups.is_empty() {
        return Ok(None);
    }
    remove_groups(s, cid, &groups, "Delete")?;
    Ok(Some(json!(groups.len())))
}

/// Shape items (groups, paths, paints and path operations) selected in the Contents of the
/// active comp's shape layers, top to bottom. An item inside another selected one goes with it,
/// so it isn't listed.
pub(crate) fn selected_contents(s: &Session) -> Vec<(LayerId, Uid)> {
    let mut out = vec![];
    let Some(c) = s.active_comp().filter(|_| !s.state.selected_props.is_empty()) else { return out };
    for l in c.layers.iter().filter(|l| matches!(l.source, LayerSource::Shape)) {
        if let Some(contents) = l.props.sub("contents") {
            let selected = |u: Uid| s.state.selected_props.contains(&(l.id, u));
            let mut uids = vec![];
            collect_selected(contents, &selected, &mut uids, 0);
            out.extend(uids.into_iter().map(|u| (l.id, u)));
        }
    }
    out
}

/// Text animators and their selectors selected on the active comp's text layers, top to bottom.
/// A selector inside a selected animator isn't listed, since it is copied with it.
pub(crate) fn selected_animator_items(s: &Session) -> Vec<(LayerId, Uid)> {
    let mut out = vec![];
    let Some(c) = s.active_comp().filter(|_| !s.state.selected_props.is_empty()) else { return out };
    for l in c.layers.iter().filter(|l| matches!(l.source, LayerSource::Text)) {
        if let Some(animators) = l.props.group("text/animators") {
            let selected = |u: Uid| s.state.selected_props.contains(&(l.id, u));
            let mut uids = vec![];
            collect_selected(animators, &selected, &mut uids, 0);
            out.extend(uids.into_iter().map(|u| (l.id, u)));
        }
    }
    out
}

fn collect_selected(g: &PropGroup, selected: &dyn Fn(Uid) -> bool, out: &mut Vec<Uid>, depth: usize) {
    if depth > super::shape_stroke::MAX_DEPTH {
        return;
    }
    for c in g.groups() {
        if is_instance(c) && selected(c.uid) {
            out.push(c.uid);
        } else {
            collect_selected(c, selected, out, depth + 1);
        }
    }
}

/// Copies of the selected shape items (Edit ▸ Copy), top to bottom.
pub(crate) fn copy_contents(s: &Session) -> Vec<PropGroup> {
    let Some(c) = s.active_comp() else { return vec![] };
    selected_contents(s).into_iter().filter_map(|(lid, uid)| c.layer(lid)?.props.find_group(uid).cloned()).collect()
}

/// Edit ▸ Cut's removal of the selected shape items (once copied).
pub(crate) fn cut_contents(s: &mut Session, items: &[(LayerId, Uid)]) -> Result<Value> {
    let cid = s.active_comp_id().ok_or(EngineError::NoComp)?;
    remove_groups(s, cid, items, "Cut")?;
    Ok(json!({"contents": items.len()}))
}

/// Edit ▸ Paste with shape items on the clipboard: copies go into each selected shape layer's
/// Contents, above its topmost selected item (in that item's group), else on top (paints and path
/// operations alone: at the bottom). Names stay unique in their group ("Rectangle 2"…), and the
/// copies are selected.
pub(crate) fn paste_contents(s: &mut Session, p: &Value) -> Result<Value> {
    let c = "edit.paste";
    let clip = s.state.contents_clipboard.clone();
    let (cid, ids) = layers_p(s, p)?;
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    let targets: Vec<LayerId> =
        ids.into_iter().filter(|l| comp.layer(*l).is_some_and(|l| matches!(l.source, LayerSource::Shape) && !l.switches.locked)).collect();
    if targets.is_empty() {
        return Err(bad(c, "select a shape layer to paste shape items into"));
    }
    let sel = selected_contents(s);
    let acts_on_above = clip.iter().all(|g| !matches!(g.match_id.as_str(), "group" | "rect" | "ellipse" | "star" | "path"));
    // Merge Paths clears the draws above it, so at the bottom it would erase the paints (#1681).
    let has_merge = clip.iter().any(|g| g.match_id == "merge");
    let label = match clip.as_slice() {
        [g] => format!("Paste {}", g.name),
        _ => "Paste".to_string(),
    };
    let pasted = s.edit(&label, None, |proj, st| {
        let mut next = proj.next_id;
        let mut out = vec![];
        for lid in &targets {
            let l = layer_mut(proj, cid, *lid)?;
            // Above the layer's topmost selected item, in its group.
            let spot = sel.iter().find(|(sl, _)| sl == lid).and_then(|(_, u)| {
                let parent = l.props.parent_of(*u)?;
                Some((parent.uid, parent.children.iter().position(|n| n.uid() == *u)?))
            });
            let unselected = spot.is_none();
            let (parent, at) = match spot {
                Some((pu, i)) => (l.props.find_group_mut(pu), i),
                None => (l.props.sub_mut("contents"), 0),
            };
            let parent = parent.ok_or_else(|| bad(c, "the layer has no contents"))?;
            // Trim Paths, Wiggle Paths, fills… act on the items above them (as Add places them),
            // so with nothing selected they go to the bottom: on top they'd affect nothing (#1681).
            let at = match unselected {
                true if has_merge => merge_spot(parent),
                true if acts_on_above => parent.children.len(),
                _ => at,
            };
            for (k, g) in clip.iter().enumerate() {
                let mut g = g.clone();
                g.reassign_uids(&mut next);
                g.name = super::effect::unique_name(parent, &g.name);
                out.push((*lid, g.uid));
                parent.children.insert((at + k).min(parent.children.len()), g.into());
            }
        }
        proj.next_id = next + 1;
        st.selected_props = out.clone();
        Ok(out)
    })?;
    Ok(json!({"contents": pasted.iter().map(|(_, u)| *u).collect::<Vec<_>>()}))
}

/// Where Merge Paths goes in `contents` when nothing is selected: before the first paint, so the
/// paints still draw the merged path (the renderer drops the draws above a Merge Paths), else after
/// the last path item.
fn merge_spot(contents: &PropGroup) -> usize {
    let is = |n: &Node, ids: &[&str]| matches!(n, Node::Group(g) if ids.contains(&g.match_id.as_str()));
    let paints = ["fill", "stroke", "gfill", "gstroke"];
    match contents.children.iter().position(|n| is(n, &paints)) {
        Some(i) => i,
        None => contents.children.iter().rposition(|n| is(n, &["rect", "ellipse", "star", "path"])).map_or(0, |i| i + 1),
    }
}

/// Edit ▸ Duplicate with shape items, text animators or selectors selected: each is copied in
/// place, above the original and named as After Effects does ("Rectangle 1" → "Rectangle 2"),
/// and the copies are selected.
pub(crate) fn duplicate_contents(s: &mut Session, items: &[(LayerId, Uid)]) -> Result<Value> {
    let c = "edit.duplicate";
    let cid = s.active_comp_id().ok_or(EngineError::NoComp)?;
    let copies = s.edit("Duplicate", None, |proj, st| {
        let mut next = proj.next_id;
        let mut out = vec![];
        for (lid, uid) in items {
            let parent = layer_mut(proj, cid, *lid)?.props.parent_of_mut(*uid).ok_or_else(|| bad(c, "the group is gone"))?;
            let Some((i, Node::Group(g))) = parent.children.iter().enumerate().find(|(_, n)| n.uid() == *uid) else { continue };
            let mut g = g.clone();
            g.reassign_uids(&mut next);
            g.name = super::effect::unique_name(parent, &g.name);
            out.push((*lid, g.uid));
            parent.children.insert(i, Node::Group(g));
        }
        proj.next_id = next + 1;
        st.selected_props = out.clone();
        Ok(out)
    })?;
    Ok(json!({"contents": copies.iter().map(|(_, u)| *u).collect::<Vec<_>>()}))
}

fn duplicate(s: &mut Session, p: &Value) -> Result<Value> {
    let c = "prop.duplicateGroup";
    let (cid, lid, uid) = group_ref(s, p, c, true)?;
    s.edit("Duplicate", None, |proj, _| {
        let mut next = proj.next_id;
        let l = layer_mut(proj, cid, lid)?;
        let parent = l.props.parent_of_mut(uid).ok_or_else(|| bad(c, "group vanished"))?;
        let i = parent.children.iter().position(|n| n.uid() == uid).ok_or_else(|| bad(c, "group vanished"))?;
        let Node::Group(mut g) = parent.children[i].clone() else { return Err(bad(c, "not a group")) };
        g.reassign_uids(&mut next);
        let new = g.uid;
        parent.children.insert(i + 1, Node::Group(g));
        proj.next_id = next + 1;
        Ok(json!({"prop": new}))
    })
}

fn move_to(s: &mut Session, p: &Value) -> Result<Value> {
    let c = "prop.moveGroup";
    let (cid, lid, uid) = group_ref(s, p, c, true)?;
    let to = p.get("index").and_then(Value::as_u64).ok_or_else(|| bad(c, "missing `index` (1-based)"))? as usize;
    s.edit("Reorder", None, |proj, _| {
        let l = layer_mut(proj, cid, lid)?;
        let parent = l.props.parent_of_mut(uid).ok_or_else(|| bad(c, "group vanished"))?;
        let i = parent.children.iter().position(|n| n.uid() == uid).ok_or_else(|| bad(c, "group vanished"))?;
        if to < 1 || to > parent.children.len() {
            return Err(bad(c, format!("index {to} is out of range 1..{}", parent.children.len())));
        }
        let n = parent.children.remove(i);
        parent.children.insert(to - 1, n);
        Ok(json!(to))
    })
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!("prop.renameGroup", "Rename Property Group", [], None, "{layer?, prop: uid, name}", has_layers, rename),
        cmd!("prop.setGroupEnabled", "Enable Property Group", [], None, "{layer?, prop: uid, value?}", has_layers, set_enabled),
        cmd!("prop.removeGroup", "Delete Property Group", [], None, "{layer?, prop: uid}", has_layers, remove),
        cmd!("prop.duplicateGroup", "Duplicate Property Group", [], None, "{layer?, prop: uid} → {prop: new uid}", has_layers, duplicate),
        cmd!("prop.moveGroup", "Reorder Property Group", [], None, "{layer?, prop: uid, index (1-based among its siblings)}", has_layers, move_to),
    ]
}

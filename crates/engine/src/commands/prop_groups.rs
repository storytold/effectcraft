//! Generic edits of property-tree groups by uid: rename, enable, remove, duplicate and reorder an
//! effect, mask, shape item or text animator (what scripting's `PropertyGroup.name`, `enabled`,
//! `remove()`, `duplicate()` and `moveTo()` do, and what agents need without a UI selection).

use std::collections::{BTreeMap, BTreeSet};

use effectcraft_project::{GroupKind, ItemId, LayerId, LayerSource, Node, PropGroup, Uid};
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
/// Contents, above its topmost selected item (in that item's group), else on top. Names stay
/// unique in their group ("Rectangle 2"…), and the copies are selected.
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
            let (parent, at) = match spot {
                Some((pu, i)) => (l.props.find_group_mut(pu), i),
                None => (l.props.sub_mut("contents"), 0),
            };
            let parent = parent.ok_or_else(|| bad(c, "the layer has no contents"))?;
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

/// Edit ▸ Duplicate with shape items selected: each is copied in place, above the original and
/// named as After Effects does ("Rectangle 1" → "Rectangle 2"), and the copies are selected.
pub(crate) fn duplicate_contents(s: &mut Session, items: &[(LayerId, Uid)]) -> Result<Value> {
    let c = "edit.duplicate";
    let cid = s.active_comp_id().ok_or(EngineError::NoComp)?;
    let copies = s.edit("Duplicate", None, |proj, st| {
        let mut next = proj.next_id;
        let mut out = vec![];
        for (lid, uid) in items {
            let parent = layer_mut(proj, cid, *lid)?.props.parent_of_mut(*uid).ok_or_else(|| bad(c, "the shape item is gone"))?;
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
    if p.get("parent").is_some() || p.get("props").is_some() {
        return move_shape_items(s, p);
    }
    let (cid, lid, uid) = group_ref(s, p, c, true)?;
    let to = p.get("index").and_then(Value::as_u64).and_then(|n| usize::try_from(n).ok()).ok_or_else(|| bad(c, "missing or invalid `index` (1-based)"))?;
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

/// A bounded, ordered view of a shape tree: group, parent uid and depth from layer Contents.
/// Moving a group must never turn a deeply nested imported tree into an unbounded walk.
fn shape_tree(root: &PropGroup) -> Result<Vec<(&PropGroup, Option<Uid>, usize)>> {
    let mut out = vec![];
    let mut stack = vec![(root, None, 0usize)];
    while let Some((group, parent, depth)) = stack.pop() {
        if depth > super::shape_stroke::MAX_DEPTH || out.len() >= 100_000 {
            return Err(bad("prop.moveGroup", "shape contents are too deeply nested or too large"));
        }
        out.push((group, parent, depth));
        for child in group.children.iter().rev().filter_map(Node::as_group) {
            if out.len().saturating_add(stack.len()) >= 100_000 {
                return Err(bad("prop.moveGroup", "shape contents are too large"));
            }
            stack.push((child, Some(group.uid), depth + 1));
        }
    }
    Ok(out)
}

/// Reparent shape items without rebuilding them: property uids, keys and expressions travel
/// with the item. `index` counts the destination's children after removing all moving items.
fn move_shape_items(s: &mut Session, p: &Value) -> Result<Value> {
    let c = "prop.moveGroup";
    let (cid, lid) = layer_p(s, p, c)?;
    let layer = s.project.comp(cid).and_then(|comp| comp.layer(lid)).ok_or(EngineError::NoComp)?;
    if !matches!(layer.source, LayerSource::Shape) || layer.switches.locked {
        return Err(bad(c, "select an unlocked shape layer"));
    }
    let root = layer.props.sub("contents").ok_or_else(|| bad(c, "the layer has no contents"))?;
    let tree = shape_tree(root)?;
    let by_uid: BTreeMap<Uid, (&PropGroup, Option<Uid>, usize)> = tree.iter().map(|entry| (entry.0.uid, *entry)).collect();
    let requested = if let Some(props) = p.get("props") {
        let values = props.as_array().ok_or_else(|| bad(c, "`props` must be an array of group uids"))?;
        if values.is_empty() || values.len() > 100_000 {
            return Err(bad(c, "select at least one shape item (at most 100000)"));
        }
        values.iter().map(|v| v.as_u64().ok_or_else(|| bad(c, "`props` must contain group uids"))).collect::<Result<Vec<_>>>()?
    } else {
        vec![p.get("prop").and_then(Value::as_u64).ok_or_else(|| bad(c, "missing `prop` (group uid)"))?]
    };
    for uid in &requested {
        let entry = by_uid.get(uid).ok_or_else(|| bad(c, format!("no shape item @{uid}")))?;
        if !matches!(entry.0.kind, GroupKind::Indexed) || entry.1.is_none_or(|parent| by_uid.get(&parent).is_none_or(|(g, _, _)| g.match_id != "contents")) {
            return Err(bad(c, "only paths, paints, operators and shape groups can be moved"));
        }
    }
    // Ancestors carry their selected children with them; preserve the tree's visual order.
    let mut items = vec![];
    let mut selected_depth = None;
    let requested: BTreeSet<Uid> = requested.into_iter().collect();
    for (g, _, depth) in &tree {
        if selected_depth.is_some_and(|d| *depth <= d) {
            selected_depth = None;
        }
        if requested.contains(&g.uid) && selected_depth.is_none() {
            items.push(g.uid);
            selected_depth = Some(*depth);
        }
    }
    let parent_arg = p.get("parent").and_then(Value::as_u64).ok_or_else(|| bad(c, "missing `parent` (Contents or shape group uid)"))?;
    let target = by_uid.get(&parent_arg).ok_or_else(|| bad(c, "no destination group"))?;
    let destination = if target.0.match_id == "group" { target.0.sub("contents").ok_or_else(|| bad(c, "the shape group has no contents"))? } else { target.0 };
    if destination.match_id != "contents" {
        return Err(bad(c, "the destination must be shape Contents"));
    }
    if destination.uid != root.uid
        && !by_uid
            .get(&destination.uid)
            .and_then(|(_, parent, _)| *parent)
            .and_then(|uid| by_uid.get(&uid))
            .is_some_and(|(g, _, _)| g.match_id == "group" && matches!(g.kind, GroupKind::Indexed))
    {
        return Err(bad(c, "the destination must be a shape group's Contents"));
    }
    let dest_depth = by_uid.get(&destination.uid).map(|(_, _, d)| *d).ok_or_else(|| bad(c, "no destination contents"))?;
    for uid in &items {
        let group = by_uid.get(uid).map(|(g, _, _)| *g).ok_or_else(|| bad(c, "shape item vanished"))?;
        let subtree = shape_tree(group)?;
        if subtree.iter().any(|(g, _, _)| g.uid == destination.uid) {
            return Err(bad(c, "a shape group cannot be moved into itself or its descendants"));
        }
        if subtree.iter().any(|(_, _, depth)| dest_depth.saturating_add(1).saturating_add(*depth) > super::shape_stroke::MAX_DEPTH) {
            return Err(bad(c, "the move would nest shape contents too deeply"));
        }
    }
    let parent = destination.uid;
    let item_set: BTreeSet<Uid> = items.iter().copied().collect();
    let remaining: Vec<Uid> = destination.children.iter().map(Node::uid).filter(|uid| !item_set.contains(uid)).collect();
    let end = remaining.len().saturating_add(1);
    let to = match p.get("index") {
        Some(v) => {
            v.as_u64().and_then(|n| usize::try_from(n).ok()).filter(|n| *n >= 1 && *n <= end).ok_or_else(|| bad(c, format!("index must be in 1..{end}")))?
        }
        None => end,
    };
    let mut order = remaining;
    order.splice(to - 1..to - 1, items.iter().copied());
    let reply = json!({"props": items, "parent": parent, "index": to});
    if order == destination.children.iter().map(Node::uid).collect::<Vec<_>>() {
        return Ok(reply);
    }
    s.edit("Move Shape Contents", None, |project, state| {
        let layer = layer_mut(project, cid, lid)?;
        let mut moving = Vec::with_capacity(items.len());
        for uid in &items {
            let source = layer.props.parent_of_mut(*uid).ok_or_else(|| bad(c, "shape item vanished"))?;
            let at = source.children.iter().position(|n| n.uid() == *uid).ok_or_else(|| bad(c, "shape item vanished"))?;
            moving.push(source.children.remove(at));
        }
        let destination = layer.props.find_group_mut(parent).ok_or_else(|| bad(c, "destination contents vanished"))?;
        destination.children.splice(to - 1..to - 1, moving);
        state.selected_props = items.iter().map(|uid| (lid, *uid)).collect();
        Ok(reply)
    })
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!("prop.renameGroup", "Rename Property Group", [], None, "{layer?, prop: uid, name}", has_layers, rename),
        cmd!("prop.setGroupEnabled", "Enable Property Group", [], None, "{layer?, prop: uid, value?}", has_layers, set_enabled),
        cmd!("prop.removeGroup", "Delete Property Group", [], None, "{layer?, prop: uid}", has_layers, remove),
        cmd!("prop.duplicateGroup", "Duplicate Property Group", [], None, "{layer?, prop: uid} → {prop: new uid}", has_layers, duplicate),
        cmd!(
            "prop.moveGroup",
            "Move Property Group",
            [],
            None,
            "{layer?, prop?: uid, props?: [uid], parent?: Contents|shape group uid, index?: 1-based} → position or moved items; without parent, prop and index reorder siblings; with parent, one of prop/props moves shape items, index counts remaining children and defaults to append",
            has_layers,
            move_to
        ),
    ]
}

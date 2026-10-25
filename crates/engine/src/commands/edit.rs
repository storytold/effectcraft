//! Edit menu.

use effectcraft_color::Label;
use effectcraft_keyframe::Value as KV;
use effectcraft_project::{Comp, Expression, ItemKind, Layer, LayerId, LayerSource, TrackMatte};
use effectcraft_time::Tick;
use serde_json::{Value, json};

use super::{CommandSpec, has_comp, has_layers, layers_p, match_path_of, selected_leaf_props, str_p};
use crate::{EngineError, KeyRef, LinkClip, Result, Session, cmd};

fn can_undo(s: &Session) -> std::result::Result<(), String> {
    if s.history.undo.is_empty() { Err("nothing to undo".into()) } else { Ok(()) }
}
fn can_redo(s: &Session) -> std::result::Result<(), String> {
    if s.history.redo.is_empty() { Err("nothing to redo".into()) } else { Ok(()) }
}
fn has_clip(s: &Session) -> std::result::Result<(), String> {
    has_comp(s)?;
    if s.state.text_edit.is_some() {
        // Text editing pastes the system clipboard's text (passed as `text`) or copied text.
        return Ok(());
    }
    if s.state.clipboard.is_empty()
        && s.state.key_clipboard.is_empty()
        && s.state.effect_clipboard.is_empty()
        && s.state.contents_clipboard.is_empty()
        && s.state.link_clipboard.is_none()
        && s.state.value_clipboard.is_empty()
    {
        Err("the clipboard is empty".into())
    } else {
        Ok(())
    }
}
fn has_key_clip(s: &Session) -> std::result::Result<(), String> {
    has_layers(s)?;
    if s.state.key_clipboard.is_empty() { Err("no keyframes on the clipboard".into()) } else { Ok(()) }
}
fn has_props_or_layers(s: &Session) -> std::result::Result<(), String> {
    has_comp(s)?;
    if s.state.selected_layers.is_empty() && s.state.selected_props.is_empty() { Err("select layers or properties first".into()) } else { Ok(()) }
}
fn has_props_or_layers_or_items(s: &Session) -> std::result::Result<(), String> {
    if has_props_or_layers(s).is_ok() || !s.state.project_selection.is_empty() { Ok(()) } else { Err("select layers or project items first".into()) }
}
fn has_selected_footage(s: &Session) -> std::result::Result<(), String> {
    if original_path(s).is_some() { Ok(()) } else { Err("select a footage item or layer".into()) }
}

fn undo(s: &mut Session, _: &Value) -> Result<Value> {
    let label = s.history.undo.last().map(|u| u.0.clone());
    s.undo();
    Ok(json!({"undone": label}))
}
fn redo(s: &mut Session, _: &Value) -> Result<Value> {
    let label = s.history.redo.last().map(|u| u.0.clone());
    s.redo();
    Ok(json!({"redone": label}))
}

fn select_all(s: &mut Session, _: &Value) -> Result<Value> {
    if s.state.text_edit.is_some() {
        return s.execute("text.setSelection", json!({"select": "all"}));
    }
    // A puppet pin selected: every pin of that kind.
    if let Some(r) = super::puppet::select_all_of_kind(s) {
        return Ok(r);
    }
    // Locked layers (and shy layers while they are hidden) can't be selected.
    if let Some(c) = s.active_comp() {
        s.state.selected_layers = c.layers.iter().filter(|l| super::selectable(c, l)).map(|l| l.id).collect();
    }
    Ok(json!(s.state.selected_layers.len()))
}
fn deselect_all(s: &mut Session, _: &Value) -> Result<Value> {
    // A deselected layer never keeps an open text edit: commit the edit first, exactly as
    // `text.endEdit` (an empty Type-tool layer included), so every route — menu, shortcut,
    // script, MCP — leaves one final state.
    super::text_edit::end_edit(s, &Value::Null)?;
    s.state.selected_layers.clear();
    s.state.selected_props.clear();
    s.state.selected_keys.clear();
    s.state.selected_vertices.clear();
    Ok(Value::Null)
}

/// Copied layers keep their parent and track matte: the copy of that layer when it was copied
/// too, else the layer itself when it is in `comp`, else none. `map`: (original, copy).
fn relink_copies(comp: &mut Comp, map: &[(LayerId, LayerId)]) {
    let present: Vec<LayerId> = comp.layers.iter().map(|l| l.id).collect();
    let link = |id: LayerId| map.iter().find(|(o, _)| *o == id).map(|(_, c)| *c).or(present.contains(&id).then_some(id));
    for l in comp.layers.iter_mut().filter(|l| map.iter().any(|(_, c)| *c == l.id)) {
        l.parent = l.parent.and_then(link);
        l.track_matte = l.track_matte.and_then(|m| link(m.layer).map(|layer| TrackMatte { layer, ..m }));
    }
}

/// Fresh ids for a copied layer (and its property uids).
pub(crate) fn reid(layer: &mut Layer, next: &mut u64) {
    layer.id = LayerId(*next);
    *next += 1;
    layer.props.reassign_uids(next);
    *next += 1;
}

fn duplicate(s: &mut Session, p: &Value) -> Result<Value> {
    // Effects selected → duplicate them (Edit ▸ Duplicate in Effect Controls).
    if p.get("layers").is_none() {
        let fx = super::effect::selected_effects(s);
        if !fx.is_empty() {
            let mut out = vec![];
            for (lid, uid) in fx {
                out.push(s.execute("effect.duplicate", json!({"layer": lid.0, "effect": uid}))?);
            }
            return Ok(json!({"effects": out}));
        }
        // Shape items selected → duplicate them inside their layers.
        let items = super::prop_groups::selected_contents(s);
        if !items.is_empty() {
            return super::prop_groups::duplicate_contents(s, &items);
        }
        // Text animators or their selectors selected → duplicate them inside their text layers.
        let items = super::prop_groups::selected_animator_items(s);
        if !items.is_empty() {
            return super::prop_groups::duplicate_contents(s, &items);
        }
    }
    let (cid, ids) = layers_p(s, p)?;
    let new = s.edit("Duplicate", None, |proj, st| {
        let mut next = proj.next_id;
        let comp = proj.comp_mut(cid).ok_or(crate::EngineError::NoComp)?;
        let mut created = vec![];
        let mut map = vec![];
        for id in &ids {
            let Some(i) = comp.layers.iter().position(|l| l.id == *id) else { continue };
            let mut l = comp.layers[i].clone();
            reid(&mut l, &mut next);
            l.name = comp.unique_layer_name(&l.name);
            created.push(l.id);
            map.push((*id, l.id));
            comp.layers.insert(i, l);
        }
        relink_copies(comp, &map);
        proj.next_id = next;
        st.selected_layers = created.clone();
        Ok(created)
    })?;
    Ok(json!(new.iter().map(|l| l.0).collect::<Vec<_>>()))
}

fn delete(s: &mut Session, p: &Value) -> Result<Value> {
    if s.state.text_edit.is_some() && p.get("layers").is_none() {
        return s.execute("text.delete", json!({}));
    }
    // Time Remap selected (its name, which selects all its keys) → time remapping off, not the
    // layer (After Effects).
    if p.get("layers").is_none() {
        let remapped = super::layer_time::selected_time_remap(s);
        if !remapped.is_empty() {
            return super::layer_time::disable_time_remap(s, &remapped);
        }
    }
    // Keyframes selected → delete keys; mask vertices → delete them; else layers.
    if !s.state.selected_keys.is_empty() && p.get("layers").is_none() {
        return s.execute("keys.delete", json!({}));
    }
    // (Points selected with their mask, which selecting a mask does: the mask goes, below.)
    let comp = s.active_comp();
    let loose = s.state.selected_vertices.iter().any(|v| {
        !s.state.selected_props.contains(&(v.layer, v.mask)) && comp.and_then(|c| c.layer(v.layer)).is_some_and(|l| l.props.find_group(v.mask).is_some())
    });
    if loose && p.get("layers").is_none() {
        return s.execute("mask.deleteVertices", json!({}));
    }
    // Puppet pins selected → delete the pins (not their layer).
    if p.get("layers").is_none() && !super::puppet::selected_pins(s).is_empty() {
        return s.execute("puppet.removePin", json!({}));
    }
    // Effects selected → remove them.
    if p.get("layers").is_none() && !super::effect::selected_effects(s).is_empty() {
        return s.execute("effect.remove", json!({}));
    }
    // Masks, shape items, text animators or trackers selected → remove them.
    if p.get("layers").is_none()
        && let Some(n) = super::prop_groups::remove_selected(s)?
    {
        return Ok(n);
    }
    let (cid, ids) = layers_p(s, p)?;
    let ids = super::unlocked(s, cid, ids, "edit.clear")?;
    // Children of deleted layers are unparented where they are (like After Effects).
    let comp = s.project.comp(cid).ok_or(crate::EngineError::NoComp)?;
    let orphans: Vec<LayerId> = comp.layers.iter().filter(|l| !ids.contains(&l.id) && l.parent.is_some_and(|p| ids.contains(&p))).map(|l| l.id).collect();
    let fixes = super::layer::parent_fixes(s, cid, &orphans, None);
    s.edit("Clear", None, |proj, st| {
        let comp = proj.comp_mut(cid).ok_or(crate::EngineError::NoComp)?;
        comp.layers.retain(|l| !ids.contains(&l.id));
        for l in &mut comp.layers {
            if l.parent.is_some_and(|p| ids.contains(&p)) {
                l.parent = None;
                if let Some(f) = fixes.iter().find(|f| f.layer == l.id) {
                    f.apply(l);
                }
            }
            if l.track_matte.is_some_and(|m| ids.contains(&m.layer)) {
                l.track_matte = None;
            }
            // A layer-reference property (an Environment light's Source, an effect's Clone Source,
            // Back Layer…) must read as None once its layer is gone, in this same history step: a
            // dead id renders as nothing while the UI shows "None", which means "fall back to the
            // comp's Environment Layer" (#2153). Keys carry the same value kind as the property.
            l.props.walk_mut(&mut |p| {
                let dead = |v: &KV| matches!(v, KV::Layer(Some(id)) if ids.contains(&LayerId(*id)));
                if dead(&p.value) {
                    p.value = KV::Layer(None);
                }
                for k in &mut p.keys {
                    if dead(&k.value) {
                        k.value = KV::Layer(None);
                    }
                }
            });
        }
        st.selected_layers.clear();
        st.selected_props.clear();
        st.selected_keys.clear();
        Ok(())
    })?;
    // A removed layer may have been the edited one: end that session before another command
    // can reuse it. Undo restores a layer as a new editor, never by reviving the old IME session.
    s.sanitize_state();
    Ok(json!(ids.len()))
}

fn clear_clipboards(s: &mut Session) {
    s.state.clipboard.clear();
    s.state.key_clipboard.clear();
    s.state.effect_clipboard.clear();
    s.state.contents_clipboard.clear();
    s.state.link_clipboard = None;
    s.state.value_clipboard.clear();
    s.state.clip_is_keys = false;
}

/// Edit ▸ Copy: selected properties copy their keyframes and values, selected keyframes go
/// to the keyframe clipboard, else layers.
fn copy(s: &mut Session, p: &Value) -> Result<Value> {
    if s.state.text_edit.is_some() && p.get("layers").is_none() {
        return super::text_edit::copy(s);
    }
    if p.get("layers").is_none()
        && let Some(r) = copy_properties(s)?
    {
        return Ok(r);
    }
    // Keyframes selected → copy keys (pasted at the CTI).
    if !s.state.selected_keys.is_empty() && p.get("layers").is_none() {
        clear_clipboards(s);
        return s.execute("keys.copy", json!({}));
    }
    // Effects selected (Effect Controls / timeline) → copy the effects.
    if p.get("layers").is_none() && !super::effect::selected_effects(s).is_empty() {
        return s.execute("effect.copy", json!({}));
    }
    // Shape items selected in a shape layer's Contents → copy them (not their layer).
    if p.get("layers").is_none() {
        let items = super::prop_groups::copy_contents(s);
        if !items.is_empty() {
            clear_clipboards(s);
            s.state.contents_clipboard = items;
            return Ok(json!({"contents": s.state.contents_clipboard.len()}));
        }
    }
    let (cid, ids) = layers_p(s, p)?;
    s.state.clip_is_keys = false;
    let comp = s.project.comp(cid).ok_or(crate::EngineError::NoComp)?;
    let layers: Vec<Layer> = comp.layers.iter().filter(|l| ids.contains(&l.id)).cloned().collect();
    clear_clipboards(s);
    s.state.clipboard = layers;
    Ok(json!(s.state.clipboard.len()))
}

/// Edit ▸ Copy with properties selected (Position, or the Transform group), as in After
/// Effects: an animated property copies its keyframes and an unanimated one its value, for Paste
/// onto the same properties of other layers (#493). With keyframes selected only those are copied,
/// even across several highlighted properties (#1552); a selected Transform group copies them all.
/// `None` when no property (or Transform group) is selected.
fn copy_properties(s: &mut Session) -> Result<Option<Value>> {
    let Some(comp) = s.active_comp() else { return Ok(None) };
    let mut leaves = vec![];
    let mut copy_all_keys = s.state.selected_keys.is_empty();
    for (lid, uid) in &s.state.selected_props {
        let Some(l) = comp.layer(*lid) else { continue };
        if l.props.find(*uid).is_some() {
            leaves.push((*lid, *uid));
        } else if let Some(g) = l.props.find_group(*uid)
            && l.props.match_path_of(*uid).as_deref() == Some("transform")
        {
            copy_all_keys = true;
            g.walk("", &mut |_, pr| leaves.push((*lid, pr.uid)));
        }
    }
    if leaves.is_empty() {
        return Ok(None);
    }
    let mut keys = s.state.selected_keys.clone();
    let mut values = vec![];
    for (lid, uid) in &leaves {
        let Some(l) = comp.layer(*lid) else { continue };
        let Some(pr) = l.props.find(*uid) else { continue };
        if pr.keys.is_empty() {
            if let Some(path) = l.props.match_path_of(*uid)
                && !values.iter().any(|(p, _)| *p == path)
            {
                values.push((path, pr.value.clone()));
            }
        } else if copy_all_keys && !keys.iter().any(|k| k.layer == *lid && k.prop == *uid) {
            keys.extend(pr.keys.iter().map(|k| KeyRef { layer: *lid, prop: *uid, time: k.time }));
        }
    }
    clear_clipboards(s);
    let mut n = 0;
    if !keys.is_empty() {
        let selected = std::mem::replace(&mut s.state.selected_keys, keys);
        let r = s.execute("keys.copy", json!({}));
        s.state.selected_keys = selected;
        n = r?.as_u64().unwrap_or(0);
    }
    let v = values.len();
    s.state.value_clipboard = values;
    // Keys alone answer as `keys.copy` does (their count).
    Ok(Some(if v == 0 { json!(n) } else { json!({"keys": n, "values": v}) }))
}

/// Paste copied property values (and keyframes, at the CTI) onto the same properties of the
/// selected layers, as one undo step.
fn paste_properties(s: &mut Session, p: &Value) -> Result<Value> {
    let cid = s.active_comp_id().ok_or(EngineError::NoComp)?;
    let ids = s.state.selected_layers.clone();
    if ids.is_empty() {
        return Err(EngineError::Other("select a layer to paste into".into()));
    }
    let values = s.state.value_clipboard.clone();
    let keys = s.state.clip_is_keys && !s.state.key_clipboard.is_empty();
    let t = s.time();
    // The values and the keys are one step: the keys' paste merges into the values' step.
    let merge = "edit.paste.properties";
    s.history.merge_key = None;
    let n = s.edit("Paste", keys.then_some(merge), |proj, _| {
        let mut n = 0;
        for lid in &ids {
            let l = super::layer_mut(proj, cid, *lid)?;
            if l.switches.locked {
                continue;
            }
            let lt = l.layer_time(t);
            for (path, v) in &values {
                let path = super::resolve_effect_path(l, path);
                if let Some(pr) = l.props.prop_mut(&path)
                    && let Some(v) = pr.value.coerce_json(&v.to_json())
                {
                    pr.set_value_at(lt, v);
                    n += 1;
                }
            }
        }
        Ok(n)
    })?;
    let mut r = json!({"values": n});
    if keys {
        let mut kp = p.clone();
        if let Some(o) = kp.as_object_mut() {
            o.insert("merge".into(), json!(merge));
        }
        let pasted = s.execute("keys.paste", kp);
        s.history.merge_key = None;
        r["keys"] = pasted?;
    }
    Ok(r)
}

/// The expression accessor for a property, e.g. `comp("Main").layer("Solid").transform("Position")`.
pub(crate) fn link_expression(s: &Session, lid: LayerId, uid: effectcraft_project::Uid, relative: bool) -> Option<String> {
    let cid = s.active_comp_id()?;
    let comp = s.project.comp(cid)?;
    let l = comp.layer(lid)?;
    let names = l.props.name_path_of(uid)?;
    let matches = match_path_of(&l.props, uid)?;
    let names: Vec<&str> = names.split('/').collect();
    let first = matches.split('/').next()?.to_string();
    let q = |x: &str| serde_json::to_string(x).unwrap_or_default();
    let base = if relative { format!("thisComp.layer({})", q(&l.name)) } else { format!("comp({}).layer({})", q(&s.project.item(cid)?.name), q(&l.name)) };
    let mut out = match first.as_str() {
        "transform" => format!("{base}.transform"),
        "effects" => format!("{base}.effect({})", q(names.get(1)?)),
        "masks" => format!("{base}.mask({})", q(names.get(1)?)),
        "text" => format!("{base}.text"),
        "contents" => format!("{base}.content({})", q(names.get(1)?)),
        _ => return None,
    };
    let skip = if matches!(first.as_str(), "effects" | "masks" | "contents") { 2 } else { 1 };
    for n in names.iter().skip(skip) {
        out.push_str(&format!("({})", q(n)));
    }
    Some(out)
}

fn copy_links(s: &mut Session, p: &Value, relative: bool) -> Result<Value> {
    let props = selected_leaf_props(s);
    if props.is_empty() {
        // Layers: copy them with every animatable transform/effect property linked to the source.
        let (cid, ids) = layers_p(s, p)?;
        let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
        let mut layers = vec![];
        for l in comp.layers.iter().filter(|l| ids.contains(&l.id)) {
            let mut copy = l.clone();
            let mut uids = vec![];
            for g in ["transform", "effects"] {
                if let Some(grp) = l.props.sub(g) {
                    grp.walk("", &mut |_, pr| {
                        if !pr.static_only && !pr.hold_only {
                            uids.push(pr.uid);
                        }
                    });
                }
            }
            for uid in uids {
                if let (Some(e), Some(pr)) = (link_expression(s, l.id, uid, relative), copy.props.find_mut(uid)) {
                    pr.expr = Some(Expression { text: e, enabled: true });
                }
            }
            layers.push(copy);
        }
        let n = layers.len();
        clear_clipboards(s);
        s.state.clipboard = layers;
        return Ok(json!({"layers": n}));
    }
    let comp = s.active_comp().ok_or(EngineError::NoComp)?;
    let mut links = vec![];
    for (lid, uid) in &props {
        let Some(path) = comp.layer(*lid).and_then(|l| match_path_of(&l.props, *uid)) else { continue };
        if let Some(e) = link_expression(s, *lid, *uid, relative) {
            links.push((path, e));
        }
    }
    let n = links.len();
    clear_clipboards(s);
    s.state.link_clipboard = Some(LinkClip::Links { relative, links });
    Ok(json!({"links": n}))
}

fn copy_expression_only(s: &mut Session, _: &Value) -> Result<Value> {
    let comp = s.active_comp().ok_or(EngineError::NoComp)?;
    let mut out = vec![];
    for (lid, uid) in selected_leaf_props(s) {
        let Some(l) = comp.layer(lid) else { continue };
        if let (Some(pr), Some(path)) = (l.props.find(uid), match_path_of(&l.props, uid))
            && let Some(e) = &pr.expr
        {
            out.push((path, e.clone()));
        }
    }
    if out.is_empty() {
        return Err(EngineError::Other("the selected properties have no expressions".into()));
    }
    let n = out.len();
    clear_clipboards(s);
    s.state.link_clipboard = Some(LinkClip::Expressions(out));
    Ok(json!({"expressions": n}))
}

/// Paste the keyframe clipboard at the CTI onto the selected layers (same property paths).
fn paste_links(s: &mut Session, clip: LinkClip) -> Result<Value> {
    let cid = s.active_comp_id().ok_or(EngineError::NoComp)?;
    let ids = s.state.selected_layers.clone();
    if ids.is_empty() {
        return Err(EngineError::Other("select a layer to paste into".into()));
    }
    let pairs: Vec<(String, Expression)> = match clip {
        LinkClip::Links { links, .. } => links.into_iter().map(|(p, e)| (p, Expression { text: e, enabled: true })).collect(),
        LinkClip::Expressions(v) => v,
    };
    let n = s.edit("Paste", None, |proj, _| {
        let mut n = 0;
        for lid in &ids {
            let l = super::layer_mut(proj, cid, *lid)?;
            for (path, e) in &pairs {
                let path = super::resolve_effect_path(l, path);
                if let Some(pr) = l.props.prop_mut(&path) {
                    pr.expr = Some(e.clone());
                    n += 1;
                }
            }
        }
        Ok(n)
    })?;
    Ok(json!({"expressions": n}))
}

fn cut(s: &mut Session, p: &Value) -> Result<Value> {
    if s.state.text_edit.is_some() && p.get("layers").is_none() {
        return super::text_edit::cut(s);
    }
    if !s.state.selected_keys.is_empty() && p.get("layers").is_none() {
        s.execute("keys.copy", json!({}))?;
        return s.execute("keys.delete", json!({}));
    }
    // Shape items selected → copied, then removed from their layers (Paste moves them).
    if p.get("layers").is_none() && super::effect::selected_effects(s).is_empty() {
        let items = super::prop_groups::selected_contents(s);
        if !items.is_empty() {
            copy(s, p)?;
            return super::prop_groups::cut_contents(s, &items);
        }
    }
    copy(s, p)?;
    delete(s, p)
}

fn paste(s: &mut Session, p: &Value) -> Result<Value> {
    if s.state.text_edit.is_some() {
        return super::text_edit::paste(s, p);
    }
    if let Some(clip) = s.state.link_clipboard.clone() {
        return paste_links(s, clip);
    }
    // A copied Mask Path / shape Path pasted onto Position becomes a motion path (#1425).
    if let [(_, effectcraft_keyframe::Value::Path(path))] = s.state.value_clipboard.as_slice()
        && let Some(target) = super::anim::path_target(s, p)?
    {
        let path = path.clone();
        return super::anim::paste_path(s, p, target, &path);
    }
    if !s.state.value_clipboard.is_empty() {
        return paste_properties(s, p);
    }
    if s.state.clip_is_keys && !s.state.key_clipboard.is_empty() {
        return s.execute("keys.paste", p.clone());
    }
    if !s.state.effect_clipboard.is_empty() {
        return s.execute("effect.paste", p.clone());
    }
    if !s.state.contents_clipboard.is_empty() {
        return super::prop_groups::paste_contents(s, p);
    }
    let cid = super::comp_id(s, p)?;
    let clip = s.state.clipboard.clone();
    let new = s.edit("Paste", None, |proj, st| {
        let mut next = proj.next_id;
        let comp = proj.comp_mut(cid).ok_or(crate::EngineError::NoComp)?;
        let at = st.selected_layers.first().and_then(|id| comp.layers.iter().position(|l| l.id == *id)).unwrap_or(0);
        let mut created = vec![];
        let mut map = vec![];
        for (k, mut l) in clip.into_iter().enumerate() {
            let original = l.id;
            reid(&mut l, &mut next);
            l.name = comp.unique_layer_name(&l.name);
            created.push(l.id);
            map.push((original, l.id));
            comp.layers.insert(at + k, l);
        }
        relink_copies(comp, &map);
        proj.next_id = next;
        st.selected_layers = created.clone();
        Ok(created)
    })?;
    Ok(json!(new.iter().map(|l| l.0).collect::<Vec<_>>()))
}

/// Largest SVG text `edit.pasteSvg` reads (a clipboard can hold anything).
const MAX_SVG_PASTE: usize = 64 << 20;

/// `edit.pasteSvg`: SVG markup (the system clipboard's, copied from a browser, Illustrator or a
/// design tool) as a new shape layer above the selected layer, its canvas centred in the comp:
/// After Effects 26.3 pastes copied vector artwork as shape layers. Edit ▸ Paste runs it when
/// the system clipboard holds SVG copied after anything copied in the app.
fn paste_svg(s: &mut Session, p: &Value) -> Result<Value> {
    let c = "edit.pasteSvg";
    let text = str_p(p, "svg").ok_or_else(|| super::bad(c, "missing `svg` (SVG markup)"))?;
    if text.len() > MAX_SVG_PASTE {
        return Err(super::bad(c, format!("the SVG is larger than {} MB", MAX_SVG_PASTE >> 20)));
    }
    if !crate::vector::is_svg_text(text) {
        return Err(super::bad(c, "the text is not SVG markup"));
    }
    let doc = effectcraft_svg::parse(text.as_bytes()).map_err(|e| super::bad(c, format!("cannot read the SVG: {e}")))?;
    if doc.flatten().is_empty() {
        return Err(super::bad(c, "the SVG has no shapes"));
    }
    let cid = super::comp_id(s, p)?;
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?.clone();
    let name = match str_p(p, "name") {
        Some(n) => n.to_string(),
        None => {
            let count = comp.layers.iter().filter(|l| matches!(l.source, LayerSource::Shape)).count();
            comp.unique_layer_name(&format!("Shape Layer {}", count.saturating_add(1)))
        }
    };
    let id = s.edit("Paste", None, |proj, st| {
        let l = crate::vector::shapes_from_svg(proj, &comp, &doc, &name);
        super::layer::insert_layer(proj, st, cid, l)
    })?;
    Ok(json!({"layer": id.0, "skipped": doc.skipped}))
}

fn split(s: &mut Session, p: &Value) -> Result<Value> {
    let (cid, ids) = layers_p(s, p)?;
    let t = s.time();
    // Settings ▸ General ▸ Create Split Layers Above Original Layer.
    let above = s.prefs.general.create_split_layers_above;
    let new = s.edit("Split Layer", None, |proj, st| {
        let mut next = proj.next_id;
        let comp = proj.comp_mut(cid).ok_or(crate::EngineError::NoComp)?;
        let mut created = vec![];
        for id in &ids {
            let Some(i) = comp.layers.iter().position(|l| l.id == *id) else { continue };
            if !(t > comp.layers[i].in_point && t < comp.layers[i].out_point) {
                continue;
            }
            let mut b = comp.layers[i].clone();
            reid(&mut b, &mut next);
            b.in_point = t;
            comp.layers[i].out_point = t;
            created.push(b.id);
            comp.layers.insert(if above { i } else { i + 1 }, b);
        }
        proj.next_id = next;
        st.selected_layers = created.clone();
        Ok(created)
    })?;
    Ok(json!(new.iter().map(|l| l.0).collect::<Vec<_>>()))
}

fn label(s: &mut Session, p: &Value) -> Result<Value> {
    // Keyframe colour labels: selected keyframes take the label (unless layers are named).
    let explicit_layers = p.get("layers").is_some() || p.get("layer").is_some();
    if p.get("keys").is_some() || (!s.state.selected_keys.is_empty() && !explicit_layers && str_p(p, "target") != Some("layers")) {
        return super::keys_more::label_keys(s, p);
    }
    let name = str_p(p, "label").unwrap_or("Red");
    let lab = s.prefs.label_from_name(name).ok_or_else(|| super::bad("edit.label", format!("unknown label `{name}`")))?;
    let (cid, ids) = layers_p(s, p)?;
    let items = s.state.project_selection.clone();
    s.edit("Label", None, |proj, _| {
        if ids.is_empty() {
            for i in &items {
                if let Some(it) = proj.item_mut(*i) {
                    it.label = lab;
                }
            }
        }
        if let Some(comp) = proj.comp_mut(cid) {
            for l in comp.layers.iter_mut().filter(|l| ids.contains(&l.id)) {
                l.label = lab;
            }
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

/// Edit ▸ Paste Reversed Keyframes: the copied keys mirrored in time over their whole span (the
/// last key first, incoming and outgoing eases swapped), pasted like Edit ▸ Paste
/// (`keys.paste`: at the current time, into the selected or the same properties).
fn paste_reversed(s: &mut Session, p: &Value) -> Result<Value> {
    let original = s.state.key_clipboard.clone();
    let span = original.iter().flat_map(|c| c.keys.iter().map(|k| k.time)).max().unwrap_or(Tick::ZERO);
    s.state.key_clipboard = original
        .iter()
        .map(|c| crate::KeyClip {
            keys: c
                .keys
                .iter()
                .rev()
                .map(|k| {
                    let mut k = k.clone();
                    k.time = span - k.time;
                    std::mem::swap(&mut k.in_interp, &mut k.out_interp);
                    std::mem::swap(&mut k.in_ease, &mut k.out_ease);
                    std::mem::swap(&mut k.spatial_in, &mut k.spatial_out);
                    k
                })
                .collect(),
            ..c.clone()
        })
        .collect();
    let r = s.execute("keys.paste", p.clone());
    s.state.key_clipboard = original;
    r
}

/// Lift (leave a gap) or extract (close the gap) the work area from the selected layers (all
/// unlocked layers when none are selected).
fn work_area_cut(s: &mut Session, p: &Value, extract: bool) -> Result<Value> {
    let cid = super::comp_id(s, p)?;
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    let (a, b) = comp.work_area;
    let ids: Vec<LayerId> = match layers_p(s, p)?.1 {
        v if v.is_empty() => comp.layers.iter().map(|l| l.id).collect(),
        v => v,
    };
    let gap = b - a;
    s.edit(if extract { "Extract Work Area" } else { "Lift Work Area" }, None, |proj, st| {
        let mut next = proj.next_id;
        let c = proj.comp_mut(cid).ok_or(EngineError::NoComp)?;
        let shift = |l: &mut Layer, d: Tick| {
            l.start_time += d;
            l.in_point += d;
            l.out_point += d;
        };
        let mut i = 0;
        while i < c.layers.len() {
            if !ids.contains(&c.layers[i].id) || c.layers[i].switches.locked {
                i += 1;
                continue;
            }
            let (lin, lout) = (c.layers[i].in_point, c.layers[i].out_point);
            if lout <= a {
                i += 1;
            } else if lin >= b {
                if extract {
                    shift(&mut c.layers[i], Tick::ZERO - gap);
                }
                i += 1;
            } else if lin >= a && lout <= b {
                c.layers.remove(i);
            } else if lin < a && lout > b {
                // Split around the work area: the layer keeps the head, a copy takes the tail.
                let mut tail = c.layers[i].clone();
                reid(&mut tail, &mut next);
                tail.in_point = b;
                if extract {
                    shift(&mut tail, Tick::ZERO - gap);
                }
                c.layers[i].out_point = a;
                c.layers.insert(i, tail);
                i += 2;
            } else if lin < a {
                c.layers[i].out_point = a;
                i += 1;
            } else {
                c.layers[i].in_point = b;
                if extract {
                    shift(&mut c.layers[i], Tick::ZERO - gap);
                }
                i += 1;
            }
        }
        let alive: Vec<LayerId> = c.layers.iter().map(|l| l.id).collect();
        st.selected_layers.retain(|l| alive.contains(l));
        proj.next_id = next;
        Ok(())
    })?;
    Ok(Value::Null)
}

fn lift(s: &mut Session, p: &Value) -> Result<Value> {
    work_area_cut(s, p, false)
}
fn extract(s: &mut Session, p: &Value) -> Result<Value> {
    work_area_cut(s, p, true)
}

fn select_label_group(s: &mut Session, _: &Value) -> Result<Value> {
    let comp = s.active_comp().ok_or(EngineError::NoComp)?;
    let labels: Vec<Label> = s.state.selected_layers.iter().filter_map(|l| comp.layer(*l)).map(|l| l.label).collect();
    if labels.is_empty() {
        // Project panel: items with the same labels.
        let items: Vec<Label> = s.state.project_selection.iter().filter_map(|i| s.project.item(*i)).map(|i| i.label).collect();
        s.state.project_selection = s.project.items.values().filter(|i| items.contains(&i.label)).map(|i| i.id).collect();
        return Ok(json!({"items": s.state.project_selection.len()}));
    }
    s.state.selected_layers = comp.layers.iter().filter(|l| labels.contains(&l.label)).map(|l| l.id).collect();
    Ok(json!({"layers": s.state.selected_layers.len()}))
}

fn purge_caches(s: &mut Session, p: &Value) -> Result<Value> {
    let what = str_p(p, "what").unwrap_or("all").to_string();
    if matches!(what.as_str(), "all" | "snapshot") {
        s.snapshot = None;
        s.state.viewer.show_snapshot = false;
    }
    let memory = matches!(what.as_str(), "all" | "memoryAndDisk" | "memory" | "image");
    let disk = matches!(what.as_str(), "all" | "memoryAndDisk" | "disk");
    if memory {
        s.layer_cache.clear();
        s.footage.purge();
    }
    let mut disk_entries = 0;
    if disk && let Some(dc) = &s.disk_cache {
        disk_entries = dc.stats().entries;
        dc.clear();
    } else if disk && let Some(h) = &s.storage {
        // The browser's disk cache (Origin Private File System).
        disk_entries = h.clear("diskCache").ok().and_then(|r| r["entries"].as_u64()).unwrap_or(0) as usize;
    }
    if matches!(what.as_str(), "all" | "image" | "memory" | "memoryAndDisk") {
        effectcraft_effects::roto::purge();
    }
    // The viewers' RAM preview goes with the memory purges (and the 3D one, whose renders it
    // holds); purging the disk cache or the snapshot leaves it.
    if memory || what == "3d" {
        s.events.push(crate::Event::PurgeCaches);
    }
    s.toast(format!("Purged {what} cache"));
    Ok(json!({"purged": what, "diskEntries": disk_entries}))
}

/// `cache.diskStats`: the disk cache's folder, limit and contents.
pub(crate) fn disk_stats(s: &mut Session, _: &Value) -> Result<Value> {
    let Some(dc) = &s.disk_cache else {
        // The browser's disk cache (Origin Private File System), when the web app has one.
        let web = s.storage.as_ref().map(|h| h.info()["diskCache"].clone()).filter(|d| d.is_object());
        return Ok(web.unwrap_or_else(|| json!({"enabled": false})));
    };
    let st = dc.stats();
    Ok(json!({
        "enabled": true,
        "folder": dc.folder().to_string_lossy(),
        "entries": st.entries,
        "frames": dc.keys(effectcraft_render::disk_cache::Kind::Frame).len(),
        "layers": dc.keys(effectcraft_render::disk_cache::Kind::Layer).len(),
        "bytes": st.bytes,
        "maxBytes": st.max_bytes,
        "hits": st.hits,
        "misses": st.misses,
        "writes": st.writes,
        "evictions": st.evictions,
    }))
}

/// Path of the footage behind the selected layer or project item.
fn original_path(s: &Session) -> Option<String> {
    let comp = s.active_comp();
    let from_layer = s.state.selected_layers.first().and_then(|l| comp?.layer(*l)).and_then(|l| match l.source {
        LayerSource::Footage { item } => Some(item),
        _ => None,
    });
    let item = from_layer.or_else(|| s.state.project_selection.first().copied())?;
    match &s.project.item(item)?.kind {
        ItemKind::Footage(f) if !f.path.is_empty() => Some(f.path.clone()),
        _ => None,
    }
}

/// Whether Edit Original may hand `path` to the system's default app: a regular file (after
/// following links) whose name, and the name it links to, have a media extension the footage
/// importer takes. Anything else (a folder or app bundle, a script, a shortcut) is refused.
pub(crate) fn openable_original(path: &str) -> std::result::Result<(), String> {
    let media = |p: &std::path::Path| {
        let ext = p.extension().and_then(std::ffi::OsStr::to_str).map(str::to_ascii_lowercase).unwrap_or_default();
        crate::media_browser::kind_of(&p.to_string_lossy()).is_some_and(|k| k != "project") || matches!(ext.as_str(), "pdf" | "ai" | "eps")
    };
    let name = std::path::Path::new(path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.to_string());
    let real = std::fs::canonicalize(path).map_err(|e| format!("cannot open “{name}”: {e}"))?;
    if !real.is_file() {
        return Err(format!("“{name}” is not a file"));
    }
    if !media(std::path::Path::new(path)) || !media(&real) {
        return Err(format!("Edit Original opens media files only; “{name}” is not one"));
    }
    Ok(())
}

fn edit_original(s: &mut Session, _: &Value) -> Result<Value> {
    let stored = original_path(s).ok_or_else(|| super::bad("edit.editOriginal", "select a footage item or layer"))?;
    let path = s.stored_path(&stored).map_err(|why| super::bad("edit.editOriginal", crate::doc_path::refusal(&stored, why)))?;
    let path = path.to_string_lossy();
    openable_original(&path).map_err(|e| super::bad("edit.editOriginal", e))?;
    let url = super::layer_menu::file_url(&path);
    s.events.push(crate::Event::OpenUrl(url.clone()));
    Ok(json!({"url": url}))
}

fn purge(s: &mut Session, _: &Value) -> Result<Value> {
    s.history.clear();
    s.toast("Purged undo history");
    Ok(Value::Null)
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!("edit.undo", "Undo", ["Edit"], Some("Cmd+Z"), "{}", can_undo, undo),
        cmd!("edit.redo", "Redo", ["Edit"], Some("Cmd+Shift+Z"), "{}", can_redo, redo),
        cmd!("edit.cut", "Cut", ["Edit"], Some("Cmd+X"), "{layers?} (keyframes, effects or shape items when they are selected)", layers_or_keys, cut),
        cmd!("edit.copy", "Copy", ["Edit"], Some("Cmd+C"), "{layers?} (keyframes, effects or shape items when they are selected)", layers_or_keys, copy),
        cmd!(
            "edit.copyWithPropertyLinks",
            "Copy with Property Links",
            ["Edit"],
            Some("Cmd+Alt+C"),
            "{layers?} (selected properties, or layers, as expressions linking to the originals)",
            has_props_or_layers,
            |s, p| copy_links(s, p, false)
        ),
        cmd!(
            "edit.copyWithRelativePropertyLinks",
            "Copy with Relative Property Links",
            ["Edit"],
            None,
            "{layers?} (like Copy with Property Links, using thisComp)",
            has_props_or_layers,
            |s, p| copy_links(s, p, true)
        ),
        cmd!("edit.copyExpressionOnly", "Copy Expression Only", ["Edit"], None, "{}", has_props_or_layers, copy_expression_only),
        cmd!(
            "edit.paste",
            "Paste",
            ["Edit"],
            Some("Cmd+V"),
            "{} (layers, keyframes at the CTI, effects, shape items into the selected shape layers, or property links / expressions)",
            has_clip,
            paste
        ),
        cmd!(
            "edit.pasteSvg",
            "Paste SVG as Shapes",
            [],
            None,
            "{svg: SVG markup, name?, comp?} → {layer, skipped} (Edit ▸ Paste runs it for SVG on the system clipboard)",
            has_comp,
            paste_svg
        ),
        cmd!(
            "edit.pasteReversedKeyframes",
            "Paste Reversed Keyframes",
            ["Edit"],
            Some("Cmd+Shift+V"),
            "{layers?, prop?|path?, time?}",
            has_key_clip,
            paste_reversed
        ),
        cmd!("edit.clear", "Clear", ["Edit"], Some("Delete"), "{layers?}", layers_or_keys, delete),
        cmd!("edit.duplicate", "Duplicate", ["Edit"], Some("Cmd+D"), "{layers?} (effects or shape items when they are selected)", has_layers, duplicate),
        cmd!("edit.splitLayer", "Split Layer", ["Edit"], Some("Cmd+Shift+D"), "{layers?}", has_layers, split),
        cmd!("edit.liftWorkArea", "Lift Work Area", ["Edit"], None, "{layers?}", has_comp, lift),
        cmd!("edit.extractWorkArea", "Extract Work Area", ["Edit"], None, "{layers?}", has_comp, extract),
        cmd!("edit.selectAll", "Select All", ["Edit"], Some("Cmd+A"), "{}", has_comp, select_all),
        cmd!("edit.deselectAll", "Deselect All", ["Edit"], ["Cmd+Shift+A", "F2"], "{}", always_ok, deselect_all),
        cmd!("edit.label", "Label", ["Edit", "Label"], None, "{label: Red|Yellow|Aqua|…, layers?, keys?, target?: layers}", always_ok, label),
        cmd!("edit.selectLabelGroup", "Select Label Group", ["Edit", "Label"], None, "{}", has_props_or_layers_or_items, select_label_group),
        cmd!("edit.purgeUndo", "Undo", ["Edit", "Purge"], None, "{}", always_ok, purge),
        cmd!("edit.purge", "Purge", [], None, "{what?: all|memoryAndDisk|memory|disk|3d|image|snapshot}", always_ok, purge_caches),
        crate::query!("cache.diskStats", "Disk Cache Statistics", "{}", disk_stats),
        cmd!("edit.editOriginal", "Edit Original...", ["Edit"], Some("Cmd+E"), "{}", has_selected_footage, edit_original),
        crate::query!(
            "edit.history.list",
            "History",
            "{} → {states: [{index, id, label, parent, depth, current, line, future}], current, branches}",
            history_list
        ),
        cmd!(
            "edit.history.goto",
            "Go to History State",
            [],
            None,
            "{index? (from edit.history.list) | id? | steps? (negative = back along the line)}",
            always_ok,
            history_goto
        ),
    ]
}

// ---------------------------------------------------------------- History panel

/// `edit.history.list`: every state of the (branching) undo history.
fn history_list(s: &mut Session, _: &Value) -> Result<Value> {
    let nodes = s.history_tree();
    let current = nodes.iter().position(|n| n.current);
    Ok(json!({"states": nodes, "current": current, "branches": s.history.branches.len()}))
}

/// `edit.history.goto`: jump to a state by `index` (from `edit.history.list`), `id`, or
/// `steps` along the working line (negative = back, like Undo).
fn history_goto(s: &mut Session, p: &Value) -> Result<Value> {
    let c = "edit.history.goto";
    if let Some(n) = p.get("steps").and_then(Value::as_i64) {
        for _ in 0..n.unsigned_abs() {
            let moved = if n < 0 { s.undo() } else { s.redo() };
            if !moved {
                break;
            }
        }
    } else {
        let nodes = s.history_tree();
        let id = match (p.get("id").and_then(Value::as_str), p.get("index").and_then(Value::as_u64)) {
            (Some(id), _) => id.to_string(),
            (None, Some(i)) => nodes.get(i as usize).map(|n| n.id.clone()).ok_or_else(|| super::bad(c, format!("no state {i} (0..{})", nodes.len())))?,
            _ => return Err(super::bad(c, "give `index`, `id` or `steps`")),
        };
        if !s.goto_history(&id) {
            return Err(super::bad(c, format!("no history state `{id}`")));
        }
    }
    let nodes = s.history_tree();
    let cur = nodes.iter().find(|n| n.current).cloned();
    Ok(json!({"current": cur.as_ref().map(|n| n.index), "label": cur.map(|n| n.label)}))
}

fn layers_or_keys(s: &Session) -> std::result::Result<(), String> {
    if !s.state.selected_keys.is_empty() || !s.state.selected_vertices.is_empty() { super::has_comp(s) } else { has_layers(s) }
}

fn always_ok(_: &Session) -> std::result::Result<(), String> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    fn tmp(name: &str) -> std::path::PathBuf {
        let d = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-out").join(name);
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// Edit Original opens the footage as a well-formed `file://` URL, whatever its name contains.
    #[test]
    fn edit_original_opens_an_encoded_file_url() {
        let clip = tmp("edit-original-url").join("take #1.mov");
        std::fs::write(&clip, b"").unwrap();
        let clip = clip.to_string_lossy().into_owned();
        let url = super::super::layer_menu::file_url(&clip);
        assert!(url.ends_with("/take%20%231.mov"), "{url}");
        let mut s = crate::Session::default();
        let id = s
            .edit("add", None, |p, _| {
                let f = effectcraft_project::Footage { path: clip.clone(), ..Default::default() };
                Ok(p.add_item("take", effectcraft_color::Label::Lavender, None, effectcraft_project::ItemKind::Footage(f)))
            })
            .unwrap();
        s.state.project_selection = vec![id];
        s.drain_events();
        let r = s.execute("edit.editOriginal", json!({})).unwrap();
        assert_eq!(r["url"], url.as_str());
        assert!(s.drain_events().iter().any(|e| matches!(e, crate::Event::OpenUrl(u) if *u == url)));
    }

    /// Only regular media files reach the system opener.
    #[test]
    fn edit_original_opens_media_files_only() {
        let d = tmp("edit-original-kinds");
        for f in ["clip.mov", "Still.PNG", "run.bat", "app.desktop", "clip.mov.lnk"] {
            std::fs::write(d.join(f), b"").unwrap();
        }
        for f in ["Tool.app", "folder.mov"] {
            std::fs::create_dir_all(d.join(f)).unwrap();
        }
        let ok = |f: &str| super::openable_original(&d.join(f).to_string_lossy()).is_ok();
        assert!(ok("clip.mov") && ok("Still.PNG"));
        for f in ["run.bat", "app.desktop", "clip.mov.lnk", "Tool.app", "folder.mov", "missing.mov", ""] {
            assert!(!ok(f), "{f}");
        }
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(d.join("app.desktop"), d.join("link.png")).unwrap();
            assert!(!ok("link.png"), "a media name linking to another kind of file");
        }
        // The command refuses without opening anything.
        let mut s = crate::Session::default();
        let path = d.join("run.bat").to_string_lossy().into_owned();
        let id = s
            .edit("add", None, |p, _| {
                let f = effectcraft_project::Footage { path, ..Default::default() };
                Ok(p.add_item("bat", effectcraft_color::Label::Lavender, None, effectcraft_project::ItemKind::Footage(f)))
            })
            .unwrap();
        s.state.project_selection = vec![id];
        s.drain_events();
        assert!(s.execute("edit.editOriginal", json!({})).is_err());
        assert!(!s.drain_events().iter().any(|e| matches!(e, crate::Event::OpenUrl(_))));
    }
}

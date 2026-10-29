//! Essential Graphics (Window ▸ Essential Graphics) and master properties.
//!
//! - **Authoring**: a comp's controls ([`EssentialGraphics`]): Add Property to Essential
//!   Graphics, Media Replacement for footage layers, groups, comments, renaming, reordering, the
//!   template name and the panel's Primary composition.
//! - **Instances**: every precomp layer of such a comp shows the controls under **Essential
//!   Properties**; changing one overrides it for that instance only (`essential.set`, or any
//!   property edit of the master property); Push to Comp writes the override into the source
//!   comp, Revert drops it.
//! - **Templates**: `essential.exportTemplate` writes an open `.ectemplate` (a ZIP holding
//!   `manifest.json` with the exposed controls, `project.ecproj` with the comp and everything it
//!   uses, the media files and a poster frame); `essential.importTemplate` merges one into the
//!   project and adds an instance to the active comp.

use effectcraft_keyframe::Value as KV;
use effectcraft_project::essential::{self, ControlType, EgControl, EgKind, EssentialGraphics};
use effectcraft_project::{GroupKind, ItemId, ItemKind, LayerId, LayerSource, ParamUi, Project, Uid};
use effectcraft_time::Tick;
use serde_json::{Value, json};

use super::{CommandSpec, b_p, bad, comp_id, has_comp, layer_p, selected_leaf_props, str_p};
use crate::{EngineError, Result, Session, cmd, query};

/// Format id written into template manifests.
pub const TEMPLATE_FORMAT: &str = "effectcraft-template";
pub const TEMPLATE_VERSION: u32 = 1;

/// `control`: a control id, or its name in the comp's Essential Graphics (`"Title"`).
fn control_p(s: &Session, comp: ItemId, p: &Value, cmd: &str) -> Result<u64> {
    match p.get("control") {
        Some(Value::Number(n)) => n.as_u64().ok_or_else(|| bad(cmd, "`control` is a control id or name")),
        Some(Value::String(name)) => {
            let eg = s.project.comp(comp).and_then(|c| c.essential.as_ref());
            let all: Vec<(u64, String)> = eg.map(|e| e.flat().into_iter().map(|c| (c.id, c.name.clone())).collect()).unwrap_or_default();
            all.iter().find(|(_, n)| n == name).or_else(|| all.iter().find(|(_, n)| n.eq_ignore_ascii_case(name))).map(|(id, _)| *id).ok_or_else(|| {
                let names: Vec<&str> = all.iter().map(|(_, n)| n.as_str()).collect();
                bad(cmd, format!("no control named `{name}` (controls: {})", names.join(", ")))
            })
        }
        _ => Err(bad(cmd, "missing `control` (control id or name, see essential.list)")),
    }
}

/// Comp whose Essential Graphics a command edits: `comp`, else the panel's Primary comp, else
/// the active comp.
fn eg_comp(s: &Session, p: &Value) -> Result<ItemId> {
    if p.get("comp").is_some() {
        return comp_id(s, p);
    }
    if let Some(c) = s.state.essential_primary.filter(|c| s.project.comp(*c).is_some()) {
        return Ok(c);
    }
    comp_id(s, p)
}

fn eg_of(proj: &mut Project, cid: ItemId) -> Result<&mut EssentialGraphics> {
    let name = proj.item(cid).map(|i| i.name.clone()).unwrap_or_default();
    let c = proj.comp_mut(cid).ok_or(EngineError::NoComp)?;
    Ok(c.essential.get_or_insert_with(|| EssentialGraphics { name, controls: vec![] }))
}

/// Insert `ctl` into `group` (or the top level) at `index` (or the end).
fn insert(eg: &mut EssentialGraphics, ctl: EgControl, group: Option<u64>, index: Option<usize>, cmd: &str) -> Result<()> {
    let list = eg.list_mut(group).ok_or_else(|| bad(cmd, "`group` is not a group control"))?;
    let i = index.unwrap_or(list.len()).min(list.len());
    list.insert(i, ctl);
    Ok(())
}

fn set_primary(s: &mut Session, p: &Value) -> Result<Value> {
    let cid = comp_id(s, p)?;
    s.state.essential_primary = Some(cid);
    s.bump();
    Ok(json!({"comp": cid.0}))
}

fn set_name(s: &mut Session, p: &Value) -> Result<Value> {
    let cid = eg_comp(s, p)?;
    let name = str_p(p, "name").ok_or_else(|| bad("essential.setName", "missing `name`"))?.to_string();
    s.edit("Essential Graphics Name", None, |proj, st| {
        st.essential_primary.get_or_insert(cid);
        eg_of(proj, cid)?.name = name.clone();
        Ok(())
    })?;
    Ok(json!({"comp": cid.0, "name": name}))
}

/// Add Property to Essential Graphics: `{layer, path|prop}` or the selected properties.
fn add_property(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "essential.addProperty";
    let targets: Vec<(ItemId, LayerId, Uid)> = if p.get("path").is_some() || p.get("prop").is_some() {
        vec![super::prop::prop_ref(s, p, C)?]
    } else {
        let cid = comp_id(s, p)?;
        selected_leaf_props(s).into_iter().map(|(l, u)| (cid, l, u)).collect()
    };
    if targets.is_empty() {
        return Err(bad(C, "select a property in the timeline (or give `layer` and `path`)"));
    }
    let group = p.get("group").and_then(Value::as_u64);
    let index = p.get("index").and_then(Value::as_u64).map(|i| i as usize);
    let rename = str_p(p, "name").map(str::to_string);
    // `as`: show the property as another control (Source Text as `font`, Scale as `scale`).
    let as_type = match str_p(p, "as") {
        None => None,
        Some(a) => {
            let t = ControlType::parse(a).ok_or_else(|| bad(C, format!("as: font|scale|text|color|slider|checkbox|point|angle|dropdown, not `{a}`")))?;
            Some(t)
        }
    };
    // A property already in the panel (as the same kind of control) is added again as a mirror
    // (`mirror: false`: skipped).
    let mirror = b_p(p, "mirror").unwrap_or(true);
    let mut added = vec![];
    for (cid, lid, uid) in &targets {
        let l = s.project.comp(*cid).and_then(|c| c.layer(*lid)).ok_or(EngineError::NoComp)?;
        let pr = l.props.find(*uid).ok_or_else(|| bad(C, "no such property"))?;
        if control_type(pr).is_none() {
            return Err(bad(C, format!("`{}` can't be added to Essential Graphics (unsupported property type)", pr.name)));
        }
        // The natural control needs no `as`.
        let as_type = as_type.filter(|t| control_type(pr) != Some(*t));
        if let Some(t) = as_type
            && !essential::can_show_as(pr, t)
        {
            return Err(bad(C, format!("`{}` can't be shown as a {} control", pr.name, t.label())));
        }
        let eg = s.project.comp(*cid).and_then(|c| c.essential.as_ref());
        let existing = eg
            .and_then(|e| e.control_for_as(*lid, *uid, as_type).or_else(|| as_type.is_none().then(|| e.driver_of(*lid, *uid)).flatten()))
            .map(|c| (c.id, c.name.clone()));
        let (kind, as_type) = match existing {
            Some(_) if !mirror => continue,
            Some((of, _)) => (EgKind::Mirror { of }, None),
            None => (EgKind::Property { layer: *lid, prop: *uid, links: vec![] }, as_type),
        };
        let name = rename.clone().or(existing.map(|e| e.1)).unwrap_or_else(|| match as_type {
            Some(ControlType::Font) => format!("{} Font", l.name),
            _ => pr.name.clone(),
        });
        added.push((*cid, kind, name, as_type));
    }
    let ids = s.edit("Add Property to Essential Graphics", None, |proj, st| {
        if let Some((cid, ..)) = added.first() {
            st.essential_primary.get_or_insert(*cid);
        }
        let mut ids = vec![];
        for (k, (cid, kind, name, as_type)) in added.iter().enumerate() {
            let id = proj.alloc();
            let eg = eg_of(proj, *cid)?;
            let ctl = EgControl { id, name: name.clone(), kind: kind.clone(), as_type: *as_type };
            insert(eg, ctl, group, index.map(|i| i + k), C)?;
            ids.push(id);
        }
        Ok(ids)
    })?;
    let mirrors: Vec<u64> = ids.iter().zip(&added).filter(|(_, a)| matches!(a.1, EgKind::Mirror { .. })).map(|(i, _)| *i).collect();
    Ok(json!({"controls": ids, "mirrors": mirrors}))
}
/// Add a mirror of a property control (the same property shown again, e.g. in another group).
fn add_mirror(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "essential.addMirror";
    let cid = eg_comp(s, p)?;
    let of = control_p(s, cid, p, C)?;
    let eg = s.project.comp(cid).and_then(|c| c.essential.clone()).unwrap_or_default();
    let master =
        eg.resolve(of).filter(|c| matches!(c.kind, EgKind::Property { .. })).ok_or_else(|| bad(C, format!("control {of} is not a property control")))?;
    let (of, master_name) = (master.id, master.name.clone());
    let name = str_p(p, "name").map(str::to_string).unwrap_or(master_name);
    let group = p.get("group").and_then(Value::as_u64);
    let index = p.get("index").and_then(Value::as_u64).map(|i| i as usize);
    let id = s.edit("Add Mirror to Essential Graphics", None, |proj, _| {
        let id = proj.alloc();
        insert(eg_of(proj, cid)?, EgControl { id, name, kind: EgKind::Mirror { of }, as_type: None }, group, index, C)?;
        Ok(id)
    })?;
    Ok(json!({"control": id, "of": of}))
}

/// The property a link command names: `{layer, path|prop}` or the first selected property.
fn link_target(s: &Session, p: &Value, cmd: &str) -> Result<(ItemId, LayerId, Uid)> {
    if p.get("path").is_some() || p.get("prop").is_some() {
        return super::prop::prop_ref(s, p, cmd);
    }
    let cid = comp_id(s, p)?;
    selected_leaf_props(s)
        .into_iter()
        .next()
        .map(|(l, u)| (cid, l, u))
        .ok_or_else(|| bad(cmd, "select a property in the timeline (or give `layer` and `path`)"))
}

/// Make a property control drive another property too (same kind of value).
fn link_property(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "essential.linkProperty";
    let cid = eg_comp(s, p)?;
    let control = control_p(s, cid, p, C)?;
    let (pc, lid, uid) = link_target(s, p, C)?;
    if pc != cid {
        return Err(bad(C, "the property must be in the Essential Graphics composition"));
    }
    let eg = s.project.comp(cid).and_then(|c| c.essential.clone()).unwrap_or_default();
    let ctl = eg.resolve(control).ok_or_else(|| bad(C, format!("no control {control}")))?.clone();
    if ctl.as_type == Some(ControlType::Font) {
        return Err(bad(C, "a Font control can't drive other properties"));
    }
    let (main, _) = essential::source_prop(&s.project, cid, &ctl).ok_or_else(|| bad(C, format!("control {control} is not a property control")))?;
    let main = main.props.find(match ctl.kind {
        EgKind::Property { prop, .. } => prop,
        _ => 0,
    });
    let target = s.project.comp(cid).and_then(|c| c.layer(lid)).and_then(|l| l.props.find(uid)).ok_or_else(|| bad(C, "no such property"))?;
    if !main.is_some_and(|m| essential::linkable(m, target)) {
        return Err(bad(C, format!("`{}` is not the same kind of property as the control", target.name)));
    }
    if let Some(d) = eg.driver_of(lid, uid) {
        return Err(bad(C, format!("`{}` is already driven by control {}", target.name, d.id)));
    }
    let id = ctl.id;
    s.edit("Link Property to Essential Graphics Control", None, |proj, _| {
        if let Some(EgControl { kind: EgKind::Property { links, .. }, .. }) = eg_of(proj, cid)?.find_mut(id) {
            links.push(essential::EgLink { layer: lid, prop: uid });
        }
        Ok(())
    })?;
    Ok(json!({"control": id, "layer": lid.0, "prop": uid}))
}

fn unlink_property(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "essential.unlinkProperty";
    let cid = eg_comp(s, p)?;
    let control = control_p(s, cid, p, C)?;
    let (_, lid, uid) = link_target(s, p, C)?;
    let eg = s.project.comp(cid).and_then(|c| c.essential.clone()).unwrap_or_default();
    let id = eg.resolve(control).map(|c| c.id).ok_or_else(|| bad(C, format!("no control {control}")))?;
    s.edit("Unlink Property from Essential Graphics Control", None, |proj, _| match eg_of(proj, cid)?.find_mut(id) {
        Some(EgControl { kind: EgKind::Property { links, .. }, .. }) if links.iter().any(|k| k.layer == lid && k.prop == uid) => {
            links.retain(|k| !(k.layer == lid && k.prop == uid));
            Ok(())
        }
        _ => Err(bad(C, "the property is not linked to this control")),
    })?;
    Ok(json!({"control": id, "layer": lid.0, "prop": uid}))
}

fn control_type(pr: &effectcraft_project::Property) -> Option<ControlType> {
    essential::control_type(pr)
}

/// A Font control's value for `prop.set`: `cur` (a Source Text value) with the font family,
/// style and size from `v` (`{font?, style?, size?}` or a family name). `None` when `cur` is
/// not text.
pub fn font_value(cur: &KV, v: &Value) -> Option<Value> {
    let KV::Text(doc) = cur else { return None };
    let mut d = (**doc).clone();
    match v {
        Value::String(f) => d.font = f.clone(),
        Value::Object(o) => {
            if let Some(f) = o.get("font").and_then(Value::as_str) {
                d.font = f.to_string();
            }
            if let Some(st) = o.get("style").and_then(Value::as_str) {
                d.style = st.to_string();
            }
            if let Some(z) = o.get("size").and_then(Value::as_f64) {
                d.size = z.max(0.1);
            }
        }
        _ => return None,
    }
    serde_json::to_value(&d).ok()
}

/// A control's value for agents: Font controls show `{font, style, size}`, the rest the
/// property's value.
fn value_json(ty: Option<ControlType>, v: &KV) -> Value {
    match (ty, v) {
        (Some(ControlType::Font), KV::Text(d)) => json!({"font": d.font, "style": d.style, "size": d.size}),
        _ => v.to_json(),
    }
}

/// Media Replacement for a footage layer.
fn add_media(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "essential.addMedia";
    let (cid, lid) = layer_p(s, p, C)?;
    let l = s.project.comp(cid).and_then(|c| c.layer(lid)).ok_or(EngineError::NoComp)?;
    if !matches!(l.source, LayerSource::Footage { .. }) {
        return Err(bad(C, "Media Replacement needs a footage layer"));
    }
    let name = str_p(p, "name").map(str::to_string).unwrap_or_else(|| format!("{} (Media)", l.name));
    let group = p.get("group").and_then(Value::as_u64);
    let id = s.edit("Add Media Replacement", None, |proj, st| {
        st.essential_primary.get_or_insert(cid);
        let id = proj.alloc();
        let eg = eg_of(proj, cid)?;
        if eg.flat().iter().any(|c| matches!(c.kind, EgKind::Media { layer } if layer == lid)) {
            return Err(bad(C, "this layer already has a Media Replacement control"));
        }
        insert(eg, EgControl { id, name, kind: EgKind::Media { layer: lid }, as_type: None }, group, None, C)?;
        Ok(id)
    })?;
    Ok(json!({"control": id}))
}

fn add_group(s: &mut Session, p: &Value) -> Result<Value> {
    let cid = eg_comp(s, p)?;
    let name = str_p(p, "name").unwrap_or("Group").to_string();
    let index = p.get("index").and_then(Value::as_u64).map(|i| i as usize);
    let id = s.edit("Add Essential Graphics Group", None, |proj, st| {
        st.essential_primary.get_or_insert(cid);
        let id = proj.alloc();
        insert(eg_of(proj, cid)?, EgControl { id, name, kind: EgKind::Group { children: vec![] }, as_type: None }, None, index, "essential.addGroup")?;
        Ok(id)
    })?;
    Ok(json!({"control": id}))
}

fn add_comment(s: &mut Session, p: &Value) -> Result<Value> {
    let cid = eg_comp(s, p)?;
    let text = str_p(p, "text").unwrap_or("Comment").to_string();
    let group = p.get("group").and_then(Value::as_u64);
    let id = s.edit("Add Essential Graphics Comment", None, |proj, st| {
        st.essential_primary.get_or_insert(cid);
        let id = proj.alloc();
        insert(
            eg_of(proj, cid)?,
            EgControl { id, name: "Comment".into(), kind: EgKind::Comment { text }, as_type: None },
            group,
            None,
            "essential.addComment",
        )?;
        Ok(id)
    })?;
    Ok(json!({"control": id}))
}

fn rename(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "essential.rename";
    let cid = eg_comp(s, p)?;
    let id = control_p(s, cid, p, C)?;
    let name = str_p(p, "name").or(str_p(p, "text")).ok_or_else(|| bad(C, "missing `name`"))?.to_string();
    s.edit("Rename Essential Graphics Control", None, |proj, _| {
        let c = eg_of(proj, cid)?.find_mut(id).ok_or_else(|| bad(C, format!("no control {id}")))?;
        match &mut c.kind {
            // A comment's name is its text.
            EgKind::Comment { text } => *text = name.clone(),
            _ => c.name = name.clone(),
        }
        Ok(())
    })?;
    Ok(json!({"control": id, "name": name}))
}

fn remove(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "essential.remove";
    let cid = eg_comp(s, p)?;
    let id = control_p(s, cid, p, C)?;
    // A property control's mirrors go with it.
    let mirrors = s.project.comp(cid).and_then(|c| c.essential.as_ref()).map(|e| e.mirrors_of(id)).unwrap_or_default();
    s.edit("Remove Essential Graphics Control", None, |proj, _| {
        let eg = eg_of(proj, cid)?;
        eg.remove(id).ok_or_else(|| bad(C, format!("no control {id}")))?;
        for m in &mirrors {
            eg.remove(*m);
        }
        Ok(())
    })?;
    Ok(json!({"control": id, "mirrors": mirrors}))
}

fn move_control(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "essential.move";
    let cid = eg_comp(s, p)?;
    let id = control_p(s, cid, p, C)?;
    let group = p.get("group").and_then(Value::as_u64);
    let index = p.get("index").and_then(Value::as_u64).map(|i| i as usize);
    if group == Some(id) {
        return Err(bad(C, "a group can't contain itself"));
    }
    s.edit("Move Essential Graphics Control", None, |proj, _| {
        let eg = eg_of(proj, cid)?;
        let ctl = eg.remove(id).ok_or_else(|| bad(C, format!("no control {id}")))?;
        if matches!(ctl.kind, EgKind::Group { .. }) && group.is_some() {
            return Err(bad(C, "groups can't be nested"));
        }
        insert(eg, ctl, group, index, C)
    })?;
    Ok(json!({"control": id}))
}

/// Dropdown Menu Control ▸ Edit: the menu items of a dropdown (popup) property.
fn edit_dropdown(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "effect.editDropdown";
    let items: Vec<String> = match p.get("items") {
        Some(Value::Array(a)) => a.iter().filter_map(|v| v.as_str().map(str::to_string)).filter(|x| !x.is_empty()).collect(),
        _ => return Err(bad(C, "missing `items` (array of strings)")),
    };
    if items.is_empty() {
        return Err(bad(C, "a dropdown needs at least one item"));
    }
    let q = if p.get("path").is_none() && p.get("prop").is_none() {
        // The first Dropdown Menu Control of the layer.
        let (cid, lid) = layer_p(s, p, C)?;
        let l = s.project.comp(cid).and_then(|c| c.layer(lid)).ok_or(EngineError::NoComp)?;
        let fx = l.effects().and_then(|fx| fx.groups().find(|g| matches!(&g.kind, GroupKind::Effect { effect } if effect == "ec.control.dropdown")));
        let uid = fx.and_then(|g| g.get("menu")).map(|pr| pr.uid).ok_or_else(|| bad(C, "the layer has no Dropdown Menu Control"))?;
        let mut q = p.clone();
        q["prop"] = json!(uid);
        q["layer"] = json!(lid.0);
        q
    } else {
        p.clone()
    };
    let (cid, lid, uid) = super::prop::prop_ref(s, &q, C)?;
    let n = items.len() as u32;
    s.edit("Edit Dropdown", None, |proj, _| {
        let pr = super::layer_mut(proj, cid, lid)?.props.find_mut(uid).ok_or_else(|| bad(C, "no such property"))?;
        if !matches!(pr.ui, ParamUi::Popup { .. }) {
            return Err(bad(C, "not a dropdown property"));
        }
        pr.ui = ParamUi::Popup { options: items.clone() };
        let clamp = |v: &mut KV| {
            if let KV::Enum(i) = v {
                *i = (*i).min(n - 1);
            }
        };
        clamp(&mut pr.value);
        for k in &mut pr.keys {
            clamp(&mut k.value);
        }
        Ok(())
    })?;
    Ok(json!({"prop": uid, "items": items}))
}

/// Composition ▸ Open in Essential Graphics.
fn open_in_eg(s: &mut Session, p: &Value) -> Result<Value> {
    let cid = comp_id(s, p)?;
    s.state.essential_primary = Some(cid);
    s.bump();
    s.events.push(crate::Event::Frontend { command: "window.panel".into(), params: json!({"panel": "essentialGraphics"}) });
    let name = s.project.item(cid).map(|i| i.name.clone()).unwrap_or_default();
    Ok(json!({"comp": cid.0, "name": name}))
}

/// Whether a property can be exposed (`property.canAddToMotionGraphicsTemplate`).
fn can_add(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "essential.canAdd";
    let (cid, lid, uid) = super::prop::prop_ref(s, p, C)?;
    let l = s.project.comp(cid).and_then(|c| c.layer(lid)).ok_or(EngineError::NoComp)?;
    let pr = l.props.find(uid).ok_or_else(|| bad(C, "no such property"))?;
    let as_type = str_p(p, "as").and_then(ControlType::parse);
    let ty = match as_type {
        Some(t) if essential::can_show_as(pr, t) => Some(t),
        Some(t) => return Ok(json!({"ok": false, "reason": format!("`{}` can't be shown as a {} control", pr.name, t.label())})),
        None => control_type(pr),
    };
    let in_eg = super::match_path_of(&l.props, uid).is_some_and(|m| m.starts_with(essential::GROUP));
    Ok(match ty {
        Some(t) if !in_eg => json!({"ok": true, "type": t}),
        _ => json!({"ok": false, "reason": format!("`{}` can't be added to Essential Graphics (unsupported property type)", pr.name)}),
    })
}

fn solo_supported(s: &mut Session, p: &Value) -> Result<Value> {
    let on = b_p(p, "on").unwrap_or(!s.state.essential_solo);
    s.state.essential_solo = on;
    s.bump();
    Ok(json!({"on": on}))
}

/// Properties of a comp's layers that Essential Graphics can expose (Solo Supported
/// Properties): `[{layer, uid, path, name, type}]`.
pub fn supported_properties(project: &Project, cid: ItemId) -> Vec<Value> {
    let Some(c) = project.comp(cid) else { return vec![] };
    let mut out = vec![];
    for l in &c.layers {
        l.props.walk("", &mut |_, pr| {
            if pr.three_d_only && !l.is_3d() {
                return;
            }
            if let Some(t) = essential::control_type(pr) {
                let path = super::match_path_of(&l.props, pr.uid).unwrap_or_default();
                if path.starts_with(essential::GROUP) {
                    return;
                }
                out.push(json!({"layer": l.id.0, "layerName": l.name, "uid": pr.uid, "path": path, "name": l.props.name_path_of(pr.uid), "type": t}));
            }
        });
    }
    out
}

fn control_json(project: &Project, cid: ItemId, c: &EgControl) -> Value {
    match &c.kind {
        EgKind::Property { layer, prop, .. } => {
            let src = essential::source_prop(project, cid, c);
            let links: Vec<Value> = essential::linked_props(project, cid, c)
                .into_iter()
                .map(|(l, pr)| json!({"layer": l.id.0, "layerName": l.name, "prop": pr.uid, "path": super::match_path_of(&l.props, pr.uid)}))
                .collect();
            json!({
                "id": c.id, "name": c.name, "kind": "property", "layer": layer.0, "prop": prop,
                "layerName": src.map(|(l, _)| l.name.clone()),
                "path": src.and_then(|(l, _)| super::match_path_of(&l.props, *prop)),
                "type": src.and_then(|(_, pr)| essential::effective_type(c, pr)),
                "value": src.map(|(_, pr)| value_json(essential::effective_type(c, pr), &pr.value_at(Tick::ZERO))),
                "missing": src.is_none(),
                "links": links,
                "mirrors": project.comp(cid).and_then(|c| c.essential.as_ref()).map(|e| e.mirrors_of(c.id)).unwrap_or_default(),
            })
        }
        EgKind::Mirror { of } => {
            // Shown as its master (same kind of control).
            let master = project.comp(cid).and_then(|x| x.essential.as_ref()).and_then(|e| e.resolve(c.id)).cloned();
            let src = essential::source_prop(project, cid, c);
            let ty = |pr| master.as_ref().and_then(|m| essential::effective_type(m, pr));
            json!({
                "id": c.id, "name": c.name, "kind": "mirror", "of": of,
                "layer": src.map(|(l, _)| l.id.0),
                "prop": src.map(|(_, pr)| pr.uid),
                "layerName": src.map(|(l, _)| l.name.clone()),
                "path": src.and_then(|(l, pr)| super::match_path_of(&l.props, pr.uid)),
                "type": src.and_then(|(_, pr)| ty(pr)),
                "value": src.map(|(_, pr)| value_json(ty(pr), &pr.value_at(Tick::ZERO))),
                "missing": src.is_none(),
            })
        }
        EgKind::Media { layer } => json!({
            "id": c.id, "name": c.name, "kind": "media", "type": "media", "layer": layer.0,
            "item": essential::source_media(project, cid, c).map(|i| i.0),
        }),
        EgKind::Comment { text } => json!({"id": c.id, "name": c.name, "kind": "comment", "text": text}),
        EgKind::Group { children } => json!({
            "id": c.id, "name": c.name, "kind": "group",
            "children": children.iter().map(|x| control_json(project, cid, x)).collect::<Vec<_>>(),
        }),
    }
}

fn list(s: &mut Session, p: &Value) -> Result<Value> {
    let cid = eg_comp(s, p)?;
    let eg = s.project.comp(cid).and_then(|c| c.essential.clone()).unwrap_or_default();
    let name = if eg.name.is_empty() { s.project.item(cid).map(|i| i.name.clone()).unwrap_or_default() } else { eg.name.clone() };
    Ok(json!({
        "comp": cid.0,
        "primary": s.state.essential_primary.map(|c| c.0),
        "name": name,
        "solo": s.state.essential_solo,
        "controls": eg.controls.iter().map(|c| control_json(&s.project, cid, c)).collect::<Vec<_>>(),
        "supported": if b_p(p, "supported").unwrap_or(false) { json!(supported_properties(&s.project, cid)) } else { Value::Null },
    }))
}

// ------------------------------------------------------------------------------ instances

/// The precomp layer (an instance) and its source comp.
fn instance(s: &Session, p: &Value, cmd: &str) -> Result<(ItemId, LayerId, ItemId)> {
    let (cid, lid) = layer_p(s, p, cmd)?;
    let l = s.project.comp(cid).and_then(|c| c.layer(lid)).ok_or(EngineError::NoComp)?;
    match l.source {
        LayerSource::Comp { item } if essential::group(l).is_some() => Ok((cid, lid, item)),
        _ => Err(bad(cmd, "the layer is not a precomp with Essential Properties")),
    }
}

/// The master property (instance child) of a control.
fn master_uid(s: &Session, cid: ItemId, lid: LayerId, control: u64) -> Option<Uid> {
    let l = s.project.comp(cid)?.layer(lid)?;
    let g = essential::group(l)?;
    let m = essential::match_id(control);
    let mut found = None;
    g.walk("", &mut |_, pr| {
        if pr.match_id == m {
            found = Some(pr.uid);
        }
    });
    found
}

fn instance_json(s: &mut Session, p: &Value) -> Result<Value> {
    let (cid, lid, src) = instance(s, p, "essential.instance")?;
    let l = s.project.comp(cid).and_then(|c| c.layer(lid)).ok_or(EngineError::NoComp)?;
    let over = essential::overridden(l);
    let eg = s.project.comp(src).and_then(|c| c.essential.clone()).unwrap_or_default();
    let t = s.time();
    let g = essential::group(l).cloned();
    let mut out = vec![];
    for c in eg.flat() {
        let m = essential::match_id(c.id);
        let mut pr = None;
        if let Some(g) = &g {
            g.walk("", &mut |_, x| {
                if x.match_id == m {
                    pr = Some(x.clone());
                }
            });
        }
        let kind = match c.kind {
            EgKind::Property { .. } => "property",
            EgKind::Mirror { .. } => "mirror",
            EgKind::Media { .. } => "media",
            EgKind::Comment { .. } => "comment",
            EgKind::Group { .. } => "group",
        };
        out.push(json!({
            "control": c.id, "name": c.name, "kind": kind,
            "uid": pr.as_ref().map(|x| x.uid),
            "path": pr.as_ref().and_then(|x| super::match_path_of(&l.props, x.uid)),
            "value": pr.as_ref().map(|x| value_json(essential::effective_type(c, x), &x.value_at(l.layer_time(t)))),
            "type": pr.as_ref().and_then(|x| essential::effective_type(c, x)).or((kind == "media").then_some(ControlType::Media)),
            "overridden": pr.as_ref().is_some_and(|x| over.contains(&x.uid)),
        }));
    }
    Ok(json!({"layer": lid.0, "comp": src.0, "name": eg.name, "controls": out}))
}

/// Set an instance's value for a control (`value`, or `item` for Media Replacement).
fn set_override(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "essential.set";
    let (cid, lid, src) = instance(s, p, C)?;
    let control = control_p(s, src, p, C)?;
    let uid = master_uid(s, cid, lid, control).ok_or_else(|| bad(C, format!("no control {control} on this instance")))?;
    if let Some(item) = p.get("item") {
        let item = match item {
            Value::Number(n) => ItemId(n.as_u64().unwrap_or(0)),
            Value::String(name) => s.project.find_by_name(name).map(|i| i.id).ok_or_else(|| bad(C, format!("no item `{name}`")))?,
            _ => return Err(bad(C, "`item` is an item id or name")),
        };
        if !matches!(s.project.item(item).map(|i| &i.kind), Some(ItemKind::Footage(f)) if f.has_video) {
            return Err(bad(C, "replacement media must be footage with video"));
        }
        s.edit("Replace Media", None, |proj, _| {
            let pr = super::layer_mut(proj, cid, lid)?.props.find_mut(uid).ok_or_else(|| bad(C, "no such property"))?;
            pr.value = KV::Scalar(item.0 as f64);
            Ok(())
        })?;
        return Ok(json!({"control": control, "item": item.0}));
    }
    let mut value = p.get("value").cloned().ok_or_else(|| bad(C, "missing `value` (or `item`)"))?;
    let as_type = s.project.comp(src).and_then(|c| c.essential.as_ref()).and_then(|e| e.find(control)).and_then(|c| c.as_type);
    if as_type == Some(ControlType::Font) {
        let cur = s.project.comp(cid).and_then(|c| c.layer(lid)).and_then(|l| l.props.find(uid)).map(|pr| pr.value.clone()).unwrap_or(KV::Scalar(0.0));
        value = font_value(&cur, &value).ok_or_else(|| bad(C, "a Font control takes {font?, style?, size?} or a font family name"))?;
    }
    let mut q = json!({"comp": cid.0, "layer": lid.0, "prop": uid, "value": value});
    if let Some(m) = p.get("merge") {
        q["merge"] = m.clone();
    }
    s.execute("prop.set", q)
}

/// Controls of an instance to act on: `control` or every overridden one.
fn targets(s: &Session, cid: ItemId, lid: LayerId, p: &Value) -> Vec<(u64, Uid)> {
    let Some(l) = s.project.comp(cid).and_then(|c| c.layer(lid)) else { return vec![] };
    let over = essential::overridden(l);
    let Some(g) = essential::group(l) else { return vec![] };
    // A control, its master and the master's mirrors share one value: act on all of them.
    let want: Option<Vec<u64>> = p.get("control").and_then(Value::as_u64).map(|w| {
        let src = match l.source {
            LayerSource::Comp { item } => s.project.comp(item).and_then(|c| c.essential.as_ref()),
            _ => None,
        };
        match src.and_then(|e| e.resolve(w).map(|m| (e, m.id))) {
            Some((e, m)) => std::iter::once(m).chain(e.mirrors_of(m)).chain(std::iter::once(w)).collect(),
            None => vec![w],
        }
    });
    let mut out = vec![];
    g.walk("", &mut |_, pr| {
        if let Some(c) = essential::control_of(&pr.match_id)
            && want.as_ref().is_none_or(|w| w.contains(&c))
            && (want.is_some() || over.contains(&pr.uid))
        {
            out.push((c, pr.uid));
        }
    });
    out
}

fn revert(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "essential.revert";
    let (cid, lid, _) = instance(s, p, C)?;
    let ts = targets(s, cid, lid, p);
    if ts.is_empty() {
        return Ok(json!({"reverted": []}));
    }
    s.edit("Revert Essential Property", None, |proj, _| {
        let l = super::layer_mut(proj, cid, lid)?;
        let g = l.props.sub_mut(essential::GROUP).ok_or_else(|| bad(C, "no Essential Properties"))?;
        if let GroupKind::Essential { overridden } = &mut g.kind {
            overridden.retain(|u| !ts.iter().any(|(_, x)| x == u));
        }
        for (_, uid) in &ts {
            if let Some(pr) = g.find_mut(*uid) {
                // The sync below brings back the source value.
                pr.keys.clear();
                pr.expr = None;
            }
        }
        Ok(())
    })?;
    // Edits that only clear an override must not be taken for a new override: values are
    // refreshed by the sync inside `edit`, which ran after the override list was trimmed.
    Ok(json!({"reverted": ts.iter().map(|(c, _)| c).collect::<Vec<_>>()}))
}

/// Push to Comp: write the instance's override values into the source comp's properties (and
/// replaced media into the source layer), then drop the overrides.
fn push_to_comp(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "essential.pushToComp";
    let (cid, lid, src) = instance(s, p, C)?;
    let ts = targets(s, cid, lid, p);
    let eg = s.project.comp(src).and_then(|c| c.essential.clone()).unwrap_or_default();
    let l = s.project.comp(cid).and_then(|c| c.layer(lid)).cloned().ok_or(EngineError::NoComp)?;
    let mut writes = vec![];
    for (control, uid) in &ts {
        // A mirror pushes into its master's properties.
        let (Some(ctl), Some(pr)) = (eg.resolve(*control), l.props.find(*uid)) else { continue };
        writes.push((ctl.kind.clone(), pr.clone(), ctl.as_type));
    }
    s.edit("Push Override Values to Source", None, |proj, _| {
        let sc = proj.comp_mut(src).ok_or(EngineError::NoComp)?;
        for (kind, pr, as_type) in &writes {
            match kind {
                // A Font control writes only the font (the source keeps its text and keys).
                EgKind::Property { layer, prop, .. } if *as_type == Some(ControlType::Font) => {
                    if let Some(target) = sc.layer_mut(*layer).and_then(|l| l.props.find_mut(*prop)) {
                        essential::merge_font(&mut target.value, &pr.value);
                        for k in &mut target.keys {
                            essential::merge_font(&mut k.value, &pr.value);
                        }
                    }
                }
                EgKind::Property { .. } => {
                    for (layer, prop) in kind.targets() {
                        let Some(inner) = sc.layer_mut(layer) else { continue };
                        // Instance keys are in the precomp layer's time (= the source comp's
                        // time); the source property's keys are in the inner layer's time.
                        let keys: Vec<_> = pr
                            .keys
                            .iter()
                            .map(|k| {
                                let mut k = k.clone();
                                k.time = inner.layer_time(k.time);
                                k
                            })
                            .collect();
                        if let Some(target) = inner.props.find_mut(prop) {
                            target.value = pr.value.clone();
                            target.keys = keys;
                        }
                    }
                }

                EgKind::Media { layer } => {
                    if let Some(inner) = sc.layer_mut(*layer) {
                        inner.source = LayerSource::Footage { item: ItemId(pr.value.as_f64().max(0.0) as u64) };
                    }
                }
                _ => {}
            }
        }
        let g = super::layer_mut(proj, cid, lid)?.props.sub_mut(essential::GROUP).ok_or_else(|| bad(C, "no Essential Properties"))?;
        if let GroupKind::Essential { overridden } = &mut g.kind {
            overridden.retain(|u| !ts.iter().any(|(_, x)| x == u));
        }
        for (_, uid) in &ts {
            if let Some(pr) = g.find_mut(*uid) {
                pr.keys.clear();
                pr.expr = None;
            }
        }
        Ok(())
    })?;
    Ok(json!({"pushed": ts.iter().map(|(c, _)| c).collect::<Vec<_>>()}))
}

// ------------------------------------------------------------------------------ templates

fn file_name(path: &str) -> String {
    std::path::Path::new(path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| path.to_string())
}

/// The manifest's control list (flattened, with the group each belongs to).
fn manifest_controls(project: &Project, cid: ItemId, eg: &EssentialGraphics) -> Vec<Value> {
    fn go(project: &Project, cid: ItemId, v: &[EgControl], group: Option<u64>, out: &mut Vec<Value>) {
        for c in v {
            let mut j = control_json(project, cid, c);
            if let Some(o) = j.as_object_mut() {
                o.remove("children");
                o.insert("group".into(), json!(group));
            }
            out.push(j);
            if let EgKind::Group { children } = &c.kind {
                go(project, cid, children, Some(c.id), out);
            }
        }
    }
    let mut out = vec![];
    go(project, cid, &eg.controls, None, &mut out);
    out
}

fn export_template(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "essential.exportTemplate";
    let cid = eg_comp(s, p)?;
    let path = str_p(p, "path").ok_or_else(|| bad(C, "missing `path` (.ectemplate)"))?.to_string();
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?.clone();
    let eg = comp.essential.clone().filter(|e| !e.controls.is_empty()).ok_or_else(|| bad(C, "the composition has no Essential Graphics controls"))?;
    let comp_name = s.project.item(cid).map(|i| i.name.clone()).unwrap_or_default();
    let name = str_p(p, "name").map(str::to_string).unwrap_or_else(|| if eg.name.is_empty() { comp_name.clone() } else { eg.name.clone() });
    // The project subset: the comp and everything it uses, at the root.
    let deps = essential::dependencies(&s.project, cid);
    let mut sub = Project { settings: s.project.settings.clone(), next_id: s.project.next_id, ..Project::default() };
    let mut entries: Vec<(String, Vec<u8>)> = vec![];
    let mut media = vec![];
    for id in &deps {
        let Some(mut it) = s.project.item(*id).cloned() else { continue };
        it.parent = None;
        it.proxy = None;
        if let ItemKind::Footage(f) = &mut it.kind
            && f.data.is_none()
        {
            let files: Vec<String> = if f.sequence.is_empty() { vec![f.path.clone()] } else { f.sequence.clone() };
            let mut stored = vec![];
            for (k, file) in files.iter().enumerate() {
                if let Ok(bytes) = s.services.read_file(file) {
                    let entry = format!("media/{}/{}", id.0, file_name(file));
                    entries.push((entry.clone(), bytes));
                    if k == 0 {
                        f.path = entry.clone();
                    }
                    stored.push(entry);
                }
            }
            if !stored.is_empty() {
                if !f.sequence.is_empty() {
                    f.sequence = stored.clone();
                }
                media.push(json!({"item": id.0, "files": stored}));
            }
        }
        sub.items.insert(*id, it);
    }
    let manifest = json!({
        "format": TEMPLATE_FORMAT,
        "version": TEMPLATE_VERSION,
        "generator": format!("EffectCraft {}", env!("CARGO_PKG_VERSION")),
        "name": name,
        "comp": cid.0,
        "compName": comp_name,
        "width": comp.width,
        "height": comp.height,
        "frameRate": comp.frame_rate.as_f64(),
        "duration": comp.duration.seconds(),
        "controls": manifest_controls(&s.project, cid, &eg),
        "media": media,
        "poster": "poster.png",
    });
    // A poster frame (longest side ≤ 320 px).
    if let Ok((w, h, rgba)) = s.render_rgba8(cid, comp.poster_time, 320)
        && let Ok(png) = super::comp_more::encode_png(&rgba, w, h)
    {
        entries.push(("poster.png".into(), png));
    }
    entries.insert(0, ("project.ecproj".into(), sub.to_json().into_bytes()));
    entries.insert(0, ("manifest.json".into(), serde_json::to_vec_pretty(&manifest).unwrap_or_default()));
    let bytes = effectcraft_lottie::zip::store(&entries);
    s.services.write_file(&path, &bytes).map_err(|e| EngineError::Other(format!("cannot write {path}: {e}")))?;
    s.toast(format!("Exported template “{name}” to {path}"));
    Ok(json!({"path": path, "name": name, "bytes": bytes.len(), "controls": eg.flat().len(), "items": deps.len()}))
}

/// Read a template's manifest and project (with media paths still archive-relative).
pub fn read_template(bytes: &[u8]) -> std::result::Result<(Value, Project, Vec<(String, Vec<u8>)>), String> {
    let entries = effectcraft_lottie::zip::read_stored(bytes);
    let get = |n: &str| entries.iter().find(|(k, _)| k == n).map(|(_, v)| v.clone());
    let manifest: Value =
        serde_json::from_slice(&get("manifest.json").ok_or("not an EffectCraft template (no manifest.json)")?).map_err(|e| format!("manifest.json: {e}"))?;
    if manifest.get("format").and_then(Value::as_str) != Some(TEMPLATE_FORMAT) {
        return Err("not an EffectCraft template".into());
    }
    if manifest.get("version").and_then(Value::as_u64).unwrap_or(0) > TEMPLATE_VERSION as u64 {
        return Err("the template was made by a newer EffectCraft".into());
    }
    let text = String::from_utf8(get("project.ecproj").ok_or("template has no project.ecproj")?).map_err(|e| e.to_string())?;
    let mut project = Project::from_json(&text).map_err(|e| e.to_string())?;
    crate::upgrade_effects(&mut project);
    Ok((manifest, project, entries))
}

/// Where a template's `media/<item>/<file>` entry is written: `<media_dir>/<item>/<file>`.
/// `None` unless `<item>` is all digits and `<file>` is a plain file name (not empty, `.` or
/// `..`, no separators, and on Windows no drive, stream or device name), so every entry lands
/// in an item folder directly inside `media_dir`.
fn template_media_path(media_dir: &std::path::Path, entry: &str) -> Option<std::path::PathBuf> {
    let (item, file) = entry.strip_prefix("media/")?.split_once('/')?;
    if item.is_empty() || !item.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if !super::scripts::is_bare_name(file) || (cfg!(windows) && super::scripts::windows_unsafe_name(file)) {
        return None;
    }
    let out = media_dir.join(item).join(file);
    (out.parent().and_then(std::path::Path::parent) == Some(media_dir) && out.file_name() == Some(std::ffi::OsStr::new(file))).then_some(out)
}

fn import_template(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "essential.importTemplate";
    let path = str_p(p, "path").ok_or_else(|| bad(C, "missing `path` (.ectemplate)"))?.to_string();
    let bytes = s.services.read_file(&path).map_err(|e| EngineError::Other(format!("cannot read {path}: {e}")))?;
    let (manifest, mut sub, entries) = read_template(&bytes).map_err(|e| bad(C, e))?;
    let name = manifest.get("name").and_then(Value::as_str).unwrap_or("Template").to_string();
    let tcomp = manifest.get("comp").and_then(Value::as_u64).map(ItemId).ok_or_else(|| bad(C, "manifest has no `comp`"))?;
    // Media next to the template: `<dir>/<stem> Media/<item>/<file>`.
    let pp = std::path::Path::new(&path);
    let dir = pp.parent().map(|d| d.to_path_buf()).unwrap_or_default();
    let stem = pp.file_stem().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "Template".into());
    let media_dir = dir.join(format!("{stem} Media"));
    let mut written = std::collections::HashMap::new();
    for (k, v) in entries.iter().filter(|(k, _)| k.starts_with("media/")) {
        let Some(out) = template_media_path(&media_dir, k) else { continue };
        if let Some(parent) = out.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let out_s = out.to_string_lossy().to_string();
        if s.services.write_file(&out_s, v).is_ok() {
            written.insert(k.clone(), out_s);
        }
    }
    for it in sub.items.values_mut() {
        if let ItemKind::Footage(f) = &mut it.kind {
            if let Some(w) = written.get(&f.path) {
                f.path = w.clone();
            }
            for x in &mut f.sequence {
                if let Some(w) = written.get(x) {
                    *x = w.clone();
                }
            }
        }
    }
    let target = if b_p(p, "addToComp") == Some(false) { None } else { comp_id(s, p).ok() };
    let base = s.project.next_id;
    essential::offset_ids(&mut sub, base);
    let new_comp = ItemId(tcomp.0 + base);
    let ids = s.edit("Import Template", None, |proj, st| {
        let folder = proj.add_item(&name, effectcraft_color::Label::Yellow, None, ItemKind::Folder);
        let mut ids = vec![];
        for (id, mut it) in std::mem::take(&mut sub.items) {
            if it.parent.is_none() {
                it.parent = Some(folder);
            }
            ids.push(id.0);
            proj.items.insert(id, it);
        }
        proj.next_id = proj.next_id.max(sub.next_id);
        proj.fix_next_id();
        st.project_selection = vec![new_comp];
        Ok(ids)
    })?;
    let mut layer = Value::Null;
    if let Some(cid) = target
        && s.project.comp(cid).is_some()
    {
        let r = s.execute("layer.addItem", json!({"comp": cid.0, "item": new_comp.0, "time": 0.0}))?;
        layer = r["layer"].clone();
    }
    s.toast(format!("Imported template “{name}”"));
    Ok(json!({"comp": new_comp.0, "items": ids, "layer": layer, "name": name}))
}

/// Read a template's manifest without importing it (Essential Graphics / agents).
fn template_info(s: &mut Session, p: &Value) -> Result<Value> {
    let path = str_p(p, "path").ok_or_else(|| bad("essential.templateInfo", "missing `path`"))?;
    let bytes = s.services.read_file(path).map_err(|e| EngineError::Other(format!("cannot read {path}: {e}")))?;
    let (manifest, _, _) = read_template(&bytes).map_err(|e| bad("essential.templateInfo", e))?;
    Ok(manifest)
}

fn has_instance(s: &Session) -> std::result::Result<(), String> {
    has_comp(s)?;
    let ok = s.active_comp().is_some_and(|c| s.state.selected_layers.iter().any(|l| c.layer(*l).is_some_and(|l| essential::group(l).is_some())));
    if ok { Ok(()) } else { Err("select a precomp layer with Essential Properties".into()) }
}

fn has_eg(s: &Session) -> std::result::Result<(), String> {
    let cid = s.state.essential_primary.or(s.state.active_comp).ok_or("no composition is open")?;
    if s.project.comp(cid).and_then(|c| c.essential.as_ref()).is_some_and(|e| !e.controls.is_empty()) {
        Ok(())
    } else {
        Err("add properties to Essential Graphics first".into())
    }
}

fn has_selected_props(s: &Session) -> std::result::Result<(), String> {
    has_comp(s)?;
    if s.state.selected_props.is_empty() { Err("select a property in the timeline".into()) } else { Ok(()) }
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!("essential.setPrimary", "Primary Composition", [], None, "{comp}", has_comp, set_primary),
        cmd!("essential.setName", "Essential Graphics Name", [], None, "{comp?, name}", has_comp, set_name),
        cmd!(
            "essential.addProperty",
            "Add Property to Essential Graphics",
            ["Animation"],
            None,
            "{comp?, layer?, path?|prop?: uid (default: the selected properties), name?, group?: control id, index?, as?: font (Source Text font family/style/size) | scale (uniform Scale), mirror?: bool (a property already present is added as a mirror; default true)}",
            has_selected_props,
            add_property
        ),
        cmd!("essential.addMirror", "Add Mirror", [], None, "{comp?, control, name?, group?, index?}", has_eg, add_mirror),
        cmd!(
            "essential.linkProperty",
            "Link Property to Control",
            [],
            None,
            "{comp?, control, layer?, path?|prop? (default: the selected property)}",
            has_eg,
            link_property
        ),
        cmd!(
            "essential.unlinkProperty",
            "Unlink Property",
            [],
            None,
            "{comp?, control, layer?, path?|prop? (default: the selected property)}",
            has_eg,
            unlink_property
        ),
        cmd!("essential.addMedia", "Add Media Replacement", [], None, "{layer, name?, group?}", has_comp, add_media),
        cmd!("essential.addGroup", "Add Group", [], None, "{comp?, name?, index?}", has_comp, add_group),
        cmd!("essential.addComment", "Add Comment", [], None, "{comp?, text?, group?}", has_comp, add_comment),
        cmd!("essential.rename", "Rename Control", [], None, "{comp?, control, name}", has_comp, rename),
        cmd!("essential.remove", "Remove Control", [], None, "{comp?, control}", has_comp, remove),
        cmd!("essential.move", "Move Control", [], None, "{comp?, control, group?: id (null = top level), index?}", has_comp, move_control),
        cmd!("essential.soloSupported", "Solo Supported Properties", [], None, "{on?}", has_comp, solo_supported),
        cmd!(
            "essential.exportTemplate",
            "Essential Graphics Template...",
            ["File", "Export"],
            None,
            "{comp?, path (.ectemplate), name?}",
            has_eg,
            export_template
        ),
        cmd!(
            "essential.importTemplate",
            "Essential Graphics Template...",
            ["File", "Import"],
            None,
            "{path (.ectemplate), comp?, addToComp?: bool}",
            super::always,
            import_template
        ),
        cmd!("essential.set", "Set Essential Property", [], None, "{layer, control: id|name, value | item (media)}", has_instance, set_override),
        cmd!("essential.pushToComp", "Push Override Values to Source", [], None, "{layer, control? (default: all overridden)}", has_instance, push_to_comp),
        cmd!("essential.revert", "Revert", [], None, "{layer, control? (default: all overridden)}", has_instance, revert),
        cmd!("effect.editDropdown", "Edit Dropdown Menu", [], None, "{layer, path?|prop?, items: [string]}", has_comp, edit_dropdown),
        query!("essential.list", "Essential Graphics controls", "{comp?, supported?: bool}", list),
        query!("essential.canAdd", "Can Add Property to Essential Graphics", "{layer, path|prop, as?} → {ok, type?, reason?}", can_add),
        cmd!(
            "comp.openInEssentialGraphics",
            "Open in Essential Graphics",
            ["Composition"],
            None,
            "{comp?} → makes it the Essential Graphics panel's Primary composition and shows the panel",
            has_comp,
            open_in_eg
        ),
        query!("essential.instance", "Essential Properties of a precomp layer", "{layer}", instance_json),
        query!("essential.templateInfo", "Read a template's manifest", "{path}", template_info),
    ]
}

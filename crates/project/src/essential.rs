//! Essential Graphics: a composition's exposed controls (the Essential Graphics panel) and the
//! "Essential Properties" of precomp layers that nest it (per-instance override values, After
//! Effects' Master Properties).
//!
//! - A comp's [`EssentialGraphics`] lists controls: properties of its layers (shown as text,
//!   colour, slider, checkbox, point, angle or dropdown controls; Source Text also as a font
//!   control and Scale as one uniform slider), Media Replacement slots for
//!   footage layers, comments and groups.
//! - Every layer that nests such a comp gets an **Essential Properties** group (match id
//!   `essential`, [`GroupKind::Essential`]) mirroring the controls, one child per control with
//!   match id `eg<control id>`. Changing a child marks it *overridden* for that instance only;
//!   [`sync_project`] keeps the groups in step with the controls (and refreshes the values of
//!   controls that are not overridden), [`with_overrides`] builds the project an instance renders.
//! - An overridden Source Text is not frozen (#3186): the group keeps, per override, the master
//!   document it is merged with (its base, [`EssentialBase`]), and every sync merges the master
//!   into the instance's value ([`TextDoc::merge_master`]). The instance keeps its characters and
//!   the attributes it changed itself; the others follow the master's restyles. A Set of an
//!   overridden Source Text rebases what it changed ([`TextDoc::rebase`]), so the value set
//!   stays the instance's own. A keyframed override has a base per key
//!   ([`EssentialBase::keys`]), recorded with the key's value, so a key that an edit moves
//!   (keeping its value) takes its base along.
//!   Ownership is kept per run and per paragraph: the base document
//!   carries the base each run (or paragraph) of the instance follows
//!   ([`TextDoc::follow_runs`], [`TextDoc::follow_paras`], empty in projects saved before and
//!   while every run has the base's), so a run the instance styled itself keeps its own base
//!   while the others follow, and [`TextDoc::rebase`] carries the records through the
//!   instance's edits (typing, splitting, merging and deleting runs). Limits: only the
//!   master's Source Text at time 0 is compared (a style that changes at a later master
//!   keyframe does not reach instances); character and paragraph attributes are merged only
//!   where uniform in the master, and where mixed in the base only when the instance kept
//!   that formatting as it is. (As before, a keyframed override's keys keep a base each but the
//!   instance's edits rebase only the override's own base, not its keys'; two master changes
//!   with no edit in between keep every key's own runs.)

use std::collections::{HashMap, VecDeque};

use effectcraft_keyframe::TextDoc;
use effectcraft_time::Tick;
use serde::{Deserialize, Serialize};

use crate::props::{EssentialBase, EssentialKeyBase, GroupKind, Node, ParamUi, PropGroup, Property, Uid};
use crate::{ItemId, ItemKind, Keyframe, Layer, LayerId, LayerSource, Project, Value};

/// Match id of the Essential Properties group on precomp layers.
pub const GROUP: &str = "essential";

/// A composition's Essential Graphics definition.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct EssentialGraphics {
    /// Template name (Essential Graphics panel ▸ name field; the exported template's title).
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub controls: Vec<EgControl>,
}

/// One entry of the Essential Graphics panel.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EgControl {
    pub id: u64,
    /// Display name (renamable; defaults to the property's name).
    pub name: String,
    pub kind: EgKind,
    /// The control a property is shown as when not its natural one ([`control_type`]): a
    /// Source Text property as a [`ControlType::Font`] control, a Scale as a uniform
    /// [`ControlType::Scale`] slider.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub as_type: Option<ControlType>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum EgKind {
    /// A property of a layer of the comp. `links`: further properties the control drives
    /// (they follow the main property in the comp and take the instance's value in every
    /// instance).
    Property {
        layer: LayerId,
        prop: Uid,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        links: Vec<EgLink>,
    },
    /// A mirror: the property of control `of` shown again (its own name and place in the
    /// panel; one value, editing either edits both, in the comp and in every instance).
    Mirror { of: u64 },
    /// Media Replacement: the footage of a footage layer can be swapped per instance.
    Media { layer: LayerId },
    /// A text note shown in the panel.
    Comment { text: String },
    /// A named, collapsible group of controls.
    Group {
        #[serde(default)]
        children: Vec<EgControl>,
    },
}

/// A further property driven by a property control.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EgLink {
    pub layer: LayerId,
    pub prop: Uid,
}

/// The kind of control a property becomes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ControlType {
    Text,
    Color,
    Slider,
    Checkbox,
    Point,
    Angle,
    Dropdown,
    Media,
    /// The font family, style and size of a Source Text property (the text itself stays).
    Font,
    /// One uniform percentage for a Scale property (every axis the same).
    Scale,
}

impl ControlType {
    /// By label or serde name, case-insensitively (`"font"`, `"Scale"`…).
    pub fn parse(s: &str) -> Option<ControlType> {
        [
            ControlType::Text,
            ControlType::Color,
            ControlType::Slider,
            ControlType::Checkbox,
            ControlType::Point,
            ControlType::Angle,
            ControlType::Dropdown,
            ControlType::Media,
            ControlType::Font,
            ControlType::Scale,
        ]
        .into_iter()
        .find(|t| t.label().eq_ignore_ascii_case(s.trim()))
    }
    pub fn label(self) -> &'static str {
        match self {
            ControlType::Font => "Font",
            ControlType::Scale => "Scale",
            ControlType::Text => "Text",
            ControlType::Color => "Color",
            ControlType::Slider => "Slider",
            ControlType::Checkbox => "Checkbox",
            ControlType::Point => "Point",
            ControlType::Angle => "Angle",
            ControlType::Dropdown => "Dropdown",
            ControlType::Media => "Media",
        }
    }
}

/// The control a property is shown as, `None` when Essential Graphics can't expose it (paths,
/// gradients, layer pickers, hidden data).
pub fn control_type(p: &Property) -> Option<ControlType> {
    if matches!(p.ui, ParamUi::Hidden) {
        return None;
    }
    Some(match (&p.value, &p.ui) {
        (Value::Text(_) | Value::Str(_), _) => ControlType::Text,
        (Value::Color(_), _) => ControlType::Color,
        (Value::Bool(_), _) | (_, ParamUi::Checkbox) => ControlType::Checkbox,
        (Value::Enum(_), _) | (_, ParamUi::Popup { .. }) => ControlType::Dropdown,
        (_, ParamUi::Angle) => ControlType::Angle,
        (Value::Vec2(_) | Value::Vec3(_), _) => ControlType::Point,
        (Value::Scalar(_), _) => ControlType::Slider,
        _ => return None,
    })
}

/// Whether property `p` can be shown as a `ty` control: its natural control, a Source Text
/// property as Font, a two- or three-dimensional property as uniform Scale.
pub fn can_show_as(p: &Property, ty: ControlType) -> bool {
    if matches!(p.ui, ParamUi::Hidden) {
        return false;
    }
    match ty {
        ControlType::Font => matches!(p.value, Value::Text(_)),
        ControlType::Scale => matches!(p.value, Value::Vec2(_) | Value::Vec3(_)) && !matches!(p.ui, ParamUi::Angle),
        ControlType::Media => false,
        t => control_type(p) == Some(t),
    }
}

/// The control `c` shows for its property `p` (its `as_type`, else the natural one).
pub fn effective_type(c: &EgControl, p: &Property) -> Option<ControlType> {
    c.as_type.filter(|t| can_show_as(p, *t)).or_else(|| control_type(p))
}

/// Apply a Font control's value: the font family, style and size of `from` onto `to` (the text,
/// colours and every other attribute stay).
pub fn merge_font(to: &mut Value, from: &Value) {
    if let (Value::Text(t), Value::Text(f)) = (to, from) {
        t.font = f.font.clone();
        t.style = f.style.clone();
        t.size = f.size;
    }
}

impl EssentialGraphics {
    /// Every control, depth first (groups before their children).
    pub fn flat(&self) -> Vec<&EgControl> {
        fn go<'a>(v: &'a [EgControl], out: &mut Vec<&'a EgControl>) {
            for c in v {
                out.push(c);
                if let EgKind::Group { children } = &c.kind {
                    go(children, out);
                }
            }
        }
        let mut out = vec![];
        go(&self.controls, &mut out);
        out
    }
    pub fn find(&self, id: u64) -> Option<&EgControl> {
        self.flat().into_iter().find(|c| c.id == id)
    }
    pub fn find_mut(&mut self, id: u64) -> Option<&mut EgControl> {
        fn go(v: &mut [EgControl], id: u64) -> Option<&mut EgControl> {
            for c in v {
                if c.id == id {
                    return Some(c);
                }
                if let EgKind::Group { children } = &mut c.kind
                    && let Some(x) = go(children, id)
                {
                    return Some(x);
                }
            }
            None
        }
        go(&mut self.controls, id)
    }
    /// Remove a control (anywhere) and return it.
    pub fn remove(&mut self, id: u64) -> Option<EgControl> {
        fn go(v: &mut Vec<EgControl>, id: u64) -> Option<EgControl> {
            if let Some(i) = v.iter().position(|c| c.id == id) {
                return Some(v.remove(i));
            }
            v.iter_mut().find_map(|c| if let EgKind::Group { children } = &mut c.kind { go(children, id) } else { None })
        }
        go(&mut self.controls, id)
    }
    /// The list a control is inserted into: a group's children or the top level.
    pub fn list_mut(&mut self, group: Option<u64>) -> Option<&mut Vec<EgControl>> {
        match group {
            None => Some(&mut self.controls),
            Some(g) => match &mut self.find_mut(g)?.kind {
                EgKind::Group { children } => Some(children),
                _ => None,
            },
        }
    }
    /// The control exposing property `prop` of `layer`, if any.
    pub fn control_for(&self, layer: LayerId, prop: Uid) -> Option<&EgControl> {
        self.flat().into_iter().find(|c| matches!(c.kind, EgKind::Property { layer: l, prop: p, .. } if l == layer && p == prop))
    }
    /// The control exposing property `prop` of `layer` as `as_type` (`None` = its natural
    /// control): a property can have one control of each kind (Source Text as Text and Font).
    pub fn control_for_as(&self, layer: LayerId, prop: Uid, as_type: Option<ControlType>) -> Option<&EgControl> {
        self.flat().into_iter().find(|c| c.as_type == as_type && matches!(c.kind, EgKind::Property { layer: l, prop: p, .. } if l == layer && p == prop))
    }
    /// The property control a property drives: its main property or one of its links.
    pub fn driver_of(&self, layer: LayerId, prop: Uid) -> Option<&EgControl> {
        self.flat().into_iter().find(|c| match &c.kind {
            EgKind::Property { layer: l, prop: p, links } => (*l == layer && *p == prop) || links.iter().any(|k| k.layer == layer && k.prop == prop),
            _ => false,
        })
    }
    /// The control a mirror shows (following mirrors of mirrors); the control itself otherwise.
    pub fn resolve(&self, id: u64) -> Option<&EgControl> {
        let mut c = self.find(id)?;
        for _ in 0..8 {
            match c.kind {
                EgKind::Mirror { of } => c = self.find(of)?,
                _ => return Some(c),
            }
        }
        None
    }
    /// Mirrors of control `id`.
    pub fn mirrors_of(&self, id: u64) -> Vec<u64> {
        self.flat().into_iter().filter(|c| matches!(c.kind, EgKind::Mirror { .. }) && self.resolve(c.id).is_some_and(|m| m.id == id)).map(|c| c.id).collect()
    }
    /// Controls an instance can override (properties, mirrors and media; not comments or
    /// groups).
    pub fn has_instance_controls(&self) -> bool {
        self.flat().iter().any(|c| matches!(c.kind, EgKind::Property { .. } | EgKind::Mirror { .. } | EgKind::Media { .. }))
    }
}

impl EgKind {
    /// Every property a property control drives: the main one, then the links.
    pub fn targets(&self) -> Vec<(LayerId, Uid)> {
        match self {
            EgKind::Property { layer, prop, links } => std::iter::once((*layer, *prop)).chain(links.iter().map(|k| (k.layer, k.prop))).collect(),
            _ => vec![],
        }
    }
}

/// Match id of an instance property / group for a control.
pub fn match_id(control: u64) -> String {
    format!("eg{control}")
}

/// Control id of an Essential Properties child (`eg42` → 42).
pub fn control_of(match_id: &str) -> Option<u64> {
    match_id.strip_prefix("eg")?.parse().ok()
}

/// The Essential Properties group of a layer.
pub fn group(layer: &Layer) -> Option<&PropGroup> {
    layer.props.sub(GROUP)
}

/// The instance property that owns this Source Text control's style base. A mirror shares
/// its source control's base.
pub fn text_base_owner(project: &Project, layer: &Layer, uid: Uid) -> Option<Uid> {
    let LayerSource::Comp { item } = layer.source else { return None };
    let g = group(layer)?;
    let p = g.find(uid)?;
    if !matches!(&p.value, Value::Text(_)) {
        return None;
    }
    let control = control_of(&p.match_id)?;
    let owner = project.comp(item)?.essential.as_ref()?.resolve(control)?;
    find_prop(&g.children, &match_id(owner.id)).map(|p| p.uid)
}

/// Carry the evaluated Source Text key's style base into the static override when the
/// stopwatch is turned off. Equal text values can have different style ownership.
pub fn preserve_text_key_base(layer: &mut Layer, uid: Uid, time: Tick) {
    let Some(g) = layer.props.sub_mut(GROUP) else { return };
    let Some(p) = g.find(uid) else { return };
    let active = p.keys.iter().filter(|k| !k.muted);
    let Some(first) = active.clone().next() else { return };
    let key = if time <= first.time { first } else { active.take_while(|k| k.time <= time).last().unwrap_or(first) };
    if !matches!(&key.value, Value::Text(_)) {
        return;
    }
    let key_time = key.time;
    let GroupKind::Essential { bases, .. } = &mut g.kind else { return };
    let Some(base) = bases.iter_mut().find(|b| b.prop == uid) else { return };
    if let Some(key_base) = base.keys.iter().find(|b| b.time == key_time) {
        base.doc = key_base.doc.clone();
        base.keys.clear();
    }
}

/// Uids of the overridden master properties of a layer.
pub fn overridden(layer: &Layer) -> Vec<Uid> {
    match group(layer).map(|g| &g.kind) {
        Some(GroupKind::Essential { overridden, .. }) => overridden.clone(),
        _ => vec![],
    }
}

/// The Source Text document control `c` of `comp` exposes, at time 0 (`None`: not text).
fn master_text(project: &Project, comp: ItemId, c: &EgControl) -> Option<Box<TextDoc>> {
    match source_prop(project, comp, c)?.1.value_at(effectcraft_time::Tick::ZERO) {
        Value::Text(d) => Some(d),
        _ => None,
    }
}

/// The source property a control exposes, in `project` (a mirror's: its master's main
/// property).
pub fn source_prop<'a>(project: &'a Project, comp: ItemId, c: &EgControl) -> Option<(&'a Layer, &'a Property)> {
    let (layer, prop) = match c.kind {
        EgKind::Property { layer, prop, .. } => (layer, prop),
        EgKind::Mirror { .. } => match project.comp(comp)?.essential.as_ref()?.resolve(c.id)?.kind {
            EgKind::Property { layer, prop, .. } => (layer, prop),
            _ => return None,
        },
        _ => return None,
    };
    let l = project.comp(comp)?.layer(layer)?;
    Some((l, l.props.find(prop)?))
}

/// The linked properties of a property control that exist in `project`: (layer, property).
pub fn linked_props<'a>(project: &'a Project, comp: ItemId, c: &EgControl) -> Vec<(&'a Layer, &'a Property)> {
    let Some(cm) = project.comp(comp) else { return vec![] };
    match &c.kind {
        EgKind::Property { links, .. } => links.iter().filter_map(|k| cm.layer(k.layer).and_then(|l| l.props.find(k.prop).map(|p| (l, p)))).collect(),
        _ => vec![],
    }
}

/// Whether property `b` can be driven by a control showing `a` (the same kind of value).
pub fn linkable(a: &Property, b: &Property) -> bool {
    control_type(a).is_some() && control_type(a) == control_type(b) && std::mem::discriminant(&a.value) == std::mem::discriminant(&b.value)
}

/// The properties a control drives stay equal: when this edit (`before` → `p`) changed one of
/// them, its value and keyframes (moved to each layer's time) go to all the others; otherwise
/// the main property's do.
fn sync_links(before: &Project, p: &mut Project) {
    let ids: Vec<ItemId> = p.comps().map(|(i, _)| *i).collect();
    for cid in ids {
        let Some(comp) = p.comp(cid) else { continue };
        let Some(eg) = &comp.essential else { continue };
        let old = before.comp(cid);
        let mut writes: Vec<(LayerId, Uid, Value, Vec<crate::Keyframe>)> = vec![];
        for c in eg.flat() {
            let targets = c.kind.targets();
            // Font controls take only the font: they never drive whole values.
            if targets.len() < 2 || c.as_type == Some(ControlType::Font) {
                continue;
            }
            let get = |cm: &crate::Comp, (l, u): (LayerId, Uid)| cm.layer(l).and_then(|l| l.props.find(u).cloned());
            let changed = |t: (LayerId, Uid)| match (get(comp, t), old.and_then(|o| get(o, t))) {
                (Some(a), Some(b)) => a.value != b.value || a.keys != b.keys,
                _ => false,
            };
            // The edited property leads (the main one when several changed).
            let lead = targets.iter().copied().find(|t| changed(*t)).filter(|t| !changed(targets[0]) || *t == targets[0]).unwrap_or(targets[0]);
            let Some(src_l) = comp.layer(lead.0) else { continue };
            let Some(src) = src_l.props.find(lead.1) else { continue };
            for k in targets.iter().filter(|t| **t != lead) {
                let Some(l) = comp.layer(k.0) else { continue };
                let Some(dst) = l.props.find(k.1) else { continue };
                if !linkable(src, dst) {
                    continue;
                }
                let keys: Vec<_> = src
                    .keys
                    .iter()
                    .map(|kf| {
                        let mut kf = kf.clone();
                        kf.time = l.layer_time(src_l.comp_time(kf.time));
                        kf
                    })
                    .collect();
                if dst.value != src.value || dst.keys != keys {
                    writes.push((k.0, k.1, src.value.clone(), keys));
                }
            }
        }
        if writes.is_empty() {
            continue;
        }
        let Some(c) = p.comp_mut(cid) else { continue };
        for (lid, uid, v, keys) in writes {
            if let Some(pr) = c.layer_mut(lid).and_then(|l| l.props.find_mut(uid)) {
                pr.value = v;
                pr.keys = keys;
            }
        }
    }
}

/// The footage item a media control shows by default (the layer's own source).
pub fn source_media(project: &Project, comp: ItemId, c: &EgControl) -> Option<ItemId> {
    let EgKind::Media { layer } = c.kind else { return None };
    match project.comp(comp)?.layer(layer)?.source {
        LayerSource::Footage { item } => Some(item),
        _ => None,
    }
}

/// Master Source Text documents (at time 0) by (comp, control), each read once per pass of
/// [`sync_project`]; `None`: the control's property is not text.
type Masters = HashMap<(ItemId, u64), Option<Box<TextDoc>>>;

/// Build the Essential Properties group a layer nesting `comp` should have, reusing uids and
/// override values from `old`. Uids for new nodes come from `next`. An overridden Source Text is
/// merged with its master ([`TextDoc::merge_master`]): the instance keeps its characters and the
/// attributes it changed itself, every other attribute follows the master.
fn build_group(project: &Project, comp: ItemId, eg: &EssentialGraphics, old: Option<&PropGroup>, masters: &mut Masters, next: &mut u64) -> PropGroup {
    let (over, old_bases): (Vec<Uid>, &[EssentialBase]) = match old.map(|g| &g.kind) {
        Some(GroupKind::Essential { overridden, bases }) => (overridden.clone(), bases.as_slice()),
        _ => (vec![], &[][..]),
    };
    let mut alloc = || {
        let v = *next;
        *next += 1;
        v
    };
    fn find_old<'a>(old: Option<&'a PropGroup>, m: &str) -> Option<&'a Node> {
        let g = old?;
        g.children.iter().find(|c| c.match_id() == m).or_else(|| g.groups().find_map(|sub| find_old(Some(sub), m)))
    }
    fn build(
        project: &Project,
        comp: ItemId,
        list: &[EgControl],
        old: Option<&PropGroup>,
        over: &[Uid],
        alloc: &mut dyn FnMut() -> u64,
        keep: &mut Vec<Uid>,
    ) -> Vec<Node> {
        let mut out = vec![];
        for c in list {
            let m = match_id(c.id);
            let prev = find_old(old, &m);
            match &c.kind {
                EgKind::Comment { .. } => {}
                EgKind::Group { children } => {
                    let uid = prev.map(Node::uid).unwrap_or_else(&mut *alloc);
                    let mut g = PropGroup::new(uid, &m, &c.name);
                    g.children = build(project, comp, children, old, over, alloc, keep);
                    out.push(Node::Group(g));
                }
                EgKind::Property { .. } | EgKind::Mirror { .. } => {
                    let Some((_, src)) = source_prop(project, comp, c) else { continue };
                    let prev = prev.and_then(Node::as_prop);
                    let uid = prev.map(|p| p.uid).unwrap_or_else(&mut *alloc);
                    let mut p = Property { uid, match_id: m, name: c.name.clone(), keys: vec![], expr: None, ..src.clone() };
                    // Instance properties follow the source's look but are always editable.
                    p.static_only = false;
                    if let Some(prev) = prev.filter(|pp| over.contains(&pp.uid)) {
                        p.value = prev.value.clone();
                        p.keys = prev.keys.clone();
                        p.expr = prev.expr.clone();
                        keep.push(uid);
                    } else {
                        p.value = src.value_at(effectcraft_time::Tick::ZERO);
                    }
                    out.push(Node::Prop(p));
                }
                EgKind::Media { .. } => {
                    let Some(item) = source_media(project, comp, c) else { continue };
                    let prev = prev.and_then(Node::as_prop);
                    let uid = prev.map(|p| p.uid).unwrap_or_else(&mut *alloc);
                    let mut p = Property::new(uid, &m, &c.name, Value::Scalar(item.0 as f64)).with_ui(ParamUi::Hidden);
                    p.static_only = true;
                    if let Some(prev) = prev.filter(|pp| over.contains(&pp.uid)) {
                        p.value = prev.value.clone();
                        keep.push(uid);
                    }
                    out.push(Node::Prop(p));
                }
            }
        }
        out
    }
    let uid = old.map(|g| g.uid).unwrap_or_else(&mut alloc);
    let mut keep = vec![];
    let mut children = build(project, comp, &eg.controls, old, &over, &mut alloc, &mut keep);
    // An overridden Source Text keeps its own characters and the attributes the instance
    // changed itself; every other attribute follows its master (#3186). An override without a
    // base (just made, or saved before bases existed) starts from the master as it is now.
    let mut bases = vec![];
    for c in eg.flat() {
        if !matches!(c.kind, EgKind::Property { .. }) {
            continue;
        }
        let Some(p) = find_prop_mut(&mut children, &match_id(c.id)) else { continue };
        if !keep.contains(&p.uid) || !matches!(p.value, Value::Text(_)) {
            continue;
        }
        let old_base = old_bases.iter().find(|b| b.prop == p.uid);
        let master = masters.entry((comp, c.id)).or_insert_with(|| master_text(project, comp, c));
        let Some(master) = master.as_deref() else {
            // The master can't be read now: keep the base for when it can.
            bases.extend(old_base.cloned());
            continue;
        };
        let base_in = old_base.map_or_else(|| master.clone(), |b| (*b.doc).clone());
        if p.keys.is_empty() {
            let mut base_out = base_in.clone();
            if let Value::Text(d) = &mut p.value {
                d.merge_master(&base_in, &mut base_out, master);
            }
            bases.push(EssentialBase { prop: p.uid, doc: Box::new(base_out), keys: vec![] });
            continue;
        }
        // Every key merges against its own base (one made by keying the override starts from
        // the value's), so a key's own value stays its own whatever another key followed.
        let old_keys = old_base.map_or(&[][..], |b| b.keys.as_slice());
        let picks = key_bases(&p.keys, old_keys);
        let mut keys = Vec::with_capacity(p.keys.len());
        for (k, pick) in p.keys.iter_mut().zip(picks) {
            let Value::Text(d) = &mut k.value else { continue };
            let key_in = pick.and_then(|j| old_keys.get(j)).map_or_else(|| base_in.clone(), |kb| (*kb.doc).clone());
            let mut key_out = key_in.clone();
            d.merge_master(&key_in, &mut key_out, master);
            keys.push(EssentialKeyBase { time: k.time, doc: Box::new(key_out), key: Some(d.clone()) });
        }
        // A key added later starts from the master as it is now.
        bases.push(EssentialBase { prop: p.uid, doc: Box::new(master.clone()), keys });
    }
    // Mirrors show their master's instance value (and override state).
    for c in eg.flat() {
        if !matches!(c.kind, EgKind::Mirror { .. }) {
            continue;
        }
        let Some(master) = eg.resolve(c.id) else { continue };
        let Some(src) = find_prop(&children, &match_id(master.id)).cloned() else { continue };
        if let Some(dst) = find_prop_mut(&mut children, &match_id(c.id)) {
            dst.value = src.value.clone();
            dst.keys = src.keys.clone();
            dst.expr = src.expr.clone();
            keep.retain(|u| *u != dst.uid);
            if keep.contains(&src.uid) {
                keep.push(dst.uid);
            }
        }
    }
    PropGroup {
        uid,
        match_id: GROUP.into(),
        name: "Essential Properties".into(),
        kind: GroupKind::Essential { overridden: keep, bases },
        enabled: true,
        label: None,
        children,
    }
}

fn find_prop<'a>(nodes: &'a [Node], m: &str) -> Option<&'a Property> {
    nodes.iter().find_map(|n| match n {
        Node::Prop(p) if p.match_id == m => Some(p),
        Node::Group(g) => find_prop(&g.children, m),
        _ => None,
    })
}

fn find_prop_mut<'a>(nodes: &'a mut [Node], m: &str) -> Option<&'a mut Property> {
    nodes.iter_mut().find_map(|n| match n {
        Node::Prop(p) if p.match_id == m => Some(p),
        Node::Group(g) => find_prop_mut(&mut g.children, m),
        _ => None,
    })
}

/// An instance's mirror property edited in this edit passes its value to its master (the
/// group build then copies the master back to every mirror).
fn propagate_mirrors(before: &Project, after: &mut Project) {
    let ids: Vec<ItemId> = after.comps().map(|(i, _)| *i).collect();
    for cid in ids {
        let (Some(comp), Some(old_comp)) = (after.comp(cid), before.comp(cid)) else { continue };
        let mut writes: Vec<(usize, String, Property)> = vec![];
        for (i, l) in comp.layers.iter().enumerate() {
            let LayerSource::Comp { item } = l.source else { continue };
            let Some(eg) = after.comp(item).and_then(|c| c.essential.as_ref()) else { continue };
            let (Some(g), Some(og)) = (group(l), old_comp.layer(l.id).and_then(group)) else { continue };
            for c in eg.flat() {
                if !matches!(c.kind, EgKind::Mirror { .. }) {
                    continue;
                }
                let Some(master) = eg.resolve(c.id) else { continue };
                let (mm, cm) = (match_id(master.id), match_id(c.id));
                let (Some(mp), Some(cp)) = (find_prop(&g.children, &mm), find_prop(&g.children, &cm)) else { continue };
                let (Some(omp), Some(ocp)) = (find_prop(&og.children, &mm), find_prop(&og.children, &cm)) else { continue };
                let changed = |a: &Property, b: &Property| a.value != b.value || a.keys != b.keys || a.expr != b.expr;
                if changed(cp, ocp) && !changed(mp, omp) {
                    writes.push((i, mm, cp.clone()));
                }
            }
        }
        if writes.is_empty() {
            continue;
        }
        let Some(c) = after.comp_mut(cid) else { continue };
        for (i, mm, src) in writes {
            if let Some(g) = c.layers[i].props.sub_mut(GROUP)
                && let Some(dst) = find_prop_mut(&mut g.children, &mm)
            {
                dst.value = src.value;
                dst.keys = src.keys;
                dst.expr = src.expr;
            }
        }
    }
}

/// Master properties the user changed in this edit (`before` → `after`) become overridden. An
/// overridden Source Text the edit changed gets its base rebased ([`TextDoc::rebase`]): what the
/// user set stays the instance's own.
fn mark_overrides(before: &Project, after: &mut Project) {
    let ids: Vec<ItemId> = after.comps().map(|(i, _)| *i).collect();
    for cid in ids {
        let Some(comp) = after.comp(cid) else { continue };
        let Some(old_comp) = before.comp(cid) else { continue };
        let mut changes: Vec<(LayerId, Vec<Uid>)> = vec![];
        let mut rebases: Vec<(LayerId, Uid, TextDoc)> = vec![];
        for l in &comp.layers {
            let Some(g) = group(l) else { continue };
            let Some(ol) = old_comp.layer(l.id) else { continue };
            let Some(og) = group(ol) else { continue };
            let copied_key_base = |uid| {
                let (GroupKind::Essential { bases: was, .. }, GroupKind::Essential { bases: now, .. }) = (&og.kind, &g.kind) else { return false };
                was.iter().any(|b| b.prop == uid && !b.keys.is_empty()) && now.iter().any(|b| b.prop == uid && b.keys.is_empty())
            };
            // Properties overridden before this edit stay as the edit left them (Revert and
            // Push to Comp clear overrides by editing them).
            let now_over = overridden(l);
            let mut over = now_over.clone();
            over.extend(overridden(ol));
            let mut add = vec![];
            let mut set = vec![];
            g.walk("", &mut |_, p| {
                if over.contains(&p.uid) {
                    if now_over.contains(&p.uid)
                        && let Some(op) = og.find(p.uid)
                        // Freezing animation copies an evaluated key, rather than editing its style.
                        && (op.keys.is_empty() || !p.keys.is_empty() || !copied_key_base(p.uid))
                        && let (Value::Text(was), Value::Text(now)) = (&op.value, &p.value)
                        && was != now
                    {
                        set.push((p.uid, was.clone(), now.clone()));
                    }
                    return;
                }
                if let Some(op) = og.find(p.uid)
                    && (op.value != p.value || op.keys != p.keys || op.expr != p.expr)
                {
                    add.push(p.uid);
                }
            });
            if !add.is_empty() {
                changes.push((l.id, add));
            }
            if !set.is_empty()
                && let LayerSource::Comp { item } = l.source
                && let Some(eg) = after.comp(item).and_then(|c| c.essential.as_ref())
                && let GroupKind::Essential { bases, .. } = &g.kind
            {
                for (uid, was, now) in set {
                    let Some(base) = bases.iter().find(|b| b.prop == uid) else { continue };
                    let Some(ctl) = g.find(uid).and_then(|p| control_of(&p.match_id)).and_then(|id| eg.find(id)) else { continue };
                    let Some(master) = master_text(after, item, ctl) else { continue };
                    let mut rebased = (*base.doc).clone();
                    TextDoc::rebase(&mut rebased, &was, &now, &master);
                    rebases.push((l.id, uid, rebased));
                }
            }
        }
        if changes.is_empty() && rebases.is_empty() {
            continue;
        }
        let Some(c) = after.comp_mut(cid) else { continue };
        for (lid, add) in changes {
            if let Some(g) = c.layer_mut(lid).and_then(|l| l.props.sub_mut(GROUP))
                && let GroupKind::Essential { overridden, .. } = &mut g.kind
            {
                overridden.extend(add);
            }
        }
        for (lid, uid, rebased) in rebases {
            if let Some(g) = c.layer_mut(lid).and_then(|l| l.props.sub_mut(GROUP))
                && let GroupKind::Essential { bases, .. } = &mut g.kind
                && let Some(b) = bases.iter_mut().find(|b| b.prop == uid)
            {
                *b.doc = rebased;
            }
        }
    }
}

/// Which base of `bases` (index) each of `keys` merges against: a key keeps the base recorded
/// with its value ([`EssentialKeyBase::key`]) wherever it moved (a drag, Quick Offset, Time
/// Reverse, a transform), follow records included. A value match beats a time match. Keys of
/// equal value pair with their bases in time order, so a move keeps them apart; when there are
/// more keys of a value than bases recorded with it (a key was pasted, added or set to another
/// key's value), the keys still at the time of a base of that value keep it first. A key whose
/// value is not recorded (it was set since, or the base predates the record) takes the unclaimed
/// base at its time; a key left without a base shares the base of the keys of equal value (it is
/// a copy of them) when they all have the same one, else (just added) it starts from the master
/// as it is now.
///
/// Keys carry no identity beyond time and value: two keys of equal value that trade times (Time
/// Reverse) cannot be told apart, so the bases stay with the times.
fn key_bases(keys: &[Keyframe], bases: &[EssentialKeyBase]) -> Vec<Option<usize>> {
    let mut ordered: Vec<_> = bases.iter().enumerate().collect();
    ordered.sort_by_key(|(_, base)| base.time);
    let groups = key_value_groups(keys, &ordered);
    let mut picks = vec![None; keys.len()];
    let mut used = vec![false; bases.len()];

    for group in &groups {
        group.claim_matching_bases(&mut picks, &mut used);
    }

    let mut by_time = HashMap::<Tick, VecDeque<usize>>::new();
    for (j, base) in ordered {
        if !used.get(j).copied().unwrap_or(true) {
            by_time.entry(base.time).or_default().push_back(j);
        }
    }
    for (key, pick) in keys.iter().zip(&mut picks) {
        if pick.is_none() {
            *pick = by_time.get_mut(&key.time).and_then(VecDeque::pop_front);
        }
    }

    let doc = |j| bases.get(j).map(|b: &EssentialKeyBase| &b.doc);
    for group in groups.iter().filter(|g| g.keys.len() > g.bases.len()) {
        let mut twins = group.keys.iter().filter_map(|&(i, _)| picks.get(i).copied().flatten());
        let Some(first) = twins.next().filter(|&first| twins.all(|j| doc(j) == doc(first))) else { continue };
        for &(i, _) in &group.keys {
            if let Some(pick @ None) = picks.get_mut(i) {
                *pick = Some(first);
            }
        }
    }
    picks
}

fn claim_base(picks: &mut [Option<usize>], used: &mut [bool], key: usize, base: usize) {
    if let (Some(pick), Some(claimed)) = (picks.get_mut(key), used.get_mut(base)) {
        *pick = Some(base);
        *claimed = true;
    }
}

struct KeyValueGroup<'a> {
    doc: &'a TextDoc,
    keys: Vec<(usize, Tick)>,
    bases: Vec<(usize, Tick)>,
}

impl KeyValueGroup<'_> {
    fn claim_matching_bases(&self, picks: &mut [Option<usize>], used: &mut [bool]) {
        // Keep same-time matches first when a copy would otherwise get the original's base.
        if self.keys.len() > self.bases.len() {
            let mut exact = HashMap::<Tick, VecDeque<usize>>::new();
            for &(j, time) in &self.bases {
                exact.entry(time).or_default().push_back(j);
            }
            for &(i, time) in &self.keys {
                if let Some(j) = exact.get_mut(&time).and_then(VecDeque::pop_front) {
                    claim_base(picks, used, i, j);
                }
            }
        }

        let mut available = self.bases.iter();
        for &(i, _) in &self.keys {
            if picks.get(i).is_some_and(Option::is_some) {
                continue;
            }
            let Some(&(j, _)) = available.find(|&&(j, _)| !used.get(j).copied().unwrap_or(true)) else { break };
            claim_base(picks, used, i, j);
        }
    }
}

/// Bucket on inexpensive fields, then use full document equality inside a bucket. Each group
/// keeps the input key order and the established base time order for tie-breaking.
fn key_value_groups<'a>(keys: &'a [Keyframe], ordered: &[(usize, &'a EssentialKeyBase)]) -> Vec<KeyValueGroup<'a>> {
    fn bucket(doc: &TextDoc) -> (&str, &str, &str, usize, usize) {
        (&doc.text, &doc.font, &doc.style, doc.runs.len(), doc.paragraphs.len())
    }
    let mut groups = HashMap::<_, Vec<KeyValueGroup<'a>>>::new();
    for (i, key) in keys.iter().enumerate() {
        let Value::Text(doc) = &key.value else { continue };
        let bucket = groups.entry(bucket(doc)).or_default();
        if let Some(group) = bucket.iter_mut().find(|group| group.doc == &**doc) {
            group.keys.push((i, key.time));
        } else {
            bucket.push(KeyValueGroup { doc, keys: vec![(i, key.time)], bases: Vec::new() });
        }
    }
    for &(j, base) in ordered {
        if let Some(doc) = base.key.as_deref()
            && let Some(group) = groups.get_mut(&bucket(doc)).and_then(|groups| groups.iter_mut().find(|group| group.doc == doc))
        {
            group.bases.push((j, base.time));
        }
    }
    groups.into_values().flatten().collect()
}

/// After an edit: mark changed master properties as overridden, then add, update or remove the
/// Essential Properties groups of every precomp layer so they match their comp's controls.
pub fn sync_project(before: &Project, after: &mut Project) {
    sync_links(before, after);
    propagate_mirrors(before, after);
    mark_overrides(before, after);
    // A control can expose another instance's Essential Property (precomps nested in precomps),
    // and comps are visited in id order: an outer comp can be visited before the comp whose
    // group it reads changed. Passes repeat until nothing changes, bounded by this nesting depth.
    const MAX_PASSES: usize = 16;
    for pass in 0..MAX_PASSES {
        if !sync_groups(after) {
            return;
        }
        if pass + 1 == MAX_PASSES {
            log::debug!("essential: groups still changing after {MAX_PASSES} sync passes (nesting deeper than that, or a cycle)");
        }
    }
}

/// One pass of [`sync_project`] over every comp. Returns whether any group changed.
fn sync_groups(after: &mut Project) -> bool {
    let ids: Vec<ItemId> = after.comps().map(|(i, _)| *i).collect();
    let mut next = after.next_id;
    let mut masters = Masters::new();
    let mut changed = false;
    for cid in ids {
        let Some(comp) = after.comp(cid) else { continue };
        let mut updates: Vec<(usize, Option<PropGroup>)> = vec![];
        for (i, l) in comp.layers.iter().enumerate() {
            let want = match l.source {
                LayerSource::Comp { item } => after.comp(item).and_then(|c| c.essential.as_ref()).filter(|e| e.has_instance_controls()).map(|e| (item, e)),
                _ => None,
            };
            let old = group(l);
            match want {
                Some((item, eg)) => {
                    let g = build_group(after, item, eg, old, &mut masters, &mut next);
                    if old != Some(&g) {
                        updates.push((i, Some(g)));
                    }
                }
                None if old.is_some() => updates.push((i, None)),
                None => {}
            }
        }
        if updates.is_empty() {
            continue;
        }
        changed = true;
        let Some(c) = after.comp_mut(cid) else { continue };
        for (i, g) in updates {
            let Some(l) = c.layers.get_mut(i) else { continue };
            let pos = l.props.children.iter().position(|n| n.match_id() == GROUP);
            match (pos, g) {
                (Some(p), Some(g)) => {
                    if let Some(n) = l.props.children.get_mut(p) {
                        *n = Node::Group(g);
                    }
                }
                (None, Some(g)) => l.props.children.insert(0, Node::Group(g)),
                (Some(p), None) => {
                    l.props.children.remove(p);
                }
                (None, None) => {}
            }
        }
    }
    after.next_id = after.next_id.max(next);
    changed
}

/// Give every overridden Source Text without a base (saved before bases existed) the master as
/// it is now as its base: nothing changes on opening, and from then on the attributes the
/// instance shares with the master follow the master's restyles, while those that differ stay
/// the instance's own. Called on a project just opened.
pub fn init_bases(p: &mut Project) {
    let mut adds: Vec<(ItemId, usize, Vec<EssentialBase>)> = vec![];
    for (cid, comp) in p.comps() {
        for (i, l) in comp.layers.iter().enumerate() {
            let LayerSource::Comp { item } = l.source else { continue };
            let Some(eg) = p.comp(item).and_then(|c| c.essential.as_ref()) else { continue };
            let Some(g) = group(l) else { continue };
            let GroupKind::Essential { overridden, bases } = &g.kind else { continue };
            let mut add = vec![];
            g.walk("", &mut |_, pr| {
                if !overridden.contains(&pr.uid) || bases.iter().any(|b| b.prop == pr.uid) || !matches!(pr.value, Value::Text(_)) {
                    return;
                }
                let Some(c) = control_of(&pr.match_id).and_then(|id| eg.find(id)) else { return };
                if !matches!(c.kind, EgKind::Property { .. }) {
                    return;
                }
                if let Some(doc) = master_text(p, item, c) {
                    add.push(EssentialBase { prop: pr.uid, doc, keys: vec![] });
                }
            });
            if !add.is_empty() {
                adds.push((*cid, i, add));
            }
        }
    }
    for (cid, i, add) in adds {
        if let Some(GroupKind::Essential { bases, .. }) =
            p.comp_mut(cid).and_then(|c| c.layers.get_mut(i)).and_then(|l| l.props.sub_mut(GROUP)).map(|g| &mut g.kind)
        {
            bases.extend(add);
        }
    }
}

/// Keys given in comp time, in the time of a layer that starts at `start` and is stretched to
/// `stretch` percent. Empty when the stretch is not a usable factor.
fn keys_in_layer_time(keys: &[Keyframe], start: Tick, stretch: f64) -> Vec<Keyframe> {
    if stretch.is_finite() && stretch > 0.0 { keys.iter().map(|k| key_in_layer_time(k, start, stretch)).collect() } else { vec![] }
}

/// A key given in comp time, moved into the layer time of a layer that starts at `start` and is
/// stretched to `stretch` percent (the inverse of `Layer::comp_time`). Ease speeds are per
/// second, so they scale with the time axis; influences are fractions of the segment and stay.
fn key_in_layer_time(k: &Keyframe, start: Tick, stretch: f64) -> Keyframe {
    let d = k.time - start;
    let mut k = k.clone();
    k.time = if (stretch - 100.0).abs() < 1e-9 { d } else { Tick((d.0 as f64 * 100.0 / stretch) as i64) };
    if (stretch - 100.0).abs() >= 1e-9 {
        for e in k.in_ease.iter_mut().chain(k.out_ease.iter_mut()) {
            e.speed *= stretch / 100.0;
        }
    }
    k
}

/// One instance override: the control and the value it takes in this instance.
#[derive(Clone, Debug, PartialEq)]
pub struct Override {
    pub control: u64,
    pub value: Value,
    /// The instance property's keyframes, in the nested comp's time (`with_overrides` moves them
    /// into each target layer's time). Empty when the property is not keyed (or its keys can't
    /// be mapped); expressions in the comp then see only `value`.
    pub keys: Vec<Keyframe>,
}

/// The project as an instance of `comp` renders it: each overridden control's source property
/// set to the instance value and keyframes (its expression, if any, still applies) and
/// replaced media swapped in. `None` when nothing applies.
pub fn with_overrides(project: &Project, comp: ItemId, overrides: &[Override]) -> Option<Project> {
    let eg = project.comp(comp)?.essential.clone()?;
    let mut p = project.clone();
    let c = p.comp_mut(comp)?;
    let mut any = false;
    // Whole values first, then Font controls (which change only the font of whatever text the
    // instance shows). A mirror acts as its master control.
    let is_font = |o: &&Override| eg.resolve(o.control).is_some_and(|c| c.as_type == Some(ControlType::Font));
    let ordered = overrides.iter().filter(|o| !is_font(o)).chain(overrides.iter().filter(is_font));
    for o in ordered {
        let Some(ctl) = eg.resolve(o.control) else { continue };
        match &ctl.kind {
            EgKind::Property { .. } if ctl.as_type == Some(ControlType::Font) => {
                for (layer, prop) in ctl.kind.targets() {
                    if let Some(pr) = c.layer_mut(layer).and_then(|l| l.props.find_mut(prop)) {
                        merge_font(&mut pr.value, &o.value);
                        for k in &mut pr.keys {
                            merge_font(&mut k.value, &o.value);
                        }
                        any = true;
                    }
                }
            }
            EgKind::Property { .. } => {
                for (layer, prop) in ctl.kind.targets() {
                    if let Some(l) = c.layer_mut(layer) {
                        let (start, stretch) = (l.start_time, l.stretch);
                        let Some(pr) = l.props.find_mut(prop) else { continue };
                        // The keys are in nested comp time; the property reads them in this
                        // layer's own time (start offset, stretch). Without a usable mapping
                        // only the value is set.
                        pr.keys = keys_in_layer_time(&o.keys, start, stretch);
                        pr.value = o.value.clone();
                        any = true;
                    }
                }
            }
            EgKind::Media { layer } => {
                let item = ItemId(o.value.as_f64().max(0.0) as u64);
                // Replacement media must be footage.
                if !matches!(project.item(item).map(|i| &i.kind), Some(ItemKind::Footage(_))) {
                    continue;
                }
                if let Some(l) = c.layer_mut(*layer) {
                    l.source = LayerSource::Footage { item };
                    any = true;
                }
            }
            _ => {}
        }
    }
    any.then_some(p)
}

// ------------------------------------------------------------------------------ id remapping

/// Shift every id (items, layers, property uids, Essential Graphics controls) of `p` by
/// `offset`, keeping all references consistent: used when merging a template or another project
/// into a project. Ids from a damaged file saturate instead of overflowing.
pub fn offset_ids(p: &mut Project, offset: u64) {
    let items = std::mem::take(&mut p.items);
    for (_, mut it) in items {
        it.id = ItemId(it.id.0.saturating_add(offset));
        it.parent = it.parent.map(|f| ItemId(f.0.saturating_add(offset)));
        if let ItemKind::Comp(c) = &mut it.kind {
            let c = std::sync::Arc::make_mut(c);
            for l in &mut c.layers {
                offset_layer(l, offset);
            }
            if let Some(eg) = &mut c.essential {
                fn go(v: &mut [EgControl], o: u64) {
                    for c in v {
                        c.id = c.id.saturating_add(o);
                        match &mut c.kind {
                            EgKind::Property { layer, prop, links } => {
                                *layer = LayerId(layer.0.saturating_add(o));
                                *prop = prop.saturating_add(o);
                                for k in links {
                                    k.layer = LayerId(k.layer.0.saturating_add(o));
                                    k.prop = k.prop.saturating_add(o);
                                }
                            }
                            EgKind::Mirror { of } => *of = of.saturating_add(o),
                            EgKind::Media { layer } => *layer = LayerId(layer.0.saturating_add(o)),
                            EgKind::Group { children } => go(children, o),
                            EgKind::Comment { .. } => {}
                        }
                    }
                }
                go(&mut eg.controls, offset);
            }
        }
        p.items.insert(it.id, it);
    }
    p.render_queue.clear();
    p.next_id = p.next_id.saturating_add(offset);
}

fn offset_layer(l: &mut Layer, o: u64) {
    l.id = LayerId(l.id.0.saturating_add(o));
    l.parent = l.parent.map(|x| LayerId(x.0.saturating_add(o)));
    if let Some(tm) = &mut l.track_matte {
        tm.layer = LayerId(tm.layer.0.saturating_add(o));
    }
    match &mut l.source {
        LayerSource::Footage { item } | LayerSource::Comp { item } | LayerSource::Solid { item } | LayerSource::Model { item } => {
            *item = ItemId(item.0.saturating_add(o))
        }
        _ => {}
    }
    fn group(g: &mut PropGroup, o: u64, essential_parent: bool) {
        g.uid = g.uid.saturating_add(o);
        match &mut g.kind {
            GroupKind::Essential { overridden, bases } => {
                for u in overridden.iter_mut() {
                    *u = u.saturating_add(o);
                }
                for b in bases.iter_mut() {
                    b.prop = b.prop.saturating_add(o);
                }
            }
            // A tracker's Motion Target is a layer of the same comp.
            GroupKind::Tracker { settings } => {
                settings.target = settings.target.map(|t| LayerId(t.0.saturating_add(o)));
            }
            _ => {}
        }
        let is_essential = matches!(g.kind, GroupKind::Essential { .. }) || essential_parent;
        for c in &mut g.children {
            match c {
                Node::Prop(p) => {
                    p.uid = p.uid.saturating_add(o);
                    let fix = |v: &mut Value| {
                        if let Value::Layer(Some(id)) = v {
                            *id = id.saturating_add(o);
                        }
                    };
                    fix(&mut p.value);
                    for k in &mut p.keys {
                        fix(&mut k.value);
                    }
                    if is_essential && control_of(&p.match_id).is_some() {
                        p.match_id = match_id(control_of(&p.match_id).unwrap_or(0).saturating_add(o));
                        // Media replacement values are item ids.
                        if matches!(p.ui, ParamUi::Hidden)
                            && let Value::Scalar(x) = &mut p.value
                        {
                            *x += o as f64;
                        }
                    }
                }
                Node::Group(sub) => {
                    if is_essential && let Some(id) = control_of(&sub.match_id) {
                        sub.match_id = match_id(id.saturating_add(o));
                    }
                    group(sub, o, is_essential);
                }
            }
        }
    }
    group(&mut l.props, o, false);
}

/// The items `comp` needs (itself, nested comps, footage, solids), transitively.
pub fn dependencies(project: &Project, comp: ItemId) -> Vec<ItemId> {
    let mut out = vec![];
    let mut stack = vec![comp];
    while let Some(id) = stack.pop() {
        if out.contains(&id) {
            continue;
        }
        let Some(it) = project.item(id) else { continue };
        out.push(id);
        if let ItemKind::Comp(c) = &it.kind {
            for l in &c.layers {
                if let Some(i) = l.source.item() {
                    stack.push(i);
                }
                // Media replacement candidates referenced by instances inside.
                if let Some(g) = group(l) {
                    g.walk("", &mut |_, p| {
                        if matches!(p.ui, ParamUi::Hidden)
                            && control_of(&p.match_id).is_some()
                            && let Value::Scalar(x) = p.value
                        {
                            stack.push(ItemId(x as u64));
                        }
                    });
                }
            }
        }
    }
    out.sort();
    out
}

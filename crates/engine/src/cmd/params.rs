use super::*;

/// Hand a frontend-only command to the UI ([`crate::Event::Frontend`]).
pub(crate) fn frontend(s: &mut Session, id: &str, p: &Value) -> Result<Value> {
    s.events.push(crate::Event::Frontend { command: id.to_string(), params: p.clone() });
    Ok(serde_json::json!({"frontend": id}))
}

/// Match path of a node (`transform/position`, `effects/#2/blurriness` style with `match#n`
/// occurrences) so it can be found again on another layer.
pub fn match_path_of(g: &effectcraft_project::PropGroup, uid: effectcraft_project::Uid) -> Option<String> {
    for c in &g.children {
        let nth = g.children.iter().take_while(|x| !std::ptr::eq(*x, c)).filter(|x| x.match_id() == c.match_id()).count() + 1;
        let seg = if nth == 1 { c.match_id().to_string() } else { format!("{}#{nth}", c.match_id()) };
        if c.uid() == uid {
            return Some(seg);
        }
        if let effectcraft_project::Node::Group(sub) = c
            && let Some(rest) = match_path_of(sub, uid)
        {
            return Some(format!("{seg}/{rest}"));
        }
    }
    None
}

/// Turn `effects/<match name>[#n]/<property>` (as [`match_path_of`] writes it) into a uid path on
/// `l`. Path parsing splits on `.`, which would cut a match name like `ec.control.slider` apart.
/// Any other path, or an effect that is not on the layer, comes back unchanged.
pub(crate) fn resolve_effect_path(l: &Layer, path: &str) -> String {
    let Some((seg, tail)) = path.strip_prefix("effects/").and_then(|r| r.split_once('/')) else { return path.to_string() };
    let (match_id, nth) = match seg.rsplit_once('#') {
        Some((m, n)) if !m.is_empty() => match n.parse::<usize>() {
            Ok(n) if n >= 1 => (m, n),
            _ => (seg, 1),
        },
        _ => (seg, 1),
    };
    match l.effects().and_then(|fx| fx.groups().filter(|g| g.match_id == match_id).nth(nth - 1)) {
        Some(g) => format!("effects/@{}/{tail}", g.uid),
        None => path.to_string(),
    }
}

/// Selected properties (leaves) of the active comp as (layer, uid); selected groups expand to
/// their properties.
pub(crate) fn selected_leaf_props(s: &Session) -> Vec<(LayerId, effectcraft_project::Uid)> {
    let Some(comp) = s.active_comp() else { return vec![] };
    let mut out = vec![];
    for (lid, uid) in &s.state.selected_props {
        let Some(l) = comp.layer(*lid) else { continue };
        if l.props.find(*uid).is_some() {
            out.push((*lid, *uid));
        } else if let Some(g) = l.props.find_group(*uid) {
            g.walk("", &mut |_, p| out.push((*lid, p.uid)));
        }
    }
    out
}

// ---------- params ----------

pub(crate) fn bad(cmd: &str, msg: impl Into<String>) -> EngineError {
    EngineError::BadParams { cmd: cmd.into(), msg: msg.into() }
}

pub(crate) fn str_p<'a>(p: &'a Value, k: &str) -> Option<&'a str> {
    p.get(k).and_then(Value::as_str)
}

pub(crate) fn f_p(p: &Value, k: &str) -> Option<f64> {
    p.get(k).and_then(Value::as_f64)
}

pub(crate) fn b_p(p: &Value, k: &str) -> Option<bool> {
    p.get(k).and_then(Value::as_bool)
}

pub(crate) fn merge_p(p: &Value) -> Option<&str> {
    str_p(p, "merge")
}

/// `comp`: item id or name (an all-digit string that names no comp is taken as an id, as MCP
/// clients often send numbers as strings); default the active comp.
pub(crate) fn comp_id(s: &Session, p: &Value) -> Result<ItemId> {
    let missing = |v: &Value| EngineError::NoSuchComp(v.to_string());
    match p.get("comp") {
        Some(v @ Value::Number(n)) => n.as_u64().map(ItemId).filter(|id| s.project.comp(*id).is_some()).ok_or_else(|| missing(v)),
        Some(v @ Value::String(name)) => s
            .project
            .items
            .values()
            .find(|i| &i.name == name && i.as_comp().is_some())
            .map(|i| i.id)
            .or_else(|| digits_id(name).map(ItemId).filter(|id| s.project.comp(*id).is_some()))
            .ok_or_else(|| missing(v)),
        _ => s.active_comp_id().ok_or(EngineError::NoComp),
    }
}

/// An all-ASCII-digit string as a number (`"98"` -> 98); anything else (names, `#n`, signs) is `None`.
fn digits_id(s: &str) -> Option<u64> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

/// Resolve a layer reference: id (number), `#n` / index (1-based), or name. An all-digit string
/// that names no layer is resolved as the number it spells.
pub(crate) fn resolve_layer(comp: &Comp, v: &Value) -> Option<LayerId> {
    match v {
        Value::Number(n) => {
            let id = n.as_u64()?;
            if comp.layer(LayerId(id)).is_some() { Some(LayerId(id)) } else { comp.layers.get((id as usize).checked_sub(1)?).map(|l| l.id) }
        }
        Value::String(s) => {
            if let Some(i) = s.strip_prefix('#').and_then(|i| i.parse::<usize>().ok()) {
                return comp.layers.get(i.checked_sub(1)?).map(|l| l.id);
            }
            comp.layers.iter().find(|l| &l.name == s).map(|l| l.id).or_else(|| digits_id(s).and_then(|id| resolve_layer(comp, &Value::from(id))))
        }
        _ => None,
    }
}

/// `layer` param or the first selected layer.
pub(crate) fn layer_p(s: &Session, p: &Value, cmd: &str) -> Result<(ItemId, LayerId)> {
    let cid = comp_id(s, p)?;
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    if let Some(v) = p.get("layer") {
        return resolve_layer(comp, v).map(|l| (cid, l)).ok_or_else(|| bad(cmd, format!("no layer {v}")));
    }
    s.state
        .selected_layers
        .first()
        .copied()
        .filter(|l| comp.layer(*l).is_some())
        .map(|l| (cid, l))
        .ok_or_else(|| bad(cmd, "no `layer` given and no layer selected"))
}

/// `layers` param (array) or the selection.
pub(crate) fn layers_p(s: &Session, p: &Value) -> Result<(ItemId, Vec<LayerId>)> {
    let cid = comp_id(s, p)?;
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    // Explicit references that match no layer are an error (agents otherwise get a silent no-op).
    let resolve =
        |v: &Value| resolve_layer(comp, v).ok_or_else(|| EngineError::Other(format!("no layer {v} (use a layer id, \"#n\" or a name from comp.info)")));
    if let Some(Value::Array(a)) = p.get("layers") {
        return Ok((cid, a.iter().map(resolve).collect::<Result<_>>()?));
    }
    if let Some(v) = p.get("layer").filter(|v| !v.is_null()) {
        return Ok((cid, vec![resolve(v)?]));
    }
    Ok((cid, s.state.selected_layers.iter().copied().filter(|l| comp.layer(*l).is_some()).collect()))
}

/// Whether a layer can be picked by the user (Select All, Ctrl+Up/Down, a Timeline click): not
/// locked, and not hidden by the composition's Hide Shy Layers.
pub(crate) fn selectable(comp: &Comp, l: &Layer) -> bool {
    !(l.switches.locked || (comp.hide_shy && l.switches.shy))
}

/// The layers of `ids` a command may edit: locked layers are left alone, as in After Effects.
/// An error when every one of them is locked.
pub(crate) fn unlocked(s: &Session, cid: ItemId, ids: Vec<LayerId>, cmd: &str) -> Result<Vec<LayerId>> {
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    let locked = |id: &LayerId| comp.layer(*id).is_some_and(|l| l.switches.locked);
    match ids.iter().find(|id| locked(id)) {
        Some(first) if ids.iter().all(locked) => {
            let name = comp.layer(*first).map(|l| l.name.clone()).unwrap_or_default();
            Err(bad(cmd, format!("“{name}” is locked (Layer ▸ Switches ▸ Unlock All Layers)")))
        }
        _ => Ok(ids.into_iter().filter(|id| !locked(id)).collect()),
    }
}

/// `visible: [{layer, prop}]`: the properties the Timeline shows (J / K and Select All
/// Keyframes use only those, as in After Effects). `None` when not given.
pub(crate) fn visible_p(p: &Value) -> Option<Vec<(LayerId, effectcraft_project::Uid)>> {
    let a = p.get("visible")?.as_array()?;
    Some(a.iter().filter_map(|v| Some((LayerId(v.get("layer")?.as_u64()?), v.get("prop")?.as_u64()?))).collect())
}

/// `time` (seconds) or `frame` param, else the CTI.
pub(crate) fn time_p(s: &Session, p: &Value, comp: Option<&Comp>) -> Tick {
    if let Some(t) = f_p(p, "time") {
        return Tick::from_seconds_f64(t);
    }
    if let (Some(f), Some(c)) = (p.get("frame").and_then(Value::as_i64), comp) {
        return c.frame_rate.tick_of(f);
    }
    s.time()
}

pub(crate) fn layer_mut(p: &mut effectcraft_project::Project, cid: ItemId, lid: LayerId) -> Result<&mut Layer> {
    p.comp_mut(cid).ok_or(EngineError::NoComp)?.layer_mut(lid).ok_or(EngineError::Project(effectcraft_project::ProjectError::NoLayer(lid)))
}

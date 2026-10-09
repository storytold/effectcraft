//! The command registry. Ids follow After Effects' menu structure; every menu item, panel button,
//! shortcut and viewer/timeline gesture maps to one of these.

mod align;
mod align_data;
mod anim;
pub mod anim_tools;
mod animation;
pub(crate) mod app_more;
mod autotrace;
mod batch;
mod camera_cmds;
mod comp;
pub(crate) mod comp_more;
pub mod content_fill;
mod create;
mod edit;
mod effect;
pub mod essential;
pub mod expr_tools;
mod file;
pub(crate) mod file_more;
mod focus;
pub mod footage_panel;
pub(crate) mod frame_export;
mod frontend;
mod help;
mod key_labels;
mod key_transform;
mod keys_more;
mod layer;
mod layer_menu;
mod layer_time;
pub(crate) mod link;
mod liquify;
mod lottie;
pub mod markers;
mod mask;
pub mod mask_interp;
pub(crate) mod model3d;
pub mod paint;
pub mod panels_cmds;
pub(crate) mod path_nulls;
mod paths;
mod project_items;
mod prop;
mod prop_groups;
mod proxy;
pub mod puppet;
mod query;
mod render_queue;
pub(crate) mod rig3d;
pub mod roto_cmds;
mod scene_detect;
pub mod scripts;
mod settings;
pub mod shape_stroke;
pub mod shape_tool;
mod stubs;
mod styles;
mod text_anim;
pub mod text_edit;
mod three_d;
pub mod time;
mod timeline;
mod track;
mod view;
pub mod viewer_cmds;
pub(crate) mod vpe;
pub(crate) mod vr_editor;
mod warp_cmds;
pub(crate) mod watch_folder;
#[cfg(test)]
pub(crate) use app_more::report_text as report_text_for_tests;
pub(crate) use app_more::view_command;
#[cfg(test)]
pub(crate) use keys_more::amplitudes as amplitudes_for_tests;
#[cfg(test)]
pub(crate) use mask::split_segment as split_segment_for_tests;

use std::sync::OnceLock;

use effectcraft_project::{Comp, ItemId, Layer, LayerId};
use effectcraft_time::Tick;
use serde_json::Value;

use crate::{EngineError, Result, Session};

pub type Run = fn(&mut Session, &Value) -> Result<Value>;
pub type Enabled = fn(&Session) -> std::result::Result<(), String>;

/// Metadata + implementation for one command.
pub struct CommandSpec {
    pub id: &'static str,
    pub label: &'static str,
    /// Menu placement, e.g. `["Layer", "New"]`. Empty = not in menus.
    pub menu: &'static [&'static str],
    /// Default shortcut (egui-style names, `Cmd` = ⌘ on macOS / Ctrl elsewhere).
    pub shortcut: Option<&'static str>,
    /// Parameter description for agents and the MCP schema.
    pub params: &'static str,
    pub enabled: Enabled,
    pub run: Run,
    /// Record in the journal (false for pure queries).
    pub journal: bool,
}

#[macro_export]
macro_rules! cmd {
    ($id:literal, $label:literal, [$($m:literal),*], $sc:expr, $params:literal, $en:expr, $run:expr) => {
        $crate::commands::CommandSpec { id: $id, label: $label, menu: &[$($m),*], shortcut: $sc, params: $params, enabled: $en, run: $run, journal: true }
    };
}
#[macro_export]
macro_rules! query {
    ($id:literal, $label:literal, $params:literal, $run:expr) => {
        $crate::commands::CommandSpec {
            id: $id,
            label: $label,
            menu: &[],
            shortcut: None,
            params: $params,
            enabled: $crate::commands::always,
            run: $run,
            journal: false,
        }
    };
}

pub fn command_specs() -> &'static [CommandSpec] {
    static SPECS: OnceLock<Vec<CommandSpec>> = OnceLock::new();
    SPECS.get_or_init(|| {
        let mut v = Vec::new();
        v.extend(file::specs());
        v.extend(crate::models::specs());
        v.extend(edit::specs());
        v.extend(comp::specs());
        v.extend(layer::specs());
        v.extend(styles::specs());
        v.extend(three_d::specs());
        v.extend(model3d::specs());
        v.extend(rig3d::specs());
        v.extend(text_anim::specs());
        v.extend(text_edit::specs());
        v.extend(layer_time::specs());
        v.extend(prop::specs());
        v.extend(prop_groups::specs());
        v.extend(project_items::specs());
        v.extend(anim::specs());
        v.extend(anim_tools::specs());
        v.extend(link::specs());
        v.extend(markers::specs());
        v.extend(mask::specs());
        v.extend(paths::specs());
        v.extend(effect::specs());
        v.extend(time::specs());
        v.extend(render_queue::specs());
        v.extend(help::specs());
        v.extend(query::specs());
        v.extend(batch::specs());
        v.extend(layer_menu::specs());
        v.extend(create::specs());
        v.extend(animation::specs());
        v.extend(view::specs());
        v.extend(viewer_cmds::specs());
        v.extend(shape_stroke::specs());
        v.extend(shape_tool::specs());
        v.extend(focus::specs());
        v.extend(key_labels::specs());
        v.extend(key_transform::specs());
        v.extend(file_more::specs());
        v.extend(lottie::specs());
        v.extend(timeline::specs());
        v.extend(comp_more::specs());
        v.extend(frame_export::specs());
        v.extend(watch_folder::specs());
        v.extend(vpe::specs());
        v.extend(path_nulls::specs());
        v.extend(vr_editor::specs());
        v.extend(frontend::specs());
        v.extend(track::specs());
        v.extend(mask_interp::specs());
        v.extend(warp_cmds::specs());
        v.extend(camera_cmds::specs());
        v.extend(roto_cmds::specs());
        v.extend(paint::specs());
        v.extend(puppet::specs());
        v.extend(liquify::specs());
        v.extend(settings::specs());
        v.extend(app_more::specs());
        v.extend(keys_more::specs());
        v.extend(essential::specs());
        v.extend(expr_tools::specs());
        v.extend(proxy::specs());
        v.extend(scripts::specs());
        v.extend(panels_cmds::specs());
        v.extend(footage_panel::specs());
        v.extend(autotrace::specs());
        v.extend(scene_detect::specs());
        v.extend(align_data::specs());
        v.extend(align::specs());
        v.extend(content_fill::specs());
        v.extend(stubs::specs());
        v.extend(crate::storage::specs());
        v.extend(crate::learn::specs());
        v.extend(crate::templates::specs());
        v.extend(crate::ease_presets::specs());
        v.extend(crate::preview::specs());
        v
    })
}

/// Set `footage` as the proxy of `item` (File ▸ Create Proxy's post-render action).
pub(crate) fn proxy_set(s: &mut Session, item: ItemId, footage: effectcraft_project::Footage) -> Result<()> {
    proxy::set_proxy_footage(s, &[item], footage)
}

/// (id, name) of the text animation presets.
pub fn text_preset_list() -> Vec<(String, String)> {
    text_anim::preset_names()
}

pub fn find(id: &str) -> Option<&'static CommandSpec> {
    // Every command execution looks its spec up: index the ~2,000 ids once.
    static INDEX: OnceLock<std::collections::HashMap<&'static str, usize>> = OnceLock::new();
    let index = INDEX.get_or_init(|| {
        let mut m = std::collections::HashMap::new();
        for (i, c) in command_specs().iter().enumerate() {
            m.entry(c.id).or_insert(i);
        }
        m
    });
    index.get(id).map(|&i| &command_specs()[i])
}

// ---------- parameter validation (agents) ----------

/// Top-level keys accepted by a params doc such as `{layers: [id|name|#n], add?, toggle?}` or
/// `{time? (s) | frame? | timecode?}`; `None` when the doc isn't a `{…}` key list.
pub fn accepted_params(doc: &str) -> Option<Vec<String>> {
    let body = doc.trim().strip_prefix('{')?;
    // Top-level pieces up to the matching close brace, split at `,` and `|`; `:` starts a value.
    let (mut keys, mut cur, mut depth, mut in_value) = (Vec::new(), String::new(), 0i32, false);
    let push = |cur: &mut String, keys: &mut Vec<String>| {
        let k: String = cur.trim().chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_').collect();
        if !k.is_empty() && !keys.contains(&k) {
            keys.push(k);
        }
        cur.clear();
    };
    for c in body.chars() {
        match c {
            '{' | '[' | '(' => depth += 1,
            ']' | ')' => depth -= 1,
            '}' if depth == 0 => {
                push(&mut cur, &mut keys);
                return Some(keys);
            }
            '}' => depth -= 1,
            ',' if depth == 0 => {
                push(&mut cur, &mut keys);
                in_value = false;
            }
            '|' if depth == 0 && !in_value => push(&mut cur, &mut keys),
            ':' if depth == 0 => {
                push(&mut cur, &mut keys);
                in_value = true;
            }
            _ if depth == 0 && !in_value => cur.push(c),
            _ => {}
        }
    }
    None
}

/// Top-level pieces of a params doc (split at depth-0 commas), e.g. `{layer?, path|prop, value}`
/// → `["layer?", "path|prop", "value"]`.
fn doc_pieces(doc: &str) -> Option<Vec<String>> {
    let body = doc.trim().strip_prefix('{')?;
    let (mut out, mut cur, mut depth) = (Vec::new(), String::new(), 0i32);
    for c in body.chars() {
        match c {
            '{' | '[' | '(' => depth += 1,
            ']' | ')' => depth -= 1,
            '}' if depth == 0 => {
                if !cur.trim().is_empty() {
                    out.push(cur.trim().to_string());
                }
                return Some(out);
            }
            '}' => depth -= 1,
            ',' if depth == 0 => {
                if !cur.trim().is_empty() {
                    out.push(cur.trim().to_string());
                }
                cur.clear();
                continue;
            }
            _ => {}
        }
        cur.push(c);
    }
    None
}

/// The JSON Schema of a command's parameters, derived from its params doc: one property per
/// accepted key (described by its piece of the doc), no others. MCP `describe_command` and
/// `list_commands {schemas: true}` serve it.
pub fn params_schema(spec: &CommandSpec) -> Value {
    let mut props = serde_json::Map::new();
    let mut required = vec![];
    for piece in doc_pieces(spec.params).unwrap_or_default() {
        let keys = accepted_params(&format!("{{{piece}}}")).unwrap_or_default();
        let alternatives = keys.len() > 1;
        for k in keys {
            // `key?` is optional; so is any key with alternatives (`path|prop`).
            let optional = alternatives || piece.contains(&format!("{k}?"));
            if !optional && !required.contains(&k) {
                required.push(k.clone());
            }
            props.insert(k, serde_json::json!({"description": piece}));
        }
    }
    serde_json::json!({
        "type": "object",
        "description": spec.params,
        "properties": props,
        "required": required,
        "additionalProperties": accepted_params(spec.params).is_none(),
    })
}

/// Keys every command understands (comp targeting, gesture merging) and accepted aliases.
const ALWAYS_OK: &[&str] = &["comp", "merge"];
const ALIASES: &[(&str, &str)] = &[("layers", "layer"), ("layer", "layers"), ("time", "frame"), ("frameRate", "fps"), ("properties", "property")];

/// Reject unknown top-level parameters (typos, guessed names) with the list of accepted keys.
pub fn check_params(spec: &CommandSpec, params: &Value) -> Result<()> {
    let (Some(obj), Some(accepted)) = (params.as_object(), accepted_params(spec.params)) else { return Ok(()) };
    let ok = |k: &str| ALWAYS_OK.contains(&k) || accepted.iter().any(|a| a == k || ALIASES.iter().any(|(doc, alias)| doc == a && *alias == k));
    let unknown: Vec<&str> = obj.keys().map(String::as_str).filter(|k| !ok(k)).collect();
    if unknown.is_empty() {
        return Ok(());
    }
    let list = if accepted.is_empty() { "none".to_string() } else { accepted.join(", ") };
    Err(bad(
        spec.id,
        format!("unknown parameter(s) {}; accepted: {list} (doc: {})", unknown.iter().map(|k| format!("`{k}`")).collect::<Vec<_>>().join(", "), spec.params),
    ))
}

// ---------- enablement ----------

pub fn always(_: &Session) -> std::result::Result<(), String> {
    Ok(())
}
pub(crate) fn has_comp(s: &Session) -> std::result::Result<(), String> {
    s.active_comp().map(|_| ()).ok_or_else(|| "no composition is open".into())
}
pub(crate) fn has_layers(s: &Session) -> std::result::Result<(), String> {
    has_comp(s)?;
    if s.state.selected_layers.is_empty() { Err("select a layer first".into()) } else { Ok(()) }
}
pub(crate) fn has_keys(s: &Session) -> std::result::Result<(), String> {
    has_comp(s)?;
    if s.state.selected_keys.is_empty() { Err("select keyframes first".into()) } else { Ok(()) }
}

pub(crate) fn has_project_selection(s: &Session) -> std::result::Result<(), String> {
    if s.state.project_selection.is_empty() { Err("select an item in the Project panel first".into()) } else { Ok(()) }
}

/// Disabled-command enablement (menu entries whose implementation lands with another milestone;
/// none are left, kept for future stubs).
#[allow(dead_code)]
pub(crate) fn not_yet(_: &Session) -> std::result::Result<(), String> {
    Err("not available yet in EffectCraft".into())
}

/// `run` of a not-yet-available command.
#[allow(dead_code)]
pub(crate) fn not_yet_run(_: &mut Session, _: &Value) -> Result<Value> {
    Err(EngineError::Other("not available yet in EffectCraft".into()))
}

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

/// `comp`: item id or name; default the active comp.
pub(crate) fn comp_id(s: &Session, p: &Value) -> Result<ItemId> {
    let missing = |v: &Value| EngineError::NoSuchComp(v.to_string());
    match p.get("comp") {
        Some(v @ Value::Number(n)) => n.as_u64().map(ItemId).filter(|id| s.project.comp(*id).is_some()).ok_or_else(|| missing(v)),
        Some(v @ Value::String(name)) => s.project.items.values().find(|i| &i.name == name && i.as_comp().is_some()).map(|i| i.id).ok_or_else(|| missing(v)),
        _ => s.active_comp_id().ok_or(EngineError::NoComp),
    }
}

/// Resolve a layer reference: id (number), `#n` / index (1-based), or name.
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
            comp.layers.iter().find(|l| &l.name == s).map(|l| l.id)
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

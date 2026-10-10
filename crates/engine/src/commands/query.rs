//! Read-only queries for agents (not journaled).

use effectcraft_project::{GroupKind, ItemKind, Layer, Node, PropGroup};
use serde_json::{Value, json};

use super::{CommandSpec, b_p, comp_id, layer_p, str_p};
use crate::{EngineError, Result, Session, query};

fn list(s: &mut Session, p: &Value) -> Result<Value> {
    let filter = str_p(p, "filter").map(str::to_ascii_lowercase);
    let only = b_p(p, "enabledOnly").unwrap_or(false);
    let schemas = b_p(p, "schemas").unwrap_or(false);
    let v: Vec<Value> = super::command_specs()
        .iter()
        .filter(|c| filter.as_ref().is_none_or(|f| c.id.to_ascii_lowercase().contains(f) || c.label.to_ascii_lowercase().contains(f)))
        .filter_map(|c| {
            let en = (c.enabled)(s);
            if only && en.is_err() {
                return None;
            }
            let mut o =
                json!({"id": c.id, "label": c.label, "menu": c.menu, "shortcut": c.shortcut, "params": c.params, "enabled": en.is_ok(), "why": en.err()});
            if schemas {
                o["schema"] = super::params_schema(c);
            }
            Some(o)
        })
        .collect();
    Ok(json!(v))
}

fn summary(s: &mut Session, _: &Value) -> Result<Value> {
    let items: Vec<Value> = s
        .project
        .items
        .values()
        .map(|i| {
            let mut o = json!({"id": i.id.0, "name": i.name, "type": i.type_name(), "label": i.label.name(), "parent": i.parent.map(|p| p.0)});
            if let Some((w, h)) = i.dimensions() {
                o["size"] = json!([w, h]);
            }
            if let Some(d) = i.duration() {
                o["duration"] = json!(d.seconds());
            }
            if let ItemKind::Comp(c) = &i.kind {
                o["layers"] = json!(c.layers.len());
                o["frameRate"] = json!(c.frame_rate.as_f64());
            }
            o
        })
        .collect();
    Ok(json!({
        "path": s.path,
        "dirty": s.is_dirty(),
        "bitDepth": s.project.settings.bit_depth.label(),
        "renderer": super::file::backend_status(s),
        "items": items,
        "activeComp": s.state.active_comp.map(|c| c.0),
        "time": s.time().seconds(),
        "selectedLayers": s.state.selected_layers.iter().map(|l| l.0).collect::<Vec<_>>(),
        "undo": s.history.undo.iter().map(|u| u.0.clone()).collect::<Vec<_>>(),
    }))
}

fn layer_json(l: &Layer, idx: usize) -> Value {
    json!({
        "id": l.id.0, "index": idx, "name": l.name, "type": l.source.type_name(), "label": l.label.name(),
        "in": l.in_point.seconds(), "out": l.out_point.seconds(), "start": l.start_time.seconds(), "stretch": l.stretch,
        "blendMode": l.blend_mode.label(), "parent": l.parent.map(|p| p.0),
        "trackMatte": l.track_matte.map(|m| json!({"layer": m.layer.0, "kind": m.kind.label()})),
        "switches": serde_json::to_value(&l.switches).unwrap_or_default(),
    })
}

fn comp_info(s: &mut Session, p: &Value) -> Result<Value> {
    let cid = comp_id(s, p)?;
    let c = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    let name = s.project.item(cid).map(|i| i.name.clone()).unwrap_or_default();
    Ok(json!({
        "id": cid.0, "name": name, "width": c.width, "height": c.height, "frameRate": c.frame_rate.as_f64(), "duration": c.duration.seconds(),
        "workArea": [c.work_area.0.seconds(), c.work_area.1.seconds()], "background": c.background,
        "preserveFrameRate": c.preserve_frame_rate, "preserveResolution": c.preserve_resolution,
        "layers": c.layers.iter().enumerate().map(|(i, l)| layer_json(l, i + 1)).collect::<Vec<_>>(),
    }))
}

/// The path segment that addresses child `i` of `g` (see `effectcraft_project::parse_path`): the match
/// id (with `#n` when it is not the first sibling matching it), or `#index` when the match id is not
/// path-safe (effects and other ids containing `.`).
fn segment(g: &PropGroup, i: usize) -> String {
    let c = &g.children[i];
    let m = c.match_id();
    if m.is_empty() || m.contains(['/', '.', '#', '@']) || m.parse::<u64>().is_ok() {
        return format!("#{}", i + 1);
    }
    let n = g.children[..i].iter().filter(|x| x.match_id() == m || x.name().eq_ignore_ascii_case(m)).count() + 1;
    if n == 1 { m.to_string() } else { format!("{m}#{n}") }
}

fn group_json(g: &PropGroup, path: &str, l: &Layer, t: effectcraft_time::Tick, depth: usize, max_depth: usize) -> Value {
    let children: Vec<Value> = if depth >= max_depth {
        vec![]
    } else {
        g.children
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let seg = segment(g, i);
                let cpath = if path.is_empty() { seg } else { format!("{path}/{seg}") };
                match c {
                    Node::Prop(p) => {
                        let mut o = json!({"path": cpath, "uid": p.uid, "match": p.match_id, "name": p.name, "type": p.value.kind_name(), "value": p.value_at(l.layer_time(t)).to_json()});
                        if !p.keys.is_empty() {
                            o["keys"] = json!(
                                p.keys
                                    .iter()
                                    .map(|k| json!({"time": k.time.seconds(), "value": k.value.to_json(), "in": k.in_interp.label(), "out": k.out_interp.label()}))
                                    .collect::<Vec<_>>()
                            );
                        }
                        if let Some(e) = &p.expr {
                            o["expression"] = json!(e.text);
                        }
                        o
                    }
                    Node::Group(g) => group_json(g, &cpath, l, t, depth + 1, max_depth),
                }
            })
            .collect()
    };
    let mut o = json!({"path": path, "uid": g.uid, "match": g.match_id, "name": g.name, "enabled": g.enabled, "children": children});
    if depth >= max_depth && !g.children.is_empty() {
        // Not expanded at this `depth`: ask again with a larger one (or for this group's layer).
        o["truncated"] = json!(true);
    }
    match &g.kind {
        GroupKind::Mask { mode, inverted, .. } => {
            o["mask"] = json!({"mode": mode.label(), "inverted": inverted});
        }
        GroupKind::Effect { effect } => {
            o["effect"] = json!(effect);
        }
        _ => {}
    }
    o
}

fn layer_tree(s: &mut Session, p: &Value) -> Result<Value> {
    let (cid, lid) = layer_p(s, p, "layer.tree")?;
    let c = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    let idx = c.index_of(lid).unwrap_or(0);
    let l = c.layer(lid).ok_or(EngineError::NoComp)?;
    let depth = p.get("depth").and_then(Value::as_u64).unwrap_or(16) as usize;
    let t = p.get("time").and_then(Value::as_f64).map(effectcraft_time::Tick::from_seconds_f64).unwrap_or_else(|| s.time());
    let mut o = layer_json(l, idx);
    o["time"] = json!(t.seconds());
    o["properties"] = group_json(&l.props, "", l, t, 0, depth);
    Ok(o)
}

/// One command in full, with the JSON Schema of its parameters.
fn describe(s: &mut Session, p: &Value) -> Result<Value> {
    let id = str_p(p, "command").or(str_p(p, "id")).ok_or_else(|| super::bad("command.describe", "missing `command`"))?;
    let c = super::find(id).ok_or_else(|| EngineError::UnknownCommand(id.to_string()))?;
    let en = (c.enabled)(s);
    Ok(json!({
        "id": c.id, "label": c.label, "menu": c.menu, "shortcut": c.shortcut, "params": c.params,
        "schema": super::params_schema(c), "undoable": c.journal, "enabled": en.is_ok(), "why": en.err(),
    }))
}

/// What this build can do: counts agents use to orient themselves, and the parity summary.
fn capabilities(s: &mut Session, _: &Value) -> Result<Value> {
    let fx = effectcraft_effects::registry();
    let formats: Vec<Value> = s.exporter.as_ref().map(|e| e.formats().iter().map(|f| json!(format!("{f:?}"))).collect()).unwrap_or_default();
    Ok(json!({
        "version": env!("CARGO_PKG_VERSION"),
        "commands": super::command_specs().len(),
        "effects": {"total": fx.len(), "gpu": fx.iter().filter(|e| e.gpu).count(), "float": fx.iter().filter(|e| e.float).count()},
        "export": formats,
        "import": s.importer.is_some(),
        "expressions": s.expr.is_some(),
        "scripting": s.script.is_some(),
        "gpu": s.accel.is_some(),
        "parity": {
            "target": "Adobe After Effects 2026",
            "summary": PARITY,
            "doc": "docs/target-app-parity.md",
        },
    }))
}

/// The headline of `docs/target-app-parity.md` (kept in sync by hand at each re-measure).
const PARITY: &str = "≈91% feature breadth and ≈45% ready for real work (estimated parity with After Effects 2026 26.5, stage alpha); all After Effects effects present (306 registered)";

fn state(s: &mut Session, _: &Value) -> Result<Value> {
    Ok(serde_json::to_value(&s.state).unwrap_or_default())
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        query!("command.list", "List Commands", "{filter?, enabledOnly?, schemas? (add each command's params JSON Schema)}", list),
        query!("command.describe", "Describe Command", "{command} → id, label, menu, shortcut, params doc, JSON `schema`, enabled", describe),
        query!("app.capabilities", "App Capabilities", "{} → version, command/effect counts, export formats, parity summary", capabilities),
        query!("project.summary", "Project Summary", "{}", summary),
        query!("comp.info", "Composition Info", "{comp?}", comp_info),
        query!("layer.tree", "Layer Property Tree", "{layer?, comp?, depth?, time? (comp s)} → nodes with `path`", layer_tree),
        query!("editor.state", "Editor State", "{}", state),
    ]
}

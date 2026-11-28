//! `doc.query`: read-only answers about the open project.
//!
//! The query is `{"what": "project" | "comp" | "selection"}`, default `project`. The scope is
//! `metadata`. `pixels` is not served: EffectCraft has no pixel snapshot API for a plug-in yet, so
//! the call ends as `E_UNSUPPORTED` rather than a made-up frame.

use craft_plugin_abi::permissions::DocScope;
use craft_plugin_host::services::QueryFailure;
use effectcraft_engine::Session;
use effectcraft_project::{Comp, Layer};
use serde_json::{Value, json};

/// Answer `query` from the session. Does not change it.
pub fn query(s: &Session, scope: DocScope, query: &Value) -> Result<Value, QueryFailure> {
    if scope != DocScope::Metadata {
        return Err(QueryFailure::Unsupported);
    }
    match query.get("what").and_then(Value::as_str).unwrap_or("project") {
        "project" => Ok(project(s)),
        "comp" => Ok(comp(s)),
        "selection" => Ok(selection(s)),
        other => Err(QueryFailure::BadQuery(format!("unknown `what`: {other}"))),
    }
}

fn layer_json(l: &Layer) -> Value {
    json!({ "id": l.id.0, "name": l.name, "source": l.source.type_name() })
}

fn comp_json(c: &Comp, id: u64, name: &str) -> Value {
    json!({
        "id": id,
        "name": name,
        "width": c.width,
        "height": c.height,
        "frame_rate": { "num": c.frame_rate.num, "den": c.frame_rate.den },
        "duration_ticks": c.duration.0,
        "layers": c.layers.iter().map(layer_json).collect::<Vec<_>>(),
        "markers": c.markers.len(),
    })
}

fn active(s: &Session) -> Option<Value> {
    let id = s.active_comp_id()?;
    let item = s.project.item(id)?;
    item.as_comp().map(|c| comp_json(c, id.0, &item.name))
}

fn project(s: &Session) -> Value {
    json!({
        "path": s.path,
        "revision": s.revision,
        "dirty": s.is_dirty(),
        "items": s.project.items.len(),
        "active_comp": active(s),
        "can_undo": !s.history.undo.is_empty(),
        "can_redo": !s.history.redo.is_empty(),
    })
}

fn comp(s: &Session) -> Value {
    active(s).unwrap_or(Value::Null)
}

fn selection(s: &Session) -> Value {
    let layers: Vec<Value> =
        s.active_comp().map(|c| c.layers.iter().filter(|l| s.state.selected_layers.contains(&l.id)).map(layer_json).collect()).unwrap_or_default();
    json!({ "layers": layers, "time_ticks": s.time().0 })
}

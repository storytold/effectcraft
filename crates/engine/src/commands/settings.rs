//! Settings (`prefs.*`), keyboard shortcut presets (`shortcuts.*`), File ▸ Open Recent and
//! auto-save commands.

use serde_json::{Value, json};

use super::{CommandSpec, always, bad, str_p};
use crate::shortcuts::{DEFAULT_PRESET, binding_key, normalize};
use crate::{EngineError, Result, Session, cmd, query};

// ---------------------------------------------------------------- prefs

fn prefs_get(s: &mut Session, p: &Value) -> Result<Value> {
    let key = str_p(p, "key").unwrap_or("");
    s.prefs.get(key).ok_or_else(|| bad("prefs.get", format!("unknown setting `{key}`")))
}

fn prefs_set(s: &mut Session, p: &Value) -> Result<Value> {
    let mut pairs: Vec<(String, Value)> = vec![];
    if let Some(Value::Object(m)) = p.get("values") {
        pairs.extend(m.iter().map(|(k, v)| (k.clone(), v.clone())));
    }
    if let Some(k) = str_p(p, "key") {
        let v = p.get("value").cloned().ok_or_else(|| bad("prefs.set", "missing `value`"))?;
        pairs.push((k.to_string(), v));
    }
    if pairs.is_empty() {
        return Err(bad("prefs.set", "give `key` and `value` (or `values: {key: value}`)"));
    }
    let mut next = s.prefs.clone();
    for (k, v) in &pairs {
        next.set(k, v.clone()).map_err(|e| bad("prefs.set", e))?;
    }
    // Checked on the final candidate so whole-page values, coercions and batches can't slip past.
    if crate::prefs::script_origin() && next.scripting.allow_scripts_write_files && !s.prefs.scripting.allow_scripts_write_files {
        return Err(bad("prefs.set", "scripts can't enable file or network access; turn on Allow Scripts to Write Files and Access Network in Preferences"));
    }
    s.prefs = next;
    s.prefs_changed();
    s.save_prefs();
    let out: serde_json::Map<String, Value> = pairs.iter().map(|(k, _)| (k.clone(), s.prefs.get(k).unwrap_or(Value::Null))).collect();
    Ok(Value::Object(out))
}

fn prefs_reset(s: &mut Session, p: &Value) -> Result<Value> {
    let page = str_p(p, "page");
    s.prefs.reset(page).map_err(|e| bad("prefs.reset", e))?;
    s.prefs_changed();
    s.save_prefs();
    s.toast(match page {
        Some(pg) => format!("Reset {pg} settings"),
        None => "Reset all settings".into(),
    });
    Ok(json!({"reset": page.unwrap_or("all")}))
}

fn prefs_open(s: &mut Session, p: &Value) -> Result<Value> {
    let page = str_p(p, "page").unwrap_or("general");
    let id = crate::prefs::page_id(page).ok_or_else(|| bad("prefs.open", format!("unknown settings page `{page}`")))?;
    super::frontend(s, "app.settings", &json!({"page": id}))
}

fn prefs_pages(_: &mut Session, _: &Value) -> Result<Value> {
    Ok(crate::prefs::pages_json())
}

// ---------------------------------------------------------------- shortcuts

/// The bindable named by `command` (+ `params`), or by a binding key (`app.settings {...}`).
fn bindable(s: &Session, p: &Value, cmd: &str) -> Result<crate::shortcuts::Bindable> {
    let c = str_p(p, "command").ok_or_else(|| bad(cmd, "missing `command`"))?;
    let params = p.get("params").cloned().unwrap_or(Value::Null);
    let t = s.shortcuts();
    let key = binding_key(c, &params);
    t.find(&key).or_else(|| t.find(c)).cloned().ok_or_else(|| bad(cmd, format!("`{key}` can't have a shortcut (see shortcuts.list)")))
}

fn keys_param(p: &Value, cmd: &str) -> Result<Vec<String>> {
    Ok(match p.get("keys") {
        None | Some(Value::Null) => vec![],
        Some(Value::String(k)) if k.trim().is_empty() => vec![],
        Some(Value::String(k)) => vec![k.clone()],
        Some(Value::Array(a)) => a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect(),
        Some(v) => return Err(bad(cmd, format!("`keys` must be a string or a list, not {v}"))),
    })
}

fn sc_list(s: &mut Session, p: &Value) -> Result<Value> {
    let q = str_p(p, "query").unwrap_or("").to_lowercase();
    let assigned = p.get("assigned").and_then(Value::as_bool).unwrap_or(false);
    let keys_q = str_p(p, "keys").and_then(normalize);
    let t = s.shortcuts();
    let rows: Vec<Value> = t
        .bindables
        .iter()
        .filter_map(|b| {
            let keys = t.keys.get(&b.key).cloned().unwrap_or_default();
            if assigned && keys.is_empty() {
                return None;
            }
            if let Some(k) = &keys_q
                && !keys.contains(k)
            {
                return None;
            }
            let hay = format!("{} {} {} {}", b.label, b.command, b.path.join(" "), keys.join(" ")).to_lowercase();
            if !q.split_whitespace().all(|w| hay.contains(w)) {
                return None;
            }
            let mut o = serde_json::to_value(b).unwrap_or_default();
            o["keys"] = json!(keys);
            Some(o)
        })
        .collect();
    Ok(json!({"preset": s.keymaps.active, "presets": s.keymaps.names(), "count": rows.len(), "commands": rows}))
}

fn sc_set(s: &mut Session, p: &Value) -> Result<Value> {
    let b = bindable(s, p, "shortcuts.set")?;
    let keys = keys_param(p, "shortcuts.set")?;
    let conflicts: Vec<_> = keys.iter().flat_map(|k| s.shortcuts().conflicts(&b, k)).collect();
    let created = s.keymaps.set(&b, &keys).map_err(|e| bad("shortcuts.set", e))?;
    s.shortcuts_changed();
    let now = s.shortcuts().keys.get(&b.key).cloned().unwrap_or_default();
    if let Some(n) = &created {
        s.toast(format!("The default shortcuts can't be changed: created the preset \"{n}\""));
    }
    if !conflicts.is_empty() {
        let names: Vec<String> = conflicts.iter().map(|c| format!("{} ({})", c.label, c.keys)).collect();
        s.toast(format!("Shortcut conflict: also used by {}", names.join(", ")));
    }
    Ok(json!({"command": b.key, "keys": now, "preset": s.keymaps.active, "created": created, "conflicts": conflicts}))
}

fn sc_reset(s: &mut Session, p: &Value) -> Result<Value> {
    if let Some(c) = str_p(p, "command") {
        let b = bindable(s, p, "shortcuts.reset")?;
        let defaults = b.defaults.clone();
        s.keymaps.set(&b, &defaults).map_err(|e| bad("shortcuts.reset", e))?;
        s.shortcuts_changed();
        return Ok(json!({"command": c, "keys": defaults}));
    }
    s.keymaps.reset_active();
    s.shortcuts_changed();
    Ok(json!({"preset": s.keymaps.active}))
}

fn sc_preset(s: &mut Session, p: &Value) -> Result<Value> {
    let op = str_p(p, "op").unwrap_or("select");
    let name = str_p(p, "name");
    let e = |m: String| bad("shortcuts.preset", m);
    let k = &mut s.keymaps;
    match op {
        "select" => k.select(name.unwrap_or(DEFAULT_PRESET)).map_err(e)?,
        "new" => {
            k.duplicate(DEFAULT_PRESET, Some(name.unwrap_or("Custom"))).map_err(e)?;
        }
        "duplicate" => {
            let from = str_p(p, "from").map(str::to_string).unwrap_or_else(|| k.active.clone());
            k.duplicate(&from, name).map_err(e)?;
        }
        "delete" => {
            let n = name.map(str::to_string).unwrap_or_else(|| k.active.clone());
            k.delete(&n).map_err(e)?;
        }
        "rename" => {
            let to = str_p(p, "newName").ok_or_else(|| bad("shortcuts.preset", "missing `newName`"))?;
            let from = name.map(str::to_string).unwrap_or_else(|| k.active.clone());
            k.rename(&from, to).map_err(e)?;
        }
        _ => return Err(bad("shortcuts.preset", "op must be select, new, duplicate, delete or rename")),
    }
    s.shortcuts_changed();
    Ok(json!({"preset": s.keymaps.active, "presets": s.keymaps.names()}))
}

fn sc_export(s: &mut Session, p: &Value) -> Result<Value> {
    let doc = s.keymaps.export(str_p(p, "preset")).map_err(|e| bad("shortcuts.export", e))?;
    if let Some(path) = str_p(p, "path") {
        let text = serde_json::to_string_pretty(&doc).unwrap_or_default();
        s.services.write_file(path, text.as_bytes()).map_err(|e| EngineError::Other(format!("cannot write {path}: {e}")))?;
        return Ok(json!({"path": path, "preset": doc["name"]}));
    }
    Ok(doc)
}

fn sc_import(s: &mut Session, p: &Value) -> Result<Value> {
    let doc = match (str_p(p, "path"), p.get("preset")) {
        (Some(path), _) => {
            let bytes = s.services.read_file(path).map_err(|e| EngineError::Other(format!("cannot read {path}: {e}")))?;
            serde_json::from_slice(&bytes).map_err(|e| bad("shortcuts.import", format!("{path}: {e}")))?
        }
        (None, Some(v)) if v.is_object() => v.clone(),
        _ => return Err(bad("shortcuts.import", "give `path` or `preset` (an exported document)")),
    };
    let name = s.keymaps.import(&doc, str_p(p, "name")).map_err(|e| bad("shortcuts.import", e))?;
    s.shortcuts_changed();
    Ok(json!({"preset": name}))
}

fn sc_conflicts(s: &mut Session, _: &Value) -> Result<Value> {
    let c: Vec<Value> =
        s.shortcuts().all_conflicts().into_iter().map(|(a, c)| json!({"command": a, "keys": c.keys, "with": c.key, "scope": c.scope})).collect();
    Ok(json!(c))
}

// ---------------------------------------------------------------- recent / auto-save

fn open_recent(s: &mut Session, p: &Value) -> Result<Value> {
    let path = match (str_p(p, "path"), p.get("index").and_then(Value::as_u64)) {
        (Some(path), _) => path.to_string(),
        (None, Some(i)) => s.prefs.recent_projects.get(i as usize).cloned().ok_or_else(|| bad("file.openRecent", format!("no recent project #{i}")))?,
        _ => s.prefs.recent_projects.first().cloned().ok_or_else(|| bad("file.openRecent", "no recent projects"))?,
    };
    match super::file::open(s, &json!({"path": path})) {
        Ok(v) => Ok(v),
        Err(e) => {
            // A project that's gone drops out of the list.
            if !std::path::Path::new(&path).exists() {
                s.prefs.recent_projects.retain(|x| *x != path);
                s.save_prefs();
            }
            Err(e)
        }
    }
}

fn clear_recent(s: &mut Session, _: &Value) -> Result<Value> {
    s.prefs.recent_projects.clear();
    s.save_prefs();
    s.prefs_revision += 1;
    Ok(Value::Null)
}

fn has_recent(s: &Session) -> std::result::Result<(), String> {
    if s.prefs.recent_projects.is_empty() { Err("no recent projects".into()) } else { Ok(()) }
}

fn auto_save(s: &mut Session, _: &Value) -> Result<Value> {
    let path = s.autosave_now()?;
    s.toast(format!("Auto-saved to {path}"));
    Ok(json!({"path": path}))
}

fn recovery_info(s: &mut Session, _: &Value) -> Result<Value> {
    let root = s.default_autosave_root();
    let prefs = s.autosave_prefs();
    let latest = crate::autosave::latest_in(s.file_ops(), &prefs, s.path.as_deref(), root.as_deref()).map(|p| p.to_string_lossy().to_string());
    Ok(json!({
        "lastAutoSave": s.autosave.last_path,
        "latestOnDisk": latest,
        "folder": crate::autosave::folder(&prefs, s.path.as_deref(), root.as_deref()).map(|p| p.to_string_lossy().to_string()),
        "recentProjects": s.prefs.recent_projects,
    }))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        query!("prefs.get", "Get Setting", "{key?: e.g. general.undoLevels (omit for all settings)}", prefs_get),
        cmd!("prefs.set", "Change Setting", [], None, "{key, value, values?: {key: value}}", always, prefs_set),
        cmd!("prefs.reset", "Reset Settings", [], None, "{page?: general|labels|project|…}", always, prefs_reset),
        cmd!(
            "prefs.open",
            "Open Settings",
            [],
            None,
            "{page?: general|startup|project|composition|previews|appearance|grids|labels|type|import|export|audio|disk|memory|video|3d|scripting}",
            always,
            prefs_open
        ),
        query!("prefs.pages", "Settings Pages", "{}", prefs_pages),
        query!("shortcuts.list", "List Keyboard Shortcuts", "{query?, keys?, assigned?: bool}", sc_list),
        cmd!("shortcuts.set", "Set Keyboard Shortcut", [], None, "{command, params?, keys: \"Cmd+Shift+K\" | [keys] | null}", always, sc_set),
        cmd!("shortcuts.reset", "Reset Keyboard Shortcuts", [], None, "{command?, params?}", always, sc_reset),
        cmd!("shortcuts.preset", "Keyboard Shortcut Preset", [], None, "{op: select|new|duplicate|delete|rename, name?, from?, newName?}", always, sc_preset),
        cmd!("shortcuts.export", "Export Keyboard Shortcuts", [], None, "{preset?, path?}", always, sc_export),
        cmd!("shortcuts.import", "Import Keyboard Shortcuts", [], None, "{path? | preset?: exported document, name?}", always, sc_import),
        query!("shortcuts.conflicts", "Keyboard Shortcut Conflicts", "{}", sc_conflicts),
        cmd!("file.openRecent", "Open Recent", [], None, "{index? | path?}", has_recent, open_recent),
        cmd!("file.clearRecent", "Clear Recent Projects", ["File", "Open Recent"], None, "{}", has_recent, clear_recent),
        cmd!("file.autoSave", "Auto-Save Now", [], None, "{}", always, auto_save),
        query!("file.recoveryInfo", "Auto-Save Status", "{}", recovery_info),
    ]
}

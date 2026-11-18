//! Effect ▸ Plug-ins ▸ the `plugin.*` commands of the shared plug-in runtime (RFC-0046): list,
//! install (a file, base64 data, a GitHub repository or a catalog's package), update, remove and
//! run plug-ins, and the catalogs they come from. The runtime and the rules a run follows are in
//! [`crate::plugin`]. The Plug-in Library's `library.*` commands are separate and unchanged.

use serde_json::{Map, Value, json};

use super::{CommandSpec, always, b_p, bad, str_p};
use crate::plugin;
use crate::{Result, Session};

/// None of these edits the project by itself (`plugin.run`'s commands record their own single
/// undo step and journal themselves), so none is journaled.
fn spec(id: &'static str, label: &'static str, params: &'static str, run: super::Run) -> CommandSpec {
    CommandSpec { id, label, menu: &[], shortcut: None, alt_shortcuts: &[], params, enabled: always, run, journal: false }
}

fn list(s: &mut Session, _p: &Value) -> Result<Value> {
    plugin::ensure(s);
    let restart = s.plugins.restart_needed();
    let plugins: Vec<Value> = s
        .plugins
        .list()
        .iter()
        .map(|p| {
            let mut v = p.describe();
            v["restartNeeded"] = json!(restart.contains(&p.manifest.id));
            v
        })
        .collect();
    let last = s.plugins.last.as_ref().map(|(id, command, _)| json!({"id": id, "command": command}));
    Ok(json!({
        "plugins": plugins,
        "problems": s.plugins.problems(),
        "catalogs": s.plugins.catalogs(),
        "stored": s.plugins.has_store(),
        "folder": s.plugins_dir().map(|d| d.display().to_string()),
        "last": last,
    }))
}

fn install(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "plugin.install";
    let mut request: Map<String, Value> = p.as_object().cloned().unwrap_or_default();
    request.remove("review");
    request.remove("sha256");
    let request = Value::Object(request);
    let wanted = str_p(p, "sha256").map(str::to_string);
    let c = match wanted.as_deref().and_then(|h| plugin::take_held(s, h, &request)) {
        Some(c) => c,
        None => plugin::obtain(s, cmd, p)?,
    };
    let (_, mut info) = plugin::review(s, cmd, &c)?;
    let sha256 = info["sha256"].as_str().unwrap_or_default().to_string();
    if let Some(w) = &wanted
        && !w.eq_ignore_ascii_case(&sha256)
    {
        return Err(bad(cmd, format!("the bundle's SHA-256 is {sha256}, not the {w} that was reviewed: review it again")));
    }
    if b_p(p, "review").unwrap_or(false) {
        plugin::hold(s, &sha256, request, c);
        info["installed"] = json!(false);
        return Ok(info);
    }
    let installed = plugin::install(s, cmd, c)?;
    info["installed"] = json!(true);
    info["restartNeeded"] = json!(s.plugins.restart_needed().contains(&installed.manifest.id));
    Ok(info)
}

fn update(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "plugin.update";
    plugin::ensure(s);
    let only = str_p(p, "id");
    if let Some(id) = only
        && s.plugins.get(id).is_none()
    {
        return Err(bad(cmd, format!("no plug-in `{id}` is installed")));
    }
    let apply = b_p(p, "apply").unwrap_or(false);
    let targets: Vec<plugin::Plugin> = s.plugins.list().iter().filter(|x| only.is_none_or(|id| x.manifest.id.eq_ignore_ascii_case(id))).cloned().collect();
    let (mut updates, mut updated, mut errors) = (vec![], vec![], vec![]);
    for old in targets {
        let id = old.manifest.id.clone();
        let (params, available) = match plugin::update_for(s, &old) {
            Ok(Some(found)) => found,
            Ok(None) => continue,
            Err(e) => {
                errors.push(json!({"id": id, "error": e}));
                continue;
            }
        };
        let mut entry = json!({"id": id, "installed": old.manifest.version, "available": available, "install": params});
        if apply {
            // An update that asks for other permissions waits for its confirmation (`plugin.install`).
            let done = plugin::obtain(s, cmd, &params).and_then(|c| {
                let (_, info) = plugin::review(s, cmd, &c)?;
                if info["permissionsChanged"].as_bool().unwrap_or(true) {
                    return Ok(false);
                }
                plugin::install(s, cmd, c).map(|_| true)
            });
            match done {
                Ok(true) => updated.push(json!(id)),
                Ok(false) => entry["permissionsChanged"] = json!(true),
                Err(e) => errors.push(json!({"id": id, "error": e.to_string()})),
            }
        }
        updates.push(entry);
    }
    Ok(json!({"updates": updates, "updated": updated, "errors": errors}))
}

fn remove(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "plugin.remove";
    let id = str_p(p, "id").ok_or_else(|| bad(cmd, "`id` required (see plugin.list)"))?.to_string();
    if !plugin::remove(s, cmd, &id)? {
        return Err(bad(cmd, format!("no plug-in `{id}` is installed")));
    }
    let restart = s.plugins.restart_needed().iter().any(|r| r.eq_ignore_ascii_case(&id));
    Ok(json!({"removed": id, "restartNeeded": restart}))
}

fn run(s: &mut Session, p: &Value) -> Result<Value> {
    let id = str_p(p, "id").ok_or_else(|| bad("plugin.run", "`id` required (see plugin.list)"))?.to_string();
    let command = str_p(p, "command").map(str::to_string);
    let fuel = p.get("fuel").and_then(Value::as_f64).filter(|f| f.is_finite() && *f >= 0.0).map(|f| f as u64);
    plugin::run(s, &id, command.as_deref(), p.get("params").unwrap_or(&Value::Null), fuel)
}

fn catalog_list(s: &mut Session, p: &Value) -> Result<Value> {
    plugin::ensure(s);
    let urls = match str_p(p, "url") {
        Some(url) => vec![url.to_string()],
        None => s.plugins.catalogs().to_vec(),
    };
    let (mut packages, mut warnings) = (vec![], vec![]);
    for url in &urls {
        match plugin::catalog_packages(s, url) {
            Ok((found, w)) => {
                packages.extend(found);
                warnings.extend(w.into_iter().map(|w| format!("{url}: {w}")));
            }
            Err(e) => warnings.push(format!("{url}: {e}")),
        }
    }
    Ok(json!({"catalogs": urls, "packages": packages, "warnings": warnings}))
}

fn catalog_add(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "plugin.catalog.add";
    let url = str_p(p, "url").map(str::trim).filter(|u| !u.is_empty()).ok_or_else(|| bad(cmd, "`url` required"))?.to_string();
    let catalogs = plugin::set_catalog(s, &url, b_p(p, "remove").unwrap_or(false)).map_err(|e| bad(cmd, e))?;
    Ok(json!({"catalogs": catalogs}))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        spec(
            "plugin.list",
            "Plug-ins",
            "{} → {plugins: [{id, name, version, kind (command|effect), permissions, commands, source, sha256, stored, restartNeeded}], problems, catalogs, stored (kept in the Plug-ins folder; else this session only), folder, last}",
            list,
        ),
        spec(
            "plugin.install",
            "Install Plug-in",
            "{path?, data? (base64 .craftplugin), name?, repo? (owner/repo or owner/repo@tag: its release's .craftplugin, checked against SHA256SUMS), catalog? (index.json URL), id? (a package of the catalog), version? (default: the newest for EffectCraft), review?: bool (check and describe it without installing: the confirmation), sha256? (install only that bundle)} → {id, name, version, kind, permissions, commands, source, sha256, inCatalog, replaces, permissionsChanged, stored, installed, restartNeeded?}",
            install,
        ),
        spec(
            "plugin.update",
            "Update Plug-ins",
            "{id?, apply?: bool (install the updates that ask for no other permissions)} — checks each plug-in's catalog or repository → {updates: [{id, installed, available, install (plugin.install params), permissionsChanged?}], updated, errors}",
            update,
        ),
        spec(
            "plugin.remove",
            "Remove Plug-in",
            "{id} → {removed, restartNeeded} — removes it and its settings (an effect plug-in's effect stays until the next start)",
            remove,
        ),
        spec(
            "plugin.run",
            "Run Plug-in",
            "{id, command? (default: the first), params? (the command's parameters), fuel? (a lower instruction budget)} → {output, notes, logs, calls, fuel} — a command plug-in's run is one undo step, rolled back when it fails",
            run,
        ),
        spec(
            "plugin.catalog.list",
            "Plug-in Catalogs",
            "{url?} → {catalogs, packages: [{catalog, id, name, version, author, description, license, repo, kind, sha256, installed}], warnings} — the packages EffectCraft can run in that catalog, or in every catalog added",
            catalog_list,
        ),
        spec("plugin.catalog.add", "Add Plug-in Catalog", "{url (https:// on GitHub, or file://, index.json), remove?: bool} → {catalogs}", catalog_add),
    ]
}

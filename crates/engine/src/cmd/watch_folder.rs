//! File ▸ Watch Folder…: render farm without a farm. EffectCraft watches a folder for project
//! files (`.ecproj`, at the top level or one folder down, e.g. the folders File ▸ Dependencies ▸
//! Collect Files writes) whose Render Queue has queued items, renders them, and writes a status
//! file next to each project (`<project>.status.json`: `rendering`, then `done` or `failed`, with
//! every item's status and output path). A project with a status file is not picked up again;
//! delete the status file to render it once more.
//!
//! [`poll`] does one pass and is what the frontends call on a timer while a folder is watched
//! (`file.watchFolder.poll`); it is headless and synchronous, so tests and the CLI use it
//! directly.

use std::path::{Path, PathBuf};

use effectcraft_project::render_queue::RenderStatus;
use serde_json::{Value, json};

use super::{CommandSpec, always, bad, str_p};
use crate::{Result, Session, cmd};

/// The status file of a project.
pub fn status_path(project: &Path) -> PathBuf {
    let mut s = project.as_os_str().to_owned();
    s.push(".status.json");
    PathBuf::from(s)
}

/// Projects in `folder` (and its direct subfolders) without a status file, sorted.
pub fn pending(folder: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut out = vec![];
    let scan = |dir: &Path, out: &mut Vec<PathBuf>, subdirs: &mut Vec<PathBuf>| -> std::io::Result<()> {
        for e in std::fs::read_dir(dir)? {
            let p = e?.path();
            if p.is_dir() {
                subdirs.push(p);
            } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("ecproj")) && !status_path(&p).exists() {
                out.push(p);
            }
        }
        Ok(())
    };
    let mut subdirs = vec![];
    scan(folder, &mut out, &mut subdirs)?;
    for d in subdirs {
        let mut ignore = vec![];
        let _ = scan(&d, &mut out, &mut ignore);
    }
    out.sort();
    Ok(out)
}

fn write_status(path: &Path, v: &Value) {
    let _ = std::fs::write(status_path(path), serde_json::to_vec_pretty(v).unwrap_or_default());
}

fn now() -> u64 {
    web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Render one project (in its own session sharing `host`'s services) and write its status.
fn render_project(host: &Session, path: &Path) -> Value {
    let name = path.to_string_lossy().to_string();
    write_status(path, &json!({"project": name, "state": "rendering", "started": now()}));
    let mut s = Session { services: host.services.clone(), footage: host.footage.clone(), expr: host.expr.clone(), ..Default::default() };
    s.importer = host.importer.clone();
    s.exporter = host.exporter.clone();
    s.accel = host.accel.clone();
    s.prefs = host.prefs.clone();
    let result = (|| -> std::result::Result<Value, String> {
        s.execute("file.open", json!({"path": name})).map_err(|e| e.to_string())?;
        let queued = s.project.render_queue.iter().filter(|i| i.render && i.status == RenderStatus::Queued).count();
        if queued == 0 {
            return Ok(json!({"state": "done", "items": [], "note": "nothing queued"}));
        }
        s.start_render(true)?;
        let items: Vec<Value> = s
            .project
            .render_queue
            .iter()
            .map(|i| {
                json!({
                    "comp": s.project.item(i.comp).map(|c| c.name.clone()),
                    "status": i.status.label(),
                    "error": if let RenderStatus::Failed(e) = &i.status { Some(e.clone()) } else { None },
                    "output": i.last_output,
                })
            })
            .collect();
        let failed = s.project.render_queue.iter().any(|i| matches!(i.status, RenderStatus::Failed(_)));
        Ok(json!({"state": if failed { "failed" } else { "done" }, "items": items}))
    })();
    let mut v = match result {
        Ok(v) => v,
        Err(e) => json!({"state": "failed", "error": e, "items": []}),
    };
    v["project"] = json!(name);
    v["finished"] = json!(now());
    write_status(path, &v);
    v
}

/// One pass over `folder`: render every new project; returns their statuses.
pub fn poll(s: &mut Session, folder: &str) -> std::result::Result<Vec<Value>, String> {
    let projects = pending(Path::new(folder)).map_err(|e| format!("cannot read {folder}: {e}"))?;
    Ok(projects.iter().map(|p| render_project(s, p)).collect())
}

fn watch(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "file.watchFolder";
    if p.get("stop").and_then(Value::as_bool) == Some(true) {
        s.state.watch_folder = None;
        s.bump();
        return Ok(json!({"watching": null}));
    }
    let folder = str_p(p, "folder").or(str_p(p, "path")).ok_or_else(|| bad(C, "missing `folder`"))?.to_string();
    if !Path::new(&folder).is_dir() {
        return Err(bad(C, format!("{folder} is not a folder")));
    }
    s.state.watch_folder = Some(folder.clone());
    s.bump();
    // The first pass right away.
    let done = poll(s, &folder).map_err(|e| bad(C, e))?;
    s.toast(format!("Watching {folder}"));
    Ok(json!({"watching": folder, "rendered": done}))
}

fn poll_cmd(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "file.watchFolder.poll";
    let folder = str_p(p, "folder").map(str::to_string).or_else(|| s.state.watch_folder.clone()).ok_or_else(|| bad(C, "no folder is being watched"))?;
    let done = poll(s, &folder).map_err(|e| bad(C, e))?;
    Ok(json!({"watching": folder, "rendered": done}))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "file.watchFolder",
            "Watch Folder...",
            ["File"],
            None,
            "{folder, stop?: bool} → watches the folder for .ecproj files with queued renders, renders them and writes `<project>.status.json`",
            always,
            watch
        ),
        cmd!(
            "file.watchFolder.poll",
            "Poll Watch Folder",
            [],
            None,
            "{folder? (default: the watched one)} → renders new projects now: {watching, rendered: [{project, state: done|failed, items: [{comp, status, output}]}]}",
            |s: &Session| if s.state.watch_folder.is_some() { Ok(()) } else { Err("no folder is being watched".into()) },
            poll_cmd
        ),
    ]
}

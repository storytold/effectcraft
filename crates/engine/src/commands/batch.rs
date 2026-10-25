//! `engine.batch`: run several commands in one call, as one undo step, with later steps able to
//! use what earlier ones returned (`"$1.layer"` is step 1's `layer` result). Agents build a
//! layer, animate it and style it in one round trip; a failing step rolls everything back.

use std::sync::Arc;

use serde_json::{Value, json};

use super::{CommandSpec, always, bad, str_p};
use crate::{EngineError, Result, Session};

/// Commands that can't run inside a batch (they replace the project or nest batches).
const NOT_IN_BATCH: &[&str] = &["engine.batch", "file.runScript", "script.run"];

/// Commands an atomic batch (the default) refuses before running any step: they write files,
/// render or open other applications, which rolling the project back can't undo (#1232). They
/// can still run in a batch with `atomic: false`, whose failures keep what earlier steps did.
/// `file.autoSave` isn't here: it only writes a recovery copy into the auto-save folder, which
/// the next auto-save replaces.
pub const NOT_IN_ATOMIC: &[&str] = &[
    // Files written.
    "file.save",
    "file.saveAs",
    "file.saveCopy",
    "file.saveCopyAsXml",
    "file.incrementAndSave",
    "file.collectFiles",
    "file.exportLottie",
    "file.exportTimeline",
    "file.createProxy",
    "comp.saveFrameAs",
    "comp.saveFrameAsPsd",
    "comp.saveFrameAsExr",
    "anim.savePreset",
    "view.exportGuides",
    "render.saveCurrentPreview",
    "render.preRender",
    "renderQueue.render",
    "essential.exportTemplate",
    "shortcuts.export",
    "templates.saveAs",
    "templates.delete",
    "file.installScript",
    "file.installScriptUIPanel",
    "file.uninstallScript",
    // Other applications.
    "file.executeFile",
    "file.revealInFinder",
    "layer.openSource",
    "help.revealLogFile",
];

/// A step's command id.
fn step_id(step: &Value) -> Option<&str> {
    step.get("command").or_else(|| step.get("id")).and_then(Value::as_str)
}

/// Resolve a `$N` / `$N.key.0.key` reference against the results so far.
fn reference(s: &str, results: &[Value]) -> Option<Result<Value>> {
    let rest = s.strip_prefix('$')?;
    let mut parts = rest.split('.');
    let n: usize = parts.next()?.parse().ok()?;
    let path: Vec<&str> = parts.collect();
    if path.iter().any(|p| p.is_empty() || !p.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')) {
        return None;
    }
    let Some(mut v) = n.checked_sub(1).and_then(|i| results.get(i)) else {
        return Some(Err(bad("engine.batch", format!("`{s}` refers to step {n}, but only {} step(s) ran before", results.len()))));
    };
    for p in &path {
        let next = match (v, p.parse::<usize>()) {
            (Value::Array(a), Ok(i)) => a.get(i),
            (Value::Object(o), _) => o.get(*p),
            _ => None,
        };
        match next {
            Some(x) => v = x,
            None => return Some(Err(bad("engine.batch", format!("`{s}`: step {n} returned {v}, which has no `{p}`")))),
        }
    }
    Some(Ok(v.clone()))
}

/// Replace every `$N…` string in `v` (recursively) with the referenced result.
fn substitute(v: &Value, results: &[Value]) -> Result<Value> {
    Ok(match v {
        Value::String(s) => match reference(s, results) {
            Some(r) => r?,
            None => v.clone(),
        },
        Value::Array(a) => Value::Array(a.iter().map(|x| substitute(x, results)).collect::<Result<_>>()?),
        Value::Object(o) => Value::Object(o.iter().map(|(k, x)| Ok((k.clone(), substitute(x, results)?))).collect::<Result<_>>()?),
        _ => v.clone(),
    })
}

fn batch(s: &mut Session, p: &Value) -> Result<Value> {
    let steps = p.get("steps").and_then(Value::as_array).ok_or_else(|| bad("engine.batch", "missing `steps`: [{command, params?}, …]"))?.clone();
    let label = str_p(p, "label").unwrap_or("Batch").to_string();
    let atomic = p.get("atomic").and_then(Value::as_bool).unwrap_or(true);
    if atomic && let Some((i, id)) = steps.iter().enumerate().find_map(|(i, st)| step_id(st).filter(|id| NOT_IN_ATOMIC.contains(id)).map(|id| (i, id))) {
        return Err(bad(
            "engine.batch",
            format!(
                "step {} (`{id}`) writes files or acts outside the project, which a rollback can't undo: run it on its own, or use `atomic: false`; nothing ran",
                i + 1
            ),
        ));
    }
    // The document as it is: a step may replace the project (File ▸ Open, New, a template),
    // so the rollback restores its file and saved state with its content (#1232).
    let (project, history, state) = (s.project.clone(), s.history.clone(), s.state.clone());
    let (path, saved_project) = (s.path.clone(), s.saved_project.clone());
    let mut results: Vec<Value> = Vec::with_capacity(steps.len());
    for (i, step) in steps.iter().enumerate() {
        let run = |s: &mut Session, results: &[Value]| -> Result<Value> {
            let id = step_id(step).ok_or_else(|| bad("engine.batch", "each step needs `command` (a command id)"))?;
            if NOT_IN_BATCH.contains(&id) {
                return Err(bad("engine.batch", format!("`{id}` can't run inside a batch")));
            }
            let params = substitute(step.get("params").unwrap_or(&json!({})), results)?;
            s.execute_checked(id, params)
        };
        match run(s, &results) {
            Ok(r) => results.push(r),
            Err(e) => {
                let id = step_id(step).unwrap_or("?");
                let note = if atomic {
                    s.project = project;
                    s.history = history;
                    s.state = state;
                    s.path = path;
                    s.saved_project = saved_project;
                    s.sanitize_state();
                    // Saved again exactly when the restored project is the saved one.
                    s.bump();
                    "; the batch was rolled back (nothing changed)".to_string()
                } else {
                    fold(s, &project, &label);
                    format!("; steps 1–{i} were applied")
                };
                return Err(EngineError::Other(format!("step {} ({id}): {e}{note}", i + 1)));
            }
        }
    }
    fold(s, &project, &label);
    Ok(json!({"steps": results.len(), "results": results}))
}

/// Fold the undo steps recorded since the project was `snap` into one step called `label`.
fn fold(s: &mut Session, snap: &Arc<effectcraft_project::Project>, label: &str) {
    let h = &mut s.history;
    let Some(start) = h.undo.iter().rposition(|(_, p)| Arc::ptr_eq(p, snap)) else { return };
    let first = h.undo[start].1.clone();
    h.undo.truncate(start);
    h.undo.push((label.to_string(), first));
    h.merge_key = None;
}

pub fn specs() -> Vec<CommandSpec> {
    vec![CommandSpec {
        id: "engine.batch",
        label: "Run Commands (Batch)",
        menu: &[],
        shortcut: None,
        alt_shortcuts: &[],
        params: "{steps: [{command, params?}], label? (undo step name, default Batch), atomic?: bool (default true: a failing step rolls the whole batch back, project file and saved state included; commands that write files or open other applications are refused, so use false for those)} → {steps, results: [each step's result]}; a string param \"$N\" or \"$N.key.0\" is step N's result (1-based), e.g. {\"layer\": \"$1.layer\"}",
        enabled: always,
        run: batch,
        // The steps journal themselves.
        journal: false,
    }]
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::Session;

    #[test]
    fn batch_refs_one_undo_step_and_rollback() {
        let mut s = Session::default();
        s.execute("comp.new", json!({"name": "B", "width": 100, "height": 100})).unwrap();
        let undo_before = s.history.undo.len();
        let r = s
            .execute_checked(
                "engine.batch",
                json!({"label": "Build", "steps": [
                    {"command": "layer.newSolid", "params": {"name": "Box", "color": "#ff0000"}},
                    {"command": "prop.set", "params": {"layer": "$1.layer", "path": "transform/opacity", "value": 40}},
                    {"command": "layer.rename", "params": {"layer": "$1.layer", "name": "Red Box"}},
                ]}),
            )
            .unwrap();
        assert_eq!(r["steps"], 3);
        let lid = r["results"][0]["layer"].as_u64().unwrap();
        let comp = s.active_comp().unwrap();
        let l = comp.layer(effectcraft_project::LayerId(lid)).unwrap();
        assert_eq!(l.name, "Red Box");
        assert_eq!(l.props.prop("transform/opacity").unwrap().value.as_f64(), 40.0);
        assert_eq!(s.history.undo.len(), undo_before + 1, "one undo step");
        assert_eq!(s.history.undo.last().unwrap().0, "Build");
        assert!(s.undo());
        assert!(s.active_comp().unwrap().layers.is_empty());
        assert!(s.redo());

        // A failing step rolls back the earlier ones and names the step.
        let n = s.active_comp().unwrap().layers.len();
        let e = s
            .execute_checked(
                "engine.batch",
                json!({"steps": [
                    {"command": "layer.newNull", "params": {}},
                    {"command": "prop.set", "params": {"layer": "$1.layer", "path": "transform/nope", "value": 1}},
                ]}),
            )
            .unwrap_err()
            .to_string();
        assert!(e.starts_with("step 2 (prop.set)") && e.contains("rolled back"), "{e}");
        assert_eq!(s.active_comp().unwrap().layers.len(), n);
        // Bad references are reported.
        let e = s.execute_checked("engine.batch", json!({"steps": [{"command": "layer.rename", "params": {"layer": "$3.layer", "name": "x"}}]})).unwrap_err();
        assert!(e.to_string().contains("refers to step 3"), "{e}");
    }

    fn tmp(name: &str) -> String {
        let d = std::env::temp_dir().join(format!("ec-batch-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d.join(name).to_string_lossy().into_owned()
    }

    /// A step that fails: no such layer.
    fn failing() -> serde_json::Value {
        json!({"command": "layer.rename", "params": {"layer": 999_999, "name": "x"}})
    }

    #[test]
    fn a_rolled_back_batch_that_opened_another_project_restores_the_whole_document() {
        // Project B on disk.
        let b_path = tmp("b.ecproj");
        let mut b = Session::default();
        b.execute("comp.new", json!({"name": "B comp", "width": 32, "height": 32})).unwrap();
        b.execute("file.saveAs", json!({"path": b_path})).unwrap();
        for dirty in [false, true] {
            // Project A, saved, then (or not) changed.
            let a_path = tmp(&format!("a-{dirty}.ecproj"));
            let mut s = Session::default();
            s.execute("comp.new", json!({"name": "A comp", "width": 32, "height": 32})).unwrap();
            s.execute("file.saveAs", json!({"path": a_path})).unwrap();
            if dirty {
                s.execute("layer.newSolid", json!({"name": "Unsaved"})).unwrap();
            }
            let (project, undo) = (s.project.clone(), s.history.undo.len());
            assert_eq!(s.is_dirty(), dirty);
            // Open B, then fail (#1232).
            let e =
                s.execute_checked("engine.batch", json!({"steps": [{"command": "file.open", "params": {"path": b_path}}, failing()]})).unwrap_err().to_string();
            assert!(e.contains("rolled back"), "{e}");
            assert!(std::sync::Arc::ptr_eq(&s.project, &project), "A's content");
            assert_eq!(s.path.as_deref(), Some(a_path.as_str()), "A's file, not B's");
            assert_eq!(s.is_dirty(), dirty, "A's saved state");
            assert_eq!(s.history.undo.len(), undo);
            // So saving writes A to A's file and leaves B alone.
            let b_before = std::fs::read(&b_path).unwrap();
            s.execute("file.save", json!({})).unwrap();
            assert_eq!(std::fs::read(&b_path).unwrap(), b_before);
            assert!(std::fs::read_to_string(&a_path).unwrap().contains("A comp"));
            assert!(!s.is_dirty());
        }
    }

    #[test]
    fn an_atomic_batch_refuses_steps_it_cannot_roll_back_before_running_any() {
        let mut s = Session::default();
        s.execute("comp.new", json!({"name": "C", "width": 32, "height": 32})).unwrap();
        let path = tmp("refused.ecproj");
        let _ = std::fs::remove_file(&path);
        let layers = |s: &Session| s.active_comp().unwrap().layers.len();
        let steps = json!([{"command": "layer.newSolid", "params": {"name": "First"}}, {"command": "file.saveAs", "params": {"path": path}}]);
        let e = s.execute_checked("engine.batch", json!({"steps": steps})).unwrap_err().to_string();
        assert!(e.contains("step 2 (`file.saveAs`)") && e.contains("nothing ran"), "{e}");
        assert_eq!(layers(&s), 0, "step 1 didn't run");
        assert!(!std::path::Path::new(&path).exists());
        // Without atomicity it runs, as before.
        s.execute_checked("engine.batch", json!({"steps": steps, "atomic": false})).unwrap();
        assert_eq!(layers(&s), 1);
        assert!(std::path::Path::new(&path).exists());
        // Every listed command exists.
        for id in super::NOT_IN_ATOMIC {
            assert!(crate::commands::find(id).is_some(), "{id} is not a command");
        }
    }
}

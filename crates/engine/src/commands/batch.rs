//! `engine.batch`: run several commands in one call, as one undo step, with later steps able to
//! use what earlier ones returned (`"$1.layer"` is step 1's `layer` result). Agents build a
//! layer, animate it and style it in one round trip; a failing step rolls everything back.

use std::sync::Arc;

use serde_json::{Value, json};

use super::{CommandSpec, always, bad, str_p};
use crate::{EngineError, Result, Session};

/// Commands that can't run inside a batch (they replace the project or nest batches).
const NOT_IN_BATCH: &[&str] = &["engine.batch", "file.runScript", "script.run"];

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
    let (project, history, state) = (s.project.clone(), s.history.clone(), s.state.clone());
    // Rolled-back steps may have queued events (an `OpenComp`, a Toast) for the state that is
    // about to be dropped: the UI must not act on them.
    let event_mark = s.events.len();
    let mut results: Vec<Value> = Vec::with_capacity(steps.len());
    for (i, step) in steps.iter().enumerate() {
        let run = |s: &mut Session, results: &[Value]| -> Result<Value> {
            let id = step
                .get("command")
                .or_else(|| step.get("id"))
                .and_then(Value::as_str)
                .ok_or_else(|| bad("engine.batch", "each step needs `command` (a command id)"))?;
            if NOT_IN_BATCH.contains(&id) {
                return Err(bad("engine.batch", format!("`{id}` can't run inside a batch")));
            }
            let params = substitute(step.get("params").unwrap_or(&json!({})), results)?;
            s.execute_checked(id, params)
        };
        match run(s, &results) {
            Ok(r) => results.push(r),
            Err(e) => {
                let id = step.get("command").or_else(|| step.get("id")).and_then(Value::as_str).unwrap_or("?");
                let note = if atomic {
                    // The snapshot is the intermediate state the failing step left: comparing it
                    // with the restored editor detects a restart or cross-comp switch inside the
                    // batch, so the pre-batch token is never revived. Do not restore
                    // `ime_revision`: a transient text edit inside the batch already advanced
                    // this process-monotonic high-water mark, and rewinding it would reuse a
                    // stamp (letting a wide-selection composition look current). An unrelated
                    // failed batch did not advance it, so its value stays exactly as it was.
                    let edited = crate::commands::text_edit::active_editor_snapshot(s);
                    let interim_revision = s.ime_revision;
                    s.project = project;
                    s.history = history;
                    s.state = state;
                    s.events.truncate(event_mark);
                    s.sanitize_after_replacement(edited.as_ref());
                    s.ime_revision = s.ime_revision.max(interim_revision);
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
        params: "{steps: [{command, params?}], label? (undo step name, default Batch), atomic?: bool (default true: a failing step rolls the whole batch back)} → {steps, results: [each step's result]}; a string param \"$N\" or \"$N.key.0\" is step N's result (1-based), e.g. {\"layer\": \"$1.layer\"}",
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
}

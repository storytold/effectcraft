//! The command journal as a replay script (see [`crate::replay`]): `journal.export` for agents
//! and `help.exportRepro` for bug reports. Neither is journaled.

use serde_json::{Value, json};

use super::{CommandSpec, always, str_p};
use crate::{Result, Session, replay};

/// `journal.export {path?}`: the script itself, or written to `path`.
fn export(s: &mut Session, p: &Value) -> Result<Value> {
    if let Some(path) = str_p(p, "path") {
        let (path, steps) = replay::save_repro(s, Some(path))?;
        return Ok(json!({"path": path, "steps": steps}));
    }
    let script = replay::export(s, None);
    Ok(json!({"steps": script.lines().count().saturating_sub(1), "script": script}))
}

/// `help.exportRepro {path?}`: save the steps so far for a bug report (default: the settings
/// folder's `Repros`).
fn export_repro(s: &mut Session, p: &Value) -> Result<Value> {
    let (path, steps) = replay::save_repro(s, str_p(p, "path"))?;
    Ok(json!({"path": path, "steps": steps}))
}

const fn spec(id: &'static str, label: &'static str, params: &'static str, run: super::Run) -> CommandSpec {
    CommandSpec { id, label, menu: &[], shortcut: None, alt_shortcuts: &[], params, enabled: always, run, journal: false }
}

pub fn specs() -> Vec<CommandSpec> {
    #[allow(unused_mut)]
    let mut v = vec![
        spec(
            "journal.export",
            "Export Command Journal",
            "{path? (write the script there)} → {script, steps} or {path, steps}: the commands run since the project was last opened, as a replay script (docs/agents.md, Repro scripts)",
            export,
        ),
        spec(
            "help.exportRepro",
            "Save Bug Repro...",
            "{path? (default: Repros in the settings folder)} → {path, steps}: the steps since the project was opened, for a bug report (commands and parameters only)",
            export_repro,
        ),
    ];
    // A command with a bug, for the crash-repro tests.
    #[cfg(test)]
    v.push(spec("test.panic", "Panic (test)", "{why?}", |_, p| {
        let why = str_p(p, "why").unwrap_or("test").to_string();
        std::panic::panic_any(why)
    }));
    v
}

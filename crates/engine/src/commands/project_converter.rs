//! File ▸ Import ▸ Converted Project…: runs an installed project converter.
//!
//! A project converter is a separate tool that describes a project from another application as
//! JSON and ships a script that rebuilds it here. That script is installed like any other (it
//! goes in File ▸ Scripts), so the converter and its format change without EffectCraft changing;
//! this menu item only runs it.

use serde_json::Value;

use super::{CommandSpec, always, bad};
use crate::{Result, Session, cmd};

/// The installed converter's import script.
pub const SCRIPT: &str = "Import Converted Project.jsx";

fn import(s: &mut Session, _: &Value) -> Result<Value> {
    if super::scripts::script_code(s, SCRIPT, Some(false)).is_none() {
        return Err(bad("file.importConvertedProject", format!("no project converter is installed: install its import script as {SCRIPT} in File ▸ Scripts")));
    }
    super::scripts::run_named(s, SCRIPT)
}

pub fn specs() -> Vec<CommandSpec> {
    vec![cmd!(
        "file.importConvertedProject",
        "Converted Project...",
        ["File", "Import"],
        None,
        "{} → runs the installed converter script `Import Converted Project.jsx`, which asks for a converted project (JSON), rebuilds it and reports what didn't come across",
        always,
        import
    )]
}

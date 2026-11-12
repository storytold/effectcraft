use super::{EngineError, Result, Session, Value};

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

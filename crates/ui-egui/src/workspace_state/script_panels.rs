//! ScriptUI panels in saved workspaces. [`PanelKind::ScriptPanel`] holds a script window id, which
//! means something only in the process that made the window, so a snapshot keeps each panel's
//! script name beside the layouts (`script_panels`, id → name). The name is the only thing that
//! carries over; it is compared with open windows and never runs a script.
//!
//! - [`bind`] gives each open script its references back at its window's id, in every workspace.

use std::collections::{BTreeMap, BTreeSet};

use effectcraft_engine::scriptui::{ScriptUi, WindowKind};
use serde_json::Value;

use crate::dock::{PanelKind, Workspace};
use crate::state::UiState;

/// A dock tree level nests its panels a few JSON levels deeper.
const MAX_JSON_DEPTH: usize = 4 * craft_ui::docking::MAX_DEPTH + 16;

/// Sends each `{"ScriptPanel": id}` under `value` through `map`; whether any id changed.
fn remap_value(value: &mut Value, map: &mut impl FnMut(u32) -> u32, depth: usize) -> Result<bool, String> {
    if depth > MAX_JSON_DEPTH {
        return Err("workspace nests too deeply".into());
    }
    let mut changed = false;
    match value {
        Value::Object(fields) => {
            if fields.len() == 1
                && let Some(id) = fields.get_mut("ScriptPanel")
            {
                let Some(old) = id.as_u64().and_then(|id| u32::try_from(id).ok()) else { return Ok(false) };
                let new = map(old);
                *id = new.into();
                return Ok(new != old);
            }
            for field in fields.values_mut() {
                changed |= remap_value(field, map, depth.saturating_add(1))?;
            }
        }
        Value::Array(items) => {
            for item in items {
                changed |= remap_value(item, map, depth.saturating_add(1))?;
            }
        }
        _ => {}
    }
    Ok(changed)
}

/// Sends every panel reference of `workspace` through `map`, by the serde path the file uses, so
/// no place that can hold a panel is missed: trees, floating windows, `hidden` with its anchors
/// and siblings, `saved`. A failure, or a result that isn't a valid workspace, changes nothing.
fn remap(workspace: &mut Workspace, map: &mut impl FnMut(u32) -> u32) -> Result<(), String> {
    let mut value = serde_json::to_value(&*workspace).map_err(|e| e.to_string())?;
    if remap_value(&mut value, map, 0)? {
        let remapped: Workspace = serde_json::from_value(value).map_err(|e| e.to_string())?;
        remapped.validate().map_err(|e| e.to_string())?;
        *workspace = remapped;
    }
    Ok(())
}

fn remap_panel(panel: PanelKind, map: &mut impl FnMut(u32) -> u32) -> PanelKind {
    match panel {
        PanelKind::ScriptPanel(id) => PanelKind::ScriptPanel(map(id)),
        panel => panel,
    }
}

/// Each open script's references move to its window's id, keeping one id per script. Idempotent.
/// A script with several panel windows binds the last, the one `window.scriptPanel` docks.
pub(crate) fn bind(ui: &mut UiState, script_ui: &ScriptUi) {
    let live: BTreeMap<&str, u32> = script_ui.windows.iter().filter(|w| w.kind == WindowKind::Panel).map(|w| (w.script.as_str(), w.id)).collect();
    let bound: BTreeMap<&str, u32> = ui.script_panels.iter().map(|(id, name)| (name.as_str(), *id)).collect();
    let moved: BTreeMap<u32, u32> = live.iter().filter_map(|(name, id)| bound.get(name).filter(|old| *old != id).map(|old| (*old, *id))).collect();
    let new: Vec<(u32, String)> = live.iter().filter(|(name, id)| bound.get(*name) != Some(*id)).map(|(name, id)| (*id, name.to_string())).collect();
    ui.script_panels.retain(|id, _| !moved.contains_key(id));
    ui.script_panels.extend(new);
    if moved.is_empty() {
        return;
    }
    let mut map = |id| moved.get(&id).copied().unwrap_or(id);
    for workspace in std::iter::once(&mut ui.dock).chain(ui.saved_workspaces.values_mut()) {
        // Only a workspace already showing the window can fail; its stale id is then dropped at load.
        if let Err(error) = remap(workspace, &mut map) {
            log::warn!("A ScriptUI panel keeps its old window id in a workspace: {error}");
        }
    }
    ui.focused = remap_panel(ui.focused, &mut map);
    ui.maximized = ui.maximized.map(|panel| remap_panel(panel, &mut map));
}

/// The names of the panels `ui` still refers to, as the file holds them.
pub(super) fn captured(ui: &UiState) -> BTreeMap<u32, String> {
    if ui.script_panels.is_empty() {
        return BTreeMap::new();
    }
    let mut referenced = BTreeSet::new();
    let mut note = |id| {
        referenced.insert(id);
        id
    };
    for workspace in std::iter::once(&ui.dock).chain(ui.saved_workspaces.values()) {
        let walked = serde_json::to_value(workspace).map_err(|e| e.to_string()).and_then(|mut value| remap_value(&mut value, &mut note, 0));
        if walked.is_err() {
            return ui.script_panels.clone();
        }
    }
    for panel in std::iter::once(ui.focused).chain(ui.maximized) {
        remap_panel(panel, &mut note);
    }
    ui.script_panels.iter().filter(|(id, _)| referenced.contains(*id)).map(|(id, name)| (*id, name.clone())).collect()
}

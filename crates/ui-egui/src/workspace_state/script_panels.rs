//! ScriptUI panels in saved workspaces. [`PanelKind::ScriptPanel`] holds a script window id, which
//! means something only in the process that made the window, so a snapshot keeps each panel's
//! script name beside the layouts (`script_panels`, id → name). The name is the only thing that
//! carries over; it is compared with open windows and never runs a script.
//!
//! - [`detach`] moves every id of a file into a range no window reaches, all at once, so an id
//!   from another process can't name a window of this one. An id without a name, or repeating a
//!   name, says nothing about which script it was and is dropped.
//! - [`bind`] gives each open script its references back at its window's id, in every workspace.
//! - [`fit`] hides what the current workspace shows of scripts that aren't open. Their place stays
//!   in `hidden` and in the stored workspaces, for [`bind`] to find when the script opens, and
//!   brings back a hidden panel whose script is open.

use std::collections::{BTreeMap, BTreeSet};

use effectcraft_engine::Session;
use effectcraft_engine::scriptui::{ScriptUi, WindowKind};
use serde_json::Value;

use super::Snapshot;
use crate::dock::{PanelKind, Workspace, WorkspaceExt as _};
use crate::state::UiState;

#[cfg(test)]
mod tests;

/// Script window ids count up from 1 in each process and never get here.
const DETACHED: u32 = 1 << 31;
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

/// Moves the panels `shown` rejects to `hidden`, with their places. craft-ui's `reconcile` does
/// this without going through the lock; an empty default adds nothing back, and it would replace
/// an invalid workspace with that default.
fn hide_unless(workspace: &mut Workspace, shown: impl Fn(&PanelKind) -> bool) {
    if workspace.validate().is_ok() && crate::dock::referenced_panels(workspace).iter().any(|panel| !shown(panel)) {
        workspace.reconcile(shown, crate::dock::permissions, &Workspace::default());
    }
}

/// The table as far as it reads: an entry that isn't an id and a name is an id without a name.
pub(super) fn read_names<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<BTreeMap<u32, String>, D::Error> {
    let table = <Value as serde::Deserialize>::deserialize(deserializer)?;
    Ok(table.as_object().into_iter().flatten().filter_map(|(id, name)| Some((id.parse().ok()?, name.as_str()?.to_string()))).collect())
}

/// See the module docs. Runs on a validated snapshot; an `Err` rejects the file.
pub(super) fn detach(snapshot: &mut Snapshot) -> Result<(), String> {
    let (mut moved, mut names, mut seen) = (BTreeMap::new(), BTreeMap::new(), BTreeSet::new());
    let next = |moved: &BTreeMap<u32, u32>| DETACHED.saturating_add(u32::try_from(moved.len()).unwrap_or(u32::MAX));
    for (id, name) in std::mem::take(&mut snapshot.script_panels) {
        if seen.insert(name.clone()) {
            let to = next(&moved);
            moved.insert(id, to);
            names.insert(to, name);
        }
    }
    // An id without a name moves too: left as it was, a later window could take its number.
    let mut map = |id| {
        let to = next(&moved);
        *moved.entry(id).or_insert(to)
    };
    for workspace in std::iter::once(&mut snapshot.dock).chain(snapshot.saved_workspaces.values_mut()) {
        remap(workspace, &mut map)?;
    }
    snapshot.focused = remap_panel(snapshot.focused, &mut map);
    snapshot.maximized = snapshot.maximized.map(|panel| remap_panel(panel, &mut map));

    let named = |panel: &PanelKind| !matches!(panel, PanelKind::ScriptPanel(id) if !names.contains_key(id));
    for workspace in std::iter::once(&mut snapshot.dock).chain(snapshot.saved_workspaces.values_mut()) {
        hide_unless(workspace, named);
        workspace.hidden.retain(|(panel, _)| named(panel));
    }
    if !named(&snapshot.focused) {
        snapshot.focused = UiState::default().focused;
    }
    snapshot.maximized = snapshot.maximized.filter(named);
    snapshot.script_panels = names;
    Ok(())
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

/// `ui.dock` shows no panel whose script window isn't open, and gives a tab to each one whose
/// window is open and that it holds a place for, behind the tab its group shows. Focus and the
/// maximized panel follow.
pub(crate) fn fit(ui: &mut UiState, script_ui: &ScriptUi) {
    let live = |panel: &PanelKind| !matches!(panel, PanelKind::ScriptPanel(id) if script_ui.window(*id).is_none());
    hide_unless(&mut ui.dock, live);
    let waiting: Vec<PanelKind> =
        ui.dock.hidden.iter().map(|(panel, _)| *panel).filter(|panel| matches!(panel, PanelKind::ScriptPanel(_)) && live(panel)).collect();
    let front: Vec<PanelKind> = ui.dock.panels().into_iter().filter(|panel| ui.dock.is_visible(*panel)).collect();
    for panel in waiting {
        if let Err(error) = ui.dock.set_visible(panel, true, None, crate::dock::permissions) {
            log::warn!("An open ScriptUI panel stays hidden: {error}");
        }
    }
    for panel in front {
        if !ui.dock.is_visible(panel) {
            let _ = ui.dock.apply(craft_ui::docking::Action::Activate { panel }, crate::dock::permissions);
        }
    }
    if !live(&ui.focused) {
        ui.focused = UiState::default().focused;
    }
    ui.maximized = ui.maximized.filter(|panel| ui.dock.is_docked(*panel));
}

/// A new app's workspace meets the panels the frontend already opened. Their queued
/// `window.scriptPanel` events would bring each to the front and take the focus, so the ones the
/// restored workspace already placed are dropped; the others still dock.
pub(crate) fn restore(ui: &mut UiState, session: &mut Session) {
    bind(ui, &session.script_ui);
    fit(ui, &session.script_ui);
    session.events.retain(|event| {
        let effectcraft_engine::Event::Frontend { command, params } = event else { return true };
        let window = params.get("window").and_then(Value::as_u64).and_then(|id| u32::try_from(id).ok());
        command != "window.scriptPanel" || !window.is_some_and(|id| ui.dock.is_open(PanelKind::ScriptPanel(id)))
    });
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

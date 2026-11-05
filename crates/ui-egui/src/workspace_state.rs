//! Persistent workspace layouts, separate from document-bound and transient UI state.

use std::collections::BTreeMap;

use effectcraft_engine::config::ConfigStore;
use serde::{Deserialize, Serialize};

use crate::dock::{PanelKind, Workspace};
use crate::state::UiState;

mod legacy;

const FILE: &str = "workspaces.json";
/// An earlier version's file, kept when it is first rewritten in the current format.
const BACKUP: &str = "workspaces.old.json";
const MAX_BYTES: usize = 1024 * 1024;
const MAX_WORKSPACES: usize = 64;
/// Leave room for new viewers without approaching the u32 allocator limit.
const MAX_VIEWER_ID: u32 = 65_535;
/// The snapshot version: 2 holds craft-ui docking workspaces.
const VERSION: u32 = 2;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Snapshot {
    version: u32,
    workspace: String,
    dock: Workspace,
    focused: PanelKind,
    maximized: Option<PanelKind>,
    saved_workspaces: BTreeMap<String, Workspace>,
}

impl Snapshot {
    fn capture(ui: &UiState) -> Self {
        Self {
            version: VERSION,
            workspace: ui.workspace.clone(),
            dock: ui.dock.clone(),
            focused: ui.focused,
            maximized: ui.maximized,
            saved_workspaces: ui.saved_workspaces.clone(),
        }
    }

    fn apply(self, ui: &mut UiState) {
        ui.workspace = self.workspace;
        ui.dock = self.dock;
        ui.focused = self.focused;
        ui.maximized = self.maximized;
        ui.saved_workspaces = self.saved_workspaces;
        // A native window's place is made safe to hand to the windowing system.
        for workspace in std::iter::once(&mut ui.dock).chain(ui.saved_workspaces.values_mut()) {
            for window in &mut workspace.layout.floating {
                window.native = window.native.map(crate::dock_ui::sanitized_window_rect);
            }
        }
        // Reserve restored viewer slots without restoring any project-specific composition
        // links. New Viewer allocates from this map, including after switching named layouts.
        for workspace in std::iter::once(&ui.dock).chain(ui.saved_workspaces.values()) {
            for panel in crate::dock::referenced_panels(workspace) {
                if let PanelKind::Viewer(id) = panel {
                    ui.viewers.entry(id).or_insert(None);
                }
            }
        }
    }

    fn validate(&self) -> Result<(), String> {
        if self.version != VERSION {
            return Err(format!("unsupported workspace version {}", self.version));
        }
        let name_ok = |name: &str| !name.trim().is_empty() && name.len() <= 256;
        if !name_ok(&self.workspace) || self.saved_workspaces.len() > MAX_WORKSPACES || self.saved_workspaces.keys().any(|name| !name_ok(name)) {
            return Err("workspace names or count exceed supported limits".into());
        }
        if !crate::dock::WORKSPACES.contains(&self.workspace.as_str()) && !self.saved_workspaces.contains_key(&self.workspace) {
            return Err("current workspace has no saved definition".into());
        }
        let panel_ok = |panel: PanelKind| match panel {
            PanelKind::Viewer(id) => (1..=MAX_VIEWER_ID).contains(&id),
            _ => true,
        };
        if !panel_ok(self.focused) || self.maximized.is_some_and(|panel| !panel_ok(panel)) {
            return Err("workspace viewer ID is outside the supported range 1–65535".into());
        }
        for workspace in std::iter::once(&self.dock).chain(self.saved_workspaces.values()) {
            // craft-ui checks the trees, selections, sizes, rects and limits.
            workspace.validate().map_err(|error| error.to_string())?;
            if crate::dock::referenced_panels(workspace).into_iter().any(|panel| !panel_ok(panel)) {
                return Err("workspace viewer ID is outside the supported range 1–65535".into());
            }
        }
        Ok(())
    }
}

pub(crate) struct WorkspaceStore {
    saved: Snapshot,
    last_attempt: f64,
    writable: bool,
    /// The text of an earlier version's file that loaded: the first save copies it to
    /// [`BACKUP`] and writes the file in the current format.
    migrated_from: Option<String>,
    pub(crate) error: Option<String>,
}

impl WorkspaceStore {
    pub(crate) fn load(config: Option<&dyn ConfigStore>, ui: &mut UiState) -> Self {
        let mut original = None;
        let mut outer = Vec::new();
        let loaded = config.map_or(Ok(None), |c| c.read_bounded(FILE, MAX_BYTES)).map_err(|e| e.to_string()).and_then(|text| {
            text.map(|text| {
                let value: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
                let snapshot = if legacy::is_legacy(&value) {
                    let (snapshot, windows) = legacy::migrate(value, ui)?;
                    original = Some(text);
                    outer = windows;
                    snapshot
                } else if value.get("version").is_none() {
                    return Err("not a workspace file".to_string());
                } else {
                    serde_json::from_value::<Snapshot>(value).map_err(|e| e.to_string())?
                };
                snapshot.validate()?;
                Ok(snapshot)
            })
            .transpose()
        });
        let error = match loaded {
            Ok(Some(snapshot)) => {
                snapshot.apply(ui);
                ui.native_outer = outer;
                None
            }
            Ok(None) => None,
            Err(e) => Some(format!("Could not load workspaces: {e}. The existing file will be preserved for this session.")),
        };
        if let Some(error) = &error {
            log::warn!("{error}");
            ui.status = error.clone();
            original = None;
        }
        // An unreadable or newer file is never replaced by this session, even after a later
        // layout edit. Ordinary failed writes remain dirty and retryable.
        Self { saved: Snapshot::capture(ui), last_attempt: f64::NEG_INFINITY, writable: error.is_none(), migrated_from: original, error }
    }

    pub(crate) fn due(&mut self, now: f64) -> bool {
        if now - self.last_attempt < 1.0 {
            return false;
        }
        self.last_attempt = now;
        true
    }

    pub(crate) fn has_changes(&self, ui: &UiState) -> bool {
        if !self.writable {
            return false;
        }
        self.migrated_from.is_some()
            || self.saved.workspace != ui.workspace
            || self.saved.dock != ui.dock
            || self.saved.focused != ui.focused
            || self.saved.maximized != ui.maximized
            || self.saved.saved_workspaces != ui.saved_workspaces
    }

    pub(crate) fn save(&mut self, config: Option<&dyn ConfigStore>, ui: &UiState) -> Result<bool, String> {
        let Some(config) = config else { return Ok(false) };
        // A window still at its outer position would be saved as if that were its content's.
        if !self.has_changes(ui) || !ui.native_outer.is_empty() {
            return Ok(false);
        }
        let snapshot = Snapshot::capture(ui);
        snapshot.validate()?;
        let text = serde_json::to_string_pretty(&snapshot).map_err(|e| format!("serialize workspaces: {e}"))?;
        if text.len() > MAX_BYTES {
            return Err("workspace file exceeds 1 MiB".into());
        }
        if let Some(original) = &self.migrated_from {
            config.write(BACKUP, original).map_err(|e| format!("keep the earlier workspace file: {e}"))?;
        }
        config.write(FILE, &text).map_err(|e| format!("save workspaces: {e}"))?;
        self.migrated_from = None;
        self.saved = snapshot;
        self.error = None;
        Ok(true)
    }

    /// An explicit save (Save Workspace, Save as New Workspace, Edit Workspaces): unlike
    /// [`Self::save`], no settings folder or a file blocked at load is an error, not "nothing to do".
    pub(crate) fn save_now(&mut self, config: Option<&dyn ConfigStore>, ui: &UiState) -> Result<(), String> {
        let Some(config) = config else {
            return Err("there is no settings folder to save them in".into());
        };
        if !self.writable {
            let path = config.dir().map_or_else(|| FILE.to_string(), |dir| dir.join(FILE).display().to_string());
            return Err(format!("{path} couldn't be loaded at startup, so it is left untouched; fix or move it and restart"));
        }
        ensure_positions_ready(ui)?;
        self.save(Some(config), ui).map(|_| ())
    }
}

/// Explicit saves must wait until migrated native-window positions refer to their content.
/// Check before capturing a named layout too, so a later background save cannot persist
/// a snapshot made with the old outer-window positions.
pub(crate) fn ensure_positions_ready(ui: &UiState) -> Result<(), String> {
    if !ui.native_outer.is_empty() {
        return Err("workspace window positions are still being restored; try saving again in a moment".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests;
